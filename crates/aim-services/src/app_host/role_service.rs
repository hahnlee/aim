//! `role`: RoleService for the app host (the Permission module at
//! `android-16.0.0_r1`): the roles and their holders that RoleUserState
//! last persisted (`RolesPersistenceImpl`,
//! `/data/misc_de/<user>/apexdata/com.android.permission/roles.xml`), as its
//! queries answer them, and the listeners for their changes. Nothing
//! changes a role's holders in the app host: the role controller
//! (PermissionController) is not run and the writes are not implemented
//! (#1657), so no listener is notified.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_android_xml::Element;
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_app_role_irolemanager as rm;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::info::array_order;

/// `UserHandle.USER_ALL`, `USER_SYSTEM`.
const USER_ALL: i32 = -1;
const USER_SYSTEM: i32 = 0;
/// `RoleManager.ROLE_BROWSER`, `ROLE_SMS`, `ROLE_EMERGENCY`.
const ROLE_BROWSER: &str = "android.app.role.BROWSER";
const ROLE_SMS: &str = "android.app.role.SMS";
const ROLE_EMERGENCY: &str = "android.app.role.EMERGENCY";
/// `RoleService.DEFAULT_APPLICATION_ROLES`.
const DEFAULT_APPLICATION_ROLES: [&str; 5] = [
    "android.app.role.ASSISTANT",
    ROLE_BROWSER,
    "android.app.role.CALL_REDIRECTION",
    "android.app.role.CALL_SCREENING",
    "android.app.role.DIALER",
];

/// A user's `RolesState`: each role (in its `ArrayMap` order) with its
/// holders (in their `ArraySet` order), and the roles whose fallback is
/// enabled.
#[derive(Debug, Default, PartialEq)]
struct Roles {
    roles: Vec<(String, Vec<String>)>,
    fallback_enabled: Vec<String>,
}

impl Roles {
    /// `RolesPersistenceImpl.parseXml` of a document: its `<roles>`.
    fn parse(root: &Element) -> Result<Roles, String> {
        let roles = if root.name == "roles" {
            root
        } else {
            root.children().find(|e| e.name == "roles").ok_or("Missing <roles> in roles.xml")?
        };
        roles
            .string("version")
            .and_then(|v| v.parse::<i32>().ok())
            .ok_or("roles.xml: no version")?;
        let mut state = Roles::default();
        for role in roles.children().filter(|e| e.name == "role") {
            let name = role.string("name").map(|s| s.into_owned()).unwrap_or_default();
            if role.string("fallbackEnabled").is_some_and(|f| f.eq_ignore_ascii_case("true")) {
                state.fallback_enabled.push(name.clone());
            }
            let mut holders = Vec::new();
            for holder in role.children().filter(|e| e.name == "holder") {
                let holder = holder.string("name").map(|s| s.into_owned()).unwrap_or_default();
                if !holders.contains(&holder) {
                    holders.push(holder);
                }
            }
            state.roles.retain(|(n, _)| *n != name);
            state.roles.push((name, array_order(holders, |h| h.as_str())));
        }
        state.roles = array_order(state.roles, |(n, _)| n.as_str());
        Ok(state)
    }

    /// `getRoleHolders`: none for a role that is not available.
    fn holders(&self, role: &str) -> Option<&[String]> {
        self.roles.iter().find(|(n, _)| n == role).map(|(_, h)| h.as_slice())
    }
}

pub struct RoleManager {
    host: Arc<AppHost>,
    /// The roles of user 0, once read.
    roles: Mutex<Option<Arc<Roles>>>,
    /// `mListeners`: by handle, the binder and its death link, and the
    /// user (or `USER_ALL`) they observe.
    listeners: Mutex<HashMap<u32, (Strong, u64, i32)>>,
}

impl RoleManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        RoleManager { host, roles: Mutex::new(None), listeners: Mutex::new(HashMap::new()) }
    }

    /// The holders of `role` of user 0 (`RoleManager.getRoleHoldersAsUser`
    /// as the system asks it): none for a role that is not available.
    pub(super) fn holders_of(&self, role: &str) -> Result<Vec<String>, Exception> {
        Ok(self.roles()?.holders(role).map(<[String]>::to_vec).unwrap_or_default())
    }

    /// `getOrCreateUserState(0)`: the persisted state as `readForUser`
    /// reads it (its reserve copy if it cannot), none without the file.
    fn roles(&self) -> Result<Arc<Roles>, Exception> {
        let mut roles = self.roles.lock().unwrap();
        if let Some(r) = roles.as_ref() {
            return Ok(r.clone());
        }
        let dir = self.host.data()?.join("misc_de/0/apexdata/com.android.permission");
        let file = dir.join("roles.xml");
        let read = |path: &std::path::Path| -> Result<Option<Roles>, String> {
            match super::device_policy::read_atomic_bytes(path)? {
                Some(bytes) => aim_android_xml::read(&bytes).and_then(|r| Roles::parse(&r)).map(Some),
                None => Ok(None),
            }
        };
        let state = match read(&file) {
            Ok(state) => state.unwrap_or_default(),
            Err(e) => {
                eprintln!("app host: Reading from reserve copy of roles.xml: {e}");
                read(&dir.join("roles.xml.reservecopy"))
                    .map_err(|_| Exception::new(EX_ILLEGAL_STATE, format!("Failed to read roles.xml: {}: {e}", file.display())))?
                    .unwrap_or_default()
            }
        };
        let state = Arc::new(state);
        *roles = Some(state.clone());
        Ok(state)
    }

    /// `enforceCallingOrSelfPermission(permission, message)`.
    fn enforce(&self, uid: i32, permission: &str, message: &str) -> Result<(), Exception> {
        self.enforce_with(uid, permission, Some(message))
    }

    fn enforce_with(&self, uid: i32, permission: &str, message: Option<&str>) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? == 0 {
            return Ok(());
        }
        let prefix = message.map(|m| format!("{m}: ")).unwrap_or_default();
        Err(Exception::security(format!("{prefix}Neither user {uid} nor current process has {permission}.")))
    }

    /// `UserUtils.enforceCrossUserPermission` (no profile group): another
    /// user than the caller's takes INTERACT_ACROSS_USERS_FULL. The shell's
    /// restriction applies to users that exist, of which only the caller's
    /// is.
    fn cross_user(&self, uid: i32, user: i32, allow_all: bool, message: &str) -> Result<(), Exception> {
        if user == uid / PER_USER_RANGE as i32 {
            return Ok(());
        }
        if !(user >= USER_SYSTEM || allow_all && user == USER_ALL) {
            return Err(Exception::illegal_argument(format!("Invalid user {user}")));
        }
        self.enforce(uid, "android.permission.INTERACT_ACROSS_USERS_FULL", message)
    }

    /// `UserUtils.isUserExistent`: user 0 alone.
    fn exists(user: i32) -> bool {
        let exists = user == USER_SYSTEM;
        if !exists {
            eprintln!("app host: user {user} does not exist");
        }
        exists
    }

    /// `Preconditions.checkStringNotEmpty`.
    fn not_empty(value: Option<String>, message: &str) -> Result<String, Exception> {
        value.filter(|v| !v.is_empty()).ok_or_else(|| Exception::illegal_argument(message))
    }

    /// `getRoleHoldersAsUser`.
    fn role_holders(&self, uid: i32, role: Option<String>, user: i32) -> Result<Vec<String>, Exception> {
        self.cross_user(uid, user, false, "getRoleHoldersAsUser")?;
        if !Self::exists(user) {
            return Ok(Vec::new());
        }
        self.enforce(uid, "android.permission.MANAGE_ROLE_HOLDERS", "getRoleHoldersAsUser")?;
        let role = Self::not_empty(role, "roleName cannot be null or empty")?;
        Ok(self.roles()?.holders(&role).map(<[String]>::to_vec).unwrap_or_default())
    }

    /// `PackageUtils.canCallingOrSelfPackageQuery`: `getPackageInfo` of the
    /// package as the caller.
    fn can_query(&self, uid: i32, package: &str, user: i32) -> Result<bool, Exception> {
        let (state, resolution) = self.host.packages()?;
        let filtered = crate::package::apps_filter::should_filter_application(
            &state,
            &resolution.apps_filter,
            state.packages.get(package),
            uid,
            user,
            true,
            true,
        )
        .map_err(|e| Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION, format!("role: {}", e.0)))?;
        Ok(!filtered && state.packages.get(package).is_some_and(|ps| ps.users.get(&user).is_none_or(|u| u.installed)))
    }

    /// The first holder of `role` the caller may see, for
    /// `getSmsRoleHolder` and `getEmergencyRoleHolder`.
    fn queryable_holder(&self, uid: i32, role: &str, user: i32) -> Result<Option<String>, Exception> {
        let holder = self.roles()?.holders(role).and_then(|h| h.first().cloned());
        match holder {
            Some(h) if self.can_query(uid, &h, user)? => Ok(Some(h)),
            _ => Ok(None),
        }
    }

    fn add_listener(&self, listener: u32, user: i32) {
        let strong = self.host.process.strong(listener);
        let me = Arc::downgrade(&self.host);
        let cookie = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(rm) = me.upgrade().and_then(|h| h.role_service.get().cloned()) {
                    rm.remove_listener(listener);
                }
            }),
        );
        if let Some((strong, cookie, _)) = self.listeners.lock().unwrap().insert(listener, (strong, cookie, user)) {
            self.host.process.clear_death(&strong, cookie);
        }
    }

    fn remove_listener(&self, listener: u32) {
        if let Some((strong, cookie, _)) = self.listeners.lock().unwrap().remove(&listener) {
            self.host.process.clear_death(&strong, cookie);
        }
    }
}

impl Service for RoleManager {
    fn descriptor(&self) -> &str {
        rm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            rm::IS_ROLE_AVAILABLE_AS_USER => {
                let a = rm::IsRoleAvailableAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.cross_user(uid, a.user_id, false, "isRoleAvailableAsUser")?;
                    let available = Self::exists(a.user_id) && {
                        let role = Self::not_empty(a.role_name, "roleName cannot be null or empty")?;
                        self.roles()?.holders(&role).is_some()
                    };
                    rm::write_is_role_available_as_user_reply(&mut p, available);
                    Ok(())
                })()
            }
            rm::IS_ROLE_HELD_AS_USER => {
                let a = rm::IsRoleHeldAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    // `AppOpsManager.checkPackage`.
                    let package = a.package_name.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "package"))?;
                    if !self.host.owns_package(uid, &package)? {
                        return Err(Exception::security(format!("Package {package} does not belong to {uid}")));
                    }
                    self.cross_user(uid, a.user_id, false, "isRoleHeldAsUser")?;
                    let held = Self::exists(a.user_id) && {
                        let role = Self::not_empty(a.role_name, "roleName cannot be null or empty")?;
                        let package = Self::not_empty(Some(package), "packageName cannot be null or empty")?;
                        self.roles()?.holders(&role).is_some_and(|h| h.contains(&package))
                    };
                    rm::write_is_role_held_as_user_reply(&mut p, held);
                    Ok(())
                })()
            }
            rm::GET_ROLE_HOLDERS_AS_USER => {
                let a = rm::GetRoleHoldersAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.role_holders(uid, a.role_name, a.user_id).map(|holders| {
                    let holders: Vec<_> = holders.into_iter().map(Some).collect();
                    rm::write_get_role_holders_as_user_reply(&mut p, &Some(holders));
                })
            }
            rm::GET_DEFAULT_APPLICATION_AS_USER => {
                let a = rm::GetDefaultApplicationAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.cross_user(uid, a.user_id, false, "getDefaultApplicationAsUser")?;
                    let holder = if Self::exists(a.user_id) {
                        self.enforce(uid, "android.permission.MANAGE_DEFAULT_APPLICATIONS", "getDefaultApplicationAsUser")?;
                        let role = Self::not_empty(a.role_name, "roleName cannot be null or empty")?;
                        if !DEFAULT_APPLICATION_ROLES.contains(&role.as_str()) {
                            return Err(Exception::illegal_argument(format!("{role} is not supported")));
                        }
                        self.roles()?.holders(&role).and_then(|h| h.first().cloned())
                    } else {
                        None
                    };
                    rm::write_get_default_application_as_user_reply(&mut p, &holder);
                    Ok(())
                })()
            }
            rm::ADD_ON_ROLE_HOLDERS_CHANGED_LISTENER_AS_USER | rm::REMOVE_ON_ROLE_HOLDERS_CHANGED_LISTENER_AS_USER => {
                let add = call.code == rm::ADD_ON_ROLE_HOLDERS_CHANGED_LISTENER_AS_USER;
                let (listener, user) = if add {
                    let a = rm::AddOnRoleHoldersChangedListenerAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.listener, a.user_id)
                } else {
                    let a = rm::RemoveOnRoleHoldersChangedListenerAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.listener, a.user_id)
                };
                let name = if add { "addOnRoleHoldersChangedListenerAsUser" } else { "removeOnRoleHoldersChangedListenerAsUser" };
                (|| {
                    self.cross_user(uid, user, true, name)?;
                    if user != USER_ALL && !Self::exists(user) {
                        return Ok(());
                    }
                    self.enforce(uid, "android.permission.OBSERVE_ROLE_HOLDERS", name)?;
                    let Some(Binder::Handle(listener)) = listener else {
                        return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "listener cannot be null"));
                    };
                    if add {
                        self.add_listener(listener, user);
                    } else {
                        self.remove_listener(listener);
                    }
                    p.write_no_exception();
                    Ok(())
                })()
            }
            rm::IS_ROLE_FALLBACK_ENABLED_AS_USER => {
                let a = rm::IsRoleFallbackEnabledAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.cross_user(uid, a.user_id, false, "isRoleFallbackEnabledAsUser")?;
                    let enabled = Self::exists(a.user_id) && {
                        self.enforce(uid, "android.permission.MANAGE_ROLE_HOLDERS", "isRoleFallbackEnabledAsUser")?;
                        let role = Self::not_empty(a.role_name, "roleName cannot be null or empty")?;
                        self.roles()?.fallback_enabled.contains(&role)
                    };
                    rm::write_is_role_fallback_enabled_as_user_reply(&mut p, enabled);
                    Ok(())
                })()
            }
            rm::GET_HELD_ROLES_FROM_CONTROLLER_AS_USER => {
                let a = rm::GetHeldRolesFromControllerAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.cross_user(uid, a.user_id, false, "getHeldRolesFromControllerAsUser")?;
                    let mut held = Vec::new();
                    if Self::exists(a.user_id) {
                        self.enforce(uid, "com.android.permissioncontroller.permission.MANAGE_ROLES_FROM_CONTROLLER", "getHeldRolesFromControllerAsUser")?;
                        let package = Self::not_empty(a.package_name, "packageName cannot be null or empty")?;
                        let roles = self.roles()?;
                        held = roles.roles.iter().filter(|(_, h)| h.contains(&package)).map(|(n, _)| Some(n.clone())).collect();
                    }
                    rm::write_get_held_roles_from_controller_as_user_reply(&mut p, &Some(held));
                    Ok(())
                })()
            }
            rm::IS_BYPASSING_ROLE_QUALIFICATION => {
                rm::IsBypassingRoleQualification::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `mBypassingRoleQualification`, which nothing sets.
                self.enforce(uid, "android.permission.MANAGE_ROLE_HOLDERS", "isBypassingRoleQualification")
                    .map(|()| rm::write_is_bypassing_role_qualification_reply(&mut p, false))
            }
            rm::GET_BROWSER_ROLE_HOLDER => {
                let a = rm::GetBrowserRoleHolder::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    if uid / PER_USER_RANGE as i32 != a.user_id {
                        self.enforce_with(uid, "android.permission.INTERACT_ACROSS_USERS_FULL", None)?;
                    }
                    // `isInstantApp` of the caller's package.
                    let (state, _) = self.host.packages()?;
                    let instant = state.packages.values().any(|ps| {
                        ps.app_id == uid.rem_euclid(PER_USER_RANGE as i32) && ps.users.get(&(uid / PER_USER_RANGE as i32)).is_some_and(|u| u.instant_app)
                    });
                    // As the system, without its own permission check.
                    let holder = if instant || !Self::exists(a.user_id) {
                        None
                    } else {
                        self.roles()?.holders(ROLE_BROWSER).and_then(|h| h.first().cloned())
                    };
                    rm::write_get_browser_role_holder_reply(&mut p, &holder);
                    Ok(())
                })()
            }
            rm::GET_SMS_ROLE_HOLDER => {
                let a = rm::GetSmsRoleHolder::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.cross_user(uid, a.user_id, false, "getSmsRoleHolder")?;
                    let holder = if Self::exists(a.user_id) { self.queryable_holder(uid, ROLE_SMS, a.user_id)? } else { None };
                    rm::write_get_sms_role_holder_reply(&mut p, &holder);
                    Ok(())
                })()
            }
            rm::GET_EMERGENCY_ROLE_HOLDER => {
                let a = rm::GetEmergencyRoleHolder::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.cross_user(uid, a.user_id, false, "getEmergencyRoleHolder")?;
                    let holder = if Self::exists(a.user_id) {
                        self.enforce(uid, "android.permission.READ_PRIVILEGED_PHONE_STATE", "getEmergencyRoleHolder")?;
                        self.queryable_holder(uid, ROLE_EMERGENCY, a.user_id)?
                    } else {
                        None
                    };
                    rm::write_get_emergency_role_holder_reply(&mut p, &holder);
                    Ok(())
                })()
            }
            code => Err(self.host.unimplemented_named("role", &format!("{} (#1657)", rm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n)))),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roles, holders and fallback read as `RolesPersistenceImpl` reads
    /// them.
    #[test]
    fn roles_are_read_as_the_persistence_reads_them() {
        let xml = br#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<roles version="1" packagesHash="AB">
  <role name="android.app.role.SYSTEM_GALLERY" fallbackEnabled="TRUE"><holder name="com.google.android.apps.photos" /></role>
  <role name="android.app.role.RETAIL_DEMO" fallbackEnabled="false" />
  <role name="android.app.role.BROWSER"><holder name="b" /><holder name="a" /><holder name="b" /></role>
</roles>"#;
        let roles = Roles::parse(&aim_android_xml::read(xml).unwrap()).unwrap();
        assert_eq!(roles.holders("android.app.role.SYSTEM_GALLERY"), Some(&["com.google.android.apps.photos".to_string()][..]));
        assert_eq!(roles.holders("android.app.role.RETAIL_DEMO"), Some(&[][..]));
        assert_eq!(roles.holders("android.app.role.DIALER"), None);
        // An `ArraySet` keeps its items by hash: "a" before "b".
        assert_eq!(roles.holders(ROLE_BROWSER), Some(&["a".to_string(), "b".to_string()][..]));
        assert_eq!(roles.fallback_enabled, ["android.app.role.SYSTEM_GALLERY"]);
        assert!(Roles::parse(&aim_android_xml::read(b"<other />").unwrap()).is_err());
    }
}
