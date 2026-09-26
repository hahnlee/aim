use crate::{ProfileError, ProfilePaths};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

const RECORD_VERSION: &[u8] = b"darwin-art-launch-v1\n";

/// The install ledger that preceded PackageManagerService. It is read once,
/// by the migration into PMS Settings (settings_migration); PMS owns every
/// installed package afterwards.
pub(crate) struct PackageRegistry {
    directory: PathBuf,
    /// The package store (`/data/app`); installs are kept in PMS's layout.
    store: PathBuf,
}

impl PackageRegistry {
    pub(crate) fn new(paths: &ProfilePaths) -> Self {
        Self {
            directory: paths.mount.join("system/package-registry"),
            store: paths.mount.join("packages"),
        }
    }

    pub(crate) fn register(&self, package: &str, record: &[u8]) -> Result<(), ProfileError> {
        validate_package(package)?;
        validate_record(record)?;
        let record = &crate::package_layout::adopt(&self.store, package, record)?;
        validate_record(record)?;
        fs::create_dir_all(&self.directory)?;
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        let app_id = crate::app_ids::allocate(&self.directory, package)?;
        let record = with_app_id(record, app_id);
        validate_record(&record)?;
        let destination = self.directory.join(format!("{package}.launch"));
        match fs::read(&destination) {
            Ok(existing) if existing == record => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let stage = self
            .directory
            .join(format!(".{package}.register-{}", std::process::id()));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&stage)?;
        let result = output
            .write_all(&record)
            .and_then(|()| output.sync_all())
            .and_then(|()| fs::set_permissions(&stage, fs::Permissions::from_mode(0o400)))
            .and_then(|()| fs::rename(&stage, &destination))
            .and_then(|()| File::open(&self.directory)?.sync_all());
        if let Err(error) = result {
            let _ = fs::remove_file(&stage);
            return Err(error.into());
        }
        Ok(())
    }

    pub(crate) fn resolve(&self, package: &str) -> Result<Vec<u8>, ProfileError> {
        validate_package(package)?;
        let record = fs::read(self.directory.join(format!("{package}.launch")))?;
        validate_record(&record)?;
        Ok(record)
    }

    /// Upgrade installed launch metadata before exposing a mounted profile.
    /// Validate every input before writing any; preserve all APK/data paths.
    /// Each publication is atomic and rerunning after interruption is safe.
    pub(crate) fn migrate_app_ids(&self) -> Result<(), ProfileError> {
        let listing = self.list()?;
        let listing = std::str::from_utf8(&listing)
            .map_err(|_| ProfileError::Daemon("package listing is not UTF-8".into()))?;
        let records = listing
            .lines()
            .map(|package| self.resolve(package).map(|record| (package, record)))
            .collect::<Result<Vec<_>, _>>()?;
        for (package, record) in records {
            self.register(package, &record)?;
        }
        Ok(())
    }

    /// Writes PackageManagerService Settings for the ledger's installs before
    /// the first PMS boot (settings_migration).
    pub(crate) fn migrate_settings(&self) -> Result<(), ProfileError> {
        let listing = self.list()?;
        let listing = std::str::from_utf8(&listing)
            .map_err(|_| ProfileError::Daemon("package listing is not UTF-8".into()))?;
        let packages = listing
            .lines()
            .map(|package| {
                let record = self.resolve(package)?;
                crate::settings_migration::LedgerPackage::from_record(&self.store, package, &record)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mount = self
            .store
            .parent()
            .ok_or_else(|| ProfileError::Daemon("package store has no profile mount".into()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        crate::settings_migration::migrate(mount, &packages, now)?;
        Ok(())
    }

    pub(crate) fn list(&self) -> Result<Vec<u8>, ProfileError> {
        let mut packages = Vec::new();
        match fs::read_dir(&self.directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    let Some(package) = name.strip_suffix(".launch") else {
                        continue;
                    };
                    if entry.file_type()?.is_file() && validate_package(package).is_ok() {
                        packages.push(package.to_owned());
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        packages.sort();
        let mut listing = packages.join("\n");
        if !listing.is_empty() {
            listing.push('\n');
        }
        Ok(listing.into_bytes())
    }
}

pub(crate) fn validate_package(package: &str) -> Result<(), ProfileError> {
    if package.is_empty()
        || package.len() > 255
        || package.starts_with('.')
        || package.ends_with('.')
        || package.contains("..")
        || !package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(ProfileError::Daemon("invalid Android package name".into()));
    }
    Ok(())
}

fn validate_record(record: &[u8]) -> Result<(), ProfileError> {
    if !record.starts_with(RECORD_VERSION)
        || record.len() > 60 * 1024
        || record.contains(&0)
        || !record.ends_with(b"\n")
    {
        return Err(ProfileError::Daemon("invalid launch record".into()));
    }
    Ok(())
}

fn with_app_id(record: &[u8], app_id: u32) -> Vec<u8> {
    let mut result = Vec::with_capacity(record.len() + 20);
    for line in record.split_inclusive(|byte| *byte == b'\n') {
        // App IDs are registry-owned, never accepted from installation input.
        if !line.starts_with(b"app_id=") {
            result.extend_from_slice(line);
        }
    }
    result.extend_from_slice(format!("app_id={app_id}\n").as_bytes());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_cannot_supply_privileged_app_id() {
        assert_eq!(
            with_app_id(b"darwin-art-launch-v1\napp_id=1000\napp_id=0\n", 10000),
            b"darwin-art-launch-v1\napp_id=10000\n"
        );
    }

    #[test]
    fn migration_preserves_metadata_and_reloads_ids() {
        let directory = std::env::temp_dir().join(format!(
            "darwin-registry-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let registry = PackageRegistry {
            directory: directory.clone(),
            store: directory.join("packages"),
        };
        let legacy = b"darwin-art-launch-v1\napk=/packages/a/base.apk\nmetadata=unchanged\n";
        fs::write(directory.join("org.example.a.launch"), legacy).unwrap();
        fs::write(directory.join("org.example.b.launch"), legacy).unwrap();
        registry.migrate_app_ids().unwrap();
        let first = registry.resolve("org.example.a").unwrap();
        assert_eq!(first, with_app_id(legacy, 10000));
        assert_eq!(
            registry.resolve("org.example.b").unwrap(),
            with_app_id(legacy, 10001)
        );
        let before = fs::metadata(directory.join("org.example.a.launch"))
            .unwrap()
            .modified()
            .unwrap();
        registry.migrate_app_ids().unwrap();
        assert_eq!(registry.resolve("org.example.a").unwrap(), first);
        assert_eq!(
            fs::metadata(directory.join("org.example.a.launch"))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
        fs::write(directory.join("org.example.c.launch"), b"corrupt").unwrap();
        assert!(registry.migrate_app_ids().is_err());
        assert_eq!(registry.resolve("org.example.a").unwrap(), first);
        fs::remove_file(directory.join("org.example.c.launch")).unwrap();
        fs::remove_file(directory.join("app-ids")).unwrap();
        assert!(registry.migrate_app_ids().is_err());
        assert_eq!(registry.resolve("org.example.a").unwrap(), first);
        for entry in fs::read_dir(&directory).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn package_names_cannot_escape_the_registry() {
        for valid in ["com.android.calculator2", "org.example.App"] {
            assert!(validate_package(valid).is_ok());
        }
        for invalid in ["", ".bad", "bad.", "a..b", "a/b"] {
            assert!(validate_package(invalid).is_err());
        }
    }
}
