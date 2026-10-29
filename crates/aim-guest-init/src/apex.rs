//! `/apex/apex-info-list.xml` for the pre-flattened APEX tree (apexd's
//! role: ADR 0012 item 6, "APEXes are pre-flattened into the derived
//! image").
//!
//! Each `/apex/<name>` directory with an `apex_manifest.pb` is an active
//! APEX. The package it came from is the partitions' `apex` file whose own
//! `apex_manifest.pb` names the module, the highest version if several do,
//! as apexd reads each package (`ApexFile::Open`). It gives the
//! `partition` and `modulePath` apexd reports; init uses the partition to
//! run vendor APEX scripts in the vendor subcontext and linkerconfig reads
//! the whole list.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Seek as _, SeekFrom};
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

const MANIFEST_ENTRY: &[u8] = b"apex_manifest.pb";

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// The `apex_manifest.pb` of an APEX or compressed APEX package, a zip
/// file with the manifest at its top level (stored or deflated).
fn package_manifest(path: &Path) -> io::Result<Manifest> {
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    // The end of central directory record, at most a comment's 64 KiB
    // from the end.
    let tail_len = size.min(22 + 0xffff);
    file.seek(SeekFrom::Start(size - tail_len))?;
    let mut tail = vec![0; tail_len as usize];
    file.read_exact(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| u32_at(&tail, at) == Some(0x0605_4b50))
        .ok_or_else(|| bad("no end of central directory"))?;
    let (entries, dir_size, dir_offset) = (
        u16_at(&tail, end + 10).unwrap_or(0),
        u32_at(&tail, end + 12).unwrap_or(0),
        u32_at(&tail, end + 16).unwrap_or(0),
    );
    file.seek(SeekFrom::Start(dir_offset.into()))?;
    let mut dir = vec![0; dir_size as usize];
    file.read_exact(&mut dir)?;
    let mut at = 0;
    for _ in 0..entries {
        if u32_at(&dir, at) != Some(0x0201_4b50) {
            return Err(bad("bad central directory"));
        }
        let field = |offset| u16_at(&dir, at + offset).unwrap_or(0) as usize;
        let (method, name_len) = (field(10), field(28));
        let next = at + 46 + name_len + field(30) + field(32);
        if dir.get(at + 46..at + 46 + name_len) == Some(MANIFEST_ENTRY) {
            let compressed = u32_at(&dir, at + 20).unwrap_or(0) as usize;
            let local = u32_at(&dir, at + 42).unwrap_or(0) as u64;
            let mut header = [0; 30];
            file.seek(SeekFrom::Start(local))?;
            file.read_exact(&mut header)?;
            if u32_at(&header, 0) != Some(0x0403_4b50) {
                return Err(bad("bad local header"));
            }
            let skip =
                u16_at(&header, 26).unwrap_or(0) as u64 + u16_at(&header, 28).unwrap_or(0) as u64;
            file.seek(SeekFrom::Start(local + 30 + skip))?;
            let mut data = vec![0; compressed];
            file.read_exact(&mut data)?;
            let bytes = match method {
                0 => data,
                8 => {
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut out)?;
                    out
                }
                _ => return Err(bad("unsupported compression")),
            };
            return parse_apex_manifest(&bytes).map_err(|e| bad(&e));
        }
        at = next;
    }
    Err(bad("no apex_manifest.pb"))
}

/// Scans the image's flattened `/apex`.
pub fn scan(image: &ImageRoot) -> Vec<ApexEntry> {
    // Each package with its manifest's name and version; one that cannot
    // be read is not a package apexd would activate.
    let mut packages: Vec<(String, i64, String, &str)> = Vec::new();
    for (dir, partition) in PARTITIONS {
        if let Ok(files) = image.regular_files(dir) {
            for file in files {
                if !file.ends_with(".apex") && !file.ends_with(".capex") {
                    continue;
                }
                if let Ok(m) = package_manifest(&image.host_path(&file)) {
                    packages.push((m.name, m.version, file, partition));
                }
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
        // The highest version; of equal ones, the first partition's.
        let package = packages
            .iter()
            .filter(|(name, ..)| *name == module_name)
            .rev()
            .max_by_key(|(_, version, ..)| *version)
            .map(|(_, _, path, partition)| (path, partition));
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

    /// A zip of `entries` (name, data, deflated), as an APEX package is.
    fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        use std::io::Write as _;
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data, deflate) in entries {
            let body = if *deflate {
                let mut e =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend(0x0403_4b50u32.to_le_bytes());
            out.extend([20, 0, 0, 0]);
            out.extend(method.to_le_bytes());
            out.extend([0; 8]);
            out.extend((body.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(3u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend([1, 2, 3]);
            out.extend(&body);
            dir.extend(0x0201_4b50u32.to_le_bytes());
            dir.extend([20, 0, 20, 0, 0, 0]);
            dir.extend(method.to_le_bytes());
            dir.extend([0; 8]);
            dir.extend((body.len() as u32).to_le_bytes());
            dir.extend((data.len() as u32).to_le_bytes());
            dir.extend((name.len() as u16).to_le_bytes());
            dir.extend([0; 12]);
            dir.extend(offset.to_le_bytes());
            dir.extend(name.as_bytes());
        }
        let dir_offset = out.len() as u32;
        out.extend(&dir);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0; 4]);
        out.extend((entries.len() as u16).to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        out.extend((dir.len() as u32).to_le_bytes());
        out.extend(dir_offset.to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(b"zz");
        out
    }

    /// `ApexManifest` with a name and a version.
    fn manifest(name: &str, version: u8) -> Vec<u8> {
        let mut m = vec![0x0a, name.len() as u8];
        m.extend(name.as_bytes());
        m.extend([0x10, version]);
        m
    }

    /// Packages are matched by their manifests, whatever their file names:
    /// Google's `com.google.android.tzdata6.apex` is `com.android.tzdata`.
    #[test]
    fn packages_are_matched_by_their_manifests() {
        let root = std::env::temp_dir().join(format!("gi-apex-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |guest: &str, bytes: &[u8]| {
            let path = root.join(guest.trim_start_matches('/'));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        };
        let tzdata = manifest("com.android.tzdata", 6);
        write("/apex/com.android.tzdata/apex_manifest.pb", &tzdata);
        write(
            "/system/apex/com.google.android.tzdata6.apex",
            &zip(&[
                ("apex_payload.img", b"payload", false),
                ("apex_manifest.pb", &tzdata, true),
            ]),
        );
        let art = manifest("com.android.art", 2);
        write("/apex/com.android.art/apex_manifest.pb", &art);
        write(
            "/system/apex/com.google.android.art.capex",
            &zip(&[
                ("apex_manifest.pb", &art, false),
                ("original_apex", b"apex", false),
            ]),
        );
        // A name-alike that is another module, and an older art.
        write(
            "/system/apex/com.android.art.apex",
            &zip(&[("apex_manifest.pb", &manifest("com.android.art", 1), true)]),
        );
        write(
            "/vendor/apex/com.android.artx.apex",
            &zip(&[("apex_manifest.pb", &manifest("com.android.artx", 9), true)]),
        );
        write("/system/apex/broken.apex", b"not a zip");
        let entries = scan(&ImageRoot::new(&root));
        let path = |name: &str| {
            entries
                .iter()
                .find(|e| e.module_name == name)
                .map(|e| (e.module_path.clone(), e.partition.clone()))
        };
        assert_eq!(
            path("com.android.tzdata"),
            Some((
                "/system/apex/com.google.android.tzdata6.apex".into(),
                "SYSTEM".into()
            ))
        );
        assert_eq!(
            path("com.android.art"),
            Some((
                "/system/apex/com.google.android.art.capex".into(),
                "SYSTEM".into()
            ))
        );
        fs::remove_dir_all(&root).unwrap();
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
