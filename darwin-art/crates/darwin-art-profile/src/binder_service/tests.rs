use super::*;
use darwin_art_binder_device::authority_protocol::{
    CallToken, LocalNodeToken, NodeToken, TransactionFailure, TransferToken,
};
use darwin_art_binder_device::{
    transaction_snapshot::TransactionSnapshot, transfer_image::TransferImage,
};
use std::os::fd::AsFd;

fn peer(pid: u32) -> PeerIdentity {
    PeerIdentity::verified(pid, 10_000 + pid, [pid as u64, 0]).unwrap()
}

fn system_peer(pid: u32) -> PeerIdentity {
    PeerIdentity::verified(pid, 1000, [pid as u64, 0]).unwrap()
}

fn local(raw: u64) -> LocalNodeToken {
    LocalNodeToken::from_nonzero(raw).unwrap()
}

fn transfer(raw: u64) -> TransferToken {
    TransferToken::from_nonzero(raw).unwrap()
}

fn descriptor() -> OwnedFd {
    let snapshot = TransactionSnapshot::capture(b"parcel", &[], 0).unwrap();
    TransferImage::capture(&snapshot, &[])
        .unwrap()
        .try_clone_descriptor()
        .unwrap()
}

fn open(
    service: Arc<BinderService>,
    pid: u32,
) -> (UnixStream, thread::JoinHandle<Result<(), ProfileError>>) {
    let (client, server) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || service.serve(server, peer(pid)));
    (client, worker)
}

fn open_peer(
    service: Arc<BinderService>,
    identity: PeerIdentity,
) -> (UnixStream, thread::JoinHandle<Result<(), ProfileError>>) {
    let (client, server) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || service.serve(server, identity));
    (client, worker)
}

fn opened(stream: &mut UnixStream) -> ConnectionToken {
    let Message::ConnectionOpened { connection, .. } = authority_protocol::decode(stream).unwrap()
    else {
        panic!("expected connection-open response")
    };
    connection
}

fn sync_call(message: Message) -> CallToken {
    let Message::RouteAccepted { call: Some(call) } = message else {
        panic!("expected synchronous route acceptance")
    };
    call
}

#[test]
fn two_real_unix_peers_publish_route_reply_and_close() {
    let service = Arc::new(BinderService::default());
    let (mut caller, caller_worker) = open(Arc::clone(&service), 10);
    let (mut callee, callee_worker) = open(Arc::clone(&service), 20);
    let caller_id = opened(&mut caller);
    let callee_id = opened(&mut callee);

    authority_protocol::encode(&mut callee, Message::PublishNode { local: local(7) }).unwrap();
    let Message::NodePublished { node } = authority_protocol::decode(&mut callee).unwrap() else {
        panic!("expected node publication")
    };
    assert_eq!(node, NodeToken::new(callee_id, local(7)));

    service
        .deposit(peer(10), transfer(8), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut caller,
        Message::RouteTransaction {
            target: node,
            caller_thread: 44,
            transfer: transfer(8),
            code: 9,
            flags: 0,
        },
    )
    .unwrap();
    let call = sync_call(authority_protocol::decode(&mut caller).unwrap());
    assert_eq!(
        authority_protocol::decode(&mut callee).unwrap(),
        Message::DeliverTransaction {
            call: Some(call),
            sender: caller_id,
            sender_pid: 10,
            sender_euid: 10_010,
            target: local(7),
            caller_thread: 44,
            transfer: transfer(8),
            code: 9,
            flags: 0,
        }
    );
    let mut descriptors = service.take(peer(20), caller_id, transfer(8)).unwrap();
    assert_eq!(
        TransferImage::import(descriptors.remove(0)).unwrap().data(),
        b"parcel"
    );

    service
        .deposit(peer(20), transfer(10), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut callee,
        Message::CompleteReply {
            call,
            transfer: transfer(10),
            code: 11,
            flags: 12,
        },
    )
    .unwrap();
    assert_eq!(
        authority_protocol::decode(&mut callee).unwrap(),
        Message::ReplyAccepted { call }
    );
    assert_eq!(
        authority_protocol::decode(&mut caller).unwrap(),
        Message::DeliverReply {
            call,
            source: callee_id,
            sender_pid: 20,
            sender_euid: 10_020,
            target_thread: 44,
            transfer: transfer(10),
            code: 11,
            flags: 12,
        }
    );
    let mut descriptors = service.take(peer(10), callee_id, transfer(10)).unwrap();
    assert_eq!(
        TransferImage::import(descriptors.remove(0)).unwrap().data(),
        b"parcel"
    );

    authority_protocol::encode(&mut caller, Message::CloseConnection).unwrap();
    authority_protocol::encode(&mut callee, Message::CloseConnection).unwrap();
    assert!(caller_worker.join().unwrap().is_ok());
    assert!(callee_worker.join().unwrap().is_ok());
}

#[test]
fn stale_target_rejects_only_the_transaction_and_keeps_sender_alive() {
    let service = Arc::new(BinderService::default());
    let (mut caller, caller_worker) = open(Arc::clone(&service), 10);
    let (mut callee, callee_worker) = open(Arc::clone(&service), 20);
    let caller_id = opened(&mut caller);
    let _callee_id = opened(&mut callee);

    authority_protocol::encode(&mut callee, Message::PublishNode { local: local(7) }).unwrap();
    let Message::NodePublished { node } = authority_protocol::decode(&mut callee).unwrap() else {
        panic!("expected node publication")
    };
    authority_protocol::encode(&mut callee, Message::CloseConnection).unwrap();
    callee_worker.join().unwrap().unwrap();

    service
        .deposit(peer(10), transfer(8), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut caller,
        Message::RouteTransaction {
            target: node,
            caller_thread: 44,
            transfer: transfer(8),
            code: 9,
            flags: 0,
        },
    )
    .unwrap();
    assert_eq!(
        authority_protocol::decode(&mut caller).unwrap(),
        Message::RouteRejected {
            reason: TransactionFailure::DeadReply,
        }
    );
    assert!(matches!(
        service
            .transfers
            .lock()
            .unwrap()
            .ensure_pending(caller_id, transfer(8)),
        Err(crate::binder_transfer::Error::Unknown)
    ));

    service
        .deposit(peer(10), transfer(9), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut caller,
        Message::RouteTransaction {
            target: node,
            caller_thread: 44,
            transfer: transfer(9),
            code: 10,
            flags: 1,
        },
    )
    .unwrap();
    assert_eq!(
        authority_protocol::decode(&mut caller).unwrap(),
        Message::RouteRejected {
            reason: TransactionFailure::DeadReply,
        }
    );
    assert!(matches!(
        service
            .transfers
            .lock()
            .unwrap()
            .ensure_pending(caller_id, transfer(9)),
        Err(crate::binder_transfer::Error::Unknown)
    ));

    authority_protocol::encode(&mut caller, Message::GetContextManager).unwrap();
    assert_eq!(
        authority_protocol::decode(&mut caller).unwrap(),
        Message::ContextManagerFound { node: None }
    );
    authority_protocol::encode(&mut caller, Message::CloseConnection).unwrap();
    caller_worker.join().unwrap().unwrap();
}

#[test]
fn caller_disconnect_does_not_kill_callee_on_late_reply() {
    let service = Arc::new(BinderService::default());
    let (mut caller, caller_worker) = open(Arc::clone(&service), 10);
    let (mut callee, callee_worker) = open(Arc::clone(&service), 20);
    let caller_id = opened(&mut caller);
    let _callee_id = opened(&mut callee);

    authority_protocol::encode(&mut callee, Message::PublishNode { local: local(7) }).unwrap();
    let Message::NodePublished { node } = authority_protocol::decode(&mut callee).unwrap() else {
        panic!("expected node publication")
    };
    service
        .deposit(peer(10), transfer(8), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut caller,
        Message::RouteTransaction {
            target: node,
            caller_thread: 44,
            transfer: transfer(8),
            code: 9,
            flags: 0,
        },
    )
    .unwrap();
    let call = sync_call(authority_protocol::decode(&mut caller).unwrap());
    assert!(matches!(
        authority_protocol::decode(&mut callee).unwrap(),
        Message::DeliverTransaction { call: Some(delivered), sender, .. }
            if delivered == call && sender == caller_id
    ));

    authority_protocol::encode(&mut caller, Message::CloseConnection).unwrap();
    assert!(caller_worker.join().unwrap().is_ok());
    assert_eq!(
        authority_protocol::decode(&mut callee).unwrap(),
        Message::CallerDead { call }
    );

    service
        .deposit(peer(20), transfer(10), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut callee,
        Message::CompleteReply {
            call,
            transfer: transfer(10),
            code: 0,
            flags: 0,
        },
    )
    .unwrap();
    assert_eq!(
        authority_protocol::decode(&mut callee).unwrap(),
        Message::ReplyAccepted { call }
    );

    authority_protocol::encode(&mut callee, Message::CloseConnection).unwrap();
    assert!(callee_worker.join().unwrap().is_ok());
}

#[test]
fn data_plane_crosses_real_scm_rights_boundaries_exactly_once() {
    let service = Arc::new(BinderService::default());
    let (mut caller, caller_worker) = open(Arc::clone(&service), 10);
    let (mut callee, callee_worker) = open(Arc::clone(&service), 20);
    let caller_id = opened(&mut caller);
    let callee_id = opened(&mut callee);

    authority_protocol::encode(&mut callee, Message::PublishNode { local: local(7) }).unwrap();
    let Message::NodePublished { node } = authority_protocol::decode(&mut callee).unwrap() else {
        panic!("expected node publication")
    };

    let snapshot = TransactionSnapshot::capture(b"immutable parcel", &[], 0).unwrap();
    let image = TransferImage::capture(&snapshot, &[]).unwrap();
    let (deposit_sender, deposit_receiver) = UnixStream::pair().unwrap();
    crate::fd_passing::send_one(&deposit_sender, image.descriptor()).unwrap();
    service
        .receive_deposit(peer(10), transfer(33), &deposit_receiver)
        .unwrap();
    drop(image);

    authority_protocol::encode(
        &mut caller,
        Message::RouteTransaction {
            target: node,
            caller_thread: 5,
            transfer: transfer(33),
            code: 8,
            flags: 0,
        },
    )
    .unwrap();
    sync_call(authority_protocol::decode(&mut caller).unwrap());
    authority_protocol::decode(&mut callee).unwrap();

    let (take_sender, take_receiver) = UnixStream::pair().unwrap();
    let descriptors = service
        .prepare_take(peer(20), caller_id, transfer(33))
        .unwrap()
        .into_descriptors();
    crate::fd_passing::send_one(&take_sender, descriptors[0].as_fd()).unwrap();
    let received = crate::fd_passing::receive_one(&take_receiver).unwrap();
    assert_eq!(
        TransferImage::import(received).unwrap().data(),
        b"immutable parcel"
    );
    assert!(
        service
            .prepare_take(peer(20), caller_id, transfer(33))
            .is_err()
    );

    authority_protocol::encode(&mut caller, Message::CloseConnection).unwrap();
    authority_protocol::encode(&mut callee, Message::CloseConnection).unwrap();
    assert!(caller_worker.join().unwrap().is_ok());
    assert!(callee_worker.join().unwrap().is_ok());
    assert_ne!(callee_id, caller_id);
}

#[test]
fn system_context_manager_is_visible_to_an_app_over_real_control_streams() {
    let service = Arc::new(BinderService::default());
    let (mut system, system_worker) = open_peer(Arc::clone(&service), system_peer(20));
    let (mut app, app_worker) = open(Arc::clone(&service), 10);
    let system_id = opened(&mut system);
    opened(&mut app);

    authority_protocol::encode(&mut system, Message::PublishNode { local: local(7) }).unwrap();
    authority_protocol::decode(&mut system).unwrap();
    authority_protocol::encode(&mut system, Message::SetContextManager { local: local(7) })
        .unwrap();
    assert_eq!(
        authority_protocol::decode(&mut system).unwrap(),
        Message::ContextManagerSet {
            node: NodeToken::new(system_id, local(7))
        }
    );

    authority_protocol::encode(&mut app, Message::GetContextManager).unwrap();
    assert_eq!(
        authority_protocol::decode(&mut app).unwrap(),
        Message::ContextManagerFound {
            node: Some(NodeToken::new(system_id, local(7)))
        }
    );

    authority_protocol::encode(&mut app, Message::CloseConnection).unwrap();
    authority_protocol::encode(&mut system, Message::CloseConnection).unwrap();
    assert!(app_worker.join().unwrap().is_ok());
    assert!(system_worker.join().unwrap().is_ok());
}

#[test]
fn abrupt_target_disconnect_is_delivered_as_target_death() {
    let service = Arc::new(BinderService::default());
    let (mut caller, caller_worker) = open(Arc::clone(&service), 10);
    let (mut callee, callee_worker) = open(Arc::clone(&service), 20);
    opened(&mut caller);
    let callee_id = opened(&mut callee);
    authority_protocol::encode(&mut callee, Message::PublishNode { local: local(1) }).unwrap();
    authority_protocol::decode(&mut callee).unwrap();
    service
        .deposit(peer(10), transfer(1), vec![descriptor()])
        .unwrap();
    authority_protocol::encode(
        &mut caller,
        Message::RouteTransaction {
            target: NodeToken::new(callee_id, local(1)),
            caller_thread: 3,
            transfer: transfer(1),
            code: 0,
            flags: 0,
        },
    )
    .unwrap();
    let call = sync_call(authority_protocol::decode(&mut caller).unwrap());
    authority_protocol::decode(&mut callee).unwrap();
    drop(callee);
    assert!(callee_worker.join().unwrap().is_ok());
    assert_eq!(
        authority_protocol::decode(&mut caller).unwrap(),
        Message::TargetDead {
            call,
            target_thread: 3,
        }
    );
    authority_protocol::encode(&mut caller, Message::CloseConnection).unwrap();
    assert!(caller_worker.join().unwrap().is_ok());
}
