//! Driver state and the reference-counting rules of `drivers/android/binder.c`.
//!
//! Split as in Linux:
//! - global: contexts (one per device, each with its context manager),
//!   nodes, transactions and death notifications;
//! - per process: threads, the node table by user pointer, the handle table,
//!   the process work queue, waiting threads and the receive allocator;
//! - per thread: looper state, the thread work queue and the transaction
//!   stack.
//!
//! All of it sits behind the driver's single lock; the functions here run
//! with that lock held.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::{Arc, Condvar};

use crate::alloc::Allocator;
use crate::host::{Credentials, Errno, File, ReceiveMemory, errno};

/// A map keyed by the driver's ids and guest tids. Neither is chosen by a
/// guest program, so a multiplicative hash does: the default SipHash was
/// the largest share of the driver's time per transaction.
pub(crate) type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

#[derive(Default)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }

    fn write_i32(&mut self, v: i32) {
        self.write_u64(v as u32 as u64);
    }
}

pub(crate) type ProcId = u64;
pub(crate) type NodeId = u64;
pub(crate) type TxnId = u64;
pub(crate) type DeathId = u64;
/// A guest (Linux) thread id.
pub type Tid = i32;

pub(crate) const LOOPER_REGISTERED: u32 = 0x01;
pub(crate) const LOOPER_ENTERED: u32 = 0x02;
pub(crate) const LOOPER_EXITED: u32 = 0x04;
pub(crate) const LOOPER_INVALID: u32 = 0x08;
pub(crate) const LOOPER_WAITING: u32 = 0x10;
pub(crate) const LOOPER_POLL: u32 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorSlot {
    /// `thread->return_error`: the thread's own failed command.
    Return,
    /// `thread->reply_error`: a failed reply delivered by another thread.
    Reply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Work {
    Transaction(TxnId),
    TransactionComplete,
    OnewaySpamSuspect,
    ReturnError { cmd: u32, slot: ErrorSlot },
    Node(NodeId),
    Death(DeathId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkList {
    Proc(ProcId),
    Thread(ProcId, Tid),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExtendedError {
    pub id: u32,
    pub command: u32,
    pub param: i32,
}

pub(crate) struct Thread {
    pub looper: u32,
    pub looper_need_return: bool,
    pub todo: VecDeque<Work>,
    pub process_todo: bool,
    pub transaction_stack: Option<TxnId>,
    pub return_error: Option<u32>,
    pub reply_error: Option<u32>,
    pub ee: ExtendedError,
    pub interrupted: bool,
    pub wait: Arc<Condvar>,
    /// The read that would wait here instead returned to its caller
    /// ([`crate::Driver::ioctl_or_park`]); this resumes it.
    pub parked: Option<Resume>,
}

impl Thread {
    fn new() -> Self {
        Self {
            looper: 0,
            // A new thread returns to user space from its first read even
            // without work, as `binder_get_thread` arranges.
            looper_need_return: true,
            todo: VecDeque::new(),
            process_todo: false,
            transaction_stack: None,
            return_error: None,
            reply_error: None,
            ee: ExtendedError::default(),
            interrupted: false,
            wait: Arc::new(Condvar::new()),
            parked: None,
        }
    }
}

pub(crate) struct Ref {
    pub node: NodeId,
    pub strong: u32,
    pub weak: u32,
    pub death: Option<DeathId>,
}

/// Resumes a parked read: its caller issues the ioctl again.
pub type Resume = Box<dyn FnOnce() + Send>;

/// Called with a thread id when a poll-mode thread gets work, or with
/// `None` when process work has no waiting thread to take it.
pub type Notifier = Arc<dyn Fn(Option<Tid>) + Send + Sync>;

pub(crate) struct Proc {
    pub context: usize,
    pub creds: Credentials,
    pub threads: IdMap<Tid, Thread>,
    /// Nodes this process owns, by user pointer.
    pub nodes: BTreeMap<u64, NodeId>,
    pub refs_by_desc: BTreeMap<u32, Ref>,
    pub refs_by_node: IdMap<NodeId, u32>,
    pub todo: VecDeque<Work>,
    pub waiting_threads: VecDeque<Tid>,
    pub delivered_death: Vec<DeathId>,
    pub alloc: Option<Allocator>,
    pub receive: Option<Arc<dyn ReceiveMemory>>,
    pub max_threads: u32,
    pub requested_threads: u32,
    pub requested_threads_started: u32,
    pub oneway_spam_detection: bool,
    pub nonblocking: bool,
    pub notifier: Option<Notifier>,
    pub outstanding_txns: u32,
}

pub(crate) struct Node {
    pub proc: Option<ProcId>,
    pub context: usize,
    pub ptr: u64,
    pub cookie: u64,
    pub internal_strong_refs: u32,
    pub local_strong_refs: u32,
    pub local_weak_refs: u32,
    /// Processes holding a ref (a process has at most one ref per node).
    pub refs: BTreeSet<ProcId>,
    pub has_strong_ref: bool,
    pub pending_strong_ref: bool,
    pub has_weak_ref: bool,
    pub pending_weak_ref: bool,
    pub accept_fds: bool,
    pub txn_security_ctx: bool,
    pub has_async_transaction: bool,
    pub async_todo: VecDeque<TxnId>,
    pub work_queued: Option<WorkList>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeathState {
    /// Registered, target alive.
    Armed,
    DeadBinder,
    DeadBinderAndClear,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeathLocation {
    Queued(WorkList),
    Delivered,
}

pub(crate) struct Death {
    pub proc: ProcId,
    pub cookie: u64,
    pub state: DeathState,
    pub location: Option<DeathLocation>,
}

pub(crate) struct FdFixup {
    /// Offset of the fd word in the target buffer.
    pub offset: usize,
    pub file: File,
    pub in_array: bool,
}

pub(crate) struct Txn {
    /// The sending thread of a synchronous transaction.
    pub from: Option<(ProcId, Tid)>,
    pub from_parent: Option<TxnId>,
    pub to_proc: Option<ProcId>,
    pub to_thread: Option<Tid>,
    pub to_parent: Option<TxnId>,
    pub code: u32,
    pub flags: u32,
    pub sender_euid: u32,
    pub target_node: Option<NodeId>,
    /// Offset of the buffer in `to_proc`'s allocator.
    pub buffer: Option<usize>,
    pub security_ctx: u64,
    pub fd_fixups: Vec<FdFixup>,
}

impl Txn {
    pub fn is_oneway(&self) -> bool {
        self.flags & crate::uapi::TF_ONE_WAY != 0
    }
}

/// An object translated into a buffer, released when the buffer is freed.
#[derive(Clone, Copy, Debug)]
pub(crate) enum BufferRecord {
    /// A local node reference (`BINDER_TYPE_(WEAK_)BINDER` in the buffer).
    Binder { node: NodeId, strong: bool },
    /// A handle in the buffer owner's table.
    Handle { desc: u32, strong: bool },
}

#[derive(Default)]
pub(crate) struct Context {
    pub name: String,
    pub mgr_node: Option<NodeId>,
    pub mgr_uid: Option<u32>,
}

#[derive(Default)]
pub(crate) struct State {
    pub contexts: Vec<Context>,
    pub procs: IdMap<ProcId, Proc>,
    pub nodes: IdMap<NodeId, Node>,
    pub txns: IdMap<TxnId, Txn>,
    pub deaths: IdMap<DeathId, Death>,
    next_id: u64,
    /// Thread wakes to deliver once the lock is released.
    pub wakes: Vec<Arc<Condvar>>,
    /// Parked reads to resume once the lock is released.
    pub resumes: Vec<Resume>,
    /// External wakes to deliver once the lock is released.
    pub notifications: Vec<(Notifier, Option<Tid>)>,
    /// Transactions being traced ([`crate::Driver::start_trace`]).
    pub trace: Option<crate::trace::Trace>,
}

/// Wakes collected under the lock, delivered after it is released.
pub(crate) struct PendingWakes {
    wakes: Vec<Arc<Condvar>>,
    resumes: Vec<Resume>,
    notifications: Vec<(Notifier, Option<Tid>)>,
}

impl PendingWakes {
    pub fn is_empty(&self) -> bool {
        self.wakes.is_empty() && self.resumes.is_empty() && self.notifications.is_empty()
    }

    pub fn deliver(self) {
        for resume in self.resumes {
            resume();
        }
        for wait in self.wakes {
            wait.notify_one();
        }
        for (notify, tid) in self.notifications {
            notify(tid);
        }
    }
}

impl State {
    pub fn take_wakes(&mut self) -> PendingWakes {
        PendingWakes {
            wakes: std::mem::take(&mut self.wakes),
            resumes: std::mem::take(&mut self.resumes),
            notifications: std::mem::take(&mut self.notifications),
        }
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn context_index(&mut self, name: &str) -> usize {
        if let Some(index) = self.contexts.iter().position(|c| c.name == name) {
            return index;
        }
        self.contexts.push(Context {
            name: name.to_owned(),
            ..Context::default()
        });
        self.contexts.len() - 1
    }

    pub fn new_proc(&mut self, context: usize, creds: Credentials) -> ProcId {
        let id = self.next_id();
        self.procs.insert(
            id,
            Proc {
                context,
                creds,
                threads: IdMap::default(),
                nodes: BTreeMap::new(),
                refs_by_desc: BTreeMap::new(),
                refs_by_node: IdMap::default(),
                todo: VecDeque::new(),
                waiting_threads: VecDeque::new(),
                delivered_death: Vec::new(),
                alloc: None,
                receive: None,
                max_threads: 0,
                requested_threads: 0,
                requested_threads_started: 0,
                oneway_spam_detection: false,
                nonblocking: false,
                notifier: None,
                outstanding_txns: 0,
            },
        );
        id
    }

    /// `binder_get_thread`.
    pub fn thread_or_create(&mut self, proc: ProcId, tid: Tid) -> Result<(), Errno> {
        let p = self.procs.get_mut(&proc).ok_or(errno::EBADF)?;
        p.threads.entry(tid).or_insert_with(Thread::new);
        Ok(())
    }

    pub fn thread(&mut self, proc: ProcId, tid: Tid) -> Option<&mut Thread> {
        self.procs.get_mut(&proc)?.threads.get_mut(&tid)
    }

    pub fn thread_exists(&self, proc: ProcId, tid: Tid) -> bool {
        self.procs
            .get(&proc)
            .is_some_and(|p| p.threads.contains_key(&tid))
    }

    // ---- work queues -------------------------------------------------

    fn list(&mut self, list: WorkList) -> Option<&mut VecDeque<Work>> {
        match list {
            WorkList::Proc(p) => self.procs.get_mut(&p).map(|p| &mut p.todo),
            WorkList::Thread(p, t) => self.thread(p, t).map(|t| &mut t.todo),
        }
    }

    /// Wake the thread if it waits for work: notify its wait, or resume its
    /// parked read. Either is delivered once the lock is released, so the
    /// woken thread does not wake up only to block on the driver lock.
    pub fn wake_waiter(&mut self, proc: ProcId, tid: Tid) {
        let Some(p) = self.procs.get_mut(&proc) else {
            return;
        };
        let Some(t) = p.threads.get_mut(&tid) else {
            return;
        };
        match t.parked.take() {
            Some(resume) => {
                p.waiting_threads.retain(|t| *t != tid);
                self.resumes.push(resume);
            }
            None => self.wakes.push(t.wait.clone()),
        }
    }

    fn wake_thread(&mut self, proc: ProcId, tid: Tid) {
        self.wake_waiter(proc, tid);
        let Some(p) = self.procs.get(&proc) else {
            return;
        };
        if p.threads
            .get(&tid)
            .is_some_and(|t| t.looper & LOOPER_POLL != 0)
            && let Some(n) = &p.notifier
        {
            self.notifications.push((n.clone(), Some(tid)));
        }
    }

    /// `binder_enqueue_thread_work_ilocked`: the thread returns to user
    /// space for it.
    pub fn enqueue_thread_work(&mut self, proc: ProcId, tid: Tid, work: Work) {
        if let Some(t) = self.thread(proc, tid) {
            t.todo.push_back(work);
            t.process_todo = true;
            self.wake_thread(proc, tid);
        }
    }

    /// `binder_enqueue_deferred_thread_work_ilocked`: queued, but the thread
    /// does not return to user space for it alone.
    pub fn enqueue_deferred_thread_work(&mut self, proc: ProcId, tid: Tid, work: Work) {
        if let Some(t) = self.thread(proc, tid) {
            t.todo.push_back(work);
        }
    }

    pub fn enqueue_proc_work(&mut self, proc: ProcId, work: Work) {
        if let Some(p) = self.procs.get_mut(&proc) {
            p.todo.push_back(work);
        }
    }

    fn remove_work(&mut self, list: WorkList, work: Work) {
        if let Some(queue) = self.list(list)
            && let Some(index) = queue.iter().position(|w| *w == work)
        {
            queue.remove(index);
        }
    }

    /// `binder_select_thread_ilocked`.
    pub fn select_thread(&mut self, proc: ProcId) -> Option<Tid> {
        self.procs.get_mut(&proc)?.waiting_threads.pop_front()
    }

    /// `binder_wakeup_thread_ilocked`.
    pub fn wakeup_thread(&mut self, proc: ProcId, tid: Option<Tid>) {
        match tid {
            Some(tid) => self.wake_thread(proc, tid),
            None => {
                // No thread waits for process work: poll-mode threads (and
                // an external reactor) must be told instead.
                if let Some(p) = self.procs.get(&proc)
                    && let Some(n) = &p.notifier
                {
                    self.notifications.push((n.clone(), None));
                }
            }
        }
    }

    /// `binder_wakeup_proc_ilocked`.
    pub fn wakeup_proc(&mut self, proc: ProcId) {
        let tid = self.select_thread(proc);
        self.wakeup_thread(proc, tid);
    }

    fn dequeue_node_work(&mut self, node: NodeId) {
        if let Some(list) = self.nodes.get_mut(&node).and_then(|n| n.work_queued.take()) {
            self.remove_work(list, Work::Node(node));
        }
    }

    pub fn dequeue_death_work(&mut self, death: DeathId) {
        let Some(d) = self.deaths.get_mut(&death) else {
            return;
        };
        let proc = d.proc;
        match d.location.take() {
            Some(DeathLocation::Queued(list)) => self.remove_work(list, Work::Death(death)),
            Some(DeathLocation::Delivered) => {
                if let Some(p) = self.procs.get_mut(&proc) {
                    p.delivered_death.retain(|d| *d != death);
                }
            }
            None => {}
        }
    }

    /// Queue death work on the looper thread that asked, else on the process.
    pub fn queue_death_for_thread_or_proc(&mut self, death: DeathId, proc: ProcId, tid: Tid) {
        let on_thread = self
            .procs
            .get(&proc)
            .and_then(|p| p.threads.get(&tid))
            .is_some_and(|t| t.looper & (LOOPER_REGISTERED | LOOPER_ENTERED) != 0);
        if on_thread {
            self.deaths.get_mut(&death).unwrap().location =
                Some(DeathLocation::Queued(WorkList::Thread(proc, tid)));
            self.enqueue_thread_work(proc, tid, Work::Death(death));
        } else {
            self.deaths.get_mut(&death).unwrap().location =
                Some(DeathLocation::Queued(WorkList::Proc(proc)));
            self.enqueue_proc_work(proc, Work::Death(death));
            self.wakeup_proc(proc);
        }
    }

    // ---- nodes -------------------------------------------------------

    pub fn is_context_manager(&self, node: NodeId) -> bool {
        self.nodes
            .get(&node)
            .is_some_and(|n| self.contexts[n.context].mgr_node == Some(node))
    }

    /// `binder_new_node`: the existing node for `ptr`, or a new one.
    pub fn new_node(&mut self, proc: ProcId, ptr: u64, cookie: u64, flags: u32) -> NodeId {
        if let Some(&id) = self.procs[&proc].nodes.get(&ptr) {
            return id;
        }
        let id = self.next_id();
        let context = self.procs[&proc].context;
        self.nodes.insert(
            id,
            Node {
                proc: Some(proc),
                context,
                ptr,
                cookie,
                internal_strong_refs: 0,
                local_strong_refs: 0,
                local_weak_refs: 0,
                refs: BTreeSet::new(),
                has_strong_ref: false,
                pending_strong_ref: false,
                has_weak_ref: false,
                pending_weak_ref: false,
                accept_fds: flags & crate::uapi::FLAT_BINDER_FLAG_ACCEPTS_FDS != 0,
                txn_security_ctx: flags & crate::uapi::FLAT_BINDER_FLAG_TXN_SECURITY_CTX != 0,
                has_async_transaction: false,
                async_todo: VecDeque::new(),
                work_queued: None,
            },
        );
        self.procs.get_mut(&proc).unwrap().nodes.insert(ptr, id);
        id
    }

    /// `binder_inc_node_nilocked`. `target` is the thread whose todo list
    /// receives the node work (`target_list`).
    pub fn inc_node(
        &mut self,
        id: NodeId,
        strong: bool,
        internal: bool,
        target: Option<(ProcId, Tid)>,
    ) -> Result<(), Errno> {
        let is_mgr = self.is_context_manager(id);
        let node = self.nodes.get_mut(&id).ok_or(errno::EINVAL)?;
        if strong {
            if internal {
                if target.is_none()
                    && node.internal_strong_refs == 0
                    && !(node.proc.is_some() && is_mgr && node.has_strong_ref)
                {
                    return Err(errno::EINVAL);
                }
                node.internal_strong_refs += 1;
            } else {
                node.local_strong_refs += 1;
            }
            if !node.has_strong_ref
                && let Some((p, t)) = target
            {
                self.dequeue_node_work(id);
                self.nodes.get_mut(&id).unwrap().work_queued = Some(WorkList::Thread(p, t));
                self.enqueue_deferred_thread_work(p, t, Work::Node(id));
            }
        } else {
            if !internal {
                node.local_weak_refs += 1;
            }
            if !node.has_weak_ref
                && node.work_queued.is_none()
                && let Some((p, t)) = target
            {
                node.work_queued = Some(WorkList::Thread(p, t));
                self.enqueue_deferred_thread_work(p, t, Work::Node(id));
            }
        }
        Ok(())
    }

    /// `binder_dec_node_nilocked` plus the free it may call for.
    pub fn dec_node(&mut self, id: NodeId, strong: bool, internal: bool) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        if strong {
            let count = if internal {
                &mut node.internal_strong_refs
            } else {
                &mut node.local_strong_refs
            };
            *count = count.saturating_sub(1);
            if node.local_strong_refs != 0 || node.internal_strong_refs != 0 {
                return;
            }
        } else {
            if !internal {
                node.local_weak_refs = node.local_weak_refs.saturating_sub(1);
            }
            if node.local_weak_refs != 0 || !node.refs.is_empty() {
                return;
            }
        }
        match node.proc {
            Some(proc) if node.has_strong_ref || node.has_weak_ref => {
                if node.work_queued.is_none() {
                    node.work_queued = Some(WorkList::Proc(proc));
                    self.enqueue_proc_work(proc, Work::Node(id));
                    self.wakeup_proc(proc);
                }
            }
            proc => {
                if node.refs.is_empty() && node.local_strong_refs == 0 && node.local_weak_refs == 0
                {
                    let ptr = node.ptr;
                    if let Some(proc) = proc {
                        self.dequeue_node_work(id);
                        if let Some(p) = self.procs.get_mut(&proc) {
                            p.nodes.remove(&ptr);
                        }
                    }
                    self.nodes.remove(&id);
                }
            }
        }
    }

    // ---- refs --------------------------------------------------------

    /// `binder_get_ref_olocked`.
    pub fn get_ref(&self, proc: ProcId, desc: u32, need_strong: bool) -> Option<&Ref> {
        let r = self.procs.get(&proc)?.refs_by_desc.get(&desc)?;
        (!need_strong || r.strong != 0).then_some(r)
    }

    /// `binder_inc_ref_for_node`: the handle `proc` holds for `node`,
    /// created with the lowest free descriptor if needed.
    pub fn inc_ref_for_node(
        &mut self,
        proc: ProcId,
        node: NodeId,
        strong: bool,
        target: Option<(ProcId, Tid)>,
    ) -> Result<u32, Errno> {
        let existing = self
            .procs
            .get(&proc)
            .ok_or(errno::ESRCH)?
            .refs_by_node
            .get(&node)
            .copied();
        let (desc, created) = match existing {
            Some(desc) => (desc, false),
            None => {
                let first = if self.is_context_manager(node) { 0 } else { 1 };
                let p = self.procs.get_mut(&proc).unwrap();
                let mut desc = first;
                for &used in p.refs_by_desc.range(first..).map(|(d, _)| d) {
                    if used > desc {
                        break;
                    }
                    desc = used + 1;
                }
                p.refs_by_desc.insert(
                    desc,
                    Ref {
                        node,
                        strong: 0,
                        weak: 0,
                        death: None,
                    },
                );
                p.refs_by_node.insert(node, desc);
                self.nodes
                    .get_mut(&node)
                    .ok_or(errno::EINVAL)?
                    .refs
                    .insert(proc);
                (desc, true)
            }
        };
        if let Err(e) = self.inc_ref(proc, desc, strong, target) {
            if created {
                self.cleanup_ref(proc, desc);
            }
            return Err(e);
        }
        Ok(desc)
    }

    /// `binder_inc_ref_olocked`.
    pub fn inc_ref(
        &mut self,
        proc: ProcId,
        desc: u32,
        strong: bool,
        target: Option<(ProcId, Tid)>,
    ) -> Result<(), Errno> {
        let r = &self.procs[&proc].refs_by_desc[&desc];
        let (node, count) = (r.node, if strong { r.strong } else { r.weak });
        if count == 0 {
            self.inc_node(node, strong, true, target)?;
        }
        let r = self
            .procs
            .get_mut(&proc)
            .unwrap()
            .refs_by_desc
            .get_mut(&desc)
            .unwrap();
        if strong {
            r.strong += 1;
        } else {
            r.weak += 1;
        }
        Ok(())
    }

    /// `binder_dec_ref_olocked`; frees the ref when both counts reach zero.
    pub fn dec_ref(&mut self, proc: ProcId, desc: u32, strong: bool) -> Result<(), Errno> {
        let r = self
            .procs
            .get_mut(&proc)
            .and_then(|p| p.refs_by_desc.get_mut(&desc))
            .ok_or(errno::EINVAL)?;
        let node = r.node;
        if strong {
            if r.strong == 0 {
                return Ok(());
            }
            r.strong -= 1;
            let now = r.strong;
            if now == 0 {
                self.dec_node(node, true, true);
            }
        } else {
            if r.weak == 0 {
                return Ok(());
            }
            r.weak -= 1;
        }
        let r = &self.procs[&proc].refs_by_desc[&desc];
        if r.strong == 0 && r.weak == 0 {
            self.cleanup_ref(proc, desc);
        }
        Ok(())
    }

    /// `binder_update_ref_for_handle`.
    pub fn update_ref_for_handle(
        &mut self,
        proc: ProcId,
        desc: u32,
        increment: bool,
        strong: bool,
    ) -> Result<(), Errno> {
        if self.get_ref(proc, desc, strong).is_none() {
            return Err(errno::EINVAL);
        }
        if increment {
            self.inc_ref(proc, desc, strong, None)
        } else {
            self.dec_ref(proc, desc, strong)
        }
    }

    /// `binder_cleanup_ref_olocked` + `binder_free_ref`.
    pub fn cleanup_ref(&mut self, proc: ProcId, desc: u32) {
        let Some(p) = self.procs.get_mut(&proc) else {
            return;
        };
        let Some(r) = p.refs_by_desc.remove(&desc) else {
            return;
        };
        p.refs_by_node.remove(&r.node);
        if r.strong != 0 {
            self.dec_node(r.node, true, true);
        }
        if let Some(n) = self.nodes.get_mut(&r.node) {
            n.refs.remove(&proc);
        }
        self.dec_node(r.node, false, true);
        if let Some(death) = r.death {
            self.dequeue_death_work(death);
            self.deaths.remove(&death);
        }
    }
}
