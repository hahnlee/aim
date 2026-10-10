//! Observe actual original creator metadata after UM/app-data creation, then
//! publish it to the same runtime persistence metadata owner and wake its timer.
use super::*;
use aim_binder_host::parcel::EX_ILLEGAL_STATE;
use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackagebootconfigurationleaf as leaf};
impl System {
    pub(crate) fn publish_native_runtime_permission_creation(&self,bridge:&Arc<crate::package::bootstrap::Bridge>,user:i32)->Result<()> {
        if user<0{return Err(Exception::illegal_argument("negative runtime creation user"));}
        self.check_package_bootstrap(bridge)?;
        let guest=format!("/data/misc_de/{user}/apexdata/com.android.permission/runtime-permissions.xml");
        let image=self.native_package_image()?;let path=image.host_path(&guest).ok_or_else(||creation_error("Runtime creation path is outside native image mapping"))?;
        let inode=match std::fs::symlink_metadata(&path){
            Ok(metadata)=>{
                if !metadata.is_file()||metadata.file_type().is_symlink(){return Err(creation_error("Runtime permission file is not an owned regular file"));}
                aim_storage::guest_inode::read(&path).map_err(|error|creation_error(error.to_string()))?
                    .ok_or_else(||creation_error("Existing runtime permission file has no recorded guest inode owner"))?
            },
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{
                let factory={let state=self.package_bootstrap.lock().unwrap();let current=state.current.as_ref().filter(|current|Arc::ptr_eq(&current.bridge,bridge)).ok_or_else(||creation_error("Runtime creation bootstrap retired"))?;
                    let factory=current.boot_configuration.values.lock().unwrap().policy.as_ref().map(|(policy,_)|policy.factory_test)
                        .ok_or_else(||creation_error("Runtime creation boot invocation unavailable"))?;
                    factory};
                let node=self.package_bootstrap_binder_leaf_with(bridge,bootstrap::GET_PACKAGE_BOOT_CONFIGURATION_LEAF,|parcel|parcel.write_bool(factory))?;
                let mut request=Parcel::new();leaf::CreationInode{guest_path:Some(guest)}.write(&mut request);
                let reply=node.transact(leaf::CREATION_INODE,&request,false).map_err(|status|creation_error(format!("Runtime original creation owner: {status}")))?;
                let mut reader=reply.reader();let bytes=leaf::read_creation_inode_reply(&mut reader).map_err(|status|creation_error(format!("Runtime creation reply: {status}")))??
                    .ok_or_else(||creation_error("Runtime original creation owner returned null"))?;
                if reader.remaining()!=0{return Err(creation_error("Runtime creation reply trailing data"));}
                decode_creation_inode(&bytes)?
            },
            Err(error)=>return Err(creation_error(error.to_string())),
        };
        self.check_package_bootstrap(bridge)?;
        let capture=self.capture_package_queries()?;
        if !capture.state().users.contains_key(&user){return Err(creation_error("Runtime creation user is not in the current native UM inventory"));}
        self.with_runtime_permission_metadata(&capture,|state|state.publish_user_creation_inode(user as u32,inode))?
            .map_err(creation_error)
    }
}
fn decode_creation_inode(bytes:&[u8])->Result<aim_storage::guest_inode::GuestInode>{
    let mut reader=Reader::new(bytes,&[]);let fail=|status|creation_error(format!("Runtime creation record: {status}"));
    let uid=reader.read_i32().map_err(fail)?;let gid=reader.read_i32().map_err(fail)?;let mode=reader.read_i32().map_err(fail)?;
    if uid<0||gid<0||!(0..=0o7777).contains(&mode)||reader.remaining()!=0{return Err(creation_error("Runtime original creation inode record is malformed"));}
    Ok(aim_storage::guest_inode::GuestInode{uid:Some(uid as u32),gid:Some(gid as u32),mode:Some(mode as u32)})
}
fn creation_error(message:impl Into<String>)->Exception{Exception::new(EX_ILLEGAL_STATE,message)}
