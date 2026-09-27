//! The boot: property init, script loading and init's main loop
//! (`SecondStageMain`) over the [`GuestExecutor`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use darwin_android_init::engine::{ActionManager, COLD_BOOT_DONE_PROP, ExecutedCommand, Step};
use darwin_android_init::props::load::{
    KernelBootProperties, PropertyInitOptions, create_serialized_property_info, property_init,
    start_property_service,
};
use darwin_android_init::props::protocol::{
    PROP_ERROR_HANDLE_CONTROL_MESSAGE, PROP_SUCCESS, serve,
};
use darwin_android_init::props::service::RESTORECON_PROPERTY;
use darwin_android_init::props::{SetEffect, Ucred};
use darwin_android_init::rc::{IdResolver, ScriptLoader, vendor_android_version};
use darwin_android_init::{ImageRoot, PropertyLookup};
use darwin_binder_host::server::Server;

use crate::apex;
use crate::executor::{GuestExecutor, Outgoing, PropsAdapter};
use crate::fsops::{Effect, FsOps};
use crate::identity::Identity;
use crate::launch::{
    DryRunLauncher, Exit, HostLauncher, Launcher, LinuxRun, LinuxRunOptions, describe_launch,
};
use crate::paths::Layout;
use crate::props::{Properties, heap_properties, mapped_properties};
use crate::propsvc::{PropertyEvent, PropertySockets, SetRequest};
use crate::supervisor::{DEFAULT_PATH, Planner};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMode {
    /// Describe launches and effects; touch nothing on the host but the
    /// runtime directory layout.
    DryRun,
    /// Launch services and apply effects.
    Run,
}

#[derive(Clone, Debug)]
pub struct BootOptions {
    pub image: PathBuf,
    pub data: PathBuf,
    pub runtime: Option<PathBuf>,
    pub mode: RunMode,
    pub only: Option<BTreeSet<String>>,
    /// `linux-run`; defaults to the one next to the current executable.
    pub linux_run: Option<PathBuf>,
    pub trace: bool,
    /// `androidboot.*` bootconfig entries (without the prefix).
    pub androidboot: Vec<(String, String)>,
    /// Run mode: stop everything after this long.
    pub timeout: Option<Duration>,
    /// Run mode with `--only`: a `wait_for_prop` or `exec` nobody selected
    /// can satisfy is simulated after this long.
    pub simulate_after: Duration,
}

impl BootOptions {
    pub fn new(image: PathBuf, data: PathBuf, mode: RunMode) -> Self {
        Self {
            image,
            data,
            runtime: None,
            mode,
            only: None,
            linux_run: None,
            trace: false,
            androidboot: vec![
                ("hardware".to_string(), "ranchu".to_string()),
                // init.ranchu.rc: ro.hardware.egl, the GLES driver the
                // original libEGL loads (/vendor/lib64/egl/libGLES_darwin.so).
                ("hardwareegl".to_string(), "darwin".to_string()),
            ],
            timeout: None,
            simulate_after: Duration::from_secs(2),
        }
    }
}

/// One executed command and what it did.
#[derive(Clone, Debug)]
pub struct CommandReport {
    pub executed: ExecutedCommand,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, Default)]
pub struct BootReport {
    pub commands: Vec<CommandReport>,
    /// Waits nobody would satisfy in this mode, satisfied by guest-init.
    pub simulated: Vec<String>,
    /// Launch descriptions in order, by pid.
    pub launches: Vec<(u32, String)>,
    pub exits: Vec<String>,
    pub log: Vec<String>,
    pub triggers: Vec<String>,
    pub fatal: Option<String>,
    pub shutdown: Option<String>,
    pub property_count: usize,
    pub services_declared: usize,
    pub diagnostics: Vec<String>,
}

impl BootReport {
    pub fn effect_counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for command in &self.commands {
            if command.executed.result.is_err() {
                *counts.entry("error").or_default() += 1;
            }
            for effect in &command.effects {
                *counts.entry(effect.kind()).or_default() += 1;
            }
        }
        counts
    }

    /// Distinct services started (excluding init's helpers and `exec`).
    pub fn services_launched(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for command in &self.commands {
            for effect in &command.effects {
                if let Effect::Launch { service, .. } = effect
                    && !service.starts_with("exec ")
                    && service != "linkerconfig"
                {
                    out.insert(service.clone());
                }
            }
        }
        out
    }

    pub fn noop_reasons(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        for command in &self.commands {
            for effect in &command.effects {
                if let Effect::NoOp(text) = effect {
                    let keyword = command
                        .executed
                        .command
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    // "<verb> <path>: <reason>" groups by reason.
                    let reason = match text.split_once(": ") {
                        Some((head, rest)) if head.contains('/') => rest,
                        _ => text.as_str(),
                    };
                    let key = format!("{keyword}: {reason}");
                    *out.entry(key).or_default() += 1;
                }
            }
        }
        out
    }

    pub fn summary(&self) -> String {
        let counts = self.effect_counts();
        let exec_count = self
            .commands
            .iter()
            .flat_map(|c| &c.effects)
            .filter(|e| matches!(e, Effect::Launch { service, .. } if service.starts_with("exec ")))
            .count();
        let helper_count = self
            .commands
            .iter()
            .flat_map(|c| &c.effects)
            .filter(|e| matches!(e, Effect::Launch { service, .. } if service == "linkerconfig"))
            .count();
        format!(
            "commands executed: {}; services declared: {}; services launched: {} (+{} exec, +{} linkerconfig); effects: {}; simulated waits: {}; triggers: {}; properties: {}{}",
            self.commands.len(),
            self.services_declared,
            self.services_launched().len(),
            exec_count,
            helper_count,
            counts
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" "),
            self.simulated.len(),
            self.triggers.len(),
            self.property_count,
            self.fatal
                .as_ref()
                .map(|f| format!("; FATAL: {f}"))
                .unwrap_or_default()
        )
    }
}

/// A prepared boot.
pub struct Boot {
    pub options: BootOptions,
    pub layout: Layout,
    pub manager: ActionManager,
    pub executor: GuestExecutor,
    props: Rc<RefCell<Properties>>,
    linux_run: LinuxRun,
    events: Option<Receiver<PropertyEvent>>,
    _sockets: Option<PropertySockets>,
    /// Run mode: the binder host behind every service's `--binder`.
    _binder: Option<Arc<Server>>,
    pub report: BootReport,
}

/// The controller mount points `SetupCgroups` would create from
/// `cgroups.json` (system, then vendor).
pub fn cgroup_mount_points(image: &ImageRoot) -> Vec<String> {
    let mut out = Vec::new();
    for file in ["/system/etc/cgroups.json", "/vendor/etc/cgroups.json"] {
        let Ok(bytes) = image.read(file) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for piece in text.split("\"Path\"").skip(1) {
            let Some(start) = piece.find('"') else {
                continue;
            };
            let rest = &piece[start + 1..];
            let Some(end) = rest.find('"') else {
                continue;
            };
            let path = &rest[..end];
            if path.starts_with('/') && !out.iter().any(|p| p == path) {
                out.push(path.to_string());
            }
        }
    }
    out
}

fn default_linux_run() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("linux-run")))
        .unwrap_or_else(|| PathBuf::from("linux-run"))
}

impl Boot {
    /// Everything before init's main loop: the runtime layout, property
    /// areas and `PropertyInit`, the ueventd role, the APEX list, scripts,
    /// the boot queue and (run mode) the property sockets.
    pub fn prepare(options: BootOptions) -> Result<Self, String> {
        let image = ImageRoot::new(&options.image);
        if !image.exists("/system/etc/init/hw/init.rc") {
            return Err(format!(
                "{}: no /system/etc/init/hw/init.rc",
                options.image.display()
            ));
        }
        let layout = Layout::new(
            options.image.clone(),
            options.data.clone(),
            options.runtime.clone(),
        );
        layout.prepare().map_err(|e| e.to_string())?;
        let map = layout.path_map();
        std::fs::write(layout.path_map_file(), map.to_file_text()).map_err(|e| e.to_string())?;
        let mut report = BootReport::default();

        let vendor_api_level = vendor_android_version(&image).unwrap_or(36);
        let mut diagnostics = Vec::new();
        let info = create_serialized_property_info(&image, vendor_api_level, &mut diagnostics)?;
        let mut properties = match options.mode {
            RunMode::DryRun => heap_properties(info)?,
            RunMode::Run => mapped_properties(&layout.properties_dir(), info)?,
        };
        let init_options = PropertyInitOptions {
            kernel: KernelBootProperties {
                bootconfig: options
                    .androidboot
                    .iter()
                    .map(|(k, v)| (format!("androidboot.{k}"), v.clone()))
                    .collect(),
                ..Default::default()
            },
            vendor_api_level,
            ..Default::default()
        };
        diagnostics.extend(property_init(&mut properties, &image, &init_options));
        diagnostics.extend(start_property_service(&mut properties));
        // ueventd's role: there is no coldboot to wait for.
        properties.init_set(COLD_BOOT_DONE_PROP, "true");
        report.diagnostics = diagnostics.iter().map(|d| d.to_string()).collect();

        let apexes = apex::scan(&image);
        let xml = apex::apex_info_list_xml(&apexes);
        std::fs::write(layout.apex_info_list(), &xml).map_err(|e| e.to_string())?;

        let ids = IdResolver::from_image(&image, &properties);
        let scripts = ScriptLoader {
            image: &image,
            properties: &properties,
            ids: &ids,
            vendor_api_level,
            vendor_apexes: Some(apex::vendor_apexes(&apexes)),
        }
        .load();
        for diagnostic in scripts
            .boot
            .diagnostics
            .iter()
            .chain(&scripts.apex.diagnostics)
        {
            report.diagnostics.push(diagnostic.to_string());
        }
        report.services_declared = scripts.apex.services.len().max(scripts.boot.services.len());

        let mut manager = ActionManager::new(scripts, vendor_api_level);
        manager.queue_boot(&properties);

        let linux_run_binary = options.linux_run.clone().unwrap_or_else(default_linux_run);
        let linux_run_options = match options.mode {
            RunMode::DryRun => LinuxRunOptions::CONTRACT,
            RunMode::Run => LinuxRunOptions::detect(&linux_run_binary),
        };
        // The binder driver is kernel state, so the init role hosts it
        // (ADR 0012 item 7): one per boot, named for this process.
        let binder_name = format!("dev.darwinart.guest-init.{}.binder", std::process::id());
        let binder = match options.mode {
            RunMode::Run if linux_run_options.binder => {
                Some(Server::start(&binder_name).map_err(|e| format!("binder host: {e}"))?)
            }
            _ => None,
        };
        let linux_run = LinuxRun {
            binary: linux_run_binary,
            image: options.image.clone(),
            path_map_file: layout.path_map_file(),
            binder: Some(binder_name),
            trace: options.trace,
            options: linux_run_options,
        };
        if options.mode == RunMode::Run && !linux_run_options.missing().is_empty() {
            report.log.push(format!(
                "linux-run lacks {} (docs/guest-init-contract.md): services see the image's own /dev, the host uid, no binder and linux-run's default environment",
                linux_run_options.missing().join(", ")
            ));
        }
        let launcher: Box<dyn Launcher> = match options.mode {
            RunMode::DryRun => Box::new(DryRunLauncher::new(linux_run.clone())),
            RunMode::Run => Box::new(HostLauncher::new(linux_run.clone(), layout.clone())),
        };
        let planner = Planner {
            layout: layout.clone(),
            map: map.clone(),
            ids,
            vendor_api_level,
            env: vec![("PATH".to_string(), DEFAULT_PATH.to_string())],
            rlimits: Vec::new(),
            boot_epoch: Instant::now(),
        };
        let mut fs = FsOps::new(map, layout.fs_attrs_file(), options.mode == RunMode::Run);
        for root in cgroup_mount_points(&image)
            .iter()
            .map(String::as_str)
            .chain(crate::fsops::LEGACY_CGROUP_ROOTS.iter().copied())
        {
            fs.add_unmounted_root(root, crate::fsops::CGROUP_REASON);
        }
        let props = Rc::new(RefCell::new(properties));
        let mut executor =
            GuestExecutor::new(props.clone(), launcher, planner, fs, options.only.clone());
        executor.sync_services(manager.services());

        let (events, sockets) = if options.mode == RunMode::Run {
            let (sender, receiver) = mpsc::channel();
            let sockets = PropertySockets::start(&layout.socket_dir(), sender)
                .map_err(|e| format!("property service sockets: {e}"))?;
            (Some(receiver), Some(sockets))
        } else {
            (None, None)
        };

        Ok(Self {
            options,
            layout,
            manager,
            executor,
            props,
            linux_run,
            events,
            _sockets: sockets,
            _binder: binder,
            report,
        })
    }

    pub fn properties(&self) -> Rc<RefCell<Properties>> {
        self.props.clone()
    }

    /// Hands the executor's queued changes to the action manager.
    fn pump_outbox(&mut self) {
        while let Some(outgoing) = self.executor.outbox.pop_front() {
            match outgoing {
                Outgoing::PropertyChanged(name, value) => {
                    self.manager.property_changed(&name, &value)
                }
                Outgoing::Trigger(trigger) => self.manager.queue_event_trigger(&trigger),
                Outgoing::ExecFinished => self.manager.exec_finished(),
            }
        }
        if let Some(command) = self.manager.take_shutdown_request() {
            self.report.shutdown.get_or_insert(command);
        }
    }

    /// The credentials of a property service peer: the identity of the
    /// service darwin-artd started with that pid, else root (the host user
    /// owns the whole guest).
    fn peer_credentials(&self, request: &SetRequest) -> (Ucred, String) {
        let pid = request.peer_pid.max(0) as u32;
        if let Some(record) = self
            .executor
            .supervisor
            .service_for_pid(pid)
            .and_then(|name| self.executor.supervisor.record(name))
        {
            let context = record
                .def
                .seclabel
                .clone()
                .unwrap_or_else(|| "u:r:init:s0".to_string());
            return (
                Ucred {
                    pid: request.peer_pid,
                    uid: record.def.uid(),
                    gid: record.def.gid(),
                },
                context,
            );
        }
        let identity = std::fs::read_to_string(
            self.layout
                .identity_dir()
                .join("by-pid")
                .join(pid.to_string()),
        )
        .ok()
        .and_then(|text| Identity::parse_file_text(&text).ok());
        match identity {
            Some(identity) => (
                Ucred {
                    pid: request.peer_pid,
                    uid: identity.uid,
                    gid: identity.gid,
                },
                if identity.seclabel.is_empty() {
                    "u:r:init:s0".to_string()
                } else {
                    identity.seclabel
                },
            ),
            None => (
                Ucred {
                    pid: request.peer_pid,
                    uid: 0,
                    gid: 0,
                },
                "u:r:su:s0".to_string(),
            ),
        }
    }

    /// `HandlePropertySet` for one socket request, then its effects.
    pub fn handle_set_request(&mut self, request: SetRequest) {
        self.executor.sync_services(self.manager.services());
        let (cred, context) = self.peer_credentials(&request);
        let served = serve(
            &mut self.props.borrow_mut(),
            &request.request,
            Some(&context),
            &cred,
        );
        if let Some(error) = &served.error {
            self.report.log.push(format!(
                "setprop from pid {} on {}: {error}",
                request.peer_pid, request.socket
            ));
        }
        let mut reply = served.reply;
        let mut persist = false;
        for effect in served.effects {
            match effect {
                SetEffect::Changed { name, value } => {
                    self.executor
                        .outbox
                        .push_back(Outgoing::PropertyChanged(name, value));
                }
                SetEffect::Control(message) => {
                    let result = self.executor.handle_control(&message);
                    self.report.log.push(format!(
                        "ctl.{} {} from pid {}: {}",
                        message.action,
                        message.target,
                        message.from_pid,
                        match &result {
                            Ok(effect) => effect.text(),
                            Err(error) => error.clone(),
                        }
                    ));
                    reply = Some(if result.is_ok() {
                        PROP_SUCCESS
                    } else {
                        PROP_ERROR_HANDLE_CONTROL_MESSAGE
                    });
                }
                SetEffect::Restorecon(path) => {
                    // restorecon is a no-op (permissive, no labels); then
                    // the property is set to the path, as init does.
                    let outcome = self.props.borrow_mut().property_set(
                        RESTORECON_PROPERTY,
                        path.as_bytes(),
                        false,
                    );
                    for effect in outcome.effects {
                        if let SetEffect::Changed { name, value } = effect {
                            self.executor
                                .outbox
                                .push_back(Outgoing::PropertyChanged(name, value));
                        }
                    }
                }
                SetEffect::Persist { .. } => persist = true,
            }
        }
        if persist && let Err(error) = self.executor.write_persistent_properties() {
            self.report
                .log
                .push(format!("persistent properties: {error}"));
        }
        if let Some(code) = reply {
            request.reply(code);
        }
        self.pump_outbox();
    }

    fn record_command(&mut self, executed: ExecutedCommand) {
        let effects = self.executor.take_effects();
        let event = executed
            .action
            .rsplit(" && ")
            .next()
            .unwrap_or("")
            .to_string();
        if !event.is_empty() && !event.contains('=') && !self.report.triggers.contains(&event) {
            self.report.triggers.push(event);
        }
        self.report
            .commands
            .push(CommandReport { executed, effects });
    }

    /// Satisfies a wait nobody will: in a dry run always, in a run with
    /// `--only` after `simulate_after`.
    fn simulate_property(&mut self, name: &str, value: &str) {
        let _ = self.executor.set_property(name, value);
        self.report.simulated.push(format!("{name}={value}"));
        // A set that init refuses (ro.* already set) still ends the wait
        // the way a daemon setting it would.
        self.manager.property_changed(name, value);
    }

    /// init's main loop. Returns when the queue is idle (dry run), on a
    /// fatal error or shutdown, or at the timeout.
    pub fn run(&mut self) -> &BootReport {
        let deadline = self.options.timeout.map(|t| Instant::now() + t);
        let dry = self.options.mode == RunMode::DryRun;
        let mut blocked_since: Option<Instant> = None;
        let mut steps = 0usize;
        loop {
            steps += 1;
            if steps > 1_000_000 {
                self.report.log.push("step limit reached".to_string());
                break;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                self.report.log.push("timeout reached".to_string());
                break;
            }
            self.pump_outbox();
            let next_process_action = self.executor.poll_processes();
            self.pump_outbox();
            if self.executor.fatal.is_some() || self.report.shutdown.is_some() {
                break;
            }
            if self.executor.shutdown.is_some() {
                self.report.shutdown = self.executor.shutdown.clone();
                break;
            }
            let step = {
                let mut adapter = PropsAdapter(self.props.clone());
                self.manager
                    .execute_one_command(&mut adapter, &mut self.executor)
            };
            let wait = match step {
                Step::Ran(executed) => {
                    blocked_since = None;
                    self.record_command(executed);
                    continue;
                }
                Step::Idle => {
                    if dry {
                        break;
                    }
                    None
                }
                Step::WaitingForProperty { name, value } => {
                    if dry {
                        self.simulate_property(&name, &value);
                        continue;
                    }
                    Some(("property", name, value))
                }
                Step::WaitingForExec => {
                    if dry {
                        if let Some(pid) = self.executor.exec_pid.take() {
                            self.report
                                .simulated
                                .push(format!("exec pid {pid} exited 0"));
                            self.executor.handle_exit(pid, Exit::Code(0));
                        } else {
                            self.manager.exec_finished();
                        }
                        continue;
                    }
                    Some(("exec", String::new(), String::new()))
                }
            };
            // Run mode: sleep until an event, a restart, or the simulate
            // grace period.
            if let Some((kind, name, value)) = &wait {
                let since = *blocked_since.get_or_insert_with(Instant::now);
                if self.options.only.is_some() && since.elapsed() >= self.options.simulate_after {
                    blocked_since = None;
                    if *kind == "property" {
                        self.report.log.push(format!(
                            "--only: nothing selected sets {name}={value}; simulating it"
                        ));
                        self.simulate_property(name, value);
                    } else if let Some(pid) = self.executor.exec_pid {
                        self.report
                            .log
                            .push(format!("--only: exec pid {pid} still running; not waiting"));
                        self.manager.exec_finished();
                    }
                    continue;
                }
            }
            let mut timeout = Duration::from_millis(100);
            if let Some(next) = next_process_action {
                timeout = timeout.min(next.saturating_duration_since(Instant::now()));
            }
            if let Some(receiver) = &self.events {
                match receiver.recv_timeout(timeout) {
                    Ok(PropertyEvent::Set(request)) => self.handle_set_request(request),
                    Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {}
                }
            } else {
                std::thread::sleep(timeout);
            }
        }
        self.finish();
        &self.report
    }

    fn finish(&mut self) {
        if self.options.mode == RunMode::Run {
            self.executor
                .supervisor
                .kill_all(self.executor.launcher.as_mut());
            std::thread::sleep(Duration::from_millis(100));
            let _ = self.executor.poll_processes();
        }
        self.report.fatal = self.executor.fatal.clone();
        self.report.property_count = self.props.borrow().areas().foreach().len();
        self.report.launches = self
            .executor
            .launches
            .iter()
            .map(|l| (l.pid, describe_launch(&self.linux_run, l.pid, &l.spec)))
            .collect();
        self.report.log.append(&mut self.executor.log);
    }

    /// Handles at most one property service request, waiting up to
    /// `timeout` (run mode). Returns whether one was handled.
    pub fn serve_pending(&mut self, timeout: Duration) -> bool {
        let Some(receiver) = &self.events else {
            return false;
        };
        match receiver.recv_timeout(timeout) {
            Ok(PropertyEvent::Set(request)) => {
                self.handle_set_request(request);
                true
            }
            Err(_) => false,
        }
    }

    /// Host paths of the property service sockets (run mode).
    pub fn property_sockets(&self) -> Vec<PathBuf> {
        self._sockets
            .as_ref()
            .map(|s| s.paths().to_vec())
            .unwrap_or_default()
    }

    pub fn property(&self, name: &str) -> Option<String> {
        self.props.borrow().property(name)
    }
}
