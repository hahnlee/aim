//! What the package queries read of the device besides the package
//! state, from the system image as the original reads it at boot:
//! SystemConfig's features, libraries and hidden-API allowlist (the `etc/sysconfig`
//! and `etc/permissions` XML of each partition and APEX,
//! `SystemConfig.readAllPermissions` at `android-16.0.0_r1`), the
//! framework's fallback app categories (`FallbackCategoryProvider`), its
//! `config_useRoundIcon` and the aconfig flags (the parser's platform),
//! and the GL ES version. Properties come from `prop`: the device's, as
//! the original reads them at run time.
//!
//! The library declaration and public native list readers are ported
//! from AOSP android-16.0.0_r1 `SystemConfig`, Copyright (C) The Android
//! Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use aim_android_xml::Element;

use super::model::System;
use super::parse::Platform;

mod libraries;
pub use libraries::Library;

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
const ALLOW_LIBS: u32 = 0x2;
const ALLOW_HIDDENAPI_WHITELISTING: u32 = 0x40;
const ALLOW_ALL: u32 = !0;

/// SystemConfig's features, libraries and hidden-API allowlist.
#[derive(Debug, Default, PartialEq)]
pub struct SystemConfig {
    /// `mSharedLibraries`, including public native libraries.
    pub libraries: BTreeMap<String, Library>,
    /// `mAvailableFeatures`, as added: name and version.
    pub features: Vec<(String, i32)>,
    pub hidden_api_allowlist: Vec<String>,
    /// `mNamedActors`: namespace, actor name and package.
    pub named_actors: Vec<(String, String, String)>,
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
        // Vendor and ODM may add libraries and features; product and system_ext all of it
        // that matters here (the product's allowlist is a TODO upstream).
        let vendor = ALLOW_FEATURES | ALLOW_LIBS;
        partition("system", ALLOW_ALL, None);
        partition("vendor", vendor, prop("ro.boot.product.vendor.sku"));
        partition("odm", vendor, prop("ro.boot.product.hardware.sku"));
        partition("oem", ALLOW_FEATURES, None);
        partition(
            "product",
            ALLOW_FEATURES | ALLOW_LIBS | ALLOW_HIDDENAPI_WHITELISTING,
            prop("ro.boot.hardware.sku"),
        );
        partition("system_ext", ALLOW_ALL, None);
        for (dir, flags) in dirs {
            c.read_dir(&root.join(dir), flags, low_ram, root, prop);
        }
        if let Ok(apexes) = fs::read_dir(root.join("apex")) {
            let mut apexes: Vec<_> = apexes.flatten().map(|e| e.path()).collect();
            apexes.sort();
            for apex in apexes {
                let name = apex.file_name().unwrap_or_default().to_string_lossy();
                if apex.is_dir() && !name.contains('@') {
                    c.read_dir(
                        &apex.join("etc/permissions"),
                        ALLOW_FEATURES | ALLOW_LIBS,
                        low_ram,
                        root,
                        prop,
                    );
                }
            }
        }
        c.read_native_libraries(root);
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
    fn read_dir(
        &mut self,
        dir: &Path,
        flags: u32,
        low_ram: bool,
        image: &Path,
        prop: &dyn Fn(&str) -> Option<String>,
    ) {
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
                self.read_root(&root, flags, low_ram, image, prop);
            }
        }
    }

    fn read_root(
        &mut self,
        root: &Element,
        flags: u32,
        low_ram: bool,
        image: &Path,
        prop: &dyn Fn(&str) -> Option<String>,
    ) {
        for e in root.children() {
            let name = e.string("name").map(|s| s.into_owned());
            match e.name.as_str() {
                "library" | "apex-library" if flags & ALLOW_LIBS != 0 => {
                    if let Some(library) = Library::read(e, image, prop) {
                        self.libraries.insert(library.name.clone(), library);
                    }
                }
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
                // Any directory may name actors; a duplicate or one in the
                // android namespace stops the original's system server.
                "named-actor" => {
                    let attr = |n| {
                        e.string(n)
                            .filter(|v| !v.is_empty())
                            .map(|v| v.into_owned())
                    };
                    if let (Some(namespace), Some(actor), Some(package)) =
                        (attr("namespace"), attr("name"), attr("package"))
                    {
                        self.named_actors.push((namespace, actor, package));
                    }
                }
                _ => {}
            }
        }
    }

    fn read_native_libraries(&mut self, root: &Path) {
        let mut files = vec![root.join("vendor/etc/public.libraries.txt")];
        for dir in ["system/etc", "system_ext/etc", "product/etc"] {
            if let Ok(entries) = fs::read_dir(root.join(dir)) {
                let mut paths: Vec<_> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        let name = p.file_name().unwrap_or_default().to_string_lossy();
                        name.starts_with("public.libraries-") && name.ends_with(".txt")
                    })
                    .collect();
                paths.sort();
                files.extend(paths);
            }
        }
        for file in files {
            let Ok(text) = fs::read_to_string(file) else {
                continue;
            };
            for line in text
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                let name = line.trim().split(' ').next().unwrap().to_owned();
                self.libraries.insert(name.clone(), Library::native(name));
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

/// `FallbackCategoryProvider.loadFallbacks`: the framework's
/// `raw/fallback_categories` (a package and a category per line).
fn fallback_categories(csv: &[u8], prop: &dyn Fn(&str) -> Option<String>) -> Vec<(String, i32)> {
    if prop("fw.ignore_fb_categories").as_deref() == Some("true") {
        return Vec::new();
    }
    String::from_utf8_lossy(csv)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (package, category) = l.split_once(',')?;
            Some((package.to_string(), category.parse().ok()?))
        })
        .collect()
}

/// Properties as the device has them at run time: a property's value by
/// name.
pub type Properties = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What the image's framework fixes: its aconfig flags,
/// `config_useRoundIcon` and `raw/fallback_categories`, as its static
/// overlays leave them (the parser's platform).
pub struct Framework {
    flags: Vec<(String, bool)>,
    use_round_icon: bool,
    fallback_categories: Vec<u8>,
}

impl Framework {
    pub fn load(root: &Path) -> Result<Framework, String> {
        let platform = Platform::load(root, Default::default())?;
        let mut flags: Vec<(String, bool)> = platform.flags.clone().into_iter().collect();
        flags.sort();
        Ok(Framework {
            flags,
            use_round_icon: platform.use_round_icon,
            fallback_categories: platform
                .framework_file(root, "raw", "fallback_categories")
                .unwrap_or_default(),
        })
    }
}

/// The device's side of the model's state: SystemConfig and the
/// framework's constants of the image whose root is `root`, with the
/// properties `prop` (the device's at run time: the SKU picks permission
/// directories, init sets the GL ES version).
pub fn system(root: &Path, prop: &dyn Fn(&str) -> Option<String>, framework: &Framework) -> System {
    let config = SystemConfig::read(root, prop);
    System {
        features: config.features,
        // `FeatureInfo.GL_ES_VERSION_UNDEFINED` without the property.
        gl_es_version: int(prop("ro.opengles.version")).unwrap_or(0),
        hidden_api_allowlist: config.hidden_api_allowlist,
        named_actors: config.named_actors,
        use_round_icon: framework.use_round_icon,
        // Settings.Global.compatibility_mode's default; PackageManager reads
        // the setting at systemReady (#737).
        compatibility_mode: true,
        fallback_categories: fallback_categories(&framework.fallback_categories, prop),
        flags: framework.flags.clone(),
        ..System::default()
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
    fn reads_library_owners_sdk_limits_and_native_lists() {
        struct Data(std::path::PathBuf);
        impl Drop for Data {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let data = Data(
            std::env::temp_dir().join(format!("aim-sysconfig-libraries-{}", std::process::id())),
        );
        fs::create_dir(&data.0).unwrap();
        let root = &data.0;
        write(root, "system/framework/example.jar", "");
        write(
            root,
            "system/etc/permissions/base.xml",
            r#"<permissions>
            <library name="kept" file="/system/framework/example.jar" dependency="a:b::"
                min-device-sdk="36" max-device-sdk="36" on-bootclasspath-since="36" />
            <library name="before" file="/system/framework/example.jar" on-bootclasspath-before="37" />
            <library name="too-new" file="/system/framework/example.jar" min-device-sdk="37" />
            <library name="too-old" file="/system/framework/example.jar" max-device-sdk="35" />
            <library name="future" file="/system/framework/example.jar" min-device-sdk="Future" />
            <library name="missing" file="/system/framework/missing.jar" />
            <library name="no-file" />
            <library name="libvendor.so" file="/system/framework/example.jar" />
            </permissions>"#,
        );
        for partition in ["vendor", "odm", "product", "system_ext", "oem"] {
            write(
                root,
                &format!("{partition}/etc/permissions/lib.xml"),
                &format!(
                    "<permissions><library name='{partition}' file='/system/framework/example.jar' /></permissions>"
                ),
            );
        }
        write(root, "apex/com.x/javalib/x.jar", "");
        write(
            root,
            "apex/com.x/etc/permissions/x.xml",
            "<permissions><apex-library name='apex' file='/apex/com.x/javalib/x.jar' /></permissions>",
        );
        write(
            root,
            "apex/com.x@1/etc/permissions/x.xml",
            "<permissions><library name='versioned' file='/apex/com.x/javalib/x.jar' /></permissions>",
        );
        write(
            root,
            "vendor/etc/public.libraries.txt",
            "# comment\nlibvendor.so 64\n\n",
        );
        write(
            root,
            "product/etc/public.libraries-example.txt",
            "libproduct.so 32\n",
        );
        write(root, "system/etc/public.libraries.txt", "ignored.so\n");
        let props = |name: &str| match name {
            "ro.build.version.sdk" => Some("36".into()),
            "ro.build.version.codename" => Some("REL".into()),
            "ro.build.version.known_codenames" => Some("Baklava".into()),
            _ => None,
        };
        let libraries = SystemConfig::read(root, &props).libraries;
        assert_eq!(
            libraries.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "apex",
                "before",
                "kept",
                "libproduct.so",
                "libvendor.so",
                "odm",
                "product",
                "system_ext",
                "vendor"
            ]
        );
        assert_eq!(libraries["kept"].dependencies, ["a", "b"]);
        assert!(libraries["kept"].can_be_safely_ignored);
        assert!(libraries["before"].can_be_safely_ignored);
        assert!(libraries["libvendor.so"].native);
        assert_eq!(libraries["libvendor.so"].filename, "libvendor.so");
        assert!(!libraries["apex"].native);
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
            r#"<permissions><feature name="v" /><hidden-api-whitelisted-app package="not.allowed" /><named-actor namespace="ns" name="a" package="v.actor" /><named-actor namespace="ns" name="" package="none" /></permissions>"#,
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
        // Any partition names actors; one without a name is skipped.
        assert_eq!(
            c.named_actors,
            [("ns".to_string(), "a".to_string(), "v.actor".to_string())]
        );

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
