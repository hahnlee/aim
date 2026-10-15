//! SuspendPackageHelper decisions with retained live policy/effect owners.
//! AOSP android-16.0.0_r1, Apache License 2.0.
use super::{apps_filter, effects, info, query::Query, restrictions::{UserState, SuspendParams, persistable::Bundle, dialog::DialogInfo}, write::mutation::{Change, Plan}};
use aim_binder_host::parcel::{Exception, Reader, BAD_VALUE};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

#[derive(Debug)]
pub struct Request {
    pub packages: Option<Vec<Option<String>>>, pub suspended: bool, pub app_extras: Option<Bundle>,
    pub launcher_extras: Option<Bundle>, pub dialog: Option<DialogInfo>, pub flags: i32,
    pub suspender: Option<String>, pub suspending_user: i32, pub user: i32,
}
#[derive(Debug)]
pub struct Batch {
    pub rejected: Option<Vec<Option<String>>>, pub plans: Vec<Plan>,
    pub notify: Vec<String>, pub notify_uids: Vec<i32>, pub changed: Vec<String>, pub changed_uids: Vec<i32>,
    pub user: i32, pub suspended: bool, pub quarantined: bool,
}
impl Request {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let args = pm::SetPackagesSuspendedAsUser::<Bundle, DialogInfo>::read(reader)?;
        if reader.remaining() != 0 { return Err(BAD_VALUE); }
        Ok(Self { packages: args.package_names, suspended: args.suspended, app_extras: args.app_extras,
            launcher_extras: args.launcher_extras, dialog: args.dialog_info, flags: args.flags,
            suspender: args.suspending_package, suspending_user: args.suspending_user_id, user: args.target_user_id })
    }
    pub fn prepare(&self, query: &Query<'_>, effects: &effects::Owner, quarantine_enabled: bool) -> Result<Batch, Exception> {
        let uid = query.calling_uid;
        let quarantined = quarantine_enabled && self.flags & 1 != 0;
        let owner = effects.owner_package(self.user)?;
        let is_owner = owner.as_ref().is_some_and(|name| query.state.packages.get(name).is_some_and(|p| apps_filter::uid(self.user, p.app_id) == uid));
        if uid != 0 && apps_filter::app_id(uid) != 1000 && !is_owner {
            let permission = if quarantined { "android.permission.QUARANTINE_APPS" } else { "android.permission.SUSPEND_APPS" };
            if !query.uid_has_permission(uid, permission).map_err(unmodelled)? { return Err(Exception::security(permission)); }
            let suspender = self.suspender.as_deref().unwrap_or_default();
            let package_uid = match query.package_uid(suspender, 0, self.user).map_err(unmodelled)? { Ok(uid) => uid, Err(error) => return Err(error) };
            if package_uid != uid && !(uid == 2000 && apps_filter::app_id(package_uid) == uid) {
                return Err(Exception::security("Suspending package does not belong to calling uid"));
            }
        }
        let mut batch = Batch { rejected: self.packages.as_ref().map(|_| Vec::new()), plans: Vec::new(), notify: Vec::new(), notify_uids: Vec::new(),
            changed: Vec::new(), changed_uids: Vec::new(), user: self.user, suspended: self.suspended, quarantined };
        let Some(packages) = self.packages.as_ref().filter(|p| !p.is_empty()) else { return Ok(batch); };
        if self.suspended && !quarantined && !effects.suspension_allowed(self.user, uid)? {
            batch.rejected = self.packages.clone(); return Ok(batch);
        }
        let suspender = self.suspender.as_deref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null suspending package"))?;
        let can_suspend = if self.suspended { Some(effects.can_suspend(packages, self.user, uid)?) } else { None };
        let parameters = self.suspended.then(|| SuspendParams { dialog: self.dialog.clone(), app_extras: self.app_extras.clone(), launcher_extras: self.launcher_extras.clone(), quarantined });
        for (index, name) in packages.iter().enumerate() {
            let reject = |batch: &mut Batch| batch.rejected.as_mut().unwrap().push(name.clone());
            let Some(name) = name.as_deref() else { reject(&mut batch); continue; };
            if name == suspender { reject(&mut batch); continue; }
            let Some(package) = query.state.packages.get(name) else { reject(&mut batch); continue; };
            let state = info::user_state(package, self.user);
            if !state.installed || query.filtered_including_uninstalled(Some(package), self.user).map_err(unmodelled)?
                || can_suspend.as_ref().is_some_and(|values| !values[index]) { reject(&mut batch); continue; }
            let mut raw = UserState { suspensions: state.suspensions.clone(), ..Default::default() };
            let entries = raw.resolved_suspensions(self.user, false);
            let previous = entries.iter().find(|(user, entry)| *user == self.user && entry.package == suspender).and_then(|(_, entry)| entry.params.as_ref());
            let changed = previous != parameters.as_ref();
            let fully_unsuspended = !self.suspended && entries.len() == 1 && entries[0].0 == self.user && entries[0].1.package == suspender;
            if self.suspended || fully_unsuspended { batch.notify.push(name.into()); batch.notify_uids.push(apps_filter::uid(self.user, package.app_id)); }
            if !changed { continue; }
            if self.suspended { raw.put_suspension(self.user, false, self.user, suspender.into(), parameters.clone()); }
            else { raw.remove_suspension(self.user, false, self.user, suspender); }
            batch.plans.push(Plan { package: name.into(), user: Some(self.user), change: Change::Suspensions(raw.suspensions) });
            batch.changed.push(name.into()); batch.changed_uids.push(apps_filter::uid(self.user, package.app_id));
        }
        batch.changed.sort_by_key(|name| info::java_hash(name));
        Ok(batch)
    }
}
impl Batch {
    pub fn finish(&self, owner: &effects::Owner) -> Result<(), Exception> {
        if !self.notify.is_empty() { owner.suspended(&self.notify, &self.notify_uids, self.user, self.suspended, self.quarantined, false)?; }
        if !self.changed.is_empty() { owner.suspended(&self.changed, &self.changed_uids, self.user, self.suspended, self.quarantined, true)?; }
        Ok(())
    }
}
fn unmodelled(error: apps_filter::NotModelled) -> Exception { Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0) }

#[derive(Debug)]
pub struct Distraction { pub packages: Option<Vec<Option<String>>>, pub flags: i32, pub user: i32 }
impl Distraction {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let args = pm::SetDistractingPackageRestrictionsAsUser::read(reader)?;
        if reader.remaining() != 0 { return Err(BAD_VALUE); }
        Ok(Self { packages: args.package_names, flags: args.restriction_flags, user: args.user_id })
    }
    pub fn prepare(&self, query: &Query<'_>, owner: &effects::Owner) -> Result<Batch, Exception> {
        let uid = query.calling_uid;
        if !matches!(uid, 0 | 1000) && !query.uid_has_permission(uid, "android.permission.SUSPEND_APPS").map_err(unmodelled)? { return Err(Exception::security("SUSPEND_APPS")); }
        if !matches!(uid, 0 | 1000) && apps_filter::user_id(uid) != self.user { return Err(Exception::security("Caller cannot change distraction flags across users")); }
        let packages = self.packages.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageNames cannot be null"))?;
        let mut batch = Batch { rejected: Some(Vec::new()), plans: Vec::new(), notify: Vec::new(), notify_uids: Vec::new(), changed: Vec::new(), changed_uids: Vec::new(), user: self.user, suspended: false, quarantined: false };
        if packages.is_empty() { return Ok(batch); }
        if self.flags != 0 && !owner.suspension_allowed(self.user, uid)? { batch.rejected = Some(packages.clone()); return Ok(batch); }
        let can_suspend = if self.flags != 0 { Some(owner.can_suspend(packages, self.user, uid)?) } else { None };
        for (index, name) in packages.iter().enumerate() {
            let Some(package) = name.as_ref().and_then(|name| query.state.packages.get(name)) else { batch.rejected.as_mut().unwrap().push(name.clone()); continue; };
            let state = info::user_state(package, self.user);
            if !state.installed || query.filtered_including_uninstalled(Some(package), self.user).map_err(unmodelled)? || can_suspend.as_ref().is_some_and(|values| !values[index]) {
                batch.rejected.as_mut().unwrap().push(name.clone()); continue;
            }
            if state.distraction_flags == self.flags { continue; }
            batch.plans.push(Plan { package: package.name.clone(), user: Some(self.user), change: Change::Distraction(self.flags) });
            batch.notify.push(package.name.clone()); batch.notify_uids.push(apps_filter::uid(self.user, package.app_id));
        }
        Ok(batch)
    }
}

fn enforce_unsuspendable_permissions(query: &Query<'_>, user: i32) -> Result<(), Exception> {
    let uid = query.calling_uid;
    if !matches!(uid, 0 | 1000) && !query.uid_has_permission(uid, "android.permission.SUSPEND_APPS").map_err(unmodelled)? {
        return Err(Exception::security("SUSPEND_APPS"));
    }
    // PMS's method directly compares UserHandle.getUserId(callingUid), then
    // requires FULL for another user. Computer's general cross-user helper
    // also allows the weaker permission and has a distinct same-user switch.
    if apps_filter::user_id(uid) != user && !matches!(uid, 0 | 1000)
        && !query.uid_has_permission(uid, "android.permission.INTERACT_ACROSS_USERS_FULL").map_err(unmodelled)? {
        return Err(Exception::security(format!("Calling uid {uid} cannot query getUnsuspendablePackagesForUser for user {user}")));
    }
    Ok(())
}

pub fn unsuspendable(query: &Query<'_>, owner: &effects::Owner, packages: &[Option<String>], user: i32) -> Result<Vec<Option<String>>, Exception> {
    let uid = query.calling_uid;
    enforce_unsuspendable_permissions(query, user)?;
    if !owner.suspension_allowed(user, uid)? { return Ok(packages.to_vec()); }
    let allowed = owner.can_suspend(packages, user, uid)?;
    let mut rejected = Vec::new();
    for (index, name) in packages.iter().enumerate() {
        let package = name.as_ref().and_then(|name| query.state.packages.get(name));
        if !allowed[index] || package.is_none_or(|package| !info::user_state(package, user).installed)
            || query.filtered_including_uninstalled(package, user).map_err(unmodelled)? {
            if !rejected.contains(name) { rejected.push(name.clone()); }
        }
    }
    rejected.sort_by_key(|name| name.as_deref().map(info::java_hash).unwrap_or(0));
    Ok(rejected)
}

#[cfg(test)]
mod unsuspendable_permission_tests {
    use super::*;
    use crate::package::{apps_filter::{AppsFilter,Config},model::{State,PackageState,PackageUserState}};
    use aim_binder_host::parcel::EX_SECURITY;
    fn check(uid:i32,user:i32,grants:&[&str])->Result<(),Exception>{
        let caller_user=apps_filter::user_id(uid);
        let package=PackageState{name:"permission.fixture".into(),app_id:apps_filter::app_id(uid),users:[(caller_user,PackageUserState{
            granted_permissions:grants.iter().map(|grant|(*grant).to_owned()).collect(),..Default::default()})].into(),..Default::default()};
        let state=State{packages:[(package.name.clone(),package)].into(),..Default::default()};
        let filter=AppsFilter::new(&state,&Config::default()).unwrap();
        enforce_unsuspendable_permissions(&Query{state:&state,filter:&filter,calling_uid:uid},user)
    }
    #[test]
    fn own_user_needs_suspend_permission_and_other_user_requires_full(){
        const SUSPEND:&str="android.permission.SUSPEND_APPS";
        const PARTIAL:&str="android.permission.INTERACT_ACROSS_USERS";
        const FULL:&str="android.permission.INTERACT_ACROSS_USERS_FULL";
        check(10151,0,&[SUSPEND]).unwrap();
        check(1010151,10,&[SUSPEND]).unwrap();
        assert_eq!(check(10151,0,&[]).unwrap_err().code,EX_SECURITY);
        assert_eq!(check(10151,10,&[SUSPEND]).unwrap_err().code,EX_SECURITY);
        assert_eq!(check(10151,10,&[SUSPEND,PARTIAL]).unwrap_err().code,EX_SECURITY);
        check(10151,10,&[SUSPEND,FULL]).unwrap();
        assert_eq!(check(10151,10,&[FULL]).unwrap_err().message,"SUSPEND_APPS");
        check(0,10,&[]).unwrap();check(1000,10,&[]).unwrap();
    }
}
