//! Candidate query owners for native installation permission transactions.
//! Per-package identities are rebuilt from the candidate; globals retain their owners.
use super::{Snapshot,query_state::{Capture,Context,PackageInputs,UserInputs}};
use crate::package::{bootstrap::Bridge,settings,restrictions,domain_verification::owner as domain,
    owner::{legacy_permissions,app_ids::Owner as UidOwner},system_config::SystemConfig};
use std::{collections::{BTreeMap,BTreeSet},sync::Arc};

pub type Current=Arc<dyn Fn()->Result<Arc<Capture>,String>+Send+Sync>;
/// Permit a retained read across actual usage publication only. All settings,
/// code, UID, user, signing, library and permission ownership remain compared.
pub(crate) fn same_install_identity(before:&Snapshot,after:&Snapshot)->Result<bool,String>{
    if before.owner()==after.owner()&&before.usage()==after.usage(){return Ok(true);}
    if before.usage().names().ne(after.usage().names()){return Ok(false);}
    let mut normalized=before.owner().clone();
    normalized.update_replica_usage(before.usage(),after.usage())?;
    Ok(&normalized==after.owner())
}
/// Installation owns the changed code/settings and every package sharing its
/// old/new appId. Other packages may receive native user-state publications.
pub(crate) fn install_targets(base:&Snapshot,candidate:&crate::package::scan::SigningScan)->BTreeSet<String>{
    let old=base.owner();
    let mut names=BTreeSet::new();
    for setting in old.settings.packages.iter().chain(candidate.settings.packages.iter()){
        let a=old.settings.packages.iter().find(|p|p.name==setting.name);
        let b=candidate.settings.packages.iter().find(|p|p.name==setting.name);
        if a!=b||old.loaded_packages().get(&setting.name)!=candidate.loaded_packages().get(&setting.name){names.insert(setting.name.clone());}
    }
    names
}
/// A three-way merge of unrelated user state. Code, settings, UID ownership,
/// permissions and library inputs remain strict; installed bits affect the
/// dependency graph and require a new admission rather than this rebase.
pub(crate) fn rebase_install_users(base:&Snapshot,current:&Snapshot,candidate:&mut crate::package::scan::SigningScan,targets:&BTreeSet<String>)->Result<bool,String>{
    Ok(install_user_rebase_conflict(base,current,candidate,targets)?.is_none())
}
pub(crate) fn install_user_rebase_conflict(base:&Snapshot,current:&Snapshot,candidate:&mut crate::package::scan::SigningScan,targets:&BTreeSet<String>)->Result<Option<String>,String>{
    let protected=base.owner().settings.packages.iter().chain(candidate.settings.packages.iter())
        .filter(|p|targets.contains(&p.name)).map(|p|p.app_id).collect::<BTreeSet<_>>();
    let mut normalized=base.owner().clone();
    let mut latest=current.owner().clone();
    let mut changes=Vec::new();
    for setting in &base.owner().settings.packages{
        if targets.contains(&setting.name)||protected.contains(&setting.app_id){continue;}
        let Some(before)=base.owner().scanned_user_states(&setting.name)else{continue;};
        let Some(after)=current.owner().scanned_user_states(&setting.name)else{return Ok(Some(format!("package {} missing current user owner",setting.name)));};
        if before.keys().ne(after.keys()){return Ok(Some(format!("package {} user inventory {:?} -> {:?}",setting.name,before.keys().collect::<Vec<_>>(),after.keys().collect::<Vec<_>>())));}
        for (user,state) in before{
            let now=&after[user];
            if state==now{continue;}
            if state.installed!=now.installed{return Ok(Some(format!("package {} user {user} installed {} -> {}",setting.name,state.installed,now.installed)));}
            let value=candidate.scanned_user_states(&setting.name).and_then(|users|users.get(user));
            if value!=Some(state)&&value!=Some(now){return Ok(Some(format!("package {} user {user} competing candidate state: before={state:?}, current={now:?}, candidate={value:?}; targets={targets:?}, protected appIds={protected:?}",setting.name)));}
            normalized.set_user_state(&setting.name,*user,now.clone())?;
            changes.push((setting.name.clone(),*user,now.clone()));
        }
    }
    if base.usage().names().ne(current.usage().names()){return Ok(Some("usage package inventory differs".into()));}
    if base.usage()!=current.usage() && normalized.has_replica_runtime() {
        normalized.update_replica_usage(base.usage(),current.usage())?;
    }
    // Resolved dependency records also capture user state; refresh only after
    // validating identical code/signatures/user inventory/installed bits.
    normalized.refresh_library_user_inputs()?;
    latest.refresh_library_user_inputs()?;
    if normalized!=latest{
        let before=Snapshot{version:base.version(),owner:normalized,usage:current.usage().clone()};
        return Ok(Some(format!("unmerged owner: {}; targets={targets:?}, protected appIds={protected:?}",install_identity_delta(&before,current))));
    }
    for (name,user,state) in changes{candidate.set_user_state(&name,user,state)?;}
    candidate.refresh_library_user_inputs()?;
    Ok(None)
}
pub(crate) fn install_identity_delta(before:&Snapshot,after:&Snapshot)->String{
    let old=before.owner();let new=after.owner();
    let mut fields=Vec::new();
    if old.settings!=new.settings{
        let names=old.settings.packages.iter().map(|package|package.name.as_str()).chain(new.settings.packages.iter().map(|package|package.name.as_str())).collect::<BTreeSet<_>>();
        for name in names{let a=old.settings.packages.iter().find(|package|package.name==name);let b=new.settings.packages.iter().find(|package|package.name==name);if a!=b{
            fields.push(format!("package {name}: {:?} -> {:?}",a.map(|p|(p.app_id,p.code_path.as_str(),p.version_code)),b.map(|p|(p.app_id,p.code_path.as_str(),p.version_code))));
        }}
        if fields.is_empty(){fields.push("settings shared/factory/global metadata".into());}
    }
    if old.identities!=new.identities{fields.push("UID/shared identity ownership".into());}
    if old.loaded_packages()!=new.loaded_packages(){fields.push("active parsed code".into());}
    if old.disabled_loaded_packages()!=new.disabled_loaded_packages(){fields.push("factory parsed code".into());}
    for setting in &old.settings.packages {
        let a=old.scanned_user_states(&setting.name);let b=new.scanned_user_states(&setting.name);
        match (a,b){
            (Some(a),Some(b))=>{for (user,before) in a {match b.get(user){
                Some(after)if before!=after=>{
                    if before.runtime.overlays()!=after.runtime.overlays()||before.runtime.libraries()!=after.runtime.libraries(){
                        fields.push(format!("package {} user {user} overlay paths {:?} -> {:?}; library overlays {:?} -> {:?}",setting.name,before.runtime.overlays(),after.runtime.overlays(),before.runtime.libraries(),after.runtime.libraries()));
                    }else{fields.push(format!("package {} user {user} state ownership: before={before:?}, current={after:?}",setting.name));}
                },None=>fields.push(format!("package {} removed user {user}",setting.name)),_=>{},
            }}for user in b.keys().filter(|user|!a.contains_key(user)){fields.push(format!("package {} added user {user}",setting.name));}},
            _ if a!=b=>fields.push(format!("package {} user inventory",setting.name)),_=>{},
        }
    }
    if old.libraries!=new.libraries{fields.push("library registry".into());}
    if old.update_ownership!=new.update_ownership{fields.push("update ownership".into());}
    if old.installers!=new.installers{fields.push("install source ownership".into());}
    if fields.is_empty(){fields.push("scoped user/seInfo/permission/runtime ownership".into());}
    format!("version {} -> {}: {}",before.version(),after.version(),fields.join("; "))
}
impl super::Store {
    /// Rebase usage and persist while the publication gate is held. The native
    /// candidate computation calls no Binder/guest owner, closing the previous
    /// capture-to-CAS window without weakening code/settings/UID ownership.
    pub(crate) fn publish_install_after(
        &self,base:&Arc<Snapshot>,mut owner:crate::package::scan::SigningScan,
        persist:impl FnOnce(&Arc<Snapshot>)->Result<(),crate::package::owner::WriteError>,
    )->Result<Arc<Snapshot>,super::CommitError>{
        let mut current=self.current.lock().unwrap();
        let targets=install_targets(base,&owner);
        if !rebase_install_users(base,&current,&mut owner,&targets).map_err(|error|super::CommitError::Snapshot(super::Error::Invalid(error)))? {
            return Err(super::CommitError::Snapshot(super::Error::Stale));
        }
        let usage=current.usage().for_install(owner.settings.packages.iter().map(|package|package.name.as_str()));
        crate::package::installer::permission_prepare::complete_candidate_runtime(&mut owner,&current,&usage)
            .map_err(|error|super::CommitError::Snapshot(super::Error::Invalid(format!("install publication runtime: {error}"))))?;
        let version=current.version().checked_add(1).filter(|version|*version<=i64::MAX as u64)
            .ok_or(super::CommitError::Snapshot(super::Error::VersionExhausted))?;
        super::validate(&owner,&usage).map_err(super::CommitError::Snapshot)?;
        let next=Arc::new(Snapshot{version,owner,usage});
        if self.replica{super::validate_replica(&next).map_err(super::CommitError::Snapshot)?;}
        match persist(&next){
            Ok(())=>{*current=next.clone();Ok(next)},
            Err(error)if error.committed=>{*current=next.clone();Err(super::CommitError::Disk{snapshot:Some(next),error})},
            Err(error)=>Err(super::CommitError::Disk{snapshot:None,error}),
        }
    }
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum Phase { BeforePermissionLifecycle, AfterPermissionLifecycle }
/// Invoke after the original permission lifecycle completed and installed its
/// live states in this candidate. The phase is an explicit pipeline boundary.
pub struct Builder {current:Current,bridge:Arc<Bridge>,config:Arc<SystemConfig>}
impl Builder {
    pub fn new(current:Current,bridge:Arc<Bridge>,config:Arc<SystemConfig>)->Arc<Self> {Arc::new(Self {current,bridge,config})}
    pub fn permission_source(self:&Arc<Self>)->crate::package::installer::permission_capture::ContextSource {
        let builder=self.clone();Arc::new(move|snapshot|builder.build(snapshot,Phase::BeforePermissionLifecycle))
    }
    pub fn publication_source(self:&Arc<Self>)->crate::package::installer::permission_capture::ContextSource {
        let builder=self.clone();Arc::new(move|snapshot|builder.build(snapshot,Phase::AfterPermissionLifecycle))
    }
    pub fn build(&self,snapshot:&Arc<Snapshot>,phase:Phase)->Result<Context,String> {
        let current=(self.current)()?;
        let scan=snapshot.owner();
        // Registration is a completed native owner, never reconstructed in map order.
        scan.package_registry()?;
        let mut context=(**current.context()).clone();
        context.scan_version=snapshot.version();
        context.packages.clear();context.retained_packages.clear();context.native_domains=None;
        let users=context.users.keys().copied().collect::<Vec<_>>();
        if users.is_empty() {return Err("install context has no resolved user owner".into());}
        let scan_users=context.scan_users.users.as_ref().ok_or("install context scan users unavailable")?;
        if scan_users.iter().map(|user|user.id).collect::<BTreeSet<_>>() != users.iter().copied().collect() {
            return Err("install context UM and scan-user inventories differ".into());
        }
        let mut permissions=BTreeMap::new();
        for (settings,factory) in [(&scan.settings.packages,false),(&scan.settings.disabled_system_packages,true)] {
            for setting in settings {
                let stored=if factory {scan.disabled_user_states(&setting.name)} else {scan.scanned_user_states(&setting.name)}
                    .ok_or("install context native package user owner unavailable")?;
                let code=if factory {scan.disabled_loaded_packages()} else {scan.loaded_packages()}.get(&setting.name);
                let parsed=code.map(|code|code.runtime_package());
                let inputs=self.package(setting,super::boot_context::permission_uid_owner(setting,parsed)?,stored,parsed,&users,&mut permissions)?;
                context.packages.insert((setting.name.clone(),factory),inputs);
            }
        }
        for (id,slot) in scan.identities.ids.owners() {
            if let UidOwner::DetachedPackage(name)=slot {
                let old=scan.identities.ids.detached_setting(id).ok_or("install context detached UID owner unavailable")?;
                let inputs=self.package(&old.package,Some(id),&old.users,None,&users,&mut permissions)?;
                context.retained_packages.insert((id,name.clone()),inputs);
            }
        }
        for group in scan.identities.shared_users.values() {
            for (name,old) in group.retained_settings() {
                let inputs=self.package(&old.package,Some(group.app_id),&old.users,None,&users,&mut permissions)?;
                if context.retained_packages.insert((group.app_id,name.into()),inputs).is_some() {
                    return Err("install context retained permission owner duplicated".into());
                }
            }
        }
        if phase==Phase::AfterPermissionLifecycle {
            // Migration state is not a substitute for a completed live permission receipt.
            // Receipt equality applies only to its genuine affected UIDs.
            // Every UID still has fresh original live grants/GIDs above.
            let receipt=scan.installed_permission_receipt_uids()?;
            for setting in &scan.settings.packages {
                let parsed=scan.loaded_packages().get(&setting.name).map(|code|code.runtime_package());
                let Some(permission_id)=super::boot_context::permission_uid_owner(setting,parsed)? else {continue;};
                if !receipt.contains(&permission_id){continue;}
                let legacy=if let Some((name,_))=scan.identities.shared_users.iter().find(|(_,group)|group.app_id==permission_id) {
                    scan.shared_legacy_permissions(name)?
                } else {scan.legacy_permissions(&setting.name,false)?}
                    .ok_or("post-install permission lifecycle projection unavailable")?;
                let live=permissions.get(&permission_id).ok_or("post-install live permission UID unavailable")?;
                if legacy.app_id()!=permission_id || legacy.bytes()!=live.bytes() {
                    return Err(format!("post-install permission lifecycle/live owner differs: {}",setting.name));
                }
            }
        }
        let existing=current.domains().ok_or("install context native domain owner unavailable")?;
        let mut domains=existing.owner().clone();
        for name in current.state().packages.keys() {
            if !scan.settings.packages.iter().any(|setting|&setting.name==name) {domains.clear_package(name);}
        }
        let mut policies=BTreeMap::new();let mut changes=Vec::new();
        for setting in &scan.settings.packages {
            let code=scan.loaded_packages().get(&setting.name).ok_or("install domain parsed-code owner unavailable")?;
            let code=&code.package;
            let policy=self.bridge.domain_verification_restricted(&code.package_name,code.target_sdk_version)
                .map_err(|error|format!("install domain compatibility: {error:?}"))?;
            policies.insert(setting.name.clone(),policy);
            let id=setting.domain_set_id.as_deref().ok_or("install domain UUID owner unavailable")?;
            let old=current.scan().owner().loaded_packages().get(&setting.name);
            let unchanged=old.is_some_and(|old|old.package==*code)
                && domains.package(&setting.name).is_some_and(|old|old.id.eq_ignore_ascii_case(id));
            if unchanged {continue;}
            let signatures=setting.signatures.as_ref().ok_or("install domain signing owner unavailable")?;
            let input=domain::Input {id,name:&setting.name,code:Some(code),signatures:&signatures.signatures,
                system:setting.flags&settings::FLAG_SYSTEM!=0,restrict_domains:policy,pre_verified:None};
            let change=match domains.package(&setting.name).map(|old|old.id.clone()) {
                Some(old_id)=>domains.migrate(&old_id,old.map(|code|&code.package),input,&self.config)?,
                None=>domains.add(input,&self.config)?,
            };
            changes.push((setting.name.clone(),change));
        }
        context=context.attach_boot_domains(scan,domain::Boot {owner:domains,changes},&self.config,&policies)?;
        // Factory and detached settings have no new domain lifecycle event. Their
        // old external domain projections are usable only with the same identity.
        for (key,inputs) in &mut context.packages {
            if !key.1 {continue;}
            let code=scan.disabled_loaded_packages().get(&key.0).ok_or("factory domain parsed-code owner unavailable")?;
            let restrict=self.bridge.domain_verification_restricted(&code.package.package_name,code.package.target_sdk_version)
                .map_err(|error|format!("factory domain compatibility: {error:?}"))?;
            let values=existing.owner().queries(&code.package,restrict,&self.config,inputs.users.keys().copied())?
                .ok_or("factory domain owner unavailable")?;
            inputs.domain_verification=values.verification;inputs.uri_relative_filter_groups=values.uri_relative_filter_groups;
            for (user,selection) in values.users {inputs.users.get_mut(&user).ok_or("factory domain user disappeared")?.domain_selection=Some(selection);}
        }
        for (key,inputs) in &mut context.retained_packages {
            let previous=current.context().retained_packages.get(key).ok_or("retained domain projection owner unavailable")?;
            if inputs.app_id!=previous.app_id||inputs.path!=previous.path||inputs.version!=previous.version {return Err("changed retained domain projection requires its own owner".into());}
            inputs.domain_verification=previous.domain_verification.clone();inputs.uri_relative_filter_groups=previous.uri_relative_filter_groups.clone();
            for (user,state) in &mut inputs.users {state.domain_selection=previous.users.get(user).ok_or("retained domain user owner unavailable")?.domain_selection.clone();}
        }
        let latest=(self.current)()?; // producer validates the actual boot epoch
        let targets=install_targets(current.scan(),scan);
        let mut retained=current.scan().owner().clone();
        if let Some(reason)=install_user_rebase_conflict(current.scan(),latest.scan(),&mut retained,&targets)? {
            return Err(format!("install context package/UID ownership changed during owner reads: {reason}"));
        }
        if current.state().users!=latest.state().users {
            return Err("install context UM user owner changed during owner reads".into());
        }
        if current.state().system.isolated_owners!=latest.state().system.isolated_owners {
            return Err(format!("install context isolated UID owner changed during owner reads: {:?} -> {:?}",current.state().system.isolated_owners,latest.state().system.isolated_owners));
        }
        Ok(context)
    }
    fn package(&self,setting:&settings::Package,permission_id:Option<i32>,stored:&BTreeMap<i32,restrictions::UserState>,code:Option<&crate::package::pkg::AndroidPackage>,users:&[i32],permissions:&mut BTreeMap<i32,legacy_permissions::State>)->Result<PackageInputs,String> {
        if let Some(id)=permission_id {
            if !(0..100_000).contains(&id) {return Err(format!("install context live permission UID unavailable: package={}, permission_id={id}",setting.name));}
        } else if super::boot_context::permission_uid_owner(setting,code)?.is_some() {return Err("install registered permission owner was omitted".into());}
        if stored.keys().any(|id|!users.contains(id)) {return Err("install context package references unknown UM user".into());}
        if let Some(id)=permission_id {
            if !permissions.contains_key(&id) {permissions.insert(id,self.bridge.legacy_permissions(id,users).map_err(|error|format!("install live permission UID: {error:?}"))?);}
        }
        let mut inputs=BTreeMap::new();
        for user in users {
            let Some(permission_id)=permission_id else {
                inputs.insert(*user,UserInputs {gids:Vec::new(),granted_permissions:Vec::new(),domain_selection:None});
                continue;
            };
            let live=&permissions[&permission_id];
            let state=live.user(*user).ok_or("install live permission user unavailable")?;
            let installed=stored.get(user).is_none_or(|state|state.installed);
            let mut grants=Vec::new();
            if installed {for permission in &state.permissions {if permission.granted {grants.push(permission.name.clone().ok_or("live granted permission has null identity")?);}}}
            let gids=self.bridge.permission_gids(permission_id,&[*user]).map_err(|error|format!("install live permission GIDs: {error:?}"))?.into_iter().map(|gid|gid as i32).collect();
            inputs.insert(*user,UserInputs {gids,granted_permissions:grants,domain_selection:None});
        }
        let syncable_authorities=code.into_iter().flat_map(|code|code.providers.iter()).filter(|provider|provider.syncable)
            .filter_map(|provider|provider.authority.as_ref().map(|authority|(provider.main.component.name.clone(),authority.clone()))).collect();
        Ok(PackageInputs {app_id:setting.app_id,path:setting.code_path.clone(),version:setting.version_code,
            installed_permissions:self.bridge.installed_permissions(&setting.name).map_err(|error|format!("install permission definitions: {error:?}"))?,
            filter_application_query:self.bridge.application_query_filtering(&setting.name,setting.target_sdk_version).map_err(|error|format!("install query compatibility: {error:?}"))?,
            users:inputs,syncable_authorities,domain_verification:None,uri_relative_filter_groups:Vec::new()})
    }
}
