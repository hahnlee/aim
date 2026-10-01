//! File descriptor syscalls: open, read/write, stat, fcntl, ioctl. Guest
//! fds are host fds; paths go through the guest root (`vfs`); flags and
//! `struct stat` are translated between Linux and Darwin. Fds with Linux
//! state of their own (`fdtab`) dispatch to their owner.

use std::borrow::Cow;
use std::ffi::CString;

use super::fdtab::{self, Kind};
use super::{attrs, dir, event, inotify, memfd, net, space};
use crate::errno::{self, EBADF, EINVAL, ENOENT, ENOTTY, ERANGE};
use crate::sys::{guest_cstr, procfs};
use crate::vfs;

// Linux arm64 open flags.
pub(super) const O_ACCMODE: u64 = 0o3;
pub(super) const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_NOCTTY: u64 = 0o400;
pub(super) const O_TRUNC: u64 = 0o1000;
const O_APPEND: u64 = 0o2000;
pub(super) const O_NONBLOCK: u64 = 0o4000;
const O_DSYNC: u64 = 0o10000;
const O_DIRECTORY: u64 = 0o40000;
const O_NOFOLLOW: u64 = 0o100000;
const O_LARGEFILE: u64 = 0o400000;
pub(super) const O_CLOEXEC: u64 = 0o2000000;
const O_SYNC: u64 = 0o4010000;
const O_PATH: u64 = 0o10000000;
const O_TMPFILE: u64 = 0o20000000;

// Linux *at() flags.
pub(super) const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_EACCESS: u64 = 0x200;
pub(super) const AT_EMPTY_PATH: u64 = 0x1000;

const EISDIR: i64 = 21;
const ESPIPE: i64 = 29;
const EROFS: i64 = 30;
const EPERM: i64 = 1;

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
    let mut f = (h & libc::O_ACCMODE) as u64 | O_LARGEFILE;
    for (l, d) in [(O_APPEND, libc::O_APPEND), (O_NONBLOCK, libc::O_NONBLOCK)] {
        if h & d != 0 {
            f |= l;
        }
    }
    f
}

/// Refuse modifying the read-only image.
pub(super) fn check_writable(r: &vfs::Resolved) -> Result<(), i64> {
    if r.read_only() { Err(-EROFS) } else { Ok(()) }
}

pub fn openat(a: [u64; 6]) -> i64 {
    let (dirfd, flags, mode) = (a[0] as i32, a[2], a[3]);
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    if flags & O_TMPFILE != 0 {
        // open(2): O_TMPFILE comes with O_DIRECTORY, without O_CREAT, and
        // with write access.
        if flags & (O_TMPFILE | O_DIRECTORY | O_CREAT) != O_TMPFILE | O_DIRECTORY
            || flags & O_ACCMODE == 0
        {
            return -(EINVAL as i64);
        }
        return match vfs::resolve(dirfd, path, flags & O_NOFOLLOW == 0) {
            Ok(r) => super::tmpfile::open(
                &r,
                flags & O_EXCL != 0,
                open_flags_to_host(flags & !(O_DIRECTORY | O_EXCL)),
                mode,
            ),
            Err(e) => -(e as i64),
        };
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
    if let Some(fd) = super::ashmem::open(&r.guest, flags) {
        return fd;
    }
    if let Some(fd) = super::evdev::open(&r, flags) {
        return fd;
    }
    if let Some(fd) = super::selinuxfs::open(&r.guest, flags) {
        return fd;
    }
    let hflags = open_flags_to_host(flags);
    if let Some(fd) = procfs::open(&r.guest, flags, hflags) {
        return fd;
    }
    let creating = flags & O_CREAT != 0 && attrs::absent(&r.host);
    if flags & (O_CREAT | O_TRUNC) != 0 || flags & O_ACCMODE != 0 {
        // Opening an existing file in the image for reading only is fine;
        // anything that could modify it is not.
        if let Err(e) = check_writable(&r) {
            return e;
        }
    }
    // /dev/kmsg is a regular file in the runtime /dev (guest-init contract,
    // section 7): every write is a record appended to the log.
    let hflags = if r.guest == "/dev/kmsg" {
        hflags | libc::O_APPEND
    } else {
        hflags
    };
    // An original ELF with a translation-cache entry is opened as the
    // translated file, before the guest reads its headers.
    if let Some(fd) = crate::xrt::open_translated(&r.host, &r.guest, hflags) {
        return fd as i64;
    }
    // SAFETY: host path from the resolver.
    let fd = unsafe { libc::open(r.host.as_ptr(), hflags, mode as libc::c_uint) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    if creating {
        attrs::created(attrs::Host::Fd(fd), || r.guest.clone());
    }
    // Or it is replaced by the translated file here.
    crate::xrt::on_open(fd, &r.host, &r.guest, hflags);
    if matches!(r.guest.as_str(), "/dev/random" | "/dev/urandom") {
        super::random::adopt(fd);
    }
    fd as i64
}

pub fn close(a: [u64; 6]) -> i64 {
    let fd = a[0] as i32;
    if fdtab::is_hidden(fd) {
        return -(EBADF as i64);
    }
    fdtab::on_close(fd);
    // SAFETY: closing a guest fd.
    errno::check(unsafe { libc::close(fd) } as i64)
}

/// The first non-empty buffer of an iovec list.
fn first(iov: &[libc::iovec]) -> (u64, usize) {
    iov.iter()
        .find(|v| v.iov_len > 0)
        .map_or((0, 0), |v| (v.iov_base as u64, v.iov_len))
}

/// read/readv on an fd with Linux state. None: a plain host fd.
fn special_read(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    let k = fdtab::get(fd)?;
    let (buf, len) = first(iov);
    match k {
        Kind::Event(_) | Kind::Timer(_) => event::read(fd, buf, len),
        Kind::Inotify(_) => inotify::read(fd, buf, len),
        Kind::Evdev(_) => super::evdev::read(fd, buf, len),
        Kind::Sock(_) => net::read(fd, iov),
        Kind::Dir(_) => Some(-EISDIR),
        Kind::Epoll(_) | Kind::SyncFile | Kind::Binder(_) => Some(-(EINVAL as i64)),
        Kind::Content | Kind::Knob(_) | Kind::Random => None,
        Kind::Memfd(_) => {
            let mut total = 0i64;
            for v in iov {
                match memfd::rw(fd, v.iov_base as u64, v.iov_len, None, false)? {
                    n if n < 0 => return Some(if total > 0 { total } else { n }),
                    n => {
                        total += n;
                        if (n as usize) < v.iov_len {
                            break;
                        }
                    }
                }
            }
            Some(total)
        }
    }
}

fn special_write(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    let k = fdtab::get(fd)?;
    let (buf, len) = first(iov);
    match k {
        Kind::Event(_) | Kind::Timer(_) => event::write(fd, buf, len),
        Kind::Sock(_) => net::write(fd, iov),
        Kind::Evdev(_) => super::evdev::write(fd, buf, len),
        Kind::Dir(_) => Some(-(EBADF as i64)),
        Kind::Epoll(_) | Kind::Inotify(_) | Kind::SyncFile | Kind::Binder(_) => {
            Some(-(EINVAL as i64))
        }
        Kind::Content => None,
        Kind::Knob(k) => Some(super::knob::write(fd, &k, iov)),
        Kind::Random => Some(super::random::write(iov)),
        Kind::Memfd(_) => {
            if memfd::write_sealed(fd) {
                return Some(-EPERM);
            }
            let mut total = 0i64;
            for v in iov {
                let n = memfd::rw(fd, v.iov_base as u64, v.iov_len, None, true)?;
                if n < 0 {
                    return Some(if total > 0 { total } else { n });
                }
                total += n;
            }
            Some(total)
        }
    }
}

/// pread/pwrite on an fd with Linux state. None: a plain host fd.
fn special_pio(fd: i32, buf: u64, len: usize, pos: i64, write: bool) -> Option<i64> {
    match fdtab::get(fd)? {
        Kind::Memfd(_) => {
            if write && memfd::write_sealed(fd) {
                return Some(-EPERM);
            }
            memfd::rw(fd, buf, len, Some(pos), write)
        }
        Kind::Dir(_) if !write => Some(-EISDIR),
        Kind::Binder(_) => Some(-(EINVAL as i64)),
        Kind::Content => None,
        Kind::Knob(k) if write => Some(super::knob::write(fd, &k, &one(buf, len))),
        // SAFETY: guest buffer.
        Kind::Knob(_) => Some(errno::check(
            unsafe { libc::pread(fd, buf as *mut _, len, pos) } as i64,
        )),
        Kind::Random if write => Some(super::random::write(&one(buf, len))),
        Kind::Random => None,
        _ => Some(-ESPIPE),
    }
}

fn one(buf: u64, len: usize) -> [libc::iovec; 1] {
    [libc::iovec {
        iov_base: buf as *mut _,
        iov_len: len,
    }]
}

pub fn read(a: [u64; 6]) -> i64 {
    let (fd, buf, len) = (a[0] as i32, a[1], a[2] as usize);
    if let Some(r) = special_read(fd, &one(buf, len)) {
        return r;
    }
    // SAFETY: guest buffer.
    errno::check(unsafe { libc::read(fd, buf as *mut _, len) } as i64)
}

pub fn write(a: [u64; 6]) -> i64 {
    let (fd, buf, len) = (a[0] as i32, a[1], a[2] as usize);
    if let Some(r) = special_write(fd, &one(buf, len)) {
        return r;
    }
    let len = match space::charge(fd, len as u64) {
        Ok(n) => n as usize,
        Err(e) => return e,
    };
    // SAFETY: guest buffer.
    errno::check(unsafe { libc::write(fd, buf as *const _, len) } as i64)
}

pub fn pread64(a: [u64; 6]) -> i64 {
    if let Some(r) = special_pio(a[0] as i32, a[1], a[2] as usize, a[3] as i64, false) {
        return r;
    }
    // SAFETY: guest buffer.
    errno::check(
        unsafe { libc::pread(a[0] as i32, a[1] as *mut _, a[2] as usize, a[3] as i64) } as i64,
    )
}

pub fn pwrite64(a: [u64; 6]) -> i64 {
    if let Some(r) = special_pio(a[0] as i32, a[1], a[2] as usize, a[3] as i64, true) {
        return r;
    }
    let len = match space::charge(a[0] as i32, a[2]) {
        Ok(n) => n as usize,
        Err(e) => return e,
    };
    // SAFETY: guest buffer.
    errno::check(unsafe { libc::pwrite(a[0] as i32, a[1] as *const _, len, a[3] as i64) } as i64)
}

fn iovs(ptr: u64, n: u64) -> Result<&'static [libc::iovec], i64> {
    if n > 1024 {
        return Err(-(EINVAL as i64));
    }
    if n == 0 {
        return Ok(&[]);
    }
    // SAFETY: guest iovec array; struct iovec is { void *base; size_t len; }
    // on both kernels.
    Ok(unsafe { std::slice::from_raw_parts(ptr as *const libc::iovec, n as usize) })
}

pub fn readv(a: [u64; 6]) -> i64 {
    let v = match iovs(a[1], a[2]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Some(r) = special_read(a[0] as i32, v) {
        return r;
    }
    // SAFETY: guest iovec array.
    errno::check(unsafe { libc::readv(a[0] as i32, v.as_ptr(), v.len() as i32) } as i64)
}

pub fn writev(a: [u64; 6]) -> i64 {
    let v = match iovs(a[1], a[2]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Some(r) = special_write(a[0] as i32, v) {
        return r;
    }
    let v = match charge_iov(a[0] as i32, v) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // SAFETY: guest iovec array.
    errno::check(unsafe { libc::writev(a[0] as i32, v.as_ptr(), v.len() as i32) } as i64)
}

/// `v` cut to what [`space::charge`] lets a write to `fd` put on its volume.
fn charge_iov(fd: i32, v: &[libc::iovec]) -> Result<Cow<'_, [libc::iovec]>, i64> {
    let total = v
        .iter()
        .try_fold(0u64, |t, io| t.checked_add(io.iov_len as u64))
        .filter(|&t| t <= isize::MAX as u64);
    // A total past SSIZE_MAX is the host call's EINVAL.
    let Some(total) = total else {
        return Ok(Cow::Borrowed(v));
    };
    let mut left = space::charge(fd, total)?;
    if left == total {
        return Ok(Cow::Borrowed(v));
    }
    let mut out = Vec::new();
    for io in v {
        if left == 0 {
            break;
        }
        let n = (io.iov_len as u64).min(left);
        out.push(libc::iovec {
            iov_base: io.iov_base,
            iov_len: n as usize,
        });
        left -= n;
    }
    Ok(Cow::Owned(out))
}

/// preadv/pwritev (69/70) and preadv2/pwritev2 (286/287; flags ignored).
pub fn preadv(write: bool, a: [u64; 6]) -> i64 {
    let (fd, pos) = (a[0] as i32, a[3] as i64);
    let v = match iovs(a[1], a[2]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if pos == -1 {
        return if write { writev(a) } else { readv(a) };
    }
    if fdtab::get(fd).is_some_and(|k| !matches!(k, Kind::Content)) {
        let mut total = 0i64;
        for io in v {
            let r = match special_pio(fd, io.iov_base as u64, io.iov_len, pos + total, write) {
                Some(r) => r,
                None => break,
            };
            if r < 0 {
                return if total > 0 { total } else { r };
            }
            total += r;
            if (r as usize) < io.iov_len {
                break;
            }
        }
        return total;
    }
    let v = if write {
        match charge_iov(fd, v) {
            Ok(v) => v,
            Err(e) => return e,
        }
    } else {
        Cow::Borrowed(v)
    };
    // SAFETY: guest iovec array.
    errno::check(unsafe {
        if write {
            libc::pwritev(fd, v.as_ptr(), v.len() as i32, pos)
        } else {
            libc::preadv(fd, v.as_ptr(), v.len() as i32, pos)
        }
    } as i64)
}

pub fn lseek(a: [u64; 6]) -> i64 {
    // SEEK_SET/CUR/END agree; Linux SEEK_DATA/HOLE are 3/4, Darwin 4/3.
    let whence = match a[2] {
        3 => libc::SEEK_DATA,
        4 => libc::SEEK_HOLE,
        w => w as i32,
    };
    if let Some(r) = dir::lseek(a[0] as i32, a[1] as i64, whence) {
        return r;
    }
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
        // The guest's view (attrs::apply): the host owner is never the
        // guest's.
        st_uid: st.st_uid,
        st_gid: st.st_gid,
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

/// The host inode of a resolved path, whose attributes `attrs` reads.
fn attrs_host(r: &vfs::Resolved) -> attrs::Host<'_> {
    if r.read_only() {
        attrs::Host::Image(&r.host)
    } else {
        attrs::Host::Path(&r.host)
    }
}

/// Host stat of an fd, with the guest's ownership view.
fn stat_fd(fd: i32) -> Result<libc::stat, i64> {
    // A synthesized /proc or /sys directory reports what its path does
    // (bionic's realpath compares the two).
    if let Some(guest) = dir::synthesized_path(fd)
        && let Some(s) = procfs::stat(&guest, true)
    {
        return s.map_err(|e| -(e as i64));
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: stat buffer on our stack.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return Err(-(errno::last() as i64));
    }
    if super::ashmem::as_device(&mut st) {
        return Ok(st);
    }
    if super::evdev::fstat(fd, &mut st).is_some() {
        return Ok(st);
    }
    match st.st_mode & libc::S_IFMT {
        libc::S_IFREG | libc::S_IFDIR | libc::S_IFLNK => {
            let guest = || {
                dir::synthesized_path(fd)
                    .or_else(|| procfs::fd_guest_path(fd).ok())
                    .unwrap_or_default()
            };
            attrs::apply(attrs::Host::Fd(fd), guest, &mut st);
        }
        _ => {
            (st.st_uid, st.st_gid) = attrs::ids(attrs::EFFECTIVE);
        }
    }
    Ok(st)
}

pub fn fstat(a: [u64; 6]) -> i64 {
    match stat_fd(a[0] as i32) {
        Ok(st) => {
            put_stat(&st, a[1]);
            0
        }
        Err(e) => e,
    }
}

/// stat of a path relative to a dirfd, with the guest's ownership view.
pub(super) fn stat_at(dirfd: i32, path: &[u8], flags: u64) -> Result<libc::stat, i64> {
    if path.is_empty() {
        return if flags & AT_EMPTY_PATH != 0 {
            stat_fd(dirfd)
        } else {
            Err(-(ENOENT as i64))
        };
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // linker64 finds the program through /proc/self/exe, whatever argv[0] is.
    if procfs::is_self_exe(path) && follow {
        let p = CString::new(crate::sys::process::exe_host_path()).map_err(|_| -(ENOENT as i64))?;
        // SAFETY: host path and local stat buffer.
        if unsafe { libc::stat(p.as_ptr(), &mut st) } < 0 {
            return Err(-(errno::last() as i64));
        }
        attrs::apply(
            attrs::Host::Path(&p),
            crate::sys::process::exe_guest_path,
            &mut st,
        );
        return Ok(st);
    }
    let r = vfs::resolve(dirfd, path, follow).map_err(|e| -(e as i64))?;
    if let Some(s) = procfs::stat(&r.guest, follow) {
        return s.map_err(|e| -(e as i64));
    }
    if let Some(s) = super::ashmem::stat(&r.guest) {
        return Ok(s);
    }
    // SAFETY: host path and local stat buffer.
    if unsafe { libc::lstat(r.host.as_ptr(), &mut st) } < 0 {
        return Err(-(errno::last() as i64));
    }
    attrs::apply(attrs_host(&r), || r.guest.clone(), &mut st);
    super::evdev::stat(&r, &mut st);
    Ok(st)
}

pub fn newfstatat(a: [u64; 6]) -> i64 {
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    match stat_at(a[0] as i32, path, a[3]) {
        Ok(st) => {
            put_stat(&st, a[2]);
            0
        }
        Err(e) => e,
    }
}

/// Linux `struct statx`, 256 bytes.
#[repr(C)]
#[derive(Default)]
struct Statx {
    mask: u32,
    blksize: u32,
    attributes: u64,
    nlink: u32,
    uid: u32,
    gid: u32,
    mode: u16,
    pad1: u16,
    ino: u64,
    size: u64,
    blocks: u64,
    attributes_mask: u64,
    atime: [i64; 2],
    btime: [i64; 2],
    ctime: [i64; 2],
    mtime: [i64; 2],
    rdev_major: u32,
    rdev_minor: u32,
    dev_major: u32,
    dev_minor: u32,
    mnt_id: u64,
    spare: [u64; 13],
}
const _: () = assert!(std::mem::size_of::<Statx>() == 256);

/// STATX_BASIC_STATS | STATX_BTIME.
const STATX_ALL: u32 = 0x7ff | 0x800;

pub fn statx(a: [u64; 6]) -> i64 {
    let (dirfd, flags, out) = (a[0] as i32, a[2], a[4]);
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    let st = match stat_at(dirfd, path, flags) {
        Ok(st) => st,
        Err(e) => return e,
    };
    let dev = st.st_dev as u32;
    let rdev = st.st_rdev as u32;
    // Timestamps are { i64 sec; u32 nsec; i32 pad }.
    let ts = |s: i64, ns: i64| [s, ns & 0xffff_ffff];
    let x = Statx {
        mask: STATX_ALL,
        blksize: st.st_blksize as u32,
        nlink: st.st_nlink as u32,
        uid: st.st_uid,
        gid: st.st_gid,
        mode: st.st_mode,
        ino: st.st_ino,
        size: st.st_size as u64,
        blocks: st.st_blocks as u64,
        atime: ts(st.st_atime, st.st_atime_nsec),
        btime: ts(st.st_birthtime, st.st_birthtime_nsec),
        ctime: ts(st.st_ctime, st.st_ctime_nsec),
        mtime: ts(st.st_mtime, st.st_mtime_nsec),
        rdev_major: rdev >> 24,
        rdev_minor: rdev & 0xff_ffff,
        dev_major: dev >> 24,
        dev_minor: dev & 0xff_ffff,
        ..Default::default()
    };
    // SAFETY: guest statx buffer.
    unsafe { (out as *mut Statx).write_unaligned(x) };
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

/// The statfs type of a kernel filesystem the path map provides.
fn kernel_fs_magic(fstype: &str) -> Option<u64> {
    Some(match fstype {
        "bpf" => 0xcafe_4a11,
        "cgroup2" => 0x6367_7270,
        "tmpfs" => 0x0102_1994,
        _ => return None,
    })
}

fn put_statfs(s: &libc::statfs, out: u64) {
    put_statfs_as(s, EXT4_SUPER_MAGIC, out);
}

fn put_statfs_as(s: &libc::statfs, f_type: u64, out: u64) {
    let mut flags = 0;
    if s.f_flags & libc::MNT_RDONLY as u32 != 0 {
        flags |= ST_RDONLY;
    }
    if s.f_flags & libc::MNT_NOSUID as u32 != 0 {
        flags |= ST_NOSUID;
    }
    let l = LinuxStatfs {
        f_type,
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
    space::adjust(&mut s);
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
    if r.read_only() {
        s.f_flags |= libc::MNT_RDONLY as u32;
    }
    space::adjust(&mut s);
    let magic = vfs::fstype(&r.guest).and_then(|t| kernel_fs_magic(&t));
    put_statfs_as(&s, magic.unwrap_or(EXT4_SUPER_MAGIC), a[1]);
    0
}

pub fn readlinkat(a: [u64; 6]) -> i64 {
    let (dirfd, buf, size) = (a[0] as i32, a[2], a[3] as usize);
    // SAFETY: guest path pointer.
    let path = unsafe { guest_cstr(a[1]) };
    if size == 0 {
        return -(EINVAL as i64);
    }
    let r = match vfs::resolve(dirfd, path, false) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    let target: Vec<u8> = if let Some(t) = procfs::readlink(r.guest.as_bytes()) {
        match t {
            Ok(t) => t,
            Err(e) => return -(e as i64),
        }
    } else {
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

const R_OK: u64 = 4;
const W_OK: u64 = 2;

pub fn faccessat(dirfd: u64, path: u64, mode: u64, flags: u64) -> i64 {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(path) };
    if mode & !7 != 0 {
        return -(EINVAL as i64);
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let r = match vfs::resolve(dirfd as i32, p, follow) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if super::binder::is_device(&r.guest) || super::ashmem::stat(&r.guest).is_some() {
        return 0;
    }
    if let Some(s) = procfs::stat(&r.guest, follow) {
        return match s {
            Ok(st) if mode & W_OK != 0 && st.st_mode & 0o222 == 0 => -13,
            Ok(_) => 0,
            Err(e) => -(e as i64),
        };
    }
    if mode & W_OK != 0 && r.read_only() {
        return -EROFS;
    }
    if !attrs::recording() {
        // No guest owners: the host's answer.
        let hflags = if flags & AT_EACCESS != 0 {
            libc::AT_EACCESS
        } else {
            0
        };
        // SAFETY: host path.
        let h = unsafe { libc::faccessat(libc::AT_FDCWD, r.host.as_ptr(), mode as i32, hflags) };
        return if h < 0 { -(errno::last() as i64) } else { 0 };
    }
    // The file exists, and the guest identity passes the recorded owner
    // and mode. A host access(2) would only add the host user's view, and
    // costs a security check a stat does not (#446).
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: host path, local buffer.
    let got = unsafe {
        if follow {
            libc::stat(r.host.as_ptr(), &mut st)
        } else {
            libc::lstat(r.host.as_ptr(), &mut st)
        }
    };
    if got < 0 {
        return -(errno::last() as i64);
    }
    if mode & (R_OK | W_OK | 1) != 0 {
        attrs::apply(attrs_host(&r), || r.guest.clone(), &mut st);
        let (uid, gid) = attrs::ids(if flags & AT_EACCESS != 0 {
            attrs::EFFECTIVE
        } else {
            attrs::REAL
        });
        if !attrs::permits(&st, mode as u32, uid, gid) {
            return -13; // EACCES
        }
    }
    0
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

fn set_cwd_checked(r: vfs::Resolved) -> i64 {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if let Some(s) = procfs::stat(&r.guest, true) {
        match s {
            Ok(s) => st = s,
            Err(e) => return -(e as i64),
        }
    // SAFETY: host path and local buffer.
    } else if unsafe { libc::stat(r.host.as_ptr(), &mut st) } < 0 {
        return -(errno::last() as i64);
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return -20; // ENOTDIR
    }
    vfs::set_cwd(r.guest);
    0
}

pub fn chdir(a: [u64; 6]) -> i64 {
    // SAFETY: guest path pointer.
    let p = unsafe { guest_cstr(a[0]) };
    match vfs::resolve(vfs::LINUX_AT_FDCWD, p, true) {
        Ok(r) => set_cwd_checked(r),
        Err(e) => -(e as i64),
    }
}

pub fn fchdir(a: [u64; 6]) -> i64 {
    match vfs::resolve(a[0] as i32, b".", true) {
        Ok(r) => set_cwd_checked(r),
        Err(e) => -(e as i64),
    }
}

pub fn dup(a: [u64; 6]) -> i64 {
    // SAFETY: plain dup.
    let r = unsafe { libc::dup(a[0] as i32) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    fdtab::on_dup(a[0] as i32, r);
    r as i64
}

pub fn dup3(a: [u64; 6]) -> i64 {
    let (old, new, flags) = (a[0] as i32, a[1] as i32, a[2]);
    if old == new || flags & !O_CLOEXEC != 0 {
        return -(EINVAL as i64);
    }
    if fdtab::is_hidden(old) {
        return -(EBADF as i64);
    }
    if fdtab::is_hidden(new) {
        event::relocate_hidden(new);
    }
    // SAFETY: plain dup2/fcntl.
    let r = unsafe { libc::dup2(old, new) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    fdtab::on_dup(old, new);
    if flags & O_CLOEXEC != 0 {
        unsafe { libc::fcntl(new, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    r as i64
}

pub fn pipe2(a: [u64; 6]) -> i64 {
    const O_DIRECT: u64 = 0o200000;
    let (out, flags) = (a[0], a[1]);
    if flags & !(O_CLOEXEC | O_NONBLOCK | O_DIRECT) != 0 {
        return -(EINVAL as i64);
    }
    let mut fds = [0i32; 2];
    // SAFETY: pipe into a local array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return -(errno::last() as i64);
    }
    for fd in fds {
        fdtab::set_flags(fd, flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0);
    }
    // SAFETY: guest int[2].
    unsafe { (out as *mut [i32; 2]).write_unaligned(fds) };
    0
}

// Linux fcntl commands.
const F_DUPFD: u64 = 0;
const F_GETFD: u64 = 1;
const F_SETFD: u64 = 2;
const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;
const F_GETLK: u64 = 5;
const F_SETLK: u64 = 6;
const F_SETLKW: u64 = 7;
const F_SETOWN: u64 = 8;
const F_GETOWN: u64 = 9;
const F_OFD_GETLK: u64 = 36;
const F_OFD_SETLK: u64 = 37;
const F_OFD_SETLKW: u64 = 38;
const F_DUPFD_CLOEXEC: u64 = 1030;
const F_SETPIPE_SZ: u64 = 1031;
const F_GETPIPE_SZ: u64 = 1032;
const F_ADD_SEALS: u64 = 1033;
const F_GET_SEALS: u64 = 1034;

// Darwin's open-file-description locks (sys/fcntl.h, private).
const DARWIN_F_OFD_SETLK: i32 = 90;
const DARWIN_F_OFD_SETLKW: i32 = 91;
const DARWIN_F_OFD_GETLK: i32 = 92;

/// Linux arm64 `struct flock` { short type, whence; off_t start, len;
/// pid_t pid; }.
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxFlock {
    l_type: i16,
    l_whence: i16,
    _pad: i32,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
    _pad2: i32,
}

fn lock(fd: i32, cmd: u64, arg: u64) -> i64 {
    // SAFETY: guest struct flock.
    let mut l = unsafe { (arg as *const LinuxFlock).read_unaligned() };
    let ty = match l.l_type {
        0 => libc::F_RDLCK,
        1 => libc::F_WRLCK,
        2 => libc::F_UNLCK,
        _ => return -(EINVAL as i64),
    };
    let mut h = libc::flock {
        l_start: l.l_start,
        l_len: l.l_len,
        l_pid: 0,
        l_type: ty,
        l_whence: l.l_whence,
    };
    let hcmd = match cmd {
        F_GETLK => libc::F_GETLK,
        F_SETLK => libc::F_SETLK,
        F_SETLKW => libc::F_SETLKW,
        F_OFD_GETLK => DARWIN_F_OFD_GETLK,
        F_OFD_SETLK => DARWIN_F_OFD_SETLK,
        _ => DARWIN_F_OFD_SETLKW,
    };
    // SAFETY: fcntl with a local struct flock.
    if unsafe { libc::fcntl(fd, hcmd, &mut h) } < 0 {
        return -(errno::last() as i64);
    }
    if matches!(cmd, F_GETLK | F_OFD_GETLK) {
        l.l_type = match h.l_type as i32 {
            t if t == libc::F_RDLCK as i32 => 0,
            t if t == libc::F_WRLCK as i32 => 1,
            _ => 2,
        };
        l.l_start = h.l_start;
        l.l_len = h.l_len;
        l.l_whence = h.l_whence;
        l.l_pid = if cmd == F_OFD_GETLK { -1 } else { h.l_pid };
        // SAFETY: guest struct flock.
        unsafe { (arg as *mut LinuxFlock).write_unaligned(l) };
    }
    0
}

fn dup_from(fd: i32, min: i32, cloexec: bool) -> i64 {
    let cmd = if cloexec {
        libc::F_DUPFD_CLOEXEC
    } else {
        libc::F_DUPFD
    };
    // SAFETY: plain fcntl.
    let r = unsafe { libc::fcntl(fd, cmd, min) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    fdtab::on_dup(fd, r);
    r as i64
}

pub fn fcntl(a: [u64; 6]) -> i64 {
    let (fd, cmd, arg) = (a[0] as i32, a[1], a[2]);
    // SAFETY: fcntl with integer arguments.
    unsafe {
        match cmd {
            F_DUPFD => dup_from(fd, arg as i32, false),
            F_DUPFD_CLOEXEC => dup_from(fd, arg as i32, true),
            F_GETFD => errno::check(libc::fcntl(fd, libc::F_GETFD) as i64),
            F_SETFD => {
                errno::check(libc::fcntl(fd, libc::F_SETFD, arg as i32 & libc::FD_CLOEXEC) as i64)
            }
            F_GETFL => {
                let r = libc::fcntl(fd, libc::F_GETFL);
                if r < 0 {
                    -(errno::last() as i64)
                } else {
                    open_flags_from_host(r) as i64
                }
            }
            F_SETFL => errno::check(libc::fcntl(
                fd,
                libc::F_SETFL,
                open_flags_to_host(arg) & !libc::O_ACCMODE,
            ) as i64),
            F_GETLK | F_SETLK | F_SETLKW | F_OFD_GETLK | F_OFD_SETLK | F_OFD_SETLKW => {
                lock(fd, cmd, arg)
            }
            F_GETOWN => errno::check(libc::fcntl(fd, libc::F_GETOWN) as i64),
            F_SETOWN => errno::check(libc::fcntl(fd, libc::F_SETOWN, arg as i32) as i64),
            // Pipe capacity is fixed on Darwin; report the request as met.
            F_SETPIPE_SZ | F_GETPIPE_SZ => {
                if libc::fcntl(fd, libc::F_GETFD) < 0 {
                    -(EBADF as i64)
                } else if cmd == F_SETPIPE_SZ {
                    (arg as i64).max(65536)
                } else {
                    65536
                }
            }
            F_ADD_SEALS => memfd::add_seals(fd, arg as u32),
            F_GET_SEALS => memfd::get_seals(fd),
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

const FIONREAD: u64 = 0x541b;
const FIONBIO: u64 = 0x5421;
const FIONCLEX: u64 = 0x5450;
const FIOCLEX: u64 = 0x5451;

pub fn ioctl(a: [u64; 6]) -> i64 {
    let (fd, req, arg) = (a[0] as i32, a[1], a[2]);
    if let Some(r) = super::binder::ioctl(fd, req, arg) {
        return r;
    }
    if let Some(r) = super::ashmem::ioctl(fd, req, arg) {
        return r;
    }
    if let Some(r) = super::evdev::ioctl(fd, req, arg) {
        return r;
    }
    if let Some(r) = super::netif::ioctl(fd, req, arg) {
        return r;
    }
    if let Some(r) = super::sync_file::ioctl(fd, req, arg) {
        return r;
    }
    // SAFETY: isatty/ioctl on a guest fd with guest argument buffers.
    unsafe {
        // Linux resolves the fd before the request (`ksys_ioctl`). The
        // device handlers above answer only their own open fds.
        if fdtab::is_hidden(fd) || libc::fcntl(fd, libc::F_GETFD) < 0 {
            return -(EBADF as i64);
        }
        if let Some(r) = super::tty::ioctl(fd, req, arg) {
            return r;
        }
        match req {
            FIONREAD => {
                let n = match inotify::pending_bytes(fd) {
                    Some(n) => n as i32,
                    None => {
                        let mut n = 0i32;
                        if libc::ioctl(fd, libc::FIONREAD, &mut n) < 0 {
                            return -(errno::last() as i64);
                        }
                        n
                    }
                };
                (arg as *mut i32).write_unaligned(n);
                0
            }
            FIONBIO => {
                let on = (arg as *const i32).read_unaligned() != 0;
                let fl = libc::fcntl(fd, libc::F_GETFL);
                if fl < 0 {
                    return -(errno::last() as i64);
                }
                let fl = if on {
                    fl | libc::O_NONBLOCK
                } else {
                    fl & !libc::O_NONBLOCK
                };
                errno::check(libc::fcntl(fd, libc::F_SETFL, fl) as i64)
            }
            FIOCLEX | FIONCLEX => errno::check(libc::fcntl(
                fd,
                libc::F_SETFD,
                if req == FIOCLEX { libc::FD_CLOEXEC } else { 0 },
            ) as i64),
            _ => -(ENOTTY as i64),
        }
    }
}

/// close_range(first, last, flags): CLOSE_RANGE_CLOEXEC (4) marks instead
/// of closing; CLOSE_RANGE_UNSHARE (2) has nothing to unshare.
/// execve in place: every guest fd marked close-on-exec is closed, as
/// `close` would.
pub fn close_on_exec() {
    for fd in fdtab::open_fds() {
        // SAFETY: plain fcntl; a closed fd reads as -1.
        if !fdtab::is_hidden(fd) && unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC > 0
        {
            close([fd as u64, 0, 0, 0, 0, 0]);
        }
    }
}

pub fn close_range(a: [u64; 6]) -> i64 {
    let (lo, hi, flags) = (a[0] as u32, a[1] as u32, a[2]);
    if flags & !6 != 0 || lo > hi {
        return -(EINVAL as i64);
    }
    for fd in fdtab::open_fds() {
        let u = fd as u32;
        if u < lo || u > hi || fdtab::is_hidden(fd) {
            continue;
        }
        if flags & 4 != 0 {
            // SAFETY: plain fcntl on a guest fd.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        } else {
            close([fd as u64, 0, 0, 0, 0, 0]);
        }
    }
    0
}
