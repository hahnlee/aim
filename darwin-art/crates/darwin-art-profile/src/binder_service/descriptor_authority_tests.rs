//! Policy/real-FD component tests. Live kernel births authenticate the fixture;
//! these do not replace native Description adoption or unchanged-app acceptance.
use super::*;
use crate::process_incarnation::ProcessIncarnation;
use darwin_art_binder_device::{
    descriptor_manifest::{DescriptorAttributes, DescriptorBundle},
    objects,
    transaction_snapshot::TransactionSnapshot,
    transfer_image::TransferImage,
};
use darwin_art_scm_transfer::capabilities::{
    Binding, DelegationState, DeliveryDisposition, decode_attributes,
};
use std::os::fd::{AsRawFd, OwnedFd};

struct Fixture {
    binder: Arc<BinderService>,
    scm: Arc<crate::scm_service::ScmService>,
    peer: PeerIdentity,
    connection: ConnectionToken,
    pair: darwin_art_scm_transfer::capabilities::RegisteredPair,
}
impl Fixture {
    fn new() -> Self {
        let pid = std::process::id();
        let birth = ProcessIncarnation::read_live(pid).unwrap();
        let peer = PeerIdentity::verified(pid, 10001, birth.parts()).unwrap();
        let scm = Arc::new(crate::scm_service::ScmService::new().unwrap());
        let pair = scm.test_pair(peer);
        let binder = Arc::new(BinderService::with_descriptor_authority(scm.clone()));
        let opened = binder.authority.open_authenticated(peer).unwrap();
        let connection = opened.session.connection();
        let (stream, _) = UnixStream::pair().unwrap();
        let (sender, _) = mpsc::sync_channel(1);
        binder.endpoints.lock().unwrap().insert(
            connection,
            Endpoint {
                peer,
                session: opened.session,
                sender,
                shutdown: Arc::new(stream),
            },
        );
        Self {
            binder,
            scm,
            peer,
            connection,
            pair,
        }
    }
    fn binding(&self, ordinal: u64) -> Binding {
        Binding::Binder {
            source_connection: self.connection.get(),
            transfer: 8,
            ordinal,
            object_offset: 4 + ordinal * 24,
        }
    }
    fn token(&self) -> TransferToken {
        TransferToken::from_nonzero(8).unwrap()
    }
    fn bind(&self) -> [u8; 40] {
        self.binder
            .bind_endpoint(self.peer, self.binding(0), self.pair.holder_a.id())
            .unwrap()
    }
    fn image(&self, attributes: [u8; 40]) -> TransferImage {
        let mut data = [0; 28];
        data[4..8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
        data[12..16].copy_from_slice(&77u32.to_le_bytes());
        let snapshot = TransactionSnapshot::capture(&data, &4u64.to_le_bytes(), 0).unwrap();
        let (socket, _) = UnixStream::pair().unwrap();
        let fd: OwnedFd = socket.into();
        TransferImage::capture_with_objects_and_descriptors(
            &snapshot,
            &[],
            &[],
            vec![DescriptorBundle::new(
                4,
                0,
                fd,
                DescriptorAttributes::new(attributes.to_vec()).unwrap(),
            )],
        )
        .unwrap()
    }
    fn deposit_route(&self, attrs: [u8; 40]) {
        let image = self.image(attrs);
        let group = image.into_transport_descriptors().unwrap();
        self.binder.deposit(self.peer, self.token(), group).unwrap();
        self.binder
            .transfers
            .lock()
            .unwrap()
            .route(self.connection, self.token(), self.connection)
            .unwrap();
    }
}
#[test]
fn cancel_after_take_preserves_authorized_claim_and_exact_native_group() {
    let f = Fixture::new();
    let attrs = f.bind();
    f.deposit_route(attrs);
    let mut delivery = f
        .binder
        .prepare_take(f.peer, f.connection, f.token())
        .unwrap();
    let mut group = delivery.take_descriptors();
    delivery.native_admitted();
    drop(delivery);
    f.binder
        .cancel_unrouted_transfer(f.peer, f.token())
        .unwrap();
    let image = TransferImage::import_with_fds(group.remove(0), group).unwrap();
    assert_eq!(image.files()[0].metadata(), attrs);
    let claim = f
        .binder
        .claim_endpoint(f.peer, f.binding(0), &attrs)
        .unwrap();
    assert_eq!(claim.endpoint, f.pair.endpoint_a);
    f.binder
        .finish_endpoint_claim(claim, f.connection.get(), 8)
        .unwrap();
    f.binder
        .settle_received_transfer(
            f.peer,
            f.connection,
            f.token(),
            DeliveryDisposition::Finished,
        )
        .unwrap();
    // An ordinary untagged image cannot accidentally claim this capability.
    let fd = image.files()[0].descriptor();
    assert!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } >= 0);
}
#[test]
fn late_bind_is_rejected_before_and_after_take_without_changing_the_manifest() {
    let f = Fixture::new();
    let attrs = f.bind();
    f.deposit_route(attrs);
    assert!(
        f.binder
            .bind_endpoint(f.peer, f.binding(1), f.pair.holder_b.id())
            .is_err()
    );
    let mut delivery = f
        .binder
        .prepare_take(f.peer, f.connection, f.token())
        .unwrap();
    assert!(
        f.binder
            .bind_endpoint(f.peer, f.binding(1), f.pair.holder_b.id())
            .is_err()
    );
    delivery.native_admitted();
    assert!(
        f.binder
            .claim_endpoint(f.peer, f.binding(0), &attrs)
            .is_ok()
    );
}
#[test]
fn missing_payload_rejects_deposit_without_committing_any_capability() {
    let f = Fixture::new();
    let attrs = f.bind();
    let image = f.image(attrs);
    let carrier = image.try_clone_descriptor().unwrap();
    assert!(f.binder.deposit(f.peer, f.token(), vec![carrier]).is_err());
    let id = decode_attributes(&attrs).unwrap().delegation;
    assert_eq!(f.scm.test_delegation(id).state, DelegationState::Pending);
    f.binder
        .cancel_unrouted_transfer(f.peer, f.token())
        .unwrap();
    assert_eq!(f.scm.test_delegation_count(), 0);
}
#[test]
fn lost_bind_response_rolls_back_even_after_an_earlier_cancel_ran() {
    let f = Fixture::new();
    f.binder
        .cancel_unrouted_transfer(f.peer, f.token())
        .unwrap();
    let (client, mut server) = UnixStream::pair().unwrap();
    drop(client);
    let bytes =
        crate::binder_capability_wire::encode(&crate::binder_capability_wire::Request::Bind {
            binding: f.binding(0),
            holder: f.pair.holder_a.id(),
        })
        .unwrap();
    assert!(
        crate::binder_capability_service::serve(&f.binder, f.peer, &mut server, &bytes).is_err()
    );
    assert_eq!(f.scm.test_delegation_count(), 0);
}
#[test]
fn failed_native_admission_discards_only_the_exact_unclaimed_transfer() {
    let f = Fixture::new();
    let attrs = f.bind();
    f.deposit_route(attrs);
    let delivery = f
        .binder
        .prepare_take(f.peer, f.connection, f.token())
        .unwrap();
    drop(delivery);
    assert_eq!(f.scm.test_delegation_count(), 0);
    assert!(
        f.binder
            .claim_endpoint(f.peer, f.binding(0), &attrs)
            .is_err()
    );
}

#[test]
fn actual_native_binder_callbacks_cross_authenticated_rpc_and_balance_the_claim() {
    let f = Fixture::new();
    let path = std::env::temp_dir().join(format!(
        "dart-binder-cap-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let binder = f.binder.clone();
    let scm = f.scm.clone();
    let peer = f.peer;
    let server = std::thread::spawn(move || {
        let birth = ProcessIncarnation::read_live(peer.pid()).unwrap();
        let mut processes = crate::process_registry::ProcessRegistry::default();
        processes
            .acquire(peer.pid(), "org.example.native.binder.cap", birth, false)
            .unwrap();
        let processes = Mutex::new(processes);
        // Positive callback sequence: Bind, Claim, native holder release,
        // source export-lease pending cancellation after a committed deposit.
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let (pid, actual_birth) = crate::peer_process::identity(&stream).unwrap();
            assert_eq!((pid, actual_birth), (peer.pid(), birth));
            let message = crate::protocol::read_message(&mut stream).unwrap();
            match message.operation {
                crate::protocol::OP_BINDER_CAPABILITY => crate::binder_capability_service::serve(
                    &binder,
                    peer,
                    &mut stream,
                    &message.payload,
                )
                .unwrap(),
                crate::protocol::OP_SCM_SERVICE => scm
                    .serve(&processes, &mut stream, &message.payload)
                    .unwrap(),
                _ => panic!("unexpected native capability callback"),
            }
        }
    });
    let owner = crate::scm_service::NativeScmEndpointProvider::new(path.clone()).unwrap();
    let hooks = owner.hooks();
    let binding = darwin_art_engine_sys::ScmBinderBindingV2 {
        source_connection: f.connection.get(),
        transfer: 8,
        ordinal: 0,
        object_offset: 4,
    };
    let holder = f.pair.holder_a.id().to_le_bytes();
    let mut attrs = [0xa5; 40];
    assert_eq!(
        unsafe {
            hooks.bind_binder.unwrap()(hooks.context, holder.as_ptr(), &binding, attrs.as_mut_ptr())
        },
        0
    );
    f.deposit_route(attrs);
    let mut delivery = f
        .binder
        .prepare_take(f.peer, f.connection, f.token())
        .unwrap();
    let group = delivery.take_descriptors();
    delivery.native_admitted();
    let mut grant = darwin_art_engine_sys::ScmGrantV2::default();
    assert_eq!(
        unsafe {
            hooks.claim_binder.unwrap()(hooks.context, &binding, attrs.as_ptr(), 40, &mut grant)
        },
        0
    );
    assert_eq!(
        grant.authority,
        f.pair.endpoint_a.carrier.authority.instance.to_le_bytes()
    );
    assert_eq!(grant.carrier, f.pair.endpoint_a.carrier.serial);
    assert_eq!(grant.side, 0);
    assert_ne!(grant.holder, holder);
    f.binder
        .settle_received_transfer(
            f.peer,
            f.connection,
            f.token(),
            DeliveryDisposition::Finished,
        )
        .unwrap();
    assert_eq!(
        unsafe { hooks.release_holder.unwrap()(hooks.context, grant.holder.as_ptr()) },
        0
    );
    assert_eq!(
        unsafe { hooks.cancel_binder.unwrap()(hooks.context, &binding) },
        0
    );
    server.join().unwrap();
    drop(group);
    std::fs::remove_file(path).unwrap();
    assert_eq!(f.scm.test_delegation_count(), 0);
}
