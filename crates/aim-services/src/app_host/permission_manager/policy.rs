//! PermissionPolicyService's PackageList and permission-change callbacks,
//! android-16.0.0_r1. Adapted from PermissionToOpSynchroniser (Apache-2.0).

use super::*;
use crate::app_host::app_op_infos::{APP_OP_INFOS, AppOpInfo};
use crate::app_host::appops_service::{self, MODE_ALLOWED, MODE_FOREGROUND, MODE_IGNORED};
use crate::app_host::package_observers::PackageListObserver;
use crate::package::pkg::booleans;

const PROTECTION_FLAG_APPOP: i32 = 0x40;
const READ_EXTERNAL_STORAGE: &str = "android.permission.READ_EXTERNAL_STORAGE";
const WRITE_EXTERNAL_STORAGE: &str = "android.permission.WRITE_EXTERNAL_STORAGE";

pub(in crate::app_host) struct PermissionPolicy(pub Weak<AppHost>);

impl PermissionPolicy {
    fn update(
        &self,
        name: Option<&str>,
        uid: i32,
        sync: bool,
        reset: bool,
    ) -> Result<(), Exception> {
        let name = name.ok_or_else(|| Exception::illegal_argument("package name absent"))?;
        if uid < 0 {
            return Err(Exception::illegal_argument("package UID absent"));
        }
        let host = self
            .0
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "permission policy: app host gone"))?;
        let owner = host.permission_manager.get().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "permission policy: access owner unavailable",
            )
        })?;
        // UserController starts only user 0 in the app host. PackageList's
        // app ID is synchronized for every started user, not only the user
        // carried by the publication's UID.
        owner.synchronize_permission_app_ops(uid % PER_USER_RANGE, reset.then_some(name), sync)
    }
}

impl PackageListObserver for PermissionPolicy {
    fn package_added(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.update(name, uid, true, false)
    }
    fn package_changed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.update(name, uid, true, true)
    }
    fn package_removed(&self, name: Option<&str>, uid: i32) -> Result<(), Exception> {
        self.update(name, uid, false, true)
    }
}

impl PermissionManager {
    pub(super) fn synchronize_permission_app_ops(
        &self,
        app_id: i32,
        reset: Option<&str>,
        sync: bool,
    ) -> Result<(), Exception> {
        if app_id >= PER_USER_RANGE || app_id < 0 {
            return Err(Exception::illegal_argument("permission policy app ID"));
        }
        self.retained_access()?;
        let mut held = self.owner.lock().unwrap();
        let before = held.user0.clone().expect("loaded");
        let system = held.system.clone().expect("loaded");
        let mut after = (*before).clone();
        let (codes, packages) = self.access_with(&system, &before, |access| {
            let op = |name: &str| {
                APP_OP_INFOS.iter().find(|op| {
                    op.permission.is_some_and(|(permission, flag)| {
                        permission == name
                            && flag.is_none_or(|flag| {
                                access
                                    .state
                                    .system
                                    .flags
                                    .iter()
                                    .any(|(n, on)| n == flag && *on)
                            })
                    })
                })
            };
            let forced = self
                .host
                .platform()?
                .settings
                .device_config("storage_native_boot", "forced_scoped_storage_whitelist");
            synchronize(
                access,
                &mut after,
                app_id,
                reset,
                sync,
                &op,
                &|op| appops_service::policy_default_mode(&self.host, op.code),
                forced.as_deref().unwrap_or(""),
            )
        })?;
        if after == *before {
            return Ok(());
        }
        self.write_user0(&after)?;
        held.user0 = Some(Arc::new(after));
        drop(held);
        appops_service::permission_modes_changed(&self.host, app_id, &codes, &packages)
    }
}

fn set_mode<K: PartialEq>(
    entries: &mut Vec<(K, permissions::Named)>,
    key: K,
    op: &str,
    mode: i32,
    default: i32,
) {
    let at = entries.iter().position(|(id, _)| *id == key);
    if let Some(at) = at {
        let ops = &mut entries[at].1;
        ops.retain(|(name, _)| name != op);
        if mode != default {
            ops.push((op.into(), mode));
        }
        if ops.is_empty() {
            entries.remove(at);
        }
    } else if mode != default {
        entries.push((key, vec![(op.into(), mode)]));
    }
}

fn raw(user: &AccessUser, id: i32, package: &str, op: &AppOpInfo, default: i32) -> i32 {
    user.app_id_app_ops
        .iter()
        .find(|(uid, _)| *uid == id)
        .and_then(|(_, ops)| ops.iter().find(|(name, _)| name == op.name))
        .map(|(_, mode)| *mode)
        .filter(|mode| *mode != default)
        .or_else(|| {
            user.package_app_ops
                .iter()
                .find(|(name, _)| name == package)
                .and_then(|(_, ops)| ops.iter().find(|(name, _)| name == op.name))
                .map(|(_, mode)| *mode)
        })
        .unwrap_or(default)
}

fn should_grant(
    access: &Access<'_>,
    ps: &PackageState,
    permission: &Permission,
    flags: i32,
    minimum_sdk: i32,
) -> Result<bool, Exception> {
    if !access.granted_for_app_id(
        ps.shared_user_app_id.unwrap_or(ps.app_id),
        info::user_state(ps, 0).instant_app,
        &permission.name,
    ) || flags & APP_OP_REVOKED != 0
    {
        return Ok(false);
    }
    let info = access.permission_info(permission).ok_or_else(|| {
        Exception::new(
            EX_ILLEGAL_STATE,
            format!(
                "permission policy: definition absent for {}",
                permission.name
            ),
        )
    })?;
    if info.flags & FLAG_HARD_RESTRICTED != 0 {
        return Ok(to_api_flags(flags) & FLAG_PERMISSION_APPLY_RESTRICTION == 0);
    }
    if info.flags & FLAG_SOFT_RESTRICTED != 0
        && matches!(
            permission.name.as_str(),
            READ_EXTERNAL_STORAGE | WRITE_EXTERNAL_STORAGE
        )
    {
        return Ok(
            flags & (INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT) != 0 || minimum_sdk >= Q,
        );
    }
    Ok(true)
}

/// Collect every package of the UID before applying modes. Allowed wins over
/// foreground, then ignored, then ignored-if-not-allowed, as syncPackages does.
#[allow(clippy::too_many_arguments)]
fn synchronize(
    access: &Access<'_>,
    after: &mut AccessUser,
    app_id: i32,
    reset: Option<&str>,
    sync: bool,
    op_for: &impl Fn(&str) -> Option<&'static AppOpInfo>,
    default_for: &impl Fn(&AppOpInfo) -> Result<i32, Exception>,
    forced_scoped: &str,
) -> Result<(Vec<i32>, Vec<String>), Exception> {
    let packages: Vec<_> = access
        .state
        .packages
        .values()
        .filter(|ps| {
            ps.shared_user_app_id.unwrap_or(ps.app_id) == app_id
                && info::user_state(ps, 0).installed
                && ps.pkg.is_some()
        })
        .collect();
    let mut names: Vec<_> = packages.iter().map(|p| p.name.clone()).collect();
    if let Some(name) = reset
        && !names.iter().any(|n| n == name)
    {
        names.push(name.into());
    }
    let requested: BTreeSet<_> = packages
        .iter()
        .flat_map(|p| &p.pkg.as_ref().unwrap().requested_permissions)
        .map(String::as_str)
        .collect();
    let minimum_sdk = packages
        .iter()
        .map(|p| p.pkg.as_ref().unwrap().target_sdk_version)
        .min()
        .unwrap_or(CUR_DEVELOPMENT);
    // (priority, package, switch op, mode); priority 3 is ignore-if-not-allowed.
    let mut changes = Vec::new();
    if sync && !matches!(app_id, ROOT_UID | SYSTEM_UID) {
        for ps in &packages {
            let pkg = ps.pkg.as_ref().unwrap();
            for name in &pkg.requested_permissions {
                let Some(permission) = access.definition(name).filter(|p| is_runtime(p)) else {
                    continue;
                };
                let flags = access.flags(app_id, name);
                let info = access.permission_info(permission);
                if to_api_flags(flags) & FLAG_PERMISSION_REVIEW_REQUIRED == 0
                    && let Some(op) = op_for(name)
                {
                    let mode = if !should_grant(access, ps, permission, flags, minimum_sdk)? {
                        MODE_IGNORED
                    } else if let Some(background) = info
                        .as_ref()
                        .and_then(|i| i.background_permission.as_deref())
                    {
                        match access.definition(background) {
                            Some(p)
                                if should_grant(
                                    access,
                                    ps,
                                    p,
                                    access.flags(app_id, background),
                                    minimum_sdk,
                                )? =>
                            {
                                MODE_ALLOWED
                            }
                            _ => MODE_FOREGROUND,
                        }
                    } else {
                        MODE_ALLOWED
                    };
                    let priority = match mode {
                        MODE_ALLOWED => 0,
                        MODE_FOREGROUND => 1,
                        _ => 2,
                    };
                    changes.push((
                        priority,
                        ps.name.as_str(),
                        &APP_OP_INFOS[op.switch_code as usize],
                        mode,
                    ));
                }
                if name == READ_EXTERNAL_STORAGE
                    && info
                        .as_ref()
                        .is_some_and(|i| i.flags & FLAG_SOFT_RESTRICTED != 0)
                {
                    let exempt = flags & (INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT) != 0;
                    let forced = forced_scoped.split(',').any(|name| name == ps.name);
                    let preserve = pkg.is(booleans::PRESERVE_LEGACY_EXTERNAL_STORAGE);
                    let legacy = packages.iter().any(|p| {
                        p.pkg
                            .as_ref()
                            .unwrap()
                            .is(booleans::REQUEST_LEGACY_EXTERNAL_STORAGE)
                            || access.granted_for_app_id(
                                app_id,
                                info::user_state(p, 0).instant_app,
                                "android.permission.WRITE_MEDIA_STORAGE",
                            )
                    });
                    let (priority, mode) = if !exempt || forced || (minimum_sdk >= 30 && !preserve)
                    {
                        (2, MODE_IGNORED)
                    } else if minimum_sdk >= 30 {
                        (3, MODE_IGNORED)
                    } else if legacy {
                        (0, MODE_ALLOWED)
                    } else {
                        return Err(Exception::new(
                            EX_UNSUPPORTED_OPERATION,
                            "permission policy: legacy external storage UID state (#1455)",
                        ));
                    };
                    changes.push((priority, ps.name.as_str(), &APP_OP_INFOS[87], mode));
                }
            }
        }
    }
    changes.sort_by_key(|(priority, ..)| *priority);
    let mut set = BTreeSet::new();
    for (priority, package, op, mode) in changes {
        // The original visits every allow entry to clear each shared-UID
        // package's incorrect package override, even for an already-set op.
        if priority != 0 && set.contains(&op.code) {
            continue;
        }
        let default = default_for(op)?;
        let old = raw(after, app_id, package, op, default);
        if priority == 3 && old == MODE_ALLOWED {
            continue;
        }
        if old != mode {
            set_mode(&mut after.app_id_app_ops, app_id, op.name, mode, default);
            if priority != 3 && raw(after, app_id, package, op, default) != mode {
                set_mode(
                    &mut after.package_app_ops,
                    package.to_owned(),
                    op.name,
                    default,
                    default,
                );
            }
        }
        set.insert(op.code);
    }
    if let Some(removed) = reset {
        if packages.is_empty() {
            // AppOpsCheckingService removes the last UID and package's modes.
            after.app_id_app_ops.retain(|(id, _)| *id != app_id);
            after.package_app_ops.retain(|(name, _)| name != removed);
        } else {
            // Runtime switches need cleanup here too: this host owns their
            // modes, without the original checking-service listener.
            let requested_ops: BTreeSet<_> = requested
                .iter()
                .filter_map(|name| op_for(name))
                .map(|op| op.switch_code)
                .collect();
            for permission in access.defined {
                let runtime = is_runtime(permission);
                if !runtime && permission.protection_level & PROTECTION_FLAG_APPOP == 0 {
                    continue;
                }
                if matches!(
                    permission.name.as_str(),
                    "android.permission.ACCESS_NOTIFICATIONS"
                        | "android.permission.MANAGE_IPSEC_TUNNELS"
                        | "android.permission.REQUEST_INSTALL_PACKAGES"
                ) {
                    continue;
                }
                let Some(op) = op_for(&permission.name) else {
                    continue;
                };
                let op = if runtime {
                    &APP_OP_INFOS[op.switch_code as usize]
                } else {
                    op
                };
                if requested.contains(permission.name.as_str())
                    || (runtime && requested_ops.contains(&op.code))
                {
                    continue;
                }
                let default = default_for(op)?;
                set_mode(&mut after.app_id_app_ops, app_id, op.name, default, default);
                for package in &names {
                    set_mode(
                        &mut after.package_app_ops,
                        package.clone(),
                        op.name,
                        default,
                        default,
                    );
                }
            }
            if !packages.iter().any(|p| p.name == removed) {
                after.package_app_ops.retain(|(name, _)| name != removed);
            }
        }
    }
    let mut codes = Vec::new();
    for op in APP_OP_INFOS.iter().filter(|op| op.code >= 0) {
        let uid_mode = |user: &AccessUser| {
            user.app_id_app_ops
                .iter()
                .find(|(id, _)| *id == app_id)
                .and_then(|(_, ops)| ops.iter().find(|(name, _)| name == op.name))
                .map(|(_, mode)| *mode)
        };
        let package_mode = |user: &AccessUser, package: &str| {
            user.package_app_ops
                .iter()
                .find(|(name, _)| name == package)
                .and_then(|(_, ops)| ops.iter().find(|(name, _)| name == op.name))
                .map(|(_, mode)| *mode)
        };
        if uid_mode(access.user0) != uid_mode(after)
            || names
                .iter()
                .any(|p| package_mode(access.user0, p) != package_mode(after, p))
        {
            codes.push(op.code);
        }
    }
    Ok((codes, names))
}

#[cfg(test)]
mod tests;
