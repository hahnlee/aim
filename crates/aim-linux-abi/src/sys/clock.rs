//! Linux clocks: clock_gettime, clock_getres, gettimeofday, and the clock
//! a timer or sleep is set on.
//!
//! - MONOTONIC, MONOTONIC_RAW and MONOTONIC_COARSE stop while the host
//!   sleeps and BOOTTIME (and BOOTTIME_ALARM) does not; both count from the
//!   host's boot in nanoseconds (`aim_hostcall::clock`, which host modules
//!   stamp events with).
//! - REALTIME, its COARSE and ALARM variants and TAI (offset 0, as until
//!   an NTP daemon sets one) are the host's wall clock. Darwin keeps it in
//!   microseconds; the nanoseconds come from the monotonic clock, kept
//!   within the wall clock's microsecond.
//! - CPU clocks: PROCESS_CPUTIME_ID, THREAD_CPUTIME_ID and the encoded ids
//!   of `clock_getcpuclockid`/`pthread_getcpuclockid` (a pid, or a tid of
//!   the caller's process).
//!
//! Resolutions are Linux's with high-resolution timers: 1 ns, except the
//! COARSE clocks and the tick-based CPU clocks (PROF, VIRT), which have a
//! tick of a CONFIG_HZ=250 kernel. There is no vDSO: each read is a
//! syscall, and the host reads cost 10-20 ns of it.
//!
//! Timers and sleeps keep their deadlines on the monotonic clock
//! ([`Base::deadline`]).

use std::sync::atomic::{AtomicI64, Ordering::Relaxed};

use aim_hostcall::clock::{boottime_ns, monotonic_ns, ticks_to_ns};

use super::park::write_timespec;
use crate::errno::{EINVAL, EPERM};

unsafe extern "C" {
    fn clock_gettime_nsec_np(clock: libc::clockid_t) -> u64;
    static mach_task_self_: libc::mach_port_t;
}

const REALTIME: u64 = 0;
const MONOTONIC: u64 = 1;
const PROCESS_CPUTIME_ID: u64 = 2;
const THREAD_CPUTIME_ID: u64 = 3;
const MONOTONIC_RAW: u64 = 4;
const REALTIME_COARSE: u64 = 5;
const MONOTONIC_COARSE: u64 = 6;
const BOOTTIME: u64 = 7;
const REALTIME_ALARM: u64 = 8;
const BOOTTIME_ALARM: u64 = 9;
const TAI: u64 = 11;

/// `CPUCLOCK_WHICH` of an encoded CPU clock (0 is PROF, user and system
/// time).
const CPUCLOCK_VIRT: i64 = 1;
const CPUCLOCK_SCHED: i64 = 2;
const CPUCLOCK_PERTHREAD: i64 = 4;

/// TICK_NSEC at CONFIG_HZ=250, as GKI kernels are built.
const TICK_NS: u64 = 4_000_000;
const CAP_WAKE_ALARM: u32 = 35;
const EOPNOTSUPP: i64 = 95;

/// A clock timers and sleeps can be set on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Base {
    Realtime,
    Monotonic,
    Boottime,
}

impl Base {
    pub fn now(self) -> u64 {
        (match self {
            Base::Realtime => realtime_ns(),
            Base::Monotonic => monotonic_ns(),
            Base::Boottime => boottime_ns(),
        }) as u64
    }

    /// An absolute time on this clock as a monotonic deadline. A REALTIME
    /// or BOOTTIME deadline is converted once, when it is set.
    pub fn deadline(self, ns: u64) -> u64 {
        match self {
            Base::Monotonic => ns,
            _ => (monotonic_ns() as u64).saturating_add(ns.saturating_sub(self.now())),
        }
    }

    /// For fork state.
    pub fn index(self) -> u32 {
        self as u32
    }

    pub fn from_index(i: u32) -> Base {
        [Base::Realtime, Base::Monotonic, Base::Boottime][i.min(2) as usize]
    }
}

/// The clock a timer (timer_create, timerfd_create) or clock_nanosleep
/// takes `id` to: EINVAL for an unknown clock, EOPNOTSUPP for one that
/// cannot be slept on, EPERM for an alarm clock without CAP_WAKE_ALARM.
/// timerfd also refuses TAI; its caller checks.
pub fn timer_base(id: u64) -> Result<Base, i64> {
    let alarm = matches!(id, REALTIME_ALARM | BOOTTIME_ALARM);
    let base = match id {
        REALTIME | REALTIME_ALARM | TAI => Base::Realtime,
        MONOTONIC => Base::Monotonic,
        BOOTTIME | BOOTTIME_ALARM => Base::Boottime,
        _ if cpu_clock(id).is_some() => return Err(-EOPNOTSUPP),
        MONOTONIC_RAW | REALTIME_COARSE | MONOTONIC_COARSE => return Err(-EOPNOTSUPP),
        _ => return Err(-(EINVAL as i64)),
    };
    if alarm && !super::cred::capable(CAP_WAKE_ALARM) {
        return Err(-(EPERM as i64));
    }
    Ok(base)
}

/// The host's wall clock in nanoseconds: the monotonic clock plus an
/// offset re-taken whenever the result leaves the wall clock's current
/// microsecond (a step, a slew, or the first read).
pub fn realtime_ns() -> i64 {
    static OFFSET: AtomicI64 = AtomicI64::new(0);
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    // SAFETY: fills the local timeval.
    unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };
    let wall = tv.tv_sec * 1_000_000_000 + tv.tv_usec as i64 * 1000;
    let mono = monotonic_ns();
    let t = mono + OFFSET.load(Relaxed);
    if (wall..wall + 1000).contains(&t) {
        return t;
    }
    OFFSET.store(wall - mono, Relaxed);
    wall
}

/// An encoded CPU clock: (pid or tid, 0 for the caller; per thread;
/// CPUCLOCK_WHICH).
fn cpu_clock(id: u64) -> Option<(i32, bool, i64)> {
    let id = id as i32 as i64;
    match id as u64 {
        PROCESS_CPUTIME_ID => Some((0, false, CPUCLOCK_SCHED)),
        THREAD_CPUTIME_ID => Some((0, true, CPUCLOCK_SCHED)),
        _ if id < 0 => Some(((!(id >> 3)) as i32, id & CPUCLOCK_PERTHREAD != 0, id & 3)),
        _ => None,
    }
}

/// User and system time of a Mach thread, in nanoseconds.
pub fn thread_times(port: libc::mach_port_t) -> Option<(u64, u64)> {
    // SAFETY: THREAD_EXTENDED_INFO into a local of the right size.
    unsafe {
        let mut info: libc::thread_extended_info = std::mem::zeroed();
        let mut count = libc::THREAD_EXTENDED_INFO_COUNT;
        let kr = libc::thread_info(
            port,
            libc::THREAD_EXTENDED_INFO as u32,
            (&mut info as *mut libc::thread_extended_info).cast(),
            &mut count,
        );
        (kr == 0).then_some((info.pth_user_time, info.pth_system_time))
    }
}

/// User and system time of host process `pid`, in nanoseconds.
fn process_times(pid: i32) -> Option<(u64, u64)> {
    // SAFETY: RUSAGE_INFO_V0 into a local of the right type.
    unsafe {
        let mut ri: libc::rusage_info_v0 = std::mem::zeroed();
        let r = libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V0,
            (&mut ri as *mut libc::rusage_info_v0).cast(),
        );
        (r == 0).then(|| {
            (
                ticks_to_ns(ri.ri_user_time) as u64,
                ticks_to_ns(ri.ri_system_time) as u64,
            )
        })
    }
}

/// `TASK_ABSOLUTETIME_INFO`, in host ticks.
#[repr(C)]
#[derive(Default)]
struct TaskAbsolutetimeInfo {
    total_user: u64,
    total_system: u64,
    threads_user: u64,
    threads_system: u64,
}

const TASK_ABSOLUTETIME_INFO: u32 = 1;

/// This process's user and system time: its live threads' (current, and
/// like Linux's counting what a thread ran before exec) and its exited
/// threads'. Mach reports live threads in microseconds; the caller's own
/// share, read before the total, is replaced by its exact time, read
/// after it, so the total is never behind the caller's thread clock.
fn own_process_times() -> Option<(u64, u64)> {
    let mut abs = TaskAbsolutetimeInfo::default();
    // SAFETY: all-zero Mach info structs are valid.
    let (mut live, mut me): (libc::task_thread_times_info, libc::thread_basic_info) =
        unsafe { std::mem::zeroed() };
    // SAFETY: Mach info calls into locals of the flavours' sizes.
    let ok = unsafe {
        let mut n = libc::THREAD_BASIC_INFO_COUNT;
        let port = libc::pthread_mach_thread_np(libc::pthread_self());
        let r = libc::thread_info(
            port,
            libc::THREAD_BASIC_INFO as u32,
            (&raw mut me).cast(),
            &mut n,
        );
        let task = mach_task_self_;
        let mut n = (size_of::<TaskAbsolutetimeInfo>() / 4) as u32;
        let a = libc::task_info(task, TASK_ABSOLUTETIME_INFO, (&raw mut abs).cast(), &mut n);
        let mut n = libc::TASK_THREAD_TIMES_INFO_COUNT;
        let b = libc::task_info(
            task,
            libc::TASK_THREAD_TIMES_INFO,
            (&raw mut live).cast(),
            &mut n,
        );
        r == 0 && a == 0 && b == 0
    };
    // SAFETY: reads the clock.
    let exact = unsafe { clock_gettime_nsec_np(libc::CLOCK_THREAD_CPUTIME_ID) };
    let ns =
        |t: libc::time_value_t| t.seconds as u64 * 1_000_000_000 + t.microseconds as u64 * 1000;
    let others_user = ns(live.user_time).saturating_sub(ns(me.user_time));
    let others_system = ns(live.system_time).saturating_sub(ns(me.system_time));
    ok.then(|| {
        (
            ticks_to_ns(abs.total_user - abs.threads_user) as u64
                + others_user
                + exact.saturating_sub(ns(me.system_time)),
            ticks_to_ns(abs.total_system - abs.threads_system) as u64
                + others_system
                + ns(me.system_time),
        )
    })
}

/// The time on CPU clock `c`, or EINVAL when it names no process or no
/// thread of the caller's. `gettime`: a process clock may name the
/// caller's tid (Linux's `pid_for_clock`).
fn cpu_ns((id, thread, which): (i32, bool, i64), gettime: bool) -> Result<u64, i64> {
    let tid = super::thread::gettid() as i32;
    let (user, system) = match (thread, id) {
        _ if which > CPUCLOCK_SCHED => None,
        (true, 0) => {
            if which == CPUCLOCK_SCHED {
                // Full resolution for the common case.
                // SAFETY: reads the clock.
                return Ok(unsafe { clock_gettime_nsec_np(libc::CLOCK_THREAD_CPUTIME_ID) });
            }
            // SAFETY: the calling thread's own port.
            thread_times(unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) })
        }
        (true, _) if id == tid => return cpu_ns((0, true, which), gettime),
        (true, _) => super::thread::cpu_times(id),
        (false, _) if id == 0 || id == std::process::id() as i32 => own_process_times(),
        (false, _) if gettime && id == tid => own_process_times(),
        (false, _) if super::pidns::contains(id) => process_times(id),
        (false, _) => None,
    }
    .ok_or(-(EINVAL as i64))?;
    Ok(match which {
        CPUCLOCK_VIRT => user,
        _ => user + system,
    })
}

/// The time on clock `id` in nanoseconds, or -errno.
fn read(id: u64) -> Result<u64, i64> {
    match id {
        REALTIME | REALTIME_COARSE | REALTIME_ALARM | TAI => Ok(realtime_ns() as u64),
        MONOTONIC | MONOTONIC_RAW | MONOTONIC_COARSE => Ok(monotonic_ns() as u64),
        BOOTTIME | BOOTTIME_ALARM => Ok(boottime_ns() as u64),
        _ => cpu_ns(cpu_clock(id).ok_or(-(EINVAL as i64))?, true),
    }
}

// struct timespec is { i64 tv_sec; i64 tv_nsec; } on both kernels.
pub fn clock_gettime(a: [u64; 6]) -> i64 {
    match read(a[0]) {
        Ok(ns) => {
            write_timespec(a[1], ns);
            0
        }
        Err(e) => e,
    }
}

pub fn clock_getres(a: [u64; 6]) -> i64 {
    let res = match a[0] {
        REALTIME | MONOTONIC | MONOTONIC_RAW | BOOTTIME | REALTIME_ALARM | BOOTTIME_ALARM | TAI => {
            1
        }
        REALTIME_COARSE | MONOTONIC_COARSE => TICK_NS,
        id => match cpu_clock(id) {
            Some(c) => match cpu_ns(c, false) {
                Ok(_) if c.2 == CPUCLOCK_SCHED => 1,
                Ok(_) => TICK_NS,
                Err(e) => return e,
            },
            None => return -(EINVAL as i64),
        },
    };
    if a[1] != 0 {
        write_timespec(a[1], res);
    }
    0
}

pub fn gettimeofday(a: [u64; 6]) -> i64 {
    if a[1] != 0 {
        // SAFETY: guest struct timezone { int; int }: UTC, no DST, as
        // Android's kernel timezone stays.
        unsafe { (a[1] as *mut [i32; 2]).write_unaligned([0, 0]) };
    }
    if a[0] != 0 {
        let ns = realtime_ns();
        // SAFETY: guest struct timeval is { i64; i64 }.
        unsafe {
            (a[0] as *mut [i64; 2]).write_unaligned([ns / 1_000_000_000, ns % 1_000_000_000 / 1000])
        };
    }
    0
}
