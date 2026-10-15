//! FreeStorageHelper's ordered policy over concrete original I/O and native owners.
//! AOSP android-16.0.0_r1, Apache-2.0.
use super::{apps_filter::{AppsFilter,Config},query::Query,scan_snapshot::query_state::Capture};
use aim_binder_host::{local::Strong,parcel::{Exception,Parcel,EX_ILLEGAL_STATE,EX_UNSUPPORTED_OPERATION}};
use aim_service_aidl::dev_aim_server_ipackagemaintenancebridge as api;
use std::{collections::BTreeMap,sync::Arc};

pub type Current=Arc<dyn Fn()->Result<Arc<Capture>,Exception>+Send+Sync>;

pub struct Owner {
    bridge:Arc<Strong>, current:Current, delete:Arc<super::installer::removal::Controller>,
    installer:Arc<super::installer::native::NativeOwners>,
}
#[derive(Debug)]
pub enum Error { Owner(Exception), Io(String) }
impl From<Exception> for Error {fn from(error:Exception)->Self {Self::Owner(error)}}
impl Owner {
    pub fn new(bridge:Arc<Strong>,current:Current,delete:Arc<super::installer::removal::Controller>,installer:Arc<super::installer::native::NativeOwners>)->Arc<Self> {
        Arc::new(Self {bridge,current,delete,installer})
    }
    fn request<T>(&self,code:u32,write:impl FnOnce(&mut Parcel),read:impl FnOnce(&mut aim_binder_host::parcel::Reader<'_>)->aim_binder_host::parcel::Result<Result<T,Exception>>)->Result<T,Error> {
        let mut request=Parcel::new();write(&mut request);
        let reply=self.bridge.transact(code,&request,false).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage owner transport: {status}")))?;
        read(&mut reply.reader()).map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("free-storage owner reply: {status}")))?.map_err(Error::Owner)
    }
    pub fn usable(&self,volume:Option<&str>)->Result<i64,Error> {
        let bytes=self.request(api::STORAGE_SPACE,|p|api::StorageSpace {volume_uuid:volume.map(str::to_owned)}.write(p),api::read_storage_space_reply)?
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"storage-space owner returned null"))?;
        let mut reader=aim_binder_host::parcel::Reader::new(&bytes,&[]);
        let read=|status|Error::Owner(Exception::new(EX_ILLEGAL_STATE,format!("storage-space record: {status}")));
        let success=reader.read_bool().map_err(read)?;
        let available=reader.read_i64().map_err(read)?;
        let error=reader.read_string16().map_err(read)?;
        if reader.remaining()!=0 {return Err(Exception::new(EX_ILLEGAL_STATE,"storage-space record trailing bytes").into());}
        if success {Ok(available)} else {Err(Error::Io(error.ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"storage-space IOException owner absent"))?))}
    }
    fn period(&self,name:&str,default:i64)->Result<i64,Error> {
        self.request(api::CACHE_PERIOD,|p|api::CachePeriod {setting:Some(name.into()),default_value:default}.write(p),api::read_cache_period_reply)
    }
    fn installd(&self,volume:Option<&str>,bytes:i64,v2:bool,defy:bool)->Result<(),Error> {
        self.request(api::FREE_INSTALLD_CACHE,|p|api::FreeInstalldCache {volume_uuid:volume.map(str::to_owned),target_bytes:bytes,v2,defy_quota:defy}.write(p),api::read_free_installd_cache_reply)
    }
    pub fn prune_unused_static_libraries(&self)->Result<(),Error>{
        self.static_libraries(i64::MAX,self.period("unused_static_shared_lib_min_cache_period",7_200_000)?)?;
        Ok(())
    }
    fn static_libraries(&self,needed:i64,max_age:i64)->Result<bool,Error> {
        let capture=(self.current)()?;
        let state=capture.state();
        let filter=AppsFilter::new(state,&Config {force_system_packages_queryable:state.system.force_system_packages_queryable,force_queryable_packages:state.system.force_queryable_packages.clone()})
            .map_err(|error|error.binder_exception().unwrap_or_else(||
                Exception::new(EX_UNSUPPORTED_OPERATION,error.message())))?;
        let query=Query {state,filter:&filter,calling_uid:1000};
        let now=self.request(api::WALL_TIME_MILLIS,|p|api::WallTimeMillis {}.write(p),api::read_wall_time_millis_reply)?;
        let libraries=state.shared_libraries.as_ref().ok_or_else(||Exception::new(EX_UNSUPPORTED_OPERATION,"final native shared-library owner unavailable"))?;
        let mut candidates=Vec::new();
        for library in libraries {
            let name=match library.kind {
                2=>query.resolve_internal_package_name(&library.declaring.0,library.declaring.1),
                3=>library.declaring.0.clone(), _=>continue,
            };
            let Some(package)=state.packages.get(&name) else {continue;};
            if now.wrapping_sub(package.last_update_time)<max_age || package.is.system {continue;}
            let package=package.pkg.as_ref().ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"shared library parsed-code owner unavailable"))?;
            candidates.push((package.package_name.clone(),library.declaring.1));
        }
        for (name,version) in candidates {
            if self.delete.delete_x(&name,version,0,2,true)?==1 && self.usable(None)? >= needed {return Ok(true);}
        }
        Ok(false)
    }
    fn instant(&self,needed:i64,max_age:i64,installed:bool)->Result<bool,Error> {
        let capture=(self.current)()?;
        let state=capture.state();
        let owner=state.system.instant_registry.as_ref().ok_or_else(||Exception::new(EX_UNSUPPORTED_OPERATION,"native instant registry unavailable"))?;
        let users=state.scan_users.as_ref().and_then(|users|users.users.as_ref()).ok_or_else(||Exception::new(EX_UNSUPPORTED_OPERATION,"resolved scan users unavailable"))?
            .iter().map(|user|user.id).collect::<Vec<_>>();
        let usable=||self.usable(None).map_err(|error|format!("{error:?}"));
        if installed {
            let latest=capture.scan().usage().names().map(|name|(name.to_owned(),capture.scan().usage().latest(name).unwrap())).collect::<BTreeMap<_,_>>();
            owner.prune_installed(state,&users,&latest,needed,max_age,&usable,&|name|self.delete.delete_x(name,-1,0,2,true).map(|result|result==1).map_err(|error|error.message))
                .map_err(|message|Error::Owner(Exception::new(EX_ILLEGAL_STATE,message)))
        } else {owner.prune_uninstalled(&users,needed,max_age,&usable).map_err(|message|Error::Owner(Exception::new(EX_ILLEGAL_STATE,message)))}
    }
    pub fn free(&self,volume:Option<&str>,bytes:i64,flags:i32)->Result<(),Error> {
        if self.usable(volume)? >= bytes {return Ok(());}
        let v2=self.request(api::FREE_CACHE_V2,|p|api::FreeCacheV2 {}.write(p),api::read_free_cache_v2_reply)?;
        if v2 {
            let internal=volume.is_none();let aggressive=flags&1!=0;
            let expired=self.request(api::PRELOADS_EXPIRED,|p|api::PreloadsExpired {}.write(p),api::read_preloads_expired_reply)?;
            if internal && (aggressive||expired) {
                self.request(api::DELETE_PRELOADS_FILE_CACHE,|p|api::DeletePreloadsFileCache {}.write(p),api::read_delete_preloads_file_cache_reply)?;
                if self.usable(volume)? >= bytes {return Ok(());}
            }
            if internal && aggressive {
                self.request(api::DELETE_PARSER_CACHE,|p|api::DeleteParserCache {}.write(p),api::read_delete_parser_cache_reply)?;
                if self.usable(volume)? >= bytes {return Ok(());}
            }
            self.installd(volume,bytes,true,false)?;
            if self.usable(volume)? >= bytes {return Ok(());}
            if internal && self.static_libraries(bytes,self.period("unused_static_shared_lib_min_cache_period",7_200_000)?)? {return Ok(());}
            // The pinned implementation has no dexopt-output or DropBox pruning stage.
            if internal && self.instant(bytes,self.period("installed_instant_app_min_cache_period",604_800_000)?,true)? {return Ok(());}
            self.installd(volume,bytes,true,true)?;
            if self.usable(volume)? >= bytes {return Ok(());}
            if internal && self.instant(bytes,self.period("uninstalled_instant_app_min_cache_period",604_800_000)?,false)? {return Ok(());}
            let required=bytes.wrapping_sub(self.usable(volume)?);
            if required>0 {self.request(api::FREE_STORAGE_SERVICE_CACHE,|p|api::FreeStorageServiceCache {volume_uuid:volume.map(str::to_owned),required_bytes:required}.write(p),api::read_free_storage_service_cache_reply)?;}
            self.installer.free_stage_dirs(volume)?;
        } else {self.installd(volume,bytes,false,false)?;}
        if self.usable(volume)? >= bytes {Ok(())} else {Err(Error::Io(format!("Failed to free {bytes} on storage device {:?}",volume)))}
    }
}
impl Owner {
    /// InstantAppRegistry.pruneInstantApps uses the actual Settings.Global
    /// cache periods and Long.MAX_VALUE space target, not the storage-pressure
    /// minimum-age defaults used by FreeStorageHelper.
    pub fn prune_instant_apps(&self)->Result<(),Error>{
        let six_months=6i64*30*24*60*60*1000;
        let installed=self.request(api::CACHE_PERIOD,|p|api::CachePeriod{setting:Some("installed_instant_app_max_cache_period".into()),default_value:six_months}.write(p),api::read_cache_period_reply)?;
        let uninstalled=self.request(api::CACHE_PERIOD,|p|api::CachePeriod{setting:Some("uninstalled_instant_app_max_cache_period".into()),default_value:six_months}.write(p),api::read_cache_period_reply)?;
        if !self.instant(i64::MAX,installed,true)?{self.instant(i64::MAX,uninstalled,false)?;}
        Ok(())
    }
}
