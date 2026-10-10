//! Settings.writePackageListLPrInternal / JournaledFile, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{WriteError, remove};
use aim_storage::guest_inode::{self, GuestInode};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn write(data: &Path, expected: Option<&str>, bytes: &[u8]) -> Result<(), WriteError> {
    write_with(data, expected, |file| file.write_all(bytes))
}

fn write_with(
    data: &Path,
    expected: Option<&str>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), WriteError> {
    let path = data.join("system/packages.list");
    let temp = data.join("system/packages.list.tmp");
    let current = crate::package::journaled(&path).map_err(WriteError::before)?;
    if current.as_deref().unwrap_or("") != expected.unwrap_or("") {
        return Err(WriteError::before(
            "packages.list changed outside the native owner",
        ));
    }
    // Recover a temporary-only read as JournaledFile.chooseForRead does before
    // chooseForWrite replaces the temporary file. Never discard that old list.
    if !path.exists() && temp.exists() {
        fs::rename(&temp, &path).map_err(WriteError::before)?;
    }
    if !path.exists() {
        let file = File::create(&path).map_err(WriteError::before)?;
        metadata(&file, &path).map_err(WriteError::before)?;
    }
    remove(&temp).map_err(WriteError::before)?;
    let result = (|| {
        let mut file = File::create(&temp)?;
        metadata(&file, &temp)?;
        write(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &path)
    })();
    if let Err(error) = result {
        let cleanup = remove(&temp);
        return Err(WriteError::before(match cleanup {
            Ok(()) => error.to_string(),
            Err(cleanup) => format!("{error}; rollback: {cleanup}"),
        }));
    }
    Ok(())
}

fn metadata(file: &File, path: &Path) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o640))?;
    guest_inode::record(
        path,
        GuestInode {
            uid: Some(1000),
            gid: Some(aim_service_aidl::android_os_process::PACKAGE_INFO_GID),
            mode: Some(0o640),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::tests::Data;

    #[test]
    fn journal_recovers_temp_only_and_rolls_back_partial_writes_and_first_write() {
        let data = Data::new();
        data.settings();
        let path = data.0.join("system/packages.list");
        let temp = data.0.join("system/packages.list.tmp");
        fs::write(&temp, "old\n").unwrap();
        let failure = write_with(&data.0, Some("old\n"), |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("injected write failure"))
        })
        .unwrap_err();
        assert!(!failure.committed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
        assert!(!temp.exists());
        fs::write(&temp, "stale\n").unwrap();
        write(&data.0, Some("old\n"), b"new\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert!(!temp.exists());
        fs::remove_file(&path).unwrap();
        assert!(
            write_with(&data.0, None, |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("injected first write failure"))
            })
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"");
        assert!(!temp.exists());
        write(&data.0, None, b"retry\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "retry\n");
    }
}
