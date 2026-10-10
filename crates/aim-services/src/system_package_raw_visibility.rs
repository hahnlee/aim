//! Private pre-permission visibility index, never a publishable query capture.
use super::*;
use crate::package::{apps_filter::{self,AppsFilter},parse::resources::{Config,Resources,Selected,TYPE_REFERENCE,TYPE_FIRST_INT,TYPE_LAST_INT},pkg::booleans};

pub struct Visibility { state:model::State, filter:AppsFilter }
impl Visibility {
    pub(super) fn metadata_state(&self)->&model::State{&self.state}
    pub fn new(owner:&SigningScan,original:&crate::package::scan_snapshot::boot_context::Original,
        system:&model::System,platform:&crate::package::parse::Platform,config:Config,sandbox:&str,bridge:&Arc<Bridge>)->Result<Self>{
        let resources=Resources{tables:vec![&platform.framework],overlays:&platform.framework_overlays,config};
        let boolean=platform.framework.id("bool","config_forceSystemPackagesQueryable").ok_or_else(||failure("raw AppsFilter boolean resource unavailable"))?;
        let mut selected=Selected{kind:TYPE_REFERENCE,data:boolean,table:None,resid:0,flags:0};resources.resolve(&mut selected);
        if !(TYPE_FIRST_INT..=TYPE_LAST_INT).contains(&selected.kind){return Err(failure("raw AppsFilter resource not boolean"));}
        let array=platform.framework.id("array","config_forceQueryablePackages").ok_or_else(||failure("raw AppsFilter package resource unavailable"))?;
        let forced=resources.string_array(array).ok_or_else(||failure("raw AppsFilter package array unavailable"))?.into_iter()
            .map(|value|value.ok_or_else(||failure("raw AppsFilter package name null"))).collect::<Result<Vec<_>>>()?;
        let mut state=model::State{system:system.clone(),platform:original.resolver.clone(),..model::State::default()};
        state.system.sdk_sandbox_package=Some(Some(sandbox.into()));
        let mut slots=BTreeMap::new();
        for record in &original.users{
            state.users.insert(record.id,model::User{id:record.id,flags:record.flags,profile_group_id:record.profile_group_id,
                unlocking_or_unlocked:record.unlocking_or_unlocked,preferred_activities:record.preferred_activities.clone(),restrictions:record.restrictions.clone(),default_browser:record.default_browser.clone()});
        }
        for setting in &owner.settings.packages{
            let stored=owner.scanned_user_states(&setting.name).ok_or_else(||failure("raw package user-state owner absent"))?;
            let code=owner.loaded_packages().get(&setting.name);
            let mut users=BTreeMap::new();
            for id in state.users.keys(){
                // PackageUserStateDefault is the original sparse Settings rule.
                // Permission vectors are deliberately never read by this index.
                let default=crate::package::restrictions::UserState::default();let user=stored.get(id).unwrap_or(&default);
                users.insert(*id,model::PackageUserState{installed:user.installed,hidden:user.hidden,instant_app:user.instant_app,
                    enabled:user.enabled,enabled_components:user.enabled_components.clone().unwrap_or_default(),disabled_components:user.disabled_components.clone().unwrap_or_default(),archive_state:user.archive_state.clone(),..model::PackageUserState::default()});
            }
            let shared=setting.shared_app_id().and_then(|id|owner.identities.shared_users.iter().find(|(_,group)|group.app_id==id).map(|(name,_)|name.clone()));
            let query=bridge.application_query_filtering(&setting.name,setting.target_sdk_version).map_err(|error|failure(format!("raw AppsFilter compatibility: {error:?}")))?;
            let package=model::PackageState{name:setting.name.clone(),app_id:setting.app_id,shared_user:shared.clone(),shared_user_app_id:setting.shared_app_id(),
                path:setting.code_path.clone(),target_sdk_version:setting.target_sdk_version,signatures:setting.signatures.clone(),filter_application_query:Some(query),
                is:model::StateFlags{system:setting.flags&crate::package::settings::FLAG_SYSTEM!=0,force_queryable_override:setting.force_queryable,hidden_until_installed:setting.transient.hidden_until_installed,..model::StateFlags::default()},
                pkg:code.map(|code|Arc::new(code.runtime_package().clone())),users,..model::PackageState::default()};
            slots.insert(setting.uid_owner_id(),match shared{Some(name)=>model::UidOwner::SharedUser(name),None=>model::UidOwner::Package(Box::new(package.clone()))});
            state.packages.insert(setting.name.clone(),package);
        }
        for (name,group) in &owner.identities.shared_users{
            let packages=state.packages.values().filter(|package|package.shared_user.as_deref()==Some(name.as_str())).cloned().collect::<Vec<_>>();
            state.shared_users.insert(name.clone(),model::SharedUser{name:name.clone(),app_id:group.app_id,
                packages:packages.iter().map(|package|package.name.clone()).collect(),native_packages:Some(packages),..model::SharedUser::default()});
            slots.insert(group.app_id,model::UidOwner::SharedUser(name.clone()));
        }
        // Use registered AppIdSettingMap slots, including detached retained owners.
        slots.clear();
        for (id,slot) in owner.identities.ids.owners(){
            use crate::package::owner::app_ids::Owner;
            let value=match slot{
                Owner::Package(name)=>model::UidOwner::Package(Box::new(state.packages.get(name).cloned().ok_or_else(||failure("raw registered package absent"))?)),
                Owner::SharedUser(name)=>{if !state.shared_users.contains_key(name){return Err(failure("raw registered shared group absent"));}model::UidOwner::SharedUser(name.clone())},
                Owner::DetachedPackage(_)=>{
                    let retained=owner.identities.ids.detached_setting(id).ok_or_else(||failure("raw detached UID owner absent"))?;
                    let users=state.users.keys().map(|user|{
                        let default=crate::package::restrictions::UserState::default();let source=retained.users.get(user).unwrap_or(&default);
                        (*user,model::PackageUserState{installed:source.installed,hidden:source.hidden,instant_app:source.instant_app,archive_state:source.archive_state.clone(),..model::PackageUserState::default()})
                    }).collect();
                    model::UidOwner::Package(Box::new(model::PackageState{name:retained.package.name.clone(),app_id:retained.package.app_id,path:retained.package.code_path.clone(),
                        target_sdk_version:retained.package.target_sdk_version,users,..model::PackageState::default()}))
                },
            };slots.insert(id,value);
        }
        state.uid_owners=Some(slots);
        let filter=AppsFilter::new(&state,&apps_filter::Config{force_system_packages_queryable:selected.data!=0,force_queryable_packages:forced})
            .map_err(|error|failure(format!("raw AppsFilter component owner: {error:?}")))?;
        Ok(Self{state,filter})
    }
    pub fn instant(&self,uid:i32)->Result<Option<String>>{
        apps_filter::instant_app_package_name(&self.state,uid).map(|name|name.map(str::to_owned)).map_err(|error|failure(format!("raw instant identity: {error:?}")))
    }
    pub fn filter_package(&self,name:Option<&str>,mut uid:i32,user:i32,filter_uninstalled:bool)->Result<bool>{
        if apps_filter::is_isolated(uid){uid=self.state.system.isolated_owners.iter().find(|(isolated,_)|*isolated==uid).map(|(_,owner)|*owner).ok_or_else(||failure("raw isolated UID owner absent"))?;}
        let instant=self.instant(uid)?.is_some();
        let Some(package)=name.and_then(|name|self.state.packages.get(name))else{return Ok(instant||filter_uninstalled)};
        let default=model::PackageUserState::default();let target=package.users.get(&user).unwrap_or(&default);
        if apps_filter::is_sdk_sandbox(uid)&&apps_filter::uid(user,package.app_id)==uid-10000{return Ok(false);}
        if filter_uninstalled&&!apps_filter::is_system_or_root_or_shell(uid)&&!package.is.hidden_until_installed&&!target.installed{return Ok(true);}
        if apps_filter::is_caller_same_app(&self.state,Some(&package.name),uid).map_err(|error|failure(format!("raw same-app identity: {error:?}")))?{return Ok(false);}
        if instant{
            if target.instant_app{return Ok(true);}
            return package.pkg.as_ref().map(|package|package.booleans&booleans::VISIBLE_TO_INSTANT_APPS==0).ok_or_else(||failure("raw instant target parsed code absent"));
        }
        if target.instant_app{
            // Original canViewInstantApps grants core UIDs before checking permissions.
            if uid<10000{return Ok(false);}
            return Err(failure("initial visibility cannot read live instant permission grants before permission owner attachment"));
        }
        Ok(self.filter.should_filter(&self.state,uid,package,user))
    }
    pub fn filter_uid(&self,target_uid:i32,calling_uid:i32)->Result<bool>{
        let user=apps_filter::user_id(target_uid);
        let names=match apps_filter::setting(&self.state,apps_filter::app_id(target_uid)){
            Some(apps_filter::Setting::Package(package))=>vec![package.name.as_str()],
            Some(apps_filter::Setting::Shared(group))=>group.packages.iter().map(String::as_str).collect(),None=>return self.filter_package(None,calling_uid,user,false),
        };
        for name in names{if !self.filter_package(Some(name),calling_uid,user,false)?{return Ok(false);}}
        Ok(true)
    }
}
