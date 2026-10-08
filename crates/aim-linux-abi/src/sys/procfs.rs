//! `/proc` and `/sys`, synthesized from Darwin state (after FreeBSD's
//! linprocfs/linsysfs, BSD-2-Clause).
//!
//! - Files are generated when opened and handed to the guest as an unlinked
//!   temporary file, so reads, seeks and fstat behave like a regular file.
//! - Directories are directory streams with fixed entries (see `dir`).
//! - Values init wrote under `<runtime>/kernfs/...`
//!   (`docs/guest-init-contract.md` section 7) take precedence over the
//!   synthesized ones and may be rewritten by the guest, but for sysfs's
//!   device trees, which hold only the modeled devices (`device_tree`).
//! - `/proc/<pid>` for another process is read from Darwin's process info,
//!   the process's record (`procrec`: threads, names, tracers, command
//!   line) and its agent (`ptrace`: memory maps and fds).

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use super::dir::{self, DirStream, Entry};
use super::fdtab::{self, Kind};
use super::vmmap;
use crate::errno::{self, EACCES, ENOENT, Errno};
use crate::vfs::{self, Area};

/// What a synthesized path is.
pub enum Node {
    File(Vec<u8>),
    NetTable(String),
    MountTable {host_pid:i32,info:bool},
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

/// The current program's; replaced when the process execs in place.
static STACK: RwLock<Option<StackInfo>> = RwLock::new(None);

/// Note the program's stack and make the process's record of it
/// (`procrec`).
pub fn note_stack(s: StackInfo) {
    *STACK.write().unwrap_or_else(|e| e.into_inner()) = Some(s);
    super::procrec::init(&s);
}

fn stack() -> Option<StackInfo> {
    *STACK.read().unwrap_or_else(|e| e.into_inner())
}

/// Fork: the main thread's stack areas (the child's memory has them at
/// the same addresses).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    // The pool stays with this process: its hidden fds go before the
    // child inherits the fd table.
    POOL.drain();
    w.opt(stack(), |w, s| {
        for v in [
            s.lo,
            s.hi,
            s.start_stack,
            s.args.0,
            s.args.1,
            s.env.0,
            s.env.1,
        ] {
            w.u64(v);
        }
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    if let Some(v) = r.opt(|r| [(); 7].map(|_| r.u64())) {
        note_stack(StackInfo {
            lo: v[0],
            hi: v[1],
            start_stack: v[2],
            args: (v[3], v[4]),
            env: (v[5], v[6]),
        });
    }
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
pub(super) fn fd_link(fd: i32) -> Option<String> {
    if fdtab::is_hidden(fd) {
        return None;
    }
    if let Some((inode, _)) = super::net::proc_socket_identity(fd) {
        return Some(format!("socket:[{inode}]"));
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
    if let Some(n) = super::memfd::fd_link(fd, &st) {
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
    let path = fd_guest_path(fd).ok()?;
    super::tmpfile::fd_link(&path, &st).or(Some(path))
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
    guest_argv(argv)
}

/// A `linux-run` command line's guest program and arguments, after its
/// own options (`src/bin/linux-run.rs`); any other argv as it is.
fn guest_argv(argv: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    if !argv.first().is_some_and(|a| a.ends_with(b"linux-run")) {
        return argv;
    }
    let mut i = 1;
    while i < argv.len() && argv[i].starts_with(b"--") {
        let takes_value = !matches!(
            &argv[i][..],
            b"--trace" | b"--inherit-env" | b"--stdio-null" | b"--no-cache"
        );
        i += if takes_value { 2 } else { 1 };
    }
    argv[i.min(argv.len())..].to_vec()
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

/// A command line as Linux's `get_mm_cmdline` reads it from the memory
/// `read` reads: the argument area `args`; or, when its last byte is no
/// longer NUL (setproctitle(3) wrote past argv, into the environment area
/// `env` if that follows), the title from the start up to its first NUL,
/// within the two areas and a page.
pub(super) fn linux_cmdline(
    args: (u64, u64),
    env: (u64, u64),
    read: impl Fn(u64, u64) -> Vec<u8>,
) -> Vec<u8> {
    let (lo, hi) = args;
    if hi <= lo || read(hi - 1, hi).first().is_none_or(|&c| c == 0) {
        return read(lo, hi);
    }
    let end = if env.0 == hi && env.1 >= env.0 {
        env.1
    } else {
        hi
    };
    let mut title = read(lo, end.min(lo + super::mem::PAGE));
    if let Some(n) = title.iter().position(|&c| c == 0) {
        title.truncate(n + 1);
    }
    title
}

fn cmdline(p: i32) -> Vec<u8> {
    if p == pid()
        && let Some(s) = stack()
    {
        return linux_cmdline(s.args, s.env, read_guest);
    }
    if let Some(c) = super::procrec::cmdline(p) {
        return c;
    }
    let mut out = Vec::new();
    for a in host_argv(p) {
        out.extend_from_slice(&a);
        out.push(0);
    }
    out
}

/// The `comm` of process `p`: its main thread's.
fn comm(p: i32) -> String {
    if let Some(n) = thread_comm(p, p) {
        return n;
    }
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

/// When the host booted on the wall clock (CLOCK_REALTIME minus
/// CLOCK_BOOTTIME), as (seconds, microseconds).
fn boot_time() -> (i64, i64) {
    use super::clock::Base;
    let ns = Base::Realtime.now() - Base::Boottime.now();
    (
        (ns / 1_000_000_000) as i64,
        (ns % 1_000_000_000 / 1000) as i64,
    )
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

/// `PROC_PIDTHREADID64INFO` (sys/proc_info.h): a thread by its host id.
const PROC_PIDTHREADID64INFO: i32 = 15;

/// User and system CPU time of thread `tid` of process `p`, in
/// nanoseconds, and its state, from its host thread.
fn thread_times(p: i32, tid: i32) -> Option<(u64, u64, char)> {
    let host = if p == pid() {
        super::thread::host_thread_id(tid)?
    } else {
        super::procrec::threads(p)?
            .into_iter()
            .find(|t| t.tid == tid)?
            .host
    };
    let mut ti: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_threadinfo>() as i32;
    // SAFETY: proc_pidinfo writes at most `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            p,
            PROC_PIDTHREADID64INFO,
            host,
            (&mut ti as *mut libc::proc_threadinfo).cast(),
            size,
        )
    };
    if n != size {
        return Some((0, 0, 'S'));
    }
    // TH_STATE_* (mach/thread_info.h).
    let state = match ti.pth_run_state {
        1 => 'R',
        2 => 'T',
        4 => 'D',
        _ => 'S',
    };
    Some((ti.pth_user_time, ti.pth_system_time, state))
}

/// `/proc/<p>/stat`, or with `thread` its thread's
/// `/proc/<p>/task/<tid>/stat`: the thread's id, `comm`, state and CPU
/// times.
fn stat_line(p: i32, thread: Option<i32>) -> Option<String> {
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
        stack().unwrap_or_default()
    } else {
        StackInfo::default()
    };
    let rss_pages = ti.pti_resident_size / super::mem::PAGE;
    let (id, name, user, system, state) = match thread {
        Some(tid) => {
            let (user, system, state) = thread_times(p, tid).unwrap_or((0, 0, state));
            let name = thread_comm(p, tid).unwrap_or_else(|| comm(p));
            (tid, name, user, system, state)
        }
        None => (p, comm(p), ti.pti_total_user, ti.pti_total_system, state),
    };
    let id=super::pidns::guest_pid(id).ok()?;
    Some(format!(
        "{id} ({name}) {state} {} {} {} 0 -1 4194560 {} 0 {} 0 {} {} 0 0 20 {} {} 0 {start} {} {rss_pages} 18446744073709551615 0 0 {} 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 {} {} {} {} 0\n",
        super::process::linux_ppid(b.pbi_ppid as i32),
        super::pidns::id_in_ns(b.pbi_pgid as i32),
        super::pidns::id_in_ns(unsafe{libc::getsid(p)}),
        ti.pti_faults,
        ti.pti_pageins,
        ns_to_ticks(user),
        ns_to_ticks(system),
        b.pbi_nice,
        if p == pid() {
            tids(p).len() as i32
        } else {
            ti.pti_threadnum
        },
        ti.pti_virtual_size,
        s.start_stack,
        s.args.0,
        s.args.1,
        s.env.0,
        s.env.1,
    ))
}

/// The credential lines of `/proc/<p>/status`, from the process's
/// identity (another's from the process table).
fn cred_lines(p: i32) -> String {
    super::cred::proc_status(p)
}

/// `/proc/<p>/status`, or with `thread` its thread's: the thread's `comm`
/// and id.
fn status(p: i32, thread: Option<i32>) -> Option<String> {
    let t = task_info(p)?;
    let state = match t.pbsd.pbi_status {
        2 => "R (running)",
        4 => "T (stopped)",
        5 => "Z (zombie)",
        _ => "S (sleeping)",
    };
    let kb = |b: u64| b / 1024;
    let n = ncpu();
    let guest_tgid=super::pidns::guest_pid(p).ok()?;
    Some(format!(
        "Name:\t{}\nUmask:\t0022\nState:\t{state}\nTgid:\t{guest_tgid}\nNgid:\t0\nPid:\t{}\nPPid:\t{}\nTracerPid:\t{}\n\
         {}FDSize:\t256\n\
         VmPeak:\t{} kB\nVmSize:\t{} kB\nVmLck:\t0 kB\nVmPin:\t0 kB\nVmHWM:\t{} kB\nVmRSS:\t{} kB\n\
         RssAnon:\t{} kB\nRssFile:\t0 kB\nRssShmem:\t0 kB\nVmData:\t{} kB\nVmStk:\t8192 kB\nVmExe:\t0 kB\n\
         VmLib:\t0 kB\nVmPTE:\t0 kB\nVmSwap:\t0 kB\nThreads:\t{}\nSigQ:\t0/0\nSigPnd:\t0000000000000000\n\
         ShdPnd:\t0000000000000000\nSigBlk:\t0000000000000000\nSigIgn:\t0000000000000000\n\
         SigCgt:\t0000000000000000\nNoNewPrivs:\t0\nSeccomp:\t0\n\
         Cpus_allowed:\t{:x}\nCpus_allowed_list:\t0-{}\nvoluntary_ctxt_switches:\t{}\nnonvoluntary_ctxt_switches:\t0\n",
        thread
            .and_then(|tid| thread_comm(p, tid))
            .unwrap_or_else(|| comm(p)),
        super::pidns::guest_pid(thread.unwrap_or(p)).ok()?,
        super::process::linux_ppid(t.pbsd.pbi_ppid as i32),
        super::pidns::guest_pid(tracer(p,thread.unwrap_or(p))).ok()?,
        cred_lines(p),
        kb(t.ptinfo.pti_virtual_size),
        kb(t.ptinfo.pti_virtual_size),
        kb(t.ptinfo.pti_resident_size),
        kb(t.ptinfo.pti_resident_size),
        kb(t.ptinfo.pti_resident_size),
        kb(t.ptinfo.pti_virtual_size),
        if p == pid() {
            tids(p).len() as i32
        } else {
            t.ptinfo.pti_threadnum
        },
        (1u64 << n) - 1,
        n - 1,
        t.ptinfo.pti_csw,
    ))
}

/// The tracer of thread `tid` of process `p`, or 0.
fn tracer(p: i32, tid: i32) -> i32 {
    if p == pid() {
        return super::ptrace::tracer_of(tid);
    }
    super::procrec::threads(p)
        .and_then(|v| v.into_iter().find(|t| t.tid == tid))
        .map_or(0, |t| t.tracer)
}

/// In pages of the guest's page size (`AT_PAGESZ`), as `rss` in `stat`.
fn statm(p: i32) -> Option<String> {
    let t = task_info(p)?;
    let pg = |b: u64| b / super::mem::PAGE;
    Some(format!(
        "{} {} 0 0 0 {} 0\n",
        pg(t.ptinfo.pti_virtual_size),
        pg(t.ptinfo.pti_resident_size),
        pg(t.ptinfo.pti_resident_size)
    ))
}

/// Linux-format `/proc/self/maps` from the VM map.
pub(super) fn maps() -> String {
    let paths = vfs::HostPathView::capture();
    let stack = stack().unwrap_or_default();
    let mut out = String::new();
    let mut regions = vmmap::regions(0, u64::MAX).peekable();
    while let Some(mut r) = regions.next() {
        // The string pages are the top of the stack, mapped from the
        // process record (`procrec`): one `[stack]` with the rest.
        if super::procrec::is_strings(r.start) {
            r.shared = false;
            r.file = None;
        }
        while let Some(n) = regions.next_if(|n| {
            n.start == r.end && n.prot == r.prot && super::procrec::is_strings(n.start)
        }) {
            r.end = n.end;
        }
        let prot = r.prot;
        let perms = format!(
            "{}{}{}{}",
            if prot & 1 != 0 { 'r' } else { '-' },
            if prot & 2 != 0 { 'w' } else { '-' },
            if prot & 4 != 0 { 'x' } else { '-' },
            if r.shared { 's' } else { 'p' }
        );
        let (mut offset, mut dev, mut ino, mut name) = (0u64, 0u64, 0u64, String::new());
        // A host file outside the guest's view (linux-run, dyld, host
        // libraries, the process record) reads as anonymous memory: its
        // path is the Mac's, not a file of the guest.
        let guest_file = r.file.as_ref().and_then(|(path, d, i)| {
            super::memfd::link_name(path, *d, *i)
                .or_else(|| crate::xrt::original_guest_path_of_host(path))
                .or_else(|| paths.as_ref().and_then(|view| view.guest_path(path)))
                .map(|n| (n, *d, *i))
        });
        if let Some((n, d, i)) = guest_file {
            // A fork child's copy of a private file mapping reports
            // offset 0 (`copies`).
            offset = super::copies::find(r.start).map_or(r.offset, |(_, off)| off);
            dev = d;
            ino = i;
            name = n;
        } else if let Some((c, off)) = super::copies::find(r.start) {
            offset = off;
            dev = c.dev;
            ino = c.ino;
            name = c.guest;
        } else if r.start < stack.hi && stack.lo < r.end && prot != 0 {
            name = "[stack]".into();
        } else if let Some(n) = crate::vdso::name_of(r.start) {
            name = n.into();
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
    // external_page_count (file-backed) is word 34 of vm_statistics64.
    let cached = pages(34);
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

/// `/proc/config.gz`: the configuration of the kernel the layer plays, as
/// libvintf's `RuntimeInfo` reads it (system_server's `Debug.isVmapStack`
/// aborts when it cannot). Options the layer does not provide are absent.
const KERNEL_CONFIG: &str = "\
# Linux/arm64 6.6.0 Kernel Configuration (aim-linux-abi)
CONFIG_ARM64=y
CONFIG_64BIT=y
CONFIG_MMU=y
CONFIG_SMP=y
CONFIG_ARM64_4K_PAGES=y
CONFIG_ANDROID_BINDER_IPC=y
CONFIG_ASHMEM=y
CONFIG_MEMFD_CREATE=y
CONFIG_BPF_SYSCALL=y
CONFIG_CGROUPS=y
CONFIG_INOTIFY_USER=y
CONFIG_EPOLL=y
CONFIG_EVENTFD=y
CONFIG_SIGNALFD=y
CONFIG_TIMERFD=y
CONFIG_FUTEX=y
CONFIG_POSIX_TIMERS=y
CONFIG_INPUT_EVDEV=y
CONFIG_UNIX=y
CONFIG_INET=y
CONFIG_IPV6=y
CONFIG_SECCOMP=y
CONFIG_SECCOMP_FILTER=y
CONFIG_SECURITY_SELINUX=y
CONFIG_VMAP_STACK=y
";

/// `data` as a gzip file of stored (uncompressed) deflate blocks.
fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    let mut chunks = data.chunks(0xffff).peekable();
    if chunks.peek().is_none() {
        out.extend([1, 0, 0, 0xff, 0xff]);
    }
    while let Some(c) = chunks.next() {
        let len = c.len() as u16;
        out.push(chunks.peek().is_none() as u8);
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(c);
    }
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    out.extend((!crc).to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
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

/// The processes `/proc` lists: the guest's pid namespace (`pidns`), not
/// the Mac's; every host process without one.
fn pids() -> Vec<i32> {
    let mut v = match super::pidns::members() {
        Some(mut v) => {
            v.retain(|&p| task_info(p).is_some());
            v
        }
        // SAFETY: sizing call then a buffer of that size.
        None => unsafe {
            let n = libc::proc_listallpids(std::ptr::null_mut(), 0);
            let mut v = vec![0i32; n.max(0) as usize + 64];
            let n = libc::proc_listallpids(v.as_mut_ptr().cast(), (v.len() * 4) as i32);
            v.truncate(n.max(0) as usize);
            v
        },
    };
    v = v.into_iter().filter_map(|host|super::pidns::guest_pid(host).ok()).collect();
    v.retain(|&p| p > 0);
    v.sort_unstable();
    v.dedup();
    v
}

/// The tids of process `p`: its guest threads, another process's from its
/// record.
fn tids(p: i32) -> Vec<i32> {
    let t = if p == pid() {
        let mut t = super::thread::tids();
        t.extend(super::procrec::foreign_tids());
        t
    } else {
        super::procrec::threads(p).map_or(Vec::new(), |v| v.into_iter().map(|t| t.tid).collect())
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
    ("net", dir::DT_DIR),
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
fn mount_table(points: Vec<vfs::MountPoint>) -> Vec<(String, String, String, &'static str)> {
    let mut out = vec![(
        "/dev/root".into(),
        "/".into(),
        "erofs".into(),
        "ro,relatime",
    )];
    for mp in points {
        let guest = mp.guest;
        // A kernel filesystem of the path map (cgroup2, bpf) or a mount the
        // process made.
        if let (Some(source), Some(fstype)) = (mp.source, mp.fstype) {
            let options = if mp.area == vfs::Area::Image {
                "ro,relatime"
            } else {
                "rw,nosuid,nodev,noexec,relatime"
            };
            out.push((source, guest, fstype, options));
            continue;
        }
        let (source, fstype, options) = match (mp.area, guest.as_str()) {
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
        out.push((source, guest, fstype.to_string(), options));
    }
    out
}

/// `/proc/<pid>/mounts`, as fstab(5) lines.
fn mounts(points:Vec<vfs::MountPoint>) -> String {
    mount_table(points)
        .into_iter()
        .map(|(source, target, fstype, options)| {
            format!("{source} {target} {fstype} {options} 0 0\n")
        })
        .collect()
}

/// `/proc/<pid>/mountinfo` (proc(5)): `/` is mount 1 and the parent of the
/// others.
fn mountinfo(points:Vec<vfs::MountPoint>) -> String {
    mount_table(points)
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

fn process_mount_points(host_pid:i32)->Result<Vec<vfs::MountPoint>,Errno>{
    if host_pid==pid(){vfs::refresh_fuse_mounts()?;return Ok(vfs::mount_points());}
    let table=super::cred::by_pid_dir().ok_or(errno::ESRCH)?;
    let process=aim_storage::process_namespace::ProcessIdentity::running(host_pid).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    let id=match aim_storage::process_namespace::InitRegistration::read(table){
        Ok(init)if init.process==process=>init.mount_namespace,
        Ok(_)=>aim_storage::process_namespace::mount_namespace_of(table,process).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?,
        Err(error)if error.kind()==std::io::ErrorKind::NotFound=>aim_storage::process_namespace::mount_namespace_of(table,process).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?,
        Err(error)=>return Err(errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))),
    };
    vfs::namespace_mount_points(&id)
}

/// The `comm` of thread `tid` of process `p`, once it has been named.
fn thread_comm(p: i32, tid: i32) -> Option<String> {
    let n = if p == pid() {
        super::thread::name_of(tid)?
    } else {
        super::procrec::threads(p)?
            .into_iter()
            .find(|t| t.tid == tid)?
            .name
    };
    let len = n.iter().position(|&b| b == 0).unwrap_or(16);
    (len > 0).then(|| String::from_utf8_lossy(&n[..len]).into_owned())
}

const NET_FILES: [&str; 4] = ["tcp", "tcp6", "udp", "udp6"];

/// Nodes under `/proc/<p>/` (`rest` is the path after it); `thread` is the
/// tid for `/proc/<p>/task/<tid>/` (and `/proc/<tid>/`).
fn pid_node(p: i32, rest: &str, thread: Option<i32>) -> Option<Node> {
    let me = p == pid();
    if !me && (!super::pidns::contains(p) || task_info(p).is_none()) {
        return None;
    }
    Some(match rest {
        "" => {
            let e = PID_ENTRIES
                .iter()
                .filter(|(n, _)| thread.is_none() || *n != "task");
            Node::Dir(e.map(|(n, t)| Entry::new(1, *t, n.as_bytes())).collect())
        }
        "net" => Node::Dir(NET_FILES.iter().map(|name| Entry::new(1, dir::DT_REG, name.as_bytes())).collect()),
        name if name.starts_with("net/") => {
            let kind = name.strip_prefix("net/")?;
            if !NET_FILES.contains(&kind) { return None; }
            Node::NetTable(kind.to_owned())
        }
        "cmdline" => Node::File(cmdline(p)),
        "comm" => {
            let named = thread.and_then(|t| thread_comm(p, t));
            Node::File(format!("{}\n", named.unwrap_or_else(|| comm(p))).into_bytes())
        }
        "environ" if me => Node::File(stack().map_or(Vec::new(), |s| read_guest(s.env.0, s.env.1))),
        "stat" => Node::File(stat_line(p, thread)?.into_bytes()),
        "status" => Node::File(status(p, thread)?.into_bytes()),
        "statm" => Node::File(statm(p)?.into_bytes()),
        "maps" if me => Node::File(maps().into_bytes()),
        "maps" => Node::File(super::ptrace::remote_maps(p)?.into_bytes()),
        "mounts" => Node::MountTable {host_pid:p,info:false},
        "mountinfo" => Node::MountTable {host_pid:p,info:true},
        "oom_score_adj" if me => {
            Node::File(format!("{}\n", super::cred::oom_score_adj()).into_bytes())
        }
        "oom_score_adj" => Node::File(b"0\n".to_vec()),
        "attr" => Node::Dir(entries(&[("current", dir::DT_REG)])),
        "attr/current" => Node::File(attr_current(p)),
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
        "fd" => Node::Dir(
            super::ptrace::remote_fds(p)?
                .into_iter()
                .map(|fd| Entry::new(fd as u64 + 1, dir::DT_LNK, fd.to_string()))
                .collect(),
        ),
        "task" if thread.is_none() => Node::Dir(
            tids(p)
                .into_iter()
                .filter_map(|t|super::pidns::guest_pid(t).ok())
                .map(|t| Entry::new(t as u64, dir::DT_DIR, t.to_string()))
                .collect(),
        ),
        _ => {
            if let Some(n) = rest.strip_prefix("fd/") {
                let fd: i32 = n.parse().ok()?;
                return if me {
                    fd_link(fd)
                } else {
                    super::ptrace::remote_fd_link(p, fd)
                }
                .map(Node::Link);
            }
            let t = rest.strip_prefix("task/").filter(|_| thread.is_none())?;
            let (tid, sub) = t.split_once('/').unwrap_or((t, ""));
            let tid: i32 = tid.parse().ok()?;
            let tid=super::pidns::syscall_pid(tid).ok()?;
            if !tids(p).contains(&tid) {
                return None;
            }
            return pid_node(p, sub, Some(tid));
        }
    })
}

/// The synthesized node at a normalized guest path under /proc or /sys.
pub fn node(guest: &str) -> Option<Node> {
    if let Some(node)=super::fuse_sysfs::node(guest){return node.ok();}
    if let Some(rest) = guest.strip_prefix("/proc") {
        let rest = rest.trim_start_matches('/');
        let (first, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let me = pid();
        return Some(match first {
            "" => {
                let mut e = entries(&[
                    ("self", dir::DT_LNK),
                    ("thread-self", dir::DT_LNK),
                    ("config.gz", dir::DT_REG),
                    ("cpuinfo", dir::DT_REG),
                    ("filesystems", dir::DT_REG),
                    ("loadavg", dir::DT_REG),
                    ("meminfo", dir::DT_REG),
                    ("mounts", dir::DT_LNK),
                    ("net", dir::DT_LNK),
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
            "net" if tail.is_empty() => Node::Link("self/net".into()),
            "net" => return pid_node(me, &format!("net/{tail}"), None),
            "thread-self" if tail.is_empty() => {
                Node::Link(format!("{me}/task/{}", super::process::gettid()))
            }
            "self" => return pid_node(me, tail, None),
            "thread-self" => {
                return pid_node(me, tail, Some(super::process::gettid() as i32));
            }
            "config.gz" => Node::File(gzip_stored(KERNEL_CONFIG.as_bytes())),
            "cpuinfo" => Node::File(cpuinfo().into_bytes()),
            "meminfo" => Node::File(meminfo().into_bytes()),
            "stat" => Node::File(proc_stat().into_bytes()),
            "uptime" => Node::File(uptime().into_bytes()),
            "loadavg" => Node::File(loadavg().into_bytes()),
            "version" => {
                Node::File(b"Linux version 6.6.0-darwin (aim-linux-abi) #1 SMP PREEMPT\n".to_vec())
            }
            "filesystems" => {
                Node::File(b"nodev\tsysfs\nnodev\tproc\nnodev\ttmpfs\n\text4\n".to_vec())
            }
            "sys" => return sys_node(tail),
            n => {
                // `/proc/<tid>` of a thread other than a main thread: the
                // thread's view of its process, as on Linux, with the
                // process's tasks.
                let guest: i32 = n.parse().ok()?;
                let n=super::pidns::syscall_pid(guest).ok()?;
                let p = super::thread::owner(n);
                if p == n {
                    return pid_node(p, tail, None);
                }
                if !tids(p).contains(&n) {
                    return None;
                }
                if tail == "task" || tail.starts_with("task/") {
                    return pid_node(p, tail, None);
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
            ("class", dir::DT_DIR),
            ("dev", dir::DT_DIR),
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
        "devices" => Node::Dir(entries(&[
            ("system", dir::DT_DIR),
            ("virtual", dir::DT_DIR),
        ])),
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
            let Some(c) = rest.strip_prefix("devices/system/cpu/cpu") else {
                return super::evdev::sys_node(rest);
            };
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

/// Under /proc or /sys, and not in a writable area mapped there (the cgroup
/// v2 hierarchy).
fn is_kernfs(guest: &str) -> bool {
    (guest == "/proc"
        || guest.starts_with("/proc/")
        || guest == "/sys"
        || guest.starts_with("/sys/"))
        && vfs::lookup(guest).1 != Area::Writable
}

/// The host path init recorded a value at, if it exists.
fn recorded(guest: &str) -> Option<(PathBuf, libc::stat)> {
    let (host, area) = vfs::lookup(guest);
    if area != Area::Kernfs || device_tree(guest) {
        return None;
    }
    // A value written for a process or thread lasts as long as it does.
    if let Some(n) = proc_entry(&canonical(guest))
        && node(&format!("/proc/{n}")).is_none()
    {
        return None;
    }
    let c = CString::new(host.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    (unsafe { libc::stat(c.as_ptr(), &mut st) } == 0).then_some((host, st))
}

/// The pid or tid of the `/proc/<n>` directory a canonical path is in.
fn proc_entry(canon: &str) -> Option<i32> {
    canon
        .strip_prefix("/proc/")?
        .split('/')
        .next()?
        .parse()
        .ok()
}

/// Files of content fds the guest closed while nothing else referred to
/// them, kept as hidden fds for the next content fd: creating and removing
/// a file takes over 100 µs of kernel time on a Mac, rewriting one a few
/// (#446). A fork child does not get them ([`fork_save`]).
static POOL: Pool = Pool(Mutex::new(Vec::new()));
const POOL_MAX: usize = 8;

struct Pool(Mutex<Vec<i32>>);

/// A file holding `data`, positioned at 0. Closing it gives the file back
/// to the pool ([`recycle`]).
pub(super) fn content_fd(data: &[u8], cloexec: bool) -> i64 {
    let fd = match POOL.reuse(data, cloexec) {
        Some(fd) => fd,
        None => match fresh(data) {
            Ok(fd) => {
                fdtab::set_flags(fd, false, cloexec);
                fd
            }
            Err(e) => return e,
        },
    };
    fdtab::insert(fd, Kind::Content);
    fd as i64
}

/// A new unlinked file holding `data`.
fn fresh(data: &[u8]) -> Result<i32, i64> {
    let mut tmpl = std::env::temp_dir()
        .join("linux-abi-proc.XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    tmpl.push(0);
    // SAFETY: mkstemp fills the template; the name is removed at once.
    let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
    if fd < 0 {
        return Err(-(errno::last() as i64));
    }
    // SAFETY: the name mkstemp just made.
    unsafe { libc::unlink(tmpl.as_ptr().cast()) };
    fill(fd, data);
    Ok(fd)
}

/// Make the file of `fd` hold exactly `data`, positioned at 0.
fn fill(fd: i32, data: &[u8]) {
    let mut off = 0;
    // SAFETY: writing our own file from a local buffer.
    unsafe {
        while off < data.len() {
            let n = libc::pwrite(
                fd,
                data[off..].as_ptr().cast(),
                data.len() - off,
                off as i64,
            );
            if n <= 0 {
                break;
            }
            off += n as usize;
        }
        libc::ftruncate(fd, off as i64);
        libc::lseek(fd, 0, libc::SEEK_SET);
    }
}

impl Pool {
    /// A pooled file holding `data`, on the lowest free fd.
    fn reuse(&self, data: &[u8], cloexec: bool) -> Option<i32> {
        let h = self.0.lock().unwrap().pop()?;
        fill(h, data);
        let cmd = if cloexec {
            libc::F_DUPFD_CLOEXEC
        } else {
            libc::F_DUPFD
        };
        // SAFETY: plain fcntls on our hidden fd; its status flags (O_NONBLOCK,
        // O_APPEND) are the last user's, so they are cleared.
        let fd = unsafe {
            libc::fcntl(h, libc::F_SETFL, 0);
            libc::fcntl(h, cmd, 0)
        };
        fdtab::unhide(h);
        // SAFETY: our hidden fd; `fd`, if any, keeps the file open.
        unsafe { libc::close(h) };
        (fd >= 0).then_some(fd)
    }

    /// The guest is closing content fd `fd`: its file goes back to the pool
    /// unless another fd, a process, a message or a mapping still refers to it.
    fn recycle(&self, fd: i32) {
        let mut pool = self.0.lock().unwrap();
        if pool.len() >= POOL_MAX
            || shared(fd)
            || file_id(fd).is_none_or(|id| MAPPED.lock().unwrap().contains(&id))
        {
            return;
        }
        // SAFETY: duplicating the guest's fd before it closes it.
        let h = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, fdtab::hidden_base()) };
        if h >= 0 {
            fdtab::keep_hidden(h);
            pool.push(h);
        }
    }

    /// Close the pooled files.
    fn drain(&self) {
        for h in self.0.lock().unwrap().drain(..) {
            fdtab::unhide(h);
            // SAFETY: our hidden fd.
            unsafe { libc::close(h) };
        }
    }
}

/// Content files that were mapped, by (dev, ino): a mapping holds the file
/// without an fd (libselinux maps `/sys/fs/selinux/status`), so they never
/// go back to the pool.
static MAPPED: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

fn file_id(fd: i32) -> Option<(u64, u64)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some((st.st_dev as u64, st.st_ino))
}

/// `fd` is being mapped.
pub fn on_mmap(fd: i32) {
    if matches!(fdtab::get(fd), Some(Kind::Content | Kind::Knob(_)))
        && let Some(id) = file_id(fd)
    {
        MAPPED.lock().unwrap().push(id);
    }
}

/// The guest is closing content fd `fd` ([`Pool::recycle`]).
pub fn recycle(fd: i32) {
    POOL.recycle(fd);
}

/// Whether the open file of `fd` has other references than `fd`
/// (`PROC_FP_SHARED`); true when that cannot be told.
fn shared(fd: i32) -> bool {
    const PROC_PIDFDVNODEINFO: i32 = 1;
    const PROC_FP_SHARED: u32 = 1;
    // struct vnode_fdinfo (176 bytes); fi_status is its second word.
    let mut info = [0u32; 44];
    // SAFETY: a buffer of the flavour's size.
    let n = unsafe {
        libc::proc_pidfdinfo(
            libc::getpid(),
            fd,
            PROC_PIDFDVNODEINFO,
            info.as_mut_ptr().cast(),
            std::mem::size_of_val(&info) as i32,
        )
    };
    n <= 4 || info[1] & PROC_FP_SHARED != 0
}

/// A directory fd for a synthesized directory.
fn dir_fd(guest: &str, list: Vec<Entry>, cloexec: bool) -> i64 {
    let mut list = list;
    if !guest.starts_with("/sys/fs/fuse") && let Some((host, _)) = recorded(guest)
        && let Ok(rd) = std::fs::read_dir(&host)
    {
        for e in rd.flatten() {
            let name = e.file_name().as_encoded_bytes().to_vec();
            // Only the namespace's processes are directories of /proc.
            let process = guest == "/proc" && name.iter().all(u8::is_ascii_digit);
            if !process && !list.iter().any(|x| x.name == name) {
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

/// The current process's procfs descriptor magic link. Callers must retain the
/// referenced descriptor before changing its inode, rather than reopen its name.
pub(super) fn current_fd_link(guest: &str) -> Option<Result<i32, Errno>> {
    let canon = canonical(guest);
    let tail = canon.strip_prefix(&format!("/proc/{}/fd/", pid()))
        .or_else(|| canon.strip_prefix(&format!("/proc/{}/task/{}/fd/", pid(), super::process::gettid())))?;
    let fd: i32 = tail.parse().ok()?;
    if fd < 0 || fdtab::is_hidden(fd) { return Some(Err(ENOENT)); }
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 { return Some(Err(ENOENT)); }
    Some(Ok(fd))
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
    if let Some(result)=super::fuse_sysfs::open(guest,flags){return Some(result);}
    if !is_kernfs(guest) {
        return None;
    }
    if let Some(node)=super::fuse_sysfs::node(guest){return Some(match node{
        Ok(Node::Dir(entries))=>dir_fd(guest,entries,flags&O_CLOEXEC!=0),
        Ok(Node::File(_) | Node::NetTable(_) | Node::MountTable{..})=>-(errno::EACCES as i64),Ok(Node::Link(_))=>-(errno::EINVAL as i64),Err(error)=>-(error as i64),
    });}
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
        let name = thread_comm(pid(), tid).unwrap_or_else(|| comm(pid()));
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
                Ok(None)
            },
        ));
    }
    if write && let Some(dev) = super::uevent::attribute(&canon) {
        return Some(super::knob::open(&dev.attribute(), cloexec, move |b| {
            super::uevent::synthesize(&dev, b).map(|()| None)
        }));
    }
    // /proc/<pid>/fd/N reopens the file behind fd N.
    if let Some(n) = canon
        .strip_prefix(&format!("/proc/{}/fd/", pid()))
        .and_then(|n| n.parse::<i32>().ok())
    {
        if fdtab::is_hidden(n) {
            return Some(-(ENOENT as i64));
        }
        if let Some(r) = super::memfd::reopen(n, host_flags) {
            return Some(r);
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
        Some(Node::MountTable {host_pid,info}) => {
            if write {return Some(-(EACCES as i64));}
            match process_mount_points(host_pid){Ok(points)=>content_fd(if info{mountinfo(points)}else{mounts(points)}.as_bytes(),cloexec),Err(error)=>-(error as i64)}
        }
        Some(Node::NetTable(kind)) => {
            if write { return Some(-(EACCES as i64)); }
            match super::net::proc_table(&kind) {
                Ok(bytes) => content_fd(&bytes, cloexec),
                Err(error) => -(error as i64),
            }
        }
        Some(Node::File(data)) => {
            if write && device_tree(&canon) {
                // A device attribute with no store method.
                return Some(-(EACCES as i64));
            }
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
        // sysfs creates no files: a missing device attribute stays missing.
        None if device_tree(&canon) => {
            let parent = canon.rsplit_once('/').map_or("", |(p, _)| p);
            if flags & O_CREAT != 0 && matches!(node(parent), Some(Node::Dir(_))) {
                -(EACCES as i64)
            } else {
                -(ENOENT as i64)
            }
        }
        None if flags & O_CREAT != 0 || write => record_write(guest, host_flags),
        None => -(ENOENT as i64),
    })
}

/// Whether `guest` is in sysfs's device trees, which hold only the devices
/// the layer models and their attributes. Values init wrote there are not
/// served: on Linux such a write fails for a device that does not exist
/// (`/sys/class/android_usb` on a device without a USB gadget), and the
/// modeled attributes answer for themselves.
fn device_tree(guest: &str) -> bool {
    ["block", "bus", "class", "dev", "devices"].iter().any(|t| {
        guest
            .strip_prefix("/sys/")
            .and_then(|r| r.strip_prefix(t))
            .is_some_and(|r| r.is_empty() || r.starts_with('/'))
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
    let canon = canonical(guest);
    // A process's files belong to its effective ids, the rest to root.
    let owner = match proc_entry(&canon).map(super::thread::owner) {
        Some(p) if p == pid() => super::attrs::ids(super::attrs::EFFECTIVE),
        Some(p) => {
            let c = super::cred::peer(p);
            (c.uid, c.gid)
        }
        None => (0, 0),
    };
    if let Some(node)=super::fuse_sysfs::node(&canon){return Some(node.map(|node|{
        let mut stat:libc::stat=unsafe{std::mem::zeroed()};stat.st_ino=canon.bytes().fold(1469598103934665603u64,|hash,byte|(hash^u64::from(byte)).wrapping_mul(1099511628211));stat.st_blksize=4096;
        match node{Node::Dir(_)=>{stat.st_mode=libc::S_IFDIR|0o555;stat.st_nlink=2;},_=>{stat.st_mode=libc::S_IFREG|0o200;stat.st_nlink=1;}}stat
    }));}
    if let Some((_, mut st)) = recorded(guest) {
        (st.st_uid, st.st_gid) = owner;
        return Some(Ok(st));
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let me = pid();
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
        Some(Node::File(_) | Node::NetTable(_) | Node::MountTable{..}) => {
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
fn attr_current(p: i32) -> Vec<u8> {
    let mut v = super::cred::seclabel_of(p).into_bytes();
    v.push(0);
    v
}

/// Whether `path` names `/proc/self/exe`.
pub fn is_self_exe(path: &[u8]) -> bool {
    path == b"/proc/self/exe" || path == format!("/proc/{}/exe", pid()).as_bytes()
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Held by tests that start processes or need no other process to
    /// inherit their fds.
    static SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn read_all(fd: i32) -> Vec<u8> {
        let mut b = vec![0u8; 64];
        // SAFETY: a local buffer.
        let n = unsafe { libc::pread(fd, b.as_mut_ptr().cast(), b.len(), 0) };
        b.truncate(n.max(0) as usize);
        b
    }

    #[test]
    fn a_closed_content_file_is_reused_only_when_nothing_else_holds_it() {
        let _spawns = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        let pool = super::Pool(std::sync::Mutex::new(Vec::new()));
        let ino = |fd: i32| {
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: a local buffer.
            unsafe { libc::fstat(fd, &mut st) };
            st.st_ino
        };
        // SAFETY: plain fd calls on our own fds.
        let close = |fd: i32| unsafe { libc::close(fd) };
        let a = super::fresh(b"a longer first content\n").unwrap();
        let first = ino(a);
        // SAFETY: plain dup of our fd.
        let dup = unsafe { libc::dup(a) };
        // Still held by `dup`: not pooled.
        pool.recycle(a);
        close(a);
        assert!(pool.reuse(b"b\n", true).is_none());
        assert_eq!(read_all(dup), b"a longer first content\n");
        // The last fd: pooled and reused, holding exactly its new contents,
        // at offset 0, visible and not close-on-exec as asked.
        pool.recycle(dup);
        close(dup);
        let c = pool.reuse(b"c\n", false).unwrap();
        assert_eq!(ino(c), first);
        assert_eq!(read_all(c), b"c\n");
        // SAFETY: plain fcntl/lseek on our fd.
        unsafe {
            assert_eq!(libc::lseek(c, 0, libc::SEEK_CUR), 0);
            assert_eq!(libc::fcntl(c, libc::F_GETFD) & libc::FD_CLOEXEC, 0);
        }
        assert!(!crate::sys::fdtab::is_hidden(c));
        // A mapped one is not reused: the mapping still shows its contents.
        crate::sys::fdtab::insert(c, crate::sys::fdtab::Kind::Content);
        super::on_mmap(c);
        crate::sys::fdtab::on_close(c);
        pool.recycle(c);
        close(c);
        assert!(pool.reuse(b"d\n", true).is_none());
    }

    #[test]
    fn guest_argv_skips_linux_run_options() {
        let argv = |s: &str| {
            s.split(' ')
                .map(|a| a.as_bytes().to_vec())
                .collect::<Vec<_>>()
        };
        let run = "/t/linux-run --root /r --path-map /m --inherit-env --binder b --stdio-null \
                   --no-cache --trace /system/bin/servicemanager -v";
        assert_eq!(
            super::guest_argv(argv(run)),
            argv("/system/bin/servicemanager -v")
        );
        assert!(super::guest_argv(argv("/t/linux-run --root /r --fork-child 7")).is_empty());
        assert_eq!(
            super::guest_argv(argv("/bin/sh -c x")),
            argv("/bin/sh -c x")
        );
    }

    /// The host's gzip reads `/proc/config.gz` back verbatim.
    #[test]
    fn config_gz_is_gzip() {
        let _spawns = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        let big: String = super::KERNEL_CONFIG.repeat(200);
        for data in ["", super::KERNEL_CONFIG, big.as_str()] {
            let mut child = Command::new("gzip")
                .arg("-dc")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let gz = super::gzip_stored(data.as_bytes());
            child.stdin.take().unwrap().write_all(&gz).unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success());
            assert_eq!(out.stdout, data.as_bytes());
        }
    }
}
