//! PackageCacher keys and pre-reconciliation parcels, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
//! The boot owner prepares /data/system/package_cache/<partition fingerprint>.
//! A native sidecar binds each parcel to image/parser/configuration and APK
//! stamps (including splits, since the host does not have dm-verity mounts).
use super::super::{parse::parcel::java_hash, pkg::AndroidPackage, write::Apks};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) struct Cache {
    dir: PathBuf,
    base: Sha256,
}

pub(super) struct Entry {
    path: PathBuf,
    stamp: [u8; 32],
}

impl Cache {
    /// No prepared directory or image identity means a cold parse.
    pub fn open(apks: &Apks, directory: Option<&Path>) -> Option<Self> {
        let dir = directory?.to_owned();
        let image = fs::read((apks.files)("/.identity")?).ok()?;
        let exe = fs::metadata(std::env::current_exe().ok()?).ok()?;
        let p = &apks.platform;
        let mut base = Sha256::new();
        base.update(b"AIMPARSE2\0");
        base.update(&image);
        base.update(format!(
            "{}:{}:{}:{}\0",
            exe.ino(),
            exe.size(),
            exe.mtime(),
            exe.mtime_nsec()
        ));
        let mut features: Vec<_> = p.features.iter().collect();
        features.sort();
        let mut extensions: Vec<_> = p.sdk_extensions.iter().flatten().collect();
        extensions.sort();
        let mut flags: Vec<_> = p.flags.iter().collect();
        flags.sort();
        let mut packages: Vec<_> = p.flag_packages.iter().collect();
        packages.sort();
        base.update(format!("{features:?}{extensions:?}{flags:?}{packages:?}"));
        base.update(format!(
            "{}:{:?}:{:?}:{}:{}:{:?}",
            p.sdk, p.codenames, p.locale, p.use_round_icon, p.recents_limit, p.density_dpi
        ));
        for split in &p.split_permissions {
            base.update(format!(
                "{:?}:{:?}:{}\0",
                split.name, split.new_permissions, split.target_sdk
            ));
        }
        fs::create_dir_all(&dir).ok()?;
        Some(Self { dir, base })
    }

    pub fn entry(&self, host: &Path, path: &str, flags: i32) -> Option<Entry> {
        let mut key = self.base.clone();
        key.update(format!("{path}\0{flags}\0"));
        stamps(host, &mut key, 0)?;
        let name = Path::new(path).file_name()?.to_str()?;
        Some(Entry {
            // PackageCacher.getCacheKey uses the guest absolute path, not the
            // host mount path (which can differ between boots).
            path: self.dir.join(format!("{name}-{flags}-{}", java_hash(path))),
            stamp: key.finalize().into(),
        })
    }

    pub fn read(&self, entry: &Entry, path: &str) -> Option<AndroidPackage> {
        let receipt = fs::read(sidecar(&entry.path)).ok()?;
        if receipt.len() != 64 || receipt[..32] != entry.stamp {
            return None;
        }
        let bytes = fs::read(&entry.path).ok()?;
        if receipt[32..] != Sha256::digest(&bytes)[..] {
            return None;
        }
        let parsed = AndroidPackage::read_cache_entry(&bytes).ok()?;
        (parsed.path.as_deref() == Some(path)).then_some(parsed)
    }

    /// Sidecar includes the parcel digest: interruption between the two atomic
    /// replacements can only cause a miss, never pair old bytes with new stamps.
    pub fn store(&self, entry: &Entry, bytes: &[u8]) {
        let mut receipt = entry.stamp.to_vec();
        receipt.extend_from_slice(&Sha256::digest(bytes));
        if atomic_write(&entry.path, bytes).is_ok() {
            let _ = atomic_write(&sidecar(&entry.path), &receipt);
        }
    }
}

fn stamps(path: &Path, key: &mut Sha256, depth: usize) -> Option<()> {
    let meta = fs::metadata(path).ok()?;
    key.update(format!(
        "{}:{}:{}\0",
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec()
    ));
    if meta.is_dir() {
        // Match parser descent, ignoring unrelated oat/lib subdirectories.
        if depth > 16 {
            return None;
        }
        let mut children = fs::read_dir(path)
            .ok()?
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        children.sort_by_key(|entry| entry.file_name());
        if let [one] = children.as_slice()
            && one.path().is_dir()
        {
            key.update(format!("{:?}\0", one.file_name()));
            stamps(&one.path(), key, depth + 1)?;
        } else {
            for entry in children {
                if entry.path().extension().is_some_and(|ext| ext == "apk") {
                    key.update(format!("{:?}\0", entry.file_name()));
                    stamps(&entry.path(), key, depth + 1)?;
                }
            }
        }
    }
    Some(())
}

fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".aim");
    name.into()
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keys_stamps_splits_and_torn_receipts() {
        let root = std::env::temp_dir().join(format!("aim-cache-unit-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let package = root.join("app");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("base.apk"), b"one").unwrap();
        let cache = Cache {
            dir: root.clone(),
            base: Sha256::new(),
        };
        let path = "/system/app/app";
        let entry = cache.entry(&package, path, 16).unwrap();
        assert_eq!(
            entry.path.file_name().unwrap(),
            format!("app-16-{}", java_hash(path)).as_str()
        );
        assert_ne!(entry.path, cache.entry(&package, path, 17).unwrap().path);
        let pkg = AndroidPackage {
            path: Some(path.into()),
            feature_flag_state: Some(vec![]),
            ..Default::default()
        };
        let bytes = pkg.to_cache_entry().unwrap().bytes;
        cache.store(&entry, &bytes);
        assert_eq!(
            cache.read(&entry, path),
            Some(AndroidPackage::read_cache_entry(&bytes).unwrap())
        );
        assert!(cache.read(&entry, "/another/path").is_none());
        // A timestamp change alone invalidates a same-size APK replacement.
        let base = package.join("base.apk");
        let time = fs::metadata(&base).unwrap().modified().unwrap();
        fs::write(&base, b"two").unwrap();
        fs::File::open(&base)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time + std::time::Duration::from_secs(1)))
            .unwrap();
        let changed_time = cache.entry(&package, path, 16).unwrap();
        assert_ne!(entry.stamp, changed_time.stamp);
        assert!(cache.read(&changed_time, path).is_none());
        fs::write(&base, b"one").unwrap();
        fs::File::open(&base)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
        assert_eq!(entry.stamp, cache.entry(&package, path, 16).unwrap().stamp);
        fs::write(package.join("split.apk"), b"two").unwrap();
        let changed = cache.entry(&package, path, 16).unwrap();
        assert_eq!(entry.path, changed.path);
        assert_ne!(entry.stamp, changed.stamp);
        assert!(cache.read(&changed, path).is_none());
        cache.store(&changed, &bytes);
        fs::write(&changed.path, b"torn parcel").unwrap();
        assert!(cache.read(&changed, path).is_none());
        cache.store(&changed, &bytes);
        fs::write(sidecar(&changed.path), b"torn receipt").unwrap();
        assert!(cache.read(&changed, path).is_none());
        fs::remove_file(package.join("split.apk")).unwrap();
        fs::write(package.join("base.apk"), b"changed").unwrap();
        assert_ne!(entry.stamp, cache.entry(&package, path, 16).unwrap().stamp);
        fs::remove_dir_all(root).unwrap();
    }
}
