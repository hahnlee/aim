//! `binder_thread_read`: turn queued work into BR returns.

use crate::host::GuestProcess;
use crate::state::{
    DeathLocation, DeathState, ErrorSlot, LOOPER_ENTERED, LOOPER_REGISTERED, ProcId, State, Tid,
    Work,
};
use crate::uapi::*;

/// Whether a read pass produced anything worth returning.
pub(crate) enum Pass {
    /// Nothing but the leading BR_NOOP: wait again (`goto retry`).
    Retry,
    Done,
}

impl State {
    /// `binder_available_for_proc_work_ilocked`.
    pub(crate) fn available_for_proc_work(&self, proc: ProcId, tid: Tid) -> bool {
        self.procs
            .get(&proc)
            .and_then(|p| p.threads.get(&tid))
            .is_some_and(|t| t.transaction_stack.is_none() && t.todo.is_empty())
    }

    /// `binder_has_work_ilocked`.
    pub(crate) fn has_work(&self, proc: ProcId, tid: Tid, do_proc_work: bool) -> bool {
        let Some(p) = self.procs.get(&proc) else {
            return true;
        };
        let Some(t) = p.threads.get(&tid) else {
            return true;
        };
        t.process_todo || t.looper_need_return || (do_proc_work && !p.todo.is_empty())
    }

    /// One pass of the read loop over the thread's and (if allowed) the
    /// process's work. `out` already holds what earlier passes wrote;
    /// `start` is the read_consumed the caller came in with and `capacity`
    /// the read_size.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn read_pass(
        &mut self,
        proc: ProcId,
        tid: Tid,
        wait_for_proc_work: bool,
        out: &mut Vec<u8>,
        start: usize,
        capacity: usize,
        guest: &mut dyn GuestProcess,
    ) -> Pass {
        loop {
            let Some(p) = self.procs.get_mut(&proc) else {
                return Pass::Done;
            };
            let Some(thread) = p.threads.get_mut(&tid) else {
                return Pass::Done;
            };
            let from_thread = !thread.todo.is_empty();
            if !from_thread && (p.todo.is_empty() || !wait_for_proc_work) {
                if start + out.len() == 4 && !thread.looper_need_return {
                    return Pass::Retry;
                }
                return Pass::Done;
            }
            if capacity - (start + out.len()) < TRANSACTION_DATA_SECCTX_SIZE + 4 {
                return Pass::Done;
            }
            let next = if from_thread {
                thread.todo.front()
            } else {
                p.todo.front()
            };
            // A transaction whose files the reader has no room for stays
            // at the front of its queue until the reader has.
            if let Some(Work::Transaction(id)) = next
                && let Some(t) = self.txns.get(id)
                && !t.fd_fixups.is_empty()
                && !guest.can_install(t.fd_fixups.len())
            {
                return Pass::Done;
            }
            let p = self.procs.get_mut(&proc).unwrap();
            let work = if from_thread {
                p.threads.get_mut(&tid).unwrap().todo.pop_front().unwrap()
            } else {
                p.todo.pop_front().unwrap()
            };
            let thread = p.threads.get_mut(&tid).unwrap();
            if thread.todo.is_empty() {
                thread.process_todo = false;
            }
            match work {
                Work::Transaction(id) => {
                    if self.deliver_transaction(proc, tid, id, out, guest) {
                        return Pass::Done;
                    }
                }
                Work::ReturnError { cmd, slot } => {
                    let thread = self.thread(proc, tid).unwrap();
                    match slot {
                        ErrorSlot::Return => thread.return_error = None,
                        ErrorSlot::Reply => thread.reply_error = None,
                    }
                    out.extend_from_slice(&cmd.to_le_bytes());
                }
                Work::TransactionComplete => {
                    out.extend_from_slice(&BR_TRANSACTION_COMPLETE.to_le_bytes());
                }
                Work::OnewaySpamSuspect => {
                    let cmd = if self.procs[&proc].oneway_spam_detection {
                        BR_ONEWAY_SPAM_SUSPECT
                    } else {
                        BR_TRANSACTION_COMPLETE
                    };
                    out.extend_from_slice(&cmd.to_le_bytes());
                }
                Work::Node(id) => self.node_work(proc, id, out),
                Work::Death(id) => {
                    let Some(d) = self.deaths.get_mut(&id) else {
                        continue;
                    };
                    let cookie = d.cookie;
                    let cmd = if d.state == DeathState::Clear {
                        self.deaths.remove(&id);
                        BR_CLEAR_DEATH_NOTIFICATION_DONE
                    } else {
                        d.location = Some(DeathLocation::Delivered);
                        self.procs.get_mut(&proc).unwrap().delivered_death.push(id);
                        BR_DEAD_BINDER
                    };
                    out.extend_from_slice(&cmd.to_le_bytes());
                    out.extend_from_slice(&cookie.to_le_bytes());
                    if cmd == BR_DEAD_BINDER {
                        // A death notification can cause transactions.
                        return Pass::Done;
                    }
                }
            }
        }
    }

    /// `BINDER_WORK_NODE`: report reference state changes to the owner.
    fn node_work(&mut self, proc: ProcId, id: crate::state::NodeId, out: &mut Vec<u8>) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        node.work_queued = None;
        let strong = node.internal_strong_refs != 0 || node.local_strong_refs != 0;
        let weak = !node.refs.is_empty() || node.local_weak_refs != 0 || strong;
        let (has_strong, has_weak) = (node.has_strong_ref, node.has_weak_ref);
        let (ptr, cookie) = (node.ptr, node.cookie);
        if weak && !has_weak {
            node.has_weak_ref = true;
            node.pending_weak_ref = true;
            node.local_weak_refs += 1;
        }
        if strong && !has_strong {
            node.has_strong_ref = true;
            node.pending_strong_ref = true;
            node.local_strong_refs += 1;
        }
        if !strong && has_strong {
            node.has_strong_ref = false;
        }
        if !weak && has_weak {
            node.has_weak_ref = false;
        }
        if !weak && !strong {
            if let Some(p) = self.procs.get_mut(&proc) {
                p.nodes.remove(&ptr);
            }
            self.nodes.remove(&id);
        }
        let mut put = |cmd: u32| {
            out.extend_from_slice(&cmd.to_le_bytes());
            out.extend_from_slice(&ptr.to_le_bytes());
            out.extend_from_slice(&cookie.to_le_bytes());
        };
        if weak && !has_weak {
            put(BR_INCREFS);
        }
        if strong && !has_strong {
            put(BR_ACQUIRE);
        }
        if !strong && has_strong {
            put(BR_RELEASE);
        }
        if !weak && has_weak {
            put(BR_DECREFS);
        }
    }

    /// Deliver one transaction or reply. Returns whether the read loop ends
    /// (it does after any delivered transaction, and after a failed reply).
    fn deliver_transaction(
        &mut self,
        proc: ProcId,
        tid: Tid,
        id: crate::state::TxnId,
        out: &mut Vec<u8>,
        guest: &mut dyn GuestProcess,
    ) -> bool {
        let Some(t) = self.txns.get(&id) else {
            return false;
        };
        let Some(offset) = t.buffer else {
            self.cleanup_transaction(id, BR_DEAD_REPLY);
            return false;
        };
        let mut tr = TransactionData::default();
        let mut cmd = match t.target_node.and_then(|n| self.nodes.get(&n)) {
            Some(node) => {
                tr.target = node.ptr;
                tr.cookie = node.cookie;
                BR_TRANSACTION
            }
            None => BR_REPLY,
        };
        tr.code = t.code;
        tr.flags = t.flags;
        tr.sender_euid = t.sender_euid;
        tr.sender_pid = t
            .from
            .and_then(|(p, _)| self.procs.get(&p))
            .map_or(0, |p| p.creds.pid);
        let oneway = t.is_oneway();
        let secctx = t.security_ctx;

        // binder_apply_fd_fixups: install the files in the reader.
        let fixups = std::mem::take(&mut self.txns.get_mut(&id).unwrap().fd_fixups);
        let memory = self.procs[&proc].receive.clone().unwrap();
        let mut installed: Vec<(u32, bool)> = Vec::new();
        let mut failed = false;
        for fixup in fixups {
            match guest.install_file(fixup.file) {
                Ok(fd) => {
                    memory.write(offset + fixup.offset, &fd.to_le_bytes());
                    installed.push((fd, fixup.in_array));
                }
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            for (fd, _) in installed {
                guest.close_fd(fd);
            }
            if let Some(b) = self.buffer_mut(proc, offset) {
                b.txn = None;
            }
            self.cleanup_transaction(id, BR_FAILED_REPLY);
            self.free_buffer(proc, offset, true, None);
            if cmd == BR_REPLY {
                out.extend_from_slice(&BR_FAILED_REPLY.to_le_bytes());
                return true;
            }
            return false;
        }

        let alloc = self.procs[&proc].alloc.as_ref().unwrap();
        let buffer = &alloc.buffers[&offset];
        tr.data_size = buffer.data_size as u64;
        tr.offsets_size = buffer.offsets_size as u64;
        tr.buffer = alloc.user_address(offset);
        tr.offsets = tr.buffer + crate::alloc::align8(tr.data_size).unwrap();
        if secctx != 0 {
            cmd = BR_TRANSACTION_SEC_CTX;
        }
        out.extend_from_slice(&cmd.to_le_bytes());
        out.extend_from_slice(&tr.encode());
        if cmd == BR_TRANSACTION_SEC_CTX {
            out.extend_from_slice(&secctx.to_le_bytes());
        }
        let buffer = self.buffer_mut(proc, offset).unwrap();
        buffer.allow_user_free = true;
        buffer.fda_fds = installed
            .into_iter()
            .filter(|(_, in_array)| *in_array)
            .map(|(fd, _)| fd)
            .collect();

        if cmd != BR_REPLY && !oneway {
            let stack = self.thread(proc, tid).unwrap().transaction_stack;
            let t = self.txns.get_mut(&id).unwrap();
            t.to_parent = stack;
            t.to_thread = Some(tid);
            self.thread(proc, tid).unwrap().transaction_stack = Some(id);
        } else {
            self.free_transaction(id);
        }
        true
    }

    /// The end of `binder_thread_read`: ask for another looper when none is
    /// waiting and the pool may grow.
    pub(crate) fn should_spawn_looper(&mut self, proc: ProcId, tid: Tid) -> bool {
        let Some(p) = self.procs.get_mut(&proc) else {
            return false;
        };
        let registered = p
            .threads
            .get(&tid)
            .is_some_and(|t| t.looper & (LOOPER_REGISTERED | LOOPER_ENTERED) != 0);
        if p.requested_threads == 0
            && p.waiting_threads.is_empty()
            && p.requested_threads_started < p.max_threads
            && registered
        {
            p.requested_threads += 1;
            return true;
        }
        false
    }
}
