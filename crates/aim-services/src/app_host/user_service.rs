//! `user`: UserManager for the app host, which runs user 0 alone,
//! unlocked, visible and in the foreground, as UserManagerService last
//! persisted it (`/data/system/users/0.xml`: its `UserInfo` and
//! restrictions), or as it records the system user at first boot when no
//! user list was persisted (`fallbackToSingleUserLP`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use aim_android_xml::{Element, Node, TEXT, Value as XmlValue};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_iusermanager as um;

use super::user_types::{self, UserType};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;

/// The only user.
pub(super) const SYSTEM_USER: i32 = 0;
/// `UserManager.DISALLOW_DEBUGGING_FEATURES`.
const DISALLOW_DEBUGGING_FEATURES: &str = "no_debugging_features";
/// `UserHandle.USER_NULL`, also `UserInfo.NO_PROFILE_GROUP_ID`.
const USER_NULL: i32 = -10000;
/// `UserInfo.FLAG_PRIMARY`, `FLAG_ADMIN`, `FLAG_DISABLED`, `FLAG_DEMO`,
/// `FLAG_FULL`, `FLAG_MAIN`.
const FLAG_PRIMARY: i32 = 0x1;
const FLAG_ADMIN: i32 = 0x2;
const FLAG_DISABLED: i32 = 0x40;
const FLAG_DEMO: i32 = 0x200;
pub(super) const FLAG_FULL: i32 = 0x400;
const FLAG_MAIN: i32 = 0x4000;
const FLAG_INITIALIZED: i32 = 0x10;
/// `UserManager.USER_TYPE_FULL_GUEST`, `USER_TYPE_FULL_RESTRICTED`.
const USER_TYPE_FULL_GUEST: &str = "android.os.usertype.full.GUEST";
const USER_TYPE_FULL_RESTRICTED: &str = "android.os.usertype.full.RESTRICTED";
/// `UserManager.USER_TYPE_PROFILE_MANAGED`.
const USER_TYPE_PROFILE_MANAGED: &str = "android.os.usertype.profile.MANAGED";

/// The multiuser flags the user types and limits read.
const BLOCK_PRIVATE_SPACE_CREATION: &str = "android.multiuser.block_private_space_creation";
const ALLOW_SUPERVISING_PROFILE: &str = "android.multiuser.allow_supervising_profile";
const PROFILES_FOR_ALL: &str = "android.multiuser.profiles_for_all";
/// `UserManager.DEV_CREATE_OVERRIDE_PROPERTY`, `DISALLOW_ADD_PRIVATE_PROFILE`.
const DEV_CREATE_OVERRIDE_PROPERTY: &str = "debug.user.creation_override";
const DISALLOW_ADD_PRIVATE_PROFILE: &str = "no_add_private_profile";
/// `PackageManager.FEATURE_*` the limits and private space read.
const FEATURE_MANAGED_USERS: &str = "android.software.managed_users";
const FEATURE_EMBEDDED: &str = "android.hardware.type.embedded";
const FEATURE_WATCH: &str = "android.hardware.type.watch";
const FEATURE_LEANBACK: &str = "android.software.leanback";
const FEATURE_AUTOMOTIVE: &str = "android.hardware.type.automotive";

/// `UserInfo.FLAG_SYSTEM`.
const FLAG_SYSTEM: i32 = 0x800;
/// `UserManager.USER_TYPE_FULL_SYSTEM`, `USER_TYPE_SYSTEM_HEADLESS`.
const USER_TYPE_FULL_SYSTEM: &str = "android.os.usertype.full.SYSTEM";
const USER_TYPE_SYSTEM_HEADLESS: &str = "android.os.usertype.system.HEADLESS";
/// `UserManagerService.MIN_USER_ID` (`UserHandle.MIN_SECONDARY_USER_ID`),
/// `USER_VERSION`.
const MIN_USER_ID: i32 = 10;
const USER_VERSION: i32 = 11;
/// `UserTypeFactory.getDefaultGuestUserRestrictions`, which the guest
/// type adds to `mGuestRestrictions` (`initDefaultGuestRestrictions`).
const DEFAULT_GUEST_RESTRICTIONS: [&str; 5] = [
    "no_outgoing_calls",
    "no_sms",
    "no_config_wifi",
    "no_install_unknown_sources",
    "no_config_credentials",
];
/// `Flags.saveGlobalAndGuestRestrictionsOnSystemUserXmlReadOnly`.
const SAVE_ON_SYSTEM_USER_XML: &str =
    "android.multiuser.save_global_and_guest_restrictions_on_system_user_xml_read_only";
/// `ResilientAtomicFile`'s mode of the user files (`getUserFile`,
/// `getUserListFile`): `rwxrwx--x`.
const USER_FILE_MODE: u32 = 0o771;

const MANAGE_USERS: &str = "android.permission.MANAGE_USERS";
const CREATE_USERS: &str = "android.permission.CREATE_USERS";
const QUERY_USERS: &str = "android.permission.QUERY_USERS";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const GET_ACCOUNTS_PRIVILEGED: &str = "android.permission.GET_ACCOUNTS_PRIVILEGED";

/// `UserInfo` as `readUserLP` reads it and `writeToParcel` writes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct UserInfo {
    pub(super) id: i32,
    name: Option<String>,
    icon_path: Option<String>,
    pub(super) flags: i32,
    pub(super) user_type: Option<String>,
    serial_number: i32,
    creation_time: i64,
    last_logged_in_time: i64,
    last_logged_in_fingerprint: Option<String>,
    pub(super) partial: bool,
    pub(super) pre_created: bool,
    pub(super) profile_group_id: i32,
    pub(super) guest_to_remove: bool,
    restricted_profile_parent_id: i32,
    profile_badge: i32,
}

impl UserInfo {
    /// A user of `user_type` with `flags` in profile group
    /// `profile_group_id`, for the user types' tests.
    #[cfg(test)]
    pub(super) fn of_type(id: i32, flags: i32, user_type: &str, profile_group_id: i32) -> Self {
        UserInfo {
            id,
            flags,
            user_type: Some(user_type.into()),
            profile_group_id,
            ..UserInfo::default()
        }
    }

    /// `UserManagerService.isSameProfileGroup`.
    pub(super) fn same_profile_group(&self, other: &UserInfo) -> bool {
        self.id == other.id
            || self.profile_group_id != USER_NULL && self.profile_group_id == other.profile_group_id
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
    let name = root
        .children()
        .find(|e| e.name == "name")
        .and_then(|e| match e.content.first() {
            Some(aim_android_xml::Node::Token(aim_android_xml::TEXT, text)) => text.clone(),
            _ => None,
        });
    let mut restrictions = BTreeSet::new();
    for tag in ["restrictions", "device_policy_global_restrictions"] {
        restrictions.extend(set_restrictions(root, tag)?);
    }
    // The local ones (version 10 on), else the legacy ones.
    if let Some(local) = root
        .children()
        .find(|e| e.name == "device_policy_local_restrictions")
    {
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
    let Some(e) = user.children().find(|e| e.name == tag) else {
        return Ok(Vec::new());
    };
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
    (
        "updateCrossProfileIntentFiltersOnOTA",
        5,
        Visible::System,
        0,
    ),
    (
        "crossProfileIntentFilterAccessControl",
        6,
        Visible::System,
        0,
    ),
    (
        "crossProfileIntentResolutionStrategy",
        7,
        Visible::System,
        0,
    ),
    ("mediaSharedWithParent", 8, Visible::Anyone, 0),
    ("credentialShareableWithParent", 9, Visible::Anyone, 0),
    (
        "authAlwaysRequiredToDisableQuietMode",
        13,
        Visible::Manage,
        0,
    ),
    (
        "allowStoppingUserWithDelayedLocking",
        16,
        Visible::System,
        0,
    ),
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
        let mut copy = UserProperties {
            present: 0,
            values: [0; 19],
        };
        for (i, (name, index, visible, default)) in USER_PROPERTIES.iter().enumerate() {
            let shown = match visible {
                Visible::Anyone => true,
                Visible::Manage => manage,
                Visible::System => expose_all,
            };
            if shown {
                copy.present |= 1 << index;
                copy.values[i] = persisted
                    .iter()
                    .find(|(n, _)| n == name)
                    .map_or(*default, |(_, v)| *v);
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
    if user == SYSTEM_USER {
        vec![SYSTEM_USER]
    } else {
        Vec::new()
    }
}

/// `readUserListLP`'s first boot: with no user list persisted,
/// `fallbackToSingleUserLP` records the system user (`writeUserLP`, then
/// `writeUserListLP`), as the app host does at its start.
pub(super) fn ensure_system_user(host: &AppHost) -> Result<(), String> {
    let data = host.data().map_err(|e| e.message)?;
    let users = data.join("system/users");
    if users.join("userlist.xml").exists() {
        return Ok(());
    }
    let packages = host.framework().map_err(|e| e.message)?;
    let save_on_system_user = packages
        .0
        .flags
        .get(SAVE_ON_SYSTEM_USER_XML)
        .copied()
        .unwrap_or(false);
    // `isDefaultHeadlessSystemUserMode` of a user build:
    // `ro.fw.mu.headless_system_user`.
    let headless = super::system_boolean(
        host.property("ro.fw.mu.headless_system_user").as_deref(),
        false,
    );
    let user_types = packages
        .framework_file(
            host.root().map_err(|e| e.message)?,
            "xml",
            "config_user_types",
        )
        .map(|xml| aim_apps::res::xml(&xml).map_err(|e| format!("config_user_types: {e:?}")))
        .transpose()?;
    if user_types.as_ref().is_some_and(|t| !t.children.is_empty()) {
        return Err(
            "the user types' customization (config_user_types) is not implemented (#1900)".into(),
        );
    }
    // `UserTypeFactory.getUserTypeVersion`.
    let user_type_version = match user_types.as_ref().and_then(|t| t.named("version")) {
        Some(aim_apps::res::Value::String(v)) => v.parse().unwrap_or(0),
        Some(aim_apps::res::Value::Int(v)) => *v,
        _ => 0,
    };
    let first_user_restrictions: Vec<String> = packages
        .framework_string_array("config_defaultFirstUserRestrictions")
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();
    if !first_user_restrictions.is_empty() {
        return Err("config_defaultFirstUserRestrictions' restrictions (#1900)".into());
    }
    let (user_type, flags) = if headless {
        (
            USER_TYPE_SYSTEM_HEADLESS,
            FLAG_SYSTEM | FLAG_PRIMARY | FLAG_ADMIN,
        )
    } else {
        (
            USER_TYPE_FULL_SYSTEM,
            FLAG_SYSTEM | FLAG_FULL | FLAG_PRIMARY | FLAG_ADMIN | FLAG_MAIN,
        )
    };
    let (user, list) = first_user_records(user_type, flags, user_type_version, save_on_system_user);
    write_users(&users, &user, &list)
}

/// `writeUserLP` and `writeUserListLP` of the system user as
/// `fallbackToSingleUserLP` makes it: of `user_type` with its type's
/// `flags`, initialized, no restrictions but the guests' defaults, kept
/// in the system user's file when `save_on_system_user`, else in the list.
fn first_user_records(
    user_type: &str,
    flags: i32,
    user_type_version: i32,
    save_on_system_user: bool,
) -> (Element, Element) {
    let restrictions = |tag: &str, keys: &[&str]| {
        element(
            tag,
            crate::package::info::array_order(keys.to_vec(), |k| k)
                .into_iter()
                .map(|k| (k, XmlValue::Bool(true)))
                .collect(),
            Vec::new(),
        )
    };
    let guests = || {
        element(
            "guestRestrictions",
            Vec::new(),
            vec![restrictions("restrictions", &DEFAULT_GUEST_RESTRICTIONS)],
        )
    };
    let mut children = vec![
        restrictions("restrictions", &[]),
        restrictions("device_policy_global_restrictions", &[]),
    ];
    if save_on_system_user {
        children.push(guests());
    }
    children.push(restrictions("device_policy_local_restrictions", &[]));
    children.push(element("userProperties", Vec::new(), Vec::new()));
    let mut ignore = element("ignorePrepareStorageErrors", Vec::new(), Vec::new());
    ignore.content.push(Node::Token(TEXT, Some("false".into())));
    children.push(ignore);
    let user = element(
        "user",
        vec![
            ("id", XmlValue::Int(SYSTEM_USER)),
            ("serialNumber", XmlValue::Int(SYSTEM_USER)),
            ("flags", XmlValue::Int(flags | FLAG_INITIALIZED)),
            ("type", XmlValue::String(user_type.into())),
            ("created", XmlValue::Long(0)),
            ("lastLoggedIn", XmlValue::Long(0)),
            ("lastEnteredForeground", XmlValue::Long(0)),
            ("profileBadge", XmlValue::Int(0)),
        ],
        children,
    );
    let mut list_children = Vec::new();
    if !save_on_system_user {
        list_children.push(guests());
    }
    list_children.push(element(
        "user",
        vec![("id", XmlValue::Int(SYSTEM_USER))],
        Vec::new(),
    ));
    let list = element(
        "users",
        vec![
            ("nextSerialNumber", XmlValue::Int(MIN_USER_ID)),
            ("version", XmlValue::Int(USER_VERSION)),
            ("userTypeConfigVersion", XmlValue::Int(user_type_version)),
        ],
        list_children,
    );
    (user, list)
}

/// `UserManager.getSerialNumberForUser`: the user's serial number, as
/// its file holds it; none for a user that does not exist.
pub(super) fn serial_number(host: &AppHost, user: i32) -> Result<Option<i32>, Exception> {
    if user != SYSTEM_USER {
        return Ok(None);
    }
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let (info, _) = read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)?;
    Ok(Some(info.serial_number))
}

/// UserManager.getMaxSupportedUsers, shared by its limits and package shell.
pub(super) fn max_supported_users(host: &AppHost) -> Result<i32, Exception> {
    if host
        .property("ro.build.id")
        .is_some_and(|id| id.starts_with("JVP"))
    {
        return Ok(1);
    }
    let config = host
        .inputs()?
        .packages
        .framework_integer("config_multiuserMaximumUsers")
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "missing config_multiuserMaximumUsers"))?;
    Ok(host
        .property("fw.max_users")
        .and_then(|v| v.trim_start().parse().ok())
        .unwrap_or(config)
        .max(1))
}

/// UserManagerServiceShellCommand.runList, invoked by pm list users with
/// exactly the original `list` arguments and the shell's identity.
pub(super) fn list_shell(
    host: Arc<AppHost>,
    uid: i32,
    output: &mut impl std::io::Write,
) -> Result<i32, Exception> {
    let owner = UserManager::new(host);
    if !owner.manage_or(uid, Some(CREATE_USERS))? {
        return Err(Exception::security(
            "You either need MANAGE_USERS or CREATE_USERS permission to: query users",
        ));
    }
    let (user, _) = owner.user_data()?;
    let mut users = Vec::new();
    if !user.partial && !user.pre_created {
        users.extend(owner.with_name(Some(user))?);
    }
    let write_error =
        |e: std::io::Error| Exception::new(EX_ILLEGAL_STATE, format!("user shell output: {e}"));
    writeln!(output, "Users:").map_err(write_error)?;
    for user in users {
        writeln!(
            output,
            "\tUserInfo{{{}:{}:{:x}}}{}",
            user.id,
            user.name.as_deref().unwrap_or("null"),
            user.flags,
            if user.id == SYSTEM_USER {
                " running"
            } else {
                ""
            }
        )
        .map_err(write_error)?;
    }
    Ok(0)
}

/// UserManagerService.getProfileParent: a full user has no profile parent.
pub(super) fn profile_parent(host: &AppHost, user: i32) -> Result<Option<i32>, Exception> {
    let Some(info) = user_info(host, user)? else {
        return Ok(None);
    };
    if info.profile_group_id == USER_NULL || info.profile_group_id == info.id {
        return Ok(None);
    }
    Ok(user_info(host, info.profile_group_id)?.map(|parent| parent.id))
}

/// `UserInfo.isDemo` of the user; false for one that does not exist.
pub(super) fn is_demo(host: &AppHost, user: i32) -> Result<bool, Exception> {
    Ok(user_flags(host, user)?.is_some_and(|f| f & FLAG_DEMO != 0))
}

/// `UserManager.isUserAdmin`: `UserInfo.isAdmin` of the user; false for
/// one that does not exist.
pub(super) fn is_admin(host: &AppHost, user: i32) -> Result<bool, Exception> {
    Ok(user_flags(host, user)?.is_some_and(|f| f & FLAG_ADMIN != 0))
}

/// The managed profiles among `UserManager.getProfiles(user)`: the user's
/// profiles that exist and are not partial, of the managed profile type
/// (`UserInfo.isManagedProfile`).
pub(super) fn managed_profiles(host: &AppHost, user: i32) -> Result<Vec<i32>, Exception> {
    let mut managed = Vec::new();
    for id in profile_ids(user) {
        if user_info(host, id)?
            .is_some_and(|u| u.user_type.as_deref() == Some(USER_TYPE_PROFILE_MANAGED))
        {
            managed.push(id);
        }
    }
    Ok(managed)
}

/// The users the package boot scan installs for (`Settings.getAllUsers`:
/// `getUsers(true, false, false)`, no partial user), with each one's
/// `isAdbInstallDisallowed` (`hasUserRestriction(DISALLOW_DEBUGGING_FEATURES)`).
/// `UserManagerService.getUsers(excludePartial=true, excludeDying,
/// excludePreCreated)`'s ids: the system user unless it is partial, or a
/// pre-created one when those are excluded; it is never dying.
pub(super) fn user_ids(host: &AppHost, include_pre_created: bool) -> Result<Vec<i32>, Exception> {
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let (info, _) = read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)?;
    Ok(
        if info.partial || (info.pre_created && !include_pre_created) {
            Vec::new()
        } else {
            vec![info.id]
        },
    )
}

/// What the package boot's context records of a user.
pub(super) struct ContextUser {
    pub id: i32,
    pub flags: i32,
    pub pre_created: bool,
    pub profile_group_id: i32,
}

/// `getUsers(true, false, false)`: the system user unless it is partial.
pub(super) fn boot_context_users(host: &AppHost) -> Result<Vec<ContextUser>, Exception> {
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let (info, _) = read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)?;
    Ok((!info.partial)
        .then_some(ContextUser {
            id: info.id,
            flags: info.flags,
            pre_created: info.pre_created,
            profile_group_id: info.profile_group_id,
        })
        .into_iter()
        .collect())
}

pub(super) fn scan_users(host: &AppHost) -> Result<Vec<crate::package::scan::User>, Exception> {
    if host.device_policies()?.owned() {
        return Err(host.unimplemented_named(
            "user",
            "a device or profile owner's user restrictions (#1521)",
        ));
    }
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let (info, _) = read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)?;
    if info.partial {
        return Ok(Vec::new());
    }
    Ok(vec![crate::package::scan::User {
        id: info.id,
        pre_created: info.pre_created,
        adb_install_disallowed: host
            .user_restrictions()?
            .contains(DISALLOW_DEBUGGING_FEATURES),
    }])
}

/// The flags of a user that exists and is not partial.
fn user_flags(host: &AppHost, user: i32) -> Result<Option<i32>, Exception> {
    Ok(user_info(host, user)?.map(|u| u.flags))
}

/// A user that exists and is not partial, as its file holds it.
fn user_info(host: &AppHost, user: i32) -> Result<Option<UserInfo>, Exception> {
    if user != SYSTEM_USER {
        return Ok(None);
    }
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let (info, _) = read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)?;
    Ok((!info.partial).then_some(info))
}

/// `UserManagerService.isSameProfileGroup` (`isSameProfileGroupNoChecks`):
/// one user, or two existing users of one profile group.
pub(super) fn same_profile_group(host: &AppHost, user: i32, other: i32) -> Result<bool, Exception> {
    if user == other {
        return Ok(true);
    }
    let group = |id| -> Result<Option<i32>, Exception> {
        Ok(user_info(host, id)?
            .map(|u| u.profile_group_id)
            .filter(|&g| g != USER_NULL))
    };
    Ok(match (group(user)?, group(other)?) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    })
}

/// What `UserDataPreparer` and `reconcileAppsData` read of a user's
/// `UserInfo`: its serial number, `lastLoggedInTime` and
/// `lastLoggedInFingerprint`; none for a user that does not exist.
pub(super) fn storage_user(
    host: &AppHost,
    user: i32,
) -> Result<Option<(i32, i64, Option<String>)>, Exception> {
    if user != SYSTEM_USER {
        return Ok(None);
    }
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let (info, _) = read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)?;
    Ok(Some((
        info.serial_number,
        info.last_logged_in_time,
        info.last_logged_in_fingerprint,
    )))
}

/// `UserManagerService.onUserLoggedIn` at the user's unlock: its
/// `lastLoggedInTime` (once the clock is past
/// `EPOCH_PLUS_30_YEARS`) and `lastLoggedInFingerprint`, written as
/// `writeUserLP` writes them.
pub(super) fn user_logged_in(
    host: &AppHost,
    user: i32,
    now: i64,
    fingerprint: &str,
) -> Result<(), Exception> {
    const EPOCH_PLUS_30_YEARS: i64 = 30 * 365 * 24 * 60 * 60 * 1000;
    if user != SYSTEM_USER {
        return Ok(());
    }
    let (path, bytes) = host.user_file()?;
    let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
    let mut root = aim_android_xml::read(&bytes).map_err(state)?;
    if read_user(&root).map_err(state)?.0.partial {
        return Ok(());
    }
    if now > EPOCH_PLUS_30_YEARS {
        set_attr(&mut root, "lastLoggedIn", None, XmlValue::Long(now));
    }
    set_attr(
        &mut root,
        "lastLoggedInFingerprint",
        Some("lastLoggedIn"),
        XmlValue::String(fingerprint.into()),
    );
    super::shortcut_service::write_resilient(&path, std::slice::from_ref(&root), USER_FILE_MODE)
        .map_err(|e| state(e.to_string()))
}

/// `name`'s value on `e`, in place, or added after `after` (else last).
fn set_attr(e: &mut Element, name: &str, after: Option<&str>, value: XmlValue) {
    if let Some(attr) = e.attrs.iter_mut().find(|(n, _)| n == name) {
        attr.1 = value;
        return;
    }
    let at = after
        .and_then(|after| e.attrs.iter().position(|(n, _)| n == after))
        .map_or(e.attrs.len(), |i| i + 1);
    e.attrs.insert(at, (name.into(), value));
}

/// The users directory (`rwxrwxr-x`), the system user's file, then the
/// user list, each as `ResilientAtomicFile` writes it.
fn write_users(users: &Path, user: &Element, list: &Element) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("{}: {e}", users.display());
    if !users.exists() {
        std::fs::create_dir_all(users).map_err(failed)?;
        aim_storage::guest_inode::record(
            users,
            aim_storage::guest_inode::GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o775),
            },
        )
        .map_err(failed)?;
    }
    let write = |name: &str, root: &Element| {
        super::shortcut_service::write_resilient(
            &users.join(name),
            std::slice::from_ref(root),
            USER_FILE_MODE,
        )
        .map_err(failed)
    };
    write("0.xml", user)?;
    write("userlist.xml", list)
}

fn element(name: &str, attrs: Vec<(&str, XmlValue)>, children: Vec<Element>) -> Element {
    Element {
        name: name.into(),
        attrs: attrs.into_iter().map(|(n, v)| (n.to_string(), v)).collect(),
        content: children.into_iter().map(Node::Element).collect(),
    }
}

pub struct UserManager {
    host: Arc<AppHost>,
    /// `mUserTypes`, made once as the constructor makes them.
    types: std::sync::OnceLock<Vec<UserType>>,
}

impl UserManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        UserManager {
            host,
            types: std::sync::OnceLock::new(),
        }
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    fn feature(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.features.iter().any(|(n, _)| n == name))
    }

    /// `Build.IS_DEBUGGABLE`.
    fn debuggable(&self) -> bool {
        self.host.property("ro.debuggable").as_deref() == Some("1")
    }

    /// `SystemProperties.getInt(name, default)`.
    fn int_property(&self, name: &str, default: i32) -> i32 {
        self.host
            .property(name)
            .and_then(|v| v.trim_start().parse().ok())
            .unwrap_or(default)
    }

    /// `UserTypeFactory.getUserTypes` over the device's
    /// `config_user_types`.
    fn user_types(&self) -> Result<&[UserType], Exception> {
        if let Some(types) = self.types.get() {
            return Ok(types);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("user types: {e}"));
        let packages = &self.host.inputs()?.packages;
        let communal = super::system_boolean(
            self.host
                .property("persist.fw.omnipresent_communal_user")
                .as_deref(),
            packages
                .framework_bool("config_omnipresentCommunalUser")
                .unwrap_or(false),
        );
        // `UserManager.isPrivateProfileEnabled`: not on a low-RAM device
        // once private space creation is blocked there.
        let low_ram = self.host.property("ro.config.low_ram").as_deref() == Some("true");
        let private = !self.flag(BLOCK_PRIVATE_SPACE_CREATION)? || !low_ram;
        let facts = user_types::Facts {
            debuggable: self.debuggable(),
            communal,
            private,
            supervising: self.flag(ALLOW_SUPERVISING_PROFILE)?,
        };
        let customize = packages
            .framework_file(self.host.root()?, "xml", "config_user_types")
            .map(|bytes| aim_apps::res::xml(&bytes).map_err(|e| state(e.to_string())))
            .transpose()?;
        let types = user_types::user_types(&facts, customize.as_ref()).map_err(state)?;
        Ok(self.types.get_or_init(|| types))
    }

    /// `mUserTypes.get(name)`.
    fn user_type(&self, name: Option<&str>) -> Result<Option<&UserType>, Exception> {
        let types = self.user_types()?;
        Ok(name.and_then(|name| types.iter().find(|t| t.name == name)))
    }

    /// What the limits read at a call: `getMaxSupportedUsers`,
    /// `isCreationOverrideEnabled`, FEATURE_MANAGED_USERS and
    /// `Flags.profilesForAll`.
    fn limits(&self) -> Result<user_types::Limits, Exception> {
        let max_users = max_supported_users(&self.host)?;
        Ok(user_types::Limits {
            max_users,
            creation_override: self.debuggable()
                && super::system_boolean(
                    self.host.property(DEV_CREATE_OVERRIDE_PROPERTY).as_deref(),
                    false,
                ),
            managed_users: self.feature(FEATURE_MANAGED_USERS)?,
            profiles_for_all: self.flag(PROFILES_FOR_ALL)?,
        })
    }

    /// `mUsers`: the system user.
    fn users(&self) -> Result<Vec<UserInfo>, Exception> {
        Ok(vec![self.user_data()?.0])
    }

    /// `getRemainingCreatableProfileCount(type, user, allowedToRemoveOne)`.
    fn remaining_profiles(
        &self,
        uid: i32,
        user_type: Option<&str>,
        user: i32,
        allowed_to_remove_one: bool,
    ) -> Result<i32, Exception> {
        self.check_query_or_create(
            uid,
            "get the remaining number of profiles that can be added to the given user.",
        )?;
        let t = self.user_type(user_type)?;
        // `getMaxUsersOfTypePerParent`: `persist.sys.max_profiles` for a
        // managed profile on a debuggable build.
        let managed_per_parent = t
            .filter(|t| self.debuggable() && t.managed_profile())
            .map(|t| self.int_property("persist.sys.max_profiles", t.max_allowed_per_parent));
        Ok(user_types::remaining_profiles(
            t,
            &self.users()?,
            user,
            allowed_to_remove_one,
            &self.limits()?,
            managed_per_parent,
        ))
    }

    /// `canAddMoreProfilesToUser`.
    fn can_add_more_profiles(
        &self,
        uid: i32,
        user_type: Option<&str>,
        user: i32,
        allowed_to_remove_one: bool,
    ) -> Result<bool, Exception> {
        Ok(
            0 < self.remaining_profiles(uid, user_type, user, allowed_to_remove_one)?
                || self.limits()?.creation_override,
        )
    }

    /// `checkCreateUsersPermission(message)`.
    fn check_create(&self, uid: i32, message: &str) -> Result<(), Exception> {
        if self.manage_or(uid, Some(CREATE_USERS))? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "You either need MANAGE_USERS or CREATE_USERS permission to: {message}"
        )))
    }

    /// `checkManageOrInteractPermissionIfCallerInOtherProfileGroup`.
    fn check_manage_or_interact_other(
        &self,
        uid: i32,
        user: i32,
        name: &str,
    ) -> Result<(), Exception> {
        let caller = uid / PER_USER_RANGE as i32;
        let same_group = match (self.user(caller)?, self.user(user)?) {
            (Some(a), Some(b)) => a.same_profile_group(&b),
            _ => false,
        };
        if caller == user || same_group || self.manage_or(uid, Some(INTERACT_ACROSS_USERS))? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "You need INTERACT_ACROSS_USERS or MANAGE_USERS permission to: check {name}"
        )))
    }

    /// User 0's file: its `UserInfo` and effective restrictions.
    fn user_data(&self) -> Result<(UserInfo, BTreeSet<String>), Exception> {
        let (path, bytes) = self.host.user_file()?;
        let state =
            |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
        read_user(&aim_android_xml::read(&bytes).map_err(state)?).map_err(state)
    }

    /// `getUserDataLU(id)`'s seed account (`seedAccountName`,
    /// `seedAccountType`), and whether it has options; none of a user that
    /// does not exist or is partial.
    fn seed_account(
        &self,
        id: i32,
    ) -> Result<Option<(Option<String>, Option<String>, bool)>, Exception> {
        if self.user(id)?.is_none() {
            return Ok(None);
        }
        let (path, bytes) = self.host.user_file()?;
        let state =
            |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
        let root = aim_android_xml::read(&bytes).map_err(state)?;
        let string = |name: &str| root.string(name).map(|s| s.into_owned());
        Ok(Some((
            string("seedAccountName"),
            string("seedAccountType"),
            root.children().any(|e| e.name == "seedAccountOptions"),
        )))
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
            Some(u) if u.profile_group_id != id && u.profile_group_id != USER_NULL => {
                self.user(u.profile_group_id)
            }
            _ => Ok(None),
        }
    }

    /// `userWithName`: the owner's name for the system or main user, the
    /// guest's for a guest, when it has none.
    fn with_name(&self, user: Option<UserInfo>) -> Result<Option<UserInfo>, Exception> {
        let Some(mut user) = user else {
            return Ok(None);
        };
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
                user.name = Some(name.ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, format!("no framework string {resource}"))
                })?);
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
            || alternative
                .map(|p| self.granted(uid, p))
                .transpose()?
                .unwrap_or(false))
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
        Err(Exception::security(format!(
            "You either need MANAGE_USERS, CREATE_USERS, or QUERY_USERS permission to: {message}"
        )))
    }

    /// `checkQueryOrCreateUsersPermissionIfCallerInOtherProfileGroup`.
    fn check_query_or_create_other(
        &self,
        uid: i32,
        user: i32,
        name: &str,
    ) -> Result<(), Exception> {
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
            um::GET_SEED_ACCOUNT_NAME => {
                let a = um::GetSeedAccountName::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, None),
                    "You need MANAGE_USERS permission to: Cannot get seed account information"
                        .into(),
                )
                .and_then(|_| self.seed_account(a.user_id))
                .map(|s| um::write_get_seed_account_name_reply(p, &s.and_then(|s| s.0)))
            }
            um::GET_SEED_ACCOUNT_TYPE => {
                let a = um::GetSeedAccountType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, None),
                    "You need MANAGE_USERS permission to: Cannot get seed account information"
                        .into(),
                )
                .and_then(|_| self.seed_account(a.user_id))
                .map(|s| um::write_get_seed_account_type_reply(p, &s.and_then(|s| s.1)))
            }
            um::GET_SEED_ACCOUNT_OPTIONS => {
                let a = um::GetSeedAccountOptions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, None),
                    "You need MANAGE_USERS permission to: Cannot get seed account information"
                        .into(),
                )
                .and_then(|_| self.seed_account(a.user_id))
                .and_then(|s| match s {
                    Some((_, _, true)) => Err(self
                        .host
                        .unimplemented_named("user", "a seed account's options (#1520)")),
                    _ => Ok(um::write_get_seed_account_options_reply::<
                        super::bind::Bundle,
                    >(p, None)),
                })
            }
            // `someUserHasSeedAccountNoChecks`: only an uninitialized user
            // keeps a seed account to check, and user 0 is initialized.
            um::SOME_USER_HAS_SEED_ACCOUNT => {
                um::SomeUserHasSeedAccount::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, None),
                    "You need MANAGE_USERS permission to: check seed account information".into(),
                )
                .and_then(|_| self.user(SYSTEM_USER))
                .and_then(|u| match u {
                    Some(u) if u.flags & FLAG_INITIALIZED == 0 => {
                        Err(self.host.unimplemented_named(
                            "user",
                            "an uninitialized user's seed account (#1520)",
                        ))
                    }
                    _ => Ok(um::write_some_user_has_seed_account_reply(p, false)),
                })
            }
            um::GET_PRIMARY_USER => {
                um::GetPrimaryUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, None),
                    "You need MANAGE_USERS permission to: query users".into(),
                )
                .and_then(|_| self.user(SYSTEM_USER))
                .map(|u| {
                    um::write_get_primary_user_reply(
                        p,
                        u.filter(|u| u.flags & FLAG_PRIMARY != 0).as_ref(),
                    )
                })
            }
            um::GET_MAIN_USER_ID => {
                um::GetMainUserId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // Partial users count.
                self.check_query_or_create(uid, "get main user id")
                    .and_then(|_| self.user_data())
                    .map(|(u, _)| {
                        um::write_get_main_user_id_reply(
                            p,
                            if u.flags & FLAG_MAIN != 0 {
                                u.id
                            } else {
                                USER_NULL
                            },
                        )
                    })
            }
            um::GET_USERS => {
                let a = um::GetUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                require(
                    self.manage_or(uid, Some(CREATE_USERS)),
                    "You either need MANAGE_USERS or CREATE_USERS permission to: query users"
                        .into(),
                )
                .and_then(|_| self.user_data())
                .and_then(|(u, _)| {
                    let listed =
                        !(a.exclude_partial && u.partial || a.exclude_pre_created && u.pre_created);
                    let users: Vec<_> = if listed {
                        vec![self.with_name(Some(u))?]
                    } else {
                        Vec::new()
                    };
                    Ok(um::write_get_users_reply(p, Some(&users)))
                })
            }
            um::CAN_ADD_PRIVATE_PROFILE => {
                let a = um::CanAddPrivateProfile::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.check_create(uid, "canHaveRestrictedProfile")?;
                    let parent = self.user(a.user_id)?;
                    let limits = self.limits()?;
                    let enabled = self
                        .user_type(Some(user_types::PROFILE_PRIVATE))?
                        .is_some_and(|t| user_types::type_enabled(t, &limits));
                    // `doesDeviceHardwareSupportPrivateSpace`.
                    let hardware = !(self.feature(FEATURE_EMBEDDED)?
                        || self.feature(FEATURE_WATCH)?
                        || self.feature(FEATURE_LEANBACK)?
                        || self.feature(FEATURE_AUTOMOTIVE)?);
                    let can = enabled
                        && self.can_add_more_profiles(
                            uid,
                            Some(user_types::PROFILE_PRIVATE),
                            a.user_id,
                            false,
                        )?
                        && parent.is_some_and(|u| u.flags & FLAG_MAIN == FLAG_MAIN)
                        && hardware
                        && !(a.user_id == SYSTEM_USER
                            && self
                                .host
                                .user_restrictions()?
                                .contains(DISALLOW_ADD_PRIVATE_PROFILE));
                    um::write_can_add_private_profile_reply(p, can);
                    Ok(())
                })()
            }
            um::IS_USER_TYPE_ENABLED => {
                let a = um::IsUserTypeEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.check_create(uid, "check if user type is enabled.")?;
                    let limits = self.limits()?;
                    let enabled = self
                        .user_type(a.user_type.as_deref())?
                        .is_some_and(|t| user_types::type_enabled(t, &limits));
                    um::write_is_user_type_enabled_reply(p, enabled);
                    Ok(())
                })()
            }
            um::CAN_ADD_MORE_USERS_OF_TYPE => {
                let a = um::CanAddMoreUsersOfType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.check_create(uid, "check if more users can be added.")?;
                    let (users, limits) = (self.users()?, self.limits()?);
                    let can = self.user_type(a.user_type.as_deref())?.is_some_and(|t| {
                        user_types::can_add_more_users_of_type(t, &users, &limits)
                    });
                    um::write_can_add_more_users_of_type_reply(p, can);
                    Ok(())
                })()
            }
            um::GET_REMAINING_CREATABLE_USER_COUNT => {
                let a = um::GetRemainingCreatableUserCount::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    self.check_query_or_create(
                        uid,
                        "get the remaining number of users that can be added.",
                    )?;
                    let count = user_types::remaining_users(
                        self.user_type(a.user_type.as_deref())?,
                        &self.users()?,
                        &self.limits()?,
                    );
                    um::write_get_remaining_creatable_user_count_reply(p, count);
                    Ok(())
                })()
            }
            um::GET_REMAINING_CREATABLE_PROFILE_COUNT => {
                let a = um::GetRemainingCreatableProfileCount::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.remaining_profiles(uid, a.user_type.as_deref(), a.user_id, false)
                    .map(|count| um::write_get_remaining_creatable_profile_count_reply(p, count))
            }
            um::CAN_ADD_MORE_PROFILES_TO_USER => {
                let a =
                    um::CanAddMoreProfilesToUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.can_add_more_profiles(
                    uid,
                    a.user_type.as_deref(),
                    a.user_id,
                    a.allowed_to_remove_one,
                )
                .map(|can| um::write_can_add_more_profiles_to_user_reply(p, can))
            }
            um::CAN_ADD_MORE_MANAGED_PROFILES => {
                let a =
                    um::CanAddMoreManagedProfiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.can_add_more_profiles(
                    uid,
                    Some(user_types::PROFILE_MANAGED),
                    a.user_id,
                    a.allowed_to_remove_one,
                )
                .map(|can| um::write_can_add_more_managed_profiles_reply(p, can))
            }
            um::HAS_BADGE => {
                let a = um::HasBadge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.check_manage_or_interact_other(uid, a.user_id, "hasBadge")?;
                    let user = self.user(a.user_id)?;
                    let badged = self
                        .user_type(user.and_then(|u| u.user_type).as_deref())?
                        .is_some_and(|t| t.has_badge);
                    um::write_has_badge_reply(p, badged);
                    Ok(())
                })()
            }
            um::GET_PROFILES => {
                let a = um::GetProfiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let full = if a.user_id != caller {
                    self.check_query_or_create(
                        uid,
                        &format!("getting profiles related to user {}", a.user_id),
                    )
                    .map(|_| true)
                } else {
                    self.manage_or(uid, Some(CREATE_USERS))
                };
                // `getProfilesLU`: the user's profile group, user 0 alone;
                // without full information, no name or icon.
                full.and_then(|full| {
                    let user = self
                        .user(a.user_id)?
                        .filter(|u| !a.enabled_only || u.flags & FLAG_DISABLED == 0);
                    let user = if full {
                        self.with_name(user)?
                    } else {
                        user.map(|u| UserInfo {
                            name: None,
                            icon_path: None,
                            ..u
                        })
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
                require(
                    self.manage_or(uid, None),
                    "You need MANAGE_USERS permission to: get the profile parent".into(),
                )
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
                self.check_query_or_create(uid, "check user type")
                    .and_then(|_| self.user(a.user_id))
                    .map(|u| {
                        let of_type =
                            a.user_type.is_some() && u.and_then(|u| u.user_type) == a.user_type;
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
                    self.profile_parent(a.user_id)
                        .and_then(|parent| match parent {
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
                    self.check_query_or_create(
                        uid,
                        &format!("query isRestricted for user {}", a.user_id),
                    )
                } else {
                    Ok(())
                };
                checked.and_then(|_| self.user(a.user_id)).map(|u| {
                    let restricted = u
                        .is_some_and(|u| u.user_type.as_deref() == Some(USER_TYPE_FULL_RESTRICTED));
                    um::write_is_restricted_reply(p, restricted)
                })
            }
            um::GET_USER_SERIAL_NUMBER => {
                let a = um::GetUserSerialNumber::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.user(a.user_id).map(|u| {
                    um::write_get_user_serial_number_reply(p, u.map_or(-1, |u| u.serial_number))
                })
            }
            // A copy of the effective restrictions: none of a user that does
            // not exist.
            um::GET_USER_RESTRICTIONS => {
                let a = um::GetUserRestrictions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_other_user(call, a.user_id, false, "getUserRestrictions")
                    .and_then(|_| {
                        if a.user_id == SYSTEM_USER {
                            Ok(self.user_data()?.1)
                        } else {
                            Ok(BTreeSet::new())
                        }
                    })
                    .map(|set| {
                        let entries = set.into_iter().map(|k| (k, Value::Bool(true))).collect();
                        let entries = crate::package::info::array_order(entries, |(k, _)| k);
                        um::write_get_user_restrictions_reply(
                            p,
                            Some(&super::bind::Bundle(entries)),
                        )
                    })
            }
            um::IS_ADMIN_USER => {
                let a = um::IsAdminUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_query_or_create_other(uid, a.user_id, "isAdminUser")
                    .and_then(|_| self.user(a.user_id))
                    .map(|u| {
                        um::write_is_admin_user_reply(
                            p,
                            u.is_some_and(|u| u.flags & FLAG_ADMIN != 0),
                        )
                    })
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
                let held = if caller == a.user_id {
                    Ok(true)
                } else {
                    self.manage_or(uid, Some(INTERACT_ACROSS_USERS))
                };
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
                self.user(SYSTEM_USER).map(|u| {
                    um::write_is_foreground_user_admin_reply(
                        p,
                        u.is_some_and(|u| u.flags & FLAG_ADMIN != 0),
                    )
                })
            }
            um::IS_USER_NAME_SET => {
                let a = um::IsUserNameSet::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let held = self.query_or_create(uid).and_then(|q| {
                    Ok(q || caller == a.user_id && self.granted(uid, GET_ACCOUNTS_PRIVILEGED)?)
                });
                require(held, "You need MANAGE_USERS, CREATE_USERS, QUERY_USERS, or GET_ACCOUNTS_PRIVILEGED permissions to: get whether user name is set".into())
                    .and_then(|_| self.user(a.user_id))
                    .map(|u| um::write_is_user_name_set_reply(p, u.is_some_and(|u| u.name.is_some())))
            }
            // The caller's user's name, or "".
            um::GET_USER_NAME => {
                um::GetUserName::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let held = self
                    .query_or_create(uid)
                    .and_then(|q| Ok(q || self.granted(uid, GET_ACCOUNTS_PRIVILEGED)?));
                require(held, "You need MANAGE_USERS, CREATE_USERS, QUERY_USERS, or GET_ACCOUNTS_PRIVILEGED permissions to: get user name".into())
                    .and_then(|_| self.with_name(self.user(caller)?))
                    .map(|u| um::write_get_user_name_reply(p, &Some(u.and_then(|u| u.name).unwrap_or_default())))
            }
            code => Err(self.host.unimplemented("user", self.descriptor(), code)),
        })
    }

    /// `checkManageOrInteractPermissionIfCallerInOtherProfileGroup`, or with
    /// `query` its `checkQueryOrInteract…` form: another user than the
    /// caller's (user 0 has no profiles) takes root or system, MANAGE_USERS
    /// (or QUERY_USERS), or INTERACT_ACROSS_USERS.
    fn check_other_user(
        &self,
        call: &Call<'_>,
        user: i32,
        query: bool,
        name: &str,
    ) -> Result<(), Exception> {
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
            format!(
                "You need INTERACT_ACROSS_USERS, MANAGE_USERS, or QUERY_USERS permission to: check {name}"
            )
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
                let args =
                    um::IsUserUnlockingOrUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) =
                    self.check_other_user(call, args.user_id, false, "isUserUnlockingOrUnlocked")
                {
                    return reply(Err(e));
                }
                match self.host.user_unlocking_or_unlocked(args.user_id) {
                    Ok(unlocked) => um::write_is_user_unlocking_or_unlocked_reply(&mut p, unlocked),
                    Err(e) => return reply(Err(e)),
                }
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
                let args =
                    um::GetCredentialOwnerProfile::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                um::write_get_credential_owner_profile_reply(&mut p, args.user_id);
            }
            // User 0 is a full user, no profile: "" (a user that does not
            // exist has none).
            um::GET_PROFILE_TYPE => {
                let args = um::GetProfileType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, true, "getProfileType") {
                    return reply(Err(e));
                }
                um::write_get_profile_type_reply(
                    &mut p,
                    &(args.user_id == SYSTEM_USER).then(String::new),
                );
            }
            um::GET_USER_PROPERTIES_COPY => {
                let args =
                    um::GetUserPropertiesCopy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.check_other_user(call, args.user_id, true, "getUserProperties")
                {
                    return reply(Err(e));
                }
                if args.user_id != SYSTEM_USER {
                    return reply(Err(Exception::illegal_argument(format!(
                        "Cannot access properties for user {}",
                        args.user_id
                    ))));
                }
                let uid = call.sender_euid as i32;
                let manage = matches!(uid % 100_000, 1000)
                    || uid == 0
                    || matches!(
                        self.host
                            .check_permission(Some("android.permission.MANAGE_USERS"), uid),
                        Ok(0)
                    );
                match self.host.user_properties() {
                    Ok(persisted) => um::write_get_user_properties_copy_reply(
                        &mut p,
                        Some(&UserProperties::copy(&persisted, uid == 1000, manage)),
                    ),
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
                let args =
                    um::HasBaseUserRestriction::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                let held =
                    |perm: &str| matches!(self.host.check_permission(Some(perm), uid), Ok(0));
                if !matches!(uid % 100_000, 0 | 1000)
                    && !held("android.permission.MANAGE_USERS")
                    && !held("android.permission.CREATE_USERS")
                {
                    return reply(Err(Exception::security(
                        "You either need MANAGE_USERS or CREATE_USERS permission to: hasBaseUserRestriction",
                    )));
                }
                let restricted = match self.host.user_restrictions() {
                    Ok(r) => {
                        args.user_id == SYSTEM_USER
                            && args.restriction_key.is_some_and(|k| r.contains(&k))
                    }
                    Err(e) => return reply(Err(e)),
                };
                um::write_has_base_user_restriction_reply(&mut p, restricted);
            }
            // `getUserRestrictionSources`: none for a restriction not in
            // effect; the sources of one in effect are #1709.
            um::GET_USER_RESTRICTION_SOURCES => {
                let args =
                    um::GetUserRestrictionSources::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                let held =
                    |perm: &str| matches!(self.host.check_permission(Some(perm), uid), Ok(0));
                if !matches!(uid % 100_000, 0 | 1000)
                    && !held("android.permission.MANAGE_USERS")
                    && !held("android.permission.QUERY_USERS")
                {
                    return reply(Err(Exception::security(
                        "You either need MANAGE_USERS or QUERY_USERS permission to call getUserRestrictionSources.",
                    )));
                }
                let restricted = match self.host.user_restrictions() {
                    Ok(r) => {
                        args.user_id == SYSTEM_USER
                            && args.restriction_key.is_some_and(|k| r.contains(&k))
                    }
                    Err(e) => return reply(Err(e)),
                };
                if restricted {
                    return reply(Err(self.host.unimplemented_named(
                        "user",
                        "getUserRestrictionSources of a restriction in effect (#1709)",
                    )));
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
                let restricted = match self.host.has_user_restriction(
                    args.restriction_key.as_deref().unwrap_or(""),
                    args.user_id,
                ) {
                    Ok(restricted) => restricted,
                    Err(e) => return reply(Err(e)),
                };
                um::write_has_user_restriction_reply(&mut p, restricted);
            }
            // The user of a serial number, -1 for none.
            um::GET_USER_HANDLE => {
                let args = um::GetUserHandle::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.user_serial() {
                    Ok(serial) => um::write_get_user_handle_reply(
                        &mut p,
                        if serial == args.user_serial_number {
                            SYSTEM_USER
                        } else {
                            -1
                        },
                    ),
                    Err(e) => return reply(Err(e)),
                }
            }
            um::IS_DEMO_USER => {
                let args = um::IsDemoUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                if args.user_id != uid / 100_000
                    && !matches!(uid % 100_000, 0 | 1000)
                    && self
                        .host
                        .check_permission(Some("android.permission.MANAGE_USERS"), uid)
                        .map_err(|_| BAD_VALUE)?
                        != 0
                {
                    return reply(Err(Exception::security(format!(
                        "You need MANAGE_USERS permission to query if u={} is a demo user",
                        args.user_id
                    ))));
                }
                let demo = self
                    .host
                    .property("ro.boot.arc_demo_mode")
                    .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "y" | "yes" | "on"))
                    || args.user_id == SYSTEM_USER
                        && matches!(self.host.user_flags(), Ok(f) if f & FLAG_DEMO != 0);
                um::write_is_demo_user_reply(&mut p, demo);
            }
            // The system user is not full.
            um::IS_HEADLESS_SYSTEM_USER_MODE => {
                um::IsHeadlessSystemUserMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.user_flags() {
                    Ok(flags) => {
                        um::write_is_headless_system_user_mode_reply(&mut p, flags & FLAG_FULL == 0)
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            // `readApplicationRestrictionsLAr`: none without the file.
            um::GET_APPLICATION_RESTRICTIONS_FOR_USER => {
                let args = um::GetApplicationRestrictionsForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let package = args.package_name.unwrap_or_default();
                match application_restrictions(
                    &self.host,
                    call.sender_euid as i32,
                    &package,
                    args.user_id,
                ) {
                    Ok(restrictions) => um::write_get_application_restrictions_for_user_reply(
                        &mut p,
                        Some(&restrictions),
                    ),
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

/// `UserManagerService.getApplicationRestrictionsForUser`: the caller's
/// own, or any for the system; those persisted (`res_<package>.xml`):
/// #1519.
pub(super) fn application_restrictions(
    host: &AppHost,
    uid: i32,
    package: &str,
    user: i32,
) -> Result<super::bind::Bundle, Exception> {
    let same_app = host.owns_package(uid, package).unwrap_or(false);
    if (uid / 100_000 != user || !same_app) && !matches!(uid % 100_000, 0 | 1000) {
        return Err(Exception::security(format!(
            "get application restrictions for other user/app {package} is only allowed for the system"
        )));
    }
    if host
        .data()?
        .join(format!("system/users/{user}/res_{package}.xml"))
        .exists()
    {
        return Err(host.unimplemented_named("user", "an app's persisted restrictions (#1519)"));
    }
    Ok(super::bind::Bundle(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fallbackToSingleUserLP`'s records of a full system user, written
    /// and read back: the original's first boot's attributes, the guests'
    /// defaults in their `ArrayMap` order in the system user's file, and
    /// the list's serial number and versions.
    #[test]
    fn the_first_boot_records_the_system_user_as_the_original() {
        let full = FLAG_SYSTEM | FLAG_FULL | FLAG_PRIMARY | FLAG_ADMIN | FLAG_MAIN;
        let (user, list) = first_user_records(USER_TYPE_FULL_SYSTEM, full, 0, true);
        let user = aim_android_xml::read(&aim_android_xml::abx::write(&user).unwrap()).unwrap();
        let (info, restrictions) = read_user(&user).unwrap();
        assert_eq!((info.id, info.serial_number, info.flags), (0, 0, 19475));
        assert_eq!(info.user_type.as_deref(), Some(USER_TYPE_FULL_SYSTEM));
        assert!(restrictions.is_empty());
        let guests: Vec<&str> = user
            .children()
            .find(|e| e.name == "guestRestrictions")
            .and_then(|g| g.children().next())
            .map(|r| r.attrs.iter().map(|(n, _)| n.as_str()).collect())
            .unwrap();
        assert_eq!(
            guests,
            [
                "no_sms",
                "no_install_unknown_sources",
                "no_config_wifi",
                "no_config_credentials",
                "no_outgoing_calls"
            ]
        );
        let names: Vec<&str> = user.children().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "restrictions",
                "device_policy_global_restrictions",
                "guestRestrictions",
                "device_policy_local_restrictions",
                "userProperties",
                "ignorePrepareStorageErrors"
            ]
        );
        assert_eq!(
            list.attrs,
            [
                ("nextSerialNumber".to_string(), XmlValue::Int(10)),
                ("version".to_string(), XmlValue::Int(11)),
                ("userTypeConfigVersion".to_string(), XmlValue::Int(0)),
            ]
        );
        assert_eq!(
            list.children().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["user"]
        );
        // Without the flag the guests' defaults are the list's.
        let (user, list) = first_user_records(USER_TYPE_FULL_SYSTEM, full, 0, false);
        assert!(user.children().all(|e| e.name != "guestRestrictions"));
        assert_eq!(list.children().next().unwrap().name, "guestRestrictions");
    }

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
        assert_eq!(
            (info.profile_group_id, info.restricted_profile_parent_id),
            (USER_NULL, USER_NULL)
        );
        assert_eq!(info.last_logged_in_time, 1792332278199);
        assert_eq!(info.name.as_deref(), Some("Kim"));
        assert_eq!(info.icon_path, None);
        assert_eq!(
            restrictions.into_iter().collect::<Vec<_>>(),
            ["no_config_wifi", "no_sms"]
        );
    }
}
