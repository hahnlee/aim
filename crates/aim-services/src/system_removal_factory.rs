//! The removal coordinator publishes through the same native package generation gate.
use super::System;
use crate::package::{installer::removal,scan_snapshot::Snapshot};
use aim_binder_host::parcel::{Exception,EX_ILLEGAL_STATE};
use std::sync::{Arc,Mutex};
fn error(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
impl System {
    pub fn native_removal_store(self:&Arc<Self>,bridge:&Arc<crate::package::bootstrap::Bridge>,
        disk:Arc<Mutex<crate::package::owner::Store>>)->Result<Arc<removal::NativeStore>,Exception> {
        self.check_package_bootstrap(bridge)?;
        let capture=self.capture_package_queries()?;
        disk.lock().unwrap().validate_committed_scan(capture.scan().owner()).map_err(|cause|error(cause.to_string()))?;
        let snapshots={let state=self.package_bootstrap.lock().unwrap();
            let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||error("Removal bootstrap replaced"))?;
            current.snapshots.as_ref().cloned().ok_or_else(||error("Removal shared native store unavailable"))?};
        if !Arc::ptr_eq(&snapshots.capture(),capture.scan()){return Err(error("Removal shared scan/query generation differs"));}
        let weak=Arc::downgrade(self);let retained=bridge.clone();
        let publish:removal::Publication=Arc::new(move |base,next|{
            let system=weak.upgrade().ok_or_else(||error("Removal native system stopped"))?;
            system.publish_removal_snapshot(&retained,base,next)?;
            Ok(())
        });
        let weak=Arc::downgrade(self);let retained=bridge.clone();
        let invalidate:removal::CacheInvalidation=Arc::new(move||{
            let system=weak.upgrade().ok_or_else(||error("Removal cache native system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            system.reconcile_original_package_domains(&retained)?;
            retained.invalidate_package_info_cache().map_err(|cause|error(format!("Removal committed; cache owner: {cause:?}")))
        });
        Ok(Arc::new(removal::NativeStore{snapshots,disk,publish,invalidate}))
    }
    fn publish_removal_snapshot(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,base:&Arc<Snapshot>,next:&Arc<Snapshot>)
        ->Result<Arc<Snapshot>,Exception> {
        loop {
            self.check_package_bootstrap(bridge)?;
            let capture = self.capture_package_queries()?;
            let shared = self.package_bootstrap.lock().unwrap().current.as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .and_then(|current| current.snapshots.clone())
                .ok_or_else(|| error("Removal canonical owner disappeared"))?;
            let latest = shared.capture();
            if next.version() <= base.version() || latest.version() < next.version() {
                return Err(error("Removal committed generation differs"));
            }
            let mut retained = next.owner().clone();
            if !crate::package::scan_snapshot::install_context::rebase_install_users(
                next, &latest, &mut retained, &std::collections::BTreeSet::new()).map_err(error)? {
                return Err(error(format!("Removal committed identity differs: {}",
                    crate::package::scan_snapshot::install_context::install_identity_delta(next, &latest))));
            }
            let updated = capture.prepare_committed_package_snapshot(latest.clone()).map_err(error)?;
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .ok_or_else(|| error("Removal bootstrap replaced after disk commit"))?;
            if !current.snapshots.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &shared)) {
                return Err(error("Removal canonical store replaced"));
            }
            if !Arc::ptr_eq(&shared.capture(), &latest)
                || !current.queries.as_ref().is_some_and(|source| Arc::ptr_eq(source, &capture)) {
                drop(state);
                continue;
            }
            let published = updated.scan().clone();
            current.queries = Some(updated);
            if let Some(page) = &current.version_page { page.publish(published.version()); }
            state.version = published.version();
            return Ok(published);
        }
    }
    pub(crate) fn restore_installer_factory_package(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,plan:&removal::Plan)
        ->Result<removal::FactoryEffects,Exception> {
        self.check_package_bootstrap(bridge)?;
        let (image,library)={
            let state=self.package_bootstrap.lock().unwrap();
            let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge))
                .ok_or_else(||error("Factory bootstrap replaced"))?;
            (current.install_environment.clone().ok_or_else(||error("Factory native install environment unavailable"))?,
             current.factory_library_settings.clone().ok_or_else(||error("Factory guest library settings unavailable"))?)
        };
        let controller=self.public_package_removal()?.controller.clone();
        if !Arc::ptr_eq(&image.snapshots,&controller.store.snapshots)
            || !Arc::ptr_eq(&controller.gate,&self.package_install_lock) {
            return Err(error("Factory restoration is detached from the native installation coordinator"));
        }
        let factory=removal::Factory{store:controller.store.clone(),resources:image.code_resources.clone(),
            image,bridge:bridge.clone(),external:controller.external.clone(),effects:controller.effects.clone(),
            page_size:library.page_size,compat_16kb_disabled:library.compat_16kb_disabled,
            preferred_removal:controller.preferred_removal.clone()};
        // Controller.execute holds the shared gate; reacquiring it here deadlocks.
        factory.restore(plan)
    }
    pub fn removal_query_source(self:&Arc<Self>,bridge:Arc<crate::package::bootstrap::Bridge>)->crate::package::installer::native::QuerySource {
        let weak=Arc::downgrade(self);
        Arc::new(move ||{
            let system=weak.upgrade().ok_or_else(||error("Removal native system stopped"))?;
            system.check_package_bootstrap(&bridge)?;
            Ok(system.capture_package_queries()?.state().clone())
        })
    }
}
