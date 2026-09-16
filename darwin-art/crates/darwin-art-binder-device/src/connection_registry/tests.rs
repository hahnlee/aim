use super::*;
use crate::{command::Kind, session::Session};

fn open(registry: &Registry) -> Key {
    registry
        .register(ConnectionOwner::new(Session::default()))
        .unwrap()
}

#[test]
fn registration_binds_every_local_node_to_its_typed_process_owner() {
    use crate::objects;

    let registry = Registry::default();
    let mut session = Session::default();
    let mut object = [0; 24];
    object[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
    object[8..16].copy_from_slice(&0x1111u64.to_le_bytes());
    object[16..24].copy_from_slice(&0x2222u64.to_le_bytes());
    let validated = objects::validate(&object, &0u64.to_le_bytes()).unwrap();
    let node = session.resolve_local(&validated[0]).unwrap();
    object[8..16].copy_from_slice(&0x3333u64.to_le_bytes());
    object[16..24].copy_from_slice(&0x4444u64.to_le_bytes());
    let validated = objects::validate(&object, &0u64.to_le_bytes()).unwrap();
    let other_node = session.resolve_local(&validated[0]).unwrap();
    assert_eq!(node.owner_connection(), None);
    assert_eq!(node.routing_id(), None);
    let key = registry.register(ConnectionOwner::new(session)).unwrap();
    assert_eq!(node.owner_connection(), Some(key.id));
    let node_id = node.routing_id().unwrap();
    let other_node_id = other_node.routing_id().unwrap();
    assert_eq!(node_id.owner(), key.id);
    assert_eq!(other_node_id.owner(), key.id);
    assert_ne!(node_id, other_node_id);
    registry.close(&key).unwrap();

    let prebound = ConnectionOwner::new(Session::default());
    prebound
        .bind_routing_id(crate::routing_id::ConnectionId::next(41).unwrap().0)
        .unwrap();
    assert!(matches!(
        registry.register(prebound),
        Err(Error::Connection(connection::Error::AlreadyRegistered))
    ));
}

#[test]
fn foreign_and_stale_keys_cannot_rebind_same_integer() {
    let first = Registry::default();
    let second = Registry::default();
    let old = open(&first);
    let other = open(&second);
    assert_eq!(old.id, other.id);
    assert!(matches!(second.lookup(&old), Err(Error::ForeignRegistry)));
    let retained = first.lookup(&old).unwrap();
    first.close(&old).unwrap();
    let new = open(&first);
    assert_ne!(old.id, new.id);
    assert!(matches!(first.lookup(&old), Err(Error::UnknownConnection)));
    assert_eq!(
        retained.execute(&Kind::EnterLooper.word().to_le_bytes()),
        Err(connection::Error::Closed)
    );
    assert_eq!(
        first.execute(&new, &Kind::EnterLooper.word().to_le_bytes()),
        Err(Error::Connection(connection::Error::Command(
            session::Error::Unsupported(Kind::EnterLooper)
        )))
    );
    assert_eq!(first.close(&old), Err(Error::UnknownConnection));
}
#[test]
fn registry_drop_closes_retained_handles_and_exhaustion_never_wraps() {
    let registry = Registry::default();
    let key = open(&registry);
    let retained = registry.lookup(&key).unwrap();
    registry.entries.lock().unwrap().next = u64::MAX;
    assert!(matches!(
        registry.register(ConnectionOwner::new(Session::default())),
        Err(Error::Exhausted)
    ));
    assert_eq!(registry.entries.lock().unwrap().owners.len(), 1);
    drop(registry);
    assert_eq!(
        retained.execute(&Kind::EnterLooper.word().to_le_bytes()),
        Err(connection::Error::Closed)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn ordinary_transaction_routes_to_node_owner_and_retains_until_free() {
    use crate::{
        objects, reference_table::Strength, transaction_request,
        transaction_snapshot::TransactionSnapshot, transaction_wire,
    };

    let _guard = crate::mapping::MAPPING_TEST_LOCK.lock().unwrap();
    let registry = Registry::default();
    let mut owner_session = Session::default();
    let mut object = [0; 24];
    object[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
    object[8..16].copy_from_slice(&0x1111u64.to_le_bytes());
    object[16..24].copy_from_slice(&0x2222u64.to_le_bytes());
    let validated = objects::validate(&object, &0u64.to_le_bytes()).unwrap();
    let node = owner_session.resolve_local(&validated[0]).unwrap();
    let mut sender_session = Session::default();
    sender_session
        .retain_transferred(node, Strength::Strong)
        .unwrap();
    let owner = registry
        .register(ConnectionOwner::new(owner_session))
        .unwrap();
    let sender = registry
        .register(ConnectionOwner::new(sender_session))
        .unwrap();
    registry.install_receive_mapping(&owner, 4096).unwrap();

    let mut command = Kind::Transaction.word().to_le_bytes().to_vec();
    command.resize(4 + Kind::Transaction.payload_size(), 0);
    command[4..8].copy_from_slice(&1u32.to_le_bytes());
    command[20..24].copy_from_slice(&77u32.to_le_bytes());
    command[24..28].copy_from_slice(&8u32.to_le_bytes());
    let request = transaction_request::decode(&command).unwrap().0;
    let snapshot = TransactionSnapshot::capture(b"parcel", &[], 0).unwrap();
    let mut sender_thread = crate::thread::Thread::new(1).unwrap();
    let reservation = sender_thread.reserve_submission(true).unwrap();
    let address = registry
        .submit_transaction(&sender, &request, &snapshot, 123, 10_123, reservation)
        .unwrap();

    let mut output = [0xcc; transaction_wire::RECORD_SIZE];
    let mut target_thread = crate::thread::Thread::new(2).unwrap();
    assert_eq!(
        registry
            .deliver_transaction(&owner, &mut target_thread, &mut output)
            .unwrap(),
        output.len()
    );
    assert_eq!(
        u32::from_le_bytes(output[..4].try_into().unwrap()),
        transaction_wire::BR_TRANSACTION
    );
    assert_eq!(
        u64::from_le_bytes(output[4..12].try_into().unwrap()),
        0x1111
    );
    assert_eq!(
        u64::from_le_bytes(output[12..20].try_into().unwrap()),
        0x2222
    );
    assert_eq!(u32::from_le_bytes(output[20..24].try_into().unwrap()), 77);
    assert_eq!(i32::from_le_bytes(output[28..32].try_into().unwrap()), 123);
    assert_eq!(
        u32::from_le_bytes(output[32..36].try_into().unwrap()),
        10_123
    );
    assert_eq!(
        u64::from_le_bytes(output[52..60].try_into().unwrap()),
        address as u64
    );
    registry.free_transaction_buffer(&owner, address).unwrap();
    assert!(registry.free_transaction_buffer(&owner, address).is_err());
    assert_eq!(registry.calls.lock().unwrap().len(), 1);
    registry.close(&sender).unwrap();
    assert_eq!(registry.calls.lock().unwrap().len(), 1);
    registry.close(&owner).unwrap();
    assert!(registry.calls.lock().unwrap().is_empty());
}
