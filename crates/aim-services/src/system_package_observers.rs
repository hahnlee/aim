//! Same concrete Java ObserverOwner is handed to LocalServices and native publication.
use super::*;
impl System {
    pub(crate) fn package_observer_owner(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<crate::package::observers::Owner>> {
        self.check_package_bootstrap(bridge)?;
        if let Some(owner) = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge)).and_then(|current| current.package_observers.clone()) {
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
        let target = reply.retain_remote_binder(binder).map_err(|status| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("package observer capability {status}"),
            )
        })?;
        self.check_package_bootstrap(bridge)?;
        let owner = crate::package::observers::Owner::new(target);
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "observer bootstrap changed"))?;
        Ok(current.package_observers.get_or_insert(owner).clone())
    }
}
