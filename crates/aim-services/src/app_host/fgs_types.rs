//! Original Java ForegroundServiceTypePolicy, hosted by aim-system. Only owner
//! inputs for ActiveServices' custom permissions cross back into native code.

use super::{AppHost, PER_USER_RANGE, reply};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_iforegroundservicetypehost as inputs;
use std::sync::Arc;

/// `ServiceInfo.FOREGROUND_SERVICE_TYPE_*`.
pub(super) const TYPE_MANIFEST: i32 = -1;
pub(super) const TYPE_NONE: i32 = 0;
pub(super) const TYPE_DATA_SYNC: i32 = 1 << 0;
pub(super) const TYPE_MEDIA_PLAYBACK: i32 = 1 << 1;
pub(super) const TYPE_PHONE_CALL: i32 = 1 << 2;
pub(super) const TYPE_CONNECTED_DEVICE: i32 = 1 << 4;
pub(super) const TYPE_MEDIA_PROJECTION: i32 = 1 << 5;
pub(super) const TYPE_SHORT_SERVICE: i32 = 1 << 11;
pub(super) const TYPE_MEDIA_PROCESSING: i32 = 1 << 13;
pub(super) const TYPE_SPECIAL_USE: i32 = 1 << 30;

// IForegroundServiceTypeHost input tags (AIDL constants are not emitted by aidl-gen).
mod facts {
    pub const SYSTEM_ALLOWLIST: i32 = 0;
    pub const DEMO: i32 = 1;
    pub const DISALLOW_APPS_CONTROL: i32 = 2;
    pub const DEVICE_OWNER: i32 = 3;
    pub const PROFILE_OWNER: i32 = 4;
    pub const PERSISTENT: i32 = 5;
    pub const SYSTEM_MODULE: i32 = 6;
    pub const CARRIER: i32 = 7;
    pub const PROTECTED: i32 = 8;
    pub const ACTIVE_ADMIN: i32 = 9;
    pub const VPN: i32 = 10;
    pub const PLATFORM_VPN: i32 = 11;
    pub const DIALER: i32 = 12;
    pub const EMERGENCY: i32 = 13;
    pub const USER_ALLOWLIST: i32 = 14;
    pub const COMPANION: i32 = 15;
    pub const INSTALLER: i32 = 16;
    pub const MEDIA_PROJECTION: i32 = 17;
}

pub(super) struct TypeHost(pub Arc<AppHost>);

impl Service for TypeHost {
    fn descriptor(&self) -> &str {
        inputs::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != 1000 {
            return reply(Err(Exception::security(
                "foreground service inputs require system UID",
            )));
        }
        if call.code != inputs::HAS_FOREGROUND_SERVICE_EXEMPTION {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        let a =
            inputs::HasForegroundServiceExemption::read(&mut call.data).map_err(|_| BAD_VALUE)?;
        let result = self
            .0
            .foreground_service_exemption(a.fact, a.uid, a.package_name.as_deref());
        reply(result.map(|value| {
            let mut p = Parcel::new();
            inputs::write_has_foreground_service_exemption_reply(&mut p, value);
            p
        }))
    }
}

impl AppHost {
    fn foreground_service_exemption(
        &self,
        fact: i32,
        uid: i32,
        package: Option<&str>,
    ) -> Result<bool, Exception> {
        use super::appops_service as ops;
        use crate::package::query::Query;
        let package = package.ok_or_else(|| Exception::illegal_argument("no FGS package"))?;
        let user = uid / PER_USER_RANGE as i32;
        match fact {
            facts::SYSTEM_ALLOWLIST => self.device_idle().fgs_allowlisted(uid, true),
            facts::USER_ALLOWLIST => self.device_idle().fgs_allowlisted(uid, false),
            facts::DEMO => Ok(self
                .inputs()?
                .settings
                .get(super::settings::Table::Global, "device_demo_mode")
                .and_then(|v| v.parse::<i32>().ok())
                .is_some_and(|v| v > 0)),
            facts::DISALLOW_APPS_CONTROL => self.has_user_restriction("no_control_apps", user),
            facts::DEVICE_OWNER | facts::PROFILE_OWNER => {
                let (state, _) = self.packages()?;
                let packages = state
                    .packages
                    .iter()
                    .filter(|(_, p)| p.app_id == uid % PER_USER_RANGE as i32)
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>();
                Ok(self
                    .device_policies()?
                    .fgs_owner(&packages, user, fact == facts::DEVICE_OWNER))
            }
            // The app host's process-state owner currently reports TOP for
            // every running app, including apps marked persistent.
            facts::PERSISTENT => Ok(false),
            facts::SYSTEM_MODULE => {
                let (state, _) = self.packages()?;
                let modules = state.system.module_metadata.as_ref().ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "module metadata owner unavailable",
                    )
                })?;
                Ok(modules
                    .modules()
                    .iter()
                    .any(|m| m.package.as_deref() == Some(package))
                    || state
                        .packages
                        .get(package)
                        .is_some_and(|p| p.path.starts_with("/apex/")))
            }
            facts::CARRIER => {
                let (state, _) = self.packages()?;
                if state
                    .system
                    .features
                    .iter()
                    .any(|(name, _)| name.starts_with("android.hardware.telephony"))
                {
                    Err(self.unimplemented_named(
                        "activity",
                        "carrier-privileged FGS exemption (#1552)",
                    ))
                } else {
                    Ok(false)
                }
            }
            facts::PROTECTED => self.data_protected(package),
            facts::ACTIVE_ADMIN => Ok(self
                .device_policies()?
                .package_has_active_admins(Some(package), user)),
            facts::VPN | facts::PLATFORM_VPN => Ok(ops::mode(
                self,
                if fact == facts::VPN {
                    ops::OP_ACTIVATE_VPN
                } else {
                    ops::OP_ACTIVATE_PLATFORM_VPN
                },
                uid,
                Some(package),
            )? == ops::MODE_ALLOWED),
            facts::DIALER | facts::EMERGENCY => {
                let role = if fact == facts::DIALER {
                    "android.app.role.DIALER"
                } else {
                    "android.app.role.EMERGENCY"
                };
                let owner = self.role_service.get().ok_or_else(|| {
                    self.unimplemented_named("role", "FGS role permission (#1657)")
                })?;
                let (state, _) = self.packages()?;
                Ok(owner.holders_of(role)?.iter().any(|name| {
                    state
                        .packages
                        .get(name)
                        .is_some_and(|p| p.app_id == uid % PER_USER_RANGE as i32)
                }))
            }
            facts::COMPANION => {
                super::companion_device::CompanionDevice::new(self.me.upgrade().unwrap())
                    .no_associations(user)?;
                Ok(false)
            }
            facts::INSTALLER => {
                let (state, resolution) = self.packages()?;
                let query = Query {
                    state: &state,
                    filter: &resolution.apps_filter,
                    calling_uid: 1000,
                };
                let owner = state.system.roles.as_ref().ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "KnownPackages owner unavailable",
                    )
                })?;
                Ok(owner
                    .known_packages(&query, 2, 0)
                    .map_err(|e| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("KnownPackages: {e:?}"),
                        )
                    })?
                    .iter()
                    .any(|p| p.as_deref() == Some(package)))
            }
            facts::MEDIA_PROJECTION => {
                Err(self
                    .unimplemented_named("activity", "media projection permission owner (#1552)"))
            }
            _ => Err(Exception::illegal_argument("unknown FGS exemption input")),
        }
    }
}
