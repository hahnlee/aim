//! PackageManagerInternal install completion and APEX deletion at Android 16 r1.
use super::*;
use aim_binder_host::parcel::EX_ILLEGAL_STATE;
use aim_service_aidl::{WriteParcelable,dev_aim_server_ipackageapexuninstallbridge as apex_api};
use crate::package::installer::internal_installs::{Owner,Worker};
impl System {
    pub fn initialize_package_internal_installs(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,apex:Strong)->Result<Worker> {
        self.check_package_bootstrap(bridge)?;
        let (owner,worker)=Owner::start(apex);
        let mut state=self.package_bootstrap.lock().unwrap();
        let current=state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,bridge))
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"Internal install bootstrap replaced"))?;
        if current.internal_installs.is_some(){return Err(Exception::new(EX_ILLEGAL_STATE,"Internal install owner already initialized"));}
        current.internal_installs=Some(owner);Ok(worker)
    }
    fn internal_install_owner(&self)->Result<Arc<Owner>> {
        self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current|current.internal_installs.clone())
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"Native internal install owner unavailable"))
    }
    pub fn register_internal_enable_rollback(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,token:i32,session:i32)->Result<()> {
        self.check_package_bootstrap(bridge)?;
        let installer={let state=self.package_bootstrap.lock().unwrap();state.current.as_ref()
            .filter(|current|Arc::ptr_eq(&current.bridge,bridge)).and_then(|current|current.installer.as_ref().map(|(owner,_)|owner.clone()))
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"Rollback native installer unavailable"))?};
        let pending=installer.internal_pending_install_session(session)?;
        if !pending.committed||!pending.sealed||pending.destroyed||pending.parent!=-1{return Err(Exception::new(EX_ILLEGAL_STATE,"Rollback install root is not committed and sealed"));}
        self.internal_install_owner()?.register_rollback(token,installer,session)
    }
    pub fn defer_internal_install_observer(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,package:String,
        observer:Binder,status:i32,message:Option<String>,extras:Option<crate::package::installer::internal_installs::Extras>)->Result<()> {
        self.check_package_bootstrap(bridge)?;
        self.internal_install_owner()?.defer_kill_observer(&self.process,package,observer,status,message,extras)
    }
    pub(crate) fn internal_set_enable_rollback_code(&self,token:i32,code:i32,_uid:i32,_pid:i32)->Result<()> {
        self.internal_install_owner()?.rollback_code(token,code)
    }
    pub(crate) fn internal_finish_package_install(&self,token:i32,did_launch:bool,uid:i32,_pid:i32)->Result<()> {
        self.finish_existing_package_install(uid as u32,token,did_launch).map(|_|())
    }
    pub(crate) fn internal_on_package_process_killed_for_uninstall(&self,package:Option<String>,_uid:i32,_pid:i32)->Result<()> {
        // HashMap.remove(null) in the original simply finds no keyed request.
        match package {Some(package)=>self.internal_install_owner()?.killed(package),None=>Ok(())}
    }
    pub(crate) fn internal_get_historical_sessions(&self,user:i32,uid:i32,_pid:i32)->Result<Vec<crate::package::installer::codec::SessionInfo>> {
        let bridge=self.package_bootstrap()?;
        let removal=self.public_package_removal()?;
        let capture=self.capture_package_queries()?;let resolver=crate::package::resolve::Resolver::default();
        let resolution=resolver.resolution(capture.state()).map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("Historical session visibility: {error:?}")))?;
        let query=crate::package::query::Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:uid};
        removal.controller.cross_user(&query,uid as u32,user,"getAllSessions")?;
        let installer={let state=self.package_bootstrap.lock().unwrap();state.current.as_ref()
            .filter(|current|Arc::ptr_eq(&current.bridge,&bridge)).and_then(|current|current.installer.as_ref().map(|(owner,_)|owner.clone()))
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"Historical native installer unavailable"))?};
        installer.historical_session_infos(user,uid as u32)
    }
    pub(crate) fn internal_uninstall_apex(&self,package:Option<String>,version:i64,user:i32,
        sender:Option<crate::package::diagnostics::IntentSender>,flags:i32,uid:i32,_pid:i32)->Result<()> {
        if uid!=0&&uid!=2000{return Err(Exception::security("Not allowed to uninstall apexes"));}
        let name=package.as_deref().unwrap_or("");
        let removal=self.public_package_removal()?;
        let sender=match sender {
            Some(sender)=>{let mut parcel=Parcel::new();sender.write_to(&mut parcel);
                Reader::new(parcel.data(),parcel.objects()).read_binder().map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("APEX status sender: {status}")))?
                    .map(|target|crate::package::installer::preapproval::IntentSender{target})},None=>None,
        };
        let report=|status,message:&str|removal.controller.external.status(sender,name,status,Some(message));
        if flags&crate::package::installer::removal::ALL_USERS==0{return report(-5,"Can't uninstall an apex for a single user");}
        let capture=self.capture_package_queries()?;let resolver=crate::package::resolve::Resolver::default();
        let resolution=resolver.resolution(capture.state()).map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("APEX selection: {error:?}")))?;
        let query=crate::package::query::Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:uid};
        let active=query.package_info(name,-1,0x40000000,0).map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.0))??;
        let Some(active)=active else{return report(-5,&format!("{name} is not an apex package"));};
        let actual=((active.version_code_major as i64)<<32)|(active.version_code as u32 as i64);
        if version!=-1&&version!=actual{return report(-5,&format!("Active version {actual} is not equal to {version}]"));}
        let path=active.application_info.as_ref().and_then(|app|app.source_dir.as_deref())
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"Active APEX source directory unavailable"))?;
        let owner=self.internal_install_owner()?;
        let mut request=Parcel::new();apex_api::UnstagePackage{package_path:Some(path.into())}.write(&mut request);
        let reply=owner.apex.transact(apex_api::UNSTAGE_PACKAGE,&request,false)
            .map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("APEX unstage owner: {status}")))?;
        let mut reader=reply.reader();let uninstalled=apex_api::read_unstage_package_reply(&mut reader)
            .map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("APEX unstage reply: {status}")))??;
        if reader.remaining()!=0{return Err(Exception::new(EX_ILLEGAL_STATE,"APEX unstage trailing data"));}
        if uninstalled{removal.controller.external.status(sender,name,1,None)}else{report(-5,&format!("Failed to uninstall apex {name}"))}
    }
}
