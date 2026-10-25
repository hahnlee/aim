//! The service registry and native publication share one observer epoch owner.
use super::*;
impl System {
    pub(crate) fn package_observer_owner(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::observers::Owner>> {
        self.check_package_bootstrap(bridge)?;
        if let Some(owner) = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.package_observers.clone())
        {
            return Ok(owner);
        }
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        let mut request = Parcel::new();
        api::GetPackageObserverEventsBridge {}.write(&mut request);
        let reply = bridge
            .owner
            .transact(api::GET_PACKAGE_OBSERVER_EVENTS_BRIDGE, &request, false)
            .map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package observer owner transport {status}"),
                )
            })?;
        let mut reader = reply.reader();
        let binder = api::read_get_package_observer_events_bridge_reply(&mut reader)
            .map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("package observer owner reply {status}"),
                )
            })??
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "package observer owner unavailable",
                )
            })?;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "package observer owner reply tail",
            ));
        }
        let target = crate::system::ServiceOwner::retain(&reply, binder).map_err(|status| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("package observer capability {status}"),
            )
        })?;
        self.check_package_bootstrap(bridge)?;
        let owner = crate::package::observers::Owner::new(target);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "observer bootstrap changed",
                )
            })?;
        Ok(current.package_observers.get_or_insert(owner).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::{EX_ILLEGAL_STATE, UNKNOWN_TRANSACTION};
    use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;

    struct MissingOwner;
    impl Service for MissingOwner {
        fn descriptor(&self) -> &str {
            bootstrap::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let mut p = Parcel::new();
            match call.code {
                bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                    bootstrap::IsTestBaseOnBootclasspath::read(&mut call.data)?;
                    bootstrap::write_is_test_base_on_bootclasspath_reply(&mut p, true);
                }
                bootstrap::IS_SIGNING_DEBUGGABLE => {
                    bootstrap::IsSigningDebuggable::read(&mut call.data)?;
                    bootstrap::write_is_signing_debuggable_reply(&mut p, false);
                }
                bootstrap::GET_PACKAGE_OBSERVER_EVENTS_BRIDGE => {
                    bootstrap::GetPackageObserverEventsBridge::read(&mut call.data)?;
                    bootstrap::write_get_package_observer_events_bridge_reply(&mut p, None);
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            Ok(p)
        }
    }

    #[test]
    fn package_observers_missing_bridge_owner_is_an_error() {
        let process = LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: 1000,
                security_context: None,
            },
        );
        let system = System::new(process.clone(), &[]);
        let Binder::Local(ptr) = process.add_service(Arc::new(MissingOwner)) else {
            unreachable!()
        };
        system
            .attach_package_bootstrap_owner(ServiceOwner::Local(
                process.local_service(ptr).unwrap(),
            ))
            .unwrap();
        let bridge = system.package_bootstrap().unwrap();
        for _ in 0..2 {
            let error = system.package_observer_owner(&bridge).err().unwrap();
            assert_eq!(error.code, EX_ILLEGAL_STATE);
            assert_eq!(error.message, "package observer owner unavailable");
        }
    }
}
