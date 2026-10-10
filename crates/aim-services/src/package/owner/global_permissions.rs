//! LegacyPermissionSettings.writePermissions/writePermissionTrees at android-16.0.0_r1.
//! Ports AOSP LegacyPermission.write, Apache License 2.0.
use super::{attribute, element};
use crate::package::settings::{Permission, PermissionOwner, Settings};
use aim_android_xml::{Element, Node, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn persisted(values: &[Permission]) -> Vec<Permission> {
    values.iter().cloned().map(|mut value| {
        // Config UID/GIDs belong to SystemConfig, not LegacyPermission.write.
        value.owner = if value.dynamic.is_some() { PermissionOwner::Dynamic } else { PermissionOwner::Manifest };
        value
    }).collect()
}

pub(super) fn replace(original: &Element, desired: &Settings) -> Result<Element, String> {
    let mut root = original.clone();
    for (tag, values) in [("permission-trees", &desired.permission_trees), ("permissions", &desired.permissions)] {
        let sections = original.children().filter(|e| e.name == tag).collect::<Vec<_>>();
        if sections.len() > 1 { return Err(format!("duplicate {tag} owner")); }
        let mut old = BTreeMap::new();
        if let Some(section) = sections.first() {
            for item in section.children().filter(|e| e.name == "item") {
                let name = item.string("name").ok_or_else(|| format!("{tag} item missing identity"))?.into_owned();
                if old.insert(name, item).is_some() { return Err(format!("duplicate {tag} definition")); }
            }
        }
        let mut section = sections.first().map(|e| (*e).clone()).unwrap_or_else(|| element(tag));
        section.content.retain(|n| !matches!(n, Node::Element(e) if e.name == "item"));
        let mut seen = BTreeSet::new();
        for value in values {
            if !seen.insert(&value.name) { return Err(format!("duplicate desired {tag} definition")); }
            let mut item = old.get(&value.name).map(|e| (*e).clone()).unwrap_or_else(|| element("item"));
            for key in ["name", "package", "protection", "type", "icon", "label"] {
                attribute(&mut item, key, None);
            }
            attribute(&mut item, "name", Some(Value::String(value.name.clone())));
            attribute(&mut item, "package", Some(Value::String(value.package.clone())));
            if value.protection_level != 0 { attribute(&mut item, "protection", Some(Value::Int(value.protection_level))); }
            if let Some((icon, label)) = &value.dynamic {
                attribute(&mut item, "type", Some(Value::String("dynamic".into())));
                if *icon != 0 { attribute(&mut item, "icon", Some(Value::Int(*icon))); }
                attribute(&mut item, "label", label.clone().map(Value::String));
            }
            section.content.push(Node::Element(item));
        }
        root.content.retain(|n| !matches!(n, Node::Element(e) if e.name == tag));
        root.content.push(Node::Element(section));
    }
    let parsed = Settings::parse(&root)?;
    if parsed.permissions != persisted(&desired.permissions)
        || parsed.permission_trees != persisted(&desired.permission_trees) {
        return Err("permission definition owner does not round trip".into());
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_and_restored_definitions_preserve_extension_owners_and_dynamic_fields() {
        let input = aim_android_xml::read(b"<packages vendor='yes'><permissions future='section'><item name='p.NORMAL' package='old' protection='2' type='dynamic' icon='9' label='old' future='item'><vendor/></item><future/></permissions><vendor-global/></packages>").unwrap();
        let defs = aim_android_xml::read(b"<packages><permissions><item name='p.NORMAL' package='p'/><item name='p.DYNAMIC' package='p' type='dynamic' protection='2' icon='12' label='label'/></permissions><permission-trees><item name='p.TREE' package='p' protection='1'/></permission-trees></packages>").unwrap();
        let desired = Settings::parse(&defs).unwrap();
        for original in [&input, &element("packages")] {
            let written = replace(original, &desired).unwrap();
            let restored = Settings::parse(&written).unwrap();
            assert_eq!(restored.permissions, desired.permissions);
            assert_eq!(restored.permission_trees, desired.permission_trees);
            assert_eq!(replace(&written, &restored).unwrap(), written);
        }
        let written = replace(&input, &desired).unwrap();
        assert_eq!(written.string("vendor").as_deref(), Some("yes"));
        assert!(written.children().any(|e| e.name == "vendor-global"));
        let section = written.children().find(|e| e.name == "permissions").unwrap();
        assert_eq!(section.string("future").as_deref(), Some("section"));
        assert!(section.children().any(|e| e.name == "future"));
        let normal = section.children().find(|e| e.string("name").as_deref() == Some("p.NORMAL")).unwrap();
        assert_eq!(normal.string("future").as_deref(), Some("item"));
        assert!(normal.children().any(|e| e.name == "vendor"));
        for key in ["protection", "type", "icon", "label"] { assert!(normal.string(key).is_none()); }
    }
    #[test]
    fn completed_scan_persists_real_definitions_and_domains_and_reopens_without_grants() {
        use crate::package::{owner::Store, scan::SigningScan, owner::usage::Usage};
        use std::{fs, sync::atomic::{AtomicU64, Ordering}};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        struct Directory(std::path::PathBuf);
        impl Drop for Directory { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
        for restored in [false, true] {
            let dir = Directory(std::env::temp_dir().join(format!("aim-global-settings-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
            fs::create_dir_all(dir.0.join("system")).unwrap();
            let path = dir.0.join("system/packages.xml");
            let old = if restored {
                b"<packages vendor='saved'><package name='p' codePath='/data/app/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'/><permissions><item name='old' package='p'/></permissions><vendor-global/><keyset-settings version='1'><keys/><keysets/><lastIssuedKeyId value='0'/><lastIssuedKeySetId value='0'/></keyset-settings></packages>".as_slice()
            } else {
                b"<packages vendor='fresh'><package name='p' codePath='/data/app/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'/><vendor-global/><keyset-settings version='1'><keys/><keysets/><lastIssuedKeyId value='0'/><lastIssuedKeySetId value='0'/></keyset-settings></packages>".as_slice()
            };
            fs::write(&path, old).unwrap();
            let mut disk = Store::open(&dir.0, &[]).unwrap().unwrap();
            assert!(disk.state().settings.key_sets.versioned);
            let before_keys = disk.state().settings.key_sets.clone();
            let mut scan = SigningScan::new(&Default::default(), &disk.state().settings, 36).unwrap();
            let source = aim_android_xml::read(b"<packages><permissions><item name='p.PERMISSION' package='p' protection='2'/></permissions><permission-trees><item name='p.TREE' package='p'/></permission-trees><domain-verifications><active><package-state packageName='p' id='00000000-0000-0000-0000-000000000001' hasAutoVerifyDomains='true'><state><domain name='example.test' state='1'/></state></package-state></active><restored/></domain-verifications><domain-verifications-legacy><user-states packageName='p'><user-state userId='0' state='2'/></user-states></domain-verifications-legacy></packages>").unwrap();
            let actual = Settings::parse(&source).unwrap();
            scan.settings.permissions = actual.permissions.clone();
            scan.settings.permission_trees = actual.permission_trees.clone();
            scan.settings.domain_verification = actual.domain_verification.clone();
            let snapshot = crate::package::scan_snapshot::Store::new(scan, Usage::new(["p"])).unwrap().capture();
            disk.commit_scan_settings(&snapshot).unwrap();
            disk.validate_committed_scan(snapshot.owner()).unwrap();
            let reopened = Store::open(&dir.0, &[]).unwrap().unwrap();
            reopened.validate_committed_scan(snapshot.owner()).unwrap();
            assert_eq!(reopened.state().settings.permissions, actual.permissions);
            assert_eq!(reopened.state().settings.permission_trees, actual.permission_trees);
            assert_eq!(reopened.state().settings.domain_verification, actual.domain_verification);
            assert!(reopened.state().users.is_empty());
            assert_eq!(reopened.state().settings.key_sets, before_keys);
            let document = aim_android_xml::read(&fs::read(&path).unwrap()).unwrap();
            assert!(document.children().any(|e| e.name == "vendor-global"));
            assert!(document.children().filter(|e| e.name == "package").all(|e| !e.children().any(|e| e.name == "perms")));
            let bytes = fs::read(&path).unwrap();
            let unchanged = disk.state().settings.clone();
            let mut bad = snapshot.owner().clone();
            bad.settings.permissions.push(bad.settings.permissions[0].clone());
            let invalid = crate::package::scan_snapshot::Store::new(bad, Usage::new(["p"])).unwrap().capture();
            assert!(!disk.commit_scan_settings(&invalid).unwrap_err().committed);
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(disk.state().settings, unchanged);
        }
    }
    #[test]
    fn duplicate_owner_rejection_preserves_source_and_config_runtime_identity() {
        let input = aim_android_xml::read(b"<packages><permissions><item name='p' package='p'/><item name='p' package='p'/></permissions></packages>").unwrap();
        let before = input.clone();
        assert!(replace(&input, &Settings::default()).is_err());
        assert_eq!(input, before);
        let value = Permission { name:"p".into(), package:"android".into(), owner:PermissionOwner::Config { uid:1000, gids:vec![1001] }, protection_level:0, dynamic:None };
        let desired = Settings { permissions:vec![value.clone()], ..Default::default() };
        let written = replace(&element("packages"), &desired).unwrap();
        assert_eq!(Settings::parse(&written).unwrap().permissions, persisted(&[value.clone()]));
        assert_eq!(desired.permissions, [value.clone()]);
        let duplicate = Settings { permissions:vec![value.clone(),value], ..Default::default() };
        assert!(replace(&written, &duplicate).is_err());
    }
}
