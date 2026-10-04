//! Image scan inputs, including the first boot with no settings (#702).
//! Scan-directory ordering and partition capabilities are ported from
//! android-16.0.0_r1 InitAppsHelper, PackagePartitions, ScanPartition and
//! InstallPackageHelper; stage filtering from PackageInstallerService.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::Error;
use crate::package::{
    parse,
    pkg::AndroidPackage,
    sign,
    write::{ApkSigningError, Apks, CertificateCollection},
};
use std::fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Partition {
    System,
    Vendor,
    Odm,
    Oem,
    Product,
    SystemExt,
    Data,
}

impl Partition {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Vendor => "vendor",
            Self::Odm => "odm",
            Self::Oem => "oem",
            Self::Product => "product",
            Self::SystemExt => "system_ext",
            Self::Data => "data",
        }
    }
}

/// The active APEX owner's inventory, in its reported order. Its origin
/// is the preinstalled partition, including for an updated active APEX.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apex {
    pub module_name: Option<String>,
    pub mount_path: String,
    pub partition: Partition,
    pub factory: bool,
    pub active_changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Overlay,
    Framework,
    PrivApp,
    App,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    pub partition: Partition,
    pub kind: Kind,
    pub apex: Option<Apex>,
}

impl Location {
    pub fn parse_flags(&self) -> i32 {
        if self.partition == Partition::Data {
            return 0;
        }
        parse::PARSE_IS_SYSTEM_DIR
            | if self.apex.is_some() {
                parse::PARSE_APK_IN_APEX
            } else {
                0
            }
    }

    pub fn privileged(&self) -> bool {
        matches!(self.kind, Kind::Framework | Kind::PrivApp)
    }
}

#[derive(Clone, Debug)]
/// Parsed code before scan identity is applied. The signing type distinguishes
/// uncollected inputs (`()`) from collected SigningDetails.
/// Record.parsed has already been renamed and must not be reused here.
pub struct Code<S = sign::SigningDetails> {
    pub location: Location,
    pub parsed: AndroidPackage,
    pub signing: S,
}

impl Code {
    /// collectCertificates sets the parsed package's collected SigningDetails
    /// before scan reconciliation can merge the saved setting's lineage.
    pub(super) fn collected_package(&self) -> Result<AndroidPackage, String> {
        let mut parsed = self.parsed.clone();
        parsed.signing_details = Some(self.signing.parcel_details()?);
        Ok(parsed)
    }
}

/// Invalid candidates remain visible to the owner. System code is preserved;
/// the data scan owner removes invalid data code after inspecting failures.
#[derive(Debug, PartialEq, Eq)]
pub struct Rejected {
    pub location: Location,
    pub reason: String,
}

#[derive(Debug)]
pub struct Image<S = sign::SigningDetails> {
    /// Scan order is significant for reconciliation and declarations;
    /// names are not deduplicated before the owner selects a package.
    pub packages: Vec<Code<S>>,
    pub rejected: Vec<Rejected>,
}

impl Image {
    /// Native parsing and full signature verification. No package feed,
    /// parser cache, settings, UID allocation or filesystem writes.
    pub fn load(apks: &Apks, apexes: &[Apex]) -> Result<Self, Error> {
        Self::load_directories(apks, directories(apexes)?, &|parsed, _| {
            apks.checked_signing_details(parsed)
                .map_err(|error| ("signatures", error))
        })
        .map(|(image, _)| image)
    }

    /// Ordered image inputs using the boot owner's certificate collection
    /// choice for each parsed candidate, before identity/reconciliation.
    pub fn load_collected<'a>(
        apks: &Apks,
        apexes: &[Apex],
        collection: &dyn Fn(
            &AndroidPackage,
            &Location,
        ) -> Result<CertificateCollection<'a>, String>,
    ) -> Result<Self, Error> {
        Self::load_directories(apks, directories(apexes)?, &|parsed, location| {
            collect(apks, parsed, location, collection)
        })
        .map(|(image, _)| image)
    }

    fn load_directories<S>(
        apks: &Apks,
        directories: Vec<Location>,
        signing: &dyn Fn(&AndroidPackage, &Location) -> Result<S, (&'static str, ApkSigningError)>,
    ) -> Result<(Image<S>, Vec<String>), Error> {
        let mut image = Image {
            packages: Vec::new(),
            rejected: Vec::new(),
        };
        let mut scan_paths = Vec::new();
        for directory in directories {
            let fail = |path: &str, phase, message| Error {
                package: String::new(),
                path: path.into(),
                phase,
                message,
            };
            let host = (apks.files)(&directory.path).ok_or_else(|| {
                fail(
                    &directory.path,
                    "location",
                    "scan directory not mapped".into(),
                )
            })?;
            let entries = match fs::read_dir(&host) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if directory.kind == Kind::Framework {
                        return Err(Error {
                            package: "android".into(),
                            path: directory.path,
                            phase: "framework",
                            message: "framework package was not loaded".into(),
                        });
                    }
                    continue;
                }
                Err(e) => return Err(fail(&directory.path, "directory", e.to_string())),
            };
            let mut candidates = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| fail(&directory.path, "directory", e.to_string()))?;
                let name = entry.file_name().into_string().map_err(|_| {
                    fail(&directory.path, "directory", "non-UTF8 package path".into())
                })?;
                if stage(&name) {
                    continue;
                }
                let metadata = fs::metadata(entry.path())
                    .map_err(|e| fail(&directory.path, "directory", e.to_string()))?;
                if name.ends_with(".apk") || metadata.is_dir() {
                    candidates.push((name, entry.path()));
                }
            }
            // Original parsing is parallel within a directory. Stable
            // path order makes a native candidate reproducible; package
            // collisions are still reconciled by the owner, not here.
            candidates.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, host) in candidates {
                let mut location = Location {
                    path: format!("{}/{name}", directory.path),
                    ..directory.clone()
                };
                let parsed = match parse::parse(
                    &host,
                    &location.path,
                    location.parse_flags(),
                    &apks.platform,
                ) {
                    Ok(parsed) => parsed,
                    Err(parse::Error::Parse(reason)) => {
                        image.rejected.push(Rejected { location, reason });
                        continue;
                    }
                    Err(parse::Error::Unsupported(reason)) => {
                        return Err(fail(
                            &location.path,
                            "parse",
                            format!("unsupported: {reason}"),
                        ));
                    }
                };
                let parsed = AndroidPackage::read_cache_entry(&parsed.to_cache_entry().bytes)
                    .map_err(|e| {
                        fail(
                            &location.path,
                            "parse",
                            format!("native parcel does not read: {e}"),
                        )
                    })?;
                let signing = match signing(&parsed, &location) {
                    Ok(signing) => signing,
                    Err((_, ApkSigningError::Invalid(error)))
                        if location.partition == Partition::Data =>
                    {
                        image.rejected.push(Rejected {
                            location,
                            reason: error.to_string(),
                        });
                        continue;
                    }
                    Err((phase, reason)) => {
                        return Err(fail(&location.path, phase, reason.to_string()));
                    }
                };
                if location.partition == Partition::Data {
                    scan_paths.push(location.path.clone());
                    location.path = parsed.path.clone().ok_or_else(|| {
                        fail(
                            &location.path,
                            "parse",
                            "parsed package has no code path".into(),
                        )
                    })?;
                }
                image.packages.push(Code {
                    location,
                    parsed,
                    signing,
                });
            }
            if directory.kind == Kind::Framework
                && !image.packages.iter().any(|code| {
                    code.location.kind == Kind::Framework && code.parsed.package_name == "android"
                })
            {
                return Err(Error {
                    package: "android".into(),
                    path: directory.path,
                    phase: "framework",
                    message: "framework package was not loaded".into(),
                });
            }
        }
        Ok((image, scan_paths))
    }
}

/// Physical data APK inventory before known-package validation or reconciliation.
/// Rejections retain the outer scan path for the removal owner.
#[derive(Debug)]
pub struct DataCode<S = sign::SigningDetails> {
    /// The outer file submitted by installPackagesFromDir, before descent.
    pub scan_path: String,
    pub code: Code<S>,
}

#[derive(Debug)]
pub struct DataImage<S = sign::SigningDetails> {
    pub packages: Vec<DataCode<S>>,
    pub rejected: Vec<Rejected>,
}

impl DataImage {
    /// Scan /data/app, then explicitly supplied mounted private volumes.
    /// Does not allocate UIDs, mutate settings or remove rejected candidates.
    pub fn load(apks: &Apks, volumes: &[String]) -> Result<Self, Error> {
        Self::load_directories(apks, volumes, &|parsed, _| {
            apks.checked_signing_details(parsed)
                .map_err(|error| ("signatures", error))
        })
    }

    pub fn load_collected<'a>(
        apks: &Apks,
        volumes: &[String],
        collection: &dyn Fn(
            &AndroidPackage,
            &Location,
        ) -> Result<CertificateCollection<'a>, String>,
    ) -> Result<Self, Error> {
        Self::load_directories(apks, volumes, &|parsed, location| {
            collect(apks, parsed, location, collection)
        })
    }
}

impl DataImage<()> {
    /// Parse data candidates without opening their signing sources. Collection
    /// follows current-setting selection in the sequential data scan owner.
    pub fn parse(apks: &Apks, volumes: &[String]) -> Result<Self, Error> {
        Self::load_directories(apks, volumes, &|_, _| Ok(()))
    }
}

impl<S> DataImage<S> {
    fn load_directories(
        apks: &Apks,
        volumes: &[String],
        signing: &dyn Fn(&AndroidPackage, &Location) -> Result<S, (&'static str, ApkSigningError)>,
    ) -> Result<Self, Error> {
        let mut roots = vec!["/data/app".to_owned()];
        for volume in volumes {
            if volume.is_empty()
                || volume.contains(['/', '\0'])
                || matches!(volume.as_str(), "." | "..")
            {
                return Err(Error {
                    package: String::new(),
                    path: volume.clone(),
                    phase: "location",
                    message: "invalid private volume name".into(),
                });
            }
            let root = format!("/mnt/expand/{volume}/app");
            if roots.contains(&root) {
                return Err(Error {
                    package: String::new(),
                    path: root,
                    phase: "location",
                    message: "duplicate private volume".into(),
                });
            }
            roots.push(root);
        }
        let (image, scan_paths) = Image::load_directories(
            apks,
            roots
                .into_iter()
                .map(|path| Location {
                    path,
                    partition: Partition::Data,
                    kind: Kind::App,
                    apex: None,
                })
                .collect(),
            signing,
        )?;
        Ok(Self {
            packages: image
                .packages
                .into_iter()
                .zip(scan_paths)
                .map(|(code, scan_path)| DataCode { scan_path, code })
                .collect(),
            rejected: image.rejected,
        })
    }
}

impl<S> Default for Image<S> {
    fn default() -> Self {
        Self {
            packages: Vec::new(),
            rejected: Vec::new(),
        }
    }
}
impl<S> Default for DataImage<S> {
    fn default() -> Self {
        Self {
            packages: Vec::new(),
            rejected: Vec::new(),
        }
    }
}

fn collect<'a>(
    apks: &Apks,
    parsed: &AndroidPackage,
    location: &Location,
    collection: &dyn Fn(&AndroidPackage, &Location) -> Result<CertificateCollection<'a>, String>,
) -> Result<sign::SigningDetails, (&'static str, ApkSigningError)> {
    let choice = collection(parsed, location)
        .map_err(|error| ("certificates", ApkSigningError::Input(error)))?;
    if choice.skip_verify != (location.partition != Partition::Data) {
        return Err((
            "certificates",
            ApkSigningError::Input("verification choice disagrees with scan partition".into()),
        ));
    }
    apks.checked_collect_signing_details(parsed, choice)
        .map_err(|error| ("signatures", error))
}

fn directories(apexes: &[Apex]) -> Result<Vec<Location>, Error> {
    let mut partitions: Vec<_> = [
        Partition::System,
        Partition::Vendor,
        Partition::Odm,
        Partition::Oem,
        Partition::Product,
        Partition::SystemExt,
    ]
    .into_iter()
    .map(|partition| (format!("/{}", partition.name()), partition, None))
    .collect();
    for apex in apexes {
        if apex.partition == Partition::Data {
            return Err(Error {
                package: String::new(),
                path: apex.mount_path.clone(),
                phase: "location",
                message: "active APEX has no preinstalled partition".into(),
            });
        }
        let name = apex.mount_path.strip_prefix("/apex/").unwrap_or_default();
        if name.is_empty() || name.contains(['/', '@']) || matches!(name, "." | "..") {
            return Err(Error {
                package: String::new(),
                path: apex.mount_path.clone(),
                phase: "location",
                message: "invalid active APEX mount path".into(),
            });
        }
        if partitions
            .iter()
            .any(|(path, _, _)| path == &apex.mount_path)
        {
            return Err(Error {
                package: String::new(),
                path: apex.mount_path.clone(),
                phase: "location",
                message: "duplicate active APEX mount path".into(),
            });
        }
        partitions.push((apex.mount_path.clone(), apex.partition, Some(apex.clone())));
    }
    let mut dirs = Vec::new();
    let mut add = |root: &str, partition, apex: &Option<Apex>, kind, folder| {
        dirs.push(Location {
            path: format!("{root}/{folder}"),
            partition,
            kind,
            apex: apex.clone(),
        });
    };
    for (root, partition, apex) in partitions.iter().rev() {
        if *partition != Partition::System {
            add(root, *partition, apex, Kind::Overlay, "overlay");
        }
    }
    add(
        "/system",
        Partition::System,
        &None,
        Kind::Framework,
        "framework",
    );
    for (root, partition, apex) in &partitions {
        if *partition != Partition::Oem {
            add(root, *partition, apex, Kind::PrivApp, "priv-app");
        }
        add(root, *partition, apex, Kind::App, "app");
    }
    Ok(dirs)
}

fn stage(name: &str) -> bool {
    ((name.starts_with("vmdl") || name.starts_with("smdl")) && name.ends_with(".tmp"))
        || name.starts_with("smdl2tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_signing_replaces_parser_details_without_mutating_code() {
        let mut code = Code {
            location: Location {
                path: "/product/app/p".into(),
                partition: Partition::Product,
                kind: Kind::App,
                apex: None,
            },
            parsed: AndroidPackage {
                feature_flag_state: Some(Vec::new()),
                uid: -1,
                signing_details: Some(Default::default()),
                ..Default::default()
            },
            signing: sign::SigningDetails {
                current_flags: Vec::new(),
                signatures: vec![vec![3]],
                scheme_version: 3,
                public_keys: vec![],
                past_signing_certificates: Some(vec![(vec![1], 15), (vec![3], 31)]),
            },
        };
        let collected = code.collected_package().unwrap();
        let loaded =
            super::super::LoadedPackage::new(collected.clone(), code.signing.clone()).unwrap();
        let mut detached = loaded.clone();
        detached
            .collected_signing
            .past_signing_certificates
            .as_mut()
            .unwrap()[0]
            .1 = 0;
        assert_eq!(
            loaded.facade_entry().unwrap().past_signing_certificates,
            code.signing.past_signing_certificates
        );
        assert_ne!(
            detached.facade_entry().unwrap().past_signing_certificates,
            loaded.facade_entry().unwrap().past_signing_certificates
        );
        let mut mismatched = collected.clone();
        mismatched.signing_details = None;
        assert!(super::super::LoadedPackage::new(mismatched, code.signing.clone()).is_err());
        let facade = collected.to_facade_entry(&code.signing).unwrap();
        assert_eq!(
            facade.past_signing_certificates,
            code.signing.past_signing_certificates
        );
        assert_eq!(
            AndroidPackage::read_cache_entry(&facade.cache.bytes)
                .unwrap()
                .signing_details,
            collected.signing_details
        );
        let mut wrong = code.signing.clone();
        wrong.signatures = vec![vec![4]];
        assert!(collected.to_facade_entry(&wrong).is_err());
        wrong = code.signing.clone();
        wrong.past_signing_certificates = None;
        assert!(collected.to_facade_entry(&wrong).is_err());
        let details = collected.signing_details.unwrap();
        assert_eq!(details.signatures, Some(vec![vec![3]]));
        assert_eq!(
            details.past_signing_certificates,
            Some(vec![vec![1], vec![3]])
        );
        assert_eq!(details.scheme_version, 3);
        assert_eq!(collected.uid, -1);
        assert_eq!(code.parsed.signing_details, Some(Default::default()));

        code.signing.public_keys.push(vec![0]);
        let before = code.parsed.clone();
        assert!(code.collected_package().is_err());
        assert_eq!(code.parsed, before);
    }

    #[test]
    fn partition_order_capabilities_and_apex_origin_are_preserved() {
        let apex = Apex {
            module_name: Some("module".into()),
            mount_path: "/apex/module".into(),
            partition: Partition::Product,
            factory: false,
            active_changed: true,
        };
        let dirs = directories(&[apex.clone()]).unwrap();
        let paths: Vec<_> = dirs.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(
            &paths[..7],
            &[
                "/apex/module/overlay",
                "/system_ext/overlay",
                "/product/overlay",
                "/oem/overlay",
                "/odm/overlay",
                "/vendor/overlay",
                "/system/framework"
            ]
        );
        assert!(dirs[6].privileged());
        assert!(!paths.contains(&"/system/overlay"));
        assert!(!paths.contains(&"/oem/priv-app"));
        assert_eq!(
            &paths[paths.len() - 2..],
            &["/apex/module/priv-app", "/apex/module/app"]
        );
        assert_eq!(dirs.last().unwrap().apex, Some(apex.clone()));
        assert_eq!(
            dirs.last().unwrap().parse_flags(),
            parse::PARSE_IS_SYSTEM_DIR | parse::PARSE_APK_IN_APEX
        );
        assert!(directories(&[apex.clone(), apex.clone()]).is_err());
        assert!(
            directories(&[Apex {
                partition: Partition::Data,
                ..apex
            }])
            .is_err()
        );
    }

    #[test]
    fn installation_stages_are_excluded_without_excluding_package_names() {
        for name in ["vmdl7.tmp", "smdl9.tmp", "smdl2tmp123"] {
            assert!(stage(name));
        }
        for name in ["vmdl.apk", "smdl.app", "other.tmp"] {
            assert!(!stage(name));
        }
    }
}
