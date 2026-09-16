//! Descriptor-relative open admission. FD lifetime stays in the descriptor table;
//! GuestRoot owns component traversal, symlinks and mount boundaries.
use super::*;

impl Facade {
    pub(super) fn openat_with_mode(
        &self,
        directory_fd: c_int,
        path: &[u8],
        flags: c_int,
        mode: u32,
    ) -> c_int {
        if path.starts_with(b"/")
            || (directory_fd == AT_FDCWD && self.namespace.guest_root.is_none())
        {
            return self.open_with_mode(path, flags, mode);
        }
        // Writable relative opens need mount-aware mutation admission. Preserve
        // the existing pathname implementation for cwd-based writable requests.
        if directory_fd == AT_FDCWD && (flags & O_ACCMODE != O_RDONLY || flags & WRITE_FLAGS != 0) {
            return self.open_with_mode(path, flags, mode);
        }
        let (directory, origin) = if directory_fd == AT_FDCWD {
            match self.namespace.cwd.lease() {
                Ok(lease) => lease,
                Err(error) => return self.fail_io(&error),
            }
        } else {
            let table = match self.descriptors.lock() {
                Ok(table) => table,
                Err(_) => return self.fail_capability(),
            };
            match table.entries.get(&directory_fd) {
                Some(Descriptor::File(file) | Descriptor::PrivateFile(file)) => {
                    match file.try_clone() {
                        Ok(file) => (file, table.origin(directory_fd).cloned()),
                        Err(error) => return self.fail_io(&error),
                    }
                }
                Some(_) => return self.fail(ANDROID_ENOTDIR),
                None => return self.fail(ANDROID_EBADF),
            }
        };
        match directory.metadata() {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return self.fail(ANDROID_ENOTDIR),
            Err(error) => return self.fail_io(&error),
        }
        if flags & !ACCEPTED_FLAGS != 0 {
            return self.fail(ANDROID_EOPNOTSUPP);
        }
        let Some(root) = &self.namespace.guest_root else {
            return self.fail(ANDROID_EOPNOTSUPP);
        };
        let resolved = match origin {
            Some(origin) => root
                .open_relative_with_origin(&directory, &origin, path, flags & O_NOFOLLOW != 0)
                .map(|(file, origin)| (file, Some(origin))),
            None => root
                .open_relative(&directory, path, flags & O_NOFOLLOW != 0)
                .map(|file| (file, None)),
        };
        let (opened, origin) = match resolved {
            Ok(file) => file,
            Err(error) => return self.fail_io(&error),
        };
        let metadata = match opened.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return self.fail_io(&error),
        };
        if metadata.file_type().is_symlink() {
            return self.fail(40);
        } // Android ELOOP
        if flags & O_DIRECTORY != 0 && !metadata.is_dir() {
            return self.fail(ANDROID_ENOTDIR);
        }
        if !metadata.is_file() && !metadata.is_dir() {
            return self.fail(ANDROID_EOPNOTSUPP);
        }
        let mut table = match self.descriptors.lock() {
            Ok(table) => table,
            Err(_) => return self.fail_capability(),
        };
        match table.insert_with_origin(Descriptor::File(opened), flags & O_CLOEXEC != 0, origin) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }
}
