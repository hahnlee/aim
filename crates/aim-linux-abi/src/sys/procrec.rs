//! The process record, `by-pid/<pid>.proc` next to the process's `by-pid`
//! entry: what the other processes of the pid namespace read of this one
//! for `/proc/<pid>` (`docs/guest-init-contract.md` section 4).
//!
//! Linux reads another process's threads and their `comm` from its tasks,
//! and its command line from its memory. Darwin lets no process read
//! another's memory, so the record holds
//! - a table of the process's threads: tid, host thread id (for their CPU
//!   times) and `comm`, written as they change;
//! - the pages of the main thread's stack that hold its argument and
//!   environment strings. The process maps those pages from the record,
//!   shared: the record is that memory, so a program that rewrites its
//!   argv (zygote names its children so) shows it to readers at once.
//!
//! The record is made when the program's stack is noted (start, execve,
//! fork child) and goes with the `by-pid` entry. A fork copies the string
//! pages (`fork::spawn`) and the child maps its copy from its own record.

use std::os::unix::fs::{FileExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU8, AtomicU64, Ordering::*};

use super::procfs::StackInfo;

/// One slot per possible thread of a process (`thread::slot`).
const SLOTS: usize = 2048;
/// Offset of the string pages in the record.
const STRINGS: u64 = 128 << 10;
const PAGE: u64 = super::mem::PAGE;

#[repr(C)]
struct Header {
    /// Guest address and length of the string pages.
    lo: AtomicU64,
    len: AtomicU64,
    args: [AtomicU64; 2],
    env: [AtomicU64; 2],
    _pad: [u64; 2],
}

#[repr(C)]
struct Slot {
    /// 0 for a free slot.
    tid: AtomicI32,
    _pad: u32,
    host: AtomicU64,
    name: [AtomicU8; 16],
}

#[repr(C)]
struct Table {
    header: Header,
    slots: [Slot; SLOTS],
}

const TABLE: usize = std::mem::size_of::<Table>();
const HEADER: usize = std::mem::size_of::<Header>();
const SLOT: usize = std::mem::size_of::<Slot>();
const _: () = assert!(TABLE as u64 <= STRINGS && STRINGS % PAGE == 0);

/// This process's table, mapped from its record.
static OWN: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());
/// The string pages mapped from the record: start and end.
static MAPPED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn path(pid: i32) -> Option<PathBuf> {
    Some(super::cred::by_pid_dir()?.join(format!("{pid}.proc")))
}

/// Make this process's record for the program whose stack is `s`, when the
/// process belongs to a pid namespace, replacing the one it had before
/// execve (or a process of the same pid left).
pub fn init(s: &StackInfo) {
    let Some(dir) = super::cred::by_pid_dir() else {
        return;
    };
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    let spans = [s.args, s.env].into_iter().filter(|(a, b)| b > a);
    let lo = spans.clone().map(|(a, _)| a).min().unwrap_or(0) & !(PAGE - 1);
    let hi = spans
        .map(|(_, b)| b)
        .max()
        .unwrap_or(0)
        .next_multiple_of(PAGE);
    let (lo, hi) = if lo >= s.lo && hi <= s.hi && hi > lo {
        (lo, hi)
    } else {
        (0, 0)
    };
    let tmp = dir.join(format!(".{pid}.proc.tmp"));
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&tmp)
    else {
        return;
    };
    let strings: &[u8] = if hi > lo {
        // SAFETY: the string pages are this process's stack.
        unsafe { std::slice::from_raw_parts(lo as *const u8, (hi - lo) as usize) }
    } else {
        &[]
    };
    let fd = std::os::fd::AsRawFd::as_raw_fd(&f);
    // SAFETY: a fresh shared mapping of the record's table.
    let table = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            TABLE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        )
    };
    if f.set_len(STRINGS + hi - lo).is_err()
        || f.write_all_at(strings, STRINGS).is_err()
        || table == libc::MAP_FAILED
    {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    // SAFETY: just mapped, for the life of the record.
    let t = unsafe { &*(table as *const Table) };
    let h = &t.header;
    for (slot, v) in [&h.lo, &h.len, &h.args[0], &h.args[1], &h.env[0], &h.env[1]]
        .into_iter()
        .zip([lo, hi - lo, s.args.0, s.args.1, s.env.0, s.env.1])
    {
        slot.store(v, Relaxed);
    }
    let old = OWN.swap(table.cast(), AcqRel);
    if !old.is_null() {
        // SAFETY: the previous program's table, no longer reachable.
        unsafe { libc::munmap(old.cast(), TABLE) };
    }
    super::thread::publish_threads();
    // SAFETY: replacing the string pages of our own stack with the same
    // bytes, from the record; no other thread runs yet.
    let mapped = hi == lo
        || unsafe {
            libc::mmap(
                lo as *mut _,
                (hi - lo) as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                fd,
                STRINGS as i64,
            )
        } != libc::MAP_FAILED;
    if mapped {
        MAPPED[0].store(lo, Relaxed);
        MAPPED[1].store(hi, Relaxed);
    }
    let _ = std::fs::rename(&tmp, dir.join(format!("{pid}.proc")));
}

/// Whether the mapping at `addr` is the string pages, which a fork copies.
pub fn is_strings(addr: u64) -> bool {
    (MAPPED[0].load(Relaxed)..MAPPED[1].load(Relaxed)).contains(&addr)
}

fn slot(tid: i32) -> Option<&'static Slot> {
    // SAFETY: mapped for the life of the process (or until execve, which
    // runs on the only thread).
    let t = unsafe { OWN.load(Acquire).as_ref() }?;
    t.slots.get(super::thread::slot(tid))
}

/// Thread `tid` exists, named `name`, on host thread `host` (0 until it
/// runs).
pub fn thread(tid: i32, host: u64, name: &[u8; 16]) {
    if let Some(s) = slot(tid) {
        s.host.store(host, Relaxed);
        named(tid, name);
        s.tid.store(tid, Release);
    }
}

pub fn running(tid: i32, host: u64) {
    if let Some(s) = slot(tid) {
        s.host.store(host, Relaxed);
    }
}

pub fn named(tid: i32, name: &[u8; 16]) {
    if let Some(s) = slot(tid) {
        for (d, &b) in s.name.iter().zip(name) {
            d.store(b, Relaxed);
        }
    }
}

pub fn gone(tid: i32) {
    if let Some(s) = slot(tid) {
        s.tid.store(0, Release);
    }
}

/// The process `pid` is gone: drop its record.
pub fn forget(pid: i32) {
    if let Some(p) = path(pid) {
        let _ = std::fs::remove_file(p);
    }
}

/// A thread of another process.
pub struct ThreadInfo {
    pub tid: i32,
    /// Host thread id (`pthread_threadid_np`), 0 until it runs.
    pub host: u64,
    pub name: [u8; 16],
}

fn open(pid: i32) -> Option<std::fs::File> {
    std::fs::File::open(path(pid)?).ok()
}

fn read_at(f: &std::fs::File, off: u64, len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len];
    let n = f.read_at(&mut v, off).unwrap_or(0);
    v.truncate(n);
    v
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    b.get(i..i + 8)
        .map_or(0, |w| u64::from_le_bytes(w.try_into().unwrap()))
}

/// The threads of process `pid`, by tid; none without a record.
pub fn threads(pid: i32) -> Option<Vec<ThreadInfo>> {
    let f = open(pid)?;
    let b = read_at(&f, HEADER as u64, SLOTS * SLOT);
    let mut v: Vec<ThreadInfo> = b
        .chunks_exact(SLOT)
        .filter_map(|s| {
            let tid = i32::from_le_bytes(s[..4].try_into().unwrap());
            (tid != 0).then(|| ThreadInfo {
                tid,
                host: u64_at(s, 8),
                name: s[16..32].try_into().unwrap(),
            })
        })
        .collect();
    v.sort_unstable_by_key(|t| t.tid);
    Some(v)
}

/// The command line of process `pid`, read from its string pages as
/// Linux reads it from its memory; none without a record.
pub fn cmdline(pid: i32) -> Option<Vec<u8>> {
    let f = open(pid)?;
    let h = read_at(&f, 0, HEADER);
    let (lo, len) = (u64_at(&h, 0), u64_at(&h, 8));
    let args = (u64_at(&h, 16), u64_at(&h, 24));
    let env = (u64_at(&h, 32), u64_at(&h, 40));
    let read = |a: u64, b: u64| {
        let (a, b) = (a.max(lo), b.min(lo + len));
        if b > a {
            read_at(&f, STRINGS + a - lo, (b - a) as usize)
        } else {
            Vec::new()
        }
    };
    Some(super::procfs::linux_cmdline(args, env, read))
}
