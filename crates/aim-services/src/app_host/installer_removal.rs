//! `IInstallerRemovalBridge` for the app host: the native installer's
//! removal and archive owners, which system_server's
//! `InstallerRemovalBridge` serves, to the system uid only. A permission
//! check, the users and the SDK library independence flag are answered;
//! an archive's owners are part of archiving (#1757) and the others of
//! uninstalling (#1996).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerremovalbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `android.content.pm.Flags.sdkLibIndependence`: a read-only flag the
/// pinned release builds off.
const SDK_LIB_INDEPENDENCE: bool = false;

pub struct InstallerRemoval {
    host: Arc<AppHost>,
}

impl InstallerRemoval {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerRemoval { host }
    }
}

impl Service for InstallerRemoval {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(bridge::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // `enforceNativeOwner`.
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "native installer bridge requires system UID",
            )));
        }
        let mut p = Parcel::new();
        match call.code {
            // `Context.checkPermission`.
            bridge::CHECK_PERMISSION => {
                let a = bridge::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    a.permission
                        .ok_or_else(|| Exception::illegal_argument("permission is null"))
                        .and_then(|permission| self.host.check_permission(Some(&permission), a.uid))
                        .map(|result| {
                            bridge::write_check_permission_reply(&mut p, result);
                            p
                        }),
                )
            }
            // `UserManagerInternal.getUserIds`.
            bridge::GET_USERS => {
                bridge::GetUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    super::user_service::user_ids(&self.host, false).map(|users| {
                        bridge::write_get_users_reply(&mut p, &Some(users));
                        p
                    }),
                )
            }
            bridge::IS_SDK_LIBRARY_INDEPENDENCE_ENABLED => {
                bridge::IsSdkLibraryIndependenceEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                bridge::write_is_sdk_library_independence_enabled_reply(
                    &mut p,
                    SDK_LIB_INDEPENDENCE,
                );
                Ok(p)
            }
            bridge::IS_ARCHIVE_OPTED_OUT
            | bridge::COLLECT_ARCHIVE
            | bridge::COLLECT_ARCHIVED
            | bridge::CLEAR_ARCHIVE_CACHES
            | bridge::SEND_ARCHIVED_INSTALL_STATUS
            | bridge::SEND_UNARCHIVE_CONFIRMATION
            | bridge::SEND_UNARCHIVE_STATUS
            | bridge::BROADCAST_UNARCHIVE => reply(Err(self
                .host
                .unimplemented_named("installer removal", &format!("{name} (#1757)")))),
            _ => reply(Err(self.host.unimplemented_named(
                "installer removal",
                &format!("{name} (#1996)"),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Only the system uid is served; a null permission is refused as
    /// `Context` refuses it, and an uninstall's owners name their issue.
    #[test]
    fn removal_is_the_systems() {
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
                process.add_service(Arc::new(InstallerRemoval::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let check = |euid: u32| {
            let mut p = Parcel::new();
            bridge::CheckPermission {
                permission: None,
                pid: 1,
                uid: 1000,
            }
            .write(&mut p);
            let reply = node(euid)
                .transact(bridge::CHECK_PERMISSION, &p, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(check(10000), EX_SECURITY);
        assert_eq!(check(SYSTEM_UID), EX_ILLEGAL_ARGUMENT);
        let mut p = Parcel::new();
        bridge::IsPinned {
            package_name: Some("a.b".into()),
        }
        .write(&mut p);
        let reply = node(SYSTEM_UID)
            .transact(bridge::IS_PINNED, &p, false)
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
    }
}
