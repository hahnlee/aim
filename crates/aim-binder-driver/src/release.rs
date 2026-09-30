//! Thread exit and process release (`binder_thread_release`,
//! `binder_deferred_release`): failed replies to callers, dead nodes and
//! their death notifications, and dropped references.

use std::collections::VecDeque;

use crate::state::{DeathLocation, DeathState, NodeId, ProcId, State, Tid, Work, WorkList};
use crate::uapi::BR_DEAD_REPLY;

impl State {
    /// `binder_thread_release`: returns the number of transactions that
    /// were still active on the thread.
    pub(crate) fn thread_release(&mut self, proc: ProcId, tid: Tid) -> usize {
        let Some(p) = self.procs.get_mut(&proc) else {
            return 0;
        };
        let Some(thread) = p.threads.remove(&tid) else {
            return 0;
        };
        p.waiting_threads.retain(|t| *t != tid);
        thread.wait.notify_all();
        self.resumes.extend(thread.parked);

        let mut active = 0;
        let mut send_reply = None;
        let mut cursor = thread.transaction_stack;
        if let Some(top) = cursor
            && self.txns[&top].to_thread == Some(tid)
            && self.txns[&top].to_proc == Some(proc)
        {
            send_reply = Some(top);
        }
        while let Some(id) = cursor {
            active += 1;
            let Some(t) = self.txns.get_mut(&id) else {
                break;
            };
            if t.to_thread == Some(tid) && t.to_proc == Some(proc) {
                t.to_proc = None;
                t.to_thread = None;
                let buffer = t.buffer.take();
                cursor = t.to_parent;
                if let Some(p) = self.procs.get_mut(&proc) {
                    p.outstanding_txns = p.outstanding_txns.saturating_sub(1);
                }
                if let Some(offset) = buffer
                    && let Some(b) = self.buffer_mut(proc, offset)
                {
                    b.txn = None;
                }
            } else if t.from == Some((proc, tid)) {
                t.from = None;
                cursor = t.from_parent;
            } else {
                break;
            }
        }
        if let Some(id) = send_reply {
            self.send_failed_reply(id, BR_DEAD_REPLY);
        }
        self.release_work(thread.todo);
        active
    }

    /// `binder_release_work`.
    pub(crate) fn release_work(&mut self, work: VecDeque<Work>) {
        for w in work {
            match w {
                Work::Transaction(id) => self.cleanup_transaction(id, BR_DEAD_REPLY),
                Work::Death(id) => {
                    let state = self.deaths.get(&id).map(|d| d.state);
                    if let Some(d) = self.deaths.get_mut(&id) {
                        d.location = None;
                    }
                    if matches!(
                        state,
                        Some(DeathState::DeadBinderAndClear | DeathState::Clear)
                    ) {
                        self.deaths.remove(&id);
                    }
                }
                Work::Node(id) => {
                    if let Some(n) = self.nodes.get_mut(&id) {
                        n.work_queued = None;
                    }
                }
                Work::TransactionComplete | Work::OnewaySpamSuspect | Work::ReturnError { .. } => {}
            }
        }
    }

    /// `binder_node_release`: the owner is gone. Holders keep the node as a
    /// dead node and get their death notifications.
    fn node_release(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        let async_todo: VecDeque<Work> = node.async_todo.drain(..).map(Work::Transaction).collect();
        node.work_queued = None;
        self.release_work(async_todo);
        let node = self.nodes.get_mut(&id).unwrap();
        if node.refs.is_empty() {
            self.nodes.remove(&id);
            return;
        }
        node.proc = None;
        node.local_strong_refs = 0;
        node.local_weak_refs = 0;
        let holders: Vec<ProcId> = node.refs.iter().copied().collect();
        for holder in holders {
            let Some(p) = self.procs.get(&holder) else {
                continue;
            };
            let Some(desc) = p.refs_by_node.get(&id) else {
                continue;
            };
            let Some(death) = p.refs_by_desc[desc].death else {
                continue;
            };
            let d = self.deaths.get_mut(&death).unwrap();
            d.state = DeathState::DeadBinder;
            d.location = Some(DeathLocation::Queued(WorkList::Proc(holder)));
            self.enqueue_proc_work(holder, Work::Death(death));
            self.wakeup_proc(holder);
        }
    }

    /// `binder_deferred_release`: the process closed its binder fd or died.
    pub(crate) fn release_proc(&mut self, proc: ProcId) {
        let Some(p) = self.procs.get(&proc) else {
            return;
        };
        let context = p.context;
        if let Some(mgr) = self.contexts[context].mgr_node
            && self.nodes.get(&mgr).is_some_and(|n| n.proc == Some(proc))
        {
            self.contexts[context].mgr_node = None;
        }
        let tids: Vec<Tid> = p.threads.keys().copied().collect();
        for tid in tids {
            self.thread_release(proc, tid);
        }
        let nodes: Vec<NodeId> = self.procs[&proc].nodes.values().copied().collect();
        self.procs.get_mut(&proc).unwrap().nodes.clear();
        for node in nodes {
            self.node_release(node);
        }
        let descs: Vec<u32> = self.procs[&proc].refs_by_desc.keys().copied().collect();
        for desc in descs {
            self.cleanup_ref(proc, desc);
        }
        let p = self.procs.get_mut(&proc).unwrap();
        let todo = std::mem::take(&mut p.todo);
        let delivered: VecDeque<Work> = p.delivered_death.drain(..).map(Work::Death).collect();
        self.release_work(todo);
        self.release_work(delivered);
        // Transactions still pointing at buffers of this process lose them.
        let p = self.procs.remove(&proc).unwrap();
        if let Some(alloc) = p.alloc {
            for buffer in alloc.buffers.values() {
                if let Some(txn) = buffer.txn
                    && let Some(t) = self.txns.get_mut(&txn)
                {
                    t.buffer = None;
                }
            }
        }
    }
}
