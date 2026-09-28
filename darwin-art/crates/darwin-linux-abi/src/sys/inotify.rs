//! inotify over kqueue EVFILT_VNODE.
//!
//! An inotify fd is a kqueue with one vnode knote per watch. Directory
//! watches keep a snapshot of their entries; a vnode event rescans the
//! directory and turns the differences into IN_CREATE, IN_DELETE and (for
//! entries whose size or mtime changed) IN_MODIFY + IN_CLOSE_WRITE. A file
//! watch reports IN_MODIFY/IN_CLOSE_WRITE on writes. Self events
//! (IN_ATTRIB, IN_DELETE_SELF, IN_MOVE_SELF, IN_IGNORED) come from the
//! knote. Rename pairing (IN_MOVED_FROM/TO cookies) is not reported: a
//! rename inside a watched directory shows as delete + create.
//!
//! Events are decoded into a queue when read; while the queue is not
//! empty, a user event keeps the kqueue readable for poll and epoll.

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::sync::{Arc, Mutex};

use super::fdtab::{self, Kind};
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
const IN_IGNORED: u32 = 0x8000;
const IN_ISDIR: u32 = 0x4000_0000;
const IN_ONESHOT: u32 = 0x8000_0000;
const IN_ONLYDIR: u32 = 0x0100_0000;
const IN_DONT_FOLLOW: u32 = 0x0200_0000;
const IN_MASK_ADD: u32 = 0x2000_0000;
const IN_ALL_EVENTS: u32 = 0xfff;

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
}

#[derive(Default)]
struct State {
    watches: HashMap<i32, Watch>,
    next_wd: i32,
    queue: VecDeque<Vec<u8>>,
}

pub struct Inotify {
    cloexec: bool,
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

fn register(kq: i32, wd: i32, fd: i32) -> bool {
    let k = libc::kevent {
        ident: fd as usize,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: VNODE_NOTES,
        data: 0,
        udata: wd as usize as *mut _,
    };
    // SAFETY: one change, no events.
    unsafe { libc::kevent(kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
}

/// Keep the kqueue readable while decoded events wait in the queue.
fn signal_pending(kq: i32, on: bool) {
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
    unsafe { libc::kevent(kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
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
    let cloexec = flags & O_CLOEXEC != 0;
    fdtab::set_flags(kq, flags & O_NONBLOCK != 0, cloexec);
    fdtab::insert(
        kq,
        Kind::Inotify(Arc::new(Inotify {
            cloexec,
            state: Mutex::new(State {
                next_wd: 1,
                ..Default::default()
            }),
        })),
    );
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
    let path = unsafe { guest_cstr(a[1]) };
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
    if let Some((wd, w)) = s.watches.iter_mut().find(|(_, w)| w.dev_ino == dev_ino) {
        w.mask = if mask & IN_MASK_ADD != 0 {
            w.mask | mask
        } else {
            mask
        };
        return *wd as i64;
    }
    // SAFETY: an event-only open of the watched path.
    let wfd = unsafe { libc::open(r.host.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
    if wfd < 0 {
        return -(errno::last() as i64);
    }
    let wd = s.next_wd;
    if !register(fd, wd, wfd) {
        // SAFETY: our watch fd.
        unsafe { libc::close(wfd) };
        return -(errno::last() as i64);
    }
    s.next_wd += 1;
    let dir = is_dir.then(|| snapshot(&r.host));
    s.watches.insert(
        wd,
        Watch {
            fd: wfd,
            dev_ino,
            mask,
            host: r.host,
            dir,
        },
    );
    wd as i64
}

fn remove(fd: i32, s: &mut State, wd: i32) -> bool {
    let Some(w) = s.watches.remove(&wd) else {
        return false;
    };
    // SAFETY: closing the watch fd drops its knote.
    unsafe { libc::close(w.fd) };
    s.queue.push_back(event(wd, IN_IGNORED, b""));
    signal_pending(fd, true);
    true
}

pub fn inotify_rm_watch(a: [u64; 6]) -> i64 {
    let (fd, wd) = (a[0] as i32, a[1] as i32);
    let ino = match inotify_of(fd) {
        Ok(i) => i,
        Err(e) => return e,
    };
    if remove(fd, &mut ino.state.lock().unwrap(), wd) {
        0
    } else {
        -(EINVAL as i64)
    }
}

/// Decode what the knote of watch `wd` reported.
fn decode(fd: i32, s: &mut State, wd: i32, notes: u32) {
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
                    None => out.push((IN_CREATE | isdir, name.clone())),
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
            s.queue.push_back(event(wd, m, &name));
            fired = true;
        }
    }
    if gone || (fired && mask & IN_ONESHOT != 0) {
        remove(fd, s, wd);
    }
}

/// Move pending knote events into the queue; wait for one when `block`.
fn collect(fd: i32, s: &mut State, block: bool) -> i64 {
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
        if e.filter == libc::EVFILT_VNODE {
            decode(fd, s, e.udata as usize as i32, e.fflags);
        }
    }
    0
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
            return Some(r);
        }
        if !s.queue.is_empty() {
            break;
        }
        if fdtab::nonblocking(fd) {
            signal_pending(fd, false);
            return Some(-(EAGAIN as i64));
        }
        drop(s);
        let r = fdtab::wait_for(fd, libc::POLLIN);
        s = ino.state.lock().unwrap();
        if r < 0 {
            return Some(r);
        }
    }
    let mut off = 0usize;
    while let Some(e) = s.queue.front() {
        if off + e.len() > len {
            break;
        }
        // SAFETY: guest buffer of `len` bytes.
        unsafe { std::ptr::copy_nonoverlapping(e.as_ptr(), (buf as *mut u8).add(off), e.len()) };
        off += e.len();
        s.queue.pop_front();
    }
    signal_pending(fd, !s.queue.is_empty());
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
    Some(s.queue.iter().map(|e| e.len() as i64).sum())
}

/// Whether `fd`, reported readable by the host, is an inotify fd with no
/// event to read. Its kqueue is readable on any vnode note, and a note may
/// decode to nothing (a rescan that finds what an earlier one reported);
/// Linux is readable only while events are queued, and a blocking read
/// after such a wakeup would never return.
pub fn spuriously_ready(fd: i32) -> bool {
    pending_bytes(fd) == Some(0)
}

/// Fork: an inotify is its watches (each on an inherited vnode fd) and its
/// queued events.
pub(super) fn save(ino: &Inotify, w: &mut super::fork_state::Writer) {
    use std::os::unix::ffi::OsStrExt;
    w.bool(ino.cloexec);
    let s = ino.state.lock().unwrap();
    w.seq(s.watches.iter(), |w, (wd, x)| {
        w.i32(*wd);
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
    });
    w.i32(s.next_wd);
    w.seq(s.queue.iter(), |w, e| w.bytes(e));
}

pub(super) fn load(r: &mut super::fork_state::Reader) -> Arc<Inotify> {
    let cloexec = r.bool();
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
        };
        (wd, watch)
    });
    let state = State {
        watches: watches.into_iter().collect(),
        next_wd: r.i32(),
        queue: r.seq(|r| r.bytes()).into(),
    };
    Arc::new(Inotify {
        cloexec,
        state: Mutex::new(state),
    })
}

/// Fork child: rebuild each inotify kqueue on its fd number.
pub fn after_fork_child() {
    for (fd, k) in fdtab::fds_where(|k| matches!(k, Kind::Inotify(_))) {
        let Kind::Inotify(ino) = k else { continue };
        // SAFETY: the parent's fd number, freed by Darwin in the child.
        unsafe {
            let kq = libc::kqueue();
            if kq < 0 {
                continue;
            }
            if kq != fd {
                libc::dup2(kq, fd);
                libc::close(kq);
            }
        }
        fdtab::set_flags(fd, false, ino.cloexec);
        let s = ino.state.lock().unwrap_or_else(|e| e.into_inner());
        for (wd, w) in &s.watches {
            register(fd, *wd, w.fd);
        }
        if !s.queue.is_empty() {
            signal_pending(fd, true);
        }
    }
}
