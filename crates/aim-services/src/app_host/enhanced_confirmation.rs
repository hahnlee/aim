//! `ecm_enhanced_confirmation`: EnhancedConfirmationService of the
//! Permission module (`android-16.0.0_r1`), which SystemServer starts with
//! `enhanced_confirmation_mode_apis_enabled` (on in the image).
//! PermissionController asks it before granting a permission
//! (`GrantPermissionsActivity.blockRestrictedPermissions`). Whether a
//! setting is restricted for a package: a protected setting of a package
//! that is guarded (`ACCESS_RESTRICTED_SETTINGS` explicitly, or installed
//! from a file or by an untrusted installer, the trusted packages and
//! installers being SystemConfig's `<enhanced-confirmation-trusted-*>`),
//! or a setting restricted during an untrusted call. Calls reach the
//! service's call tracker only from Telecom's in-call service, which the
//! app host does not run (#1786), so no call is ever ongoing. Clearing a
//! restriction sets the app op, a mode write the app host does not make
//! (#1848); the service's statsd atoms are not written (#1849).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_app_ecm_ienhancedconfirmationmanager as ecm;

use super::{AppHost, appops_service, reply};
use crate::package::info::FLAG_SYSTEM;
use crate::package::query::Query;

const MANAGE_ENHANCED_CONFIRMATION_STATES: &str =
    "android.permission.MANAGE_ENHANCED_CONFIRMATION_STATES";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const PER_USER_RANGE: i32 = 100_000;
/// The app host runs user 0 alone.
const USERS: [i32; 1] = [0];
/// `EnhancedConfirmationManager.REASON_PACKAGE_RESTRICTED`.
const REASON_PACKAGE_RESTRICTED: &str = "package_restricted";
/// `AppOpsManager.OP_ACCESS_RESTRICTED_SETTINGS`.
const OP_ACCESS_RESTRICTED_SETTINGS: i32 = 119;
/// `EcmState`: the app op's modes.
const ECM_STATE_NOT_GUARDED: i32 = 0;
const ECM_STATE_GUARDED: i32 = 2;
const ECM_STATE_GUARDED_AND_ACKNOWLEDGED: i32 = 1;
/// `PackageInstaller.PACKAGE_SOURCE_LOCAL_FILE`, `PACKAGE_SOURCE_DOWNLOADED_FILE`.
const PACKAGE_SOURCE_LOCAL_FILE: i32 = 3;
const PACKAGE_SOURCE_DOWNLOADED_FILE: i32 = 4;
const FEATURE_LEANBACK: &str = "android.software.leanback";
const FEATURE_AUTOMOTIVE: &str = "android.hardware.type.automotive";
const FLAGS: &str = "android.permission.flags.";
/// `PackageManager.CERT_INPUT_SHA256`.
const CERT_INPUT_SHA256: i32 = 1;

/// `PER_PACKAGE_PROTECTED_SETTINGS`: runtime permissions, app ops and
/// default application roles.
const PER_PACKAGE_PROTECTED_SETTINGS: [&str; 15] = [
    "android.permission.SEND_SMS",
    "android.permission.RECEIVE_SMS",
    "android.permission.READ_SMS",
    "android.permission.RECEIVE_MMS",
    "android.permission.RECEIVE_WAP_PUSH",
    "android.permission.READ_CELL_BROADCASTS",
    "android.permission-group.SMS",
    "android.permission.BIND_DEVICE_ADMIN",
    "android:bind_accessibility_service",
    "android:access_notifications",
    "android:system_alert_window",
    "android:get_usage_stats",
    "android:loader_usage_stats",
    "android.app.role.DIALER",
    "android.app.role.SMS",
];

/// `UNTRUSTED_CALL_RESTRICTED_SETTINGS` with
/// `unknown_call_package_install_blocking_enabled`.
const UNTRUSTED_CALL_RESTRICTED_SETTINGS: [&str; 2] = [
    "android:request_install_packages",
    "android:bind_accessibility_service",
];

pub struct EnhancedConfirmation {
    host: Arc<AppHost>,
}

impl EnhancedConfirmation {
    pub fn new(host: Arc<AppHost>) -> Self {
        EnhancedConfirmation { host }
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        self.host.flag(&format!("{FLAGS}{name}"))
    }

    fn feature(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .system
            .features
            .iter()
            .any(|(n, version)| n == name && *version >= 0))
    }

    /// `enforcePermissions`: `UserUtils.enforceCrossUserPermission`
    /// (another user than the caller's needs INTERACT_ACROSS_USERS_FULL;
    /// the shell's restriction applies to users the app host does not
    /// have), then MANAGE_ENHANCED_CONFIRMATION_STATES.
    fn enforce_permissions(&self, uid: i32, method: &str, user: i32) -> Result<(), Exception> {
        let denied = |permission: &str| {
            Exception::security(format!(
                "{method}: Neither user {uid} nor current process has {permission}."
            ))
        };
        if user != uid / PER_USER_RANGE {
            if user < 0 {
                return Err(Exception::illegal_argument(format!("Invalid user {user}")));
            }
            if self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)?
                != 0
            {
                return Err(denied(INTERACT_ACROSS_USERS_FULL));
            }
        }
        if self
            .host
            .check_permission(Some(MANAGE_ENHANCED_CONFIRMATION_STATES), uid)?
            != 0
        {
            return Err(denied(MANAGE_ENHANCED_CONFIRMATION_STATES));
        }
        Ok(())
    }

    /// `Preconditions.checkStringNotEmpty`.
    fn not_empty<'a>(value: Option<&'a str>, message: &str) -> Result<&'a str, Exception> {
        value
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Exception::illegal_argument(message))
    }

    /// `isSettingEcmProtected`.
    fn protected(&self, setting: &str) -> Result<bool, Exception> {
        if self.feature(FEATURE_LEANBACK)? || self.feature(FEATURE_AUTOMOTIVE)? {
            return Ok(false);
        }
        Ok(PER_PACKAGE_PROTECTED_SETTINGS.contains(&setting)
            || (self.flag("unknown_call_package_install_blocking_enabled")?
                && UNTRUSTED_CALL_RESTRICTED_SETTINGS.contains(&setting)))
    }

    /// `getRestrictionReason`.
    fn restriction_reason(
        &self,
        uid: i32,
        package: Option<&str>,
        setting: Option<&str>,
        user: i32,
    ) -> Result<Option<&'static str>, Exception> {
        self.enforce_permissions(uid, "isRestricted", user)?;
        if !USERS.contains(&user) {
            eprintln!("app host: ecm: user {user} does not exist");
            return Ok(None);
        }
        let package = Self::not_empty(package, "packageName cannot be null or empty")?;
        let setting = Self::not_empty(setting, "settingIdentifier cannot be null or empty")?;
        if !self.protected(setting)? {
            return Ok(None);
        }
        if PER_PACKAGE_PROTECTED_SETTINGS.contains(&setting) && self.guarded(uid, package, user)? {
            return Ok(Some(REASON_PACKAGE_RESTRICTED));
        }
        // `getGlobalProtectionReason`: no untrusted call is ongoing.
        Ok(None)
    }

    /// `getPackageUid` as the caller; NameNotFoundException becomes the
    /// service's IllegalArgumentException.
    fn package_uid(&self, uid: i32, package: &str, user: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let found = query
            .package_uid(package, 0, user)
            .map_err(|e| self.host.unimplemented_named("ecm", e.0))??;
        if found < 0 {
            return Err(Exception::illegal_argument(format!(
                "android.content.pm.PackageManager$NameNotFoundException: {package}"
            )));
        }
        Ok(found)
    }

    /// `getAppEcmState`: `noteOpNoThrow(ACCESS_RESTRICTED_SETTINGS)` as the
    /// system.
    fn ecm_state(&self, uid: i32, package: &str, user: i32) -> Result<i32, Exception> {
        let package_uid = self.package_uid(uid, package, user)?;
        appops_service::note(
            &self.host,
            OP_ACCESS_RESTRICTED_SETTINGS,
            package_uid,
            Some(package),
        )
    }

    /// `getApplicationInfoAsUser(0)`'s `FLAG_SYSTEM`; none when the
    /// package is not found.
    fn preinstalled(&self, uid: i32, package: Option<&str>, user: i32) -> Result<bool, Exception> {
        let Some(package) = package else {
            return Ok(false);
        };
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let info = query
            .application_info(package, 0, user)
            .map_err(|e| self.host.unimplemented_named("ecm", e.0))?;
        Ok(matches!(info, Ok(Some(info)) if info.flags & FLAG_SYSTEM != 0))
    }

    /// `isPackageSignedWithAnyOf` the digests SystemConfig trusts it with.
    fn signed_with_any_of(
        &self,
        uid: i32,
        package: Option<&str>,
        trusted: &[(String, Vec<u8>)],
    ) -> Result<bool, Exception> {
        let Some(package) = package else {
            return Ok(false);
        };
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        for (_, digest) in trusted.iter().filter(|(p, _)| p == package) {
            if query
                .package_certificate(package, Some(digest), CERT_INPUT_SHA256)
                .map_err(|e| self.host.unimplemented_named("ecm", e.0))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `isPackageEcmGuarded`.
    fn guarded(&self, uid: i32, package: &str, user: i32) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        let trusted_packages = &state.system.ecm_trusted_packages;
        let trusted_installers = &state.system.ecm_trusted_installers;
        if self.signed_with_any_of(uid, Some(package), trusted_packages)?
            || self.signed_with_any_of(uid, Some(package), trusted_installers)?
            || self.preinstalled(uid, Some(package), user)?
        {
            return Ok(false);
        }
        match self.ecm_state(uid, package, user)? {
            ECM_STATE_GUARDED | ECM_STATE_GUARDED_AND_ACKNOWLEDGED => return Ok(true),
            ECM_STATE_NOT_GUARDED => return Ok(false),
            _ => {}
        }
        // `getInstallSourceInfo` as the system sees it.
        let Some(source) = state
            .packages
            .get(package)
            .map(|ps| ps.install_source.clone())
        else {
            eprintln!("app host: ecm: Package not found: {package}");
            return Ok(false);
        };
        if source.package_source == PACKAGE_SOURCE_LOCAL_FILE
            || source.package_source == PACKAGE_SOURCE_DOWNLOADED_FILE
        {
            return Ok(true);
        }
        let installer = source.installer.as_deref();
        Ok(!(trusted_installers.is_empty()
            || self.preinstalled(uid, installer, user)?
            || self.signed_with_any_of(uid, installer, trusted_installers)?))
    }

    /// `isClearRestrictionAllowed`.
    fn clear_restriction_allowed(
        &self,
        uid: i32,
        package: Option<&str>,
        user: i32,
    ) -> Result<bool, Exception> {
        self.enforce_permissions(uid, "isClearRestrictionAllowed", user)?;
        if !USERS.contains(&user) {
            return Ok(false);
        }
        let package = Self::not_empty(package, "packageName cannot be null or empty")?;
        Ok(self.ecm_state(uid, package, user)? == ECM_STATE_GUARDED_AND_ACKNOWLEDGED)
    }

    /// `clearRestriction`: allowed once acknowledged, then
    /// `ACCESS_RESTRICTED_SETTINGS` set to allowed (#1848).
    fn clear_restriction(
        &self,
        uid: i32,
        package: Option<&str>,
        user: i32,
    ) -> Result<(), Exception> {
        self.enforce_permissions(uid, "clearRestriction", user)?;
        if !USERS.contains(&user) {
            return Ok(());
        }
        let package = Self::not_empty(package, "packageName cannot be null or empty")?;
        if self.ecm_state(uid, package, user)? != ECM_STATE_GUARDED_AND_ACKNOWLEDGED {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "Clear restriction attempted but not allowed",
            ));
        }
        Err(self
            .host
            .unimplemented_named("ecm", "setting ACCESS_RESTRICTED_SETTINGS (#1848)"))
    }

    /// `setClearRestrictionAllowed`: a guarded package's
    /// `ACCESS_RESTRICTED_SETTINGS` set to acknowledged (#1848).
    fn set_clear_restriction_allowed(
        &self,
        uid: i32,
        package: Option<&str>,
        user: i32,
    ) -> Result<(), Exception> {
        self.enforce_permissions(uid, "setClearRestrictionAllowed", user)?;
        if !USERS.contains(&user) {
            return Ok(());
        }
        let package = Self::not_empty(package, "packageName cannot be null or empty")?;
        if !self.guarded(uid, package, user)? {
            return Ok(());
        }
        Err(self
            .host
            .unimplemented_named("ecm", "setting ACCESS_RESTRICTED_SETTINGS (#1848)"))
    }
}

impl Service for EnhancedConfirmation {
    fn descriptor(&self) -> &str {
        ecm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            ecm::IS_RESTRICTED => {
                let a = ecm::IsRestricted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.restriction_reason(
                    uid,
                    a.package_name.as_deref(),
                    a.setting_identifier.as_deref(),
                    a.user_id,
                )
                .map(|reason| ecm::write_is_restricted_reply(&mut p, reason.is_some()))
            }
            ecm::GET_RESTRICTION_REASON => {
                let a = ecm::GetRestrictionReason::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.restriction_reason(
                    uid,
                    a.package_name.as_deref(),
                    a.setting_identifier.as_deref(),
                    a.user_id,
                )
                .map(|reason| {
                    ecm::write_get_restriction_reason_reply(&mut p, &reason.map(String::from))
                })
            }
            ecm::CLEAR_RESTRICTION => {
                let a = ecm::ClearRestriction::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.clear_restriction(uid, a.package_name.as_deref(), a.user_id)
                    .map(|()| ecm::write_clear_restriction_reply(&mut p))
            }
            ecm::IS_CLEAR_RESTRICTION_ALLOWED => {
                let a =
                    ecm::IsClearRestrictionAllowed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.clear_restriction_allowed(uid, a.package_name.as_deref(), a.user_id)
                    .map(|allowed| ecm::write_is_clear_restriction_allowed_reply(&mut p, allowed))
            }
            ecm::SET_CLEAR_RESTRICTION_ALLOWED => {
                let a =
                    ecm::SetClearRestrictionAllowed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_clear_restriction_allowed(uid, a.package_name.as_deref(), a.user_id)
                    .map(|()| ecm::write_set_clear_restriction_allowed_reply(&mut p))
            }
            code => {
                Err(self
                    .host
                    .unimplemented("ecm_enhanced_confirmation", self.descriptor(), code))
            }
        };
        reply(result.map(|()| p))
    }
}
