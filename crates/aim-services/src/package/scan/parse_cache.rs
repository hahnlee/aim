//! Parsed system packages kept across boots, as `PackageCacher` keeps them
//! in `/data/system/package_cache`. An entry is used only for the same image,
//! runtime build, platform inputs and package files it was parsed from.
use super::super::write::Apks;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "/data/system/aim_package_cache";

pub(super) struct Cache {
    dir: PathBuf,
    base: Sha256,
}

impl Cache {
    /// The cache of this image and runtime; `None` (parse everything) when
    /// either identity is unavailable.
    pub fn open(apks: &Apks) -> Option<Self> {
        let image = fs::read((apks.files)("/.identity")?).ok()?;
        let exe = fs::metadata(std::env::current_exe().ok()?).ok()?;
        let platform = &apks.platform;
        let mut base = Sha256::new();
        base.update(b"AIMPARSE1\0");
        base.update(&image);
        base.update(format!(
            "{}:{}:{}\0",
            exe.ino(),
            exe.size(),
            exe.mtime_nsec() + exe.mtime() * 1_000_000_000
        ));
        let mut features: Vec<_> = platform.features.iter().collect();
        features.sort();
        let mut extensions: Vec<_> = platform.sdk_extensions.iter().flatten().collect();
        extensions.sort();
        base.update(format!("{features:?}{extensions:?}"));
        let generation = hex(&base.clone().finalize()[..8]);
        let root = (apks.files)(DIRECTORY)?;
        // Entries of another image or build are never read again.
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry.file_name().to_str() != Some(&generation) {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        let dir = root.join(generation);
        fs::create_dir_all(&dir).ok()?;
        Some(Self { dir, base })
    }

    /// The entry file for a package at `path`, parsed from `host` with `flags`.
    pub fn entry(&self, host: &Path, path: &str, flags: i32) -> Option<PathBuf> {
        let mut key = self.base.clone();
        key.update(format!("{path}\0{flags}\0"));
        let mut files = Vec::new();
        let meta = fs::metadata(host).ok()?;
        if meta.is_dir() {
            for entry in fs::read_dir(host).ok()? {
                let entry = entry.ok()?;
                let meta = entry.metadata().ok()?;
                if meta.is_file() {
                    files.push((entry.file_name(), meta));
                }
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));
        } else {
            files.push((Default::default(), meta));
        }
        for (name, meta) in files {
            key.update(format!(
                "{name:?}:{}:{}:{}:{}.{}\0",
                meta.dev(),
                meta.ino(),
                meta.size(),
                meta.mtime(),
                meta.mtime_nsec()
            ));
        }
        Some(self.dir.join(hex(&key.finalize())))
    }

    /// Stores `bytes` at `entry`, whole or not at all.
    pub fn store(&self, entry: &Path, bytes: &[u8]) {
        let temporary = entry.with_extension(format!("{}.tmp", std::process::id()));
        if fs::write(&temporary, bytes).is_err() || fs::rename(&temporary, entry).is_err() {
            let _ = fs::remove_file(&temporary);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_follows_its_package_files() {
        let root = std::env::temp_dir().join(format!("aim-parse-cache-{}", std::process::id()));
        let package = root.join("app");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("base.apk"), b"one").unwrap();
        let cache = Cache {
            dir: root.join("cache"),
            base: Sha256::new(),
        };
        fs::create_dir_all(&cache.dir).unwrap();
        let first = cache.entry(&package, "/system/app/app", 16).unwrap();
        assert_eq!(cache.entry(&package, "/system/app/app", 16).unwrap(), first);
        assert_ne!(cache.entry(&package, "/system/app/app", 17).unwrap(), first);
        cache.store(&first, b"parsed");
        assert_eq!(fs::read(&first).unwrap(), b"parsed");
        // A split added beside the base is another package input.
        fs::write(package.join("split.apk"), b"two").unwrap();
        assert_ne!(cache.entry(&package, "/system/app/app", 16).unwrap(), first);
        fs::remove_dir_all(&root).unwrap();
    }
}
