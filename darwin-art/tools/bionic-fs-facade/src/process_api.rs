#[derive(Default)]
struct ProcessOwnerState {
    facade: Option<Arc<Facade>>,
    installing: bool,
    draining: bool,
    in_flight: usize,
}

struct ProcessOwner {
    state: Mutex<ProcessOwnerState>,
    quiescent: Condvar,
}

static PROCESS_OWNER: LazyLock<ProcessOwner> = LazyLock::new(|| ProcessOwner {
    state: Mutex::new(ProcessOwnerState::default()),
    quiescent: Condvar::new(),
});

type SpecialDeviceOpen = unsafe extern "C" fn(c_int, u32, *mut c_int) -> c_int;
static BINDER_DEVICE_OPEN: Mutex<Option<SpecialDeviceOpen>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_bind_binder_device_open(
    callback: Option<SpecialDeviceOpen>,
) -> c_int {
    let Ok(mut installed) = BINDER_DEVICE_OPEN.lock() else {
        return -1;
    };
    match (installed.is_some(), callback) {
        (false, Some(callback)) => {
            *installed = Some(callback);
            0
        }
        (true, None) => {
            *installed = None;
            0
        }
        _ => -1,
    }
}

fn open_binder_device(flags: c_int, mode: u32) -> Option<c_int> {
    let callback = BINDER_DEVICE_OPEN.lock().ok().and_then(|slot| *slot)?;
    let mut android_errno = 0;
    let descriptor = unsafe { callback(flags, mode, &mut android_errno) };
    if descriptor < 0 {
        Facade::set_android_errno(if android_errno > 0 {
            android_errno
        } else {
            ANDROID_EINVAL
        });
    }
    Some(descriptor)
}

fn process_owner_state() -> MutexGuard<'static, ProcessOwnerState> {
    PROCESS_OWNER.state.lock().unwrap_or_else(|_| {
        // The phase and in-flight count cannot be reconstructed after a panic
        // while holding this lifecycle lock.
        std::process::abort()
    })
}

struct ActiveFacade {
    facade: Arc<Facade>,
    process_lease: bool,
}

impl Drop for ActiveFacade {
    fn drop(&mut self) {
        if !self.process_lease {
            return;
        }
        let mut state = process_owner_state();
        assert!(state.in_flight > 0, "process owner lease underflow");
        state.in_flight -= 1;
        if state.in_flight == 0 {
            PROCESS_OWNER.quiescent.notify_all();
        }
    }
}

fn acquire_active() -> Option<ActiveFacade> {
    if let Some(facade) = ACTIVE.with(|active| active.borrow().clone()) {
        return Some(ActiveFacade {
            facade,
            process_lease: false,
        });
    }
    let mut state = process_owner_state();
    if state.installing || state.draining {
        return None;
    }
    let facade = state.facade.as_ref()?.clone();
    state.in_flight = state
        .in_flight
        .checked_add(1)
        .expect("process owner lease count overflow");
    Some(ActiveFacade {
        facade,
        process_lease: true,
    })
}

#[cfg(test)]
fn publish_process_facade(facade: Arc<Facade>) -> c_int {
    let mut state = process_owner_state();
    if state.facade.is_some() || state.installing || state.draining {
        return PROCESS_OWNER_ALREADY_INSTALLED;
    }
    state.facade = Some(facade);
    PROCESS_OWNER_OK
}

#[unsafe(no_mangle)]
/// Installs one process-wide facade from a caller-authorized directory fd.
///
/// # Safety
///
/// Each nonempty byte slice must be readable for its supplied length. The
/// caller must keep `root_fd` live until this call returns; the facade owns an
/// immediate duplicate and never consumes the caller's descriptor.
pub unsafe extern "C" fn darwin_art_bionic_fs_process_install(
    root_fd: c_int,
    guest_mount: *const u8,
    guest_mount_length: usize,
    cwd: *const u8,
    cwd_length: usize,
) -> c_int {
    if root_fd < 0
        || guest_mount.is_null()
        || guest_mount_length == 0
        || cwd.is_null()
        || cwd_length == 0
    {
        return PROCESS_OWNER_INVALID_ARGUMENT;
    }
    {
        let mut state = process_owner_state();
        if state.facade.is_some() || state.installing || state.draining {
            return PROCESS_OWNER_ALREADY_INSTALLED;
        }
        state.installing = true;
    }

    // SAFETY: the descriptor lifetime contract is stated above. Duplication
    // completes before the caller may reclaim its descriptor.
    let duplicate = unsafe { BorrowedFd::borrow_raw(root_fd) }.try_clone_to_owned();
    // SAFETY: non-null and nonempty arguments were validated above.
    let guest_mount = unsafe { slice::from_raw_parts(guest_mount, guest_mount_length) };
    // SAFETY: same byte-slice contract as guest_mount.
    let cwd = unsafe { slice::from_raw_parts(cwd, cwd_length) };
    let facade = match duplicate {
        Ok(fd) => match Facade::new(File::from(fd), guest_mount, cwd) {
            Ok(facade) => Some(Arc::new(facade)),
            Err(error) => {
                eprintln!("DARWIN FS: process owner creation failed: {error}");
                None
            }
        },
        Err(error) => {
            eprintln!("DARWIN FS: process owner descriptor duplication failed: {error}");
            None
        }
    };

    let mut state = process_owner_state();
    assert!(state.installing, "process owner install reservation lost");
    state.installing = false;
    match facade {
        Some(facade) => {
            assert!(state.facade.is_none() && !state.draining);
            state.facade = Some(facade);
            PROCESS_OWNER.quiescent.notify_all();
            PROCESS_OWNER_OK
        }
        None => {
            PROCESS_OWNER.quiescent.notify_all();
            PROCESS_OWNER_CREATE_FAILED
        }
    }
}

#[unsafe(no_mangle)]
/// Stops new process-owner calls, drains every active guest call, then drops
/// all facade-owned descriptors and broker authority.
pub extern "C" fn darwin_art_bionic_fs_process_uninstall() -> c_int {
    let facade = {
        let mut state = process_owner_state();
        if state.installing || state.draining {
            return PROCESS_OWNER_BUSY;
        }
        if state.facade.is_none() {
            return PROCESS_OWNER_NOT_INSTALLED;
        }
        state.draining = true;
        PROCESS_OWNER.quiescent.notify_all();
        while state.in_flight != 0 {
            state = PROCESS_OWNER
                .quiescent
                .wait(state)
                .unwrap_or_else(|_| std::process::abort());
        }
        state.facade.take()
    };
    drop(facade);
    let mut state = process_owner_state();
    assert!(state.facade.is_none() && state.draining && state.in_flight == 0);
    state.draining = false;
    PROCESS_OWNER.quiescent.notify_all();
    PROCESS_OWNER_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_process_has_capability_failure() -> c_int {
    acquire_active().map_or(-1, |active| {
        c_int::from(active.facade.has_capability_failure())
    })
}

#[unsafe(no_mangle)]
/// Creates an app-owned directory hierarchy inside the private `/data` mount.
///
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_seed_private_directory(path: *const c_char) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.seed_private_directory(path))
}

#[unsafe(no_mangle)]
/// Resolves an Android `/data` path to the process-authorized host backing path.
///
/// This is intentionally narrower than a general guest-to-host escape hatch:
/// immutable mounts and paths outside the private overlay are rejected. Passing
/// a null/zero output buffer returns the required byte count without a trailing
/// NUL. A non-null buffer receives a NUL-terminated path.
///
/// # Safety
///
/// `path` must be a readable NUL-terminated Android byte path. When `output` is
/// non-null, it must be writable for `capacity` bytes.
pub unsafe extern "C" fn darwin_art_bionic_fs_resolve_private_host_path(
    path: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| {
        let host_path = match facade.resolve_private_host_path(path) {
            Ok(path) => path,
            Err(error) => return facade.fail(error) as isize,
        };
        let bytes = host_path.as_os_str().as_bytes();
        if output.is_null() || capacity == 0 {
            return isize::try_from(bytes.len()).unwrap_or_else(|_| {
                facade.fail(ANDROID_ERANGE);
                -1
            });
        }
        let Some(required) = bytes.len().checked_add(1) else {
            return facade.fail(ANDROID_ERANGE) as isize;
        };
        if capacity < required {
            return facade.fail(ANDROID_ERANGE) as isize;
        }
        // SAFETY: the caller's writable-buffer contract and capacity check
        // cover both the byte path and its trailing NUL.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), bytes.len());
            *output.add(bytes.len()) = 0;
        }
        bytes.len() as isize
    })
}

pub const IOCTL_FD_INFO_ABI_VERSION: u32 = 1;
pub const IOCTL_FD_OTHER: c_int = 0;
pub const IOCTL_FD_RANDOM_DEVICE: c_int = 1;
pub const IOCTL_FD_FOUND: c_int = 0;
pub const IOCTL_FD_BAD: c_int = 1;
pub const IOCTL_FD_CAPABILITY_UNAVAILABLE: c_int = 2;

pub const SENDFILE_ABI_VERSION: u32 = 1;
pub const SENDFILE_TRANSFER_OK: c_int = 0;
pub const SENDFILE_TRANSFER_BAD_FD: c_int = 1;
pub const SENDFILE_TRANSFER_UNAVAILABLE: c_int = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SendfileRequest {
    pub abi_version: u32,
    pub output_fd: c_int,
    pub input_fd: c_int,
    pub has_explicit_offset: u32,
    pub offset: i64,
    pub count: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SendfileResult {
    pub abi_version: u32,
    pub android_errno: c_int,
    pub transferred: isize,
    pub next_offset: i64,
}

const _: () = assert!(size_of::<SendfileRequest>() == 32);
const _: () = assert!(size_of::<SendfileResult>() == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IoctlFdInfo {
    pub abi_version: u32,
    pub kind: c_int,
}

const _: () = assert!(size_of::<IoctlFdInfo>() == 8);

#[unsafe(no_mangle)]
/// # Safety
///
/// `info` must point to a writable `DarwinArtBionicIoctlFdInfo`.
pub unsafe extern "C" fn darwin_art_bionic_fs_ioctl_fd_lookup(
    _context: *mut c_void,
    fd: c_int,
    info: *mut IoctlFdInfo,
) -> c_int {
    if info.is_null() {
        return IOCTL_FD_CAPABILITY_UNAVAILABLE;
    }
    let Some(active) = acquire_active() else {
        return IOCTL_FD_CAPABILITY_UNAVAILABLE;
    };
    let descriptors = match active.facade.descriptors.lock() {
        Ok(descriptors) => descriptors,
        Err(_) => {
            active
                .facade
                .capability_failure
                .store(true, Ordering::Release);
            return IOCTL_FD_CAPABILITY_UNAVAILABLE;
        }
    };
    let kind = match descriptors.entries.get(&fd) {
        Some(Descriptor::Random(_)) => IOCTL_FD_RANDOM_DEVICE,
        Some(Descriptor::File(_) | Descriptor::PrivateFile(_) | Descriptor::Overlay(_)) => {
            IOCTL_FD_OTHER
        }
        None => return IOCTL_FD_BAD,
    };
    // SAFETY: required by this callback ABI; the ioctl provider passes its
    // stack-local record and consumes it before releasing the callback lease.
    unsafe {
        info.write(IoctlFdInfo {
            abi_version: IOCTL_FD_INFO_ABI_VERSION,
            kind,
        })
    };
    IOCTL_FD_FOUND
}

#[unsafe(no_mangle)]
/// Duplicates the Darwin descriptor behind a facade-owned regular file.
///
/// A successful result transfers ownership of `*host_fd` to the caller. The
/// duplicate makes a concurrent Android `close(fd)` harmless to an in-flight
/// operation while keeping host descriptors out of the guest-visible table.
///
/// # Safety
///
/// `host_fd` must point to writable storage for one `c_int`.
pub unsafe extern "C" fn darwin_art_bionic_fs_dup_host_fd_core(
    fd: c_int,
    host_fd: *mut c_int,
) -> c_int {
    if host_fd.is_null() {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    }
    with_active(-1, |facade| {
        let descriptors = match facade.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return facade.fail_capability(),
        };
        let Some(descriptor) = descriptors.entries.get(&fd) else {
            return 0;
        };
        let file = match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => file,
            _ => return facade.fail(ANDROID_EOPNOTSUPP),
        };
        let duplicate = match file.try_clone() {
            Ok(duplicate) => duplicate,
            Err(error) => return facade.fail_io(&error),
        };
        // SAFETY: the caller supplied writable storage, checked above. The raw
        // descriptor is now owned by that caller.
        unsafe { host_fd.write(duplicate.into_raw_fd()) };
        1
    })
}

#[unsafe(no_mangle)]
/// Consumes a platform-owned Darwin descriptor and returns its Android-visible
/// virtual file descriptor. This is the boundary used by Android platform APIs
/// such as `AAsset_openFileDescriptor`; raw host descriptor numbers must never
/// escape into guest native code.
pub extern "C" fn darwin_art_bionic_fs_adopt_host_fd_core(host_fd: c_int) -> c_int {
    if host_fd < 0 {
        Facade::set_android_errno(ANDROID_EBADF);
        return -1;
    }
    // SAFETY: the ABI transfers the one live host descriptor to this function.
    let file = unsafe { File::from_raw_fd(host_fd) };
    with_active(-1, |facade| {
        let mut descriptors = match facade.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return facade.fail_capability(),
        };
        match descriptors.insert(Descriptor::File(file)) {
            Ok(fd) => fd,
            Err(()) => facade.fail(ANDROID_EMFILE),
        }
    })
}

#[unsafe(no_mangle)]
/// Atomic transfer callback for the standalone sendfile provider.
///
/// # Safety
///
/// `request` must be readable and `result` writable for their complete ABI
/// records. The sendfile provider retains neither pointer after this call.
pub unsafe extern "C" fn darwin_art_bionic_fs_sendfile_transfer(
    _context: *mut c_void,
    request: *const SendfileRequest,
    result: *mut SendfileResult,
) -> c_int {
    if request.is_null() || result.is_null() {
        return SENDFILE_TRANSFER_UNAVAILABLE;
    }
    let Some(active) = acquire_active() else {
        return SENDFILE_TRANSFER_UNAVAILABLE;
    };
    // SAFETY: required by the callback ABI and consumed synchronously.
    let request = unsafe { &*request };
    // SAFETY: same callback lifetime contract as request.
    let result = unsafe { &mut *result };
    active.facade.sendfile_transfer(request, result)
}

/// A thread-affine activation scope.
///
/// Forgetting a guard leaks an `Arc` into that pthread rather than leaving a
/// dangling pointer. Moving the guard to another thread is rejected:
///
/// ```compile_fail
/// use bionic_fs_facade::Activation;
/// fn require_send<T: Send>() {}
/// require_send::<Activation>();
/// ```
pub struct Activation {
    previous: Option<Arc<Facade>>,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for Activation {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

fn with_active<T>(default: T, operation: impl FnOnce(&Facade) -> T) -> T {
    match acquire_active() {
        Some(active) => operation(&active.facade),
        None => {
            Facade::set_android_errno(ANDROID_EINVAL);
            default
        }
    }
}

unsafe fn path_bytes<'a>(path: *const c_char) -> Option<&'a [u8]> {
    if path.is_null() {
        return None;
    }
    // SAFETY: the Android libc ABI requires a readable NUL-terminated path.
    Some(unsafe { CStr::from_ptr(path) }.to_bytes())
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_open_core(
    path: *const c_char,
    flags: c_int,
    mode: u32,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    if path == b"/dev/binder" {
        if let Some(descriptor) = open_binder_device(flags, mode) {
            return descriptor;
        }
    }
    with_active(-1, |facade| facade.open_with_mode(path, flags, mode))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_openat_core(
    directory_fd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: u32,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| {
        facade.openat_with_mode(directory_fd, path, flags, mode)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// For nonzero `count`, `buffer` must be writable for `count` bytes.
pub unsafe extern "C" fn darwin_art_bionic_fs_read_core(
    fd: c_int,
    buffer: *mut c_void,
    count: usize,
) -> isize {
    let result = with_active(-1, |facade| unsafe { facade.read(fd, buffer, count) });
    if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() && (fd >= 10_000 || result != 0) {
        eprintln!("DARWIN FS: read fd={fd} count={count} result={result}");
    }
    result
}

#[unsafe(no_mangle)]
/// # Safety
///
/// For nonzero `count`, `buffer` must be readable for `count` bytes.
pub unsafe extern "C" fn darwin_art_bionic_fs_write_core(
    fd: c_int,
    buffer: *const c_void,
    count: usize,
) -> isize {
    with_active(-1, |facade| unsafe { facade.write(fd, buffer, count) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `vectors` points to `count` Android-arm64 iovec entries and each writable
/// buffer is valid for its declared length for the duration of this call.
pub unsafe extern "C" fn darwin_art_bionic_fs_readv_core(
    fd: c_int,
    vectors: *const crate::NativeIovec,
    count: c_int,
) -> isize {
    if !(0..=1024).contains(&count) || (count != 0 && vectors.is_null()) {
        return with_active(-1, |facade| facade.fail(22) as isize);
    }
    with_active(-1, |facade| unsafe { facade.readv(fd, vectors, count) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `vectors` points to `count` Android-arm64 iovec entries and each readable
/// buffer is valid for its declared length for the duration of this call.
pub unsafe extern "C" fn darwin_art_bionic_fs_writev_core(
    fd: c_int,
    vectors: *const crate::NativeIovec,
    count: c_int,
) -> isize {
    if !(0..=1024).contains(&count) || (count != 0 && vectors.is_null()) {
        return with_active(-1, |facade| facade.fail(22) as isize);
    }
    with_active(-1, |facade| unsafe { facade.writev(fd, vectors, count) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// For nonzero `count`, `buffer` must be writable for `count` bytes.
pub unsafe extern "C" fn darwin_art_bionic_fs_pread_core(
    fd: c_int,
    buffer: *mut c_void,
    count: usize,
    offset: i64,
) -> isize {
    let result = with_active(-1, |facade| unsafe {
        facade.pread(fd, buffer, count, offset)
    });
    if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() && fd >= 10_000 {
        eprintln!("DARWIN FS: pread fd={fd} offset={offset} count={count} result={result}");
    }
    result
}

#[unsafe(no_mangle)]
/// # Safety
///
/// For nonzero `count`, `buffer` must be readable for `count` bytes.
pub unsafe extern "C" fn darwin_art_bionic_fs_pwrite_core(
    fd: c_int,
    buffer: *const c_void,
    count: usize,
    offset: i64,
) -> isize {
    with_active(-1, |facade| unsafe {
        facade.pwrite(fd, buffer, count, offset)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_lseek_core(fd: c_int, offset: i64, whence: c_int) -> i64 {
    let result = with_active(-1, |facade| facade.lseek(fd, offset, whence));
    if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() && fd >= 10_000 {
        eprintln!("DARWIN FS: lseek fd={fd} offset={offset} whence={whence} result={result}");
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_close_core(fd: c_int) -> c_int {
    with_active(-1, |facade| facade.close(fd))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_owns_fd_core(fd: c_int) -> c_int {
    let Some(active) = acquire_active() else {
        return 0;
    };
    match active.facade.descriptors.lock() {
        Ok(descriptors) => c_int::from(descriptors.entries.contains_key(&fd)),
        Err(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_flock_core(fd: c_int, operation: c_int) -> c_int {
    with_active(-1, |facade| facade.flock(fd, operation))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_fcntl_core(
    fd: c_int,
    command: c_int,
    argument: isize,
) -> c_int {
    with_active(-1, |facade| facade.fcntl(fd, command, argument))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `status` must point to a writable 128-byte Android arm64 `struct stat`.
pub unsafe extern "C" fn darwin_art_bionic_fs_fstat_core(
    fd: c_int,
    status: *mut AndroidStat,
) -> c_int {
    let result = with_active(-1, |facade| unsafe { facade.fstat(fd, status) });
    if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() && fd >= 10_000 {
        let size = if result == 0 && !status.is_null() {
            // SAFETY: a successful fstat initialized the caller-owned object.
            unsafe { (*status).st_size }
        } else {
            -1
        };
        eprintln!("DARWIN FS: fstat fd={fd} result={result} size={size}");
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_fsync_core(fd: c_int) -> c_int {
    with_active(-1, |facade| facade.fsync(fd))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_fchdir_core(fd: c_int) -> c_int {
    with_active(-1, |facade| facade.fchdir(fd))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be a readable NUL-terminated byte path and `status` must point
/// to a writable 128-byte Android arm64 `struct stat`.
pub unsafe extern "C" fn darwin_art_bionic_fs_stat_core(
    path: *const c_char,
    status: *mut AndroidStat,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| unsafe { facade.stat(path, status, false) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// Same pointer contract as [`darwin_art_bionic_fs_stat_core`]. GuestRoot
/// installations support final-link metadata without following its target.
pub unsafe extern "C" fn darwin_art_bionic_fs_lstat_core(
    path: *const c_char,
    status: *mut AndroidStat,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| unsafe { facade.stat(path, status, true) })
}

#[unsafe(no_mangle)]
/// # Safety
/// Path is readable/NUL-terminated; status is a writable Android stat object.
pub unsafe extern "C" fn darwin_art_bionic_fs_fstatat_core(
    fd: c_int, path: *const c_char, status: *mut AndroidStat, flags: c_int,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| unsafe { facade.fstatat(fd, path, status, flags) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be readable and NUL-terminated. For nonzero `size`, `buffer`
/// must be writable for that many bytes. Android's virtual proc executable
/// links are synthesized; arbitrary broker symlink bytes remain unavailable.
pub unsafe extern "C" fn darwin_art_bionic_fs_readlink_core(
    path: *const c_char,
    buffer: *mut c_char,
    size: usize,
) -> isize {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.readlink(path, buffer, size))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `buffer` must be writable for `size` bytes. Null allocation mode is an
/// explicit unsupported capability.
pub unsafe extern "C" fn darwin_art_bionic_fs_getcwd_core(
    buffer: *mut c_char,
    size: usize,
) -> *mut c_char {
    with_active(ptr::null_mut(), |facade| unsafe {
        facade.getcwd(buffer, size)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be a readable NUL-terminated byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_chdir_core(path: *const c_char) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.chdir(path))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be a readable NUL-terminated byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_opendir_core(path: *const c_char) -> *mut c_void {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return ptr::null_mut();
    };
    with_active(ptr::null_mut(), |facade| facade.opendir(path))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_fdopendir_core(fd: c_int) -> *mut c_void {
    with_active(ptr::null_mut(), |facade| facade.fdopendir(fd))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_readdir_core(directory: *mut c_void) -> *mut AndroidDirent {
    with_active(ptr::null_mut(), |facade| facade.readdir(directory))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_rewinddir_core(directory: *mut c_void) {
    with_active((), |facade| facade.rewinddir(directory))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_closedir_core(directory: *mut c_void) -> c_int {
    with_active(-1, |facade| facade.closedir(directory))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_dirfd_core(directory: *mut c_void) -> c_int {
    with_active(-1, |facade| facade.dirfd(directory))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_fchmod_core(fd: c_int, _mode: u32) -> c_int {
    with_active(-1, |facade| facade.fchmod(fd, _mode))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_fchown_core(fd: c_int, owner: u32, group: u32) -> c_int {
    with_active(-1, |facade| facade.fchown(fd, owner, group))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_chmod_core(path: *const c_char, mode: u32) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.chmod(path, mode))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_fchmodat_core(
    directory_fd: c_int,
    path: *const c_char,
    _mode: u32,
    flags: c_int,
) -> c_int {
    if flags != 0 && flags != AT_SYMLINK_NOFOLLOW {
        Facade::set_android_errno(ANDROID_EINVAL);
        return -1;
    }
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| {
        facade.reject_at_path_mutation(directory_fd, path)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_ftruncate_core(fd: c_int, _length: i64) -> c_int {
    with_active(-1, |facade| facade.ftruncate(fd, _length))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_posix_fallocate_core(
    fd: c_int,
    offset: i64,
    length: i64,
) -> c_int {
    if offset < 0 || length < 0 || offset.checked_add(length).is_none() {
        return 22;
    }
    with_active(9, |facade| {
        let end = offset + length;
        let mut status = AndroidStat::default();
        if unsafe { facade.fstat(fd, &mut status) } != 0 {
            return 9;
        }
        if status.st_size >= end {
            return 0;
        }
        if facade.ftruncate(fd, end) == 0 { 0 } else { 9 }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_isatty_core(fd: c_int) -> c_int {
    with_active(0, |facade| facade.isatty(fd))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// Both paths must point to readable NUL-terminated Android byte paths.
pub unsafe extern "C" fn darwin_art_bionic_fs_link_core(
    old_path: *const c_char,
    new_path: *const c_char,
) -> c_int {
    let Some(old_path) = (unsafe { path_bytes(old_path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    let Some(new_path) = (unsafe { path_bytes(new_path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| {
        facade.reject_two_path_mutation(old_path, new_path)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_mkdir_core(path: *const c_char, _mode: u32) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.mkdir(path, _mode))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_mkdirat_core(
    directory_fd: c_int,
    path: *const c_char,
    mode: u32,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.mkdir_at(directory_fd, path, mode))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_pathconf_core(
    path: *const c_char,
    name: c_int,
) -> i64 {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.pathconf(path, name))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be readable and NUL terminated. A non-null `resolved` must be
/// writable for Android `PATH_MAX` bytes.
pub unsafe extern "C" fn darwin_art_bionic_fs_realpath_core(
    path: *const c_char,
    resolved: *mut c_char,
) -> *mut c_char {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return ptr::null_mut();
    };
    with_active(ptr::null_mut(), |facade| unsafe {
        facade.realpath(path, resolved)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_remove_core(path: *const c_char) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.remove_path(path))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// Both paths must point to readable NUL-terminated Android byte paths.
pub unsafe extern "C" fn darwin_art_bionic_fs_rename_core(
    old_path: *const c_char,
    new_path: *const c_char,
) -> c_int {
    let Some(old_path) = (unsafe { path_bytes(old_path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    let Some(new_path) = (unsafe { path_bytes(new_path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.rename_path(old_path, new_path))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must be readable and NUL terminated; `status` must point to a
/// writable 112-byte Android arm64 `struct statvfs`.
pub unsafe extern "C" fn darwin_art_bionic_fs_statvfs_core(
    path: *const c_char,
    status: *mut AndroidStatvfs,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| unsafe { facade.statvfs(path, status) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// Both arguments must point to readable NUL-terminated byte strings. The
/// target is never interpreted because immutable mounts reject link creation.
pub unsafe extern "C" fn darwin_art_bionic_fs_symlink_core(
    target: *const c_char,
    link_path: *const c_char,
) -> c_int {
    if (unsafe { path_bytes(target) }).is_none() {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    }
    let Some(link_path) = (unsafe { path_bytes(link_path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.reject_path_mutation(link_path))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_truncate_core(
    path: *const c_char,
    _length: i64,
) -> c_int {
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| facade.truncate_path(path, _length))
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable NUL-terminated Android byte path.
pub unsafe extern "C" fn darwin_art_bionic_fs_unlinkat_core(
    directory_fd: c_int,
    path: *const c_char,
    flags: c_int,
) -> c_int {
    if flags != 0 && flags != AT_REMOVEDIR {
        Facade::set_android_errno(ANDROID_EINVAL);
        return -1;
    }
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| {
        facade.reject_at_path_mutation(directory_fd, path)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// A non-null `path` must be readable and NUL terminated. `times` may be null;
/// otherwise the Android ABI requires two readable timespec records, which are
/// deliberately not consumed by the immutable-mount rejection.
pub unsafe extern "C" fn darwin_art_bionic_fs_utimensat_core(
    directory_fd: c_int,
    path: *const c_char,
    _times: *const AndroidTimespec,
    flags: c_int,
) -> c_int {
    if flags != 0 && flags != AT_SYMLINK_NOFOLLOW {
        Facade::set_android_errno(ANDROID_EINVAL);
        return -1;
    }
    if path.is_null() {
        return with_active(-1, |facade| facade.reject_fd_mutation(directory_fd));
    }
    let Some(path) = (unsafe { path_bytes(path) }) else {
        Facade::set_android_errno(ANDROID_EFAULT);
        return -1;
    };
    with_active(-1, |facade| {
        facade.reject_at_path_mutation(directory_fd, path)
    })
}

#[cfg(test)]
include!("tests.rs");
