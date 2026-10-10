//! Native mutation preparation, persistence and original-owner effect execution.
//! The System publication gate consumes Prepared before calling finish outside
//! its locks. Shared dependency callbacks bind the real native install/preference
//! pipelines; they never delegate to original PMS.
use super::{apps_filter, changes, effects, info, mutations, preferred, query::Query,
    scan::SigningScan, suspension, write::{enabled, mutation::{Change, Plan, Request}}};
use aim_binder_host::parcel::{Exception, Parcel, Reader, EX_ILLEGAL_STATE, BAD_VALUE};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use std::sync::Arc;

pub struct Dependencies {
    pub effects: Arc<effects::Owner>,
    pub changes: Arc<changes::Owner>,
    pub preferred: Arc<preferred::registry::Registry>,
    pub shell_restricted: Box<dyn Fn(i32) -> Result<bool, Exception> + Send + Sync>,
    pub enable_compressed: Box<dyn Fn(&str, i32) -> Result<bool, Exception> + Send + Sync>,
    pub commit_preferred: Box<dyn Fn(preferred::registry::Stage) -> Result<(), Exception> + Send + Sync>,
    pub system_install_state: Box<dyn Fn(&str, bool, i32) -> Result<bool, Exception> + Send + Sync>,
    pub quarantine_enabled: bool,
}
pub enum Finish {
    None,
    Single(Request),
    Enabled(Vec<enabled::Setting>),
    Hidden { package: String, user: i32, hidden: bool },
    Suspension(suspension::Batch),
    Distraction { names: Vec<String>, uids: Vec<i32>, user: i32, flags: i32 },
    Mime { package: String, group: Option<String> },
}
pub enum Global { HiddenUntilInstalled(String, bool), RequiredForSystemUser(String, bool) }
pub struct Prepared {
    pub code: u32,
    pub plans: Vec<Plan>,
    pub global: Option<Global>,
    pub finish: Finish,
    pub reply: Parcel,
    pub distraction_cleanup: Vec<(Vec<String>, Vec<i32>, i32)>,
}
impl Prepared {
    pub fn changes_state(&self) -> bool {
        self.global.is_some() || self.plans.iter().any(|plan| !matches!(plan.change, Change::None))
    }

    pub fn apply_scan(&self, scan: &mut SigningScan) -> Result<(), String> {
        let mut candidate = scan.clone();
        for plan in &self.plans { plan.apply_scan(&mut candidate)?; }
        match &self.global {
            Some(Global::HiddenUntilInstalled(name, hidden)) => {
                for packages in [&mut candidate.settings.packages, &mut candidate.settings.disabled_system_packages] {
                    if let Some(package) = packages.iter_mut().find(|package| package.name == *name) { package.transient.hidden_until_installed = *hidden; }
                }
            }
            Some(Global::RequiredForSystemUser(name, required)) => {
                let package = candidate.settings.packages.iter_mut().find(|package| package.name == *name).ok_or("required-for-system-user package absent")?;
                if *required { package.private_flags |= 1 << 9; } else { package.private_flags &= !(1 << 9); }
            }
            None => {}
        }
        *scan = candidate;
        Ok(())
    }
    pub fn persist(&self, disk: &mut super::owner::Store, scan: &SigningScan) -> Result<(), super::owner::WriteError> {
        if matches!(self.global, Some(Global::RequiredForSystemUser(..))) { return disk.commit_scan_settings_owner(scan); }
        let mut users = std::collections::BTreeSet::new();
        for plan in &self.plans {
            match plan.change {
                Change::None => {}
                Change::MimeGroup { .. } | Change::UpdateAvailable(_) | Change::CategoryHint(_) | Change::RelinquishUpdateOwner => {
                    disk.commit_mutation(plan)?;
                }
                _ => if let Some(user) = plan.user { users.insert(user); },
            }
        }
        for user in users {
            let user = u32::try_from(user).map_err(|error| super::owner::WriteError { committed: false, message: error.to_string() })?;
            disk.commit_updated_scan_restrictions(scan, user, false)?;
        }
        Ok(())
    }
    pub fn finish(&self, dependencies: &Dependencies, before: &super::model::State, after: &super::model::State, uid: i32) -> Result<(), Exception> {
        for (names, uids, user) in &self.distraction_cleanup {
            if !names.is_empty() { dependencies.effects.distraction(names, uids, *user, 0)?; }
        }
        match &self.finish {
            Finish::None => {}
            Finish::Single(request) => {
                for plan in &self.plans {
                    if matches!(request, Request::Enabled(_)) && !matches!(plan.change, Change::Enabled(_)) { continue; }
                    if let Request::Enabled(setting) = request {
                        if matches!(plan.change, Change::Enabled(_)) { dependencies.changes.update(&plan.package, &[setting.user]); }
                    }
                    dependencies.effects.finish_mutation(request, plan, after, uid)?;
                }
                if self.plans.is_empty() {
                    if let Request::Stopped { package, user, stopped: false } = request {
                        if after.users.contains_key(user) { dependencies.effects.unhibernate(package, *user)?; }
                    }
                }
            }
            Finish::Enabled(settings) => {
                for plan in &self.plans {
                    if !matches!(plan.change, Change::Enabled(_)) { continue; }
                    let Some(user) = plan.user else { continue; };
                    let matching: Vec<_> = settings.iter().filter(|setting| {
                        if setting.package != plan.package { return false; }
                        let Some(old) = before.packages.get(&setting.package) else { return false; };
                        let Some(new) = after.packages.get(&setting.package) else { return false; };
                        let old = info::user_state(old, user); let new = info::user_state(new, user);
                        if let Some(class) = &setting.class {
                            let value = |state: &super::model::PackageUserState| if state.enabled_components.contains(class) { 1 } else if state.disabled_components.contains(class) { 2 } else { 0 };
                            value(&old) != value(&new)
                        } else { old.enabled != new.enabled }
                    }).collect();
                    let components: Vec<_> = matching.iter().map(|setting| setting.class.clone().unwrap_or_else(|| plan.package.clone())).collect();
                    if matching.is_empty() { continue; }
                    for _ in &matching { dependencies.changes.update(&plan.package, &[user]); }
                    let flags = matching.first().unwrap().flags;
                    let package = after.packages.get(&plan.package).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "enabled package disappeared"))?;
                    dependencies.effects.package_changed(&plan.package, apps_filter::uid(user, package.app_id), flags & 1 != 0, &components, None, uid, flags & 1 != 0)?;
                }
            }
            Finish::Hidden { package, user, hidden } => dependencies.effects.hidden(package, *user, *hidden)?,
            Finish::Suspension(batch) => batch.finish(&dependencies.effects)?,
            Finish::Distraction { names, uids, user, flags } => {
                if !names.is_empty() { dependencies.effects.distraction(names, uids, *user, *flags)?; }
            }
            Finish::Mime { package, group } => {
                if effects::mime_index_changed(before, after, package, group.as_deref())? {
                    let stages = dependencies.preferred.prepare_clear_all(Some(package)).map_err(|error| Exception::new(EX_ILLEGAL_STATE, error))?;
                    for stage in stages { (dependencies.commit_preferred)(stage)?; }
                    let state = after.packages.get(package).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "MIME package disappeared"))?;
                    for (user, state_user) in &state.users {
                        if state_user.installed { dependencies.effects.package_changed(package, apps_filter::uid(*user, state.app_id), true, &[package.clone()], Some("The mimeGroup is changed"), uid, false)?; }
                    }
                }
            }
        }
        Ok(())
    }
}
fn issue(error: apps_filter::NotModelled) -> Exception { Exception::new(EX_ILLEGAL_STATE, error.0) }
fn decode(status: i32) -> Exception { Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, format!("mutation parcel: {status}")) }
fn void_reply() -> Parcel { let mut parcel = Parcel::new(); parcel.write_no_exception(); parcel }

pub(crate) fn handles(code: u32) -> bool {
    matches!(code, pm::SET_COMPONENT_ENABLED_SETTINGS | pm::SET_APPLICATION_HIDDEN_SETTING_AS_USER | pm::SET_PACKAGES_SUSPENDED_AS_USER | pm::SET_DISTRACTING_PACKAGE_RESTRICTIONS_AS_USER | pm::GET_UNSUSPENDABLE_PACKAGES_FOR_USER | pm::GET_CHANGED_PACKAGES | pm::SET_SYSTEM_APP_HIDDEN_UNTIL_INSTALLED | pm::SET_SYSTEM_APP_INSTALL_STATE | pm::SET_REQUIRED_FOR_SYSTEM_USER | pm::SET_COMPONENT_ENABLED_SETTING | pm::SET_APPLICATION_ENABLED_SETTING | pm::SET_PACKAGE_STOPPED_STATE | pm::SET_SPLASH_SCREEN_THEME | pm::SET_HARMFUL_APP_WARNING | pm::SET_APPLICATION_CATEGORY_HINT | pm::RELINQUISH_UPDATE_OWNERSHIP | pm::SET_USER_MIN_ASPECT_RATIO | pm::SET_MIME_GROUP | pm::SET_UPDATE_AVAILABLE)
}

pub fn prepare(code: u32, reader: &mut Reader<'_>, query: &Query<'_>, pid: i32, dependencies: &Dependencies) -> Option<Result<Prepared, Exception>> {
    if !handles(code) { return None; }
    Some((|| {
        let mut result = Prepared { code, plans: Vec::new(), global: None, finish: Finish::None, reply: void_reply(), distraction_cleanup: Vec::new() };
        match code {
            pm::SET_COMPONENT_ENABLED_SETTINGS => {
                let batch = enabled::Batch::read(query.calling_uid, reader).map_err(decode)?;
                if query.state.users.contains_key(&batch.user) {
                    if query.calling_uid == 2000 && (dependencies.shell_restricted)(batch.user)? { return Err(Exception::security("Shell does not have permission to access this user")); }
                    if let Err(error) = query.enforce_cross_user(batch.user, false, false, "set enabled").map_err(issue)? { return Err(error); }
                }
                let settings = batch.settings(query)?;
                let mut policies = Vec::new();
                let mut compressed = Vec::new();
                for setting in &settings {
                    let (policy, needs_compressed) = enabled_policy(query, setting, pid, dependencies)?;
                    policies.push(policy); compressed.push(needs_compressed);
                }
                for ((setting, policy), needed) in settings.iter().zip(&mut policies).zip(compressed) {
                    if needed { policy.compressed_enabled = Some((dependencies.enable_compressed)(&setting.package, setting.user)?); }
                }
                result.plans = batch.prepare(query, pid, &settings, &policies)?;
                result.finish = Finish::Enabled(settings);
            }
            pm::SET_APPLICATION_HIDDEN_SETTING_AS_USER => {
                let hidden = mutations::Hidden::read(reader).map_err(decode)?;
                if !matches!(apps_filter::app_id(query.calling_uid), 0 | 1000) && !query.uid_has_permission(query.calling_uid, "android.permission.MANAGE_USERS").map_err(issue)? { return Err(Exception::security("MANAGE_USERS")); }
                let restricted = if query.calling_uid == 2000 && hidden.user >= 0 { Some((dependencies.shell_restricted)(hidden.user)?) } else { None };
                if let Err(error) = query.full_cross_user_with_shell(hidden.user, true, restricted).map_err(issue)? { return Err(error); }
                let policy = mutations::HiddenPolicy { active_device_admin: if hidden.hidden { Some(dependencies.effects.device_admin_nullable(hidden.package.as_deref(), hidden.user)?) } else { None },
                    protected: if hidden.hidden { Some(dependencies.effects.state_protected_nullable(hidden.package.as_deref(), hidden.user)?) } else { None }, shell_restricted: restricted };
                let plan = hidden.decide(query, policy).map_err(issue)??;
                result.reply = Parcel::new(); pm::write_set_application_hidden_setting_as_user_reply(&mut result.reply, plan.is_some());
                if let Some(plan) = plan { result.finish = Finish::Hidden { package: plan.package.clone(), user: hidden.user, hidden: hidden.hidden }; result.plans.push(plan); }
            }
            pm::SET_PACKAGES_SUSPENDED_AS_USER => {
                let request = suspension::Request::read(reader).map_err(decode)?;
                let batch = request.prepare(query, &dependencies.effects, dependencies.quarantine_enabled)?;
                result.plans = batch.plans.clone(); result.reply = Parcel::new();
                pm::write_set_packages_suspended_as_user_reply(&mut result.reply, &batch.rejected);
                result.finish = Finish::Suspension(batch);
            }
            pm::SET_DISTRACTING_PACKAGE_RESTRICTIONS_AS_USER => {
                let request = suspension::Distraction::read(reader).map_err(decode)?;
                let batch = request.prepare(query, &dependencies.effects)?;
                result.plans = batch.plans.clone(); result.reply = Parcel::new();
                pm::write_set_distracting_package_restrictions_as_user_reply(&mut result.reply, &batch.rejected);
                result.finish = Finish::Distraction { names: batch.notify, uids: batch.notify_uids, user: request.user, flags: request.flags };
            }
            pm::GET_UNSUSPENDABLE_PACKAGES_FOR_USER => {
                let args = pm::GetUnsuspendablePackagesForUser::read(reader).map_err(decode)?;
                if reader.remaining() != 0 { return Err(decode(BAD_VALUE)); }
                let packages = args.package_names.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageNames cannot be null"))?;
                let value = suspension::unsuspendable(query, &dependencies.effects, packages, args.user_id)?;
                result.reply = Parcel::new(); pm::write_get_unsuspendable_packages_for_user_reply(&mut result.reply, &Some(value));
            }
            pm::GET_CHANGED_PACKAGES => {
                let args = pm::GetChangedPackages::read(reader).map_err(decode)?;
                if reader.remaining() != 0 { return Err(decode(BAD_VALUE)); }
                let changed = changes::query(query, &dependencies.changes, args.sequence_number, args.user_id).map_err(issue)??;
                result.reply = Parcel::new(); pm::write_get_changed_packages_reply(&mut result.reply, changed.as_ref());
            }
            pm::SET_SYSTEM_APP_HIDDEN_UNTIL_INSTALLED | pm::SET_SYSTEM_APP_INSTALL_STATE => {
                let (name, hidden, install_user) = if code == pm::SET_SYSTEM_APP_HIDDEN_UNTIL_INSTALLED {
                    let args = pm::SetSystemAppHiddenUntilInstalled::read(reader).map_err(decode)?; (args.package_name, args.hidden, None)
                } else { let args = pm::SetSystemAppInstallState::read(reader).map_err(decode)?; (args.package_name, args.installed, Some(args.user_id)) };
                if reader.remaining() != 0 { return Err(decode(BAD_VALUE)); }
                let privileged = matches!(apps_filter::app_id(query.calling_uid), 1000 | 1001);
                if !privileged && !query.uid_has_permission(query.calling_uid, "android.permission.SUSPEND_APPS").map_err(issue)? { return Err(Exception::security("SUSPEND_APPS")); }
                let package = name.as_ref().and_then(|name| query.state.packages.get(name)).filter(|package| package.is.system && package.pkg.is_some());
                if let Some(package) = package {
                    if !privileged && package.pkg.as_ref().unwrap().booleans & super::pkg::booleans::CORE_APP != 0 { return Err(Exception::security("Only system or phone callers can modify core apps")); }
                    if let Some(user) = install_user {
                        let changed = info::user_state(package, user).installed != hidden && (dependencies.system_install_state)(&package.name, hidden, user)?;
                        result.reply = Parcel::new(); pm::write_set_system_app_install_state_reply(&mut result.reply, changed);
                    } else { result.global = Some(Global::HiddenUntilInstalled(package.name.clone(), hidden)); }
                } else if install_user.is_some() { result.reply = Parcel::new(); pm::write_set_system_app_install_state_reply(&mut result.reply, false); }
            }
            pm::SET_REQUIRED_FOR_SYSTEM_USER => {
                let args = pm::SetRequiredForSystemUser::read(reader).map_err(decode)?;
                if reader.remaining() != 0 { return Err(decode(BAD_VALUE)); }
                if !matches!(query.calling_uid, 0 | 1000) { return Err(Exception::security("setRequiredForSystemUser can only be run by the system or root")); }
                let exists = args.package_name.as_ref().is_some_and(|name| query.state.packages.contains_key(name));
                result.reply = Parcel::new(); pm::write_set_required_for_system_user_reply(&mut result.reply, exists);
                if exists { result.global = Some(Global::RequiredForSystemUser(args.package_name.unwrap(), args.system_user_app)); }
            }
            _ => {
                let request = Request::read(code, query.calling_uid as u32, reader).ok_or_else(|| decode(BAD_VALUE))?.map_err(decode)?;
                let restricted = match &request {
                    Request::Stopped { user, .. } | Request::HarmfulWarning { user, .. } if query.calling_uid == 2000 && *user >= 0 => Some((dependencies.shell_restricted)(*user)?),
                    _ => None,
                };
                let plan = if let Request::Enabled(setting) = &request {
                    let (mut policy, needs_compressed) = enabled_policy(query, setting, pid, dependencies)?;
                    if needs_compressed { policy.compressed_enabled = Some((dependencies.enable_compressed)(&setting.package, setting.user)?); }
                    request.decide_with_enabled_policy(query, pid, restricted, policy).map_err(issue)??
                } else { request.decide_with_shell(query, pid, restricted).map_err(issue)?? };
                if let Change::MimeGroup { group, .. } = &plan.change { result.finish = Finish::Mime { package: plan.package.clone(), group: group.clone() }; }
                else { result.finish = Finish::Single(request); }
                result.plans.push(plan);
            }
        }
        let disabled_users: Vec<_> = match &result.finish {
            Finish::Single(Request::Enabled(setting)) => vec![(setting.package.clone(), setting.user, setting.class.is_none(), setting.new_state)],
            Finish::Enabled(settings) => settings.iter().map(|setting| (setting.package.clone(), setting.user, setting.class.is_none(), setting.new_state)).collect(),
            _ => Vec::new(),
        };
        for (name, user, application, new_state) in disabled_users {
            if !application || !matches!(new_state, 2 | 3) || !result.plans.iter().any(|plan| plan.package == name && matches!(plan.change, Change::Enabled(_))) { continue; }
            let package = query.state.packages.get(&name).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "disabled package absent"))?;
            if !query.uid_has_permission(apps_filter::uid(user, package.app_id), "android.permission.SUSPEND_APPS").map_err(issue)? { continue; }
            // The pinned cross-user flag is false. The original USER_ALL
            // suspension cleanup observes its default -1 state (no owners).
            // The accompanying distraction cleanup uses this real user.
            let mut names = Vec::new(); let mut uids = Vec::new();
            for package in query.state.packages.values() {
                if info::user_state(package, user).distraction_flags == 0 { continue; }
                result.plans.push(Plan { package: package.name.clone(), user: Some(user), change: Change::Distraction(0) });
                names.push(package.name.clone()); uids.push(apps_filter::uid(user, package.app_id));
            }
            result.distraction_cleanup.push((names, uids, user));
        }
        Ok(result)
    })())
}
fn enabled_policy(query: &Query<'_>, setting: &enabled::Setting, pid: i32, dependencies: &Dependencies) -> Result<(enabled::Policy, bool), Exception> {
    let policy = enabled::Policy { protected: Some(dependencies.effects.state_protected(&setting.package, setting.user)?),
        shell_restricted: if query.calling_uid == 2000 { Some((dependencies.shell_restricted)(setting.user)?) } else { None },
        compressed_enabled: None, suspension_cleanup: true };
    let current = |package: &super::model::PackageState, user| {
        let state = info::user_state(package, user);
        super::write::Enabled { enabled: state.enabled, last_disable_app_caller: state.last_disable_app_caller,
            enabled_components: state.enabled_components.into_iter().collect(), disabled_components: state.disabled_components.into_iter().collect() }
    };
    // The read-only decision reaches this dedicated error only after all
    // cross-user, permission, protected/shell and component checks passed.
    match enabled::decide_with_policy(query, setting, pid, &current, Some(policy)) {
        Err(apps_filter::NotModelled("enabling a system stub, which installs it")) => {
            return Ok((policy, true));
        }
        Err(error) => return Err(issue(error)),
        Ok(Err(error)) => return Err(error),
        Ok(Ok(_)) => {}
    }
    Ok((policy, false))
}

impl Prepared {
    /// A retained plan may cross an unrelated publication. Validate every field
    /// it writes against its source before applying it to the latest owner.
    pub(crate) fn validate_rebase(&self, base:&SigningScan, latest:&SigningScan)->Result<(),String>{
        let mut desired=base.clone();self.apply_scan(&mut desired)?;
        for plan in &self.plans {
            let find=|scan:&SigningScan|scan.settings.packages.iter().find(|setting|setting.name==plan.package).cloned().ok_or("prepared mutation package removed");
            let old=find(base)?;let current=find(latest)?;let intended=find(&desired)?;
            if old.app_id!=current.app_id||old.signatures!=current.signatures||old.shared_app_id()!=current.shared_app_id(){return Err("prepared mutation package identity changed".into());}
            let check=|unchanged:bool,already:bool|if unchanged||already{Ok(())}else{Err(format!("prepared mutation concurrent field conflict: {}",plan.package))};
            macro_rules! field {($field:ident)=>{check(current.$field==old.$field,current.$field==intended.$field)?};}
            match &plan.change {
                Change::None=>{},Change::CategoryHint(_)=>field!(category_hint),Change::UpdateAvailable(_)=>field!(update_available),
                Change::RelinquishUpdateOwner=>check(current.install_source.update_owner==old.install_source.update_owner,current.install_source.update_owner==intended.install_source.update_owner)?,
                Change::MimeGroup{group,..}=>{
                    let values=|setting:&super::settings::Package|setting.mime_groups.iter().find(|(name,_)|name==group).map(|(_,types)|types.clone());
                    check(values(&current)==values(&old),values(&current)==values(&intended))?;
                },
                change=>{
                    let user=plan.user.ok_or("prepared mutation user missing")?;
                    let state=|scan:&SigningScan|scan.scanned_user_states(&plan.package).and_then(|users|users.get(&user)).cloned().ok_or("prepared mutation user removed");
                    let old=state(base)?;let current=state(latest)?;let intended=state(&desired)?;
                    macro_rules! user_field {($field:ident)=>{check(current.$field==old.$field,current.$field==intended.$field)?};}
                    match change {
                        Change::Enabled(_)=>{user_field!(enabled);user_field!(last_disable_app_caller);user_field!(enabled_components);user_field!(disabled_components);},
                        Change::Stopped{..}=>{user_field!(stopped);user_field!(not_launched);},
                        Change::Suspensions(_)=>user_field!(suspensions),Change::Distraction(_)=>user_field!(distraction_flags),Change::Hidden(_)=>user_field!(hidden),
                        Change::SplashTheme(_)=>user_field!(splash_screen_theme),Change::HarmfulWarning(_)=>user_field!(harmful_app_warning),Change::MinAspectRatio(_)=>user_field!(min_aspect_ratio),
                        _=>return Err("prepared mutation rebase field owner unavailable".into()),
                    }
                }
            }
        }
        if let Some(global)=&self.global {
            let name=match global{Global::HiddenUntilInstalled(name,_)|Global::RequiredForSystemUser(name,_)=>name};
            for factory in [false,true] {
                let find=|scan:&SigningScan|{let list=if factory{&scan.settings.disabled_system_packages}else{&scan.settings.packages};list.iter().find(|setting|setting.name==*name).cloned()};
                let old=find(base);let current=find(latest);let intended=find(&desired);
                match (old,current,intended){(None,None,None)=>{},(Some(old),Some(current),Some(intended))=>{
                    let valid=match global{Global::HiddenUntilInstalled(..)=>current.transient.hidden_until_installed==old.transient.hidden_until_installed||current.transient.hidden_until_installed==intended.transient.hidden_until_installed,
                        Global::RequiredForSystemUser(..)=>(current.private_flags&512)==(old.private_flags&512)||(current.private_flags&512)==(intended.private_flags&512)};
                    if !valid{return Err("prepared global mutation concurrent field conflict".into());}
                },_=>return Err("prepared global mutation scope changed".into())}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod prepared_rebase_tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn stopped_plan_preserves_concurrent_inode_and_rejects_enabled_conflict() {
        let mut base=SigningScan::new(&Default::default(),&super::super::settings::Settings{
            packages:vec![super::super::settings::Package{name:"p".into(),app_id:10100,..Default::default()}],..Default::default()},36).unwrap();
        base.capture_user_states(BTreeMap::from([(("p".into(),false),super::super::scan::CapturedUsers{
            states:BTreeMap::from([(0,super::super::restrictions::UserState{stopped:true,not_launched:true,..Default::default()})]),active_aliases:Default::default(),
        })])).unwrap();
        let prepared=Prepared{code:0,plans:vec![Plan{package:"p".into(),user:Some(0),change:Change::Stopped{stopped:false,not_launched:false,first_launch_installer:None,was_stopped:true}}],global:None,finish:Finish::None,reply:Parcel::new(),distraction_cleanup:vec![]};
        let mut latest=base.clone();let mut state=latest.scanned_user_states("p").unwrap()[&0].clone();state.ce_data_inode=99;state.de_data_inode=101;
        latest.set_user_state("p",0,state).unwrap();prepared.validate_rebase(&base,&latest).unwrap();prepared.apply_scan(&mut latest).unwrap();
        let state=&latest.scanned_user_states("p").unwrap()[&0];assert!(!state.stopped);assert!(!state.not_launched);assert_eq!(state.ce_data_inode,99);assert_eq!(state.de_data_inode,101);
        // Boolean writes are idempotent when the latest owner already reached
        // the intended value; compound enabled state has real conflict values.
        prepared.validate_rebase(&base,&latest).unwrap();
        let enabled=Prepared{code:0,plans:vec![Plan{package:"p".into(),user:Some(0),change:Change::Enabled(super::super::write::Enabled{enabled:1,last_disable_app_caller:None,enabled_components:Default::default(),disabled_components:Default::default()})}],global:None,finish:Finish::None,reply:Parcel::new(),distraction_cleanup:vec![]};
        let mut state=latest.scanned_user_states("p").unwrap()[&0].clone();state.enabled=2;latest.set_user_state("p",0,state).unwrap();
        assert!(enabled.validate_rebase(&base,&latest).unwrap_err().contains("concurrent field conflict"));
        assert_eq!(latest.scanned_user_states("p").unwrap()[&0].enabled,2);
    }
}

/// Classify actual lower-owner installation work without consuming request data.
/// Component mutations never decompress a stub; only changing an application's
/// enabled state from its current value can enter that original owner branch.
pub(crate) fn owns_install_side_effect(code:u32,reader:&mut Reader<'_>,query:&Query<'_>)->bool{
    let position=reader.position();
    let result=(||{
        if code==pm::SET_SYSTEM_APP_INSTALL_STATE{return true;}
        let settings=if code==pm::SET_COMPONENT_ENABLED_SETTINGS {
            let Ok(batch)=enabled::Batch::read(query.calling_uid,reader)else{return false};
            let Ok(settings)=batch.settings(query)else{return false};settings
        }else{
            let Some(setting)=enabled::Setting::read(code,query.calling_uid as u32,reader)else{return false};vec![setting]
        };
        settings.iter().any(|setting|{
            setting.class.is_none()&&matches!(setting.new_state,0|1)
                &&query.state.packages.get(&setting.package).is_some_and(|package|
                    package.is.system&&package.pkg.as_ref().is_some_and(|code|code.is2(super::pkg::booleans2::STUB))
                        &&info::user_state(package,setting.user).enabled!=setting.new_state)
        })
    })();
    reader.set_position(position);result
}

#[cfg(test)]
mod enabled_install_gate_tests {
    use super::*;
    #[test]
    fn component_setters_stay_atomic_even_when_target_is_a_compressed_system_stub(){
        use super::super::{model::{State,PackageState,StateFlags,PackageUserState},pkg::{AndroidPackage,booleans2}};
        let mut state=State::default();
        state.packages.insert("p".into(),PackageState{name:"p".into(),app_id:10100,is:StateFlags{system:true,..Default::default()},
            pkg:Some(Arc::new(AndroidPackage{package_name:"p".into(),booleans2:booleans2::STUB,..Default::default()})),
            users:std::collections::BTreeMap::from([(0,PackageUserState{enabled:2,..Default::default()})]),..Default::default()});
        let filter=apps_filter::AppsFilter::new(&state,&Default::default()).unwrap();
        let query=Query{state:&state,filter:&filter,calling_uid:1000};
        let mut component=Parcel::new();component.write_interface_token(pm::DESCRIPTOR);component.write_i32(1);component.write_string16(Some("p"));component.write_string16(Some("p.Component"));component.write_i32(1);component.write_i32(1);component.write_i32(0);component.write_string16(None);
        let mut reader=Reader::new(component.data(),component.objects());let position=reader.position();
        assert!(!owns_install_side_effect(pm::SET_COMPONENT_ENABLED_SETTING,&mut reader,&query));
        assert_eq!(reader.position(),position);
        let mut application=Parcel::new();pm::SetApplicationEnabledSetting{package_name:Some("p".into()),new_state:1,flags:0,user_id:0,calling_package:None}.write(&mut application);
        let mut reader=Reader::new(application.data(),application.objects());
        assert!(owns_install_side_effect(pm::SET_APPLICATION_ENABLED_SETTING,&mut reader,&query));
        assert_eq!(reader.position(),0);
        let mut noop=Parcel::new();pm::SetApplicationEnabledSetting{package_name:Some("p".into()),new_state:2,flags:0,user_id:0,calling_package:None}.write(&mut noop);
        assert!(!owns_install_side_effect(pm::SET_APPLICATION_ENABLED_SETTING,&mut Reader::new(noop.data(),noop.objects()),&query));
    }
}
