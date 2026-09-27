//! Guest fds whose Linux behaviour is more than the Darwin object behind
//! them.
//!
//! Every guest fd is a real host fd, so dup, fork, close-on-exec and
//! descriptor passing keep working. Some need Linux semantics on top: an
//! eventfd is a datagram socketpair carrying counter records, a SEQPACKET
//! socket is a framed stream socket, epoll and inotify are kqueues. Those
//! fds are listed here and have their [`SLOW`] byte set, so the lean path in
//! `trampoline.S` sends read, write, pread, pwrite and close on them to Rust.
//!
//! Sockets carry a marker (the `SO_LINGER` time, meaningless while lingering
//! is off) so a SEQPACKET or datagram socket that arrives by `SCM_RIGHTS`,
//! across exec or from the binder driver is recognized by [`adopt`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use super::{epoll, event, inotify, knob, memfd, net};

/// Guest fds below this have a byte in [`SLOW`]; the lean path sends larger
/// fds to Rust.
pub const SLOW_FDS: usize = 65536;

/// Read by `trampoline.S`: nonzero sends read/write/pread/pwrite/close on
/// the fd to Rust.
pub static SLOW: [AtomicU8; SLOW_FDS] = [const { AtomicU8::new(0) }; SLOW_FDS];

#[derive(Clone)]
pub enum Kind {
    Event(Arc<event::EventFd>),
    Timer(Arc<event::TimerFd>),
    Sock(Arc<net::Sock>),
    Epoll(Arc<epoll::Epoll>),
    Memfd(memfd::Key),
    Inotify(Arc<inotify::Inotify>),
    /// A directory stream: host entries read once, or a synthesized
    /// `/proc`/`/sys` directory.
    Dir(Arc<Mutex<super::dir::DirStream>>),
    /// A kernel file whose writes act (`knob`).
    Knob(Arc<knob::Knob>),
}

static TABLE: LazyLock<RwLock<HashMap<i32, Kind>>> = LazyLock::new(Default::default);

fn set_slow(fd: i32, on: bool) {
    if let Some(b) = SLOW.get(fd as usize) {
        b.store(on as u8, Ordering::Relaxed);
    }
}

pub fn insert(fd: i32, kind: Kind) {
    TABLE.write().unwrap().insert(fd, kind);
    set_slow(fd, true);
}

pub fn get(fd: i32) -> Option<Kind> {
    if SLOW
        .get(fd as usize)
        .is_some_and(|b| b.load(Ordering::Relaxed) == 0)
    {
        return None;
    }
    TABLE.read().unwrap().get(&fd).cloned()
}

/// The guest closed `fd` (or is about to replace it with dup2).
pub fn on_close(fd: i32) {
    if get(fd).is_none() {
        return;
    }
    set_slow(fd, false);
    TABLE.write().unwrap().remove(&fd);
}

/// `new` now refers to the same open file as `old`.
pub fn on_dup(old: i32, new: i32) {
    on_close(new);
    if let Some(k) = get(old) {
        insert(new, k);
    }
}

/// Give the sockets inherited across exec their Linux state.
fn adopt_inherited() {
    for fd in open_fds() {
        net::adopt(fd);
    }
}

/// The open fds of this process.
pub fn open_fds() -> Vec<i32> {
    // SAFETY: sizing call, then a buffer of that size.
    unsafe {
        let pid = libc::getpid();
        let n = libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        let sz = std::mem::size_of::<libc::proc_fdinfo>();
        let mut v: Vec<libc::proc_fdinfo> = Vec::with_capacity(n as usize / sz + 16);
        let n = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            v.as_mut_ptr().cast(),
            (v.capacity() * sz) as i32,
        );
        if n <= 0 {
            return Vec::new();
        }
        v.set_len(n as usize / sz);
        v.iter().map(|f| f.proc_fd).collect()
    }
}

/// Link text for `/proc/self/fd/N` of an fd with no path.
pub fn anon_name(fd: i32) -> Option<String> {
    Some(
        match get(fd)? {
            Kind::Event(_) => "anon_inode:[eventfd]",
            Kind::Timer(_) => "anon_inode:[timerfd]",
            Kind::Epoll(_) => "anon_inode:[eventpoll]",
            Kind::Inotify(_) => "anon_inode:inotify",
            Kind::Sock(_) | Kind::Dir(_) | Kind::Memfd(_) | Kind::Knob(_) => return None,
        }
        .to_string(),
    )
}

/// The pid whose fork-child fixups ran last.
static FIXED_FOR: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// In a fork child, before guest code runs: Darwin gives the child no
/// kqueues and no threads, so epoll and inotify fds are rebuilt on their
/// numbers and the timerfd thread restarted. Idempotent per process, so the
/// fork path may call it besides the `pthread_atfork` handler.
pub fn after_fork_child() {
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    if FIXED_FOR.swap(pid, Ordering::Relaxed) == pid {
        return;
    }
    epoll::after_fork_child();
    inotify::after_fork_child();
    event::after_fork_child();
}

extern "C" fn atfork_child() {
    after_fork_child();
}

/// Set up the table for this process: recognize inherited sockets and
/// rebuild kqueue-backed fds in fork children (Darwin does not inherit
/// kqueues).
pub fn init() {
    // SAFETY: trivial.
    FIXED_FOR.store(unsafe { libc::getpid() }, Ordering::Relaxed);
    adopt_inherited();
    // SAFETY: registering a plain C callback.
    unsafe { libc::pthread_atfork(None, None, Some(atfork_child)) };
}

/// Every fd in the table whose kind matches `f`.
pub fn fds_where(f: impl Fn(&Kind) -> bool) -> Vec<(i32, Kind)> {
    TABLE
        .read()
        .unwrap()
        .iter()
        .filter(|(_, k)| f(k))
        .map(|(fd, k)| (*fd, k.clone()))
        .collect()
}

/// Whether the open file behind `fd` is in non-blocking mode.
pub fn nonblocking(fd: i32) -> bool {
    // SAFETY: plain fcntl.
    unsafe { libc::fcntl(fd, libc::F_GETFL) & libc::O_NONBLOCK != 0 }
}

/// Set O_NONBLOCK and FD_CLOEXEC on a host fd as requested.
pub fn set_flags(fd: i32, nonblock: bool, cloexec: bool) {
    // SAFETY: plain fcntl on our fd.
    unsafe {
        if nonblock {
            let fl = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        if cloexec {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// Block until `fd` has one of `events` (host poll bits). Returns 0, or a
/// negative Linux errno (EINTR when a signal arrived).
pub fn wait_for(fd: i32, events: i16) -> i64 {
    let mut p = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one pollfd on our stack.
    if unsafe { libc::poll(&mut p, 1, -1) } < 0 {
        return -(crate::errno::last() as i64);
    }
    0
}

static HIDDEN: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Move a descriptor the layer keeps for itself (an eventfd's peer end)
/// high up, out of the guest's way, and leave it out of `/proc/self/fd`.
pub fn hide(fd: i32) -> i32 {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: plain getrlimit into a local.
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) };
    let base = (lim.rlim_cur.min(1 << 20) * 3 / 4).max(64) as i32;
    // SAFETY: duplicating our own fd, then closing the original.
    let high = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, base) };
    let fd = if high >= 0 {
        unsafe { libc::close(fd) };
        high
    } else {
        fd
    };
    HIDDEN.lock().unwrap().push(fd);
    fd
}

pub fn unhide(fd: i32) {
    HIDDEN.lock().unwrap().retain(|&h| h != fd);
}

pub fn is_hidden(fd: i32) -> bool {
    HIDDEN.lock().unwrap().contains(&fd)
}
