//! `IPackageMutationBridge` for the app host: the native package writer's
//! effects, answered as system_server's `NativePackageEffectsBridge`
//! answers them (`android-16.0.0_r1`), to the system uid only. The
//! packages' protection is `ProtectedPackages`' as the app host keeps it
//! (`data_protected`); the other effects' owners are not ported (#2154).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagemutationbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct PackageEffects {
    host: Arc<AppHost>,
}

impl PackageEffects {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageEffects { host }
    }

    /// `ProtectedPackages.isPackageStateProtected` and
    /// `isPackageDataProtected`, which are the same: no package is
    /// protected without a name.
    fn protected(&self, package: Option<&str>) -> Result<bool, Exception> {
        package.map_or(Ok(false), |package| self.host.data_protected(package))
    }
}

impl Service for PackageEffects {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "native package effects require system UID",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::IS_STATE_PROTECTED => {
                let a = bridge::IsStateProtected::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.protected(a.package_name.as_deref())
                    .map(|v| bridge::write_is_state_protected_reply(&mut p, v))
            }
            bridge::IS_DATA_PROTECTED => {
                let a = bridge::IsDataProtected::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.protected(a.package_name.as_deref())
                    .map(|v| bridge::write_is_data_protected_reply(&mut p, v))
            }
            bridge::PACKAGE_CHANGED => {
                let a = bridge::PackageChanged::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let name = a.package_name.as_deref().ok_or_else(|| {
                    Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName")
                });
                let components = a
                    .components
                    .ok_or_else(|| {
                        Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "components")
                    })
                    .and_then(|values| {
                        values
                            .into_iter()
                            .map(|name| {
                                name.ok_or_else(|| {
                                    Exception::new(
                                        aim_binder_host::parcel::EX_NULL_POINTER,
                                        "component",
                                    )
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()
                    });
                name.and_then(|name| {
                    components.and_then(|components| {
                        self.host
                            .package_writes
                            .get_or_init(|| super::package_writes::Writes::new(self.host.clone()))
                            .changed(
                                name,
                                a.package_uid,
                                a.dont_kill_app,
                                components,
                                a.delayed,
                                a.reason.as_deref(),
                            )
                    })
                })
                .map(|()| bridge::write_package_changed_reply(&mut p))
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("package effects", &format!("{name} (#2154)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::ServiceOwner;

    /// The writer's effects owner takes the bridge; an effect not ported
    /// names its issue, and no package is protected without a name.
    #[test]
    fn the_writer_takes_the_effects_bridge() {
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
            process.add_service(Arc::new(PackageEffects::new(host)))
        else {
            unreachable!()
        };
        crate::package::effects::Owner::new(ServiceOwner::Local(
            process.local_service(ptr).unwrap(),
        ))
        .unwrap();
        let node = process.local_service(ptr).unwrap();
        let mut p = Parcel::new();
        bridge::IsStateProtected {
            package_name: None,
            user_id: 0,
        }
        .write(&mut p);
        let reply = node
            .transact(bridge::IS_STATE_PROTECTED, &p, false)
            .unwrap();
        assert!(
            !bridge::read_is_state_protected_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let mut p = Parcel::new();
        bridge::UserRemoved { user_id: 10 }.write(&mut p);
        let reply = node.transact(bridge::USER_REMOVED, &p, false).unwrap();
        let e = bridge::read_user_removed_reply(&mut reply.reader())
            .unwrap()
            .unwrap_err();
        assert!(e.message.contains("userRemoved (#2154)"), "{}", e.message);
    }
}
