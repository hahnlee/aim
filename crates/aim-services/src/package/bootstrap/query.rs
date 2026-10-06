//! Live permission definitions, grants, GIDs and AppsFilter compatibility for native query capture (#953).
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
    Permissions(OwnerError),
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

    pub fn domain_verification_restricted(
        &self,
        name: &str,
        target_sdk: i32,
    ) -> Result<bool, OwnerError> {
        if name.is_empty() || target_sdk < 0 {
            return Err(OwnerError::Code(
                "invalid domain compatibility identity".into(),
            ));
        }
        let mut request = Parcel::new();
        bridge::IsDomainVerificationRestricted {
            package_name: Some(name.into()),
            target_sdk,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::IS_DOMAIN_VERIFICATION_RESTRICTED, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let enabled = bridge::read_is_domain_verification_restricted_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(enabled)
    }

    pub fn domain_policies(
        &self,
        owner: &SigningScan,
    ) -> Result<BTreeMap<String, bool>, QueryContextError> {
        let mut inputs = Vec::new();
        for setting in &owner.settings.packages {
            let code = owner
                .loaded_packages()
                .get(&setting.name)
                .ok_or_else(|| QueryContextError::Input("missing domain policy code".into()))?;
            if code.package.package_name != setting.name || code.package.target_sdk_version < 0 {
                return Err(QueryContextError::Input(
                    "invalid domain policy code identity".into(),
                ));
            }
            inputs.push((&setting.name, code.package.target_sdk_version));
        }
        inputs
            .into_iter()
            .map(|(name, sdk)| {
                self.domain_verification_restricted(name, sdk)
                    .map(|value| (name.clone(), value))
                    .map_err(QueryContextError::Compatibility)
            })
            .collect()
    }

    pub fn resolve_domain_query_context(
        &self,
        owner: &SigningScan,
        context: Context,
        domains: &crate::package::domain_verification::owner::Owner,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<Context, QueryContextError> {
        let policies = self.domain_policies(owner)?;
        context
            .resolve_domains(owner, domains, config, &policies)
            .map_err(QueryContextError::Input)
    }

    pub fn installed_permissions(&self, name: &str) -> Result<Vec<String>, OwnerError> {
        if name.is_empty() {
            return Err(OwnerError::Code("missing permission package".into()));
        }
        let mut request = Parcel::new();
        bridge::GetPackageInstalledPermissions {
            package_name: Some(name.into()),
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::GET_PACKAGE_INSTALLED_PERMISSIONS, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let names = bridge::read_get_package_installed_permissions_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        permission_names(names, reader.remaining())
    }

    pub fn granted_permissions(
        &self,
        name: &str,
        app_id: i32,
        user_id: i32,
    ) -> Result<Vec<String>, OwnerError> {
        if name.is_empty() || !(0..100_000).contains(&app_id) || user_id < 0 {
            return Err(OwnerError::Code("invalid permission identity".into()));
        }
        let mut request = Parcel::new();
        bridge::GetPackageGrantedPermissions {
            package_name: Some(name.into()),
            app_id,
            user_id,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(bridge::GET_PACKAGE_GRANTED_PERMISSIONS, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let names = bridge::read_get_package_granted_permissions_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        permission_names(names, reader.remaining())
    }

    /// Validate the complete inventory before asking external owners.
    /// Domain and global context inputs remain explicit separate owners.
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
            self.resolve_package(packages[key].0, packages[key].1, extra)?;
        }
        for (key, extra) in &mut context.retained_packages {
            self.resolve_package(retained[key].0, retained[key].1, extra)?;
        }
        Ok(context)
    }

    fn resolve_package(
        &self,
        setting: &settings::Package,
        stored: &BTreeMap<i32, crate::package::restrictions::UserState>,
        extra: &mut PackageInputs,
    ) -> Result<(), QueryContextError> {
        extra.installed_permissions = self
            .installed_permissions(&setting.name)
            .map_err(QueryContextError::Permissions)?;
        extra.filter_application_query = self
            .application_query_filtering(&setting.name, setting.target_sdk_version)
            .map_err(QueryContextError::Compatibility)?;
        for (id, user) in &mut extra.users {
            user.granted_permissions = if stored.get(id).is_none_or(|state| state.installed) {
                self.granted_permissions(&setting.name, setting.app_id, *id)
                    .map_err(QueryContextError::Permissions)?
            } else {
                vec![]
            };
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

fn permission_names(
    names: Option<Vec<Option<String>>>,
    remaining: usize,
) -> Result<Vec<String>, OwnerError> {
    if remaining != 0 {
        return Err(OwnerError::Transport(BAD_VALUE));
    }
    let names = names.ok_or_else(|| OwnerError::Code("missing permission names".into()))?;
    let names = names
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| OwnerError::Code("null permission name".into()))?;
    if names.iter().collect::<BTreeSet<_>>().len() != names.len() {
        return Err(OwnerError::Code("duplicate permission name".into()));
    }
    Ok(names)
}
