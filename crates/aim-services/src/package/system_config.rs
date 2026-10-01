//! What the package queries read of the device besides the package
//! state, from the system image as the original reads it at boot:
//! SystemConfig's features and hidden-API allowlist (the `etc/sysconfig`
//! and `etc/permissions` XML of each partition and APEX,
//! `SystemConfig.readAllPermissions` at `android-16.0.0_r1`), the
//! framework's fallback app categories (`FallbackCategoryProvider`), its
//! `config_useRoundIcon` and the aconfig flags (the parser's platform),
//! and the GL ES version. Properties come from `prop`: the device's, as
//! the original reads them at run time.

use std::fs;
use std::path::Path;

use aim_android_xml::Element;
use aim_apps::apk::Apk;

use super::model::System;
use super::parse::Platform;

/// `PackageManager.FEATURE_*` that SystemConfig adds from the environment.
const FEATURE_FILE_BASED_ENCRYPTION: &str = "android.software.file_based_encryption";
const FEATURE_SECURELY_REMOVES_USERS: &str = "android.software.securely_removes_users";
const FEATURE_ADOPTABLE_STORAGE: &str = "android.software.adoptable_storage";
const FEATURE_RAM_LOW: &str = "android.hardware.ram.low";
const FEATURE_RAM_NORMAL: &str = "android.hardware.ram.normal";
const FEATURE_APP_ENUMERATION: &str = "android.software.app_enumeration";
const FEATURE_IPSEC_TUNNELS: &str = "android.software.ipsec_tunnels";
const FEATURE_IPSEC_TUNNEL_MIGRATION: &str = "android.software.ipsec_tunnel_migration";
/// `Build.VERSION_CODES.Q`, `TIRAMISU`.
const Q: i32 = 29;
const TIRAMISU: i32 = 33;

/// What a partition's files may configure (`SystemConfig.ALLOW_*`).
const ALLOW_FEATURES: u32 = 0x1;
const ALLOW_HIDDENAPI_WHITELISTING: u32 = 0x40;
const ALLOW_ALL: u32 = !0;

/// SystemConfig's features and hidden-API allowlist.
#[derive(Debug, Default, PartialEq)]
pub struct SystemConfig {
    /// `mAvailableFeatures`, as added: name and version.
    pub features: Vec<(String, i32)>,
    pub hidden_api_allowlist: Vec<String>,
    unavailable: Vec<String>,
}

impl SystemConfig {
    /// `readAllPermissions` of the image whose root is `root`.
    pub fn read(root: &Path, prop: &dyn Fn(&str) -> Option<String>) -> SystemConfig {
        let mut c = SystemConfig::default();
        let low_ram = prop("ro.config.low_ram").as_deref() == Some("true");
        let first_sdk = int(prop("ro.product.first_api_level")).unwrap_or(0);
        let mut dirs: Vec<(String, u32)> = Vec::new();
        let mut partition = |dir: &str, flags: u32, sku: Option<String>| {
            for kind in ["sysconfig", "permissions"] {
                dirs.push((format!("{dir}/etc/{kind}"), flags));
            }
            if let Some(sku) = sku.filter(|s| !s.is_empty()) {
                for kind in ["sysconfig", "permissions"] {
                    dirs.push((format!("{dir}/etc/{kind}/sku_{sku}"), flags));
                }
            }
        };
        // Vendor and ODM may add features; product and system_ext all of it
        // that matters here (the product's allowlist is a TODO upstream).
        let vendor = ALLOW_FEATURES;
        partition("system", ALLOW_ALL, None);
        partition("vendor", vendor, prop("ro.boot.product.vendor.sku"));
        partition("odm", vendor, prop("ro.boot.product.hardware.sku"));
        partition("oem", ALLOW_FEATURES, None);
        partition(
            "product",
            ALLOW_FEATURES | ALLOW_HIDDENAPI_WHITELISTING,
            prop("ro.boot.hardware.sku"),
        );
        partition("system_ext", ALLOW_ALL, None);
        for (dir, flags) in dirs {
            c.read_dir(&root.join(dir), flags, low_ram);
        }
        if let Ok(apexes) = fs::read_dir(root.join("apex")) {
            let mut apexes: Vec<_> = apexes.flatten().map(|e| e.path()).collect();
            apexes.sort();
            for apex in apexes {
                let name = apex.file_name().unwrap_or_default().to_string_lossy();
                if apex.is_dir() && !name.contains('@') {
                    c.read_dir(&apex.join("etc/permissions"), ALLOW_FEATURES, low_ram);
                }
            }
        }
        // readAllPermissionsFromEnvironment.
        if prop("ro.crypto.type").as_deref() == Some("file") {
            c.add(FEATURE_FILE_BASED_ENCRYPTION, 0);
            c.add(FEATURE_SECURELY_REMOVES_USERS, 0);
        }
        if prop("vold.has_adoptable").as_deref() == Some("1") {
            c.add(FEATURE_ADOPTABLE_STORAGE, 0);
        }
        c.add(
            if low_ram {
                FEATURE_RAM_LOW
            } else {
                FEATURE_RAM_NORMAL
            },
            0,
        );
        // The incremental (IncrementalManager.getVersion) and EROFS
        // features depend on kernel file systems the guest's kernel, AIM's
        // Linux layer, does not have.
        c.add(FEATURE_APP_ENUMERATION, 0);
        if first_sdk >= Q {
            c.add(FEATURE_IPSEC_TUNNELS, 0);
        }
        if int(prop("ro.vendor.api_level")).unwrap_or(first_sdk) > TIRAMISU {
            c.add(FEATURE_IPSEC_TUNNEL_MIGRATION, 0);
        }
        for name in std::mem::take(&mut c.unavailable) {
            c.features.retain(|(n, _)| *n != name);
        }
        c
    }

    /// `readPermissions` of a directory: its XML files, `platform.xml` of a
    /// `permissions` directory last.
    fn read_dir(&mut self, dir: &Path, flags: u32, low_ram: bool) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|f| f.is_file() && f.extension().is_some_and(|e| e == "xml"))
            .collect();
        files.sort_by_key(|f| (f.ends_with("etc/permissions/platform.xml"), f.clone()));
        for f in files {
            let Ok(bytes) = fs::read(&f) else { continue };
            // A file that does not parse adds nothing (the original keeps
            // what it read before the error).
            let Ok(root) = aim_android_xml::read(&bytes) else {
                continue;
            };
            if root.name == "permissions" || root.name == "config" {
                self.read_root(&root, flags, low_ram);
            }
        }
    }

    fn read_root(&mut self, root: &Element, flags: u32, low_ram: bool) {
        for e in root.children() {
            let name = e.string("name").map(|s| s.into_owned());
            match e.name.as_str() {
                "feature" if flags & ALLOW_FEATURES != 0 => {
                    let version = int(e.string("version").map(|s| s.into_owned())).unwrap_or(0);
                    let allowed = !low_ram || e.string("notLowRam").as_deref() != Some("true");
                    if let Some(name) = name.filter(|_| allowed) {
                        self.add(&name, version);
                    }
                }
                "unavailable-feature" if flags & ALLOW_FEATURES != 0 => {
                    if let Some(name) = name {
                        self.unavailable.push(name);
                    }
                }
                "hidden-api-whitelisted-app" if flags & ALLOW_HIDDENAPI_WHITELISTING != 0 => {
                    if let Some(package) = e.string("package")
                        && !self.hidden_api_allowlist.iter().any(|p| *p == package)
                    {
                        self.hidden_api_allowlist.push(package.into_owned());
                    }
                }
                _ => {}
            }
        }
    }

    /// `addFeature`: a feature added twice keeps its highest version.
    fn add(&mut self, name: &str, version: i32) {
        match self.features.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = (*v).max(version),
            None => self.features.push((name.to_string(), version)),
        }
    }
}

/// `XmlUtils.readIntAttribute` and `SystemProperties.getInt`.
fn int(s: Option<String>) -> Option<i32> {
    s?.trim().parse().ok()
}

/// `FallbackCategoryProvider.loadFallbacks`: framework-res's
/// `fallback_categories` (a package, a category per line).
fn fallback_categories(root: &Path, prop: &dyn Fn(&str) -> Option<String>) -> Vec<(String, i32)> {
    if prop("fw.ignore_fb_categories").as_deref() == Some("true") {
        return Vec::new();
    }
    let apk = root.join("system/framework/framework-res.apk");
    let Ok(csv) = Apk::open(&apk).and_then(|a| a.file("res/raw/fallback_categories.csv")) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&csv)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (package, category) = l.split_once(',')?;
            Some((package.to_string(), category.parse().ok()?))
        })
        .collect()
}

/// The device's side of the model's state, read from the image whose
/// root is `root` and the properties `prop`.
pub fn system(root: &Path, prop: &dyn Fn(&str) -> Option<String>) -> Result<System, String> {
    let config = SystemConfig::read(root, prop);
    let names = config.features.iter().map(|(n, _)| n.clone()).collect();
    let platform = Platform::load(root, names)?;
    let mut flags: Vec<(String, bool)> = platform.flags.into_iter().collect();
    flags.sort();
    Ok(System {
        features: config.features,
        // `FeatureInfo.GL_ES_VERSION_UNDEFINED` without the property.
        gl_es_version: int(prop("ro.opengles.version")).unwrap_or(0),
        hidden_api_allowlist: config.hidden_api_allowlist,
        use_round_icon: platform.use_round_icon,
        // Settings.Global.compatibility_mode's default; PackageManager reads
        // the setting at systemReady (#737).
        compatibility_mode: true,
        fallback_categories: fallback_categories(root, prop),
        flags,
        ..System::default()
    })
}

/// The properties of the image's build.prop files (a device's read-only
/// ones), for `prop`.
pub fn build_props(root: &Path) -> impl Fn(&str) -> Option<String> {
    let mut props = Vec::new();
    for file in [
        "system/build.prop",
        "vendor/build.prop",
        "odm/etc/build.prop",
        "product/etc/build.prop",
        "system_ext/etc/build.prop",
    ] {
        let Ok(text) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=')
                && !k.trim_start().starts_with('#')
            {
                props.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    move |name: &str| {
        props
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_features_as_system_config() {
        let root = std::env::temp_dir().join(format!("aim-sysconfig-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write(
            &root,
            "system/etc/permissions/platform.xml",
            r#"<permissions><feature name="a" version="1" /><unavailable-feature name="b" /></permissions>"#,
        );
        write(
            &root,
            "system/etc/permissions/z.xml",
            r#"<permissions><feature name="a" version="3" /><feature name="b" /><feature name="low" notLowRam="true" /></permissions>"#,
        );
        write(
            &root,
            "vendor/etc/permissions/v.xml",
            r#"<permissions><feature name="v" /><hidden-api-whitelisted-app package="not.allowed" /></permissions>"#,
        );
        write(
            &root,
            "product/etc/sysconfig/p.xml",
            r#"<config><hidden-api-whitelisted-app package="org.example" /></config>"#,
        );
        write(
            &root,
            "apex/com.x@1/etc/permissions/x.xml",
            r#"<permissions><feature name="skipped" /></permissions>"#,
        );
        write(
            &root,
            "apex/com.x/etc/permissions/x.xml",
            r#"<permissions><feature name="apex" /></permissions>"#,
        );
        let props = |name: &str| match name {
            "ro.product.first_api_level" => Some("36".to_string()),
            _ => None,
        };
        let c = SystemConfig::read(&root, &props);
        // Platform.xml is read last, so its version 1 does not lower 3; the
        // unavailable feature goes.
        let names: Vec<(&str, i32)> = c.features.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        assert_eq!(
            names,
            [
                ("a", 3),
                ("low", 0),
                ("v", 0),
                ("apex", 0),
                (FEATURE_RAM_NORMAL, 0),
                (FEATURE_APP_ENUMERATION, 0),
                (FEATURE_IPSEC_TUNNELS, 0),
                (FEATURE_IPSEC_TUNNEL_MIGRATION, 0),
            ]
        );
        assert_eq!(c.hidden_api_allowlist, ["org.example"]);

        let low_ram = |name: &str| (name == "ro.config.low_ram").then(|| "true".to_string());
        let c = SystemConfig::read(&root, &low_ram);
        assert!(
            c.features
                .iter()
                .all(|(n, _)| n != "low" && n != FEATURE_IPSEC_TUNNELS)
        );
        assert!(c.features.iter().any(|(n, _)| n == FEATURE_RAM_LOW));
        fs::remove_dir_all(root).unwrap();
    }
}
