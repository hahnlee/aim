//! `IInstallerPolicyBridge` for the app host: the build's flags the native
//! installer's session policy reads, as system_server's
//! `NativeInstallerPolicyBridge` answers them (`android-16.0.0_r1`), to
//! root and the system uid only.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerpolicybridge as bridge;

use super::{AppHost, reply};

const ROOT_UID: u32 = 0;
const SYSTEM_UID: u32 = 1000;
/// `android.content.pm.Flags.rollbackLifetime`.
const ROLLBACK_LIFETIME: &str = "android.content.pm.rollback_lifetime";

pub struct InstallerPolicy {
    host: Arc<AppHost>,
}

/// `getPolicyFlags`: `Build.IS_DEBUGGABLE`, `ApexManager.isApexSupported`
/// and `Flags.rollbackLifetime`, as bits 1, 2 and 4.
fn policy_flags(debuggable: bool, apex_supported: bool, rollback_lifetime: bool) -> i32 {
    debuggable as i32 | (apex_supported as i32) << 1 | (rollback_lifetime as i32) << 2
}

impl InstallerPolicy {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerPolicy { host }
    }

    fn flags(&self) -> Result<i32, Exception> {
        let framework = self.host.framework()?;
        Ok(policy_flags(
            self.host.property("ro.debuggable").as_deref() == Some("1"),
            // `ApexProperties.updatable()`, false where unset.
            self.host.property("ro.apex.updatable").as_deref() == Some("true"),
            framework.0.flags.get(ROLLBACK_LIFETIME) == Some(&true),
        ))
    }
}

impl Service for InstallerPolicy {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::GET_POLICY_FLAGS => {
                bridge::GetPolicyFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if call.sender_euid != ROOT_UID && call.sender_euid != SYSTEM_UID {
                    Err(Exception::security("untrusted installer policy caller"))
                } else {
                    self.flags()
                        .map(|flags| bridge::write_get_policy_flags_reply(&mut p, flags))
                }
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
    fn the_flags_are_the_originals_bits() {
        assert_eq!(policy_flags(false, false, false), 0);
        assert_eq!(policy_flags(true, false, false), 1);
        assert_eq!(policy_flags(false, true, false), 2);
        assert_eq!(policy_flags(false, false, true), 4);
        assert_eq!(policy_flags(true, true, true), 7);
    }
}
