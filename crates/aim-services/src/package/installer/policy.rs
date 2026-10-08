//! PackageInstallerService.createSessionInternal at android-16.0.0_r1.
//! Copyright AOSP, Apache License 2.0. External policy is captured explicitly.
use super::{Record, codec::SessionParams};
use crate::package::{
    apps_filter::{self, NotModelled},
    query::Query,
    system_config::SystemConfig,
};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception};
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub struct UserPolicy {
    pub disallow_install_apps: bool,
    pub disallow_debugging_features: bool,
    pub organization_managed: bool,
}
#[derive(Clone, Debug)]
pub struct DevicePolicy {
    pub debuggable: bool,
    pub apex_supported: bool,
    pub rollback_lifetime: bool,
    pub users: BTreeMap<i32, UserPolicy>,
    /// Native app-ops' current delegated shell identity.
    pub adopted_shell_uids: std::collections::BTreeSet<u32>,
    pub verifier_uid: Option<u32>,
}
#[derive(Clone, Debug, Default)]
pub struct ServicePolicy {
    pub disable_verification_for_uid: Option<i32>,
    pub bypass_next_staged_installer_check: bool,
    pub bypass_next_allowed_apex_update_check: bool,
}
pub fn unknown(error: NotModelled) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, error.0)
}
pub fn permission(q: &Query<'_>, name: &str) -> Result<bool, Exception> {
    match apps_filter::app_id(q.calling_uid) {
        0 | 1000 => Ok(true),
        _ if apps_filter::is_isolated(q.calling_uid) => Ok(false),
        _ => q.uid_has_permission(q.calling_uid, name).map_err(unknown),
    }
}
pub fn cross_user(
    q: &Query<'_>,
    policy: &DevicePolicy,
    user: i32,
    shell: bool,
    operation: &str,
) -> Result<(), Exception> {
    if user < 0 {
        return Err(Exception::illegal_argument(format!(
            "Invalid userId {user}"
        )));
    }
    if user != apps_filter::user_id(q.calling_uid)
        && !matches!(q.calling_uid, 0 | 1000)
        && !permission(q, "android.permission.INTERACT_ACROSS_USERS_FULL")?
    {
        return Err(Exception::security(format!(
            "{operation}: requires INTERACT_ACROSS_USERS_FULL"
        )));
    }
    if shell
        && q.calling_uid == 2000
        && policy
            .users
            .get(&user)
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "UserManager policy capture unavailable")
            })?
            .disallow_debugging_features
    {
        return Err(Exception::security(
            "Shell does not have permission to access user",
        ));
    }
    Ok(())
}
pub fn check_package(q: &Query<'_>, package: Option<&str>) -> Result<(), Exception> {
    let matches = q
        .packages_for_uid(q.calling_uid)
        .map_err(unknown)?
        .is_some_and(|names| names.iter().any(|name| name.as_deref() == package));
    if matches {
        Ok(())
    } else {
        Err(Exception::security(format!(
            "Package {package:?} does not belong to uid {}",
            q.calling_uid
        )))
    }
}
pub fn can_query(q: &Query<'_>, package: Option<&str>) -> Result<bool, Exception> {
    q.internal_can_query(q.calling_uid, package)
        .map_err(unknown)?
}

fn valid_name(value: &str) -> bool {
    if value.is_empty() || value == "." || value == ".." || value.encode_utf16().count() > 255 {
        return false;
    }
    let mut front = true;
    for c in value.chars() {
        if c.is_ascii_alphabetic() {
            front = false
        } else if !front && (c.is_ascii_digit() || c == '_') {
        } else if c == '.' {
            front = true
        } else {
            return false;
        }
    }
    true
}
/// Normalization changes flags before mode/storage validation, as the original does.
/// Icon resizing, incremental installation and archiving require their concrete owners.
pub fn normalize(
    q: &Query<'_>,
    device: &DevicePolicy,
    service: &mut ServicePolicy,
    config: &SystemConfig,
    mut params: SessionParams,
    mut installer: Option<String>,
    tag: Option<String>,
    user: i32,
    now: i64,
) -> Result<(Record, bool), Exception> {
    if let Some(error) = &config.installer_policy_error {
        return Err(Exception::new(EX_ILLEGAL_STATE, error));
    }
    if params.data_loader_params.is_some() {
        if !permission(q, "android.permission.USE_INSTALLER_V2")? {
            return Err(Exception::security(
                "You need USE_INSTALLER_V2 permission to use a data loader",
            ));
        }
    }
    params.install_flags &= !(1 << 29);
    cross_user(q, device, user, true, "createSession")?;
    let user_policy = device.users.get(&user).cloned().ok_or_else(|| {
        Exception::new(EX_ILLEGAL_STATE, "UserManager policy capture unavailable")
    })?;
    if user_policy.disallow_install_apps {
        return Err(Exception::security("User restriction prevents installing"));
    }
    if params.install_reason == 5
        && !permission(q, "android.permission.MANAGE_ROLLBACKS")?
        && !permission(q, "android.permission.TEST_MANAGE_ROLLBACKS")?
    {
        return Err(Exception::security(
            "INSTALL_REASON_ROLLBACK requires MANAGE_ROLLBACKS or TEST_MANAGE_ROLLBACKS",
        ));
    }
    if params
        .app_package_name
        .as_deref()
        .is_some_and(|name| !valid_name(name))
    {
        params.app_package_name = None;
    }
    if let Some(label) = &params.app_label {
        params.app_label = Some(
            label
                .chars()
                .scan(0usize, |count, c| {
                    *count += c.len_utf16();
                    Some((*count, c))
                })
                .take_while(|(count, _)| *count <= 1000)
                .map(|(_, c)| c)
                .collect(),
        );
    }
    if params
        .installer_package_name
        .as_deref()
        .is_some_and(|name| !valid_name(name))
    {
        params.installer_package_name = None;
    }
    if installer.as_deref().is_some_and(|name| !valid_name(name)) {
        installer = None;
    }
    let mut requested = params
        .installer_package_name
        .clone()
        .or_else(|| installer.clone());
    let uid = q.calling_uid as u32;
    let special = matches!(uid, 0 | 1000 | 2000);
    let adb = matches!(uid, 0 | 2000) || device.adopted_shell_uids.contains(&uid);
    let install_permission = permission(q, "android.permission.INSTALL_PACKAGES")?;
    if adb {
        params.install_flags |= 0x20;
        installer = Some("com.android.shell".into());
    } else {
        if uid != 1000 {
            check_package(q, installer.as_deref())?;
        }
        if requested != installer && !install_permission {
            check_package(q, requested.as_deref())?;
        }
        params.install_flags &= !(0x20 | 0x40 | (1 << 27));
        params.install_flags |= 2;
        if params.install_flags & 0x10000 != 0 && device.verifier_uid != Some(uid) {
            params.install_flags &= !0x10000;
        }
        if !permission(q, "android.permission.INSTALL_TEST_ONLY_PACKAGE")? {
            params.install_flags &= !4;
        }
        params.development_install_flags = 0;
    }
    if device.debuggable || matches!(uid, 0 | 1000) {
        params.install_flags |= 0x100000;
    } else {
        params.install_flags &= !0x100000;
    }
    if let Some(allowed) = service.disable_verification_for_uid.take() {
        if allowed == uid as i32 {
            params.install_flags |= 0x80000;
        } else {
            params.install_flags &= !0x80000;
        }
    } else if params.install_flags & (0x20 | 4) != (0x20 | 4) {
        params.install_flags &= !0x80000;
    }
    if device.rollback_lifetime {
        if params.rollback_lifetime_millis < 0 {
            return Err(Exception::illegal_argument(
                "rollbackLifetimeMillis can't be negative.",
            ));
        }
        if params.rollback_lifetime_millis > 0 {
            rollback(q, &params, "rollbackLifetimeMillis")?;
        }
    }
    if params.rollback_impact_level < 0 {
        return Err(Exception::illegal_argument(
            "rollbackImpactLevel can't be negative.",
        ));
    }
    if matches!(params.rollback_impact_level, 1 | 2) {
        rollback(q, &params, "rollbackImpactLevel")?;
    }
    let apex = params.install_flags & 0x20000 != 0;
    if apex && !permission(q, "android.permission.INSTALL_PACKAGE_UPDATES")? && !install_permission
    {
        return Err(Exception::security("Not allowed to perform APEX updates"));
    }
    if !apex && params.staged && !install_permission {
        return Err(Exception::security(
            "Staged install requires INSTALL_PACKAGES",
        ));
    }
    if apex {
        if !device.apex_supported {
            return Err(Exception::illegal_argument(
                "This device doesn't support the installation of APEX files",
            ));
        }
        if params.multi_package {
            return Err(Exception::illegal_argument(
                "A multi-session can't be set as APEX.",
            ));
        }
        if special || service.bypass_next_allowed_apex_update_check {
            params.install_flags |= 0x800000;
        } else {
            params.install_flags &= !0x800000;
        }
    }
    if !special && !device.debuggable && !device.adopted_shell_uids.contains(&uid) {
        params.install_flags &= !0x1000000;
    }
    params.install_flags &= !(1 << 30);
    if let Some(name) = &params.app_package_name
        && q.state.packages.get(name).is_some_and(|state| {
            crate::package::info::user_state(state, user)
                .archive_state
                .is_some()
        })
    {
        return Err(Exception::new(
            EX_UNSUPPORTED_OPERATION,
            "Native unarchive owner unavailable",
        ));
    }
    if params.install_flags & 0x800 != 0
        && !special
        && !q
            .packages_for_uid(uid as i32)
            .map_err(unknown)?
            .unwrap_or_default()
            .iter()
            .flatten()
            .filter_map(|name| q.state.packages.get(name))
            .any(|state| state.is.system)
    {
        return Err(Exception::security(
            "Only system apps could use INSTALL_INSTANT_APP",
        ));
    }
    if (params.staged || apex)
        && !special
        && !service.bypass_next_staged_installer_check
        && !requested
            .as_ref()
            .is_some_and(|name| config.staged_installers.contains(name))
    {
        return Err(Exception::security(
            "Installer not allowed to commit staged/APEX install",
        ));
    }
    service.bypass_next_staged_installer_check = false;
    service.bypass_next_allowed_apex_update_check = false;
    if !params.multi_package {
        let grant = permission(q, "android.permission.INSTALL_GRANT_RUNTIME_PERMISSIONS")?;
        if params.install_flags & 0x100 != 0 && !grant {
            return Err(Exception::security(
                "You need INSTALL_GRANT_RUNTIME_PERMISSIONS permission to grant all requested permissions",
            ));
        }
        if !grant
            && params.permission_states.iter().any(|(name, _)| {
                name.as_deref() != Some("android.permission.USE_FULL_SCREEN_INTENT")
            })
        {
            return Err(Exception::security(
                "You need INSTALL_GRANT_RUNTIME_PERMISSIONS permission to grant runtime permissions",
            ));
        }
        if !matches!(params.mode, 1 | 2) {
            return Err(Exception::illegal_argument(format!(
                "Invalid install mode: {}",
                params.mode
            )));
        }
        params.install_flags |= 0x10;
    }
    if params.force_queryable_override && !matches!(uid, 0 | 2000) {
        params.force_queryable_override = false;
    }
    if user_policy.organization_managed {
        params.install_flags |= 1 << 26;
    }
    if apex || !permission(q, "android.permission.ENFORCE_UPDATE_OWNERSHIP")? {
        params.install_flags &= !(1 << 25);
    }
    let mut requested_uid = -1;
    if let Some(name) = &requested {
        if let Some(state) = q.state.packages.get(name)
            && crate::package::info::user_state(state, user).installed
            && !q
                .filtered(Some(state), q.calling_uid, user)
                .map_err(unknown)?
        {
            requested_uid = user * 100000 + state.app_id;
        }
    }
    if requested_uid == -1 {
        requested = None;
    }
    let originating = if params.originating_uid >= 0 && params.originating_uid != q.calling_uid {
        q.packages_for_uid(params.originating_uid)
            .map_err(unknown)?
            .and_then(|names| names.into_iter().next().flatten())
    } else {
        None
    };
    if let Some(loader) = &params.data_loader_params {
        let loader = super::codec::DataLoader::from_object(loader)
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "Invalid data loader parcel"))?;
        if apex {
            return Err(Exception::illegal_argument(
                "DataLoader installation of APEX modules is not allowed.",
            ));
        }
        if loader.package.as_deref() == Some("android")
            && !permission(q, "android.permission.USE_SYSTEM_DATA_LOADERS")?
        {
            return Err(Exception::security(
                "You need com.android.permission.USE_SYSTEM_DATA_LOADERS permission to use system data loaders",
            ));
        }
        if loader.kind == 2 {
            return Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "IncrementalManager eligibility and filesystem owner unavailable",
            ));
        }
    }
    Ok((
        Record {
            params,
            installer_uid: uid,
            user: user as u32,
            installer_package: requested,
            installer_attribution_tag: tag,
            created_millis: now,
            initiating_package: installer,
            originating_package: originating,
            installer_package_uid: requested_uid,
        },
        install_permission,
    ))
}
fn rollback(q: &Query<'_>, params: &SessionParams, field: &str) -> Result<(), Exception> {
    if params.install_flags & 0x40000 == 0 {
        return Err(Exception::illegal_argument(format!(
            "Can't set {field} when rollback is not enabled"
        )));
    }
    if !permission(q, "android.permission.MANAGE_ROLLBACKS")? {
        return Err(Exception::security(
            "Setting rollback policy requires MANAGE_ROLLBACKS",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        apps_filter::{AppsFilter, Config},
        model::{PackageState, SharedUser, State, User},
        pkg::AndroidPackage,
    };
    use std::sync::Arc;
    #[test]
    fn callback_visibility_uses_shared_uid_declarations_and_denies_unknown_uid() {
        let code = |name: &str, queries: Vec<String>| {
            Arc::new(AndroidPackage {
                package_name: name.into(),
                uid: 10101,
                target_sdk_version: 35,
                queries_packages: queries,
                ..Default::default()
            })
        };
        let state = State {
            shared_users: [(
                "shared".into(),
                SharedUser {
                    name: "shared".into(),
                    app_id: 10101,
                    packages: vec!["hidden".into(), "visible".into()],
                    ..Default::default()
                },
            )]
            .into(),
            packages: [
                (
                    "hidden".into(),
                    PackageState {
                        name: "hidden".into(),
                        app_id: 10101,
                        shared_user: Some("shared".into()),
                        pkg: Some(code("hidden", Vec::new())),
                        ..Default::default()
                    },
                ),
                (
                    "visible".into(),
                    PackageState {
                        name: "visible".into(),
                        app_id: 10101,
                        shared_user: Some("shared".into()),
                        pkg: Some(code("visible", vec!["future.target".into()])),
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        let query = |uid| Query {
            state: &state,
            filter: &filter,
            calling_uid: uid,
        };
        assert!(can_query(&query(10101), Some("future.target")).unwrap());
        assert!(!can_query(&query(10101), Some("private.target")).unwrap());
        assert!(!can_query(&query(10102), Some("future.target")).unwrap());
        assert!(can_query(&query(0), Some("private.target")).unwrap());
    }
}
