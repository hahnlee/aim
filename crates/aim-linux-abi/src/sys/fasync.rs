//! Signal-driven I/O (O_ASYNC) that this layer delivers: the watcher
//! thread, and the owners Darwin's fown cannot hold.
//!
//! Darwin keeps a host file's O_ASYNC flag and its group owner, shared by
//! every descriptor of the file as Linux's are, and raises SIGIO itself;
//! a process owner it holds only for a socket (its other files take the
//! process's group in its place). It has no F_SETSIG and no thread owner
//! (F_SETOWN_EX F_OWNER_TID), and its SIGIO does not say which descriptor
//! raised it. A host file with a Linux fasync (a pipe, socket or tty) and
//! an owner Darwin cannot hold keeps it here instead, Darwin's is cleared,
//! and while O_ASYNC is set a private kqueue holds EV_CLEAR read and write
//! knotes on a private reference to the file: a wakeup of the file wakes
//! the watcher thread, which signals the owner as the Linux driver's
//! `kill_fasync` would (POLL_IN, POLL_OUT or POLL_HUP, see [`Fasync`]),
//! with the descriptor that set O_ASYNC as `si_fd`. The knotes report the
//! file's own wakeups, where a readiness check on SIGIO would signal every
//! ready descriptor again; wakeups before the watcher reads them are one
//! signal each way, where Linux queues one real-time signal each. A
//! socket's or tty's write knote wakes on every freeing of space, where
//! Linux sends POLL_OUT only once a write found the buffer full
//! (SOCKWQ_ASYNC_NOSPACE, TTY_DO_WRITE_WAKEUP).
//! The private kqueue is drained once the knotes are in it, so readiness
//! that is already there raises nothing, as on Linux. The registration
//! lasts until the last guest fd of the file known here closes (Linux
//! removes the fasync entry at the last reference), and a fork child gets
//! a copy of each owner and registration, as an inotify's; the copies
//! elect one process to signal each event ([`Elect`]).
//!
//! An evdev (its socket is the layer's transport) and a pty (Darwin's
//! F_SETOWN of a tty sets its foreground group, and the description's
//! O_ASYNC is the pty keeper's) are signalled the same way, with their
//! owner always kept here.
//!
//! The descriptors this layer implements whose Linux files have no fasync
//! (eventfd, timerfd, signalfd, epoll, pidfd, ...) and host files without
//! one (a regular file, a memfd) keep their owner here too, never
//! signalled: Darwin's stand-in for them would raise SIGIO.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use super::fdtab;
use super::fown::{F_GETOWN, F_GETOWN_EX, F_GETSIG, F_SETOWN, F_SETOWN_EX, F_SETSIG, Fown};
use crate::errno::{self, Errno};

/// What the watcher thread wakes when its kqueue reports.
pub trait Arrival: Send + Sync {
    fn arrived(&self);
}

/// The watcher thread's kqueue (an EV_CLEAR read knote on each registered
/// kqueue, with its id), and the targets by id.
struct Watcher {
    kq: i32,
    targets: HashMap<u64, Weak<dyn Arrival>>,
    next: u64,
}

static WATCHER: Mutex<Option<Watcher>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

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
            crate::diag!("O_ASYNC watcher: errno {}", errno::last());
            return;
        }
        for e in &evs[..n as usize] {
            let target = lock(&WATCHER)
                .as_ref()
                .and_then(|w| w.targets.get(&(e.udata as u64)))
                .and_then(Weak::upgrade);
            if let Some(target) = target {
                target.arrived();
            }
        }
    }
}

/// Have the watcher thread, started on first use, wake `target` whenever
/// private kqueue `kq` becomes readable; the registration's id.
pub fn register(kq: i32, target: Weak<dyn Arrival>) -> Result<u64, Errno> {
    let mut watcher = lock(&WATCHER);
    if watcher.is_none() {
        let wkq = private_kqueue(None)?;
        std::thread::Builder::new()
            .name("linux-abi-fasync".into())
            .spawn(move || watch(wkq))
            .map_err(|_| errno::ENOMEM)?;
        *watcher = Some(Watcher {
            kq: wkq,
            targets: HashMap::new(),
            next: 0,
        });
    }
    let w = watcher.as_mut().unwrap();
    w.next += 1;
    let k = libc::kevent {
        ident: kq as usize,
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
    w.targets.insert(w.next, target);
    Ok(w.next)
}

/// Leave the watcher thread; closing the registered kqueue drops its knote.
pub fn unregister(id: u64) {
    if let Some(w) = lock(&WATCHER).as_mut() {
        w.targets.remove(&id);
    }
}

/// The events a host file's private kqueue `kq` (its read and write
/// knotes) holds now.
fn knotes(kq: i32) -> Vec<libc::kevent> {
    // SAFETY: an all-zero kevent is valid.
    let mut evs: [libc::kevent; 2] = unsafe { std::mem::zeroed() };
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: output array we own, no wait.
    let n = unsafe { libc::kevent(kq, std::ptr::null(), 0, evs.as_mut_ptr(), 2, &zero) };
    evs[..n.max(0) as usize].to_vec()
}

/// Discard what kqueue `kq` reports.
pub fn drain(kq: i32) {
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

/// A private kqueue, out of the guest's way: a new one, or a reference to
/// the file of descriptor `source`.
pub fn private_kqueue(source: Option<i32>) -> Result<i32, Errno> {
    // SAFETY: a new kqueue, or our own reference to kqueue `source`.
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

pub fn close_private(fd: i32) {
    fdtab::unhide(fd);
    // SAFETY: our private kqueue.
    unsafe { libc::close(fd) };
}

/// The one process that signals for an O_ASYNC registration that a fork
/// child copied: Linux has one fasync entry per open file, here each
/// process holding the file has a copy and is woken for the same event.
/// The copies share a private file, and the process that holds, or now
/// takes, an exclusive POSIX record lock on it signals; Darwin gives the
/// lock to one process at a time and drops it when that process closes
/// the file or exits, so another copy takes over.
pub struct Elect(i32);

impl Elect {
    pub fn new() -> Result<Elect, Errno> {
        let mut tmpl = std::env::temp_dir()
            .join("linux-abi-fasync.XXXXXX")
            .into_os_string()
            .into_encoded_bytes();
        tmpl.push(0);
        // SAFETY: mkstemp fills in the template; the name goes at once.
        let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
        if fd < 0 {
            return Err(errno::last());
        }
        // SAFETY: our new file's name and descriptor.
        unsafe {
            libc::unlink(tmpl.as_ptr().cast());
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        Ok(Elect(fdtab::hide(fd)))
    }

    /// Whether this process signals the event at hand.
    pub fn won(&self) -> bool {
        // SAFETY: an all-zero flock is valid.
        let mut whole: libc::flock = unsafe { std::mem::zeroed() };
        whole.l_type = libc::F_WRLCK as i16;
        whole.l_whence = libc::SEEK_SET as i16;
        // SAFETY: a lock request on our private file.
        if unsafe { libc::fcntl(self.0, libc::F_SETLK, &whole) } == 0 {
            return true;
        }
        let error = errno::last();
        if error == errno::EAGAIN || error == errno::EACCES {
            return false;
        }
        // Signalling twice is safer than not at all.
        crate::diag!("O_ASYNC election: errno {error}");
        true
    }

    /// Fork: the child shares the private file.
    pub(super) fn save(&self, w: &mut super::fork_state::Writer) {
        w.retain_private(self.0);
        w.i32(self.0);
    }

    pub(super) fn load(r: &mut super::fork_state::Reader) -> Elect {
        Elect(r.i32())
    }
}

impl Drop for Elect {
    fn drop(&mut self) {
        close_private(self.0);
    }
}

/// The owner of an open file that is kept here, shared by its dups.
struct File {
    fown: Fown,
    source: Source,
    armed: Mutex<Option<Armed>>,
}

/// Who holds and signals the owner of an open file.
#[derive(Clone, Copy)]
pub struct Source {
    /// The Linux fasync of the file, which the watcher delivers while
    /// Darwin cannot; None for a file without one, whose owner is kept
    /// here and never signalled.
    fasync: Option<Fasync>,
    /// The host file is the Linux file, whose fown Darwin holds when it
    /// can; else Darwin's fown of it is never used.
    darwin: bool,
}

impl Source {
    /// The source of the open file behind host descriptor `fd`, of `kind`.
    pub fn of(fd: i32, kind: Option<&fdtab::Kind>) -> Result<Source, Errno> {
        use fdtab::Kind;
        let (fasync, darwin) = match kind {
            // `evdev_fasync`; its socket is the layer's transport.
            Some(Kind::Evdev(_)) => (Some(Fasync::Evdev), false),
            // `tty_fasync`; Darwin's F_SETOWN of a tty would set its
            // foreground group, and the description's O_ASYNC is the pty
            // keeper's.
            Some(Kind::Pty(_)) => (Some(Fasync::Tty), false),
            None | Some(Kind::Sock(_) | Kind::Regular(_) | Kind::Memfd(_))
                if super::fuse_client::get(fd).is_none() =>
            {
                (Fasync::of(fd)?, true)
            }
            _ => (None, false),
        };
        Ok(Source { fasync, darwin })
    }

    /// Whether the file has a Linux fasync, which FIOASYNC may change.
    pub fn has_fasync(&self) -> bool {
        self.fasync.is_some()
    }

    /// Darwin's owner of the file for `owner` when Darwin holds it.
    fn darwin_owner(&self, owner: &super::fown::Owner) -> Option<i32> {
        match self.fasync {
            Some(fasync) if self.darwin => owner.host(fasync == Fasync::Socket),
            _ => None,
        }
    }

    fn save(&self, w: &mut super::fork_state::Writer) {
        w.opt(self.fasync.as_ref(), |w, fasync| w.u32(*fasync as u32));
        w.bool(self.darwin);
    }

    fn load(r: &mut super::fork_state::Reader) -> Source {
        let fasync = r.opt(|r| match r.u32() {
            0 => Fasync::Pipe,
            1 => Fasync::Socket,
            2 => Fasync::Tty,
            _ => Fasync::Evdev,
        });
        Source {
            fasync,
            darwin: r.bool(),
        }
    }
}

/// The files with a Linux fasync, by what their wakeups signal.
#[derive(Clone, Copy, PartialEq)]
enum Fasync {
    /// `pipe_fasync`: POLL_IN to readers on a write, POLL_OUT to writers
    /// on a read, and the same when the other side's last end closes.
    Pipe,
    /// `sock_fasync`: POLL_IN on data, POLL_OUT on write space, POLL_HUP
    /// when the peer is gone (`sk_wake_async`).
    Socket,
    /// `tty_fasync`: POLL_IN on input, POLL_OUT on write space; a hangup
    /// drops the registration without a signal.
    Tty,
    /// `evdev_fasync`: POLL_IN on events (`evdev_pass_values`), POLL_HUP
    /// when the device goes away (`evdev_hangup`).
    Evdev,
}

impl Fasync {
    /// The fasync of host descriptor `fd`; None for a file without one (a
    /// regular file, most devices).
    fn of(fd: i32) -> Result<Option<Fasync>, Errno> {
        // SAFETY: an all-zero stat is valid; fstat fills it.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: plain fstat into a local.
        if unsafe { libc::fstat(fd, &mut st) } < 0 {
            return Err(errno::last());
        }
        Ok(match st.st_mode & libc::S_IFMT {
            libc::S_IFIFO => Some(Fasync::Pipe),
            libc::S_IFSOCK => Some(Fasync::Socket),
            // SAFETY: plain isatty.
            libc::S_IFCHR if unsafe { libc::isatty(fd) } == 1 => Some(Fasync::Tty),
            _ => None,
        })
    }

    /// The reason (`signal::POLL_*`) that a knote's event signals, if any,
    /// for a file whose write side last had `space` free.
    fn reason(self, held: i32, space: &mut isize, e: &libc::kevent) -> Option<i32> {
        use super::signal::{POLL_HUP, POLL_IN, POLL_OUT};
        let eof = e.flags & libc::EV_EOF != 0;
        let read = e.filter == libc::EVFILT_READ;
        match self {
            Fasync::Pipe if read => Some(POLL_IN),
            Fasync::Pipe => {
                let freed = e.data > *space;
                *space = e.data;
                (freed || eof).then_some(POLL_OUT)
            }
            Fasync::Evdev => Some(if eof { POLL_HUP } else { POLL_IN }),
            _ if !eof => Some(if read { POLL_IN } else { POLL_OUT }),
            // A peer gone for good is POLL_HUP; one that only shut down
            // its writing is POLL_IN (`unix_shutdown`, `tcp_fin`). Its
            // write side's EOF is no signal.
            Fasync::Socket if read => Some(if hung_up(held) { POLL_HUP } else { POLL_IN }),
            Fasync::Socket | Fasync::Tty => None,
        }
    }
}

/// Whether socket `fd` can neither receive nor send any more.
fn hung_up(fd: i32) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    // SAFETY: one local pollfd, no wait.
    let ready = unsafe { libc::poll(&mut p, 1, 0) };
    ready == 1 && p.revents & libc::POLLHUP != 0
}

/// A host file's O_ASYNC registration with the watcher thread.
struct Armed {
    /// The guest fd that set O_ASYNC (`fa_fd`).
    fd: i32,
    fasync: Fasync,
    /// A pipe writer's free space when last seen: Darwin also wakes it
    /// for its own writes, Linux only for reads.
    space: isize,
    /// The private reference to the file that the knote is on.
    held: i32,
    /// The private kqueue holding the knote.
    kq: i32,
    id: u64,
    elect: Elect,
}

impl Arrival for File {
    fn arrived(&self) {
        let (fd, reasons) = match lock(&self.armed).as_mut() {
            Some(armed) => {
                let reasons: Vec<i32> = knotes(armed.kq)
                    .iter()
                    .filter_map(|e| armed.fasync.reason(armed.held, &mut armed.space, e))
                    .collect();
                if reasons.is_empty() || !armed.elect.won() {
                    return;
                }
                (armed.fd, reasons)
            }
            None => return,
        };
        let owner = self.fown.owner();
        for reason in reasons {
            owner.notify(fd, reason);
        }
    }
}

impl File {
    fn disarm(&self) {
        if let Some(armed) = lock(&self.armed).take() {
            unregister(armed.id);
            close_private(armed.kq);
            close_private(armed.held);
        }
    }
}

/// The files kept here, by guest fd.
static FILES: Mutex<Option<HashMap<i32, Arc<File>>>> = Mutex::new(None);

fn file(fd: i32) -> Option<Arc<File>> {
    lock(&FILES).as_ref()?.get(&fd).cloned()
}

/// fcntl's owner commands (F_SETOWN, F_SETSIG, ...) on guest fd `guest`,
/// whose host descriptor is `fd` and whose file's owner `source` holds;
/// None for other commands.
pub fn fcntl(fd: i32, guest: i32, source: Source, cmd: u64, arg: u64) -> Option<i64> {
    if !matches!(
        cmd,
        F_SETOWN | F_GETOWN | F_SETOWN_EX | F_GETOWN_EX | F_SETSIG | F_GETSIG
    ) {
        return None;
    }
    // A host file without a Linux fasync keeps its owner here, unsignalled.
    let host = source.darwin && source.fasync.is_some();
    let (file, kept) = match file(guest) {
        Some(file) => (file, true),
        None if host
            && (cmd == F_GETOWN
                || cmd == F_SETOWN
                    && (arg as i32 <= 0 || source.fasync == Some(Fasync::Socket))) =>
        {
            return Some(host_fcntl(fd, cmd, arg));
        }
        None => {
            let fown = if host {
                match host_owner(fd) {
                    Ok(owner) => Fown::from_host(owner),
                    Err(error) => return Some(-(error as i64)),
                }
            } else {
                Fown::default()
            };
            let file = Arc::new(File {
                fown,
                source,
                armed: Mutex::new(None),
            });
            (file, false)
        }
    };
    let result = file.fown.fcntl(cmd, arg)?;
    if result != 0 || !matches!(cmd, F_SETOWN | F_SETOWN_EX | F_SETSIG) {
        return Some(result);
    }
    if !kept {
        lock(&FILES)
            .get_or_insert_default()
            .insert(guest, file.clone());
    }
    if source.fasync.is_some()
        && let Err(error) = project(fd, guest, &file)
    {
        return Some(-(error as i64));
    }
    Some(0)
}

/// Darwin's owner of host descriptor `fd`: a pid, or a negated group.
fn host_owner(fd: i32) -> Result<i32, Errno> {
    // SAFETY: plain fcntl; F_GETOWN's -1 can be a group, so errno decides.
    unsafe {
        *libc::__error() = 0;
        let owner = libc::fcntl(fd, libc::F_GETOWN);
        if owner == -1 && *libc::__error() != 0 {
            return Err(errno::last());
        }
        Ok(owner)
    }
}

/// F_SETOWN and F_GETOWN of a host file whose owner Darwin holds.
fn host_fcntl(fd: i32, cmd: u64, arg: u64) -> i64 {
    if cmd == F_GETOWN {
        return match host_owner(fd) {
            Ok(0) => 0,
            Ok(owner) => match super::pidns::guest_pid(owner.abs()) {
                Ok(pid) => (pid * owner.signum()) as i64,
                Err(error) => error,
            },
            Err(error) => -(error as i64),
        };
    }
    let target = match arg as i32 {
        0 => 0,
        i32::MIN => return -(errno::EINVAL as i64),
        pid => match super::pidns::syscall_pid(pid.abs()) {
            Ok(target) => target * pid.signum(),
            Err(error) => return error,
        },
    };
    // SAFETY: plain fcntl.
    errno::check(unsafe { libc::fcntl(fd, libc::F_SETOWN, target) } as i64)
}

/// After the owner changed: Darwin takes an owner it can signal, and the
/// file leaves this table; otherwise Darwin's is cleared and the watcher
/// delivers while O_ASYNC is set.
fn project(fd: i32, guest: i32, file: &Arc<File>) -> Result<(), Errno> {
    let host = file.source.darwin_owner(&file.fown.owner());
    // SAFETY: plain fcntl.
    if file.source.darwin && unsafe { libc::fcntl(fd, libc::F_SETOWN, host.unwrap_or(0)) } < 0 {
        return Err(errno::last());
    }
    if host.is_some() {
        if let Some(files) = lock(&FILES).as_mut() {
            files.retain(|_, other| !Arc::ptr_eq(other, file));
        }
        file.disarm();
        return Ok(());
    }
    refresh(fd, guest, file, None)
}

/// The status flags of host descriptor `fd`, as Darwin's F_GETFL gives
/// them: a pty's are its keeper's.
fn status(fd: i32) -> Result<i32, Errno> {
    if let Some(fdtab::Kind::Pty(description)) = fdtab::get(fd) {
        return Ok(super::fs::open_flags_to_host(description.flags()));
    }
    // SAFETY: plain fcntl.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(errno::last());
    }
    Ok(flags)
}

/// Arm or disarm file `file` (descriptor `fd`, guest fd `guest`) as its
/// O_ASYNC flag now says, sharing the election `elect` of a fork parent.
fn refresh(fd: i32, guest: i32, file: &Arc<File>, elect: Option<Elect>) -> Result<(), Errno> {
    let flags = status(fd)?;
    let fasync = match file.source.fasync {
        Some(fasync) if flags & libc::O_ASYNC != 0 => fasync,
        _ => {
            file.disarm();
            return Ok(());
        }
    };
    let mut armed = lock(&file.armed);
    if armed.is_some() {
        return Ok(());
    }
    let elect = match elect {
        Some(elect) => elect,
        None => Elect::new()?,
    };
    let held = private_kqueue(Some(fd))?;
    let kq = match private_kqueue(None) {
        Ok(kq) => kq,
        Err(error) => {
            close_private(held);
            return Err(error);
        }
    };
    let registered = (|| {
        let mode = flags & libc::O_ACCMODE;
        let filters = [
            (libc::EVFILT_READ, mode != libc::O_WRONLY),
            (
                libc::EVFILT_WRITE,
                mode != libc::O_RDONLY && fasync != Fasync::Evdev,
            ),
        ];
        for (filter, _) in filters.into_iter().filter(|(_, wanted)| *wanted) {
            let k = libc::kevent {
                ident: held as usize,
                filter,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: one change, no events.
            if unsafe { libc::kevent(kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
                return Err(errno::last());
            }
        }
        let space = knotes(kq)
            .iter()
            .find(|e| e.filter == libc::EVFILT_WRITE)
            .map_or(0, |e| e.data);
        let target: Weak<File> = Arc::downgrade(file);
        register(kq, target).map(|id| (id, space))
    })();
    match registered {
        Ok((id, space)) => {
            *armed = Some(Armed {
                fd: guest,
                fasync,
                space,
                held,
                kq,
                id,
                elect,
            });
            Ok(())
        }
        Err(error) => {
            close_private(kq);
            close_private(held);
            Err(error)
        }
    }
}

/// F_SETFL changed host descriptor `fd` (guest fd `guest`): a file kept
/// here follows its O_ASYNC flag.
pub fn status_changed(fd: i32, guest: i32) -> Result<(), Errno> {
    match file(guest) {
        Some(file)
            if file.source.fasync.is_some()
                && file.source.darwin_owner(&file.fown.owner()).is_none() =>
        {
            refresh(fd, guest, &file, None)
        }
        _ => Ok(()),
    }
}

/// Guest fd `fd` closed: it leaves the table, and its file's O_ASYNC
/// registration ends with the file's last guest fd here.
pub fn on_close(fd: i32) {
    let mut files = lock(&FILES);
    let Some(file) = files.as_mut().and_then(|files| files.remove(&fd)) else {
        return;
    };
    let last = !files
        .iter()
        .flat_map(|files| files.values())
        .any(|other| Arc::ptr_eq(other, &file));
    drop(files);
    if last {
        file.disarm();
    }
}

/// Guest fd `new` now refers to `old`'s open file.
pub fn on_dup(old: i32, new: i32) {
    let mut files = lock(&FILES);
    if let Some(files) = files.as_mut()
        && let Some(file) = files.get(&old).cloned()
    {
        files.insert(new, file);
    }
}

/// Fork: each file's guest fds, owner, kind and O_ASYNC registration's fd
/// (Linux children share the file; here each process has a copy, which
/// signals the owner too).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let files = lock(&FILES);
    let mut groups: Vec<(Vec<i32>, &Arc<File>)> = Vec::new();
    for (fd, file) in files.iter().flatten() {
        match groups
            .iter_mut()
            .find(|(_, other)| Arc::ptr_eq(other, file))
        {
            Some((fds, _)) => fds.push(*fd),
            None => groups.push((vec![*fd], file)),
        }
    }
    w.seq(groups.iter(), |w, (fds, file)| {
        w.seq(fds.iter(), |w, fd| w.i32(*fd));
        file.fown.save(w);
        file.source.save(w);
        w.opt(lock(&file.armed).as_ref(), |w, armed| {
            w.i32(armed.fd);
            armed.elect.save(w);
        });
    });
}

/// The registrations [`fork_restore`] read, armed by [`after_fork_child`].
static RESTORED: Mutex<Vec<(i32, i32, Arc<File>, Elect)>> = Mutex::new(Vec::new());

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let groups = r.seq(|r| {
        let fds = r.seq(|r| r.i32());
        let file = Arc::new(File {
            fown: Fown::load(r),
            source: Source::load(r),
            armed: Mutex::new(None),
        });
        (fds, file, r.opt(|r| (r.i32(), Elect::load(r))))
    });
    let mut files = lock(&FILES);
    let files = files.get_or_insert_default();
    for (fds, file, armed) in groups {
        if let (Some((si_fd, elect)), Some(fd)) = (armed, fds.first()) {
            lock(&RESTORED).push((*fd, si_fd, file.clone(), elect));
        }
        for fd in fds {
            files.insert(fd, file.clone());
        }
    }
}

/// Fork child: arm each registration again on its inherited file.
pub fn after_fork_child() -> Result<(), Errno> {
    for (fd, si_fd, file, elect) in std::mem::take(&mut *lock(&RESTORED)) {
        refresh(fd, si_fd, &file, Some(elect))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::sigframe::Siginfo;
    use super::super::signal::take_process;
    use super::super::{event, fs, net};
    use super::*;

    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;
    const O_ASYNC: u64 = 0o20000;
    const FIOASYNC: u64 = 0x5452;
    const SIGIO: i32 = 29;
    const ENOTTY: i64 = 25;

    fn fcntl(fd: i32, cmd: u64, arg: u64) -> i64 {
        fs::fcntl([fd as u64, cmd, arg, 0, 0, 0])
    }

    fn fioasync(fd: i32, on: i32) -> i64 {
        fs::ioctl([fd as u64, FIOASYNC, &on as *const i32 as u64, 0, 0, 0])
    }

    fn is_async(fd: i32) -> bool {
        fcntl(fd, F_GETFL, 0) as u64 & O_ASYNC != 0
    }

    fn put(fd: i32, bytes: &[u8]) {
        let r = fs::write([
            fd as u64,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
            0,
        ]);
        assert_eq!(r, bytes.len() as i64);
    }

    fn get(fd: i32) {
        let mut bytes = [0u8; 64];
        assert!(fs::read([fd as u64, bytes.as_mut_ptr() as u64, 64, 0, 0, 0]) > 0);
    }

    fn take(sig: i32) -> Option<Siginfo> {
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
    }

    fn quiet(sig: i32) -> bool {
        std::thread::sleep(std::time::Duration::from_millis(200));
        take_process(sig).is_none()
    }

    fn socketpair() -> [i32; 2] {
        let mut fds = [0i32; 2];
        assert_eq!(net::socketpair([1, 1, 0, fds.as_mut_ptr() as u64, 0, 0]), 0);
        fds
    }

    fn close(fds: &[i32]) {
        for fd in fds {
            fdtab::close_owned_guest(*fd, false).unwrap();
        }
    }

    /// A socket's O_ASYNC and process owner are Darwin's: F_SETFL sets the
    /// host flag, F_GETFL reports it, and Darwin's SIGIO reaches the owner.
    #[test]
    fn o_async_socket_signals_its_owner() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::o_async_socket_signals_its_owner") {
            return;
        }
        fdtab::init();
        super::super::signal::install_host_handlers();
        let [a, b] = socketpair();
        let pid = unsafe { libc::getpid() };
        assert_eq!(fcntl(a, F_SETOWN, pid as u64), 0);
        assert_eq!(fcntl(a, F_GETOWN, 0), pid as i64);
        put(b, b"x");
        assert!(quiet(SIGIO));
        get(a);

        assert_eq!(fcntl(a, F_SETFL, O_ASYNC), 0);
        assert!(is_async(a));
        assert_ne!(unsafe { libc::fcntl(a, libc::F_GETFL) } & libc::O_ASYNC, 0);
        put(b, b"x");
        assert!(take(SIGIO).is_some());
        get(a);

        assert_eq!(fcntl(a, F_SETFL, 0), 0);
        assert!(!is_async(a));
        put(b, b"x");
        assert!(quiet(SIGIO));
        close(&[a, b]);
    }

    /// F_SETSIG on a socket: Darwin's owner is cleared, and each arrival of
    /// data (not data already queued) sends the signal with POLL_IN, its
    /// band and the fd that set O_ASYNC; a dup shares the signal, and
    /// without it Darwin signals the owner again.
    #[test]
    fn f_setsig_socket_names_the_fd() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::f_setsig_socket_names_the_fd") {
            return;
        }
        const SIGRT: i32 = 35;
        fdtab::init();
        super::super::signal::install_host_handlers();
        let [a, b] = socketpair();
        let pid = unsafe { libc::getpid() };
        assert_eq!(fcntl(a, F_SETOWN, pid as u64), 0);
        assert_eq!(fcntl(a, F_SETSIG, SIGRT as u64), 0);
        assert_eq!(fcntl(a, F_GETSIG, 0), SIGRT as i64);
        assert_eq!(fcntl(a, F_GETOWN, 0), pid as i64);
        assert_eq!(unsafe { libc::fcntl(a, libc::F_GETOWN) }, 0);
        let mut owner = [0i32; 2];
        assert_eq!(fcntl(a, F_GETOWN_EX, owner.as_mut_ptr() as u64), 0);
        assert_eq!(owner, [1, pid]);

        put(b, b"x");
        assert_eq!(fcntl(a, F_SETFL, O_ASYNC), 0);
        assert!(is_async(a));
        assert!(quiet(SIGRT) && quiet(SIGIO));
        put(b, b"y");
        let info = take(SIGRT).expect("F_SETSIG signal");
        assert_eq!((info.code, info.fields[0]), (1, 0x41));
        assert_eq!(info.fields[1] as i32, a);
        assert!(quiet(SIGIO));
        get(a);

        let alias = fs::dup([a as u64, 0, 0, 0, 0, 0]) as i32;
        assert!(alias >= 0);
        assert_eq!(fcntl(alias, F_GETSIG, 0), SIGRT as i64);

        // FIOASYNC off and on again, through the dup.
        assert_eq!(fioasync(alias, 0), 0);
        assert!(!is_async(a));
        put(b, b"x");
        assert!(quiet(SIGRT));
        get(a);
        assert_eq!(fioasync(alias, 1), 0);
        put(b, b"x");
        let info = take(SIGRT).expect("F_SETSIG signal after FIOASYNC");
        assert_eq!(info.fields[1] as i32, alias);
        get(a);

        assert_eq!(fcntl(a, F_SETSIG, 0), 0);
        assert_eq!(fcntl(alias, F_GETSIG, 0), 0);
        assert_eq!(unsafe { libc::fcntl(a, libc::F_GETOWN) }, pid);
        put(b, b"x");
        assert!(take(SIGIO).is_some());
        assert!(quiet(SIGRT));
        close(&[a, alias, b]);
    }

    /// Closing the fd that set O_ASYNC leaves a dup's signals; the last
    /// close ends them without holding the file open. A fork child's copy
    /// of the owner comes back from the fork state.
    #[test]
    fn registration_outlives_the_arming_fd() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::registration_outlives_the_arming_fd") {
            return;
        }
        use super::super::fork_state::{Reader, Writer};
        const SIGRT: i32 = 37;
        fdtab::init();
        let [a, b] = socketpair();
        let pid = unsafe { libc::getpid() };
        assert_eq!(fcntl(a, F_SETOWN, pid as u64), 0);
        assert_eq!(fcntl(a, F_SETSIG, SIGRT as u64), 0);
        assert_eq!(fcntl(a, F_SETFL, O_ASYNC), 0);
        let alias = fs::dup([a as u64, 0, 0, 0, 0, 0]) as i32;
        assert!(alias >= 0);

        let mut w = Writer::default();
        fork_save(&mut w);
        let blob = w.into_bytes();
        let saved = lock(&FILES).take();
        let mut r = Reader::new(&blob);
        fork_restore(&mut r);
        assert!(r.ok());
        assert_eq!(fcntl(alias, F_GETSIG, 0), SIGRT as i64);
        let restored = std::mem::take(&mut *lock(&RESTORED));
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].1, a);
        // The copy's election is the parent's own file here.
        std::mem::forget(restored);
        *lock(&FILES) = saved;

        close(&[a]);
        put(b, b"x");
        let info = take(SIGRT).expect("signal after the arming fd closed");
        assert_eq!(info.fields[1] as i32, a);
        get(alias);
        close(&[alias]);
        assert!(file(alias).is_none());
        let mut byte = 0u8;
        assert_eq!(
            fs::read([b as u64, &mut byte as *mut u8 as u64, 1, 0, 0, 0]),
            0
        );
        close(&[b]);
    }

    /// Without a pid table, so with no agent to carry it, a kill of SIGIO
    /// to another process is Darwin's, and is delivered.
    #[test]
    fn a_sigio_kill_without_an_agent_is_delivered() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("10")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        assert!(super::super::cred::by_pid_dir().is_none());
        assert_eq!(
            super::super::signal::signal_process(pid, SIGIO, libc::SIGIO),
            0
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A pipe's FIOASYNC is O_ASYNC: the reader's owner hears each write,
    /// as `send_sigio` signals it without F_SETSIG: SI_KERNEL, from no
    /// process.
    #[test]
    fn fioasync_pipe() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::fioasync_pipe") {
            return;
        }
        fdtab::init();
        super::super::signal::install_host_handlers();
        let mut p = [0i32; 2];
        assert_eq!(fs::pipe2([p.as_mut_ptr() as u64, 0, 0, 0, 0, 0]), 0);
        let pid = unsafe { libc::getpid() };
        assert_eq!(fcntl(p[0], F_SETOWN, pid as u64), 0);
        assert_eq!(fioasync(p[0], 1), 0);
        assert!(is_async(p[0]));
        put(p[1], b"x");
        let info = take(SIGIO).unwrap();
        assert_eq!((info.code, info.fields[0]), (0x80, 0));
        get(p[0]);
        assert_eq!(fioasync(p[0], 0), 0);
        assert!(!is_async(p[0]));
        put(p[1], b"x");
        assert!(quiet(SIGIO));
        close(&p);
    }

    /// An F_SETSIG owner of the descriptor `fd` that set O_ASYNC.
    fn setsig(fd: i32, sig: i32) {
        let pid = unsafe { libc::getpid() };
        assert_eq!(fcntl(fd, F_SETOWN, pid as u64), 0);
        assert_eq!(fcntl(fd, F_SETSIG, sig as u64), 0);
        assert_eq!(fcntl(fd, F_SETFL, O_ASYNC), 0);
    }

    /// The reason and band of `sig`'s next signal, which names `fd`.
    fn reason(sig: i32, fd: i32) -> (i32, u64) {
        let info = take(sig).expect("F_SETSIG signal");
        assert_eq!(info.fields[1] as i32, fd);
        (info.code, info.fields[0])
    }

    /// `pipe_read` and `pipe_write` signal the other end's owner, POLL_OUT
    /// for a writer and POLL_IN for a reader, and `pipe_release` does too
    /// when the other end closes.
    #[test]
    fn f_setsig_pipe_signals_both_ends() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::f_setsig_pipe_signals_both_ends") {
            return;
        }
        const READER: i32 = 38;
        const WRITER: i32 = 39;
        fdtab::init();
        super::super::signal::install_host_handlers();
        let mut p = [0i32; 2];
        assert_eq!(fs::pipe2([p.as_mut_ptr() as u64, 0, 0, 0, 0, 0]), 0);
        setsig(p[0], READER);
        setsig(p[1], WRITER);
        put(p[1], b"x");
        assert_eq!(reason(READER, p[0]), (1, 0x41));
        assert!(quiet(WRITER));
        get(p[0]);
        assert_eq!(reason(WRITER, p[1]), (2, 0x304));
        close(&[p[0]]);
        assert_eq!(reason(WRITER, p[1]), (2, 0x304));
        assert!(quiet(READER));
        close(&[p[1]]);

        assert_eq!(fs::pipe2([p.as_mut_ptr() as u64, 0, 0, 0, 0, 0]), 0);
        setsig(p[0], READER);
        close(&[p[1]]);
        assert_eq!(reason(READER, p[0]), (1, 0x41));
        close(&[p[0]]);
    }

    /// A socket's peer that shuts down its writing is POLL_IN; one that
    /// closes is POLL_HUP with EPOLLHUP | EPOLLERR.
    #[test]
    fn f_setsig_socket_hangup() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::f_setsig_socket_hangup") {
            return;
        }
        const SIGRT: i32 = 41;
        fdtab::init();
        super::super::signal::install_host_handlers();
        let [a, b] = socketpair();
        setsig(a, SIGRT);
        assert_eq!(net::shutdown([b as u64, 1, 0, 0, 0, 0]), 0);
        assert_eq!(reason(SIGRT, a), (1, 0x41));
        assert!(quiet(SIGRT));
        close(&[b]);
        assert_eq!(reason(SIGRT, a), (6, 0x18));
        assert!(quiet(SIGRT) && quiet(SIGIO));
        close(&[a]);
    }

    /// A memfd (shmem) has no fasync: F_SETSIG and O_ASYNC are kept, and
    /// a write signals nothing.
    #[test]
    fn f_setsig_memfd_signals_nothing() {
        if fdtab::isolated_kernel_test("sys::fasync::tests::f_setsig_memfd_signals_nothing") {
            return;
        }
        const SIGRT: i32 = 42;
        let _view = crate::vfs::test_view();
        fdtab::init();
        super::super::signal::install_host_handlers();
        let name = c"fasync";
        let fd = super::super::memfd::memfd_create([name.as_ptr() as u64, 0, 0, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        setsig(fd, SIGRT);
        assert!(is_async(fd));
        put(fd, b"x");
        assert!(quiet(SIGRT) && quiet(SIGIO));
        close(&[fd]);
    }

    /// An eventfd has no fasync: FIOASYNC cannot change O_ASYNC, F_SETFL
    /// keeps it without effect, and its owner is never signalled.
    #[test]
    fn eventfd_keeps_o_async_and_owner_without_signals() {
        if fdtab::isolated_kernel_test(
            "sys::fasync::tests::eventfd_keeps_o_async_and_owner_without_signals",
        ) {
            return;
        }
        fdtab::init();
        super::super::signal::install_host_handlers();
        let fd = event::eventfd2([0, 0, 0, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        assert_eq!(fioasync(fd, 1), -ENOTTY);
        assert_eq!(fioasync(fd, 0), 0);
        let pid = unsafe { libc::getpid() };
        assert_eq!(fcntl(fd, F_SETOWN, pid as u64), 0);
        assert_eq!(fcntl(fd, F_GETOWN, 0), pid as i64);
        assert_eq!(fcntl(fd, F_SETSIG, 40), 0);
        assert_eq!(fcntl(fd, F_GETSIG, 0), 40);
        assert_eq!(fcntl(fd, F_SETFL, O_ASYNC), 0);
        assert!(is_async(fd));
        assert_eq!(fioasync(fd, 1), 0);
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETOWN) }, 0);
        put(fd, &1u64.to_le_bytes());
        assert!(quiet(SIGIO) && quiet(40));
        close(&[fd]);
    }
}
