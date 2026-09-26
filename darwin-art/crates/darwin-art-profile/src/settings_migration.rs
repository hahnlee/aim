//! One-time migration of the install ledger into PackageManagerService
//! Settings (ADR 0009).
//!
//! PMS keeps a `PackageSetting` for every non-system package in
//! `/data/system/packages.xml`, and on boot it removes any `/data/app`
//! directory that has none. Before the first PMS boot of a profile, the
//! ledger's installs are written there with their existing app ids, so the
//! packages, their uids and their data carry over. Once PMS has written its
//! own settings, this migration never runs again.

use crate::ProfileError;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One ledger install, as PMS Settings records it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LedgerPackage {
    pub(crate) name: String,
    /// Guest code directory under `/data/app`.
    pub(crate) code_path: String,
    pub(crate) version_code: i64,
    pub(crate) app_id: u32,
    pub(crate) has_native_libraries: bool,
}

impl LedgerPackage {
    /// Reads a ledger record whose install is in the PMS layout under `store`.
    pub(crate) fn from_record(
        store: &Path,
        name: &str,
        record: &[u8],
    ) -> Result<Self, ProfileError> {
        let text = std::str::from_utf8(record)
            .map_err(|_| ProfileError::Daemon("launch record is not UTF-8".into()))?;
        let field = |key: &str| text.lines().find_map(|line| line.strip_prefix(key));
        let invalid = |what: &str| ProfileError::Daemon(format!("{name}: {what}"));
        let apk = field("apk=").ok_or_else(|| invalid("record has no APK"))?;
        // Records name the store by its host path when they were written; a
        // copied or renamed profile has the same install in its own store.
        let relative = apk
            .rsplit_once("/packages/")
            .map(|(_, inside)| inside)
            .and_then(|inside| Path::new(inside).parent())
            .ok_or_else(|| invalid("install is outside the package store"))?;
        let code = store.join(relative);
        if !code.join("base.apk").is_file() {
            return Err(invalid("install is missing from the package store"));
        }
        if relative.iter().count() != 2
            || !relative
                .iter()
                .next()
                .and_then(|b| b.to_str())
                .is_some_and(|b| b.starts_with("~~"))
        {
            return Err(invalid(
                "install is not in the PackageManagerService layout",
            ));
        }
        let version_code = text
            .split(|c: char| c == ' ' || c == '\n')
            .find_map(|token| token.strip_prefix("version_code="))
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| invalid("record has no version code"))?;
        let app_id = field("app_id=")
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| invalid("record has no app id"))?;
        let native = code.join("lib/arm64");
        let has_native_libraries = fs::read_dir(&native)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        Ok(Self {
            name: name.to_owned(),
            code_path: format!("/data/app/{}", relative.to_string_lossy()),
            version_code,
            app_id,
            has_native_libraries,
        })
    }
}

/// The system process's Settings file (its private `/data/system`).
pub(crate) fn settings_path(mount: &Path) -> PathBuf {
    mount.join("data/apps/android.system/private-data/system/packages.xml")
}

/// Writes `packages.xml` for `packages` unless PMS settings already exist.
/// Returns whether it wrote the file.
pub(crate) fn migrate(
    mount: &Path,
    packages: &[LedgerPackage],
    now_millis: u64,
) -> Result<bool, ProfileError> {
    let settings = settings_path(mount);
    let directory = settings.parent().expect("settings has a parent");
    if packages.is_empty()
        || settings.exists()
        || directory.join("packages-backup.xml").exists()
        || directory.join("packages-reserve-copy.xml").exists()
    {
        return Ok(false);
    }
    fs::create_dir_all(directory)?;
    let mut xml =
        String::from("<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<packages>\n");
    for package in packages {
        let mut attributes = vec![
            ("name", escape(&package.name)),
            ("codePath", escape(&package.code_path)),
            (
                "nativeLibraryPath",
                escape(&format!("{}/lib", package.code_path)),
            ),
        ];
        if package.has_native_libraries {
            attributes.push(("primaryCpuAbi", "arm64-v8a".into()));
        }
        attributes.extend([
            ("publicFlags", "0".into()),
            ("privateFlags", "0".into()),
            ("ft", format!("{now_millis:x}")),
            ("it", format!("{now_millis:x}")),
            ("ut", format!("{now_millis:x}")),
            ("version", package.version_code.to_string()),
            ("userId", package.app_id.to_string()),
        ]);
        xml.push_str("  <package");
        for (name, value) in attributes {
            xml.push_str(&format!(" {name}=\"{value}\""));
        }
        xml.push_str(" />\n");
    }
    xml.push_str("</packages>\n");
    let stage = directory.join(format!(".packages.xml.migrate-{}", std::process::id()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)?;
    output.write_all(xml.as_bytes())?;
    output.sync_all()?;
    fs::rename(&stage, &settings)?;
    fs::File::open(directory)?.sync_all()?;
    Ok(true)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("darwin-settings-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn reads_a_pms_layout_record() {
        let mount = root("record");
        let store = mount.join("packages");
        let code = store.join("~~A==/org.example-B==");
        fs::create_dir_all(code.join("lib/arm64")).unwrap();
        fs::write(code.join("lib/arm64/libx.so"), b"").unwrap();
        fs::write(code.join("base.apk"), b"apk").unwrap();
        let record = format!(
            "darwin-art-launch-v1\napk={}/base.apk\nmetadata=apk-app-runtime: package=org.example version_code=42 x=y\napp_id=10003\n",
            code.display()
        );
        let package = LedgerPackage::from_record(&store, "org.example", record.as_bytes()).unwrap();
        assert_eq!(
            package,
            LedgerPackage {
                name: "org.example".into(),
                code_path: "/data/app/~~A==/org.example-B==".into(),
                version_code: 42,
                app_id: 10003,
                has_native_libraries: true,
            }
        );
        // A record written under the profile's earlier location names the
        // same install in this store.
        let relocated = record.replace(
            &store.display().to_string(),
            "/Users/someone/DarwinART/profiles/old/mnt/packages",
        );
        assert_eq!(
            LedgerPackage::from_record(&store, "org.example", relocated.as_bytes()).unwrap(),
            package
        );
        let missing = record.replace("~~A==", "~~C==");
        assert!(LedgerPackage::from_record(&store, "org.example", missing.as_bytes()).is_err());
        let legacy = format!(
            "darwin-art-launch-v1\napk={}/org.example/42/abc/base.apk\n",
            store.display()
        );
        assert!(LedgerPackage::from_record(&store, "org.example", legacy.as_bytes()).is_err());
    }

    #[test]
    fn writes_settings_once() {
        let mount = root("write");
        let packages = [LedgerPackage {
            name: "org.example".into(),
            code_path: "/data/app/~~A==/org.example-B==".into(),
            version_code: 42,
            app_id: 10003,
            has_native_libraries: false,
        }];
        assert!(migrate(&mount, &packages, 0x1234).unwrap());
        let written = fs::read_to_string(settings_path(&mount)).unwrap();
        assert!(written.contains(
            "<package name=\"org.example\" codePath=\"/data/app/~~A==/org.example-B==\" \
             nativeLibraryPath=\"/data/app/~~A==/org.example-B==/lib\" publicFlags=\"0\" \
             privateFlags=\"0\" ft=\"1234\" it=\"1234\" ut=\"1234\" version=\"42\" userId=\"10003\" />"
        ));
        // PMS owns the file afterwards; the migration never rewrites it.
        fs::write(settings_path(&mount), "pms").unwrap();
        assert!(!migrate(&mount, &packages, 0x9999).unwrap());
        assert_eq!(fs::read_to_string(settings_path(&mount)).unwrap(), "pms");
        assert!(!migrate(&root("empty"), &[], 0).unwrap());
    }
}
