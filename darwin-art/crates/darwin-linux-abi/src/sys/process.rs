//! Process identity and lifetime syscalls.

use std::sync::OnceLock;

use crate::errno::{self, EINVAL, EPERM, ESRCH};

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

/// Linux tid of the calling host thread. The main thread's tid equals the
/// pid; `clone` threads are future work.
pub fn host_tid() -> i64 {
    // SAFETY: trivial.
    if unsafe { libc::pthread_main_np() } == 1 {
        return getpid();
    }
    let mut id = 0u64;
    // SAFETY: querying the current thread.
    unsafe { libc::pthread_threadid_np(0 as libc::pthread_t, &mut id) };
    (id & 0x3fff_ffff) as i64
}

/// gettid: the tid recorded for this guest thread (the lean path answers it
/// without reaching here unless tracing).
pub fn gettid() -> i64 {
    host_tid()
}

pub fn set_tid_address(a: [u64; 6]) -> i64 {
    super::thread::set_clear_tid(a[0]);
    gettid()
}

pub fn exit_group(a: [u64; 6]) -> i64 {
    super::cred::forget(getpid() as i32);
    // SAFETY: terminating the process without running host atexit handlers,
    // as a Linux exit_group would.
    unsafe { libc::_exit(a[0] as i32) }
}

/// Linux -> Darwin signal numbers (0 when Darwin has no equivalent).
pub fn signal_to_host(sig: i32) -> i32 {
    match sig {
        1 => libc::SIGHUP,
        2 => libc::SIGINT,
        3 => libc::SIGQUIT,
        4 => libc::SIGILL,
        5 => libc::SIGTRAP,
        6 => libc::SIGABRT,
        7 => libc::SIGBUS,
        8 => libc::SIGFPE,
        9 => libc::SIGKILL,
        10 => libc::SIGUSR1,
        11 => libc::SIGSEGV,
        12 => libc::SIGUSR2,
        13 => libc::SIGPIPE,
        14 => libc::SIGALRM,
        15 => libc::SIGTERM,
        17 => libc::SIGCHLD,
        18 => libc::SIGCONT,
        19 => libc::SIGSTOP,
        20 => libc::SIGTSTP,
        21 => libc::SIGTTIN,
        22 => libc::SIGTTOU,
        23 => libc::SIGURG,
        24 => libc::SIGXCPU,
        25 => libc::SIGXFSZ,
        26 => libc::SIGVTALRM,
        27 => libc::SIGPROF,
        28 => libc::SIGWINCH,
        29 => libc::SIGIO,
        31 => libc::SIGSYS,
        _ => 0,
    }
}

/// kill/tkill/tgkill aimed at this process. Guest signal handlers are not
/// delivered yet, so a fatal signal takes its default action on the host.
pub fn kill(nr: u64, a: [u64; 6]) -> i64 {
    let (target, sig) = match nr {
        131 => (a[1] as i64, a[2] as i32),
        _ => (a[0] as i64, a[1] as i32),
    };
    let me = getpid();
    let self_target = target == me || target == gettid() || (nr == 129 && target == 0);
    if !self_target {
        return -(if target <= 0 { EPERM } else { ESRCH } as i64);
    }
    if sig == 0 {
        return 0;
    }
    let host = signal_to_host(sig);
    if host == 0 {
        return -(EINVAL as i64);
    }
    eprintln!("[linux-abi] guest sent itself signal {sig}; taking the default action");
    // SAFETY: reset to default and raise, as Linux would with no handler.
    unsafe {
        libc::signal(host, libc::SIG_DFL);
        errno::check(libc::raise(host) as i64)
    }
}

const SCHED_OTHER: u64 = 0;

/// sched_setparam (118), sched_setscheduler (119), sched_getscheduler (120),
/// sched_getparam (121) for this process. Guest threads run under Darwin's
/// scheduler, which Linux reports as SCHED_OTHER with priority 0.
pub fn sched_policy(nr: u64, a: [u64; 6]) -> i64 {
    let pid = a[0] as i64;
    if pid < 0 {
        return -(EINVAL as i64);
    }
    if pid != 0 && pid != getpid() && pid != gettid() {
        return -(ESRCH as i64);
    }
    match nr {
        120 => SCHED_OTHER as i64,
        121 => {
            // SAFETY: guest struct sched_param { int sched_priority; }.
            unsafe { (a[1] as *mut i32).write_unaligned(0) };
            0
        }
        119 if a[1] & !0x4000_0000 != SCHED_OTHER => -(EPERM as i64),
        _ => 0,
    }
}

/// sched_getaffinity: every online host CPU. Returns the mask size in bytes
/// as the kernel does (a multiple of 8 covering the CPUs).
pub fn sched_getaffinity(a: [u64; 6]) -> i64 {
    let (pid, len, mask) = (a[0] as i64, a[1] as usize, a[2]);
    if pid != 0 && pid != getpid() && pid != gettid() {
        return -(ESRCH as i64);
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
