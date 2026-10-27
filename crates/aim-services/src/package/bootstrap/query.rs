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
    pub fn is_domain_verifier_uid(&self, uid: i32) -> Result<bool, OwnerError> {
        if uid < 0 {
            return Err(OwnerError::Code("invalid domain verifier UID".into()));
        }
        let mut request = Parcel::new();
        bridge::IsDomainVerifierUid { uid }.write(&mut request);
        let reply = self
            .owner
            .transact_as_owner(bridge::IS_DOMAIN_VERIFIER_UID, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let verified = bridge::read_is_domain_verifier_uid_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(verified)
    }

    pub fn invalidate_package_info_cache(&self) -> Result<(), OwnerError> {
        let mut request = Parcel::new();
        bridge::InvalidatePackageInfoCache {}.write(&mut request);
        let reply = self
            .owner
            .transact_as_owner(bridge::INVALIDATE_PACKAGE_INFO_CACHE, &request, false)
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        bridge::read_invalidate_package_info_cache_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(())
    }

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
        let (code, data) = application_query_filtering_call(name, target_sdk);
        let reply = self.read(code, &data).map_err(OwnerError::Transport)?;
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
        let (code, request) = domain_verification_restricted_call(name, target_sdk);
        let reply = self.read(code, &request).map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let enabled = bridge::read_is_domain_verification_restricted_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(enabled)
    }

    pub fn domain_verification_settings_v2(
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
        bridge::IsDomainVerificationSettingsV2Enabled {
            package_name: Some(name.into()),
            target_sdk,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact_as_owner(
                bridge::IS_DOMAIN_VERIFICATION_SETTINGS_V2ENABLED,
                &request,
                false,
            )
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let enabled = bridge::read_is_domain_verification_settings_v2enabled_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(enabled)
    }

    pub fn domain_uuid_strict_validation(&self) -> Result<bool, OwnerError> {
        let mut request = Parcel::new();
        bridge::IsDomainSetUuidStrictValidationEnabled {}.write(&mut request);
        let reply = self
            .owner
            .transact_as_owner(
                bridge::IS_DOMAIN_SET_UUID_STRICT_VALIDATION_ENABLED,
                &request,
                false,
            )
            .map_err(OwnerError::Transport)?;
        let mut reader = reply.reader();
        let value = bridge::read_is_domain_set_uuid_strict_validation_enabled_reply(&mut reader)
            .map_err(OwnerError::Transport)?
            .map_err(OwnerError::Owner)?;
        if reader.remaining() != 0 {
            return Err(OwnerError::Transport(BAD_VALUE));
        }
        Ok(value)
    }

    pub fn domain_policies(
        &self,
        owner: &SigningScan,
    ) -> Result<BTreeMap<String, bool>, QueryContextError> {
        let calls = owner
            .settings
            .packages
            .iter()
            .filter_map(|setting| owner.domain_code_owner(setting).ok().flatten())
            .map(|code| {
                domain_verification_restricted_call(
                    &code.package.package_name,
                    code.package.target_sdk_version,
                )
            })
            .collect();
        self.read_ahead(calls, || self.read_domain_policies(owner))
    }

    fn read_domain_policies(
        &self,
        owner: &SigningScan,
    ) -> Result<BTreeMap<String, bool>, QueryContextError> {
        let mut inputs = Vec::new();
        for setting in &owner.settings.packages {
            let Some(code) = owner
                .domain_code_owner(setting)
                .map_err(QueryContextError::Input)?
            else {
                continue;
            };
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

    pub fn boot_domains(
        &self,
        owner: &SigningScan,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<crate::package::domain_verification::owner::Boot, QueryContextError> {
        let policies = self.domain_policies(owner)?;
        crate::package::domain_verification::owner::Owner::from_boot(owner, config, &policies)
            .map_err(QueryContextError::Input)
    }

    pub fn resolve_boot_domain_query_context(
        &self,
        owner: &SigningScan,
        context: Context,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<Context, QueryContextError> {
        let policies = self.domain_policies(owner)?;
        let boot =
            crate::package::domain_verification::owner::Owner::from_boot(owner, config, &policies)
                .map_err(QueryContextError::Input)?;
        context
            .attach_boot_domains(owner, boot, config, &policies)
            .map_err(QueryContextError::Input)
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
        let (code, request) = installed_permissions_call(name);
        let reply = self.read(code, &request).map_err(OwnerError::Transport)?;
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
        let (code, request) = granted_permissions_call(name, app_id, user_id);
        let reply = self.read(code, &request).map_err(OwnerError::Transport)?;
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
        let mut permission_ids = BTreeMap::new();
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
                let code = if factory {
                    owner.disabled_loaded_packages()
                } else {
                    owner.loaded_packages()
                }
                .get(&setting.name);
                let id = crate::package::scan_snapshot::boot_context::permission_uid_owner(
                    setting,
                    code.map(|code| code.runtime_package()),
                )
                .map_err(QueryContextError::Input)?;
                permission_ids.insert((setting.name.clone(), factory), id);
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
        let mut calls = Vec::new();
        for (setting, stored, extra, permission_id) in context
            .packages
            .iter()
            .map(|(key, extra)| (packages[key].0, packages[key].1, extra, permission_ids[key]))
            .chain(
                context
                    .retained_packages
                    .iter()
                    .map(|(key, extra)| (retained[key].0, retained[key].1, extra, Some(key.0))),
            )
        {
            calls.push(installed_permissions_call(&setting.name));
            calls.push(application_query_filtering_call(
                &setting.name,
                setting.target_sdk_version,
            ));
            for id in extra.users.keys() {
                if let Some(permission_id) = permission_id {
                    if stored.get(id).is_none_or(|state| state.installed) {
                        calls.push(granted_permissions_call(&setting.name, permission_id, *id));
                    }
                    calls.extend(super::permission_gids_calls(permission_id, &[*id]));
                }
            }
        }
        self.read_ahead(calls, || {
            for (key, extra) in &mut context.packages {
                self.resolve_package(packages[key].0, packages[key].1, permission_ids[key], extra)?;
            }
            for (key, extra) in &mut context.retained_packages {
                self.resolve_package(retained[key].0, retained[key].1, Some(key.0), extra)?;
            }
            Ok(())
        })?;
        Ok(context)
    }

    fn resolve_package(
        &self,
        setting: &settings::Package,
        stored: &BTreeMap<i32, crate::package::restrictions::UserState>,
        permission_id: Option<i32>,
        extra: &mut PackageInputs,
    ) -> Result<(), QueryContextError> {
        extra.installed_permissions = self
            .installed_permissions(&setting.name)
            .map_err(QueryContextError::Permissions)?;
        extra.filter_application_query = self
            .application_query_filtering(&setting.name, setting.target_sdk_version)
            .map_err(QueryContextError::Compatibility)?;
        for (id, user) in &mut extra.users {
            let Some(permission_id) = permission_id else {
                user.granted_permissions.clear();
                user.gids.clear();
                continue;
            };
            user.granted_permissions = if stored.get(id).is_none_or(|state| state.installed) {
                self.granted_permissions(&setting.name, permission_id, *id)
                    .map_err(QueryContextError::Permissions)?
            } else {
                vec![]
            };
            user.gids = self
                .permission_gids(permission_id, &[*id])
                .map_err(QueryContextError::Gids)?
                .into_iter()
                .map(|gid| gid as i32)
                .collect();
        }
        Ok(())
    }
}

pub fn application_query_filtering_call(name: &str, target_sdk: i32) -> (u32, Parcel) {
    let mut data = Parcel::new();
    bridge::IsApplicationQueryFilteringEnabled {
        package_name: Some(name.into()),
        target_sdk,
    }
    .write(&mut data);
    (bridge::IS_APPLICATION_QUERY_FILTERING_ENABLED, data)
}

pub fn domain_verification_restricted_call(name: &str, target_sdk: i32) -> (u32, Parcel) {
    let mut data = Parcel::new();
    bridge::IsDomainVerificationRestricted {
        package_name: Some(name.into()),
        target_sdk,
    }
    .write(&mut data);
    (bridge::IS_DOMAIN_VERIFICATION_RESTRICTED, data)
}

pub fn installed_permissions_call(name: &str) -> (u32, Parcel) {
    let mut data = Parcel::new();
    bridge::GetPackageInstalledPermissions {
        package_name: Some(name.into()),
    }
    .write(&mut data);
    (bridge::GET_PACKAGE_INSTALLED_PERMISSIONS, data)
}

pub fn granted_permissions_call(name: &str, app_id: i32, user_id: i32) -> (u32, Parcel) {
    let mut data = Parcel::new();
    bridge::GetPackageGrantedPermissions {
        package_name: Some(name.into()),
        app_id,
        user_id,
    }
    .write(&mut data);
    (bridge::GET_PACKAGE_GRANTED_PERMISSIONS, data)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_refresh_uses_boot_uid_identity_for_shared_and_uidless_apex() {
        crate::system::with_discovery_session(|host, _, _| {
            let system = host.package_system().unwrap();
            let bridge = system.package_bootstrap().unwrap();
            let capture = system.capture_package_queries().unwrap();
            for shared in [true, false] {
                let mut owner = capture.scan().owner().clone();
                let setting = owner
                    .settings
                    .packages
                    .iter_mut()
                    .find(|p| p.name == "p")
                    .unwrap();
                setting.app_id = -1;
                setting.shared_user = shared;
                setting.shared_user_app_id = shared.then_some(10100);
                setting.code_path = "/apex/p".into();
                let mut code = (*capture.state().packages["p"].pkg.as_ref().unwrap())
                    .as_ref()
                    .clone();
                code.path = Some("/apex/p".into());
                code.uid = -1;
                code.booleans2 |= crate::package::pkg::booleans2::APEX;
                owner.load_test_code(code);
                let mut context = (**capture.context()).clone();
                let inputs = context.packages.get_mut(&("p".into(), false)).unwrap();
                inputs.app_id = -1;
                inputs.path = "/apex/p".into();
                inputs.users.get_mut(&0).unwrap().gids = vec![9999];
                inputs.users.get_mut(&0).unwrap().granted_permissions = vec!["stale".into()];
                let refreshed = bridge
                    .resolve_query_context(&owner, context.clone())
                    .unwrap();
                let inputs = &refreshed.packages[&("p".into(), false)];
                assert_eq!(inputs.app_id, -1);
                assert!(inputs.users[&0].gids.is_empty());
                assert!(inputs.users[&0].granted_permissions.is_empty());
                if !shared {
                    let mut code = owner.loaded_packages()["p"].runtime_package().clone();
                    code.booleans2 &= !crate::package::pkg::booleans2::APEX;
                    owner.load_test_code(code);
                    assert!(matches!(
                        bridge.resolve_query_context(&owner, context),
                        Err(QueryContextError::Input(_))
                    ));
                }
            }
        });
    }
}
