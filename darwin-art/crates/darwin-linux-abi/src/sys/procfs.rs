//! The parts of `/proc` the guest reads through links: `/proc/self/exe` and
//! `/proc/self/fd/N`. A full procfs view is future work.

use crate::errno::{self, ENOENT, Errno};

/// Strip `/proc/self/` or `/proc/<our pid>/` from a guest path.
fn self_relative(path: &[u8]) -> Option<&[u8]> {
    let rest = path.strip_prefix(b"/proc/")?;
    let slash = rest.iter().position(|&c| c == b'/')?;
    let (who, tail) = (&rest[..slash], &rest[slash + 1..]);
    let pid = super::process::getpid().to_string();
    (who == b"self" || who == b"thread-self" || who == pid.as_bytes()).then_some(tail)
}

/// Guest path of an open host fd.
pub fn fd_guest_path(fd: i32) -> Result<String, Errno> {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return Err(errno::last());
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let host = std::path::PathBuf::from(String::from_utf8_lossy(&buf[..len]).into_owned());
    Ok(crate::vfs::guest_path_of_host(&host).unwrap_or_else(|| host.display().to_string()))
}

/// Target of a `/proc` link, or None when `path` is not one we synthesize.
pub fn readlink(path: &[u8]) -> Option<Result<Vec<u8>, Errno>> {
    let tail = self_relative(path)?;
    if tail == b"exe" {
        return Some(Ok(super::process::exe_guest_path().into_bytes()));
    }
    let n = tail.strip_prefix(b"fd/")?;
    let fd: i32 = match std::str::from_utf8(n).ok().and_then(|s| s.parse().ok()) {
        Some(fd) => fd,
        None => return Some(Err(ENOENT)),
    };
    Some(
        fd_guest_path(fd)
            .map(String::into_bytes)
            .map_err(|_| ENOENT),
    )
}

/// Whether `path` names `/proc/self/exe`.
pub fn is_self_exe(path: &[u8]) -> bool {
    self_relative(path) == Some(b"exe")
}
