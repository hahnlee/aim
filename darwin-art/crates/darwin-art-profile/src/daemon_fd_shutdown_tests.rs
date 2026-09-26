//! Actual shutdown handler contract; no mounted profile or APK is created.
use super::*;

fn state() -> Arc<State> {
    let paths = ProfilePaths::new(
        std::env::temp_dir().join("da-fd-shutdown-unmounted"),
        "test",
    )
    .unwrap();
    Arc::new(State {
        binder: Default::default(),
        fd_deliveries: crate::host_fd_delivery::transport::new_owner().unwrap(),
        scm: std::sync::Arc::new(crate::scm_service::ScmService::new().unwrap()),
        filesystem: Mutex::new(ProfileFilesystem::new(paths.clone())),
        registry: Mutex::new(PackageRegistry::new(&paths)),
        paths,
        processes: Mutex::new(ProcessRegistry::default()),
        properties: Mutex::new(None),
        build_identity: String::new(),
        host_commands: Default::default(),
        runtime_services: Default::default(),
        application_launches: Default::default(),
        bound_services: Default::default(),
        start_gate: Mutex::new(()),
        bound_service_gate: Mutex::new(()),
        lease_gate: Mutex::new(()),
        leases: AtomicUsize::new(0),
        handlers: AtomicUsize::new(1), // this shutdown handler
        shutdown: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
    })
}

fn request(state: &Arc<State>) -> bool {
    let (mut client, server) = UnixStream::pair().unwrap();
    protocol::write_request(&mut client, protocol::OP_SHUTDOWN, &[]).unwrap();
    handle(server, state).unwrap();
    protocol::expect_ok(&mut client, protocol::OP_SHUTDOWN).is_ok()
}

#[test]
fn shutdown_rejects_pending_delivery_until_actual_guardian_eof() {
    let state = state();
    let (carrier, peer) = UnixStream::pair().unwrap();
    let target = crate::host_fd_delivery::transport::destination(&carrier).unwrap();
    let payload = File::open("/dev/null").unwrap().into();
    let prepared =
        crate::host_fd_delivery::transport::prepare(&state.fd_deliveries, target, vec![payload])
            .unwrap();
    assert!(!request(&state));
    assert!(!state.shutdown.load(Ordering::SeqCst));
    drop(prepared);
    // A child another test forks holds a copy of the guardian's write end
    // until it execs, so EOF can follow the drop by a moment (#81).
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !request(&state) {
        assert!(Instant::now() < deadline, "guardian EOF never observed");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(state.shutdown.load(Ordering::SeqCst));
    drop((carrier, peer));
}

#[test]
fn shutdown_rejects_other_acquisition_handler_before_it_reserves() {
    let state = state();
    state.handlers.store(2, Ordering::SeqCst);
    assert!(!request(&state));
    assert!(!state.shutdown.load(Ordering::SeqCst));
    state.handlers.store(1, Ordering::SeqCst);
    assert!(request(&state));
}
