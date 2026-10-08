//! Focused regression for retaining the factory before compressed boot replacement.
use super::*;
use crate::package::{libraries::ScanPackage,owner::usage::Usage,pkg::AndroidPackage,settings::{Package,Settings},sign::SigningDetails};
use crate::system::package_boot_scan::factory_retention;
use std::{collections::BTreeMap,sync::Arc};

fn fixture()->(SigningScan,Usage,ScanPackage){
    let setting=Package{name:"factory.fixture".into(),code_path:"/product/app/Factory-Stub".into(),app_id:10123,flags:1,version_code:12,..Default::default()};
    let mut owner=SigningScan::new(&Default::default(),&Settings{packages:vec![setting.clone()],..Default::default()},36).unwrap();
    let signing=SigningDetails{unknown:false,scheme_version:3,signatures:vec![vec![3]],current_flags:vec![],public_keys:Some(vec![]),past_signing_certificates:None};
    let parsed=AndroidPackage{package_name:setting.name.clone(),path:Some(setting.code_path.clone()),base_apk_path:Some(format!("{}/base.apk",setting.code_path)),version_code:12,booleans2:crate::package::pkg::booleans2::STUB,feature_flag_state:Some(vec![]),signing_details:Some(signing.parcel_details().unwrap()),..Default::default()};
    owner.loaded.insert(setting.name.clone(),Arc::new(LoadedPackage::new(parsed.clone(),signing).unwrap()));
    let users=[(0,crate::package::restrictions::UserState{enabled:1,..Default::default()}),(17,crate::package::restrictions::UserState{installed:false,..Default::default()})].into_iter().collect::<BTreeMap<_,_>>();
    owner.scanned_users.insert(setting.name.clone(),users.clone());
    owner.capture_user_states([((setting.name.clone(),false),CapturedUsers{states:users.clone(),active_aliases:Default::default()})].into()).unwrap();
    let mut usage=Usage::new([setting.name.as_str()]);usage.notify(&setting.name,2,71);
    let libraries=ScanPackage{code:Arc::new(parsed),signatures:None,users,uses_library_files:vec![Some("/product/framework/factory.jar".into())],uses_library_infos:vec![]};
    (owner,usage,libraries)
}

#[test]
fn compressed_factory_retention_keeps_code_and_sparse_aliases_after_replacement(){
    let (mut owner,usage,libraries)=fixture();
    // Use the real native seInfo capture owner; the fixture is an explicit policy input.
    owner.assign_seinfo_at_boot(&crate::package::owner::seinfo::Policy::unread(),&mut |_|Ok(36)).unwrap();
    let retained=factory_retention::capture_native(&owner,"factory.fixture",&usage,&libraries).unwrap();
    let code=owner.loaded_packages()["factory.fixture"].clone();
    assert!(owner.disable_system_package("factory.fixture").unwrap());
    let old=owner.loaded_packages()["factory.fixture"].clone();
    let mut replacement=old.package.clone();replacement.path=Some("/data/app/expanded/base".into());replacement.version_code=99;replacement.booleans2&=!crate::package::pkg::booleans2::STUB;
    owner.loaded.insert("factory.fixture".into(),Arc::new(LoadedPackage::new(replacement,old.collected_signing.clone()).unwrap()));
    let active=owner.settings.packages.iter_mut().find(|setting|setting.name=="factory.fixture").unwrap();active.code_path="/data/app/expanded/base".into();active.version_code=99;
    let mut changed=owner.scanned_user_states("factory.fixture").unwrap()[&0].clone();changed.enabled=0;
    owner.set_user_state("factory.fixture",0,changed.clone()).unwrap();
    owner.set_user_state("factory.fixture",10,crate::package::restrictions::UserState::default()).unwrap();
    retained.check_factory(&owner).unwrap();
    assert!(Arc::ptr_eq(&code,&owner.disabled_loaded_packages()["factory.fixture"]));
    assert!(!Arc::ptr_eq(&code,&owner.loaded_packages()["factory.fixture"]));
    assert_eq!(retained.runtime.path,"/product/app/Factory-Stub");assert_eq!(retained.runtime.version,12);
    assert_eq!(retained.runtime.state.usage[2],71);assert_eq!(retained.runtime.state.library_files,libraries.uses_library_files);
    assert_eq!(retained.scope.users,[0,17].into());assert_eq!(retained.scope.active_aliases,[0,17].into());
    assert_eq!(owner.disabled_user_aliases("factory.fixture").unwrap(),vec![0,17]);
    assert_eq!(owner.disabled_user_states("factory.fixture").unwrap()[&0],changed);
    assert!(!owner.disabled_user_states("factory.fixture").unwrap().contains_key(&10));
    let mut foreign=owner.clone();foreign.settings.disabled_system_packages[0].code_path="/data/app/expanded/base".into();
    assert!(retained.check_factory(&foreign).is_err());
    let mut foreign=owner.clone();foreign.disabled_loaded.insert("factory.fixture".into(),foreign.loaded_packages()["factory.fixture"].clone());
    assert!(retained.check_factory(&foreign).is_err());
}

#[test]
fn saved_factory_runtime_uses_factory_code_and_refreshes_on_repeat_restore(){
    let (mut owner,active_usage,_)=fixture();
    let name="factory.fixture";
    let mut factory=owner.loaded_packages()[name].package.clone();factory.target_sdk_version=28;factory.uses_libraries=vec!["factory.lib".into()];
    let signer=owner.loaded_packages()[name].collected_signing.clone();
    owner.loaded.insert(name.into(),Arc::new(LoadedPackage::new(factory.clone(),signer.clone()).unwrap()));
    assert!(owner.disable_system_package(name).unwrap());
    let mut active=factory.clone();active.path=Some("/data/app/expanded".into());active.base_apk_path=Some("/data/app/expanded/base.apk".into());active.version_code=99;active.target_sdk_version=36;active.uses_libraries=vec!["active.lib".into()];
    owner.loaded.insert(name.into(),Arc::new(LoadedPackage::new(active,signer.clone()).unwrap()));
    owner.settings.packages[0].code_path="/data/app/expanded".into();owner.settings.packages[0].version_code=99;
    let mut config=crate::package::system_config::SystemConfig::default();
    for (library,path) in [("factory.lib","/product/framework/factory.jar"),("active.lib","/system/framework/active.jar"),("refreshed.lib","/product/framework/refreshed.jar")]{
        config.libraries.insert(library.into(),crate::package::system_config::Library{name:library.into(),filename:path.into(),dependencies:vec![],on_bootclasspath_since:None,on_bootclasspath_before:None,can_be_safely_ignored:false,native:false});
    }
    owner.libraries=crate::package::libraries::Registry::new(&config);
    let policy=crate::package::owner::seinfo::Policy::parse(&[aim_android_xml::read(b"<policy><signer signature='03'><seinfo value='factory'/></signer></policy>").unwrap()]).unwrap();
    let compatibility=|package:&AndroidPackage|Ok(package.target_sdk_version);
    let library_policy=|_:&str,_:&AndroidPackage|Ok(crate::package::libraries::Policy{enforce_native_dependencies:true,sdk_library_independence:false});
    let capture=|owner:&SigningScan|factory_retention::capture_saved_factory_inputs(owner,SeInfoScan{policy:&policy,compatibility:&compatibility},&library_policy).unwrap();
    let first=capture(&owner);let original=&first[&(name.into(),true)];
    assert_eq!((original.app_id,original.path.as_str(),original.version,original.has_code),(10123,"/product/app/Factory-Stub",12,true));
    assert_eq!(original.state.library_files,vec![Some("/product/framework/factory.jar".into())]);
    assert!(original.state.seinfo.as_deref().unwrap().contains("targetSdkVersion=28"));
    assert_eq!(original.state.usage,[0;crate::package::owner::usage::REASONS]);
    assert_ne!(&original.state.usage,active_usage.times(name).unwrap());
    assert_eq!(original.state.override_seinfo,None);
    assert_eq!(capture(&owner),first);
    let mut refreshed=factory;refreshed.version_code=13;refreshed.target_sdk_version=29;refreshed.uses_libraries=vec!["refreshed.lib".into()];
    owner.settings.disabled_system_packages[0].version_code=13;
    let setting=owner.settings.disabled_system_packages[0].clone();
    owner.disabled_loaded.insert(name.into(),Arc::new(LoadedPackage::new(refreshed,signer).unwrap().bind_disabled(&setting)));
    let next=capture(&owner);let runtime=&next[&(name.into(),true)];
    assert_eq!(runtime.version,13);assert_eq!(runtime.state.library_files,vec![Some("/product/framework/refreshed.jar".into())]);
    assert!(runtime.state.seinfo.as_deref().unwrap().contains("targetSdkVersion=29"));
    assert_eq!(first[&(name.into(),true)].version,12);
}
