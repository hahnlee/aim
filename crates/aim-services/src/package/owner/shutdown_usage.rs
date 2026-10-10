//! PackageUsage AtomicFile writeNow, pinned AOSP Apache-2.0.
use super::{Store, WriteError};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};
impl Store {
    pub fn write_usage_now(
        &mut self,
        usage: &crate::package::owner::usage::Usage,
    ) -> Result<(), WriteError> {
        let dir = self.data.join("system");
        fs::create_dir_all(&dir).map_err(WriteError::before)?;
        let main = dir.join("package-usage.list");
        let backup = crate::package::sibling(&main, ".bak");
        let new = crate::package::sibling(&main, ".new");
        for path in [&main, &backup, &new] {
            if fs::symlink_metadata(path)
                .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
            {
                return Err(WriteError::before(
                    "package usage path is not an owned regular file",
                ));
            }
        }
        if backup.exists() {
            fs::rename(&backup, &main).map_err(WriteError::before)?;
        }
        let mut bytes = Vec::from(b"PACKAGE_USAGE__VERSION_1\n".as_slice());
        let mut settings = self.state.settings.packages.iter().collect::<Vec<_>>();
        settings.sort_by_key(|setting| crate::package::info::java_hash(&setting.name));
        for setting in settings {
            let Some(times) = usage.times(&setting.name) else {
                return Err(WriteError::before(
                    "package usage setting absent from live usage owner",
                ));
            };
            if times.iter().copied().fold(0, i64::max) == 0 {
                continue;
            }
            for c in setting.name.chars() {
                bytes.push(if c.is_ascii() { c as u8 } else { b'?' });
            }
            for time in times {
                bytes.push(b' ');
                bytes.extend_from_slice(time.to_string().as_bytes());
            }
            bytes.push(b'\n');
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&new)
            .map_err(WriteError::before)?;
        let identity = file.metadata().map_err(WriteError::before)?;
        let result = (|| -> std::io::Result<()> {
            file.set_permissions(fs::Permissions::from_mode(0o640))?;
            aim_storage::guest_inode::record(
                &new,
                aim_storage::guest_inode::GuestInode {
                    uid: Some(1000),
                    gid: Some(1032),
                    mode: Some(0o640),
                },
            )?;
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()?;
            fs::rename(&new, &main)
        })();
        drop(file);
        if let Err(error) = result {
            use std::os::unix::fs::MetadataExt;
            let cleanup = match fs::symlink_metadata(&new) {
                Ok(current)
                    if current.dev() == identity.dev() && current.ino() == identity.ino() =>
                {
                    fs::remove_file(&new)
                }
                Ok(_) => {
                    return Err(WriteError::before(format!(
                        "{error}; usage temporary identity changed"
                    )));
                }
                Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(inspect) => Err(inspect),
            };
            return Err(WriteError::before(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; usage temporary cleanup: {cleanup}"),
            }));
        }
        Ok(())
    }
}
