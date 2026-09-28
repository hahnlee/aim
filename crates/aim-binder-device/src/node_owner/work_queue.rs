//! Coalesced owner-node work. Queue lock is never held while taking a node lock.
use super::*;
use std::{
    collections::{HashSet, VecDeque},
    io,
    sync::Condvar,
    time::Duration,
};

#[derive(Default)]
struct Pending {
    order: VecDeque<u64>,
    membership: HashSet<u64>,
}
#[derive(Default)]
pub(super) struct WorkQueue {
    pending: Mutex<Pending>,
    reading: Mutex<()>,
    changed: Condvar,
}
impl OwnerState {
    pub(super) fn schedule(&self, pointer: u64) {
        let mut pending = self.work.pending.lock().unwrap_or_else(|p| p.into_inner());
        if self.alive.load(Ordering::Acquire) && pending.membership.insert(pointer) {
            pending.order.push_back(pointer);
            self.work.changed.notify_all();
            if let Some(signal) = self
                .signal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
            {
                signal.notify();
            }
        }
    }
}
impl NodeOwner {
    /// Nonblocking owner work read. One node batch per call; stale coalesced
    /// entries present at entry are drained at most once. Zero means no batch
    /// produced this pass; concurrent new work can remain queued for the next pass.
    /// Thread eligibility and all other Binder work remain separate.
    pub fn read_pending_node_work(&self, output: &mut [u8]) -> io::Result<usize> {
        let queue = &self.state.work;
        let _reader = queue.reading.lock().unwrap_or_else(|p| p.into_inner());
        let budget = queue
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .order
            .len();
        for _ in 0..budget {
            let pointer = {
                let mut pending = queue.pending.lock().unwrap_or_else(|p| p.into_inner());
                let Some(pointer) = pending.order.pop_front() else {
                    return Ok(0);
                };
                pending.membership.remove(&pointer);
                pointer
            };
            match self.read_node_notifications(pointer, output) {
                Ok(0) => continue,
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    let mut pending = queue.pending.lock().unwrap_or_else(|p| p.into_inner());
                    if pending.membership.insert(pointer) {
                        pending.order.push_front(pointer);
                        queue.changed.notify_all();
                    }
                    return Err(error);
                }
            }
        }
        Ok(0)
    }

    /// Level-triggered, bounded condition-variable wait on the same mutex used
    /// by enqueue. True means queued work, not guaranteed nonempty BR output.
    /// Device poll descriptors, interruption and process shutdown are not wired.
    pub fn wait_for_node_work(&self, timeout: Duration) -> bool {
        let queue = &self.state.work;
        let pending = queue.pending.lock().unwrap_or_else(|p| p.into_inner());
        let (pending, _) = queue
            .changed
            .wait_timeout_while(pending, timeout, |pending| pending.order.is_empty())
            .unwrap_or_else(|p| p.into_inner());
        !pending.order.is_empty()
    }
}

#[cfg(test)]
mod tests;
