//! File descriptor syscalls: open, read/write, stat, fcntl, ioctl. Guest
//! fds are host fds; paths go through the guest root (`vfs`); flags and
//! `struct stat` are translated between Linux and Darwin. Fds with Linux
//! state of their own (`fdtab`) dispatch to their owner.

use std::borrow::Cow;
use std::ffi::CString;
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};

use super::fdtab::{self, Kind};
use super::{attrs, dir, event, inotify, memfd, net, space};
use crate::errno::{self, EACCES, EBADF, EINVAL, ENOENT, ENOTTY, ERANGE};
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
pub(super) const O_PATH: u64 = 0o10000000;
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
        return libc::O_EVTONLY
            | if f & O_CLOEXEC != 0 {
                libc::O_CLOEXEC
            } else {
                0
            }
            | if f & O_DIRECTORY != 0 {
                libc::O_DIRECTORY
            } else {
                0
            }
            | if f & O_NOFOLLOW != 0 {
                libc::O_SYMLINK
            } else {
                0
            };
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
    if h & libc::O_EVTONLY != 0 {
        return O_PATH;
    }
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

/// Loader-owned read capability: enforce the original guest's path/DAC view,
/// then keep the actual descriptor private rather than publishing a guest FD.
pub(crate) fn open_kernel_authorized(
    guest: &[u8],
    id: &super::cred::Identity,
) -> Result<aim_storage::private_fd::PrivateFile, errno::Errno> {
    let resolved = resolve_as(vfs::LINUX_AT_FDCWD, guest, true, id, attrs::FS)?;
    open_permissions(&resolved, 0, false, id)?;
    aim_storage::private_fd::PrivateFile::allocate(|| {
        std::fs::File::open(std::path::Path::new(std::ffi::OsStr::from_bytes(
            resolved.host.as_bytes(),
        )))
    })
    .map_err(|error| errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))
}

pub fn openat(a: [u64; 6]) -> i64 {
    openat_as(a, &super::cred::current())
}

fn resolve_as(
    dirfd: i32,
    path: &[u8],
    follow: bool,
    id: &super::cred::Identity,
    kind: usize,
) -> Result<vfs::Resolved, errno::Errno> {
    vfs::resolve_checked(dirfd, path, follow, |directory| {
        attrs::search(directory, id, kind)
    })
}

pub(super) fn openat_as(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    let result = openat_owned(a, id);
    if result >= 0 {
        let _guard = fdtab::lifecycle();
        if let Err(error) = fdtab::publish_guest(result as i32) {
            fdtab::on_close(result as i32);
            unsafe {
                libc::close(result as i32);
            }
            return -(error as i64);
        }
    }
    result
}
fn openat_owned(a: [u64; 6], id: &super::cred::Identity) -> i64 {
    let (dirfd, mut flags, mode) = (a[0] as i32, a[2], a[3]);
    if flags & O_PATH != 0 {
        flags &= O_PATH | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW;
    }
    // SAFETY: guest path pointer.
    let path = match guest_cstr(a[1]) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let path = path.as_slice();
    if flags & O_TMPFILE != 0 {
        // open(2): O_TMPFILE comes with O_DIRECTORY, without O_CREAT, and
        // with write access.
        if flags & (O_TMPFILE | O_DIRECTORY | O_CREAT) != O_TMPFILE | O_DIRECTORY
            || flags & O_ACCMODE == 0
        {
            return -(EINVAL as i64);
        }
        return match resolve_as(dirfd, path, flags & O_NOFOLLOW == 0, id, attrs::FS) {
            Ok(r) => {
                if let Some(route) = vfs::fuse_route(&r.guest) {
                    return super::fuse_client::tmpfile(route, r.guest, flags as u32, mode as u32)
                        .map(|fd| fd as i64)
                        .unwrap_or_else(|error| -(error as i64));
                }
                if attrs::recording() {
                    let stat = match attrs::path_stat(&r.guest) {
                        Ok(stat) => stat,
                        Err(error) => return -(error as i64),
                    };
                    // O_DIRECTORY's lookup fails before any permission check.
                    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
                        return -(errno::ENOTDIR as i64);
                    }
                    if !attrs::permits(&stat, 3, id, attrs::FS) {
                        return -(errno::EACCES as i64);
                    }
                }
                super::tmpfile::open(
                    &r,
                    flags & O_EXCL != 0,
                    open_flags_to_host(flags & !(O_DIRECTORY | O_EXCL)),
                    mode,
                )
            }
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
    let r = match resolve_as(dirfd, path, follow, id, attrs::FS) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if r.guest == "/dev/fuse" {
        return match vfs::runtime_dir() {
            Some(runtime) => super::fuse::open_device(runtime, flags as i32)
                .map(|fd| {
                    super::fuse_device::adopt(fd);
                    fd as i64
                })
                .unwrap_or_else(|error| -(error as i64)),
            None => -(crate::errno::ENODEV as i64),
        };
    }
    if let Some(route) = vfs::fuse_route(&r.guest) {
        return match super::fuse_client::open_fd(route, r.guest.clone(), flags as u32, mode as u32)
        {
            Ok(fd) => {
                if flags & O_PATH != 0 {
                    fdtab::insert(fd, Kind::Path(fdtab::PathDescription::new(flags)));
                }
                fd as i64
            }
            Err(error) => -(error as i64),
        };
    }
    if let Some(fd) = super::binder::open(&r.guest, flags) {
        return fd;
    }
    if let Some(fd) = super::ashmem::open(&r.guest, flags) {
        return fd;
    }
    if let Some(fd) = super::dma_heap::open(&r.guest, flags) {
        return fd;
    }
    if let Some(fd) = super::evdev::open(&r, flags) {
        return fd;
    }
    if let Some(fd) = super::selinuxfs::open(&r.guest, flags) {
        return fd;
    }
    // The kernel log is a character device on Linux: O_TRUNC leaves it.
    // A cgroup hierarchy's files are the kernel's (kernfs): none can be
    // made, and O_TRUNC leaves a control file.
    let cgroup = vfs::cgroup(&r.guest);
    if cgroup.is_some() && flags & O_CREAT != 0 && attrs::absent(&r.host) {
        return -(EACCES as i64);
    }
    // devpts makes no node (it has no `create`): a name no guest pair
    // holds cannot be created, whatever the name.
    let in_devpts = r
        .guest
        .strip_prefix("/dev/pts/")
        .is_some_and(|name| !name.is_empty() && !name.contains('/'));
    if in_devpts
        && flags & O_CREAT != 0
        && super::tty::pts_host(&r.guest).is_none()
        && attrs::absent(&r.host)
    {
        return -(EACCES as i64);
    }
    let flags = if r.guest == "/dev/kmsg" || cgroup.is_some() {
        flags & !O_TRUNC
    } else {
        flags
    };
    let hflags = open_flags_to_host(flags);
    if let Some(fd) = procfs::open(&r.guest, flags, hflags) {
        return fd;
    }
    // Linux publishes a new inode with its owner and mode already set. Keep
    // regular-file lookup/open behind the creator's directory lock until our
    // host inode has its guest metadata. Separate opens (including O_RDONLY)
    // and bind aliases must share this lock across processes.
    if attrs::recording() && !r.read_only() {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let found = unsafe { libc::lstat(r.host.as_ptr(), &mut stat) } == 0;
        // Do not hold a directory lock across a blocking FIFO/device open.
        if found && stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return openat_host(&r, flags, mode, id);
        }
        let parent = std::path::Path::new(std::ffi::OsStr::from_bytes(r.host.as_bytes()))
            .parent()
            .and_then(|path| CString::new(path.as_os_str().as_bytes()).ok());
        let Some(parent) = parent else {
            return -(EINVAL as i64);
        };
        if unsafe { libc::stat(parent.as_ptr(), &mut stat) } < 0 {
            return -(errno::last() as i64);
        }
        return attrs::with_inode_lock(attrs::Host::Path(&parent), &stat, || {
            Ok(openat_host(&r, flags, mode, id))
        })
        .unwrap_or_else(|error| -(error as i64));
    }
    openat_host(&r, flags, mode, id)
}

fn openat_host(r: &vfs::Resolved, flags: u64, mode: u64, id: &super::cred::Identity) -> i64 {
    let hflags = open_flags_to_host(flags);
    let cgroup = vfs::cgroup(&r.guest);
    let pts = super::tty::pts_index(&r.guest);
    let creating = flags & O_CREAT != 0 && attrs::absent(&r.host);
    if flags & (O_CREAT | O_TRUNC) != 0 || flags & O_ACCMODE != 0 {
        // Opening an existing file in the image for reading only is fine;
        // anything that could modify it is not.
        if let Err(e) = check_writable(&r) {
            return e;
        }
    }
    if attrs::recording() {
        if let Err(error) = open_permissions(&r, flags, creating, id) {
            return -(error as i64);
        }
    }
    // An original ELF with a translation-cache entry is opened as the
    // translated file, before the guest reads its headers.
    if let Some(fd) = crate::xrt::open_translated(&r.host, &r.guest, hflags) {
        return fd as i64;
    }
    if flags & O_PATH == 0 && (r.guest == "/dev/ptmx" || pts.is_some()) {
        if flags & O_DIRECTORY != 0 {
            return -(errno::ENOTDIR as i64);
        }
        if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
            return -(errno::EEXIST as i64);
        }
    }
    if flags & O_PATH == 0 && r.guest == "/dev/ptmx" {
        let description = match super::pty_owner::allocate(flags) {
            Ok(Some(description)) => description,
            Ok(None) => return -(errno::ENODEV as i64),
            Err(error) => return -(error as i64),
        };
        use std::os::fd::AsRawFd;
        if let Err(error) =
            super::tty::allocated_owned_master(description.descriptor().as_raw_fd(), id)
        {
            return -(error as i64);
        }
        return super::pty_owner::publish(description, flags)
            .map(|fd| fd as i64)
            .unwrap_or_else(|error| -(error as i64));
    }
    // The guest's devpts node: its owner's pair, never the Darwin slave of
    // the same number (the Mac's own terminals are none of the guest's).
    if let (Some(index), 0) = (pts, flags & O_PATH) {
        return super::pty_owner::open_slave(index, flags)
            .and_then(|description| super::pty_owner::publish(description, flags))
            .map(|fd| fd as i64)
            .unwrap_or_else(|error| -(error as i64));
    }
    // A file the guest creates keeps its owner's access on the host, as
    // mkdirat's does (fsops::host_mode), so its recorded metadata stays
    // readable; the guest's mode is recorded when the host's differs.
    let host_create_mode = if creating && attrs::recording() {
        mode | 0o600
    } else {
        mode
    };
    // SAFETY: host path from the resolver.
    let fd = unsafe {
        libc::open(
            r.host.as_ptr(),
            hflags & !libc::O_TRUNC,
            host_create_mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    if r.guest == "/dev/ptmx" && flags & O_PATH == 0 {
        if let Err(error) = super::tty::allocated_master(fd, id) {
            unsafe {
                libc::close(fd);
            }
            return -(error as i64);
        }
    }
    if creating {
        if let Err(error) = attrs::created(attrs::Host::Fd(fd), || r.guest.clone()) {
            unsafe {
                libc::close(fd);
            }
            return -(error as i64);
        }
        if let Err(error) = record_created_mode(attrs::Host::Fd(fd), &r.guest, mode as u32) {
            unsafe {
                libc::close(fd);
            }
            return -(error as i64);
        }
    }
    if flags & O_PATH != 0 {
        fdtab::insert(fd, Kind::Path(fdtab::PathDescription::new(flags)));
    } else {
        let description = match super::regular_file::adopt(fd, flags) {
            Ok(description) => description,
            Err(error) => {
                unsafe {
                    libc::close(fd);
                }
                return -(error as i64);
            }
        };
        if flags & O_TRUNC != 0 {
            let writable_anchor = if flags & O_ACCMODE == 0 {
                match aim_storage::private_fd::PrivateFile::allocate(|| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                        .open(std::path::Path::new(std::ffi::OsStr::from_bytes(
                            r.host.as_bytes(),
                        )))
                }) {
                    Ok(anchor) => Some(anchor),
                    Err(error) => {
                        unsafe {
                            libc::close(fd);
                        }
                        return -(errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
                            as i64);
                    }
                }
            } else {
                None
            };
            use std::os::fd::{AsFd, AsRawFd};
            let target = writable_anchor
                .as_ref()
                .map(|anchor| anchor.as_raw_fd())
                .unwrap_or(fd);
            if let Some(anchor) = &writable_anchor {
                let original = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
                match (
                    aim_storage::inode_lease::Identity::from_fd(original),
                    aim_storage::inode_lease::Identity::from_fd(anchor.as_fd()),
                ) {
                    (Ok(original), Ok(actual)) if original == actual => {}
                    _ => {
                        unsafe {
                            libc::close(fd);
                        }
                        return -(errno::from_darwin(libc::ESTALE) as i64);
                    }
                }
            }
            let truncation = if let Some(description) = &description {
                let source = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
                match description.store.lock_inode(&source) {
                    Ok(admission) => match admission.writer_lease() {
                        Ok(_lease) => errno::check(unsafe { libc::ftruncate(target, 0) } as i64),
                        Err(error) => {
                            -(match error {
                                aim_storage::fsverity::Error::Linux(error) => error,
                                aim_storage::fsverity::Error::Io(error) => {
                                    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
                                }
                            } as i64)
                        }
                    },
                    Err(error) => {
                        -(match error {
                            aim_storage::fsverity::Error::Linux(error) => error,
                            aim_storage::fsverity::Error::Io(error) => {
                                errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
                            }
                        } as i64)
                    }
                }
            } else {
                errno::check(unsafe { libc::ftruncate(target, 0) } as i64)
            };
            if truncation < 0 {
                unsafe {
                    libc::close(fd);
                }
                return truncation;
            }
        }
        if let Some(description) = description {
            fdtab::insert(fd, Kind::Regular(description));
        }
    }
    // Or it is replaced by the translated file here.
    crate::xrt::on_open(fd, &r.host, &r.guest, hflags);
    if matches!(r.guest.as_str(), "/dev/random" | "/dev/urandom") {
        super::random::adopt(fd);
    }
    if r.guest == "/dev/kmsg" && super::kmsg::adopt(fd) {
        if let Err(error) = super::kmsg::opened(fd, flags) {
            fdtab::on_close(fd);
            // SAFETY: the fd this open made.
            unsafe { libc::close(fd) };
            return -(error as i64);
        }
    }
    if let Some((controller, root)) = cgroup
        && flags & O_PATH == 0
    {
        let host = std::path::Path::new(std::ffi::OsStr::from_bytes(r.host.as_bytes()));
        if host.is_file() {
            super::cgroup::opened(fd, host, controller, root);
        }
    }
    fd as i64
}

/// The mode of a file the guest created with `want`, under the umask,
/// recorded when the host's (which keeps its owner's access) differs.
pub(super) fn record_created_mode(
    host: attrs::Host,
    guest: &str,
    want: u32,
) -> Result<(), errno::Errno> {
    if !attrs::recording() {
        return Ok(());
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let got = unsafe {
        match host {
            attrs::Host::Fd(fd) => libc::fstat(fd, &mut st),
            attrs::Host::Path(p) | attrs::Host::Image(p) => libc::lstat(p.as_ptr(), &mut st),
        }
    };
    if got < 0 {
        return Err(errno::last());
    }
    attrs::apply(host, || guest.into(), &mut st);
    let made = st.st_mode as u32 & 0o7777;
    let wanted = want & 0o7777 & !super::pstate::current_umask();
    let mode = (made & !0o700) | (wanted & 0o700);
    if mode == made {
        return Ok(());
    }
    attrs::record_checked(
        host,
        || guest.into(),
        attrs::Attr {
            mode: Some(mode),
            ..Default::default()
        },
    )
}

fn open_permissions(
    r: &vfs::Resolved,
    flags: u64,
    creating: bool,
    id: &super::cred::Identity,
) -> Result<(), errno::Errno> {
    if creating {
        let parent = std::path::Path::new(&r.guest)
            .parent()
            .ok_or(errno::ENOENT)?;
        let stat = attrs::path_stat(parent.to_str().ok_or(errno::EINVAL)?)?;
        return if attrs::permits(&stat, 3, id, attrs::FS) {
            Ok(())
        } else {
            Err(errno::EACCES)
        };
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::lstat(r.host.as_ptr(), &mut stat) } < 0 {
        return Err(errno::last());
    }
    if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
        return Err(errno::EEXIST);
    }
    if flags & O_PATH != 0 {
        return Ok(());
    }
    if stat.st_mode & libc::S_IFMT == libc::S_IFLNK && flags & O_NOFOLLOW != 0 {
        return Err(errno::ELOOP);
    }
    if flags & O_DIRECTORY != 0 && stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(errno::ENOTDIR);
    }
    attrs::apply(attrs_host(r), || r.guest.clone(), &mut stat);
    let access = flags & O_ACCMODE;
    let mut want = match access {
        0 => 4,
        1 => 2,
        2 => 6,
        _ => 6,
    };
    if flags & O_TRUNC != 0 {
        want |= 2;
    }
    if attrs::permits(&stat, want, id, attrs::FS) {
        Ok(())
    } else {
        Err(errno::EACCES)
    }
}

pub fn close(a: [u64; 6]) -> i64 {
    let mut inner = 0;
    let mut evidence = None;
    let result = super::close_effects::run(|| {
        inner = close_inner(a, &mut evidence);
        inner
    });
    if result != inner {
        if let Some(evidence) = evidence {
            evidence.failure(
                "close-wrapper",
                result,
                if inner == 0 { 0 } else { i32::MIN },
                0,
            );
        }
    }
    result
}
struct CloseEvidence {
    fd: i32,
    visible: bool,
    hidden: bool,
    fd_flags: i32,
    file_flags: i32,
    mode: u32,
    kind: &'static str,
}
impl CloseEvidence {
    fn capture(fd: i32) -> Self {
        let saved = unsafe { *libc::__error() };
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let mode = if unsafe { libc::fstat(fd, &mut stat) } == 0 {
            stat.st_mode as u32
        } else {
            0
        };
        let kind = match fdtab::get(fd) {
            None => "plain",
            Some(Kind::Sock(_)) => "socket",
            Some(Kind::Regular(_)) => "regular",
            Some(Kind::Dir(_)) => "directory",
            Some(Kind::Event(_)) => "event",
            Some(Kind::Timer(_)) => "timer",
            Some(Kind::Epoll(_)) => "epoll",
            Some(Kind::Inotify(_)) => "inotify",
            Some(Kind::Path(_)) => "path",
            Some(_) => "other-typed",
        };
        let evidence = Self {
            fd,
            kind,
            visible: fdtab::visible(fd),
            hidden: fdtab::is_hidden(fd),
            fd_flags: unsafe { libc::fcntl(fd, libc::F_GETFD) },
            file_flags: unsafe { libc::fcntl(fd, libc::F_GETFL) },
            mode,
        };
        unsafe {
            *libc::__error() = saved;
        }
        evidence
    }
    fn failure(&self, stage: &str, result: i64, native_ret: i32, native_errno: i32) {
        super::close_effects::diagnostic(format_args!(
            "stage={stage} fd={} visible={} hidden={} fd_flags={} file_flags={} mode={:#x} kind={} result={result} native_ret={native_ret} native_errno={native_errno}",
            self.fd,
            self.visible,
            self.hidden,
            self.fd_flags,
            self.file_flags,
            self.mode,
            self.kind
        ));
    }
}
fn close_inner(a: [u64; 6], capture: &mut Option<CloseEvidence>) -> i64 {
    let fd = a[0] as i32;
    let guard = match fdtab::close_admission(fd) {
        Ok(guard) => guard,
        Err(error) => return -(error as i64),
    };
    *capture = Some(CloseEvidence::capture(fd));
    let evidence = capture.as_ref().unwrap();
    if let Err(error) = fdtab::require_guest_visible(fd) {
        // An fd that is not open at all is close(2)'s EBADF, as a process
        // closing every descriptor before exec sees it; one the layer
        // holds hidden is the fault the record is for.
        if evidence.hidden || evidence.fd_flags >= 0 {
            evidence.failure("pre-visibility", -(error as i64), i32::MIN, 0);
        }
        return -(error as i64);
    }
    let retained = fdtab::get(fd);
    let file = super::fuse_client::get(fd);
    let socket = matches!(retained, Some(Kind::Sock(_)));
    let posix = match fdtab::guest_close_owner(fd) {
        Ok(owner) => owner,
        Err(error) => {
            evidence.failure("prepare-posix", -(error as i64), i32::MIN, 0);
            return -(error as i64);
        }
    };
    if let Err(error) = fdtab::withdraw_guest(fd) {
        evidence.failure("withdraw", -(error as i64), i32::MIN, 0);
        return -(error as i64);
    }
    fdtab::on_close(fd);
    let native_ret = unsafe { libc::close(fd) };
    let native_errno = if native_ret < 0 {
        unsafe { *libc::__error() }
    } else {
        0
    };
    let result = if native_ret < 0 {
        -(errno::from_darwin(native_errno) as i64)
    } else {
        native_ret as i64
    };
    if result < 0 {
        evidence.failure("native-close", result, native_ret, native_errno);
    }
    let binder_flush = guard.finish(result == 0);
    let posix = if result == 0 {
        fdtab::finish_guest_close(posix)
    } else {
        Ok(())
    };
    let flush = file.as_ref().map(|file| {
        super::fuse_cache::flush_file(file).and_then(|_| super::fuse_client::flush(file))
    });
    drop(retained);
    drop(file);
    if result == 0 && socket {
        super::close_effects::note_socket_close();
    }
    if let Err(error) = posix {
        evidence.failure("finish-posix", -(error as i64), native_ret, native_errno);
        return -(error as i64);
    }
    if let Err(error) = binder_flush {
        if result == 0 {
            evidence.failure("finish-binder", -(error as i64), native_ret, native_errno);
            return -(error as i64);
        }
    }
    match flush {
        Some(Err(error)) if result == 0 => {
            evidence.failure("finish-fuse", -(error as i64), native_ret, native_errno);
            -(error as i64)
        }
        _ => result,
    }
}

/// The first non-empty buffer of an iovec list.
fn first(iov: &[libc::iovec]) -> (u64, usize) {
    iov.iter()
        .find(|v| v.iov_len > 0)
        .map_or((0, 0), |v| (v.iov_base as u64, v.iov_len))
}

pub(super) fn is_path_fd(fd: i32) -> bool {
    matches!(fdtab::get(fd), Some(Kind::Path(_)))
        || super::fuse_client::get(fd).is_some_and(|file| file.flags as u64 & O_PATH != 0)
}

/// read/readv on an fd with Linux state. None: a plain host fd.
fn special_read(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    let pin = match fdtab::pin_guest_io(fd) {
        Ok(pin) => pin,
        Err(error) => return Some(-(error as i64)),
    };
    if let Err(error) = prepare_iov(&pin, iov, false) {
        return Some(-(error as i64));
    }
    use std::os::fd::AsRawFd;
    let actual = pin.descriptor().as_raw_fd();
    Some(special_read_kernel(&pin, iov).unwrap_or_else(|| {
        errno::check(unsafe { libc::readv(actual, iov.as_ptr(), iov.len() as i32) } as i64)
    }))
}
fn special_write(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    let pin = match fdtab::pin_guest_io(fd) {
        Ok(pin) => pin,
        Err(error) => return Some(-(error as i64)),
    };
    if let Err(error) = prepare_iov(&pin, iov, true) {
        return Some(-(error as i64));
    }
    use std::os::fd::AsRawFd;
    let actual = pin.descriptor().as_raw_fd();
    if let Some(Kind::Regular(description)) = pin.kind() {
        if let Err(error) = description.check_write() {
            return Some(-(error as i64));
        }
        let iov = match charge_iov(actual, iov) {
            Ok(iov) => iov,
            Err(error) => return Some(error),
        };
        return Some(description.rw(pin.descriptor(), &iov, None, true));
    }
    Some(special_write_kernel(&pin, iov).unwrap_or_else(|| {
        let iov = match charge_iov(actual, iov) {
            Ok(iov) => iov,
            Err(error) => return error,
        };
        errno::check(unsafe { libc::writev(actual, iov.as_ptr(), iov.len() as i32) } as i64)
    }))
}
/// Whether the host descriptor is O_APPEND, as F_SETFL leaves it.
fn appends(actual: i32) -> bool {
    (unsafe { libc::fcntl(actual, libc::F_GETFL) }) & libc::O_APPEND != 0
}

/// `generic_write_checks`: a positioned write on an O_APPEND description
/// of a regular file is at its end (pwrite(2) BUGS); none for others.
fn append_end(actual: i32) -> Option<Result<i64, i64>> {
    if !appends(actual) {
        return None;
    }
    match stat_fd_kernel(actual) {
        Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFREG => Some(Ok(st.st_size)),
        Ok(_) => None,
        Err(error) => Some(Err(error)),
    }
}

fn special_pio(fd: i32, buf: u64, len: usize, pos: i64, write: bool) -> Option<i64> {
    let pin = match fdtab::pin_guest_io(fd) {
        Ok(pin) => pin,
        Err(error) => return Some(-(error as i64)),
    };
    if let Err(error) = prepare_iov(&pin, &one(buf, len), write) {
        return Some(-(error as i64));
    }
    use std::os::fd::AsRawFd;
    let actual = pin.descriptor().as_raw_fd();
    if let Some(Kind::Regular(description)) = pin.kind() {
        if write {
            if let Err(error) = description.check_write() {
                return Some(-(error as i64));
            }
        }
        let len = if write {
            match space::charge(actual, len as u64) {
                Ok(len) => len as usize,
                Err(error) => return Some(error),
            }
        } else {
            len
        };
        if write && appends(actual) {
            return Some(description.append(pin.descriptor(), &one(buf, len)));
        }
        return Some(description.rw(pin.descriptor(), &one(buf, len), Some(pos), write));
    }
    if let Some(Kind::ProcMem(open)) = pin.kind() {
        return Some(super::proc_mem::rw(
            actual,
            *open,
            &one(buf, len),
            Some(pos as u64),
            write,
        ));
    }
    let pos = match write.then(|| append_end(actual)).flatten() {
        Some(Ok(end)) => end,
        Some(Err(error)) => return Some(error),
        None => pos,
    };
    Some(
        special_pio_kernel(actual, buf, len, pos, write).unwrap_or_else(|| {
            let len = if write {
                match space::charge(actual, len as u64) {
                    Ok(len) => len as usize,
                    Err(error) => return error,
                }
            } else {
                len
            };
            errno::check(unsafe {
                if write {
                    libc::pwrite(actual, buf as *const _, len, pos)
                } else {
                    libc::pread(actual, buf as *mut _, len, pos)
                }
            } as i64)
        }),
    )
}

fn special_read_kernel(pin: &fdtab::Pinned, iov: &[libc::iovec]) -> Option<i64> {
    use std::os::fd::AsRawFd;
    let fd = pin.descriptor().as_raw_fd();
    if is_path_fd(fd) {
        return Some(-(EBADF as i64));
    }
    if let Some(result) = super::fuse_device::rw(fd, iov, false) {
        return Some(result);
    }
    if let Some(error) = super::fuse_client::inherited_error(fd) {
        return Some(-(error as i64));
    }
    if let Some(result) = super::fuse_client::rw(fd, iov, None, false) {
        return Some(result);
    }
    let k = fdtab::get(fd)?;
    let (buf, len) = first(iov);
    match k {
        Kind::Regular(description) => Some(description.rw(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
            iov,
            None,
            false,
        )),
        Kind::ProcMem(open) => Some(super::proc_mem::rw(fd, open, iov, None, false)),
        Kind::Pty(description) => Some(description.rw(iov, false)),
        Kind::Path(_) => Some(-(EBADF as i64)),
        Kind::ProxyFile => Some(super::proxy_file::rw(fd, iov, None, false)),
        Kind::Event(_) | Kind::Timer(_) => event::read(fd, buf, len),
        Kind::Inotify(_) => inotify::read(fd, buf, len),
        Kind::Evdev(_) => super::evdev::read(fd, buf, len),
        Kind::Kmsg(open) => Some(super::kmsg::read(fd, &open, iov)),
        Kind::Cgroup(open) => Some(super::cgroup::read(&open, iov, None)),
        Kind::Sock(_) => net::read_pinned(pin, iov),
        Kind::Dir(_) => Some(-EISDIR),
        Kind::Epoll(_) | Kind::SyncFile | Kind::Binder(_) => Some(-(EINVAL as i64)),
        Kind::Content | Kind::Knob(_) | Kind::Random | Kind::Ashmem => None,
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

fn special_write_kernel(pin: &fdtab::Pinned, iov: &[libc::iovec]) -> Option<i64> {
    use std::os::fd::AsRawFd;
    let fd = pin.descriptor().as_raw_fd();
    if is_path_fd(fd) {
        return Some(-(EBADF as i64));
    }
    if let Some(result) = super::fuse_device::rw(fd, iov, true) {
        return Some(result);
    }
    if let Some(error) = super::fuse_client::inherited_error(fd) {
        return Some(-(error as i64));
    }
    if let Some(result) = super::fuse_client::rw(fd, iov, None, true) {
        return Some(result);
    }
    let k = fdtab::get(fd)?;
    let (buf, len) = first(iov);
    match k {
        Kind::Regular(description) => Some(description.rw(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
            iov,
            None,
            true,
        )),
        Kind::ProcMem(open) => Some(super::proc_mem::rw(fd, open, iov, None, true)),
        Kind::Pty(description) => Some(description.rw(iov, true)),
        Kind::Path(_) => Some(-(EBADF as i64)),
        Kind::ProxyFile => Some(super::proxy_file::rw(fd, iov, None, true)),
        Kind::Event(_) | Kind::Timer(_) => event::write(fd, buf, len),
        Kind::Sock(_) => net::write_pinned(pin, iov),
        Kind::Evdev(_) => super::evdev::write(fd, buf, len),
        Kind::Kmsg(open) => Some(super::kmsg::write(&open, iov)),
        Kind::Cgroup(open) => Some(super::cgroup::write(&open, iov, None)),
        Kind::Dir(_) => Some(-(EBADF as i64)),
        Kind::Epoll(_) | Kind::Inotify(_) | Kind::SyncFile | Kind::Binder(_) => {
            Some(-(EINVAL as i64))
        }
        Kind::Content => None,
        Kind::Knob(k) => Some(super::knob::write(fd, &k, iov)),
        Kind::Random => Some(super::random::write(iov)),
        // `ashmem_fops` has no write.
        Kind::Ashmem => Some(-(EINVAL as i64)),
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
fn special_pio_kernel(fd: i32, buf: u64, len: usize, pos: i64, write: bool) -> Option<i64> {
    if is_path_fd(fd) {
        return Some(-(EBADF as i64));
    }
    if super::fuse_device::is_device(fd) {
        return Some(-29);
    }
    if let Some(result) = super::fuse_client::rw(fd, &one(buf, len), Some(pos), write) {
        return Some(result);
    }
    match fdtab::get(fd)? {
        Kind::Regular(description) => Some(description.rw(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
            &one(buf, len),
            Some(pos),
            write,
        )),
        Kind::ProcMem(open) => Some(super::proc_mem::rw(
            fd,
            open,
            &one(buf, len),
            Some(pos as u64),
            write,
        )),
        Kind::Path(_) => Some(-(EBADF as i64)),
        Kind::ProxyFile => Some(super::proxy_file::rw(fd, &one(buf, len), Some(pos), write)),
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
        Kind::Ashmem if write => Some(-(EINVAL as i64)),
        Kind::Ashmem => None,
        // devkmsg's reads and writes take no position.
        Kind::Kmsg(open) if write => Some(super::kmsg::write(&open, &one(buf, len))),
        Kind::Kmsg(open) => Some(super::kmsg::read(fd, &open, &one(buf, len))),
        Kind::Cgroup(open) if write => Some(super::cgroup::write(
            &open,
            &one(buf, len),
            Some(pos as u64),
        )),
        Kind::Cgroup(open) => Some(super::cgroup::read(&open, &one(buf, len), Some(pos as u64))),
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

fn iovs(ptr: u64, n: u64) -> Result<Vec<libc::iovec>, i64> {
    if n > 1024 {
        return Err(-(EINVAL as i64));
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: guest iovec array; struct iovec is { void *base; size_t len; }
    // on both kernels.
    let bytes =
        super::user_memory::read_exact(ptr, n as usize * 16).map_err(|error| -(error as i64))?;
    Ok(bytes
        .chunks_exact(16)
        .map(|bytes| libc::iovec {
            iov_base: u64::from_le_bytes(bytes[..8].try_into().unwrap()) as *mut _,
            iov_len: u64::from_le_bytes(bytes[8..].try_into().unwrap()) as usize,
        })
        .collect())
}
fn prepare_iov(pin: &fdtab::Pinned, iov: &[libc::iovec], write: bool) -> Result<(), errno::Errno> {
    if matches!(pin.kind(), Some(Kind::Path(_))) {
        return Err(EBADF);
    }
    if !write && matches!(pin.kind(), Some(Kind::Dir(_))) {
        return Err(EISDIR as errno::Errno);
    }
    if let Some(Kind::ProcMem(open)) = pin.kind() {
        let mode = open.mode;
        if mode == 3 || write && mode == 0 || !write && mode == 1 {
            return Err(EBADF);
        }
    } else if let Some(Kind::Regular(description)) = pin.kind() {
        let mode = description.flags & 3;
        if mode == 3 || write && mode == 0 || !write && mode == 1 {
            return Err(EBADF);
        }
    } else {
        use std::os::fd::AsRawFd;
        let flags = unsafe { libc::fcntl(pin.descriptor().as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(errno::last());
        }
        let mode = flags & libc::O_ACCMODE;
        if write && mode == libc::O_RDONLY || !write && mode == libc::O_WRONLY {
            return Err(EBADF);
        }
    }
    let length = iov
        .iter()
        .try_fold(0usize, |sum, vector| sum.checked_add(vector.iov_len))
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or(EINVAL)?;
    if length == 0 {
        return Ok(());
    }
    for vector in iov {
        if write {
            super::user_memory::prepare_read(vector.iov_base as u64, vector.iov_len)?;
        } else {
            super::user_memory::prepare_write(vector.iov_base as u64, vector.iov_len)?;
        }
    }
    Ok(())
}

pub fn readv(a: [u64; 6]) -> i64 {
    let v = match iovs(a[1], a[2]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let v = v.as_slice();
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
    let v = v.as_slice();
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

/// `RWF_*` (`include/uapi/linux/fs.h`, 6.12): `RWF_SUPPORTED` is every
/// bit through `RWF_ATOMIC`.
const RWF_HIPRI: u32 = 0x01;
const RWF_DSYNC: u32 = 0x02;
const RWF_SYNC: u32 = 0x04;
const RWF_NOWAIT: u32 = 0x08;
const RWF_APPEND: u32 = 0x10;
const RWF_NOAPPEND: u32 = 0x20;
const RWF_ATOMIC: u32 = 0x40;
const RWF_SUPPORTED: u32 =
    RWF_HIPRI | RWF_DSYNC | RWF_SYNC | RWF_NOWAIT | RWF_APPEND | RWF_NOAPPEND | RWF_ATOMIC;

/// `kiocb_set_rw_flags`: unknown flags, and the two no descriptor here
/// offers (FMODE_NOWAIT, FMODE_CAN_ATOMIC_WRITE), are EOPNOTSUPP;
/// RWF_HIPRI only polls direct I/O, which buffered I/O ignores.
fn rw_flags(flags: u32) -> Result<(), errno::Errno> {
    if flags & !RWF_SUPPORTED != 0 || flags & (RWF_NOWAIT | RWF_ATOMIC) != 0 {
        return Err(errno::EOPNOTSUPP);
    }
    Ok(())
}

/// preadv/pwritev (69/70) and preadv2/pwritev2 (286/287), whose offset -1
/// is the current position and whose flags apply to this call.
pub fn preadv(write: bool, v2: bool, a: [u64; 6]) -> i64 {
    let fd = a[0] as i32;
    let mut pos = a[3] as i64;
    // do_preadv: a negative offset is EINVAL, before the descriptor.
    if pos < 0 && !(v2 && pos == -1) {
        return -(EINVAL as i64);
    }
    let flags = if v2 { a[5] as u32 } else { 0 };
    let pin = match fdtab::pin_guest_io(fd) {
        Ok(pin) => pin,
        Err(error) => return -(error as i64),
    };
    let vectors = match iovs(a[1], a[2]) {
        Ok(vectors) => vectors,
        Err(error) => return error,
    };
    let vectors = vectors.as_slice();
    if let Err(error) = prepare_iov(&pin, vectors, write) {
        return -(error as i64);
    }
    if let Err(error) = rw_flags(flags) {
        return -(error as i64);
    }
    use std::os::fd::AsRawFd;
    let actual = pin.descriptor().as_raw_fd();
    // After the write: the position a current-position write moves to when
    // it was made at an explicit one.
    let mut advance = None;
    // A positioned write on an O_APPEND description appends, unless
    // RWF_NOAPPEND (pwrite(2) BUGS); a regular description appends
    // atomically, as the host's append writes do.
    if write && pos != -1 && flags & RWF_NOAPPEND == 0 && appends(actual) {
        if let Some(Kind::Regular(description)) = pin.kind() {
            if let Err(error) = description.check_write() {
                return -(error as i64);
            }
            let vectors = match charge_iov(actual, vectors) {
                Ok(vectors) => vectors,
                Err(error) => return error,
            };
            let result = description.append(pin.descriptor(), &vectors);
            return synced(fd, result, flags);
        }
    }
    if write && (flags & (RWF_APPEND | RWF_NOAPPEND) != 0 || pos != -1) {
        // generic_write_checks: IOCB_APPEND (the description's O_APPEND or
        // RWF_APPEND, less RWF_NOAPPEND) writes a regular file at its end.
        let regular = stat_fd(fd)
            .ok()
            .filter(|st| st.st_mode & libc::S_IFMT == libc::S_IFREG);
        let described = appends(actual);
        let append = flags & RWF_APPEND != 0 || described && flags & RWF_NOAPPEND == 0;
        match (regular, append) {
            (Some(st), true) if pos != -1 => pos = st.st_size,
            (Some(st), true) => {
                pos = st.st_size;
                advance = Some(());
            }
            // A current-position write the description would append: at
            // the position instead, which it then moves.
            (Some(_), false) if described && pos == -1 => {
                let current = lseek([fd as u64, 0, libc::SEEK_CUR as u64, 0, 0, 0]);
                if current < 0 {
                    return current;
                }
                pos = current;
                advance = Some(());
            }
            _ => {}
        }
    }
    let result = preadv_at(&pin, vectors, pos, write);
    if result >= 0 && advance.is_some() {
        let moved = lseek([
            fd as u64,
            (pos + result) as u64,
            libc::SEEK_SET as u64,
            0,
            0,
            0,
        ]);
        if moved < 0 {
            return moved;
        }
    }
    if write {
        return synced(fd, result, flags);
    }
    result
}

/// `generic_write_sync`: IOCB_DSYNC syncs the written range, IOCB_SYNC
/// its metadata too.
fn synced(fd: i32, result: i64, flags: u32) -> i64 {
    if result > 0 && flags & (RWF_DSYNC | RWF_SYNC) != 0 {
        let synced = super::fsops::fsync([fd as u64, 0, 0, 0, 0, 0]);
        if synced < 0 {
            return synced;
        }
    }
    result
}

/// The transfer at `pos`, or the current position for -1.
fn preadv_at(pin: &fdtab::Pinned, vectors: &[libc::iovec], pos: i64, write: bool) -> i64 {
    if let Some(Kind::Regular(description)) = pin.kind() {
        if write {
            if let Err(error) = description.check_write() {
                return -(error as i64);
            }
        }
        let vectors = if write {
            match charge_iov(pin.descriptor().as_raw_fd(), vectors) {
                Ok(vectors) => vectors,
                Err(error) => return error,
            }
        } else {
            Cow::Borrowed(vectors)
        };
        return description.rw(
            pin.descriptor(),
            &vectors,
            (pos != -1).then_some(pos),
            write,
        );
    }
    use std::os::fd::AsRawFd;
    let actual = pin.descriptor().as_raw_fd();
    let plain = matches!(pin.kind(), None | Some(Kind::Content) | Some(Kind::Random))
        && super::fuse_client::get(actual).is_none()
        && !super::fuse_device::is_device(actual);
    let vectors = if write && plain {
        match charge_iov(actual, vectors) {
            Ok(vectors) => vectors,
            Err(error) => return error,
        }
    } else {
        Cow::Borrowed(vectors)
    };
    let vectors = &*vectors;
    if pos == -1 {
        return if write {
            special_write_kernel(&pin, vectors).unwrap_or_else(|| {
                errno::check(
                    unsafe { libc::writev(actual, vectors.as_ptr(), vectors.len() as i32) } as i64,
                )
            })
        } else {
            special_read_kernel(&pin, vectors).unwrap_or_else(|| {
                errno::check(
                    unsafe { libc::readv(actual, vectors.as_ptr(), vectors.len() as i32) } as i64,
                )
            })
        };
    }
    let mut total = 0i64;
    for vector in vectors {
        let result = special_pio_kernel(
            actual,
            vector.iov_base as u64,
            vector.iov_len,
            pos + total,
            write,
        )
        .unwrap_or_else(|| {
            errno::check(unsafe {
                if write {
                    libc::pwrite(actual, vector.iov_base, vector.iov_len, pos + total)
                } else {
                    libc::pread(actual, vector.iov_base, vector.iov_len, pos + total)
                }
            } as i64)
        });
        if result < 0 {
            return if total > 0 { total } else { result };
        }
        total += result;
        if (result as usize) < vector.iov_len {
            break;
        }
    }
    total
}

pub fn lseek(a: [u64; 6]) -> i64 {
    let pin = match fdtab::pin_guest_io(a[0] as i32) {
        Ok(pin) => pin,
        Err(error) => return -(error as i64),
    };
    use std::os::fd::AsRawFd;
    if let Some(Kind::Regular(description)) = pin.kind() {
        let whence = match a[2] as i32 {
            3 => 4,
            4 => 3,
            whence => whence,
        };
        return description.seek(pin.descriptor(), a[1] as i64, whence);
    }
    if let Some(Kind::ProcMem(_)) = pin.kind() {
        return super::proc_mem::seek(pin.descriptor().as_raw_fd(), a[1] as i64, a[2] as i32);
    }
    if let Some(Kind::Cgroup(open)) = pin.kind() {
        return super::cgroup::lseek(&open, a[1] as i64, a[2] as i32);
    }
    if let Some(Kind::Kmsg(open)) = pin.kind() {
        return super::kmsg::lseek(
            pin.descriptor().as_raw_fd(),
            &open,
            a[1] as i64,
            a[2] as i32,
        );
    }
    let mut a = a;
    a[0] = pin.descriptor().as_raw_fd() as u64;
    lseek_kernel(a)
}
fn lseek_kernel(a: [u64; 6]) -> i64 {
    if is_path_fd(a[0] as i32) {
        return -(EBADF as i64);
    }
    if super::fuse_device::is_device(a[0] as i32) {
        return -29;
    }
    if let Some(result) = super::fuse_client::seek(a[0] as i32, a[1] as i64, a[2] as u32) {
        return result;
    }
    if super::proxy_file::is_proxy(a[0] as i32) {
        return super::proxy_file::seek(a[0] as i32, a[1] as i64, a[2] as u32);
    }
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

fn put_user_struct<T>(value: &T, out: u64) -> i64 {
    let bytes = unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    };
    super::user_memory::write_exact(out, bytes)
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64))
}

fn put_stat(st: &libc::stat, out: u64) -> i64 {
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
    put_user_struct(&l, out)
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
    let pin = fdtab::pin_guest_io(fd).map_err(|error| -(error as i64))?;
    use std::os::fd::AsRawFd;
    stat_fd_kernel(pin.descriptor().as_raw_fd())
}
pub(super) fn stat_fd_kernel(fd: i32) -> Result<libc::stat, i64> {
    if let Some(Kind::ProcMem(open)) = fdtab::get(fd) {
        return Ok(open.stat());
    }
    if let Some(stat) = super::fuse_device::stat(fd) {
        return Ok(stat);
    }
    if let Some(file) = super::fuse_client::get(fd) {
        return super::fuse_client::stat(
            &file.route,
            Some(file.node),
            (file.flags as u64 & O_PATH == 0).then_some(file.fh),
        )
        .map_err(|error| -(error as i64));
    }
    if super::proxy_file::is_proxy(fd) {
        let size = super::proxy_file::size(fd)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_mode = libc::S_IFREG | 0o777;
        st.st_size = size;
        st.st_blksize = 4096;
        return Ok(st);
    }
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
    if super::ashmem::as_device(&mut st) || super::dma_heap::as_device(&mut st) {
        return Ok(st);
    }
    if super::evdev::fstat(fd, &mut st).is_some() {
        return Ok(st);
    }
    if let Some(result) = super::net::socket_inode_stat(fd) {
        return result.map_err(|error| -(error as i64));
    }
    if let Some((inode, uid)) = super::net::proc_socket_identity(fd) {
        st.st_ino = inode;
        st.st_uid = uid;
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
        Ok(st) => put_stat(&st, a[1]),
        Err(e) => e,
    }
}

/// stat of a path relative to a dirfd, with the guest's ownership view.
pub(super) fn stat_at(dirfd: i32, path: &[u8], flags: u64) -> Result<libc::stat, i64> {
    stat_at_as(dirfd, path, flags, &super::cred::current())
}

fn stat_at_as(
    dirfd: i32,
    path: &[u8],
    flags: u64,
    id: &super::cred::Identity,
) -> Result<libc::stat, i64> {
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
    let r = resolve_as(dirfd, path, follow, id, attrs::FS).map_err(|e| -(e as i64))?;
    if r.guest == "/dev/fuse" {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_mode = libc::S_IFCHR | 0o666;
        st.st_nlink = 1;
        st.st_rdev = (10 << 24) | 229;
        st.st_blksize = 4096;
        return Ok(st);
    }
    if let Some(route) = vfs::fuse_route(&r.guest) {
        return super::fuse_client::stat(&route, None, None).map_err(|error| -(error as i64));
    }
    if let Some(s) = procfs::stat(&r.guest, follow) {
        return s.map_err(|e| -(e as i64));
    }
    if let Some(s) = super::ashmem::stat(&r.guest).or_else(|| super::dma_heap::stat(&r.guest)) {
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
    let path = match guest_cstr(a[1]) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let path = path.as_slice();
    match stat_at(a[0] as i32, path, a[3]) {
        Ok(st) => put_stat(&st, a[2]),
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
    let path = match guest_cstr(a[1]) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let path = path.as_slice();
    let st = match stat_at(dirfd, path, flags) {
        Ok(st) => st,
        Err(e) => return e,
    };
    let dev = st.st_dev as u32;
    let rdev = st.st_rdev as u32;
    let verity = match super::regular_file::verity_stat(&st) {
        Ok(verity) => verity,
        Err(error) => return -(error as i64),
    };
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
        attributes: if verity == Some(true) {
            super::fsverity_ioctl::VERITY_ATTRIBUTE
        } else {
            0
        },
        attributes_mask: if verity.is_some() {
            super::fsverity_ioctl::VERITY_ATTRIBUTE
        } else {
            0
        },
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
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&x as *const Statx).cast::<u8>(),
            std::mem::size_of::<Statx>(),
        )
    };
    super::fsverity_ioctl::write(out, bytes)
        .map(|_| 0)
        .unwrap_or_else(|error| -(error as i64))
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
        t if t.starts_with("cgroup:") => 0x0027_e0eb,
        "tmpfs" => 0x0102_1994,
        _ => return None,
    })
}

fn put_statfs(s: &libc::statfs, out: u64) -> i64 {
    put_statfs_as(s, EXT4_SUPER_MAGIC, out)
}

fn put_statfs_as(s: &libc::statfs, f_type: u64, out: u64) -> i64 {
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
    put_user_struct(&l, out)
}

fn put_fuse_statfs(route: &vfs::FuseRoute, out: u64) -> i64 {
    let bytes = match super::fuse_client::statfs(route) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    if bytes.len() < 48 {
        return -(crate::errno::EIO as i64);
    }
    let q = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    let d = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as u64;
    let stat = LinuxStatfs {
        f_type: 0x65735546,
        f_blocks: q(0),
        f_bfree: q(8),
        f_bavail: q(16),
        f_files: q(24),
        f_ffree: q(32),
        f_bsize: d(40),
        f_namelen: d(44),
        f_frsize: if bytes.len() >= 52 { d(48) } else { d(40) },
        f_flags: u64::from(route.read_only),
        ..Default::default()
    };
    put_user_struct(&stat, out)
}
pub fn fstatfs(a: [u64; 6]) -> i64 {
    let pin = match fdtab::pin_guest(a[0] as i32) {
        Ok(pin) => pin,
        Err(error) => return -(error as i64),
    };
    use std::os::fd::AsRawFd;
    let mut a = a;
    a[0] = pin.descriptor().as_raw_fd() as u64;
    if let Some(file) = super::fuse_client::get(a[0] as i32) {
        return put_fuse_statfs(&file.route, a[1]);
    }
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: local buffer.
    if unsafe { libc::fstatfs(a[0] as i32, &mut s) } < 0 {
        return -(errno::last() as i64);
    }
    space::adjust(&mut s);
    put_statfs(&s, a[1])
}

pub fn statfs(a: [u64; 6]) -> i64 {
    // SAFETY: guest path pointer.
    let p = match guest_cstr(a[0]) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let p = p.as_slice();
    let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, p, true) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if let Some(route) = vfs::fuse_route(&r.guest) {
        return put_fuse_statfs(&route, a[1]);
    }
    if let Some(magic) = super::selinuxfs::statfs_magic(&r.guest) {
        let l = LinuxStatfs {
            f_type: magic,
            f_bsize: 4096,
            f_namelen: 255,
            f_frsize: 4096,
            ..Default::default()
        };
        // SAFETY: guest statfs buffer.
        return put_user_struct(&l, a[1]);
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
    put_statfs_as(&s, magic.unwrap_or(EXT4_SUPER_MAGIC), a[1])
}

pub fn readlinkat(a: [u64; 6]) -> i64 {
    let (dirfd, buf, size) = (a[0] as i32, a[2], a[3] as usize);
    // SAFETY: guest path pointer.
    let path = match guest_cstr(a[1]) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let path = path.as_slice();
    if size == 0 {
        return -(EINVAL as i64);
    }
    let r = match resolve_as(dirfd, path, false, &super::cred::current(), attrs::FS) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if let Some(route) = vfs::fuse_route(&r.guest) {
        return match super::fuse_client::readlink(&route) {
            Ok(bytes) => {
                let count = bytes.len().min(a[3] as usize);
                super::user_memory::write_exact(a[2], &bytes[..count])
                    .map(|_| count as i64)
                    .unwrap_or_else(|error| -(error as i64))
            }
            Err(error) => -(error as i64),
        };
    }
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
    super::user_memory::write_exact(buf, &target[..n])
        .map(|_| n as i64)
        .unwrap_or_else(|error| -(error as i64))
}

const R_OK: u64 = 4;
const W_OK: u64 = 2;

pub fn faccessat(dirfd: u64, path: u64, mode: u64, flags: u64) -> i64 {
    // SAFETY: guest path pointer.
    let p = match guest_cstr(path) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let p = p.as_slice();
    if mode & !7 != 0 {
        return -(EINVAL as i64);
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;
    let id = super::cred::current();
    let kind = if flags & AT_EACCESS != 0 {
        attrs::EFFECTIVE
    } else {
        attrs::REAL
    };
    let r = match resolve_as(dirfd as i32, p, follow, &id, kind) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if let Some(route) = vfs::fuse_route(&r.guest) {
        return super::fuse_client::access(&route, mode as u32)
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64));
    }
    if super::binder::is_device(&r.guest)
        || super::ashmem::stat(&r.guest).is_some()
        || super::dma_heap::stat(&r.guest).is_some()
    {
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
        if !attrs::permits(&st, mode as u32, &id, kind) {
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
    let mut bytes = cwd.into_bytes();
    bytes.push(0);
    super::user_memory::write_exact(a[0], &bytes)
        .map(|_| bytes.len() as i64)
        .unwrap_or_else(|error| -(error as i64))
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
    let p = match guest_cstr(a[0]) {
        Ok(path) => path,
        Err(error) => return -(error as i64),
    };
    let p = p.as_slice();
    let id = super::cred::current();
    match resolve_as(vfs::LINUX_AT_FDCWD, p, true, &id, attrs::FS) {
        Ok(r) => match attrs::search(&r.guest, &id, attrs::FS) {
            Ok(()) => set_cwd_checked(r),
            Err(error) => -(error as i64),
        },
        Err(e) => -(e as i64),
    }
}

pub fn fchdir(a: [u64; 6]) -> i64 {
    let pin = match fdtab::pin_guest(a[0] as i32) {
        Ok(pin) => pin,
        Err(error) => return -(error as i64),
    };
    use std::os::fd::AsRawFd;
    let fd = pin.descriptor().as_raw_fd();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 {
        return -(errno::last() as i64);
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return -(errno::ENOTDIR as i64);
    }
    let guest = dir::synthesized_path(fd)
        .or_else(|| super::fuse_client::get(fd).map(|file| file.guest.clone()))
        .or_else(|| {
            crate::xrt::fd_path(fd)
                .and_then(|path| vfs::guest_path_of_host(std::path::Path::new(&path)))
        });
    let Some(guest) = guest else {
        return -(ENOENT as i64);
    };
    let id = super::cred::current();
    if let Err(error) = attrs::search(&guest, &id, attrs::FS) {
        return -(error as i64);
    }
    vfs::set_cwd(guest);
    0
}

pub fn dup(a: [u64; 6]) -> i64 {
    let _guard = fdtab::lifecycle();
    let fd = a[0] as i32;
    if let Err(error) = fdtab::require_guest_visible(fd) {
        return -(error as i64);
    }
    let result = unsafe { libc::dup(fd) };
    if result < 0 {
        return -(errno::last() as i64);
    }
    fdtab::on_dup(fd, result);
    if let Err(error) = fdtab::publish_guest(result) {
        fdtab::on_close(result);
        unsafe {
            libc::close(result);
        }
        return -(error as i64);
    }
    result as i64
}

pub fn dup3(a: [u64; 6]) -> i64 {
    super::close_effects::run(|| dup3_inner(a))
}
fn dup3_inner(a: [u64; 6]) -> i64 {
    let (old, new, flags) = (a[0] as i32, a[1] as i32, a[2]);
    if old == new || flags & !O_CLOEXEC != 0 {
        return -(EINVAL as i64);
    }
    let guard = match fdtab::close_admission(new) {
        Ok(guard) => guard,
        Err(error) => return -(error as i64),
    };
    if let Err(error) = fdtab::require_guest_visible(old) {
        return -(error as i64);
    }
    if fdtab::is_hidden(new)
        || (!fdtab::visible(new) && unsafe { libc::fcntl(new, libc::F_GETFD) } >= 0)
    {
        return -(EBADF as i64);
    }
    let retained = fdtab::get(new);
    let retained_fuse = super::fuse_client::get(new);
    let socket = matches!(retained, Some(Kind::Sock(_)));
    let posix = match fdtab::guest_close_owner(new) {
        Ok(owner) => owner,
        Err(error) => return -(error as i64),
    };
    let was_visible = fdtab::visible(new);
    if was_visible {
        if let Err(error) = fdtab::withdraw_guest(new) {
            return -(error as i64);
        }
    }
    let result = unsafe { libc::dup2(old, new) };
    if result < 0 {
        let error = errno::last();
        let restored = if was_visible {
            fdtab::publish_guest(new)
        } else {
            Ok(())
        };
        drop(guard);
        drop(retained);
        drop(retained_fuse);
        return -(restored.err().unwrap_or(error) as i64);
    }
    fdtab::on_dup(old, new);
    let result = if flags & O_CLOEXEC != 0
        && unsafe { libc::fcntl(new, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
    {
        -(errno::last() as i64)
    } else {
        fdtab::publish_guest(new)
            .map(|_| new as i64)
            .unwrap_or_else(|error| -(error as i64))
    };
    let binder_flush = guard.finish(was_visible);
    drop(retained);
    drop(retained_fuse);
    if socket {
        super::close_effects::note_socket_close();
    }
    fdtab::report_replaced_flush(binder_flush);
    match fdtab::finish_guest_close(posix) {
        Ok(()) => result,
        Err(error) => -(error as i64),
    }
}

pub fn pipe2(a: [u64; 6]) -> i64 {
    const O_DIRECT: u64 = 0o200000;
    let (out, flags) = (a[0], a[1]);
    if flags & !(O_CLOEXEC | O_NONBLOCK | O_DIRECT) != 0 {
        return -(EINVAL as i64);
    }
    if let Err(error) = super::user_memory::prepare_write(out, 8) {
        return -(error as i64);
    }
    let mut fds = [0i32; 2];
    // SAFETY: pipe into a local array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return -(errno::last() as i64);
    }
    for fd in fds {
        fdtab::set_flags(fd, flags & O_NONBLOCK != 0, flags & O_CLOEXEC != 0);
    }
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&fds[0].to_le_bytes());
    bytes[4..].copy_from_slice(&fds[1].to_le_bytes());
    if let Err(error) = super::user_memory::write_exact(out, &bytes) {
        for fd in fds {
            unsafe {
                libc::close(fd);
            }
        }
        return -(error as i64);
    }
    let _guard = fdtab::lifecycle();
    for fd in fds {
        if let Err(error) = fdtab::publish_guest(fd) {
            for fd in fds {
                if fdtab::visible(fd) {
                    let _ = fdtab::withdraw_guest(fd);
                }
                unsafe {
                    libc::close(fd);
                }
            }
            return -(error as i64);
        }
    }
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
    if matches!(cmd, F_GETLK | F_SETLK | F_SETLKW) && super::posix_locks::configured() {
        let mut bytes = match super::fsverity_ioctl::read::<32>(arg) {
            Ok(bytes) => bytes,
            Err(error) => return -(error as i64),
        };
        if cmd != F_GETLK
            && i16::from_le_bytes(bytes[..2].try_into().unwrap()) != 2
            && matches!(fdtab::get(fd),Some(Kind::Regular(description))if description.flags&3==3)
        {
            return -(EBADF as i64);
        }
        let result = super::posix_locks::dispatch(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
            cmd,
            bytes.as_mut_ptr() as u64,
        );
        if result == 0 && cmd == F_GETLK {
            return super::fsverity_ioctl::write(arg, &bytes)
                .map(|_| 0)
                .unwrap_or_else(|error| -(error as i64));
        }
        return result;
    }
    // SAFETY: guest struct flock.
    let bytes = match super::fsverity_ioctl::read::<32>(arg) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    let mut l = unsafe { (bytes.as_ptr() as *const LinuxFlock).read_unaligned() };
    if cmd != F_OFD_GETLK
        && l.l_type != 2
        && matches!(fdtab::get(fd),Some(Kind::Regular(description))if description.flags&3==3)
    {
        return -(EBADF as i64);
    }
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
        return put_user_struct(&l, arg);
    }
    0
}

fn dup_from(fd: i32, min: i32, cloexec: bool) -> i64 {
    let _guard = fdtab::lifecycle();
    if let Err(error) = fdtab::require_guest_visible(fd) {
        return -(error as i64);
    }
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
    fdtab::publish_guest(r)
        .map(|_| r as i64)
        .unwrap_or_else(|error| {
            fdtab::on_close(r);
            unsafe {
                libc::close(r);
            }
            -(error as i64)
        })
}

pub fn fcntl(a: [u64; 6]) -> i64 {
    let (original, cmd, arg) = (a[0] as i32, a[1], a[2]);
    if matches!(cmd, F_DUPFD | F_DUPFD_CLOEXEC) {
        return dup_from(original, arg as i32, cmd == F_DUPFD_CLOEXEC);
    }
    if matches!(cmd, F_GETFD | F_SETFD) {
        let _guard = fdtab::lifecycle();
        if let Err(error) = fdtab::require_guest_visible(original) {
            return -(error as i64);
        }
        return errno::check(unsafe {
            if cmd == F_GETFD {
                libc::fcntl(original, libc::F_GETFD)
            } else {
                libc::fcntl(original, libc::F_SETFD, arg as i32 & libc::FD_CLOEXEC)
            }
        } as i64);
    }
    let pin = match fdtab::pin_guest(original) {
        Ok(pin) => pin,
        Err(error) => return -(error as i64),
    };
    use std::os::fd::AsRawFd;
    let fd = pin.descriptor().as_raw_fd();
    if let Some(Kind::Path(path)) = fdtab::get(fd) {
        if cmd == F_GETFL {
            return path.flags as i64;
        }
        if !matches!(cmd, F_DUPFD | F_DUPFD_CLOEXEC | F_GETFD | F_SETFD) {
            return -(EBADF as i64);
        }
    }
    if super::fuse_client::get(fd).is_some() && matches!(cmd, F_GETFL | F_SETFL) {
        return super::fuse::description_flags(fd, (cmd == F_SETFL).then_some(arg as u32))
            .map(|flags| if cmd == F_GETFL { flags as i64 } else { 0 })
            .unwrap_or_else(|error| -(error as i64));
    }
    if let Some(Kind::Pty(description)) = pin.kind() {
        if cmd == F_GETFL {
            return description.flags() as i64;
        }
        if cmd == F_SETFL {
            return description
                .set_flags(arg)
                .map(|_| 0)
                .unwrap_or_else(|error| -(error as i64));
        }
    }
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
                    let mut flags = open_flags_from_host(r);
                    if let Some(Kind::Regular(description)) = fdtab::get(fd) {
                        flags = (flags & !O_ACCMODE) | (description.flags & O_ACCMODE);
                    }
                    if let Some(Kind::ProcMem(open)) = fdtab::get(fd) {
                        flags = (flags & !O_ACCMODE) | open.mode as u64;
                    }
                    flags as i64
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
            F_GETOWN => {
                *libc::__error() = 0;
                let value = libc::fcntl(fd, libc::F_GETOWN);
                if value == -1 && *libc::__error() != 0 {
                    return errno::check(-1);
                }
                if value == 0 {
                    0
                } else {
                    let group = value < 0;
                    let host = if group { -value } else { value };
                    match super::pidns::guest_pid(host) {
                        Ok(pid) => {
                            if group {
                                -(pid as i64)
                            } else {
                                pid as i64
                            }
                        }
                        Err(error) => error,
                    }
                }
            }
            F_SETOWN => {
                let target = if arg as i32 > 0 {
                    match super::pidns::syscall_pid(arg as i32) {
                        Ok(pid) => pid,
                        Err(error) => return error,
                    }
                } else if (arg as i32) < 0 {
                    match super::pidns::syscall_pid(-(arg as i32)) {
                        Ok(group) => -group,
                        Err(error) => return error,
                    }
                } else {
                    0
                };
                errno::check(libc::fcntl(fd, libc::F_SETOWN, target) as i64)
            }
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
    if matches!(a[1], FIOCLEX | FIONCLEX) {
        let _guard = fdtab::lifecycle();
        let fd = a[0] as i32;
        if let Err(error) = fdtab::require_guest_visible(fd) {
            return -(error as i64);
        }
        if is_path_fd(fd) {
            return -(EBADF as i64);
        }
        return errno::check(unsafe {
            libc::fcntl(
                fd,
                libc::F_SETFD,
                if a[1] == FIOCLEX { libc::FD_CLOEXEC } else { 0 },
            )
        } as i64);
    }
    let pin = match fdtab::pin_guest(a[0] as i32) {
        Ok(pin) => pin,
        Err(error) => return -(error as i64),
    };
    use std::os::fd::AsRawFd;
    let fd = pin.descriptor().as_raw_fd();
    if is_path_fd(fd) {
        return -(EBADF as i64);
    }
    let (req, arg) = (a[1], a[2]);
    if req == FIONBIO {
        if let Some(Kind::Pty(description)) = pin.kind() {
            let on = match super::user_memory::read_exact(arg, 4) {
                Ok(bytes) => i32::from_le_bytes(bytes.try_into().unwrap()) != 0,
                Err(error) => return -(error as i64),
            };
            let flags = description.flags();
            let flags = if on {
                flags | O_NONBLOCK
            } else {
                flags & !O_NONBLOCK
            };
            return description
                .set_flags(flags)
                .map(|_| 0)
                .unwrap_or_else(|error| -(error as i64));
        }
    }
    if let Some(result) = super::fsverity_ioctl::ioctl(&pin, req, arg) {
        return result;
    }
    if super::fuse_device::is_device(fd) {
        if req != super::fuse::FUSE_DEV_IOC_CLONE {
            return -(crate::errno::ENOTTY as i64);
        }
        let source = match super::fsverity_ioctl::read::<4>(arg) {
            Ok(bytes) => i32::from_le_bytes(bytes),
            Err(error) => return -(error as i64),
        };
        let source = match fdtab::pin_guest(source) {
            Ok(source) => source,
            Err(error) => return -(error as i64),
        };
        return super::fuse::ioctl_clone(fd, source.descriptor().as_raw_fd())
            .map(|_| 0)
            .unwrap_or_else(|error| -(error as i64));
    }
    if let Some(file) = super::fuse_client::get(fd) {
        return super::fuse_client::ioctl(&file, req as u32, arg)
            .map(|result| result as i64)
            .unwrap_or_else(|error| -(error as i64));
    }
    if let Some(r) = super::binder::ioctl(fd, req, arg) {
        return r;
    }
    if let Some(r) = super::ashmem::ioctl(fd, req, arg) {
        return r;
    }
    if let Some(r) = super::dma_heap::ioctl(fd, req, arg) {
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
        if libc::fcntl(fd, libc::F_GETFD) < 0 {
            return -(EBADF as i64);
        }
        if let Some(r) = super::tty::ioctl(fd, req, arg) {
            return r;
        }
        match req {
            FIONREAD => {
                let n = match inotify::pending_bytes(fd) {
                    Some(n) => n as i32,
                    None if let Some(r) = super::net::fionread(fd) => match r {
                        Ok(n) => n,
                        Err(error) => return error,
                    },
                    None => {
                        let mut n = 0i32;
                        if libc::ioctl(fd, libc::FIONREAD, &mut n) < 0 {
                            return -(errno::last() as i64);
                        }
                        n
                    }
                };
                super::user_memory::write_exact(arg, &n.to_le_bytes())
                    .map(|_| 0)
                    .unwrap_or_else(|error| -(error as i64))
            }
            FIONBIO => {
                let on = match super::fsverity_ioctl::read::<4>(arg) {
                    Ok(bytes) => i32::from_le_bytes(bytes) != 0,
                    Err(error) => return -(error as i64),
                };
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
            _ => -(ENOTTY as i64),
        }
    }
}

/// execve in place: prepare every close-on-exec owner release before any
/// native descriptor close makes the new image observable to a waiter.
pub fn close_on_exec() -> Result<(), errno::Errno> {
    let mut targets = Vec::new();
    for fd in super::fd_visibility::visible() {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            let error = errno::last();
            if error == EBADF {
                continue;
            }
            return Err(error);
        }
        if flags & libc::FD_CLOEXEC != 0 {
            targets.push(fd);
        }
    }
    fdtab::close_exec_batch(targets)
}

pub fn close_range(a: [u64; 6]) -> i64 {
    let (lo, hi, flags) = (a[0] as u32, a[1] as u32, a[2]);
    if flags & !6 != 0 || lo > hi {
        return -(EINVAL as i64);
    }
    if flags & 2 != 0 && !super::thread::alone() {
        return -95;
    }
    for fd in super::fd_visibility::visible() {
        let u = fd as u32;
        if u < lo || u > hi {
            continue;
        }
        if flags & 4 != 0 {
            let _guard = fdtab::lifecycle();
            if !fdtab::visible(fd) {
                continue;
            }
            // SAFETY: plain fcntl on a guest fd.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        } else {
            close([fd as u64, 0, 0, 0, 0, 0]);
        }
    }
    0
}

#[cfg(test)]
mod dac_tests {
    use super::super::cred::Identity;
    use super::*;
    use std::{fs, os::unix::ffi::OsStrExt};

    fn identity(uid: u32, gid: u32) -> Identity {
        Identity {
            uid: [uid; 4],
            gid: [gid; 4],
            cap_eff: 0,
            cap_perm: 0,
            ..Default::default()
        }
    }
    fn record(guest: &str, uid: u32, gid: u32, mode: u32) {
        let (host, _) = vfs::lookup(guest);
        let host = CString::new(host.as_os_str().as_bytes()).unwrap();
        attrs::record(
            attrs::Host::Path(&host),
            || guest.into(),
            attrs::Attr {
                uid: Some(uid),
                gid: Some(gid),
                mode: Some(mode),
            },
        );
    }
    fn open(guest: &str, flags: u64, mode: u64, id: &Identity) -> i64 {
        let path = CString::new(guest).unwrap();
        openat_as(
            [
                vfs::LINUX_AT_FDCWD as u64,
                path.as_ptr() as u64,
                flags,
                mode,
                0,
                0,
            ],
            id,
        )
    }
    #[track_caller]
    fn closed(fd: i64) {
        assert!(fd >= 0, "open failed {fd}");
        assert_eq!(close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }
    #[test]
    fn concurrent_open_waits_for_created_inode_ownership() {
        if fdtab::isolated_kernel_test(
            "sys::fs::dac_tests::concurrent_open_waits_for_created_inode_ownership",
        ) {
            return;
        }
        use std::{os::fd::AsRawFd, sync::mpsc, time::Duration};
        let (_guard, root) = vfs::test_view();
        let directory = root.join("data/create-publication");
        fs::create_dir(&directory).unwrap();
        record("/data/create-publication", 1017, 1017, 0o700);
        let app = identity(1017, 1017);
        super::super::cred::init(app.clone(), None);
        let parent = CString::new(directory.as_os_str().as_bytes()).unwrap();
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::stat(parent.as_ptr(), &mut stat) }, 0);
        let (started, ready) = mpsc::channel();
        let (sent, received) = mpsc::channel();
        let mut threads = Vec::new();
        let early = attrs::with_inode_lock(attrs::Host::Path(&parent), &stat, || {
            // Pause the creator after the host inode exists but before its
            // guest metadata is published. Neither kind of open may see root:root.
            let file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(directory.join("database"))
                .unwrap();
            let unpublished = attrs::path_stat("/data/create-publication/database").unwrap();
            assert_eq!(
                (unpublished.st_uid, unpublished.st_mode & 0o777),
                (0, 0o600)
            );
            for flags in [0, O_CREAT | 2] {
                let (started, sent, app) = (started.clone(), sent.clone(), app.clone());
                threads.push(std::thread::spawn(move || {
                    started.send(()).unwrap();
                    sent.send(open(
                        "/data/create-publication/database",
                        flags,
                        0o666,
                        &app,
                    ))
                    .unwrap();
                }));
            }
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            let early = received.recv_timeout(Duration::from_millis(100)).ok();
            attrs::created(attrs::Host::Fd(file.as_raw_fd()), || {
                "/data/create-publication/database".into()
            })?;
            Ok(early)
        })
        .unwrap();
        for _ in 0..2 - usize::from(early.is_some()) {
            closed(received.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        for thread in threads {
            thread.join().unwrap();
        }
        if let Some(fd) = early {
            closed(fd);
            panic!("open observed an inode before ownership publication");
        }
        let stat = attrs::path_stat("/data/create-publication/database").unwrap();
        assert_eq!(
            (stat.st_uid, stat.st_gid, stat.st_mode & 0o777),
            (1017, 1017, 0o600)
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn typed_path_fileport_import_preserves_roles_flags_and_duplicate_lifetime() {
        if fdtab::isolated_kernel_test(
            "sys::fs::dac_tests::typed_path_fileport_import_preserves_roles_flags_and_duplicate_lifetime",
        ) {
            return;
        }
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let (_guard, root) = vfs::test_view();
        let path = root.join("typed-path-import");
        fs::write(&path, b"path-only").unwrap();
        let native = CString::new(path.as_os_str().as_bytes()).unwrap();
        let backing = unsafe {
            OwnedFd::from_raw_fd(libc::open(
                native.as_ptr(),
                libc::O_EVTONLY | libc::O_CLOEXEC,
            ))
        };
        assert!(backing.as_raw_fd() >= 0);
        let before = stat_fd_kernel(backing.as_raw_fd()).unwrap();
        let flags = (O_PATH | O_NOFOLLOW) as u32;
        let capability = aim_binder_host::path_file::create(backing, flags).unwrap();
        assert_eq!(
            aim_binder_host::path_file::registered_class_result(capability.carrier.as_raw_fd())
                .unwrap(),
            aim_binder_host::path_file::CLASS
        );
        let port = aim_binder_host::mach::fd_to_port(capability.carrier.as_raw_fd()).unwrap();
        let incoming = aim_binder_host::mach::port_to_fd(port).unwrap();
        aim_binder_host::mach::release_send(port);
        assert_eq!(
            unsafe { libc::fcntl(incoming, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        fdtab::install_path(incoming).unwrap();
        fdtab::publish_guest(incoming).unwrap();
        let after = stat_fd(incoming).unwrap();
        assert_eq!(
            (after.st_dev, after.st_ino, after.st_mode),
            (before.st_dev, before.st_ino, before.st_mode)
        );
        assert_eq!(fcntl([incoming as u64, 3, 0, 0, 0, 0]), flags as i64);
        assert_eq!(
            fcntl([incoming as u64, 1, 0, 0, 0, 0]),
            libc::FD_CLOEXEC as i64
        );
        let exported = fdtab::export_fd(incoming).unwrap();
        assert!(fdtab::is_hidden(exported.fd));
        assert_eq!(
            aim_binder_host::path_file::registered_class_result(exported.fd).unwrap(),
            aim_binder_host::path_file::CLASS
        );
        let alias = dup([incoming as u64, 0, 0, 0, 0, 0]);
        assert!(alias >= 0);
        closed(incoming as i64);
        drop(capability);
        drop(exported);
        let mut byte = 0u8;
        assert_eq!(
            read([alias as u64, (&mut byte as *mut u8) as u64, 1, 0, 0, 0]),
            -9
        );
        let mut writer = super::super::fork_state::Writer::default();
        fdtab::fork_save(&mut writer);
        let bytes = writer.into_bytes();
        let mut reader = super::super::fork_state::Reader::new(&bytes);
        fdtab::fork_restore(&mut reader);
        assert!(reader.ok());
        assert_eq!(fcntl([alias as u64, 3, 0, 0, 0, 0]), flags as i64);
        assert_eq!(
            pread64([alias as u64, (&mut byte as *mut u8) as u64, 1, 0, 0, 0]),
            -9
        );
        closed(alias);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn hidden_kernel_descriptor_is_not_a_guest_io_or_dup_target() {
        use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
        let (_guard, root) = vfs::test_view();
        let path = root.join("kernel-private-fd");
        fs::write(&path, b"kernel-private-bytes").unwrap();
        let file = fs::File::open(&path).unwrap();
        let hidden = fdtab::hide(file.into_raw_fd());
        let file = unsafe { std::os::fd::OwnedFd::from_raw_fd(hidden) };
        let mut byte = 0u8;
        assert_eq!(
            read([hidden as u64, (&mut byte as *mut u8) as u64, 1, 0, 0, 0]),
            -9
        );
        assert_eq!(write([hidden as u64, b"x".as_ptr() as u64, 1, 0, 0, 0]), -9);
        assert_eq!(stat_fd(hidden).err(), Some(-9));
        assert_eq!(fcntl([hidden as u64, 3, 0, 0, 0, 0]), -9);
        assert_eq!(ioctl([hidden as u64, 0, 0, 0, 0, 0]), -9);
        assert_eq!(
            super::super::mem::mmap([0, 4096, 1, 2, hidden as u64, 0]),
            -9
        );
        assert_eq!(
            super::super::dir::getdents64([
                hidden as u64,
                (&mut byte as *mut u8) as u64,
                1,
                0,
                0,
                0
            ]),
            -9
        );
        assert_eq!(fdtab::export_fd(hidden).err().unwrap(), errno::EBADF);
        assert_eq!(dup([hidden as u64, 0, 0, 0, 0, 0]), -9);
        let source = fs::File::open(&path).unwrap();
        assert_eq!(
            dup3([source.as_raw_fd() as u64, hidden as u64, 0, 0, 0, 0]),
            -9
        );
        assert!(super::super::procfs::fd_link(hidden).is_none());
        // The kernel owner retains the same descriptor and bytes after rejected guest operations.
        assert_eq!(
            unsafe { libc::read(hidden, (&mut byte as *mut u8).cast(), 1) },
            1
        );
        assert_eq!(byte, b'k');
        fdtab::unhide(hidden);
        drop(file);
        fs::remove_file(path).unwrap();
    }

    /// A file created with no permissions, so no one reads it while it
    /// is written, then made readable through its descriptor (as
    /// FileBlobCache writes a shader cache): its creator owns it, with the
    /// group of its set-group-ID directory, and the mode set.
    #[test]
    fn a_file_created_with_no_permissions_is_its_creators_and_takes_fchmod() {
        if fdtab::isolated_kernel_test(
            "sys::fs::dac_tests::a_file_created_with_no_permissions_is_its_creators_and_takes_fchmod",
        ) {
            return;
        }
        let (_guard, _root) = vfs::test_view();
        let base = "/data/aim-create-mode-zero";
        let (host, _) = vfs::lookup(base);
        if host.exists() {
            fs::remove_dir_all(&host).unwrap();
        }
        fs::create_dir_all(host.join("cache")).unwrap();
        record(base, 1000, 1000, 0o755);
        record(&format!("{base}/cache"), 10154, 20154, 0o2771);
        let mut app = identity(10154, 10154);
        app.groups.push(20154);
        super::super::cred::init(app.clone(), None);
        let file = format!("{base}/cache/shaders");
        let fd = open(&file, O_CREAT | O_EXCL | 2, 0, &app);
        assert!(fd >= 0, "open failed {fd}");
        assert_eq!(write([fd as u64, b"cache".as_ptr() as u64, 5, 0, 0, 0]), 5);
        assert_eq!(
            super::super::fsops::fchmod([fd as u64, 0o400, 0, 0, 0, 0]),
            0
        );
        closed(fd);
        let stat = stat_at_as(vfs::LINUX_AT_FDCWD, file.as_bytes(), 0, &app).unwrap();
        assert_eq!(
            (stat.st_uid, stat.st_gid, stat.st_mode as u32 & 0o7777),
            (10154, 20154, 0o400)
        );
        closed(open(&file, 0, 0, &app));
    }

    #[test]
    fn guest_dac_enforces_search_file_access_and_create_flags() {
        if fdtab::isolated_kernel_test(
            "sys::fs::dac_tests::guest_dac_enforces_search_file_access_and_create_flags",
        ) {
            return;
        }
        let (_guard, _root) = vfs::test_view();
        let base = "/data/aim-dac-regression";
        // /data uses the test view's data root, irrespective of this helper's root return.
        let (host, _) = vfs::lookup(base);
        if host.exists() {
            fs::remove_dir_all(&host).unwrap();
        }
        fs::create_dir_all(host.join("owner")).unwrap();
        for name in ["private", "public", "noaccess", "group"] {
            fs::write(host.join("owner").join(name), b"intact").unwrap();
        }
        record(base, 1000, 1000, 0o755);
        record(&format!("{base}/owner"), 10101, 10101, 0o700);
        for (name, mode) in [
            ("private", 0o600),
            ("public", 0o644),
            ("noaccess", 0),
            ("group", 0o640),
        ] {
            record(&format!("{base}/owner/{name}"), 10101, 10222, mode);
        }
        let owner = identity(10101, 10101);
        let outsider = identity(10102, 10102);
        let private = format!("{base}/owner/private");
        let public = format!("{base}/owner/public");
        closed(open(&private, 0, 0, &owner));
        assert_eq!(open(&private, 0, 0, &outsider), -13);
        assert_eq!(open(&public, 0, 0, &outsider), -13);
        assert_eq!(
            resolve_as(
                vfs::LINUX_AT_FDCWD,
                public.as_bytes(),
                true,
                &outsider,
                attrs::FS
            )
            .err(),
            Some(errno::EACCES)
        );
        // Stat needs only ancestor search: stat of the final private directory is legal.
        assert!(
            stat_at_as(
                vfs::LINUX_AT_FDCWD,
                format!("{base}/owner").as_bytes(),
                0,
                &outsider
            )
            .is_ok()
        );
        assert_eq!(
            stat_at_as(vfs::LINUX_AT_FDCWD, public.as_bytes(), 0, &outsider).err(),
            Some(-13)
        );
        record(&format!("{base}/owner"), 10101, 10101, 0o711);
        closed(open(&public, 0, 0, &outsider));
        assert_eq!(open(&private, 0, 0, &outsider), -13);
        assert_eq!(open(&public, 1 | O_TRUNC, 0, &outsider), -13);
        assert_eq!(fs::read(host.join("owner/public")).unwrap(), b"intact");
        assert_eq!(
            open(&private, O_CREAT | O_EXCL, 0o600, &outsider),
            -(errno::EEXIST as i64)
        );
        assert_eq!(
            open(
                &format!("{base}/owner/missing"),
                O_CREAT | 1,
                0o600,
                &outsider
            ),
            -13
        );
        let created = format!("{base}/owner/new");
        closed(open(&created, O_CREAT | O_EXCL | 2, 0, &owner));
        assert_eq!(open(&created, 0, 0, &owner), -13);
        let mut group = outsider.clone();
        group.groups.push(10222);
        closed(open(&format!("{base}/owner/group"), 0, 0, &group));
        assert_eq!(open(&format!("{base}/owner/group"), 1, 0, &group), -13);
        // Owner bits take precedence, even when supplementary group bits grant more.
        record(&format!("{base}/owner/group"), 10102, 10222, 0o040);
        assert_eq!(open(&format!("{base}/owner/group"), 0, 0, &group), -13);
        std::os::unix::fs::symlink("private", host.join("owner/link")).unwrap();
        let link = format!("{base}/owner/link");
        assert_eq!(open(&link, O_NOFOLLOW, 0, &owner), -(errno::ELOOP as i64));
        assert_eq!(
            open(&link, O_CREAT | O_EXCL, 0, &owner),
            -(errno::EEXIST as i64)
        );
        closed(open(&link, 0, 0, &owner));
        let opath = open(
            &format!("{base}/owner/noaccess"),
            O_PATH | O_TRUNC | O_CREAT | 2,
            0,
            &outsider,
        );
        assert!(opath >= 0);
        let mut byte = 0u8;
        assert_eq!(
            read([opath as u64, (&mut byte as *mut u8) as u64, 1, 0, 0, 0]),
            -(errno::EBADF as i64)
        );
        assert_eq!(write([opath as u64, b"x".as_ptr() as u64, 1, 0, 0, 0]), -9);
        assert_eq!(
            pread64([opath as u64, (&mut byte as *mut u8) as u64, 1, 0, 0, 0]),
            -9
        );
        assert_eq!(
            pwrite64([opath as u64, b"x".as_ptr() as u64, 1, 0, 0, 0]),
            -9
        );
        assert_eq!(ioctl([opath as u64, 0, 0, 0, 0, 0]), -9);
        assert_eq!(
            super::super::mem::mmap([0, 4096, 1, 2, opath as u64, 0]),
            -9
        );
        assert!(stat_fd(opath as i32).is_ok());
        assert_eq!(fcntl([opath as u64, 3, 0, 0, 0, 0]), O_PATH as i64);
        let copy = fcntl([opath as u64, 0, 0, 0, 0, 0]);
        assert!(copy >= 0);
        assert_eq!(
            read([copy as u64, (&mut byte as *mut u8) as u64, 1, 0, 0, 0]),
            -9
        );
        closed(copy);
        closed(opath);
        let link_path = open(&link, O_PATH | O_NOFOLLOW, 0, &outsider);
        assert_eq!(
            stat_fd(link_path as i32).unwrap().st_mode & libc::S_IFMT,
            libc::S_IFLNK
        );
        closed(link_path);
        let directory_path = open(&format!("{base}/owner"), O_PATH | O_DIRECTORY, 0, &outsider);
        assert!(directory_path >= 0);
        let mut entries = [0u8; 1024];
        assert_eq!(
            super::super::dir::getdents64([
                directory_path as u64,
                entries.as_mut_ptr() as u64,
                1024,
                0,
                0,
                0
            ]),
            -9
        );
        closed(directory_path);
        assert_eq!(
            open(&private, O_PATH | O_DIRECTORY, 0, &outsider),
            -(errno::ENOTDIR as i64)
        );
        // An already-open dirfd begins the walk there, without checking its ancestors again.
        let dir = open(&format!("{base}/owner"), O_DIRECTORY, 0, &owner);
        assert!(dir >= 0);
        record(base, 1000, 1000, 0o700);
        let relative = CString::new("public").unwrap();
        closed(openat_as(
            [dir as u64, relative.as_ptr() as u64, 0, 0, 0, 0],
            &outsider,
        ));
        closed(dir as i64);
        record(base, 1000, 1000, 0o755);
        fs::remove_dir_all(&host).unwrap();
    }
}

#[cfg(test)]
mod regular_admission_tests {
    use super::*;
    use std::{
        fs,
        os::fd::{AsFd, AsRawFd},
        sync::Arc,
    };
    #[test]
    fn readonly_truncate_uses_writable_inode_admission_and_verity_preserves_data() {
        let (_guard, root) = vfs::test_view();
        fdtab::install_storage_registrar().unwrap();
        let name = std::ffi::CString::new("/data/readonly-truncate-owner").unwrap();
        let path = vfs::lookup(name.to_str().unwrap()).0;
        fs::write(&path, b"truncate this exact inode").unwrap();
        let fd = openat([
            vfs::LINUX_AT_FDCWD as u64,
            name.as_ptr() as u64,
            O_TRUNC,
            0,
            0,
            0,
        ]);
        assert!(fd >= 0, "readonly truncate fd {fd}");
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        let byte = b'x';
        assert_eq!(
            write([fd as u64, (&byte as *const u8) as u64, 1, 0, 0, 0]),
            -9
        );
        assert_eq!(close([fd as u64, 0, 0, 0, 0, 0]), 0);
        fs::write(&path, b"unchanged verity data").unwrap();
        let data = std::fs::File::open(&path).unwrap();
        let mut locator = fs::read(vfs::runtime_dir().unwrap().join("fs-verity-root")).unwrap();
        let store_root = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
            &locator.split_off(b"AIMVRTROOT01\0".len()),
        ));
        let store = Arc::new(
            aim_storage::fsverity::Store::new(
                &store_root,
                &vfs::runtime_dir().unwrap().join("fs-verity-leases"),
            )
            .unwrap(),
        );
        let admission = store.lock_inode(&data).unwrap();
        let enable = admission.begin_enable().unwrap();
        drop(admission);
        let prepared = enable
            .build(
                aim_storage::fsverity::BuildOptions::new(1, 4096, vec![], 16384, 4096).unwrap(),
                &[],
                || false,
            )
            .unwrap();
        enable.commit(prepared).unwrap();
        assert_eq!(
            openat([
                vfs::LINUX_AT_FDCWD as u64,
                name.as_ptr() as u64,
                O_TRUNC,
                0,
                0,
                0
            ]),
            -(errno::EPERM as i64)
        );
        assert_eq!(fs::read(&path).unwrap(), b"unchanged verity data");
        let fd = openat([vfs::LINUX_AT_FDCWD as u64, name.as_ptr() as u64, 0, 0, 0, 0]);
        assert!(fd >= 0);
        let mut bytes = [0u8; 32];
        let n = read([
            fd as u64,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
            0,
        ]);
        assert_eq!(&bytes[..n as usize], b"unchanged verity data");
        assert_eq!(close([fd as u64, 0, 0, 0, 0, 0]), 0);
        drop(data);
        fs::remove_file(path).unwrap();
        let _ = root;
    }
}

#[cfg(test)]
mod retained_socket_io_tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn call(number: u64, args: [u64; 6]) -> i64 {
        let context = crate::context::init_thread();
        unsafe {
            (&mut (*context).x)[..6].copy_from_slice(&args);
            (*context).x[8] = number;
            super::super::dispatch(&mut *context);
            (*context).x[0] as i64
        }
    }
    fn pair() -> [i32; 2] {
        let mut descriptors = [-1; 2];
        assert_eq!(
            net::socketpair([
                1,
                1 | 0x800 | 0x80000,
                0,
                descriptors.as_mut_ptr() as u64,
                0,
                0
            ]),
            0
        );
        descriptors
    }
    fn closed(fd: i32) {
        assert_eq!(close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }
    fn vector(bytes: &mut [u8]) -> libc::iovec {
        libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        }
    }

    #[test]
    fn public_unix_socket_scalar_vector_and_offset_minus_one_io_use_retained_authority() {
        if fdtab::isolated_kernel_test(
            "sys::fs::retained_socket_io_tests::public_unix_socket_scalar_vector_and_offset_minus_one_io_use_retained_authority",
        ) {
            return;
        }
        let (_view, _directory) = vfs::test_view();
        let sockets = pair();
        let mut received = [0u8; 4];
        assert_eq!(
            write([sockets[0] as u64, b"live".as_ptr() as u64, 4, 0, 0, 0]),
            4
        );
        assert_eq!(
            read([sockets[1] as u64, received.as_mut_ptr() as u64, 4, 0, 0, 0]),
            4
        );
        assert_eq!(&received, b"live");
        let mut tx_a = *b"ve";
        let mut tx_b = *b"ct";
        let source = [vector(&mut tx_a), vector(&mut tx_b)];
        let mut a = [0u8; 2];
        let mut b = [0u8; 2];
        let target = [vector(&mut a), vector(&mut b)];
        assert_eq!(
            writev([sockets[1] as u64, source.as_ptr() as u64, 2, 0, 0, 0]),
            4
        );
        assert_eq!(
            readv([sockets[0] as u64, target.as_ptr() as u64, 2, 0, 0, 0]),
            4
        );
        assert_eq!((&a, &b), (&*b"ve", &*b"ct"));
        assert_eq!(
            call(
                287,
                [sockets[0] as u64, source.as_ptr() as u64, 2, u64::MAX, 0, 0]
            ),
            4
        );
        a.fill(0);
        b.fill(0);
        assert_eq!(
            call(
                286,
                [sockets[1] as u64, target.as_ptr() as u64, 2, u64::MAX, 0, 0]
            ),
            4
        );
        assert_eq!((&a, &b), (&*b"ve", &*b"ct"));
        // An ordinary positioned socket operation remains ESPIPE.
        assert_eq!(
            call(286, [sockets[0] as u64, target.as_ptr() as u64, 2, 0, 0, 0]),
            -29
        );
        let pin = fdtab::pin_guest(sockets[0]).unwrap();
        let hidden = pin.descriptor().as_raw_fd();
        assert!(fdtab::is_hidden(hidden));
        for call in [read, write] {
            assert_eq!(
                call([hidden as u64, received.as_mut_ptr() as u64, 4, 0, 0, 0]),
                -(EBADF as i64)
            );
        }
        for call in [readv, writev] {
            assert_eq!(
                call([hidden as u64, target.as_ptr() as u64, 2, 0, 0, 0]),
                -(EBADF as i64)
            );
        }
        for writing in [false, true] {
            assert_eq!(
                call(
                    if writing { 287 } else { 286 },
                    [hidden as u64, target.as_ptr() as u64, 2, u64::MAX, 0, 0]
                ),
                -(EBADF as i64)
            );
        }
        assert_eq!(
            read([sockets[1] as u64, received.as_mut_ptr() as u64, 4, 0, 0, 0]),
            -(errno::EAGAIN as i64)
        );
        drop(pin);
        super::super::close_effects::flush().unwrap();
        for fd in sockets {
            closed(fd);
        }
    }

    #[test]
    fn retained_socket_kernel_io_survives_public_close_and_numeric_reuse() {
        if fdtab::isolated_kernel_test(
            "sys::fs::retained_socket_io_tests::retained_socket_kernel_io_survives_public_close_and_numeric_reuse",
        ) {
            return;
        }
        let (_view, _directory) = vfs::test_view();
        let original = pair();
        let replacement = pair();
        let pin = fdtab::pin_guest(original[0]).unwrap();
        let hidden = pin.descriptor().as_raw_fd();
        closed(original[0]);
        assert_eq!(
            dup3([replacement[0] as u64, original[0] as u64, 0, 0, 0, 0]),
            original[0] as i64
        );
        let mut byte = [b'o'];
        assert_eq!(special_write_kernel(&pin, &[vector(&mut byte)]), Some(1));
        let mut received = [0u8];
        assert_eq!(
            read([original[1] as u64, received.as_mut_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(received, [b'o']);
        assert_eq!(
            write([original[0] as u64, b"r".as_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(
            read([
                replacement[1] as u64,
                received.as_mut_ptr() as u64,
                1,
                0,
                0,
                0
            ]),
            1
        );
        assert_eq!(received, [b'r']);
        assert_eq!(
            write([original[1] as u64, b"p".as_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(
            write([replacement[1] as u64, b"q".as_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(special_read_kernel(&pin, &[vector(&mut received)]), Some(1));
        assert_eq!(received, [b'p']);
        assert_eq!(
            read([original[0] as u64, received.as_mut_ptr() as u64, 1, 0, 0, 0]),
            1
        );
        assert_eq!(received, [b'q']);
        assert_eq!(
            write([hidden as u64, b"x".as_ptr() as u64, 1, 0, 0, 0]),
            -(EBADF as i64)
        );
        drop(pin);
        super::super::close_effects::flush().unwrap();
        assert_eq!(
            read([original[1] as u64, received.as_mut_ptr() as u64, 1, 0, 0, 0]),
            0,
            "last actual original pin closes the original socket"
        );
        for fd in [original[0], original[1], replacement[0], replacement[1]] {
            closed(fd);
        }
    }
}

#[cfg(test)]
mod read_only_map_tests {
    use super::*;
    #[test]
    fn initial_read_only_map_preserves_read_access_and_rejects_writes() {
        if crate::sys::fdtab::isolated_kernel_test(
            "sys::fs::read_only_map_tests::initial_read_only_map_preserves_read_access_and_rejects_writes",
        ) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("aim-vfs-ro-map-{}", std::process::id()));
        let root = dir.join("root");
        let apex = dir.join("apex");
        std::fs::create_dir_all(root.join("bootstrap-apex")).unwrap();
        std::fs::create_dir_all(&apex).unwrap();
        std::fs::write(apex.join("payload"), b"original").unwrap();
        let map = dir.join("path-map");
        let text = format!(
            "root\t/\t{}\nro\t/bootstrap-apex/example\t{}\n",
            root.display(),
            apex.display()
        );
        std::fs::write(&map, &text).unwrap();
        vfs::init(&root, Some(&map)).unwrap();
        assert_eq!(
            vfs::lookup("/bootstrap-apex/example/payload"),
            (apex.join("payload"), vfs::Area::Image)
        );
        let path = CString::new("/bootstrap-apex/example/payload").unwrap();
        let args = |flags| [(-100i64) as u64, path.as_ptr() as u64, flags, 0, 0, 0];
        let fd = super::openat(args(0));
        assert!(fd >= 0, "read-only open: {fd}");
        assert_eq!(super::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(super::openat(args(1)), -(errno::EROFS as i64));
        assert_eq!(std::fs::read(apex.join("payload")).unwrap(), b"original");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
