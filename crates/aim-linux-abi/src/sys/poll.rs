//! ppoll and pselect6 over Darwin poll. Every guest fd (eventfd, timerfd,
//! epoll and inotify included) is a host fd whose poll readiness already
//! has Linux meaning, so only the event bits and timeouts are translated.

use crate::errno::{self, EBADF, EINVAL};

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
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as u32, ms) };
    let err = errno::last();
    restore_sigmask(old);
    if n < 0 { -(err as i64) } else { n as i64 }
}

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
    let mut host: Vec<libc::pollfd> = guest
        .iter()
        .map(|p| {
            if p.events & POLLIN != 0 {
                // As Linux's binder_poll, from the poll path.
                super::binder::poll(p.fd);
            }
            libc::pollfd {
                fd: p.fd,
                events: to_host(p.events),
                revents: 0,
            }
        })
        .collect();
    let t0 = now_ns();
    loop {
        let left = if ms < 0 {
            -1
        } else {
            (ms as i64 - (now_ns() - t0) / 1_000_000).max(0) as i32
        };
        let n = host_poll(&mut host, left, mask);
        if n < 0 {
            return n;
        }
        let mut ready = 0;
        for (g, h) in guest.iter_mut().zip(&mut host) {
            if h.revents & libc::POLLIN != 0 && super::inotify::spuriously_ready(h.fd) {
                h.revents &= !libc::POLLIN;
            }
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
    for fd in 0..nfds {
        let mut ev = 0;
        for (s, w) in sets.iter().zip(wanted) {
            if bit(*s, fd) {
                ev |= w;
            }
        }
        if ev & POLLIN != 0 {
            super::binder::poll(fd as i32);
        }
        if ev != 0 {
            host.push(libc::pollfd {
                fd: fd as i32,
                events: ev,
                revents: 0,
            });
        }
    }
    // The 6th argument is { const sigset_t *ss; size_t ss_len; }.
    let mask = if sig == 0 {
        0
    } else {
        // SAFETY: guest struct.
        unsafe { (sig as *const u64).read_unaligned() }
    };
    let t0 = now_ns();
    let n = host_poll(&mut host, ms, mask);
    if n < 0 {
        return n;
    }
    if host.iter().any(|p| p.revents & POLLNVAL != 0) {
        return -(EBADF as i64);
    }
    update_timeout(ts, t0);
    for s in sets.iter().filter(|&&s| s != 0) {
        // SAFETY: clearing the guest fd_set words that cover nfds.
        unsafe { std::ptr::write_bytes(*s as *mut u64, 0, nfds.div_ceil(64)) };
    }
    let mut count = 0;
    for p in &host {
        let fd = p.fd as usize;
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
