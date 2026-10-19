//! `network_management`: NetworkManagementService for the app host
//! (`android-16.0.0_r1`): bandwidth control is always on, as the
//! original's has been since the traffic accounting moved to BPF; the
//! uids' cleartext network policies (StrictMode's), kept until netd is
//! connected as the original stashes them. Applying one to netd
//! (`strictUidCleartextPenalty`) and the rest are not implemented (#1714).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::android_os_inetworkmanagementservice as nm;

use super::{AppHost, reply};

const NETWORK_STACK: &str = "android.permission.NETWORK_STACK";
const MAINLINE_NETWORK_STACK: &str = "android.permission.MAINLINE_NETWORK_STACK";
/// `StrictMode.NETWORK_POLICY_ACCEPT`.
const NETWORK_POLICY_ACCEPT: i32 = 0;

pub struct NetworkManagement {
    host: Arc<AppHost>,
    /// `mUidCleartextPolicy`.
    cleartext: Mutex<HashMap<i32, i32>>,
}

impl NetworkManagement {
    pub fn new(host: Arc<AppHost>) -> Self {
        NetworkManagement { host, cleartext: Mutex::default() }
    }

    /// `setUidCleartextNetworkPolicy`: another uid's with a network stack
    /// permission; nothing to do for the policy it has; stashed until netd
    /// is connected (`mStrictEnabled`).
    fn set_cleartext_policy(&self, caller: i32, uid: i32, policy: i32) -> Result<(), Exception> {
        if caller != uid {
            // `PermissionUtils.enforceNetworkStackPermission`.
            let held = |p: &str| matches!(self.host.check_permission(Some(p), caller), Ok(0));
            if !held(NETWORK_STACK) && !held(MAINLINE_NETWORK_STACK) {
                return Err(Exception::security(format!(
                    "Requires one of the following permissions: {NETWORK_STACK}, {MAINLINE_NETWORK_STACK}."
                )));
            }
        }
        let mut cleartext = self.cleartext.lock().unwrap();
        if cleartext.get(&uid).copied().unwrap_or(NETWORK_POLICY_ACCEPT) == policy {
            return Ok(());
        }
        let strict_enabled = self.host.connectivity_service.get().is_some_and(|c| c.netd_connected());
        if !strict_enabled {
            cleartext.insert(uid, policy);
            return Ok(());
        }
        Err(self.host.unimplemented_named("network_management", "netd's strictUidCleartextPenalty (#1714)"))
    }
}

impl Service for NetworkManagement {
    fn descriptor(&self) -> &str {
        nm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            nm::SET_UID_CLEARTEXT_NETWORK_POLICY => {
                let a = nm::SetUidCleartextNetworkPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.set_cleartext_policy(call.sender_euid as i32, a.uid, a.policy).map(|()| {
                    let mut p = Parcel::new();
                    nm::write_set_uid_cleartext_network_policy_reply(&mut p);
                    p
                }))
            }
            nm::IS_BANDWIDTH_CONTROL_ENABLED => {
                nm::IsBandwidthControlEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                nm::write_is_bandwidth_control_enabled_reply(&mut p, true);
                Ok(p)
            }
            code => {
                let name = nm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                reply(Err(self.host.unimplemented_named("network_management", &format!("{name} (#1714)"))))
            }
        }
    }
}
