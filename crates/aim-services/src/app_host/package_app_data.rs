//! `IPackageAppDataBridge` for the app host: the native installer's
//! app-data owner, which system_server's `NativePackageAppDataBridge`
//! serves (`android-16.0.0_r1`), to the system uid only. A user's storage
//! flags are answered from the app host's storage preparation: CE once
//! vold and installd have prepared it, else DE while the user runs. An install's app data, made,
//! committed and rolled back, is not implemented (#2160).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageappdatabridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `StorageManager.FLAG_STORAGE_DE`, `FLAG_STORAGE_CE`.
const FLAG_STORAGE_DE: i32 = 1;
const FLAG_STORAGE_CE: i32 = 2;
/// The one user the app host runs (`isUserRunning`).
const SYSTEM_USER: i32 = 0;

pub struct PackageAppData {
    host: Arc<AppHost>,
}

impl PackageAppData {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageAppData { host }
    }

    /// `appDataFlags`.
    fn flags(&self, user: i32) -> Result<i32, Exception> {
        if super::user_service::serial_number(&self.host, user)?.is_none() {
            return Err(Exception::illegal_argument(format!("Unknown user {user}")));
        }
        Ok(if self.host.ce_storage_prepared(user) {
            FLAG_STORAGE_DE | FLAG_STORAGE_CE
        } else if user == SYSTEM_USER {
            FLAG_STORAGE_DE
        } else {
            0
        })
    }
}

impl Service for PackageAppData {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `owner`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security("Native app-data owner required"))
            }
        };
        let unimplemented =
            |name: &str| Err(self.host.unimplemented_named("package app data", name));
        match call.code {
            bridge::APP_DATA_FLAGS => {
                let a = bridge::AppDataFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| self.flags(a.user_id)).map(|flags| {
                    let mut p = Parcel::new();
                    bridge::write_app_data_flags_reply(&mut p, flags);
                    p
                }))
            }
            bridge::CREATE_APP_DATA => {
                bridge::CreateAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| unimplemented("createAppData (#2160)")))
            }
            bridge::COMMIT_APP_DATA => {
                bridge::CommitAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| unimplemented("commitAppData (#2160)")))
            }
            bridge::ROLLBACK_APP_DATA => {
                bridge::RollbackAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| unimplemented("rollbackAppData (#2160)")))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Only the system uid is served; an install's app data names its issue.
    #[test]
    fn app_data_is_the_systems() {
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
                process.add_service(Arc::new(PackageAppData::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let commit = |euid: u32| {
            let mut p = Parcel::new();
            bridge::CommitAppData {
                package_name: Some("a.b".into()),
                user_id: 0,
                ce_data_inode: 1,
            }
            .write(&mut p);
            let reply = node(euid)
                .transact(bridge::COMMIT_APP_DATA, &p, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(commit(10000), EX_SECURITY);
        assert_eq!(commit(SYSTEM_UID), EX_UNSUPPORTED_OPERATION);
    }
}
