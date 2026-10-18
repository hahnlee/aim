//! `user`: UserManager for the app host, which runs user 0 alone,
//! unlocked and visible, with the restrictions UserManagerService last
//! persisted for it (`/data/system/users/0.xml`'s `<restrictions>`).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_os_iusermanager as um;

use super::{AppHost, reply};

/// The only user.
const SYSTEM_USER: i32 = 0;
/// `UserInfo.FLAG_DEMO`, `FLAG_FULL`.
const FLAG_DEMO: i32 = 0x200;
const FLAG_FULL: i32 = 0x400;

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
                let ids = if args.user_id == SYSTEM_USER { vec![SYSTEM_USER] } else { Vec::new() };
                um::write_get_profile_ids_reply(&mut p, &Some(ids));
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
