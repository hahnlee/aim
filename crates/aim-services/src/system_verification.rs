//! Public package verification APIs use the native registry and selected verifier.
use super::System;
use crate::package::{domain_verification::{self as domain,agent::Runtime},intent::ComponentName,query::Query,resolve::Resolver};
use aim_binder_host::parcel::{Exception,EX_ILLEGAL_STATE,EX_NULL_POINTER};
use std::sync::Arc;
fn error(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
impl System {
    pub fn install_domain_verification_agent(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,
        capture:&Arc<crate::package::scan_snapshot::query_state::Capture>)->Result<(),Exception> {
        self.check_package_bootstrap(bridge)?;
        let runtime=Runtime::select(capture.state())?;
        let mut state=self.package_bootstrap.lock().unwrap();
        let current=state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,bridge)
            && current.queries.as_ref().is_some_and(|source|Arc::ptr_eq(source,capture)))
            .ok_or_else(||error("native verifier bootstrap generation changed"))?;
        if current.verification_agent.is_some(){return Err(error("native verifier already selected"));}
        current.verification_agent=Some(runtime);Ok(())
    }
    pub fn package_verification_runtime(&self)->Result<Arc<Runtime>,Exception> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current|current.verification_agent.clone())
            .ok_or_else(||error("native verifier selection unavailable"))
    }
    pub(crate) fn get_native_domain_verification_agent(&self,uid:i32,user:i32)->Result<Option<ComponentName>,Exception> {
        if uid!=0 && uid!=2000 {return Err(Exception::security("Not allowed to query domain verification agent"));}
        let runtime=self.package_verification_runtime()?;
        let agent=runtime.component.as_ref().ok_or_else(||Exception::new(EX_NULL_POINTER,
            "Attempt to invoke virtual method 'java.lang.String android.content.ComponentName.getPackageName()' on a null object reference"))?;
        let capture=self.capture_package_queries()?;let resolver=Resolver::default();
        let resolution=resolver.resolution(capture.state()).map_err(|cause|error(format!("native verifier visibility: {cause:?}")))?;
        let query=Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:uid};
        let Some(package)=capture.state().packages.get(&agent.package) else {return Ok(None);};
        if query.filtered_including_uninstalled(Some(package),user).map_err(|cause|error(cause.0))? {return Ok(None);}
        if crate::package::info::user_state(package,user).disabled_components.contains(&agent.class) {return Ok(None);}
        Ok(Some(agent.clone()))
    }
    pub(crate) fn is_native_domain_verifier_uid(&self,uid:i32)->Result<bool,Exception> {
        let runtime=self.package_verification_runtime()?;
        let Some(agent)=runtime.component.as_ref() else {return Ok(false);};
        let capture=self.capture_package_queries()?;let resolver=Resolver::default();
        let resolution=resolver.resolution(capture.state()).map_err(|cause|error(format!("native verifier UID owner: {cause:?}")))?;
        let query=Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:1000};
        let value=query.package_uid_internal(&agent.package,0,crate::package::apps_filter::user_id(uid),1000)
            .map_err(|cause|error(cause.0))?;
        Ok(value==uid)
    }
    pub(crate) fn get_native_domain_backup(&self,uid:i32,user:i32)->Result<Option<Vec<u8>>,Exception> {
        if uid!=1000 {return Err(Exception::security("Only the system may call getDomainVerificationBackup()"));}
        let runtime=self.package_verification_runtime()?;
        let capture=self.capture_package_queries()?;
        let domains=capture.domains().ok_or_else(||error("native domain registry unavailable"))?;
        match domain::backup::write(domains.owner(),capture.state(),user) {
            Ok(bytes)=>Ok(Some(bytes)),
            Err(cause)=>{runtime.note(format!("Unable to write domain verification for backup: {cause}"));Ok(None)},
        }
    }
    pub(crate) fn restore_native_domain_backup(&self,uid:i32,bytes:Option<&[u8]>,_user:i32)->Result<(),Exception> {
        if uid!=1000 {return Err(Exception::security("Only the system may call restorePreferredActivities()"));}
        let runtime=self.package_verification_runtime()?;
        let Some(bytes)=bytes else {runtime.note("Exception restoring domain verification: null backup");return Ok(());};
        let bridge=self.package_bootstrap()?;
        let strict=bridge.domain_uuid_strict_validation().map_err(|cause|error(format!("native domain UUID policy: {cause:?}")))?;
        // The source parses the entire detached input before modifying its maps.
        let decode=||->Result<domain::ReadResult,String>{
            let mut reader=aim_android_xml::pull::Reader::new(bytes)?;reader.next()?;
            domain::State::read_events(&mut reader,|id|domain::uuid::parse(id,strict).map_err(crate::package::settings::ReadError::File))
                .map_err(|cause|cause.to_string())
        };
        let result=match decode(){Ok(result)=>result,Err(cause)=>{runtime.note(format!("Exception restoring domain verification: {cause}"));return Ok(());}};
        if result.state.active.is_empty()&&result.state.restored.is_empty(){for diagnostic in result.diagnostics {runtime.note(diagnostic.message);}return Ok(());}
        let input=result.state;let diagnostics=result.diagnostics;
        loop {
            self.check_package_bootstrap(&bridge)?;
            let capture=self.capture_package_queries()?;
            let domains=capture.domains().ok_or_else(||error("native domain restore registry unavailable"))?;
            let mut owner=domains.owner().clone();
            let result=domain::ReadResult{state:input.clone(),diagnostics:vec![]};
            owner.restore_backup(result,|name| {
                let Some(package)=capture.state().packages.get(name).and_then(|package|package.pkg.as_deref()) else {return Ok(vec![]);};
                let policy=domains.collector_policy(&package.package_name)?;
                Ok(domain::collector::collect(package,policy,domain::collector::Kind::ValidAutoVerify))
            }).map_err(error)?;
            let update=capture.prepare_domain_update(owner).map_err(error)?;
            let persistence={let state=self.package_bootstrap.lock().unwrap();
                state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&bridge)).and_then(|current|current.persistence.clone())
                    .ok_or_else(||error("native domain restore disk owner unavailable"))?};
            match self.commit_package_domains_if_current(&bridge,update,&mut persistence.lock().unwrap()) {
                Ok(None)=>continue,
                Ok(Some(_))=>{for diagnostic in diagnostics {runtime.note(diagnostic.message);}return Ok(());},
                Err(cause)=>return Err(error(format!("native domain restore persistence (committed={}): {}",cause.committed,cause.message))),
            }
        }
    }
}
