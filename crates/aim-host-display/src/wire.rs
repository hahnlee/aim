//! The private protocol between the host module and `aim-display`.
//!
//! One Unix stream connection per client process. The client first sends
//! [`OP_HELLO`]; the server answers with a filled
//! [`display::Connect`](aim_hostcall::display::Connect) and from then on
//! writes only [`display::Event`](aim_hostcall::display::Event) records,
//! which the guest reads directly. Requests are fixed-size [`Request`]
//! records; an [`OP_IMPORT`] carries the buffer's fd as `SCM_RIGHTS`.
//! Both ends are built from this crate, so the layout is checked only by
//! [`VERSION`].

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use aim_hostcall::display::Import;

/// Sent in the hello; the server closes a connection of another version.
pub const VERSION: u64 = 1;

/// `id` = [`VERSION`], `flag` = display index.
pub const OP_HELLO: u32 = 1;
/// `import` describes the buffer; its fd rides along.
pub const OP_IMPORT: u32 = 2;
pub const OP_PRESENT: u32 = 3;
pub const OP_RELEASE: u32 = 4;
/// `flag` = 1 to send vsync events, 0 to stop.
pub const OP_SET_VSYNC: u32 = 5;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Request {
    pub op: u32,
    pub flag: u32,
    /// The buffer of [`OP_PRESENT`] and [`OP_RELEASE`].
    pub id: u64,
    pub import: Import,
}

/// A plain-old-data record as bytes.
pub fn bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: `T` is a `#[repr(C)]` record without padding holes that
    // matter (padding is sent as whatever it holds).
    unsafe { std::slice::from_raw_parts((v as *const T).cast(), size_of::<T>()) }
}

/// Write all of `data`, with `fd` attached to its first byte.
pub fn send(sock: BorrowedFd, data: &[u8], fd: Option<RawFd>) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut _,
        iov_len: data.len(),
    };
    let mut control = [0u64; 4];
    // SAFETY: msghdr over local buffers; CMSG macros stay inside `control`.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if let Some(fd) = fd {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = libc::CMSG_SPACE(size_of::<RawFd>() as u32) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;
            libc::CMSG_DATA(c).cast::<RawFd>().write_unaligned(fd);
        }
        loop {
            let n = libc::sendmsg(sock.as_raw_fd(), &msg, 0);
            if n == data.len() as isize {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if n < 0 && e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // A stream socket in blocking mode sends a short record only
            // when the peer is gone.
            return Err(if n < 0 {
                e
            } else {
                io::ErrorKind::WriteZero.into()
            });
        }
    }
}

/// Read exactly `buf.len()` bytes and any fd sent with them. `Ok(false)` at
/// end of stream.
pub fn recv(sock: BorrowedFd, buf: &mut [u8], fd: &mut Option<OwnedFd>) -> io::Result<bool> {
    let mut done = 0;
    while done < buf.len() {
        let mut iov = libc::iovec {
            iov_base: buf[done..].as_mut_ptr().cast(),
            iov_len: buf.len() - done,
        };
        let mut control = [0u64; 4];
        // SAFETY: msghdr over local buffers; received fds are adopted once.
        let n = unsafe {
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = size_of_val(&control) as _;
            let n = libc::recvmsg(sock.as_raw_fd(), &mut msg, 0);
            let mut c = if n > 0 {
                libc::CMSG_FIRSTHDR(&msg)
            } else {
                std::ptr::null_mut()
            };
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let count =
                        ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<RawFd>();
                    for i in 0..count {
                        let raw = libc::CMSG_DATA(c).cast::<RawFd>().add(i).read_unaligned();
                        *fd = Some(OwnedFd::from_raw_fd(raw));
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
            n
        };
        match n {
            0 if done == 0 => return Ok(false),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n if n < 0 => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
            n => done += n as usize,
        }
    }
    Ok(true)
}

/// Read one `T` record.
pub fn recv_record<T: Copy + Default>(sock: BorrowedFd) -> io::Result<Option<T>> {
    let mut v = T::default();
    // SAFETY: `T` is plain old data; every byte pattern is a value.
    let buf = unsafe { std::slice::from_raw_parts_mut((&mut v as *mut T).cast(), size_of::<T>()) };
    let mut fd = None;
    Ok(recv(sock, buf, &mut fd)?.then_some(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, Write};
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn records_carry_their_fd() {
        let (a, b) = UnixStream::pair().unwrap();
        let path = std::env::temp_dir().join(format!("wire-{}", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let _ = std::fs::remove_file(&path);
        file.write_all(b"pixels").unwrap();
        let import = Request {
            op: OP_IMPORT,
            id: 9,
            ..Default::default()
        };
        let present = Request {
            op: OP_PRESENT,
            id: 9,
            ..Default::default()
        };
        send(a.as_fd(), bytes(&import), Some(file.as_raw_fd())).unwrap();
        send(a.as_fd(), bytes(&present), None).unwrap();

        let mut got = Request::default();
        let mut fd = None;
        // SAFETY: `Request` is plain old data.
        let buf = unsafe {
            std::slice::from_raw_parts_mut((&mut got as *mut Request).cast(), size_of::<Request>())
        };
        assert!(recv(b.as_fd(), buf, &mut fd).unwrap());
        assert_eq!((got.op, got.id), (OP_IMPORT, 9));
        let mut received = std::fs::File::from(fd.take().expect("the fd"));
        received.rewind().unwrap();
        let mut text = String::new();
        received.read_to_string(&mut text).unwrap();
        assert_eq!(text, "pixels");

        let next = recv_record::<Request>(b.as_fd()).unwrap().unwrap();
        assert_eq!((next.op, next.id), (OP_PRESENT, 9));
        drop(a);
        assert!(recv_record::<Request>(b.as_fd()).unwrap().is_none());
    }
}
