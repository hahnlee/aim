//! The driver's entry points: open, mmap, ioctl, poll and release, shaped
//! like the Linux file operations of `/dev/binder`.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::alloc::Allocator;
use crate::host::{Credentials, Errno, GuestProcess, ReceiveMemory, errno};
use crate::read::Pass;
use crate::state::{ExtendedError, LOOPER_POLL, LOOPER_WAITING, Notifier, ProcId, State, Tid};
use crate::uapi::*;

/// The binder devices of the pinned image. Each is its own context: its own
/// context manager, and nodes that never cross to another device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    Binder,
    HwBinder,
    VndBinder,
}

impl Device {
    pub fn path(self) -> &'static str {
        match self {
            Self::Binder => "/dev/binder",
            Self::HwBinder => "/dev/hwbinder",
            Self::VndBinder => "/dev/vndbinder",
        }
    }

    pub fn from_path(path: &str) -> Option<Self> {
        [Self::Binder, Self::HwBinder, Self::VndBinder]
            .into_iter()
            .find(|d| d.path() == path)
    }
}

/// One open binder file description (`struct binder_proc`). Dropping it
/// does not release the process; call [`Driver::release`] on the last close
/// or at process exit, as the fd layer decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcHandle(ProcId);

/// Linux caps nothing here, but a binder mapping larger than 4 MiB is
/// silently limited to 4 MiB.
pub use crate::alloc::MAX_MAPPING;

/// The driver: every binder process and all cross-process state.
pub struct Driver {
    state: Mutex<State>,
}

impl Default for Driver {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
        }
    }
}

impl Driver {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Deliver the external wakes queued under the lock.
    fn unlock(&self, mut guard: MutexGuard<'_, State>) {
        let pending = guard.take_wakes();
        drop(guard);
        pending.deliver();
    }

    /// `binder_open`.
    pub fn open(&self, device: Device, creds: Credentials) -> ProcHandle {
        let mut st = self.lock();
        let context = st.context_index(device.path());
        let id = st.new_proc(context, creds);
        self.unlock(st);
        ProcHandle(id)
    }

    /// `O_NONBLOCK` on the file description: reads with no work fail with
    /// `EAGAIN` instead of waiting.
    pub fn set_nonblocking(&self, proc: ProcHandle, nonblocking: bool) {
        let mut st = self.lock();
        if let Some(p) = st.procs.get_mut(&proc.0) {
            p.nonblocking = nonblocking;
        }
        self.unlock(st);
    }

    /// Register the callback that makes the binder fd readable for epoll: it
    /// runs (outside the driver lock) when a poll-mode thread gets work, or
    /// process work finds no waiting thread.
    pub fn set_notifier(&self, proc: ProcHandle, notifier: Notifier) {
        let mut st = self.lock();
        if let Some(p) = st.procs.get_mut(&proc.0) {
            p.notifier = Some(notifier);
        }
        self.unlock(st);
    }

    /// `binder_mmap`: install the receive buffer. `vm_start` is the address
    /// the guest mapped it at; `memory` writes into that mapping.
    pub fn mmap(
        &self,
        proc: ProcHandle,
        vm_start: u64,
        len: usize,
        writable: bool,
        memory: Arc<dyn ReceiveMemory>,
    ) -> Result<(), Errno> {
        if writable {
            return Err(errno::EPERM);
        }
        let mut st = self.lock();
        let p = st.procs.get_mut(&proc.0).ok_or(errno::EBADF)?;
        if p.alloc.is_some() {
            return Err(errno::EBUSY);
        }
        p.alloc = Some(Allocator::new(vm_start, len));
        p.receive = Some(memory);
        self.unlock(st);
        Ok(())
    }

    /// `binder_poll` for thread `tid`: marks it a poll-mode thread and
    /// reports whether a read would find work.
    pub fn poll(&self, proc: ProcHandle, tid: Tid) -> Result<bool, Errno> {
        let mut st = self.lock();
        st.thread_or_create(proc.0, tid)?;
        st.thread(proc.0, tid).unwrap().looper |= LOOPER_POLL;
        let proc_work = st.available_for_proc_work(proc.0, tid);
        let ready = st.has_work(proc.0, tid, proc_work);
        self.unlock(st);
        Ok(ready)
    }

    /// Interrupt a thread blocked in a read (a signal arrived): the ioctl
    /// fails with `EINTR` and libbinder retries it.
    pub fn interrupt(&self, proc: ProcHandle, tid: Tid) {
        let mut st = self.lock();
        if let Some(t) = st.thread(proc.0, tid) {
            t.interrupted = true;
            t.wait.notify_all();
        }
        self.unlock(st);
    }

    /// `binder_release`: the last reference to the file description is gone
    /// (close or process exit). Callers waiting on this process get
    /// `BR_DEAD_REPLY`; holders of its nodes get `BR_DEAD_BINDER`.
    pub fn release(&self, proc: ProcHandle) {
        let mut st = self.lock();
        if let Some(p) = st.procs.get(&proc.0) {
            for t in p.threads.values() {
                t.wait.notify_all();
            }
        }
        st.release_proc(proc.0);
        self.unlock(st);
    }

    /// `binder_ioctl` with the argument structure already copied in:
    /// `arg` has exactly `_IOC_SIZE(cmd)` bytes and holds the result on
    /// return. Pointers inside it (the BINDER_WRITE_READ buffers) are
    /// resolved through `guest`.
    pub fn ioctl(
        &self,
        proc: ProcHandle,
        tid: Tid,
        cmd: u32,
        arg: &mut [u8],
        guest: &mut dyn GuestProcess,
    ) -> Result<(), Errno> {
        if arg.len() != ioc_size(cmd) {
            return Err(errno::EINVAL);
        }
        let mut st = self.lock();
        if let Err(e) = st.thread_or_create(proc.0, tid) {
            self.unlock(st);
            return Err(e);
        }
        let (mut st, result) = self.dispatch(st, proc.0, tid, cmd, arg, guest);
        if let Some(t) = st.thread(proc.0, tid) {
            t.looper_need_return = false;
        }
        self.unlock(st);
        result
    }

    /// `ioctl(fd, cmd, arg)` with `arg` a guest address: copies the argument
    /// structure in and out through `guest`.
    pub fn ioctl_user(
        &self,
        proc: ProcHandle,
        tid: Tid,
        cmd: u32,
        arg: u64,
        guest: &mut dyn GuestProcess,
    ) -> Result<(), Errno> {
        let mut bytes = vec![0u8; ioc_size(cmd)];
        let reads_arg = cmd >> 30 & 1 != 0;
        if reads_arg && !bytes.is_empty() {
            guest.copy_from_user(arg, &mut bytes)?;
        }
        let result = self.ioctl(proc, tid, cmd, &mut bytes, guest);
        let writes_arg = cmd >> 31 != 0;
        // BINDER_WRITE_READ reports consumption even when it fails.
        if writes_arg && !bytes.is_empty() && (result.is_ok() || cmd == BINDER_WRITE_READ) {
            guest.copy_to_user(arg, &bytes)?;
        }
        result
    }

    fn dispatch<'a>(
        &'a self,
        mut st: MutexGuard<'a, State>,
        proc: ProcId,
        tid: Tid,
        cmd: u32,
        arg: &mut [u8],
        guest: &mut dyn GuestProcess,
    ) -> (MutexGuard<'a, State>, Result<(), Errno>) {
        let result = match cmd {
            BINDER_WRITE_READ => return self.write_read(st, proc, tid, arg, guest),
            BINDER_SET_MAX_THREADS => {
                st.procs.get_mut(&proc).unwrap().max_threads = u32_at(arg, 0);
                Ok(())
            }
            BINDER_SET_CONTEXT_MGR => st.set_context_manager(proc, None),
            BINDER_SET_CONTEXT_MGR_EXT => {
                let fbo = FlatBinderObject::decode(arg);
                st.set_context_manager(proc, Some(fbo))
            }
            BINDER_THREAD_EXIT => {
                st.thread_release(proc, tid);
                Ok(())
            }
            BINDER_VERSION => {
                put_i32(arg, 0, CURRENT_PROTOCOL_VERSION);
                Ok(())
            }
            BINDER_GET_NODE_INFO_FOR_REF => st.node_info_for_ref(proc, arg),
            BINDER_GET_NODE_DEBUG_INFO => {
                st.node_debug_info(proc, arg);
                Ok(())
            }
            BINDER_ENABLE_ONEWAY_SPAM_DETECTION => {
                st.procs.get_mut(&proc).unwrap().oneway_spam_detection = u32_at(arg, 0) != 0;
                Ok(())
            }
            BINDER_GET_EXTENDED_ERROR => {
                let t = st.thread(proc, tid).unwrap();
                let ee = std::mem::replace(
                    &mut t.ee,
                    ExtendedError {
                        id: 0,
                        command: BR_OK,
                        param: 0,
                    },
                );
                put_u32(arg, 0, ee.id);
                put_u32(arg, 4, ee.command);
                put_i32(arg, 8, ee.param);
                Ok(())
            }
            // The device has no cgroup freezer: freezing is not supported,
            // and there is no frozen state to report.
            BINDER_FREEZE | BINDER_GET_FROZEN_INFO => Err(errno::EINVAL),
            _ => Err(errno::EINVAL),
        };
        (st, result)
    }

    /// `binder_ioctl_write_read`.
    fn write_read<'a>(
        &'a self,
        mut st: MutexGuard<'a, State>,
        proc: ProcId,
        tid: Tid,
        arg: &mut [u8],
        guest: &mut dyn GuestProcess,
    ) -> (MutexGuard<'a, State>, Result<(), Errno>) {
        let mut bwr = WriteRead::decode(arg);
        let mut result = Ok(());
        if bwr.write_size > 0 {
            let consumed = bwr.write_consumed as usize;
            let size = bwr.write_size as usize;
            let outcome = if consumed > size {
                Err(errno::EFAULT)
            } else {
                let mut write = vec![0u8; size - consumed];
                match guest.copy_from_user(bwr.write_buffer + consumed as u64, &mut write) {
                    Err(e) => Err(e),
                    Ok(()) => {
                        let mut done = 0;
                        let r = st.thread_write(proc, tid, &write, &mut done, guest);
                        bwr.write_consumed = (consumed + done) as u64;
                        r
                    }
                }
            };
            if let Err(e) = outcome {
                bwr.read_consumed = 0;
                arg.copy_from_slice(&bwr.encode());
                return (st, Err(e));
            }
        }
        if bwr.read_size > 0 {
            let (guard, r) = self.thread_read(st, proc, tid, &mut bwr, guest);
            st = guard;
            if !st.procs.get(&proc).is_none_or(|p| p.todo.is_empty()) {
                st.wakeup_proc(proc);
            }
            result = r;
        }
        arg.copy_from_slice(&bwr.encode());
        (st, result)
    }

    /// `binder_thread_read`, including the wait for work.
    fn thread_read<'a>(
        &'a self,
        mut st: MutexGuard<'a, State>,
        proc: ProcId,
        tid: Tid,
        bwr: &mut WriteRead,
        guest: &mut dyn GuestProcess,
    ) -> (MutexGuard<'a, State>, Result<(), Errno>) {
        let start = bwr.read_consumed as usize;
        let capacity = bwr.read_size as usize;
        if start > capacity {
            return (st, Err(errno::EFAULT));
        }
        let mut out = Vec::new();
        if start == 0 {
            out.extend_from_slice(&BR_NOOP.to_le_bytes());
        }
        loop {
            let wait_for_proc_work = st.available_for_proc_work(proc, tid);
            let nonblocking = st.procs.get(&proc).is_some_and(|p| p.nonblocking);
            match st.thread(proc, tid) {
                Some(t) => t.looper |= LOOPER_WAITING,
                None => return (st, Err(errno::EBADF)),
            }
            // binder_wait_for_work
            loop {
                if st.has_work(proc, tid, wait_for_proc_work) {
                    break;
                }
                let Some(t) = st.thread(proc, tid) else { break };
                if nonblocking {
                    t.looper &= !LOOPER_WAITING;
                    return (st, Err(errno::EAGAIN));
                }
                if std::mem::take(&mut t.interrupted) {
                    t.looper &= !LOOPER_WAITING;
                    return (st, Err(errno::EINTR));
                }
                let wait = t.wait.clone();
                // Deliver the wakes this ioctl caused before sleeping, then
                // look for work again: it may have arrived meanwhile.
                let pending = st.take_wakes();
                if !pending.is_empty() {
                    drop(st);
                    pending.deliver();
                    st = self.lock();
                    continue;
                }
                if wait_for_proc_work {
                    st.procs
                        .get_mut(&proc)
                        .unwrap()
                        .waiting_threads
                        .push_back(tid);
                }
                st = wait.wait(st).unwrap_or_else(|e| e.into_inner());
                if let Some(p) = st.procs.get_mut(&proc) {
                    p.waiting_threads.retain(|t| *t != tid);
                }
            }
            match st.thread(proc, tid) {
                Some(t) => t.looper &= !LOOPER_WAITING,
                None => return (st, Err(errno::EBADF)),
            }
            match st.read_pass(
                proc,
                tid,
                wait_for_proc_work,
                &mut out,
                start,
                capacity,
                guest,
            ) {
                Pass::Retry => continue,
                Pass::Done => break,
            }
        }
        let spawn = st.should_spawn_looper(proc, tid);
        let mut result = guest.copy_to_user(bwr.read_buffer + start as u64, &out);
        if result.is_ok() && spawn {
            result = guest.copy_to_user(bwr.read_buffer, &BR_SPAWN_LOOPER.to_le_bytes());
        }
        bwr.read_consumed = (start + out.len()) as u64;
        (st, result)
    }
}

impl State {
    /// `binder_ioctl_set_ctx_mgr`.
    fn set_context_manager(
        &mut self,
        proc: ProcId,
        fbo: Option<FlatBinderObject>,
    ) -> Result<(), Errno> {
        let context = self.procs[&proc].context;
        if self.contexts[context].mgr_node.is_some() {
            return Err(errno::EBUSY);
        }
        let euid = self.procs[&proc].creds.euid;
        match self.contexts[context].mgr_uid {
            Some(uid) if uid != euid => return Err(errno::EPERM),
            Some(_) => {}
            None => self.contexts[context].mgr_uid = Some(euid),
        }
        let fbo = fbo.unwrap_or_default();
        let node = self.new_node(proc, fbo.binder, fbo.cookie, fbo.flags);
        let n = self.nodes.get_mut(&node).unwrap();
        n.local_weak_refs += 1;
        n.local_strong_refs += 1;
        n.has_strong_ref = true;
        n.has_weak_ref = true;
        self.contexts[context].mgr_node = Some(node);
        Ok(())
    }

    /// `binder_ioctl_get_node_info_for_ref`: context manager only.
    fn node_info_for_ref(&mut self, proc: ProcId, arg: &mut [u8]) -> Result<(), Errno> {
        let handle = u32_at(arg, 0);
        if arg[4..].iter().any(|b| *b != 0) {
            return Err(errno::EINVAL);
        }
        let context = self.procs[&proc].context;
        let is_mgr = self.contexts[context]
            .mgr_node
            .is_some_and(|n| self.nodes[&n].proc == Some(proc));
        if !is_mgr {
            return Err(errno::EPERM);
        }
        let node = self.get_ref(proc, handle, true).ok_or(errno::EINVAL)?.node;
        let n = &self.nodes[&node];
        put_u32(arg, 4, n.local_strong_refs + n.internal_strong_refs);
        put_u32(arg, 8, n.local_weak_refs);
        Ok(())
    }

    /// `binder_ioctl_get_node_debug_info`: the first node above `ptr`.
    fn node_debug_info(&mut self, proc: ProcId, arg: &mut [u8]) {
        let ptr = u64_at(arg, 0);
        arg.fill(0);
        let p = &self.procs[&proc];
        if let Some((_, &id)) = p
            .nodes
            .range((std::ops::Bound::Excluded(ptr), std::ops::Bound::Unbounded))
            .next()
        {
            let n = &self.nodes[&id];
            put_u64(arg, 0, n.ptr);
            put_u64(arg, 8, n.cookie);
            put_u32(arg, 16, n.has_strong_ref as u32);
            put_u32(arg, 20, n.has_weak_ref as u32);
        }
    }
}
