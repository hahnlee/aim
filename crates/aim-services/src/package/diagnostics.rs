//! PackageManager maintenance contracts at android-16.0.0_r1.
//! Adapted from AOSP IPackageManagerBase and PackageManagerService (Apache-2.0).
use aim_binder_host::{
    local::{Call, LocalProcess, LocalService, Reply, Strong},
    parcel::{Binder, Exception, Parcel, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, BAD_VALUE, DEAD_OBJECT},
};
use aim_service_aidl::{android_content_pm_ipackagemanager as pm,
    android_content_pm_idexmoduleregistercallback as dex_callback};
use std::sync::{Arc, Mutex, mpsc};
use super::{apps_filter, query::Query};
use aim_service_aidl::dev_aim_server_ipackagemaintenancebridge as bridge;
use std::thread::{self, JoinHandle};

enum Target { Local(Arc<LocalService>), Remote(Strong) }
impl Target {
    fn retain(process: &Arc<LocalProcess>, binder: Binder) -> Result<Self, i32> {
        match binder {
            Binder::Local(pointer) => process.local_service(pointer).map(Arc::new).map(Self::Local).ok_or(DEAD_OBJECT),
            Binder::Handle(handle) => Ok(Self::Remote(process.strong(handle))),
        }
    }
    fn send(&self, code: u32, request: &Parcel) -> Result<(), i32> {
        match self {
            Self::Local(target) => target.transact(code, request, true).map(|_|()),
            Self::Remote(target) => target.transact(code, request, true).map(|_|()),
        }
    }
}
struct Callback { target: Target, path: Option<String> }
struct Cache { package:Option<String>, user:i32, caller:i32, instant:bool, observer:Option<Target> }
#[derive(Clone,Copy)]
pub struct IntentSender { target:Option<Binder> }
impl aim_service_aidl::ReadParcelable for IntentSender {fn read_from(reader:&mut aim_binder_host::parcel::Reader<'_>)->aim_binder_host::parcel::Result<Self> {Ok(Self {target:reader.read_binder()?})}}
impl aim_service_aidl::WriteParcelable for IntentSender {fn write_to(&self,parcel:&mut Parcel) {parcel.write_binder(self.target);}}
struct Free { volume:Option<String>, bytes:i64,flags:i32,observer:Option<Target>, sender:Option<(IntentSender,Target)> }
enum Job { Callback(Callback), Cache(Cache), Free(Free), PruneLibraries, Stop }
/// Root binds this to the serialized native usage publication owner.
pub type UsagePublication=Arc<dyn Fn(&Query<'_>,Option<&str>,i32,i64)->Result<(),Exception>+Send+Sync>;
/// Concrete bootstrap inputs; all mutable package actions use the same native owners.
pub struct Inputs {
    pub process: Arc<LocalProcess>,
    pub maintenance: Arc<Strong>,
    pub effects: Arc<super::effects::Owner>,
    pub install_serial: Arc<Mutex<()>>,
    pub current: super::diagnostics_storage::Current,
    pub removal: Arc<super::installer::removal::Controller>,
    pub installer: Arc<super::installer::native::NativeOwners>,
    pub usage: UsagePublication,
}
/// Native Handler FIFO. Binder capabilities are retained before request buffers expire.
pub struct Runtime {
    process: Arc<LocalProcess>,
    queue: Arc<Mutex<Option<mpsc::Sender<Job>>>>,
    errors: Arc<Mutex<Vec<String>>>,
    bridge: Arc<Strong>,
    art: Mutex<Option<(Binder,Target)>>,
    effects: Arc<super::effects::Owner>,
    install_serial: Arc<Mutex<()>>,
    usage: UsagePublication,
}
pub struct Worker { queue: Arc<Mutex<Option<mpsc::Sender<Job>>>>, thread: Option<JoinHandle<()>> }
impl Worker {
    pub fn is_finished(&self)->bool {self.thread.as_ref().is_none_or(JoinHandle::is_finished)}
    /// Drop this guard outside native publication/installation locks: callbacks may reenter.
    pub fn shutdown(self) {drop(self);}
}
impl Drop for Worker {
    fn drop(&mut self) { if let Some(queue)=self.queue.lock().unwrap().take() {let _=queue.send(Job::Stop);} if let Some(thread)=self.thread.take() { let _=thread.join(); } }
}
impl Runtime {
    pub fn schedule_unused_library_prune(&self)->Result<(),Exception>{
        self.queue.lock().unwrap().as_ref().ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"Package maintenance stopped"))?
            .send(Job::PruneLibraries).map_err(|_|Exception::new(EX_ILLEGAL_STATE,"Package maintenance queue stopped"))
    }
    pub fn stop(&self) {
        if let Some(queue) = self.queue.lock().unwrap().take() {
            let _ = queue.send(Job::Stop);
        }
    }
    pub fn attach(inputs: Inputs) -> Result<(Arc<Self>, Worker), Exception> {
        let storage=super::diagnostics_storage::Owner::new(inputs.maintenance.clone(),inputs.current,inputs.removal,inputs.installer);
        Self::new(inputs.process,inputs.maintenance,inputs.effects,inputs.install_serial,storage,inputs.usage)
    }
    /// Called after the caller's query capture is obtained, outside publication locks.
    pub fn answer(&self,call:&mut Call<'_>,query:&Query<'_>)->Option<Reply> {
        if !Self::handles(call.code) {return None;}
        if let Some(reply)=self.dispatch(call) {return Some(reply);}
        self.dispatch_art(call,query)
    }
    pub fn handles(code:u32)->bool {
        matches!(code,pm::GET_PACKAGE_SIZE_INFO|pm::REGISTER_DEX_MODULE|pm::NOTIFY_PACKAGE_USE
            |pm::GET_ART_MANAGER|pm::PERFORM_DEX_OPT_MODE|pm::PERFORM_DEX_OPT_SECONDARY
            |pm::NOTIFY_DEX_LOAD|pm::CLEAR_APPLICATION_PROFILE_DATA|pm::FREE_STORAGE
            |pm::FREE_STORAGE_AND_NOTIFY|pm::DELETE_APPLICATION_CACHE_FILES|pm::DELETE_APPLICATION_CACHE_FILES_AS_USER)
    }
    pub fn new(process: Arc<LocalProcess>, bridge: Arc<Strong>, effects: Arc<super::effects::Owner>, install_serial: Arc<Mutex<()>>, storage:Arc<super::diagnostics_storage::Owner>, usage:UsagePublication) -> Result<(Arc<Self>, Worker), Exception> {
        let (tx,rx)=mpsc::channel();
        let errors=Arc::new(Mutex::new(Vec::new()));
        let callback_errors=errors.clone();
        let storage_bridge=bridge.clone();let storage_serial=install_serial.clone();let free_storage=storage.clone();
        let worker=thread::Builder::new().name("package-maintenance-callbacks".into()).spawn(move|| {
            while let Ok(job)=rx.recv() {
                match job {
                    Job::Stop => break,
                    Job::PruneLibraries => {if let Err(error)=free_storage.prune_unused_static_libraries(){callback_errors.lock().unwrap().push(format!("Static library maintenance: {error:?}"));}},
                    Job::Free(free) => {
                        let success=match free_storage.free(free.volume.as_deref(),free.bytes,free.flags) {
                            Ok(())=>true,
                            Err(super::diagnostics_storage::Error::Io(message))=>{callback_errors.lock().unwrap().push(message);false},
                            Err(super::diagnostics_storage::Error::Owner(error))=>{callback_errors.lock().unwrap().push(format!("free-storage owner: {}",error.message));continue;},
                        };
                        if let Some(observer)=free.observer {
                            let mut request=Parcel::new();aim_service_aidl::android_content_pm_ipackagedataobserver::OnRemoveCompleted {package_name:None,succeeded:success}.write(&mut request);
                            if let Err(status)=observer.send(aim_service_aidl::android_content_pm_ipackagedataobserver::ON_REMOVE_COMPLETED,&request) {callback_errors.lock().unwrap().push(format!("free-storage observer no longer available: {status}"));}
                        }
                        if let Some((sender,_capability))=free.sender {
                            let mut request=Parcel::new();bridge::SendFreeStorageResult {sender:Some(sender),success}.write(&mut request);
                            match storage_bridge.transact(bridge::SEND_FREE_STORAGE_RESULT,&request,false) {
                                Err(status)=>callback_errors.lock().unwrap().push(format!("free-storage IntentSender transport: {status}")),
                                Ok(reply)=>if let Err(error)=bridge::read_send_free_storage_result_reply(&mut reply.reader()).and_then(|result|result.map_err(|_|BAD_VALUE)) {callback_errors.lock().unwrap().push(format!("free-storage IntentSender reply: {error}"));}
                            }
                        }
                    }
                    Job::Cache(cache) => {
                        let _install=storage_serial.lock().unwrap();
                        let mut request=Parcel::new();
                        bridge::ClearCacheFiles {package_name:cache.package.clone(),user_id:cache.user,can_access_instant_apps:cache.instant,calling_uid:cache.caller}.write(&mut request);
                        let cleared=storage_bridge.transact(bridge::CLEAR_CACHE_FILES,&request,false)
                            .map_err(|status|format!("cache owner transport: {status}"))
                            .and_then(|reply|bridge::read_clear_cache_files_reply(&mut reply.reader()).map_err(|status|format!("cache owner reply: {status}")).and_then(|result|result.map_err(|error|error.message)));
                        drop(_install);
                        match cleared {
                            Err(error)=>callback_errors.lock().unwrap().push(error),
                            Ok(())=>if let Some(observer)=cache.observer {
                                let mut request=Parcel::new();
                                aim_service_aidl::android_content_pm_ipackagedataobserver::OnRemoveCompleted {package_name:cache.package,succeeded:true}.write(&mut request);
                                if let Err(status)=observer.send(aim_service_aidl::android_content_pm_ipackagedataobserver::ON_REMOVE_COMPLETED,&request) {callback_errors.lock().unwrap().push(format!("cache observer no longer available: {status}"));}
                            }
                        }
                    }
                    Job::Callback(callback) => {
                        let mut request=Parcel::new();
                        dex_callback::OnDexModuleRegistered {dex_module_path:callback.path,success:false,
                            message:Some("registerDexModule call not supported since Android U".into())}.write(&mut request);
                        if let Err(status)=callback.target.send(dex_callback::ON_DEX_MODULE_REGISTERED,&request) {
                            callback_errors.lock().unwrap().push(format!("dex module observer no longer available: {status}"));
                        }
                    }
                }
            }
        }).map_err(|error|Exception::new(EX_ILLEGAL_STATE,error.to_string()))?;
        let queue=Arc::new(Mutex::new(Some(tx)));
        Ok((Arc::new(Self {process,queue:queue.clone(),errors,bridge,art:Mutex::new(None),effects,install_serial,usage}),Worker {queue,thread:Some(worker)}))
    }
    pub fn take_errors(&self)->Vec<String> {std::mem::take(&mut *self.errors.lock().unwrap())}
    fn bridge_request(&self, request: &Parcel, code: u32) -> Result<aim_binder_host::local::Received,Exception> {
        self.bridge.transact(code,request,false).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("original ART owner transport: {status}")))
    }
    pub fn dispatch_art(&self,call:&mut Call<'_>,query:&Query<'_>)->Option<Reply> {
        let result:Result<Parcel,Exception>=(|| {
            let unsupported=|error:apps_filter::NotModelled|Exception::new(EX_UNSUPPORTED_OPERATION,error.0);
            let uid=query.calling_uid;
            let pid=call.sender_pid;
            let mut request=Parcel::new();
            match call.code {
                pm::NOTIFY_PACKAGE_USE => {
                    let args=pm::NotifyPackageUse::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("package usage request: {status}")))?;
                    bridge::WallTimeMillis {}.write(&mut request);
                    let reply=self.bridge_request(&request,bridge::WALL_TIME_MILLIS)?;
                    let now=bridge::read_wall_time_millis_reply(&mut reply.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("package usage clock reply: {status}")))??;
                    (self.usage)(query,args.package_name.as_deref(),args.reason,now)?;
                    Ok(Parcel::new())
                }
                pm::GET_ART_MANAGER => {
                    pm::GetArtManager::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("ART manager request: {status}")))?;
                    let mut art=self.art.lock().unwrap();
                    if art.is_none() {
                        bridge::GetArtManagerBinder {}.write(&mut request);
                        let received=self.bridge_request(&request,bridge::GET_ART_MANAGER_BINDER)?;
                        let binder=bridge::read_get_art_manager_binder_reply(&mut received.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("ART manager reply: {status}")))??;
                        let binder=binder.ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"original ART Binder owner unavailable"))?;
                        let target=Target::retain(&self.process,binder).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("ART manager capability: {status}")))?;
                        *art=Some((binder,target));
                    }
                    let mut reply=Parcel::new();pm::write_get_art_manager_reply(&mut reply,art.as_ref().map(|(binder,_)|*binder));Ok(reply)
                }
                pm::PERFORM_DEX_OPT_MODE | pm::PERFORM_DEX_OPT_SECONDARY => {
                    let (package,filter,force,complete,split,secondary)=if call.code==pm::PERFORM_DEX_OPT_MODE {
                        let args=pm::PerformDexOptMode::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("dexopt request: {status}")))?;
                        (args.package_name,args.target_compiler_filter,args.force,args.boot_complete,args.split_name,false)
                    } else {
                        let args=pm::PerformDexOptSecondary::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("dexopt request: {status}")))?;
                        (args.package_name,args.target_compiler_filter,args.force,true,None,true)
                    };
                    if !secondary && !matches!(uid,0|1000|2000) {
                        let internal=query.resolve_internal_package_name(package.as_deref().unwrap_or_default(),-1);
                        let owner=query.state.packages.get(&internal).and_then(|state|state.install_source.installer.as_deref())
                            .map(|installer|query.resolve_internal_package_name(installer,-1));
                        let allowed=owner.as_ref().and_then(|name|query.state.packages.get(name)).and_then(|state|state.pkg.as_ref()).is_some_and(|pkg|pkg.uid==uid);
                        if !allowed {return Err(Exception::security("performDexOptMode"));}
                    }
                    let user=apps_filter::user_id(uid);
                    let target=package.as_ref().and_then(|name|query.state.packages.get(name));
                    let caller_instant=apps_filter::instant_app_package_name(query.state,uid).map_err(unsupported)?.is_some();
                    let target_instant=target.and_then(|state|state.users.get(&user)).is_some_and(|state|state.instant_app);
                    let result=if caller_instant || target_instant {false}
                    else if target.and_then(|state|state.pkg.as_ref()).is_some_and(|pkg|pkg.is2(super::pkg::booleans2::APEX)) {true}
                    else {
                        bridge::DexoptPackage {package_name:package,compiler_filter:filter,force,boot_complete:complete,split_name:split,secondary_only:secondary,calling_uid:uid,calling_pid:pid}.write(&mut request);
                        let reply=self.bridge_request(&request,bridge::DEXOPT_PACKAGE)?;
                        bridge::read_dexopt_package_reply(&mut reply.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("ART dexopt reply: {status}")))??
                    };
                    let mut reply=Parcel::new();if secondary {pm::write_perform_dex_opt_secondary_reply(&mut reply,result);} else {pm::write_perform_dex_opt_mode_reply(&mut reply,result);}Ok(reply)
                }
                pm::NOTIFY_DEX_LOAD => {
                    let args=pm::NotifyDexLoad::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("dex usage request: {status}")))?;
                    let resolved_uid=query.state.system.isolated_owners.iter().find(|(isolated,_)|*isolated==uid).map_or(uid,|(_,owner)|*owner);
                    if !matches!(uid,0|1000) && !apps_filter::is_caller_same_app(query.state,args.loading_package_name.as_deref(),resolved_uid).map_err(unsupported)? {return Ok(Parcel::new());}
                    let (paths,contexts)=match args.class_loader_context_map {Some(map)=>{let (paths,contexts):(Vec<_>,Vec<_>)=map.into_iter().unzip();(Some(paths),Some(contexts))},None=>(None,None)};
                    bridge::NotifyDexLoad {loading_package_name:args.loading_package_name,dex_paths:paths,class_loader_contexts:contexts,loader_isa:args.loader_isa,calling_uid:uid,calling_pid:pid}.write(&mut request);
                    let reply=self.bridge_request(&request,bridge::NOTIFY_DEX_LOAD)?;
                    bridge::read_notify_dex_load_reply(&mut reply.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("dex usage reply: {status}")))??;
                    Ok(Parcel::new())
                }
                pm::FREE_STORAGE | pm::FREE_STORAGE_AND_NOTIFY => {
                    let (volume,bytes,flags,observer,sender)=if call.code==pm::FREE_STORAGE {
                        let args=pm::FreeStorage::<IntentSender>::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage request: {status}")))?;
                        (args.volume_uuid,args.free_storage_size,args.storage_flags,None,args.pi)
                    } else {
                        let args=pm::FreeStorageAndNotify::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage request: {status}")))?;
                        (args.volume_uuid,args.free_storage_size,args.storage_flags,args.observer,None)
                    };
                    bridge::EnforcePermission {permission:Some("android.permission.CLEAR_APP_CACHE".into()),calling_uid:uid,calling_pid:pid}.write(&mut request);
                    let reply=self.bridge_request(&request,bridge::ENFORCE_PERMISSION)?;
                    bridge::read_enforce_permission_reply(&mut reply.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage enforcement reply: {status}")))??;
                    let observer=observer.map(|observer|Target::retain(&self.process,observer)).transpose().map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage observer: {status}")))?;
                    let sender=sender.and_then(|sender|sender.target.map(|target|(sender,target))).map(|(sender,target)|Target::retain(&self.process,target).map(|target|(sender,target))).transpose().map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage IntentSender capability: {status}")))?;
                    let queue=self.queue.lock().unwrap();
                    if queue.as_ref().is_none_or(|queue|queue.send(Job::Free(Free {volume,bytes,flags,observer,sender})).is_err()) {return Err(Exception::new(EX_ILLEGAL_STATE,"package maintenance Handler stopped"));}
                    let mut reply=Parcel::new();reply.write_no_exception();Ok(reply)
                }
                pm::DELETE_APPLICATION_CACHE_FILES | pm::DELETE_APPLICATION_CACHE_FILES_AS_USER => {
                    let (package,user,observer)=if call.code==pm::DELETE_APPLICATION_CACHE_FILES {
                        let args=pm::DeleteApplicationCacheFiles::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("cache clearing request: {status}")))?;
                        (args.package_name,apps_filter::user_id(uid),args.observer)
                    } else {
                        let args=pm::DeleteApplicationCacheFilesAsUser::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("cache clearing request: {status}")))?;
                        (args.package_name,args.user_id,args.observer)
                    };
                    bridge::CheckPermission {permission:Some("android.permission.INTERNAL_DELETE_CACHE_FILES".into()),calling_uid:uid,calling_pid:pid}.write(&mut request);
                    let received=self.bridge_request(&request,bridge::CHECK_PERMISSION)?;
                    let internal=bridge::read_check_permission_reply(&mut received.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("cache permission reply: {status}")))??;
                    if !internal {
                        request=Parcel::new();bridge::CheckPermission {permission:Some("android.permission.DELETE_CACHE_FILES".into()),calling_uid:uid,calling_pid:pid}.write(&mut request);
                        let received=self.bridge_request(&request,bridge::CHECK_PERMISSION)?;
                        let legacy=bridge::read_check_permission_reply(&mut received.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("legacy cache permission reply: {status}")))??;
                        if legacy {let mut reply=Parcel::new();reply.write_no_exception();return Ok(reply);}
                        request=Parcel::new();bridge::EnforcePermission {permission:Some("android.permission.INTERNAL_DELETE_CACHE_FILES".into()),calling_uid:uid,calling_pid:pid}.write(&mut request);
                        let received=self.bridge_request(&request,bridge::ENFORCE_PERMISSION)?;
                        bridge::read_enforce_permission_reply(&mut received.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("cache enforcement reply: {status}")))??;
                    }
                    query.enforce_cross_user(user,true,false,"delete application cache files").map_err(unsupported)??;
                    request=Parcel::new();bridge::CheckPermission {permission:Some("android.permission.ACCESS_INSTANT_APPS".into()),calling_uid:uid,calling_pid:pid}.write(&mut request);
                    let received=self.bridge_request(&request,bridge::CHECK_PERMISSION)?;
                    let instant=bridge::read_check_permission_reply(&mut received.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("instant cache permission reply: {status}")))??;
                    request=Parcel::new();bridge::NoteCacheClearCaller {package_name:package.clone(),calling_uid:uid,calling_pid:pid}.write(&mut request);
                    let received=self.bridge_request(&request,bridge::NOTE_CACHE_CLEAR_CALLER)?;
                    bridge::read_note_cache_clear_caller_reply(&mut received.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("cache caller event reply: {status}")))??;
                    let observer=observer.map(|observer|Target::retain(&self.process,observer)).transpose().map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("cache observer: {status}")))?;
                    let queue=self.queue.lock().unwrap();
                    if queue.as_ref().is_none_or(|queue|queue.send(Job::Cache(Cache {package,user,caller:uid,instant,observer})).is_err()) {return Err(Exception::new(EX_ILLEGAL_STATE,"package maintenance Handler stopped"));}
                    let mut reply=Parcel::new();reply.write_no_exception();Ok(reply)
                }
                pm::CLEAR_APPLICATION_PROFILE_DATA => {
                    let args=pm::ClearApplicationProfileData::read(&mut call.data).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("profile clearing request: {status}")))?;
                    if !matches!(uid,0|1000|2000) {return Err(Exception::security("Only the system or shell can clear all profile data"));}
                    let lifecycle=query.state.system.lifecycle.as_ref().ok_or_else(||Exception::new(EX_UNSUPPORTED_OPERATION,"native PackageFreezer owner unavailable"))?;
                    let freeze=args.package_name.as_ref().map(|name|lifecycle.freeze(name.clone()))
                        .transpose().map_err(|message|Exception::new(EX_ILLEGAL_STATE,message))?;
                    if let Some(state)=args.package_name.as_ref().and_then(|name|query.state.packages.get(name)) {
                        self.effects.kill(&state.name,state.app_id,-1,"clearApplicationProfileData",13)?;
                    }
                    let _install=self.install_serial.lock().unwrap();
                    // A missing parsed package follows original clearAppProfilesLIF's diagnostic return.
                    if args.package_name.as_ref().and_then(|name|query.state.packages.get(name)).and_then(|state|state.pkg.as_ref()).is_some() {
                        bridge::ClearAppProfiles {package_name:args.package_name}.write(&mut request);
                        let reply=self.bridge_request(&request,bridge::CLEAR_APP_PROFILES)?;
                        bridge::read_clear_app_profiles_reply(&mut reply.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("profile clearing reply: {status}")))??;
                    }
                    drop(_install);
                    if let Some(freeze)=freeze {
                        freeze.close().map_err(|message|Exception::new(EX_ILLEGAL_STATE,message))?;
                    }
                    let mut reply=Parcel::new();pm::write_clear_application_profile_data_reply(&mut reply);Ok(reply)
                }
                _=>return Err(Exception::new(EX_UNSUPPORTED_OPERATION,"not an ART maintenance method")),
            }
        })();
        if !matches!(call.code,pm::NOTIFY_PACKAGE_USE|pm::GET_ART_MANAGER|pm::PERFORM_DEX_OPT_MODE|pm::PERFORM_DEX_OPT_SECONDARY|pm::NOTIFY_DEX_LOAD|pm::CLEAR_APPLICATION_PROFILE_DATA|pm::FREE_STORAGE|pm::FREE_STORAGE_AND_NOTIFY|pm::DELETE_APPLICATION_CACHE_FILES|pm::DELETE_APPLICATION_CACHE_FILES_AS_USER) {return None;}
        Some(Ok(match result {Ok(reply)=>reply,Err(error)=>{let mut reply=Parcel::new();reply.write_exception(&error);reply}}))
    }
    pub fn dispatch(&self,call:&mut Call<'_>)->Option<Reply> {
        Some(match call.code {
            pm::GET_PACKAGE_SIZE_INFO => {
                if pm::GetPackageSizeInfo::read(&mut call.data).is_err() {return Some(Err(BAD_VALUE));}
                let mut reply=Parcel::new();
                reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION,
                    "Shame on you for calling the hidden API getPackageSizeInfo(). Shame!"));
                Ok(reply)
            }
            pm::REGISTER_DEX_MODULE => {
                let args=match pm::RegisterDexModule::read(&mut call.data) {Ok(args)=>args,Err(status)=>return Some(Err(status))};
                if let Some(binder)=args.callback {
                    let target=match Target::retain(&self.process,binder) {Ok(target)=>target,Err(status)=>return Some(Err(status))};
                    let queue=self.queue.lock().unwrap();
                    if queue.as_ref().is_none_or(|queue|queue.send(Job::Callback(Callback {target,path:args.dex_module_path})).is_err()) {
                        let mut reply=Parcel::new();reply.write_exception(&Exception::new(EX_ILLEGAL_STATE,"package maintenance Handler stopped"));return Some(Ok(reply));
                    }
                }
                Ok(Parcel::new()) // pinned AIDL is oneway
            }
            _=>return None,
        })
    }
}

/// notifyPackageUseInternal writes the real PackageUsage owner; the caller publishes
/// the prepared usage with its exact current native package generation.
pub fn notify_package_use(query: &Query<'_>, usage: &mut super::owner::usage::Usage,
    package: Option<&str>, reason: i32, now_millis: i64) -> Result<(),Exception> {
    let uid=query.calling_uid;
    let caller_instant=apps_filter::instant_app_package_name(query.state,uid)
        .map_err(|error|Exception::new(EX_UNSUPPORTED_OPERATION,error.0))?.is_some();
    let notify=if caller_instant {
        apps_filter::is_caller_same_app(query.state,package,uid)
            .map_err(|error|Exception::new(EX_UNSUPPORTED_OPERATION,error.0))?
    } else {
        !package.and_then(|name|query.state.packages.get(name))
            .and_then(|state|state.users.get(&apps_filter::user_id(uid))).is_some_and(|state|state.instant_app)
    };
    if notify {if let Some(package)=package {usage.notify(package,reason,now_millis);}}
    Ok(())
}
