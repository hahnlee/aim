//! Namespace and metadata changes: mkdir, unlink, link, rename, chmod,
//! chown, truncate, fallocate, sync, utimens, flock. The image under a path
//! map is read-only (EROFS); ownership the host cannot represent is
//! recorded on the host inode (`attrs`).

use super::attrs::{self, Attr, Host};
use super::fs::{AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW, check_writable};
use super::memfd;
use super::tmpfile;
use crate::errno::{self, EINVAL, ENOENT, ENOSPC};
use crate::sys::{guest_cstr, procfs};
use crate::vfs::{self, Resolved};

const AT_REMOVEDIR: u64 = 0x200;
const AT_SYMLINK_FOLLOW: u64 = 0x400;
const EPERM: i64 = 1;
const EXDEV: i64 = 18;
const EISDIR: i64 = 21;
const ENOTEMPTY: i64 = 39;
const EOPNOTSUPP: i64 = 95;

fn resolve_w(dirfd: u64, path: u64, follow: bool) -> Result<Resolved, i64> {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(path) };
    let r = vfs::resolve(dirfd as i32, p, follow).map_err(|e| -(e as i64))?;
    check_writable(&r)?;
    Ok(r)
}

pub fn mkdirat(a: [u64; 6]) -> i64 {
    match resolve_w(a[0], a[1], false) {
        Ok(r) => {
            // The owner keeps access on the host (see host_mode), and
            // macOS drops S_ISVTX; the guest's mode is recorded when the
            // host's differs.
            let want = a[2] as u32 & 0o7777;
            let mode = host_mode(want, libc::S_IFDIR);
            // SAFETY: host path.
            let res = errno::check(unsafe { libc::mkdir(r.host.as_ptr(), mode) } as i64);
            if res == 0 {
                let host = Host::Path(&r.host);
                attrs::created(host, || r.guest.clone());
                if attrs::recording() {
                    let mut st: libc::stat = unsafe { std::mem::zeroed() };
                    // SAFETY: host path, local buffer.
                    unsafe { libc::stat(r.host.as_ptr(), &mut st) };
                    let made = st.st_mode as u32 & 0o7777;
                    let guest = (made & 0o077) | (want & 0o700) | (want & libc::S_ISVTX as u32);
                    if guest != made {
                        attrs::record(
                            host,
                            || r.guest.clone(),
                            Attr {
                                mode: Some(guest),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            res
        }
        Err(e) => e,
    }
}

pub fn mknodat(a: [u64; 6]) -> i64 {
    let mode = a[2] as u32;
    let r = match resolve_w(a[0], a[1], false) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let res = mknod_host(&r, mode);
    if res == 0 {
        attrs::created(Host::Path(&r.host), || r.guest.clone());
    }
    res
}

fn mknod_host(r: &Resolved, mode: u32) -> i64 {
    // SAFETY: host path.
    unsafe {
        match mode & libc::S_IFMT as u32 {
            m if m == libc::S_IFIFO as u32 => {
                errno::check(libc::mkfifo(r.host.as_ptr(), (mode & 0o7777) as u16) as i64)
            }
            0 | 0o100000 => {
                let fd = libc::open(
                    r.host.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
                    mode & 0o7777,
                );
                if fd < 0 {
                    return -(errno::last() as i64);
                }
                libc::close(fd);
                0
            }
            // Device nodes and sockets need privileges the layer does not
            // grant.
            _ => -EPERM,
        }
    }
}

fn directory_permits(st: &libc::stat, want: u32, id: &super::cred::Identity) -> bool {
    let bits = if st.st_uid == id.uid[3] {
        (st.st_mode as u32 >> 6) & 7
    } else if st.st_gid == id.gid[3] || id.groups.contains(&st.st_gid) {
        (st.st_mode as u32 >> 3) & 7
    } else {
        st.st_mode as u32 & 7
    };
    bits & want == want
        || id.cap_eff & (1 << 1) != 0
        || (want & 2 == 0 && id.cap_eff & (1 << 2) != 0)
}
fn sticky_permits(parent: &libc::stat, target: &libc::stat, id: &super::cred::Identity) -> bool {
    parent.st_mode as u32 & libc::S_ISVTX as u32 == 0
        || id.uid[3] == parent.st_uid
        || id.uid[3] == target.st_uid
        || id.cap_eff & (1 << 3) != 0
}
fn unlink_permissions(r: &Resolved, id: &super::cred::Identity) -> Result<(), i64> {
    if !attrs::recording() {
        return Ok(());
    }
    let parent = std::path::Path::new(&r.guest)
        .parent()
        .ok_or(-crate::errno::EBUSY as i64)?;
    let directory = super::fs::stat_at(
        crate::vfs::LINUX_AT_FDCWD,
        parent.as_os_str().as_encoded_bytes(),
        0,
    )?;
    let target = super::fs::stat_at(crate::vfs::LINUX_AT_FDCWD, r.guest.as_bytes(), AT_SYMLINK_NOFOLLOW)?;
    if !directory_permits(&directory, 3, id) {return Err(-crate::errno::EACCES as i64);}
    if directory.st_mode as u32 & libc::S_ISVTX as u32 != 0 {
        if !sticky_permits(&directory, &target, id) {
            return Err(-EPERM);
        }
    }
    Ok(())
}

pub fn unlinkat(a: [u64; 6]) -> i64 {
    unlinkat_as(a, &super::cred::current())
}
fn unlinkat_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    if a[2] & !AT_REMOVEDIR != 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    let resolved = vfs::resolve_checked(a[0] as i32, path, false, |directory| {
        if !attrs::recording() {
            return Ok(());
        }
        let st = super::fs::stat_at(crate::vfs::LINUX_AT_FDCWD, directory.as_bytes(), 0)
            .map_err(|error| -error as i32)?;
        if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(crate::errno::ENOTDIR);
        }
        if directory_permits(&st, 1, id) {
            Ok(())
        } else {
            Err(crate::errno::EACCES)
        }
    })
    .map_err(|error| -i64::from(error));
    match resolved {
        Ok(r) => {
            let last = path
                .split(|&byte| byte == b'/')
                .rfind(|component| !component.is_empty());
            match last {
                None => {
                    return if a[2] & AT_REMOVEDIR != 0 {
                        -crate::errno::EBUSY as i64
                    } else {
                        -EISDIR
                    };
                }
                Some(b".") => {
                    return if a[2] & AT_REMOVEDIR != 0 {
                        -EINVAL as i64
                    } else {
                        -EISDIR
                    };
                }
                Some(b"..") => {
                    return if a[2] & AT_REMOVEDIR != 0 {
                        -ENOTEMPTY
                    } else {
                        -EISDIR
                    };
                }
                _ => (),
            }
            let parent = std::path::Path::new(&r.guest).parent().unwrap();
            let parent = match vfs::resolve(
                crate::vfs::LINUX_AT_FDCWD,
                parent.as_os_str().as_encoded_bytes(),
                true,
            ) {
                Ok(parent) => parent,
                Err(error) => return -i64::from(error),
            };
            if let Err(error) = check_writable(&parent) {
                return error;
            }
            if a[2] & AT_REMOVEDIR == 0 && path.last() == Some(&b'/') {
                return match super::fs::stat_at(
                    crate::vfs::LINUX_AT_FDCWD,
                    r.guest.as_bytes(),
                    AT_SYMLINK_NOFOLLOW,
                ) {
                    Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFDIR => -EISDIR,
                    Ok(_) => -crate::errno::ENOTDIR as i64,
                    Err(error) => error,
                };
            }
            if let Err(error) = unlink_permissions(&r, id) {
                return error;
            }
            let target = match super::fs::stat_at(
                crate::vfs::LINUX_AT_FDCWD,
                r.guest.as_bytes(),
                AT_SYMLINK_NOFOLLOW,
            ) {
                Ok(target) => target,
                Err(error) => return error,
            };
            let directory = target.st_mode & libc::S_IFMT == libc::S_IFDIR;
            if a[2] & AT_REMOVEDIR != 0 && !directory {
                return -crate::errno::ENOTDIR as i64;
            }
            if a[2] & AT_REMOVEDIR == 0 && directory {
                return -EISDIR;
            }
            if vfs::is_mountpoint(&r.guest) {
                return -crate::errno::EBUSY as i64;
            }
            // SAFETY: resolved host path.
            let result = unsafe {
                if a[2] & AT_REMOVEDIR != 0 {
                    libc::rmdir(r.host.as_ptr())
                } else {
                    libc::unlink(r.host.as_ptr())
                }
            };
            if result == 0 {
                return 0;
            }
            let error = errno::last() as i64;
            if a[2] == 0 && error == EPERM {
                // Darwin returns EPERM for directory unlink; Linux uses
                // EISDIR. Do not follow a symlink to a directory.
                let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
                // SAFETY: resolved host path and local stat buffer.
                if unsafe { libc::lstat(r.host.as_ptr(), &mut metadata) } == 0
                    && metadata.st_mode & libc::S_IFMT == libc::S_IFDIR
                {
                    return -EISDIR;
                }
            }
            -error
        }
        Err(e) => e,
    }
}


pub fn symlinkat(a: [u64; 6]) -> i64 {
    // SAFETY: guest string; the target is stored verbatim (guest-relative).
    let target = unsafe { guest_cstr(a[0]) };
    let Ok(t) = std::ffi::CString::new(target) else {
        return -(EINVAL as i64);
    };
    match resolve_w(a[1], a[2], false) {
        Ok(r) => {
            // SAFETY: host path.
            let res = errno::check(unsafe { libc::symlink(t.as_ptr(), r.host.as_ptr()) } as i64);
            if res == 0 {
                attrs::created(Host::Path(&r.host), || r.guest.clone());
            }
            res
        }
        Err(e) => e,
    }
}

pub fn linkat(a: [u64; 6]) -> i64 {
    let flags = a[4];
    if flags & !(AT_SYMLINK_FOLLOW | AT_EMPTY_PATH) != 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: guest path pointer.
    let empty = flags & AT_EMPTY_PATH != 0 && unsafe { guest_cstr(a[1]) }.is_empty();
    let old = if empty {
        None
    } else {
        match resolve_w(a[0], a[1], flags & AT_SYMLINK_FOLLOW != 0) {
            Ok(r) => Some(r),
            Err(e) => return e,
        }
    };
    let new = match resolve_w(a[2], a[3], false) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // An open file, by AT_EMPTY_PATH or its /proc/self/fd link (how an
    // O_TMPFILE file gets its name).
    let fd = match &old {
        None => Some(a[0] as i32),
        Some(r) if flags & AT_SYMLINK_FOLLOW != 0 => tmpfile::proc_fd(&r.guest),
        Some(_) => None,
    };
    match (fd, old) {
        (Some(fd), _) => tmpfile::link_fd(fd, &new),
        // SAFETY: host paths.
        (None, Some(old)) => {
            errno::check(unsafe { libc::link(old.host.as_ptr(), new.host.as_ptr()) } as i64)
        }
        (None, None) => unreachable!(),
    }
}

const RENAME_NOREPLACE: u64 = 1;
const RENAME_EXCHANGE: u64 = 2;
const RENAME_WHITEOUT: u64 = 4;

pub fn renameat2(a: [u64; 6]) -> i64 {
    let flags = a[4];
    if flags & RENAME_WHITEOUT != 0 {
        return -EOPNOTSUPP;
    }
    if flags & !(RENAME_NOREPLACE | RENAME_EXCHANGE) != 0
        || flags == RENAME_NOREPLACE | RENAME_EXCHANGE
    {
        return -(EINVAL as i64);
    }
    let old = match resolve_w(a[0], a[1], false) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let new = match resolve_w(a[2], a[3], false) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let hflags = if flags & RENAME_NOREPLACE != 0 {
        libc::RENAME_EXCL
    } else if flags & RENAME_EXCHANGE != 0 {
        libc::RENAME_SWAP
    } else {
        0
    };
    // SAFETY: host paths.
    let r = unsafe { libc::renamex_np(old.host.as_ptr(), new.host.as_ptr(), hflags) };
    if r < 0 {
        let e = errno::last();
        return if e == libc::EXDEV {
            -EXDEV
        } else {
            -(e as i64)
        };
    }
    // Owners and modes are attributes of the inodes: they moved along.
    0
}

pub fn renameat(a: [u64; 6]) -> i64 {
    renameat2([a[0], a[1], a[2], a[3], 0, 0])
}

/// The host mode for a guest chmod. Under a path map the guest's mode is
/// the recorded one (`attrs`), and the host user keeps owner access: guest
/// root, which Linux lets past the mode bits, runs as that user.
fn host_mode(mode: u32, st_mode: u16) -> u16 {
    if !attrs::recording() {
        return mode as u16;
    }
    let owner = if st_mode & libc::S_IFMT == libc::S_IFDIR {
        0o700
    } else {
        0o600
    };
    (mode | owner) as u16
}

pub fn fchmodat(a: [u64; 6]) -> i64 {
    let mode = a[2] as u32 & 0o7777;
    let r = match resolve_w(a[0], a[1], true) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    unsafe { libc::stat(r.host.as_ptr(), &mut st) };
    // SAFETY: host path.
    if unsafe { libc::chmod(r.host.as_ptr(), host_mode(mode, st.st_mode)) } < 0 {
        return -(errno::last() as i64);
    }
    attrs::record(
        Host::Path(&r.host),
        || r.guest,
        Attr {
            mode: Some(mode),
            ..Default::default()
        },
    );
    0
}

fn fd_guest(fd: i32) -> Option<String> {
    procfs::fd_guest_path(fd).ok()
}

pub fn fchmod(a: [u64; 6]) -> i64 {
    let (fd, mode) = (a[0] as i32, a[1] as u32 & 0o7777);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer, then a plain fchmod.
    unsafe { libc::fstat(fd, &mut st) };
    if unsafe { libc::fchmod(fd, host_mode(mode, st.st_mode)) } < 0 {
        return -(errno::last() as i64);
    }
    attrs::record(
        Host::Fd(fd),
        || fd_guest(fd).unwrap_or_default(),
        Attr {
            mode: Some(mode),
            ..Default::default()
        },
    );
    0
}

fn owner(uid: u64, gid: u64) -> Attr {
    let id = |v: u64| (v as u32 != u32::MAX).then_some(v as u32);
    Attr {
        uid: id(uid),
        gid: id(gid),
        mode: None,
    }
}

/// chown: the host cannot take Android ids. Under a path map the owner is
/// recorded; otherwise the host call decides.
fn chown_host(path: &std::ffi::CStr, guest: &str, uid: u64, gid: u64, follow: bool) -> i64 {
    if vfs::runtime_dir().is_some() {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: host path, local buffer.
        let r = unsafe {
            if follow {
                libc::stat(path.as_ptr(), &mut st)
            } else {
                libc::lstat(path.as_ptr(), &mut st)
            }
        };
        if r < 0 {
            return -(errno::last() as i64);
        }
        attrs::record(Host::Path(path), || guest.to_string(), owner(uid, gid));
        return 0;
    }
    // SAFETY: host path.
    errno::check(unsafe {
        if follow {
            libc::chown(path.as_ptr(), uid as u32, gid as u32)
        } else {
            libc::lchown(path.as_ptr(), uid as u32, gid as u32)
        }
    } as i64)
}

pub fn fchownat(a: [u64; 6]) -> i64 {
    let (uid, gid, flags) = (a[2], a[3], a[4]);
    // SAFETY: guest path pointer.
    if unsafe { guest_cstr(a[1]) }.is_empty() && flags & AT_EMPTY_PATH != 0 {
        return fchown([a[0], uid, gid, 0, 0, 0]);
    }
    let r = match resolve_w(a[0], a[1], flags & AT_SYMLINK_NOFOLLOW == 0) {
        Ok(r) => r,
        Err(e) => return e,
    };
    chown_host(
        &r.host,
        &r.guest,
        uid,
        gid,
        flags & AT_SYMLINK_NOFOLLOW == 0,
    )
}

pub fn fchown(a: [u64; 6]) -> i64 {
    let fd = a[0] as i32;
    if vfs::runtime_dir().is_some() {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: plain fstat.
        if unsafe { libc::fstat(fd, &mut st) } < 0 {
            return -(errno::last() as i64);
        }
        attrs::record(
            Host::Fd(fd),
            || fd_guest(fd).unwrap_or_default(),
            owner(a[1], a[2]),
        );
        return 0;
    }
    // SAFETY: plain fchown.
    errno::check(unsafe { libc::fchown(fd, a[1] as u32, a[2] as u32) } as i64)
}

pub fn truncate(a: [u64; 6]) -> i64 {
    match resolve_w(vfs::LINUX_AT_FDCWD as u64, a[0], true) {
        // SAFETY: host path.
        Ok(r) => errno::check(unsafe { libc::truncate(r.host.as_ptr(), a[1] as i64) } as i64),
        Err(e) => e,
    }
}

pub fn ftruncate(a: [u64; 6]) -> i64 {
    let (fd, len) = (a[0] as i32, a[1] as i64);
    if len < 0 {
        return -(EINVAL as i64);
    }
    if memfd::resize_sealed(fd, len as u64) {
        return -EPERM;
    }
    // SAFETY: plain ftruncate.
    errno::check(unsafe { libc::ftruncate(fd, len) } as i64)
}

const FALLOC_FL_KEEP_SIZE: u64 = 1;
const FALLOC_FL_PUNCH_HOLE: u64 = 2;

pub fn fallocate(a: [u64; 6]) -> i64 {
    let (fd, mode, off, len) = (a[0] as i32, a[1], a[2] as i64, a[3] as i64);
    if off < 0 || len <= 0 {
        return -(EINVAL as i64);
    }
    if mode == FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE {
        if memfd::write_sealed(fd) {
            return -EPERM;
        }
        let ph = libc::fpunchhole_t {
            fp_flags: 0,
            reserved: 0,
            fp_offset: off,
            fp_length: len,
        };
        // SAFETY: F_PUNCHHOLE with a local struct.
        return errno::check(unsafe { libc::fcntl(fd, libc::F_PUNCHHOLE, &ph) } as i64);
    }
    if mode & !FALLOC_FL_KEEP_SIZE != 0 {
        return -EOPNOTSUPP;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: plain fstat.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    let end = off + len;
    if mode & FALLOC_FL_KEEP_SIZE == 0 && end > st.st_size && memfd::resize_sealed(fd, end as u64) {
        return -EPERM;
    }
    let mut fs = libc::fstore_t {
        fst_flags: libc::F_ALLOCATEALL,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: (end - st.st_size).max(0),
        fst_bytesalloc: 0,
    };
    if fs.fst_length > 0 {
        match super::space::charge(fd, fs.fst_length as u64) {
            Ok(n) if n == fs.fst_length as u64 => {}
            Ok(_) => return -(ENOSPC as i64),
            Err(e) => return e,
        }
        // SAFETY: F_PREALLOCATE with a local fstore; advisory on APFS.
        unsafe { libc::fcntl(fd, libc::F_PREALLOCATE, &mut fs) };
    }
    if mode & FALLOC_FL_KEEP_SIZE == 0 && end > st.st_size {
        // SAFETY: plain ftruncate.
        return errno::check(unsafe { libc::ftruncate(fd, end) } as i64);
    }
    0
}

/// fsync (82), fdatasync (83), syncfs (267).
pub fn fsync(a: [u64; 6]) -> i64 {
    // SAFETY: plain fsync.
    errno::check(unsafe { libc::fsync(a[0] as i32) } as i64)
}

pub fn sync() -> i64 {
    // SAFETY: trivial.
    unsafe { libc::sync() };
    0
}

/// sync_file_range: write-out of a range; a full fsync covers it.
pub fn sync_file_range(a: [u64; 6]) -> i64 {
    if a[3] & !7 != 0 || (a[1] as i64) < 0 || (a[2] as i64) < 0 {
        return -(EINVAL as i64);
    }
    fsync(a)
}

pub fn flock(a: [u64; 6]) -> i64 {
    // LOCK_SH/EX/NB/UN agree.
    // SAFETY: plain flock.
    errno::check(unsafe { libc::flock(a[0] as i32, a[1] as i32) } as i64)
}

const UTIME_NOW: i64 = (1 << 30) - 1;
const UTIME_OMIT: i64 = (1 << 30) - 2;

pub fn utimensat(a: [u64; 6]) -> i64 {
    let (dirfd, path, times, flags) = (a[0] as i32, a[1], a[2], a[3]);
    let mut ts = [libc::timespec {
        tv_sec: 0,
        tv_nsec: libc::UTIME_NOW,
    }; 2];
    if times != 0 {
        // SAFETY: guest timespec[2].
        let t = unsafe { (times as *const [i64; 4]).read_unaligned() };
        for i in 0..2 {
            ts[i] = libc::timespec {
                tv_sec: t[i * 2],
                tv_nsec: match t[i * 2 + 1] {
                    UTIME_NOW => libc::UTIME_NOW,
                    UTIME_OMIT => libc::UTIME_OMIT,
                    n => n,
                },
            };
        }
    }
    // SAFETY: guest path pointer (NULL: the fd itself).
    if path == 0 || (unsafe { guest_cstr(path) }.is_empty() && flags & AT_EMPTY_PATH != 0) {
        // SAFETY: futimens with local timespecs.
        return errno::check(unsafe { libc::futimens(dirfd, ts.as_ptr()) } as i64);
    }
    let r = match resolve_w(dirfd as u64, path, flags & AT_SYMLINK_NOFOLLOW == 0) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let hflags = if flags & AT_SYMLINK_NOFOLLOW != 0 {
        libc::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    // SAFETY: host path and local timespecs.
    let res = unsafe { libc::utimensat(libc::AT_FDCWD, r.host.as_ptr(), ts.as_ptr(), hflags) };
    if res < 0 && errno::last() == ENOENT {
        return -(ENOENT as i64);
    }
    errno::check(res as i64)
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use super::*;
    use crate::sys::fs::{openat, stat_at};
    use crate::vfs::LINUX_AT_FDCWD;

    const AT: u64 = LINUX_AT_FDCWD as u64;

    fn c(path: &str) -> CString {
        CString::new(path).unwrap()
    }

    /// (uid, gid, mode) the guest's stat shows.
    fn owner(path: &str) -> (u32, u32, u32) {
        let st = stat_at(LINUX_AT_FDCWD, path.as_bytes(), AT_SYMLINK_NOFOLLOW).unwrap();
        (st.st_uid, st.st_gid, st.st_mode as u32 & 0o7777)
    }

    fn chown(path: &str, uid: u32, gid: u32) {
        let p = c(path);
        let flags = AT_SYMLINK_NOFOLLOW;
        let r = fchownat([AT, p.as_ptr() as u64, uid as u64, gid as u64, flags, 0]);
        assert_eq!(r, 0, "chown {path}");
    }

    fn chmod(path: &str, mode: u32) {
        let p = c(path);
        assert_eq!(fchmodat([AT, p.as_ptr() as u64, mode as u64, 0, 0, 0]), 0);
    }

    fn create(path: &str) {
        let p = c(path);
        let flags = 0o1 | 0o100 | 0o200 | 0o2000000; // O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC
        let fd = openat([AT, p.as_ptr() as u64, flags, 0o600, 0, 0]);
        assert!(fd >= 0, "create {path}: {fd}");
        // SAFETY: the fd just opened.
        unsafe { libc::close(fd as i32) };
    }

    #[test]
    fn owners_live_on_the_inode() {
        let (_view, dir) = vfs::test_view();
        let base = format!("/data/attrs-{}", std::process::id());
        let p = c(&base);
        assert_eq!(mkdirat([AT, p.as_ptr() as u64, 0o771, 0, 0, 0]), 0);
        chown(&base, 1000, 1000);
        chmod(&base, 0o771);
        assert_eq!(owner(&base), (1000, 1000, 0o771));

        // Created files belong to the creator (root here) until chowned.
        let (a, b) = (format!("{base}/a"), format!("{base}/b"));
        create(&a);
        assert_eq!(owner(&a).0, 0);
        chown(&a, 10123, 10123);
        chmod(&a, 0o640);
        assert_eq!(owner(&a), (10123, 10123, 0o640));
        create(&b);
        chown(&b, 2000, 2001);

        // A rename, an exchange and a hard link keep each inode's owner.
        let moved = c(&format!("{base}/moved"));
        let pa = c(&a);
        assert_eq!(
            renameat2([AT, pa.as_ptr() as u64, AT, moved.as_ptr() as u64, 0, 0]),
            0
        );
        assert_eq!(owner(&format!("{base}/moved")), (10123, 10123, 0o640));
        let pb = c(&b);
        let swap = [AT, pb.as_ptr() as u64, AT, moved.as_ptr() as u64, 2, 0];
        assert_eq!(renameat2(swap), 0);
        assert_eq!(owner(&b), (10123, 10123, 0o640));
        assert_eq!(owner(&format!("{base}/moved")).0, 2000);
        let link = c(&format!("{base}/link"));
        assert_eq!(
            linkat([AT, pb.as_ptr() as u64, AT, link.as_ptr() as u64, 0, 0]),
            0
        );
        assert_eq!(owner(&format!("{base}/link")), (10123, 10123, 0o640));

        // A file created read-only gets its owner too.
        let ro = c(&format!("{base}/ro"));
        let fd = crate::sys::fs::openat([AT, ro.as_ptr() as u64, 0o100 | 0o200, 0o444, 0, 0]);
        assert!(fd >= 0);
        // SAFETY: the fd just opened.
        unsafe { libc::close(fd as i32) };
        chown(&format!("{base}/ro"), 1013, 1013);
        assert_eq!(owner(&format!("{base}/ro")), (1013, 1013, 0o444));

        // A FIFO and a symlink carry their own owner.
        let fifo = c(&format!("{base}/fifo"));
        assert_eq!(
            mknodat([
                AT,
                fifo.as_ptr() as u64,
                libc::S_IFIFO as u64 | 0o600,
                0,
                0,
                0
            ]),
            0
        );
        chown(&format!("{base}/fifo"), 1036, 1036);
        assert_eq!(owner(&format!("{base}/fifo")).0, 1036);
        let sym = c(&format!("{base}/sym"));
        let target = c("b");
        assert_eq!(
            symlinkat([target.as_ptr() as u64, AT, sym.as_ptr() as u64, 0, 0, 0]),
            0
        );
        chown(&format!("{base}/sym"), 1001, 1001);
        assert_eq!(owner(&format!("{base}/sym")).0, 1001);
        assert_eq!(owner(&b).0, 10123);

        // A second boot starts with a fresh runtime directory: the owners
        // are still there (#261).
        let _ = std::fs::remove_file(dir.join("run/fs-attrs"));
        assert_eq!(owner(&b), (10123, 10123, 0o640));
        assert_eq!(owner(&base), (1000, 1000, 0o771));
    }

    #[test]
    fn image_paths_use_the_table() {
        let (_view, dir) = vfs::test_view();
        let name = format!("image-{}", std::process::id());
        let host = dir.join("root").join(&name);
        std::fs::create_dir_all(&host).unwrap();
        let guest = format!("/{name}");
        // The guest cannot change the read-only image.
        let p = c(&guest);
        assert_eq!(fchownat([AT, p.as_ptr() as u64, 1000, 1000, 0, 0]), -30);
        // init's chown of an image directory goes to the table.
        let h = CString::new(host.as_os_str().as_bytes()).unwrap();
        let a = Attr {
            uid: Some(1000),
            gid: Some(1001),
            mode: Some(0o751),
        };
        attrs::record(attrs::Host::Image(&h), || guest.clone(), a);
        let table = std::fs::read_to_string(dir.join("run/fs-attrs")).unwrap();
        assert!(
            table.contains(&format!("{guest}\t1000\t1001\t751\n")),
            "{table}"
        );
        assert_eq!(owner(&guest), (1000, 1001, 0o751));
    }

    /// The cost of a guest stat (#273): `cargo test -p aim-linux-abi --lib
    /// --release stat_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stat_cost() {
        let (_view, dir) = vfs::test_view();
        let base = format!("/data/bench-{}", std::process::id());
        let p = c(&base);
        assert_eq!(mkdirat([AT, p.as_ptr() as u64, 0o771, 0, 0, 0]), 0);
        let files: Vec<String> = (0..1000).map(|i| format!("{base}/f{i}")).collect();
        for f in &files {
            create(f);
            chown(f, 10123, 10123);
        }
        let image = dir.join("root/bench");
        std::fs::create_dir_all(&image).unwrap();
        let time = |what: &str, paths: &[String]| {
            let rounds = 20;
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                for f in paths {
                    std::hint::black_box(owner(f));
                }
            }
            let ns = start.elapsed().as_nanos() / (rounds * paths.len()) as u128;
            eprintln!("{what}: {ns} ns/stat");
        };
        time("writable file (guest attribute)", &files);
        time(
            "image file (table, original attribute)",
            &["/bench".to_string()],
        );
        // What every stat paid before, after any process appended to the
        // table: rereading and parsing it whole (a boot's 51,800 lines).
        let table: String = (0..51_800)
            .map(|i| format!("/data/data/com.example.app{i}/cache\t10123\t10123\t771\n"))
            .collect();
        let start = std::time::Instant::now();
        std::hint::black_box(attrs::tests_parse(&table));
        eprintln!(
            "table reparse ({} bytes): {} us",
            table.len(),
            start.elapsed().as_micros()
        );
    }
}

#[cfg(test)]
mod unlink_permission_tests {
    use super::*;
    #[test]
    fn unlink_and_rmdir_preserve_active_mount_backing_nodes() {
        use std::ffi::CString;
        let (_guard, view) = crate::vfs::test_view(); let base = format!("/data/remove-mount-{}", std::process::id()); let host = view.join("data").join(base.trim_start_matches("/data/")); std::fs::create_dir_all(&host).unwrap();
        let backing = view.join("remove-mount-backing"); std::fs::create_dir_all(&backing).unwrap(); std::fs::write(backing.join("keep"), b"owned").unwrap();
        let file = view.join("remove-mount-file"); std::fs::write(&file, b"owned").unwrap();
        let directory_guest = format!("{base}/directory"); let file_guest = format!("{base}/file");
        crate::vfs::add_mount(&directory_guest, backing.clone(), crate::vfs::Area::Writable, "bind", "bind"); crate::vfs::add_mount(&file_guest, file.clone(), crate::vfs::Area::Writable, "bind", "bind");
        let id = super::super::cred::Identity::default(); let invoke = |path: &str, flags| {let path = CString::new(path).unwrap(); unlinkat_as([crate::vfs::LINUX_AT_FDCWD as u64, path.as_ptr() as u64, flags, 0, 0, 0], &id)};
        assert_eq!(invoke(&directory_guest, AT_REMOVEDIR), -crate::errno::EBUSY as i64); assert_eq!(invoke(&directory_guest, 0), -EISDIR);
        assert_eq!(invoke(&file_guest, 0), -crate::errno::EBUSY as i64); assert_eq!(invoke(&file_guest, AT_REMOVEDIR), -crate::errno::ENOTDIR as i64);
        assert_eq!(invoke("/data", AT_REMOVEDIR), -30, "readonly parent mount precedes busy target");
        std::os::unix::fs::symlink(&directory_guest, host.join("link")).unwrap(); assert_eq!(invoke(&format!("{base}/link"), 0), 0);
        assert!(backing.join("keep").is_file() && file.is_file());
        crate::vfs::add_mount(&file_guest, file.clone(), crate::vfs::Area::Image, "bind", "bind"); assert_eq!(invoke(&file_guest, 0), -crate::errno::EBUSY as i64); assert!(crate::vfs::remove_mount(&file_guest));
        crate::vfs::add_mount(&directory_guest, file.clone(), crate::vfs::Area::Writable, "bind", "bind"); assert_eq!(invoke(&directory_guest, 0), -crate::errno::EBUSY as i64);
        assert!(crate::vfs::remove_mount(&directory_guest)); assert!(crate::vfs::is_mountpoint(&directory_guest)); assert!(crate::vfs::remove_mount(&directory_guest)); assert!(!crate::vfs::is_mountpoint(&directory_guest)); assert!(crate::vfs::remove_mount(&file_guest));
        std::fs::remove_dir_all(backing).unwrap(); std::fs::remove_file(file).unwrap(); std::fs::remove_dir_all(host).unwrap();
    }
    #[test]
    fn removal_last_components_and_trailing_slashes_keep_linux_errors() {
        use std::ffi::CString;
        let (_guard, view) = crate::vfs::test_view(); let base = format!("/data/unlink-last-{}", std::process::id()); let host = view.join("data").join(base.trim_start_matches("/data/")); std::fs::create_dir_all(host.join("dir")).unwrap(); std::fs::write(host.join("file"), b"owned").unwrap();
        std::os::unix::fs::symlink("dir", host.join("dirlink")).unwrap(); std::os::unix::fs::symlink("file", host.join("filelink")).unwrap();
        let id = super::super::cred::Identity::default(); let invoke = |path: &str, flags| {let path = CString::new(path).unwrap(); unlinkat_as([crate::vfs::LINUX_AT_FDCWD as u64, path.as_ptr() as u64, flags, 0, 0, 0], &id)};
        for root in ["/", "///"] {assert_eq!(invoke(root, 0), -EISDIR); assert_eq!(invoke(root, AT_REMOVEDIR), -crate::errno::EBUSY as i64);}
        for suffix in [".", "../"] {assert_eq!(invoke(&format!("{base}/{suffix}"), 0), -EISDIR); assert_eq!(invoke(&format!("{base}/{suffix}"), AT_REMOVEDIR), if suffix == "." {-EINVAL as i64} else {-ENOTEMPTY});}
        assert_eq!(invoke(&format!("{base}/file/"), 0), -crate::errno::ENOTDIR as i64);
        assert_eq!(invoke(&format!("{base}/dir/"), 0), -EISDIR);
        for link in ["dirlink", "filelink"] {assert_eq!(invoke(&format!("{base}/{link}/"), 0), -crate::errno::ENOTDIR as i64); assert_eq!(invoke(&format!("{base}/{link}/"), AT_REMOVEDIR), -crate::errno::ENOTDIR as i64);}
        assert_eq!(invoke(&format!("{base}/missing/"), 0), -crate::errno::ENOENT as i64);
        assert!(host.join("file").is_file() && host.join("dir").is_dir() && host.join("dirlink").symlink_metadata().is_ok());
        assert_eq!(invoke(&format!("{base}/dir/"), AT_REMOVEDIR), 0); std::fs::remove_dir_all(host).unwrap();
    }
    #[test]
    fn unlink_walk_checks_symlink_prefixes_dotdot_and_relative_fd_base() {
        use std::{ffi::CString, os::fd::AsRawFd};
        let (_guard, view) = crate::vfs::test_view();
        let base = format!("/data/unlink-walk-{}", std::process::id()); let host = view.join("data").join(base.trim_start_matches("/data/"));
        std::fs::create_dir_all(host.join("denied")).unwrap(); std::fs::create_dir_all(host.join("allowed")).unwrap();
        let record = |suffix: &str, uid, mode| {
            let path = CString::new(host.join(suffix).as_os_str().as_encoded_bytes()).unwrap();
            super::super::attrs::record(super::super::attrs::Host::Path(&path), || format!("{base}/{suffix}"), super::super::attrs::Attr {uid: Some(uid), gid: Some(2000), mode: Some(mode)});
        };
        record("denied", 1000, 0o700); record("allowed", 3000, 0o777);
        std::os::unix::fs::symlink(format!("{base}/allowed"), host.join("denied/link")).unwrap(); std::fs::write(host.join("allowed/victim"), b"owned").unwrap();
        let mut id = super::super::cred::Identity::default(); id.uid = [3000; 4]; id.gid = [3000; 4]; id.cap_eff = 0;
        let invoke = |fd, path: &str, flags| {let path = CString::new(path).unwrap(); unlinkat_as([fd as u64, path.as_ptr() as u64, flags, 0, 0, 0], &id)};
        assert_eq!(invoke(crate::vfs::LINUX_AT_FDCWD, &format!("{base}/denied/link/victim"), 0), -crate::errno::EACCES as i64);
        assert_eq!(invoke(crate::vfs::LINUX_AT_FDCWD, &format!("{base}/denied/../allowed/victim"), 0), -crate::errno::EACCES as i64);
        assert!(host.join("allowed/victim").is_file());
        std::fs::create_dir(host.join("denied/nested")).unwrap(); record("denied/nested", 3000, 0o777); std::fs::write(host.join("denied/nested/victim"), b"owned").unwrap();
        let fd = std::fs::File::open(host.join("denied/nested")).unwrap();
        assert_eq!(invoke(fd.as_raw_fd(), "victim", 0), 0, "dirfd access does not search ancestors above its base");
        record("allowed", 3000, 0o555);
        assert_eq!(invoke(crate::vfs::LINUX_AT_FDCWD, &format!("{base}/allowed/victim"), 0), -crate::errno::EACCES as i64);
        assert_eq!(invoke(crate::vfs::LINUX_AT_FDCWD, &format!("{base}/allowed/missing"), 0), -crate::errno::ENOENT as i64);
        std::fs::remove_dir_all(&host).unwrap();
    }
    #[test]
    fn unlink_sticky_checks_symlink_owner_and_rmdir() {
        use std::ffi::CString;
        let (_guard, view) = crate::vfs::test_view(); let base = format!("/data/unlink-sticky-{}", std::process::id()); let host = view.join("data").join(base.trim_start_matches("/data/")); std::fs::create_dir_all(&host).unwrap();
        let record = |suffix: &str, uid, mode| {let path = CString::new(host.join(suffix).as_os_str().as_encoded_bytes()).unwrap(); super::super::attrs::record(super::super::attrs::Host::Path(&path), || if suffix.is_empty() {base.clone()} else {format!("{base}/{suffix}")}, super::super::attrs::Attr {uid: Some(uid), gid: Some(2000), mode: Some(mode)});};
        record("", 1000, 0o1777); std::fs::write(host.join("target"), b"owned").unwrap(); record("target", 3000, 0o600); std::os::unix::fs::symlink("target", host.join("link")).unwrap(); record("link", 2000, 0o777);
        let mut id = super::super::cred::Identity::default(); id.uid = [3000; 4]; id.gid = [3000; 4]; id.cap_eff = 0;
        let link = CString::new(format!("{base}/link")).unwrap(); let args = [crate::vfs::LINUX_AT_FDCWD as u64, link.as_ptr() as u64, 0, 0, 0, 0];
        assert_eq!(unlinkat_as(args, &id), -EPERM); assert!(host.join("link").symlink_metadata().is_ok()); id.uid[3] = 2000; assert_eq!(unlinkat_as(args, &id), 0); assert!(host.join("target").is_file());
        std::fs::create_dir(host.join("dir")).unwrap(); record("dir", 4000, 0o700); let dir = CString::new(format!("{base}/dir")).unwrap(); let args = [crate::vfs::LINUX_AT_FDCWD as u64, dir.as_ptr() as u64, AT_REMOVEDIR, 0, 0, 0];
        assert_eq!(unlinkat_as(args, &id), -EPERM); id.cap_eff = 1 << 3; assert_eq!(unlinkat_as(args, &id), 0);
        std::fs::remove_dir_all(&host).unwrap();
    }
    #[test]
    fn sticky_directory_requires_owner_or_fowner_capability() {
        let mut parent: libc::stat = unsafe {std::mem::zeroed()}; parent.st_mode = libc::S_IFDIR | 0o1777; parent.st_uid = 1000;
        let mut target: libc::stat = unsafe {std::mem::zeroed()}; target.st_uid = 2000;
        let mut id = super::super::cred::Identity::default(); id.cap_eff = 0; id.uid = [3000; 4];
        assert!(!sticky_permits(&parent, &target, &id)); id.uid[3] = 1000; assert!(sticky_permits(&parent, &target, &id));
        id.uid[3] = 2000; assert!(sticky_permits(&parent, &target, &id)); id.uid[3] = 3000;
        id.cap_eff = 1 << 1; assert!(!sticky_permits(&parent, &target, &id)); id.cap_eff = 1 << 3; assert!(sticky_permits(&parent, &target, &id));
    }
    #[test]
    fn directory_dac_uses_fs_ids_groups_and_capabilities() {
        let mut st: libc::stat = unsafe {std::mem::zeroed()}; st.st_mode = libc::S_IFDIR | 0o750; st.st_uid = 1000; st.st_gid = 2000;
        let mut id = super::super::cred::Identity::default(); id.uid = [1001; 4]; id.gid = [2001; 4]; id.cap_eff = 0;
        assert!(!directory_permits(&st, 3, &id)); id.groups.push(2000); assert!(directory_permits(&st, 1, &id)); assert!(!directory_permits(&st, 3, &id));
        id.uid[3] = 1000; assert!(directory_permits(&st, 3, &id));
        id.uid[3] = 0; st.st_mode = libc::S_IFDIR; assert!(!directory_permits(&st, 3, &id));
        id.cap_eff = 1 << 2; assert!(directory_permits(&st, 1, &id)); assert!(!directory_permits(&st, 3, &id));
        id.cap_eff = 1 << 1; assert!(directory_permits(&st, 3, &id));
    }
}
