//! `INativeStagingBridge` for the app host: the native staging owner's
//! apexd input, which system_server's `NativeStagingBridge` reads as
//! `ApexManager.getStagedApexInfos` (`android-16.0.0_r1`), to the system
//! uid only. The app host does not read apexd's staged APEXes (#2158).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_inativestagingbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct NativeStaging {
    host: Arc<AppHost>,
}

impl NativeStaging {
    pub fn new(host: Arc<AppHost>) -> Self {
        NativeStaging { host }
    }
}

impl Service for NativeStaging {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            bridge::GET_STAGED_APEX_INFOS => {
                let a = bridge::GetStagedApexInfos::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(Err(if call.sender_euid != SYSTEM_UID {
                    Exception::security("native staging transport requires system UID")
                } else if a.session_id <= 0 || a.apex_child_session_ids.is_none() {
                    Exception::illegal_argument("invalid staged session")
                } else {
                    self.host
                        .unimplemented_named("native staging", "getStagedApexInfos (#2158)")
                }))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_UNSUPPORTED_OPERATION, Parcel};

    /// An invalid session is refused; a valid one names the issue.
    #[test]
    fn staged_apex_infos_are_not_read() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(NativeStaging::new(host)))
        else {
            unreachable!()
        };
        let node = process.local_service(ptr).unwrap();
        let infos = |session_id: i32| {
            let mut p = Parcel::new();
            bridge::GetStagedApexInfos {
                session_id,
                apex_child_session_ids: Some(vec![2]),
            }
            .write(&mut p);
            let reply = node
                .transact(bridge::GET_STAGED_APEX_INFOS, &p, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(infos(0), EX_ILLEGAL_ARGUMENT);
        assert_eq!(infos(1), EX_UNSUPPORTED_OPERATION);
    }
}
