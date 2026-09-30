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
use std::fs;
use std::io;
use std::path::Path;

use aim_android_init::ImageRoot;

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
    /// apexd activates it in bootstrap mode, before init's early-init
    /// `perform_apex_config --bootstrap`.
    pub vendor_bootstrap: bool,
}

/// The `ApexManifest` fields apexd reports or acts on.
#[derive(Debug, Default)]
pub struct Manifest {
    /// Field 1.
    pub name: String,
    /// Field 2.
    pub version: i64,
    /// Field 5.
    pub version_name: String,
    /// Field 15, `vendorBootstrap`.
    pub vendor_bootstrap: bool,
}

pub fn parse_apex_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let mut reader = ProtoReader { bytes, at: 0 };
    let mut manifest = Manifest::default();
    while let Some((field, wire)) = reader.key()? {
        match (field, wire) {
            (1, 2) => manifest.name = String::from_utf8_lossy(reader.bytes()?).into_owned(),
            (2, 0) => manifest.version = reader.varint()? as i64,
            (5, 2) => manifest.version_name = String::from_utf8_lossy(reader.bytes()?).into_owned(),
            (15, 0) => manifest.vendor_bootstrap = reader.varint()? != 0,
            _ => reader.skip(wire)?,
        }
    }
    Ok(manifest)
}

/// Whether a package file name belongs to the module: `com.android.art`
/// matches `com.android.art.apex`, `com.android.art-1234.apex` and the
/// Google-signed `com.google.android.art.capex`.
#[cfg(test)]
fn package_matches(module: &str, file: &str) -> bool {
    package_match_quality(module, file).is_some()
}

/// 0 for an exact name, 1 for a suffixed variant (`-<version>`,
/// `.nonsecure`, or a format generation such as Google's
/// `com.google.android.tzdata6`), `None` otherwise.
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
        let Some(rest) = stem.strip_prefix(name.as_str()) else {
            continue;
        };
        if rest.starts_with(['-', '_', '.'])
            || (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
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
        let Ok(manifest) = parse_apex_manifest(&bytes) else {
            continue;
        };
        let module_name = if manifest.name.is_empty() {
            dir.clone()
        } else {
            manifest.name
        };
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
            version_code: manifest.version,
            version_name: manifest.version_name,
            partition,
            vendor_bootstrap: manifest.vendor_bootstrap,
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

/// The APEXes whose scripts `perform_apex_config --bootstrap` loads: the
/// vendor APEXes apexd activates in bootstrap mode (`vendorBootstrap`, for
/// example the gatekeeper HAL of class `early_hal`). apexd's own bootstrap
/// list (runtime, i18n, tzdata) carries no scripts.
pub fn bootstrap_apexes(entries: &[ApexEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.vendor_bootstrap)
        .map(|e| e.module_name.clone())
        .collect()
}

/// apexd's bootstrap mode: the `vendorBootstrap` APEXes as `/bootstrap-apex`
/// shows them, where libvintf reads the vendor VINTF fragments (the info
/// list and `<name>/etc/vintf`) until `apex.all.ready`. Without them
/// servicemanager refuses the `early_hal` HALs' `addService` (the
/// gatekeeper HAL aborted, and system_server, which needs gatekeeperd
/// behind it, crashed when it came first: #205, #490). `dir` is the host
/// directory of `/bootstrap-apex`; each APEX is a link to its flattened
/// `/apex/<name>`.
pub fn write_bootstrap(dir: &Path, entries: &[ApexEntry]) -> io::Result<()> {
    let bootstrap: Vec<ApexEntry> = entries
        .iter()
        .filter(|e| e.vendor_bootstrap)
        .cloned()
        .collect();
    fs::write(
        dir.join("apex-info-list.xml"),
        apex_info_list_xml(&bootstrap),
    )?;
    for entry in &bootstrap {
        std::os::unix::fs::symlink(
            format!("/apex/{}", entry.module_name),
            dir.join(&entry.module_name),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_android_init::rc::read_apex_info_list;

    #[test]
    fn manifest_and_packages() {
        let manifest = [
            0x0a, 0x0f, b'c', b'o', b'm', b'.', b'a', b'n', b'd', b'r', b'o', b'i', b'd', b'.',
            b'a', b'r', b't', 0x10, 0x9f, 0x96, 0xf3, 0xab, 0x01,
        ];
        let parsed = parse_apex_manifest(&manifest).unwrap();
        assert_eq!(parsed.name, "com.android.art");
        assert_eq!(parsed.version, 360_499_999);
        assert!(!parsed.vendor_bootstrap);
        // com.android.hardware.gatekeeper's: version 1, vendorBootstrap.
        let gatekeeper = [0x0a, 0x02, b'g', b'k', 0x10, 0x01, 0x78, 0x01];
        let parsed = parse_apex_manifest(&gatekeeper).unwrap();
        assert!(parsed.vendor_bootstrap && parsed.version == 1);
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
        assert_eq!(
            package_match_quality("com.android.tzdata", "com.google.android.tzdata6.apex"),
            Some(1)
        );
        let xml = apex_info_list_xml(&[ApexEntry {
            module_name: "com.android.hardware.power".into(),
            module_path: "/vendor/apex/com.android.hardware.power.apex".into(),
            version_code: 1,
            version_name: String::new(),
            partition: "VENDOR".into(),
            vendor_bootstrap: false,
        }]);
        let parsed = read_apex_info_list(&xml);
        assert_eq!(parsed[0].module_name, "com.android.hardware.power");
        assert_eq!(parsed[0].partition, "VENDOR");
        assert!(parsed[0].is_active);
    }

    #[test]
    fn bootstrap_lists_and_links_vendor_bootstrap_apexes() {
        let entry = |name: &str, vendor_bootstrap| ApexEntry {
            module_name: name.into(),
            module_path: format!("/vendor/apex/{name}.apex"),
            version_code: 1,
            version_name: String::new(),
            partition: "VENDOR".into(),
            vendor_bootstrap,
        };
        let dir = std::env::temp_dir().join(format!("gi-bootstrap-apex-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        write_bootstrap(
            &dir,
            &[
                entry("com.android.hardware.gatekeeper", true),
                entry("com.android.hardware.power", false),
            ],
        )
        .unwrap();
        let parsed =
            read_apex_info_list(&fs::read_to_string(dir.join("apex-info-list.xml")).unwrap());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].module_name, "com.android.hardware.gatekeeper");
        assert_eq!(
            fs::read_link(dir.join("com.android.hardware.gatekeeper")).unwrap(),
            Path::new("/apex/com.android.hardware.gatekeeper")
        );
        assert!(!dir.join("com.android.hardware.power").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
