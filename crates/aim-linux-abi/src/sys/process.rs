//! Process identity and lifetime syscalls.

use std::sync::RwLock;

use std::sync::atomic::Ordering::SeqCst;

use super::thread::{self, Thread};
use crate::errno::{self, EINVAL, EPERM, ESRCH};

struct Exe {
    guest: String,
    host: String,
}

static EXE: RwLock<Option<Exe>> = RwLock::new(None);

/// Record the program being run, for `/proc/self/exe`.
pub fn set_exe(guest: String, host: String) {
    *EXE.write().unwrap_or_else(|e| e.into_inner()) = Some(Exe { guest, host });
}

fn exe<R>(f: impl FnOnce(&Exe) -> R) -> Option<R> {
    EXE.read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(f)
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
    exe(|e| e.guest.clone()).unwrap_or_default()
}

pub fn exe_host_path() -> String {
    exe(|e| e.host.clone()).unwrap_or_default()
}

pub fn getpid() -> i64 {
    // SAFETY: trivial.
    unsafe { libc::getpid() as i64 }
}

/// getpid(2): the thread group's id, which for a thread running as a
/// process of its own (a clone without CLONE_FILES, #228) is the process
/// it is a thread of.
pub fn getpid_tgid() -> i64 {
    super::procrec::own_tgid().map_or_else(getpid, |g| g as i64)
}

/// getppid: 0 for a parent outside the pid namespace (guest-init, the
/// shell that started a standalone linux-run), as Linux reports a parent
/// in an ancestor namespace.
pub fn getppid() -> i64 {
    // SAFETY: trivial.
    linux_ppid(unsafe { libc::getppid() }) as i64
}

/// The parent process Linux reports for host parent `host`: its thread
/// group, when it is a thread running as a process of its own (`fork`).
pub fn linux_ppid(host: i32) -> i32 {
    if !super::pidns::contains(host) {
        return 0;
    }
    let group = super::procrec::tgid(host);
    super::pidns::guest_pid(group).unwrap_or(0)
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
    if owner as i64 != getpid() && super::pidns::contains(owner) {
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

/// The Darwin QoS class for a thread's Linux scheduling that does not
/// raise it. Android runs background work at a nice value of
/// ANDROID_PRIORITY_BACKGROUND (10) or more, or in SCHED_BATCH or
/// SCHED_IDLE; Darwin runs that at utility QoS, the lowest nice value and
/// SCHED_IDLE at background QoS (`PRIO_DARWIN_BG`: efficiency cores,
/// throttled I/O), and everything else at the default.
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

/// Darwin's default priority (`BASEPRI_DEFAULT`) and the top of its user
/// range (`MAXPRI_USER`).
const BASEPRI_DEFAULT: i32 = 31;
const MAXPRI_USER: i32 = 63;

/// The Darwin timeshare priority of a nice value, as Darwin's own nice
/// value sets it: the task importance `-nice` over the default priority
/// (31), within the user range (15-47).
fn timeshare_priority(nice: i32) -> i32 {
    (BASEPRI_DEFAULT - nice).clamp(15, 47)
}

/// This task's base priority (`task->priority`): the default moved by the
/// host process's nice value (Darwin's task importance), or a host clamp.
fn task_priority() -> i32 {
    // SAFETY: proc_pidinfo into a local struct of the size given.
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = size_of::<libc::proc_taskinfo>() as i32;
        if libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&raw mut info).cast(),
            size,
        ) == size
        {
            info.pti_priority
        } else {
            BASEPRI_DEFAULT
        }
    }
}

thread_local! {
    /// The calling thread has a Darwin scheduling policy, which opts it
    /// out of QoS for good (`pthread_set_qos_class_self_np` then fails).
    static HOST_POLICY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "C" {
    fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
}

/// Give the calling thread the host scheduling of its Linux scheduling.
/// Linux runs a SCHED_FIFO or SCHED_RR thread before every normal one and
/// weighs normal threads by nice value; Android raises display and audio
/// threads to negative nice values (-4 display to -19 urgent audio) and
/// FAST audio threads to SCHED_FIFO. Darwin caps a thread's QoS at its
/// task's, below what a raised thread needs, so a raised thread takes a
/// Darwin policy instead: the real-time policies Darwin's fixed priority 47
/// (the highest outside its real-time band, which needs a period), a
/// negative nice value its timeshare priority ([`timeshare_priority`]).
/// Other scheduling is a QoS class ([`host_qos`]) until a thread was raised.
/// Darwin sets a thread's QoS only from the thread itself, so a thread that
/// changes another's scheduling marks it ([`thread::Sched::host_stale`])
/// and that thread applies it at its next syscall ([`apply_stale_sched`]).
/// Darwin keeps a fixed or timeshare priority as an importance over its
/// default priority on top of the task's base priority, which the host
/// process's nice value moves (a job a shell starts in the background runs
/// at nice 5); a Linux thread's priority is its own, so the importance is
/// taken from the task's base priority as it is, and threads apply theirs
/// again when it changes ([`task_priority_changed`]).
pub fn apply_host_sched(nice: i32, policy: i32) -> Result<(), crate::errno::Errno> {
    let th = thread::current();
    let (floor, ceiling, io) = th.map_or((0, 33, 0), |th| {
        (
            th.sched.cgroup_min.load(SeqCst),
            if matches!(policy, SCHED_FIFO | SCHED_RR) {
                th.sched.cgroup_max.load(SeqCst)
            } else {
                th.sched
                    .cgroup_max
                    .load(SeqCst)
                    .min(th.sched.cgroup_fair_max.load(SeqCst))
            },
            th.sched.cgroup_io.load(SeqCst),
        )
    });
    // Disk policy is per thread, so a task move cannot throttle siblings.
    let io_result = unsafe { setiopolicy_np(0, 1, io) };
    if io_result < 0 {
        return Err(crate::errno::last());
    }
    let raised = matches!(policy, SCHED_FIFO | SCHED_RR)
        || nice < 0 && !matches!(policy, SCHED_BATCH | SCHED_IDLE)
        || floor >= 25;
    if !raised && !HOST_POLICY.get() {
        // SAFETY: plain call on the calling thread.
        let result = unsafe {
            libc::pthread_set_qos_class_self_np(
                super::cgroup::clamp_qos(host_qos(nice, policy) as i32, floor, ceiling),
                0,
            )
        };
        return if result == 0 {
            Ok(())
        } else {
            Err(crate::errno::from_darwin(result))
        };
    }
    let old_host_policy = HOST_POLICY.replace(true);
    let (policy, priority) = match policy {
        SCHED_FIFO => (libc::SCHED_FIFO, 47),
        SCHED_RR => (libc::SCHED_RR, 47),
        SCHED_IDLE => (libc::SCHED_OTHER, 15),
        SCHED_BATCH => (libc::SCHED_OTHER, timeshare_priority(nice.max(10))),
        _ => (libc::SCHED_OTHER, timeshare_priority(nice)),
    };
    let floor_priority = match floor {
        25 => 37,
        33 => 46,
        _ => 0,
    };
    let ceiling_priority = match ceiling {
        9 => 4,
        17 => 20,
        _ => MAXPRI_USER,
    };
    // Darwin's non-QoS user range starts at 15; the separate background
    // thread policy applies MAXPRI_THROTTLE (4) below that range.
    let priority = priority.max(floor_priority).min(ceiling_priority).max(15);
    let priority = (priority + BASEPRI_DEFAULT - task_priority()).min(MAXPRI_USER);
    // SAFETY: sched_param is plain data; plain call on the calling thread.
    let result = unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = priority;
        libc::pthread_setschedparam(libc::pthread_self(), policy, &param)
    };
    if result != 0 {
        HOST_POLICY.set(old_host_policy);
        return Err(crate::errno::from_darwin(result));
    }
    // DARWIN_THREAD is used only after leaving QoS: XNU removes a thread's
    // QoS policy and returns EIDRM if both APIs are mixed.
    let result = unsafe {
        libc::setpriority(
            libc::PRIO_DARWIN_THREAD,
            0,
            if ceiling == 9 {
                libc::PRIO_DARWIN_BG
            } else {
                0
            },
        )
    };
    if result < 0 {
        return Err(crate::errno::last());
    }
    Ok(())
}

/// The task's base priority changed (the host process's nice value): every
/// other thread applies its scheduling again at its next syscall.
pub fn task_priority_changed() {
    let me = thread::gettid() as i32;
    thread::with_table(|t| {
        for th in t.values().filter(|th| th.tid != me) {
            th.sched.host_stale.store(true, SeqCst);
        }
    });
}

/// Apply the calling thread's scheduling, changed by another thread, to
/// the host.
pub fn apply_stale_sched(th: &Thread) -> Result<(), crate::errno::Errno> {
    if th.sched.host_stale.load(SeqCst) && th.sched.host_stale.swap(false, SeqCst) {
        if let Err(error) = apply_host_sched(
            super::cred::thread_nice(th.tid),
            th.sched.policy.load(SeqCst),
        ) {
            th.sched.host_stale.store(true, SeqCst);
            return Err(error);
        }
    }
    Ok(())
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
/// (120), sched_getparam (121). The policy and priority are recorded per
/// thread and set its host scheduling ([`apply_host_sched`]); another
/// process's threads are checked, not changed (#1616).
pub fn sched_policy(nr: u64, a: [u64; 6]) -> i64 {
    let mut a = a;
    a[0] = match super::pidns::syscall_pid(a[0] as i32) {
        Ok(pid) => pid as u64,
        Err(error) => return error,
    };
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
                    if th.is_none()
                        && let Err(e) = may_change(a[0] as i64)
                    {
                        return e;
                    }
                    if let Some(th) = &th {
                        th.sched.policy.store(policy, SeqCst);
                        th.sched.priority.store(prio, SeqCst);
                        if let Err(error) = super::cgroup::refresh_own() {
                            return -(error as i64);
                        }
                        if th.tid == thread::gettid() as i32 {
                            if let Err(error) =
                                apply_host_sched(super::cred::thread_nice(th.tid), policy)
                            {
                                return -(error as i64);
                            }
                        } else {
                            th.sched.host_stale.store(true, SeqCst);
                        }
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
    let mut a = a;
    a[0] = match super::pidns::syscall_pid(a[0] as i32) {
        Ok(pid) => pid as u64,
        Err(error) => return error,
    };
    match sched_target(a[0] as i64) {
        Ok(Some(_)) => 0,
        Ok(None) => may_change(a[0] as i64).map_or_else(|e| e, |_| 0),
        Err(e) => e,
    }
}

/// sched_getaffinity: every online host CPU. Returns the mask size in bytes
/// as the kernel does (a multiple of 8 covering the CPUs).
pub fn sched_getaffinity(a: [u64; 6]) -> i64 {
    let mut a = a;
    a[0] = match super::pidns::syscall_pid(a[0] as i32) {
        Ok(pid) => pid as u64,
        Err(error) => return error,
    };
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
