//! First native query capture: image, UM, permission and domain owners.
//! No original PMS feed or previously constructed Capture is required.
use super::{Snapshot,query_state::{Capture,Context,PackageInputs,UserInputs}};
use crate::package::{bootstrap::{Bridge,ApexInventory,ScanUsers},model,parse::Platform,
    owner::legacy_permissions,settings,restrictions,system_config::SystemConfig};
use std::{collections::{BTreeMap,BTreeSet},sync::Arc};

/// UserManager records are captured after Settings restrictions restoration.
/// The native disk documents supply preferred/restriction/browser fields; UM
/// supplies flags, profile grouping and actual unlocking/unlocked state.
pub struct UserRecord {
    pub id:i32,pub flags:i32,pub pre_created:bool,pub profile_group_id:i32,pub unlocking_or_unlocked:bool,
    pub preferred_activities:Option<Vec<u8>>,pub restrictions:Option<Vec<u8>>,pub default_browser:Option<String>,
}
/// Independent original/resource owners supply the system and resolver values.
/// `image_system` is system_config::system over the same image/property owner.
/// `runtime_system` carries real native service-owner Arcs and interaction state,
/// not values imported from IPackageFeedHost or a fabricated empty service graph.
pub struct Inputs {
    pub snapshot:Arc<Snapshot>,pub config:Arc<SystemConfig>,pub platform:Arc<Platform>,pub bridge:Arc<Bridge>,
    pub image_system:model::System,pub runtime_system:model::System,
    pub resolver:model::Platform,pub users:Vec<UserRecord>,
    pub classes:Arc<aim_android_image::linkage::Hierarchy>,pub apex:ApexInventory,pub scan_users:ScanUsers,
    pub resource_config:crate::package::parse::resources::Config,
    pub cross_user_suspensions:bool,pub nonce:Option<i64>,
    /// Native boot selection already performed by its real configured/resolver owner.
    pub sdk_sandbox_package:String,
}
pub fn build(input:Inputs)->Result<Context,String> {
    let scan=input.snapshot.owner();scan.package_registry()?;
    if input.platform.sdk<0 {return Err("boot parser SDK owner invalid".into());}
    if input.image_system.features!=input.config.features
        || input.image_system.hidden_api_allowlist!=input.config.hidden_api_allowlist
        || input.image_system.named_actors!=input.config.named_actors {
        return Err("boot image/SystemConfig owners differ".into());
    }
    let mut flags=input.platform.flags.iter().map(|(name,value)|(name.clone(),*value)).collect::<Vec<_>>();flags.sort();
    let mut image_flags=input.image_system.flags.clone();image_flags.sort();
    if image_flags!=flags || input.image_system.use_round_icon!=input.platform.use_round_icon {
        return Err("boot framework/parser flag owners differ".into());
    }
    let selected=scan.settings.packages.iter().find(|setting|setting.name==input.sdk_sandbox_package)
        .ok_or("boot SDK sandbox selection absent from native settings")?;
    if selected.flags&settings::FLAG_SYSTEM==0 || !scan.loaded_packages().contains_key(&selected.name) {
        return Err("boot SDK sandbox selection lacks native system code".into());
    }
    let mut system=input.runtime_system;
    if system.lifecycle.is_none() || system.permission_groups.is_none() || system.user_policy.is_none()
        || system.uninstall_blocks.is_none() || system.security_policy.is_none() {
        return Err("boot lifecycle/permission-group/UM/security/global Settings owner unavailable".into());
    }
    system.features=input.config.features.clone();system.hidden_api_allowlist=input.config.hidden_api_allowlist.clone();
    system.named_actors=input.config.named_actors.clone();system.system_permissions=Some(input.config.system_permissions.clone());
    let mut initial=input.config.initial_non_stopped_system_packages.iter().cloned().collect::<Vec<_>>();
    initial.sort_by_key(|name|crate::package::info::java_hash(name));system.initial_non_stopped_system_packages=Some(initial);
    system.gl_es_version=input.image_system.gl_es_version;system.use_round_icon=input.platform.use_round_icon;
    system.compatibility_mode=input.image_system.compatibility_mode;system.fallback_categories=input.image_system.fallback_categories;
    system.flags=flags;system.sdk_sandbox_package=Some(Some(input.sdk_sandbox_package));
    let resources=crate::package::parse::resources::Resources {tables:vec![&input.platform.framework],overlays:&input.platform.framework_overlays,config:input.resource_config};
    use crate::package::parse::resources::{Selected,TYPE_REFERENCE,TYPE_FIRST_INT,TYPE_LAST_INT};
    let id=input.platform.framework.id("bool","config_forceSystemPackagesQueryable").ok_or("boot AppsFilter system-query resource unavailable")?;
    let mut selected=Selected {kind:TYPE_REFERENCE,data:id,table:None,resid:0,flags:0};resources.resolve(&mut selected);
    if !(TYPE_FIRST_INT..=TYPE_LAST_INT).contains(&selected.kind) {return Err("boot AppsFilter boolean resource has no native value".into());}
    system.force_system_packages_queryable=selected.data!=0;
    let id=input.platform.framework.id("array","config_forceQueryablePackages").ok_or("boot AppsFilter query-list resource unavailable")?;
    system.force_queryable_packages=resources.string_array(id).ok_or("boot AppsFilter string-array value unavailable")?
        .into_iter().map(|name|name.ok_or("boot AppsFilter query-list contains null".to_owned())).collect::<Result<_,_>>()?;
    let mut users=BTreeMap::new();let mut pre_created=BTreeMap::new();
    for record in input.users {
        if record.id<0 || users.contains_key(&record.id) {return Err("boot UM record identity duplicated/invalid".into());}
        if record.restrictions.is_none() {return Err("boot native restriction document owner unavailable".into());}
        pre_created.insert(record.id,record.pre_created);
        let user=model::User {id:record.id,flags:record.flags,profile_group_id:record.profile_group_id,
            unlocking_or_unlocked:record.unlocking_or_unlocked,preferred_activities:record.preferred_activities,
            restrictions:record.restrictions,default_browser:record.default_browser};
        users.insert(user.id,user);
    }
    let scanned=input.scan_users.users.as_ref().ok_or("boot scan-user owner unavailable")?;
    if users.is_empty() || scanned.iter().map(|user|user.id).collect::<BTreeSet<_>>() != users.keys().copied().collect() {
        return Err("boot UM/scan-user inventories differ".into());
    }
    for user in scanned {
        if pre_created[&user.id]!=user.pre_created {return Err("boot UM/scan pre-created state differs".into());}
    }
    let ids=users.keys().copied().collect::<Vec<_>>();
    let mut context=Context {scan_version:input.snapshot.version(),native_domains:None,boot_classes:Some(input.classes),nonce:input.nonce,
        system,platform:input.resolver,users,apex_inventory:input.apex,scan_users:input.scan_users,
        cross_user_suspensions:input.cross_user_suspensions,packages:BTreeMap::new(),retained_packages:BTreeMap::new()};
    let mut permissions=BTreeMap::new();
    for (settings,factory) in [(&scan.settings.packages,false),(&scan.settings.disabled_system_packages,true)] {
        for setting in settings {
            let stored=if factory {scan.disabled_user_states(&setting.name)} else {scan.scanned_user_states(&setting.name)}
                .ok_or("boot native user-state owner unavailable")?;
            let code=if factory {scan.disabled_loaded_packages()} else {scan.loaded_packages()}.get(&setting.name);
            let parsed=code.map(|code|code.runtime_package());
            let value=package(&input.bridge,setting,permission_uid_owner(setting,parsed)?,stored,parsed,&ids,&mut permissions)?;
            context.packages.insert((setting.name.clone(),factory),value);
        }
    }
    for (id,slot) in scan.identities.ids.owners() {
        if let crate::package::owner::app_ids::Owner::DetachedPackage(name)=slot {
            let old=scan.identities.ids.detached_setting(id).ok_or("boot detached setting owner unavailable")?;
            context.retained_packages.insert((id,name.clone()),package(&input.bridge,&old.package,Some(id),&old.users,None,&ids,&mut permissions)?);
        }
    }
    for group in scan.identities.shared_users.values() {
        for (name,old) in group.retained_settings() {
            let value=package(&input.bridge,&old.package,Some(group.app_id),&old.users,None,&ids,&mut permissions)?;
            if context.retained_packages.insert((group.app_id,name.into()),value).is_some() {return Err("boot retained UID role duplicated".into());}
        }
    }
    // Native boot-domain attachment uses persisted native domain state, real
    // accepted code/signing/UUIDs, and the independent PlatformCompat leaves.
    input.bridge.resolve_boot_domain_query_context(scan,context,&input.config)
        .map_err(|error|format!("boot native domain query owner: {error:?}"))
}
pub fn capture(input:Inputs)->Result<Arc<Capture>,String> {
    let snapshot=input.snapshot.clone();Capture::new(snapshot,build(input)?)
}
pub(crate) fn permission_uid_owner(setting:&settings::Package,code:Option<&crate::package::pkg::AndroidPackage>)->Result<Option<i32>,String> {
    let id=setting.uid_owner_id();
    if id==-1 && !setting.shared_user && code.is_some_and(|code|
        code.is2(crate::package::pkg::booleans2::APEX) && code.uid==-1
        && code.package_name==setting.name && code.path.as_deref()==Some(setting.code_path.as_str())) {
        // Initial non-shared APEX code has no process UID. Original permission
        // getLegacyPermissionState has no UID state; getGidsForUid returns the
        // empty array for that absent state. Definitions remain a separate owner.
        return Ok(None);
    }
    if !(0..100_000).contains(&id) {return Err(format!("boot registered permission UID owner unavailable: package={}, app_id={}, shared_app_id={:?}, permission_id={id}",setting.name,setting.app_id,setting.shared_app_id()));}
    Ok(Some(id))
}
fn package(bridge:&Bridge,setting:&settings::Package,permission_id:Option<i32>,stored:&BTreeMap<i32,restrictions::UserState>,
    code:Option<&crate::package::pkg::AndroidPackage>,users:&[i32],cache:&mut BTreeMap<i32,legacy_permissions::State>)->Result<PackageInputs,String> {
    if let Some(id)=permission_id {
        if !(0..100_000).contains(&id) {return Err(format!("boot registered permission UID owner unavailable: package={}, permission_id={id}",setting.name));}
    } else if permission_uid_owner(setting,code)?.is_some() {return Err("boot registered permission owner was omitted".into());}
    if stored.keys().any(|id|!users.contains(id)) {return Err("boot package user is outside UM inventory".into());}
    if let Some(id)=permission_id {
        if !cache.contains_key(&id) {cache.insert(id,bridge.legacy_permissions(id,users).map_err(|error|format!("boot live permission UID: {error:?}"))?);}
    }
    let mut values=BTreeMap::new();
    for id in users {
        let Some(permission_id)=permission_id else {
            values.insert(*id,UserInputs {gids:Vec::new(),granted_permissions:Vec::new(),domain_selection:None});
            continue;
        };
        let live=&cache[&permission_id];
        let state=live.user(*id).ok_or("boot live permission user owner unavailable")?;
        let mut grants=Vec::new();
        if stored.get(id).is_none_or(|state|state.installed) {
            for permission in &state.permissions {if permission.granted {grants.push(permission.name.clone().ok_or("boot granted permission identity null")?);}}
        }
        let gids=bridge.permission_gids(permission_id,&[*id]).map_err(|error|format!("boot actual permission GIDs: {error:?}"))?.into_iter().map(|gid|gid as i32).collect();
        values.insert(*id,UserInputs {gids,granted_permissions:grants,domain_selection:None});
    }
    let syncable=code.into_iter().flat_map(|code|code.providers.iter()).filter(|provider|provider.syncable)
        .filter_map(|provider|provider.authority.as_ref().map(|authority|(provider.main.component.name.clone(),authority.clone()))).collect();
    Ok(PackageInputs {app_id:setting.app_id,path:setting.code_path.clone(),version:setting.version_code,
        installed_permissions:bridge.installed_permissions(&setting.name).map_err(|error|format!("boot live permission definitions: {error:?}"))?,
        filter_application_query:bridge.application_query_filtering(&setting.name,setting.target_sdk_version).map_err(|error|format!("boot actual query compatibility: {error:?}"))?,
        users:values,syncable_authorities:syncable,domain_verification:None,uri_relative_filter_groups:Vec::new()})
}

/// Concrete UM/resource leaf result; restriction documents stay native-disk-owned.
pub struct Original {
    pub users:Vec<UserRecord>,pub resolver:model::Platform,pub compatibility_mode:bool,
}
impl Original {
    pub fn capture(owner:&aim_binder_host::local::Strong)->Result<Self,String> {
        use aim_binder_host::parcel::{Parcel,Reader};
        use aim_service_aidl::dev_aim_server_ipackagebootcontextleaf as api;
        let mut request=Parcel::new();api::CaptureBootContext {}.write(&mut request);
        let reply=owner.transact(api::CAPTURE_BOOT_CONTEXT,&request,false).map_err(|status|format!("boot context leaf transport: {status}"))?;
        let bytes=api::read_capture_boot_context_reply(&mut reply.reader()).map_err(|status|format!("boot context leaf reply: {status}"))?
            .map_err(|error|error.message)?.ok_or("boot context leaf returned null")?;
        let mut reader=Reader::new(&bytes,&[]);let read=|status|format!("boot context record: {status}");
        if reader.read_i32().map_err(read)?!=2 {return Err("boot context record version differs".into());}
        let count=reader.read_i32().map_err(read)?;if count<0||count as usize>reader.remaining()/20 {return Err("boot UM count invalid".into());}
        let mut users=Vec::new();let mut previous=-1;
        for _ in 0..count {
            let id=reader.read_i32().map_err(read)?;if id<=previous {return Err("boot original UM order invalid".into());}previous=id;
            users.push(UserRecord {id,flags:reader.read_i32().map_err(read)?,pre_created:reader.read_bool().map_err(read)?,
                profile_group_id:reader.read_i32().map_err(read)?,unlocking_or_unlocked:reader.read_bool().map_err(read)?,
                preferred_activities:None,restrictions:None,default_browser:None});
        }
        let theme=reader.read_i32().map_err(read)?;let count=reader.read_i32().map_err(read)?;
        if count<0||count as usize>reader.remaining()/8 {return Err("boot resolver titles count invalid".into());}
        let mut titles=Vec::new();for _ in 0..count {titles.push((reader.read_string16().map_err(read)?,reader.read_i32().map_err(read)?));}
        let custom=reader.read_string16().map_err(read)?.ok_or("boot custom resolver resource missing")?;
        let resolver=model::Platform {resolver_theme:theme,resolver_titles:titles,custom_resolver:(!custom.is_empty()).then_some(custom),
            android_application:None,device_provisioned:false,settings_owner:None,instant_app_resolver:None,instant_app_installer:None,
            query_filtering_disabled:reader.read_bool().map_err(read)?};
        let compatibility_mode=reader.read_bool().map_err(read)?;
        if reader.remaining()!=0 {return Err("boot context record trailing bytes".into());}
        Ok(Self {users,resolver,compatibility_mode})
    }
    /// Connect this concrete original leaf record to native/image construction.
    pub fn populate(self,mut input:Inputs)->Result<Inputs,String> {
        if self.users.iter().any(|user|user.restrictions.is_none()) {return Err("boot leaf native documents were not attached".into());}
        if self.resolver.settings_owner.is_none() {return Err("boot original lazy settings owner unavailable".into());}
        input.users=self.users;input.resolver=self.resolver;input.image_system.compatibility_mode=self.compatibility_mode;
        Ok(input)
    }
    pub fn bind_settings(mut self,owner:Arc<aim_binder_host::local::Strong>)->Self {
        self.resolver.settings_owner=Some(crate::package::resolve::settings::Owner::new(owner,self.resolver.query_filtering_disabled));self
    }
    pub fn with_native_users(mut self,documents:&BTreeMap<i32,(Option<Vec<u8>>,Vec<u8>,Option<String>)>)->Result<Self,String> {
        if documents.keys().copied().collect::<BTreeSet<_>>() != self.users.iter().map(|user|user.id).collect() {return Err("boot native restriction document inventory differs".into());}
        for user in &mut self.users {
            let (preferred,restrictions,browser)=&documents[&user.id];
            user.preferred_activities=preferred.clone();user.restrictions=Some(restrictions.clone());user.default_browser=browser.clone();
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn boot_permission_uid_owner_preserves_shared_apex_identity_and_rejects_unowned() {
        let xml=aim_android_xml::read(b"<packages><shared-user name='android.uid.system' userId='1000'/><package name='com.android.runtime' codePath='/apex/com.android.runtime' sharedUserId='1000' version='1'/></packages>").unwrap();
        let mut saved=crate::package::settings::Settings::parse(&xml).unwrap();
        let setting=&mut saved.packages[0];
        // Initial APEX registration sets only mAppId to INVALID_UID; the
        // shared relationship restored by the real Settings parser remains.
        setting.shared_user_app_id=Some(setting.app_id);
        setting.app_id=-1;
        let before=setting.clone();
        assert_eq!(super::permission_uid_owner(setting,None).unwrap(),Some(1000));
        assert_eq!(setting.app_id,-1);
        assert_eq!(*setting,before);
        setting.shared_user=false;
        let error=super::permission_uid_owner(setting,None).unwrap_err();
        assert!(error.contains("com.android.runtime"));
        assert!(error.contains("permission_id=-1"));
        let mut code=crate::package::pkg::AndroidPackage {
            package_name:setting.name.clone(),path:Some(setting.code_path.clone()),uid:-1,
            booleans2:crate::package::pkg::booleans2::APEX,..Default::default()
        };
        let before=setting.clone();
        assert_eq!(super::permission_uid_owner(setting,Some(&code)).unwrap(),None);
        assert_eq!(*setting,before);
        code.booleans2=0;
        assert!(super::permission_uid_owner(setting,Some(&code)).is_err());
        code.booleans2=crate::package::pkg::booleans2::APEX;
        code.uid=1000;
        assert!(super::permission_uid_owner(setting,Some(&code)).is_err());
        code.uid=-1;code.path=Some("/apex/other".into());
        assert!(super::permission_uid_owner(setting,Some(&code)).is_err());
    }
}
