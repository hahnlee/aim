//! Original PM Internal persistence leaves over native Settings owners.
use super::*;
use aim_service_aidl::dev_aim_server_ipermissionpersistencebridge as permission_api;
use std::{collections::BTreeMap, sync::Condvar, thread::JoinHandle, time::{Duration, Instant}};

pub struct Runtime {
    bridge: Arc<crate::package::bootstrap::Bridge>,
    permission: Strong,
    kernel: Arc<crate::package::kernel_mappings::Owner>,
    signal: Arc<(Mutex<(bool, Option<Instant>)>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
    writes: Mutex<()>,
    error: Mutex<Option<Exception>>,
}
fn illegal(message: impl Into<String>) -> Exception { Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message) }
fn enforce(uid: i32) -> Result<()> {
    if matches!(uid, 0 | 1000) { Ok(()) } else { Err(Exception::security("Package internal persistence requires system UID")) }
}
impl Runtime {
    pub fn stop(&self) {
        let (lock, cv) = &*self.signal;
        lock.lock().unwrap().0 = true;
        cv.notify_all();
    }
    pub fn start(system: &Arc<System>, bridge: Arc<crate::package::bootstrap::Bridge>,
        permission: Strong, kernel: Arc<crate::package::kernel_mappings::Owner>) -> Result<Arc<Self>> {
        system.check_package_bootstrap(&bridge)?;
        let owner = Arc::new(Self { bridge, permission, kernel,
            signal: Arc::new((Mutex::new((false, None)), Condvar::new())),
            worker: Mutex::new(None), writes: Mutex::new(()), error: Mutex::new(None) });
        let weak = Arc::downgrade(&owner); let weak_system = Arc::downgrade(system);
        let signal = owner.signal.clone();
        let worker = std::thread::Builder::new().name("package-settings-writes".into()).spawn(move || {
            let (lock, cv) = &*signal;
            loop {
                let mut state = lock.lock().unwrap();
                while !state.0 {
                    match state.1 {
                        Some(deadline) if deadline <= Instant::now() => break,
                        Some(deadline) => { state = cv.wait_timeout(state, deadline.saturating_duration_since(Instant::now())).unwrap().0; }
                        None => { state = cv.wait(state).unwrap(); }
                    }
                }
                if state.0 { break; }
                state.1 = None; drop(state);
                let (Some(owner), Some(system)) = (weak.upgrade(), weak_system.upgrade()) else { break; };
                if let Err(error) = system.persist_internal_settings(&owner) { *owner.error.lock().unwrap() = Some(error); }
            }
        }).map_err(|error| illegal(error.to_string()))?;
        *owner.worker.lock().unwrap() = Some(worker);
        Ok(owner)
    }
    pub fn take_error(&self) -> Option<Exception> { self.error.lock().unwrap().take() }
    fn schedule(&self) -> Result<()> {
        if let Some(error) = self.take_error() { return Err(error); }
        let (lock, cv) = &*self.signal; let mut state = lock.lock().unwrap();
        if state.0 { return Err(illegal("settings scheduler stopped")); }
        // PackageManagerService.scheduleWriteSettings coalesces the first request.
        state.1.get_or_insert(Instant::now() + Duration::from_secs(10)); cv.notify_all(); Ok(())
    }
    fn call(&self, code: u32) -> Result<aim_binder_host::local::Received> {
        let mut request = Parcel::new(); request.write_interface_token(permission_api::DESCRIPTOR);
        let reply = self.permission.transact(code, &request, false)
            .map_err(|status| illegal(format!("permission persistence transport: {status}")))?;
        reply.reader().read_exception().map_err(|status| illegal(format!("permission persistence reply: {status}")))??;
        Ok(reply)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let (lock, cv) = &*self.signal; lock.lock().unwrap().0 = true; cv.notify_all();
        if let Some(worker) = self.worker.lock().unwrap().take() {
            if worker.thread().id() != std::thread::current().id() { let _ = worker.join(); }
        }
    }
}
impl System {
    pub(crate) fn install_internal_persistence(&self, owner: Arc<Runtime>) -> Result<()> {
        self.check_package_bootstrap(&owner.bridge)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &owner.bridge))
            .ok_or_else(|| illegal("internal persistence bootstrap changed"))?;
        if current.internal_persistence.is_some() { return Err(illegal("internal persistence already installed")); }
        current.internal_persistence = Some(owner); Ok(())
    }
    fn internal_persistence_owner(&self) -> Result<Arc<Runtime>> {
        self.package_bootstrap.lock().unwrap().current.as_ref()
            .and_then(|current| current.internal_persistence.clone()).ok_or_else(|| illegal("internal persistence owner unavailable"))
    }
    pub(crate) fn internal_write_settings(&self, asynchronous: bool, uid: i32, _pid: i32) -> Result<()> {
        enforce(uid)?; let owner = self.internal_persistence_owner()?;
        if asynchronous {
            self.check_package_bootstrap(&owner.bridge)?;
            owner.bridge.invalidate_package_info_cache().map_err(|error| illegal(format!("settings schedule cache owner: {error:?}")))?;
            owner.schedule()
        } else { self.persist_internal_settings(&owner) }
    }
    fn persist_internal_settings(&self, owner: &Runtime) -> Result<()> {
        let _write = owner.writes.lock().unwrap();
        owner.signal.0.lock().unwrap().1 = None;
        self.check_package_bootstrap(&owner.bridge)?;
        let reply = owner.call(permission_api::CAPTURE_DEFINITIONS)?;
        let mut reader = reply.reader(); reader.read_exception().map_err(|status| illegal(status.to_string()))??;
        let bytes = aim_service_aidl::read_byte_array(&mut reader).map_err(|status| illegal(status.to_string()))?
            .ok_or_else(|| illegal("permission definitions missing"))?;
        if reader.remaining() != 0 { return Err(illegal("permission definitions trailing data")); }
        let root = aim_android_xml::abx::read(&bytes).map_err(illegal)?;
        let definitions = crate::package::settings::Settings::parse(&root).map_err(illegal)?;
        let install = self.package_install_guard();
        let disk = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current|Arc::ptr_eq(&current.bridge,&owner.bridge))
            .and_then(|current| current.persistence.clone()).ok_or_else(|| illegal("Settings disk owner unavailable"))?;
        let mut disk = disk.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, &owner.bridge))
            .ok_or_else(|| illegal("settings writer bootstrap changed"))?;
        let base=current.queries.clone().ok_or_else(||illegal("settings writer current capture unavailable"))?;
        let mut scan = base.scan().owner().clone();
        apply_permission_definitions(&mut scan,&definitions);
        let update = base.prepare_package_update(scan).map_err(illegal)?;
        let result = disk.commit_scan_settings(update.capture.scan());
        if result.is_ok() || result.as_ref().is_err_and(|error| error.committed) {
            current.publish_snapshot(update.store); current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
            state.version = update.capture.scan().version();
        }
        drop(state);
        drop(install);
        result.map_err(|error| illegal(error.to_string()))?;
        owner.bridge.invalidate_package_info_cache().map_err(|error| illegal(format!("settings write cache owner: {error:?}")))?;
        let users = update.capture.state().users.keys().copied().collect::<Vec<_>>();
        for setting in &update.capture.scan().owner().settings.packages {
            let states = update.capture.scan().owner().scanned_user_states(&setting.name)
                .ok_or_else(|| illegal("settings kernel mapping user owner unavailable"))?;
            let excluded = states.iter().filter_map(|(id, state)| (!state.installed).then_some(*id)).collect::<Vec<_>>();
            owner.kernel.update(&setting.name, setting.app_id, &excluded).map_err(illegal)?;
        }
        self.commit_package_list_from_scan(&owner.bridge, &mut disk, &update.capture, &users)
            .map_err(|error| illegal(error.to_string()))?;
        drop(disk);
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(update.capture.state()).map_err(|error| illegal(format!("settings restriction resolution: {error:?}")))?;
        let query = crate::package::query::Query { state: update.capture.state(), filter: &resolution.apps_filter, calling_uid: 1000 };
        for user in &users { self.flush_native_package_restrictions(&query, *user, &update.capture)?; }
        self.internal_write_permission_settings(Some(users), true, 1000, -1)?;
        Ok(())
    }
    pub(crate) fn internal_write_permission_settings(&self, users: Option<Vec<i32>>, asynchronous: bool,
        uid: i32, _pid: i32) -> Result<()> {
        enforce(uid)?; let users = users.ok_or_else(|| Exception::illegal_argument("null permission users"))?;
        let runtime = self.internal_persistence_owner()?; let capture = self.capture_package_queries()?;
        for user in &users {
            if !capture.state().users.contains_key(user) { return Err(Exception::illegal_argument("unknown permission user")); }
        }
        self.with_runtime_permission_metadata(&capture, |metadata| {
            for user in &users { metadata.request_permission_write(*user)?; }
            Ok::<_, String>(())
        })?.map_err(illegal)?;
        if asynchronous { return Ok(()); }
        let disk = self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.persistence.clone())
            .ok_or_else(|| illegal("permission disk owner unavailable"))?;
        let mut disk = disk.lock().unwrap();
        let metadata_owner = self.runtime_permission_metadata_owner(&capture)?;
        let mut metadata = metadata_owner.lock().unwrap();
        for user in users {
            let inode = metadata.creation_inode(user as u32).ok_or_else(|| illegal("permission user inode unavailable"))?;
            self.commit_runtime_permissions_from_scan(&runtime.bridge, &mut disk, &capture, user as u32, &metadata, inode)
                .map_err(|error| illegal(error.to_string()))?;
            metadata.acknowledge_write(user);
        }
        Ok(())
    }
    pub(crate) fn internal_update_runtime_permissions_fingerprint(&self, user: i32, uid: i32, _pid: i32) -> Result<()> {
        enforce(uid)?; let capture = self.capture_package_queries()?;
        if !capture.state().users.contains_key(&user) { return Err(Exception::illegal_argument("unknown fingerprint user")); }
        self.with_runtime_permission_metadata(&capture, |metadata| metadata.update_fingerprint(user))?.map_err(illegal)
    }
    pub(crate) fn internal_remove_legacy_default_browser_package_name(&self, user: i32, uid: i32, _pid: i32) -> Result<Option<String>> {
        enforce(uid)?; let user = u32::try_from(user).map_err(|_| Exception::illegal_argument("negative browser user"))?;
        let disk = self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.persistence.clone())
            .ok_or_else(|| illegal("pending browser disk owner unavailable"))?;
        let mut disk = disk.lock().unwrap();
        let old = disk.pending_default_browser(user).map_err(illegal)?;
        if old.is_some() { disk.set_pending_default_browser(user, None).map_err(illegal)?; }
        Ok(old)
    }
    pub(crate) fn read_native_package_permission_user(&self, bridge: &Arc<crate::package::bootstrap::Bridge>,
        disk: &Arc<Mutex<crate::package::owner::Store>>, user: i32) -> Result<()> {
        self.check_package_bootstrap(bridge)?; let runtime = self.internal_persistence_owner()?;
        if !Arc::ptr_eq(&runtime.bridge, bridge) { return Err(illegal("permission read leaf belongs to another bootstrap")); }
        let id = u32::try_from(user).map_err(|_| Exception::illegal_argument("negative permission read user"))?;
        let base = self.capture_package_queries()?;
        let config = self.package_bootstrap.lock().unwrap().current.as_ref().and_then(|current| current.installer.as_ref().map(|(owner, _)| owner.system_config().clone()))
            .ok_or_else(|| illegal("permission read SystemConfig unavailable"))?;
        // Original modern writeLegacyPermissionStateTEMP is a no-op. The
        // native Settings read restores its actual persisted legacy objects;
        // a computed live permission export is a separate query owner.
        let install = self.package_install_guard();
        let mut disk = disk.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| illegal("permission read bootstrap changed"))?;
        let latest = current.queries.clone().ok_or_else(||illegal("permission read current capture unavailable"))?;
        if base.state().users.keys().ne(latest.state().users.keys())
            || base.scan().owner().settings.packages.iter().map(|setting|(&setting.name,setting.app_id,&setting.code_path,setting.version_code,setting.shared_app_id())).collect::<std::collections::BTreeSet<_>>()
                != latest.scan().owner().settings.packages.iter().map(|setting|(&setting.name,setting.app_id,&setting.code_path,setting.version_code,setting.shared_app_id())).collect() {
            return Err(illegal("permission read UID/user inventory changed during original owner capture"));
        }
        let mut scan = latest.scan().owner().clone();
        let metadata = disk.read_permission_user(&mut scan, &config, id).map_err(|error| illegal(error.to_string()))?;
        let update = latest.prepare_package_update(scan).map_err(illegal)?;
        current.publish_snapshot(update.store); current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version(); drop(state); drop(disk); drop(install);
        self.with_runtime_permission_metadata(&update.capture, |owner| owner.import_permission_user(&metadata, user))?;
        let reply = runtime.call(permission_api::IMPORT_NATIVE_MIGRATION)?;
        let mut reader = reply.reader(); reader.read_exception().map_err(|status| illegal(status.to_string()))??;
        if reader.remaining() != 0 { return Err(illegal("permission import trailing data")); }
        self.check_package_bootstrap(bridge)
    }
}

impl System {
    pub(crate) fn flush_pending_internal_settings(&self)->Result<()> {
        let owner=self.internal_persistence_owner()?;
        let pending=owner.signal.0.lock().unwrap().1.is_some();
        if pending {self.persist_internal_settings(&owner)?;}
        Ok(())
    }
}

// Original permission namespace is captured by its owner; merge only these
// definitions into the newest native package graph, preserving unrelated state.
fn apply_permission_definitions(scan:&mut crate::package::scan::SigningScan,definitions:&crate::package::settings::Settings){
    scan.settings.permissions=definitions.permissions.clone();
    scan.settings.permission_trees=definitions.permission_trees.clone();
}

#[cfg(test)]
mod latest_settings_tests {
    use super::*;
    use crate::package::{settings::{Settings,Package},scan::{SigningScan,CapturedUsers},restrictions::UserState};
    #[test]
    fn permission_definition_merge_retains_latest_package_and_app_data_fields(){
        let mut latest=SigningScan::new(&Default::default(),&Settings{packages:vec![Package{name:"p".into(),app_id:10100,code_path:"/data/app/p".into(),category_hint:7,..Default::default()}],..Default::default()},36).unwrap();
        latest.capture_user_states(BTreeMap::from([(("p".into(),false),CapturedUsers{states:BTreeMap::from([(0,UserState{ce_data_inode:99,de_data_inode:101,stopped:false,min_aspect_ratio:3,..Default::default()})]),active_aliases:Default::default()})])).unwrap();
        let retained=latest.clone();
        let document=aim_android_xml::read(b"<packages><permissions><item name='p.PERMISSION' package='p' protection='2'/></permissions><permission-trees><item name='p.TREE' package='p' protection='0'/></permission-trees></packages>").unwrap();
        let definitions=Settings::parse(&document).unwrap();
        assert_eq!(definitions.permissions.len(),1);assert_eq!(definitions.permission_trees.len(),1);
        apply_permission_definitions(&mut latest,&definitions);
        assert_eq!(latest.settings.permissions,definitions.permissions);assert_eq!(latest.settings.permission_trees,definitions.permission_trees);
        assert_eq!(latest.settings.packages,retained.settings.packages);
        assert_eq!(latest.scanned_user_states("p"),retained.scanned_user_states("p"));
        assert!(retained.settings.permissions.is_empty());
    }
}
