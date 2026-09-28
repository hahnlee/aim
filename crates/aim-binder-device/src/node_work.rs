//! Owner notification/acknowledgement state, separate from demand counters.
//! Demand must come from node-owned references, locals and temporary holds.
use crate::reference_table::Strength;

#[derive(Clone, Copy, Debug, Default)]
pub struct Demand {
    pub internal_strong: bool,
    /// Local holds excluding this state's pending-ack hold.
    pub local_strong: bool,
    pub has_remote_references: bool,
    /// Local holds excluding this state's pending-ack hold.
    pub local_weak: bool,
    pub temporary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notification {
    Increfs,
    Acquire,
    Release,
    Decrefs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeWork {
    has_weak: bool,
    has_strong: bool,
    pending_weak: bool,
    pending_strong: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NoPendingAcknowledgement;

impl NodeWork {
    /// binder_ioctl_set_ctx_mgr marks both owner refs present. Existing pending
    /// acknowledgements, if any, are not cleared by this operation.
    pub(crate) fn mark_context_manager_refs(&mut self) {
        self.has_weak = true;
        self.has_strong = true;
    }
    /// Prepare while borrowing state exclusively. Dropping the batch before read
    /// publication leaves state unchanged; the driver must publish then commit
    /// under its session/read serialization. This does not encode BR wire bytes.
    pub fn prepare(&mut self, demand: Demand) -> Batch<'_> {
        let strong = demand.internal_strong || demand.local_strong || self.pending_strong;
        let weak = demand.has_remote_references
            || demand.local_weak
            || demand.temporary
            || self.pending_weak
            || strong;
        let mut next = *self;
        let mut notifications = [None; 4];
        if weak && !self.has_weak {
            next.has_weak = true;
            next.pending_weak = true;
            notifications[0] = Some(Notification::Increfs);
        }
        if strong && !self.has_strong {
            next.has_strong = true;
            next.pending_strong = true;
            notifications[1] = Some(Notification::Acquire);
        }
        if !strong && self.has_strong {
            next.has_strong = false;
            notifications[2] = Some(Notification::Release);
        }
        if !weak && self.has_weak {
            next.has_weak = false;
            notifications[3] = Some(Notification::Decrefs);
        }
        Batch {
            state: self,
            next,
            notifications,
        }
    }

    /// Caller must first authenticate the owning session and match pointer/cookie.
    pub fn acknowledge(&mut self, strength: Strength) -> Result<(), NoPendingAcknowledgement> {
        let pending = match strength {
            Strength::Strong => &mut self.pending_strong,
            Strength::Weak => &mut self.pending_weak,
        };
        if !*pending {
            return Err(NoPendingAcknowledgement);
        }
        *pending = false;
        Ok(())
    }
}

pub struct Batch<'a> {
    state: &'a mut NodeWork,
    next: NodeWork,
    notifications: [Option<Notification>; 4],
}
impl Batch<'_> {
    pub fn notifications(&self) -> impl Iterator<Item = Notification> + '_ {
        self.notifications.iter().flatten().copied()
    }
    pub fn commit(self) {
        *self.state = self.next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn publish(state: &mut NodeWork, demand: Demand) -> Vec<Notification> {
        let batch = state.prepare(demand);
        let commands = batch.notifications().collect();
        batch.commit();
        commands
    }
    #[test]
    fn pending_ack_holds_prevent_premature_release() {
        let mut state = NodeWork::default();
        let strong = Demand {
            internal_strong: true,
            ..Demand::default()
        };
        assert_eq!(
            publish(&mut state, strong),
            [Notification::Increfs, Notification::Acquire]
        );
        assert!(publish(&mut state, Demand::default()).is_empty());
        state.acknowledge(Strength::Strong).unwrap();
        assert_eq!(
            publish(&mut state, Demand::default()),
            [Notification::Release]
        );
        assert!(state.acknowledge(Strength::Strong).is_err());
        state.acknowledge(Strength::Weak).unwrap();
        assert_eq!(
            publish(&mut state, Demand::default()),
            [Notification::Decrefs]
        );
        assert_eq!(state, NodeWork::default());
    }
    #[test]
    fn abandoned_read_does_not_consume_work_and_final_release_order_is_stable() {
        let mut state = NodeWork::default();
        let demand = Demand {
            local_strong: true,
            ..Demand::default()
        };
        {
            let batch = state.prepare(demand);
            assert_eq!(batch.notifications().count(), 2);
        }
        assert_eq!(state, NodeWork::default());
        assert!(state.acknowledge(Strength::Weak).is_err());
        assert_eq!(
            publish(&mut state, demand),
            [Notification::Increfs, Notification::Acquire]
        );
        state.acknowledge(Strength::Weak).unwrap();
        state.acknowledge(Strength::Strong).unwrap();
        assert_eq!(
            publish(&mut state, Demand::default()),
            [Notification::Release, Notification::Decrefs]
        );
        let temporary = Demand {
            temporary: true,
            ..Demand::default()
        };
        assert_eq!(publish(&mut state, temporary), [Notification::Increfs]);
        state.acknowledge(Strength::Weak).unwrap();
        assert!(publish(&mut state, temporary).is_empty());
        assert_eq!(
            publish(&mut state, Demand::default()),
            [Notification::Decrefs]
        );
    }
}
