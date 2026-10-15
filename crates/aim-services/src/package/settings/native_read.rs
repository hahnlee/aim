//! Constructor persistence owners for native Settings recovery.
//! Legacy single-user records are retained until user restrictions are attached.
use super::{Package, ReadError, ReadOwners, Settings, SharedUser};
use crate::package::{owner::{app_ids::AppIds, legacy_permissions::{InstallRead, Migration}}, preferred::Preferred, restrictions::UserState};
use aim_android_xml::{Element, Node, pull::{Event, Reader}};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct NativeRead {
    pub legacy_user_zero: BTreeMap<String, UserState>,
    pub legacy_domains: BTreeMap<String, i32>,
    pub preferred_user_zero: Preferred,
    pub default_browser_user_zero: Option<String>,
    pub installer_packages: BTreeSet<String>,
    registered_packages: BTreeSet<String>,
    registered_shared: BTreeMap<String, i32>,
}

/// Own all SettingBase migration objects for the entire recursive recovery.
/// UID bootstrap must use this same owner before the first packages.xml read.
pub struct NativeReaders {
    pub users: Vec<i32>,
    pub packages: BTreeMap<String, Migration>,
    pub factories: BTreeMap<String, Migration>,
    pub shared_users: BTreeMap<String, Migration>,
    pub remaining: NativeRead,
    pub retained_shared: BTreeMap<String, crate::package::owner::shared_users::SharedUser>,
    /// Distinct SettingBase instances displaced in the same group. Original
    /// ArraySet membership uses object identity rather than package name.
    pub retained_shared_instances: BTreeMap<i32, Vec<crate::package::owner::app_ids::DetachedSetting>>,
    pending_previous_users: BTreeMap<String, Option<UserState>>,
    completed_ids: Option<AppIds>,
}
impl NativeReaders {
    pub fn new(users: Vec<i32>) -> Result<Self, ReadError> {
        if users.iter().any(|user| *user < 0) || users.iter().collect::<BTreeSet<_>>().len() != users.len() {
            return Err(ReadError::Owner("invalid native Settings user inventory".into()));
        }
        Ok(Self { users, packages: BTreeMap::new(), factories: BTreeMap::new(), shared_users: BTreeMap::new(), remaining: NativeRead::default(), retained_shared: BTreeMap::new(), retained_shared_instances: BTreeMap::new(), pending_previous_users: BTreeMap::new(), completed_ids: None })
    }
    /// Called at ReadStage::Complete before recovery opens per-user files.
    pub fn complete(&mut self, settings: &mut Settings, ids: &mut AppIds, attempt: &mut super::PackageReadAttempt)
        -> Result<Vec<super::PendingOutcome>, ReadError> {
        // addPackageSettingLPw replaces only mPackages[name]. Prior AppIds and
        // SharedUserSetting references still point at the old SettingBase.
        for saved in &settings.shared_users {
            let owner=self.retained_shared.entry(saved.name.clone()).or_insert_with(||crate::package::owner::shared_users::SharedUser::new(saved.app_id,saved.flags,0));
            if owner.app_id!=saved.app_id{return Err(ReadError::Owner("retained shared UID identity changed".into()));}
            owner.signatures=saved.signatures.clone();
        }
        for saved in settings.packages.iter().filter(|package|package.shared_user){
            let owner=self.retained_shared.values_mut().find(|owner|Some(owner.app_id)==saved.shared_app_id()).ok_or_else(||ReadError::Owner("saved shared member has no UID owner".into()))?;
            owner.add_package(&saved.name,saved.flags,saved.private_flags);
        }
        let outcomes=attempt.resolve_pending(settings,ids,|package,group,previous,ids| {
            if let Some(old)=previous {
                let previous_user=self.pending_previous_users.remove(&package.name).flatten();
                let users=previous_user.into_iter().map(|state|(0,state)).collect::<BTreeMap<_,_>>();
                let legacy=if old.shared_user{
                    let group_name=self.retained_shared.iter().find(|(_,owner)|Some(owner.app_id)==old.shared_app_id()).map(|(name,_)|name.clone()).ok_or("displaced shared SettingBase has no owner")?;
                    Some(self.shared_users.get(&group_name).ok_or("displaced shared permission state missing")?.project(old.app_id,&self.users).map_err(|error|format!("displaced shared permissions: {error:?}"))?)
                }else{
                    Some(self.packages.get(&old.name).ok_or("displaced package permission state missing")?.project(old.app_id,&self.users).map_err(|error|format!("displaced package permissions: {error:?}"))?)
                };
                let retained=crate::package::owner::app_ids::DetachedSetting{package:old.clone(),user_aliases:users.keys().copied().collect(),users,legacy,install_fixed:Some(old.install_permissions_fixed),runtime:None};
                if old.shared_user{
                    let owner=self.retained_shared.values_mut().find(|owner|Some(owner.app_id)==old.shared_app_id()).ok_or("displaced shared SettingBase missing")?;
                    if old.shared_app_id()==Some(group.app_id)||owner.retained_setting(&old.name).is_some(){
                        self.retained_shared_instances.entry(owner.app_id).or_default().push(retained);
                    }else{owner.retain_unparsed_setting(retained)?;}
                }else{ids.detach(retained)?;}
            }
            if !self.shared_users.contains_key(&group.name) {
                return Err("pending shared package has no native permission SettingBase".into());
            }
            self.retained_shared.get_mut(&group.name).ok_or("pending shared member owner missing")?.add_package(&package.name,package.flags,package.private_flags);
            self.remaining.registered_packages.insert(package.name.clone());
            Ok(())
        }).map_err(ReadError::Owner)?;
        for outcome in &outcomes {
            if !matches!(outcome,super::PendingOutcome::Attached(_)) {eprintln!("Rejected pending Settings package without a shared UID owner");}
        }
        self.attach_legacy_domains(settings);
        self.completed_ids=Some(ids.clone());
        Ok(outcomes)
    }
    fn install(&mut self) -> InstallRead<'_, NativeRead> {
        InstallRead { users: &self.users, packages: &mut self.packages, factories: &mut self.factories, shared_users: &mut self.shared_users, remaining: &mut self.remaining }
    }
    /// Merge imported legacy domains before the attached domain owner is built.
    pub fn attach_legacy_domains(&self, settings: &mut Settings) {
        settings.legacy_domain_info.extend(self.remaining.legacy_domains.clone());
    }
}
impl ReadOwners for NativeReaders {
    fn factory_record(&mut self,s:&mut Settings,r:&mut Reader<'_>,e:&Element,ids:&AppIds)->Result<(),ReadError>{self.install().factory_record(s,r,e,ids)}
    fn start_attempt(&mut self,s:&Settings,p:&[Package])->Result<(),ReadError>{self.pending_previous_users.clear();self.install().start_attempt(s,p)}
    fn package_registered(&mut self,p:&Package,c:bool)->Result<(),ReadError>{self.install().package_registered(p,c)}
    fn package_header(&mut self,p:&Package,e:&Element)->Result<(),ReadError>{
        if p.shared_user&&p.app_id==0{
            self.pending_previous_users.entry(p.name.clone()).or_insert_with(||self.remaining.legacy_user_zero.get(&p.name).cloned());
            self.remaining.legacy_user_zero.remove(&p.name);
        }
        self.remaining.package_header(p,e)
    }
    fn shared_registered(&mut self,g:&SharedUser,c:bool)->Result<(),ReadError>{self.install().shared_registered(g,c)}
    fn package_child(&mut self,p:&mut Package,r:&mut Reader<'_>,e:&Element,ids:&AppIds)->Result<bool,ReadError>{self.install().package_child(p,r,e,ids)}
    fn shared_child(&mut self,g:&mut SharedUser,r:&mut Reader<'_>,e:&Element)->Result<bool,ReadError>{self.install().shared_child(g,r,e)}
    fn public_key(&mut self,b:&[u8])->Result<Option<Vec<u8>>,ReadError>{self.remaining.public_key(b)}
    fn global_record(&mut self,s:&mut Settings,r:&mut Reader<'_>,e:&Element)->Result<bool,ReadError>{self.remaining.global_record(s,r,e)}
}
impl ReadOwners for NativeRead {
    fn factory_record(&mut self,_:&mut Settings,_:&mut Reader<'_>,_:&Element,_:&AppIds)->Result<(),ReadError>{
        Err(ReadError::Owner("factory records require the native InstallRead owner".into()))
    }
    fn start_attempt(&mut self,settings:&Settings,pending:&[Package])->Result<(),ReadError>{
        self.registered_packages.extend(settings.packages.iter().chain(pending).map(|p|p.name.clone()));
        for group in &settings.shared_users {
            if self.registered_shared.get(&group.name).is_some_and(|id|*id!=group.app_id){return Err(ReadError::Owner("shared UID reader identity changed".into()));}
            self.registered_shared.insert(group.name.clone(),group.app_id);
        }
        Ok(())
    }
    fn package_registered(&mut self,p:&Package,_:bool)->Result<(),ReadError>{
        self.registered_packages.insert(p.name.clone());
        for name in [&p.install_source.installer,&p.install_source.initiating_package,&p.install_source.originating_package] {
            if let Some(name)=name {self.installer_packages.insert(name.clone());}
        }
        Ok(())
    }
    fn package_header(&mut self,p:&Package,start:&Element)->Result<(),ReadError>{
        let state=self.legacy_user_zero.entry(p.name.clone()).or_default();
        let enabled=start.string("enabled");
        let value=match enabled.as_deref(){None=>Some(0),Some(value)=>value.parse::<i32>().ok().or_else(||{
            if value.eq_ignore_ascii_case("true"){Some(1)}else if value.eq_ignore_ascii_case("false"){Some(2)}else if value.eq_ignore_ascii_case("default"){Some(0)}else{None}
        })};
        if let Some(value)=value{state.enabled=value;state.last_disable_app_caller=Some("settings".into());}
        else {eprintln!("Settings package {} has invalid enabled value",p.name);}
        Ok(())
    }
    fn shared_registered(&mut self,g:&SharedUser,_:bool)->Result<(),ReadError>{
        if self.registered_shared.get(&g.name).is_some_and(|id|*id!=g.app_id){return Err(ReadError::Owner("shared UID reader identity changed".into()));}
        self.registered_shared.insert(g.name.clone(),g.app_id);Ok(())
    }
    fn package_child(&mut self,p:&mut Package,r:&mut Reader<'_>,e:&Element,_:&AppIds)->Result<bool,ReadError>{
        match e.name.as_str(){
            "enabled-components"|"disabled-components"=>{
                let state=self.legacy_user_zero.entry(p.name.clone()).or_default();
                let target=if e.name=="enabled-components"{&mut state.enabled_components}else{&mut state.disabled_components};
                let values=target.get_or_insert_with(Vec::new);let outer=r.depth();
                loop{match r.next()?{
                    Event::Start(item)=>{if item.name=="item"{if let Some(name)=item.string("name"){let name=name.into_owned();if !values.contains(&name){values.push(name);}}else{eprintln!("Settings component item has no name");}}else{eprintln!("Unknown Settings component element {}",item.name);}super::skip(r)?;},
                    Event::End(_) if r.depth()<=outer=>break,Event::EndDocument=>break,_=>{}
                }}Ok(true)
            }
            "domain-verification"=>{let status=e.int("status").unwrap_or(None).unwrap_or(-1);self.legacy_domains.insert(p.name.clone(),status);super::skip(r)?;Ok(true)}
            "perms"=>Err(ReadError::Owner("install permission read requires native InstallRead".into())),
            _=>Ok(false),
        }
    }
    fn shared_child(&mut self,_:&mut SharedUser,_:&mut Reader<'_>,e:&Element)->Result<bool,ReadError>{
        if e.name=="perms"{return Err(ReadError::Owner("shared permission read requires native InstallRead".into()));}Ok(false)
    }
    fn public_key(&mut self,encoded:&[u8])->Result<Option<Vec<u8>>,ReadError>{
        // PackageParser.parsePublicKey returns null for malformed or unsupported
        // RSA/EC/DSA SPKI; the native serializer validates and canonicalizes it.
        match crate::package::sign::canonical_public_keys(&[encoded.to_vec()]){
            Ok(mut keys)=>Ok(keys.pop()),Err(error)=>{eprintln!("Settings invalid public key: {error}");Ok(None)}
        }
    }
    fn global_record(&mut self,_:&mut Settings,r:&mut Reader<'_>,e:&Element)->Result<bool,ReadError>{
        match e.name.as_str(){
            "preferred-activities"|"persistent-preferred-activities"|"crossProfile-intent-filters"=>{
                let section=subtree(r,e)?;let bytes=aim_android_xml::abx::write(&section).map_err(ReadError::File)?;
                let parsed=if e.name=="preferred-activities"{Preferred::parse(Some(&bytes),None)}else{Preferred::parse(None,Some(&bytes))};
                for item in parsed.preferred.entries(){self.preferred_user_zero.preferred.add(item.clone());}
                for item in parsed.persistent.entries(){self.preferred_user_zero.persistent.add(item.clone());}
                for item in parsed.cross_profile.entries(){self.preferred_user_zero.cross_profile.add(item.clone());}
                self.preferred_user_zero.preferred_resolver_present|=parsed.preferred_resolver_present;
                self.preferred_user_zero.persistent_resolver_present|=parsed.persistent_resolver_present;
                self.preferred_user_zero.cross_profile_resolver_present|=parsed.cross_profile_resolver_present;Ok(true)
            }
            "default-browser"=>{
                // The legacy Settings tag calls readDefaultApps on its body;
                // it does not read packageName from the container itself.
                let outer=r.depth();let mut browser=None;
                loop{match r.next()?{
                    Event::Start(item)=>match item.name.as_str(){
                        "default-browser"=>browser=item.string("packageName").map(|name|name.into_owned()),
                        "default-dialer"=>{},
                        _=>{eprintln!("Unknown default-apps element {}",item.name);super::skip(r)?;},
                    },
                    Event::End(_) if r.depth()<=outer=>break,Event::EndDocument=>break,_=>{}
                }}
                if let Some(browser)=browser{self.default_browser_user_zero=Some(browser);}Ok(true)
            }
            _=>Ok(false), // Settings dispatcher applies original skipCurrentTag.
        }
    }
}
fn subtree(r:&mut Reader<'_>,start:&Element)->Result<Element,ReadError>{
    let mut stack=vec![start.clone()];let outer=r.depth();
    loop {match r.next()?{
        Event::Start(element)=>stack.push(element),
        Event::Text(text)=>{stack.last_mut().unwrap().content.push(Node::Token(4,Some(text)));},
        Event::End(_)=>{if stack.len()>1{let child=stack.pop().unwrap();stack.last_mut().unwrap().content.push(Node::Element(child));}else if r.depth()<=outer{return Ok(stack.pop().unwrap());}},
        Event::EndDocument=>return Err(ReadError::File("unexpected EOF in Settings owned section".into())),
    }}
}

/// One retained constructor owner usable by the frontend and its completion
/// callback without borrowing it twice. No Android service owns these records.
#[derive(Clone)]
pub struct SharedReaders(pub std::sync::Arc<std::sync::Mutex<NativeReaders>>);
impl SharedReaders {
    pub fn new(users: Vec<i32>) -> Result<Self, ReadError> {
        Ok(Self(std::sync::Arc::new(std::sync::Mutex::new(NativeReaders::new(users)?))))
    }
    pub fn complete(&self, settings:&mut Settings, ids:&mut AppIds, attempt:&mut super::PackageReadAttempt)->Result<(),ReadError>{
        self.0.lock().map_err(|_|ReadError::Owner("native Settings read owner poisoned".into()))?.complete(settings,ids,attempt)?;
        Ok(())
    }
    fn locked(&self)->Result<std::sync::MutexGuard<'_,NativeReaders>,ReadError>{
        self.0.lock().map_err(|_|ReadError::Owner("native Settings read owner poisoned".into()))
    }
}
impl ReadOwners for SharedReaders {
    fn factory_record(&mut self,s:&mut Settings,r:&mut Reader<'_>,e:&Element,ids:&AppIds)->Result<(),ReadError>{self.locked()?.factory_record(s,r,e,ids)}
    fn start_attempt(&mut self,s:&Settings,p:&[Package])->Result<(),ReadError>{self.locked()?.start_attempt(s,p)}
    fn package_registered(&mut self,p:&Package,c:bool)->Result<(),ReadError>{self.locked()?.package_registered(p,c)}
    fn package_header(&mut self,p:&Package,e:&Element)->Result<(),ReadError>{self.locked()?.package_header(p,e)}
    fn shared_registered(&mut self,g:&SharedUser,c:bool)->Result<(),ReadError>{self.locked()?.shared_registered(g,c)}
    fn package_child(&mut self,p:&mut Package,r:&mut Reader<'_>,e:&Element,ids:&AppIds)->Result<bool,ReadError>{self.locked()?.package_child(p,r,e,ids)}
    fn shared_child(&mut self,g:&mut SharedUser,r:&mut Reader<'_>,e:&Element)->Result<bool,ReadError>{self.locked()?.shared_child(g,r,e)}
    fn public_key(&mut self,b:&[u8])->Result<Option<Vec<u8>>,ReadError>{self.locked()?.public_key(b)}
    fn global_record(&mut self,s:&mut Settings,r:&mut Reader<'_>,e:&Element)->Result<bool,ReadError>{self.locked()?.global_record(s,r,e)}
}

/// Transfer the completed constructor graph into the native scan candidate.
/// Call before scanning any APK or capturing runtime/permission owners.
pub fn attach_reader_graph(readers:&SharedReaders,scan:&mut crate::package::scan::SigningScan)->Result<(),ReadError>{
    let readers=readers.locked()?;
    let mut candidate=scan.clone();
    candidate.identities.ids=readers.completed_ids.clone().ok_or_else(||ReadError::Owner("Settings reader completion must precede scan graph attachment".into()))?;
    for (name,group) in &readers.retained_shared {
        if candidate.identities.shared_users.get(name).is_none_or(|current|current.app_id!=group.app_id){
            return Err(ReadError::Owner("scan shared identity differs from constructor reader".into()));
        }
        candidate.identities.shared_users.insert(name.clone(),group.clone());
    }
    for (id,instances) in &readers.retained_shared_instances {
        let group=candidate.identities.shared_users.values_mut().find(|group|group.app_id==*id)
            .ok_or_else(||ReadError::Owner("retained SettingBase instance has no scan shared owner".into()))?;
        for instance in instances {group.retain_read_instance(instance.clone()).map_err(ReadError::Owner)?;}
    }
    let mut packages=BTreeMap::new();
    for (settings,factory) in [(&candidate.settings.packages,false),(&candidate.settings.disabled_system_packages,true)]{
        for setting in settings {
            // Each shared PackageSetting still owns its own empty constructor
            // LegacyPermissionState; grants were read on its SharedUserSetting.
            let migration=if factory{
                readers.factories.get(&setting.name).cloned().ok_or_else(||ReadError::Owner("factory migration owner missing".into()))?
            }else if setting.shared_user{Migration::default()}
            else{readers.packages.get(&setting.name).cloned().ok_or_else(||ReadError::Owner("package migration owner missing".into()))?};
            packages.insert((setting.name.clone(),factory),migration);
        }
    }
    candidate.capture_legacy_permissions(&readers.users,packages,readers.shared_users.clone()).map_err(ReadError::Owner)?;
    *scan=candidate;Ok(())
}

/// Restore original SettingBase permission owners before fresh code admission.
/// Subsequent native constructors/disable/replacement operations copy or relink
/// these exact owners; post-scan inventory cannot substitute for saved Settings.
pub fn prepare_saved_permission_scan(
    readers:&SharedReaders,
    scan:&mut crate::package::scan::SigningScan,
    disk:&crate::package::owner::Store,
    config:&crate::package::system_config::SystemConfig,
)->Result<crate::package::owner::runtime_metadata::State,ReadError>{
    let mut candidate=scan.clone();
    attach_reader_graph(readers,&mut candidate)?;
    let metadata=disk.restore_runtime_permission_owners(&mut candidate,config)
        .map_err(|error|ReadError::Owner(format!("saved SettingBase permission restoration: {error}")))?;
    *scan=candidate;
    Ok(metadata)
}
