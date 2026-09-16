//! Host directory authority prepared before ART or NativeLoader starts.
//! Android path resolution stays in the native filesystem owner; this module
//! only opens the explicitly configured macOS directory and lends its fd.

use crate::config::HostError;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

fn open(root: &Path) -> Result<File, HostError> {
    if !root.is_absolute() {
        return Err(HostError::HostService(format!(
            "Android filesystem authority must be absolute: {}",
            root.display()
        )));
    }
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|error| {
            HostError::HostService(format!("open filesystem root {}: {error}", root.display()))
        })?;
    Ok(root)
}

pub(super) fn open_filesystem_authority() -> Result<Option<File>, HostError> {
    if let Some(root) =
        std::env::var_os("DARWIN_ART_ANDROID_FILESYSTEM_ROOT").filter(|v| !v.is_empty())
    {
        return open(Path::new(&root)).map(Some);
    }
    if let Some(root) = std::env::var_os("DARWIN_ART_ANDROID_SYSTEM_ROOT").filter(|v| !v.is_empty())
    {
        return open(Path::new(&root)).map(Some);
    }
    // Non-Android test processes may have no guest filesystem. No host root
    // is implicitly exposed when the caller supplied no directory authority.
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_directory_retains_authority() {
        let opened = open(Path::new("/")).unwrap();
        assert!(opened.metadata().unwrap().is_dir());
    }

    #[test]
    fn empty_root_and_non_directory_fail() {
        assert!(open(Path::new("")).is_err());
        assert!(open(Path::new(".")).is_err());
        assert!(open(Path::new("/dev/null")).is_err());
    }
}
