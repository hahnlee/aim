//! Native pre-scan constructor: actual image/data/bridge inputs, no PMS feed.
use super::*;
use crate::{system_package_persistence_init::Prepared,system_package_lifecycle::{Production,Restored},package::{
    bootstrap::{Bridge,ScanPolicy,DataBootInputs},scan::{SigningScan,SavedSystemScanInputs,SystemImagePackages},
    scan_snapshot::query_state::Capture,owner::{resources::CodeResources,usage::Usage},model,
}};
use aim_service_aidl::dev_aim_server_ipackageinitialcontextleaf as initial;
use std::collections::{BTreeMap,BTreeSet};
use std::path::{Path,PathBuf};

/// Root binds exactly one early original serializer, constructed independently
/// of PMS after the original D2 UserManagerService singleton is installed.
pub struct InitialOwner { node:Arc<Strong>, error:Mutex<Option<Exception>> }
impl InitialOwner {
    pub fn new(node:Strong)->Arc<Self>{Arc::new(Self{node:Arc::new(node),error:Mutex::new(None)})}
    pub(super) fn invoke<T>(&self,code:u32,fill:impl FnOnce(&mut Parcel),decode:impl FnOnce(&mut aim_binder_host::parcel::Reader<'_>)->std::result::Result<T,i32>)->Result<T>{
        let mut data=Parcel::new();data.write_interface_token(initial::DESCRIPTOR);fill(&mut data);
        let reply=self.node.transact(code,&data,false).map_err(|code|failure(format!("initial owner transport: {code}")))?;
        let mut reader=reply.reader();reader.read_exception().map_err(|code|failure(format!("initial owner exception: {code}")))??;
        let value=decode(&mut reader).map_err(|code|failure(format!("initial owner record: {code}")))?;
        if reader.remaining()!=0{return Err(failure("initial owner record tail"));}Ok(value)
    }
    pub fn runtime_properties(self:&Arc<Self>,property_area:PathBuf)->crate::system_package_installer_init::Properties{
        let owner=self.clone();Arc::new(move||{
            use aim_android_init::props::{PropertyInfoArea,PropAreaReader};
            use aim_android_init::props::areas::{PROPERTY_INFO,PROPERTIES_SERIAL};
            let serial=||->Result<u32>{
                let bytes=std::fs::read(property_area.join(PROPERTIES_SERIAL)).map_err(|error|failure(format!("live property serial: {error}")))?;
                Ok(PropAreaReader::new(&bytes).map_err(failure)?.serial())
            };
            for _ in 0..8{
            let before=serial()?;
            let info=std::fs::read(property_area.join(PROPERTY_INFO)).map_err(|error|failure(format!("live property inventory: {error}")))?;
            let contexts=PropertyInfoArea::new(&info).map_err(failure)?;
            let mut names=BTreeSet::new();
            for context in contexts.contexts(){
                let bytes=std::fs::read(property_area.join(context)).map_err(|error|failure(format!("live property area {context}: {error}")))?;
                let area=PropAreaReader::new(&bytes).map_err(failure)?;
                names.extend(area.foreach().into_iter().map(|property|property.name));
            }
            let names=names.into_iter().collect::<Vec<_>>();
            let bytes=owner.invoke(initial::GET_LIVE_PROPERTIES,|parcel|{
                parcel.write_i32(names.len() as i32);for name in &names{parcel.write_string16(Some(name));}
            },|reader|aim_service_aidl::read_byte_array(reader)?.ok_or(aim_binder_host::parcel::BAD_VALUE))?;
            let mut reader=aim_binder_host::parcel::Reader::new(&bytes,&[]);let decode=|code|failure(format!("live property map parcel: {code}"));
            if reader.read_i32().map_err(decode)?!=1{return Err(failure("live property map schema"));}
            let count=reader.read_i32().map_err(decode)?;
            if usize::try_from(count).ok()!=Some(names.len()){return Err(failure("live property map inventory differs"));}
            let mut map=BTreeMap::new();
            for expected in names{
                let name=reader.read_string16().map_err(decode)?.ok_or_else(||failure("live property name null"))?;
                let value=reader.read_string16().map_err(decode)?.ok_or_else(||failure("live property value null"))?;
                if name!=expected||map.insert(name,value).is_some(){return Err(failure("live property map identity differs"));}
            }
            if reader.remaining()!=0{return Err(failure("live property map tail"));}
            if before==serial()?{return Ok(map);}
            }
            Err(failure("live property inventory changed throughout capture"))
        })
    }
    pub(super) fn capture_scan_inputs(&self)->Result<Vec<u8>>{
        self.invoke(initial::CAPTURE_IMAGE_SCAN_INPUTS,|_|{},|reader|aim_service_aidl::read_byte_array(reader)?.ok_or(aim_binder_host::parcel::BAD_VALUE))
    }
    pub(super) fn property_value(&self,name:&str)->Option<String>{
        match self.property(name){Ok(value)=>value,Err(error)=>{let mut first=self.error.lock().unwrap();if first.is_none(){*first=Some(error);}None}}
    }
    pub(super) fn check(&self)->Result<()>{match self.error.lock().unwrap().take(){Some(error)=>Err(error),None=>Ok(())}}
    pub(super) fn property(&self,name:&str)->Result<Option<String>>{
        let mut request=Parcel::new();initial::GetLiveProperty{name:Some(name.into())}.write(&mut request);
        let reply=self.node.transact(initial::GET_LIVE_PROPERTY,&request,false).map_err(|code|failure(format!("live original property: {code}")))?;
        let mut reader=reply.reader();let value=initial::read_get_live_property_reply(&mut reader).map_err(|code|failure(format!("live property reply: {code}")))??;
        if reader.remaining()!=0{return Err(failure("live property reply tail"));}Ok(value.filter(|value|!value.is_empty()))
    }

}
fn failure(message:impl Into<String>)->Exception{Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}

pub struct Begun {
    pub prepared:Prepared,pub lifecycle:Production,pub system_config:crate::package::system_config::SystemConfig,
    pub model_system:model::System,pub image:PathBuf,pub data:PathBuf,pub initial:Arc<InitialOwner>,
    pub original_context:Option<Strong>,
    pub retained_readers: Option<crate::package::settings::native_read::SharedReaders>,
}
pub struct Scanned {
    pub prepared:Prepared,pub capture:Arc<Capture>,pub runtime_metadata:crate::package::owner::runtime_metadata::State,
    pub apex_results:Vec<crate::package::scan::ApexScanResult>,pub registrations:Vec<crate::package::pkg::AndroidPackage>,
    pub lifecycle:Arc<crate::package::lifecycle::Owner>,
    pub boot_apex_changed:bool,
    pub decompression:Arc<crate::package::scan::boot_compressed::BootDecompression>,
}
pub struct RawProjection {
    pub begin:Begun,pub owner:SigningScan,pub usage:Usage,
    pub runtime_metadata:crate::package::owner::runtime_metadata::State,
    pub original:crate::package::scan_snapshot::boot_context::Original,
    pub runtime_system:model::System,pub classes:Arc<aim_android_image::linkage::Hierarchy>,
    pub platform:Arc<crate::package::parse::Platform>,pub resource_config:crate::package::parse::resources::Config,
    pub apex_inventory:crate::package::bootstrap::ApexInventory,pub scan_users:crate::package::bootstrap::ScanUsers,
    pub sdk_sandbox_package:String,pub apex_results:Vec<crate::package::scan::ApexScanResult>,
    pub registrations:Vec<crate::package::pkg::AndroidPackage>,
    pub boot_apex_changed:bool,
    pub decompression:Arc<crate::package::scan::boot_compressed::BootDecompression>,
}
pub struct RawScanned {
    pub projection:Mutex<Option<RawProjection>>,
    pub visibility:raw_visibility::Visibility,
}
#[path="system_package_raw_visibility.rs"]
mod raw_visibility;
#[path="system_package_raw_roles.rs"]
mod raw_roles;
#[path="system_package_factory_retention.rs"]
pub(crate) mod factory_retention;
impl RawScanned {
    pub fn permission_admissions_record(&self)->Result<Vec<u8>>{
        let projection=self.projection.lock().unwrap();
        let raw=projection.as_ref().ok_or_else(||failure("raw metadata epoch already consumed"))?;
        raw.owner.permission_admissions_record().map_err(failure)
    }
    pub fn device_upgrading(&self)->Result<bool>{
        let projection=self.projection.lock().unwrap();
        let raw=projection.as_ref().ok_or_else(||failure("raw metadata epoch already consumed"))?;
        Ok(raw.begin.lifecycle.owner.device_upgrading())
    }
    pub fn legacy_runtime_permissions_state_record(&self,user:i32)->Result<Vec<u8>>{
        let projection=self.projection.lock().unwrap();
        let raw=projection.as_ref().ok_or_else(||failure("raw metadata epoch already consumed"))?;
        let users=raw.original.users.iter().map(|record|record.id).collect();
        crate::package::internal_host::legacy_runtime_record_from_owners(&raw.owner,&raw.runtime_metadata,&users,user)
    }
    pub fn legacy_permissions_version(&self,user:i32)->Result<i32>{
        let projection=self.projection.lock().unwrap();
        let raw=projection.as_ref().ok_or_else(||failure("raw metadata epoch already consumed"))?;
        if !raw.original.users.iter().any(|record|record.id==user){return Err(failure("initial legacy permission user absent"));}
        Ok(raw.runtime_metadata.version(user))
    }
    pub fn legacy_permission_definitions_record(&self)->Result<Vec<u8>>{
        let snapshot=self.metadata_snapshot()?;
        Ok(crate::package::scan_snapshot::computer::legacy_permission_definitions_record(&snapshot))
    }
    pub fn cross_user_suspensions(&self)->Result<bool>{
        if self.projection.lock().unwrap().is_none(){return Err(failure("raw metadata epoch already consumed"));}
        Ok(crate::package::restrictions::PINNED_CROSS_USER_SUSPENSIONS)
    }
    /// Immutable admitted code/settings metadata before live permission capture.
    pub fn metadata_snapshot(&self)->Result<Arc<crate::package::scan_snapshot::Snapshot>>{
        let projection=self.projection.lock().unwrap();
        let raw=projection.as_ref().ok_or_else(||failure("raw metadata epoch already consumed"))?;
        crate::package::scan_snapshot::Store::new(raw.owner.clone(),raw.usage.clone())
            .map(|store|store.capture()).map_err(|error|failure(format!("initial metadata snapshot: {error:?}")))
    }
    pub fn initial_instant(&self,uid:i32)->Result<Option<String>>{self.visibility.instant(uid)}
    pub fn filter_package(&self,name:Option<&str>,uid:i32,user:i32,filter_uninstalled:bool)->Result<bool>{
        self.visibility.filter_package(name,uid,user,filter_uninstalled)
    }
    pub fn filter_uid(&self,target_uid:i32,calling_uid:i32)->Result<bool>{self.visibility.filter_uid(target_uid,calling_uid)}
}
impl Begun {
    pub fn bind_original_context(&mut self,context:Strong)->Result<()>{
        if self.original_context.is_some(){return Err(failure("original boot context already bound"));}
        self.original_context=Some(context);Ok(())
    }
}
impl System {
    /// Begin before UM-dependent facts are requested. The restored version copy
    /// is bound before scan forceCurrent; this phase does not query UM or PMS.
    pub fn begin_native_package_scan(self:&Arc<Self>,bridge:&Arc<Bridge>,prepared:Prepared,
        image:&Path,data:&Path,initial:Arc<InitialOwner>,ce_leaf:Strong)->Result<Begun>{
        self.check_package_bootstrap(bridge)?;
        let original=prepared.settings.clone();let restored=Restored::capture(&original,&prepared.report);
        let live=initial.clone();let lifecycle=Production::construct(restored,bridge,ce_leaf,Box::new(move|name|
            live.property_value(name)))?;
        let values=initial.clone();let property=|name:&str|values.property_value(name);
        let system_config=crate::package::system_config::SystemConfig::read(image,&property);
        let framework=crate::package::system_config::Framework::load(image).map_err(failure)?;
        let model_system=crate::package::system_config::system(image,&property,&framework).map_err(failure)?;
        initial.check()?;
        Ok(Begun{prepared,lifecycle,system_config,model_system,image:image.into(),data:data.into(),initial,original_context:None,retained_readers:None})
    }
    /// Finish after actual D2 UM construction. All policy arguments describe
    /// real image/CLI/native filesystem owners; no fixture supplies scan flags.
    pub fn finish_raw_native_package_scan(self:&Arc<Self>,bridge:&Arc<Bridge>,mut begin:Begun,
        original_context:Strong,runtime_system:model::System,classes:Arc<aim_android_image::linkage::Hierarchy>,
        apks:&crate::package::write::Apks,policy:ScanPolicy<'_>,resources:&CodeResources,
        volumes:&[String],old_stubs:&BTreeSet<String>,incremental_packages:&BTreeSet<String>,
        expecting_better:&BTreeSet<String>,destinations:&BTreeMap<String,crate::package::scan::NativeLibraryDestination<'_>>,
        is_incremental:&dyn Fn(&str)->std::result::Result<bool,String>,
        zip_time:&dyn Fn(u32)->std::result::Result<std::time::SystemTime,String>,resource_config:crate::package::parse::resources::Config)
        ->Result<RawScanned>{
        self.check_package_bootstrap(bridge)?;
        let values=begin.initial.clone();let props=|name:&str|values.property_value(name);
        let boot=bridge.resolve_boot(&begin.system_config,&props).map_err(|error|failure(format!("actual boot inputs: {error:?}")))?;
        let settings_leaf = Arc::new(self.binder_process().strong(original_context.handle));
        let mut original=crate::package::scan_snapshot::boot_context::Original::capture(&original_context)
            .map_err(failure)?.bind_settings(settings_leaf);
        original.resolver.settings_owner.as_ref().ok_or_else(|| failure("native initial settings owner absent"))?
            .register_updates(&self.binder_process())?;
        let source=begin.prepared.disk.lock().unwrap();
        let saved_users=source.state().users.iter().flat_map(|(id,user)|user.restrictions.packages.iter().map(move|(name,state)|(name.clone(),*id as i32,state.clone())))
            .fold(BTreeMap::<String,BTreeMap<i32,crate::package::restrictions::UserState>>::new(),|mut map,(name,id,state)|{map.entry(name).or_default().insert(id,state);map});
        drop(source);
        let upgrade=begin.lifecycle.owner.first_boot()||begin.lifecycle.owner.device_upgrading();
        let mut restored_runtime_metadata=None;
        let (mut owner,system,apex)=if begin.prepared.report.first_boot{
            let mut scan=boot.scan_first_boot(apks,policy).map_err(|error|failure(format!("first boot scan: {error:?}")))?;
            scan.owner.retain_first_boot_versions(&begin.prepared.settings).map_err(failure)?;
            let factories=SystemImagePackages{packages:scan.packages,retained_data:vec![],retained_code:vec![],rejected:scan.rejected};
            (scan.owner,factories,scan.apex)
        }else{
            let mut owner=SigningScan::new(&begin.system_config,&begin.prepared.settings,policy.first_api_level).map_err(|error|failure(format!("restored scan owner: {error:?}")))?;
            let readers = begin.retained_readers.as_ref().ok_or_else(|| failure("restored native Settings reader graph unavailable"))?;
            restored_runtime_metadata=Some(crate::package::settings::native_read::prepare_saved_permission_scan(
                readers,&mut owner,&begin.prepared.disk.lock().unwrap(),&begin.system_config)
                .map_err(|error|failure(format!("saved permission owner graph: {error}")))?);
            self.check_package_bootstrap(bridge)?;
            let system=boot.scan_saved_system(&mut owner,apks,policy,SavedSystemScanInputs{users:&saved_users,first_boot_or_upgrade:upgrade,old_stub_packages:old_stubs,incremental_packages,resources})
                .map_err(|error|failure(format!("saved system scan: {error:?}")))?;
            (owner,system.system,system.apex)
        };
        owner.prune_system_shared_users().map_err(|error|failure(format!("system shared owner pruning: {error}")))?;
        let mut retained_runtime=factory_retention::capture_saved_factories(&owner,
            crate::package::scan::SeInfoScan{policy:policy.seinfo,compatibility:bridge.as_ref()},bridge)?;
        let signing=owner.loaded_packages().get("android").ok_or_else(||failure("actual platform package absent"))?.collected_signing.clone();
        boot.scan_data(&mut owner,apks,policy,DataBootInputs{factories:&system,platform:&signing,volumes,users:&saved_users,first_boot_or_upgrade:upgrade,old_stub_packages:old_stubs,expecting_better,is_incremental,destinations,resources})
            .map_err(|error|failure(format!("native data scan: {error:?}")))?;
        let decompression=Arc::new(crate::package::scan::boot_compressed::BootDecompression::new(begin.data.join("app")).map_err(failure)?);
        let mut usage=Usage::new(owner.loaded_packages().keys().map(String::as_str));usage.read(&begin.data.join("system/package-usage.list")).map_err(failure)?;
        self.expand_native_boot_stubs(bridge,&begin,&mut owner,apks,policy,&system,&decompression,zip_time,&usage,&mut retained_runtime)?;
        let runtime_metadata=if begin.prepared.report.first_boot{
            // No saved runtime input was read by readLPw. Real constructor
            // SettingBase owners remain empty until permission initialization.
            let users=boot.users().users.as_ref().ok_or_else(||failure("first boot user owner absent"))?.iter().map(|user|user.id).collect::<Vec<_>>();
            let packages=owner.settings.packages.iter().map(|package|((package.name.clone(),false),crate::package::owner::legacy_permissions::Migration::default()))
                .chain(owner.settings.disabled_system_packages.iter().map(|package|((package.name.clone(),true),crate::package::owner::legacy_permissions::Migration::default()))).collect();
            let shared=owner.identities.shared_users.keys().map(|name|(name.clone(),crate::package::owner::legacy_permissions::Migration::default())).collect();
            owner.capture_legacy_permissions(&users,packages,shared).map_err(failure)?;
            let fixed=owner.settings.packages.iter().map(|setting|((setting.name.clone(),false),setting.install_permissions_fixed))
                .chain(owner.settings.disabled_system_packages.iter().map(|setting|((setting.name.clone(),true),setting.install_permissions_fixed))).collect();
            owner.capture_install_permissions_fixed(fixed).map_err(failure)?;
            crate::package::owner::runtime_metadata::State::default()
        }else{restored_runtime_metadata.ok_or_else(||failure("restored runtime metadata owner unavailable"))?};
        owner.finish_boot_settings(&begin.lifecycle.current_version, &begin.lifecycle.owner).map_err(failure)?;
        let mut owner=self.complete_package_owner(bridge,owner,&usage,retained_runtime)?;
        owner.complete_boot_loading(is_incremental).map_err(|error|failure(format!("native loading completion: {error:?}")))?;
        owner.rebuild_shared_processes_from_native_members().map_err(failure)?;
        // Persist the completed native boot owner on every boot. Restored
        // reconciliation changes Settings too; existing resolver/browser owner
        // sections are retained rather than first-boot seeded again.
        let snapshot=crate::package::scan_snapshot::Store::new(owner.clone(),usage.clone()).map_err(|error|failure(format!("completed persistence scan: {error:?}")))?.capture();
        begin.prepared.disk.lock().unwrap().commit_completed_boot_scan(&snapshot,begin.prepared.report.first_boot)
            .map_err(|error|failure(error.to_string()))?;
        let mut documents=BTreeMap::new();
        {
            let source=begin.prepared.disk.lock().unwrap();
            for (id,_) in &source.state().users{
                let document=source.preferred_user_document(*id as i32).map_err(failure)?;
                let browser=source.pending_default_browser(*id).map_err(failure)?;
                documents.insert(*id as i32,(Some(document.clone()),document,browser));
            }
        }
        original=original.with_native_users(&documents).map_err(failure)?;
        let action=begin.initial.invoke(initial::GET_SDK_SANDBOX_SERVICE_ACTION,|_|{},|r|r.read_string16())?.ok_or_else(||failure("original SDK sandbox action absent"))?;
        let mut sandbox=Vec::new();
        for setting in &owner.settings.packages{
            if setting.flags&crate::package::settings::FLAG_SYSTEM==0{continue;}
            let Some(code)=owner.loaded_packages().get(&setting.name)else{continue};
            let state=saved_users.get(&setting.name).and_then(|users|users.get(&0));
            if state.is_some_and(|state|!state.installed||state.hidden||matches!(state.enabled,2|3|4)){continue;}
            if state.map_or(true,|state|state.enabled==0)&&!code.package.is(crate::package::pkg::booleans::ENABLED){continue;}
            for service in &code.package.services{
                let component=&service.main.component.name;
                let enabled=state.and_then(|state|state.enabled_components.as_ref()).is_some_and(|names|names.contains(component))
                    || (service.main.enabled&&!state.and_then(|state|state.disabled_components.as_ref()).is_some_and(|names|names.contains(component)));
                if enabled&&service.main.component.intents.iter().any(|intent|intent.filter.actions.contains(&action)){
                    sandbox.push(setting.name.clone());
                }
            }
        }
        if sandbox.len()!=1{return Err(failure(format!("required SDK sandbox service matches: {}",sandbox.len())));}
        let sdk_sandbox_package=sandbox.remove(0);
        let registrations=owner.loaded_packages().values().map(|code|code.package.clone()).collect();
        let visibility=raw_visibility::Visibility::new(&owner,&original,&runtime_system,&apks.platform,resource_config,&sdk_sandbox_package,bridge)?;
        begin.initial.check()?;
        let apex_inventory=boot.apex().clone();let scan_users=boot.users().clone();
        let bcp_modules=begin.initial.invoke(initial::GET_BOOT_CLASS_PATH_APEX_MODULES,|_|{},|reader|{
            let count=reader.read_i32()?;if count<0{return Err(-22);}let mut modules=BTreeSet::new();
            for _ in 0..count{modules.insert(reader.read_string16()?.ok_or(-22)?);}Ok(modules)
        })?;
        let boot_apex_changed=apex_inventory.active.iter().any(|apex|apex.active_changed&&apex.module_name.as_ref().is_some_and(|name|bcp_modules.contains(name)));

        let platform=Arc::new(crate::package::parse::Platform::load(&begin.image,apks.platform.features.clone()).map_err(failure)?);
        Ok(RawScanned{projection:Mutex::new(Some(RawProjection{begin,owner,usage,runtime_metadata,original,runtime_system,classes,
            platform,resource_config,apex_inventory,scan_users,sdk_sandbox_package,
            apex_results:apex,registrations,boot_apex_changed,decompression})),visibility})
    }
    /// Permission projections run only after Java installs real early Internal reads.
    pub fn finish_native_package_capture(self:&Arc<Self>,bridge:&Arc<Bridge>,raw:&Arc<RawScanned>)->Result<Scanned>{
        self.check_package_bootstrap(bridge)?;
        let projection=raw.projection.lock().unwrap().take().ok_or_else(||failure("raw capture projection already consumed"))?;
        let RawProjection{begin,owner,usage,runtime_metadata,original,runtime_system,classes,platform,resource_config,
            apex_inventory,scan_users,sdk_sandbox_package,apex_results,registrations,boot_apex_changed,decompression}=projection;
        let snapshot=crate::package::scan_snapshot::Store::new(owner.clone(),usage.clone())
            .map_err(|error|failure(format!("initial native snapshot: {error:?}")))?.capture();
        let input=crate::package::scan_snapshot::boot_context::Inputs{snapshot,config:Arc::new(begin.system_config.clone()),platform:platform.clone(),bridge:bridge.clone(),
            image_system:begin.model_system,runtime_system,resolver:original.resolver.clone(),users:vec![],classes,
            apex:apex_inventory,scan_users,resource_config,
            cross_user_suspensions:crate::package::restrictions::PINNED_CROSS_USER_SUSPENSIONS,
            nonce:None,sdk_sandbox_package};
        let input=original.populate(input).map_err(failure)?;
        let context=crate::package::scan_snapshot::boot_context::build(input).map_err(failure)?;
        let values=begin.initial.clone();let props=|name:&str|values.property_value(name);
        begin.initial.check()?;
        self.publish_package_scan_with_configured_roles(bridge,None,owner,usage,context,&platform,resource_config,&props,&begin.system_config)?;
        self.install_package_freezer_publication(bridge)?;
        self.initialize_web_instant_policy(bridge)?;
        let capture=self.capture_package_queries()?;
        Ok(Scanned{prepared:begin.prepared,capture,runtime_metadata,apex_results,registrations,lifecycle:begin.lifecycle.owner,boot_apex_changed,decompression})
    }
}

impl System {
    fn expand_native_boot_stubs(&self,bridge:&Arc<Bridge>,begin:&Begun,owner:&mut SigningScan,
        apks:&crate::package::write::Apks,policy:ScanPolicy<'_>,factories:&SystemImagePackages,decompression:&crate::package::scan::boot_compressed::BootDecompression,
        zip_time:&dyn Fn(u32)->std::result::Result<std::time::SystemTime,String>,usage:&Usage,
        retained_runtime:&mut BTreeMap<(String,bool),crate::package::scan::OriginalRuntime>)->Result<()> {
        use crate::package::scan::{AbiScanContext,AbiScanMode,NativeLibraryEnvironment,NativeLibraryDestination,ScanMetadataCompletion,SettingUpdate,Code,Location,Partition,Kind};
        use crate::package::pkg::booleans2;
        let properties=|name:&str|begin.initial.property_value(name);
        let compatibility=bridge.library_compatibility(&begin.system_config,&properties).map_err(|error|failure(format!("compressed library policy: {error:?}")))?;
        begin.initial.check()?;
        let users=bridge.scan_users().map_err(|error|failure(format!("compressed scan users: {error:?}")))?;
        let platform=owner.loaded_packages().get("android").ok_or_else(||failure("compressed platform owner missing"))?.collected_signing.clone();
        let names=owner.settings.packages.iter().filter(|setting|owner.loaded_packages().get(&setting.name).is_some_and(|code|code.package.is2(booleans2::STUB))).map(|setting|setting.name.clone()).collect::<Vec<_>>();
        for name in names.into_iter().rev() {
            let stub=owner.loaded_packages().get(&name).map(|code|code.package.clone());
            let disabled=owner.settings.disabled_system_packages.iter().any(|setting|setting.name==name);
            let state=owner.scanned_user_states(&name).and_then(|states|states.get(&0)).cloned().unwrap_or_default();
            if !crate::package::scan::boot_compressed::BootDecompression::admitted(disabled,stub.as_ref(),state.enabled){continue;}
            let stub=stub.ok_or_else(||failure("admitted compressed stub owner disappeared"))?;
            let saved=owner.settings.packages.iter().find(|setting|setting.name==name).cloned().ok_or_else(||failure("compressed setting missing"))?;
            let label_data=begin.data.canonicalize().map_err(|error|failure(error.to_string()))?;
            let label=|path:&Path|->std::result::Result<(),String>{
                let tail=path.strip_prefix(&label_data).map_err(|error|error.to_string())?;
                let guest=format!("/data/{}",tail.to_str().ok_or("compressed native path UTF-8")?);
                bridge.restore_installer_context(&guest).map_err(|error|format!("compressed native label: {error:?}"))
            };
            let retained=factory_retention::capture(owner,&name,usage,bridge)?;
            let expanded=match decompression.decompress(&stub,apks,policy.abi,policy.install,zip_time,&label){
                Ok(Some(expanded))=>Some(expanded),Ok(None)=>None,
                Err(error)=>{eprintln!("Failed to decompress system package {name}: {error}");None}
            };
            let mut succeeded=false;
            if let Some(mut expanded)=expanded {
                let attempt=(||->std::result::Result<(SigningScan,crate::package::scan::OriginalRuntime),String>{
                    let mut staged=owner.clone();
                    staged.disable_system_package(&name).map_err(|error|format!("compressed factory retention: {error:?}"))?;
                    let signing=apks.signing_details(&expanded.package)?;
                    let factory=factories.packages.iter().find(|completed|completed.candidate.record.settings.name==name)
                        .map(|completed|&completed.candidate.record).ok_or("compressed factory scan record missing")?;
                    let mut manifest=crate::package::scan::ScanPolicy::default();
                    manifest.inherit_system_setting(&saved);
                    manifest.adjust_shared_uid_privilege(&expanded.package,&signing,&platform,&staged.identities,policy.vendor_sdk);
                    manifest.apply_manifest(&mut expanded.package,&signing,Some(&platform),true,apks)?;
                    compatibility.apply(&mut expanded.package,true,true,None)?;
                    let parsed=&expanded.package;
                    let versions=parsed.uses_sdk_libraries_versions_major.as_deref().unwrap_or_default();
                    let optional=parsed.uses_sdk_libraries_optional.as_deref().unwrap_or_default();
                    let static_versions=parsed.uses_static_libraries_versions.as_deref().unwrap_or_default();
                    if versions.len()!=parsed.uses_sdk_libraries.len()||optional.len()!=versions.len()||static_versions.len()!=parsed.uses_static_libraries.len(){return Err("compressed library metadata arrays disagree".into());}
                    let (flags,private_flags)=crate::package::scan::application_flags(parsed,true);
                    let update=SettingUpdate{code_path:expanded.receipt.guest_path.clone(),legacy_native_library_path:saved.legacy_native_library_path.clone(),primary_cpu_abi:None,secondary_cpu_abi:None,flags,private_flags,
                        uses_sdk_libraries:parsed.uses_sdk_libraries.iter().zip(versions).zip(optional).map(|((name,version),optional)|crate::package::settings::UsesSdkLibrary{name:name.clone(),version_major:*version,optional:*optional}).collect(),
                        uses_static_libraries:parsed.uses_static_libraries.iter().cloned().zip(static_versions.iter().copied()).collect(),mime_groups:parsed.mime_groups.clone(),domain_set_id:bridge.new_domain_id().map_err(|error|format!("compressed domain: {error:?}"))?,target_sdk_version:parsed.target_sdk_version,restrict_update_hash:parsed.restrict_update_hash.clone()};
                    let mut saved_users=BTreeMap::new();
                    for setting in &staged.settings.packages{if let Some(states)=staged.scanned_user_states(&setting.name){saved_users.insert(setting.name.clone(),states.clone());}}
                    let environment=NativeLibraryEnvironment{preferred_abi:policy.preferred_abi,app_lib32_install_dir:policy.app_lib32_install_dir,code_is_directory:true,canonical_source:None};
                    let library_guest=format!("{}/lib",expanded.receipt.guest_path);
                    let library_host=(apks.files)(&library_guest).ok_or("compressed native destination mapping missing")?;
                    let destination=NativeLibraryDestination{guest_root:&library_guest,root:&library_host,owner:aim_storage::guest_inode::GuestInode{uid:Some(1000),gid:Some(1000),mode:None},zip_time,restorecon:&label};
                    let code=Code{location:Location{path:expanded.receipt.guest_path.clone(),partition:Partition::Data,kind:Kind::App,apex:None},parsed:expanded.package.clone(),signing};
                    staged.scan_existing(&code,update,&saved_users,users.users.as_deref(),Some(factory),apks,ScanMetadataCompletion{
                        scan_as_instant_app:saved_users.get(&name).and_then(|users|users.get(&0)).is_some_and(|state|state.instant_app),
                        seinfo:crate::package::scan::SeInfoScan{policy:policy.seinfo,compatibility:bridge.as_ref()},abi_policy:policy.abi,native_environment:&environment,
                        context:AbiScanContext{mode:AbiScanMode::Existing{first_boot_or_upgrade:true,old_was_stub:true,saved:Some(&saved)},system:true,updated:true,override_abi:None,platform_runtime_64bit:None},
                        install:policy.install,destination:Some(&destination),clock:policy.clock,factory_test:policy.factory_test,
                    }).map_err(|error|format!("compressed boot scan: {error:?}"))?;
                    let mut state=staged.scanned_user_states(&name).and_then(|states|states.get(&0)).cloned().unwrap_or_default();
                    state.enabled=0;state.last_disable_app_caller=Some("android".into());staged.set_user_state(&name,0,state)?;
                    retained.check_factory(&staged)?;
                    Ok((staged,retained.runtime))
                })();
                match attempt{Ok((staged,retained))=>{retained_runtime.insert((name.clone(),true),retained);*owner=staged;succeeded=true;},Err(error)=>{decompression.reject(&expanded.receipt).map_err(failure)?;eprintln!("Failed to install compressed system package {name}: {error}");}}
            }
            if !succeeded {
                let mut state=owner.scanned_user_states(&name).and_then(|states|states.get(&0)).cloned().unwrap_or_default();
                state.enabled=2;state.last_disable_app_caller=Some("android".into());owner.set_user_state(&name,0,state).map_err(failure)?;
            }
        }
        Ok(())
    }
}
