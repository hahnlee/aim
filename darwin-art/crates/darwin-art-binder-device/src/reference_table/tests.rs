use super::*;
use crate::{
    node_owner::NodeOwner,
    objects::{self, Kind},
};
fn node(owner: &mut NodeOwner, pointer: u64) -> Arc<Node> {
    let mut data = [0; 24];
    data[..4].copy_from_slice(&Kind::Binder.tag().to_le_bytes());
    data[8..16].copy_from_slice(&pointer.to_le_bytes());
    let objects = objects::validate(&data, &0u64.to_le_bytes()).unwrap();
    owner.resolve(&objects[0]).unwrap()
}
#[test]
fn context_zero_is_reserved_without_overwriting_old_generation() {
    let mut old_owner = NodeOwner::new();
    let old = node(&mut old_owner, 0);
    let mut new_owner = NodeOwner::new();
    let new = node(&mut new_owner, 0);
    let ordinary = node(&mut new_owner, 123);
    let mut table = ReferenceTable::default();
    assert_eq!(
        table
            .retain_context_manager(Arc::clone(&old), Strength::Weak)
            .unwrap()
            .0,
        0
    );
    assert_eq!(table.retain(ordinary, Strength::Strong).unwrap().0, 1);
    drop(old_owner);
    assert_eq!(
        table
            .retain_context_manager(Arc::clone(&new), Strength::Strong)
            .unwrap()
            .0,
        2
    );
    assert!(Arc::ptr_eq(
        table.lookup(0, Strength::Weak).unwrap().node(),
        &old
    ));
    assert!(Arc::ptr_eq(
        table.lookup(2, Strength::Strong).unwrap().node(),
        &new
    ));
    table.decrement(0, Strength::Weak).unwrap();
    // Existing node identity wins over newly free descriptor zero.
    assert_eq!(
        table
            .retain_context_manager(Arc::clone(&new), Strength::Weak)
            .unwrap()
            .0,
        2
    );
    table.decrement(2, Strength::Weak).unwrap();
    table.decrement(2, Strength::Strong).unwrap();
    assert_eq!(
        table
            .retain_context_manager(new, Strength::Strong)
            .unwrap()
            .0,
        0
    );
}

#[test]
fn handle_identity_counts_lookup_and_lowest_hole_reuse() {
    let mut owner = NodeOwner::new();
    let first = node(&mut owner, 1);
    let second = node(&mut owner, 2);
    let mut table = ReferenceTable::default();
    assert_eq!(
        table.retain(Arc::clone(&first), Strength::Weak).unwrap().0,
        1
    );
    assert!(matches!(
        table.lookup(1, Strength::Strong),
        Err(Error::StrongRequired)
    ));
    assert!(Arc::ptr_eq(
        &first,
        table.lookup(1, Strength::Weak).unwrap().node()
    ));
    assert_eq!(
        table
            .retain(Arc::clone(&first), Strength::Strong)
            .unwrap()
            .0,
        1
    );
    assert_eq!(table.counts(1).unwrap(), Counts { strong: 1, weak: 1 });
    assert_eq!(table.retain(second, Strength::Strong).unwrap().0, 2);
    table.decrement(1, Strength::Strong).unwrap();
    assert!(matches!(
        table.decrement(1, Strength::Strong),
        Err(Error::Underflow)
    ));
    assert_eq!(table.counts(1).unwrap(), Counts { strong: 0, weak: 1 });
    table.decrement(1, Strength::Weak).unwrap();
    assert!(matches!(
        table.lookup(1, Strength::Weak),
        Err(Error::UnknownHandle)
    ));
    assert_eq!(table.retain(first, Strength::Strong).unwrap().0, 1);
    assert!(matches!(
        table.lookup(0, Strength::Strong),
        Err(Error::UnknownHandle)
    ));
}
#[test]
fn separate_receiver_tables_and_checked_overflow() {
    let mut owner = NodeOwner::new();
    let a = node(&mut owner, 1);
    let b = node(&mut owner, 2);
    let mut first = ReferenceTable::default();
    let mut second = ReferenceTable::default();
    first.retain(Arc::clone(&a), Strength::Strong).unwrap();
    second.retain(Arc::clone(&b), Strength::Strong).unwrap();
    assert!(Arc::ptr_eq(
        &a,
        first.lookup(1, Strength::Strong).unwrap().node()
    ));
    assert!(Arc::ptr_eq(
        &b,
        second.lookup(1, Strength::Strong).unwrap().node()
    ));
    first.entries.get_mut(&1).unwrap().counts.strong = i32::MAX as u32;
    assert!(matches!(
        first.increment(1, Strength::Strong),
        Err(Error::Overflow)
    ));
    assert_eq!(first.counts(1).unwrap().strong, i32::MAX as u32);
}

#[test]
fn local_and_authority_nodes_share_one_descriptor_namespace() {
    let connection = crate::authority_protocol::ConnectionToken::from_nonzero(5).unwrap();
    let context = crate::authority_protocol::NodeToken::new(
        connection,
        crate::authority_protocol::LocalNodeToken::from_nonzero(7).unwrap(),
    );
    let remote = crate::authority_protocol::NodeToken::new(
        connection,
        crate::authority_protocol::LocalNodeToken::from_nonzero(8).unwrap(),
    );
    let mut owner = NodeOwner::new();
    let local = node(&mut owner, 22);
    let mut table = ReferenceTable::default();
    assert_eq!(
        table
            .retain_remote_context_manager(context, Strength::Strong)
            .unwrap()
            .0,
        0
    );
    assert_eq!(table.retain(local, Strength::Strong).unwrap().0, 1);
    assert_eq!(table.retain_remote(remote, Strength::Strong).unwrap().0, 2);
    assert!(matches!(
        table.resolve(0, Strength::Strong),
        Ok(ResolvedTarget::Remote(node)) if node == context
    ));
    assert!(matches!(
        table.resolve(2, Strength::Strong),
        Ok(ResolvedTarget::Remote(node)) if node == remote
    ));
    assert_eq!(
        table.lookup(2, Strength::Strong).err(),
        Some(Error::RemoteHandle)
    );
    table.decrement(1, Strength::Strong).unwrap();
    let replacement = crate::authority_protocol::NodeToken::new(
        connection,
        crate::authority_protocol::LocalNodeToken::from_nonzero(9).unwrap(),
    );
    assert_eq!(
        table.retain_remote(replacement, Strength::Weak).unwrap().0,
        1
    );
}
