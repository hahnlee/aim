//! `user`: UserManager for the app host, which runs user 0 alone,
//! unlocked and visible, with the restrictions UserManagerService last
//! persisted for it (`/data/system/users/0.xml`'s `<restrictions>`).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_iusermanager as um;

use super::{AppHost, reply};

/// The only user.
const SYSTEM_USER: i32 = 0;
/// `UserInfo.FLAG_DEMO`, `FLAG_FULL`.
const FLAG_DEMO: i32 = 0x200;
const FLAG_FULL: i32 = 0x400;

/// Who may read a `UserProperties` field in a copy
/// (`UserProperties(orig, exposeAllFields, hasManage, hasQueryOrManage)`).
#[derive(Clone, Copy, PartialEq)]
enum Visible {
    Anyone,
    Manage,
    System,
}

/// `UserProperties`' fields in parcel order: the persisted attribute, the
/// index of its presence bit, who may read it in a copy, and the full
/// system user type's value (`UserProperties.Builder`'s defaults, as
/// `UserTypeFactory.getDefaultTypeFullSystem` sets none). Booleans are 0 or 1.
const USER_PROPERTIES: [(&str, u32, Visible, i32); 19] = [
    ("showInLauncher", 0, Visible::Anyone, 0),
    ("startWithParent", 1, Visible::System, 0),
    ("showInSettings", 2, Visible::Manage, 0),
    ("showInQuietMode", 12, Visible::Anyone, 0),
    // SHOW_IN_SHARING_SURFACES_SEPARATE.
    ("showInSharingSurfaces", 14, Visible::Anyone, 1),
    ("inheritDevicePolicy", 3, Visible::System, 0),
    ("useParentsContacts", 4, Visible::Manage, 0),
    ("updateCrossProfileIntentFiltersOnOTA", 5, Visible::System, 0),
    ("crossProfileIntentFilterAccessControl", 6, Visible::System, 0),
    ("crossProfileIntentResolutionStrategy", 7, Visible::System, 0),
    ("mediaSharedWithParent", 8, Visible::Anyone, 0),
    ("credentialShareableWithParent", 9, Visible::Anyone, 0),
    ("authAlwaysRequiredToDisableQuietMode", 13, Visible::Manage, 0),
    ("allowStoppingUserWithDelayedLocking", 16, Visible::System, 0),
    ("deleteAppWithParent", 10, Visible::System, 0),
    ("alwaysVisible", 11, Visible::System, 0),
    ("crossProfileContentSharingStrategy", 15, Visible::Anyone, 0),
    ("profileApiVisibility", 17, Visible::Anyone, 0),
    ("itemsRestrictedOnHomeScreen", 18, Visible::Anyone, 0),
];

/// A copy of user 0's `UserProperties`: the fields present, and every
/// field's value (0 when not copied), written as `writeToParcel` writes
/// them.
struct UserProperties {
    present: i64,
    values: [i32; 19],
}

impl UserProperties {
    /// User 0's properties (its type's, then those `users/0.xml`'s
    /// `<userProperties>` holds) as copied for a caller that is the system
    /// (`expose_all`) or holds MANAGE_USERS (`manage`).
    fn copy(persisted: &[(String, i32)], expose_all: bool, manage: bool) -> UserProperties {
        let mut copy = UserProperties { present: 0, values: [0; 19] };
        for (i, (name, index, visible, default)) in USER_PROPERTIES.iter().enumerate() {
            let shown = match visible {
                Visible::Anyone => true,
                Visible::Manage => manage,
                Visible::System => expose_all,
            };
            if shown {
                copy.present |= 1 << index;
                copy.values[i] = persisted.iter().find(|(n, _)| n == name).map_or(*default, |(_, v)| *v);
            }
        }
        copy
    }
}

impl WriteParcelable for UserProperties {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.present);
        for v in self.values {
            p.write_i32(v);
        }
    }
}

/// The profiles of `user`'s profile group: user 0 alone, and none of a
/// user that does not exist.
pub(super) fn profile_ids(user: i32) -> Vec<i32> {
    if user == SYSTEM_USER { vec![SYSTEM_USER] } else { Vec::new() }
}

pub struct UserManager {
    host: Arc<AppHost>,
}

impl UserManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        UserManager { host }
    }

    /// `checkManageOrInteractPermissionIfCallerInOtherProfileGroup`, or with
    /// `query` its `checkQueryOrInteract…` form: another user than the
    /// caller's (user 0 has no profiles) takes root or system, MANAGE_USERS
    /// (or QUERY_USERS), or INTERACT_ACROSS_USERS.
    fn check_other_user(&self, call: &Call<'_>, user: i32, query: bool, name: &str) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        if user == uid / 100_000 || matches!(uid % 100_000, 0 | 1000) {
            return Ok(());
        }
        let held = |p: &str| self.host.check_permission(Some(p), uid).map(|r| r == 0);
        if held("android.permission.MANAGE_USERS")?
            || query && held("android.permission.QUERY_USERS")?
            || held("android.permission.INTERACT_ACROSS_USERS")?
        {
            return Ok(());
        }
        Err(Exception::security(if query {
            format!("You need INTERACT_ACROSS_USERS, MANAGE_USERS, or QUERY_USERS permission to: check {name}")
        } else {
            format!("You need INTERACT_ACROSS_USERS or MANAGE_USERS permission to: check {name}")
        }))
    }
}

impl Service for UserManager {
    fn descriptor(&self) -> &str {
        um::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            um::IS_USER_UNLOCKING_OR_UNLOCKED => {
                let args = um::IsUserUnlockingOrUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, false, "isUserUnlockingOrUnlocked") {
                    return reply(Err(e));
                }
                um::write_is_user_unlocking_or_unlocked_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::IS_USER_RUNNING => {
                let args = um::IsUserRunning::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, false, "isUserRunning") {
                    return reply(Err(e));
                }
                um::write_is_user_running_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::IS_USER_VISIBLE => {
                let args = um::IsUserVisible::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_is_user_visible_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::GET_PROFILE_IDS => {
                let args = um::GetProfileIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_get_profile_ids_reply(&mut p, &Some(profile_ids(args.user_id)));
            }
            um::GET_CREDENTIAL_OWNER_PROFILE => {
                let args = um::GetCredentialOwnerProfile::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_get_credential_owner_profile_reply(&mut p, args.user_id);
            }
            // User 0 is a full user, no profile: "" (a user that does not
            // exist has none).
            um::GET_PROFILE_TYPE => {
                let args = um::GetProfileType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, true, "getProfileType") {
                    return reply(Err(e));
                }
                um::write_get_profile_type_reply(&mut p, &(args.user_id == SYSTEM_USER).then(String::new));
            }
            um::GET_USER_PROPERTIES_COPY => {
                let args = um::GetUserPropertiesCopy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, true, "getUserProperties") {
                    return reply(Err(e));
                }
                if args.user_id != SYSTEM_USER {
                    return reply(Err(Exception::illegal_argument(format!("Cannot access properties for user {}", args.user_id))));
                }
                let uid = call.sender_euid as i32;
                let manage = matches!(uid % 100_000, 1000) || uid == 0
                    || matches!(self.host.check_permission(Some("android.permission.MANAGE_USERS"), uid), Ok(0));
                match self.host.user_properties() {
                    Ok(persisted) => um::write_get_user_properties_copy_reply(&mut p, Some(&UserProperties::copy(&persisted, uid == 1000, manage))),
                    Err(e) => return reply(Err(e)),
                }
            }
            // User 0 is no profile, so never in quiet mode.
            um::IS_QUIET_MODE_ENABLED => {
                um::IsQuietModeEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_is_quiet_mode_enabled_reply(&mut p, false);
            }
            // A restriction of the base set `users/0.xml` holds; a key no
            // valid restriction has is in none.
            um::HAS_BASE_USER_RESTRICTION => {
                let args = um::HasBaseUserRestriction::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                let held = |perm: &str| matches!(self.host.check_permission(Some(perm), uid), Ok(0));
                if !matches!(uid % 100_000, 0 | 1000) && !held("android.permission.MANAGE_USERS") && !held("android.permission.CREATE_USERS") {
                    return reply(Err(Exception::security("You either need MANAGE_USERS or CREATE_USERS permission to: hasBaseUserRestriction")));
                }
                let restricted = match self.host.user_restrictions() {
                    Ok(r) => args.user_id == SYSTEM_USER && args.restriction_key.is_some_and(|k| r.contains(&k)),
                    Err(e) => return reply(Err(e)),
                };
                um::write_has_base_user_restriction_reply(&mut p, restricted);
            }
            um::IS_USER_UNLOCKED => {
                let args = um::IsUserUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, false, "isUserUnlocked") {
                    return reply(Err(e));
                }
                um::write_is_user_unlocked_reply(&mut p, args.user_id == SYSTEM_USER);
            }
            um::HAS_USER_RESTRICTION => {
                let args = um::HasUserRestriction::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let restricted = match self.host.user_restrictions() {
                    Ok(r) => args.user_id == SYSTEM_USER && args.restriction_key.is_some_and(|k| r.contains(&k)),
                    Err(e) => return reply(Err(e)),
                };
                um::write_has_user_restriction_reply(&mut p, restricted);
            }
            // The user of a serial number, -1 for none.
            um::GET_USER_HANDLE => {
                let args = um::GetUserHandle::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.user_serial() {
                    Ok(serial) => um::write_get_user_handle_reply(&mut p, if serial == args.user_serial_number { SYSTEM_USER } else { -1 }),
                    Err(e) => return reply(Err(e)),
                }
            }
            um::IS_DEMO_USER => {
                let args = um::IsDemoUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                if args.user_id != uid / 100_000
                    && !matches!(uid % 100_000, 0 | 1000)
                    && self.host.check_permission(Some("android.permission.MANAGE_USERS"), uid).map_err(|_| BAD_VALUE)? != 0
                {
                    return reply(Err(Exception::security(format!(
                        "You need MANAGE_USERS permission to query if u={} is a demo user",
                        args.user_id
                    ))));
                }
                let demo = self.host.property("ro.boot.arc_demo_mode").is_some_and(|v| matches!(v.as_str(), "1" | "true" | "y" | "yes" | "on"))
                    || args.user_id == SYSTEM_USER && matches!(self.host.user_flags(), Ok(f) if f & FLAG_DEMO != 0);
                um::write_is_demo_user_reply(&mut p, demo);
            }
            // The system user is not full.
            um::IS_HEADLESS_SYSTEM_USER_MODE => {
                um::IsHeadlessSystemUserMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.user_flags() {
                    Ok(flags) => um::write_is_headless_system_user_mode_reply(&mut p, flags & FLAG_FULL == 0),
                    Err(e) => return reply(Err(e)),
                }
            }
            // `readApplicationRestrictionsLAr`: none without the file.
            um::GET_APPLICATION_RESTRICTIONS_FOR_USER => {
                let args = um::GetApplicationRestrictionsForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = args.package_name.unwrap_or_default();
                let uid = call.sender_euid as i32;
                let same_app = self.host.owns_package(uid, &package).unwrap_or(false);
                if (uid / 100_000 != args.user_id || !same_app) && !matches!(uid % 100_000, 0 | 1000) {
                    return reply(Err(Exception::security(format!(
                        "get application restrictions for other user/app {package} is only allowed for the system"
                    ))));
                }
                let file = self.host.data().map(|d| d.join(format!("system/users/{}/res_{package}.xml", args.user_id)));
                match file {
                    Ok(f) if f.exists() => {
                        return reply(Err(self.host.unimplemented_named("user", "an app's persisted restrictions (#1519)")));
                    }
                    Ok(_) => um::write_get_application_restrictions_for_user_reply(&mut p, Some(&super::bind::Bundle(Vec::new()))),
                    Err(e) => return reply(Err(e)),
                }
            }
            code => return reply(Err(self.host.unimplemented("user", code))),
        }
        Ok(p)
    }
}
