//! Linux extended attributes on a writable leaf, stored as Darwin extended
//! attributes of the same name. The leaf is opened relative to its retained
//! parent; a final host symlink is never followed by the attribute call.
use super::*;
use std::ffi::CStr;

const LINUX_XATTR_CREATE: i32 = 1;
const LINUX_XATTR_REPLACE: i32 = 2;

impl WritableMount {
    fn open_attribute_leaf(&self, relative: &[u8], no_follow: bool) -> io::Result<File> {
        // O_NONBLOCK: a FIFO leaf must not block an attribute operation.
        let flags =
            crate::O_RDONLY | crate::O_NONBLOCK | if no_follow { crate::O_NOFOLLOW } else { 0 };
        self.open_file(relative, flags, 0)
    }

    /// getxattr: the value length, copied into `value` when it is non-empty.
    pub(crate) fn get_xattr(
        &self,
        relative: &[u8],
        name: &CStr,
        value: &mut [u8],
        no_follow: bool,
    ) -> io::Result<usize> {
        let file = self.open_attribute_leaf(relative, no_follow)?;
        let buffer = if value.is_empty() {
            std::ptr::null_mut()
        } else {
            value.as_mut_ptr().cast()
        };
        // SAFETY: live descriptor, terminated name, buffer of `value.len()`.
        let length =
            unsafe { libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), buffer, value.len(), 0, 0) };
        if length < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(length as usize)
    }

    pub(crate) fn set_xattr(
        &self,
        relative: &[u8],
        name: &CStr,
        value: &[u8],
        flags: i32,
        no_follow: bool,
    ) -> io::Result<()> {
        if flags & !(LINUX_XATTR_CREATE | LINUX_XATTR_REPLACE) != 0
            || flags == LINUX_XATTR_CREATE | LINUX_XATTR_REPLACE
        {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let mut options = 0;
        if flags & LINUX_XATTR_CREATE != 0 {
            options |= libc::XATTR_CREATE;
        }
        if flags & LINUX_XATTR_REPLACE != 0 {
            options |= libc::XATTR_REPLACE;
        }
        let file = self.open_attribute_leaf(relative, no_follow)?;
        // SAFETY: live descriptor, terminated name, readable value slice.
        if unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                options,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn remove_xattr(
        &self,
        relative: &[u8],
        name: &CStr,
        no_follow: bool,
    ) -> io::Result<()> {
        let file = self.open_attribute_leaf(relative, no_follow)?;
        // SAFETY: live descriptor and terminated name.
        if unsafe { libc::fremovexattr(file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Every attribute name of the leaf, as the host stores them.
    pub(crate) fn list_xattr(&self, relative: &[u8], no_follow: bool) -> io::Result<Vec<u8>> {
        let file = self.open_attribute_leaf(relative, no_follow)?;
        loop {
            // SAFETY: a null buffer queries the size.
            let size = unsafe { libc::flistxattr(file.as_raw_fd(), std::ptr::null_mut(), 0, 0) };
            if size < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut names = vec![0u8; size as usize];
            // SAFETY: writable buffer of `names.len()` bytes.
            let listed = unsafe {
                libc::flistxattr(file.as_raw_fd(), names.as_mut_ptr().cast(), names.len(), 0)
            };
            if listed >= 0 {
                names.truncate(listed as usize);
                return Ok(names);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ERANGE) {
                return Err(error);
            }
            // A name was added between the two calls; ask again.
        }
    }
}
