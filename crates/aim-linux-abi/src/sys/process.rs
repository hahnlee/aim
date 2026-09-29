//! Process identity and lifetime syscalls.

use std::sync::OnceLock;

use std::sync::atomic::Ordering::SeqCst;

use super::thread::{self, Thread};
use crate::errno::{self, EINVAL, EPERM, ESRCH};

struct Exe {
    guest: String,
    host: String,
}

static EXE: OnceLock<Exe> = OnceLock::new();

/// Record the program being run, for `/proc/self/exe`. exec names the
/// process after it (`comm`, up to 15 bytes).
pub fn set_exe(guest: String, host: String) {
    let base = guest.rsplit('/').next().unwrap_or("").as_bytes();
    let mut name = [0u8; 16];
    let len = base.len().min(15);
    name[..len].copy_from_slice(&base[..len]);
    thread::set_name_of(thread::main_tid(), name);
    let _ = EXE.set(Exe { guest, host });
}

/// Fork: the program, for `/proc/self/exe`.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    w.str(&exe_guest_path());
    w.str(&exe_host_path());
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let (guest, host) = (r.str(), r.str());
    set_exe(guest, host);
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

/// getppid: 0 for a parent outside the pid namespace (guest-init, the
/// shell that started a standalone linux-run), as Linux reports a parent
/// in an ancestor namespace.
pub fn getppid() -> i64 {
    // SAFETY: trivial.
    super::pidns::vnr(unsafe { libc::getppid() }) as i64
}

pub use super::thread::gettid;

const SCHED_OTHER: i32 = 0;
const SCHED_FIFO: i32 = 1;
const SCHED_RR: i32 = 2;
const SCHED_BATCH: i32 = 3;
const SCHED_IDLE: i32 = 5;
const SCHED_RESET_ON_FORK: i32 = 0x4000_0000;

/// The thread a scheduling call names: 0 is the caller, a pid means the
/// main thread (Linux applies these per thread).
enum Target {
    Own(std::sync::Arc<Thread>),
    /// A thread of another process of the namespace, with its record.
    Peer(i32, super::procrec::Peer),
}

fn sched_target(pid: i64) -> Result<Target, i64> {
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
        return Ok(Target::Own(t));
    }
    let owner = thread::owner(tid);
    if owner as i64 == getpid() {
        return Err(-(ESRCH as i64));
    }
    super::pidns::check(owner)?;
    super::procrec::Peer::open(owner)
        .filter(|p| p.has(tid))
        .map(|p| Target::Peer(tid, p))
        .ok_or(-(ESRCH as i64))
}

fn priority_range(policy: i32) -> Option<(i32, i32)> {
    match policy {
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE => Some((0, 0)),
        SCHED_FIFO | SCHED_RR => Some((1, 99)),
        _ => None,
    }
}

/// The Darwin QoS class for a thread's Linux scheduling. Android runs
/// background work at a nice value of ANDROID_PRIORITY_BACKGROUND (10) or
/// more, or in SCHED_BATCH or SCHED_IDLE; Darwin runs that at utility QoS,
/// the lowest nice value and SCHED_IDLE at background QoS (`PRIO_DARWIN_BG`:
/// efficiency cores, throttled I/O), and everything else at the default.
pub fn host_qos(nice: i32, policy: i32) -> libc::qos_class_t {
    use libc::qos_class_t::*;
    match policy {
        SCHED_IDLE => QOS_CLASS_BACKGROUND,
        SCHED_BATCH => QOS_CLASS_UTILITY,
        _ if nice >= 19 => QOS_CLASS_BACKGROUND,
        _ if nice >= 10 => QOS_CLASS_UTILITY,
        _ => QOS_CLASS_DEFAULT,
    }
}

/// Give the calling thread `th` the QoS of its Linux scheduling.
pub fn apply_own_qos(th: &Thread) {
    let qos = host_qos(
        super::cred::thread_nice(th.tid),
        th.sched.policy.load(SeqCst),
    );
    // SAFETY: plain call on the calling thread.
    unsafe { libc::pthread_set_qos_class_self_np(qos, 0) };
}

/// Thread `tid` of this process has new scheduling. Darwin sets a thread's
/// QoS only from the thread itself: the calling thread applies its own at
/// once, another thread is poked to apply it ([`apply_own_qos`]) when it
/// next checks for signals.
pub fn sched_changed(tid: i32) {
    let Some(th) = thread::find(tid) else { return };
    if thread::is_current(&th) {
        apply_own_qos(&th);
    } else {
        th.qos_stale.store(true, SeqCst);
        super::signal::poke(&th);
    }
}

/// Another process changed the scheduling of thread `tid` in this
/// process's record (`procrec`): take it over.
pub fn adopt(tid: i32, s: super::procrec::Sched) {
    let Some(th) = thread::find(tid) else { return };
    th.sched.policy.store(s.policy, SeqCst);
    th.sched.priority.store(s.priority, SeqCst);
    super::cred::adopt_nice(tid, s.nice);
    sched_changed(tid);
}

/// A change to another process's scheduling (a thread of it, `pid`) needs
/// the same owner or CAP_SYS_NICE (`check_same_owner`).
fn may_change(pid: i64) -> Result<(), i64> {
    if super::cred::may_renice(thread::owner(pid as i32)) {
        Ok(())
    } else {
        Err(-(EPERM as i64))
    }
}

/// sched_setparam (118), sched_setscheduler (119), sched_getscheduler
/// (120), sched_getparam (121). The policy and priority are kept per
/// thread and set its host QoS ([`host_qos`]); another process's thread
/// takes them through its record.
pub fn sched_policy(nr: u64, a: [u64; 6]) -> i64 {
    let target = match sched_target(a[0] as i64) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let now = match &target {
        Target::Own(th) => th.sched_now(),
        Target::Peer(tid, p) => p.sched(*tid).unwrap_or_default(),
    };
    match nr {
        120 => now.policy as i64,
        121 => {
            // SAFETY: guest struct sched_param { int sched_priority; }.
            unsafe { (a[1] as *mut i32).write_unaligned(now.priority) };
            0
        }
        _ => {
            let policy = if nr == 119 {
                a[1] as i32 & !SCHED_RESET_ON_FORK
            } else {
                now.policy
            };
            let param = if nr == 119 { a[2] } else { a[1] };
            if param == 0 {
                return -(EINVAL as i64);
            }
            // SAFETY: guest struct sched_param.
            let priority = unsafe { (param as *const i32).read_unaligned() };
            if !priority_range(policy).is_some_and(|(lo, hi)| (lo..=hi).contains(&priority)) {
                return -(EINVAL as i64);
            }
            let set = move |s| super::procrec::Sched {
                policy,
                priority,
                ..s
            };
            match target {
                Target::Own(th) => {
                    th.sched.policy.store(policy, SeqCst);
                    th.sched.priority.store(priority, SeqCst);
                    super::procrec::set_sched(th.tid, set);
                    sched_changed(th.tid);
                }
                Target::Peer(tid, p) => {
                    if let Err(e) = may_change(a[0] as i64) {
                        return e;
                    }
                    p.set_sched(tid, set);
                }
            }
            0
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
        Ok(Target::Own(_)) => 0,
        Ok(Target::Peer(..)) => may_change(a[0] as i64).map_or_else(|e| e, |_| 0),
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

/// The host's getrlimit (163) and prlimit64 (261) for the calling process
/// (`cred::prlimit` resolves the pid).
pub fn prlimit(nr: u64, a: [u64; 6]) -> i64 {
    let (res, new, old) = if nr == 163 {
        (a[0], 0, a[1])
    } else {
        (a[1], a[2], a[3])
    };
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
