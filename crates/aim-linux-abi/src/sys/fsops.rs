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
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const AT_REMOVEDIR: u64 = 0x200;
const AT_SYMLINK_FOLLOW: u64 = 0x400;
const EPERM: i64 = 1;
const EXDEV: i64 = 18;
const EISDIR: i64 = 21;
const ENOTEMPTY: i64 = 39;
const EOPNOTSUPP: i64 = 95;
const ENODATA: i64 = 61;

fn resolve_w(dirfd: u64, path: u64, follow: bool) -> Result<Resolved, i64> {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(path) };
    if !p.starts_with(b"/")&&super::fdtab::is_hidden(dirfd as i32){return Err(-(crate::errno::EBADF as i64));}
    let r = vfs::resolve(dirfd as i32, p, follow).map_err(|e| -(e as i64))?;
    check_writable(&r)?;
    Ok(r)
}

pub fn mkdirat(a: [u64; 6]) -> i64 {
    match resolve_w(a[0], a[1], false) {
        Ok(r) => {
            if let Some(route)=vfs::fuse_route(&r.guest){return super::fuse_client::mkdir(&route,a[2] as u32).map(|_|0).unwrap_or_else(|error|-(error as i64));}
            // The owner keeps access on the host (see host_mode), and
            // macOS drops S_ISVTX; the guest's mode is recorded when the
            // host's differs.
            let want = a[2] as u32 & 0o7777;
            let mode = host_mode(want, libc::S_IFDIR);
            // SAFETY: host path.
            let res = errno::check(unsafe { libc::mkdir(r.host.as_ptr(), mode) } as i64);
            if res == 0 {
                let host = Host::Path(&r.host);
                if let Err(error) = attrs::created(host, || r.guest.clone()) { return -(error as i64); }
                if attrs::recording() {
                    let mut st: libc::stat = unsafe { std::mem::zeroed() };
                    // SAFETY: host path, local buffer.
                    unsafe { libc::stat(r.host.as_ptr(), &mut st) };
                    attrs::apply(host, || r.guest.clone(), &mut st);
                    let made = st.st_mode as u32 & 0o7777;
                    let guest = (made & (0o077 | libc::S_ISGID as u32)) | (want & 0o700) | (want & libc::S_ISVTX as u32);
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
    if let Some(route)=vfs::fuse_route(&r.guest){return super::fuse_client::mknod(&route,mode,a[3] as u32).map(|_|0).unwrap_or_else(|error|-(error as i64));}
    let res = mknod_host(&r, mode);
    if res == 0 {
        if let Err(error) = attrs::created(Host::Path(&r.host), || r.guest.clone()) { return -(error as i64); }
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
    let target = super::fs::stat_at(
        crate::vfs::LINUX_AT_FDCWD,
        r.guest.as_bytes(),
        AT_SYMLINK_NOFOLLOW,
    )?;
    if !directory_permits(&directory, 3, id) {
        return Err(-crate::errno::EACCES as i64);
    }
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
    if !path.starts_with(b"/")&&super::fdtab::is_hidden(a[0] as i32){return -(crate::errno::EBADF as i64);}

    if let Ok(resolved)=vfs::resolve(a[0] as i32,path,false){if let Some(route)=vfs::fuse_route(&resolved.guest){return super::fuse_client::unlink(&route,a[2]&AT_REMOVEDIR!=0).map(|_|0).unwrap_or_else(|error|-(error as i64));}}
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
            if let Some(route)=vfs::fuse_route(&r.guest){return super::fuse_client::symlink(&route,target).map(|_|0).unwrap_or_else(|error|-(error as i64));}
            // SAFETY: host path.
            let res = errno::check(unsafe { libc::symlink(t.as_ptr(), r.host.as_ptr()) } as i64);
            if res == 0 {
                if let Err(error) = attrs::created(Host::Path(&r.host), || r.guest.clone()) { return -(error as i64); }
            }
            res
        }
        Err(e) => e,
    }
}

pub fn linkat(a: [u64; 6]) -> i64 {
    if a[4]&AT_EMPTY_PATH!=0&&unsafe{guest_cstr(a[1])}.is_empty()&&super::fdtab::is_hidden(a[0] as i32){return -(crate::errno::EBADF as i64);}
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
    let old_route=old.as_ref().and_then(|old|vfs::fuse_route(&old.guest));let new_route=vfs::fuse_route(&new.guest);
    match(old_route,new_route){(Some(old),Some(new))=>return super::fuse_client::link(&old,&new).map(|_|0).unwrap_or_else(|error|-(error as i64)),(Some(_),None)|(None,Some(_))=>return -EXDEV,_=>{}}
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
    match(vfs::fuse_route(&old.guest),vfs::fuse_route(&new.guest)){
        (Some(old),Some(new))=>return super::fuse_client::rename(&old,&new,flags as u32).map(|_|0).unwrap_or_else(|error|-(error as i64)),
        (Some(_),None)|(None,Some(_))=>return -EXDEV,_=>{}
    }
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

fn resolve_metadata(
    dirfd: i32,
    path: &[u8],
    follow: bool,
    id: &super::cred::Identity,
) -> Result<Resolved, i64> {
    if !path.starts_with(b"/") && super::fdtab::is_hidden(dirfd) {
        return Err(-(crate::errno::EBADF as i64));
    }
    let resolved = vfs::resolve_checked(dirfd, path, follow, |directory| {
        attrs::search(directory, id, attrs::FS)
    })
    .map_err(|error| -(error as i64))?;
    check_writable(&resolved)?;
    Ok(resolved)
}
struct MetadataFd(Option<OwnedFd>);
impl AsRawFd for MetadataFd {
    fn as_raw_fd(&self) -> i32 {
        self.0.as_ref().unwrap().as_raw_fd()
    }
}
impl Drop for MetadataFd {
    fn drop(&mut self) {
        if let Some(fd) = self.0.take() {
            let raw = fd.as_raw_fd();
            drop(fd);
            super::fdtab::unhide(raw);
        }
    }
}
fn private_metadata_fd(fd: i32) -> MetadataFd {
    MetadataFd(Some(unsafe {
        OwnedFd::from_raw_fd(super::fdtab::hide(fd))
    }))
}
fn metadata_fd_copy(fd: i32) -> Result<MetadataFd, i64> {
    let _creating = super::fork::spawn::own_fds();
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if copy < 0 {
        return Err(-(errno::last() as i64));
    }
    Ok(private_metadata_fd(copy))
}
fn inode_mutation(
    host: Host,
    guest: &str,
    follow: bool,
    change: impl FnOnce(&libc::stat) -> i64,
) -> i64 {
    let initial = match metadata_stat(host, guest, follow) {
        Ok(stat) => stat,
        Err(error) => return error,
    };
    attrs::with_inode_lock(host, &initial, || {
        let stat = metadata_stat(host, guest, follow).map_err(|error| (-error) as i32)?;
        let result = change(&stat);
        if result < 0 {
            Err((-result) as i32)
        } else {
            Ok(result)
        }
    })
    .unwrap_or_else(|error| -(error as i64))
}
fn metadata_anchor(resolved: &Resolved, follow: bool, write: bool) -> Result<MetadataFd, i64> {
    let _creating = super::fork::spawn::own_fds();
    // O_EVTONLY does not require content-read access. O_NOFOLLOW prevents a
    // swapped final symlink from redirecting the authenticated inode change.
    let flags = libc::O_EVTONLY
        | libc::O_NONBLOCK
        | libc::O_CLOEXEC
        | if follow {
            libc::O_NOFOLLOW
        } else {
            libc::O_SYMLINK
        };
    let fd = unsafe { libc::open(resolved.host.as_ptr(), flags) };
    if fd < 0 {
        return Err(-(errno::last() as i64));
    }
    let anchor = private_metadata_fd(fd);
    if !write {
        return Ok(anchor);
    }
    let mut first: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(anchor.as_raw_fd(), &mut first) } < 0 {
        return Err(-(errno::last() as i64));
    }
    if first.st_mode & libc::S_IFMT == libc::S_IFDIR {
        return Err(-EISDIR);
    }
    if first.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(-(EINVAL as i64));
    }
    let writable = unsafe {
        libc::open(
            resolved.host.as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if writable < 0 {
        return Err(-(errno::last() as i64));
    }
    let writable = private_metadata_fd(writable);
    let mut second: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(writable.as_raw_fd(), &mut second) } < 0 {
        return Err(-(errno::last() as i64));
    }
    if first.st_dev != second.st_dev || first.st_ino != second.st_ino {
        return Err(-(crate::errno::EAGAIN as i64));
    }
    Ok(writable)
}
fn metadata_stat(host: Host, guest: &str, follow: bool) -> Result<libc::stat, i64> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe {
        match host {
            Host::Path(path) | Host::Image(path) => {
                if follow {
                    libc::stat(path.as_ptr(), &mut stat)
                } else {
                    libc::lstat(path.as_ptr(), &mut stat)
                }
            }
            Host::Fd(fd) => libc::fstat(fd, &mut stat),
        }
    };
    if result < 0 {
        return Err(-(errno::last() as i64));
    }
    attrs::apply(host, || guest.to_owned(), &mut stat);
    Ok(stat)
}
fn fd_guest(fd: i32) -> Option<String> {
    procfs::fd_guest_path(fd).ok()
}
fn writable_fd_metadata(fd: i32, stat: &libc::stat) -> Result<(), i64> {
    if vfs::on_read_only_root(stat.st_dev) {
        return Err(-(crate::errno::EROFS as i64));
    }
    // Anonymous memory has an actual writable inode/seal owner. F_GETPATH's
    // diagnostic host pathname is not a guest mount identity.
    if memfd::key(fd).is_some() { return Ok(()); }
    let guest = crate::xrt::original_guest_path(fd).or_else(|| {
        let mut path=[0u8;libc::PATH_MAX as usize];
        if unsafe{libc::fcntl(fd,libc::F_GETPATH,path.as_mut_ptr())}<0 {return None;}
        let length=path.iter().position(|byte|*byte==0)?;
        use std::os::unix::ffi::OsStrExt;
        vfs::guest_path_of_host(std::path::Path::new(std::ffi::OsStr::from_bytes(&path[..length])))
    });
    if attrs::recording() && guest.is_some_and(|path|vfs::lookup(&path).1==vfs::Area::Image) {
        return Err(-(crate::errno::EROFS as i64));
    }
    Ok(())
}
fn chmod_mode(stat: &libc::stat, mode: u32, id: &super::cred::Identity) -> Result<u32, i64> {
    if stat.st_uid != id.uid[attrs::FS] && id.cap_eff & (1 << 3) == 0 {
        return Err(-EPERM);
    }
    let group = stat.st_gid == id.gid[attrs::FS] || id.groups.contains(&stat.st_gid);
    Ok(if !group && id.cap_eff & (1 << 4) == 0 {
        mode & !(libc::S_ISGID as u32)
    } else {
        mode
    })
}
fn chown_change(
    stat: &libc::stat,
    uid: u64,
    gid: u64,
    id: &super::cred::Identity,
) -> Result<Attr, i64> {
    let mut change = owner(uid, gid);
    let capable = id.cap_eff & 1 != 0;
    let own = stat.st_uid == id.uid[attrs::FS];
    if change
        .uid
        .is_some_and(|uid| (!own || uid != stat.st_uid) && !capable)
    {
        return Err(-EPERM);
    }
    if change.gid.is_some_and(|gid| {
        (!own || (gid != stat.st_gid && gid != id.gid[attrs::FS] && !id.groups.contains(&gid)))
            && !capable
    }) {
        return Err(-EPERM);
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        // chown clears execution privilege even for root; mandatory-locking
        // setgid (without group execute) remains, as in Linux chown_common.
        let old = stat.st_mode as u32 & 0o7777;
        let group = stat.st_gid == id.gid[attrs::FS] || id.groups.contains(&stat.st_gid);
        let mode = old
            & !(libc::S_ISUID as u32)
            & if old & 0o010 != 0 || !group && id.cap_eff & (1 << 4) == 0 {
                !(libc::S_ISGID as u32)
            } else {
                u32::MAX
            };
        if mode != old {
            change.mode = Some(mode);
        }
    }
    Ok(change)
}
fn kill_file_capabilities(host: Host, stat: &libc::stat) -> Result<(), i64> {
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Ok(());
    }
    let name = c"dev.aim.xattr.security.capability";
    let size = unsafe { match host {
        Host::Path(path) => libc::getxattr(path.as_ptr(),name.as_ptr(),std::ptr::null_mut(),0,0,libc::XATTR_NOFOLLOW),
        Host::Fd(fd) => libc::fgetxattr(fd,name.as_ptr(),std::ptr::null_mut(),0,0,0),
        Host::Image(_) => return Err(-(crate::errno::EROFS as i64)),
    }};
    if size<=0 {
        if size==0{return Ok(());}
        let error=errno::last();
        return if error as i64==ENODATA||error==crate::errno::EOPNOTSUPP{Ok(())}else{Err(-(error as i64))};
    }
    let result = unsafe {
        match host {
            Host::Path(path) => {
                libc::removexattr(path.as_ptr(), name.as_ptr(), libc::XATTR_NOFOLLOW)
            }
            Host::Fd(fd) => libc::fremovexattr(fd, name.as_ptr(), 0),
            Host::Image(_) => return Err(-(crate::errno::EROFS as i64)),
        }
    };
    if result < 0 {
        let error = errno::last();
        if error as i64 != ENODATA && error != crate::errno::EOPNOTSUPP {
            return Err(-(error as i64));
        }
    }
    Ok(())
}
fn owner(uid: u64, gid: u64) -> Attr {
    let id = |value: u64| (value as u32 != u32::MAX).then_some(value as u32);
    Attr {
        uid: id(uid),
        gid: id(gid),
        mode: None,
    }
}
pub fn fchmodat(a: [u64; 6]) -> i64 {
    fchmodat_as(a, &super::cred::current())
}
fn fchmodat_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    let path = unsafe { guest_cstr(a[1]) };
    let resolved = match resolve_metadata(a[0] as i32, path, true, id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(route) = vfs::fuse_route(&resolved.guest) {
        let mut mode = a[2] as u32 & 0o7777;
        if route.default_permissions {
            let stat = match super::fuse_client::stat(&route, None, None) {
                Ok(stat) => stat,
                Err(error) => return -(error as i64),
            };
            mode = match chmod_mode(&stat, mode, id) {
                Ok(mode) => mode,
                Err(error) => return error,
            };
        }
        return super::fuse_client::setattr(&route, None, None, Some(mode), None, None, None)
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64));
    }
    let anchor = match metadata_anchor(&resolved, true, false) {
        Ok(fd) => fd,
        Err(error) => return error,
    };
    let host = Host::Fd(anchor.as_raw_fd());
    inode_mutation(host, &resolved.guest, true, |stat| {
        let mode = match chmod_mode(&stat, a[2] as u32 & 0o7777, id) {
            Ok(mode) => mode,
            Err(error) => return error,
        };
        if unsafe { libc::fchmod(anchor.as_raw_fd(), host_mode(mode, stat.st_mode)) } < 0 {
            return -(errno::last() as i64);
        }
        attrs::record_checked(
            host,
            || resolved.guest.clone(),
            Attr {
                mode: Some(mode),
                ..Default::default()
            },
        )
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64))
    })
}
pub fn fchmod(a: [u64; 6]) -> i64 {
    fchmod_as(a, &super::cred::current())
}
fn fchmod_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    let fd = a[0] as i32;
    if super::fdtab::is_hidden(fd) {
        return -(crate::errno::EBADF as i64);
    }
    if super::fs::is_path_fd(fd) {
        return -(crate::errno::EBADF as i64);
    }
    if let Some(file) = super::fuse_client::get(fd) {
        let mut mode = a[1] as u32 & 0o7777;
        if file.route.default_permissions {
            let stat = match super::fuse_client::stat(&file.route, Some(file.node), Some(file.fh)) {
                Ok(stat) => stat,
                Err(error) => return -(error as i64),
            };
            mode = match chmod_mode(&stat, mode, id) {
                Ok(mode) => mode,
                Err(error) => return error,
            };
        }
        return super::fuse_client::setattr(
            &file.route,
            Some(file.node),
            Some(file.fh),
            Some(mode),
            None,
            None,
            None,
        )
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64));
    }
    let guest = fd_guest(fd).unwrap_or_default();
    let original = fd;
    let anchor = match metadata_fd_copy(fd) {
        Ok(fd) => fd,
        Err(error) => return error,
    };
    let fd = anchor.as_raw_fd();
    inode_mutation(Host::Fd(fd), &guest, true, |stat| {
        if let Err(error) = writable_fd_metadata(original, &stat) {
            return error;
        }
        let mode = match chmod_mode(&stat, a[1] as u32 & 0o7777, id) {
            Ok(mode) => mode,
            Err(error) => return error,
        };
        if unsafe { libc::fchmod(fd, host_mode(mode, stat.st_mode)) } < 0 {
            return -(errno::last() as i64);
        }
        attrs::record_checked(
            Host::Fd(fd),
            || guest.clone(),
            Attr {
                mode: Some(mode),
                ..Default::default()
            },
        )
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64))
    })
}
pub fn fchownat(a: [u64; 6]) -> i64 {
    fchownat_as(a, &super::cred::current())
}
fn fchownat_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    let (uid, gid, flags) = (a[2], a[3], a[4]);
    if flags & !(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW) != 0 {
        return -(EINVAL as i64);
    }
    let path = unsafe { guest_cstr(a[1]) };
    if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        if a[0] as i32 == vfs::LINUX_AT_FDCWD {
            return fchownat_as(
                [
                    a[0],
                    c".".as_ptr() as u64,
                    uid,
                    gid,
                    flags & AT_SYMLINK_NOFOLLOW,
                    0,
                ],
                id,
            );
        }
        return fchown_fd_as([a[0], uid, gid, 0, 0, 0], id, true);
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let resolved = match resolve_metadata(a[0] as i32, path, follow, id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(route) = vfs::fuse_route(&resolved.guest) {
        let mut change = owner(uid, gid);
        if route.default_permissions {
            let stat = match super::fuse_client::stat(&route, None, None) {
                Ok(stat) => stat,
                Err(error) => return -(error as i64),
            };
            change = match chown_change(&stat, uid, gid, id) {
                Ok(change) => change,
                Err(error) => return error,
            };
        }
        return super::fuse_client::setattr(
            &route,
            None,
            None,
            change.mode,
            change.uid,
            change.gid,
            None,
        )
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64));
    }
    let anchor = match metadata_anchor(&resolved, follow, false) {
        Ok(fd) => fd,
        Err(error) => return error,
    };
    let host = Host::Fd(anchor.as_raw_fd());
    inode_mutation(host, &resolved.guest, follow, |stat| {
        let change = match chown_change(&stat, uid, gid, id) {
            Ok(change) => change,
            Err(error) => return error,
        };
        if !attrs::recording() {
            return errno::check(
                unsafe { libc::fchown(anchor.as_raw_fd(), uid as u32, gid as u32) } as i64,
            );
        }
        if let Err(error) = kill_file_capabilities(host, &stat) {
            return error;
        }
        attrs::record_checked(host, || resolved.guest.clone(), change)
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64))
    })
}
pub fn fchown(a: [u64; 6]) -> i64 {
    fchown_as(a, &super::cred::current())
}
fn fchown_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    fchown_fd_as(a, id, false)
}
fn fchown_fd_as(a: [u64; 6], id: &super::cred::Identity, allow_path: bool) -> i64 {
    let fd = a[0] as i32;
    if super::fdtab::is_hidden(fd) {
        return -(crate::errno::EBADF as i64);
    }
    if !allow_path && super::fs::is_path_fd(fd) {
        return -(crate::errno::EBADF as i64);
    }
    if let Some(file) = super::fuse_client::get(fd) {
        let mut change = owner(a[1], a[2]);
        if file.route.default_permissions {
            let stat = match super::fuse_client::stat(&file.route, Some(file.node), Some(file.fh)) {
                Ok(stat) => stat,
                Err(error) => return -(error as i64),
            };
            change = match chown_change(&stat, a[1], a[2], id) {
                Ok(change) => change,
                Err(error) => return error,
            };
        }
        return super::fuse_client::setattr(
            &file.route,
            Some(file.node),
            Some(file.fh),
            change.mode,
            change.uid,
            change.gid,
            None,
        )
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64));
    }
    let guest = fd_guest(fd).unwrap_or_default();
    let original = fd;
    let anchor = match metadata_fd_copy(fd) {
        Ok(fd) => fd,
        Err(error) => return error,
    };
    let fd = anchor.as_raw_fd();
    inode_mutation(Host::Fd(fd), &guest, true, |stat| {
        if let Err(error) = writable_fd_metadata(original, &stat) {
            return error;
        }
        let change = match chown_change(&stat, a[1], a[2], id) {
            Ok(change) => change,
            Err(error) => return error,
        };
        if !attrs::recording() {
            return errno::check(unsafe { libc::fchown(fd, a[1] as u32, a[2] as u32) } as i64);
        }
        if let Err(error) = kill_file_capabilities(Host::Fd(fd), &stat) {
            return error;
        }
        attrs::record_checked(Host::Fd(fd), || guest.clone(), change)
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64))
    })
}
pub fn truncate(a: [u64; 6]) -> i64 {
    truncate_as(a, &super::cred::current())
}
fn truncate_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    if (a[1] as i64) < 0 {
        return -(EINVAL as i64);
    }
    let path = unsafe { guest_cstr(a[0]) };
    let resolved = match resolve_metadata(vfs::LINUX_AT_FDCWD, path, true, id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(route) = vfs::fuse_route(&resolved.guest) {
        if route.default_permissions {
            let stat = match super::fuse_client::stat(&route, None, None) {
                Ok(stat) => stat,
                Err(error) => return -(error as i64),
            };
            if !attrs::permits(&stat, 2, id, attrs::FS) {
                return -(crate::errno::EACCES as i64);
            }
        }
        return super::fuse_client::setattr(&route, None, None, None, None, None, Some(a[1]))
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64));
    }
    let anchor = match metadata_anchor(&resolved, true, true) {
        Ok(fd) => fd,
        Err(error) => return error,
    };
    let host = Host::Fd(anchor.as_raw_fd());
    inode_mutation(host, &resolved.guest, true, |stat| {
        if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            return -EISDIR;
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return -(EINVAL as i64);
        }
        if !attrs::permits(&stat, 2, id, attrs::FS) {
            return -(crate::errno::EACCES as i64);
        }
        let result =
            errno::check(unsafe { libc::ftruncate(anchor.as_raw_fd(), a[1] as i64) } as i64);
        if result == 0 {
            if let Err(error) = kill_file_capabilities(host, &stat) {
                return error;
            }
        }
        if result == 0 && id.cap_eff & (1 << 4) == 0 {
            let mut mode = stat.st_mode as u32 & 0o7777;
            mode &= !(libc::S_ISUID as u32);
            if mode & 0o010 != 0
                || stat.st_gid != id.gid[attrs::FS] && !id.groups.contains(&stat.st_gid)
            {
                mode &= !(libc::S_ISGID as u32);
            }
            if let Err(error) = attrs::record_checked(
                host,
                || resolved.guest.clone(),
                Attr {
                    mode: Some(mode),
                    ..Default::default()
                },
            ) {
                return -(error as i64);
            }
        }
        result
    })
}
pub fn ftruncate(a: [u64; 6]) -> i64 {
    ftruncate_as(a, &super::cred::current())
}
fn ftruncate_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    let (fd, len) = (a[0] as i32, a[1] as i64);
    if super::fdtab::is_hidden(fd) {
        return -(crate::errno::EBADF as i64);
    }
    if super::fs::is_path_fd(fd) {
        return -(crate::errno::EBADF as i64);
    }
    if len < 0 {
        return -(EINVAL as i64);
    }
    if let Some(file) = super::fuse_client::get(fd) {
        if file.flags & 3 == 0 {
            return -(EINVAL as i64);
        }
        return super::fuse_client::set_size(&file, a[1])
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64));
    }
    let guest = fd_guest(fd).unwrap_or_default();
    let original = fd;
    let anchor = match metadata_fd_copy(fd) {
        Ok(fd) => fd,
        Err(error) => return error,
    };
    let fd = anchor.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return -(errno::last() as i64);
    }
    if flags & libc::O_ACCMODE == libc::O_RDONLY {
        return -(EINVAL as i64);
    }
    inode_mutation(Host::Fd(fd), &guest, true, |stat| {
        if let Err(error) = writable_fd_metadata(original, &stat) {
            return error;
        }
        if memfd::resize_sealed(fd, len as u64) {
            return -EPERM;
        }
        let result = errno::check(unsafe { libc::ftruncate(fd, len) } as i64);
        if result == 0 {
            if let Err(error) = kill_file_capabilities(Host::Fd(fd), &stat) {
                return error;
            }
        }
        if result == 0 && id.cap_eff & (1 << 4) == 0 {
            let mut mode = stat.st_mode as u32 & 0o7777;
            mode &= !(libc::S_ISUID as u32);
            if mode & 0o010 != 0
                || stat.st_gid != id.gid[attrs::FS] && !id.groups.contains(&stat.st_gid)
            {
                mode &= !(libc::S_ISGID as u32);
            }
            if let Err(error) = attrs::record_checked(
                Host::Fd(fd),
                || guest.clone(),
                Attr {
                    mode: Some(mode),
                    ..Default::default()
                },
            ) {
                return -(error as i64);
            }
        }
        result
    })
}

const FALLOC_FL_KEEP_SIZE: u64 = 1;
const FALLOC_FL_PUNCH_HOLE: u64 = 2;

pub fn fallocate(a: [u64; 6]) -> i64 {
    if super::fdtab::is_hidden(a[0] as i32){return -(crate::errno::EBADF as i64);}
    let (fd, mode, off, len) = (a[0] as i32, a[1], a[2] as i64, a[3] as i64);
    if let Some(file)=super::fuse_client::get(fd){if off<0||len<=0{return -(EINVAL as i64);}return super::fuse_client::fallocate(&file,off as u64,len as u64,mode as u32).map(|_|0).unwrap_or_else(|error|-(error as i64));}
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
    if super::fdtab::is_hidden(a[0] as i32){return -(crate::errno::EBADF as i64);}
    if super::fs::is_path_fd(a[0] as i32) { return -(crate::errno::EBADF as i64); }
    if super::fuse_device::is_device(a[0] as i32){return -(EINVAL as i64);}
    if let Some(file)=super::fuse_client::get(a[0] as i32){if let Err(error)=super::fuse_cache::flush_file(&file){return -(error as i64);}return file.fsync(false).map(|_|0).unwrap_or_else(|error|-(error as i64));}
    if super::proxy_file::is_proxy(a[0] as i32) {
        return super::proxy_file::sync(a[0] as i32);
    }
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
    if super::fdtab::is_hidden(a[0] as i32){return -(crate::errno::EBADF as i64);}
    if super::fs::is_path_fd(a[0] as i32) { return -(crate::errno::EBADF as i64); }
    if a[3] & !7 != 0 || (a[1] as i64) < 0 || (a[2] as i64) < 0 {
        return -(EINVAL as i64);
    }
    fsync(a)
}

pub fn flock(a: [u64; 6]) -> i64 {
    if super::fdtab::is_hidden(a[0] as i32){return -(crate::errno::EBADF as i64);}
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
    if (path==0||unsafe{guest_cstr(path)}.is_empty()||!unsafe{guest_cstr(path)}.starts_with(b"/"))&&super::fdtab::is_hidden(dirfd){return -(crate::errno::EBADF as i64);}
    // SAFETY: guest path pointer (NULL: the fd itself).
    if path == 0 || (unsafe { guest_cstr(path) }.is_empty() && flags & AT_EMPTY_PATH != 0) {
        if path==0&&super::fs::is_path_fd(dirfd){return -(crate::errno::EBADF as i64);}
        if let Some(file)=super::fuse_client::get(dirfd){return super::fuse_client::times(&file.route,Some(file.node),Some(file.fh),&ts).map(|_|0).unwrap_or_else(|error|-(error as i64));}
        // SAFETY: futimens with local timespecs.
        return errno::check(unsafe { libc::futimens(dirfd, ts.as_ptr()) } as i64);
    }
    let r = match resolve_w(dirfd as u64, path, flags & AT_SYMLINK_NOFOLLOW == 0) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if let Some(route)=vfs::fuse_route(&r.guest){return super::fuse_client::times(&route,None,None,&ts).map(|_|0).unwrap_or_else(|error|-(error as i64));}
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
    fn mapped_metadata_memfd_and_unmapped_fd_resize_real_shared_memory() {
        let(_guard,_view)=crate::vfs::test_view();assert!(attrs::recording());
        let fd=memfd::memfd_create([c"metadata-anonymous".as_ptr() as u64,3,0,0,0,0]);assert!(fd>=0);assert!(memfd::key(fd as i32).is_some());
        assert_eq!(ftruncate([fd as u64,16384,0,0,0,0]),0);
        let address=super::super::mem::mmap([0,16384,3,1,fd as u64,0]);assert!(address>0,"actual shared mmap: {address}");
        unsafe{(address as *mut u8).write(0x5a)};
        let mut byte=0u8;assert_eq!(unsafe{libc::pread(fd as i32,(&mut byte as *mut u8).cast(),1,0)},1);assert_eq!(byte,0x5a);
        let context:crate::context::GuestContext=unsafe{std::mem::zeroed()};
        assert_eq!(super::super::mem::munmap(&context,[address as u64,16384,0,0,0,0]),0);
        assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
        let path=std::env::temp_dir().join(format!("aim-unmapped-metadata-{}",std::process::id()));
        let ordinary=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        use std::os::fd::AsRawFd;assert!(vfs::guest_path_of_host(&path).is_none());
        assert_eq!(ftruncate([ordinary.as_raw_fd() as u64,37,0,0,0,0]),0);assert_eq!(ordinary.metadata().unwrap().len(),37);
        drop(ordinary);std::fs::remove_file(path).unwrap();
        let image_path=_view.join("root/system/metadata-readonly-fixture");std::fs::create_dir_all(image_path.parent().unwrap()).unwrap();
        let image=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&image_path).unwrap();
        assert_eq!(vfs::lookup(&vfs::guest_path_of_host(&image_path.canonicalize().unwrap()).unwrap()).1,vfs::Area::Image);
        assert_eq!(ftruncate([image.as_raw_fd() as u64,37,0,0,0,0]),-(crate::errno::EROFS as i64));
        assert_eq!(image.metadata().unwrap().len(),0);drop(image);std::fs::remove_file(image_path).unwrap();
    }

    #[test]
    fn metadata_anchor_preserves_inode_after_path_swap_and_hides_internal_fds() {
        let(_guard,view)=crate::vfs::test_view();let base=format!("/data/metadata-anchor-{}",std::process::id());
        let directory=view.join("data").join(base.trim_start_matches("/data/"));std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("owned"),b"owned").unwrap();std::fs::write(directory.join("foreign"),b"foreign").unwrap();
        let record=|name:&str,uid|{let path=c(directory.join(name).to_str().unwrap());attrs::record_checked(Host::Path(&path),||format!("{base}/{name}"),Attr{uid:Some(uid),gid:Some(uid),mode:Some(0o600)}).unwrap();};
        record("",0);let path=c(directory.to_str().unwrap());attrs::record_checked(Host::Path(&path),||base.clone(),Attr{mode:Some(0o777),..Default::default()}).unwrap();record("owned",2000);record("foreign",5000);
        let mut id=super::super::cred::Identity::default();id.uid=[2000;4];id.gid=[2000;4];id.cap_eff=0;
        let resolved=resolve_metadata(LINUX_AT_FDCWD,format!("{base}/owned").as_bytes(),true,&id).unwrap();
        let anchor=metadata_anchor(&resolved,true,false).unwrap();let raw=anchor.as_raw_fd();assert!(super::super::fdtab::is_hidden(raw));
        std::fs::rename(directory.join("owned"),directory.join("old-inode")).unwrap();std::fs::rename(directory.join("foreign"),directory.join("owned")).unwrap();
        let root=super::super::cred::Identity::default();
        assert_eq!(fchmod_as([raw as u64,0o777,0,0,0,0],&root),-(crate::errno::EBADF as i64));
        assert_eq!(fchown_as([raw as u64,0,0,0,0,0],&root),-(crate::errno::EBADF as i64));
        assert_eq!(ftruncate_as([raw as u64,0,0,0,0,0],&root),-(crate::errno::EBADF as i64));
        assert_eq!(fsync([raw as u64,0,0,0,0,0]),-(crate::errno::EBADF as i64));
        assert_eq!(fallocate([raw as u64,0,0,1,0,0]),-(crate::errno::EBADF as i64));
        assert_eq!(fchownat_as([raw as u64,c"".as_ptr() as u64,0,0,AT_EMPTY_PATH,0],&root),-(crate::errno::EBADF as i64));
        assert_eq!(fchmodat_as([raw as u64,c"entry".as_ptr() as u64,0o777,0,0,0],&root),-(crate::errno::EBADF as i64));
        let host=Host::Fd(raw);assert_eq!(inode_mutation(host,&resolved.guest,true,|stat|{
            let mode=match chmod_mode(stat,0o640,&id){Ok(mode)=>mode,Err(error)=>return error};
            if unsafe{libc::fchmod(raw,host_mode(mode,stat.st_mode))}<0{return -(errno::last() as i64);}
            attrs::record_checked(host,||resolved.guest.clone(),Attr{mode:Some(mode),..Default::default()}).map(|_|0).unwrap_or_else(|error|-(error as i64))
        }),0);
        assert_eq!(owner(&format!("{base}/old-inode")),(2000,2000,0o640));assert_eq!(owner(&format!("{base}/owned")),(5000,5000,0o600));
        drop(anchor);assert!(!super::super::fdtab::is_hidden(raw));std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn metadata_dac_checks_real_inode_owner_groups_caps_search_and_open_fd_rights() {
        use std::os::fd::AsRawFd;
        let(_guard,view)=crate::vfs::test_view();
        let base=format!("/data/metadata-dac-{}",std::process::id());let host=view.join("data").join(base.trim_start_matches("/data/"));
        std::fs::create_dir_all(host.join("private")).unwrap();std::fs::write(host.join("file"),b"0123456789").unwrap();std::fs::write(host.join("private/file"),b"private").unwrap();
        let record=|suffix:&str,uid,gid,mode|{
            let path=CString::new(host.join(suffix).as_os_str().as_encoded_bytes()).unwrap();
            attrs::record_checked(Host::Path(&path),||format!("{base}/{suffix}"),Attr{uid:Some(uid),gid:Some(gid),mode:Some(mode)}).unwrap();
        };
        record("",0,0,0o777);record("private",3000,3000,0o700);record("private/file",2000,2100,0o666);record("file",2000,2100,0o660);
        let mut owner_id=super::super::cred::Identity::default();owner_id.uid=[2000;4];owner_id.gid=[2000;4];owner_id.cap_eff=0;
        let mut foreign=owner_id.clone();foreign.uid=[4000;4];foreign.gid=[4000;4];
        let file=c(&format!("{base}/file"));let private=c(&format!("{base}/private/file"));
        let chmod=|path:&CString,mode,id:&super::super::cred::Identity|fchmodat_as([AT,path.as_ptr() as u64,mode,0,0,0],id);
        assert_eq!(chmod(&file,0o777,&foreign),-EPERM);assert_eq!(owner(&format!("{base}/file")),(2000,2100,0o660));
        let mut fs_owner=foreign.clone();fs_owner.uid[attrs::FS]=2000;
        assert_eq!(chmod(&file,0o660,&fs_owner),0);
        let mut root_without_caps=foreign.clone();root_without_caps.uid=[0;4];assert_eq!(chmod(&file,0o777,&root_without_caps),-EPERM);
        assert_eq!(chmod(&private,0o777,&owner_id),-(crate::errno::EACCES as i64));
        assert_eq!(chmod(&file,0o6760,&owner_id),0);assert_eq!(owner(&format!("{base}/file")).2,0o4760);
        owner_id.groups=vec![2100];assert_eq!(chmod(&file,0o2760,&owner_id),0);assert_eq!(owner(&format!("{base}/file")).2,0o2760);
        let mut privileged=foreign.clone();privileged.cap_eff=1<<3;assert_eq!(chmod(&file,0o660,&privileged),0);
        let chown=|uid,gid,id:&super::super::cred::Identity|fchownat_as([AT,file.as_ptr() as u64,uid,gid,0,0],id);
        assert_eq!(chown(4000,u32::MAX as u64,&owner_id),-EPERM);
        assert_eq!(chown(u32::MAX as u64,2300,&owner_id),-EPERM);
        owner_id.groups.push(2300);assert_eq!(chown(u32::MAX as u64,2300,&owner_id),0);assert_eq!(owner(&format!("{base}/file")).1,2300);
        assert_eq!(chown(2000,2300,&foreign),-EPERM);
        privileged.cap_eff=1;assert_eq!(chown(5000,5100,&privileged),0);assert_eq!(owner(&format!("{base}/file")).0,5000);
        assert_eq!(truncate_as([file.as_ptr() as u64,3,0,0,0,0],&foreign),-(crate::errno::EACCES as i64));assert_eq!(std::fs::metadata(host.join("file")).unwrap().len(),10);
        foreign.groups=vec![5100];assert_eq!(truncate_as([file.as_ptr() as u64,3,0,0,0,0],&foreign),0);
        let rw=std::fs::OpenOptions::new().read(true).write(true).open(host.join("file")).unwrap();let ro=std::fs::File::open(host.join("file")).unwrap();
        record("file",5000,5100,0o000);foreign.groups.clear();
        assert_eq!(fchmod_as([rw.as_raw_fd() as u64,0o777,0,0,0,0],&foreign),-EPERM);
        assert_eq!(fchown_as([rw.as_raw_fd() as u64,4000,u32::MAX as u64,0,0,0],&foreign),-EPERM);
        assert_eq!(truncate_as([file.as_ptr() as u64,2,0,0,0,0],&foreign),-(crate::errno::EACCES as i64));
        assert_eq!(ftruncate_as([rw.as_raw_fd() as u64,2,0,0,0,0],&foreign),0);assert_eq!(std::fs::metadata(host.join("file")).unwrap().len(),2);
        assert_eq!(ftruncate_as([ro.as_raw_fd() as u64,0,0,0,0,0],&foreign),-(EINVAL as i64));
        privileged.cap_eff=1<<3;assert_eq!(fchmod_as([ro.as_raw_fd() as u64,0o640,0,0,0,0],&privileged),0);
        let path_fd=openat([AT,file.as_ptr() as u64,0o10000000,0,0,0]);assert!(path_fd>=0);
        assert_eq!(fchown_as([path_fd as u64,5000,u32::MAX as u64,0,0,0],&privileged),-(crate::errno::EBADF as i64));
        let mut current_owner=owner_id.clone();current_owner.uid=[5000;4];
        assert_eq!(fchownat_as([path_fd as u64,c"".as_ptr() as u64,5000,u32::MAX as u64,AT_EMPTY_PATH,0],&current_owner),0);
        assert_eq!(super::super::fs::close([path_fd as u64,0,0,0,0,0]),0);
        let cap_name=c"dev.aim.xattr.security.capability";let caps=[0x01u8,0,0,0x02,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0];
        assert_eq!(unsafe{libc::fsetxattr(rw.as_raw_fd(),cap_name.as_ptr(),caps.as_ptr().cast(),caps.len(),0,0)},0);
        assert_eq!(fchown_as([rw.as_raw_fd() as u64,5000,u32::MAX as u64,0,0,0],&current_owner),0);
        assert_eq!(unsafe{libc::fgetxattr(rw.as_raw_fd(),cap_name.as_ptr(),std::ptr::null_mut(),0,0,0)},-1);
        assert_eq!(errno::last() as i64,ENODATA);
        record("file",5000,5100,0o6750);
        assert_eq!(unsafe{libc::fsetxattr(rw.as_raw_fd(),cap_name.as_ptr(),caps.as_ptr().cast(),caps.len(),0,0)},0);
        let mut keep_setid=foreign.clone();keep_setid.cap_eff=1<<4;
        assert_eq!(ftruncate_as([rw.as_raw_fd() as u64,1,0,0,0,0],&keep_setid),0);
        assert_eq!(owner(&format!("{base}/file")).2,0o6750);
        assert_eq!(unsafe{libc::fgetxattr(rw.as_raw_fd(),cap_name.as_ptr(),std::ptr::null_mut(),0,0,0)},-1);
        assert_eq!(errno::last() as i64,ENODATA);
        assert_eq!(fchmod_as([u64::MAX,0,0,0,0,0],&foreign),-(crate::errno::EBADF as i64));
        assert_eq!(fchownat_as([AT,file.as_ptr() as u64,0,0,0x8000,0],&foreign),-(EINVAL as i64));
        let mut root=super::super::cred::Identity::default();root.uid=[0;4];assert_eq!(chmod(&file,0o640,&root),0);
        drop(rw);drop(ro);std::fs::remove_dir_all(host).unwrap();
    }

    #[test]
    fn recorded_setgid_parent_survives_atomic_replacement_and_nested_creation() {
        let (_view, _) = vfs::test_view();
        let base = format!("/data/setgid-creation-{}", std::process::id());
        let parent = c(&base);
        assert_eq!(mkdirat([AT, parent.as_ptr() as u64, 0o750, 0, 0, 0]), 0);
        let (uid, gid) = attrs::ids(attrs::FS);
        let inherited = if gid == 2000 { 2001 } else { 2000 };
        chown(&base, uid, inherited);
        chmod(&base, 0o2750);
        let target = format!("{base}/atomic.state");
        let temporary = format!("{target}.new");
        for _ in 0..3 {
            create(&temporary);
            chmod(&temporary, 0o640);
            let source = c(&temporary);
            let destination = c(&target);
            assert_eq!(renameat2([AT, source.as_ptr() as u64, AT, destination.as_ptr() as u64, 0, 0]), 0);
            assert_eq!(owner(&target), (uid, inherited, 0o640));
        }
        // Reopening an existing inode must not reassign the creator's fs gid.
        chown(&target, uid, 2010);
        let destination = c(&target);
        let fd = openat([AT, destination.as_ptr() as u64, 0o1 | 0o100, 0o600, 0, 0]);
        assert!(fd >= 0); unsafe { libc::close(fd as i32); }
        assert_eq!(owner(&target).1, 2010);
        let nested = format!("{base}/nested");
        let child = c(&nested);
        assert_eq!(mkdirat([AT, child.as_ptr() as u64, 0o700, 0, 0, 0]), 0);
        assert_eq!(owner(&nested), (uid, inherited, 0o2700));
        create(&format!("{nested}/file"));
        assert_eq!(owner(&format!("{nested}/file")).1, inherited);
        // A directory fd and a followed symlink select the same real parent.
        let directory = openat([AT, parent.as_ptr() as u64, 0o200000, 0, 0, 0]);
        assert!(directory >= 0);
        let name = c("relative");
        let fd = openat([directory as u64, name.as_ptr() as u64, 0o1 | 0o100 | 0o200, 0o600, 0, 0]);
        assert!(fd >= 0); unsafe { libc::close(fd as i32); libc::close(directory as i32); }
        assert_eq!(owner(&format!("{base}/relative")).1, inherited);
        let alias = format!("{base}-symlink");
        let alias_name = c(&alias);
        assert_eq!(symlinkat([parent.as_ptr() as u64, AT, alias_name.as_ptr() as u64, 0, 0, 0]), 0);
        create(&format!("{alias}/through-link"));
        assert_eq!(owner(&format!("{base}/through-link")).1, inherited);
        // Non-setgid parent creation continues to use the creator's fs gid.
        chmod(&base, 0o750);
        create(&format!("{base}/ordinary"));
        assert_eq!(owner(&format!("{base}/ordinary")).1, gid);
        let resolved = vfs::resolve(LINUX_AT_FDCWD, base.as_bytes(), true).unwrap();
        let bind_alias = format!("{base}-bind");
        vfs::add_mount(&bind_alias, std::path::PathBuf::from(std::ffi::OsStr::from_bytes(resolved.host.as_bytes())),
            resolved.area, &base, "bind");
        chmod(&base, 0o2750);
        create(&format!("{bind_alias}/through-bind"));
        assert_eq!(owner(&format!("{base}/through-bind")).1, inherited);
        assert!(vfs::remove_mount(&bind_alias));
        let resolved_alias = vfs::resolve(LINUX_AT_FDCWD, alias.as_bytes(), false).unwrap();
        std::fs::remove_file(std::path::Path::new(std::ffi::OsStr::from_bytes(resolved_alias.host.as_bytes()))).unwrap();
        std::fs::remove_dir_all(std::path::Path::new(std::ffi::OsStr::from_bytes(resolved.host.as_bytes()))).unwrap();
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
        let (_guard, view) = crate::vfs::test_view();
        let base = format!("/data/remove-mount-{}", std::process::id());
        let host = view.join("data").join(base.trim_start_matches("/data/"));
        std::fs::create_dir_all(&host).unwrap();
        let backing = view.join("remove-mount-backing");
        std::fs::create_dir_all(&backing).unwrap();
        std::fs::write(backing.join("keep"), b"owned").unwrap();
        let file = view.join("remove-mount-file");
        std::fs::write(&file, b"owned").unwrap();
        let directory_guest = format!("{base}/directory");
        let file_guest = format!("{base}/file");
        crate::vfs::add_mount(
            &directory_guest,
            backing.clone(),
            crate::vfs::Area::Writable,
            "bind",
            "bind",
        );
        crate::vfs::add_mount(
            &file_guest,
            file.clone(),
            crate::vfs::Area::Writable,
            "bind",
            "bind",
        );
        let id = super::super::cred::Identity::default();
        let invoke = |path: &str, flags| {
            let path = CString::new(path).unwrap();
            unlinkat_as(
                [
                    crate::vfs::LINUX_AT_FDCWD as u64,
                    path.as_ptr() as u64,
                    flags,
                    0,
                    0,
                    0,
                ],
                &id,
            )
        };
        assert_eq!(
            invoke(&directory_guest, AT_REMOVEDIR),
            -crate::errno::EBUSY as i64
        );
        assert_eq!(invoke(&directory_guest, 0), -EISDIR);
        assert_eq!(invoke(&file_guest, 0), -crate::errno::EBUSY as i64);
        assert_eq!(
            invoke(&file_guest, AT_REMOVEDIR),
            -crate::errno::ENOTDIR as i64
        );
        assert_eq!(
            invoke("/data", AT_REMOVEDIR),
            -30,
            "readonly parent mount precedes busy target"
        );
        std::os::unix::fs::symlink(&directory_guest, host.join("link")).unwrap();
        assert_eq!(invoke(&format!("{base}/link"), 0), 0);
        assert!(backing.join("keep").is_file() && file.is_file());
        crate::vfs::add_mount(
            &file_guest,
            file.clone(),
            crate::vfs::Area::Image,
            "bind",
            "bind",
        );
        assert_eq!(invoke(&file_guest, 0), -crate::errno::EBUSY as i64);
        assert!(crate::vfs::remove_mount(&file_guest));
        crate::vfs::add_mount(
            &directory_guest,
            file.clone(),
            crate::vfs::Area::Writable,
            "bind",
            "bind",
        );
        assert_eq!(invoke(&directory_guest, 0), -crate::errno::EBUSY as i64);
        assert!(crate::vfs::remove_mount(&directory_guest));
        assert!(crate::vfs::is_mountpoint(&directory_guest));
        assert!(crate::vfs::remove_mount(&directory_guest));
        assert!(!crate::vfs::is_mountpoint(&directory_guest));
        assert!(crate::vfs::remove_mount(&file_guest));
        std::fs::remove_dir_all(backing).unwrap();
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir_all(host).unwrap();
    }
    #[test]
    fn removal_last_components_and_trailing_slashes_keep_linux_errors() {
        use std::ffi::CString;
        let (_guard, view) = crate::vfs::test_view();
        let base = format!("/data/unlink-last-{}", std::process::id());
        let host = view.join("data").join(base.trim_start_matches("/data/"));
        std::fs::create_dir_all(host.join("dir")).unwrap();
        std::fs::write(host.join("file"), b"owned").unwrap();
        std::os::unix::fs::symlink("dir", host.join("dirlink")).unwrap();
        std::os::unix::fs::symlink("file", host.join("filelink")).unwrap();
        let id = super::super::cred::Identity::default();
        let invoke = |path: &str, flags| {
            let path = CString::new(path).unwrap();
            unlinkat_as(
                [
                    crate::vfs::LINUX_AT_FDCWD as u64,
                    path.as_ptr() as u64,
                    flags,
                    0,
                    0,
                    0,
                ],
                &id,
            )
        };
        for root in ["/", "///"] {
            assert_eq!(invoke(root, 0), -EISDIR);
            assert_eq!(invoke(root, AT_REMOVEDIR), -crate::errno::EBUSY as i64);
        }
        for suffix in [".", "../"] {
            assert_eq!(invoke(&format!("{base}/{suffix}"), 0), -EISDIR);
            assert_eq!(
                invoke(&format!("{base}/{suffix}"), AT_REMOVEDIR),
                if suffix == "." {
                    -EINVAL as i64
                } else {
                    -ENOTEMPTY
                }
            );
        }
        assert_eq!(
            invoke(&format!("{base}/file/"), 0),
            -crate::errno::ENOTDIR as i64
        );
        assert_eq!(invoke(&format!("{base}/dir/"), 0), -EISDIR);
        for link in ["dirlink", "filelink"] {
            assert_eq!(
                invoke(&format!("{base}/{link}/"), 0),
                -crate::errno::ENOTDIR as i64
            );
            assert_eq!(
                invoke(&format!("{base}/{link}/"), AT_REMOVEDIR),
                -crate::errno::ENOTDIR as i64
            );
        }
        assert_eq!(
            invoke(&format!("{base}/missing/"), 0),
            -crate::errno::ENOENT as i64
        );
        assert!(
            host.join("file").is_file()
                && host.join("dir").is_dir()
                && host.join("dirlink").symlink_metadata().is_ok()
        );
        assert_eq!(invoke(&format!("{base}/dir/"), AT_REMOVEDIR), 0);
        std::fs::remove_dir_all(host).unwrap();
    }
    #[test]
    fn unlink_walk_checks_symlink_prefixes_dotdot_and_relative_fd_base() {
        use std::{ffi::CString, os::fd::AsRawFd};
        let (_guard, view) = crate::vfs::test_view();
        let base = format!("/data/unlink-walk-{}", std::process::id());
        let host = view.join("data").join(base.trim_start_matches("/data/"));
        std::fs::create_dir_all(host.join("denied")).unwrap();
        std::fs::create_dir_all(host.join("allowed")).unwrap();
        let record = |suffix: &str, uid, mode| {
            let path = CString::new(host.join(suffix).as_os_str().as_encoded_bytes()).unwrap();
            super::super::attrs::record(
                super::super::attrs::Host::Path(&path),
                || format!("{base}/{suffix}"),
                super::super::attrs::Attr {
                    uid: Some(uid),
                    gid: Some(2000),
                    mode: Some(mode),
                },
            );
        };
        record("denied", 1000, 0o700);
        record("allowed", 3000, 0o777);
        std::os::unix::fs::symlink(format!("{base}/allowed"), host.join("denied/link")).unwrap();
        std::fs::write(host.join("allowed/victim"), b"owned").unwrap();
        let mut id = super::super::cred::Identity::default();
        id.uid = [3000; 4];
        id.gid = [3000; 4];
        id.cap_eff = 0;
        let invoke = |fd, path: &str, flags| {
            let path = CString::new(path).unwrap();
            unlinkat_as([fd as u64, path.as_ptr() as u64, flags, 0, 0, 0], &id)
        };
        assert_eq!(
            invoke(
                crate::vfs::LINUX_AT_FDCWD,
                &format!("{base}/denied/link/victim"),
                0
            ),
            -crate::errno::EACCES as i64
        );
        assert_eq!(
            invoke(
                crate::vfs::LINUX_AT_FDCWD,
                &format!("{base}/denied/../allowed/victim"),
                0
            ),
            -crate::errno::EACCES as i64
        );
        assert!(host.join("allowed/victim").is_file());
        std::fs::create_dir(host.join("denied/nested")).unwrap();
        record("denied/nested", 3000, 0o777);
        std::fs::write(host.join("denied/nested/victim"), b"owned").unwrap();
        let fd = std::fs::File::open(host.join("denied/nested")).unwrap();
        assert_eq!(
            invoke(fd.as_raw_fd(), "victim", 0),
            0,
            "dirfd access does not search ancestors above its base"
        );
        record("allowed", 3000, 0o555);
        assert_eq!(
            invoke(
                crate::vfs::LINUX_AT_FDCWD,
                &format!("{base}/allowed/victim"),
                0
            ),
            -crate::errno::EACCES as i64
        );
        assert_eq!(
            invoke(
                crate::vfs::LINUX_AT_FDCWD,
                &format!("{base}/allowed/missing"),
                0
            ),
            -crate::errno::ENOENT as i64
        );
        std::fs::remove_dir_all(&host).unwrap();
    }
    #[test]
    fn unlink_sticky_checks_symlink_owner_and_rmdir() {
        use std::ffi::CString;
        let (_guard, view) = crate::vfs::test_view();
        let base = format!("/data/unlink-sticky-{}", std::process::id());
        let host = view.join("data").join(base.trim_start_matches("/data/"));
        std::fs::create_dir_all(&host).unwrap();
        let record = |suffix: &str, uid, mode| {
            let path = CString::new(host.join(suffix).as_os_str().as_encoded_bytes()).unwrap();
            super::super::attrs::record(
                super::super::attrs::Host::Path(&path),
                || {
                    if suffix.is_empty() {
                        base.clone()
                    } else {
                        format!("{base}/{suffix}")
                    }
                },
                super::super::attrs::Attr {
                    uid: Some(uid),
                    gid: Some(2000),
                    mode: Some(mode),
                },
            );
        };
        record("", 1000, 0o1777);
        std::fs::write(host.join("target"), b"owned").unwrap();
        record("target", 3000, 0o600);
        std::os::unix::fs::symlink("target", host.join("link")).unwrap();
        record("link", 2000, 0o777);
        let mut id = super::super::cred::Identity::default();
        id.uid = [3000; 4];
        id.gid = [3000; 4];
        id.cap_eff = 0;
        let link = CString::new(format!("{base}/link")).unwrap();
        let args = [
            crate::vfs::LINUX_AT_FDCWD as u64,
            link.as_ptr() as u64,
            0,
            0,
            0,
            0,
        ];
        assert_eq!(unlinkat_as(args, &id), -EPERM);
        assert!(host.join("link").symlink_metadata().is_ok());
        id.uid[3] = 2000;
        assert_eq!(unlinkat_as(args, &id), 0);
        assert!(host.join("target").is_file());
        std::fs::create_dir(host.join("dir")).unwrap();
        record("dir", 4000, 0o700);
        let dir = CString::new(format!("{base}/dir")).unwrap();
        let args = [
            crate::vfs::LINUX_AT_FDCWD as u64,
            dir.as_ptr() as u64,
            AT_REMOVEDIR,
            0,
            0,
            0,
        ];
        assert_eq!(unlinkat_as(args, &id), -EPERM);
        id.cap_eff = 1 << 3;
        assert_eq!(unlinkat_as(args, &id), 0);
        std::fs::remove_dir_all(&host).unwrap();
    }
    #[test]
    fn sticky_directory_requires_owner_or_fowner_capability() {
        let mut parent: libc::stat = unsafe { std::mem::zeroed() };
        parent.st_mode = libc::S_IFDIR | 0o1777;
        parent.st_uid = 1000;
        let mut target: libc::stat = unsafe { std::mem::zeroed() };
        target.st_uid = 2000;
        let mut id = super::super::cred::Identity::default();
        id.cap_eff = 0;
        id.uid = [3000; 4];
        assert!(!sticky_permits(&parent, &target, &id));
        id.uid[3] = 1000;
        assert!(sticky_permits(&parent, &target, &id));
        id.uid[3] = 2000;
        assert!(sticky_permits(&parent, &target, &id));
        id.uid[3] = 3000;
        id.cap_eff = 1 << 1;
        assert!(!sticky_permits(&parent, &target, &id));
        id.cap_eff = 1 << 3;
        assert!(sticky_permits(&parent, &target, &id));
    }
    #[test]
    fn directory_dac_uses_fs_ids_groups_and_capabilities() {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_mode = libc::S_IFDIR | 0o750;
        st.st_uid = 1000;
        st.st_gid = 2000;
        let mut id = super::super::cred::Identity::default();
        id.uid = [1001; 4];
        id.gid = [2001; 4];
        id.cap_eff = 0;
        assert!(!directory_permits(&st, 3, &id));
        id.groups.push(2000);
        assert!(directory_permits(&st, 1, &id));
        assert!(!directory_permits(&st, 3, &id));
        id.uid[3] = 1000;
        assert!(directory_permits(&st, 3, &id));
        id.uid[3] = 0;
        st.st_mode = libc::S_IFDIR;
        assert!(!directory_permits(&st, 3, &id));
        id.cap_eff = 1 << 2;
        assert!(directory_permits(&st, 1, &id));
        assert!(!directory_permits(&st, 3, &id));
        id.cap_eff = 1 << 1;
        assert!(directory_permits(&st, 3, &id));
    }
}
