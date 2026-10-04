//! SettingBase's legacy migration owner, separate from the live exporter.
//! Ported from pinned LegacyPermissionState and Settings permission readers.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Error, Permission, State, User, validate_setting};
use aim_android_xml::Element;
use std::collections::{BTreeMap, BTreeSet};

/// A new SettingBase constructs an empty owner. Its disk restore and permission
/// initialization are explicit operations; this is not a missing-live-state default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migration {
    users: BTreeMap<i32, Vec<Permission>>,
    missing: BTreeSet<i32>,
}

fn user_id(user: i32) -> Result<(), String> {
    if user < 0 {
        Err(format!("Invalid user ID {user}"))
    } else {
        Ok(())
    }
}

impl Migration {
    pub fn permissions(&self, user: i32) -> Result<&[Permission], String> {
        user_id(user)?;
        Ok(self.users.get(&user).map(Vec::as_slice).unwrap_or(&[]))
    }
    pub fn permission(&self, user: i32, name: Option<&str>) -> Result<Option<&Permission>, String> {
        Ok(self
            .permissions(user)?
            .iter()
            .find(|permission| permission.name.as_deref() == name))
    }
    pub fn has_permission_state(&self, names: &[Option<&str>]) -> bool {
        self.users.values().any(|permissions| {
            permissions
                .iter()
                .any(|p| names.contains(&p.name.as_deref()))
        })
    }
    pub fn put(&mut self, user: i32, permission: Permission) -> Result<(), String> {
        user_id(user)?;
        let permissions = self.users.entry(user).or_default();
        if let Some(old) = permissions
            .iter_mut()
            .find(|old| old.name == permission.name)
        {
            *old = permission;
        } else {
            permissions.push(permission);
            // Stable sort retains insertion order for equal signed Java hashes.
            permissions.sort_by_key(|p| {
                p.name
                    .as_deref()
                    .map(crate::package::parse::parcel::java_hash)
                    .unwrap_or(0)
            });
        }
        Ok(())
    }
    pub fn is_missing(&self, user: i32) -> Result<bool, String> {
        user_id(user)?;
        Ok(self.missing.contains(&user))
    }
    pub fn set_missing(&mut self, user: i32, missing: bool) -> Result<(), String> {
        user_id(user)?;
        if missing {
            self.missing.insert(user);
        } else {
            self.missing.remove(&user);
        }
        Ok(())
    }
    pub fn reset(&mut self) {
        self.users.clear();
        self.missing.clear();
    }

    /// Settings.readInstallPermissionsLPr: every install permission goes to
    /// every enumerated user. Unknown subtrees are skipped; nested items are read.
    pub fn read_install(&mut self, root: &Element, users: &[i32]) -> Result<(), String> {
        for item in root.children() {
            if item.name != "item" {
                continue;
            }
            let permission = xml_permission(item, false);
            for &user in users {
                self.put(user, permission.clone())?;
            }
            self.read_install(item, users)?;
        }
        Ok(())
    }

    /// Settings.parseLegacyPermissionsLPr: unknown elements are visited rather
    /// than skipped. Existing install permission names are overwritten in place.
    pub fn read_legacy_runtime(&mut self, root: &Element, user: i32) -> Result<(), String> {
        for item in root.children() {
            if item.name == "item" {
                self.put(user, xml_permission(item, true))?;
            }
            self.read_legacy_runtime(item, user)?;
        }
        Ok(())
    }

    /// Settings.readPermissionsState consumes the modern persistence API's
    /// decoded states; it merges with install states without clearing missing.
    pub fn read_runtime(
        &mut self,
        user: i32,
        states: &[crate::package::permissions::RuntimePermission],
    ) -> Result<(), String> {
        for state in states {
            self.put(
                user,
                Permission {
                    name: Some(state.name.clone()),
                    runtime: true,
                    granted: state.granted,
                    flags: state.flags,
                },
            )?;
        }
        Ok(())
    }

    pub fn project(&self, app_id: i32, users: &[i32]) -> Result<State, Error> {
        validate_setting(app_id, users)?;
        Ok(State {
            app_id,
            users: users
                .iter()
                .map(|&user| User {
                    id: user,
                    missing: self.missing.contains(&user),
                    permissions: self.users.get(&user).cloned().unwrap_or_default(),
                })
                .collect(),
        })
    }
}

fn xml_permission(item: &Element, runtime: bool) -> Permission {
    Permission {
        name: item.string("name").map(|name| name.into_owned()),
        runtime,
        // These are the original TypedXmlPullParser default-valued getters;
        // malformed attributes use the supplied default, as absent ones do.
        granted: item.bool("granted").ok().flatten().unwrap_or(true),
        flags: item.int_hex("flags").ok().flatten().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn xml(text: &str) -> Element {
        aim_android_xml::read_next(text.as_bytes()).unwrap()
    }

    #[test]
    fn legacy_restore_merges_by_name_and_preserves_each_users_original_state() {
        let mut migration = Migration::default();
        migration.set_missing(10, true).unwrap();
        migration.read_install(&xml("<perms><item name='BB'/><item/><item name=''/><item name='Aa' flags='-1'/><item name='BB' granted='false' flags='17'/><unknown><item name='ignored'/></unknown><item name='outer'><item name='inner'/></item></perms>"), &[10, 0]).unwrap();
        assert_eq!(
            migration.permission(10, Some("BB")).unwrap().unwrap().flags,
            0x17
        );
        assert!(
            !migration
                .permission(0, Some("BB"))
                .unwrap()
                .unwrap()
                .granted
        );
        assert!(migration.has_permission_state(&[None]));
        assert!(!migration.has_permission_state(&[Some("ignored")]));
        let old = migration.project(10042, &[10, 0, 11]).unwrap();
        migration.read_legacy_runtime(&xml("<permissions><unknown><item name='BB' flags='42'/></unknown><item name='runtime' granted='nonsense' flags='overflow'/></permissions>"), 10).unwrap();
        assert!(
            migration
                .permission(10, Some("BB"))
                .unwrap()
                .unwrap()
                .runtime
        );
        assert!(
            !migration
                .permission(0, Some("BB"))
                .unwrap()
                .unwrap()
                .runtime
        );
        assert_eq!(
            migration
                .permission(10, Some("runtime"))
                .unwrap()
                .unwrap()
                .flags,
            0
        );
        assert!(migration.is_missing(10).unwrap());
        let mut copied = migration.clone();
        copied.reset();
        assert!(copied.permissions(10).unwrap().is_empty());
        assert!(!copied.is_missing(10).unwrap());
        assert!(
            !old.user(10)
                .unwrap()
                .permissions
                .iter()
                .find(|p| p.name.as_deref() == Some("BB"))
                .unwrap()
                .runtime
        );
        let new = migration.project(10042, &[10, 0, 11]).unwrap();
        assert_eq!(State::read(&new.bytes(), 10042, &[10, 0, 11]).unwrap(), new);
    }

    #[test]
    fn invalid_uid_setting_owns_legacy_state_without_a_live_application_lookup() {
        let mut owner = Migration::default();
        owner.set_missing(10, true).unwrap();
        owner
            .put(
                0,
                Permission {
                    name: Some("camera".into()),
                    runtime: true,
                    granted: true,
                    flags: 17,
                },
            )
            .unwrap();
        let state = owner.project(-1, &[0, 10]).unwrap();
        assert_eq!(State::read(&state.bytes(), -1, &[0, 10]).unwrap(), state);
        assert!(state.user(10).unwrap().missing);
        assert!(state.user(0).unwrap().permissions[0].granted);
        assert!(super::super::validate(-1, &[0, 10]).is_err());
        assert!(owner.project(-2, &[0, 10]).is_err());
        assert!(State::read(&state.bytes(), 10000, &[0, 10]).is_err());
    }

    #[test]
    fn invalid_users_do_not_mutate_owner_and_modern_runtime_rows_merge_without_reset() {
        let mut state = Migration::default();
        let permission = Permission {
            name: None,
            runtime: false,
            granted: true,
            flags: -1,
        };
        assert!(state.put(-1, permission).is_err());
        assert!(state.set_missing(-1, true).is_err());
        assert_eq!(state, Migration::default());
        assert!(state.permissions(-1).is_err());
        assert!(state.is_missing(-1).is_err());
        state.set_missing(0, true).unwrap();
        state
            .read_runtime(
                0,
                &[crate::package::permissions::RuntimePermission {
                    name: "camera".into(),
                    granted: false,
                    flags: i32::MIN,
                }],
            )
            .unwrap();
        assert!(state.is_missing(0).unwrap());
        assert_eq!(
            state.permission(0, Some("camera")).unwrap().unwrap().flags,
            i32::MIN
        );
        state.set_missing(0, false).unwrap();
        assert!(!state.is_missing(0).unwrap());
    }
}
