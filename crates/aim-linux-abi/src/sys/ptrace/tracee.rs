//! The tracee's side: which of this process's threads are traced, and
//! their ptrace-stops.
//!
//! A traced thread stops where the layer has its complete guest registers,
//! the points where it delivers signals (`signal`): at a syscall exit, in a
//! blocking call the stop interrupts (the call restarts afterwards, and the
//! tracer sees the registers of the restart: pc at the `svc`, x0 its first
//! argument), and in guest code on the carrier signal. It publishes the
//! registers, and blocks until its tracer resumes or detaches it. The
//! agent (`agent`) answers the tracer from this state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::context;
use crate::sys::sigframe::Cpu;

pub(super) const SIGTRAP: i32 = 5;
pub(super) const EVENT_FORK: u32 = 1;
pub(super) const EVENT_VFORK: u32 = 2;
pub(super) const EVENT_CLONE: u32 = 3;
pub(super) const EVENT_STOP: u32 = 128;
const CLONE_VFORK: u64 = 0x4000;
const LINUX_SIGCHLD: u64 = 17;
/// How long an ending process waits for its tracers to take the news.
const END_GRACE: Duration = Duration::from_millis(200);

/// Registers of a stopped thread, as the regsets `PTRACE_GETREGSET` reads.
pub(super) struct Regs {
    /// NT_PRSTATUS: `struct user_pt_regs`.
    pub gp: Vec<u8>,
    /// NT_PRFPREG: `struct user_fpsimd_state`.
    pub fp: Vec<u8>,
    /// NT_ARM_TLS: tpidr_el0, tpidr2_el0.
    pub tls: Vec<u8>,
}

impl Regs {
    fn of(cpu: &Cpu) -> Regs {
        let mut x = cpu.x;
        // The guest's x18 (Linux's shadow call stack) lives in a slot.
        x[18] = context::guest_scs();
        let gp = x
            .iter()
            .chain([cpu.sp, cpu.pc, cpu.pstate].iter())
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut fp: Vec<u8> = cpu.v.iter().flat_map(|v| v.to_le_bytes()).collect();
        fp.extend(cpu.fpsr.to_le_bytes());
        fp.extend(cpu.fpcr.to_le_bytes());
        fp.extend([0; 8]);
        let mut tls = context::guest_tp().to_le_bytes().to_vec();
        tls.extend([0; 8]);
        Regs { gp, fp, tls }
    }
}

pub(super) struct Traced {
    /// The tracer's pid.
    pub tracer: i32,
    pub options: u64,
    /// A PTRACE_INTERRUPT stop is due (Linux's JOBCTL_TRAP_STOP).
    pub trap: bool,
    /// In a ptrace-stop: its registers.
    pub stopped: Option<Regs>,
    /// The stop's wait status, until the tracer's wait takes it.
    pub report: Option<i32>,
    /// PTRACE_GETEVENTMSG's value.
    pub message: u64,
}

#[derive(Default)]
pub(super) struct State {
    pub traced: HashMap<i32, Traced>,
    /// Traced threads that ended: their tracer and wait status, until the
    /// tracer's wait takes it.
    pub ended: HashMap<i32, (i32, i32)>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
/// Signalled on every change of [`STATE`].
pub(super) static CHANGED: Condvar = Condvar::new();
/// Number of traced threads: the signal paths check nothing while it is 0.
static TRACED: AtomicUsize = AtomicUsize::new(0);

pub(super) fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wait for a change, at most `d`.
pub(super) fn wait(g: MutexGuard<'static, State>, d: Duration) -> MutexGuard<'static, State> {
    CHANGED
        .wait_timeout(g, d)
        .map_or_else(|e| e.into_inner().0, |r| r.0)
}

impl State {
    pub fn attach(&mut self, tid: i32, t: Traced) {
        crate::sys::procrec::traced(tid, t.tracer);
        if self.traced.insert(tid, t).is_none() {
            TRACED.fetch_add(1, SeqCst);
        }
    }

    pub fn detach(&mut self, tid: i32) -> Option<Traced> {
        let t = self.traced.remove(&tid)?;
        TRACED.fetch_sub(1, SeqCst);
        crate::sys::procrec::traced(tid, 0);
        CHANGED.notify_all();
        Some(t)
    }

    /// The thread `tid` if `tracer` traces it (and it is stopped, with
    /// `stopped`): ptrace's ESRCH otherwise.
    pub fn of(&mut self, tid: i32, tracer: i32, stopped: bool) -> Option<&mut Traced> {
        self.traced
            .get_mut(&tid)
            .filter(|t| t.tracer == tracer && (!stopped || t.stopped.is_some()))
    }
}

/// Whether thread `tid` has a PTRACE_INTERRUPT stop due.
pub fn trap_pending(tid: i32) -> bool {
    TRACED.load(SeqCst) != 0 && lock().traced.get(&tid).is_some_and(|t| t.trap)
}

/// The PTRACE_INTERRUPT stop of thread `tid`, if due, with registers `cpu`.
pub fn trap_stop(tid: i32, cpu: &Cpu) {
    if trap_pending(tid) {
        let status = SIGTRAP | (EVENT_STOP as i32) << 8;
        stop(tid, status << 8 | 0x7f, cpu, None);
    }
}

/// Stop thread `tid` with wait status `status` (and event message) until
/// its tracer resumes or leaves it.
fn stop(tid: i32, status: i32, cpu: &Cpu, message: Option<u64>) {
    let regs = Regs::of(cpu);
    let mut g = lock();
    let Some(t) = g.traced.get_mut(&tid) else {
        return;
    };
    t.trap = false;
    t.stopped = Some(regs);
    t.report = Some(status);
    if let Some(m) = message {
        t.message = m;
    }
    CHANGED.notify_all();
    while g.traced.get(&tid).is_some_and(|t| t.stopped.is_some()) {
        g = CHANGED.wait(g).unwrap_or_else(|e| e.into_inner());
    }
}

/// A traced thread's clone that its options report: the event, and the
/// tracer and options the new thread or process is traced with
/// (`kernel_clone`'s choice of PTRACE_EVENT_VFORK, _CLONE or _FORK).
pub struct CloneEvent {
    event: u32,
    pub tracer: i32,
    pub options: u64,
}

pub fn clone_event(flags: u64) -> Option<CloneEvent> {
    if TRACED.load(SeqCst) == 0 {
        return None;
    }
    let event = if flags & CLONE_VFORK != 0 {
        EVENT_VFORK
    } else if flags & 0xff != LINUX_SIGCHLD {
        EVENT_CLONE
    } else {
        EVENT_FORK
    };
    let tid = crate::sys::thread::gettid() as i32;
    let g = lock();
    let t = g.traced.get(&tid)?;
    (t.options & 1 << event != 0).then_some(CloneEvent {
        event,
        tracer: t.tracer,
        options: t.options,
    })
}

/// The calling thread made `child` under clone event `e`: its event stop.
pub fn clone_stop(e: &CloneEvent, child: i32, cpu: &Cpu) {
    let tid = crate::sys::thread::gettid() as i32;
    let status = (SIGTRAP | (e.event as i32) << 8) << 8 | 0x7f;
    stop(tid, status, cpu, Some(child as u64));
}

/// Thread `tid` starts traced (an automatic attach): it stops before it
/// runs guest code.
pub fn born_traced(tid: i32, tracer: i32, options: u64) {
    lock().attach(
        tid,
        Traced {
            tracer,
            options,
            trap: true,
            stopped: None,
            report: None,
            message: 0,
        },
    );
    crate::sys::signal::poke(tid);
}

/// A thread that never started: it is not traced.
pub fn forget(tid: i32) {
    lock().detach(tid);
}

/// Thread `tid` ends with wait status `status`.
pub fn thread_ended(tid: i32, status: i32) {
    if TRACED.load(SeqCst) == 0 {
        return;
    }
    let mut g = lock();
    if let Some(t) = g.detach(tid) {
        g.ended.insert(tid, (t.tracer, status));
    }
}

/// The process ends with wait status `status`: each traced thread ends
/// with it, and the process waits briefly for its tracers' waits, which
/// cannot reach it later.
pub fn process_ending(status: i32) {
    if TRACED.load(SeqCst) == 0 {
        return;
    }
    let mut g = lock();
    let tids: Vec<i32> = g.traced.keys().copied().collect();
    for tid in tids {
        if let Some(t) = g.detach(tid) {
            g.ended.insert(tid, (t.tracer, status));
        }
    }
    let deadline = Instant::now() + END_GRACE;
    while !g.ended.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        g = wait(g, left);
    }
}

/// The tracer `pid` is gone: its tracees go on untraced (and are killed
/// with PTRACE_O_EXITKILL), as Linux's `exit_ptrace`.
pub(super) fn tracer_gone(pid: i32) {
    let mut g = lock();
    let tids: Vec<i32> = g
        .traced
        .iter()
        .filter(|t| t.1.tracer == pid)
        .map(|t| *t.0)
        .collect();
    let mut kill = false;
    for tid in tids {
        kill |= g
            .detach(tid)
            .is_some_and(|t| t.options & super::O_EXITKILL != 0);
    }
    g.ended.retain(|_, e| e.0 != pid);
    drop(g);
    if kill {
        // SAFETY: killing this process, as the tracer's exit asks.
        unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
    }
}

/// The tracer of thread `tid`, or 0 (`TracerPid`).
pub fn tracer_of(tid: i32) -> i32 {
    if TRACED.load(SeqCst) == 0 {
        return 0;
    }
    lock().traced.get(&tid).map_or(0, |t| t.tracer)
}
