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
//! is off), and memfds marker flags, so a SEQPACKET or datagram socket or a
//! memfd that arrives by `SCM_RIGHTS`, across exec or from the binder driver
//! is recognized by [`adopt`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use super::{epoll, evdev, event, inotify, knob, memfd, net, random, sync_file};

/// Guest fds below this have a byte in [`SLOW`]; the lean path sends larger
/// fds to Rust.
pub const SLOW_FDS: usize = 65536;

/// Read by `trampoline.S`: nonzero sends read/write/pread/pwrite/close on
/// the fd to Rust.
pub static SLOW: [AtomicU8; SLOW_FDS] = [const { AtomicU8::new(0) }; SLOW_FDS];

#[derive(Clone)]
pub enum Kind {
    ProxyFile,
    Event(Arc<event::EventFd>),
    Timer(Arc<event::TimerFd>),
    Sock(Arc<net::Sock>),
    Epoll(Arc<epoll::Epoll>),
    Memfd(memfd::Key),
    Inotify(Arc<inotify::Inotify>),
    /// A directory stream: host entries read once, or a synthesized
    /// `/proc`/`/sys` directory.
    Dir(Arc<Mutex<super::dir::DirStream>>),
    /// A synthesized kernel file: an unlinked file holding its contents
    /// (`procfs::content_fd`).
    Content,
    /// A kernel file whose writes act (`knob`); also a content file.
    Knob(Arc<knob::Knob>),
    /// An open evdev device (`/dev/input/eventN`).
    Evdev(Arc<evdev::Evdev>),
    /// A fence (`sync_file`); its state is the host socket's.
    SyncFile,
    /// `/dev/random` or `/dev/urandom` open for writing (`random`).
    Random,
    /// A binder device file; the daemon holds its state.
    Binder(aim_binder_host::client::BinderFile),
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
    super::fuse_client::close(fd);
    if super::fuse_device::is_device(fd){set_slow(fd,false);}
    match get(fd) {
        None => return,
        Some(Kind::Content | Kind::Knob(_)) => super::procfs::recycle(fd),
        Some(_) => {}
    }
    set_slow(fd, false);
    TABLE.write().unwrap().remove(&fd);
}

/// `new` now refers to the same open file as `old`.
pub fn on_dup(old: i32, new: i32) {
    on_close(new);
    super::fuse_client::dup(old,new);
    if super::fuse_device::is_device(old){super::fuse_device::adopt(new);}
    if let Some(k) = get(old) {
        insert(new, k);
    }
}

/// Recognize an fd that arrived from elsewhere (exec, `SCM_RIGHTS`,
/// binder): a socket or a memfd gets its Linux state.
pub fn adopt(fd: i32) {
    if super::fuse_device::adopt(fd){return;}
    if super::fuse_client::adopt(fd)==Ok(true){return;}
    if super::binder::file_class(fd) == Ok(aim_binder_host::proxy_file::CLASS) {
        insert(fd, Kind::ProxyFile);
        return;
    }
    adopt_untyped(fd);
}

pub(super) fn adopt_received(fd: i32) -> Result<(), i32> {
    if super::fuse_device::adopt(fd){return Ok(());}
    if super::fuse_client::adopt(fd)?{return Ok(());}
    match super::binder::file_class(fd)? {
        0 => adopt_untyped(fd),
        aim_binder_host::proxy_file::CLASS => insert(fd, Kind::ProxyFile),
        _ => return Err(71),
    }
    Ok(())
}

pub(super) fn refresh_capabilities() -> Result<(), i32> {
    for fd in open_fds() {
        if super::binder::file_class(fd)? == aim_binder_host::proxy_file::CLASS {
            insert(fd, Kind::ProxyFile);
        }
    }
    Ok(())
}

pub(super) fn adopt_untyped(fd: i32) {
    if super::fuse_device::adopt(fd){return;}
    if super::fuse_client::adopt(fd)==Ok(true){return;}
    net::adopt(fd);
    memfd::adopt(fd);
    random::adopt(fd);
}

/// Give the fds inherited across exec their Linux state.
fn adopt_inherited() {
    for fd in open_fds() {
        adopt(fd);
    }
}

/// execve in place: the kept fds this process holds as plain fds (a fork
/// child's evdev fds, say) get their Linux state, as in a new process.
pub fn adopt_plain() {
    for fd in open_fds()
        .into_iter()
        .filter(|&fd| get(fd).is_none() && !is_hidden(fd))
    {
        adopt(fd);
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
            Kind::SyncFile => "anon_inode:sync_file",
            Kind::Evdev(e) => return Some(e.path()),
            Kind::Sock(_)
            | Kind::Dir(_)
            | Kind::Memfd(_)
            | Kind::Content
            | Kind::Knob(_)
            | Kind::Random
            | Kind::ProxyFile
            | Kind::Binder(_) => {
                return None;
            }
        }
        .to_string(),
    )
}

/// The fds of epolls and inotifies: kqueues, which a fork child makes
/// again on the same numbers.
pub fn kqueue_fds() -> Vec<i32> {
    fds_where(|k| matches!(k, Kind::Epoll(_) | Kind::Inotify(_)))
        .into_iter()
        .map(|(fd, _)| fd)
        .collect()
}

/// In a fork child, once the table is restored and before guest code
/// runs: kqueues are not inherited and the child has no threads, so epoll
/// and inotify fds are rebuilt on their numbers (held by placeholders until
/// now) and the timerfd thread restarted.
pub fn after_fork_child() {
    epoll::after_fork_child();
    inotify::after_fork_child();
    event::after_fork_child();
}

/// Set up the table for this process: recognize inherited fds.
pub fn init() {
    sync_file::init();
    adopt_inherited();
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
    let base = hidden_base();
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

/// Where the layer's own fds go: high up, but below `OPEN_MAX` (10240),
/// the highest fd `posix_spawn` can pass to a fork child
/// (`posix_spawn_file_actions_addinherit_np` refuses the rest with EBADF),
/// and within the descriptor table: F_DUPFD fails above it, as it does at
/// 3/4 of a soft RLIMIT_NOFILE beyond kern.maxfilesperproc.
pub fn hidden_base() -> i32 {
    // <sys/syslimits.h>.
    const OPEN_MAX: i32 = 10240;
    // SAFETY: plain getdtablesize.
    let table = unsafe { libc::getdtablesize() };
    (table.min(OPEN_MAX) * 3 / 4).max(64)
}

/// Leave `fd` (the layer's, kept across exec) out of the guest's view.
pub fn keep_hidden(fd: i32) {
    HIDDEN.lock().unwrap().push(fd);
}

pub fn unhide(fd: i32) {
    HIDDEN.lock().unwrap().retain(|&h| h != fd);
}

pub fn is_hidden(fd: i32) -> bool {
    HIDDEN.lock().unwrap().contains(&fd)
}

/// Fork: every fd's kind, with fds that share an object (dup'ed ones)
/// sharing it again in the child, and the layer's hidden fds. The fds
/// themselves are inherited. Content, knob, evdev and binder fds stay plain
/// fds: a content file is the parent's to reuse, a knob's action is code,
/// an input device is opened again, and a binder file is the parent's
/// process of the driver.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let table = TABLE.read().unwrap();
    let mut objects: Vec<*const ()> = Vec::new();
    let mut entries: Vec<(i32, &Kind, usize, bool)> = Vec::new();
    for (fd, k) in table.iter() {
        let p = match k {
            Kind::Event(a) => Arc::as_ptr(a) as *const (),
            Kind::Timer(a) => Arc::as_ptr(a) as *const (),
            Kind::Sock(a) => Arc::as_ptr(a) as *const (),
            Kind::Epoll(a) => Arc::as_ptr(a) as *const (),
            Kind::Inotify(a) => Arc::as_ptr(a) as *const (),
            Kind::Dir(a) => Arc::as_ptr(a) as *const (),
            Kind::Memfd(_) | Kind::SyncFile | Kind::Random | Kind::ProxyFile => std::ptr::null(),
            Kind::Content | Kind::Knob(_) | Kind::Evdev(_) | Kind::Binder(_) => continue,
        };
        let (i, new) = match objects.iter().position(|&o| !p.is_null() && o == p) {
            Some(i) => (i, false),
            None => {
                objects.push(p);
                (objects.len() - 1, true)
            }
        };
        entries.push((*fd, k, i, new));
    }
    // Objects first seen at a lower index come first in the child too.
    entries.sort_by_key(|e| (e.2, !e.3));
    w.seq(entries.into_iter(), |w, (fd, k, i, new)| {
        w.i32(fd);
        w.u64(i as u64);
        w.bool(new);
        if !new {
            return;
        }
        match k {
            Kind::Event(e) => {
                w.u32(0);
                event::save_event(e, w);
            }
            Kind::Timer(t) => {
                w.u32(1);
                event::save_timer(t, w);
            }
            Kind::Sock(s) => {
                w.u32(2);
                net::save_sock(s, w);
            }
            Kind::Epoll(e) => {
                w.u32(3);
                epoll::save(e, w);
            }
            Kind::Inotify(i) => {
                w.u32(4);
                inotify::save(i, w);
            }
            Kind::Dir(d) => {
                w.u32(5);
                super::dir::save(&d.lock().unwrap(), w);
            }
            Kind::Memfd(k) => {
                w.u32(6);
                w.u64(k.0);
                w.u64(k.1);
            }
            Kind::SyncFile => w.u32(7),
            Kind::Random => w.u32(8),
            Kind::ProxyFile => w.u32(9),
            Kind::Content | Kind::Knob(_) | Kind::Evdev(_) | Kind::Binder(_) => unreachable!(),
        }
    });
    w.seq(HIDDEN.lock().unwrap().iter(), |w, fd| w.i32(*fd));
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let mut objects: Vec<Kind> = Vec::new();
    let entries = r.seq(|r| {
        let fd = r.i32();
        let i = r.u64() as usize;
        if !r.bool() {
            return (fd, objects.get(i).cloned());
        }
        let k = match r.u32() {
            0 => Kind::Event(event::load_event(r)),
            1 => Kind::Timer(event::load_timer(r)),
            2 => Kind::Sock(net::load_sock(r)),
            3 => Kind::Epoll(epoll::load(r)),
            4 => Kind::Inotify(inotify::load(r)),
            5 => Kind::Dir(Arc::new(Mutex::new(super::dir::load(r)))),
            7 => Kind::SyncFile,
            8 => Kind::Random,
            9 => Kind::ProxyFile,
            _ => Kind::Memfd((r.u64(), r.u64())),
        };
        objects.push(k.clone());
        (fd, Some(k))
    });
    for (fd, k) in entries {
        if let Some(k) = k {
            insert(fd, k);
        }
    }
    keep_inherited_hidden(r.seq(|r| r.i32()));
    for fd in open_fds(){if super::fuse_device::adopt(fd){continue;}if !is_hidden(fd)&&super::fuse::marker(fd)==Some(super::fuse::FILE_MARKER){if let Err(error)=super::fuse_client::adopt(fd){set_slow(fd,true);eprintln!("inherited FUSE descriptor adoption failed: {error}");}}}
}

/// Fork child: hide the parent's hidden fds that came along, beside this
/// process's own. One that did not (a kqueue, such as the timer kqueue)
/// is a free number the guest may get.
fn keep_inherited_hidden(parent: Vec<i32>) {
    let mut hidden = HIDDEN.lock().unwrap();
    for fd in parent {
        // SAFETY: plain fcntl on a possibly closed fd.
        if !hidden.contains(&fd) && unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0 {
            hidden.push(fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        fn posix_spawn_file_actions_addinherit_np(
            actions: *mut libc::posix_spawn_file_actions_t,
            fd: libc::c_int,
        ) -> libc::c_int;
    }

    /// A hidden fd goes high even when the soft RLIMIT_NOFILE is above
    /// what the descriptor table can hold, and stays where `posix_spawn`
    /// can pass it to a fork child.
    #[test]
    fn a_hidden_fd_goes_high_and_stays_inheritable() {
        // SAFETY: raising this test process's soft limit to its hard one.
        unsafe {
            let mut lim: libc::rlimit = std::mem::zeroed();
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim);
            lim.rlim_cur = lim.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
        // SAFETY: a new kqueue of our own.
        let kq = unsafe { libc::kqueue() };
        let fd = hide(kq);
        assert!(fd >= hidden_base() && fd < 10240, "{fd}");
        // SAFETY: a file action list of our own, destroyed here.
        let r = unsafe {
            let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
            libc::posix_spawn_file_actions_init(&mut actions);
            let r = posix_spawn_file_actions_addinherit_np(&mut actions, fd);
            libc::posix_spawn_file_actions_destroy(&mut actions);
            r
        };
        assert_eq!(r, 0, "fd {fd} cannot be passed to a fork child");
        unhide(fd);
        // SAFETY: our fd.
        unsafe { libc::close(fd) };
    }

    /// A fork child hides the parent's hidden fds it inherited, not the
    /// numbers of those it did not, and keeps its own.
    #[test]
    fn a_fork_child_hides_only_inherited_fds() {
        // SAFETY: a new pipe of our own.
        let (own, inherited) = unsafe {
            let mut p = [0; 2];
            libc::pipe(p.as_mut_ptr());
            (p[0], p[1])
        };
        // Not open here, as a parent's kqueue is not in the child: a number
        // above any descriptor table, which no other test's fd can take.
        let gone = i32::MAX;
        keep_hidden(own);
        keep_inherited_hidden(vec![inherited, gone]);
        assert!(is_hidden(own) && is_hidden(inherited) && !is_hidden(gone));
        for fd in [own, inherited] {
            unhide(fd);
            // SAFETY: our fds.
            unsafe { libc::close(fd) };
        }
    }
}
