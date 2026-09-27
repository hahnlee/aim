//! `/proc` and `/sys`, synthesized from Darwin state (after FreeBSD's
//! linprocfs/linsysfs, BSD-2-Clause).
//!
//! - Files are generated when opened and handed to the guest as an unlinked
//!   temporary file, so reads, seeks and fstat behave like a regular file.
//! - Directories are directory streams with fixed entries (see `dir`).
//! - Values init wrote under `<runtime>/kernfs/...`
//!   (`docs/guest-init-contract.md` section 7) take precedence over the
//!   synthesized ones and may be rewritten by the guest.
//! - `/proc/<pid>` for another process is read from Darwin's process info;
//!   memory maps and fds are only available for this process.

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use super::dir::{self, DirStream, Entry};
use super::fdtab::{self, Kind};
use super::vmmap;
use crate::errno::{self, EACCES, ENOENT, Errno};
use crate::vfs::{self, Area};

/// What a synthesized path is.
pub enum Node {
    File(Vec<u8>),
    Dir(Vec<Entry>),
    Link(String),
}

/// The main thread's stack and argument areas, recorded by the loader.
#[derive(Clone, Copy, Default)]
pub struct StackInfo {
    pub lo: u64,
    pub hi: u64,
    pub start_stack: u64,
    pub args: (u64, u64),
    pub env: (u64, u64),
}

static STACK: OnceLock<StackInfo> = OnceLock::new();

pub fn note_stack(s: StackInfo) {
    let _ = STACK.set(s);
}

fn pid() -> i32 {
    super::process::getpid() as i32
}

/// Guest path of an open host fd.
pub fn fd_guest_path(fd: i32) -> Result<String, Errno> {
    if let Some(p) = crate::xrt::original_guest_path(fd) {
        return Ok(p);
    }
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return Err(errno::last());
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let host = PathBuf::from(String::from_utf8_lossy(&buf[..len]).into_owned());
    Ok(vfs::guest_path_of_host(&host).unwrap_or_else(|| host.display().to_string()))
}

/// Target of `/proc/self/fd/N`.
fn fd_link(fd: i32) -> Option<String> {
    if fdtab::is_hidden(fd) {
        return None;
    }
    if let Some(n) = fdtab::anon_name(fd) {
        return Some(n);
    }
    // A synthesized /proc or /sys directory: its host fd is the root.
    if let Some(p) = super::dir::synthesized_path(fd) {
        return Some(p);
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return None;
    }
    if let Some(n) = super::memfd::link_name(st.st_dev as u32 as u64, st.st_ino) {
        return Some(n);
    }
    if let Some(n) = super::ashmem::link_name(&st) {
        return Some(n);
    }
    match st.st_mode & libc::S_IFMT {
        libc::S_IFSOCK => return Some(format!("socket:[{}]", st.st_ino)),
        libc::S_IFIFO => return Some(format!("pipe:[{}]", st.st_ino)),
        _ => {}
    }
    fd_guest_path(fd).ok()
}

// ---- process information ---------------------------------------------------

fn task_info(pid: i32) -> Option<libc::proc_taskallinfo> {
    let mut t: libc::proc_taskallinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskallinfo>() as i32;
    // SAFETY: proc_pidinfo writes at most `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKALLINFO,
            0,
            (&mut t as *mut libc::proc_taskallinfo).cast(),
            size,
        )
    };
    (n == size).then_some(t)
}

/// argv of a host process running `linux-run`, without its own options.
fn host_argv(pid: i32) -> Vec<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; 256 << 10];
    let mut len = buf.len();
    // SAFETY: sysctl into our buffer.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } < 0
        || len < 4
    {
        return Vec::new();
    }
    let argc = i32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    let mut rest = &buf[4..len];
    // exec path, then NUL padding, then argv.
    let skip = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    rest = &rest[skip..];
    let start = rest.iter().position(|&c| c != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    let argv: Vec<Vec<u8>> = rest
        .split(|&c| c == 0)
        .take(argc)
        .map(<[u8]>::to_vec)
        .collect();
    if argv.first().is_some_and(|a| a.ends_with(b"linux-run")) {
        let mut i = 1;
        while i < argv.len() && argv[i].starts_with(b"--") {
            let takes_value = !matches!(&argv[i][..], b"--trace" | b"--inherit-env");
            i += if takes_value { 2 } else { 1 };
        }
        return argv[i.min(argv.len())..].to_vec();
    }
    argv
}

fn read_guest(lo: u64, hi: u64) -> Vec<u8> {
    let mut v = vec![0u8; hi.saturating_sub(lo) as usize];
    let mut got = 0u64;
    // SAFETY: a fault-safe copy from our own task.
    unsafe {
        mach_vm_read_overwrite(
            mach_task_self_,
            lo,
            v.len() as u64,
            v.as_mut_ptr() as u64,
            &mut got,
        )
    };
    v.truncate(got as usize);
    v
}

unsafe extern "C" {
    static mach_task_self_: libc::mach_port_t;
    fn mach_vm_read_overwrite(t: libc::mach_port_t, a: u64, s: u64, d: u64, o: *mut u64) -> i32;
}

fn cmdline(p: i32) -> Vec<u8> {
    if p == pid()
        && let Some(s) = STACK.get()
    {
        return read_guest(s.args.0, s.args.1);
    }
    let mut out = Vec::new();
    for a in host_argv(p) {
        out.extend_from_slice(&a);
        out.push(0);
    }
    out
}

fn comm(p: i32) -> String {
    let argv0 = if p == pid() {
        super::process::exe_guest_path()
    } else {
        host_argv(p)
            .first()
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .unwrap_or_default()
    };
    let base = argv0.rsplit('/').next().unwrap_or("").to_string();
    base.chars().take(15).collect()
}

fn boot_time() -> (i64, i64) {
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut len = std::mem::size_of::<libc::timeval>();
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    // SAFETY: sysctl into a local timeval.
    unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut tv as *mut libc::timeval).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (tv.tv_sec, tv.tv_usec as i64)
}

fn now() -> f64 {
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    // SAFETY: local timeval.
    unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };
    tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6
}

fn ncpu() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

const TICKS: u64 = 100;

fn stat_line(p: i32) -> Option<String> {
    let t = task_info(p)?;
    let b = &t.pbsd;
    let ti = &t.ptinfo;
    let state = match b.pbi_status {
        2 => 'R',
        4 => 'T',
        5 => 'Z',
        _ => 'S',
    };
    let (boot, _) = boot_time();
    let start = (b.pbi_start_tvsec as i64 - boot).max(0) as u64 * TICKS
        + b.pbi_start_tvusec as u64 * TICKS / 1_000_000;
    let ns_to_ticks = |ns: u64| ns / (1_000_000_000 / TICKS);
    let s = if p == pid() {
        STACK.get().copied().unwrap_or_default()
    } else {
        StackInfo::default()
    };
    let rss_pages = ti.pti_resident_size / 4096;
    Some(format!(
        "{p} ({}) {state} {} {} {} 0 -1 4194560 {} 0 {} 0 {} {} 0 0 20 {} {} 0 {start} {} {rss_pages} 18446744073709551615 0 0 {} 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 {} {} {} {} 0\n",
        comm(p),
        b.pbi_ppid,
        b.pbi_pgid,
        b.pbi_pgid,
        ti.pti_faults,
        ti.pti_pageins,
        ns_to_ticks(ti.pti_total_user),
        ns_to_ticks(ti.pti_total_system),
        b.pbi_nice,
        ti.pti_threadnum,
        ti.pti_virtual_size,
        s.start_stack,
        s.args.0,
        s.args.1,
        s.env.0,
        s.env.1,
    ))
}

/// The credential lines of `/proc/<p>/status`: the identity's for this
/// process; uid and gid from the process table for others.
fn cred_lines(p: i32) -> String {
    if p == pid() {
        return super::cred::proc_status();
    }
    let c = super::cred::peer(p);
    let (u, g) = (c.uid, c.gid);
    let caps = if u == 0 {
        "000001ffffffffff"
    } else {
        "0000000000000000"
    };
    format!(
        "Uid:\t{u}\t{u}\t{u}\t{u}\nGid:\t{g}\t{g}\t{g}\t{g}\nGroups:\t\n\
         CapInh:\t0000000000000000\nCapPrm:\t{caps}\nCapEff:\t{caps}\nCapBnd:\t{caps}\n\
         CapAmb:\t0000000000000000\n"
    )
}

fn status(p: i32) -> Option<String> {
    let t = task_info(p)?;
    let state = match t.pbsd.pbi_status {
        2 => "R (running)",
        4 => "T (stopped)",
        5 => "Z (zombie)",
        _ => "S (sleeping)",
    };
    let kb = |b: u64| b / 1024;
    let n = ncpu();
    Some(format!(
        "Name:\t{}\nUmask:\t0022\nState:\t{state}\nTgid:\t{p}\nNgid:\t0\nPid:\t{p}\nPPid:\t{}\nTracerPid:\t0\n\
         {}FDSize:\t256\n\
         VmPeak:\t{} kB\nVmSize:\t{} kB\nVmLck:\t0 kB\nVmPin:\t0 kB\nVmHWM:\t{} kB\nVmRSS:\t{} kB\n\
         RssAnon:\t{} kB\nRssFile:\t0 kB\nRssShmem:\t0 kB\nVmData:\t{} kB\nVmStk:\t8192 kB\nVmExe:\t0 kB\n\
         VmLib:\t0 kB\nVmPTE:\t0 kB\nVmSwap:\t0 kB\nThreads:\t{}\nSigQ:\t0/0\nSigPnd:\t0000000000000000\n\
         ShdPnd:\t0000000000000000\nSigBlk:\t0000000000000000\nSigIgn:\t0000000000000000\n\
         SigCgt:\t0000000000000000\nNoNewPrivs:\t0\nSeccomp:\t0\n\
         Cpus_allowed:\t{:x}\nCpus_allowed_list:\t0-{}\nvoluntary_ctxt_switches:\t{}\nnonvoluntary_ctxt_switches:\t0\n",
        comm(p),
        t.pbsd.pbi_ppid,
        cred_lines(p),
        kb(t.ptinfo.pti_virtual_size),
        kb(t.ptinfo.pti_virtual_size),
        kb(t.ptinfo.pti_resident_size),
        kb(t.ptinfo.pti_resident_size),
        kb(t.ptinfo.pti_resident_size),
        kb(t.ptinfo.pti_virtual_size),
        t.ptinfo.pti_threadnum,
        (1u64 << n) - 1,
        n - 1,
        t.ptinfo.pti_csw,
    ))
}

fn statm(p: i32) -> Option<String> {
    let t = task_info(p)?;
    let pg = |b: u64| b / 4096;
    Some(format!(
        "{} {} 0 0 0 {} 0\n",
        pg(t.ptinfo.pti_virtual_size),
        pg(t.ptinfo.pti_resident_size),
        pg(t.ptinfo.pti_resident_size)
    ))
}

/// Linux-format `/proc/self/maps` from the VM map.
fn maps() -> String {
    let stack = STACK.get().copied().unwrap_or_default();
    let mut out = String::new();
    for r in vmmap::regions(0, u64::MAX) {
        let prot = r.prot;
        let perms = format!(
            "{}{}{}{}",
            if prot & 1 != 0 { 'r' } else { '-' },
            if prot & 2 != 0 { 'w' } else { '-' },
            if prot & 4 != 0 { 'x' } else { '-' },
            if r.shared { 's' } else { 'p' }
        );
        let (mut offset, mut dev, mut ino, mut name) = (0u64, 0u64, 0u64, String::new());
        if let Some((path, d, i)) = &r.file {
            offset = r.offset;
            dev = *d;
            ino = *i;
            name = super::memfd::link_name(*d, *i)
                .or_else(|| crate::xrt::original_guest_path_of_host(path))
                .or_else(|| vfs::guest_path_of_host(path))
                .unwrap_or_else(|| path.display().to_string());
        } else if let Some((c, off)) = super::copies::find(r.start) {
            offset = off;
            dev = c.dev;
            ino = c.ino;
            name = c.guest;
        } else if r.start < stack.hi && stack.lo < r.end && prot != 0 {
            name = "[stack]".into();
        } else if let Some(n) = super::memfd::anon_name(r.start) {
            name = n;
        }
        let line = format!(
            "{:08x}-{:08x} {perms} {offset:08x} {:02x}:{:02x} {ino}",
            r.start,
            r.end,
            (dev >> 24) & 0xff,
            dev & 0xff_ffff
        );
        out.push_str(&line);
        if !name.is_empty() {
            out.push_str(&" ".repeat(73usize.saturating_sub(line.len()).max(1)));
            out.push_str(&name);
        }
        out.push('\n');
    }
    out
}

fn sysctl_u64(name: &str) -> u64 {
    let c = CString::new(name).unwrap();
    let mut v = 0u64;
    let mut len = 8usize;
    // SAFETY: sysctlbyname into a u64.
    unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            (&mut v as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    v
}

fn sysctl_string(name: &str) -> String {
    let c = CString::new(name).unwrap();
    let mut buf = [0u8; 256];
    let mut len = buf.len();
    // SAFETY: sysctlbyname into our buffer.
    unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    String::from_utf8_lossy(&buf[..len.min(buf.len())])
        .trim_end_matches('\0')
        .to_string()
}

unsafe extern "C" {
    fn mach_host_self() -> u32;
    fn host_statistics64(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    fn host_processor_info(
        host: u32,
        flavor: i32,
        count: *mut u32,
        info: *mut *mut i32,
        n: *mut u32,
    ) -> i32;
    fn mach_vm_deallocate(t: libc::mach_port_t, a: u64, s: u64) -> i32;
}

fn meminfo() -> String {
    let total = sysctl_u64("hw.memsize");
    let page = 16384u64;
    // vm_statistics64: free, active, inactive, wire, ... (natural_t counts).
    let mut v = [0i32; 64];
    let mut count = 38u32;
    // SAFETY: HOST_VM_INFO64 (4) into a large enough buffer.
    unsafe { host_statistics64(mach_host_self(), 4, v.as_mut_ptr(), &mut count) };
    let pages = |i: usize| v[i] as u32 as u64 * page / 1024;
    let (free, active, inactive, wired) = (pages(0), pages(1), pages(2), pages(3));
    // external_page_count (file-backed) is field 29 of vm_statistics64.
    let cached = pages(29);
    let avail = free + inactive;
    let kb = total / 1024;
    format!(
        "MemTotal:       {kb:>8} kB\nMemFree:        {free:>8} kB\nMemAvailable:   {avail:>8} kB\n\
         Buffers:               0 kB\nCached:         {cached:>8} kB\nSwapCached:            0 kB\n\
         Active:         {active:>8} kB\nInactive:       {inactive:>8} kB\nUnevictable:    {wired:>8} kB\n\
         Mlocked:               0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n\
         Dirty:                 0 kB\nWriteback:             0 kB\nAnonPages:      {active:>8} kB\n\
         Mapped:         {cached:>8} kB\nShmem:                 0 kB\nSlab:                  0 kB\n\
         SReclaimable:          0 kB\nSUnreclaim:            0 kB\nKernelStack:           0 kB\n\
         PageTables:            0 kB\nCommitLimit:    {kb:>8} kB\nCommitted_AS:   {:>8} kB\n\
         VmallocTotal:   {:>8} kB\nVmallocUsed:           0 kB\nVmallocChunk:          0 kB\n",
        kb - free,
        1u64 << 37,
    )
}

const HWCAP_NAMES: [&str; 32] = [
    "fp", "asimd", "evtstrm", "aes", "pmull", "sha1", "sha2", "crc32", "atomics", "fphp",
    "asimdhp", "cpuid", "asimdrdm", "jscvt", "fcma", "lrcpc", "dcpop", "sha3", "sm3", "sm4",
    "asimddp", "sha512", "sve", "asimdfhm", "dit", "uscat", "ilrcpc", "flagm", "ssbs", "sb",
    "paca", "pacg",
];
const HWCAP2_NAMES: [&str; 38] = [
    "dcpodp",
    "sve2",
    "sveaes",
    "svepmull",
    "svebitperm",
    "svesha3",
    "svesm4",
    "flagm2",
    "frint",
    "svei8mm",
    "svef32mm",
    "svef64mm",
    "svebf16",
    "i8mm",
    "bf16",
    "dgh",
    "rng",
    "bti",
    "mte",
    "ecv",
    "afp",
    "rpres",
    "mte3",
    "sme",
    "smei16i64",
    "smef64f64",
    "smei8i32",
    "smef16f32",
    "smeb16f32",
    "smef32f32",
    "smefa64",
    "wfxt",
    "ebf16",
    "sveebf16",
    "cssc",
    "rprfm",
    "sve2p1",
    "sme2",
];

fn cpuinfo() -> String {
    let (h, h2) = crate::hwcap::host_hwcaps();
    let mut feats: Vec<&str> = (0..32)
        .filter(|b| h & (1 << b) != 0)
        .map(|b| HWCAP_NAMES[b])
        .collect();
    feats.extend(
        (0..HWCAP2_NAMES.len())
            .filter(|b| h2 & (1 << b) != 0)
            .map(|b| HWCAP2_NAMES[b]),
    );
    let feats = feats.join(" ");
    let mut out = String::new();
    for i in 0..ncpu() {
        out.push_str(&format!(
            "processor\t: {i}\nBogoMIPS\t: 48.00\nFeatures\t: {feats}\nCPU implementer\t: 0x61\n\
             CPU architecture: 8\nCPU variant\t: 0x0\nCPU part\t: 0x000\nCPU revision\t: 0\n\n"
        ));
    }
    out
}

fn proc_stat() -> String {
    let mut n = 0u32;
    let mut info: *mut i32 = std::ptr::null_mut();
    let mut cnt = 0u32;
    // SAFETY: PROCESSOR_CPU_LOAD_INFO (2): per-CPU [user, system, idle, nice]
    // tick counts in a vm_allocated array we release below.
    let ok = unsafe { host_processor_info(mach_host_self(), 2, &mut n, &mut info, &mut cnt) } == 0;
    let mut cpus = Vec::new();
    if ok {
        for i in 0..n as usize {
            // SAFETY: n entries of 4 ints.
            let t = unsafe { std::slice::from_raw_parts(info.add(i * 4), 4) };
            cpus.push([
                t[0] as u32 as u64,
                t[3] as u32 as u64,
                t[1] as u32 as u64,
                t[2] as u32 as u64,
            ]);
        }
        // SAFETY: releasing the kernel's array.
        unsafe { mach_vm_deallocate(mach_task_self_, info as u64, cnt as u64 * 4) };
    }
    let mut total = [0u64; 4];
    for c in &cpus {
        for k in 0..4 {
            total[k] += c[k];
        }
    }
    let line = |name: &str, c: &[u64; 4]| {
        format!("{name} {} {} {} {} 0 0 0 0 0 0\n", c[0], c[1], c[2], c[3])
    };
    let mut out = line("cpu ", &total);
    for (i, c) in cpus.iter().enumerate() {
        out.push_str(&line(&format!("cpu{i}"), c));
    }
    out.push_str(&format!(
        "intr 0\nctxt 0\nbtime {}\nprocesses 0\nprocs_running 1\nprocs_blocked 0\nsoftirq 0\n",
        boot_time().0
    ));
    out
}

fn uptime() -> String {
    let (s, us) = boot_time();
    let up = now() - (s as f64 + us as f64 / 1e6);
    format!("{up:.2} {:.2}\n", up * ncpu() as f64 * 0.9)
}

fn loadavg() -> String {
    let mut l = [0f64; 3];
    // SAFETY: three doubles.
    unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    format!("{:.2} {:.2} {:.2} 1/1 {}\n", l[0], l[1], l[2], pid())
}

fn uuid(bytes: [u8; 16]) -> String {
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}\n",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn random_uuid() -> String {
    let mut b = [0u8; 16];
    // SAFETY: local buffer.
    unsafe { libc::getentropy(b.as_mut_ptr().cast(), 16) };
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    uuid(b)
}

fn cpu_range() -> String {
    format!("0-{}\n", ncpu() - 1)
}

fn pids() -> Vec<i32> {
    // SAFETY: sizing call then a buffer of that size.
    unsafe {
        let n = libc::proc_listallpids(std::ptr::null_mut(), 0);
        let mut v = vec![0i32; n.max(0) as usize + 64];
        let n = libc::proc_listallpids(v.as_mut_ptr().cast(), (v.len() * 4) as i32);
        v.truncate(n.max(0) as usize);
        v.retain(|&p| p > 0);
        v.sort_unstable();
        v
    }
}

/// The tids of process `p`: the guest threads for this process; other
/// processes' threads are not listed.
fn tids(p: i32) -> Vec<i32> {
    let t = if p == pid() {
        super::thread::tids()
    } else {
        Vec::new()
    };
    if t.is_empty() { vec![p] } else { t }
}

fn entries(names: &[(&str, u8)]) -> Vec<Entry> {
    names
        .iter()
        .map(|(n, t)| Entry::new(1, *t, n.as_bytes()))
        .collect()
}

const PID_ENTRIES: &[(&str, u8)] = &[
    ("attr", dir::DT_DIR),
    ("cmdline", dir::DT_REG),
    ("comm", dir::DT_REG),
    ("cwd", dir::DT_LNK),
    ("environ", dir::DT_REG),
    ("exe", dir::DT_LNK),
    ("fd", dir::DT_DIR),
    ("maps", dir::DT_REG),
    ("mountinfo", dir::DT_REG),
    ("mounts", dir::DT_REG),
    ("oom_score_adj", dir::DT_REG),
    ("root", dir::DT_LNK),
    ("stat", dir::DT_REG),
    ("statm", dir::DT_REG),
    ("status", dir::DT_REG),
    ("task", dir::DT_DIR),
];

/// The persistent areas of the guest-init contract (docs/guest-init-contract.md,
/// section 2), which Android mounts from block devices; the other `rw` areas
/// are per boot, as tmpfs.
const BLOCK_MOUNTS: &[&str] = &["/data", "/metadata", "/cache"];

/// The mount table: the read-only image at `/` and the path map's areas.
/// Each is `(source, target, fstype, options)`.
fn mount_table() -> Vec<(String, String, &'static str, &'static str)> {
    let mut out = vec![("/dev/root".into(), "/".into(), "erofs", "ro,relatime")];
    for (guest, area) in vfs::mount_points() {
        let (source, fstype, options) = match (area, guest.as_str()) {
            (vfs::Area::Kernfs, "/proc") => {
                ("proc".into(), "proc", "rw,nosuid,nodev,noexec,relatime")
            }
            (vfs::Area::Kernfs, "/sys") => {
                ("sysfs".into(), "sysfs", "rw,nosuid,nodev,noexec,relatime")
            }
            (vfs::Area::Writable, g) if BLOCK_MOUNTS.contains(&g) => (
                format!("/dev/block/by-name{g}"),
                "ext4",
                "rw,nosuid,nodev,noatime",
            ),
            (vfs::Area::Writable, _) => ("tmpfs".into(), "tmpfs", "rw,nosuid,relatime"),
            _ => continue,
        };
        out.push((source, guest, fstype, options));
    }
    out
}

/// `/proc/<pid>/mounts`, as fstab(5) lines.
fn mounts() -> String {
    mount_table()
        .into_iter()
        .map(|(source, target, fstype, options)| {
            format!("{source} {target} {fstype} {options} 0 0\n")
        })
        .collect()
}

/// `/proc/<pid>/mountinfo` (proc(5)): `/` is mount 1 and the parent of the
/// others.
fn mountinfo() -> String {
    mount_table()
        .into_iter()
        .enumerate()
        .map(|(i, (source, target, fstype, options))| {
            let id = i + 1;
            let parent = if i == 0 { 0 } else { 1 };
            let rw = options.split(',').next().unwrap_or("rw");
            format!("{id} {parent} 0:{id} / {target} {options} - {fstype} {source} {rw}\n")
        })
        .collect()
}

/// The `comm` of this process's thread `tid`, once it has been named.
fn thread_comm(tid: i32) -> Option<String> {
    let n = super::thread::name_of(tid)?;
    let len = n.iter().position(|&b| b == 0).unwrap_or(16);
    (len > 0).then(|| String::from_utf8_lossy(&n[..len]).into_owned())
}

/// Nodes under `/proc/<p>/` (`rest` is the path after it); `thread` is the
/// tid for `/proc/<p>/task/<tid>/` (and `/proc/<tid>/`).
fn pid_node(p: i32, rest: &str, thread: Option<i32>) -> Option<Node> {
    let me = p == pid();
    if !me && task_info(p).is_none() {
        return None;
    }
    Some(match rest {
        "" => {
            let e = PID_ENTRIES
                .iter()
                .filter(|(n, _)| thread.is_none() || *n != "task");
            Node::Dir(e.map(|(n, t)| Entry::new(1, *t, n.as_bytes())).collect())
        }
        "cmdline" => Node::File(cmdline(p)),
        "comm" => {
            let named = me.then(|| thread_comm(thread.unwrap_or(p))).flatten();
            Node::File(format!("{}\n", named.unwrap_or_else(|| comm(p))).into_bytes())
        }
        "environ" if me => Node::File(
            STACK
                .get()
                .map_or(Vec::new(), |s| read_guest(s.env.0, s.env.1)),
        ),
        "stat" => Node::File(stat_line(p)?.into_bytes()),
        "status" => Node::File(status(p)?.into_bytes()),
        "statm" => Node::File(statm(p)?.into_bytes()),
        "maps" if me => Node::File(maps().into_bytes()),
        "mounts" => Node::File(mounts().into_bytes()),
        "mountinfo" => Node::File(mountinfo().into_bytes()),
        "oom_score_adj" if me => {
            Node::File(format!("{}\n", super::cred::oom_score_adj()).into_bytes())
        }
        "oom_score_adj" => Node::File(b"0\n".to_vec()),
        "attr" => Node::Dir(entries(&[("current", dir::DT_REG)])),
        "attr/current" => Node::File(attr_current()),
        "exe" if me => Node::Link(super::process::exe_guest_path()),
        "cwd" if me => Node::Link(vfs::cwd()),
        "root" => Node::Link("/".into()),
        "fd" if me => Node::Dir(
            fdtab::open_fds()
                .into_iter()
                .filter(|&fd| !fdtab::is_hidden(fd))
                .map(|fd| Entry::new(fd as u64 + 1, dir::DT_LNK, fd.to_string()))
                .collect(),
        ),
        "task" if thread.is_none() => Node::Dir(
            tids(p)
                .into_iter()
                .map(|t| Entry::new(t as u64, dir::DT_DIR, t.to_string()))
                .collect(),
        ),
        _ => {
            if let Some(n) = rest.strip_prefix("fd/").filter(|_| me) {
                let fd: i32 = n.parse().ok()?;
                return fd_link(fd).map(Node::Link);
            }
            let t = rest.strip_prefix("task/").filter(|_| thread.is_none())?;
            let (tid, sub) = t.split_once('/').unwrap_or((t, ""));
            let tid: i32 = tid.parse().ok()?;
            if !tids(p).contains(&tid) {
                return None;
            }
            return pid_node(p, sub, Some(tid));
        }
    })
}

/// The synthesized node at a normalized guest path under /proc or /sys.
pub fn node(guest: &str) -> Option<Node> {
    if let Some(rest) = guest.strip_prefix("/proc") {
        let rest = rest.trim_start_matches('/');
        let (first, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let me = pid();
        return Some(match first {
            "" => {
                let mut e = entries(&[
                    ("self", dir::DT_LNK),
                    ("thread-self", dir::DT_LNK),
                    ("cpuinfo", dir::DT_REG),
                    ("filesystems", dir::DT_REG),
                    ("loadavg", dir::DT_REG),
                    ("meminfo", dir::DT_REG),
                    ("mounts", dir::DT_LNK),
                    ("stat", dir::DT_REG),
                    ("sys", dir::DT_DIR),
                    ("uptime", dir::DT_REG),
                    ("version", dir::DT_REG),
                ]);
                e.extend(
                    pids()
                        .into_iter()
                        .map(|p| Entry::new(p as u64, dir::DT_DIR, p.to_string())),
                );
                Node::Dir(e)
            }
            "self" if tail.is_empty() => Node::Link(me.to_string()),
            "mounts" if tail.is_empty() => Node::Link("self/mounts".into()),
            "thread-self" if tail.is_empty() => {
                Node::Link(format!("{me}/task/{}", super::process::gettid()))
            }
            "self" => return pid_node(me, tail, None),
            "thread-self" => {
                return pid_node(me, tail, Some(super::process::gettid() as i32));
            }
            "cpuinfo" => Node::File(cpuinfo().into_bytes()),
            "meminfo" => Node::File(meminfo().into_bytes()),
            "stat" => Node::File(proc_stat().into_bytes()),
            "uptime" => Node::File(uptime().into_bytes()),
            "loadavg" => Node::File(loadavg().into_bytes()),
            "version" => Node::File(
                b"Linux version 6.6.0-darwin (darwin-linux-abi) #1 SMP PREEMPT\n".to_vec(),
            ),
            "filesystems" => {
                Node::File(b"nodev\tsysfs\nnodev\tproc\nnodev\ttmpfs\n\text4\n".to_vec())
            }
            "sys" => return sys_node(tail),
            n => {
                // `/proc/<tid>` of a thread other than a main thread: the
                // thread's view of its process, as on Linux.
                let n: i32 = n.parse().ok()?;
                let p = super::thread::owner(n);
                if p == n {
                    return pid_node(p, tail, None);
                }
                if p == me && !tids(p).contains(&n) {
                    return None;
                }
                return pid_node(p, tail, Some(n));
            }
        });
    }
    let rest = guest.strip_prefix("/sys")?.trim_start_matches('/');
    if let Some(t) = rest.strip_prefix("fs/selinux")
        && (t.is_empty() || t.starts_with('/'))
    {
        return super::selinuxfs::node(t);
    }
    Some(match rest {
        "" => Node::Dir(entries(&[
            ("devices", dir::DT_DIR),
            ("fs", dir::DT_DIR),
            ("kernel", dir::DT_DIR),
        ])),
        "fs" => Node::Dir(entries(&[("selinux", dir::DT_DIR)])),
        "kernel" => Node::Dir(entries(&[("debug", dir::DT_DIR), ("tracing", dir::DT_DIR)])),
        "kernel/debug" => Node::Dir(entries(&[("tracing", dir::DT_DIR)])),
        "kernel/tracing" | "kernel/debug/tracing" => Node::Dir(entries(&[
            ("trace_marker", dir::DT_REG),
            ("tracing_on", dir::DT_REG),
        ])),
        "kernel/tracing/tracing_on" | "kernel/debug/tracing/tracing_on" => {
            Node::File(b"0\n".to_vec())
        }
        "kernel/tracing/trace_marker" | "kernel/debug/tracing/trace_marker" => {
            Node::File(Vec::new())
        }
        "devices" => Node::Dir(entries(&[("system", dir::DT_DIR)])),
        "devices/system" => Node::Dir(entries(&[("cpu", dir::DT_DIR)])),
        "devices/system/cpu" => {
            let mut e = entries(&[
                ("kernel_max", dir::DT_REG),
                ("online", dir::DT_REG),
                ("possible", dir::DT_REG),
                ("present", dir::DT_REG),
            ]);
            e.extend((0..ncpu()).map(|i| Entry::new(1, dir::DT_DIR, format!("cpu{i}"))));
            Node::Dir(e)
        }
        "devices/system/cpu/possible"
        | "devices/system/cpu/present"
        | "devices/system/cpu/online" => Node::File(cpu_range().into_bytes()),
        "devices/system/cpu/kernel_max" => Node::File(format!("{}\n", ncpu() - 1).into_bytes()),
        _ => {
            let c = rest.strip_prefix("devices/system/cpu/cpu")?;
            let (n, tail) = c.split_once('/').unwrap_or((c, ""));
            if n.parse::<usize>().ok()? >= ncpu() {
                return None;
            }
            match tail {
                "" => Node::Dir(entries(&[("online", dir::DT_REG)])),
                "online" => Node::File(b"1\n".to_vec()),
                _ => return None,
            }
        }
    })
}

fn sys_node(tail: &str) -> Option<Node> {
    Some(match tail {
        "" => Node::Dir(entries(&[("kernel", dir::DT_DIR)])),
        "kernel" => Node::Dir(entries(&[
            ("hostname", dir::DT_REG),
            ("osrelease", dir::DT_REG),
            ("ostype", dir::DT_REG),
            ("pid_max", dir::DT_REG),
            ("random", dir::DT_DIR),
        ])),
        "kernel/random" => Node::Dir(entries(&[("boot_id", dir::DT_REG), ("uuid", dir::DT_REG)])),
        "kernel/random/boot_id" => {
            let s = sysctl_string("kern.bootsessionuuid").to_lowercase();
            Node::File(format!("{s}\n").into_bytes())
        }
        "kernel/random/uuid" => Node::File(random_uuid().into_bytes()),
        "kernel/pid_max" => Node::File(b"99999\n".to_vec()),
        "kernel/ostype" => Node::File(b"Linux\n".to_vec()),
        "kernel/osrelease" => Node::File(b"6.6.0-darwin\n".to_vec()),
        "kernel/hostname" => {
            Node::File(format!("{}\n", sysctl_string("kern.hostname")).into_bytes())
        }
        _ => return None,
    })
}

fn is_kernfs(guest: &str) -> bool {
    guest == "/proc" || guest.starts_with("/proc/") || guest == "/sys" || guest.starts_with("/sys/")
}

/// The host path init recorded a value at, if it exists.
fn recorded(guest: &str) -> Option<(PathBuf, libc::stat)> {
    let (host, area) = vfs::lookup(guest);
    if area != Area::Kernfs {
        return None;
    }
    let c = CString::new(host.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    (unsafe { libc::stat(c.as_ptr(), &mut st) } == 0).then_some((host, st))
}

/// A file holding `data`, positioned at 0.
pub(super) fn content_fd(data: &[u8], cloexec: bool) -> i64 {
    let mut tmpl = std::env::temp_dir()
        .join("linux-abi-proc.XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    tmpl.push(0);
    // SAFETY: mkstemp fills the template; the name is removed at once.
    unsafe {
        let fd = libc::mkstemp(tmpl.as_mut_ptr().cast());
        if fd < 0 {
            return -(errno::last() as i64);
        }
        libc::unlink(tmpl.as_ptr().cast());
        let mut off = 0;
        while off < data.len() {
            let n = libc::write(fd, data[off..].as_ptr().cast(), data.len() - off);
            if n <= 0 {
                break;
            }
            off += n as usize;
        }
        libc::lseek(fd, 0, libc::SEEK_SET);
        // Read-only, as procfs files mostly are.
        fdtab::set_flags(fd, false, cloexec);
        fd as i64
    }
}

/// A directory fd for a synthesized directory.
fn dir_fd(guest: &str, list: Vec<Entry>, cloexec: bool) -> i64 {
    let mut list = list;
    if let Some((host, _)) = recorded(guest)
        && let Ok(rd) = std::fs::read_dir(&host)
    {
        for e in rd.flatten() {
            let name = e.file_name().as_encoded_bytes().to_vec();
            if !list.iter().any(|x| x.name == name) {
                let t = if e.file_type().is_ok_and(|t| t.is_dir()) {
                    dir::DT_DIR
                } else {
                    dir::DT_REG
                };
                list.push(Entry::new(1, t, name));
            }
        }
    }
    // A real directory backs the fd so fstat and fchdir behave.
    let c = CString::new(vfs::root().as_os_str().as_encoded_bytes()).unwrap_or_default();
    // SAFETY: opening the root directory read-only.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    fdtab::set_flags(fd, false, cloexec);
    let mut all = vec![
        Entry::new(1, dir::DT_DIR, "."),
        Entry::new(1, dir::DT_DIR, ".."),
    ];
    all.extend(list);
    fdtab::insert(
        fd,
        Kind::Dir(Arc::new(Mutex::new(DirStream::synthesized(
            guest.to_string(),
            all,
        )))),
    );
    fd as i64
}

/// Canonical form: `self` and `thread-self` become the pid path.
/// `/proc/self/...` below the link itself: `/proc/self` alone stays a link.
fn canonical(guest: &str) -> String {
    let me = pid();
    if let Some(t) = guest.strip_prefix("/proc/self/") {
        return format!("/proc/{me}/{t}");
    }
    if let Some(t) = guest.strip_prefix("/proc/thread-self/") {
        return format!("/proc/{me}/task/{}/{t}", super::process::gettid());
    }
    guest.to_string()
}

/// The thread of this process whose `comm` a canonical path names.
fn comm_tid(canon: &str) -> Option<i32> {
    let rest = canon.strip_prefix("/proc/")?.strip_suffix("/comm")?;
    let (p, tid) = match rest.split_once("/task/") {
        Some((p, t)) => (p.parse().ok()?, t.parse().ok()?),
        None => {
            let t: i32 = rest.parse().ok()?;
            (super::thread::owner(t), t)
        }
    };
    (p == pid() && super::thread::find(tid).is_some()).then_some(tid)
}

const O_ACCMODE: u64 = 3;
const O_CREAT: u64 = 0o100;
const O_CLOEXEC: u64 = 0o2000000;

/// openat of a guest path under /proc or /sys. None: not ours.
pub fn open(guest: &str, flags: u64, host_flags: i32) -> Option<i64> {
    if !is_kernfs(guest) {
        return None;
    }
    let cloexec = flags & O_CLOEXEC != 0;
    let write = flags & O_ACCMODE != 0;
    if let Some((host, st)) = recorded(guest)
        && st.st_mode & libc::S_IFMT != libc::S_IFDIR
    {
        let c = CString::new(host.as_os_str().as_encoded_bytes()).ok()?;
        // SAFETY: opening the recorded value file.
        return Some(errno::check(
            unsafe { libc::open(c.as_ptr(), host_flags, 0o644) } as i64,
        ));
    }
    let canon = canonical(guest);
    if canon.ends_with("/tracing/trace_marker") && node(&canon).is_some() {
        // Trace events are not collected: writes are discarded.
        // SAFETY: opening the host's null device.
        return Some(errno::check(
            unsafe { libc::open(c"/dev/null".as_ptr(), host_flags) } as i64,
        ));
    }
    if let Some(tid) = comm_tid(&canon) {
        let name = thread_comm(tid).unwrap_or_else(|| comm(pid()));
        return Some(super::knob::open(
            format!("{name}\n").as_bytes(),
            cloexec,
            move |b| {
                // Linux takes up to 15 bytes as they are, newline included.
                let mut n = [0u8; 16];
                let len = b
                    .iter()
                    .take(15)
                    .position(|&c| c == 0)
                    .unwrap_or(b.len().min(15));
                n[..len].copy_from_slice(&b[..len]);
                super::thread::set_name_of(tid, n);
                None
            },
        ));
    }
    // /proc/<pid>/fd/N reopens the file behind fd N.
    if let Some(n) = canon
        .strip_prefix(&format!("/proc/{}/fd/", pid()))
        .and_then(|n| n.parse::<i32>().ok())
    {
        if fdtab::is_hidden(n) {
            return Some(-(ENOENT as i64));
        }
        let mut path = [0u8; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH into a local buffer; then open or dup.
        return Some(unsafe {
            if libc::fcntl(n, libc::F_GETPATH, path.as_mut_ptr()) == 0 {
                errno::check(
                    libc::open(path.as_ptr().cast(), host_flags & !libc::O_CREAT, 0) as i64,
                )
            } else {
                let r = libc::fcntl(
                    n,
                    if cloexec {
                        libc::F_DUPFD_CLOEXEC
                    } else {
                        libc::F_DUPFD
                    },
                    0,
                );
                if r >= 0 {
                    fdtab::on_dup(n, r);
                }
                errno::check(r as i64)
            }
        });
    }
    Some(match node(&canon) {
        Some(Node::File(data)) => {
            if write {
                // Kernel knobs the layer does not emulate: record the value
                // under kernfs like init's writes, or refuse.
                return Some(record_write(guest, host_flags));
            }
            content_fd(&data, cloexec)
        }
        Some(Node::Dir(list)) => dir_fd(&canon, list, cloexec),
        Some(Node::Link(t)) => {
            let target = if t.starts_with('/') {
                t
            } else {
                format!("/proc/{t}")
            };
            let linked = node(&canonical(&target));
            if let Some(Node::Dir(list)) = linked {
                dir_fd(&canonical(&target), list, cloexec)
            } else if let Some(Node::File(data)) = linked.filter(|_| !write) {
                // /proc/mounts -> self/mounts.
                content_fd(&data, cloexec)
            } else {
                let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, target.as_bytes(), true) {
                    Ok(r) => r,
                    Err(e) => return Some(-(e as i64)),
                };
                if r.guest == super::process::exe_guest_path() {
                    let c = CString::new(super::process::exe_host_path()).ok()?;
                    // SAFETY: opening the program file.
                    return Some(errno::check(
                        unsafe { libc::open(c.as_ptr(), host_flags) } as i64
                    ));
                }
                // SAFETY: opening the link target.
                errno::check(unsafe { libc::open(r.host.as_ptr(), host_flags, 0) } as i64)
            }
        }
        None if flags & O_CREAT != 0 || write => record_write(guest, host_flags),
        None => -(ENOENT as i64),
    })
}

/// A guest write to a /proc or /sys value: kept under kernfs when there is
/// a path map, else refused.
fn record_write(guest: &str, host_flags: i32) -> i64 {
    let (host, area) = vfs::lookup(guest);
    if area != Area::Kernfs {
        return -(EACCES as i64);
    }
    if let Some(p) = host.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let Ok(c) = CString::new(host.as_os_str().as_encoded_bytes()) else {
        return -(ENOENT as i64);
    };
    // SAFETY: creating or opening the value file.
    errno::check(unsafe { libc::open(c.as_ptr(), host_flags | libc::O_CREAT, 0o644) } as i64)
}

/// stat of a guest path under /proc or /sys (links followed when
/// `follow`). None: not ours.
pub fn stat(guest: &str, follow: bool) -> Option<Result<libc::stat, Errno>> {
    if !is_kernfs(guest) {
        return None;
    }
    if let Some((_, st)) = recorded(guest) {
        return Some(Ok(st));
    }
    let canon = canonical(guest);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let me = pid();
    let owner = if canon.starts_with(&format!("/proc/{me}")) {
        super::attrs::ids(super::attrs::EFFECTIVE)
    } else {
        (0, 0)
    };
    st.st_uid = owner.0;
    st.st_gid = owner.1;
    st.st_blksize = 1024;
    st.st_ino = canon.bytes().fold(1469598103934665603u64, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    });
    let now = now() as i64;
    st.st_atime = now;
    st.st_mtime = now;
    st.st_ctime = now;
    match node(&canon) {
        Some(Node::File(_)) => {
            st.st_mode = libc::S_IFREG | 0o444;
            st.st_nlink = 1;
        }
        Some(Node::Dir(_)) => {
            st.st_mode = libc::S_IFDIR | 0o555;
            st.st_nlink = 2;
        }
        Some(Node::Link(t)) if follow => {
            let fd_n = canon
                .strip_prefix(&format!("/proc/{me}/fd/"))
                .and_then(|n| n.parse::<i32>().ok());
            if let Some(n) = fd_n {
                // SAFETY: fstat of the fd the link names.
                if unsafe { libc::fstat(n, &mut st) } < 0 {
                    return Some(Err(errno::last()));
                }
                return Some(Ok(st));
            }
            let target = if t.starts_with('/') {
                t
            } else {
                format!("/proc/{t}")
            };
            if let Some(r) = stat(&target, true) {
                return Some(r);
            }
            let path = if target == super::process::exe_guest_path() {
                CString::new(super::process::exe_host_path()).ok()?
            } else {
                match vfs::resolve(vfs::LINUX_AT_FDCWD, target.as_bytes(), true) {
                    Ok(r) => r.host,
                    Err(e) => return Some(Err(e)),
                }
            };
            // SAFETY: host path, local buffer.
            if unsafe { libc::stat(path.as_ptr(), &mut st) } < 0 {
                return Some(Err(errno::last()));
            }
        }
        Some(Node::Link(t)) => {
            st.st_mode = libc::S_IFLNK | 0o777;
            st.st_nlink = 1;
            st.st_size = t.len() as i64;
        }
        None => return Some(Err(ENOENT)),
    }
    Some(Ok(st))
}

/// Target of a `/proc` link, or None when `path` is not one we synthesize.
pub fn readlink(path: &[u8]) -> Option<Result<Vec<u8>, Errno>> {
    let guest = std::str::from_utf8(path).ok()?;
    if !guest.starts_with("/proc/") {
        return None;
    }
    let canon = canonical(guest);
    Some(match node(&canon) {
        Some(Node::Link(t)) => Ok(t.into_bytes()),
        Some(_) => Err(crate::errno::EINVAL),
        None => Err(ENOENT),
    })
}

/// `attr/current`: the identity's context, NUL-terminated.
fn attr_current() -> Vec<u8> {
    let mut v = super::cred::seclabel().into_bytes();
    v.push(0);
    v
}

/// Whether `path` names `/proc/self/exe`.
pub fn is_self_exe(path: &[u8]) -> bool {
    path == b"/proc/self/exe" || path == format!("/proc/{}/exe", pid()).as_bytes()
}
