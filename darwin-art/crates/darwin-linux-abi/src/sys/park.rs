//! Blocking for guest threads: a per-thread parker that futex wakes, signals
//! and deadlines unpark, plus the sleeps built on it.
//!
//! Darwin's timed waits (`__ulock_wait2`, Mach semaphores, psynch) fire up
//! to 10% late: 2 ms on a 20 ms wait, 8 ms for semaphores. Only a kqueue
//! EVFILT_TIMER with zero leeway is precise (+30 µs; experiments/p0/03). A
//! deadline is therefore armed on one process-wide kqueue whose host thread
//! unparks the waiter when it fires, and the wait itself stays untimed on
//! `os_sync_wait_on_address`.
//!
//! Guest clocks: CLOCK_MONOTONIC is the host CLOCK_MONOTONIC, as in
//! `misc::clock_gettime`. Deadlines are host CLOCK_MONOTONIC nanoseconds.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};

use crate::errno::{EBADF, EINTR, EINVAL};

unsafe extern "C" {
    fn os_sync_wait_on_address(addr: *mut libc::c_void, value: u64, size: usize, flags: u32)
    -> i32;
    fn __ulock_wait2(
        op: u32,
        addr: *mut libc::c_void,
        value: u64,
        timeout_ns: u64,
        value2: u64,
    ) -> i32;
    fn os_sync_wake_by_address_any(addr: *mut libc::c_void, size: usize, flags: u32) -> i32;
}

/// `UL_COMPARE_AND_WAIT | ULF_NO_ERRNO`: the flavour `os_sync` uses for a
/// private 4-byte word.
const UL_WAIT: u32 = 1 | 0x0100_0000;

pub fn now(clock: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: local timespec.
    unsafe { libc::clock_gettime(clock, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub fn monotonic() -> u64 {
    now(libc::CLOCK_MONOTONIC)
}

/// A guest `struct timespec` in nanoseconds, or -EINVAL.
pub fn read_timespec(p: u64) -> Result<u64, i64> {
    // SAFETY: guest timespec { i64 tv_sec; i64 tv_nsec; }.
    let [sec, nsec] = unsafe { (p as *const [i64; 2]).read_unaligned() };
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(-(EINVAL as i64));
    }
    Ok((sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64))
}

pub fn write_timespec(p: u64, ns: u64) {
    let v = [(ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64];
    // SAFETY: guest timespec.
    unsafe { (p as *mut [i64; 2]).write_unaligned(v) };
}

/// A deadline `ns` from now.
pub fn after(ns: u64) -> u64 {
    monotonic().saturating_add(ns)
}

/// An absolute guest time on CLOCK_REALTIME or CLOCK_MONOTONIC as a
/// deadline. A REALTIME deadline is converted once, on entry.
pub fn absolute(ns: u64, realtime: bool) -> u64 {
    if realtime {
        after(ns.saturating_sub(now(libc::CLOCK_REALTIME)))
    } else {
        ns
    }
}

/// One per guest thread. States: 0 idle, 1 unparked (a token), 2 parked.
#[derive(Default)]
pub struct Parker {
    word: AtomicU32,
}

impl Parker {
    fn addr(&self) -> *mut libc::c_void {
        self.word.as_ptr() as *mut _
    }

    pub fn unpark(&self) {
        if self.word.swap(1, SeqCst) == 2 {
            // SAFETY: waking waiters on our own word.
            unsafe { os_sync_wake_by_address_any(self.addr(), 4, 0) };
        }
    }

    /// Block until unparked, `deadline` passes, or a host signal interrupts
    /// the wait. Returns early and spuriously; callers re-check their
    /// condition, pending signals and the clock after every return. `tid` is
    /// the caller's (the timer unparks it by tid).
    pub fn park(&self, tid: i32, deadline: Option<u64>) {
        if self.word.swap(0, SeqCst) == 1
            || self.word.compare_exchange(0, 2, SeqCst, SeqCst).is_err()
        {
            self.word.store(0, SeqCst);
            return;
        }
        let timer = deadline.and_then(|d| arm(tid, d));
        loop {
            let r = match (deadline, timer) {
                (Some(d), None) => {
                    let left = d.saturating_sub(monotonic());
                    if left == 0 {
                        break;
                    }
                    // SAFETY: waiting on our own word. Without a timer
                    // kqueue the deadline carries Darwin's timer leeway.
                    unsafe { __ulock_wait2(UL_WAIT, self.addr(), 2, left, 0) }
                }
                // SAFETY: waiting on our own word.
                _ => unsafe { os_sync_wait_on_address(self.addr(), 2, 4, 0) },
            };
            // Unparked, or interrupted (EINTR) or timed out.
            if self.word.load(SeqCst) != 2 || r < 0 {
                break;
            }
            if deadline.is_some_and(|d| monotonic() >= d) {
                break;
            }
        }
        if let Some(t) = timer {
            disarm(t);
        }
        self.word.store(0, SeqCst);
    }
}

// ---- precise deadlines ------------------------------------------------------

/// The timer kqueue, or -1.
static TIMER_KQ: Mutex<i32> = Mutex::new(-1);
static NEXT_TIMER: AtomicU64 = AtomicU64::new(1);

fn new_timer_kq() -> Option<i32> {
    // SAFETY: plain kqueue creation.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return None;
    }
    // Out of the guest's way and out of /proc/self/fd. If the guest closes
    // it anyway, arming fails with EBADF and a new one is made.
    let kq = super::fdtab::hide(kq);
    std::thread::Builder::new()
        .name("linux-abi-timers".into())
        .spawn(move || timer_loop(kq))
        .ok()?;
    Some(kq)
}

/// The current timer kqueue; `stale` (one that failed with EBADF) is
/// replaced.
fn timer_kq(stale: i32) -> Option<i32> {
    let mut kq = TIMER_KQ.lock().unwrap_or_else(|e| e.into_inner());
    if *kq < 0 || *kq == stale {
        if *kq >= 0 {
            super::fdtab::unhide(*kq);
        }
        *kq = new_timer_kq()?;
    }
    Some(*kq)
}

/// Held across a fork (see `thread::fork_prepare`).
pub fn fork_lock() -> std::sync::MutexGuard<'static, i32> {
    TIMER_KQ.lock().unwrap_or_else(|e| e.into_inner())
}

/// In the child: kqueues are not inherited and the timer thread is gone.
pub fn fork_child(mut kq: std::sync::MutexGuard<'static, i32>) {
    if *kq >= 0 {
        super::fdtab::unhide(*kq);
    }
    *kq = -1;
}

fn kevent(kq: i32, ev: &libc::kevent64_s) -> i32 {
    // SAFETY: one change, no events out.
    unsafe { libc::kevent64(kq, ev, 1, std::ptr::null_mut(), 0, 0, std::ptr::null()) }
}

fn timer_event(id: u64, flags: u16, fflags: u32, data: i64, udata: u64) -> libc::kevent64_s {
    libc::kevent64_s {
        ident: id,
        filter: libc::EVFILT_TIMER,
        flags,
        fflags,
        data,
        udata,
        ext: [0, 0],
    }
}

/// `udata` of a POSIX timer's expiry, handed to `ptimer`; any other event
/// unparks the tid in its `udata`.
pub const PTIMER_EXPIRY: u64 = 1 << 63;

/// A fresh identifier for [`arm_event`].
pub fn timer_id() -> u64 {
    NEXT_TIMER.fetch_add(1, SeqCst)
}

/// Fire timer `id` once at `deadline`, delivering `udata`. Returns the
/// kqueue it is armed on, for [`disarm`].
pub fn arm_event(id: u64, udata: u64, deadline: u64) -> Option<i32> {
    let rel = deadline.saturating_sub(monotonic()).max(1);
    let ev = timer_event(
        id,
        libc::EV_ADD | libc::EV_ONESHOT,
        libc::NOTE_NSECONDS | libc::NOTE_LEEWAY | libc::NOTE_CRITICAL,
        rel.min(i64::MAX as u64) as i64,
        udata,
    );
    let mut stale = -1;
    for _ in 0..2 {
        let kq = timer_kq(stale)?;
        if kevent(kq, &ev) == 0 {
            return Some(kq);
        }
        if crate::errno::last() != EBADF {
            return None;
        }
        stale = kq;
    }
    None
}

/// Unpark `tid` at `deadline`. None if no timer could be armed.
fn arm(tid: i32, deadline: u64) -> Option<(i32, u64)> {
    let id = timer_id();
    arm_event(id, tid as u32 as u64, deadline).map(|kq| (kq, id))
}

pub fn disarm((kq, id): (i32, u64)) {
    kevent(kq, &timer_event(id, libc::EV_DELETE, 0, 0, 0));
}

fn timer_loop(kq: i32) {
    // SAFETY: this host thread takes no signals, so the kernel never picks
    // it for a process-directed one; it runs at the highest QoS so that a
    // busy host delays expiries as little as possible.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
        let mut all: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }
    // SAFETY: an all-zero kevent64_s is valid.
    let mut evs: [libc::kevent64_s; 16] = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: waiting for events into a local buffer.
        let n = unsafe {
            libc::kevent64(
                kq,
                std::ptr::null(),
                0,
                evs.as_mut_ptr(),
                evs.len() as i32,
                0,
                std::ptr::null(),
            )
        };
        if n < 0 && crate::errno::last() == EINTR {
            continue;
        }
        if n < 0 {
            // Closed by the guest: `arm` makes a new one.
            return;
        }
        for ev in &evs[..n as usize] {
            if ev.udata & PTIMER_EXPIRY != 0 {
                super::ptimer::expired(ev.ident);
            } else {
                super::thread::unpark(ev.udata as i32);
            }
        }
    }
}

// ---- sleeps -----------------------------------------------------------------

const TIMER_ABSTIME: u64 = 1;

/// Sleep until `deadline`. Returns -EINTR (and the time left) when a guest
/// signal handler is to run; signals with no handler do not interrupt.
fn sleep_until(deadline: u64) -> Result<(), u64> {
    let Some(th) = super::thread::current() else {
        return Ok(());
    };
    loop {
        let now = monotonic();
        if now >= deadline {
            return Ok(());
        }
        if super::signal::interrupted(th) {
            return Err(deadline - now);
        }
        th.park.park(th.tid, Some(deadline));
    }
}

/// nanosleep(req, rem).
pub fn nanosleep(a: [u64; 6]) -> i64 {
    let ns = match read_timespec(a[0]) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match sleep_until(after(ns)) {
        Ok(()) => 0,
        Err(left) => {
            if a[1] != 0 {
                write_timespec(a[1], left);
            }
            -(EINTR as i64)
        }
    }
}

/// clock_nanosleep(clock, flags, req, rem): returns the error number itself.
pub fn clock_nanosleep(a: [u64; 6]) -> i64 {
    let (clock, flags, req, rem) = (a[0], a[1], a[2], a[3]);
    let realtime = match clock {
        0 | 5 | 8 => true,          // REALTIME, REALTIME_COARSE, REALTIME_ALARM
        1 | 4 | 6 | 7 | 9 => false, // MONOTONIC(_RAW/_COARSE), BOOTTIME(_ALARM)
        2 | 3 => return -95,        // CPU-time clocks: EOPNOTSUPP
        _ => return -(EINVAL as i64),
    };
    let ns = match read_timespec(req) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let abs = flags & TIMER_ABSTIME != 0;
    let deadline = if abs {
        absolute(ns, realtime)
    } else {
        after(ns)
    };
    match sleep_until(deadline) {
        Ok(()) => 0,
        Err(left) => {
            if !abs && rem != 0 {
                write_timespec(rem, left);
            }
            -(EINTR as i64)
        }
    }
}
