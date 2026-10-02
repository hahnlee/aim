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
use crate::package::settings::Signatures;
use crate::package::sign::{self, Apk, Build};

/// Where the service host reads a guest path, if others may read it.
pub type Files = Box<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// The guest's files and the platform the parser and the verifier
/// depend on.
pub struct Apks {
    pub files: Files,
    pub platform: Platform,
}

impl Apks {
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
        Ok(Signatures {
            scheme_version: details.scheme_version,
            signatures: details.signatures,
            public_keys: Some(
                sign::serialize_public_keys(&details.public_keys)?
                    .into_iter()
                    .map(Some)
                    .collect(),
            ),
            past_signatures: details.past_signing_certificates,
        })
    }

    /// Native verified details retain SPKI keys for the persistence owner;
    /// `signatures` adds the query parcel's Java serialization.
    pub fn signing_details(&self, pkg: &AndroidPackage) -> Result<sign::SigningDetails, String> {
        let base = pkg
            .base_apk_path
            .as_ref()
            .ok_or("no parsed base APK path")?;
        let mut paths = vec![base.clone()];
        if let Some(splits) = &pkg.split_code_paths {
            for path in splits {
                paths.push(path.clone().ok_or("null parsed split APK path")?);
            }
        }
        let sources = paths
            .iter()
            .map(|path| {
                let host = (self.files)(path).ok_or_else(|| format!("{path}: not readable"))?;
                FileSource::open(&host).map_err(|e| format!("{path}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let apk = |i: usize| Apk {
            path: &paths[i],
            data: &sources[i],
            v4: None,
        };
        let splits: Vec<Apk> = (1..sources.len()).map(apk).collect();
        sign::package_signing_details(
            &apk(0),
            &splits,
            pkg.static_shared_library_name.is_some(),
            pkg.target_sdk_version,
            false,
            &Build::of(&self.platform),
        )
        .map_err(|e| e.to_string())
    }

    /// The package the native parser makes of the APK at `ps`'s code path,
    /// read back as the original's parcel reads.
    pub fn parsed(&self, ps: &PackageState) -> Result<AndroidPackage, String> {
        self.parsed_path(&ps.path, 0)
    }

    pub fn parsed_path(&self, path: &str, flags: i32) -> Result<AndroidPackage, String> {
        let host = (self.files)(path).ok_or_else(|| format!("{path}: not readable"))?;
        let package =
            parse::parse(&host, path, flags, &self.platform).map_err(|e| e.to_string())?;
        AndroidPackage::read_cache_entry(&package.to_cache_entry().bytes)
            .map_err(|s| format!("the parser's entry does not read: status {s}"))
    }
}

fn scan_file_time(files: &Files, pkg: &AndroidPackage) -> Result<i64, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = |path: &str| {
        let host = (files)(path).ok_or_else(|| format!("{path}: not readable"))?;
        std::fs::metadata(host).map_err(|e| format!("{path}: {e}"))
    };
    let millis = |path: &str| {
        let stat = metadata(path)?;
        stat.mtime()
            .checked_mul(1000)
            .and_then(|s| s.checked_add(stat.mtime_nsec() / 1_000_000))
            .ok_or_else(|| format!("{path}: modification time exceeds milliseconds range"))
    };
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
