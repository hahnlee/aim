//! Retained authority for one writable Android mount, independent of package
//! policy. Its guest prefix is part of the authority, not a host pathname or
//! an implicit permission to mutate another mounted tree.
//! Namespace-level callers must select the destination authority for cross-
//! mount operations; this owner never silently grants a sibling's authority.
//!
//! The pathname is retained for exact host-capability comparisons, while the
//! opened directory descriptor is the authority for guest-relative opens.
//! Keeping both values together prevents a renamed/replaced directory from
//! redirecting those opens. mkdir/rename/remove also retain parent descriptors.
//! Recursive seeding and chmod use the same retained directory authority.
//! Configured parent traversal uses GuestRoot and checks the resolved writable
//! mount. Standalone authorities without a guest namespace reject symlinks;
//! they never delegate Android link interpretation to the host filesystem.

use darwin_art_fs_broker::guest_path::{GuestRoot, MountOrigin};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
mod file;
#[cfg(test)]
mod origin_tests;
mod xattr;

// A retained parent plus a single leaf is the boundary for Darwin *at calls.
// No host pathname is reopened after the authority has been acquired.
struct ParentEntry {
    directory: File,
    leaf: CString,
}

// Darwin fcntl.h values. These are host flags, not the Android flags used by
// the guest ABI in lib.rs. O_NOFOLLOW applies to the final configured root;
// the retained descriptor protects subsequent relative operations.
const DARWIN_O_NOFOLLOW: i32 = 0x0000_0100;
const DARWIN_O_DIRECTORY: i32 = 0x0010_0000;
const DARWIN_O_CLOEXEC: i32 = 0x0100_0000;

pub(super) struct WritableMount {
    path: PathBuf,
    guest_prefix: Vec<u8>,
    directory: File,
    guest_root: Option<Arc<GuestRoot>>,
    mount_origin: Option<MountOrigin>,
}

impl WritableMount {
    pub(super) fn open(path: PathBuf, guest_prefix: &[u8]) -> Result<Self, &'static str> {
        if guest_prefix == b"/"
            || !guest_prefix.starts_with(b"/")
            || guest_prefix.len() >= 4096
            || guest_prefix.contains(&0)
            || guest_prefix[1..]
                .split(|byte| *byte == b'/')
                .any(|part| part.is_empty() || part == b"." || part == b"..")
        {
            return Err("writable mount prefix must be a canonical guest submount");
        }
        if !path.is_absolute() {
            return Err("writable mount root must be absolute");
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(DARWIN_O_DIRECTORY | DARWIN_O_CLOEXEC | DARWIN_O_NOFOLLOW)
            .open(&path)
            .map_err(|_| "writable mount root cannot be opened")?;
        if !directory
            .metadata()
            .map_err(|_| "writable mount root cannot be inspected")?
            .is_dir()
        {
            return Err("writable mount root is not a directory");
        }
        Ok(Self {
            path,
            guest_prefix: guest_prefix.to_vec(),
            directory,
            guest_root: None,
            mount_origin: None,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn guest_prefix(&self) -> &[u8] {
        &self.guest_prefix
    }

    pub(super) fn directory(&self) -> &File {
        &self.directory
    }

    /// A new open-file description; cloning the authority would share readdir
    /// offsets with later opens of the same mounted directory.
    pub(super) fn reopen_directory(&self) -> io::Result<File> {
        // SAFETY: the retained authority is live and '.' is NUL terminated.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat transfers a fresh descriptor to this File.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// The configured guest mount must refer to this exact directory authority.
    /// No cycle: GuestRoot owns cloned descriptors, not this WritableMount.
    pub(super) fn attach_guest_root(&mut self, guest: Arc<GuestRoot>) -> io::Result<()> {
        let resolved = guest.open(&self.guest_prefix)?;
        if resolved.origin().mount_path() != self.guest_prefix {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
        let origin = resolved.origin().clone();
        let mounted = resolved.node.into_file().metadata()?;
        let owned = self.directory.metadata()?;
        if mounted.dev() != owned.dev() || mounted.ino() != owned.ino() {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
        self.guest_root = Some(guest);
        self.mount_origin = Some(origin);
        Ok(())
    }

    fn requires_mount_origin(&self, origin: &MountOrigin, error: i32) -> io::Result<()> {
        if self
            .mount_origin
            .as_ref()
            .is_some_and(|expected| expected == origin)
        {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    }

    fn open_directory(&self, relative: &[u8]) -> io::Result<File> {
        if relative.starts_with(b"/") || relative.contains(&0) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        if let Some(guest) = &self.guest_root {
            let mut path = self.guest_prefix.clone();
            path.push(b'/');
            path.extend_from_slice(relative);
            let resolved = guest.open(&path)?;
            self.requires_mount_origin(resolved.origin(), libc::EROFS)?;
            if resolved.canonical_path != self.guest_prefix
                && !resolved
                    .canonical_path
                    .strip_prefix(self.guest_prefix.as_slice())
                    .is_some_and(|suffix| suffix.starts_with(b"/"))
            {
                return Err(io::Error::from_raw_os_error(libc::EROFS));
            }
            let directory = resolved.node.into_file();
            if !directory.metadata()?.is_dir() {
                return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
            }
            return Ok(directory);
        }
        if relative.is_empty() || relative == b"." {
            return self.reopen_directory();
        }
        let parent = self.parent_entry(relative)?;
        // SAFETY: owned parent and terminated leaf; do not follow host links.
        let fd = unsafe {
            libc::openat(
                parent.directory.as_raw_fd(),
                parent.leaf.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a fresh owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn parent_entry(&self, relative: &[u8]) -> io::Result<ParentEntry> {
        if self.guest_root.is_some() {
            if relative.is_empty() || relative.starts_with(b"/") || relative.contains(&0) {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            let split = relative.iter().rposition(|byte| *byte == b'/');
            let (parent, leaf) = match split {
                Some(index) => (&relative[..index], &relative[index + 1..]),
                None => (b"".as_slice(), relative),
            };
            if leaf.is_empty() || leaf == b"." || leaf == b".." {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            // GuestRoot follows links within the virtual namespace. A link
            // into /system must not grant write access to the immutable mount.
            let directory = self.open_directory(parent)?;
            return Ok(ParentEntry {
                directory,
                leaf: CString::new(leaf).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?,
            });
        }
        let mut components = relative.split(|byte| *byte == b'/').peekable();
        let mut directory = self.directory.try_clone()?;
        while let Some(component) = components.next() {
            if component.is_empty() || component == b"." || component == b".." {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            let name =
                CString::new(component).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
            if components.peek().is_none() {
                return Ok(ParentEntry {
                    directory,
                    leaf: name,
                });
            }
            // SAFETY: name is terminated, directory owns a live FD. Never
            // follow a host symlink: Android link traversal belongs to GuestRoot.
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful openat transfers a fresh descriptor.
            directory = unsafe { File::from_raw_fd(fd) };
        }
        Err(io::Error::from_raw_os_error(libc::EINVAL))
    }

    pub(super) fn mkdir(&self, relative: &[u8], mode: u32) -> io::Result<()> {
        let parent = self.parent_entry(relative)?;
        // SAFETY: owned FD and terminated single component remain live.
        let result = unsafe {
            libc::mkdirat(
                parent.directory.as_raw_fd(),
                parent.leaf.as_ptr(),
                (mode & 0o7777) as libc::mode_t,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Create one leaf through an already-resolved guest directory FD. The
    /// retained GuestRoot naming check authenticates that the FD belongs to
    /// this writable mount; the actual operation never reopens a host path.
    pub(super) fn mkdir_at(&self, parent: &File, leaf: &CString, mode: u32) -> io::Result<()> {
        let Some(guest) = &self.guest_root else {
            return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
        };
        let path = guest.directory_path(parent)?;
        let resolved = guest.open(&path)?;
        self.requires_mount_origin(resolved.origin(), libc::EACCES)?;
        let resolved_metadata = resolved.node.metadata();
        if path != self.guest_prefix
            && !path
                .strip_prefix(self.guest_prefix.as_slice())
                .is_some_and(|suffix| suffix.starts_with(b"/"))
        {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
        let parent_metadata = parent.metadata()?;
        if !parent_metadata.is_dir() {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
        if resolved_metadata.dev() != parent_metadata.dev()
            || resolved_metadata.ino() != parent_metadata.ino()
        {
            return Err(io::Error::from_raw_os_error(libc::EACCES));
        }
        // SAFETY: parent is a live retained directory and leaf is NUL
        // terminated by CString; mode is restricted to the Android mode bits.
        let result = unsafe {
            libc::mkdirat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                (mode & 0o7777) as libc::mode_t,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Create missing initialization directories without reopening a host path.
    /// Existing entries must resolve to directories in the writable mount.
    /// This supplies the filesystem mechanism, not package-install policy.
    pub(super) fn seed_directory(&self, relative: &[u8]) -> io::Result<()> {
        if relative.is_empty() {
            return Ok(());
        }
        let mut prefix = Vec::new();
        for component in relative.split(|byte| *byte == b'/') {
            if component.is_empty()
                || component == b"."
                || component == b".."
                || component.contains(&0)
            {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            if !prefix.is_empty() {
                prefix.push(b'/');
            }
            prefix.extend_from_slice(component);
            match self.mkdir(&prefix, 0o777) {
                Ok(()) => (),
                Err(error) if error.raw_os_error() == Some(libc::EEXIST) => (),
                Err(error) => return Err(error),
            }
            // Validate the full prefix, including terminal guest symlinks.
            // A preexisting file or link into a read-only mount is not success.
            let _directory = self.open_directory(&prefix)?;
        }
        Ok(())
    }

    pub(super) fn rename(&self, old: &[u8], new: &[u8]) -> io::Result<()> {
        let old = self.parent_entry(old)?;
        let new = self.parent_entry(new)?;
        // SAFETY: both parents are retained and both leaf names are terminated.
        let result = unsafe {
            libc::renameat(
                old.directory.as_raw_fd(),
                old.leaf.as_ptr(),
                new.directory.as_raw_fd(),
                new.leaf.as_ptr(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn remove(&self, relative: &[u8]) -> io::Result<()> {
        let parent = self.parent_entry(relative)?;
        // unlinkat does not follow the leaf: deleting a link deletes the link,
        // not its target. A directory requires the explicit AT_REMOVEDIR form.
        // SAFETY: parent and terminated leaf remain live for both calls.
        if unsafe { libc::unlinkat(parent.directory.as_raw_fd(), parent.leaf.as_ptr(), 0) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::EISDIR)) {
            return Err(error);
        }
        // SAFETY: same owned parent and leaf, no recursive deletion.
        let result = unsafe {
            libc::unlinkat(
                parent.directory.as_raw_fd(),
                parent.leaf.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temporary_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "darwin-art-private-data-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn open_retains_absolute_path_and_directory_authority() {
        let path = temporary_directory("valid");
        let root = WritableMount::open(path.clone(), b"/data").unwrap();
        assert_eq!(root.path(), path);
        assert!(root.directory().metadata().unwrap().is_dir());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn open_rejects_relative_non_directory_and_final_symlink() {
        assert!(WritableMount::open(PathBuf::from("relative"), b"/data").is_err());
        let file = std::env::temp_dir().join(format!(
            "darwin-art-private-data-file-{}",
            std::process::id()
        ));
        let _ = fs::remove_file(&file);
        fs::write(&file, b"not a directory").unwrap();
        assert!(WritableMount::open(file.clone(), b"/data").is_err());
        fs::remove_file(&file).unwrap();

        let target = temporary_directory("target");
        let link = target.with_file_name(format!(
            "{}-link",
            target.file_name().unwrap().to_string_lossy()
        ));
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(WritableMount::open(link.clone(), b"/data").is_err());
        fs::remove_file(link).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
}
