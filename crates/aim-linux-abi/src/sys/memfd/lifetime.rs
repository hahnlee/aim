//! A native keeper pins each open description once. Darwin's
//! PROC_FP_SHARED reports whether its fileglob has other references,
//! including fork copies, fileports and SCM_RIGHTS still in a socket.
//! Guest flock and record locks never serve as lifetime signals.
//!
//! A crashed keeper leaves reference attributes behind: retain those names
//! rather than guessing whether descriptors are still in flight. Normal
//! guest crashes need no special recovery; the keeper outlives its clients.

use super::*;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const FORMAT: &CStr = c"dev.aim.memfd.lifetime";
pub(super) const REFERENCE: &str = "dev.aim.memfd.ref.";
const TIMEOUT: Duration = Duration::from_secs(10);

struct PrivateStream(UnixStream);

impl Drop for PrivateStream {
    fn drop(&mut self) {
        fdtab::unhide(self.0.as_raw_fd());
    }
}

fn error() -> io::Error {
    io::Error::last_os_error()
}

/// Register only new descriptions (create/reopen), never dup or adoption:
/// two keeper pins of one description would keep each other alive.
pub(super) fn hold(fd: i32) -> Result<(), errno::Errno> {
    request(Some(fd)).map_err(|e| errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))
}

/// Drop unreferenced pins before a guest lock operation, so the pin does
/// not extend an old description's flock/OFD lock past its last close.
pub(super) fn sync() -> Result<(), errno::Errno> {
    request(None).map_err(|e| errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))
}

fn request(fd: Option<i32>) -> io::Result<()> {
    let _own = super::super::fork::spawn::own_fds();
    let _lock = state_lock_io()?;
    let path = dir().join(".lifetime.sock");
    let stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ECONNREFUSED)) => {
            if fd.is_none() {
                return Ok(());
            }
            if e.raw_os_error() == Some(libc::ECONNREFUSED) {
                std::fs::remove_file(&path)?;
            }
            let mut cmd = Command::new(std::env::current_exe()?);
            if cfg!(test) {
                cmd.args([
                    "--exact",
                    "sys::memfd::tests::lifetime_worker",
                    "--nocapture",
                ]);
                // The test worker recognizes an inherited socket on stdin.
                let (token, child_token) = UnixStream::pair()?;
                cmd.stdin(Stdio::from(OwnedFd::from(child_token)));
                drop(token);
            } else {
                cmd.arg("--memfd-lifetime").stdin(Stdio::null());
            }
            let mut child = cmd
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let deadline = Instant::now() + TIMEOUT;
            let stream = loop {
                if let Ok(stream) = UnixStream::connect(&path) {
                    break stream;
                }
                if child.try_wait()?.is_some() || Instant::now() >= deadline {
                    // Do not kill a helper that may already hold references.
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            stream
        }
        Err(e) => return Err(e),
    };
    let raw = stream.into_raw_fd();
    let mut stream = PrivateStream(unsafe { UnixStream::from_raw_fd(fdtab::hide(raw)) });
    stream.0.set_read_timeout(Some(TIMEOUT))?;
    stream.0.set_write_timeout(Some(TIMEOUT))?;
    send(&stream.0, fd)?;
    let mut reply = [0; 4];
    stream.0.read_exact(&mut reply)?;
    match i32::from_le_bytes(reply) {
        0 => Ok(()),
        e => Err(io::Error::from_raw_os_error(e)),
    }
}

fn send(stream: &UnixStream, fd: Option<i32>) -> io::Result<()> {
    let mut byte = [u8::from(fd.is_some())];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u64; 4];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(4) };
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(4);
            libc::CMSG_DATA(cmsg).cast::<i32>().write_unaligned(fd);
        }
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) } == 1 {
        Ok(())
    } else {
        Err(error())
    }
}

fn receive(stream: &UnixStream) -> io::Result<Option<OwnedFd>> {
    let mut byte = [0u8];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u64; 4];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&control) as u32;
    if unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) } != 1 {
        return Err(error());
    }
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    let fd = if !cmsg.is_null()
        && unsafe {
            (*cmsg).cmsg_level == libc::SOL_SOCKET
                && (*cmsg).cmsg_type == libc::SCM_RIGHTS
                && (*cmsg).cmsg_len == libc::CMSG_LEN(4)
        } {
        Some(unsafe { OwnedFd::from_raw_fd(libc::CMSG_DATA(cmsg).cast::<i32>().read_unaligned()) })
    } else {
        None
    };
    if msg.msg_flags & libc::MSG_CTRUNC != 0 || (byte[0] != 0) != fd.is_some() {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    Ok(fd)
}

struct Pin {
    fd: OwnedFd,
    reference: CString,
}

/// sys/proc_info.h: proc_fileinfo followed by vnode_info.
#[repr(C)]
struct VnodeFdInfo {
    open_flags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guard_flags: u32,
    vnode: libc::vnode_info,
}

impl Pin {
    fn new(fd: OwnedFd, serial: u64) -> io::Result<Self> {
        let raw = fd.as_raw_fd();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let reference = CString::new(format!(
            "{REFERENCE}{}.{nonce:x}.{serial:x}",
            std::process::id()
        ))
        .unwrap();
        if unsafe { libc::fsetxattr(raw, reference.as_ptr(), c"1".as_ptr().cast(), 1, 0, 0) } < 0
            || unsafe { libc::fsetxattr(raw, FORMAT.as_ptr(), c"1".as_ptr().cast(), 1, 0, 0) } < 0
        {
            return Err(error());
        }
        Ok(Self { fd, reference })
    }

    fn shared(&self) -> bool {
        let mut info: VnodeFdInfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        let n = unsafe {
            libc::proc_pidfdinfo(
                std::process::id() as i32,
                self.fd.as_raw_fd(),
                1, /* PROC_PIDFDVNODEINFO */
                (&raw mut info).cast(),
                size,
            )
        };
        // XNU fill_fileinfo sets PROC_FP_SHARED iff fg_count > 1.
        n != size || info.status & 1 /* PROC_FP_SHARED */ != 0
    }

    fn release(&self) {
        let fd = self.fd.as_raw_fd();
        if unsafe { libc::fremovexattr(fd, self.reference.as_ptr(), 0) } < 0 {
            return;
        }
        if !try_attr_names(fd).is_ok_and(|names| !names.iter().any(|a| a.starts_with(REFERENCE))) {
            return;
        }
        let mut path = [0u8; libc::PATH_MAX as usize];
        if unsafe { libc::fcntl(fd, libc::F_GETPATH, path.as_mut_ptr()) } < 0 {
            return;
        }
        let path = unsafe { CStr::from_ptr(path.as_ptr().cast()) };
        let mut named: libc::stat = unsafe { std::mem::zeroed() };
        if let Some(st) = memfd_stat(fd)
            && unsafe { libc::lstat(path.as_ptr(), &mut named) } == 0
            && key_of(&st) == key_of(&named)
        {
            unsafe { libc::unlink(path.as_ptr()) };
        }
    }
}

pub(crate) fn serve() -> io::Result<()> {
    let path = dir().join(".lifetime.sock");
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    let mut pins: Vec<Pin> = Vec::new();
    let mut serial = 0;
    let reap = |pins: &mut Vec<Pin>| {
        pins.retain(|p| {
            if p.shared() {
                true
            } else {
                p.release();
                false
            }
        });
    };
    let mut idle = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Darwin accept inherits the listener's nonblocking mode.
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(TIMEOUT))?;
                let mut uid = 0;
                let mut gid = 0;
                if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } < 0
                    || uid != unsafe { libc::geteuid() }
                {
                    continue;
                }
                let result = receive(&stream).and_then(|fd| {
                    reap(&mut pins);
                    if let Some(fd) = fd {
                        serial += 1;
                        pins.push(Pin::new(fd, serial)?);
                    }
                    Ok(())
                });
                let code = result
                    .err()
                    .map_or(0, |e| e.raw_os_error().unwrap_or(libc::EIO));
                let _ = stream.write_all(&code.to_le_bytes());
                idle = Instant::now();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                reap(&mut pins);
                if pins.is_empty() && idle.elapsed() >= Duration::from_millis(100) {
                    // New clients hold this same lock through their reply.
                    let lock = std::fs::File::open(dir().join(".state-lock"))?;
                    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0
                    {
                        std::fs::remove_file(&path)?;
                        return Ok(());
                    }
                }
                let mut ready = libc::pollfd {
                    fd: listener.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut ready, 1, 100) } < 0
                    && error().raw_os_error() != Some(libc::EINTR)
                {
                    return Err(error());
                }
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeper_waits_for_request_payload() {
        let path = dir().join(format!("payload-test-{}", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        assert_eq!(unsafe { libc::fchflags(file.as_raw_fd(), MARK) }, 0);
        hold(file.as_raw_fd()).unwrap();
        {
            let _own = super::super::super::fork::spawn::own_fds();
            let _lock = state_lock_io().unwrap();
            let mut stream = UnixStream::connect(dir().join(".lifetime.sock")).unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            let mut ready = libc::pollfd {
                fd: stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // Connecting is not a request: the owner must wait for payload.
            assert_eq!(unsafe { libc::poll(&mut ready, 1, 50) }, 0);
            send(&stream, None).unwrap();
            let mut reply = [0; 4];
            stream.read_exact(&mut reply).unwrap();
            assert_eq!(i32::from_le_bytes(reply), 0);
        }
        drop(file);
        sync().unwrap();
        assert!(!path.exists());
    }
}
