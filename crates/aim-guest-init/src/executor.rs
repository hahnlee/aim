//! The real [`CommandExecutor`]: init's builtins as aimd performs
//! them. Service commands go to the [`Supervisor`], filesystem commands to
//! [`FsOps`], and everything the Darwin host has no counterpart for is a
//! logged [`Effect::NoOp`] with its reason.
//!
//! Roles aimd plays itself instead of starting the original daemon:
//!
//! - `ueventd`: no device nodes are created; `ro.cold_boot_done` is set
//!   before the boot starts.
//! - `apexd-bootstrap`, `apexd-snapshotde`: APEXes are pre-flattened;
//!   `apexd.status` goes to `ready` when `apexd-snapshotde` would run.
//!   `apexd` itself does start: the derived image's replacement
//!   (`daemons/apexd`) serves `apexservice` from the APEX info list and
//!   reports `activated` and `apex.all.ready`.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aim_android_init::PropertyLookup;
use aim_android_init::engine::{
    Command, CommandExecutor, CommandFlow, ExecSpec, InitAction, InitProperties, Invocation,
    PERSISTENT_PROPERTIES_READY_PROP,
};
use aim_android_init::props::SetEffect;
use aim_android_init::props::service::ControlMessage;
use aim_android_init::rc::{CommandSpec, Rlimit, Service, expand_props};

use crate::fsops::{Effect, FsOps, parse_mode};
use crate::launch::{Exit, LaunchSpec, Launcher};
use crate::props::{Properties, decode_persistent_properties, encode_persistent_properties};
use crate::supervisor::{Planner, StartOutcome, Supervisor, SupervisorEvent, template_service};

/// Something the boot loop must hand to the action manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outgoing {
    PropertyChanged(String, String),
    Trigger(String),
    ExecFinished,
}

/// Services whose role aimd plays.
pub const ROLE_SERVICES: &[&str] = &["ueventd", "apexd-bootstrap", "apexd-snapshotde"];

/// Linux resource limits Darwin has no counterpart for.
const LINUX_ONLY_RLIMITS: &[(u32, &str)] = &[
    (10, "RLIMIT_LOCKS"),
    (11, "RLIMIT_SIGPENDING"),
    (12, "RLIMIT_MSGQUEUE"),
    (13, "RLIMIT_NICE"),
    (14, "RLIMIT_RTPRIO"),
    (15, "RLIMIT_RTTIME"),
];

/// `/data/property/persistent_properties`.
pub const PERSISTENT_PROPERTY_FILE: &str = "/data/property/persistent_properties";

/// How a process launch is reported.
#[derive(Clone, Debug)]
pub struct LaunchRecord {
    pub pid: u32,
    pub spec: LaunchSpec,
    /// A helper init runs itself (linkerconfig), not a service.
    pub helper: bool,
}

pub struct GuestExecutor {
    pub props: Rc<RefCell<Properties>>,
    pub supervisor: Supervisor,
    pub launcher: Box<dyn Launcher>,
    pub planner: Planner,
    pub fs: FsOps,
    /// `--only`: services outside the set are not started.
    pub only: Option<BTreeSet<String>>,
    /// `--exclude`: services (and `exec` programs) in the set are not
    /// started.
    pub exclude: BTreeSet<String>,
    pub outbox: VecDeque<Outgoing>,
    pub launches: Vec<LaunchRecord>,
    /// Effects of the command being executed.
    current: Vec<Effect>,
    /// init's log lines (reaps, restarts, errors outside commands).
    pub log: Vec<String>,
    pub fatal: Option<String>,
    pub shutdown: Option<String>,
    /// Exits observed while waiting for a helper, handled afterwards.
    deferred_exits: Vec<(u32, Exit)>,
    /// The running exec service's pid.
    pub exec_pid: Option<u32>,
    pub helper_timeout: Duration,
    /// The boot's deadline (`--timeout`): a wait for a helper ends there
    /// too, as it does when a stop is requested.
    pub stop_at: Option<Instant>,
}

impl GuestExecutor {
    pub fn new(
        props: Rc<RefCell<Properties>>,
        launcher: Box<dyn Launcher>,
        planner: Planner,
        fs: FsOps,
        only: Option<BTreeSet<String>>,
    ) -> Self {
        Self {
            props,
            supervisor: Supervisor::new(),
            launcher,
            planner,
            fs,
            only,
            exclude: BTreeSet::new(),
            outbox: VecDeque::new(),
            launches: Vec::new(),
            current: Vec::new(),
            log: Vec::new(),
            fatal: None,
            shutdown: None,
            deferred_exits: Vec::new(),
            exec_pid: None,
            helper_timeout: Duration::from_secs(60),
            stop_at: None,
        }
    }

    /// Effects of the last executed command.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.current)
    }

    fn effect(&mut self, effect: Effect) {
        self.current.push(effect);
    }

    /// Makes the supervisor aware of a service list (the engine's list at
    /// the last command, or the one passed here).
    pub fn sync_services(&mut self, services: &[Service]) {
        self.supervisor.sync(services);
    }

    /// init setting a property (`SetProperty` from init's main thread).
    pub fn set_property(&mut self, name: &str, value: &str) -> Result<(), String> {
        let outcome = self.props.borrow_mut().init_set(name, value);
        if !outcome.is_success() {
            return Err(outcome.error.unwrap_or_default());
        }
        for effect in outcome.effects {
            if let SetEffect::Changed { name, value } = effect {
                self.outbox
                    .push_back(Outgoing::PropertyChanged(name, value));
            }
        }
        Ok(())
    }

    /// Why `--only`/`--exclude` keep a service or `exec` program from
    /// starting.
    fn filtered(&self, name: &str) -> Option<String> {
        if self.exclude.contains(name) {
            Some(format!("'{name}' excluded"))
        } else if self.only.as_ref().is_some_and(|only| !only.contains(name)) {
            Some(format!("'{name}' not in --only"))
        } else {
            None
        }
    }

    fn now() -> Instant {
        Instant::now()
    }

    fn record_start(&mut self, outcome: StartOutcome) -> Effect {
        match outcome {
            StartOutcome::Started { pid, spec } => {
                let service = spec.service.clone();
                if spec.service.starts_with("exec ") {
                    self.exec_pid.get_or_insert(pid);
                }
                self.launches.push(LaunchRecord {
                    pid,
                    spec: *spec,
                    helper: false,
                });
                Effect::Launch { service, pid }
            }
            StartOutcome::AlreadyRunning => Effect::Applied("already running".to_string()),
        }
    }

    /// The role aimd plays for `name`, if any.
    fn play_role(&mut self, name: &str) -> Option<Effect> {
        match name {
            "ueventd" => Some(Effect::NoOp(
                "ueventd role: guest-init creates no device nodes and set ro.cold_boot_done=true before boot".to_string(),
            )),
            "apexd-bootstrap" => Some(Effect::NoOp(
                "apexd role: bootstrap APEXes are pre-flattened into the derived image".to_string(),
            )),
            "apexd-snapshotde" => {
                // apexd's OnAllPackagesReady (apex.all.ready was set when
                // the packages were activated).
                let _ = self.set_property("apexd.status", "ready");
                Some(Effect::Applied(
                    "apexd role: no DE snapshot to take; apexd.status=ready"
                        .to_string(),
                ))
            }
            _ => None,
        }
    }

    /// Checks shared by every way a service gets started.
    fn pre_start(&mut self, name: &str) -> Option<Effect> {
        if let Some(effect) = self.play_role(name) {
            return Some(effect);
        }
        if !self.supervisor.contains(name) {
            return Some(Effect::NoOp(format!(
                "service '{name}' is not declared (hardware or feature not declared on this device)"
            )));
        }
        if let Some(reason) = self.filtered(name) {
            return Some(Effect::Skipped(reason));
        }
        None
    }

    /// `start` / `ctl.start`.
    pub fn start_service(&mut self, name: &str) -> Result<Effect, String> {
        if let Some(effect) = self.pre_start(name) {
            return Ok(effect);
        }
        let props = self.props.clone();
        let props = props.borrow();
        let outcome = self.supervisor.start(
            name,
            self.launcher.as_mut(),
            &self.planner,
            &*props,
            Self::now(),
        );
        drop(props);
        let outcome = outcome?;
        Ok(self.record_start(outcome))
    }

    pub fn stop_service(&mut self, name: &str) -> Result<Effect, String> {
        if ROLE_SERVICES.contains(&name) || !self.supervisor.contains(name) {
            return Ok(Effect::NoOp(format!(
                "stop '{name}': no process (role or undeclared)"
            )));
        }
        self.supervisor
            .stop(name, self.launcher.as_mut(), &self.planner)?;
        Ok(Effect::Applied(format!("stop '{name}'")))
    }

    pub fn restart_service(&mut self, name: &str, only_if_running: bool) -> Result<Effect, String> {
        // Role services have no process, so they never count as running.
        let running = self
            .supervisor
            .record(name)
            .is_some_and(|record| record.is_running());
        if only_if_running && !running {
            return Ok(Effect::NoOp(format!(
                "restart --only-if-running '{name}': not running"
            )));
        }
        if let Some(effect) = self.pre_start(name) {
            return Ok(effect);
        }
        let props = self.props.clone();
        let props = props.borrow();
        let outcome = self.supervisor.restart(
            name,
            self.launcher.as_mut(),
            &self.planner,
            &*props,
            Self::now(),
        );
        drop(props);
        match outcome? {
            Some(outcome) => Ok(self.record_start(outcome)),
            None => Ok(Effect::Applied(format!(
                "restart '{name}' (restarts on exit)"
            ))),
        }
    }

    fn enable_service(&mut self, name: &str) -> Result<Effect, String> {
        if let Some(effect) = self.pre_start(name) {
            return Ok(effect);
        }
        let props = self.props.clone();
        let props = props.borrow();
        let outcome = self.supervisor.enable(
            name,
            self.launcher.as_mut(),
            &self.planner,
            &*props,
            Self::now(),
        );
        drop(props);
        match outcome? {
            Some(outcome) => Ok(self.record_start(outcome)),
            None => Ok(Effect::Applied(format!("enable '{name}'"))),
        }
    }

    fn class_start(&mut self, class: &str) -> Result<(), String> {
        if self
            .props
            .borrow()
            .property_or(&format!("persist.init.dont_start_class.{class}"), "")
            == "true"
        {
            self.effect(Effect::NoOp(format!(
                "persist.init.dont_start_class.{class} is set"
            )));
            return Ok(());
        }
        for name in self.supervisor.class_members(class) {
            let disabled = self
                .supervisor
                .record(&name)
                .is_some_and(|r| r.flags & crate::supervisor::flags::DISABLED != 0);
            if !disabled && let Some(effect) = self.pre_start(&name) {
                self.effect(effect);
                continue;
            }
            let props = self.props.clone();
            let props = props.borrow();
            let outcome = self.supervisor.start_if_not_disabled(
                &name,
                self.launcher.as_mut(),
                &self.planner,
                &*props,
                Self::now(),
            );
            drop(props);
            match outcome {
                Ok(Some(outcome)) => {
                    let effect = self.record_start(outcome);
                    self.effect(effect);
                }
                Ok(None) => {}
                Err(error) => {
                    self.effect(Effect::NoOp(format!("could not start '{name}': {error}")))
                }
            }
        }
        Ok(())
    }

    fn for_class(&mut self, class: &str, what: &str, only_enabled: bool) -> Result<(), String> {
        for name in self.supervisor.class_members(class) {
            if only_enabled
                && self
                    .supervisor
                    .record(&name)
                    .is_some_and(|r| r.flags & crate::supervisor::flags::DISABLED != 0)
            {
                continue;
            }
            let result = match what {
                "stop" => self
                    .supervisor
                    .stop(&name, self.launcher.as_mut(), &self.planner)
                    .map(|()| None),
                "reset" => self
                    .supervisor
                    .reset(&name, self.launcher.as_mut(), &self.planner)
                    .map(|()| None),
                _ => {
                    if self.filtered(&name).is_some() || ROLE_SERVICES.contains(&name.as_str()) {
                        continue;
                    }
                    let props = self.props.clone();
                    let props = props.borrow();
                    let outcome = self.supervisor.restart(
                        &name,
                        self.launcher.as_mut(),
                        &self.planner,
                        &*props,
                        Self::now(),
                    );
                    drop(props);
                    outcome
                }
            };
            match result {
                Ok(Some(outcome)) => {
                    let effect = self.record_start(outcome);
                    self.effect(effect);
                }
                Ok(None) => {}
                Err(error) => self.effect(Effect::NoOp(format!("{what} '{name}': {error}"))),
            }
        }
        self.effect(Effect::Applied(format!("class_{what} {class}")));
        Ok(())
    }

    fn exec(&mut self, spec: &ExecSpec, blocking: bool) -> Result<CommandFlow, String> {
        let name = self.supervisor.add_exec_service(spec, &self.planner.ids)?;
        let program = spec.args[0].rsplit('/').next().unwrap_or("").to_string();
        if let Some(reason) = self.filtered(&program) {
            self.supervisor.discard_temporary(&name);
            self.effect(Effect::Skipped(format!("{name}: {reason}")));
            return Ok(CommandFlow::Done);
        }
        let props = self.props.clone();
        let props = props.borrow();
        let outcome = if blocking {
            self.supervisor.exec_start(
                &name,
                self.launcher.as_mut(),
                &self.planner,
                &*props,
                Self::now(),
            )
        } else {
            self.supervisor.start(
                &name,
                self.launcher.as_mut(),
                &self.planner,
                &*props,
                Self::now(),
            )
        };
        drop(props);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.supervisor.discard_temporary(&name);
                return Err(error);
            }
        };
        let effect = self.record_start(outcome);
        self.effect(effect);
        Ok(if blocking {
            CommandFlow::ExecServiceRunning
        } else {
            CommandFlow::Done
        })
    }

    fn exec_start(&mut self, name: &str) -> Result<CommandFlow, String> {
        if let Some(effect) = self.pre_start(name) {
            self.effect(effect);
            return Ok(CommandFlow::Done);
        }
        let props = self.props.clone();
        let props = props.borrow();
        let outcome = self.supervisor.exec_start(
            name,
            self.launcher.as_mut(),
            &self.planner,
            &*props,
            Self::now(),
        );
        drop(props);
        let outcome = outcome?;
        if let StartOutcome::Started { pid, .. } = &outcome {
            self.exec_pid = Some(*pid);
        }
        let effect = self.record_start(outcome);
        self.effect(effect);
        Ok(CommandFlow::ExecServiceRunning)
    }

    /// `GenerateLinkerConfiguration`: runs the original linkerconfig and
    /// waits for it, as init's `logwrap_fork_execvp` does.
    fn update_linker_config(&mut self) -> Result<(), String> {
        let def = template_service(
            "linkerconfig",
            vec![
                "/apex/com.android.runtime/bin/linkerconfig".to_string(),
                "--target".to_string(),
                "/linkerconfig".to_string(),
            ],
        );
        let mut record_def = crate::supervisor::ServiceRecord::for_helper(def);
        record_def.starts = self.launches.iter().filter(|l| l.helper).count() as u64;
        let spec = {
            let props = self.props.borrow();
            self.planner.plan(&record_def, &*props)?
        };
        let pid = self.launcher.launch(&spec)?;
        self.launches.push(LaunchRecord {
            pid,
            spec,
            helper: true,
        });
        if !self.fs.apply {
            self.effect(Effect::Launch {
                service: "linkerconfig".to_string(),
                pid,
            });
            return Ok(());
        }
        let deadline = Instant::now() + self.helper_timeout;
        loop {
            for (exited, exit) in self.launcher.reap() {
                if exited == pid {
                    self.effect(Effect::Launch {
                        service: "linkerconfig".to_string(),
                        pid,
                    });
                    return if exit.is_success() {
                        Ok(())
                    } else {
                        Err(format!("linkerconfig {exit}"))
                    };
                }
                self.deferred_exits.push((exited, exit));
            }
            let stopping =
                crate::boot::stop_requested() || self.stop_at.is_some_and(|d| Instant::now() >= d);
            if stopping || Instant::now() > deadline {
                self.launcher.kill_group(pid, libc::SIGKILL);
                return Err(if stopping {
                    "linkerconfig stopped: the boot is ending".to_string()
                } else {
                    "linkerconfig did not finish in time".to_string()
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn set_rlimit(&mut self, limit: &Rlimit) -> Effect {
        self.planner
            .rlimits
            .retain(|l| l.resource != limit.resource);
        self.planner.rlimits.push(*limit);
        match LINUX_ONLY_RLIMITS
            .iter()
            .find(|(r, _)| *r == limit.resource)
        {
            Some((_, name)) => Effect::NoOp(format!(
                "setrlimit: Darwin has no {name}; recorded in identity files for getrlimit only"
            )),
            None => Effect::Recorded(format!(
                "setrlimit {} {} {}: recorded in identity files (services inherit init's limits)",
                limit.resource, limit.soft, limit.hard
            )),
        }
    }

    fn load_exports(&mut self, path: &str) -> Result<Effect, String> {
        let resolved = self.fs.map.resolve(path, true)?;
        let text = match std::fs::read_to_string(&resolved.host) {
            Ok(text) => text,
            Err(error) if !self.fs.apply => {
                return Ok(Effect::NoOp(format!(
                    "load_exports {path}: not readable in a dry run ({error})"
                )));
            }
            Err(error) => return Err(format!("Could not read {path}: {error}")),
        };
        let mut count = 0;
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if let ["export", name, value] = fields[..] {
                self.planner.set_env(name, value);
                count += 1;
            }
        }
        if self.fs.apply {
            self.planner.write_env()?;
        }
        Ok(Effect::Recorded(format!("{count} exports from {path}")))
    }

    fn persistent_file(&self) -> Option<PathBuf> {
        self.fs
            .map
            .resolve(PERSISTENT_PROPERTY_FILE, true)
            .ok()
            .map(|r| r.host)
    }

    /// `load_persist_props`.
    fn load_persist_props(&mut self) -> Result<Effect, String> {
        let mut loaded = 0;
        if let Some(path) = self.persistent_file()
            && let Ok(bytes) = std::fs::read(&path)
        {
            for (name, value) in decode_persistent_properties(&bytes)? {
                if !name.starts_with("persist.") {
                    continue;
                }
                let outcome = self.props.borrow_mut().set_no_socket(&name, &value);
                for effect in outcome.effects {
                    if let SetEffect::Changed { name, value } = effect {
                        self.outbox
                            .push_back(Outgoing::PropertyChanged(name, value));
                    }
                }
                loaded += 1;
            }
        }
        self.props.borrow_mut().persistent_properties_loaded = true;
        self.set_property(PERSISTENT_PROPERTIES_READY_PROP, "true")?;
        Ok(Effect::Applied(format!(
            "loaded {loaded} persistent properties from {PERSISTENT_PROPERTY_FILE}"
        )))
    }

    /// Writes every `persist.*` property (`WritePersistentProperty`).
    pub fn write_persistent_properties(&self) -> Result<(), String> {
        let Some(path) = self.persistent_file() else {
            return Ok(());
        };
        let props: Vec<(String, String)> = self
            .props
            .borrow()
            .areas()
            .foreach()
            .into_iter()
            .filter(|(name, _)| name.starts_with("persist."))
            .collect();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, encode_persistent_properties(&props)).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &path).map_err(|e| e.to_string())
    }

    fn mount_all(&mut self, args: &[String]) -> Result<Effect, String> {
        let mode = if args.iter().any(|a| a == "--early") {
            "early"
        } else if args.iter().any(|a| a == "--late") {
            "late"
        } else {
            "default"
        };
        let _ = self.set_property(&format!("ro.boottime.init.mount_all.{mode}"), "0");
        if mode == "early" {
            return Ok(Effect::NoOp(
                "mount_all --early: partitions are pre-extracted into the derived image"
                    .to_string(),
            ));
        }
        // userdata "mounted": queue_fs_event(FS_MGR_MNTALL_DEV_NOT_ENCRYPTED).
        let _ = self.set_property("ro.crypto.state", "unencrypted");
        self.outbox
            .push_back(Outgoing::Trigger("nonencrypted".to_string()));
        Ok(Effect::NoOp(format!(
            "mount_all ({mode}): partitions are pre-extracted and /data is a host directory without FBE; ro.crypto.state=unencrypted, trigger nonencrypted"
        )))
    }

    fn perform_apex_config(&mut self, bootstrap: bool) -> Result<(), String> {
        if !bootstrap {
            // CreateApexDataDirs.
            let names = self
                .fs
                .map
                .resolve("/apex", true)
                .ok()
                .and_then(|r| std::fs::read_dir(r.host).ok())
                .map(|read| {
                    let mut names: Vec<String> = read
                        .flatten()
                        .filter(|e| e.path().join("apex_manifest.pb").exists())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| !n.contains('@'))
                        .collect();
                    names.sort();
                    names
                })
                .unwrap_or_default();
            let system_gid = self.planner.ids.decode_gid("system").unwrap_or(1000);
            let mut made = 0;
            for name in &names {
                if self
                    .fs
                    .mkdir(
                        &format!("/data/misc/apexdata/{name}"),
                        Some(0o771),
                        Some(0),
                        Some(system_gid),
                    )
                    .is_ok()
                {
                    made += 1;
                }
            }
            self.effect(Effect::Applied(format!(
                "created /data/misc/apexdata for {made} of {} APEXes",
                names.len()
            )));
        }
        self.update_linker_config()
    }

    /// Runs `onrestart` commands (`Action::ExecuteAllCommands` on the
    /// service's `onrestart_` action).
    fn run_onrestart(&mut self, service: &str, commands: &[CommandSpec]) {
        for spec in commands {
            let mut expanded = vec![spec.args[0].clone()];
            let mut failed = None;
            for arg in &spec.args[1..] {
                let props = self.props.borrow();
                match expand_props(arg, &*props, self.planner.vendor_api_level) {
                    Ok(v) => expanded.push(v),
                    Err(e) => failed = Some(e),
                }
            }
            let result = match failed {
                Some(error) => Err(error),
                None => Command::from_args(spec.builtin, &expanded)
                    .and_then(|command| self.run_command(&command).map(|_| ())),
            };
            let effects = self.take_effects();
            self.log.push(format!(
                "onrestart '{service}': {} -> {}",
                spec.command_string(),
                match &result {
                    Ok(()) => effects
                        .iter()
                        .map(|e| format!("[{}] {}", e.kind(), e.text()))
                        .collect::<Vec<_>>()
                        .join("; "),
                    Err(error) => format!("error: {error}"),
                }
            ));
        }
    }

    /// Applies the supervisor's queued events.
    pub fn drain_supervisor(&mut self) {
        loop {
            let events = self.supervisor.take_events();
            if events.is_empty() {
                return;
            }
            for event in events {
                match event {
                    SupervisorEvent::SetProperty { name, value } => {
                        let _ = self.set_property(&name, &value);
                    }
                    SupervisorEvent::OnRestart {
                        service, commands, ..
                    } => {
                        let saved = std::mem::take(&mut self.current);
                        self.run_onrestart(&service, &commands);
                        self.current = saved;
                    }
                    SupervisorEvent::ExecFinished { service } => {
                        self.log.push(format!("exec service '{service}' finished"));
                        self.exec_pid = None;
                        self.outbox.push_back(Outgoing::ExecFinished);
                    }
                    SupervisorEvent::Fatal(reason) => {
                        self.log.push(format!(
                            "FATAL: {reason} (init would reboot into the bootloader)"
                        ));
                        self.fatal.get_or_insert(reason);
                    }
                    SupervisorEvent::Shutdown(target) => {
                        self.shutdown.get_or_insert(target);
                    }
                    SupervisorEvent::Log(line) => self.log.push(line),
                }
            }
        }
    }

    /// A process exited (`ReapOneProcess`).
    pub fn handle_exit(&mut self, pid: u32, exit: Exit) {
        let props = self.props.clone();
        let props = props.borrow();
        let known = self.supervisor.reap(
            pid,
            exit,
            self.launcher.as_mut(),
            &self.planner,
            &*props,
            Self::now(),
        );
        drop(props);
        if !known {
            self.log.push(format!("untracked process {pid} {exit}"));
        }
        self.drain_supervisor();
    }

    /// Reaps exited processes and starts services whose restart time came.
    /// Returns when something is due next.
    pub fn poll_processes(&mut self) -> Option<Instant> {
        let mut exits = std::mem::take(&mut self.deferred_exits);
        exits.extend(self.launcher.reap());
        for (pid, exit) in exits {
            self.handle_exit(pid, exit);
        }
        let (due, next) = self
            .supervisor
            .process_actions(self.launcher.as_mut(), Self::now());
        for name in due {
            let result = self.start_service(&name);
            self.log.push(format!(
                "restarting '{name}': {}",
                match &result {
                    Ok(effect) => effect.text(),
                    Err(error) => error.clone(),
                }
            ));
            self.drain_supervisor();
        }
        next
    }

    /// `HandleControlMessage`.
    pub fn handle_control(&mut self, message: &ControlMessage) -> Result<Effect, String> {
        let name = message.target.as_str();
        let service = |this: &Self, action: &str| -> Result<String, String> {
            if action.starts_with("interface_") {
                this.supervisor
                    .service_for_interface(name)
                    .map(str::to_string)
                    .ok_or_else(|| format!("Could not find service hosting interface {name}"))
            } else {
                Ok(name.to_string())
            }
        };
        let result = match message.action.as_str() {
            "start" | "interface_start" => {
                let target = service(self, &message.action)?;
                self.start_service(&target)
            }
            "stop" | "interface_stop" => {
                let target = service(self, &message.action)?;
                self.stop_service(&target)
            }
            "restart" | "interface_restart" => {
                let target = service(self, &message.action)?;
                self.restart_service(&target, false)
            }
            "oneshot_on" | "oneshot_off" => self
                .supervisor
                .set_oneshot(name, message.action == "oneshot_on")
                .map(|()| Effect::Applied(format!("{} {name}", message.action))),
            "sigstop_on" | "sigstop_off" => Ok(Effect::NoOp(
                "sigstop: no ptrace on the syscall layer yet".to_string(),
            )),
            "apex_load" | "apex_unload" => Ok(Effect::NoOp(
                "APEXes are pre-flattened; nothing to load".to_string(),
            )),
            other => Err(format!("Unknown control msg '{other}'")),
        };
        self.drain_supervisor();
        result
    }

    /// One builtin, with effects recorded in `current`.
    pub fn run_command(&mut self, command: &Command) -> Result<CommandFlow, String> {
        let effect = match command {
            Command::Start { service } => self.start_service(service)?,
            Command::Stop { service } => self.stop_service(service)?,
            Command::Restart {
                service,
                only_if_running,
            } => self.restart_service(service, *only_if_running)?,
            Command::Enable { service } => self.enable_service(service)?,
            Command::ClassStart { class } => {
                self.class_start(class)?;
                return Ok(CommandFlow::Done);
            }
            Command::ClassStop { class } => {
                self.for_class(class, "stop", false)?;
                return Ok(CommandFlow::Done);
            }
            Command::ClassReset { class } => {
                self.for_class(class, "reset", false)?;
                return Ok(CommandFlow::Done);
            }
            Command::ClassRestart {
                class,
                only_enabled,
            } => {
                self.for_class(class, "restart", *only_enabled)?;
                return Ok(CommandFlow::Done);
            }
            Command::Exec(spec) => return self.exec(spec, true),
            Command::ExecBackground(spec) => return self.exec(spec, false),
            Command::ExecStart { service } => return self.exec_start(service),
            Command::InterfaceStart { interface }
            | Command::InterfaceStop { interface }
            | Command::InterfaceRestart { interface } => {
                let action = match command {
                    Command::InterfaceStart { .. } => "interface_start",
                    Command::InterfaceStop { .. } => "interface_stop",
                    _ => "interface_restart",
                };
                self.handle_control(&ControlMessage {
                    action: action.to_string(),
                    target: interface.clone(),
                    from_pid: 1,
                })?
            }
            Command::Mkdir {
                path,
                mode,
                owner,
                group,
                options,
            } => {
                let mode = mode.as_deref().map(parse_mode).transpose()?;
                let ids = &self.planner.ids;
                let uid = owner.as_deref().map(|n| ids.decode_uid(n)).transpose()?;
                let gid = group.as_deref().map(|n| ids.decode_gid(n)).transpose()?;
                let mut effect = self.fs.mkdir(path, mode, uid, gid)?;
                if !options.is_empty()
                    && let Effect::Applied(text) = &mut effect
                {
                    text.push_str(&format!(
                        " ({}: no fscrypt policy on a host directory)",
                        options.join(" ")
                    ));
                }
                effect
            }
            Command::Chmod { mode, path } => self.fs.chmod(parse_mode(mode)?, path)?,
            Command::Chown { owner, group, path } => {
                let ids = &self.planner.ids;
                let uid = ids.decode_uid(owner)?;
                let gid = group.as_deref().map(|n| ids.decode_gid(n)).transpose()?;
                self.fs.chown(uid, gid, path)?
            }
            Command::Write { path, content } => self.fs.write(path, content)?,
            Command::Copy { source, target } => self.fs.copy(source, target)?,
            Command::CopyPerLine { source, target } => self.fs.copy_per_line(source, target)?,
            Command::Symlink { target, link } => self.fs.symlink(target, link)?,
            Command::Rm { path } => self.fs.remove(path, false)?,
            Command::Rmdir { path } => self.fs.remove(path, true)?,
            Command::Mount {
                fs_type,
                device,
                target,
                options,
            } => self.fs.mount(fs_type, device, target, options)?,
            Command::Wait { path, .. } => self.fs.wait(path)?,
            Command::Hostname { name } => {
                self.fs.kernel_value("/proc/sys/kernel/hostname", name)?
            }
            Command::Domainname { name } => {
                self.fs.kernel_value("/proc/sys/kernel/domainname", name)?
            }
            Command::Export { name, value } => {
                self.planner.set_env(name, value);
                if self.fs.apply {
                    self.planner.write_env()?;
                }
                Effect::Recorded(format!("export {name}={value} (service environment)"))
            }
            Command::LoadExports { path } => self.load_exports(path)?,
            Command::Setrlimit(limit) => self.set_rlimit(limit),
            Command::LoadPersistProps => self.load_persist_props()?,
            Command::LoadSystemProps => Effect::NoOp(
                "load_system_props: .prop files were loaded by property_init before the boot"
                    .to_string(),
            ),
            Command::MountAll { args } => self.mount_all(args)?,
            Command::UmountAll { .. } | Command::Umount { .. } => {
                Effect::NoOp("nothing is mounted: partitions are host directories".to_string())
            }
            Command::SwaponAll { .. } | Command::Swapoff { .. } => {
                Effect::NoOp("no swap: macOS manages memory compression and swap".to_string())
            }
            Command::Restorecon { .. } | Command::RestoreconRecursive { .. } => Effect::NoOp(
                "SELinux is permissive and files carry no labels (ADR 0012)".to_string(),
            ),
            Command::Insmod { path, .. } => Effect::NoOp(format!(
                "insmod {path}: no kernel modules on the syscall layer"
            )),
            Command::PerformApexConfig { bootstrap } => {
                self.perform_apex_config(*bootstrap)?;
                return Ok(CommandFlow::Done);
            }
            Command::UpdateLinkerConfig => {
                self.update_linker_config()?;
                return Ok(CommandFlow::Done);
            }
            Command::Sysclktz { minutes_west } => Effect::NoOp(format!(
                "sysclktz {minutes_west}: the kernel timezone is the host's"
            )),
            Command::Loglevel { level } => {
                Effect::NoOp(format!("loglevel {level}: guest-init logs everything"))
            }
            Command::Bootchart { .. } => Effect::NoOp("bootchart is not supported".to_string()),
            Command::Readahead { path, .. } => {
                Effect::NoOp(format!("readahead {path}: page cache hint only"))
            }
            Command::VerityUpdateState => Effect::NoOp(
                "no dm-verity: the derived image is verified by its identity".to_string(),
            ),
            Command::MarkPostData => {
                Effect::NoOp("mark_post_data: /data is always available".to_string())
            }
            // do_init_user0: vold prepares /data/data, /data/user/0 and
            // user 0's DE storage (keys only when FBE is on).
            Command::InitUser0 => {
                let spec = ExecSpec {
                    seclabel: None,
                    user: None,
                    group: None,
                    supplementary_groups: Vec::new(),
                    args: ["/system/bin/vdc", "--wait", "cryptfs", "init_user0"]
                        .map(String::from)
                        .to_vec(),
                };
                return self.exec(&spec, true);
            }
            Command::Installkey { .. } => Effect::NoOp(
                "file-based encryption keys: /data is a host directory without FBE".to_string(),
            ),
            Command::EnterDefaultMountNs => {
                Effect::NoOp("one mount namespace; nothing to enter".to_string())
            }
            Command::Ifup { interface } => Effect::NoOp(format!(
                "ifup {interface}: networking is the replaced netd's (ADR 0012 appendix)"
            )),
            Command::Trigger { event } => Effect::Engine(format!("trigger {event} queued")),
            Command::Setprop { name, value } => Effect::Engine(format!("setprop {name}={value}")),
            Command::WaitForProp { name, value } => {
                Effect::Engine(format!("wait_for_prop {name} {value}"))
            }
            Command::Init(action) => match action {
                InitAction::WaitForColdbootDone
                | InitAction::QueuePropertyTriggers
                | InitAction::EnablePropertyTrigger => Effect::Engine(action.name().to_string()),
                // libprocessgroup's CgroupSetup: v1 controllers stay
                // unmounted; the v2 hierarchy gets its system/app
                // isolation directories.
                InitAction::SetupCgroups => match self.fs.cgroup2.clone() {
                    Some((root, mode, uid, gid)) => {
                        for sub in ["apps", "system"] {
                            let path = format!("{root}/{sub}");
                            self.fs.mkdir(&path, Some(mode), Some(uid), Some(gid))?;
                        }
                        Effect::Applied(format!(
                            "SetupCgroups: {root}/apps and {root}/system (cgroup v2; no controller acts)"
                        ))
                    }
                    None => Effect::NoOp(
                        "SetupCgroups: cgroups are answered unsupported (ADR 0012 appendix)"
                            .to_string(),
                    ),
                },
                InitAction::SetKptrRestrict => {
                    Effect::NoOp("SetKptrRestrict: no kernel pointers are exposed".to_string())
                }
                InitAction::TestPerfEventSelinux => {
                    Effect::NoOp("TestPerfEventSelinux: no perf_event".to_string())
                }
                InitAction::ConnectEarlyStageSnapuserd => {
                    Effect::NoOp("no snapuserd: the image is pre-flattened".to_string())
                }
                InitAction::CheckTradeInModeStatus => {
                    Effect::NoOp("CheckTradeInModeStatus: not a trade-in device".to_string())
                }
                InitAction::SetMmapRndBits => {
                    Effect::NoOp("SetMmapRndBits: ASLR is the host's".to_string())
                }
                InitAction::KeychordInit => {
                    Effect::NoOp("KeychordInit: no keychords without evdev".to_string())
                }
            },
        };
        self.effect(effect);
        Ok(CommandFlow::Done)
    }
}

impl CommandExecutor for GuestExecutor {
    fn execute(&mut self, invocation: &Invocation<'_>) -> Result<CommandFlow, String> {
        self.sync_services(invocation.services);
        self.current.clear();
        let result = self.run_command(invocation.command);
        self.drain_supervisor();
        result
    }
}

/// The engine's view of the shared property service.
pub struct PropsAdapter(pub Rc<RefCell<Properties>>);

impl PropertyLookup for PropsAdapter {
    fn property(&self, name: &str) -> Option<String> {
        self.0.borrow().property(name)
    }
}

impl InitProperties for PropsAdapter {
    fn init_set(&mut self, name: &str, value: &str) -> Result<Vec<(String, String)>, String> {
        InitProperties::init_set(&mut *self.0.borrow_mut(), name, value)
    }
}
