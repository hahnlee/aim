mod boot_configuration_epoch {
    use super::super::*;
    use aim_binder_driver::{Credentials,Device,Driver,Errno,File,GuestProcess,errno,uapi::*};
    use aim_binder_host::local::{Call,Reply,Service};
    use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap,dev_aim_server_iservicehost as host};
    use std::{path::PathBuf,time::Duration,sync::atomic::{AtomicUsize,Ordering}};

    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self,_:u64,_:&mut[u8])->std::result::Result<(),Errno>{Err(errno::EFAULT)}
        fn copy_to_user(&mut self,_:u64,_:&[u8])->std::result::Result<(),Errno>{Err(errno::EFAULT)}
        fn get_file(&mut self,_:u32)->std::result::Result<File,Errno>{Err(errno::EBADF)}
        fn install_file(&mut self,_:File)->std::result::Result<u32,Errno>{Err(errno::EBADF)}
        fn close_fd(&mut self,_:u32){panic!("epoch fixture does not exchange files")}
    }
    struct OriginalBridge;
    impl Service for OriginalBridge {
        fn descriptor(&self)->&str{bootstrap::DESCRIPTOR}
        fn transact(&self,call:&mut Call<'_>)->Reply{
            call.data.enforce_interface(bootstrap::DESCRIPTOR)?;
            assert_eq!(call.sender_euid,1000);assert_eq!(call.data.remaining(),0);
            let mut reply=Parcel::new();
            match call.code {
                bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH=>bootstrap::write_is_test_base_on_bootclasspath_reply(&mut reply,true),
                bootstrap::IS_SIGNING_DEBUGGABLE=>bootstrap::write_is_signing_debuggable_reply(&mut reply,false),
                _=>return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
            }Ok(reply)
        }
    }
    struct Fixture {
        driver:Arc<Driver>,native:Arc<LocalProcess>,original:Vec<Arc<LocalProcess>>,system:Arc<System>,root:PathBuf,
    }
    impl Fixture {
        fn new()->Self {
            static NEXT:AtomicUsize=AtomicUsize::new(0);
            let root=std::env::temp_dir().join(format!("aim-boot-epoch-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
            std::fs::create_dir(&root).unwrap();std::fs::create_dir(root.join("image")).unwrap();std::fs::create_dir(root.join("data")).unwrap();
            let driver=Driver::new();let native=LocalProcess::open(&driver,Device::Binder,Credentials{pid:97101,euid:1000,security_context:None});
            let system=System::new(native.clone(),&[]);
            let Binder::Local(pointer)=native.add_service(Arc::new(crate::service_host::ServiceHost::new(native.clone(),&system)))else{panic!("local host expected")};
            let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:0,binder:pointer,cookie:pointer}.encode();
            driver.ioctl(native.proc_handle(),97102,BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut NoMemory).unwrap();native.start();
            system.configure_native_package_image(&root.join("image"),&root.join("data"),&[]).unwrap();
            Self{driver,native,original:Vec::new(),system,root}
        }
        fn attach(&mut self)->Arc<crate::package::bootstrap::Bridge>{
            let original=LocalProcess::open(&self.driver,Device::Binder,Credentials{pid:97110+self.original.len() as i32,euid:1000,security_context:None});
            original.start();let node=original.add_service(Arc::new(OriginalBridge));
            let mut request=Parcel::new();host::AttachPackageBootstrapBridge{bridge:Some(node)}.write(&mut request);
            let reply=original.strong(0).transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE,&request,false).unwrap();
            host::read_attach_package_bootstrap_bridge_reply(&mut reply.reader()).unwrap().unwrap();
            self.original.push(original);self.system.package_bootstrap().unwrap()
        }
        fn early(&self,user:i32)->(Arc<crate::package::boot_configuration::Early>,package_boot_inputs::Cli,crate::package::parse::resources::Config,crate::system_package_persistence_init::Inputs){
            let policy=package_boot_inputs::Cli{factory_test:false,install_user:Some(user),allow_install:true,instant_app:false,virtual_preload:false,stopped_system_app:false,compat_16kb_disabled:false,update_time:false,scan_user:user,parser_cache:Some(self.root.join("data/parser-cache"))};
            let resources=crate::package::parse::resources::Config{density:320,..Default::default()};
            let inode=aim_storage::guest_inode::GuestInode{uid:Some(1000),gid:Some(1000),mode:Some(0o660)};
            let persistence=crate::system_package_persistence_init::Inputs{data:self.root.join("data"),original_roots:vec![],original_writer_stopped:true,users:vec![user as u32],session_inode:inode,stage_inode:inode,runtime_inodes:[(user as u32,inode)].into(),controller_version:crate::package::boot_configuration::CONTROLLER_VERSION_DEFERRED};
            let early=crate::package::boot_configuration::epoch_fixture_early(self.system.native_package_image().unwrap(),policy.clone(),resources,persistence.clone());
            (early,policy,resources,persistence)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self){for process in &self.original{self.driver.release(process.proc_handle());}self.driver.release(self.native.proc_handle());std::fs::remove_dir_all(&self.root).unwrap();}
    }

    #[test]
    fn boot_configuration_replacement_and_death_retire_the_exact_epoch(){
        let mut fixture=Fixture::new();let first=fixture.attach();
        let retained=fixture.system.boot_configuration_for(&first).unwrap();
        let (early,policy,resources,persistence)=fixture.early(0);
        fixture.system.configure_native_package_early_for(&first,early.clone(),policy.clone(),resources,persistence.clone()).unwrap();
        assert!(fixture.system.configure_native_package_early_for(&first,early.clone(),policy,resources,persistence).is_err());
        assert!(Arc::ptr_eq(retained.values.lock().unwrap().early.as_ref().unwrap(),&early));
        let old_early=Arc::downgrade(&early);drop(early);
        let second=fixture.attach();let fresh=fixture.system.boot_configuration_for(&second).unwrap();
        assert!(!Arc::ptr_eq(&retained,&fresh));assert!(fixture.system.boot_configuration_for(&first).is_err());
        assert!(fresh.values.lock().unwrap().policy.is_none());assert!(fresh.values.lock().unwrap().early.is_none());
        let (early,policy,resources,persistence)=fixture.early(10);
        assert!(fixture.system.configure_native_package_early_for(&first,early.clone(),policy.clone(),resources,persistence.clone()).is_err());
        fixture.system.configure_native_package_early_for(&second,early.clone(),policy,resources,persistence).unwrap();
        // A retained old Arc may be mutated by an already-running old caller;
        // it must never alias the new epoch's configuration storage.
        retained.values.lock().unwrap().policy.as_mut().unwrap().0.scan_user=99;
        assert_eq!(fresh.values.lock().unwrap().policy.as_ref().unwrap().0.scan_user,10);
        assert!(Arc::ptr_eq(fresh.values.lock().unwrap().early.as_ref().unwrap(),&early));
        fixture.system.detach_package_bootstrap(&first).unwrap();
        assert!(fixture.system.boot_configuration_for(&second).is_ok());
        drop(retained);assert!(old_early.upgrade().is_none());
        fixture.driver.release(fixture.original.last().unwrap().proc_handle());
        // Actual remote Binder death removes the exact current Bridge through
        // System's production death notification, not direct field replacement.
        let deadline=std::time::Instant::now()+Duration::from_secs(3);
        while fixture.system.boot_configuration_for(&second).is_ok(){assert!(std::time::Instant::now()<deadline,"Binder death did not retire boot epoch");std::thread::yield_now();}
        assert!(fixture.system.package_bootstrap().is_err());
        let third=fixture.attach();let after_death=fixture.system.boot_configuration_for(&third).unwrap();
        assert!(after_death.values.lock().unwrap().policy.is_none());assert!(after_death.values.lock().unwrap().early.is_none());
        let (replacement,policy,resources,persistence)=fixture.early(0);
        assert!(fixture.system.configure_native_package_early_for(&second,replacement.clone(),policy.clone(),resources,persistence.clone()).is_err());
        fixture.system.configure_native_package_early_for(&third,replacement,policy,resources,persistence).unwrap();
    }
    fn empty_query_publication(version:u64)->(crate::package::scan::SigningScan,crate::package::owner::usage::Usage,crate::package::scan_snapshot::query_state::Context){
        use crate::package::{bootstrap::{ApexInventory,ScanUsers},model,scan::{SigningScan,User},scan_snapshot::query_state::Context};
        let mut owner=SigningScan::new(&crate::package::system_config::SystemConfig::default(),&crate::package::settings::Settings::default(),36).unwrap();
        let shared=owner.identities.shared_users.keys().map(|name|(name.clone(),Default::default())).collect();
        owner.capture_legacy_permissions(&[0],Default::default(),shared).unwrap();
        let process_order=owner.identities.shared_users.keys().map(|name|(name.clone(),Vec::new())).collect();
        owner.complete_shared_processes(process_order).unwrap();
        let context=Context{scan_version:version,native_domains:None,boot_classes:None,nonce:Some(17),
            system:model::System{sdk_sandbox_package:Some(None),..Default::default()},platform:model::Platform::default(),
            users:[(0,model::User{id:0,profile_group_id:0,unlocking_or_unlocked:true,..Default::default()})].into(),
            apex_inventory:ApexInventory{packages:Some(vec![]),active:vec![]},
            scan_users:ScanUsers{users:Some(vec![User{id:0,pre_created:false,adb_install_disallowed:false}])},
            cross_user_suspensions:false,packages:Default::default(),retained_packages:Default::default()};
        (owner,crate::package::owner::usage::Usage::new(std::iter::empty::<&str>()),context)
    }

    #[test]
    fn initial_publication_rebinds_constructor_version_only_for_new_epoch(){
        let mut fixture=Fixture::new();let first=fixture.attach();
        let (owner,usage,context)=empty_query_publication(1);
        let initial=fixture.system.publish_package_scan_with_queries(&first,None,owner,usage,context).unwrap();
        assert_eq!(initial.scan().version(),1);
        let (owner,usage,context)=empty_query_publication(2);
        let updated=fixture.system.publish_package_scan_with_queries(&first,Some(initial.scan()),owner,usage,context).unwrap();
        assert_eq!(updated.scan().version(),2);
        let second=fixture.attach();
        // A freshly constructed context independently starts at version1. The
        // coordinator retained service-wide version2 across the actual Bridge
        // replacement; exercising publication must assign generation3 itself.
        let (owner,usage,context)=empty_query_publication(1);
        let replaced=fixture.system.publish_package_scan_with_queries(&second,None,owner,usage,context).unwrap();
        assert_eq!(replaced.scan().version(),3);
        assert_eq!(replaced.state().generation,3);
        assert_eq!(replaced.context().nonce,Some(17));
        let (owner,usage,context)=empty_query_publication(1);
        let error=fixture.system.publish_package_scan_with_queries(&first,None,owner,usage,context).err().expect("publication must reject this stale owner");
        assert!(error.message.contains("bootstrap owner changed"));
        let (owner,usage,context)=empty_query_publication(4);
        let error=fixture.system.publish_package_scan_with_queries(&second,Some(updated.scan()),owner,usage,context).err().expect("publication must reject this stale owner");
        assert!(error.message.contains("publication base differs"));
        let (owner,usage,context)=empty_query_publication(1);
        let error=fixture.system.publish_package_scan_with_queries(&second,Some(replaced.scan()),owner,usage,context).err().expect("publication must reject this stale owner");
        assert!(error.message.contains("context scan version differs"));
        assert!(Arc::ptr_eq(fixture.system.capture_package_queries().unwrap().scan(),replaced.scan()));
        let (owner,usage,context)=empty_query_publication(4);
        let next=fixture.system.publish_package_scan_with_queries(&second,Some(replaced.scan()),owner,usage,context).unwrap();
        assert_eq!(next.scan().version(),4);
        let (owner,usage,context)=empty_query_publication(1);
        let error=fixture.system.publish_package_scan_with_queries(&second,None,owner,usage,context).err().expect("publication must reject this stale owner");
        assert!(error.message.contains("publication base differs"));
    }

    #[test]
    fn freezer_publication_keeps_context_and_scan_generation_together(){
        let mut fixture=Fixture::new();let bridge=fixture.attach();
        let lifecycle=crate::package::lifecycle::Owner::from_settings(
            &crate::package::settings::Settings{versions:vec![crate::package::settings::Version{fingerprint:Some("epoch-fixture".into()),sdk_version:36,..Default::default()}],..Default::default()},
            true,"epoch-fixture",Box::new(|_|None),Box::new(|_|panic!("freeze must not query CE storage"))).unwrap();
        let (owner,usage,mut context)=empty_query_publication(1);context.system.lifecycle=Some(lifecycle.clone());
        let initial=fixture.system.publish_package_scan_with_queries(&bridge,None,owner,usage,context).unwrap();
        fixture.system.install_package_freezer_publication(&bridge).unwrap();
        let bound=fixture.system.capture_package_queries().unwrap();
        assert_eq!(bound.scan().version(),initial.scan().version()+1);
        assert_eq!(bound.context().scan_version,bound.scan().version());
        assert!(bound.frozen_packages().unwrap().is_empty());
        let first=lifecycle.freeze("under-admission".into()).unwrap();
        let one=fixture.system.capture_package_queries().unwrap();
        assert_eq!(one.scan().version(),bound.scan().version()+1);
        assert_eq!(one.context().scan_version,one.scan().version());
        assert_eq!(one.frozen_packages().unwrap().get("under-admission"),Some(&1));
        let second=lifecycle.freeze("under-admission".into()).unwrap();
        let two=fixture.system.capture_package_queries().unwrap();
        assert_eq!(two.scan().version(),one.scan().version()+1);
        assert_eq!(two.context().scan_version,two.scan().version());
        assert_eq!(two.frozen_packages().unwrap().get("under-admission"),Some(&2));
        assert_eq!(one.frozen_packages().unwrap().get("under-admission"),Some(&1));
        first.close().unwrap();second.close().unwrap();
        let released=fixture.system.capture_package_queries().unwrap();
        assert_eq!(released.context().scan_version,released.scan().version());
        assert!(released.frozen_packages().unwrap().is_empty());
        assert_eq!(two.frozen_packages().unwrap().get("under-admission"),Some(&2));
        let replacement=fixture.attach();
        assert!(lifecycle.freeze("retired-owner".into()).is_err());
        assert!(fixture.system.check_package_bootstrap(&replacement).is_ok());
        assert!(fixture.system.capture_package_queries().is_err());
    }

    #[test]
    fn bridge_death_closes_registry_retained_session_and_releases_writer_lease(){
        use crate::package::{boot_session::Session,early_user_operations,scan_snapshot::endpoint};
        let mut fixture=Fixture::new();let old=fixture.attach();
        let (owner,usage,context)=empty_query_publication(1);
        let capture=fixture.system.publish_package_scan_with_queries(&old,None,owner,usage,context).unwrap();
        let early=early_user_operations::Endpoint::new(&crate::package::system_config::SystemConfig::default());
        let users=fixture.native.add_service(early.clone());let internal=fixture.system.package_internal_host().unwrap();
        let session=Session::new(&fixture.system,old.clone(),early,users,internal,false);
        let metadata=Arc::new(endpoint::Endpoint::raw_metadata(capture.scan().clone()));
        let metadata_binder=fixture.native.add_service(metadata.clone());
        let prepared=crate::system_package_persistence_init::epoch_fixture_prepared(&fixture.root.join("data")).unwrap();
        let disk=Arc::downgrade(&prepared.disk);
        assert!(crate::system_package_persistence_init::epoch_fixture_prepared(&fixture.root.join("data")).is_err(),"actual writer lease must exclude a second constructor");
        let lifecycle=crate::package::lifecycle::Owner::from_settings(
            &crate::package::settings::Settings{versions:vec![crate::package::settings::Version{fingerprint:Some("epoch-fixture".into()),..Default::default()}],..Default::default()},
            true,"epoch-fixture",Box::new(|_|None),Box::new(|_|panic!("closing session must not query CE storage"))).unwrap();
        std::fs::create_dir(fixture.root.join("data/app")).unwrap();
        let decompression=Arc::new(crate::package::scan::boot_compressed::BootDecompression::new(fixture.root.join("data/app")).unwrap());
        let decompression_weak=Arc::downgrade(&decompression);
        session.epoch_fixture_set_scanned(crate::system::package_boot_scan::Scanned{prepared,capture,runtime_metadata:Default::default(),
            apex_results:vec![],registrations:vec![],lifecycle,boot_apex_changed:false,decompression},&metadata);
        {
            let mut state=fixture.system.package_bootstrap.lock().unwrap();
            state.current.as_mut().unwrap().boot_session=Some(Arc::downgrade(&session));
        }
        let binder=fixture.native.add_service(session.clone());let weak=Arc::downgrade(&session);drop(session);
        // The host registry still owns both Binder endpoints after the remote
        // original process dies. A mere current.take() leaves this writer locked.
        let Binder::Local(pointer)=binder else{panic!("local session endpoint expected")};
        let retained=fixture.native.local_service(pointer).unwrap();
        fixture.driver.release(fixture.original.last().unwrap().proc_handle());
        let deadline=std::time::Instant::now()+Duration::from_secs(3);
        loop {
            let session=weak.upgrade().expect("registry must still retain Session");
            if session.epoch_fixture_is_closed_and_empty()&&disk.upgrade().is_none()&&decompression_weak.upgrade().is_none(){break;}
            assert!(std::time::Instant::now()<deadline,"Bridge death did not complete retained Session resource teardown");std::thread::yield_now();
        }
        assert!(disk.upgrade().is_none());assert!(decompression_weak.upgrade().is_none());
        let next_lease=crate::system_package_persistence_init::epoch_fixture_prepared(&fixture.root.join("data")).expect("death must release actual DataLease/O_EXLOCK so retry can acquire it");
        drop(next_lease);
        let mut request=Parcel::new();aim_service_aidl::dev_aim_server_ipackagebootsession::GetUserOperations{}.write(&mut request);
        let reply=retained.transact(aim_service_aidl::dev_aim_server_ipackagebootsession::GET_USER_OPERATIONS,&request,false).unwrap();
        assert!(reply.reader().read_exception().unwrap().is_err());
        let Binder::Local(meta_pointer)=metadata_binder else{panic!("local metadata endpoint expected")};
        let meta=fixture.native.local_service(meta_pointer).unwrap();
        let mut request=Parcel::new();aim_service_aidl::dev_aim_server_ipackagescansnapshot::GetVersion{}.write(&mut request);
        let reply=meta.transact(aim_service_aidl::dev_aim_server_ipackagescansnapshot::GET_VERSION,&request,false).unwrap();
        assert!(reply.reader().read_exception().unwrap().is_err(),"retained raw metadata endpoint must be revoked");
        let replacement=fixture.attach();
        weak.upgrade().unwrap().close().unwrap();
        assert!(fixture.system.check_package_bootstrap(&replacement).is_ok(),"old Session close must not detach replacement");
    }

}
