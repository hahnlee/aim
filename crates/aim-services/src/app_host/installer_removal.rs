//! `IInstallerRemovalBridge` for the app host: the native installer's
//! removal and archive owners, which system_server's
//! `InstallerRemovalBridge` serves, to the system uid only. Permission
//! checks, users and task policy come from their app-host owners. Archive
//! and user-action leaves remain unsupported (#1757 / #1996).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::dev_aim_server_iinstallerremovalbridge as bridge;
use aim_service_aidl::{Returned, android_os_iinstalld as installd};

use super::kept::{RawBundle, RawIntent};
use super::{AppHost, reply};
use crate::bundle::Value;
use crate::package::installer::preapproval::IntentSender;

const SYSTEM_UID: u32 = 1000;
/// `android.content.pm.Flags.sdkLibIndependence`: a read-only flag the
/// pinned release builds off.
const SDK_LIB_INDEPENDENCE: bool = false;
/// `IInstalld.FLAG_STORAGE_EXTERNAL` (DE and CE live in `app_data`).
const FLAG_STORAGE_EXTERNAL: i32 = 4;

pub struct InstallerRemoval {
    host: Arc<AppHost>,
}

impl InstallerRemoval {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerRemoval { host }
    }

    fn send(&self, receiver: Option<IntentSender>, intent: RawIntent) -> Result<(), Exception> {
        let Some(receiver) = receiver else {
            return Ok(());
        };
        let options = RawBundle::empty()
            .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "installer broadcast options"))?;
        let result = self.host.send_intent_sender(
            SYSTEM_UID as i32,
            None,
            Some(receiver.target),
            0,
            Some(intent),
            None,
            None,
            None,
            Some(options),
        )?;
        if result < 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer status delivery failed",
            ));
        }
        Ok(())
    }

    fn installd(
        &self,
        code: u32,
        args: &Parcel,
        read: impl FnOnce(&mut Reader<'_>) -> Result<Returned<()>, i32>,
    ) -> Result<(), Exception> {
        let fail = |e| Exception::new(EX_ILLEGAL_STATE, format!("installer removal installd: {e}"));
        let owner = self.host.installd().map_err(fail)?;
        let response = owner
            .transact(code, args, false)
            .map_err(|e| fail(e.to_string()))?;
        read(&mut response.reader()).map_err(|e| fail(e.to_string()))?
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
            bridge::SEND_UNINSTALL_STATUS => {
                let a = bridge::SendUninstallStatus::<IntentSender>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.send(
                        a.receiver,
                        uninstall_status(a.package_name, a.status, a.message),
                    )
                    .map(|()| {
                        bridge::write_send_uninstall_status_reply(&mut p);
                        p
                    }),
                )
            }
            bridge::SEND_REMOVED_BROADCAST => {
                let a =
                    bridge::SendRemovedBroadcast::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let package = a
                        .package_name
                        .as_deref()
                        .ok_or_else(|| Exception::illegal_argument("package name is null"))?;
                    self.host
                        .package_writes
                        .get_or_init(|| super::package_writes::Writes::new(self.host.clone()))
                        .removed(
                            package,
                            a.app_id,
                            a.user,
                            a.data_removed,
                            a.fully_removed,
                            a.uid_removed,
                        )?;
                    if a.uid_removed {
                        self.host
                            .foreground_service_uid_removed(a.user * 100_000 + a.app_id)?;
                    }
                    bridge::write_send_removed_broadcast_reply(&mut p);
                    Ok(p)
                })())
            }
            bridge::PREPARE_PERMISSIONS => {
                let a = bridge::PreparePermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let owner = self.host.permission_manager.get().ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable")
                    })?;
                    let package = a
                        .package_name
                        .as_deref()
                        .ok_or_else(|| Exception::illegal_argument("package name is null"))?;
                    let scope = owner.prepare_removal(package, a.app_id, a.user)?;
                    bridge::write_prepare_permissions_reply(&mut p, Some(scope));
                    Ok(p)
                })())
            }
            bridge::DESTROY_APP_DATA => {
                let a = bridge::DestroyAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut args = Parcel::new();
                installd::DestroyAppData {
                    uuid: a.volume,
                    package_name: a.package_name,
                    user_id: a.user,
                    flags: super::app_data::FLAG_STORAGE_DE
                        | super::app_data::FLAG_STORAGE_CE
                        | FLAG_STORAGE_EXTERNAL,
                    ce_data_inode: a.ce_inode,
                }
                .write(&mut args);
                reply(
                    self.installd(
                        installd::DESTROY_APP_DATA,
                        &args,
                        installd::read_destroy_app_data_reply,
                    )
                    .map(|()| {
                        bridge::write_destroy_app_data_reply(&mut p);
                        p
                    }),
                )
            }
            bridge::DESTROY_PROFILES => {
                let a = bridge::DestroyProfiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut args = Parcel::new();
                installd::DestroyAppProfiles {
                    package_name: a.package_name,
                }
                .write(&mut args);
                reply(
                    self.installd(
                        installd::DESTROY_APP_PROFILES,
                        &args,
                        installd::read_destroy_app_profiles_reply,
                    )
                    .map(|()| {
                        bridge::write_destroy_profiles_reply(&mut p);
                        p
                    }),
                )
            }
            bridge::REMOVE_CODE => {
                let a = bridge::RemoveCode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut args = Parcel::new();
                installd::RmPackageDir {
                    package_name: a.package_name,
                    package_dir: a.path,
                }
                .write(&mut args);
                reply(
                    self.installd(
                        installd::RM_PACKAGE_DIR,
                        &args,
                        installd::read_rm_package_dir_reply,
                    )
                    .map(|()| {
                        bridge::write_remove_code_reply(&mut p);
                        p
                    }),
                )
            }
            bridge::CHECK_PACKAGE => {
                let a = bridge::CheckPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let owns = match a.package_name.as_deref() {
                        Some(package) => self.host.owns_package(a.uid, package)?,
                        None => false,
                    };
                    if !owns {
                        return Err(Exception::security(format!(
                            "Package {} does not belong to {}",
                            a.package_name.as_deref().unwrap_or("null"),
                            a.uid
                        )));
                    }
                    bridge::write_check_package_reply(&mut p);
                    Ok(p)
                })())
            }
            bridge::CAN_SILENTLY_INSTALL => {
                let a = bridge::CanSilentlyInstall::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.host
                        .device_policies()
                        .and_then(|policies| {
                            policies.can_silently_install_package(
                                a.package_name.as_deref(),
                                a.uid,
                                |package| self.host.owns_package(a.uid, package),
                            )
                        })
                        .map(|can| {
                            bridge::write_can_silently_install_reply(&mut p, can);
                            p
                        }),
                )
            }
            bridge::HAS_ACTIVE_ADMIN => {
                let a = bridge::HasActiveAdmin::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.host.device_policies().map(|policies| {
                    bridge::write_has_active_admin_reply(
                        &mut p,
                        policies.package_has_active_admins(a.package_name.as_deref(), a.user),
                    );
                    p
                }))
            }
            bridge::IS_PINNED => {
                let a = bridge::IsPinned::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_is_pinned_reply(
                    &mut p,
                    self.host
                        .tasks
                        .is_base_of_locked_task(a.package_name.as_deref()),
                );
                Ok(p)
            }
            bridge::IS_UNINSTALL_RESTRICTED => {
                let a =
                    bridge::IsUninstallRestricted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.host
                        .has_user_restriction("no_uninstall_apps", a.user)
                        .map(|restricted| {
                            bridge::write_is_uninstall_restricted_reply(&mut p, restricted);
                            p
                        }),
                )
            }
            bridge::GET_CHILDREN_DELETED_WITH_PARENT => {
                let a = bridge::GetChildrenDeletedWithParent::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                bridge::write_get_children_deleted_with_parent_reply(
                    &mut p,
                    &Some(super::user_service::children_deleted_with_parent(a.user)),
                );
                Ok(p)
            }
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

/// `PackageManager.deleteStatusToPublicStatus` / `deleteStatusToString`.
fn uninstall_status(package: Option<String>, status: i32, message: Option<String>) -> RawIntent {
    let (public, label) = match status {
        1 => (0, "DELETE_SUCCEEDED".into()),
        -1 => (1, "DELETE_FAILED_INTERNAL_ERROR".into()),
        -2 => (2, "DELETE_FAILED_DEVICE_POLICY_MANAGER".into()),
        -3 => (2, "DELETE_FAILED_USER_RESTRICTED".into()),
        -4 => (2, "DELETE_FAILED_OWNER_BLOCKED".into()),
        -5 => (3, "DELETE_FAILED_ABORTED".into()),
        -6 => (5, "DELETE_FAILED_USED_SHARED_LIBRARY".into()),
        -7 => (2, "DELETE_FAILED_APP_PINNED".into()),
        _ => (1, status.to_string()),
    };
    let message = message.map_or_else(|| label.clone(), |message| format!("{label}: {message}"));
    RawIntent::with_data(
        None,
        None,
        None,
        0,
        &[
            (
                "android.content.pm.extra.PACKAGE_NAME",
                Value::String(package),
            ),
            ("android.content.pm.extra.STATUS", Value::Int(public)),
            (
                "android.content.pm.extra.STATUS_MESSAGE",
                Value::String(Some(message)),
            ),
            ("android.content.pm.extra.LEGACY_STATUS", Value::Int(status)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Binder;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, EX_UNSUPPORTED_OPERATION};
    use std::sync::{Mutex, mpsc};

    struct Registry(std::collections::BTreeMap<String, Binder>);
    impl Service for Registry {
        fn descriptor(&self) -> &str {
            aim_service_aidl::android_os_iservicemanager::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            use aim_service_aidl::android_os_iservicemanager as sm;
            let a = sm::CheckService::read(&mut call.data)?;
            let mut p = Parcel::new();
            sm::write_check_service_reply(&mut p, self.0.get(&a.name.unwrap()).copied());
            Ok(p)
        }
    }
    struct Owners {
        driver: Arc<aim_binder_driver::Driver>,
        remote: Arc<aim_binder_host::local::LocalProcess>,
        host: Arc<AppHost>,
    }
    impl Owners {
        fn new(data: Arc<DataOwner>, status: Arc<StatusReceiver>) -> Self {
            use aim_binder_driver::{Credentials, Device, uapi::*};
            use aim_binder_host::local::LocalProcess;
            let driver = aim_binder_driver::Driver::new();
            let open = |pid| {
                LocalProcess::open(
                    &driver,
                    Device::Binder,
                    Credentials {
                        pid,
                        euid: SYSTEM_UID,
                        security_context: None,
                    },
                )
            };
            let remote = open(97151);
            let host = AppHost::new(open(97152));
            let nodes = [
                ("installd".into(), remote.add_service(data)),
                ("test.removal.status".into(), remote.add_service(status)),
            ]
            .into();
            let Binder::Local(ptr) = remote.add_service(Arc::new(Registry(nodes))) else {
                unreachable!()
            };
            let mut object = FlatBinderObject {
                kind: BINDER_TYPE_BINDER,
                flags: 0,
                binder: ptr,
                cookie: ptr,
            }
            .encode();
            struct NoMemory;
            impl aim_binder_driver::GuestProcess for NoMemory {
                fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
                    Err(aim_binder_driver::errno::EFAULT)
                }
                fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
                    Err(aim_binder_driver::errno::EFAULT)
                }
                fn get_file(&mut self, _: u32) -> Result<aim_binder_driver::File, i32> {
                    Err(aim_binder_driver::errno::EBADF)
                }
                fn install_file(&mut self, _: aim_binder_driver::File) -> Result<u32, i32> {
                    Err(aim_binder_driver::errno::EBADF)
                }
                fn close_fd(&mut self, _: u32) {
                    panic!("no fixture files")
                }
            }
            driver
                .ioctl(
                    remote.proc_handle(),
                    97151,
                    BINDER_SET_CONTEXT_MGR_EXT,
                    &mut object,
                    &mut NoMemory,
                )
                .unwrap();
            remote.start();
            Self {
                driver,
                remote,
                host,
            }
        }
    }
    impl Drop for Owners {
        fn drop(&mut self) {
            self.driver.release(self.host.process.proc_handle());
            self.driver.release(self.remote.proc_handle());
        }
    }

    struct DataOwner(Mutex<Vec<u32>>);
    impl Service for DataOwner {
        fn descriptor(&self) -> &str {
            installd::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, SYSTEM_UID);
            let mut p = Parcel::new();
            match call.code {
                installd::DESTROY_APP_DATA => {
                    let a = installd::DestroyAppData::read(&mut call.data)?;
                    assert_eq!(
                        (
                            a.uuid.as_deref(),
                            a.package_name.as_deref(),
                            a.user_id,
                            a.flags,
                            a.ce_data_inode
                        ),
                        (None, Some("p"), 0, 7, 991)
                    );
                    installd::write_destroy_app_data_reply(&mut p);
                }
                installd::DESTROY_APP_PROFILES => {
                    let a = installd::DestroyAppProfiles::read(&mut call.data)?;
                    assert_eq!(a.package_name.as_deref(), Some("p"));
                    installd::write_destroy_app_profiles_reply(&mut p);
                }
                installd::RM_PACKAGE_DIR => {
                    let a = installd::RmPackageDir::read(&mut call.data)?;
                    assert_eq!(
                        (a.package_name.as_deref(), a.package_dir.as_deref()),
                        (Some("p"), Some("/data/app/p"))
                    );
                    p.write_exception(&Exception::security("installd refused removal"));
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            self.0.lock().unwrap().push(call.code);
            Ok(p)
        }
    }

    struct StatusReceiver(mpsc::Sender<RawIntent>);
    impl Service for StatusReceiver {
        fn descriptor(&self) -> &str {
            aim_service_aidl::android_content_iintentsender::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            use aim_service_aidl::android_content_iintentsender as sender;
            assert_eq!(call.code, sender::SEND);
            let a = sender::Send::<RawIntent, RawBundle>::read(&mut call.data)?;
            assert_eq!(a.code, 0);
            assert!(a.options.is_some());
            self.0.send(a.intent.unwrap()).unwrap();
            Ok(Parcel::new())
        }
    }

    #[test]
    fn installd_calls_and_local_and_remote_status_use_their_binders() {
        let data = Arc::new(DataOwner(Mutex::default()));
        let (send, receive) = mpsc::channel();
        let owners = Owners::new(data.clone(), Arc::new(StatusReceiver(send.clone())));
        let host = &owners.host;
        let leaf = InstallerRemoval::new(host.clone());
        let r = call(&leaf, bridge::DESTROY_APP_DATA, |p| {
            bridge::DestroyAppData {
                volume: None,
                package_name: Some("p".into()),
                user: 0,
                ce_inode: 991,
            }
            .write(p)
        });
        r.reader().read_exception().unwrap().unwrap();
        let r = call(&leaf, bridge::DESTROY_PROFILES, |p| {
            bridge::DestroyProfiles {
                package_name: Some("p".into()),
            }
            .write(p)
        });
        r.reader().read_exception().unwrap().unwrap();
        let r = call(&leaf, bridge::REMOVE_CODE, |p| {
            bridge::RemoveCode {
                package_name: Some("p".into()),
                path: Some("/data/app/p".into()),
            }
            .write(p)
        });
        assert_eq!(
            r.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        assert_eq!(
            *data.0.lock().unwrap(),
            [
                installd::DESTROY_APP_DATA,
                installd::DESTROY_APP_PROFILES,
                installd::RM_PACKAGE_DIR
            ]
        );
        let local = host
            .process
            .add_service(Arc::new(StatusReceiver(send.clone())));
        let retained =
            super::super::display::check_service(&host.process, "test.removal.status").unwrap();
        for target in [local, retained.binder()] {
            let r = call(&leaf, bridge::SEND_UNINSTALL_STATUS, |p| {
                bridge::SendUninstallStatus {
                    receiver: Some(IntentSender { target }),
                    package_name: Some("p".into()),
                    status: 1,
                    message: None,
                }
                .write(p)
            });
            r.reader().read_exception().unwrap().unwrap();
            let intent = receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert_eq!(intent.intent.action, None);
            let values = intent.test_extras();
            assert_eq!(
                values["android.content.pm.extra.PACKAGE_NAME"],
                Value::String(Some("p".into()))
            );
            assert_eq!(values["android.content.pm.extra.STATUS"], Value::Int(0));
            assert_eq!(
                values["android.content.pm.extra.LEGACY_STATUS"],
                Value::Int(1)
            );
            assert_eq!(
                values["android.content.pm.extra.STATUS_MESSAGE"],
                Value::String(Some("DELETE_SUCCEEDED".into()))
            );
        }
        assert!(receive.try_recv().is_err());
        leaf.send(None, uninstall_status(None, 1, None)).unwrap();
        assert!(
            leaf.send(
                Some(IntentSender {
                    target: Binder::Handle(u32::MAX)
                }),
                uninstall_status(None, 1, None)
            )
            .is_err()
        );
    }

    fn call(leaf: &InstallerRemoval, code: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
        let mut p = Parcel::new();
        args(&mut p);
        leaf.transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: SYSTEM_UID,
            data: p.reader(),
        })
        .unwrap()
    }

    #[test]
    fn uninstall_policy_uses_the_package_user_and_device_owners() {
        crate::system::with_discovery_session(|host, _, _| {
            let data = host.data().unwrap();
            std::fs::write(data.join("system/device_owner_2.xml"),
                br#"<root><device-owner component="p/.Admin"/><device-owner-context userId="0"/></root>"#).unwrap();
            std::fs::write(
                data.join("system/device_policies.xml"),
                br#"<policies><admin name="p/.Admin"/></policies>"#,
            )
            .unwrap();
            host.user_restrictions
                .set(["no_uninstall_apps".into()].into())
                .unwrap();
            let leaf = InstallerRemoval::new(host.clone());
            for (uid, owns) in [(10100, true), (10101, false)] {
                let r = call(&leaf, bridge::CHECK_PACKAGE, |p| {
                    bridge::CheckPackage {
                        uid,
                        package_name: Some("p".into()),
                    }
                    .write(p)
                });
                assert_eq!(r.reader().read_exception().unwrap().is_ok(), owns);
                let r = call(&leaf, bridge::CAN_SILENTLY_INSTALL, |p| {
                    bridge::CanSilentlyInstall {
                        uid,
                        package_name: Some("p".into()),
                    }
                    .write(p)
                });
                assert_eq!(
                    bridge::read_can_silently_install_reply(&mut r.reader())
                        .unwrap()
                        .unwrap(),
                    owns
                );
            }
            for (user, admin) in [(0, true), (10, false)] {
                let r = call(&leaf, bridge::HAS_ACTIVE_ADMIN, |p| {
                    bridge::HasActiveAdmin {
                        user,
                        package_name: Some("p".into()),
                    }
                    .write(p)
                });
                assert_eq!(
                    bridge::read_has_active_admin_reply(&mut r.reader())
                        .unwrap()
                        .unwrap(),
                    admin
                );
                let r = call(&leaf, bridge::GET_CHILDREN_DELETED_WITH_PARENT, |p| {
                    bridge::GetChildrenDeletedWithParent { user }.write(p)
                });
                assert_eq!(
                    bridge::read_get_children_deleted_with_parent_reply(&mut r.reader())
                        .unwrap()
                        .unwrap(),
                    Some(vec![])
                );
            }
            let r = call(&leaf, bridge::IS_UNINSTALL_RESTRICTED, |p| {
                bridge::IsUninstallRestricted { user: 0 }.write(p)
            });
            assert!(
                bridge::read_is_uninstall_restricted_reply(&mut r.reader())
                    .unwrap()
                    .unwrap()
            );
        });
    }

    /// Only the system uid is served; a null permission is refused as
    /// `Context` refuses it, and an uninstall's owners name their issue.
    #[test]
    fn removal_is_the_systems() {
        let node = |euid: u32| {
            let process = crate::test_process(euid);
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
        assert!(
            !bridge::read_is_pinned_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let mut p = Parcel::new();
        bridge::IsArchiveOptedOut {
            package_name: Some("a.b".into()),
            uid: 10000,
        }
        .write(&mut p);
        let response = node(SYSTEM_UID)
            .transact(bridge::IS_ARCHIVE_OPTED_OUT, &p, false)
            .unwrap();
        assert_eq!(
            response
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
    }
}
