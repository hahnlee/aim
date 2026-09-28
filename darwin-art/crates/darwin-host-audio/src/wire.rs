//! Requests and replies between the module (in the HAL's `linux-run`) and
//! its CoreAudio process (`linux-run --audio-io`, [`crate::io`]): fixed-size
//! records on a Unix stream socket, a ring's memfd riding along an open
//! request as `SCM_RIGHTS`.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use darwin_hostcall::audio::{Devices, Open};

/// `op` is the host-call function ([`darwin_hostcall::audio::FN_OPEN`]
/// and the rest).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Request {
    pub op: u32,
    pub reserved: u32,
    pub stream: u64,
    pub open: Open,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Reply {
    /// 0 or a negative errno.
    pub status: i64,
    pub stream: u64,
    pub devices: Devices,
}

fn bytes<T>(v: &T) -> &[u8] {
    // SAFETY: a plain-old-data record, viewed as its bytes.
    unsafe { std::slice::from_raw_parts((v as *const T).cast(), size_of::<T>()) }
}

const CMSG_SPACE: usize = 64;

/// Send a record, with `fd` as `SCM_RIGHTS`.
pub fn send<T>(sock: BorrowedFd, record: &T, fd: Option<i32>) -> io::Result<()> {
    let data = bytes(record);
    let mut iov = libc::iovec {
        iov_base: data.as_ptr().cast_mut().cast(),
        iov_len: data.len(),
    };
    let mut control = [0u64; CMSG_SPACE / 8];
    // SAFETY: msghdr over our buffers; the control buffer holds one fd.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if let Some(fd) = fd {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = libc::CMSG_SPACE(size_of::<i32>() as u32) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(size_of::<i32>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(c).cast::<i32>(), fd);
        }
        loop {
            let n = libc::sendmsg(sock.as_raw_fd(), &msg, 0);
            if n == data.len() as isize {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if n >= 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// Receive a record and the fd that came with it. `Ok(None)` at the end of
/// the stream; a timeout (`SO_RCVTIMEO`) is `WouldBlock`.
pub fn recv<T: Default>(sock: BorrowedFd) -> io::Result<Option<(T, Option<OwnedFd>)>> {
    let mut record = T::default();
    let mut iov = libc::iovec {
        iov_base: (&mut record as *mut T).cast(),
        iov_len: size_of::<T>(),
    };
    let mut control = [0u64; CMSG_SPACE / 8];
    // SAFETY: msghdr over our buffers; a passed fd becomes ours.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = CMSG_SPACE as _;
        let n = loop {
            let n = libc::recvmsg(sock.as_raw_fd(), &mut msg, libc::MSG_WAITALL);
            if n >= 0 {
                break n;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        };
        let mut fd = None;
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let raw = std::ptr::read_unaligned(libc::CMSG_DATA(c).cast::<i32>());
                fd = Some(OwnedFd::from_raw_fd(raw));
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        match n as usize {
            0 => Ok(None),
            n if n == size_of::<T>() => Ok(Some((record, fd))),
            _ => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }
}
