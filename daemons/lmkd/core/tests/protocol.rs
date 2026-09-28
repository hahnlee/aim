//! lmkd end to end on the host: ActivityManager's side of the socket, a
//! simulated pressure source, and real processes that get killed.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lmkd_core::policy::{Level, Memory};
use lmkd_core::proto::*;
use lmkd_core::server::{Pressure, Server};

/// The host's memory as the test sets it, with a pipe to announce changes.
struct Simulated {
    now: Arc<Mutex<Memory>>,
    wake: OwnedFd,
}

impl Pressure for Simulated {
    fn fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }

    fn read(&mut self) -> Memory {
        let mut buf = [0u8; 64];
        // SAFETY: a non-blocking read into a local buffer.
        unsafe { libc::read(self.wake.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        *self.now.lock().unwrap()
    }
}

struct Host {
    now: Arc<Mutex<Memory>>,
    wake: OwnedFd,
    /// The peer of lmkd's listening socket, kept open so that it stays
    /// quiet.
    _listener_peer: OwnedFd,
}

impl Host {
    fn set(&self, level: Level) {
        self.now.lock().unwrap().level = level;
        // SAFETY: one byte into our pipe.
        unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
    }
}

/// lmkd with a simulated host at normal pressure, and a connection to it.
/// macOS has no `SOCK_SEQPACKET` Unix sockets; a datagram socket pair keeps
/// the packet boundaries the protocol relies on.
fn start() -> (Host, OwnedFd) {
    let pair = |ty| {
        let mut fds = [0; 2];
        // SAFETY: a socket pair into a local array, owned at once.
        unsafe {
            assert_eq!(libc::socketpair(libc::AF_UNIX, ty, 0, fds.as_mut_ptr()), 0);
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        }
    };
    // Never connected to: the test's connection is adopted.
    let (listener, listener_peer) = pair(libc::SOCK_STREAM);
    let (lmkd, client) = pair(libc::SOCK_DGRAM);
    let (r, w) = pair(libc::SOCK_STREAM);
    // SAFETY: flags on our socket.
    unsafe { libc::fcntl(r.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    let now = Arc::new(Mutex::new(Memory {
        level: Level::Normal,
        free: 1 << 20,
        file: 1 << 20,
    }));
    let pressure = Simulated {
        now: now.clone(),
        wake: r,
    };
    let mut server = Server::new(listener, pressure, Duration::from_millis(50));
    server.adopt(lmkd);
    std::thread::spawn(move || server.run());
    // A missing packet fails the test instead of hanging it.
    let tv = libc::timeval {
        tv_sec: 5,
        tv_usec: 0,
    };
    // SAFETY: a socket option from a local.
    unsafe {
        libc::setsockopt(
            client.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const tv).cast(),
            size_of::<libc::timeval>() as _,
        )
    };
    let host = Host {
        now,
        wake: w,
        _listener_peer: listener_peer,
    };
    (host, client)
}

fn send(fd: &OwnedFd, words: &[i32]) {
    let b = encode(words);
    // SAFETY: a send from a local buffer.
    assert_eq!(
        unsafe { libc::send(fd.as_raw_fd(), b.as_ptr().cast(), b.len(), 0) },
        b.len() as isize
    );
}

fn recv(fd: &OwnedFd) -> Vec<i32> {
    let mut buf = [0u8; 256];
    // SAFETY: a receive into a local buffer.
    let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
    assert!(
        n > 0,
        "no packet from lmkd: {}",
        std::io::Error::last_os_error()
    );
    decode(&buf[..n as usize])
}

fn request(fd: &OwnedFd, words: &[i32]) -> Vec<i32> {
    send(fd, words);
    recv(fd)
}

/// ActivityManager's connect sequence (ProcessList.onLmkdConnect).
fn connect_as_activity_manager(fd: &OwnedFd) {
    send(fd, &[LMK_PROCPURGE]);
    send(
        fd,
        &[
            LMK_TARGET, 4608, 0, 5760, 100, 6912, 200, 8064, 250, 13824, 900, 20160, 950,
        ],
    );
    send(fd, &[LMK_SUBSCRIBE, LMK_ASYNC_EVENT_KILL]);
    send(fd, &[LMK_SUBSCRIBE, LMK_ASYNC_EVENT_STAT]);
}

struct App(Child);

impl App {
    fn start() -> App {
        App(Command::new("sleep").arg("60").spawn().unwrap())
    }

    fn pid(&self) -> i32 {
        self.0.id() as i32
    }

    fn alive(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }

    fn was_killed(&mut self) -> bool {
        self.0.wait().unwrap().signal() == Some(libc::SIGKILL)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The next kill report: (pid, uid).
fn next_kill(fd: &OwnedFd) -> (i32, u32) {
    let p = recv(fd);
    assert_eq!((p[0], p.len()), (LMK_PROCKILL, 4), "{p:?}");
    (p[1], p[2] as u32)
}

fn register(fd: &OwnedFd, app: &App, uid: u32, adj: i32) {
    send(
        fd,
        &[LMK_PROCPRIO, app.pid(), uid as i32, adj, PROC_TYPE_APP],
    );
}

#[test]
fn pressure_kills_in_oom_score_adj_order() {
    let (host, am) = start();
    connect_as_activity_manager(&am);
    let mut apps: Vec<App> = (0..7).map(|_| App::start()).collect();
    // (uid, oom_score_adj): cached, then perceptible, visible and
    // foreground. 3 is registered after 2 at the same level.
    let prio = [
        (10000, 999),
        (10001, 950),
        (10002, 900),
        (10003, 900),
        (10004, 200),
        (10005, 100),
        (10006, 0),
    ];
    for (app, (uid, adj)) in apps.iter().zip(prio) {
        register(&am, app, uid, adj);
    }
    // Two of them in one batch, as ActivityManager sends updates.
    send(
        &am,
        &[
            LMK_PROCS_PRIO,
            apps[2].pid(),
            10002,
            900,
            PROC_TYPE_APP,
            apps[3].pid(),
            10003,
            900,
            PROC_TYPE_APP,
        ],
    );
    assert_eq!(
        request(&am, &[LMK_GETKILLCNT, 0, 1000]),
        [LMK_GETKILLCNT, 0]
    );

    // Warn: the cached processes, highest oom_score_adj first, and the
    // least recently prioritized first within one level.
    host.set(Level::Warn);
    for want in [0, 1, 2, 3] {
        assert_eq!(next_kill(&am), (apps[want].pid(), 10000 + want as u32));
        assert!(apps[want].was_killed());
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        apps[4..].iter_mut().all(App::alive),
        "warn kills only cached processes"
    );
    assert_eq!(
        request(&am, &[LMK_GETKILLCNT, 900, 1000]),
        [LMK_GETKILLCNT, 4]
    );

    // Critical: down to perceptible, not below.
    host.set(Level::Critical);
    assert_eq!(next_kill(&am), (apps[4].pid(), 10004));
    assert!(apps[4].was_killed());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        apps[5].alive() && apps[6].alive(),
        "visible and foreground stay"
    );

    // Back to normal: nothing more dies, even when cached.
    host.set(Level::Normal);
    let mut late = App::start();
    let mut removed = App::start();
    register(&am, &late, 10007, 950);
    register(&am, &removed, 10008, 999);
    std::thread::sleep(Duration::from_millis(300));
    assert!(late.alive() && removed.alive());

    // A removed process is not lmkd's to kill.
    send(&am, &[LMK_PROCREMOVE, removed.pid()]);
    host.set(Level::Warn);
    assert_eq!(next_kill(&am), (late.pid(), 10007));
    assert!(late.was_killed());
    std::thread::sleep(Duration::from_millis(300));
    assert!(removed.alive());

    // Minfree: with free and file memory under the foreground level, even
    // warn goes all the way down.
    {
        let mut now = host.now.lock().unwrap();
        now.free = 100;
        now.file = 100;
    }
    host.set(Level::Warn);
    assert_eq!(next_kill(&am), (apps[5].pid(), 10005));
    assert_eq!(next_kill(&am), (apps[6].pid(), 10006));

    assert_eq!(
        request(&am, &[LMK_GETKILLCNT, 0, 1000]),
        [LMK_GETKILLCNT, 8]
    );
    assert_eq!(
        request(&am, &[LMK_GETKILLCNT, 1001, 1001]),
        [LMK_GETKILLCNT, 8]
    );
}

#[test]
fn answers_what_lmkd_answers() {
    let (_host, am) = start();
    assert_eq!(request(&am, &[LMK_UPDATE_PROPS]), [LMK_UPDATE_PROPS, 0]);
    assert_eq!(request(&am, &[LMK_BOOT_COMPLETED]), [LMK_BOOT_COMPLETED, 0]);
    assert_eq!(request(&am, &[LMK_BOOT_COMPLETED]), [LMK_BOOT_COMPLETED, 1]);
    // Malformed packets are dropped and the connection keeps working.
    send(&am, &[LMK_PROCPRIO, 1]);
    send(&am, &[LMK_TARGET, 1, 2, 3]);
    send(&am, &[LMK_PROCPRIO, std::process::id() as i32, 0, 2000, 0]);
    assert_eq!(
        request(&am, &[LMK_GETKILLCNT, 0, 1000]),
        [LMK_GETKILLCNT, 0]
    );
}
