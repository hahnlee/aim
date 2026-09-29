//! `binder_transaction()` and what it relies on: target resolution, the
//! single copy into the target's receive buffer, object translation,
//! delivery to a thread or queue, failed replies and buffer release.

use crate::alloc::align8;
use crate::host::{Errno, GuestProcess, errno};
use crate::state::{
    BufferRecord, ErrorSlot, FdFixup, LOOPER_ENTERED, LOOPER_POLL, LOOPER_REGISTERED, NodeId,
    ProcId, State, Tid, Txn, TxnId, Work,
};
use crate::trace::TraceRecord;
use crate::uapi::*;

/// How much of a traced transaction's data is read for its interface token.
const TOKEN_BYTES: usize = 512;

/// Why a transaction failed: the BR code for the sender and the errno kept
/// in the extended error.
struct Failure {
    cmd: u32,
    param: i32,
}

fn fail(cmd: u32, param: Errno) -> Failure {
    Failure { cmd, param: -param }
}

/// A `BINDER_TYPE_PTR` object already placed in the target buffer.
struct PlacedPtr {
    /// Offset of the object in the data area.
    object_offset: usize,
    /// Offset of its copied payload in the target buffer.
    target_offset: usize,
    length: usize,
    parent: Option<usize>,
    parent_offset: usize,
}

impl State {
    /// `binder_transaction`. Failures are delivered as return work on the
    /// calling thread, exactly as Linux does; nothing is returned.
    pub(crate) fn transaction(
        &mut self,
        proc: ProcId,
        tid: Tid,
        tr: &TransactionData,
        reply: bool,
        extra_buffers_size: u64,
        guest: &mut dyn GuestProcess,
    ) {
        let debug_id = self.next_id() as u32;
        if let Some(t) = self.thread(proc, tid) {
            t.ee = crate::state::ExtendedError {
                id: debug_id,
                command: BR_OK,
                param: 0,
            };
        }
        let mut in_reply_to: Option<TxnId> = None;
        let result = self.transaction_inner(
            proc,
            tid,
            tr,
            reply,
            extra_buffers_size,
            guest,
            &mut in_reply_to,
        );
        if let Err(failure) = result {
            if let Some(original) = in_reply_to {
                // The replier sees its reply as complete; the failure goes
                // to whoever waits for the original transaction.
                if let Some(t) = self.thread(proc, tid) {
                    t.return_error = Some(BR_TRANSACTION_COMPLETE);
                }
                self.enqueue_thread_work(
                    proc,
                    tid,
                    Work::ReturnError {
                        cmd: BR_TRANSACTION_COMPLETE,
                        slot: ErrorSlot::Return,
                    },
                );
                self.send_failed_reply(original, failure.cmd);
            } else {
                if let Some(t) = self.thread(proc, tid) {
                    t.ee = crate::state::ExtendedError {
                        id: debug_id,
                        command: failure.cmd,
                        param: failure.param,
                    };
                    t.return_error = Some(failure.cmd);
                }
                self.enqueue_thread_work(
                    proc,
                    tid,
                    Work::ReturnError {
                        cmd: failure.cmd,
                        slot: ErrorSlot::Return,
                    },
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transaction_inner(
        &mut self,
        proc: ProcId,
        tid: Tid,
        tr: &TransactionData,
        reply: bool,
        extra_buffers_size: u64,
        guest: &mut dyn GuestProcess,
        in_reply_to_out: &mut Option<TxnId>,
    ) -> Result<(), Failure> {
        let oneway = !reply && tr.flags & TF_ONE_WAY != 0;
        let mut target_thread: Option<Tid> = None;
        let mut target_node: Option<NodeId> = None;
        let target_proc: ProcId;

        if reply {
            let in_reply_to = self
                .thread(proc, tid)
                .and_then(|t| t.transaction_stack)
                .ok_or(fail(BR_FAILED_REPLY, errno::EPROTO))?;
            if self.txns[&in_reply_to].to_thread != Some(tid)
                || self.txns[&in_reply_to].to_proc != Some(proc)
            {
                return Err(fail(BR_FAILED_REPLY, errno::EPROTO));
            }
            let parent = self.txns[&in_reply_to].to_parent;
            self.thread(proc, tid).unwrap().transaction_stack = parent;
            *in_reply_to_out = Some(in_reply_to);
            let Some((from_proc, from_tid)) = self.txns[&in_reply_to].from else {
                return Err(fail(BR_DEAD_REPLY, 0));
            };
            if !self.thread_exists(from_proc, from_tid) {
                return Err(fail(BR_DEAD_REPLY, 0));
            }
            if self.thread(from_proc, from_tid).unwrap().transaction_stack != Some(in_reply_to) {
                *in_reply_to_out = None;
                return Err(fail(BR_FAILED_REPLY, errno::EPROTO));
            }
            target_proc = from_proc;
            target_thread = Some(from_tid);
        } else {
            let handle = tr.handle();
            let node = if handle != 0 {
                let r = self
                    .get_ref(proc, handle, true)
                    .ok_or(fail(BR_FAILED_REPLY, errno::EINVAL))?;
                r.node
            } else {
                let context = self.procs[&proc].context;
                self.contexts[context]
                    .mgr_node
                    .ok_or(fail(BR_DEAD_REPLY, errno::EINVAL))?
            };
            // binder_get_node_refs_for_txn: a local strong ref keeps the
            // node alive until the buffer is freed.
            let owner = self.nodes[&node]
                .proc
                .ok_or(fail(BR_DEAD_REPLY, errno::EINVAL))?;
            self.inc_node(node, true, false, None)
                .map_err(|e| fail(BR_FAILED_REPLY, e))?;
            target_node = Some(node);
            target_proc = owner;
            if owner == proc {
                self.dec_node(node, true, false);
                return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
            }
            let thread = self.thread(proc, tid).unwrap();
            if !oneway && matches!(thread.todo.front(), Some(Work::Transaction(_))) {
                self.dec_node(node, true, false);
                return Err(fail(BR_FAILED_REPLY, errno::EPROTO));
            }
            if !oneway && let Some(top) = thread.transaction_stack {
                if self.txns[&top].to_thread != Some(tid) {
                    self.dec_node(node, true, false);
                    return Err(fail(BR_FAILED_REPLY, errno::EPROTO));
                }
                // A nested call goes back to the thread of target_proc that
                // is waiting on this thread's call chain.
                let mut cursor = Some(top);
                while let Some(id) = cursor {
                    let t = &self.txns[&id];
                    if let Some((from_proc, from_tid)) = t.from
                        && from_proc == target_proc
                    {
                        target_thread = Some(from_tid);
                        break;
                    }
                    cursor = t.from_parent;
                }
            }
        }

        // The sender's security context, for nodes that asked for one.
        let secctx: Option<Vec<u8>> = target_node
            .filter(|n| self.nodes[n].txn_security_ctx)
            .and_then(|_| self.procs[&proc].creds.security_context.clone())
            .map(|s| {
                let mut bytes = s.into_bytes();
                bytes.push(0);
                bytes
            });
        let secctx_space = secctx
            .as_ref()
            .map(|s| align8(s.len() as u64).unwrap())
            .unwrap_or(0);
        let Some(extra_total) = extra_buffers_size.checked_add(secctx_space) else {
            self.release_target_node(target_node);
            return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
        };

        let sender_pid = self.procs[&proc].creds.pid;
        let allocation = match self
            .procs
            .get_mut(&target_proc)
            .and_then(|p| p.alloc.as_mut())
        {
            None => Err(errno::ESRCH),
            Some(alloc) => alloc.allocate(
                tr.data_size,
                tr.offsets_size,
                extra_total,
                oneway,
                sender_pid,
            ),
        };
        let allocation = match allocation {
            Ok(a) => a,
            Err(e) => {
                self.release_target_node(target_node);
                let cmd = if e == errno::ESRCH {
                    BR_DEAD_REPLY
                } else {
                    BR_FAILED_REPLY
                };
                return Err(fail(cmd, e));
            }
        };
        let buffer_offset = allocation.offset;
        let id = self.next_id();
        self.txns.insert(
            id,
            Txn {
                from: (!reply && !oneway).then_some((proc, tid)),
                from_parent: None,
                to_proc: Some(target_proc),
                to_thread: target_thread,
                to_parent: None,
                code: tr.code,
                flags: tr.flags,
                sender_euid: self.procs[&proc].creds.euid,
                target_node,
                buffer: Some(buffer_offset),
                security_ctx: 0,
                fd_fixups: Vec::new(),
            },
        );
        {
            let buffer = self.buffer_mut(target_proc, buffer_offset).unwrap();
            buffer.txn = Some(id);
            buffer.target_node = target_node;
            buffer.clear_on_free = tr.flags & TF_CLEAR_BUF != 0;
        }

        let copied = self.copy_transaction(
            proc,
            tid,
            target_proc,
            id,
            buffer_offset,
            tr,
            extra_buffers_size,
            secctx.as_deref(),
            *in_reply_to_out,
            guest,
        );
        if let Err(failure) = copied {
            self.abort_transaction(target_proc, id, buffer_offset);
            return Err(failure);
        }

        if self.trace.is_some() && !reply {
            self.trace_sent(proc, tid, target_proc, id, tr, guest);
        }

        let complete = if allocation.oneway_spam_suspect {
            Work::OnewaySpamSuspect
        } else {
            Work::TransactionComplete
        };
        if reply {
            let in_reply_to = in_reply_to_out.unwrap();
            let target_tid = target_thread.unwrap();
            self.enqueue_thread_work(proc, tid, complete);
            if !self.thread_exists(target_proc, target_tid) {
                self.remove_last_complete(proc, tid);
                self.abort_transaction(target_proc, id, buffer_offset);
                return Err(fail(BR_DEAD_REPLY, 0));
            }
            self.pop_transaction(target_proc, target_tid, in_reply_to);
            self.enqueue_thread_work(target_proc, target_tid, Work::Transaction(id));
            self.procs.get_mut(&target_proc).unwrap().outstanding_txns += 1;
            if let Some(trace) = &mut self.trace {
                trace.replied(in_reply_to, id);
            }
            self.free_transaction(in_reply_to);
        } else if !oneway {
            // Deferred: the sender returns to user space with the reply, not
            // before, so the target can start at once.
            self.enqueue_deferred_thread_work(proc, tid, complete);
            let stack = self.thread(proc, tid).unwrap().transaction_stack;
            self.txns.get_mut(&id).unwrap().from_parent = stack;
            self.thread(proc, tid).unwrap().transaction_stack = Some(id);
            if let Err(cmd) = self.proc_transaction(id, target_proc, target_thread) {
                self.pop_transaction(proc, tid, id);
                self.remove_last_complete(proc, tid);
                self.abort_transaction(target_proc, id, buffer_offset);
                return Err(fail(cmd, 0));
            }
        } else {
            let result = self.proc_transaction(id, target_proc, None);
            self.enqueue_thread_work(proc, tid, complete);
            if let Err(cmd) = result {
                self.remove_last_complete(proc, tid);
                self.abort_transaction(target_proc, id, buffer_offset);
                return Err(fail(cmd, 0));
            }
        }
        Ok(())
    }

    /// Record a sent transaction with the interface token its data starts
    /// with.
    fn trace_sent(
        &mut self,
        proc: ProcId,
        tid: Tid,
        target_proc: ProcId,
        id: TxnId,
        tr: &TransactionData,
        guest: &mut dyn GuestProcess,
    ) {
        let mut head = vec![0u8; (tr.data_size as usize).min(TOKEN_BYTES)];
        if guest.copy_from_user(tr.buffer, &mut head).is_err() {
            head.clear();
        }
        let from = &self.procs[&proc];
        let to = &self.procs[&target_proc];
        let record = TraceRecord {
            device: self.contexts[from.context].name.clone(),
            from_pid: from.creds.pid,
            from_euid: from.creds.euid,
            from_tid: tid,
            to_pid: to.creds.pid,
            descriptor: crate::trace::descriptor(&head),
            code: tr.code,
            oneway: tr.flags & TF_ONE_WAY != 0,
            waiting: to.waiting_threads.len() as u32
                + to.threads
                    .iter()
                    .filter(|(tid, t)| {
                        t.looper & LOOPER_POLL != 0
                            && self.available_for_proc_work(target_proc, **tid)
                    })
                    .count() as u32,
            loopers: to
                .threads
                .values()
                .filter(|t| t.looper & (LOOPER_REGISTERED | LOOPER_ENTERED) != 0)
                .count() as u32,
            ..TraceRecord::default()
        };
        self.trace.as_mut().unwrap().sent(id, record);
    }

    fn release_target_node(&mut self, node: Option<NodeId>) {
        if let Some(node) = node {
            self.dec_node(node, true, false);
        }
    }

    fn remove_last_complete(&mut self, proc: ProcId, tid: Tid) {
        if let Some(t) = self.thread(proc, tid)
            && let Some(index) = t
                .todo
                .iter()
                .rposition(|w| matches!(w, Work::TransactionComplete | Work::OnewaySpamSuspect))
        {
            t.todo.remove(index);
        }
    }

    /// Undo an allocated but undelivered transaction: release what was
    /// translated, free the buffer, drop the record.
    fn abort_transaction(&mut self, target_proc: ProcId, id: TxnId, buffer_offset: usize) {
        if let Some(t) = self.txns.get_mut(&id) {
            t.buffer = None;
        }
        self.txns.remove(&id);
        self.free_buffer(target_proc, buffer_offset, true, None);
    }

    pub(crate) fn buffer_mut(
        &mut self,
        proc: ProcId,
        offset: usize,
    ) -> Option<&mut crate::alloc::Buffer> {
        self.procs
            .get_mut(&proc)?
            .alloc
            .as_mut()?
            .buffers
            .get_mut(&offset)
    }

    fn record(&mut self, proc: ProcId, offset: usize, record: BufferRecord) {
        if let Some(b) = self.buffer_mut(proc, offset) {
            b.objects.push(record);
        }
    }

    /// Build the target buffer: data, offsets, scatter-gather payloads and
    /// the security context, with every object translated, then write it
    /// into the target's mapping in one copy.
    #[allow(clippy::too_many_arguments)]
    fn copy_transaction(
        &mut self,
        proc: ProcId,
        tid: Tid,
        target_proc: ProcId,
        id: TxnId,
        buffer_offset: usize,
        tr: &TransactionData,
        extra_buffers_size: u64,
        secctx: Option<&[u8]>,
        in_reply_to: Option<TxnId>,
        guest: &mut dyn GuestProcess,
    ) -> Result<(), Failure> {
        let data_size = tr.data_size as usize;
        let offsets_size = tr.offsets_size as usize;
        if offsets_size % 8 != 0 || extra_buffers_size % 8 != 0 {
            return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
        }
        let (buffer_size, vm_start) = {
            let alloc = self.procs[&target_proc].alloc.as_ref().unwrap();
            (
                alloc.buffers[&buffer_offset].size,
                alloc.user_address(buffer_offset),
            )
        };
        let off_start = align8(data_size as u64).unwrap() as usize;
        let sg_start = align8((off_start + offsets_size) as u64).unwrap() as usize;
        let sg_end = sg_start + extra_buffers_size as usize;
        let mut image = vec![0u8; buffer_size];

        if data_size != 0 {
            guest
                .copy_from_user(tr.buffer, &mut image[..data_size])
                .map_err(|_| fail(BR_FAILED_REPLY, errno::EFAULT))?;
        }
        if offsets_size != 0 {
            guest
                .copy_from_user(tr.offsets, &mut image[off_start..off_start + offsets_size])
                .map_err(|_| fail(BR_FAILED_REPLY, errno::EFAULT))?;
        }
        if let Some(ctx) = secctx {
            let at = buffer_size - align8(ctx.len() as u64).unwrap() as usize;
            image[at..at + ctx.len()].copy_from_slice(ctx);
            self.txns.get_mut(&id).unwrap().security_ctx = vm_start + at as u64;
        }

        let accept_fds = match in_reply_to {
            Some(original) => self.txns[&original].flags & TF_ACCEPT_FDS != 0,
            None => self.txns[&id]
                .target_node
                .is_some_and(|n| self.nodes[&n].accept_fds),
        };

        let mut off_min = 0usize;
        let mut sg_cursor = sg_start;
        let mut placed: Vec<PlacedPtr> = Vec::new();
        // binder_validate_fixup state.
        let mut last_fixup_obj: Option<usize> = None;
        let mut last_fixup_min = 0usize;
        let mut fixups: Vec<FdFixup> = Vec::new();

        for index in 0..offsets_size / 8 {
            let at = off_start + index * 8;
            let object_offset = u64_at(&image, at) as usize;
            let kind = if object_offset % 4 == 0 && object_offset + 4 <= data_size {
                u32_at(&image, object_offset)
            } else {
                0
            };
            let size = object_size(kind).ok_or(fail(BR_FAILED_REPLY, errno::EINVAL))?;
            if object_offset < off_min || object_offset + size > data_size {
                return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
            }
            off_min = object_offset + size;
            let object = &mut image[object_offset..object_offset + size];
            match kind {
                BINDER_TYPE_BINDER | BINDER_TYPE_WEAK_BINDER => {
                    let mut fp = FlatBinderObject::decode(object);
                    let strong = kind == BINDER_TYPE_BINDER;
                    let node = self.new_node(proc, fp.binder, fp.cookie, fp.flags);
                    if self.nodes[&node].cookie != fp.cookie {
                        return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                    }
                    let desc = self
                        .inc_ref_for_node(target_proc, node, strong, Some((proc, tid)))
                        .map_err(|e| fail(BR_FAILED_REPLY, e))?;
                    self.record(
                        target_proc,
                        buffer_offset,
                        BufferRecord::Handle { desc, strong },
                    );
                    fp.kind = if strong {
                        BINDER_TYPE_HANDLE
                    } else {
                        BINDER_TYPE_WEAK_HANDLE
                    };
                    fp.binder = desc as u64;
                    fp.cookie = 0;
                    object.copy_from_slice(&fp.encode());
                }
                BINDER_TYPE_HANDLE | BINDER_TYPE_WEAK_HANDLE => {
                    let mut fp = FlatBinderObject::decode(object);
                    let strong = kind == BINDER_TYPE_HANDLE;
                    let node = self
                        .get_ref(proc, fp.handle(), strong)
                        .map(|r| r.node)
                        .ok_or(fail(BR_FAILED_REPLY, errno::EINVAL))?;
                    if self.nodes[&node].proc == Some(target_proc) {
                        // Back to its owner: the owner sees its own object.
                        let n = &self.nodes[&node];
                        fp.kind = if strong {
                            BINDER_TYPE_BINDER
                        } else {
                            BINDER_TYPE_WEAK_BINDER
                        };
                        fp.binder = n.ptr;
                        fp.cookie = n.cookie;
                        self.inc_node(node, strong, false, None)
                            .map_err(|e| fail(BR_FAILED_REPLY, e))?;
                        self.record(
                            target_proc,
                            buffer_offset,
                            BufferRecord::Binder { node, strong },
                        );
                    } else {
                        let desc = self
                            .inc_ref_for_node(target_proc, node, strong, None)
                            .map_err(|e| fail(BR_FAILED_REPLY, e))?;
                        self.record(
                            target_proc,
                            buffer_offset,
                            BufferRecord::Handle { desc, strong },
                        );
                        fp.binder = desc as u64;
                        fp.cookie = 0;
                    }
                    object.copy_from_slice(&fp.encode());
                }
                BINDER_TYPE_FD => {
                    if !accept_fds {
                        return Err(fail(BR_FAILED_REPLY, errno::EPERM));
                    }
                    let fd = u32_at(object, 8);
                    let file = guest
                        .get_file(fd)
                        .map_err(|_| fail(BR_FAILED_REPLY, errno::EBADF))?;
                    fixups.push(FdFixup {
                        offset: object_offset + 8,
                        file,
                        in_array: false,
                    });
                    // pad_binder = 0; the fd is written by the receiver.
                    object[8..16].fill(0);
                }
                BINDER_TYPE_FDA => {
                    let fda = FdArrayObject::decode(object);
                    let parent = placed
                        .iter()
                        .position(|p| {
                            index > fda.parent as usize
                                && u64_at(&image, off_start + fda.parent as usize * 8) as usize
                                    == p.object_offset
                        })
                        .ok_or(fail(BR_FAILED_REPLY, errno::EINVAL))?;
                    if !validate_fixup(
                        &placed,
                        parent,
                        fda.parent_offset as usize,
                        last_fixup_obj,
                        last_fixup_min,
                    ) {
                        return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                    }
                    let fd_bytes = (fda.num_fds as usize)
                        .checked_mul(4)
                        .ok_or(fail(BR_FAILED_REPLY, errno::EINVAL))?;
                    let parent_ptr = &placed[parent];
                    if fd_bytes > parent_ptr.length
                        || fda.parent_offset as usize > parent_ptr.length - fd_bytes
                    {
                        return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                    }
                    let base = parent_ptr.target_offset + fda.parent_offset as usize;
                    if base % 4 != 0 {
                        return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                    }
                    if fda.num_fds != 0 && !accept_fds {
                        return Err(fail(BR_FAILED_REPLY, errno::EPERM));
                    }
                    for i in 0..fda.num_fds as usize {
                        let fd = u32_at(&image, base + i * 4);
                        let file = guest
                            .get_file(fd)
                            .map_err(|_| fail(BR_FAILED_REPLY, errno::EBADF))?;
                        fixups.push(FdFixup {
                            offset: base + i * 4,
                            file,
                            in_array: true,
                        });
                    }
                    last_fixup_obj = Some(parent);
                    last_fixup_min = fda.parent_offset as usize + fd_bytes;
                }
                BINDER_TYPE_PTR => {
                    let mut bp = BufferObject::decode(object);
                    let length = bp.length as usize;
                    if bp.length > (sg_end - sg_cursor) as u64 {
                        return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                    }
                    if length != 0 {
                        guest
                            .copy_from_user(bp.buffer, &mut image[sg_cursor..sg_cursor + length])
                            .map_err(|_| fail(BR_FAILED_REPLY, errno::EFAULT))?;
                    }
                    let target_offset = sg_cursor;
                    bp.buffer = vm_start + sg_cursor as u64;
                    sg_cursor += align8(bp.length).unwrap() as usize;
                    let mut parent = None;
                    if bp.flags & BINDER_BUFFER_FLAG_HAS_PARENT != 0 {
                        let p = placed
                            .iter()
                            .position(|p| {
                                index > bp.parent as usize
                                    && u64_at(&image, off_start + bp.parent as usize * 8) as usize
                                        == p.object_offset
                            })
                            .ok_or(fail(BR_FAILED_REPLY, errno::EINVAL))?;
                        if !validate_fixup(
                            &placed,
                            p,
                            bp.parent_offset as usize,
                            last_fixup_obj,
                            last_fixup_min,
                        ) {
                            return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                        }
                        let parent_ptr = &placed[p];
                        if parent_ptr.length < 8
                            || bp.parent_offset as usize > parent_ptr.length - 8
                        {
                            return Err(fail(BR_FAILED_REPLY, errno::EINVAL));
                        }
                        // Point the parent's embedded pointer at the copy.
                        let at = parent_ptr.target_offset + bp.parent_offset as usize;
                        put_u64(&mut image, at, bp.buffer);
                        parent = Some(p);
                    }
                    image[object_offset..object_offset + size].copy_from_slice(&bp.encode());
                    placed.push(PlacedPtr {
                        object_offset,
                        target_offset,
                        length,
                        parent,
                        parent_offset: bp.parent_offset as usize,
                    });
                    last_fixup_obj = Some(placed.len() - 1);
                    last_fixup_min = 0;
                }
                _ => unreachable!(),
            }
        }

        let memory = self.procs[&target_proc].receive.clone().unwrap();
        memory.write(buffer_offset, &image);
        self.txns.get_mut(&id).unwrap().fd_fixups = fixups;
        Ok(())
    }

    /// `binder_proc_transaction`: queue a transaction on a thread, the
    /// process, or (one-way, while one is outstanding) the node.
    pub(crate) fn proc_transaction(
        &mut self,
        id: TxnId,
        proc: ProcId,
        thread: Option<Tid>,
    ) -> Result<(), u32> {
        let t = &self.txns[&id];
        let oneway = t.is_oneway();
        let node = t.target_node.expect("transaction without target node");
        let mut pending_async = false;
        if oneway {
            let n = self.nodes.get_mut(&node).unwrap();
            if n.has_async_transaction {
                pending_async = true;
            } else {
                n.has_async_transaction = true;
            }
        }
        if !self.procs.contains_key(&proc)
            || thread.is_some_and(|tid| !self.thread_exists(proc, tid))
        {
            return Err(BR_DEAD_REPLY);
        }
        let thread = if thread.is_none() && !pending_async {
            self.select_thread(proc)
        } else {
            thread
        };
        if let Some(tid) = thread {
            self.enqueue_thread_work(proc, tid, Work::Transaction(id));
        } else if !pending_async {
            self.enqueue_proc_work(proc, Work::Transaction(id));
            self.wakeup_thread(proc, None);
        } else {
            self.nodes.get_mut(&node).unwrap().async_todo.push_back(id);
        }
        self.procs.get_mut(&proc).unwrap().outstanding_txns += 1;
        Ok(())
    }

    /// `binder_pop_transaction_ilocked`.
    pub(crate) fn pop_transaction(&mut self, proc: ProcId, tid: Tid, id: TxnId) {
        let parent = self.txns.get(&id).and_then(|t| t.from_parent);
        if let Some(thread) = self.thread(proc, tid)
            && thread.transaction_stack == Some(id)
        {
            thread.transaction_stack = parent;
        }
        if let Some(t) = self.txns.get_mut(&id) {
            t.from = None;
        }
    }

    /// `binder_free_transaction`.
    pub(crate) fn free_transaction(&mut self, id: TxnId) {
        let Some(t) = self.txns.remove(&id) else {
            return;
        };
        if let Some(trace) = &mut self.trace {
            trace.dropped(id);
        }
        if let Some(to) = t.to_proc {
            if let Some(p) = self.procs.get_mut(&to) {
                p.outstanding_txns = p.outstanding_txns.saturating_sub(1);
            }
            if let Some(offset) = t.buffer
                && let Some(b) = self.buffer_mut(to, offset)
            {
                b.txn = None;
            }
        }
    }

    /// `binder_send_failed_reply`: tell the nearest live caller in the chain.
    pub(crate) fn send_failed_reply(&mut self, mut id: TxnId, cmd: u32) {
        loop {
            let Some(t) = self.txns.get(&id) else { return };
            if let Some((proc, tid)) = t.from
                && self.thread_exists(proc, tid)
            {
                self.pop_transaction(proc, tid, id);
                let thread = self.thread(proc, tid).unwrap();
                if thread.reply_error.is_none() {
                    thread.reply_error = Some(cmd);
                    self.enqueue_thread_work(
                        proc,
                        tid,
                        Work::ReturnError {
                            cmd,
                            slot: ErrorSlot::Reply,
                        },
                    );
                }
                self.free_transaction(id);
                return;
            }
            let next = t.from_parent;
            self.free_transaction(id);
            match next {
                Some(n) => id = n,
                None => return,
            }
        }
    }

    /// `binder_cleanup_transaction`.
    pub(crate) fn cleanup_transaction(&mut self, id: TxnId, cmd: u32) {
        let Some(t) = self.txns.get(&id) else { return };
        if t.target_node.is_some() && !t.is_oneway() {
            self.send_failed_reply(id, cmd);
        } else {
            self.free_transaction(id);
        }
    }

    /// `binder_free_buf` plus `binder_release_entire_buffer`: release the
    /// objects a buffer holds, start the node's next one-way transaction,
    /// and return the space. `guest` (the owner, when it is the caller)
    /// closes fds received through fd arrays.
    pub(crate) fn free_buffer(
        &mut self,
        proc: ProcId,
        offset: usize,
        is_failure: bool,
        guest: Option<&mut dyn GuestProcess>,
    ) {
        let Some(alloc) = self.procs.get_mut(&proc).and_then(|p| p.alloc.as_mut()) else {
            return;
        };
        let size = alloc.buffers.get(&offset).map(|b| b.size).unwrap_or(0);
        let Some(buffer) = alloc.free(offset) else {
            return;
        };
        if buffer.clear_on_free
            && let Some(memory) = &self.procs[&proc].receive
        {
            memory.clear(offset, size);
        }
        if let Some(txn) = buffer.txn
            && let Some(t) = self.txns.get_mut(&txn)
        {
            t.buffer = None;
        }
        if buffer.is_async
            && let Some(node) = buffer.target_node
            && let Some(n) = self.nodes.get_mut(&node)
        {
            match n.async_todo.pop_front() {
                None => n.has_async_transaction = false,
                Some(next) => {
                    self.enqueue_proc_work(proc, Work::Transaction(next));
                    self.wakeup_proc(proc);
                }
            }
        }
        if let Some(node) = buffer.target_node {
            self.dec_node(node, true, false);
        }
        for record in &buffer.objects {
            match *record {
                BufferRecord::Binder { node, strong } => self.dec_node(node, strong, false),
                BufferRecord::Handle { desc, strong } => {
                    let _ = self.dec_ref(proc, desc, strong);
                }
            }
        }
        if !is_failure && let Some(guest) = guest {
            for fd in buffer.fda_fds {
                guest.close_fd(fd);
            }
        }
    }
}

/// `binder_validate_fixup`: fixups must go into the last fixed-up buffer
/// or one of its ancestors, at increasing offsets.
fn validate_fixup(
    placed: &[PlacedPtr],
    parent: usize,
    fixup_offset: usize,
    last_obj: Option<usize>,
    last_min: usize,
) -> bool {
    let Some(mut cursor) = last_obj else {
        return false;
    };
    let mut min = last_min;
    while cursor != parent {
        match placed[cursor].parent {
            Some(p) => {
                // Moving up to an ancestor: fixups continue after the
                // pointer to the child we came from.
                min = placed[cursor].parent_offset + 8;
                cursor = p;
            }
            None => return false,
        }
    }
    fixup_offset >= min
}
