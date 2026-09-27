//! AF_UNIX sockets at host paths longer than Darwin's 104-byte `sun_path`.
//!
//! The runtime directory can be deep (a temp or data directory), so the
//! socket is bound or connected by its short name relative to the socket
//! directory, which is made this thread's working directory with
//! `pthread_fchdir_np` for the duration of the call. The syscall layer can
//! use the same technique for guest `bind`/`connect` on mapped paths
//! (`docs/guest-init-contract.md`, "Unix sockets").

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
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
    let (addr, len) = short_address(name)?;
    // SAFETY: fresh socket fd.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is ours.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    in_directory(dir, || {
        // SAFETY: connect with a valid sockaddr_un.
        if unsafe {
            libc::connect(
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
    Ok(UnixStream::from(owned))
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
        let mut root = std::env::temp_dir().join(format!("dgi-unixsock-{}", std::process::id()));
        std::fs::remove_dir_all(&root).unwrap();
        root.pop();
    }
}
