//! File syscalls. Guest fds are host fds; paths go through the guest root
//! (`vfs`); flags and `struct stat` are translated between Linux and Darwin.

use std::ffi::CString;

use crate::errno::{self, EBADF, EINVAL, ENOENT, ENOTTY, ERANGE};
use crate::sys::{guest_cstr, procfs};
use crate::vfs;

// Linux arm64 open flags.
const O_ACCMODE: u64 = 0o3;
const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_NOCTTY: u64 = 0o400;
const O_TRUNC: u64 = 0o1000;
const O_APPEND: u64 = 0o2000;
const O_NONBLOCK: u64 = 0o4000;
const O_DSYNC: u64 = 0o10000;
const O_DIRECTORY: u64 = 0o40000;
const O_NOFOLLOW: u64 = 0o100000;
const O_CLOEXEC: u64 = 0o2000000;
const O_SYNC: u64 = 0o4010000;
const O_PATH: u64 = 0o10000000;
const O_TMPFILE: u64 = 0o20000000;

// Linux *at() flags.
const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_EACCESS: u64 = 0x200;
const AT_EMPTY_PATH: u64 = 0x1000;

fn open_flags_to_host(f: u64) -> i32 {
    let mut h = match f & O_ACCMODE {
        0 => libc::O_RDONLY,
        1 => libc::O_WRONLY,
        _ => libc::O_RDWR,
    };
    if f & O_PATH != 0 {
        h = libc::O_RDONLY;
    }
    for (l, d) in [
        (O_CREAT, libc::O_CREAT),
        (O_EXCL, libc::O_EXCL),
        (O_NOCTTY, libc::O_NOCTTY),
        (O_TRUNC, libc::O_TRUNC),
        (O_APPEND, libc::O_APPEND),
        (O_NONBLOCK, libc::O_NONBLOCK),
        (O_DIRECTORY, libc::O_DIRECTORY),
        (O_NOFOLLOW, libc::O_NOFOLLOW),
        (O_CLOEXEC, libc::O_CLOEXEC),
    ] {
        if f & l != 0 {
            h |= d;
        }
    }
    if f & O_SYNC == O_SYNC {
        h |= libc::O_SYNC;
    } else if f & O_DSYNC != 0 {
        h |= libc::O_DSYNC;
    }
    h
}

fn open_flags_from_host(h: i32) -> u64 {
    let mut f = (h & libc::O_ACCMODE) as u64;
    for (l, d) in [(O_APPEND, libc::O_APPEND), (O_NONBLOCK, libc::O_NONBLOCK)] {
        if h & d != 0 {
            f |= l;
        }
    }
    f
}

pub fn openat(a: [u64; 6]) -> i64 {
    let (dirfd, flags, mode) = (a[0] as i32, a[2], a[3]);
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    if flags & O_TMPFILE == O_TMPFILE {
        return -95; // EOPNOTSUPP
    }
    if procfs::is_self_exe(path) {
        return match CString::new(crate::sys::process::exe_host_path()) {
            Ok(p) => {
                errno::check(unsafe { libc::open(p.as_ptr(), open_flags_to_host(flags)) } as i64)
            }
            Err(_) => -(ENOENT as i64),
        };
    }
    let follow = flags & O_NOFOLLOW == 0 && flags & (O_CREAT | O_EXCL) != (O_CREAT | O_EXCL);
    let r = match vfs::resolve(dirfd, path, follow) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if let Some(fd) = super::binder::open(&r.guest, flags) {
        return fd;
    }
    if let Some(fd) = super::selinuxfs::open(&r.guest, flags) {
        return fd;
    }
    let hflags = open_flags_to_host(flags);
    // SAFETY: host path from the resolver.
    let fd = unsafe { libc::open(r.host.as_ptr(), hflags, mode as libc::c_uint) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    // An original ELF with a translation-cache entry is replaced by the
    // translated file here, before the guest reads its headers.
    crate::xrt::on_open(fd, &r.host, &r.guest, hflags);
    fd as i64
}

pub fn close(a: [u64; 6]) -> i64 {
    // SAFETY: closing a guest fd.
    errno::check(unsafe { libc::close(a[0] as i32) } as i64)
}

pub fn read(a: [u64; 6]) -> i64 {
    // SAFETY: guest buffer.
    errno::check(unsafe { libc::read(a[0] as i32, a[1] as *mut _, a[2] as usize) } as i64)
}

pub fn write(a: [u64; 6]) -> i64 {
    // SAFETY: guest buffer.
    errno::check(unsafe { libc::write(a[0] as i32, a[1] as *const _, a[2] as usize) } as i64)
}

pub fn pread64(a: [u64; 6]) -> i64 {
    // SAFETY: guest buffer.
    errno::check(
        unsafe { libc::pread(a[0] as i32, a[1] as *mut _, a[2] as usize, a[3] as i64) } as i64,
    )
}

pub fn pwrite64(a: [u64; 6]) -> i64 {
    // SAFETY: guest buffer.
    errno::check(
        unsafe { libc::pwrite(a[0] as i32, a[1] as *const _, a[2] as usize, a[3] as i64) } as i64,
    )
}

// struct iovec is { void *base; size_t len; } on both kernels.
pub fn readv(a: [u64; 6]) -> i64 {
    // SAFETY: guest iovec array.
    errno::check(
        unsafe { libc::readv(a[0] as i32, a[1] as *const libc::iovec, a[2] as i32) } as i64,
    )
}

pub fn writev(a: [u64; 6]) -> i64 {
    // SAFETY: guest iovec array.
    errno::check(
        unsafe { libc::writev(a[0] as i32, a[1] as *const libc::iovec, a[2] as i32) } as i64,
    )
}

pub fn lseek(a: [u64; 6]) -> i64 {
    // SEEK_SET/CUR/END agree; Linux SEEK_DATA/HOLE are 3/4, Darwin 4/3.
    let whence = match a[2] {
        3 => libc::SEEK_DATA,
        4 => libc::SEEK_HOLE,
        w => w as i32,
    };
    // SAFETY: plain lseek.
    errno::check(unsafe { libc::lseek(a[0] as i32, a[1] as i64, whence) })
}

/// Linux arm64 (asm-generic) `struct stat`, 128 bytes.
#[repr(C)]
#[derive(Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    pad1: u64,
    st_size: i64,
    st_blksize: i32,
    pad2: i32,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: u64,
    st_mtime: i64,
    st_mtime_nsec: u64,
    st_ctime: i64,
    st_ctime_nsec: u64,
    unused: [u32; 2],
}
const _: () = assert!(std::mem::size_of::<LinuxStat>() == 128);

/// Darwin dev_t packs major in the top 8 bits; Linux's new encoding differs.
fn linux_dev(d: i32) -> u64 {
    let d = d as u32 as u64;
    let (major, minor) = (d >> 24, d & 0xff_ffff);
    (minor & 0xff) | ((major & 0xfff) << 8) | ((minor & !0xff) << 12) | ((major & !0xfff) << 32)
}

fn put_stat(st: &libc::stat, out: u64) {
    let l = LinuxStat {
        st_dev: linux_dev(st.st_dev),
        st_ino: st.st_ino,
        st_mode: st.st_mode as u32,
        st_nlink: st.st_nlink as u32,
        // Android files are root's unless init changed them (the contract's
        // fs-attrs); the host owner is never the guest's.
        st_uid: 0,
        st_gid: 0,
        st_rdev: linux_dev(st.st_rdev),
        st_size: st.st_size,
        st_blksize: st.st_blksize,
        st_blocks: st.st_blocks,
        st_atime: st.st_atime,
        st_atime_nsec: st.st_atime_nsec as u64,
        st_mtime: st.st_mtime,
        st_mtime_nsec: st.st_mtime_nsec as u64,
        st_ctime: st.st_ctime,
        st_ctime_nsec: st.st_ctime_nsec as u64,
        ..Default::default()
    };
    // SAFETY: guest stat buffer.
    unsafe { (out as *mut LinuxStat).write_unaligned(l) };
}

fn fstat_into(fd: i32, out: u64) -> i64 {
    // SAFETY: stat buffer on our stack.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    put_stat(&st, out);
    0
}

pub fn fstat(a: [u64; 6]) -> i64 {
    fstat_into(a[0] as i32, a[1])
}

pub fn newfstatat(a: [u64; 6]) -> i64 {
    let (dirfd, out, flags) = (a[0] as i32, a[2], a[3]);
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    if path.is_empty() {
        return if flags & AT_EMPTY_PATH != 0 {
            fstat_into(dirfd, out)
        } else {
            -(ENOENT as i64)
        };
    }
    // linker64 finds the program through /proc/self/exe, whatever argv[0] is.
    let host = if procfs::is_self_exe(path) && flags & AT_SYMLINK_NOFOLLOW == 0 {
        match CString::new(crate::sys::process::exe_host_path()) {
            Ok(p) => p,
            Err(_) => return -(ENOENT as i64),
        }
    } else {
        match vfs::resolve(dirfd, path, flags & AT_SYMLINK_NOFOLLOW == 0) {
            Ok(r) => r.host,
            Err(e) => return -(e as i64),
        }
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path and local stat buffer.
    if unsafe { libc::lstat(host.as_ptr(), &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    put_stat(&st, out);
    0
}

/// Linux arm64 `struct statfs` (asm-generic, 64-bit fields), 120 bytes.
#[repr(C)]
#[derive(Default)]
struct LinuxStatfs {
    f_type: u64,
    f_bsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: [i32; 2],
    f_namelen: u64,
    f_frsize: u64,
    f_flags: u64,
    f_spare: [u64; 4],
}
const _: () = assert!(std::mem::size_of::<LinuxStatfs>() == 120);

/// The guest image is presented as ext4, the filesystem of Android's
/// system partitions.
const EXT4_SUPER_MAGIC: u64 = 0xef53;
const ST_RDONLY: u64 = 1;
const ST_NOSUID: u64 = 2;

fn put_statfs(s: &libc::statfs, out: u64) {
    let mut flags = 0;
    if s.f_flags & libc::MNT_RDONLY as u32 != 0 {
        flags |= ST_RDONLY;
    }
    if s.f_flags & libc::MNT_NOSUID as u32 != 0 {
        flags |= ST_NOSUID;
    }
    let l = LinuxStatfs {
        f_type: EXT4_SUPER_MAGIC,
        f_bsize: s.f_bsize as u64,
        f_blocks: s.f_blocks,
        f_bfree: s.f_bfree,
        f_bavail: s.f_bavail,
        f_files: s.f_files,
        f_ffree: s.f_ffree,
        // SAFETY: fsid_t is two i32 values.
        f_fsid: unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(s.f_fsid) },
        f_namelen: 255,
        f_frsize: s.f_bsize as u64,
        f_flags: flags,
        ..Default::default()
    };
    // SAFETY: guest statfs buffer.
    unsafe { (out as *mut LinuxStatfs).write_unaligned(l) };
}

pub fn fstatfs(a: [u64; 6]) -> i64 {
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: local buffer.
    if unsafe { libc::fstatfs(a[0] as i32, &mut s) } < 0 {
        return -(errno::last() as i64);
    }
    put_statfs(&s, a[1]);
    0
}

pub fn statfs(a: [u64; 6]) -> i64 {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(a[0]) };
    let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, p, true) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if let Some(magic) = super::selinuxfs::statfs_magic(&r.guest) {
        let l = LinuxStatfs {
            f_type: magic,
            f_bsize: 4096,
            f_namelen: 255,
            f_frsize: 4096,
            ..Default::default()
        };
        // SAFETY: guest statfs buffer.
        unsafe { (a[1] as *mut LinuxStatfs).write_unaligned(l) };
        return 0;
    }
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    if unsafe { libc::statfs(r.host.as_ptr(), &mut s) } < 0 {
        return -(errno::last() as i64);
    }
    put_statfs(&s, a[1]);
    0
}

pub fn readlinkat(a: [u64; 6]) -> i64 {
    let (dirfd, buf, size) = (a[0] as i32, a[2], a[3] as usize);
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    if size == 0 {
        return -(EINVAL as i64);
    }
    let target: Vec<u8> = if let Some(t) = procfs::readlink(path) {
        match t {
            Ok(t) => t,
            Err(e) => return -(e as i64),
        }
    } else {
        let r = match vfs::resolve(dirfd, path, false) {
            Ok(r) => r,
            Err(e) => return -(e as i64),
        };
        let mut tmp = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: host path, local buffer.
        let n = unsafe { libc::readlink(r.host.as_ptr(), tmp.as_mut_ptr().cast(), tmp.len()) };
        if n < 0 {
            return -(errno::last() as i64);
        }
        tmp.truncate(n as usize);
        tmp
    };
    let n = target.len().min(size);
    // SAFETY: guest buffer of `size` bytes.
    unsafe { std::ptr::copy_nonoverlapping(target.as_ptr(), buf as *mut u8, n) };
    n as i64
}

pub fn faccessat(dirfd: u64, path: u64, mode: u64, flags: u64) -> i64 {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(path) };
    let r = match vfs::resolve(dirfd as i32, p, flags & AT_SYMLINK_NOFOLLOW == 0) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if super::binder::is_device(&r.guest) {
        return 0;
    }
    let hflags = if flags & AT_EACCESS != 0 {
        libc::AT_EACCESS
    } else {
        0
    };
    // SAFETY: host path.
    errno::check(
        unsafe { libc::faccessat(libc::AT_FDCWD, r.host.as_ptr(), mode as i32, hflags) } as i64,
    )
}

pub fn getcwd(a: [u64; 6]) -> i64 {
    let cwd = vfs::cwd();
    if cwd.len() + 1 > a[1] as usize {
        return -(ERANGE as i64);
    }
    // SAFETY: guest buffer of a[1] bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(cwd.as_ptr(), a[0] as *mut u8, cwd.len());
        (a[0] as *mut u8).add(cwd.len()).write(0);
    }
    (cwd.len() + 1) as i64
}

pub fn chdir(a: [u64; 6]) -> i64 {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(a[0]) };
    let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, p, true) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path and local buffer.
    if unsafe { libc::stat(r.host.as_ptr(), &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return -20; // ENOTDIR
    }
    vfs::set_cwd(r.guest);
    0
}

/// pipe2(fds, flags): O_CLOEXEC and O_NONBLOCK. Packet mode (O_DIRECT)
/// has no Darwin pipe equivalent.
pub fn pipe2(a: [u64; 6]) -> i64 {
    let flags = a[1];
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return -(EINVAL as i64);
    }
    let mut fds = [0i32; 2];
    // SAFETY: a local array, then flags on our new fds.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) < 0 {
            return -(errno::last() as i64);
        }
        for fd in fds {
            if flags & O_CLOEXEC != 0 {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            if flags & O_NONBLOCK != 0 {
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
            }
        }
        // SAFETY: guest int[2].
        (a[0] as *mut [i32; 2]).write_unaligned(fds);
    }
    0
}

pub fn dup(a: [u64; 6]) -> i64 {
    // SAFETY: plain dup.
    errno::check(unsafe { libc::dup(a[0] as i32) } as i64)
}

pub fn dup3(a: [u64; 6]) -> i64 {
    let (old, new, flags) = (a[0] as i32, a[1] as i32, a[2]);
    if old == new {
        return -(EINVAL as i64);
    }
    // SAFETY: plain dup2/fcntl.
    let r = unsafe { libc::dup2(old, new) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    if flags & O_CLOEXEC != 0 {
        unsafe { libc::fcntl(new, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    r as i64
}

pub fn fcntl(a: [u64; 6]) -> i64 {
    let (fd, cmd, arg) = (a[0] as i32, a[1], a[2]);
    // SAFETY: fcntl with integer arguments.
    unsafe {
        match cmd {
            0 => errno::check(libc::fcntl(fd, libc::F_DUPFD, arg as i32) as i64),
            1030 => errno::check(libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, arg as i32) as i64),
            1 => errno::check(libc::fcntl(fd, libc::F_GETFD) as i64),
            2 => errno::check(libc::fcntl(fd, libc::F_SETFD, arg as i32 & libc::FD_CLOEXEC) as i64),
            3 => {
                let r = libc::fcntl(fd, libc::F_GETFL);
                if r < 0 {
                    -(errno::last() as i64)
                } else {
                    open_flags_from_host(r) as i64
                }
            }
            4 => errno::check(libc::fcntl(
                fd,
                libc::F_SETFL,
                open_flags_to_host(arg) & !libc::O_ACCMODE,
            ) as i64),
            _ => {
                if libc::fcntl(fd, libc::F_GETFD) < 0 {
                    -(EBADF as i64)
                } else {
                    -(EINVAL as i64)
                }
            }
        }
    }
}

const TCGETS: u64 = 0x5401;
const TIOCGWINSZ: u64 = 0x5413;

pub fn ioctl(a: [u64; 6]) -> i64 {
    let (fd, req, arg) = (a[0] as i32, a[1], a[2]);
    if let Some(r) = super::binder::ioctl(fd, req, arg) {
        return r;
    }
    // SAFETY: isatty/ioctl on a guest fd.
    let tty = unsafe { libc::isatty(fd) } == 1;
    match req {
        TCGETS | TIOCGWINSZ if !tty => -(ENOTTY as i64),
        TIOCGWINSZ => {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } < 0 {
                return -(errno::last() as i64);
            }
            // struct winsize has the same layout on both kernels.
            unsafe { (arg as *mut libc::winsize).write_unaligned(ws) };
            0
        }
        TCGETS => {
            // Report a tty with a zeroed Linux termios (60 bytes); translation is future work.
            unsafe { std::ptr::write_bytes(arg as *mut u8, 0, 60) };
            0
        }
        _ => -(ENOTTY as i64),
    }
}
