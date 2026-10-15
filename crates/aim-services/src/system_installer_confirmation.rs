//! Concrete native PIS confirmation policy with original non-PMS leaves.
//! AOSP android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
use crate::package::{apps_filter, installer::{self, commit::{ConfirmationPolicy, ConfirmationPolicySource}, policy::CallingPermissions}, model::State,
    query::Query, resolve::Resolver};
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, Reader, EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_iinstallerconfirmationbridge as api;
use std::{path::PathBuf, sync::Arc};

/// Root binds NativeOwners' real storage.validation_path + guest path mapping.
pub type StagePath = Box<dyn Fn(&installer::Session, &installer::Record) -> Result<PathBuf, Exception> + Send + Sync>;
/// Root supplies its captured immutable native package state, never a PMS feed.
pub type QuerySource = Arc<dyn Fn() -> Result<Arc<State>, Exception> + Send + Sync>;
pub struct Source {
    leaf: Strong,
    state: QuerySource,
    resolver: Arc<Resolver>,
    stages: StagePath,
    installer_ui_package: String,
    permissions: CallingPermissions,
}
impl Source {
    pub fn new(leaf: Strong, state: QuerySource, resolver: Arc<Resolver>, stages: StagePath,
        installer_ui_package: String, permissions: CallingPermissions) -> Result<Arc<Self>, Exception> {
        let reply = leaf.transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false).map_err(transport)?;
        let mut reader = reply.reader();
        if reader.read_string16().map_err(transport)?.as_deref() != Some(api::DESCRIPTOR) || reader.remaining() != 0 {
            return Err(Exception::new(EX_ILLEGAL_STATE, "installer confirmation leaf descriptor differs"));
        }
        if installer_ui_package.is_empty() { return Err(Exception::new(EX_ILLEGAL_STATE, "native required installer package unavailable")); }
        Ok(Arc::new(Self { leaf, state, resolver, stages, installer_ui_package, permissions }))
    }
    pub fn policy_source(self: &Arc<Self>) -> ConfirmationPolicySource {
        let owner = self.clone();
        Arc::new(move |session, record| owner.policy(session, record))
    }
    fn call<T>(&self, code: u32, write: impl FnOnce(&mut Parcel), read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>) -> Result<T, Exception> {
        let mut request = Parcel::new(); write(&mut request);
        let reply = self.leaf.transact(code, &request, false).map_err(transport)?;
        let mut reader = reply.reader(); reader.read_exception().map_err(transport)??;
        let value = read(&mut reader).map_err(transport)?;
        if reader.remaining() != 0 { return Err(Exception::new(EX_ILLEGAL_STATE, "installer confirmation leaf reply tail")); }
        Ok(value)
    }
    pub fn policy(&self, session: &installer::Session, record: &installer::Record) -> Result<ConfirmationPolicy, Exception> {
        let state = (self.state)()?;
        let resolution = self.resolver.resolution(&state).map_err(|error| Exception::new(EX_ILLEGAL_STATE, format!("installer confirmation native resolution: {error:?}")))?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: session.installer_uid as i32 };
        let user = i32::try_from(session.user).map_err(|error| Exception::illegal_argument(error.to_string()))?;
        let target = session.resolved_package.as_deref().or(record.params.app_package_name.as_deref());
        let uid = session.installer_uid as i32;
        let installer = record.installer_package.as_deref();
        let device_owner_or_affiliated = self.call(api::IS_DEVICE_OWNER_OR_AFFILIATED, |p| api::IsDeviceOwnerOrAffiliated {
            installer_package: installer.map(str::to_owned), installer_uid: uid, user_id: user,
        }.write(p), |r| r.read_bool())?;
        let install_disabled = self.call(api::IS_INSTALL_DISABLED, |p| api::IsInstallDisabled {
            installer_package: installer.map(str::to_owned), installer_uid: uid, user_id: user,
        }.write(p), |r| r.read_bool())?;
        let silent_target_allowed = self.call(api::IS_SILENT_TARGET_ALLOWED, |p| api::IsSilentTargetAllowed {
            target_package: target.map(str::to_owned), target_sdk_version: session.validated_target_sdk.unwrap_or(i32::MAX),
        }.write(p), |r| r.read_bool())?;
        let dependency_installer_enabled = self.call(api::IS_DEPENDENCY_INSTALLER_ENABLED, |p| api::IsDependencyInstallerEnabled {}.write(p), |r| r.read_bool())?;
        let update_ownership_enabled = self.call(api::IS_UPDATE_OWNERSHIP_ENABLED, |p| api::IsUpdateOwnershipEnabled {}.write(p), |r| r.read_bool())?;
        let uptime_millis = self.call(api::GET_UPTIME_MILLIS, |p| api::GetUptimeMillis {}.write(p), |r| r.read_i64())?;
        let emergency_install = emergency(&query, target, user, uid, &self.permissions)?;
        let (has_device_admin_receiver, is_sdk_or_static_library) = if session.validated_target_sdk.is_none() && !session.sealed {
            // Native session is in the explicit pre-parser phase, corresponding
            // to original mPackageLite=null/mHasDeviceAdminReceiver=false.
            (false, false)
        } else {
            let path = (self.stages)(session, record)?;
            lite_manifest_facts(&path)?
        };
        if !state.packages.get(&self.installer_ui_package).is_some_and(|package| package.is.system && package.pkg.is_some()) {
            return Err(Exception::new(EX_ILLEGAL_STATE, "native required installer selection no longer owns system code"));
        }
        Ok(ConfirmationPolicy { installer_package: self.installer_ui_package.clone(), device_owner_or_affiliated,
            emergency_install, install_disabled, dependency_installer_enabled, update_ownership_enabled,
            silent_target_allowed, has_device_admin_receiver, is_sdk_or_static_library, uptime_millis })
    }
}
fn emergency(query: &Query<'_>, target: Option<&str>, user: i32, installer_uid: i32, permissions: &CallingPermissions) -> Result<bool, Exception> {
    let Some(package) = target.and_then(|name| query.state.packages.get(name)).filter(|package| package.is.system && package.pkg.is_some()) else { return Ok(false); };
    let Some(emergency_installer) = package.pkg.as_ref().unwrap().emergency_installer.as_deref() else { return Ok(false); };
    let names = query.packages_for_uid(installer_uid).map_err(missing)?;
    if !names.is_some_and(|names| names.iter().any(|name| name.as_deref() == Some(emergency_installer))) { return Ok(false); }
    let target_uid = apps_filter::uid(user, package.app_id);
    let privileged = permissions.check("android.permission.INSTALL_PACKAGES", -1, target_uid)?
        || permissions.check("android.permission.INSTALL_PACKAGE_UPDATES", -1, target_uid)?
        || permissions.check("android.permission.INSTALL_SELF_UPDATES", -1, target_uid)?;
    Ok(privileged && permissions.check("android.permission.EMERGENCY_INSTALL_PACKAGES", -1, installer_uid)?)
}
fn lite_manifest_facts(stage: &std::path::Path) -> Result<(bool, bool), Exception> {
    use aim_apps::{apk::Apk, res::{Element, Value}};
    let path = if stage.is_dir() { stage.join("base.apk") } else { stage.to_path_buf() };
    let apk = Apk::open(&path).map_err(|error| Exception::new(EX_ILLEGAL_STATE, format!("confirmation APK: {error}")))?;
    let manifest = apk.manifest().map_err(|error| Exception::new(EX_ILLEGAL_STATE, format!("confirmation manifest: {error}")))?;
    fn text<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
        element.attrs.iter().find(|attribute| attribute.ns == "http://schemas.android.com/apk/res/android" && attribute.name == name)
            .and_then(|attribute| match &attribute.value { Value::String(value) => Some(value.as_str()), _ => None })
    }
    let mut admin = false; let mut library = false;
    for application in manifest.children.iter().filter(|element| element.name == "application") {
        let app_bind = text(application, "permission") == Some("android.permission.BIND_DEVICE_ADMIN");
        for element in &application.children {
            if matches!(element.name.as_str(), "sdk-library" | "static-library") { library = true; }
            if element.name == "receiver" && (app_bind || text(element, "permission") == Some("android.permission.BIND_DEVICE_ADMIN"))
                && element.children.iter().any(|child| child.name == "meta-data" && text(child, "name") == Some("android.app.device_admin")) { admin = true; }
        }
    }
    Ok((admin, library))
}
fn transport(status: i32) -> Exception { Exception::new(EX_ILLEGAL_STATE, format!("installer confirmation transport: {status}")) }
fn missing(error: apps_filter::NotModelled) -> Exception { Exception::new(EX_ILLEGAL_STATE, error.0) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{apps_filter::{AppsFilter, Config}, model::{PackageState, PackageUserState, User}, pkg::AndroidPackage};
    use std::sync::{Mutex, atomic::{AtomicBool, AtomicU8, Ordering}};

    fn state() -> State {
        let mut target = PackageState {
            name: "system.installer".into(), app_id: 10101,
            pkg: Some(Arc::new(AndroidPackage { package_name: "system.installer".into(), uid: 10101,
                emergency_installer: Some("emergency.installer".into()), ..Default::default() })),
            users: [(0, PackageUserState::default())].into(), ..Default::default()
        };
        target.is.system = true;
        State {
            packages: [(target.name.clone(), target), ("emergency.installer".into(), PackageState {
                name: "emergency.installer".into(), app_id: 10100,
                pkg: Some(Arc::new(AndroidPackage { package_name: "emergency.installer".into(), uid: 10100, ..Default::default() })),
                users: [(0, PackageUserState::default())].into(), ..Default::default()
            })].into(),
            users: [(0, User { id: 0, ..Default::default() })].into(), ..Default::default()
        }
    }
    #[test]
    fn emergency_permission_adoption_and_revocation_are_live_for_both_uids() {
        let state = state();
        let before = state.clone();
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        let query = Query { state: &state, filter: &filter, calling_uid: 10100 };
        let target = Arc::new(AtomicBool::new(false));
        let installer = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let target_grant = target.clone();
        let installer_grant = installer.clone();
        let observed = reads.clone();
        let permissions = CallingPermissions::new(Arc::new(move |name, pid, uid| {
            observed.lock().unwrap().push((name.to_owned(), pid, uid));
            Ok(match (name, uid) {
                ("android.permission.INSTALL_PACKAGES", 10101) => target_grant.load(Ordering::SeqCst),
                ("android.permission.EMERGENCY_INSTALL_PACKAGES", 10100) => installer_grant.load(Ordering::SeqCst),
                _ => false,
            })
        }));
        let decide = || emergency(&query, Some("system.installer"), 0, 10100, &permissions).unwrap();
        assert!(!decide());
        target.store(true, Ordering::SeqCst);
        installer.store(true, Ordering::SeqCst);
        assert!(decide());
        installer.store(false, Ordering::SeqCst);
        assert!(!decide());
        installer.store(true, Ordering::SeqCst);
        target.store(false, Ordering::SeqCst);
        assert!(!decide());
        target.store(true, Ordering::SeqCst);
        assert!(decide());
        let count = reads.lock().unwrap().len();
        assert!(!emergency(&query, Some("system.installer"), 0, 10102, &permissions).unwrap());
        assert_eq!(reads.lock().unwrap().len(), count);
        let reads = reads.lock().unwrap();
        assert!(reads.iter().all(|(_, pid, _)| *pid == -1));
        assert!(reads.iter().filter(|(name, _, _)| name == "android.permission.EMERGENCY_INSTALL_PACKAGES").all(|(_, _, uid)| *uid == 10100));
        assert!(reads.iter().filter(|(name, _, _)| name.starts_with("android.permission.INSTALL_")).all(|(_, _, uid)| *uid == 10101));
        assert_eq!(state, before);
    }
    #[test]
    fn emergency_permission_owner_errors_abort_decision_without_state_changes() {
        let state = state();
        let before = state.clone();
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        let query = Query { state: &state, filter: &filter, calling_uid: 10100 };
        let failure = Arc::new(AtomicU8::new(0));
        let flag = failure.clone();
        let reads = Arc::new(Mutex::new(Vec::new()));
        let observed = reads.clone();
        let permissions = CallingPermissions::new(Arc::new(move |name, pid, uid| {
            observed.lock().unwrap().push((name.to_owned(), pid, uid));
            if flag.load(Ordering::SeqCst) == 1 && uid == 10101 || flag.load(Ordering::SeqCst) == 2 && uid == 10100 {
                return Err(Exception::new(EX_ILLEGAL_STATE, "permission owner retired"));
            }
            Ok(true)
        }));
        assert!(emergency(&query, Some("system.installer"), 0, 10100, &permissions).unwrap());
        failure.store(1, Ordering::SeqCst);
        reads.lock().unwrap().clear();
        let error = emergency(&query, Some("system.installer"), 0, 10100, &permissions).unwrap_err();
        assert_eq!((error.code, error.message.as_str()), (EX_ILLEGAL_STATE, "permission owner retired"));
        assert_eq!(reads.lock().unwrap().len(), 1);
        failure.store(2, Ordering::SeqCst);
        let error = emergency(&query, Some("system.installer"), 0, 10100, &permissions).unwrap_err();
        assert_eq!((error.code, error.message.as_str()), (EX_ILLEGAL_STATE, "permission owner retired"));
        assert_eq!(state, before);
    }
}
