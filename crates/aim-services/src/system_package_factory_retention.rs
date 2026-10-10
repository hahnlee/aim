//! Preserve the admitted factory before disableSystemPackageLPw replaces active code (#873/#862).
use super::*;
use crate::package::scan::{OriginalRuntime,OriginalUserScope,ReplicaRuntime};

pub struct Factory {
    pub runtime:OriginalRuntime,
    pub scope:OriginalUserScope,
    code:Arc<crate::package::scan::LoadedPackage>,
}
pub fn capture(owner:&SigningScan,name:&str,usage:&Usage,bridge:&Arc<Bridge>)->Result<Factory>{
    let policies=owner.loaded_packages().iter().map(|(name,code)|{
        bridge.library_policy(name,code.package.target_sdk_version).map(|policy|(name.clone(),policy))
            .map_err(|error|failure(format!("factory retained library policy: {error:?}")))
    }).collect::<Result<BTreeMap<_,_>>>()?;
    // Resolve the pre-replacement graph without committing effects to active users.
    let graph=owner.resolve_library_dependencies(&|name,_|{
        let policy=policies.get(name).ok_or(crate::package::libraries::ResolveError::Incomplete("factory library policy owner"))?;
        Ok(crate::package::libraries::Policy{enforce_native_dependencies:policy.enforce_native_dependencies,sdk_library_independence:policy.sdk_library_independence})
    }).map_err(|error|failure(format!("factory retained native dependencies: {error:?}")))?;
    let libraries=graph.packages.get(name).ok_or_else(||failure("factory retained dependency record absent"))?;
    capture_native(owner,name,usage,libraries)
}
pub(crate) fn capture_native(owner:&SigningScan,name:&str,usage:&Usage,libraries:&crate::package::libraries::ScanPackage)->Result<Factory>{
    let setting=owner.settings.packages.iter().find(|setting|setting.name==name).ok_or_else(||failure("factory retention setting absent"))?;
    if owner.settings.disabled_system_packages.iter().any(|setting|setting.name==name){return Err(failure("factory retention already disabled"));}
    let code=owner.loaded_packages().get(name).cloned().ok_or_else(||failure("factory retention parsed code absent"))?;
    let users=owner.scanned_user_states(name).ok_or_else(||failure("factory retention user scope absent"))?;
    let labels=owner.seinfo_state(name).map_err(failure)?.ok_or_else(||failure("factory retained seInfo owner absent"))?;
    let runtime=OriginalRuntime{name:name.into(),app_id:setting.app_id,path:setting.code_path.clone(),version:setting.version_code,has_code:true,
        state:ReplicaRuntime{usage:*usage.times(name).ok_or_else(||failure("factory retained usage owner absent"))?,seinfo:labels.base.clone(),override_seinfo:labels.override_label.clone(),
            library_files:libraries.uses_library_files.clone(),libraries:libraries.uses_library_infos.clone()},transient:setting.transient.clone()};
    let scope=OriginalUserScope{name:name.into(),app_id:setting.app_id,path:setting.code_path.clone(),version:setting.version_code,factory:true,
        users:users.keys().copied().collect(),active_aliases:users.keys().copied().collect()};
    Ok(Factory{runtime,scope,code})
}
impl Factory {
    pub fn check_factory(&self,owner:&SigningScan)->std::result::Result<(),String>{
        let setting=owner.settings.disabled_system_packages.iter().find(|setting|setting.name==self.runtime.name).ok_or("retained factory setting missing after replacement")?;
        if setting.app_id!=self.runtime.app_id||setting.code_path!=self.runtime.path||setting.version_code!=self.runtime.version{return Err("retained factory identity changed during replacement".into());}
        let code=owner.disabled_loaded_packages().get(&setting.name).ok_or("retained factory code missing after replacement")?;
        if !Arc::ptr_eq(code,&self.code){return Err("retained factory code is not the pre-decompression owner".into());}
        let users=owner.disabled_user_states(&setting.name).ok_or("retained factory users missing after replacement")?;
        if users.keys().copied().collect::<BTreeSet<_>>()!=self.scope.users{return Err("retained factory user scope changed during replacement".into());}
        Ok(())
    }
}

/// Saved disabled settings are new PackageStateUnserialized owners on readLPw.
/// Their scanned factory code is independent of the later active data package.
pub fn capture_saved_factories(owner:&SigningScan,seinfo:crate::package::scan::SeInfoScan<'_>,bridge:&Arc<Bridge>)
    ->Result<BTreeMap<(String,bool),OriginalRuntime>>{
    capture_saved_factory_inputs(owner,seinfo,&|name,package|{
        let policy=bridge.library_policy(name,package.target_sdk_version).map_err(|error|failure(format!("saved factory library policy: {error:?}")))?;
        Ok(crate::package::libraries::Policy{enforce_native_dependencies:policy.enforce_native_dependencies,sdk_library_independence:policy.sdk_library_independence})
    })
}
pub(crate) fn capture_saved_factory_inputs(owner:&SigningScan,seinfo:crate::package::scan::SeInfoScan<'_>,
    policy:&dyn Fn(&str,&crate::package::pkg::AndroidPackage)->Result<crate::package::libraries::Policy>)
    ->Result<BTreeMap<(String,bool),OriginalRuntime>>{
    use crate::package::{libraries::ScanPackage,owner::seinfo::{Partition,Signing},settings::PRIVATE_FLAG_PRIVILEGED};
    let mut available=BTreeMap::new();
    for (name,code) in owner.loaded_packages(){
        let setting=owner.settings.packages.iter().find(|setting|setting.name==*name).ok_or_else(||failure("saved factory dependency active setting absent"))?;
        let users=owner.scanned_user_states(name).ok_or_else(||failure("saved factory dependency active users absent"))?;
        available.insert(name.clone(),ScanPackage{code:Arc::new(code.runtime_package().clone()),signatures:setting.signatures.clone(),users:users.clone(),uses_library_files:Vec::new(),uses_library_infos:Vec::new()});
    }
    let factory_usage=Usage::new(owner.settings.disabled_system_packages.iter().map(|setting|setting.name.as_str()));
    let mut retained=BTreeMap::new();
    for setting in &owner.settings.disabled_system_packages{
        let code=owner.disabled_loaded_packages().get(&setting.name);
        let users=owner.disabled_user_states(&setting.name).ok_or_else(||failure("saved factory user owner absent"))?;
        // readDisabledSysPackageLPw does not import active usage/override fields.
        let mut state=ReplicaRuntime{usage:*factory_usage.times(&setting.name).ok_or_else(||failure("saved factory usage constructor owner absent"))?,
            seinfo:None,override_seinfo:None,library_files:Vec::new(),libraries:Vec::new()};
        if let Some(code)=code{
            let (target,shared_privileged)=if setting.shared_user{
                let group=owner.identities.shared_users.values().find(|group|Some(group.app_id)==setting.shared_app_id()).ok_or_else(||failure("saved factory seInfo shared owner absent"))?;
                (group.seinfo_target_sdk(),group.private_flags&PRIVATE_FLAG_PRIVILEGED!=0)
            }else{(seinfo.compatibility.target_sdk(code.runtime_package()).map_err(failure)?,false)};
            let partition=[(1<<21,Partition::SystemExt),(1<<19,Partition::Product),(1<<18,Partition::Vendor),(1<<17,Partition::Oem),(1<<30,Partition::Odm)]
                .into_iter().find(|(mask,_)|setting.private_flags&mask!=0).map(|(_,partition)|partition)
                .unwrap_or(if setting.flags&crate::package::settings::FLAG_SYSTEM!=0{Partition::System}else{Partition::Data});
            state.seinfo=Some(seinfo.policy.label(&code.package.package_name,Signing::Known(&code.collected_signing),shared_privileged||setting.private_flags&PRIVATE_FLAG_PRIVILEGED!=0,target,partition));
            let mut inputs=available.clone();
            inputs.insert(setting.name.clone(),ScanPackage{code:Arc::new(code.runtime_package().clone()),signatures:setting.signatures.clone(),users:users.clone(),uses_library_files:Vec::new(),uses_library_infos:Vec::new()});
            let mut policies=BTreeMap::new();
            for (name,package) in &inputs{
                policies.insert(name.clone(),policy(name,&package.code)?);
            }
            let graph=owner.libraries.resolve_scan(&inputs,&|package|policies.get(&package.code.package_name).map(|policy|crate::package::libraries::Policy{enforce_native_dependencies:policy.enforce_native_dependencies,sdk_library_independence:policy.sdk_library_independence}).ok_or(crate::package::libraries::ResolveError::Incomplete("saved factory library policy owner")))
                .map_err(|error|failure(format!("saved factory dependency graph: {error:?}")))?;
            let resolved=graph.packages.get(&setting.name).ok_or_else(||failure("saved factory resolved code absent"))?;
            state.library_files=resolved.uses_library_files.clone();state.libraries=resolved.uses_library_infos.clone();
        }
        retained.insert((setting.name.clone(),true),OriginalRuntime{name:setting.name.clone(),app_id:setting.app_id,path:setting.code_path.clone(),version:setting.version_code,
            has_code:code.is_some(),state,transient:setting.transient.clone()});
    }
    Ok(retained)
}
