//! Interval timers: `setitimer` and `getitimer` (setitimer(2); bionic's
//! `alarm` is `setitimer(ITIMER_REAL)`).
//!
//! - ITIMER_REAL counts real (monotonic) time and sends SIGALRM.
//! - ITIMER_VIRTUAL counts the process's user CPU time and sends SIGVTALRM,
//!   ITIMER_PROF its user and system time and sends SIGPROF.
//!
//! One host thread, started with the first armed timer, sleeps until the
//! nearest expiry. CPU time cannot pass faster than wall time on every CPU
//! at once, so a CPU timer is checked again after its remaining budget
//! divided by the CPU count (at least a tick): it fires within a tick of
//! its expiry, as a tick-driven kernel fires it. Signals carry
//! `si_code` SI_KERNEL. A fork child has no timers (the thread and the
//! table are the parent's); execve keeps them (`--itimers`).

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use super::clock::TICK_NS;
use super::sigframe::Siginfo;
use crate::errno::{EFAULT, EINVAL};

const ITIMER_REAL: usize = 0;
const ITIMER_VIRTUAL: usize = 1;
const ITIMER_PROF: usize = 2;
const SIGNALS: [i32; 3] = [14, 26, 27];
const SI_KERNEL: i32 = 0x80;
const NS_PER_US: u64 = 1_000;

#[derive(Clone, Copy, Default)]
struct Timer {
    interval: u64,
    /// Expiry on the timer's clock (monotonic or CPU time), in ns; 0 when
    /// disarmed.
    next: u64,
}

struct Timers {
    t: [Timer; 3],
    started: bool,
}

static TIMERS: Mutex<Timers> = Mutex::new(Timers {
    t: [Timer {
        interval: 0,
        next: 0,
    }; 3],
    started: false,
});
static CHANGED: Condvar = Condvar::new();

fn timers() -> MutexGuard<'static, Timers> {
    TIMERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The latest CPU times read, so each CPU clock reads as never going back
/// (Mach reports live threads' times in microseconds, the caller's exact).
static CPU_SEEN: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Now on timer `which`'s clock.
fn now(which: usize) -> u64 {
    if which == ITIMER_REAL {
        return super::park::monotonic();
    }
    let (user, system) = super::clock::own_process_times().unwrap_or((0, 0));
    let t = if which == ITIMER_VIRTUAL {
        user
    } else {
        user + system
    };
    let seen = &CPU_SEEN[which - 1];
    seen.fetch_max(t, Relaxed).max(t)
}

/// The first expiry after `now` of a timer due at `next`, or 0 when it
/// does not repeat.
fn after(next: u64, interval: u64, now: u64) -> u64 {
    if interval == 0 {
        return 0;
    }
    next + (1 + (now - next) / interval) * interval
}

fn cpus() -> u64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u64)
}

/// The timer thread: fire what is due, then sleep until the next check.
fn run() {
    let mut g = timers();
    loop {
        let mut wait: Option<u64> = None;
        for (which, &sig) in SIGNALS.iter().enumerate() {
            let tm = g.t[which];
            if tm.next == 0 {
                continue;
            }
            let t = now(which);
            if t >= tm.next {
                g.t[which].next = after(tm.next, tm.interval, t);
                super::signal::send_timer(Siginfo::new(sig, SI_KERNEL), None);
                if g.t[which].next == 0 {
                    continue;
                }
            }
            let left = g.t[which].next - t.min(g.t[which].next);
            let sleep = if which == ITIMER_REAL {
                left
            } else {
                (left / cpus()).max(TICK_NS)
            };
            wait = Some(wait.map_or(sleep, |w| w.min(sleep)));
        }
        g = match wait {
            None => CHANGED.wait(g).unwrap_or_else(|e| e.into_inner()),
            Some(ns) => {
                CHANGED
                    .wait_timeout(g, Duration::from_nanos(ns))
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            }
        };
    }
}

/// Arm (or disarm, with `value` 0) timer `which`; its previous interval
/// and remaining time.
fn set(which: usize, interval: u64, value: u64) -> [u64; 2] {
    let mut g = timers();
    let old = remaining(&g, which);
    g.t[which] = Timer {
        interval,
        next: if value == 0 { 0 } else { now(which) + value },
    };
    if value != 0 && !g.started {
        let spawned = std::thread::Builder::new().name("itimer".into()).spawn(run);
        match spawned {
            Ok(_) => g.started = true,
            Err(e) => crate::diag!("[linux-abi] setitimer: no timer thread: {e}"),
        }
    }
    CHANGED.notify_all();
    old
}

/// Interval and time left of timer `which`. A timer whose expiry is due
/// but not yet signalled reports the least it can: a microsecond of real
/// time, a tick of CPU time (as `itimer_get_remtime` and `get_cpu_itimer`).
fn remaining(g: &Timers, which: usize) -> [u64; 2] {
    let tm = g.t[which];
    if tm.next == 0 {
        return [tm.interval, 0];
    }
    let t = now(which);
    let least = if which == ITIMER_REAL {
        NS_PER_US
    } else {
        TICK_NS
    };
    [tm.interval, tm.next.saturating_sub(t).max(least)]
}

/// Linux `struct itimerval` { it_interval, it_value }, each a `timeval`.
fn read_itimerval(p: u64) -> Result<[u64; 2], i64> {
    // SAFETY: guest struct itimerval (four longs).
    let v = unsafe { (p as *const [i64; 4]).read_unaligned() };
    let ns = |sec: i64, usec: i64| {
        if sec < 0 || !(0..1_000_000).contains(&usec) {
            return Err(-(EINVAL as i64));
        }
        Ok((sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(usec as u64 * NS_PER_US))
    };
    Ok([ns(v[0], v[1])?, ns(v[2], v[3])?])
}

/// A time in ns as a `timeval`, rounded up to the microsecond.
fn timeval(ns: u64) -> [i64; 2] {
    let us = ns.div_ceil(NS_PER_US);
    [(us / 1_000_000) as i64, (us % 1_000_000) as i64]
}

fn write_itimerval(p: u64, v: [u64; 2]) {
    let (i, r) = (timeval(v[0]), timeval(v[1]));
    // SAFETY: guest struct itimerval.
    unsafe { (p as *mut [i64; 4]).write_unaligned([i[0], i[1], r[0], r[1]]) };
}

fn which(w: u64) -> Result<usize, i64> {
    match w as usize {
        w @ (ITIMER_REAL | ITIMER_VIRTUAL | ITIMER_PROF) => Ok(w),
        _ => Err(-(EINVAL as i64)),
    }
}

/// setitimer(which, new, old).
pub fn setitimer(a: [u64; 6]) -> i64 {
    let w = match which(a[0]) {
        Ok(w) => w,
        Err(e) => return e,
    };
    // A null new value disarms, as Linux has allowed since 2.6.
    let [interval, value] = if a[1] == 0 {
        [0, 0]
    } else {
        match read_itimerval(a[1]) {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
    let old = set(w, interval, value);
    if a[2] != 0 {
        write_itimerval(a[2], old);
    }
    0
}

/// getitimer(which, curr).
pub fn getitimer(a: [u64; 6]) -> i64 {
    let w = match which(a[0]) {
        Ok(w) => w,
        Err(e) => return e,
    };
    if a[1] == 0 {
        return -(EFAULT as i64);
    }
    write_itimerval(a[1], remaining(&timers(), w));
    0
}

/// execve: the armed timers, as `which:interval:remaining` in ns, for the
/// new image (`--itimers`).
pub fn exec_text() -> String {
    let g = timers();
    (0..3)
        .filter(|&w| g.t[w].next != 0)
        .map(|w| {
            let [i, r] = remaining(&g, w);
            format!("{w}:{i}:{r}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// After execve: arm the timers the old image had.
pub fn exec_restore(text: &str) {
    for t in text.split(',') {
        let v: Vec<u64> = t.split(':').filter_map(|n| n.parse().ok()).collect();
        if let [w, i, r] = v[..]
            && let Ok(w) = which(w)
        {
            set(w, i, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiries_repeat_past_now() {
        assert_eq!(after(100, 0, 150), 0);
        assert_eq!(after(100, 10, 100), 110);
        assert_eq!(after(100, 10, 135), 140);
    }

    #[test]
    fn itimerval_round_trips_and_rejects_bad_usec() {
        let mut v = [0i64, 250_000, 1, 500_001];
        assert_eq!(
            read_itimerval(v.as_ptr() as u64),
            Ok([250_000_000, 1_500_001_000])
        );
        write_itimerval(v.as_mut_ptr() as u64, [1, 1_500_001_000]);
        assert_eq!(v, [0, 1, 1, 500_001]);
        let bad = [0i64, 1_000_000, 0, 0];
        assert_eq!(read_itimerval(bad.as_ptr() as u64), Err(-(EINVAL as i64)));
        let neg = [0i64, 0, -1, 0];
        assert_eq!(read_itimerval(neg.as_ptr() as u64), Err(-(EINVAL as i64)));
    }
}
