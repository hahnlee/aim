//! epoll over kqueue (after FreeBSD's Linuxulator, `linux_event.c`,
//! BSD-2-Clause).
//!
//! - An epoll fd is a kqueue; an interest becomes EVFILT_READ and/or
//!   EVFILT_WRITE knotes with the fd as ident. Level-triggered is kqueue's
//!   default, EPOLLET maps to EV_CLEAR and EPOLLONESHOT to EV_DISPATCH (the
//!   sibling filter is disabled on delivery, as epoll disables the whole
//!   interest). EPOLLEXCLUSIVE is checked as epoll_ctl(2) checks it; each
//!   attached epoll then still receives the wakeup, which is the "one or
//!   more" the flag allows.
//! - A kqueue is itself pollable, so epoll fds nest and work in poll().
//! - A signalfd's socket shows only the process's pending signals. An
//!   epoll_wait reads each signalfd in its interest list as the waiter
//!   (`ep_item_poll` runs `signalfd_poll` in the waiter's context), waiting
//!   on the kqueue and its thread's wake token together. A poll of the
//!   epoll fd, or an epoll watching it, sees only the socket.
//! - The interest list is kept here too: Darwin does not inherit kqueues
//!   across fork, so a fork child rebuilds each one on the same fd number.
//!   The kernel stays authoritative for whether an fd is registered: a
//!   closed fd drops its knotes, and ADD checks the kernel before EEXIST.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::fdtab::{self, Kind};
use super::wait::ReapAlias;
use crate::errno::{self, EBADF, EEXIST, EINVAL, ENOENT, EPERM};

// Each receipt cohort is bounded independently, including busy level-triggered waits.
fn trace_receipt(counter: &std::sync::atomic::AtomicUsize) -> bool {
    super::tracing()
        && counter
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| (n < 64).then_some(n + 1),
            )
            .is_ok()
}
static TRACE_CTL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TRACE_WAIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn trace_ctl(epfd: i32, op: u64, fd: i32, interest: Option<Interest>) {
    if trace_receipt(&TRACE_CTL) {
        crate::diag!(
            "epoll receipt pid={} tid={} ctl epfd={epfd} op={op} fd={fd} interest={:?}",
            unsafe { libc::getpid() },
            super::thread::host_tid(),
            interest.map(|i| (i.events, i.data))
        );
    }
}
fn trace_wait(epfd: i32, kevs: &[libc::kevent], out: &[(i32, u32, u64)]) {
    if !trace_receipt(&TRACE_WAIT) {
        return;
    }
    crate::diag!(
        "epoll receipt pid={} tid={} wait epfd={epfd} backend={} returned={} shown_max=16",
        unsafe { libc::getpid() },
        super::thread::host_tid(),
        kevs.len(),
        out.len()
    );
    for k in kevs.iter().take(16) {
        let (ident, filter, flags, fflags, data, udata) = (
            k.ident,
            k.filter,
            k.flags,
            k.fflags,
            k.data,
            k.udata as usize,
        );
        crate::diag!(
            "epoll backend epfd={epfd} ident={} filter={} flags={:#x} fflags={:#x} data={} udata={:#x}",
            ident,
            filter,
            flags,
            fflags,
            data,
            udata
        );
    }
    for &(fd, mask, data) in out.iter().take(16) {
        crate::diag!("epoll returned epfd={epfd} fd={fd} mask={mask:#x} data={data:#x}");
    }
}

const EPOLLIN: u32 = 0x1;
const EPOLLPRI: u32 = 0x2;
const EPOLLOUT: u32 = 0x4;
const EPOLLERR: u32 = 0x8;
const EPOLLHUP: u32 = 0x10;
const EPOLLRDNORM: u32 = 0x40;
const EPOLLWRNORM: u32 = 0x100;
const EPOLLRDHUP: u32 = 0x2000;
const EPOLLEXCLUSIVE: u32 = 1 << 28;
const EPOLLWAKEUP: u32 = 1 << 29;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;

const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLL_CTL_MOD: u64 = 3;
const EPOLL_CLOEXEC: u64 = 0o2000000;
/// `EPOLLEXCLUSIVE_OK_BITS` (`fs/eventpoll.c`).
const EPOLLEXCLUSIVE_OK_BITS: u32 =
    EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLWAKEUP | EPOLLET | EPOLLEXCLUSIVE;

/// do_epoll_ctl's EINVAL for EPOLLEXCLUSIVE: only ADD takes it, not for an
/// epoll target, and with no events beyond EPOLLEXCLUSIVE_OK_BITS; and an
/// exclusive attachment is never modified.
fn exclusive_invalid(op: u64, new: Option<&Interest>, old: Option<&Interest>, epoll: bool) -> bool {
    let asked = new.is_some_and(|n| n.events & EPOLLEXCLUSIVE != 0);
    (asked
        && (op == EPOLL_CTL_MOD
            || epoll
            || new.is_some_and(|n| n.events & !EPOLLEXCLUSIVE_OK_BITS != 0)))
        || op == EPOLL_CTL_MOD && old.is_some_and(|o| o.events & EPOLLEXCLUSIVE != 0)
}

#[derive(Clone, Copy)]
struct Interest {
    events: u32,
    data: u64,
    /// Delivered with EPOLLONESHOT and not re-armed yet.
    disabled: bool,
    /// An edge-triggered signalfd: the [`super::signalfd::notified`] count
    /// it was last reported at, as `ep_poll_callback` queues it again only
    /// on a newer wakeup.
    seen: u64,
    /// The shutdowns ([`super::net::unconnected`]) an edge-triggered
    /// interest in an unconnected TCP socket was reported at since its
    /// last change: only a `shutdown` queues it again (`sk_state_change`)
    /// until the socket leaves `TCP_CLOSE`.
    reported: Option<u32>,
}

impl Interest {
    /// Whether an edge-triggered interest in an unconnected socket was
    /// reported at its `shutdown`s already.
    fn edge_reported(&self, shutdown: u32) -> bool {
        self.events & EPOLLET != 0 && self.reported == Some(shutdown)
    }
    fn wants_read(&self) -> bool {
        self.events & (EPOLLIN | EPOLLRDNORM | EPOLLRDHUP | EPOLLPRI) != 0
    }
    fn wants_write(&self) -> bool {
        self.events & (EPOLLOUT | EPOLLWRNORM) != 0
    }
    /// Whether the interest has an EVFILT_READ knote: one that wants no
    /// filter keeps one, so that closing the fd still drops it.
    fn read_knote(&self) -> bool {
        self.wants_read() || !self.wants_write()
    }
    /// Whether the interest is only for the EPOLLERR and EPOLLHUP that
    /// `ep_item_poll` always reports. Its EVFILT_READ knote's low-water
    /// mark is out of reach, but Darwin caps it at the buffer's size, so a
    /// full buffer fires it too: the knote is edge-triggered, ignored
    /// unless at EOF, and armed again after a hangup it reports, which
    /// stays level-triggered.
    fn hangup_only(&self) -> bool {
        !self.wants_read() && !self.wants_write() && self.events & (EPOLLERR | EPOLLHUP) != 0
    }
}
fn pty_ctl(
    ep: &Arc<Epoll>,
    epfd: i32,
    fd: i32,
    op: u64,
    event: u64,
    description: &Arc<super::pty_owner::Description>,
) -> i64 {
    let watch = match description.watch() {
        Ok(watch) => Arc::new(watch),
        Err(error) => return -(error as i64),
    };
    let identity =
        match aim_storage::inode_lease::Identity::from_fd(watch.observation.status.descriptor()) {
            Ok(identity) => identity,
            Err(error) => {
                return -(errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)) as i64);
            }
        };
    let new = if op == EPOLL_CTL_DEL {
        None
    } else {
        let bytes = match super::user_memory::read_exact(event, 16) {
            Ok(bytes) => bytes,
            Err(error) => return -(error as i64),
        };
        Some(Interest {
            events: u32::from_le_bytes(bytes[..4].try_into().unwrap()),
            data: u64::from_le_bytes(bytes[8..].try_into().unwrap()),
            disabled: false,
            seen: 0,
            reported: None,
        })
    };
    let mut retired = None;
    let guard = fdtab::lifecycle();
    if !fdtab::visible(epfd)
        || !fdtab::visible(fd)
        || !matches!(fdtab::get(epfd),Some(Kind::Epoll(current))if Arc::ptr_eq(&current,ep))
        || !matches!(fdtab::get(fd),Some(Kind::Pty(current))if Arc::ptr_eq(&current,description))
    {
        return -(EBADF as i64);
    }
    let mut interests = ep.ptys.lock().unwrap();
    let index = interests
        .iter()
        .position(|interest| interest.fd == fd && interest.identity == identity);
    if exclusive_invalid(
        op,
        new.as_ref(),
        index.map(|index| &interests[index].interest),
        false,
    ) {
        return -(EINVAL as i64);
    }
    match op {
        EPOLL_CTL_ADD if index.is_some() => return -(EEXIST as i64),
        EPOLL_CTL_MOD | EPOLL_CTL_DEL if index.is_none() => return -(ENOENT as i64),
        EPOLL_CTL_ADD | EPOLL_CTL_MOD | EPOLL_CTL_DEL => {}
        _ => return -(EINVAL as i64),
    }
    // Retire the spare watch after lifecycle admission; its private FDs use the same lock.
    let active = index
        .map(|index| interests[index].watch.clone())
        .unwrap_or_else(|| watch.clone());
    let tag = match index {
        Some(index) => interests[index].tag,
        None => match NEXT_PTY_TAG.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |tag| tag.checked_add(1),
        ) {
            Ok(tag) => tag,
            Err(_) => {
                drop(interests);
                drop(guard);
                drop(active);
                return -(errno::from_darwin(libc::EOVERFLOW) as i64);
            }
        },
    };
    let result = apply(
        epfd,
        &pty_changes(
            &active,
            tag,
            index.map(|index| &interests[index].interest),
            new.as_ref(),
        ),
    );
    if result < 0 {
        drop(interests);
        drop(guard);
        drop(active);
        return result;
    }
    match (index, new) {
        (Some(index), Some(interest)) => interests[index].interest = interest,
        (Some(index), None) => retired = Some(interests.remove(index)),
        (None, Some(interest)) => interests.push(PtyInterest {
            tag,
            fd,
            identity,
            watch: active.clone(),
            interest,
        }),
        _ => {}
    }
    drop(interests);
    drop(guard);
    drop(retired);
    trace_ctl(epfd, op, fd, new);
    0
}

pub struct Epoll {
    interest: Mutex<HashMap<i32, Interest>>,
    /// The pidfd interests' own fds of their reap watches.
    reaps: Mutex<HashMap<i32, ReapAlias>>,
    ptys: Mutex<Vec<PtyInterest>>,
}
struct PtyInterest {
    tag: u64,
    fd: i32,
    identity: aim_storage::inode_lease::Identity,
    watch: Arc<super::pty_owner::Watch>,
    interest: Interest,
}
static NEXT_PTY_TAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn pty_changes(
    watch: &super::pty_owner::Watch,
    tag: u64,
    old: Option<&Interest>,
    new: Option<&Interest>,
) -> Vec<libc::kevent> {
    use std::os::fd::AsRawFd;
    let data = watch.observation.data().as_raw_fd();
    let mut changes = changes(data, None, old, new);
    if let Some(notification) = watch.observation.notification() {
        let fd = notification.as_raw_fd();
        if old.is_some() {
            changes.push(kev(fd, libc::EVFILT_READ, libc::EV_DELETE));
        }
        if let Some(interest) = new {
            let flags = libc::EV_ADD
                | if interest.disabled {
                    libc::EV_DISABLE
                } else {
                    libc::EV_ENABLE
                }
                | if interest.events & EPOLLET != 0 {
                    libc::EV_CLEAR
                } else {
                    0
                }
                | if interest.events & EPOLLONESHOT != 0 {
                    libc::EV_DISPATCH
                } else {
                    0
                };
            changes.push(kev(fd, libc::EVFILT_READ, flags));
        }
    }
    for change in &mut changes {
        change.udata = tag as *mut _;
    }
    changes
}

fn kev(fd: i32, filter: i16, flags: u16) -> libc::kevent {
    libc::kevent {
        ident: fd as usize,
        filter,
        flags: flags | libc::EV_RECEIPT,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

/// Apply `changes` to `kq`; the first error other than a missing knote on
/// delete, as a negative Linux errno.
fn apply(kq: i32, changes: &[libc::kevent]) -> i64 {
    if changes.is_empty() {
        return 0;
    }
    let mut out = changes.to_vec();
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: change and receipt arrays of equal length.
    let n = unsafe {
        libc::kevent(
            kq,
            changes.as_ptr(),
            changes.len() as i32,
            out.as_mut_ptr(),
            out.len() as i32,
            &zero,
        )
    };
    if n < 0 {
        return -(errno::last() as i64);
    }
    for (c, r) in changes.iter().zip(&out[..n as usize]) {
        if r.flags & libc::EV_ERROR != 0 && r.data != 0 {
            if c.flags & libc::EV_DELETE != 0 && r.data == libc::ENOENT as isize {
                continue;
            }
            return -(errno::from_darwin(r.data as i32) as i64);
        }
    }
    0
}

/// The knote changes that make `kq` match `new` for `fd` (from `old`),
/// and for a pidfd's on `reap`, its interest's alias of the reap watch.
/// Darwin keeps a knote's EV_CLEAR/EV_DISPATCH from its creation, so a
/// modified interest replaces its knotes.
fn changes(
    fd: i32,
    reap: Option<i32>,
    old: Option<&Interest>,
    new: Option<&Interest>,
) -> Vec<libc::kevent> {
    let mut v = Vec::new();
    for (filter, want) in [
        (
            libc::EVFILT_READ,
            Interest::read_knote as fn(&Interest) -> bool,
        ),
        (libc::EVFILT_WRITE, Interest::wants_write),
    ] {
        if old.is_some_and(want) {
            v.push(kev(fd, filter, libc::EV_DELETE));
        }
        if let Some(n) = new.filter(|n| want(n)) {
            let hangup = filter == libc::EVFILT_READ && n.hangup_only();
            let mut f = libc::EV_ADD;
            f |= if n.disabled || filter == libc::EVFILT_READ && !n.wants_read() && !hangup {
                libc::EV_DISABLE
            } else {
                libc::EV_ENABLE
            };
            if n.events & EPOLLET != 0 || hangup {
                f |= libc::EV_CLEAR;
            }
            if n.events & EPOLLONESHOT != 0 {
                f |= libc::EV_DISPATCH;
            }
            let mut k = kev(fd, filter, f);
            if hangup {
                k.fflags = libc::NOTE_LOWAT;
                k.data = isize::MAX;
            }
            v.push(k);
        }
    }
    // A pidfd's reap is its reap watch's, as the pidfd itself polls
    // readable from the exit on.
    if let Some(reap) = reap {
        if old.is_some() {
            v.push(kev(reap, libc::EVFILT_READ, libc::EV_DELETE));
        }
        if let Some(n) = new {
            let mut f = libc::EV_ADD;
            f |= if n.disabled {
                libc::EV_DISABLE
            } else {
                libc::EV_ENABLE
            };
            if n.events & EPOLLET != 0 {
                f |= libc::EV_CLEAR;
            }
            if n.events & EPOLLONESHOT != 0 {
                f |= libc::EV_DISPATCH;
            }
            v.push(kev(reap, libc::EVFILT_READ, f));
        }
    }
    v
}

/// Whether the kernel still has a knote for `fd` in `kq`. A change with no
/// action flags only reports whether the knote exists.
fn registered(kq: i32, fd: i32, i: &Interest) -> bool {
    let filter = if i.read_knote() {
        libc::EVFILT_READ
    } else {
        libc::EVFILT_WRITE
    };
    apply(kq, &[kev(fd, filter, 0)]) == 0
}

fn epoll_of(epfd: i32) -> Result<Arc<Epoll>, i64> {
    if super::fs::is_path_fd(epfd) {
        return Err(-(EBADF as i64));
    }
    match fdtab::get(epfd) {
        Some(Kind::Epoll(e)) => Ok(e),
        // SAFETY: plain fcntl to tell EBADF from EINVAL.
        _ if unsafe { libc::fcntl(epfd, libc::F_GETFD) } < 0 => Err(-(EBADF as i64)),
        _ => Err(-(EINVAL as i64)),
    }
}

pub fn epoll_create1(a: [u64; 6]) -> i64 {
    let flags = a[0];
    if flags & !EPOLL_CLOEXEC != 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: plain kqueue.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return -(errno::last() as i64);
    }
    fdtab::set_flags(kq, false, flags & EPOLL_CLOEXEC != 0);
    fdtab::insert(
        kq,
        Kind::Epoll(Arc::new(Epoll {
            interest: Mutex::new(HashMap::new()),
            reaps: Mutex::new(HashMap::new()),
            ptys: Mutex::new(Vec::new()),
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

pub fn epoll_ctl(a: [u64; 6]) -> i64 {
    let (epfd, op, fd, ev) = (a[0] as i32, a[1], a[2] as i32, a[3]);
    let ep = match epoll_of(epfd) {
        Ok(e) => e,
        Err(e) => return e,
    };
    if super::fs::is_path_fd(fd) {
        return -(EBADF as i64);
    }
    if let Some(Kind::Pty(description)) = fdtab::get(fd) {
        return pty_ctl(&ep, epfd, fd, op, ev, &description);
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return -(EBADF as i64);
    }
    if fd == epfd {
        return -(EINVAL as i64);
    }
    if matches!(st.st_mode & libc::S_IFMT, libc::S_IFREG | libc::S_IFDIR) {
        return -(EPERM as i64);
    }
    let kind = fdtab::get(fd);
    let new = if op != EPOLL_CTL_DEL {
        // SAFETY: guest struct epoll_event (arm64: u32 events, u64 data at 8).
        let raw = user_read!(ev, [u64; 2]);
        Some(Interest {
            events: raw[0] as u32,
            data: raw[1],
            disabled: false,
            seen: 0,
            reported: None,
        })
    } else {
        None
    };
    // The flag's own rules precede the interest list's EEXIST and ENOENT;
    // refusing to modify an exclusive attachment needs the attachment.
    let epoll = matches!(kind, Some(Kind::Epoll(_)));
    if exclusive_invalid(op, new.as_ref(), None, epoll) {
        return -(EINVAL as i64);
    }
    // Only the events the fd's Linux `_poll` can report are asked of its
    // knotes (`poll::reportable`): a timerfd's or signalfd's socket is
    // writable, and a kqueue takes no EVFILT_WRITE. Any other fd is also
    // watched for EPOLLERR and EPOLLHUP, as `ep_insert` and `ep_modify`
    // add them.
    let new = new.map(|mut n| {
        match super::poll::reportable(kind.as_ref()) {
            Some(events) => {
                n.events &= events as u32 | EPOLLEXCLUSIVE | EPOLLWAKEUP | EPOLLONESHOT | EPOLLET
            }
            None => n.events |= EPOLLERR | EPOLLHUP,
        }
        n
    });
    let mut interest = ep.interest.lock().unwrap();
    let old = interest
        .get(&fd)
        .copied()
        .filter(|i| registered(epfd, fd, i));
    if old.is_some() && exclusive_invalid(op, new.as_ref(), old.as_ref(), epoll) {
        return -(EINVAL as i64);
    }
    match op {
        EPOLL_CTL_ADD if old.is_some() => return -(EEXIST as i64),
        EPOLL_CTL_MOD | EPOLL_CTL_DEL if old.is_none() => return -(ENOENT as i64),
        EPOLL_CTL_ADD | EPOLL_CTL_MOD | EPOLL_CTL_DEL => {}
        _ => return -(EINVAL as i64),
    }
    if new.is_some_and(|n| n.events & EPOLLIN != 0) {
        // Linux calls binder_poll from here: the thread becomes a poller.
        super::binder::poll(fd);
    }
    // A pidfd's reap knote is on the interest's own alias of its reap
    // watch; one left by a file closed since is closed, with its knote.
    let mut reaps = ep.reaps.lock().unwrap();
    let (mut alias, stale): (Option<ReapAlias>, _) = match reaps.remove(&fd) {
        Some(alias) if alias.current(fd) => (Some(alias), None),
        stale => (None, stale),
    };
    if new.is_some() && alias.is_none() {
        alias = match ReapAlias::new(fd).transpose() {
            Ok(alias) => alias,
            Err(error) => return -(error as i64),
        };
    }
    let reap = alias.as_ref().map(ReapAlias::fd);
    let r = apply(epfd, &changes(fd, reap, old.as_ref(), new.as_ref()));
    if r < 0 {
        if let Some(alias) = alias.filter(|_| old.is_some()) {
            reaps.insert(fd, alias);
        }
        return r;
    }
    match new {
        Some(n) => {
            interest.insert(fd, n);
            if let Some(alias) = alias.take() {
                reaps.insert(fd, alias);
            }
        }
        None => {
            interest.remove(&fd);
        }
    }
    drop(reaps);
    drop(interest);
    drop((alias, stale));
    trace_ctl(epfd, op, fd, new.or(old));
    0
}

/// Linux epoll event bits for one kevent of an interest.
fn bits(k: &libc::kevent, i: &Interest) -> u32 {
    if let Some(events) = super::fuse_device::readiness(k.ident as i32) {
        return match events {
            Ok(events) => (events as u32) & (i.events | EPOLLERR | EPOLLHUP),
            Err(_) => EPOLLERR,
        };
    }
    let eof = k.flags & libc::EV_EOF != 0;
    let mut b = 0;
    if k.flags & libc::EV_ERROR != 0 {
        return EPOLLERR;
    }
    // A pipe end whose other end is gone, which Darwin marks with EOF for
    // either filter: `pipe_poll` reports a read end hung up, readable only
    // with data queued, and a write end EPOLLERR, never EPOLLHUP, and
    // writable, as Darwin frees the buffer with the last reader.
    let pipe = eof
        .then(|| {
            let fd = k.ident as i32;
            super::poll::pipe(fd, fdtab::get(fd).as_ref())
        })
        .flatten();
    match pipe {
        Some(super::poll::PipeEnd::Read) => {
            return EPOLLHUP
                | if k.filter == libc::EVFILT_READ && k.data > 0 {
                    i.events & (EPOLLIN | EPOLLRDNORM)
                } else {
                    0
                };
        }
        Some(super::poll::PipeEnd::Write) => return EPOLLERR | i.events & (EPOLLOUT | EPOLLWRNORM),
        Some(super::poll::PipeEnd::Both) | None => {}
    }
    // A socket's shutdown, which Darwin marks with EOF either way: Linux
    // reports a hangup only once both directions are shut.
    let shut = eof
        .then(|| {
            super::tcp_shutdown::shutdown(
                k.ident as i32,
                k.filter == libc::EVFILT_READ,
                k.filter == libc::EVFILT_WRITE,
            )
            .or_else(|| super::net::shut_directions(k.ident as i32))
        })
        .flatten();
    if let Some(shut) = shut {
        if k.filter == libc::EVFILT_READ {
            b |= i.events & (EPOLLIN | EPOLLRDNORM | EPOLLRDHUP);
        } else if k.filter == libc::EVFILT_WRITE {
            b |= i.events & (EPOLLOUT | EPOLLWRNORM);
        }
        if shut.hangup() {
            b |= EPOLLHUP;
        }
        if k.fflags != 0 {
            b |= EPOLLERR;
        }
    } else if k.filter == libc::EVFILT_READ {
        b |= i.events & (EPOLLIN | EPOLLRDNORM);
        if eof {
            b |= EPOLLHUP | (i.events & EPOLLRDHUP);
            if k.fflags != 0 {
                b |= EPOLLERR;
            }
        } else if k.data == 0
            || !i.wants_read()
            || matches!(fdtab::get(k.ident as i32), Some(Kind::Sock(s)) if s.datagram())
        {
            // Darwin's filt_soread fires for a pending socket error, a
            // datagram's without EOF, as for readable data: a firing with
            // nothing to read, or a hangup-only knote's, may be the
            // error's, which `sock_poll` reports as EPOLLERR, with EPOLLIN
            // only for a queued datagram, a zero-length one included (its
            // address counts in Darwin's queued bytes, not in `data`). A
            // datagram socket's error may also arrive behind a queued one.
            if let Some(queued) = super::net::pending_error(k.ident as i32) {
                b = EPOLLERR | if queued == 0 { 0 } else { b };
            }
        }
        if b & EPOLLIN != 0 && super::wait::pidfd_reaped(k.ident as i32) {
            b |= EPOLLHUP;
        }
    } else if k.filter == libc::EVFILT_WRITE {
        b |= i.events & (EPOLLOUT | EPOLLWRNORM);
        if eof {
            b |= if k.fflags != 0 { EPOLLERR } else { EPOLLHUP };
        } else if super::net::pending_error(k.ident as i32).is_some() {
            // Nothing in the firing tells an error apart: filt_sowrite
            // reports it with the free space, as for plain writability.
            b |= EPOLLERR;
        }
    }
    b
}

/// Whether `ep` (or an epoll it watches) watches a socket. Only the fds'
/// kinds are read: touching their kqueue registrations would re-arm
/// edge-triggered ones.
pub(super) fn observes_sockets(ep: &Epoll, seen: &mut std::collections::HashSet<usize>) -> bool {
    if !seen.insert(ep as *const Epoll as usize) {
        return false;
    }
    let fds: Vec<i32> = ep
        .interest
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(fd, interest)| {
            (!interest.disabled
                && (interest.wants_read() || interest.wants_write() || interest.hangup_only()))
            .then_some(*fd)
        })
        .collect();
    fds.into_iter().any(|fd| match fdtab::get(fd) {
        Some(Kind::Sock(_)) => true,
        Some(Kind::Epoll(child)) => observes_sockets(&child, seen),
        _ => false,
    })
}

/// The armed signalfd interests of `ep` that ask for EPOLLIN, with their
/// descriptions and the count an edge-triggered one was last reported at.
fn signal_interests(ep: &Epoll) -> Vec<(i32, Arc<super::signalfd::SignalFd>, u64)> {
    let interests: Vec<(i32, u64)> = ep
        .interest
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, i)| !i.disabled && i.events & EPOLLIN != 0)
        .map(|(fd, i)| (*fd, if i.events & EPOLLET != 0 { i.seen } else { 0 }))
        .collect();
    interests
        .into_iter()
        .filter_map(|(fd, seen)| match fdtab::get(fd) {
            Some(Kind::Signal(description)) => Some((fd, description, seen)),
            _ => None,
        })
        .collect()
}

/// What interest `i` reports of `kept`: no EPOLLERR of a kept error to
/// an edge-triggered one, which no wakeup queued.
fn kept_bits(kept: &super::net::Kept, i: &Interest) -> u32 {
    let ignored = if i.events & EPOLLET != 0 && kept.events & EPOLLERR != 0 {
        EPOLLERR
    } else {
        0
    };
    kept.events & (i.events | EPOLLHUP | EPOLLERR) & !ignored
}

/// The armed interests of `ep` in a socket with events this layer keeps
/// ([`super::net::kept`]), which no knote fires for, that are to be
/// reported.
fn kept_interests(ep: &Epoll) -> Vec<(i32, super::net::Kept)> {
    let interests: Vec<(i32, Interest)> = ep
        .interest
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, i)| !i.disabled)
        .map(|(fd, i)| (*fd, *i))
        .collect();
    interests
        .into_iter()
        .filter_map(|(fd, i)| {
            let kept = match fdtab::get(fd) {
                Some(Kind::Sock(s)) => super::net::kept(&s, fd)?,
                _ => return None,
            };
            (kept_bits(&kept, &i) != 0 && !i.edge_reported(kept.generation)).then_some((fd, kept))
        })
        .collect()
}

/// Milliseconds for poll(2), rounded up, -1 for none.
fn poll_ms(timeout: Option<libc::timespec>) -> i32 {
    timeout.map_or(-1, |t| {
        t.tv_sec
            .saturating_mul(1000)
            .saturating_add((t.tv_nsec + 999_999) / 1_000_000)
            .min(i32::MAX as i64) as i32
    })
}

fn wait(epfd: i32, events: u64, maxevents: i32, timeout: Option<libc::timespec>, mask: u64) -> i64 {
    let ep = match epoll_of(epfd) {
        Ok(e) => e,
        Err(e) => return e,
    };
    if maxevents <= 0 || maxevents > i32::MAX / 16 {
        return -(EINVAL as i64);
    }
    if observes_sockets(&ep, &mut std::collections::HashSet::new()) {
        if let Err(error) = super::close_effects::observe_socket() {
            return -(error as i64);
        }
    }
    // Every pending knote at once (an interest has a read and a write
    // knote, a pty also its notification), so an fd's filters merge into
    // one event and the fds past `maxevents` are re-armed, not lost.
    let knotes = 2 * ep.interest.lock().unwrap().len() + 3 * ep.ptys.lock().unwrap().len();
    let fetch = knotes.max(maxevents as usize).min(i32::MAX as usize);
    let mut kevs: Vec<libc::kevent> = Vec::with_capacity(fetch);
    let deadline = timeout.map(|t| now_ns() + t.tv_sec * 1_000_000_000 + t.tv_nsec);
    let zero = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let out = loop {
        let left = deadline.map(|d| {
            let ns = (d - now_ns()).max(0);
            libc::timespec {
                tv_sec: ns / 1_000_000_000,
                tv_nsec: ns % 1_000_000_000,
            }
        });
        // The signalfd interests as of now, as another thread may have
        // changed them, or reported or disabled one, since the last wait.
        let signals = signal_interests(&ep);
        let kept = kept_interests(&ep);
        let wait = if kept.is_empty() { left } else { Some(zero) };
        let polling = if signals.is_empty() {
            None
        } else {
            let watched = signals
                .iter()
                .map(|(_, description, seen)| (Arc::clone(description), *seen))
                .collect();
            match super::signalfd::Polling::begin(watched) {
                Ok(polling) => polling,
                Err(error) => return -(error as i64),
            }
        };
        // SAFETY: output array with capacity `fetch`.
        let mut fetch_events = |wait: Option<&libc::timespec>| unsafe {
            libc::kevent(
                epfd,
                std::ptr::null(),
                0,
                kevs.as_mut_ptr(),
                fetch as i32,
                wait.map_or(std::ptr::null(), |t| t as *const _),
            )
        };
        let old_mask = user_try!(super::poll::swap_sigmask(mask));
        // With signalfds the wait is on the kqueue and the wake token, then
        // the kqueue is read without waiting.
        let (n, woke) = match &polling {
            None => {
                let n = fetch_events(wait.as_ref());
                (n, n != 0)
            }
            Some(polling) => {
                polling.sync();
                let mut fds = [
                    libc::pollfd {
                        fd: epfd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: polling.fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: a pollfd array we own.
                match unsafe { libc::poll(fds.as_mut_ptr(), 2, poll_ms(wait)) } {
                    woke if woke <= 0 => (woke, woke != 0),
                    _ => (fetch_events(Some(&zero)), true),
                }
            }
        };
        let err = errno::last();
        super::poll::restore_sigmask(old_mask);
        if n < 0 {
            return -(err as i64);
        }
        // SAFETY: the kernel filled n entries.
        unsafe { kevs.set_len(n as usize) };
        let (found, rearm) = ready(&ep, &kevs, maxevents as usize, &signals, &kept);
        apply(epfd, &rearm);
        // An inotify fd that woke the wait with nothing to read, or an
        // unconnected socket another waiter reported or a change removed
        // since it was found: wait on.
        if !found.is_empty()
            || !woke && kept.is_empty()
            || left.is_some_and(|t| t.tv_sec == 0 && t.tv_nsec == 0)
        {
            break found;
        }
    };
    for (n, (_, b, data)) in out.iter().enumerate() {
        // SAFETY: guest array of `maxevents` 16-byte struct epoll_event.
        user_write!(events + n as u64 * 16, [u64; 2], [*b as u64, *data]);
    }
    trace_wait(epfd, &kevs, &out);
    out.len() as i64
}

fn now_ns() -> i64 {
    super::clock::Base::Monotonic.now() as i64
}

/// The ready interests among `kevs` as (fd, events, data), at most
/// `limit` of them with each fd's filters in one event, as epoll's ready
/// list gives them; and the changes that disable the other filter of the
/// one-shot ones and re-arm the fds past `limit`, which stay ready for the
/// next wait as on Linux (a retrieved edge-triggered or dispatched knote
/// would otherwise not fire again), and the one-shot ones whose readiness
/// was stale, which `ep_item_poll` leaves armed. The `signals` interests
/// are read as the waiter reads them, whether their socket fired or not,
/// and the `unconnected` ones as `tcp_poll` reports them.
fn ready(
    ep: &Epoll,
    kevs: &[libc::kevent],
    limit: usize,
    signals: &[(i32, Arc<super::signalfd::SignalFd>, u64)],
    kept: &[(i32, super::net::Kept)],
) -> (Vec<(i32, u32, u64)>, Vec<libc::kevent>) {
    let mut out: Vec<(i32, u32, u64)> = Vec::with_capacity(kevs.len().min(limit));
    let mut rearm = Vec::new();
    let mut rearmed = Vec::new();
    // Closed after the locks.
    let mut retired = Vec::new();
    let mut interest = ep.interest.lock().unwrap();
    let mut reaps = ep.reaps.lock().unwrap();
    for k in kevs {
        if !k.udata.is_null() {
            continue;
        }
        let ident = k.ident as i32;
        // A pidfd's knote, or its interest's alias of its reap watch.
        let (fd, reap) = if interest.contains_key(&ident) {
            (ident, false)
        } else {
            let owner = reaps
                .iter()
                .find(|(_, alias)| alias.fd() == ident)
                .map(|(&fd, alias)| (fd, alias.current(fd)));
            match owner {
                Some((fd, true)) => (fd, true),
                // The pidfd's fd was closed, dropping its own knote, while
                // a dup keeps the watch open: closing the alias drops this
                // one.
                Some((fd, false)) => {
                    retired.extend(reaps.remove(&fd));
                    continue;
                }
                None => continue,
            }
        };
        let Some(i) = interest.get_mut(&fd) else {
            continue;
        };
        let fired = if !reap {
            bits(k, i)
        } else if super::wait::pidfd_reaped(fd) {
            // pidfd_poll: a reaped process's pidfd is readable and hung up.
            EPOLLHUP | i.events & (EPOLLIN | EPOLLRDNORM)
        } else {
            // The pidfd was closed, which dropped its own knote.
            rearm.push(kev(ident, k.filter, libc::EV_DELETE));
            continue;
        };
        let mut b = fired;
        if b & EPOLLIN != 0 && super::inotify::readable(fd) == Some(false) {
            b &= !(EPOLLIN | EPOLLRDNORM);
        }
        if signals.iter().any(|signal| signal.0 == fd) {
            b = 0;
        }
        if b == 0 {
            // A dispatched knote that fired for nothing (a hangup-only one
            // on a full buffer) waits on.
            if (fired != 0 || i.hangup_only()) && i.events & EPOLLONESHOT != 0 && !i.disabled {
                rearm.push(kev(ident, k.filter, libc::EV_ENABLE));
            }
            continue;
        }
        if let Some(e) = out.iter_mut().find(|e| e.0 == fd) {
            e.1 |= b;
            continue;
        }
        if out.len() == limit {
            if !rearmed.contains(&fd) {
                rearmed.push(fd);
                rearm.extend(changes(
                    fd,
                    reaps.get(&fd).map(ReapAlias::fd),
                    None,
                    Some(i),
                ));
            }
            continue;
        }
        if i.events & EPOLLONESHOT != 0 {
            i.disabled = true;
            let other = if k.filter == libc::EVFILT_READ {
                libc::EVFILT_WRITE
            } else {
                libc::EVFILT_READ
            };
            if (other == libc::EVFILT_READ && i.wants_read())
                || (other == libc::EVFILT_WRITE && i.wants_write())
            {
                rearm.push(kev(fd, other, libc::EV_DISABLE));
            }
            if reap {
                if i.wants_read() {
                    rearm.push(kev(fd, libc::EVFILT_READ, libc::EV_DISABLE));
                }
            } else if let Some(reap) = reaps.get(&fd) {
                rearm.push(kev(reap.fd(), libc::EVFILT_READ, libc::EV_DISABLE));
            }
        } else if i.hangup_only() && i.events & EPOLLET == 0 {
            rearm.extend(changes(
                fd,
                reaps.get(&fd).map(ReapAlias::fd),
                None,
                Some(i),
            ));
        }
        out.push((fd, b, i.data));
    }
    // `signalfd_poll` in the waiter's context, which also drops a token a
    // mask another process changed made stale.
    for (fd, description, _) in signals {
        let Some(i) = interest.get_mut(fd) else {
            continue;
        };
        let notified = super::signalfd::notified();
        if i.disabled
            || i.events & EPOLLET != 0 && notified <= i.seen
            || out.len() == limit
            || !super::signalfd::ready(description)
        {
            continue;
        }
        i.seen = notified;
        if i.events & EPOLLONESHOT != 0 {
            i.disabled = true;
            rearm.push(kev(*fd, libc::EVFILT_READ, libc::EV_DISABLE));
        }
        out.push((*fd, EPOLLIN, i.data));
    }
    for (fd, kept) in kept {
        let fd = *fd;
        let Some(i) = interest.get_mut(&fd) else {
            continue;
        };
        let b = kept_bits(kept, i);
        if let Some(e) = out.iter_mut().find(|e| e.0 == fd) {
            e.1 |= b;
            continue;
        }
        // Read again under the lock: another waiter may have reported it.
        if b == 0 || i.disabled || i.edge_reported(kept.generation) || out.len() == limit {
            continue;
        }
        i.reported = Some(kept.generation);
        if i.events & EPOLLONESHOT != 0 {
            i.disabled = true;
            rearm.extend(changes(fd, None, None, Some(&*i)));
        }
        out.push((fd, b, i.data));
    }
    drop(interest);
    use std::os::fd::AsRawFd;
    let mut retired = Vec::new();
    let mut ptys = ep.ptys.lock().unwrap();
    for index in (0..ptys.len()).rev() {
        if ptys[index].watch.observation.status.retired() {
            retired.push(ptys.remove(index));
            continue;
        }
        let pty = &mut ptys[index];
        if pty.interest.disabled {
            continue;
        }
        let data = pty.watch.observation.data().as_raw_fd();
        let notification = pty
            .watch
            .observation
            .notification()
            .map(|fd| fd.as_raw_fd());
        let mut bits = 0;
        let mut hit = false;
        for event in kevs {
            if event.udata as u64 != pty.tag {
                continue;
            }
            if event.ident as i32 == data {
                hit = true;
                bits |= self::bits(event, &pty.interest);
            } else if Some(event.ident as i32) == notification {
                hit = true;
            }
        }
        if !hit {
            continue;
        }
        if pty.watch.observation.status.hangup() {
            bits |= EPOLLHUP;
            if pty.watch.side == aim_storage::pty_owner::Side::Slave {
                bits |= (EPOLLIN | EPOLLOUT) & pty.interest.events | EPOLLERR;
            }
        }
        if bits == 0 {
            continue;
        }
        if out.len() == limit {
            rearm.extend(pty_changes(&pty.watch, pty.tag, None, Some(&pty.interest)));
            continue;
        }
        if pty.interest.events & EPOLLONESHOT != 0 {
            pty.interest.disabled = true;
            rearm.extend(pty_changes(
                &pty.watch,
                pty.tag,
                Some(&Interest {
                    disabled: false,
                    ..pty.interest
                }),
                Some(&pty.interest),
            ));
        }
        out.push((pty.fd, bits, pty.interest.data));
    }
    drop(ptys);
    drop(retired);
    (out, rearm)
}

pub fn epoll_pwait(a: [u64; 6]) -> i64 {
    let timeout = (a[3] as i32 >= 0).then(|| {
        let ms = a[3] as i32 as i64;
        libc::timespec {
            tv_sec: ms / 1000,
            tv_nsec: (ms % 1000) * 1_000_000,
        }
    });
    wait(a[0] as i32, a[1], a[2] as i32, timeout, a[4])
}

pub fn epoll_pwait2(a: [u64; 6]) -> i64 {
    let timeout = if a[3] != 0 {
        // SAFETY: guest struct timespec.
        let t = user_read!(a[3], [i64; 2]);
        Some(libc::timespec {
            tv_sec: t[0],
            tv_nsec: t[1],
        })
    } else {
        None
    };
    if timeout.is_some_and(|t| t.tv_sec < 0 || !(0..1_000_000_000).contains(&t.tv_nsec)) {
        return -(EINVAL as i64);
    }
    wait(a[0] as i32, a[1], a[2] as i32, timeout, a[4])
}

/// Fork: an epoll is its interest list (its kqueue is made again in the
/// child, by [`after_fork_child`]).
pub(super) fn save(ep: &Epoll, w: &mut super::fork_state::Writer) {
    let interest = ep.interest.lock().unwrap();
    w.seq(interest.iter(), |w, (fd, i)| {
        w.i32(*fd);
        w.u32(i.events);
        w.u64(i.data);
        w.bool(i.disabled);
    });
    drop(interest);
    let ptys = ep.ptys.lock().unwrap();
    w.seq(ptys.iter(), |w, pty| {
        w.i32(pty.fd);
        w.u64(pty.tag);
        w.bytes(&pty.identity.to_bytes());
        w.u32(pty.interest.events);
        w.u64(pty.interest.data);
        w.bool(pty.interest.disabled);
        pty.watch.save(w);
    });
}

pub(super) fn load(r: &mut super::fork_state::Reader) -> Arc<Epoll> {
    let interest = r.seq(|r| {
        (
            r.i32(),
            Interest {
                events: r.u32(),
                data: r.u64(),
                disabled: r.bool(),
                seen: 0,
                reported: None,
            },
        )
    });
    let ptys = r.seq(|r| {
        let fd = r.i32();
        let tag = r.u64();
        let identity = aim_storage::inode_lease::Identity::from_bytes(&r.bytes());
        let interest = Interest {
            events: r.u32(),
            data: r.u64(),
            disabled: r.bool(),
            seen: 0,
            reported: None,
        };
        let watch = super::pty_owner::Watch::restore(r);
        let valid = match (identity, watch) {
            (Ok(identity), Ok(watch)) if tag > 0 && tag < u64::MAX => {
                match aim_storage::inode_lease::Identity::from_fd(
                    watch.observation.status.descriptor(),
                ) {
                    Ok(actual) if actual == identity => {
                        NEXT_PTY_TAG.fetch_max(tag + 1, std::sync::atomic::Ordering::Relaxed);
                        Some(PtyInterest {
                            fd,
                            tag,
                            identity,
                            interest,
                            watch: Arc::new(watch),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if valid.is_none() {
            r.invalidate();
        }
        valid
    });
    Arc::new(Epoll {
        interest: Mutex::new(interest.into_iter().collect()),
        reaps: Mutex::new(HashMap::new()),
        ptys: Mutex::new(ptys.into_iter().flatten().collect()),
    })
}

/// Fork child: kqueues are not inherited. Make each epoll again on its fd
/// numbers, then re-register the interests (after all kqueues exist, since
/// an epoll may watch another).
pub fn after_fork_child() -> Result<(), errno::Errno> {
    rebuild(
        fdtab::fds_where(|k| matches!(k, Kind::Epoll(_)))
            .into_iter()
            .filter_map(|(fd, k)| match k {
                Kind::Epoll(ep) => Some((fd, ep)),
                _ => None,
            })
            .collect(),
    )
}

fn rebuild(epolls: Vec<(i32, Arc<Epoll>)>) -> Result<(), errno::Errno> {
    // One kqueue per epoll, duplicated onto every fd that refers to it.
    let mut made: Vec<(i32, Arc<Epoll>)> = Vec::new();
    for (fd, ep) in epolls {
        let src = made.iter().find(|(_, e)| Arc::ptr_eq(e, &ep)).map(|m| m.0);
        // SAFETY: fd numbers the parent used for epolls, held by
        // placeholders (`fork::spawn::reserve_fds`).
        unsafe {
            let kq = match src {
                Some(s) => s,
                None => libc::kqueue(),
            };
            if kq < 0 {
                return Err(errno::last());
            }
            let result = fdtab::restore_kqueue(fd, kq);
            if kq != fd && src.is_none() {
                libc::close(kq);
            }
            result?;
        }
        if src.is_none() {
            made.push((fd, ep));
        }
    }
    for (fd, ep) in made {
        let mut interest = ep.interest.lock().unwrap_or_else(|e| e.into_inner());
        let mut reaps = ep.reaps.lock().unwrap_or_else(|e| e.into_inner());
        for &t in interest.keys() {
            if let Some(alias) = ReapAlias::new(t).transpose()? {
                reaps.insert(t, alias);
            }
        }
        interest.retain(|&t, i| {
            let reap = reaps.get(&t).map(ReapAlias::fd);
            apply(fd, &changes(t, reap, None, Some(i))) == 0
        });
        reaps.retain(|t, _| interest.contains_key(t));
        drop(reaps);
        drop(interest);
        for pty in ep.ptys.lock().unwrap().iter() {
            let result = apply(
                fd,
                &pty_changes(&pty.watch, pty.tag, None, Some(&pty.interest)),
            );
            if result < 0 {
                return Err(-result as errno::Errno);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fork_state::{Reader, Writer};

    /// A fork child makes the epoll again on its fd number from the
    /// parent's state, with its interest list.
    #[test]
    fn an_epoll_is_rebuilt_from_its_saved_state() {
        fdtab::init();
        let ep = epoll_create1([0; 6]) as i32;
        let efd = super::super::event::eventfd2([0, 0o4000, 0, 0, 0, 0]) as i32;
        assert!(ep >= 0 && efd >= 0);
        let ev = [EPOLLIN as u64, 0x55];
        let add = [
            ep as u64,
            EPOLL_CTL_ADD,
            efd as u64,
            ev.as_ptr() as u64,
            0,
            0,
        ];
        assert_eq!(epoll_ctl(add), 0);
        let Some(Kind::Epoll(e)) = fdtab::get(ep) else {
            panic!("no epoll")
        };
        let mut w = Writer::default();
        save(&e, &mut w);
        let saved = w.into_bytes();
        // As in the child: the number is held by a placeholder.
        // SAFETY: replacing our own kqueue fd with /dev/null.
        unsafe {
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
            libc::dup2(null, ep);
            libc::close(null);
        }
        let loaded = load(&mut Reader::new(&saved));
        fdtab::insert(ep, Kind::Epoll(loaded.clone()));
        rebuild(vec![(ep, loaded)]).unwrap();
        let one = 1u64;
        super::super::event::write(efd, &one as *const u64 as u64, 8);
        let mut out = [0u64; 2];
        let n = epoll_pwait([ep as u64, out.as_mut_ptr() as u64, 1, 1000, 0, 0]);
        assert_eq!((n, out[1]), (1, 0x55));
        for fd in [ep, efd] {
            fdtab::on_close(fd);
            // SAFETY: our test fds.
            unsafe { libc::close(fd) };
        }
    }
}
