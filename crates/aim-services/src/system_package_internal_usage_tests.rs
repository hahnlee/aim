//! Native usage publication concurrency and epoch regression.
use super::*;
#[path = "system_package_internal_isolated_tests.rs"]
mod isolated_tests;
use aim_binder_driver::{Credentials,Device,Driver,Errno,File,GuestProcess,errno,uapi::*};
use aim_binder_host::local::{Call,Reply,Service};
use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap,dev_aim_server_iservicehost as host};
use std::{path::PathBuf,collections::BTreeMap,sync::atomic::{AtomicUsize,Ordering}};
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
            let root=std::env::temp_dir().join(format!("aim-usage-epoch-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
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
    }
impl Drop for Fixture {
    fn drop(&mut self){for original in &self.original{self.driver.release(original.proc_handle());}self.driver.release(self.native.proc_handle());std::fs::remove_dir_all(&self.root).unwrap();}
}
fn publish(fixture:&Fixture,bridge:&Arc<crate::package::bootstrap::Bridge>)->Arc<crate::package::scan_snapshot::query_state::Capture>{
    publish_with_lifecycle(fixture, bridge, None)
}
fn publish_with_lifecycle(fixture:&Fixture,bridge:&Arc<crate::package::bootstrap::Bridge>,lifecycle:Option<Arc<crate::package::lifecycle::Owner>>)->Arc<crate::package::scan_snapshot::query_state::Capture>{
    use crate::package::{scan::{SigningScan,CapturedUsers,ReplicaRuntime,User},settings::{Settings,Package},owner::usage::Usage,
        scan_snapshot::query_state::{Context,PackageInputs,UserInputs},bootstrap::{ApexInventory,ScanUsers},model};
    let mut owner=SigningScan::new(&Default::default(),&Settings{packages:vec![Package{name:"p".into(),app_id:10100,code_path:"/data/app/p".into(),domain_set_id:Some("00000000-0000-0000-0000-000000000001".into()),..Default::default()}],..Default::default()},36).unwrap();
    owner.capture_user_states(BTreeMap::from([(("p".into(),false),CapturedUsers{states:BTreeMap::from([(0,Default::default())]),active_aliases:Default::default()})])).unwrap();
    let shared=owner.identities.shared_users.keys().map(|name|(name.clone(),Default::default())).collect();
    owner.capture_legacy_permissions(&[0],BTreeMap::from([(("p".into(),false),Default::default())]),shared).unwrap();
    owner.capture_install_permissions_fixed(BTreeMap::from([(("p".into(),false),false)])).unwrap();
    owner.rebuild_shared_processes_from_native_members().unwrap();
    owner.capture_replica_runtime(BTreeMap::from([(("p".into(),false),ReplicaRuntime{usage:[0;8],seinfo:None,override_seinfo:None,library_files:vec![],libraries:vec![]})])).unwrap();
    let context=Context{scan_version:1,native_domains:None,boot_classes:None,nonce:Some(17),system:model::System{sdk_sandbox_package:Some(None),lifecycle,..Default::default()},platform:Default::default(),
        users:BTreeMap::from([(0,model::User{id:0,profile_group_id:0,unlocking_or_unlocked:true,..Default::default()})]),apex_inventory:ApexInventory{packages:Some(vec![]),active:vec![]},
        scan_users:ScanUsers{users:Some(vec![User{id:0,pre_created:false,adb_install_disallowed:false}])},cross_user_suspensions:false,
        packages:BTreeMap::from([(("p".into(),false),PackageInputs{app_id:10100,path:"/data/app/p".into(),version:0,installed_permissions:vec![],domain_verification:None,uri_relative_filter_groups:vec![],filter_application_query:false,syncable_authorities:vec![],users:BTreeMap::from([(0,UserInputs{gids:vec![],granted_permissions:vec![],domain_selection:None})])})]),retained_packages:Default::default()};
    fixture.system.publish_package_scan_with_queries(bridge,None,owner,Usage::new(["p"]),context).unwrap()
}
#[test]
fn usage_preparation_releases_coordinator_and_retains_both_concurrent_updates(){
    let mut fixture=Fixture::new();let bridge=fixture.attach();let old=publish(&fixture,&bridge);
    let (ready,preparing)=std::sync::mpsc::channel();let (release,proceed)=std::sync::mpsc::channel();
    let system=fixture.system.clone();let retained=bridge.clone();
    let first=std::thread::spawn(move||{
        let capture={let state=system.package_bootstrap.lock().unwrap();state.current.as_ref().unwrap().queries.clone().unwrap()};
        let mut usage=capture.scan().usage().clone();usage.notify("p",0,111);
        let pending=capture.prepare_usage_update(usage).unwrap();
        ready.send(()).unwrap();proceed.recv().unwrap();
        // Exercise the exact CAS precondition: the intervening second update
        // invalidated pending. Production notifyPackageUse must start from latest.
        let state=system.package_bootstrap.lock().unwrap();assert!(!Arc::ptr_eq(state.current.as_ref().unwrap().queries.as_ref().unwrap(),&capture));drop(state);drop(pending);
        system.check_package_bootstrap(&retained).unwrap();system.internal_notify_package_use(Some("p".into()),0,1000,0).unwrap();
    });
    preparing.recv().unwrap();
    assert!(fixture.system.package_bootstrap.try_lock().is_ok(),"completed preparation must not retain coordinator");
    fixture.system.internal_notify_package_use(Some("p".into()),2,1000,0).unwrap();
    let second=fixture.system.capture_package_queries().unwrap();assert!(second.scan().usage().times("p").unwrap()[2]>0);
    release.send(()).unwrap();first.join().unwrap();
    let current=fixture.system.capture_package_queries().unwrap();let times=current.scan().usage().times("p").unwrap();
    assert!(times[0]>0);assert_eq!(times[2],second.scan().usage().times("p").unwrap()[2]);
    assert_eq!(current.scan().owner().replica_runtime("p",false).unwrap().unwrap().usage,*times);
    assert_eq!(old.scan().usage().times("p").unwrap(),&[0;8]);
}
#[test]
fn prepared_usage_cannot_publish_into_retired_bootstrap_epoch(){
    let mut fixture=Fixture::new();let old_bridge=fixture.attach();let old=publish(&fixture,&old_bridge);
    let mut usage=old.scan().usage().clone();usage.notify("p",0,111);let prepared=old.prepare_usage_update(usage).unwrap();
    let new_bridge=fixture.attach();let replacement=publish(&fixture,&new_bridge);
    assert!(fixture.system.check_package_bootstrap(&old_bridge).is_err());
    assert!(fixture.system.publish_package_scan_with_queries(&old_bridge,Some(old.scan()),prepared.capture.scan().owner().clone(),prepared.capture.scan().usage().clone(),(**prepared.capture.context()).clone()).is_err());
    assert!(Arc::ptr_eq(&replacement,&fixture.system.capture_package_queries().unwrap()));
    assert_eq!(replacement.scan().usage().times("p").unwrap(),&[0;8]);
}

#[test]
fn public_read_dispatch_skips_held_mutation_and_bootstrap_gates(){
    use aim_service_aidl::android_content_pm_ipackagemanager as pm;
    let mut fixture=Fixture::new();let bridge=fixture.attach();let capture=publish(&fixture,&bridge);
    let resolution=capture.resolution().unwrap();
    let install=fixture.system.package_install_guard();
    let publication=fixture.system.package_bootstrap.lock().unwrap();
    let system=fixture.system.clone();let retained=capture.clone();
    let (send,receive)=std::sync::mpsc::channel();
    let read=std::thread::spawn(move||{
        let query=crate::package::query::Query{state:retained.state(),filter:&resolution.apps_filter,calling_uid:1000};
        let mut data=Parcel::new();pm::GetPackageInfo{package_name:Some("p".into()),flags:0,user_id:0}.write(&mut data);
        let mut call=Call{code:pm::GET_PACKAGE_INFO,flags:0,sender_pid:98000,sender_euid:1000,data:aim_binder_host::parcel::Reader::new(data.data(),data.objects())};
        let position=call.data.position();let result=system.dispatch_package_mutation(&mut call,&query,&retained);
        let read_unhandled=result.is_none();
        call.code=u32::MAX;
        let unknown_unhandled=system.dispatch_package_mutation(&mut call,&query,&retained).is_none();
        send.send((read_unhandled&&unknown_unhandled,call.data.position()==position)).unwrap();
    });
    let result=receive.recv_timeout(std::time::Duration::from_secs(2));
    drop(publication);drop(install);read.join().unwrap();
    assert_eq!(result.unwrap(),(true,true),"read dispatch must not wait for either mutation owner gate or consume query arguments");
    assert!(!crate::package::mutation_dispatch::handles(pm::GET_PACKAGE_INFO));
    assert!(!crate::package::mutation_dispatch::handles(u32::MAX));
    for code in [pm::SET_PACKAGE_STOPPED_STATE,pm::SET_COMPONENT_ENABLED_SETTING,pm::SET_COMPONENT_ENABLED_SETTINGS,pm::SET_APPLICATION_ENABLED_SETTING,pm::SET_PACKAGES_SUSPENDED_AS_USER,pm::SET_MIME_GROUP]{
        assert!(crate::package::mutation_dispatch::handles(code),"actual mutation must retain native ownership: {code}");
    }
    assert!(Arc::ptr_eq(&capture,&fixture.system.capture_package_queries().unwrap()));
}

#[test]
fn usage_only_capture_retains_projected_owners_and_coherent_runtime(){
    let mut fixture=Fixture::new();let bridge=fixture.attach();let before=publish(&fixture,&bridge);
    let registry=before.state().package_registry.as_ref().unwrap().clone();
    let before_record=crate::package::scan_snapshot::runtime_record::captured(before.scan(),"p",false).unwrap().unwrap();
    let mut usage=before.scan().usage().clone();usage.notify("p",2,1234);
    let prepared=before.prepare_usage_update(usage).unwrap();let after=&prepared.capture;
    assert_eq!(after.state().generation,after.scan().version());
    assert_eq!(after.context().scan_version,after.scan().version());
    assert_eq!(after.scan().usage().times("p").unwrap()[2],1234);
    assert_eq!(after.scan().owner().replica_runtime("p",false).unwrap().unwrap().usage[2],1234);
    assert!(Arc::ptr_eq(&registry,after.state().package_registry.as_ref().unwrap()),"usage must retain actual projected registry owner instead of rebuilding all code");
    assert_eq!(before.state().packages,after.state().packages);
    assert_eq!(before.state().shared_users,after.state().shared_users);
    assert_eq!(before.state().uid_owners,after.state().uid_owners);
    assert_eq!(before.state().disabled_system_packages,after.state().disabled_system_packages);
    assert_eq!(before.state().system,after.state().system);
    assert_eq!(crate::package::scan_snapshot::runtime_record::captured(before.scan(),"p",false).unwrap().unwrap(),before_record);
    assert_eq!(before.scan().usage().times("p").unwrap()[2],0);
    assert!(!Arc::ptr_eq(before.state(),after.state()));
}

#[test]
fn overlay_rebase_after_lost_publication_cas_keeps_usage_and_rejects_true_conflict(){
    use crate::package::{internal_mutation_record::{Record,Setting,User},internal_mutation_apply::apply_rebased,model::OverlayPaths};
    let mut fixture=Fixture::new();let bridge=fixture.attach();let initial=publish(&fixture,&bridge);
    let mut owner=initial.scan().owner().clone();
    owner.assign_seinfo_at_boot(&crate::package::owner::seinfo::Policy::parse(&[]).unwrap(),&mut |_|panic!("unloaded fixture has no seInfo compatibility request")).unwrap();
    let bound=initial.prepare_internal_mutation(owner,initial.scan().usage().clone()).unwrap();
    let base=fixture.system.publish_package_scan_with_queries(&bridge,Some(initial.scan()),bound.capture.scan().owner().clone(),bound.capture.scan().usage().clone(),(**bound.capture.context()).clone()).unwrap();
    let setting=&base.scan().owner().settings.packages[0];let runtime=base.scan().owner().replica_runtime("p",false).unwrap().unwrap();
    let raw=&base.scan().owner().scanned_user_states("p").unwrap()[&0];
    let overlay=OverlayPaths{resource_dirs:vec!["/data/resource-cache/overlay".into()],overlay_paths:vec![]};
    let record=Record{version:base.scan().version() as i64,active:vec![Setting{name:setting.name.clone(),factory:false,app_id:setting.app_id,private_flags:setting.private_flags,
        category:setting.category_hint,page_flags:setting.page_size_compat,update_available:setting.update_available,loading_progress:setting.loading_progress,loading_completed:setting.loading_completed_time,
        hidden_until_installed:setting.transient.hidden_until_installed,override_seinfo:runtime.override_seinfo.clone(),usage:runtime.usage.to_vec(),installer:setting.install_source.installer.clone(),installer_uid:setting.install_source.installer_uid,
        update_owner:setting.install_source.update_owner.clone(),mime_groups:None,users:vec![User{id:0,installed:raw.installed,uninstall_reason:raw.uninstall_reason,distraction_flags:raw.distraction_flags,hidden:raw.hidden,
            stopped:raw.stopped,not_launched:raw.not_launched,warning:raw.harmful_app_warning.clone(),splash:raw.splash_screen_theme.clone(),min_aspect_ratio:raw.min_aspect_ratio,overlays:Some(overlay.clone()),libraries:None,suspensions:raw.suspensions.clone(),overrides:None}]}],disabled:vec![]};
    let first=apply_rebased(base.scan(),base.scan(),&record).unwrap();
    let stale=base.prepare_internal_mutation(first.scan,first.usage).unwrap();
    fixture.system.internal_notify_package_use(Some("p".into()),2,1000,0).unwrap();
    let latest=fixture.system.capture_package_queries().unwrap();let usage_time=latest.scan().usage().times("p").unwrap()[2];assert!(usage_time>0);
    assert!(fixture.system.publish_package_scan_with_queries(&bridge,Some(base.scan()),stale.capture.scan().owner().clone(),stale.capture.scan().usage().clone(),(**stale.capture.context()).clone()).err().expect("stale publication must reject").message.contains("publication base differs"));
    let rebased=apply_rebased(base.scan(),latest.scan(),&record).unwrap();
    let prepared=latest.prepare_internal_mutation(rebased.scan,rebased.usage).unwrap();
    let current=fixture.system.publish_package_scan_with_queries(&bridge,Some(latest.scan()),prepared.capture.scan().owner().clone(),prepared.capture.scan().usage().clone(),(**prepared.capture.context()).clone()).unwrap();
    assert_eq!(current.scan().usage().times("p").unwrap()[2],usage_time);
    assert_eq!(current.scan().owner().replica_runtime("p",false).unwrap().unwrap().usage[2],usage_time);
    assert_eq!(current.scan().owner().scanned_user_states("p").unwrap()[&0].runtime.overlays(),Some(&overlay));
    assert!(base.scan().owner().scanned_user_states("p").unwrap()[&0].runtime.overlays().is_none());
    let mut owner=current.scan().owner().clone();let mut state=owner.scanned_user_states("p").unwrap()[&0].clone();
    state.runtime.set_overlay_paths(Some(OverlayPaths{resource_dirs:vec!["conflicting".into()],overlay_paths:vec![]}));owner.set_user_state("p",0,state).unwrap();
    let conflict=current.prepare_internal_mutation(owner,current.scan().usage().clone()).unwrap();
    assert!(apply_rebased(base.scan(),conflict.capture.scan(),&record).err().expect("overlapping overlay must reject").contains("overlays"));
}


#[test]
fn canonical_install_survives_old_usage_and_retains_usage_during_query_lag() {
    use crate::package::scan_snapshot::{Store, Error};
    let mut fixture = Fixture::new();
    let bridge = fixture.attach();
    let old = publish(&fixture, &bridge);
    let snapshots = fixture.system.package_bootstrap.lock().unwrap().current.as_ref().unwrap().snapshots.clone().unwrap();
    let mut stale_usage = old.scan().usage().clone();
    stale_usage.notify("p", 0, 111);
    let stale = old.prepare_usage_update(stale_usage).unwrap();
    // The disk owner commits before the query/context projection completes.
    let mut installed_owner = old.scan().owner().clone();
    installed_owner.settings.packages[0].version_code = 7;
    installed_owner.capture_replica_runtime(BTreeMap::from([(("p".into(), false), old.scan().owner().replica_runtime("p", false).unwrap().unwrap().clone())])).unwrap();
    let installed = snapshots.publish_after(old.scan(), installed_owner, old.scan().usage().clone(), |_| Ok(())).unwrap();
    assert_eq!(snapshots.publish_validated_store_after(old.scan(), &stale.store).unwrap_err(), Error::Stale);
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    fixture.system.internal_notify_package_use(Some("p".into()), 2, 1000, 0).unwrap();
    let latest = snapshots.capture();
    assert_eq!(latest.owner().settings.packages[0].version_code, 7);
    let time = latest.usage().times("p").unwrap()[2];
    assert!(time > 0);
    assert_eq!(latest.owner().replica_runtime("p", false).unwrap().unwrap().usage[2], time);
    assert!(Arc::ptr_eq(&fixture.system.capture_package_queries().unwrap(), &old));
    // A following usage publication must also keep the committed code and the
    // first usage reason while the same old query remains readable.
    fixture.system.internal_notify_package_use(Some("p".into()), 0, 1000, 0).unwrap();
    let latest = snapshots.capture();
    assert_eq!(latest.owner().settings.packages[0].version_code, 7);
    assert_eq!(latest.usage().times("p").unwrap()[2], time);
    assert!(latest.usage().times("p").unwrap()[0] > 0);
    assert_eq!(latest.owner().replica_runtime("p", false).unwrap().unwrap().usage, *latest.usage().times("p").unwrap());
    let projected = Store::prepare_usage_store(&latest, latest.usage().clone()).unwrap();
    assert_eq!(projected.capture().owner().settings.packages[0].version_code, 7);
}


#[test]
fn actual_freeze_thaw_during_install_query_lag_keeps_canonical_install() {
    use crate::package::{lifecycle::Owner, settings::{Settings, Version}};
    let mut fixture = Fixture::new();
    let bridge = fixture.attach();
    let lifecycle = Owner::from_settings(&Settings {
        versions: vec![Version { fingerprint: Some("usage-fixture".into()), sdk_version: 36, ..Default::default() }],
        ..Default::default()
    }, true, "usage-fixture", Box::new(|_| None), Box::new(|_| panic!("freezer does not query CE storage"))).unwrap();
    publish_with_lifecycle(&fixture, &bridge, Some(lifecycle.clone()));
    fixture.system.install_package_freezer_publication(&bridge).unwrap();
    let before = fixture.system.capture_package_queries().unwrap();
    let snapshots = fixture.system.package_bootstrap.lock().unwrap().current.as_ref().unwrap().snapshots.clone().unwrap();
    let mut installed_owner = before.scan().owner().clone();
    installed_owner.settings.packages[0].version_code = 7;
    installed_owner.capture_replica_runtime(BTreeMap::from([(("p".into(), false), before.scan().owner().replica_runtime("p", false).unwrap().unwrap().clone())])).unwrap();
    let installed = snapshots.publish_after(before.scan(), installed_owner, before.scan().usage().clone(), |_| Ok(())).unwrap();
    let first = lifecycle.freeze("p".into()).unwrap();
    let one = fixture.system.capture_package_queries().unwrap();
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    assert!(Arc::ptr_eq(one.scan(), before.scan()));
    assert_eq!(one.frozen_packages().unwrap(), &BTreeMap::from([("p".into(), 1)]));
    let second = lifecycle.freeze("p".into()).unwrap();
    let two = fixture.system.capture_package_queries().unwrap();
    assert_eq!(two.frozen_packages().unwrap(), &BTreeMap::from([("p".into(), 2)]));
    assert_eq!(one.frozen_packages().unwrap(), &BTreeMap::from([("p".into(), 1)]));
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    // Actual AMS usage and freezer callbacks interleave while installed code
    // is durable and its native query projection has not yet been published.
    fixture.system.internal_notify_package_use(Some("p".into()), 2, 1000, 0).unwrap();
    let used = snapshots.capture();
    let time = used.usage().times("p").unwrap()[2];
    assert!(time > 0);
    first.close().unwrap();
    assert_eq!(fixture.system.capture_package_queries().unwrap().frozen_packages().unwrap(), &BTreeMap::from([("p".into(), 1)]));
    second.close().unwrap();
    let released = fixture.system.capture_package_queries().unwrap();
    assert!(released.frozen_packages().unwrap().is_empty());
    assert!(lifecycle.frozen_snapshot().unwrap().is_empty());
    assert!(Arc::ptr_eq(released.scan(), before.scan()));
    assert!(Arc::ptr_eq(&snapshots.capture(), &used));
    assert_eq!(used.owner().settings.packages[0].version_code, 7);
    assert_eq!(used.owner().replica_runtime("p", false).unwrap().unwrap().usage[2], time);
    lifecycle.check_frozen_publication().unwrap();
}


#[test]
fn implicit_visibility_publication_preserves_install_and_actual_grant_during_query_lag() {
    let mut fixture = Fixture::new();
    let bridge = fixture.attach();
    let before = publish(&fixture, &bridge);
    let snapshots = fixture.system.package_bootstrap.lock().unwrap().current.as_ref().unwrap().snapshots.clone().unwrap();
    let mut grants = before.state().system.implicit_access.clone();
    assert!(grants.grant(10100, 10101, true));
    let view = before.with_visibility_view(grants.clone()).unwrap();
    let pending = before.prepare_visibility_update(grants.clone()).unwrap();
    // Real canonical disk publication wins after the visibility callback has
    // prepared its candidate. The callback must retain its grant without replay.
    let mut installed_owner = before.scan().owner().clone();
    installed_owner.settings.packages[0].version_code = 7;
    installed_owner.capture_replica_runtime(BTreeMap::from([(("p".into(), false), before.scan().owner().replica_runtime("p", false).unwrap().unwrap().clone())])).unwrap();
    let installed = snapshots.publish_after(before.scan(), installed_owner, before.scan().usage().clone(), |_| Ok(())).unwrap();
    {
        let mut state = fixture.system.package_bootstrap.lock().unwrap();
        fixture.system.publish_internal_visibility_view(&mut state, before.scan(), view, Some(pending)).unwrap();
    }
    let retained = fixture.system.capture_package_queries().unwrap();
    assert!(Arc::ptr_eq(retained.scan(), before.scan()));
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    assert_eq!(retained.state().system.implicit_access, grants);
    assert_eq!(retained.context().system.implicit_access, grants);
    assert_ne!(before.state().system.implicit_access, grants);
    // The post-install context producer copies this retained context; binding it
    // to the genuine new native scan keeps both installed identity and grant.
    let mut context = (**retained.context()).clone();
    context.scan_version = installed.version();
    context.packages.get_mut(&("p".into(), false)).unwrap().version = 7;
    let projected = crate::package::scan_snapshot::query_state::Capture::new(installed.clone(), context).unwrap();
    assert_eq!(projected.scan().owner().settings.packages[0].version_code, 7);
    assert_eq!(projected.state().system.implicit_access, grants);
    // A second genuine grant during existing query lag takes the view-only path.
    let mut next = grants.clone();
    assert!(next.grant(10100, 10102, false));
    let view = retained.with_visibility_view(next.clone()).unwrap();
    {
        let mut state = fixture.system.package_bootstrap.lock().unwrap();
        fixture.system.publish_internal_visibility_view(&mut state, &installed, view, None).unwrap();
    }
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    assert_eq!(fixture.system.capture_package_queries().unwrap().context().system.implicit_access, next);
}


#[test]
fn usage_delta_requires_validated_snapshot_and_keeps_all_other_owner_fields() {
    use crate::package::{scan_snapshot::{Store, Error}, owner::usage::Usage};
    let mut fixture = Fixture::new();
    let bridge = fixture.attach();
    let base = publish(&fixture, &bridge);
    let unsealed = Store::new(base.scan().owner().clone(), base.scan().usage().clone()).unwrap();
    assert!(matches!(Store::prepare_usage_store(&unsealed.capture(), base.scan().usage().clone()), Err(Error::Invalid(_))));
    assert!(matches!(Store::prepare_usage_store(base.scan(), Usage::new(["foreign"])), Err(Error::Invalid(_))));
    let mut usage = base.scan().usage().clone();
    usage.notify("p", 2, 991);
    let prepared = Store::prepare_usage_store(base.scan(), usage.clone()).unwrap();
    let after = prepared.capture();
    assert_eq!(after.version(), base.scan().version() + 1);
    assert_eq!(after.usage(), &usage);
    let mut expected = base.scan().owner().clone();
    let mut runtime = base.scan().owner().replica_runtime("p", false).unwrap().unwrap().clone();
    runtime.usage[2] = 991;
    expected.capture_replica_runtime(BTreeMap::from([(("p".into(), false), runtime)])).unwrap();
    assert_eq!(after.owner(), &expected, "sealed delta must equal the fully validated owner update");
    assert_eq!(after.owner().replica_runtime("p", false).unwrap().unwrap().usage[2], 991);
    assert_eq!(base.scan().usage().times("p").unwrap()[2], 0);
    // Revalidating the result through the general untrusted-owner constructor
    // still passes; this seal never admits a new unchecked package graph.
    Store::new_replica(after.owner().clone(), after.usage().clone()).unwrap();
}


#[test]
fn usage_metadata_revision_and_bounded_batch_preserve_exact_snapshot_contract() {
    use aim_binder_host::{local::Service, parcel::Reader};
    use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
    use crate::package::scan_snapshot::{Store, endpoint::Endpoint};
    let mut fixture=Fixture::new();let bridge=fixture.attach();let base=publish(&fixture,&bridge);
    let mut usage=base.scan().usage().clone();usage.notify("p",2,991);
    let prepared=Store::prepare_usage_store(base.scan(),usage).unwrap();let next=prepared.capture();
    assert_eq!(next.metadata_revision(),base.scan().metadata_revision());
    assert!(next.version()>next.metadata_revision());
    let endpoint=Endpoint::new(next.clone());
    let call=|code,request:&Parcel| endpoint.transact(&mut Call{code,flags:0,sender_pid:97101,sender_euid:1000,data:Reader::new(request.data(),request.objects())}).unwrap();
    let mut request=Parcel::new();api::GetMetadataVersion{}.write(&mut request);
    let reply=call(api::GET_METADATA_VERSION,&request);
    assert_eq!(api::read_get_metadata_version_reply(&mut reply.reader()).unwrap().unwrap(),base.scan().metadata_revision() as i64);
    let mut request=Parcel::new();api::GetUsageRecordsLength{}.write(&mut request);
    let reply=call(api::GET_USAGE_RECORDS_LENGTH,&request);let length=api::read_get_usage_records_length_reply(&mut reply.reader()).unwrap().unwrap();
    assert!(length>20);
    let mut request=Parcel::new();api::GetUsageRecordsChunk{offset:0,length}.write(&mut request);
    let reply=call(api::GET_USAGE_RECORDS_CHUNK,&request);let bytes=api::read_get_usage_records_chunk_reply(&mut reply.reader()).unwrap().unwrap().unwrap();
    let mut reader=Reader::new(&bytes,&[]);
    assert_eq!(reader.read_i64().unwrap(),next.version() as i64);
    assert_eq!(reader.read_i64().unwrap(),next.metadata_revision() as i64);
    assert_eq!(reader.read_i32().unwrap(),1);
    assert_eq!(reader.read_i64().unwrap(),next.version() as i64);
    assert_eq!(reader.read_string16().unwrap().as_deref(),Some("p"));
    assert_eq!(reader.read_bool().unwrap(),next.usage().historical_available());
    assert_eq!(aim_service_aidl::read_long_array(&mut reader).unwrap().unwrap(),vec![0,0,991,0,0,0,0,0]);
    assert_eq!(reader.remaining(),0);
    let mut invalid=Parcel::new();api::GetUsageRecordsChunk{offset:0,length:65537}.write(&mut invalid);
    let reply=call(api::GET_USAGE_RECORDS_CHUNK,&invalid);assert!(api::read_get_usage_records_chunk_reply(&mut reply.reader()).unwrap().is_err());
    // Every ordinary publication changes metadata, even when the caller's
    // current owner happens to compare equal. Only sealed usage inherits it.
    let mut changed=next.owner().clone();changed.settings.packages[0].category_hint=7;
    let generic=prepared.publish(&next,changed,next.usage().clone()).unwrap();
    assert_eq!(generic.metadata_revision(),generic.version());
    assert!(generic.metadata_revision()>next.metadata_revision());
    assert_eq!(base.scan().usage().times("p").unwrap()[2],0);
}


#[test]
fn query_epochs_retain_metadata_only_for_equal_complete_native_owners() {
    use crate::package::scan_snapshot::Store;
    let mut fixture=Fixture::new();let bridge=fixture.attach();let base=publish(&fixture,&bridge);
    let identical=base.prepare_package_update(base.scan().owner().clone()).unwrap();
    assert_eq!(identical.capture.scan().metadata_revision(),base.scan().metadata_revision());
    let mut grants=base.state().system.implicit_access.clone();grants.grant(10100,10101,true);
    let visibility=base.prepare_visibility_update(grants).unwrap();
    assert_eq!(visibility.capture.scan().metadata_revision(),base.scan().metadata_revision());
    assert_ne!(visibility.capture.state().system.implicit_access,base.state().system.implicit_access);
    let version=base.scan().version()+1;
    let changes=|owner| {
        let prepared=Store::new_replica_after(base.scan(),owner,base.scan().usage().clone(),version).unwrap();
        assert_eq!(prepared.capture().metadata_revision(),version);
    };
    let mut owner=base.scan().owner().clone();owner.settings.packages[0].category_hint=7;changes(owner);
    let mut owner=base.scan().owner().clone();let mut user=owner.scanned_user_states("p").unwrap()[&0].clone();
    user.stopped=true;owner.set_user_state("p",0,user).unwrap();changes(owner);
    let mut owner=base.scan().owner().clone();owner.capture_install_permissions_fixed(BTreeMap::from([(("p".into(),false),true)])).unwrap();changes(owner);
    for library in [false,true] {
        let mut owner=base.scan().owner().clone();let mut runtime=owner.replica_runtime("p",false).unwrap().unwrap().clone();
        if library {runtime.library_files=vec![Some("/system/framework/captured.jar".into())];}
        else {runtime.seinfo=Some("captured_label".into());}
        owner.capture_replica_runtime(BTreeMap::from([(("p".into(),false),runtime)])).unwrap();changes(owner);
    }
    // Historical factory runtime cannot be normalized as active PackageUsage.
    let mut owner=base.scan().owner().clone();let mut factory=owner.settings.packages[0].clone();factory.code_path="/system/p".into();
    owner.settings.disabled_system_packages.push(factory);
    let users=crate::package::scan::CapturedUsers{states:owner.scanned_user_states("p").unwrap().clone(),active_aliases:Default::default()};
    owner.capture_user_states(BTreeMap::from([(("p".into(),false),users.clone()),(("p".into(),true),users)])).unwrap();
    let shared=owner.identities.shared_users.keys().map(|name|(name.clone(),Default::default())).collect();
    owner.capture_legacy_permissions(&[0],BTreeMap::from([(("p".into(),false),Default::default()),(("p".into(),true),Default::default())]),shared).unwrap();
    owner.capture_install_permissions_fixed(BTreeMap::from([(("p".into(),false),false),(("p".into(),true),false)])).unwrap();
    let runtime=base.scan().owner().replica_runtime("p",false).unwrap().unwrap().clone();
    owner.capture_replica_runtime(BTreeMap::from([(("p".into(),false),runtime.clone()),(("p".into(),true),runtime.clone())])).unwrap();
    let factory_store=Store::new_replica(owner,base.scan().usage().clone()).unwrap();let factory_base=factory_store.capture();
    // A factory-only user delta conservatively recaptures the complete shared
    // graph; it must never keep a stale retained SharedUser member.
    let mut user_owner=factory_base.owner().clone();
    let active=crate::package::scan::CapturedUsers{states:user_owner.scanned_user_states("p").unwrap().clone(),active_aliases:Default::default()};
    let mut factory_users=user_owner.disabled_user_states("p").unwrap().clone();factory_users.get_mut(&0).unwrap().stopped=true;
    user_owner.capture_user_states(BTreeMap::from([(("p".into(),false),active),(("p".into(),true),crate::package::scan::CapturedUsers{states:factory_users,active_aliases:Default::default()})])).unwrap();
    assert_eq!(factory_base.owner().scoped_user_metadata_changes(&user_owner),Some(vec![("p".into(),true)]));
    let changed_users=Store::new_replica_after(&factory_base,user_owner,factory_base.usage().clone(),factory_base.version()+1).unwrap();
    use aim_binder_host::local::Service;
    use aim_service_aidl::dev_aim_server_ipackagescansnapshot as meta;
    let old=crate::package::scan_snapshot::endpoint::Endpoint::new(factory_base.clone());
    let call=|endpoint:&crate::package::scan_snapshot::endpoint::Endpoint,code,request:&Parcel|endpoint.transact(&mut Call{code,flags:0,sender_pid:97101,sender_euid:1000,data:aim_binder_host::parcel::Reader::new(request.data(),request.objects())}).unwrap();
    let mut request=Parcel::new();meta::GetMetadataComparisonId{}.write(&mut request);
    let reply=call(&old,meta::GET_METADATA_COMPARISON_ID,&request);
    let id=meta::read_get_metadata_comparison_id_reply(&mut reply.reader()).unwrap().unwrap();
    let mut request=Parcel::new();meta::GetChangedUsersForMetadataBase{comparison_id:id}.write(&mut request);
    let current=crate::package::scan_snapshot::endpoint::Endpoint::new(changed_users.capture());
    let reply=call(&current,meta::GET_CHANGED_USERS_FOR_METADATA_BASE,&request);
    assert!(meta::read_get_changed_users_for_metadata_base_reply(&mut reply.reader()).unwrap().unwrap().is_none());
    let mut owner=factory_base.owner().clone();let mut changed=runtime.clone();changed.usage[0]=123;
    owner.capture_replica_runtime(BTreeMap::from([(("p".into(),false),runtime),(("p".into(),true),changed)])).unwrap();
    let changed=Store::new_replica_after(&factory_base,owner,factory_base.usage().clone(),factory_base.version()+1).unwrap().capture();
    assert_eq!(changed.metadata_revision(),changed.version());
    assert_ne!(changed.metadata_revision(),factory_base.metadata_revision());
}


#[test]
fn duplicate_app_data_receipt_keeps_metadata_but_persists_and_publishes_each_commit() {
    use crate::package::{installer::removal::NativeStore, owner::Store as Disk};
    let mut fixture=Fixture::new();let bridge=fixture.attach();let base=publish(&fixture,&bridge);
    let snapshots=fixture.system.package_bootstrap.lock().unwrap().current.as_ref().unwrap().snapshots.clone().unwrap();
    std::fs::create_dir_all(fixture.root.join("data/system/users/0")).unwrap();
    let mut disk=Disk::create(&fixture.root.join("data"),&[0]).unwrap();disk.commit_scan_settings(base.scan()).unwrap();
    let publications=Arc::new(AtomicUsize::new(0));let count=publications.clone();
    let store=NativeStore{snapshots:snapshots.clone(),disk:Arc::new(Mutex::new(disk)),
        publish:Arc::new(move|before,after|{assert_eq!(after.version(),before.version()+1);count.fetch_add(1,Ordering::SeqCst);Ok(())}),
        invalidate:Arc::new(||Ok(()))};
    let mut receipt=base.scan().owner().scanned_user_states("p").unwrap()[&0].clone();receipt.ce_data_inode=991;receipt.de_data_inode=992;
    let first=store.user_state("p",0,receipt.clone()).unwrap();
    assert_eq!(first.metadata_revision(),first.version());
    let path=fixture.root.join("data/system/users/0/package-restrictions.xml");let written=std::fs::read(&path).unwrap();
    let duplicate=store.user_state("p",0,receipt.clone()).unwrap();
    assert_eq!(duplicate.version(),first.version()+1);
    assert_eq!(duplicate.metadata_revision(),first.metadata_revision());
    assert_eq!(duplicate.owner(),first.owner());
    assert_eq!(publications.load(Ordering::SeqCst),2);
    assert_eq!(std::fs::read(&path).unwrap(),written);
    receipt.stopped=!receipt.stopped;
    let stopped=store.user_state("p",0,receipt.clone()).unwrap();
    assert_eq!(stopped.metadata_revision(),stopped.version());
    receipt.ce_data_inode+=1;
    let inode=store.user_state("p",0,receipt.clone()).unwrap();
    assert_eq!(inode.metadata_revision(),inode.version());
    receipt.installed=false;
    let removed=store.user_state("p",0,receipt).unwrap();
    assert_eq!(removed.metadata_revision(),removed.version());
    assert_eq!(publications.load(Ordering::SeqCst),5);
    // A no-op generic persisted commit also keeps metadata while executing its
    // actual writer once; publication generation and CAS are not skipped.
    let writes=AtomicUsize::new(0);
    let same=snapshots.publish_after(&removed,removed.owner().clone(),removed.usage().clone(),|_|{writes.fetch_add(1,Ordering::SeqCst);Ok(())}).unwrap();
    assert_eq!(same.metadata_revision(),removed.metadata_revision());
    assert_eq!(same.version(),removed.version()+1);assert_eq!(writes.load(Ordering::SeqCst),1);
}

#[test]
fn metadata_comparison_capabilities_are_lease_bound_and_strictly_user_scoped() {
    use crate::package::scan_snapshot::{Store,endpoint::Endpoint};
    use aim_binder_host::{local::Service,parcel::Reader};
    use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
    let mut fixture=Fixture::new();let bridge=fixture.attach();let base=publish(&fixture,&bridge);
    let call=|endpoint:&Endpoint,code,request:&Parcel| endpoint.transact(&mut Call{code,flags:0,sender_pid:97101,sender_euid:1000,data:Reader::new(request.data(),request.objects())}).unwrap();
    let baseline=Endpoint::new(base.scan().clone());
    let mut request=Parcel::new();api::GetMetadataComparisonId{}.write(&mut request);
    let reply=call(&baseline,api::GET_METADATA_COMPARISON_ID,&request);
    let id=api::read_get_metadata_comparison_id_reply(&mut reply.reader()).unwrap().unwrap();assert!(id>0);
    let mut owner=base.scan().owner().clone();let mut user=owner.scanned_user_states("p").unwrap()[&0].clone();
    user.enabled_components=Some(vec!["fixture.Component".into()]);owner.set_user_state("p",0,user).unwrap();
    let store=Store::new_replica_after(base.scan(),owner,base.scan().usage().clone(),base.scan().version()+1).unwrap();
    let current=Endpoint::new(store.capture());
    let get=|endpoint:&Endpoint,from| {
        let mut request=Parcel::new();api::GetChangedUsersForMetadataBase{comparison_id:from}.write(&mut request);
        let reply=call(endpoint,api::GET_CHANGED_USERS_FOR_METADATA_BASE,&request);
        api::read_get_changed_users_for_metadata_base_reply(&mut reply.reader()).unwrap().unwrap()
    };
    let bytes=get(&current,id).expect("actual single-package user change supports incremental capture");
    let mut record=Reader::new(&bytes,&[]);
    assert_eq!(record.read_i64().unwrap(),store.capture().version() as i64);
    assert_eq!(record.read_i64().unwrap(),store.capture().metadata_revision() as i64);
    assert_eq!(record.read_i32().unwrap(),1);assert_eq!(record.read_string16().unwrap().as_deref(),Some("p"));
    assert!(!record.read_bool().unwrap());assert_eq!(record.remaining(),0);
    // A real changed metadata owner alongside user state requires the full graph.
    for kind in 0..3 {
        let mut owner=store.capture().owner().clone();
        if kind==0 {owner.settings.packages[0].category_hint=7;}
        else if kind==1 {owner.capture_install_permissions_fixed(BTreeMap::from([(("p".into(),false),true)])).unwrap();}
        else {
            let mut runtime=owner.replica_runtime("p",false).unwrap().unwrap().clone();runtime.library_files=vec![Some("/system/framework/captured.jar".into())];
            owner.capture_replica_runtime(BTreeMap::from([(("p".into(),false),runtime)])).unwrap();
        }
        let changed=Store::new_replica_after(&store.capture(),owner,store.capture().usage().clone(),store.capture().version()+1).unwrap();
        assert!(get(&Endpoint::new(changed.capture()),id).is_none());
    }
    let foreign=Store::new_replica(base.scan().owner().clone(),base.scan().usage().clone()).unwrap();
    assert!(get(&Endpoint::new(foreign.capture()),id).is_none(),"equal fields cannot authorize foreign Store lineage");
    assert!(get(&current,i64::MAX).is_none());
    baseline.close_lease();
    assert!(get(&current,id).is_none(),"closed endpoint capability expires even while its snapshot remains strongly held");
    let released=Endpoint::new(base.scan().clone());
    let reply=call(&released,api::GET_METADATA_COMPARISON_ID,&request);
    let released_id=api::read_get_metadata_comparison_id_reply(&mut reply.reader()).unwrap().unwrap();assert_ne!(released_id,id);
    drop(released);assert!(get(&current,released_id).is_none(),"dropping a lease also expires its comparison ID");
    assert!(base.scan().owner().scanned_user_states("p").unwrap()[&0].enabled_components.is_none());
}

#[test]
fn service_host_lease_pairs_query_scan_across_durable_canonical_publication() {
    use aim_service_aidl::{dev_aim_server_ipackagescansnapshot as scan_api,dev_aim_server_ipackagecomputer as computer_api};
    let mut fixture=Fixture::new();let bridge=fixture.attach();let old=publish(&fixture,&bridge);
    let snapshots=fixture.system.package_bootstrap.lock().unwrap().current.as_ref().unwrap().snapshots.clone().unwrap();
    let mut owner=old.scan().owner().clone();owner.settings.packages[0].category_hint=7;
    let durable=fixture.root.join("data/committed-category");
    let new=snapshots.publish_after(old.scan(),owner,old.scan().usage().clone(),|snapshot|{
        std::fs::write(&durable,snapshot.owner().settings.packages[0].category_hint.to_string()).map_err(|error|crate::package::owner::WriteError{committed:false,message:error.to_string()})
    }).unwrap();
    assert_eq!(std::fs::read_to_string(&durable).unwrap(),"7");
    assert!(Arc::ptr_eq(&fixture.system.capture_package_scan().unwrap(),&new));
    let (paired,query)=fixture.system.capture_package_scan_and_queries().unwrap();
    let query=query.unwrap();assert!(Arc::ptr_eq(&query,&old));assert!(Arc::ptr_eq(&paired,old.scan()));
    assert_eq!(paired.version(),query.scan().version());
    let original=fixture.original[0].clone();
    let capture=||{
        let mut request=Parcel::new();host::CapturePackageScan{}.write(&mut request);
        let reply=original.strong(0).transact(host::CAPTURE_PACKAGE_SCAN,&request,false).unwrap();
        let binder=host::read_capture_package_scan_reply(&mut reply.reader()).unwrap().unwrap().unwrap();
        reply.retain_remote_binder(binder).unwrap()
    };
    let versions=|lease:&Strong|{
        let mut request=Parcel::new();scan_api::GetVersion{}.write(&mut request);
        let reply=lease.transact(scan_api::GET_VERSION,&request,false).unwrap();
        let scan=scan_api::read_get_version_reply(&mut reply.reader()).unwrap().unwrap();
        let mut request=Parcel::new();scan_api::GetComputer{}.write(&mut request);
        let reply=lease.transact(scan_api::GET_COMPUTER,&request,false).unwrap();
        let binder=scan_api::read_get_computer_reply(&mut reply.reader()).unwrap().unwrap().unwrap();
        let computer=reply.retain_remote_binder(binder).unwrap();
        let mut request=Parcel::new();computer_api::GetVersion{}.write(&mut request);
        let reply=computer.transact(computer_api::GET_VERSION,&request,false).unwrap();
        let query=computer_api::read_get_version_reply(&mut reply.reader()).unwrap().unwrap();
        (scan,query)
    };
    let retained=capture();assert_eq!(versions(&retained),(old.scan().version() as i64,old.scan().version() as i64));
    let next=old.prepare_committed_package_snapshot(new.clone()).unwrap();
    fixture.system.package_bootstrap.lock().unwrap().current.as_mut().unwrap().queries=Some(next.clone());
    let (paired,query)=fixture.system.capture_package_scan_and_queries().unwrap();
    assert!(Arc::ptr_eq(&paired,&new));assert!(Arc::ptr_eq(&query.unwrap(),&next));
    let current=capture();assert_eq!(versions(&current),(new.version() as i64,new.version() as i64));
    assert_eq!(versions(&retained),(old.scan().version() as i64,old.scan().version() as i64),"old lease must keep its exact immutable query owner");
    // Initial/raw boot metadata still exposes the real canonical Store when no
    // finalized query owner has been published; it invents no Computer lease.
    fixture.system.package_bootstrap.lock().unwrap().current.as_mut().unwrap().queries=None;
    let (paired,query)=fixture.system.capture_package_scan_and_queries().unwrap();
    assert!(Arc::ptr_eq(&paired,&new));assert!(query.is_none());
}
