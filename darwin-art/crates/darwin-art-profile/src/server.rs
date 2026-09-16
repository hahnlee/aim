use crate::bound_service_process::{BoundServiceProcessRequest, BoundServiceProcessResponse};
use crate::bound_service_registry::{
    BoundServiceRecord, BoundServiceRegistry, BoundServiceSlot, RESTART_REAP_TIMEOUT,
};
use crate::filesystem::ProfileFilesystem;
use crate::process_incarnation::ProcessIncarnation;
use crate::process_registry::{ProcessLease, ProcessRegistry};
use crate::registry::PackageRegistry;
use crate::{ProfileError, ProfilePaths, protocol, write_path};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub struct DaemonConfig {
    pub paths: ProfilePaths,
    pub idle_timeout: Duration,
}

struct State {
    binder: crate::binder_service::BinderService,
    filesystem: Mutex<ProfileFilesystem>,
    paths: ProfilePaths,
    registry: Mutex<PackageRegistry>,
    processes: Mutex<ProcessRegistry>,
    runtime_services: crate::runtime_service_state::RuntimeServiceState,
    application_launches:
        Mutex<BTreeMap<String, crate::application_launch_template::ApplicationLaunchTemplate>>,
    bound_services: BoundServiceRegistry,
    start_gate: Mutex<()>,
    bound_service_gate: Mutex<()>,
    lease_gate: Mutex<()>,
    leases: AtomicUsize,
    handlers: AtomicUsize,
    shutdown: AtomicBool,
    last_activity: Mutex<Instant>,
}

struct HandlerGuard(Arc<State>);
struct LeaseGuard {
    state: Arc<State>,
    process: Option<ProcessLease>,
}

impl Drop for HandlerGuard {
    fn drop(&mut self) {
        self.0.handlers.fetch_sub(1, Ordering::SeqCst);
        *self.0.last_activity.lock().unwrap() = Instant::now();
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.state.leases.fetch_sub(1, Ordering::SeqCst);
        if let Some(process) = self.process {
            self.state.processes.lock().unwrap().release(process);
        }
    }
}

pub fn run_daemon(config: DaemonConfig) -> Result<(), ProfileError> {
    fs::create_dir_all(&config.paths.profile_root)?;
    fs::set_permissions(
        &config.paths.profile_root,
        fs::Permissions::from_mode(0o700),
    )?;
    let _lock = acquire_lock(&config.paths)?;
    if config.paths.socket.as_os_str().as_encoded_bytes().len() >= 104 {
        return Err(ProfileError::Daemon(format!(
            "control socket path exceeds macOS's 103-byte limit; shorten {}",
            config.paths.profiles_root.display()
        )));
    }
    remove_stale_socket(&config.paths)?;
    let listener = UnixListener::bind(&config.paths.socket)?;
    fs::set_permissions(&config.paths.socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let state = Arc::new(State {
        binder: Default::default(),
        filesystem: Mutex::new(ProfileFilesystem::new(config.paths.clone())),
        paths: config.paths.clone(),
        registry: Mutex::new(PackageRegistry::new(&config.paths)),
        processes: Mutex::new(ProcessRegistry::default()),
        runtime_services: Default::default(),
        application_launches: Default::default(),
        bound_services: BoundServiceRegistry::default(),
        start_gate: Mutex::new(()),
        bound_service_gate: Mutex::new(()),
        lease_gate: Mutex::new(()),
        leases: AtomicUsize::new(0),
        handlers: AtomicUsize::new(0),
        shutdown: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
    });
    while !state.shutdown.load(Ordering::SeqCst) {
        if !crate::listener_wait::wait_readable(listener.as_fd(), Duration::from_millis(50))? {
            if state.leases.load(Ordering::SeqCst) == 0
                && state.handlers.load(Ordering::SeqCst) == 0
                && state.last_activity.lock().unwrap().elapsed() >= config.idle_timeout
            {
                break;
            }
            continue;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                // Darwin inherits O_NONBLOCK from the listening socket. Client
                // streams are blocking so an acquire handler remains a lease
                // until the peer closes instead of treating EAGAIN as EOF.
                stream.set_nonblocking(false)?;
                state.handlers.fetch_add(1, Ordering::SeqCst);
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    let _guard = HandlerGuard(Arc::clone(&state));
                    if let Err(error) = handle(stream, &state) {
                        eprintln!("darwin-artd: client error: {error}");
                    }
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // Another acceptor cannot consume this listener, but a
                // readiness notification can still race a peer disconnect.
                continue;
            }
            Err(error) => return Err(error.into()),
        }
    }
    drop(listener);
    while state.handlers.load(Ordering::SeqCst) != 0 {
        thread::sleep(Duration::from_millis(10));
    }
    state.filesystem.lock().unwrap().detach()?;
    let _ = fs::remove_file(&config.paths.socket);
    Ok(())
}

fn handle(mut stream: UnixStream, state: &Arc<State>) -> Result<(), ProfileError> {
    verify_same_user(&stream)?;
    let message = protocol::read_message(&mut stream)?;
    match message.operation {
        protocol::OP_ENSURE => {
            require_empty(&message.payload)?;
            let filesystem = state.filesystem.lock().unwrap();
            match filesystem.ensure() {
                Ok(path) => {
                    state.registry.lock().unwrap().migrate_app_ids()?;
                    protocol::write_response(
                        &mut stream,
                        message.operation,
                        0,
                        write_path(path.as_os_str()),
                    )?;
                }
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    5,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        protocol::OP_ACQUIRE => {
            let identity = parse_process_identity(&message.payload)?;
            let incarnation = identity
                .as_ref()
                .map(|(pid, _)| crate::peer_process::verify_registration(&stream, *pid))
                .transpose()?;
            let gate = state.lease_gate.lock().unwrap();
            if state.shutdown.load(Ordering::SeqCst) {
                protocol::write_response(
                    &mut stream,
                    message.operation,
                    108,
                    b"daemon is shutting down",
                )?;
                return Ok(());
            }
            if !state.filesystem.lock().unwrap().is_mounted()? {
                protocol::write_response(
                    &mut stream,
                    message.operation,
                    2,
                    b"profile is not ready",
                )?;
                return Ok(());
            }
            let process = match identity
                .as_ref()
                .map(|(pid, package)| {
                    let mut processes = state.processes.lock().unwrap();
                    let current = ProcessIncarnation::read(*pid)?;
                    if Some(current) != incarnation {
                        return Err(ProfileError::Daemon(
                            "process changed before registration".into(),
                        ));
                    }
                    processes.acquire(*pid, package, current, false)
                })
                .transpose()
            {
                Ok(process) => process,
                Err(error) => {
                    protocol::write_response(
                        &mut stream,
                        message.operation,
                        22,
                        error.to_string().as_bytes(),
                    )?;
                    return Ok(());
                }
            };
            state.leases.fetch_add(1, Ordering::SeqCst);
            let _lease = LeaseGuard {
                state: Arc::clone(state),
                process,
            };
            drop(gate);
            protocol::write_response(&mut stream, message.operation, 0, b"")?;
            let mut byte = [0_u8; 1];
            while stream.read(&mut byte).is_ok_and(|count| count != 0) {}
        }
        protocol::OP_STATUS => {
            require_empty(&message.payload)?;
            let mounted = state.filesystem.lock().unwrap().is_mounted()?;
            let status = format!(
                "profile={} mounted={} leases={}",
                state.paths.profile_id,
                mounted,
                state.leases.load(Ordering::SeqCst)
            );
            protocol::write_response(&mut stream, message.operation, 0, status.as_bytes())?;
        }
        protocol::OP_SHUTDOWN => {
            require_empty(&message.payload)?;
            let _gate = state.lease_gate.lock().unwrap();
            let leases = state.leases.load(Ordering::SeqCst);
            if leases == 0 {
                protocol::write_response(&mut stream, message.operation, 0, b"")?;
                state.shutdown.store(true, Ordering::SeqCst);
            } else {
                protocol::write_response(
                    &mut stream,
                    message.operation,
                    16,
                    format!("{leases} active lease(s)").as_bytes(),
                )?;
            }
        }
        protocol::OP_REGISTER => {
            let separator = message
                .payload
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| ProfileError::Daemon("register request has no package".into()))?;
            let package = std::str::from_utf8(&message.payload[..separator])
                .map_err(|_| ProfileError::Daemon("package is not UTF-8".into()))?;
            let record = &message.payload[separator + 1..];
            state.filesystem.lock().unwrap().ensure()?;
            state.registry.lock().unwrap().register(package, record)?;
            protocol::write_response(&mut stream, message.operation, 0, b"")?;
        }
        protocol::OP_RESOLVE => {
            let package = std::str::from_utf8(&message.payload)
                .map_err(|_| ProfileError::Daemon("package is not UTF-8".into()))?;
            state.filesystem.lock().unwrap().ensure()?;
            let resolved = { state.registry.lock().unwrap().resolve(package) };
            match resolved {
                Ok(record) => {
                    protocol::write_response(&mut stream, message.operation, 0, &record)?;
                }
                Err(ProfileError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                    protocol::write_response(
                        &mut stream,
                        message.operation,
                        protocol::STATUS_NOT_FOUND,
                        b"package is not installed",
                    )?;
                }
                Err(error) => {
                    protocol::write_response(
                        &mut stream,
                        message.operation,
                        1,
                        error.to_string().as_bytes(),
                    )?;
                }
            }
        }
        protocol::OP_LIST => {
            require_empty(&message.payload)?;
            state.filesystem.lock().unwrap().ensure()?;
            let packages = state.registry.lock().unwrap().list()?;
            protocol::write_response(&mut stream, message.operation, 0, &packages)?;
        }
        protocol::OP_PROCESSES => {
            require_empty(&message.payload)?;
            let mut processes = state
                .processes
                .lock()
                .unwrap()
                .processes()
                .map(|(pid, package)| format!("{pid}\t{package}"))
                .collect::<Vec<_>>()
                .join("\n");
            if !processes.is_empty() {
                processes.push('\n');
            }
            protocol::write_response(&mut stream, message.operation, 0, processes.as_bytes())?;
        }
        protocol::OP_PROCESS_IDENTITY => {
            if message.payload.len() != 4 {
                return Err(ProfileError::Daemon(
                    "process identity request requires a PID".into(),
                ));
            }
            let pid = u32::from_le_bytes(message.payload[..4].try_into().unwrap());
            let incarnation = ProcessIncarnation::read(pid)?;
            let (package, registered_uid) = {
                let processes = state.processes.lock().unwrap();
                let package = processes
                    .package(pid, incarnation)
                    .ok_or_else(|| ProfileError::Daemon("process is not registered".into()))?
                    .to_owned();
                (package, processes.android_uid(pid, incarnation))
            };
            let uid = if let Some(uid) = registered_uid {
                uid
            } else if package == "android.system" {
                1000
            } else {
                state.registry.lock().unwrap().app_id(&package)?
            };
            let identity = crate::ProcessIdentity { pid, uid, package };
            protocol::write_response(&mut stream, message.operation, 0, &identity.encode())?;
        }
        protocol::OP_UNREGISTER => {
            if message.payload.len() < 2 || message.payload[0] > 1 {
                protocol::write_response(
                    &mut stream,
                    message.operation,
                    22,
                    b"unregister requires a data policy and package",
                )?;
                return Ok(());
            }
            let remove_data = message.payload[0] == 1;
            let package = std::str::from_utf8(&message.payload[1..])
                .map_err(|_| ProfileError::Daemon("package is not UTF-8".into()))?;
            crate::registry::validate_package(package)?;
            let _gate = state.lease_gate.lock().unwrap();
            if state.processes.lock().unwrap().contains_package(package) {
                protocol::write_response(
                    &mut stream,
                    message.operation,
                    16,
                    b"package is running",
                )?;
                return Ok(());
            }
            state.filesystem.lock().unwrap().ensure()?;
            let registry = state.registry.lock().unwrap();
            match unregister_package_files(&state.paths, &registry, package, remove_data) {
                Ok(()) => protocol::write_response(&mut stream, message.operation, 0, b"")?,
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    5,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        protocol::OP_DAEMONIZE => {
            let (package, arguments, mut environment) = parse_daemonize(&message.payload)?;
            crate::registry::validate_package(&package)?;
            let android_uid = if package == "android.system" {
                1000
            } else {
                state.registry.lock().unwrap().app_id(&package)?
            };
            environment.retain(|(name, _)| name != "DARWIN_ART_ANDROID_UID");
            environment.push((
                "DARWIN_ART_ANDROID_UID".into(),
                android_uid.to_string().into(),
            ));
            let application_template =
                crate::application_launch_template::ApplicationLaunchTemplate::capture(
                    &package,
                    &arguments,
                    &environment,
                )?;
            let _start = state.start_gate.lock().unwrap();
            if state.runtime_services.contains_package(&package) {
                return protocol::write_response(
                    &mut stream,
                    message.operation,
                    16,
                    b"package is owned by a runtime service instance",
                )
                .map_err(Into::into);
            }
            state.filesystem.lock().unwrap().ensure()?;
            let mut command = crate::process_command::prepare_command(
                &arguments,
                &environment,
                &state.paths.profile_root,
            )?;
            let mut child = command.spawn()?;
            let pid = child.id();
            let on_exit = match register_child(state, pid, &package) {
                Ok(on_exit) => on_exit,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };
            if let Some(template) = application_template {
                state
                    .application_launches
                    .lock()
                    .map_err(|_| {
                        ProfileError::Daemon("application launch registry is poisoned".into())
                    })?
                    .insert(package.clone(), template);
            }
            thread::spawn(move || {
                let _ = child.wait();
                on_exit();
            });
            protocol::write_response(&mut stream, message.operation, 0, &pid.to_le_bytes())?;
        }
        protocol::OP_START_RUNTIME => {
            let result = start_runtime(state, &message.payload);
            match result {
                Ok(response) => {
                    protocol::write_response(&mut stream, message.operation, 0, &response)?
                }
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    5,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        protocol::OP_START_BOUND_SERVICE => {
            let result = start_bound_service(state, &message.payload, &stream);
            match result {
                Ok(response) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    0,
                    &response.encode()?,
                )?,
                Err(error) => {
                    let status = if error.to_string().contains("not authorized") {
                        13
                    } else {
                        22
                    };
                    protocol::write_response(
                        &mut stream,
                        message.operation,
                        status,
                        error.to_string().as_bytes(),
                    )?
                }
            }
        }
        protocol::OP_ACTIVATE_BOUND_SERVICE => {
            let result = activate_bound_service(state, &message.payload, &stream);
            match result {
                Ok(()) => protocol::write_response(&mut stream, message.operation, 0, b""),
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    22,
                    error.to_string().as_bytes(),
                ),
            }?
        }
        protocol::OP_RUNTIME_READY => {
            let result = (|| {
                let ready =
                    crate::runtime_service_protocol::ReadyRequest::decode(&message.payload)?;
                let (pid, incarnation) = crate::peer_process::identity(&stream)?;
                state
                    .runtime_services
                    .publish_ready(pid, incarnation, ready)
            })();
            match result {
                Ok(()) => protocol::write_response(&mut stream, message.operation, 0, b"")?,
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    5,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        protocol::OP_RUNTIME_LOST => {
            let result = (|| {
                let lost = crate::runtime_service_protocol::LossRequest::decode(&message.payload)?;
                let (pid, incarnation) = crate::peer_process::identity(&stream)?;
                state
                    .runtime_services
                    .publish_runtime_lost(pid, incarnation, lost)
            })();
            match result {
                Ok(()) => protocol::write_response(&mut stream, message.operation, 0, b"")?,
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    5,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        protocol::OP_BINDER_SESSION => {
            require_empty(&message.payload)?;
            let peer = binder_peer(state, &stream)?;
            protocol::write_response(&mut stream, message.operation, 0, b"")?;
            return state.binder.serve(stream, peer);
        }
        protocol::OP_BINDER_TRANSFER_DEPOSIT => {
            let token = parse_transfer_token(&message.payload)?;
            let peer = binder_peer(state, &stream)?;
            match state.binder.receive_deposit(peer, token, &stream) {
                Ok(()) => protocol::write_response(&mut stream, message.operation, 0, b"")?,
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    22,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        protocol::OP_BINDER_TRANSFER_TAKE => {
            let (source, token) = parse_transfer_key(&message.payload)?;
            let peer = binder_peer(state, &stream)?;
            match state.binder.prepare_take(peer, source, token) {
                Ok(delivery) => {
                    protocol::write_response(&mut stream, message.operation, 0, b"")?;
                    delivery.send(&stream)?;
                }
                Err(error) => protocol::write_response(
                    &mut stream,
                    message.operation,
                    22,
                    error.to_string().as_bytes(),
                )?,
            }
        }
        _ => protocol::write_response(&mut stream, message.operation, 38, b"unknown operation")?,
    }
    Ok(())
}

fn binder_peer(
    state: &State,
    stream: &UnixStream,
) -> Result<darwin_art_binder_device::routing_authority::PeerIdentity, ProfileError> {
    let (pid, incarnation) = crate::peer_process::identity(stream)?;
    let (package, registered_uid) = {
        let processes = state
            .processes
            .lock()
            .map_err(|_| ProfileError::Daemon("process registry is poisoned".into()))?;
        let package = processes
            .package(pid, incarnation)
            .map(str::to_owned)
            .ok_or_else(|| {
                ProfileError::Daemon("Binder peer is not a registered process".into())
            })?;
        (package, processes.android_uid(pid, incarnation))
    };
    // Binder credentials describe the exact executing process, not merely its
    // installed package. In particular, isolated service processes retain the
    // UID allocated by system_server for this launch incarnation.
    let android_uid = if let Some(uid) = registered_uid {
        uid
    } else if package == "android.system" {
        1000
    } else {
        state
            .registry
            .lock()
            .map_err(|_| ProfileError::Daemon("package registry is poisoned".into()))?
            .app_id(&package)?
    };
    darwin_art_binder_device::routing_authority::PeerIdentity::verified(
        pid,
        android_uid,
        incarnation.parts(),
    )
    .ok_or_else(|| ProfileError::Daemon("invalid Binder peer identity".into()))
}

fn parse_transfer_token(
    payload: &[u8],
) -> Result<darwin_art_binder_device::authority_protocol::TransferToken, ProfileError> {
    if payload.len() != 8 {
        return Err(ProfileError::Daemon(
            "Binder transfer token must be 8 bytes".into(),
        ));
    }
    let raw = u64::from_le_bytes(payload.try_into().unwrap());
    darwin_art_binder_device::authority_protocol::TransferToken::from_nonzero(raw)
        .ok_or_else(|| ProfileError::Daemon("Binder transfer token is zero".into()))
}

fn parse_transfer_key(
    payload: &[u8],
) -> Result<
    (
        darwin_art_binder_device::authority_protocol::ConnectionToken,
        darwin_art_binder_device::authority_protocol::TransferToken,
    ),
    ProfileError,
> {
    if payload.len() != 16 {
        return Err(ProfileError::Daemon(
            "Binder transfer key must be 16 bytes".into(),
        ));
    }
    let source = u64::from_le_bytes(payload[..8].try_into().unwrap());
    let source =
        darwin_art_binder_device::authority_protocol::ConnectionToken::from_nonzero(source)
            .ok_or_else(|| ProfileError::Daemon("Binder transfer source is zero".into()))?;
    Ok((source, parse_transfer_token(&payload[8..])?))
}

fn register_child(
    state: &Arc<State>,
    pid: u32,
    package: &str,
) -> Result<Box<dyn FnOnce() + Send>, ProfileError> {
    register_child_with_uid(state, pid, package, None)
}

fn register_child_with_uid(
    state: &Arc<State>,
    pid: u32,
    package: &str,
    android_uid: Option<u32>,
) -> Result<Box<dyn FnOnce() + Send>, ProfileError> {
    let _gate = state.lease_gate.lock().unwrap();
    if state.shutdown.load(Ordering::SeqCst) {
        return Err(ProfileError::Daemon("daemon is shutting down".into()));
    }
    let mut processes = state.processes.lock().unwrap();
    let incarnation = ProcessIncarnation::read(pid)?;
    let lease = processes.acquire_with_uid(pid, package, incarnation, true, android_uid)?;
    state.leases.fetch_add(1, Ordering::SeqCst);
    let owner = Arc::clone(state);
    Ok(Box::new(move || {
        let _gate = owner.lease_gate.lock().unwrap();
        owner.processes.lock().unwrap().child_exited(lease);
        owner.leases.fetch_sub(1, Ordering::SeqCst);
        *owner.last_activity.lock().unwrap() = Instant::now();
    }))
}

fn start_bound_service(
    state: &Arc<State>,
    payload: &[u8],
    stream: &UnixStream,
) -> Result<BoundServiceProcessResponse, ProfileError> {
    // A same-user Unix socket is sufficient for profile administration, but
    // not for process creation.  The only caller allowed to exercise this
    // operation is the daemon-spawned, incarnation-checked android.system
    // child.  In particular, an app cannot self-register as system and then
    // obtain a process-spawn capability.
    let (peer_pid, peer_incarnation) = crate::peer_process::identity(stream)?;
    let is_system = state
        .processes
        .lock()
        .map_err(|_| ProfileError::Daemon("process registry is poisoned".into()))?
        .is_child_owner(peer_pid, peer_incarnation, "android.system");
    if !is_system {
        return Err(ProfileError::Daemon(
            "bound-service requester is not authorized: expected android.system child".into(),
        ));
    }
    let request = BoundServiceProcessRequest::decode(payload)?;
    validate_bound_service_request(state, &request)?;
    let identity = request.identity();
    let _gate = state.bound_service_gate.lock().unwrap();
    if state
        .processes
        .lock()
        .map_err(|_| ProfileError::Daemon("process registry is poisoned".into()))?
        .contains_unowned_package(&request.package)
    {
        return Err(ProfileError::Daemon(
            "bound-service package already has a caller-owned process".into(),
        ));
    }
    if let BoundServiceSlot::Existing(existing) =
        state
            .bound_services
            .await_slot(&identity, request.start_sequence, RESTART_REAP_TIMEOUT)?
    {
        return Ok(existing);
    }

    let system_template = state.runtime_services.system_launch_template()?;
    let application_template = state
        .application_launches
        .lock()
        .map_err(|_| ProfileError::Daemon("application launch registry is poisoned".into()))?
        .get(&request.package)
        .cloned()
        .ok_or_else(|| {
            ProfileError::Daemon("bound-service package has no daemon launch template".into())
        })?;
    let launch = application_template.bound_service_launch(request.clone(), &system_template)?;

    let service_uid = request.uid;
    let (response, child) =
        crate::bound_service_process::spawn(&launch, &state.paths.profile_root, |pid| {
            register_child_with_uid(state, pid, &request.package, Some(service_uid))
        })?;
    state.bound_services.insert(
        identity.clone(),
        BoundServiceRecord::new(response, child.activation()),
    )?;
    let owner = Arc::clone(state);
    let retired_identity = identity.clone();
    let retired_pid = response.pid;
    let retired_start_sequence = response.start_sequence;
    let retired_incarnation = response.incarnation;
    if let Err(error) = child.supervise(move || {
        let _ = owner.bound_services.remove_incarnation(
            &retired_identity,
            retired_pid,
            retired_start_sequence,
            retired_incarnation,
        );
    }) {
        // `OwnedBoundServiceChild::Drop` has already killed/reaped the child
        // and run the process-lease callback.  Retire only our exact record.
        state.bound_services.remove_incarnation(
            &identity,
            response.pid,
            response.start_sequence,
            response.incarnation,
        )?;
        return Err(error);
    }
    Ok(response)
}

fn activate_bound_service(
    state: &Arc<State>,
    payload: &[u8],
    stream: &UnixStream,
) -> Result<(), ProfileError> {
    let (peer_pid, peer_incarnation) = crate::peer_process::identity(stream)?;
    let is_system = state
        .processes
        .lock()
        .map_err(|_| ProfileError::Daemon("process registry is poisoned".into()))?
        .is_child_owner(peer_pid, peer_incarnation, "android.system");
    if !is_system {
        return Err(ProfileError::Daemon(
            "bound-service requester is not authorized: expected android.system child".into(),
        ));
    }
    let handle = BoundServiceProcessResponse::decode(payload)?;
    let _gate = state.bound_service_gate.lock().unwrap();
    let activation = state.bound_services.activation(handle)?;
    crate::bound_service_process::OwnedBoundServiceChild::activate(&activation)
}

fn validate_bound_service_request(
    state: &Arc<State>,
    request: &BoundServiceProcessRequest,
) -> Result<(), ProfileError> {
    crate::registry::validate_package(&request.package)?;
    let installed_uid = if request.package == "android.system" {
        1000
    } else {
        state.registry.lock().unwrap().app_id(&request.package)?
    };
    if !request.isolated && installed_uid != request.uid {
        return Err(ProfileError::Daemon(
            "bound-service UID does not match installed package identity".into(),
        ));
    }
    if request.package == "android.system" && request.uid != 1000 {
        return Err(ProfileError::Daemon(
            "android.system may not launch a non-system service identity".into(),
        ));
    }
    Ok(())
}

fn start_runtime(state: &Arc<State>, payload: &[u8]) -> Result<Vec<u8>, ProfileError> {
    let request = crate::runtime_service_protocol::StartRuntimeRequest::decode(payload)?;
    let instance = {
        let _start = state.start_gate.lock().unwrap();
        if let Some(previous) = state.runtime_services.conflicting_instance(&request)? {
            if request.package != "android.system" {
                return Err(ProfileError::Daemon(
                    "only the profile system runtime supports generation replacement".into(),
                ));
            }
            // Serialize the final idle check with system-server-owned child
            // creation. Once the old runtime is failed, no late bound-service
            // request may cross the generation boundary before it is reaped.
            let _bound_services = state.bound_service_gate.lock().unwrap();
            if state
                .processes
                .lock()
                .map_err(|_| ProfileError::Daemon("process registry is poisoned".into()))?
                .contains_package_other_than(&request.package)
            {
                return Err(ProfileError::Daemon(
                    "cannot replace the system runtime while Android applications are running"
                        .into(),
                ));
            }
            previous.fail("runtime generation superseded by a new build")?;
            previous.wait_exited(crate::runtime_service_launch::START_TIMEOUT)?;
        }
        if !state.runtime_services.contains_package(&request.package)
            && state
                .processes
                .lock()
                .unwrap()
                .contains_package(&request.package)
        {
            return Err(ProfileError::Daemon(
                "package already has an unversioned process".into(),
            ));
        }
        state.filesystem.lock().unwrap().ensure()?;
        let (instance, fresh) = state.runtime_services.reserve(&request)?;
        if fresh {
            crate::runtime_service_launch::launch(
                Arc::clone(&instance),
                &request,
                &state.paths.profile_root,
                &state.paths.socket,
                |pid| register_child(state, pid, &request.package),
            )?;
        }
        instance
    };
    instance
        .wait_ready(crate::runtime_service_launch::START_TIMEOUT)?
        .encode()
}

fn unregister_package_files(
    paths: &ProfilePaths,
    registry: &PackageRegistry,
    package: &str,
    remove_data: bool,
) -> Result<(), ProfileError> {
    registry.resolve(package)?;
    let trash = paths.mount.join("run").join(format!(
        ".uninstall.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir(&trash)?;
    let mut moved: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut candidates = vec![
        (
            paths.mount.join("packages").join(package),
            trash.join("package"),
        ),
        (
            paths.mount.join("system/package-code").join(package),
            trash.join("code"),
        ),
    ];
    if remove_data {
        candidates.push((
            paths.mount.join("data/apps").join(package),
            trash.join("data"),
        ));
    }
    for (source, destination) in candidates {
        if !source.exists() {
            continue;
        }
        if let Err(error) = fs::rename(&source, &destination) {
            for (restore_from, restore_to) in moved.into_iter().rev() {
                let _ = fs::rename(restore_from, restore_to);
            }
            let _ = fs::remove_dir(&trash);
            return Err(error.into());
        }
        moved.push((destination, source));
    }
    if let Err(error) = registry.unregister(package) {
        for (restore_from, restore_to) in moved.into_iter().rev() {
            let _ = fs::rename(restore_from, restore_to);
        }
        let _ = fs::remove_dir(&trash);
        return Err(error);
    }
    if make_tree_removable(&trash).is_ok() {
        let _ = fs::remove_dir_all(&trash);
    }
    Ok(())
}

fn make_tree_removable(path: &std::path::Path) -> Result<(), ProfileError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        for entry in fs::read_dir(path)? {
            make_tree_removable(&entry?.path())?;
        }
    } else {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn parse_daemonize(
    payload: &[u8],
) -> Result<
    (
        String,
        Vec<std::ffi::OsString>,
        Vec<(std::ffi::OsString, std::ffi::OsString)>,
    ),
    ProfileError,
> {
    let mut cursor = 0_usize;
    let package = String::from_utf8(read_field(payload, &mut cursor)?.to_vec())
        .map_err(|_| ProfileError::Daemon("daemonize package is not UTF-8".into()))?;
    let argument_count = read_u32(payload, &mut cursor)? as usize;
    if argument_count == 0 || argument_count > 64 {
        return Err(ProfileError::Daemon(
            "invalid daemonize argument count".into(),
        ));
    }
    let mut arguments = Vec::with_capacity(argument_count);
    for _ in 0..argument_count {
        arguments.push(std::ffi::OsString::from_vec(
            read_field(payload, &mut cursor)?.to_vec(),
        ));
    }
    let environment_count = read_u32(payload, &mut cursor)? as usize;
    if environment_count > 256 {
        return Err(ProfileError::Daemon(
            "invalid daemonize environment count".into(),
        ));
    }
    let mut environment = Vec::with_capacity(environment_count);
    for _ in 0..environment_count {
        let key = std::ffi::OsString::from_vec(read_field(payload, &mut cursor)?.to_vec());
        let value = std::ffi::OsString::from_vec(read_field(payload, &mut cursor)?.to_vec());
        environment.push((key, value));
    }
    if cursor != payload.len() {
        return Err(ProfileError::Daemon("trailing daemonize payload".into()));
    }
    Ok((package, arguments, environment))
}

fn read_u32(payload: &[u8], cursor: &mut usize) -> Result<u32, ProfileError> {
    let end = cursor
        .checked_add(4)
        .filter(|end| *end <= payload.len())
        .ok_or_else(|| ProfileError::Daemon("truncated daemonize payload".into()))?;
    let value = u32::from_le_bytes(payload[*cursor..end].try_into().unwrap());
    *cursor = end;
    Ok(value)
}

fn read_field<'a>(payload: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], ProfileError> {
    let length = read_u32(payload, cursor)? as usize;
    let end = cursor
        .checked_add(length)
        .filter(|end| *end <= payload.len())
        .ok_or_else(|| ProfileError::Daemon("truncated daemonize field".into()))?;
    let field = &payload[*cursor..end];
    if field.contains(&0) {
        return Err(ProfileError::Daemon("NUL in daemonize field".into()));
    }
    *cursor = end;
    Ok(field)
}

fn require_empty(payload: &[u8]) -> Result<(), ProfileError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(ProfileError::Daemon("unexpected request payload".into()))
    }
}

fn parse_process_identity(payload: &[u8]) -> Result<Option<(u32, String)>, ProfileError> {
    if payload.is_empty() {
        return Ok(None);
    }
    if payload.len() < 5 {
        return Err(ProfileError::Daemon("process identity is truncated".into()));
    }
    let pid = u32::from_le_bytes(payload[..4].try_into().unwrap());
    if pid == 0 {
        return Err(ProfileError::Daemon("process PID must be non-zero".into()));
    }
    let package = std::str::from_utf8(&payload[4..])
        .map_err(|_| ProfileError::Daemon("package is not UTF-8".into()))?;
    crate::registry::validate_package(package)?;
    Ok(Some((pid, package.to_owned())))
}

fn acquire_lock(paths: &ProfilePaths) -> Result<File, ProfileError> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(paths.profile_root.join("darwin-artd.lock"))?;
    let result = unsafe { flock(lock.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result != 0 {
        return Err(ProfileError::Daemon(
            "profile daemon is already running".into(),
        ));
    }
    Ok(lock)
}

fn remove_stale_socket(paths: &ProfilePaths) -> Result<(), ProfileError> {
    match fs::symlink_metadata(&paths.socket) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&paths.socket)?,
        Ok(_) => {
            return Err(ProfileError::Daemon(format!(
                "control socket path is not a socket: {}",
                paths.socket.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn verify_same_user(stream: &UnixStream) -> Result<(), ProfileError> {
    let mut uid = 0_u32;
    let mut gid = 0_u32;
    if unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
        || uid != unsafe { geteuid() }
    {
        return Err(ProfileError::Daemon(
            "client uid does not own this daemon".into(),
        ));
    }
    Ok(())
}

const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;

unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn getpeereid(socket: i32, effective_user: *mut u32, effective_group: *mut u32) -> i32;
    fn geteuid() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn temporary_paths(test: &str) -> ProfilePaths {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        ProfilePaths::new(
            std::env::temp_dir().join(format!("darwin-art-profile-{test}-{nonce}")),
            "test",
        )
        .unwrap()
    }

    fn field(payload: &mut Vec<u8>, value: &[u8]) {
        payload.extend_from_slice(&(value.len() as u32).to_le_bytes());
        payload.extend_from_slice(value);
    }

    #[test]
    fn daemonize_payload_preserves_argv_and_environment_bytes() {
        let mut payload = Vec::new();
        field(&mut payload, b"android.system");
        payload.extend_from_slice(&2_u32.to_le_bytes());
        field(&mut payload, b"/runtime host");
        field(&mut payload, b"--window-seconds=0");
        payload.extend_from_slice(&1_u32.to_le_bytes());
        field(&mut payload, b"DARWIN_ART_MODE");
        field(&mut payload, b"system server");

        let (package, arguments, environment) = parse_daemonize(&payload).unwrap();
        assert_eq!(package, "android.system");
        assert_eq!(arguments[0].as_bytes(), b"/runtime host");
        assert_eq!(arguments[1].as_bytes(), b"--window-seconds=0");
        assert_eq!(environment[0].0.as_bytes(), b"DARWIN_ART_MODE");
        assert_eq!(environment[0].1.as_bytes(), b"system server");
    }

    #[test]
    fn daemonize_payload_rejects_nul_and_trailing_bytes() {
        let mut payload = Vec::new();
        field(&mut payload, b"android.system");
        payload.extend_from_slice(&1_u32.to_le_bytes());
        field(&mut payload, b"bad\0program");
        payload.extend_from_slice(&0_u32.to_le_bytes());
        assert!(parse_daemonize(&payload).is_err());

        let mut valid = Vec::new();
        field(&mut valid, b"android.system");
        valid.extend_from_slice(&1_u32.to_le_bytes());
        field(&mut valid, b"/runtime");
        valid.extend_from_slice(&0_u32.to_le_bytes());
        valid.push(0);
        assert!(parse_daemonize(&valid).is_err());
    }

    #[test]
    fn unregister_removes_code_and_honors_the_data_policy() {
        let paths = temporary_paths("unregister");
        let package = "com.example.app";
        fs::create_dir_all(paths.mount.join("run")).unwrap();
        fs::create_dir_all(paths.mount.join("packages").join(package)).unwrap();
        fs::create_dir_all(paths.mount.join("system/package-code").join(package)).unwrap();
        fs::create_dir_all(paths.mount.join("data/apps").join(package)).unwrap();
        let registry = PackageRegistry::new(&paths);
        registry
            .register(package, b"darwin-art-launch-v1\npackage=com.example.app\n")
            .unwrap();

        unregister_package_files(&paths, &registry, package, false).unwrap();

        assert!(registry.resolve(package).is_err());
        assert!(!paths.mount.join("packages").join(package).exists());
        assert!(
            !paths
                .mount
                .join("system/package-code")
                .join(package)
                .exists()
        );
        assert!(paths.mount.join("data/apps").join(package).exists());

        fs::create_dir_all(paths.mount.join("packages").join(package)).unwrap();
        registry
            .register(package, b"darwin-art-launch-v1\npackage=com.example.app\n")
            .unwrap();
        unregister_package_files(&paths, &registry, package, true).unwrap();
        assert!(!paths.mount.join("data/apps").join(package).exists());
        fs::remove_dir_all(paths.profiles_root).unwrap();
    }
}
