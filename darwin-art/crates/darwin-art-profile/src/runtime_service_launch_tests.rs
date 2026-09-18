use super::*;
use crate::runtime_service_protocol::{
    InstanceToken, LossRequest, ReadinessMask, ReadyRequest, RuntimeKey,
};
use crate::runtime_service_state::RuntimeServiceState;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Child;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn externally_consumed_wait_status_settles_exact_registration() {
    externally_consumed_wait_status();
}

#[test]
fn externally_consumed_status_after_live_check_has_terminal_proof() {
    externally_consumed_wait_status();
}

fn externally_consumed_wait_status() {
    let child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    let exits = Arc::new(AtomicUsize::new(0));
    let callback_exits = Arc::clone(&exits);
    let mut owner = crate::process_wait::ProcessWaitOwner::from_child(
        child,
        incarnation,
        Box::new(move || {
            callback_exits.fetch_add(1, Ordering::SeqCst);
        }),
    );
    assert!(matches!(
        owner.poll(),
        crate::process_wait::PollOutcome::Pending
    ));
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGTERM) }, 0);
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid as i32, &mut status, 0) },
        pid as i32
    );
    assert!(matches!(
        owner.poll(),
        crate::process_wait::PollOutcome::GoneWithoutStatus
    ));
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(matches!(
        owner.poll(),
        crate::process_wait::PollOutcome::GoneWithoutStatus
    ));
    assert_eq!(exits.load(Ordering::SeqCst), 1);
}

#[test]
fn timeout_reaps_only_owned_child_before_reservation_can_restart() {
    let services = RuntimeServiceState::default();
    let request = StartRuntimeRequest {
        package: "org.example.timeoutfixture".into(),
        key: RuntimeKey([8; 32]),
        required: ReadinessMask::BINDER,
        arguments: vec!["/bin/sleep".into(), "60".into()],
        environment: vec![],
    };
    let (instance, _) = services.reserve(&request).unwrap();
    let pid = Arc::new(AtomicUsize::new(0));
    let exits = Arc::new(AtomicUsize::new(0));
    let child_pid = Arc::clone(&pid);
    let child_exits = Arc::clone(&exits);
    launch_with_timeout(
        Arc::clone(&instance),
        &request,
        Path::new("/tmp"),
        Path::new("/tmp/not-used"),
        move |value| {
            child_pid.store(value as usize, Ordering::SeqCst);
            Ok(Box::new(move || {
                child_exits.fetch_add(1, Ordering::SeqCst);
            }))
        },
        Duration::from_millis(50),
    )
    .unwrap();
    assert!(instance.wait_ready(Duration::from_secs(5)).is_err());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !instance.snapshot().unwrap().is_exited && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(instance.snapshot().unwrap().is_exited);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(ProcessIncarnation::read(pid.load(Ordering::SeqCst) as u32).is_err());
    assert!(services.reserve(&request).unwrap().1);
}

fn accept(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("fixture accept: {error}"),
        }
    }
}

struct ReapOnDrop(Option<Child>);

impl ReapOnDrop {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.as_mut().unwrap().try_wait()
    }
}

impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

#[test]
#[ignore = "child fixture invoked by owned runtime child supervision tests"]
fn runtime_child_fixture() {
    // SAFETY: isolated fixture subprocess, before any fixture threads or
    // environment access. Exercises the same startup barrier as the real host.
    unsafe {
        crate::wait_for_process_registration().unwrap();
    }
    let token = std::env::var(INSTANCE_TOKEN_ENV).unwrap();
    assert_eq!(token.len(), 32);
    if let Ok(binder) = std::env::var("DARWIN_ART_SYSTEM_SERVER_SOCKET") {
        assert_eq!(binder, format!("/tmp/{token}.system.sock"));
        assert_eq!(
            std::env::var("DARWIN_ART_SURFACEFLINGER_SOCKET").unwrap(),
            format!("/tmp/{token}.sf.sock")
        );
    }
    let mut bytes = [0; 16];
    for (index, value) in bytes.iter_mut().enumerate() {
        *value = u8::from_str_radix(&token[index * 2..index * 2 + 2], 16).unwrap();
    }
    crate::publish_runtime_ready(
        Path::new(&std::env::var(PROFILE_SOCKET_ENV).unwrap()),
        ReadyRequest {
            token: InstanceToken(bytes),
            readiness: ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR),
        },
    )
    .unwrap();
    let gate = std::env::var_os("DARWIN_ART_TEST_RUNTIME_LOSS_TRIGGER").map(|_| {
        UnixStream::connect(std::env::var("DARWIN_ART_TEST_RUNTIME_GATE").unwrap()).unwrap()
    });
    if let Some(trigger) = std::env::var_os("DARWIN_ART_TEST_RUNTIME_LOSS_TRIGGER") {
        let mut trigger = UnixStream::connect(trigger).unwrap();
        trigger.read_to_end(&mut Vec::new()).unwrap();
    }
    if let Some(mask) = std::env::var_os("DARWIN_ART_TEST_RUNTIME_LOSS_MASK") {
        let lost = mask.to_str().unwrap().parse::<u32>().unwrap();
        let _ = crate::publish_runtime_lost(
            Path::new(&std::env::var(PROFILE_SOCKET_ENV).unwrap()),
            LossRequest {
                token: InstanceToken(bytes),
                lost: ReadinessMask::new(lost).unwrap(),
            },
        );
    }
    let mut gate = gate.unwrap_or_else(|| {
        UnixStream::connect(std::env::var("DARWIN_ART_TEST_RUNTIME_GATE").unwrap()).unwrap()
    });
    gate.read_to_end(&mut Vec::new()).unwrap();
}

#[test]
fn owned_runtime_child_protocol_and_reap() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let socket = std::path::PathBuf::from(format!("/tmp/dar-{}-{stamp}.sock", std::process::id()));
    let gate_path = socket.with_extension("gate");
    let listener = UnixListener::bind(&socket).unwrap();
    let gate_listener = UnixListener::bind(&gate_path).unwrap();
    let services = Arc::new(RuntimeServiceState::default());
    let request = StartRuntimeRequest {
        package: "org.example.runtimefixture".into(),
        key: RuntimeKey([7; 32]),
        required: ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR),
        arguments: vec![
            std::env::current_exe().unwrap().into_os_string(),
            "--exact".into(),
            "runtime_service_launch::tests::runtime_child_fixture".into(),
            "--ignored".into(),
        ],
        environment: vec![
            (
                "DARWIN_ART_TEST_RUNTIME_GATE".into(),
                gate_path.clone().into_os_string(),
            ),
            (
                "DARWIN_ART_SYSTEM_SERVER_SOCKET".into(),
                "/tmp/legacy.system.sock".into(),
            ),
            (
                "DARWIN_ART_SURFACEFLINGER_SOCKET".into(),
                "/tmp/legacy.sf.sock".into(),
            ),
        ],
    };
    let (instance, fresh) = services.reserve(&request).unwrap();
    assert!(fresh);
    let endpoint_owner = Arc::clone(&services);
    let transport = std::thread::spawn(move || {
        let mut stream = accept(&listener);
        let message = crate::protocol::read_message(&mut stream).unwrap();
        assert_eq!(message.operation, crate::protocol::OP_RUNTIME_READY);
        let (pid, incarnation) = crate::peer_process::identity(&stream).unwrap();
        endpoint_owner
            .publish_ready(
                pid,
                incarnation,
                ReadyRequest::decode(&message.payload).unwrap(),
            )
            .unwrap();
        crate::protocol::write_response(&mut stream, message.operation, 0, b"").unwrap();
    });
    let exits = Arc::new(AtomicUsize::new(0));
    let exit_count = Arc::clone(&exits);
    launch(
        Arc::clone(&instance),
        &request,
        Path::new("/tmp"),
        &socket,
        move |_| {
            Ok(Box::new(move || {
                exit_count.fetch_add(1, Ordering::SeqCst);
            }))
        },
    )
    .unwrap();
    let response = instance.wait_ready(Duration::from_secs(15)).unwrap();
    assert_ne!(response.pid, std::process::id());
    let (reused, fresh) = services.reserve(&request).unwrap();
    assert!(!fresh && Arc::ptr_eq(&instance, &reused));
    let gate = accept(&gate_listener);
    transport.join().unwrap();
    drop(gate); // Release only this fixture child; supervisor must reap it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !instance.snapshot().unwrap().is_exited && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(instance.snapshot().unwrap().is_exited);
    assert_eq!(exits.load(Ordering::SeqCst), 1);
    assert!(ProcessIncarnation::read(response.pid).is_err());
    drop(gate_listener);
    std::fs::remove_file(socket).unwrap();
    std::fs::remove_file(gate_path).unwrap();
}

#[test]
fn owned_runtime_child_readiness_loss_kills_only_that_child() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let socket =
        std::path::PathBuf::from(format!("/tmp/dar-loss-{}-{stamp}.sock", std::process::id()));
    let gate_path = socket.with_extension("gate");
    let trigger_path = socket.with_extension("trigger");
    let listener = UnixListener::bind(&socket).unwrap();
    let gate_listener = UnixListener::bind(&gate_path).unwrap();
    let trigger_listener = UnixListener::bind(&trigger_path).unwrap();
    let services = Arc::new(RuntimeServiceState::default());
    let request = StartRuntimeRequest {
        package: "org.example.lossfixture".into(),
        key: RuntimeKey([9; 32]),
        required: ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR),
        arguments: vec![
            std::env::current_exe().unwrap().into_os_string(),
            "--exact".into(),
            "runtime_service_launch::tests::runtime_child_fixture".into(),
            "--ignored".into(),
        ],
        environment: vec![
            (
                "DARWIN_ART_TEST_RUNTIME_GATE".into(),
                gate_path.clone().into_os_string(),
            ),
            (
                "DARWIN_ART_TEST_RUNTIME_LOSS_TRIGGER".into(),
                trigger_path.clone().into_os_string(),
            ),
            ("DARWIN_ART_TEST_RUNTIME_LOSS_MASK".into(), "2".into()),
        ],
    };
    let (instance, fresh) = services.reserve(&request).unwrap();
    assert!(fresh);
    let endpoint_owner = Arc::clone(&services);
    let transport = std::thread::spawn(move || {
        let mut stream = accept(&listener);
        let message = crate::protocol::read_message(&mut stream).unwrap();
        assert_eq!(message.operation, crate::protocol::OP_RUNTIME_READY);
        let (pid, incarnation) = crate::peer_process::identity(&stream).unwrap();
        endpoint_owner
            .publish_ready(
                pid,
                incarnation,
                ReadyRequest::decode(&message.payload).unwrap(),
            )
            .unwrap();
        crate::protocol::write_response(&mut stream, message.operation, 0, b"").unwrap();

        let mut stream = accept(&listener);
        let message = crate::protocol::read_message(&mut stream).unwrap();
        assert_eq!(message.operation, crate::protocol::OP_RUNTIME_LOST);
        let (pid, incarnation) = crate::peer_process::identity(&stream).unwrap();
        endpoint_owner
            .publish_runtime_lost(
                pid,
                incarnation,
                LossRequest::decode(&message.payload).unwrap(),
            )
            .unwrap();
        let _ = crate::protocol::write_response(&mut stream, message.operation, 0, b"");
    });

    // This unrelated process must remain alive while the owned runtime child
    // is killed in response to the authenticated required-capability loss.
    let mut sibling = ReapOnDrop::new(
        std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap(),
    );
    let child_pid = Arc::new(AtomicUsize::new(0));
    let child_pid_for_callback = Arc::clone(&child_pid);
    launch(
        Arc::clone(&instance),
        &request,
        Path::new("/tmp"),
        &socket,
        move |pid| {
            child_pid_for_callback.store(pid as usize, Ordering::SeqCst);
            Ok(Box::new(|| {}))
        },
    )
    .unwrap();

    // Keep the child in its post-loss gate until the supervisor's OwnedChild
    // Drop has proved kill/reap.  The trigger makes the Ready response and
    // loss notification ordering deterministic rather than sleep-based.
    let gate = accept(&gate_listener);
    let trigger = accept(&trigger_listener);
    let response = instance.wait_ready(Duration::from_secs(15)).unwrap();
    assert_eq!(response.pid, child_pid.load(Ordering::SeqCst) as u32);
    drop(trigger);
    transport.join().unwrap();
    let snapshot = instance.snapshot().unwrap();
    assert!(snapshot.is_failed || snapshot.is_exited);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !instance.snapshot().unwrap().is_exited && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(instance.snapshot().unwrap().is_exited);
    assert!(ProcessIncarnation::read(response.pid).is_err());
    assert!(sibling.try_wait().unwrap().is_none());

    drop(gate);
    drop(trigger_listener);
    drop(gate_listener);
    std::fs::remove_file(socket).unwrap();
    std::fs::remove_file(gate_path).unwrap();
    std::fs::remove_file(trigger_path).unwrap();
}
