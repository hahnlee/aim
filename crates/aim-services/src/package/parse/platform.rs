//! The platform a parse depends on, read from a system image: the build's
//! SDK level and codenames, the aconfig flags, the split permissions
//! (`SystemConfig`), the framework's resources and the default locale.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use aim_apps::apk::Apk;

use super::attrs::{ANDROID, attr_bool, attr_value};
use super::resources::{Overlay, Resources, Table};
use super::{Platform, SplitPermission};

/// The partitions in the order `SystemConfig` reads them.
const PARTITIONS: [&str; 6] = ["system", "vendor", "odm", "oem", "product", "system_ext"];

impl Platform {
    /// The platform of the image whose root is `root`, with the system
    /// features `features`.
    pub fn load(root: &Path, features: HashSet<String>) -> Result<Platform, String> {
        let props = build_props(root);
        let prop = |k: &str| props.get(k).map(String::as_str);
        let sdk = prop("ro.build.version.sdk")
            .and_then(|s| s.parse().ok())
            .ok_or("no ro.build.version.sdk")?;
        let codenames = match prop("ro.build.version.codename") {
            Some("REL") | None => Vec::new(),
            Some(_) => prop("ro.build.version.all_codenames")
                .unwrap_or_default()
                .split(',')
                .map(str::to_owned)
                .collect(),
        };
        let locale = locale(
            prop("persist.sys.locale")
                .or(prop("ro.product.locale"))
                .unwrap_or("en-US"),
        );
        let framework_apk = root.join("system/framework/framework-res.apk");
        let framework = Apk::open(&framework_apk)
            .and_then(|a| a.file("resources.arsc"))
            .map_err(|e| format!("{}: {e}", framework_apk.display()))?;
        let framework = Table::parse(&framework).map_err(|e| e.to_string())?;
        let low_ram = prop("ro.config.low_ram") == Some("true");
        let (flag_packages, flags) = aconfig_flags(root)?;
        let mut platform = Platform {
            sdk,
            codenames,
            features,
            flag_packages,
            flags,
            split_permissions: split_permissions(root)?,
            locale,
            use_round_icon: false,
            // `ActivityTaskManager.getMaxRecentTasksStatic() / 6`.
            recents_limit: if low_ram { 36 } else { 48 } / 6,
            framework_overlays: Vec::new(),
            framework_attrs: framework.attr_ids(),
            framework,
            density_dpi: prop("ro.sf.lcd_density").and_then(|d| d.parse().ok()),
        };
        platform.framework_overlays = framework_overlays(root, &platform.framework)?;
        platform.use_round_icon = platform.framework_bool("config_useRoundIcon");
        Ok(platform)
    }

    /// A framework `bool` resource for the parser's configuration.
    fn framework_bool(&self, name: &str) -> bool {
        let Some(id) = self.framework.id("bool", name) else {
            return false;
        };
        let res = Resources {
            tables: vec![&self.framework],
            overlays: &self.framework_overlays,
            config: self.config(),
        };
        let mut v = super::resources::Selected {
            kind: super::resources::TYPE_REFERENCE,
            data: id,
            table: None,
            resid: 0,
            flags: 0,
        };
        res.resolve(&mut v);
        v.kind == super::resources::TYPE_INT_BOOLEAN && v.data != 0
    }
}

/// The static overlays of the framework, in the order `OverlayConfig`
/// gives the zygote their idmaps: by partition, then priority, then path.
/// None of this image's partitions configures its overlays
/// (`overlay-config.xml`); one that does is refused.
fn framework_overlays(root: &Path, framework: &Table) -> Result<Vec<Overlay>, String> {
    let mut found = Vec::new();
    for (rank, p) in PARTITIONS.iter().enumerate() {
        if root.join(p).join("overlay/config/config.xml").exists() {
            return Err(format!("{p}: an overlay configuration is not supported"));
        }
        let dir = root.join(p).join("overlay");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut apks = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                if let Ok(inner) = fs::read_dir(&path) {
                    apks.extend(inner.flatten().map(|i| i.path()));
                }
            } else {
                apks.push(path);
            }
        }
        for path in apks {
            if path.extension().is_none_or(|e| e != "apk") {
                continue;
            }
            let apk = Apk::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let manifest = apk
                .manifest()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let Some(overlay) = manifest.children.iter().find(|c| c.name == "overlay") else {
                continue;
            };
            if attr_value(overlay, ANDROID, "targetPackage").as_deref() != Some("android")
                || !attr_bool(overlay, ANDROID, "isStatic", false)
            {
                continue;
            }
            let priority = overlay
                .attrs
                .iter()
                .find(|a| a.ns == ANDROID && a.name == "priority")
                .map_or(0, |a| a.data as i32);
            let table = apk
                .file("resources.arsc")
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let table = Table::parse(&table).map_err(|e| format!("{}: {e}", path.display()))?;
            found.push((rank, priority, path, table));
        }
    }
    found.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    Ok(found
        .into_iter()
        .map(|(_, _, _, t)| Overlay::new(framework, t))
        .collect())
}

/// A BCP 47 tag's language and region.
pub fn locale(tag: &str) -> ([u8; 2], [u8; 2]) {
    let mut parts = tag.split(['-', '_']);
    let two = |s: Option<&str>| {
        let b = s.unwrap_or_default().as_bytes();
        [
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
        ]
    };
    (two(parts.next()), two(parts.next()))
}

/// The build properties of every partition.
fn build_props(root: &Path) -> HashMap<String, String> {
    let mut props = HashMap::new();
    for file in [
        "system/build.prop",
        "vendor/build.prop",
        "product/etc/build.prop",
        "system_ext/etc/build.prop",
    ] {
        let Ok(text) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=')
                && !k.starts_with('#')
            {
                props
                    .entry(k.trim().to_owned())
                    .or_insert(v.trim().to_owned());
            }
        }
    }
    props
}

/// A storage file's string: its length, then its bytes.
fn storage_string(b: &[u8], at: &mut usize) -> Option<String> {
    let n = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?) as usize;
    let s = std::str::from_utf8(b.get(*at + 4..*at + 4 + n)?).ok()?;
    *at += 4 + n;
    Some(s.to_owned())
}

fn storage_u32(b: &[u8], at: &mut usize) -> Option<u32> {
    let v = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
    *at += 4;
    Some(v)
}

fn storage_u16(b: &[u8], at: &mut usize) -> Option<u16> {
    let v = u16::from_le_bytes(b.get(*at..*at + 2)?.try_into().ok()?);
    *at += 2;
    Some(v)
}

/// A storage file's header: its version and where its nodes (or, in a
/// value file, its values) start.
fn storage_header(b: &[u8]) -> Option<(u32, usize)> {
    let mut at = 0;
    let version = storage_u32(b, &mut at)?;
    storage_string(b, &mut at)?;
    let kind = *b.get(at)?;
    at += 1 + 4 + 4; // the type, the file's size, the count
    if kind != 2 {
        at += 4; // the buckets' offset
    }
    Some((version, storage_u32(b, &mut at)? as usize))
}

/// One container's flags (aconfig storage: `package.map`, `flag.map`,
/// `flag.val` of `etc/aconfig` or an APEX's `etc`).
fn read_container(
    dir: &Path,
    packages: &mut HashSet<String>,
    flags: &mut HashMap<String, bool>,
) -> Result<(), String> {
    let read = |name: &str| fs::read(dir.join(name)).ok();
    let (Some(pmap), Some(fmap), Some(vals)) =
        (read("package.map"), read("flag.map"), read("flag.val"))
    else {
        return Ok(());
    };
    let mut parse = || -> Option<()> {
        let (version, mut at) = storage_header(&pmap)?;
        // Package nodes: name, id, fingerprint (version 2), first boolean.
        let mut by_id = HashMap::new();
        while at < pmap.len() {
            let name = storage_string(&pmap, &mut at)?;
            let id = storage_u32(&pmap, &mut at)?;
            if version >= 2 {
                at += 8;
            }
            let start = storage_u32(&pmap, &mut at)?;
            storage_u32(&pmap, &mut at)?;
            packages.insert(name.clone());
            by_id.insert(id, (name, start));
        }
        let (_, values) = storage_header(&vals)?;
        let (_, mut at) = storage_header(&fmap)?;
        // Flag nodes: package id, name, type, index.
        while at < fmap.len() {
            let package = storage_u32(&fmap, &mut at)?;
            let name = storage_string(&fmap, &mut at)?;
            storage_u16(&fmap, &mut at)?;
            let index = storage_u16(&fmap, &mut at)? as usize;
            storage_u32(&fmap, &mut at)?;
            let (pkg, start) = by_id.get(&package)?;
            let v = *vals.get(values + *start as usize + index)?;
            flags.insert(format!("{pkg}.{name}"), v != 0);
        }
        Some(())
    };
    parse().ok_or_else(|| format!("{}: malformed aconfig storage", dir.display()))
}

/// The aconfig flags' values and the packages that declare flags, as the
/// new flag storage holds them for every container.
fn aconfig_flags(root: &Path) -> Result<(HashSet<String>, HashMap<String, bool>), String> {
    let mut packages = HashSet::new();
    let mut flags = HashMap::new();
    for p in PARTITIONS {
        read_container(&root.join(p).join("etc/aconfig"), &mut packages, &mut flags)?;
    }
    if let Ok(apexes) = fs::read_dir(root.join("apex")) {
        for a in apexes.flatten() {
            read_container(&a.path().join("etc"), &mut packages, &mut flags)?;
        }
    }
    Ok((packages, flags))
}

/// `SystemConfig`'s split permissions, in the order it reads them:
/// partition by partition, a directory's `platform.xml` last.
fn split_permissions(root: &Path) -> Result<Vec<SplitPermission>, String> {
    let mut out = Vec::new();
    for p in PARTITIONS {
        let dir = root.join(p).join("etc/permissions");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut files: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.retain(|f| f.extension().is_some_and(|e| e == "xml"));
        files.sort_by_key(|f| f.file_name() == Some("platform.xml".as_ref()));
        for f in files {
            // `readPermissionsFromXml` skips a file it cannot read.
            let Ok(text) = fs::read(&f) else { continue };
            if !text.windows(16).any(|w| w == b"split-permission") {
                continue;
            }
            let root = aim_android_xml::read(&text).map_err(|e| format!("{}: {e}", f.display()))?;
            for s in root.children().filter(|c| c.name == "split-permission") {
                let name = s.string("name").map(|s| s.into_owned()).unwrap_or_default();
                // Without a level, every level (`CUR_DEVELOPMENT + 1`).
                let target_sdk = s
                    .string("targetSdk")
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(10_001);
                let new_permissions = s
                    .children()
                    .filter(|c| c.name == "new-permission")
                    .filter_map(|c| c.string("name").map(|n| n.into_owned()))
                    .collect();
                out.push(SplitPermission {
                    name,
                    new_permissions,
                    target_sdk,
                });
            }
        }
    }
    Ok(out)
}
