//! `vpn_management`: VpnManagerService for the app host
//! (`android-16.0.0_r1`), whose `VpnManager` Settings and apps make. No VPN
//! is established in the app host, so user 0's `Vpn` has no configuration
//! (`getVpnConfig`). The rest is not implemented (#1640).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_net_ivpnmanager as vm;

use super::{AppHost, PER_USER_RANGE, reply};

const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const CONTROL_VPN: &str = "android.permission.CONTROL_VPN";

/// `VpnConfig`, of which no user has one.
enum NoConfig {}

impl aim_service_aidl::WriteParcelable for NoConfig {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

pub struct VpnManager {
    host: Arc<AppHost>,
}

impl VpnManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        VpnManager { host }
    }

    /// `Context.enforceCallingOrSelfPermission`.
    fn enforce(&self, uid: i32, permission: &str, message: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? != 0 {
            return Err(Exception::security(format!(
                "{message}: Neither user {uid} nor current process has {permission}."
            )));
        }
        Ok(())
    }

    /// `getVpnConfig`: across users with INTERACT_ACROSS_USERS_FULL
    /// (`enforceCrossUserPermission`); user 0's `Vpn` with CONTROL_VPN
    /// (`Vpn.getVpnConfig`), its configuration none.
    fn vpn_config(&self, uid: i32, user: i32) -> Result<(), Exception> {
        if user != uid / PER_USER_RANGE as i32 {
            self.enforce(uid, INTERACT_ACROSS_USERS_FULL, "VpnManagerService")?;
        }
        if user == 0 {
            self.enforce(uid, CONTROL_VPN, "Unauthorized Caller")?;
        }
        Ok(())
    }
}

impl Service for VpnManager {
    fn descriptor(&self) -> &str {
        vm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        match call.code {
            vm::GET_VPN_CONFIG => {
                let a = vm::GetVpnConfig::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.vpn_config(uid, a.user_id).map(|()| {
                    let mut p = Parcel::new();
                    vm::write_get_vpn_config_reply::<NoConfig>(&mut p, None);
                    p
                }))
            }
            code => reply(Err(self.host.unimplemented(
                "vpn_management",
                self.descriptor(),
                code,
            ))),
        }
    }
}
