//! The minimum of epoll, eventfd and timerfd that a `Looper` (servicemanager)
//! and libperfetto's task runner need, on host fds so the lean read/write
//! path keeps working:
//!
//! - an epoll fd is a kqueue; registrations are kqueue filters whose udata
//!   is the epoll data, level-triggered unless EPOLLET;
//! - an eventfd is a FIFO opened read-write: each 8-byte write is readable
//!   as one 8-byte read (a counter's sum is not accumulated);
//! - a timerfd is such a FIFO fed by a timer thread, which writes an
//!   expiration count of 1 while the FIFO is empty.
//!
//! Darwin does not pass kqueues to a forked child, so each epoll fd's
//! registrations are recorded and replayed there ([`after_fork_child`]). A
//! disabled `EVFILT_USER` filter tags each epoll kqueue, so a record is only
//! trusted while its fd still is that kqueue. (The child's epoll instance is
//! its own copy, where Linux would share one.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::errno::{self, EBADF, EINVAL, ENOENT};

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;

const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLONESHOT: u32 = 1 << 30;
const EPOLLET: u32 = 1 << 31;
const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLL_CTL_MOD: u64 = 3;

fn set_fd_flags(fd: i32, flags: u64) {
    // SAFETY: flag changes on our own new fd.
    unsafe {
        if flags & O_CLOEXEC != 0 {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        if flags & O_NONBLOCK != 0 {
            libc::fcntl(
                fd,
                libc::F_SETFL,
                libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK,
            );
        }
    }
}

/// A registration: ident, filter, kqueue flags, epoll data.
type Registration = (usize, i16, u16, u64);

struct Epoll {
    cloexec: bool,
    registrations: Vec<Registration>,
}

static EPOLLS: Mutex<Option<HashMap<i32, Epoll>>> = Mutex::new(None);
/// `EVFILT_USER` ident of the tag filter: this prefix | the epoll fd.
const EPOLL_TAG: usize = 0x6570_6f6c_0000_0000;

/// A kqueue tagged as the epoll fd `fd`.
fn new_epoll(fd: Option<i32>) -> i32 {
    // SAFETY: plain kqueue.
    let kq = unsafe { libc::kqueue() };
    if kq >= 0 {
        let mut t = kev(
            fd.unwrap_or(kq),
            libc::EVFILT_USER,
            libc::EV_ADD | libc::EV_DISABLE,
            0,
        );
        t.ident |= EPOLL_TAG;
        apply(kq, &t);
    }
    kq
}

fn is_epoll(kq: i32) -> bool {
    let mut probe = kev(kq, libc::EVFILT_USER, libc::EV_DISABLE, 0);
    probe.ident |= EPOLL_TAG;
    apply(kq, &probe) == 0
}

pub fn epoll_create1(a: [u64; 6]) -> i64 {
    if a[0] & !O_CLOEXEC != 0 {
        return -(EINVAL as i64);
    }
    let kq = new_epoll(None);
    if kq < 0 {
        return -(errno::last() as i64);
    }
    set_fd_flags(kq, a[0]);
    EPOLLS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(
            kq,
            Epoll {
                cloexec: a[0] & O_CLOEXEC != 0,
                registrations: Vec::new(),
            },
        );
    kq as i64
}

/// Record the outcome of an epoll_ctl on `kq`.
fn record(kq: i32, fd: i32, adds: &[(i16, u16, u64)]) {
    let mut t = EPOLLS.lock().unwrap();
    if let Some(e) = t.as_mut().and_then(|t| t.get_mut(&kq)) {
        e.registrations.retain(|r| r.0 != fd as usize);
        e.registrations
            .extend(adds.iter().map(|&(f, fl, d)| (fd as usize, f, fl, d)));
    }
}

/// Before fork: forget epoll fds the guest closed or replaced.
pub(super) fn prune_epolls() {
    if let Some(t) = EPOLLS.lock().unwrap().as_mut() {
        t.retain(|&kq, _| is_epoll(kq));
    }
}

/// In a forked child: recreate each epoll fd at its number with its
/// registrations.
pub(super) fn after_fork_child() {
    let mut g = EPOLLS.lock().unwrap();
    let Some(t) = g.as_mut() else { return };
    t.retain(|&fd, e| {
        // SAFETY: probing and filling the fd number the kqueue had.
        unsafe {
            if libc::fcntl(fd, libc::F_GETFD) >= 0 {
                return false;
            }
            let kq = new_epoll(Some(fd));
            if kq < 0 {
                return false;
            }
            for &(ident, filter, flags, data) in &e.registrations {
                apply(kq, &kev(ident as i32, filter, flags, data));
            }
            if kq != fd {
                let ok = libc::dup2(kq, fd) == fd;
                libc::close(kq);
                if !ok {
                    return false;
                }
            }
            if e.cloexec {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            true
        }
    });
}

fn kev(fd: i32, filter: i16, flags: u16, data: u64) -> libc::kevent {
    libc::kevent {
        ident: fd as usize,
        filter,
        flags,
        fflags: 0,
        data: 0,
        udata: data as *mut _,
    }
}

fn apply(kq: i32, ev: &libc::kevent) -> i32 {
    // SAFETY: one change, no events out.
    let r = unsafe { libc::kevent(kq, ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    if r < 0 { errno::last() } else { 0 }
}

pub fn epoll_ctl(a: [u64; 6]) -> i64 {
    let (kq, op, fd, event) = (a[0] as i32, a[1], a[2] as i32, a[3]);
    if kq == fd {
        return -(EINVAL as i64);
    }
    let (events, data) = if op == EPOLL_CTL_DEL {
        (0, 0)
    } else {
        // SAFETY: arm64 `struct epoll_event` is { u32 events; u64 data; }.
        unsafe {
            (
                (event as *const u32).read(),
                ((event + 8) as *const u64).read(),
            )
        }
    };
    let mut flags = libc::EV_ADD;
    if events & EPOLLET != 0 {
        flags |= libc::EV_CLEAR;
    }
    if events & EPOLLONESHOT != 0 {
        flags |= libc::EV_ONESHOT;
    }
    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if events & EPOLLIN != 0 {
                super::binder::poll(fd);
            }
            let mut adds = Vec::new();
            for (bit, filter) in [(EPOLLIN, libc::EVFILT_READ), (EPOLLOUT, libc::EVFILT_WRITE)] {
                let e = if events & bit != 0 {
                    adds.push((filter, flags, data));
                    apply(kq, &kev(fd, filter, flags, data))
                } else {
                    let e = apply(kq, &kev(fd, filter, libc::EV_DELETE, 0));
                    if e == libc::ENOENT { 0 } else { e }
                };
                if e != 0 {
                    return -(errno::from_darwin(e) as i64);
                }
            }
            record(kq, fd, &adds);
            0
        }
        EPOLL_CTL_DEL => {
            let r = apply(kq, &kev(fd, libc::EVFILT_READ, libc::EV_DELETE, 0));
            let w = apply(kq, &kev(fd, libc::EVFILT_WRITE, libc::EV_DELETE, 0));
            record(kq, fd, &[]);
            match (r, w) {
                (0, _) | (_, 0) => 0,
                (libc::EBADF, _) => -(EBADF as i64),
                _ => -(ENOENT as i64),
            }
        }
        _ => -(EINVAL as i64),
    }
}

pub fn epoll_pwait(a: [u64; 6]) -> i64 {
    let (kq, out, max, timeout) = (a[0] as i32, a[1], a[2] as i32, a[3] as i32);
    if max <= 0 {
        return -(EINVAL as i64);
    }
    let ts;
    let tsp = if timeout < 0 {
        std::ptr::null()
    } else {
        ts = libc::timespec {
            tv_sec: (timeout / 1000) as libc::time_t,
            tv_nsec: (timeout % 1000) as libc::c_long * 1_000_000,
        };
        &ts as *const libc::timespec
    };
    let n = max.min(256) as usize;
    // SAFETY: an event buffer on our side, then guest epoll_event slots.
    unsafe {
        let mut evs: Vec<libc::kevent> = vec![std::mem::zeroed(); n];
        let got = libc::kevent(kq, std::ptr::null(), 0, evs.as_mut_ptr(), n as i32, tsp);
        if got < 0 {
            return -(errno::last() as i64);
        }
        // One epoll event per fd: merge its read and write filters.
        let mut merged: Vec<(usize, u32, u64)> = Vec::new();
        for ev in &evs[..got as usize] {
            let mut bits = match ev.filter {
                libc::EVFILT_READ => EPOLLIN,
                libc::EVFILT_WRITE => EPOLLOUT,
                _ => 0,
            };
            if ev.flags & libc::EV_EOF != 0 {
                bits |= EPOLLHUP;
            }
            if ev.flags & libc::EV_ERROR != 0 {
                bits = EPOLLERR;
            }
            match merged.iter_mut().find(|m| m.0 == ev.ident) {
                Some(m) => m.1 |= bits,
                None => merged.push((ev.ident, bits, ev.udata as u64)),
            }
        }
        for (i, (_, bits, data)) in merged.iter().enumerate() {
            let slot = out + i as u64 * 16;
            (slot as *mut u32).write(*bits);
            ((slot + 8) as *mut u64).write(*data);
        }
        merged.len() as i64
    }
}

static FIFO_SEQ: AtomicU64 = AtomicU64::new(0);

/// A FIFO opened read-write, with no name left in the file system.
fn anonymous_fifo() -> Result<i32, i64> {
    let path = std::env::temp_dir().join(format!(
        "linux-abi-fifo-{}-{}",
        std::process::id(),
        FIFO_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: creating, opening and unlinking our own FIFO.
    unsafe {
        if libc::mkfifo(c.as_ptr(), 0o600) < 0 {
            return Err(-(errno::last() as i64));
        }
        let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK);
        let e = errno::last();
        libc::unlink(c.as_ptr());
        if fd < 0 {
            return Err(-(e as i64));
        }
        // Blocking unless the guest asked otherwise.
        libc::fcntl(
            fd,
            libc::F_SETFL,
            libc::fcntl(fd, libc::F_GETFL) & !libc::O_NONBLOCK,
        );
        Ok(fd)
    }
}

pub fn eventfd2(a: [u64; 6]) -> i64 {
    let (initval, flags) = (a[0], a[1]);
    let fd = match anonymous_fifo() {
        Ok(fd) => fd,
        Err(e) => return e,
    };
    if initval != 0 {
        // SAFETY: writing the initial count into our FIFO.
        unsafe { libc::write(fd, initval.to_le_bytes().as_ptr().cast(), 8) };
    }
    set_fd_flags(fd, flags);
    fd as i64
}

struct Timer {
    /// Our own reference to the FIFO.
    fd: i32,
    state: Mutex<(Option<Instant>, Duration)>,
    wake: Condvar,
}

/// Timers by the inode of their FIFO.
static TIMERS: Mutex<Option<HashMap<u64, Arc<Timer>>>> = Mutex::new(None);

fn fifo_inode(fd: i32) -> Option<u64> {
    // SAFETY: fstat into a local buffer.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st.st_ino)
}

fn timer_of(fd: i32) -> Option<Arc<Timer>> {
    TIMERS
        .lock()
        .unwrap()
        .as_ref()?
        .get(&fifo_inode(fd)?)
        .cloned()
}

impl Timer {
    fn run(self: Arc<Self>) {
        let mut st = self.state.lock().unwrap();
        loop {
            match st.0 {
                None => st = self.wake.wait(st).unwrap(),
                Some(due) => {
                    let now = Instant::now();
                    if now < due {
                        st = self.wake.wait_timeout(st, due - now).unwrap().0;
                        continue;
                    }
                    let mut queued = 0i32;
                    // SAFETY: querying and writing our own FIFO.
                    unsafe {
                        libc::ioctl(self.fd, libc::FIONREAD, &mut queued);
                        if queued == 0 {
                            libc::write(self.fd, 1u64.to_le_bytes().as_ptr().cast(), 8);
                        }
                    }
                    st.0 = (!st.1.is_zero()).then(|| due + st.1);
                }
            }
        }
    }
}

pub fn timerfd_create(a: [u64; 6]) -> i64 {
    let flags = a[1];
    let fd = match anonymous_fifo() {
        Ok(fd) => fd,
        Err(e) => return e,
    };
    set_fd_flags(fd, flags);
    // SAFETY: our own reference, kept by the timer thread.
    let own = unsafe { libc::dup(fd) };
    let Some(ino) = fifo_inode(fd) else {
        return -(EINVAL as i64);
    };
    let timer = Arc::new(Timer {
        fd: own,
        state: Mutex::new((None, Duration::ZERO)),
        wake: Condvar::new(),
    });
    TIMERS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(ino, timer.clone());
    std::thread::spawn(move || timer.run());
    fd as i64
}

fn read_timespec(p: u64) -> Duration {
    // SAFETY: a guest struct timespec { i64 sec; i64 nsec; }.
    let [s, ns] = unsafe { (p as *const [i64; 2]).read_unaligned() };
    Duration::new(s.max(0) as u64, ns.clamp(0, 999_999_999) as u32)
}

fn write_itimerspec(p: u64, st: &(Option<Instant>, Duration)) {
    if p == 0 {
        return;
    }
    let left = st.0.map_or(Duration::ZERO, |due| {
        due.saturating_duration_since(Instant::now())
            .max(Duration::from_nanos(1))
    });
    let v = [
        st.1.as_secs() as i64,
        st.1.subsec_nanos() as i64,
        left.as_secs() as i64,
        left.subsec_nanos() as i64,
    ];
    // SAFETY: a guest struct itimerspec.
    unsafe { (p as *mut [i64; 4]).write_unaligned(v) };
}

const TFD_TIMER_ABSTIME: u64 = 1;

pub fn timerfd_settime(a: [u64; 6]) -> i64 {
    let (fd, flags, new, old) = (a[0] as i32, a[1], a[2], a[3]);
    let Some(t) = timer_of(fd) else {
        return -(EINVAL as i64);
    };
    let interval = read_timespec(new);
    let value = read_timespec(new + 16);
    let mut st = t.state.lock().unwrap();
    write_itimerspec(old, &st);
    let due = if value.is_zero() {
        None
    } else if flags & TFD_TIMER_ABSTIME != 0 {
        // CLOCK_MONOTONIC and Instant share the host's monotonic clock.
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: local timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        let now = Duration::new(now.tv_sec as u64, now.tv_nsec as u32);
        Some(Instant::now() + value.saturating_sub(now))
    } else {
        Some(Instant::now() + value)
    };
    *st = (due, interval);
    t.wake.notify_all();
    0
}

pub fn timerfd_gettime(a: [u64; 6]) -> i64 {
    let Some(t) = timer_of(a[0] as i32) else {
        return -(EINVAL as i64);
    };
    write_itimerspec(a[1], &t.state.lock().unwrap());
    0
}

/// Linux poll bits that differ from Darwin's: POLLWRNORM and POLLWRBAND.
const LINUX_POLLWRNORM: i16 = 0x100;
const LINUX_POLLWRBAND: i16 = 0x200;

fn poll_to_host(e: i16) -> i16 {
    let mut h = e & 0xff;
    if e & LINUX_POLLWRNORM != 0 {
        h |= libc::POLLWRNORM;
    }
    if e & LINUX_POLLWRBAND != 0 {
        h |= libc::POLLWRBAND;
    }
    h
}

fn poll_from_host(h: i16) -> i16 {
    let mut e = h & 0xff;
    if h & libc::POLLWRBAND != 0 {
        e |= LINUX_POLLWRBAND;
    }
    e
}

pub fn ppoll(a: [u64; 6]) -> i64 {
    let (fds, nfds, tsp) = (a[0] as *mut libc::pollfd, a[1] as usize, a[2]);
    let timeout = if tsp == 0 {
        -1
    } else {
        let d = read_timespec(tsp);
        d.as_millis().min(i32::MAX as u128) as i32
    };
    // SAFETY: the guest's pollfd array (same layout on both kernels).
    unsafe {
        let guest = std::slice::from_raw_parts_mut(fds, nfds);
        let mut host: Vec<libc::pollfd> = guest
            .iter()
            .map(|p| {
                if p.events & libc::POLLIN != 0 {
                    super::binder::poll(p.fd);
                }
                libc::pollfd {
                    fd: p.fd,
                    events: poll_to_host(p.events),
                    revents: 0,
                }
            })
            .collect();
        let r = libc::poll(host.as_mut_ptr(), nfds as libc::nfds_t, timeout);
        if r < 0 {
            return -(errno::last() as i64);
        }
        for (g, h) in guest.iter_mut().zip(&host) {
            g.revents = poll_from_host(h.revents);
        }
        r as i64
    }
}
