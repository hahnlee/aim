//! inotify over kqueue EVFILT_VNODE.
//!
//! An inotify fd is a kqueue with one vnode knote per watch. Directory
//! watches keep a snapshot of their entries; a vnode event rescans the
//! directory and turns the differences into IN_CREATE, IN_DELETE and (for
//! entries whose size or mtime changed) IN_MODIFY + IN_CLOSE_WRITE. A
//! write into a file changes nothing of its directory's vnode on Darwin,
//! so a directory watch that asks for IN_MODIFY or IN_CLOSE_WRITE also has
//! a knote on each regular file in it, which reports them for that entry.
//! A file watch reports IN_MODIFY/IN_CLOSE_WRITE on writes. Self events
//! (IN_ATTRIB, IN_DELETE_SELF, IN_MOVE_SELF, IN_IGNORED) come from the
//! knote. Rename pairing (IN_MOVED_FROM/TO cookies) is not reported: a
//! rename inside a watched directory shows as delete + create.
//!
//! Events are decoded into a queue when read or polled; while the queue
//! is not empty, a user event keeps the kqueue readable. A vnode note may
//! decode to nothing, so a readable kqueue is only a candidate: poll,
//! select and epoll ask [`readable`], as Linux asks inotify_poll.
//!
//! With O_ASYNC, each queued event signals the F_SETOWN owner. Events are
//! then decoded as they arrive: a private kqueue holds a second knote on
//! each watched vnode, and the O_ASYNC watcher thread waits on all of
//! them (the inotify's own kqueue cannot be waited on for that: a
//! nested kqueue that is already readable does not report another note).

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::sync::{Arc, Mutex, Weak};

use super::fdtab::{self, Kind};
use super::fown::Fown;
use crate::errno::{self, EAGAIN, EBADF, EINVAL};
use crate::sys::guest_cstr;
use crate::vfs;

const IN_MODIFY: u32 = 0x2;
const IN_ATTRIB: u32 = 0x4;
const IN_CLOSE_WRITE: u32 = 0x8;
const IN_CREATE: u32 = 0x100;
const IN_DELETE: u32 = 0x200;
const IN_DELETE_SELF: u32 = 0x400;
const IN_MOVE_SELF: u32 = 0x800;
const IN_Q_OVERFLOW: u32 = 0x4000;
const IN_IGNORED: u32 = 0x8000;
const IN_ISDIR: u32 = 0x4000_0000;
const IN_ONESHOT: u32 = 0x8000_0000;
const IN_ONLYDIR: u32 = 0x0100_0000;
const IN_DONT_FOLLOW: u32 = 0x0200_0000;
const IN_MASK_ADD: u32 = 0x2000_0000;
const IN_ALL_EVENTS: u32 = 0xfff;
/// A knote's udata bit for a directory entry's knote: the watch's wd in
/// bits 32-61, the entry's fd below.
const ENTRY: usize = 1 << 62;

/// /proc/sys/fs/inotify/max_queued_events at Linux's default.
pub(super) const MAX_QUEUED_EVENTS: usize = 16384;

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;

const VNODE_NOTES: u32 = libc::NOTE_WRITE
    | libc::NOTE_EXTEND
    | libc::NOTE_ATTRIB
    | libc::NOTE_DELETE
    | libc::NOTE_RENAME
    | libc::NOTE_REVOKE;

/// (inode, size, mtime ns, is a directory) of a directory entry.
type Snapshot = HashMap<Vec<u8>, (u64, i64, i64, bool)>;

struct Watch {
    /// O_EVTONLY fd on the watched vnode.
    fd: i32,
    dev_ino: (u64, u64),
    mask: u32,
    host: CString,
    dir: Option<Snapshot>,
    /// O_EVTONLY fds on the directory's regular files, by name, while
    /// the mask asks for writes into them.
    entries: HashMap<Vec<u8>, i32>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        for fd in std::iter::once(self.fd).chain(self.entries.values().copied()) {
            if let Err(error) = fdtab::close_fork_private(fd) {
                crate::diag!("inotify private watch close: errno {error}");
            }
        }
    }
}

/// O_ASYNC, set through guest fd `fd` (the signals' `si_fd`).
struct Fasync {
    fd: i32,
    /// None in a fork child until [`after_fork_child`].
    armed: Option<Armed>,
}

/// An O_ASYNC inotify's registration with the watcher thread.
struct Armed {
    /// A private reference to the inotify's kqueue.
    kq: i32,
    /// The private kqueue holding a second knote on each watched vnode.
    mirror: i32,
    /// The inotify's key in [`Watcher::inotifies`].
    id: u64,
}

#[derive(Default)]
struct State {
    watches: HashMap<i32, Watch>,
    next_wd: i32,
    queue: VecDeque<Vec<u8>>,
    /// Whether an IN_Q_OVERFLOW waits in the queue.
    overflowed: bool,
    owner: Fown,
    fasync: Option<Fasync>,
    /// Events queued under O_ASYNC whose signal [`Inotify::unlock`] sends.
    raised: u32,
}

fn mask_of(e: &[u8]) -> u32 {
    u32::from_le_bytes([e[4], e[5], e[6], e[7]])
}

impl State {
    /// fsnotify_add_event: past max_queued_events an event is dropped and
    /// one IN_Q_OVERFLOW (wd -1) is queued unless one already waits; an
    /// event equal to the last queued one (inotify_merge: wd, mask, cookie
    /// and name), unless that is an IN_IGNORED, is folded into it. Each
    /// event queued signals the owner under O_ASYNC (`kill_fasync`).
    fn push(&mut self, e: Vec<u8>) {
        if self.queue.len() >= MAX_QUEUED_EVENTS {
            if !self.overflowed {
                self.overflowed = true;
                self.queue.push_back(event(-1, IN_Q_OVERFLOW, b""));
                self.raise();
            }
            return;
        }
        if self
            .queue
            .back()
            .is_some_and(|last| *last == e && mask_of(last) & IN_IGNORED == 0)
        {
            return;
        }
        self.queue.push_back(e);
        self.raise();
    }

    fn raise(&mut self) {
        if self.fasync.is_some() {
            self.raised += 1;
        }
    }

    /// Take the oldest event.
    fn pop(&mut self) {
        if self
            .queue
            .pop_front()
            .is_some_and(|e| mask_of(&e) == IN_Q_OVERFLOW)
        {
            self.overflowed = false;
        }
    }

    /// The private kqueue of the watched vnodes' second knotes.
    fn mirror(&self) -> Option<i32> {
        Some(self.fasync.as_ref()?.armed.as_ref()?.mirror)
    }
}

pub struct Inotify {
    state: Mutex<State>,
}

fn snapshot(host: &CString) -> Snapshot {
    let mut s = Snapshot::new();
    use std::os::unix::ffi::OsStrExt;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(host.as_bytes()));
    if let Ok(rd) = std::fs::read_dir(path) {
        use std::os::unix::fs::MetadataExt;
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                s.insert(
                    e.file_name().as_encoded_bytes().to_vec(),
                    (
                        m.ino(),
                        m.size() as i64,
                        m.mtime() * 1_000_000_000 + m.mtime_nsec(),
                        m.is_dir(),
                    ),
                );
            }
        }
    }
    s
}

fn event(wd: i32, mask: u32, name: &[u8]) -> Vec<u8> {
    let len = if name.is_empty() {
        0
    } else {
        (name.len() + 1).div_ceil(16) * 16
    };
    let mut e = Vec::with_capacity(16 + len);
    e.extend_from_slice(&wd.to_le_bytes());
    e.extend_from_slice(&mask.to_le_bytes());
    e.extend_from_slice(&0u32.to_le_bytes());
    e.extend_from_slice(&(len as u32).to_le_bytes());
    e.extend_from_slice(name);
    e.resize(16 + len, 0);
    e
}

fn register(kq: i32, wd: i32, fd: i32, mirror: Option<i32>) -> bool {
    register_note(kq, fd, wd as usize, mirror)
}

/// A knote on vnode `fd`, and its second one on `mirror` under O_ASYNC.
fn register_note(kq: i32, fd: i32, udata: usize, mirror: Option<i32>) -> bool {
    vnode_note(kq, fd, udata) && mirror.is_none_or(|mirror| vnode_note(mirror, fd, 0))
}

fn vnode_note(kq: i32, fd: i32, udata: usize) -> bool {
    let k = libc::kevent {
        ident: fd as usize,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: VNODE_NOTES,
        data: 0,
        udata: udata as *mut _,
    };
    // SAFETY: one change, no events.
    unsafe { libc::kevent(kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
}

fn entry_udata(wd: i32, fd: i32) -> usize {
    ENTRY | (wd as usize) << 32 | fd as usize
}

/// Make watch `wd`'s entry knotes match its snapshot and mask: one on each
/// regular file while the mask asks for IN_MODIFY or IN_CLOSE_WRITE, none
/// otherwise. A replaced file (another inode) gets a new one.
fn sync_entries(kq: i32, wd: i32, w: &mut Watch, mirror: Option<i32>) {
    let wanted = w.mask & (IN_MODIFY | IN_CLOSE_WRITE) != 0;
    let files: HashMap<&Vec<u8>, u64> = match &w.dir {
        Some(d) if wanted => d
            .iter()
            .filter(|(_, e)| !e.3)
            .map(|(n, e)| (n, e.0))
            .collect(),
        _ => HashMap::new(),
    };
    w.entries.retain(|name, fd| {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: our entry fd, local buffer.
        let ino = (unsafe { libc::fstat(*fd, &mut st) } == 0).then_some(st.st_ino);
        let keep = ino.is_some() && files.get(name).copied() == ino;
        if !keep {
            if let Err(error) = fdtab::close_fork_private(*fd) {
                crate::diag!("inotify private entry close: errno {error}");
            }
        }
        keep
    });
    for name in files.keys() {
        if w.entries.contains_key(*name) {
            continue;
        }
        let mut path = w.host.as_bytes().to_vec();
        path.push(b'/');
        path.extend_from_slice(name);
        let Ok(path) = CString::new(path) else {
            continue;
        };
        // SAFETY: an event-only open of the entry.
        let efd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_EVTONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if efd < 0 {
            continue;
        }
        if register_note(kq, efd, entry_udata(wd, efd), mirror) {
            w.entries.insert((*name).clone(), efd);
        } else {
            // SAFETY: our entry fd.
            unsafe { libc::close(efd) };
        }
    }
}

/// Keep the kqueue readable while decoded events wait in the queue.
fn signal_pending(kq: i32, on: bool) -> bool {
    let k = libc::kevent {
        ident: 0,
        filter: libc::EVFILT_USER,
        flags: if on {
            libc::EV_ADD | libc::EV_ENABLE
        } else {
            libc::EV_DELETE
        },
        fflags: if on { libc::NOTE_TRIGGER } else { 0 },
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: one change, no events.
    unsafe { libc::kevent(kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
}

pub fn inotify_init1(a: [u64; 6]) -> i64 {
    let flags = a[0];
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: plain kqueue.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return -(errno::last() as i64);
    }
    fdtab::set_flags(kq, flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0);
    fdtab::insert(
        kq,
        Kind::Inotify(Arc::new(Inotify {
            state: Mutex::new(State {
                next_wd: 1,
                ..Default::default()
            }),
        })),
    );
    if let Err(error) = fdtab::publish_guest(kq) {
        fdtab::on_close(kq);
        unsafe {
            libc::close(kq);
        }
        return -(error as i64);
    }
    kq as i64
}

fn inotify_of(fd: i32) -> Result<Arc<Inotify>, i64> {
    match fdtab::get(fd) {
        Some(Kind::Inotify(i)) => Ok(i),
        // SAFETY: plain fcntl to tell EBADF from EINVAL.
        _ if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 => Err(-(EBADF as i64)),
        _ => Err(-(EINVAL as i64)),
    }
}

pub fn inotify_add_watch(a: [u64; 6]) -> i64 {
    let (fd, mask) = (a[0] as i32, a[2] as u32);
    let ino = match inotify_of(fd) {
        Ok(i) => i,
        Err(e) => return e,
    };
    if mask & IN_ALL_EVENTS == 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest path.
    let owned_path = match guest_cstr(a[1]) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    let path = owned_path.as_slice();
    let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, path, mask & IN_DONT_FOLLOW == 0) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    if unsafe { libc::lstat(r.host.as_ptr(), &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    let is_dir = st.st_mode & libc::S_IFMT == libc::S_IFDIR;
    if mask & IN_ONLYDIR != 0 && !is_dir {
        return -20; // ENOTDIR
    }
    let dev_ino = (st.st_dev as u32 as u64, st.st_ino);
    let mut s = ino.state.lock().unwrap();
    let mirror = s.mirror();
    if let Some((wd, w)) = s.watches.iter_mut().find(|(_, w)| w.dev_ino == dev_ino) {
        w.mask = if mask & IN_MASK_ADD != 0 {
            w.mask | mask
        } else {
            mask
        };
        sync_entries(fd, *wd, w, mirror);
        return *wd as i64;
    }
    // SAFETY: an event-only open of the watched path.
    let wfd = unsafe { libc::open(r.host.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
    if wfd < 0 {
        return -(errno::last() as i64);
    }
    let wd = s.next_wd;
    if !register(fd, wd, wfd, mirror) {
        // SAFETY: our watch fd.
        unsafe { libc::close(wfd) };
        return -(errno::last() as i64);
    }
    s.next_wd += 1;
    let dir = is_dir.then(|| snapshot(&r.host));
    let mut w = Watch {
        fd: wfd,
        dev_ino,
        mask,
        host: r.host,
        dir,
        entries: HashMap::new(),
    };
    sync_entries(fd, wd, &mut w, mirror);
    s.watches.insert(wd, w);
    wd as i64
}

fn remove(fd: i32, s: &mut State, wd: i32) -> bool {
    let Some(w) = s.watches.remove(&wd) else {
        return false;
    };
    drop(w);
    s.push(event(wd, IN_IGNORED, b""));
    signal_pending(fd, true);
    true
}

pub fn inotify_rm_watch(a: [u64; 6]) -> i64 {
    let (fd, wd) = (a[0] as i32, a[1] as i32);
    let ino = match inotify_of(fd) {
        Ok(i) => i,
        Err(e) => return e,
    };
    let mut s = ino.state.lock().unwrap();
    let removed = remove(fd, &mut s, wd);
    ino.unlock(s);
    if removed { 0 } else { -(EINVAL as i64) }
}

/// Decode what the knote of watch `wd` reported.
fn decode(fd: i32, s: &mut State, wd: i32, notes: u32) {
    let mirror = s.mirror();
    let Some(w) = s.watches.get_mut(&wd) else {
        return;
    };
    let mut out = Vec::new();
    let dir_flag = if w.dir.is_some() { IN_ISDIR } else { 0 };
    if let Some(old) = &mut w.dir {
        if notes & (libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_ATTRIB) != 0 {
            let new = snapshot(&w.host);
            for (name, e) in &new {
                let isdir = if e.3 { IN_ISDIR } else { 0 };
                match old.get(name) {
                    None => {
                        out.push((IN_CREATE | isdir, name.clone()));
                        // Written before its knote could see it.
                        if !e.3 && e.1 > 0 {
                            out.push((IN_MODIFY, name.clone()));
                            out.push((IN_CLOSE_WRITE, name.clone()));
                        }
                    }
                    Some(o) if o.0 == e.0 && (o.1, o.2) != (e.1, e.2) && !e.3 => {
                        out.push((IN_MODIFY, name.clone()));
                        out.push((IN_CLOSE_WRITE, name.clone()));
                    }
                    Some(o) if o.0 != e.0 => {
                        out.push((IN_DELETE | if o.3 { IN_ISDIR } else { 0 }, name.clone()));
                        out.push((IN_CREATE | isdir, name.clone()));
                    }
                    _ => {}
                }
            }
            for (name, o) in old.iter() {
                if !new.contains_key(name) {
                    out.push((IN_DELETE | if o.3 { IN_ISDIR } else { 0 }, name.clone()));
                }
            }
            *old = new;
        }
    } else if notes & (libc::NOTE_WRITE | libc::NOTE_EXTEND) != 0 {
        out.push((IN_MODIFY, Vec::new()));
        out.push((IN_CLOSE_WRITE, Vec::new()));
    }
    if w.dir.is_some() {
        sync_entries(fd, wd, w, mirror);
    }
    if notes & libc::NOTE_ATTRIB != 0 {
        out.push((IN_ATTRIB | dir_flag, Vec::new()));
    }
    if notes & libc::NOTE_RENAME != 0 {
        out.push((IN_MOVE_SELF | dir_flag, Vec::new()));
    }
    let gone = notes & (libc::NOTE_DELETE | libc::NOTE_REVOKE) != 0;
    if gone {
        out.push((IN_DELETE_SELF | dir_flag, Vec::new()));
    }
    let mask = w.mask;
    let mut fired = false;
    for (m, name) in out {
        if m & mask & IN_ALL_EVENTS != 0 {
            s.push(event(wd, m, &name));
            fired = true;
        }
    }
    if gone || (fired && mask & IN_ONESHOT != 0) {
        remove(fd, s, wd);
    }
}

/// A write into entry `efd` of directory watch `wd`: IN_MODIFY and
/// IN_CLOSE_WRITE for its name, and its snapshot entry updated so the next
/// rescan does not report it again. Its removal and renames come from the
/// directory's own knote.
fn decode_entry(fd: i32, s: &mut State, wd: i32, efd: i32, notes: u32) {
    if notes & (libc::NOTE_WRITE | libc::NOTE_EXTEND) == 0 {
        return;
    }
    let Some(w) = s.watches.get_mut(&wd) else {
        return;
    };
    let Some(name) = w
        .entries
        .iter()
        .find(|(_, f)| **f == efd)
        .map(|(n, _)| n.clone())
    else {
        return;
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: our entry fd, local buffer.
    if unsafe { libc::fstat(efd, &mut st) } == 0
        && let Some(e) = w.dir.as_mut().and_then(|d| d.get_mut(&name))
    {
        e.1 = st.st_size;
        e.2 = st.st_mtime * 1_000_000_000 + st.st_mtime_nsec;
    }
    let mask = w.mask;
    let mut fired = false;
    for m in [IN_MODIFY, IN_CLOSE_WRITE] {
        if m & mask != 0 {
            s.push(event(wd, m, &name));
            fired = true;
        }
    }
    if fired && mask & IN_ONESHOT != 0 {
        remove(fd, s, wd);
    }
}

/// Move all pending knote events into the queue; wait for one when
/// `block`.
fn collect(fd: i32, s: &mut State, mut block: bool) -> i64 {
    loop {
        let n = collect_some(fd, s, block);
        if n < 32 {
            return n.min(0);
        }
        block = false;
    }
}

/// Up to 32 knote events into the queue: their count, or a negative errno.
fn collect_some(fd: i32, s: &mut State, block: bool) -> i64 {
    let mut evs = [libc::kevent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    }; 32];
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: output array we own.
    let n = unsafe {
        libc::kevent(
            fd,
            std::ptr::null(),
            0,
            evs.as_mut_ptr(),
            evs.len() as i32,
            if block { std::ptr::null() } else { &zero },
        )
    };
    if n < 0 {
        return -(errno::last() as i64);
    }
    for e in &evs[..n as usize] {
        let udata = e.udata as usize;
        if e.filter == libc::EVFILT_VNODE && udata & ENTRY != 0 {
            decode_entry(
                fd,
                s,
                (udata >> 32) as i32 & 0x3fff_ffff,
                udata as u32 as i32,
                e.fflags,
            );
        } else if e.filter == libc::EVFILT_VNODE {
            decode(fd, s, udata as i32, e.fflags);
        }
    }
    n as i64
}

/// read(2) on an inotify fd; None when `fd` is not one.
pub fn read(fd: i32, buf: u64, len: usize) -> Option<i64> {
    let Kind::Inotify(ino) = fdtab::get(fd)? else {
        return None;
    };
    let mut s = ino.state.lock().unwrap();
    loop {
        let r = collect(fd, &mut s, false);
        if r < 0 {
            ino.unlock(s);
            return Some(r);
        }
        if !s.queue.is_empty() {
            break;
        }
        if fdtab::nonblocking(fd) {
            signal_pending(fd, false);
            ino.unlock(s);
            return Some(-(EAGAIN as i64));
        }
        ino.unlock(s);
        let r = fdtab::wait_for(fd, libc::POLLIN);
        s = ino.state.lock().unwrap();
        if r < 0 {
            ino.unlock(s);
            return Some(r);
        }
    }
    let mut off = 0usize;
    let mut fault = None;
    while let Some(e) = s.queue.front() {
        if off + e.len() > len {
            break;
        }
        // SAFETY: guest buffer of `len` bytes.
        if let Err(error) = unsafe {
            crate::sys::user_memory::copy_bytes(
                e.as_ptr() as *const u8,
                buf.wrapping_add(off as u64) as *mut u8,
                e.len(),
            )
        } {
            fault = Some(error);
            break;
        }
        off += e.len();
        s.pop();
    }
    signal_pending(fd, !s.queue.is_empty());
    ino.unlock(s);
    if let Some(error) = fault {
        return crate::sys::user_memory::CopyError::copy_error(error);
    }
    Some(if off == 0 {
        -(EINVAL as i64)
    } else {
        off as i64
    })
}

/// FIONREAD: bytes of decoded events ready.
pub fn pending_bytes(fd: i32) -> Option<i64> {
    let Kind::Inotify(ino) = fdtab::get(fd)? else {
        return None;
    };
    let mut s = ino.state.lock().unwrap();
    collect(fd, &mut s, false);
    signal_pending(fd, !s.queue.is_empty());
    let bytes = s.queue.iter().map(|e| e.len() as i64).sum();
    ino.unlock(s);
    Some(bytes)
}

/// inotify_poll: whether inotify fd `fd` has an event to read, None when
/// `fd` is not one. Its kqueue is readable on any vnode note, and a note
/// may decode to nothing (a rescan that finds what an earlier one
/// reported, an event outside the mask); Linux is readable only while
/// events are queued. Collecting clears those notes, so the kqueue stays
/// readable only while the queue is not empty and a level-triggered wait
/// on it sleeps.
pub fn readable(fd: i32) -> Option<bool> {
    pending_bytes(fd).map(|bytes| bytes > 0)
}

impl Inotify {
    /// Release the state, then signal the owner for each event queued
    /// under O_ASYNC: sending can reach another process's agent, which the
    /// inotify must not wait on locked.
    fn unlock(&self, mut s: std::sync::MutexGuard<'_, State>) {
        let raised = std::mem::take(&mut s.raised);
        let fd = s.fasync.as_ref().map(|f| f.fd);
        let owner = s.owner.owner();
        drop(s);
        if let Some(fd) = fd {
            for _ in 0..raised {
                owner.readable(fd);
            }
        }
    }

    /// fcntl's owner commands (F_SETOWN, F_SETSIG, ...); None for others.
    pub fn owner_fcntl(&self, cmd: u64, arg: u64) -> Option<i64> {
        self.state.lock().unwrap().owner.fcntl(cmd, arg)
    }

    /// Whether O_ASYNC is set.
    pub fn is_async(&self) -> bool {
        self.state.lock().unwrap().fasync.is_some()
    }

    /// The watcher thread saw a watched vnode change: decode what the
    /// inotify's kqueue holds, which queues (and signals) its events.
    fn arrived(&self) {
        let mut s = self.state.lock().unwrap();
        let Some((kq, mirror)) = s
            .fasync
            .as_ref()
            .and_then(|f| f.armed.as_ref())
            .map(|a| (a.kq, a.mirror))
        else {
            return;
        };
        // Drained first: a note after this is in both kqueues.
        drain(mirror);
        let r = collect(kq, &mut s, false);
        if r < 0 {
            crate::diag!("inotify O_ASYNC collect: errno {}", -r);
        }
        signal_pending(kq, !s.queue.is_empty());
        self.unlock(s);
    }
}

impl Drop for Inotify {
    fn drop(&mut self) {
        let s = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        if let Some(armed) = s.fasync.take().and_then(|f| f.armed) {
            disarm(armed);
        }
    }
}

/// The O_ASYNC watcher thread's kqueue (a knote on each armed inotify's
/// mirror kqueue, with its id), and the inotifies by id.
struct Watcher {
    kq: i32,
    inotifies: HashMap<u64, Weak<Inotify>>,
    next: u64,
}

static WATCHER: Mutex<Option<Watcher>> = Mutex::new(None);

fn watch(kq: i32) {
    // SAFETY: this host thread takes no signals, so the kernel never picks
    // it for a process-directed one.
    unsafe {
        let mut all: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }
    // SAFETY: an all-zero kevent is valid.
    let mut evs: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: waiting for events into a local buffer.
        let n = unsafe {
            libc::kevent(
                kq,
                std::ptr::null(),
                0,
                evs.as_mut_ptr(),
                evs.len() as i32,
                std::ptr::null(),
            )
        };
        if n < 0 && errno::last() == errno::EINTR {
            continue;
        }
        if n < 0 {
            crate::diag!("inotify O_ASYNC watcher: errno {}", errno::last());
            return;
        }
        for e in &evs[..n as usize] {
            let ino = WATCHER
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|w| w.inotifies.get(&(e.udata as u64)))
                .and_then(Weak::upgrade);
            if let Some(ino) = ino {
                ino.arrived();
            }
        }
    }
}

/// Discard what kqueue `kq` reports.
fn drain(kq: i32) {
    // SAFETY: an all-zero kevent is valid.
    let mut evs: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: output array we own, no wait.
    while unsafe {
        libc::kevent(
            kq,
            std::ptr::null(),
            0,
            evs.as_mut_ptr(),
            evs.len() as i32,
            &zero,
        )
    } == evs.len() as i32
    {}
}

/// A private kqueue, out of the guest's way.
fn private_kqueue(source: Option<i32>) -> Result<i32, errno::Errno> {
    // SAFETY: a new kqueue, or our own reference to inotify kqueue `source`.
    let kq = unsafe {
        match source {
            Some(fd) => libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, fdtab::hidden_base()),
            None => libc::kqueue(),
        }
    };
    if kq < 0 {
        return Err(errno::last());
    }
    Ok(fdtab::hide(kq))
}

fn close_private(fd: i32) {
    fdtab::unhide(fd);
    // SAFETY: our private kqueue.
    unsafe { libc::close(fd) };
}

/// Register inotify `ino` (kqueue `fd`, state `s`) with the watcher
/// thread, started on first use.
fn arm(ino: &Arc<Inotify>, fd: i32, s: &State) -> Result<Armed, errno::Errno> {
    let kq = private_kqueue(Some(fd))?;
    let mirror = match private_kqueue(None) {
        Ok(mirror) => mirror,
        Err(error) => {
            close_private(kq);
            return Err(error);
        }
    };
    let mut watcher = WATCHER.lock().unwrap();
    let registered = (|| {
        for w in s.watches.values() {
            for fd in std::iter::once(w.fd).chain(w.entries.values().copied()) {
                if !vnode_note(mirror, fd, 0) {
                    return Err(errno::last());
                }
            }
        }
        if watcher.is_none() {
            let wkq = private_kqueue(None)?;
            std::thread::Builder::new()
                .name("linux-abi-inotify-async".into())
                .spawn(move || watch(wkq))
                .map_err(|_| errno::ENOMEM)?;
            *watcher = Some(Watcher {
                kq: wkq,
                inotifies: HashMap::new(),
                next: 0,
            });
        }
        let w = watcher.as_mut().unwrap();
        w.next += 1;
        let k = libc::kevent {
            ident: mirror as usize,
            filter: libc::EVFILT_READ,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: 0,
            data: 0,
            udata: w.next as usize as *mut _,
        };
        // SAFETY: one change, no events.
        if unsafe { libc::kevent(w.kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
            return Err(errno::last());
        }
        w.inotifies.insert(w.next, Arc::downgrade(ino));
        Ok(w.next)
    })();
    match registered {
        Ok(id) => Ok(Armed { kq, mirror, id }),
        Err(error) => {
            close_private(mirror);
            close_private(kq);
            Err(error)
        }
    }
}

/// Leave the watcher thread; closing the mirror drops its knotes.
fn disarm(armed: Armed) {
    if let Some(w) = WATCHER.lock().unwrap().as_mut() {
        w.inotifies.remove(&armed.id);
    }
    close_private(armed.mirror);
    close_private(armed.kq);
}

/// F_SETFL's O_ASYNC change (Linux `inotify_fasync`) on inotify `ino`
/// through guest fd `guest`, whose kqueue is `fd`.
pub fn set_async(ino: &Arc<Inotify>, fd: i32, guest: i32, on: bool) -> Result<(), errno::Errno> {
    let mut s = ino.state.lock().unwrap();
    match (on, s.fasync.is_some()) {
        (true, false) => {
            let armed = arm(ino, fd, &s)?;
            s.fasync = Some(Fasync {
                fd: guest,
                armed: Some(armed),
            });
        }
        (false, true) => {
            if let Some(armed) = s.fasync.take().and_then(|f| f.armed) {
                disarm(armed);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Fork: an inotify is its watches (each on an inherited vnode fd), its
/// queued events and its O_ASYNC state (Linux children share the file;
/// here each process has a copy, which signals the owner too).
pub(super) fn save(ino: &Inotify, w: &mut super::fork_state::Writer) {
    use std::os::unix::ffi::OsStrExt;
    let s = ino.state.lock().unwrap();
    w.seq(s.watches.iter(), |w, (wd, x)| {
        w.i32(*wd);
        w.retain_private(x.fd);
        w.i32(x.fd);
        w.u64(x.dev_ino.0);
        w.u64(x.dev_ino.1);
        w.u32(x.mask);
        w.bytes(std::ffi::OsStr::from_bytes(x.host.as_bytes()).as_bytes());
        w.opt(x.dir.as_ref(), |w, d| {
            w.seq(d.iter(), |w, (name, e)| {
                w.bytes(name);
                w.u64(e.0);
                w.i64(e.1);
                w.i64(e.2);
                w.bool(e.3);
            })
        });
        w.seq(x.entries.iter(), |w, (name, efd)| {
            w.bytes(name);
            w.retain_private(*efd);
            w.i32(*efd);
        });
    });
    w.i32(s.next_wd);
    w.seq(s.queue.iter(), |w, e| w.bytes(e));
    s.owner.save(w);
    w.opt(s.fasync.as_ref(), |w, f| w.i32(f.fd));
}

pub(super) fn load(r: &mut super::fork_state::Reader) -> Arc<Inotify> {
    let watches = r.seq(|r| {
        let wd = r.i32();
        let fd = r.i32();
        let watch = Watch {
            fd,
            dev_ino: (r.u64(), r.u64()),
            mask: r.u32(),
            host: CString::new(r.bytes()).unwrap_or_default(),
            dir: r.opt(|r| {
                r.seq(|r| (r.bytes(), (r.u64(), r.i64(), r.i64(), r.bool())))
                    .into_iter()
                    .collect()
            }),
            entries: r.seq(|r| (r.bytes(), r.i32())).into_iter().collect(),
        };
        (wd, watch)
    });
    let next_wd = r.i32();
    let queue: VecDeque<Vec<u8>> = r.seq(|r| r.bytes()).into();
    let state = State {
        watches: watches.into_iter().collect(),
        next_wd,
        overflowed: queue.iter().any(|e| mask_of(e) == IN_Q_OVERFLOW),
        queue,
        owner: Fown::load(r),
        fasync: r.opt(|r| Fasync {
            fd: r.i32(),
            armed: None,
        }),
        raised: 0,
    };
    Arc::new(Inotify {
        state: Mutex::new(state),
    })
}

/// Fork child: rebuild each inotify kqueue on its fd number.
pub fn after_fork_child() -> Result<(), errno::Errno> {
    let mut made: Vec<(i32, Arc<Inotify>)> = Vec::new();
    for (fd, k) in fdtab::fds_where(|k| matches!(k, Kind::Inotify(_))) {
        let Kind::Inotify(ino) = k else { continue };
        if let Some((source, _)) = made.iter().find(|(_, other)| Arc::ptr_eq(other, &ino)) {
            fdtab::restore_kqueue(fd, *source)?;
            continue;
        }
        // SAFETY: the parent's fd number, freed by Darwin in the child.
        unsafe {
            let kq = libc::kqueue();
            if kq < 0 {
                return Err(errno::last());
            }
            let result = fdtab::restore_kqueue(fd, kq);
            if kq != fd {
                libc::close(kq);
            }
            result?;
        }
        let mut s = ino.state.lock().unwrap_or_else(|e| e.into_inner());
        for (wd, w) in &s.watches {
            if !register(fd, *wd, w.fd, None) {
                return Err(errno::last());
            }
            for efd in w.entries.values() {
                if !register_note(fd, *efd, entry_udata(*wd, *efd), None) {
                    return Err(errno::last());
                }
            }
        }
        if !s.queue.is_empty() && !signal_pending(fd, true) {
            return Err(errno::last());
        }
        if s.fasync.as_ref().is_some_and(|f| f.armed.is_none()) {
            let armed = arm(&ino, fd, &s)?;
            if let Some(f) = s.fasync.as_mut() {
                f.armed = Some(armed);
            }
        }
        drop(s);
        made.push((fd, ino));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fown;
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn restored_aliases_share_pending_readiness() {
        if fdtab::isolated_kernel_test(
            "sys::inotify::tests::restored_aliases_share_pending_readiness",
        ) {
            return;
        }
        fdtab::init();
        let fd = inotify_init1([O_NONBLOCK, 0, 0, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        let alias = super::super::fs::dup([fd as u64, 0, 0, 0, 0, 0]) as i32;
        assert!(alias >= 0);
        let Some(Kind::Inotify(ino)) = fdtab::get(fd) else {
            panic!("inotify")
        };
        ino.state
            .lock()
            .unwrap()
            .queue
            .push_back(event(1, IN_MODIFY, &[]));
        let null = std::fs::File::open("/dev/null").unwrap();
        for target in [fd, alias] {
            assert_eq!(unsafe { libc::dup2(null.as_raw_fd(), target) }, target);
        }
        after_fork_child().unwrap();
        let readable = |fd| {
            let mut poll = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            (unsafe { libc::poll(&mut poll, 1, 0) }) == 1
        };
        assert!(readable(fd) && readable(alias));
        let mut bytes = [0u8; 16];
        assert_eq!(
            read(alias, bytes.as_mut_ptr() as u64, bytes.len()),
            Some(16)
        );
        assert!(!readable(fd) && !readable(alias));
        for target in [fd, alias] {
            fdtab::close_owned_guest(target, false).unwrap();
        }
    }

    /// fsnotify_add_event: the queue holds max_queued_events, then one
    /// IN_Q_OVERFLOW with wd -1 for all that are dropped, and once reads
    /// make room the next events queue behind it; inotify_merge folds an
    /// event equal to the last one queued, but never into an IN_IGNORED.
    #[test]
    fn a_full_queue_ends_in_one_overflow_event() {
        let mut s = State::default();
        for wd in 0..MAX_QUEUED_EVENTS as i32 + 3 {
            s.push(event(wd, IN_MODIFY, b""));
        }
        let overflow = event(-1, IN_Q_OVERFLOW, b"");
        assert_eq!(s.queue.len(), MAX_QUEUED_EVENTS + 1);
        assert_eq!(s.queue.back(), Some(&overflow));
        // The overflow event counts toward the limit, as on Linux.
        s.pop();
        s.pop();
        s.push(event(1, IN_ATTRIB, b""));
        s.push(event(2, IN_ATTRIB, b""));
        assert_eq!(s.queue.len(), MAX_QUEUED_EVENTS);
        assert_eq!(s.queue[MAX_QUEUED_EVENTS - 2], overflow);
        assert_eq!(s.queue.back(), Some(&event(1, IN_ATTRIB, b"")));
        assert_eq!(s.queue.iter().filter(|e| **e == overflow).count(), 1);
        while s.queue.len() > 1 {
            s.pop();
        }
        assert!(!s.overflowed);

        let mut s = State::default();
        for _ in 0..3 {
            s.push(event(1, IN_MODIFY, b"name"));
        }
        s.push(event(1, IN_MODIFY, b"other"));
        s.push(event(1, IN_CLOSE_WRITE, b"other"));
        for _ in 0..2 {
            s.push(event(1, IN_IGNORED, b""));
        }
        assert_eq!(s.queue.len(), 5);
    }

    #[test]
    fn o_async_signals_the_owner_of_each_event() {
        if fdtab::isolated_kernel_test(
            "sys::inotify::tests::o_async_signals_the_owner_of_each_event",
        ) {
            return;
        }
        use super::super::signal::take_process;
        const F_GETFL: u64 = 3;
        const F_SETFL: u64 = 4;
        const O_ASYNC: u64 = 0o20000;
        const SIGIO: i32 = 29;
        const SIGRT: i32 = 35;
        fdtab::init();
        let (_vfs, view) = crate::vfs::test_view();
        std::fs::create_dir_all(view.join("data/async")).unwrap();
        let fcntl = |fd: i32, cmd, arg| super::super::fs::fcntl([fd as u64, cmd, arg, 0, 0, 0]);
        let take = |sig| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(info) = take_process(sig) {
                    return Some(info);
                }
                if std::time::Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let fd = inotify_init1([O_NONBLOCK, 0, 0, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        let dir = CString::new("/data/async").unwrap();
        assert!(inotify_add_watch([fd as u64, dir.as_ptr() as u64, IN_CREATE as u64, 0, 0, 0]) > 0);
        let pid = unsafe { libc::getpid() } as u64;
        assert_eq!(fcntl(fd, fown::F_SETOWN, pid), 0);
        assert_eq!(fcntl(fd, fown::F_GETOWN, 0), pid as i64);

        // Without O_ASYNC, events queue silently.
        std::fs::write(view.join("data/async/quiet"), b"").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(pending_bytes(fd), Some(32));
        assert!(take_process(SIGIO).is_none());
        let mut bytes = [0u8; 64];
        assert_eq!(read(fd, bytes.as_mut_ptr() as u64, bytes.len()), Some(32));

        // O_ASYNC: an event signals the owner with no read or poll.
        assert_eq!(fcntl(fd, F_SETFL, O_NONBLOCK | O_ASYNC), 0);
        assert_ne!(fcntl(fd, F_GETFL, 0) as u64 & O_ASYNC, 0);
        std::fs::write(view.join("data/async/loud"), b"").unwrap();
        let info = take(SIGIO).expect("SIGIO");
        assert_eq!(info.code, 0x80);

        // F_SETSIG: the signal carries POLL_IN, the band and the fd.
        assert_eq!(fcntl(fd, fown::F_SETSIG, SIGRT as u64), 0);
        assert_eq!(fcntl(fd, fown::F_GETSIG, 0), SIGRT as i64);
        std::fs::write(view.join("data/async/rt"), b"").unwrap();
        let info = take(SIGRT).expect("F_SETSIG signal");
        assert_eq!((info.code, info.fields[0]), (1, 0x41));
        assert_eq!(info.fields[1] as i32, fd);
        assert_eq!(read(fd, bytes.as_mut_ptr() as u64, bytes.len()), Some(64));

        // Clearing O_ASYNC stops the signals.
        assert_eq!(fcntl(fd, F_SETFL, O_NONBLOCK), 0);
        assert_eq!(fcntl(fd, F_GETFL, 0) as u64 & O_ASYNC, 0);
        std::fs::write(view.join("data/async/after"), b"").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(pending_bytes(fd), Some(32));
        assert!(take_process(SIGRT).is_none() && take_process(SIGIO).is_none());
        fdtab::close_owned_guest(fd, false).unwrap();
    }

    /// More notes than one kevent call returns, all fired before the
    /// watcher thread decodes: each still queues its event and signal.
    #[test]
    fn o_async_decodes_a_burst_beyond_one_kevent_batch() {
        if fdtab::isolated_kernel_test(
            "sys::inotify::tests::o_async_decodes_a_burst_beyond_one_kevent_batch",
        ) {
            return;
        }
        const F_SETFL: u64 = 4;
        const O_ASYNC: u64 = 0o20000;
        const SIGRT: i32 = 36;
        const FILES: usize = 40;
        fdtab::init();
        let (_vfs, view) = crate::vfs::test_view();
        std::fs::create_dir_all(view.join("data/burst")).unwrap();
        let fcntl = |fd: i32, cmd, arg| super::super::fs::fcntl([fd as u64, cmd, arg, 0, 0, 0]);
        let fd = inotify_init1([O_NONBLOCK, 0, 0, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        for i in 0..FILES {
            std::fs::write(view.join(format!("data/burst/{i}")), b"").unwrap();
            let path = CString::new(format!("/data/burst/{i}")).unwrap();
            let watch = [fd as u64, path.as_ptr() as u64, IN_MODIFY as u64, 0, 0, 0];
            assert!(inotify_add_watch(watch) > 0);
        }
        let pid = unsafe { libc::getpid() } as u64;
        assert_eq!(fcntl(fd, fown::F_SETOWN, pid), 0);
        assert_eq!(fcntl(fd, fown::F_SETSIG, SIGRT as u64), 0);
        assert_eq!(fcntl(fd, F_SETFL, O_NONBLOCK | O_ASYNC), 0);
        let Some(Kind::Inotify(ino)) = fdtab::get(fd) else {
            panic!("inotify")
        };
        // The watcher waits for the lock while every file is written.
        let held = ino.state.lock().unwrap();
        for i in 0..FILES {
            std::fs::write(view.join(format!("data/burst/{i}")), b"x").unwrap();
        }
        drop(held);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut signals = 0;
        while signals < FILES && std::time::Instant::now() < deadline {
            if super::super::signal::take_process(SIGRT).is_some() {
                signals += 1;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        assert_eq!(signals, FILES);
        assert_eq!(ino.state.lock().unwrap().queue.len(), FILES);
        drop(ino);
        fdtab::close_owned_guest(fd, false).unwrap();
    }
}
