//! ppoll and pselect6 over Darwin poll. Every guest fd (eventfd, timerfd,
//! epoll and inotify included) is a host fd whose poll readiness has Linux
//! meaning for the events its Linux `_poll` can report ([`reportable`]),
//! so only those are asked and the event bits and timeouts translated; a
//! signalfd is also readable for the polling thread's own signals, which
//! its wake token reports. A socket or pipe polled for no event, which
//! Darwin's poll watches for nothing, is watched by [`Hangups`] for the
//! hangup and error Linux poll always reports.

use crate::errno::{self, EBADF, EINVAL};
use std::os::fd::AsRawFd;

const POLLIN: i16 = 0x1;
const POLLPRI: i16 = 0x2;
const POLLOUT: i16 = 0x4;
const POLLERR: i16 = 0x8;
const POLLHUP: i16 = 0x10;
const POLLNVAL: i16 = 0x20;
const POLLRDNORM: i16 = 0x40;
const POLLRDBAND: i16 = 0x80;
const POLLWRNORM: i16 = 0x100;
const POLLWRBAND: i16 = 0x200;
const POLLRDHUP: i16 = 0x2000;

/// Bits with the same value on both kernels.
const SAME: i16 =
    POLLIN | POLLPRI | POLLOUT | POLLERR | POLLHUP | POLLNVAL | POLLRDNORM | POLLRDBAND;

fn to_host(e: i16) -> i16 {
    let mut h = e & SAME;
    if e & POLLWRNORM != 0 {
        h |= libc::POLLWRNORM;
    }
    if e & POLLWRBAND != 0 {
        h |= libc::POLLWRBAND;
    }
    if e & POLLRDHUP != 0 {
        h |= POLLIN;
    }
    h
}

fn from_host(h: i16, asked: i16) -> i16 {
    let mut e = h & SAME & (asked | POLLERR | POLLHUP | POLLNVAL);
    if h & libc::POLLOUT != 0 && asked & POLLWRNORM != 0 {
        e |= POLLWRNORM;
    }
    if h & libc::POLLWRBAND != 0 && asked & POLLWRBAND != 0 {
        e |= POLLWRBAND;
    }
    if h & POLLHUP != 0 && asked & POLLRDHUP != 0 {
        e |= POLLRDHUP;
    }
    e
}

/// The events guest fd `fd`'s Linux `_poll` can report, when its host
/// readiness has others: a signalfd or timerfd is only ever readable
/// (`signalfd_poll`, `timerfd_poll`), not writable as its socket is; an
/// eventfd is POLLIN and POLLOUT without their socket's POLLRDNORM and
/// POLLWRNORM (`eventfd_poll`); an epoll, inotify or pidfd is a kqueue,
/// which Darwin poll reports POLLNVAL for when asked a write event, and
/// Linux reports only readable (`ep_eventpoll_poll`, `inotify_poll`,
/// `pidfd_poll`, which adds POLLHUP once the process is reaped: the
/// pidfd's reap watch, [`super::wait::ReapWatch`]).
pub(super) fn reportable(kind: Option<&super::fdtab::Kind>) -> Option<i16> {
    use super::fdtab::Kind;
    match kind {
        Some(Kind::Signal(_) | Kind::Timer(_)) => Some(POLLIN),
        Some(Kind::Event(_)) => Some(POLLIN | POLLOUT),
        Some(Kind::Epoll(_) | Kind::Inotify(_) | Kind::PidFd(_)) => Some(POLLIN | POLLRDNORM),
        _ => None,
    }
}

/// The events of `asked` that guest fd `fd` can report, a signalfd's as
/// of a mask read now: those polled for, and the only ones its host
/// readiness is translated back to. A pipe or FIFO, `pipe`, is never
/// POLLPRI, POLLRDBAND or POLLWRBAND, which Darwin's is whenever readable
/// or writable, nor POLLRDHUP, and its read end never writable, which a
/// named FIFO's is on Darwin (`pipe_poll`).
fn asked_events(kind: Option<&super::fdtab::Kind>, pipe: Option<PipeEnd>, asked: i16) -> i16 {
    if let Some(super::fdtab::Kind::Signal(description)) = kind {
        super::signalfd::ready(description);
    }
    match pipe {
        Some(PipeEnd::Read) => {
            return asked & !(POLLPRI | POLLRDBAND | POLLWRBAND | POLLRDHUP | POLLOUT | POLLWRNORM);
        }
        Some(_) => return asked & !(POLLPRI | POLLRDBAND | POLLWRBAND | POLLRDHUP),
        None => {}
    }
    reportable(kind).map_or(asked, |events| asked & events)
}

/// Register the calling thread as a poller of the signalfds among `pins`
/// polled for POLLIN, the only event they report, whose readiness counts
/// its own pending signals, and add its wake token to `host`.
fn watch_signals(
    pins: &[Option<super::fdtab::Pinned>],
    host: &mut Vec<libc::pollfd>,
) -> Result<Option<super::signalfd::Polling>, crate::errno::Errno> {
    let watched: Vec<_> = pins
        .iter()
        .zip(host.iter())
        .filter(|(_, h)| h.events & POLLIN != 0)
        .filter_map(|(pin, _)| match pin.as_ref()?.kind() {
            Some(super::fdtab::Kind::Signal(description)) => Some((description.clone(), 0)),
            _ => None,
        })
        .collect();
    if watched.is_empty() {
        return Ok(None);
    }
    let polling = super::signalfd::Polling::begin(watched)?;
    if let Some(polling) = &polling {
        host.push(libc::pollfd {
            fd: polling.fd(),
            events: libc::POLLIN,
            revents: 0,
        });
    }
    Ok(polling)
}

/// A signalfd's host revents as the polling thread reads it now: its socket
/// shows only the process's pending signals.
fn signal_revents(pin: Option<&super::fdtab::Pinned>, host: &mut libc::pollfd) {
    if let Some(super::fdtab::Kind::Signal(description)) = pin.and_then(|pin| pin.kind()) {
        host.revents = if super::signalfd::ready(description) {
            POLLIN
        } else {
            0
        };
    }
}

/// Whether a guest's poll `events` ask Darwin's poll to watch nothing:
/// Linux poll reports POLLERR and POLLHUP whatever was asked, but Darwin's
/// registers a knote only for a read or write event, or POLLHUP.
fn eventless(events: i16) -> bool {
    to_host(events) & !(POLLERR | POLLHUP | POLLNVAL) == 0
}

fn is_fifo(fd: i32) -> bool {
    // SAFETY: fstat into a local struct.
    unsafe {
        let mut status: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut status) == 0 && status.st_mode & libc::S_IFMT == libc::S_IFIFO
    }
}

/// The ends of a pipe or FIFO a descriptor holds, whose Darwin readiness
/// differs from `pipe_poll`'s.
#[derive(Clone, Copy)]
pub(super) enum PipeEnd {
    Read,
    Write,
    /// A FIFO open for both.
    Both,
}

/// Which ends of a pipe or FIFO host `fd` holds, by its access mode.
fn pipe_end(fd: i32) -> Option<PipeEnd> {
    if !is_fifo(fd) {
        return None;
    }
    // SAFETY: F_GETFL on a host descriptor.
    match unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_ACCMODE {
        libc::O_RDONLY => Some(PipeEnd::Read),
        libc::O_WRONLY => Some(PipeEnd::Write),
        _ => Some(PipeEnd::Both),
    }
}

/// The pipe ends of an untyped guest fd on its host descriptor `host`: a
/// typed one's host socket or kqueue (a pidfd's, which Darwin stats as a
/// FIFO, included) is never a FIFO.
pub(super) fn pipe(host: i32, kind: Option<&super::fdtab::Kind>) -> Option<PipeEnd> {
    kind.is_none().then(|| pipe_end(host)).flatten()
}

/// The bytes queued in pipe read end `fd`.
fn pipe_queued(fd: i32) -> i32 {
    let mut queued = 0;
    // SAFETY: FIONREAD into a local int.
    unsafe { libc::ioctl(fd, libc::FIONREAD, &mut queued) };
    queued
}

/// A hung-up pipe end's Darwin poll `revents` as `pipe_poll` reports
/// them. A read end is readable only with data queued, where Darwin's is
/// also once its writers are gone. A write end whose readers are gone,
/// which Darwin reports as hung up and, when asked, readable, is POLLERR
/// and writable alone. Linux keeps the buffer, so a full one is not
/// writable; Darwin frees it with its last reader, and that reader's
/// description, which may close in any process sharing it, exit and
/// SIGKILL included, was the only record of the count: the write end's
/// `st_size`, FIONREAD and EVFILT_WRITE data are all 0 afterwards (#2815).
fn pipe_revents(end: PipeEnd, fd: i32, revents: i16) -> i16 {
    match end {
        PipeEnd::Read if pipe_queued(fd) == 0 => revents & !(POLLIN | POLLRDNORM),
        PipeEnd::Write => POLLERR | libc::POLLOUT,
        PipeEnd::Read | PipeEnd::Both => revents,
    }
}

/// Whether `pin`, on host `fd`, is a TCP socket never connected.
fn unconnected(pin: Option<&super::fdtab::Pinned>, fd: i32) -> bool {
    matches!(
        pin.and_then(|pin| pin.kind()),
        Some(super::fdtab::Kind::Sock(s)) if super::net::unconnected(s, fd)
    )
}

/// The `revents` of guest fd `fd`, pinned as `pin`, for `asked`, from its
/// host descriptor's Darwin poll `h`: a socket's as `tcp_poll`,
/// `unix_poll`, `datagram_poll` and `sock_poll` report it.
fn guest_revents(fd: i32, pin: Option<&super::fdtab::Pinned>, h: &libc::pollfd, asked: i16) -> i16 {
    // `TCP_CLOSE` without a shutdown is hung up and writable.
    if unconnected(pin, h.fd) {
        return POLLHUP | asked & (POLLOUT | POLLWRNORM);
    }
    let mut revents = match (h.revents & POLLHUP != 0)
        .then(|| {
            super::tcp_shutdown::polled_hangup(h.fd).or_else(|| super::net::shut_directions(h.fd))
        })
        .flatten()
    {
        // A socket's shutdown, which Darwin reports as HUP alone, with
        // POLLPRI and POLLRDBAND, as tcp_poll, unix_poll and datagram_poll
        // report it: by direction, and never those two.
        Some(tcp) => {
            let mut revents = h.revents & !(POLLHUP | POLLPRI | POLLRDBAND);
            if tcp.receive {
                revents |= POLLIN | POLLRDNORM;
            }
            if tcp.send || super::tcp_shutdown::writable(h.fd) {
                revents |= libc::POLLOUT;
            }
            if tcp.hangup() {
                revents |= POLLHUP;
            }
            from_host(revents, asked) | if tcp.receive { asked & POLLRDHUP } else { 0 }
        }
        None => from_host(h.revents, asked),
    };
    if h.revents & libc::POLLIN != 0 && super::wait::pidfd_reaped(fd) {
        revents |= POLLHUP;
    }
    // POLLERR for a pending socket error, which Darwin reports as
    // readable, writable or not at all; readable only with data queued or
    // at EOF. Darwin's queued count includes each datagram's address, so a
    // zero-length one counts.
    if pin.is_some() && (h.revents != 0 || h.events & (POLLIN | libc::POLLOUT) == 0) {
        if let Some(queued) = super::net::pending_error(h.fd) {
            revents |= POLLERR;
            if queued == 0 && h.revents & POLLHUP == 0 {
                revents &= !(POLLIN | POLLRDNORM);
            }
        }
    }
    revents
}

/// The sockets and pipes polled for no event, as EVFILT_READ knotes of a
/// kqueue of the call, polled with the others: their low-water mark out of
/// reach, they fire for the EOF and the pending error that `sock_poll` and
/// `pipe_poll` report whatever was asked (filt_soread, filt_piperead, a
/// write end's at its readers' close), and for a full socket buffer, which
/// Darwin caps the mark at. They are edge-triggered, so that a firing that
/// reports nothing (a full buffer, a half-close) is waited past, not
/// spun on, while a later one (the reset that follows) still wakes.
struct Hangups {
    kq: std::os::fd::OwnedFd,
    /// The poll set's entries watched, one knote for those of one socket.
    entries: Vec<usize>,
}

impl Hangups {
    /// Watch host socket or pipe `fd`, the poll set's `index`th entry.
    fn watch(hangups: &mut Option<Self>, fd: i32, index: usize) -> Result<(), crate::errno::Errno> {
        let this = match hangups {
            Some(hangups) => hangups,
            None => {
                // SAFETY: a new kqueue, owned from here.
                let kq = unsafe { libc::kqueue() };
                if kq < 0 {
                    return Err(errno::last());
                }
                use std::os::fd::FromRawFd;
                // SAFETY: the kqueue just created.
                hangups.insert(Self {
                    kq: unsafe { std::os::fd::OwnedFd::from_raw_fd(kq) },
                    entries: Vec::new(),
                })
            }
        };
        this.entries.push(index);
        let change = libc::kevent {
            ident: fd as usize,
            filter: libc::EVFILT_READ,
            flags: libc::EV_ADD | libc::EV_CLEAR | libc::EV_RECEIPT,
            fflags: libc::NOTE_LOWAT,
            data: isize::MAX,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one change, its receipt into a local event.
        let mut receipt: libc::kevent = unsafe { std::mem::zeroed() };
        let n = unsafe {
            libc::kevent(
                this.kq.as_raw_fd(),
                &change,
                1,
                &mut receipt,
                1,
                std::ptr::null(),
            )
        };
        if n < 0 {
            return Err(errno::last());
        }
        match receipt.data {
            0 => Ok(()),
            error => Err(errno::from_darwin(error as i32)),
        }
    }

    /// Take the knotes fired since the last pass, adding HUP to the host
    /// revents of each entry of a socket or pipe at EOF: what Darwin's poll
    /// reports for it.
    fn fired(&self, host: &mut [libc::pollfd]) -> Result<(), crate::errno::Errno> {
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        loop {
            // SAFETY: an event array we own.
            let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
            let n = unsafe {
                libc::kevent(
                    self.kq.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as i32,
                    &zero,
                )
            };
            if n < 0 {
                return Err(errno::last());
            }
            for event in &events[..n as usize] {
                if event.flags & libc::EV_EOF == 0 {
                    continue;
                }
                for &index in &self.entries {
                    if host[index].fd as usize == event.ident {
                        host[index].revents |= POLLHUP;
                    }
                }
            }
            if (n as usize) < events.len() {
                return Ok(());
            }
        }
    }

    fn pollfd(&self) -> libc::pollfd {
        libc::pollfd {
            fd: self.kq.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }
    }
}

/// An inotify's host revents as inotify_poll reads them: its kqueue is
/// also readable for host notes that decode to no event.
fn inotify_revents(host: &mut libc::pollfd) {
    if host.revents & libc::POLLIN != 0 && super::inotify::readable(host.fd) == Some(false) {
        host.revents &= !(libc::POLLIN | libc::POLLRDNORM);
    }
}

/// Install `mask` (a guest sigset pointer, 0 for none) for the duration of a
/// wait; returns the mask to restore.
pub fn swap_sigmask(mask: u64) -> Result<Option<u64>, i32> {
    if mask == 0 {
        return Ok(None);
    }
    let mut old = 0u64;
    let result = super::signal::rt_sigprocmask([2, mask, &mut old as *mut u64 as u64, 8, 0, 0]);
    if result < 0 {
        return Err(-result as i32);
    }
    Ok(Some(old))
}

pub fn restore_sigmask(old: Option<u64>) {
    if let Some(o) = old {
        super::signal::rt_sigprocmask([2, &o as *const u64 as u64, 0, 8, 0, 0]);
    }
}

/// Milliseconds for a guest timespec (rounded up), -1 for none.
fn timeout_ms(ts: u64) -> Result<i32, i64> {
    if ts == 0 {
        return Ok(-1);
    }
    // SAFETY: guest struct timespec.
    let t = user_read!(ts, [i64; 2]);
    if t[0] < 0 || !(0..1_000_000_000).contains(&t[1]) {
        return Err(-(EINVAL as i64));
    }
    let ms = t[0]
        .saturating_mul(1000)
        .saturating_add((t[1] + 999_999) / 1_000_000);
    Ok(ms.min(i32::MAX as i64) as i32)
}

fn now_ns() -> i64 {
    super::clock::Base::Monotonic.now() as i64
}

/// Write back the time left of a guest timespec after waiting since `t0`.
fn update_timeout(ts: u64, t0: i64) -> Result<(), crate::errno::Errno> {
    if ts == 0 {
        return Ok(());
    }
    // SAFETY: guest struct timespec.
    {
        let t = user_read!(ts, [i64; 2]);
        let left = (t[0] * 1_000_000_000 + t[1] - (now_ns() - t0)).max(0);
        user_write!(ts, [i64; 2], [left / 1_000_000_000, left % 1_000_000_000]);
    }
    Ok(())
}

fn host_poll(fds: &mut [libc::pollfd], ms: i32, mask: u64) -> i64 {
    let old = user_try!(swap_sigmask(mask));
    // SAFETY: a pollfd array we own.
    let fuse_ready = fds.iter().any(|descriptor| {
        super::fuse_device::readiness(descriptor.fd).is_some_and(|events| {
            events.is_ok_and(|events| events & (descriptor.events | POLLERR | POLLHUP) != 0)
        })
    });
    let n = unsafe {
        libc::poll(
            fds.as_mut_ptr(),
            fds.len() as u32,
            if fuse_ready { 0 } else { ms },
        )
    };
    let err = errno::last();
    restore_sigmask(old);
    if n < 0 { -(err as i64) } else { n as i64 }
}
#[cfg(test)]
thread_local! {pub(super) static BEFORE_WAIT:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const{std::cell::RefCell::new(None)};}

pub fn ppoll(a: [u64; 6]) -> i64 {
    let (fds, nfds, ts, mask) = (a[0], a[1], a[2], a[3]);
    if nfds > 1 << 20 {
        return -(EINVAL as i64);
    }
    let ms = match timeout_ms(ts) {
        Ok(ms) => ms,
        Err(e) => return e,
    };
    // struct pollfd { int fd; short events; short revents; } on both.
    // SAFETY: guest pollfd array of nfds entries.
    let mut guest = Vec::with_capacity(nfds as usize);
    for index in 0..nfds {
        guest.push(user_read!(fds + index * 8, libc::pollfd));
    }
    let mut pins = Vec::with_capacity(guest.len());
    let mut ptys = Vec::with_capacity(guest.len());
    let mut reaps = Vec::with_capacity(guest.len());
    let mut pipes = Vec::with_capacity(guest.len());
    let mut hangups = None;
    let mut invalid = false;
    let mut host = Vec::with_capacity(guest.len());
    for entry in guest.iter_mut() {
        let mut pin = if entry.fd < 0 {
            None
        } else {
            match super::fdtab::pin_guest(entry.fd) {
                Ok(pin) if !matches!(pin.kind(), Some(super::fdtab::Kind::Path(_))) => Some(pin),
                Ok(_) => {
                    invalid = true;
                    None
                }
                Err(EBADF) => {
                    invalid = true;
                    None
                }
                Err(error) => return -(error as i64),
            }
        };
        let pty = match pin.as_ref().and_then(|pin| pin.kind()) {
            Some(super::fdtab::Kind::Pty(description)) => match description.watch() {
                Ok(watch) => Some(watch),
                Err(error) => return -(error as i64),
            },
            _ => None,
        };
        let fd = pty.as_ref().map_or_else(
            || pin.as_ref().map_or(-1, |pin| pin.descriptor().as_raw_fd()),
            |watch| watch.observation.data().as_raw_fd(),
        );
        let kind = pin.as_ref().and_then(|pin| pin.kind());
        let pipe = pin.as_ref().and_then(|_| self::pipe(fd, kind));
        entry.events = asked_events(kind, pipe, entry.events);
        let socket = matches!(
            pin.as_ref().and_then(|pin| pin.kind()),
            Some(super::fdtab::Kind::Sock(_))
        );
        if pty.is_some() {
            pin = None;
        }
        if entry.events & POLLIN != 0 && fd >= 0 {
            super::binder::poll(fd);
        }
        // A pidfd polled for no event reports only its reap, which its
        // reap watch polls readable for: the pidfd is from the exit on.
        let reap = (entry.events & (POLLIN | POLLRDNORM) == 0)
            .then(|| super::wait::reap_watch(entry.fd))
            .flatten();
        host.push(match &reap {
            Some((_, watch)) => libc::pollfd {
                fd: *watch,
                events: libc::POLLIN,
                revents: 0,
            },
            // A socket's or pipe's, its knote in `hangups` instead.
            None if eventless(entry.events) && (socket || pipe.is_some()) => {
                if let Err(error) = Hangups::watch(&mut hangups, fd, host.len()) {
                    return -(error as i64);
                }
                libc::pollfd {
                    fd,
                    events: 0,
                    revents: 0,
                }
            }
            None => libc::pollfd {
                fd,
                events: to_host(entry.events),
                revents: 0,
            },
        });
        reaps.push(reap);
        pipes.push(pipe);
        pins.push(pin);
        ptys.push(pty);
    }
    for watch in ptys.iter().flatten() {
        let Some(notification) = watch.observation.notification() else {
            return -(crate::errno::EIO as i64);
        };
        host.push(libc::pollfd {
            fd: notification.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
    }
    host.extend(hangups.as_ref().map(Hangups::pollfd));
    let polling = match watch_signals(&pins, &mut host) {
        Ok(polling) => polling,
        Err(error) => return -(error as i64),
    };
    if pins.iter().flatten().any(super::close_effects::observes) {
        if let Err(error) = super::close_effects::observe_socket() {
            return -(error as i64);
        }
    }
    let t0 = now_ns();
    #[cfg(test)]
    BEFORE_WAIT.with(|hook| {
        let action = hook.borrow_mut().take();
        if let Some(action) = action {
            action();
        }
    });
    loop {
        let left = if ms < 0 {
            -1
        } else {
            (ms as i64 - (now_ns() - t0) / 1_000_000).max(0) as i32
        };
        if let Some(polling) = &polling {
            polling.sync();
        }
        // Darwin poll waits for a socket's pending error only through a
        // read or write event (filt_soread, filt_sowrite), which
        // `sock_poll` reports whatever was asked, and never reports an
        // unconnected TCP socket. Checked on each pass, as another thread
        // may take the error, or connect, before it is reported.
        let errored = host.iter().zip(&pins).any(|(h, pin)| {
            (pin.is_some()
                && h.events & (POLLIN | libc::POLLOUT) == 0
                && super::net::pending_error(h.fd).is_some())
                || unconnected(pin.as_ref(), h.fd)
        });
        let n = host_poll(&mut host, if invalid || errored { 0 } else { left }, mask);
        if n < 0 {
            return n;
        }
        if let Some(hangups) = &hangups {
            if let Err(error) = hangups.fired(&mut host) {
                return -(error as i64);
            }
        }
        let mut ready = 0;
        for ((((g, h), pin), pty), pipe) in guest
            .iter_mut()
            .zip(&mut host)
            .zip(&pins)
            .zip(&ptys)
            .zip(&pipes)
        {
            if g.fd >= 0 && pin.is_none() && pty.is_none() {
                g.revents = POLLNVAL;
                ready += 1;
                continue;
            }
            signal_revents(pin.as_ref(), h);
            if let Some(watch) = pty {
                h.revents = watch.events(h.revents);
            }
            inotify_revents(h);
            if let Some(events) = super::fuse_device::readiness(h.fd) {
                h.revents = match events {
                    Ok(events) => events,
                    Err(_) => POLLERR,
                };
            }
            if let Some(end) = pipe.filter(|_| h.revents & POLLHUP != 0) {
                h.revents = pipe_revents(end, h.fd, h.revents);
            }
            g.revents = guest_revents(g.fd, pin.as_ref(), h, g.events);
            if g.revents != 0 {
                ready += 1;
            }
        }
        if ready > 0 || n == 0 && !errored || left == 0 {
            user_try!(update_timeout(ts, t0));
            for (index, entry) in guest.iter().enumerate() {
                user_write!(fds + index as u64 * 8 + 6, i16, entry.revents);
            }
            return ready;
        }
        for h in &mut host {
            h.revents = 0;
        }
    }
}

pub fn pselect6(a: [u64; 6]) -> i64 {
    let (nfds, sets, ts, sig) = (a[0] as usize, [a[1], a[2], a[3]], a[4], a[5]);
    if nfds > 1 << 20 {
        return -(EINVAL as i64);
    }
    let ms = match timeout_ms(ts) {
        Ok(ms) => ms,
        Err(e) => return e,
    };
    let mut input_sets = Vec::new();
    for set in sets {
        let bytes = if set == 0 {
            vec![0; nfds.div_ceil(64) * 8]
        } else {
            user_try!(super::user_memory::read_exact(set, nfds.div_ceil(64) * 8))
        };
        input_sets.push(bytes);
    }
    let wanted = [POLLIN, POLLOUT, POLLPRI];
    let mut host = Vec::new();
    let mut pins = Vec::new();
    let mut ptys = Vec::new();
    let mut guest_fds = Vec::new();
    let mut pipes = Vec::new();
    for fd in 0..nfds {
        let mut ev = 0;
        for (s, w) in input_sets.iter().zip(wanted) {
            if s[fd / 8] & (1 << (fd % 8)) != 0 {
                ev |= w;
            }
        }
        if ev != 0 {
            let pin = match super::fdtab::pin_guest(fd as i32) {
                Ok(pin) => pin,
                Err(error) => return -(error as i64),
            };
            if matches!(pin.kind(), Some(super::fdtab::Kind::Path(_))) {
                return -(EBADF as i64);
            }
            let pty = match pin.kind() {
                Some(super::fdtab::Kind::Pty(description)) => match description.watch() {
                    Ok(watch) => Some(watch),
                    Err(error) => return -(error as i64),
                },
                _ => None,
            };
            let host_fd = pty.as_ref().map_or_else(
                || pin.descriptor().as_raw_fd(),
                |watch| watch.observation.data().as_raw_fd(),
            );
            if ev & POLLIN != 0 {
                super::binder::poll(host_fd);
            }
            let pipe = pipe(host_fd, pin.kind());
            ev = to_host(asked_events(pin.kind(), pipe, ev));
            host.push(libc::pollfd {
                fd: host_fd,
                events: ev,
                revents: 0,
            });
            guest_fds.push(fd);
            pipes.push(pipe);
            pins.push(if pty.is_none() { Some(pin) } else { None });
            ptys.push(pty);
        }
    }
    for watch in ptys.iter().flatten() {
        let Some(notification) = watch.observation.notification() else {
            return -(crate::errno::EIO as i64);
        };
        host.push(libc::pollfd {
            fd: notification.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
    }
    // The 6th argument is { const sigset_t *ss; size_t ss_len; }.
    let mask = if sig == 0 {
        0
    } else {
        // SAFETY: guest struct.
        user_read!(sig, u64)
    };
    let polling = match watch_signals(&pins, &mut host) {
        Ok(polling) => polling,
        Err(error) => return -(error as i64),
    };
    if pins.iter().flatten().any(super::close_effects::observes) {
        if let Err(error) = super::close_effects::observe_socket() {
            return -(error as i64);
        }
    }
    let t0 = now_ns();
    loop {
        let left = if ms < 0 {
            -1
        } else {
            (ms as i64 - (now_ns() - t0) / 1_000_000).max(0) as i32
        };
        if let Some(polling) = &polling {
            polling.sync();
        }
        // Darwin never reports an unconnected TCP socket ready, which
        // POLLIN_SET and POLLOUT_SET hold (its HUP and OUT), not
        // POLLEX_SET.
        let unconnected = host.iter().zip(&pins).any(|(h, pin)| {
            h.events & (libc::POLLIN | libc::POLLOUT) != 0 && unconnected(pin.as_ref(), h.fd)
        });
        let n = host_poll(&mut host, if unconnected { 0 } else { left }, mask);
        if n < 0 {
            return n;
        }
        for descriptor in &mut host {
            if let Some(events) = super::fuse_device::readiness(descriptor.fd) {
                descriptor.revents = match events {
                    Ok(events) => events,
                    Err(_) => POLLERR,
                };
            }
        }
        for (((descriptor, watch), pin), pipe) in host.iter_mut().zip(&ptys).zip(&pins).zip(&pipes)
        {
            if let Some(watch) = watch {
                descriptor.revents = watch.events(descriptor.revents);
            }
            signal_revents(pin.as_ref(), descriptor);
            if let Some(end) = pipe.filter(|_| descriptor.revents & POLLHUP != 0) {
                descriptor.revents = pipe_revents(end, descriptor.fd, descriptor.revents);
            }
            inotify_revents(descriptor);
        }
        if host.iter().any(|p| p.revents & POLLNVAL != 0) {
            return -(EBADF as i64);
        }
        let mut output_sets = vec![vec![0u8; nfds.div_ceil(64) * 8]; 3];
        let mut count = 0;
        for ((p, &fd), pin) in host.iter().zip(&guest_fds).zip(&pins) {
            // POLLIN_SET, POLLOUT_SET and POLLEX_SET of what poll reports.
            let revents = guest_revents(fd as i32, pin.as_ref(), p, p.events);
            let hits = [
                revents & (POLLIN | POLLHUP | POLLERR) != 0,
                revents & (POLLOUT | POLLERR) != 0,
                revents & POLLPRI != 0,
            ];
            for (index, ((s, w), hit)) in sets.iter().zip(wanted).zip(hits).enumerate() {
                if *s != 0 && p.events & w != 0 && hit {
                    // SAFETY: guest fd_set word covering fd.
                    output_sets[index][fd / 8] |= 1 << (fd % 8);
                    count += 1;
                }
            }
        }
        // A wake token, a pty notification or an inotify note with nothing
        // ready: wait on.
        if count > 0 || n == 0 && !unconnected || left == 0 {
            user_try!(update_timeout(ts, t0));
            for (set, bytes) in sets.iter().zip(&output_sets) {
                if *set != 0 {
                    user_try!(super::user_memory::write_exact(*set, bytes));
                }
            }
            return count;
        }
        for descriptor in &mut host {
            descriptor.revents = 0;
        }
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;
    use std::os::fd::FromRawFd;

    #[test]
    fn poll_keeps_unknown_and_negative_fd_semantics_and_select_guest_indexes() {
        if super::super::fdtab::isolated_kernel_test(
            "sys::poll::pin_tests::poll_keeps_unknown_and_negative_fd_semantics_and_select_guest_indexes",
        ) {
            return;
        }
        let (_view, _root) = crate::vfs::test_view();
        let mut raw = [0; 2];
        assert_eq!(unsafe { libc::pipe(raw.as_mut_ptr()) }, 0);
        let read = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw[0]) };
        let write = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw[1]) };
        super::super::fdtab::publish_guest(read.as_raw_fd()).unwrap();
        let byte = b'x';
        assert_eq!(
            unsafe { libc::write(write.as_raw_fd(), (&byte as *const u8).cast(), 1) },
            1
        );
        let mut entries = [
            libc::pollfd {
                fd: read.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: write.as_raw_fd(),
                events: POLLOUT,
                revents: 0,
            },
            libc::pollfd {
                fd: -1,
                events: POLLIN,
                revents: 0,
            },
        ];
        let mut timeout = [0u64; 2];
        assert_eq!(
            ppoll([
                entries.as_mut_ptr() as u64,
                3,
                timeout.as_mut_ptr() as u64,
                0,
                0,
                0
            ]),
            2
        );
        assert_eq!(entries[0].revents, POLLIN);
        assert_eq!(entries[1].revents, POLLNVAL);
        assert_eq!(entries[2].revents, 0);
        let fd = read.as_raw_fd() as usize;
        let mut selected = vec![0u64; (fd + 1).div_ceil(64)];
        selected[fd / 64] |= 1 << (fd % 64);
        assert_eq!(
            pselect6([
                (fd + 1) as u64,
                selected.as_mut_ptr() as u64,
                0,
                0,
                timeout.as_mut_ptr() as u64,
                0
            ]),
            1
        );
        assert_eq!(selected[fd / 64], 1 << (fd % 64));
        super::super::fdtab::withdraw_guest(read.as_raw_fd()).unwrap();
        assert_eq!(
            pselect6([
                (fd + 1) as u64,
                selected.as_mut_ptr() as u64,
                0,
                0,
                timeout.as_mut_ptr() as u64,
                0
            ]),
            -(EBADF as i64)
        );
    }
}

#[cfg(test)]
mod reportable_tests {
    use super::*;
    use crate::sys::{epoll, event, fs, inotify, wait};

    const O_NONBLOCK: u64 = 0o4000;
    /// For a pipe whose ends' close a test awaits: a process spawned
    /// meanwhile by a test running in parallel (an isolated kernel test's,
    /// a sleep) would otherwise inherit and hold them open for its life.
    const O_CLOEXEC: u64 = 0o2000000;
    /// Every event bit a guest may ask poll for.
    const ALL: i16 = POLLIN | POLLPRI | POLLOUT | POLLRDNORM | POLLRDBAND | POLLWRNORM | POLLWRBAND;

    /// ppoll on one fd without waiting: (count, revents).
    fn polled(fd: i64, events: i16) -> (i64, i16) {
        let mut entry = libc::pollfd {
            fd: fd as i32,
            events,
            revents: 0,
        };
        let mut timeout = [0i64; 2];
        let n = ppoll([
            &mut entry as *mut _ as u64,
            1,
            timeout.as_mut_ptr() as u64,
            0,
            0,
            0,
        ]);
        (n, entry.revents)
    }

    /// pselect6 on one fd in the read and write sets, without waiting:
    /// (count, in the read set, in the write set).
    fn selected(fd: i64) -> (i64, bool, bool) {
        let (word, bit) = (fd as usize / 64, 1u64 << (fd % 64));
        let (mut read, mut write) = ([0u64; 16], [0u64; 16]);
        read[word] = bit;
        write[word] = bit;
        let mut timeout = [0i64; 2];
        let n = pselect6([
            fd as u64 + 1,
            read.as_mut_ptr() as u64,
            write.as_mut_ptr() as u64,
            0,
            timeout.as_mut_ptr() as u64,
            0,
        ]);
        (n, read[word] == bit, write[word] == bit)
    }

    /// A new epoll watching `fd` for `events`, with `fd` as data.
    fn watching(fd: i64, events: u32) -> i64 {
        let ep = epoll::epoll_create1([0; 6]);
        assert!(ep >= 0);
        let interest = [events as u64, fd as u64];
        assert_eq!(
            epoll::epoll_ctl([ep as u64, 1, fd as u64, interest.as_ptr() as u64, 0, 0]),
            0
        );
        ep
    }

    /// epoll_pwait on `ep` for up to `ms`: (count, events of the first).
    fn waited(ep: i64, ms: i64) -> (i64, u64) {
        let mut event = [0u64; 2];
        let n = epoll::epoll_pwait([ep as u64, event.as_mut_ptr() as u64, 1, ms as u64, 0, 0]);
        (n, event[0])
    }

    fn close(fds: &[i64]) {
        for &fd in fds {
            assert_eq!(fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
    }

    /// timerfd_poll: a timerfd is never writable, and readable only once
    /// it expired (LTP timerfd01, poll01).
    #[test]
    fn a_timerfd_is_only_readable() {
        let _view = crate::vfs::test_view();
        const MONOTONIC: u64 = 1;
        let fd = event::timerfd_create([MONOTONIC, O_NONBLOCK, 0, 0, 0, 0]);
        assert!(fd >= 0);
        let read_write = watching(fd, 0x1 | 0x4 | 0x100);
        let write_only = watching(fd, 0x4 | 0x100);
        assert_eq!(polled(fd, ALL), (0, 0));
        assert_eq!(selected(fd), (0, false, false));
        assert_eq!(waited(read_write, 0).0, 0);
        let spec = [0i64, 0, 0, 20_000_000];
        assert_eq!(
            event::timerfd_settime([fd as u64, 0, spec.as_ptr() as u64, 0, 0, 0]),
            0
        );
        assert_eq!(waited(read_write, 2_000), (1, 0x1));
        assert_eq!(polled(fd, ALL), (1, POLLIN));
        assert_eq!(polled(fd, POLLOUT | POLLWRNORM | POLLRDNORM), (0, 0));
        assert_eq!(selected(fd), (1, true, false));
        assert_eq!(waited(write_only, 50).0, 0);
        close(&[fd, read_write, write_only]);
    }

    /// eventfd_poll reports EPOLLIN and EPOLLOUT, never their socket's
    /// EPOLLRDNORM and EPOLLWRNORM.
    #[test]
    fn an_eventfd_is_never_rdnorm_or_wrnorm() {
        let _view = crate::vfs::test_view();
        let counter = event::eventfd2([1, O_NONBLOCK, 0, 0, 0, 0]);
        assert!(counter >= 0);
        assert_eq!(polled(counter, ALL), (1, POLLIN | POLLOUT));
        assert_eq!(polled(counter, POLLRDNORM | POLLWRNORM), (0, 0));
        assert_eq!(selected(counter), (2, true, true));
        let ep = watching(counter, 0x1 | 0x4 | 0x40 | 0x100);
        assert_eq!(waited(ep, 0), (1, 0x1 | 0x4));
        close(&[counter, ep]);
    }

    /// An interest stripped of every event is still the fd's: replacing
    /// the file under its number (dup3 closes it, so `eventpoll_release`
    /// drops the item) lets ADD take the number again, and MOD find none.
    #[test]
    fn an_eventless_interest_goes_with_its_file() {
        let _view = crate::vfs::test_view();
        let fd = event::timerfd_create([1, O_NONBLOCK, 0, 0, 0, 0]);
        let other = event::eventfd2([0, O_NONBLOCK, 0, 0, 0, 0]);
        assert!(fd >= 0 && other >= 0);
        let ep = watching(fd, 0x4);
        let interest = [0x1u64, fd as u64];
        let ctl =
            |op: u64| epoll::epoll_ctl([ep as u64, op, fd as u64, interest.as_ptr() as u64, 0, 0]);
        assert_eq!(ctl(1), -(crate::errno::EEXIST as i64));
        assert_eq!(fs::dup3([other as u64, fd as u64, 0, 0, 0, 0]), fd);
        assert_eq!(ctl(3), -(crate::errno::ENOENT as i64));
        assert_eq!(ctl(1), 0);
        close(&[fd, other, ep]);
    }

    fn write_count(fd: i64, v: u64) -> i64 {
        event::write(fd as i32, &v as *const u64 as u64, 8).unwrap()
    }

    fn read_count(fd: i64) -> Result<u64, i64> {
        let mut v = 0u64;
        match event::read(fd as i32, &mut v as *mut u64 as u64, 8).unwrap() {
            8 => Ok(v),
            error => Err(error),
        }
    }

    /// eventfd_write: a write that would take the count past
    /// ULLONG_MAX - 1 fails with EAGAIN without blocking, and eventfd_poll
    /// drops POLLOUT while a write of 1 would (LTP eventfd01).
    #[test]
    fn an_eventfd_count_stops_at_its_maximum() {
        let _view = crate::vfs::test_view();
        let counter = event::eventfd2([5, O_NONBLOCK, 0, 0, 0, 0]);
        assert!(counter >= 0);
        let ep = watching(counter, 0x4);
        assert_eq!(write_count(counter, u64::MAX), -(EINVAL as i64));
        assert_eq!(
            write_count(counter, u64::MAX - 5),
            -(crate::errno::EAGAIN as i64)
        );
        assert_eq!(polled(counter, ALL), (1, POLLIN | POLLOUT));
        assert_eq!(write_count(counter, u64::MAX - 6), 8);
        assert_eq!(polled(counter, ALL), (1, POLLIN));
        assert_eq!(selected(counter), (1, true, false));
        assert_eq!(waited(ep, 0).0, 0);
        assert_eq!(write_count(counter, 1), -(crate::errno::EAGAIN as i64));
        assert_eq!(write_count(counter, 0), 8);
        assert_eq!(read_count(counter), Ok(u64::MAX - 1));
        assert_eq!(polled(counter, ALL), (1, POLLOUT));
        assert_eq!(waited(ep, 0), (1, 0x4));
        assert_eq!(read_count(counter), Err(-(crate::errno::EAGAIN as i64)));
        close(&[counter, ep]);
    }

    /// A blocking write that would overflow the count sleeps until a read
    /// makes room, and is then added (LTP eventfd01's blocking case).
    #[test]
    fn an_overflowing_eventfd_write_waits_for_a_read() {
        let _view = crate::vfs::test_view();
        let counter = event::eventfd2([0, 0, 0, 0, 0, 0]);
        assert!(counter >= 0);
        assert_eq!(write_count(counter, 10), 8);
        let reader = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            read_count(counter)
        });
        let start = std::time::Instant::now();
        assert_eq!(write_count(counter, u64::MAX - 10), 8);
        assert!(start.elapsed() >= std::time::Duration::from_millis(40));
        assert_eq!(reader.join().unwrap(), Ok(10));
        assert_eq!(polled(counter, ALL), (1, POLLIN | POLLOUT));
        assert_eq!(read_count(counter), Ok(u64::MAX - 10));
        close(&[counter]);
    }

    /// pidfd_poll: an exited process's pidfd is readable, and also hung up
    /// once the process is reaped (LTP pidfd_open03).
    #[test]
    fn a_reaped_process_pidfd_hangs_up() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::a_reaped_process_pidfd_hangs_up",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id() as i32;
        let pidfd = wait::open_pidfd(pid, false);
        assert!(pidfd >= 0, "{pidfd}");
        let ep = watching(pidfd, 0x1);
        let mut entry = libc::pollfd {
            fd: pidfd as i32,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd on the stack.
        assert_eq!(unsafe { libc::poll(&mut entry, 1, 5_000) }, 1);
        assert_eq!(polled(pidfd, POLLIN | POLLRDNORM), (1, POLLIN | POLLRDNORM));
        assert_eq!(waited(ep, 0), (1, 0x1));
        // SAFETY: reaping the child spawned above.
        assert_eq!(unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) }, pid);
        assert_eq!(polled(pidfd, POLLIN), (1, POLLIN | POLLHUP));
        assert_eq!(waited(ep, 0), (1, 0x1 | 0x10));
        assert_eq!(selected(pidfd), (1, true, false));
        close(&[pidfd, ep]);
    }

    /// ppoll on one fd for `events`, waiting up to `ms`: (count, revents).
    fn polled_for(fd: i64, events: i16, ms: i64) -> (i64, i16) {
        let mut entry = libc::pollfd {
            fd: fd as i32,
            events,
            revents: 0,
        };
        let mut timeout = [ms / 1000, ms % 1000 * 1_000_000];
        let n = ppoll([
            &mut entry as *mut _ as u64,
            1,
            timeout.as_mut_ptr() as u64,
            0,
            0,
            0,
        ]);
        (n, entry.revents)
    }

    /// pidfd_poll reports EPOLLHUP once the process is reaped, which
    /// ep_item_poll and poll report whatever was asked: an interest in no
    /// events sleeps while the process runs and while it is a zombie, and
    /// wakes for the reap; an edge-triggered one gets an edge at the exit
    /// and another at the reap.
    #[test]
    fn an_eventless_pidfd_interest_wakes_for_the_reap() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::an_eventless_pidfd_interest_wakes_for_the_reap",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let pidfd = wait::open_pidfd(pid, false);
        assert!(pidfd >= 0, "{pidfd}");
        let level = watching(pidfd, 0);
        let edge = watching(pidfd, 0x1 | 1 << 31);
        let once = watching(pidfd, 1 << 30);
        let asleep = || {
            let cpu = thread_cpu();
            assert_eq!(polled_for(pidfd, 0, 50), (0, 0));
            assert_eq!(waited(level, 50).0, 0);
            assert!(thread_cpu() - cpu < 25_000_000);
        };
        asleep();
        assert_eq!(waited(edge, 0).0, 0);
        child.kill().unwrap();
        let mut entry = libc::pollfd {
            fd: pidfd as i32,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd on the stack.
        assert_eq!(unsafe { libc::poll(&mut entry, 1, 5_000) }, 1);
        asleep();
        assert_eq!(waited(edge, 0), (1, 0x1));
        assert_eq!(waited(edge, 0).0, 0);
        let sleepers = [
            std::thread::spawn(move || waited(level, 5_000)),
            std::thread::spawn(move || {
                let (n, revents) = polled_for(pidfd, 0, 5_000);
                (n, revents as u64)
            }),
        ];
        std::thread::sleep(std::time::Duration::from_millis(50));
        // SAFETY: reaping the child spawned above.
        assert_eq!(unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) }, pid);
        for (sleeper, hangup) in sleepers.into_iter().zip([0x10, POLLHUP as u64]) {
            assert_eq!(sleeper.join().unwrap(), (1, hangup));
        }
        assert_eq!(polled(pidfd, 0), (1, POLLHUP));
        assert_eq!(waited(level, 0), (1, 0x10));
        assert_eq!(waited(edge, 0), (1, 0x1 | 0x10));
        assert_eq!(waited(edge, 0).0, 0);
        assert_eq!(waited(once, 0), (1, 0x10));
        assert_eq!(waited(once, 0).0, 0);
        // Closing the pidfd ends the interest, reap included.
        close(&[pidfd]);
        let cpu = thread_cpu();
        assert_eq!(waited(level, 50).0, 0);
        assert!(thread_cpu() - cpu < 25_000_000);
        close(&[level, edge, once]);
    }

    /// A pidfd opened on a zombie (to which Darwin attaches no process
    /// filter) still wakes for the reap when this process reaps it.
    #[test]
    fn a_zombie_pidfd_wakes_for_its_reap_here() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::a_zombie_pidfd_wakes_for_its_reap_here",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id() as i32;
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waiting for the zombie without reaping it.
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as u32,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            },
            0
        );
        let pidfd = wait::open_pidfd(pid, false);
        assert!(pidfd >= 0, "{pidfd}");
        let level = watching(pidfd, 0);
        assert_eq!(polled(pidfd, 0), (0, 0));
        assert_eq!(waited(level, 0).0, 0);
        assert_eq!(wait::wait4([pid as u64, 0, 0, 0, 0, 0]), pid as i64);
        assert_eq!(polled(pidfd, 0), (1, POLLHUP));
        assert_eq!(waited(level, 0), (1, 0x10));
        close(&[pidfd, level]);
    }

    /// epoll_ctl `op` of `fd` in `ep` for `events`.
    fn ctl(ep: i64, op: u64, fd: i64, events: u32) -> i64 {
        let interest = [events as u64, fd as u64];
        epoll::epoll_ctl([ep as u64, op, fd as u64, interest.as_ptr() as u64, 0, 0])
    }

    /// A pidfd held by a process other than the parent wakes for the reap
    /// the parent does (NOTE_REAP); EPOLLONESHOT is re-armed by
    /// EPOLL_CTL_MOD and EPOLL_CTL_DEL drops the reap knote; closing the
    /// pidfd closes its reap watch.
    #[test]
    fn a_pidfd_wakes_for_another_parents_reap() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::a_pidfd_wakes_for_another_parents_reap",
        ) {
            return;
        }
        use std::io::BufRead;
        let _view = crate::vfs::test_view();
        // The shell is the parent: it reaps the sleep when it ends.
        let mut shell = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 0.3 & echo $!; wait"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(shell.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid: i32 = line.trim().parse().unwrap();
        let pidfd = wait::open_pidfd(pid, false);
        assert!(pidfd >= 0, "{pidfd}");
        let level = watching(pidfd, 0);
        let once = watching(pidfd, 1 << 30);
        let removed = watching(pidfd, 0);
        assert_eq!(ctl(removed, 2, pidfd, 0), 0);
        assert_eq!(polled_for(pidfd, 0, 0), (0, 0));
        assert_eq!(waited(level, 5_000), (1, 0x10));
        assert_eq!(polled(pidfd, 0), (1, POLLHUP));
        assert_eq!(waited(once, 0), (1, 0x10));
        assert_eq!(waited(once, 0).0, 0);
        assert_eq!(ctl(once, 3, pidfd, 1 << 30), 0);
        assert_eq!(waited(once, 0), (1, 0x10));
        assert_eq!(waited(once, 0).0, 0);
        let cpu = thread_cpu();
        assert_eq!(waited(removed, 50).0, 0);
        assert!(thread_cpu() - cpu < 25_000_000);
        let watch = wait::reap_watch(pidfd as i32).unwrap().1;
        close(&[pidfd]);
        // SAFETY: probing a descriptor number.
        assert!(unsafe { libc::fcntl(watch, libc::F_GETFD) } < 0);
        assert!(shell.wait().unwrap().success());
        close(&[level, once, removed]);
    }

    /// A dup of a pidfd (dup, fcntl F_DUPFD_CLOEXEC, dup3 over another
    /// file) is the same open file, so the same pidfd: it signals, waits
    /// and polls readable at the exit and hung up at the reap through the
    /// one reap watch, which closing the original leaves to the dups until
    /// the last of them closes.
    #[test]
    fn a_dup_of_a_pidfd_is_the_same_pidfd() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::a_dup_of_a_pidfd_is_the_same_pidfd",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let pidfd = wait::open_pidfd(pid, false);
        assert!(pidfd >= 0, "{pidfd}");
        let duped = fs::dup([pidfd as u64, 0, 0, 0, 0, 0]);
        // F_DUPFD_CLOEXEC.
        let cloexec = fs::fcntl([pidfd as u64, 1030, 0, 0, 0, 0]);
        let replaced = event::eventfd2([0, O_NONBLOCK, 0, 0, 0, 0]);
        assert!(duped >= 0 && cloexec >= 0 && replaced >= 0);
        assert_eq!(
            fs::dup3([pidfd as u64, replaced as u64, 0, 0, 0, 0]),
            replaced
        );
        let watch = wait::reap_watch(pidfd as i32).unwrap().1;
        close(&[pidfd]);
        for fd in [duped, cloexec, replaced] {
            assert_eq!(wait::reap_watch(fd as i32).unwrap().1, watch);
            assert_eq!(wait::pidfd_send_signal([fd as u64, 0, 0, 0, 0, 0]), 0);
        }
        let level = watching(duped, 0);
        let readable = watching(cloexec, 0x1);
        assert_eq!(polled(replaced, POLLIN), (0, 0));
        child.kill().unwrap();
        assert_eq!(waited(readable, 5_000), (1, 0x1));
        assert_eq!(polled(replaced, POLLIN), (1, POLLIN));
        // waitid(P_PIDFD, WEXITED | WNOWAIT) on a dup.
        let mut info = [0i32; 32];
        assert_eq!(
            wait::waitid([
                3,
                replaced as u64,
                info.as_mut_ptr() as u64,
                0x4 | 0x0100_0000,
                0,
                0
            ]),
            0
        );
        assert_eq!(info[4], pid);
        assert_eq!(waited(level, 0).0, 0);
        child.wait().unwrap();
        assert_eq!(waited(level, 5_000), (1, 0x10));
        assert_eq!(polled(replaced, 0), (1, POLLHUP));
        close(&[duped, cloexec, level, readable]);
        // SAFETY: probing descriptor numbers.
        assert!(unsafe { libc::fcntl(watch, libc::F_GETFD) } >= 0);
        close(&[replaced]);
        assert!(unsafe { libc::fcntl(watch, libc::F_GETFD) } < 0);
    }

    /// Two dups of one pidfd in one epoll are two interests, each with its
    /// own reap knote, as an epitem is per file and fd: both report the
    /// reap, and deleting another, or one firing one-shot, leaves the rest.
    #[test]
    fn two_dups_of_a_pidfd_in_one_epoll_each_report_the_reap() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::two_dups_of_a_pidfd_in_one_epoll_each_report_the_reap",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let pidfd = wait::open_pidfd(child.id() as i32, false);
        assert!(pidfd >= 0, "{pidfd}");
        let once = fs::dup([pidfd as u64, 0, 0, 0, 0, 0]);
        let level = fs::dup([pidfd as u64, 0, 0, 0, 0, 0]);
        assert!(once >= 0 && level >= 0);
        let ep = watching(pidfd, 0);
        assert_eq!(ctl(ep, 1, once, 1 << 30), 0);
        assert_eq!(ctl(ep, 1, level, 0), 0);
        assert_eq!(ctl(ep, 2, pidfd, 0), 0);
        child.kill().unwrap();
        child.wait().unwrap();
        // epoll_pwait for up to four events: (data, events) of each.
        let ready = |ms: u64| {
            let mut events = [0u64; 8];
            let n = epoll::epoll_pwait([ep as u64, events.as_mut_ptr() as u64, 4, ms, 0, 0]);
            assert!(n >= 0, "{n}");
            let mut got: Vec<(u64, u64)> = (0..n as usize)
                .map(|i| (events[2 * i + 1], events[2 * i]))
                .collect();
            got.sort();
            got
        };
        let mut got = ready(5_000);
        if got.len() < 2 {
            got.extend(ready(1_000));
            got.sort();
        }
        assert_eq!(got, [(once as u64, 0x10), (level as u64, 0x10)]);
        assert_eq!(ready(0), [(level as u64, 0x10)]);
        close(&[pidfd, once, level, ep]);
    }

    /// This thread's CPU time in nanoseconds.
    fn thread_cpu() -> i64 {
        let mut t = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a timespec on the stack.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) },
            0
        );
        t.tv_sec * 1_000_000_000 + t.tv_nsec
    }

    /// ep_item_poll always reports EPOLLERR and EPOLLHUP: an interest with
    /// no events still reports the peer's close, level-triggered unless
    /// one-shot, and nothing before it, not even for a full buffer
    /// (`sock_poll`, `pipe_poll`; LTP epoll_ctl01 style).
    #[test]
    fn an_eventless_interest_reports_hangup() {
        // The socketpair's and pipe's numbers are not in the guest table,
        // which other tests' descriptors would take.
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::an_eventless_interest_reports_hangup",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let (mut socket, mut pipe) = ([0i32; 2], [0i32; 2]);
        // SAFETY: a socketpair and a pipe into local arrays.
        unsafe {
            assert_eq!(
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, socket.as_mut_ptr()),
                0
            );
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
        }
        // A receive buffer smaller than what the peer can send, which
        // Darwin caps the low-water mark at.
        let small: i32 = 2048;
        // SAFETY: setting an int option on our socket.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    socket[0],
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    (&small as *const i32).cast(),
                    4,
                )
            },
            0
        );
        for [local, peer] in [socket, pipe] {
            let level = watching(local as i64, 0);
            let oneshot = watching(local as i64, 1 << 30);
            // SAFETY: filling our own descriptor's buffer without blocking.
            unsafe {
                libc::fcntl(peer, libc::F_SETFL, libc::O_NONBLOCK);
                while libc::write(peer, [0u8; 512].as_ptr().cast(), 512) > 0 {}
            }
            // Waited for, not spun on.
            let cpu = thread_cpu();
            assert_eq!(waited(level, 50).0, 0);
            assert!(thread_cpu() - cpu < 25_000_000);
            assert_eq!(waited(oneshot, 0).0, 0);
            // SAFETY: our descriptor.
            unsafe { libc::close(peer) };
            assert_eq!(waited(level, 2_000), (1, 0x10));
            assert_eq!(waited(level, 0), (1, 0x10));
            assert_eq!(waited(oneshot, 0), (1, 0x10));
            assert_eq!(waited(oneshot, 0).0, 0);
            // SAFETY: our descriptor, not published to the guest table.
            unsafe { libc::close(local) };
            close(&[level, oneshot]);
        }
    }

    /// A guest IPv4 socket of `kind`, non-blocking, connected to loopback
    /// `port`: (fd, connect's result).
    fn connected_to(kind: u64, port: u16) -> (i64, i64) {
        let fd = crate::sys::net::socket([2, kind | O_NONBLOCK, 0, 0, 0, 0]);
        assert!(fd >= 0, "socket {fd}");
        let mut address = [0u8; 16];
        address[..2].copy_from_slice(&2u16.to_le_bytes());
        address[2..4].copy_from_slice(&port.to_be_bytes());
        address[4..8].copy_from_slice(&[127, 0, 0, 1]);
        let connected = crate::sys::net::connect([fd as u64, address.as_ptr() as u64, 16, 0, 0, 0]);
        (fd, connected)
    }

    /// A guest socket of `kind` connected to a closed loopback port.
    fn refused(kind: u64) -> (i64, i64) {
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        connected_to(kind, port)
    }

    /// Send a byte on `fd`, a UDP socket connected to a closed port, until
    /// `wait` sees the ICMP port unreachable it draws, for up to 10 s:
    /// Darwin rate-limits ICMP errors (net.inet.icmp.icmplim) and drops
    /// them under load (#2803). A send that finds an error pending returns
    /// it, so `wait`'s first event is the error of the last send.
    fn refusal<E>(fd: i64, mut wait: impl FnMut() -> (i64, E)) -> (i64, E) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let byte = 0u8;
            let sent = crate::sys::net::sendto([fd as u64, &byte as *const u8 as u64, 1, 0, 0, 0]);
            assert!(sent == 1 || sent == -111, "sendto {sent}");
            let waited = wait();
            if waited.0 != 0 || std::time::Instant::now() >= deadline {
                return waited;
            }
        }
    }

    /// recvfrom into `buffer`, without an address.
    fn received(fd: i64, buffer: &mut [u8]) -> i64 {
        crate::sys::net::recvfrom([
            fd as u64,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
            0,
            0,
        ])
    }

    /// An edge-triggered interest reports the error's wakeup once
    /// (`ep_poll_callback`), however long it stays pending.
    #[test]
    fn an_edge_triggered_interest_reports_a_pending_error_once() {
        let _view = crate::vfs::test_view();
        let (fd, connected) = refused(2);
        assert_eq!(connected, 0);
        let edge = watching(fd, 0x1 | 1 << 31);
        assert_eq!(refusal(fd, || waited(edge, 500)), (1, 0x8));
        assert_eq!(waited(edge, 0).0, 0);
        assert_eq!(polled(fd, POLLIN), (1, POLLERR));
        close(&[fd, edge]);
    }

    /// A queued datagram, a zero-length one included, is EPOLLIN with a
    /// pending error's EPOLLERR (`datagram_poll`, `sock_poll`).
    #[test]
    fn a_queued_datagram_is_readable_with_a_pending_error() {
        let _view = crate::vfs::test_view();
        let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let (fd, connected) = connected_to(2, peer.local_addr().unwrap().port());
        assert_eq!(connected, 0);
        let mut local = [0u8; 16];
        let mut len = 16u32;
        assert_eq!(
            crate::sys::net::getsockname([
                fd as u64,
                local.as_mut_ptr() as u64,
                &mut len as *mut u32 as u64,
                0,
                0,
                0
            ]),
            0
        );
        let port = u16::from_be_bytes([local[2], local[3]]);
        peer.send_to(&[], ("127.0.0.1", port)).unwrap();
        drop(peer);
        let readable = watching(fd, 0x1);
        assert_eq!(waited(readable, 2_000), (1, 0x1));
        assert_eq!(refusal(fd, || polled_for(fd, 0, 500)), (1, POLLERR));
        assert_eq!(polled(fd, POLLIN), (1, POLLIN | POLLERR));
        assert_eq!(waited(readable, 0), (1, 0x1 | 0x8));
        close(&[fd, readable]);
    }

    /// A datagram queued before a pending error: `sock_poll` reports
    /// EPOLLIN and EPOLLERR, and `__skb_try_recv_datagram` returns the
    /// error, taking it, before the datagram.
    #[test]
    fn a_pending_error_is_received_before_a_queued_datagram() {
        let _view = crate::vfs::test_view();
        let peer = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let (fd, connected) = connected_to(2, peer.local_addr().unwrap().port());
        assert_eq!(connected, 0);
        let mut local = [0u8; 16];
        let mut len = 16u32;
        assert_eq!(
            crate::sys::net::getsockname([
                fd as u64,
                local.as_mut_ptr() as u64,
                &mut len as *mut u32 as u64,
                0,
                0,
                0
            ]),
            0
        );
        let port = u16::from_be_bytes([local[2], local[3]]);
        peer.send_to(b"xy", ("127.0.0.1", port)).unwrap();
        drop(peer);
        let readable = watching(fd, 0x1);
        assert_eq!(waited(readable, 2_000), (1, 0x1));
        assert_eq!(refusal(fd, || polled_for(fd, 0, 500)), (1, POLLERR));
        assert_eq!(polled(fd, POLLIN), (1, POLLIN | POLLERR));
        assert_eq!(waited(readable, 0), (1, 0x1 | 0x8));
        let mut buffer = [0u8; 4];
        assert_eq!(received(fd, &mut buffer), -111);
        assert_eq!(polled(fd, POLLIN), (1, POLLIN));
        assert_eq!(waited(readable, 0), (1, 0x1));
        assert_eq!(received(fd, &mut buffer), 2);
        assert_eq!(&buffer[..2], b"xy");
        assert_eq!(received(fd, &mut buffer), -11);
        close(&[fd, readable]);
    }

    /// A reset with data queued: `tcp_reset` sets ECONNRESET and
    /// `tcp_done` shuts the socket both ways, so it polls IN, OUT, HUP and
    /// ERR, and stays readable once SO_ERROR took the error.
    #[test]
    fn a_reset_with_data_queued_is_readable_and_pollerr() {
        let _view = crate::vfs::test_view();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (fd, connected) = connected_to(1, listener.local_addr().unwrap().port());
        assert_eq!(connected, -115);
        let (peer, _) = listener.accept().unwrap();
        use std::io::Write;
        (&peer).write_all(b"x").unwrap();
        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        // SAFETY: setting SO_LINGER on our own socket.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    peer.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_LINGER,
                    (&linger as *const libc::linger).cast(),
                    std::mem::size_of::<libc::linger>() as u32,
                )
            },
            0
        );
        drop(peer);
        let both = watching(fd, 0x1 | 0x4);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while waited(both, 0).1 & 0x8 == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(waited(both, 0), (1, 0x1 | 0x4 | 0x8 | 0x10));
        assert_eq!(
            polled(fd, POLLIN | POLLOUT),
            (1, POLLIN | POLLOUT | POLLERR | POLLHUP)
        );
        let mut error = [0i32; 2];
        let mut len = 4u32;
        assert_eq!(
            crate::sys::net::getsockopt([
                fd as u64,
                1,
                4,
                error.as_mut_ptr() as u64,
                &mut len as *mut u32 as u64,
                0,
            ]),
            0
        );
        assert_eq!(error[0], 104);
        assert_eq!(waited(both, 0), (1, 0x1 | 0x4 | 0x10));
        assert_eq!(
            polled(fd, POLLIN | POLLOUT),
            (1, POLLIN | POLLOUT | POLLHUP)
        );
        close(&[fd, both]);
    }

    /// An AF_UNIX socket has no pending error: poll and epoll report its
    /// readiness alone.
    #[test]
    fn a_unix_socket_reports_no_error() {
        let _view = crate::vfs::test_view();
        let mut pair = [0i32; 2];
        assert_eq!(
            crate::sys::net::socketpair([1, 1 | O_NONBLOCK, 0, pair.as_mut_ptr() as u64, 0, 0]),
            0
        );
        let (fd, other) = (pair[0] as i64, pair[1] as i64);
        let byte = 0u8;
        assert_eq!(
            crate::sys::net::sendto([other as u64, &byte as *const u8 as u64, 1, 0, 0, 0]),
            1
        );
        let both = watching(fd, 0x1 | 0x4);
        assert_eq!(waited(both, 0), (1, 0x1 | 0x4));
        assert_eq!(polled(fd, POLLIN | POLLOUT), (1, POLLIN | POLLOUT));
        assert_eq!(polled(fd, 0), (0, 0));
        let mut buffer = [0u8; 4];
        assert_eq!(received(fd, &mut buffer), 1);
        assert_eq!(waited(both, 0), (1, 0x4));
        assert_eq!(polled(fd, POLLIN), (0, 0));
        close(&[fd, other, both]);
    }

    /// `sock_poll` reports EPOLLERR while `sk_err` is set, here by the
    /// ICMP port unreachable a connected UDP socket's send draws, whatever
    /// was asked and without EPOLLIN for an empty queue (`datagram_poll`);
    /// the next receive returns ECONNREFUSED once and clears it, and
    /// SO_ERROR reads and clears it the same way (LTP-style).
    #[test]
    fn a_pending_datagram_error_is_pollerr_until_returned() {
        let _view = crate::vfs::test_view();
        for consume in ["recv", "SO_ERROR"] {
            let (fd, connected) = refused(2);
            assert_eq!(connected, 0);
            let readable = watching(fd, 0x1);
            let eventless = watching(fd, 0);
            let writable = watching(fd, 0x4);
            assert_eq!(refusal(fd, || waited(readable, 500)), (1, 0x8), "{consume}");
            assert_eq!(waited(readable, 0), (1, 0x8));
            assert_eq!(waited(eventless, 0), (1, 0x8));
            assert_eq!(waited(writable, 0), (1, 0x4 | 0x8));
            assert_eq!(polled(fd, 0), (1, POLLERR));
            assert_eq!(polled(fd, POLLIN | POLLRDNORM), (1, POLLERR));
            assert_eq!(polled(fd, POLLOUT), (1, POLLOUT | POLLERR));
            if consume == "recv" {
                let mut buffer = [0u8; 4];
                assert_eq!(received(fd, &mut buffer), -111);
                assert_eq!(received(fd, &mut buffer), -(crate::errno::EAGAIN as i64));
            } else {
                let mut error = [0i32; 2];
                let mut len = 4u32;
                let get = |error: &mut [i32; 2], len: &mut u32| {
                    crate::sys::net::getsockopt([
                        fd as u64,
                        1,
                        4,
                        error.as_mut_ptr() as u64,
                        len as *mut u32 as u64,
                        0,
                    ])
                };
                assert_eq!(get(&mut error, &mut len), 0);
                assert_eq!(error[0], 111);
                assert_eq!(get(&mut error, &mut len), 0);
                assert_eq!(error[0], 0);
            }
            assert_eq!(waited(readable, 0).0, 0);
            assert_eq!(waited(eventless, 0).0, 0);
            assert_eq!(waited(writable, 0), (1, 0x4));
            assert_eq!(polled(fd, 0), (0, 0));
            assert_eq!(polled(fd, POLLIN), (0, 0));
            assert_eq!(polled(fd, POLLOUT), (1, POLLOUT));
            close(&[fd, readable, eventless, writable]);
        }
    }

    /// `tcp_poll`: a TCP socket never connected, bound or not, is
    /// `TCP_CLOSE` with no shutdown, so hung up and writable to poll,
    /// select and epoll, at once whatever the timeout, and an
    /// edge-triggered or one-shot interest reports it once (LTP poll01,
    /// epoll_wait01 style).
    #[test]
    fn an_unconnected_tcp_socket_is_hung_up_and_writable() {
        let _view = crate::vfs::test_view();
        const EPOLLOUT: u64 = 0x4;
        const EPOLLHUP: u64 = 0x10;
        const EPOLLET: u32 = 1 << 31;
        const EPOLLONESHOT: u32 = 1 << 30;
        for family in [2u64, 10] {
            let fd = crate::sys::net::socket([family, 1 | O_NONBLOCK, 0, 0, 0, 0]);
            assert!(fd >= 0, "socket {fd}");
            for bound in [false, true] {
                if bound {
                    let mut address = [0u8; 28];
                    address[..2].copy_from_slice(&(family as u16).to_le_bytes());
                    if family == 2 {
                        address[4..8].copy_from_slice(&[127, 0, 0, 1]);
                    } else {
                        address[23] = 1;
                    }
                    let len = if family == 2 { 16 } else { 28 };
                    assert_eq!(
                        crate::sys::net::bind([fd as u64, address.as_ptr() as u64, len, 0, 0, 0]),
                        0
                    );
                }
                let case = format!("family {family}, bound {bound}");
                assert_eq!(
                    polled(fd, ALL),
                    (1, POLLOUT | POLLWRNORM | POLLHUP),
                    "{case}"
                );
                assert_eq!(polled(fd, POLLIN), (1, POLLHUP), "{case}");
                assert_eq!(polled(fd, 0), (1, POLLHUP), "{case}");
                assert_eq!(polled_for(fd, POLLIN, 60_000), (1, POLLHUP), "{case}");
                assert_eq!(selected(fd), (2, true, true), "{case}");
                for (events, fired) in [
                    (0x1 | 0x4, EPOLLOUT | EPOLLHUP),
                    (0x1, EPOLLHUP),
                    (0, EPOLLHUP),
                ] {
                    let level = watching(fd, events);
                    assert_eq!(waited(level, 60_000), (1, fired), "{case}: {events:#x}");
                    assert_eq!(waited(level, 0), (1, fired), "{case}: {events:#x}");
                    let edge = watching(fd, events | EPOLLET);
                    assert_eq!(waited(edge, 0), (1, fired), "{case}: {events:#x}");
                    assert_eq!(waited(edge, 0).0, 0, "{case}: {events:#x}");
                    let once = watching(fd, events | EPOLLONESHOT);
                    assert_eq!(waited(once, 0), (1, fired), "{case}: {events:#x}");
                    assert_eq!(waited(once, 0).0, 0, "{case}: {events:#x}");
                    close(&[level, edge, once]);
                }
            }
            close(&[fd]);
        }
    }

    /// `POLLEX_SET` is EPOLLPRI alone: select sleeps for its timeout on an
    /// unconnected TCP socket asked only for exceptions, which `tcp_poll`
    /// reports hung up and writable.
    #[test]
    fn select_sleeps_on_an_unconnected_tcp_socket_asked_only_for_exceptions() {
        let _view = crate::vfs::test_view();
        let fd = crate::sys::net::socket([2, 1 | O_NONBLOCK, 0, 0, 0, 0]);
        assert!(fd >= 0, "socket {fd}");
        let mut except = [0u64; 16];
        except[fd as usize / 64] = 1 << (fd % 64);
        let timeout = [0i64, 200_000_000];
        let cpu = || {
            let mut t = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: into a local timespec.
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
            t.tv_sec as f64 + t.tv_nsec as f64 / 1e9
        };
        let (started, spent) = (std::time::Instant::now(), cpu());
        assert_eq!(
            pselect6([
                fd as u64 + 1,
                0,
                0,
                except.as_mut_ptr() as u64,
                timeout.as_ptr() as u64,
                0,
            ]),
            0
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(190));
        let busy = cpu() - spent;
        assert!(busy < 0.02, "spun for {busy} s of CPU");
        close(&[fd]);
    }

    /// Two threads waiting on one epoll: an edge-triggered interest in an
    /// unconnected TCP socket is delivered to one (`ep_poll`), and the
    /// other waits out its timeout.
    #[test]
    fn an_unconnected_tcp_socket_edge_reaches_one_waiter() {
        let _view = crate::vfs::test_view();
        for _ in 0..20 {
            let fd = crate::sys::net::socket([2, 1 | O_NONBLOCK, 0, 0, 0, 0]);
            assert!(fd >= 0, "socket {fd}");
            let edge = watching(fd, 0x4 | 1 << 31);
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let waiters: Vec<_> = (0..2)
                .map(|_| {
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        let started = std::time::Instant::now();
                        (waited(edge, 50).0, started.elapsed())
                    })
                })
                .collect();
            let results: Vec<_> = waiters.into_iter().map(|w| w.join().unwrap()).collect();
            assert_eq!(results.iter().map(|r| r.0).sum::<i64>(), 1, "{results:?}");
            for (n, elapsed) in &results {
                assert!(
                    *n == 1 || *elapsed >= std::time::Duration::from_millis(45),
                    "{results:?}"
                );
            }
            close(&[fd, edge]);
        }
    }

    /// `inet_csk_listen_poll`: a listening socket is only readable, once a
    /// connection is queued; the connected socket is writable, no longer
    /// hung up.
    #[test]
    fn a_listening_or_connected_tcp_socket_is_not_hung_up() {
        let _view = crate::vfs::test_view();
        let fd = crate::sys::net::socket([2, 1 | O_NONBLOCK, 0, 0, 0, 0]);
        assert!(fd >= 0, "socket {fd}");
        let mut address = [0u8; 16];
        address[..2].copy_from_slice(&2u16.to_le_bytes());
        address[4..8].copy_from_slice(&[127, 0, 0, 1]);
        assert_eq!(
            crate::sys::net::bind([fd as u64, address.as_ptr() as u64, 16, 0, 0, 0]),
            0
        );
        assert_eq!(crate::sys::net::listen([fd as u64, 1, 0, 0, 0, 0]), 0);
        let both = watching(fd, 0x1 | 0x4);
        assert_eq!(polled(fd, ALL), (0, 0));
        assert_eq!(selected(fd), (0, false, false));
        assert_eq!(waited(both, 0).0, 0);
        let mut len = 16u32;
        assert_eq!(
            crate::sys::net::getsockname([
                fd as u64,
                address.as_mut_ptr() as u64,
                &mut len as *mut u32 as u64,
                0,
                0,
                0
            ]),
            0
        );
        let port = u16::from_be_bytes([address[2], address[3]]);
        let (client, connected) = connected_to(1, port);
        assert_eq!(connected, -115);
        assert_eq!(polled_for(client, POLLOUT, 2_000), (1, POLLOUT));
        assert_eq!(polled(client, POLLIN | POLLOUT), (1, POLLOUT));
        assert_eq!(selected(client), (1, false, true));
        assert_eq!(polled(fd, ALL), (1, POLLIN | POLLRDNORM));
        assert_eq!(selected(fd), (1, true, false));
        assert_eq!(waited(both, 0), (1, 0x1));
        close(&[client, fd, both]);
    }

    /// A refused non-blocking TCP connect: `tcp_done` shuts it both ways,
    /// so `tcp_poll` reports HUP, IN and OUT, and ERR until SO_ERROR
    /// returns ECONNREFUSED once.
    #[test]
    fn a_refused_connect_is_pollerr_until_so_error() {
        let _view = crate::vfs::test_view();
        let (fd, connected) = refused(1);
        assert_eq!(connected, -115);
        let eventless = watching(fd, 0);
        let both = watching(fd, 0x1 | 0x4);
        assert_eq!(waited(eventless, 2_000), (1, 0x8 | 0x10));
        assert_eq!(waited(both, 0), (1, 0x1 | 0x4 | 0x8 | 0x10));
        assert_eq!(
            polled(fd, POLLIN | POLLOUT),
            (1, POLLIN | POLLOUT | POLLERR | POLLHUP)
        );
        assert_eq!(
            polled(fd, ALL),
            (
                1,
                POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM | POLLERR | POLLHUP
            )
        );
        assert_eq!(selected(fd), (2, true, true));
        let mut error = [0i32; 2];
        let mut len = 4u32;
        let mut get = || {
            crate::sys::net::getsockopt([
                fd as u64,
                1,
                4,
                error.as_mut_ptr() as u64,
                &mut len as *mut u32 as u64,
                0,
            ]);
            error[0]
        };
        assert_eq!(get(), 111);
        assert_eq!(get(), 0);
        assert_eq!(waited(eventless, 0), (1, 0x10));
        assert_eq!(
            polled(fd, POLLIN | POLLOUT),
            (1, POLLIN | POLLOUT | POLLHUP)
        );
        close(&[fd, eventless, both]);
    }

    /// Closing the last reference to a file drops its eventless interest
    /// (`eventpoll_release`): a new file under the number is not in the
    /// interest list.
    #[test]
    fn an_eventless_interest_goes_with_its_close() {
        // A closed number is reused below, which only this process's own
        // descriptors can keep vacant.
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::an_eventless_interest_goes_with_its_close",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        let counter = event::eventfd2([0, O_NONBLOCK, 0, 0, 0, 0]);
        let timer = event::timerfd_create([1, O_NONBLOCK, 0, 0, 0, 0]);
        let other = event::eventfd2([0, O_NONBLOCK, 0, 0, 0, 0]);
        let ep = epoll::epoll_create1([0; 6]);
        assert!(counter >= 0 && timer >= 0 && other >= 0 && ep >= 0);
        for (fd, events) in [(counter, 0u64), (timer, 0x4)] {
            let interest = [events, fd as u64];
            let ctl = |op: u64| {
                epoll::epoll_ctl([ep as u64, op, fd as u64, interest.as_ptr() as u64, 0, 0])
            };
            assert_eq!(ctl(1), 0);
            close(&[fd]);
            assert_eq!(fs::dup3([other as u64, fd as u64, 0, 0, 0, 0]), fd);
            assert_eq!(ctl(3), -(crate::errno::ENOENT as i64));
            assert_eq!(ctl(2), -(crate::errno::ENOENT as i64));
            assert_eq!(ctl(1), 0);
            close(&[fd]);
        }
        close(&[other, ep]);
    }

    /// ep_eventpoll_poll, inotify_poll and pidfd_poll report only
    /// EPOLLIN|EPOLLRDNORM: asked for write events, an epoll, inotify or
    /// pidfd is not ready, and epoll takes the interest.
    #[test]
    fn a_kqueue_descriptor_is_never_writable() {
        let _view = crate::vfs::test_view();
        let inner = epoll::epoll_create1([0; 6]);
        let notify = inotify::inotify_init1([O_NONBLOCK, 0, 0, 0, 0, 0]);
        let pidfd = wait::pidfd_open([unsafe { libc::getpid() } as u64, 0, 0, 0, 0, 0]);
        for fd in [inner, notify, pidfd] {
            assert!(fd >= 0, "{fd}");
            assert_eq!(polled(fd, ALL), (0, 0), "fd {fd}");
            assert_eq!(polled(fd, POLLOUT), (0, 0), "fd {fd}");
            assert_eq!(selected(fd), (0, false, false), "fd {fd}");
            let ep = watching(fd, 0x1 | 0x4 | 0x100);
            assert_eq!(waited(ep, 0).0, 0);
            close(&[ep]);
        }
        // An epoll with a ready interest is readable, not writable.
        let counter = event::eventfd2([1, O_NONBLOCK, 0, 0, 0, 0]);
        assert!(counter >= 0);
        let ready = watching(counter, 0x1);
        let outer = watching(ready, 0x4);
        assert_eq!(polled(ready, ALL), (1, POLLIN | POLLRDNORM));
        assert_eq!(selected(ready), (1, true, false));
        assert_eq!(waited(outer, 0).0, 0);
        close(&[inner, notify, pidfd, counter, ready, outer]);
    }

    /// Run `action` on another thread after `ms`, as a wait is under way.
    fn after(ms: u64, action: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            action();
        })
    }

    /// ppoll on `fd` for no event, with a timeout of 30 s, while `action`
    /// runs after 100 ms: (count, revents).
    fn woken(fd: i64, action: impl FnOnce() + Send + 'static) -> (i64, i16) {
        let thread = after(100, action);
        let polled = polled_for(fd, 0, 30_000);
        thread.join().unwrap();
        polled
    }

    /// ppoll on `fd` for no event sleeps out its 200 ms timeout, not
    /// spinning.
    fn sleeps(fd: i64) {
        let (start, cpu) = (std::time::Instant::now(), thread_cpu());
        assert_eq!(polled_for(fd, 0, 200), (0, 0), "fd {fd}");
        assert!(start.elapsed().as_millis() >= 200);
        assert!(thread_cpu() - cpu < 25_000_000, "fd {fd}");
    }

    /// SO_LINGER of 0 on `socket`, so that closing it resets.
    fn abortive(socket: &std::net::TcpStream) {
        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        // SAFETY: setting SO_LINGER on our own socket.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_LINGER,
                    (&linger as *const libc::linger).cast(),
                    std::mem::size_of::<libc::linger>() as u32,
                )
            },
            0
        );
    }

    /// Linux poll reports POLLERR and POLLHUP whatever was asked
    /// (`sock_poll`, `tcp_poll`): a TCP socket polled for no event sleeps
    /// while established, with data queued, and after the peer's FIN,
    /// which `tcp_poll` reports as POLLIN|POLLRDHUP alone; the reset that
    /// follows wakes the wait with POLLERR|POLLHUP (LTP poll01 style).
    #[test]
    fn an_eventless_tcp_poll_wakes_for_a_reset() {
        let _view = crate::vfs::test_view();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (fd, connected) = connected_to(1, listener.local_addr().unwrap().port());
        assert_eq!(connected, -115);
        let (peer, _) = listener.accept().unwrap();
        assert_eq!(polled_for(fd, POLLOUT, 2_000), (1, POLLOUT));
        sleeps(fd);
        use std::io::Write;
        (&peer).write_all(b"x").unwrap();
        assert_eq!(polled_for(fd, POLLIN, 2_000), (1, POLLIN));
        sleeps(fd);
        peer.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(
            polled_for(fd, POLLRDHUP, 2_000),
            (1, POLLRDHUP),
            "the FIN arrived"
        );
        sleeps(fd);
        abortive(&peer);
        let (n, revents) = woken(fd, move || drop(peer));
        assert_eq!((n, revents), (1, POLLERR | POLLHUP));
        close(&[fd]);
    }

    /// A connected UDP socket polled for no event wakes for the ICMP port
    /// unreachable that a send during the wait draws (`sk_error_report`),
    /// with POLLERR alone (`datagram_poll`).
    #[test]
    fn an_eventless_udp_poll_wakes_for_an_error() {
        let _view = crate::vfs::test_view();
        let (fd, connected) = refused(2);
        assert_eq!(connected, 0);
        sleeps(fd);
        let (n, revents) = woken(fd, move || {
            assert_eq!(refusal(fd, || polled_for(fd, 0, 500)), (1, POLLERR));
        });
        assert_eq!((n, revents), (1, POLLERR));
        close(&[fd]);
    }

    /// An AF_UNIX stream socket polled for no event sleeps with data
    /// queued and with its buffer full, and wakes with POLLHUP when the
    /// peer closes (`unix_release_sock` shuts it both ways, `unix_poll`).
    #[test]
    fn an_eventless_unix_poll_wakes_for_the_peer_close() {
        let _view = crate::vfs::test_view();
        let mut pair = [0i32; 2];
        assert_eq!(
            crate::sys::net::socketpair([1, 1 | O_NONBLOCK, 0, pair.as_mut_ptr() as u64, 0, 0]),
            0
        );
        let (fd, other) = (pair[0] as i64, pair[1] as i64);
        sleeps(fd);
        let chunk = [0u8; 512];
        while crate::sys::net::sendto([other as u64, chunk.as_ptr() as u64, 512, 0, 0, 0]) > 0 {}
        sleeps(fd);
        sleeps(other);
        let (n, revents) = woken(fd, move || close(&[other]));
        assert_eq!((n, revents), (1, POLLHUP));
        close(&[fd]);
    }

    /// `pipe_poll`: a pipe end polled for no event sleeps while the other
    /// is open, with data queued or the buffer full; the read end wakes with POLLHUP when the last writer
    /// closes, and the write end with POLLERR, never POLLHUP, when the
    /// last reader does.
    #[test]
    fn an_eventless_pipe_poll_wakes_for_the_other_end_close() {
        let _view = crate::vfs::test_view();
        for (end, revents) in [(0, POLLHUP), (1, POLLERR)] {
            let mut pipe = [0i32; 2];
            assert_eq!(
                fs::pipe2([pipe.as_mut_ptr() as u64, O_NONBLOCK | O_CLOEXEC, 0, 0, 0, 0]),
                0
            );
            let (fd, other) = (pipe[end] as i64, pipe[1 - end] as i64);
            sleeps(fd);
            // The read end with data queued, the write end with the
            // buffer full.
            let writer = pipe[1];
            // SAFETY: writing to our own non-blocking pipe.
            while unsafe { libc::write(writer, [0u8; 512].as_ptr().cast(), 512) } > 0 && end == 1 {}
            sleeps(fd);
            let (n, polled_revents) = woken(fd, move || close(&[other]));
            assert_eq!((n, polled_revents), (1, revents), "end {end}");
            assert_eq!(polled(fd, 0), (1, revents));
            if end == 1 {
                assert_eq!(polled(fd, POLLOUT).1 & (POLLERR | POLLHUP), POLLERR);
            }
            close(&[fd]);
        }
    }

    /// ppoll on `entries` for up to `ms`: the count.
    fn polled_all(entries: &mut [libc::pollfd], ms: i64) -> i64 {
        let mut timeout = [ms / 1000, ms % 1000 * 1_000_000];
        ppoll([
            entries.as_mut_ptr() as u64,
            entries.len() as u64,
            timeout.as_mut_ptr() as u64,
            0,
            0,
            0,
        ])
    }

    /// A guest AF_UNIX stream socketpair, non-blocking.
    fn unix_pair() -> (i64, i64) {
        let mut pair = [0i32; 2];
        assert_eq!(
            crate::sys::net::socketpair([1, 1 | O_NONBLOCK, 0, pair.as_mut_ptr() as u64, 0, 0]),
            0
        );
        (pair[0] as i64, pair[1] as i64)
    }

    fn shut(fd: i64, how: u64) {
        assert_eq!(crate::sys::net::shutdown([fd as u64, how, 0, 0, 0, 0]), 0);
    }

    /// `unix_poll` reports POLLHUP only once both directions are shut
    /// (`sk_shutdown == SHUTDOWN_MASK`), a receive shutdown alone as
    /// POLLIN|POLLRDNORM|POLLRDHUP: a poll or epoll interest with no event
    /// sleeps through the peer's SHUT_WR and its own SHUT_RD, and wakes
    /// for the peer's close that completes them.
    #[test]
    fn an_eventless_unix_poll_sleeps_through_a_half_close() {
        let _view = crate::vfs::test_view();
        let (fd, other) = unix_pair();
        shut(other, 1);
        sleeps(fd);
        let eventless = watching(fd, 0);
        assert_eq!(waited(eventless, 50).0, 0);
        assert_eq!(
            polled(fd, POLLIN | POLLRDHUP | POLLOUT),
            (1, POLLIN | POLLRDHUP | POLLOUT)
        );
        let readable = watching(fd, 0x1 | 0x2000);
        assert_eq!(waited(readable, 0), (1, 0x1 | 0x2000));
        let (n, revents) = woken(fd, move || close(&[other]));
        assert_eq!((n, revents), (1, POLLHUP));
        assert_eq!(waited(eventless, 0), (1, 0x10));
        close(&[fd, eventless, readable]);
        let (fd, other) = unix_pair();
        shut(fd, 0);
        sleeps(fd);
        assert_eq!(polled(fd, POLLIN | POLLRDHUP), (1, POLLIN | POLLRDHUP));
        let (n, revents) = woken(fd, move || close(&[other]));
        assert_eq!((n, revents), (1, POLLHUP));
        close(&[fd]);
    }

    /// Entries for no event, one socket's twice, poll with others: each
    /// reports its own readiness, and a hangup every entry of its socket.
    #[test]
    fn eventless_entries_mix_with_others() {
        let _view = crate::vfs::test_view();
        let (fd, other) = unix_pair();
        let (data, sender) = unix_pair();
        let entry = |fd: i64, events: i16| libc::pollfd {
            fd: fd as i32,
            events,
            revents: 0,
        };
        let mut entries = [
            entry(fd, 0),
            entry(data, POLLIN),
            entry(fd, 0),
            entry(fd, POLLOUT),
        ];
        assert_eq!(polled_all(&mut entries[..3], 50), 0);
        let byte = 0u8;
        assert_eq!(
            crate::sys::net::sendto([sender as u64, &byte as *const u8 as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(polled_all(&mut entries, 2_000), 2);
        assert_eq!(entries.map(|e| e.revents), [0, POLLIN, 0, POLLOUT]);
        let mut buffer = [0u8; 4];
        assert_eq!(received(data, &mut buffer), 1);
        let thread = after(100, move || close(&[other]));
        assert_eq!(polled_all(&mut entries[..3], 5_000), 2);
        thread.join().unwrap();
        assert_eq!(entries.map(|e| e.revents)[..3], [POLLHUP, 0, POLLHUP]);
        close(&[fd, data, sender]);
    }

    extern "C" fn interrupt(_: i32) {}

    /// A signal interrupts a poll for no event with EINTR, and the wait's
    /// own descriptors go with it.
    #[test]
    fn an_eventless_poll_is_interruptible() {
        // A signal handler is installed for the process.
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::poll::reportable_tests::an_eventless_poll_is_interruptible",
        ) {
            return;
        }
        let _view = crate::vfs::test_view();
        // SAFETY: a handler without SA_RESTART, for this process only.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = interrupt as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            assert_eq!(
                libc::sigaction(libc::SIGUSR2, &action, std::ptr::null_mut()),
                0
            );
        }
        let (fd, other) = unix_pair();
        // SAFETY: probing the lowest free descriptor number.
        let lowest_free = || unsafe {
            let probe = libc::dup(0);
            libc::close(probe);
            probe
        };
        let free = lowest_free();
        let (tx, rx) = std::sync::mpsc::channel();
        let poller = std::thread::spawn(move || {
            // SAFETY: this thread's own handle.
            tx.send(unsafe { libc::pthread_self() } as usize).unwrap();
            polled_for(fd, 0, 5_000)
        });
        let thread = rx.recv().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !poller.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            // SAFETY: signalling the polling thread, which is still joinable.
            assert_eq!(
                unsafe { libc::pthread_kill(thread as libc::pthread_t, libc::SIGUSR2) },
                0
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(poller.join().unwrap(), (-(crate::errno::EINTR as i64), 0));
        assert_eq!(lowest_free(), free);
        close(&[fd, other]);
    }

    /// A pipe end of `pipe2` whose other end is closed after `fill`ing the
    /// pipe from its write end: 1 byte, or until full.
    fn widowed_end(end: usize, full: bool) -> i64 {
        let mut pipe = [0i32; 2];
        assert_eq!(
            fs::pipe2([pipe.as_mut_ptr() as u64, O_NONBLOCK | O_CLOEXEC, 0, 0, 0, 0]),
            0
        );
        // SAFETY: writing to our own non-blocking pipe.
        while unsafe { libc::write(pipe[1], [0u8; 512].as_ptr().cast(), 1 + 511 * full as usize) }
            > 0
            && full
        {}
        close(&[pipe[1 - end] as i64]);
        // A host process a test running in parallel is spawning holds a
        // copy of every descriptor until its exec drops the close-on-exec
        // ones: the other end is gone once Darwin reports the hang-up.
        let mut host = libc::pollfd {
            fd: pipe[end],
            events: POLLHUP,
            revents: 0,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        // SAFETY: one pollfd on our stack.
        while unsafe { libc::poll(&mut host, 1, 0) } >= 0
            && host.revents & POLLHUP == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_ne!(host.revents & POLLHUP, 0, "the other end stayed open");
        pipe[end] as i64
    }

    /// `pipe_poll` never reports POLLPRI, POLLRDBAND or POLLWRBAND, which
    /// Darwin's poll reports for a readable or writable pipe end.
    #[test]
    fn a_pipe_is_never_priority_or_band() {
        let _view = crate::vfs::test_view();
        let mut pipe = [0i32; 2];
        assert_eq!(fs::pipe2([pipe.as_mut_ptr() as u64, 0, 0, 0, 0, 0]), 0);
        let (read, write) = (pipe[0] as i64, pipe[1] as i64);
        assert_eq!(polled(write, ALL), (1, POLLOUT | POLLWRNORM));
        assert_eq!(
            fs::write([write as u64, b"x".as_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(polled(read, ALL), (1, POLLIN | POLLRDNORM));
        assert_eq!(polled(read, POLLPRI | POLLRDBAND), (0, 0));
        let mut except = [0u64; 16];
        except[read as usize / 64] = 1 << (read % 64);
        let timeout = [0i64; 2];
        assert_eq!(
            pselect6([
                read as u64 + 1,
                0,
                0,
                except.as_mut_ptr() as u64,
                timeout.as_ptr() as u64,
                0,
            ]),
            0
        );
        close(&[read, write]);
    }

    /// `pipe_poll` reports a named FIFO's ends, one open for both included,
    /// as a pipe's: never POLLPRI, POLLRDBAND, POLLWRBAND or POLLRDHUP.
    #[test]
    fn a_fifo_is_never_priority_band_or_rdhup() {
        let (_view, dir) = crate::vfs::test_view();
        let name = format!("poll-fifo-{}", std::process::id());
        let host = std::ffi::CString::new(dir.join("data").join(&name).to_str().unwrap()).unwrap();
        // SAFETY: a new FIFO in the test view's data.
        assert_eq!(unsafe { libc::mkfifo(host.as_ptr(), 0o600) }, 0);
        let guest = std::ffi::CString::new(format!("/data/{name}")).unwrap();
        let open = |mode: u64| {
            let fd = fs::openat([
                -100i64 as u64,
                guest.as_ptr() as u64,
                mode | O_NONBLOCK,
                0,
                0,
                0,
            ]);
            assert!(fd >= 0, "mode {mode}: {fd}");
            fd
        };
        let (read, write, both) = (open(0), open(1), open(2));
        assert_eq!(polled(write, ALL), (1, POLLOUT | POLLWRNORM));
        assert_eq!(
            fs::write([write as u64, b"x".as_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(polled(read, ALL), (1, POLLIN | POLLRDNORM));
        assert_eq!(
            polled(both, ALL | POLLRDHUP),
            (1, POLLIN | POLLRDNORM | POLLOUT | POLLWRNORM)
        );
        assert_eq!(polled(both, POLLPRI | POLLRDHUP), (0, 0));
        let mut except = [0u64; 16];
        except[both as usize / 64] = 1 << (both % 64);
        let timeout = [0i64; 2];
        assert_eq!(
            pselect6([
                both as u64 + 1,
                0,
                0,
                except.as_mut_ptr() as u64,
                timeout.as_ptr() as u64,
                0,
            ]),
            0
        );
        close(&[read, write, both]);
        // SAFETY: the FIFO made above.
        assert_eq!(unsafe { libc::unlink(host.as_ptr()) }, 0);
    }

    /// `pipe_poll`: a write end without readers is POLLERR, never POLLHUP,
    /// with POLLOUT|POLLWRNORM when the pipe isn't full, and readable and
    /// writable to select, which counts POLLERR in both sets (LTP poll01,
    /// epoll_wait02, select01 style).
    #[test]
    fn a_pipe_writer_without_readers_is_pollerr() {
        let _view = crate::vfs::test_view();
        const EPOLLOUT: u64 = 0x4;
        const EPOLLERR: u64 = 0x8;
        const EPOLLWRNORM: u64 = 0x100;
        const EPOLLET: u32 = 1 << 31;
        let fd = widowed_end(1, false);
        assert_eq!(
            polled(fd, POLLOUT | POLLWRNORM),
            (1, POLLOUT | POLLWRNORM | POLLERR)
        );
        assert_eq!(polled(fd, ALL), (1, POLLOUT | POLLWRNORM | POLLERR));
        assert_eq!(polled(fd, POLLIN), (1, POLLERR));
        assert_eq!(polled(fd, 0), (1, POLLERR));
        assert_eq!(selected(fd), (2, true, true));
        for (events, fired) in [
            (0x4 | 0x100, EPOLLOUT | EPOLLWRNORM | EPOLLERR),
            (0x1 | 0x4, EPOLLOUT | EPOLLERR),
            (0x1, EPOLLERR),
            (0, EPOLLERR),
        ] {
            let level = watching(fd, events);
            assert_eq!(waited(level, 0), (1, fired), "events {events:#x}");
            assert_eq!(waited(level, 0), (1, fired), "events {events:#x}");
            let edge = watching(fd, events | EPOLLET);
            assert_eq!(waited(edge, 0), (1, fired), "events {events:#x}");
            assert_eq!(waited(edge, 0).0, 0, "events {events:#x}");
            close(&[level, edge]);
        }
        close(&[fd]);
        // Darwin frees the pipe's buffer with its last reader, so a full
        // pipe's writer reports room too, where Linux's is POLLERR alone
        // (#2815).
        let fd = widowed_end(1, true);
        assert_eq!(polled(fd, POLLOUT), (1, POLLOUT | POLLERR));
        assert_eq!(selected(fd), (2, true, true));
        let level = watching(fd, 0x4);
        assert_eq!(waited(level, 0), (1, EPOLLOUT | EPOLLERR));
        close(&[level, fd]);
    }

    /// `pipe_poll`: a read end without writers is POLLHUP, with
    /// POLLIN|POLLRDNORM while data is queued, and only readable to select.
    #[test]
    fn a_pipe_reader_without_writers_hangs_up() {
        let _view = crate::vfs::test_view();
        const EPOLLIN: u64 = 0x1;
        const EPOLLHUP: u64 = 0x10;
        const EPOLLRDNORM: u64 = 0x40;
        const EPOLLET: u32 = 1 << 31;
        let fd = widowed_end(0, false);
        assert_eq!(
            polled(fd, POLLIN | POLLRDNORM),
            (1, POLLIN | POLLRDNORM | POLLHUP)
        );
        assert_eq!(polled(fd, ALL), (1, POLLIN | POLLRDNORM | POLLHUP));
        assert_eq!(polled(fd, POLLIN | POLLRDHUP), (1, POLLIN | POLLHUP));
        assert_eq!(selected(fd), (1, true, false));
        for (events, fired) in [
            (0x1 | 0x40, EPOLLIN | EPOLLRDNORM | EPOLLHUP),
            (0x4, EPOLLHUP),
            (0, EPOLLHUP),
        ] {
            let level = watching(fd, events);
            assert_eq!(waited(level, 0), (1, fired), "events {events:#x}");
            assert_eq!(waited(level, 0), (1, fired), "events {events:#x}");
            let edge = watching(fd, events | EPOLLET);
            assert_eq!(waited(edge, 0), (1, fired), "events {events:#x}");
            assert_eq!(waited(edge, 0).0, 0, "events {events:#x}");
            close(&[level, edge]);
        }
        let mut byte = 0u8;
        assert_eq!(
            fs::read([fd as u64, &mut byte as *mut u8 as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(polled(fd, POLLIN | POLLRDNORM), (1, POLLHUP));
        assert_eq!(polled(fd, 0), (1, POLLHUP));
        assert_eq!(polled(fd, POLLRDHUP), (1, POLLHUP));
        assert_eq!(selected(fd), (1, true, false));
        let level = watching(fd, 0x1 | 0x2000);
        assert_eq!(waited(level, 0), (1, EPOLLHUP));
        close(&[level, fd]);
    }

    /// inotify_poll (man 7 inotify, LTP inotify): poll, select and a
    /// level-triggered epoll find an inotify readable exactly while events
    /// are queued, so a wait on one whose host notes decoded to nothing
    /// (here a delete outside its IN_CREATE mask) sleeps, and IN_IGNORED of
    /// a removed watch makes it readable.
    #[test]
    fn an_inotify_is_readable_only_while_events_are_queued() {
        let (_vfs, view) = crate::vfs::test_view();
        let dir = view.join("data/inotify-readiness");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let notify = inotify::inotify_init1([O_NONBLOCK, 0, 0, 0, 0, 0]);
        assert!(notify >= 0);
        let path = std::ffi::CString::new("/data/inotify-readiness").unwrap();
        const IN_CREATE: u64 = 0x100;
        let wd =
            inotify::inotify_add_watch([notify as u64, path.as_ptr() as u64, IN_CREATE, 0, 0, 0]);
        assert!(wd > 0, "{wd}");
        let ep = watching(notify, 0x1);
        let mut buf = [0u64; 32];
        let buf = buf.as_mut_ptr();
        let read = || inotify::read(notify as i32, buf as u64, 256);
        let quiet = |what: &str| {
            assert_eq!(polled(notify, ALL), (0, 0), "{what}");
            assert_eq!(selected(notify), (0, false, false), "{what}");
            assert_eq!(waited(ep, 0).0, 0, "{what}");
        };
        let queued = |what: &str| {
            assert_eq!(polled(notify, ALL), (1, POLLIN | POLLRDNORM), "{what}");
            assert_eq!(selected(notify), (1, true, false), "{what}");
            assert_eq!(waited(ep, 0), (1, 0x1), "{what}");
        };
        quiet("empty");
        let file = dir.join("file");
        std::fs::write(&file, b"x").unwrap();
        queued("created");
        assert_eq!(read(), Some(32));
        quiet("drained");
        assert_eq!(read(), Some(-(crate::errno::EAGAIN as i64)));
        // Each check after its own note, as a check collects the note.
        let noise = |n: usize| {
            let entry = dir.join(format!("noise-{n}"));
            std::fs::write(&entry, b"").unwrap();
            assert_eq!(read(), Some(32));
            std::fs::remove_file(entry).unwrap();
        };
        noise(0);
        assert_eq!(polled(notify, ALL), (0, 0));
        noise(1);
        assert_eq!(selected(notify), (0, false, false));
        noise(2);
        let cpu = || {
            let mut t = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            assert_eq!(
                unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) },
                0
            );
            t.tv_sec as i64 * 1_000_000_000 + t.tv_nsec as i64
        };
        let (started, spent) = (std::time::Instant::now(), cpu());
        assert_eq!(waited(ep, 200).0, 0);
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
        assert!(cpu() - spent < 50_000_000, "epoll_pwait spun");
        noise(3);
        let mut entry = libc::pollfd {
            fd: notify as i32,
            events: POLLIN,
            revents: 0,
        };
        let mut timeout = [0i64, 200_000_000];
        let started = std::time::Instant::now();
        assert_eq!(
            ppoll([
                &mut entry as *mut _ as u64,
                1,
                timeout.as_mut_ptr() as u64,
                0,
                0,
                0
            ]),
            0
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
        noise(4);
        let (word, bit) = (notify as usize / 64, 1u64 << (notify % 64));
        let mut set = [0u64; 16];
        set[word] = bit;
        let mut timeout = [0i64, 200_000_000];
        let started = std::time::Instant::now();
        assert_eq!(
            pselect6([
                notify as u64 + 1,
                set.as_mut_ptr() as u64,
                0,
                0,
                timeout.as_mut_ptr() as u64,
                0,
            ]),
            0
        );
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));
        assert_eq!((set[word], timeout), (0, [0, 0]));
        assert_eq!(
            inotify::inotify_rm_watch([notify as u64, wd as u64, 0, 0, 0, 0]),
            0
        );
        queued("watch removed");
        assert_eq!(read(), Some(16));
        assert_eq!(unsafe { buf.read() } >> 32, 0x8000, "IN_IGNORED");
        quiet("IN_IGNORED read");
        std::fs::remove_dir_all(&dir).unwrap();
        close(&[ep, notify]);
    }
}
