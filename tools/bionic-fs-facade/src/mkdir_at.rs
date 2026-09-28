//! Descriptor-relative directory creation.
//!
//! Parent traversal is performed by the retained GuestRoot directory
//! authority. The final mkdirat uses that same opened parent descriptor, so a
//! rename after lookup cannot redirect the operation through a host path.
use super::*;
use std::ffi::CString;

impl Facade {
    pub(super) fn mkdir_at(&self, directory_fd: c_int, path: &[u8], mode: u32) -> c_int {
        // Linux ignores directory_fd for absolute paths, including an invalid
        // descriptor. A facade without GuestRoot retains the old pathname
        // policy for AT_FDCWD; a retained guest cwd follows the same FD path
        // as an explicit directory descriptor.
        if path.starts_with(b"/") {
            return self.mkdir(path, mode);
        }
        if directory_fd == AT_FDCWD {
            if self.namespace.guest_root.is_none() {
                return self.mkdir(path, mode);
            }
            let directory = match self.namespace.cwd.directory() {
                Ok(directory) => directory,
                Err(error) => return self.fail_io(&error),
            };
            return self.mkdir_at_directory(directory, path, mode);
        }
        if path.is_empty() {
            return self.fail(ANDROID_ENOENT);
        }

        let (directory, read_only_root) = {
            let descriptors = match self.descriptors.lock() {
                Ok(descriptors) => descriptors,
                Err(_) => return self.fail_capability(),
            };
            match descriptors.entries.get(&directory_fd) {
                Some(Descriptor::File(file) | Descriptor::PrivateFile(file)) => {
                    match file.try_clone() {
                        Ok(file) => (
                            file,
                            matches!(
                                descriptors.entries.get(&directory_fd),
                                Some(Descriptor::File(_))
                            ) && self.namespace.guest_root.is_none(),
                        ),
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

        // The legacy root-only facade admits File descriptors through its
        // read-only broker. This is a known mount restriction, not missing
        // path support. PrivateFile still requires guest parent resolution.
        if read_only_root {
            return self.fail(ANDROID_EROFS);
        }
        self.mkdir_at_directory(directory, path, mode)
    }

    fn mkdir_at_directory(&self, directory: File, path: &[u8], mode: u32) -> c_int {
        let mut trimmed = path;
        while trimmed.last() == Some(&b'/') {
            trimmed = &trimmed[..trimmed.len() - 1];
        }
        if trimmed.is_empty() {
            return self.fail(ANDROID_ENOENT);
        }
        let (parent_path, leaf) = match trimmed.iter().rposition(|byte| *byte == b'/') {
            Some(index) => {
                let parent = if index == 0 {
                    b"/".as_slice()
                } else {
                    &trimmed[..index]
                };
                (parent, &trimmed[index + 1..])
            }
            None => (b".".as_slice(), trimmed),
        };
        if leaf.is_empty() || leaf.contains(&0) {
            return self.fail(ANDROID_EINVAL);
        }
        let Some(guest) = self.namespace.guest_root.as_ref() else {
            return self.fail(ANDROID_EOPNOTSUPP);
        };
        let parent = match guest.open_relative(&directory, parent_path, false) {
            Ok(parent) => parent,
            Err(error) => return self.fail_io(&error),
        };
        match parent.metadata() {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return self.fail(ANDROID_ENOTDIR),
            Err(error) => return self.fail_io(&error),
        }
        let parent_guest_path = match guest.directory_path(&parent) {
            Ok(path) => path,
            Err(error) => return self.fail_io(&error),
        };
        let resolution = match self.resolve_from(b"/", &parent_guest_path) {
            Ok(resolution) => resolution,
            Err(error) => return self.fail(error),
        };
        let Some(root) = self.writable_root(resolution.mount_id) else {
            return self.fail(ANDROID_EROFS);
        };
        let leaf = match CString::new(leaf) {
            Ok(leaf) => leaf,
            Err(_) => return self.fail(ANDROID_EINVAL),
        };
        match root.mkdir_at(&parent, &leaf, mode) {
            Ok(()) => 0,
            Err(error) => self.fail_io(&error),
        }
    }
}
