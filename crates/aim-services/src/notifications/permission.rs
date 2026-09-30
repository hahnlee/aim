//! POST_NOTIFICATIONS as the Mac decides it (#470): an app's request for
//! the permission is answered by its shim's authorization prompt, and the
//! Mac's per-app setting is mirrored afterwards (allowed → granted, denied
//! → revoked). The permission is set as a device's settings do
//! (`PermissionHelper.setNotificationPermission` with `userSet`): granted
//! or revoked through `IPermissionManager`, with the user-set flag and
//! without the fixed, default-grant and review flags.

use aim_service_aidl::android_permission_ipermissionmanager as pm;

use super::Bridge;

const POST_NOTIFICATIONS: &str = "android.permission.POST_NOTIFICATIONS";
/// `VirtualDeviceManager.PERSISTENT_DEVICE_ID_DEFAULT`.
const PERSISTENT_DEVICE_ID_DEFAULT: &str = "default:0";
/// `PackageManager.PERMISSION_GRANTED`.
const PERMISSION_GRANTED: i32 = 0;
/// `PackageManager.FLAG_PERMISSION_*`.
const FLAG_USER_SET: i32 = 1 << 0;
const FLAG_USER_FIXED: i32 = 1 << 1;
const FLAG_GRANTED_BY_DEFAULT: i32 = 1 << 5;
const FLAG_REVIEW_REQUIRED: i32 = 1 << 6;
/// The flags a user's decision sets (`FLAG_USER_SET`) and clears.
const FLAG_MASK: i32 =
    FLAG_USER_SET | FLAG_USER_FIXED | FLAG_GRANTED_BY_DEFAULT | FLAG_REVIEW_REQUIRED;
/// `revokeRuntimePermission`'s reason, logged by the platform.
const REASON: &str = "the Mac's notification setting";

/// What a user's decision changes of the grant.
#[derive(Debug, PartialEq, Eq)]
enum Change {
    Grant,
    Revoke,
}

fn change(granted: bool, allow: bool) -> Option<Change> {
    match (granted, allow) {
        (false, true) => Some(Change::Grant),
        (true, false) => Some(Change::Revoke),
        _ => None,
    }
}

impl Bridge {
    /// Whether `package` of `user` holds POST_NOTIFICATIONS.
    pub(super) fn notifications_granted(&self, package: &str, user: i32) -> Result<bool, String> {
        let args = pm::CheckPermission {
            package_name: Some(package.into()),
            permission_name: Some(POST_NOTIFICATIONS.into()),
            persistent_device_id: Some(PERSISTENT_DEVICE_ID_DEFAULT.into()),
            user_id: user,
        };
        self.call(
            "permissionmgr",
            pm::CHECK_PERMISSION,
            |p| args.write(p),
            pm::read_check_permission_reply,
        )
        .map(|r| r == PERMISSION_GRANTED)
    }

    /// Grants or revokes POST_NOTIFICATIONS of `package` for `user` as the
    /// user decided on the Mac.
    pub(super) fn set_notifications(
        &self,
        package: &str,
        user: i32,
        allow: bool,
    ) -> Result<(), String> {
        match change(self.notifications_granted(package, user)?, allow) {
            Some(Change::Grant) => self.call(
                "permissionmgr",
                pm::GRANT_RUNTIME_PERMISSION,
                |p| {
                    pm::GrantRuntimePermission {
                        package_name: Some(package.into()),
                        permission_name: Some(POST_NOTIFICATIONS.into()),
                        persistent_device_id: Some(PERSISTENT_DEVICE_ID_DEFAULT.into()),
                        user_id: user,
                    }
                    .write(p)
                },
                pm::read_grant_runtime_permission_reply,
            )?,
            Some(Change::Revoke) => self.call(
                "permissionmgr",
                pm::REVOKE_RUNTIME_PERMISSION,
                |p| {
                    pm::RevokeRuntimePermission {
                        package_name: Some(package.into()),
                        permission_name: Some(POST_NOTIFICATIONS.into()),
                        persistent_device_id: Some(PERSISTENT_DEVICE_ID_DEFAULT.into()),
                        user_id: user,
                        reason: Some(REASON.into()),
                    }
                    .write(p)
                },
                pm::read_revoke_runtime_permission_reply,
            )?,
            None => {}
        }
        self.call(
            "permissionmgr",
            pm::UPDATE_PERMISSION_FLAGS,
            |p| {
                pm::UpdatePermissionFlags {
                    package_name: Some(package.into()),
                    permission_name: Some(POST_NOTIFICATIONS.into()),
                    flag_mask: FLAG_MASK,
                    flag_values: FLAG_USER_SET,
                    check_adjust_policy_flag_permission: true,
                    persistent_device_id: Some(PERSISTENT_DEVICE_ID_DEFAULT.into()),
                    user_id: user,
                }
                .write(p)
            },
            pm::read_update_permission_flags_reply,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_the_grant_only_where_it_differs() {
        assert_eq!(change(false, true), Some(Change::Grant));
        assert_eq!(change(true, false), Some(Change::Revoke));
        assert_eq!(change(true, true), None);
        assert_eq!(change(false, false), None);
    }
}
