//! The native package boot's internal persistence and install leaves for
//! the app host, to the system uid only: `IPackageApexUninstallBridge`,
//! an APEX unstaged by apexd as `NativePackageApexUninstallBridge` asks
//! `ApexManager.uninstallApex`; and `IPermissionPersistenceBridge`, the
//! permission manager's definitions and the modern backend's state import.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageapexuninstallbridge as apex;
use aim_service_aidl::dev_aim_server_ipermissionpersistencebridge as permission;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct ApexUninstall {
    host: Arc<AppHost>,
}

impl ApexUninstall {
    pub fn new(host: Arc<AppHost>) -> Self {
        ApexUninstall { host }
    }
}

impl Service for ApexUninstall {
    fn descriptor(&self) -> &str {
        apex::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != apex::UNSTAGE_PACKAGE {
            return Err(UNKNOWN_TRANSACTION);
        }
        let a = apex::UnstagePackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
        reply(if call.sender_euid != SYSTEM_UID {
            Err(Exception::security(
                "native APEX uninstall requires system UID",
            ))
        } else {
            a.package_path
                .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packagePath"))
                .map(|path| {
                    let unstaged = super::apex_manager::uninstall_apex(&self.host.process, &path);
                    let mut p = Parcel::new();
                    apex::write_unstage_package_reply(&mut p, unstaged);
                    p
                })
        })
    }
}

pub struct PermissionPersistence {
    host: Arc<AppHost>,
}

impl PermissionPersistence {
    pub fn new(host: Arc<AppHost>) -> Self {
        PermissionPersistence { host }
    }

    fn owner(
        &self,
        uid: u32,
    ) -> Result<&Arc<super::permission_manager::PermissionManager>, Exception> {
        // `Bridge.enforceSystemUid`.
        if uid != SYSTEM_UID {
            return Err(Exception::security(
                "native package leaf requires system UID",
            ));
        }
        self.host.permission_manager.get().ok_or_else(|| {
            Exception::new(EX_NULL_POINTER, "permission persistence owner unavailable")
        })
    }
}

impl Service for PermissionPersistence {
    fn descriptor(&self) -> &str {
        permission::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            permission::CAPTURE_DEFINITIONS => {
                permission::CaptureDefinitions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.owner(call.sender_euid).and_then(|owner| {
                    owner.capture_definitions().map(|bytes| {
                        let mut p = Parcel::new();
                        permission::write_capture_definitions_reply(&mut p, &Some(bytes));
                        p
                    })
                }))
            }
            permission::IMPORT_NATIVE_MIGRATION => {
                permission::ImportNativeMigration::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.owner(call.sender_euid).map(|_| {
                    // AOSP android-16.0.0_r1 PermissionService.readLegacyPermissionStateTEMP() {}.
                    let mut p = Parcel::new();
                    permission::write_import_native_migration_reply(&mut p);
                    p
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

    fn node(
        euid: u32,
        leaf: impl FnOnce(Arc<AppHost>) -> Arc<dyn Service>,
    ) -> aim_binder_host::local::LocalService {
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
        let aim_binder_host::parcel::Binder::Local(ptr) = process.add_service(leaf(host)) else {
            unreachable!()
        };
        process.local_service(ptr).unwrap()
    }

    /// Only the system uid is served: a null path is refused, and a
    /// permission manager must be present even for the empty migration.
    #[test]
    fn internal_leaves_are_the_systems() {
        let mut unstage = Parcel::new();
        apex::UnstagePackage { package_path: None }.write(&mut unstage);
        let apex_code = |euid: u32| {
            let reply = node(euid, |h| Arc::new(ApexUninstall::new(h)))
                .transact(apex::UNSTAGE_PACKAGE, &unstage, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(apex_code(10000), EX_SECURITY);
        assert_eq!(apex_code(SYSTEM_UID), EX_NULL_POINTER);
        for code in [
            permission::CAPTURE_DEFINITIONS,
            permission::IMPORT_NATIVE_MIGRATION,
        ] {
            let mut request = Parcel::new();
            request.write_interface_token(permission::DESCRIPTOR);
            for (uid, expected) in [(10000, EX_SECURITY), (SYSTEM_UID, EX_NULL_POINTER)] {
                let reply = node(uid, |h| Arc::new(PermissionPersistence::new(h)))
                    .transact(code, &request, false)
                    .unwrap();
                let e = reply.reader().read_exception().unwrap().unwrap_err();
                assert_eq!(e.code, expected);
                if uid == SYSTEM_UID {
                    assert_eq!(e.message, "permission persistence owner unavailable");
                }
            }
            let leaf = node(SYSTEM_UID, |h| Arc::new(PermissionPersistence::new(h)));
            assert!(matches!(
                leaf.transact(code, &Parcel::new(), false),
                Err(BAD_VALUE)
            ));
            assert!(matches!(
                leaf.transact(u32::MAX, &request, false),
                Err(UNKNOWN_TRANSACTION)
            ));
        }
    }

    #[test]
    fn permission_persistence_captures_the_owners_definitions() {
        use super::super::permission_manager::PermissionManager;
        use crate::package::permissions::{AccessSystem, Permission};

        let leaf = node(SYSTEM_UID, |host| {
            let owner = PermissionManager::with_system(
                host.clone(),
                AccessSystem {
                    permissions: vec![Permission {
                        name: "permission".into(),
                        package: "owner".into(),
                        protection_level: 2,
                        dynamic: None,
                    }],
                    ..Default::default()
                },
            );
            assert!(host.permission_manager.set(Arc::new(owner)).is_ok());
            Arc::new(PermissionPersistence::new(host))
        });
        let capture = || {
            let mut request = Parcel::new();
            permission::CaptureDefinitions {}.write(&mut request);
            let reply = leaf
                .transact(permission::CAPTURE_DEFINITIONS, &request, false)
                .unwrap();
            let mut reader = reply.reader();
            let bytes = permission::read_capture_definitions_reply(&mut reader)
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(reader.remaining(), 0);
            bytes
        };
        let bytes = capture();
        let root = aim_android_xml::abx::read(&bytes).unwrap();
        let settings = crate::package::settings::Settings::parse(&root).unwrap();
        assert_eq!(settings.permissions.len(), 1);
        assert_eq!(settings.permissions[0].name, "permission");
        let mut request = Parcel::new();
        permission::ImportNativeMigration {}.write(&mut request);
        let reply = leaf
            .transact(permission::IMPORT_NATIVE_MIGRATION, &request, false)
            .unwrap();
        let mut reader = reply.reader();
        permission::read_import_native_migration_reply(&mut reader)
            .unwrap()
            .unwrap();
        assert_eq!(reader.remaining(), 0);
        assert_eq!(capture(), bytes);
    }
}
