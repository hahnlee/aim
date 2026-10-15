//! Original PMS settings phase: no provider read while native scanning.
use aim_binder_host::{local::Strong,parcel::{Exception,Parcel,EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_ipackagebootcontextleaf as api;
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
pub struct Owner {owner:Arc<Strong>,filtering:AtomicBool}
impl Owner {
    pub fn new(owner:Arc<Strong>,initial_disabled:bool)->Arc<Self> {Arc::new(Self {owner,filtering:AtomicBool::new(initial_disabled)})}
    fn call(&self,code:u32,write:impl FnOnce(&mut Parcel),read:impl FnOnce(&mut aim_binder_host::parcel::Reader<'_>)->aim_binder_host::parcel::Result<Result<bool,Exception>>)->Result<bool,Exception> {
        let mut request=Parcel::new();write(&mut request);let reply=self.owner.transact(code,&request,false).map_err(error)?;
        read(&mut reply.reader()).map_err(error)?
    }
    pub fn cached_filtering_disabled(&self)->bool {self.filtering.load(Ordering::Acquire)}
    pub fn register_updates(self:&Arc<Self>,process:&Arc<aim_binder_host::local::LocalProcess>)->Result<(),Exception> {
        let callback=process.add_service(Arc::new(Updates {owner:Arc::downgrade(self)}));
        let mut request=Parcel::new();api::RegisterFilteringUpdates {owner:Some(callback)}.write(&mut request);
        let reply=self.owner.transact(api::REGISTER_FILTERING_UPDATES,&request,false).map_err(error)?;
        api::read_register_filtering_updates_reply(&mut reply.reader()).map_err(error)?
    }
    pub fn unlocked(&self,user:i32)->Result<bool,Exception> {
        self.call(api::USER_UNLOCKING_OR_UNLOCKED,|p|api::UserUnlockingOrUnlocked {user_id:user}.write(p),api::read_user_unlocking_or_unlocked_reply)
    }
    pub fn provisioned(&self)->Result<bool,Exception> {self.call(api::DEVICE_PROVISIONED,|p|api::DeviceProvisioned {}.write(p),api::read_device_provisioned_reply)}
    pub fn filtering_disabled(&self)->Result<bool,Exception> {self.call(api::QUERY_FILTERING_DISABLED,|p|api::QueryFilteringDisabled {}.write(p),api::read_query_filtering_disabled_reply)}
    /// Root publishes returned compatibility into Context.system and updates
    /// parsing policy at the existing actual SystemReady lifecycle point.
    pub fn begin_system_ready(&self)->Result<(),Exception> {
        let mut request=Parcel::new();api::BeginSystemReady {}.write(&mut request);
        let reply=self.owner.transact(api::BEGIN_SYSTEM_READY,&request,false).map_err(error)?;
        api::read_begin_system_ready_reply(&mut reply.reader()).map_err(error)?
    }
    pub fn system_ready(&self)->Result<bool,Exception> {self.call(api::ENTER_SYSTEM_READY,|p|api::EnterSystemReady {}.write(p),api::read_enter_system_ready_reply)}
}
fn error(status:i32)->Exception {Exception::new(EX_ILLEGAL_STATE,format!("original settings phase owner: {status}"))}
impl std::fmt::Debug for Owner {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.debug_struct("PmsSettingsPhase").finish_non_exhaustive()}}
impl PartialEq for Owner {fn eq(&self,other:&Self)->bool {std::ptr::eq(self,other)}}
impl super::Resolution {
    pub(super) fn device_provisioned(&self)->super::Result<bool> {
        match &self.state.platform.settings_owner {Some(owner)=>owner.provisioned().map_err(super::ResolutionError::Original),None=>Ok(self.state.platform.device_provisioned)}
    }
}

struct Updates {owner:std::sync::Weak<Owner>}
impl aim_binder_host::local::Service for Updates {
    fn descriptor(&self)->&str {aim_service_aidl::dev_aim_server_ipackagefilteringupdates::DESCRIPTOR}
    fn transact(&self,call:&mut aim_binder_host::local::Call<'_>)->aim_binder_host::local::Reply {
        use aim_service_aidl::dev_aim_server_ipackagefilteringupdates as updates;
        if call.sender_euid!=1000 {return Err(aim_binder_host::parcel::PERMISSION_DENIED);}
        if call.code!=updates::CHANGED {return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);}
        let args=updates::Changed::read(&mut call.data)?;
        if let Some(owner)=self.owner.upgrade() {owner.filtering.store(args.disabled,Ordering::Release);}
        Ok(Parcel::new())
    }
}
