//! Copies between the guest's volumes that keep what the guest sees of each
//! file: its content, host mode, extended attributes (the guest's owner,
//! mode and SELinux label: docs/guest-init-contract.md section 6) and
//! access and modification times.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Copies each of `paths` (relative, files or directories with everything
/// under them) from the tree at `from` into the tree at `to`, creating
/// their parent directories as `from` has them. Returns the files copied,
/// relative. Nothing of `to` is replaced.
pub fn copy_paths(from: &Path, to: &Path, paths: &[&Path]) -> Result<Vec<PathBuf>, String> {
    let mut copier = Copier {
        from,
        to,
        files: Vec::new(),
        dirs: Vec::new(),
    };
    for path in paths {
        let mut ancestors: Vec<&Path> = path
            .ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
            .collect();
        ancestors.reverse();
        for dir in ancestors {
            if !to.join(dir).exists() {
                copier.dir(dir)?;
            }
        }
        copier.tree(path)?;
    }
    // A directory's times last, once nothing more is created in it.
    for dir in copier.dirs.iter().rev() {
        times(&from.join(dir), &to.join(dir))?;
    }
    Ok(copier.files)
}

struct Copier<'a> {
    from: &'a Path,
    to: &'a Path,
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

impl Copier<'_> {
    fn tree(&mut self, path: &Path) -> Result<(), String> {
        let source = self.from.join(path);
        let meta =
            fs::symlink_metadata(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        if meta.is_dir() {
            self.dir(path)?;
            let mut names: Vec<_> = fs::read_dir(&source)
                .map_err(|e| format!("{}: {e}", source.display()))?
                .map(|e| e.map(|e| e.file_name()))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("{}: {e}", source.display()))?;
            names.sort();
            for name in names {
                self.tree(&path.join(name))?;
            }
            Ok(())
        } else if meta.is_file() {
            let dest = self.to.join(path);
            fs::copy(&source, &dest).map_err(|e| format!("{}: {e}", source.display()))?;
            attributes(&source, &dest, meta.permissions().mode())?;
            times(&source, &dest)?;
            self.files.push(path.to_path_buf());
            Ok(())
        } else {
            Err(format!(
                "{}: neither a file nor a directory",
                source.display()
            ))
        }
    }

    fn dir(&mut self, path: &Path) -> Result<(), String> {
        let (source, dest) = (self.from.join(path), self.to.join(path));
        let mode = fs::metadata(&source)
            .map_err(|e| format!("{}: {e}", source.display()))?
            .permissions()
            .mode();
        fs::create_dir(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        attributes(&source, &dest, mode)?;
        self.dirs.push(path.to_path_buf());
        Ok(())
    }
}

fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())
}

/// Gives `dest` the ownership and extended attributes of `source`, then `mode`. Setting
/// an attribute needs write access, which the final mode may deny.
fn attributes(source: &Path, dest: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let source_meta = fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let dest_meta = fs::symlink_metadata(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    if (source_meta.uid(), source_meta.gid()) != (dest_meta.uid(), dest_meta.gid()) {
        let to = c_path(dest)?;
        // Real filesystem ownership is independent of the guest-inode xattr.
        // chown may clear set-ID bits, so restore the final mode afterwards.
        if unsafe { libc::fchownat(libc::AT_FDCWD, to.as_ptr(), source_meta.uid(), source_meta.gid(), libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            return Err(format!("{}: chown: {}", dest.display(), std::io::Error::last_os_error()));
        }
    }
    let set_mode = |mode: u32| {
        fs::set_permissions(dest, fs::Permissions::from_mode(mode & 0o7777))
            .map_err(|e| format!("{}: {e}", dest.display()))
    };
    set_mode(mode | 0o200)?;
    let (from, to) = (c_path(source)?, c_path(dest)?);
    // SAFETY: NUL-terminated paths and buffers of the lengths passed, in
    // the list/get/set pattern of listxattr(2) and getxattr(2).
    unsafe {
        let len = libc::listxattr(from.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW);
        if len < 0 {
            return Err(format!(
                "{}: listxattr: {}",
                source.display(),
                std::io::Error::last_os_error()
            ));
        }
        let mut names = vec![0u8; len as usize];
        let len = libc::listxattr(
            from.as_ptr(),
            names.as_mut_ptr().cast(),
            names.len(),
            libc::XATTR_NOFOLLOW,
        );
        if len < 0 {
            return Err(format!(
                "{}: listxattr: {}",
                source.display(),
                std::io::Error::last_os_error()
            ));
        }
        for name in names[..len as usize]
            .split(|&b| b == 0)
            .filter(|n| !n.is_empty())
        {
            let name = CString::new(name).map_err(|e| e.to_string())?;
            let size = libc::getxattr(
                from.as_ptr(),
                name.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                libc::XATTR_NOFOLLOW,
            );
            if size < 0 {
                return Err(format!(
                    "{}: getxattr: {}",
                    source.display(),
                    std::io::Error::last_os_error()
                ));
            }
            let mut value = vec![0u8; size as usize];
            let size = libc::getxattr(
                from.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            );
            if size < 0
                || libc::setxattr(
                    to.as_ptr(),
                    name.as_ptr(),
                    value.as_ptr().cast(),
                    size as usize,
                    0,
                    libc::XATTR_NOFOLLOW,
                ) != 0
            {
                return Err(format!(
                    "{}: {}: {}",
                    source.display(),
                    name.to_string_lossy(),
                    std::io::Error::last_os_error()
                ));
            }
        }
    }
    set_mode(mode)
}

/// Gives `dest` the access and modification times of `source`.
fn times(source: &Path, dest: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let spec = |sec: i64, nsec: i64| libc::timespec {
        tv_sec: sec,
        tv_nsec: nsec,
    };
    let stamps = [
        spec(meta.atime(), meta.atime_nsec()),
        spec(meta.mtime(), meta.mtime_nsec()),
    ];
    let to = c_path(dest)?;
    // SAFETY: a NUL-terminated path and two timespecs.
    if unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            to.as_ptr(),
            stamps.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(format!(
            "{}: utimensat: {}",
            dest.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
        let (p, n) = (c_path(path).unwrap(), CString::new(name).unwrap());
        let mut value = vec![0u8; 64];
        // SAFETY: NUL-terminated strings and a buffer of the length passed.
        let size = unsafe {
            libc::getxattr(
                p.as_ptr(),
                n.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        (size >= 0).then(|| value[..size as usize].to_vec())
    }

    fn set_xattr(path: &Path, name: &str, value: &[u8]) {
        let (p, n) = (c_path(path).unwrap(), CString::new(name).unwrap());
        // SAFETY: NUL-terminated strings and a buffer of the length passed.
        let status = unsafe {
            libc::setxattr(
                p.as_ptr(),
                n.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        assert_eq!(status, 0);
    }

    #[test]
    fn copies_content_attributes_modes_and_times_with_the_parents() {
        let root = std::env::temp_dir().join(format!("aim-copy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (from, to) = (root.join("from"), root.join("to"));
        let cache = from.join("data/system/cache");
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(from.join("data/system/other")).unwrap();
        fs::create_dir_all(&to).unwrap();
        let entry = cache.join("entry");
        fs::write(&entry, "parcel").unwrap();
        set_xattr(&entry, "dev.aim.guest-inode", b"DAGI\x01\x07");
        set_xattr(&from.join("data/system"), "dev.aim.guest-inode", b"system");
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o400)).unwrap();
        let old = [
            libc::timespec {
                tv_sec: 1_000_000,
                tv_nsec: 5,
            },
            libc::timespec {
                tv_sec: 2_000_000,
                tv_nsec: 7,
            },
        ];
        for path in [&entry, &cache] {
            let p = c_path(path).unwrap();
            // SAFETY: a NUL-terminated path and two timespecs.
            assert_eq!(
                unsafe { libc::utimensat(libc::AT_FDCWD, p.as_ptr(), old.as_ptr(), 0) },
                0
            );
        }

        let files = copy_paths(&from, &to, &[Path::new("data/system/cache")]).unwrap();
        assert_eq!(files, [PathBuf::from("data/system/cache/entry")]);
        let copied = to.join("data/system/cache/entry");
        assert_eq!(fs::read_to_string(&copied).unwrap(), "parcel");
        assert_eq!(
            xattr(&copied, "dev.aim.guest-inode").unwrap(),
            b"DAGI\x01\x07"
        );
        assert_eq!(
            xattr(&to.join("data/system"), "dev.aim.guest-inode").unwrap(),
            b"system"
        );
        let meta = fs::metadata(&copied).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o400);
        assert_eq!((meta.mtime(), meta.mtime_nsec()), (2_000_000, 7));
        let dir = fs::metadata(to.join("data/system/cache")).unwrap();
        assert_eq!((dir.mtime(), dir.mtime_nsec()), (2_000_000, 7));
        // Only what was asked for.
        assert!(!to.join("data/system/other").exists());
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }
}
