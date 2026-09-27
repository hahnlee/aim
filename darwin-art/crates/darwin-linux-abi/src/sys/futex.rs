//! futex over Darwin's `__ulock_wait2` / `__ulock_wake`.
//!
//! Supported: FUTEX_WAIT, FUTEX_WAKE, FUTEX_WAIT_BITSET and FUTEX_WAKE_BITSET
//! (bitsets other than all-ones wake conservatively). `__ulock_wake` wakes one
//! or all waiters, so a wake of 2..INT_MAX-1 wakes all, which futex users must
//! already tolerate as spurious wakeups. Priority-inheritance and requeue
//! operations are not implemented yet.

use crate::errno::{self, EAGAIN, EINTR, EINVAL, ENOSYS};

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_WAKE_BITSET: u64 = 10;
const FUTEX_PRIVATE_FLAG: u64 = 128;
const FUTEX_CLOCK_REALTIME: u64 = 256;

const UL_COMPARE_AND_WAIT: u32 = 1;
const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
const ULF_WAKE_ALL: u32 = 0x100;
const ULF_NO_ERRNO: u32 = 0x0100_0000;

unsafe extern "C" {
    fn __ulock_wait2(
        operation: u32,
        addr: *mut libc::c_void,
        value: u64,
        timeout_ns: u64,
        value2: u64,
    ) -> i32;
    fn __ulock_wake(operation: u32, addr: *mut libc::c_void, wake_value: u64) -> i32;
}

fn now_ns(realtime: bool) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let clk = if realtime {
        libc::CLOCK_REALTIME
    } else {
        libc::CLOCK_MONOTONIC
    };
    // SAFETY: local timespec.
    unsafe { libc::clock_gettime(clk, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Relative timeout in ns (0 = forever), or Err(-ETIMEDOUT) if already due.
fn timeout(op: u64, ts: u64, realtime: bool) -> Result<u64, i64> {
    if ts == 0 {
        return Ok(0);
    }
    // SAFETY: guest timespec.
    let [sec, nsec] = unsafe { (ts as *const [i64; 2]).read_unaligned() };
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(-(EINVAL as i64));
    }
    let t = sec as u64 * 1_000_000_000 + nsec as u64;
    if op == FUTEX_WAIT {
        return Ok(t.max(1));
    }
    let now = now_ns(realtime);
    if t <= now { Err(-110) } else { Ok(t - now) }
}

pub fn futex(a: [u64; 6]) -> i64 {
    let (uaddr, op_full, val, ts) = (a[0], a[1], a[2] as u32, a[3]);
    let op = op_full & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    let shared = op_full & FUTEX_PRIVATE_FLAG == 0;
    let realtime = op_full & FUTEX_CLOCK_REALTIME != 0;
    if uaddr & 3 != 0 {
        return -(EINVAL as i64);
    }
    let ul = if shared {
        UL_COMPARE_AND_WAIT_SHARED
    } else {
        UL_COMPARE_AND_WAIT
    };
    match op {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            if op == FUTEX_WAIT_BITSET && a[5] as u32 == 0 {
                return -(EINVAL as i64);
            }
            let t = match timeout(op, ts, realtime) {
                Ok(t) => t,
                Err(e) => return e,
            };
            // SAFETY: guest futex word.
            if unsafe { (uaddr as *const u32).read_volatile() } != val {
                return -(EAGAIN as i64);
            }
            // SAFETY: waiting on a guest futex word.
            let r = unsafe { __ulock_wait2(ul | ULF_NO_ERRNO, uaddr as *mut _, val as u64, t, 0) };
            if r >= 0 {
                0
            } else {
                match -r {
                    libc::ETIMEDOUT => -110,
                    libc::EINTR => -(EINTR as i64),
                    // The value changed before we slept.
                    libc::EFAULT => -(errno::EFAULT as i64),
                    _ => 0,
                }
            }
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if val == 0 {
                return 0;
            }
            let flags = if val == 1 { 0 } else { ULF_WAKE_ALL };
            // SAFETY: waking waiters of a guest futex word.
            let r = unsafe { __ulock_wake(ul | flags | ULF_NO_ERRNO, uaddr as *mut _, 0) };
            // Darwin does not report how many waiters woke.
            if r == 0 { 1 } else { 0 }
        }
        _ => {
            eprintln!("[linux-abi] futex op {op_full:#x} not implemented");
            -(ENOSYS as i64)
        }
    }
}
