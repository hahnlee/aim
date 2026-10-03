//! Captured SettingBase migration owners, not the live permission exporter (#858).
use super::SigningScan;
use crate::package::owner::legacy_permissions::{Migration, State, validate};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Assignments {
    users: Vec<i32>,
    packages: BTreeMap<(String, bool), (i32, bool, Migration)>,
    shared_users: BTreeMap<String, (i32, Migration)>,
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
        });
        Ok(())
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
        let current = store.publish(&base, owner, base.usage().clone()).unwrap();
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
