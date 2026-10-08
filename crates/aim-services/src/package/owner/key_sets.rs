//! KeySetManagerService.removeAppKeySetDataLPw and DomainVerificationService
//! cleanup at android-16.0.0_r1 (#823/#798). Copyright (C) The Android Open
//! Source Project, Apache License 2.0. Package and UID removal follow later.
use crate::package::settings::{KeySetData, Settings};
use aim_android_xml::{Element, Node, Value};
use std::collections::{BTreeMap, BTreeSet};
mod persistence;
mod registration;
pub(super) use persistence::{replace_for_scan, replace_registered};
pub use registration::{register, restore};

/// addRefCountsFromSavedPackagesLPw after a complete persistence container.
/// Use read-event counts, including pending/replaced roles, rather than deriving
/// them from the final package map. Missing public keys are an uncaught input error.
pub fn finish_read(
    settings: &mut Settings,
    refs: &BTreeMap<i64, i32>,
) -> Result<(), crate::package::settings::ReadError> {
    use crate::package::settings::ReadError;
    let counts = settings.key_sets.reference_counts.get_or_insert_with(|| {
        settings
            .key_sets
            .key_sets
            .iter()
            .map(|(id, _)| (*id, 0))
            .collect()
    });
    for (id, count) in refs {
        if let Some(value) = counts.get_mut(id) {
            *value = *count;
        } else {
            eprintln!("Encountered non-existent key-set reference when reading settings: {id}");
        }
    }
    let mut orphans = Vec::new();
    for (id, keys) in &settings.key_sets.key_sets {
        if counts.get(id) == Some(&0) {
            eprintln!("Encountered key-set w/out package references when reading settings: {id}");
            orphans.push(*id);
        }
        for key in keys {
            if !settings
                .key_sets
                .public_keys
                .iter()
                .any(|(id, _)| id == key)
            {
                return Err(ReadError::FatalInput(format!(
                    "keyset public-key owner is absent: {key}"
                )));
            }
        }
    }
    for id in orphans {
        registration::release(settings, id);
    }
    Ok(())
}

/// Signing sets and defined aliases each hold a reference; upgrade sets do
/// not. Public keys are referenced by key sets, not by packages. Validate the
/// pool structure before changing this stage; absent saved role handles are
/// ignored as in the original decrementKeySetLPw. Counters never rewind.
pub fn clear_package(settings: &mut Settings, name: &str) -> Result<(), String> {
    let at = settings
        .packages
        .iter()
        .position(|p| p.name == name)
        .ok_or_else(|| format!("unknown keyset package {name}"))?;
    validated_pool(settings)?;
    let ids: Vec<_> = std::iter::once(settings.packages[at].key_set_data.proper_signing_key_set)
        .chain(
            settings.packages[at]
                .key_set_data
                .defined_key_sets
                .iter()
                .map(|(_, id)| *id),
        )
        .collect();
    if ids.iter().any(|id| *id != -1) {
        registration::initialize_references(settings);
        for id in ids {
            registration::release(settings, id);
        }
    }
    settings.packages[at].key_set_data = KeySetData::default();
    Ok(())
}

fn validated_pool(settings: &Settings) -> Result<BTreeMap<i64, &Vec<i64>>, String> {
    let mut sets = BTreeMap::new();
    for (id, keys) in &settings.key_sets.key_sets {
        if *id <= 0 || sets.insert(*id, keys).is_some() {
            return Err("invalid or duplicate keyset identity".into());
        }
    }
    if let Some(counts) = &settings.key_sets.reference_counts {
        if counts.len() != sets.len() || counts.keys().any(|id| !sets.contains_key(id)) {
            return Err("keyset reference owner does not match its pool".into());
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
    Ok(sets)
}

fn validated_sets(settings: &Settings) -> Result<BTreeMap<i64, &Vec<i64>>, String> {
    let sets = validated_pool(settings)?;
    for package in &settings.packages {
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
            if *id != -1 && !sets.contains_key(id) {
                return Err(format!(
                    "package {} references missing keyset {id}",
                    package.name
                ));
            }
        }
    }
    Ok(sets)
}

pub(super) fn replace(
    original: &Element,
    desired: &Settings,
    name: &str,
) -> Result<Element, String> {
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
    if super::canonical_persistent_settings(Settings::parse(&root)?)
        != super::canonical_persistent_settings(desired.clone())
    {
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
                        defined_key_sets: vec![(Some("same".into()), 1), (Some("only".into()), 2)],
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
                versioned: true,
                public_keys: vec![(1, vec![1]), (2, vec![2])],
                key_sets: vec![(1, vec![1]), (2, vec![1, 2])],
                last_issued_key_id: 2,
                last_issued_key_set_id: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        settings.packages[0].key_set_data.add_defined_key_set(99, Some("absent".into()));
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
    fn retirement_clears_absent_saved_roles_without_creating_handles() {
        let mut settings = Settings {
            packages: vec![Package {
                name: "a".into(),
                key_set_data: KeySetData {
                    proper_signing_key_set: 7,
                    defined_key_sets: vec![(Some("missing".into()), 8)],
                    upgrade_key_sets: vec![9],
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        clear_package(&mut settings, "a").unwrap();
        assert_eq!(settings.packages[0].key_set_data, KeySetData::default());
        assert!(settings.key_sets.key_sets.is_empty());
        assert!(settings.key_sets.public_keys.is_empty());
        assert!(clear_package(&mut settings, "unknown").is_err());
    }

    #[test]
    fn unversioned_keysets_drop_restored_package_references() {
        let root = aim_android_xml::read(b"<packages><package name='a' codePath='/system/app/a' userId='10100'><proper-signing-keyset identifier='7'/><defined-keyset alias='old' identifier='8'/><upgrade-keyset identifier='8'/></package><keyset-settings><keys/><keysets/></keyset-settings></packages>").unwrap();
        let settings = Settings::parse(&root).unwrap();
        assert_eq!(settings.packages[0].key_set_data, KeySetData::default());
        assert_eq!(settings.packages[0].key_set_data.proper_signing_key_set, -1);
    }
}

impl super::Store {
    /// Metadata-only callbacks cannot allocate or retire keysets. If their
    /// unchanged native baseline has an older allocation history, retain the
    /// complete current durable pool owner after exact pool/role validation.
    pub(crate) fn rebase_unchanged_key_set_metadata(
        &self, baseline:&Settings, candidate:&mut Settings,
    )->Result<bool,super::WriteError>{
        if candidate.key_sets!=baseline.key_sets {
            return Err(super::WriteError::before("metadata mutation changed keyset pool owner"));
        }
        let roles=|settings:&Settings|settings.packages.iter().filter(|package|package.app_id>0)
            .map(|package|((package.name.clone(),false),package.key_set_data.clone()))
            .chain(settings.disabled_system_packages.iter().filter(|package|package.app_id>0).map(|package|((package.name.clone(),true),package.key_set_data.clone())))
            .collect::<BTreeMap<_,_>>();
        if roles(candidate)!=roles(baseline){return Err(super::WriteError::before("metadata mutation changed keyset roles"));}
        let current=&self.state().settings;
        if baseline.key_sets==current.key_sets { return Ok(false); }
        let pools_equal=baseline.key_sets.versioned==current.key_sets.versioned
            &&baseline.key_sets.public_keys==current.key_sets.public_keys&&baseline.key_sets.key_sets==current.key_sets.key_sets;
        let roles_equal=roles(baseline)==roles(current);
        let history_forward=current.key_sets.last_issued_key_id>=baseline.key_sets.last_issued_key_id
            &&current.key_sets.last_issued_key_set_id>=baseline.key_sets.last_issued_key_set_id;
        if baseline.key_sets!=current.key_sets {
            eprintln!("native/durable keyset baseline: native=({},{}), durable=({},{}), pools_equal={}, roles_equal={}, history_forward={}",
                baseline.key_sets.last_issued_key_id,baseline.key_sets.last_issued_key_set_id,current.key_sets.last_issued_key_id,current.key_sets.last_issued_key_set_id,pools_equal,roles_equal,history_forward);
        }
        if !pools_equal||!roles_equal||!history_forward {
            return Err(super::WriteError::before(format!("metadata keyset owner differs: native=({},{}), durable=({},{}), pools_equal={pools_equal}, roles_equal={roles_equal}, history_forward={history_forward}",
                baseline.key_sets.last_issued_key_id,baseline.key_sets.last_issued_key_set_id,current.key_sets.last_issued_key_id,current.key_sets.last_issued_key_set_id)));
        }
        validated_sets(current).map_err(super::WriteError::before)?;
        let changed=candidate.key_sets!=current.key_sets;
        candidate.key_sets=current.key_sets.clone();
        Ok(changed)
    }
}
