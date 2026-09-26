//! PackageManagerService's `/data/system/packages.list`, read the way
//! libpackagelistparser reads it for installd, run-as and the zygote: the
//! authority for an installed package's uid (ADR 0009).
use crate::ProfileError;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One `packages.list` line (Settings.writePackageListLPrInternal).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PackageListEntry {
    pub(crate) name: String,
    pub(crate) uid: u32,
    pub(crate) debuggable: bool,
    /// Guest path of the package's credential-encrypted data.
    pub(crate) data_path: String,
    /// The installing package, or `@system`, `@product` or `@null`.
    pub(crate) installer: String,
}

impl PackageListEntry {
    /// Not a system or product package (`pm list packages -3`); an install
    /// without a recorded installer is `@null`.
    pub(crate) fn is_third_party(&self) -> bool {
        self.installer != "@system" && self.installer != "@product"
    }
}

/// The system process's `packages.list` (its private `/data/system`).
pub(crate) fn path(mount: &Path) -> PathBuf {
    mount.join("data/apps/android.system/private-data/system/packages.list")
}

fn parse_line(line: &str) -> Option<PackageListEntry> {
    let mut fields = line.split(' ');
    let name = fields.next()?;
    let uid = fields.next()?.parse().ok()?;
    let debuggable = match fields.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let data_path = fields.next()?;
    // seinfo, gids, profileableFromShell, longVersionCode, profileable, installer.
    let rest = fields.collect::<Vec<_>>();
    if name.is_empty() || !data_path.starts_with('/') || rest.len() < 6 {
        return None;
    }
    Some(PackageListEntry {
        name: name.to_owned(),
        uid,
        debuggable,
        data_path: data_path.to_owned(),
        installer: rest[5].to_owned(),
    })
}

/// Every package PMS has written, in file order.
pub(crate) fn read(mount: &Path) -> Result<Vec<PackageListEntry>, ProfileError> {
    let text = match fs::read_to_string(path(mount)) {
        Ok(text) => text,
        // PMS has not booted on this profile yet: nothing is installed.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            parse_line(line)
                .ok_or_else(|| ProfileError::Daemon(format!("invalid packages.list line: {line}")))
        })
        .collect()
}

/// The installed package's uid, or NotFound when PMS has no such package.
pub(crate) fn uid(mount: &Path, package: &str) -> Result<u32, ProfileError> {
    crate::registry::validate_package(package)?;
    read(mount)?
        .into_iter()
        .find(|entry| entry.name == package)
        .map(|entry| entry.uid)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{package} is not installed"),
            )
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_package_manager_lines() {
        let mount =
            std::env::temp_dir().join(format!("darwin-package-list-{}", std::process::id()));
        let _ = fs::remove_dir_all(&mount);
        assert!(read(&mount).unwrap().is_empty());
        assert!(
            matches!(uid(&mount, "org.example"), Err(ProfileError::Io(error))
            if error.kind() == io::ErrorKind::NotFound)
        );
        fs::create_dir_all(path(&mount).parent().unwrap()).unwrap();
        fs::write(
            path(&mount),
            "com.android.shell 2000 0 /data/user/0/com.android.shell platform:privapp:targetSdkVersion=36 3002,3003 0 36 1 @system\n\
             org.example 10003 1 /data/user/0/org.example default:targetSdkVersion=28 none 0 42 1 com.android.shell\n",
        )
        .unwrap();
        let entries = read(&mount).unwrap();
        assert_eq!(
            entries[1],
            PackageListEntry {
                name: "org.example".into(),
                uid: 10003,
                debuggable: true,
                data_path: "/data/user/0/org.example".into(),
                installer: "com.android.shell".into(),
            }
        );
        assert!(entries[1].is_third_party() && !entries[0].is_third_party());
        assert_eq!(uid(&mount, "org.example").unwrap(), 10003);
        assert_eq!(uid(&mount, "com.android.shell").unwrap(), 2000);
        fs::write(path(&mount), "org.example 10003 1\n").unwrap();
        assert!(read(&mount).is_err());
        fs::remove_dir_all(&mount).unwrap();
    }
}
