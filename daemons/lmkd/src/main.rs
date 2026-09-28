//! `lmkd` of the derived image: the lmkd control socket, answered as the
//! original answers it, with no kills.
//!
//! ActivityManager tells lmkd about every process it starts, reprioritizes
//! and removes, and waits for the socket while it holds its own lock: with
//! no lmkd, each of those waits for a connection that never comes, the lock
//! is held for seconds, and the watchdog ends system_server. The original
//! lmkd kills by PSI and memcg pressure, which Darwin does not report
//! (#222); until kills follow macOS memory pressure, this one keeps the
//! protocol and kills nothing.
//!
//! The protocol is `lmkd.h`'s: packets of big-endian 32-bit words, the
//! command first. Only `LMK_GETKILLCNT`, `LMK_UPDATE_PROPS` and
//! `LMK_BOOT_COMPLETED` are answered; the rest are one-way. `lmkd --reinit`
//! and `lmkd --boot_completed` (lmkd.rc's `exec_background`) send the
//! matching command to the running lmkd and exit with its result.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

const LMK_TARGET: i32 = 0;
const LMK_PROCPRIO: i32 = 1;
const LMK_PROCREMOVE: i32 = 2;
const LMK_PROCPURGE: i32 = 3;
const LMK_GETKILLCNT: i32 = 4;
const LMK_SUBSCRIBE: i32 = 5;
const LMK_UPDATE_PROPS: i32 = 7;
const LMK_START_MONITORING: i32 = 9;
const LMK_BOOT_COMPLETED: i32 = 10;
const LMK_PROCS_PRIO: i32 = 11;

/// `boot_completed_notification_result`.
const BOOT_COMPLETED_SUCCESS: i32 = 0;
const BOOT_COMPLETED_ALREADY_HANDLED: i32 = 2;

/// `LMKD_CTRL_PACKET_SIZE`: the largest packet (LMK_TARGET with 6 pairs,
/// LMK_PROCS_PRIO with 3 words for each of up to 32 processes).
const PACKET: usize = 4 * (32 * 3 + 2);

static BOOT_COMPLETED: AtomicBool = AtomicBool::new(false);

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

/// The listening socket init made from lmkd.rc (`socket lmkd seqpacket`).
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
    log::info!("serving the lmkd socket; processes are not killed (no memory pressure source)");
    loop {
        // SAFETY: accept on the listening socket init passed us.
        let conn = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if conn < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            log::error!("accept: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }
        // SAFETY: a fresh connection, owned from here on.
        let conn = unsafe { OwnedFd::from_raw_fd(conn) };
        std::thread::spawn(move || client(conn));
    }
}

/// Answer one client until it hangs up.
fn client(conn: OwnedFd) {
    let mut buf = [0u8; PACKET];
    loop {
        // SAFETY: a receive into our buffer.
        let n = unsafe { libc::recv(conn.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if n < 4 {
            return;
        }
        let words: Vec<i32> = buf[..n as usize]
            .chunks_exact(4)
            .map(|w| i32::from_be_bytes(w.try_into().unwrap()))
            .collect();
        if let Some(reply) = handle(&words) {
            let bytes: Vec<u8> = reply.iter().flat_map(|w| w.to_be_bytes()).collect();
            // SAFETY: a send from our buffer.
            unsafe { libc::send(conn.as_raw_fd(), bytes.as_ptr().cast(), bytes.len(), 0) };
        }
    }
}

/// The reply to a packet, for the commands that have one.
fn handle(words: &[i32]) -> Option<[i32; 2]> {
    match words[0] {
        LMK_GETKILLCNT => Some([LMK_GETKILLCNT, 0]),
        LMK_UPDATE_PROPS => Some([LMK_UPDATE_PROPS, 0]),
        LMK_BOOT_COMPLETED => {
            let r = if BOOT_COMPLETED.swap(true, SeqCst) {
                BOOT_COMPLETED_ALREADY_HANDLED
            } else {
                BOOT_COMPLETED_SUCCESS
            };
            Some([LMK_BOOT_COMPLETED, r])
        }
        LMK_TARGET | LMK_PROCPRIO | LMK_PROCREMOVE | LMK_PROCPURGE | LMK_SUBSCRIBE
        | LMK_START_MONITORING | LMK_PROCS_PRIO => None,
        other => {
            log::warn!("unknown command {other}");
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_what_lmkd_answers() {
        assert_eq!(
            handle(&[LMK_GETKILLCNT, 0, 1000]),
            Some([LMK_GETKILLCNT, 0])
        );
        assert_eq!(handle(&[LMK_UPDATE_PROPS]), Some([LMK_UPDATE_PROPS, 0]));
        assert_eq!(handle(&[LMK_PROCPRIO, 1234, 10001, 900, 0]), None);
        assert_eq!(handle(&[LMK_BOOT_COMPLETED]), Some([LMK_BOOT_COMPLETED, 0]));
        assert_eq!(handle(&[LMK_BOOT_COMPLETED]), Some([LMK_BOOT_COMPLETED, 2]));
    }
}
