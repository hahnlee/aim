//! The process record: what the other processes of the pid namespace read
//! of this one and ask of it, in a file next to its `by-pid` entry
//! (`<pid>.proc`) that the process maps shared.
//!
//! Linux reads another process's `/proc` files from its task and memory
//! and changes its threads' scheduling directly. Darwin lets no process
//! read another's memory, and sets a thread's QoS only from the thread
//! itself. So:
//! - the process keeps its threads in the record as they change: tid,
//!   `comm`, policy, priority and nice value (`/proc/<pid>/task`, `comm`
//!   and the scheduling calls on another process read them there);
//! - `cmdline` is the argument area of the guest's memory, which a program
//!   rewrites in place (zygote names its children so): a reader asks for
//!   it, and the process copies it into the record from its handler of
//!   the request signal. A process that does not answer in time is read
//!   as it last answered (or as it started);
//! - a scheduling change for one of its threads is written into the
//!   thread's slot, with the same signal: the process hands it to the
//!   thread, which applies it to its host thread (`process`).
//!
//! The request signal is Darwin's SIGINFO, which Linux lacks and Darwin
//! ignores by default, so a process that does not serve requests yet (or
//! any more) is unharmed.

use std::path::Path;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU8, AtomicU32, Ordering::*};
use std::time::{Duration, Instant};

/// The host signal of a request.
pub const REQUEST: i32 = libc::SIGINFO;
/// One slot per possible tid of a process (`thread`).
const SLOTS: usize = 4096;
/// The part of the argument area a record holds.
const ARGS_MAX: usize = (64 << 10) - 64;
/// How long a reader waits for an answer.
const PATIENCE: Duration = Duration::from_millis(50);

#[repr(C)]
struct Header {
    /// Requests for the argument area: the last one asked and the last one
    /// answered.
    asked: AtomicU32,
    answered: AtomicU32,
    /// Set with a slot's `changed`.
    changed: AtomicU32,
    args_len: AtomicU32,
    _pad: [u32; 12],
}

#[repr(C)]
struct Slot {
    /// 0 for a free slot.
    tid: AtomicI32,
    /// [`Sched`], packed.
    sched: AtomicU32,
    /// Another process changed `sched`.
    changed: AtomicU32,
    _pad: u32,
    name: [AtomicU8; 16],
}

#[repr(C)]
struct Record {
    header: Header,
    args: [AtomicU8; ARGS_MAX],
    slots: [Slot; SLOTS],
}

const SIZE: usize = std::mem::size_of::<Record>();

/// A thread's scheduling as Linux keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sched {
    pub policy: i32,
    pub priority: i32,
    pub nice: i32,
}

impl Sched {
    fn pack(self) -> u32 {
        (self.policy as u8 as u32)
            | (self.priority as u8 as u32) << 8
            | (self.nice as u8 as u32) << 16
    }

    fn unpack(w: u32) -> Sched {
        Sched {
            policy: (w & 0xff) as i32,
            priority: ((w >> 8) & 0xff) as i32,
            nice: (w >> 16) as u8 as i8 as i32,
        }
    }
}

/// This process's record, once mapped.
static OWN: AtomicPtr<Record> = AtomicPtr::new(std::ptr::null_mut());

fn me() -> i32 {
    // SAFETY: trivial.
    unsafe { libc::getpid() }
}

fn own() -> Option<&'static Record> {
    // SAFETY: mapped once for the life of the process, never unmapped.
    unsafe { OWN.load(Acquire).as_ref() }
}

fn path(dir: &Path, pid: i32) -> std::path::PathBuf {
    dir.join(format!("{pid}.proc"))
}

/// Map `fd`'s record shared.
fn map(fd: i32, write: bool) -> Option<*mut Record> {
    let prot = libc::PROT_READ | if write { libc::PROT_WRITE } else { 0 };
    // SAFETY: a fresh shared mapping of the whole record.
    let p = unsafe { libc::mmap(std::ptr::null_mut(), SIZE, prot, libc::MAP_SHARED, fd, 0) };
    (p != libc::MAP_FAILED).then_some(p.cast())
}

/// Make this process's record in the process table `dir`, replacing a
/// record a process of the same pid left or this one had before exec. The
/// main thread is in it with `name` and nice value `nice`.
pub fn init(dir: &Path, nice: i32) {
    let pid = me();
    let tmp = dir.join(format!(".{pid}.proc.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let rec = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&tmp)
        .ok()
        .filter(|f| f.set_len(SIZE as u64).is_ok())
        .and_then(|f| map(std::os::fd::AsRawFd::as_raw_fd(&f), true));
    let Some(rec) = rec else {
        let _ = std::fs::remove_file(&tmp);
        return;
    };
    // Complete before anyone can open it.
    // SAFETY: just mapped.
    let r = unsafe { &*rec };
    r.slots[0].tid.store(pid, Relaxed);
    r.slots[0].sched.store(
        Sched {
            nice,
            ..Sched::default()
        }
        .pack(),
        Relaxed,
    );
    if std::fs::rename(&tmp, path(dir, pid)).is_err() {
        let _ = std::fs::remove_file(&tmp);
        // SAFETY: our unpublished mapping.
        unsafe { libc::munmap(rec.cast(), SIZE) };
        return;
    }
    OWN.store(rec, Release);
}

/// Process `pid` is gone: drop its record.
pub fn forget(dir: &Path, pid: i32) {
    let _ = std::fs::remove_file(path(dir, pid));
}

fn own_slot(tid: i32) -> Option<&'static Slot> {
    Some(&own()?.slots[super::thread::slot_of(me(), tid)?])
}

fn store_name(s: &Slot, name: &[u8; 16]) {
    for (d, b) in s.name.iter().zip(name) {
        d.store(*b, Relaxed);
    }
}

/// Thread `tid` started with `name` and scheduling `sched`.
pub fn add_thread(tid: i32, name: &[u8; 16], sched: Sched) {
    if let Some(s) = own_slot(tid) {
        store_name(s, name);
        s.sched.store(sched.pack(), Relaxed);
        s.changed.store(0, Relaxed);
        s.tid.store(tid, Release);
    }
}

/// Thread `tid` ended.
pub fn remove_thread(tid: i32) {
    if let Some(s) = own_slot(tid) {
        s.tid.store(0, Release);
    }
}

/// Thread `tid`'s `comm` changed.
pub fn set_name(tid: i32, name: &[u8; 16]) {
    if let Some(s) = own_slot(tid) {
        store_name(s, name);
    }
}

/// Thread `tid`'s scheduling changed here: `f` makes the new one.
pub fn set_sched(tid: i32, f: impl Fn(Sched) -> Sched) {
    if let Some(s) = own_slot(tid) {
        let _ = s
            .sched
            .fetch_update(Relaxed, Relaxed, |w| Some(f(Sched::unpack(w)).pack()));
    }
}

/// The guest's argument area was recorded or reset (at exec, in a fork
/// child): the answer until the first request.
pub fn note_args() {
    if let Some(r) = own() {
        copy_args(r);
    }
}

/// Copy the argument area into the record. Async-signal-safe.
fn copy_args(r: &Record) {
    let (lo, hi) = super::procfs::args_area();
    let len = (hi.saturating_sub(lo) as usize).min(ARGS_MAX);
    let mut got = 0u64;
    // SAFETY: a fault-safe copy from our own task into the record.
    unsafe {
        mach_vm_read_overwrite(
            mach_task_self_,
            lo,
            len as u64,
            r.args.as_ptr() as u64,
            &mut got,
        )
    };
    r.header.args_len.store(got as u32, Release);
}

unsafe extern "C" {
    static mach_task_self_: libc::mach_port_t;
    fn mach_vm_read_overwrite(t: libc::mach_port_t, a: u64, s: u64, d: u64, o: *mut u64) -> i32;
}

/// The request signal arrived (a host signal handler, on any thread):
/// answer a request for the argument area. True when a thread's
/// scheduling changed, for a thread of the process to take over
/// ([`take_changes`]).
pub fn serve() -> bool {
    let Some(r) = own() else { return false };
    let h = &r.header;
    let asked = h.asked.load(Acquire);
    if h.answered.load(Relaxed) != asked {
        copy_args(r);
        h.answered.store(asked, Release);
    }
    h.changed.load(Relaxed) != 0
}

/// The threads whose scheduling another process changed, with the new
/// scheduling.
pub fn take_changes() -> Vec<(i32, Sched)> {
    let Some(r) = own() else { return Vec::new() };
    let h = &r.header;
    if h.changed.load(Relaxed) == 0 || h.changed.swap(0, AcqRel) == 0 {
        return Vec::new();
    }
    super::thread::tids()
        .into_iter()
        .filter_map(|tid| {
            let s = own_slot(tid)?;
            (s.changed.swap(0, AcqRel) != 0).then(|| (tid, Sched::unpack(s.sched.load(Acquire))))
        })
        .collect()
}

/// Another process's record, mapped for a moment.
pub struct Peer {
    pid: i32,
    rec: *mut Record,
}

impl Drop for Peer {
    fn drop(&mut self) {
        // SAFETY: our mapping.
        unsafe { libc::munmap(self.rec.cast(), SIZE) };
    }
}

impl Peer {
    /// The record of process `pid` of the namespace, if it has made one
    /// (a record older than the process is another's).
    pub fn open(pid: i32) -> Option<Peer> {
        let dir = super::cred::by_pid_dir()?;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path(dir, pid))
            .ok()?;
        let m = f.metadata().ok()?;
        let started = super::pidns::started(pid)?;
        if m.len() != SIZE as u64 || m.created().ok()? < started {
            return None;
        }
        let rec = map(std::os::fd::AsRawFd::as_raw_fd(&f), true)?;
        Some(Peer { pid, rec })
    }

    fn rec(&self) -> &Record {
        // SAFETY: mapped while self lives.
        unsafe { &*self.rec }
    }

    fn slot(&self, tid: i32) -> Option<&Slot> {
        let s = &self.rec().slots[super::thread::slot_of(self.pid, tid)?];
        (s.tid.load(Acquire) == tid).then_some(s)
    }

    /// The process's threads, in order.
    pub fn tids(&self) -> Vec<i32> {
        let mut v: Vec<i32> = self
            .rec()
            .slots
            .iter()
            .map(|s| s.tid.load(Acquire))
            .filter(|&t| t != 0)
            .collect();
        v.sort_unstable();
        v
    }

    pub fn has(&self, tid: i32) -> bool {
        self.slot(tid).is_some()
    }

    /// Thread `tid`'s `comm`.
    pub fn name(&self, tid: i32) -> Option<[u8; 16]> {
        let s = self.slot(tid)?;
        Some(std::array::from_fn(|i| s.name[i].load(Relaxed)))
    }

    pub fn sched(&self, tid: i32) -> Option<Sched> {
        Some(Sched::unpack(self.slot(tid)?.sched.load(Acquire)))
    }

    fn ask(&self) {
        // SAFETY: a signal Darwin ignores unless the process serves it.
        unsafe { libc::kill(self.pid, REQUEST) };
    }

    /// Change thread `tid`'s scheduling with `f`, for the process to apply.
    /// False when the thread does not exist.
    pub fn set_sched(&self, tid: i32, f: impl Fn(Sched) -> Sched) -> bool {
        let Some(s) = self.slot(tid) else {
            return false;
        };
        let _ = s
            .sched
            .fetch_update(AcqRel, Acquire, |w| Some(f(Sched::unpack(w)).pack()));
        s.changed.store(1, Release);
        self.rec().header.changed.store(1, Release);
        self.ask();
        true
    }

    /// The process's argument area, as it answers now or answered last.
    pub fn args(&self) -> Vec<u8> {
        let h = &self.rec().header;
        let asked = h.asked.fetch_add(1, AcqRel).wrapping_add(1);
        self.ask();
        let deadline = Instant::now() + PATIENCE;
        while (h.answered.load(Acquire).wrapping_sub(asked) as i32) < 0 {
            if Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_micros(50));
        }
        let len = (h.args_len.load(Acquire) as usize).min(ARGS_MAX);
        self.rec().args[..len]
            .iter()
            .map(|b| b.load(Relaxed))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Sched;

    #[test]
    fn sched_packs_negative_nice_values() {
        for s in [
            Sched::default(),
            Sched {
                policy: 1,
                priority: 99,
                nice: -20,
            },
            Sched {
                policy: 5,
                priority: 0,
                nice: 19,
            },
        ] {
            assert_eq!(Sched::unpack(s.pack()), s);
        }
    }
}
