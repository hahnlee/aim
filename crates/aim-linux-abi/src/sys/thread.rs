//! Guest threads: `clone` for threads, thread exit, tids, and the per-thread
//! records other threads reach (signals, futex wakes, timers, scheduling).
//!
//! Each guest thread is a Darwin pthread (ADR 0012, "Platform probes"). Its
//! host stack is the pthread's own; the child resumes the parent's copied
//! registers on the guest stack through the trampoline's exit path, so it
//! starts exactly as Linux starts a clone child and takes pending signals
//! first.
//!
//! Tids: the main thread's is the pid. Others are `TID_BASE + (pid << 12) +
//! n` (n in 1..4096): unique across processes and never a Darwin pid (those
//! stay below 100000), and below 2^30 as `FUTEX_TID_MASK` requires. When
//! aimd owns the pid space (ADR 0012, section 7) it assigns them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use super::park::Parker;
use super::signal::ThreadSignals;
use crate::context::{self, GuestContext, HostStacks};
use crate::errno::{EAGAIN, EINVAL, ENOSYS, ESRCH};

/// A clone's tid is TID_BASE + (pid << TID_SHIFT) + n, above every host
/// pid (at most 99999) and, like a Linux tid, below 2^28, the most a CPU
/// clock id (`~tid << 3`) can encode.
const TID_BASE: i32 = 1 << 17;
const TID_SHIFT: u32 = 11;
const TIDS_PER_PROCESS: u32 = (1 << TID_SHIFT) - 1;
/// Host stack of a clone thread (the syscall layer runs on it).
const HOST_STACK: usize = 512 << 10;

pub const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_PIDFD: u64 = 0x1000;
const CLONE_PTRACE: u64 = 0x2000;
const CLONE_VFORK: u64 = 0x4000;
const CLONE_PARENT: u64 = 0x8000;
pub const CLONE_THREAD: u64 = 0x1_0000;
const CLONE_SYSVSEM: u64 = 0x4_0000;
const CLONE_SETTLS: u64 = 0x8_0000;
const CLONE_PARENT_SETTID: u64 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
const CLONE_DETACHED: u64 = 0x40_0000;
const CLONE_UNTRACED: u64 = 0x80_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;
const CLONE_IO: u64 = 0x8000_0000;
/// What a thread clone may carry besides the exit-signal byte.
const THREAD_FLAGS: u64 = CLONE_VM
    | CLONE_FS
    | CLONE_FILES
    | CLONE_SIGHAND
    | CLONE_THREAD
    | CLONE_SYSVSEM
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_DETACHED
    | CLONE_UNTRACED
    | CLONE_CHILD_SETTID
    | CLONE_IO
    | CLONE_PTRACE
    | CLONE_PARENT;

/// Scheduling attributes Linux keeps per thread.
pub struct Sched {
    pub nice: AtomicI32,
    pub policy: AtomicI32,
    pub priority: AtomicI32,
}

pub struct Thread {
    pub tid: i32,
    /// The host pthread_t; 0 until the thread runs.
    pthread: AtomicUsize,
    ctx: *mut GuestContext,
    pub sig: ThreadSignals,
    pub park: Parker,
    clear_child_tid: AtomicU64,
    robust_list: AtomicU64,
    /// `comm`, as PR_SET_NAME sets it.
    pub name: Mutex<[u8; 16]>,
    pub sched: Sched,
    stacks: Mutex<Option<HostStacks>>,
}

// SAFETY: `ctx` is only dereferenced by the owning thread (and read-only
// through its atomics by others); everything else is synchronized.
unsafe impl Send for Thread {}
// SAFETY: as above.
unsafe impl Sync for Thread {}

impl Thread {
    fn new(tid: i32, ctx: *mut GuestContext, mask: u64) -> Self {
        Thread {
            tid,
            pthread: AtomicUsize::new(0),
            ctx,
            sig: ThreadSignals::new(mask),
            park: Parker::default(),
            clear_child_tid: AtomicU64::new(0),
            robust_list: AtomicU64::new(0),
            name: Mutex::new([0; 16]),
            sched: Sched {
                nice: AtomicI32::new(0),
                policy: AtomicI32::new(0),
                priority: AtomicI32::new(0),
            },
            stacks: Mutex::new(None),
        }
    }

    /// Raise a host signal on this thread. The caller holds the table lock,
    /// so the host thread cannot finish exiting meanwhile.
    pub fn raise_host_locked(&self, sig: i32) {
        let p = self.pthread.load(SeqCst);
        if p != 0 {
            // SAFETY: the pthread is alive while it is in the table.
            unsafe { libc::pthread_kill(p as libc::pthread_t, sig) };
        }
    }
}

/// Lookups and signal sends share it; a sender holds it across the host
/// pthread_kill, so an exclusive lock let a signal flood starve the
/// target's own lookups (sched_getscheduler) for good (#219).
static THREADS: LazyLock<RwLock<HashMap<i32, Arc<Thread>>>> = LazyLock::new(Default::default);
static NEXT_TID: AtomicU32 = AtomicU32::new(0);
/// The main thread's exit code once it has called `exit` (the process
/// exits with it when the last thread does), else -1.
static LEADER_EXIT: AtomicI32 = AtomicI32::new(-1);
/// For host signals that land on a non-guest thread: the main thread's
/// pthread and record, reachable without locks.
static MAIN_PTHREAD: AtomicUsize = AtomicUsize::new(0);
static MAIN_THREAD: AtomicUsize = AtomicUsize::new(0);

pub fn with_table<R>(f: impl FnOnce(&HashMap<i32, Arc<Thread>>) -> R) -> R {
    f(&THREADS.read().unwrap_or_else(|e| e.into_inner()))
}

fn with_table_mut<R>(f: impl FnOnce(&mut HashMap<i32, Arc<Thread>>) -> R) -> R {
    f(&mut THREADS.write().unwrap_or_else(|e| e.into_inner()))
}

pub fn find(tid: i32) -> Option<Arc<Thread>> {
    with_table(|t| t.get(&tid).cloned())
}

/// User and system CPU time of thread `tid` of this process, in
/// nanoseconds; none yet for a thread whose host thread has not started.
pub fn cpu_times(tid: i32) -> Option<(u64, u64)> {
    with_table(|t| {
        let p = t.get(&tid)?.pthread.load(SeqCst);
        if p == 0 {
            return Some((0, 0));
        }
        // SAFETY: the pthread is alive while it is in the table (whose lock
        // is held).
        super::clock::thread_times(unsafe { libc::pthread_mach_thread_np(p as libc::pthread_t) })
    })
}

/// Call `f` with the Mach port of every other guest thread that is
/// running (on a core or waiting for one), under the table lock, so none
/// can finish exiting meanwhile. A thread whose state cannot be read
/// counts as running.
pub fn each_running_other(mut f: impl FnMut(libc::mach_port_t)) {
    with_table(|t| {
        for th in t.values() {
            let p = th.pthread.load(SeqCst);
            if p == 0 || is_current(th) {
                continue;
            }
            // SAFETY: the pthread is alive while it is in the table (whose
            // lock is held); THREAD_BASIC_INFO into a local of its size.
            let (port, running) = unsafe {
                let port = libc::pthread_mach_thread_np(p as libc::pthread_t);
                let mut info: libc::thread_basic_info = std::mem::zeroed();
                let mut count = libc::THREAD_BASIC_INFO_COUNT;
                let kr = libc::thread_info(
                    port,
                    libc::THREAD_BASIC_INFO as u32,
                    (&mut info as *mut libc::thread_basic_info).cast(),
                    &mut count,
                );
                (port, kr != 0 || info.run_state == libc::TH_STATE_RUNNING)
            };
            if running {
                f(port);
            }
        }
    });
}

pub fn unpark(tid: i32) {
    if let Some(th) = find(tid) {
        th.park.unpark();
    }
}

fn pid() -> i32 {
    // SAFETY: trivial.
    unsafe { libc::getpid() }
}

pub fn main_tid() -> i32 {
    pid()
}

/// The calling thread's record, if it runs guest code.
pub fn current() -> Option<&'static Thread> {
    let ctx = context::current_ctx();
    // SAFETY: a live context points at its thread, which outlives it.
    unsafe { ctx.as_ref().and_then(|c| c.thread.as_ref()) }
}

pub fn is_current(th: &Thread) -> bool {
    current().is_some_and(|c| std::ptr::eq(c, th))
}

/// Linux tid of the calling thread (the pid before it runs guest code).
pub fn host_tid() -> i64 {
    current().map_or(pid(), |t| t.tid) as i64
}

pub fn gettid() -> i64 {
    host_tid()
}

/// Set the calling thread's `comm` (PR_SET_NAME).
pub fn set_name(name: [u8; 16]) {
    if let Some(t) = current() {
        *t.name.lock().unwrap_or_else(|e| e.into_inner()) = name;
    }
}

/// The `comm` of this process's thread `tid`.
pub fn name_of(tid: i32) -> Option<[u8; 16]> {
    find(tid).map(|t| *t.name.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Set the `comm` of this process's thread `tid` (a write to its
/// `/proc/<pid>/task/<tid>/comm`). Darwin names only the calling host
/// thread, so another thread's host name stays.
pub fn set_name_of(tid: i32, name: [u8; 16]) -> bool {
    let Some(t) = find(tid) else {
        return false;
    };
    *t.name.lock().unwrap_or_else(|e| e.into_inner()) = name;
    if is_current(&t) {
        let len = name.iter().position(|&b| b == 0).unwrap_or(15);
        if let Ok(c) = std::ffi::CString::new(&name[..len]) {
            // SAFETY: naming the calling thread.
            unsafe { libc::pthread_setname_np(c.as_ptr()) };
        }
    }
    true
}

/// The process a tid belongs to (a pid is its own main thread's tid).
pub fn owner(tid: i32) -> i32 {
    if tid >= TID_BASE {
        (tid - TID_BASE) >> TID_SHIFT
    } else {
        tid
    }
}

/// Every tid of this process, in order.
pub fn tids() -> Vec<i32> {
    let mut v: Vec<i32> = with_table(|t| t.keys().copied().collect());
    v.sort_unstable();
    v
}

pub fn name() -> [u8; 16] {
    current().map_or([0; 16], |t| {
        *t.name.lock().unwrap_or_else(|e| e.into_inner())
    })
}

/// Poke the main thread from a host signal handler on a thread that runs
/// no guest code (no locks: the handler may have interrupted their owner).
pub fn poke_main_from_handler(sig: i32) {
    let main = MAIN_THREAD.load(SeqCst) as *const Thread;
    // SAFETY: the main thread's record is never freed (see `exit`).
    if let Some(th) = unsafe { main.as_ref() } {
        th.sig.attn.store(1, SeqCst);
        th.park.unpark();
    }
    let p = MAIN_PTHREAD.load(SeqCst);
    if p != 0 {
        // SAFETY: Darwin keeps the main thread's pthread_t valid.
        unsafe { libc::pthread_kill(p as libc::pthread_t, sig) };
    }
}

fn alloc_tid(t: &HashMap<i32, Arc<Thread>>) -> Option<i32> {
    let base = TID_BASE + (pid() << TID_SHIFT);
    (0..TIDS_PER_PROCESS)
        .map(|_| base + (NEXT_TID.fetch_add(1, SeqCst) % TIDS_PER_PROCESS) as i32 + 1)
        .find(|tid| !t.contains_key(tid))
}

/// Make the calling host thread a guest thread with context `ctx` (which
/// has no thread yet): the first becomes the main thread (tid = pid).
pub fn register_current(ctx: *mut GuestContext, stacks: HostStacks) {
    let th = with_table_mut(|t| {
        let tid = if t.contains_key(&pid()) {
            alloc_tid(t).expect("no free tid")
        } else {
            pid()
        };
        let th = Arc::new(Thread::new(tid, ctx, 0));
        t.insert(tid, th.clone());
        th
    });
    *th.stacks.lock().unwrap() = Some(stacks);
    // SAFETY: pthread_self is always valid.
    th.pthread
        .store(unsafe { libc::pthread_self() } as usize, SeqCst);
    if th.tid == pid() {
        MAIN_PTHREAD.store(th.pthread.load(SeqCst), SeqCst);
        MAIN_THREAD.store(Arc::as_ptr(&th) as usize, SeqCst);
    }
    // SAFETY: `ctx` is the caller's fresh context.
    unsafe {
        (*ctx).tid = th.tid as u64;
        (*ctx).attn = &th.sig.attn;
        (*ctx).thread = Arc::into_raw(th);
    }
}

// ---- clone ----------------------------------------------------------------------

struct Boot {
    thread: Arc<Thread>,
    tp: u64,
}

/// clone(flags, newsp, parent_tid, tls, child_tid) for threads.
pub fn clone(ctx: &GuestContext, a: [u64; 6]) -> i64 {
    spawn(ctx, a[0], a[1], a[2], a[3], a[4])
}

/// clone3(args, size) for threads.
pub fn clone3(ctx: &GuestContext, a: [u64; 6]) -> i64 {
    if a[1] < 64 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest struct clone_args (at least the version-0 64 bytes).
    let c = unsafe { (a[0] as *const [u64; 8]).read_unaligned() };
    let [
        flags,
        pidfd,
        child_tid,
        parent_tid,
        exit_signal,
        stack,
        stack_size,
        tls,
    ] = c;
    if flags & CLONE_THREAD != 0 && (pidfd != 0 || exit_signal != 0) {
        return -(EINVAL as i64);
    }
    let sp = if stack == 0 { 0 } else { stack + stack_size };
    spawn(ctx, flags | exit_signal, sp, parent_tid, tls, child_tid)
}

fn spawn(ctx: &GuestContext, flags: u64, newsp: u64, ptid: u64, tls: u64, ctid: u64) -> i64 {
    if flags & CLONE_THREAD == 0 {
        crate::diag!(
            "[linux-abi] clone({flags:#x}) of a process is not implemented; only threads are"
        );
        return -(ENOSYS as i64);
    }
    // Linux: THREAD needs SIGHAND, which needs VM.
    if flags & CLONE_SIGHAND == 0 || flags & CLONE_VM == 0 || flags & !(THREAD_FLAGS | 0xff) != 0 {
        return -(EINVAL as i64);
    }
    if flags & (CLONE_VFORK | CLONE_PIDFD) != 0 {
        return -(EINVAL as i64);
    }
    let Some(parent) = current() else {
        return -(EINVAL as i64);
    };
    let child_ctx = Box::into_raw(context::copy_regs(ctx));
    // SAFETY: fresh context owned here until the child starts.
    let c = unsafe { &mut *child_ctx };
    c.x[0] = 0;
    if newsp != 0 {
        c.sp = newsp;
    }
    let th = with_table_mut(|t| {
        let tid = alloc_tid(t)?;
        let th = Arc::new(Thread::new(tid, child_ctx, parent.sig.mask()));
        *th.name.lock().unwrap() = *parent.name.lock().unwrap();
        for (d, s) in [
            (&th.sched.nice, &parent.sched.nice),
            (&th.sched.policy, &parent.sched.policy),
            (&th.sched.priority, &parent.sched.priority),
        ] {
            d.store(s.load(SeqCst), SeqCst);
        }
        if flags & CLONE_CHILD_CLEARTID != 0 {
            th.clear_child_tid.store(ctid, SeqCst);
        }
        t.insert(tid, th.clone());
        Some(th)
    });
    let Some(th) = th else {
        // SAFETY: never handed out.
        drop(unsafe { Box::from_raw(child_ctx) });
        return -(EAGAIN as i64);
    };
    let tid = th.tid;
    c.tid = tid as u64;
    c.attn = &th.sig.attn;
    c.thread = Arc::into_raw(th.clone());
    // Linux writes both before the child can run.
    // SAFETY: guest tid words.
    unsafe {
        if flags & CLONE_PARENT_SETTID != 0 {
            (ptid as *mut i32).write_volatile(tid);
        }
        if flags & CLONE_CHILD_SETTID != 0 {
            (ctid as *mut i32).write_volatile(tid);
        }
    }
    let tp = if flags & CLONE_SETTLS != 0 {
        tls
    } else {
        context::guest_tp()
    };
    let boot = Box::into_raw(Box::new(Boot {
        thread: th.clone(),
        tp,
    }));
    // The child may run, exit and be freed before pthread_create returns:
    // nothing of it is touched afterwards (ADR 0012, "Platform probes").
    let ok = {
        // SAFETY: standard pthread creation with a detached attribute.
        unsafe {
            let mut attr: libc::pthread_attr_t = std::mem::zeroed();
            libc::pthread_attr_init(&mut attr);
            libc::pthread_attr_setstacksize(&mut attr, HOST_STACK);
            libc::pthread_attr_setdetachstate(&mut attr, libc::PTHREAD_CREATE_DETACHED);
            let mut pt: libc::pthread_t = std::mem::zeroed();
            let r = libc::pthread_create(&mut pt, &attr, thread_start, boot as *mut _);
            libc::pthread_attr_destroy(&mut attr);
            r == 0
        }
    };
    if !ok {
        with_table_mut(|t| t.remove(&tid));
        // SAFETY: the child never started; reclaim what it would own.
        unsafe {
            drop(Box::from_raw(boot));
            drop(Arc::from_raw(c.thread));
            drop(Box::from_raw(child_ctx));
        }
        return -(EAGAIN as i64);
    }
    tid as i64
}

extern "C" fn thread_start(arg: *mut libc::c_void) -> *mut libc::c_void {
    // SAFETY: `arg` is the Boot handed over by `spawn`.
    let boot = unsafe { Box::from_raw(arg as *mut Boot) };
    let th = &boot.thread;
    let ctx = th.ctx;
    let anchor = 0u8;
    let host_sp = (&anchor as *const u8 as u64 - 256) & !15;
    // SAFETY: the child's own context, now bound to this host thread.
    unsafe {
        (*ctx).host_sp = host_sp;
        *th.stacks.lock().unwrap() = Some(context::bind(ctx, boot.tp));
        th.pthread.store(libc::pthread_self() as usize, SeqCst);
    }
    drop(boot);
    // SAFETY: the context holds the child's registers.
    unsafe { context::resume(ctx) }
}

// ---- exit -----------------------------------------------------------------------

/// exit(code): end the calling thread. The process ends with the last one
/// (with the main thread's code if it exited earlier, as Linux reports).
pub fn exit(a: [u64; 6]) -> ! {
    let code = a[0] as i32 & 0xff;
    let Some(th) = current() else { exit_group(a) };
    // No host signal handler may run on this thread from here on.
    // SAFETY: blocking signals for the calling thread.
    unsafe {
        let mut all: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }
    // Now off the guest stack: its deferred munmap (bionic's
    // _exit_with_stack_teardown) can run.
    super::run_deferred_unmaps();
    super::futex::exit_robust_list(th.robust_list.load(SeqCst), th.tid);
    let main = th.tid == pid();
    // Out of the table before a joiner wakes: its tid is gone for tgkill.
    let left = with_table_mut(|t| {
        t.remove(&th.tid);
        t.len()
    });
    let ctid = th.clear_child_tid.load(SeqCst);
    if ctid != 0 {
        // SAFETY: the guest tid word registered with CLONE_CHILD_CLEARTID or
        // set_tid_address.
        unsafe { (ctid as *mut u32).write_volatile(0) };
        super::futex::wake_one(ctid);
    }
    if main {
        LEADER_EXIT.store(code, SeqCst);
    }
    if left == 0 {
        let leader = LEADER_EXIT.load(SeqCst);
        end_process(if leader >= 0 { leader } else { code });
    }
    super::signal::reroute_process_pending();
    let ctx = th.ctx;
    let stacks = th.stacks.lock().unwrap().take();
    // SAFETY: nothing runs on this thread's context, stacks or record after
    // this; the main thread's record stays (host handlers may reach it).
    unsafe {
        context::unbind(stacks);
        if !main {
            drop(Arc::from_raw((*ctx).thread));
            drop(Box::from_raw(ctx));
        }
        libc::pthread_exit(std::ptr::null_mut())
    }
}

pub fn exit_group(a: [u64; 6]) -> ! {
    end_process(a[0] as i32 & 0xff)
}

fn end_process(code: i32) -> ! {
    super::fork::spawn::wait_handovers();
    // A parent in the namespace reaps this process and drops its entry
    // then: until then it is a zombie there, whose credentials SIGCHLD
    // and waitid report.
    // SAFETY: trivial.
    if !super::pidns::contains(unsafe { libc::getppid() }) {
        super::cred::forget(pid());
    }
    super::pidns::leave();
    // SAFETY: ending the process without host atexit handlers, as Linux's
    // exit_group.
    unsafe { libc::_exit(code) }
}

pub fn set_tid_address(a: [u64; 6]) -> i64 {
    match current() {
        Some(th) => {
            th.clear_child_tid.store(a[0], SeqCst);
            th.tid as i64
        }
        None => gettid(),
    }
}

/// set_robust_list(head, len): the list is walked when the thread exits.
pub fn set_robust_list(a: [u64; 6]) -> i64 {
    if a[1] != 24 {
        return -(EINVAL as i64);
    }
    if let Some(th) = current() {
        th.robust_list.store(a[0], SeqCst);
    }
    0
}

/// get_robust_list(tid, head_ptr, len_ptr).
pub fn get_robust_list(a: [u64; 6]) -> i64 {
    let th = if a[0] == 0 {
        current().and_then(|c| find(c.tid))
    } else {
        find(a[0] as i32)
    };
    let Some(th) = th else {
        return -(ESRCH as i64);
    };
    // SAFETY: guest out-pointers.
    unsafe {
        (a[1] as *mut u64).write_unaligned(th.robust_list.load(SeqCst));
        (a[2] as *mut u64).write_unaligned(24);
    }
    0
}

/// Whether the calling thread is the process's only guest thread, its
/// main thread.
pub fn alone() -> bool {
    with_table(|t| t.len() == 1) && current().is_some_and(|th| th.tid == pid())
}

/// execve in place, on the process's only thread: no clear_child_tid or
/// robust list, the `comm` of a new process, no thread pointer and an
/// empty shadow call stack.
pub(super) fn exec_reset() {
    let th = current().expect("exec from a thread without a guest context");
    th.clear_child_tid.store(0, SeqCst);
    th.robust_list.store(0, SeqCst);
    *th.name.lock().unwrap_or_else(|e| e.into_inner()) = [0; 16];
    context::set_guest_tp(0);
    if let Some(st) = th.stacks.lock().unwrap().as_ref() {
        context::set_guest_scs(st.scs());
    }
}

// ---- fork -------------------------------------------------------------------------

/// Fork: the forking thread's name and scheduling attributes, which the
/// child's main thread takes.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let th = current().expect("fork from a thread without a guest context");
    w.bytes(&*th.name.lock().unwrap_or_else(|e| e.into_inner()));
    for v in [&th.sched.nice, &th.sched.policy, &th.sched.priority] {
        w.i32(v.load(SeqCst));
    }
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let th = current().expect("fork child without a guest context");
    let mut name = [0u8; 16];
    let saved = r.bytes();
    let n = saved.len().min(16);
    name[..n].copy_from_slice(&saved[..n]);
    set_name_of(th.tid, name);
    for v in [&th.sched.nice, &th.sched.policy, &th.sched.priority] {
        v.store(r.i32(), SeqCst);
    }
}
