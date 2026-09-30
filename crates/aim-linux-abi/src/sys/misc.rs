//! sched_yield, randomness and prctl.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use crate::errno::{EFAULT, EINVAL};

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

const PR_SET_PDEATHSIG: u64 = 1;
const PR_GET_PDEATHSIG: u64 = 2;
const PR_GET_DUMPABLE: u64 = 3;
const PR_SET_DUMPABLE: u64 = 4;
const PR_GET_KEEPCAPS: u64 = 7;
const PR_SET_KEEPCAPS: u64 = 8;
const PR_SET_NAME: u64 = 15;
const PR_GET_NAME: u64 = 16;
const PR_GET_SECCOMP: u64 = 21;
const PR_SET_SECCOMP: u64 = 22;
const SECCOMP_MODE_FILTER: u64 = 2;
const PR_GET_TIMERSLACK: u64 = 30;
const PR_SET_TIMERSLACK: u64 = 29;
const PR_SET_NO_NEW_PRIVS: u64 = 38;
const PR_GET_NO_NEW_PRIVS: u64 = 39;
const PR_SET_VMA: u64 = 0x53564d41;
const PR_SET_PTRACER: u64 = 0x59616d61;

/// Per-process prctl state that only reads back.
static PDEATHSIG: AtomicU64 = AtomicU64::new(0);
static KEEPCAPS: AtomicU64 = AtomicU64::new(0);
static DUMPABLE: AtomicU64 = AtomicU64::new(1);
/// Linux's default timer slack (50 us).
static TIMERSLACK: AtomicU64 = AtomicU64::new(50_000);
static SECCOMP: AtomicU64 = AtomicU64::new(0);

/// Whether the process is dumpable (PR_SET_DUMPABLE), which ptrace needs.
pub fn dumpable() -> bool {
    DUMPABLE.load(Relaxed) == 1
}

/// execve in place: the keep-capabilities flag clears and the process is
/// dumpable again (Linux's `setup_new_exec` for a program that gains no
/// privileges).
pub(super) fn exec_reset() {
    KEEPCAPS.store(0, Relaxed);
    DUMPABLE.store(1, Relaxed);
}

/// Fork: what a child inherits (not the parent-death signal, which
/// Linux clears).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    for v in [&KEEPCAPS, &DUMPABLE, &TIMERSLACK, &SECCOMP] {
        w.u64(v.load(Relaxed));
    }
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    for v in [&KEEPCAPS, &DUMPABLE, &TIMERSLACK, &SECCOMP] {
        v.store(r.u64(), Relaxed);
    }
}

pub fn prctl(a: [u64; 6]) -> i64 {
    if let Some(r) = super::cred::prctl(a) {
        return r;
    }
    match a[0] {
        // Anonymous VMA names are diagnostics only on Linux.
        PR_SET_VMA | PR_SET_NO_NEW_PRIVS | PR_SET_PTRACER => 0,
        PR_GET_NO_NEW_PRIVS => 0,
        PR_SET_DUMPABLE if a[1] <= 1 => {
            DUMPABLE.store(a[1], Relaxed);
            0
        }
        PR_GET_DUMPABLE => DUMPABLE.load(Relaxed) as i64,
        PR_SET_PDEATHSIG if a[1] <= 64 => {
            PDEATHSIG.store(a[1], Relaxed);
            0
        }
        PR_GET_PDEATHSIG => {
            // SAFETY: guest int.
            unsafe { (a[1] as *mut i32).write_unaligned(PDEATHSIG.load(Relaxed) as i32) };
            0
        }
        PR_SET_KEEPCAPS if a[1] <= 1 => {
            KEEPCAPS.store(a[1], Relaxed);
            0
        }
        PR_GET_KEEPCAPS => KEEPCAPS.load(Relaxed) as i64,
        PR_SET_TIMERSLACK => {
            TIMERSLACK.store(if a[1] == 0 { 50_000 } else { a[1] }, Relaxed);
            0
        }
        PR_GET_TIMERSLACK => TIMERSLACK.load(Relaxed) as i64,
        // minijail installs a filter in the media services and aborts when
        // it cannot. Darwin has no seccomp: the filter is accepted and not
        // enforced.
        PR_SET_SECCOMP if a[1] == SECCOMP_MODE_FILTER => {
            SECCOMP.store(SECCOMP_MODE_FILTER, Relaxed);
            0
        }
        PR_GET_SECCOMP => SECCOMP.load(Relaxed) as i64,
        PR_SET_NAME => {
            // SAFETY: guest string (at most 16 bytes used).
            let s = unsafe { crate::sys::guest_cstr(a[1]) };
            let len = s.len().min(15);
            let mut b = [0u8; 16];
            b[..len].copy_from_slice(&s[..len]);
            super::thread::set_name(b);
            if let Ok(c) = std::ffi::CString::new(&s[..len]) {
                // SAFETY: naming the current thread.
                unsafe { libc::pthread_setname_np(c.as_ptr()) };
            }
            0
        }
        PR_GET_NAME => {
            let n = super::thread::name();
            // SAFETY: guest 16-byte buffer.
            unsafe { std::ptr::copy_nonoverlapping(n.as_ptr(), a[1] as *mut u8, 16) };
            0
        }
        _ => -(EINVAL as i64),
    }
}
