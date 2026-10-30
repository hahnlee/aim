//! Production maintenance construction over the canonical native package owners.
use super::System;
use crate::package::{
    bootstrap::Bridge,
    diagnostics::{self, Inputs},
};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
use std::sync::Arc;
fn illegal(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

impl System {
    /// C constructor calls after installer/removal/lifecycle/instant publication.
    /// The returned worker is retained outside the bootstrap mutex.
    pub fn initialize_package_maintenance(
        self: &Arc<Self>,
        bridge: &Arc<Bridge>,
    ) -> Result<diagnostics::Worker, Exception> {
        self.check_package_bootstrap(bridge)?;
        let effects = self.package_effects_owner(bridge)?;
        let removal = self.public_package_removal()?.controller.clone();
        let (installer, snapshots, persistence) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .ok_or_else(|| illegal("Maintenance bootstrap replaced"))?;
            if current.maintenance.is_some() {
                return Err(illegal("Maintenance runtime already installed"));
            }
            (
                current
                    .installer
                    .as_ref()
                    .map(|(owner, _)| owner.clone())
                    .ok_or_else(|| illegal("Maintenance native installer unavailable"))?,
                current
                    .snapshots
                    .clone()
                    .ok_or_else(|| illegal("Maintenance native store unavailable"))?,
                current
                    .persistence
                    .clone()
                    .ok_or_else(|| illegal("Maintenance disk owner unavailable"))?,
            )
        };
        if !Arc::ptr_eq(&removal.store.snapshots, &snapshots)
            || !Arc::ptr_eq(&removal.store.disk, &persistence)
            || !Arc::ptr_eq(&removal.gate, &self.package_install_lock)
            || !Arc::ptr_eq(&removal.effects, &effects)
        {
            return Err(illegal(
                "Maintenance removal is detached from shared installation owners",
            ));
        }
        let capture = self.capture_package_queries_for(bridge)?;
        if !Arc::ptr_eq(capture.scan(), &snapshots.capture()) {
            return Err(illegal("Maintenance scan/query generations differ"));
        }
        if capture.state().system.lifecycle.is_none()
            || capture.state().system.instant_registry.is_none()
        {
            return Err(illegal(
                "Maintenance lifecycle/instant native owners unavailable",
            ));
        }
        persistence
            .lock()
            .unwrap()
            .validate_committed_scan(capture.scan().owner())
            .map_err(|error| illegal(format!("Maintenance persisted scan owner: {error}")))?;
        let maintenance = bridge.package_maintenance().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(error) => error,
            other => illegal(format!("Maintenance independent original owner: {other:?}")),
        })?;
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let current = Arc::new(move || {
            let system = weak
                .upgrade()
                .ok_or_else(|| illegal("Maintenance native system stopped"))?;
            system.capture_package_queries_for(&retained)
        });
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let usage: diagnostics::UsagePublication = Arc::new(move |caller, package, reason, now| {
            let system = weak
                .upgrade()
                .ok_or_else(|| illegal("Usage native system stopped"))?;
            system.publish_maintenance_usage(&retained, caller.calling_uid, package, reason, now)
        });
        let (runtime, worker) = diagnostics::Runtime::attach(Inputs {
            process: self.process.clone(),
            maintenance: Arc::new(maintenance),
            effects,
            install_serial: self.package_install_lock.clone(),
            current,
            removal,
            installer,
            usage,
        })?;
        let installed = {
            let mut state = self.package_bootstrap.lock().unwrap();
            match state
                .current
                .as_mut()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            {
                Some(current)
                    if current.maintenance.is_none()
                        && current
                            .snapshots
                            .as_ref()
                            .is_some_and(|owner| Arc::ptr_eq(owner, &snapshots))
                        && current
                            .persistence
                            .as_ref()
                            .is_some_and(|owner| Arc::ptr_eq(owner, &persistence)) =>
                {
                    current.maintenance = Some(runtime.clone());
                    true
                }
                _ => false,
            }
        };
        if !installed {
            runtime.stop();
            drop(worker);
            return Err(illegal("Maintenance owners changed during construction"));
        }
        Ok(worker)
    }

    fn publish_maintenance_usage(
        &self,
        bridge: &Arc<Bridge>,
        uid: i32,
        package: Option<&str>,
        reason: i32,
        now: i64,
    ) -> Result<(), Exception> {
        self.publish_package_usage_with(Some((bridge, uid)), package, reason, now, || {})
    }
}
