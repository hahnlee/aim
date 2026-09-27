//! The minimum of AF_UNIX sockets a service needs to reach init's property
//! service: `socket`, `connect` to a path through the guest view, `sendto`
//! and `recvfrom` without addresses. Other families, SOCK_SEQPACKET and
//! `bind`/`listen` are not handled here.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

use crate::errno::{self, EINVAL};
use crate::sys::guest_cstr;
use crate::vfs;

const AF_UNIX: u64 = 1;
const SOCK_STREAM: u64 = 1;
const SOCK_DGRAM: u64 = 2;
const SOCK_NONBLOCK: u64 = 0o4000;
const SOCK_CLOEXEC: u64 = 0o2000000;
const EAFNOSUPPORT: i64 = 97;
const EPROTONOSUPPORT: i64 = 93;

// Linux MSG_* bits that differ from Darwin's.
const MSG_DONTWAIT: u64 = 0x40;
const MSG_WAITALL: u64 = 0x100;

fn msg_flags(f: u64) -> i32 {
    let mut h = (f & 0x3) as i32; // MSG_OOB, MSG_PEEK
    if f & MSG_DONTWAIT != 0 {
        h |= libc::MSG_DONTWAIT;
    }
    if f & MSG_WAITALL != 0 {
        h |= libc::MSG_WAITALL;
    }
    // MSG_NOSIGNAL (0x4000) needs nothing: every socket is SO_NOSIGPIPE.
    h
}

pub fn socket(a: [u64; 6]) -> i64 {
    let (domain, kind) = (a[0], a[1]);
    if domain != AF_UNIX {
        return -EAFNOSUPPORT;
    }
    let base = kind & 0xf;
    if base != SOCK_STREAM && base != SOCK_DGRAM {
        return -EPROTONOSUPPORT;
    }
    // SAFETY: plain socket creation and flags on the new fd.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, base as i32, 0);
        if fd < 0 {
            return -(errno::last() as i64);
        }
        let one: i32 = 1;
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&one as *const i32).cast(),
            4,
        );
        if kind & SOCK_CLOEXEC != 0 {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        if kind & SOCK_NONBLOCK != 0 {
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
        }
        fd as i64
    }
}

unsafe extern "C" {
    fn pthread_fchdir_np(fd: i32) -> i32;
}

/// `connect` to a guest AF_UNIX path. Host paths longer than `sun_path`
/// are reached by their name relative to the directory, which becomes this
/// thread's working directory for the call.
pub fn connect(a: [u64; 6]) -> i64 {
    let (fd, addr, len) = (a[0] as i32, a[1], a[2] as usize);
    if len < 3 {
        return -(EINVAL as i64);
    }
    // SAFETY: the guest's sockaddr_un (u16 family, then the path).
    let (family, path) = unsafe { ((addr as *const u16).read_unaligned(), guest_cstr(addr + 2)) };
    if family as u64 != AF_UNIX || path.is_empty() {
        return -(errno::ENOENT as i64);
    }
    let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, path, true) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    let host = std::path::Path::new(std::ffi::OsStr::from_bytes(r.host.as_bytes()));
    let (dir, name) = match (host.parent(), host.file_name()) {
        (Some(d), Some(n)) => (d, n),
        _ => return -(errno::ENOENT as i64),
    };
    let target = if r.host.as_bytes().len() < 104 {
        r.host.as_bytes()
    } else {
        name.as_bytes()
    };
    // SAFETY: a local sockaddr_un; the directory fd is ours.
    unsafe {
        let mut sa: libc::sockaddr_un = std::mem::zeroed();
        sa.sun_family = libc::AF_UNIX as u8;
        if target.len() >= sa.sun_path.len() {
            return -36; // ENAMETOOLONG
        }
        for (d, s) in sa.sun_path.iter_mut().zip(target) {
            *d = *s as libc::c_char;
        }
        let relative = target.len() != r.host.as_bytes().len();
        let mut dirfd = -1;
        if relative {
            let d = CString::new(dir.as_os_str().as_bytes()).unwrap();
            dirfd = libc::open(
                d.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            );
            if dirfd < 0 || pthread_fchdir_np(dirfd) < 0 {
                if dirfd >= 0 {
                    libc::close(dirfd);
                }
                return -(errno::ENOENT as i64);
            }
        }
        let rc = libc::connect(
            fd,
            (&sa as *const libc::sockaddr_un).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as u32,
        );
        let e = errno::last();
        if relative {
            pthread_fchdir_np(-1);
            libc::close(dirfd);
        }
        if rc < 0 { -(e as i64) } else { 0 }
    }
}

pub fn sendto(a: [u64; 6]) -> i64 {
    if a[4] != 0 {
        return -(errno::EINVAL as i64);
    }
    // SAFETY: the guest's buffer.
    errno::check(unsafe {
        libc::send(
            a[0] as i32,
            a[1] as *const _,
            a[2] as usize,
            msg_flags(a[3]),
        )
    } as i64)
}

pub fn recvfrom(a: [u64; 6]) -> i64 {
    if a[4] != 0 {
        return -(errno::EINVAL as i64);
    }
    // SAFETY: the guest's buffer.
    errno::check(
        unsafe { libc::recv(a[0] as i32, a[1] as *mut _, a[2] as usize, msg_flags(a[3])) } as i64,
    )
}
