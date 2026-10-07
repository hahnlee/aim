//! Settings runtime maps projected from the current permission producer.
//! Ported from Android 16 Settings, Copyright (C) The Android Open Source
//! Project, Apache License 2.0.
use super::Bridge;
use crate::package::{
    permissions::{RuntimePermission, RuntimePermissions},
    scan_snapshot::query_state::Capture,
};

impl Bridge {
    /// Version/fingerprint belong to the persistence owner. Live permission
    /// states are fetched here, never substituted with saved migration records.
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
        let current = |app_id| -> Result<Vec<RuntimePermission>, String> {
            let state = self
                .legacy_permissions(app_id, &[user])
                .map_err(|error| format!("live runtime permission owner: {error:?}"))?;
            let state = state
                .user(user)
                .ok_or("missing live runtime permission user")?;
            Ok(state
                .permissions
                .iter()
                .map(|p| RuntimePermission {
                    name: p.name.clone(),
                    granted: p.granted,
                    flags: p.flags,
                })
                .collect())
        };
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
            let permissions = current(ps.app_id)?;
            if !permissions.is_empty() || ps.is.install_permissions_fixed {
                output.packages.push((Some(ps.name.clone()), permissions));
            }
        }
        let mut groups: Vec<_> = model.shared_users.values().collect();
        groups.sort_by_key(|g| crate::package::info::java_hash(&g.name));
        for group in groups {
            output
                .shared_users
                .push((Some(group.name.clone()), current(group.app_id)?));
        }
        Ok(output)
    }
}
