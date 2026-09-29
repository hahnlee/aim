//! Linux signals on Darwin.
//!
//! Guest dispositions, masks, pending sets and alternate stacks live in the
//! layer; the host signal mask never follows the guest's. Linux frames
//! (`sigframe`) are built only where the guest register state is complete:
//! - in guest code, from a host signal handler: synchronous faults, and the
//!   carrier signal that pokes a thread for an asynchronous signal;
//! - at syscall exit, from the saved `GuestContext`: the trampoline calls
//!   [`linux_abi_deliver`] while the thread's attention flag is set.
//!
//! Anywhere else (the trampoline, host code) a poke only leaves the
//! attention flag set, and the next exit check delivers. Blocking syscalls
//! of the layer re-check it through the thread's parker; a blocking Darwin
//! syscall returns EINTR and is restarted when no handler runs (or when the
//! handler has SA_RESTART), as Linux restarts it.
//!
//! - Asynchronous guest signals are queued per thread or per process. The
//!   target's flag is set, its parker unparked and the carrier (Darwin
//!   SIGEMT, which Linux lacks) raised on its host thread. This also gives
//!   Linux's real-time signals 32..64, which Darwin does not have.
//! - Host signals from outside (another process, a terminal) are forwarded
//!   as process-directed guest signals.
//! - `rt_sigreturn` loads the frame into the context and resumes through a
//!   `udf` trap whose SIGILL handler sets every register: on arm64 there is
//!   no other exact way back to an arbitrary pc (ADR 0012, "Platform probes").

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed, Ordering::SeqCst};

use super::sigframe::{
    self, AltStack, Cpu, DarwinMcontext, Entry, KSigaction, MINSIGSTKSZ, SIG_DFL, SIG_IGN,
    SS_AUTODISARM, SS_DISABLE, SS_ONSTACK, Siginfo,
};
use super::thread::{self, Thread};
use super::{park, ptimer, window};
use crate::context::{self, GuestContext};
use crate::errno::{EAGAIN, EINTR, EINVAL, ENOMEM, EPERM, ESRCH};

const NSIG: i32 = 64;
const SIGKILL: i32 = 9;
const SIGSTOP: i32 = 19;
const SIGCONT: i32 = 18;
const UNBLOCKABLE: u64 = bit(SIGKILL) | bit(SIGSTOP);
/// Delivered before other pending signals, as Linux's `dequeue_synchronous_signal`.
const SYNCHRONOUS: u64 = bit(4) | bit(5) | bit(7) | bit(8) | bit(11) | bit(31);

const SA_NOCLDSTOP: u64 = 1;
const SA_NOCLDWAIT: u64 = 2;
const SA_RESTART: u64 = 0x1000_0000;
const SA_NODEFER: u64 = 0x4000_0000;
const SA_RESETHAND: u64 = 0x8000_0000;

/// The host signal that pokes a guest thread.
const CARRIER: i32 = libc::SIGEMT;
/// Host signals the kernel raises on the thread whose syscall caused them.
const THREAD_DIRECTED: u64 = bit(13) | bit(25); // SIGPIPE, SIGXFSZ
/// Queued signals per queue; beyond it sigqueue fails with EAGAIN.
const MAX_QUEUED: usize = 4096;

const fn bit(sig: i32) -> u64 {
    1 << (sig - 1)
}

fn lowest(set: u64) -> i32 {
    set.trailing_zeros() as i32 + 1
}

/// Linux -> Darwin signal numbers (0 when Darwin has no equivalent).
pub fn to_host(sig: i32) -> i32 {
    match sig {
        1 => libc::SIGHUP,
        2 => libc::SIGINT,
        3 => libc::SIGQUIT,
        4 => libc::SIGILL,
        5 => libc::SIGTRAP,
        6 => libc::SIGABRT,
        7 => libc::SIGBUS,
        8 => libc::SIGFPE,
        9 => libc::SIGKILL,
        10 => libc::SIGUSR1,
        11 => libc::SIGSEGV,
        12 => libc::SIGUSR2,
        13 => libc::SIGPIPE,
        14 => libc::SIGALRM,
        15 => libc::SIGTERM,
        17 => libc::SIGCHLD,
        18 => libc::SIGCONT,
        19 => libc::SIGSTOP,
        20 => libc::SIGTSTP,
        21 => libc::SIGTTIN,
        22 => libc::SIGTTOU,
        23 => libc::SIGURG,
        24 => libc::SIGXCPU,
        25 => libc::SIGXFSZ,
        26 => libc::SIGVTALRM,
        27 => libc::SIGPROF,
        28 => libc::SIGWINCH,
        29 => libc::SIGIO,
        31 => libc::SIGSYS,
        _ => 0,
    }
}

/// Darwin -> Linux signal numbers (0 for SIGEMT and SIGINFO).
pub fn from_host(h: i32) -> i32 {
    (1..32).find(|&s| to_host(s) == h).unwrap_or(0)
}

enum Default {
    Ignore,
    Stop,
    Die,
}

fn default_action(sig: i32) -> Default {
    match sig {
        17 | 18 | 23 | 28 => Default::Ignore,
        19..=22 => Default::Stop,
        _ => Default::Die,
    }
}

fn ignored(sig: i32, act: &KSigaction) -> bool {
    act.handler == SIG_IGN
        || act.handler == SIG_DFL && matches!(default_action(sig), Default::Ignore)
}

/// The guest's default action for a fatal signal: the process dies of the
/// Darwin equivalent (so its parent sees the signal), or exits 128+sig.
pub fn die(sig: i32) -> ! {
    super::fork::spawn::wait_handovers();
    super::pidns::leave();
    let h = to_host(sig);
    // SAFETY: restoring the default action and raising it on this thread.
    unsafe {
        if h != 0 {
            libc::signal(h, libc::SIG_DFL);
            let mut set: libc::sigset_t = 0;
            libc::sigaddset(&mut set, h);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::pthread_kill(libc::pthread_self(), h);
        }
        libc::_exit(128 + sig)
    }
}

static ACTIONS: Mutex<[KSigaction; NSIG as usize]> = Mutex::new(
    [KSigaction {
        handler: 0,
        flags: 0,
        restorer: 0,
        mask: 0,
    }; NSIG as usize],
);

fn action(sig: i32) -> KSigaction {
    ACTIONS.lock().unwrap_or_else(|e| e.into_inner())[sig as usize - 1]
}

/// Pending signals: standard signals coalesce, real-time ones queue.
struct Pending {
    set: u64,
    queue: Vec<Siginfo>,
}

impl Pending {
    const fn new() -> Self {
        Pending {
            set: 0,
            queue: Vec::new(),
        }
    }

    /// Ok(false) when a standard signal was already pending.
    fn push(&mut self, info: Siginfo) -> Result<bool, i64> {
        let b = bit(info.signo);
        if info.signo < 32 && self.set & b != 0 {
            return Ok(false);
        }
        if self.queue.len() >= MAX_QUEUED {
            return Err(-(EAGAIN as i64));
        }
        self.queue.push(info);
        self.set |= b;
        Ok(true)
    }

    fn take(&mut self, sig: i32) -> Option<Siginfo> {
        let i = self.queue.iter().position(|q| q.signo == sig)?;
        let mut info = self.queue.remove(i);
        if !self.queue.iter().any(|q| q.signo == sig) {
            self.set &= !bit(sig);
        }
        if info.code == ptimer::SI_TIMER {
            ptimer::dequeued(&mut info);
        }
        Some(info)
    }

    fn flush(&mut self, sig: i32) {
        self.queue.retain_mut(|q| {
            if q.signo == sig && q.code == ptimer::SI_TIMER {
                ptimer::dequeued(q);
            }
            q.signo != sig
        });
        self.set &= !bit(sig);
    }
}

static PROCESS: Mutex<Pending> = Mutex::new(Pending::new());

fn process() -> std::sync::MutexGuard<'static, Pending> {
    PROCESS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A signal taken off a queue whose handler is to run.
struct Taken {
    info: Siginfo,
    act: KSigaction,
}

/// Per-thread signal state. `attn` is read by the trampoline; the rest is
/// written by the owner only, except `pending` (by senders).
pub struct ThreadSignals {
    pub attn: AtomicU32,
    mask: AtomicU64,
    /// Host signals raised by this thread's own syscalls, recorded without
    /// locks by the handler and queued by [`take`].
    own: AtomicU64,
    /// Signals `rt_sigtimedwait` is waiting for: routable here although
    /// blocked, as Linux's `real_blocked`.
    waitset: AtomicU64,
    pending: Mutex<Pending>,
    alt: Mutex<AltStack>,
    /// `rt_sigsuspend`'s mask to restore after the handler.
    saved_mask: Mutex<Option<u64>>,
    /// A signal taken by a blocking syscall that returns EINTR for it.
    stash: Mutex<Option<Taken>>,
    /// The syscall that just returned (nr + 1, bit 63: timed wait) and its
    /// first argument, for SA_RESTART.
    restart_nr: AtomicU64,
    restart_a0: AtomicU64,
    /// After `rt_sigreturn`: the context must be resumed exactly.
    exact: AtomicBool,
    /// Set while the thread blocks in [`interruptible`].
    waker: Mutex<Option<Waker>>,
}

/// A borrowed `dyn Fn` with its lifetime erased; [`interruptible`] clears it
/// before the closure goes away.
struct Waker(*const (dyn Fn() + Sync));

// SAFETY: the closure is Sync and outlives its registration.
unsafe impl Send for Waker {}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ThreadSignals {
    pub fn new(mask: u64) -> Self {
        ThreadSignals {
            attn: AtomicU32::new(0),
            mask: AtomicU64::new(mask),
            own: AtomicU64::new(0),
            waitset: AtomicU64::new(0),
            pending: Mutex::new(Pending::new()),
            alt: Mutex::new(AltStack::DISABLED),
            saved_mask: Mutex::new(None),
            stash: Mutex::new(None),
            restart_nr: AtomicU64::new(0),
            restart_a0: AtomicU64::new(0),
            exact: AtomicBool::new(false),
            waker: Mutex::new(None),
        }
    }

    pub fn mask(&self) -> u64 {
        self.mask.load(Relaxed)
    }

    fn eligible(&self, sig: i32) -> bool {
        self.mask.load(SeqCst) & bit(sig) == 0 || self.waitset.load(SeqCst) & bit(sig) != 0
    }
}

fn thread_of(ctx: &GuestContext) -> Option<&'static Thread> {
    if ctx.thread.is_null() {
        thread::current()
    } else {
        // SAFETY: a live context points at its live thread.
        Some(unsafe { &*ctx.thread })
    }
}

// ---- sending --------------------------------------------------------------

/// Set `th`'s attention flag and interrupt whatever it is doing. The table
/// lock must be held (the host thread cannot finish exiting meanwhile).
fn poke_locked(th: &Thread) {
    th.sig.attn.store(1, SeqCst);
    th.park.unpark();
    if let Some(w) = lock(&th.sig.waker).as_ref() {
        // SAFETY: registered by `interruptible`, which clears it (under this
        // lock) before the closure ends.
        unsafe { (*w.0)() };
    }
    if !thread::is_current(th) {
        th.raise_host_locked(CARRIER);
    }
}

fn host_self(sig: i32) -> i64 {
    // SAFETY: stopping or killing our own process.
    unsafe { libc::kill(libc::getpid(), to_host(sig)) };
    0
}

fn send_thread(th: &Thread, info: Siginfo) -> i64 {
    let sig = info.signo;
    if sig == SIGKILL || sig == SIGSTOP {
        return host_self(sig);
    }
    if let Err(e) = lock(&th.sig.pending).push(info) {
        return e;
    }
    if th.sig.eligible(sig) {
        thread::with_table(|t| {
            if t.contains_key(&th.tid) {
                poke_locked(th)
            }
        });
    }
    0
}

fn send_process(info: Siginfo) -> i64 {
    let sig = info.signo;
    if sig == SIGKILL || sig == SIGSTOP {
        return host_self(sig);
    }
    match process().push(info) {
        Err(e) => e,
        Ok(false) => 0,
        Ok(true) => {
            route(sig);
            0
        }
    }
}

/// Queue a POSIX timer's signal for the process, or for this process's
/// thread `tid`. False when it was not queued (a standard signal already
/// pending, a full queue, the thread gone): the timer counts an overrun
/// instead of waiting for it.
pub fn send_timer(info: Siginfo, tid: Option<i32>) -> bool {
    let sig = info.signo;
    let Some(tid) = tid else {
        let queued = process().push(info) == Ok(true);
        if queued {
            route(sig);
        }
        return queued;
    };
    let Some(th) = thread::find(tid) else {
        return false;
    };
    if lock(&th.sig.pending).push(info) != Ok(true) {
        return false;
    }
    if th.sig.eligible(sig) {
        thread::with_table(|t| {
            if t.contains_key(&th.tid) {
                poke_locked(&th)
            }
        });
    }
    true
}

/// Poke a thread that can take process-directed `sig`: the main thread
/// first, as Linux's `complete_signal`.
fn route(sig: i32) {
    thread::with_table(|t| {
        let main = thread::main_tid();
        let target = t
            .get(&main)
            .filter(|th| th.sig.eligible(sig))
            .or_else(|| t.values().find(|th| th.sig.eligible(sig)));
        if let Some(th) = target {
            poke_locked(th);
        }
    });
}

/// Called by an exiting thread: pending process signals it was poked for
/// go to another thread.
pub fn reroute_process_pending() {
    let set = process().set;
    for sig in 1..=NSIG {
        if set & bit(sig) != 0 {
            route(sig);
        }
    }
}

// ---- signals from outside the process -----------------------------------------

struct External {
    /// The sender's host pid.
    from: AtomicU64,
    /// si_code | status << 32
    code: AtomicU64,
}

static EXTERNAL: AtomicU64 = AtomicU64::new(0);
static EXTERNAL_INFO: [External; 32] = [const {
    External {
        from: AtomicU64::new(0),
        code: AtomicU64::new(0),
    }
}; 32];

/// The guest's `si_uid` of a signal host process `pid` sent: its real uid
/// in the process table (a child's is there until it is reaped), else
/// root's, as for a process outside the namespace.
pub(super) fn sender_uid(pid: i32) -> u32 {
    super::cred::real_uid_of(pid).unwrap_or(0)
}

/// Record a host signal from outside; async-signal-safe (no locks).
fn record_external(sig: i32, si: &libc::siginfo_t) {
    let code = match si.si_code {
        0x10002 => -1,               // SI_QUEUE
        0x10003 => -2,               // SI_TIMER
        c @ 1..=6 if sig == 17 => c, // CLD_*
        _ => sigframe::SI_USER,
    };
    let e = &EXTERNAL_INFO[sig as usize - 1];
    e.from.store(si.si_pid as u32 as u64, Relaxed);
    e.code.store(
        code as u32 as u64 | (si.si_status as u32 as u64) << 32,
        Relaxed,
    );
    EXTERNAL.fetch_or(bit(sig), SeqCst);
}

fn drain_external() {
    let bits = EXTERNAL.swap(0, SeqCst);
    for sig in 1..32 {
        if bits & bit(sig) != 0 {
            let e = &EXTERNAL_INFO[sig as usize - 1];
            let (from, code) = (e.from.load(Relaxed), e.code.load(Relaxed));
            let pid = from as u32 as i32;
            let mut info = Siginfo::from_sender(
                sig,
                code as u32 as i32,
                super::pidns::vnr(pid),
                sender_uid(pid),
            );
            info.fields[1] = code >> 32; // _sigchld.status
            send_process(info);
        }
    }
}

// ---- taking and delivering --------------------------------------------------

/// Take the next signal whose handler is to run on `th`, applying ignore
/// and default actions to the ones before it. Clears the attention flag
/// first, so a sender racing with the scan sets it again.
fn take(th: &Thread) -> Option<Taken> {
    th.sig.attn.store(0, SeqCst);
    drain_external();
    let own = th.sig.own.swap(0, SeqCst);
    for sig in 1..32 {
        if own & bit(sig) != 0 {
            let _ = lock(&th.sig.pending).push(sender(sig, sigframe::SI_USER));
        }
    }
    loop {
        let (info, more) = {
            let mut tp = lock(&th.sig.pending);
            let mut pp = process();
            let allow = !th.sig.mask.load(SeqCst);
            let (t, p) = (tp.set & allow, pp.set & allow);
            if t | p == 0 {
                return None;
            }
            let sig = lowest(if t & SYNCHRONOUS != 0 {
                t & SYNCHRONOUS
            } else {
                t | p
            });
            let info = if t & bit(sig) != 0 {
                tp.take(sig)
            } else {
                pp.take(sig)
            };
            (info?, (tp.set | pp.set) & allow != 0)
        };
        let sig = info.signo;
        let act = action(sig);
        if act.handler == SIG_IGN {
            continue;
        }
        if act.handler == SIG_DFL {
            match default_action(sig) {
                Default::Ignore => continue,
                Default::Stop => {
                    host_self(SIGSTOP);
                    continue;
                }
                Default::Die => die(sig),
            }
        }
        if more {
            th.sig.attn.store(1, SeqCst);
        }
        return Some(Taken { info, act });
    }
}

/// For the layer's own blocking calls: whether a guest handler is to run,
/// so the call must return EINTR. Signals without a handler are consumed
/// here and do not interrupt, as on Linux.
pub fn interrupted(th: &Thread) -> bool {
    // A stashed signal keeps the flag set until it is delivered.
    if th.sig.attn.load(SeqCst) == 0 {
        return false;
    }
    let mut stash = lock(&th.sig.stash);
    if stash.is_some() {
        return true;
    }
    match take(th) {
        Some(t) => {
            *stash = Some(t);
            th.sig.attn.store(1, SeqCst);
            true
        }
        None => false,
    }
}

/// For the layer's blocking calls that cannot wait on the thread's parker
/// (binder's read, parked in the daemon): run `f`, and while it runs, any
/// thread that queues a signal for this one calls `wake`, which must make
/// `f` return soon (with EINTR). None, without running `f`, when a guest
/// handler is already due: the syscall then returns EINTR. The exit path
/// restarts an interrupted call when no handler runs, or after an
/// SA_RESTART one.
pub fn interruptible<R>(wake: &(dyn Fn() + Sync), f: impl FnOnce() -> R) -> Option<R> {
    let Some(th) = thread::current() else {
        return Some(f());
    };
    // SAFETY: only the lifetime is erased; the registration is removed
    // below, before `wake` can go away.
    let w: *const (dyn Fn() + Sync) = unsafe { std::mem::transmute(wake) };
    *lock(&th.sig.waker) = Some(Waker(w));
    let r = if interrupted(th) { None } else { Some(f()) };
    *lock(&th.sig.waker) = None;
    r
}

/// Build the frame for `t` and apply Linux's mask and SA_RESETHAND rules.
fn frame(th: &Thread, cpu: &Cpu, t: &Taken, uc_mask: u64, esr: u64, fault: u64) -> Entry {
    let sig = t.info.signo;
    let d = sigframe::Delivery {
        info: &t.info,
        act: &t.act,
        uc_mask,
        esr,
        fault_address: fault,
        default_restorer: context::restorer(),
    };
    // SAFETY: the guest stack (or alternate stack) the frame goes on is the
    // guest's own; a bad one faults like Linux's forced SIGSEGV.
    let e = unsafe { sigframe::setup(cpu, &d, &mut lock(&th.sig.alt)) };
    let mut add = t.act.mask;
    if t.act.flags & SA_NODEFER == 0 {
        add |= bit(sig);
    }
    th.sig.mask.fetch_or(add & !UNBLOCKABLE, SeqCst);
    if t.act.flags & SA_RESETHAND != 0 {
        ACTIONS.lock().unwrap_or_else(|e| e.into_inner())[sig as usize - 1].handler = SIG_DFL;
    }
    e
}

/// Syscalls Linux never restarts after a handler, even with SA_RESTART.
fn restartable(nr: u64, timed: bool) -> bool {
    !(matches!(nr, 22 | 72 | 73 | 101 | 115 | 133 | 137 | 441) || nr == 98 && timed)
}

/// `restart_nr` flags: a futex wait with a timeout (never restarted), and
/// FUTEX_LOCK_PI(2), which Linux always restarts (-ERESTARTNOINTR).
const RESTART_TIMED: u64 = 1 << 63;
const RESTART_ALWAYS: u64 = 1 << 62;

/// Called after every syscall the dispatcher ran (`a` are its arguments).
/// Returns true when the syscall was interrupted by a host signal but no
/// guest handler is to run: it is then restarted in place.
pub fn after_syscall(ctx: &GuestContext, nr: u64, a: &[u64; 6], r: i64) -> bool {
    let Some(th) = thread_of(ctx) else {
        return false;
    };
    // rt_sigreturn returns the restored x0; a host call's result is not a
    // Linux errno. Neither is ever restarted.
    if nr == 139 || nr == aim_hostcall::SYSCALL_NR {
        th.sig.restart_nr.store(0, Relaxed);
        return false;
    }
    let mut flags = 0;
    if nr == 98 {
        match a[1] & 0x7f {
            0 | 9 if a[3] != 0 => flags = RESTART_TIMED,
            6 | 13 => flags = RESTART_ALWAYS,
            _ => {}
        }
    }
    th.sig.restart_nr.store((nr + 1) | flags, Relaxed);
    th.sig.restart_a0.store(a[0], Relaxed);
    if r != -(EINTR as i64) {
        return false;
    }
    let mut stash = lock(&th.sig.stash);
    if stash.is_some() {
        return false;
    }
    match take(th) {
        None => true,
        Some(t) => {
            *stash = Some(t);
            th.sig.attn.store(1, SeqCst);
            false
        }
    }
}

/// Trampoline: a lean-path syscall returned with the attention flag set.
/// Its result is in x0 and its first argument in `orig_x0`.
#[unsafe(no_mangle)]
extern "C" fn linux_abi_lean_signal(ctx: *mut GuestContext) {
    // SAFETY: the trampoline passes this thread's live context.
    let ctx = unsafe { &mut *ctx };
    let nr = ctx.x[8];
    let a = [
        ctx.orig_x0,
        ctx.x[1],
        ctx.x[2],
        ctx.x[3],
        ctx.x[4],
        ctx.x[5],
    ];
    if after_syscall(ctx, nr, &a, ctx.x[0] as i64) {
        ctx.x[0] = ctx.orig_x0;
        super::dispatch(ctx);
    }
}

/// Trampoline, at syscall exit while the attention flag is set: deliver
/// one signal into the context (the trampoline calls again while the flag
/// stays set), or resume exactly after `rt_sigreturn`.
#[unsafe(no_mangle)]
extern "C" fn linux_abi_deliver(ctx: *mut GuestContext) {
    let run = || {
        // SAFETY: the trampoline passes this thread's live context.
        let ctx = unsafe { &mut *ctx };
        let Some(th) = thread_of(ctx) else {
            return;
        };
        let taken = lock(&th.sig.stash).take().or_else(|| take(th));
        let restart = th.sig.restart_nr.swap(0, Relaxed);
        let Some(t) = taken else {
            if let Some(m) = lock(&th.sig.saved_mask).take() {
                th.sig.mask.store(m, SeqCst);
            }
            if th.sig.exact.swap(false, SeqCst) {
                context::resume_trap();
            }
            return;
        };
        let mut cpu = Cpu::from_ctx(ctx);
        let nr = (restart & !(RESTART_TIMED | RESTART_ALWAYS)).wrapping_sub(1);
        if restart != 0
            && ctx.x[0] as i64 == -(EINTR as i64)
            && (restart & RESTART_ALWAYS != 0
                || t.act.flags & SA_RESTART != 0 && restartable(nr, restart & RESTART_TIMED != 0))
        {
            cpu.pc -= 4;
            cpu.x[0] = th.sig.restart_a0.load(Relaxed);
        }
        th.sig.exact.store(false, SeqCst);
        let uc_mask = lock(&th.sig.saved_mask)
            .take()
            .unwrap_or_else(|| th.sig.mask());
        frame(th, &cpu, &t, uc_mask, 0, 0).to_ctx(ctx);
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).is_err() {
        crate::diag!("[linux-abi] panic in signal delivery; aborting");
        std::process::abort();
    }
}

/// Deliver everything deliverable on a thread interrupted in guest code.
fn deliver_to_mc(th: &Thread, m: &mut DarwinMcontext) {
    while let Some(t) = take(th) {
        let cpu = Cpu::from_mc(m);
        frame(th, &cpu, &t, th.sig.mask(), 0, 0).to_mc(m);
    }
}

/// A poke (or a forwarded host signal) arrived at pc `m.pc`.
///
/// # Safety
/// `ctx` is this thread's live context and `m` its interrupted state.
unsafe fn act_now(ctx: *mut GuestContext, m: &mut DarwinMcontext) {
    let pc = m.pc;
    match context::region(pc) {
        // Re-run the lean exit check, which will see the flag.
        context::Region::LeanTail => m.pc = context::lean_check(),
        // Re-run the full exit check from the host stack.
        context::Region::FullTail => {
            m.pc = context::full_recheck();
            m.x[19] = ctx as u64;
            // SAFETY: caller contract.
            m.sp = unsafe { (*ctx).host_sp };
        }
        // An exit check comes later.
        context::Region::Trampoline => {}
        // SAFETY: caller contract.
        context::Region::Other if unsafe { (*ctx).in_host } != 0 => {}
        context::Region::Other => {
            // SAFETY: caller contract.
            if let Some(th) = thread_of(unsafe { &*ctx }) {
                deliver_to_mc(th, m);
            }
        }
    }
}

// ---- host signal handler --------------------------------------------------------

/// The host handler for every signal the layer takes. `patch` handles its
/// `brk` sites first. A fault nothing takes is reported by `diag`, then
/// kills the process with the signal, as Linux's default action.
extern "C" fn host_handler(sig: i32, info: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    // SAFETY: SA_SIGINFO handler arguments from the kernel.
    unsafe {
        if sig == libc::SIGTRAP && crate::patch::handle_brk(uc as *mut libc::ucontext_t) {
            return;
        }
        if let Some(fatal) = host_signal(sig, &*info, uc) {
            crate::diag::report(sig, info, uc);
            die(fatal);
        }
    }
}

/// Returns the Linux signal to die of for a fault nothing takes.
///
/// # Safety
/// Handler arguments from the kernel.
unsafe fn host_signal(hsig: i32, si: &libc::siginfo_t, uc: *mut libc::c_void) -> Option<i32> {
    // SAFETY: caller contract.
    let m = unsafe { sigframe::mcontext(uc) };
    let ctx = context::current_ctx();
    if hsig == CARRIER {
        if !ctx.is_null() {
            // SAFETY: this thread's context and interrupted state.
            unsafe { act_now(ctx, m) };
        }
        return None;
    }
    let fault = matches!(
        hsig,
        libc::SIGSEGV | libc::SIGBUS | libc::SIGILL | libc::SIGTRAP | libc::SIGFPE
    ) && si.si_code > 0
        && si.si_code < 0x10000;
    if fault {
        if matches!(hsig, libc::SIGSEGV | libc::SIGBUS) && super::jit::fault(ctx, m) {
            return None;
        }
        let host_fault = Some(from_host(hsig));
        if ctx.is_null() {
            return host_fault;
        }
        // SAFETY: this thread's context.
        let ctx = unsafe { &mut *ctx };
        if hsig == libc::SIGILL && m.pc == context::resume_trap_pc() {
            resume_exact(ctx, m);
            return None;
        }
        if context::region(m.pc) != context::Region::Other || ctx.in_host != 0 {
            return host_fault;
        }
        return guest_fault(ctx, hsig, si.si_code, m);
    }
    let sig = from_host(hsig);
    if sig == 0 {
        return None;
    }
    // SAFETY: this thread's context, if any.
    let th = unsafe { ctx.as_ref() }.and_then(thread_of);
    match th {
        // What a thread's own syscall raised (a write to a broken pipe, a
        // file too large) is directed at that thread, as on Linux.
        Some(th) if THREAD_DIRECTED & bit(sig) != 0 => {
            th.sig.own.fetch_or(bit(sig), SeqCst);
        }
        _ => record_external(sig, si),
    }
    match th {
        None => thread::poke_main_from_handler(CARRIER),
        Some(th) => {
            th.sig.attn.store(1, SeqCst);
            // The thread may be about to park (after its last check).
            th.park.unpark();
            // SAFETY: this thread's context and interrupted state.
            unsafe { act_now(ctx, m) };
        }
    }
    None
}

/// A synchronous fault in guest code. Some(signal) when the guest has no
/// handler that can run (Linux then kills the process with the signal).
fn guest_fault(ctx: &GuestContext, hsig: i32, code: i32, m: &mut DarwinMcontext) -> Option<i32> {
    let f = sigframe::translate_fault(hsig, code, m);
    let sig = f.info.signo;
    // macOS maps nothing below 4 GiB, so an access there beyond the null
    // pages (which implicit null checks use) is a stray pointer, such as a
    // heap reference that was not decoded: name the code that made it.
    // So is an access to the first page of ART's heap window
    // (`window::BASE`), which the guest sees as its null page: a null heap
    // reference decoded as if it were not.
    let window_null =
        sig == sigframe::SIGSEGV && (window::BASE..window::BASE + 0x1000).contains(&m.far);
    if (0x10000..1 << 32).contains(&f.fault_address) || window_null {
        crate::diag!(
            "[linux-abi] signal {sig}: access to {:#x} at pc {:#x} ({}), lr {:#x} ({})",
            if window_null { m.far } else { f.fault_address },
            m.pc,
            crate::diag::describe(m.pc),
            m.lr,
            crate::diag::describe(m.lr)
        );
    }
    let th = thread_of(ctx)?;
    let act = action(sig);
    let mask = th.sig.mask();
    if act.handler == SIG_DFL || act.handler == SIG_IGN || mask & bit(sig) != 0 {
        return Some(sig);
    }
    let cpu = Cpu::from_mc(m);
    let t = Taken { info: f.info, act };
    frame(th, &cpu, &t, mask, f.esr, f.fault_address).to_mc(m);
    None
}

/// The `rt_sigreturn` trap: load the complete context into the host state.
fn resume_exact(ctx: &mut GuestContext, m: &mut DarwinMcontext) {
    Cpu::from_ctx(ctx).to_mc(m);
    ctx.in_host = 0;
    if let Some(th) = thread_of(ctx)
        && th.sig.attn.load(SeqCst) != 0
    {
        deliver_to_mc(th, m);
    }
}

fn host_action(hsig: i32, handler: libc::sighandler_t, flags: i32) {
    // SAFETY: installing a process-wide host disposition.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler;
        sa.sa_flags = flags;
        libc::sigfillset(&mut sa.sa_mask);
        libc::sigaction(hsig, &sa, std::ptr::null_mut());
    }
}

fn forward(hsig: i32, extra: i32) {
    host_action(
        hsig,
        host_handler as usize,
        libc::SA_SIGINFO | libc::SA_ONSTACK | extra,
    );
}

/// Install the host handlers: faults, the carrier, and a forwarder for
/// every signal whose Linux default is not "ignore" (a forwarded signal the
/// guest blocks must stay pending instead of taking the host default).
/// Handlers run on each thread's host alternate stack and never restart
/// host syscalls, so a blocked thread notices the guest signal.
pub fn install_host_handlers() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        forward(CARRIER, 0);
        for sig in 1..32 {
            let h = to_host(sig);
            if h != 0
                && sig != SIGKILL
                && sig != SIGSTOP
                && !matches!(default_action(sig), Default::Ignore)
            {
                forward(h, 0);
            }
        }
    });
}

/// Mirror a guest disposition on the host where the kernel acts on it:
/// SIG_IGN (so SIGPIPE turns into EPIPE and SIGCHLD reaps), and the
/// SIGCHLD flags.
fn mirror_on_host(sig: i32, act: &KSigaction) {
    let h = to_host(sig);
    if h == 0 || SYNCHRONOUS & bit(sig) != 0 || sig == SIGKILL || sig == SIGSTOP {
        return;
    }
    if act.handler == SIG_IGN {
        host_action(h, libc::SIG_IGN, 0);
    } else if act.handler == SIG_DFL && matches!(default_action(sig), Default::Ignore) {
        host_action(h, libc::SIG_DFL, 0);
    } else {
        let mut extra = 0;
        if sig == 17 {
            if act.flags & SA_NOCLDSTOP != 0 {
                extra |= libc::SA_NOCLDSTOP;
            }
            if act.flags & SA_NOCLDWAIT != 0 {
                extra |= libc::SA_NOCLDWAIT;
            }
        }
        forward(h, extra);
    }
}

// ---- syscalls ---------------------------------------------------------------------

fn valid(sig: u64) -> bool {
    (1..=NSIG as u64).contains(&sig)
}

pub fn rt_sigaction(a: [u64; 6]) -> i64 {
    let (sig, act, oact, size) = (a[0], a[1], a[2], a[3]);
    if size != 8 || !valid(sig) {
        return -(EINVAL as i64);
    }
    let sig = sig as i32;
    if act != 0 && (sig == SIGKILL || sig == SIGSTOP) {
        return -(EINVAL as i64);
    }
    let new = {
        let mut t = ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &mut t[sig as usize - 1];
        // SAFETY: guest sigaction buffers.
        unsafe {
            let old = *slot;
            if act != 0 {
                let mut new = (act as *const KSigaction).read_unaligned();
                new.mask &= !UNBLOCKABLE;
                *slot = new;
            }
            if oact != 0 {
                (oact as *mut KSigaction).write_unaligned(old);
            }
        }
        *slot
    };
    if act != 0 {
        mirror_on_host(sig, &new);
        // POSIX: ignoring a signal discards it where it is pending.
        if ignored(sig, &new) {
            process().flush(sig);
            let all: Vec<_> = thread::with_table(|t| t.values().cloned().collect());
            for th in all {
                lock(&th.sig.pending).flush(sig);
            }
        }
    }
    0
}

fn current() -> &'static Thread {
    thread::current().expect("signal syscall on a thread without a guest context")
}

pub fn rt_sigprocmask(a: [u64; 6]) -> i64 {
    let (how, set, oset, size) = (a[0], a[1], a[2], a[3]);
    if size != 8 {
        return -(EINVAL as i64);
    }
    let th = current();
    let old = th.sig.mask();
    if set != 0 {
        // SAFETY: guest sigset.
        let s = unsafe { (set as *const u64).read_unaligned() };
        let new = match how {
            0 => old | s,
            1 => old & !s,
            2 => s,
            _ => return -(EINVAL as i64),
        } & !UNBLOCKABLE;
        th.sig.mask.store(new, SeqCst);
        if old & !new != 0 {
            // Something was unblocked: the exit check looks for it.
            th.sig.attn.store(1, SeqCst);
        }
    }
    if oset != 0 {
        // SAFETY: guest sigset.
        unsafe { (oset as *mut u64).write_unaligned(old) };
    }
    0
}

pub fn rt_sigpending(a: [u64; 6]) -> i64 {
    if a[1] != 8 {
        return -(EINVAL as i64);
    }
    let th = current();
    drain_external();
    let pending = (lock(&th.sig.pending).set | process().set) & th.sig.mask();
    // SAFETY: guest sigset.
    unsafe { (a[0] as *mut u64).write_unaligned(pending) };
    0
}

pub fn rt_sigsuspend(a: [u64; 6]) -> i64 {
    if a[1] != 8 {
        return -(EINVAL as i64);
    }
    let th = current();
    // SAFETY: guest sigset.
    let new = unsafe { (a[0] as *const u64).read_unaligned() } & !UNBLOCKABLE;
    lock(&th.sig.saved_mask).get_or_insert(th.sig.mask());
    th.sig.mask.store(new, SeqCst);
    th.sig.attn.store(1, SeqCst);
    while !interrupted(th) {
        th.park.park(th.tid, None);
    }
    -(EINTR as i64)
}

/// Take a pending signal in `set` (thread queue first).
fn dequeue(th: &Thread, set: u64) -> Option<Siginfo> {
    drain_external();
    let mut tp = lock(&th.sig.pending);
    let mut pp = process();
    let (t, p) = (tp.set & set, pp.set & set);
    if t | p == 0 {
        return None;
    }
    let sig = lowest(t | p);
    if t & bit(sig) != 0 {
        tp.take(sig)
    } else {
        pp.take(sig)
    }
}

pub fn rt_sigtimedwait(a: [u64; 6]) -> i64 {
    let (set, info, ts, size) = (a[0], a[1], a[2], a[3]);
    if size != 8 {
        return -(EINVAL as i64);
    }
    let deadline = if ts == 0 {
        None
    } else {
        match park::read_timespec(ts) {
            Ok(ns) => Some(park::after(ns)),
            Err(e) => return e,
        }
    };
    let th = current();
    // SAFETY: guest sigset.
    let set = unsafe { (set as *const u64).read_unaligned() } & !UNBLOCKABLE;
    th.sig.waitset.store(set, SeqCst);
    let r = loop {
        if let Some(i) = dequeue(th, set) {
            if info != 0 {
                // SAFETY: guest siginfo buffer.
                unsafe { i.write(info) };
            }
            break i.signo as i64;
        }
        if deadline.is_some_and(|d| park::monotonic() >= d) {
            break -(EAGAIN as i64);
        }
        if interrupted(th) {
            break -(EINTR as i64);
        }
        th.park.park(th.tid, deadline);
    };
    th.sig.waitset.store(0, SeqCst);
    r
}

fn sender(sig: i32, code: i32) -> Siginfo {
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    Siginfo::from_sender(sig, code, pid, super::cred::getuid(174) as u32)
}

fn my_pid() -> i64 {
    super::process::getpid()
}

/// Send host signal `h` (0: none) for Linux `sig` to another process `p`
/// of the namespace, with `check_kill_permission`'s rule: the credentials
/// of `cred::may_signal`, or SIGCONT within the caller's session.
pub(super) fn signal_process(p: i32, sig: i32, h: i32) -> i64 {
    // SAFETY: trivial.
    let same_session = || unsafe { libc::getsid(p) == libc::getsid(0) };
    if !(super::cred::may_signal(p) || sig == SIGCONT && same_session()) {
        return -(EPERM as i64);
    }
    // SAFETY: plain kill of a process of the namespace.
    crate::errno::check(unsafe { libc::kill(p, h) } as i64)
}

/// kill(pid, sig). Other processes are signalled through Darwin, which has
/// no real-time signals.
pub fn kill(a: [u64; 6]) -> i64 {
    let (pid, sig) = (a[0] as i32 as i64, a[1] as i32);
    if !(0..=NSIG).contains(&sig) {
        return -(EINVAL as i64);
    }
    if pid == my_pid() {
        return if sig == 0 {
            0
        } else {
            send_process(sender(sig, sigframe::SI_USER))
        };
    }
    let h = to_host(sig);
    if sig != 0 && h == 0 {
        if pid == 0 {
            return send_process(sender(sig, sigframe::SI_USER));
        }
        return -(EINVAL as i64);
    }
    let targets = match pid {
        -1 => super::pidns::members().map(|m| (m, true)),
        ..=0 => {
            // SAFETY: trivial.
            let pgrp = if pid == 0 {
                unsafe { libc::getpgrp() }
            } else {
                -pid as i32
            };
            super::pidns::group(pgrp).map(|m| (m, false))
        }
        _ => {
            if let Err(e) = super::pidns::check(pid as i32) {
                return e;
            }
            return signal_process(pid as i32, sig, h);
        }
    };
    let Some((targets, all)) = targets else {
        // SAFETY: plain kill of a process or group.
        return crate::errno::check(unsafe { libc::kill(pid as i32, h) } as i64);
    };
    // Each member in turn, with the kernel's result: a group succeeds if
    // one member was signalled (`__kill_pgrp_info`); `kill(-1)` skips the
    // caller, ignores EPERM and fails only for want of a target.
    let me = my_pid() as i32;
    let targets: Vec<i32> = targets.into_iter().filter(|&p| !all || p != me).collect();
    let mut sent = false;
    let mut err = if all && !targets.is_empty() {
        0
    } else {
        -(ESRCH as i64)
    };
    for p in targets {
        match signal_process(p, sig, h) {
            0 => sent = !all,
            e if all && e == -(EPERM as i64) => {}
            e => err = e,
        }
    }
    if sent { 0 } else { err }
}

fn target(tgid: Option<i64>, tid: i64, sig: i32) -> Result<std::sync::Arc<Thread>, i64> {
    if tid <= 0 || tgid.is_some_and(|g| g <= 0) || !(0..=NSIG).contains(&sig) {
        return Err(-(EINVAL as i64));
    }
    if tgid.is_some_and(|g| g != my_pid()) {
        return Err(-(ESRCH as i64));
    }
    thread::find(tid as i32).ok_or(-(ESRCH as i64))
}

/// tkill(tid, sig) and tgkill(tgid, tid, sig).
pub fn tgkill(nr: u64, a: [u64; 6]) -> i64 {
    let (tgid, tid, sig) = if nr == 131 {
        (Some(a[0] as i32 as i64), a[1] as i32 as i64, a[2] as i32)
    } else {
        (None, a[0] as i32 as i64, a[1] as i32)
    };
    // Another process's thread (its tid names the process, see `thread`):
    // Darwin signals whole processes, so the signal goes to the process.
    let owner = thread::owner(tid as i32) as i64;
    if tid > 0 && owner != my_pid() && tgid.is_none_or(|g| g == owner) {
        return kill([owner as u64, sig as u64, 0, 0, 0, 0]);
    }
    match target(tgid, tid, sig) {
        Err(e) => e,
        Ok(_) if sig == 0 => 0,
        Ok(th) => send_thread(&th, sender(sig, sigframe::SI_TKILL)),
    }
}

/// The guest's siginfo for rt_(tg)sigqueueinfo. Linux lets a process claim
/// any si_code towards itself, the only target here.
fn queued_info(sig: i32, p: u64) -> Siginfo {
    // SAFETY: guest siginfo.
    let mut info = unsafe { Siginfo::read(p) };
    info.signo = sig;
    info
}

/// rt_sigqueueinfo(tgid, sig, info).
pub fn rt_sigqueueinfo(a: [u64; 6]) -> i64 {
    let (tgid, sig) = (a[0] as i32 as i64, a[1] as i32);
    if !valid(sig as u64) {
        return -(EINVAL as i64);
    }
    if tgid != my_pid() {
        return -(if tgid <= 0 { EINVAL } else { ESRCH } as i64);
    }
    send_process(queued_info(sig, a[2]))
}

/// rt_tgsigqueueinfo(tgid, tid, sig, info).
pub fn rt_tgsigqueueinfo(a: [u64; 6]) -> i64 {
    let sig = a[2] as i32;
    if !valid(sig as u64) {
        return -(EINVAL as i64);
    }
    let th = match target(Some(a[0] as i32 as i64), a[1] as i32 as i64, sig) {
        Ok(t) => t,
        Err(e) => return e,
    };
    send_thread(&th, queued_info(sig, a[3]))
}

/// Linux `stack_t`: { void *ss_sp; int ss_flags; size_t ss_size; }.
pub fn sigaltstack(ctx: &GuestContext, a: [u64; 6]) -> i64 {
    let (ss, old) = (a[0], a[1]);
    let th = current();
    let mut alt = lock(&th.sig.alt);
    let on = alt.contains(ctx.sp);
    let cur = *alt;
    if ss != 0 {
        // SAFETY: guest stack_t.
        let (sp, flags, size) = unsafe {
            (
                (ss as *const u64).read_unaligned(),
                ((ss + 8) as *const i32).read_unaligned(),
                ((ss + 16) as *const u64).read_unaligned(),
            )
        };
        if on {
            return -(EPERM as i64);
        }
        let mode = flags & !SS_AUTODISARM;
        if mode != 0 && mode != SS_DISABLE && mode != SS_ONSTACK {
            return -(EINVAL as i64);
        }
        *alt = if mode == SS_DISABLE {
            AltStack::DISABLED
        } else if size < MINSIGSTKSZ {
            return -(ENOMEM as i64);
        } else {
            AltStack {
                sp,
                size,
                flags: flags & SS_AUTODISARM,
            }
        };
    }
    if old != 0 {
        let flags = if !cur.enabled() {
            SS_DISABLE
        } else if on {
            SS_ONSTACK
        } else {
            0
        } | cur.flags & SS_AUTODISARM;
        // SAFETY: guest stack_t.
        unsafe {
            (old as *mut u64).write_unaligned(cur.sp);
            ((old + 8) as *mut i32).write_unaligned(flags);
            ((old + 16) as *mut u64).write_unaligned(cur.size);
        }
    }
    0
}

/// rt_sigreturn: restore the frame at the guest sp. On a live context the
/// exit check then resumes exactly (or delivers what became unblocked).
pub fn rt_sigreturn(ctx: &mut GuestContext) -> i64 {
    let th = current();
    // SAFETY: the frame the guest handler returned from.
    let Some((cpu, mask, saved)) = (unsafe { sigframe::restore(ctx.sp) }) else {
        crate::diag!("[linux-abi] rt_sigreturn: bad frame at {:#x}", ctx.sp);
        die(sigframe::SIGSEGV);
    };
    th.sig.mask.store(mask & !UNBLOCKABLE, SeqCst);
    {
        // Linux restore_altstack: only when not running on the alternate
        // stack, and errors other than EFAULT are ignored.
        let mut alt = lock(&th.sig.alt);
        if !alt.contains(ctx.sp) {
            if saved.flags & SS_DISABLE != 0 {
                *alt = AltStack::DISABLED;
            } else if saved.size >= MINSIGSTKSZ {
                *alt = AltStack {
                    flags: saved.flags & SS_AUTODISARM,
                    ..saved
                };
            }
        }
    }
    cpu.to_ctx(ctx);
    // The restored mask may unblock something; on a live context the exit
    // check also performs the exact resume.
    th.sig.exact.store(context::is_live(ctx), SeqCst);
    th.sig.attn.store(1, SeqCst);
    ctx.x[0] as i64
}

/// After a syscall dispatched from the `brk` fallback's SIGTRAP handler,
/// which has no exit check: a pending signal is delivered by the carrier
/// once the handler returns to guest code.
pub fn repoke_self() {
    if thread::current().is_some_and(|th| th.sig.attn.load(SeqCst) != 0) {
        // SAFETY: raising a signal on the calling thread.
        unsafe { libc::pthread_kill(libc::pthread_self(), CARRIER) };
    }
}

/// execve in place (`flush_signal_handlers`): caught signals go back to
/// their default action, ignored ones stay ignored, every action loses its
/// flags, mask and restorer, and the alternate stack is gone. The mask and
/// pending signals stay.
pub(super) fn exec_reset() {
    for sig in (1..=NSIG).filter(|&s| s != SIGKILL && s != SIGSTOP) {
        let act = KSigaction {
            handler: if action(sig).handler == SIG_IGN {
                SIG_IGN
            } else {
                SIG_DFL
            },
            flags: 0,
            restorer: 0,
            mask: 0,
        };
        rt_sigaction([sig as u64, &act as *const KSigaction as u64, 0, 8, 0, 0]);
    }
    *lock(&current().sig.alt) = AltStack::DISABLED;
}

// ---- fork -------------------------------------------------------------------------

/// Fork: the dispositions, and the forking thread's mask and alternate
/// stack. Pending signals are not inherited.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let actions = *ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
    w.seq(actions.iter(), |w, a| {
        for v in [a.handler, a.flags, a.restorer, a.mask] {
            w.u64(v);
        }
    });
    let th = current();
    w.u64(th.sig.mask());
    let alt = *lock(&th.sig.alt);
    w.u64(alt.sp);
    w.u64(alt.size);
    w.i32(alt.flags);
}

/// Restore on the child's main thread, which the context of the fork
/// runs on.
pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let actions = r.seq(|r| KSigaction {
        handler: r.u64(),
        flags: r.u64(),
        restorer: r.u64(),
        mask: r.u64(),
    });
    for (i, act) in actions.iter().enumerate() {
        let sig = i as u64 + 1;
        if valid(sig) && sig != SIGKILL as u64 && sig != SIGSTOP as u64 {
            rt_sigaction([sig, act as *const KSigaction as u64, 0, 8, 0, 0]);
        }
    }
    let mask = r.u64();
    rt_sigprocmask([2, &mask as *const u64 as u64, 0, 8, 0, 0]);
    let alt = AltStack {
        sp: r.u64(),
        size: r.u64(),
        flags: r.i32(),
    };
    *lock(&current().sig.alt) = alt;
}
