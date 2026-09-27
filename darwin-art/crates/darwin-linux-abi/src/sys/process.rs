//! Process identity and lifetime syscalls.

use std::sync::OnceLock;

use std::sync::atomic::Ordering::SeqCst;

use super::thread::{self, Thread};
use crate::errno::{self, EINVAL, ESRCH};

struct Exe {
    guest: String,
    host: String,
}

static EXE: OnceLock<Exe> = OnceLock::new();

/// Record the program being run, for `/proc/self/exe`.
pub fn set_exe(guest: String, host: String) {
    let _ = EXE.set(Exe { guest, host });
}

pub fn exe_guest_path() -> String {
    EXE.get().map(|e| e.guest.clone()).unwrap_or_default()
}

pub fn exe_host_path() -> String {
    EXE.get().map(|e| e.host.clone()).unwrap_or_default()
}

pub fn getpid() -> i64 {
    // SAFETY: trivial.
    unsafe { libc::getpid() as i64 }
}

pub fn getppid() -> i64 {
    // SAFETY: trivial.
    unsafe { libc::getppid() as i64 }
}

pub use super::thread::gettid;

const SCHED_OTHER: i32 = 0;
const SCHED_FIFO: i32 = 1;
const SCHED_RR: i32 = 2;
const SCHED_BATCH: i32 = 3;
const SCHED_IDLE: i32 = 5;
const SCHED_RESET_ON_FORK: i32 = 0x4000_0000;

/// The thread a scheduling call names: 0 is the caller, a pid means the
/// main thread (Linux applies these per thread). None: a thread of another
/// live process, whose attributes are accepted and not kept (Darwin
/// schedules the host threads anyway).
fn sched_target(pid: i64) -> Result<Option<std::sync::Arc<Thread>>, i64> {
    // The kernel takes a pid_t.
    let pid = pid as i32;
    if pid < 0 {
        return Err(-(EINVAL as i64));
    }
    let tid = if pid == 0 {
        thread::gettid() as i32
    } else {
        pid
    };
    if let Some(t) = thread::find(tid) {
        return Ok(Some(t));
    }
    let owner = thread::owner(tid);
    // SAFETY: probing for the process without signalling it.
    let alive = owner as i64 != getpid()
        && (unsafe { libc::kill(owner, 0) } == 0 || errno::last() == libc::EPERM);
    if alive {
        Ok(None)
    } else {
        Err(-(ESRCH as i64))
    }
}

fn priority_range(policy: i32) -> Option<(i32, i32)> {
    match policy {
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE => Some((0, 0)),
        SCHED_FIFO | SCHED_RR => Some((1, 99)),
        _ => None,
    }
}

/// sched_setparam (118), sched_setscheduler (119), sched_getscheduler
/// (120), sched_getparam (121). The policy and priority are recorded per
/// thread; Darwin schedules the host threads as it sees fit.
pub fn sched_policy(nr: u64, a: [u64; 6]) -> i64 {
    let th = match sched_target(a[0] as i64) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let s = th.as_ref().map(|t| &t.sched);
    let get = |v: fn(&thread::Sched) -> i32| s.map_or(0, v);
    match nr {
        120 => get(|s| s.policy.load(SeqCst)) as i64,
        121 => {
            // SAFETY: guest struct sched_param { int sched_priority; }.
            unsafe { (a[1] as *mut i32).write_unaligned(get(|s| s.priority.load(SeqCst))) };
            0
        }
        _ => {
            let policy = if nr == 119 {
                a[1] as i32 & !SCHED_RESET_ON_FORK
            } else {
                get(|s| s.policy.load(SeqCst))
            };
            let param = if nr == 119 { a[2] } else { a[1] };
            if param == 0 {
                return -(EINVAL as i64);
            }
            // SAFETY: guest struct sched_param.
            let prio = unsafe { (param as *const i32).read_unaligned() };
            match priority_range(policy) {
                Some((lo, hi)) if (lo..=hi).contains(&prio) => {
                    if let Some(s) = s {
                        s.policy.store(policy, SeqCst);
                        s.priority.store(prio, SeqCst);
                    }
                    0
                }
                _ => -(EINVAL as i64),
            }
        }
    }
}

/// sched_get_priority_max (125) and sched_get_priority_min (126).
pub fn sched_priority_range(nr: u64, a: [u64; 6]) -> i64 {
    match priority_range(a[0] as i32) {
        Some((lo, hi)) => (if nr == 125 { hi } else { lo }) as i64,
        None => -(EINVAL as i64),
    }
}

/// sched_setaffinity: accepted; Darwin places the host threads.
pub fn sched_setaffinity(a: [u64; 6]) -> i64 {
    match sched_target(a[0] as i64) {
        Ok(_) => 0,
        Err(e) => e,
    }
}

/// sched_getaffinity: every online host CPU. Returns the mask size in bytes
/// as the kernel does (a multiple of 8 covering the CPUs).
pub fn sched_getaffinity(a: [u64; 6]) -> i64 {
    let (pid, len, mask) = (a[0] as i64, a[1] as usize, a[2]);
    if let Err(e) = sched_target(pid) {
        return e;
    }
    let ncpu = std::thread::available_parallelism().map_or(1, |n| n.get());
    let bytes = ncpu.div_ceil(64) * 8;
    if len < bytes || len % 8 != 0 {
        return -(EINVAL as i64);
    }
    let mut words = vec![0u64; bytes / 8];
    for cpu in 0..ncpu {
        words[cpu / 64] |= 1 << (cpu % 64);
    }
    // SAFETY: guest cpu_set_t of at least `bytes` bytes.
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, mask as *mut u8, bytes) };
    bytes as i64
}

const LINUX_RLIM_INFINITY: u64 = u64::MAX;

fn rlimit_to_host(res: u64) -> Option<i32> {
    Some(match res {
        0 => libc::RLIMIT_CPU,
        1 => libc::RLIMIT_FSIZE,
        2 => libc::RLIMIT_DATA,
        3 => libc::RLIMIT_STACK,
        4 => libc::RLIMIT_CORE,
        5 => libc::RLIMIT_RSS,
        6 => libc::RLIMIT_NPROC,
        7 => libc::RLIMIT_NOFILE,
        8 => libc::RLIMIT_MEMLOCK,
        9 => libc::RLIMIT_AS,
        _ => return None,
    })
}

fn lim_from_host(v: u64) -> u64 {
    if v == libc::RLIM_INFINITY {
        LINUX_RLIM_INFINITY
    } else {
        v
    }
}

fn lim_to_host(v: u64) -> u64 {
    if v == LINUX_RLIM_INFINITY {
        libc::RLIM_INFINITY
    } else {
        v
    }
}

/// getrlimit (163) and prlimit64 (261) for the calling process.
pub fn prlimit(nr: u64, a: [u64; 6]) -> i64 {
    let (pid, res, new, old) = if nr == 163 {
        (0, a[0], 0, a[1])
    } else {
        (a[0], a[1], a[2], a[3])
    };
    if pid != 0 && pid as i64 != getpid() {
        return -(ESRCH as i64);
    }
    let Some(hres) = rlimit_to_host(res) else {
        if old != 0 {
            // SAFETY: guest rlimit64 buffer.
            unsafe { (old as *mut [u64; 2]).write_unaligned([LINUX_RLIM_INFINITY; 2]) };
        }
        return if res < 16 { 0 } else { -(EINVAL as i64) };
    };
    let mut cur = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: local buffers and guest rlimit64 pointers.
    unsafe {
        if libc::getrlimit(hres, &mut cur) < 0 {
            return -(errno::last() as i64);
        }
        if old != 0 {
            (old as *mut [u64; 2])
                .write_unaligned([lim_from_host(cur.rlim_cur), lim_from_host(cur.rlim_max)]);
        }
        if new != 0 {
            let [c, m] = (new as *const [u64; 2]).read_unaligned();
            let l = libc::rlimit {
                rlim_cur: lim_to_host(c),
                rlim_max: lim_to_host(m),
            };
            if libc::setrlimit(hres, &l) < 0 {
                return -(errno::last() as i64);
            }
        }
    }
    0
}
