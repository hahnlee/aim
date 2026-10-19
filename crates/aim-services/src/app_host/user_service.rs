//! `user`: UserManager for the app host, which runs user 0 alone,
//! unlocked, visible and in the foreground, as UserManagerService last
//! persisted it (`/data/system/users/0.xml`: its `UserInfo` and
//! restrictions).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeSet;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_iusermanager as um;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;

/// The only user.
const SYSTEM_USER: i32 = 0;
/// `UserHandle.USER_NULL`, also `UserInfo.NO_PROFILE_GROUP_ID`.
const USER_NULL: i32 = -10000;
/// `UserInfo.FLAG_PRIMARY`, `FLAG_ADMIN`, `FLAG_DISABLED`, `FLAG_DEMO`,
/// `FLAG_FULL`, `FLAG_MAIN`.
const FLAG_PRIMARY: i32 = 0x1;
const FLAG_ADMIN: i32 = 0x2;
const FLAG_DISABLED: i32 = 0x40;
const FLAG_DEMO: i32 = 0x200;
const FLAG_FULL: i32 = 0x400;
const FLAG_MAIN: i32 = 0x4000;
/// `UserManager.USER_TYPE_FULL_GUEST`, `USER_TYPE_FULL_RESTRICTED`.
const USER_TYPE_FULL_GUEST: &str = "android.os.usertype.full.GUEST";
const USER_TYPE_FULL_RESTRICTED: &str = "android.os.usertype.full.RESTRICTED";

const MANAGE_USERS: &str = "android.permission.MANAGE_USERS";
const CREATE_USERS: &str = "android.permission.CREATE_USERS";
const QUERY_USERS: &str = "android.permission.QUERY_USERS";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const GET_ACCOUNTS_PRIVILEGED: &str = "android.permission.GET_ACCOUNTS_PRIVILEGED";

/// `UserInfo` as `readUserLP` reads it and `writeToParcel` writes it.
#[derive(Clone, Debug, PartialEq)]
struct UserInfo {
    id: i32,
    name: Option<String>,
    icon_path: Option<String>,
    flags: i32,
    user_type: Option<String>,
    serial_number: i32,
    creation_time: i64,
    last_logged_in_time: i64,
    last_logged_in_fingerprint: Option<String>,
    partial: bool,
    pre_created: bool,
    profile_group_id: i32,
    guest_to_remove: bool,
    restricted_profile_parent_id: i32,
    profile_badge: i32,
}

impl UserInfo {
    /// `UserManagerService.isSameProfileGroup`.
    fn same_profile_group(&self, other: &UserInfo) -> bool {
        self.id == other.id || self.profile_group_id != USER_NULL && self.profile_group_id == other.profile_group_id
    }
}

impl WriteParcelable for UserInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.id);
        p.write_string8(self.name.as_deref());
        p.write_string8(self.icon_path.as_deref());
        p.write_i32(self.flags);
        p.write_string8(self.user_type.as_deref());
        p.write_i32(self.serial_number);
        p.write_i64(self.creation_time);
        p.write_i64(self.last_logged_in_time);
        p.write_string8(self.last_logged_in_fingerprint.as_deref());
        p.write_bool(self.partial);
        p.write_bool(self.pre_created);
        p.write_i32(self.profile_group_id);
        p.write_bool(self.guest_to_remove);
        p.write_i32(self.restricted_profile_parent_id);
        p.write_i32(self.profile_badge);
    }
}

/// A user file (`readUserLP`): its `UserInfo`, and its effective
/// restrictions (`computeEffectiveUserRestrictionsLR`: the base ones, then
/// the device policy's global and local ones; `UserRestrictionsUtils.
/// readRestrictions` keeps those set).
fn read_user(root: &aim_android_xml::Element) -> Result<(UserInfo, BTreeSet<String>), String> {
    if root.name != "user" {
        return Err(format!("<{}>", root.name));
    }
    let id = root.int("id")?.unwrap_or(-1);
    let int = |name: &str, default: i32| root.int(name).map(|v| v.unwrap_or(default));
    let long = |name: &str| root.long(name).map(|v| v.unwrap_or(0));
    let flag = |name: &str| root.bool(name).map(|v| v.unwrap_or(false));
    let string = |name: &str| root.string(name).map(|s| s.into_owned());
    // The text right after `<name>`.
    let name = root.children().find(|e| e.name == "name").and_then(|e| match e.content.first() {
        Some(aim_android_xml::Node::Token(aim_android_xml::TEXT, text)) => text.clone(),
        _ => None,
    });
    let mut restrictions = BTreeSet::new();
    for tag in ["restrictions", "device_policy_global_restrictions"] {
        restrictions.extend(set_restrictions(root, tag)?);
    }
    // The local ones (version 10 on), else the legacy ones.
    if let Some(local) = root.children().find(|e| e.name == "device_policy_local_restrictions") {
        if local.children().next().is_some() {
            return Err("local restrictions of a user version before 10".into());
        }
        restrictions.extend(set_restrictions(root, "device_policy_local_restrictions")?);
    } else {
        restrictions.extend(set_restrictions(root, "device_policy_restrictions")?);
    }
    let info = UserInfo {
        id,
        name,
        icon_path: string("icon"),
        flags: int("flags", 0)?,
        user_type: string("type"),
        serial_number: int("serialNumber", id)?,
        creation_time: long("created")?,
        last_logged_in_time: long("lastLoggedIn")?,
        last_logged_in_fingerprint: string("lastLoggedInFingerprint"),
        partial: flag("partial")?,
        pre_created: flag("preCreated")?,
        profile_group_id: int("profileGroupId", USER_NULL)?,
        guest_to_remove: flag("guestToRemove")?,
        restricted_profile_parent_id: int("restrictedProfileParentId", USER_NULL)?,
        profile_badge: int("profileBadge", 0)?,
    };
    Ok((info, restrictions))
}

/// The restrictions the element `tag` of `user` sets.
fn set_restrictions(user: &aim_android_xml::Element, tag: &str) -> Result<Vec<String>, String> {
    let Some(e) = user.children().find(|e| e.name == tag) else { return Ok(Vec::new()) };
    let mut set = Vec::new();
    for (key, _) in &e.attrs {
        if e.bool(key)? == Some(true) {
            set.push(key.clone());
        }
    }
    Ok(set)
}

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

    /// User 0's file: its `UserInfo` and effective restrictions.
    fn user_data(&self) -> Result<(UserInfo, BTreeSet<String>), Exception> {
        let path = self.host.data()?.join("system/users/0.xml");
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
        let bytes = std::fs::read(&path).map_err(|e| state(e.to_string()))?;
        read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)
    }

    /// `getUserInfoLU`: none of a user that does not exist or is partial.
    fn user(&self, id: i32) -> Result<Option<UserInfo>, Exception> {
        if id != SYSTEM_USER {
            return Ok(None);
        }
        Ok(Some(self.user_data()?.0).filter(|u| !u.partial))
    }

    /// `getProfileParentLU`.
    fn profile_parent(&self, id: i32) -> Result<Option<UserInfo>, Exception> {
        match self.user(id)? {
            Some(u) if u.profile_group_id != id && u.profile_group_id != USER_NULL => self.user(u.profile_group_id),
            _ => Ok(None),
        }
    }

    /// `userWithName`: the owner's name for the system or main user, the
    /// guest's for a guest, when it has none.
    fn with_name(&self, user: Option<UserInfo>) -> Result<Option<UserInfo>, Exception> {
        let Some(mut user) = user else { return Ok(None) };
        if user.name.is_none() {
            let resource = if user.id == SYSTEM_USER || user.flags & FLAG_MAIN != 0 {
                Some("owner_name")
            } else if user.user_type.as_deref() == Some(USER_TYPE_FULL_GUEST) {
                Some("guest_name")
            } else {
                None
            };
            if let Some(resource) = resource {
                let name = self.host.inputs()?.packages.framework_string(resource);
                user.name = Some(name.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no framework string {resource}")))?);
            }
        }
        Ok(Some(user))
    }

    /// `hasPermissionGranted`.
    fn granted(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `hasManageUsersOrPermission`: the system or root, MANAGE_USERS, or
    /// `alternative` (`hasManageUsersPermission` without one).
    fn manage_or(&self, uid: i32, alternative: Option<&str>) -> Result<bool, Exception> {
        Ok(uid as u32 % PER_USER_RANGE == 1000
            || uid == 0
            || self.granted(uid, MANAGE_USERS)?
            || alternative.map(|p| self.granted(uid, p)).transpose()?.unwrap_or(false))
    }

    /// `hasQueryOrCreateUsersPermission`.
    fn query_or_create(&self, uid: i32) -> Result<bool, Exception> {
        Ok(self.manage_or(uid, Some(CREATE_USERS))? || self.granted(uid, QUERY_USERS)?)
    }

    /// `checkQueryOrCreateUsersPermission(message)`.
    fn check_query_or_create(&self, uid: i32, message: &str) -> Result<(), Exception> {
        if self.query_or_create(uid)? {
            return Ok(());
        }
        Err(Exception::security(format!("You either need MANAGE_USERS, CREATE_USERS, or QUERY_USERS permission to: {message}")))
    }

    /// `checkQueryOrCreateUsersPermissionIfCallerInOtherProfileGroup`.
    fn check_query_or_create_other(&self, uid: i32, user: i32, name: &str) -> Result<(), Exception> {
        let caller = uid / PER_USER_RANGE as i32;
        let same_group = match (self.user(caller)?, self.user(user)?) {
            (Some(a), Some(b)) => a.same_profile_group(&b),
            _ => false,
        };
        if caller == user || same_group {
            return Ok(());
        }
        self.check_query_or_create(uid, name)
    }

    /// The query of `call`, a transaction the arms of `transact` do not
    /// serve.
    fn query(&self, call: &mut Call<'_>, p: &mut Parcel) -> Result<Result<(), Exception>, i32> {
        let uid = call.sender_euid as i32;
        let caller = uid / PER_USER_RANGE as i32;
        let require = |held: Result<bool, Exception>, message: String| match held {
            Ok(true) => Ok(()),
            Ok(false) => Err(Exception::security(message)),
            Err(e) => Err(e),
        };
        Ok(match call.code {
            um::GET_PRIMARY_USER => {
                um::GetPrimaryUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(self.manage_or(uid, None), "You need MANAGE_USERS permission to: query users".into())
                    .and_then(|_| self.user(SYSTEM_USER))
                    .map(|u| um::write_get_primary_user_reply(p, u.filter(|u| u.flags & FLAG_PRIMARY != 0).as_ref()))
            }
            um::GET_MAIN_USER_ID => {
                um::GetMainUserId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // Partial users count.
                self.check_query_or_create(uid, "get main user id")
                    .and_then(|_| self.user_data())
                    .map(|(u, _)| um::write_get_main_user_id_reply(p, if u.flags & FLAG_MAIN != 0 { u.id } else { USER_NULL }))
            }
            um::GET_USERS => {
                let a = um::GetUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(self.manage_or(uid, Some(CREATE_USERS)), "You either need MANAGE_USERS or CREATE_USERS permission to: query users".into())
                    .and_then(|_| self.user_data())
                    .and_then(|(u, _)| {
                        let listed = !(a.exclude_partial && u.partial || a.exclude_pre_created && u.pre_created);
                        let users: Vec<_> = if listed { vec![self.with_name(Some(u))?] } else { Vec::new() };
                        Ok(um::write_get_users_reply(p, Some(&users)))
                    })
            }
            um::GET_PROFILES => {
                let a = um::GetProfiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let full = if a.user_id != caller {
                    self.check_query_or_create(uid, &format!("getting profiles related to user {}", a.user_id)).map(|_| true)
                } else {
                    self.manage_or(uid, Some(CREATE_USERS))
                };
                // `getProfilesLU`: the user's profile group, user 0 alone;
                // without full information, no name or icon.
                full.and_then(|full| {
                    let user = self.user(a.user_id)?.filter(|u| !a.enabled_only || u.flags & FLAG_DISABLED == 0);
                    let user = if full {
                        self.with_name(user)?
                    } else {
                        user.map(|u| UserInfo { name: None, icon_path: None, ..u })
                    };
                    let users: Vec<_> = user.into_iter().map(Some).collect();
                    Ok(um::write_get_profiles_reply(p, Some(&users)))
                })
            }
            um::GET_PROFILE_PARENT => {
                let a = um::GetProfileParent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, Some(INTERACT_ACROSS_USERS)),
                    "You need MANAGE_USERS or INTERACT_ACROSS_USERS permission to get the profile parent".into(),
                )
                .and_then(|_| self.profile_parent(a.user_id))
                .map(|u| um::write_get_profile_parent_reply(p, u.as_ref()))
            }
            um::GET_PROFILE_PARENT_ID => {
                let a = um::GetProfileParentId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(self.manage_or(uid, None), "You need MANAGE_USERS permission to: get the profile parent".into())
                    .and_then(|_| self.profile_parent(a.user_id))
                    .map(|u| um::write_get_profile_parent_id_reply(p, u.map_or(a.user_id, |u| u.id)))
            }
            um::IS_SAME_PROFILE_GROUP => {
                let a = um::IsSameProfileGroup::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if a.user_id == a.other_user_handle {
                    Ok(um::write_is_same_profile_group_reply(p, true))
                } else {
                    require(
                        self.manage_or(uid, Some(QUERY_USERS)),
                        "You either need MANAGE_USERS or QUERY_USERS permission to: check if in the same profile group".into(),
                    )
                    .and_then(|_| Ok((self.user(a.user_id)?, self.user(a.other_user_handle)?)))
                    .map(|users| {
                        let same = matches!(users, (Some(a), Some(b)) if a.same_profile_group(&b));
                        um::write_is_same_profile_group_reply(p, same)
                    })
                }
            }
            um::IS_USER_OF_TYPE => {
                let a = um::IsUserOfType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_query_or_create(uid, "check user type").and_then(|_| self.user(a.user_id)).map(|u| {
                    let of_type = a.user_type.is_some() && u.and_then(|u| u.user_type) == a.user_type;
                    um::write_is_user_of_type_reply(p, of_type)
                })
            }
            um::GET_USER_INFO => {
                let a = um::GetUserInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_query_or_create(uid, "query user")
                    .and_then(|_| self.with_name(self.user(a.user_id)?))
                    .map(|u| um::write_get_user_info_reply(p, u.as_ref()))
            }
            // The caller's own user, or a profile of it.
            um::GET_USER_CREATION_TIME => {
                let a = um::GetUserCreationTime::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let user = if caller == a.user_id {
                    self.user(a.user_id)
                } else {
                    self.profile_parent(a.user_id).and_then(|parent| match parent {
                        Some(parent) if parent.id == caller => self.user(a.user_id),
                        _ => Ok(None),
                    })
                };
                user.and_then(|u| {
                    u.ok_or_else(|| Exception::security("userId can only be the calling user or a profile associated with this user"))
                })
                .map(|u| um::write_get_user_creation_time_reply(p, u.creation_time))
            }
            um::IS_RESTRICTED => {
                let a = um::IsRestricted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let checked = if a.user_id != caller {
                    self.check_query_or_create(uid, &format!("query isRestricted for user {}", a.user_id))
                } else {
                    Ok(())
                };
                checked.and_then(|_| self.user(a.user_id)).map(|u| {
                    let restricted = u.is_some_and(|u| u.user_type.as_deref() == Some(USER_TYPE_FULL_RESTRICTED));
                    um::write_is_restricted_reply(p, restricted)
                })
            }
            um::GET_USER_SERIAL_NUMBER => {
                let a = um::GetUserSerialNumber::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.user(a.user_id).map(|u| um::write_get_user_serial_number_reply(p, u.map_or(-1, |u| u.serial_number)))
            }
            // A copy of the effective restrictions: none of a user that does
            // not exist.
            um::GET_USER_RESTRICTIONS => {
                let a = um::GetUserRestrictions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_other_user(call, a.user_id, false, "getUserRestrictions")
                    .and_then(|_| if a.user_id == SYSTEM_USER { Ok(self.user_data()?.1) } else { Ok(BTreeSet::new()) })
                    .map(|set| {
                        let entries = set.into_iter().map(|k| (k, Value::Bool(true))).collect();
                        let entries = crate::package::info::array_order(entries, |(k, _)| k);
                        um::write_get_user_restrictions_reply(p, Some(&super::bind::Bundle(entries)))
                    })
            }
            um::IS_ADMIN_USER => {
                let a = um::IsAdminUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_query_or_create_other(uid, a.user_id, "isAdminUser")
                    .and_then(|_| self.user(a.user_id))
                    .map(|u| um::write_is_admin_user_reply(p, u.is_some_and(|u| u.flags & FLAG_ADMIN != 0)))
            }
            um::IS_PRE_CREATED => {
                let a = um::IsPreCreated::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_other_user(call, a.user_id, false, "isPreCreated")
                    .and_then(|_| self.user(a.user_id))
                    .map(|u| um::write_is_pre_created_reply(p, u.is_some_and(|u| u.pre_created)))
            }
            // User 0 is the current user.
            um::IS_USER_FOREGROUND => {
                let a = um::IsUserForeground::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let held = if caller == a.user_id { Ok(true) } else { self.manage_or(uid, Some(INTERACT_ACROSS_USERS)) };
                require(
                    held,
                    format!(
                        "Caller from user {caller} needs MANAGE_USERS or INTERACT_ACROSS_USERS permission to check if another user ({}) is running in the foreground",
                        a.user_id
                    ),
                )
                .map(|_| um::write_is_user_foreground_reply(p, a.user_id == SYSTEM_USER))
            }
            um::GET_VISIBLE_USERS => {
                um::GetVisibleUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, Some(INTERACT_ACROSS_USERS)),
                    "Caller needs MANAGE_USERS or INTERACT_ACROSS_USERS permission to get list of visible users".into(),
                )
                .map(|_| um::write_get_visible_users_reply(p, &Some(vec![SYSTEM_USER])))
            }
            um::IS_FOREGROUND_USER_ADMIN => {
                um::IsForegroundUserAdmin::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.user(SYSTEM_USER).map(|u| um::write_is_foreground_user_admin_reply(p, u.is_some_and(|u| u.flags & FLAG_ADMIN != 0)))
            }
            um::IS_USER_NAME_SET => {
                let a = um::IsUserNameSet::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let held = self.query_or_create(uid).and_then(|q| Ok(q || caller == a.user_id && self.granted(uid, GET_ACCOUNTS_PRIVILEGED)?));
                require(held, "You need MANAGE_USERS, CREATE_USERS, QUERY_USERS, or GET_ACCOUNTS_PRIVILEGED permissions to: get whether user name is set".into())
                    .and_then(|_| self.user(a.user_id))
                    .map(|u| um::write_is_user_name_set_reply(p, u.is_some_and(|u| u.name.is_some())))
            }
            // The caller's user's name, or "".
            um::GET_USER_NAME => {
                um::GetUserName::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let held = self.query_or_create(uid).and_then(|q| Ok(q || self.granted(uid, GET_ACCOUNTS_PRIVILEGED)?));
                require(held, "You need MANAGE_USERS, CREATE_USERS, QUERY_USERS, or GET_ACCOUNTS_PRIVILEGED permissions to: get user name".into())
                    .and_then(|_| self.with_name(self.user(caller)?))
                    .map(|u| um::write_get_user_name_reply(p, &Some(u.and_then(|u| u.name).unwrap_or_default())))
            }
            code => Err(self.host.unimplemented("user", code)),
        })
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
            // `getUserRestrictionSources`: none for a restriction not in
            // effect; the sources of one in effect are #1709.
            um::GET_USER_RESTRICTION_SOURCES => {
                let args = um::GetUserRestrictionSources::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                let held = |perm: &str| matches!(self.host.check_permission(Some(perm), uid), Ok(0));
                if !matches!(uid % 100_000, 0 | 1000) && !held("android.permission.MANAGE_USERS") && !held("android.permission.QUERY_USERS") {
                    return reply(Err(Exception::security("You either need MANAGE_USERS or QUERY_USERS permission to call getUserRestrictionSources.")));
                }
                let restricted = match self.host.user_restrictions() {
                    Ok(r) => args.user_id == SYSTEM_USER && args.restriction_key.is_some_and(|k| r.contains(&k)),
                    Err(e) => return reply(Err(e)),
                };
                if restricted {
                    return reply(Err(self.host.unimplemented_named("user", "getUserRestrictionSources of a restriction in effect (#1709)")));
                }
                p.write_no_exception();
                p.write_i32(0);
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
            _ => {
                if let Err(e) = self.query(call, &mut p)? {
                    return reply(Err(e));
                }
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The attributes, the name's text, and the base and device policy
    /// restrictions set, as `readUserLP` reads them.
    #[test]
    fn reads_a_user_as_the_original() {
        let xml = br#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<user id="0" serialNumber="0" flags="19475" type="android.os.usertype.full.SYSTEM" created="0" lastLoggedIn="1792332278199" lastLoggedInFingerprint="f" profileBadge="0">
  <name>Kim</name>
  <restrictions no_sms="true" no_usb_file_transfer="false" />
  <device_policy_global_restrictions no_config_wifi="true" />
  <guestRestrictions>
    <restrictions no_outgoing_calls="true" />
  </guestRestrictions>
  <device_policy_local_restrictions />
</user>"#;
        let (info, restrictions) = read_user(&aim_android_xml::read(xml).unwrap()).unwrap();
        assert_eq!((info.id, info.flags, info.serial_number), (0, 19475, 0));
        assert_eq!((info.profile_group_id, info.restricted_profile_parent_id), (USER_NULL, USER_NULL));
        assert_eq!(info.last_logged_in_time, 1792332278199);
        assert_eq!(info.name.as_deref(), Some("Kim"));
        assert_eq!(info.icon_path, None);
        assert_eq!(restrictions.into_iter().collect::<Vec<_>>(), ["no_config_wifi", "no_sms"]);
    }
}
