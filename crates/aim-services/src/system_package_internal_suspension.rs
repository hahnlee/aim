//! Original PackageManagerInternal suspension/distraction mutation bodies.
//! Included as a child of system.rs so the common generation gate stays shared.
use super::*;
use crate::package::{apps_filter, effects, info, mutation_dispatch::{Prepared, Finish},
    restrictions::UserState, suspension, write::mutation::{Change, Plan, Request}};

impl System {
    fn internal_suspension_context(&self, uid: i32, pid: i32) -> Result<(Arc<crate::package::bootstrap::Bridge>, Arc<crate::package::scan_snapshot::query_state::Capture>, Arc<crate::package::mutation_dispatch::Dependencies>)> {
        if uid < 0 || pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
        let state = self.package_bootstrap.lock().unwrap();
        let owner = state.current.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal package bootstrap unavailable"))?;
        let capture = owner.queries.clone().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal native package capture unavailable"))?;
        let dependencies = owner.mutations.clone().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal native mutation owners unavailable"))?;
        Ok((owner.bridge.clone(), capture, dependencies))
    }
    fn internal_remove_suspension_owners(&self, package: Option<String>, user: i32, admin: bool, uid: i32, pid: i32) -> Result<()> {
        let install = self.package_install_guard();
        let (bridge, capture, dependencies) = self.internal_suspension_context(uid, pid)?;
        self.check_package_bootstrap(&bridge)?;
        let mut plans = Vec::new(); let mut unsuspended = Vec::new(); let mut uids = Vec::new();
        let names: Vec<_> = match package { Some(name) => vec![name], None => capture.scan().owner().settings.packages.iter().map(|package| package.name.clone()).collect() };
        for name in names {
            let Some(package) = capture.state().packages.get(&name) else { continue; };
            let state = info::user_state(package, user);
            let mut raw = UserState { suspensions: state.suspensions.clone(), ..Default::default() };
            let entries = raw.resolved_suspensions(user, false);
            if entries.is_empty() { continue; }
            let remove: Vec<_> = entries.iter().filter(|(suspender_user, entry)| {
                if admin { *suspender_user == user && entry.package == "android" } else { entry.package != "android" }
            }).map(|(owner_user, entry)| (*owner_user, entry.package.clone())).collect();
            if remove.is_empty() { continue; }
            if remove.len() == entries.len() { unsuspended.push(name.clone()); uids.push(apps_filter::uid(user, package.app_id)); }
            for (owner_user, owner_package) in remove { raw.remove_suspension(user, false, owner_user, &owner_package); }
            plans.push(Plan { package: name, user: Some(user), change: Change::Suspensions(raw.suspensions) });
        }
        let mut reply = Parcel::new(); reply.write_no_exception();
        let prepared = Prepared { code: 0, plans, global: None, finish: Finish::None, reply, distraction_cleanup: Vec::new() };
        self.publish_prepared_package_mutation_guarded(&bridge, &capture, prepared, &dependencies, uid, install)?;
        {
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|owner| Arc::ptr_eq(&owner.bridge, &bridge)).ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal suspension bootstrap changed"))?;
            // The original schedules restrictions even if no suspension matched.
            current.dirty_restrictions.insert(user);
        }
        if !unsuspended.is_empty() {
            // Original cleanup sends MY_PACKAGE_UNSUSPENDED before its aggregate
            // PACKAGES_UNSUSPENDED event, without a suspension-changed event.
            dependencies.effects.removed_suspensions(&unsuspended, &uids, user)?;
        }
        Ok(())
    }
    pub(crate) fn internal_remove_all_non_system_package_suspensions(&self, user_id: i32, calling_uid: i32, calling_pid: i32) -> Result<()> {
        self.internal_remove_suspension_owners(None, user_id, false, calling_uid, calling_pid)
    }
    pub(crate) fn internal_remove_non_system_package_suspensions(&self, package_name: Option<String>, user_id: i32, calling_uid: i32, calling_pid: i32) -> Result<()> {
        // A null single-package argument denotes an unknown target, not USER_ALL.
        if package_name.is_none() { self.internal_suspension_context(calling_uid, calling_pid)?; return Ok(()); }
        self.internal_remove_suspension_owners(package_name, user_id, false, calling_uid, calling_pid)
    }
    pub(crate) fn internal_unsuspend_admin_suspended_packages(&self, user_id: i32, calling_uid: i32, calling_pid: i32) -> Result<()> {
        self.internal_remove_suspension_owners(None, user_id, true, calling_uid, calling_pid)
    }
    fn internal_clear_distraction(&self, package: Option<String>, all: bool, user: i32, uid: i32, pid: i32) -> Result<()> {
        let install = self.package_install_guard();
        let (bridge, capture, dependencies) = self.internal_suspension_context(uid, pid)?;
        let names: Vec<_> = if all { capture.scan().owner().settings.packages.iter().map(|package| package.name.clone()).collect() } else { package.into_iter().collect() };
        let mut plans = Vec::new(); let mut names_changed = Vec::new(); let mut uids = Vec::new();
        for name in names {
            let Some(package) = capture.state().packages.get(&name) else { continue; };
            if info::user_state(package, user).distraction_flags == 0 { continue; }
            plans.push(Plan { package: name.clone(), user: Some(user), change: Change::Distraction(0) });
            names_changed.push(name); uids.push(apps_filter::uid(user, package.app_id));
        }
        let changed = !plans.is_empty();
        let mut reply = Parcel::new(); reply.write_no_exception();
        let prepared = Prepared { code: 0, plans, global: None, finish: Finish::Distraction { names: names_changed, uids, user, flags: 0 }, reply, distraction_cleanup: Vec::new() };
        self.publish_prepared_package_mutation_guarded(&bridge, &capture, prepared, &dependencies, uid, install)?;
        if changed {
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|owner| Arc::ptr_eq(&owner.bridge, &bridge)).ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "internal distraction bootstrap changed"))?;
            current.dirty_restrictions.insert(user);
        }
        Ok(())
    }
    pub(crate) fn internal_remove_distracting_package_restrictions(&self, package_name: Option<String>, user_id: i32, calling_uid: i32, calling_pid: i32) -> Result<()> {
        self.internal_clear_distraction(package_name, false, user_id, calling_uid, calling_pid)
    }
    pub(crate) fn internal_remove_all_distracting_package_restrictions(&self, user_id: i32, calling_uid: i32, calling_pid: i32) -> Result<()> {
        self.internal_clear_distraction(None, true, user_id, calling_uid, calling_pid)
    }
    pub(crate) fn internal_set_packages_suspended_by_admin(&self, user_id: i32, packages: Option<Vec<Option<String>>>, suspended: bool, calling_uid: i32, calling_pid: i32) -> Result<Option<Vec<Option<String>>>> {
        let install = self.package_install_guard();
        let (bridge, capture, dependencies) = self.internal_suspension_context(calling_uid, calling_pid)?;
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(capture.state()).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("internal suspension resolution: {error:?}")))?;
        // This internal operation explicitly uses original Process.SYSTEM_UID,
        // independent of its Binder caller, and attributes to the platform.
        let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: 1000 };
        let request = suspension::Request { packages, suspended, app_extras: None, launcher_extras: None, dialog: None, flags: 0,
            suspender: Some("android".into()), suspending_user: user_id, user: user_id };
        let batch = request.prepare(&query, &dependencies.effects, dependencies.quarantine_enabled)?;
        let rejected = batch.rejected.clone();
        let mut reply = Parcel::new(); reply.write_no_exception();
        let prepared = Prepared { code: 0, plans: batch.plans.clone(), global: None, finish: Finish::Suspension(batch), reply, distraction_cleanup: Vec::new() };
        self.publish_prepared_package_mutation_guarded(&bridge, &capture, prepared, &dependencies, 1000, install)?;
        Ok(rejected)
    }
    pub(crate) fn internal_set_package_stopped_state(&self, package_name: Option<String>, stopped: bool, user_id: i32, calling_uid: i32, calling_pid: i32) -> Result<()> {
        let install = self.package_install_guard();
        let (bridge, capture, dependencies) = self.internal_suspension_context(calling_uid, calling_pid)?;
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(capture.state()).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("internal stopped resolution: {error:?}")))?;
        let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid };
        let package = package_name.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName"))?;
        let request = Request::Stopped { package, user: user_id, stopped };
        let restricted = if calling_uid == 2000 && user_id >= 0 { Some(self.package_shell_debugging_policy(user_id)?) } else { None };
        let plan = request.decide_with_shell(&query, calling_pid, restricted).map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))??;
        let mut reply = Parcel::new(); reply.write_no_exception();
        let prepared = Prepared { code: 0, plans: vec![plan], global: None, finish: Finish::Single(request), reply, distraction_cleanup: Vec::new() };
        self.publish_prepared_package_mutation_guarded(&bridge, &capture, prepared, &dependencies, calling_uid, install).map(|_| ())
    }
    pub(crate) fn internal_set_visibility_logging(&self, package_name: Option<String>, enabled: bool, calling_uid: i32, calling_pid: i32) -> Result<()> {
        if !matches!(calling_uid, 0 | 1000 | 2000) { return Err(Exception::security("Only the system or shell can set visibility logging.")); }
        let (bridge, capture, _) = self.internal_suspension_context(calling_uid, calling_pid)?;
        let package = package_name.as_ref().and_then(|name| capture.state().packages.get(name)).ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("No package found for {}", package_name.as_deref().unwrap_or("null"))))?;
        self.check_package_bootstrap(&bridge)?;
        // Logging is keyed by appId, so all packages sharing the UID participate.
        let logging = capture.state().system.visibility_logging.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native AppsFilter logging owner unavailable"))?;
        logging.enable(package.app_id, enabled);
        Ok(())
    }
}
