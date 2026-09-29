//! Process state kept by the host kernel (sessions, process groups, umask,
//! resource usage) and what the guest reads about the system: `uname`,
//! `sysinfo`, `getcpu` and the personality.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::errno::{self, EINVAL};

pub fn setsid() -> i64 {
    // SAFETY: trivial.
    errno::check(unsafe { libc::setsid() } as i64)
}

/// A pid argument where 0 means the caller: a process in the guest's pid
/// namespace (`pidns`).
fn member(pid: u64) -> Result<i32, i64> {
    match pid as i32 {
        0 => Ok(0),
        p => super::pidns::check(p),
    }
}

pub fn setpgid(a: [u64; 6]) -> i64 {
    if (a[1] as i32) < 0 {
        return -(EINVAL as i64);
    }
    let pid = match member(a[0]) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // SAFETY: trivial.
    errno::check(unsafe { libc::setpgid(pid, a[1] as i32) } as i64)
}

/// getpgid (155) and getsid (156): a group or session led from outside
/// the namespace is 0, as pid_vnr gives.
fn ns_id(pid: u64, get: unsafe extern "C" fn(libc::pid_t) -> libc::pid_t) -> i64 {
    let pid = match member(pid) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // SAFETY: trivial.
    match errno::check(unsafe { get(pid) } as i64) {
        id if id > 0 => super::pidns::id_in_ns(id as i32) as i64,
        r => r,
    }
}

pub fn getpgid(a: [u64; 6]) -> i64 {
    ns_id(a[0], libc::getpgid)
}

pub fn getsid(a: [u64; 6]) -> i64 {
    ns_id(a[0], libc::getsid)
}

pub fn umask(a: [u64; 6]) -> i64 {
    // SAFETY: trivial; the permission bits agree.
    unsafe { libc::umask((a[0] & 0o777) as libc::mode_t) as i64 }
}

const RUSAGE_SELF: i32 = 0;
const RUSAGE_CHILDREN: i32 = -1;
const RUSAGE_THREAD: i32 = 1;

unsafe extern "C" {
    fn mach_thread_self() -> libc::mach_port_t;
    static mach_task_self_: libc::mach_port_t;
    fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> i32;
    fn mach_host_self() -> libc::mach_port_t;
}

/// User and system time of the calling thread.
fn thread_times() -> Option<(libc::timeval, libc::timeval)> {
    // SAFETY: THREAD_BASIC_INFO into a local of the right size.
    unsafe {
        let port = mach_thread_self();
        let mut info: libc::thread_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::thread_basic_info>() / 4) as u32;
        let kr = libc::thread_info(
            port,
            libc::THREAD_BASIC_INFO as u32,
            (&mut info as *mut libc::thread_basic_info).cast(),
            &mut count,
        );
        mach_port_deallocate(mach_task_self_, port);
        if kr != 0 {
            return None;
        }
        let tv = |t: libc::time_value_t| libc::timeval {
            tv_sec: t.seconds as i64,
            tv_usec: t.microseconds,
        };
        Some((tv(info.user_time), tv(info.system_time)))
    }
}

/// getrusage (165). RUSAGE_THREAD takes the thread's times from Mach and
/// the rest from the process.
pub fn getrusage(a: [u64; 6]) -> i64 {
    let who = a[0] as i32;
    let host = match who {
        RUSAGE_SELF | RUSAGE_THREAD => libc::RUSAGE_SELF,
        RUSAGE_CHILDREN => libc::RUSAGE_CHILDREN,
        _ => return -(EINVAL as i64),
    };
    let r = super::wait::put_rusage_of(host, a[1]);
    if r == 0
        && who == RUSAGE_THREAD
        && let Some((u, s)) = thread_times()
    {
        // SAFETY: guest struct rusage: two leading { i64, i64 } timevals.
        unsafe {
            (a[1] as *mut [i64; 4]).write_unaligned([
                u.tv_sec,
                u.tv_usec as i64,
                s.tv_sec,
                s.tv_usec as i64,
            ])
        };
    }
    r
}

/// USER_HZ, the unit of `times` and AT_CLKTCK.
const USER_HZ: i64 = 100;

fn ticks(tv: libc::timeval) -> i64 {
    tv.tv_sec * USER_HZ + tv.tv_usec as i64 * USER_HZ / 1_000_000
}

/// Nanoseconds since boot (CLOCK_BOOTTIME).
fn uptime() -> u64 {
    super::clock::Base::Boottime.now()
}

/// times (153): struct tms in clock ticks; returns ticks since boot.
pub fn times(a: [u64; 6]) -> i64 {
    if a[0] != 0 {
        // SAFETY: local buffers.
        let mut me: libc::rusage = unsafe { std::mem::zeroed() };
        let mut kids: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe {
            libc::getrusage(libc::RUSAGE_SELF, &mut me);
            libc::getrusage(libc::RUSAGE_CHILDREN, &mut kids);
        }
        let t = [
            ticks(me.ru_utime),
            ticks(me.ru_stime),
            ticks(kids.ru_utime),
            ticks(kids.ru_stime),
        ];
        // SAFETY: guest struct tms of four clock_t.
        unsafe { (a[0] as *mut [i64; 4]).write_unaligned(t) };
    }
    (uptime() / (1_000_000_000 / USER_HZ as u64)) as i64
}

/// The kernel release the image expects: the newest kernel of its newest
/// framework compatibility matrix, named like a GKI build of the image's
/// Android release (for example `6.12.0-android16`).
pub fn kernel_release() -> &'static str {
    static RELEASE: OnceLock<String> = OnceLock::new();
    RELEASE.get_or_init(|| {
        let root = crate::vfs::root();
        let mut best: Option<(u64, [u64; 3])> = None;
        if let Ok(dir) = std::fs::read_dir(root.join("system/etc/vintf")) {
            for e in dir.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with("compatibility_matrix.") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(e.path()) else {
                    continue;
                };
                for k in text.split("<kernel ").skip(1) {
                    let attr = |a: &str| {
                        let rest = k.split_once(&format!("{a}=\""))?.1;
                        Some(rest.split_once('"')?.0.to_string())
                    };
                    let (Some(v), Some(l)) = (attr("version"), attr("level")) else {
                        continue;
                    };
                    let mut ver = [0u64; 3];
                    for (d, p) in ver.iter_mut().zip(v.split('.')) {
                        *d = p.parse().unwrap_or(0);
                    }
                    let cand = (l.parse().unwrap_or(0), ver);
                    if best.is_none_or(|b| cand > b) {
                        best = Some(cand);
                    }
                }
            }
        }
        let release = std::fs::read_to_string(root.join("system/build.prop"))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("ro.build.version.release="))
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let [a, b, c] = best.map_or([6, 12, 0], |b| b.1);
        format!("{a}.{b}.{c}-android{release}")
    })
}

/// Linux `struct utsname`: six 65-byte fields.
pub fn uname(a: [u64; 6]) -> i64 {
    let fields: [&[u8]; 6] = [
        b"Linux",
        b"localhost",
        kernel_release().as_bytes(),
        b"#1 SMP PREEMPT",
        b"aarch64",
        b"(none)",
    ];
    let mut out = [0u8; 65 * 6];
    for (i, f) in fields.iter().enumerate() {
        let n = f.len().min(64);
        out[i * 65..i * 65 + n].copy_from_slice(&f[..n]);
    }
    // SAFETY: guest utsname buffer.
    unsafe { std::ptr::copy_nonoverlapping(out.as_ptr(), a[0] as *mut u8, out.len()) };
    0
}

fn sysctl_u64(name: &std::ffi::CStr) -> u64 {
    let mut v = 0u64;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: sysctl reading an integer into a local.
    unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut v as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    v
}

/// Free physical memory in bytes.
fn free_memory() -> u64 {
    // SAFETY: HOST_VM_INFO64 into a local of the right size.
    unsafe {
        let mut info: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::vm_statistics64>() / 4) as u32;
        if libc::host_statistics64(
            mach_host_self(),
            libc::HOST_VM_INFO64,
            (&mut info as *mut libc::vm_statistics64).cast(),
            &mut count,
        ) != 0
        {
            return 0;
        }
        (info.free_count as u64 + info.inactive_count as u64) * libc::vm_page_size as u64
    }
}

/// sysinfo (179): Linux arm64 `struct sysinfo` (112 bytes, mem_unit 1).
/// `procs` counts the processes of the guest's pid namespace.
pub fn sysinfo(a: [u64; 6]) -> i64 {
    let mut load = [0f64; 3];
    // SAFETY: local buffer.
    unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    let procs = match super::pidns::members() {
        Some(m) => m.len() as i32,
        // SAFETY: counting pids.
        None => unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) }.max(0),
    };
    let mut w = [0u64; 14];
    w[0] = uptime() / 1_000_000_000;
    for i in 0..3 {
        w[1 + i] = (load[i] * 65536.0) as u64;
    }
    w[4] = sysctl_u64(c"hw.memsize");
    w[5] = free_memory();
    // sharedram, bufferram, totalswap, freeswap stay 0.
    w[10] = procs.min(u16::MAX as i32) as u64;
    // totalhigh and freehigh are 0; mem_unit is 1.
    w[13] = 1;
    // SAFETY: guest struct sysinfo.
    unsafe { (a[0] as *mut [u64; 14]).write_unaligned(w) };
    0
}

/// getcpu (168): Darwin keeps the CPU number in the low bits of TPIDR_EL0.
pub fn getcpu(a: [u64; 6]) -> i64 {
    let tp: u64;
    // SAFETY: reading TPIDR_EL0 is always permitted at EL0.
    unsafe { std::arch::asm!("mrs {}, tpidr_el0", out(reg) tp) };
    let ncpu = std::thread::available_parallelism().map_or(1, |n| n.get()) as u64;
    let cpu = ((tp & 0xfff) % ncpu) as u32;
    // SAFETY: guest unsigned ints.
    unsafe {
        if a[0] != 0 {
            (a[0] as *mut u32).write_unaligned(cpu);
        }
        if a[1] != 0 {
            (a[1] as *mut u32).write_unaligned(0);
        }
    }
    0
}

static PERSONALITY: AtomicU32 = AtomicU32::new(0);

pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    w.u32(personality_value());
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    set_personality(r.u32());
}

pub fn personality_value() -> u32 {
    PERSONALITY.load(Ordering::Relaxed)
}

pub fn set_personality(p: u32) {
    PERSONALITY.store(p, Ordering::Relaxed);
}

/// personality (92): 0xffffffff queries; returns the previous value.
pub fn personality(a: [u64; 6]) -> i64 {
    let p = a[0] as u32;
    if p == u32::MAX {
        return personality_value() as i64;
    }
    PERSONALITY.swap(p, Ordering::Relaxed) as i64
}
