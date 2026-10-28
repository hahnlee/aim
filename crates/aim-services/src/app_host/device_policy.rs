//! `device_policy`: DevicePolicyManagerService for the app host
//! (`DevicePolicyManagerService` at `android-16.0.0_r1`, ADR 0013), the
//! reads apps make of a device with no device or profile owner, no active
//! admins and no policies: the owners (`OwnersData`: `device_owner_2.xml`,
//! each user's `profile_owner.xml`), the active admins and each user's
//! setup and provisioning state (`DevicePolicyData`: `device_policies.xml`)
//! and the policies the policy engine resolved (`DevicePolicyEngine`:
//! `device_policy_state.xml`), as the original persisted them, with the
//! original's checks and its answers when none of them is set. A device
//! that has an owner, an admin, a delegation or a policy, authorized policy
//! writes and every other method are not implemented (#1521).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aim_android_xml::Element;
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader,
};
use aim_service_aidl::android_app_admin_idevicepolicymanager as dp;
use aim_service_aidl::dev_aim_server_idevicepolicydefaults as defaults;

use super::account_service::UserHandle;
use super::activity_start::Component;
use super::settings::Table;
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::intent::ComponentName;

/// `PackageManager.FEATURE_DEVICE_ADMIN`: without it the service answers
/// without reading its state (`mHasFeature`).
const FEATURE_DEVICE_ADMIN: &str = "android.software.device_admin";
/// `Process.ROOT_UID`, `SYSTEM_UID`, `CAMERASERVER_UID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const CAMERASERVER_UID: i32 = 1047;
/// `UserHandle.USER_NULL`.
const USER_NULL: i32 = -10000;
/// `DevicePolicyManager.PERMISSION_POLICY_PROMPT`.
const PERMISSION_POLICY_PROMPT: i32 = 0;
/// `DevicePolicyManager.ENCRYPTION_STATUS_*`.
const ENCRYPTION_STATUS_UNSUPPORTED: i32 = 0;
const ENCRYPTION_STATUS_ACTIVE: i32 = 3;
const ENCRYPTION_STATUS_ACTIVE_PER_USER: i32 = 5;
/// `Build.VERSION_CODES.M`.
const M: i32 = 23;
/// `DevicePolicyManager.PASSWORD_QUALITY_UNSPECIFIED`,
/// `KEYGUARD_DISABLE_FEATURES_NONE`, `STATE_USER_UNMANAGED`.
const PASSWORD_QUALITY_UNSPECIFIED: i32 = 0;
const KEYGUARD_DISABLE_FEATURES_NONE: i32 = 0;
const STATE_USER_UNMANAGED: i32 = 0;
/// `DevicePolicyManager.PASSWORD_COMPLEXITY_NONE`.
const PASSWORD_COMPLEXITY_NONE: i32 = 0;

const MANAGE_USERS: &str = "android.permission.MANAGE_USERS";
const MANAGE_PROFILE_AND_DEVICE_OWNERS: &str =
    "android.permission.MANAGE_PROFILE_AND_DEVICE_OWNERS";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const QUERY_ADMIN_POLICY: &str = "android.permission.QUERY_ADMIN_POLICY";
const MANAGE_DEVICE_POLICY_APPS_CONTROL: &str =
    "android.permission.MANAGE_DEVICE_POLICY_APPS_CONTROL";
const MANAGE_DEVICE_POLICY_CONTENT_PROTECTION: &str =
    "android.permission.MANAGE_DEVICE_POLICY_CONTENT_PROTECTION";
const MANAGE_DEVICE_POLICY_KEYGUARD: &str = "android.permission.MANAGE_DEVICE_POLICY_KEYGUARD";
const MANAGE_DEVICE_POLICY_CAMERA: &str = "android.permission.MANAGE_DEVICE_POLICY_CAMERA";
/// `CROSS_USER_PERMISSIONS`' permission of `MANAGE_DEVICE_POLICY_CAMERA`.
const MANAGE_DEVICE_POLICY_ACROSS_USERS: &str =
    "android.permission.MANAGE_DEVICE_POLICY_ACROSS_USERS";
const SET_TIME: &str = "android.permission.SET_TIME";
const SET_TIME_ZONE: &str = "android.permission.SET_TIME_ZONE";
/// The aconfig flags the service reads.
const MANAGE_CONTENT_PROTECTION: &str =
    "android.view.contentprotection.flags.manage_device_policy_enabled";
const SET_KEYGUARD_DISABLED_FEATURES_COEXISTENCE: &str =
    "android.app.admin.flags.set_keyguard_disabled_features_coexistence";
const SET_AUTO_TIME_ENABLED_COEXISTENCE: &str =
    "android.app.admin.flags.set_auto_time_enabled_coexistence";
const SET_AUTO_TIME_ZONE_ENABLED_COEXISTENCE: &str =
    "android.app.admin.flags.set_auto_time_zone_enabled_coexistence";

const DEVICE_OWNER_XML: &str = "device_owner_2.xml";
const PROFILE_OWNER_XML: &str = "profile_owner.xml";
const DEVICE_POLICIES_XML: &str = "device_policies.xml";
const DEVICE_POLICY_STATE_XML: &str = "device_policy_state.xml";
const UPDATED_RESOURCES_XML: &str = "updated_resources.xml";
/// The tags `DeviceOwnerReadWriter.readInner` and
/// `ProfileOwnerReadWriter.readInner` know; an unknown one ends the read.
const DEVICE_OWNER_TAGS: &[&str] = &[
    "device-owner",
    "device-owner-context",
    "system-update-policy",
    "pending-ota-info",
    "freeze-record",
    "device-owner-type",
    "device-owner-protected-packages",
    "policy-engine-migration",
];
const PROFILE_OWNER_TAGS: &[&str] = &["profile-owner"];

/// A `ParcelableResource`, of which the service answers none.
enum ParcelableResource {}

impl aim_service_aidl::WriteParcelable for ParcelableResource {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

/// One user's `DevicePolicyData`, as far as its reads need it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct UserData {
    /// `mUserSetupComplete`.
    setup_complete: bool,
    /// `mUserProvisioningState`.
    provisioning_state: i32,
}

impl Default for UserData {
    fn default() -> Self {
        UserData {
            setup_complete: false,
            provisioning_state: STATE_USER_UNMANAGED,
        }
    }
}

/// What DevicePolicyManagerService persisted.
#[derive(Debug, Default)]
pub struct Policies {
    /// A device owner, or a profile owner of a user.
    owned: bool,
    device_owner: Option<String>,
    device_owner_user: i32,
    device_owner_type: i32,
    profile_owners: HashMap<i32, ComponentName>,
    active_admins: HashMap<i32, Vec<ComponentName>>,
    affiliation_ids: HashMap<i32, Vec<String>>,
    managed_users: Vec<i32>,
    admin_packages: Vec<String>,
    /// The owners' system update policy (`OwnersData.mSystemUpdatePolicy`).
    system_update_policy: bool,
    /// An active admin or a delegation of a user.
    admins: bool,
    /// A policy the policy engine keeps.
    policies: bool,
    /// DevicePolicyEngine's persisted policy state, decoded by the original Java.
    engine_xml: Vec<u8>,
    users: HashMap<i32, UserData>,
    /// The ids of the strings a device policy resource update replaced
    /// (`DeviceManagementResourcesProvider.mUpdatedStrings`).
    updated_strings: Vec<String>,
}

impl Policies {
    /// The state under the guest's `/data`, as `Owners.load`,
    /// `loadSettingsLocked` and `DevicePolicyEngine.load` read it.
    pub fn load(data: &Path) -> Result<Policies, String> {
        let system = data.join("system");
        let owners = read_atomic(&system.join(DEVICE_OWNER_XML))?;
        let mut policies = Policies {
            owned: owners
                .as_ref()
                .is_some_and(|root| names_owner(root, "device-owner", DEVICE_OWNER_TAGS)),
            system_update_policy: owners.as_ref().is_some_and(|root| {
                root.name == "root"
                    && root
                        .children()
                        .take_while(|e| DEVICE_OWNER_TAGS.contains(&e.name.as_str()))
                        .any(|e| e.name == "system-update-policy")
            }),
            ..Default::default()
        };
        // `getDeviceOwnerComponent(false)` and `DeviceStateCacheImpl`'s
        // enterprise device owner (the default owner type).
        let mut owner_type = 0;
        if let Some(root) = owners.as_ref().filter(|r| r.name == "root") {
            for e in root
                .children()
                .take_while(|e| DEVICE_OWNER_TAGS.contains(&e.name.as_str()))
            {
                if e.name == "device-owner" {
                    policies.device_owner_user = 0;
                } else if e.name == "device-owner-context" {
                    policies.device_owner_user =
                        e.int("userId")?.unwrap_or(policies.device_owner_user);
                }
            }
            policies.device_owner = root
                .children()
                .take_while(|e| DEVICE_OWNER_TAGS.contains(&e.name.as_str()))
                .filter(|e| e.name == "device-owner")
                .filter_map(|e| e.string("component"))
                .filter(|c| unflattens(c))
                .map(|c| c.split_once('/').unwrap().0.to_string())
                .last();
            for e in root
                .children()
                .take_while(|e| DEVICE_OWNER_TAGS.contains(&e.name.as_str()))
                .filter(|e| {
                    e.name == "device-owner-type"
                        && e.string("package").as_deref() == policies.device_owner.as_deref()
                })
            {
                owner_type = e
                    .int("value")
                    .map_err(|e| format!("device owner type: {e}"))?
                    .unwrap_or(0);
            }
        }
        policies.device_owner_type = owner_type;
        let managed_device = policies.device_owner.is_some() && owner_type == 0;
        if managed_device {
            policies.managed_users.push(0);
        }
        // `getPolicyFileDirectory`: the system user's file is in
        // `/data/system`, the others' in their user directories.
        let mut files = vec![(0, system.join(DEVICE_POLICIES_XML))];
        let users = match fs::read_dir(system.join("users")) {
            Ok(entries) => entries
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("system/users: {e}"))?,
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("system/users: {e}")),
        };
        for entry in users {
            let Some(user) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<i32>().ok())
            else {
                continue;
            };
            let profile = read_atomic(&entry.path().join(PROFILE_OWNER_XML))?;
            let profile_owner = profile
                .as_ref()
                .is_some_and(|root| names_owner(root, "profile-owner", PROFILE_OWNER_TAGS));
            if let Some(root) = profile.as_ref().filter(|_| profile_owner) {
                for e in root
                    .children()
                    .take_while(|e| PROFILE_OWNER_TAGS.contains(&e.name.as_str()))
                {
                    if let Some(component) = e
                        .string("component")
                        .and_then(|c| crate::package::preferred::unflatten(&c))
                    {
                        policies.profile_owners.insert(user, component);
                    }
                }
            }
            policies.owned |= profile_owner;
            if managed_device || profile_owner {
                policies.managed_users.push(user);
            }
            if user != 0 {
                files.push((user, entry.path().join(DEVICE_POLICIES_XML)));
            }
        }
        for (user, file) in files {
            let Some(root) = read_journaled(&file)? else {
                continue;
            };
            // `DevicePolicyData.load`: a document of another root is not read.
            if root.name != "policies" {
                continue;
            }
            let provisioning_state = root
                .int("provisioning-state")
                .map_err(|e| format!("{}: {e}", file.display()))?;
            policies.users.insert(
                user,
                UserData {
                    setup_complete: root.string("setup-complete").as_deref() == Some("true"),
                    provisioning_state: provisioning_state
                        .filter(|s| *s != -1)
                        .unwrap_or(STATE_USER_UNMANAGED),
                },
            );
            policies.admins |= root
                .children()
                .any(|e| e.name == "admin" || e.name == "delegation");
            policies.active_admins.insert(
                user,
                root.children()
                    .filter(|e| e.name == "admin")
                    .filter_map(|e| e.string("name"))
                    .filter_map(|c| crate::package::preferred::unflatten(&c))
                    .collect(),
            );
            policies.affiliation_ids.insert(
                user,
                root.children()
                    .filter(|e| e.name == "affiliation-id")
                    .filter_map(|e| e.string("id").map(|id| id.into_owned()))
                    .collect(),
            );
            // `DevicePolicyData.load`: active admins' flattened components.
            policies.admin_packages.extend(
                root.children()
                    .filter(|e| e.name == "admin")
                    .filter_map(|e| e.string("name"))
                    .filter(|c| unflattens(c))
                    .map(|c| c.split_once('/').unwrap().0.to_string()),
            );
        }
        // `DevicePoliciesReaderWriter.readFromFileLocked`: nothing without
        // the file itself; its entries are each a root.
        let engine = system.join(DEVICE_POLICY_STATE_XML);
        if engine.exists()
            && let Some(bytes) = read_atomic_bytes(&engine)?
        {
            let entries = aim_android_xml::read_sequence(&bytes)
                .map_err(|e| format!("{}: {e}", engine.display()))?;
            policies.engine_xml = bytes;
            policies.policies = entries
                .iter()
                .any(|e| e.name == "local-policy-entry" || e.name == "global-policy-entry");
        }
        // `ResourcesReaderWriter.readFromFileLocked`: nothing under another
        // root, and nothing after a tag it does not know.
        if let Some(root) = read_atomic(&system.join(UPDATED_RESOURCES_XML))?
            && root.name == "root"
        {
            for e in root.children() {
                match e.name.as_str() {
                    "drawable-style-entry" | "drawable-source-entry" => {}
                    "string-entry" => policies
                        .updated_strings
                        .extend(e.string("source-id").map(|s| s.into_owned())),
                    _ => break,
                }
            }
        }
        Ok(policies)
    }

    pub(super) fn fgs_owner(&self, packages: &[&str], user: i32, device: bool) -> bool {
        if device {
            user == self.device_owner_user
                && self
                    .device_owner
                    .as_deref()
                    .is_some_and(|p| packages.contains(&p))
        } else {
            self.profile_owners
                .get(&user)
                .is_some_and(|p| packages.contains(&p.package.as_str()))
        }
    }

    /// Whether the device has a device owner or a user a profile owner.
    pub fn owned(&self) -> bool {
        self.owned
    }

    /// Whether a user has an active admin or a delegation, or the policy
    /// engine a policy.
    pub fn has_admins_or_policies(&self) -> bool {
        self.admins || self.policies
    }

    /// `DevicePolicyManager.packageHasActiveAdmins` of this user.
    pub(super) fn package_has_active_admins(&self, package: Option<&str>, user: i32) -> bool {
        self.active_admins
            .get(&user)
            .is_some_and(|admins| admins.iter().any(|a| Some(a.package.as_str()) == package))
    }

    /// `DevicePolicyManagerInternal.canSilentlyInstallPackage`: an affiliated
    /// device owner or active profile owner. Identity is the uid, not the
    /// supplied package name (which the original only checks for null).
    pub(super) fn can_silently_install_package(
        &self,
        installer: Option<&str>,
        uid: i32,
        owns: impl Fn(&str) -> Result<bool, Exception>,
    ) -> Result<bool, Exception> {
        if installer.is_none() || self.device_owner.is_none() {
            return Ok(false);
        }
        let user = uid / PER_USER_RANGE as i32;
        let profile = self.profile_owners.get(&user);
        let affiliated = user == 0
            || user == self.device_owner_user
            || (profile.is_some()
                && self.affiliation_ids.get(&user).is_some_and(|ids| {
                    self.affiliation_ids
                        .get(&0)
                        .is_some_and(|device| ids.iter().any(|id| device.contains(id)))
                }));
        if !affiliated {
            return Ok(false);
        }
        if let Some(component) = profile
            && self
                .active_admins
                .get(&user)
                .is_some_and(|admins| admins.contains(component))
            && owns(&component.package)?
        {
            return Ok(true);
        }
        if user == self.device_owner_user && matches!(self.device_owner_type, 0 | 1) {
            return owns(self.device_owner.as_deref().unwrap());
        }
        Ok(false)
    }

    /// `getDeviceOwnerComponent(false)` and `packageHasActiveAdmins`.
    pub(super) fn package_is_admin(&self, package: &str) -> bool {
        self.device_owner.as_deref() == Some(package)
            || self.admin_packages.iter().any(|p| p == package)
    }

    /// `isUserOrganizationManaged`: a device or profile owner owns the user.
    pub(super) fn managed_users(&self) -> &[i32] {
        &self.managed_users
    }

    /// `getUserData(user)`: a user without a file has the defaults.
    fn user(&self, user: i32) -> UserData {
        self.users.get(&user).copied().unwrap_or_default()
    }
}

/// `path`'s bytes, none if it does not exist.
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `path` with `suffix` appended to its name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// `path`'s bytes as `AtomicFile.openRead` reads them: its legacy backup's
/// in its place if there is one.
pub(super) fn read_atomic_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match read_bytes(&suffixed(path, ".bak"))? {
        Some(bytes) => Ok(Some(bytes)),
        None => read_bytes(path),
    }
}

fn read_atomic(path: &Path) -> Result<Option<Element>, String> {
    read_document(path, read_atomic_bytes(path)?)
}

/// The document of `path`, as `JournaledFile.chooseForRead` reads it: its
/// temporary file in its place if it alone exists.
fn read_journaled(path: &Path) -> Result<Option<Element>, String> {
    let bytes = match read_bytes(path)? {
        Some(bytes) => Some(bytes),
        None => read_bytes(&suffixed(path, ".tmp"))?,
    };
    read_document(path, bytes)
}

fn read_document(path: &Path, bytes: Option<Vec<u8>>) -> Result<Option<Element>, String> {
    bytes
        .map(|b| aim_android_xml::read(&b).map_err(|e| format!("{}: {e}", path.display())))
        .transpose()
}

/// `FileReadWriter.readFromFileLocked` of an owners file: under a `root`
/// element, its tags in order until one it does not know; whether one is
/// `tag` with a component (`OwnerInfo.readFromXml`).
fn names_owner(root: &Element, tag: &str, known: &[&str]) -> bool {
    root.name == "root"
        && root
            .children()
            .take_while(|e| known.contains(&e.name.as_str()))
            .any(|e| e.name == tag && e.string("component").is_some_and(|c| unflattens(&c)))
}

/// `ComponentName.unflattenFromString` makes a component of `s`.
fn unflattens(s: &str) -> bool {
    s.find('/').is_some_and(|sep| sep + 1 < s.len())
}

/// `ComponentName.toString`.
fn component_string(c: &ComponentName) -> String {
    format!("ComponentInfo{{{}/{}}}", c.package, c.class)
}

fn user_of(uid: i32) -> i32 {
    uid / PER_USER_RANGE as i32
}

fn app_id(uid: i32) -> i32 {
    uid.rem_euclid(PER_USER_RANGE as i32)
}

/// `Preconditions.checkCallAuthorization`.
fn authorize(authorized: bool) -> Result<(), Exception> {
    match authorized {
        true => Ok(()),
        false => Err(Exception::security("Calling identity is not authorized")),
    }
}

/// `Preconditions.checkArgumentNonnegative(user, "Invalid userId")`.
fn valid_user(user: i32) -> Result<(), Exception> {
    match user < 0 {
        true => Err(Exception::illegal_argument("Invalid userId")),
        false => Ok(()),
    }
}

/// `Objects.requireNonNull(value, "<what> is null")`'s exception.
fn null(what: &str) -> Exception {
    Exception::new(EX_NULL_POINTER, format!("{what} is null"))
}

/// `Settings.*.getInt(name, def)`: the default for a value that is not a
/// number.
fn int_or(value: Option<String>, default: i32) -> i32 {
    value.and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// `getStorageEncryptionStatus`'s status for `ro.crypto.type` and the
/// caller's target SDK: file-based encryption is active per user, reported
/// as active to apps of M and before.
fn encryption_status(crypto_type: Option<&str>, target_sdk: i32) -> i32 {
    match crypto_type.is_some_and(|t| t.eq_ignore_ascii_case("file")) {
        true if target_sdk <= M => ENCRYPTION_STATUS_ACTIVE,
        true => ENCRYPTION_STATUS_ACTIVE_PER_USER,
        false => ENCRYPTION_STATUS_UNSUPPORTED,
    }
}

/// What the service reads beside its own state.
trait Device {
    fn usb_data_signaling_enabled(&self) -> Result<bool, Exception>;
    fn content_protection_default(&self, user: i32) -> Result<i32, Exception>;
    fn string_array(&self, name: &str) -> Result<Vec<Option<String>>, Exception>;
    /// `mHasFeature`.
    fn feature(&self) -> Result<bool, Exception>;
    fn policies(&self) -> Result<&Policies, Exception>;
    /// `checkCallingOrSelfPermission` for a caller of `uid`.
    fn permitted(&self, uid: i32, permission: &str) -> Result<bool, Exception>;
    /// The app id of `package`, if it is installed in `user`.
    fn app_id(&self, package: &str, user: i32) -> Result<Option<i32>, Exception>;
    fn target_sdk(&self, package: &str) -> Result<Option<i32>, Exception>;
    fn flag(&self, name: &str) -> Result<bool, Exception>;
    /// User 0's setting.
    fn setting(&self, table: Table, name: &str) -> Result<Option<String>, Exception>;
    fn property(&self, name: &str) -> Option<String>;
    /// The managed profiles of `user` (`UserManager.getProfiles`).
    fn managed_profiles(&self, user: i32) -> Result<Vec<i32>, Exception>;
    fn profiles(&self, user: i32) -> Vec<i32>;
    /// The exception of `what`, not implemented.
    fn unimplemented(&self, what: &str) -> Exception;
}

/// The caller's checks, as the original makes them.
struct Caller<'a, D> {
    device: &'a D,
    uid: i32,
}

impl<D: Device> Caller<'_, D> {
    fn user(&self) -> i32 {
        user_of(self.uid)
    }

    fn permitted(&self, permission: &str) -> Result<bool, Exception> {
        self.device.permitted(self.uid, permission)
    }

    fn system_or_root(&self) -> bool {
        matches!(app_id(self.uid), SYSTEM_UID | ROOT_UID)
    }

    /// `hasFullCrossUsersPermission`.
    fn full_cross_users(&self, user: i32) -> Result<bool, Exception> {
        Ok(user == self.user()
            || self.system_or_root()
            || self.permitted(INTERACT_ACROSS_USERS_FULL)?)
    }

    /// `hasCrossUsersPermission`.
    fn cross_users(&self, user: i32) -> Result<bool, Exception> {
        Ok(
            user == self.user()
                || self.system_or_root()
                || self.permitted(INTERACT_ACROSS_USERS)?,
        )
    }

    /// `canManageUsers`.
    fn manages_users(&self) -> Result<bool, Exception> {
        self.permitted(MANAGE_USERS)
    }

    /// `canManageUsers` or `MANAGE_PROFILE_AND_DEVICE_OWNERS`.
    fn manages_owners(&self) -> Result<bool, Exception> {
        Ok(self.manages_users()? || self.permitted(MANAGE_PROFILE_AND_DEVICE_OWNERS)?)
    }

    /// `isCallingFromPackage`.
    fn is_package(&self, package: &str) -> Result<bool, Exception> {
        let user = self.user();
        Ok(self
            .device
            .app_id(package, user)?
            .is_some_and(|id| self.uid == user * PER_USER_RANGE as i32 + id))
    }

    /// `getCallerIdentity(who, package)`: the package, the caller's, and the
    /// admin, which no user has.
    fn identity<'p>(
        &self,
        who: Option<&ComponentName>,
        package: Option<&'p str>,
    ) -> Result<Option<&'p str>, Exception> {
        if let Some(package) = package
            && !self.is_package(package)?
        {
            return Err(Exception::security(format!(
                "Caller with uid {} is not {package}",
                self.uid
            )));
        }
        match who {
            Some(who) => Err(self.not_admin(who)),
            None => Ok(package),
        }
    }

    /// `getCallerIdentity`'s exception for an admin that is not active.
    fn not_admin(&self, who: &ComponentName) -> Exception {
        Exception::security(format!(
            "Admin {} does not exist or is not owned by uid {}",
            component_string(who),
            self.uid
        ))
    }

    /// `hasPermission(permission, package)`: the caller's package holds
    /// it; no owner grants it and none delegates it.
    fn has_permission(&self, permission: &str, package: Option<&str>) -> Result<bool, Exception> {
        let package = package.ok_or_else(|| null("callerPackageName"))?;
        self.identity(None, Some(package))?;
        self.permitted(permission)
    }

    /// `hasPermission(permission, package, user)`, with the permission
    /// `CROSS_USER_PERMISSIONS` gives it for another user.
    fn has_permission_on(
        &self,
        permission: &str,
        across: &str,
        package: Option<&str>,
        user: i32,
    ) -> Result<bool, Exception> {
        Ok(self.has_permission(permission, package)?
            && (self.user() == user || self.has_permission(across, package)?))
    }

    /// `enforcePermission`: no DPC, role holder or delegation grants it here.
    fn enforce_permission(&self, permission: &str, package: Option<&str>) -> Result<(), Exception> {
        if !self.has_permission(permission, package)? {
            return Err(Exception::security(format!(
                "Caller does not have the required permissions for this user. Permission required: {permission}."
            )));
        }
        Ok(())
    }

    /// `enforceCanQuery`: QUERY_ADMIN_POLICY also permits cross-user queries.
    fn can_query(
        &self,
        permission: &str,
        package: Option<&str>,
        user: i32,
    ) -> Result<(), Exception> {
        if self.has_permission(QUERY_ADMIN_POLICY, package)? {
            return Ok(());
        }
        self.enforce_permission(permission, package)?;
        if self.user() != user {
            self.enforce_permission(MANAGE_DEVICE_POLICY_ACROSS_USERS, package)?;
        }
        Ok(())
    }
}

/// The reply to `code` of `uid`, with its arguments in `r`.
fn answer(device: &impl Device, uid: i32, code: u32, r: &mut Reader<'_>) -> Reply {
    let caller = Caller { device, uid };
    let mut p = Parcel::new();
    let feature = || device.feature();
    // Without the feature the original loads neither owners nor policies.
    let unmanaged = || -> Result<(), Exception> {
        let policies = device.policies()?;
        match feature()? && (policies.owned || policies.admins || policies.policies) {
            true => Err(device.unimplemented(
                "a device with a device or profile owner, active admins or policies (#1521)",
            )),
            false => Ok(()),
        }
    };
    let result: Result<(), Exception> = match code {
        dp::IS_USB_DATA_SIGNALING_ENABLED => {
            let a = dp::IsUsbDataSignalingEnabled::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                caller.identity(None, a.caller_package.as_deref())?;
                dp::write_is_usb_data_signaling_enabled_reply(
                    &mut p,
                    device.usb_data_signaling_enabled()?,
                );
                Ok(())
            })()
        }
        dp::GET_CONTENT_PROTECTION_POLICY => {
            let a =
                dp::GetContentProtectionPolicy::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if device.flag(MANAGE_CONTENT_PROTECTION)? {
                    unmanaged()?;
                    caller.identity(a.who.as_ref(), a.caller_package_name.as_deref())?;
                    caller.can_query(
                        MANAGE_DEVICE_POLICY_CONTENT_PROTECTION,
                        a.caller_package_name.as_deref(),
                        a.user_id,
                    )?;
                }
                dp::write_get_content_protection_policy_reply(
                    &mut p,
                    device.content_protection_default(a.user_id)?,
                );
                Ok(())
            })()
        }
        dp::SET_CONTENT_PROTECTION_POLICY => {
            let a =
                dp::SetContentProtectionPolicy::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if device.flag(MANAGE_CONTENT_PROTECTION)? {
                    unmanaged()?;
                    caller.identity(a.who.as_ref(), a.caller_package_name.as_deref())?;
                    caller.enforce_permission(
                        MANAGE_DEVICE_POLICY_CONTENT_PROTECTION,
                        a.caller_package_name.as_deref(),
                    )?;
                    return Err(device.unimplemented("setContentProtectionPolicy (#1521)"));
                }
                dp::write_set_content_protection_policy_reply(&mut p);
                Ok(())
            })()
        }
        dp::SET_USER_CONTROL_DISABLED_PACKAGES => {
            let a = dp::SetUserControlDisabledPackages::<ComponentName>::read(r)
                .map_err(|_| BAD_VALUE)?;
            (|| {
                a.packages.as_ref().ok_or_else(|| null("packages"))?;
                unmanaged()?;
                caller.identity(a.admin.as_ref(), a.caller_package_name.as_deref())?;
                // The original has no safety checker on an unmanaged device.
                caller.enforce_permission(
                    MANAGE_DEVICE_POLICY_APPS_CONTROL,
                    a.caller_package_name.as_deref(),
                )?;
                Err(device.unimplemented("setUserControlDisabledPackages (#1521)"))
            })()
        }
        dp::GET_USER_CONTROL_DISABLED_PACKAGES => {
            let a = dp::GetUserControlDisabledPackages::<ComponentName>::read(r)
                .map_err(|_| BAD_VALUE)?;
            (|| {
                unmanaged()?;
                caller.identity(a.admin.as_ref(), a.caller_package_name.as_deref())?;
                caller.can_query(
                    MANAGE_DEVICE_POLICY_APPS_CONTROL,
                    a.caller_package_name.as_deref(),
                    caller.user(),
                )?;
                dp::write_get_user_control_disabled_packages_reply(&mut p, &Some(vec![]));
                Ok(())
            })()
        }
        dp::SET_KEYGUARD_DISABLED_FEATURES => {
            let a =
                dp::SetKeyguardDisabledFeatures::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    if device.flag(SET_KEYGUARD_DISABLED_FEATURES_COEXISTENCE)? {
                        caller.identity(a.who.as_ref(), a.caller_package_name.as_deref())?;
                        // A privileged policy write, including the parent's policy,
                        // still needs the policy engine (#1521).
                        caller.enforce_permission(
                            MANAGE_DEVICE_POLICY_KEYGUARD,
                            a.caller_package_name.as_deref(),
                        )?;
                        return Err(device.unimplemented("setKeyguardDisabledFeatures (#1521)"));
                    }
                    caller.identity(a.who.as_ref(), None)?;
                    // No active admin passed identity, so requireNonNull(who) fails.
                    return Err(null("ComponentName"));
                }
                dp::write_set_keyguard_disabled_features_reply(&mut p);
                Ok(())
            })()
        }
        dp::GET_DEFAULT_CROSS_PROFILE_PACKAGES => {
            dp::GetDefaultCrossProfilePackages::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                // The original has no feature, owner or permission gate here.
                let mut packages: HashSet<_> = device
                    .string_array("cross_profile_apps")?
                    .into_iter()
                    .collect();
                packages.extend(device.string_array("vendor_cross_profile_apps")?);
                let packages: Vec<_> = packages.into_iter().collect();
                dp::write_get_default_cross_profile_packages_reply(&mut p, &Some(packages));
                Ok(())
            })()
        }
        dp::RESET_DEFAULT_CROSS_PROFILE_INTENT_FILTERS => {
            let a = dp::ResetDefaultCrossProfileIntentFilters::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                authorize(
                    caller.system_or_root()
                        || caller.permitted(MANAGE_PROFILE_AND_DEVICE_OWNERS)?,
                )?;
                // DevicePolicyManagerService.resetDefaultCrossProfileIntentFilters,
                // android-16.0.0_r1: no filters to reset in a group of at most one.
                if device.profiles(a.user_id).len() > 1 {
                    return Err(device.unimplemented(
                        "resetting cross-profile intent filters for profiles (#1521)",
                    ));
                }
                dp::write_reset_default_cross_profile_intent_filters_reply(&mut p);
                Ok(())
            })()
        }
        dp::GET_PASSWORD_QUALITY => {
            let a = dp::GetPasswordQuality::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    valid_user(a.user_handle)?;
                    authorize(caller.full_cross_users(a.user_handle)?)?;
                    // A system caller may ask about a particular admin.
                    authorize(match &a.who {
                        None => true,
                        Some(who) => {
                            caller.is_package(&who.package)?
                                || caller.permitted(QUERY_ADMIN_POLICY)?
                        }
                    })?;
                }
                // The strictest of no admins.
                dp::write_get_password_quality_reply(&mut p, PASSWORD_QUALITY_UNSPECIFIED);
                Ok(())
            })()
        }
        dp::GET_STORAGE_ENCRYPTION_STATUS => {
            let a = dp::GetStorageEncryptionStatus::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                valid_user(a.user_handle)?;
                caller.identity(None, a.caller_package.as_deref())?;
                authorize(caller.full_cross_users(a.user_handle)?)?;
                let installed = match &a.caller_package {
                    Some(package) if device.app_id(package, a.user_handle)?.is_some() => {
                        device.target_sdk(package)?
                    }
                    _ => None,
                };
                let target_sdk = installed.ok_or_else(|| {
                    Exception::new(
                        EX_NULL_POINTER,
                        "Attempt to read from field 'int android.content.pm.ApplicationInfo.targetSdkVersion' on a null object reference",
                    )
                })?;
                let status =
                    encryption_status(device.property("ro.crypto.type").as_deref(), target_sdk);
                dp::write_get_storage_encryption_status_reply(&mut p, status);
                Ok(())
            })()
        }
        dp::GET_CAMERA_DISABLED => {
            let a = dp::GetCameraDisabled::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    let package =
                        caller.identity(a.who.as_ref(), a.caller_package_name.as_deref())?;
                    authorize(
                        caller.full_cross_users(a.user_handle)?
                            || app_id(uid) == CAMERASERVER_UID
                            || caller.has_permission_on(
                                MANAGE_DEVICE_POLICY_CAMERA,
                                MANAGE_DEVICE_POLICY_ACROSS_USERS,
                                package,
                                a.user_handle,
                            )?
                            || caller.has_permission(QUERY_ADMIN_POLICY, package)?,
                    )?;
                }
                // No admin set `DISALLOW_CAMERA`.
                dp::write_get_camera_disabled_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_SCREEN_CAPTURE_DISABLED => {
            let a =
                dp::GetScreenCaptureDisabled::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    caller.identity(a.who.as_ref(), None)?;
                    authorize(caller.full_cross_users(a.user_handle)?)?;
                    // The parent instance is a profile owner's of an
                    // organization-owned device, which no user has.
                    authorize(!a.parent)?;
                }
                dp::write_get_screen_capture_disabled_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_PERMISSION_POLICY => {
            dp::GetPermissionPolicy::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                // `DevicePolicyCache.getPermissionPolicy` of the caller's
                // user: no resolved policy sets one.
                unmanaged()?;
                dp::write_get_permission_policy_reply(&mut p, PERMISSION_POLICY_PROMPT);
                Ok(())
            })()
        }
        dp::IS_COMMON_CRITERIA_MODE_ENABLED => {
            let a =
                dp::IsCommonCriteriaModeEnabled::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                // Only a device owner or an organization-owned device's
                // profile owner turns it on; there is none.
                unmanaged()?;
                if a.admin.is_some() {
                    caller.identity(a.admin.as_ref(), None)?;
                }
                dp::write_is_common_criteria_mode_enabled_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_KEYGUARD_DISABLED_FEATURES => {
            let a =
                dp::GetKeyguardDisabledFeatures::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    valid_user(a.user_handle)?;
                    authorize(caller.full_cross_users(a.user_handle)?)?;
                    authorize(match &a.who {
                        None => true,
                        Some(who) => caller.is_package(&who.package)? || app_id(uid) == SYSTEM_UID,
                    })?;
                }
                dp::write_get_keyguard_disabled_features_reply(
                    &mut p,
                    KEYGUARD_DISABLE_FEATURES_NONE,
                );
                Ok(())
            })()
        }
        dp::IS_ADMIN_ACTIVE => {
            let a = dp::IsAdminActive::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    valid_user(a.user_handle)?;
                    authorize(caller.full_cross_users(a.user_handle)?)?;
                }
                dp::write_is_admin_active_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_ACTIVE_ADMINS => {
            let a = dp::GetActiveAdmins::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                // Without the feature an empty list, else none for no admins.
                let none: &[Option<Component>] = &[];
                let admins = match feature()? {
                    false => Some(none),
                    true => {
                        unmanaged()?;
                        valid_user(a.user_handle)?;
                        authorize(caller.full_cross_users(a.user_handle)?)?;
                        None
                    }
                };
                dp::write_get_active_admins_reply(&mut p, admins);
                Ok(())
            })()
        }
        dp::GET_AGGREGATED_PASSWORD_COMPLEXITY_FOR_USER => {
            let a = dp::GetAggregatedPasswordComplexityForUser::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    authorize(caller.full_cross_users(a.user_id)?)?;
                    // The strictest of the admins' or the resolved policies:
                    // with no admins and no policies, none.
                    unmanaged()?;
                }
                dp::write_get_aggregated_password_complexity_for_user_reply(
                    &mut p,
                    PASSWORD_COMPLEXITY_NONE,
                );
                Ok(())
            })()
        }
        dp::GET_POLICY_MANAGED_PROFILES => {
            let a = dp::GetPolicyManagedProfiles::<UserHandle>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                authorize(caller.permitted(MANAGE_PROFILE_AND_DEVICE_OWNERS)?)?;
                let user = a.user_handle.ok_or_else(|| {
                    Exception::new(
                        EX_NULL_POINTER,
                        "Attempt to invoke virtual method 'int android.os.UserHandle.getIdentifier()' on a null object reference",
                    )
                })?;
                // The user's managed profiles that have a profile owner.
                if !device.managed_profiles(user.0)?.is_empty() {
                    return Err(device.unimplemented("a managed profile's owner (#1521)"));
                }
                let none: &[Option<UserHandle>] = &[];
                dp::write_get_policy_managed_profiles_reply(&mut p, Some(none));
                Ok(())
            })()
        }
        dp::PACKAGE_HAS_ACTIVE_ADMINS => {
            let a = dp::PackageHasActiveAdmins::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    valid_user(a.user_handle)?;
                    authorize(caller.full_cross_users(a.user_handle)?)?;
                }
                dp::write_package_has_active_admins_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_SYSTEM_UPDATE_POLICY => {
            dp::GetSystemUpdatePolicy::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? && device.policies()?.system_update_policy {
                    return Err(device.unimplemented("a system update policy (#1521)"));
                }
                dp::write_get_system_update_policy_reply::<ParcelableResource>(&mut p, None);
                Ok(())
            })()
        }
        dp::GET_DEVICE_OWNER_COMPONENT => {
            let a = dp::GetDeviceOwnerComponent::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    if !a.calling_user_only {
                        authorize(caller.manages_owners()?)?;
                    }
                }
                dp::write_get_device_owner_component_reply::<Component>(&mut p, None);
                Ok(())
            })()
        }
        dp::GET_DEVICE_OWNER_COMPONENT_ON_USER => {
            let a = dp::GetDeviceOwnerComponentOnUser::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    if caller.user() != a.user_id {
                        authorize(caller.manages_owners()?)?;
                    }
                }
                dp::write_get_device_owner_component_on_user_reply::<Component>(&mut p, None);
                Ok(())
            })()
        }
        dp::HAS_DEVICE_OWNER => {
            dp::HasDeviceOwner::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                unmanaged()?;
                // Neither a default nor a financed device owner calls.
                authorize(caller.manages_owners()?)?;
                dp::write_has_device_owner_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_DEVICE_OWNER_NAME => {
            dp::GetDeviceOwnerName::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    authorize(caller.manages_owners()?)?;
                }
                dp::write_get_device_owner_name_reply(&mut p, &None);
                Ok(())
            })()
        }
        dp::GET_DEVICE_OWNER_USER_ID => {
            dp::GetDeviceOwnerUserId::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    authorize(caller.manages_users()?)?;
                }
                dp::write_get_device_owner_user_id_reply(&mut p, USER_NULL);
                Ok(())
            })()
        }
        dp::GET_PROFILE_OWNER_AS_USER => {
            let a = dp::GetProfileOwnerAsUser::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    valid_user(a.user_handle)?;
                    authorize(
                        caller.cross_users(a.user_handle)?
                            || caller.full_cross_users(a.user_handle)?,
                    )?;
                }
                dp::write_get_profile_owner_as_user_reply::<Component>(&mut p, None);
                Ok(())
            })()
        }
        dp::GET_PROFILE_OWNER_OR_DEVICE_OWNER_SUPERVISION_COMPONENT => {
            let a = dp::GetProfileOwnerOrDeviceOwnerSupervisionComponent::<UserHandle>::read(r)
                .map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    if a.user_handle.is_none() {
                        return Err(Exception::new(
                            EX_NULL_POINTER,
                            "Attempt to invoke virtual method 'int android.os.UserHandle.getIdentifier()' on a null object reference",
                        ));
                    }
                }
                // No owner to be the supervision component.
                dp::write_get_profile_owner_or_device_owner_supervision_component_reply::<Component>(
                    &mut p, None,
                );
                Ok(())
            })()
        }
        dp::GET_PROFILE_OWNER_NAME => {
            let a = dp::GetProfileOwnerName::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    authorize(caller.manages_owners()?)?;
                    // `getProfileOwnerNameUnchecked` asks
                    // `getProfileOwnerAsUser`, with its checks.
                    valid_user(a.user_handle)?;
                    authorize(
                        caller.cross_users(a.user_handle)?
                            || caller.full_cross_users(a.user_handle)?,
                    )?;
                }
                dp::write_get_profile_owner_name_reply(&mut p, &None);
                Ok(())
            })()
        }
        dp::IS_ORGANIZATION_OWNED_DEVICE_WITH_MANAGED_PROFILE => {
            dp::IsOrganizationOwnedDeviceWithManagedProfile::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                }
                // No managed profile has an owner.
                dp::write_is_organization_owned_device_with_managed_profile_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_PERMITTED_ACCESSIBILITY_SERVICES => {
            let a = dp::GetPermittedAccessibilityServices::<ComponentName>::read(r)
                .map_err(|_| BAD_VALUE)?;
            (|| {
                // Only an owner, by its admin, asks.
                if feature()? {
                    let admin = a.admin.ok_or_else(|| null("ComponentName"))?;
                    unmanaged()?;
                    return Err(caller.not_admin(&admin));
                }
                dp::write_get_permitted_accessibility_services_reply(&mut p, &None);
                Ok(())
            })()
        }
        dp::GET_PERMITTED_ACCESSIBILITY_SERVICES_FOR_USER => {
            dp::GetPermittedAccessibilityServicesForUser::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    authorize(caller.manages_users()? || caller.permitted(QUERY_ADMIN_POLICY)?)?;
                }
                // No admin of the user or its profiles keeps a list.
                dp::write_get_permitted_accessibility_services_for_user_reply(&mut p, &None);
                Ok(())
            })()
        }
        dp::GET_PERMITTED_INPUT_METHODS => {
            let a =
                dp::GetPermittedInputMethods::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    caller.identity(a.admin.as_ref(), a.caller_package_name.as_deref())?;
                }
                // No `PERMITTED_INPUT_METHODS` policy.
                dp::write_get_permitted_input_methods_reply(&mut p, &None);
                Ok(())
            })()
        }
        dp::GET_PERMITTED_INPUT_METHODS_AS_USER => {
            let a = dp::GetPermittedInputMethodsAsUser::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                unmanaged()?;
                authorize(caller.full_cross_users(a.user_id)?)?;
                authorize(caller.manages_users()? || caller.permitted(QUERY_ADMIN_POLICY)?)?;
                dp::write_get_permitted_input_methods_as_user_reply(&mut p, &None);
                Ok(())
            })()
        }
        dp::GET_CROSS_PROFILE_CALLER_ID_DISABLED
        | dp::GET_CROSS_PROFILE_CONTACTS_SEARCH_DISABLED
        | dp::GET_BLUETOOTH_CONTACT_SHARING_DISABLED => {
            let who = match code {
                dp::GET_CROSS_PROFILE_CALLER_ID_DISABLED => {
                    dp::GetCrossProfileCallerIdDisabled::<ComponentName>::read(r).map(|a| a.who)
                }
                dp::GET_CROSS_PROFILE_CONTACTS_SEARCH_DISABLED => {
                    dp::GetCrossProfileContactsSearchDisabled::<ComponentName>::read(r)
                        .map(|a| a.who)
                }
                _ => {
                    dp::GetBluetoothContactSharingDisabled::<ComponentName>::read(r).map(|a| a.who)
                }
            }
            .map_err(|_| BAD_VALUE)?;
            (|| {
                // Only an owner, by its admin, asks.
                if feature()? {
                    let who = who.ok_or_else(|| null("ComponentName"))?;
                    unmanaged()?;
                    return Err(caller.not_admin(&who));
                }
                // The three reply a boolean alike.
                dp::write_get_cross_profile_caller_id_disabled_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_CROSS_PROFILE_CALLER_ID_DISABLED_FOR_USER
        | dp::GET_CROSS_PROFILE_CONTACTS_SEARCH_DISABLED_FOR_USER => {
            let user = match code {
                dp::GET_CROSS_PROFILE_CALLER_ID_DISABLED_FOR_USER => {
                    dp::GetCrossProfileCallerIdDisabledForUser::read(r).map(|a| a.user_id)
                }
                _ => dp::GetCrossProfileContactsSearchDisabledForUser::read(r).map(|a| a.user_id),
            }
            .map_err(|_| BAD_VALUE)?;
            (|| {
                unmanaged()?;
                valid_user(user)?;
                authorize(caller.cross_users(user)?)?;
                // The user has no profile owner; both reply a boolean alike.
                dp::write_get_cross_profile_caller_id_disabled_for_user_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_AUTO_TIME_REQUIRED => {
            dp::GetAutoTimeRequired::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                }
                // No admin set `DISALLOW_CONFIG_DATE_TIME`.
                dp::write_get_auto_time_required_reply(&mut p, false);
                Ok(())
            })()
        }
        dp::GET_AUTO_TIME_ENABLED | dp::GET_AUTO_TIME_ZONE_ENABLED => {
            let (flag, permission, setting) = match code {
                dp::GET_AUTO_TIME_ENABLED => {
                    (SET_AUTO_TIME_ENABLED_COEXISTENCE, SET_TIME, "auto_time")
                }
                _ => (
                    SET_AUTO_TIME_ZONE_ENABLED_COEXISTENCE,
                    SET_TIME_ZONE,
                    "auto_time_zone",
                ),
            };
            let (who, package) = match code {
                dp::GET_AUTO_TIME_ENABLED => dp::GetAutoTimeEnabled::<ComponentName>::read(r)
                    .map(|a| (a.who, a.caller_package_name)),
                _ => dp::GetAutoTimeZoneEnabled::<ComponentName>::read(r)
                    .map(|a| (a.who, a.caller_package_name)),
            }
            .map_err(|_| BAD_VALUE)?;
            (|| {
                let enabled = match feature()? {
                    false => false,
                    true => {
                        unmanaged()?;
                        caller.identity(who.as_ref(), None)?;
                        // Without the flag only an owner, by its admin, asks.
                        if !device.flag(flag)? {
                            return Err(null("ComponentName"));
                        }
                        authorize(caller.has_permission(permission, package.as_deref())?)?;
                        int_or(device.setting(Table::Global, setting)?, 0) > 0
                    }
                };
                // Both reply a boolean alike.
                dp::write_get_auto_time_enabled_reply(&mut p, enabled);
                Ok(())
            })()
        }
        dp::IS_MANAGED_PROFILE => {
            let a = dp::IsManagedProfile::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                let admin = a.admin.ok_or_else(|| null("ComponentName"))?;
                unmanaged()?;
                // Only an owner, by its admin, asks.
                Err(caller.not_admin(&admin))
            })()
        }
        dp::GET_USER_PROVISIONING_STATE => {
            let a = dp::GetUserProvisioningState::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                let state = match feature()? {
                    false => STATE_USER_UNMANAGED,
                    true => {
                        authorize(caller.manages_owners()?)?;
                        if a.user_handle != caller.user() {
                            authorize(
                                caller.manages_users()?
                                    || caller.permitted(INTERACT_ACROSS_USERS)?,
                            )?;
                        }
                        device.policies()?.user(a.user_handle).provisioning_state
                    }
                };
                dp::write_get_user_provisioning_state_reply(&mut p, state);
                Ok(())
            })()
        }
        dp::IS_DEVICE_PROVISIONED => {
            dp::IsDeviceProvisioned::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                authorize(caller.manages_users()?)?;
                // `updateUserSetupCompleteAndPaired` takes a completed setup
                // from the setting at boot; it never goes back.
                let provisioned = device.policies()?.user(0).setup_complete
                    || int_or(device.setting(Table::Secure, "user_setup_complete")?, 0) != 0;
                dp::write_is_device_provisioned_reply(&mut p, provisioned);
                Ok(())
            })()
        }
        dp::IS_NETWORK_LOGGING_ENABLED => {
            let a = dp::IsNetworkLoggingEnabled::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    unmanaged()?;
                    caller.identity(a.admin.as_ref(), a.package_name.as_deref())?;
                    // No owner or delegate calls.
                    authorize(caller.manages_users()?)?;
                }
                dp::write_is_network_logging_enabled_reply(&mut p, false);
                Ok(())
            })()
        }
        // `DeviceManagementResourcesProvider.getString`, which only loads
        // with the feature: none of a string no update replaced.
        dp::GET_STRING => {
            let a = dp::GetString::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                if feature()? {
                    let updated = &device.policies()?.updated_strings;
                    if a.string_id.as_ref().is_some_and(|id| updated.contains(id)) {
                        return Err(device
                            .unimplemented("an updated device policy resource string (#1521)"));
                    }
                }
                dp::write_get_string_reply::<ParcelableResource>(&mut p, None);
                Ok(())
            })()
        }
        dp::GET_DEVICE_OWNER_TYPE => {
            dp::GetDeviceOwnerType::<ComponentName>::read(r).map_err(|_| BAD_VALUE)?;
            (|| {
                unmanaged()?;
                // `verifyDeviceOwnerTypePreconditionsLocked`.
                Err(Exception::new(EX_ILLEGAL_STATE, "there is no device owner"))
            })()
        }
        code => {
            let name = dp::METHODS
                .iter()
                .find(|(c, _)| *c == code)
                .map_or("an unknown call", |(_, n)| n);
            Err(device.unimplemented(&format!("{name} (#1521)")))
        }
    };
    reply(result.map(|()| p))
}

pub struct DevicePolicy {
    host: Arc<AppHost>,
}

impl DevicePolicy {
    pub fn new(host: Arc<AppHost>) -> DevicePolicy {
        DevicePolicy { host }
    }
}

impl Device for DevicePolicy {
    fn usb_data_signaling_enabled(&self) -> Result<bool, Exception> {
        let mut data = Parcel::new();
        defaults::IsUsbDataSignalingEnabled {
            policies: Some(self.host.device_policies()?.engine_xml.clone()),
        }
        .write(&mut data);
        let result = self
            .host
            .device_policy_default(defaults::IS_USB_DATA_SIGNALING_ENABLED, &data)?;
        defaults::read_is_usb_data_signaling_enabled_reply(&mut result.reader())
            .map_err(default_transport)?
    }
    fn content_protection_default(&self, user_id: i32) -> Result<i32, Exception> {
        let mut data = Parcel::new();
        defaults::GetContentProtectionPolicyDefault { user_id }.write(&mut data);
        let result = self
            .host
            .device_policy_default(defaults::GET_CONTENT_PROTECTION_POLICY_DEFAULT, &data)?;
        defaults::read_get_content_protection_policy_default_reply(&mut result.reader())
            .map_err(default_transport)?
    }

    fn string_array(&self, name: &str) -> Result<Vec<Option<String>>, Exception> {
        self.host
            .framework()?
            .framework_string_array(name)
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, format!("framework array/{name} absent"))
            })
    }

    fn feature(&self) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .system
            .features
            .iter()
            .any(|(n, v)| n == FEATURE_DEVICE_ADMIN && *v >= 0))
    }

    fn policies(&self) -> Result<&Policies, Exception> {
        self.host.device_policies()
    }

    fn permitted(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    fn app_id(&self, package: &str, user: i32) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.packages()?;
        // `getUserStateOrDefault`: installed for a user it has no state of.
        Ok(state
            .packages
            .get(package)
            .filter(|ps| ps.users.get(&user).is_none_or(|u| u.installed))
            .map(|ps| ps.app_id))
    }

    fn target_sdk(&self, package: &str) -> Result<Option<i32>, Exception> {
        Ok(self
            .host
            .application_info(package, 0, None)?
            .map(|ai| ai.target_sdk_version))
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    fn setting(&self, table: Table, name: &str) -> Result<Option<String>, Exception> {
        Ok(self.host.inputs()?.settings.get(table, name))
    }

    fn property(&self, name: &str) -> Option<String> {
        (self.host.configured.get()?.properties)(name)
    }

    fn managed_profiles(&self, user: i32) -> Result<Vec<i32>, Exception> {
        super::user_service::managed_profiles(&self.host, user)
    }

    fn profiles(&self, user: i32) -> Vec<i32> {
        super::user_service::profile_ids(user)
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("device_policy", what)
    }
}

fn default_transport(status: i32) -> Exception {
    Exception::new(
        EX_ILLEGAL_STATE,
        format!("device policy defaults: binder status {status}"),
    )
}

impl AppHost {
    pub(super) fn attach_device_policy_defaults(
        &self,
        owner: Option<Binder>,
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = owner else {
            return Err(Exception::illegal_argument(
                "no device policy defaults bridge",
            ));
        };
        let owner = Arc::new(self.process.strong(handle));
        let weak = self.me.clone();
        self.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(host) = weak.upgrade() {
                    let mut current = host.device_policy_defaults.lock().unwrap();
                    if current.as_ref().is_some_and(|p| p.handle == handle) {
                        current.take();
                    }
                }
            }),
        );
        *self.device_policy_defaults.lock().unwrap() = Some(owner);
        Ok(())
    }

    fn device_policy_default(
        &self,
        code: u32,
        data: &Parcel,
    ) -> Result<aim_binder_host::local::Received, Exception> {
        let owner = self
            .device_policy_defaults
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| {
                self.unimplemented_named(
                    "device_policy",
                    "original defaults bridge is not attached (#1521)",
                )
            })?;
        owner.transact(code, data, false).map_err(default_transport)
    }
}

impl Service for DevicePolicy {
    fn descriptor(&self) -> &str {
        dp::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        answer(self, call.sender_euid as i32, call.code, &mut call.data)
    }
}

#[cfg(test)]
mod tests {
    use aim_android_xml::{Node, Value};

    use super::*;

    /// A guest `/data` of its own, empty.
    fn data(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aim-device-policy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("system/users/0")).unwrap();
        dir
    }

    fn element(name: &str, attrs: &[(&str, Value)], children: Vec<Element>) -> Element {
        Element {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
            content: children.into_iter().map(Node::Element).collect(),
        }
    }

    fn string(s: &str) -> Value {
        Value::String(s.into())
    }

    fn write(path: &Path, root: &Element) {
        fs::write(path, aim_android_xml::abx::write(root).unwrap()).unwrap();
    }

    /// The strings an update replaced, read as `ResourcesReaderWriter`
    /// reads them: up to a tag it does not know.
    #[test]
    fn updated_strings_are_read_up_to_an_unknown_tag() {
        let dir = data("resources");
        let entry = |id: &str| element("string-entry", &[("source-id", string(id))], vec![]);
        let unknown = element("other", &[], vec![]);
        write(
            &dir.join("system").join(UPDATED_RESOURCES_XML),
            &element("root", &[], vec![entry("a"), unknown, entry("b")]),
        );
        assert_eq!(Policies::load(&dir).unwrap().updated_strings, ["a"]);
    }

    /// `OwnersData`'s files as a device without owners keeps them (the
    /// policy engine's migration only), with a device owner, behind a tag
    /// the reader stops at, and a user's profile owner.
    #[test]
    fn owners_are_read_as_owners_data_reads_them() {
        let dir = data("owners");
        assert!(!Policies::load(&dir).unwrap().owned());
        let migration = element(
            "policy-engine-migration",
            &[("migratedToPolicyEngine", Value::Bool(true))],
            vec![],
        );
        let owner_file = dir.join("system").join(DEVICE_OWNER_XML);
        write(&owner_file, &element("root", &[], vec![migration.clone()]));
        assert!(!Policies::load(&dir).unwrap().owned());
        let owner = element(
            "device-owner",
            &[("component", string("com.example.dpc/.Admin"))],
            vec![],
        );
        write(
            &owner_file,
            &element("root", &[], vec![migration, owner.clone()]),
        );
        assert!(Policies::load(&dir).unwrap().owned());
        // An unknown tag ends the read; a component that does not unflatten
        // is no owner.
        let unknown = element("something-else", &[], vec![]);
        write(
            &owner_file,
            &element("root", &[], vec![unknown, owner.clone()]),
        );
        assert!(!Policies::load(&dir).unwrap().owned());
        let unparsable = element(
            "device-owner",
            &[("component", string("com.example.dpc"))],
            vec![],
        );
        write(&owner_file, &element("root", &[], vec![unparsable]));
        assert!(!Policies::load(&dir).unwrap().owned());
        // `AtomicFile`'s legacy backup is read in the file's place.
        write(
            &suffixed(&owner_file, ".bak"),
            &element("root", &[], vec![owner]),
        );
        assert!(Policies::load(&dir).unwrap().owned());
        fs::remove_file(suffixed(&owner_file, ".bak")).unwrap();
        // `mSystemUpdatePolicy`, with or without an owner, while no
        // unknown tag ends the read before it.
        let update = element(
            "system-update-policy",
            &[("policy_type", Value::Int(1))],
            vec![],
        );
        write(&owner_file, &element("root", &[], vec![update.clone()]));
        assert!(Policies::load(&dir).unwrap().system_update_policy);
        write(
            &owner_file,
            &element(
                "root",
                &[],
                vec![element("something-else", &[], vec![]), update],
            ),
        );
        assert!(!Policies::load(&dir).unwrap().system_update_policy);
        fs::remove_file(&owner_file).unwrap();
        let profile = element(
            "profile-owner",
            &[("component", string("com.example.dpc/.Admin"))],
            vec![],
        );
        fs::create_dir_all(dir.join("system/users/10")).unwrap();
        write(
            &dir.join("system/users/10").join(PROFILE_OWNER_XML),
            &element("root", &[], vec![profile]),
        );
        assert!(Policies::load(&dir).unwrap().owned());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `DevicePolicyData.load`: user 0's file in `/data/system`, the
    /// others' in their directories, the temporary file when it alone
    /// exists; admins; the policy engine's entries.
    #[test]
    fn user_data_and_policies_are_read_as_the_original_reads_them() {
        let dir = data("users");
        let policies = Policies::load(&dir).unwrap();
        assert_eq!(
            policies.user(0),
            UserData {
                setup_complete: false,
                provisioning_state: STATE_USER_UNMANAGED
            }
        );
        assert!(!policies.admins && !policies.policies);
        let root = element(
            "policies",
            &[
                ("setup-complete", Value::Bool(true)),
                ("provisioning-state", Value::Int(3)),
            ],
            vec![element(
                "failed-password-attempts",
                &[("value", Value::Int(0))],
                vec![],
            )],
        );
        write(&dir.join("system").join(DEVICE_POLICIES_XML), &root);
        fs::create_dir_all(dir.join("system/users/10")).unwrap();
        let text = "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<policies setup-complete=\"true\" />\n";
        fs::write(
            suffixed(
                &dir.join("system/users/10").join(DEVICE_POLICIES_XML),
                ".tmp",
            ),
            text,
        )
        .unwrap();
        let policies = Policies::load(&dir).unwrap();
        assert_eq!(
            policies.user(0),
            UserData {
                setup_complete: true,
                provisioning_state: 3
            }
        );
        assert_eq!(
            policies.user(10),
            UserData {
                setup_complete: true,
                provisioning_state: STATE_USER_UNMANAGED
            }
        );
        assert_eq!(policies.user(11), UserData::default());
        assert!(!policies.admins);
        let admin = element("admin", &[("name", string("com.example/.Admin"))], vec![]);
        write(
            &dir.join("system").join(DEVICE_POLICIES_XML),
            &element("policies", &[], vec![admin]),
        );
        assert!(Policies::load(&dir).unwrap().admins);
        // An empty engine keeps only its size limit.
        let engine = dir.join("system").join(DEVICE_POLICY_STATE_XML);
        let limit = element("max-policy-size-limit", &[("size", Value::Int(-1))], vec![]);
        fs::write(
            &engine,
            aim_android_xml::abx::write_sequence(std::slice::from_ref(&limit)).unwrap(),
        )
        .unwrap();
        assert!(!Policies::load(&dir).unwrap().policies);
        let entry = element("global-policy-entry", &[], vec![]);
        fs::write(
            &engine,
            aim_android_xml::abx::write_sequence(&[entry, limit]).unwrap(),
        )
        .unwrap();
        assert!(Policies::load(&dir).unwrap().policies);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn components_unflatten_as_component_name_does() {
        assert!(unflattens("p/.C"));
        assert!(unflattens("/C"));
        assert!(!unflattens("p/"));
        assert!(!unflattens("p"));
        let c = ComponentName {
            package: "p".into(),
            class: "p.C".into(),
        };
        assert_eq!(component_string(&c), "ComponentInfo{p/p.C}");
    }

    /// `getStorageEncryptionStatus`: file-based encryption per user, and
    /// active for apps of M and before.
    #[test]
    fn encryption_status_follows_the_crypto_type() {
        assert_eq!(
            encryption_status(Some("file"), 34),
            ENCRYPTION_STATUS_ACTIVE_PER_USER
        );
        assert_eq!(encryption_status(Some("file"), M), ENCRYPTION_STATUS_ACTIVE);
        assert_eq!(encryption_status(None, 34), ENCRYPTION_STATUS_UNSUPPORTED);
        assert_eq!(
            encryption_status(Some("block"), 34),
            ENCRYPTION_STATUS_UNSUPPORTED
        );
    }

    const APP: i32 = 10100;
    const OTHER_APP: i32 = 10101;

    /// A device with `com.example` (app id 10100) in every user, the
    /// permissions by uid, user 0's settings and the flags on or off.
    struct Fake {
        usb_enabled: bool,
        content_protection: bool,
        keyguard_coexistence: bool,
        feature: bool,
        policies: Policies,
        permissions: Vec<(i32, &'static str)>,
        settings: Vec<(&'static str, &'static str)>,
        flags: bool,
        /// User 0's managed profiles.
        managed: Vec<i32>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Fake {
                usb_enabled: true,
                content_protection: false,
                keyguard_coexistence: true,
                feature: true,
                policies: Policies::default(),
                permissions: vec![],
                settings: vec![],
                flags: true,
                managed: vec![],
            }
        }
    }

    impl Device for Fake {
        fn usb_data_signaling_enabled(&self) -> Result<bool, Exception> {
            Ok(self.usb_enabled)
        }
        fn content_protection_default(&self, _user: i32) -> Result<i32, Exception> {
            Ok(0)
        }

        fn string_array(&self, name: &str) -> Result<Vec<Option<String>>, Exception> {
            Ok(match name {
                "cross_profile_apps" => vec![Some("platform".into()), Some("both".into())],
                "vendor_cross_profile_apps" => vec![Some("vendor".into()), Some("both".into())],
                _ => panic!("unexpected resource {name}"),
            })
        }

        fn feature(&self) -> Result<bool, Exception> {
            Ok(self.feature)
        }

        fn policies(&self) -> Result<&Policies, Exception> {
            Ok(&self.policies)
        }

        fn permitted(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
            Ok(app_id(uid) == SYSTEM_UID || self.permissions.contains(&(uid, permission)))
        }

        fn app_id(&self, package: &str, _user: i32) -> Result<Option<i32>, Exception> {
            Ok((package == "com.example").then_some(APP))
        }

        fn target_sdk(&self, package: &str) -> Result<Option<i32>, Exception> {
            Ok((package == "com.example").then_some(36))
        }

        fn flag(&self, name: &str) -> Result<bool, Exception> {
            Ok(match name {
                MANAGE_CONTENT_PROTECTION => self.content_protection,
                SET_KEYGUARD_DISABLED_FEATURES_COEXISTENCE => self.keyguard_coexistence,
                _ => self.flags,
            })
        }

        fn setting(&self, _table: Table, name: &str) -> Result<Option<String>, Exception> {
            Ok(self
                .settings
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string()))
        }

        fn property(&self, name: &str) -> Option<String> {
            (name == "ro.crypto.type").then(|| "file".into())
        }

        fn managed_profiles(&self, user: i32) -> Result<Vec<i32>, Exception> {
            Ok(self.managed.iter().copied().filter(|_| user == 0).collect())
        }

        fn profiles(&self, user: i32) -> Vec<i32> {
            if user == 0 {
                std::iter::once(0)
                    .chain(self.managed.iter().copied())
                    .collect()
            } else {
                vec![]
            }
        }

        fn unimplemented(&self, what: &str) -> Exception {
            Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION, what)
        }
    }

    /// `code`'s reply to `uid`, its arguments written by `args`.
    fn call(device: &Fake, uid: i32, code: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
        let mut data = Parcel::new();
        args(&mut data);
        answer(device, uid, code, &mut data.reader()).unwrap()
    }

    fn exception(reply: &Parcel) -> Exception {
        reply.reader().read_exception().unwrap().unwrap_err()
    }

    fn not_authorized() -> Exception {
        Exception::security("Calling identity is not authorized")
    }

    #[test]
    fn usb_signaling_checks_package_identity_without_a_privileged_permission() {
        let mut device = Fake::default();
        let ask = |device: &Fake, uid, package| {
            call(device, uid, dp::IS_USB_DATA_SIGNALING_ENABLED, |p| {
                dp::IsUsbDataSignalingEnabled {
                    caller_package: package,
                }
                .write(p)
            })
        };
        for enabled in [true, false] {
            device.usb_enabled = enabled;
            // Owners and other policies do not block this engine read.
            device.policies.owned = true;
            for package in [None, Some("com.example".into())] {
                let p = ask(&device, APP, package);
                assert_eq!(
                    dp::read_is_usb_data_signaling_enabled_reply(&mut p.reader()).unwrap(),
                    Ok(enabled)
                );
            }
        }
        assert_eq!(
            exception(&ask(&device, OTHER_APP, Some("com.example".into()))),
            Exception::security("Caller with uid 10101 is not com.example")
        );
    }

    fn permission_required(permission: &str) -> Exception {
        Exception::security(format!(
            "Caller does not have the required permissions for this user. Permission required: {permission}."
        ))
    }

    #[test]
    fn original_defaults_bridge_authenticates_attachment_and_preserves_java_results() {
        struct Original;
        impl Service for Original {
            fn descriptor(&self) -> &str {
                defaults::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.sender_euid, 1000);
                let mut p = Parcel::new();
                match call.code {
                    defaults::IS_USB_DATA_SIGNALING_ENABLED => {
                        let a = defaults::IsUsbDataSignalingEnabled::read(&mut call.data)?;
                        assert_eq!(a.policies, Some(b"<global-policy-entry/>".to_vec()));
                        defaults::write_is_usb_data_signaling_enabled_reply(&mut p, false);
                    }
                    defaults::GET_CONTENT_PROTECTION_POLICY_DEFAULT => {
                        let a = defaults::GetContentProtectionPolicyDefault::read(&mut call.data)?;
                        if a.user_id == 10 {
                            p.write_exception(&Exception::security("original exception"));
                        } else if a.user_id == 11 {
                            return Err(BAD_VALUE);
                        } else {
                            defaults::write_get_content_protection_policy_default_reply(&mut p, 37);
                        }
                    }
                    _ => return Err(BAD_VALUE),
                }
                Ok(p)
            }
        }
        crate::system::with_discovery_binders(|host, _, _, export| {
            use aim_service_aidl::dev_aim_server_isystemprocess as sp;
            let service = DevicePolicy::new(host.clone());
            std::fs::write(
                host.data().unwrap().join("system/device_policy_state.xml"),
                b"<global-policy-entry/>",
            )
            .unwrap();
            let error = service.content_protection_default(0).unwrap_err();
            assert_eq!(
                error.code,
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
            );
            assert!(error.message.contains("#1521"));
            for owner in [None, Some(Binder::Local(1))] {
                assert!(host.attach_device_policy_defaults(owner).is_err());
            }
            let owner = export(Arc::new(Original));
            let mut data = Parcel::new();
            sp::AttachDevicePolicyDefaults {
                defaults: Some(Binder::Handle(owner.handle)),
            }
            .write(&mut data);
            let system = super::super::system_process::SystemProcess::new(host.clone());
            for uid in [APP as u32, 1000] {
                let p = system
                    .transact(&mut Call {
                        code: sp::ATTACH_DEVICE_POLICY_DEFAULTS,
                        flags: 0,
                        sender_pid: 1,
                        sender_euid: uid,
                        data: data.reader(),
                    })
                    .unwrap();
                let result = sp::read_attach_device_policy_defaults_reply(&mut p.reader()).unwrap();
                if uid == 1000 {
                    result.unwrap();
                } else {
                    assert_eq!(
                        result.unwrap_err().code,
                        aim_binder_host::parcel::EX_SECURITY
                    );
                }
            }
            assert_eq!(service.content_protection_default(0).unwrap(), 37);
            assert!(!service.usb_data_signaling_enabled().unwrap());
            assert_eq!(
                service.content_protection_default(10).unwrap_err(),
                Exception::security("original exception")
            );
            assert_eq!(
                service.content_protection_default(11).unwrap_err().code,
                EX_ILLEGAL_STATE
            );
        });
    }

    /// DPMS.get/setContentProtectionPolicy, android-16.0.0_r1;
    /// the flag-off path precedes identity checks, and the flag-on query
    /// permits QUERY_ADMIN_POLICY or the policy and cross-user permissions.
    #[test]
    fn content_protection_and_its_setter_follow_the_original_flag_and_checks() {
        let mut device = Fake::default();
        let get = |d: &Fake, package: Option<&str>, user_id| {
            call(d, APP, dp::GET_CONTENT_PROTECTION_POLICY, |p| {
                dp::GetContentProtectionPolicy::<Component> {
                    who: None,
                    caller_package_name: package.map(Into::into),
                    user_id,
                }
                .write(p)
            })
        };
        let set = |d: &Fake, who, package: Option<&str>| {
            call(d, APP, dp::SET_CONTENT_PROTECTION_POLICY, |p| {
                dp::SetContentProtectionPolicy::<Component> {
                    who,
                    caller_package_name: package.map(Into::into),
                    policy: 1,
                }
                .write(p)
            })
        };
        assert_eq!(
            dp::read_get_content_protection_policy_reply(
                &mut get(&device, Some("other"), -1).reader()
            )
            .unwrap(),
            Ok(0)
        );
        assert_eq!(
            dp::read_set_content_protection_policy_reply(
                &mut set(&device, Some(Component("other", "Admin")), None).reader()
            )
            .unwrap(),
            Ok(())
        );
        device.content_protection = true;
        assert_eq!(exception(&get(&device, None, 0)), null("callerPackageName"));
        let package = Some("com.example");
        assert_eq!(
            exception(&get(&device, package, 0)),
            permission_required(MANAGE_DEVICE_POLICY_CONTENT_PROTECTION)
        );
        assert_eq!(
            exception(&set(&device, None, package)),
            permission_required(MANAGE_DEVICE_POLICY_CONTENT_PROTECTION)
        );
        assert_eq!(
            exception(&set(
                &device,
                Some(Component("com.example", "com.example.Admin")),
                package
            )),
            Caller {
                device: &device,
                uid: APP
            }
            .not_admin(&ComponentName {
                package: "com.example".into(),
                class: "com.example.Admin".into()
            })
        );
        assert_eq!(
            exception(&get(&device, Some("other"), 0)),
            Exception::security(format!("Caller with uid {APP} is not other"))
        );
        device
            .permissions
            .push((APP, MANAGE_DEVICE_POLICY_CONTENT_PROTECTION));
        assert_eq!(
            dp::read_get_content_protection_policy_reply(&mut get(&device, package, 0).reader())
                .unwrap(),
            Ok(0)
        );
        assert_eq!(
            exception(&get(&device, package, 10)),
            permission_required(MANAGE_DEVICE_POLICY_ACROSS_USERS)
        );
        device
            .permissions
            .push((APP, MANAGE_DEVICE_POLICY_ACROSS_USERS));
        assert_eq!(
            dp::read_get_content_protection_policy_reply(&mut get(&device, package, 10).reader())
                .unwrap(),
            Ok(0)
        );
        let error = exception(&set(&device, None, package));
        assert_eq!(
            error.code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
        assert!(error.message.contains("#1521"));
        device.permissions = vec![(APP, QUERY_ADMIN_POLICY)];
        assert_eq!(
            dp::read_get_content_protection_policy_reply(&mut get(&device, package, 10).reader())
                .unwrap(),
            Ok(0)
        );
        assert_eq!(
            exception(&set(&device, None, package)),
            permission_required(MANAGE_DEVICE_POLICY_CONTENT_PROTECTION)
        );
        device.policies.owned = true;
        assert_eq!(
            exception(&get(&device, package, 0)).code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
    }

    /// DPMS.set/getUserControlDisabledPackages: packages null is checked
    /// first, including for clearing; neither a component nor a package
    /// supplied by a non-admin bypasses the original caller checks.
    #[test]
    fn user_control_disabled_packages_checks_both_directions_and_clearing() {
        let mut device = Fake::default();
        let set = |d: &Fake, who, package: Option<&str>, packages| {
            call(d, APP, dp::SET_USER_CONTROL_DISABLED_PACKAGES, |p| {
                dp::SetUserControlDisabledPackages::<Component> {
                    admin: who,
                    caller_package_name: package.map(Into::into),
                    packages,
                }
                .write(p)
            })
        };
        let get = |d: &Fake, who, package: Option<&str>| {
            call(d, APP, dp::GET_USER_CONTROL_DISABLED_PACKAGES, |p| {
                dp::GetUserControlDisabledPackages::<Component> {
                    admin: who,
                    caller_package_name: package.map(Into::into),
                }
                .write(p)
            })
        };
        assert_eq!(
            exception(&set(
                &device,
                Some(Component("other", "Admin")),
                Some("other"),
                None
            )),
            null("packages")
        );
        assert_eq!(
            exception(&set(&device, None, None, Some(vec![]))),
            null("callerPackageName")
        );
        for packages in [vec![], vec![Some("example.disabled".into())]] {
            assert_eq!(
                exception(&set(&device, None, Some("com.example"), Some(packages))),
                permission_required(MANAGE_DEVICE_POLICY_APPS_CONTROL)
            );
        }
        let who = || Some(Component("com.example", "com.example.Admin"));
        let denied = Exception::security(format!(
            "Admin ComponentInfo{{com.example/com.example.Admin}} does not exist or is not owned by uid {APP}"
        ));
        assert_eq!(exception(&get(&device, who(), Some("com.example"))), denied);
        assert_eq!(
            exception(&set(&device, who(), Some("com.example"), Some(vec![]))),
            denied
        );
        assert_eq!(
            exception(&get(&device, None, Some("other"))),
            Exception::security(format!("Caller with uid {APP} is not other"))
        );
        assert_eq!(
            exception(&get(&device, None, Some("com.example"))),
            permission_required(MANAGE_DEVICE_POLICY_APPS_CONTROL)
        );
        device.permissions.push((APP, QUERY_ADMIN_POLICY));
        assert_eq!(
            dp::read_get_user_control_disabled_packages_reply(
                &mut get(&device, None, Some("com.example")).reader()
            )
            .unwrap(),
            Ok(Some(vec![]))
        );
        device.permissions = vec![(APP, MANAGE_DEVICE_POLICY_APPS_CONTROL)];
        assert_eq!(
            dp::read_get_user_control_disabled_packages_reply(
                &mut get(&device, None, Some("com.example")).reader()
            )
            .unwrap(),
            Ok(Some(vec![]))
        );
        let error = exception(&set(&device, None, Some("com.example"), Some(vec![])));
        assert_eq!(
            error.code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
        assert!(error.message.contains("#1521"));
        // Unlike keyguard, these calls have no mHasFeature early return.
        device.feature = false;
        device.permissions.clear();
        assert_eq!(
            exception(&set(&device, None, Some("com.example"), Some(vec![]))),
            permission_required(MANAGE_DEVICE_POLICY_APPS_CONTROL)
        );
    }

    /// DPMS.setKeyguardDisabledFeatures and its getter on an admin-less
    /// device (CTS DevicePolicyManagerTest): feature, coexistence flag,
    /// admin identity, null component and policy permission ordering.
    #[test]
    fn keyguard_setter_rejects_non_admins_and_getter_keeps_the_default() {
        let mut device = Fake::default();
        let set = |d: &Fake, who, package: Option<&str>, which, parent| {
            call(d, APP, dp::SET_KEYGUARD_DISABLED_FEATURES, |p| {
                dp::SetKeyguardDisabledFeatures::<Component> {
                    who,
                    caller_package_name: package.map(Into::into),
                    which,
                    parent,
                }
                .write(p)
            })
        };
        for coexistence in [false, true] {
            device.keyguard_coexistence = coexistence;
            for which in [0, 0x7fffffff] {
                for parent in [false, true] {
                    let r = set(
                        &device,
                        Some(Component("com.example", "com.example.Admin")),
                        Some("com.example"),
                        which,
                        parent,
                    );
                    assert_eq!(
                        exception(&r),
                        Exception::security(format!(
                            "Admin ComponentInfo{{com.example/com.example.Admin}} does not exist or is not owned by uid {APP}"
                        ))
                    );
                }
            }
            let error = exception(&set(&device, None, Some("com.example"), 0, false));
            assert_eq!(
                error,
                if coexistence {
                    permission_required(MANAGE_DEVICE_POLICY_KEYGUARD)
                } else {
                    null("ComponentName")
                }
            );
            let r = call(&device, APP, dp::GET_KEYGUARD_DISABLED_FEATURES, |p| {
                dp::GetKeyguardDisabledFeatures::<Component> {
                    who: None,
                    user_handle: 0,
                    parent: false,
                }
                .write(p)
            });
            assert_eq!(
                dp::read_get_keyguard_disabled_features_reply(&mut r.reader()).unwrap(),
                Ok(0)
            );
        }
        device
            .permissions
            .push((APP, MANAGE_DEVICE_POLICY_KEYGUARD));
        let error = exception(&set(&device, None, Some("com.example"), 0, false));
        assert_eq!(
            error.code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
        assert!(error.message.contains("#1521"));
        device.feature = false;
        assert_eq!(
            dp::read_set_keyguard_disabled_features_reply(
                &mut set(&device, None, Some("other"), -1, true).reader()
            )
            .unwrap(),
            Ok(())
        );
    }

    #[test]
    fn default_cross_profile_packages_are_the_union_without_permission_or_feature_checks() {
        for feature in [false, true] {
            let device = Fake {
                feature,
                policies: Policies {
                    owned: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let r = call(&device, APP, dp::GET_DEFAULT_CROSS_PROFILE_PACKAGES, |p| {
                dp::GetDefaultCrossProfilePackages {}.write(p)
            });
            let packages = dp::read_get_default_cross_profile_packages_reply(&mut r.reader())
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(packages.len(), 3);
            assert_eq!(
                packages.into_iter().collect::<HashSet<_>>(),
                ["platform", "vendor", "both"]
                    .map(|p| Some(p.into()))
                    .into()
            );
        }
    }

    /// DevicePolicyManagerService.resetDefaultCrossProfileIntentFilters,
    /// android-16.0.0_r1: authorize before returning for a group of <= 1.
    #[test]
    fn reset_cross_profile_filters_checks_permission_before_the_single_user_return() {
        let reset = |device: &Fake, uid, user_id| {
            call(
                device,
                uid,
                dp::RESET_DEFAULT_CROSS_PROFILE_INTENT_FILTERS,
                |p| dp::ResetDefaultCrossProfileIntentFilters { user_id }.write(p),
            )
        };
        let mut device = Fake::default();
        for user in [0, 10] {
            assert_eq!(exception(&reset(&device, APP, user)), not_authorized());
            assert!(
                reset(&device, SYSTEM_UID, user)
                    .reader()
                    .read_exception()
                    .unwrap()
                    .is_ok()
            );
        }
        device.permissions.push((APP, MANAGE_USERS));
        assert_eq!(exception(&reset(&device, APP, 0)), not_authorized());
        device
            .permissions
            .push((APP, MANAGE_PROFILE_AND_DEVICE_OWNERS));
        // Neither a missing feature nor persisted policy changes this decision.
        for feature in [true, false] {
            device.feature = feature;
            device.policies.owned = true;
            assert!(
                reset(&device, APP, 0)
                    .reader()
                    .read_exception()
                    .unwrap()
                    .is_ok()
            );
        }
        device.managed.push(10);
        assert_eq!(
            exception(&reset(&device, APP, 0)).code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
    }

    /// `getSystemUpdatePolicy`: none without a persisted one, which is not
    /// read yet.
    #[test]
    fn the_system_update_policy_is_none_unless_persisted() {
        let mut device = Fake::default();
        let r = call(&device, APP, dp::GET_SYSTEM_UPDATE_POLICY, |p| {
            dp::GetSystemUpdatePolicy {}.write(p)
        });
        let mut reader = r.reader();
        assert!(reader.read_exception().unwrap().is_ok());
        assert_eq!(reader.read_i32().unwrap(), 0);
        device.policies.system_update_policy = true;
        let r = call(&device, APP, dp::GET_SYSTEM_UPDATE_POLICY, |p| {
            dp::GetSystemUpdatePolicy {}.write(p)
        });
        assert_eq!(
            exception(&r).code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
    }

    /// What Settings and ManagedProvisioning read at boot: no password
    /// complexity with no admins, and no managed profiles with owners;
    /// each with its check.
    #[test]
    fn boot_reads_of_an_unmanaged_device() {
        let mut device = Fake::default();
        let complexity = |device: &Fake, uid: i32, user_id: i32| {
            call(
                device,
                uid,
                dp::GET_AGGREGATED_PASSWORD_COMPLEXITY_FOR_USER,
                |p| {
                    dp::GetAggregatedPasswordComplexityForUser {
                        user_id,
                        device_wide_only: false,
                    }
                    .write(p)
                },
            )
        };
        let r = complexity(&device, APP, 0);
        assert_eq!(
            dp::read_get_aggregated_password_complexity_for_user_reply(&mut r.reader()).unwrap(),
            Ok(PASSWORD_COMPLEXITY_NONE)
        );
        assert_eq!(exception(&complexity(&device, APP, 10)), not_authorized());
        let profiles = |device: &Fake, uid: i32| {
            call(device, uid, dp::GET_POLICY_MANAGED_PROFILES, |p| {
                dp::GetPolicyManagedProfiles {
                    user_handle: Some(UserHandle(0)),
                }
                .write(p)
            })
        };
        assert_eq!(exception(&profiles(&device, APP)), not_authorized());
        let r = profiles(&device, SYSTEM_UID);
        let list =
            dp::read_get_policy_managed_profiles_reply::<UserHandle>(&mut r.reader()).unwrap();
        assert!(list.unwrap().is_some_and(|l| l.is_empty()));
        // A managed profile's owner is not modelled.
        device.managed = vec![10];
        assert_eq!(
            exception(&profiles(&device, SYSTEM_UID)).code,
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
        );
    }

    /// An app's reads of its own user on a device without owners, admins
    /// or policies: the original's defaults (CTS `DevicePolicyManagerTest`
    /// on a device without a device owner).
    #[test]
    fn an_unmanaged_device_answers_the_defaults() {
        let device = Fake::default();
        let r = call(&device, APP, dp::GET_PASSWORD_QUALITY, |p| {
            dp::GetPasswordQuality::<Component> {
                who: None,
                user_handle: 0,
                parent: false,
            }
            .write(p)
        });
        assert_eq!(
            dp::read_get_password_quality_reply(&mut r.reader()).unwrap(),
            Ok(PASSWORD_QUALITY_UNSPECIFIED)
        );
        let r = call(&device, APP, dp::GET_CAMERA_DISABLED, |p| {
            let package = Some("com.example".into());
            dp::GetCameraDisabled::<Component> {
                who: None,
                caller_package_name: package,
                user_handle: 0,
                parent: false,
            }
            .write(p)
        });
        assert_eq!(
            dp::read_get_camera_disabled_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        let r = call(&device, APP, dp::GET_KEYGUARD_DISABLED_FEATURES, |p| {
            dp::GetKeyguardDisabledFeatures::<Component> {
                who: None,
                user_handle: 0,
                parent: false,
            }
            .write(p)
        });
        assert_eq!(
            dp::read_get_keyguard_disabled_features_reply(&mut r.reader()).unwrap(),
            Ok(0)
        );
        let r = call(&device, APP, dp::GET_ACTIVE_ADMINS, |p| {
            dp::GetActiveAdmins { user_handle: 0 }.write(p)
        });
        let admins = dp::read_get_active_admins_reply::<ComponentName>(&mut r.reader())
            .unwrap()
            .unwrap();
        assert!(admins.is_none());
        let r = call(&device, APP, dp::IS_ADMIN_ACTIVE, |p| {
            let who = Some(Component("com.example", "com.example.Admin"));
            dp::IsAdminActive::<Component> {
                policy_receiver: who,
                user_handle: 0,
            }
            .write(p)
        });
        assert_eq!(
            dp::read_is_admin_active_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        let r = call(&device, APP, dp::GET_DEVICE_OWNER_COMPONENT, |p| {
            dp::GetDeviceOwnerComponent {
                calling_user_only: true,
            }
            .write(p)
        });
        let owner = dp::read_get_device_owner_component_reply::<ComponentName>(&mut r.reader())
            .unwrap()
            .unwrap();
        assert!(owner.is_none());
        let r = call(&device, APP, dp::GET_PROFILE_OWNER_AS_USER, |p| {
            dp::GetProfileOwnerAsUser { user_handle: 0 }.write(p)
        });
        let owner = dp::read_get_profile_owner_as_user_reply::<ComponentName>(&mut r.reader())
            .unwrap()
            .unwrap();
        assert!(owner.is_none());
        let r = call(&device, APP, dp::GET_STORAGE_ENCRYPTION_STATUS, |p| {
            dp::GetStorageEncryptionStatus {
                caller_package: Some("com.example".into()),
                user_handle: 0,
            }
            .write(p)
        });
        let status = dp::read_get_storage_encryption_status_reply(&mut r.reader()).unwrap();
        assert_eq!(status, Ok(ENCRYPTION_STATUS_ACTIVE_PER_USER));
        let r = call(&device, APP, dp::GET_PERMITTED_INPUT_METHODS, |p| {
            let package = Some("com.example".into());
            dp::GetPermittedInputMethods::<Component> {
                admin: None,
                caller_package_name: package,
                parent: false,
            }
            .write(p)
        });
        assert_eq!(
            dp::read_get_permitted_input_methods_reply(&mut r.reader()).unwrap(),
            Ok(None)
        );
        let r = call(
            &device,
            APP,
            dp::IS_ORGANIZATION_OWNED_DEVICE_WITH_MANAGED_PROFILE,
            |p| dp::IsOrganizationOwnedDeviceWithManagedProfile {}.write(p),
        );
        let owned =
            dp::read_is_organization_owned_device_with_managed_profile_reply(&mut r.reader())
                .unwrap();
        assert_eq!(owned, Ok(false));
        let r = call(
            &device,
            APP,
            dp::GET_CROSS_PROFILE_CALLER_ID_DISABLED_FOR_USER,
            |p| dp::GetCrossProfileCallerIdDisabledForUser { user_id: 0 }.write(p),
        );
        let disabled =
            dp::read_get_cross_profile_caller_id_disabled_for_user_reply(&mut r.reader()).unwrap();
        assert_eq!(disabled, Ok(false));
        let r = call(&device, APP, dp::GET_AUTO_TIME_REQUIRED, |p| {
            dp::GetAutoTimeRequired {}.write(p)
        });
        assert_eq!(
            dp::read_get_auto_time_required_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
    }

    /// Without `FEATURE_DEVICE_ADMIN` the original answers before its
    /// checks: an empty list of admins, not none.
    #[test]
    fn without_the_feature_the_defaults_come_unchecked() {
        let device = Fake {
            feature: false,
            ..Default::default()
        };
        let r = call(&device, APP, dp::GET_ACTIVE_ADMINS, |p| {
            dp::GetActiveAdmins { user_handle: 10 }.write(p)
        });
        let admins = dp::read_get_active_admins_reply::<ComponentName>(&mut r.reader())
            .unwrap()
            .unwrap();
        assert_eq!(admins.map(|a| a.len()), Some(0));
        let r = call(&device, APP, dp::GET_DEVICE_OWNER_USER_ID, |p| {
            dp::GetDeviceOwnerUserId {}.write(p)
        });
        assert_eq!(
            dp::read_get_device_owner_user_id_reply(&mut r.reader()).unwrap(),
            Ok(USER_NULL)
        );
    }

    /// The checks: another user's state takes INTERACT_ACROSS_USERS_FULL,
    /// the owners' queries MANAGE_USERS or MANAGE_PROFILE_AND_DEVICE_OWNERS
    /// (`isDeviceManaged` is a system API), a caller's package must be its
    /// own, a negative user is invalid, and an admin no user has is
    /// refused.
    #[test]
    fn the_original_checks_are_made() {
        let mut device = Fake::default();
        let other_user = |p: &mut Parcel| dp::GetActiveAdmins { user_handle: 10 }.write(p);
        assert_eq!(
            exception(&call(&device, APP, dp::GET_ACTIVE_ADMINS, other_user)),
            not_authorized()
        );
        let negative = |p: &mut Parcel| {
            dp::PackageHasActiveAdmins {
                package_name: None,
                user_handle: -1,
            }
            .write(p)
        };
        let r = call(&device, APP, dp::PACKAGE_HAS_ACTIVE_ADMINS, negative);
        assert_eq!(exception(&r), Exception::illegal_argument("Invalid userId"));
        let has_owner = |p: &mut Parcel| dp::HasDeviceOwner {}.write(p);
        assert_eq!(
            exception(&call(&device, APP, dp::HAS_DEVICE_OWNER, has_owner)),
            not_authorized()
        );
        let r = call(&device, SYSTEM_UID, dp::HAS_DEVICE_OWNER, has_owner);
        assert_eq!(
            dp::read_has_device_owner_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        let any_user = |p: &mut Parcel| {
            dp::GetDeviceOwnerComponent {
                calling_user_only: false,
            }
            .write(p)
        };
        assert_eq!(
            exception(&call(
                &device,
                APP,
                dp::GET_DEVICE_OWNER_COMPONENT,
                any_user
            )),
            not_authorized()
        );
        device
            .permissions
            .push((APP, MANAGE_PROFILE_AND_DEVICE_OWNERS));
        let r = call(&device, APP, dp::GET_DEVICE_OWNER_COMPONENT, any_user);
        assert!(
            dp::read_get_device_owner_component_reply::<ComponentName>(&mut r.reader())
                .unwrap()
                .unwrap()
                .is_none()
        );
        device.permissions.push((APP, INTERACT_ACROSS_USERS_FULL));
        let r = call(&device, APP, dp::GET_ACTIVE_ADMINS, other_user);
        assert!(
            dp::read_get_active_admins_reply::<ComponentName>(&mut r.reader())
                .unwrap()
                .unwrap()
                .is_none()
        );
        let not_its_package = |p: &mut Parcel| {
            dp::GetStorageEncryptionStatus {
                caller_package: Some("com.other".into()),
                user_handle: 0,
            }
            .write(p)
        };
        let r = call(
            &device,
            APP,
            dp::GET_STORAGE_ENCRYPTION_STATUS,
            not_its_package,
        );
        assert_eq!(
            exception(&r),
            Exception::security("Caller with uid 10100 is not com.other")
        );
        let admin = |p: &mut Parcel| {
            let who = Some(Component("com.example", "com.example.Admin"));
            dp::GetScreenCaptureDisabled::<Component> {
                who,
                user_handle: 0,
                parent: false,
            }
            .write(p)
        };
        let r = call(&device, APP, dp::GET_SCREEN_CAPTURE_DISABLED, admin);
        let refused = "Admin ComponentInfo{com.example/com.example.Admin} does not exist or is not owned by uid 10100";
        assert_eq!(exception(&r), Exception::security(refused));
        let no_admin = |p: &mut Parcel| dp::IsManagedProfile::<Component> { admin: None }.write(p);
        let r = call(&device, APP, dp::IS_MANAGED_PROFILE, no_admin);
        assert_eq!(
            exception(&r),
            Exception::new(EX_NULL_POINTER, "ComponentName is null")
        );
        let parent = |p: &mut Parcel| {
            dp::GetScreenCaptureDisabled::<Component> {
                who: None,
                user_handle: 0,
                parent: true,
            }
            .write(p)
        };
        assert_eq!(
            exception(&call(&device, APP, dp::GET_SCREEN_CAPTURE_DISABLED, parent)),
            not_authorized()
        );
        let owner_type = |p: &mut Parcel| {
            dp::GetDeviceOwnerType::<Component> {
                admin: Some(Component("com.example", "com.example.Admin")),
            }
            .write(p)
        };
        let r = call(&device, APP, dp::GET_DEVICE_OWNER_TYPE, owner_type);
        assert_eq!(
            exception(&r),
            Exception::new(EX_ILLEGAL_STATE, "there is no device owner")
        );
    }

    /// The camera: the caller's own user, the camera server, or
    /// MANAGE_DEVICE_POLICY_CAMERA (with its cross-user permission for
    /// another user); a caller without a package for the permission is a
    /// null pointer.
    #[test]
    fn the_camera_takes_its_permissions() {
        let mut device = Fake::default();
        let ask = |package: Option<&str>, user| {
            let package = package.map(str::to_string);
            move |p: &mut Parcel| {
                dp::GetCameraDisabled::<Component> {
                    who: None,
                    caller_package_name: package,
                    user_handle: user,
                    parent: false,
                }
                .write(p)
            }
        };
        let r = call(
            &device,
            CAMERASERVER_UID,
            dp::GET_CAMERA_DISABLED,
            ask(None, 10),
        );
        assert_eq!(
            dp::read_get_camera_disabled_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        let r = call(&device, APP, dp::GET_CAMERA_DISABLED, ask(None, 10));
        assert_eq!(
            exception(&r),
            Exception::new(EX_NULL_POINTER, "callerPackageName is null")
        );
        let r = call(
            &device,
            APP,
            dp::GET_CAMERA_DISABLED,
            ask(Some("com.example"), 10),
        );
        assert_eq!(exception(&r), not_authorized());
        device.permissions.push((APP, MANAGE_DEVICE_POLICY_CAMERA));
        let r = call(
            &device,
            APP,
            dp::GET_CAMERA_DISABLED,
            ask(Some("com.example"), 10),
        );
        assert_eq!(exception(&r), not_authorized());
        device
            .permissions
            .push((APP, MANAGE_DEVICE_POLICY_ACROSS_USERS));
        let r = call(
            &device,
            APP,
            dp::GET_CAMERA_DISABLED,
            ask(Some("com.example"), 10),
        );
        assert_eq!(
            dp::read_get_camera_disabled_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        // Another app is not the package.
        let r = call(
            &device,
            OTHER_APP,
            dp::GET_CAMERA_DISABLED,
            ask(Some("com.example"), 0),
        );
        assert_eq!(
            exception(&r),
            Exception::security("Caller with uid 10101 is not com.example")
        );
    }

    /// Auto time with the coexistence flag: SET_TIME, then the global
    /// setting; without it, only an owner's admin asks.
    #[test]
    fn auto_time_reads_the_setting_for_set_time() {
        let mut device = Fake {
            settings: vec![("auto_time", "1"), ("auto_time_zone", "x")],
            ..Default::default()
        };
        let ask = |p: &mut Parcel| {
            dp::GetAutoTimeEnabled::<Component> {
                who: None,
                caller_package_name: Some("com.example".into()),
            }
            .write(p)
        };
        assert_eq!(
            exception(&call(&device, APP, dp::GET_AUTO_TIME_ENABLED, ask)),
            not_authorized()
        );
        device
            .permissions
            .extend([(APP, SET_TIME), (APP, SET_TIME_ZONE)]);
        let r = call(&device, APP, dp::GET_AUTO_TIME_ENABLED, ask);
        assert_eq!(
            dp::read_get_auto_time_enabled_reply(&mut r.reader()).unwrap(),
            Ok(true)
        );
        let zone = |p: &mut Parcel| {
            dp::GetAutoTimeZoneEnabled::<Component> {
                who: None,
                caller_package_name: Some("com.example".into()),
            }
            .write(p)
        };
        let r = call(&device, APP, dp::GET_AUTO_TIME_ZONE_ENABLED, zone);
        assert_eq!(
            dp::read_get_auto_time_zone_enabled_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        device.flags = false;
        let r = call(&device, APP, dp::GET_AUTO_TIME_ENABLED, ask);
        assert_eq!(
            exception(&r),
            Exception::new(EX_NULL_POINTER, "ComponentName is null")
        );
    }

    /// Provisioning: the persisted setup, or the setting the original takes
    /// it from at boot; the user's provisioning state.
    #[test]
    fn provisioning_is_the_users_data() {
        let mut device = Fake {
            permissions: vec![(APP, MANAGE_USERS)],
            ..Default::default()
        };
        let provisioned = |p: &mut Parcel| dp::IsDeviceProvisioned {}.write(p);
        let r = call(&device, APP, dp::IS_DEVICE_PROVISIONED, provisioned);
        assert_eq!(
            dp::read_is_device_provisioned_reply(&mut r.reader()).unwrap(),
            Ok(false)
        );
        device.settings.push(("user_setup_complete", "1"));
        let r = call(&device, APP, dp::IS_DEVICE_PROVISIONED, provisioned);
        assert_eq!(
            dp::read_is_device_provisioned_reply(&mut r.reader()).unwrap(),
            Ok(true)
        );
        device.settings.clear();
        device.policies.users.insert(
            0,
            UserData {
                setup_complete: true,
                provisioning_state: 3,
            },
        );
        let r = call(&device, APP, dp::IS_DEVICE_PROVISIONED, provisioned);
        assert_eq!(
            dp::read_is_device_provisioned_reply(&mut r.reader()).unwrap(),
            Ok(true)
        );
        let state = |p: &mut Parcel| dp::GetUserProvisioningState { user_handle: 0 }.write(p);
        let r = call(&device, APP, dp::GET_USER_PROVISIONING_STATE, state);
        assert_eq!(
            dp::read_get_user_provisioning_state_reply(&mut r.reader()).unwrap(),
            Ok(3)
        );
        assert_eq!(
            exception(&call(
                &device,
                APP + 1,
                dp::IS_DEVICE_PROVISIONED,
                provisioned
            )),
            not_authorized()
        );
    }

    /// A device with an owner, admins or policies is not served, and
    /// neither is a method left out.
    #[test]
    fn a_managed_device_is_not_implemented() {
        let unimplemented =
            "a device with a device or profile owner, active admins or policies (#1521)";
        for policies in [
            Policies {
                owned: true,
                ..Default::default()
            },
            Policies {
                admins: true,
                ..Default::default()
            },
            Policies {
                policies: true,
                ..Default::default()
            },
        ] {
            let device = Fake {
                policies,
                ..Default::default()
            };
            let r = call(&device, SYSTEM_UID, dp::HAS_DEVICE_OWNER, |p| {
                dp::HasDeviceOwner {}.write(p)
            });
            assert_eq!(exception(&r).message, unimplemented);
            let r = call(&device, APP, dp::GET_CAMERA_DISABLED, |p| {
                let package = Some("com.example".into());
                dp::GetCameraDisabled::<Component> {
                    who: None,
                    caller_package_name: package,
                    user_handle: 0,
                    parent: false,
                }
                .write(p)
            });
            assert_eq!(exception(&r).message, unimplemented);
        }
        let device = Fake::default();
        let mut data = Parcel::new();
        data.write_interface_token(dp::DESCRIPTOR);
        let r = answer(
            &device,
            APP,
            dp::GET_PASSWORD_MINIMUM_LENGTH,
            &mut data.reader(),
        )
        .unwrap();
        assert_eq!(exception(&r).message, "getPasswordMinimumLength (#1521)");
    }
}
