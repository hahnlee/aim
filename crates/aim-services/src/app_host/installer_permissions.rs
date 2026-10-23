//! `IInstallerPermissionBridge` for the app host: the install
//! environment's permission owner, which system_server's
//! `InstallerPermissionBridge` serves, to the system uid only. An
//! install's permission state, prepared and rolled back, is not
//! implemented (#2161).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerpermissionbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct InstallerPermissions {
    host: Arc<AppHost>,
}

impl InstallerPermissions {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerPermissions { host }
    }
}

impl Service for InstallerPermissions {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let name = match call.code {
            bridge::PREPARE => {
                bridge::Prepare::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                "prepare (#2161)"
            }
            bridge::ROLLBACK => {
                bridge::Rollback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                "rollback (#2161)"
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        // `Bridge.enforceSystemUid`.
        reply(Err(if call.sender_euid != SYSTEM_UID {
            Exception::security("native package leaf requires system UID")
        } else {
            self.host.unimplemented_named("installer permissions", name)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION, Parcel};

    /// Only the system uid is served; an install's permissions name the
    /// issue.
    #[test]
    fn install_permissions_are_the_systems() {
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
                process.add_service(Arc::new(InstallerPermissions::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let rollback = |euid: u32| {
            let mut p = Parcel::new();
            bridge::Rollback {
                candidate: None,
                current: None,
                package_names: Some(vec![Some("a.b".into())]),
                cross_user_suspensions: false,
            }
            .write(&mut p);
            let reply = node(euid).transact(bridge::ROLLBACK, &p, false).unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(rollback(10000), EX_SECURITY);
        assert_eq!(rollback(SYSTEM_UID), EX_UNSUPPORTED_OPERATION);
    }
}
