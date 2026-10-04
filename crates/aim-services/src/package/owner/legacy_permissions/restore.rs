//! Settings.readLPw legacy migration inputs, in original read order (#858).
//! Ported from android-16.0.0_r1 Settings, Copyright (C) The Android Open
//! Source Project, Apache License 2.0. Permission persistence remains original.
use super::{Migration, validate};
use crate::package::{
    State,
    owner::{app_ids::Owner, shared_users::Bootstrap},
    settings::{self, Settings},
    system_config::SystemConfig,
};
use aim_android_xml::Element;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMetadata {
    pub version: Option<i32>,
    pub fingerprint: Option<String>,
    /// Original writeStateForUserAsync follows legacy-file fallback, including
    /// no file. This is a request for the original permission writer, not ours.
    pub rewrite_requested: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub install_permissions_fixed: BTreeSet<String>,
    pub users: BTreeMap<i32, UserMetadata>,
}

pub(in crate::package) struct Restored {
    pub users: Vec<i32>,
    pub packages: BTreeMap<(String, bool), Migration>,
    pub shared_users: BTreeMap<String, Migration>,
    pub metadata: Metadata,
}

pub(in crate::package) fn read(
    data: &Path,
    state: &State,
    config: &SystemConfig,
) -> Result<Restored, String> {
    let users = state
        .users
        .iter()
        .map(|(id, _)| i32::try_from(*id).map_err(|_| "legacy user ID out of range".to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    validate(0, &users).map_err(|e| format!("legacy user inventory: {e:?}"))?;
    let system = data.join("system");
    let root = crate::package::resilient(
        &system.join("packages.xml"),
        &system.join("packages-backup.xml"),
        |root| {
            if Settings::parse_with_config(root, config)? != state.settings {
                return Err("Settings changed before legacy restoration".into());
            }
            Ok(root.clone())
        },
    )?
    .ok_or("Settings disappeared before legacy restoration")?;
    let mut restored = from_settings(&root, state, config, &users)?;
    let mut internal_sdk = 0;
    for entry in settings::records(&root) {
        if entry.name == "version" && entry.string("volumeUuid").is_none() {
            internal_sdk = entry
                .int("sdkVersion")?
                .ok_or("internal version without SDK")?;
        } else if entry.name == "last-platform-version" {
            internal_sdk = entry.int("internal").ok().flatten().unwrap_or(0);
        }
    }
    for ((_, user), &id) in state.users.iter().zip(&users) {
        if let Some(runtime) = &user.runtime_permissions {
            restored.metadata.users.insert(
                id,
                UserMetadata {
                    version: Some(runtime.version),
                    fingerprint: runtime.fingerprint.clone(),
                    rewrite_requested: false,
                },
            );
            for package in &state.settings.packages {
                let migration = restored
                    .packages
                    .get_mut(&(package.name.clone(), false))
                    .ok_or("missing restored package")?;
                // RuntimePermissionsState holds a map: the last duplicate entry wins.
                if let Some((_, permissions)) = runtime
                    .packages
                    .iter()
                    .rev()
                    .find(|(name, _)| name == &package.name)
                {
                    migration.read_runtime(id, permissions)?;
                    restored
                        .metadata
                        .install_permissions_fixed
                        .insert(package.name.clone());
                } else if !package.shared_user && internal_sdk >= 30 {
                    migration.set_missing(id, true)?;
                }
            }
            for (name, migration) in &mut restored.shared_users {
                if let Some((_, permissions)) = runtime
                    .shared_users
                    .iter()
                    .rev()
                    .find(|(entry, _)| entry == name)
                {
                    migration.read_runtime(id, permissions)?;
                } else if internal_sdk >= 30 {
                    migration.set_missing(id, true)?;
                }
            }
        } else {
            let mut metadata = UserMetadata {
                version: None,
                fingerprint: None,
                rewrite_requested: true,
            };
            let path = system
                .join("users")
                .join(id.to_string())
                .join("runtime-permissions.xml");
            // Original checks the main file's existence before AtomicFile.openRead.
            if path.exists() {
                let backup = crate::package::sibling(&path, ".bak");
                let input = if backup.exists() { backup } else { path };
                let bytes =
                    std::fs::read(&input).map_err(|e| format!("{}: {e}", input.display()))?;
                let root = aim_android_xml::read_next(&bytes)?;
                read_legacy(&root, id, &mut restored, &mut metadata)?;
            }
            restored.metadata.users.insert(id, metadata);
        }
    }
    Ok(restored)
}

fn from_settings(
    root: &Element,
    state: &State,
    config: &SystemConfig,
    users: &[i32],
) -> Result<Restored, String> {
    let final_owners = Bootstrap::restore(config, &state.settings)
        .map_err(|e| format!("legacy UID restoration: {e:?}"))?;
    let initial = Bootstrap::new(config);
    let mut visible = initial.ids;
    let mut restored = Restored {
        users: users.to_vec(),
        packages: state
            .settings
            .packages
            .iter()
            .map(|p| ((p.name.clone(), false), Migration::default()))
            .chain(
                state
                    .settings
                    .disabled_system_packages
                    .iter()
                    .map(|p| ((p.name.clone(), true), Migration::default())),
            )
            .collect(),
        shared_users: final_owners
            .shared_users
            .keys()
            .map(|name| (name.clone(), Migration::default()))
            .collect(),
        metadata: Metadata {
            install_permissions_fixed: BTreeSet::new(),
            users: BTreeMap::new(),
        },
    };
    for entry in settings::records(root) {
        match entry.name.as_str() {
            "shared-user" => {
                let Some(name) = entry.string("name").map(|s| s.into_owned()) else {
                    continue;
                };
                let Some(group) = final_owners.shared_users.get(&name) else {
                    continue;
                };
                let id = entry.int("userId")?.unwrap_or(0);
                if id != group.app_id {
                    return Err("legacy shared UID identity differs".into());
                }
                let owner = Owner::SharedUser(name.clone());
                if visible.get(id) != Some(&owner) {
                    visible
                        .register_existing(id, owner)
                        .map_err(|e| format!("legacy shared UID event: {e:?}"))?;
                }
                let migration = restored
                    .shared_users
                    .get_mut(&name)
                    .ok_or("missing legacy shared user")?;
                for perms in entry.children().filter(|e| e.name == "perms") {
                    migration.read_install(perms, users)?;
                }
            }
            "package" | "updated-package" => {
                let factory = entry.name == "updated-package";
                let Some(name) = entry.string("name").map(|s| s.into_owned()) else {
                    continue;
                };
                let key = (name.clone(), factory);
                if !restored.packages.contains_key(&key) {
                    continue;
                }
                let own_id = entry.int("userId")?.unwrap_or(0);
                let target = if own_id > 0 {
                    if !factory {
                        visible
                            .register_existing(own_id, Owner::Package(name.clone()))
                            .map_err(|e| format!("legacy package UID event: {e:?}"))?;
                    }
                    Some(Owner::Package(name.clone()))
                } else {
                    // Pending shared packages can only use an owner visible at
                    // this XML event; a later shared-user tag does not replay perms.
                    visible
                        .get(entry.int("sharedUserId")?.unwrap_or(0))
                        .cloned()
                };
                let perms = package_permissions(entry, factory);
                if let Some(target) = target {
                    let migration = match target {
                        Owner::Package(target) => {
                            restored.packages.get_mut(&(target, factory && own_id > 0))
                        }
                        Owner::SharedUser(target) => restored.shared_users.get_mut(&target),
                        Owner::DetachedPackage(_) => {
                            return Err("detached UID in XML restoration".into());
                        }
                    }
                    .ok_or("visible legacy permission owner is missing")?;
                    for root in &perms {
                        migration.read_install(root, users)?;
                    }
                    if !factory && !perms.is_empty() {
                        restored.metadata.install_permissions_fixed.insert(name);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(restored)
}

fn package_permissions(root: &Element, factory: bool) -> Vec<&Element> {
    let mut out = Vec::new();
    for entry in root.children() {
        if entry.name == "perms" {
            out.push(entry);
        } else if !factory
            && matches!(
                entry.name.as_str(),
                "proper-signing-keyset" | "signing-keyset" | "upgrade-keyset" | "defined-keyset"
            )
        {
            out.extend(package_permissions(entry, false));
        }
    }
    out
}

fn read_legacy(
    root: &Element,
    user: i32,
    restored: &mut Restored,
    metadata: &mut UserMetadata,
) -> Result<(), String> {
    match root.name.as_str() {
        "runtime-permissions" => {
            metadata.version = Some(root.int("version").ok().flatten().unwrap_or(-1));
            metadata.fingerprint = root.string("fingerprint").map(|s| s.into_owned());
        }
        "package" | "shared-user" => {
            let name = root.string("name").map(|s| s.into_owned());
            let migration = name.as_ref().and_then(|name| {
                if root.name == "package" {
                    restored.packages.get_mut(&(name.clone(), false))
                } else {
                    restored.shared_users.get_mut(name)
                }
            });
            if let Some(migration) = migration {
                migration.read_legacy_runtime(root, user)?;
            }
            return Ok(());
        }
        _ => {}
    }
    for child in root.children() {
        read_legacy(child, user, restored, metadata)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{owner::tests::Data, scan::SigningScan};
    use std::fs;

    const XML: &str = "<packages><version sdkVersion='36' databaseVersion='3'/><package name='early' codePath='/data/early' sharedUserId='10050'><perms><item name='dropped'/></perms></package><shared-user name='group' userId='10050'><perms><item name='base' flags='17'/></perms></shared-user><package name='late' codePath='/data/late' sharedUserId='10050'><perms><item name='late'/></perms></package><updated-package name='late' codePath='/system/late' userId='10050'><perms><item name='factory'/></perms></updated-package><updated-package name='early' codePath='/system/early' sharedUserId='10050'><perms><item name='factory-shared'/></perms></updated-package><package name='standalone' codePath='/data/standalone' userId='10070'><signing-keyset identifier='1'><perms><item name='nested'/></perms></signing-keyset><uses-static-lib><perms><item name='ignored'/></perms></uses-static-lib></package></packages>";

    #[test]
    fn real_file_restoration_preserves_event_owners_and_modern_missing_policy() {
        let data = Data::new();
        data.settings();
        fs::write(data.0.join("system/packages.xml"), XML).unwrap();
        let modular = data.0.join("misc_de/0/apexdata/com.android.permission");
        fs::create_dir_all(&modular).unwrap();
        fs::write(modular.join("runtime-permissions.xml"), "<runtime-permissions version='7' fingerprint='fp'><package name='early'><permission name='discarded' granted='true' flags='7'/></package><package name='early'/></runtime-permissions>").unwrap();
        let state = State::read(&data.0, &[10, 0, 11]).unwrap().unwrap();
        let mut scan = SigningScan::new(&Default::default(), &state.settings, 36).unwrap();
        scan.restore_legacy_permissions_from_data(&data.0, &state, &Default::default())
            .unwrap();
        let early = scan.legacy_permissions("early", false).unwrap().unwrap();
        assert!(early.user(10).unwrap().permissions.is_empty());
        assert!(early.user(0).unwrap().permissions.is_empty());
        assert!(!early.user(0).unwrap().missing);
        let factory = scan.legacy_permissions("late", true).unwrap().unwrap();
        assert_eq!(
            factory.user(10).unwrap().permissions[0].name.as_deref(),
            Some("factory")
        );
        let group = scan.shared_legacy_permissions("group").unwrap().unwrap();
        assert_eq!(group.user(10).unwrap().permissions.len(), 3);
        assert!(group.user(0).unwrap().missing);
        assert!(!group.user(10).unwrap().missing);
        assert!(
            group
                .user(10)
                .unwrap()
                .permissions
                .iter()
                .all(|p| p.name.as_deref() != Some("dropped")
                    && p.name.as_deref() != Some("factory"))
        );
        let standalone = scan
            .legacy_permissions("standalone", false)
            .unwrap()
            .unwrap();
        assert!(standalone.user(0).unwrap().missing);
        assert_eq!(
            standalone.user(10).unwrap().permissions[0].name.as_deref(),
            Some("nested")
        );
        let metadata = scan.legacy_restoration_metadata().unwrap().unwrap();
        assert_eq!(
            metadata.install_permissions_fixed,
            BTreeSet::from(["early".into(), "late".into(), "standalone".into()])
        );
        assert_eq!(metadata.users[&0].version, Some(7));
        assert_eq!(metadata.users[&0].fingerprint.as_deref(), Some("fp"));
        assert!(!metadata.users[&0].rewrite_requested);
        assert!(metadata.users[&10].rewrite_requested);
        assert_eq!(
            scan.install_permissions_fixed("early", false).unwrap(),
            Some(true)
        );
        assert_eq!(
            scan.install_permissions_fixed("late", false).unwrap(),
            Some(true)
        );
        assert_eq!(
            scan.install_permissions_fixed("late", true).unwrap(),
            Some(false)
        );
        let before = scan.clone();
        fs::write(data.0.join("system/packages.xml"), "<packages/>").unwrap();
        assert!(
            scan.restore_legacy_permissions_from_data(&data.0, &state, &Default::default())
                .is_err()
        );
        assert_eq!(scan, before);
    }

    #[test]
    fn legacy_fallback_reads_backup_and_upgrade_does_not_mark_missing() {
        let data = Data::new();
        data.settings();
        fs::write(data.0.join("system/packages.xml"), "<packages><last-platform-version internal='29'/><package name='app' codePath='/data/app' userId='10070'><perms><item name='keep'/></perms></package></packages>").unwrap();
        let old = data.0.join("system/users/0/runtime-permissions.xml");
        fs::write(&old, "<wrong/>").unwrap();
        fs::write(crate::package::sibling(&old, ".bak"), "<runtime-permissions version='4' fingerprint='old'><unknown><package name='app'><unknown><item name='keep' granted='false' flags='17'/></unknown></package></unknown><package name='absent'><package name='app'><item name='ignored'/></package></package></runtime-permissions>").unwrap();
        let state = State::read(&data.0, &[0]).unwrap().unwrap();
        let restored = read(&data.0, &state, &Default::default()).unwrap();
        let package = &restored.packages[&("app".into(), false)];
        let permission = package.permission(0, Some("keep")).unwrap().unwrap();
        assert!(permission.runtime);
        assert!(!permission.granted);
        assert_eq!(permission.flags, 0x17);
        assert!(!package.has_permission_state(&[Some("ignored")]));
        assert_eq!(restored.metadata.users[&0].version, Some(4));
        assert!(restored.metadata.users[&0].rewrite_requested);
        fs::remove_file(&old).unwrap();
        let skipped = read(&data.0, &state, &Default::default()).unwrap();
        assert!(
            !skipped.packages[&("app".into(), false)]
                .permission(0, Some("keep"))
                .unwrap()
                .unwrap()
                .runtime
        );
        let modular = data.0.join("misc_de/0/apexdata/com.android.permission");
        fs::create_dir_all(&modular).unwrap();
        fs::write(
            modular.join("runtime-permissions.xml"),
            "<runtime-permissions/>",
        )
        .unwrap();
        let modern = State::read(&data.0, &[0]).unwrap().unwrap();
        let modern = read(&data.0, &modern, &Default::default()).unwrap();
        assert!(
            !modern.packages[&("app".into(), false)]
                .is_missing(0)
                .unwrap()
        );
        assert!(
            modern
                .shared_users
                .values()
                .all(|p| !p.is_missing(0).unwrap())
        );
    }

    #[test]
    fn configured_seeded_uids_and_nested_records_survive_without_xml_declarations() {
        let mut config = SystemConfig::default();
        config.oem_defined_uids = vec![("android.uid.vendor.fixture".into(), 2901)];
        let root = aim_android_xml::read_next(b"<packages><preferred-packages><package name='system' codePath='/system/app' sharedUserId='1000'><perms><item name='seed'/></perms></package></preferred-packages><package name='oem' codePath='/vendor/app' sharedUserId='2901'/><updated-package name='system' codePath='/system/old' userId='1000'/><unknown><package name='ignored' codePath='/data/app' userId='10090'/></unknown></packages>").unwrap();
        let settings = Settings::parse_with_config(&root, &config).unwrap();
        assert_eq!(settings.packages.len(), 2);
        assert!(settings.disabled_system_packages[0].shared_user);
        let owners = Bootstrap::restore(&config, &settings).unwrap();
        assert!(owners.shared_users["android.uid.system"].has_package("system"));
        assert!(owners.shared_users["android.uid.vendor.fixture"].has_package("oem"));
        assert_eq!(Settings::parse(&root).unwrap().packages.len(), 1);
        let data = Data::new();
        data.settings();
        fs::write(
            data.0.join("system/packages.xml"),
            aim_android_xml::abx::write(&root).unwrap(),
        )
        .unwrap();
        let state = State::read_with_config(&data.0, &[0], &config)
            .unwrap()
            .unwrap();
        let restored = read(&data.0, &state, &config).unwrap();
        assert!(restored.shared_users["android.uid.system"].has_permission_state(&[Some("seed")]));
        assert!(
            restored.packages[&("system".into(), false)]
                .permissions(0)
                .unwrap()
                .is_empty()
        );
        assert!(read(&data.0, &state, &Default::default()).is_err());
    }
}
