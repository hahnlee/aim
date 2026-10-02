//! Image scan inputs, including the first boot with no settings (#702).
//! Scan-directory ordering and partition capabilities are ported from
//! android-16.0.0_r1 InitAppsHelper, PackagePartitions, ScanPartition and
//! InstallPackageHelper; stage filtering from PackageInstallerService.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::Error;
use crate::package::{parse, pkg::AndroidPackage, sign, write::Apks};
use std::fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Partition {
    System,
    Vendor,
    Odm,
    Oem,
    Product,
    SystemExt,
}

impl Partition {
    fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Vendor => "vendor",
            Self::Odm => "odm",
            Self::Oem => "oem",
            Self::Product => "product",
            Self::SystemExt => "system_ext",
        }
    }
}

/// The active APEX owner's inventory, in its reported order. Its origin
/// is the preinstalled partition, including for an updated active APEX.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apex {
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

#[derive(Debug)]
pub struct Code {
    pub location: Location,
    pub parsed: AndroidPackage,
    pub signing: sign::SigningDetails,
}

/// Invalid system-directory candidates remain visible to the owner, as
/// the original records parse failures without deleting system code.
#[derive(Debug, PartialEq, Eq)]
pub struct Rejected {
    pub location: Location,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Image {
    /// Scan order is significant for reconciliation and declarations;
    /// names are not deduplicated before the owner selects a package.
    pub packages: Vec<Code>,
    pub rejected: Vec<Rejected>,
}

impl Image {
    /// Native parsing and full signature verification. No package feed,
    /// parser cache, settings, UID allocation or filesystem writes.
    pub fn load(apks: &Apks, apexes: &[Apex]) -> Result<Self, Error> {
        let mut image = Self::default();
        for directory in directories(apexes)? {
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
                let location = Location {
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
                let signing = apks
                    .signing_details(&parsed)
                    .map_err(|e| fail(&location.path, "signatures", e))?;
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
        Ok(image)
    }
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
    fn partition_order_capabilities_and_apex_origin_are_preserved() {
        let apex = Apex {
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
        assert!(directories(&[apex.clone(), apex]).is_err());
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
