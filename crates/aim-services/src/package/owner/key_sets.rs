//! KeySetManagerService.removeAppKeySetDataLPw and DomainVerificationService
//! cleanup at android-16.0.0_r1 (#823/#798). Copyright (C) The Android Open
//! Source Project, Apache License 2.0. Package and UID removal follow later.
use crate::package::settings::{KeySetData, Settings};
use aim_android_xml::{Element, Node, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Signing sets and defined aliases each hold a reference; upgrade sets do
/// not. Public keys are referenced by key sets, not by packages. Validate the
/// complete owner view before changing this stage; counters never rewind.
pub fn clear_package(settings: &mut Settings, name: &str) -> Result<(), String> {
    let at = settings
        .packages
        .iter()
        .position(|p| p.name == name)
        .ok_or_else(|| format!("unknown keyset package {name}"))?;
    let mut sets = BTreeMap::new();
    for (id, keys) in &settings.key_sets.key_sets {
        if *id <= 0 || sets.insert(*id, keys).is_some() {
            return Err("invalid or duplicate keyset identity".into());
        }
    }
    let mut public = BTreeSet::new();
    for (id, _) in &settings.key_sets.public_keys {
        if *id <= 0 || !public.insert(*id) {
            return Err("invalid or duplicate public key identity".into());
        }
    }
    if sets
        .values()
        .any(|keys| keys.iter().any(|id| !public.contains(id)))
    {
        return Err("keyset refers to a missing public key".into());
    }
    let mut remaining = BTreeSet::new();
    let mut retired = BTreeSet::new();
    for (index, package) in settings.packages.iter().enumerate() {
        let data = &package.key_set_data;
        let mut aliases = BTreeSet::new();
        if data
            .defined_key_sets
            .iter()
            .any(|(alias, _)| !aliases.insert(alias))
        {
            return Err("duplicate keyset alias".into());
        }
        for id in std::iter::once(&data.proper_signing_key_set)
            .chain(data.defined_key_sets.iter().map(|(_, id)| id))
        {
            if *id == -1 {
                continue;
            }
            if !sets.contains_key(id) {
                return Err(format!(
                    "package {} references missing keyset {id}",
                    package.name
                ));
            }
            if index == at {
                retired.insert(*id);
            } else {
                remaining.insert(*id);
            }
        }
    }
    retired.retain(|id| !remaining.contains(id));
    let retired_keys: BTreeSet<_> = retired
        .iter()
        .flat_map(|id| sets[id].iter().copied())
        .collect();
    let kept_keys: BTreeSet<_> = sets
        .iter()
        .filter(|(id, _)| !retired.contains(id))
        .flat_map(|(_, keys)| keys.iter().copied())
        .collect();
    settings
        .key_sets
        .key_sets
        .retain(|(id, _)| !retired.contains(id));
    settings
        .key_sets
        .public_keys
        .retain(|(id, _)| !retired_keys.contains(id) || kept_keys.contains(id));
    settings.packages[at].key_set_data = KeySetData::default();
    Ok(())
}

pub(super) fn replace(original: &Element, name: &str) -> Result<Element, String> {
    let mut desired = Settings::parse(original)?;
    desired.domain_verification.clear_package(name);
    clear_package(&mut desired, name)?;
    let sets: BTreeSet<_> = desired
        .key_sets
        .key_sets
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let keys: BTreeSet<_> = desired
        .key_sets
        .public_keys
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let mut root = original.clone();
    for owner in root.content.iter_mut().filter_map(|n| {
        if let Node::Element(e) = n {
            Some(e)
        } else {
            None
        }
    }) {
        match owner.name.as_str() {
            "package" if owner.string("name").as_deref() == Some(name) => {
                owner.content.retain(|n| !matches!(n, Node::Element(e) if matches!(e.name.as_str(), "proper-signing-keyset" | "defined-keyset" | "upgrade-keyset")));
                let mut signing = super::element("proper-signing-keyset");
                super::attribute(&mut signing, "identifier", Some(Value::Long(-1)));
                owner.content.push(Node::Element(signing));
            }
            "keyset-settings" => {
                for section in owner.content.iter_mut().filter_map(|n| {
                    if let Node::Element(e) = n {
                        Some(e)
                    } else {
                        None
                    }
                }) {
                    let (tag, kept) = match section.name.as_str() {
                        "keys" => ("public-key", &keys),
                        "keysets" => ("keyset", &sets),
                        _ => continue,
                    };
                    let mut content = Vec::new();
                    for node in &section.content {
                        if let Node::Element(e) = node
                            && e.name == tag
                        {
                            let id = e
                                .long("identifier")?
                                .ok_or("keyset record without identifier")?;
                            if !kept.contains(&id) {
                                continue;
                            }
                        }
                        content.push(node.clone());
                    }
                    section.content = content;
                }
            }
            "domain-verifications" => {
                for section in owner.content.iter_mut().filter_map(|n| {
                    if let Node::Element(e) = n {
                        Some(e)
                    } else {
                        None
                    }
                }) {
                    if matches!(section.name.as_str(), "active" | "restored") {
                        section.content.retain(|n| !matches!(n, Node::Element(e) if e.name == "package-state" && e.string("packageName").as_deref() == Some(name)));
                    }
                }
            }
            _ => {}
        }
    }
    if Settings::parse(&root)? != desired {
        return Err("boot removal metadata did not preserve desired settings".into());
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{KeySets, Package};
    #[test]
    fn retirement_preserves_shared_sets_keys_and_monotonic_counters() {
        let mut settings = Settings {
            packages: vec![
                Package {
                    name: "a".into(),
                    key_set_data: KeySetData {
                        proper_signing_key_set: 1,
                        defined_key_sets: vec![("same".into(), 1), ("only".into(), 2)],
                        upgrade_key_sets: vec![2],
                    },
                    ..Default::default()
                },
                Package {
                    name: "b".into(),
                    key_set_data: KeySetData {
                        proper_signing_key_set: 1,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ],
            key_sets: KeySets {
                version: Some(1),
                public_keys: vec![(1, vec![1]), (2, vec![2])],
                key_sets: vec![(1, vec![1]), (2, vec![1, 2])],
                last_issued_key_id: 2,
                last_issued_key_set_id: 2,
            },
            ..Default::default()
        };
        clear_package(&mut settings, "a").unwrap();
        assert_eq!(settings.key_sets.key_sets, [(1, vec![1])]);
        assert_eq!(settings.key_sets.public_keys, [(1, vec![1])]);
        assert_eq!(settings.packages[0].key_set_data, KeySetData::default());
        clear_package(&mut settings, "a").unwrap();
        clear_package(&mut settings, "b").unwrap();
        assert!(settings.key_sets.key_sets.is_empty());
        assert!(settings.key_sets.public_keys.is_empty());
        assert_eq!(
            (
                settings.key_sets.last_issued_key_id,
                settings.key_sets.last_issued_key_set_id
            ),
            (2, 2)
        );
    }
    #[test]
    fn malformed_references_leave_the_owner_unchanged() {
        let mut settings = Settings {
            packages: vec![Package {
                name: "a".into(),
                key_set_data: KeySetData {
                    proper_signing_key_set: 7,
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let original = settings.clone();
        assert!(clear_package(&mut settings, "a").is_err());
        assert_eq!(settings, original);
    }

    #[test]
    fn unversioned_keysets_drop_restored_package_references() {
        let root = aim_android_xml::read(b"<packages><package name='a' codePath='/system/app/a' userId='10100'><proper-signing-keyset identifier='7'/><defined-keyset alias='old' identifier='8'/><upgrade-keyset identifier='8'/></package><keyset-settings><keys/><keysets/></keyset-settings></packages>").unwrap();
        let settings = Settings::parse(&root).unwrap();
        assert_eq!(settings.packages[0].key_set_data, KeySetData::default());
        assert_eq!(settings.packages[0].key_set_data.proper_signing_key_set, -1);
    }
}
