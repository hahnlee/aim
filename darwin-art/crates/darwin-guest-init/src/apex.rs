//! `/apex/apex-info-list.xml` for the pre-flattened APEX tree (apexd's
//! role: ADR 0012 item 6, "APEXes are pre-flattened into the derived
//! image").
//!
//! Each `/apex/<name>` directory with an `apex_manifest.pb` is an active
//! APEX. The package it came from is looked up in the partitions' `apex`
//! directories to give the `partition` and `modulePath` apexd reports;
//! init uses the partition to run vendor APEX scripts in the vendor
//! subcontext and linkerconfig reads the whole list.

use std::fmt::Write as _;

use darwin_android_init::ImageRoot;

use crate::props::ProtoReader;

/// Partitions apexd scans, with the name it reports (`ApexPartition`).
const PARTITIONS: &[(&str, &str)] = &[
    ("/system/apex", "SYSTEM"),
    ("/system_ext/apex", "SYSTEM_EXT"),
    ("/product/apex", "PRODUCT"),
    ("/vendor/apex", "VENDOR"),
    ("/odm/apex", "ODM"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApexEntry {
    pub module_name: String,
    pub module_path: String,
    pub version_code: i64,
    pub version_name: String,
    pub partition: String,
}

/// `ApexManifest` fields 1 (`name`), 2 (`version`) and 5 (`versionName`).
pub fn parse_apex_manifest(bytes: &[u8]) -> Result<(String, i64, String), String> {
    let mut reader = ProtoReader { bytes, at: 0 };
    let (mut name, mut version, mut version_name) = (String::new(), 0i64, String::new());
    while let Some((field, wire)) = reader.key()? {
        match (field, wire) {
            (1, 2) => name = String::from_utf8_lossy(reader.bytes()?).into_owned(),
            (2, 0) => version = reader.varint()? as i64,
            (5, 2) => version_name = String::from_utf8_lossy(reader.bytes()?).into_owned(),
            _ => reader.skip(wire)?,
        }
    }
    Ok((name, version, version_name))
}

/// Whether a package file name belongs to the module: `com.android.art`
/// matches `com.android.art.apex`, `com.android.art-1234.apex` and the
/// Google-signed `com.google.android.art.capex`.
#[cfg(test)]
fn package_matches(module: &str, file: &str) -> bool {
    package_match_quality(module, file).is_some()
}

/// 0 for an exact name, 1 for a suffixed variant (`-<version>`,
/// `.nonsecure`), `None` otherwise.
fn package_match_quality(module: &str, file: &str) -> Option<u8> {
    let stem = file
        .strip_suffix(".capex")
        .or_else(|| file.strip_suffix(".apex"))?;
    let candidates = [
        module.to_string(),
        module.replacen("com.android.", "com.google.android.", 1),
    ];
    let mut best = None;
    for name in &candidates {
        if stem == name {
            return Some(0);
        }
        if stem.starts_with(&format!("{name}-"))
            || stem.starts_with(&format!("{name}_"))
            || stem.starts_with(&format!("{name}."))
        {
            best = Some(1);
        }
    }
    best
}

/// Scans the image's flattened `/apex`.
pub fn scan(image: &ImageRoot) -> Vec<ApexEntry> {
    let mut packages: Vec<(String, &str)> = Vec::new();
    for (dir, partition) in PARTITIONS {
        if let Ok(files) = image.regular_files(dir) {
            for file in files {
                packages.push((file, partition));
            }
        }
    }
    let mut out = Vec::new();
    let mut names = image.subdirectories("/apex").unwrap_or_default();
    names.sort();
    for dir in names {
        let Ok(bytes) = image.read(&format!("/apex/{dir}/apex_manifest.pb")) else {
            continue;
        };
        let Ok((name, version_code, version_name)) = parse_apex_manifest(&bytes) else {
            continue;
        };
        let module_name = if name.is_empty() { dir.clone() } else { name };
        let package = packages
            .iter()
            .filter_map(|(path, partition)| {
                package_match_quality(&module_name, path.rsplit('/').next().unwrap_or(""))
                    .map(|quality| (quality, path, partition))
            })
            .min_by_key(|(quality, path, _)| (*quality, path.len()))
            .map(|(_, path, partition)| (path, partition));
        let (module_path, partition) = match package {
            Some((path, partition)) => (path.clone(), partition.to_string()),
            None => (format!("/apex/{dir}"), "SYSTEM".to_string()),
        };
        out.push(ApexEntry {
            module_name,
            module_path,
            version_code,
            version_name,
            partition,
        });
    }
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// apexd's `apex-info-list.xml` (`com::android::apex::write`).
pub fn apex_info_list_xml(entries: &[ApexEntry]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<apex-info-list>\n");
    for entry in entries {
        let _ = writeln!(
            out,
            "    <apex-info moduleName=\"{}\" modulePath=\"{}\" preinstalledModulePath=\"{}\" versionCode=\"{}\" versionName=\"{}\" isFactory=\"true\" isActive=\"true\" provideSharedApexLibs=\"false\" partition=\"{}\"/>",
            escape(&entry.module_name),
            escape(&entry.module_path),
            escape(&entry.module_path),
            entry.version_code,
            escape(&entry.version_name),
            entry.partition
        );
    }
    out.push_str("</apex-info-list>\n");
    out
}

/// The vendor and odm APEXes, which init's script loader needs.
pub fn vendor_apexes(entries: &[ApexEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.partition == "VENDOR" || e.partition == "ODM")
        .map(|e| e.module_name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use darwin_android_init::rc::read_apex_info_list;

    #[test]
    fn manifest_and_packages() {
        let manifest = [
            0x0a, 0x0f, b'c', b'o', b'm', b'.', b'a', b'n', b'd', b'r', b'o', b'i', b'd', b'.',
            b'a', b'r', b't', 0x10, 0x9f, 0x96, 0xf3, 0xab, 0x01,
        ];
        let (name, version, _) = parse_apex_manifest(&manifest).unwrap();
        assert_eq!(name, "com.android.art");
        assert_eq!(version, 360_499_999);
        assert!(package_matches(
            "com.android.art",
            "com.google.android.art.capex"
        ));
        assert_eq!(
            package_match_quality("com.android.media", "com.google.android.media.capex"),
            Some(0)
        );
        assert_eq!(
            package_match_quality(
                "com.android.media",
                "com.google.android.media.swcodec.capex"
            ),
            Some(1)
        );
        assert!(package_matches(
            "com.google.android.widevine",
            "com.google.android.widevine-13130248.apex"
        ));
        assert!(package_matches(
            "com.android.hardware.gatekeeper",
            "com.android.hardware.gatekeeper.nonsecure.apex"
        ));
        assert!(!package_matches("com.android.art", "com.android.artx.apex"));
        let xml = apex_info_list_xml(&[ApexEntry {
            module_name: "com.android.hardware.power".into(),
            module_path: "/vendor/apex/com.android.hardware.power.apex".into(),
            version_code: 1,
            version_name: String::new(),
            partition: "VENDOR".into(),
        }]);
        let parsed = read_apex_info_list(&xml);
        assert_eq!(parsed[0].module_name, "com.android.hardware.power");
        assert_eq!(parsed[0].partition, "VENDOR");
        assert!(parsed[0].is_active);
    }
}
