//! Settings package/shared install permissions at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::Migration;
use crate::package::{
    owner::app_ids::{AppIds, Owner},
    settings::{Package, ReadError, ReadOwners, Settings, SharedUser},
};
use aim_android_xml::{Element, pull::Reader};
use std::collections::{BTreeMap, BTreeSet};

/// Bind explicit SettingBase owners to the UID slots visible at each XML event.
/// Constructors supply the maps; unavailable native owners fail explicitly.
pub struct InstallRead<'a, T> {
    pub users: &'a [i32],
    pub packages: &'a mut BTreeMap<String, Migration>,
    pub shared_users: &'a mut BTreeMap<String, Migration>,
    pub install_permissions_fixed: &'a mut BTreeSet<String>,
    pub remaining: &'a mut T,
}

impl<T: ReadOwners> ReadOwners for InstallRead<'_, T> {
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
        self.install_permissions_fixed.insert(package.name.clone());
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
