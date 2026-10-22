//! A TCP socket's shutdown as Linux's `tcp_poll` reports it. Darwin marks
//! any shutdown with EOF and HUP: a peer's FIN, our own `SHUT_RD` or
//! `SHUT_WR` alike, and its poll then drops POLLOUT. Linux reports a
//! receive shutdown as IN | RDNORM | RDHUP, a send shutdown as OUT |
//! WRNORM, and HUP only once both directions are shut or the connection is
//! closed (`sk_shutdown == SHUTDOWN_MASK || state == TCP_CLOSE`). The
//! directions follow from the connection's TCP state.

use std::os::fd::RawFd;

/// `TCP_CONNECTION_INFO`, whose `tcpi_state` is its first byte.
const TCP_CONNECTION_INFO: i32 = 0x106;
/// Darwin's `TCPS_*`.
const CLOSED: u8 = 0;
const ESTABLISHED: u8 = 4;
const CLOSE_WAIT: u8 = 5;
const FIN_WAIT_1: u8 = 6;
const CLOSING: u8 = 7;
const LAST_ACK: u8 = 8;
const FIN_WAIT_2: u8 = 9;
const TIME_WAIT: u8 = 10;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shutdown {
    pub receive: bool,
    pub send: bool,
    closed: bool,
}

impl Shutdown {
    /// EPOLLHUP / POLLHUP.
    pub fn hangup(&self) -> bool {
        self.receive && self.send || self.closed
    }

    /// The shutdown of a connection in TCP state `state`, with what the
    /// host already reported: EOF on the receive side, on the send side.
    fn of(state: u8, receive_eof: bool, send_eof: bool) -> Self {
        Shutdown {
            // The peer's FIN has arrived, or our SHUT_RD (which changes no
            // state) made the receive side end.
            receive: receive_eof || matches!(state, CLOSE_WAIT | CLOSING | LAST_ACK | TIME_WAIT),
            // Our FIN has been sent.
            send: send_eof
                || matches!(
                    state,
                    FIN_WAIT_1 | CLOSING | LAST_ACK | FIN_WAIT_2 | TIME_WAIT
                ),
            closed: state == CLOSED,
        }
    }
}

/// `fd`'s TCP state, or none for a socket that is not TCP.
fn state(fd: RawFd) -> Option<u8> {
    let mut info = [0u8; 256];
    let mut len = info.len() as libc::socklen_t;
    // SAFETY: a local buffer of its length.
    let r = unsafe {
        libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            TCP_CONNECTION_INFO,
            info.as_mut_ptr().cast(),
            &mut len,
        )
    };
    (r == 0 && len > 0).then_some(info[0])
}

/// The shutdown of TCP socket `fd`, given the EOF the host reported on
/// either side; none for another kind of file.
pub(super) fn shutdown(fd: RawFd, receive_eof: bool, send_eof: bool) -> Option<Shutdown> {
    state(fd).map(|state| Shutdown::of(state, receive_eof, send_eof))
}

/// For Darwin's poll, which reports HUP alone for any shutdown: the
/// shutdown of TCP socket `fd` it reported HUP for. While established,
/// that HUP is our own SHUT_RD.
pub(super) fn polled_hangup(fd: RawFd) -> Option<Shutdown> {
    state(fd).map(|state| Shutdown::of(state, state == ESTABLISHED, false))
}

/// Whether `fd` has room to send now: its EVFILT_WRITE, which Darwin's
/// poll hides behind HUP.
pub(super) fn writable(fd: RawFd) -> bool {
    // SAFETY: a kqueue of our own, closed here; one change, one event.
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return false;
        }
        let mut change: libc::kevent = std::mem::zeroed();
        change.ident = fd as usize;
        change.filter = libc::EVFILT_WRITE;
        change.flags = libc::EV_ADD;
        let mut event: libc::kevent = std::mem::zeroed();
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let n = libc::kevent(kq, &change, 1, &mut event, 1, &zero);
        libc::close(kq);
        n == 1 && event.flags & libc::EV_ERROR == 0 && event.flags & libc::EV_EOF == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Darwin's states after each shutdown (as a loopback connection
    /// shows them), against `tcp_poll`'s HUP and RDHUP.
    #[test]
    fn shutdowns_as_tcp_poll_reports_them() {
        // The peer sent FIN: readable to its end, still writable, no HUP.
        let peer = Shutdown::of(CLOSE_WAIT, true, false);
        assert!(peer.receive && !peer.send && !peer.hangup());
        // Our own SHUT_WR: no HUP until the peer's FIN.
        let own = Shutdown::of(FIN_WAIT_2, false, true);
        assert!(!own.receive && own.send && !own.hangup());
        // Our own SHUT_RD while established.
        let read = Shutdown::of(ESTABLISHED, true, false);
        assert!(read.receive && !read.send && !read.hangup());
        // Both directions, or closed (a reset).
        assert!(Shutdown::of(TIME_WAIT, true, false).hangup());
        assert!(Shutdown::of(LAST_ACK, false, true).hangup());
        assert!(Shutdown::of(CLOSED, false, false).hangup());
    }

    #[test]
    fn loopback_states() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut a = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (b, _) = listener.accept().unwrap();
        assert_eq!(
            shutdown(b.as_raw_fd(), false, false).map(|s| s.hangup()),
            Some(false)
        );
        a.write_all(b"x").unwrap();
        a.shutdown(std::net::Shutdown::Write).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let s = shutdown(b.as_raw_fd(), true, false).unwrap();
        assert!(s.receive && !s.send && !s.hangup(), "{s:?}");
        assert!(writable(b.as_raw_fd()));
        b.shutdown(std::net::Shutdown::Write).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(shutdown(b.as_raw_fd(), true, false).unwrap().hangup());
        // Not TCP.
        let (u, _v) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(shutdown(u.as_raw_fd(), true, false), None);
    }
}
