use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use super::types::DeliveryError;

pub(crate) fn duplicate(fd: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if raw < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

pub(crate) fn make_guardian() -> Result<(OwnedFd, OwnedFd), DeliveryError> {
    // The same inheritance guard used by spawn_owned must cover pipe creation
    // and all close-on-exec flag setup; there is no post-fork race window.
    let _inheritance_guard =
        darwin_art_scm_transfer::inheritance::guard().map_err(|source| DeliveryError::Io {
            operation: "lock FD inheritance",
            source,
        })?;
    let mut fds = [-1; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(DeliveryError::Io {
            operation: "create delivery guardian",
            source: io::Error::last_os_error(),
        });
    }
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let read_flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    let read_fd_flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFD) };
    let write_fd_flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFD) };
    if read_flags < 0
        || read_fd_flags < 0
        || write_fd_flags < 0
        || unsafe {
            libc::fcntl(
                read.as_raw_fd(),
                libc::F_SETFL,
                read_flags | libc::O_NONBLOCK,
            )
        } < 0
        || unsafe {
            libc::fcntl(
                read.as_raw_fd(),
                libc::F_SETFD,
                read_fd_flags | libc::FD_CLOEXEC,
            )
        } < 0
        || unsafe {
            libc::fcntl(
                write.as_raw_fd(),
                libc::F_SETFD,
                write_fd_flags | libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(DeliveryError::Io {
            operation: "configure delivery guardian",
            source: io::Error::last_os_error(),
        });
    }
    Ok((read, write))
}

pub(crate) fn is_eof(fd: RawFd) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
    if result < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    if result == 0 || pollfd.revents & libc::POLLHUP == 0 {
        return Ok(false);
    }
    let mut byte = [0u8; 1];
    let read = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), byte.len()) };
    if read == 0 {
        Ok(true)
    } else if read < 0
        && io::Error::last_os_error()
            .raw_os_error()
            .is_some_and(|code| {
                code == libc::EAGAIN || code == libc::EWOULDBLOCK || code == libc::EINTR
            })
    {
        Ok(false)
    } else if read < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(false)
    }
}
