//! `IApplicationDataBridge` for the app host: the native package boot's
//! application data owners, which system_server's
//! `NativeApplicationDataBridge` serves, to root and the system uid. A
//! permission is enforced as `Context.enforcePermission` enforces it; the
//! memory check is DeviceStorageMonitorService's, which the app host does
//! not run, so none is made; the preloads' file cache is emptied. A
//! package's processes killed, its data cleared and the customization
//! broadcast are not implemented (#2164).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iapplicationdatabridge as bridge;

use super::{AppHost, reply};

const ROOT_UID: u32 = 0;
const SYSTEM_UID: u32 = 1000;
/// `PackageManager.PERMISSION_GRANTED`.
const GRANTED: i32 = 0;

pub struct ApplicationData {
    host: Arc<AppHost>,
}

impl ApplicationData {
    pub fn new(host: Arc<AppHost>) -> Self {
        ApplicationData { host }
    }

    /// `Context.enforcePermission(permission, pid, uid, permission)`.
    fn enforce_permission(&self, permission: Option<String>, uid: i32) -> Result<(), Exception> {
        let permission =
            permission.ok_or_else(|| Exception::illegal_argument("permission is null"))?;
        if self.host.check_permission(Some(&permission), uid)? == GRANTED {
            return Ok(());
        }
        Err(Exception::security(format!(
            "{permission}: uid {uid} does not have {permission}."
        )))
    }
}

impl Service for ApplicationData {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(bridge::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // `enforce`.
        if call.sender_euid != ROOT_UID && call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "untrusted application data owner caller",
            )));
        }
        let mut p = Parcel::new();
        match call.code {
            bridge::ENFORCE_PERMISSION => {
                let a = bridge::EnforcePermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.enforce_permission(a.permission, a.uid).map(|()| {
                    bridge::write_enforce_permission_reply(&mut p);
                    p
                }))
            }
            bridge::CHECK_MEMORY => {
                bridge::CheckMemory::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_check_memory_reply(&mut p);
                Ok(p)
            }
            // `FileUtils.deleteContents(getDataPreloadsFileCacheDirectory())`.
            bridge::DELETE_PRELOADS => {
                bridge::DeletePreloads::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.host.data().map(|data| {
                    super::app_data::delete_contents(&data.join("preloads/file_cache"), false);
                    bridge::write_delete_preloads_reply(&mut p);
                    p
                }))
            }
            _ => reply(Err(self.host.unimplemented_named(
                "application data",
                &format!("{name} (#2164)"),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Root and the system are served; a null permission is refused as
    /// `Context` refuses it; a package's data names its issue.
    #[test]
    fn application_data_is_the_systems() {
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
                process.add_service(Arc::new(ApplicationData::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let code = |euid: u32, code: u32, p: &Parcel| {
            let reply = node(euid).transact(code, p, false).unwrap();
            reply
                .reader()
                .read_exception()
                .unwrap()
                .err()
                .map(|e| e.code)
        };
        let mut memory = Parcel::new();
        bridge::CheckMemory {}.write(&mut memory);
        assert_eq!(
            code(10000, bridge::CHECK_MEMORY, &memory),
            Some(EX_SECURITY)
        );
        assert_eq!(code(ROOT_UID, bridge::CHECK_MEMORY, &memory), None);
        let mut enforce = Parcel::new();
        bridge::EnforcePermission {
            permission: None,
            pid: 1,
            uid: 10000,
        }
        .write(&mut enforce);
        assert_eq!(
            code(SYSTEM_UID, bridge::ENFORCE_PERMISSION, &enforce),
            Some(EX_ILLEGAL_ARGUMENT)
        );
        let mut clear = Parcel::new();
        bridge::ClearData {
            package_name: Some("a.b".into()),
            user: 0,
        }
        .write(&mut clear);
        assert_eq!(
            code(SYSTEM_UID, bridge::CLEAR_DATA, &clear),
            Some(EX_UNSUPPORTED_OPERATION)
        );
    }
}
