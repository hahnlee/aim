//! `IInstallerPreparationBridge` for the app host: the native installer's
//! staged and streaming preparation, which system_server's
//! `NativeInstallerPreparationBridge` serves, to the system uid only.
//! Whether storage checkpoints are supported is vold's answer; an
//! install's checkpoints, rollback ids, staged APEXes and data loaders are
//! not implemented (#2163).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerpreparationbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct InstallerPreparation {
    host: Arc<AppHost>,
}

impl InstallerPreparation {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerPreparation { host }
    }
}

impl Service for InstallerPreparation {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(bridge::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // `enforce`.
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "native installer preparation requires system UID",
            )));
        }
        if call.code != bridge::SUPPORTS_CHECKPOINT {
            return reply(Err(self.host.unimplemented_named(
                "installer preparation",
                &format!("{name} (#2163)"),
            )));
        }
        bridge::SupportsCheckpoint::read(&mut call.data).map_err(|_| BAD_VALUE)?;
        reply(self.host.supports_checkpoint().map(|supported| {
            let mut p = Parcel::new();
            bridge::write_supports_checkpoint_reply(&mut p, supported);
            p
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Only the system uid is served; an install's preparation names its
    /// issue.
    #[test]
    fn preparation_is_the_systems() {
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
                process.add_service(Arc::new(InstallerPreparation::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let start = |euid: u32| {
            let mut p = Parcel::new();
            bridge::StartCheckpoint {}.write(&mut p);
            let reply = node(euid)
                .transact(bridge::START_CHECKPOINT, &p, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(start(10000), EX_SECURITY);
        assert_eq!(start(SYSTEM_UID), EX_UNSUPPORTED_OPERATION);
    }
}
