use super::*;
use crate::{
    node_work::{Demand, NodeWork, Notification},
    reference_table::{Counts, Strength},
};
use std::io;
mod context_manager;
pub use context_manager::ContextManagerRefs;

#[derive(Default)]
pub(super) struct Lifecycle {
    remote_tables: u32,
    strong_tables: u32,
    local_strong: u32,
    local_weak: u32,
    temporary: u32,
    work: NodeWork,
}

impl Node {
    pub(crate) fn update_remote(&self, before: Counts, after: Counts) -> Result<(), ()> {
        let mut state = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        fn transition(value: u32, before: bool, after: bool) -> Result<u32, ()> {
            match (before, after) {
                (false, true) => value
                    .checked_add(1)
                    .filter(|n| *n <= i32::MAX as u32)
                    .ok_or(()),
                (true, false) => value.checked_sub(1).ok_or(()),
                _ => Ok(value),
            }
        }
        let remote = transition(
            state.remote_tables,
            before != Counts::default(),
            after != Counts::default(),
        )?;
        let strong = transition(state.strong_tables, before.strong != 0, after.strong != 0)?;
        state.remote_tables = remote;
        state.strong_tables = strong;
        self.owner.schedule(self.pointer);
        Ok(())
    }
}

impl NodeOwner {
    /// Only this owner table can acknowledge its node. Device/session dispatch
    /// must choose the table from authenticated connection identity, not a PID.
    pub fn acknowledge(&self, pointer: u64, cookie: u64, strength: Strength) -> Result<(), Error> {
        let node = self.nodes.get(&pointer).ok_or(Error::UnknownNode)?;
        if node.cookie != cookie {
            return Err(Error::CookieMismatch);
        }
        let mut state = node.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        state
            .work
            .acknowledge(strength)
            .map_err(|_| Error::NoPendingAcknowledgement)?;
        node.owner.schedule(node.pointer);
        Ok(())
    }

    /// The callback must atomically accept the whole batch or fail without
    /// publication. It runs under the node lock and must not reenter Binder.
    /// This callback boundary does not itself perform wire encoding or thread wake.
    pub fn publish_notifications(
        &self,
        pointer: u64,
        publish: impl FnOnce(u64, &[Notification]) -> io::Result<()>,
    ) -> io::Result<usize> {
        let node = self
            .nodes
            .get(&pointer)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ENOENT))?;
        // A panicking callback cannot mutate work before commit. Recovering the
        // guard is safe because counter transitions also compute before assigning.
        let mut state = node.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        let demand = Demand {
            internal_strong: state.strong_tables != 0,
            has_remote_references: state.remote_tables != 0,
            local_strong: state.local_strong != 0,
            local_weak: state.local_weak != 0,
            temporary: state.temporary != 0,
        };
        let batch = state.work.prepare(demand);
        let mut commands = [Notification::Increfs; 4];
        let mut count = 0;
        for command in batch.notifications() {
            commands[count] = command;
            count += 1;
        }
        if count != 0 {
            publish(node.cookie, &commands[..count])?;
        }
        batch.commit();
        Ok(count)
    }
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub enum HoldKind {
    Strong,
    Weak,
    Temporary,
}

/// An actual local/temporary node-demand hold, distinct from Arc metadata lifetime.
/// Drop releases exactly this hold. Pending owner ACK holds remain in NodeWork.
pub struct LocalHold {
    node: Arc<Node>,
    kind: HoldKind,
}
impl LocalHold {
    pub fn node(&self) -> &Arc<Node> {
        &self.node
    }
}
impl Lifecycle {
    fn local_count(&mut self, kind: HoldKind) -> &mut u32 {
        match kind {
            HoldKind::Strong => &mut self.local_strong,
            HoldKind::Weak => &mut self.local_weak,
            HoldKind::Temporary => &mut self.temporary,
        }
    }
}
impl Node {
    /// Callers acquire this from an already-authorized node lookup/transaction.
    /// It does not authenticate a sender or register a remote handle.
    pub fn hold(self: &Arc<Self>, kind: HoldKind) -> io::Result<LocalHold> {
        let mut state = self.lifecycle.lock().unwrap_or_else(|p| p.into_inner());
        let count = state.local_count(kind);
        *count = count
            .checked_add(1)
            .filter(|n| *n <= i32::MAX as u32)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        self.owner.schedule(self.pointer);
        Ok(LocalHold {
            node: Arc::clone(self),
            kind,
        })
    }
}
impl Drop for LocalHold {
    fn drop(&mut self) {
        let mut state = self
            .node
            .lifecycle
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let count = state.local_count(self.kind);
        *count = count
            .checked_sub(1)
            .expect("local hold owns its demand count");
        self.node.owner.schedule(self.node.pointer);
    }
}
