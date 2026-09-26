//! Persistent external-file layout migration at the macOS filesystem boundary.
//! Move, never copy or merge: all existing file identities survive. Android
//! sees only /storage; the old host path remains an alias for legacy producers.
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

unsafe extern "C" {
    fn renameatx_np(
        from_fd: i32,
        from: *const libc::c_char,
        to_fd: i32,
        to: *const libc::c_char,
        flags: u32,
    ) -> i32;
}

struct Directory(File);
impl Directory {
    fn checked(file: File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::other(
                "storage authority must be an owned non-publicly-writable directory",
            ));
        }
        Ok(Self(file))
    }
    fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute()
            || path.parent().is_none()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(io::Error::other(
                "storage authority requires an absolute non-root path without parent traversal",
            ));
        }
        Self::checked(
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?,
        )
    }
    fn child(&self, name: &str) -> io::Result<Self> {
        let name = CString::new(name)?;
        // SAFETY: retained parent and one terminated component.
        if unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(error);
            }
        }
        self.existing(&name)
    }
    fn existing(&self, name: &std::ffi::CStr) -> io::Result<Self> {
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Self::checked(unsafe { File::from_raw_fd(fd) })
    }
    fn kind(&self, name: &std::ffi::CStr) -> io::Result<Option<libc::mode_t>> {
        let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                status.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Ok(Some(unsafe { status.assume_init() }.st_mode & libc::S_IFMT));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(error)
        }
    }
    fn lock(&self) -> io::Result<File> {
        // Two publishers creating the lock at once can see a transient ENOENT
        // from openat(O_CREAT) on macOS (#21); the name is ours, so retry.
        let mut attempts = 0;
        let fd = loop {
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    c".external-storage.lock".as_ptr(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0o600u32,
                )
            };
            if fd >= 0 {
                break fd;
            }
            let error = io::Error::last_os_error();
            attempts += 1;
            if error.raw_os_error() != Some(libc::ENOENT) || attempts == 8 {
                return Err(error);
            }
        };
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::other("invalid external-storage migration lock"));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }
}

/// Prepare one package's canonical external files without deleting user data.
/// Conflicts, foreign symlinks and cross-device moves fail explicitly. Closing
/// the owned lock releases it; a retry repairs an interrupted move-before-alias.
pub fn prepare(storage_root: &Path, app_data: &Path, package: &str) -> io::Result<PathBuf> {
    if package.is_empty()
        || package.len() > 255
        || package.split('.').any(|part| {
            let mut bytes = part.bytes();
            !bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(io::Error::other("invalid Android package name"));
    }
    let storage = Directory::open(storage_root)?;
    let source = Directory::open(app_data)?;
    if storage_root.starts_with(app_data) || app_data.starts_with(storage_root) {
        return Err(io::Error::other(
            "storage and app-data authorities must not overlap",
        ));
    }
    // Keep host migration bookkeeping outside the Android-visible mount.
    // RENAME_EXCL separately arbitrates destination conflicts across sources.
    let _lock = source.lock()?;
    let destination_path = storage_root
        .join("emulated/0/Android/data")
        .join(package)
        .join("files");
    let destination_text = CString::new(destination_path.as_os_str().as_bytes())?;
    let destination = storage
        .child("emulated")?
        .child("0")?
        .child("Android")?
        .child("data")?
        .child(package)?;
    let source_kind = source.kind(c"external")?;
    let target_kind = destination.kind(c"files")?;
    if target_kind.is_some_and(|kind| kind != libc::S_IFDIR) {
        return Err(io::Error::other(
            "external-files destination is not a real directory; preserved unchanged",
        ));
    }
    if source_kind == Some(libc::S_IFLNK) {
        let mut target = vec![0u8; 4096];
        let count = unsafe {
            libc::readlinkat(
                source.0.as_raw_fd(),
                c"external".as_ptr(),
                target.as_mut_ptr().cast(),
                target.len(),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if &target[..count as usize] != destination_text.as_bytes() || target_kind.is_none() {
            return Err(io::Error::other(
                "legacy external alias differs from canonical storage; preserved unchanged",
            ));
        }
        let _validated = destination.existing(c"files")?;
        return Ok(destination_path);
    }
    match (source_kind, target_kind) {
        (Some(libc::S_IFDIR), None) => {
            let original = source.existing(c"external")?.0.metadata()?;
            // macOS RENAME_EXCL (sys/stdio.h): never replace a concurrently
            // created target, and never fall back to cross-volume copying.
            if unsafe {
                renameatx_np(
                    source.0.as_raw_fd(),
                    c"external".as_ptr(),
                    destination.0.as_raw_fd(),
                    c"files".as_ptr(),
                    0x0000_0004,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            let moved = destination.existing(c"files")?.0.metadata()?;
            if original.dev() != moved.dev() || original.ino() != moved.ino() {
                return Err(io::Error::other(
                    "external source changed during migration; destination preserved without alias",
                ));
            }
        }
        (None, None) => {
            let _created = destination.child("files")?;
        }
        (None, Some(libc::S_IFDIR)) => {
            let _validated = destination.existing(c"files")?;
        }
        (Some(libc::S_IFDIR), Some(libc::S_IFDIR)) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "legacy and canonical external directories both exist; neither was merged or replaced",
            ));
        }
        _ => {
            return Err(io::Error::other(
                "legacy external storage is not a directory; preserved unchanged",
            ));
        }
    }
    if unsafe {
        libc::symlinkat(
            destination_text.as_ptr(),
            source.0.as_raw_fd(),
            c"external".as_ptr(),
        )
    } != 0
    {
        return Err(io::Error::other(format!(
            "external files preserved at {}; legacy alias creation failed: {}",
            destination_path.display(),
            io::Error::last_os_error()
        )));
    }
    Ok(destination_path)
}
