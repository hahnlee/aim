//! Signal state syscalls. Dispositions, masks and alternate stacks are
//! recorded with Linux semantics; delivery with Linux signal frames is the
//! next step and not implemented yet (handlers are never invoked).

use std::cell::Cell;
use std::sync::Mutex;

use crate::errno::{EINVAL, EPERM};

const NSIG: usize = 64;
const SIGKILL: u64 = 9;
const SIGSTOP: u64 = 19;
const SS_DISABLE: i32 = 2;
const SS_ONSTACK: i32 = 1;
const SS_AUTODISARM: i32 = i32::MIN;

/// Linux arm64 `struct sigaction` as seen by the kernel.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KSigaction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

static ACTIONS: Mutex<[KSigaction; NSIG]> = Mutex::new(
    [KSigaction {
        handler: 0,
        flags: 0,
        restorer: 0,
        mask: 0,
    }; NSIG],
);

thread_local! {
    static MASK: Cell<u64> = const { Cell::new(0) };
    static ALTSTACK: Cell<(u64, i32, u64)> = const { Cell::new((0, SS_DISABLE, 0)) };
}

pub fn rt_sigaction(a: [u64; 6]) -> i64 {
    let (sig, act, oact, size) = (a[0], a[1], a[2], a[3]);
    if size != 8 || sig == 0 || sig > NSIG as u64 {
        return -(EINVAL as i64);
    }
    if act != 0 && (sig == SIGKILL || sig == SIGSTOP) {
        return -(EINVAL as i64);
    }
    let mut t = ACTIONS.lock().unwrap();
    let slot = &mut t[sig as usize - 1];
    // SAFETY: guest sigaction buffers.
    unsafe {
        if oact != 0 {
            (oact as *mut KSigaction).write_unaligned(*slot);
        }
        if act != 0 {
            let mut new = (act as *const KSigaction).read_unaligned();
            new.mask &= !((1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)));
            *slot = new;
        }
    }
    0
}

pub fn rt_sigprocmask(a: [u64; 6]) -> i64 {
    let (how, set, oset, size) = (a[0], a[1], a[2], a[3]);
    if size != 8 {
        return -(EINVAL as i64);
    }
    MASK.with(|m| {
        let old = m.get();
        if set != 0 {
            // SAFETY: guest sigset pointer.
            let s = unsafe { (set as *const u64).read_unaligned() }
                & !((1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)));
            let new = match how {
                0 => old | s,
                1 => old & !s,
                2 => s,
                _ => return -(EINVAL as i64),
            };
            m.set(new);
        }
        if oset != 0 {
            // SAFETY: guest sigset pointer.
            unsafe { (oset as *mut u64).write_unaligned(old) };
        }
        0
    })
}

/// Linux `stack_t`: { void *ss_sp; int ss_flags; size_t ss_size; }.
pub fn sigaltstack(a: [u64; 6]) -> i64 {
    let (ss, old) = (a[0], a[1]);
    ALTSTACK.with(|cur| {
        let (sp, flags, size) = cur.get();
        if old != 0 {
            // SAFETY: guest stack_t.
            unsafe {
                let p = old as *mut u8;
                (p as *mut u64).write_unaligned(sp);
                (p.add(8) as *mut i32).write_unaligned(flags);
                (p.add(16) as *mut u64).write_unaligned(size);
            }
        }
        if ss != 0 {
            if flags & SS_ONSTACK != 0 {
                return -(EPERM as i64);
            }
            // SAFETY: guest stack_t.
            let (nsp, nflags, nsize) = unsafe {
                let p = ss as *const u8;
                (
                    (p as *const u64).read_unaligned(),
                    (p.add(8) as *const i32).read_unaligned(),
                    (p.add(16) as *const u64).read_unaligned(),
                )
            };
            if nflags & !(SS_DISABLE | SS_AUTODISARM) != 0 {
                return -(EINVAL as i64);
            }
            cur.set(if nflags & SS_DISABLE != 0 {
                (0, SS_DISABLE, 0)
            } else {
                (nsp, 0, nsize)
            });
        }
        0
    })
}
