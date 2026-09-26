//! Linux extended-attribute calls (getxattr, setxattr, removexattr,
//! listxattr). Only the `user.` namespace exists here: the host has no
//! security, trusted or system attribute namespaces for guest files. The
//! writable private data mount stores user attributes on the host file; the
//! read-only image carries none, as an erofs image without user attributes.
use super::*;
use std::ffi::CString;

const ANDROID_ENODATA: i32 = 61;
const ANDROID_E2BIG: i32 = 7;
/// Linux XATTR_NAME_MAX / XATTR_SIZE_MAX.
const NAME_MAX: usize = 255;
const SIZE_MAX: usize = 65536;
const USER_NAMESPACE: &[u8] = b"user.";

impl Facade {
    fn attribute_name(name: &[u8]) -> Result<CString, c_int> {
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(ANDROID_ERANGE);
        }
        if !name.starts_with(USER_NAMESPACE) || name.len() == USER_NAMESPACE.len() {
            return Err(ANDROID_EOPNOTSUPP);
        }
        CString::new(name).map_err(|_| ANDROID_EINVAL)
    }

    /// The attribute authority of `path`: its writable mount, or none for an
    /// existing read-only path.
    fn attribute_target(
        &self,
        path: &[u8],
        no_follow: bool,
    ) -> Result<Option<(&writable_mount::WritableMount, Vec<u8>)>, c_int> {
        let resolution = self.resolve(path)?;
        if resolution.mount_id == 2 {
            let root = self.writable_root(2).ok_or(ANDROID_EROFS)?;
            let relative = resolution.relative_path.clone();
            return Ok(Some((root, relative)));
        }
        if self.writable_root(resolution.mount_id).is_some() {
            // Shared storage is Android's FUSE mount, which keeps no user
            // attributes.
            return Err(ANDROID_EOPNOTSUPP);
        }
        let mut status = AndroidStat::default();
        // SAFETY: writable local stat storage.
        if unsafe { self.stat(path, &mut status, no_follow) } != 0 {
            return Err(Self::android_errno());
        }
        Ok(None)
    }

    fn attribute_io_error(&self, error: &std::io::Error) -> c_int {
        // Darwin reports a missing attribute as ENOATTR; Linux as ENODATA.
        if error.raw_os_error() == Some(libc::ENOATTR) {
            return self.fail(ANDROID_ENODATA);
        }
        self.fail_io(error)
    }

    pub(super) fn get_xattr(
        &self,
        path: &[u8],
        name: &[u8],
        value: &mut [u8],
        no_follow: bool,
    ) -> isize {
        let name = match Self::attribute_name(name) {
            Ok(name) => name,
            Err(error) => return self.fail(error) as isize,
        };
        match self.attribute_target(path, no_follow) {
            Err(error) => self.fail(error) as isize,
            Ok(None) => self.fail(ANDROID_ENODATA) as isize,
            Ok(Some((root, relative))) => {
                match root.get_xattr(&relative, &name, value, no_follow) {
                    Ok(length) => length as isize,
                    Err(error) => self.attribute_io_error(&error) as isize,
                }
            }
        }
    }

    pub(super) fn set_xattr(
        &self,
        path: &[u8],
        name: &[u8],
        value: &[u8],
        flags: c_int,
        no_follow: bool,
    ) -> c_int {
        let name = match Self::attribute_name(name) {
            Ok(name) => name,
            Err(error) => return self.fail(error),
        };
        if value.len() > SIZE_MAX {
            return self.fail(ANDROID_E2BIG);
        }
        match self.attribute_target(path, no_follow) {
            Err(error) => self.fail(error),
            Ok(None) => self.fail(ANDROID_EROFS),
            Ok(Some((root, relative))) => {
                match root.set_xattr(&relative, &name, value, flags, no_follow) {
                    Ok(()) => 0,
                    Err(error) => self.attribute_io_error(&error),
                }
            }
        }
    }

    pub(super) fn remove_xattr(&self, path: &[u8], name: &[u8], no_follow: bool) -> c_int {
        let name = match Self::attribute_name(name) {
            Ok(name) => name,
            Err(error) => return self.fail(error),
        };
        match self.attribute_target(path, no_follow) {
            Err(error) => self.fail(error),
            Ok(None) => self.fail(ANDROID_EROFS),
            Ok(Some((root, relative))) => match root.remove_xattr(&relative, &name, no_follow) {
                Ok(()) => 0,
                Err(error) => self.attribute_io_error(&error),
            },
        }
    }

    /// listxattr: the NUL-terminated `user.` names; host metadata
    /// attributes (com.apple.*) are not guest attributes.
    pub(super) fn list_xattr(&self, path: &[u8], output: &mut [u8], no_follow: bool) -> isize {
        let names = match self.attribute_target(path, no_follow) {
            Err(error) => return self.fail(error) as isize,
            Ok(None) => Vec::new(),
            Ok(Some((root, relative))) => match root.list_xattr(&relative, no_follow) {
                Ok(names) => names,
                Err(error) => return self.attribute_io_error(&error) as isize,
            },
        };
        let mut guest = Vec::new();
        for name in names
            .split(|byte| *byte == 0)
            .filter(|name| name.starts_with(USER_NAMESPACE))
        {
            guest.extend_from_slice(name);
            guest.push(0);
        }
        if output.is_empty() {
            return guest.len() as isize;
        }
        if output.len() < guest.len() {
            return self.fail(ANDROID_ERANGE) as isize;
        }
        output[..guest.len()].copy_from_slice(&guest);
        guest.len() as isize
    }
}
