//! epoll over kqueue (after FreeBSD's Linuxulator, `linux_event.c`,
//! BSD-2-Clause).
//!
//! - An epoll fd is a kqueue; an interest becomes EVFILT_READ and/or
//!   EVFILT_WRITE knotes with the fd as ident. Level-triggered is kqueue's
//!   default, EPOLLET maps to EV_CLEAR and EPOLLONESHOT to EV_DISPATCH (the
//!   sibling filter is disabled on delivery, as epoll disables the whole
//!   interest). EPOLLEXCLUSIVE is accepted and has no effect.
//! - A kqueue is itself pollable, so epoll fds nest and work in poll().
//! - The interest list is kept here too: Darwin does not inherit kqueues
//!   across fork, so a fork child rebuilds each one on the same fd number.
//!   The kernel stays authoritative for whether an fd is registered: a
//!   closed fd drops its knotes, and ADD checks the kernel before EEXIST.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::fdtab::{self, Kind};
use crate::errno::{self, EBADF, EEXIST, EINVAL, ENOENT, EPERM};

const EPOLLIN: u32 = 0x1;
const EPOLLPRI: u32 = 0x2;
const EPOLLOUT: u32 = 0x4;
const EPOLLERR: u32 = 0x8;
const EPOLLHUP: u32 = 0x10;
const EPOLLRDNORM: u32 = 0x40;
const EPOLLWRNORM: u32 = 0x100;
const EPOLLRDHUP: u32 = 0x2000;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;

const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLL_CTL_MOD: u64 = 3;
const EPOLL_CLOEXEC: u64 = 0o2000000;

#[derive(Clone, Copy)]
struct Interest {
    events: u32,
    data: u64,
    /// Delivered with EPOLLONESHOT and not re-armed yet.
    disabled: bool,
}

impl Interest {
    fn wants_read(&self) -> bool {
        self.events & (EPOLLIN | EPOLLRDNORM | EPOLLRDHUP | EPOLLPRI) != 0
    }
    fn wants_write(&self) -> bool {
        self.events & (EPOLLOUT | EPOLLWRNORM) != 0
    }
}

pub struct Epoll {
    cloexec: bool,
    interest: Mutex<HashMap<i32, Interest>>,
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

/// The knote changes that make `kq` match `new` for `fd` (from `old`).
/// Darwin keeps a knote's EV_CLEAR/EV_DISPATCH from its creation, so a
/// modified interest replaces its knotes.
fn changes(fd: i32, old: Option<&Interest>, new: Option<&Interest>) -> Vec<libc::kevent> {
    let mut v = Vec::new();
    for (filter, want) in [
        (
            libc::EVFILT_READ,
            Interest::wants_read as fn(&Interest) -> bool,
        ),
        (libc::EVFILT_WRITE, Interest::wants_write),
    ] {
        if old.is_some_and(want) {
            v.push(kev(fd, filter, libc::EV_DELETE));
        }
        if let Some(n) = new.filter(|n| want(n)) {
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
            v.push(kev(fd, filter, f));
        }
    }
    v
}

/// Whether the kernel still has a knote for `fd` in `kq`. A change with no
/// action flags only reports whether the knote exists.
fn registered(kq: i32, fd: i32, i: &Interest) -> bool {
    let filter = if i.wants_read() {
        libc::EVFILT_READ
    } else if i.wants_write() {
        libc::EVFILT_WRITE
    } else {
        return true;
    };
    apply(kq, &[kev(fd, filter, 0)]) == 0
}

fn epoll_of(epfd: i32) -> Result<Arc<Epoll>, i64> {
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
    let cloexec = flags & EPOLL_CLOEXEC != 0;
    fdtab::set_flags(kq, false, cloexec);
    fdtab::insert(
        kq,
        Kind::Epoll(Arc::new(Epoll {
            cloexec,
            interest: Mutex::new(HashMap::new()),
        })),
    );
    kq as i64
}

pub fn epoll_ctl(a: [u64; 6]) -> i64 {
    let (epfd, op, fd, ev) = (a[0] as i32, a[1], a[2] as i32, a[3]);
    let ep = match epoll_of(epfd) {
        Ok(e) => e,
        Err(e) => return e,
    };
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
    let new = (op != EPOLL_CTL_DEL).then(|| {
        // SAFETY: guest struct epoll_event (arm64: u32 events, u64 data at 8).
        let raw = unsafe { (ev as *const [u64; 2]).read_unaligned() };
        Interest {
            events: raw[0] as u32,
            data: raw[1],
            disabled: false,
        }
    });
    let mut interest = ep.interest.lock().unwrap();
    let old = interest
        .get(&fd)
        .copied()
        .filter(|i| registered(epfd, fd, i));
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
    let r = apply(epfd, &changes(fd, old.as_ref(), new.as_ref()));
    if r < 0 {
        return r;
    }
    match new {
        Some(n) => interest.insert(fd, n),
        None => interest.remove(&fd),
    };
    0
}

/// Linux epoll event bits for one kevent of an interest.
fn bits(k: &libc::kevent, i: &Interest) -> u32 {
    let eof = k.flags & libc::EV_EOF != 0;
    let mut b = 0;
    if k.flags & libc::EV_ERROR != 0 {
        return EPOLLERR;
    }
    if k.filter == libc::EVFILT_READ {
        b |= i.events & (EPOLLIN | EPOLLRDNORM);
        if eof {
            b |= EPOLLHUP | (i.events & EPOLLRDHUP);
            if k.fflags != 0 {
                b |= EPOLLERR;
            }
        }
    } else if k.filter == libc::EVFILT_WRITE {
        b |= i.events & (EPOLLOUT | EPOLLWRNORM);
        if eof {
            b |= if k.fflags != 0 { EPOLLERR } else { EPOLLHUP };
        }
    }
    b
}

fn wait(epfd: i32, events: u64, maxevents: i32, timeout: Option<libc::timespec>, mask: u64) -> i64 {
    let ep = match epoll_of(epfd) {
        Ok(e) => e,
        Err(e) => return e,
    };
    if maxevents <= 0 || maxevents > i32::MAX / 16 {
        return -(EINVAL as i64);
    }
    let mut kevs: Vec<libc::kevent> = Vec::with_capacity(maxevents as usize);
    let deadline = timeout.map(|t| now_ns() + t.tv_sec * 1_000_000_000 + t.tv_nsec);
    let (out, rearm) = loop {
        let left = deadline.map(|d| {
            let ns = (d - now_ns()).max(0);
            libc::timespec {
                tv_sec: ns / 1_000_000_000,
                tv_nsec: ns % 1_000_000_000,
            }
        });
        let old_mask = super::poll::swap_sigmask(mask);
        // SAFETY: output array with capacity `maxevents`.
        let n = unsafe {
            libc::kevent(
                epfd,
                std::ptr::null(),
                0,
                kevs.as_mut_ptr(),
                maxevents,
                left.as_ref().map_or(std::ptr::null(), |t| t as *const _),
            )
        };
        let err = errno::last();
        super::poll::restore_sigmask(old_mask);
        if n < 0 {
            return -(err as i64);
        }
        // SAFETY: the kernel filled n entries.
        unsafe { kevs.set_len(n as usize) };
        let found = ready(&ep, &kevs);
        // An inotify fd that woke the wait with nothing to read: wait on.
        if !found.0.is_empty() || n == 0 || left.is_some_and(|t| t.tv_sec == 0 && t.tv_nsec == 0) {
            break found;
        }
    };
    apply(epfd, &rearm);
    for (n, (_, b, data)) in out.iter().enumerate() {
        // SAFETY: guest array of `maxevents` 16-byte struct epoll_event.
        unsafe {
            (events as *mut [u64; 2])
                .add(n)
                .write_unaligned([*b as u64, *data])
        };
    }
    out.len() as i64
}

fn now_ns() -> i64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec * 1_000_000_000 + t.tv_nsec
}

/// The ready interests among `kevs` as (fd, events, data), and the
/// changes that disable the other filter of the one-shot ones.
fn ready(ep: &Epoll, kevs: &[libc::kevent]) -> (Vec<(i32, u32, u64)>, Vec<libc::kevent>) {
    let mut out: Vec<(i32, u32, u64)> = Vec::with_capacity(kevs.len());
    let mut rearm = Vec::new();
    let mut interest = ep.interest.lock().unwrap();
    for k in kevs {
        let fd = k.ident as i32;
        let Some(i) = interest.get_mut(&fd) else {
            continue;
        };
        let mut b = bits(k, i);
        if b & EPOLLIN != 0 && super::inotify::spuriously_ready(fd) {
            b &= !(EPOLLIN | EPOLLRDNORM);
        }
        if b == 0 {
            continue;
        }
        if let Some(e) = out.iter_mut().find(|e| e.0 == fd) {
            e.1 |= b;
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
        }
        out.push((fd, b, i.data));
    }
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
    let timeout = (a[3] != 0).then(|| {
        // SAFETY: guest struct timespec.
        let t = unsafe { (a[3] as *const [i64; 2]).read_unaligned() };
        libc::timespec {
            tv_sec: t[0],
            tv_nsec: t[1],
        }
    });
    if timeout.is_some_and(|t| t.tv_sec < 0 || !(0..1_000_000_000).contains(&t.tv_nsec)) {
        return -(EINVAL as i64);
    }
    wait(a[0] as i32, a[1], a[2] as i32, timeout, a[4])
}

/// Fork child: Darwin dropped every kqueue. Recreate each epoll on its fd
/// numbers, then re-register the interests (after all kqueues exist, since
/// an epoll may watch another).
pub fn after_fork_child() {
    // One kqueue per epoll, duplicated onto every fd that refers to it.
    let mut made: Vec<(i32, Arc<Epoll>)> = Vec::new();
    for (fd, k) in fdtab::fds_where(|k| matches!(k, Kind::Epoll(_))) {
        let Kind::Epoll(ep) = k else { continue };
        let src = made.iter().find(|(_, e)| Arc::ptr_eq(e, &ep)).map(|m| m.0);
        // SAFETY: fd numbers the parent used for epolls; Darwin freed them.
        unsafe {
            let kq = match src {
                Some(s) => s,
                None => libc::kqueue(),
            };
            if kq < 0 {
                continue;
            }
            if kq != fd {
                libc::dup2(kq, fd);
                if src.is_none() {
                    libc::close(kq);
                }
            }
        }
        fdtab::set_flags(fd, false, ep.cloexec);
        if src.is_none() {
            made.push((fd, ep));
        }
    }
    for (fd, ep) in made {
        let mut interest = ep.interest.lock().unwrap_or_else(|e| e.into_inner());
        interest.retain(|&t, i| apply(fd, &changes(t, None, Some(i))) == 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fork child gets the epoll back on the same fd number, with its
    /// interest list, and sees the shared eventfd counter.
    #[test]
    fn epoll_and_eventfd_survive_fork() {
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
        // SAFETY: the child only makes syscalls and exits.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            let one = 1u64;
            super::super::event::write(efd, &one as *const u64 as u64, 8);
            let mut out = [0u64; 2];
            let n = epoll_pwait([ep as u64, out.as_mut_ptr() as u64, 1, 1000, 0, 0]);
            // SAFETY: leaving the forked test process.
            unsafe { libc::_exit(if n == 1 && out[1] == 0x55 { 0 } else { 1 }) };
        }
        let mut st = 0;
        // SAFETY: waiting for our child.
        assert_eq!(unsafe { libc::waitpid(pid, &mut st, 0) }, pid);
        assert!(
            libc::WIFEXITED(st) && libc::WEXITSTATUS(st) == 0,
            "status {st:#x}"
        );
        let mut v = 0u64;
        let r = super::super::event::read(efd, &mut v as *mut u64 as u64, 8);
        assert_eq!((r, v), (Some(8), 1), "the child's write reached the parent");
    }
}
