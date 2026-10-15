//! Native InstantAppRegistry disk state, cookies and access grants.
//! Ported from android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
use super::{info::ApplicationInfo, intent::Intent, model::State, pkg::AndroidPackage};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, fs, io, path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, atomic::{AtomicI32, AtomicU64, Ordering}}, thread::{self, JoinHandle}, time::{Duration, Instant}};

pub const DEFAULT_INSTALLED_MIN_CACHE_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const DEFAULT_UNINSTALLED_MIN_CACHE_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;
pub const DEFAULT_INSTALLED_MAX_CACHE_MILLIS: i64 = 6 * 30 * 24 * 60 * 60 * 1000;
pub const DEFAULT_UNINSTALLED_MAX_CACHE_MILLIS: i64 = 6 * 30 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Access {
    pub installed: BTreeMap<i32, BTreeSet<i32>>,
    grants: BTreeSet<(i32, i32, i32)>,
}
impl Access {
    pub fn granted(&self, user: i32, recipient: i32, instant: i32) -> bool {
        self.grants.contains(&(user, recipient, instant))
    }
}
#[derive(Clone, Debug)]
pub struct App {
    pub package: Option<String>,
    pub label: Option<String>,
    pub requested: Vec<Option<String>>,
    pub granted: Vec<Option<String>>,
    pub application: Option<ApplicationInfo>,
}
impl WriteParcelable for App {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_string16(self.package.as_deref());
        crate::clip::write_char_sequence(parcel, self.label.as_deref());
        strings(parcel, &self.requested);
        strings(parcel, &self.granted);
        parcel.write_string16(self.application.as_ref().map(|_| "android.content.pm.ApplicationInfo"));
        if let Some(application) = &self.application { application.write_to(parcel); }
    }
}
fn strings(parcel: &mut Parcel, values: &[Option<String>]) {
    parcel.write_i32(values.len() as i32);
    for value in values { parcel.write_string16(value.as_deref()); }
}
#[derive(Clone)]
struct SavedApp { app: App, timestamp: i64 }
struct Pending { bytes: Option<Vec<u8>>, path: PathBuf, due: Instant }
#[derive(Default)]
struct Live {
    access: Access,
    uninstalled: BTreeMap<i32, Vec<SavedApp>>,
    pending: BTreeMap<(i32, String), Pending>,
    errors: Vec<String>,
    stopped: bool,
}
struct Shared { live: Mutex<Live>, wake: Condvar }
pub type CookieLimitSource = Arc<dyn Fn() -> Result<i32, String> + Send + Sync>;
enum CookieLimit { Fixed(AtomicI32), Original(CookieLimitSource) }
pub struct Owner {
    data: PathBuf,
    cookie_limit: CookieLimit,
    density_dpi: i32,
    shared: Arc<Shared>,
}
impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("InstantRegistry").finish_non_exhaustive() }
}
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
pub struct Worker { shared: Arc<Shared>, thread: Option<JoinHandle<()>> }
impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.live.lock().unwrap().stopped = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                self.shared.live.lock().unwrap().errors.push("instant cookie worker panicked".into());
            }
        }
    }
}
impl Owner {
    pub fn stop(&self) {
        self.shared.live.lock().unwrap().stopped = true;
        self.shared.wake.notify_all();
    }
    /// data is the native service's mapped Android /data, never the original image.
    /// The worker guard must outlive publication and be dropped outside bootstrap locks.
    pub fn open(data: &Path, state: &State, cookie_limit: i32, density_dpi: i32) -> Result<(Arc<Self>, Worker), String> {
        Self::open_inner(data, state, CookieLimit::Fixed(AtomicI32::new(cookie_limit)), density_dpi)
    }
    /// Original PMS reads PackageManager's Settings-backed limit when handling
    /// setInstantAppCookie, never when constructing InstantAppRegistry.
    pub fn open_with_cookie_limit(data: &Path, state: &State, cookie_limit: CookieLimitSource, density_dpi: i32) -> Result<(Arc<Self>, Worker), String> {
        Self::open_inner(data, state, CookieLimit::Original(cookie_limit), density_dpi)
    }
    fn open_inner(data: &Path, state: &State, cookie_limit: CookieLimit, density_dpi: i32) -> Result<(Arc<Self>, Worker), String> {
        let mut live = Live::default();
        for package in state.packages.values() {
            for (user, user_state) in &package.users {
                if user_state.installed && user_state.instant_app {
                    live.access.installed.entry(*user).or_default().insert(package.app_id);
                }
            }
        }
        let shared = Arc::new(Shared { live: Mutex::new(live), wake: Condvar::new() });
        let owner = Arc::new(Self { data: data.to_owned(), cookie_limit, density_dpi, shared: shared.clone() });
        let worker_owner = owner.clone();
        let thread = thread::Builder::new().name("instant-cookie".into()).spawn(move || worker_owner.persist_worker())
            .map_err(|error| error.to_string())?;
        Ok((owner, Worker { shared, thread: Some(thread) }))
    }
    pub fn snapshot(&self) -> Access { self.shared.live.lock().unwrap().access.clone() }
    pub fn has_metadata(&self, package: Option<&str>, user: i32) -> Result<bool, super::apps_filter::NotModelled> {
        let package = package.ok_or(super::apps_filter::NotModelled("null instant metadata package"))?;
        if self.shared.live.lock().unwrap().uninstalled.get(&user).is_some_and(|apps|
            apps.iter().any(|saved| saved.app.package.as_deref() == Some(package))) {
            return Ok(true);
        }
        let directory = self.directory(user, package)
            .map_err(|_| super::apps_filter::NotModelled("instant metadata disk owner unavailable"))?;
        if ["metadata.xml", "icon.png", "android_id"].iter().any(|name| directory.join(name).exists()) {
            return Ok(true);
        }
        self.peek_cookie(user, package).map(|cookie| cookie.is_some())
            .map_err(|_| super::apps_filter::NotModelled("instant metadata cookie owner unavailable"))
    }
    pub fn set_cookie_limit(&self, limit: i32) -> Result<(), String> {
        match &self.cookie_limit {
            CookieLimit::Fixed(current) => { current.store(limit, Ordering::Release); Ok(()) },
            CookieLimit::Original(_) => Err("live original cookie policy cannot be overridden".into()),
        }
    }
    pub fn take_errors(&self) -> Vec<String> { std::mem::take(&mut self.shared.live.lock().unwrap().errors) }
    fn directory(&self, user: i32, package: &str) -> Result<PathBuf, String> {
        if user < 0 || package.is_empty() || package.contains('/') || package == "." || package == ".." {
            return Err("invalid instant application disk identity".into());
        }
        Ok(self.data.join(format!("system/users/{user}/instant")).join(package))
    }
    fn record_io(&self, operation: &str, error: io::Error) {
        self.shared.live.lock().unwrap().errors.push(format!("{operation}: {error}"));
    }
    fn peek_cookie(&self, user: i32, package: &str) -> Result<Option<PathBuf>, String> {
        let directory = self.directory(user, package)?;
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => { self.record_io("list instant cookie directory", error); return Ok(None); }
        };
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name(); let name = name.to_string_lossy();
            if !entry.file_type().map_err(|error| error.to_string())?.is_dir()
                && name.starts_with("cookie_") && name.ends_with(".dat") { return Ok(Some(entry.path())); }
        }
        Ok(None)
    }
    pub fn android_id(&self, user: i32, package: &str) -> Result<String, String> {
        let directory = self.directory(user, package)?; let file = directory.join("android_id");
        match fs::read(&file) {
            Ok(bytes) => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => self.record_io("read instant app scoped Android id", error),
        }
        let mut random = [0u8; 8];
        // SAFETY: getentropy fills the writable eight-byte buffer.
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 { return Err(io::Error::last_os_error().to_string()); }
        let id = random.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        if let Err(error) = fs::create_dir_all(&directory) { self.record_io("create instant id directory", error); return Ok(id); }
        if let Err(error) = fs::write(&file, id.as_bytes()) { self.record_io("persist instant app scoped Android id", error); }
        Ok(id)
    }

    pub fn cookie(&self, user: i32, package: &str) -> Result<Option<Vec<u8>>, String> {
        if let Some(bytes) = self.shared.live.lock().unwrap().pending.get(&(user, package.into()))
            .and_then(|pending| pending.bytes.clone()) { return Ok(Some(bytes)); }
        let Some(path) = self.peek_cookie(user, package)? else { return Ok(None); };
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) => { self.record_io("read instant cookie", error); Ok(None) }
        }
    }
    pub fn set_cookie(&self, user: i32, package: &AndroidPackage, bytes: Option<Vec<u8>>) -> Result<bool, String> {
        let limit = match &self.cookie_limit {
            CookieLimit::Fixed(limit) => limit.load(Ordering::Acquire),
            CookieLimit::Original(read) => read()?,
        };
        if bytes.as_ref().is_some_and(|bytes| !bytes.is_empty() && bytes.len() as i64 > i64::from(limit)) { return Ok(false); }
        let signatures = package.signing_details.as_ref().and_then(|details| details.signatures.as_ref())
            .ok_or("instant cookie parsed signing details unavailable")?;
        let digest = signatures_digest(signatures);
        let path = self.directory(user, &package.package_name)?.join(format!("cookie_{digest}.dat"));
        if let Some(old) = self.peek_cookie(user, &package.package_name)? {
            if old != path { if let Err(error) = fs::remove_file(old) { self.record_io("delete legacy instant cookie", error); } }
        }
        let mut live = self.shared.live.lock().unwrap();
        if live.stopped { return Err("instant cookie owner stopped".into()); }
        live.pending.insert((user, package.package_name.clone()), Pending { bytes, path, due: Instant::now() + Duration::from_secs(1) });
        self.shared.wake.notify_all();
        Ok(true)
    }
    fn persist_worker(&self) {
        loop {
            let mut live = self.shared.live.lock().unwrap();
            if live.stopped { return; }
            let Some(due) = live.pending.values().map(|pending| pending.due).min() else {
                drop(self.shared.wake.wait(live).unwrap()); continue;
            };
            let now = Instant::now();
            if due > now { drop(self.shared.wake.wait_timeout(live, due - now).unwrap()); continue; }
            let keys = live.pending.iter().filter(|(_, pending)| pending.due <= now).map(|(key, _)| key.clone()).collect::<Vec<_>>();
            for key in keys {
                let pending = live.pending.remove(&key).unwrap();
                // The original Handler removes pending work before persistence.
                drop(live);
                let result = (|| -> io::Result<()> {
                    fs::create_dir_all(pending.path.parent().unwrap())?;
                    match fs::remove_file(&pending.path) { Ok(()) => {}, Err(error) if error.kind() == io::ErrorKind::NotFound => {}, Err(error) => self.record_io("delete instant cookie", error) }
                    if let Some(bytes) = pending.bytes.filter(|bytes| !bytes.is_empty()) { fs::write(&pending.path, bytes)?; }
                    Ok(())
                })();
                if let Err(error) = result { self.record_io("persist instant cookie", error); }
                live = self.shared.live.lock().unwrap();
            }
        }
    }
    pub fn uninstalled(&self, user: i32) -> Result<Vec<App>, String> {
        if let Some(cached) = self.shared.live.lock().unwrap().uninstalled.get(&user) { return Ok(cached.iter().map(|saved| saved.app.clone()).collect()); }
        let root = self.data.join(format!("system/users/{user}/instant"));
        let entries = match fs::read_dir(root) {
            Ok(entries) => Some(entries), Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        let mut apps = Vec::new();
        for entry in entries.into_iter().flatten() {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry.file_type().map_err(|error| error.to_string())?.is_dir() { continue; }
            let file = entry.path().join("metadata.xml");
            let backup = entry.path().join("metadata.xml.bak");
            if backup.exists() { fs::rename(&backup, &file).map_err(|error| error.to_string())?; }
            let staging = entry.path().join("metadata.xml.new");
            if staging.exists() && file.exists() { self.delete_file(&staging); }
            let bytes = match fs::read(&file) { Ok(bytes) => bytes, Err(error) if error.kind() == io::ErrorKind::NotFound => continue, Err(error) => return Err(format!("instant metadata: {error}")) };
            let xml = aim_android_xml::read(&bytes).map_err(|error| format!("instant metadata: {error}"))?;
            let root = if xml.name == "package" { Some(&xml) } else { xml.children().find(|child| child.name == "package") };
            let root = root.ok_or("null InstantAppInfo in original uninstalled metadata list")?;
            let mut requested = Vec::new(); let mut granted = Vec::new();
            for permissions in root.children().filter(|child| child.name == "permissions") {
                for permission in permissions.children().filter(|child| child.name == "permission") {
                    let name = permission.string("name").map(|value| value.into_owned());
                    requested.push(name.clone());
                    if permission.bool("granted")?.unwrap_or(false) { granted.push(name); }
                }
            }
            let timestamp = file.metadata().and_then(|metadata| metadata.modified()).map_err(|error| error.to_string())?
                .duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_millis() as i64;
            apps.push(SavedApp { timestamp, app: App { package: Some(entry.file_name().to_string_lossy().into_owned()), label: root.string("label").map(|value| value.into_owned()), requested, granted, application: None } });
        }
        self.shared.live.lock().unwrap().uninstalled.insert(user, apps.clone());
        Ok(apps.into_iter().map(|saved| saved.app).collect())
    }
    pub fn installed(&self, package: &super::model::PackageState, users: &[i32],
        permission_definitions: &[super::permissions::Permission]) -> Result<Vec<PermissionGrant>, String> {
        let Some(pkg) = package.pkg.as_deref() else { return Ok(Vec::new()); };
        let mut grants = Vec::new();
        for user in users {
            let state = super::info::user_state(package, *user);
            if !state.installed { continue; }
            for prior in self.uninstalled(*user)? {
                if prior.package.as_deref() != Some(pkg.package_name.as_str()) { continue; }
                for permission in prior.granted.iter().flatten() {
                    if !pkg.requested_permissions.contains(permission) { continue; }
                    let eligible = permission_definitions.iter().find(|definition| &definition.name == permission)
                        .is_some_and(|definition| (definition.protection_level & 15 == 1
                            || definition.protection_level & 0x20 != 0) && definition.protection_level & 0x1000 != 0);
                    if eligible { grants.push(PermissionGrant { package: pkg.package_name.clone(), user: *user, permission: permission.clone() }); }
                }
            }
            {
                let mut live = self.shared.live.lock().unwrap();
                if state.instant_app { live.access.installed.entry(*user).or_default().insert(package.app_id); }
                if let Some(cached) = live.uninstalled.get_mut(user) { cached.retain(|saved| saved.app.package.as_deref() != Some(pkg.package_name.as_str())); }
            }
            let directory = self.directory(*user, &pkg.package_name)?;
            self.delete_file(&directory.join("metadata.xml")); self.delete_file(&directory.join("icon.png"));
            let Some(cookie) = self.peek_cookie(*user, &pkg.package_name)? else { continue; };
            let name = cookie.file_name().unwrap().to_string_lossy();
            let digest = &name["cookie_".len()..name.len() - ".dat".len()];
            let details = pkg.signing_details.as_ref().ok_or("installed instant signing owner unavailable")?;
            let current = details.signatures.as_ref().ok_or("installed instant current certificates unavailable")?;
            let same = signatures_digest(current) == digest
                || current.iter().any(|certificate| signature_digest(certificate) == digest)
                || package.signatures.as_ref().and_then(|signatures| signatures.past_signatures.as_ref())
                    .is_some_and(|past| past.iter().any(|(certificate, flags)| signature_digest(certificate) == digest && flags & 1 != 0));
            if same { return Ok(grants); }
            self.shared.live.lock().unwrap().pending.remove(&(*user, pkg.package_name.clone()));
            self.delete_file(&cookie);
        }
        Ok(grants)
    }
    fn delete_file(&self, file: &Path) {
        match fs::remove_file(file) { Ok(()) => {}, Err(error) if error.kind() == io::ErrorKind::NotFound => {}, Err(error) => self.record_io("delete instant metadata file", error) }
    }
    pub fn uninstall(&self, user: i32, app_id: i32, instant: bool,
        app: App, icon_png: Option<&[u8]>, binary_xml: bool) -> Result<(), String> {
        let package = app.package.as_deref().ok_or("uninstalled instant app requires resolved package name")?;
        if instant {
            let directory = self.directory(user, package)?;
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            self.write_metadata(user, &app, binary_xml)?;
            if let Some(png) = icon_png { fs::write(directory.join("icon.png"), png).map_err(|error| error.to_string())?; }
            self.shared.live.lock().unwrap().uninstalled.entry(user).or_default().push(SavedApp { app, timestamp: unix_millis() });
            self.remove_installed(user, app_id);
        } else {
            let directory = self.directory(user, package)?;
            match fs::remove_dir_all(directory) { Ok(()) => {}, Err(error) if error.kind() == io::ErrorKind::NotFound => {}, Err(error) => self.record_io("delete uninstalled instant data", error) }
            let mut live = self.shared.live.lock().unwrap();
            live.pending.remove(&(user, package.into()));
            live.access.grants.retain(|(grant_user, recipient, _)| *grant_user != user || *recipient != app_id);
        }
        Ok(())
    }
    pub fn remove_user(&self, user: i32) {
        let mut live = self.shared.live.lock().unwrap();
        live.uninstalled.remove(&user); live.access.installed.remove(&user);
        live.access.grants.retain(|(grant_user, _, _)| *grant_user != user);
        drop(live);
        match fs::remove_dir_all(self.data.join(format!("system/users/{user}/instant"))) {
            Ok(()) => {}, Err(error) if error.kind() == io::ErrorKind::NotFound => {}, Err(error) => self.record_io("delete instant user data", error)
        }
    }
    pub fn delete_metadata(&self, user: i32, package: &str) -> Result<(), String> {
        if let Some(cached) = self.shared.live.lock().unwrap().uninstalled.get_mut(&user) { cached.retain(|saved| saved.app.package.as_deref() != Some(package)); }
        let directory = self.directory(user, package)?;
        for file in ["metadata.xml", "icon.png", "android_id"] { self.delete_file(&directory.join(file)); }
        if let Some(cookie) = self.peek_cookie(user, package)? { self.delete_file(&cookie); }
        Ok(())
    }
    pub fn write_metadata(&self, user: i32, app: &App, binary_xml: bool) -> Result<(), String> {
        use aim_android_xml::{Element, Node, Value};
        use std::io::Write;
        let package = app.package.as_deref().ok_or("instant metadata package unavailable")?;
        let directory = self.directory(user, package)?;
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let permissions = app.requested.iter().map(|permission| {
            let permission = permission.as_ref().ok_or("instant metadata requested permission is null")?;
            let mut attrs = vec![("name".into(), Value::String(permission.clone()))];
            if app.granted.iter().any(|granted| granted.as_ref() == Some(permission)) { attrs.push(("granted".into(), Value::Bool(true))); }
            Ok(Node::Element(Element { name: "permission".into(), attrs, content: Vec::new() }))
        }).collect::<Result<Vec<_>, String>>()?;
        let root = Element { name: "package".into(), attrs: vec![("label".into(), Value::String(app.label.clone().ok_or("instant metadata resolved label unavailable")?))],
            content: vec![Node::Element(Element { name: "permissions".into(), attrs: Vec::new(), content: permissions })] };
        let bytes = if binary_xml { aim_android_xml::abx::write(&root)? } else { metadata_text(app)? };
        let file = directory.join("metadata.xml"); let staging = directory.join("metadata.xml.new");
        let result = (|| -> io::Result<()> { let mut output = fs::File::create(&staging)?; output.write_all(&bytes)?; output.sync_all()?; drop(output); fs::rename(&staging, &file) })();
        if let Err(error) = result { self.delete_file(&staging); return Err(error.to_string()); }
        Ok(())
    }

    /// FreeStorageHelper invokes these stages with its original configured cache
    /// ages. Installed deletion is the native DeletePackageHelper actor.
    pub fn prune_installed(&self, state: &State, users: &[i32], latest_use: &BTreeMap<String, i64>,
        needed: i64, max_age: i64, usable: &dyn Fn() -> Result<i64, String>,
        delete: &dyn Fn(&str) -> Result<bool, String>) -> Result<bool, String> {
        if usable()? >= needed { return Ok(true); }
        let now = unix_millis();
        let mut packages = Vec::new();
        let order = state.system.settings_package_order.as_ref().ok_or("instant pruning Settings registration order unavailable")?;
        for name in order {
            let package = state.packages.get(name).ok_or("instant pruning registered package unavailable")?;
            let Some(_) = package.pkg.as_ref() else { continue; };
            let used = *latest_use.get(name).ok_or("instant pruning latest package usage owner unavailable")?;
            if now.wrapping_sub(used) < max_age { continue; }
            let mut installed = false; let mut all_instant = true;
            for user in users {
                let user_state = super::info::user_state(package, *user);
                if !user_state.installed { continue; }
                installed = true;
                if !user_state.instant_app { all_instant = false; break; }
            }
            if installed && all_instant {
                let first = package.users.values().map(|user| user.first_install_time).filter(|time| *time != 0).min().unwrap_or(0);
                packages.push((name.clone(), used, first));
            }
        }
        // The pinned Java comparator returns -1 for equal install timestamps.
        // Insertion ordering preserves that comparison without Rust's total-order requirement.
        for index in 1..packages.len() {
            let mut cursor = index;
            while cursor > 0 && (packages[cursor].1 < packages[cursor - 1].1
                || packages[cursor].1 == packages[cursor - 1].1 && packages[cursor].2 <= packages[cursor - 1].2) {
                packages.swap(cursor, cursor - 1); cursor -= 1;
            }
        }
        for (name, _, _) in packages { if delete(&name)? && usable()? >= needed { return Ok(true); } }
        self.prune_uninstalled(users, needed, i64::MAX, usable)
    }
    pub fn prune_uninstalled(&self, users: &[i32], needed: i64, max_age: i64,
        usable: &dyn Fn() -> Result<i64, String>) -> Result<bool, String> {
        if usable()? >= needed { return Ok(true); }
        for user in users {
            if let Some(cached) = self.shared.live.lock().unwrap().uninstalled.get_mut(user) {
                cached.retain(|saved| unix_millis().wrapping_sub(saved.timestamp) <= max_age);
            }
            let directory = self.data.join(format!("system/users/{user}/instant"));
            let entries = match fs::read_dir(directory) { Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => { self.record_io("list instant prune directory", error); continue; } };
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if !entry.file_type().map_err(|error| error.to_string())?.is_dir() { continue; }
                let metadata = entry.path().join("metadata.xml");
                let modified = match metadata.metadata().and_then(|metadata| metadata.modified()) {
                    Ok(modified) => modified, Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.to_string()),
                };
                let timestamp = modified.duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_millis() as i64;
                if unix_millis().wrapping_sub(timestamp) > max_age {
                    if let Err(error) = fs::remove_dir_all(entry.path()) { self.record_io("delete expired instant cache", error); }
                    if usable()? >= needed { return Ok(true); }
                }
            }
        }
        Ok(false)
    }

    pub fn grant_access(&self, user: i32, intent: Option<&Intent>, recipient: i32, instant: i32) -> bool {
        let mut live = self.shared.live.lock().unwrap();
        let installed = live.access.installed.get(&user);
        if !installed.is_some_and(|installed| installed.contains(&instant))
            || installed.is_some_and(|installed| installed.contains(&recipient)) { return false; }
        if intent.is_some_and(|intent| intent.action.as_deref() == Some("android.intent.action.VIEW")
            && intent.categories.as_ref().is_some_and(|categories| categories.iter().any(|category| category == "android.intent.category.BROWSABLE"))) { return false; }
        live.access.grants.insert((user, recipient, instant)); true
    }
    pub fn add_installed(&self, user: i32, app_id: i32) { self.shared.live.lock().unwrap().access.installed.entry(user).or_default().insert(app_id); }
    pub fn remove_installed(&self, user: i32, app_id: i32) {
        let mut live = self.shared.live.lock().unwrap();
        if let Some(installed) = live.access.installed.get_mut(&user) { installed.remove(&app_id); }
        live.access.grants.retain(|(grant_user, recipient, instant)| *grant_user != user || *recipient != app_id && *instant != app_id);
    }
    pub fn icon(&self, user: i32, package: &str) -> Result<Option<Bitmap>, String> {
        let file = self.directory(user, package)?.join("icon.png");
        let bytes = match fs::read(file) { Ok(bytes) => bytes, Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None), Err(error) => { self.record_io("read instant icon", error); return Ok(None); } };
        Bitmap::decode(&bytes, self.density_dpi)
    }
}
pub struct PermissionGrant { pub package: String, pub user: i32, pub permission: String }
fn signature_digest(certificate: &[u8]) -> String {
    Sha256::digest(certificate).iter().map(|byte| format!("{byte:02X}")).collect()
}
fn signatures_digest(signatures: &[Vec<u8>]) -> String {
    let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02X}")).collect::<String>();
    let mut digests = signatures.iter().map(|signature| hex(&Sha256::digest(signature))).collect::<Vec<_>>();
    if digests.len() == 1 { return digests.pop().unwrap(); }
    digests.sort(); hex(&Sha256::digest(digests.concat().as_bytes()))
}

static NEXT_BITMAP_ID: AtomicU64 = AtomicU64::new(1);
pub enum Bitmap {
    Pixels { width: i32, height: i32, density_dpi: i32, id: u64, pixels: Vec<u8> },
    Original(Parcel),
}
impl Bitmap {
    pub fn from_owned_parcel(body: Parcel) -> Self { Self::Original(body) }
    pub(crate) fn decode(bytes: &[u8], density_dpi: i32) -> Result<Option<Self>, String> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = match decoder.read_info() { Ok(reader) => reader, Err(_) => return Ok(None) };
        let mut bytes = vec![0; reader.output_buffer_size()];
        let info = match reader.next_frame(&mut bytes) { Ok(info) => info, Err(_) => return Ok(None) };
        let mut pixels = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        let channels = match info.color_type { png::ColorType::Rgba => 4, png::ColorType::Rgb => 3, png::ColorType::GrayscaleAlpha => 2, png::ColorType::Grayscale => 1, _ => return Err("instant icon expanded PNG color type unsupported".into()) };
        for pixel in bytes[..info.buffer_size()].chunks_exact(channels) {
            let (red, green, blue, alpha) = match channels { 4 => (pixel[0], pixel[1], pixel[2], pixel[3]), 3 => (pixel[0], pixel[1], pixel[2], 255), 2 => (pixel[0], pixel[0], pixel[0], pixel[1]), _ => (pixel[0], pixel[0], pixel[0], 255) };
            let premul = |value: u8| ((u32::from(value) * u32::from(alpha) + 127) / 255) as u8;
            pixels.extend_from_slice(&[premul(red), premul(green), premul(blue), alpha]);
        }
        Ok(Some(Self::Pixels { width: info.width.try_into().map_err(|_| "instant icon width exceeds Java int")?, height: info.height.try_into().map_err(|_| "instant icon height exceeds Java int")?, density_dpi, id: NEXT_BITMAP_ID.fetch_add(1, Ordering::Relaxed), pixels }))
    }
}
impl WriteParcelable for Bitmap {
    fn write_to(&self, parcel: &mut Parcel) {
        match self {
            Self::Original(body) => parcel.write_raw_files(body.data(), body.objects(), body.files()),
            Self::Pixels { width, height, density_dpi, id, pixels } => {
                parcel.write_i32(0); parcel.write_i32(4); parcel.write_i32(2);
                parcel.write_i32(-1); // default sRGB
                parcel.write_i32(*width); parcel.write_i32(*height); parcel.write_i32(*width * 4);
                parcel.write_i32(*density_dpi); parcel.write_i64(*id as i64);
                parcel.write_i32(0); // inline native Bitmap blob
                parcel.write_i32(pixels.len() as i32); parcel.write_raw(pixels, &[]);
            }
        }
    }
}

fn metadata_text(app: &App) -> Result<Vec<u8>, String> {
    fn attribute(value: &str) -> Result<String, String> {
        let mut escaped = String::new();
        for ch in value.chars() {
            match ch {
                '&' => escaped.push_str("&amp;"), '<' => escaped.push_str("&lt;"), '>' => escaped.push_str("&gt;"),
                '"' => escaped.push_str("&quot;"), '\n' => escaped.push_str("&#10;"), '\r' => escaped.push_str("&#13;"), '\t' => escaped.push_str("&#9;"),
                ch if ch < ' ' => return Err("invalid instant metadata XML character".into()),
                ch => escaped.push(ch),
            }
        }
        Ok(escaped)
    }
    let label = attribute(app.label.as_deref().ok_or("instant metadata resolved label unavailable")?)?;
    let mut text = format!("<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<package label=\"{label}\">\n  <permissions>\n");
    for permission in &app.requested {
        let permission = permission.as_ref().ok_or("instant metadata requested permission is null")?;
        let name = attribute(permission)?;
        let granted = if app.granted.iter().any(|granted| granted.as_ref() == Some(permission)) { " granted=\"true\"" } else { "" };
        text.push_str(&format!("    <permission name=\"{name}\"{granted} />\n"));
    }
    text.push_str("  </permissions>\n</package>\n");
    Ok(text.into_bytes())
}

fn unix_millis() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as i64,
        Err(error) => -(error.duration().as_millis() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn cookie_policy_is_lazy_dynamic_and_failure_preserves_existing_cookie() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let data = std::env::temp_dir().join(format!("aim-instant-policy-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&data).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
        let _cleanup = Cleanup(data.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let value = Arc::new(AtomicI32::new(3));
        let read_calls = calls.clone();
        let read_value = value.clone();
        let source: CookieLimitSource = Arc::new(move || {
            read_calls.fetch_add(1, Ordering::AcqRel);
            match read_value.load(Ordering::Acquire) {
                -1 => Err("original Settings owner failed".into()),
                limit => Ok(limit),
            }
        });
        let (owner, worker) = Owner::open_with_cookie_limit(&data, &State::default(), source, 160).unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 0);
        let package = AndroidPackage {
            package_name: "fixture.instant".into(),
            signing_details: Some(super::super::pkg::SigningDetails {
                signatures: Some(vec![vec![7, 7]]), ..Default::default()
            }), ..Default::default()
        };
        assert!(owner.set_cookie(0, &package, Some(vec![1, 2, 3])).unwrap());
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(owner.cookie(0, &package.package_name).unwrap(), Some(vec![1, 2, 3]));
        value.store(2, Ordering::Release);
        assert!(!owner.set_cookie(0, &package, Some(vec![4, 5, 6])).unwrap());
        assert_eq!(owner.cookie(0, &package.package_name).unwrap(), Some(vec![1, 2, 3]));
        value.store(-1, Ordering::Release);
        assert_eq!(owner.set_cookie(0, &package, None).unwrap_err(), "original Settings owner failed");
        assert_eq!(owner.cookie(0, &package.package_name).unwrap(), Some(vec![1, 2, 3]));
        value.store(4, Ordering::Release);
        assert!(owner.set_cookie(0, &package, Some(vec![4, 5, 6, 7])).unwrap());
        assert_eq!(owner.cookie(0, &package.package_name).unwrap(), Some(vec![4, 5, 6, 7]));
        assert_eq!(calls.load(Ordering::Acquire), 4);
        assert!(owner.set_cookie_limit(100).is_err());
        drop(worker);
        assert!(owner.take_errors().is_empty());
    }
}
