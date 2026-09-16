use super::*;
use crate::{
    objects::{self, Kind},
    reference_table::ReferenceTable,
};

fn node(owner: &mut NodeOwner) -> Arc<Node> {
    let mut bytes = [0; 24];
    bytes[..4].copy_from_slice(&Kind::Binder.tag().to_le_bytes());
    bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
    bytes[16..24].copy_from_slice(&456u64.to_le_bytes());
    let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
    owner.resolve(&objects[0]).unwrap()
}
fn publish(owner: &NodeOwner) -> Vec<Notification> {
    let mut result = Vec::new();
    owner
        .publish_notifications(123, |cookie, commands| {
            assert_eq!(cookie, 456);
            result.extend_from_slice(commands);
            Ok(())
        })
        .unwrap();
    result
}

#[test]
fn handle_lookup_owns_temporary_demand_after_table_teardown() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner);
    let mut refs = ReferenceTable::default();
    refs.retain(Arc::clone(&node), Strength::Strong).unwrap();
    assert_eq!(
        publish(&owner),
        [Notification::Increfs, Notification::Acquire]
    );
    owner.acknowledge(123, 456, Strength::Weak).unwrap();
    owner.acknowledge(123, 456, Strength::Strong).unwrap();
    let lookup = refs.lookup(1, Strength::Strong).unwrap();
    drop(refs);
    assert_eq!(publish(&owner), [Notification::Release]);
    assert_eq!(lookup.node().pointer(), 123);
    drop(lookup);
    assert_eq!(publish(&owner), [Notification::Decrefs]);
}

#[test]
fn scoped_strong_holds_survive_receiver_teardown_until_last_drop() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner);
    let mut refs = ReferenceTable::default();
    refs.retain(Arc::clone(&node), Strength::Strong).unwrap();
    let first = node.hold(HoldKind::Strong).unwrap();
    let second = node.hold(HoldKind::Strong).unwrap();
    assert!(Arc::ptr_eq(first.node(), &node));
    assert_eq!(
        publish(&owner),
        [Notification::Increfs, Notification::Acquire]
    );
    owner.acknowledge(123, 456, Strength::Weak).unwrap();
    owner.acknowledge(123, 456, Strength::Strong).unwrap();
    drop(refs);
    drop(first);
    assert!(publish(&owner).is_empty());
    drop(second);
    assert_eq!(
        publish(&owner),
        [Notification::Release, Notification::Decrefs]
    );
    // Metadata Arc alone does not preserve Android reference demand.
    assert!(node.owner_alive());
}

#[test]
fn weak_and_temporary_holds_preserve_ack_and_error_unwind_cleanup() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner);
    for kind in [HoldKind::Weak, HoldKind::Temporary] {
        let result: io::Result<()> = (|| {
            let _hold = node.hold(kind)?;
            assert_eq!(publish(&owner), [Notification::Increfs]);
            Err(io::Error::other("transaction preparation failed"))
        })();
        assert!(result.is_err());
        assert!(publish(&owner).is_empty());
        owner.acknowledge(123, 456, Strength::Weak).unwrap();
        assert_eq!(publish(&owner), [Notification::Decrefs]);
    }
}

#[test]
fn multiple_receiver_tables_drive_one_node_and_teardown_releases_demand() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner);
    let mut first = ReferenceTable::default();
    let mut second = ReferenceTable::default();
    first.retain(Arc::clone(&node), Strength::Strong).unwrap();
    first.increment(1, Strength::Strong).unwrap();
    second.retain(node, Strength::Strong).unwrap();
    assert_eq!(
        publish(&owner),
        [Notification::Increfs, Notification::Acquire]
    );
    owner.acknowledge(123, 456, Strength::Weak).unwrap();
    owner.acknowledge(123, 456, Strength::Strong).unwrap();
    first.decrement(1, Strength::Strong).unwrap();
    assert!(publish(&owner).is_empty());
    drop(first);
    assert!(publish(&owner).is_empty());
    drop(second);
    assert_eq!(
        publish(&owner),
        [Notification::Release, Notification::Decrefs]
    );
}

#[test]
fn failed_publication_and_wrong_owner_cookie_leave_pending_state_intact() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner);
    let mut refs = ReferenceTable::default();
    refs.retain(node, Strength::Weak).unwrap();
    assert!(
        owner
            .publish_notifications(123, |_, _| Err(io::Error::other("queue full")))
            .is_err()
    );
    assert_eq!(
        owner.acknowledge(123, 456, Strength::Weak),
        Err(Error::NoPendingAcknowledgement)
    );
    assert_eq!(publish(&owner), [Notification::Increfs]);
    assert_eq!(
        owner.acknowledge(123, 999, Strength::Weak),
        Err(Error::CookieMismatch)
    );
    let foreign = NodeOwner::new();
    assert_eq!(
        foreign.acknowledge(123, 456, Strength::Weak),
        Err(Error::UnknownNode)
    );
    drop(refs);
    assert!(publish(&owner).is_empty());
    owner.acknowledge(123, 456, Strength::Weak).unwrap();
    assert_eq!(publish(&owner), [Notification::Decrefs]);
}
