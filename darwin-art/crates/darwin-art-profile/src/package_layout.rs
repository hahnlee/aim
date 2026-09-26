//! PackageManagerService's `/data/app` code-directory layout for the profile
//! package store (ADR 0009).
//!
//! PMS installs each package into `/data/app/~~<random>/<package>-<random>`
//! holding `base.apk`, split APKs, `lib/<isa>` for extracted native libraries
//! and `oat/<isa>`. Earlier installs used `packages/<package>/<version>/<sha>`
//! with native libraries in `android-elf/arm64-v8a`; PMS treats any other
//! shape as an invalid package. `adopt` moves a legacy install into the PMS
//! layout without touching its files and rewrites the ledger record's paths.

use crate::ProfileError;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const LEGACY_NATIVE: &str = "android-elf/arm64-v8a";
const PMS_NATIVE: &str = "lib/arm64";

/// Record lines whose values are paths inside the install directory.
const PATH_KEYS: [&str; 4] = ["apk=", "dex=", "split=", "native_library_dir="];

/// Returns `record` with its install moved into the PMS layout under
/// `store`. Records already in that layout, or not naming a store install,
/// are returned unchanged.
pub(crate) fn adopt(store: &Path, package: &str, record: &[u8]) -> Result<Vec<u8>, ProfileError> {
    let text = std::str::from_utf8(record)
        .map_err(|_| ProfileError::Daemon("launch record is not UTF-8".into()))?;
    let relocated = relocate(store, text);
    let text = relocated.as_str();
    let Some(apk) = text.lines().find_map(|line| line.strip_prefix("apk=")) else {
        return Ok(record.to_vec());
    };
    let apk = Path::new(apk);
    let Some(directory) = apk.parent() else {
        return Ok(record.to_vec());
    };
    let Some(relative) = directory.strip_prefix(store).ok() else {
        return Ok(record.to_vec()); // Not a profile store install (tests, fixtures).
    };
    let components: Vec<_> = relative.iter().collect();
    let legacy = components.len() == 3 && components[0] == package;
    let target = if legacy {
        match recover(store, package, directory)? {
            Some(moved) => moved,
            None => move_install(store, package, directory)?,
        }
    } else {
        directory.to_path_buf()
    };
    adopt_native_directory(&target)?;
    Ok(rewrite(text, directory, &target).into_bytes())
}

/// A previous adoption that moved the directory but crashed before the
/// record was rewritten: the install is found by its identity file.
fn recover(store: &Path, package: &str, legacy: &Path) -> Result<Option<PathBuf>, ProfileError> {
    if legacy.exists() {
        return Ok(None);
    }
    let identity = legacy
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ProfileError::Daemon("invalid legacy install path".into()))?;
    for bucket in fs::read_dir(store)? {
        let bucket = bucket?.path();
        if !bucket
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("~~"))
        {
            continue;
        }
        for install in fs::read_dir(&bucket)? {
            let install = install?.path();
            let contract = fs::read_to_string(install.join("install.contract")).unwrap_or_default();
            if contract
                .lines()
                .any(|line| line == format!("package={package}"))
                && contract
                    .lines()
                    .any(|line| line == format!("apk-sha256={identity}"))
            {
                return Ok(Some(install));
            }
        }
    }
    Err(ProfileError::Daemon(format!(
        "installed code for {package} is missing: {}",
        legacy.display()
    )))
}

fn move_install(store: &Path, package: &str, legacy: &Path) -> Result<PathBuf, ProfileError> {
    let bucket = store.join(format!("~~{}", random_name()?));
    let target = bucket.join(format!("{package}-{}", random_name()?));
    fs::create_dir(&bucket)?;
    // Moving a directory rewrites its `..` entry; installs are read-only.
    let mut permissions = fs::metadata(legacy)?.permissions();
    let mode = permissions.mode();
    permissions.set_mode(mode | 0o700);
    fs::set_permissions(legacy, permissions)?;
    fs::rename(legacy, &target)?;
    let mut permissions = fs::metadata(&target)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(&target, permissions)?;
    // Drop the now-empty `<package>/<version>` parents.
    for parent in [legacy.parent(), legacy.parent().and_then(Path::parent)]
        .into_iter()
        .flatten()
    {
        match fs::remove_dir(parent) {
            Ok(()) => {}
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOTEMPTY)) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(target)
}

/// `android-elf/arm64-v8a` becomes PMS's `lib/arm64`.
fn adopt_native_directory(install: &Path) -> Result<(), ProfileError> {
    let legacy = install.join(LEGACY_NATIVE);
    let target = install.join(PMS_NATIVE);
    if !legacy.exists() || target.exists() {
        return Ok(());
    }
    let writable = |path: &Path| -> io::Result<u32> {
        let mut permissions = fs::metadata(path)?.permissions();
        let mode = permissions.mode();
        permissions.set_mode(mode | 0o700);
        fs::set_permissions(path, permissions)?;
        Ok(mode)
    };
    let install_mode = writable(install)?;
    let elf_mode = writable(&install.join("android-elf"))?;
    let native_mode = writable(&legacy)?;
    fs::create_dir(install.join("lib"))?;
    fs::rename(&legacy, &target)?;
    fs::set_permissions(&target, fs::Permissions::from_mode(native_mode))?;
    fs::remove_dir(install.join("android-elf"))?;
    fs::set_permissions(install.join("lib"), fs::Permissions::from_mode(elf_mode))?;
    fs::set_permissions(install, fs::Permissions::from_mode(install_mode))?;
    Ok(())
}

/// Records name the store by the host path it had when they were written.
/// A copied or renamed profile keeps its installs in its own store, so a
/// path inside another `/packages/` store names the same install in `store`
/// when that install exists there.
fn relocate(store: &Path, record: &str) -> String {
    let store = store.to_string_lossy();
    let mut out = String::with_capacity(record.len());
    for line in record.split_inclusive('\n') {
        match PATH_KEYS.iter().find(|key| line.starts_with(**key)) {
            Some(key) => {
                let value = line[key.len()..].trim_end_matches('\n');
                let relocated = value
                    .rsplit_once("/packages/")
                    .filter(|_| !value.starts_with(store.as_ref()))
                    .map(|(_, inside)| format!("{store}/{inside}"))
                    // Only an install that is in this store is the same one.
                    .filter(|candidate| Path::new(candidate).exists());
                let value = relocated.unwrap_or_else(|| value.to_string());
                out.push_str(key);
                out.push_str(&value);
                out.push('\n');
            }
            None => out.push_str(line),
        }
    }
    out
}

fn rewrite(record: &str, legacy: &Path, target: &Path) -> String {
    let legacy = legacy.to_string_lossy();
    let target = target.to_string_lossy();
    let mut out = String::with_capacity(record.len());
    for line in record.split_inclusive('\n') {
        match PATH_KEYS.iter().find(|key| line.starts_with(**key)) {
            Some(key) => {
                let value = &line[key.len()..].trim_end_matches('\n');
                let value = value
                    .strip_prefix(legacy.as_ref())
                    .map(|rest| format!("{target}{rest}"))
                    .unwrap_or_else(|| value.to_string())
                    .replace(&format!("/{LEGACY_NATIVE}"), &format!("/{PMS_NATIVE}"));
                out.push_str(key);
                out.push_str(&value);
                out.push('\n');
            }
            None => out.push_str(line),
        }
    }
    out
}

/// PackageManagerServiceUtils.getNextCodePath: 16 random bytes, URL-safe
/// base64 ("==" padded).
fn random_name() -> Result<String, ProfileError> {
    let mut bytes = [0u8; 16];
    // SAFETY: exact writable span.
    if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |acc, (index, byte)| {
            acc | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..=chunk.len() {
            out.push(TABLE[((value >> (18 - 6 * index)) & 63) as usize] as char);
        }
    }
    while out.len() % 4 != 0 {
        out.push('=');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "darwin-package-layout-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn legacy_install(store: &Path) -> PathBuf {
        let install = store.join("org.example/7/abc");
        fs::create_dir_all(install.join("android-elf/arm64-v8a")).unwrap();
        fs::create_dir_all(install.join("oat/arm64")).unwrap();
        fs::write(install.join("base.apk"), b"base").unwrap();
        fs::write(install.join("split-0.apk"), b"split").unwrap();
        fs::write(install.join("android-elf/arm64-v8a/libx.so"), b"elf").unwrap();
        fs::write(
            install.join("install.contract"),
            "darwin-art-apk-install-v1\npackage=org.example\napk-sha256=abc\n",
        )
        .unwrap();
        for path in [
            install.join("android-elf/arm64-v8a"),
            install.join("android-elf"),
            install.clone(),
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
        }
        install
    }

    #[test]
    fn moves_a_legacy_install_into_the_pms_layout() {
        let store = store("move");
        let install = legacy_install(&store);
        let i = install.to_string_lossy();
        let record = format!(
            "darwin-art-launch-v1\napk={i}/base.apk\ndex={i}/base.apk\nsplit={i}/split-0.apk\n\
             native_library_dir={i}/android-elf/arm64-v8a\nmetadata=kept\napp_id=10001\n"
        );
        let adopted =
            String::from_utf8(adopt(&store, "org.example", record.as_bytes()).unwrap()).unwrap();
        let apk = adopted
            .lines()
            .find_map(|l| l.strip_prefix("apk="))
            .unwrap();
        let target = Path::new(apk).parent().unwrap();
        let bucket = target.parent().unwrap();
        assert!(
            bucket
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("~~")
        );
        assert!(
            target
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("org.example-")
        );
        assert_eq!(bucket.parent().unwrap(), store);
        assert_eq!(fs::read(target.join("base.apk")).unwrap(), b"base");
        assert_eq!(fs::read(target.join("split-0.apk")).unwrap(), b"split");
        assert_eq!(fs::read(target.join("lib/arm64/libx.so")).unwrap(), b"elf");
        assert!(target.join("oat/arm64").is_dir());
        assert!(!target.join("android-elf").exists());
        assert!(!store.join("org.example").exists());
        let t = target.to_string_lossy();
        assert!(adopted.contains(&format!("split={t}/split-0.apk\n")));
        assert!(adopted.contains(&format!("native_library_dir={t}/lib/arm64\n")));
        assert!(adopted.ends_with("metadata=kept\napp_id=10001\n"));
        // Adopting again is a no-op.
        assert_eq!(
            adopt(&store, "org.example", adopted.as_bytes()).unwrap(),
            adopted.as_bytes()
        );
    }

    #[test]
    fn recovers_a_move_whose_record_was_not_rewritten() {
        let store = store("recover");
        let install = legacy_install(&store);
        let record = format!(
            "darwin-art-launch-v1\napk={}/base.apk\n",
            install.to_string_lossy()
        );
        let moved = move_install(&store, "org.example", &install).unwrap();
        let adopted =
            String::from_utf8(adopt(&store, "org.example", record.as_bytes()).unwrap()).unwrap();
        assert_eq!(
            adopted,
            format!(
                "darwin-art-launch-v1\napk={}/base.apk\n",
                moved.to_string_lossy()
            )
        );
        assert!(moved.join("lib/arm64/libx.so").exists());
    }

    #[test]
    fn adopts_a_record_written_under_another_profile_location() {
        let store = store("relocated");
        let install = legacy_install(&store);
        let inside = install
            .strip_prefix(&store)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let record = format!(
            "darwin-art-launch-v1\napk=/Users/someone/profiles/old/mnt/packages/{inside}/base.apk\n"
        );
        let adopted =
            String::from_utf8(adopt(&store, "org.example", record.as_bytes()).unwrap()).unwrap();
        let apk = adopted
            .lines()
            .find_map(|l| l.strip_prefix("apk="))
            .unwrap();
        assert!(Path::new(apk).starts_with(&store));
        assert_eq!(fs::read(apk).unwrap(), b"base");
    }

    #[test]
    fn leaves_records_outside_the_store_unchanged() {
        let store = store("outside");
        let record = b"darwin-art-launch-v1\napk=/elsewhere/base.apk\n";
        assert_eq!(adopt(&store, "org.example", record).unwrap(), record);
    }

    #[test]
    fn random_names_are_pms_shaped() {
        let name = random_name().unwrap();
        assert_eq!(name.len(), 24);
        assert!(name.ends_with("=="));
        assert!(
            name[..22]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }
}
