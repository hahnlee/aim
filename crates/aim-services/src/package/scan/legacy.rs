//! Captured SettingBase migration owners, not the live permission exporter (#858).
use super::SigningScan;
use crate::package::owner::legacy_permissions::{Migration, State, validate};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Assignments {
    users: Vec<i32>,
    packages: BTreeMap<(String, bool), (i32, bool, Migration)>,
    shared_users: BTreeMap<String, (i32, Migration)>,
    restoration: Option<crate::package::owner::legacy_permissions::Metadata>,
    install_fixed: Option<BTreeMap<(String, bool), bool>>,
}

impl SigningScan {
    /// The restoration/import owner supplies every active/factory SettingBase
    /// and every shared user, plus the complete resolved user inventory. Missing
    /// owners reject before replacing the prior assignment; no live state is inferred.
    pub fn capture_legacy_permissions(
        &mut self,
        users: &[i32],
        mut packages: BTreeMap<(String, bool), Migration>,
        mut shared_users: BTreeMap<String, Migration>,
    ) -> Result<(), String> {
        validate(0, users).map_err(|error| format!("legacy user inventory: {error:?}"))?;
        let mut captured = BTreeMap::new();
        for (settings, factory) in [
            (&self.settings.packages, false),
            (&self.settings.disabled_system_packages, true),
        ] {
            for setting in settings {
                let key = (setting.name.clone(), factory);
                let migration = packages
                    .remove(&key)
                    .ok_or_else(|| format!("missing legacy setting owner: {key:?}"))?;
                migration
                    .project(setting.app_id, users)
                    .map_err(|e| format!("legacy setting identity: {e:?}"))?;
                captured.insert(key, (setting.app_id, setting.shared_user, migration));
            }
        }
        let mut groups = BTreeMap::new();
        for (name, group) in &self.identities.shared_users {
            let migration = shared_users
                .remove(name)
                .ok_or_else(|| format!("missing legacy shared owner: {}", name))?;
            migration
                .project(group.app_id, users)
                .map_err(|e| format!("legacy shared identity: {e:?}"))?;
            groups.insert(name.clone(), (group.app_id, migration));
        }
        if !packages.is_empty() || !shared_users.is_empty() {
            return Err("legacy capture contains unknown owners".into());
        }
        self.legacy_permissions = Some(Assignments {
            users: users.to_vec(),
            packages: captured,
            shared_users: groups,
            restoration: None,
            install_fixed: None,
        });
        Ok(())
    }

    pub(super) fn disabled_legacy_for_replacement(&self, name: &str) -> Result<Migration, String> {
        self.validate_legacy_permissions()?;
        self.legacy_permissions
            .as_ref()
            .and_then(|owners| owners.packages.get(&(name.into(), true)))
            .map(|(_, _, migration)| migration.clone())
            .ok_or_else(|| "disabled legacy migration owner is not captured".into())
    }

    /// A newly allocated SharedUserSetting owns an empty constructor state,
    /// even when a later scan fails. Do not create an absent import inventory.
    pub(super) fn legacy_shared_constructor(&mut self, name: &str, id: i32) -> Result<(), String> {
        if let Some(owners) = &mut self.legacy_permissions {
            if owners.shared_users.contains_key(name) {
                return Err("new legacy shared owner already exists".into());
            }
            owners
                .shared_users
                .insert(name.into(), (id, Migration::default()));
        }
        self.validate_legacy_permissions()
    }

    /// New PackageSetting constructors start empty, except for the exact
    /// disabled-factory inheritance branch resolved before construction.
    pub(super) fn legacy_setting_constructor(
        &mut self,
        name: &str,
        previous: bool,
        inherited: Option<Migration>,
    ) -> Result<(), String> {
        let setting = self
            .settings
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or("new legacy setting is missing")?;
        if let Some(owners) = &mut self.legacy_permissions {
            let key = (name.into(), false);
            if owners.packages.contains_key(&key) != previous {
                return Err("new legacy setting owner inventory differs".into());
            }
            owners.packages.insert(
                key.clone(),
                (
                    setting.app_id,
                    setting.shared_user,
                    inherited.unwrap_or_default(),
                ),
            );
            owners
                .install_fixed
                .get_or_insert_with(Default::default)
                .insert(key, false);
        } else if inherited.is_some() {
            return Err("inherited legacy owner is not captured".into());
        }
        self.validate_legacy_permissions()
    }

    /// Retained settings and final registration change only the captured ID.
    pub(super) fn rebind_legacy_setting(&mut self, name: &str) -> Result<(), String> {
        let setting = self
            .settings
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or("retained legacy setting is missing")?;
        if let Some(owners) = &mut self.legacy_permissions {
            let active = owners
                .packages
                .get_mut(&(name.into(), false))
                .ok_or("retained legacy owner is missing")?;
            active.0 = setting.app_id;
            active.1 = setting.shared_user;
        }
        self.validate_legacy_permissions()
    }

    pub(super) fn prune_legacy_shared(&mut self, id: i32) -> Result<(), String> {
        if let Some(owners) = &mut self.legacy_permissions {
            let name = owners
                .shared_users
                .iter()
                .find(|(_, (value, _))| *value == id)
                .map(|(name, _)| name.clone())
                .ok_or("pruned legacy shared owner is missing")?;
            owners.shared_users.remove(&name);
        }
        self.validate_legacy_permissions()
    }

    /// Called only after the complete inventory and disable preconditions
    /// were checked, before copying the active PackageSetting.
    pub(super) fn copy_disabled_legacy(&mut self, name: &str) {
        if let Some(owners) = &mut self.legacy_permissions {
            let active = owners.packages[&(name.into(), false)].clone();
            owners.packages.insert((name.into(), true), active);
            if let Some(values) = &mut owners.install_fixed {
                if let Some(fixed) = values.get(&(name.into(), false)).copied() {
                    values.insert((name.into(), true), fixed);
                }
            }
        }
    }

    /// Conversion unlinks ownership without copying LegacyPermissionState or
    /// installPermissionsFixed. Uncaptured owners remain uncaptured.
    pub(super) fn commit_converted_legacy(
        &mut self,
        name: &str,
        group: &str,
        id: i32,
    ) -> Result<(), String> {
        let Some(owners) = &mut self.legacy_permissions else {
            return Ok(());
        };
        let active = owners
            .packages
            .get_mut(&(name.into(), false))
            .ok_or("converted legacy active owner is missing")?;
        if !active.1
            || ![-1, id].contains(&active.0)
            || owners.shared_users.get(group).map(|g| g.0) != Some(id)
        {
            return Err("converted legacy ownership differs".into());
        }
        active.0 = -1;
        active.1 = false;
        for setting in &self.settings.disabled_system_packages {
            let value = owners
                .packages
                .get_mut(&(setting.name.clone(), true))
                .ok_or("converted legacy disabled owner is missing")?;
            if !setting.shared_user && value.1 && value.0 == id {
                value.1 = false;
            }
        }
        owners.shared_users.remove(group);
        self.validate_legacy_permissions()
    }

    pub(in crate::package) fn validate_legacy_permissions(&self) -> Result<(), String> {
        let Some(owners) = &self.legacy_permissions else {
            return Ok(());
        };
        let mut expected = BTreeMap::new();
        for (settings, factory) in [
            (&self.settings.packages, false),
            (&self.settings.disabled_system_packages, true),
        ] {
            for setting in settings {
                expected.insert(
                    (setting.name.clone(), factory),
                    (setting.app_id, setting.shared_user),
                );
            }
        }
        if expected
            != owners
                .packages
                .iter()
                .map(|(key, (app_id, shared, _))| (key.clone(), (*app_id, *shared)))
                .collect()
        {
            return Err("legacy package owner inventory differs".into());
        }
        let groups: BTreeMap<_, _> = self
            .identities
            .shared_users
            .iter()
            .map(|(name, group)| (name.clone(), group.app_id))
            .collect();
        if groups
            != owners
                .shared_users
                .iter()
                .map(|(name, (id, _))| (name.clone(), *id))
                .collect()
        {
            return Err("legacy shared owner inventory differs".into());
        }
        Ok(())
    }

    /// The original import/restoration owner supplies every PackageSetting bit.
    pub fn capture_install_permissions_fixed(
        &mut self,
        values: BTreeMap<(String, bool), bool>,
    ) -> Result<(), String> {
        self.validate_legacy_permissions()?;
        let owners = self
            .legacy_permissions
            .as_mut()
            .ok_or("legacy migration owner is not captured")?;
        if values.keys().ne(owners.packages.keys()) {
            return Err("install permissions fixed owner inventory differs".into());
        }
        owners.install_fixed = Some(values);
        Ok(())
    }

    pub fn install_permissions_fixed(
        &self,
        name: &str,
        factory: bool,
    ) -> Result<Option<bool>, String> {
        self.validate_legacy_permissions()?;
        Ok(self
            .legacy_permissions
            .as_ref()
            .and_then(|owners| owners.install_fixed.as_ref())
            .and_then(|values| values.get(&(name.into(), factory)))
            .copied())
    }

    /// PackageSetting.setInstallPermissionsFixed after the owner was resolved.
    pub fn set_install_permissions_fixed(
        &mut self,
        name: &str,
        factory: bool,
        fixed: bool,
    ) -> Result<(), String> {
        self.validate_legacy_permissions()?;
        let value = self
            .legacy_permissions
            .as_mut()
            .and_then(|owners| owners.install_fixed.as_mut())
            .and_then(|values| values.get_mut(&(name.into(), factory)))
            .ok_or("install permissions fixed owner is not captured")?;
        *value = fixed;
        Ok(())
    }

    pub fn legacy_permissions(&self, name: &str, factory: bool) -> Result<Option<State>, String> {
        self.validate_legacy_permissions()?;
        let owners = self
            .legacy_permissions
            .as_ref()
            .ok_or("legacy migration owner is not captured")?;
        owners
            .packages
            .get(&(name.into(), factory))
            .map(|(app_id, _, state)| {
                state
                    .project(*app_id, &owners.users)
                    .map_err(|e| format!("legacy setting projection: {e:?}"))
            })
            .transpose()
    }
    pub fn shared_legacy_permissions(&self, name: &str) -> Result<Option<State>, String> {
        self.validate_legacy_permissions()?;
        let owners = self
            .legacy_permissions
            .as_ref()
            .ok_or("legacy migration owner is not captured")?;
        owners
            .shared_users
            .get(name)
            .map(|(app_id, state)| {
                state
                    .project(*app_id, &owners.users)
                    .map_err(|e| format!("legacy shared projection: {e:?}"))
            })
            .transpose()
    }
    /// Read the original migration files against the exact saved Settings
    /// input. All reads and assignments finish on a candidate before commit.
    pub fn restore_legacy_permissions_from_data(
        &mut self,
        data: &std::path::Path,
        state: &crate::package::State,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<(), String> {
        let identities = |settings: &crate::package::settings::Settings| {
            settings
                .packages
                .iter()
                .map(|p| ((p.name.clone(), false), (p.app_id, p.shared_user)))
                .chain(
                    settings
                        .disabled_system_packages
                        .iter()
                        .map(|p| ((p.name.clone(), true), (p.app_id, p.shared_user))),
                )
                .collect::<BTreeMap<_, _>>()
        };
        if identities(&self.settings) != identities(&state.settings) {
            return Err("legacy restoration setting identities differ".into());
        }
        let restored =
            crate::package::owner::legacy_permissions::restore::read(data, state, config)?;
        let mut candidate = self.clone();
        candidate.capture_legacy_permissions(
            &restored.users,
            restored.packages,
            restored.shared_users,
        )?;
        let fixed = candidate
            .legacy_permissions
            .as_ref()
            .unwrap()
            .packages
            .keys()
            .map(|(name, factory)| {
                (
                    (name.clone(), *factory),
                    !factory && restored.metadata.install_permissions_fixed.contains(name),
                )
            })
            .collect();
        candidate.capture_install_permissions_fixed(fixed)?;
        candidate.legacy_permissions.as_mut().unwrap().restoration = Some(restored.metadata);
        self.legacy_permissions = candidate.legacy_permissions;
        Ok(())
    }
    pub fn legacy_restoration_metadata(
        &self,
    ) -> Result<Option<&crate::package::owner::legacy_permissions::Metadata>, String> {
        self.validate_legacy_permissions()?;
        Ok(self
            .legacy_permissions
            .as_ref()
            .and_then(|owners| owners.restoration.as_ref()))
    }

    pub fn has_legacy_permissions(&self) -> bool {
        self.legacy_permissions.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        owner::{legacy_permissions::Permission, usage::Usage},
        scan_snapshot::Store,
        settings::{Package, Settings, SharedUser},
    };
    use std::sync::Arc;

    #[test]
    fn setting_and_shared_migration_captures_are_distinct_and_publish_atomically() {
        let package = Package {
            name: "fixture".into(),
            app_id: 10043,
            shared_user: true,
            ..Default::default()
        };
        let settings = Settings {
            packages: vec![package.clone()],
            disabled_system_packages: vec![package],
            shared_users: vec![SharedUser {
                name: "group".into(),
                app_id: 10043,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        owner
            .assign_seinfo_at_boot(
                &crate::package::owner::seinfo::Policy::unread(),
                &mut |_| Ok(30),
            )
            .unwrap();
        assert!(owner.legacy_permissions("fixture", false).is_err());
        let make = |flags| {
            let mut state = Migration::default();
            state
                .put(
                    10,
                    Permission {
                        name: Some("BB".into()),
                        runtime: false,
                        granted: true,
                        flags,
                    },
                )
                .unwrap();
            state.set_missing(10, true).unwrap();
            state
        };
        let packages = BTreeMap::from([
            (("fixture".into(), false), make(17)),
            (("fixture".into(), true), make(19)),
        ]);
        let mut groups: BTreeMap<_, _> = owner
            .identities
            .shared_users
            .keys()
            .map(|name| (name.clone(), Migration::default()))
            .collect();
        groups.insert("group".into(), make(29));
        owner
            .capture_legacy_permissions(&[10, 0, 11], packages.clone(), groups.clone())
            .unwrap();
        let before = owner.clone();
        assert!(
            owner
                .capture_legacy_permissions(&[10, 0], BTreeMap::new(), groups.clone())
                .is_err()
        );
        assert_eq!(owner, before);
        assert!(
            owner
                .capture_legacy_permissions(&[10, 10], packages.clone(), groups.clone())
                .is_err()
        );
        assert_eq!(owner, before);
        let mut foreign = groups.clone();
        foreign.insert("unknown".into(), Migration::default());
        assert!(
            owner
                .capture_legacy_permissions(&[10, 0], packages.clone(), foreign)
                .is_err()
        );
        assert_eq!(owner, before);
        assert_eq!(owner.legacy_permissions("absent", false).unwrap(), None);
        assert_eq!(owner.shared_legacy_permissions("absent").unwrap(), None);
        assert_eq!(
            owner
                .shared_legacy_permissions("group")
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            29
        );
        assert_eq!(
            owner
                .legacy_permissions("fixture", true)
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            19
        );
        assert_eq!(
            owner.install_permissions_fixed("fixture", false).unwrap(),
            None
        );
        assert!(
            owner
                .set_install_permissions_fixed("fixture", false, true)
                .is_err()
        );
        let unknown = owner.clone();
        assert!(
            owner
                .capture_install_permissions_fixed(BTreeMap::from([(
                    ("fixture".into(), false),
                    true
                )]))
                .is_err()
        );
        assert_eq!(owner, unknown);
        owner
            .capture_install_permissions_fixed(BTreeMap::from([
                (("fixture".into(), false), true),
                (("fixture".into(), true), false),
            ]))
            .unwrap();
        assert!(
            owner
                .set_install_permissions_fixed("absent", false, true)
                .is_err()
        );
        let store = Store::new(owner.clone(), Usage::new(["fixture"])).unwrap();
        let base = store.capture();
        let mut invalid = owner.clone();
        invalid.settings.packages[0].app_id = 10044;
        assert!(store.publish(&base, invalid, base.usage().clone()).is_err());
        assert!(Arc::ptr_eq(&base, &store.capture()));
        let next_packages = BTreeMap::from([
            (("fixture".into(), false), make(33)),
            (("fixture".into(), true), make(19)),
        ]);
        owner
            .capture_legacy_permissions(&[10, 0, 11], next_packages, groups)
            .unwrap();
        // A fresh migration import invalidates bits until supplied by that owner.
        assert_eq!(
            owner.install_permissions_fixed("fixture", false).unwrap(),
            None
        );
        owner
            .capture_install_permissions_fixed(BTreeMap::from([
                (("fixture".into(), false), true),
                (("fixture".into(), true), false),
            ]))
            .unwrap();
        owner
            .set_install_permissions_fixed("fixture", false, false)
            .unwrap();
        owner
            .set_install_permissions_fixed("fixture", true, true)
            .unwrap();
        let current = store.publish(&base, owner, base.usage().clone()).unwrap();
        assert_eq!(
            base.owner()
                .install_permissions_fixed("fixture", false)
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            base.owner()
                .install_permissions_fixed("fixture", true)
                .unwrap(),
            Some(false)
        );
        assert_eq!(
            current
                .owner()
                .install_permissions_fixed("fixture", false)
                .unwrap(),
            Some(false)
        );
        assert_eq!(
            current
                .owner()
                .install_permissions_fixed("fixture", true)
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            base.owner()
                .legacy_permissions("fixture", false)
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            17
        );
        assert_eq!(
            current
                .owner()
                .legacy_permissions("fixture", false)
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            33
        );
        assert_eq!(
            current
                .owner()
                .shared_legacy_permissions("group")
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            29
        );
    }
}
