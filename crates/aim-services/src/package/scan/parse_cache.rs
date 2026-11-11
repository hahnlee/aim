//! PackageCacher keys and pre-reconciliation parcels, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
//! The boot owner prepares /data/system/package_cache/<partition fingerprint>.
//! A native sidecar binds each parcel to image/parser/configuration and APK
//! stamps (including splits, since the host does not have dm-verity mounts).
//! Beside the image, a cache that every data directory booting it shares
//! holds the same parcels by their keys, so a new data directory's first
//! boot parses only what no boot of this image and parser parsed (#2984).
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
    /// `package-cache/<image>-<parser>` beside the image root.
    shared: Option<PathBuf>,
    base: Sha256,
}

pub(super) struct Entry {
    path: PathBuf,
    shared: Option<PathBuf>,
    stamp: [u8; 32],
}

impl Cache {
    /// No prepared directory or image identity means a cold parse.
    pub fn open(apks: &Apks, directory: Option<&Path>) -> Option<Self> {
        let cache = Self::existing(apks, directory?)?;
        fs::create_dir_all(&cache.dir).ok()?;
        if let Some(shared) = &cache.shared {
            // Another image's or parser's entries are never read again.
            if let (Some(parent), Some(name)) = (shared.parent(), shared.file_name()) {
                for stale in fs::read_dir(parent).into_iter().flatten().flatten() {
                    if stale.file_name() != name {
                        let _ = fs::remove_dir_all(stale.path());
                    }
                }
            }
            let _ = fs::create_dir_all(shared);
        }
        Some(cache)
    }

    /// The cache `dir` holds or will hold, read without being made: its
    /// entries are found only once the boot owner prepared it.
    pub fn existing(apks: &Apks, dir: &Path) -> Option<Self> {
        let dir = dir.to_owned();
        let image = fs::read((apks.files)("/.identity")?).ok()?;
        // The parser is this executable's code, named by its build's UUID,
        // whichever copy of it runs.
        let parser = executable_uuid(&std::env::current_exe().ok()?)?;
        let shared = (apks.files)("/")
            .and_then(|root| Some(root.parent()?.join("package-cache")))
            .map(|cache| {
                let image = Sha256::digest(&image);
                cache.join(format!("{}-{}", hex(&image[..8]), hex(&parser)))
            });
        let p = &apks.platform;
        let mut base = Sha256::new();
        base.update(b"AIMPARSE3\0");
        base.update(&image);
        base.update(parser);
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
        Some(Self { dir, shared, base })
    }

    pub fn entry(&self, host: &Path, path: &str, flags: i32) -> Option<Entry> {
        let mut key = self.base.clone();
        key.update(format!("{path}\0{flags}\0"));
        stamps(host, &mut key, 0)?;
        let name = Path::new(path).file_name()?.to_str()?;
        let stamp: [u8; 32] = key.finalize().into();
        Some(Entry {
            // PackageCacher.getCacheKey uses the guest absolute path, not the
            // host mount path (which can differ between boots).
            path: self.dir.join(format!("{name}-{flags}-{}", java_hash(path))),
            shared: self.shared.as_ref().map(|shared| shared.join(hex(&stamp))),
            stamp,
        })
    }

    /// Whether `entry` keys the same inputs as `other`.
    pub fn same(entry: &Entry, other: &Entry) -> bool {
        entry.stamp == other.stamp
    }

    /// The data directory's parcel, else the shared one.
    pub fn read(&self, entry: &Entry, path: &str) -> Option<AndroidPackage> {
        std::iter::once(&entry.path)
            .chain(&entry.shared)
            .find_map(|file| read(file, &entry.stamp, path))
    }

    /// Sidecar includes the parcel digest: interruption between the two atomic
    /// replacements can only cause a miss, never pair old bytes with new stamps.
    /// The data directory's parcel takes its directory's policy; the shared
    /// one is the host's.
    pub fn store(&self, entry: &Entry, bytes: &[u8]) {
        let mut receipt = entry.stamp.to_vec();
        receipt.extend_from_slice(&Sha256::digest(bytes));
        for (file, adopt) in
            std::iter::once((&entry.path, true)).chain(entry.shared.iter().map(|f| (f, false)))
        {
            if atomic_write(file, bytes, adopt).is_ok() {
                let _ = atomic_write(&sidecar(file), &receipt, adopt);
            }
        }
    }
}

fn read(file: &Path, stamp: &[u8; 32], path: &str) -> Option<AndroidPackage> {
    let receipt = fs::read(sidecar(file)).ok()?;
    if receipt.len() != 64 || receipt[..32] != *stamp {
        return None;
    }
    let bytes = fs::read(file).ok()?;
    if receipt[32..] != Sha256::digest(&bytes)[..] {
        return None;
    }
    let parsed = AndroidPackage::read_cache_entry(&bytes).ok()?;
    (parsed.path.as_deref() == Some(path)).then_some(parsed)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `LC_UUID` of the Mach-O executable at `path`, which the linker
/// derives from its contents.
fn executable_uuid(path: &Path) -> Option<[u8; 16]> {
    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const LC_UUID: u32 = 0x1b;
    let mut header = vec![0; 32];
    let mut file = fs::File::open(path).ok()?;
    std::io::Read::read_exact(&mut file, &mut header).ok()?;
    let word =
        |bytes: &[u8], at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if word(&header, 0)? != MH_MAGIC_64 {
        return None;
    }
    let (count, size) = (word(&header, 16)?, word(&header, 20)? as usize);
    let mut commands = vec![0; size];
    std::io::Read::read_exact(&mut file, &mut commands).ok()?;
    let mut at = 0;
    for _ in 0..count {
        let (command, length) = (word(&commands, at)?, word(&commands, at + 4)? as usize);
        if command == LC_UUID {
            return commands.get(at + 8..at + 24)?.try_into().ok();
        }
        at = at.checked_add(length).filter(|_| length >= 8)?;
    }
    None
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

fn atomic_write(path: &Path, bytes: &[u8], adopt: bool) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    // Written host-side: a parcel under /data and the directories made for
    // it take their encrypted directory's policy (ADR 0016) before it is in
    // place.
    let result = fs::write(&tmp, bytes)
        .and_then(|()| match adopt {
            true => aim_storage::fscrypt::adopt(&tmp),
            false => Ok(()),
        })
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parser key names the code, not the file: a copy of this
    /// executable has its UUID.
    #[test]
    fn the_parser_is_named_by_its_executables_uuid() {
        let exe = std::env::current_exe().unwrap();
        let uuid = executable_uuid(&exe).unwrap();
        let root = crate::app_host::test_temp_dir("parser-uuid");
        let copy = root.join("copy");
        fs::copy(&exe, &copy).unwrap();
        assert_eq!(executable_uuid(&copy), Some(uuid));
        fs::write(&copy, b"not an executable").unwrap();
        assert_eq!(executable_uuid(&copy), None);
        fs::remove_dir_all(root).unwrap();
    }

    /// A new data directory reads the parcels another one's boot stored
    /// beside the image, without writing its own; the shared cache of
    /// another image or parser is removed.
    #[test]
    fn data_directories_share_the_parcels_beside_the_image() {
        let Some(image) = aim_paths::input(aim_paths::derived_image(), "derived-image") else {
            return;
        };
        let root = crate::app_host::test_temp_dir("parse-shared");
        let system = root.join("image/root");
        let host = system.join("system/app/app");
        fs::create_dir_all(&host).unwrap();
        fs::write(host.join("base.apk"), b"one").unwrap();
        fs::write(system.join(".identity"), b"fixture image").unwrap();
        let stale = root.join("image/package-cache/another");
        fs::create_dir_all(&stale).unwrap();
        let files = system.clone();
        let apks = Apks {
            platform: crate::package::parse::Platform::load(&image, Default::default()).unwrap(),
            signing_overrides: None,
            files: Box::new(move |guest| Some(files.join(guest.trim_start_matches('/')))),
        };
        let path = "/system/app/app";
        let first = Cache::open(&apks, Some(&root.join("one"))).unwrap();
        assert!(!stale.exists());
        let bytes = AndroidPackage {
            path: Some(path.into()),
            feature_flag_state: Some(vec![]),
            ..Default::default()
        }
        .to_cache_entry()
        .unwrap()
        .bytes;
        first.store(&first.entry(&host, path, 16).unwrap(), &bytes);
        let second = Cache::open(&apks, Some(&root.join("two"))).unwrap();
        let entry = second.entry(&host, path, 16).unwrap();
        assert_eq!(
            second.read(&entry, path),
            Some(AndroidPackage::read_cache_entry(&bytes).unwrap())
        );
        assert!(!entry.path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// A parcel written in an encrypted package_cache takes its policy, as
    /// the directory made for it does.
    #[test]
    fn parcels_take_the_policy_of_the_cache() {
        use std::os::unix::ffi::OsStrExt;
        let root = std::env::temp_dir().join(format!("aim-cache-fbe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let context = [&[2u8, 1, 4, 2, 0, 0, 0, 0][..], &[7; 16], &[0; 16]].concat();
        let name = std::ffi::CString::new(root.as_os_str().as_bytes()).unwrap();
        let name = name.as_ptr();
        let key = aim_storage::fscrypt::CONTEXT.as_ptr();
        let set = unsafe { libc::setxattr(name, key, context.as_ptr().cast(), 40, 0, 0) };
        assert_eq!(set, 0);
        let parcel = root.join("fingerprint/app-0-1");
        fs::create_dir(parcel.parent().unwrap()).unwrap();
        atomic_write(&parcel, b"parcel", true).unwrap();
        for made in [parcel.parent().unwrap(), &parcel] {
            let own = aim_storage::fscrypt::context(made).unwrap().unwrap();
            assert_eq!(own[..24], context[..24], "{}", made.display());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keys_stamps_splits_and_torn_receipts() {
        let root = std::env::temp_dir().join(format!("aim-cache-unit-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let package = root.join("app");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("base.apk"), b"one").unwrap();
        let cache = Cache {
            dir: root.clone(),
            shared: None,
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
