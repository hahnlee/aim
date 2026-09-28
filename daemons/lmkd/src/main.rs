//! `lmkd` of the derived image: the original's control protocol and kill
//! order ([`lmkd_core`]), with the Mac's memory pressure as its trigger.
//!
//! ActivityManager tells lmkd about every process it starts, reprioritizes
//! and removes, and waits for the socket while it holds its own lock: with
//! no lmkd, each of those waits for a connection that never comes, the lock
//! is held for seconds, and the watchdog ends system_server. The original
//! lmkd kills on PSI and memcg pressure, which Darwin does not report
//! (#222). The guest reports the Mac's whole RAM, so this one kills when
//! the Mac is under pressure: the host-call module `memory` wakes it on
//! each change of the Mac's level and reports the level and free memory.
//!
//! `lmkd --reinit` and `lmkd --boot_completed` (lmkd.rc's
//! `exec_background`) send the matching command to the running lmkd and
//! exit with its result.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

use aim_hostcall::guest;
use aim_hostcall::memory::level;
use lmkd_core::policy::{Level, Memory};
use lmkd_core::proto::{LMK_BOOT_COMPLETED, LMK_UPDATE_PROPS};
use lmkd_core::server::{Pressure, Server};

/// How often lmkd re-reads the Mac's memory while it is under pressure,
/// and so the most it kills: one process each time. macOS takes a moment
/// to lower its level after memory is freed.
const INTERVAL: Duration = Duration::from_secs(1);

fn main() {
    daemon_log::init("lowmemorykiller");
    let arg = std::env::args().nth(1);
    let cmd = match arg.as_deref() {
        None => return serve(),
        Some("--reinit") => LMK_UPDATE_PROPS,
        Some("--boot_completed") => LMK_BOOT_COMPLETED,
        Some(other) => {
            log::error!("unknown argument {other}");
            std::process::exit(2);
        }
    };
    match request(cmd) {
        Ok(0) => {}
        Ok(r) => {
            log::warn!("command {cmd} answered {r}");
            std::process::exit(1);
        }
        Err(e) => {
            log::error!("command {cmd}: {e}");
            std::process::exit(1);
        }
    }
}

/// The Mac's memory pressure, through the host-call module `memory`.
struct Host {
    watch: OwnedFd,
    page: u64,
}

impl Pressure for Host {
    fn fd(&self) -> RawFd {
        self.watch.as_raw_fd()
    }

    fn read(&mut self) -> Memory {
        let mut buf = [0u8; 64];
        // SAFETY: draining our non-blocking pipe into a local buffer.
        while unsafe { libc::read(self.watch.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0
        {
        }
        let m = guest::memory_read().unwrap_or_else(|e| {
            log::error!("reading the host's memory: {e:?}");
            Default::default()
        });
        Memory {
            level: match m.level {
                level::WARN => Level::Warn,
                level::CRITICAL => Level::Critical,
                _ => Level::Normal,
            },
            free: m.free / self.page,
            file: m.file / self.page,
        }
    }
}

/// Serve the listening socket init made from lmkd.rc (`socket lmkd
/// seqpacket`).
fn serve() {
    let Some(fd) = std::env::var("ANDROID_SOCKET_lmkd")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    else {
        log::error!("no lmkd socket from init");
        std::process::exit(1);
    };
    // init binds the socket; the daemon listens (lmkd's MAX_DATA_CONN).
    // SAFETY: listen on the socket init passed us.
    if unsafe { libc::listen(fd, 3) } != 0 {
        log::error!("listen: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    let watch = match guest::memory_watch() {
        // SAFETY: the host made the pipe for us.
        Ok(fd) => unsafe { OwnedFd::from_raw_fd(fd) },
        Err(e) => {
            log::error!("no memory pressure from the host (host-call module memory: {e:?})");
            std::process::exit(1);
        }
    };
    // SAFETY: plain sysconf.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    // SAFETY: the socket init passed us, ours from here on.
    let listener = unsafe { OwnedFd::from_raw_fd(fd) };
    let e = Server::new(listener, Host { watch, page }, INTERVAL).run();
    log::error!("poll: {e}");
    std::process::exit(1);
}

/// Send `cmd` to the running lmkd and return the result it answers.
fn request(cmd: i32) -> std::io::Result<i32> {
    let path = c"/dev/socket/lmkd";
    // SAFETY: plain socket calls on a socket we own.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sock = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (d, s) in addr.sun_path.iter_mut().zip(path.to_bytes()) {
            *d = *s as libc::c_char;
        }
        let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
        if libc::connect(
            sock.as_raw_fd(),
            (&addr as *const libc::sockaddr_un).cast(),
            len,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let out = cmd.to_be_bytes();
        if libc::send(sock.as_raw_fd(), out.as_ptr().cast(), 4, 0) != 4 {
            return Err(std::io::Error::last_os_error());
        }
        let mut reply = [0u8; 8];
        if libc::recv(sock.as_raw_fd(), reply.as_mut_ptr().cast(), 8, 0) != 8 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(i32::from_be_bytes(reply[4..].try_into().unwrap()))
    }
}
