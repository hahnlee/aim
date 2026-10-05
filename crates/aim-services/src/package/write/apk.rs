//! An installed package's APKs, read from the guest's files: their
//! signers, verified as the original's `ApkSignatureVerifier` verifies an
//! install (`package::sign`), and the package the native parser makes of
//! them (`package::parse`), for install checks and native scan inputs.
//! Scan timestamps port PackageManagerServiceUtils at android-16.0.0_r1,
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::PathBuf;

use android_image_extract::source::FileSource;

use crate::package::model::PackageState;
use crate::package::parse::{self, Platform};
use crate::package::pkg::AndroidPackage;
use crate::package::settings::{Package, Signatures};
use crate::package::sign::{self, Apk, Build};

/// Where the service host reads a guest path, if others may read it.
pub type Files = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// The guest's files and the platform the parser and the verifier
/// depend on.
pub struct Apks {
    pub files: Files,
    pub platform: Platform,
}

/// Keep failed source ownership separate from a verifier's APK rejection.
#[derive(Debug)]
pub(crate) enum ApkSigningError {
    Input(String),
    Invalid(sign::Error),
}
impl std::fmt::Display for ApkSigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(e) => f.write_str(e),
            Self::Invalid(e) => e.fmt(f),
        }
    }
}

/// ScanPackageUtils.collectCertificatesLI inputs from the scan owner.
#[derive(Clone, Copy)]
pub struct CertificateCollection<'a> {
    pub saved: Option<&'a Package>,
    pub database_version: i32,
    pub force_collect: bool,
    /// Only verified system partitions may skip APK content verification.
    pub skip_verify: bool,
    pub pre_n_mr1_upgrade: bool,
}

impl Apks {
    /// PackageManagerServiceUtils.compressedFileExists: the sibling of a
    /// code directory ending in -Stub contains an entry ending in .gz.
    pub fn scan_compressed_files_exist(&self, pkg: &AndroidPackage) -> Result<bool, String> {
        scan_compressed_files_exist(&self.files, pkg)
    }
    /// PackageManagerServiceUtils.getLastModifiedTime for available scan code:
    /// a monolithic path's timestamp, or the maximum base/split APK timestamp.
    /// Read errors reject the candidate; code removal belongs to reconciliation.
    pub fn scan_file_time(&self, pkg: &AndroidPackage) -> Result<i64, String> {
        scan_file_time(&self.files, pkg)
    }

    /// The signers of the parsed package's APK paths: its base
    /// APK's, which each split shares (`getSigningDetails`, verified in
    /// full).
    pub fn signatures(&self, pkg: &AndroidPackage) -> Result<Signatures, String> {
        let details = self.signing_details(pkg)?;
        let public_keys = details.serialized_public_keys()?;
        Ok(Signatures {
            scheme_version: details.scheme_version,
            signatures: details.signatures,
            current_flags: details.current_flags,
            public_keys,
            past_signatures: details.past_signing_certificates,
        })
    }

    /// Native verified details retain SPKI keys for the persistence owner;
    /// `signatures` adds the query parcel's Java serialization.
    pub fn signing_details(&self, pkg: &AndroidPackage) -> Result<sign::SigningDetails, String> {
        self.checked_signing_details(pkg).map_err(|e| e.to_string())
    }

    /// Collect code signing before scan reconciliation, reusing saved signing
    /// only under the pinned path/time/version gates. This does not authorize
    /// installation or replace the ordered reconciliation owner.
    pub fn collect_signing_details(
        &self,
        pkg: &AndroidPackage,
        collection: CertificateCollection<'_>,
    ) -> Result<sign::SigningDetails, String> {
        self.checked_collect_signing_details(pkg, collection)
            .map_err(|e| e.to_string())
    }

    pub(crate) fn checked_collect_signing_details(
        &self,
        pkg: &AndroidPackage,
        collection: CertificateCollection<'_>,
    ) -> Result<sign::SigningDetails, ApkSigningError> {
        self.collect_signing_details_using(pkg, collection, None)
    }

    pub fn collect_signing_details_with_overrides(
        &self,
        pkg: &AndroidPackage,
        collection: CertificateCollection<'_>,
        overrides: &sign::Overrides,
    ) -> Result<sign::SigningDetails, String> {
        self.collect_signing_details_using(pkg, collection, Some(overrides))
            .map_err(|e| e.to_string())
    }

    fn collect_signing_details_using(
        &self,
        pkg: &AndroidPackage,
        collection: CertificateCollection<'_>,
        overrides: Option<&sign::Overrides>,
    ) -> Result<sign::SigningDetails, ApkSigningError> {
        let modified = collection_file_time(&self.files, pkg, collection.pre_n_mr1_upgrade)
            .map_err(ApkSigningError::Input)?;
        if let Some(saved) = collection.saved
            && !collection.force_collect
            && pkg.path.as_deref() == Some(saved.code_path.as_str())
            && saved.last_modified_time == modified
            // SIGNATURE_END_ENTITY=2; SIGNATURE_MALFORMED_RECOVER=3.
            && collection.database_version >= 3
            && let Some(signing) = &saved.signatures
            && !signing.signatures.is_empty()
            && signing.scheme_version != sign::UNKNOWN
        {
            return sign::SigningDetails::from_saved(signing).map_err(ApkSigningError::Input);
        }
        self.signing_details_with_verification(pkg, collection.skip_verify, overrides)
    }

    pub(crate) fn checked_signing_details(
        &self,
        pkg: &AndroidPackage,
    ) -> Result<sign::SigningDetails, ApkSigningError> {
        self.signing_details_with_verification(pkg, false, None)
    }

    fn signing_details_with_verification(
        &self,
        pkg: &AndroidPackage,
        skip_verify: bool,
        overrides: Option<&sign::Overrides>,
    ) -> Result<sign::SigningDetails, ApkSigningError> {
        let base = pkg
            .base_apk_path
            .as_ref()
            .ok_or_else(|| ApkSigningError::Input("no parsed base APK path".into()))?;
        let mut paths = vec![base.clone()];
        if let Some(splits) = &pkg.split_code_paths {
            for path in splits {
                paths.push(
                    path.clone().ok_or_else(|| {
                        ApkSigningError::Input("null parsed split APK path".into())
                    })?,
                );
            }
        }
        let sources = paths
            .iter()
            .map(|path| {
                let host = (self.files)(path)
                    .ok_or_else(|| ApkSigningError::Input(format!("{path}: not mapped")))?;
                // ApkSignatureVerifier catches source IO as a package parse
                // failure. A native mapping failure remains an owner error.
                FileSource::open(&host).map_err(|e| {
                    ApkSigningError::Invalid(sign::Error {
                        code: sign::INSTALL_PARSE_FAILED_NO_CERTIFICATES,
                        message: format!("Failed to collect certificates from {path}: {e}"),
                    })
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let apk = |i: usize| Apk {
            path: &paths[i],
            data: &sources[i],
            v4: None,
        };
        let splits: Vec<Apk> = (1..sources.len()).map(apk).collect();
        let build = Build::of(&self.platform);
        let result = match overrides {
            Some(owner) => sign::package_signing_details_with_overrides(
                &apk(0),
                &splits,
                pkg.static_shared_library_name.is_some(),
                pkg.target_sdk_version,
                skip_verify,
                &build,
                owner,
            ),
            None => sign::package_signing_details(
                &apk(0),
                &splits,
                pkg.static_shared_library_name.is_some(),
                pkg.target_sdk_version,
                skip_verify,
                &build,
            ),
        };
        result.map_err(ApkSigningError::Invalid)
    }

    /// The package the native parser makes of the APK at `ps`'s code path,
    /// read back as the original's parcel reads.
    pub fn parsed(&self, ps: &PackageState) -> Result<AndroidPackage, String> {
        self.parsed_path(&ps.path, 0)
    }

    pub fn parsed_path(&self, path: &str, flags: i32) -> Result<AndroidPackage, String> {
        self.checked_parsed_path(path, flags)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn checked_parsed_path(
        &self,
        path: &str,
        flags: i32,
    ) -> Result<AndroidPackage, parse::Error> {
        let host = (self.files)(path)
            .ok_or_else(|| parse::Error::Unsupported(format!("{path}: not mapped")))?;
        let package = parse::parse(&host, path, flags, &self.platform)?;
        AndroidPackage::read_cache_entry(&package.to_cache_entry().bytes).map_err(|s| {
            parse::Error::Unsupported(format!("the parser's entry does not read: status {s}"))
        })
    }
}

fn scan_compressed_files_exist(files: &Files, pkg: &AndroidPackage) -> Result<bool, String> {
    let path = std::path::Path::new(pkg.path.as_deref().ok_or("no parsed package path")?);
    let Some(name) = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix("-Stub"))
    else {
        return Ok(false);
    };
    let parent = path.parent().ok_or("no stub parent directory")?;
    let compressed = parent.join(name);
    let guest = compressed.to_str().ok_or("non-UTF8 compressed path")?;
    let host = files(guest).ok_or_else(|| format!("{guest}: not readable"))?;
    let entries = match std::fs::read_dir(host) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{guest}: {e}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("{guest}: {e}"))?;
        if entry
            .file_name()
            .to_string_lossy()
            .to_lowercase()
            .ends_with(".gz")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// File.lastModified returns zero on stat failure; a missing native path
// mapping is an owner configuration failure, not that guest API result.
fn collection_file_time(files: &Files, pkg: &AndroidPackage, legacy: bool) -> Result<i64, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = |path: &str| {
        let host = files(path).ok_or_else(|| format!("{path}: not readable"))?;
        Ok::<_, String>(std::fs::metadata(host).ok())
    };
    let millis = |path: &str| {
        let Some(stat) = metadata(path)? else {
            return Ok(0);
        };
        stat.mtime()
            .checked_mul(1000)
            .and_then(|s| s.checked_add(stat.mtime_nsec() / 1_000_000))
            .ok_or_else(|| format!("{path}: modification time exceeds milliseconds range"))
    };
    let path = pkg.path.as_deref().ok_or("no parsed package path")?;
    if legacy || !metadata(path)?.is_some_and(|m| m.is_dir()) {
        return millis(path);
    }
    let base = pkg
        .base_apk_path
        .as_deref()
        .ok_or("no parsed base APK path")?;
    let mut latest = millis(base)?;
    for split in pkg.split_code_paths.iter().flatten() {
        latest = latest.max(millis(
            split.as_deref().ok_or("null parsed split APK path")?,
        )?);
    }
    Ok(latest)
}

fn file_time(files: &Files, path: &str) -> Result<i64, String> {
    use std::os::unix::fs::MetadataExt;
    let host = files(path).ok_or_else(|| format!("{path}: not readable"))?;
    let stat = std::fs::metadata(host).map_err(|e| format!("{path}: {e}"))?;
    stat.mtime()
        .checked_mul(1000)
        .and_then(|s| s.checked_add(stat.mtime_nsec() / 1_000_000))
        .ok_or_else(|| format!("{path}: modification time exceeds milliseconds range"))
}

fn scan_file_time(files: &Files, pkg: &AndroidPackage) -> Result<i64, String> {
    let metadata = |path: &str| {
        let host = (files)(path).ok_or_else(|| format!("{path}: not readable"))?;
        std::fs::metadata(host).map_err(|e| format!("{path}: {e}"))
    };
    let millis = |path: &str| file_time(files, path);
    let path = pkg.path.as_deref().ok_or("no parsed package path")?;
    let code = metadata(path)?;
    if !code.is_dir() {
        return millis(path);
    }
    let base = pkg
        .base_apk_path
        .as_deref()
        .ok_or("no parsed base APK path")?;
    let mut latest = millis(base)?;
    for split in pkg.split_code_paths.iter().flatten() {
        latest = latest.max(millis(
            split.as_deref().ok_or("null parsed split APK path")?,
        )?);
    }
    Ok(latest)
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;
    use std::fs::{self, File, FileTimes};
    use std::time::{Duration, UNIX_EPOCH};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn set_time(path: &std::path::Path, time: std::time::SystemTime) {
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(time))
            .unwrap();
    }
    #[test]
    fn compressed_inventory_uses_stub_sibling_and_case_insensitive_extension() {
        let dir = std::env::temp_dir().join(format!("aim-compressed-unit-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let fixture = Fixture(dir);
        let root = fixture.0.clone();
        let files: Files = Box::new(move |p| Some(root.join(p.trim_start_matches('/'))));
        let mut pkg = AndroidPackage {
            path: Some("/Module-Stub".into()),
            ..Default::default()
        };
        assert!(!scan_compressed_files_exist(&files, &pkg).unwrap());
        fs::create_dir(fixture.0.join("Module")).unwrap();
        fs::write(fixture.0.join("Module/ignored.apk"), []).unwrap();
        assert!(!scan_compressed_files_exist(&files, &pkg).unwrap());
        fs::write(fixture.0.join("Module/base.GZ"), []).unwrap();
        assert!(scan_compressed_files_exist(&files, &pkg).unwrap());
        pkg.path = Some("/Module-Stub/base.apk".into());
        assert!(!scan_compressed_files_exist(&files, &pkg).unwrap());
        pkg.path = Some("/Module-Stub-extra".into());
        assert!(!scan_compressed_files_exist(&files, &pkg).unwrap());
        pkg.path = Some("/Module-Stub".into());
        let unreadable: Files = Box::new(|_| None);
        assert!(scan_compressed_files_exist(&unreadable, &pkg).is_err());
        fs::remove_file(fixture.0.join("Module/base.GZ")).unwrap();
        fs::remove_file(fixture.0.join("Module/ignored.apk")).unwrap();
        fs::remove_dir(fixture.0.join("Module")).unwrap();
        fs::write(fixture.0.join("Module"), []).unwrap();
        assert!(scan_compressed_files_exist(&files, &pkg).is_err());
    }

    #[test]
    fn file_times_use_code_path_or_latest_apk_and_report_missing_inputs() {
        let dir = std::env::temp_dir().join(format!("aim-code-time-unit-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let fixture = Fixture(dir);
        let cluster = fixture.0.join("cluster");
        fs::create_dir(&cluster).unwrap();
        for name in ["mono.apk", "cluster/base.apk", "cluster/split.apk"] {
            fs::write(fixture.0.join(name), []).unwrap();
        }
        set_time(
            &fixture.0.join("mono.apk"),
            UNIX_EPOCH - Duration::from_nanos(500_000_001),
        );
        set_time(
            &cluster.join("base.apk"),
            UNIX_EPOCH + Duration::from_millis(1000),
        );
        set_time(
            &cluster.join("split.apk"),
            UNIX_EPOCH + Duration::from_millis(2000),
        );
        set_time(&cluster, UNIX_EPOCH + Duration::from_millis(40000));
        let root = fixture.0.clone();
        let files: Files = Box::new(move |p| Some(root.join(p.trim_start_matches('/'))));
        let mut pkg = AndroidPackage {
            path: Some("/mono.apk".into()),
            base_apk_path: Some("/not-used.apk".into()),
            ..Default::default()
        };
        assert_eq!(scan_file_time(&files, &pkg).unwrap(), -501);
        pkg.path = Some("/cluster".into());
        pkg.base_apk_path = Some("/cluster/base.apk".into());
        pkg.split_code_paths = Some(vec![Some("/cluster/split.apk".into())]);
        assert_eq!(scan_file_time(&files, &pkg).unwrap(), 2000);
        pkg.split_code_paths = None;
        assert_eq!(scan_file_time(&files, &pkg).unwrap(), 1000);
        pkg.split_code_paths = Some(vec![None]);
        assert!(
            scan_file_time(&files, &pkg)
                .unwrap_err()
                .contains("null parsed split")
        );
        pkg.split_code_paths = Some(vec![Some("/missing.apk".into())]);
        assert!(
            scan_file_time(&files, &pkg)
                .unwrap_err()
                .contains("/missing.apk")
        );
        pkg.path = None;
        assert!(
            scan_file_time(&files, &pkg)
                .unwrap_err()
                .contains("package path")
        );
    }
}
