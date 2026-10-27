//! Linux clocks: clock_gettime, clock_getres, gettimeofday, and the clock
//! a timer or sleep is set on.
//!
//! - MONOTONIC, MONOTONIC_RAW and MONOTONIC_COARSE stop while the host
//!   sleeps and BOOTTIME (and BOOTTIME_ALARM) does not; both count from the
//!   host's boot in nanoseconds (`aim_hostcall::clock`, which host modules
//!   stamp events with).
//! - REALTIME, its COARSE and ALARM variants and TAI (offset 0, as until
//!   an NTP daemon sets one) are the host's wall clock, to the nanosecond.
//! - CPU clocks: PROCESS_CPUTIME_ID, THREAD_CPUTIME_ID and the encoded ids
//!   of `clock_getcpuclockid`/`pthread_getcpuclockid` (a pid, or a tid of
//!   the caller's process).
//!
//! Resolutions are Linux's with high-resolution timers: 1 ns, except the
//! COARSE clocks and the tick-based CPU clocks (PROF, VIRT), which have a
//! tick of a CONFIG_HZ=250 kernel. The vDSO (`crate::vdso`) serves the
//! MONOTONIC, BOOTTIME and REALTIME reads without a syscall, from the same
//! sources and with the same arithmetic.
//!
//! Timers and sleeps keep their deadlines on their own clock
//! ([`Deadline`]).

use aim_hostcall::clock::{boottime_ns, monotonic_ns, ticks_to_ns};

use super::park::write_timespec;
use crate::errno::{EINVAL, EPERM};

unsafe extern "C" {
    fn clock_gettime_nsec_np(clock: libc::clockid_t) -> u64;
    fn mach_absolute_time() -> u64;
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
pub(crate) const TICK_NS: u64 = 4_000_000;
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

    /// Whether the clock runs on while the Mac sleeps.
    pub fn counts_sleep(self) -> bool {
        self != Base::Monotonic
    }

    /// The clock a timer set on this one runs on: `__hrtimer_init` moves a
    /// relative REALTIME timer to MONOTONIC, so a wall-clock step does not
    /// move it.
    pub fn timer(self, absolute: bool) -> Base {
        match self {
            Base::Realtime if !absolute => Base::Monotonic,
            b => b,
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

/// A time a timer or sleep waits for, kept on its own clock as an hrtimer
/// is on its clock's base: a BOOTTIME or REALTIME deadline passes while
/// the Mac sleeps, and a MONOTONIC one does not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Deadline {
    pub base: Base,
    pub ns: u64,
}

impl Deadline {
    /// `ns` from now on `base`.
    pub fn after(base: Base, ns: u64) -> Deadline {
        Deadline {
            base,
            ns: base.now().saturating_add(ns),
        }
    }

    /// The time left, 0 once it has passed.
    pub fn left(self) -> u64 {
        self.ns.saturating_sub(self.base.now())
    }

    pub fn passed(self) -> bool {
        self.left() == 0
    }
}

/// The wall-clock base of a timer or sleep: EINVAL for an unknown clock,
/// EOPNOTSUPP for clocks without a wall-clock base, EPERM for an alarm
/// clock without CAP_WAKE_ALARM. POSIX CPU timers use TimerClock instead;
/// timerfd also refuses TAI (its caller checks).
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

/// Darwin's commpage (xnu `osfmk/arm/cpu_capabilities.h`), mapped read-only
/// into every process: the clock state libsystem's `mach_absolute_time`,
/// `mach_continuous_time` and `gettimeofday` read without a trap. The vDSO
/// reads it too.
pub mod commpage {
    pub const BASE: u64 = 0xf_ffff_c000;
    /// u64 added to the counter for `mach_absolute_time`; changes on wake.
    pub const TIMEBASE_OFFSET: u64 = 0x88;
    /// u8: how user space reads the counter; 0 when it cannot.
    pub const USER_TIMEBASE: u64 = 0x90;
    /// u8: the counter keeps running across sleep.
    pub const CONT_HWCLOCK: u64 = 0x91;
    /// u64 added to the counter for `mach_continuous_time` when it does.
    pub const CONT_HW_TIMEBASE: u64 = 0xa8;
    /// The wall clock at a tick: tick, seconds, a 64-bit fraction of a
    /// second, the fraction per tick, and ticks per second (u64 each).
    pub const TIMEOFDAY: u64 = 0x120;
}

/// The host's wall clock in nanoseconds, as Darwin's `gettimeofday`
/// computes it from the commpage but to the nanosecond: the kernel's
/// timestamp plus the ticks since it at the kernel's rate. A timestamp a
/// second old is refreshed by the `gettimeofday` syscall, which libsystem
/// makes when it finds it so.
pub fn realtime_ns() -> i64 {
    if let Some(t) = commpage_realtime() {
        return t;
    }
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    // SAFETY: fills the local timeval.
    unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };
    commpage_realtime().unwrap_or(tv.tv_sec * 1_000_000_000 + tv.tv_usec as i64 * 1000)
}

/// The commpage's wall clock (libsystem's `__commpage_gettimeofday`), or
/// None when it cannot serve it. The vDSO computes the same.
fn commpage_realtime() -> Option<i64> {
    let at = |i: u64| {
        // SAFETY: the commpage is mapped in every Darwin process.
        unsafe { ((commpage::BASE + commpage::TIMEOFDAY + i * 8) as *const u64).read_volatile() }
    };
    let (tick, now, sec, frac, scale, per_sec) = loop {
        let tick = at(0);
        // SAFETY: reads the clock.
        let now = unsafe { mach_absolute_time() };
        let v = (tick, now, at(1), at(2), at(3), at(4));
        if at(0) == tick {
            break v;
        }
    };
    let delta = now.wrapping_sub(tick);
    if tick == 0 || delta >= per_sec || sec >> 63 != 0 {
        return None;
    }
    let step = scale as u128 * delta as u128;
    let (frac, carry) = frac.overflowing_add(step as u64);
    let sec = sec + (step >> 64) as u64 + carry as u64;
    let nsec = (frac as u128 * 1_000_000_000) >> 64;
    Some((sec * 1_000_000_000 + nsec as u64) as i64)
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

/// A CPU clock bound to its creator's target, not the host timer thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CpuClock(i32, bool, i64);

impl CpuClock {
    pub fn now(self) -> Result<u64, i64> {
        if self.1 {
            // A host worker has no guest thread; gettid's fallback there
            // is the process leader, not the thread being sampled.
            let (user, system) = super::thread::cpu_times(self.0).ok_or(-(EINVAL as i64))?;
            Ok(if self.2 == CPUCLOCK_VIRT {
                user
            } else {
                user + system
            })
        } else {
            cpu_ns((self.0, false, self.2), false)
        }
    }

    pub fn is_calling_thread(self) -> bool {
        self.1 && self.0 == super::thread::gettid() as i32
    }
}

/// Decode and validate as timer_create does: unlike clock_gettime, a
/// process clock must name a thread-group leader. Resolve pid/tid 0 now.
pub fn cpu_timer(id: u64) -> Result<Option<CpuClock>, i64> {
    let Some((id, thread, which)) = cpu_clock(id) else {
        return Ok(None);
    };
    cpu_ns((id, thread, which), false)?;
    let id = if id != 0 {
        id
    } else if thread {
        super::thread::gettid() as i32
    } else {
        std::process::id() as i32
    };
    Ok(Some(CpuClock(id, thread, which)))
}

/// POSIX timers can also run on a CPU clock; timerfds stay on Base.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimerClock {
    Wall(Base),
    Cpu(CpuClock),
}

impl TimerClock {
    pub fn new(id: u64) -> Result<Self, i64> {
        match cpu_timer(id)? {
            Some(c) => Ok(Self::Cpu(c)),
            None => timer_base(id).map(Self::Wall),
        }
    }

    pub fn now(self) -> Result<u64, i64> {
        match self {
            Self::Wall(b) => Ok(b.now()),
            Self::Cpu(c) => c.now(),
        }
    }

    pub fn timer(self, absolute: bool) -> Self {
        match self {
            Self::Wall(b) => Self::Wall(b.timer(absolute)),
            c => c,
        }
    }

    pub fn deadline(self, ns: u64) -> Deadline {
        match self {
            Self::Wall(base) => Deadline { base, ns },
            // CPU expiries are sampled every Linux tick (CONFIG_HZ=250,
            // 4 ms) using host task/thread usage, never elapsed wall time.
            // The same cadence is used by CPU-clock nanosleep.
            Self::Cpu(c) => Deadline::after(
                Base::Monotonic,
                if c.now().is_ok_and(|now| now >= ns) {
                    0
                } else {
                    TICK_NS
                },
            ),
        }
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
    // SAFETY: PROC_PIDTASKINFO into a local of the right size. Its total
    // times include live and exited threads, in Mach absolute ticks.
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = size_of::<libc::proc_taskinfo>() as i32;
        let r = libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut info).cast(), size);
        (r == size).then(|| {
            (
                ticks_to_ns(info.pti_total_user) as u64,
                ticks_to_ns(info.pti_total_system) as u64,
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
pub(super) fn own_process_times() -> Option<(u64, u64)> {
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
            user_try!(write_timespec(a[1], ns));
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
        user_try!(write_timespec(a[1], res));
    }
    0
}

pub fn gettimeofday(a: [u64; 6]) -> i64 {
    if a[1] != 0 {
        // SAFETY: guest struct timezone { int; int }: UTC, no DST, as
        // Android's kernel timezone stays.
        user_write!(a[1], [i32; 2], [0, 0]);
    }
    if a[0] != 0 {
        let ns = realtime_ns();
        // SAFETY: guest struct timeval is { i64; i64 }.
        {
            user_write!(
                a[0],
                [i64; 2],
                [ns / 1_000_000_000, ns % 1_000_000_000 / 1000]
            )
        };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relative REALTIME timer runs on MONOTONIC; every other setting
    /// keeps its clock, and only MONOTONIC stops while the Mac sleeps.
    #[test]
    fn timers_run_on_their_own_clock() {
        assert_eq!(Base::Realtime.timer(false), Base::Monotonic);
        assert_eq!(Base::Realtime.timer(true), Base::Realtime);
        for absolute in [false, true] {
            assert_eq!(Base::Monotonic.timer(absolute), Base::Monotonic);
            assert_eq!(Base::Boottime.timer(absolute), Base::Boottime);
        }
        assert!(!Base::Monotonic.counts_sleep());
        assert!(Base::Boottime.counts_sleep() && Base::Realtime.counts_sleep());
    }

    /// A deadline is counted down on its own clock, not converted to
    /// another one.
    #[test]
    fn a_deadline_stays_on_its_clock() {
        for base in [Base::Realtime, Base::Monotonic, Base::Boottime] {
            let d = Deadline::after(base, 3_600_000_000_000);
            assert!(!d.passed());
            assert!(d.left() <= 3_600_000_000_000 && d.left() > 3_599_000_000_000);
            let now = Deadline {
                base,
                ns: base.now(),
            };
            assert!(now.passed());
        }
    }
}
