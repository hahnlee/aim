//! `permissionmgr`: PermissionManagerService's `IPermissionManager` for
//! the app host (`PermissionManagerService` and the permission module's
//! `PermissionService`, which Android 16 runs behind it, at
//! `android-16.0.0_r1`, ADR 0013): the checks by package and by uid from
//! the access state the permission service last persisted (`access.abx`:
//! each app id's permission flags, granted among them), through shared
//! uids, with the fuller permissions that imply others and only `instant`
//! permissions for instant apps; the definitions, as the caller may see
//! the packages that declare them (`PermissionInfo` from the declaring
//! package's parse, the permission groups as `AppIdPermissionPolicy` adds
//! them); a package's permission flags, whether policy revoked one,
//! whether to show a rationale, the restricted permissions' allowlists and
//! SystemConfig's split permissions. The app host has no
//! VirtualDeviceManager, so every device id is the default device's, as
//! the original decides without one. The state does not change while the
//! app host runs, so listeners are kept and never called. Grants,
//! revocations, flag and allowlist changes, a virtual device's
//! permissions, one-time sessions, auto-revoke, attribution sources and
//! the permission states are not implemented (#1508).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_permission_ipermissionmanager as pm;

use super::compat::CompatConfig;
use super::packages::permission_granted;
use super::{AppHost, reply};
use crate::package::apps_filter::{self, AppsFilter, NotModelled, Setting};
use crate::package::info::{self, PackageItemInfo, PermissionGroupInfo, PermissionInfo, flags::GET_META_DATA};
use crate::package::list_slice::Slice;
use crate::package::model::{PackageState, State};
use crate::package::parse::SplitPermission;
use crate::package::permissions::{AccessUser, Permission};
use crate::package::query::Query;

/// `PackageManager.PERMISSION_GRANTED`, `PERMISSION_DENIED`.
const GRANTED: i32 = 0;
const DENIED: i32 = -1;
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
/// `VirtualDeviceManager.PERSISTENT_DEVICE_ID_DEFAULT`.
const DEFAULT_DEVICE: &str = "default:0";
/// `PackageManager.VERSION_CODE_HIGHEST`.
const VERSION_CODE_HIGHEST: i64 = -1;
/// `PermissionInfo.PROTECTION_MASK_BASE`, `PROTECTION_SIGNATURE`,
/// `PROTECTION_FLAG_INSTANT`, `FLAG_INSTALLED`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_SIGNATURE: i32 = 2;
const PROTECTION_FLAG_INSTANT: i32 = 0x1000;
const FLAG_INSTALLED: i32 = 1 << 30;
/// `Build.VERSION_CODES.O`, `CUR_DEVELOPMENT`.
const O: i32 = 26;
const CUR_DEVELOPMENT: i32 = 10000;
/// `PermissionFlags` (services/permission) that `toApiFlags` reads.
const ROLE: i32 = 1 << 3;
const USER_SET: i32 = 1 << 5;
const USER_FIXED: i32 = 1 << 6;
const POLICY_FIXED: i32 = 1 << 7;
const SYSTEM_FIXED: i32 = 1 << 8;
const PREGRANT: i32 = 1 << 9;
const LEGACY_GRANTED: i32 = 1 << 10;
const IMPLICIT: i32 = 1 << 12;
const USER_SENSITIVE_WHEN_GRANTED: i32 = 1 << 13;
const USER_SENSITIVE_WHEN_REVOKED: i32 = 1 << 14;
const INSTALLER_EXEMPT: i32 = 1 << 15;
const SYSTEM_EXEMPT: i32 = 1 << 16;
const UPGRADE_EXEMPT: i32 = 1 << 17;
const RESTRICTION_REVOKED: i32 = 1 << 18;
const SOFT_RESTRICTED: i32 = 1 << 19;
const APP_OP_REVOKED: i32 = 1 << 20;
const ONE_TIME: i32 = 1 << 21;
const HIBERNATION: i32 = 1 << 22;
const USER_SELECTED: i32 = 1 << 23;
/// `PermissionFlags.toApiFlags`: the flags that are one
/// `PackageManager.FLAG_PERMISSION_*` each.
const API_FLAGS: &[(i32, i32)] = &[
    (USER_SET, 1 << 0),
    (USER_FIXED, 1 << 1),
    (POLICY_FIXED, 1 << 2),
    (SYSTEM_FIXED, 1 << 4),
    (PREGRANT, 1 << 5),
    (USER_SENSITIVE_WHEN_GRANTED, 1 << 8),
    (USER_SENSITIVE_WHEN_REVOKED, 1 << 9),
    (INSTALLER_EXEMPT, 1 << 11),
    (SYSTEM_EXEMPT, 1 << 12),
    (UPGRADE_EXEMPT, 1 << 13),
    (ROLE, 1 << 15),
    (APP_OP_REVOKED, 1 << 3),
    (ONE_TIME, 1 << 16),
    (HIBERNATION, 1 << 17),
    (USER_SELECTED, 1 << 19),
];
/// `FLAG_PERMISSION_REVIEW_REQUIRED`, `REVOKE_WHEN_REQUESTED`,
/// `APPLY_RESTRICTION`.
const FLAG_PERMISSION_REVIEW_REQUIRED: i32 = 1 << 6;
const FLAG_PERMISSION_REVOKE_WHEN_REQUESTED: i32 = 1 << 7;
const FLAG_PERMISSION_APPLY_RESTRICTION: i32 = 1 << 14;
/// `PackageManager.FLAG_PERMISSION_WHITELIST_*`.
const WHITELIST_SYSTEM: i32 = 1 << 0;
const WHITELIST_INSTALLER: i32 = 1 << 1;
const WHITELIST_UPGRADE: i32 = 1 << 2;
/// `PermissionService.UNREQUESTABLE_MASK`, `BACKGROUND_RATIONALE_CHANGE_ID`,
/// `FULLER_PERMISSIONS`.
const UNREQUESTABLE_MASK: i32 = RESTRICTION_REVOKED | SYSTEM_FIXED | POLICY_FIXED | USER_FIXED;
const BACKGROUND_RATIONALE_CHANGE_ID: i64 = 147_316_723;
const FULLER_PERMISSIONS: &[(&str, &str)] = &[
    ("android.permission.ACCESS_COARSE_LOCATION", "android.permission.ACCESS_FINE_LOCATION"),
    (INTERACT_ACROSS_USERS, INTERACT_ACROSS_USERS_FULL),
];
const ACCESS_BACKGROUND_LOCATION: &str = "android.permission.ACCESS_BACKGROUND_LOCATION";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const RUNTIME_PERMISSION_READERS: &[&str] = &[
    "android.permission.GRANT_RUNTIME_PERMISSIONS",
    "android.permission.REVOKE_RUNTIME_PERMISSIONS",
    "android.permission.GET_RUNTIME_PERMISSIONS",
];
const WHITELIST_RESTRICTED_PERMISSIONS: &str = "android.permission.WHITELIST_RESTRICTED_PERMISSIONS";
const OBSERVE_GRANT_REVOKE_PERMISSIONS: &str = "android.permission.OBSERVE_GRANT_REVOKE_PERMISSIONS";
const PERMISSION_GROUP_INFO: &str = "android.content.pm.PermissionGroupInfo";
const PERMISSION_INFO: &str = "android.content.pm.PermissionInfo";

fn not_modelled(e: NotModelled) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, format!("permissionmgr: {}", e.0))
}

/// Kotlin's check of a non-null parameter of `PermissionService.method`.
fn non_null<'s>(value: Option<&'s str>, method: &str, parameter: &str) -> Result<&'s str, Exception> {
    value.ok_or_else(|| {
        Exception::new(
            EX_NULL_POINTER,
            format!(
                "Parameter specified as non-null is null: method \
                 com.android.server.permission.access.permission.PermissionService.{method}, parameter {parameter}"
            ),
        )
    })
}

/// `PermissionFlags.toApiFlags`.
fn to_api_flags(flags: i32) -> i32 {
    let has = |bits: i32| flags & bits == bits;
    let mut api = API_FLAGS.iter().filter(|(f, _)| has(*f)).fold(0, |api, (_, a)| api | a);
    if has(IMPLICIT) {
        api |= if has(LEGACY_GRANTED) { FLAG_PERMISSION_REVIEW_REQUIRED } else { FLAG_PERMISSION_REVOKE_WHEN_REQUESTED };
    }
    if has(RESTRICTION_REVOKED) || has(SOFT_RESTRICTED) {
        api |= FLAG_PERMISSION_APPLY_RESTRICTION;
    }
    api
}

fn fuller(permission: &str) -> Option<&'static str> {
    FULLER_PERMISSIONS.iter().find(|(p, _)| *p == permission).map(|(_, f)| *f)
}

/// `generatePermissionInfo`: installed, its meta-data only if asked,
/// and for a caller targeting an SDK before O only the base protection
/// of a permission that is not a signature one.
fn generate_permission_info(mut info: PermissionInfo, flags: i32, target_sdk: i32) -> PermissionInfo {
    info.flags |= FLAG_INSTALLED;
    if i64::from(flags) & GET_META_DATA == 0 {
        info.item.meta_data = None;
    }
    let protection = info.protection_level & PROTECTION_MASK_BASE;
    if target_sdk < O && protection != PROTECTION_SIGNATURE {
        info.protection_level = protection;
    }
    info
}

/// `generatePermissionGroupInfo`: its meta-data only if asked.
fn generate_permission_group_info(group: &PermissionGroupInfo, flags: i32) -> PermissionGroupInfo {
    let mut group = group.clone();
    if i64::from(flags) & GET_META_DATA == 0 {
        group.item.meta_data = None;
    }
    group
}

/// `AppIdPermissionPolicy.addPermissionGroups` of every package, in
/// PackageManager's order (its `ArrayMap`'s): a group is its first
/// declarer's unless a system package declares it over a package that
/// is not one; a package that is instant in every user declares none.
/// In the groups' own map order.
fn permission_groups(state: &State) -> Vec<PermissionGroupInfo> {
    // Each group with whether its package is a system one.
    let mut groups: Vec<(PermissionGroupInfo, bool)> = Vec::new();
    for ps in info::array_order(state.packages.values().collect(), |ps| ps.name.as_str()) {
        let Some(pkg) = ps.pkg.as_deref() else { continue };
        if ps.users.values().all(|u| u.instant_app) {
            continue;
        }
        for g in &pkg.permission_groups {
            let group = info::generate_permission_group_info(g, GET_META_DATA);
            match groups.iter().position(|(old, _)| old.item.name == group.item.name) {
                Some(i) => {
                    let (old, old_system) = &groups[i];
                    if old.item.package_name != group.item.package_name && (!ps.is.system || *old_system) {
                        continue;
                    }
                    groups[i] = (group, ps.is.system);
                }
                None => groups.push((group, ps.is.system)),
            }
        }
    }
    info::array_order(groups.into_iter().map(|(g, _)| g).collect(), |g| g.item.name.as_deref().unwrap_or_default())
}

/// `SplitPermissionInfoParcelable`.
struct SplitPermissionInfo<'a>(&'a SplitPermission);

impl WriteParcelable for SplitPermissionInfo<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.0.name));
        p.write_i32(self.0.new_permissions.len() as i32);
        for name in &self.0.new_permissions {
            p.write_string16(Some(name));
        }
        p.write_i32(self.0.target_sdk);
    }
}

/// What the permission service reads: the package model with its apps
/// filter, the permissions defined, the permission groups and the access
/// state of user 0, the model's one user.
struct Access<'a> {
    state: &'a State,
    filter: &'a AppsFilter,
    defined: &'a [Permission],
    groups: &'a [PermissionGroupInfo],
    user0: &'a AccessUser,
    compat: &'a CompatConfig,
}

impl Access<'_> {
    /// `UserManagerInternal.exists`.
    fn user_exists(&self, user: i32) -> bool {
        self.state.users.contains_key(&user)
    }

    /// `AppIdPermissionPolicy.getPermissionFlags` of an existing user's
    /// app id.
    fn flags(&self, app_id: i32, permission: &str) -> i32 {
        self.user0
            .app_id_permissions
            .iter()
            .find(|(id, _)| *id == app_id)
            .and_then(|(_, permissions)| permissions.iter().find(|(name, _)| name == permission))
            .map_or(0, |(_, flags)| *flags)
    }

    fn definition(&self, permission: &str) -> Option<&Permission> {
        self.defined.iter().find(|p| p.name == permission)
    }

    /// `isPermissionGranted`: the permission or the fuller one that
    /// implies it. Instant apps have no shared uid, so the package's own
    /// state says whether it is one.
    fn granted(&self, ps: &PackageState, user: i32, permission: &str) -> bool {
        let instant = info::user_state(ps, user).instant_app;
        self.single_granted(ps.app_id, instant, permission)
            || fuller(permission).is_some_and(|f| self.single_granted(ps.app_id, instant, f))
    }

    /// `isSinglePermissionGranted`: an instant app holds only `instant`
    /// permissions.
    fn single_granted(&self, app_id: i32, instant: bool, permission: &str) -> bool {
        permission_granted(self.flags(app_id, permission))
            && (!instant
                || self.definition(permission).is_some_and(|p| p.protection_level & PROTECTION_FLAG_INSTANT != 0))
    }

    /// `PackageManagerLocal.FilteredSnapshot.getPackageState`: the
    /// package, unless the caller may not see it in `user`.
    fn package(&self, name: &str, caller: i32, user: i32) -> Result<Option<&PackageState>, Exception> {
        let query = Query { state: self.state, filter: self.filter, calling_uid: caller };
        let name = query.resolve_internal_package_name(name, VERSION_CODE_HIGHEST);
        let Some(ps) = self.state.packages.get(&name) else { return Ok(None) };
        Ok((!query.filtered(Some(ps), caller, user).map_err(not_modelled)?).then_some(ps))
    }

    /// `isPackageVisibleToUid`, in the uid's user.
    fn visible(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.package(package, uid, apps_filter::user_id(uid))?.is_some())
    }

    /// `isUidInstantApp`.
    fn instant_caller(&self, uid: i32) -> Result<bool, Exception> {
        Ok(apps_filter::instant_app_package_name(self.state, uid).map_err(not_modelled)?.is_some())
    }

    /// `checkPermission`.
    fn check_permission(&self, caller: i32, package: &str, permission: &str, user: i32) -> Result<i32, Exception> {
        if !self.user_exists(user) {
            return Ok(DENIED);
        }
        let Some(ps) = self.package(package, caller, user)? else { return Ok(DENIED) };
        Ok(if self.granted(ps, user, permission) { GRANTED } else { DENIED })
    }

    /// `checkUidPermission`: by the uid's package (`PackageManagerInternal
    /// .getPackage(int)`: an SDK sandbox's is its sandbox package's, a
    /// shared uid's the first of its packages installed in the uid's
    /// user), else by SystemConfig's grants to the uid.
    fn check_uid_permission(&self, uid: i32, permission: &str) -> Result<i32, Exception> {
        let user = apps_filter::user_id(uid);
        if !self.user_exists(user) {
            return Ok(DENIED);
        }
        let app_id = if apps_filter::is_sdk_sandbox(uid) {
            self.state
                .system
                .sdk_sandbox_package
                .as_ref()
                .and_then(|name| self.state.packages.get(name.as_deref()?))
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no SDK sandbox package"))?
                .app_id
        } else {
            apps_filter::app_id(uid)
        };
        let packages: Vec<&PackageState> = match apps_filter::setting(self.state, app_id) {
            Some(Setting::Package(ps)) => vec![ps],
            Some(Setting::Shared(su)) => apps_filter::shared_packages(self.state, su).collect(),
            None => Vec::new(),
        };
        if let Some(ps) = packages.into_iter().find(|ps| info::user_state(ps, user).installed && ps.pkg.is_some()) {
            return Ok(if self.granted(ps, user, permission) { GRANTED } else { DENIED });
        }
        let grants = self.state.system.system_permissions.as_ref().ok_or_else(|| {
            not_modelled(NotModelled("SystemConfig system-UID permission assignments"))
        })?;
        let held = grants
            .get(&uid)
            .is_some_and(|g| g.contains(permission) || fuller(permission).is_some_and(|f| g.contains(f)));
        Ok(if held { GRANTED } else { DENIED })
    }

    /// `Context.checkCallingOrSelfPermission` in a call from `uid`
    /// (`ActivityManager.checkComponentPermission`): root and system hold
    /// every permission, isolated processes none.
    fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        if matches!(apps_filter::app_id(uid), ROOT_UID | SYSTEM_UID) {
            return Ok(true);
        }
        if apps_filter::is_isolated(uid) {
            return Ok(false);
        }
        Ok(self.check_uid_permission(uid, permission)? == GRANTED)
    }

    /// `enforceCallingOrSelfCrossUserPermission`, without the shell's
    /// restriction.
    fn enforce_cross_user(&self, caller: i32, user: i32, full: bool, message: &str) -> Result<(), Exception> {
        if user < 0 {
            return Err(Exception::illegal_argument(format!("userId {user} is invalid")));
        }
        let permission = if full { INTERACT_ACROSS_USERS_FULL } else { INTERACT_ACROSS_USERS };
        if user != apps_filter::user_id(caller) && !self.holds(caller, permission)? {
            return Err(Exception::security(format!(
                "{message}: Neither user {caller} nor current process has {permission} to access user {user}"
            )));
        }
        Ok(())
    }

    /// `enforceCallingOrSelfAnyPermission`.
    fn enforce_any(&self, caller: i32, message: &str, permissions: &[&str]) -> Result<(), Exception> {
        for permission in permissions {
            if self.holds(caller, permission)? {
                return Ok(());
            }
        }
        Err(Exception::security(format!(
            "{message}: Neither user {caller} nor current process has any of {}",
            permissions.join(", ")
        )))
    }

    /// `Context.enforceCallingOrSelfPermission`.
    fn enforce(&self, caller: i32, permission: &str, message: &str) -> Result<(), Exception> {
        if self.holds(caller, permission)? {
            return Ok(());
        }
        Err(Exception::security(format!("{message}: Neither user {caller} nor current process has {permission}.")))
    }

    /// The `PermissionInfo` the permission service keeps for `p`: a
    /// manifest permission's from its package's parse, with its meta-data
    /// (none once the package no longer declares it: the service trims
    /// it); a dynamic one's as it was persisted.
    fn permission_info(&self, p: &Permission) -> Option<PermissionInfo> {
        let Some((icon, label)) = &p.dynamic else {
            let pkg = self.state.packages.get(&p.package)?.pkg.as_deref()?;
            // A later declaration replaces an earlier one.
            let parsed = pkg.permissions.iter().rev().find(|d| !d.tree && d.component.name == p.name)?;
            return Some(info::generate_permission_info(parsed, GET_META_DATA));
        };
        Some(PermissionInfo {
            item: PackageItemInfo {
                name: Some(p.name.clone()),
                package_name: Some(p.package.clone()),
                non_localized_label: label.clone(),
                icon: *icon,
                ..PackageItemInfo::default()
            },
            protection_level: p.protection_level,
            flags: 0,
            group: None,
            background_permission: None,
            description_res: 0,
            request_res: 0,
            known_certs: Some(Vec::new()),
        })
    }

    /// `getPermissionInfo`: the caller's target SDK is its op package's;
    /// root, system and the shell see every protection flag.
    fn get_permission_info(
        &self,
        caller: i32,
        permission: &str,
        op_package: &str,
        flags: i32,
    ) -> Result<Option<PermissionInfo>, Exception> {
        if self.instant_caller(caller)? {
            return Ok(None);
        }
        let Some(p) = self.definition(permission) else { return Ok(None) };
        if !self.visible(&p.package, caller)? {
            return Ok(None);
        }
        let target_sdk = if apps_filter::is_system_or_root_or_shell(caller) {
            CUR_DEVELOPMENT
        } else {
            self.state
                .packages
                .get(op_package)
                .and_then(|ps| ps.pkg.as_deref())
                .map_or(CUR_DEVELOPMENT, |pkg| pkg.target_sdk_version)
        };
        Ok(self.permission_info(p).map(|info| generate_permission_info(info, flags, target_sdk)))
    }

    /// `queryPermissionsByGroup`: the permissions of `group` (or of none),
    /// as the caller may see them.
    fn query_permissions_by_group(
        &self,
        caller: i32,
        group: Option<&str>,
        flags: i32,
    ) -> Result<Option<Vec<PermissionInfo>>, Exception> {
        if self.instant_caller(caller)? {
            return Ok(None);
        }
        if let Some(group) = group {
            let Some(g) = self.groups.iter().find(|g| g.item.name.as_deref() == Some(group)) else {
                return Ok(None);
            };
            if !self.visible(g.item.package_name.as_deref().unwrap_or_default(), caller)? {
                return Ok(None);
            }
        }
        let mut infos = Vec::new();
        for p in self.defined {
            let Some(info) = self.permission_info(p) else { continue };
            if info.group.as_deref() == group && self.visible(&p.package, caller)? {
                infos.push(generate_permission_info(info, flags, CUR_DEVELOPMENT));
            }
        }
        Ok(Some(infos))
    }

    /// `getAllPermissionGroups`.
    fn all_permission_groups(&self, caller: i32, flags: i32) -> Result<Vec<PermissionGroupInfo>, Exception> {
        if self.instant_caller(caller)? {
            return Ok(Vec::new());
        }
        let mut groups = Vec::new();
        for g in self.groups {
            if self.visible(g.item.package_name.as_deref().unwrap_or_default(), caller)? {
                groups.push(generate_permission_group_info(g, flags));
            }
        }
        Ok(groups)
    }

    /// `getPermissionGroupInfo`.
    fn permission_group_info(&self, caller: i32, group: &str, flags: i32) -> Result<Option<PermissionGroupInfo>, Exception> {
        if self.instant_caller(caller)? {
            return Ok(None);
        }
        let Some(g) = self.groups.iter().find(|g| g.item.name.as_deref() == Some(group)) else { return Ok(None) };
        if !self.visible(g.item.package_name.as_deref().unwrap_or_default(), caller)? {
            return Ok(None);
        }
        Ok(Some(generate_permission_group_info(g, flags)))
    }

    /// `getPermissionFlags`, of a package the caller sees in its own user.
    fn permission_flags(&self, caller: i32, package: &str, permission: &str, user: i32) -> Result<i32, Exception> {
        if !self.user_exists(user) {
            return Ok(0);
        }
        self.enforce_cross_user(caller, user, true, "getPermissionFlags")?;
        self.enforce_any(caller, "getPermissionFlags", RUNTIME_PERMISSION_READERS)?;
        let Some(ps) = self.package(package, caller, apps_filter::user_id(caller))? else { return Ok(0) };
        if self.definition(permission).is_none() {
            return Ok(0);
        }
        Ok(to_api_flags(self.flags(ps.app_id, permission)))
    }

    /// `isPermissionRevokedByPolicy`: not granted, and fixed by policy.
    fn revoked_by_policy(&self, caller: i32, package: &str, permission: &str, user: i32) -> Result<bool, Exception> {
        if !self.user_exists(user) {
            return Ok(false);
        }
        self.enforce_cross_user(caller, user, true, "isPermissionRevokedByPolicy")?;
        let Some(ps) = self.package(package, caller, user)? else { return Ok(false) };
        Ok(!self.granted(ps, user, permission) && self.flags(ps.app_id, permission) & POLICY_FIXED != 0)
    }

    /// `shouldShowRequestPermissionRationale`, for the caller's own
    /// package: a permission it does not hold and may request, which the
    /// user denied (background location always, once its compat change
    /// is enabled for the package).
    fn should_show_rationale(&self, caller: i32, package: &str, permission: &str, user: i32) -> Result<bool, Exception> {
        if !self.user_exists(user) {
            return Ok(false);
        }
        self.enforce_cross_user(caller, user, true, "shouldShowRequestPermissionRationale")?;
        let Some(ps) = self.package(package, caller, user)? else { return Ok(false) };
        if apps_filter::app_id(caller) != ps.app_id || self.granted(ps, user, permission) {
            return Ok(false);
        }
        let flags = self.flags(ps.app_id, permission);
        if flags & UNREQUESTABLE_MASK != 0 {
            return Ok(false);
        }
        if permission == ACCESS_BACKGROUND_LOCATION {
            // `PlatformCompat.isChangeEnabledByPackageName`.
            let enabled = match ps.pkg.as_deref() {
                Some(pkg) => self.compat.is_enabled(BACKGROUND_RATIONALE_CHANGE_ID, pkg.target_sdk_version),
                None => self.compat.default_value(BACKGROUND_RATIONALE_CHANGE_ID),
            };
            if enabled {
                return Ok(true);
            }
        }
        Ok(flags & USER_SET != 0)
    }

    /// `getAllowlistedRestrictedPermissions`: the system's allowlist for
    /// holders of WHITELIST_RESTRICTED_PERMISSIONS, the upgrade and
    /// installer ones for them and the package's installer of record.
    fn allowlisted(&self, caller: i32, package: &str, flags: i32, user: i32) -> Result<Option<Vec<String>>, Exception> {
        let mask = WHITELIST_SYSTEM | WHITELIST_INSTALLER | WHITELIST_UPGRADE;
        if flags & !mask != 0 {
            return Err(Exception::illegal_argument(format!(
                "Requested flags 0x{flags:x}, but only 0x{mask:x} are allowed"
            )));
        }
        if user < 0 {
            return Err(Exception::illegal_argument("userId cannot be null"));
        }
        if !self.user_exists(user) {
            return Ok(None);
        }
        self.enforce_cross_user(caller, user, false, "getAllowlistedRestrictedPermissions")?;
        let Some(ps) = self.package(package, caller, user)?.filter(|ps| ps.pkg.is_some()) else { return Ok(None) };
        let privileged = self.holds(caller, WHITELIST_RESTRICTED_PERMISSIONS)?;
        if flags & WHITELIST_SYSTEM != 0 && !privileged {
            return Err(Exception::security(format!("Querying system allowlist requires {WHITELIST_RESTRICTED_PERMISSIONS}")));
        }
        // `isCallerInstallerOfRecord`.
        let installer = ps
            .install_source
            .installer
            .as_deref()
            .and_then(|name| self.state.packages.get(name))
            .is_some_and(|i| i.app_id == apps_filter::app_id(caller));
        if flags & (WHITELIST_UPGRADE | WHITELIST_INSTALLER) != 0 && !privileged && !installer {
            return Err(Exception::security(format!(
                "Querying upgrade or installer allowlist requires being installer on record or {WHITELIST_RESTRICTED_PERMISSIONS}"
            )));
        }
        let mut query = 0;
        for (allowlist, exempt) in
            [(WHITELIST_SYSTEM, SYSTEM_EXEMPT), (WHITELIST_UPGRADE, UPGRADE_EXEMPT), (WHITELIST_INSTALLER, INSTALLER_EXEMPT)]
        {
            if flags & allowlist != 0 {
                query |= exempt;
            }
        }
        Ok(self.user0.app_id_permissions.iter().find(|(id, _)| *id == ps.app_id).map(|(_, permissions)| {
            permissions.iter().filter(|(_, f)| f & query != 0).map(|(name, _)| name.clone()).collect()
        }))
    }
}

pub struct PermissionManager {
    host: Arc<AppHost>,
    groups: OnceLock<Vec<PermissionGroupInfo>>,
    listeners: Mutex<Vec<Binder>>,
}

impl PermissionManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        PermissionManager { host, groups: OnceLock::new(), listeners: Mutex::new(Vec::new()) }
    }

    fn access<T>(&self, f: impl FnOnce(&Access<'_>) -> Result<T, Exception>) -> Result<T, Exception> {
        let inputs = self.host.inputs()?;
        let (state, resolution) = self.host.packages()?;
        let user0 = inputs
            .packages
            .access()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no permission state of user 0"))?;
        f(&Access {
            state: &state,
            filter: &resolution.apps_filter,
            defined: inputs.packages.permissions(),
            groups: self.groups.get_or_init(|| permission_groups(&state)),
            user0,
            compat: &inputs.compat,
        })
    }

    /// A persistent device id: the app host has no virtual devices, and
    /// another device's permissions are not implemented (#1508).
    fn default_device(&self, device: &str) -> Result<(), Exception> {
        if device == DEFAULT_DEVICE {
            return Ok(());
        }
        Err(self.host.unimplemented_named("permissionmgr", "a virtual device's permissions (#1508)"))
    }
}

fn parcel(write: impl FnOnce(&mut Parcel)) -> Parcel {
    let mut p = Parcel::new();
    write(&mut p);
    p
}

impl Service for PermissionManager {
    fn descriptor(&self) -> &str {
        pm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let caller = call.sender_euid as i32;
        let process = || self.host.process.clone();
        reply(match call.code {
            pm::GET_ALL_PERMISSION_GROUPS => {
                let a = pm::GetAllPermissionGroups::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.access(|access| access.all_permission_groups(caller, a.flags)).map(|groups| {
                    let slice = Slice::new(process(), PERMISSION_GROUP_INFO, groups);
                    parcel(|p| pm::write_get_all_permission_groups_reply(p, Some(&slice)))
                })
            }
            pm::GET_PERMISSION_GROUP_INFO => {
                let a = pm::GetPermissionGroupInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                non_null(a.group_name.as_deref(), "getPermissionGroupInfo", "permissionGroupName")
                    .and_then(|group| self.access(|access| access.permission_group_info(caller, group, a.flags)))
                    .map(|info| parcel(|p| pm::write_get_permission_group_info_reply(p, info.as_ref())))
            }
            pm::GET_PERMISSION_INFO => {
                let a = pm::GetPermissionInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let permission = non_null(a.permission_name.as_deref(), "getPermissionInfo", "permissionName")?;
                    let op_package = non_null(a.package_name.as_deref(), "getPermissionInfo", "opPackageName")?;
                    self.access(|access| access.get_permission_info(caller, permission, op_package, a.flags))
                })()
                .map(|info| parcel(|p| pm::write_get_permission_info_reply(p, info.as_ref())))
            }
            pm::QUERY_PERMISSIONS_BY_GROUP => {
                let a = pm::QueryPermissionsByGroup::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.access(|access| access.query_permissions_by_group(caller, a.group_name.as_deref(), a.flags)).map(
                    |infos| {
                        let slice = infos.map(|infos| Slice::new(process(), PERMISSION_INFO, infos));
                        parcel(|p| pm::write_query_permissions_by_group_reply(p, slice.as_ref()))
                    },
                )
            }
            pm::GET_PERMISSION_FLAGS => {
                let a = pm::GetPermissionFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = non_null(a.package_name.as_deref(), "getPermissionFlags", "packageName")?;
                    let permission = non_null(a.permission_name.as_deref(), "getPermissionFlags", "permissionName")?;
                    self.default_device(non_null(a.persistent_device_id.as_deref(), "getPermissionFlags", "deviceId")?)?;
                    self.access(|access| access.permission_flags(caller, package, permission, a.user_id))
                })()
                .map(|flags| parcel(|p| pm::write_get_permission_flags_reply(p, flags)))
            }
            pm::ADD_ON_PERMISSIONS_CHANGE_LISTENER | pm::REMOVE_ON_PERMISSIONS_CHANGE_LISTENER => {
                let add = call.code == pm::ADD_ON_PERMISSIONS_CHANGE_LISTENER;
                let (listener, method) = if add {
                    let a = pm::AddOnPermissionsChangeListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.listener, "addOnPermissionsChangeListener")
                } else {
                    let a = pm::RemoveOnPermissionsChangeListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.listener, "removeOnPermissionsChangeListener")
                };
                self.access(|access| access.enforce(caller, OBSERVE_GRANT_REVOKE_PERMISSIONS, method))
                    .and_then(|()| {
                        listener.ok_or_else(|| {
                            Exception::new(
                                EX_NULL_POINTER,
                                format!(
                                    "Parameter specified as non-null is null: method \
                                     com.android.server.permission.access.permission.PermissionService.{method}, parameter listener"
                                ),
                            )
                        })
                    })
                    .map(|listener| {
                        let mut listeners = self.listeners.lock().unwrap();
                        if add {
                            listeners.push(listener);
                            parcel(pm::write_add_on_permissions_change_listener_reply)
                        } else {
                            listeners.retain(|l| *l != listener);
                            parcel(pm::write_remove_on_permissions_change_listener_reply)
                        }
                    })
            }
            pm::GET_ALLOWLISTED_RESTRICTED_PERMISSIONS => {
                let a = pm::GetAllowlistedRestrictedPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                non_null(a.package_name.as_deref(), "getAllowlistedRestrictedPermissions", "packageName")
                    .and_then(|package| self.access(|access| access.allowlisted(caller, package, a.flags, a.user_id)))
                    .map(|names| {
                        let names: Option<Vec<Option<String>>> = names.map(|n| n.into_iter().map(Some).collect());
                        parcel(|p| pm::write_get_allowlisted_restricted_permissions_reply(p, &names))
                    })
            }
            pm::SHOULD_SHOW_REQUEST_PERMISSION_RATIONALE => {
                let a = pm::ShouldShowRequestPermissionRationale::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let method = "shouldShowRequestPermissionRationale";
                (|| {
                    let package = non_null(a.package_name.as_deref(), method, "packageName")?;
                    let permission = non_null(a.permission_name.as_deref(), method, "permissionName")?;
                    self.access(|access| access.should_show_rationale(caller, package, permission, a.user_id))
                })()
                .map(|show| parcel(|p| pm::write_should_show_request_permission_rationale_reply(p, show)))
            }
            pm::IS_PERMISSION_REVOKED_BY_POLICY => {
                let a = pm::IsPermissionRevokedByPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = non_null(a.package_name.as_deref(), "isPermissionRevokedByPolicy", "packageName")?;
                    let permission =
                        non_null(a.permission_name.as_deref(), "isPermissionRevokedByPolicy", "permissionName")?;
                    self.access(|access| access.revoked_by_policy(caller, package, permission, a.user_id))
                })()
                .map(|revoked| parcel(|p| pm::write_is_permission_revoked_by_policy_reply(p, revoked)))
            }
            pm::GET_SPLIT_PERMISSIONS => {
                pm::GetSplitPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host.inputs().map(|inputs| {
                    let splits: Vec<_> =
                        inputs.packages.split_permissions().iter().map(|s| Some(SplitPermissionInfo(s))).collect();
                    parcel(|p| pm::write_get_split_permissions_reply(p, Some(splits.as_slice())))
                })
            }
            pm::CHECK_PERMISSION => {
                let a = pm::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    // `PermissionManagerService.checkPermission`: no package or
                    // permission is denied.
                    let (Some(package), Some(permission)) = (a.package_name.as_deref(), a.permission_name.as_deref())
                    else {
                        return Ok(DENIED);
                    };
                    self.default_device(non_null(a.persistent_device_id.as_deref(), "checkPermission", "deviceId")?)?;
                    self.access(|access| access.check_permission(caller, package, permission, a.user_id))
                })()
                .map(|result| parcel(|p| pm::write_check_permission_reply(p, result)))
            }
            pm::CHECK_UID_PERMISSION => {
                let a = pm::CheckUidPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `PermissionManagerService.checkUidPermission`: no permission
                // is denied.
                let result = match a.permission_name.as_deref() {
                    None => Ok(DENIED),
                    Some(permission) => self.access(|access| access.check_uid_permission(a.uid, permission)),
                };
                result.map(|result| parcel(|p| pm::write_check_uid_permission_reply(p, result)))
            }
            code => {
                let name = pm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("permissionmgr", &format!("{name} (#1508)")))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::package::model::{PackageUserState, SharedUser, User};
    use crate::package::pkg::{self, AndroidPackage, Component};

    const CAMERA: &str = "android.permission.CAMERA";
    const INTERNET: &str = "android.permission.INTERNET";
    const RUNTIME_GRANTED: i32 = 1 << 4;
    const INSTALL_GRANTED: i32 = 1 << 0;

    fn package(name: &str, app_id: i32, edit: impl FnOnce(&mut PackageState, &mut AndroidPackage)) -> PackageState {
        let mut ps = PackageState {
            name: name.into(),
            app_id,
            users: [(0, PackageUserState { installed: true, ..PackageUserState::default() })].into(),
            ..PackageState::default()
        };
        let mut pkg = AndroidPackage { package_name: name.into(), target_sdk_version: 35, ..AndroidPackage::default() };
        edit(&mut ps, &mut pkg);
        ps.pkg = Some(Arc::new(pkg));
        ps
    }

    fn component(name: &str, package: &str) -> Component {
        Component { name: name.into(), package_name: package.into(), ..Component::default() }
    }

    fn definition(name: &str, package: &str, protection_level: i32) -> Permission {
        Permission { name: name.into(), package: package.into(), protection_level, dynamic: None }
    }

    /// `android` (system, declaring CAMERA in the CAMERA group and
    /// INTERNET), an app with CAMERA granted, one denied by policy, one
    /// denied by the user, two sharing a uid with INTERNET, and an
    /// instant app with both granted.
    fn fixture() -> (State, Vec<Permission>, AccessUser) {
        let android = package("android", 1000, |ps, pkg| {
            ps.is.system = true;
            pkg.permission_groups = vec![pkg::PermissionGroup {
                component: component("android.permission-group.CAMERA", "android"),
                priority: 500,
                request_res: 7,
                ..Default::default()
            }];
            pkg.permissions = vec![
                pkg::Permission {
                    component: component(CAMERA, "android"),
                    group: Some("android.permission-group.CAMERA".into()),
                    protection_level: 1 | PROTECTION_FLAG_INSTANT,
                    ..Default::default()
                },
                pkg::Permission { component: component(INTERNET, "android"), ..Default::default() },
            ];
        });
        let instant = package("instant", 10004, |ps, _| ps.users.get_mut(&0).unwrap().instant_app = true);
        let shared_a = package("shared.a", 10003, |ps, _| ps.shared_user = Some("shared".into()));
        let shared_b = package("shared.b", 10003, |ps, _| ps.shared_user = Some("shared".into()));
        let state = State {
            packages: [
                ("android".into(), android),
                ("granted".into(), package("granted", 10001, |_, _| {})),
                ("denied".into(), package("denied", 10002, |_, _| {})),
                ("shared.a".into(), shared_a),
                ("shared.b".into(), shared_b),
                ("instant".into(), instant),
                ("asked".into(), package("asked", 10005, |_, _| {})),
            ]
            .into(),
            shared_users: [(
                "shared".into(),
                SharedUser {
                    name: "shared".into(),
                    app_id: 10003,
                    packages: vec!["shared.a".into(), "shared.b".into()],
                    ..SharedUser::default()
                },
            )]
            .into(),
            users: [(0, User { id: 0, ..User::default() })].into(),
            ..State::default()
        };
        let mut state = state;
        state.system.system_permissions = Some(BTreeMap::from([(1041, BTreeSet::from([INTERNET.to_string()]))]));
        let defined = vec![definition(CAMERA, "android", 1 | PROTECTION_FLAG_INSTANT), definition(INTERNET, "android", 0)];
        let access = AccessUser {
            app_id_permissions: vec![
                (10001, vec![(CAMERA.into(), RUNTIME_GRANTED | USER_SET)]),
                (10002, vec![(CAMERA.into(), USER_SET | USER_FIXED | POLICY_FIXED)]),
                (10003, vec![(INTERNET.into(), INSTALL_GRANTED)]),
                (10004, vec![(CAMERA.into(), RUNTIME_GRANTED), (INTERNET.into(), INSTALL_GRANTED)]),
                (10005, vec![(CAMERA.into(), USER_SET)]),
            ],
            ..AccessUser::default()
        };
        (state, defined, access)
    }

    fn with_access<T>(f: impl FnOnce(&Access<'_>) -> T) -> T {
        let (state, defined, user0) = fixture();
        let groups = permission_groups(&state);
        let filter = AppsFilter::default();
        let compat = CompatConfig::default();
        f(&Access { state: &state, filter: &filter, defined: &defined, groups: &groups, user0: &user0, compat: &compat })
    }

    /// CtsPermissionTestCases' `PermissionManagerTest.testCheckPermission`
    /// and `PackageManagerTest`'s checks: a granted runtime permission,
    /// a denied one, a package and a user that do not exist.
    #[test]
    fn check_permission_follows_the_grant() {
        with_access(|a| {
            assert_eq!(a.check_permission(1000, "granted", CAMERA, 0).unwrap(), GRANTED);
            assert_eq!(a.check_permission(1000, "denied", CAMERA, 0).unwrap(), DENIED);
            assert_eq!(a.check_permission(1000, "granted", INTERNET, 0).unwrap(), DENIED);
            assert_eq!(a.check_permission(1000, "no.such.package", CAMERA, 0).unwrap(), DENIED);
            assert_eq!(a.check_permission(1000, "granted", CAMERA, 10).unwrap(), DENIED);
            assert_eq!(a.check_uid_permission(10001, CAMERA).unwrap(), GRANTED);
            assert_eq!(a.check_uid_permission(10002, CAMERA).unwrap(), DENIED);
            assert_eq!(a.check_uid_permission(1_010_001, CAMERA).unwrap(), DENIED);
        });
    }

    /// A shared uid's grants are its app id's, so each of its packages
    /// and the uid hold them; a uid of no package holds SystemConfig's.
    #[test]
    fn shared_and_system_uids() {
        with_access(|a| {
            assert_eq!(a.check_permission(1000, "shared.a", INTERNET, 0).unwrap(), GRANTED);
            assert_eq!(a.check_permission(1000, "shared.b", INTERNET, 0).unwrap(), GRANTED);
            assert_eq!(a.check_uid_permission(10003, INTERNET).unwrap(), GRANTED);
            assert_eq!(a.check_uid_permission(1041, INTERNET).unwrap(), GRANTED);
            assert_eq!(a.check_uid_permission(1042, INTERNET).unwrap(), DENIED);
        });
    }

    /// `isSinglePermissionGranted`: an instant app holds a granted
    /// permission only if it is an `instant` one.
    #[test]
    fn instant_apps_hold_only_instant_permissions() {
        with_access(|a| {
            assert_eq!(a.check_permission(10004, "instant", CAMERA, 0).unwrap(), GRANTED);
            assert_eq!(a.check_permission(10004, "instant", INTERNET, 0).unwrap(), DENIED);
            assert_eq!(a.check_uid_permission(10004, INTERNET).unwrap(), DENIED);
            // Definitions are hidden from instant apps.
            assert_eq!(a.get_permission_info(10004, CAMERA, "instant", 0).unwrap(), None);
            assert!(a.all_permission_groups(10004, 0).unwrap().is_empty());
        });
    }

    /// The fuller permission implies the coarser one.
    #[test]
    fn fine_location_implies_coarse() {
        let (state, defined, mut user0) = fixture();
        user0.app_id_permissions[0].1.push(("android.permission.ACCESS_FINE_LOCATION".into(), RUNTIME_GRANTED));
        let groups = permission_groups(&state);
        let (filter, compat) = (AppsFilter::default(), CompatConfig::default());
        let a = Access { state: &state, filter: &filter, defined: &defined, groups: &groups, user0: &user0, compat: &compat };
        assert_eq!(a.check_uid_permission(10001, "android.permission.ACCESS_COARSE_LOCATION").unwrap(), GRANTED);
    }

    /// `getPermissionFlags`' API flags, `isPermissionRevokedByPolicy` and
    /// `shouldShowRequestPermissionRationale` of the caller's package.
    #[test]
    fn flags_policy_and_rationale() {
        with_access(|a| {
            assert_eq!(a.permission_flags(1000, "granted", CAMERA, 0).unwrap(), 1);
            assert_eq!(a.permission_flags(1000, "denied", CAMERA, 0).unwrap(), 1 | 2 | 4);
            assert_eq!(a.permission_flags(1000, "denied", "no.such.permission", 0).unwrap(), 0);
            assert!(a.permission_flags(10001, "granted", CAMERA, 0).unwrap_err().message.contains("Neither user 10001"));
            assert!(a.revoked_by_policy(1000, "denied", CAMERA, 0).unwrap());
            assert!(!a.revoked_by_policy(1000, "granted", CAMERA, 0).unwrap());
            // Granted, or fixed: no rationale; not the caller's: none.
            assert!(!a.should_show_rationale(10001, "granted", CAMERA, 0).unwrap());
            assert!(!a.should_show_rationale(10002, "denied", CAMERA, 0).unwrap());
            assert!(!a.should_show_rationale(1000, "denied", CAMERA, 0).unwrap());
            // Denied by the user and still requestable: a rationale.
            assert!(a.should_show_rationale(10005, "asked", CAMERA, 0).unwrap());
            assert!(!a.revoked_by_policy(10005, "asked", CAMERA, 0).unwrap());
        });
        assert_eq!(to_api_flags(IMPLICIT | LEGACY_GRANTED), FLAG_PERMISSION_REVIEW_REQUIRED);
        assert_eq!(to_api_flags(IMPLICIT | APP_OP_REVOKED), FLAG_PERMISSION_REVOKE_WHEN_REQUESTED | 1 << 3);
        assert_eq!(to_api_flags(SOFT_RESTRICTED), FLAG_PERMISSION_APPLY_RESTRICTION);
    }

    /// `getAllowlistedRestrictedPermissions`' argument checks and its
    /// system allowlist.
    #[test]
    fn allowlisted_restricted_permissions() {
        let (state, defined, mut user0) = fixture();
        user0.app_id_permissions[0].1.push(("android.permission.READ_SMS".into(), SYSTEM_EXEMPT));
        let groups = permission_groups(&state);
        let (filter, compat) = (AppsFilter::default(), CompatConfig::default());
        let a = Access { state: &state, filter: &filter, defined: &defined, groups: &groups, user0: &user0, compat: &compat };
        assert_eq!(
            a.allowlisted(1000, "granted", WHITELIST_SYSTEM, 0).unwrap(),
            Some(vec!["android.permission.READ_SMS".to_string()])
        );
        assert_eq!(a.allowlisted(1000, "granted", WHITELIST_UPGRADE, 0).unwrap(), Some(vec![]));
        assert_eq!(a.allowlisted(1000, "no.such.package", WHITELIST_SYSTEM, 0).unwrap(), None);
        assert_eq!(a.allowlisted(1000, "granted", 8, 0).unwrap_err().message, "Requested flags 0x8, but only 0x7 are allowed");
        assert!(a.allowlisted(10001, "granted", WHITELIST_SYSTEM, 0).unwrap_err().message.contains("system allowlist"));
    }

    /// `getPermissionInfo` and `queryPermissionsByGroup`: installed, the
    /// group's permissions, and a caller targeting before O sees only the
    /// base protection.
    #[test]
    fn definitions() {
        with_access(|a| {
            let camera = a.get_permission_info(1000, CAMERA, "android", 0).unwrap().unwrap();
            assert_eq!(camera.item.name.as_deref(), Some(CAMERA));
            assert_eq!(camera.flags, FLAG_INSTALLED);
            assert_eq!(camera.protection_level, 1 | PROTECTION_FLAG_INSTANT);
            assert_eq!(camera.group.as_deref(), Some("android.permission-group.CAMERA"));
            assert_eq!(a.get_permission_info(1000, "no.such.permission", "android", 0).unwrap(), None);
            let old = generate_permission_info(camera.clone(), 0, 25);
            assert_eq!(old.protection_level, 1);
            let in_group = a.query_permissions_by_group(1000, Some("android.permission-group.CAMERA"), 0).unwrap().unwrap();
            assert_eq!(in_group.iter().map(|p| p.item.name.as_deref().unwrap()).collect::<Vec<_>>(), [CAMERA]);
            let ungrouped = a.query_permissions_by_group(1000, None, 0).unwrap().unwrap();
            assert_eq!(ungrouped.iter().map(|p| p.item.name.as_deref().unwrap()).collect::<Vec<_>>(), [INTERNET]);
            assert_eq!(a.query_permissions_by_group(1000, Some("no.such.group"), 0).unwrap(), None);
            let groups = a.all_permission_groups(1000, 0).unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].priority, 500);
            assert_eq!(a.permission_group_info(1000, "android.permission-group.CAMERA", 0).unwrap(), Some(groups[0].clone()));
        });
    }

    /// `addPermissionGroups`: a system package's declaration replaces an
    /// app's, an app's never replaces another's.
    #[test]
    fn system_packages_take_over_permission_groups() {
        let declaring = |name: &str, app_id: i32, system: bool| {
            package(name, app_id, |ps, pkg| {
                ps.is.system = system;
                pkg.permission_groups =
                    vec![pkg::PermissionGroup { component: component("group", name), ..Default::default() }];
            })
        };
        let state = State {
            packages: [
                ("app.one".into(), declaring("app.one", 10001, false)),
                ("app.two".into(), declaring("app.two", 10002, false)),
                ("system".into(), declaring("system", 10003, true)),
            ]
            .into(),
            ..State::default()
        };
        let groups = permission_groups(&state);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].item.package_name.as_deref(), Some("system"));
    }

    /// The parcels as `PermissionInfo`, `PermissionGroupInfo` and
    /// `SplitPermissionInfoParcelable` write them.
    #[test]
    fn parcels() {
        let group = with_access(|a| a.all_permission_groups(1000, 0).unwrap().remove(0));
        let mut p = Parcel::new();
        group.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("android.permission-group.CAMERA"));
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("android"));
        // labelRes, the label (a plain char sequence, null), icon, logo,
        // no meta-data, banner, showUserIcon (USER_NULL), isArchived.
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 0, -1]);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, -10000]);
        assert!(!r.read_bool().unwrap());
        // descriptionRes, requestRes, the request details, the description,
        // flags, priority.
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 7]);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 0, 0]);
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 500]);
        assert_eq!(r.remaining(), 0);

        let camera = with_access(|a| a.get_permission_info(1000, CAMERA, "android", 0).unwrap().unwrap());
        let mut p = Parcel::new();
        camera.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string8().unwrap().as_deref(), Some(CAMERA));
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("android"));
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 1]);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 0, -1]);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, -10000]);
        assert!(!r.read_bool().unwrap());
        // protectionLevel, flags, group, backgroundPermission,
        // descriptionRes, requestRes, the description, no known certs.
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [1 | PROTECTION_FLAG_INSTANT, FLAG_INSTALLED]);
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("android.permission-group.CAMERA"));
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 0, 1]);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);

        let split = SplitPermission {
            name: "android.permission.ACCESS_FINE_LOCATION".into(),
            new_permissions: vec!["android.permission.ACCESS_BACKGROUND_LOCATION".into()],
            target_sdk: 29,
        };
        let mut p = Parcel::new();
        SplitPermissionInfo(&split).write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android.permission.ACCESS_FINE_LOCATION"));
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some(ACCESS_BACKGROUND_LOCATION));
        assert_eq!(r.read_i32().unwrap(), 29);
        assert_eq!(r.remaining(), 0);
    }
}
