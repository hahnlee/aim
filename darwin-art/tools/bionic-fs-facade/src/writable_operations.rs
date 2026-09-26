//! Writable-mount dispatch, separate from virtual descriptor/overlay lifetime.
//! Android mount identity chooses an authority; no host path is constructed.
use super::writable_mount::WritableMount;
use super::*;

impl Facade {
    fn writable_request(&self, path: &[u8]) -> Result<(&WritableMount, Vec<u8>), c_int> {
        let cwd = self.namespace.cwd.named_lease().map_err(|_| ANDROID_EIO)?;
        self.writable_request_from(&cwd.path, path)
    }

    fn writable_request_from(
        &self,
        cwd: &[u8],
        path: &[u8],
    ) -> Result<(&WritableMount, Vec<u8>), c_int> {
        let resolution = self.resolve_from(cwd, path)?;
        let root = self
            .writable_root(resolution.mount_id)
            .ok_or(ANDROID_EROFS)?;
        Ok((root, Self::writable_relative_from(root, cwd, path)?))
    }

    pub(super) fn remove_path(&self, path: &[u8]) -> c_int {
        let (root, relative) = match self.writable_request(path) {
            Ok(value) => value,
            Err(error) => return self.fail(error),
        };
        match root.remove(&relative) {
            Ok(()) => 0,
            Err(error) => self.fail_io(&error),
        }
    }

    pub(super) fn rename_path(&self, old_path: &[u8], new_path: &[u8]) -> c_int {
        let cwd = match self.namespace.cwd.named_lease() {
            Ok(cwd) => cwd,
            Err(error) => return self.fail_io(&error),
        };
        let (old_root, old) = match self.writable_request_from(&cwd.path, old_path) {
            Ok(value) => value,
            Err(error) => return self.fail(error),
        };
        let (new_root, new) = match self.writable_request_from(&cwd.path, new_path) {
            Ok(value) => value,
            Err(error) => return self.fail(error),
        };
        if old_root.guest_prefix() != new_root.guest_prefix() {
            return self.fail(18);
        } // Android EXDEV
        match old_root.rename(&old, &new) {
            Ok(()) => 0,
            Err(error) => self.fail_io(&error),
        }
    }

    pub(super) fn truncate_path(&self, path: &[u8], length: i64) -> c_int {
        if length < 0 {
            return self.fail(ANDROID_EINVAL);
        }
        let (root, relative) = match self.writable_request(path) {
            Ok(value) => value,
            Err(error) => return self.fail(error),
        };
        let file = match root.open_file(&relative, O_WRONLY | O_NONBLOCK | O_CLOEXEC, 0) {
            Ok(file) => file,
            Err(error) => return self.fail_io(&error),
        };
        match file.set_len(length as u64) {
            Ok(()) => 0,
            Err(error) => self.fail_io(&error),
        }
    }

    pub(super) fn open_for_path_query(
        &self,
        path: &[u8],
        resolution: &Resolution,
    ) -> Result<File, c_int> {
        if let Some(root) = self.writable_root(resolution.mount_id) {
            let relative = self
                .writable_relative(root, path)
                .map_err(|error| self.fail(error))?;
            root.open_file(&relative, O_RDONLY | O_CLOEXEC, 0)
                .map_err(|error| self.fail_io(&error))
        } else {
            self.namespace
                .broker
                .open(&resolution.relative_path)
                .map(|opened| opened.into_file())
                .map_err(|error| self.fail_broker(&error))
        }
    }

    pub(super) fn writable_root(&self, mount_id: u32) -> Option<&WritableMount> {
        match mount_id {
            2 => self.private_root.as_deref(),
            3 => self.storage_root.as_ref(),
            4 => self.package_root.as_ref(),
            _ => None,
        }
    }

    pub(super) fn writable_relative(
        &self,
        root: &WritableMount,
        path: &[u8],
    ) -> Result<Vec<u8>, c_int> {
        if path.starts_with(b"/") {
            return Self::writable_relative_from(root, b"/", path);
        }
        let cwd = self.namespace.cwd.snapshot().map_err(|_| ANDROID_EIO)?;
        Self::writable_relative_from(root, &cwd, path)
    }

    fn writable_relative_from(
        root: &WritableMount,
        cwd: &[u8],
        path: &[u8],
    ) -> Result<Vec<u8>, c_int> {
        let absolute = if path.starts_with(b"/") {
            path.to_vec()
        } else {
            let mut cwd = cwd.to_vec();
            if !cwd.ends_with(b"/") {
                cwd.push(b'/');
            }
            cwd.extend_from_slice(path);
            cwd
        };
        if absolute == root.guest_prefix() {
            return Ok(Vec::new());
        }
        absolute
            .strip_prefix(root.guest_prefix())
            .and_then(|suffix| suffix.strip_prefix(b"/"))
            .map(Vec::from)
            .ok_or(ANDROID_EACCES)
    }

    pub(super) fn open_writable(
        &self,
        path: &[u8],
        resolution: Resolution,
        flags: c_int,
        mode: u32,
        cwd: &[u8],
    ) -> c_int {
        let Some(root) = self.writable_root(resolution.mount_id) else {
            return self.fail(ANDROID_EROFS);
        };
        let relative = match Self::writable_relative_from(root, cwd, path) {
            Ok(relative) => relative,
            Err(error) => return self.fail(error),
        };
        let file = match root.open_file(&relative, flags, mode) {
            Ok(file) => file,
            Err(error) => return self.fail_io(&error),
        };
        let mut descriptors = match self.descriptors.lock() {
            Ok(table) => table,
            Err(_) => return self.fail_capability(),
        };
        match descriptors.insert_with_flags(Descriptor::PrivateFile(file), flags & O_CLOEXEC != 0) {
            Ok(fd) => fd,
            Err(()) => self.fail(ANDROID_EMFILE),
        }
    }
}
