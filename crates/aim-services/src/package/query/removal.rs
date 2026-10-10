//! Public IPackageManager deletion, distinct from installer caller/IntentSender
//! admission. AOSP android-16.0.0_r1 DeletePackageHelper, Apache-2.0.
use crate::package::{apps_filter, events, installer::{policy, removal::{self, Controller, Plan, Request, VersionedPackage}}, query::Query, resolve::Resolver};
use aim_binder_host::{local::{Call, LocalProcess, LocalService, Reply, Service, Strong}, parcel::{Binder, Exception, Parcel, Reader, BAD_VALUE, UNKNOWN_TRANSACTION, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER}};
use aim_service_aidl::{android_content_pm_ipackagemanager as pm, android_content_pm_ipackagedeleteobserver2 as observer, android_content_pm_ipackagedeleteobserver as legacy_observer};
use std::sync::{Arc, Mutex};

enum Target { Local(LocalService), Remote(Strong) }
struct Observer { binder: Binder, target: Target }
impl Observer {
    fn retain(process: &LocalProcess, binder: Binder) -> Result<Self, Exception> {
        let target = match binder {
            Binder::Local(pointer) => Target::Local(process.local_service(pointer).ok_or_else(|| illegal("Unknown deletion observer"))?),
            Binder::Handle(handle) => Target::Remote(process.strong(handle)),
        };
        Ok(Self { binder, target })
    }
    fn deleted(&self, package: &str, status: i32, message: Option<&str>) -> Result<(), Exception> {
        let mut request = Parcel::new();
        observer::OnPackageDeleted { package_name: Some(package.into()), return_code: status, msg: message.map(str::to_owned) }.write(&mut request);
        let result = match &self.target {
            Target::Local(target) => target.transact(observer::ON_PACKAGE_DELETED, &request, true),
            Target::Remote(target) => target.transact(observer::ON_PACKAGE_DELETED, &request, true),
        };
        result.map(|_| ()).map_err(|status| illegal(format!("Deletion observer callback: {status}")))
    }
}
/// PackageManager.LegacyPackageDeleteObserver adapts the old oneway callback.
/// Its inherited user-action callback is intentionally empty in the pinned API.
struct LegacyObserver {
    legacy: Option<Target>,
    owner: std::sync::Weak<Owner>,
}
impl Service for LegacyObserver {
    fn descriptor(&self) -> &str { observer::DESCRIPTOR }
    fn accepts_fds(&self) -> bool { true }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            observer::ON_PACKAGE_DELETED => {
                let args = observer::OnPackageDeleted::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                if let Some(target) = &self.legacy {
                    let mut request = Parcel::new();
                    legacy_observer::PackageDeleted { package_name: args.package_name,
                        return_code: args.return_code }.write(&mut request);
                    let result = match target {
                        Target::Local(target) => target.transact(legacy_observer::PACKAGE_DELETED, &request, true),
                        Target::Remote(target) => target.transact(legacy_observer::PACKAGE_DELETED, &request, true),
                    };
                    // The original catches RemoteException and completes the adapter.
                    // Retain diagnostics without changing the deletion result.
                    if let Err(status) = result {
                        if let Some(owner) = self.owner.upgrade() {
                            owner.errors.lock().unwrap().push(illegal(format!("Legacy deletion observer callback: {status}")));
                        }
                    }
                }
                Ok(Parcel::new())
            }
            observer::ON_USER_ACTION_REQUIRED => {
                observer::OnUserActionRequired::<crate::package::intent::Intent>::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(BAD_VALUE); }
                Ok(Parcel::new())
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

pub struct Owner {
    pub controller: Arc<Controller>, pub process: Arc<LocalProcess>, pub events: Arc<events::Owner>,
    resolver: Resolver, errors: Mutex<Vec<Exception>>,
}
enum Decision { Action, Status(i32), Delete(Plan) }
impl Owner {
    pub fn new(controller: Arc<Controller>, process: Arc<LocalProcess>, events: Arc<events::Owner>) -> Arc<Self> {
        Arc::new(Self { controller, process, events, resolver: Resolver::default(), errors: Mutex::new(vec![]) })
    }
    fn decide(&self, query: &Query<'_>, request: Request) -> Result<Decision, Exception> {
        let captured = (self.controller.policy)(query, &request)?;
        let internal = query.resolve_internal_package_name(&request.package, request.version);
        let package = query.state.packages.get(&internal);
        if request.existing_only {
            let installed = package.map_or(0, |package| captured.users.iter().filter(|user| package.users.get(user).is_none_or(|state| state.installed)).count());
            if installed <= 1 { return Ok(Decision::Status(removal::INTERNAL_ERROR)) }
        }
        if request.version < -1 { return Err(Exception::new(EX_ILLEGAL_ARGUMENT, "versionCode must be >= -1")) }
        if captured.pinned { return Ok(Decision::Status(removal::APP_PINNED)) }
        let mut users = if request.flags & removal::ALL_USERS != 0 { captured.users.clone() } else { vec![request.user] };
        let caller_user = apps_filter::user_id(query.calling_uid);
        let package_uid_matches = |name: Option<&str>| -> Result<bool, Exception> {
            let Some(name) = name else { return Ok(false) };
            Ok(query.package_uid_internal(name, 0, caller_user, 1000).map_err(policy::unknown)? == query.calling_uid)
        };
        let mut silent = request.existing_only || matches!(request.uid, 0 | 2000) || apps_filter::app_id(query.calling_uid) == 1000
            || package.is_some_and(|package| package.install_source.is_orphaned);
        if !silent {
            silent = package_uid_matches(package.and_then(|package| package.install_source.installer.as_deref()))?;
            for name in &captured.verifier_packages { if package_uid_matches(Some(name))? { silent = true; break; } }
            silent |= package_uid_matches(captured.uninstaller_package.as_deref())? || package_uid_matches(captured.storage_manager_package.as_deref())?
                || policy::permission(query, "android.permission.MANAGE_PROFILE_AND_DEVICE_OWNERS")?;
        }
        if !silent { return Ok(Decision::Action) }
        for &user in &users {
            policy::cross_user(query, &captured.device, user, false, "deletePackage")?;
            if captured.admins.contains(&user) { return Ok(Decision::Status(removal::DEVICE_POLICY)) }
            if captured.protected.contains(&user) { return Ok(Decision::Status(removal::INTERNAL_ERROR)) }
        }
        if captured.uninstall_restricted.contains(&request.user) { return Ok(Decision::Status(removal::USER_RESTRICTED)) }
        let blocks = query.state.system.uninstall_blocks.as_ref().ok_or_else(|| illegal("Uninstall block owner unavailable"))?;
        let blocked: Vec<_> = users.iter().copied().filter(|user| blocks.get(*user, Some(&internal))).collect();
        if request.flags & removal::ALL_USERS == 0 && !blocked.is_empty() { return Ok(Decision::Status(removal::OWNER_BLOCKED)) }
        users.retain(|user| !blocked.contains(user));
        if request.flags & removal::ALL_USERS == 0 {
            for &child in captured.child_users.get(&request.user).into_iter().flatten() {
                if package.is_some_and(|package| package.users.get(&child).is_some_and(|state| state.installed)) && !users.contains(&child) { users.push(child); }
            }
        }
        if package.is_some_and(|package| package.users.get(&caller_user).is_some_and(|state| state.instant_app))
            && !query.internal_can_view_instant(query.calling_uid, request.user).map_err(policy::unknown)? {
            return Ok(Decision::Status(removal::INTERNAL_ERROR));
        }
        Ok(Decision::Delete(Plan { request, internal_package: internal, users, blocked_users: blocked, keep_uninstalled: captured.keep_uninstalled }))
    }
    fn invoke(self: &Arc<Self>, versioned: Option<VersionedPackage>, observer: Option<Binder>, uid: u32, pid: i32, user: i32, flags: i32, existing: bool) -> Result<(), Exception> {
        // Public DELETE_PACKAGES is mandatory even for device/profile owners.
        if !self.controller.external.permission("android.permission.DELETE_PACKAGES", pid, uid)? { return Err(Exception::security("deletePackage requires DELETE_PACKAGES")) }
        let versioned = versioned.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null versionedPackage"))?;
        let observer = Observer::retain(&self.process, observer.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null observer"))?)?;
        let request = Request { package: versioned.name.clone(), version: versioned.version, caller_package: None, uid, pid, user, flags, existing_only: existing };
        let state = (self.controller.source)()?;
        let resolution = self.resolver.resolution(&state).map_err(|error| illegal(format!("Public deletion resolution: {error:?}")))?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: uid as i32 };
        let decision = self.decide(&query, request)?;
        let owner = self.clone(); let package = versioned.name;
        self.events.post(false, Box::new(move || {
            let result = match decision {
                Decision::Action => owner.controller.external.delete_user_action(&package, flags, observer.binder),
                Decision::Status(status) => observer.deleted(&package, status, None),
                Decision::Delete(plan) => match owner.controller.execute(&plan) {
                    Ok(status) => observer.deleted(&package, status, None),
                    Err(error) => {
                        let callback = observer.deleted(&package, removal::INTERNAL_ERROR, Some(&error.message));
                        owner.errors.lock().unwrap().push(error); callback
                    }
                },
            };
            if let Err(error) = result { owner.errors.lock().unwrap().push(error); }
        }))
    }
    fn invoke_legacy(self: &Arc<Self>, package: Option<String>, version: i32, observer: Option<Binder>,
        uid: u32, pid: i32, user: i32, flags: i32) -> Result<(), Exception> {
        let legacy = observer.map(|binder| Observer::retain(&self.process, binder).map(|observer| observer.target)).transpose()?;
        let adapter = self.process.add_service(Arc::new(LegacyObserver { legacy, owner: Arc::downgrade(self) }));
        let package = package.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null packageName"))?;
        self.invoke(Some(VersionedPackage { name: package, version: i64::from(version) }),
            Some(adapter), uid, pid, user, flags, false)
    }

    pub fn dispatch(self: &Arc<Self>, uid: u32, pid: i32, code: u32, reader: &mut Reader<'_>) -> Option<aim_binder_host::local::Reply> {
        if !matches!(code, pm::DELETE_PACKAGE_AS_USER | pm::DELETE_PACKAGE_VERSIONED | pm::DELETE_EXISTING_PACKAGE_AS_USER) { return None }
        Some((|| {
            if code == pm::DELETE_PACKAGE_AS_USER {
                let args = pm::DeletePackageAsUser::read(reader)?;
                if reader.remaining() != 0 { return Err(BAD_VALUE); }
                let result = self.invoke_legacy(args.package_name, args.version_code, args.observer,
                    uid, pid, args.user_id, args.flags);
                let mut reply = Parcel::new();
                match result { Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error) }
                return Ok(reply);
            }
            let (versioned, observer, user, flags, existing) = if code == pm::DELETE_PACKAGE_VERSIONED {
                let args = pm::DeletePackageVersioned::<VersionedPackage>::read(reader)?;
                (args.versioned_package, args.observer, args.user_id, args.flags, false)
            } else {
                let args = pm::DeleteExistingPackageAsUser::<VersionedPackage>::read(reader)?;
                (args.versioned_package, args.observer, args.user_id, 0, true)
            };
            if reader.remaining() != 0 { return Err(BAD_VALUE) }
            let result = self.invoke(versioned, observer, uid, pid, user, flags, existing);
            let mut reply = Parcel::new(); match result { Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error) }; Ok(reply)
        })())
    }
}
fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
