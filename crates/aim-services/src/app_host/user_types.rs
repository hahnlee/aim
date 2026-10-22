//! The user types (`UserTypeFactory`, `UserTypeDetails` at
//! `android-16.0.0_r1`): the AOSP types with their limits and whether they
//! are enabled, as the device's `config_user_types` customizes them, and
//! the limits UserManagerService derives from them and the users that
//! exist (`canAddMoreUsersOfType`, `getRemainingCreatableUserCount`,
//! `getRemainingCreatableProfileCount`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_apps::res::{Element, Value};

use super::user_service::UserInfo;

/// `UserTypeDetails.UNLIMITED_NUMBER_OF_USERS`.
pub(super) const UNLIMITED: i32 = -1;
/// `UserInfo.FLAG_FULL`, `FLAG_SYSTEM`, `FLAG_PROFILE`, `FLAG_MAIN`.
const FLAG_FULL: i32 = 0x400;
const FLAG_SYSTEM: i32 = 0x800;
const FLAG_PROFILE: i32 = 0x1000;
const FLAG_MAIN: i32 = 0x4000;
/// `UserInfo.FLAG_DEMO`.
const FLAG_DEMO: i32 = 0x200;

/// `UserManager.USER_TYPE_*`.
pub(super) const PROFILE_MANAGED: &str = "android.os.usertype.profile.MANAGED";
pub(super) const PROFILE_PRIVATE: &str = "android.os.usertype.profile.PRIVATE";
const PROFILE_CLONE: &str = "android.os.usertype.profile.CLONE";
const PROFILE_COMMUNAL: &str = "android.os.usertype.profile.COMMUNAL";
const PROFILE_SUPERVISING: &str = "android.os.usertype.profile.SUPERVISING";
const PROFILE_TEST: &str = "android.os.usertype.profile.TEST";
const FULL_SYSTEM: &str = "android.os.usertype.full.SYSTEM";
const FULL_SECONDARY: &str = "android.os.usertype.full.SECONDARY";
const FULL_GUEST: &str = "android.os.usertype.full.GUEST";
const FULL_DEMO: &str = "android.os.usertype.full.DEMO";
const FULL_RESTRICTED: &str = "android.os.usertype.full.RESTRICTED";
const SYSTEM_HEADLESS: &str = "android.os.usertype.system.HEADLESS";

/// What of a type the limits read (`UserTypeDetails`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct UserType {
    pub name: String,
    /// `getBaseType`: FLAG_FULL, FLAG_PROFILE, FLAG_SYSTEM or a mix.
    pub base_type: i32,
    pub enabled: bool,
    pub max_allowed: i32,
    pub max_allowed_per_parent: i32,
    /// `hasBadge`: an icon badge other than `Resources.ID_NULL`.
    pub has_badge: bool,
}

impl UserType {
    fn new(name: &str, base_type: i32) -> Self {
        // The Builder's defaults.
        UserType {
            name: name.into(),
            base_type,
            enabled: true,
            max_allowed: UNLIMITED,
            max_allowed_per_parent: UNLIMITED,
            has_badge: false,
        }
    }

    fn max(mut self, max_allowed: i32) -> Self {
        self.max_allowed = max_allowed;
        self
    }

    fn per_parent(mut self, max: i32) -> Self {
        self.max_allowed_per_parent = max;
        self
    }

    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn badged(mut self) -> Self {
        self.has_badge = true;
        self
    }

    /// `isManagedProfile`.
    pub fn managed_profile(&self) -> bool {
        self.name == PROFILE_MANAGED
    }
}

/// What the default types depend on.
pub(super) struct Facts {
    /// `Build.IS_DEBUGGABLE`, which adds the test profile type.
    pub debuggable: bool,
    /// `UserManager.isCommunalProfileEnabled`.
    pub communal: bool,
    /// `UserManager.isPrivateProfileEnabled`.
    pub private: bool,
    /// `android.multiuser.Flags.allowSupervisingProfile`.
    pub supervising: bool,
}

/// `UserTypeFactory.getUserTypes`: the default types, as `customize`
/// (the root of `config_user_types`) changes them.
pub(super) fn user_types(
    facts: &Facts,
    customize: Option<&Element>,
) -> Result<Vec<UserType>, String> {
    let mut types = vec![
        UserType::new(PROFILE_MANAGED, FLAG_PROFILE)
            .per_parent(1)
            .badged(),
        UserType::new(FULL_SYSTEM, FLAG_SYSTEM | FLAG_FULL).max(1),
        UserType::new(FULL_SECONDARY, FLAG_FULL),
        UserType::new(FULL_GUEST, FLAG_FULL).max(1),
        UserType::new(FULL_DEMO, FLAG_FULL),
        UserType::new(FULL_RESTRICTED, FLAG_FULL),
        UserType::new(SYSTEM_HEADLESS, FLAG_SYSTEM).max(1),
        UserType::new(PROFILE_CLONE, FLAG_PROFILE)
            .per_parent(1)
            .badged(),
        UserType::new(PROFILE_COMMUNAL, FLAG_PROFILE)
            .max(1)
            .enabled(facts.communal)
            .badged(),
        UserType::new(PROFILE_PRIVATE, FLAG_PROFILE)
            .per_parent(1)
            .enabled(facts.private)
            .badged(),
        UserType::new(PROFILE_SUPERVISING, FLAG_PROFILE)
            .max(1)
            .enabled(facts.supervising),
    ];
    if facts.debuggable {
        types.push(
            UserType::new(PROFILE_TEST, FLAG_PROFILE)
                .per_parent(2)
                .badged(),
        );
    }
    if let Some(root) = customize {
        customize_builders(&mut types, root)?;
    }
    Ok(types)
}

/// `customizeBuilders`: `profile-type` and `full-type` elements change an
/// AOSP type (`android.*`) or define an OEM profile type; their `enabled`,
/// `max-allowed` and, for profiles, `max-allowed-per-parent` and
/// `icon-badge`. An attribute that is no integer throws, as
/// `Integer.parseInt` does.
fn customize_builders(types: &mut Vec<UserType>, root: &Element) -> Result<(), String> {
    if root.name != "user-types" {
        // `beginDocument`'s XmlPullParserException, which the original
        // logs and leaves the defaults.
        return Ok(());
    }
    for e in &root.children {
        let profile = match e.name.as_str() {
            "profile-type" => true,
            "full-type" => false,
            _ => continue,
        };
        let name = match e.named("name") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        let index = if name.starts_with("android.") {
            let i = types
                .iter()
                .position(|t| t.name == name)
                .ok_or_else(|| {
                    format!(
                        "Illegal custom user type name {name}: Non-AOSP user types cannot start with 'android.'"
                    )
                })?;
            let base = types[i].base_type;
            if !(profile && base == FLAG_PROFILE || !profile && base == FLAG_FULL) {
                return Err(format!(
                    "Wrong base type to customize user type ({name}), which is type {base:#x}"
                ));
            }
            i
        } else if profile {
            types.push(UserType::new(&name, FLAG_PROFILE));
            types.len() - 1
        } else {
            return Err(format!(
                "Creation of non-profile user type ({name}) is not currently supported."
            ));
        };
        let t = &mut types[index];
        if profile {
            if let Some(v) = int_attribute(e, "max-allowed-per-parent")? {
                t.max_allowed_per_parent = v;
            }
            if let Some(v) = e.named("icon-badge") {
                // `getAttributeResourceValue`, `ID_NULL` for no resource.
                t.has_badge = matches!(v, Value::Ref(id) if *id != 0);
            }
        }
        if let Some(v) = int_attribute(e, "enabled")? {
            t.enabled = v != 0;
        }
        if let Some(v) = int_attribute(e, "max-allowed")? {
            t.max_allowed = v;
        }
    }
    Ok(())
}

/// `setIntAttribute`.
fn int_attribute(e: &Element, name: &str) -> Result<Option<i32>, String> {
    match e.named(name) {
        None => Ok(None),
        Some(Value::Int(i)) => Ok(Some(*i)),
        Some(Value::String(s)) => s
            .parse()
            .map(Some)
            .map_err(|_| format!("Cannot parse value of '{s}' for {name}")),
        Some(v) => Err(format!("Cannot parse value of '{v:?}' for {name}")),
    }
}

/// What the limits read of the device besides the types and users.
pub(super) struct Limits {
    /// `UserManager.getMaxSupportedUsers`.
    pub max_users: i32,
    /// `isCreationOverrideEnabled`.
    pub creation_override: bool,
    /// `hasSystemFeature(FEATURE_MANAGED_USERS)`.
    pub managed_users: bool,
    /// `android.multiuser.Flags.profilesForAll`.
    pub profiles_for_all: bool,
}

/// `isUserTypeEnabled(UserTypeDetails)`.
pub(super) fn type_enabled(t: &UserType, limits: &Limits) -> bool {
    t.enabled || limits.creation_override
}

/// `getNumberOfUsersOfType`: alive users of the type, pre-created ones
/// not counted.
fn users_of_type(users: &[UserInfo], name: &str) -> i32 {
    users
        .iter()
        .filter(|u| u.user_type.as_deref() == Some(name) && !u.guest_to_remove && !u.pre_created)
        .count() as i32
}

/// `getAliveUsersExcludingGuestsCountLU`.
fn alive_users_excluding_guests(users: &[UserInfo]) -> i32 {
    users
        .iter()
        .filter(|u| u.user_type.as_deref() != Some(FULL_GUEST) && !u.pre_created)
        .count() as i32
}

/// `UserInfo.canHaveProfile`.
fn can_have_profile(u: &UserInfo, limits: &Limits) -> bool {
    let of = |t: &str| u.user_type.as_deref() == Some(t);
    if u.flags & FLAG_FULL != FLAG_FULL
        || u.flags & FLAG_PROFILE != 0
        || of(FULL_GUEST)
        || of(FULL_RESTRICTED)
        || of(FULL_DEMO)
        || u.flags & FLAG_DEMO != 0
    {
        return false;
    }
    u.flags & FLAG_MAIN == FLAG_MAIN || limits.profiles_for_all
}

/// `canAddMoreUsersOfType(UserTypeDetails)`.
pub(super) fn can_add_more_users_of_type(
    t: &UserType,
    users: &[UserInfo],
    limits: &Limits,
) -> bool {
    if !type_enabled(t, limits) {
        return false;
    }
    t.max_allowed == UNLIMITED
        || users_of_type(users, &t.name) < t.max_allowed
        || limits.creation_override
}

/// `getRemainingCreatableUserCount`, after its permission check.
pub(super) fn remaining_users(t: Option<&UserType>, users: &[UserInfo], limits: &Limits) -> i32 {
    let Some(t) = t.filter(|t| type_enabled(t, limits)) else {
        return 0;
    };
    let count = alive_users_excluding_guests(users);
    let mut result = if t.name == FULL_GUEST || t.name == FULL_DEMO {
        i32::MAX
    } else {
        limits.max_users - count
    };
    if t.managed_profile() {
        if !limits.managed_users {
            return 0;
        }
        if result <= 0 && count == 1 {
            result = 1;
        }
    }
    if result <= 0 {
        return 0;
    }
    let of_type = if t.max_allowed == UNLIMITED {
        i32::MAX
    } else {
        t.max_allowed - users_of_type(users, &t.name)
    };
    result.min(of_type).max(0)
}

/// `getRemainingCreatableProfileCount(type, parent, allowedToRemoveOne)`,
/// after its permission check; `getMaxUsersOfTypePerParent` of a managed
/// profile on a debuggable build is `persist.sys.max_profiles` when set,
/// given as `managed_per_parent`.
pub(super) fn remaining_profiles(
    t: Option<&UserType>,
    users: &[UserInfo],
    parent: i32,
    allowed_to_remove_one: bool,
    limits: &Limits,
    managed_per_parent: Option<i32>,
) -> i32 {
    let Some(t) = t.filter(|t| type_enabled(t, limits)) else {
        return 0;
    };
    let managed = t.managed_profile();
    if managed && !limits.managed_users {
        return 0;
    }
    let Some(parent) = users.iter().find(|u| u.id == parent && !u.partial) else {
        return 0;
    };
    if !can_have_profile(parent, limits) {
        return 0;
    }
    // `getProfileIds(userId, userType, false, false)`: the parent's
    // profile group's users of the type.
    let of_type = users
        .iter()
        .filter(|u| !u.partial && u.same_profile_group(parent))
        .filter(|u| u.user_type.as_deref() == Some(t.name.as_str()))
        .count() as i32;
    let removed = (of_type > 0 && allowed_to_remove_one) as i32;
    let after = alive_users_excluding_guests(users) - removed;
    let mut result = limits.max_users - after;
    if result <= 0 && managed && after == 1 {
        result = 1;
    }
    let per_parent = match (managed, managed_per_parent) {
        (true, Some(max)) => max,
        _ => t.max_allowed_per_parent,
    };
    if per_parent != UNLIMITED {
        result = result.min(per_parent - (of_type - removed));
    }
    if result <= 0 {
        return 0;
    }
    if t.max_allowed != UNLIMITED {
        result = result.min(t.max_allowed - (users_of_type(users, &t.name) - removed));
    }
    result.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            debuggable: false,
            communal: false,
            private: true,
            supervising: false,
        }
    }

    fn limits(max_users: i32) -> Limits {
        Limits {
            max_users,
            creation_override: false,
            managed_users: true,
            profiles_for_all: false,
        }
    }

    /// `UserInfo.NO_PROFILE_GROUP_ID`.
    const USER_NULL: i32 = -10000;

    /// The system user in profile group `group` (`USER_NULL` until it has
    /// a profile).
    fn system_user(group: i32) -> UserInfo {
        UserInfo::of_type(
            0,
            FLAG_SYSTEM | FLAG_FULL | FLAG_MAIN | 0x3 | 0x10,
            FULL_SYSTEM,
            group,
        )
    }

    fn get<'a>(types: &'a [UserType], name: &str) -> Option<&'a UserType> {
        types.iter().find(|t| t.name == name)
    }

    /// `getDefaultBuilders`: the test type only on a debuggable build, the
    /// communal, private and supervising types as their switches say.
    #[test]
    fn default_types_as_the_original() {
        let types = user_types(&facts(), None).unwrap();
        assert_eq!(types.len(), 11);
        assert!(get(&types, PROFILE_TEST).is_none());
        let private = get(&types, PROFILE_PRIVATE).unwrap();
        assert!(private.enabled && private.has_badge);
        assert_eq!(
            (private.max_allowed, private.max_allowed_per_parent),
            (UNLIMITED, 1)
        );
        assert!(!get(&types, PROFILE_COMMUNAL).unwrap().enabled);
        assert!(!get(&types, PROFILE_SUPERVISING).unwrap().has_badge);
        assert_eq!(get(&types, FULL_SYSTEM).unwrap().max_allowed, 1);
        let debuggable = Facts {
            debuggable: true,
            ..facts()
        };
        let test = user_types(&debuggable, None).unwrap();
        assert_eq!(get(&test, PROFILE_TEST).unwrap().max_allowed_per_parent, 2);
    }

    /// `customizeBuilders`: an AOSP profile type's attributes changed, an
    /// OEM profile type added, a full type with a profile's attribute
    /// refused, and an attribute that is no integer thrown.
    #[test]
    fn config_user_types_customizes_them() {
        let el = |name: &str, attrs: &[(&str, Value)]| Element {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(n, v)| aim_apps::res::Attr {
                    ns: String::new(),
                    name: (*n).into(),
                    id: 0,
                    value: v.clone(),
                    kind: 0,
                    data: 0,
                })
                .collect(),
            ..Element::default()
        };
        let root = Element {
            name: "user-types".into(),
            children: vec![
                el(
                    "profile-type",
                    &[
                        ("name", Value::String(PROFILE_PRIVATE.into())),
                        ("enabled", Value::String("0".into())),
                        ("max-allowed", Value::Int(2)),
                    ],
                ),
                el(
                    "profile-type",
                    &[
                        ("name", Value::String("com.oem.profile.KIDS".into())),
                        ("max-allowed-per-parent", Value::Int(3)),
                        ("icon-badge", Value::Ref(0x0108_0001)),
                    ],
                ),
                el("change-user-type", &[]),
            ],
            ..Element::default()
        };
        let types = user_types(&facts(), Some(&root)).unwrap();
        let private = get(&types, PROFILE_PRIVATE).unwrap();
        assert_eq!((private.enabled, private.max_allowed), (false, 2));
        let kids = get(&types, "com.oem.profile.KIDS").unwrap();
        assert_eq!(
            (kids.base_type, kids.max_allowed_per_parent, kids.has_badge),
            (FLAG_PROFILE, 3, true)
        );
        let wrong = Element {
            children: vec![el(
                "full-type",
                &[("name", Value::String(PROFILE_CLONE.into()))],
            )],
            ..root.clone()
        };
        assert!(
            user_types(&facts(), Some(&wrong))
                .unwrap_err()
                .starts_with("Wrong base type")
        );
        let bad = Element {
            children: vec![el(
                "profile-type",
                &[
                    ("name", Value::String(PROFILE_CLONE.into())),
                    ("enabled", Value::String("yes".into())),
                ],
            )],
            ..root
        };
        assert!(user_types(&facts(), Some(&bad)).is_err());
    }

    /// `getRemainingCreatableProfileCount` for the system user alone: one
    /// private profile while users may be added; a managed profile even
    /// at the user limit, as its special case allows; none without the
    /// managed users feature or for a user that does not exist.
    #[test]
    fn profile_limits_follow_the_types_and_users() {
        let types = user_types(&facts(), None).unwrap();
        let users = [system_user(USER_NULL)];
        let private = get(&types, PROFILE_PRIVATE);
        assert_eq!(
            remaining_profiles(private, &users, 0, false, &limits(4), None),
            1
        );
        assert_eq!(
            remaining_profiles(private, &users, 0, false, &limits(1), None),
            0
        );
        assert_eq!(
            remaining_profiles(private, &users, 10, false, &limits(4), None),
            0
        );
        let managed = get(&types, PROFILE_MANAGED);
        assert_eq!(
            remaining_profiles(managed, &users, 0, false, &limits(1), None),
            1
        );
        let no_feature = Limits {
            managed_users: false,
            ..limits(4)
        };
        assert_eq!(
            remaining_profiles(managed, &users, 0, false, &no_feature, None),
            0
        );
        assert_eq!(
            remaining_profiles(managed, &users, 0, false, &limits(8), Some(3)),
            3
        );
        // A private profile already in the group: none more, one if it may
        // be removed.
        let profile = UserInfo::of_type(10, FLAG_PROFILE, PROFILE_PRIVATE, 0);
        let users = [system_user(0), profile];
        assert_eq!(
            remaining_profiles(private, &users, 0, false, &limits(4), None),
            0
        );
        assert_eq!(
            remaining_profiles(private, &users, 0, true, &limits(4), None),
            1
        );
    }

    /// `getRemainingCreatableUserCount` and `canAddMoreUsersOfType`: the
    /// system type is at its maximum, a guest is not counted against the
    /// user limit, a disabled type allows none unless overridden.
    #[test]
    fn user_limits_follow_the_types_and_users() {
        let types = user_types(&facts(), None).unwrap();
        let users = [system_user(USER_NULL)];
        assert_eq!(
            remaining_users(get(&types, FULL_SYSTEM), &users, &limits(4)),
            0
        );
        assert!(!can_add_more_users_of_type(
            get(&types, FULL_SYSTEM).unwrap(),
            &users,
            &limits(4)
        ));
        assert_eq!(
            remaining_users(get(&types, FULL_SECONDARY), &users, &limits(4)),
            3
        );
        assert_eq!(
            remaining_users(get(&types, FULL_GUEST), &users, &limits(1)),
            1
        );
        assert_eq!(remaining_users(None, &users, &limits(4)), 0);
        let communal = get(&types, PROFILE_COMMUNAL).unwrap();
        assert!(!can_add_more_users_of_type(communal, &users, &limits(4)));
        let override_ = Limits {
            creation_override: true,
            ..limits(4)
        };
        assert!(can_add_more_users_of_type(communal, &users, &override_));
    }
}
