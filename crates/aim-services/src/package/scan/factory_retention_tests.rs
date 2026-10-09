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

#[test]
fn current_factory_record_survives_loading_completion_and_real_existing_scan() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Files(std::path::PathBuf);
    impl Drop for Files {fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}
    let files=Files(std::env::temp_dir().join(format!("aim-factory-flow-{}-{}",std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed))));
    std::fs::create_dir(&files.0).unwrap();
    let apk=files.0.join("code.apk");
    let mut empty_zip=vec![0;22];empty_zip[..4].copy_from_slice(b"PK\x05\x06");std::fs::write(&apk,empty_zip).unwrap();
    let mut table=vec![2,0,12,0,40,0,0,0,0,0,0,0,1,0,28,0,28,0,0,0];
    table.extend([0;12]);table.extend(28u32.to_le_bytes());table.extend([0;4]);
    let platform=crate::package::parse::Platform {
        sdk:36,codenames:vec![],sdk_extensions:None,features:Default::default(),
        flags:[("android.content.pm.app_compat_option_16kb".into(),false)].into(),
        flag_packages:Default::default(),split_permissions:vec![],locale:(*b"en",*b"US"),
        use_round_icon:false,recents_limit:16,framework:crate::package::parse::resources::Table::parse(&table).unwrap(),
        framework_overlays:vec![],framework_overlay_apks:vec![],framework_attrs:Default::default(),density_dpi:Some(160),
    };
    let apks=crate::package::write::Apks {signing_overrides:None,files:Box::new(move |_|Some(apk.clone())),platform};
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let mut public_key=vec![0x30,0x59,0x30,0x13,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,
        0x08,0x2a,0x86,0x48,0xce,0x3d,0x03,0x01,0x07,0x03,0x42,0x00];
    public_key.extend_from_slice(p256::SecretKey::from_slice(&[1;32]).unwrap().public_key().to_encoded_point(false).as_bytes());
    let signer=SigningDetails{unknown:false,scheme_version:3,signatures:vec![vec![3]],current_flags:vec![],public_keys:Some(vec![Some(public_key)]),past_signing_certificates:None};
    let policy=crate::package::owner::seinfo::Policy::parse(&[aim_android_xml::read(b"<policy><signer signature='03'><seinfo value='factory'/></signer></policy>").unwrap()]).unwrap();
    let compat=|pkg:&AndroidPackage|Ok(pkg.target_sdk_version);
    let abi=AbiPolicy{all:vec!["arm64-v8a".into()],bit32:vec![],bit64:vec!["arm64-v8a".into()],native32:vec![],native64:vec!["arm64-v8a".into()],force_multi_arch_match:false};
    let env=NativeLibraryEnvironment{preferred_abi:"arm64-v8a",app_lib32_install_dir:"/data/app-lib",code_is_directory:false,canonical_source:None};
    let inputs=||ScanMetadataCompletion{
        scan_as_instant_app:false,seinfo:SeInfoScan{policy:&policy,compatibility:&compat},
        abi_policy:&abi,native_environment:&env,context:AbiScanContext{
            mode:AbiScanMode::Install{moved:None},system:true,updated:false,override_abi:None,platform_runtime_64bit:None},
        install:NativeLibraryInstallPolicy{page_size:4096,extract:false,debuggable:false,compat_16kb_disabled:false,manifest_compat_disabled:false},
        destination:None,clock:ScanClock{current_time:0,user_id:-1,update_time:false},factory_test:false};
    let parsed=AndroidPackage{package_name:"factory.example".into(),path:Some("/product/app/Factory/base.apk".into()),
        base_apk_path:Some("/product/app/Factory/base.apk".into()),version_code:1,target_sdk_version:35,
        booleans:crate::package::pkg::booleans::SYSTEM|crate::package::pkg::booleans::ENABLED,
        booleans2:crate::package::pkg::booleans2::STUB,feature_flag_state:Some(vec![]),
        signing_details:signer.package_details().unwrap(),..Default::default()};
    let code=Code{location:Location{path:parsed.path.clone().unwrap(),partition:Partition::Product,kind:Kind::App,apex:None},parsed,signing:signer};
    let meta=SettingMetadata{code_path:code.location.path.clone(),legacy_native_library_path:None,primary_cpu_abi:None,secondary_cpu_abi:None,
        version_code:1,flags:crate::package::settings::FLAG_SYSTEM,private_flags:0,last_modified_time:0,
        uses_sdk_libraries:vec![],uses_static_libraries:vec![],mime_groups:vec![],domain_set_id:[1;16],target_sdk_version:35,restrict_update_hash:None};
    let users=[User{id:0,pre_created:false,adb_install_disallowed:false}];
    let mut owner=SigningScan::new(&Default::default(),&Default::default(),36).unwrap();
    let admitted=owner.scan_new_system(&code,meta,UserPolicy{install_user:None,users:Some(&users),allow_install:true,instant_app:false,virtual_preload:false,stopped_system_app:false},&apks,inputs()).unwrap();
    assert_eq!(owner.loaded_packages()["factory.example"].package,admitted.candidate.record.parsed);
    assert!(owner.seinfo("factory.example").unwrap().is_some());
    let old_progress=admitted.candidate.record.settings.loading_progress;
    owner.complete_boot_loading(&|_|Ok(false)).unwrap();
    owner.settings.packages[0].category_hint=8;
    assert!(owner.disable_system_package("factory.example").unwrap());
    let current=owner.current_disabled_record(&admitted.candidate.record).unwrap();
    assert_eq!(current.settings,owner.settings.disabled_system_packages[0]);
    assert_eq!((current.settings.loading_progress,current.settings.category_hint),(1.0,8));
    assert_eq!(admitted.candidate.record.settings.loading_progress,old_progress);
    for kind in 0..5 {
        let mut foreign=Record{settings:admitted.candidate.record.settings.clone(),parsed:admitted.candidate.record.parsed.clone(),
            signing:admitted.candidate.record.signing.clone(),identity:admitted.candidate.record.identity.clone(),origin:admitted.candidate.record.origin};
        match kind {0=>foreign.settings.app_id+=1,1=>foreign.settings.code_path.push_str(".foreign"),2=>foreign.settings.version_code+=1,
            3=>foreign.signing.signatures=vec![vec![9]],_=>foreign.parsed.version_code+=1}
        let before=owner.clone();assert!(owner.current_disabled_record(&foreign).is_err());assert_eq!(owner,before);
    }
    let saved=owner.settings.packages[0].clone();
    let mut replacement=Code{location:Location{path:"/data/app/replacement/base.apk".into(),partition:Partition::Data,kind:Kind::App,apex:None},
        parsed:current.parsed.clone(),signing:current.signing.clone()};
    replacement.parsed.path=Some(replacement.location.path.clone());replacement.parsed.base_apk_path=Some(replacement.location.path.clone());
    replacement.parsed.version_code=2;replacement.parsed.booleans2&=!crate::package::pkg::booleans2::STUB;
    let update=SettingUpdate{code_path:replacement.location.path.clone(),legacy_native_library_path:None,primary_cpu_abi:None,secondary_cpu_abi:None,
        flags:crate::package::settings::FLAG_SYSTEM,private_flags:0,uses_sdk_libraries:vec![],uses_static_libraries:vec![],mime_groups:vec![],
        domain_set_id:[2;16],target_sdk_version:35,restrict_update_hash:None};
    let saved_users=[("factory.example".into(),owner.scanned_user_states("factory.example").unwrap().clone())].into();
    let before=owner.clone();
    assert!(owner.scan_existing(&replacement,update.clone(),&saved_users,Some(&users),Some(&admitted.candidate.record),&apks,inputs()).is_err());
    assert_eq!(owner,before);
    let mut completion=inputs();completion.context.mode=AbiScanMode::Existing{first_boot_or_upgrade:true,old_was_stub:true,saved:Some(&saved)};completion.context.updated=true;
    owner.scan_existing(&replacement,update,&saved_users,Some(&users),Some(&current),&apks,completion).unwrap();
    assert_eq!(owner.settings.packages[0].code_path,replacement.location.path);
    assert_eq!(owner.settings.disabled_system_packages[0],current.settings);
    assert_eq!(owner.disabled_loaded_packages()["factory.example"].package,current.parsed);
}
