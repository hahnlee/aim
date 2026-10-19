//! `device_identifiers`: DeviceIdentifiersPolicyService for the app host
//! (`android-16.0.0_r1`): the serial number (`ro.serialno`, unset in the
//! guest, so `unknown`) to a caller that may read device identifiers, as
//! `TelephonyPermissions.checkCallingOrSelfReadDeviceIdentifiers` decides
//! it. The app host has no subscriptions, so no caller has carrier
//! privileges.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_os_ideviceidentifierspolicyservice as di;

use super::appops_service::{self, MODE_ALLOWED, MODE_DEFAULT, MODE_FOREGROUND};
use super::legacy_permission::Subject;
use super::{AppHost, PER_USER_RANGE, reply};

/// `Build.UNKNOWN`; `Build.VERSION_CODES.Q`.
const UNKNOWN: &str = "unknown";
const Q: i32 = 29;
/// `AppOpsManager.OP_USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER`.
const OP_USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER: i32 = 105;
const USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER: &str = "android.permission.USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER";
const SYSTEM_UID: i32 = 1000;

pub struct DeviceIdentifiers {
    host: Arc<AppHost>,
}

impl DeviceIdentifiers {
    pub fn new(host: Arc<AppHost>) -> Self {
        DeviceIdentifiers { host }
    }

    /// `checkCallingOrSelfReadDeviceIdentifiers(context, package, feature,
    /// "getSerial")`: whether the caller may, false where an app before Q
    /// with READ_PHONE_STATE gets `unknown`, an error otherwise.
    fn may_read(&self, uid: i32, pid: i32, package: Option<&str>, feature: Option<&str>) -> Result<bool, Exception> {
        const MESSAGE: &str = "getSerial";
        let host = &self.host;
        // `checkCallingOrSelfUseIccAuthWithDeviceIdentifier`.
        if let Some(package) = package {
            match appops_service::note_from(host, SYSTEM_UID, OP_USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER, uid, Some(package), feature)? {
                MODE_ALLOWED | MODE_FOREGROUND => return Ok(true),
                MODE_DEFAULT if host.check_permission(Some(USE_ICC_AUTH_WITH_DEVICE_IDENTIFIER), uid)? == 0 => return Ok(true),
                _ => {}
            }
        }
        // `checkPrivilegedReadPermissionOrCarrierPrivilegePermission`.
        let denied = || Exception::security(format!("{MESSAGE}: The uid {uid} does not meet the requirements to access device identifiers."));
        let subject = Subject { package, message: Some(MESSAGE), feature, pid, uid };
        match host.device_identifier_access((uid, pid), &subject) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(e) if e.code == aim_binder_host::parcel::EX_SECURITY => return Err(denied()),
            Err(e) => return Err(e),
        }
        // `reportAccessDeniedToReadIdentifiers`.
        eprintln!("app host: device_identifiers: reportAccessDeniedToReadIdentifiers:{}:{MESSAGE}:-1", package.unwrap_or("null"));
        let (state, _) = host.packages()?;
        let target_sdk = package.and_then(|p| state.packages.get(p)).map(|ps| ps.target_sdk_version);
        if target_sdk.is_some_and(|t| t < Q) && host.check_permission(Some("android.permission.READ_PHONE_STATE"), uid)? == 0 {
            return Ok(false);
        }
        Err(denied())
    }

    fn serial(&self) -> String {
        self.host.property("ro.serialno").filter(|s| !s.is_empty()).unwrap_or_else(|| UNKNOWN.into())
    }
}

impl Service for DeviceIdentifiers {
    fn descriptor(&self) -> &str {
        di::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let (uid, pid) = (call.sender_euid as i32, call.sender_pid);
        let mut p = Parcel::new();
        let result = match call.code {
            di::GET_SERIAL => {
                di::GetSerial::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.may_read(uid, pid, None, None).map(|may| di::write_get_serial_reply(&mut p, &Some(if may { self.serial() } else { UNKNOWN.into() })))
            }
            di::GET_SERIAL_FOR_PACKAGE => {
                let a = di::GetSerialForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    // `checkPackageBelongsToCaller`.
                    let package = a.calling_package.as_deref().unwrap_or_default();
                    let (state, resolution) = self.host.packages()?;
                    let query = crate::package::query::Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
                    let package_uid = query.package_uid(package, 0, uid / PER_USER_RANGE as i32).map_err(|e| self.host.unimplemented_named("device_identifiers", e.0))??;
                    if package_uid != uid {
                        return Err(Exception::illegal_argument(format!("Invalid callingPackage or callingPackage does not belong to caller's uid:{uid}")));
                    }
                    let may = self.may_read(uid, pid, Some(package), a.calling_feature_id.as_deref())?;
                    di::write_get_serial_for_package_reply(&mut p, &Some(if may { self.serial() } else { UNKNOWN.into() }));
                    Ok(())
                })()
            }
            code => {
                let name = di::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("device_identifiers", name))
            }
        };
        reply(result.map(|()| p))
    }
}
