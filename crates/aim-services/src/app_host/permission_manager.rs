//! `permissionmgr`: PermissionManagerService's `IPermissionManager` for
//! the app host (`PermissionManagerService` and the permission module's
//! `PermissionService`, which Android 16 runs behind it, at
//! `android-16.0.0_r1`, ADR 0013): the checks by package and by uid from
//! the access state the permission service last persisted (`access.abx`:
//! each app id's permission flags, granted among them), through shared
//! uids, with the fuller permissions that imply others and only `instant`
//! permissions for instant apps; the system access state's definitions
//! and trees, also exported as `writeLegacyPermissionsTEMP` exports them;
//! the definitions as the caller may see the packages that declare them
//! (`PermissionInfo` from the declaring package's parse, the permission
//! groups as `AppIdPermissionPolicy` adds them); a package's permission
//! flags, whether policy revoked one, whether to show a rationale, the
//! restricted permissions' allowlists and SystemConfig's split permissions.
//! The app host has no VirtualDeviceManager, so every device id is the
//! default device's, as the original decides without one. Runtime and role
//! permissions are granted and revoked and their flags updated as
//! `PermissionService` mutates the state: user 0's `access.abx` written back
//! (`AccessPersistence`), the package model's grants with it, the listeners
//! told of a runtime permission's change and a uid that lost one killed.
//! Development permissions' changes (their gids are not
//! modelled, #1857), allowlist changes, a virtual device's permissions,
//! one-time sessions, auto-revoke, attribution sources and the permission
//! states are not implemented (#1508).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

mod install;
mod removal;

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_permission_ionpermissionschangelistener as changed_listener;
use aim_service_aidl::android_permission_ipermissionmanager as pm;

use super::compat::CompatConfig;
use super::{AppHost, reply};
use crate::package::apps_filter::{self, AppsFilter, NotModelled, Setting};
use crate::package::info::{
    self, PackageItemInfo, PermissionGroupInfo, PermissionInfo, flags::GET_META_DATA,
};
use crate::package::list_slice::Slice;
use crate::package::model::{PackageState, State};
use crate::package::parse::SplitPermission;
use crate::package::permissions::{self, AccessSystem, AccessUser, Permission};
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
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_SIGNATURE: i32 = 2;
const PROTECTION_FLAG_DEVELOPMENT: i32 = 0x20;
const PROTECTION_FLAG_INSTANT: i32 = 0x1000;
const PROTECTION_FLAG_ROLE: i32 = 0x400_0000;
/// `PermissionInfo.FLAG_REMOVED`, `FLAG_HARD_RESTRICTED`,
/// `FLAG_SOFT_RESTRICTED`.
const FLAG_REMOVED: i32 = 1 << 1;
const FLAG_HARD_RESTRICTED: i32 = 1 << 2;
const FLAG_SOFT_RESTRICTED: i32 = 1 << 3;
const FLAG_INSTALLED: i32 = 1 << 30;
/// `Build.VERSION_CODES.M`, `O`, `Q`, `CUR_DEVELOPMENT`.
const M: i32 = 23;
const O: i32 = 26;
const Q: i32 = 29;
const CUR_DEVELOPMENT: i32 = 10000;
/// `PermissionFlags` (services/permission).
const INSTALL_GRANTED: i32 = 1 << 0;
const INSTALL_REVOKED: i32 = 1 << 1;
const PROTECTION_GRANTED: i32 = 1 << 2;
const ROLE: i32 = 1 << 3;
const RUNTIME_GRANTED: i32 = 1 << 4;
const USER_SET: i32 = 1 << 5;
const USER_FIXED: i32 = 1 << 6;
const POLICY_FIXED: i32 = 1 << 7;
const SYSTEM_FIXED: i32 = 1 << 8;
const PREGRANT: i32 = 1 << 9;
const LEGACY_GRANTED: i32 = 1 << 10;
const IMPLICIT_GRANTED: i32 = 1 << 11;
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
/// The flags only the system may change (`updatePermissionFlags`):
/// `FLAG_PERMISSION_SYSTEM_FIXED`, `GRANTED_BY_DEFAULT`, the restriction
/// exemptions and `APPLY_RESTRICTION`.
const SYSTEM_ONLY_API_FLAGS: i32 =
    1 << 4 | 1 << 5 | 1 << 12 | 1 << 11 | 1 << 13 | FLAG_PERMISSION_APPLY_RESTRICTION;
const FLAG_PERMISSION_POLICY_FIXED: i32 = 1 << 2;
/// `PackageManager.FLAG_PERMISSION_WHITELIST_*`.
const WHITELIST_SYSTEM: i32 = 1 << 0;
const WHITELIST_INSTALLER: i32 = 1 << 1;
const WHITELIST_UPGRADE: i32 = 1 << 2;
/// `PermissionService.UNREQUESTABLE_MASK`, `BACKGROUND_RATIONALE_CHANGE_ID`,
/// `FULLER_PERMISSIONS`.
const UNREQUESTABLE_MASK: i32 = RESTRICTION_REVOKED | SYSTEM_FIXED | POLICY_FIXED | USER_FIXED;
const BACKGROUND_RATIONALE_CHANGE_ID: i64 = 147_316_723;
const FULLER_PERMISSIONS: &[(&str, &str)] = &[
    (
        "android.permission.ACCESS_COARSE_LOCATION",
        "android.permission.ACCESS_FINE_LOCATION",
    ),
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
const WHITELIST_RESTRICTED_PERMISSIONS: &str =
    "android.permission.WHITELIST_RESTRICTED_PERMISSIONS";
const OBSERVE_GRANT_REVOKE_PERMISSIONS: &str =
    "android.permission.OBSERVE_GRANT_REVOKE_PERMISSIONS";
const GRANT_RUNTIME_PERMISSIONS: &str = "android.permission.GRANT_RUNTIME_PERMISSIONS";
const REVOKE_RUNTIME_PERMISSIONS: &str = "android.permission.REVOKE_RUNTIME_PERMISSIONS";
const ADJUST_RUNTIME_PERMISSIONS_POLICY: &str =
    "android.permission.ADJUST_RUNTIME_PERMISSIONS_POLICY";
const BACKUP: &str = "android.permission.BACKUP";
/// `PermissionService.NOTIFICATIONS_PERMISSIONS`.
const POST_NOTIFICATIONS: &str = "android.permission.POST_NOTIFICATIONS";
/// `PermissionManager.KILL_APP_REASON_PERMISSIONS_REVOKED`.
const KILL_APP_REASON_PERMISSIONS_REVOKED: &str = "permissions revoked";
/// `UserManager.DISALLOW_DEBUGGING_FEATURES`, `Process.SHELL_UID`.
const DISALLOW_DEBUGGING_FEATURES: &str = "no_debugging_features";
const SHELL_UID: i32 = 2000;
/// `Settings.Secure.USER_SETUP_PERSONALIZATION_STARTED`.
const USER_SETUP_PERSONALIZATION_STARTED: i32 = 1;
/// AccessPersistence's system and user 0 state.
const SYSTEM_ACCESS: &str = "misc/apexdata/com.android.permission/access.abx";
const USER0_ACCESS: &str = "misc_de/0/apexdata/com.android.permission/access.abx";
const PERMISSION_GROUP_INFO: &str = "android.content.pm.PermissionGroupInfo";
const PERMISSION_INFO: &str = "android.content.pm.PermissionInfo";

fn not_modelled(e: NotModelled) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, format!("permissionmgr: {}", e.0))
}

/// Kotlin's check of a non-null parameter of `PermissionService.method`.
fn non_null<'s>(
    value: Option<&'s str>,
    method: &str,
    parameter: &str,
) -> Result<&'s str, Exception> {
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
    let mut api = API_FLAGS
        .iter()
        .filter(|(f, _)| has(*f))
        .fold(0, |api, (_, a)| api | a);
    if has(IMPLICIT) {
        api |= if has(LEGACY_GRANTED) {
            FLAG_PERMISSION_REVIEW_REQUIRED
        } else {
            FLAG_PERMISSION_REVOKE_WHEN_REQUESTED
        };
    }
    if has(RESTRICTION_REVOKED) || has(SOFT_RESTRICTED) {
        api |= FLAG_PERMISSION_APPLY_RESTRICTION;
    }
    api
}

/// `PermissionFlags.updateFlags`: `old` with its API flags of `mask` set
/// to `values`, `fromApiFlags` keeping the grants' own flags.
fn update_flags(info_flags: i32, old: i32, mask: i32, values: i32) -> i32 {
    let api = (to_api_flags(old) & !mask) | (values & mask);
    let mut flags = old
        & (INSTALL_GRANTED
            | INSTALL_REVOKED
            | PROTECTION_GRANTED
            | RUNTIME_GRANTED
            | LEGACY_GRANTED
            | IMPLICIT_GRANTED);
    flags |= API_FLAGS
        .iter()
        .filter(|(_, a)| api & a == *a)
        .fold(0, |f, (internal, _)| f | internal);
    if api & (FLAG_PERMISSION_REVIEW_REQUIRED | FLAG_PERMISSION_REVOKE_WHEN_REQUESTED) != 0 {
        flags |= IMPLICIT;
    }
    if api & FLAG_PERMISSION_APPLY_RESTRICTION != 0 {
        if info_flags & FLAG_HARD_RESTRICTED != 0 {
            flags |= RESTRICTION_REVOKED;
        }
        if info_flags & FLAG_SOFT_RESTRICTED != 0 {
            flags |= SOFT_RESTRICTED;
        }
    }
    flags
}

/// What a permission's protection makes it (`Permission`).
fn is_runtime(p: &Permission) -> bool {
    p.protection_level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS
}

fn is_role(p: &Permission) -> bool {
    p.protection_level & PROTECTION_FLAG_ROLE != 0
}

fn is_development(p: &Permission) -> bool {
    p.protection_level & PROTECTION_MASK_BASE == PROTECTION_SIGNATURE
        && p.protection_level & PROTECTION_FLAG_DEVELOPMENT != 0
}

/// A permission's flags changed (`onPermissionFlagsChanged`) in user 0.
#[derive(Debug)]
struct FlagsChanged {
    app_id: i32,
    permission: String,
    old: i32,
    new: i32,
}

/// `AppIdPermissionPolicy.setPermissionFlags` in `new`: the change, if
/// the flags changed.
fn set_flags(new: &mut AccessUser, app_id: i32, permission: &str, flags: i32) -> Vec<FlagsChanged> {
    let old = new.set_permission_flags(app_id, permission, flags);
    if old == flags {
        return Vec::new();
    }
    vec![FlagsChanged {
        app_id,
        permission: permission.to_owned(),
        old,
        new: flags,
    }]
}

/// `isRootOrSystemUid`.
fn root_or_system(uid: i32) -> bool {
    matches!(apps_filter::app_id(uid), ROOT_UID | SYSTEM_UID)
}

fn fuller(permission: &str) -> Option<&'static str> {
    FULLER_PERMISSIONS
        .iter()
        .find(|(p, _)| *p == permission)
        .map(|(_, f)| *f)
}

/// `generatePermissionInfo`: installed, its meta-data only if asked,
/// and for a caller targeting an SDK before O only the base protection
/// of a permission that is not a signature one.
fn generate_permission_info(
    mut info: PermissionInfo,
    flags: i32,
    target_sdk: i32,
) -> PermissionInfo {
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
        let Some(pkg) = ps.pkg.as_deref() else {
            continue;
        };
        if ps.users.values().all(|u| u.instant_app) {
            continue;
        }
        for g in &pkg.permission_groups {
            let group = info::generate_permission_group_info(g, GET_META_DATA);
            match groups
                .iter()
                .position(|(old, _)| old.item.name == group.item.name)
            {
                Some(i) => {
                    let (old, old_system) = &groups[i];
                    if old.item.package_name != group.item.package_name
                        && (!ps.is.system || *old_system)
                    {
                        continue;
                    }
                    groups[i] = (group, ps.is.system);
                }
                None => groups.push((group, ps.is.system)),
            }
        }
    }
    info::array_order(groups.into_iter().map(|(g, _)| g).collect(), |g| {
        g.item.name.as_deref().unwrap_or_default()
    })
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

/// `UserHandle.PER_USER_RANGE`.
const PER_USER_RANGE: i32 = 100_000;

/// `PermissionService.getGidsForUid` of a uid in `user` whose state holds
/// `flags`: SystemConfig's global gids, then those of each permission it
/// holds, in the state's order (an `IndexedMap`: by name hash), a per-user
/// one's as `user`'s uids; the global ones alone with no state.
fn uid_gids(
    config: &crate::package::system_config::SystemConfig,
    flags: Option<&permissions::Named>,
    user: i32,
) -> Vec<i32> {
    let mut gids = config.global_gids.clone();
    let Some(flags) = flags else {
        return gids;
    };
    for (name, flags) in crate::package::info::array_order(flags.clone(), |(n, _)| n) {
        let Some(permission) = config.permissions.get(&name) else {
            continue;
        };
        if !permission_granted(flags) {
            continue;
        }
        gids.extend(
            permission
                .gids
                .iter()
                .map(|&gid| match permission.per_user {
                    // `UserHandle.getUid`.
                    true => user * PER_USER_RANGE + gid % PER_USER_RANGE,
                    false => gid,
                }),
        );
    }
    gids
}

/// The order of a `TreeSet` of names: by UTF-16 code unit, each once.
fn tree_order(mut names: Vec<String>) -> Vec<String> {
    names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    names.dedup();
    names
}

/// `PermissionPersistenceBridge.captureDefinitions`: `LegacyPermissionSettings`
/// replaces its two ArrayMaps with `toLegacyPermissions` (UID/GIDs unused),
/// then `LegacyPermission.write` writes each definition, Android 16 r1.
fn legacy_definitions(system: &AccessSystem) -> Result<Vec<u8>, String> {
    use aim_android_xml::{Element, Node, Value};
    let element = |name: &str, attrs, content| Element {
        name: name.into(),
        attrs,
        content,
    };
    let mut sections = Vec::new();
    for (tag, values) in [
        ("permissions", &system.permissions),
        ("permission-trees", &system.permission_trees),
    ] {
        let mut items = Vec::new();
        for value in values {
            let mut attrs = vec![
                ("name".into(), Value::String(value.name.clone())),
                ("package".into(), Value::String(value.package.clone())),
            ];
            if value.protection_level != 0 {
                attrs.push(("protection".into(), Value::Int(value.protection_level)));
            }
            if let Some((icon, label)) = &value.dynamic {
                attrs.push(("type".into(), Value::String("dynamic".into())));
                if *icon != 0 {
                    attrs.push(("icon".into(), Value::Int(*icon)));
                }
                if let Some(label) = label {
                    attrs.push(("label".into(), Value::String(label.clone())));
                }
            }
            items.push(Node::Element(element("item", attrs, Vec::new())));
        }
        sections.push(Node::Element(element(tag, Vec::new(), items)));
    }
    aim_android_xml::abx::write(&element("packages", Vec::new(), sections))
}

/// `PackageLegacyPermissions.validate`: an app id and a user inventory
/// of distinct users.
pub(super) fn validate_legacy_identity(app_id: i32, users: &[i32]) -> Result<(), Exception> {
    if app_id < 0 {
        return Err(Exception::illegal_argument(
            "live permission lookup requires an application ID",
        ));
    }
    if app_id >= PER_USER_RANGE || users.is_empty() {
        return Err(Exception::illegal_argument(
            "permission capture requires app ID and resolved users",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    if users.iter().any(|&u| u < 0 || !seen.insert(u)) {
        return Err(Exception::illegal_argument(
            "invalid permission user inventory",
        ));
    }
    Ok(())
}

/// `PackageLegacyPermissions.capture` of the legacy state of `app_id`
/// whose user 0 state holds `flags`: for each of `users`, never missing,
/// each permission it holds that is defined (`runtime` says whether, and
/// if it is a runtime one) in the state's order (an `ArrayMap`: by name
/// hash), granted or not, with its API flags.
fn legacy_state_record(
    app_id: i32,
    users: &[i32],
    flags: Option<&permissions::Named>,
    runtime: impl Fn(&str) -> Option<bool>,
) -> Vec<u8> {
    let mut out = Parcel::new();
    out.write_i32(app_id);
    out.write_i32(users.len() as i32);
    for &user in users {
        out.write_i32(user);
        out.write_bool(false);
        let states: Vec<(String, bool, i32)> = match (user, flags) {
            (0, Some(flags)) => crate::package::info::array_order(flags.clone(), |(n, _)| n)
                .into_iter()
                .filter_map(|(name, f)| runtime(&name).map(|r| (name, r, f)))
                .collect(),
            _ => Vec::new(),
        };
        out.write_i32(states.len() as i32);
        for (name, runtime, flags) in states {
            out.write_string16(Some(&name));
            out.write_bool(runtime);
            out.write_bool(permission_granted(flags));
            out.write_i32(to_api_flags(flags));
        }
    }
    out.data().to_vec()
}

/// `isSinglePermissionGranted`: instant apps hold only instant permissions.
fn single_permission_granted(
    defined: &[Permission],
    flags: i32,
    instant: bool,
    name: &str,
) -> bool {
    permission_granted(flags)
        && (!instant
            || defined
                .iter()
                .find(|p| p.name == name)
                .is_some_and(|p| p.protection_level & PROTECTION_FLAG_INSTANT != 0))
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
    /// `PermissionService.getGrantedPermissions` against validated package owners.
    fn granted_permissions(
        &self,
        package: &str,
        app_id: i32,
        user: i32,
    ) -> Result<Vec<String>, Exception> {
        let ps = self
            .state
            .packages
            .get(package)
            .filter(|ps| ps.shared_user_app_id.unwrap_or(ps.app_id) == app_id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "permission UID owner differs"))?;
        if !self.user_exists(user) || !ps.users.contains_key(&user) {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "permission user owner differs",
            ));
        }
        // The access owner has user 0's state alone.
        if user != 0 {
            return Ok(Vec::new());
        }
        let Some((_, flags)) = self
            .user0
            .app_id_permissions
            .iter()
            .find(|(id, _)| *id == app_id)
        else {
            return Ok(Vec::new());
        };
        let instant = info::user_state(ps, user).instant_app;
        Ok(tree_order(
            flags
                .iter()
                .filter(|(name, _)| self.granted_for_app_id(app_id, instant, name))
                .map(|(name, _)| name.clone())
                .collect(),
        ))
    }

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
        self.granted_for_app_id(
            ps.app_id,
            info::user_state(ps, user).instant_app,
            permission,
        )
    }

    fn granted_for_app_id(&self, app_id: i32, instant: bool, permission: &str) -> bool {
        self.single_granted(app_id, instant, permission)
            || fuller(permission).is_some_and(|f| self.single_granted(app_id, instant, f))
    }

    /// `isSinglePermissionGranted`: an instant app holds only `instant`
    /// permissions.
    fn single_granted(&self, app_id: i32, instant: bool, permission: &str) -> bool {
        single_permission_granted(
            self.defined,
            self.flags(app_id, permission),
            instant,
            permission,
        )
    }

    /// `PackageManagerLocal.FilteredSnapshot.getPackageState`: the
    /// package, unless the caller may not see it in `user`.
    fn package(
        &self,
        name: &str,
        caller: i32,
        user: i32,
    ) -> Result<Option<&PackageState>, Exception> {
        let query = Query {
            state: self.state,
            filter: self.filter,
            calling_uid: caller,
        };
        let name = query.resolve_internal_package_name(name, VERSION_CODE_HIGHEST);
        let Some(ps) = self.state.packages.get(&name) else {
            return Ok(None);
        };
        Ok((!query
            .filtered(Some(ps), caller, user)
            .map_err(not_modelled)?)
        .then_some(ps))
    }

    /// `isPackageVisibleToUid`, in the uid's user.
    fn visible(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self
            .package(package, uid, apps_filter::user_id(uid))?
            .is_some())
    }

    /// `isUidInstantApp`.
    fn instant_caller(&self, uid: i32) -> Result<bool, Exception> {
        Ok(apps_filter::instant_app_package_name(self.state, uid)
            .map_err(not_modelled)?
            .is_some())
    }

    /// `enforceCallingOrSelfCrossUserPermission` with the shell's
    /// restriction: the shell may not reach a user that disallows
    /// debugging features.
    fn enforce_cross_user_not_shell(
        &self,
        caller: i32,
        user: i32,
        restrictions: &BTreeSet<String>,
        message: &str,
    ) -> Result<(), Exception> {
        self.enforce_cross_user(caller, user, true, message)?;
        if apps_filter::app_id(caller) == SHELL_UID
            && restrictions.contains(DISALLOW_DEBUGGING_FEATURES)
        {
            return Err(Exception::security(format!(
                "{message}: Shell is disallowed to access user {user}"
            )));
        }
        Ok(())
    }

    /// The packages of the app id `app_id`: its package or its shared
    /// user's.
    fn packages_of(&self, app_id: i32) -> Vec<&PackageState> {
        match apps_filter::setting(self.state, app_id) {
            Some(Setting::Package(ps)) => vec![ps],
            Some(Setting::Shared(su)) => apps_filter::shared_packages(self.state, su).collect(),
            None => Vec::new(),
        }
    }

    /// `setRuntimePermissionGranted` of a caller: its checks, then the
    /// state's change in `new` (`MutateStateScope
    /// .setRuntimePermissionGranted`), reporting errors.
    #[allow(clippy::too_many_arguments)]
    fn set_runtime_permission_granted(
        &self,
        new: &mut AccessUser,
        restrictions: &BTreeSet<String>,
        caller: i32,
        package: &str,
        permission: &str,
        user: i32,
        granted: bool,
    ) -> Result<Vec<FlagsChanged>, Exception> {
        let method = if granted {
            "grantRuntimePermission"
        } else {
            "revokeRuntimePermission"
        };
        if !self.user_exists(user) {
            eprintln!("app host: permissionmgr: {method}: Unknown user {user}");
            return Ok(Vec::new());
        }
        self.enforce_cross_user_not_shell(caller, user, restrictions, method)?;
        let enforced = if granted {
            GRANT_RUNTIME_PERMISSIONS
        } else {
            REVOKE_RUNTIME_PERMISSIONS
        };
        self.enforce(caller, enforced, method)?;
        let controller = self
            .state
            .system
            .permission_controller_package
            .as_ref()
            .ok_or_else(|| not_modelled(NotModelled("the permission controller package")))?
            .as_deref()
            .and_then(|name| self.state.packages.get(name));
        // Unknown and invisible packages alike, not to leak which exist.
        let Some((ps, pkg)) = self
            .package(package, caller, user)?
            .and_then(|ps| Some((ps, ps.pkg.as_deref()?)))
        else {
            eprintln!("app host: permissionmgr: {method}: Unknown package {package}");
            return Ok(Vec::new());
        };
        let can_manage_role = root_or_system(caller)
            || controller.is_some_and(|c| c.app_id == apps_filter::app_id(caller));
        let override_policy_fixed = self.holds(caller, ADJUST_RUNTIME_PERMISSIONS_POLICY)?;
        let definition = self.definition(permission).ok_or_else(|| {
            Exception::illegal_argument(format!("Unknown permission {permission}"))
        })?;
        let name = &ps.name;
        if is_development(definition) {
            return Err(not_modelled(NotModelled(
                "a development permission's grant (#1857)",
            )));
        } else if is_role(definition) {
            if !can_manage_role {
                return Err(Exception::security(format!(
                    "Permission {permission} is managed by role"
                )));
            }
        } else if is_runtime(definition) {
            // A legacy app's permissions stay granted, pending review.
            if pkg.target_sdk_version < M {
                return Ok(Vec::new());
            }
            if granted
                && info::user_state(ps, user).instant_app
                && definition.protection_level & PROTECTION_FLAG_INSTANT == 0
            {
                return Err(Exception::security(format!(
                    "Cannot grant non-instant permission {permission} to package {name}"
                )));
            }
        } else {
            return Err(Exception::security(format!(
                "Permission {permission} requested by package {name} is not a changeable permission type"
            )));
        }
        let old = new.permission_flags(ps.app_id, permission);
        if !pkg.requested_permissions.iter().any(|r| r == permission) && old == 0 {
            eprintln!(
                "app host: permissionmgr: Permission {permission} isn't requested by package {name}"
            );
            return Ok(Vec::new());
        }
        let refused = [
            (
                old & SYSTEM_FIXED != 0,
                "Cannot change system fixed permission",
            ),
            (
                old & POLICY_FIXED != 0 && !override_policy_fixed,
                "Cannot change policy fixed permission",
            ),
            (
                granted && old & RESTRICTION_REVOKED != 0,
                "Cannot grant hard-restricted non-exempt permission",
            ),
            (
                granted && old & SOFT_RESTRICTED != 0,
                "Cannot grant soft-restricted non-exempt permission",
            ),
        ];
        if let Some((_, why)) = refused.iter().find(|(refused, _)| *refused) {
            eprintln!("app host: permissionmgr: {method}: {why} {permission} for package {name}");
            return Ok(Vec::new());
        }
        let flags = if granted {
            old | RUNTIME_GRANTED
        } else {
            old & !RUNTIME_GRANTED
        };
        Ok(set_flags(new, ps.app_id, permission, flags))
    }

    /// `Permission.isRemoved`: its declaration's `PermissionInfo.FLAG_REMOVED`.
    fn removed(&self, p: &Permission) -> bool {
        self.state
            .packages
            .get(&p.package)
            .and_then(|ps| ps.pkg.as_deref())
            .and_then(|pkg| pkg.permissions.iter().find(|d| d.component.name == p.name))
            .is_some_and(|d| d.component.flags & FLAG_REMOVED != 0)
    }

    /// `PermissionService.resetRuntimePermissions` of `package` in `user`
    /// in `new`: `AppIdPermissionPolicy`'s, then `DevicePermissionPolicy`'s,
    /// which only a virtual device's permissions have (#1508). A runtime
    /// permission no other package of the app id requests and neither the
    /// system nor a policy fixed loses what the user set, and its grant
    /// unless a role or the default grants gave it.
    fn reset_runtime_permissions(
        &self,
        new: &mut AccessUser,
        package: &str,
        user: i32,
    ) -> Result<Vec<FlagsChanged>, Exception> {
        let Some(ps) = self.state.packages.get(package) else {
            return Ok(Vec::new());
        };
        let Some(pkg) = ps.pkg.as_deref() else {
            return Ok(Vec::new());
        };
        // The access state is user 0's, the model's only user.
        if user != 0 {
            return Ok(Vec::new());
        }
        let others: Vec<&PackageState> = self
            .packages_of(ps.app_id)
            .into_iter()
            .filter(|other| other.name != ps.name)
            .collect();
        let requested_by_others = |permission: &str| {
            others.iter().any(|other| {
                other
                    .pkg
                    .as_deref()
                    .is_some_and(|pkg| pkg.requested_permissions.iter().any(|r| r == permission))
            })
        };
        let devices = new
            .app_id_device_permissions
            .iter()
            .any(|(id, devices)| *id == ps.app_id && !devices.is_empty());
        if devices
            && pkg
                .requested_permissions
                .iter()
                .any(|p| !requested_by_others(p))
        {
            return Err(not_modelled(NotModelled(
                "a virtual device's permissions (#1508)",
            )));
        }
        let system_or_installed = ps.is.system || info::user_state(ps, user).installed;
        let mut changes = Vec::new();
        for permission in &pkg.requested_permissions {
            let Some(definition) = self.definition(permission) else {
                continue;
            };
            if !is_runtime(definition) || self.removed(definition) {
                continue;
            }
            if requested_by_others(permission) {
                continue;
            }
            let old = new.permission_flags(ps.app_id, permission);
            if old & (SYSTEM_FIXED | POLICY_FIXED) != 0 {
                continue;
            }
            let mut flags = if system_or_installed && old & (ROLE | PREGRANT) != 0 {
                old | RUNTIME_GRANTED
            } else {
                old & !(RUNTIME_GRANTED | ROLE | PREGRANT)
            };
            flags &=
                !(USER_SET | USER_FIXED | APP_OP_REVOKED | ONE_TIME | HIBERNATION | USER_SELECTED);
            if flags & LEGACY_GRANTED != 0 {
                flags |= IMPLICIT;
            }
            changes.extend(set_flags(new, ps.app_id, permission, flags));
        }
        Ok(changes)
    }

    /// `updatePermissionFlags` of a caller.
    #[allow(clippy::too_many_arguments)]
    fn update_permission_flags(
        &self,
        new: &mut AccessUser,
        restrictions: &BTreeSet<String>,
        caller: i32,
        package: &str,
        permission: &str,
        (mut mask, mut values): (i32, i32),
        check_adjust_policy: bool,
        user: i32,
    ) -> Result<Vec<FlagsChanged>, Exception> {
        const METHOD: &str = "updatePermissionFlags";
        if !self.user_exists(user) {
            eprintln!("app host: permissionmgr: {METHOD}: Unknown user {user}");
            return Ok(Vec::new());
        }
        self.enforce_cross_user_not_shell(caller, user, restrictions, METHOD)?;
        self.enforce_any(
            caller,
            METHOD,
            &[GRANT_RUNTIME_PERMISSIONS, REVOKE_RUNTIME_PERMISSIONS],
        )?;
        if !root_or_system(caller) && mask & FLAG_PERMISSION_POLICY_FIXED != 0 {
            if check_adjust_policy {
                self.enforce(
                    caller,
                    ADJUST_RUNTIME_PERMISSIONS_POLICY,
                    &format!("Need {ADJUST_RUNTIME_PERMISSIONS_POLICY} to change policy flags"),
                )?;
            } else {
                // `getUidTargetSdkVersion`: the lowest of the uid's
                // packages.
                let target = self
                    .packages_of(apps_filter::app_id(caller))
                    .iter()
                    .filter_map(|ps| ps.pkg.as_ref().map(|pkg| pkg.target_sdk_version))
                    .min()
                    .unwrap_or(CUR_DEVELOPMENT);
                if target >= Q {
                    return Err(Exception::illegal_argument(format!(
                        "{ADJUST_RUNTIME_PERMISSIONS_POLICY} needs to be checked for packages targeting {Q} or later when changing policy flags"
                    )));
                }
            }
        }
        let Some((ps, pkg)) = self
            .package(package, caller, user)?
            .and_then(|ps| Some((ps, ps.pkg.as_deref()?)))
        else {
            eprintln!("app host: permissionmgr: {METHOD}: Unknown package {package}");
            return Ok(Vec::new());
        };
        let requested = pkg.requested_permissions.iter().any(|r| r == permission)
            || self.packages_of(ps.app_id).iter().any(|shared| {
                shared
                    .pkg
                    .as_ref()
                    .is_some_and(|p| p.requested_permissions.iter().any(|r| r == permission))
            });
        // Only the system changes these.
        if !root_or_system(caller) {
            mask &= !SYSTEM_ONLY_API_FLAGS;
            values &= !SYSTEM_ONLY_API_FLAGS;
        }
        let definition = self.definition(permission).ok_or_else(|| {
            Exception::illegal_argument(format!("Unknown permission {permission}"))
        })?;
        let old = new.permission_flags(ps.app_id, permission);
        if !requested && old == 0 {
            eprintln!(
                "app host: permissionmgr: {METHOD}: Permission {permission} isn't requested by package {}",
                ps.name
            );
            return Ok(Vec::new());
        }
        let info_flags = self.permission_info(definition).map_or(0, |i| i.flags);
        let flags = update_flags(info_flags, old, mask, values);
        Ok(set_flags(new, ps.app_id, permission, flags))
    }

    /// `checkPermission`.
    fn check_permission(
        &self,
        caller: i32,
        package: &str,
        permission: &str,
        user: i32,
    ) -> Result<i32, Exception> {
        if !self.user_exists(user) {
            return Ok(DENIED);
        }
        let Some(ps) = self.package(package, caller, user)? else {
            return Ok(DENIED);
        };
        Ok(if self.granted(ps, user, permission) {
            GRANTED
        } else {
            DENIED
        })
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
        if let Some(ps) = packages
            .into_iter()
            .find(|ps| info::user_state(ps, user).installed && ps.pkg.is_some())
        {
            return Ok(if self.granted(ps, user, permission) {
                GRANTED
            } else {
                DENIED
            });
        }
        let grants = self
            .state
            .system
            .system_permissions
            .as_ref()
            .ok_or_else(|| {
                not_modelled(NotModelled(
                    "SystemConfig system-UID permission assignments",
                ))
            })?;
        let held = grants.get(&uid).is_some_and(|g| {
            g.contains(permission) || fuller(permission).is_some_and(|f| g.contains(f))
        });
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
    fn enforce_cross_user(
        &self,
        caller: i32,
        user: i32,
        full: bool,
        message: &str,
    ) -> Result<(), Exception> {
        if user < 0 {
            return Err(Exception::illegal_argument(format!(
                "userId {user} is invalid"
            )));
        }
        let permission = if full {
            INTERACT_ACROSS_USERS_FULL
        } else {
            INTERACT_ACROSS_USERS
        };
        if user != apps_filter::user_id(caller) && !self.holds(caller, permission)? {
            return Err(Exception::security(format!(
                "{message}: Neither user {caller} nor current process has {permission} to access user {user}"
            )));
        }
        Ok(())
    }

    /// `enforceCallingOrSelfAnyPermission`.
    fn enforce_any(
        &self,
        caller: i32,
        message: &str,
        permissions: &[&str],
    ) -> Result<(), Exception> {
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
        Err(Exception::security(format!(
            "{message}: Neither user {caller} nor current process has {permission}."
        )))
    }

    /// The `PermissionInfo` the permission service keeps for `p`: a
    /// manifest permission's from its package's parse, with its meta-data
    /// (none once the package no longer declares it: the service trims
    /// it); a dynamic one's as it was persisted.
    fn permission_info(&self, p: &Permission) -> Option<PermissionInfo> {
        let Some((icon, label)) = &p.dynamic else {
            let pkg = self.state.packages.get(&p.package)?.pkg.as_deref()?;
            // A later declaration replaces an earlier one.
            let parsed = pkg
                .permissions
                .iter()
                .rev()
                .find(|d| !d.tree && d.component.name == p.name)?;
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
        let Some(p) = self.definition(permission) else {
            return Ok(None);
        };
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
        Ok(self
            .permission_info(p)
            .map(|info| generate_permission_info(info, flags, target_sdk)))
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
            let Some(g) = self
                .groups
                .iter()
                .find(|g| g.item.name.as_deref() == Some(group))
            else {
                return Ok(None);
            };
            if !self.visible(g.item.package_name.as_deref().unwrap_or_default(), caller)? {
                return Ok(None);
            }
        }
        let mut infos = Vec::new();
        for p in self.defined {
            let Some(info) = self.permission_info(p) else {
                continue;
            };
            if info.group.as_deref() == group && self.visible(&p.package, caller)? {
                infos.push(generate_permission_info(info, flags, CUR_DEVELOPMENT));
            }
        }
        Ok(Some(infos))
    }

    /// `getAllPermissionGroups`.
    fn all_permission_groups(
        &self,
        caller: i32,
        flags: i32,
    ) -> Result<Vec<PermissionGroupInfo>, Exception> {
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
    fn permission_group_info(
        &self,
        caller: i32,
        group: &str,
        flags: i32,
    ) -> Result<Option<PermissionGroupInfo>, Exception> {
        if self.instant_caller(caller)? {
            return Ok(None);
        }
        let Some(g) = self
            .groups
            .iter()
            .find(|g| g.item.name.as_deref() == Some(group))
        else {
            return Ok(None);
        };
        if !self.visible(g.item.package_name.as_deref().unwrap_or_default(), caller)? {
            return Ok(None);
        }
        Ok(Some(generate_permission_group_info(g, flags)))
    }

    /// `getPermissionFlags`, of a package the caller sees in its own user.
    fn permission_flags(
        &self,
        caller: i32,
        package: &str,
        permission: &str,
        user: i32,
    ) -> Result<i32, Exception> {
        if !self.user_exists(user) {
            return Ok(0);
        }
        self.enforce_cross_user(caller, user, true, "getPermissionFlags")?;
        self.enforce_any(caller, "getPermissionFlags", RUNTIME_PERMISSION_READERS)?;
        let Some(ps) = self.package(package, caller, apps_filter::user_id(caller))? else {
            return Ok(0);
        };
        if self.definition(permission).is_none() {
            return Ok(0);
        }
        Ok(to_api_flags(self.flags(ps.app_id, permission)))
    }

    /// `isPermissionRevokedByPolicy`: not granted, and fixed by policy.
    fn revoked_by_policy(
        &self,
        caller: i32,
        package: &str,
        permission: &str,
        user: i32,
    ) -> Result<bool, Exception> {
        if !self.user_exists(user) {
            return Ok(false);
        }
        self.enforce_cross_user(caller, user, true, "isPermissionRevokedByPolicy")?;
        let Some(ps) = self.package(package, caller, user)? else {
            return Ok(false);
        };
        Ok(!self.granted(ps, user, permission)
            && self.flags(ps.app_id, permission) & POLICY_FIXED != 0)
    }

    /// `shouldShowRequestPermissionRationale`, for the caller's own
    /// package: a permission it does not hold and may request, which the
    /// user denied (background location always, once its compat change
    /// is enabled for the package).
    fn should_show_rationale(
        &self,
        caller: i32,
        package: &str,
        permission: &str,
        user: i32,
    ) -> Result<bool, Exception> {
        if !self.user_exists(user) {
            return Ok(false);
        }
        self.enforce_cross_user(caller, user, true, "shouldShowRequestPermissionRationale")?;
        let Some(ps) = self.package(package, caller, user)? else {
            return Ok(false);
        };
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
                Some(pkg) => self.compat.is_enabled(
                    BACKGROUND_RATIONALE_CHANGE_ID,
                    Some(&ps.name),
                    pkg.target_sdk_version,
                ),
                None => self
                    .compat
                    .will_be_enabled(BACKGROUND_RATIONALE_CHANGE_ID, &ps.name),
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
    fn allowlisted(
        &self,
        caller: i32,
        package: &str,
        flags: i32,
        user: i32,
    ) -> Result<Option<Vec<String>>, Exception> {
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
        let Some(ps) = self
            .package(package, caller, user)?
            .filter(|ps| ps.pkg.is_some())
        else {
            return Ok(None);
        };
        let privileged = self.holds(caller, WHITELIST_RESTRICTED_PERMISSIONS)?;
        if flags & WHITELIST_SYSTEM != 0 && !privileged {
            return Err(Exception::security(format!(
                "Querying system allowlist requires {WHITELIST_RESTRICTED_PERMISSIONS}"
            )));
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
        for (allowlist, exempt) in [
            (WHITELIST_SYSTEM, SYSTEM_EXEMPT),
            (WHITELIST_UPGRADE, UPGRADE_EXEMPT),
            (WHITELIST_INSTALLER, INSTALLER_EXEMPT),
        ] {
            if flags & allowlist != 0 {
                query |= exempt;
            }
        }
        Ok(self
            .user0
            .app_id_permissions
            .iter()
            .find(|(id, _)| *id == ps.app_id)
            .map(|(_, permissions)| {
                permissions
                    .iter()
                    .filter(|(_, f)| f & query != 0)
                    .map(|(name, _)| name.clone())
                    .collect()
            }))
    }
}

/// `PermissionFlags.isPermissionGranted` (services/permission at
/// `android-16.0.0_r1`).
pub(super) fn permission_granted(flags: i32) -> bool {
    const INSTALL_GRANTED: i32 = 1 << 0;
    const INSTALL_REVOKED: i32 = 1 << 1;
    const PROTECTION_GRANTED: i32 = 1 << 2;
    const RUNTIME_GRANTED: i32 = 1 << 4;
    const LEGACY_GRANTED: i32 = 1 << 10;
    const IMPLICIT_GRANTED: i32 = 1 << 11;
    const RESTRICTION_REVOKED: i32 = 1 << 18;
    let has = |bits: i32| flags & bits == bits;
    if has(INSTALL_GRANTED) {
        return true;
    }
    if has(INSTALL_REVOKED) {
        return false;
    }
    if has(PROTECTION_GRANTED) || has(LEGACY_GRANTED) || has(IMPLICIT_GRANTED) {
        return true;
    }
    if has(RESTRICTION_REVOKED) {
        return false;
    }
    has(RUNTIME_GRANTED)
}

#[derive(Default)]
struct AccessOwner {
    system: Option<Arc<AccessSystem>>,
    user0: Option<Arc<AccessUser>>,
}

pub struct PermissionManager {
    host: Arc<AppHost>,
    /// The sole mutable access owner; readers retain immutable states.
    owner: Mutex<AccessOwner>,
    installs: Mutex<BTreeMap<u64, install::Candidate>>,
    groups: Mutex<Option<(Weak<State>, Arc<Vec<PermissionGroupInfo>>)>>,
    listeners: Mutex<Vec<Binder>>,
    /// `PermissionService.isDelayedPermissionBackupFinished`.
    delayed_backup_finished: Mutex<BTreeSet<i32>>,
}

impl PermissionManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        PermissionManager {
            host,
            owner: Mutex::new(AccessOwner::default()),
            installs: Mutex::new(BTreeMap::new()),
            groups: Mutex::new(None),
            listeners: Mutex::new(Vec::new()),
            delayed_backup_finished: Mutex::new(BTreeSet::new()),
        }
    }

    /// `PermissionService.restoreDelayedRuntimePermissions`: even before
    /// any backup was staged, ask PermissionController once per user. Only
    /// its successful "no more backup" callback closes that user's restore.
    pub(super) fn restore_delayed_runtime_permissions(
        self: &Arc<Self>,
        package: &str,
        user: i32,
    ) -> Result<(), Exception> {
        if user < 0 {
            return Err(Exception::illegal_argument("userId"));
        }
        if self.delayed_backup_finished.lock().unwrap().contains(&user) {
            return Ok(());
        }
        let owner = Arc::downgrade(self);
        super::permission_controller::apply_staged_runtime_permission_backup(
            &self.host,
            package,
            user,
            move |has_more| {
                if let Some(owner) = owner.upgrade() {
                    owner.delayed_backup_completed(user, has_more);
                }
            },
        )
    }

    fn delayed_backup_completed(&self, user: i32, has_more: bool) {
        if !has_more {
            self.delayed_backup_finished.lock().unwrap().insert(user);
        }
    }

    #[cfg(test)]
    pub(super) fn with_system(host: Arc<AppHost>, system: AccessSystem) -> Self {
        let manager = Self::new(host);
        manager.owner.lock().unwrap().system = Some(Arc::new(system));
        manager
    }

    fn system(&self) -> Result<Arc<AccessSystem>, Exception> {
        let mut owner = self.owner.lock().unwrap();
        if owner.system.is_none() {
            owner.system = Some(Arc::new(
                self.read_access(SYSTEM_ACCESS, AccessSystem::parse)?,
            ));
        }
        Ok(owner.system.as_ref().unwrap().clone())
    }

    pub(crate) fn retained_access(&self) -> Result<Arc<permissions::AccessView>, Exception> {
        self.system()?;
        self.user0()?;
        let owner = self.owner.lock().unwrap();
        Ok(Arc::new(permissions::AccessView {
            system: owner.system.as_ref().unwrap().clone(),
            user0: owner.user0.as_ref().unwrap().clone(),
        }))
    }

    fn read_access<T>(
        &self,
        path: &str,
        parse: impl Fn(&aim_android_xml::Element) -> Result<T, String>,
    ) -> Result<T, Exception> {
        crate::package::atomic(&self.host.data()?.join(path), parse)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
            .ok_or_else(|| {
                self.host.unimplemented_named(
                    "permissionmgr",
                    &format!("{path}: access state migration without access.abx (#2168)"),
                )
            })
    }

    /// `PermissionService.writeLegacyPermissionsTEMP` / `toLegacyPermissions`.
    pub(super) fn capture_definitions(&self) -> Result<Vec<u8>, Exception> {
        legacy_definitions(self.system()?.as_ref()).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
    }

    /// User 0's access state, from `access.abx` until it changes.
    fn user0(&self) -> Result<Arc<AccessUser>, Exception> {
        let mut owner = self.owner.lock().unwrap();
        if owner.user0.is_none() {
            owner.user0 = Some(Arc::new(self.read_access(USER0_ACCESS, AccessUser::parse)?));
        }
        Ok(owner.user0.as_ref().unwrap().clone())
    }

    pub(super) fn uid_mode(&self, app_id: i32, op: &str) -> Result<Option<i32>, Exception> {
        let state = self.user0()?;
        Ok(state
            .app_id_app_ops
            .iter()
            .find(|(id, _)| *id == app_id)
            .and_then(|(_, ops)| ops.iter().find(|(name, _)| name == op))
            .map(|(_, mode)| *mode))
    }

    pub(super) fn package_mode(&self, package: &str, op: &str) -> Result<Option<i32>, Exception> {
        let state = self.user0()?;
        Ok(state
            .package_app_ops
            .iter()
            .find(|(name, _)| name == package)
            .and_then(|(_, ops)| ops.iter().find(|(name, _)| name == op))
            .map(|(_, mode)| *mode))
    }

    /// `PermissionService.getInstalledPermissions`: the names of the
    /// permissions `package` defines, in `TreeSet` order.
    pub(super) fn installed_permissions(&self, package: &str) -> Result<Vec<String>, Exception> {
        Ok(tree_order(
            self.system()?
                .permissions
                .iter()
                .filter(|p| p.package == package)
                .map(|p| p.name.clone())
                .collect(),
        ))
    }

    /// `PackageBootstrapBridge.getPackageGrantedPermissions`: `package`
    /// of `app_id`, then `PermissionService.getGrantedPermissions` in
    /// `user`, in `TreeSet` order.
    pub(super) fn granted_permissions(
        &self,
        package: &str,
        app_id: i32,
        user: i32,
    ) -> Result<Vec<String>, Exception> {
        let system =
            self.host.system.get().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "native package owner unavailable")
            })?;
        let bridge = system.package_bootstrap()?;
        let capture = system.capture_package_queries_for(&bridge)?;
        let view = self.retained_access()?;
        self.access_with_capture(&view.system, &view.user0, &capture, "package", |access| {
            access.granted_permissions(package, app_id, user)
        })
    }

    /// `PermissionService.getLegacyPermissionState` of `app_id`, captured
    /// for `users` as `PackageLegacyPermissions.capture` writes it; the
    /// model has user 0's state alone.
    pub(super) fn legacy_permission_state(
        &self,
        app_id: i32,
        users: &[i32],
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<Vec<u8>, Exception> {
        self.retained_access()?.legacy_state(app_id, users, config)
    }

    /// `PermissionService.getGidsForUid` (`uid_gids`), of user 0's state.
    pub(super) fn gids_for_uid(
        &self,
        uid: i32,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<Vec<i32>, Exception> {
        if uid < 0 {
            return Err(Exception::illegal_argument("negative permission UID"));
        }
        if let Some(error) = &config.permission_error {
            return Err(Exception::new(EX_ILLEGAL_STATE, error.clone()));
        }
        let user0 = match uid / PER_USER_RANGE {
            0 => Some(self.user0()?),
            _ => None,
        };
        let flags = user0.as_ref().and_then(|user0| {
            user0
                .app_id_permissions
                .iter()
                .find(|(id, _)| *id == uid % PER_USER_RANGE)
                .map(|(_, flags)| flags)
        });
        Ok(uid_gids(config, flags, uid / PER_USER_RANGE))
    }

    fn access<T>(
        &self,
        f: impl FnOnce(&Access<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let view = self.retained_access()?;
        self.access_with(&view.system, &view.user0, f)
    }

    /// `getPermissionGroupInfo` from `caller`.
    pub(super) fn group_info(
        &self,
        caller: i32,
        group: &str,
        flags: i32,
    ) -> Result<Option<PermissionGroupInfo>, Exception> {
        self.access(|access| access.permission_group_info(caller, group, flags))
    }

    fn access_with<T>(
        &self,
        system: &AccessSystem,
        user0: &AccessUser,
        f: impl FnOnce(&Access<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        self.host.platform()?;
        let capture = self.host.package_capture()?;
        self.access_with_capture(system, user0, &capture, "package", f)
    }

    fn access_with_capture<T>(
        &self,
        system: &AccessSystem,
        user0: &AccessUser,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        resolution_context: &str,
        f: impl FnOnce(&Access<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let platform = self.host.platform()?;
        let state = capture.state();
        let resolution = capture.resolution().map_err(|error| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("{resolution_context} resolution: {error:?}"),
            )
        })?;
        let groups = self.groups(state);
        f(&Access {
            state,
            filter: &resolution.apps_filter,
            defined: &system.permissions,
            groups: &groups,
            user0,
            compat: &platform.compat,
        })
    }

    fn groups(&self, state: &Arc<State>) -> Arc<Vec<PermissionGroupInfo>> {
        let mut held = self.groups.lock().unwrap();
        if let Some((source, groups)) = &*held
            && source.ptr_eq(&Arc::downgrade(state))
        {
            return groups.clone();
        }
        let groups = Arc::new(permission_groups(state));
        *held = Some((Arc::downgrade(state), groups.clone()));
        groups
    }

    /// `service.mutateState` of user 0's state: `change` decides the new
    /// state from the current one; a changed state is written
    /// (`AccessPersistence`), the package model's grants follow it, and
    /// the flags-changed listener's effects run (`onStateMutated`): the
    /// listeners told of each uid whose runtime permissions changed, and
    /// a uid that lost one killed unless `kill` is none.
    fn mutate(
        &self,
        kill: Option<&str>,
        change: impl FnOnce(&Access<'_>, &mut AccessUser) -> Result<Vec<FlagsChanged>, Exception>,
    ) -> Result<(), Exception> {
        self.retained_access()?;
        let mut held = self.owner.lock().unwrap();
        let before = held.user0.clone().expect("loaded");
        let system = held.system.clone().expect("loaded");
        let mut after = (*before).clone();
        let changes = self.access_with(&system, &before, |access| change(access, &mut after))?;
        if changes.is_empty() {
            return Ok(());
        }
        self.write_user0(&after)?;
        let after = Arc::new(after);
        held.user0 = Some(after.clone());
        drop(held);
        self.grants_changed()?;
        let effects = self.on_state_mutated(&changes, kill);
        let audio = self.host.refresh_audio_permissions();
        effects?;
        audio
    }

    /// `PermissionManagerServiceInternal.resetRuntimePermissions` of
    /// `package` in `user`, as PackageManager clears its data: the
    /// change's effects run as a revocation's (`onStateMutated`).
    pub(super) fn reset_runtime_permissions(
        &self,
        package: &str,
        user: i32,
    ) -> Result<(), Exception> {
        self.mutate(Some(KILL_APP_REASON_PERMISSIONS_REVOKED), |access, new| {
            access.reset_runtime_permissions(new, package, user)
        })
    }

    /// `AccessPersistence.writeUserState` of user 0 through `AtomicFile`
    /// with its reserve copy: the document as read with its
    /// `<app-id-permissions>` written anew.
    fn write_user0(&self, state: &AccessUser) -> Result<(), Exception> {
        let fail = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{USER0_ACCESS}: {e}"));
        let path = self.host.data()?.join(USER0_ACCESS);
        let backup = path.with_extension("abx.bak");
        let read = if backup.exists() { &backup } else { &path };
        let bytes = std::fs::read(read).map_err(|e| fail(e.to_string()))?;
        let root = aim_android_xml::read(&bytes).map_err(fail)?;
        let root = permissions::with_app_id_permissions(root, &state.app_id_permissions)
            .and_then(|root| permissions::with_package_versions(root, &state.package_versions))
            .map_err(fail)?;
        let bytes = aim_android_xml::abx::write(&root).map_err(fail)?;
        self.write_access(USER0_ACCESS, &bytes)
    }

    fn write_system(&self, state: &AccessSystem) -> Result<(), Exception> {
        let path = self.host.data()?.join(SYSTEM_ACCESS);
        let backup = path.with_extension("abx.bak");
        let bytes = std::fs::read(if backup.exists() { &backup } else { &path })
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.to_string()))?;
        let root = aim_android_xml::read(&bytes)
            .and_then(|root| permissions::with_system_permissions(root, state))
            .and_then(|root| aim_android_xml::abx::write(&root))
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        self.write_access(SYSTEM_ACCESS, &root)
    }

    fn write_access(&self, relative: &str, bytes: &[u8]) -> Result<(), Exception> {
        let fail = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{relative}: {e}"));
        let path = self.host.data()?.join(relative);
        let backup = path.with_extension("abx.bak");
        // New files of the system's, under system_server's umask (077).
        let owner = aim_storage::guest_inode::GuestInode {
            uid: Some(SYSTEM_UID as u32),
            gid: Some(SYSTEM_UID as u32),
            mode: Some(0o600),
        };
        let write = |to: &std::path::Path| -> std::io::Result<()> {
            match std::fs::remove_file(to) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(to)?;
            std::io::Write::write_all(&mut file, &bytes)?;
            crate::fsync(&file)?;
            aim_storage::guest_inode::record(to, owner)
        };
        let new = path.with_extension("abx.new");
        write(&new)
            .and_then(|()| std::fs::rename(&new, &path))
            .and_then(|()| {
                if backup.exists() {
                    std::fs::remove_file(&backup)?;
                }
                Ok(())
            })
            .map_err(|e| fail(e.to_string()))?;
        // `writeWithReserveCopy`: a failed reserve copy is only logged.
        if let Err(e) = write(&path.with_extension("abx.reservecopy")) {
            eprintln!("app host: permissionmgr: Failed to write the reserve copy: {e}");
        }
        Ok(())
    }

    /// Publish the committed access owner's grants and GIDs through the session.
    fn grants_changed(&self) -> Result<(), Exception> {
        let system = self.host.package_system()?;
        let bridge = system.package_bootstrap()?;
        let capture = system.capture_package_queries_for(&bridge)?;
        system.refresh_package_permission_queries(&bridge, &capture)
    }

    /// `OnPermissionFlagsChangedListener.onStateMutated`.
    fn on_state_mutated(
        &self,
        changes: &[FlagsChanged],
        kill: Option<&str>,
    ) -> Result<(), Exception> {
        let system = self.system()?;
        self.permission_effects(&system.permissions, changes, kill)
    }

    fn permission_effects(
        &self,
        definitions: &[Permission],
        changes: &[FlagsChanged],
        kill: Option<&str>,
    ) -> Result<(), Exception> {
        let mut changed = Vec::new();
        // By uid, whether only notification permissions were revoked.
        let mut revoked: BTreeMap<i32, bool> = BTreeMap::new();
        for c in changes {
            let Some(definition) = definitions.iter().find(|p| p.name == c.permission) else {
                continue;
            };
            if !is_runtime(definition) {
                continue;
            }
            // User 0's uid is its app id.
            let uid = c.app_id;
            if permission_granted(c.old) && !permission_granted(c.new) {
                let only = revoked.get(&uid).copied().unwrap_or(true);
                revoked.insert(uid, c.permission == POST_NOTIFICATIONS && only);
            }
            if !changed.contains(&uid) {
                changed.push(uid);
            }
        }
        let listeners = self.listeners.lock().unwrap().clone();
        for uid in changed {
            let mut data = Parcel::new();
            data.write_interface_token(changed_listener::DESCRIPTOR);
            data.write_i32(uid);
            data.write_string16(Some(DEFAULT_DEVICE));
            for listener in &listeners {
                let sent = match *listener {
                    Binder::Handle(handle) => self
                        .host
                        .process
                        .transact(
                            handle,
                            changed_listener::ON_PERMISSIONS_CHANGED,
                            &data,
                            true,
                        )
                        .map(drop),
                    Binder::Local(ptr) => match self.host.process.local_service(ptr) {
                        Some(service) => service
                            .transact(changed_listener::ON_PERMISSIONS_CHANGED, &data, true)
                            .map(drop),
                        None => Ok(()),
                    },
                };
                if let Err(e) = sent {
                    eprintln!(
                        "app host: permissionmgr: Error when calling OnPermissionsChangeListener: {e:?}"
                    );
                }
            }
        }
        let Some(reason) = kill else { return Ok(()) };
        for (uid, only_notifications) in revoked {
            if only_notifications && self.backup_and_restore_running(uid)? {
                continue;
            }
            let host = self.host.clone();
            let reason = reason.to_owned();
            // `handler.post`: after the call.
            std::thread::spawn(move || host.kill_uid_for_permission_change(uid as u32, &reason));
        }
        Ok(())
    }

    /// `isAppBackupAndRestoreRunning`: a uid holding BACKUP while its
    /// user is in setup, or in deferred setup.
    fn backup_and_restore_running(&self, uid: i32) -> Result<bool, Exception> {
        if !self.access(|access| Ok(access.check_uid_permission(uid, BACKUP)? == GRANTED))? {
            return Ok(false);
        }
        let settings = &self.host.platform()?.settings;
        // `getIntForUser`: a setting not set or not an int is none.
        let int = |name: &str| {
            settings
                .get(super::settings::Table::Secure, name)
                .and_then(|v| v.parse::<i32>().ok())
        };
        Ok(int("user_setup_complete") == Some(0)
            || int("user_setup_personalization_state") == Some(USER_SETUP_PERSONALIZATION_STARTED))
    }

    /// The user restrictions a caller's writes are checked against: the
    /// shell's only (`hasUserRestriction` in
    /// `enforceCallingOrSelfCrossUserPermission`).
    fn shell_restrictions(&self, caller: i32) -> Result<BTreeSet<String>, Exception> {
        if apps_filter::app_id(caller) != SHELL_UID {
            return Ok(BTreeSet::new());
        }
        Ok(self.host.user_restrictions()?.clone())
    }

    /// AudioServerPermissionProvider's batch, over one package capture and access-owner view.
    pub(super) fn audio_permission_holders(
        &self,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        permissions: &[&str],
        users: &[i32],
        app_ids: impl Iterator<Item = i32>,
    ) -> Result<Vec<Vec<i32>>, Exception> {
        let view = self.retained_access()?;
        self.access_with_capture(
            &view.system,
            &view.user0,
            capture,
            "audio permission",
            |access| {
                let app_ids: Vec<_> = app_ids.collect();
                permissions
                    .iter()
                    .map(|permission| {
                        let mut holders = Vec::new();
                        for user in users {
                            for &app_id in &app_ids {
                                let uid = user * PER_USER_RANGE + app_id;
                                if access.holds(uid, permission)? {
                                    holders.push(uid);
                                }
                            }
                        }
                        holders.sort_unstable();
                        Ok(holders)
                    })
                    .collect()
            },
        )
    }

    /// `Context.checkPermission(permission, INVALID_PID, uid)` in the
    /// system server, of the default device.
    pub(super) fn holds(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        self.access(|access| access.holds(uid, permission))
    }

    /// The protection level of `permission` as `getPermissionInfo` gives
    /// it to the system server; none for one not defined.
    pub(super) fn protection_level(&self, permission: &str) -> Result<Option<i32>, Exception> {
        Ok(self
            .system()?
            .permissions
            .iter()
            .find(|p| p.name == permission)
            .map(|p| p.protection_level))
    }

    /// A persistent device id: the app host has no virtual devices, and
    /// another device's permissions are not implemented (#1508).
    fn default_device(&self, device: &str) -> Result<(), Exception> {
        if device == DEFAULT_DEVICE {
            return Ok(());
        }
        Err(self
            .host
            .unimplemented_named("permissionmgr", "a virtual device's permissions (#1508)"))
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
                self.access(|access| access.all_permission_groups(caller, a.flags))
                    .map(|groups| {
                        let slice = Slice::new(process(), PERMISSION_GROUP_INFO, groups);
                        parcel(|p| pm::write_get_all_permission_groups_reply(p, Some(&slice)))
                    })
            }
            pm::GET_PERMISSION_GROUP_INFO => {
                let a = pm::GetPermissionGroupInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                non_null(
                    a.group_name.as_deref(),
                    "getPermissionGroupInfo",
                    "permissionGroupName",
                )
                .and_then(|group| {
                    self.access(|access| access.permission_group_info(caller, group, a.flags))
                })
                .map(|info| parcel(|p| pm::write_get_permission_group_info_reply(p, info.as_ref())))
            }
            pm::GET_PERMISSION_INFO => {
                let a = pm::GetPermissionInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let permission = non_null(
                        a.permission_name.as_deref(),
                        "getPermissionInfo",
                        "permissionName",
                    )?;
                    let op_package = non_null(
                        a.package_name.as_deref(),
                        "getPermissionInfo",
                        "opPackageName",
                    )?;
                    self.access(|access| {
                        access.get_permission_info(caller, permission, op_package, a.flags)
                    })
                })()
                .map(|info| parcel(|p| pm::write_get_permission_info_reply(p, info.as_ref())))
            }
            pm::QUERY_PERMISSIONS_BY_GROUP => {
                let a = pm::QueryPermissionsByGroup::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.access(|access| {
                    access.query_permissions_by_group(caller, a.group_name.as_deref(), a.flags)
                })
                .map(|infos| {
                    let slice = infos.map(|infos| Slice::new(process(), PERMISSION_INFO, infos));
                    parcel(|p| pm::write_query_permissions_by_group_reply(p, slice.as_ref()))
                })
            }
            pm::GET_PERMISSION_FLAGS => {
                let a = pm::GetPermissionFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = non_null(
                        a.package_name.as_deref(),
                        "getPermissionFlags",
                        "packageName",
                    )?;
                    let permission = non_null(
                        a.permission_name.as_deref(),
                        "getPermissionFlags",
                        "permissionName",
                    )?;
                    self.default_device(non_null(
                        a.persistent_device_id.as_deref(),
                        "getPermissionFlags",
                        "deviceId",
                    )?)?;
                    self.access(|access| {
                        access.permission_flags(caller, package, permission, a.user_id)
                    })
                })()
                .map(|flags| parcel(|p| pm::write_get_permission_flags_reply(p, flags)))
            }
            pm::ADD_ON_PERMISSIONS_CHANGE_LISTENER | pm::REMOVE_ON_PERMISSIONS_CHANGE_LISTENER => {
                let add = call.code == pm::ADD_ON_PERMISSIONS_CHANGE_LISTENER;
                let (listener, method) = if add {
                    let a = pm::AddOnPermissionsChangeListener::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.listener, "addOnPermissionsChangeListener")
                } else {
                    let a = pm::RemoveOnPermissionsChangeListener::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
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
                let a = pm::GetAllowlistedRestrictedPermissions::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                non_null(
                    a.package_name.as_deref(),
                    "getAllowlistedRestrictedPermissions",
                    "packageName",
                )
                .and_then(|package| {
                    self.access(|access| access.allowlisted(caller, package, a.flags, a.user_id))
                })
                .map(|names| {
                    let names: Option<Vec<Option<String>>> =
                        names.map(|n| n.into_iter().map(Some).collect());
                    parcel(|p| pm::write_get_allowlisted_restricted_permissions_reply(p, &names))
                })
            }
            pm::SHOULD_SHOW_REQUEST_PERMISSION_RATIONALE => {
                let a = pm::ShouldShowRequestPermissionRationale::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let method = "shouldShowRequestPermissionRationale";
                (|| {
                    let package = non_null(a.package_name.as_deref(), method, "packageName")?;
                    let permission =
                        non_null(a.permission_name.as_deref(), method, "permissionName")?;
                    self.access(|access| {
                        access.should_show_rationale(caller, package, permission, a.user_id)
                    })
                })()
                .map(|show| {
                    parcel(|p| pm::write_should_show_request_permission_rationale_reply(p, show))
                })
            }
            pm::IS_PERMISSION_REVOKED_BY_POLICY => {
                let a =
                    pm::IsPermissionRevokedByPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = non_null(
                        a.package_name.as_deref(),
                        "isPermissionRevokedByPolicy",
                        "packageName",
                    )?;
                    let permission = non_null(
                        a.permission_name.as_deref(),
                        "isPermissionRevokedByPolicy",
                        "permissionName",
                    )?;
                    self.access(|access| {
                        access.revoked_by_policy(caller, package, permission, a.user_id)
                    })
                })()
                .map(|revoked| {
                    parcel(|p| pm::write_is_permission_revoked_by_policy_reply(p, revoked))
                })
            }
            pm::GET_SPLIT_PERMISSIONS => {
                pm::GetSplitPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host.framework().map(|framework| {
                    let splits: Vec<_> = framework
                        .split_permissions()
                        .iter()
                        .map(|s| Some(SplitPermissionInfo(s)))
                        .collect();
                    parcel(|p| pm::write_get_split_permissions_reply(p, Some(splits.as_slice())))
                })
            }
            pm::CHECK_PERMISSION => {
                let a = pm::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    // `PermissionManagerService.checkPermission`: no package or
                    // permission is denied.
                    let (Some(package), Some(permission)) =
                        (a.package_name.as_deref(), a.permission_name.as_deref())
                    else {
                        return Ok(DENIED);
                    };
                    self.default_device(non_null(
                        a.persistent_device_id.as_deref(),
                        "checkPermission",
                        "deviceId",
                    )?)?;
                    self.access(|access| {
                        access.check_permission(caller, package, permission, a.user_id)
                    })
                })()
                .map(|result| parcel(|p| pm::write_check_permission_reply(p, result)))
            }
            pm::CHECK_UID_PERMISSION => {
                let a = pm::CheckUidPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `PermissionManagerService.checkUidPermission`: no permission
                // is denied.
                let result = match a.permission_name.as_deref() {
                    None => Ok(DENIED),
                    Some(permission) => {
                        self.access(|access| access.check_uid_permission(a.uid, permission))
                    }
                };
                result.map(|result| parcel(|p| pm::write_check_uid_permission_reply(p, result)))
            }
            pm::GRANT_RUNTIME_PERMISSION | pm::REVOKE_RUNTIME_PERMISSION => {
                let (method, package, permission, device, user, reason) =
                    if call.code == pm::GRANT_RUNTIME_PERMISSION {
                        let a = pm::GrantRuntimePermission::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?;
                        let (p, n, d) = (a.package_name, a.permission_name, a.persistent_device_id);
                        ("grantRuntimePermission", p, n, d, a.user_id, None)
                    } else {
                        let a = pm::RevokeRuntimePermission::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?;
                        let (p, n, d) = (a.package_name, a.permission_name, a.persistent_device_id);
                        (
                            "revokeRuntimePermission",
                            p,
                            n,
                            d,
                            a.user_id,
                            Some(a.reason),
                        )
                    };
                (|| {
                    let package = non_null(package.as_deref(), method, "packageName")?;
                    let permission = non_null(permission.as_deref(), method, "permissionName")?;
                    self.default_device(non_null(device.as_deref(), method, "deviceId")?)?;
                    let restrictions = self.shell_restrictions(caller)?;
                    let granted = reason.is_none();
                    // A revocation's reason, else the default one.
                    let kill = reason
                        .flatten()
                        .unwrap_or_else(|| KILL_APP_REASON_PERMISSIONS_REVOKED.to_owned());
                    self.mutate(Some(&kill), |access, new| {
                        access.set_runtime_permission_granted(
                            new,
                            &restrictions,
                            caller,
                            package,
                            permission,
                            user,
                            granted,
                        )
                    })
                })()
                .map(|()| {
                    if call.code == pm::GRANT_RUNTIME_PERMISSION {
                        parcel(pm::write_grant_runtime_permission_reply)
                    } else {
                        parcel(pm::write_revoke_runtime_permission_reply)
                    }
                })
            }
            pm::UPDATE_PERMISSION_FLAGS => {
                let a = pm::UpdatePermissionFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                const METHOD: &str = "updatePermissionFlags";
                (|| {
                    let package = non_null(a.package_name.as_deref(), METHOD, "packageName")?;
                    let permission =
                        non_null(a.permission_name.as_deref(), METHOD, "permissionName")?;
                    self.default_device(non_null(
                        a.persistent_device_id.as_deref(),
                        METHOD,
                        "deviceId",
                    )?)?;
                    let restrictions = self.shell_restrictions(caller)?;
                    self.mutate(Some(KILL_APP_REASON_PERMISSIONS_REVOKED), |access, new| {
                        access.update_permission_flags(
                            new,
                            &restrictions,
                            caller,
                            package,
                            permission,
                            (a.flag_mask, a.flag_values),
                            a.check_adjust_policy_flag_permission,
                            a.user_id,
                        )
                    })
                })()
                .map(|()| parcel(pm::write_update_permission_flags_reply))
            }
            code => {
                let name = pm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("permissionmgr", &format!("{name} (#1508)")))
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

    #[test]
    fn delayed_restore_caches_only_the_controllers_no_more_backup_for_each_user() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let owner = Arc::new(PermissionManager::new(AppHost::new(process)));
        // With no configured package owner, the first request cannot reach
        // PermissionController and must not fabricate a finished restore.
        assert!(owner.restore_delayed_runtime_permissions("p", 0).is_err());
        owner.delayed_backup_completed(0, true);
        assert!(owner.restore_delayed_runtime_permissions("p", 0).is_err());
        owner.delayed_backup_completed(0, false);
        owner.restore_delayed_runtime_permissions("p", 0).unwrap();
        owner
            .restore_delayed_runtime_permissions("updated.package", 0)
            .unwrap();
        assert!(owner.restore_delayed_runtime_permissions("p", 10).is_err());
        assert_eq!(
            owner
                .restore_delayed_runtime_permissions("p", -1)
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
    }

    #[test]
    fn audio_batch_uses_the_live_permission_owner_and_retained_package_capture() {
        crate::system::with_discovery_session(|host, publish, _| {
            let manager = PermissionManager::with_system(
                host.clone(),
                AccessSystem {
                    permissions: vec![definition(CAMERA, "p", 1)],
                    ..Default::default()
                },
            );
            manager.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser {
                app_id_permissions: vec![(10100, vec![(CAMERA.into(), RUNTIME_GRANTED)])],
                ..Default::default()
            }));
            let before = host.package_capture().unwrap();
            assert_eq!(
                manager
                    .audio_permission_holders(
                        &before,
                        &[CAMERA],
                        &[0],
                        [10100, 10101, 1000, 99001].into_iter()
                    )
                    .unwrap(),
                [vec![1000, 10100]]
            );
            manager.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser::default()));
            publish(&mut |pkg| pkg.activities.clear());
            assert_eq!(
                manager
                    .audio_permission_holders(&before, &[CAMERA], &[0], [10100, 10101].into_iter())
                    .unwrap(),
                [Vec::<i32>::new()]
            );
            assert!(!Arc::ptr_eq(&before, &host.package_capture().unwrap()));
        });
    }

    pub(super) struct PersistedAccess {
        pub(super) host: Arc<AppHost>,
        data: std::path::PathBuf,
    }

    impl PersistedAccess {
        pub(super) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let data = std::env::temp_dir().join(format!(
                "aim-permission-bootstrap-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&data).unwrap();
            let process = aim_binder_host::local::LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid: 1000,
                    security_context: None,
                },
            );
            let host = AppHost::new(process);
            host.configure(
                &data.join("missing-image"),
                &data,
                &data,
                &data,
                Box::new(|_| None),
                Box::new(|_, _| {}),
                Box::new(|| Err(std::io::Error::other("no zygote"))),
            )
            .unwrap();
            Self { host, data }
        }

        pub(super) fn write(&self, path: &str, xml: &[u8]) {
            write_access(&self.host, path, xml);
        }
    }

    pub(super) fn write_access(host: &AppHost, path: &str, xml: &[u8]) {
        let path = host.data().unwrap().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let root = aim_android_xml::read(xml).unwrap();
        std::fs::write(path, aim_android_xml::abx::write(&root).unwrap()).unwrap();
    }

    impl Drop for PersistedAccess {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.data).unwrap();
        }
    }

    pub(super) const BOOT_SYSTEM: &[u8] = br#"<access><permissions>
        <permission name="android.permission.INTERNET" packageName="android" protectionLevel="0" type="0"/>
        <permission name="android.permission.CAMERA" packageName="android" protectionLevel="1" type="0"/>
        </permissions><permission-trees>
        <permission name="tree" packageName="android" protectionLevel="0" type="0"/>
        </permission-trees></access>"#;
    pub(super) const BOOT_USER: &[u8] = br#"<access><app-id-permissions><app-id id="10123">
        <permission name="android.permission.CAMERA" flags="10"/>
        <permission name="android.permission.INTERNET" flags="1"/>
        <permission name="configured" flags="1"/>
        <permission name="undefined" flags="1"/>
        </app-id></app-id-permissions></access>"#;

    #[test]
    fn retained_access_keeps_definitions_and_grants_when_owner_changes() {
        let fixture = PersistedAccess::new();
        fixture.write(SYSTEM_ACCESS, BOOT_SYSTEM);
        fixture.write(USER0_ACCESS, BOOT_USER);
        let manager = PermissionManager::new(fixture.host.clone());
        let before = manager.retained_access().unwrap();
        assert!(before.same_base(&manager.retained_access().unwrap()));
        {
            let mut owner = manager.owner.lock().unwrap();
            let mut system = (*before.system).clone();
            system
                .permissions
                .push(definition("fixture.NORMAL", "fixture", 0));
            let mut user = (*before.user0).clone();
            user.set_permission_flags(10123, "fixture.NORMAL", INSTALL_GRANTED);
            owner.system = Some(Arc::new(system));
            owner.user0 = Some(Arc::new(user));
        }
        let after = manager.retained_access().unwrap();
        assert!(!before.same_base(&after));
        assert!(
            !before
                .system
                .permissions
                .iter()
                .any(|p| p.name == "fixture.NORMAL")
        );
        assert_eq!(before.user0.permission_flags(10123, "fixture.NORMAL"), 0);
        assert!(
            after
                .system
                .permissions
                .iter()
                .any(|p| p.name == "fixture.NORMAL")
        );
        assert_eq!(
            after.user0.permission_flags(10123, "fixture.NORMAL"),
            INSTALL_GRANTED
        );
    }

    /// The real boot leaves read their persisted owners even when Inputs cannot
    /// load. No package, platform or resolver is constructed by these calls.
    #[test]
    fn bootstrap_permission_getters_do_not_load_inputs() {
        use crate::package::system_config::{ConfigPermission, SystemConfig};
        use aim_service_aidl::{
            dev_aim_server_ipackagebootstrapbridge as boot,
            dev_aim_server_ipermissionpersistencebridge as persistence,
        };
        let fixture = PersistedAccess::new();
        fixture.write(SYSTEM_ACCESS, BOOT_SYSTEM);
        fixture.write(USER0_ACCESS, BOOT_USER);
        let host = &fixture.host;
        let manager = Arc::new(PermissionManager::new(host.clone()));
        assert!(host.permission_manager.set(manager.clone()).is_ok());
        let mut config = SystemConfig::default();
        config.permissions.insert(
            INTERNET.into(),
            ConfigPermission {
                per_user: false,
                gids: vec![3003],
            },
        );
        config
            .permissions
            .insert("configured".into(), ConfigPermission::default());
        assert_eq!(manager.gids_for_uid(10123, &config).unwrap(), [3003]);
        let record = manager
            .legacy_permission_state(10123, &[0, 10], &config)
            .unwrap();
        let flags = manager.user0().unwrap();
        assert_eq!(
            record,
            legacy_state_record(
                10123,
                &[0, 10],
                Some(&flags.app_id_permissions[0].1),
                |name| {
                    match name {
                        CAMERA => Some(true),
                        INTERNET | "configured" => Some(false),
                        _ => None,
                    }
                }
            )
        );
        let node = |service: Arc<dyn Service>| {
            let Binder::Local(ptr) = host.process.add_service(service) else {
                unreachable!()
            };
            host.process.local_service(ptr).unwrap()
        };
        let bootstrap = node(Arc::new(
            super::super::package_bootstrap::PackageBootstrap::new(host.clone()),
        ));
        let mut request = Parcel::new();
        boot::GetPackageInstalledPermissions {
            package_name: Some("android".into()),
        }
        .write(&mut request);
        let reply = bootstrap
            .transact(boot::GET_PACKAGE_INSTALLED_PERMISSIONS, &request, false)
            .unwrap();
        assert_eq!(
            boot::read_get_package_installed_permissions_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            Some(vec![Some(CAMERA.into()), Some(INTERNET.into())])
        );
        let mut request = Parcel::new();
        boot::GetLegacyPermissionState {
            app_id: 10123,
            user_ids: Some(vec![0, 10]),
        }
        .write(&mut request);
        let reply = bootstrap
            .transact(boot::GET_LEGACY_PERMISSION_STATE, &request, false)
            .unwrap();
        assert_eq!(
            boot::read_get_legacy_permission_state_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            Some(
                manager
                    .legacy_permission_state(10123, &[0, 10], &SystemConfig::default())
                    .unwrap()
            )
        );
        let mut request = Parcel::new();
        boot::GetPermissionGidsForUid { uid: 10123 }.write(&mut request);
        let reply = bootstrap
            .transact(boot::GET_PERMISSION_GIDS_FOR_UID, &request, false)
            .unwrap();
        assert_eq!(
            boot::read_get_permission_gids_for_uid_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            Some(vec![])
        );
        let persistence = node(Arc::new(
            super::super::package_internal_leaves::PermissionPersistence::new(host.clone()),
        ));
        let mut request = Parcel::new();
        persistence::CaptureDefinitions {}.write(&mut request);
        let reply = persistence
            .transact(persistence::CAPTURE_DEFINITIONS, &request, false)
            .unwrap();
        assert_eq!(
            persistence::read_capture_definitions_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            Some(manager.capture_definitions().unwrap())
        );
        assert!(host.platform.get().is_none());
        assert_eq!(
            host.inputs().err().unwrap().message,
            "no property ro.build.version.sdk"
        );
        assert_eq!(
            manager
                .granted_permissions("android", 1000, 0)
                .unwrap_err()
                .message,
            "native package owner unavailable"
        );
    }

    /// AccessPersistence's backup wins; a malformed document tries the reserve
    /// copy. Missing access is the migration boundary, not an empty grant set.
    #[test]
    fn persisted_access_errors_and_atomic_recovery() {
        let fixture = PersistedAccess::new();
        for (path, xml) in [(SYSTEM_ACCESS, BOOT_SYSTEM), (USER0_ACCESS, BOOT_USER)] {
            let read = || {
                let manager = PermissionManager::new(fixture.host.clone());
                if path == SYSTEM_ACCESS {
                    manager.capture_definitions().map(|_| ())
                } else {
                    manager.gids_for_uid(10123, &Default::default()).map(|_| ())
                }
            };
            let missing = read().unwrap_err();
            assert_eq!(missing.code, EX_UNSUPPORTED_OPERATION);
            assert!(missing.message.contains("#2168"));
            assert!(missing.message.contains(path));
            fixture.write(path, xml);
            std::fs::write(fixture.data.join(format!("{path}.bak")), b"broken").unwrap();
            assert_eq!(read().unwrap_err().code, EX_ILLEGAL_STATE);
            fixture.write(&format!("{path}.reservecopy"), xml);
            read().unwrap();
            std::fs::write(fixture.data.join(path), b"broken").unwrap();
            fixture.write(&format!("{path}.bak"), xml);
            std::fs::remove_file(fixture.data.join(format!("{path}.reservecopy"))).unwrap();
            read().unwrap();
            std::fs::remove_file(fixture.data.join(format!("{path}.bak"))).unwrap();
            assert_eq!(read().unwrap_err().code, EX_ILLEGAL_STATE);
            fixture.write(&format!("{path}.reservecopy"), xml);
            read().unwrap();
            std::fs::remove_file(fixture.data.join(path)).unwrap();
            assert_eq!(read().unwrap_err().code, EX_UNSUPPORTED_OPERATION);
        }
    }

    /// `AppIdPermissionPersistence.parsePermission`, `toLegacyPermissions`
    /// and `LegacyPermissionSettings.writePermissions` / `writePermissionTrees`:
    /// both maps, their ArrayMap order (including a collision and replacement),
    /// typed decimal attributes and only dynamic permissions' icon and label.
    #[test]
    fn legacy_definitions_from_the_permission_manager() {
        let root = aim_android_xml::read(br#"<access>
            <permissions>
                <permission name="z" packageName="android" protectionLevel="2" type="0" icon="9" label="ignored"/>
                <permission name="Aa" packageName="old" protectionLevel="1" type="2"/>
                <permission name="BB" packageName="owner" protectionLevel="0" type="2" label=""/>
                <permission name="a" packageName="owner" protectionLevel="0" type="0"/>
                <permission name="Aa" packageName="owner" protectionLevel="1002" type="2" icon="-edcba99" label="&lt;dynamic&gt; &amp; label"/>
                <permission name="unknown" packageName="owner" protectionLevel="2" type="99"/>
            </permissions>
            <permission-trees>
                <permission name="tree" packageName="owner" protectionLevel="2" type="0"/>
            </permission-trees>
        </access>"#).unwrap();
        let system = AccessSystem::parse(&root).unwrap();
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let manager = PermissionManager::with_system(AppHost::new(process), system.clone());
        let bytes = manager.capture_definitions().unwrap();
        assert_eq!(bytes, legacy_definitions(&system).unwrap());
        assert_eq!(manager.protection_level("Aa").unwrap(), Some(0x1002));
        assert_eq!(manager.protection_level("a").unwrap(), Some(0));
        assert_eq!(manager.protection_level("tree").unwrap(), None);
        let document = aim_android_xml::abx::read(&bytes).unwrap();
        assert_eq!(document.name, "packages");
        let sections = document.children().collect::<Vec<_>>();
        assert_eq!(
            sections.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["permissions", "permission-trees"]
        );
        let items = sections[0].children().collect::<Vec<_>>();
        assert_eq!(
            items
                .iter()
                .map(|e| e.string("name").unwrap().into_owned())
                .collect::<Vec<_>>(),
            ["a", "z", "Aa", "BB"]
        );
        assert_eq!(items[0].attrs.len(), 2);
        assert_eq!(items[1].attrs.len(), 3);
        assert_eq!(items[1].int("protection").unwrap(), Some(2));
        assert_eq!(items[2].int("protection").unwrap(), Some(0x1002));
        assert_eq!(items[2].int("icon").unwrap(), Some(0xf1234567u32 as i32));
        assert_eq!(items[2].string("package").as_deref(), Some("owner"));
        assert_eq!(
            items[2].string("label").as_deref(),
            Some("<dynamic> & label")
        );
        assert_eq!(items[3].attrs.len(), 4);
        assert_eq!(items[3].string("type").as_deref(), Some("dynamic"));
        assert_eq!(items[3].string("label").as_deref(), Some(""));
        let settings = crate::package::settings::Settings::parse(&document).unwrap();
        assert_eq!(settings.permissions.len(), system.permissions.len());
        assert_eq!(settings.permission_trees[0].name, "tree");
        assert_eq!(
            settings.permissions[2].dynamic,
            Some((0xf1234567u32 as i32, Some("<dynamic> & label".into())))
        );
        assert_eq!(&*manager.system().unwrap(), &system);
    }

    #[test]
    fn empty_legacy_definition_maps_are_written() {
        let root =
            aim_android_xml::abx::read(&legacy_definitions(&AccessSystem::default()).unwrap())
                .unwrap();
        assert_eq!(root.children().count(), 2);
        assert!(root.children().all(|e| e.content.is_empty()));
    }

    const CAMERA: &str = "android.permission.CAMERA";
    const INTERNET: &str = "android.permission.INTERNET";
    const RUNTIME_GRANTED: i32 = 1 << 4;
    const INSTALL_GRANTED: i32 = 1 << 0;

    /// The global gids first, then a granted permission's in the state's
    /// hash order, duplicates kept; a per-user one's as the user's uids; an
    /// ungranted or unconfigured one adds none.
    #[test]
    fn uid_gids_follow_the_permission_service() {
        use crate::package::system_config::{ConfigPermission, SystemConfig};
        let mut config = SystemConfig::default();
        config.global_gids = vec![3003];
        config.permissions = [
            (
                "b".to_string(),
                ConfigPermission {
                    per_user: false,
                    gids: vec![1015, 3003],
                },
            ),
            (
                "a".to_string(),
                ConfigPermission {
                    per_user: true,
                    gids: vec![9997],
                },
            ),
            (
                "c".to_string(),
                ConfigPermission {
                    per_user: false,
                    gids: vec![1],
                },
            ),
        ]
        .into();
        let flags: permissions::Named = vec![
            ("b".into(), INSTALL_GRANTED),
            ("unconfigured".into(), INSTALL_GRANTED),
            ("c".into(), 0),
            ("a".into(), RUNTIME_GRANTED),
        ];
        assert_eq!(uid_gids(&config, Some(&flags), 0), [3003, 9997, 1015, 3003]);
        assert_eq!(
            uid_gids(&config, Some(&flags), 10),
            [3003, 1_009_997, 1015, 3003]
        );
        assert_eq!(uid_gids(&config, None, 0), [3003]);
    }

    /// A `TreeSet` of names: `String.compareTo`'s UTF-16 order, each once.
    #[test]
    fn names_in_tree_set_order() {
        let names = ["b", "a", "\u{10000}", "\u{ffff}", "a"]
            .map(String::from)
            .to_vec();
        assert_eq!(tree_order(names), ["a", "b", "\u{10000}", "\u{ffff}"]);
    }

    /// The capture `PackageLegacyPermissions.restore` reads: each user,
    /// never missing; user 0's defined permissions in hash order with
    /// their kind, grant and API flags; an undefined one left out.
    #[test]
    fn the_legacy_state_record_as_the_bridge_captures_it() {
        let flags: permissions::Named = vec![
            ("b".into(), INSTALL_GRANTED),
            ("undefined".into(), INSTALL_GRANTED),
            ("a".into(), RUNTIME_GRANTED),
        ];
        let runtime = |name: &str| match name {
            "a" => Some(true),
            "b" => Some(false),
            _ => None,
        };
        let bytes = legacy_state_record(10_123, &[0, 10], Some(&flags), runtime);
        let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (10_123, 2));
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (0, 0));
        assert_eq!(r.read_i32().unwrap(), 2);
        for (name, runtime, flags) in [("a", true, RUNTIME_GRANTED), ("b", false, INSTALL_GRANTED)]
        {
            assert_eq!(r.read_string16().unwrap().as_deref(), Some(name));
            assert_eq!(r.read_i32().unwrap(), runtime as i32);
            assert_eq!(r.read_i32().unwrap(), 1);
            assert_eq!(r.read_i32().unwrap(), to_api_flags(flags));
        }
        assert_eq!(
            (
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ),
            (10, 0, 0)
        );
        assert_eq!(r.remaining(), 0);
        assert!(validate_legacy_identity(-1, &[0]).is_err());
        assert!(validate_legacy_identity(10_123, &[0, 0]).is_err());
        assert!(validate_legacy_identity(10_123, &[]).is_err());
    }

    /// `gids_for_uid` reads user 0's app id state, and a fatal
    /// SystemConfig fails it.
    #[test]
    fn gids_for_uid_of_user_0s_state() {
        use crate::package::system_config::{ConfigPermission, SystemConfig};
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let permissions = PermissionManager::new(AppHost::new(process));
        permissions.owner.lock().unwrap().user0 = Some(Arc::new(AccessUser {
            app_id_permissions: vec![(10_123, vec![(INTERNET.into(), INSTALL_GRANTED)])],
            ..Default::default()
        }));
        let mut config = SystemConfig::default();
        config.permissions = [(
            INTERNET.to_string(),
            ConfigPermission {
                per_user: false,
                gids: vec![3003],
            },
        )]
        .into();
        assert_eq!(permissions.gids_for_uid(10_123, &config).unwrap(), [3003]);
        assert!(
            permissions
                .gids_for_uid(10_124, &config)
                .unwrap()
                .is_empty()
        );
        assert!(
            permissions
                .gids_for_uid(1_010_123, &config)
                .unwrap()
                .is_empty()
        );
        config.permission_error = Some("Duplicate permission definition for p".into());
        assert!(permissions.gids_for_uid(10_123, &config).is_err());
    }

    fn package(
        name: &str,
        app_id: i32,
        edit: impl FnOnce(&mut PackageState, &mut AndroidPackage),
    ) -> PackageState {
        let mut ps = PackageState {
            name: name.into(),
            app_id,
            users: [(
                0,
                PackageUserState {
                    installed: true,
                    ..PackageUserState::default()
                },
            )]
            .into(),
            ..PackageState::default()
        };
        let mut pkg = AndroidPackage {
            package_name: name.into(),
            target_sdk_version: 35,
            ..AndroidPackage::default()
        };
        edit(&mut ps, &mut pkg);
        ps.pkg = Some(Arc::new(pkg));
        ps
    }

    fn component(name: &str, package: &str) -> Component {
        Component {
            name: name.into(),
            package_name: package.into(),
            ..Component::default()
        }
    }

    fn definition(name: &str, package: &str, protection_level: i32) -> Permission {
        Permission {
            name: name.into(),
            package: package.into(),
            protection_level,
            dynamic: None,
        }
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
                pkg::Permission {
                    component: component(INTERNET, "android"),
                    ..Default::default()
                },
            ];
        });
        let instant = package("instant", 10004, |ps, _| {
            ps.users.get_mut(&0).unwrap().instant_app = true
        });
        let shared_a = package("shared.a", 10003, |ps, _| {
            ps.shared_user = Some("shared".into())
        });
        let shared_b = package("shared.b", 10003, |ps, _| {
            ps.shared_user = Some("shared".into())
        });
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
            users: [(
                0,
                User {
                    id: 0,
                    ..User::default()
                },
            )]
            .into(),
            ..State::default()
        };
        let mut state = state;
        state.system.system_permissions = Some(BTreeMap::from([(
            1041,
            BTreeSet::from([INTERNET.to_string()]),
        )]));
        let defined = vec![
            definition(CAMERA, "android", 1 | PROTECTION_FLAG_INSTANT),
            definition(INTERNET, "android", 0),
        ];
        let access = AccessUser {
            app_id_permissions: vec![
                (10001, vec![(CAMERA.into(), RUNTIME_GRANTED | USER_SET)]),
                (
                    10002,
                    vec![(CAMERA.into(), USER_SET | USER_FIXED | POLICY_FIXED)],
                ),
                (10003, vec![(INTERNET.into(), INSTALL_GRANTED)]),
                (
                    10004,
                    vec![
                        (CAMERA.into(), RUNTIME_GRANTED),
                        (INTERNET.into(), INSTALL_GRANTED),
                    ],
                ),
                (10005, vec![(CAMERA.into(), USER_SET)]),
            ],
            ..AccessUser::default()
        };
        (state, defined, access)
    }

    #[test]
    fn appops_reads_the_live_access_owner_without_package_inputs() {
        let persisted = PersistedAccess::new();
        persisted.write(USER0_ACCESS, BOOT_USER);
        let manager = PermissionManager::new(persisted.host.clone());
        assert_eq!(manager.uid_mode(10123, "android:camera").unwrap(), None);
        assert_eq!(manager.package_mode("app", "android:camera").unwrap(), None);
        let before = manager.user0().unwrap();
        let mut after = (*before).clone();
        after.app_id_app_ops = vec![(10123, vec![("android:camera".into(), 1)])];
        after.package_app_ops = vec![("app".into(), vec![("android:camera".into(), 2)])];
        manager.owner.lock().unwrap().user0 = Some(Arc::new(after));
        assert_eq!(manager.uid_mode(10123, "android:camera").unwrap(), Some(1));
        assert_eq!(
            manager.package_mode("app", "android:camera").unwrap(),
            Some(2)
        );
        assert_eq!(manager.uid_mode(10124, "android:camera").unwrap(), None);
        assert_eq!(
            manager.package_mode("other", "android:camera").unwrap(),
            None
        );
        assert!(before.app_id_app_ops.is_empty());
        assert!(before.package_app_ops.is_empty());
    }

    #[test]
    fn permission_groups_follow_the_retained_package_state() {
        let persisted = PersistedAccess::new();
        let manager = PermissionManager::new(persisted.host.clone());
        let (state, _, _) = fixture();
        let before = Arc::new(state);
        let groups = manager.groups(&before);
        assert!(!groups.is_empty());
        assert!(Arc::ptr_eq(&groups, &manager.groups(&before)));
        let mut state = (*before).clone();
        for ps in state.packages.values_mut() {
            if let Some(pkg) = &mut ps.pkg {
                Arc::make_mut(pkg).permission_groups.clear();
            }
        }
        let after = Arc::new(state);
        assert_eq!(before.generation, after.generation);
        assert!(manager.groups(&after).is_empty());
        assert!(!groups.is_empty());
        assert_eq!(*manager.groups(&before), *groups);
    }

    fn with_access<T>(f: impl FnOnce(&Access<'_>) -> T) -> T {
        let (state, defined, user0) = fixture();
        let groups = permission_groups(&state);
        let filter = AppsFilter::default();
        let compat = CompatConfig::default();
        f(&Access {
            state: &state,
            filter: &filter,
            defined: &defined,
            groups: &groups,
            user0: &user0,
            compat: &compat,
        })
    }

    #[test]
    fn granted_permission_refresh_validates_package_and_user() {
        with_access(|access| {
            let granted = |name, id, user| access.granted_permissions(name, id, user);
            assert_eq!(granted("granted", 10001, 0).unwrap(), [CAMERA]);
            assert!(granted("denied", 10002, 0).unwrap().is_empty());
            assert_eq!(granted("instant", 10004, 0).unwrap(), [CAMERA]);
            assert_eq!(granted("shared.a", 10003, 0).unwrap(), [INTERNET]);
            for (name, id) in [("granted", 10002), ("missing", 10001)] {
                let error = granted(name, id, 0).unwrap_err();
                assert_eq!(error.code, EX_ILLEGAL_STATE);
                assert_eq!(error.message, "permission UID owner differs");
            }
            for user in [-1, 10] {
                let error = granted("granted", 10001, user).unwrap_err();
                assert_eq!(error.code, EX_ILLEGAL_STATE);
                assert_eq!(error.message, "permission user owner differs");
            }
            let mut state = access.state.clone();
            let shared = state.packages.get_mut("shared.a").unwrap();
            shared.app_id = -1;
            shared.shared_user_app_id = Some(10003);
            assert_eq!(
                Access {
                    state: &state,
                    ..*access
                }
                .granted_permissions("shared.a", 10003, 0)
                .unwrap(),
                [INTERNET]
            );
            state.packages.get_mut("granted").unwrap().users.clear();
            assert_eq!(
                Access {
                    state: &state,
                    ..*access
                }
                .granted_permissions("granted", 10001, 0)
                .unwrap_err()
                .message,
                "permission user owner differs"
            );
        });
    }

    /// CtsPermissionTestCases' `PermissionManagerTest.testCheckPermission`
    /// and `PackageManagerTest`'s checks: a granted runtime permission,
    /// a denied one, a package and a user that do not exist.
    #[test]
    fn check_permission_follows_the_grant() {
        with_access(|a| {
            assert_eq!(
                a.check_permission(1000, "granted", CAMERA, 0).unwrap(),
                GRANTED
            );
            assert_eq!(
                a.check_permission(1000, "denied", CAMERA, 0).unwrap(),
                DENIED
            );
            assert_eq!(
                a.check_permission(1000, "granted", INTERNET, 0).unwrap(),
                DENIED
            );
            assert_eq!(
                a.check_permission(1000, "no.such.package", CAMERA, 0)
                    .unwrap(),
                DENIED
            );
            assert_eq!(
                a.check_permission(1000, "granted", CAMERA, 10).unwrap(),
                DENIED
            );
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
            assert_eq!(
                a.check_permission(1000, "shared.a", INTERNET, 0).unwrap(),
                GRANTED
            );
            assert_eq!(
                a.check_permission(1000, "shared.b", INTERNET, 0).unwrap(),
                GRANTED
            );
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
            assert_eq!(
                a.check_permission(10004, "instant", CAMERA, 0).unwrap(),
                GRANTED
            );
            assert_eq!(
                a.check_permission(10004, "instant", INTERNET, 0).unwrap(),
                DENIED
            );
            assert_eq!(a.check_uid_permission(10004, INTERNET).unwrap(), DENIED);
            // Definitions are hidden from instant apps.
            assert_eq!(
                a.get_permission_info(10004, CAMERA, "instant", 0).unwrap(),
                None
            );
            assert!(a.all_permission_groups(10004, 0).unwrap().is_empty());
        });
    }

    /// The fuller permission implies the coarser one.
    #[test]
    fn fine_location_implies_coarse() {
        let (state, defined, mut user0) = fixture();
        user0.app_id_permissions[0].1.push((
            "android.permission.ACCESS_FINE_LOCATION".into(),
            RUNTIME_GRANTED,
        ));
        let groups = permission_groups(&state);
        let (filter, compat) = (AppsFilter::default(), CompatConfig::default());
        let a = Access {
            state: &state,
            filter: &filter,
            defined: &defined,
            groups: &groups,
            user0: &user0,
            compat: &compat,
        };
        assert_eq!(
            a.check_uid_permission(10001, "android.permission.ACCESS_COARSE_LOCATION")
                .unwrap(),
            GRANTED
        );
    }

    /// `getPermissionFlags`' API flags, `isPermissionRevokedByPolicy` and
    /// `shouldShowRequestPermissionRationale` of the caller's package.
    #[test]
    fn flags_policy_and_rationale() {
        with_access(|a| {
            assert_eq!(a.permission_flags(1000, "granted", CAMERA, 0).unwrap(), 1);
            assert_eq!(
                a.permission_flags(1000, "denied", CAMERA, 0).unwrap(),
                1 | 2 | 4
            );
            assert_eq!(
                a.permission_flags(1000, "denied", "no.such.permission", 0)
                    .unwrap(),
                0
            );
            assert!(
                a.permission_flags(10001, "granted", CAMERA, 0)
                    .unwrap_err()
                    .message
                    .contains("Neither user 10001")
            );
            assert!(a.revoked_by_policy(1000, "denied", CAMERA, 0).unwrap());
            assert!(!a.revoked_by_policy(1000, "granted", CAMERA, 0).unwrap());
            // Granted, or fixed: no rationale; not the caller's: none.
            assert!(
                !a.should_show_rationale(10001, "granted", CAMERA, 0)
                    .unwrap()
            );
            assert!(!a.should_show_rationale(10002, "denied", CAMERA, 0).unwrap());
            assert!(!a.should_show_rationale(1000, "denied", CAMERA, 0).unwrap());
            // Denied by the user and still requestable: a rationale.
            assert!(a.should_show_rationale(10005, "asked", CAMERA, 0).unwrap());
            assert!(!a.revoked_by_policy(10005, "asked", CAMERA, 0).unwrap());
        });
        assert_eq!(
            to_api_flags(IMPLICIT | LEGACY_GRANTED),
            FLAG_PERMISSION_REVIEW_REQUIRED
        );
        assert_eq!(
            to_api_flags(IMPLICIT | APP_OP_REVOKED),
            FLAG_PERMISSION_REVOKE_WHEN_REQUESTED | 1 << 3
        );
        assert_eq!(
            to_api_flags(SOFT_RESTRICTED),
            FLAG_PERMISSION_APPLY_RESTRICTION
        );
    }

    /// `getAllowlistedRestrictedPermissions`' argument checks and its
    /// system allowlist.
    #[test]
    fn allowlisted_restricted_permissions() {
        let (state, defined, mut user0) = fixture();
        user0.app_id_permissions[0]
            .1
            .push(("android.permission.READ_SMS".into(), SYSTEM_EXEMPT));
        let groups = permission_groups(&state);
        let (filter, compat) = (AppsFilter::default(), CompatConfig::default());
        let a = Access {
            state: &state,
            filter: &filter,
            defined: &defined,
            groups: &groups,
            user0: &user0,
            compat: &compat,
        };
        assert_eq!(
            a.allowlisted(1000, "granted", WHITELIST_SYSTEM, 0).unwrap(),
            Some(vec!["android.permission.READ_SMS".to_string()])
        );
        assert_eq!(
            a.allowlisted(1000, "granted", WHITELIST_UPGRADE, 0)
                .unwrap(),
            Some(vec![])
        );
        assert_eq!(
            a.allowlisted(1000, "no.such.package", WHITELIST_SYSTEM, 0)
                .unwrap(),
            None
        );
        assert_eq!(
            a.allowlisted(1000, "granted", 8, 0).unwrap_err().message,
            "Requested flags 0x8, but only 0x7 are allowed"
        );
        assert!(
            a.allowlisted(10001, "granted", WHITELIST_SYSTEM, 0)
                .unwrap_err()
                .message
                .contains("system allowlist")
        );
    }

    /// `getPermissionInfo` and `queryPermissionsByGroup`: installed, the
    /// group's permissions, and a caller targeting before O sees only the
    /// base protection.
    #[test]
    fn definitions() {
        with_access(|a| {
            let camera = a
                .get_permission_info(1000, CAMERA, "android", 0)
                .unwrap()
                .unwrap();
            assert_eq!(camera.item.name.as_deref(), Some(CAMERA));
            assert_eq!(camera.flags, FLAG_INSTALLED);
            assert_eq!(camera.protection_level, 1 | PROTECTION_FLAG_INSTANT);
            assert_eq!(
                camera.group.as_deref(),
                Some("android.permission-group.CAMERA")
            );
            assert_eq!(
                a.get_permission_info(1000, "no.such.permission", "android", 0)
                    .unwrap(),
                None
            );
            let old = generate_permission_info(camera.clone(), 0, 25);
            assert_eq!(old.protection_level, 1);
            let in_group = a
                .query_permissions_by_group(1000, Some("android.permission-group.CAMERA"), 0)
                .unwrap()
                .unwrap();
            assert_eq!(
                in_group
                    .iter()
                    .map(|p| p.item.name.as_deref().unwrap())
                    .collect::<Vec<_>>(),
                [CAMERA]
            );
            let ungrouped = a
                .query_permissions_by_group(1000, None, 0)
                .unwrap()
                .unwrap();
            assert_eq!(
                ungrouped
                    .iter()
                    .map(|p| p.item.name.as_deref().unwrap())
                    .collect::<Vec<_>>(),
                [INTERNET]
            );
            assert_eq!(
                a.query_permissions_by_group(1000, Some("no.such.group"), 0)
                    .unwrap(),
                None
            );
            let groups = a.all_permission_groups(1000, 0).unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].priority, 500);
            assert_eq!(
                a.permission_group_info(1000, "android.permission-group.CAMERA", 0)
                    .unwrap(),
                Some(groups[0].clone())
            );
        });
    }

    /// `addPermissionGroups`: a system package's declaration replaces an
    /// app's, an app's never replaces another's.
    #[test]
    fn system_packages_take_over_permission_groups() {
        let declaring = |name: &str, app_id: i32, system: bool| {
            package(name, app_id, |ps, pkg| {
                ps.is.system = system;
                pkg.permission_groups = vec![pkg::PermissionGroup {
                    component: component("group", name),
                    ..Default::default()
                }];
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

    /// The fixture with uid 1041 holding the grant permissions but not
    /// ADJUST_RUNTIME_PERMISSIONS_POLICY, and `android` the permission
    /// controller.
    fn with_writable_access<T>(f: impl FnOnce(&Access<'_>, &mut AccessUser) -> T) -> T {
        let (mut state, defined, user0) = fixture();
        state.system.permission_controller_package = Some(Some("android".into()));
        state.system.system_permissions = Some(BTreeMap::from([(
            1041,
            BTreeSet::from([
                GRANT_RUNTIME_PERMISSIONS.to_string(),
                REVOKE_RUNTIME_PERMISSIONS.to_string(),
            ]),
        )]));
        let groups = permission_groups(&state);
        let filter = AppsFilter::default();
        let compat = CompatConfig::default();
        let mut new = user0.clone();
        f(
            &Access {
                state: &state,
                filter: &filter,
                defined: &defined,
                groups: &groups,
                user0: &user0,
                compat: &compat,
            },
            &mut new,
        )
    }

    /// `AppIdPermissionPolicy.resetRuntimePermissions`: the user's choices
    /// and the grant go, a policy-fixed permission stays, a default grant
    /// is given again (a legacy one asks for review), and a permission
    /// another package of the uid requests is left alone.
    #[test]
    fn runtime_permissions_are_reset() {
        let (mut state, defined, mut user0) = fixture();
        for name in ["granted", "denied", "asked", "shared.a", "shared.b"] {
            let ps = state.packages.get_mut(name).unwrap();
            Arc::make_mut(ps.pkg.as_mut().unwrap()).requested_permissions = vec![CAMERA.into()];
        }
        user0.app_id_permissions[4].1[0].1 = PREGRANT | USER_SET | LEGACY_GRANTED;
        user0
            .app_id_permissions
            .push((10003, vec![(CAMERA.into(), RUNTIME_GRANTED | USER_FIXED)]));
        user0.app_id_permissions.sort_by_key(|(id, _)| *id);
        let groups = permission_groups(&state);
        let filter = AppsFilter::default();
        let compat = CompatConfig::default();
        let a = Access {
            state: &state,
            filter: &filter,
            defined: &defined,
            groups: &groups,
            user0: &user0,
            compat: &compat,
        };
        let mut new = user0.clone();
        let reset = |new: &mut AccessUser, package| {
            a.reset_runtime_permissions(new, package, 0)
                .unwrap()
                .into_iter()
                .map(|c| (c.old, c.new))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            reset(&mut new, "granted"),
            [(RUNTIME_GRANTED | USER_SET, 0)]
        );
        assert!(reset(&mut new, "denied").is_empty());
        assert_eq!(
            reset(&mut new, "asked"),
            [(
                PREGRANT | USER_SET | LEGACY_GRANTED,
                PREGRANT | LEGACY_GRANTED | RUNTIME_GRANTED | IMPLICIT
            )]
        );
        assert!(reset(&mut new, "shared.a").is_empty());
        assert!(reset(&mut new, "unknown").is_empty());
    }

    /// CtsPermissionTestCases' `GrantRuntimePermissionTest` and
    /// `RevokeRuntimePermissionTest` shapes: the caller needs the grant or
    /// revoke permission; a runtime permission's grant keeps its other
    /// flags; policy-fixed ones need ADJUST_RUNTIME_PERMISSIONS_POLICY; a
    /// permission that is not runtime cannot change; an unknown package
    /// changes nothing.
    #[test]
    fn runtime_permissions_are_granted_and_revoked() {
        let none = BTreeSet::new();
        with_writable_access(|a, new| {
            let grant = |new: &mut AccessUser, caller, package, permission, granted| {
                a.set_runtime_permission_granted(
                    new, &none, caller, package, permission, 0, granted,
                )
            };
            let err = grant(new, 10_001, "asked", CAMERA, true).unwrap_err();
            assert!(
                err.message.contains("GRANT_RUNTIME_PERMISSIONS"),
                "{}",
                err.message
            );
            let changed = grant(new, 1041, "asked", CAMERA, true).unwrap();
            assert_eq!(
                (changed[0].old, changed[0].new),
                (USER_SET, USER_SET | RUNTIME_GRANTED)
            );
            assert!(grant(new, 1041, "asked", CAMERA, true).unwrap().is_empty());
            // Policy fixed: refused without the policy permission, not for
            // the system.
            assert!(grant(new, 1041, "denied", CAMERA, true).unwrap().is_empty());
            assert_eq!(grant(new, 1000, "denied", CAMERA, true).unwrap().len(), 1);
            let err = grant(new, 1041, "asked", INTERNET, true).unwrap_err();
            assert!(err.message.contains("not a changeable permission type"));
            assert!(
                grant(new, 1041, "missing", CAMERA, true)
                    .unwrap()
                    .is_empty()
            );
            let changed = grant(new, 1041, "granted", CAMERA, false).unwrap();
            assert_eq!(changed[0].new, USER_SET);
            assert_eq!(new.permission_flags(10_001, CAMERA), USER_SET);
            let shell = BTreeSet::from([DISALLOW_DEBUGGING_FEATURES.to_string()]);
            let err = a
                .set_runtime_permission_granted(new, &shell, 2000, "asked", CAMERA, 0, true)
                .unwrap_err();
            assert!(
                err.message.contains("Shell is disallowed"),
                "{}",
                err.message
            );
        });
    }

    /// `updatePermissionFlags`: the API flags of the mask set, the
    /// system's flags only the system's to change, policy flags only with
    /// ADJUST_RUNTIME_PERMISSIONS_POLICY.
    #[test]
    fn permission_flags_are_updated() {
        const FLAG_USER_SET: i32 = 1 << 0;
        const FLAG_USER_FIXED: i32 = 1 << 1;
        const FLAG_SYSTEM_FIXED: i32 = 1 << 4;
        let none = BTreeSet::new();
        with_writable_access(|a, new| {
            let update = |new: &mut AccessUser, caller, mask, values, adjust| {
                a.update_permission_flags(
                    new,
                    &none,
                    caller,
                    "asked",
                    CAMERA,
                    (mask, values),
                    adjust,
                    0,
                )
            };
            let changed = update(
                new,
                1041,
                FLAG_USER_SET | FLAG_USER_FIXED,
                FLAG_USER_FIXED,
                true,
            )
            .unwrap();
            assert_eq!((changed[0].old, changed[0].new), (USER_SET, USER_FIXED));
            // Ignored for an app.
            assert!(
                update(new, 1041, FLAG_SYSTEM_FIXED, FLAG_SYSTEM_FIXED, true)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                update(new, 1000, FLAG_SYSTEM_FIXED, FLAG_SYSTEM_FIXED, true).unwrap()[0].new,
                USER_FIXED | SYSTEM_FIXED
            );
            let err = update(new, 1041, FLAG_PERMISSION_POLICY_FIXED, 0, true).unwrap_err();
            assert!(
                err.message.contains("to change policy flags"),
                "{}",
                err.message
            );
        });
        assert_eq!(
            update_flags(
                FLAG_HARD_RESTRICTED,
                RUNTIME_GRANTED,
                FLAG_PERMISSION_APPLY_RESTRICTION,
                FLAG_PERMISSION_APPLY_RESTRICTION
            ),
            RUNTIME_GRANTED | RESTRICTION_REVOKED
        );
    }

    /// The parcels as `PermissionInfo`, `PermissionGroupInfo` and
    /// `SplitPermissionInfoParcelable` write them.
    #[test]
    fn parcels() {
        let group = with_access(|a| a.all_permission_groups(1000, 0).unwrap().remove(0));
        let mut p = Parcel::new();
        group.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(
            r.read_string8().unwrap().as_deref(),
            Some("android.permission-group.CAMERA")
        );
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("android"));
        // labelRes, the label (a plain char sequence, null), icon, logo,
        // no meta-data, banner, showUserIcon (USER_NULL), isArchived.
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(
            [
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ],
            [0, 0, -1]
        );
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, -10000]);
        assert!(!r.read_bool().unwrap());
        // descriptionRes, requestRes, the request details, the description,
        // flags, priority.
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 7]);
        assert_eq!(
            [
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ],
            [0, 0, 0]
        );
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 500]);
        assert_eq!(r.remaining(), 0);

        let camera = with_access(|a| {
            a.get_permission_info(1000, CAMERA, "android", 0)
                .unwrap()
                .unwrap()
        });
        let mut p = Parcel::new();
        camera.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string8().unwrap().as_deref(), Some(CAMERA));
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("android"));
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 1]);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(
            [
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ],
            [0, 0, -1]
        );
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, -10000]);
        assert!(!r.read_bool().unwrap());
        // protectionLevel, flags, group, backgroundPermission,
        // descriptionRes, requestRes, the description, no known certs.
        assert_eq!(
            [r.read_i32().unwrap(), r.read_i32().unwrap()],
            [1 | PROTECTION_FLAG_INSTANT, FLAG_INSTALLED]
        );
        assert_eq!(
            r.read_string8().unwrap().as_deref(),
            Some("android.permission-group.CAMERA")
        );
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(
            [
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ],
            [0, 0, 1]
        );
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
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.permission.ACCESS_FINE_LOCATION")
        );
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some(ACCESS_BACKGROUND_LOCATION)
        );
        assert_eq!(r.read_i32().unwrap(), 29);
        assert_eq!(r.remaining(), 0);
    }
}
