//! ppoll and pselect6 over Darwin poll. Every guest fd (eventfd, timerfd,
//! epoll and inotify included) is a host fd whose poll readiness has Linux
//! meaning for the events its Linux `_poll` can report ([`reportable`]),
//! so only those are asked and the event bits and timeouts translated; a
//! signalfd is also readable for the polling thread's own signals, which
//! its wake token reports.

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
/// `pidfd_poll`, which adds POLLHUP once the process is reaped).
pub(super) fn reportable(fd: i32, kind: Option<&super::fdtab::Kind>) -> Option<i16> {
    use super::fdtab::Kind;
    match kind {
        Some(Kind::Signal(_) | Kind::Timer(_)) => Some(POLLIN),
        Some(Kind::Event(_)) => Some(POLLIN | POLLOUT),
        Some(Kind::Epoll(_) | Kind::Inotify(_)) => Some(POLLIN | POLLRDNORM),
        None if super::wait::is_pidfd(fd) => Some(POLLIN | POLLRDNORM),
        _ => None,
    }
}

/// The events of `asked` that guest fd `fd` can report, a signalfd's as
/// of a mask read now: those polled for, and the only ones its host
/// readiness is translated back to.
fn asked_events(fd: i32, kind: Option<&super::fdtab::Kind>, asked: i16) -> i16 {
    if let Some(super::fdtab::Kind::Signal(description)) = kind {
        super::signalfd::ready(description);
    }
    reportable(fd, kind).map_or(asked, |events| asked & events)
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
        entry.events = asked_events(
            entry.fd,
            pin.as_ref().and_then(|pin| pin.kind()),
            entry.events,
        );
        if pty.is_some() {
            pin = None;
        }
        if entry.events & POLLIN != 0 && fd >= 0 {
            super::binder::poll(fd);
        }
        host.push(libc::pollfd {
            fd,
            events: to_host(entry.events),
            revents: 0,
        });
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
        let n = host_poll(&mut host, if invalid { 0 } else { left }, mask);
        if n < 0 {
            return n;
        }
        let mut ready = 0;
        for (((g, h), pin), pty) in guest.iter_mut().zip(&mut host).zip(&pins).zip(&ptys) {
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
            g.revents = match (h.revents & POLLHUP != 0)
                .then(|| super::tcp_shutdown::polled_hangup(h.fd))
                .flatten()
            {
                // A TCP socket's shutdown, which Darwin reports as HUP
                // alone, as tcp_poll reports it.
                Some(tcp) => {
                    let mut revents = h.revents & !POLLHUP;
                    if tcp.receive {
                        revents |= POLLIN | POLLRDNORM;
                    }
                    if tcp.send || super::tcp_shutdown::writable(h.fd) {
                        revents |= libc::POLLOUT;
                    }
                    if tcp.hangup() {
                        revents |= POLLHUP;
                    }
                    from_host(revents, g.events)
                        | if tcp.receive { g.events & POLLRDHUP } else { 0 }
                }
                None => from_host(h.revents, g.events),
            };
            if h.revents & libc::POLLIN != 0 && super::wait::pidfd_reaped(g.fd) {
                g.revents |= POLLHUP;
            }
            if g.revents != 0 {
                ready += 1;
            }
        }
        if ready > 0 || n == 0 || left == 0 {
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
            ev = to_host(asked_events(fd as i32, pin.kind(), ev));
            host.push(libc::pollfd {
                fd: host_fd,
                events: ev,
                revents: 0,
            });
            guest_fds.push(fd);
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
        let n = host_poll(&mut host, left, mask);
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
        for ((descriptor, watch), pin) in host.iter_mut().zip(&ptys).zip(&pins) {
            if let Some(watch) = watch {
                descriptor.revents = watch.events(descriptor.revents);
            }
            signal_revents(pin.as_ref(), descriptor);
            inotify_revents(descriptor);
        }
        if host.iter().any(|p| p.revents & POLLNVAL != 0) {
            return -(EBADF as i64);
        }
        let mut output_sets = vec![vec![0u8; nfds.div_ceil(64) * 8]; 3];
        let mut count = 0;
        for (p, &fd) in host.iter().zip(&guest_fds) {
            let hits = [
                p.revents & (POLLIN | POLLHUP | POLLERR) != 0,
                p.revents & (POLLOUT | POLLERR) != 0,
                p.revents & POLLPRI != 0,
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
        if count > 0 || n == 0 || left == 0 {
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
        let timeout = [0u64; 2];
        assert_eq!(
            ppoll([
                entries.as_mut_ptr() as u64,
                3,
                timeout.as_ptr() as u64,
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
                timeout.as_ptr() as u64,
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
                timeout.as_ptr() as u64,
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
