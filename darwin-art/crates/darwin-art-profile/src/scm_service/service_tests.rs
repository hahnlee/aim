use super::{ScmService, process_epoch};
use crate::{
    ProfileError, process_incarnation::ProcessIncarnation, process_registry::ProcessRegistry,
    protocol,
};
use darwin_art_scm_transfer::inheritance::spawn_owned;
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

struct SocketPath(PathBuf);

impl Drop for SocketPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn listener() -> (SocketPath, UnixListener) {
    // Parallel tests can read the same clock value (#21).
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "da-scm-{}-{suffix}-{serial}.sock",
        std::process::id()
    ));
    let listener = UnixListener::bind(&path).unwrap();
    (SocketPath(path), listener)
}

fn register_request() -> Vec<u8> {
    // SCM v1 header: version, operation, reserved bits, body length.
    vec![
        2,
        super::wire::Operation::RegisterPair as u8,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
}

fn current_process_registry() -> (Arc<Mutex<ProcessRegistry>>, ProcessIncarnation) {
    let pid = std::process::id();
    let birth = ProcessIncarnation::read_live(pid).unwrap();
    let mut registry = ProcessRegistry::default();
    registry
        .acquire(pid, "org.example.scm.service", birth, false)
        .unwrap();
    (Arc::new(Mutex::new(registry)), birth)
}

fn accepted(path: &SocketPath, listener: &UnixListener) -> (UnixStream, UnixStream) {
    let client = UnixStream::connect(&path.0).unwrap();
    let (server, _) = listener.accept().unwrap();
    (client, server)
}

fn pair_server(
    service: Arc<ScmService>,
    processes: Arc<Mutex<ProcessRegistry>>,
    mut server: UnixStream,
) -> JoinHandle<Result<(), ProfileError>> {
    thread::spawn(move || service.serve(&processes, &mut server, &register_request()))
}

#[test]
fn unregistered_native_audit_connection_is_rejected() {
    let service = ScmService::new().unwrap();
    let processes = Mutex::new(ProcessRegistry::default());
    let (path, listener) = listener();
    let (mut client, mut server) = accepted(&path, &listener);
    let result = service.serve(&processes, &mut server, &register_request());
    assert!(
        result.is_err(),
        "unregistered audit-token peer was accepted"
    );
    drop(server);
    let mut byte = [0_u8; 1];
    assert_eq!(client.read(&mut byte).unwrap_or(0), 0);
}

#[test]
fn live_pair_survives_receiver_connection_close_for_same_birth() {
    let service = Arc::new(ScmService::new().unwrap());
    let (processes, _birth) = current_process_registry();
    let (path, listener) = listener();
    let (mut client, server) = accepted(&path, &listener);
    let task = pair_server(Arc::clone(&service), Arc::clone(&processes), server);

    let response = protocol::expect_ok(&mut client, protocol::OP_SCM_SERVICE).unwrap();
    assert!(response.len() >= 8, "pair response was not returned");
    client.write_all(&response).unwrap();
    client.flush().unwrap();
    assert!(
        protocol::expect_ok(&mut client, protocol::OP_SCM_SERVICE)
            .unwrap()
            .is_empty()
    );
    assert!(task.join().unwrap().is_ok());
    drop(client);

    assert!(service.has_pending().unwrap());
    assert_eq!(service.owner.lock().unwrap().counts(), [2, 1, 0]);
    let holder_a = u128::from_le_bytes(response[32..48].try_into().unwrap());
    let holder_b = u128::from_le_bytes(response[48..64].try_into().unwrap());
    let pid = std::process::id();
    let birth = ProcessIncarnation::read_live(pid).unwrap();
    let epoch = process_epoch(pid, birth);
    let mut owner = service.owner.lock().unwrap();
    owner.release_holder(epoch, holder_a).unwrap();
    owner.release_holder(epoch, holder_b).unwrap();
    drop(owner);
    assert!(!service.has_pending().unwrap());
    assert_eq!(service.owner.lock().unwrap().counts(), [0, 0, 0]);
}

#[test]
fn lost_pair_response_rolls_back_exact_initial_grants() {
    let service = Arc::new(ScmService::new().unwrap());
    let (processes, _birth) = current_process_registry();
    let (path, listener) = listener();
    let (mut client, server) = accepted(&path, &listener);
    let task = pair_server(Arc::clone(&service), Arc::clone(&processes), server);

    // Consume the initial framed response so the test exercises the echo
    // phase, then close before sending the exact raw response bytes back.
    let response = protocol::expect_ok(&mut client, protocol::OP_SCM_SERVICE).unwrap();
    assert!(!response.is_empty());
    drop(client);
    assert!(task.join().unwrap().is_err());
    assert_eq!(service.owner.lock().unwrap().counts(), [0, 0, 0]);
}

struct OwnedChild(Option<Child>);

impl OwnedChild {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn id(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }

    fn terminate(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.0 = None;
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[test]
fn exact_child_birth_death_retires_initial_and_settled_holders() {
    let service = ScmService::new().unwrap();
    let mut child = OwnedChild::new(spawn_owned(Command::new("/bin/sleep").arg("10")).unwrap());
    let pid = child.id();
    let birth = ProcessIncarnation::read_live(pid).unwrap();
    let epoch = process_epoch(pid, birth);
    let pair = service.owner.lock().unwrap().register_pair(epoch).unwrap();
    service
        .participants
        .lock()
        .unwrap()
        .track(epoch, birth)
        .unwrap();

    let (payload, _payload_peer) = UnixStream::pair().unwrap();
    let payloads = vec![payload.into()];
    let prepared = service
        .owner
        .lock()
        .unwrap()
        .prepare(
            epoch,
            pair.holder_a.id(),
            &payloads,
            &[(0, pair.holder_a.id())],
        )
        .unwrap();
    let key = prepared.transfer.key();
    // Prepared response layout is fixed by wire v1: header, key, count,
    // managed-count, ordinal, raw DelegationId.
    let delegation = u128::from_le_bytes(prepared.response[44..60].try_into().unwrap());
    drop(prepared);
    let claims = service
        .owner
        .lock()
        .unwrap()
        .admit(epoch, pair.holder_b.id(), key, 1, &[(0, delegation)], &[0])
        .unwrap();
    assert_eq!(claims.len(), 1);
    service
        .owner
        .lock()
        .unwrap()
        .settle(
            epoch,
            key,
            darwin_art_scm_transfer::capabilities::DeliveryDisposition::Finished,
        )
        .unwrap();
    assert_eq!(service.owner.lock().unwrap().counts(), [3, 1, 0]);

    child.terminate();
    assert!(!service.has_pending().unwrap());
    assert_eq!(service.owner.lock().unwrap().counts(), [0, 0, 0]);
}
