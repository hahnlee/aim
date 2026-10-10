//! The boot: property init, script loading and init's main loop
//! (`SecondStageMain`) over the [`GuestExecutor`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use aim_android_image::system_server::{NATIVE_SERVICES, parse_native_services};
use aim_android_init::engine::{ActionManager, COLD_BOOT_DONE_PROP, ExecutedCommand, Step};
use aim_android_init::props::load::{
    KernelBootProperties, PropertyInitOptions, create_serialized_property_info, property_init,
    start_property_service,
};
use aim_android_init::props::protocol::{PROP_ERROR_HANDLE_CONTROL_MESSAGE, PROP_SUCCESS, serve};
use aim_android_init::props::service::RESTORECON_PROPERTY;
use aim_android_init::props::{SetEffect, Ucred};
use aim_android_init::rc::{IdResolver, ScriptLoader, vendor_android_version};
use aim_android_init::{ImageRoot, PropertyLookup};
use aim_binder_host::server::Server;
use aim_services::NativeServices;

use crate::apex;
use crate::executor::{GuestExecutor, Outgoing, PropsAdapter};
use crate::fsops::{Effect, FsOps};
use crate::identity::Identity;
use crate::launch::{
    DryRunLauncher, Exit, HostLauncher, Launcher, LinuxRun, LinuxRunOptions, describe_launch,
};
use crate::mount::DataMount;
use crate::paths::{Layout, Sweep};
use crate::props::{Properties, heap_properties, mapped_properties, read_property, share_areas};
use crate::propsvc::{PropertyEvent, PropertySockets, SetRequest};
use crate::supervisor::{DEFAULT_PATH, Planner};

fn init_caught_signal_mask() -> std::io::Result<u64> {
    let mut caught = 0u64;
    for host_signal in 1..32 {
        // Darwin rejects even read-only sigaction queries for these signals (#1200).
        if matches!(host_signal, libc::SIGKILL | libc::SIGSTOP) {
            continue;
        }
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(host_signal, std::ptr::null(), &mut action) } < 0 {
            return Err(std::io::Error::other(format!(
                "init signal {host_signal} disposition: {}", std::io::Error::last_os_error()
            )));
        }
        if action.sa_sigaction != libc::SIG_DFL && action.sa_sigaction != libc::SIG_IGN {
            let guest_signal = aim_storage::process_namespace::signal_from_host(host_signal);
            if guest_signal > 0 {
                caught |= 1u64 << (guest_signal - 1);
            }
        }
    }
    Ok(caught)
}

/// Wakes the boot loop on every SIGCHLD, as init's signalfd does, so a
/// child's exit (an `exec` ending) is handled at once rather than at the
/// loop's next timeout. The thread ends with the loop's receiver.
fn watch_children(events: mpsc::Sender<PropertyEvent>) -> std::io::Result<()> {
    // SAFETY: a new kqueue and its signal filter; EVFILT_SIGNAL records the
    // signal beside its disposition.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let change = libc::kevent {
        ident: libc::SIGCHLD as usize,
        filter: libc::EVFILT_SIGNAL,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: registering one event on our kqueue.
    if unsafe { libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
        let e = std::io::Error::last_os_error();
        // SAFETY: our kqueue.
        unsafe { libc::close(kq) };
        return Err(e);
    }
    std::thread::spawn(move || {
        loop {
            // SAFETY: waiting on our kqueue into a local event.
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            let n =
                unsafe { libc::kevent(kq, std::ptr::null(), 0, &mut event, 1, std::ptr::null()) };
            if n > 0 && events.send(PropertyEvent::ChildExited).is_err() {
                break;
            }
        }
        // SAFETY: our kqueue.
        unsafe { libc::close(kq) };
    });
    Ok(())
}

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
    /// Services (and `exec` programs) not to start.
    pub exclude: BTreeSet<String>,
    /// `linux-run`; defaults to the one next to the current executable.
    pub linux_run: Option<PathBuf>,
    /// `linux-run --gpu`, `--vulkan` and `--display` for every service.
    pub gpu: Option<PathBuf>,
    pub vulkan: Option<PathBuf>,
    pub display: Option<PathBuf>,
    pub trace: bool,
    /// Run mode: append every binder transaction to this file
    /// ([`trace_binder`]).
    pub binder_trace: Option<PathBuf>,
    /// Run mode: compare these original services with their native models
    /// and log each call to the file (`aim_services::shadow`).
    pub binder_shadow: Option<(Vec<String>, PathBuf)>,
    /// Build mode: claim a new host directory for the native constructor snapshot.
    pub package_constructor_capture: Option<PathBuf>,
    /// `androidboot.*` bootconfig entries (without the prefix).
    pub androidboot: Vec<(String, String)>,
    /// Run mode: stop everything after this long.
    pub timeout: Option<Duration>,
    /// Run mode with `--only` or `--exclude`: a `wait_for_prop` or `exec`
    /// nobody selected can satisfy is simulated after this long.
    pub simulate_after: Duration,
    /// Run mode: the data templates (`userdata` build node) a new data
    /// directory starts from (`aim_storage::data::template`).
    pub userdata: Option<PathBuf>,
}

impl BootOptions {
    pub fn merge_androidboot(&mut self, entries: Vec<(String, String)>) {
        for (name, value) in entries {
            self.androidboot.retain(|(existing, _)| existing != &name);
            self.androidboot.push((name, value));
        }
    }
    pub fn new(image: PathBuf, data: PathBuf, mode: RunMode) -> Self {
        Self {
            image,
            data,
            runtime: None,
            mode,
            only: None,
            exclude: BTreeSet::new(),
            linux_run: None,
            gpu: None,
            vulkan: None,
            display: None,
            trace: false,
            binder_trace: None,
            binder_shadow: None,
            package_constructor_capture: None,
            // The device: init.rc imports init.aim.rc and vold reads
            // fstab.aim (image/overlay.toml).
            androidboot: vec![("hardware".to_string(), "aim".to_string())],
            timeout: None,
            simulate_after: Duration::from_secs(2),
            userdata: None,
        }
    }
}

/// One executed command and what it did.
#[derive(Clone, Debug)]
pub struct CommandReport {
    pub executed: ExecutedCommand,
    pub effects: Vec<Effect>,
    /// When it ran, since the boot started.
    pub at: Duration,
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
    /// The boot's preparation steps, the waits of init's queue and the
    /// properties services set, each with when it ended since the boot
    /// started.
    pub timeline: Vec<(Duration, String)>,
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

/// Set from a signal handler: the running boot stops its services and
/// returns, as at its timeout.
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Asks the running boot to stop. Async-signal-safe.
pub fn request_stop() {
    STOP_REQUESTED.store(true, Ordering::Relaxed);
}

pub fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::Relaxed)
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
    /// Run mode: changes of the Mac's settings (`mac::watch`).
    mac: Option<Receiver<(String, String)>>,
    _sockets: Option<PropertySockets>,
    /// Run mode: the binder host behind every service's `--binder`.
    binder: Option<Arc<Server>>,
    /// Run mode: the system services implemented natively (ADR 0013),
    /// registered once servicemanager is ready.
    native_services: Option<Arc<NativeServices>>,
    /// Drops after executor's HostLauncher has killed and reaped its children.
    _verity_control: Option<aim_storage::verity_control::RunningServer>,
    _posix_control: Option<Arc<aim_storage::posix_broker::Controller>>,
    _pty_owner: Option<aim_storage::pty_owner::transport::RunningServer>,
    /// The native status bar runs (`start_status_bar`).
    status_bar: bool,
    pub report: BootReport,
    /// The previous boot's runtime directory, removed while this one runs.
    _sweep: Sweep,
    _image_lease: Option<aim_storage::system::ImageLease>,
    /// `/data`, `/metadata` and `/cache`: in run mode the data directory's
    /// case-sensitive image, attached at it for the boot (docs/storage.md).
    /// Declared last, so it is detached after the rest is dropped.
    data: Rc<RefCell<DataMount>>,
}

/// `servicemanager` sets it once it serves calls.
const SERVICEMANAGER_READY: &str = "servicemanager.ready";

/// The lightweight shell's init script sets it (docs/m1-shell.md): the
/// image has no SystemUI, so the native status bar takes its place.
const LIGHTWEIGHT_SHELL: &str = "ro.vendor.aim.lightweight_shell";

/// The system services the image says are native (ADR 0013): SystemServer
/// does not start them, so they are created here, to be registered with
/// servicemanager once it is ready.
fn start_native_services(
    image: &ImageRoot,
    data: &std::path::Path,
    properties: &std::path::Path,
    server: &Arc<Server>,
    guest_pid:i32,
    constructor_capture: Option<&std::path::Path>,
) -> Result<Option<Arc<NativeServices>>, String> {
    let Ok(list) = image.read(NATIVE_SERVICES) else {
        if constructor_capture.is_some(){return Err("original package constructor capture capability unavailable".into());}
        return Ok(None);
    };
    let names: Vec<String> = parse_native_services(&String::from_utf8_lossy(&list))
        .map_err(|e| format!("{NATIVE_SERVICES}: {e}"))?
        .into_iter()
        .map(|s| s.name)
        .collect();
    if constructor_capture.is_some() && !names.iter().any(|name| name == "package") {
        return Err("constructor capture requires native package owner; original constructor capture unavailable".into());
    }
    let services = NativeServices::new_for_namespace(server.driver(), &names,guest_pid).map_err(|e| format!("native services: {e}"))?;
    if names.iter().any(|name| name == "package") {
        if let Some(path)=constructor_capture { services.configure_package_constructor_capture(path)?; }
        services.configure_package_image(image.root(), data, &[])
            .map_err(|error| format!("native package image: {error}"))?;
        services.configure_package_property_area(properties)
            .map_err(|error| format!("native package properties: {error}"))?;
    }
    Ok(Some(Arc::new(services)))
}

/// Trace every binder transaction into `file`, one tab-separated line
/// each: microseconds since the trace started, device, sender pid and
/// euid, target pid, interface token, code, `oneway` or `sync`; for a
/// synchronous call the nanoseconds until the reply, until a target
/// thread took it and until the sender took the reply (`-` for none); the
/// target's free and looper threads when it was sent; and the sending
/// thread and the target thread that took it (0 for none). The sibling
/// `<file>.pending.tsv` samples outstanding calls and their thread/parent
/// graph every 250 ms; snapshot headers contain pending/returning counts.
fn trace_binder(server: &Arc<Server>, file: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::fs::File::create(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut pending_path = file.as_os_str().to_os_string();
    pending_path.push(".pending.tsv");
    let pending_path = PathBuf::from(pending_path);
    let mut pending = std::fs::File::create(&pending_path)
        .map_err(|error| format!("{}: {error}", pending_path.display()))?;
    writeln!(pending, "# at_us\tid\trequest_id\tphase\tage_ns\tdevice\tfrom_pid\tfrom_euid\tfrom_tid\tto_pid\tto_tid\tdescriptor\tcode\tdelivered_ns\treplied_ns\twaiting\tloopers\tfrom_parent\tto_parent\tfrom_stack\tto_stack\tfrom_queued\tto_queued")
        .map_err(|error| format!("{}: {error}", pending_path.display()))?;
    let driver = server.driver().clone();
    driver.start_trace();
    let nanos = |d: Option<Duration>| d.map_or("-".to_string(), |d| d.as_nanos().to_string());
    std::thread::Builder::new()
        .name("binder-trace".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let mut text = String::new();
                for r in driver.take_trace() {
                    text.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        r.at.as_micros(),
                        r.device,
                        r.from_pid,
                        r.from_euid,
                        r.to_pid,
                        r.descriptor,
                        r.code,
                        if r.oneway { "oneway" } else { "sync" },
                        nanos(r.latency),
                        nanos(r.delivered),
                        nanos(r.returned),
                        r.waiting,
                        r.loopers,
                        r.from_tid,
                        r.to_tid,
                    ));
                }
                let snapshot = driver.pending_trace();
                let returning = snapshot.records.iter().filter(|record| record.returning).count();
                let mut graph = format!("# snapshot\t{}\t{}\t{}\n", snapshot.at.as_micros(), snapshot.records.len() - returning, returning);
                let id = |value: Option<u64>| value.map_or("-".to_string(), |value| value.to_string());
                for record in snapshot.records {
                    let r = record.record;
                    let descriptor = r.descriptor.replace(['\t', '\r', '\n'], " ");
                    graph.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                        snapshot.at.as_micros(), record.id, r.transaction_id,
                        if record.returning { "returning" } else { "pending" },
                        record.age.as_nanos(), r.device, r.from_pid, r.from_euid,
                        r.from_tid, r.to_pid, r.to_tid, descriptor, r.code,
                        nanos(r.delivered), nanos(r.latency), r.waiting, r.loopers,
                        id(r.from_parent), id(r.to_parent), id(record.from_stack),
                        id(record.to_stack), record.from_queued, record.to_queued,
                    ));
                }
                if let Err(error) = out.write_all(text.as_bytes()).and_then(|()| pending.write_all(graph.as_bytes())) {
                    eprintln!("binder trace: writing completed or pending records failed: {error}");
                    return;
                }
            }
        })
        .map(drop)
        .map_err(|e| format!("binder trace: {e}"))
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

/// The cgroup v2 hierarchy of `cgroups.json` (`Cgroups2`): its path, mode
/// and owner names. The path map gives it a writable directory
/// (`paths::MapKind::Cgroup2`).
pub fn cgroup2_hierarchy(image: &ImageRoot) -> Option<(String, u32, String, String)> {
    let bytes = image.read("/system/etc/cgroups.json").ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let section = &text[text.find("\"Cgroups2\"")?..];
    let field = |key: &str| -> Option<String> {
        let rest = &section[section.find(&format!("\"{key}\""))? + key.len() + 2..];
        let rest = &rest[rest.find('"')? + 1..];
        Some(rest[..rest.find('"')?].to_string())
    };
    let mode = u32::from_str_radix(&field("Mode").unwrap_or_else(|| "0755".into()), 8).ok()?;
    Some((
        field("Path")?,
        mode,
        field("UID").unwrap_or_else(|| "root".into()),
        field("GID").unwrap_or_else(|| "root".into()),
    ))
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
        // The boot's start, as the kernel's boot is init's: `ro.boottime.*`
        // and the timeline count from here.
        let epoch = Instant::now();
        // The wall clock too, to line the timeline up with logd's.
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let mut timeline = vec![(
            Duration::ZERO,
            format!("boot started at {:.3} (Unix time)", wall.as_secs_f64()),
        )];
        let mut mark = |what: &str| timeline.push((epoch.elapsed(), what.to_string()));
        let image_lease = aim_storage::system::ImageLease::read_root(&options.image)?;
        let image = ImageRoot::new(&options.image);
        if !image.exists("/system/etc/init/hw/init.rc") {
            return Err(format!(
                "{}: no /system/etc/init/hw/init.rc",
                options.image.display()
            ));
        }
        let linux_run_binary = options.linux_run.clone().unwrap_or_else(default_linux_run);
        let linux_run_options = match options.mode {
            RunMode::DryRun => LinuxRunOptions::CONTRACT,
            RunMode::Run => LinuxRunOptions::detect(&linux_run_binary)?,
        };
        // The data image attaches while the boot starts; init's mount_all
        // waits for it (`mount`).
        let data = match options.mode {
            RunMode::Run => aim_storage::data::mount_point(&options.data)?,
            RunMode::DryRun => options.data.clone(),
        };
        let layout = Layout::new(options.image.clone(), data, options.runtime.clone());
        let data_mount = DataMount::start(
            layout.clone(),
            (options.mode == RunMode::Run).then(|| options.data.clone()),
            options.userdata.clone(),
            options
                .androidboot
                .iter()
                .find(|(k, _)| k == crate::sku::KEY)
                .map(|(_, v)| v.clone()),
            epoch,
        )?;
        let data_mount = Rc::new(RefCell::new(data_mount));
        let sweep = layout.sweep_runtime().map_err(|e| e.to_string())?;
        layout.prepare_runtime().map_err(|e| e.to_string())?;
        let verity_control = if options.mode == RunMode::Run {
            data_mount.borrow_mut().mount()?;
            let store = Arc::new(aim_storage::fsverity::Store::new(
                &layout.data.join("fs-verity"), &layout.runtime.join("fs-verity-leases"),
            ).map_err(|error| format!("verity store: {error:?}"))?);
            let owner = aim_storage::verity_control::RunningServer::start_private_with_store(Duration::from_secs(5), store)
                .map_err(|error| format!("native verity control startup: {error}"))?;
            aim_storage::verity_control::OwnerConfig { endpoint:owner.endpoint().to_path_buf(), process: owner.process }
                .write(&layout.runtime.join("verity-control-owner")).map_err(|error| format!("native verity owner locator: {error}"))?;
            Some(owner)
        } else {
            None
        };
        let mut map = layout.path_map();
        std::fs::write(layout.path_map_file(), map.to_file_text()).map_err(|e| e.to_string())?;
        if options.mode == RunMode::Run {
            let process = aim_storage::process_namespace::ProcessIdentity::running(unsafe { libc::getpid() })
                .map_err(|error| error.to_string())?;
            let namespace = format!("init-{}-{}-{}",process.host_pid,process.start_seconds,process.start_microseconds);
            aim_storage::mount_namespace::Namespace::open(layout.path_map_file().parent().ok_or("path-map has no owner directory")?,&namespace)
                .and_then(|owner| owner.initialize(&map.to_file_text())).map_err(|error|error.to_string())?;
            aim_storage::process_namespace::InitRegistration::register(&layout.identity_dir().join("by-pid"),process,&namespace)
                .map_err(|error| error.to_string())?;
            let identity=crate::identity::Identity{service:"init".into(),uid:0,gid:0,groups:Vec::new(),
                capabilities:crate::identity::Capabilities::for_service(0,None),seclabel:"u:r:init:s0".into(),priority:0,oom_score_adjust:0,rlimits:Vec::new()};
            let identity_file=layout.identity_dir().join("init.identity");
            std::fs::write(&identity_file,identity.to_file_text()).map_err(|error|error.to_string())?;
            std::os::unix::fs::symlink(&identity_file,layout.identity_dir().join("by-pid").join(process.host_pid.to_string())).map_err(|error|error.to_string())?;
            aim_storage::process_namespace::register_mount_namespace(&layout.identity_dir().join("by-pid"),process,&namespace).map_err(|error|error.to_string())?;
            let caught = init_caught_signal_mask().map_err(|error| error.to_string())?;
            aim_storage::process_namespace::register_init_signals(&layout.identity_dir().join("by-pid"),process,caught).map_err(|error|error.to_string())?;
            // Publish only after the actual init, mount owner and signal
            // dispositions are registered, before any native or guest service.
            aim_storage::process_namespace::InitRegistration::read(&layout.identity_dir().join("by-pid")).map_err(|error|error.to_string())?
                .activate_pid_mapping(&layout.identity_dir().join("by-pid")).map_err(|error|error.to_string())?;
        }
        mark("runtime layout prepared");
        let mut report = BootReport::default();

        let vendor_api_level = vendor_android_version(&image).unwrap_or(36);
        let mut diagnostics = Vec::new();
        let info = create_serialized_property_info(&image, vendor_api_level, &mut diagnostics)?;
        let mut properties = match options.mode {
            RunMode::DryRun => heap_properties(info)?,
            RunMode::Run => mapped_properties(&layout.properties_dir(), info)?,
        };
        // The bootloader's SKU follows the Mac, unless an `androidboot`
        // option names one.
        let named = options
            .androidboot
            .iter()
            .any(|(k, _)| k == crate::sku::KEY);
        let sku = match options.mode {
            RunMode::Run if !named => {
                crate::sku::host().map(|s| (crate::sku::KEY.to_string(), s.to_string()))
            }
            _ => None,
        };
        let init_options = PropertyInitOptions {
            kernel: KernelBootProperties {
                bootconfig: options
                    .androidboot
                    .iter()
                    .cloned()
                    .chain(sku)
                    .map(|(k, v)| (format!("androidboot.{k}"), v))
                    .collect(),
                ..Default::default()
            },
            vendor_api_level,
            ..Default::default()
        };
        diagnostics.extend(property_init(&mut properties, &image, &init_options));
        diagnostics.extend(start_property_service(&mut properties));
        // The Mac's settings the device follows, before init's first
        // action reads them (docs/mac-settings.md).
        let tzdata = image.read(crate::mac::TZDATA).unwrap_or_default();
        let mac = match options.mode {
            RunMode::Run => crate::mac::properties(&tzdata),
            RunMode::DryRun => Vec::new(),
        };
        for (name, value) in &mac {
            properties.init_set(name, value);
        }
        // ueventd's role: there is no coldboot to wait for.
        properties.init_set(COLD_BOOT_DONE_PROP, "true");
        mark("properties initialized");
        report.diagnostics = diagnostics.iter().map(|d| d.to_string()).collect();

        let apexes = apex::scan(&image);
        let xml = apex::apex_info_list_xml(&apexes);
        std::fs::write(layout.apex_info_list(), &xml).map_err(|e| e.to_string())?;
        apex::write_bootstrap(&layout.bootstrap_apex_dir(), &apexes).map_err(|e| e.to_string())?;
        let mount_namespaces=if options.mode==RunMode::Run{
            // The pinned original apexd bootstrap vector includes virt
            // (RELEASE_AVF_ENABLE_EARLY_VM, original ELF proof in #1228).
            let(owner,bootstrap)=crate::mount_namespace::MountNamespaces::setup(&layout,&map,&apexes,true)?;
            let(ids_bootstrap,ids_default)=owner.namespace_ids();
            report.log.push(format!("native mount owners: bootstrap={ids_bootstrap}, default={ids_default}"));
            map=bootstrap;Some(owner)
        }else{None};
        mark("APEX list written");

        let ids = IdResolver::from_image(&image, &properties);
        let scripts = ScriptLoader {
            image: &image,
            properties: &properties,
            ids: &ids,
            vendor_api_level,
            vendor_apexes: Some(apex::vendor_apexes(&apexes)),
            bootstrap_apexes: apex::bootstrap_apexes(&apexes),
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
        mark("init scripts loaded");

        let posix_control=if options.mode==RunMode::Run {
            let process=aim_storage::process_namespace::ProcessIdentity::running(std::process::id()as i32).map_err(|error|error.to_string())?;
            let endpoint=format!("dev.aim.posix.{}.{}.{}",process.host_pid,process.start_seconds,process.start_microseconds);
            let holder=std::fs::canonicalize(linux_run_binary.with_file_name("aim-lock-holder")).map_err(|error|format!("configured POSIX lock holder: {error}"))?;
            let controller=Arc::new(aim_storage::posix_broker::Controller::start(aim_storage::posix_broker::Config{endpoint,holder,startup_timeout:Duration::from_secs(5)}).map_err(|error|error.to_string())?);
            let table=std::fs::canonicalize(layout.identity_dir().join("by-pid")).map_err(|error|error.to_string())?;
            for entry in map.entries(){
                let host=match std::fs::canonicalize(&entry.host){Ok(host)=>host,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>entry.host.clone(),Err(error)=>return Err(error.to_string())};
                if table.starts_with(&host)||layout.runtime.join("posix-control-owner").starts_with(&host){return Err("native POSIX identity receipts or locator overlap a guest mount".into());}
            }
            let init=aim_storage::process_namespace::InitRegistration::read(&table).map_err(|error|error.to_string())?;
            controller.bind_namespace(&table,&init).map_err(|error|error.to_string())?;
            controller.register_guest(aim_storage::posix_control::Owner{process,guest_pid:1}).map_err(|error|error.to_string())?;
            controller.owner_config().and_then(|owner|owner.write(&layout.runtime.join("posix-control-owner"))).map_err(|error|error.to_string())?;
            Some(controller)
        }else{None};
        let pty_owner = if options.mode == RunMode::Run {
            use std::os::unix::fs::PermissionsExt;
            let runtime = layout.runtime.join("pty-pairs");
            std::fs::create_dir(&runtime).map_err(|error| format!("PTY owner directory: {error}"))?;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
            let runtime = std::fs::canonicalize(runtime).map_err(|error| error.to_string())?;
            let endpoint = format!("dev.aim.pty.{}", std::process::id());
            let owner = aim_storage::pty_owner::transport::RunningServer::start_with_credentials(&endpoint, &runtime,&layout.identity_dir().join("by-pid"))
                .map_err(|error| format!("PTY owner: {error}"))?;
            owner.config.write(&layout.runtime.join("pty-control-owner"))
                .map_err(|error| format!("PTY owner locator: {error}"))?;
            aim_binder_host::pty_file::install_owner(&owner.config)
                .map_err(|error| format!("PTY transport owner: {error}"))?;
            Some(owner)
        } else { None };
        // The binder driver is kernel state, so the init role hosts it
        // (ADR 0012 item 7): one per boot, named for this process.
        let binder_name = format!("dev.aim.guest-init.{}.binder", std::process::id());
        let binder = match options.mode {
            RunMode::Run if linux_run_options.binder => {
                Some(Server::start(&binder_name).map_err(|e| format!("binder host: {e}"))?)
            }
            _ => None,
        };
        let table=layout.identity_dir().join("by-pid");
        let ready=match std::fs::read(table.join("namespace-pid-ready")){
            Ok(bytes)if bytes==b"AIMNS-PID-READY1\n"=>true,
            Ok(_)=>return Err("invalid native init PID readiness record".into()),
            Err(error)if error.kind()==std::io::ErrorKind::NotFound=>false,
            Err(error)=>return Err(error.to_string()),
        };
        let native_pid=if ready{
            let init=aim_storage::process_namespace::InitRegistration::read(&table).map_err(|error|error.to_string())?;
            if init.process.host_pid!=unsafe{libc::getpid()}{return Err("native init registration names another process".into());}1
        }else{unsafe{libc::getpid()}};
        let mut native_services = None;
        if let Some(server) = &binder {
            share_areas(&properties, server);
            if let Some(file) = &options.binder_trace {
                trace_binder(server, file)?;
            }
            native_services = start_native_services(&image, &layout.data.join("data"), &layout.properties_dir(), server,native_pid,options.package_constructor_capture.as_deref())?;
            if let Some((names, log)) = &options.binder_shadow {
                let properties_dir = layout.properties_dir();
                let files = map.clone();
                native_services
                    .as_ref()
                    .ok_or("--binder-shadow: the image has no native services")?
                    .shadow(
                        server.driver(),
                        names,
                        log,
                        image.root(),
                        Box::new(move |name| read_property(&properties_dir, name)),
                        Box::new(move |guest: &str| files.readable_by_others(guest)),
                    )?;
            }
            // Android's notifications on the Mac, from the original
            // NotificationManagerService once it is published.
            if let Some(display) = &options.display {
                let map = map.clone();
                aim_services::notifications::Bridge::start(
                    server.driver(),
                    display,
                    Arc::new(move |guest: &str| map.readable_by_others(guest)),
                );
                // The Mac's Now Playing and screen capture consent, from
                // the original media session and projection services.
                aim_services::media::Bridge::start_for_namespace(server.driver(), display,native_pid);
            }
            // Android's media volume is the Mac's output volume.
            aim_services::volume::start_for_namespace(server.driver(),native_pid);
            mark("binder host and native services started");
        }
        let linux_run = LinuxRun {
            binary: linux_run_binary,
            image: options.image.clone(),
            path_map_file: layout.path_map_file(),
            binder: Some(binder_name),
            gpu: options.gpu.clone(),
            vulkan: options.vulkan.clone(),
            display: options.display.clone(),
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
            RunMode::Run => Box::new(HostLauncher::new(linux_run.clone(), layout.clone()).with_posix(posix_control.as_ref().unwrap().clone(),layout.runtime.join("posix-control-owner"))),
        };
        let planner = Planner {
            layout: layout.clone(),
            map: map.clone(),
            ids,
            vendor_api_level,
            env: vec![("PATH".to_string(), DEFAULT_PATH.to_string())],
            rlimits: Vec::new(),
            boot_epoch: epoch,
        };
        let mut fs = FsOps::new(map, layout.fs_attrs_file(), options.mode == RunMode::Run);
        fs.set_path_map_file(layout.path_map_file());
        if let Some(owner)=mount_namespaces{fs.set_mount_namespaces(owner);}
        let cgroup2 = cgroup2_hierarchy(&image);
        for root in cgroup_mount_points(&image)
            .iter()
            .map(String::as_str)
            .chain(crate::fsops::LEGACY_CGROUP_ROOTS.iter().copied())
            .filter(|root| cgroup2.as_ref().is_none_or(|(path, ..)| path != root))
        {
            fs.add_unmounted_root(root, crate::fsops::CGROUP_REASON);
        }
        fs.cgroup2 = cgroup2.and_then(|(path, mode, uid, gid)| {
            let ids = &planner.ids;
            Some((
                path,
                mode,
                ids.decode_uid(&uid).ok()?,
                ids.decode_uid(&gid).ok()?,
            ))
        });
        let props = Rc::new(RefCell::new(properties));
        let mut executor =
            GuestExecutor::new(props.clone(), launcher, planner, fs, options.only.clone());
        executor.exclude = options.exclude.clone();
        executor.data = Some(data_mount.clone());
        executor.sync_services(manager.services());

        let mac = (options.mode == RunMode::Run).then(|| {
            let (sender, receiver) = mpsc::channel();
            crate::mac::watch(sender, tzdata, mac);
            receiver
        });
        let (events, sockets) = if options.mode == RunMode::Run {
            let (sender, receiver) = mpsc::channel();
            watch_children(sender.clone()).map_err(|e| format!("SIGCHLD watcher: {e}"))?;
            let sockets = PropertySockets::start(&layout.socket_dir(), sender)
                .map_err(|e| format!("property service sockets: {e}"))?;
            (Some(receiver), Some(sockets))
        } else {
            (None, None)
        };
        mark("prepared");
        report.timeline = timeline;

        Ok(Self {
            options,
            layout,
            manager,
            executor,
            props,
            linux_run,
            events,
            mac,
            _sockets: sockets,
            binder,
            native_services,
            _verity_control: verity_control,
            _posix_control: posix_control,
            _pty_owner: pty_owner,
            status_bar: false,
            data: data_mount,
            _sweep: sweep,
            _image_lease: image_lease,
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
    /// service aimd started with that pid, else root (the host user
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
        let identity = Identity::read_entry(
            &self
                .layout
                .identity_dir()
                .join("by-pid")
                .join(pid.to_string()),
        );
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
        let at = self.elapsed();
        for effect in served.effects {
            match effect {
                SetEffect::Changed { name, value } => {
                    self.report
                        .timeline
                        .push((at, format!("pid {} set {name}={value}", request.peer_pid)));
                    if name == SERVICEMANAGER_READY && value == "true" {
                        self.register_native_services();
                        self.start_status_bar();
                    }
                    self.executor
                        .outbox
                        .push_back(Outgoing::PropertyChanged(name, value));
                }
                SetEffect::Control(message) => {
                    let result = self.executor.handle_control(&message);
                    self.report.timeline.push((
                        at,
                        format!(
                            "pid {} ctl.{} {}",
                            message.from_pid, message.action, message.target
                        ),
                    ));
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

    /// Registers the native services with servicemanager, which just set
    /// `servicemanager.ready` (again after a restart). On another thread:
    /// servicemanager waits for this property set's reply before it serves
    /// calls.
    fn register_native_services(&mut self) {
        let Some(services) = self.native_services.clone() else {
            return;
        };
        let _ = std::thread::Builder::new()
            .name("native-services".into())
            .spawn(move || {
                if let Err(error) = services.register() {
                    eprintln!("guest-init: native services: {error}");
                }
            });
    }

    /// The native `IStatusBar` in SystemUI's place, where the image says
    /// so: once per boot, once servicemanager is ready (the init script's
    /// property is set by then). It follows system_server's restarts.
    fn start_status_bar(&mut self) {
        if self.status_bar
            || self.props.borrow().property(LIGHTWEIGHT_SHELL).as_deref() != Some("true")
        {
            return;
        }
        if let (Some(server), Some(display)) = (&self.binder, &self.options.display) {
            aim_services::statusbar::StatusBar::start(server.driver(), display);
            self.status_bar = true;
        }
    }

    /// Sets a property for the host, as init sets one, and runs its
    /// triggers.
    fn set_host_property(&mut self, name: &str, value: &str) {
        if let Err(error) = self.executor.set_property(name, value) {
            self.report.log.push(format!("{name}={value}: {error}"));
        }
        self.pump_outbox();
    }

    /// Since the boot started.
    fn elapsed(&self) -> Duration {
        self.executor.planner.boot_epoch.elapsed()
    }

    fn record_command(&mut self, executed: ExecutedCommand, at: Duration) {
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
        self.report.commands.push(CommandReport {
            executed,
            effects,
            at,
        });
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
        if self.options.mode == RunMode::Run {
            self.sweep_memfds();
        }
        let deadline = self.options.timeout.map(|t| Instant::now() + t);
        self.executor.stop_at = deadline;
        let dry = self.options.mode == RunMode::DryRun;
        let mut blocked_since: Option<Instant> = None;
        // What init's queue is blocked on, and since when.
        let mut waiting: Option<(Duration, String)> = None;
        let mut idle_logged = false;
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
            if stop_requested() {
                self.report.log.push("stop requested".to_string());
                break;
            }
            while let Some((name, value)) = self.mac.as_ref().and_then(|m| m.try_recv().ok()) {
                self.set_host_property(&name, &value);
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
            let at = self.elapsed();
            let step = {
                let mut adapter = PropsAdapter(self.props.clone());
                self.manager
                    .execute_one_command(&mut adapter, &mut self.executor)
            };
            if !matches!(step, Step::WaitingForProperty { .. } | Step::WaitingForExec)
                && let Some((since, what)) = waiting.take()
            {
                self.report.timeline.push((
                    at,
                    format!("waited {:.3} s for {what}", (at - since).as_secs_f64()),
                ));
            }
            let wait = match step {
                Step::Ran(executed) => {
                    blocked_since = None;
                    self.record_command(executed, at);
                    continue;
                }
                Step::Idle => {
                    if dry {
                        break;
                    }
                    if !idle_logged {
                        idle_logged = true;
                        let elapsed = self.elapsed();
                        self.report.log.push(format!(
                            "boot queue first idle after {:.2} s",
                            elapsed.as_secs_f64()
                        ));
                        self.report
                            .timeline
                            .push((elapsed, "boot queue first idle".to_string()));
                    }
                    None
                }
                Step::WaitingForProperty { name, value } => {
                    if dry {
                        self.simulate_property(&name, &value);
                        continue;
                    }
                    waiting.get_or_insert_with(|| (at, format!("{name}={value}")));
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
                    waiting.get_or_insert_with(|| {
                        let pid = self.executor.exec_pid.unwrap_or(0);
                        let launch = self.executor.launches.iter().rev().find(|l| l.pid == pid);
                        let service = launch.map_or("", |l| l.spec.service.as_str());
                        (at, format!("{service} (pid {pid})"))
                    });
                    Some(("exec", String::new(), String::new()))
                }
            };
            // Run mode: sleep until an event, a restart, or the simulate
            // grace period.
            if let Some((kind, name, value)) = &wait {
                let since = *blocked_since.get_or_insert_with(Instant::now);
                let selected = self.options.only.is_some() || !self.options.exclude.is_empty();
                if selected && since.elapsed() >= self.options.simulate_after {
                    blocked_since = None;
                    if *kind == "property" {
                        self.report.log.push(format!(
                            "nothing selected sets {name}={value}; simulating it"
                        ));
                        self.simulate_property(name, value);
                    } else if let Some(pid) = self.executor.exec_pid {
                        self.report
                            .log
                            .push(format!("exec pid {pid} still running; not waiting"));
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
                    Ok(PropertyEvent::ChildExited)
                    | Err(RecvTimeoutError::Timeout)
                    | Err(RecvTimeoutError::Disconnected) => {}
                }
            } else {
                std::thread::sleep(timeout);
            }
        }
        self.finish();
        &self.report
    }

    fn sweep_memfds(&mut self) {
        match std::process::Command::new(&self.linux_run.binary)
            .arg("--sweep-memfds")
            .output()
        {
            Ok(output) if output.status.success() => self
                .report
                .log
                .push(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            Ok(output) => self.report.log.push(format!(
                "memfd cleanup failed: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )),
            Err(error) => self
                .report
                .log
                .push(format!("memfd cleanup failed: {error}")),
        }
    }

    fn finish(&mut self) {
        if self.options.mode == RunMode::Run {
            self.executor
                .supervisor
                .kill_all(self.executor.launcher.as_mut());
            std::thread::sleep(Duration::from_millis(100));
            let _ = self.executor.poll_processes();
            self.sweep_memfds();
        }
        self.report.fatal = self.executor.fatal.clone();
        self.report.timeline.extend(self.data.borrow().timeline());
        self.report.timeline.sort_by_key(|(at, _)| *at);
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
            Ok(PropertyEvent::ChildExited) | Err(_) => false,
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

#[cfg(test)]
mod androidboot_options_tests {
    use super::*;
    #[test]
    fn actual_init_signal_dispositions_exclude_uncatchable_signals() {
        let caught = init_caught_signal_mask().unwrap();
        assert_eq!(caught & ((1 << (9 - 1)) | (1 << (19 - 1))), 0);
        for signal in [libc::SIGKILL, libc::SIGSTOP] {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) }, -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EINVAL));
        }
    }

    #[test]
    fn explicit_debug_port_preserves_device_hardware_and_hardware_override_is_unique() {
        let mut options = BootOptions::new(PathBuf::new(), PathBuf::new(), RunMode::DryRun);
        options.merge_androidboot(vec![("aim.adb.port".into(), "5627".into())]);
        assert!(options.androidboot.contains(&("hardware".into(), "aim".into())));
        assert!(options.androidboot.contains(&("aim.adb.port".into(), "5627".into())));
        options.merge_androidboot(vec![("hardware".into(), "test-device".into())]);
        assert_eq!(options.androidboot.iter().filter(|(name, _)| name == "hardware").count(), 1);
        assert!(options.androidboot.contains(&("hardware".into(), "test-device".into())));
    }
}

#[cfg(test)]
mod capability_boot_tests {
    use super::*;
    #[test]
    fn failed_capability_query_returns_before_data_and_service_creation() {
        use std::os::unix::fs::PermissionsExt;
        let root=std::env::temp_dir().join(format!("aim-capability-boot-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let image=root.join("image");let rc=image.join("system/etc/init/hw/init.rc");
        std::fs::create_dir_all(rc.parent().unwrap()).unwrap();
        std::fs::write(rc,include_bytes!("../../aim-android-init/tests/golden/image/system/etc/init/hw/init.rc")).unwrap();
        let helper=root.join("failed-helper");std::fs::write(&helper,b"#!/bin/sh\nkill -KILL $$\n").unwrap();
        std::fs::set_permissions(&helper,std::fs::Permissions::from_mode(0o700)).unwrap();
        let data=root.join("data");let mut options=BootOptions::new(image,data.clone(),RunMode::Run);options.linux_run=Some(helper);
        let error=Boot::prepare(options).err().unwrap();assert!(error.contains("capability query") && error.contains("signal"),"{error}");
        assert!(!data.exists());assert!(!root.join("data.asif").exists());assert!(!root.join("data.run").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
