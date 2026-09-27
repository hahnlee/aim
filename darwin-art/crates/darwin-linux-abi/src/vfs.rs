//! Guest path view: the guest's `/` is a host directory (`--root`).
//!
//! Paths are resolved component by component with symlinks interpreted
//! relative to the guest root, the way a chroot would, so absolute links
//! inside the image (for example `/bin -> /system/bin`) stay inside it.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::errno::{self, Errno};

pub const LINUX_AT_FDCWD: i32 = -100;
const MAX_SYMLINKS: usize = 40;

/// Host device nodes the guest sees at the same path.
const HOST_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
];

struct Vfs {
    root: PathBuf,
    cwd: Mutex<String>,
}

static VFS: OnceLock<Vfs> = OnceLock::new();

pub fn init(root: &Path) -> std::io::Result<()> {
    let root = root.canonicalize()?;
    let _ = VFS.set(Vfs {
        root,
        cwd: Mutex::new("/".into()),
    });
    Ok(())
}

fn vfs() -> &'static Vfs {
    VFS.get().expect("vfs not initialized")
}

pub fn root() -> &'static Path {
    &vfs().root
}

pub fn cwd() -> String {
    vfs().cwd.lock().unwrap().clone()
}

pub fn set_cwd(guest: String) {
    *vfs().cwd.lock().unwrap() = guest;
}

/// Guest path of a host path, if it lies inside the root.
pub fn guest_path_of_host(host: &Path) -> Option<String> {
    let rel = host.strip_prefix(root()).ok()?;
    Some(format!("/{}", rel.display()))
}

/// Guest path of an open directory fd.
fn guest_path_of_fd(fd: i32) -> Result<String, Errno> {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return Err(errno::last());
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let host = Path::new(OsStr::from_bytes(&buf[..len]));
    Ok(guest_path_of_host(host).unwrap_or_else(|| host.display().to_string()))
}

fn host_of(components: &[Vec<u8>]) -> PathBuf {
    let mut p = root().to_path_buf();
    for c in components {
        p.push(OsStr::from_bytes(c));
    }
    p
}

pub struct Resolved {
    pub host: CString,
    pub guest: String,
}

/// Resolve a guest path relative to a Linux dirfd into a host path.
pub fn resolve(dirfd: i32, path: &[u8], follow_last: bool) -> Result<Resolved, Errno> {
    if path.is_empty() {
        return Err(errno::ENOENT);
    }
    let base = if path[0] == b'/' {
        String::from("/")
    } else if dirfd == LINUX_AT_FDCWD {
        cwd()
    } else {
        guest_path_of_fd(dirfd)?
    };
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut joined = base.into_bytes();
    joined.push(b'/');
    joined.extend_from_slice(path);
    for c in joined.split(|&b| b == b'/').rev() {
        if !c.is_empty() {
            pending.push(c.to_vec());
        }
    }
    let mut done: Vec<Vec<u8>> = Vec::new();
    let mut links = 0;
    while let Some(c) = pending.pop() {
        match c.as_slice() {
            b"." => continue,
            b".." => {
                done.pop();
                continue;
            }
            _ => {}
        }
        done.push(c);
        let is_last = pending.is_empty();
        if is_last && !follow_last {
            break;
        }
        let host = host_of(&done);
        let Ok(target) = std::fs::read_link(&host) else {
            continue;
        };
        links += 1;
        if links > MAX_SYMLINKS {
            return Err(errno::ELOOP);
        }
        done.pop();
        let t = target.as_os_str().as_bytes();
        if t.first() == Some(&b'/') {
            done.clear();
        }
        for part in t.split(|&b| b == b'/').rev() {
            if !part.is_empty() {
                pending.push(part.to_vec());
            }
        }
    }
    let mut guest = String::new();
    for c in &done {
        guest.push('/');
        guest.push_str(&String::from_utf8_lossy(c));
    }
    if guest.is_empty() {
        guest.push('/');
    }
    let host = if HOST_DEVICES.contains(&guest.as_str()) {
        PathBuf::from(&guest)
    } else {
        host_of(&done)
    };
    Ok(Resolved {
        host: CString::new(host.as_os_str().as_bytes()).map_err(|_| errno::EINVAL)?,
        guest,
    })
}
