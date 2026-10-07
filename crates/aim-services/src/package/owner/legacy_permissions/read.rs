//! Settings package/shared install permissions at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::Migration;
use crate::package::{
    owner::app_ids::{AppIds, Owner},
    settings::{Package, ReadError, ReadOwners, Settings, SharedUser},
};
use aim_android_xml::{Element, pull::Reader};
use std::collections::BTreeMap;

/// Bind explicit SettingBase owners to the UID slots visible at each XML event.
/// Successful registration creates empty SettingBase owners in supplied maps.
/// Existing owners retain earlier permission state across repeats and retries.
pub struct InstallRead<'a, T> {
    pub users: &'a [i32],
    pub packages: &'a mut BTreeMap<String, Migration>,
    pub shared_users: &'a mut BTreeMap<String, Migration>,
    pub remaining: &'a mut T,
}

impl<T: ReadOwners> ReadOwners for InstallRead<'_, T> {
    fn start_attempt(&mut self, settings: &Settings, pending: &[Package]) -> Result<(), ReadError> {
        self.remaining.start_attempt(settings, pending)
    }

    fn package_registered(&mut self, package: &Package, created: bool) -> Result<(), ReadError> {
        // Pending objects are owned individually by PackageReadAttempt, never
        // inserted into the name-keyed UID owner map before attachment.
        if !package.shared_user {
            register(&mut *self.packages, &package.name, created)?;
        }
        self.remaining.package_registered(package, created)
    }

    fn shared_registered(&mut self, group: &SharedUser, created: bool) -> Result<(), ReadError> {
        register(&mut *self.shared_users, &group.name, created)?;
        self.remaining.shared_registered(group, created)
    }

    fn package_child(
        &mut self,
        package: &mut Package,
        reader: &mut Reader<'_>,
        start: &Element,
        ids: &AppIds,
    ) -> Result<bool, ReadError> {
        if start.name != "perms" {
            return self.remaining.package_child(package, reader, start, ids);
        }
        let target = if package.shared_user {
            ids.get(package.uid_owner_id()).cloned()
        } else {
            Some(Owner::Package(package.name.clone()))
        };
        let state = match target {
            Some(Owner::Package(name)) => self.packages.get_mut(&name),
            Some(Owner::SharedUser(name)) => self.shared_users.get_mut(&name),
            Some(Owner::DetachedPackage(_)) => {
                return Err(ReadError::Owner(
                    "detached legacy permission binding is unavailable".into(),
                ));
            }
            None => {
                // Original getSettingLPr(null) leaves the children in the package
                // loop; a later shared-user record does not replay these grants.
                return Ok(true);
            }
        }
        .ok_or_else(|| {
            ReadError::Owner("registered legacy permission owner is unavailable".into())
        })?;
        state.read_install_events(reader, self.users)?;
        package.install_permissions_fixed = true;
        Ok(true)
    }

    fn shared_child(
        &mut self,
        group: &mut SharedUser,
        reader: &mut Reader<'_>,
        start: &Element,
    ) -> Result<bool, ReadError> {
        if start.name != "perms" {
            return self.remaining.shared_child(group, reader, start);
        }
        self.shared_users
            .get_mut(&group.name)
            .ok_or_else(|| {
                ReadError::Owner("registered shared legacy permission owner is unavailable".into())
            })?
            .read_install_events(reader, self.users)?;
        Ok(true)
    }

    fn public_key(&mut self, encoded: &[u8]) -> Result<Option<Vec<u8>>, ReadError> {
        self.remaining.public_key(encoded)
    }

    fn global_record(
        &mut self,
        settings: &mut Settings,
        reader: &mut Reader<'_>,
        start: &Element,
    ) -> Result<bool, ReadError> {
        self.remaining.global_record(settings, reader, start)
    }
}

fn register(
    owners: &mut BTreeMap<String, Migration>,
    name: &str,
    created: bool,
) -> Result<(), ReadError> {
    use std::collections::btree_map::Entry;
    match (owners.entry(name.into()), created) {
        (Entry::Vacant(slot), true) => {
            slot.insert(Migration::default());
            Ok(())
        }
        (Entry::Occupied(_), false) => Ok(()),
        _ => Err(ReadError::Owner(
            "legacy constructor identity differs from registered setting".into(),
        )),
    }
}
