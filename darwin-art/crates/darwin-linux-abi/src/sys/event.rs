//! eventfd and timerfd.
//!
//! Both are the guest end of a Darwin datagram socketpair, so they poll,
//! dup, fork and close like any fd, and kqueue sees them readable exactly
//! when a read would not block.
//!
//! - **eventfd:** the counter lives in the socket as 8-byte records sent
//!   from the hidden peer end; a read drains and sums them (or takes one
//!   unit with EFD_SEMAPHORE). The count therefore survives fork and is
//!   shared by every process holding the fd.
//! - **timerfd:** a host thread sends a token record when a timer expires;
//!   a read computes the expirations from the timer's settings and drains
//!   the token. Timer settings are per process.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use super::fdtab::{self, Kind};
use crate::errno::{self, EAGAIN, EBADF, EINVAL};

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;
const EFD_SEMAPHORE: u64 = 1;

/// A socketpair: (guest end, hidden peer end).
fn pair(flags: u64) -> Result<(i32, i32), i64> {
    let mut sv = [0i32; 2];
    // SAFETY: socketpair into a local array.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, sv.as_mut_ptr()) } < 0 {
        return Err(-(errno::last() as i64));
    }
    fdtab::set_flags(sv[0], flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0);
    fdtab::set_flags(sv[1], true, true);
    let peer = fdtab::hide(sv[1]);
    let one: i32 = 1;
    // SAFETY: setting an int option on our socket.
    unsafe {
        libc::setsockopt(
            peer,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&one as *const i32).cast(),
            4,
        )
    };
    Ok((sv[0], peer))
}

fn send_record(peer: i32, v: u64) -> bool {
    // SAFETY: sending 8 bytes from a local.
    unsafe { libc::send(peer, (&v as *const u64).cast(), 8, libc::MSG_DONTWAIT) == 8 }
}

fn recv_record(fd: i32) -> Option<u64> {
    let mut v = 0u64;
    // SAFETY: receiving into a local.
    let n = unsafe { libc::recv(fd, (&mut v as *mut u64).cast(), 8, libc::MSG_DONTWAIT) };
    (n == 8).then_some(v)
}

/// Receive and sum every pending record.
fn drain(fd: i32) -> u64 {
    let mut sum = 0u64;
    while let Some(v) = recv_record(fd) {
        sum = sum.saturating_add(v);
    }
    sum
}

pub struct EventFd {
    peer: AtomicI32,
    semaphore: bool,
    lock: Mutex<()>,
}

impl Drop for EventFd {
    fn drop(&mut self) {
        close_peer(&self.peer);
    }
}

fn close_peer(peer: &AtomicI32) {
    let fd = peer.load(Ordering::Relaxed);
    fdtab::unhide(fd);
    // SAFETY: the hidden peer end is ours.
    unsafe { libc::close(fd) };
}

impl EventFd {
    fn add(&self, fd: i32, v: u64) {
        let peer = self.peer.load(Ordering::Relaxed);
        if send_record(peer, v) {
            return;
        }
        // The socket buffer is full of records: fold them into one.
        let sum = drain(fd).saturating_add(v).min(u64::MAX - 1);
        send_record(peer, sum);
    }

    fn read(&self, fd: i32, buf: u64, len: usize) -> i64 {
        if len < 8 {
            return -(EINVAL as i64);
        }
        loop {
            {
                let _g = self.lock.lock().unwrap();
                let v = if self.semaphore {
                    recv_record(fd).map(|v| {
                        if v > 1 {
                            self.add(fd, v - 1);
                        }
                        1
                    })
                } else {
                    Some(drain(fd)).filter(|&s| s > 0)
                };
                if let Some(v) = v {
                    // SAFETY: guest buffer of at least 8 bytes.
                    unsafe { (buf as *mut u64).write_unaligned(v) };
                    return 8;
                }
            }
            if fdtab::nonblocking(fd) {
                return -(EAGAIN as i64);
            }
            let r = fdtab::wait_for(fd, libc::POLLIN);
            if r < 0 {
                return r;
            }
        }
    }

    fn write(&self, fd: i32, buf: u64, len: usize) -> i64 {
        if len < 8 {
            return -(EINVAL as i64);
        }
        // SAFETY: guest buffer of at least 8 bytes.
        let v = unsafe { (buf as *const u64).read_unaligned() };
        if v == u64::MAX {
            return -(EINVAL as i64);
        }
        if v > 0 {
            let _g = self.lock.lock().unwrap();
            self.add(fd, v);
        }
        8
    }
}

pub fn eventfd2(a: [u64; 6]) -> i64 {
    let (init, flags) = (a[0] as u32 as u64, a[1]);
    if flags & !(EFD_SEMAPHORE | O_NONBLOCK | O_CLOEXEC) != 0 {
        return -(EINVAL as i64);
    }
    let (fd, peer) = match pair(flags) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if init > 0 {
        send_record(peer, init);
    }
    fdtab::insert(
        fd,
        Kind::Event(Arc::new(EventFd {
            peer: AtomicI32::new(peer),
            semaphore: flags & EFD_SEMAPHORE != 0,
            lock: Mutex::new(()),
        })),
    );
    fd as i64
}

// ---- timerfd ----------------------------------------------------------------

const CLOCK_REALTIME: u64 = 0;
const CLOCK_MONOTONIC: u64 = 1;
const CLOCK_BOOTTIME: u64 = 7;
const CLOCK_REALTIME_ALARM: u64 = 8;
const CLOCK_BOOTTIME_ALARM: u64 = 9;
const TFD_TIMER_ABSTIME: u64 = 1;
const TFD_TIMER_CANCEL_ON_SET: u64 = 2;

fn now_ns(clock: libc::clockid_t) -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: local timespec.
    unsafe { libc::clock_gettime(clock, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn mono() -> u64 {
    now_ns(libc::CLOCK_MONOTONIC)
}

#[derive(Default)]
struct TimerState {
    /// Monotonic deadline of the next expiration; 0 when disarmed.
    next: u64,
    interval: u64,
    /// A token record is in the socket.
    token: bool,
}

pub struct TimerFd {
    peer: AtomicI32,
    realtime: bool,
    state: Mutex<TimerState>,
}

impl Drop for TimerFd {
    fn drop(&mut self) {
        close_peer(&self.peer);
    }
}

struct Timers {
    list: Mutex<(bool, Vec<Weak<TimerFd>>)>,
    wake: Condvar,
}

static TIMERS: Timers = Timers {
    list: Mutex::new((false, Vec::new())),
    wake: Condvar::new(),
};

/// The timer thread: sends a token for each timer that expired and has none.
fn timer_thread() {
    let mut g = TIMERS.list.lock().unwrap();
    loop {
        g.1.retain(|w| w.strong_count() > 0);
        let now = mono();
        let mut earliest = u64::MAX;
        for t in g.1.iter().filter_map(Weak::upgrade) {
            let mut s = t.state.lock().unwrap();
            if s.next == 0 || s.token {
                continue;
            }
            if s.next <= now {
                s.token = send_record(t.peer.load(Ordering::Relaxed), 1);
            } else {
                earliest = earliest.min(s.next);
            }
        }
        g = if earliest == u64::MAX {
            TIMERS.wake.wait(g).unwrap()
        } else {
            TIMERS
                .wake
                .wait_timeout(g, Duration::from_nanos(earliest - now))
                .unwrap()
                .0
        };
    }
}

fn register(t: &Arc<TimerFd>) {
    let mut g = TIMERS.list.lock().unwrap();
    g.1.push(Arc::downgrade(t));
    if !g.0 {
        g.0 = true;
        std::thread::spawn(timer_thread);
    }
}

/// Wake the timer thread to recompute its deadline. Taking the lock first
/// means the thread is either waiting or will see the new state.
fn poke() {
    drop(TIMERS.list.lock().unwrap());
    TIMERS.wake.notify_all();
}

/// Fork: an eventfd is its hidden peer (inherited) and its mode.
pub(super) fn save_event(e: &EventFd, w: &mut super::fork_state::Writer) {
    w.i32(e.peer.load(Ordering::Relaxed));
    w.bool(e.semaphore);
}

pub(super) fn load_event(r: &mut super::fork_state::Reader) -> Arc<EventFd> {
    let peer = r.i32();
    Arc::new(EventFd {
        peer: AtomicI32::new(peer),
        semaphore: r.bool(),
        lock: Mutex::new(()),
    })
}

/// Fork: a timerfd is its hidden peer and its settings (Linux children
/// share the timer with the parent; here each process has a copy).
pub(super) fn save_timer(t: &TimerFd, w: &mut super::fork_state::Writer) {
    w.i32(t.peer.load(Ordering::Relaxed));
    w.bool(t.realtime);
    let s = t.state.lock().unwrap();
    w.u64(s.next);
    w.u64(s.interval);
    w.bool(s.token);
}

pub(super) fn load_timer(r: &mut super::fork_state::Reader) -> Arc<TimerFd> {
    let peer = r.i32();
    let realtime = r.bool();
    let state = TimerState {
        next: r.u64(),
        interval: r.u64(),
        token: r.bool(),
    };
    let t = Arc::new(TimerFd {
        peer: AtomicI32::new(peer),
        realtime,
        state: Mutex::new(state),
    });
    TIMERS.list.lock().unwrap().1.push(Arc::downgrade(&t));
    t
}

/// Fork child: the timer thread did not survive.
pub fn after_fork_child() {
    let mut g = TIMERS.list.lock().unwrap_or_else(|e| e.into_inner());
    g.0 = !g.1.is_empty();
    if g.0 {
        std::thread::spawn(timer_thread);
    }
}

impl TimerFd {
    /// Expirations since the last read, advancing the deadline.
    fn take_expirations(s: &mut TimerState, now: u64) -> u64 {
        if s.next == 0 || now < s.next {
            return 0;
        }
        if s.interval == 0 {
            s.next = 0;
            return 1;
        }
        let n = 1 + (now - s.next) / s.interval;
        s.next += n * s.interval;
        n
    }

    fn read(&self, fd: i32, buf: u64, len: usize) -> i64 {
        if len < 8 {
            return -(EINVAL as i64);
        }
        loop {
            {
                let mut s = self.state.lock().unwrap();
                let n = Self::take_expirations(&mut s, mono());
                if n > 0 {
                    drain(fd);
                    s.token = false;
                    drop(s);
                    poke();
                    // SAFETY: guest buffer of at least 8 bytes.
                    unsafe { (buf as *mut u64).write_unaligned(n) };
                    return 8;
                }
            }
            if fdtab::nonblocking(fd) {
                return -(EAGAIN as i64);
            }
            let r = fdtab::wait_for(fd, libc::POLLIN);
            if r < 0 {
                return r;
            }
        }
    }
}

pub fn timerfd_create(a: [u64; 6]) -> i64 {
    let (clock, flags) = (a[0], a[1]);
    let realtime = match clock {
        CLOCK_REALTIME | CLOCK_REALTIME_ALARM => true,
        CLOCK_MONOTONIC | CLOCK_BOOTTIME | CLOCK_BOOTTIME_ALARM => false,
        _ => return -(EINVAL as i64),
    };
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return -(EINVAL as i64);
    }
    let (fd, peer) = match pair(flags) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let t = Arc::new(TimerFd {
        peer: AtomicI32::new(peer),
        realtime,
        state: Mutex::new(TimerState::default()),
    });
    register(&t);
    fdtab::insert(fd, Kind::Timer(t));
    fd as i64
}

fn timer(fd: i32) -> Result<Arc<TimerFd>, i64> {
    match fdtab::get(fd) {
        Some(Kind::Timer(t)) => Ok(t),
        // SAFETY: plain fcntl to tell EBADF from EINVAL.
        _ if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 => Err(-(EBADF as i64)),
        _ => Err(-(EINVAL as i64)),
    }
}

fn ns_of(ts: [i64; 2]) -> Option<u64> {
    (ts[0] >= 0 && (0..1_000_000_000).contains(&ts[1]))
        .then(|| ts[0] as u64 * 1_000_000_000 + ts[1] as u64)
}

fn ts_of(ns: u64) -> [i64; 2] {
    [(ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64]
}

/// `struct itimerspec` { it_interval, it_value } of the timer now.
fn current(s: &TimerState, now: u64) -> [i64; 4] {
    let left = if s.next == 0 {
        0
    } else {
        s.next.saturating_sub(now).max(1)
    };
    let i = ts_of(s.interval);
    let v = ts_of(left);
    [i[0], i[1], v[0], v[1]]
}

pub fn timerfd_settime(a: [u64; 6]) -> i64 {
    let (fd, flags, new, old) = (a[0] as i32, a[1], a[2], a[3]);
    let t = match timer(fd) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if flags & !(TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET) != 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest struct itimerspec.
    let v = unsafe { (new as *const [i64; 4]).read_unaligned() };
    let (Some(interval), Some(value)) = (ns_of([v[0], v[1]]), ns_of([v[2], v[3]])) else {
        return -(EINVAL as i64);
    };
    let now = mono();
    let mut s = t.state.lock().unwrap();
    if old != 0 {
        // SAFETY: guest struct itimerspec.
        unsafe { (old as *mut [i64; 4]).write_unaligned(current(&s, now)) };
    }
    s.interval = interval;
    s.next = if value == 0 {
        0
    } else if flags & TFD_TIMER_ABSTIME == 0 {
        now + value
    } else if t.realtime {
        // Absolute wall-clock time, converted once to the monotonic clock.
        (now + value)
            .saturating_sub(now_ns(libc::CLOCK_REALTIME))
            .max(1)
    } else {
        value.max(1)
    };
    drain(fd);
    s.token = false;
    drop(s);
    poke();
    0
}

pub fn timerfd_gettime(a: [u64; 6]) -> i64 {
    let t = match timer(a[0] as i32) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let s = t.state.lock().unwrap();
    // SAFETY: guest struct itimerspec.
    unsafe { (a[1] as *mut [i64; 4]).write_unaligned(current(&s, mono())) };
    0
}

/// read(2) on an eventfd or timerfd; None when `fd` is neither.
pub fn read(fd: i32, buf: u64, len: usize) -> Option<i64> {
    match fdtab::get(fd)? {
        Kind::Event(e) => Some(e.read(fd, buf, len)),
        Kind::Timer(t) => Some(t.read(fd, buf, len)),
        _ => None,
    }
}

/// write(2) on an eventfd (a timerfd is not writable).
pub fn write(fd: i32, buf: u64, len: usize) -> Option<i64> {
    match fdtab::get(fd)? {
        Kind::Event(e) => Some(e.write(fd, buf, len)),
        Kind::Timer(_) => Some(-(EINVAL as i64)),
        _ => None,
    }
}

/// The guest is about to dup2 onto `fd`, one of our hidden peer ends: move
/// it elsewhere first.
pub fn relocate_hidden(fd: i32) {
    for (_, k) in fdtab::fds_where(|k| matches!(k, Kind::Event(_) | Kind::Timer(_))) {
        let peer = match &k {
            Kind::Event(e) => &e.peer,
            Kind::Timer(t) => &t.peer,
            _ => continue,
        };
        if peer.load(Ordering::Relaxed) == fd {
            fdtab::unhide(fd);
            peer.store(fdtab::hide(fd), Ordering::Relaxed);
            return;
        }
    }
}
