//! Concrete original installd/UM/storage producer for installation Environment.
use super::environment::{AppDataCreate,AppDataFlags,AppDataRollback,AppDataResult,AppDataCommit};
use aim_binder_host::{local::Strong,parcel::{Exception,Parcel,Reader,EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_ipackageappdatabridge as api;
use std::sync::Arc;
pub struct Owner { bridge:Arc<Strong> }
impl Owner {
    pub fn new(bridge:Strong)->Arc<Self> {Arc::new(Self {bridge:Arc::new(bridge)})}
    fn call<T>(&self,code:u32,write:impl FnOnce(&mut Parcel),read:impl FnOnce(&mut Reader<'_>)->aim_binder_host::parcel::Result<Result<T,Exception>>)->Result<T,Exception> {
        let mut request=Parcel::new();write(&mut request);
        let reply=self.bridge.transact(code,&request,false).map_err(transport)?;
        let mut reader=reply.reader();let value=read(&mut reader).map_err(transport)??;
        if reader.remaining()!=0 {return Err(Exception::new(EX_ILLEGAL_STATE,"app-data owner reply trailing bytes"));}
        Ok(value)
    }
    pub fn flags(&self,user:i32)->Result<i32,Exception> {
        self.call(api::APP_DATA_FLAGS,|p|api::AppDataFlags {user_id:user}.write(p),api::read_app_data_flags_reply)
    }
    pub fn create(&self,request:crate::package::users::AppData)->Result<AppDataResult,Exception> {
        let bytes=self.call(api::CREATE_APP_DATA,|p|api::CreateAppData {volume_uuid:request.volume_uuid,package_name:Some(request.package),user_id:request.user,flags:request.flags,app_id:request.app_id,se_info:request.seinfo,target_sdk_version:request.target_sdk_version}.write(p),api::read_create_app_data_reply)?
            .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"app-data result is null"))?;
        let mut reader=Reader::new(&bytes,&[]);
        let result=AppDataResult {ce_inode:reader.read_i64().map_err(transport)?,de_inode:reader.read_i64().map_err(transport)?,newly_created:reader.read_bool().map_err(transport)?};
        if reader.remaining()!=0 {return Err(Exception::new(EX_ILLEGAL_STATE,"app-data result trailing bytes"));}
        Ok(result)
    }
    pub fn rollback(&self,name:&str,user:i32,ce:i64)->Result<(),Exception> {
        self.call(api::ROLLBACK_APP_DATA,|p|api::RollbackAppData {package_name:Some(name.into()),user_id:user,ce_data_inode:ce}.write(p),api::read_rollback_app_data_reply)
    }
    pub fn commit(&self,name:&str,user:i32,ce:i64)->Result<(),Exception> {
        self.call(api::COMMIT_APP_DATA,|p|api::CommitAppData {package_name:Some(name.into()),user_id:user,ce_data_inode:ce}.write(p),api::read_commit_app_data_reply)
    }
    pub fn callbacks(self:&Arc<Self>)->(AppDataCreate,AppDataFlags,AppDataRollback,AppDataCommit) {
        let create=self.clone();let flags=self.clone();let rollback=self.clone();let commit=self.clone();
        (Arc::new(move|request|create.create(request)),Arc::new(move|user|flags.flags(user)),
            Arc::new(move|name,user,inode|rollback.rollback(name,user,inode)),Arc::new(move|name,user,inode|commit.commit(name,user,inode)))
    }
}
fn transport(status:i32)->Exception {Exception::new(EX_ILLEGAL_STATE,format!("app-data owner transport: {status}"))}
