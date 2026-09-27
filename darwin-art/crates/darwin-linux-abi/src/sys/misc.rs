//! Time, randomness and prctl.

use crate::errno::{self, EFAULT, EINVAL};

fn clock_to_host(id: u64) -> Option<libc::clockid_t> {
    Some(match id {
        0 | 5 => libc::CLOCK_REALTIME,       // REALTIME, REALTIME_COARSE
        1 | 6 | 7 => libc::CLOCK_MONOTONIC,  // MONOTONIC, MONOTONIC_COARSE, BOOTTIME
        4 => libc::CLOCK_MONOTONIC_RAW,      // MONOTONIC_RAW
        2 => libc::CLOCK_PROCESS_CPUTIME_ID, // PROCESS_CPUTIME_ID
        3 => libc::CLOCK_THREAD_CPUTIME_ID,  // THREAD_CPUTIME_ID
        8 | 9 => libc::CLOCK_REALTIME,       // REALTIME_ALARM, BOOTTIME_ALARM
        _ => return None,
    })
}

// struct timespec is { i64 tv_sec; i64 tv_nsec; } on both kernels.
pub fn clock_gettime(a: [u64; 6]) -> i64 {
    let Some(id) = clock_to_host(a[0]) else {
        return -(EINVAL as i64);
    };
    // SAFETY: guest timespec.
    errno::check(unsafe { libc::clock_gettime(id, a[1] as *mut libc::timespec) } as i64)
}

pub fn clock_getres(a: [u64; 6]) -> i64 {
    let Some(id) = clock_to_host(a[0]) else {
        return -(EINVAL as i64);
    };
    if a[1] == 0 {
        return 0;
    }
    // SAFETY: guest timespec.
    errno::check(unsafe { libc::clock_getres(id, a[1] as *mut libc::timespec) } as i64)
}

pub fn gettimeofday(a: [u64; 6]) -> i64 {
    if a[0] == 0 {
        return 0;
    }
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    // SAFETY: local timeval; guest struct timeval is { i64; i64 }.
    unsafe {
        libc::gettimeofday(&mut tv, std::ptr::null_mut());
        (a[0] as *mut [i64; 2]).write_unaligned([tv.tv_sec, tv.tv_usec as i64]);
    }
    0
}

pub fn nanosleep(a: [u64; 6]) -> i64 {
    // SAFETY: guest timespecs.
    errno::check(unsafe {
        libc::nanosleep(a[0] as *const libc::timespec, a[1] as *mut libc::timespec)
    } as i64)
}

pub fn sched_yield() -> i64 {
    // SAFETY: trivial.
    unsafe { libc::sched_yield() as i64 }
}

pub fn getrandom(a: [u64; 6]) -> i64 {
    let (buf, len) = (a[0], a[1] as usize);
    let mut done = 0;
    while done < len {
        let n = (len - done).min(256);
        // SAFETY: guest buffer of `len` bytes.
        if unsafe { libc::getentropy((buf as *mut u8).add(done).cast(), n) } < 0 {
            return if done > 0 {
                done as i64
            } else {
                -(EFAULT as i64)
            };
        }
        done += n;
    }
    len as i64
}

const PR_GET_DUMPABLE: u64 = 3;
const PR_SET_DUMPABLE: u64 = 4;
const PR_SET_NAME: u64 = 15;
const PR_GET_NAME: u64 = 16;
const PR_SET_NO_NEW_PRIVS: u64 = 38;
const PR_GET_NO_NEW_PRIVS: u64 = 39;
const PR_SET_VMA: u64 = 0x53564d41;

thread_local! {
    static NAME: std::cell::RefCell<[u8; 16]> = const { std::cell::RefCell::new([0; 16]) };
}

pub fn prctl(a: [u64; 6]) -> i64 {
    if let Some(r) = super::cred::prctl(a) {
        return r;
    }
    match a[0] {
        // Anonymous VMA names are diagnostics only on Linux.
        PR_SET_VMA => 0,
        PR_SET_DUMPABLE | PR_SET_NO_NEW_PRIVS => 0,
        PR_GET_DUMPABLE => 1,
        PR_GET_NO_NEW_PRIVS => 0,
        PR_SET_NAME => {
            // SAFETY: guest string (at most 16 bytes used).
            let s = unsafe { crate::sys::guest_cstr(a[1]) };
            NAME.with(|n| {
                let mut b = [0u8; 16];
                let len = s.len().min(15);
                b[..len].copy_from_slice(&s[..len]);
                *n.borrow_mut() = b;
                if let Ok(c) = std::ffi::CString::new(&s[..len]) {
                    // SAFETY: naming the current thread.
                    unsafe { libc::pthread_setname_np(c.as_ptr()) };
                }
            });
            0
        }
        PR_GET_NAME => {
            // SAFETY: guest 16-byte buffer.
            NAME.with(|n| unsafe {
                std::ptr::copy_nonoverlapping(n.borrow().as_ptr(), a[1] as *mut u8, 16)
            });
            0
        }
        _ => -(EINVAL as i64),
    }
}
