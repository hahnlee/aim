use super::*;
use crate::{objects, session::Session};
fn key(registry: &Registry) -> Key {
    registry
        .register(ConnectionOwner::new(Session::default()))
        .unwrap()
}
fn bytes(kind: objects::Kind) -> [u8; 24] {
    let mut bytes = [0; 24];
    bytes[..4].copy_from_slice(&kind.tag().to_le_bytes());
    bytes
}
#[test]
fn busy_precedes_security_and_disconnect_preserves_uid() {
    let registry = Registry::default();
    let first = key(&registry);
    let second = key(&registry);
    let bytes = bytes(objects::Kind::Binder);
    let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
    registry
        .register_context_manager(&first, &objects[0], |_| Ok(1000))
        .unwrap();
    assert_eq!(
        registry.register_context_manager(&second, &objects[0], |_| panic!(
            "busy must precede security"
        )),
        Err(Error::Busy)
    );
    let node = Arc::clone(
        registry
            .context
            .lock()
            .unwrap()
            .manager
            .as_ref()
            .unwrap()
            ._references
            .node(),
    );
    registry.close(&first).unwrap();
    assert!(!node.owner_alive());
    assert!(registry.context.lock().unwrap().manager.is_none());
    assert_eq!(
        registry.register_context_manager(&second, &objects[0], |_| Ok(1001)),
        Err(Error::WrongUid)
    );
    registry
        .register_context_manager(&second, &objects[0], |_| Ok(1000))
        .unwrap();
}
#[test]
fn security_failure_does_not_bind_uid_but_node_failure_does() {
    let registry = Registry::default();
    let owner = key(&registry);
    let invalid = bytes(objects::Kind::Handle);
    let objects = objects::validate(&invalid, &0u64.to_le_bytes()).unwrap();
    assert_eq!(
        registry.register_context_manager(&owner, &objects[0], |_| Err(libc::EPERM)),
        Err(Error::Security(libc::EPERM))
    );
    assert_eq!(registry.context.lock().unwrap().uid, None);
    assert!(matches!(
        registry.register_context_manager(&owner, &objects[0], |_| Ok(1000)),
        Err(Error::Node(_))
    ));
    assert_eq!(registry.context.lock().unwrap().uid, Some(1000));
    assert!(registry.context.lock().unwrap().manager.is_none());
    assert_eq!(
        registry.register_context_manager(&owner, &objects[0], |_| Ok(1001)),
        Err(Error::WrongUid)
    );
}

#[test]
fn reservation_is_invisible_until_commit_and_abort_restores_slot() {
    let registry = Registry::default();
    let owner = key(&registry);
    let bytes = bytes(objects::Kind::Binder);
    let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
    let (reservation, local) = registry
        .begin_context_manager(&owner, &objects[0], |_| Ok(1000))
        .unwrap();
    assert_eq!(local.get(), 1);
    assert!(registry.context.lock().unwrap().manager.is_none());
    assert!(matches!(
        registry.begin_context_manager(&owner, &objects[0], |_| Ok(1000)),
        Err(Error::Busy)
    ));
    registry.abort_context_manager(&owner, reservation);
    let (replacement, replacement_local) = registry
        .begin_context_manager(&owner, &objects[0], |_| Ok(1000))
        .unwrap();
    assert_eq!(replacement_local, local);
    registry
        .commit_context_manager(&owner, replacement)
        .unwrap();
    assert!(registry.context.lock().unwrap().manager.is_some());
    assert_eq!(
        registry.commit_context_manager(&owner, replacement),
        Err(Error::NoPendingReservation)
    );
}
