//! Native BootSession lifecycle. Stage numbers match NativePackageBootstrap.Lifecycle.
use super::*;
use aim_binder_host::{local::{Call,Reply,Service},parcel::{BAD_VALUE,EX_ILLEGAL_STATE,UNKNOWN_TRANSACTION}};
use aim_service_aidl::{dev_aim_server_ipackagebootlifecycleleaf as leaf,dev_aim_server_ipackagebootlifecycleevents as events};
use std::{collections::BTreeSet,sync::{mpsc,Condvar},thread::{self,JoinHandle}};
#[derive(Clone,Debug)]
pub struct AndroidMetrics {pub width_pixels:i32,pub height_pixels:i32,pub density_dpi:i32,pub density:f32,pub scaled_density:f32,pub xdpi:f32,pub ydpi:f32}
enum Job {BootDeferred(super::internal_storage::BootAppData),Prepare,Barrier(mpsc::Sender<Result<()>>),Stop}
pub struct Runtime {
    system:Weak<System>,bridge:Arc<crate::package::bootstrap::Bridge>,leaf:Strong,
    events:Mutex<Option<Binder>>,queue:mpsc::Sender<Job>,startup:Arc<(Mutex<Option<Result<()>>>,Condvar)>,
    stages:Mutex<BTreeSet<i32>>,operation:Mutex<()>,metrics:Mutex<Option<AndroidMetrics>>,
    decompression:Arc<crate::package::scan::boot_compressed::BootDecompression>,boot_apex_changed:bool,closed:std::sync::atomic::AtomicBool,
}
pub struct Worker {runtime:Arc<Runtime>,thread:Option<JoinHandle<()>>,events:LocalService}
impl Runtime { pub(crate) fn leaf(&self) -> &Strong { &self.leaf }
 pub fn stop(&self) { self.closed.store(true,std::sync::atomic::Ordering::Release); let _ = self.queue.send(Job::Stop); } }
impl System {
    fn enter_native_package_settings_ready(&self, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let owner = self.capture_package_queries()?.state().platform.settings_owner.clone()
            .ok_or_else(|| fail("native boot settings owner unavailable"))?;
        let compatibility = owner.system_ready()?;
        let gate = self.package_install_lock.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| fail("settings-ready bootstrap replaced"))?;
        let capture = current.queries.as_ref().ok_or_else(|| fail("settings-ready capture absent"))?;
        if capture.state().system.compatibility_mode == compatibility { return Ok(()); }
        let update = capture.prepare_compatibility_mode(compatibility).map_err(fail)?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        drop(state);
        drop(gate);
        bridge.invalidate_packages_for_uid_cache().map_err(|error| match error {
            crate::package::bootstrap::OwnerError::Owner(error) => error,
            error => fail(format!("settings-ready cache publication: {error:?}")),
        })
    }
    pub fn initialize_package_boot_lifecycle(self:&Arc<Self>,bridge:&Arc<crate::package::bootstrap::Bridge>,
        leaf:Strong,decompression:Arc<crate::package::scan::boot_compressed::BootDecompression>,boot_apex_changed:bool)->Result<Worker> {
        self.check_package_bootstrap(bridge)?;
        let capture=self.capture_package_queries()?;
        if capture.state().system.lifecycle.is_none(){return Err(fail("Boot package lifecycle owner unavailable"));}
        let prepared=self.prepare_native_boot_core_app_data()?;
        let (queue,receiver)=mpsc::channel();let startup=Arc::new((Mutex::new(None),Condvar::new()));
        let runtime=Arc::new(Runtime{system:Arc::downgrade(self),bridge:bridge.clone(),leaf,events:Mutex::new(None),queue,startup:startup.clone(),
            stages:Mutex::new(BTreeSet::new()),operation:Mutex::new(()),metrics:Mutex::new(None),decompression,boot_apex_changed,closed:std::sync::atomic::AtomicBool::new(false)});
        let event_binder=self.process.add_service(Arc::new(Events(runtime.clone())));
        let events=self.process.local_service(match event_binder{Binder::Local(pointer)=>pointer,_=>return Err(fail("Boot events local owner unavailable"))}).ok_or_else(||fail("Boot events publication unavailable"))?;
        *runtime.events.lock().unwrap()=Some(event_binder);
        let run=runtime.clone();let thread=thread::spawn(move||while let Ok(job)=receiver.recv(){match job {
            Job::Stop=>break,
            Job::BootDeferred(prepared)=>{let result=run.system().and_then(|system|system.prepare_native_boot_deferred_app_data(prepared));*run.startup.0.lock().unwrap()=Some(result);run.startup.1.notify_all();},
            Job::Prepare=>{let result=run.prepare_app_data();*run.startup.0.lock().unwrap()=Some(result);run.startup.1.notify_all();},
            Job::Barrier(reply)=>{let result=run.startup.0.lock().unwrap().clone().unwrap_or_else(||Err(fail("Boot app-data startup was not queued")));let _=reply.send(result);},
        }});
        let worker=Worker{runtime:runtime.clone(),thread:Some(thread),events};
        {
            let mut state=self.package_bootstrap.lock().unwrap();let current=state.current.as_mut().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||fail("Boot lifecycle bootstrap replaced"))?;
            if current.boot_lifecycle.is_some(){return Err(fail("Boot lifecycle already initialized"));}
            current.boot_lifecycle=Some(runtime.clone());
        }
        runtime.queue.send(Job::BootDeferred(prepared)).map_err(|_|fail("Boot app-data preparation queue stopped"))?;
        Ok(worker)
    }
    fn boot_lifecycle_runtime(&self)->Result<Arc<Runtime>>{self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current|current.boot_lifecycle.clone()).ok_or_else(||fail("Boot lifecycle runtime unavailable"))}
    pub fn run_native_package_lifecycle(&self,stage:i32)->Result<()>{self.boot_lifecycle_runtime()?.run(stage)}
    pub fn wait_native_package_app_data(&self)->Result<()>{self.boot_lifecycle_runtime()?.wait()}
    pub fn update_native_package_metrics(&self,record:&[u8])->Result<()> {
        let mut r=Reader::new(record,&[]);let read=|status|fail(format!("Android display metrics record: {status}"));
        if r.read_i32().map_err(read)?!=1{return Err(fail("Android display metrics record version differs"));}
        let metrics=AndroidMetrics{width_pixels:r.read_i32().map_err(read)?,height_pixels:r.read_i32().map_err(read)?,density_dpi:r.read_i32().map_err(read)?,density:r.read_f32().map_err(read)?,scaled_density:r.read_f32().map_err(read)?,xdpi:r.read_f32().map_err(read)?,ydpi:r.read_f32().map_err(read)?};
        if r.remaining()!=0||metrics.width_pixels<=0||metrics.height_pixels<=0||metrics.density_dpi<=0||[metrics.density,metrics.scaled_density,metrics.xdpi,metrics.ydpi].iter().any(|value|!value.is_finite()||*value<=0.0){return Err(Exception::illegal_argument("Invalid Android display metrics"));}
        *self.boot_lifecycle_runtime()?.metrics.lock().unwrap()=Some(metrics);Ok(())
    }
    pub fn native_package_android_metrics(&self)->Result<AndroidMetrics>{self.boot_lifecycle_runtime()?.metrics.lock().unwrap().clone().ok_or_else(||fail("Android display metrics not supplied"))}
}
fn fail(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
impl Runtime {
    fn system(&self)->Result<Arc<System>>{if self.closed.load(std::sync::atomic::Ordering::Acquire){return Err(fail("Boot lifecycle closed"));}let system=self.system.upgrade().ok_or_else(||fail("Boot native System stopped"))?;system.check_package_bootstrap(&self.bridge)?;Ok(system)}
    fn call(&self,code:u32,fill:impl FnOnce(&mut Parcel))->Result<()> {
        self.system()?;let mut request=Parcel::new();request.write_interface_token(leaf::DESCRIPTOR);fill(&mut request);
        let reply=self.leaf.transact(code,&request,false).map_err(|status|fail(format!("Boot lifecycle owner: {status}")))?;
        let mut reader=reply.reader();reader.read_exception().map_err(|status|fail(format!("Boot lifecycle reply: {status}")))??;
        if reader.remaining()!=0{return Err(fail("Boot lifecycle trailing data"));}self.system()?;Ok(())
    }
    fn prepare_app_data(&self)->Result<()> {
        let system=self.system()?;
        let (users,flags)={let state=system.package_bootstrap.lock().unwrap();let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&self.bridge)).ok_or_else(||fail("Boot app-data bootstrap changed"))?;
            let image=current.install_environment.as_ref().ok_or_else(||fail("Boot app-data environment unavailable"))?;
            (image.users.iter().map(|user|user.id).collect::<Vec<_>>(),image.app_data_flags.clone())};
        for user in users{let flags=flags(user)?;if flags&3!=0{system.internal_reconcile_apps_data(user,flags,false,1000,0)?;}}
        Ok(())
    }
    fn wait(&self)->Result<()> {
        self.system()?;let (send,receive)=mpsc::channel();self.queue.send(Job::Barrier(send)).map_err(|_|fail("Boot app-data worker stopped"))?;
        receive.recv().map_err(|_|fail("Boot app-data barrier disconnected"))?
    }
    fn run(&self,stage:i32)->Result<()> {
        if !(1..=11).contains(&stage){return Err(Exception::illegal_argument("Unknown native package lifecycle stage"));}
        let _operation=self.operation.lock().unwrap();let system=self.system()?;
        if self.stages.lock().unwrap().contains(&stage){return Err(fail(format!("Native package lifecycle stage {stage} already completed")));}
        if (2..=9).contains(&stage)&&!self.stages.lock().unwrap().contains(&(stage-1)){return Err(fail(format!("Native package lifecycle stage {stage} is out of order")));}
        let installer=||->Result<Arc<crate::package::installer::native::NativeOwners>>{let state=system.package_bootstrap.lock().unwrap();state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&self.bridge))
            .and_then(|current|current.installer.as_ref().map(|(owner,_)|owner.clone())).ok_or_else(||fail("Boot installer unavailable"))};
        match stage {
            1=>{
                self.wait()?;
                let paths=self.decompression.release_paths().map_err(fail)?;
                self.call(leaf::RELEASE_COMPRESSED_BLOCKS,|p|{p.write_i32(paths.len() as i32);for path in &paths{p.write_string16(Some(path));}})?;
                self.decompression.acknowledge_released(&paths).map_err(fail)?;
                // Original PMS publishes mSystemReady before synchronous web,
                // AppsFilter, carrier and parser callbacks can query the owner.
                system.capture_package_queries()?.state().system.lifecycle.as_ref().ok_or_else(||fail("Boot lifecycle missing"))?.system_ready(1000).map_err(fail)?;
                system.start_web_instant_policy(&self.bridge)?;
                system.capture_package_queries()?.state().platform.settings_owner.as_ref().ok_or_else(||fail("Boot settings owner missing"))?.begin_system_ready()?;
                self.call(leaf::PREPARE_READY,|_|{})?;
                system.enter_native_package_settings_ready(&self.bridge)?;
                system.sweep_dangling_package_preferred(&self.bridge)?;
            },
            2=>{let events=*self.events.lock().unwrap();self.call(leaf::REGISTER_STORAGE_LISTENERS,|p|p.write_binder(events))?;},
            3=>{installer()?.free_stage_dirs(None)?;},
            4=>self.call(leaf::DEX_OPTIMIZER_READY,|_|{})?,
            5=>{self.queue.send(Job::Prepare).map_err(|_|fail("App-data queue stopped"))?;self.wait()?;},
            6=>{let events=*self.events.lock().unwrap();self.call(leaf::REGISTER_PACKAGE_OBSERVERS,|p|p.write_binder(events))?;},
            7=>{
                let capture=system.capture_package_queries()?;
                let owner=capture.state().system.module_metadata.as_ref().ok_or_else(||fail("Native module metadata not initialized"))?;
                // The native constructor loaded the original provider XML using
                // accepted APK/APEX code; retaining that owner completes readiness.
                if let Some(diagnostic)=owner.diagnostic(){eprintln!("Native module metadata: {diagnostic}");}
            },
            8=>installer()?.restore_native_staged_on_boot(&self.leaf)?,
            9=>{system.package_maintenance_owner()?.schedule_unused_library_prune()?;self.call(leaf::SCHEDULE_MAINTENANCE,|_|{})?;},
            10=>{
                let capture=system.capture_package_queries()?;let owner=capture.state().system.lifecycle.as_ref().ok_or_else(||fail("Boot dexopt lifecycle missing"))?;
                let reason=if owner.first_boot(){Some("first-boot")}else if owner.device_upgrading(){Some("boot-after-ota")}else if self.boot_apex_changed{Some("boot-after-mainline-update")}else{None};
                if let Some(reason)=reason{let completion=system.package_dexopt_completion(&self.bridge)?;self.call(leaf::UPGRADE_DEXOPT,|p|{p.write_string16(Some(reason));p.write_binder(Some(completion));})?;}
            },
            11=>self.call(leaf::PERFORM_FSTRIM,|_|{})?,
            _=>unreachable!(),
        }
        self.stages.lock().unwrap().insert(stage);Ok(())
    }
}
struct Events(Arc<Runtime>);
impl Service for Events {
    fn descriptor(&self)->&str{events::DESCRIPTOR}
    fn transact(&self,call:&mut Call<'_>)->Reply {
        let result=match call.code {
            events::VOLUME_READY=>{
                let args=events::VolumeReady::read(&mut call.data)?;if call.data.remaining()!=0{return Err(BAD_VALUE);}
                let _volume=args.volume_uuid;
                self.0.queue.send(Job::Prepare).map_err(|_|fail("Storage app-data queue stopped"))
            },
            events::OVERLAY_CHANGED=>{
                let args=events::OverlayChanged::read(&mut call.data)?;if call.data.remaining()!=0{return Err(BAD_VALUE);}
                (||->Result<()>{let system=self.0.system()?;let Some(name)=args.package_name else{return Ok(());};
                    let capture=system.capture_package_queries()?;let Some(package)=capture.state().packages.get(&name)else{return Ok(());};
                    let uid=crate::package::apps_filter::uid(args.user_id,package.app_id);
                    system.package_effects_owner(&self.0.bridge)?.package_changed(&name,uid,true,&[name.clone()],Some("android.intent.action.OVERLAY_CHANGED"),1000,false)
                })()
            },
            _=>return Err(UNKNOWN_TRANSACTION),
        };
        let mut reply=Parcel::new();match result{Ok(())=>reply.write_no_exception(),Err(error)=>reply.write_exception(&error)};Ok(reply)
    }
}
impl Drop for Worker {
    fn drop(&mut self){self.runtime.stop();if let Some(thread)=self.thread.take(){if thread.thread().id()!=thread::current().id(){let _=thread.join();}}
        let mut request=Parcel::new();request.write_interface_token(leaf::DESCRIPTOR);
        match self.runtime.leaf.transact(leaf::CLOSE,&request,false){
            Err(error)=>eprintln!("Boot lifecycle leaf cleanup transport: {error}"),
            Ok(reply)=>if let Err(error)=reply.reader().read_exception().and_then(|value|value.map_err(|_|BAD_VALUE)){eprintln!("Boot lifecycle leaf cleanup reply: {error}");},
        }
        let _=&self.events;
    }
}
