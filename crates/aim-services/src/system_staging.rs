//! Declared as a child of system, retaining staging with its bootstrap lifetime.
use super::*;
impl System {
    pub fn install_package_staging(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        transport: aim_binder_host::local::Strong,
    ) -> Result<Arc<crate::package::staging::Owner>> {
        self.check_package_bootstrap(bridge)?;
        let owner = crate::package::staging::Owner::new(self.process.clone(), transport);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "staging bootstrap changed",
                )
            })?;
        if current.staging.is_some() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "staging owner already installed",
            ));
        }
        current.staging = Some(owner.clone());
        Ok(owner)
    }
    pub fn package_staging_owner(&self) -> Result<Arc<crate::package::staging::Owner>> {
        self.package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .and_then(|current| current.staging.clone())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native staging owner unavailable",
                )
            })
    }
}
impl System {
    pub(crate) fn configure_package_staging(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::staging::Owner>> {
        self.check_package_bootstrap(bridge)?;
        {
            let state = self.package_bootstrap.lock().unwrap();
            if let Some(owner) = state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .and_then(|current| current.staging.clone())
            {
                return Ok(owner);
            }
        }
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as aidl;
        let mut request = Parcel::new();
        aidl::GetNativeStagingBridge {}.write(&mut request);
        let reply = bridge
            .owner
            .transact(aidl::GET_NATIVE_STAGING_BRIDGE, &request, false)
            .map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("staging bridge transport {status}"),
                )
            })?;
        let mut reader = reply.reader();
        let binder =
            aidl::read_get_native_staging_bridge_reply(&mut reader).map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("staging bridge reply {status}"),
                )
            })??;
        if reader.remaining() != 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "staging bridge reply trailing bytes",
            ));
        }
        let strong = reply
            .retain_remote_binder(binder.ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "staging bridge unavailable",
                )
            })?)
            .map_err(|status| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("staging bridge capability {status}"),
                )
            })?;
        self.install_package_staging(bridge, strong)
    }
}
