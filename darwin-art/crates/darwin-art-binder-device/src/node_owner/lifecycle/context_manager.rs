//! Special context-manager local node refs; context registration policy is separate.
use super::*;

pub struct ContextManagerRefs {
    strong: LocalHold,
    _weak: LocalHold,
}
impl ContextManagerRefs {
    pub fn node(&self) -> &Arc<Node> {
        self.strong.node()
    }
}
impl NodeOwner {
    /// Context owner must enforce registration authorization, unique manager and
    /// sticky UID before calling this. This does not register handle0 or bypass
    /// that policy; it establishes the special kernel node reference state.
    pub fn acquire_context_manager_refs(&self, node: Arc<Node>) -> io::Result<ContextManagerRefs> {
        if !self.owns(&node) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let mut state = node.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        let checked = |value: u32| {
            value
                .checked_add(1)
                .filter(|v| *v <= i32::MAX as u32)
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))
        };
        let strong = checked(state.local_strong)?;
        let weak = checked(state.local_weak)?;
        state.local_strong = strong;
        state.local_weak = weak;
        state.work.mark_context_manager_refs();
        drop(state);
        Ok(ContextManagerRefs {
            strong: LocalHold {
                node: Arc::clone(&node),
                kind: HoldKind::Strong,
            },
            _weak: LocalHold {
                node,
                kind: HoldKind::Weak,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{objects, reference_table::ReferenceTable};
    fn node(owner: &mut NodeOwner) -> Arc<Node> {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
        let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
        owner.resolve(&objects[0]).unwrap()
    }
    #[test]
    fn initial_owner_refs_need_no_br_acquire_or_ack_and_pin_demand() {
        let mut owner = NodeOwner::new();
        let node = node(&mut owner);
        let held = owner
            .acquire_context_manager_refs(Arc::clone(&node))
            .unwrap();
        let mut refs = ReferenceTable::default();
        refs.retain(node, Strength::Strong).unwrap();
        assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 0);
        assert_eq!(
            owner.acknowledge(0, 0, Strength::Strong),
            Err(Error::NoPendingAcknowledgement)
        );
        assert_eq!(
            owner.acknowledge(0, 0, Strength::Weak),
            Err(Error::NoPendingAcknowledgement)
        );
        drop(refs);
        assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 0);
        drop(held);
        assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 40);
    }
    #[test]
    fn foreign_owner_rejected_and_existing_pending_ack_preserved() {
        let mut owner = NodeOwner::new();
        let node = node(&mut owner);
        let foreign = NodeOwner::new();
        assert_eq!(
            foreign
                .acquire_context_manager_refs(Arc::clone(&node))
                .err()
                .unwrap()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        let mut refs = ReferenceTable::default();
        refs.retain(Arc::clone(&node), Strength::Strong).unwrap();
        assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 40);
        let held = owner.acquire_context_manager_refs(node).unwrap();
        owner.acknowledge(0, 0, Strength::Strong).unwrap();
        owner.acknowledge(0, 0, Strength::Weak).unwrap();
        drop((refs, held));
        assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 40);
    }
}
