//! KeySetManagerService addScannedPackageLPw/addRefCountsFromSavedPackagesLPw
//! at android-16.0.0_r1 (#824). Copyright (C) The Android Open Source Project,
//! Apache License 2.0. Public keys are canonical PublicKey.getEncoded bytes.
use super::{Settings, validated_sets};
use crate::package::sign::canonical_public_keys;
use std::collections::BTreeSet;

pub(super) fn initialize_references(settings: &mut Settings) {
    if settings.key_sets.reference_counts.is_some() {
        return;
    }
    let mut counts = settings
        .key_sets
        .key_sets
        .iter()
        .map(|(id, _)| (*id, 0i32))
        .collect::<std::collections::BTreeMap<_, _>>();
    for package in &settings.packages {
        for id in std::iter::once(package.key_set_data.proper_signing_key_set).chain(
            package
                .key_set_data
                .defined_key_sets
                .iter()
                .map(|(_, id)| *id),
        ) {
            if let Some(count) = counts.get_mut(&id) {
                *count = count.wrapping_add(1);
            }
        }
    }
    settings.key_sets.reference_counts = Some(counts);
}

pub(super) fn release(settings: &mut Settings, id: i64) {
    let Some(count) = settings
        .key_sets
        .reference_counts
        .as_mut()
        .unwrap()
        .get_mut(&id)
    else {
        return;
    };
    *count = count.wrapping_sub(1);
    if *count > 0 {
        return;
    }
    settings
        .key_sets
        .reference_counts
        .as_mut()
        .unwrap()
        .remove(&id);
    let removed_keys: BTreeSet<_> = settings
        .key_sets
        .key_sets
        .iter()
        .filter(|(set, _)| *set == id)
        .flat_map(|(_, keys)| keys.iter().copied())
        .collect();
    settings.key_sets.key_sets.retain(|(set, _)| *set != id);
    let live_keys: BTreeSet<_> = settings
        .key_sets
        .key_sets
        .iter()
        .flat_map(|(_, keys)| keys.iter().copied())
        .collect();
    settings
        .key_sets
        .public_keys
        .retain(|(key, _)| !removed_keys.contains(key) || live_keys.contains(key));
}

/// Load the owner and prune unreferenced key sets, as the original reader does.
/// Public keys not referenced by any removed set are retained. Invalid owner
/// input leaves settings unchanged; disabled factories hold no separate refs.
pub fn restore(settings: &mut Settings) -> Result<(), String> {
    validated_sets(settings)?;
    initialize_references(settings);
    let orphans: Vec<_> = settings
        .key_sets
        .reference_counts
        .as_ref()
        .unwrap()
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    for id in orphans {
        release(settings, id);
    }
    settings.key_sets.key_sets.sort_by_key(|(id, _)| *id);
    for (_, keys) in &mut settings.key_sets.key_sets {
        let mut distinct = Vec::new();
        for id in keys.iter().copied() {
            if !distinct.contains(&id) {
                distinct.push(id);
            }
        }
        distinct.sort_by_key(|id| (*id ^ (*id >> 32)) as i32);
        *keys = distinct;
    }
    settings.key_sets.public_keys.sort_by_key(|(id, _)| *id);
    Ok(())
}

/// Replace scanned signing keys and optional defined aliases. The caller
/// supplies aliases/upgrade names in the original map/set iteration order.
/// New aliases acquire references before old aliases release theirs.
pub fn register(
    settings: &mut Settings,
    name: &str,
    signing: &[Vec<u8>],
    defined: Option<&[(String, Vec<Vec<u8>>)]>,
    upgrades: &[String],
) -> Result<(), String> {
    validated_sets(settings)?;
    let at = settings
        .packages
        .iter()
        .position(|p| p.name == name)
        .ok_or("unknown keyset package")?;
    let signing = canonical_public_keys(signing)?;
    if signing.is_empty() {
        return Err("package has no signing public keys".into());
    }
    let mut aliases = Vec::new();
    if let Some(defined) = defined {
        let mut names = BTreeSet::new();
        for (alias, keys) in defined {
            if !names.insert(alias) {
                return Err("duplicate defined keyset alias".into());
            }
            let keys = canonical_public_keys(keys)?;
            if keys.is_empty() {
                return Err("defined keyset has no public keys".into());
            }
            aliases.push((alias.clone(), keys));
        }
    }
    if upgrades
        .iter()
        .any(|alias| defined.is_none() || !aliases.iter().any(|(name, _)| name == alias))
    {
        return Err("upgrade keyset has no corresponding definition".into());
    }
    let mut staged = settings.clone();
    initialize_references(&mut staged);
    let old = staged.packages[at].key_set_data.proper_signing_key_set;
    let unchanged = staged
        .key_sets
        .key_sets
        .iter()
        .find(|(id, _)| *id == old)
        .is_some_and(|(_, ids)| {
            let existing: BTreeSet<_> = ids
                .iter()
                .filter_map(|id| {
                    staged
                        .key_sets
                        .public_keys
                        .iter()
                        .find(|(key, _)| key == id)
                        .map(|(_, key)| key)
                })
                .collect();
            existing == signing.iter().collect()
        });
    if !unchanged {
        staged.packages[at].key_set_data.proper_signing_key_set = -1;
        release(&mut staged, old);
        let id = add_set(&mut staged, &signing)?;
        staged.packages[at].key_set_data.proper_signing_key_set = id;
    }
    if defined.is_some() {
        let old: Vec<_> = staged.packages[at]
            .key_set_data
            .defined_key_sets
            .iter()
            .map(|(_, id)| *id)
            .collect();
        let mut added = Vec::new();
        for (alias, keys) in aliases {
            added.push((Some(alias), add_set(&mut staged, &keys)?));
        }
        staged.packages[at].key_set_data.defined_key_sets.clear();
        for (alias, id) in added {
            staged.packages[at]
                .key_set_data
                .add_defined_key_set(id, alias);
        }
        staged.packages[at].key_set_data.upgrade_key_sets.clear();
        for alias in upgrades {
            let id = staged.packages[at]
                .key_set_data
                .defined_key_sets
                .iter()
                .find(|(name, _)| name.as_deref() == Some(alias.as_str()))
                .unwrap()
                .1;
            if !staged.packages[at]
                .key_set_data
                .upgrade_key_sets
                .contains(&id)
            {
                staged.packages[at].key_set_data.upgrade_key_sets.push(id);
            }
        }
        for id in old {
            release(&mut staged, id);
        }
    }
    staged.key_sets.versioned = true;
    *settings = staged;
    Ok(())
}

fn next(counter: &mut i64, maximum: i64) -> Result<i64, String> {
    let id = counter
        .checked_add(1)
        .filter(|id| *id > 0 && *id > maximum)
        .ok_or("invalid or exhausted keyset allocation counter")?;
    *counter = id;
    Ok(id)
}

fn add_set(settings: &mut Settings, keys: &[Vec<u8>]) -> Result<i64, String> {
    let owner = &mut settings.key_sets;
    let mut ids = Vec::new();
    for key in keys {
        let id = match owner
            .public_keys
            .iter()
            .filter(|(_, value)| value == key)
            .map(|(id, _)| *id)
            .min()
        {
            Some(id) => id,
            None => {
                let id = next(
                    &mut owner.last_issued_key_id,
                    owner
                        .public_keys
                        .iter()
                        .map(|(id, _)| *id)
                        .max()
                        .unwrap_or(0),
                )?;
                owner.public_keys.push((id, key.clone()));
                id
            }
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.sort_by_key(|id| (*id ^ (*id >> 32)) as i32);
    let members: BTreeSet<_> = ids.iter().copied().collect();
    if let Some(id) = owner
        .key_sets
        .iter()
        .filter(|(_, keys)| keys.iter().copied().collect::<BTreeSet<_>>() == members)
        .map(|(id, _)| *id)
        .min()
    {
        let count = owner
            .reference_counts
            .as_mut()
            .unwrap()
            .get_mut(&id)
            .ok_or("keyset has no reference owner")?;
        *count = count.wrapping_add(1);
        return Ok(id);
    }
    let id = next(
        &mut owner.last_issued_key_set_id,
        owner.key_sets.iter().map(|(id, _)| *id).max().unwrap_or(0),
    )?;
    owner.key_sets.push((id, ids));
    owner.reference_counts.as_mut().unwrap().insert(id, 1);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        owner::key_sets::clear_package,
        settings::{KeySets, Package},
    };
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn key(value: u8) -> Vec<u8> {
        let mut encoded = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        encoded.extend_from_slice(
            p256::SecretKey::from_slice(&[value; 32])
                .unwrap()
                .public_key()
                .to_encoded_point(false)
                .as_bytes(),
        );
        encoded
    }
    fn settings() -> Settings {
        Settings {
            packages: ["a", "b"]
                .map(|name| Package {
                    name: name.into(),
                    ..Default::default()
                })
                .to_vec(),
            ..Default::default()
        }
    }
    #[test]
    fn replacement_reuses_shared_keys_and_acquires_aliases_before_retirement() {
        let (first, second) = (key(1), key(2));
        let mut settings = settings();
        register(
            &mut settings,
            "a",
            &[first.clone()],
            Some(&[
                ("common".into(), vec![first.clone()]),
                ("extra".into(), vec![second.clone()]),
            ]),
            &["extra".into()],
        )
        .unwrap();
        assert_eq!(settings.packages[0].key_set_data.proper_signing_key_set, 1);
        assert_eq!(
            settings.packages[0].key_set_data.defined_key_sets,
            [(Some("common".into()), 1), (Some("extra".into()), 2)]
        );
        assert_eq!(settings.packages[0].key_set_data.upgrade_key_sets, [2]);
        register(&mut settings, "b", &[first.clone()], None, &[]).unwrap();
        register(
            &mut settings,
            "a",
            &[first],
            Some(&[("renamed".into(), vec![second.clone()])]),
            &[],
        )
        .unwrap();
        assert_eq!(
            settings.packages[0].key_set_data.defined_key_sets,
            [(Some("renamed".into()), 2)]
        );
        assert!(
            settings.packages[0]
                .key_set_data
                .upgrade_key_sets
                .is_empty()
        );
        assert_eq!(
            (
                settings.key_sets.last_issued_key_id,
                settings.key_sets.last_issued_key_set_id
            ),
            (2, 2)
        );
        register(&mut settings, "a", &[second], Some(&[]), &[]).unwrap();
        assert_eq!(settings.packages[0].key_set_data.proper_signing_key_set, 2);
        clear_package(&mut settings, "b").unwrap();
        assert_eq!(settings.key_sets.key_sets, [(2, vec![2])]);
        clear_package(&mut settings, "a").unwrap();
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
    fn restoration_prunes_orphan_sets_but_preserves_shared_and_unreferenced_keys() {
        let mut settings = settings();
        settings.packages[0].key_set_data.proper_signing_key_set = 1;
        settings.key_sets = KeySets {
            versioned: true,
            key_sets: vec![(1, vec![1]), (2, vec![1, 2])],
            public_keys: vec![(1, key(1)), (2, key(2)), (3, key(3))],
            last_issued_key_id: 3,
            last_issued_key_set_id: 2,
            ..Default::default()
        };
        restore(&mut settings).unwrap();
        assert_eq!(settings.key_sets.key_sets, [(1, vec![1])]);
        assert_eq!(
            settings
                .key_sets
                .public_keys
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(settings.key_sets.last_issued_key_id, 3);
        let saved = settings.clone();
        restore(&mut settings).unwrap();
        assert_eq!(settings, saved);
    }

    #[test]
    fn allocation_and_upgrade_failures_leave_settings_unchanged() {
        let mut settings = settings();
        settings.key_sets.last_issued_key_id = i64::MAX;
        let old = settings.clone();
        assert!(register(&mut settings, "a", &[key(1)], None, &[]).is_err());
        assert_eq!(settings, old);
        assert!(register(&mut settings, "a", &[key(1)], None, &["missing".into()]).is_err());
        assert_eq!(settings, old);
    }

    #[test]
    fn compressed_and_uncompressed_ec_encodings_share_one_identity() {
        let public = p256::SecretKey::from_slice(&[1; 32]).unwrap().public_key();
        let full = key(1);
        assert_eq!(full.len(), 91);
        let mut compressed = full[..26].to_vec();
        compressed[1] = 57;
        compressed[24] = 34;
        compressed.extend_from_slice(public.to_encoded_point(true).as_bytes());
        let mut settings = settings();
        register(&mut settings, "a", &[compressed, full.clone()], None, &[]).unwrap();
        assert_eq!(settings.key_sets.public_keys, [(1, full)]);
        assert_eq!(settings.key_sets.key_sets, [(1, vec![1])]);
    }
}
