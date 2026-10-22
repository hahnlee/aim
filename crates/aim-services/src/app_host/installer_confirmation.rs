//! `IInstallerConfirmationBridge` for the app host: the native installer
//! session's confirmation leaves, answered as system_server's
//! `InstallerConfirmationBridge` answers them (`android-16.0.0_r1`), to
//! the system uid only. Whether an install is disabled for a package is
//! the package policy bridge's, not ported (#2023).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerconfirmationbridge as bridge;

use super::{AppHost, PER_USER_RANGE, clock_ms, reply};

const SYSTEM_UID: u32 = 1000;
/// `PackageInstallerSession`'s silent install target SDK change.
const SILENT_UPDATE_MINIMUM_TARGET_SDK: i64 = 325888262;

pub struct InstallerConfirmation {
    host: Arc<AppHost>,
    /// The image's flag the native bootstrap passes.
    dependency_installer_enabled: bool,
}

/// `DeviceConfig.getBoolean`: `Boolean.parseBoolean` of the flag, the
/// default where unset.
fn device_config_boolean(value: Option<String>, default: bool) -> bool {
    value.map_or(default, |v| v.eq_ignore_ascii_case("true"))
}

impl InstallerConfirmation {
    pub fn new(host: Arc<AppHost>, dependency_installer_enabled: bool) -> Self {
        InstallerConfirmation {
            host,
            dependency_installer_enabled,
        }
    }

    /// `isDeviceOwnerOrAffiliated`: of the installer's own user only,
    /// `DevicePolicyManagerInternal.canSilentlyInstallPackage`, which no
    /// package can without a device or profile owner (#1521).
    fn device_owner_or_affiliated(&self, uid: i32, user: i32) -> Result<bool, Exception> {
        if user != uid / PER_USER_RANGE as i32 {
            return Ok(false);
        }
        if self.host.device_policies()?.owned() {
            return Err(self.host.unimplemented_named(
                "installer confirmation",
                "a device or profile owner's silent installs (#1521)",
            ));
        }
        Ok(false)
    }
}

impl Service for InstallerConfirmation {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "installer confirmation requires native system UID",
            )));
        }
        let host = &self.host;
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::IS_DEVICE_OWNER_OR_AFFILIATED => {
                let a = bridge::IsDeviceOwnerOrAffiliated::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.device_owner_or_affiliated(a.installer_uid, a.user_id)
                    .map(|v| bridge::write_is_device_owner_or_affiliated_reply(&mut p, v))
            }
            bridge::IS_SILENT_TARGET_ALLOWED => {
                let a =
                    bridge::IsSilentTargetAllowed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if a.target_sdk_version == i32::MAX {
                    bridge::write_is_silent_target_allowed_reply(&mut p, false);
                    Ok(())
                } else {
                    host.platform().map(|platform| {
                        let allowed = platform.compat.is_enabled(
                            SILENT_UPDATE_MINIMUM_TARGET_SDK,
                            a.target_package.as_deref(),
                            a.target_sdk_version,
                        );
                        bridge::write_is_silent_target_allowed_reply(&mut p, allowed)
                    })
                }
            }
            bridge::IS_INSTALL_DISABLED => {
                bridge::IsInstallDisabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Err(host.unimplemented_named(
                    "installer confirmation",
                    "isInstallDisabledForPackage (#2023)",
                ))
            }
            bridge::IS_DEPENDENCY_INSTALLER_ENABLED => {
                bridge::IsDependencyInstallerEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                bridge::write_is_dependency_installer_enabled_reply(
                    &mut p,
                    self.dependency_installer_enabled,
                );
                Ok(())
            }
            bridge::IS_UPDATE_OWNERSHIP_ENABLED => {
                bridge::IsUpdateOwnershipEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.inputs().map(|inputs| {
                    let enabled = device_config_boolean(
                        inputs.settings.device_config(
                            "package_manager_service",
                            "is_update_ownership_enforcement_available",
                        ),
                        true,
                    );
                    bridge::write_is_update_ownership_enabled_reply(&mut p, enabled)
                })
            }
            bridge::GET_UPTIME_MILLIS => {
                bridge::GetUptimeMillis::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_get_uptime_millis_reply(&mut p, clock_ms(libc::CLOCK_UPTIME_RAW));
                Ok(())
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_config_booleans_parse_as_java() {
        assert!(device_config_boolean(None, true));
        assert!(!device_config_boolean(None, false));
        assert!(device_config_boolean(Some("TRUE".into()), false));
        assert!(!device_config_boolean(Some("1".into()), true));
    }
}
