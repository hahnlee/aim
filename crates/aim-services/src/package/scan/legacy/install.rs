//! PermissionManagerServiceInternal's completed live install projection.
use super::{Migration, SigningScan, State};
use std::collections::BTreeMap;

impl SigningScan {
    /// UserManager supplies the transition, including pre-created users. Saved
    /// grants are retained on removal exactly as original migration objects are;
    /// the projection inventory alone is changed before the new scan is captured.
    pub(crate) fn transition_permission_user_inventory(&mut self, user: i32, created: bool,
        bridge: &crate::package::bootstrap::Bridge) -> Result<(), String> {
        if user < 0 { return Err("negative permission inventory user".into()); }
        let mut users = self.legacy_permissions.as_ref().ok_or("legacy permission owner unavailable")?.users.clone();
        if created {
            if users.contains(&user) { return Err("created permission inventory user already exists".into()); }
            users.push(user); users.sort();
        } else {
            if !users.contains(&user) { return Err("removed permission inventory user is unknown".into()); }
            users.retain(|id| *id != user);
        }
        let mut candidate = self.clone();
        candidate.set_legacy_user_inventory(&users)?;
        candidate.identities.ids.capture_retained_permission_inventory(bridge, &users)?;
        for group in candidate.identities.shared_users.values_mut() {
            group.capture_retained_permission_inventory(bridge, &users)?;
        }
        *self = candidate;
        Ok(())
    }
    pub(crate) fn restore_permission_user_from_data(&mut self, data: &std::path::Path,
        state: &crate::package::State, config: &crate::package::system_config::SystemConfig,
        user: i32) -> Result<crate::package::owner::runtime_metadata::State, String> {
        self.validate_legacy_permissions()?;
        let restored = crate::package::owner::legacy_permissions::restore::read(data, state, config)?;
        let mut candidate = self.clone();
        let owners = candidate.legacy_permissions.as_mut().ok_or("legacy permission owner unavailable")?;
        if !owners.users.contains(&user) { return Err("permission read user is outside native inventory".into()); }
        let user_inventory = owners.users.clone();
        let replace_user = |old: &Migration, saved: &Migration| -> Result<Migration, String> {
            let mut next = Migration::default();
            for id in &user_inventory {
                let source = if *id == user { saved } else { old };
                next.set_missing(*id, source.is_missing(*id)?)?;
                for permission in source.permissions(*id)? { next.put(*id, permission.clone())?; }
            }
            Ok(next)
        };
        for (key, (_, _, value)) in &mut owners.packages {
            let saved = restored.packages.get(key).ok_or("permission read saved package owner missing")?;
            *value = replace_user(value, saved)?;
        }
        for (name, (_, value)) in &mut owners.shared_users {
            let saved = restored.shared_users.get(name).ok_or("permission read saved shared owner missing")?;
            *value = replace_user(value, saved)?;
        }
        let saved_metadata = restored.metadata.users.get(&user).cloned().ok_or("permission read user metadata missing")?;
        if let Some(metadata) = &mut owners.restoration { metadata.users.insert(user, saved_metadata.clone()); }
        let mut metadata = crate::package::owner::legacy_permissions::Metadata {
            users: BTreeMap::from([(user, saved_metadata)]), install_permissions_fixed: Default::default(),
        };
        for name in restored.metadata.install_permissions_fixed {
            if let Some(fixed) = owners.install_fixed.as_mut().and_then(|fixed| fixed.get_mut(&(name.clone(), false))) { *fixed = true; }
            metadata.install_permissions_fixed.insert(name);
        }
        candidate.validate_legacy_permissions()?;
        let mut runtime = crate::package::owner::runtime_metadata::State::default();
        runtime.restore(&metadata);
        *self = candidate;
        Ok(runtime)
    }
    /// Retain actual modern permission receipts independently of persisted
    /// SettingBase migrations. The original modern TEMP read/write methods do
    /// not copy these computed values into Settings legacy objects.
    pub fn apply_installed_permission_states(
        &mut self,
        users: &[i32],
        states: BTreeMap<i32, State>,
    ) -> Result<(), String> {
        self.validate_legacy_permissions()?;
        let owners = self.legacy_permissions.as_ref().ok_or("legacy permission owner unavailable")?;
        if owners.users != users { return Err("permission install user inventory changed".into()); }

        for (app_id, state) in &states {
            if *app_id != state.app_id() || state.users().iter().map(|u| u.id).ne(users.iter().copied()) {
                return Err("permission install UID projection differs".into());
            }
            if !owners.packages.values().any(|(id, _, _)| id == app_id)
                && !owners.shared_users.values().any(|(id, _)| id == app_id) {
                return Err("permission install contains unknown UID".into());
            }
        }
        let owners = self.legacy_permissions.as_mut().unwrap();
        owners.installed_receipt = states.keys().copied().collect();
        owners.modern_receipts.extend(states);
        self.validate_legacy_permissions()
    }
}
