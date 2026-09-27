//! POSIX per-process timers: `timer_create`, `timer_settime`,
//! `timer_gettime`, `timer_getoverrun` and `timer_delete`.
//!
//! Expiries are one-shot zero-leeway kqueue timers on `park`'s timer
//! kqueue (a periodic timer is re-armed at each expiry), so they are as
//! precise as the layer's sleeps. An expiry queues the timer's signal
//! (`SIGEV_SIGNAL` to the process, `SIGEV_THREAD_ID` to one thread) with
//! `si_code` SI_TIMER. As on Linux, a timer has at most one signal queued:
//! expiries while it is pending count as overruns, reported in `si_overrun`
//! and by `timer_getoverrun` once the signal is taken.
//!
//! Timers are per process: a fork child has none.

use std::sync::{Mutex, MutexGuard};

use super::park::{self, PTIMER_EXPIRY};
use super::sigframe::Siginfo;
use crate::errno::{EINVAL, ENOMEM};

pub const SI_TIMER: i32 = -2;
const SIGEV_SIGNAL: i32 = 0;
const SIGEV_NONE: i32 = 1;
const SIGEV_THREAD: i32 = 2;
const SIGEV_THREAD_ID: i32 = 4;
const SIGALRM: i32 = 14;
const NSIG: i32 = 64;
const TIMER_ABSTIME: u64 = 1;
/// Timers per process (Linux bounds them by RLIMIT_SIGPENDING).
const MAX_TIMERS: usize = 4096;

pub struct Timer {
    id: i32,
    realtime: bool,
    /// None for SIGEV_NONE.
    signo: Option<i32>,
    value: u64,
    /// The SIGEV_THREAD_ID target.
    tid: Option<i32>,
    /// Monotonic deadline of the next expiry; 0 when disarmed.
    next: u64,
    interval: u64,
    /// Expiries not signalled while the signal was queued.
    overrun: u64,
    /// `overrun` when the last signal was taken.
    overrun_last: u64,
    queued: bool,
    /// The kqueue event of the pending expiry: (kqueue, ident).
    armed: Option<(i32, u64)>,
}

static TIMERS: Mutex<Vec<Timer>> = Mutex::new(Vec::new());

fn timers() -> MutexGuard<'static, Vec<Timer>> {
    TIMERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Arm the kqueue for `id`'s deadline `next` (outside the table lock: the
/// timer kqueue's lock is taken before this one across a fork).
fn arm(id: i32, ident: u64, next: u64) {
    let Some(kq) = park::arm_event(ident, PTIMER_EXPIRY, next) else {
        return;
    };
    let current = match timers()
        .iter_mut()
        .find(|t| t.armed.is_some_and(|a| a.1 == ident) && t.id == id)
    {
        Some(tm) => {
            tm.armed = Some((kq, ident));
            true
        }
        None => false,
    };
    if !current {
        // Re-set or deleted meanwhile.
        park::disarm((kq, ident));
    }
}

/// Expirations due at `now` and the next deadline.
fn advance(next: u64, interval: u64, now: u64) -> (u64, u64) {
    if next == 0 || now < next {
        return (0, next);
    }
    if interval == 0 {
        return (1, 0);
    }
    let n = 1 + (now - next) / interval;
    (n, next + n * interval)
}

/// The timer thread: the kqueue event `ident` fired.
pub fn expired(ident: u64) {
    let now = park::monotonic();
    let (info, tid, rearm) = {
        let mut t = timers();
        let Some(tm) = t.iter_mut().find(|t| t.armed.is_some_and(|a| a.1 == ident)) else {
            return;
        };
        // Zero leeway fires on time; the monotonic clock may lag slightly.
        let (n, next) = advance(tm.next, tm.interval, now.max(tm.next));
        tm.next = next;
        tm.armed = None;
        let info = match tm.signo {
            Some(sig) if !tm.queued => {
                tm.queued = true;
                tm.overrun = tm.overrun.saturating_add(n - 1);
                let mut i = Siginfo::new(sig, SI_TIMER);
                i.fields[0] = tm.id as u32 as u64;
                i.fields[1] = tm.value;
                Some(i)
            }
            _ => {
                tm.overrun = tm.overrun.saturating_add(n);
                None
            }
        };
        let rearm = (next != 0).then(|| {
            let ident = park::timer_id();
            tm.armed = Some((-1, ident));
            (tm.id, ident, next)
        });
        (info, tm.tid, rearm)
    };
    if let Some(info) = info {
        let id = info.fields[0] as i32;
        if !super::signal::send_timer(info, tid)
            && let Some(tm) = timers().iter_mut().find(|t| t.id == id)
        {
            tm.queued = false;
        }
    }
    if let Some((id, ident, next)) = rearm {
        arm(id, ident, next);
    }
}

/// A timer's signal was taken off a queue (or discarded): fill in its
/// overrun count, and let the timer queue its next one.
pub fn dequeued(info: &mut Siginfo) {
    let id = info.fields[0] as u32 as i32;
    let mut t = timers();
    if let Some(tm) = t.iter_mut().find(|t| t.id == id && t.queued) {
        tm.queued = false;
        tm.overrun_last = tm.overrun;
        tm.overrun = 0;
        let o = tm.overrun_last.min(i32::MAX as u64);
        info.fields[0] = id as u32 as u64 | o << 32;
    }
}

/// Held across a fork (see `thread::fork_prepare`), after the signal locks.
pub fn fork_lock() -> MutexGuard<'static, Vec<Timer>> {
    timers()
}

/// Fork child: timers are not inherited (and the timer kqueue is gone).
pub fn fork_child(mut t: MutexGuard<'static, Vec<Timer>>) {
    t.clear();
}

fn clock_realtime(clock: u64) -> Result<bool, i64> {
    match clock {
        0 | 8 => Ok(true),      // REALTIME(_ALARM)
        1 | 7 | 9 => Ok(false), // MONOTONIC, BOOTTIME(_ALARM)
        _ => Err(-(EINVAL as i64)),
    }
}

/// timer_create(clock, sevp, timerid).
pub fn timer_create(a: [u64; 6]) -> i64 {
    let realtime = match clock_realtime(a[0]) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // Kernel struct sigevent: value, signo, notify, then the thread id.
    let (value, signo, notify, tid) = if a[1] == 0 {
        (None, SIGALRM, SIGEV_SIGNAL, 0)
    } else {
        // SAFETY: guest struct sigevent.
        unsafe {
            (
                Some((a[1] as *const u64).read_unaligned()),
                ((a[1] + 8) as *const i32).read_unaligned(),
                ((a[1] + 12) as *const i32).read_unaligned(),
                ((a[1] + 16) as *const i32).read_unaligned(),
            )
        }
    };
    let target = match notify {
        SIGEV_NONE => None,
        SIGEV_SIGNAL | SIGEV_THREAD => Some(None),
        SIGEV_THREAD_ID => {
            if super::thread::find(tid).is_none() {
                return -(EINVAL as i64);
            }
            Some(Some(tid))
        }
        _ => return -(EINVAL as i64),
    };
    if target.is_some() && !(1..=NSIG).contains(&signo) {
        return -(EINVAL as i64);
    }
    let mut t = timers();
    if t.len() >= MAX_TIMERS {
        return -(ENOMEM as i64);
    }
    let id = (0..).find(|i| !t.iter().any(|tm| tm.id == *i)).unwrap_or(0);
    t.push(Timer {
        id,
        realtime,
        signo: target.map(|_| signo),
        value: value.unwrap_or(id as u64),
        tid: target.flatten(),
        next: 0,
        interval: 0,
        overrun: 0,
        overrun_last: 0,
        queued: false,
        armed: None,
    });
    // SAFETY: guest timer_t (int).
    unsafe { (a[2] as *mut i32).write_unaligned(id) };
    0
}

/// `struct itimerspec` { it_interval, it_value } of `tm` at `now`.
fn itimerspec(tm: &Timer, now: u64) -> [u64; 2] {
    let (_, next) = advance(tm.next, tm.interval, now);
    let left = if next == 0 {
        0
    } else {
        next.saturating_sub(now).max(1)
    };
    [tm.interval, left]
}

fn write_itimerspec(p: u64, v: [u64; 2]) {
    park::write_timespec(p, v[0]);
    park::write_timespec(p + 16, v[1]);
}

/// timer_settime(id, flags, new, old).
pub fn timer_settime(a: [u64; 6]) -> i64 {
    let (id, flags, new, old) = (a[0] as i32, a[1], a[2], a[3]);
    let (interval, value) = match (park::read_timespec(new), park::read_timespec(new + 16)) {
        (Ok(i), Ok(v)) => (i, v),
        _ => return -(EINVAL as i64),
    };
    let now = park::monotonic();
    let (prev, rearm) = {
        let mut t = timers();
        let Some(tm) = t.iter_mut().find(|t| t.id == id) else {
            return -(EINVAL as i64);
        };
        if old != 0 {
            write_itimerspec(old, itimerspec(tm, now));
        }
        tm.interval = interval;
        tm.next = match value {
            0 => 0,
            v if flags & TIMER_ABSTIME == 0 => now.saturating_add(v),
            v => park::absolute(v, tm.realtime).max(1),
        };
        tm.overrun = 0;
        let prev = tm.armed.take();
        let rearm = (tm.next != 0 && tm.signo.is_some()).then(|| {
            let ident = park::timer_id();
            tm.armed = Some((-1, ident));
            (ident, tm.next)
        });
        (prev, rearm)
    };
    if let Some(p) = prev.filter(|p| p.0 >= 0) {
        park::disarm(p);
    }
    if let Some((ident, next)) = rearm {
        arm(id, ident, next);
    }
    0
}

/// timer_gettime(id, curr).
pub fn timer_gettime(a: [u64; 6]) -> i64 {
    let t = timers();
    let Some(tm) = t.iter().find(|t| t.id == a[0] as i32) else {
        return -(EINVAL as i64);
    };
    write_itimerspec(a[1], itimerspec(tm, park::monotonic()));
    0
}

/// timer_getoverrun(id).
pub fn timer_getoverrun(a: [u64; 6]) -> i64 {
    match timers().iter().find(|t| t.id == a[0] as i32) {
        Some(tm) => tm.overrun_last.min(i32::MAX as u64) as i64,
        None => -(EINVAL as i64),
    }
}

/// timer_delete(id).
pub fn timer_delete(a: [u64; 6]) -> i64 {
    let armed = {
        let mut t = timers();
        let Some(i) = t.iter().position(|t| t.id == a[0] as i32) else {
            return -(EINVAL as i64);
        };
        t.remove(i).armed
    };
    if let Some(p) = armed.filter(|p| p.0 >= 0) {
        park::disarm(p);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expirations_and_overruns() {
        assert_eq!(advance(0, 10, 100), (0, 0));
        assert_eq!(advance(100, 0, 99), (0, 100));
        assert_eq!(advance(100, 0, 150), (1, 0));
        assert_eq!(advance(100, 10, 100), (1, 110));
        assert_eq!(advance(100, 10, 135), (4, 140));
    }

    #[test]
    fn create_set_get_delete() {
        let mut id = -1i32;
        let ev: [u64; 8] = [7, (SIGEV_NONE as u64) << 32, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            timer_create([1, ev.as_ptr() as u64, &mut id as *mut i32 as u64, 0, 0, 0]),
            0
        );
        assert_eq!(
            timer_create([2, ev.as_ptr() as u64, &mut id as *mut i32 as u64, 0, 0, 0]),
            -(EINVAL as i64)
        );
        // 1 s, then every 250 ms.
        let spec: [i64; 4] = [0, 250_000_000, 1, 0];
        assert_eq!(
            timer_settime([id as u64, 0, spec.as_ptr() as u64, 0, 0, 0]),
            0
        );
        let mut cur = [0i64; 4];
        assert_eq!(
            timer_gettime([id as u64, cur.as_mut_ptr() as u64, 0, 0, 0, 0]),
            0
        );
        assert_eq!(cur[..2], [0, 250_000_000]);
        let left = cur[2] * 1_000_000_000 + cur[3];
        assert!(left > 900_000_000 && left <= 1_000_000_000);
        assert_eq!(timer_getoverrun([id as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(timer_delete([id as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(timer_delete([id as u64, 0, 0, 0, 0, 0]), -(EINVAL as i64));
    }
}
