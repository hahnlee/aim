//! Native MovePackageHelper orchestration, android-16.0.0_r1 (AOSP, Apache-2.0).
//! Storage relocation precedes the existing native install transaction's durable
//! publication; original PMS is never called. Freezers and workers own lifetimes.
use super::{effects, installer::native::QuerySource, lifecycle, moves, pkg::booleans, query::Query, resolve::Resolver};
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, Reader, BAD_VALUE, EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_ipackagerelocationbridge as api;
use std::{path::PathBuf, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, thread::{self, JoinHandle}, time::Duration};

pub const SUCCESS: i32 = -100;
pub const STORAGE: i32 = -1;
pub const MISSING: i32 = -2;
pub const SYSTEM: i32 = -3;
pub const INTERNAL: i32 = -6;
pub const PENDING: i32 = -7;
pub const ADMIN: i32 = -8;
pub const DISALLOW_INTERNAL: i32 = -9;
pub const LOCKED: i32 = -10;
#[derive(Debug)]
pub struct Failure { pub status: i32, pub message: String, pub committed: bool }
fn failure(status: i32, message: impl Into<String>) -> Failure { Failure { status, message: message.into(), committed: false } }
fn owner_failure(error: Exception) -> Failure { failure(INTERNAL, format!("Move primitive: {error:?}")) }
pub fn install_failure(legacy: i32, committed: bool, message: impl Into<String>) -> Failure {
    Failure { status: if matches!(legacy, -4 | -18 | -19 | -20) { STORAGE } else { INTERNAL }, committed, message: message.into() }
}
pub struct Volume { pub complete: bool, pub measure_path: String }
pub struct StorageOwner { binder: Strong }
impl StorageOwner {
    pub fn new(binder: Strong) -> Arc<Self> { Arc::new(Self { binder }) }
    fn call<T>(&self, code: u32, write: impl FnOnce(&mut Parcel), read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>) -> Result<T, Exception> {
        let mut data = Parcel::new(); data.write_interface_token(api::DESCRIPTOR); write(&mut data);
        let reply = self.binder.transact(code, &data, false).map_err(|code| illegal(format!("Move storage transport: {code}")))?;
        let mut reader = reply.reader(); reader.read_exception().map_err(|code| illegal(format!("Move storage exception: {code}")))??;
        let value = read(&mut reader).map_err(|code| illegal(format!("Move storage reply: {code}")))?;
        if reader.remaining() != 0 { return Err(illegal("Move storage reply has trailing bytes")) }
        Ok(value)
    }
    fn volume(&self, uuid: Option<&str>) -> Result<Volume, Exception> {
        self.call(api::RESOLVE_VOLUME, |p| p.write_string16(uuid), |r| {
            if r.read_i32()? == 0 { return Err(BAD_VALUE) }
            let start = r.position(); let size = r.read_i32()?;
            if size < 4 { return Err(BAD_VALUE) }
            let end = start.checked_add(size as usize).ok_or(BAD_VALUE)?;
            if end > r.position() + r.remaining() { return Err(BAD_VALUE) }
            let complete = r.read_bool()?; let measure_path = r.read_string16()?.ok_or(BAD_VALUE)?;
            if r.position() > end { return Err(BAD_VALUE) } r.set_position(end);
            Ok(Volume { complete, measure_path })
        })
    }
    fn users(&self) -> Result<Vec<i32>, Exception> { self.call(api::GET_USERS, |_| {}, |r| aim_service_aidl::read_int_array(r)?.ok_or(BAD_VALUE)) }
    fn allow_internal(&self) -> Result<bool, Exception> { self.call(api::ALLOW_THIRD_PARTY_INTERNAL, |_| {}, |r| r.read_bool()) }
    fn encrypted(&self) -> Result<bool, Exception> { self.call(api::IS_FILE_ENCRYPTED, |_| {}, |r| r.read_bool()) }
    fn unlocked(&self, user: i32) -> Result<bool, Exception> { self.call(api::IS_USER_UNLOCKED, |p| p.write_i32(user), |r| r.read_bool()) }
    fn label(&self, name: &str, user: i32) -> Result<String, Exception> { self.call(api::APPLICATION_LABEL, |p| { p.write_string16(Some(name)); p.write_i32(user); }, |r| r.read_string16()?.ok_or(BAD_VALUE)) }
    fn bytes(&self, path: &str, low: bool) -> Result<i64, Exception> { self.call(if low { api::BYTES_UNTIL_LOW } else { api::USABLE_BYTES }, |p| p.write_string16(Some(path)), |r| r.read_i64()) }
    fn measure(&self, state: &super::model::PackageState, user: i32) -> Result<(i64, i64), Exception> {
        self.call(api::MEASURE_PACKAGE, |p| {
            p.write_string16(state.volume_uuid.as_deref()); p.write_string16(Some(&state.name)); p.write_i32(user); p.write_i32(state.app_id);
            p.write_i64(state.users.get(&user).map_or(0, |state| state.ce_data_inode)); p.write_string16(Some(&state.path));
        }, |r| {
            let values = aim_service_aidl::read_long_array(r)?.ok_or(BAD_VALUE)?;
            if values.len() != 6 { return Err(BAD_VALUE) }
            // getAppSize returns code, data (including cache), cache, external triplet.
            Ok((values[0], values[1].checked_sub(values[2]).ok_or(BAD_VALUE)?))
        })
    }
    fn prepare(&self, plan: &Plan) -> Result<(), Exception> {
        self.call(api::PREPARE_USERS, |p| {
            p.write_string16(plan.source_volume.as_deref()); p.write_string16(plan.destination_volume.as_deref());
            aim_service_aidl::write_int_array(p, Some(&plan.users));
        }, |_| Ok(()))
    }
    fn relocate(&self, plan: &Plan) -> Result<(), Exception> {
        self.call(api::MOVE_COMPLETE_APP, |p| {
            p.write_string16(plan.source_volume.as_deref()); p.write_string16(plan.destination_volume.as_deref());
            p.write_string16(Some(&plan.package)); p.write_i32(plan.app_id); p.write_string16(plan.seinfo.as_deref());
            p.write_i32(plan.target_sdk); p.write_string16(Some(&plan.from_code_path));
        }, |_| Ok(()))
    }
}

#[derive(Clone)]
pub struct Plan {
    pub move_id: i32, pub package: String, pub version: i64, pub app_id: i32,
    pub source_volume: Option<String>, pub destination_volume: Option<String>,
    pub code_path: String, pub from_code_path: String, pub users: Vec<i32>, pub install_user: i32,
    pub seinfo: Option<String>, pub target_sdk: i32, pub abi_override: Option<String>,
    pub install_source: super::model::InstallSource, pub complete: bool, pub originally_external: bool,
    pub install_flags: i32,
}
/// This is the actual native install admission/metadata owner, shared with normal
/// installs. It reserves destination code before I/O and owns rollback on drop.
pub trait PreparedMove: Send {
    /// Code-only physical-volume moves use the ordinary native APK copier.
    fn copy_code(&mut self) -> Result<(), Failure>;
    /// Reconcile, persist and publish the moved package using the reserved path.
    fn commit(self: Box<Self>) -> Result<Receipt, Failure>;
}
pub struct Receipt { pub generation: u64, pub package: String, pub version: i64, pub path: String, pub volume: Option<String> }
pub trait InstallOwner: Send + Sync {
    fn prepare(&self, plan: &Plan) -> Result<Box<dyn PreparedMove>, Failure>;
    fn finish(&self, plan: &Plan, receipt: &Receipt) -> Result<(), Failure>;
}
pub type Files = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;
pub struct Owner {
    source: QuerySource, resolver: Resolver, files: Files,
    storage: Arc<StorageOwner>, install: Arc<dyn InstallOwner>,
    effects: Arc<effects::Owner>, lifecycle: Arc<lifecycle::Owner>, callbacks: Arc<moves::Owner>,
    gate: Arc<Mutex<()>>, workers: Mutex<Vec<JoinHandle<()>>>, closed: AtomicBool, errors: Arc<Mutex<Vec<Failure>>>,
}
pub struct Workers { owner: Arc<Owner> }
impl Drop for Workers {
    fn drop(&mut self) {
        self.owner.closed.store(true, Ordering::Release);
        let workers = std::mem::take(&mut *self.owner.workers.lock().unwrap());
        for worker in workers { if worker.thread().id() != thread::current().id() && worker.join().is_err() { self.owner.errors.lock().unwrap().push(failure(INTERNAL, "Move worker panicked")); } }
    }
}
impl Owner {
    pub fn close(&self) { self.closed.store(true, Ordering::Release); }
    pub fn new(source: QuerySource, files: Files, storage: Arc<StorageOwner>, install: Arc<dyn InstallOwner>, effects: Arc<effects::Owner>, lifecycle: Arc<lifecycle::Owner>, callbacks: Arc<moves::Owner>, gate: Arc<Mutex<()>>) -> (Arc<Self>, Workers) {
        let owner = Arc::new(Self { source, resolver: Resolver::default(), files, storage, install, effects, lifecycle, callbacks, gate,
            workers: Mutex::new(vec![]), closed: AtomicBool::new(false), errors: Arc::new(Mutex::new(vec![])) });
        (owner.clone(), Workers { owner })
    }
    pub fn move_package(self: &Arc<Self>, package: Option<String>, destination: Option<String>, uid: u32, user: i32) -> Result<i32, Exception> {
        let state = (self.source)()?; let resolved = self.resolver.resolution(&state).map_err(|error| illegal(format!("Move permission snapshot: {error:?}")))?;
        let query = Query { state: &state, filter: &resolved.apps_filter, calling_uid: uid as i32 };
        if !super::installer::policy::permission(&query, "android.permission.MOVE_PACKAGE")? { return Err(Exception::security("movePackage requires MOVE_PACKAGE")) }
        let mut workers = self.workers.lock().unwrap();
        if self.closed.load(Ordering::Acquire) { return Err(illegal("Native move owner closed")) }
        let id = self.callbacks.next_id()?;
        let owner = self.clone();
        let worker = thread::Builder::new().name(format!("package-move-{id}")).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.run(id, package.as_deref(), destination, uid, user)))
                .unwrap_or_else(|_| Err(failure(INTERNAL, "Native move transaction panicked")));
            if let Err(error) = result {
                if let Err(callback) = owner.callbacks.changed(id, error.status, -1) { owner.errors.lock().unwrap().push(owner_failure(callback)); }
                owner.errors.lock().unwrap().push(error);
            }
        }).map_err(|error| illegal(format!("Move worker: {error}")))?;
        workers.push(worker); Ok(id)
    }
    fn run(&self, id: i32, name: Option<&str>, destination: Option<String>, uid: u32, user: i32) -> Result<(), Failure> {
        let _install_guard = self.gate.lock().unwrap();
        if self.closed.load(Ordering::Acquire) { return Err(failure(INTERNAL, "Native move owner closed")) }
        let state = (self.source)().map_err(owner_failure)?;
        let resolved = self.resolver.resolution(&state).map_err(|error| failure(INTERNAL, format!("Move visibility: {error:?}")))?;
        let query = Query { state: &state, filter: &resolved.apps_filter, calling_uid: uid as i32 };
        let package = name.and_then(|name| state.packages.get(name)).ok_or_else(|| failure(MISSING, "Missing package"))?;
        if package.pkg.is_none() || package.users.get(&user).is_some_and(|state| !state.installed)
            || query.filtered(Some(package), uid as i32, user).map_err(|error| failure(INTERNAL, error.0))? { return Err(failure(MISSING, "Missing package")) }
        let all_users = self.storage.users().map_err(owner_failure)?;
        let users: Vec<_> = all_users.iter().copied().filter(|user| package.users.get(user).is_none_or(|state| state.installed)).collect();
        let first = *users.first().ok_or_else(|| failure(MISSING, "Package is not installed for any user"))?;
        for &user in &users {
            if query.filtered_including_uninstalled(Some(package), user).map_err(|error| failure(INTERNAL, error.0))? { return Err(failure(MISSING, "Missing package")) }
        }
        if package.is.system { return Err(failure(SYSTEM, "Cannot move system application")) }
        if destination.as_deref() == Some("private") && !self.storage.allow_internal().map_err(owner_failure)? { return Err(failure(DISALLOW_INTERNAL, "3rd party apps are not allowed on internal storage")) }
        let path = (self.files)(&package.path).ok_or_else(|| failure(INTERNAL, "Move code path VFS owner unavailable"))?;
        if !path.is_dir() { return Err(failure(INTERNAL, "Move only supported for modern cluster style installs")) }
        if package.volume_uuid == destination { return Err(failure(INTERNAL, "Package already moved to destination")) }
        let code = package.pkg.as_ref().unwrap(); let external = code.is(booleans::EXTERNAL_STORAGE);
        if !external {
            for &user in &all_users { if self.effects.device_admin(&package.name, user).map_err(owner_failure)? { return Err(failure(ADMIN, "Device admin cannot be moved")) } }
        }
        if self.lifecycle.checked_is_frozen(&package.name).map_err(|message| failure(INTERNAL, message))? {
            return Err(failure(PENDING, "Failed to move already frozen package"));
        }
        let freezer = self.lifecycle.freeze(package.name.clone())
            .map_err(|message| failure(INTERNAL, message))?;
        self.effects.kill(&package.name, package.app_id, -1, "movePackageInternal", 10).map_err(owner_failure)?;
        let label = self.storage.label(&package.name, first).map_err(owner_failure)?;
        let mut extras = Parcel::new(); crate::bundle::write(&mut extras, &[
            ("android.intent.extra.PACKAGE_NAME", crate::bundle::Value::String(Some(package.name.clone()))),
            ("android.intent.extra.TITLE", crate::bundle::Value::String(Some(label))),
        ]); self.callbacks.created(id, &extras).map_err(owner_failure)?;
        let volume = self.storage.volume(destination.as_deref()).map_err(owner_failure)?;
        if volume.complete && self.storage.encrypted().map_err(owner_failure)? {
            for &user in &users { if !self.storage.unlocked(user).map_err(owner_failure)? { return Err(failure(LOCKED, format!("User {user} must be unlocked"))) } }
        }
        let (mut code_size, mut data_size) = (0i64, 0i64);
        for &user in &users {
            let (code, data) = self.storage.measure(package, user).map_err(owner_failure)?;
            code_size = code_size.checked_add(code).ok_or_else(|| failure(INTERNAL, "Move size overflow"))?;
            data_size = data_size.checked_add(data).ok_or_else(|| failure(INTERNAL, "Move size overflow"))?;
        }
        let size = if volume.complete { code_size.checked_add(data_size).ok_or_else(|| failure(INTERNAL, "Move size overflow"))? } else { code_size };
        if size < 0 { return Err(failure(INTERNAL, "Negative measured move size")) }
        let start_free = self.storage.bytes(&volume.measure_path, false).map_err(owner_failure)?;
        if size > self.storage.bytes(&volume.measure_path, true).map_err(owner_failure)? { return Err(failure(INTERNAL, "Not enough free space to move")) }
        let from_code_path = package.path.rsplit_once('/').filter(|(parent, _)| parent.rsplit('/').next().is_some_and(|name| name.starts_with("~~"))).map_or_else(|| package.path.clone(), |(parent, _)| parent.into());
        let plan = Plan { move_id: id, package: package.name.clone(), version: package.version_code, app_id: package.app_id,
            source_volume: package.volume_uuid.clone(), destination_volume: destination, code_path: package.path.clone(), from_code_path,
            users, install_user: first, seinfo: package.seinfo.clone(), target_sdk: code.target_sdk_version,
            abi_override: package.cpu_abi_override.clone(), install_source: package.install_source.clone(), complete: volume.complete, originally_external: external, install_flags: 0x10 | 0x2 };
        self.storage.prepare(&plan).map_err(owner_failure)?;
        let mut prepared = self.install.prepare(&plan)?;
        self.callbacks.changed(id, 10, -1).map_err(owner_failure)?;
        let done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let progress = if plan.complete {
            let storage = self.storage.clone(); let callbacks = self.callbacks.clone(); let done = done.clone(); let measure = volume.measure_path; let errors = self.errors.clone();
            Some(thread::Builder::new().name(format!("move-progress-{id}")).spawn(move || {
                let mut completed = done.0.lock().unwrap();
                while !*completed {
                    completed = done.1.wait_timeout(completed, Duration::from_secs(1)).unwrap().0;
                    if *completed { break }
                    let available = match storage.bytes(&measure, false) { Ok(bytes) => bytes, Err(error) => { errors.lock().unwrap().push(owner_failure(error)); break } };
                    let progress = if size == 0 { 10 } else { 10 + (((start_free as i128 - available as i128) * 80 / size as i128).clamp(0, 80) as i32) };
                    if let Err(error) = callbacks.changed(id, progress, -1) { errors.lock().unwrap().push(owner_failure(error)); break }
                }
            }).map_err(|error| failure(INTERNAL, format!("Move progress worker: {error}")))?)
        } else { None };
        let progress = Progress { done, thread: progress, errors: self.errors.clone() };
        let result = (|| {
            if plan.complete { self.storage.relocate(&plan).map_err(owner_failure)?; }
            else { prepared.copy_code()?; }
            let receipt = prepared.commit()?;
            let published = (self.source)().map_err(owner_failure)?;
            let actual = published.packages.get(&plan.package).ok_or_else(|| failure(INTERNAL, "Move publication missing package"))?;
            if published.generation != receipt.generation || receipt.package != plan.package || receipt.version != plan.version
                || actual.version_code != receipt.version || actual.path != receipt.path || actual.volume_uuid != plan.destination_volume
                || receipt.volume != plan.destination_volume || !plan.users.iter().all(|user| actual.users.get(user).is_none_or(|state| state.installed)) {
                return Err(Failure { status: INTERNAL, message: "Move publication receipt differs from native state".into(), committed: true });
            }
            self.install.finish(&plan, &receipt)
        })();
        drop(progress);
        if let Err(message) = freezer.close() {
            return Err(Failure { status: INTERNAL, message,
                committed: result.as_ref().map_or_else(|error| error.committed, |_| true) });
        }
        result?; self.callbacks.changed(id, SUCCESS, -1).map_err(owner_failure)
    }
}
struct Progress { done: Arc<(Mutex<bool>, std::sync::Condvar)>, thread: Option<JoinHandle<()>>, errors: Arc<Mutex<Vec<Failure>>> }
impl Drop for Progress {
    fn drop(&mut self) {
        *self.done.0.lock().unwrap() = true; self.done.1.notify_all();
        if let Some(worker) = self.thread.take() { if worker.join().is_err() { self.errors.lock().unwrap().push(failure(INTERNAL, "Move progress worker panicked")); } }
    }
}
fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
