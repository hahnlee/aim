//! Mount namespaces: `unshare(CLONE_NEWNS)`, `mount` and `umount2`.
//!
//! zygote unshares its mount namespace and mounts storage views for each
//! child (`MountEmulatedStorage`, app data isolation): bind mounts and
//! tmpfs. Here a process's mounts are entries of its own path table
//! (`vfs::add_mount`), inherited by fork and carried over exec
//! (`--mounts`), so every process behaves as if it had unshared: nothing
//! propagates to other processes. The other namespaces are answered
//! "unsupported" (ADR 0012 appendix).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::errno::{EBUSY, EINVAL, ENODEV, ENOENT, ENOTDIR, EPERM};
use crate::vfs::{self, Area, LINUX_AT_FDCWD};

const CLONE_FILES: u64 = 0x400;
const CLONE_FS: u64 = 0x200;
const CLONE_NEWNS: u64 = 0x20000;
const CLONE_SYSVSEM: u64 = 0x40000;

const MS_RDONLY: u64 = 1;
const MS_REMOUNT: u64 = 32;
const MS_BIND: u64 = 4096;
const MS_MOVE: u64 = 8192;
const MS_UNBINDABLE: u64 = 1 << 17;
const MS_PRIVATE: u64 = 1 << 18;
const MS_SLAVE: u64 = 1 << 19;
const MS_SHARED: u64 = 1 << 20;
const PROPAGATION: u64 = MS_UNBINDABLE | MS_PRIVATE | MS_SLAVE | MS_SHARED;

const MNT_DETACH: u64 = 2;
const UMOUNT_NOFOLLOW: u64 = 8;

const CAP_SYS_ADMIN: u32 = 21;

pub fn unshare(a: [u64; 6]) -> i64 {
    // A process's files, cwd, SysV semaphore undo list and mount table are
    // already its own.
    if a[0] & !(CLONE_FILES | CLONE_FS | CLONE_SYSVSEM | CLONE_NEWNS) != 0 {
        return -(EINVAL as i64);
    }
    if a[0] & CLONE_NEWNS != 0 && !super::cred::capable(CAP_SYS_ADMIN) {
        return -(EPERM as i64);
    }
    0
}

fn resolve(path: u64, follow: bool) -> Result<vfs::Resolved, i64> {
    // SAFETY: guest string.
    let path = unsafe { super::guest_cstr(path) };
    vfs::resolve(LINUX_AT_FDCWD, path, follow).map_err(|e| -(e as i64))
}

fn is_dir(r: &vfs::Resolved) -> Result<bool, i64> {
    // SAFETY: host path, local buffer.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(r.host.as_ptr(), &mut st) } != 0 {
        return Err(-(ENOENT as i64));
    }
    Ok(st.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

/// A fresh host directory for a tmpfs mount, under the runtime directory
/// (per boot) or the host's temporary directory.
fn tmpfs_dir(data: &[u8]) -> Result<PathBuf, i64> {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let base = vfs::runtime_dir()
        .map(|r| r.join("tmpfs"))
        .unwrap_or_else(std::env::temp_dir);
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    let dir = base.join(format!("{pid}.{}", SEQ.fetch_add(1, Relaxed)));
    std::fs::create_dir_all(&dir).map_err(|_| -(ENOENT as i64))?;
    let mode = String::from_utf8_lossy(data)
        .split(',')
        .find_map(|o| o.strip_prefix("mode="))
        .and_then(|m| u32::from_str_radix(m, 8).ok())
        .unwrap_or(0o1777);
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode & 0o7777));
    Ok(dir)
}

pub fn mount(a: [u64; 6]) -> i64 {
    match do_mount(a) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn do_mount(a: [u64; 6]) -> Result<(), i64> {
    let flags = a[3];
    if !super::cred::capable(CAP_SYS_ADMIN) {
        return Err(-(EPERM as i64));
    }
    let target = resolve(a[1], true)?;
    if !is_dir(&target)? && flags & (MS_BIND | MS_MOVE) == 0 {
        return Err(-(ENOTDIR as i64));
    }
    if flags & MS_REMOUNT != 0 {
        // Mount options (read-only, nosuid, ...) are not enforced.
        return Ok(());
    }
    if flags & PROPAGATION != 0 {
        // Nothing propagates between processes here.
        return Ok(());
    }
    if flags & MS_BIND != 0 {
        let source = resolve(a[0], true)?;
        if is_dir(&source)? != is_dir(&target)? {
            return Err(-(ENOTDIR as i64));
        }
        let area = if source.area == Area::Image || flags & MS_RDONLY != 0 {
            Area::Image
        } else {
            source.area
        };
        // MS_REC or not, the mounts below the source that live in its host
        // tree come along.
        let host = PathBuf::from(OsStr::from_bytes(source.host.as_bytes()));
        vfs::add_mount(&target.guest, host, area, &source.guest, "bind");
        return Ok(());
    }
    if flags & MS_MOVE != 0 {
        let source = resolve(a[0], true)?;
        return if vfs::move_mount(&source.guest, &target.guest) {
            Ok(())
        } else {
            Err(-(EINVAL as i64))
        };
    }
    // SAFETY: guest strings.
    let fstype = unsafe { super::guest_cstr(a[2]) };
    match fstype {
        b"tmpfs" => {
            // SAFETY: guest string (options), may be null.
            let data = unsafe { super::guest_cstr(a[4]) };
            let dir = tmpfs_dir(data)?;
            let area = if flags & MS_RDONLY != 0 {
                Area::Image
            } else {
                Area::Writable
            };
            vfs::add_mount(&target.guest, dir, area, "tmpfs", "tmpfs");
            Ok(())
        }
        _ => Err(-(ENODEV as i64)),
    }
}

pub fn umount2(a: [u64; 6]) -> i64 {
    if a[1] & !(MNT_DETACH | UMOUNT_NOFOLLOW | 1 | 4) != 0 {
        return -(EINVAL as i64);
    }
    if !super::cred::capable(CAP_SYS_ADMIN) {
        return -(EPERM as i64);
    }
    let target = match resolve(a[0], a[1] & UMOUNT_NOFOLLOW == 0) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if vfs::remove_mount(&target.guest) {
        0
    } else if target.guest == "/" {
        -(EBUSY as i64)
    } else {
        -(EINVAL as i64)
    }
}
