//! ppoll and pselect6 over Darwin poll. Every guest fd (eventfd, timerfd,
//! epoll and inotify included) is a host fd whose poll readiness already
//! has Linux meaning, so only the event bits and timeouts are translated.

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

/// Install `mask` (a guest sigset pointer, 0 for none) for the duration of a
/// wait; returns the mask to restore.
pub fn swap_sigmask(mask: u64) -> Option<u64> {
    if mask == 0 {
        return None;
    }
    let mut old = 0u64;
    super::signal::rt_sigprocmask([2, mask, &mut old as *mut u64 as u64, 8, 0, 0]);
    Some(old)
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
    let t = unsafe { (ts as *const [i64; 2]).read_unaligned() };
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
fn update_timeout(ts: u64, t0: i64) {
    if ts == 0 {
        return;
    }
    // SAFETY: guest struct timespec.
    unsafe {
        let t = (ts as *const [i64; 2]).read_unaligned();
        let left = (t[0] * 1_000_000_000 + t[1] - (now_ns() - t0)).max(0);
        (ts as *mut [i64; 2]).write_unaligned([left / 1_000_000_000, left % 1_000_000_000]);
    }
}

fn host_poll(fds: &mut [libc::pollfd], ms: i32, mask: u64) -> i64 {
    let old = swap_sigmask(mask);
    // SAFETY: a pollfd array we own.
    let fuse_ready=fds.iter().any(|descriptor|super::fuse_device::readiness(descriptor.fd).is_some_and(|events|events.is_ok_and(|events|events&(descriptor.events|POLLERR|POLLHUP)!=0)));
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as u32, if fuse_ready{0}else{ms}) };
    let err = errno::last();
    restore_sigmask(old);
    if n < 0 { -(err as i64) } else { n as i64 }
}
#[cfg(test)]
thread_local!{pub(super) static BEFORE_WAIT:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const{std::cell::RefCell::new(None)};}

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
    let guest = unsafe { std::slice::from_raw_parts_mut(fds as *mut libc::pollfd, nfds as usize) };
    let mut pins = Vec::with_capacity(guest.len());
    let mut ptys = Vec::with_capacity(guest.len());
    let mut invalid = false;
    let mut host = Vec::with_capacity(guest.len());
    for entry in guest.iter() {
        let mut pin = if entry.fd < 0 { None } else {
            match super::fdtab::pin_guest(entry.fd) {
                Ok(pin) if !matches!(pin.kind(), Some(super::fdtab::Kind::Path(_))) => Some(pin),
                Ok(_) => { invalid = true; None },
                Err(EBADF) => { invalid = true; None },
                Err(error) => return -(error as i64),
            }
        };
        let pty=match pin.as_ref().and_then(|pin|pin.kind()){
            Some(super::fdtab::Kind::Pty(description))=>match description.watch(){Ok(watch)=>Some(watch),Err(error)=>return -(error as i64)},_=>None,
        };
        let fd=pty.as_ref().map_or_else(||pin.as_ref().map_or(-1,|pin|pin.descriptor().as_raw_fd()),|watch|watch.observation.data().as_raw_fd());
        if pty.is_some(){pin=None;}
        if entry.events & POLLIN != 0 && fd >= 0 { super::binder::poll(fd); }
        host.push(libc::pollfd { fd, events: to_host(entry.events), revents: 0 });
        pins.push(pin);
        ptys.push(pty);
    }
    for watch in ptys.iter().flatten(){
        let Some(notification)=watch.observation.notification()else{return -(crate::errno::EIO as i64);};
        host.push(libc::pollfd{fd:notification.as_raw_fd(),events:libc::POLLIN,revents:0});
    }
    if pins.iter().flatten().any(super::close_effects::observes){
        if let Err(error)=super::close_effects::observe_socket(){return -(error as i64);}
    }
    let t0 = now_ns();
    #[cfg(test)]
    BEFORE_WAIT.with(|hook|{let action=hook.borrow_mut().take();if let Some(action)=action{action();}});
    loop {
        let left = if ms < 0 {
            -1
        } else {
            (ms as i64 - (now_ns() - t0) / 1_000_000).max(0) as i32
        };
        let n = host_poll(&mut host, if invalid { 0 } else { left }, mask);
        if n < 0 {
            return n;
        }
        let mut ready = 0;
        for (((g, h), pin),pty) in guest.iter_mut().zip(&mut host).zip(&pins).zip(&ptys) {
            if g.fd >= 0 && pin.is_none()&&pty.is_none() { g.revents = POLLNVAL; ready += 1; continue; }
            if let Some(watch)=pty{
                h.revents=watch.events(h.revents);
            }
            if h.revents & libc::POLLIN != 0 && super::inotify::spuriously_ready(h.fd) {
                h.revents &= !libc::POLLIN;
            }
            if let Some(events)=super::fuse_device::readiness(h.fd){h.revents=match events{Ok(events)=>events,Err(_)=>POLLERR};}
            g.revents = from_host(h.revents, g.events);
            if g.revents != 0 {
                ready += 1;
            }
        }
        if ready > 0 || n == 0 || left == 0 {
            update_timeout(ts, t0);
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
    let bit = |set: u64, fd: usize| -> bool {
        // SAFETY: guest fd_set of at least nfds bits (u64 words).
        set != 0 && unsafe { *(set as *const u64).add(fd / 64) } & (1 << (fd % 64)) != 0
    };
    let wanted = [POLLIN, POLLOUT, POLLPRI];
    let mut host = Vec::new();
    let mut pins = Vec::new();
    let mut ptys = Vec::new();
    let mut guest_fds = Vec::new();
    for fd in 0..nfds {
        let mut ev = 0;
        for (s, w) in sets.iter().zip(wanted) {
            if bit(*s, fd) {
                ev |= w;
            }
        }
        if ev != 0 {
            let pin = match super::fdtab::pin_guest(fd as i32) { Ok(pin) => pin, Err(error) => return -(error as i64) };
            if matches!(pin.kind(), Some(super::fdtab::Kind::Path(_))) { return -(EBADF as i64); }
            let pty=match pin.kind(){Some(super::fdtab::Kind::Pty(description))=>match description.watch(){Ok(watch)=>Some(watch),Err(error)=>return -(error as i64)},_=>None};
            let host_fd=pty.as_ref().map_or_else(||pin.descriptor().as_raw_fd(),|watch|watch.observation.data().as_raw_fd());
            if ev & POLLIN != 0 { super::binder::poll(host_fd); }
            host.push(libc::pollfd { fd: host_fd, events: ev, revents: 0 });
            guest_fds.push(fd);
            pins.push(if pty.is_none(){Some(pin)}else{None});
            ptys.push(pty);
        }
    }
    for watch in ptys.iter().flatten(){
        let Some(notification)=watch.observation.notification()else{return -(crate::errno::EIO as i64);};
        host.push(libc::pollfd{fd:notification.as_raw_fd(),events:libc::POLLIN,revents:0});
    }
    // The 6th argument is { const sigset_t *ss; size_t ss_len; }.
    let mask = if sig == 0 {
        0
    } else {
        // SAFETY: guest struct.
        unsafe { (sig as *const u64).read_unaligned() }
    };
    if pins.iter().flatten().any(super::close_effects::observes){
        if let Err(error)=super::close_effects::observe_socket(){return -(error as i64);}
    }
    let t0 = now_ns();
    let n = host_poll(&mut host, ms, mask);
    if n < 0 {
        return n;
    }
    for descriptor in &mut host{if let Some(events)=super::fuse_device::readiness(descriptor.fd){descriptor.revents=match events{Ok(events)=>events,Err(_)=>POLLERR};}}
    for (descriptor,watch) in host.iter_mut().zip(&ptys){if let Some(watch)=watch{descriptor.revents=watch.events(descriptor.revents);}}
    if host.iter().any(|p| p.revents & POLLNVAL != 0) {
        return -(EBADF as i64);
    }
    update_timeout(ts, t0);
    for s in sets.iter().filter(|&&s| s != 0) {
        // SAFETY: clearing the guest fd_set words that cover nfds.
        unsafe { std::ptr::write_bytes(*s as *mut u64, 0, nfds.div_ceil(64)) };
    }
    let mut count = 0;
    for (p, fd) in host.iter().zip(guest_fds) {
        let hits = [
            p.revents & (POLLIN | POLLHUP | POLLERR) != 0,
            p.revents & (POLLOUT | POLLERR) != 0,
            p.revents & POLLPRI != 0,
        ];
        for ((s, w), hit) in sets.iter().zip(wanted).zip(hits) {
            if *s != 0 && p.events & w != 0 && hit {
                // SAFETY: guest fd_set word covering fd.
                unsafe { *(*s as *mut u64).add(fd / 64) |= 1 << (fd % 64) };
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod pin_tests {
    use super::*;
    use std::os::fd::FromRawFd;

    #[test]
    fn poll_keeps_unknown_and_negative_fd_semantics_and_select_guest_indexes() {
        if super::super::fdtab::isolated_kernel_test("sys::poll::pin_tests::poll_keeps_unknown_and_negative_fd_semantics_and_select_guest_indexes") { return; }
        let (_view, _root) = crate::vfs::test_view();
        let mut raw = [0; 2];
        assert_eq!(unsafe { libc::pipe(raw.as_mut_ptr()) }, 0);
        let read = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw[0]) };
        let write = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw[1]) };
        super::super::fdtab::publish_guest(read.as_raw_fd()).unwrap();
        let byte = b'x';
        assert_eq!(unsafe { libc::write(write.as_raw_fd(), (&byte as *const u8).cast(), 1) }, 1);
        let mut entries = [libc::pollfd { fd: read.as_raw_fd(), events: POLLIN, revents: 0 },
            libc::pollfd { fd: write.as_raw_fd(), events: POLLOUT, revents: 0 },
            libc::pollfd { fd: -1, events: POLLIN, revents: 0 }];
        let timeout = [0u64; 2];
        assert_eq!(ppoll([entries.as_mut_ptr() as u64, 3, timeout.as_ptr() as u64, 0, 0, 0]), 2);
        assert_eq!(entries[0].revents, POLLIN);
        assert_eq!(entries[1].revents, POLLNVAL);
        assert_eq!(entries[2].revents, 0);
        let fd = read.as_raw_fd() as usize;
        let mut selected = vec![0u64; (fd + 1).div_ceil(64)];
        selected[fd / 64] |= 1 << (fd % 64);
        assert_eq!(pselect6([(fd + 1) as u64, selected.as_mut_ptr() as u64, 0, 0, timeout.as_ptr() as u64, 0]), 1);
        assert_eq!(selected[fd / 64], 1 << (fd % 64));
        super::super::fdtab::withdraw_guest(read.as_raw_fd()).unwrap();
        assert_eq!(pselect6([(fd + 1) as u64, selected.as_mut_ptr() as u64, 0, 0, timeout.as_ptr() as u64, 0]), -(EBADF as i64));
    }
}
