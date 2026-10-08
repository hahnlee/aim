//! Native verification owner construction; retain workers for the bootstrap lifetime.
use super::System;
use crate::package::{bootstrap::Bridge, domain_verification::agent::Runtime,
    installer::native::NativeOwners, install_verification, legacy_domain_mutation};
use aim_binder_host::parcel::{Exception,EX_ILLEGAL_STATE};
use std::sync::{Arc,Mutex};

pub struct VerificationWorkers {
    pub packages:install_verification::Worker,
    pub legacy:legacy_domain_mutation::Worker,
}
impl System {
    pub fn initialize_native_package_verification(self:&Arc<Self>,bridge:&Arc<Bridge>,
        persistence:Arc<Mutex<crate::package::owner::Store>>)->Result<VerificationWorkers,Exception> {
        self.check_package_bootstrap(bridge)?;
        let capture=self.capture_package_queries()?;
        if capture.domains().is_none(){return Err(failure("Native domain registry must be captured before verification initialization"));}
        persistence.lock().unwrap().validate_committed_scan(capture.scan().owner()).map_err(|error|failure(error.to_string()))?;
        let selected=Runtime::select(capture.state())?;
        let legacy_enabled=selected.legacy_enabled;
        {
            let state=self.package_bootstrap.lock().unwrap();
            let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge))
                .ok_or_else(||failure("Verification bootstrap changed"))?;
            if current.verification_agent.is_some()||current.pending_verification.is_some()||current.legacy_domain_verification.is_some(){
                return Err(failure("Verification owners already initialized"));
            }
            if !current.persistence.as_ref().is_some_and(|store|Arc::ptr_eq(store,&persistence)) {
                return Err(failure("Verification persistence is detached from package bootstrap"));
            }
        }
        // Permission checks are the original permission service's AIDL; user
        // existence, visibility and verifier identities are the native capture.
        let packages=self.install_package_verification(bridge)?;
        let legacy=match self.install_legacy_domain_verification(bridge,persistence,legacy_enabled) {
            Ok(worker)=>worker,
            Err(error)=>{drop(packages);self.clear_package_verification_initialization(bridge);return Err(error);},
        };
        let mut state=self.package_bootstrap.lock().unwrap();
        let current=match state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,bridge)) {
            Some(current)=>current,
            None=>{drop(state);drop(legacy);drop(packages);return Err(failure("Verification bootstrap replaced during initialization"));},
        };
        current.verification_agent=Some(selected);
        Ok(VerificationWorkers{packages,legacy})
    }
    fn clear_package_verification_initialization(&self,bridge:&Arc<Bridge>) {
        let mut state=self.package_bootstrap.lock().unwrap();
        if let Some(current)=state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,bridge)) {
            current.pending_verification=None;
            current.legacy_domain_verification=None;
            current.verification_agent=None;
        }
    }
    /// Register before sending verification broadcasts. The continuation retains
    /// the real native installer and rechecks bootstrap ownership when resumed.
    pub fn register_native_install_verification(self:&Arc<Self>,bridge:&Arc<Bridge>,
        installer:Arc<NativeOwners>,verification_id:i32,session_id:i32,
        required:Vec<i32>,sufficient:Vec<i32>)->Result<(),Exception> {
        self.check_package_bootstrap(bridge)?;
        {
            let state=self.package_bootstrap.lock().unwrap();
            let attached=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge))
                .and_then(|current|current.installer.as_ref());
            if !attached.is_some_and(|(owner,_)|Arc::ptr_eq(owner,&installer)) {
                return Err(failure("Verification continuation belongs to a different native installer"));
            }
        }
        if verification_id<0||required.is_empty(){return Err(failure("Install verification requires an allocated token and required verifiers"));}
        let capture=self.capture_package_queries()?;
        let resolver=crate::package::resolve::Resolver::default();
        let resolution=resolver.resolution(capture.state()).map_err(|error|failure(format!("Verifier visibility: {error:?}")))?;
        let query=crate::package::query::Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:1000};
        for uid in required.iter().chain(sufficient.iter()) {
            if *uid<0||!capture.state().users.contains_key(&crate::package::apps_filter::user_id(*uid))
                || !query.uid_has_permission(*uid,"android.permission.PACKAGE_VERIFICATION_AGENT").map_err(|error|failure(error.0))? {
                return Err(Exception::security("Install verifier lacks PACKAGE_VERIFICATION_AGENT"));
            }
        }
        let weak=Arc::downgrade(self);let retained=bridge.clone();
        self.pending_package_verification()?.register(verification_id,install_verification::Verification::new(required,sufficient),
            Box::new(move |allowed|{
                let system=weak.upgrade().ok_or_else(||failure("Verification native system stopped"))?;
                system.check_package_bootstrap(&retained)?;
                installer.complete_native_package_verification(session_id,allowed)
            }))
    }
}
fn failure(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
