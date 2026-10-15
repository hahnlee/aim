//! Extended attributes (`*xattr`). A guest attribute `name` is the host
//! attribute `dev.aim.xattr.<name>`, so the guest neither sees nor
//! changes the host's own (`com.apple.*`) and the layer's private ones.
//!
//! `security.selinux` is special as on Linux, where every inode has a
//! context: bpffs and cgroup2 inodes read as their `genfscon` label
//! (`genfs`), and a file nobody labeled reads as `unlabeled`, which is what
//! the kernel reports for an inode without the attribute. Image files carry
//! their original label (android-image-extract). libselinux's restorecon
//! (installd, for app data) reads the label before it sets one.

use std::{ffi::{CStr, CString}, os::fd::AsRawFd};

use super::fs::check_writable;
use super::{genfs, procfs};
use crate::errno::{self, E2BIG, EINVAL, ERANGE};
use crate::sys::guest_cstr;
use crate::vfs;

const PREFIX: &[u8] = b"dev.aim.xattr.";
const SELINUX: &[u8] = b"security.selinux";
/// What the kernel reports for an inode without a label.
const UNLABELED: &[u8] = b"u:object_r:unlabeled:s0\0";

const XATTR_CREATE: u64 = 1;
const XATTR_REPLACE: u64 = 2;
/// Linux XATTR_NAME_MAX and XATTR_SIZE_MAX.
const NAME_MAX: usize = 255;
const SIZE_MAX: usize = 65536;
const AT_FDCWD: i32 = -100;

/// A file named by path (following symlinks or not) or by descriptor.
enum Target {
    /// Host path, whether a final symlink is followed, guest path.
    Path(CString, bool, String),
    Fd(i32),
}

fn host_name(name: &[u8]) -> Result<CString, i64> {
    if name.is_empty() || name.len() > NAME_MAX {
        return Err(-(ERANGE as i64));
    }
    let mut n = PREFIX.to_vec();
    n.extend_from_slice(name);
    CString::new(n).map_err(|_| -(EINVAL as i64))
}

fn path_target(path: u64, follow: bool, write: bool) -> Result<Target, i64> {
    // SAFETY: guest path pointer.
    let path=guest_cstr(path).map_err(|error|-(error as i64))?;
    let p=path.as_slice();
    let r = vfs::resolve(AT_FDCWD, p, follow).map_err(|e| -(e as i64))?;
    if write {
        check_writable(&r)?;
    }
    Ok(Target::Path(r.host, follow, r.guest))
}

fn options(t: &Target) -> libc::c_int {
    match t {
        Target::Path(_, false, _) => libc::XATTR_NOFOLLOW,
        _ => 0,
    }
}

fn host_get(t: &Target, name: &CStr, buf: *mut u8, size: usize) -> i64 {
    let (buf, size) = if size == 0 {
        (std::ptr::null_mut(), 0)
    } else {
        (buf, size)
    };
    // SAFETY: host path or fd, guest buffer of `size` bytes.
    let n = unsafe {
        match t {
            Target::Path(p, _, _) => {
                libc::getxattr(p.as_ptr(), name.as_ptr(), buf.cast(), size, 0, options(t))
            }
            Target::Fd(fd) => libc::fgetxattr(*fd, name.as_ptr(), buf.cast(), size, 0, 0),
        }
    };
    if n < 0 {
        -(errno::last() as i64)
    } else {
        n as i64
    }
}

/// Whether the file exists (so a missing label is `unlabeled`, not ENOENT).
fn exists(t: &Target) -> Result<(), i64> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path or fd, local buffer.
    let r = unsafe {
        match t {
            Target::Path(p, true, _) => libc::stat(p.as_ptr(), &mut st),
            Target::Path(p, false, _) => libc::lstat(p.as_ptr(), &mut st),
            Target::Fd(fd) => libc::fstat(*fd, &mut st),
        }
    };
    if r < 0 {
        Err(-(errno::last() as i64))
    } else {
        Ok(())
    }
}

fn get(t: Target, name: u64, value: u64, size: u64) -> i64 {
    // SAFETY: guest name pointer.
    let name=match guest_cstr(name){Ok(name)=>name,Err(error)=>return -(error as i64)};
    let name=name.as_slice();
    let hname = match host_name(name) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let size = size as usize;
    let r = host_get(&t, &hname, value as *mut u8, size);
    // A file without a label, or one the host keeps no attributes on
    // (pipes, sockets), has the default context.
    if name != SELINUX || r >= 0 || r == -(ERANGE as i64) {
        return r;
    }
    if let Err(e) = exists(&t) {
        return e;
    }
    let guest = match &t {
        Target::Path(_, _, g) => Some(g.clone()),
        Target::Fd(fd) => procfs::fd_guest_path(*fd).ok(),
    };
    let label = match guest.and_then(|g| genfs::label(&g)) {
        Some(l) => {
            let mut v = l.into_bytes();
            v.push(0);
            v
        }
        None => UNLABELED.to_vec(),
    };
    if size == 0 {
        return label.len() as i64;
    }
    if size < label.len() {
        return -(ERANGE as i64);
    }
    // SAFETY: guest buffer of `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(label.as_ptr(), value as *mut u8, label.len()) };
    label.len() as i64
}

fn set(t: Target, name: u64, value: u64, size: u64, flags: u64) -> i64 {
    // SAFETY: guest name pointer.
    let name=match guest_cstr(name){Ok(name)=>name,Err(error)=>return -(error as i64)};
    let name=name.as_slice();
    let hname = match host_name(name) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0 {
        return -(EINVAL as i64);
    }
    if size as usize > SIZE_MAX {
        return -(E2BIG as i64);
    }
    let mut opts = options(&t);
    if flags & XATTR_CREATE != 0 {
        opts |= libc::XATTR_CREATE;
    }
    if flags & XATTR_REPLACE != 0 {
        opts |= libc::XATTR_REPLACE;
    }
    // SAFETY: host path or fd, guest value of `size` bytes.
    let r = unsafe {
        match &t {
            Target::Path(p, _, _) => libc::setxattr(
                p.as_ptr(),
                hname.as_ptr(),
                value as *const _,
                size as usize,
                0,
                opts,
            ),
            Target::Fd(fd) => libc::fsetxattr(
                *fd,
                hname.as_ptr(),
                value as *const _,
                size as usize,
                0,
                opts,
            ),
        }
    };
    errno::check(r as i64)
}

/// The guest's names in a host name list.
fn guest_names(host: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for n in host.split(|&b| b == 0) {
        if let Some(g) = n.strip_prefix(PREFIX) {
            out.extend_from_slice(g);
            out.push(0);
        }
    }
    out
}

fn list(t: Target, buf: u64, size: u64) -> i64 {
    // SAFETY: host path or fd; a size query, then a read into a local buffer.
    let query = |b: *mut libc::c_char, n: usize| unsafe {
        match &t {
            Target::Path(p, _, _) => libc::listxattr(p.as_ptr(), b, n, options(&t)),
            Target::Fd(fd) => libc::flistxattr(*fd, b, n, 0),
        }
    };
    let mut host = Vec::new();
    loop {
        let need = query(std::ptr::null_mut(), 0);
        if need < 0 {
            return -(errno::last() as i64);
        }
        host.resize(need as usize, 0);
        let n = query(host.as_mut_ptr().cast(), host.len());
        if n >= 0 {
            host.truncate(n as usize);
            break;
        }
        if errno::last() != ERANGE {
            return -(errno::last() as i64);
        }
    }
    let mut names = guest_names(&host);
    if !names.split(|&b| b == 0).any(|n| n == SELINUX) {
        names.extend_from_slice(SELINUX);
        names.push(0);
    }
    let size = size as usize;
    if size == 0 {
        return names.len() as i64;
    }
    if size < names.len() {
        return -(ERANGE as i64);
    }
    // SAFETY: guest buffer of `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(names.as_ptr(), buf as *mut u8, names.len()) };
    names.len() as i64
}

fn remove(t: Target, name: u64) -> i64 {
    // SAFETY: guest name pointer.
    let name=match guest_cstr(name){Ok(name)=>name,Err(error)=>return -(error as i64)};
    let name=name.as_slice();
    let hname = match host_name(name) {
        Ok(n) => n,
        Err(e) => return e,
    };
    // SAFETY: host path or fd.
    let r = unsafe {
        match &t {
            Target::Path(p, _, _) => libc::removexattr(p.as_ptr(), hname.as_ptr(), options(&t)),
            Target::Fd(fd) => libc::fremovexattr(*fd, hname.as_ptr(), 0),
        }
    };
    errno::check(r as i64)
}

fn pin_target(nr: u64, descriptor_call: u64, args: &mut [u64; 6]) -> Result<Option<super::fdtab::Pinned>, i64> {
    if nr != descriptor_call { return Ok(None); }
    let pin = super::fdtab::pin_guest(args[0] as i32).map_err(|error| -(error as i64))?;
    if matches!(pin.kind(), Some(super::fdtab::Kind::Path(_))) { return Err(-(errno::EBADF as i64)); }
    args[0] = pin.descriptor().as_raw_fd() as u64;
    Ok(Some(pin))
}

/// setxattr (5), lsetxattr (6), fsetxattr (7).
pub fn setxattr(nr: u64, mut a: [u64; 6]) -> i64 {
    let _pin = match pin_target(nr, 7, &mut a) { Ok(pin) => pin, Err(error) => return error };
    if let Some(result)=super::fuse_client::xattr_syscall(nr,a){return result;}
    let t = match nr {
        7 => Target::Fd(a[0] as i32),
        _ => match path_target(a[0], nr == 5, true) {
            Ok(t) => t,
            Err(e) => return e,
        },
    };
    set(t, a[1], a[2], a[3], a[4])
}

/// getxattr (8), lgetxattr (9), fgetxattr (10).
pub fn getxattr(nr: u64, mut a: [u64; 6]) -> i64 {
    let _pin = match pin_target(nr, 10, &mut a) { Ok(pin) => pin, Err(error) => return error };
    if let Some(result)=super::fuse_client::xattr_syscall(nr,a){return result;}
    let t = match nr {
        10 => Target::Fd(a[0] as i32),
        _ => match path_target(a[0], nr == 8, false) {
            Ok(t) => t,
            Err(e) => return e,
        },
    };
    get(t, a[1], a[2], a[3])
}

/// listxattr (11), llistxattr (12), flistxattr (13).
pub fn listxattr(nr: u64, mut a: [u64; 6]) -> i64 {
    let _pin = match pin_target(nr, 13, &mut a) { Ok(pin) => pin, Err(error) => return error };
    if let Some(result)=super::fuse_client::xattr_syscall(nr,a){return result;}
    let t = match nr {
        13 => Target::Fd(a[0] as i32),
        _ => match path_target(a[0], nr == 11, false) {
            Ok(t) => t,
            Err(e) => return e,
        },
    };
    list(t, a[1], a[2])
}

/// removexattr (14), lremovexattr (15), fremovexattr (16).
pub fn removexattr(nr: u64, mut a: [u64; 6]) -> i64 {
    let _pin = match pin_target(nr, 16, &mut a) { Ok(pin) => pin, Err(error) => return error };
    if let Some(result)=super::fuse_client::xattr_syscall(nr,a){return result;}
    let t = match nr {
        16 => Target::Fd(a[0] as i32),
        _ => match path_target(a[0], nr == 14, true) {
            Ok(t) => t,
            Err(e) => return e,
        },
    };
    remove(t, a[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_xattrs_keep_the_pinned_inode_after_original_slot_reuse() {
        if super::super::fdtab::isolated_kernel_test("sys::xattr::tests::descriptor_xattrs_keep_the_pinned_inode_after_original_slot_reuse"){return;}
        use std::{fs::OpenOptions, os::fd::{AsFd, AsRawFd}};
        let (_view, root) = vfs::test_view();
        let original = OpenOptions::new().read(true).write(true).create_new(true).open(root.join("xattr-original")).unwrap();
        let other = OpenOptions::new().read(true).write(true).create_new(true).open(root.join("xattr-other")).unwrap();
        let fd = unsafe { libc::dup(original.as_raw_fd()) };
        assert!(fd >= 0); super::super::fdtab::publish_guest(fd).unwrap();
        let mut args = [fd as u64, 0, 0, 0, 0, 0];
        let pin = pin_target(7, 7, &mut args).unwrap().unwrap();
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(unsafe { libc::dup2(other.as_raw_fd(), fd) }, fd);
        let value = b"owned";
        assert_eq!(set(Target::Fd(args[0] as i32), c"user.pin".as_ptr() as u64, value.as_ptr() as u64, value.len() as u64, 0), 0);
        let mut read = [0u8; 8];
        assert_eq!(get(Target::Fd(original.as_fd().as_raw_fd()), c"user.pin".as_ptr() as u64, read.as_mut_ptr() as u64, 8), 5);
        assert_eq!(&read[..5], value);
        assert!(get(Target::Fd(other.as_raw_fd()), c"user.pin".as_ptr() as u64, read.as_mut_ptr() as u64, 8) < 0);
        drop(pin); assert_eq!(unsafe { libc::close(fd) }, 0);
        assert!(pin_target(10, 10, &mut [other.as_raw_fd() as u64, 0, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn only_guest_names_are_listed() {
        let host = b"com.apple.provenance\0dev.aim.xattr.user.a\0dev.aim.ashmem\0dev.aim.xattr.security.selinux\0";
        assert_eq!(guest_names(host), b"user.a\0security.selinux\0".to_vec());
    }

    #[test]
    fn names_are_bounded() {
        assert!(host_name(b"").is_err());
        assert!(host_name(&[b'a'; 256]).is_err());
        assert_eq!(
            host_name(b"user.x").unwrap().as_bytes(),
            b"dev.aim.xattr.user.x"
        );
    }
}
