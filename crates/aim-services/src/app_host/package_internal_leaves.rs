//! The native package boot's internal persistence and install leaves for
//! the app host, to the system uid only: `IPackageApexUninstallBridge`,
//! an APEX unstaged by apexd as `NativePackageApexUninstallBridge` asks
//! `ApexManager.uninstallApex`; and `IPermissionPersistenceBridge`, the
//! permission registry's definitions and the permission state's import,
//! not implemented (#2165).

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
}

impl Service for PermissionPersistence {
    fn descriptor(&self) -> &str {
        permission::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(permission::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // `Bridge.enforceSystemUid`.
        reply(Err(if call.sender_euid != SYSTEM_UID {
            Exception::security("native package leaf requires system UID")
        } else {
            self.host
                .unimplemented_named("permission persistence", &format!("{name} (#2165)"))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};

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

    /// Only the system uid is served: a null path is refused, and the
    /// permission state names its issue.
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
        let mut capture = Parcel::new();
        permission::CaptureDefinitions {}.write(&mut capture);
        let reply = node(SYSTEM_UID, |h| Arc::new(PermissionPersistence::new(h)))
            .transact(permission::CAPTURE_DEFINITIONS, &capture, false)
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
    }
}
