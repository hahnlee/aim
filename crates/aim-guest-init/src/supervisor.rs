//! init's service state machine (system/core/init/service.cpp `Start`,
//! `StopOrReset`, `Restart`, `Reap`, `Enable`, `ExecStart`,
//! `NotifyStateChange`; init.cpp `HandleProcessActions`), over a
//! [`Launcher`].
//!
//! The supervisor never touches properties or the action queue itself: it
//! emits [`SupervisorEvent`]s (property sets, `onrestart` commands, the end
//! of an `exec` service) that its owner applies in order.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aim_android_init::PropertyLookup;
use aim_android_init::engine::ExecSpec;
use aim_android_init::rc::{CommandSpec, IdResolver, Service, expand_props};

use crate::identity::{Capabilities, Identity};
use crate::launch::{
    FIRST_DESCRIPTOR_FD, FileSpec, LaunchSpec, Launcher, SocketSpec, descriptor_env_name,
};
use crate::paths::{Layout, PathMap};

/// Service flags, with init's values (service.h).
pub mod flags {
    pub const DISABLED: u32 = 0x001;
    pub const ONESHOT: u32 = 0x002;
    pub const RUNNING: u32 = 0x004;
    pub const RESTARTING: u32 = 0x008;
    pub const CONSOLE: u32 = 0x010;
    pub const CRITICAL: u32 = 0x020;
    pub const RESET: u32 = 0x040;
    pub const RC_DISABLED: u32 = 0x080;
    pub const RESTART: u32 = 0x100;
    pub const DISABLED_START: u32 = 0x200;
    pub const EXEC: u32 = 0x400;
    pub const SHUTDOWN_CRITICAL: u32 = 0x800;
    pub const TEMPORARY: u32 = 0x1000;
    pub const GENTLE_KILL: u32 = 0x2000;
}

/// init's crash limit for critical services: more than this many crashes
/// inside the window (or before boot completes) is fatal.
pub const CRITICAL_CRASH_LIMIT: u32 = 4;

/// What the supervisor asks its owner to do, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupervisorEvent {
    /// `SetProperty` from init (`init.svc.*`, `ro.boottime.*`, ...).
    SetProperty { name: String, value: String },
    /// `onrestart_.ExecuteAllCommands()` for a service that is restarting.
    OnRestart {
        service: String,
        commands: Vec<CommandSpec>,
        vendor_subcontext: bool,
    },
    /// The `exec`/`exec_start` service exited: unblock the action queue.
    ExecFinished { service: String },
    /// A critical service crashed too often: init would reboot into the
    /// bootloader (`LOG(FATAL)`).
    Fatal(String),
    /// `reboot_on_failure`: init would shut down with this target.
    Shutdown(String),
    /// A line for init's log.
    Log(String),
}

/// One service's runtime state.
#[derive(Clone, Debug)]
pub struct ServiceRecord {
    pub def: Service,
    pub flags: u32,
    pub pid: Option<u32>,
    pub time_started: Option<Instant>,
    pub time_crashed: Option<Instant>,
    pub crash_count: u32,
    pub starts: u64,
    mount_namespace: std::cell::Cell<Option<crate::service_namespace::Selection>>,
    origin: crate::service_namespace::LaunchOrigin,
    pub was_last_exit_ok: bool,
    /// For `exec` services: the credentials from the command line.
    pub exec_identity: Option<(u32, u32, Vec<u32>, Option<String>)>,
}

impl ServiceRecord {
    fn new(def: Service) -> Self {
        let mut flags = 0;
        if def.disabled {
            flags |= flags::DISABLED | flags::RC_DISABLED;
        }
        if def.oneshot {
            flags |= flags::ONESHOT;
        }
        if def.critical.is_some() {
            flags |= flags::CRITICAL;
        }
        if def.console.is_some() {
            flags |= flags::CONSOLE;
        }
        if def.shutdown_critical {
            flags |= flags::SHUTDOWN_CRITICAL;
        }
        if def.gentle_kill {
            flags |= flags::GENTLE_KILL;
        }
        Self {
            def,
            flags,
            pid: None,
            time_started: None,
            time_crashed: None,
            crash_count: 0,
            starts: 0,
            mount_namespace: std::cell::Cell::new(None),
            origin: crate::service_namespace::LaunchOrigin::Service,
            was_last_exit_ok: true,
            exec_identity: None,
        }
    }

    /// A process init runs itself as root (linkerconfig), outside the
    /// service list.
    pub fn for_helper(def: Service) -> Self {
        let mut record = Self::new(def);
        record.flags |= flags::TEMPORARY;
        record.origin=crate::service_namespace::LaunchOrigin::Helper;
        record.exec_identity = Some((0, 0, Vec::new(), None));
        record
    }

    pub fn is_running(&self) -> bool {
        self.flags & flags::RUNNING != 0
    }

    /// The state name init publishes in `init.svc.<name>`.
    pub fn state(&self) -> &'static str {
        if self.flags & flags::RUNNING != 0 {
            if self.flags & (flags::DISABLED | flags::RESET | flags::RESTART) != 0
                && self.pid.is_some()
            {
                "stopping"
            } else {
                "running"
            }
        } else if self.flags & flags::RESTARTING != 0 {
            "restarting"
        } else {
            "stopped"
        }
    }

    fn crash_window(&self) -> Duration {
        let minutes = self
            .def
            .critical
            .as_ref()
            .map_or(4, |c| c.window_minutes.max(0) as u64);
        Duration::from_secs(minutes * 60)
    }
}

/// Everything besides the service definition a launch needs.
pub struct Planner {
    pub layout: Layout,
    pub map: PathMap,
    pub ids: IdResolver,
    pub vendor_api_level: u32,
    /// init's environment: `PATH` then `export`s, in order.
    pub env: Vec<(String, String)>,
    /// init's own `setrlimit`s, inherited by every service.
    pub rlimits: Vec<aim_android_init::rc::Rlimit>,
    /// `boot_clock` zero (for `ro.boottime.*`).
    pub boot_epoch: Instant,
}

/// `_PATH_DEFPATH`, which first-stage init exports as `PATH`.
pub const DEFAULT_PATH: &str = "/product/bin:/apex/com.android.runtime/bin:/apex/com.android.art/bin:/system_ext/bin:/system/bin:/system/xbin:/odm/bin:/vendor/bin:/vendor/xbin";

impl Planner {
    pub fn set_env(&mut self, name: &str, value: &str) {
        if let Some(entry) = self.env.iter_mut().find(|(k, _)| k == name) {
            entry.1 = value.to_string();
        } else {
            self.env.push((name.to_string(), value.to_string()));
        }
    }

    /// Publishes `env` in the layout's environ file, replaced whole.
    pub fn write_env(&self) -> Result<(), String> {
        let path = self.layout.environ_file();
        let text: String = self.env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, text).map_err(|e| format!("{}: {e}", temp.display()))?;
        std::fs::rename(&temp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `Service::Start` up to `fork`: argument expansion, the program
    /// check, descriptors, environment and credentials.
    pub fn plan(
        &self,
        record: &ServiceRecord,
        properties: &dyn PropertyLookup,
    ) -> Result<LaunchSpec, String> {
        let def = &record.def;
        let mut argv = Vec::with_capacity(def.args.len());
        for (index, arg) in def.args.iter().enumerate() {
            // ExpandArgsAndExecv expands every argument but the program.
            if index == 0 {
                argv.push(arg.clone());
                continue;
            }
            argv.push(
                expand_props(arg, properties, self.vendor_api_level)
                    .map_err(|e| format!("Could not expand argument '{arg}': {e}"))?,
            );
        }
        let program = argv.first().ok_or("service has no program")?;
        let resolved = self.map.resolve(program, true)?;
        if !resolved.host.is_file() {
            return Err(format!(
                "Cannot find '{program}': No such file or directory"
            ));
        }
        let generation = record.starts + 1;
        let mut fd = FIRST_DESCRIPTOR_FD;
        let mut sockets = Vec::new();
        for socket in &def.sockets {
            sockets.push(SocketSpec {
                name: socket.name.clone(),
                socket_type: socket.socket_type,
                passcred: socket.passcred,
                listen: socket.listen,
                perm: socket.perm,
                uid: socket.uid.as_ref().map_or(0, |id| id.id),
                gid: socket.gid.as_ref().map_or(0, |id| id.id),
                host_path: self.layout.socket_dir().join(&socket.name),
                env_name: descriptor_env_name("ANDROID_SOCKET_", &socket.name),
                fd,
            });
            fd += 1;
        }
        let mut files = Vec::new();
        for file in &def.files {
            let resolved = self.map.resolve(&file.name, true)?;
            files.push(FileSpec {
                guest: file.name.clone(),
                host: resolved.host,
                mode: file.mode,
                env_name: descriptor_env_name("ANDROID_FILE_", &file.name),
                fd,
            });
            fd += 1;
        }
        let mut env = self.env.clone();
        for (key, value) in &def.environment {
            if let Some(entry) = env.iter_mut().find(|(k, _)| k == key) {
                entry.1 = value.clone();
            } else {
                env.push((key.clone(), value.clone()));
            }
        }
        for socket in &sockets {
            env.push((socket.env_name.clone(), socket.fd.to_string()));
        }
        for file in &files {
            env.push((file.env_name.clone(), file.fd.to_string()));
        }
        let (uid, gid, groups, seclabel) = match &record.exec_identity {
            Some((uid, gid, groups, seclabel)) => (*uid, *gid, groups.clone(), seclabel.clone()),
            None => (
                def.uid(),
                def.gid(),
                def.supplementary_groups.iter().map(|g| g.id).collect(),
                def.seclabel.clone(),
            ),
        };
        let mut rlimits = self.rlimits.clone();
        for limit in &def.rlimits {
            rlimits.retain(|l| l.resource != limit.resource);
            rlimits.push(*limit);
        }
        let file_stem = sanitize(&def.name);
        let identity = Identity {
            service: def.name.clone(),
            uid,
            gid,
            groups,
            capabilities: Capabilities::for_service(uid, def.capabilities),
            seclabel: seclabel.unwrap_or_default(),
            priority: def.priority,
            oom_score_adjust: def.oom_score_adjust,
            rlimits,
        };
        let origin=record.origin;
        let(namespace,remembered)=crate::service_namespace::plan(&self.layout,&def.name,&argv[0],origin,if origin==crate::service_namespace::LaunchOrigin::Service{record.mount_namespace.get()}else{None})?;
        if remembered.is_some(){record.mount_namespace.set(remembered);}
        Ok(LaunchSpec {
            service: def.name.clone(),
            origin,
            mount_namespace: namespace,
            generation,
            argv,
            env,
            sockets,
            files,
            identity,
            identity_file: self
                .layout
                .identity_dir()
                .join(format!("{file_stem}.{generation}")),
            log_file: self.layout.logs_dir().join(format!("{file_stem}.log")),
        })
    }

    fn socket_host_path(&self, name: &str) -> PathBuf {
        self.layout.socket_dir().join(name)
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-@".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// What a start request did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartOutcome {
    Started { pid: u32, spec: Box<LaunchSpec> },
    AlreadyRunning,
}

/// All services init knows, plus the running `exec` service.
#[derive(Default)]
pub struct Supervisor {
    records: BTreeMap<String, ServiceRecord>,
    by_pid: HashMap<u32, String>,
    /// Interface name (`aidl/foo`) → service.
    interfaces: HashMap<String, String>,
    exec_count: u64,
    delayed: Vec<String>,
    events: Vec<SupervisorEvent>,
    synced: (usize, usize),
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds definitions for services not seen yet and updates the
    /// definitions of known ones, keeping runtime state (the APEX scripts
    /// replace the list at `perform_apex_config`).
    pub fn sync(&mut self, services: &[Service]) {
        let key = (services.as_ptr() as usize, services.len());
        if key == self.synced {
            return;
        }
        self.synced = key;
        for service in services {
            for interface in &service.interfaces {
                self.interfaces
                    .insert(interface.clone(), service.name.clone());
            }
            match self.records.get_mut(&service.name) {
                Some(record) if record.flags & flags::TEMPORARY == 0 => {
                    record.def = service.clone();
                }
                Some(_) => {}
                None => {
                    self.records
                        .insert(service.name.clone(), ServiceRecord::new(service.clone()));
                }
            }
        }
    }

    pub fn record(&self, name: &str) -> Option<&ServiceRecord> {
        self.records.get(name)
    }

    pub fn records(&self) -> impl Iterator<Item = &ServiceRecord> {
        self.records.values()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.records.contains_key(name)
    }

    pub fn service_for_interface(&self, interface: &str) -> Option<&str> {
        self.interfaces.get(interface).map(String::as_str)
    }

    pub fn service_for_pid(&self, pid: u32) -> Option<&str> {
        self.by_pid.get(&pid).map(String::as_str)
    }

    pub fn take_events(&mut self) -> Vec<SupervisorEvent> {
        std::mem::take(&mut self.events)
    }

    /// Services of a class, in name order (`ServiceList` iteration order is
    /// definition order; start order only matters for logs here).
    pub fn class_members(&self, class: &str) -> Vec<String> {
        self.records
            .values()
            .filter(|r| r.def.classnames.contains(class) && r.flags & flags::TEMPORARY == 0)
            .map(|r| r.def.name.clone())
            .collect()
    }

    fn notify(&mut self, name: &str, state: &str, planner: &Planner) {
        let Some(record) = self.records.get(name) else {
            return;
        };
        if record.flags & flags::TEMPORARY != 0 {
            return;
        }
        self.events.push(SupervisorEvent::SetProperty {
            name: format!("init.svc.{name}"),
            value: state.to_string(),
        });
        if state == "running" {
            let started = record.time_started.map_or(0, |t| {
                t.saturating_duration_since(planner.boot_epoch).as_nanos()
            });
            self.events.push(SupervisorEvent::SetProperty {
                name: format!("ro.boottime.{name}"),
                value: started.to_string(),
            });
            self.events.push(SupervisorEvent::SetProperty {
                name: format!("init.svc_debug_pid.{name}"),
                value: record.pid.unwrap_or(0).to_string(),
            });
        } else if state == "stopped" {
            self.events.push(SupervisorEvent::SetProperty {
                name: format!("init.svc_debug_pid.{name}"),
                value: String::new(),
            });
        }
    }

    /// `Service::Start`.
    pub fn start(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
        properties: &dyn PropertyLookup,
        now: Instant,
    ) -> Result<StartOutcome, String> {
        let record = self
            .records
            .get_mut(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        if record.def.updatable&&!crate::service_namespace::ready(&planner.layout.runtime)?{
            if record.flags&flags::EXEC==0{self.delayed.push(name.into());}
            return Err(format!("Cannot start updatable service '{name}' before default linker configuration is ready"));
        }
        let disabled = record.flags & (flags::DISABLED | flags::RESET) != 0;
        record.flags &= !(flags::DISABLED
            | flags::RESTARTING
            | flags::RESET
            | flags::RESTART
            | flags::DISABLED_START);
        if record.flags & flags::RUNNING != 0 {
            if record.flags & flags::ONESHOT != 0 && disabled {
                record.flags |= flags::RESTART;
            }
            return Ok(StartOutcome::AlreadyRunning);
        }
        let spec = match planner.plan(record, properties) {
            Ok(spec) => spec,
            Err(error) => {
                record.flags |= flags::DISABLED;
                return Err(error);
            }
        };
        let pid = match launcher.launch(&spec) {
            Ok(pid) => pid,
            Err(error) => {
                record.flags |= flags::DISABLED;
                return Err(format!("Failed to start '{name}': {error}"));
            }
        };
        record.pid = Some(pid);
        record.flags |= flags::RUNNING;
        record.time_started = Some(now);
        record.starts += 1;
        self.by_pid.insert(pid, name.to_string());
        self.notify(name, "running", planner);
        Ok(StartOutcome::Started {
            pid,
            spec: Box::new(spec),
        })
    }

    pub fn take_delayed(&mut self)->Vec<String>{std::mem::take(&mut self.delayed).into_iter().collect()}

    /// `Service::StartIfNotDisabled` (`class_start`).
    pub fn start_if_not_disabled(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
        properties: &dyn PropertyLookup,
        now: Instant,
    ) -> Result<Option<StartOutcome>, String> {
        let record = self
            .records
            .get_mut(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        if record.flags & flags::DISABLED == 0 {
            self.start(name, launcher, planner, properties, now)
                .map(Some)
        } else {
            record.flags |= flags::DISABLED_START;
            Ok(None)
        }
    }

    /// `Service::Enable`.
    pub fn enable(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
        properties: &dyn PropertyLookup,
        now: Instant,
    ) -> Result<Option<StartOutcome>, String> {
        let record = self
            .records
            .get_mut(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        record.flags &= !(flags::DISABLED | flags::RC_DISABLED);
        if record.flags & flags::DISABLED_START != 0 {
            return self
                .start(name, launcher, planner, properties, now)
                .map(Some);
        }
        Ok(None)
    }

    /// `Service::StopOrReset`.
    fn stop_or_reset(
        &mut self,
        name: &str,
        how: u32,
        launcher: &mut dyn Launcher,
        planner: &Planner,
    ) -> Result<(), String> {
        let record = self
            .records
            .get_mut(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        record.flags &= !(flags::RESTARTING | flags::DISABLED_START);
        let how = if matches!(how, flags::DISABLED | flags::RESET | flags::RESTART) {
            how
        } else {
            flags::DISABLED
        };
        if how == flags::RESET {
            record.flags |= if record.flags & flags::RC_DISABLED != 0 {
                flags::DISABLED
            } else {
                flags::RESET
            };
        } else {
            record.flags |= how;
        }
        if let Some(pid) = record.pid {
            let signal = if record.flags & flags::GENTLE_KILL != 0 {
                libc::SIGTERM
            } else {
                libc::SIGKILL
            };
            launcher.kill_group(pid, signal);
            self.notify(name, "stopping", planner);
        } else {
            self.notify(name, "stopped", planner);
        }
        Ok(())
    }

    /// `Service::Stop`.
    pub fn stop(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
    ) -> Result<(), String> {
        self.stop_or_reset(name, flags::DISABLED, launcher, planner)
    }

    /// `Service::Reset` (`class_reset`).
    pub fn reset(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
    ) -> Result<(), String> {
        self.stop_or_reset(name, flags::RESET, launcher, planner)
    }

    /// `Service::Restart`.
    pub fn restart(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
        properties: &dyn PropertyLookup,
        now: Instant,
    ) -> Result<Option<StartOutcome>, String> {
        let record = self
            .records
            .get(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        if record.flags & flags::RUNNING != 0 {
            self.stop_or_reset(name, flags::RESTART, launcher, planner)?;
            Ok(None)
        } else if record.flags & flags::RESTARTING == 0 {
            self.start(name, launcher, planner, properties, now)
                .map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn set_oneshot(&mut self, name: &str, oneshot: bool) -> Result<(), String> {
        let record = self
            .records
            .get_mut(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        if oneshot {
            record.flags |= flags::ONESHOT;
        } else {
            record.flags &= !flags::ONESHOT;
        }
        Ok(())
    }

    /// `Service::MakeTemporaryOneshotService`: `exec` creates a temporary
    /// service named `exec N (args)`.
    pub fn add_exec_service(
        &mut self,
        spec: &ExecSpec,
        ids: &IdResolver,
    ) -> Result<String, String> {
        self.exec_count += 1;
        let name = format!("exec {} ({})", self.exec_count, spec.args.join(" "));
        let uid = spec
            .user
            .as_deref()
            .map(|u| ids.decode_uid(u))
            .transpose()?
            .unwrap_or(0);
        let gid = spec
            .group
            .as_deref()
            .map(|g| ids.decode_gid(g))
            .transpose()?
            .unwrap_or(0);
        let groups = spec
            .supplementary_groups
            .iter()
            .map(|g| ids.decode_gid(g))
            .collect::<Result<Vec<_>, _>>()?;
        let mut def = template_service(&name, spec.args.clone());
        def.oneshot = true;
        let mut record = ServiceRecord::new(def);
        record.flags |= flags::TEMPORARY;
        record.origin=crate::service_namespace::LaunchOrigin::Transient;
        record.exec_identity = Some((uid, gid, groups, spec.seclabel.clone()));
        self.records.insert(name.clone(), record);
        Ok(name)
    }

    /// `Service::ExecStart`: marks the service as the blocking exec
    /// service, then starts it.
    pub fn exec_start(
        &mut self,
        name: &str,
        launcher: &mut dyn Launcher,
        planner: &Planner,
        properties: &dyn PropertyLookup,
        now: Instant,
    ) -> Result<StartOutcome, String> {
        let record = self
            .records
            .get_mut(name)
            .ok_or_else(|| format!("Could not find service '{name}'"))?;
        if record.def.updatable&&!crate::service_namespace::ready(&planner.layout.runtime)?{return Err(format!("Cannot exec updatable service '{name}' before default linker configuration is ready"));}
        record.flags |= flags::ONESHOT | flags::EXEC;
        match self.start(name, launcher, planner, properties, now) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                if let Some(record) = self.records.get_mut(name) {
                    record.flags &= !flags::EXEC;
                    if record.flags & flags::TEMPORARY != 0 {
                        self.records.remove(name);
                    }
                }
                Err(error)
            }
        }
    }

    /// Removes a temporary service that could not start.
    pub fn discard_temporary(&mut self, name: &str) {
        if self
            .records
            .get(name)
            .is_some_and(|r| r.flags & flags::TEMPORARY != 0 && r.pid.is_none())
        {
            self.records.remove(name);
        }
    }

    /// `Service::Reap` for the process `pid`. Returns false for a pid that
    /// is not a service.
    pub fn reap(
        &mut self,
        pid: u32,
        exit: crate::launch::Exit,
        launcher: &mut dyn Launcher,
        planner: &Planner,
        properties: &dyn PropertyLookup,
        now: Instant,
    ) -> bool {
        let Some(name) = self.by_pid.remove(&pid) else {
            return false;
        };
        let Some(record) = self.records.get_mut(&name) else {
            return false;
        };
        self.events.push(SupervisorEvent::Log(format!(
            "Service '{name}' (pid {pid}) {exit}"
        )));
        // Kill the rest of the group; since R also for oneshot services.
        launcher.kill_group(pid, libc::SIGKILL);
        for socket in &record.def.sockets {
            let _ = std::fs::remove_file(planner.socket_host_path(&socket.name));
        }
        if !exit.is_success()
            && let Some(target) = record.def.reboot_on_failure.clone()
        {
            self.events.push(SupervisorEvent::Log(
                "Service with 'reboot_on_failure' option failed, shutting down system.".to_string(),
            ));
            self.events.push(SupervisorEvent::Shutdown(target));
        }
        if record.flags & flags::EXEC != 0 {
            record.flags &= !flags::EXEC;
            self.events.push(SupervisorEvent::ExecFinished {
                service: name.clone(),
            });
        }
        if record.flags & flags::TEMPORARY != 0 {
            self.records.remove(&name);
            return true;
        }
        record.pid = None;
        record.flags &= !flags::RUNNING;
        record.was_last_exit_ok = exit.is_success();
        if record.flags & flags::ONESHOT != 0
            && record.flags & flags::RESTART == 0
            && record.flags & flags::RESET == 0
        {
            record.flags |= flags::DISABLED;
        }
        if record.flags & (flags::DISABLED | flags::RESET) != 0 {
            self.notify(&name, "stopped", planner);
            return true;
        }
        if record.flags & flags::CRITICAL != 0
            && record.flags & flags::RESTART == 0
            && !record.was_last_exit_ok
        {
            let boot_completed = properties.property_or("sys.boot_completed", "") == "1";
            let in_window = record
                .time_crashed
                .is_some_and(|t| now < t + record.crash_window());
            if in_window || !boot_completed {
                record.crash_count += 1;
                if record.crash_count > CRITICAL_CRASH_LIMIT {
                    let reason = if boot_completed {
                        format!("in {} minutes", record.crash_window().as_secs() / 60)
                    } else {
                        "before boot completed".to_string()
                    };
                    let no_fatal = properties
                        .property_or(&format!("init.svc_debug.no_fatal.{name}"), "")
                        == "true";
                    if !no_fatal {
                        self.events.push(SupervisorEvent::Fatal(format!(
                            "critical process '{name}' exited {CRITICAL_CRASH_LIMIT} times {reason}"
                        )));
                    }
                }
            } else {
                record.time_crashed = Some(now);
                record.crash_count = 1;
            }
        }
        record.flags &= !flags::RESTART;
        record.flags |= flags::RESTARTING;
        let commands = record.def.onrestart.clone();
        let vendor = record.def.vendor_subcontext;
        if !commands.is_empty() {
            self.events.push(SupervisorEvent::OnRestart {
                service: name.clone(),
                commands,
                vendor_subcontext: vendor,
            });
        }
        self.notify(&name, "restarting", planner);
        true
    }

    /// `HandleProcessActions`: services whose restart time has come (to be
    /// started by the owner), running services past `timeout_period`
    /// (killed here), and the next time something is due.
    pub fn process_actions(
        &mut self,
        launcher: &mut dyn Launcher,
        now: Instant,
    ) -> (Vec<String>, Option<Instant>) {
        let mut due = Vec::new();
        let mut next: Option<Instant> = None;
        let mut soonest = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));
        for record in self.records.values_mut() {
            if record.flags & flags::RUNNING != 0
                && let (Some(timeout), Some(started), Some(pid)) =
                    (record.def.timeout_period, record.time_started, record.pid)
            {
                let deadline = started + Duration::from_secs(timeout);
                if now > deadline {
                    launcher.kill_group(pid, libc::SIGKILL);
                } else {
                    soonest(deadline);
                }
            }
            if record.flags & flags::RESTARTING == 0 {
                continue;
            }
            let restart_at =
                record.time_started.unwrap_or(now) + Duration::from_secs(record.def.restart_period);
            if now > restart_at {
                due.push(record.def.name.clone());
            } else {
                soonest(restart_at);
            }
        }
        (due, next)
    }

    /// Kills every running service (shutdown). They are stopped, as init's
    /// shutdown stops them, so their exits run no `onrestart` and nothing
    /// restarts.
    pub fn kill_all(&mut self, launcher: &mut dyn Launcher) {
        for record in self.records.values_mut() {
            record.flags |= flags::DISABLED;
            record.flags &= !flags::RESTARTING;
            if let Some(pid) = record.pid {
                launcher.kill_group(pid, libc::SIGKILL);
            }
        }
    }
}

/// A service definition with init's defaults, for `exec` services.
pub fn template_service(name: &str, args: Vec<String>) -> Service {
    Service {
        name: name.to_string(),
        args,
        filename: "<exec>".to_string(),
        line: 0,
        classnames: BTreeSet::from(["default".to_string()]),
        disabled: false,
        oneshot: false,
        critical: None,
        user: None,
        group: None,
        supplementary_groups: Vec::new(),
        capabilities: None,
        sockets: Vec::new(),
        files: Vec::new(),
        onrestart: Vec::new(),
        seclabel: None,
        writepid_files: Vec::new(),
        task_profiles: Vec::new(),
        interfaces: BTreeSet::new(),
        ioprio_class: Default::default(),
        ioprio_priority: 0,
        priority: 0,
        rlimits: Vec::new(),
        environment: Vec::new(),
        namespaces: Default::default(),
        updatable: false,
        is_override: false,
        restart_period: 5,
        timeout_period: None,
        // DEFAULT_OOM_SCORE_ADJUST (lmkd_service.h).
        oom_score_adjust: -1000,
        console: None,
        stdio_to_kmsg: false,
        gentle_kill: false,
        sigstop: false,
        shutdown_critical: false,
        reboot_on_failure: None,
        other_options: Vec::new(),
        vendor_subcontext: false,
    }
}
