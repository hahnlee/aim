//! Native PMInternal storage policy over actual independent installd/storage/ART.
use super::*;
use std::collections::BTreeMap;
use aim_service_aidl::{dev_aim_server_ipackageinternalstoragebridge as storage_api,dev_aim_server_ipackagebootstrapbridge as bootstrap_api};

struct StorageLeaf { owner: aim_binder_host::local::Strong }
pub(crate) struct BootAppData { flags:i32,encrypted:bool,deferred:Vec<String> }
impl StorageLeaf {
    fn call<T>(&self,code:u32,write:impl FnOnce(&mut Parcel),read:impl FnOnce(&mut Reader<'_>)->std::result::Result<T,i32>)->Result<T>{
        let mut request=Parcel::new();request.write_interface_token(storage_api::DESCRIPTOR);write(&mut request);
        let reply=self.owner.transact(code,&request,false).map_err(|status|storage_error(format!("Internal storage owner: {status}")))?;
        let mut reader=reply.reader();reader.read_exception().map_err(|status|storage_error(format!("Internal storage exception: {status}")))??;
        let value=read(&mut reader).map_err(|status|storage_error(format!("Internal storage reply: {status}")))?;
        if reader.remaining()!=0{return Err(storage_error("Internal storage reply trailing bytes"));}Ok(value)
    }
}
impl System {
    fn internal_storage_leaf(&self)->Result<StorageLeaf>{
        let bridge=self.package_bootstrap()?;self.check_package_bootstrap(&bridge)?;
        let mut request=Parcel::new();request.write_interface_token(bootstrap_api::DESCRIPTOR);
        let reply=bridge.owner.transact(bootstrap_api::GET_PACKAGE_INTERNAL_STORAGE_BRIDGE,&request,false)
            .map_err(|status|storage_error(format!("Internal storage attach: {status}")))?;
        let mut reader=reply.reader();reader.read_exception().map_err(|status|storage_error(format!("Internal storage attach reply: {status}")))??;
        let binder=reader.read_binder().map_err(|status|storage_error(format!("Internal storage binder: {status}")))?.ok_or_else(||storage_error("Internal storage owner unavailable"))?;
        if reader.remaining()!=0{return Err(storage_error("Internal storage attach trailing data"));}
        let owner=reply.retain_remote_binder(binder).map_err(|status|storage_error(format!("Internal storage lifetime: {status}")))?;
        self.check_package_bootstrap(&bridge)?;Ok(StorageLeaf{owner})
    }
    pub fn internal_free_storage(self:&Arc<Self>,volume_uuid:Option<String>,bytes:i64,flags:i32,_calling_uid:i32,_calling_pid:i32)->Result<()>{
        let bridge=self.package_bootstrap()?;
        let installer={let state=self.package_bootstrap.lock().unwrap();state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,&bridge))
            .and_then(|current|current.installer.as_ref().map(|(owner,_)|owner.clone())).ok_or_else(||storage_error("Native storage installer unavailable"))?};
        let deletion=installer.removal_owner()?;
        let maintenance=Arc::new(bridge.package_maintenance().map_err(|error|storage_error(format!("Native maintenance storage: {error:?}")))?);
        let system=Arc::downgrade(self);let retained=bridge.clone();
        let current=Arc::new(move||{let system=system.upgrade().ok_or_else(||storage_error("Native storage system stopped"))?;system.check_package_bootstrap(&retained)?;system.capture_package_queries()});
        let owner=crate::package::diagnostics_storage::Owner::new(maintenance,current,deletion,installer);
        match owner.free(volume_uuid.as_deref(),bytes,flags){Ok(())=>Ok(()),Err(crate::package::diagnostics_storage::Error::Owner(error))=>Err(error),Err(crate::package::diagnostics_storage::Error::Io(message))=>Err(storage_io(message))}
    }
    pub fn internal_free_all_app_cache_above_quota(&self,volume_uuid:Option<String>,_calling_uid:i32,_calling_pid:i32)->Result<()>{
        let leaf=self.internal_storage_leaf()?;let _install=self.package_install_guard();
        leaf.call(storage_api::FREE_ALL_ABOVE_QUOTA,|p|p.write_string16(volume_uuid.as_deref()),|_|Ok(()))
    }
    pub fn internal_delete_oat_artifacts_of_package(&self,package_name:Option<String>,calling_uid:i32,_calling_pid:i32)->Result<i64>{
        if !matches!(calling_uid,0|1000|2000){return Err(Exception::security("Only the system or shell can delete oat artifacts"));}
        self.internal_storage_leaf()?.call(storage_api::DELETE_OAT,|p|{p.write_string16(package_name.as_deref());p.write_i32(calling_uid);},|r|r.read_i64())
    }
    pub fn internal_migrate_legacy_obb_data(&self,_calling_uid:i32,_calling_pid:i32)->Result<()>{
        self.internal_storage_leaf()?.call(storage_api::MIGRATE_OBB,|_|{},|_|Ok(()))
    }
    pub fn internal_reconcile_apps_data(&self,user_id:i32,flags:i32,migrate_apps_data:bool,_calling_uid:i32,_calling_pid:i32)->Result<()>{
        let leaf=self.internal_storage_leaf()?;
        let volumes=leaf.call(storage_api::WRITABLE_VOLUMES,|_|{},|r|aim_service_aidl::read_string_list(r)?.ok_or(aim_binder_host::parcel::BAD_VALUE))?;
        let encrypted=leaf.call(storage_api::FILE_ENCRYPTED,|_|{},|r|r.read_bool())?;
        let apply_device=leaf.call(storage_api::APPLY_DEFAULT_DEVICE_STORAGE,|_|{},|r|r.read_bool())?;
        if flags&2!=0&&encrypted&&!leaf.call(storage_api::CE_UNLOCKED,|p|p.write_i32(user_id),|r|r.read_bool())?{return Err(storage_error("Refusing CE reconciliation while user is locked; it would destroy valid data"));}
        self.reconcile_apps_data_volumes(&leaf,volumes,user_id,flags,migrate_apps_data,None,None,encrypted,apply_device).map(|_|())
    }
    /// AppDataHelper.fixAppsDataOnBoot: internal volume/system user with flags
    /// based on file encryption, independent of whether AMS has started user 0.
    pub(crate) fn prepare_native_boot_core_app_data(&self)->Result<BootAppData> {
        let leaf=self.internal_storage_leaf()?;
        let encrypted=leaf.call(storage_api::FILE_ENCRYPTED,|_|{},|r|r.read_bool())?;
        let flags=if encrypted{1}else{3};
        let apply_device=leaf.call(storage_api::APPLY_DEFAULT_DEVICE_STORAGE,|_|{},|r|r.read_bool())?;
        let deferred=self.reconcile_apps_data_volumes(&leaf,vec![None],0,flags,true,Some(true),None,encrypted,apply_device)?;
        Ok(BootAppData{flags,encrypted,deferred})
    }
    pub(crate) fn prepare_native_boot_deferred_app_data(&self,prepared:BootAppData)->Result<()> {
        let leaf=self.internal_storage_leaf()?;
        leaf.call(storage_api::FIXUP_DATA,|p|{p.write_string16(None);p.write_i32(3);},|_|Ok(()))?;
        let apply_device=leaf.call(storage_api::APPLY_DEFAULT_DEVICE_STORAGE,|_|{},|r|r.read_bool())?;
        self.reconcile_apps_data_volumes(&leaf,vec![None],0,prepared.flags,true,Some(false),Some(&prepared.deferred),prepared.encrypted,apply_device).map(|_|())
    }
    fn reconcile_apps_data_volumes(&self,leaf:&StorageLeaf,volumes:Vec<Option<String>>,user_id:i32,flags:i32,
        migrate_apps_data:bool,only_core:Option<bool>,deferred:Option<&[String]>,encrypted:bool,apply_device:bool)->Result<Vec<String>> {
        let installer={let state=self.package_bootstrap.lock().unwrap();state.current.as_ref().and_then(|current|current.installer.as_ref().map(|(owner,_)|owner.clone())).ok_or_else(||storage_error("Reconciliation package data owner unavailable"))?};
        let store=installer.removal_owner()?.store.clone();
        let mut deferred_packages=Vec::new();
        for volume in volumes {
            let before=store.snapshots.capture().version();
            let install=self.package_install_guard();
            let mut receipts=BTreeMap::new();
            let result=(|| -> Result<()> {
            if only_core!=Some(false) {leaf.call(storage_api::CLEANUP_INVALID,|p|{p.write_string16(volume.as_deref());p.write_i32(user_id);p.write_i32(flags);},|_|Ok(()))?;}
            let capture=self.capture_package_queries()?;let state=capture.state();
            for (mask,ce) in [(2,true),(1,false)] {
                if only_core==Some(false){break;}
                if flags&mask==0{continue;}
                let names=leaf.call(storage_api::DATA_DIRECTORY_NAMES,|p|{p.write_string16(volume.as_deref());p.write_i32(user_id);p.write_bool(ce);},|r|aim_service_aidl::read_string_list(r)?.ok_or(aim_binder_host::parcel::BAD_VALUE))?;
                for name in names {
                    let Some(name)=name else{return Err(storage_error("Null app-data directory name"));};
                    let setting=state.packages.get(&name);let valid=setting.is_some_and(|setting|setting.volume_uuid==volume&&storage_allowed(setting)&&{
                        let user=crate::package::info::user_state(setting,user_id);user.installed||user.ce_data_inode!=0||user.de_data_inode!=0});
                    if !valid{let inode=setting.map_or(0,|setting|crate::package::info::user_state(setting,user_id).ce_data_inode);
                        leaf.call(storage_api::DESTROY_DATA,|p|{p.write_string16(volume.as_deref());p.write_string16(Some(&name));p.write_i32(user_id);p.write_i32(mask);p.write_i64(inode);},|_|Ok(()))?;}
                }
            }
            let mut failure:Option<Exception>=None;
            for setting in state.packages.values().filter(|setting|setting.volume_uuid==volume&&setting.pkg.is_some()) {
                let code=setting.pkg.as_ref().unwrap();
                if only_core==Some(true)&&!code.is(crate::package::pkg::booleans::CORE_APP){deferred_packages.push(setting.name.clone());continue;}
                if deferred.is_some_and(|names|!names.contains(&setting.name)){continue;}
                if !crate::package::info::user_state(setting,user_id).installed||!storage_allowed(setting){continue;}
                let prepared=(|| -> Result<()> {
                let seinfo=setting.seinfo.as_ref().ok_or_else(||storage_error("Reconciliation seinfo owner unavailable"))?;
                let state_user=crate::package::info::user_state(setting,user_id);let seinfo=format!("{seinfo}{}",if state_user.instant_app{":ephemeralapp:complete"}else{":complete"});
                let create=||leaf.call(storage_api::PREPARE_DATA,|p|{p.write_string16(volume.as_deref());p.write_string16(Some(&setting.name));p.write_i32(user_id);p.write_i32(flags);p.write_i32(setting.app_id);p.write_string16(Some(&seinfo));p.write_i32(setting.target_sdk_version);p.write_bool(!setting.uses_sdk_libraries.is_empty());},|r|aim_service_aidl::read_long_array(r)?.filter(|values|values.len()==2).ok_or(aim_binder_host::parcel::BAD_VALUE));
                let mut user=capture.scan().owner().scanned_user_states(&setting.name)
                    .ok_or_else(||storage_error("App-data receipt package user owner unavailable"))?
                    .get(&user_id).cloned().unwrap_or_default();
                let mut record=|inodes:&[i64]| {
                    if flags&2!=0&&inodes[0]!=-1{user.ce_data_inode=inodes[0];}
                    if flags&1!=0&&inodes[1]!=-1{user.de_data_inode=inodes[1];}
                    receipts.insert(setting.name.clone(),user.clone());
                };
                let mut inodes=create()?;
                record(&inodes);
                leaf.call(storage_api::PREPARE_CONTENTS,|p|{p.write_string16(volume.as_deref());p.write_string16(Some(&setting.name));p.write_i32(user_id);p.write_i32(flags);p.write_string16(setting.primary_cpu_abi.as_deref());p.write_string16(code.native_library_dir.as_deref());},|_|Ok(()))?;
                if migrate_apps_data&&setting.is.system&&!encrypted&&apply_device{
                    let target=if code.is(crate::package::pkg::booleans::DEFAULT_TO_DEVICE_PROTECTED_STORAGE){1}else{2};
                    leaf.call(storage_api::MIGRATE_DATA,|p|{p.write_string16(volume.as_deref());p.write_string16(Some(&setting.name));p.write_i32(user_id);p.write_i32(target);},|_|Ok(()))?;inodes=create()?;
                    record(&inodes);
                    leaf.call(storage_api::PREPARE_CONTENTS,|p|{p.write_string16(volume.as_deref());p.write_string16(Some(&setting.name));p.write_i32(user_id);p.write_i32(flags);p.write_string16(setting.primary_cpu_abi.as_deref());p.write_string16(code.native_library_dir.as_deref());},|_|Ok(()))?;
                }
                Ok(())
                })();
                if let Err(mut error)=prepared {
                    error.message=format!("App-data {}: {}",setting.name,error.message);
                    if let Some(cause)=&mut failure{cause.message.push_str(&format!("; {}",error.message));}
                    else{failure=Some(error);}
                }
            }
            if let Some(error)=failure{Err(error)}else{Ok(())}
            })();
            let result=if receipts.is_empty(){result}else{
                match (result,store.user_states(user_id,receipts.into_iter().collect())) {
                    (result,Ok(_))=>result,
                    (Ok(()),Err(error))=>Err(error),
                    (Err(mut cause),Err(error))=>{cause.message.push_str(&format!("; app-data inode publication: {}",error.message));Err(cause)}
                }
            };
            drop(install);
            store.finish_after_unlock(before,result)?;
        }Ok(deferred_packages)
    }
}
fn storage_allowed(setting:&crate::package::model::PackageState)->bool{
    if setting.pkg.is_none(){return true;}if setting.app_id<0{return false;}
    !setting.pkg.as_ref().unwrap().properties.as_ref().is_some_and(|properties|properties.iter().any(|(name,property)|name=="android.internal.PROPERTY_NO_APP_DATA_STORAGE"&&matches!(property.value,crate::package::pkg::PropertyValue::Bool(true))))
}
fn storage_error(message:impl Into<String>)->Exception{Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}
fn storage_io(message:String)->Exception{let mut parcel=Parcel::new();parcel.write_string16(Some("java.io.IOException"));parcel.write_string16(Some(&message));Exception::parcelable(Some(&message),&parcel).expect("IOException parcel has no capabilities")}
