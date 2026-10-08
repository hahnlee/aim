//! Host side: the guest's clocks, which the syscall layer serves and host
//! modules stamp events with.
//!
//! Linux `CLOCK_MONOTONIC` stops while the machine sleeps and
//! `CLOCK_BOOTTIME` does not; both count from boot at nanosecond
//! resolution. Darwin's `mach_absolute_time` and `mach_continuous_time` are
//! exactly those two clocks, in the same ticks, so a host timestamp in
//! ticks (Core Video, Core Audio, `NSEvent`) is a guest time without an
//! offset. Darwin's own `CLOCK_MONOTONIC` is neither: it counts sleep, has
//! microsecond resolution and a different origin.

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_continuous_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

/// `numer << 32 | denom`; 0 until read.
static TIMEBASE: AtomicU64 = AtomicU64::new(0);

/// The host tick's length in nanoseconds, as (numerator, denominator).
pub fn timebase() -> (u32, u32) {
    let mut tb = TIMEBASE.load(Relaxed);
    if tb == 0 {
        let mut info = MachTimebaseInfo { numer: 1, denom: 1 };
        // SAFETY: fills the local struct.
        unsafe { mach_timebase_info(&mut info) };
        tb = (info.numer as u64) << 32 | info.denom.max(1) as u64;
        TIMEBASE.store(tb, Relaxed);
    }
    ((tb >> 32) as u32, tb as u32)
}

/// Host ticks to nanoseconds.
pub fn ticks_to_ns(ticks: u64) -> i64 {
    let (numer, denom) = timebase();
    let (numer, denom) = (numer as u64, denom as u64);
    // Split so that nothing overflows 64 bits for centuries of uptime.
    ((ticks / denom) * numer + (ticks % denom) * numer / denom) as i64
}

/// The guest's `CLOCK_MONOTONIC`, in nanoseconds.
pub fn monotonic_ns() -> i64 {
    // SAFETY: reads the clock.
    ticks_to_ns(unsafe { mach_absolute_time() })
}

/// The guest's `CLOCK_BOOTTIME`, in nanoseconds.
pub fn boottime_ns() -> i64 {
    // SAFETY: reads the clock.
    ticks_to_ns(unsafe { mach_continuous_time() })
}

#[cfg(test)]
mod tests {
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock: u32) -> u64;
    }

    const CLOCK_MONOTONIC_RAW: u32 = 4;
    const CLOCK_UPTIME_RAW: u32 = 8;

    /// Each clock lies between two reads of the Darwin clock it converts.
    #[test]
    fn clocks_are_uptime_and_continuous_time() {
        for (clock, darwin) in [
            (super::monotonic_ns as fn() -> i64, CLOCK_UPTIME_RAW),
            (super::boottime_ns, CLOCK_MONOTONIC_RAW),
        ] {
            // SAFETY: plain clock reads.
            let before = unsafe { clock_gettime_nsec_np(darwin) } as i64;
            let t = clock();
            // SAFETY: as above.
            let after = unsafe { clock_gettime_nsec_np(darwin) } as i64;
            assert!(before <= t && t <= after, "{before} {t} {after}");
        }
        // Read in this order: boottime only ever runs ahead of monotonic.
        let monotonic = super::monotonic_ns();
        assert!(super::boottime_ns() >= monotonic);
    }
}
