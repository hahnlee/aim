//! Actual boot dexopt completion factory over the retained lifecycle and shared disk owner.
use super::*;
use aim_binder_host::parcel::EX_ILLEGAL_STATE;
impl System {
    pub fn package_dexopt_completion(self:&Arc<Self>,bridge:&Arc<crate::package::bootstrap::Bridge>)->Result<Binder>{
        self.check_package_bootstrap(bridge)?;
        let mut state=self.package_bootstrap.lock().unwrap();
        let current=state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||dexopt_error("Dexopt bootstrap replaced"))?;
        if let Some(binder)=current.dexopt_completion{return Ok(binder);}
        // Local registration invokes no Binder callback; publish one capability
        // for the entire epoch. Its timing lives in the shared lifecycle owner.
        let binder=self.process.add_service(Arc::new(crate::package::dexopt_completion::Endpoint{system:Arc::downgrade(self),bridge:bridge.clone()}));
        current.dexopt_completion=Some(binder);Ok(binder)
    }
    pub(crate) fn native_dexopt_hibernation_leaf(&self,bridge:&Arc<crate::package::bootstrap::Bridge>)->Result<Strong>{
        self.check_package_bootstrap(bridge)?;
        let state=self.package_bootstrap.lock().unwrap();let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||dexopt_error("Dexopt hibernation bootstrap replaced"))?;
        let binder=current.boot_lifecycle.as_ref().ok_or_else(||dexopt_error("Dexopt boot lifecycle unavailable"))?.leaf().binder();
        match binder {
            Binder::Handle(handle)=>Ok(self.process.strong(handle)),
            Binder::Local(_)=>Err(dexopt_error("Dexopt hibernation leaf must be a retained original remote owner")),
        }
    }
    pub(crate) fn native_boot_dexopt_start_time(&self,bridge:&Arc<crate::package::bootstrap::Bridge>)->Result<i64>{
        self.check_package_bootstrap(bridge)?;
        self.capture_package_queries()?.state().system.lifecycle.as_ref().ok_or_else(||dexopt_error("Dexopt lifecycle unavailable"))?
            .boot_dexopt_start_nanos().map_err(dexopt_error)
    }
    pub(crate) fn note_native_boot_dexopt_start_time(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,started:i64)->Result<()>{
        self.check_package_bootstrap(bridge)?;
        self.capture_package_queries()?.state().system.lifecycle.as_ref().ok_or_else(||dexopt_error("Dexopt lifecycle unavailable"))?
            .note_boot_dexopt_start_nanos(started).map_err(dexopt_error)
    }
    pub(crate) fn persist_native_dexopt_package_usage(&self,bridge:&Arc<crate::package::bootstrap::Bridge>)->Result<()>{
        let _gate=self.package_install_lock.lock().unwrap();self.check_package_bootstrap(bridge)?;
        let (capture,store)={let state=self.package_bootstrap.lock().unwrap();let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||dexopt_error("Dexopt usage bootstrap replaced"))?;
            (current.queries.clone().ok_or_else(||dexopt_error("Dexopt usage capture unavailable"))?,current.persistence.clone().ok_or_else(||dexopt_error("Dexopt usage disk owner unavailable"))?)};
        let mut store=store.lock().unwrap();store.validate_committed_scan(capture.scan().owner()).map_err(|error|dexopt_error(error.to_string()))?;
        store.write_usage_now(capture.scan().usage()).map_err(|error|dexopt_error(format!("Dexopt package usage committed={}: {}",error.committed,error.message)))
    }
}
fn dexopt_error(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
