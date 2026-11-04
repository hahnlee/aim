//! PermissionService's restricted-permission allowlist changes
//! (`addAllowlistedRestrictedPermission`,
//! `removeAllowlistedRestrictedPermission`) and AppIdPermissionPolicy's
//! `updatePermissionExemptFlags`, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::existing::exemption_flags;
use super::*;

/// `PermissionInfo.FLAG_IMMUTABLY_RESTRICTED`.
const FLAG_IMMUTABLY_RESTRICTED: i32 = 1 << 4;

/// The exemption flags of `PackageManager.FLAG_PERMISSION_WHITELIST_*`
/// `allowlist`.
pub(super) fn exempt_mask(allowlist: i32) -> i32 {
    [
        (WHITELIST_SYSTEM, SYSTEM_EXEMPT),
        (WHITELIST_UPGRADE, UPGRADE_EXEMPT),
        (WHITELIST_INSTALLER, INSTALLER_EXEMPT),
    ]
    .into_iter()
    .filter(|(a, _)| allowlist & a != 0)
    .fold(0, |mask, (_, exempt)| mask | exempt)
}

impl Access<'_> {
    /// `PackageManagerInternal.isCallerInstallerOfRecord`.
    pub(super) fn installer_of_record(&self, ps: &PackageState, caller: i32) -> bool {
        ps.install_source
            .installer
            .as_deref()
            .and_then(|name| self.state.packages.get(name))
            .is_some_and(|i| i.app_id == apps_filter::app_id(caller))
    }

    /// `getAppIdTargetSdkVersion`: the lowest target of the app id's
    /// packages that request `permission`.
    pub(super) fn app_id_target_sdk(&self, app_id: i32, permission: &str) -> i32 {
        self.packages_of(app_id)
            .iter()
            .filter_map(|ps| ps.pkg.as_deref())
            .filter(|pkg| pkg.requested_permissions.iter().any(|r| r == permission))
            .map(|pkg| pkg.target_sdk_version)
            .min()
            .unwrap_or(CUR_DEVELOPMENT)
    }

    /// The `PermissionInfo` flags of a hard or soft restricted definition.
    fn restriction(&self, p: &Permission) -> Option<i32> {
        self.permission_info(p)
            .map(|i| i.flags)
            .filter(|f| f & (FLAG_HARD_RESTRICTED | FLAG_SOFT_RESTRICTED) != 0)
    }

    /// `enforceRestrictedPermission`: whether the permission's allowlists
    /// may change, an immutably restricted one only for holders of
    /// WHITELIST_RESTRICTED_PERMISSIONS.
    fn enforce_restricted_permission(
        &self,
        caller: i32,
        permission: &str,
    ) -> Result<bool, Exception> {
        let Some(p) = self.definition(permission) else {
            eprintln!(
                "app host: permissionmgr: permission definition for {permission} does not exist"
            );
            return Ok(false);
        };
        if self
            .package(&p.package, caller, apps_filter::user_id(caller))?
            .is_none()
        {
            return Ok(false);
        }
        let immutable = self
            .restriction(p)
            .is_some_and(|f| f & FLAG_IMMUTABLY_RESTRICTED != 0);
        if immutable && !self.holds(caller, WHITELIST_RESTRICTED_PERMISSIONS)? {
            return Err(Exception::security(format!(
                "Cannot modify allowlist of an immutably restricted permission: {permission}"
            )));
        }
        Ok(true)
    }

    /// `addAllowlistedRestrictedPermission` (`add`) or
    /// `removeAllowlistedRestrictedPermission` in `new`: whether the
    /// allowlist `allowlist` of `package` was set anew, and the changes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn change_allowlist(
        &self,
        new: &mut AccessUser,
        caller: i32,
        package: &str,
        permission: &str,
        allowlist: i32,
        user: i32,
        add: bool,
    ) -> Result<(bool, Vec<FlagsChanged>), Exception> {
        if !self.enforce_restricted_permission(caller, permission)? {
            return Ok((false, Vec::new()));
        }
        let mut names = match self.allowlisted(caller, package, allowlist, user)? {
            Some(names) => names,
            None if add => Vec::new(),
            None => return Ok((false, Vec::new())),
        };
        if names.iter().any(|n| n == permission) == add {
            return Ok((false, Vec::new()));
        }
        if add {
            names.push(permission.to_owned());
        } else {
            names.retain(|n| n != permission);
        }
        self.set_allowlist(new, caller, package, &names, allowlist, user, add)
    }

    /// `setAllowlistedRestrictedPermissions`: the upgrade allowlist only
    /// for the installer of record or holders of
    /// WHITELIST_RESTRICTED_PERMISSIONS, and added to only by the latter;
    /// then each restricted permission the package requests exempt by
    /// `allowlist` if it is in `names`, else not.
    #[allow(clippy::too_many_arguments)]
    fn set_allowlist(
        &self,
        new: &mut AccessUser,
        caller: i32,
        package: &str,
        names: &[String],
        allowlist: i32,
        user: i32,
        adding: bool,
    ) -> Result<(bool, Vec<FlagsChanged>), Exception> {
        // `Preconditions.checkArgument`, without a message.
        if allowlist.count_ones() != 1 {
            return Err(Exception::illegal_argument(""));
        }
        let privileged = self.holds(caller, WHITELIST_RESTRICTED_PERMISSIONS)?;
        let Some((ps, pkg)) = self
            .package_by_name(package, caller, user)?
            .and_then(|ps| Some((ps, ps.pkg.as_deref()?)))
        else {
            return Ok((false, Vec::new()));
        };
        if allowlist & WHITELIST_UPGRADE != 0 {
            if !privileged && !self.installer_of_record(ps, caller) {
                return Err(Exception::security(format!(
                    "Modifying upgrade allowlist requires being installer on record or {WHITELIST_RESTRICTED_PERMISSIONS}"
                )));
            }
            if adding && !privileged {
                return Err(Exception::security(format!(
                    "Adding to upgrade allowlist requires{WHITELIST_RESTRICTED_PERMISSIONS}"
                )));
            }
        }
        let mask = exempt_mask(allowlist);
        let mut changes = Vec::new();
        for name in &pkg.requested_permissions {
            let Some(info) = self.definition(name).and_then(|p| self.restriction(p)) else {
                continue;
            };
            let values = if names.contains(name) { mask } else { 0 };
            let old = new.permission_flags(ps.app_id, name);
            let flags = (old & !mask) | values;
            if old == flags {
                continue;
            }
            let target = self.app_id_target_sdk(ps.app_id, name);
            changes.extend(set_flags(
                new,
                ps.app_id,
                name,
                exemption_flags(name, info, target, old, flags),
            ));
        }
        Ok((true, changes))
    }
}
