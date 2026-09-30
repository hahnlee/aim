//! The private protocol between the host module and `aim-display`.
//!
//! One Unix stream connection per client process. The client first sends
//! [`OP_HELLO`]; the server answers with a filled
//! [`display::Connect`](aim_hostcall::display::Connect) and from then on
//! writes only [`display::Event`](aim_hostcall::display::Event) records,
//! which the guest reads directly. Requests are fixed-size [`Request`]
//! records; an [`OP_IMPORT`] carries the buffer's fd as `SCM_RIGHTS`, an
//! [`OP_PRESENT`] and an [`OP_CURSOR`] their fences.
//!
//! The guest's task bridge (`docs/windows.md`) opens its own connection
//! with [`OP_WINDOWS`]; the server answers with a filled
//! [`display::Windows`](aim_hostcall::display::Windows), and from then on
//! both ends write [`display::Window`](aim_hostcall::display::Window)
//! records.
//!
//! The notification bridge (`docs/notifications.md`) opens one with
//! [`OP_NOTIFICATIONS`] and exchanges [`crate::notify`] frames on it.
//!
//! A window host (an app's shim, `docs/windows.md`) opens a connection with
//! [`OP_HOST`]; from then on both ends write [`Host`] records: the server
//! sends it the buffers, presents and task records of its package, and it
//! answers presents and sends its windows' requests and input.
//! Both ends are built from this crate, so the layout is checked only by
//! [`VERSION`].

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use aim_hostcall::display::{Import, Window};

/// Sent in the hello; the server closes a connection of another version.
pub const VERSION: u64 = 7;

/// `id` = [`VERSION`], `flag` = display index.
pub const OP_HELLO: u32 = 1;
/// `import` describes the buffer; its fd rides along.
pub const OP_IMPORT: u32 = 2;
/// `id` is the buffer. The fds are the present fence's writer, then the
/// acquire fence when `flag` has [`PRESENT_ACQUIRE`].
pub const OP_PRESENT: u32 = 3;
pub const OP_RELEASE: u32 = 4;
/// `flag` = 1 to send vsync events, 0 to stop.
pub const OP_SET_VSYNC: u32 = 5;
/// `id` = [`VERSION`]: the task bridge's hello.
pub const OP_WINDOWS: u32 = 6;
/// `id` = [`VERSION`]: a window host's hello; [`Host`] records follow.
pub const OP_HOST: u32 = 7;
/// The hardware cursor: `id` is the buffer (0: none), `x`, `y` its top left
/// corner, `flag` [`CURSOR_CHANGED`] and [`CURSOR_ACQUIRE`] (the fence
/// rides along).
pub const OP_CURSOR: u32 = 8;

/// [`OP_CURSOR`]: the buffer's content is new.
pub const CURSOR_CHANGED: u32 = 1;
/// [`OP_CURSOR`] carries an acquire fence.
pub const CURSOR_ACQUIRE: u32 = 2;
/// `id` = [`VERSION`]: the notification bridge's hello; the server answers
/// its mode (a `u32`), then [`crate::notify`] frames follow both ways.
pub const OP_NOTIFICATIONS: u32 = 9;

/// [`Host::op`] values.
pub mod host {
    /// Host: its package and launcher activity, `package/class`, in
    /// `window.text`; `id` = [`super::VERSION`].
    pub const HELLO: u32 = 1;
    /// Server: a buffer (`import`), its fd attached; `id` names it.
    pub const IMPORT: u32 = 2;
    /// Server: forget buffer `id`.
    pub const RELEASE: u32 = 3;
    /// Server: show buffer `id` in the host's windows; `flag` = sequence.
    pub const PRESENT: u32 = 4;
    /// Host: present `flag` has read its buffer; `id` = the time
    /// (`CLOCK_MONOTONIC` ns).
    pub const SAMPLED: u32 = 5;
    /// Either way: a task record (`window`), to the host as the bridge
    /// sends it, from the host as a request for the bridge.
    pub const WINDOW: u32 = 6;
    /// Host: an input event (`input`).
    pub const INPUT: u32 = 7;
    /// Host: task `flag`'s window has number `id`.
    pub const NUMBER: u32 = 8;
    /// Host: it minimized a window; stack the tasks as the screen does.
    pub const RESTACK: u32 = 9;
    /// Server: the pointer's image, `import.width` x `import.height`
    /// premultiplied RGBA pixels, which follow the record (`id` bytes; 0:
    /// the default cursor); `input.x`, `input.y` its hot spot.
    pub const CURSOR: u32 = 10;
    /// Either way: a [`crate::notify`] frame follows.
    pub const NOTIFY: u32 = 11;
}

/// [`HostInput::kind`] values: `translate::Input`'s methods. Positions
/// (`x`, `y`) are display pixels; a gesture phase is 0 began, 1 changed,
/// 2 ended.
pub mod input {
    /// `touch`: `task`, `code` = phase (0 down, 1 drag, 2 up), `x`, `y`,
    /// `area`.
    pub const TOUCH: u32 = 1;
    /// `key`: `code` = macOS virtual key, `down` = 0 up, 1 down, 2 a
    /// repeat, `flags` = `modifierFlags`.
    pub const KEY: u32 = 2;
    /// `hover`: `x`, `y`.
    pub const HOVER: u32 = 3;
    /// `flags_changed`: `code`, `flags` = `modifierFlags`.
    pub const FLAGS: u32 = 4;
    /// `scroll`: `x`, `y`, `dx`, `dy`, `code` = 1 precise | 2 momentum,
    /// `down` = the swipe's phase + 1 (0: none), `sx`, `sy` its motion.
    pub const SCROLL: u32 = 5;
    /// `leave`.
    pub const LEAVE: u32 = 6;
    /// `button`: `code` = 0 right, 1 middle, 2 back, 3 forward, `down`,
    /// `x`, `y`.
    pub const BUTTON: u32 = 7;
    /// `release_all`.
    pub const RELEASE_ALL: u32 = 8;
    /// `twist`: `code` = 0 magnify, 1 rotate, `down` = phase, `dx` = the
    /// amount, `x`, `y`, `area`.
    pub const TWIST: u32 = 9;
}

/// An input event a window host forwards.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HostInput {
    pub kind: u32,
    pub task: i32,
    pub code: u32,
    pub down: u32,
    pub x: f64,
    pub y: f64,
    pub area: [i32; 4],
    pub flags: u64,
    pub time_ns: i64,
    pub dx: f64,
    pub dy: f64,
    pub sx: f64,
    pub sy: f64,
}

/// A record between the display server and a window host.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Host {
    pub op: u32,
    pub flag: u32,
    pub id: u64,
    pub import: Import,
    pub window: Window,
    pub input: HostInput,
}

/// [`OP_PRESENT`] carries an acquire fence.
pub const PRESENT_ACQUIRE: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Request {
    pub op: u32,
    pub flag: u32,
    /// The buffer of [`OP_PRESENT`], [`OP_RELEASE`] and [`OP_CURSOR`].
    pub id: u64,
    pub import: Import,
    /// The position of [`OP_CURSOR`].
    pub x: i32,
    pub y: i32,
}

/// A plain-old-data record as bytes.
pub fn bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: `T` is a `#[repr(C)]` record without padding holes that
    // matter (padding is sent as whatever it holds).
    unsafe { std::slice::from_raw_parts((v as *const T).cast(), size_of::<T>()) }
}

/// Write all of `data`, with `fd` attached to its first byte.
pub fn send(sock: BorrowedFd, data: &[u8], fd: Option<RawFd>) -> io::Result<()> {
    send_fds(sock, data, fd.as_slice())
}

/// Write all of `data`, with `fds` attached to its first byte.
pub fn send_fds(sock: BorrowedFd, data: &[u8], fds: &[RawFd]) -> io::Result<()> {
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
        if !fds.is_empty() {
            let len = size_of_val(fds) as u32;
            assert!(libc::CMSG_SPACE(len) as usize <= size_of_val(&control));
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = libc::CMSG_SPACE(len) as _;
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(len) as _;
            for (i, fd) in fds.iter().enumerate() {
                libc::CMSG_DATA(c)
                    .cast::<RawFd>()
                    .add(i)
                    .write_unaligned(*fd);
            }
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

/// Read exactly `buf.len()` bytes and any fds sent with them, in order.
/// `Ok(false)` at end of stream.
pub fn recv(sock: BorrowedFd, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<bool> {
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
                        fds.push(OwnedFd::from_raw_fd(raw));
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
    Ok(recv(sock, buf, &mut Vec::new())?.then_some(v))
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
        let (fence, writer) = aim_sync_file::pair().unwrap();
        send_fds(
            a.as_fd(),
            bytes(&present),
            &[writer.as_raw_fd(), file.as_raw_fd()],
        )
        .unwrap();
        drop(writer);

        let mut got = Request::default();
        let mut fds = Vec::new();
        // SAFETY: `Request` is plain old data.
        let buf = unsafe {
            std::slice::from_raw_parts_mut((&mut got as *mut Request).cast(), size_of::<Request>())
        };
        assert!(recv(b.as_fd(), buf, &mut fds).unwrap());
        assert_eq!((got.op, got.id), (OP_IMPORT, 9));
        let mut received = std::fs::File::from(fds.pop().expect("the fd"));
        received.rewind().unwrap();
        let mut text = String::new();
        received.read_to_string(&mut text).unwrap();
        assert_eq!(text, "pixels");

        assert!(recv(b.as_fd(), buf, &mut fds).unwrap());
        assert_eq!((got.op, got.id, fds.len()), (OP_PRESENT, 9, 2));
        assert!(aim_sync_file::is_sync_file(fence.as_fd()));
        assert!(!aim_sync_file::wait(fence.as_fd(), 0));
        // The server's copy of the writer: dropping it fails the fence.
        fds.clear();
        assert!(aim_sync_file::wait(fence.as_fd(), 0));
        drop(a);
        assert!(recv_record::<Request>(b.as_fd()).unwrap().is_none());
    }
}
