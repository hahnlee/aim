//! Live permission GIDs and AppsFilter compatibility for native query capture (#953).
use super::{Bridge, OwnerError, PermissionGidError, bridge};
use crate::package::{
    scan::SigningScan,
    scan_snapshot::query_state::{Context, PackageInputs},
    settings,
};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub enum QueryContextError {
    Input(String),
    Compatibility(OwnerError),
    Gids(PermissionGidError),
}

impl Bridge {
    pub fn application_query_filtering(
        &self,
        name: &str,
        target_sdk: i32,
    ) -> Result<bool, OwnerError> {
        if name.is_empty() || target_sdk < 0 {
            return Err(OwnerError::Code(
                "invalid query compatibility identity".into(),
            ));
        }
        let mut data = Parcel::new();
        bridge::IsApplicationQueryFilteringEnabled {
            package_name: Some(name.into()),
            target_sdk,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(bridge::IS_APPLICATION_QUERY_FILTERING_ENABLED, &data, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let enabled = bridge::read_is_application_query_filtering_enabled_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(enabled)
    }

    /// Validate the complete inventory before asking external owners. Grant,
    /// installed-definition and domain inputs remain explicit separate owners.
    pub fn resolve_query_context(
        &self,
        owner: &SigningScan,
        mut context: Context,
    ) -> Result<Context, QueryContextError> {
        let mut packages = BTreeMap::new();
        for (settings, factory) in [
            (&owner.settings.packages, false),
            (&owner.settings.disabled_system_packages, true),
        ] {
            for setting in settings {
                let users = if factory {
                    owner.disabled_user_states(&setting.name)
                } else {
                    owner.scanned_user_states(&setting.name)
                }
                .ok_or_else(|| QueryContextError::Input("missing query user owner".into()))?;
                packages.insert((setting.name.clone(), factory), (setting, users));
            }
        }
        let mut retained = BTreeMap::new();
        for (id, _) in owner.identities.ids.owners() {
            if let Some(old) = owner.identities.ids.detached_setting(id) {
                retained.insert((id, old.package.name.clone()), (&old.package, &old.users));
            }
        }
        for group in owner.identities.shared_users.values() {
            for (name, old) in group.retained_settings() {
                retained.insert((group.app_id, name.into()), (&old.package, &old.users));
            }
        }
        validate(&packages, &context.packages, &context.users)?;
        validate(&retained, &context.retained_packages, &context.users)?;
        for (key, extra) in &mut context.packages {
            self.resolve_package(packages[key].0, extra)?;
        }
        for (key, extra) in &mut context.retained_packages {
            self.resolve_package(retained[key].0, extra)?;
        }
        Ok(context)
    }

    fn resolve_package(
        &self,
        setting: &settings::Package,
        extra: &mut PackageInputs,
    ) -> Result<(), QueryContextError> {
        extra.filter_application_query = self
            .application_query_filtering(&setting.name, setting.target_sdk_version)
            .map_err(QueryContextError::Compatibility)?;
        for (id, user) in &mut extra.users {
            user.gids = self
                .permission_gids(setting.app_id, &[*id])
                .map_err(QueryContextError::Gids)?
                .into_iter()
                .map(|gid| gid as i32)
                .collect();
        }
        Ok(())
    }
}

fn validate<K: Ord>(
    inventory: &BTreeMap<
        K,
        (
            &settings::Package,
            &BTreeMap<i32, crate::package::restrictions::UserState>,
        ),
    >,
    inputs: &BTreeMap<K, PackageInputs>,
    users: &BTreeMap<i32, crate::package::model::User>,
) -> Result<(), QueryContextError> {
    let fail = |message: &str| QueryContextError::Input(message.into());
    if inventory.keys().ne(inputs.keys())
        || users.iter().any(|(id, user)| *id < 0 || user.id != *id)
    {
        return Err(fail("query owner inventory differs"));
    }
    for (key, (setting, stored)) in inventory {
        let extra = &inputs[key];
        if (extra.app_id, extra.path.as_str(), extra.version)
            != (
                setting.app_id,
                setting.code_path.as_str(),
                setting.version_code,
            )
        {
            return Err(fail("query package identity differs"));
        }
        let ids: BTreeSet<_> = stored.keys().chain(users.keys()).copied().collect();
        if ids.iter().any(|id| *id < 0)
            || extra.users.keys().copied().collect::<BTreeSet<_>>() != ids
        {
            return Err(fail("query permission users differ"));
        }
    }
    Ok(())
}
