//! AF_UNIX sockets at host paths longer than Darwin's 104-byte `sun_path`.
//!
//! The runtime directory can be deep (a temp or data directory), so the
//! socket is bound or connected by its short name relative to the socket
//! directory, which is made this thread's working directory with
//! `pthread_fchdir_np` for the duration of the call. guest-init's sockets,
//! the display server's and its input devices' all go this way; the
//! syscall layer can use the same technique for guest `bind`/`connect` on
//! mapped paths (`docs/guest-init-contract.md`, "Unix sockets").

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

unsafe extern "C" {
    /// libpthread: sets the calling thread's working directory; `-1`
    /// returns the thread to the process working directory.
    fn pthread_fchdir_np(fd: libc::c_int) -> libc::c_int;
}

/// Runs `f` with `dir` as this thread's working directory.
fn in_directory<T>(dir: &Path, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let c_dir = CString::new(dir.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: open/close of a directory fd; pthread_fchdir_np only changes
    // this thread's cwd and is undone before returning.
    unsafe {
        let fd = libc::open(
            c_dir.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        if pthread_fchdir_np(fd) != 0 {
            let error = io::Error::last_os_error();
            libc::close(fd);
            return Err(error);
        }
        let result = f();
        pthread_fchdir_np(-1);
        libc::close(fd);
        result
    }
}

fn short_address(name: &str) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: zeroed sockaddr_un is valid.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = name.as_bytes();
    if bytes.len() >= addr.sun_path.len() || name.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket name '{name}' must be a short file name"),
        ));
    }
    for (i, byte) in bytes.iter().enumerate() {
        addr.sun_path[i] = *byte as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    addr.sun_len = len as u8;
    Ok((addr, len))
}

/// Creates a socket of `socket_type` bound at `dir/name` (replacing a
/// stale file). The fd is close-on-exec.
pub fn bind_at(dir: &Path, name: &str, socket_type: libc::c_int) -> io::Result<OwnedFd> {
    let (addr, len) = short_address(name)?;
    // SAFETY: fresh socket fd, owned immediately.
    let fd = unsafe { libc::socket(libc::AF_UNIX, socket_type, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is ours.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: fcntl on our fd.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    let _ = std::fs::remove_file(dir.join(name));
    in_directory(dir, || {
        // SAFETY: bind with a valid sockaddr_un.
        if unsafe {
            libc::bind(
                owned.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                len,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })?;
    Ok(owned)
}

/// Connects a stream socket to `dir/name`.
pub fn connect_at(dir: &Path, name: &str) -> io::Result<UnixStream> {
    // SAFETY: fresh socket fd.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is ours.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: fcntl on our fd.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    connect_fd_at(owned.as_raw_fd(), dir, name)?;
    Ok(UnixStream::from(owned))
}

/// Connects the caller's socket `fd` to `dir/name`.
pub fn connect_fd_at(fd: RawFd, dir: &Path, name: &str) -> io::Result<()> {
    let (addr, len) = short_address(name)?;
    in_directory(dir, || {
        // SAFETY: connect with a valid sockaddr_un.
        if unsafe { libc::connect(fd, (&addr as *const libc::sockaddr_un).cast(), len) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })
}

/// The directory and file name of a socket path.
fn split(path: &Path) -> io::Result<(&Path, &str)> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(io::ErrorKind::InvalidInput)?;
    Ok((
        if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        },
        name,
    ))
}

/// A stream socket listening at `path`, whatever the path's length.
pub fn listen(path: &Path, backlog: libc::c_int) -> io::Result<UnixListener> {
    let (dir, name) = split(path)?;
    let fd = bind_at(dir, name, libc::SOCK_STREAM)?;
    // SAFETY: listen on our fd.
    if unsafe { libc::listen(fd.as_raw_fd(), backlog) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(UnixListener::from(fd))
}

/// A stream socket connected to `path`, whatever the path's length.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    let (dir, name) = split(path)?;
    connect_at(dir, name)
}

/// Connects the caller's socket `fd` to `path`, whatever its length.
pub fn connect_fd(fd: RawFd, path: &Path) -> io::Result<()> {
    let (dir, name) = split(path)?;
    connect_fd_at(fd, dir, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn long_directories_work() {
        let mut dir = std::env::temp_dir().join(format!("dgi-unixsock-{}", std::process::id()));
        for _ in 0..6 {
            dir = dir.join("a-rather-long-directory-name");
        }
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir.join("sock").as_os_str().len() > 104);
        let listener = bind_at(&dir, "sock", libc::SOCK_STREAM).unwrap();
        // SAFETY: listen on our fd.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 8) }, 0);
        let listener = std::os::unix::net::UnixListener::from(listener);
        let mut client = connect_at(&dir, "sock").unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        server.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        // By the whole path, as the display server and its clients go.
        let path = dir.join("display");
        let listener = listen(&path, 8).unwrap();
        let mut client = connect(&path).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(b"pong").unwrap();
        server.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"pong");
        // SAFETY: a fresh socket of ours.
        let fd = unsafe { OwnedFd::from_raw_fd(libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0)) };
        connect_fd(fd.as_raw_fd(), &path).unwrap();
        assert!(listener.accept().is_ok());
        let mut root = std::env::temp_dir().join(format!("dgi-unixsock-{}", std::process::id()));
        std::fs::remove_dir_all(&root).unwrap();
        root.pop();
    }
}
