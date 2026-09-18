//! Native Unix connection ownership, separate from Android/service policy.
//! The socket creator shares the native intake/spawn inheritance boundary.
//! Readiness and protocol I/O never run under that guard.
use std::{
    io,
    mem::offset_of,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    ptr,
    time::{Duration, Instant},
};

pub(crate) fn connect_with_timeout(socket: &Path, timeout: Duration) -> io::Result<UnixStream> {
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Unix socket connect timed out",
        ));
    }
    let (address, address_length) = socket_address(socket)?;
    let descriptor = {
        // Share the EXACT Rust boundary used by native intake and owned spawn.
        // Only creation/ownership/CLOEXEC is protected; connect/poll stay outside.
        let _inheritance = darwin_art_scm_transfer::inheritance::guard()?;
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(raw) };
        set_close_on_exec(owned.as_raw_fd())?;
        set_nonblocking(owned.as_raw_fd(), true)?;
        owned
    };

    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket connect deadline overflow",
        )
    })?;
    let result = unsafe {
        libc::connect(
            descriptor.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_length,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(error)
                if error == libc::EINPROGRESS
                    || error == libc::EALREADY
                    || error == libc::EWOULDBLOCK
        ) {
            return Err(error);
        }
        wait_for_connect(descriptor.as_raw_fd(), deadline)?;
    }
    set_nonblocking(descriptor.as_raw_fd(), false)?;
    let stream = UnixStream::from(descriptor);
    // Darwin can report a pending Unix connect as writable with SO_ERROR == 0
    // before a peer is actually attached.  Confirm the stream has a peer
    // before handing it to the framed protocol.
    stream.peer_addr()?;
    Ok(stream)
}

fn socket_address(socket: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = socket.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let path_offset = offset_of!(libc::sockaddr_un, sun_path);
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket path is empty, contains NUL, or is too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as _;
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            address.sun_path.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
    }
    let address_length = path_offset
        .checked_add(bytes.len() + 1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Unix socket path overflow"))?;
    #[cfg(target_os = "macos")]
    {
        address.sun_len = u8::try_from(address_length).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unix socket address is too long",
            )
        })?;
    }
    let address_length = libc::socklen_t::try_from(address_length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket address is too long",
        )
    })?;
    Ok((address, address_length))
}

fn set_close_on_exec(descriptor: libc::c_int) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(descriptor: libc::c_int, enabled: bool) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let updated = if enabled {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, updated) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wait_for_connect(descriptor: libc::c_int, deadline: Instant) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Unix socket connect timed out",
            ));
        }
        let timeout = poll_timeout(deadline);
        let mut pollfd = libc::pollfd {
            fd: descriptor,
            events: libc::POLLOUT | libc::POLLERR | libc::POLLHUP,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut pollfd, 1, timeout) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Unix socket connect timed out",
            ));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Unix socket connect timed out",
            ));
        }
        let mut error: libc::c_int = 0;
        let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                descriptor,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut libc::c_int).cast(),
                &mut length,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        return Ok(());
    }
}

fn poll_timeout(deadline: Instant) -> libc::c_int {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return 0;
    }
    let millis = remaining.as_millis().saturating_add(1);
    millis.min(i32::MAX as u128) as libc::c_int
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn socket_path() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "darwin-art-runtime-client-{}-{id}.sock",
            std::process::id()
        ))
    }

    fn fill_nonaccepting_backlog(path: &Path) -> Vec<UnixStream> {
        let mut connections = Vec::new();
        for _ in 0..256 {
            match connect_with_timeout(path, Duration::from_millis(2)) {
                Ok(stream) => connections.push(stream),
                Err(_) => break,
            }
        }
        connections
    }

    #[test]
    fn connected_stream_is_blocking_close_on_exec_and_overflow_is_rejected() {
        let path = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        assert!(connect_with_timeout(&path, Duration::MAX).is_err());
        let stream = connect_with_timeout(&path, Duration::from_secs(1)).unwrap();
        let (peer, _) = listener.accept().unwrap();
        let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(flags & libc::O_NONBLOCK, 0);
        let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        assert!(stream.peer_addr().is_ok());
        drop((stream, peer, listener));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn nonaccepting_listener_connect_is_bounded() {
        let path = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let held = fill_nonaccepting_backlog(&path);
        let start = Instant::now();
        let result = connect_with_timeout(&path, Duration::from_millis(50));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(
            result.is_err(),
            "nonaccepting listener unexpectedly accepted"
        );
        drop(held);
        drop(listener);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn zero_timeout_does_not_wait_for_pending_connect() {
        let path = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let held = fill_nonaccepting_backlog(&path);
        let start = Instant::now();
        let result = connect_with_timeout(&path, Duration::ZERO);
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(
            result.is_err(),
            "zero-timeout connect unexpectedly succeeded"
        );
        drop(held);
        drop(listener);
        let _ = std::fs::remove_file(path);
    }
}
