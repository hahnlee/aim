//! `legacy_permission`: LegacyPermissionManagerService for the app host
//! (`android-16.0.0_r1`), its access checks: device identifiers
//! (`checkDeviceIdentifierAccess`) and the phone number
//! (`checkPhoneNumberAccess`), with appops noted as the original notes
//! them. On a device with no device or profile owner, no package has a
//! device policy's identifier access; one with admins or policies is not
//! implemented (#1737). The default grants it makes for carrier, IMS, data
//! and LUI apps are not implemented (#1520).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_permission_ilegacypermissionmanager as lp;

use super::appops_service::{self, MODE_ALLOWED};
use super::{AppHost, PER_USER_RANGE, reply};

const SYSTEM_UID: i32 = 1000;
const ROOT_UID: i32 = 0;
const FIRST_APPLICATION_UID: i32 = 10_000;
const READ_PRIVILEGED_PHONE_STATE: &str = "android.permission.READ_PRIVILEGED_PHONE_STATE";
/// `AppOpsManager.OP_READ_DEVICE_IDENTIFIERS`, `OP_READ_PHONE_STATE`,
/// `OP_WRITE_SMS`, `OP_READ_PHONE_NUMBERS`, `OP_READ_SMS`.
const OP_READ_DEVICE_IDENTIFIERS: i32 = 89;
const OP_READ_PHONE_STATE: i32 = 51;
const OP_WRITE_SMS: i32 = 15;
const OP_READ_PHONE_NUMBERS: i32 = 65;
const OP_READ_SMS: i32 = 14;
/// `Build.VERSION_CODES.Q`.
const Q: i32 = 29;

/// An access check's subject: the package, uid and pid asked about, the
/// feature and message of the appops it notes.
pub(super) struct Subject<'a> {
    pub package: Option<&'a str>,
    pub message: Option<&'a str>,
    pub feature: Option<&'a str>,
    pub pid: i32,
    pub uid: i32,
}

impl AppHost {
    /// `verifyCallerCanCheckAccess`: an app asks only of itself, and of a
    /// package of the uid asked about.
    fn verify_caller_can_check_access(&self, caller: (i32, i32), s: &Subject<'_>) -> Result<(), Exception> {
        let (calling_uid, calling_pid) = caller;
        let app = |uid: i32| uid % PER_USER_RANGE as i32 >= FIRST_APPLICATION_UID;
        let mut report = app(calling_uid) && (calling_uid != s.uid || calling_pid != s.pid);
        if let Some(package) = s.package
            && app(s.uid)
        {
            let (state, resolution) = self.packages()?;
            let query = crate::package::query::Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
            let package_uid = query
                .package_uid(package, 0, s.uid / PER_USER_RANGE as i32)
                .map_err(|e| self.unimplemented_named("legacy_permission", e.0))??;
            report |= package_uid != s.uid;
        }
        if report {
            return Err(Exception::security(format!(
                "Calling uid {calling_uid}, pid {calling_pid} cannot access for package {} (uid={}, pid={}): {}",
                s.package.unwrap_or("null"),
                s.uid,
                s.pid,
                s.message.unwrap_or("null")
            )));
        }
        Ok(())
    }

    /// `noteOpNoThrow(op, uid, package, feature, message)` as the system.
    fn noted_allowed(&self, code: i32, s: &Subject<'_>, package: &str) -> Result<bool, Exception> {
        Ok(appops_service::note_from(self, SYSTEM_UID, code, s.uid, Some(package), s.feature)? == MODE_ALLOWED)
    }

    /// `checkDeviceIdentifierAccess` asked by `caller` (its uid and pid):
    /// the system and root, READ_PRIVILEGED_PHONE_STATE, or a package
    /// allowed READ_DEVICE_IDENTIFIERS.
    pub(super) fn device_identifier_access(&self, caller: (i32, i32), s: &Subject<'_>) -> Result<bool, Exception> {
        self.verify_caller_can_check_access(caller, s)?;
        if matches!(s.uid % PER_USER_RANGE as i32, SYSTEM_UID | ROOT_UID) || self.check_permission(Some(READ_PRIVILEGED_PHONE_STATE), s.uid)? == 0 {
            return Ok(true);
        }
        let Some(package) = s.package else { return Ok(false) };
        if self.noted_allowed(OP_READ_DEVICE_IDENTIFIERS, s, package)? {
            return Ok(true);
        }
        // `DevicePolicyManager.hasDeviceIdentifierAccess`: a device or
        // profile owner's.
        if self.device_policies()?.has_admins_or_policies() {
            return Err(self.unimplemented_named("legacy_permission", "device identifier access with device admins or policies (#1737)"));
        }
        Ok(false)
    }

    /// `checkPermissionAndAppop`: granted, else `MODE_IGNORED` (1) if
    /// only the op is not allowed, else denied (-1).
    fn permission_and_op(&self, s: &Subject<'_>, package: &str, permission: Option<&str>, code: i32) -> Result<i32, Exception> {
        if let Some(permission) = permission
            && self.check_permission(Some(permission), s.uid)? != 0
        {
            return Ok(-1);
        }
        Ok(if self.noted_allowed(code, s, package)? { 0 } else { 1 })
    }

    /// `checkPhoneNumberAccess`.
    fn phone_number_access(&self, caller: (i32, i32), s: &Subject<'_>) -> Result<i32, Exception> {
        self.verify_caller_can_check_access(caller, s)?;
        if self.check_permission(Some(READ_PRIVILEGED_PHONE_STATE), s.uid)? == 0 {
            return Ok(0);
        }
        let Some(package) = s.package else { return Ok(-1) };
        let (state, _) = self.packages()?;
        let pre_r = state.packages.get(package).is_some_and(|ps| ps.target_sdk_version <= Q);
        let mut result = -1;
        if pre_r {
            result = self.permission_and_op(s, package, Some("android.permission.READ_PHONE_STATE"), OP_READ_PHONE_STATE)?;
            if result == 0 {
                return Ok(0);
            }
        }
        for (permission, code) in [
            (None, OP_WRITE_SMS),
            (Some("android.permission.READ_PHONE_NUMBERS"), OP_READ_PHONE_NUMBERS),
            (Some("android.permission.READ_SMS"), OP_READ_SMS),
        ] {
            if self.permission_and_op(s, package, permission, code)? == 0 {
                return Ok(0);
            }
        }
        Ok(result)
    }
}

pub struct LegacyPermissionManager {
    host: Arc<AppHost>,
}

impl LegacyPermissionManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        LegacyPermissionManager { host }
    }
}

impl Service for LegacyPermissionManager {
    fn descriptor(&self) -> &str {
        lp::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let caller = (call.sender_euid as i32, call.sender_pid);
        let mut p = Parcel::new();
        let result = match call.code {
            lp::CHECK_DEVICE_IDENTIFIER_ACCESS => {
                let a = lp::CheckDeviceIdentifierAccess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let s = Subject { package: a.package_name.as_deref(), message: a.message.as_deref(), feature: a.calling_feature_id.as_deref(), pid: a.pid, uid: a.uid };
                self.host.device_identifier_access(caller, &s).map(|granted| lp::write_check_device_identifier_access_reply(&mut p, if granted { 0 } else { -1 }))
            }
            lp::CHECK_PHONE_NUMBER_ACCESS => {
                let a = lp::CheckPhoneNumberAccess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let s = Subject { package: a.package_name.as_deref(), message: a.message.as_deref(), feature: a.calling_feature_id.as_deref(), pid: a.pid, uid: a.uid };
                self.host.phone_number_access(caller, &s).map(|r| lp::write_check_phone_number_access_reply(&mut p, r))
            }
            code => {
                let name = lp::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("legacy_permission", &format!("{name} (#1520)")))
            }
        };
        reply(result.map(|()| p))
    }
}
