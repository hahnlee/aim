//! Darwin native guardian operations; transfer policy stays in the lease owner.
use super::{LeaseError, inheritance};
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
};

pub(crate) fn duplicate(fd: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    // SAFETY: fd is borrowed for this call; F_DUPFD_CLOEXEC returns a fresh fd.
    let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if raw < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: raw is a newly-created descriptor owned exactly once here.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }
}

pub(crate) fn make_guardian() -> Result<(OwnedFd, OwnedFd), LeaseError> {
    let _inheritance = inheritance::guard().map_err(|source| LeaseError::Io {
        operation: "protect guardian inheritance",
        source,
    })?;
    let mut fds = [-1; 2];
    // SAFETY: fds points to two writable c_int slots.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(LeaseError::Io {
            operation: "create guardian pipe",
            source: io::Error::last_os_error(),
        });
    }
    // SAFETY: pipe initialized both descriptors and ownership is transferred.
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    // SAFETY: fds[1] is the second distinct descriptor returned by pipe.
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
    let read_fd_flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFD) };
    let write_fd_flags = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || read_fd_flags < 0
        || write_fd_flags < 0
        || unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
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
        return Err(LeaseError::Io {
            operation: "set guardian flags",
            source: io::Error::last_os_error(),
        });
    }
    Ok((read, write))
}

pub(crate) fn guardian_is_eof(fd: RawFd) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    // SAFETY: descriptor is initialized and timeout zero never blocks.
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    if result == 0 || descriptor.revents & libc::POLLHUP == 0 {
        return Ok(false);
    }
    let mut byte = [0u8; 1];
    // SAFETY: byte is valid writable storage and fd is nonblocking.
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
