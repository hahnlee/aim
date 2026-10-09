//! O_TMPFILE (open(2)): an unnamed regular file in a directory, which
//! `linkat` names later unless it was opened with O_EXCL.
//!
//! Darwin cannot give an unlinked file a name, so the file keeps a hidden
//! name in its directory while it may still be linked; directory listings
//! leave such names out. As with memfds (`memfd.rs`), each open file
//! description holds a shared `flock` on the file, taken as the file is
//! made, so a hidden file no description holds is unreferenced, and the
//! next O_TMPFILE in that directory removes its name. Linking it renames the
//! hidden name (the file has no other), after which it is an ordinary file.
//! With O_EXCL the name goes at once.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicU32, Ordering};

use super::attrs::{self, Host};
use crate::errno::{self, EINVAL, ENOTDIR};
use crate::vfs::Resolved;

/// The hidden names: `.aim-tmpfile.<pid>.<n>`.
const PREFIX: &[u8] = b".aim-tmpfile.";

/// Whether a directory entry is a hidden O_TMPFILE name.
pub fn is_hidden(name: &[u8]) -> bool {
    name.starts_with(PREFIX)
}

/// Remove the hidden names in `dir` that no open file description holds.
fn sweep(dir: &std::path::Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if !is_hidden(e.file_name().as_bytes()) {
            continue;
        }
        let Ok(p) = CString::new(e.path().into_os_string().into_encoded_bytes()) else {
            continue;
        };
        // SAFETY: probing a hidden file; its name goes only when the
        // exclusive lock shows nothing holds it.
        unsafe {
            let fd = libc::open(
                p.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_EXLOCK,
            );
            if fd >= 0 {
                libc::unlink(p.as_ptr());
                libc::close(fd);
            }
        }
    }
}

/// An O_TMPFILE open of directory `dir` with host open flags `hflags`
/// (access mode, O_CLOEXEC and the like); `excl`: O_EXCL.
pub fn open(dir: &Resolved, excl: bool, hflags: i32, mode: u64) -> i64 {
    static MADE: AtomicU32 = AtomicU32::new(0);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path from the resolver, local buffer.
    if unsafe { libc::stat(dir.host.as_ptr(), &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return -(ENOTDIR as i64);
    }
    if let Err(e) = super::fs::check_writable(dir) {
        return e;
    }
    let host_dir = std::path::Path::new(std::ffi::OsStr::from_bytes(dir.host.as_bytes()));
    sweep(host_dir);
    // SAFETY: trivial.
    let name = format!(
        "{}{}.{}",
        String::from_utf8_lossy(PREFIX),
        unsafe { libc::getpid() },
        MADE.fetch_add(1, Ordering::Relaxed)
    );
    let Ok(host) = CString::new(host_dir.join(&name).into_os_string().into_encoded_bytes()) else {
        return -(EINVAL as i64);
    };
    let hflags = hflags | libc::O_CREAT | libc::O_EXCL | libc::O_SHLOCK;
    // SAFETY: a new file in the resolved directory.
    let fd = unsafe { libc::open(host.as_ptr(), hflags, mode as libc::c_uint) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    if let Err(error) = attrs::created(Host::Fd(fd), || format!("{}/{name}", dir.guest)) {
        unsafe { libc::close(fd); libc::unlink(host.as_ptr()); }
        return -(error as i64);
    }
    if excl {
        // SAFETY: the name just made.
        unsafe { libc::unlink(host.as_ptr()) };
    }
    if let Err(error)=super::fdtab::publish_guest(fd){
        unsafe{libc::close(fd);libc::unlink(host.as_ptr());}return -(error as i64);
    }
    fd as i64
}

/// linkat of open file `fd` to `new`: an O_TMPFILE file gets `new` as its
/// name; any other file a further link.
pub fn link_fd(fd: i32, new: &Resolved) -> i64 {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return -(errno::last() as i64);
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = &buf[..len];
    let name = path.rsplit(|&c| c == b'/').next().unwrap_or_default();
    let Ok(old) = CString::new(path) else {
        return -(EINVAL as i64);
    };
    // SAFETY: host paths.
    errno::check(unsafe {
        if is_hidden(name) {
            libc::renamex_np(old.as_ptr(), new.host.as_ptr(), libc::RENAME_EXCL)
        } else {
            libc::link(old.as_ptr(), new.host.as_ptr())
        }
    } as i64)
}

/// `/proc/self/fd/N` of an O_TMPFILE file not linked yet, `guest` its
/// path: `<dir>/#<inode> (deleted)`, as Linux shows it.
pub fn fd_link(guest: &str, st: &libc::stat) -> Option<String> {
    let (dir, name) = guest.rsplit_once('/')?;
    is_hidden(name.as_bytes()).then(|| format!("{dir}/#{} (deleted)", st.st_ino))
}

/// The fd `/proc/self/fd/N` (or `/proc/<own pid>/fd/N`) names.
pub fn proc_fd(guest: &str) -> Option<i32> {
    let rest = guest.strip_prefix("/proc/")?;
    let (pid, fd) = rest.split_once("/fd/")?;
    // SAFETY: trivial.
    let me = unsafe { libc::getpid() };
    if pid != "self" && pid.parse() != Ok(me) {
        return None;
    }
    fd.parse().ok()
}
