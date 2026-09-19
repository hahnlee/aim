//! Component tests of the real server admission path, never product fixtures.
use super::*;
use crate::bound_service_child_control::BoundServiceChildControl;
use crate::runtime_service_protocol::{ReadinessMask, RuntimeKey, StartRuntimeRequest};

struct SocketPath(std::path::PathBuf);

impl Drop for SocketPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn existing_admission(
    exact_template_owner: bool,
) -> Result<BoundServiceProcessResponse, ProfileError> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let socket_path = SocketPath(
        std::env::temp_dir().join(format!("da-admit-{}-{nonce}.sock", std::process::id())),
    );
    let listener = UnixListener::bind(&socket_path.0).unwrap();
    let _client = UnixStream::connect(&socket_path.0).unwrap();
    let (stream, _) = listener.accept().unwrap();
    let (pid, incarnation) = crate::peer_process::identity(&stream).unwrap();
    let paths = ProfilePaths::new(
        std::env::temp_dir().join(format!("da-admit-profile-{}-{nonce}", std::process::id())),
        "test",
    )
    .unwrap();
    // No mount, package files or child process is created by this Existing fixture.
    let state = Arc::new(State {
        fd_deliveries: crate::host_fd_delivery::transport::new_owner().unwrap(),
        scm: std::sync::Arc::new(crate::scm_service::ScmService::new().unwrap()),
        binder: Default::default(),
        filesystem: Mutex::new(ProfileFilesystem::new(paths.clone())),
        registry: Mutex::new(PackageRegistry::new(&paths)),
        paths,
        processes: Mutex::new(ProcessRegistry::default()),
        runtime_services: Default::default(),
        application_launches: Default::default(),
        bound_services: Default::default(),
        start_gate: Mutex::new(()),
        bound_service_gate: Mutex::new(()),
        lease_gate: Mutex::new(()),
        leases: AtomicUsize::new(0),
        handlers: AtomicUsize::new(0),
        shutdown: AtomicBool::new(false),
        last_activity: Mutex::new(Instant::now()),
    });
    // Registry preauthentication succeeds in BOTH cases. Only the exact
    // current runtime instance may authorize its immutable command template.
    state
        .processes
        .lock()
        .unwrap()
        .acquire(pid, "android.system", incarnation, true)
        .unwrap();
    let template = StartRuntimeRequest {
        package: "android.system".into(),
        key: RuntimeKey([1; 32]),
        required: ReadinessMask::BINDER,
        arguments: vec!["unused-existing-fixture".into()],
        environment: Vec::new(),
    };
    let (instance, _) = state.runtime_services.reserve(&template).unwrap();
    let template_incarnation = if exact_template_owner {
        incarnation
    } else {
        ProcessIncarnation::fixture(u64::MAX)
    };
    instance.attach(pid, template_incarnation).unwrap();
    let request = BoundServiceProcessRequest {
        package: "android.system".into(),
        process_name: "android.system:fixture".into(),
        uid: 1000,
        isolated: false,
        start_sequence: 7,
    };
    let existing = BoundServiceProcessResponse {
        pid: 42,
        start_sequence: 7,
        incarnation: [3, 4],
        activation_token: [5; 16],
    };
    state
        .bound_services
        .insert(
            request.identity(),
            BoundServiceRecord::new(existing, Arc::new(BoundServiceChildControl::new(None))),
        )
        .unwrap();
    start_bound_service(&state, &request.encode().unwrap(), &stream)
}

#[test]
fn existing_response_cannot_bypass_exact_current_system_template_owner() {
    let error = existing_admission(false).unwrap_err().to_string();
    assert!(error.contains("not its exact child"), "{error}");
}

#[test]
fn exact_attached_starting_system_can_coalesce_existing_response() {
    let response = existing_admission(true).unwrap();
    assert_eq!(response.pid, 42);
    assert_eq!(response.start_sequence, 7);
    assert_eq!(response.activation_token, [5; 16]);
}
