//! Android pathname metadata queries; descriptor ownership remains in the broker.
use super::*;

impl Facade {
    pub(super) unsafe fn fstatat(
        &self,
        directory_fd: c_int,
        path: &[u8],
        status: *mut AndroidStat,
        flags: c_int,
    ) -> c_int {
        const AT_EMPTY_PATH: c_int = 0x1000;
        const AT_NO_AUTOMOUNT: c_int = 0x800;
        if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_NO_AUTOMOUNT) != 0 {
            return self.fail(ANDROID_EINVAL);
        }
        if status.is_null() {
            return self.fail(ANDROID_EFAULT);
        }
        if path.is_empty() {
            if flags & AT_EMPTY_PATH == 0 {
                return self.fail(ANDROID_ENOENT);
            }
            return if directory_fd == AT_FDCWD {
                match self.namespace.cwd.metadata() {
                    Ok(metadata) => {
                        unsafe { status.write(metadata_to_android(&metadata)) };
                        0
                    }
                    Err(error) => self.fail_io(&error),
                }
            } else {
                unsafe { self.fstat(directory_fd, status) }
            };
        }
        if path.starts_with(b"/") {
            return unsafe { self.stat(path, status, flags & AT_SYMLINK_NOFOLLOW != 0) };
        }
        if directory_fd == AT_FDCWD {
            if self.namespace.guest_root.is_none() {
                return unsafe { self.stat(path, status, flags & AT_SYMLINK_NOFOLLOW != 0) };
            }
            let directory = match self.namespace.cwd.directory() {
                Ok(file) => file,
                Err(error) => return self.fail_io(&error),
            };
            return unsafe { self.stat_relative(&directory, path, status, flags) };
        }
        let descriptors = match self.descriptors.lock() {
            Ok(table) => table,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.entries.get(&directory_fd) {
            None => self.fail(ANDROID_EBADF),
            Some(Descriptor::File(file) | Descriptor::PrivateFile(file)) => match file.metadata() {
                Ok(metadata) if metadata.is_dir() => unsafe {
                    self.stat_relative(file, path, status, flags)
                },
                Ok(_) => self.fail(ANDROID_ENOTDIR),
                Err(error) => self.fail_io(&error),
            },
            Some(_) => self.fail(ANDROID_ENOTDIR),
        }
    }

    unsafe fn stat_relative(
        &self,
        directory: &File,
        path: &[u8],
        status: *mut AndroidStat,
        flags: c_int,
    ) -> c_int {
        let Some(root) = &self.namespace.guest_root else {
            return self.fail(ANDROID_EOPNOTSUPP);
        };
        let file = match root.open_relative(directory, path, flags & AT_SYMLINK_NOFOLLOW != 0) {
            Ok(file) => file,
            Err(error) => return self.fail_io(&error),
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return self.fail_io(&error),
        };
        unsafe { status.write(metadata_to_android(&metadata)) };
        0
    }

    /// Stat a node in the writable data capability without reopening its host
    /// pathname. When the full guest-root resolver is available, this also
    /// gives absolute and relative links Android's virtual `/data` semantics;
    /// the descriptor-only fallback deliberately rejects links rather than
    /// allowing a host-path escape.
    fn stat_private_relative(&self, relative: &[u8], no_follow: bool) -> Result<Metadata, c_int> {
        let root = self
            .private_root
            .as_deref()
            .ok_or_else(|| self.fail(ANDROID_EIO))?;
        self.stat_writable_relative(root, relative, no_follow)
    }

    fn stat_writable_relative(
        &self,
        authority: &super::writable_mount::WritableMount,
        relative: &[u8],
        no_follow: bool,
    ) -> Result<Metadata, c_int> {
        if let Some(root) = &self.namespace.guest_root {
            let mount = root
                .open(authority.guest_prefix())
                .map_err(|error| self.fail_io(&error))?
                .node
                .into_file();
            let file = if relative.is_empty() {
                mount
            } else {
                root.open_relative(&mount, relative, no_follow)
                    .map_err(|error| self.fail_io(&error))?
            };
            return Ok(file
                .metadata()
                .map_err(|error| self.fail_io(&error))?
                .clone());
        }

        let file = if relative.is_empty() {
            authority
                .directory()
                .try_clone()
                .map_err(|error| self.fail_io(&error))?
        } else {
            // The namespace-less writable authority walks every component
            // with O_NOFOLLOW, including the final one. This fallback is
            // safe but intentionally reports an explicit failure for a
            // symlink when the virtual guest-root resolver is unavailable.
            authority
                .open_file(relative, O_RDONLY | O_NONBLOCK | O_CLOEXEC, 0)
                .map_err(|error| self.fail_io(&error))?
        };
        let metadata = file.metadata().map_err(|error| self.fail_io(&error))?;
        if !no_follow && metadata.file_type().is_symlink() {
            return Err(self.fail(ANDROID_EOPNOTSUPP));
        }
        Ok(metadata)
    }

    pub(super) unsafe fn stat(
        &self,
        path: &[u8],
        status: *mut AndroidStat,
        no_follow: bool,
    ) -> c_int {
        if status.is_null() {
            return self.fail(ANDROID_EFAULT);
        }
        if let Some(kind) = Self::random_device(path) {
            // SAFETY: the Android ABI requires one writable 128-byte stat object.
            unsafe { status.write(random_device_stat(kind)) };
            return 0;
        }
        if let Some(data) = Self::synthetic_proc_contents(path) {
            let file = OverlayFile {
                inode: Self::synthetic_inode(path),
                mode: ANDROID_S_IFREG | 0o444,
                data,
            };
            // SAFETY: the Android ABI requires one writable 128-byte stat
            // object, checked for null above.
            unsafe { status.write(overlay_file_stat(&file)) };
            return 0;
        }
        if let Some(apk) = self.authorized_host_apk_path(path) {
            let metadata = match if no_follow {
                fs::symlink_metadata(apk)
            } else {
                fs::metadata(apk)
            } {
                Ok(metadata) => metadata,
                Err(error) => return self.fail_io(&error),
            };
            unsafe { status.write(metadata_to_android(&metadata)) };
            return 0;
        }
        if let Some(native_path) = self.authorized_host_native_path(path) {
            let metadata = match if no_follow {
                fs::symlink_metadata(native_path)
            } else {
                fs::metadata(native_path)
            } {
                Ok(metadata) => metadata,
                Err(error) => return self.fail_io(&error),
            };
            unsafe { status.write(metadata_to_android(&metadata)) };
            return 0;
        }
        if let Some(private_path) = self.authorized_host_private_candidate(path) {
            let relative = match self.private_relative_from_host_path(&private_path) {
                Some(relative) => relative,
                None => return self.fail(ANDROID_EACCES),
            };
            let metadata = match self.stat_private_relative(&relative, no_follow) {
                Ok(metadata) => metadata,
                Err(error) => return error,
            };
            unsafe { status.write(metadata_to_android(&metadata)) };
            return 0;
        }
        // Preserve ENOENT for a missing final component below the writable
        // private root.  Falling through to guest-path resolution would treat
        // this absolute host pathname as an immutable mount and can turn the
        // ordinary missing-file result into ENOTDIR.
        let resolution = match self.resolve(path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        if let Some(root) = self.writable_root(resolution.mount_id) {
            let relative = match self.writable_relative(root, path) {
                Ok(relative) => relative,
                Err(error) => return self.fail(error),
            };
            let metadata = match self.stat_writable_relative(root, &relative, no_follow) {
                Ok(metadata) => metadata,
                Err(error) => return error,
            };
            unsafe { status.write(metadata_to_android(&metadata)) };
            return 0;
        }
        if resolution.mount_id == 2 {
            let overlay = match self.overlay.lock() {
                Ok(overlay) => overlay,
                Err(_) => return self.fail_capability(),
            };
            let translated = match overlay.entries.get(&resolution.relative_path) {
                Some(OverlayEntry::File(node)) => {
                    let file = match node.lock() {
                        Ok(file) => file,
                        Err(_) => return self.fail_capability(),
                    };
                    overlay_file_stat(&file)
                }
                Some(OverlayEntry::Directory(directory)) => overlay_directory_stat(*directory),
                None => return self.fail(ANDROID_ENOENT),
            };
            unsafe { status.write(translated) };
            return 0;
        }
        if let Some(root) = &self.namespace.guest_root {
            let absolute = if path.starts_with(b"/") {
                path.to_vec()
            } else {
                let Ok(cwd) = self.namespace.cwd.snapshot() else {
                    return self.fail_capability();
                };
                let mut absolute = cwd.clone();
                if !absolute.ends_with(b"/") {
                    absolute.push(b'/');
                }
                absolute.extend_from_slice(path);
                absolute
            };
            // Resolve links before '..', anchored to the guest root and
            // mounted descriptors. Stat the opened node, never reopen a
            // resolved pathname through the host filesystem.
            let opened = match if no_follow {
                root.open_no_follow(&absolute)
            } else {
                root.open(&absolute)
            } {
                Ok(opened) => opened,
                Err(error) => return self.fail_io(&error),
            };
            unsafe { status.write(metadata_to_android(opened.node.metadata())) };
            return 0;
        }
        let mut relative_path = resolution.relative_path;
        if resolution.requires_directory && !relative_path.is_empty() {
            relative_path.push(b'/');
        }
        let metadata = match self.namespace.broker.stat(&relative_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if no_follow && is_final_symlink_rejection(&error, &relative_path) {
                    return self.fail(ANDROID_EOPNOTSUPP);
                }
                return self.fail_broker(&error);
            }
        };
        let translated = metadata_to_android(&metadata);
        // SAFETY: the guest ABI requires a writable 128-byte Android stat object.
        unsafe { status.write(translated) };
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    #[test]
    fn private_metadata_keeps_virtual_mount_symlinks_inside_guest_root() {
        let nonce = format!("{}-{:?}", std::process::id(), std::thread::current().id());
        let guest_path = std::env::temp_dir().join(format!("darwin-stat-guest-{nonce}"));
        let private_path = std::env::temp_dir().join(format!("darwin-stat-private-{nonce}"));
        let outside_path = std::env::temp_dir().join(format!("darwin-stat-outside-{nonce}"));
        fs::create_dir(&guest_path).unwrap();
        fs::create_dir(&private_path).unwrap();
        fs::create_dir(&outside_path).unwrap();
        fs::write(outside_path.join("secret"), b"host-only").unwrap();
        symlink(outside_path.join("secret"), private_path.join("escape")).unwrap();
        symlink(&outside_path, private_path.join("directory-escape")).unwrap();

        let mut facade = Facade::new(File::open(&guest_path).unwrap(), b"/", b"/").unwrap();
        let mut guest = darwin_art_fs_broker::guest_path::GuestRoot::from_directory(
            File::open(&guest_path).unwrap(),
        )
        .unwrap();
        guest
            .mount_directory(b"/data", File::open(&private_path).unwrap())
            .unwrap();
        facade.namespace.guest_root = Some(std::sync::Arc::new(guest));
        facade.private_root =
            Some(super::private_data::PrivateDataRoot::open(private_path.clone()).unwrap());

        let mut status = std::mem::MaybeUninit::<AndroidStat>::uninit();
        assert_eq!(
            unsafe { facade.stat(b"/data/escape", status.as_mut_ptr(), false) },
            -1
        );
        assert_eq!(
            unsafe { facade.stat(b"/data/directory-escape/secret", status.as_mut_ptr(), false) },
            -1
        );
        assert_eq!(
            unsafe { facade.stat(b"/data/escape", status.as_mut_ptr(), true) },
            0
        );
        assert_eq!(unsafe { status.assume_init() }.st_mode & 0o170000, 0o120000);

        let host_path = private_path.join("escape");
        assert_eq!(
            unsafe { facade.stat(host_path.as_os_str().as_bytes(), status.as_mut_ptr(), false) },
            -1
        );

        fs::remove_dir_all(&guest_path).unwrap();
        fs::remove_dir_all(&private_path).unwrap();
        fs::remove_dir_all(&outside_path).unwrap();
    }

    #[test]
    fn fstatat_selects_path_or_descriptor_without_host_fd_fallback() {
        let facade = Facade::new(File::open("/").unwrap(), b"/system", b"/system").unwrap();
        let mut status = std::mem::MaybeUninit::<AndroidStat>::uninit();
        assert_eq!(
            unsafe { facade.fstatat(-77, b"/system", status.as_mut_ptr(), 0) },
            0
        );
        assert_eq!(
            unsafe { status.assume_init() }.st_mode & 0o170000,
            ANDROID_S_IFDIR
        );
        assert_eq!(
            unsafe { facade.fstatat(AT_FDCWD, b".", status.as_mut_ptr(), 0x800) },
            0
        );
        let fd = facade.open(b"/system", 0);
        assert!(fd >= 0);
        assert_eq!(
            unsafe { facade.fstatat(fd, b"", status.as_mut_ptr(), 0x1000) },
            0
        );
        assert_eq!(
            unsafe { facade.fstatat(fd, b"", status.as_mut_ptr(), 0) },
            -1
        );
        assert_eq!(
            unsafe { facade.fstatat(fd, b"", status.as_mut_ptr(), 0x2000) },
            -1
        );
        assert_eq!(facade.close(fd), 0);
        assert_eq!(
            unsafe { facade.fstatat(fd, b"", status.as_mut_ptr(), 0x1000) },
            -1
        );
        // A real host fd that was never admitted is not an Android capability.
        assert_eq!(
            unsafe { facade.fstatat(0, b"", status.as_mut_ptr(), 0x1000) },
            -1
        );
    }

    #[test]
    fn stat_follows_guest_links_before_parent_components() {
        let root = std::env::temp_dir().join(format!(
            "darwin-stat-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/target"), b"data").unwrap();
        symlink("/a/b", root.join("link")).unwrap();
        symlink("/etc/passwd", root.join("outside")).unwrap();
        let facade = Facade::new(File::open(&root).unwrap(), b"/", b"/").unwrap();
        let mut status = std::mem::MaybeUninit::<AndroidStat>::uninit();
        assert_eq!(
            unsafe { facade.stat(b"/link", status.as_mut_ptr(), false) },
            0
        );
        assert_eq!(
            unsafe { status.assume_init() }.st_mode & 0o170000,
            ANDROID_S_IFDIR
        );
        assert_eq!(
            unsafe { facade.stat(b"link/../target", status.as_mut_ptr(), false) },
            0
        );
        assert_eq!(unsafe { status.assume_init() }.st_size, 4);
        assert_eq!(
            unsafe { facade.stat(b"/outside", status.as_mut_ptr(), false) },
            -1
        );
        assert_eq!(
            unsafe { facade.stat(b"/outside", status.as_mut_ptr(), true) },
            0
        );
        assert_eq!(unsafe { status.assume_init() }.st_mode & 0o170000, 0o120000);
        assert_eq!(
            unsafe { facade.stat(b"/link/", status.as_mut_ptr(), true) },
            0
        );
        assert_eq!(
            unsafe { status.assume_init() }.st_mode & 0o170000,
            ANDROID_S_IFDIR
        );
        assert_eq!(
            unsafe { facade.stat(b"/absent", status.as_mut_ptr(), false) },
            -1
        );
        fs::remove_dir_all(root).unwrap();
    }
}
