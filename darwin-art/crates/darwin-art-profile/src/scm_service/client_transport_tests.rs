use super::*;
use crate::scm_service::ScmService;
use crate::{process_incarnation::ProcessIncarnation, process_registry::ProcessRegistry};
use std::{
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::UnixListener,
    },
    sync::{Arc, Mutex},
    thread,
};

struct Fixture {
    path: std::path::PathBuf,
    listener: UnixListener,
    service: Arc<ScmService>,
    processes: Arc<Mutex<ProcessRegistry>>,
}

impl Fixture {
    fn new() -> Self {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dart-scm-client-{}-{suffix}.sock",
            std::process::id()
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let birth = ProcessIncarnation::read_live(std::process::id()).unwrap();
        let mut processes = ProcessRegistry::default();
        processes
            .acquire(
                std::process::id(),
                "org.example.client.transport",
                birth,
                false,
            )
            .unwrap();
        Self {
            path,
            listener,
            service: Arc::new(ScmService::new().unwrap()),
            processes: Arc::new(Mutex::new(processes)),
        }
    }

    fn operation(&self) -> (UnixStream, thread::JoinHandle<Result<(), ProfileError>>) {
        let client = connect(&self.path).unwrap();
        let (mut server, _) = self.listener.accept().unwrap();
        let service = self.service.clone();
        let processes = self.processes.clone();
        let job = thread::spawn(move || {
            let request = protocol::read_message(&mut server)?;
            assert_eq!(request.operation, protocol::OP_SCM_SERVICE);
            service.serve(&processes, &mut server, &request.payload)
        });
        (client, job)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn client_install_failure_closes_connection_and_server_rolls_back_grants() {
    let fixture = Fixture::new();
    let (stream, job) = fixture.operation();
    assert!(register_pair::<()>(stream, |_| Err(failed("controlled install failure"))).is_err());
    assert!(job.join().unwrap().is_err());
    assert!(!fixture.service.has_pending().unwrap());
}

#[test]
fn actual_authenticated_prepare_returns_owned_metadata_and_guardian() {
    let fixture = Fixture::new();
    let (stream, job) = fixture.operation();
    // TEST-ONLY attribute sink: tests codec/receipt transport, NOT native
    // descriptions or managed carrier adoption. No frame is fabricated.
    let (holder_a, holder_b) =
        register_pair(stream, |offer| Ok((offer.holder_a, offer.holder_b))).unwrap();
    job.join().unwrap().unwrap();
    let file = std::fs::File::open("/dev/null").unwrap();
    let (stream, job) = fixture.operation();
    let prepared = prepare(stream, holder_a, &[file.as_fd()], &[]).unwrap();
    job.join().unwrap().unwrap();
    assert_eq!(prepared.offer.payload_count, 1);
    assert!(prepared.offer.managed.is_empty());
    for descriptor in [&prepared.metadata, &prepared.guardian] {
        assert_ne!(
            unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    assert_eq!(
        unsafe { libc::fcntl(prepared.metadata.as_raw_fd(), libc::F_GETFL) } & libc::O_ACCMODE,
        libc::O_RDONLY
    );
    let (stream, job) = fixture.operation();
    assert!(
        admit(
            stream,
            &Request::Admit {
                carrier_holder: holder_b,
                key: prepared.offer.key,
                payload_count: 1,
                managed: vec![],
                publish_ordinals: vec![],
            }
        )
        .unwrap()
        .is_empty()
    );
    job.join().unwrap().unwrap();
    let (stream, job) = fixture.operation();
    settle_or_release(
        stream,
        &Request::Settle {
            key: prepared.offer.key,
            disposition: darwin_art_scm_transfer::capabilities::DeliveryDisposition::Finished,
        },
    )
    .unwrap();
    job.join().unwrap().unwrap();
    assert!(fixture.service.owner.lock().unwrap().has_pending().unwrap());
    drop(prepared);
    // Observe real guardian EOF, never retire aliases by this test deadline.
    let deadline = Instant::now() + Duration::from_secs(1);
    while fixture.service.owner.lock().unwrap().has_pending().unwrap() {
        assert!(Instant::now() < deadline, "real guardian EOF not observed");
        thread::yield_now();
    }
    for holder in [holder_a, holder_b] {
        let (stream, job) = fixture.operation();
        settle_or_release(stream, &Request::ReleaseHolder { holder }).unwrap();
        job.join().unwrap().unwrap();
    }
    assert!(!fixture.service.has_pending().unwrap());
}
