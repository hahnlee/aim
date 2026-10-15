//! Settings compatibility runtime maps projected from retained SettingBase owners.
//! Ported from Android 16 Settings, Copyright (C) The Android Open Source
//! Project, Apache License 2.0.
use super::Bridge;
use crate::package::{
    permissions::{RuntimePermission, RuntimePermissions},
    scan_snapshot::query_state::Capture,
};

impl Bridge {
    /// Android 16 Settings.RuntimePermissionPersistence reads the SettingBase
    /// legacy maps after writeLegacyPermissionStateTEMP. PermissionService's
    /// TEMP method is a no-op; its computed modern getter is not this file's
    /// persistence owner. Live grants/GIDs continue through the separate exporter.
    /// Version/fingerprint remain supplied by the runtime metadata owner.
    pub fn runtime_permissions(
        &self,
        capture: &Capture,
        user: i32,
        version: i32,
        fingerprint: Option<String>,
    ) -> Result<RuntimePermissions, String> {
        let model = capture.state();
        if user < 0 || !model.users.contains_key(&user) {
            return Err("runtime permission user is outside the captured inventory".into());
        }
        let mut packages: Vec<_> = capture.scan().owner().settings.packages.iter().collect();
        packages.sort_by_key(|p| crate::package::info::java_hash(&p.name));
        let mut output = RuntimePermissions {
            version,
            fingerprint,
            ..Default::default()
        };
        for setting in packages {
            let ps = model
                .packages
                .get(&setting.name)
                .ok_or("missing current package runtime role")?;
            if ps.shared_user.is_some() {
                continue;
            }
            let state = capture
                .scan()
                .owner()
                .validated_legacy_permissions(&ps.name, false)?
                .ok_or_else(|| {
                    format!(
                        "missing persisted SettingBase permission owner: {}",
                        ps.name
                    )
                })?;
            let permissions = persisted_permissions(&state, ps.app_id, user)?;
            append_package(
                &mut output,
                &ps.name,
                ps.is.install_permissions_fixed,
                permissions,
            );
        }
        let mut groups: Vec<_> = model.shared_users.values().collect();
        groups.sort_by_key(|g| crate::package::info::java_hash(&g.name));
        for group in groups {
            output.shared_users.push((
                Some(group.name.clone()),
                persisted_permissions(
                    &capture
                        .scan()
                        .owner()
                        .shared_legacy_permissions(&group.name)?
                        .ok_or_else(|| {
                            format!("missing persisted shared permission owner: {}", group.name)
                        })?,
                    group.app_id,
                    user,
                )?,
            ));
        }
        Ok(output)
    }
}

fn append_package(
    output: &mut RuntimePermissions,
    name: &str,
    fixed: bool,
    permissions: Vec<RuntimePermission>,
) {
    if !permissions.is_empty() || fixed {
        output.packages.push((Some(name.into()), permissions));
    }
}

fn persisted_permissions(
    state: &crate::package::owner::legacy_permissions::State,
    app_id: i32,
    user: i32,
) -> Result<Vec<RuntimePermission>, String> {
    if state.app_id() != app_id {
        return Err("persisted permission SettingBase identity differs".into());
    }
    let state = state
        .user(user)
        .ok_or("persisted permission user is not captured")?;
    Ok(state
        .permissions
        .iter()
        .map(|permission| RuntimePermission {
            name: permission.name.clone(),
            granted: permission.granted,
            flags: permission.flags,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::legacy_permissions::{Migration, Permission};
    #[test]
    fn fresh_runtime_xml_omits_empty_packages_and_keeps_shared_owners_and_metadata() {
        let fingerprint = "fixture.partition?pc_version=330000000";
        let mut output = RuntimePermissions {
            version: 7,
            fingerprint: Some(fingerprint.into()),
            packages: Vec::new(),
            shared_users: Vec::new(),
        };
        let fresh = Migration::default().project(10100, &[0]).unwrap();
        append_package(
            &mut output,
            "fresh",
            false,
            persisted_permissions(&fresh, 10100, 0).unwrap(),
        );
        append_package(
            &mut output,
            "fixed",
            true,
            persisted_permissions(&fresh, 10100, 0).unwrap(),
        );
        let shared = Migration::default().project(1000, &[0]).unwrap();
        output.shared_users.push((
            Some("shared".into()),
            persisted_permissions(&shared, 1000, 0).unwrap(),
        ));
        let bytes = output.serialize().unwrap();
        let root = aim_android_xml::read(&bytes).unwrap();
        let parsed = RuntimePermissions::parse(&root).unwrap();
        assert_eq!(parsed, output);
        assert_eq!(parsed.version, 7);
        assert_eq!(parsed.fingerprint.as_deref(), Some(fingerprint));
        assert_eq!(parsed.packages.len(), 1);
        assert_eq!(parsed.packages[0].0.as_deref(), Some("fixed"));
        assert_eq!(parsed.shared_users.len(), 1);
        assert!(parsed.shared_users[0].1.is_empty());
    }
    #[test]
    fn persisted_runtime_rows_retain_restored_state_without_modern_receipts() {
        for app_id in [-1, 1000, 10100] {
            let mut saved = Migration::default();
            saved
                .put(
                    0,
                    Permission {
                        name: Some("saved.permission".into()),
                        runtime: true,
                        granted: true,
                        flags: 0x23,
                    },
                )
                .unwrap();
            let state = saved.project(app_id, &[0, 10]).unwrap();
            let rows = persisted_permissions(&state, app_id, 0).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].name.as_deref(), Some("saved.permission"));
            assert!(rows[0].granted);
            assert_eq!(rows[0].flags, 0x23);
            assert!(persisted_permissions(&state, app_id, 10)
                .unwrap()
                .is_empty());
            assert!(persisted_permissions(&state, app_id, 11).is_err());
            assert!(persisted_permissions(&state, 10101, 0).is_err());
            let fresh = Migration::default().project(app_id, &[0]).unwrap();
            assert!(persisted_permissions(&fresh, app_id, 0).unwrap().is_empty());
        }
    }
}
