#![forbid(unsafe_op_in_unsafe_fn)]

use darwin_art_fs_broker::{BrokerError, ReadOnlyBroker};
use darwin_art_prefix::{MountKind, MountTable, PrefixError, Resolution};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::fmt::Write as _;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::FileExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};

mod canonical_path;
mod descriptor_table;
mod descriptor_group;
mod directory;
mod filesystem_namespace;
mod immutable_open;
#[cfg(test)]
mod immutable_open_tests;
mod metadata;
mod mkdir_at;
mod native_image;
mod open_at;
mod private_data;
#[cfg(test)]
mod private_data_tests;
#[cfg(test)]
mod shared_storage_tests;
mod working_directory;
mod writable_mount;
#[cfg(test)]
mod writable_mount_tests;
mod writable_operations;
use descriptor_table::{DescriptorTable, FD_CLOEXEC};
use directory::DirectoryTable;
use private_data::PrivateDataRoot;

type HostDescriptorResolver = unsafe extern "C" fn(c_int, *mut c_int) -> c_int;
static HOST_DESCRIPTOR_RESOLVER: Mutex<Option<HostDescriptorResolver>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_fs_bind_host_descriptor_resolver(
    callback: Option<HostDescriptorResolver>,
) -> c_int {
    let Ok(mut installed) = HOST_DESCRIPTOR_RESOLVER.lock() else {
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

const AT_FDCWD: c_int = -100;
const O_ACCMODE: c_int = 3;
const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_RDWR: c_int = 2;
const O_CREAT: c_int = 64;
const O_EXCL: c_int = 128;
const O_TRUNC: c_int = 512;
const O_APPEND: c_int = 1024;
const O_NONBLOCK: c_int = 2048;
const O_DSYNC: c_int = 4096;
const O_DIRECTORY: c_int = 16384;
const O_NOFOLLOW: c_int = 32768;
const O_LARGEFILE: c_int = 131072;
const O_CLOEXEC: c_int = 524288;
const O_SYNC: c_int = 1_052_672;
const O_TMPFILE: c_int = 4_210_688;
const ACCEPTED_FLAGS: c_int = O_NONBLOCK | O_DIRECTORY | O_NOFOLLOW | O_LARGEFILE | O_CLOEXEC;
const WRITE_FLAGS: c_int = O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_DSYNC | O_SYNC;

const ANDROID_EBADF: i32 = 9;
const ANDROID_EACCES: i32 = 13;
const ANDROID_EFAULT: i32 = 14;
const ANDROID_EIO: i32 = 5;
const ANDROID_ENOENT: i32 = 2;
const ANDROID_EEXIST: i32 = 17;
const ANDROID_ENOTDIR: i32 = 20;
const ANDROID_EISDIR: i32 = 21;
const ANDROID_EINVAL: i32 = 22;
const ANDROID_EMFILE: i32 = 24;
const ANDROID_ENOTTY: i32 = 25;
const ANDROID_EROFS: i32 = 30;
const ANDROID_ERANGE: i32 = 34;
const ANDROID_EOPNOTSUPP: i32 = 95;
const DARWIN_ELOOP: i32 = 62;
const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
const AT_REMOVEDIR: c_int = 0x200;
const ANDROID_ST_RDONLY: u64 = 0x0001;
const ANDROID_S_IFCHR: u32 = 0o020000;
const ANDROID_S_IFDIR: u32 = 0o040000;
const ANDROID_S_IFREG: u32 = 0o100000;
const ANDROID_RANDOM_MODE: u32 = ANDROID_S_IFCHR | 0o666;
const ANDROID_RANDOM_RDEV: u64 = 0x108;
const ANDROID_URANDOM_RDEV: u64 = 0x109;
const PATHCONF_MIN: c_int = 0;
const PATHCONF_MAX: c_int = 19;

unsafe extern "C" {
    fn darwin_art_bionic_errno_store(android_errno: i32);
    fn darwin_art_bionic_errno_set_from_darwin(darwin_errno: i32) -> c_int;
    #[link_name = "close"]
    fn host_close(fd: c_int) -> c_int;
    #[link_name = "flock"]
    fn host_flock(fd: c_int, operation: c_int) -> c_int;
    #[link_name = "fcntl"]
    fn host_fcntl(fd: c_int, command: c_int, ...) -> c_int;
    #[link_name = "readv"]
    fn host_readv(fd: c_int, vectors: *const NativeIovec, count: c_int) -> isize;
    #[link_name = "writev"]
    fn host_writev(fd: c_int, vectors: *const NativeIovec, count: c_int) -> isize;
    fn darwin_art_bionic_fs_host_record_lock(
        host_fd: c_int,
        android_command: c_int,
        android_lock: isize,
        host_errno: *mut c_int,
    ) -> c_int;
    fn darwin_art_bionic_fs_host_cpu_count(online: c_int) -> c_long;
    fn darwin_art_bionic_fs_host_fdopendir(fd: c_int, host_errno: *mut c_int) -> *mut c_void;
    fn darwin_art_bionic_fs_host_readdir(
        directory: *mut c_void,
        entry: *mut HostDirent,
        host_errno: *mut c_int,
    ) -> c_int;
    fn darwin_art_bionic_fs_host_rewinddir(directory: *mut c_void);
    fn darwin_art_bionic_fs_host_closedir(directory: *mut c_void, host_errno: *mut c_int) -> c_int;
    fn darwin_art_bionic_fs_host_fpathconf(
        fd: c_int,
        semantic_name: c_int,
        value: *mut i64,
        host_errno: *mut c_int,
    ) -> c_int;
    fn darwin_art_bionic_fs_host_fstatvfs(
        fd: c_int,
        status: *mut HostStatvfs,
        host_errno: *mut c_int,
    ) -> c_int;
    #[link_name = "kSecRandomDefault"]
    static SEC_RANDOM_DEFAULT: *const c_void;
    #[link_name = "SecRandomCopyBytes"]
    fn sec_random_copy_bytes(random: *const c_void, count: usize, bytes: *mut u8) -> c_int;
    fn darwin_art_bionic_fs_host_enumerate_regions(
        callback: unsafe extern "C" fn(*mut c_void, u64, u64, c_int, *const c_char, u64) -> c_int,
        context: *mut c_void,
    ) -> c_int;
}

// Darwin and Android arm64 use the same two-word iovec shape at this native
// boundary. Keep it explicit so the production vector facade does not depend
// on a host libc typedef leaking into the Android ELF ABI.
#[repr(C)]
pub struct NativeIovec {
    pub base: *mut c_void,
    pub length: usize,
}

fn host_cpu_counts() -> (usize, usize) {
    fn query(online: bool) -> usize {
        // SAFETY: the shim forwards this fixed Darwin sysconf selector and
        // returns only the scalar result.
        let count = unsafe { darwin_art_bionic_fs_host_cpu_count(online as c_int) };
        usize::try_from(count)
            .ok()
            .filter(|count| *count > 0)
            .unwrap_or(1)
    }

    (query(false), query(true))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AndroidTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AndroidStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub pad1: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    pub pad2: i32,
    pub st_blocks: i64,
    pub st_atim: AndroidTimespec,
    pub st_mtim: AndroidTimespec,
    pub st_ctim: AndroidTimespec,
    pub unused4: u32,
    pub unused5: u32,
}

const _: () = assert!(size_of::<AndroidStat>() == 128);
const _: () = assert!(size_of::<AndroidTimespec>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AndroidDirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

impl Default for AndroidDirent {
    fn default() -> Self {
        Self {
            d_ino: 0,
            d_off: 0,
            d_reclen: 0,
            d_type: 0,
            d_name: [0; 256],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HostDirent {
    d_ino: u64,
    d_name_length: u16,
    d_type: u8,
    d_name: [u8; 256],
}

impl Default for HostDirent {
    fn default() -> Self {
        Self {
            d_ino: 0,
            d_name_length: 0,
            d_type: 0,
            d_name: [0; 256],
        }
    }
}

const _: () = assert!(size_of::<AndroidDirent>() == 280);
const _: () = assert!(std::mem::offset_of!(AndroidDirent, d_name) == 19);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AndroidStatvfs {
    pub f_bsize: u64,
    pub f_frsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_favail: u64,
    pub f_fsid: u64,
    pub f_flag: u64,
    pub f_namemax: u64,
    pub reserved: [u32; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct HostStatvfs {
    f_bsize: u64,
    f_frsize: u64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: u64,
    f_flag: u64,
    f_namemax: u64,
}

const _: () = assert!(size_of::<AndroidStatvfs>() == 112);
const _: () = assert!(std::mem::offset_of!(AndroidStatvfs, f_flag) == 72);
const _: () = assert!(size_of::<HostStatvfs>() == 88);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RandomDeviceKind {
    Random,
    Urandom,
}

enum Descriptor {
    File(File),
    PrivateFile(File),
    Random(RandomDeviceKind),
    Overlay(OverlayDescriptor),
}

struct OverlayFile {
    inode: u64,
    mode: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy)]
struct OverlayDirectory {
    inode: u64,
    mode: u32,
}

enum OverlayEntry {
    File(Arc<Mutex<OverlayFile>>),
    Directory(OverlayDirectory),
}

struct OverlayState {
    entries: BTreeMap<Vec<u8>, OverlayEntry>,
    next_inode: u64,
}

impl Default for OverlayState {
    fn default() -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(
            Vec::new(),
            OverlayEntry::Directory(OverlayDirectory {
                inode: 1,
                mode: ANDROID_S_IFDIR | 0o700,
            }),
        );
        Self {
            entries,
            next_inode: 2,
        }
    }
}

struct OverlayDescriptor {
    node: Arc<Mutex<OverlayFile>>,
    offset: u64,
    readable: bool,
    writable: bool,
}

const CENTRAL_BROKER_TOKEN_MARKER: c_int = 0x4000_0000;

trait EntropyBackend: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), ()>;
}

struct SecurityEntropy;

impl EntropyBackend for SecurityEntropy {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), ()> {
        if bytes.is_empty() {
            return Ok(());
        }
        // SAFETY: Security.framework accepts this writable slice for its exact
        // length and does not retain it. The default generator is process-owned.
        let status =
            unsafe { sec_random_copy_bytes(SEC_RANDOM_DEFAULT, bytes.len(), bytes.as_mut_ptr()) };
        if status == 0 { Ok(()) } else { Err(()) }
    }
}

pub struct Facade {
    namespace: filesystem_namespace::FilesystemNamespace,
    // Native APK code sometimes receives the installer-owned host path from
    // ApplicationInfo and opens it directly (Unity's central-directory scan
    // is one example). Keep this as a single capability, never a general host
    // path escape: only the exact path supplied by the launcher is accepted.
    authorized_host_apk: Option<PathBuf>,
    // ABI/resource splits are separate installer-owned files. Keep each one
    // as an exact-file capability as well; authorizing their parent directory
    // would let native code read unrelated host files.
    authorized_host_apk_splits: Vec<PathBuf>,
    // Immutable font fixtures used by the framework-only graphics probe.
    // These are exact-file capabilities, never a parent-directory grant.
    authorized_host_test_fonts: Vec<PathBuf>,
    // Extracted native libraries are another installer-owned capability. The
    // Java PathClassLoader must be able to stat the directory and open its
    // direct children for DexPathList.findLibrary(), while arbitrary host
    // traversal remains outside the guest filesystem contract.
    authorized_host_native_dir: Option<PathBuf>,
    descriptors: Mutex<DescriptorTable>,
    overlay: Mutex<OverlayState>,
    private_root: Option<PrivateDataRoot>,
    storage_root: Option<writable_mount::WritableMount>,
    directories: Mutex<DirectoryTable>,
    entropy: Arc<dyn EntropyBackend>,
    capability_failure: AtomicBool,
}

impl Facade {
    pub fn new(root: File, guest_mount: &[u8], cwd: &[u8]) -> Result<Self, &'static str> {
        Self::new_with_entropy(root, guest_mount, cwd, Arc::new(SecurityEntropy))
    }

    fn new_with_entropy(
        root: File,
        guest_mount: &[u8],
        cwd: &[u8],
        entropy: Arc<dyn EntropyBackend>,
    ) -> Result<Self, &'static str> {
        if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
            eprintln!(
                "DARWIN FS: install mount={} cwd={}",
                String::from_utf8_lossy(guest_mount),
                String::from_utf8_lossy(cwd)
            );
        }
        let mut private_root = std::env::var_os("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("DARWIN_ART_ANDROID_FILESYSTEM_ROOT")
                    .map(PathBuf::from)
                    .map(|root| root.join("data"))
            })
            .map(PrivateDataRoot::open)
            .transpose()
            .map_err(|_| "invalid private data root")?;
        let authorized_host_apk = std::env::var_os("DARWIN_ART_APK_APP_RESOURCE_APK")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file());
        let authorized_host_apk_splits = std::env::var_os("DARWIN_ART_APK_APP_SPLIT_SOURCE_DIRS")
            .map(|paths| {
                paths
                    .as_bytes()
                    .split(|byte| *byte == b':')
                    .filter_map(|path| {
                        if path.is_empty() {
                            return None;
                        }
                        let path = PathBuf::from(std::ffi::OsString::from_vec(path.to_vec()));
                        (path.is_absolute() && path.is_file()).then_some(path)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let authorized_host_native_dir = std::env::var_os("DARWIN_ART_APK_APP_NATIVE_DIR")
            .map(PathBuf::from)
            .and_then(|path| path.canonicalize().ok())
            .filter(|path| path.is_absolute() && path.is_dir());
        let authorized_host_test_fonts = ["DARWIN_ART_TEST_FONTS_XML", "DARWIN_ART_TEST_FONT"]
            .into_iter()
            .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
            .filter(|path| path.is_absolute() && path.is_file())
            .collect();
        let mut storage_root = std::env::var_os("DARWIN_ART_ANDROID_SHARED_STORAGE_ROOT")
            .map(PathBuf::from)
            .map(|path| writable_mount::WritableMount::open(path, b"/storage"))
            .transpose()?;
        // Installed package code (the profile's package store), read-only at
        // /data/app as Android's PackageManagerService and apps see it.
        let package_root = std::env::var_os("DARWIN_ART_ANDROID_PACKAGE_ROOT")
            .filter(|value| !value.is_empty())
            .map(|path| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(PathBuf::from(path))
                    .map_err(|_| "invalid installed package root")
            })
            .transpose()?;
        let namespace = filesystem_namespace::FilesystemNamespace::with_storage(
            root,
            guest_mount,
            cwd,
            private_root.as_ref(),
            storage_root.as_ref(),
            package_root.as_ref(),
        )?;
        if let (Some(private), Some(guest)) = (&mut private_root, &namespace.guest_root) {
            private
                .attach_guest_root(guest.clone())
                .map_err(|_| "private data root differs from guest mount")?;
        }
        if let (Some(storage), Some(guest)) = (&mut storage_root, &namespace.guest_root) {
            storage
                .attach_guest_root(guest.clone())
                .map_err(|_| "storage root differs from guest mount")?;
        }
        Ok(Self {
            namespace,
            authorized_host_apk,
            authorized_host_apk_splits,
            authorized_host_test_fonts,
            authorized_host_native_dir,
            descriptors: Mutex::new(DescriptorTable::default()),
            overlay: Mutex::new(OverlayState::default()),
            private_root,
            storage_root,
            directories: Mutex::new(DirectoryTable::default()),
            entropy,
            capability_failure: AtomicBool::new(false),
        })
    }

    pub fn activate(self: &Arc<Self>) -> Activation {
        let previous = ACTIVE.with(|active| active.replace(Some(Arc::clone(self))));
        Activation {
            previous,
            _not_send: PhantomData,
        }
    }

    pub fn has_capability_failure(&self) -> bool {
        self.capability_failure.load(Ordering::Acquire)
    }

    fn set_android_errno(value: i32) {
        // SAFETY: the standalone Bionic errno module is linked for the facade lifetime.
        unsafe { darwin_art_bionic_errno_store(value) };
    }

    fn fail(&self, android_errno: i32) -> c_int {
        Self::set_android_errno(android_errno);
        -1
    }

    fn fail_capability(&self) -> c_int {
        self.capability_failure.store(true, Ordering::Release);
        Self::set_android_errno(ANDROID_EIO);
        -1
    }

    fn fail_io(&self, error: &std::io::Error) -> c_int {
        if let Some(raw) = error.raw_os_error() {
            // SAFETY: translation has no pointer arguments and preserves host errno.
            if unsafe { darwin_art_bionic_errno_set_from_darwin(raw) } != 0 {
                return -1;
            }
        }
        self.fail_capability()
    }

    fn fail_broker(&self, error: &BrokerError) -> c_int {
        match error {
            BrokerError::InvalidPath(_) => self.fail(ANDROID_EACCES),
            BrokerError::NotDirectoryRoot => self.fail(ANDROID_ENOTDIR),
            BrokerError::UnsupportedNodeType => self.fail(ANDROID_EOPNOTSUPP),
            BrokerError::Io { source, .. } => self.fail_io(source),
        }
    }

    fn resolve_from(&self, cwd: &[u8], path: &[u8]) -> Result<Resolution, c_int> {
        match self.namespace.prefix.resolve(cwd, path) {
            Ok(resolution)
                if (resolution.mount_id == 1 && !resolution.writable)
                    || (resolution.mount_id == 2 && resolution.writable)
                    || (resolution.mount_id == 3
                        && resolution.writable
                        && self.storage_root.is_some())
                    || (resolution.mount_id == 4
                        && !resolution.writable
                        && self.namespace.packages) =>
            {
                Ok(resolution)
            }
            Ok(resolution) => {
                if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                    eprintln!(
                        "DARWIN FS: rejected mount path={} id={} writable={}",
                        String::from_utf8_lossy(path),
                        resolution.mount_id,
                        resolution.writable
                    );
                }
                Err(ANDROID_EACCES)
            }
            Err(PrefixError::NoMount) => Err(ANDROID_EACCES),
            Err(_) => Err(ANDROID_EINVAL),
        }
    }

    fn resolve(&self, path: &[u8]) -> Result<Resolution, c_int> {
        let cwd = match self.namespace.cwd.snapshot() {
            Ok(cwd) => cwd.clone(),
            Err(_) => {
                self.fail_capability();
                return Err(ANDROID_EIO);
            }
        };
        self.resolve_from(&cwd, path)
    }

    fn validate_immutable_flags(&self, flags: c_int) -> Result<(), c_int> {
        if flags & O_ACCMODE == O_WRONLY
            || flags & O_ACCMODE == O_RDWR
            || flags & WRITE_FLAGS != 0
            || flags & O_TMPFILE == O_TMPFILE
        {
            return Err(ANDROID_EROFS);
        }
        if flags & !ACCEPTED_FLAGS != 0 {
            return Err(ANDROID_EOPNOTSUPP);
        }
        Ok(())
    }

    fn authorized_host_native_path(&self, path: &[u8]) -> Option<PathBuf> {
        let root = self.authorized_host_native_dir.as_ref()?;
        let requested = PathBuf::from(std::ffi::OsString::from_vec(path.to_vec()));
        if requested == *root {
            return Some(requested);
        }
        if requested.parent() != Some(root.as_path()) {
            return None;
        }
        // Installed native artifacts are regular files. Reject a direct-child
        // symlink before any host open can follow it outside the sealed root.
        match fs::symlink_metadata(&requested) {
            Ok(metadata) if !metadata.file_type().is_symlink() => Some(requested),
            _ => None,
        }
    }

    fn authorized_host_apk_path(&self, path: &[u8]) -> Option<&std::path::Path> {
        find_authorized_host_apk_path(
            self.authorized_host_apk.as_ref(),
            &self.authorized_host_apk_splits,
            path,
        )
        .or_else(|| {
            let requested = PathBuf::from(std::ffi::OsString::from_vec(path.to_vec()));
            self.authorized_host_test_fonts
                .iter()
                .find(|candidate| **candidate == requested)
                .map(PathBuf::as_path)
        })
    }

    // ART's DexPathList receives the host backing path for an app-private DEX
    // after the JNI loader seam translates it. Keep that path inside the same
    // private-data capability as the Android /data view; this is deliberately
    // read-only and accepts only an existing regular file or its root.
    fn authorized_host_private_path(&self, path: &[u8]) -> Option<PathBuf> {
        let requested = self.authorized_host_private_candidate(path)?;
        match fs::metadata(&requested) {
            Ok(metadata) if metadata.is_file() || metadata.is_dir() => Some(requested),
            _ => None,
        }
    }

    // Host paths below DEX_LOCATION are the writable backing of the private
    // Android data mount.  Unlike the read-only lookup above, this form also
    // admits a not-yet-created final component so java.nio Files.copy and
    // friends can create app-private files without falling through to the
    // immutable guest broker (which reports ENOTDIR for a host pathname).
    fn authorized_host_private_candidate(&self, path: &[u8]) -> Option<PathBuf> {
        let root = self.private_root.as_ref()?.path();
        let requested = PathBuf::from(std::ffi::OsString::from_vec(path.to_vec()));
        let relative = requested.strip_prefix(root).ok()?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return None;
        }
        let parent = requested.parent()?;
        if !parent.is_dir() {
            return None;
        }
        Some(requested)
    }

    fn private_relative_from_host_path(&self, path: &std::path::Path) -> Option<Vec<u8>> {
        let root = self.private_root.as_ref()?.path();
        let relative = path.strip_prefix(root).ok()?;
        let bytes = relative.as_os_str().as_bytes();
        if bytes.is_empty()
            || bytes
                .split(|byte| *byte == b'/')
                .any(|component| component.is_empty() || component == b"." || component == b"..")
        {
            return None;
        }
        Some(bytes.to_vec())
    }

    fn open_private_relative(
        &self,
        relative: &[u8],
        flags: c_int,
        mode: u32,
    ) -> Result<File, c_int> {
        let authority = self
            .private_root
            .as_ref()
            .ok_or_else(|| self.fail(ANDROID_EIO))?;
        authority
            .open_file(relative, flags, mode)
            .map_err(|error| self.fail_io(&error))
    }

    fn open_authorized_host_private(&self, path: PathBuf, flags: c_int, mode: u32) -> c_int {
        let accepted = O_ACCMODE
            | O_CREAT
            | O_EXCL
            | O_TRUNC
            | O_APPEND
            | O_DSYNC
            | O_SYNC
            | O_NONBLOCK
            | O_DIRECTORY
            | O_NOFOLLOW
            | O_LARGEFILE
            | O_CLOEXEC;
        if flags & !accepted != 0 || flags & O_TMPFILE == O_TMPFILE {
            return self.fail(ANDROID_EOPNOTSUPP);
        }
        let relative = match self.private_relative_from_host_path(&path) {
            Some(relative) => relative,
            None => return self.fail(ANDROID_EACCES),
        };
        let file = match self.open_private_relative(&relative, flags, mode) {
            Ok(file) => file,
            Err(error) => return error,
        };
        if flags & O_DIRECTORY != 0 && !file.metadata().is_ok_and(|metadata| metadata.is_dir()) {
            return self.fail(ANDROID_ENOTDIR);
        }
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(Descriptor::PrivateFile(file), flags & O_CLOEXEC != 0) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }

    fn overlay_parent(path: &[u8]) -> &[u8] {
        path.iter()
            .rposition(|byte| *byte == b'/')
            .map_or(b"".as_slice(), |index| &path[..index])
    }

    fn validate_overlay_flags(&self, flags: c_int) -> Result<(), c_int> {
        let access = flags & O_ACCMODE;
        if access != O_RDONLY && access != O_WRONLY {
            return Err(ANDROID_EOPNOTSUPP);
        }
        if flags & (O_APPEND | O_DSYNC | O_SYNC) != 0
            || flags & O_TMPFILE == O_TMPFILE
            || flags & O_DIRECTORY != 0
        {
            return Err(ANDROID_EOPNOTSUPP);
        }
        let accepted =
            O_ACCMODE | O_CREAT | O_EXCL | O_TRUNC | O_NOFOLLOW | O_LARGEFILE | O_CLOEXEC;
        if flags & !accepted != 0 {
            return Err(ANDROID_EOPNOTSUPP);
        }
        if access == O_RDONLY && flags & (O_CREAT | O_EXCL | O_TRUNC) != 0 {
            return Err(ANDROID_EINVAL);
        }
        Ok(())
    }

    fn private_path(&self, relative: &[u8]) -> Result<PathBuf, c_int> {
        let root = self.private_root.as_ref().ok_or(ANDROID_EIO)?.path();
        if relative.contains(&0)
            || (!relative.is_empty()
                && relative.split(|byte| *byte == b'/').any(|component| {
                    component.is_empty() || component == b"." || component == b".."
                }))
        {
            return Err(ANDROID_EINVAL);
        }
        Ok(root.join(std::ffi::OsString::from_vec(relative.to_vec())))
    }

    fn resolve_private_host_path(&self, path: &[u8]) -> Result<PathBuf, c_int> {
        let resolution = self.resolve(path)?;
        if resolution.mount_id != 2 || !resolution.writable {
            return Err(ANDROID_EACCES);
        }
        self.private_path(&resolution.relative_path)
    }

    fn random_device(path: &[u8]) -> Option<RandomDeviceKind> {
        match path {
            b"/dev/random" => Some(RandomDeviceKind::Random),
            b"/dev/urandom" => Some(RandomDeviceKind::Urandom),
            _ => None,
        }
    }

    fn proc_self_maps(path: &[u8]) -> bool {
        Self::proc_self_memory_file(path, b"maps")
    }

    fn proc_self_smaps(path: &[u8]) -> bool {
        Self::proc_self_memory_file(path, b"smaps")
    }

    fn proc_self_memory_file(path: &[u8], file_name: &[u8]) -> bool {
        let self_path = [b"/proc/self/".as_slice(), file_name].concat();
        let thread_self_path = [b"/proc/thread-self/".as_slice(), file_name].concat();
        if path == self_path || path == thread_self_path {
            return true;
        }
        let suffix = [b"/".as_slice(), file_name].concat();
        let Some(pid) = path
            .strip_prefix(b"/proc/")
            .and_then(|path| path.strip_suffix(suffix.as_slice()))
        else {
            return false;
        };
        // Android child services are isolated as virtual processes inside one
        // Darwin task. Chromium commonly spells its own maps path using the
        // Java/ActivityManager pid rather than the host getpid() value. Every
        // such virtual pid therefore aliases this task's synthetic image map.
        !pid.is_empty() && pid.len() <= 10 && pid.iter().all(u8::is_ascii_digit) && pid != b"0"
    }

    /// Return the bounded, read-only device view exposed to native Android
    /// code.  The host is not allowed to leak its procfs/sysfs, but native
    /// engines legitimately use these files in addition to sysconf(3) and
    /// sysinfo(2) when selecting allocators and graphics paths.  Keep this
    /// snapshot consistent with the values reported by the Java/runtime
    /// device APIs: the host's configured/online CPU topology and 8 GiB of
    /// memory. CPU counts come from Darwin sysconf rather than process
    /// affinity, which may describe a virtual guest restriction.
    fn synthetic_proc_contents(path: &[u8]) -> Option<Vec<u8>> {
        const MEMORY_KIB: usize = 8 * 1024 * 1024;
        fn is_virtual_proc_file(path: &[u8], suffix: &[u8]) -> bool {
            if suffix == b"/statm"
                && (path == b"/proc/self/statm" || path == b"/proc/thread-self/statm")
            {
                return true;
            }
            path.strip_prefix(b"/proc/")
                .and_then(|rest| rest.strip_suffix(suffix))
                .is_some_and(|pid| {
                    !pid.is_empty()
                        && pid.len() <= 10
                        && pid.iter().all(u8::is_ascii_digit)
                        && pid != b"0"
                })
        }
        let is_self = |name: &[u8]| {
            let mut self_path = b"/proc/self/".to_vec();
            self_path.extend_from_slice(name);
            let mut thread_self_path = b"/proc/thread-self/".to_vec();
            thread_self_path.extend_from_slice(name);
            let mut pid_suffix = Vec::with_capacity(name.len() + 1);
            pid_suffix.push(b'/');
            pid_suffix.extend_from_slice(name);
            path == self_path
                || path == thread_self_path
                || path
                    .strip_prefix(b"/proc/")
                    .and_then(|rest| rest.strip_suffix(pid_suffix.as_slice()))
                    .is_some_and(|pid| {
                        !pid.is_empty()
                            && pid.len() <= 10
                            && pid.iter().all(u8::is_ascii_digit)
                            && pid != b"0"
                    })
        };
        let is_cpuinfo = path == b"/proc/cpuinfo";
        let is_statm = is_virtual_proc_file(path, b"/statm");
        let is_status = is_self(b"status");
        let is_cmdline = is_self(b"cmdline");
        let is_oom_score_adj = is_self(b"oom_score_adj");
        let is_cpu_sysfs = path == b"/sys/devices/system/cpu/possible"
            || path == b"/sys/devices/system/cpu/present"
            || path == b"/sys/devices/system/cpu/online"
            || path.starts_with(b"/sys/devices/system/cpu/cpu");
        let is_static_proc =
            path == b"/proc/meminfo" || path == b"/proc/loadavg" || path == b"/proc/uptime";
        if !is_cpuinfo
            && !is_statm
            && !is_status
            && !is_cmdline
            && !is_oom_score_adj
            && !is_cpu_sysfs
            && !is_static_proc
        {
            return None;
        }
        // Only CPU proc/sysfs surfaces need the host topology. Keep fixed
        // memory/load/uptime data and ordinary assets off this host query.
        let (configured_cpu_count, online_cpu_count) = if is_cpuinfo || is_statm || is_cpu_sysfs {
            host_cpu_counts()
        } else {
            (0, 0)
        };

        if path == b"/proc/cpuinfo" {
            let mut contents = String::new();
            let cpuinfo_style = std::env::var("DARWIN_ART_CPUINFO_STYLE").ok();
            let cpu_part = if std::env::var_os("DARWIN_ART_CPUINFO_DECIMAL_PART").is_some() {
                "3331"
            } else {
                "0xd03"
            };
            for cpu in 0..online_cpu_count {
                let sep = match cpuinfo_style.as_deref() {
                    Some("none") => ":",
                    Some("space") => " :",
                    _ => "\t:",
                };
                let _ = writeln!(contents, "processor{sep} {cpu}");
                let _ = writeln!(contents, "BogoMIPS{sep} 2400.00");
                let _ = writeln!(
                    contents,
                    "Features{sep} fp asimd evtstrm aes pmull sha1 sha2 crc32"
                );
                let _ = writeln!(contents, "CPU variant{sep} 0x0");
                let _ = writeln!(contents, "CPU part{sep} {cpu_part}");
                let _ = writeln!(contents, "CPU revision{sep} 4");
                let _ = writeln!(contents, "CPU architecture{sep} 8");
                let _ = writeln!(contents, "CPU implementer{sep} 0x41");
                let _ = writeln!(contents);
            }
            let mut bytes = contents.into_bytes();
            if std::env::var_os("DARWIN_ART_CPUINFO_NUL").is_some() {
                bytes.push(0);
            }
            return Some(bytes);
        }
        if path == b"/proc/meminfo" {
            return Some(
                format!(
                    "MemTotal:       {MEMORY_KIB:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\nBuffers:        {:>8} kB\nCached:         {:>8} kB\nSwapCached:     {:>8} kB\nActive:         {:>8} kB\nInactive:       {:>8} kB\nSwapTotal:      {:>8} kB\nSwapFree:       {:>8} kB\n",
                    MEMORY_KIB / 2,
                    MEMORY_KIB / 2,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                )
                .into_bytes(),
            );
        }
        if path == b"/proc/loadavg" {
            return Some(b"0.00 0.00 0.00 1/128 1\n".to_vec());
        }
        if path == b"/proc/uptime" {
            return Some(b"86400.00 86400.00\n".to_vec());
        }
        if is_status {
            let uid = std::env::var("DARWIN_ART_ANDROID_UID")
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(10_000);
            let process_name = std::env::var("DARWIN_ART_APK_PROCESS_NAME")
                .or_else(|_| std::env::var("DARWIN_ART_APK_APP_PACKAGE"))
                .unwrap_or_else(|_| "darwin-art".to_owned());
            return Some(
                format!(
                    "Name:\t{process_name}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\nVmHWM:\t262144 kB\nVmRSS:\t262144 kB\nRssAnon:\t262144 kB\nRssShmem:\t0 kB\nVmSwap:\t0 kB\n"
                )
                .into_bytes(),
            );
        }
        if is_cmdline {
            let mut process_name = std::env::var("DARWIN_ART_APK_PROCESS_NAME")
                .or_else(|_| std::env::var("DARWIN_ART_APK_APP_PACKAGE"))
                .unwrap_or_else(|_| "darwin-art".to_owned())
                .into_bytes();
            process_name.push(0);
            return Some(process_name);
        }
        if is_oom_score_adj {
            return Some(b"0\n".to_vec());
        }
        if is_self(b"statm") {
            return Some(b"262144 65536 0 0 0 0 0\n".to_vec());
        }

        match path {
            b"/sys/devices/system/cpu/possible" | b"/sys/devices/system/cpu/present" => {
                Some(format!("0-{}\n", configured_cpu_count - 1).into_bytes())
            }
            b"/sys/devices/system/cpu/online" => {
                Some(format!("0-{}\n", online_cpu_count - 1).into_bytes())
            }
            _ => {
                let cpu = path
                    .strip_prefix(b"/sys/devices/system/cpu/cpu")
                    .and_then(|rest| rest.strip_suffix(b"/cpu_capacity"))
                    .and_then(|digits| {
                        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                            return None;
                        }
                        std::str::from_utf8(digits).ok()?.parse::<usize>().ok()
                    });
                if let Some(cpu) = cpu.filter(|cpu| *cpu < configured_cpu_count) {
                    // Expose a stable two-cluster ARM topology so
                    // Unity/Chromium's cluster classifier reports every host
                    // configured CPU instead of treating it as zero-core.
                    return Some(if cpu < configured_cpu_count / 2 {
                        b"512\n".to_vec()
                    } else {
                        b"1024\n".to_vec()
                    });
                }
                let cpu = path
                    .strip_prefix(b"/sys/devices/system/cpu/cpu")
                    .and_then(|rest| rest.strip_suffix(b"/cpufreq/cpuinfo_max_freq"))
                    .and_then(|digits| {
                        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                            return None;
                        }
                        std::str::from_utf8(digits).ok()?.parse::<usize>().ok()
                    });
                cpu.filter(|cpu| *cpu < configured_cpu_count).map(|cpu| {
                    if cpu < configured_cpu_count / 2 {
                        b"1800000\n".to_vec()
                    } else {
                        b"2400000\n".to_vec()
                    }
                })
            }
        }
    }

    fn synthetic_inode(path: &[u8]) -> u64 {
        // Stable FNV-1a identity makes fstat/stat agree without retaining a
        // host path or sharing mutable synthetic state between descriptors.
        path.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x1000_0000_01b3)
        })
    }

    fn open_synthetic_readonly(&self, path: &[u8], flags: c_int, data: Vec<u8>) -> c_int {
        if let Err(error) = self.validate_immutable_flags(flags) {
            return self.fail(error);
        }
        if flags & O_DIRECTORY != 0 {
            return self.fail(ANDROID_ENOTDIR);
        }
        let node = Arc::new(Mutex::new(OverlayFile {
            inode: Self::synthetic_inode(path),
            mode: ANDROID_S_IFREG | 0o444,
            data,
        }));
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(
            Descriptor::Overlay(OverlayDescriptor {
                node,
                offset: 0,
                readable: true,
                writable: false,
            }),
            flags & O_CLOEXEC != 0,
        ) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }

    fn open_proc_memory_file(&self, flags: c_int, smaps: bool) -> c_int {
        if flags & O_ACCMODE != O_RDONLY || flags & WRITE_FLAGS != 0 {
            return self.fail(ANDROID_EROFS);
        }
        if flags & !(O_ACCMODE | O_CLOEXEC | O_NONBLOCK | O_LARGEFILE) != 0 {
            return self.fail(ANDROID_EOPNOTSUPP);
        }

        #[repr(C)]
        struct ProcMemoryOutput {
            output: *mut String,
            smaps: bool,
        }

        unsafe extern "C" fn append_region(
            context: *mut c_void,
            start: u64,
            end: u64,
            protection: c_int,
            path: *const c_char,
            file_offset: u64,
        ) -> c_int {
            if context.is_null() || start >= end {
                return -1;
            }
            // SAFETY: host_enumerate_regions synchronously passes back the
            // context supplied by this function.
            let state = unsafe { &mut *context.cast::<ProcMemoryOutput>() };
            if state.output.is_null() {
                return -1;
            }
            // SAFETY: state.output points at the live String owned by the
            // caller for this synchronous enumeration.
            let output = unsafe { &mut *state.output };
            let read = if protection & 1 != 0 { 'r' } else { '-' };
            let write = if protection & 2 != 0 { 'w' } else { '-' };
            let execute = if protection & 4 != 0 { 'x' } else { '-' };
            let pathname = if path.is_null() {
                String::new()
            } else {
                // SAFETY: the C provider keeps this NUL-terminated path
                // alive for the duration of this synchronous callback.
                unsafe { CStr::from_ptr(path) }
                    .to_string_lossy()
                    .into_owned()
            };
            // Darwin reports canonical paths below /private, while Android's
            // capability paths (and the POSIX spelling used by callers) use
            // /var, /tmp, and similar public mount aliases.
            let pathname = pathname
                .strip_prefix("/private")
                .unwrap_or(&pathname)
                .to_owned();
            if pathname.is_empty() {
                if writeln!(
                    output,
                    "{start:016x}-{end:016x} {read}{write}{execute}p {file_offset:08x} 00:00 0"
                )
                .is_err()
                {
                    return -1;
                }
            } else if writeln!(
                output,
                "{start:016x}-{end:016x} {read}{write}{execute}p {file_offset:08x} 00:00 0 {pathname}"
            )
            .is_err()
            {
                return -1;
            }
            if state.smaps {
                let size_kib = end.saturating_sub(start) / 1024;
                // Keep the complete Android smaps entry shape.  Darwin does
                // not expose Linux's per-page dirty accounting, so the
                // conservative values are zero; this is preferable to
                // claiming dirty pages based on an unrelated host metric.
                for line in [
                    format!("Size:                 {size_kib} kB"),
                    "Rss:                       0 kB".to_owned(),
                    "Pss:                       0 kB".to_owned(),
                    "Shared_Clean:              0 kB".to_owned(),
                    "Shared_Dirty:              0 kB".to_owned(),
                    "Private_Clean:             0 kB".to_owned(),
                    "Private_Dirty:             0 kB".to_owned(),
                    "Referenced:                0 kB".to_owned(),
                    "Anonymous:                 0 kB".to_owned(),
                    "AnonHugePages:             0 kB".to_owned(),
                    "ShmemPmdMapped:            0 kB".to_owned(),
                    "FilePmdMapped:             0 kB".to_owned(),
                    "Shared_Hugetlb:            0 kB".to_owned(),
                    "Private_Hugetlb:           0 kB".to_owned(),
                    "Swap:                      0 kB".to_owned(),
                    "SwapPss:                   0 kB".to_owned(),
                    "Locked:                    0 kB".to_owned(),
                    "THPeligible:               0".to_owned(),
                    "VmFlags: rd".to_owned(),
                ] {
                    if writeln!(output, "{line}").is_err() {
                        return -1;
                    }
                }
            }
            0
        }

        let mut contents = String::new();
        let mut state = ProcMemoryOutput {
            output: &mut contents,
            smaps,
        };
        // SAFETY: the callback and context remain live for this synchronous
        // enumeration of the current task's Mach VM map.
        let result = unsafe {
            darwin_art_bionic_fs_host_enumerate_regions(
                append_region,
                (&mut state as *mut ProcMemoryOutput).cast(),
            )
        };
        if result != 0 {
            return self.fail(ANDROID_EIO);
        }
        let node = Arc::new(Mutex::new(OverlayFile {
            inode: 0x7072_6f63,
            mode: ANDROID_S_IFREG | 0o444,
            data: contents.into_bytes(),
        }));
        let descriptor = Descriptor::Overlay(OverlayDescriptor {
            node,
            offset: 0,
            readable: true,
            writable: false,
        });
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(descriptor, flags & O_CLOEXEC != 0) {
            Ok(fd) => {
                if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                    eprintln!(
                        "DARWIN FS: synthetic proc {} fd={fd}",
                        if smaps { "smaps" } else { "maps" }
                    );
                }
                fd
            }
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }

    fn open_random(&self, kind: RandomDeviceKind, flags: c_int) -> c_int {
        if flags & O_ACCMODE != O_RDONLY
            || flags & WRITE_FLAGS != 0
            || flags & O_TMPFILE == O_TMPFILE
        {
            return self.fail(ANDROID_EROFS);
        }
        // Android callers routinely open the kernel random devices with
        // O_CLOEXEC (Chromium's PartitionAlloc does this during startup), and
        // may also request non-blocking semantics. Neither flag changes the
        // behavior of the synthetic descriptor, but both are valid on the
        // real Android character devices.
        if flags & !(O_ACCMODE | O_CLOEXEC | O_NONBLOCK | O_LARGEFILE) != 0 {
            return self.fail(ANDROID_EOPNOTSUPP);
        }
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(Descriptor::Random(kind), flags & O_CLOEXEC != 0) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }

    fn open_null(&self, flags: c_int) -> c_int {
        if flags & !(O_ACCMODE | O_CLOEXEC | O_NONBLOCK | O_LARGEFILE) != 0 {
            return self.fail(ANDROID_EOPNOTSUPP);
        }
        let access = flags & O_ACCMODE;
        let file = match OpenOptions::new()
            .read(access != O_WRONLY)
            .write(access != O_RDONLY)
            .open("/dev/null")
        {
            Ok(file) => file,
            Err(error) => return self.fail_io(&error),
        };
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(Descriptor::File(file), flags & O_CLOEXEC != 0) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }

    #[cfg(test)]
    fn open(&self, path: &[u8], flags: c_int) -> c_int {
        self.open_with_mode(path, flags, 0o600)
    }

    fn open_with_mode(&self, path: &[u8], flags: c_int, mode: u32) -> c_int {
        if let Some(kind) = Self::random_device(path) {
            return self.open_random(kind, flags);
        }
        if path == b"/dev/null" {
            return self.open_null(flags);
        }
        if Self::proc_self_maps(path) {
            return self.open_proc_memory_file(flags, false);
        }
        if Self::proc_self_smaps(path) {
            return self.open_proc_memory_file(flags, true);
        }
        if let Some(data) = Self::synthetic_proc_contents(path) {
            return self.open_synthetic_readonly(path, flags, data);
        }
        // A native library may open the APK by the host path returned through
        // ApplicationInfo. Treat that path as a read-only capability and feed
        // it through the normal descriptor table so read/pread/mmap/fstat all
        // retain the Android virtual-fd semantics.
        if let Some(apk) = self.authorized_host_apk_path(path) {
            if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                eprintln!(
                    "DARWIN FS: authorized APK open path={} flags={flags:#x}",
                    String::from_utf8_lossy(path)
                );
            }
            if let Err(error) = self.validate_immutable_flags(flags) {
                if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                    eprintln!("DARWIN FS: authorized APK flags rejected error={error}");
                }
                return self.fail(error);
            }
            let file = match File::open(apk) {
                Ok(file) => file,
                Err(error) => {
                    if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                        eprintln!("DARWIN FS: authorized APK host open failed error={error}");
                    }
                    return self.fail_io(&error);
                }
            };
            let mut descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            return match descriptors
                .insert_with_flags(Descriptor::File(file), flags & O_CLOEXEC != 0)
            {
                Ok(fd) => fd,
                Err(()) => self.fail(ANDROID_EMFILE),
            };
        }
        if let Some(native_path) = self.authorized_host_native_path(path) {
            if let Err(error) = self.validate_immutable_flags(flags) {
                return self.fail(error);
            }
            let file = match File::open(native_path) {
                Ok(file) => file,
                Err(error) => return self.fail_io(&error),
            };
            let mut descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            return match descriptors
                .insert_with_flags(Descriptor::File(file), flags & O_CLOEXEC != 0)
            {
                Ok(fd) => fd,
                Err(()) => self.fail(ANDROID_EMFILE),
            };
        }
        if let Some(private_path) = self.authorized_host_private_candidate(path) {
            if std::env::var_os("DARWIN_ART_DEBUG_PRIVATE_HOST").is_some() {
                eprintln!(
                    "DARWIN FS: private host candidate path={} flags={flags:#x}",
                    String::from_utf8_lossy(path)
                );
            }
            // Route reads through the same fd-relative no-follow walk as
            // creates and writes. A metadata/canonicalize check followed by
            // File::open would reintroduce a symlink race at the final node.
            return self.open_authorized_host_private(private_path, flags, mode);
        }
        if let Some(private_path) = self.authorized_host_private_path(path) {
            if let Err(error) = self.validate_immutable_flags(flags) {
                return self.fail(error);
            }
            let file = match File::open(private_path) {
                Ok(file) => file,
                Err(error) => return self.fail_io(&error),
            };
            let mut descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            return match descriptors
                .insert_with_flags(Descriptor::File(file), flags & O_CLOEXEC != 0)
            {
                Ok(fd) => fd,
                Err(()) => self.fail(ANDROID_EMFILE),
            };
        }
        let cwd = match self.namespace.cwd.named_lease() {
            Ok(cwd) => cwd,
            Err(error) => return self.fail_io(&error),
        };
        let resolution = match self.resolve_from(&cwd.path, path) {
            Ok(resolution) => resolution,
            Err(error) => {
                if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                    eprintln!(
                        "DARWIN FS: open resolve failed path={} errno={error}",
                        String::from_utf8_lossy(path)
                    );
                }
                return self.fail(error);
            }
        };
        if self.writable_root(resolution.mount_id).is_some() {
            return self.open_writable(path, resolution, flags, mode, &cwd.path);
        }
        if resolution.mount_id == 2 {
            return self.open_overlay(resolution, flags, mode);
        }
        if let Err(error) = self.validate_immutable_flags(flags) {
            return self.fail(error);
        }
        let result = self.open_immutable(path, resolution, flags, &cwd);
        if std::env::var_os("DARWIN_ART_FS_TRACE").is_some()
            && (path.windows(7).any(|part| part == b"/fonts/") || path.ends_with(b"fonts.xml"))
        {
            eprintln!(
                "DARWIN FS: open path={} fd={result}",
                String::from_utf8_lossy(path)
            );
        }
        result
    }

    fn open_overlay(&self, resolution: Resolution, flags: c_int, mode: u32) -> c_int {
        if resolution.requires_directory || resolution.relative_path.is_empty() {
            return self.fail(ANDROID_EISDIR);
        }
        if let Err(error) = self.validate_overlay_flags(flags) {
            return self.fail(error);
        }
        let access = flags & O_ACCMODE;
        let mut overlay = match self.overlay.lock() {
            Ok(overlay) => overlay,
            Err(_) => return self.fail_capability(),
        };
        let parent = Self::overlay_parent(&resolution.relative_path);
        if !matches!(
            overlay.entries.get(parent),
            Some(OverlayEntry::Directory(_))
        ) {
            return self.fail(ANDROID_ENOENT);
        }
        let existing = overlay.entries.get(&resolution.relative_path);
        if flags & O_CREAT != 0 && flags & O_EXCL != 0 && existing.is_some() {
            return self.fail(ANDROID_EEXIST);
        }
        let node = match existing {
            Some(OverlayEntry::File(node)) => Arc::clone(node),
            Some(OverlayEntry::Directory(_)) => return self.fail(ANDROID_EISDIR),
            None if flags & O_CREAT == 0 => return self.fail(ANDROID_ENOENT),
            None => {
                let inode = overlay.next_inode;
                overlay.next_inode = overlay.next_inode.saturating_add(1);
                let node = Arc::new(Mutex::new(OverlayFile {
                    inode,
                    mode: ANDROID_S_IFREG | (mode & 0o7777),
                    data: Vec::new(),
                }));
                overlay.entries.insert(
                    resolution.relative_path.clone(),
                    OverlayEntry::File(Arc::clone(&node)),
                );
                node
            }
        };
        if flags & O_TRUNC != 0 {
            let mut file = match node.lock() {
                Ok(file) => file,
                Err(_) => return self.fail_capability(),
            };
            file.data.clear();
        }
        drop(overlay);
        let descriptor = Descriptor::Overlay(OverlayDescriptor {
            node,
            offset: 0,
            readable: access == O_RDONLY,
            writable: access == O_WRONLY,
        });
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(descriptor, flags & O_CLOEXEC != 0) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }

    unsafe fn read(&self, fd: c_int, buffer: *mut c_void, count: usize) -> isize {
        if count > isize::MAX as usize {
            return self.fail(ANDROID_EINVAL) as isize;
        }
        if buffer.is_null() && count != 0 {
            return self.fail(ANDROID_EFAULT) as isize;
        }
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as isize,
        };
        let Some(descriptor) = descriptors.entries.get_mut(&fd) else {
            return self.fail(ANDROID_EBADF) as isize;
        };
        // SAFETY: the guest ABI requires a writable buffer for count bytes. Zero length
        // accepts a null pointer, represented here with a dangling aligned pointer.
        let pointer = if count == 0 {
            ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            buffer.cast::<u8>()
        };
        let bytes = unsafe { slice::from_raw_parts_mut(pointer, count) };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => match file.read(bytes) {
                Ok(read) => read as isize,
                Err(error) => self.fail_io(&error) as isize,
            },
            Descriptor::Random(_) => match self.entropy.fill(bytes) {
                Ok(()) => count as isize,
                Err(()) => self.fail(ANDROID_EIO) as isize,
            },
            Descriptor::Overlay(descriptor) => {
                if !descriptor.readable {
                    return self.fail(ANDROID_EBADF) as isize;
                }
                let file = match descriptor.node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability() as isize,
                };
                let start = usize::try_from(descriptor.offset).unwrap_or(usize::MAX);
                let available = file.data.len().saturating_sub(start);
                let copied = available.min(bytes.len());
                if copied != 0 {
                    bytes[..copied].copy_from_slice(&file.data[start..start + copied]);
                    descriptor.offset += copied as u64;
                }
                copied as isize
            }
        }
    }

    unsafe fn write(&self, fd: c_int, buffer: *const c_void, count: usize) -> isize {
        if count > isize::MAX as usize {
            return self.fail(ANDROID_EINVAL) as isize;
        }
        if buffer.is_null() && count != 0 {
            return self.fail(ANDROID_EFAULT) as isize;
        }
        let pointer = if count == 0 {
            ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            buffer.cast::<u8>()
        };
        // SAFETY: the guest ABI requires a readable buffer for count bytes.
        let bytes = unsafe { slice::from_raw_parts(pointer, count) };
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as isize,
        };
        let Some(descriptor) = descriptors.entries.get_mut(&fd) else {
            return self.fail(ANDROID_EBADF) as isize;
        };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => match file.write(bytes) {
                Ok(written) => written as isize,
                Err(error) => self.fail_io(&error) as isize,
            },
            Descriptor::Random(_) => self.fail(ANDROID_EBADF) as isize,
            Descriptor::Overlay(descriptor) => {
                if !descriptor.writable {
                    return self.fail(ANDROID_EBADF) as isize;
                }
                let mut file = match descriptor.node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability() as isize,
                };
                let start = match usize::try_from(descriptor.offset) {
                    Ok(start) => start,
                    Err(_) => return self.fail(ANDROID_EINVAL) as isize,
                };
                let Some(end) = start.checked_add(bytes.len()) else {
                    return self.fail(ANDROID_EINVAL) as isize;
                };
                if start > file.data.len() {
                    file.data.resize(start, 0);
                }
                if end > file.data.len() {
                    file.data.resize(end, 0);
                }
                file.data[start..end].copy_from_slice(bytes);
                descriptor.offset = end as u64;
                bytes.len() as isize
            }
        }
    }

    unsafe fn readv(&self, fd: c_int, vectors: *const NativeIovec, count: c_int) -> isize {
        let vectors = unsafe { slice::from_raw_parts(vectors, count as usize) };
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as isize,
        };
        let Some(descriptor) = descriptors.entries.get_mut(&fd) else {
            return self.fail(ANDROID_EBADF) as isize;
        };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                // One host readv preserves POSIX short-read and blocking
                // semantics across all buffers; a loop of read() calls is not
                // equivalent for pipes, stream endpoints, or interruption
                // boundaries.
                let result = unsafe { host_readv(file.as_raw_fd(), vectors.as_ptr(), count) };
                if result < 0 {
                    self.fail_io(&std::io::Error::last_os_error()) as isize
                } else {
                    result
                }
            }
            Descriptor::Random(_) | Descriptor::Overlay(_) => {
                let mut total = 0usize;
                for vector in vectors {
                    if vector.base.is_null() && vector.length != 0 {
                        return self.fail(ANDROID_EFAULT) as isize;
                    }
                    let pointer = if vector.length == 0 {
                        ptr::NonNull::<u8>::dangling().as_ptr()
                    } else {
                        vector.base.cast::<u8>()
                    };
                    let bytes = unsafe { slice::from_raw_parts_mut(pointer, vector.length) };
                    let result = match descriptor {
                        Descriptor::Random(_) => match self.entropy.fill(bytes) {
                            Ok(()) => bytes.len(),
                            Err(()) => return self.fail(ANDROID_EIO) as isize,
                        },
                        Descriptor::Overlay(overlay) => {
                            if !overlay.readable {
                                return self.fail(ANDROID_EBADF) as isize;
                            }
                            let file = match overlay.node.lock() {
                                Ok(file) => file,
                                Err(_) => return self.fail_capability() as isize,
                            };
                            let start = usize::try_from(overlay.offset).unwrap_or(usize::MAX);
                            let copied = file.data.len().saturating_sub(start).min(bytes.len());
                            if copied != 0 {
                                bytes[..copied].copy_from_slice(&file.data[start..start + copied]);
                                overlay.offset += copied as u64;
                            }
                            copied
                        }
                        _ => unreachable!(),
                    };
                    total = total.saturating_add(result);
                    if result != bytes.len() {
                        break;
                    }
                }
                total as isize
            }
        }
    }

    unsafe fn writev(&self, fd: c_int, vectors: *const NativeIovec, count: c_int) -> isize {
        let vectors = unsafe { slice::from_raw_parts(vectors, count as usize) };
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as isize,
        };
        let Some(descriptor) = descriptors.entries.get_mut(&fd) else {
            return self.fail(ANDROID_EBADF) as isize;
        };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                let result = unsafe { host_writev(file.as_raw_fd(), vectors.as_ptr(), count) };
                if result < 0 {
                    self.fail_io(&std::io::Error::last_os_error()) as isize
                } else {
                    result
                }
            }
            Descriptor::Random(_) => self.fail(ANDROID_EBADF) as isize,
            Descriptor::Overlay(_) => {
                let mut total = 0usize;
                for vector in vectors {
                    if vector.base.is_null() && vector.length != 0 {
                        return self.fail(ANDROID_EFAULT) as isize;
                    }
                    let pointer = if vector.length == 0 {
                        ptr::NonNull::<u8>::dangling().as_ptr()
                    } else {
                        vector.base.cast_const().cast::<u8>()
                    };
                    let bytes = unsafe { slice::from_raw_parts(pointer, vector.length) };
                    let Descriptor::Overlay(overlay) = descriptor else {
                        unreachable!()
                    };
                    if !overlay.writable {
                        return self.fail(ANDROID_EBADF) as isize;
                    }
                    let mut file = match overlay.node.lock() {
                        Ok(file) => file,
                        Err(_) => return self.fail_capability() as isize,
                    };
                    let start = match usize::try_from(overlay.offset) {
                        Ok(start) => start,
                        Err(_) => return self.fail(ANDROID_EINVAL) as isize,
                    };
                    let Some(end) = start.checked_add(bytes.len()) else {
                        return self.fail(ANDROID_EINVAL) as isize;
                    };
                    if end > file.data.len() {
                        file.data.resize(end, 0);
                    }
                    file.data[start..end].copy_from_slice(bytes);
                    overlay.offset = end as u64;
                    total = total.saturating_add(bytes.len());
                }
                total as isize
            }
        }
    }

    unsafe fn pread(&self, fd: c_int, buffer: *mut c_void, count: usize, offset: i64) -> isize {
        if count > isize::MAX as usize || offset < 0 {
            return self.fail(ANDROID_EINVAL) as isize;
        }
        if buffer.is_null() && count != 0 {
            return self.fail(ANDROID_EFAULT) as isize;
        }
        let pointer = if count == 0 {
            ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            buffer.cast::<u8>()
        };
        let bytes = unsafe { slice::from_raw_parts_mut(pointer, count) };
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as isize,
        };
        let Some(descriptor) = descriptors.entries.get(&fd) else {
            return self.fail(ANDROID_EBADF) as isize;
        };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                match file.read_at(bytes, offset as u64) {
                    Ok(read) => read as isize,
                    Err(error) => self.fail_io(&error) as isize,
                }
            }
            Descriptor::Random(_) => match self.entropy.fill(bytes) {
                Ok(()) => count as isize,
                Err(()) => self.fail(ANDROID_EIO) as isize,
            },
            Descriptor::Overlay(descriptor) => {
                if !descriptor.readable {
                    return self.fail(ANDROID_EBADF) as isize;
                }
                let file = match descriptor.node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability() as isize,
                };
                let start = offset as usize;
                let copied = file.data.len().saturating_sub(start).min(bytes.len());
                if copied != 0 {
                    bytes[..copied].copy_from_slice(&file.data[start..start + copied]);
                }
                copied as isize
            }
        }
    }

    unsafe fn pwrite(&self, fd: c_int, buffer: *const c_void, count: usize, offset: i64) -> isize {
        if count > isize::MAX as usize || offset < 0 {
            return self.fail(ANDROID_EINVAL) as isize;
        }
        if buffer.is_null() && count != 0 {
            return self.fail(ANDROID_EFAULT) as isize;
        }
        let pointer = if count == 0 {
            ptr::NonNull::<u8>::dangling().as_ptr()
        } else {
            buffer.cast::<u8>()
        };
        // SAFETY: the guest ABI requires a readable buffer for count bytes.
        let bytes = unsafe { slice::from_raw_parts(pointer, count) };
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as isize,
        };
        let Some(descriptor) = descriptors.entries.get(&fd) else {
            return self.fail(ANDROID_EBADF) as isize;
        };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                match file.write_at(bytes, offset as u64) {
                    Ok(written) => written as isize,
                    Err(error) => self.fail_io(&error) as isize,
                }
            }
            Descriptor::Random(_) => self.fail(ANDROID_EBADF) as isize,
            Descriptor::Overlay(descriptor) => {
                if !descriptor.writable {
                    return self.fail(ANDROID_EBADF) as isize;
                }
                let mut file = match descriptor.node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability() as isize,
                };
                let start = offset as usize;
                let Some(end) = start.checked_add(bytes.len()) else {
                    return self.fail(ANDROID_EINVAL) as isize;
                };
                if start > file.data.len() {
                    file.data.resize(start, 0);
                }
                if end > file.data.len() {
                    file.data.resize(end, 0);
                }
                file.data[start..end].copy_from_slice(bytes);
                bytes.len() as isize
            }
        }
    }

    fn lseek(&self, fd: c_int, offset: i64, whence: c_int) -> i64 {
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability() as i64,
        };
        let Some(descriptor) = descriptors.entries.get_mut(&fd) else {
            return self.fail(ANDROID_EBADF) as i64;
        };
        match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                let seek = match whence {
                    0 if offset >= 0 => SeekFrom::Start(offset as u64),
                    1 => SeekFrom::Current(offset),
                    2 => SeekFrom::End(offset),
                    _ => return self.fail(ANDROID_EINVAL) as i64,
                };
                match file.seek(seek) {
                    Ok(position) => {
                        i64::try_from(position).unwrap_or_else(|_| self.fail(ANDROID_EINVAL) as i64)
                    }
                    Err(error) => self.fail_io(&error) as i64,
                }
            }
            Descriptor::Random(_) => self.fail(ANDROID_EINVAL) as i64,
            Descriptor::Overlay(descriptor) => {
                let length = match descriptor.node.lock() {
                    Ok(file) => file.data.len() as i128,
                    Err(_) => return self.fail_capability() as i64,
                };
                let base = match whence {
                    0 => 0,
                    1 => descriptor.offset as i128,
                    2 => length,
                    _ => return self.fail(ANDROID_EINVAL) as i64,
                };
                let next = base + offset as i128;
                if !(0..=i64::MAX as i128).contains(&next) {
                    return self.fail(ANDROID_EINVAL) as i64;
                }
                descriptor.offset = next as u64;
                next as i64
            }
        }
    }

    fn close(&self, fd: c_int) -> c_int {
        let descriptor = {
            let mut descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            let Some(descriptor) = descriptors.close_entry(fd) else {
                return self.fail(ANDROID_EBADF);
            };
            descriptor
        };
        let file = match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => file,
            _ => return 0,
        };
        let raw = file.into_raw_fd();
        // SAFETY: into_raw_fd transfers this one live descriptor to host close.
        if unsafe { host_close(raw) } == 0 {
            0
        } else {
            let error = std::io::Error::last_os_error();
            self.fail_io(&error)
        }
    }

    fn flock(&self, fd: c_int, operation: c_int) -> c_int {
        if operation & !0x0f != 0 || operation & 0x0b == 0 {
            return self.fail(ANDROID_EINVAL);
        }
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        let Some(Descriptor::PrivateFile(file)) = descriptors.entries.get(&fd) else {
            return self.fail(ANDROID_EBADF);
        };
        // Android and Darwin share LOCK_SH/EX/NB/UN bit values. The virtual
        // descriptor remains leased while the host applies the advisory lock.
        if unsafe { host_flock(file.as_raw_fd(), operation) } == 0 {
            0
        } else {
            self.fail_io(&std::io::Error::last_os_error())
        }
    }

    fn fcntl(&self, fd: c_int, command: c_int, argument: isize) -> c_int {
        const F_DUPFD: c_int = 0;
        const F_GETFD: c_int = 1;
        const F_SETFD: c_int = 2;
        const F_GETFL: c_int = 3;
        const F_SETFL: c_int = 4;
        const F_DUPFD_CLOEXEC: c_int = 1030;
        if matches!(command, F_DUPFD | F_DUPFD_CLOEXEC) {
            if argument < 0 {
                return self.fail(ANDROID_EINVAL);
            }
            let mut descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            let duplicate = match descriptors.entries.get(&fd) {
                Some(Descriptor::File(file)) => match file.try_clone() {
                    Ok(file) => Descriptor::File(file),
                    Err(error) => return self.fail_io(&error),
                },
                Some(Descriptor::PrivateFile(file)) => match file.try_clone() {
                    Ok(file) => Descriptor::PrivateFile(file),
                    Err(error) => return self.fail_io(&error),
                },
                Some(Descriptor::Random(kind)) => Descriptor::Random(*kind),
                Some(Descriptor::Overlay(_)) => return self.fail(ANDROID_EOPNOTSUPP),
                None => return self.fail(ANDROID_EBADF),
            };
            let origin = descriptors.origin(fd).cloned();
            return match descriptors.insert_with_origin(
                duplicate,
                command == F_DUPFD_CLOEXEC,
                origin,
            ) {
                Ok(duplicate_fd) if (duplicate_fd as isize) >= argument => duplicate_fd,
                Ok(duplicate_fd) => {
                    let _ = descriptors.close_entry(duplicate_fd);
                    self.fail(ANDROID_EMFILE)
                }
                Err(()) => self.fail(ANDROID_EMFILE),
            };
        }
        if matches!(command, F_GETFD | F_SETFD | F_GETFL | F_SETFL) {
            if command == F_SETFD && argument & !(FD_CLOEXEC as isize) != 0 {
                return self.fail(ANDROID_EINVAL);
            }
            if command == F_SETFL && argument & !(O_APPEND | O_NONBLOCK) as isize != 0 {
                return self.fail(ANDROID_EINVAL);
            }
            let mut descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            if command == F_GETFD {
                return descriptors
                    .fd_flags(fd)
                    .unwrap_or_else(|| self.fail(ANDROID_EBADF));
            }
            if command == F_SETFD {
                if descriptors.set_fd_flags(fd, argument as c_int) {
                    return 0;
                }
                return self.fail(ANDROID_EBADF);
            }
            let Some(descriptor) = descriptors.entries.get(&fd) else {
                return self.fail(ANDROID_EBADF);
            };
            let access = match descriptor {
                Descriptor::File(_) | Descriptor::Random(_) => O_RDONLY,
                Descriptor::PrivateFile(file) => {
                    // Darwin and Android share the access-mode low bits. The
                    // remaining status flags are translated explicitly below.
                    let flags = unsafe { host_fcntl(file.as_raw_fd(), F_GETFL) };
                    if flags < 0 {
                        return self.fail_io(&std::io::Error::last_os_error());
                    }
                    flags & O_ACCMODE
                }
                Descriptor::Overlay(file) => match (file.readable, file.writable) {
                    (true, true) => O_RDWR,
                    (false, true) => O_WRONLY,
                    _ => O_RDONLY,
                },
            };
            if command == F_GETFL {
                let mut android_flags = access;
                if let Descriptor::PrivateFile(file) = descriptor {
                    let host_flags = unsafe { host_fcntl(file.as_raw_fd(), F_GETFL) };
                    if host_flags < 0 {
                        return self.fail_io(&std::io::Error::last_os_error());
                    }
                    const DARWIN_O_NONBLOCK: c_int = 0x4;
                    const DARWIN_O_APPEND: c_int = 0x8;
                    if host_flags & DARWIN_O_NONBLOCK != 0 {
                        android_flags |= O_NONBLOCK;
                    }
                    if host_flags & DARWIN_O_APPEND != 0 {
                        android_flags |= O_APPEND;
                    }
                }
                return android_flags;
            }
            let Descriptor::PrivateFile(file) = descriptor else {
                return 0;
            };
            const DARWIN_O_NONBLOCK: c_int = 0x4;
            const DARWIN_O_APPEND: c_int = 0x8;
            let current = unsafe { host_fcntl(file.as_raw_fd(), F_GETFL) };
            if current < 0 {
                return self.fail_io(&std::io::Error::last_os_error());
            }
            let mut updated = current & !(DARWIN_O_NONBLOCK | DARWIN_O_APPEND);
            if argument as c_int & O_NONBLOCK != 0 {
                updated |= DARWIN_O_NONBLOCK;
            }
            if argument as c_int & O_APPEND != 0 {
                updated |= DARWIN_O_APPEND;
            }
            let result = unsafe { host_fcntl(file.as_raw_fd(), F_SETFL, updated) };
            return if result == 0 {
                0
            } else {
                self.fail_io(&std::io::Error::last_os_error())
            };
        }
        if !matches!(command, 5..=7) {
            return self.fail(ANDROID_EINVAL);
        }
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        let Some(Descriptor::PrivateFile(file)) = descriptors.entries.get(&fd) else {
            return self.fail(ANDROID_EBADF);
        };
        let mut host_errno = 0;
        // SAFETY: the Android caller owns the flock object for this synchronous
        // call; the C adapter translates its arm64 layout to Darwin's layout.
        let result = unsafe {
            darwin_art_bionic_fs_host_record_lock(
                file.as_raw_fd(),
                command,
                argument,
                &mut host_errno,
            )
        };
        if result == 0 {
            return 0;
        }
        if result == -2 {
            return self.fail(ANDROID_EINVAL);
        }
        if host_errno != 0 {
            // SAFETY: this converts one Darwin errno into Android TLS errno.
            if unsafe { darwin_art_bionic_errno_set_from_darwin(host_errno) } != 0 {
                return -1;
            }
        }
        self.fail_capability()
    }

    unsafe fn fstat(&self, fd: c_int, status: *mut AndroidStat) -> c_int {
        if status.is_null() {
            return self.fail(ANDROID_EFAULT);
        }
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        let translated = if let Some(descriptor) = descriptors.entries.get(&fd) {
            match descriptor {
                Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                    let metadata = match file.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => return self.fail_io(&error),
                    };
                    metadata_to_android(&metadata)
                }
                Descriptor::Random(kind) => random_device_stat(*kind),
                Descriptor::Overlay(descriptor) => {
                    let file = match descriptor.node.lock() {
                        Ok(file) => file,
                        Err(_) => return self.fail_capability(),
                    };
                    overlay_file_stat(&file)
                }
            }
        } else {
            // External descriptor owners (ashmem today, and other brokered
            // Android descriptors later) retain policy and lifetime. fstat
            // borrows one owned host duplicate only long enough to translate
            // host metadata into Android's arm64 struct stat.
            drop(descriptors);
            let resolver = match HOST_DESCRIPTOR_RESOLVER.lock() {
                Ok(resolver) => *resolver,
                Err(_) => return self.fail_capability(),
            };
            let Some(resolver) = resolver else {
                return self.fail(ANDROID_EBADF);
            };
            let mut host_fd = -1;
            // SAFETY: host_fd is writable for the callback and status 1
            // transfers ownership of exactly one duplicate to this call.
            let resolution = unsafe { resolver(fd, &mut host_fd) };
            match resolution {
                0 => return self.fail(ANDROID_EBADF),
                -1 => return -1,
                1 if host_fd >= 0 => {
                    // SAFETY: resolution 1 transfers this descriptor. File
                    // closes it after metadata capture, so close/reuse races
                    // cannot invalidate the in-flight operation.
                    let duplicate = unsafe { File::from_raw_fd(host_fd) };
                    let metadata = match duplicate.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => return self.fail_io(&error),
                    };
                    if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                        eprintln!(
                            "DARWIN FS: fstat external fd={fd} host_fd={host_fd} size={}",
                            metadata.len()
                        );
                    }
                    metadata_to_android(&metadata)
                }
                _ => return self.fail_capability(),
            }
        };
        // SAFETY: the guest ABI requires a writable 128-byte Android stat object.
        unsafe { status.write(translated) };
        0
    }

    fn fsync(&self, fd: c_int) -> c_int {
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        let Some(descriptor) = descriptors.entries.get(&fd) else {
            return self.fail(ANDROID_EBADF);
        };
        let result = match descriptor {
            Descriptor::File(file) | Descriptor::PrivateFile(file) => file.sync_all(),
            // Synthetic overlay files have no host-side dirty pages. Their
            // contents are committed to the in-memory overlay by write().
            Descriptor::Overlay(_) => return 0,
            Descriptor::Random(_) => return self.fail(ANDROID_EINVAL),
        };
        match result {
            Ok(()) => 0,
            Err(error) => self.fail_io(&error),
        }
    }

    fn readlink(&self, path: &[u8], buffer: *mut c_char, size: usize) -> isize {
        if buffer.is_null() && size != 0 {
            return self.fail(ANDROID_EFAULT) as isize;
        }
        if size == 0 {
            return self.fail(ANDROID_EINVAL) as isize;
        }
        if path == b"/proc/self/exe" || path == b"/proc/thread-self/exe" {
            // Android application processes are forked from zygote's
            // app_process64 executable. Chromium uses this Linux identity to
            // derive process paths; expose the Android executable identity,
            // never the Darwin host binary or an authorized host path.
            const TARGET: &[u8] = b"/system/bin/app_process64";
            let copied = TARGET.len().min(size);
            // SAFETY: the readlink ABI guarantees a writable buffer for size
            // bytes. Like Linux readlink(2), the result is not NUL-terminated
            // and truncation returns exactly the copied byte count.
            unsafe {
                ptr::copy_nonoverlapping(TARGET.as_ptr(), buffer.cast::<u8>(), copied);
            }
            return copied as isize;
        }
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error) as isize,
        };
        match self.namespace.broker.open(&resolution.relative_path) {
            // A securely opened regular file or directory is definitively not
            // a symlink. Match Linux/Bionic readlink with EINVAL.
            Ok(_) => self.fail(ANDROID_EINVAL) as isize,
            Err(error) if is_final_symlink_rejection(&error, &resolution.relative_path) => {
                // The broker cannot safely return the final link's bytes.
                self.fail(ANDROID_EOPNOTSUPP) as isize
            }
            Err(error) => self.fail_broker(&error) as isize,
        }
    }

    fn reject_fd_mutation(&self, fd: c_int) -> c_int {
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        if descriptors.entries.contains_key(&fd) {
            self.fail(ANDROID_EROFS)
        } else {
            self.fail(ANDROID_EBADF)
        }
    }

    fn fchmod(&self, fd: c_int, mode: u32) -> c_int {
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.entries.get_mut(&fd) {
            Some(Descriptor::PrivateFile(file)) => {
                use std::os::unix::fs::PermissionsExt;
                match file.set_permissions(fs::Permissions::from_mode(mode & 0o7777)) {
                    Ok(()) => 0,
                    Err(error) => self.fail_io(&error),
                }
            }
            Some(Descriptor::Overlay(descriptor)) if descriptor.writable => {
                let mut file = match descriptor.node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability(),
                };
                file.mode = ANDROID_S_IFREG | (mode & 0o7777);
                0
            }
            Some(Descriptor::Overlay(_)) => self.fail(ANDROID_EBADF),
            Some(_) => self.fail(ANDROID_EROFS),
            None => self.fail(ANDROID_EBADF),
        }
    }

    fn chmod(&self, path: &[u8], mode: u32) -> c_int {
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        if resolution.mount_id != 2 {
            return self.fail(ANDROID_EROFS);
        }
        if let Some(root) = self.private_root.as_ref() {
            return match root.chmod(&resolution.relative_path, mode) {
                Ok(()) => 0,
                Err(error) => self.fail_io(&error),
            };
        }
        let mut overlay = match self.overlay.lock() {
            Ok(overlay) => overlay,
            Err(_) => return self.fail_capability(),
        };
        let result = match overlay.entries.get_mut(&resolution.relative_path) {
            Some(OverlayEntry::File(node)) => {
                let mut file = match node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability(),
                };
                file.mode = ANDROID_S_IFREG | (mode & 0o7777);
                0
            }
            Some(OverlayEntry::Directory(directory)) => {
                directory.mode = ANDROID_S_IFDIR | (mode & 0o7777);
                0
            }
            None => self.fail(ANDROID_ENOENT),
        };
        if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
            eprintln!(
                "DARWIN FS: chmod path={} relative={} mode={mode:o} result={result}",
                String::from_utf8_lossy(path),
                String::from_utf8_lossy(&resolution.relative_path)
            );
        }
        result
    }

    fn fchown(&self, fd: c_int, _owner: u32, _group: u32) -> c_int {
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.entries.get(&fd) {
            // Overlay nodes do not yet store guest ownership or authorize
            // transitions against process credentials. Never acknowledge a
            // change whose requested uid/gid would simply be discarded.
            Some(Descriptor::Overlay(_)) => self.fail(ANDROID_EOPNOTSUPP),
            Some(_) => self.fail(ANDROID_EROFS),
            None => self.fail(ANDROID_EBADF),
        }
    }

    fn ftruncate(&self, fd: c_int, length: i64) -> c_int {
        if length < 0 {
            return self.fail(ANDROID_EINVAL);
        }
        let Ok(length) = usize::try_from(length) else {
            return self.fail(ANDROID_EINVAL);
        };
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.entries.get_mut(&fd) {
            Some(Descriptor::PrivateFile(file)) => match file.set_len(length as u64) {
                Ok(()) => 0,
                Err(error) => self.fail_io(&error),
            },
            Some(Descriptor::Overlay(descriptor)) if descriptor.writable => {
                let mut file = match descriptor.node.lock() {
                    Ok(file) => file,
                    Err(_) => return self.fail_capability(),
                };
                file.data.resize(length, 0);
                0
            }
            Some(Descriptor::Overlay(_)) => self.fail(ANDROID_EBADF),
            Some(_) => self.fail(ANDROID_EROFS),
            None => self.fail(ANDROID_EBADF),
        }
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> c_int {
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        if !resolution.writable {
            return self.fail(ANDROID_EROFS);
        }
        if resolution.relative_path.is_empty() {
            return self.fail(ANDROID_EEXIST);
        }
        if let Some(root) = self.writable_root(resolution.mount_id) {
            let relative = match self.writable_relative(root, path) {
                Ok(relative) => relative,
                Err(error) => return self.fail(error),
            };
            return match root.mkdir(&relative, mode) {
                Ok(()) => 0,
                Err(error) => self.fail_io(&error),
            };
        }
        let mut overlay = match self.overlay.lock() {
            Ok(overlay) => overlay,
            Err(_) => return self.fail_capability(),
        };
        if overlay.entries.contains_key(&resolution.relative_path) {
            if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                eprintln!(
                    "DARWIN FS: mkdir exists path={}",
                    String::from_utf8_lossy(path)
                );
            }
            return self.fail(ANDROID_EEXIST);
        }
        if !matches!(
            overlay
                .entries
                .get(Self::overlay_parent(&resolution.relative_path)),
            Some(OverlayEntry::Directory(_))
        ) {
            if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
                eprintln!(
                    "DARWIN FS: mkdir missing-parent path={} relative={} parent={}",
                    String::from_utf8_lossy(path),
                    String::from_utf8_lossy(&resolution.relative_path),
                    String::from_utf8_lossy(Self::overlay_parent(&resolution.relative_path))
                );
            }
            return self.fail(ANDROID_ENOENT);
        }
        let inode = overlay.next_inode;
        overlay.next_inode = overlay.next_inode.saturating_add(1);
        overlay.entries.insert(
            resolution.relative_path,
            OverlayEntry::Directory(OverlayDirectory {
                inode,
                mode: ANDROID_S_IFDIR | (mode & 0o7777),
            }),
        );
        if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
            eprintln!(
                "DARWIN FS: mkdir path={} mode={mode:o} result=0",
                String::from_utf8_lossy(path)
            );
        }
        0
    }

    fn seed_private_directory(&self, path: &[u8]) -> c_int {
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        if resolution.mount_id != 2 {
            return self.fail(ANDROID_EROFS);
        }
        if let Some(root) = &self.private_root {
            return match root.seed_directory(&resolution.relative_path) {
                Ok(()) => 0,
                Err(error) => self.fail_io(&error),
            };
        }
        let mut overlay = match self.overlay.lock() {
            Ok(overlay) => overlay,
            Err(_) => return self.fail_capability(),
        };
        let mut prefix = Vec::new();
        for component in resolution
            .relative_path
            .split(|byte| *byte == b'/')
            .filter(|component| !component.is_empty())
        {
            if !prefix.is_empty() {
                prefix.push(b'/');
            }
            prefix.extend_from_slice(component);
            match overlay.entries.get(&prefix) {
                Some(OverlayEntry::Directory(_)) => continue,
                Some(OverlayEntry::File(_)) => return self.fail(ANDROID_ENOTDIR),
                None => {}
            }
            let inode = overlay.next_inode;
            overlay.next_inode = overlay.next_inode.saturating_add(1);
            overlay.entries.insert(
                prefix.clone(),
                OverlayEntry::Directory(OverlayDirectory {
                    inode,
                    mode: ANDROID_S_IFDIR | 0o700,
                }),
            );
        }
        if std::env::var_os("DARWIN_ART_FS_TRACE").is_some() {
            eprintln!(
                "DARWIN FS: seeded private directory path={} relative={}",
                String::from_utf8_lossy(path),
                String::from_utf8_lossy(&resolution.relative_path)
            );
        }
        0
    }

    fn resolve_at(&self, directory_fd: c_int, path: &[u8]) -> Result<Resolution, c_int> {
        if directory_fd == AT_FDCWD || path.starts_with(b"/") {
            return self.resolve(path);
        }
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => {
                self.fail_capability();
                return Err(ANDROID_EIO);
            }
        };
        match descriptors.entries.get(&directory_fd) {
            Some(Descriptor::Random(_)) => Err(ANDROID_ENOTDIR),
            Some(Descriptor::File(_) | Descriptor::PrivateFile(_)) => Err(ANDROID_EOPNOTSUPP),
            Some(Descriptor::Overlay(_)) => Err(ANDROID_ENOTDIR),
            None => Err(ANDROID_EBADF),
        }
    }

    fn reject_path_mutation(&self, path: &[u8]) -> c_int {
        match self.resolve(path) {
            Ok(_) => self.fail(ANDROID_EROFS),
            Err(error) => self.fail(error),
        }
    }

    fn reject_at_path_mutation(&self, directory_fd: c_int, path: &[u8]) -> c_int {
        match self.resolve_at(directory_fd, path) {
            Ok(_) => self.fail(ANDROID_EROFS),
            Err(error) => self.fail(error),
        }
    }

    fn reject_two_path_mutation(&self, old_path: &[u8], new_path: &[u8]) -> c_int {
        if let Err(error) = self.resolve(old_path) {
            return self.fail(error);
        }
        if let Err(error) = self.resolve(new_path) {
            return self.fail(error);
        }
        self.fail(ANDROID_EROFS)
    }

    fn isatty(&self, fd: c_int) -> c_int {
        let descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => return self.fail_capability(),
        };
        if descriptors.entries.contains_key(&fd) {
            // Brokered files, directories, and synthetic random devices are
            // not terminals. isatty returns zero and publishes ENOTTY.
            Self::set_android_errno(ANDROID_ENOTTY);
            0
        } else {
            Self::set_android_errno(ANDROID_EBADF);
            0
        }
    }

    fn pathconf(&self, path: &[u8], name: c_int) -> i64 {
        if !(PATHCONF_MIN..=PATHCONF_MAX).contains(&name) {
            return self.fail(ANDROID_EINVAL) as i64;
        }
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error) as i64,
        };
        let opened = match self.open_for_path_query(path, &resolution) {
            Ok(opened) => opened,
            Err(error) => return error as i64,
        };
        let mut value = -1_i64;
        let mut host_errno = 0;
        // SAFETY: the opened descriptor and both output cells live through the
        // call. The helper maps the semantic Android selector explicitly.
        let result = unsafe {
            darwin_art_bionic_fs_host_fpathconf(
                opened.as_raw_fd(),
                name,
                &mut value,
                &mut host_errno,
            )
        };
        match result {
            1 => value,
            0 => -1,
            _ => self.fail_host_errno(host_errno) as i64,
        }
    }

    unsafe fn statvfs(&self, path: &[u8], status: *mut AndroidStatvfs) -> c_int {
        if status.is_null() {
            return self.fail(ANDROID_EFAULT);
        }
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        let opened = match self.open_for_path_query(path, &resolution) {
            Ok(file) => file,
            Err(error) => return error,
        };
        let mut host = HostStatvfs::default();
        let mut host_errno = 0;
        // SAFETY: the descriptor and fixed-layout outputs live through the call.
        if unsafe {
            darwin_art_bionic_fs_host_fstatvfs(opened.as_raw_fd(), &mut host, &mut host_errno)
        } != 0
        {
            return self.fail_host_errno(host_errno);
        }
        let translated = AndroidStatvfs {
            f_bsize: host.f_bsize,
            f_frsize: host.f_frsize,
            f_blocks: host.f_blocks,
            f_bfree: host.f_bfree,
            f_bavail: host.f_bavail,
            f_files: host.f_files,
            f_ffree: host.f_ffree,
            f_favail: host.f_favail,
            f_fsid: host.f_fsid,
            // The guest mount is immutable even when its provider-local host
            // filesystem is writable. Only semantically translated bits cross.
            f_flag: host.f_flag | ANDROID_ST_RDONLY,
            f_namemax: host.f_namemax,
            reserved: [0; 6],
        };
        unsafe { status.write(translated) };
        0
    }

    fn fail_host_errno(&self, host_errno: c_int) -> c_int {
        if host_errno != 0 {
            // SAFETY: translation is value-only and preserves Darwin errno.
            if unsafe { darwin_art_bionic_errno_set_from_darwin(host_errno) } != 0 {
                return -1;
            }
        }
        self.fail_capability()
    }

    fn sendfile_transfer(&self, request: &SendfileRequest, result: &mut SendfileResult) -> c_int {
        *result = SendfileResult {
            abi_version: SENDFILE_ABI_VERSION,
            android_errno: 0,
            transferred: -1,
            next_offset: request.offset,
        };
        if request.abi_version != SENDFILE_ABI_VERSION
            || request.has_explicit_offset > 1
            || (request.has_explicit_offset != 0 && request.offset < 0)
            || request.output_fd == request.input_fd
        {
            result.android_errno = ANDROID_EINVAL;
            return SENDFILE_TRANSFER_OK;
        }
        let mut descriptors = match self.descriptors.lock() {
            Ok(descriptors) => descriptors,
            Err(_) => {
                self.capability_failure.store(true, Ordering::Release);
                return SENDFILE_TRANSFER_UNAVAILABLE;
            }
        };
        if !descriptors.entries.contains_key(&request.input_fd)
            || !descriptors.entries.contains_key(&request.output_fd)
        {
            return SENDFILE_TRANSFER_BAD_FD;
        }
        let Some(mut output) = descriptors.take(request.output_fd) else {
            return SENDFILE_TRANSFER_BAD_FD;
        };
        let transfer_result = (|| {
            let input = descriptors
                .entries
                .get_mut(&request.input_fd)
                .ok_or(ANDROID_EBADF)?;
            if let (Descriptor::Overlay(input_descriptor), Descriptor::Overlay(output_descriptor)) =
                (&*input, &output)
                && Arc::ptr_eq(&input_descriptor.node, &output_descriptor.node)
            {
                return Err(ANDROID_EINVAL);
            }
            let wanted = request.count.min(64 * 1024);
            let mut bytes = vec![0_u8; wanted];
            let read = match input {
                Descriptor::File(file) | Descriptor::PrivateFile(file)
                    if request.has_explicit_offset != 0 =>
                {
                    file.read_at(&mut bytes, request.offset as u64)
                        .map_err(|_| ANDROID_EIO)?
                }
                Descriptor::File(file) | Descriptor::PrivateFile(file) => {
                    file.read(&mut bytes).map_err(|_| ANDROID_EIO)?
                }
                Descriptor::Overlay(input_descriptor) => {
                    if !input_descriptor.readable {
                        return Err(ANDROID_EBADF);
                    }
                    let input_file = input_descriptor.node.lock().map_err(|_| ANDROID_EIO)?;
                    let offset = if request.has_explicit_offset != 0 {
                        request.offset as u64
                    } else {
                        input_descriptor.offset
                    };
                    let start = usize::try_from(offset).unwrap_or(usize::MAX);
                    let copied = input_file.data.len().saturating_sub(start).min(bytes.len());
                    if copied != 0 {
                        bytes[..copied].copy_from_slice(&input_file.data[start..start + copied]);
                    }
                    drop(input_file);
                    if request.has_explicit_offset == 0 {
                        input_descriptor.offset += copied as u64;
                    }
                    copied
                }
                Descriptor::Random(_) => return Err(ANDROID_EINVAL),
            };
            bytes.truncate(read);
            match &mut output {
                Descriptor::PrivateFile(file) => file.write(&bytes).map_err(|_| ANDROID_EIO),
                Descriptor::Overlay(output_descriptor) => {
                    if !output_descriptor.writable {
                        return Err(ANDROID_EBADF);
                    }
                    let mut output_file = output_descriptor.node.lock().map_err(|_| ANDROID_EIO)?;
                    let output_start =
                        usize::try_from(output_descriptor.offset).map_err(|_| ANDROID_EINVAL)?;
                    let output_end = output_start
                        .checked_add(bytes.len())
                        .ok_or(ANDROID_EINVAL)?;
                    if output_file.data.len() < output_end {
                        output_file.data.resize(output_end, 0);
                    }
                    output_file.data[output_start..output_end].copy_from_slice(&bytes);
                    output_descriptor.offset += bytes.len() as u64;
                    Ok(bytes.len())
                }
                Descriptor::File(_) => Err(ANDROID_EROFS),
                Descriptor::Random(_) => Err(ANDROID_EINVAL),
            }
        })();
        descriptors.restore(request.output_fd, output);
        match transfer_result {
            Ok(transferred) => {
                result.transferred = transferred as isize;
                result.next_offset = match request.offset.checked_add(transferred as i64) {
                    Some(offset) => offset,
                    None => {
                        result.transferred = -1;
                        result.android_errno = ANDROID_EINVAL;
                        request.offset
                    }
                };
                SENDFILE_TRANSFER_OK
            }
            Err(error) => {
                if error == ANDROID_EIO {
                    self.capability_failure.store(true, Ordering::Release);
                }
                result.android_errno = error;
                SENDFILE_TRANSFER_OK
            }
        }
    }
}

fn find_authorized_host_apk_path<'a>(
    base: Option<&'a PathBuf>,
    splits: &'a [PathBuf],
    requested: &[u8],
) -> Option<&'a std::path::Path> {
    base.into_iter()
        .chain(splits.iter())
        .find(|apk| apk.as_os_str().as_bytes() == requested)
        .map(PathBuf::as_path)
}

#[cfg(test)]
mod authorized_host_apk_tests {
    use super::find_authorized_host_apk_path;
    use std::path::PathBuf;

    #[test]
    fn exact_base_and_split_files_are_authorized_but_unrelated_paths_are_not() {
        let base = PathBuf::from("/private/app/base.apk");
        let splits = vec![PathBuf::from("/private/app/config.arm64_v8a.apk")];

        assert_eq!(
            find_authorized_host_apk_path(Some(&base), &splits, b"/private/app/base.apk"),
            Some(base.as_path())
        );
        assert_eq!(
            find_authorized_host_apk_path(
                Some(&base),
                &splits,
                b"/private/app/config.arm64_v8a.apk"
            ),
            Some(splits[0].as_path())
        );
        assert_eq!(
            find_authorized_host_apk_path(Some(&base), &splits, b"/private/app/other.apk"),
            None
        );
        assert_eq!(
            find_authorized_host_apk_path(Some(&base), &splits, b"/private/app"),
            None
        );
    }
}

fn is_final_symlink_rejection(error: &BrokerError, relative_path: &[u8]) -> bool {
    if error.raw_os_error() != Some(DARWIN_ELOOP) || relative_path.is_empty() {
        return false;
    }
    let final_index = relative_path.split(|byte| *byte == b'/').count() - 1;
    matches!(
        error,
        BrokerError::Io {
            component_index: Some(index),
            ..
        } if *index == final_index
    )
}

fn android_directory_type(host_type: u8) -> u8 {
    match host_type {
        1 | 2 | 4 | 6 | 8 | 10 | 12 | 14 => host_type,
        _ => 0,
    }
}

fn metadata_to_android(metadata: &Metadata) -> AndroidStat {
    AndroidStat {
        st_dev: metadata.dev(),
        st_ino: metadata.ino(),
        st_mode: metadata.mode(),
        st_nlink: metadata.nlink() as u32,
        st_uid: metadata.uid(),
        st_gid: metadata.gid(),
        st_rdev: metadata.rdev(),
        pad1: 0,
        st_size: metadata.size() as i64,
        st_blksize: metadata.blksize() as i32,
        pad2: 0,
        st_blocks: metadata.blocks() as i64,
        st_atim: AndroidTimespec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        st_mtim: AndroidTimespec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
        st_ctim: AndroidTimespec {
            tv_sec: metadata.ctime(),
            tv_nsec: metadata.ctime_nsec(),
        },
        unused4: 0,
        unused5: 0,
    }
}

fn random_device_stat(kind: RandomDeviceKind) -> AndroidStat {
    let (inode, device) = match kind {
        RandomDeviceKind::Random => (1, ANDROID_RANDOM_RDEV),
        RandomDeviceKind::Urandom => (2, ANDROID_URANDOM_RDEV),
    };
    AndroidStat {
        st_dev: 0,
        st_ino: inode,
        st_mode: ANDROID_RANDOM_MODE,
        st_nlink: 1,
        st_uid: 0,
        st_gid: 0,
        st_rdev: device,
        pad1: 0,
        st_size: 0,
        st_blksize: 4096,
        pad2: 0,
        st_blocks: 0,
        st_atim: AndroidTimespec::default(),
        st_mtim: AndroidTimespec::default(),
        st_ctim: AndroidTimespec::default(),
        unused4: 0,
        unused5: 0,
    }
}

fn overlay_file_stat(file: &OverlayFile) -> AndroidStat {
    let size = file.data.len() as i64;
    AndroidStat {
        st_dev: 2,
        st_ino: file.inode,
        st_mode: file.mode,
        st_nlink: 1,
        st_uid: 0,
        st_gid: 0,
        st_rdev: 0,
        pad1: 0,
        st_size: size,
        st_blksize: 4096,
        pad2: 0,
        st_blocks: (size + 511) / 512,
        st_atim: AndroidTimespec::default(),
        st_mtim: AndroidTimespec::default(),
        st_ctim: AndroidTimespec::default(),
        unused4: 0,
        unused5: 0,
    }
}

fn overlay_directory_stat(directory: OverlayDirectory) -> AndroidStat {
    AndroidStat {
        st_dev: 2,
        st_ino: directory.inode,
        st_mode: directory.mode,
        st_nlink: 1,
        st_uid: 0,
        st_gid: 0,
        st_rdev: 0,
        pad1: 0,
        st_size: 0,
        st_blksize: 4096,
        pad2: 0,
        st_blocks: 0,
        st_atim: AndroidTimespec::default(),
        st_mtim: AndroidTimespec::default(),
        st_ctim: AndroidTimespec::default(),
        unused4: 0,
        unused5: 0,
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<Facade>>> = const { RefCell::new(None) };
}

pub const PROCESS_OWNER_OK: c_int = 0;
pub const PROCESS_OWNER_INVALID_ARGUMENT: c_int = 1;
pub const PROCESS_OWNER_ALREADY_INSTALLED: c_int = 2;
pub const PROCESS_OWNER_CREATE_FAILED: c_int = 3;
pub const PROCESS_OWNER_NOT_INSTALLED: c_int = 4;
pub const PROCESS_OWNER_BUSY: c_int = 5;

// The process-wide owner, activation leases, and exported C ABI form one
// safety boundary. Keep it in the parent module's scope so the internal
// filesystem model stays private while the ABI lifecycle remains reviewable.
include!("process_api.rs");
