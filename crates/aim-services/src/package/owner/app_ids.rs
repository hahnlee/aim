//! Native UID-slot ownership for boot and install reconciliation (#702).
//! Ported from android-16.0.0_r1 AppIdSettingMap and Settings; Copyright
//! (C) The Android Open Source Project, Apache License 2.0. A sparse map
//! retains the original array extent and deletion cursor without allocating
//! an array up to an untrusted persisted app ID.

use crate::package::settings::Settings;
use std::collections::{BTreeMap, BTreeSet};

pub const FIRST_APPLICATION_UID: i32 = 10000;
pub const LAST_APPLICATION_UID: i32 = 19999;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    Package(String),
    DetachedPackage(String),
    SharedUser(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    DetachedOwner {
        name: String,
    },
    Occupied {
        app_id: i32,
        existing: Owner,
        requested: Owner,
    },
    Exhausted,
    OutsideArray {
        app_id: i32,
    },
    DuplicatePackage(String),
    DuplicateSharedUser(String),
    MissingSharedUser {
        package: String,
        app_id: i32,
    },
    InvalidId {
        name: String,
        app_id: i32,
    },
}

/// Clone supplies a candidate or immutable version; operations never
/// mutate persisted settings or an older published snapshot.
/// A UID slot can retain the prior setting after a distinct copied setting
/// replaces it in Settings.mPackages. Captured imports remain optional.
#[derive(Clone, Debug, PartialEq)]
pub struct DetachedSetting {
    pub package: crate::package::settings::Package,
    pub users: BTreeMap<i32, crate::package::restrictions::UserState>,
    pub(in crate::package) user_aliases: BTreeSet<i32>,
    pub legacy: Option<super::legacy_permissions::State>,
    pub install_fixed: Option<bool>,
    pub runtime: Option<crate::package::scan::ReplicaRuntime>,
}

impl DetachedSetting {
    pub fn aliases_user(&self, user: i32) -> bool {
        self.user_aliases.contains(&user)
    }

    pub(in crate::package) fn update_user_aliases(
        &mut self,
        users: &BTreeMap<i32, crate::package::restrictions::UserState>,
    ) {
        for id in &self.user_aliases {
            if let Some(state) = users.get(id) {
                self.users.insert(*id, state.clone());
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppIds {
    slots: BTreeMap<i32, Owner>,
    detached: BTreeMap<i32, std::sync::Arc<DetachedSetting>>,
    non_system_size: i64,
    first_available: i32,
}

impl Default for AppIds {
    fn default() -> Self {
        Self {
            slots: BTreeMap::new(),
            detached: BTreeMap::new(),
            non_system_size: 0,
            first_available: FIRST_APPLICATION_UID,
        }
    }
}

impl AppIds {
    /// Active packages reserve slots even when uninstalled for every
    /// user. Disabled system versions share the active version's identity
    /// and do not independently register a UID. The pinned Settings
    /// reader requires an app ID even for SDK libraries (#802).
    pub fn restore(settings: &Settings) -> Result<Self, Error> {
        Self::restore_seeded(settings, &Self::default())
    }

    pub(in crate::package) fn restore_seeded(
        settings: &Settings,
        seeds: &Self,
    ) -> Result<Self, Error> {
        let mut ids = seeds.clone();
        let mut shared_names = BTreeSet::new();
        for shared in &settings.shared_users {
            if !shared_names.insert(&shared.name) {
                return Err(Error::DuplicateSharedUser(shared.name.clone()));
            }
            if shared.app_id <= 0 {
                return Err(Error::InvalidId {
                    name: shared.name.clone(),
                    app_id: shared.app_id,
                });
            }
            let owner = Owner::SharedUser(shared.name.clone());
            if ids.get(shared.app_id) != Some(&owner) {
                ids.register_existing(shared.app_id, owner)?;
            }
        }
        let mut packages = BTreeSet::new();
        for package in &settings.packages {
            if !packages.insert(&package.name) {
                return Err(Error::DuplicatePackage(package.name.clone()));
            }
            if package.app_id <= 0 {
                return Err(Error::InvalidId {
                    name: package.name.clone(),
                    app_id: package.app_id,
                });
            }
            if package.shared_user {
                if !matches!(ids.get(package.uid_owner_id()), Some(Owner::SharedUser(_))) {
                    return Err(Error::MissingSharedUser {
                        package: package.name.clone(),
                        app_id: package.app_id,
                    });
                }
            } else {
                ids.register_existing(package.app_id, Owner::Package(package.name.clone()))?;
            }
        }
        Ok(ids)
    }

    pub fn get(&self, app_id: i32) -> Option<&Owner> {
        self.slots.get(&app_id)
    }

    /// Preserve the exact prior non-shared setting without freeing its slot
    /// or changing allocation extent/cursor. Only its current owner can detach.
    pub(in crate::package) fn detach(&mut self, value: DetachedSetting) -> Result<(), String> {
        let id = value.package.app_id;
        if id <= 0
            || value.package.shared_user
            || self.get(id) != Some(&Owner::Package(value.package.name.clone()))
            || self.detached.contains_key(&id)
            || value.users.keys().any(|id| *id < 0)
            || !value
                .user_aliases
                .iter()
                .all(|id| value.users.contains_key(id))
            || value
                .legacy
                .as_ref()
                .is_some_and(|state| state.app_id() != id)
        {
            return Err("detached setting differs from registered UID owner".into());
        }
        self.slots
            .insert(id, Owner::DetachedPackage(value.package.name.clone()));
        self.detached.insert(id, std::sync::Arc::new(value));
        Ok(())
    }

    pub fn detached_setting(&self, app_id: i32) -> Option<&DetachedSetting> {
        self.detached.get(&app_id).map(std::sync::Arc::as_ref)
    }

    pub(in crate::package) fn update_user_aliases(
        &mut self,
        name: &str,
        users: &BTreeMap<i32, crate::package::restrictions::UserState>,
    ) {
        for value in self
            .detached
            .values_mut()
            .filter(|v| v.package.name == name)
        {
            std::sync::Arc::make_mut(value).update_user_aliases(users);
        }
    }

    pub(in crate::package) fn detach_user_aliases(&mut self, name: &str) {
        for value in self
            .detached
            .values_mut()
            .filter(|v| v.package.name == name)
        {
            std::sync::Arc::make_mut(value).user_aliases.clear();
        }
    }

    pub(in crate::package) fn validate_detached(&self) -> Result<(), String> {
        for (id, owner) in &self.slots {
            if let Owner::DetachedPackage(name) = owner {
                let value = self
                    .detached_setting(*id)
                    .ok_or("detached UID setting is missing")?;
                if value.package.app_id != *id
                    || value.package.name != *name
                    || value.package.shared_user
                    || value.users.keys().any(|id| *id < 0)
                    || !value
                        .user_aliases
                        .iter()
                        .all(|id| value.users.contains_key(id))
                    || value
                        .legacy
                        .as_ref()
                        .is_some_and(|state| state.app_id() != *id)
                {
                    return Err("detached UID setting identity differs".into());
                }
            }
        }
        if self.detached.iter().any(|(id, value)| {
            self.get(*id) != Some(&Owner::DetachedPackage(value.package.name.clone()))
        }) {
            return Err("detached setting has no registered UID slot".into());
        }
        Ok(())
    }

    /// Matches registerExistingAppId, including preserved IDs outside
    /// the automatic allocation range. Validation belongs to Settings.
    pub fn register_existing(&mut self, app_id: i32, owner: Owner) -> Result<(), Error> {
        if let Owner::DetachedPackage(name) = &owner {
            return Err(Error::DetachedOwner { name: name.clone() });
        }
        if let Some(existing) = self.get(app_id) {
            return Err(Error::Occupied {
                app_id,
                existing: existing.clone(),
                requested: owner,
            });
        }
        if app_id >= FIRST_APPLICATION_UID {
            self.non_system_size = self
                .non_system_size
                .max(i64::from(app_id) - i64::from(FIRST_APPLICATION_UID) + 1);
        }
        self.slots.insert(app_id, owner);
        Ok(())
    }

    pub fn acquire(&mut self, owner: Owner) -> Result<i32, Error> {
        if let Owner::DetachedPackage(name) = &owner {
            return Err(Error::DetachedOwner { name: name.clone() });
        }
        let extent = i64::from(FIRST_APPLICATION_UID) + self.non_system_size;
        let mut candidate = i64::from(self.first_available);
        for (&id, _) in self.slots.range(self.first_available..) {
            if i64::from(id) >= extent || i64::from(id) > candidate {
                break;
            }
            candidate = i64::from(id) + 1;
        }
        let id = if candidate < extent {
            candidate as i32
        } else {
            if self.non_system_size > i64::from(LAST_APPLICATION_UID - FIRST_APPLICATION_UID) {
                return Err(Error::Exhausted);
            }
            let id = extent as i32;
            self.non_system_size += 1;
            id
        };
        self.slots.insert(id, owner);
        Ok(id)
    }

    /// Removing a slot advances the cursor even if it was empty. The
    /// array extent remains; a fresh restore resets the runtime cursor.
    pub fn remove(&mut self, app_id: i32) -> Option<Owner> {
        let old = self.slots.remove(&app_id);
        self.detached.remove(&app_id);
        self.first_available = self.first_available.max(app_id.wrapping_add(1));
        old
    }

    /// Shared-user migration replaces slot ownership without assigning
    /// a new ID. Existing array holes are valid replacement positions.
    pub fn replace(&mut self, app_id: i32, owner: Owner) -> Result<(), Error> {
        if let Owner::DetachedPackage(name) = &owner {
            return Err(Error::DetachedOwner { name: name.clone() });
        }
        if app_id >= FIRST_APPLICATION_UID
            && i64::from(app_id) - i64::from(FIRST_APPLICATION_UID) >= self.non_system_size
        {
            return Err(Error::OutsideArray { app_id });
        }
        self.slots.insert(app_id, owner);
        self.detached.remove(&app_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{Package, SharedUser};

    fn owner(name: &str) -> Owner {
        Owner::Package(name.into())
    }

    #[test]
    fn detached_slots_preserve_snapshots_and_exact_removal_replacement() {
        let mut ids = AppIds::default();
        ids.register_existing(10000, owner("old")).unwrap();
        let retained = DetachedSetting {
            package: Package {
                name: "old".into(),
                app_id: 10000,
                ..Default::default()
            },
            users: BTreeMap::new(),
            user_aliases: Default::default(),
            legacy: None,
            install_fixed: None,
            runtime: None,
        };
        let original = ids.clone();
        let mut foreign = retained.clone();
        foreign.package.app_id = 10001;
        assert!(ids.detach(foreign).is_err());
        assert_eq!(ids, original);
        ids.detach(retained.clone()).unwrap();
        ids.validate_detached().unwrap();
        assert_eq!(original.get(10000), Some(&owner("old")));
        assert_eq!(original.detached_setting(10000), None);
        assert_eq!(ids.detached_setting(10000), Some(&retained));
        assert!(ids.detach(retained.clone()).is_err());
        let captured = ids.clone();
        assert_eq!(ids.acquire(owner("next")), Ok(10001));
        assert_eq!(
            ids.remove(10000),
            Some(Owner::DetachedPackage("old".into()))
        );
        assert_eq!(ids.detached_setting(10000), None);
        assert_eq!(captured.detached_setting(10000), Some(&retained));
        let mut replaced = captured.clone();
        replaced.replace(10000, owner("replacement")).unwrap();
        assert_eq!(replaced.detached_setting(10000), None);
        replaced.validate_detached().unwrap();
        assert_eq!(captured.detached_setting(10000), Some(&retained));
        let mut invalid = captured.clone();
        invalid.detached.clear();
        assert!(invalid.validate_detached().is_err());
        let fake = Owner::DetachedPackage("fake".into());
        assert!(ids.acquire(fake.clone()).is_err());
        assert!(ids.register_existing(10002, fake.clone()).is_err());
        assert!(ids.replace(10001, fake).is_err());
    }

    #[test]
    fn allocation_preserves_imported_holes_and_runtime_deletion_cursor() {
        let mut ids = AppIds::default();
        ids.register_existing(10002, owner("existing")).unwrap();
        assert_eq!(ids.acquire(owner("a")), Ok(10000));
        assert_eq!(ids.acquire(owner("b")), Ok(10001));
        assert_eq!(ids.acquire(owner("c")), Ok(10003));
        let snapshot = ids.clone();
        assert_eq!(ids.remove(10001), Some(owner("b")));
        assert_eq!(ids.acquire(owner("d")), Ok(10004));
        assert_eq!(snapshot.get(10001), Some(&owner("b")));
        ids.replace(10001, Owner::SharedUser("group".into()))
            .unwrap();
        assert_eq!(ids.get(10001), Some(&Owner::SharedUser("group".into())));
        assert_eq!(
            ids.replace(10006, owner("bad")),
            Err(Error::OutsideArray { app_id: 10006 })
        );
    }

    #[test]
    fn restoration_registers_groups_once_and_preserves_all_package_ids() {
        let settings = Settings {
            shared_users: vec![SharedUser {
                name: "group".into(),
                app_id: 1000,
                ..Default::default()
            }],
            packages: vec![
                Package {
                    name: "shared-a".into(),
                    app_id: 1000,
                    shared_user: true,
                    ..Default::default()
                },
                Package {
                    name: "shared-b".into(),
                    app_id: 1000,
                    shared_user: true,
                    ..Default::default()
                },
                Package {
                    name: "app".into(),
                    app_id: 10002,
                    ..Default::default()
                },
                Package {
                    name: "sdk".into(),
                    app_id: 10003,
                    is_sdk_library: true,
                    ..Default::default()
                },
            ],
            disabled_system_packages: vec![Package {
                name: "app".into(),
                app_id: 10002,
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = settings.clone();
        let mut ids = AppIds::restore(&settings).unwrap();
        assert_eq!(ids.get(1000), Some(&Owner::SharedUser("group".into())));
        assert_eq!(ids.get(10002), Some(&owner("app")));
        assert_eq!(ids.get(-1), None);
        assert_eq!(ids.get(10003), Some(&owner("sdk")));
        assert_eq!(ids.acquire(owner("new")), Ok(10000));
        assert_eq!(settings, before);
        let mut invalid = settings.clone();
        invalid.packages[0].app_id = 1001;
        assert!(matches!(
            AppIds::restore(&invalid),
            Err(Error::MissingSharedUser { .. })
        ));
        invalid = settings.clone();
        invalid.packages[2].app_id = 1000;
        assert!(matches!(
            AppIds::restore(&invalid),
            Err(Error::Occupied { .. })
        ));
        invalid = settings.clone();
        invalid.packages[3].app_id = -1;
        assert!(matches!(
            AppIds::restore(&invalid),
            Err(Error::InvalidId { .. })
        ));
    }

    #[test]
    fn exhaustion_is_atomic_and_sparse_registration_keeps_original_extent_rules() {
        let mut ids = AppIds::default();
        for id in FIRST_APPLICATION_UID..=LAST_APPLICATION_UID {
            assert_eq!(ids.acquire(owner("full")), Ok(id));
        }
        let before = ids.clone();
        assert_eq!(ids.acquire(owner("overflow")), Err(Error::Exhausted));
        assert_eq!(ids, before);
        ids.remove(LAST_APPLICATION_UID);
        assert_eq!(ids.acquire(owner("not-reused")), Err(Error::Exhausted));
        let mut sparse = AppIds::default();
        sparse
            .register_existing(i32::MAX, owner("persisted"))
            .unwrap();
        assert_eq!(sparse.acquire(owner("gap")), Ok(10000));
        assert!(matches!(
            sparse.register_existing(i32::MAX, owner("collision")),
            Err(Error::Occupied { .. })
        ));
    }
}
