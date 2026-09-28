//! One-time move of host state from its pre-rename (darwin-art) location.
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Moves `old` to `new` with a single exclusive `rename`: only when `new`
/// does not exist yet, never copying, replacing or deleting anything.
/// Returns whether it moved.
pub(crate) fn migrate(old: &Path, new: &Path) -> io::Result<bool> {
    let from = CString::new(old.as_os_str().as_bytes())?;
    let to = CString::new(new.as_os_str().as_bytes())?;
    // SAFETY: both paths are NUL-terminated and outlive the call.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOENT | libc::EEXIST) => Ok(false),
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::migrate;
    use std::fs;

    #[test]
    fn moves_once_and_never_replaces() {
        let root = std::env::temp_dir().join(format!("aim-profile-migrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (old, new) = (root.join("DarwinART"), root.join("aim"));
        fs::create_dir_all(old.join("profiles/default")).unwrap();
        fs::write(old.join("profiles/default/state"), b"kept").unwrap();

        assert!(migrate(&old, &new).unwrap());
        assert!(!old.exists());
        assert_eq!(
            fs::read(new.join("profiles/default/state")).unwrap(),
            b"kept"
        );
        assert!(!migrate(&old, &new).unwrap());

        // Both present: neither is touched, even an empty new directory.
        fs::create_dir(&old).unwrap();
        fs::write(old.join("stale"), b"old").unwrap();
        assert!(!migrate(&old, &new).unwrap());
        assert_eq!(fs::read(old.join("stale")).unwrap(), b"old");
        assert_eq!(
            fs::read(new.join("profiles/default/state")).unwrap(),
            b"kept"
        );
        let empty = root.join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(!migrate(&old, &empty).unwrap());
        assert!(old.join("stale").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
