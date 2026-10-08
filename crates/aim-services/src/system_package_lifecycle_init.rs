//! Native constructor lifecycle binding, before scan forceCurrent and publication.
use crate::{system::System,system_package_lifecycle::{Production,Restored},package::{
    bootstrap::Bridge,owner::{Store,recovery::Report,usage::Usage,app_ids::AppIds},
    settings::{Settings,PackageReadAttempt,ReadOwners},scan::SigningScan,
    scan_snapshot::query_state::{Context,Capture},parse::{Platform,resources::Config},
}};
use aim_android_init::props::{PropertyService,area::AreaMemory};
use aim_binder_host::parcel::{Exception,Parcel,EX_ILLEGAL_STATE};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
use std::{path::Path,sync::{Arc,Mutex}};

/// Its state can only be obtained from the original recovery report while the
/// constructor still owns the restored Settings. Live properties stay bound to
/// init's actual mutable property service, not a copied boot property map.
pub struct BeforeScan<M:AreaMemory+Send+'static>{
    system:Arc<System>,bridge:Arc<Bridge>,production:Production,
    properties:Arc<Mutex<PropertyService<M>>>,
    pub persistence:Arc<Mutex<Store>>,pub recovery:Report,
}
pub struct Published{
    pub lifecycle:Arc<crate::package::lifecycle::Owner>,
    pub capture:Arc<Capture>,pub persistence:Arc<Mutex<Store>>,pub recovery:Report,
}
fn transport(stage:&str,status:i32)->Exception{Exception::new(EX_ILLEGAL_STATE,format!("package lifecycle {stage}: {status}"))}
impl<M:AreaMemory+Send+'static> BeforeScan<M>{
    /// Execute the existing native Settings record dispatcher and bind lifecycle
    /// before the caller advances the scan/version writer. ReadOwners is the
    /// constructor's concrete UID/permission/keyset continuation, not a new
    /// callback-only recovery implementation.
    pub fn recover(system:Arc<System>,bridge:Arc<Bridge>,data:&Path,users:&[u32],
        settings:&mut Settings,ids:&mut AppIds,attempt:&mut PackageReadAttempt,
        owners:&mut impl ReadOwners,properties:Arc<Mutex<PropertyService<M>>>)
        ->Result<Self,Exception>{
        let (store,recovery)=system.recover_owned_package_settings(&bridge,data,users,settings,ids,attempt,owners)
            .map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("native settings recovery: {}",error.message)))?;
        Self::from_recovered(system,bridge,settings,store,recovery,properties)
    }
    pub fn from_recovered(system:Arc<System>,bridge:Arc<Bridge>,settings:&Settings,
        store:Store,recovery:Report,properties:Arc<Mutex<PropertyService<M>>>)
        ->Result<Self,Exception>{
        system.check_package_bootstrap(&bridge)?;
        let restored=Restored::capture(settings,&recovery);
        let mut request=Parcel::new();bootstrap::GetPackageLifecycleLeaf{}.write(&mut request);
        let reply=bridge.owner.transact(bootstrap::GET_PACKAGE_LIFECYCLE_LEAF,&request,false)
            .map_err(|status|transport("CE leaf transport",status))?;
        let mut reader=reply.reader();
        let binder=bootstrap::read_get_package_lifecycle_leaf_reply(&mut reader)
            .map_err(|status|transport("CE leaf reply",status))??
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"original CE lifecycle leaf unavailable"))?;
        if reader.remaining()!=0{return Err(Exception::new(EX_ILLEGAL_STATE,"CE lifecycle leaf reply tail"));}
        let leaf=reply.retain_remote_binder(binder).map_err(|status|transport("CE leaf capability",status))?;
        let live=properties.clone();
        let production=Production::construct(restored,&bridge,leaf,Box::new(move|name|live.lock().unwrap().get(name)))?;
        system.check_package_bootstrap(&bridge)?;
        Ok(Self{system,bridge,production,properties,persistence:Arc::new(Mutex::new(store)),recovery})
    }
    pub fn lifecycle(&self)->&Arc<crate::package::lifecycle::Owner>{&self.production.owner}
    /// Complete scan admission first, then attach the SAME lifecycle to every
    /// public/private query snapshot. Only after the first capture is installed
    /// does the existing System hook publish freezer changes into new versions.
    pub fn publish_initial(self,owner:SigningScan,usage:Usage,context:Context,
        platform:&Platform,config:Config,system_config:&crate::package::system_config::SystemConfig)->Result<Published,Exception>{
        self.system.check_package_bootstrap(&self.bridge)?;
        let context=self.production.attach(context)?;
        let context=self.bridge.resolve_boot_domain_query_context(&owner,context,system_config)
            .map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("initial domain inputs: {error:?}")))?;
        let context=self.bridge.resolve_query_context(&owner,context)
            .map_err(|error|Exception::new(EX_ILLEGAL_STATE,format!("initial query inputs: {error:?}")))?;
        let properties=self.properties.clone();
        let capture=self.system.publish_package_scan_with_configured_roles(&self.bridge,None,owner,usage,context,platform,config,
            &|name|properties.lock().unwrap().get(name),system_config)?;
        if !capture.state().system.lifecycle.as_ref().is_some_and(|owner|Arc::ptr_eq(owner,&self.production.owner)){
            return Err(Exception::new(EX_ILLEGAL_STATE,"initial capture replaced production lifecycle"));
        }
        self.system.install_package_freezer_publication(&self.bridge)?;
        let capture=self.system.capture_package_queries()?;
        self.system.check_package_bootstrap(&self.bridge)?;
        Ok(Published{lifecycle:self.production.owner,capture,persistence:self.persistence,recovery:self.recovery})
    }
}
