//! `IInstallerArchiveQueryBridge` for the app host: the native archive
//! queries' launcher, app-op and graphics owners, which system_server's
//! `InstallerArchiveQueryBridge` serves, to the system uid only. They are
//! part of archiving, not implemented (#1757).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{Exception, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerarchivequerybridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct InstallerArchiveQueries {
    host: Arc<AppHost>,
}

impl InstallerArchiveQueries {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerArchiveQueries { host }
    }
}

impl Service for InstallerArchiveQueries {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(bridge::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        reply(Err(if call.sender_euid != SYSTEM_UID {
            Exception::security("native installer bridge requires system UID")
        } else {
            self.host
                .unimplemented_named("installer archive queries", &format!("{name} (#1757)"))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION, Parcel};

    /// Only the system uid is served; the queries name archiving's issue.
    #[test]
    fn archive_queries_are_the_systems() {
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
                process.add_service(Arc::new(InstallerArchiveQueries::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let mode = |euid: u32| {
            let mut p = Parcel::new();
            bridge::GetArchiveOverlayMode {
                uid: 10000,
                package_name: Some("a.b".into()),
            }
            .write(&mut p);
            let reply = node(euid)
                .transact(bridge::GET_ARCHIVE_OVERLAY_MODE, &p, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(mode(10000), EX_SECURITY);
        assert_eq!(mode(SYSTEM_UID), EX_UNSUPPORTED_OPERATION);
    }
}
