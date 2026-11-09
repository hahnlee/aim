//! Signal-driven I/O (O_ASYNC) that this layer delivers: the watcher
//! thread, and the owners Darwin's fown cannot hold.
//!
//! Darwin keeps a host file's O_ASYNC flag and its process or group owner,
//! shared by every descriptor of the file as Linux's are, and raises SIGIO
//! itself. It has no F_SETSIG and no thread owner (F_SETOWN_EX
//! F_OWNER_TID), and its SIGIO does not say which descriptor raised it. A
//! host file with either keeps its owner here instead, Darwin's is cleared,
//! and while O_ASYNC is set a private kqueue holds an EV_CLEAR read knote
//! on the descriptor that set it: each arrival of data (`kill_fasync` with
//! POLL_IN) wakes the watcher thread, which signals the owner with that
//! descriptor as `si_fd`. A knote reports each wakeup of the file, so an
//! event is one signal, where a readiness check on SIGIO would signal every
//! readable descriptor again. The private kqueue is drained once the knote
//! is in it, so data already queued raises nothing, as on Linux.
//!
//! The descriptors this layer implements whose Linux files have no fasync
//! (eventfd, timerfd, signalfd, epoll, pidfd, ...) keep their owner here
//! too, never signalled: Darwin's stand-in for them would raise SIGIO.

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
/// kqueue `source`.
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

/// The owner of an open file that is kept here, shared by its dups.
struct File {
    fown: Fown,
    /// A host file, which the watcher signals for; else one without fasync.
    host: bool,
    armed: Mutex<Option<Armed>>,
}

/// A host file's O_ASYNC registration with the watcher thread.
struct Armed {
    /// The guest fd that set O_ASYNC, with the knote (`fa_fd`).
    fd: i32,
    /// The private kqueue holding the knote.
    kq: i32,
    id: u64,
}

impl Arrival for File {
    fn arrived(&self) {
        let fd = match lock(&self.armed).as_ref() {
            Some(armed) => {
                drain(armed.kq);
                armed.fd
            }
            None => return,
        };
        self.fown.owner().readable(fd);
    }
}

impl File {
    fn disarm(&self) {
        if let Some(armed) = lock(&self.armed).take() {
            unregister(armed.id);
            close_private(armed.kq);
        }
    }
}

/// The files kept here, by guest fd.
static FILES: Mutex<Option<HashMap<i32, Arc<File>>>> = Mutex::new(None);

fn file(fd: i32) -> Option<Arc<File>> {
    lock(&FILES).as_ref()?.get(&fd).cloned()
}

/// fcntl's owner commands (F_SETOWN, F_SETSIG, ...) on guest fd `guest`,
/// whose host descriptor is `fd`; None for other commands. `host`: the
/// host file is the Linux file, whose fown Darwin holds when it can.
pub fn fcntl(fd: i32, guest: i32, host: bool, cmd: u64, arg: u64) -> Option<i64> {
    if !matches!(
        cmd,
        F_SETOWN | F_GETOWN | F_SETOWN_EX | F_GETOWN_EX | F_SETSIG | F_GETSIG
    ) {
        return None;
    }
    let (file, kept) = match file(guest) {
        Some(file) => (file, true),
        None if host && matches!(cmd, F_SETOWN | F_GETOWN) => {
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
                host,
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
    if host && let Err(error) = project(fd, guest, &file) {
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
    let host = file.fown.owner().host();
    // SAFETY: plain fcntl.
    if unsafe { libc::fcntl(fd, libc::F_SETOWN, host.unwrap_or(0)) } < 0 {
        return Err(errno::last());
    }
    if host.is_some() {
        if let Some(files) = lock(&FILES).as_mut() {
            files.retain(|_, other| !Arc::ptr_eq(other, file));
        }
        file.disarm();
        return Ok(());
    }
    refresh(fd, guest, file)
}

/// Arm or disarm host file `file` (descriptor `fd`, guest fd `guest`) as
/// its O_ASYNC flag now says.
fn refresh(fd: i32, guest: i32, file: &Arc<File>) -> Result<(), Errno> {
    // SAFETY: plain fcntl.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(errno::last());
    }
    if flags & libc::O_ASYNC == 0 {
        file.disarm();
        return Ok(());
    }
    let mut armed = lock(&file.armed);
    if armed.is_some() {
        return Ok(());
    }
    let kq = private_kqueue(None)?;
    let registered = (|| {
        if flags & libc::O_ACCMODE != libc::O_WRONLY {
            let k = libc::kevent {
                ident: guest as usize,
                filter: libc::EVFILT_READ,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: one change, no events.
            if unsafe { libc::kevent(kq, &k, 1, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 {
                return Err(errno::last());
            }
            drain(kq);
        }
        let target: Weak<File> = Arc::downgrade(file);
        register(kq, target)
    })();
    match registered {
        Ok(id) => {
            *armed = Some(Armed { fd: guest, kq, id });
            Ok(())
        }
        Err(error) => {
            close_private(kq);
            Err(error)
        }
    }
}

/// F_SETFL changed host descriptor `fd` (guest fd `guest`): a file kept
/// here follows its O_ASYNC flag.
pub fn status_changed(fd: i32, guest: i32) -> Result<(), Errno> {
    match file(guest) {
        Some(file) if file.host && file.fown.owner().host().is_none() => refresh(fd, guest, &file),
        _ => Ok(()),
    }
}

/// Guest fd `fd` closed: its file leaves the table, and its O_ASYNC
/// registration ends with the knote Darwin dropped.
pub fn on_close(fd: i32) {
    let Some(file) = lock(&FILES).as_mut().and_then(|files| files.remove(&fd)) else {
        return;
    };
    if lock(&file.armed)
        .as_ref()
        .is_some_and(|armed| armed.fd == fd)
    {
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

    /// A pipe's FIOASYNC is O_ASYNC: the reader's owner hears each write.
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
        assert!(take(SIGIO).is_some());
        get(p[0]);
        assert_eq!(fioasync(p[0], 0), 0);
        assert!(!is_async(p[0]));
        put(p[1], b"x");
        assert!(quiet(SIGIO));
        close(&p);
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
