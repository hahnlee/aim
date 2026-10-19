//! `platform_compat`: PlatformCompat's queries for apps
//! (`CompatChanges.isChangeEnabled` by uid or package), as
//! `com.android.server.compat.PlatformCompat` answers them at
//! `android-16.0.0_r1`, over the app host's compat config
//! (`super::compat`). The change reports it logs go to statsd only.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::com_android_internal_compat_iplatformcompat as compat;

use super::{AppHost, reply};

/// The permissions its queries need (`@EnforcePermission(allOf = …)`).
const QUERY_PERMISSIONS: [&str; 2] = [
    "android.permission.LOG_COMPAT_CHANGE",
    "android.permission.READ_COMPAT_CHANGE_CONFIG",
];

pub struct PlatformCompat(pub Arc<AppHost>);

impl PlatformCompat {
    /// `PermissionEnforcer.enforcePermissionAllOf`.
    fn enforce(&self, uid: i32) -> Result<(), Exception> {
        for permission in QUERY_PERMISSIONS {
            if self.0.check_permission(Some(permission), uid)? != 0 {
                return Err(Exception::security(format!(
                    "Access denied, requires: allOf={{{}}}",
                    QUERY_PERMISSIONS.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// `isChangeEnabledByPackageName`: the installed package's, else
    /// whether the change will be enabled for it.
    fn enabled_by_package(&self, change: i64, package: Option<&str>) -> Result<bool, Exception> {
        let inputs = self.0.inputs()?;
        Ok(
            match package.and_then(|p| inputs.packages.application_info(p, 0)) {
                Some(ai) => inputs
                    .compat
                    .is_enabled(change, package, ai.target_sdk_version),
                None => package.is_none_or(|p| inputs.compat.will_be_enabled(change, p)),
            },
        )
    }
}

impl Service for PlatformCompat {
    fn descriptor(&self) -> &str {
        compat::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        match call.code {
            compat::IS_CHANGE_ENABLED_BY_UID => {
                let a =
                    compat::IsChangeEnabledByUid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self
                    .enforce(uid)
                    .and_then(|()| self.0.change_enabled(a.change_id, a.uid))
                {
                    Ok(enabled) => compat::write_is_change_enabled_by_uid_reply(&mut p, enabled),
                    Err(e) => return reply(Err(e)),
                }
            }
            compat::IS_CHANGE_ENABLED_BY_PACKAGE_NAME => {
                let a = compat::IsChangeEnabledByPackageName::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match self
                    .enforce(uid)
                    .and_then(|()| self.enabled_by_package(a.change_id, a.package_name.as_deref()))
                {
                    Ok(enabled) => {
                        compat::write_is_change_enabled_by_package_name_reply(&mut p, enabled)
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            code => return reply(Err(self.0.unimplemented("platform_compat", code))),
        }
        Ok(p)
    }
}
