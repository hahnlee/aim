//! The `fwmarkd` socket (netd's FwmarkServer). libnetd_client in every
//! process sends a FwmarkCommand, with the socket as SCM_RIGHTS, on socket
//! creation, connect, sends and close, and waits for an int reply. netd
//! would mark the socket with its network and account it; with no policy
//! routing here the answer is always success.

use std::os::fd::RawFd;

/// FwmarkCommand plus FwmarkConnectInfo: 16 + 36 bytes; room to spare.
const MAX_MESSAGE: usize = 128;

/// Serve `listener` (init's `fwmarkd` socket, bound but not listening:
/// SocketListener::startListener listens) forever.
pub fn serve(listener: RawFd) {
    // SAFETY: listening on the socket init handed us.
    if unsafe { libc::listen(listener, 4) } != 0 {
        log::error!("fwmarkd listen: {}", std::io::Error::last_os_error());
        return;
    }
    loop {
        // SAFETY: accepting on our listening socket.
        let conn = unsafe {
            libc::accept4(
                listener,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        };
        if conn < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            log::error!("fwmarkd accept: {}", std::io::Error::last_os_error());
            return;
        }
        answer(conn);
        // SAFETY: closing the accepted connection.
        unsafe { libc::close(conn) };
    }
}

fn answer(conn: RawFd) {
    let mut buf = [0u8; MAX_MESSAGE];
    // Space for one descriptor (CMSG_SPACE(sizeof(int)) on 64-bit Linux).
    let mut control = [0u64; 4];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    // SAFETY: a zeroed msghdr pointing at our buffers.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&control);
    // SAFETY: receiving into the buffers above.
    if unsafe { libc::recvmsg(conn, &mut msg, libc::MSG_CMSG_CLOEXEC) } <= 0 {
        return;
    }
    // Close the socket the client passed; nothing is marked here.
    // SAFETY: walking the control messages the kernel wrote.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let fd = (libc::CMSG_DATA(c) as *const RawFd).read_unaligned();
                libc::close(fd);
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    let ok = 0i32.to_ne_bytes();
    // SAFETY: sending a local int.
    unsafe { libc::send(conn, ok.as_ptr().cast(), ok.len(), libc::MSG_NOSIGNAL) };
}
