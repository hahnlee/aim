use super::*;
use aim_binder_driver::{Credentials,Device,Driver,Errno,File,GuestProcess,errno,uapi::*};
use aim_binder_host::local::{Call,LocalProcess,Reply,Service};
use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap,dev_aim_server_iservicehost as host,android_content_pm_ipackagemanager as pm};
use std::{collections::BTreeMap,sync::{Arc,Mutex}};
struct NoMemory;
impl GuestProcess for NoMemory{
 fn copy_from_user(&mut self,_:u64,_:&mut[u8])->std::result::Result<(),Errno>{Err(errno::EFAULT)}
 fn copy_to_user(&mut self,_:u64,_:&[u8])->std::result::Result<(),Errno>{Err(errno::EFAULT)}
 fn get_file(&mut self,_:u32)->std::result::Result<File,Errno>{Err(errno::EBADF)}
 fn install_file(&mut self,_:File)->std::result::Result<u32,Errno>{Err(errno::EBADF)}
 fn close_fd(&mut self,_:u32){panic!("fixture does not exchange files")}
}
struct Registry{names:Mutex<BTreeMap<String,Arc<Strong>>>,process:std::sync::Weak<LocalProcess>}
impl Service for Registry{
 fn descriptor(&self)->&str{sm::DESCRIPTOR}
 fn transact(&self,call:&mut Call<'_>)->Reply{
  let at=call.data.position();let manager=call.data.enforce_interface(sm::DESCRIPTOR).is_ok();call.data.set_position(at);
  if !manager{return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION)}
  let mut reply=Parcel::new();match call.code{
   sm::ADD_SERVICE=>{let args=sm::AddService::read(&mut call.data)?;let Binder::Handle(handle)=args.service.unwrap()else{panic!("registry owns remote nodes")};self.names.lock().unwrap().insert(args.name.unwrap(),Arc::new(self.process.upgrade().unwrap().strong(handle)));reply.write_no_exception();},
   sm::CHECK_SERVICE=>{let args=sm::CheckService::read(&mut call.data)?;sm::write_check_service_reply(&mut reply,self.names.lock().unwrap().get(&args.name.unwrap()).map(|owner|owner.binder()));},
   _=>return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
  }Ok(reply)
 }
}
struct Original;
impl Service for Original{
 fn descriptor(&self)->&str{bootstrap::DESCRIPTOR}
 fn transact(&self,call:&mut Call<'_>)->Reply{call.data.enforce_interface(bootstrap::DESCRIPTOR)?;let mut reply=Parcel::new();match call.code{
  bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH=>bootstrap::write_is_test_base_on_bootclasspath_reply(&mut reply,true),
  bootstrap::IS_SIGNING_DEBUGGABLE=>bootstrap::write_is_signing_debuggable_reply(&mut reply,false),
  _=>return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
 }Ok(reply)}
}
fn feature(process:&Arc<LocalProcess>,binder:Binder,name:&str)->Result<bool>{
 let mut request=Parcel::new();pm::HasSystemFeature{name:Some(name.into()),version:1}.write(&mut request);
 let reply=match binder{Binder::Local(pointer)=>process.local_service(pointer).unwrap().transact(pm::HAS_SYSTEM_FEATURE,&request,false),Binder::Handle(handle)=>process.strong(handle).transact(pm::HAS_SYSTEM_FEATURE,&request,false)}.unwrap();
 pm::read_has_system_feature_reply(&mut reply.reader()).unwrap()
}
fn publish(system:&Arc<System>,bridge:&Arc<crate::package::bootstrap::Bridge>,name:&str){
 use crate::package::{bootstrap::{ApexInventory,ScanUsers},model,scan::{SigningScan,User},scan_snapshot::query_state::Context};
 let mut owner=SigningScan::new(&Default::default(),&Default::default(),36).unwrap();
 let shared=owner.identities.shared_users.keys().map(|name|(name.clone(),Default::default())).collect();owner.capture_legacy_permissions(&[0],Default::default(),shared).unwrap();
 owner.complete_shared_processes(owner.identities.shared_users.keys().map(|name|(name.clone(),Vec::new())).collect()).unwrap();
 let context=Context{scan_version:1,native_domains:None,boot_classes:None,nonce:Some(17),system:model::System{sdk_sandbox_package:Some(None),features:vec![(name.into(),1)],..Default::default()},platform:Default::default(),users:[(0,model::User{id:0,profile_group_id:0,unlocking_or_unlocked:true,..Default::default()})].into(),apex_inventory:ApexInventory{packages:Some(vec![]),active:vec![]},scan_users:ScanUsers{users:Some(vec![User{id:0,pre_created:false,adb_install_disallowed:false}])},cross_user_suspensions:false,packages:Default::default(),retained_packages:Default::default()};
 system.publish_package_scan_with_queries(bridge,None,owner,crate::package::owner::usage::Usage::new(std::iter::empty::<&str>()),context).unwrap();
}
#[test]
fn original_names_stay_same_binder_across_two_epochs_private_reference_retires(){
 let driver=Driver::new();let native=LocalProcess::open(&driver,Device::Binder,Credentials{pid:98201,euid:1000,security_context:None});let system=System::new(native.clone(),&[]);
 let manager=LocalProcess::open(&driver,Device::Binder,Credentials{pid:98200,euid:1000,security_context:None});
 let registry=Arc::new(Registry{names:Mutex::new(BTreeMap::new()),process:Arc::downgrade(&manager)});
 let Binder::Local(pointer)=manager.add_service(registry.clone())else{panic!("local registry expected")};let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:0,binder:pointer,cookie:pointer}.encode();driver.ioctl(manager.proc_handle(),98202,BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut NoMemory).unwrap();manager.start();native.start();
 let host_node=native.add_service(Arc::new(crate::service_host::ServiceHost::new(native.clone(),&system)));system.add_service("fixture.host",host_node).unwrap();
 let mut originals=Vec::new();
 let mut attach=||{let original=LocalProcess::open(&driver,Device::Binder,Credentials{pid:98210+originals.len() as i32,euid:1000,security_context:None});original.start();let node=original.add_service(Arc::new(Original));let mut request=Parcel::new();host::AttachPackageBootstrapBridge{bridge:Some(node)}.write(&mut request);let mut lookup=Parcel::new();sm::CheckService{name:Some("fixture.host".into())}.write(&mut lookup);let looked=original.strong(0).transact(sm::CHECK_SERVICE,&lookup,false).unwrap();let Some(Binder::Handle(handle))=sm::read_check_service_reply(&mut looked.reader()).unwrap().unwrap()else{panic!("host capability expected")};let reply=original.strong(handle).transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE,&request,false).unwrap();host::read_attach_package_bootstrap_bridge_reply(&mut reply.reader()).unwrap().unwrap();originals.push(original);system.package_bootstrap().unwrap()};
 let first=attach();publish(&system,&first,"epoch.first");let one=system.register_public_package_fronts(&first).unwrap();assert!(feature(&native,one.public,"epoch.first").unwrap());
 let(private,_)=crate::package::service::PackageQueries::from_bootstrap(&system,&first);let private=native.add_service(private);
 let second=attach();publish(&system,&second,"epoch.second");let two=system.register_public_package_fronts(&second).unwrap();assert_eq!(one.public,two.public);assert_eq!(one.native,two.native);
 for(name,binder)in[("package",two.public),("package_native",two.native)]{let mut lookup=Parcel::new();sm::CheckService{name:Some(name.into())}.write(&mut lookup);let reply=native.transact(0,sm::CHECK_SERVICE,&lookup,false).unwrap();assert_eq!(sm::read_check_service_reply(&mut reply.reader()).unwrap().unwrap(),Some(binder));}
 assert!(feature(&native,one.public,"epoch.second").unwrap());assert!(!feature(&native,one.public,"epoch.first").unwrap());assert!(feature(&native,private,"epoch.second").is_err());
 system.detach_package_bootstrap(&second).unwrap();for original in originals{driver.release(original.proc_handle());}driver.release(native.proc_handle());driver.release(manager.proc_handle());
}
