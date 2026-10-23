//! `IPackageEnableBridge` for the app host: the native package boot's
//! feature configuration and installer storage primitive, which
//! system_server's `PackageEnableBridge` serves, to the system uid only.
//! An install's code cache cleared is not implemented (#2162).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageenablebridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `android.content.pm.Flags.quarantinedEnabled`: a read-only flag the
/// pinned release builds off.
const QUARANTINED_ENABLED: bool = false;

pub struct PackageEnable {
    host: Arc<AppHost>,
}

impl PackageEnable {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageEnable { host }
    }
}

impl Service for PackageEnable {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `Bridge.enforceSystemUid`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security(
                    "native package leaf requires system UID",
                ))
            }
        };
        match call.code {
            bridge::QUARANTINE_ENABLED => {
                bridge::QuarantineEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().map(|()| {
                    let mut p = Parcel::new();
                    bridge::write_quarantine_enabled_reply(&mut p, QUARANTINED_ENABLED);
                    p
                }))
            }
            bridge::CLEAR_CODE_CACHE => {
                bridge::ClearCodeCache::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| {
                    Err(self
                        .host
                        .unimplemented_named("package enable", "clearCodeCache (#2162)"))
                }))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_SECURITY;

    /// The quarantine flag is the release's, to the system only.
    #[test]
    fn quarantine_follows_the_release() {
        let node = |euid: u32| {
            let process = aim_binder_host::local::LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid,
                    security_context: None,
                },
            );
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(PackageEnable::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let quarantine = |euid: u32| {
            let mut p = Parcel::new();
            bridge::QuarantineEnabled {}.write(&mut p);
            let reply = node(euid)
                .transact(bridge::QUARANTINE_ENABLED, &p, false)
                .unwrap();
            bridge::read_quarantine_enabled_reply(&mut reply.reader()).unwrap()
        };
        assert!(!quarantine(SYSTEM_UID).unwrap());
        assert_eq!(quarantine(10000).unwrap_err().code, EX_SECURITY);
    }
}
