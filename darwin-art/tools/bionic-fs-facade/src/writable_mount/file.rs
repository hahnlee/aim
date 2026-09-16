//! Android open flags and writable leaf admission at the Darwin openat edge.
//! Parent traversal stays with WritableMount/GuestRoot, and the final host
//! open never follows a symlink after virtual resolution.
use super::*;
use crate::{
    O_ACCMODE, O_APPEND, O_CLOEXEC, O_CREAT, O_DIRECTORY, O_DSYNC, O_EXCL, O_LARGEFILE, O_NOFOLLOW,
    O_NONBLOCK, O_RDONLY, O_RDWR, O_SYNC, O_TMPFILE, O_TRUNC, O_WRONLY,
};

fn host_flags(flags: i32) -> io::Result<i32> {
    let accepted = O_ACCMODE
        | O_APPEND
        | O_CLOEXEC
        | O_CREAT
        | O_DIRECTORY
        | O_DSYNC
        | O_EXCL
        | O_LARGEFILE
        | O_NOFOLLOW
        | O_NONBLOCK
        | O_SYNC
        | O_TRUNC;
    if flags & !accepted != 0 || flags & O_TMPFILE == O_TMPFILE {
        return Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP));
    }
    let mut host = match flags & O_ACCMODE {
        O_RDONLY => libc::O_RDONLY,
        O_WRONLY => libc::O_WRONLY,
        O_RDWR => libc::O_RDWR,
        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    for (android, darwin) in [
        (O_APPEND, libc::O_APPEND),
        (O_CREAT, libc::O_CREAT),
        (O_DIRECTORY, libc::O_DIRECTORY),
        (O_EXCL, libc::O_EXCL),
        (O_NONBLOCK, libc::O_NONBLOCK),
        (O_TRUNC, libc::O_TRUNC),
    ] {
        if flags & android != 0 {
            host |= darwin;
        }
    }
    // O_SYNC includes O_DSYNC. Testing any shared bit would wrongly turn a
    // data-only sync request into the stronger full-sync operation.
    if flags & O_SYNC == O_SYNC {
        host |= libc::O_SYNC;
    } else if flags & O_DSYNC != 0 {
        host |= libc::O_DSYNC;
    }
    Ok(host | libc::O_NOFOLLOW | libc::O_CLOEXEC)
}

impl WritableMount {
    pub(crate) fn open_file(&self, relative: &[u8], flags: i32, mode: u32) -> io::Result<File> {
        let host = host_flags(flags)?;
        self.with_leaf(relative, flags, |parent, directory| {
            let host = host | if directory { libc::O_DIRECTORY } else { 0 };
            // SAFETY: retained parent, terminated leaf; the host cannot follow links.
            let fd = unsafe {
                libc::openat(
                    parent.directory.as_raw_fd(),
                    parent.leaf.as_ptr(),
                    host,
                    (mode & 0o7777) as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful openat transfers a fresh descriptor.
            Ok(unsafe { File::from_raw_fd(fd) })
        })
    }

    /// chmod requires ownership, not read permission. Darwin O_EVTONLY still
    /// checks read permission, so operate relative to the retained parent and
    /// forbid final host symlink traversal even if the leaf changes concurrently.
    pub(crate) fn chmod(&self, relative: &[u8], mode: u32) -> io::Result<()> {
        self.with_leaf(relative, 0, |parent, directory| {
            let mut status: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: live parent, terminated leaf and writable stat storage.
            if unsafe {
                libc::fstatat(
                    parent.directory.as_raw_fd(),
                    parent.leaf.as_ptr(),
                    &mut status,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            if status.st_mode & libc::S_IFMT == libc::S_IFLNK {
                return Err(io::Error::from_raw_os_error(libc::ELOOP));
            }
            if directory && status.st_mode & libc::S_IFMT != libc::S_IFDIR {
                return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
            }
            // SAFETY: same retained authority; NOFOLLOW also protects the gap
            // after fstatat from following a replacement link out of the mount.
            if unsafe {
                libc::fchmodat(
                    parent.directory.as_raw_fd(),
                    parent.leaf.as_ptr(),
                    (mode & 0o7777) as libc::mode_t,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    }

    fn with_leaf<T>(
        &self,
        relative: &[u8],
        flags: i32,
        mut operation: impl FnMut(&ParentEntry, bool) -> io::Result<T>,
    ) -> io::Result<T> {
        if relative.starts_with(b"/") || relative.contains(&0) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        if relative.len() >= 4096 {
            return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
        }
        let mut follow_final = flags & O_NOFOLLOW == 0;
        let mut directory = flags & O_DIRECTORY != 0;
        let mut path = relative.to_vec();
        for links in 0..=40 {
            if path.ends_with(b"/") {
                directory = true;
                follow_final = true; // Slash makes this a directory traversal.
            }
            while path.ends_with(b"/") {
                path.pop();
            }
            let parent = if path.is_empty() {
                ParentEntry {
                    directory: self.directory.try_clone()?,
                    leaf: CString::new(".").unwrap(),
                }
            } else if path
                .rsplit(|byte| *byte == b'/')
                .next()
                .is_some_and(|leaf| leaf == b"." || leaf == b"..")
            {
                ParentEntry {
                    directory: self.open_directory(&path)?,
                    leaf: CString::new(".").unwrap(),
                }
            } else {
                self.parent_entry(&path)?
            };
            // A namespace-less capability deliberately keeps its no-link
            // contract. CREATE|EXCL checks the original leaf, including links.
            if self.guest_root.is_some()
                && follow_final
                && flags & (O_CREAT | O_EXCL) != (O_CREAT | O_EXCL)
            {
                let mut target = [0u8; 4096];
                // SAFETY: parent/leaf and writable buffer live for this call.
                let count = unsafe {
                    libc::readlinkat(
                        parent.directory.as_raw_fd(),
                        parent.leaf.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if count >= 0 {
                    if links == 40 {
                        return Err(io::Error::from_raw_os_error(libc::ELOOP));
                    }
                    if count as usize == target.len() {
                        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
                    }
                    let target = &target[..count as usize];
                    if target.starts_with(b"/") {
                        path = if target == self.guest_prefix {
                            Vec::new()
                        } else {
                            target
                                .strip_prefix(self.guest_prefix.as_slice())
                                .and_then(|suffix| suffix.strip_prefix(b"/"))
                                .ok_or_else(|| io::Error::from_raw_os_error(libc::EROFS))?
                                .to_vec()
                        };
                    } else {
                        let prefix = path
                            .iter()
                            .rposition(|byte| *byte == b'/')
                            .map_or(0, |index| index + 1);
                        path.truncate(prefix);
                        path.extend_from_slice(target);
                    }
                    if path.len() >= 4096 {
                        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
                    }
                    continue;
                }
                let error = io::Error::last_os_error();
                if !matches!(
                    error.raw_os_error(),
                    Some(libc::EINVAL) | Some(libc::ENOENT)
                ) {
                    return Err(error);
                }
            }
            if let Some(guest) = &self.guest_root {
                // A virtual mount can occupy the final component too. Consult
                // the mount table using the retained parent, not an extra
                // read-open of the leaf (which would break write-only files
                // and CREATE|EXCL symlink error semantics).
                if let Some(origin) =
                    guest.mounted_origin_at(&parent.directory, parent.leaf.as_bytes())?
                {
                    self.requires_mount_origin(&origin, libc::EROFS)?;
                }
            }
            return operation(&parent, directory);
        }
        unreachable!("link limit returns an error")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sync_flags_are_not_conflated_and_invalid_access_is_rejected() {
        assert_eq!(host_flags(O_DSYNC).unwrap() & libc::O_SYNC, 0);
        assert_ne!(host_flags(O_DSYNC).unwrap() & libc::O_DSYNC, 0);
        assert_ne!(host_flags(O_SYNC).unwrap() & libc::O_SYNC, 0);
        assert!(host_flags(O_ACCMODE).is_err());
        assert!(host_flags(O_TMPFILE).is_err());
    }
}
