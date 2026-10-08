//! Native archival and unarchival state, android-16.0.0_r1 PackageArchiver and
//! PackageInstallerSession (AOSP, Apache-2.0). Graphics and broadcasts use their
//! original owners; package/settings/session changes remain native.
use super::{codec::{Object, SessionParams}, preapproval::IntentSender, removal::{self, External, NativeStore}, Record, Sessions};
use crate::package::{apps_filter, intent::ComponentName, intent_filter::Plain, pkg, query::Query, restrictions::{ArchiveActivity, ArchiveState}, sign};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable, read_byte_array, read_typed, read_typed_list};
use std::{collections::BTreeMap, sync::{Arc, Mutex}};

pub const ARCHIVED: i32 = 1 << 27;
pub const DRAFT: i32 = 1 << 29;
pub const UNARCHIVE: i32 = 1 << 30;
pub const UNSET: i32 = -1;
pub const OK: i32 = 0;
pub const USER_ACTION: i32 = 1;
pub const STORAGE: i32 = 2;

fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
pub fn name_error(message: impl Into<String>) -> Exception {
    let message = message.into();
    let mut payload = Parcel::new();
    payload.write_string16(Some("android.content.pm.PackageManager$NameNotFoundException"));
    payload.write_string16(Some(&message));
    Exception::parcelable(Some(&message), &payload).expect("NameNotFoundException has no capability objects")
}
fn not_found(message: impl Into<String>) -> Exception { name_error(message) }
fn end(reader: &mut Reader<'_>) -> Result<usize, i32> {
    let start = reader.position(); let size = reader.read_i32()?;
    if size < 4 { return Err(BAD_VALUE) }
    let end = start.checked_add(size as usize).ok_or(BAD_VALUE)?;
    if end > reader.position() + reader.remaining() { return Err(BAD_VALUE) }
    Ok(end)
}
fn finish(reader: &mut Reader<'_>, end: usize) -> Result<(), i32> {
    if reader.position() > end { return Err(BAD_VALUE) }
    reader.set_position(end); Ok(())
}

#[derive(Clone, Debug)]
pub struct ArchivedActivity {
    pub title: Option<String>, pub component: Option<ComponentName>, pub icon: Option<Vec<u8>>, pub monochrome: Option<Vec<u8>>,
}
impl ReadParcelable for ArchivedActivity {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let limit = end(reader)?;
        let value = Self { title: reader.read_string16()?, component: read_typed(reader)?, icon: read_byte_array(reader)?, monochrome: read_byte_array(reader)? };
        finish(reader, limit)?; Ok(value)
    }
}
#[derive(Clone, Debug)]
pub struct ArchivedPackage {
    pub name: String, pub signing: Option<pkg::SigningDetails>, pub version: i32, pub version_major: i32, pub target_sdk: i32,
    pub device_storage: Option<String>, pub legacy_storage: Option<String>, pub fragile: Option<String>,
    pub activities: Vec<ArchivedActivity>, raw: Object,
}
impl ReadParcelable for ArchivedPackage {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let start = reader.position(); let limit = end(reader)?;
        let name = reader.read_string16()?.ok_or(BAD_VALUE)?;
        let signing = if reader.read_i32()? == 0 { None } else { pkg::read_signing_details_payload(reader, &mut Plain)? };
        let version = reader.read_i32()?; let version_major = reader.read_i32()?; let target_sdk = reader.read_i32()?;
        let device_storage = reader.read_string16()?; let legacy_storage = reader.read_string16()?; let fragile = reader.read_string16()?;
        let activities = read_typed_list::<ArchivedActivity>(reader)?.unwrap_or_default().into_iter().map(|item| item.ok_or(BAD_VALUE)).collect::<Result<_, _>>()?;
        finish(reader, limit)?; let (bytes, objects) = reader.since(start);
        Ok(Self { name, signing, version, version_major, target_sdk, device_storage, legacy_storage, fragile, activities, raw: Object { bytes: bytes.to_vec(), objects } })
    }
}
impl WriteParcelable for ArchivedPackage { fn write_to(&self, parcel: &mut Parcel) { parcel.write_raw(&self.raw.bytes, &self.raw.objects); } }
impl ArchivedPackage {
    /// ParsingPackageUtils.parsePackageFromPackageLite: there is deliberately
    /// no executable code or manifest component registry in an archived install.
    pub fn package(&self, stage: &str) -> pkg::AndroidPackage {
        use pkg::booleans as b;
        let convert = |value: Option<&str>, fallback| value.map_or(fallback, |value| matches!(value, "true" | "TRUE" | "1"));
        let mut flags = b::ALLOW_BACKUP | b::ALLOW_CLEAR_USER_DATA | b::ALLOW_CLEAR_USER_DATA_ON_FAILED_RESTORE
            | b::ALLOW_NATIVE_HEAP_POINTER_TAGGING | b::ENABLED | b::EXTRACT_NATIVE_LIBS;
        if self.target_sdk >= 29 { flags |= b::ALLOW_AUDIO_PLAYBACK_CAPTURE; }
        if self.target_sdk >= 14 { flags |= b::HARDWARE_ACCELERATED; }
        if self.target_sdk < 28 { flags |= b::USES_CLEARTEXT_TRAFFIC; }
        if convert(self.legacy_storage.as_deref(), self.target_sdk < 29) { flags |= b::REQUEST_LEGACY_EXTERNAL_STORAGE; }
        if convert(self.device_storage.as_deref(), false) { flags |= b::DEFAULT_TO_DEVICE_PROTECTED_STORAGE; }
        if convert(self.fragile.as_deref(), false) { flags |= b::HAS_FRAGILE_USER_DATA; }
        pkg::AndroidPackage {
            feature_flag_state: Some(vec![]), package_name: self.name.clone(), manifest_package_name: Some(self.name.clone()),
            path: Some(stage.into()), base_apk_path: Some(format!("{stage}/base.apk")),
            version_code: self.version, version_code_major: self.version_major, target_sdk_version: self.target_sdk,
            target_sandbox_version: 1, min_sdk_version: 1, category: -1, install_location: -1,
            process_name: Some(self.name.clone()), task_affinity: Some(self.name.clone()),
            signing_details: self.signing.clone(), booleans: flags, gwp_asan_mode: -1, memtag_mode: -1,
            ..Default::default()
        }
    }
    pub fn collected_signing(&self) -> Result<sign::SigningDetails, Exception> {
        let saved = self.signing.as_ref().ok_or_else(|| illegal("Archived package signingDetails missing"))?;
        let signatures = saved.signatures.clone().filter(|signatures| !signatures.is_empty()).ok_or_else(|| illegal("Archived package has no signers"))?;
        let keys = saved.public_keys.as_ref().map(|keys| keys.iter().map(|key| key.as_ref().map(sign::deserialize_public_key).transpose()).collect::<Result<Vec<_>, _>>()).transpose().map_err(illegal)?;
        Ok(sign::SigningDetails { unknown: false, signatures, current_flags: vec![], scheme_version: saved.scheme_version, public_keys: keys,
            past_signing_certificates: saved.past_signing_certificates.as_ref().map(|past| past.iter().map(|certificate| (certificate.clone(), 0)).collect()) })
    }
}
pub fn read_metadata(reader: &mut Reader<'_>) -> Result<ArchiveState, i32> {
    let limit = end(reader)?;
    let title = reader.read_string16()?.ok_or(BAD_VALUE)?; let time = reader.read_i64()?;
    if time < 0 { return Err(BAD_VALUE) }
    let count = reader.read_i32()?; if count < 0 || count as usize > reader.remaining() / 4 { return Err(BAD_VALUE) }
    let mut activities = Vec::new();
    for _ in 0..count {
        if reader.read_i32()? == 0 { return Err(BAD_VALUE) }
        let item_limit = end(reader)?;
        activities.push(ArchiveActivity { title: reader.read_string16()?.ok_or(BAD_VALUE)?, original_component_name: reader.read_string16()?.ok_or(BAD_VALUE)?, icon_path: reader.read_string16()?, monochrome_icon_path: reader.read_string16()? });
        finish(reader, item_limit)?;
    }
    finish(reader, limit)?; Ok(ArchiveState { installer_title: title, archive_time: time, activities })
}

pub fn validate_report(status: i32, required: i64, action: Option<IntentSender>) -> Result<(), Exception> {
    if status == USER_ACTION && action.is_none() { return Err(Exception::new(EX_NULL_POINTER, "null userActionIntent")) }
    if status == STORAGE && required <= 0 { return Err(illegal("Insufficient storage error set, but requiredStorageBytes unspecified.")) }
    if status != STORAGE && required > 0 { return Err(illegal(format!("requiredStorageBytes set, but error is {status}."))) }
    if !matches!(status, 0..=5 | 100) { return Err(illegal(format!("Invalid status code passed {status}"))) }
    Ok(())
}
pub type DraftRecord = Arc<dyn Fn(&str, &str, i32, &str) -> Result<Record, Exception> + Send + Sync>;
pub type SaveSessions = Arc<dyn Fn() -> Result<(), Exception> + Send + Sync>;
#[derive(Clone)]
struct Listener { sender: IntentSender, _reference: Arc<aim_binder_host::local::Strong> }
#[derive(Clone)]
struct Unarchive {
    package: String, installer: String, installer_uid: u32, user: i32, title: String,
    status: i32, listeners: Vec<Listener>,
}
pub struct Drafts {
    sessions: Arc<Sessions>, entries: Mutex<BTreeMap<i32, Unarchive>>, create: DraftRecord, save: SaveSessions,
    operations: Arc<dyn super::SessionOperations>,
    timer: super::archive_timer::Timer,
}
impl Drafts {
    /// Called by the installer handler's 120-second draft cleanup timer.
    /// A claimed session has had DRAFT cleared and is not abandoned here.
    pub fn expire(&self, id: i32) -> Result<(), Exception> {
        let session = self.sessions.snapshot(id)?;
        if session.parameters.install_flags & DRAFT != 0 {
            self.operations.abandon_session(id, session.installer_uid)?;
            (self.save)()?;
            self.entries.lock().unwrap().remove(&id);
        }
        Ok(())
    }
    pub fn new(sessions: Arc<Sessions>, create: DraftRecord, save: SaveSessions, operations: Arc<dyn super::SessionOperations>) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self { sessions, entries: Mutex::new(BTreeMap::new()), create, save, operations, timer: super::archive_timer::Timer::new(weak.clone()) })
    }
    fn attach(&self, record: Record, package: &str, installer: &str, title: &str, receiver: IntentSender, external: &External) -> Result<i32, Exception> {
        let listener = Listener { sender: receiver, _reference: external.retain_sender(receiver)? };
        if record.params.app_package_name.as_deref() != Some(package) || record.installer_package.as_deref() != Some(installer)
            || record.params.install_flags & (DRAFT | UNARCHIVE) != DRAFT | UNARCHIVE { return Err(illegal("Internal draft normalization differs")) }
        let mut entries = self.entries.lock().unwrap();
        if let Some((&id, current)) = entries.iter_mut().find(|(_, entry)| entry.package == package && entry.installer_uid == record.installer_uid && entry.user == record.user as i32) {
            if current.status == OK {
                let current = current.clone(); drop(entries);
                external.unarchive_status(receiver, package, installer, title, current.user, OK, 0, None)?;
                return Ok(id);
            }
            if current.status != UNSET { return Err(illegal(format!("Session {id} has unarchive status {} but is still active.", current.status))) }
            if !current.listeners.iter().any(|listener| listener.sender == receiver) { current.listeners.push(listener); }
            return Ok(id);
        }
        let uid = record.installer_uid; let user = record.user as i32;
        let id = self.sessions.create_record(record, true)?;
        (self.save)()?;
        entries.insert(id, Unarchive { package: package.into(), installer: installer.into(), installer_uid: uid, user, title: title.into(), status: UNSET, listeners: vec![listener] });
        drop(entries); self.timer.schedule(id);
        Ok(id)
    }
    pub fn report(&self, caller: u32, id: i32, user: i32, status: i32, required: i64, action: Option<IntentSender>, external: &External) -> Result<(), Exception> {
        validate_report(status, required, action)?;
        let mut entries = self.entries.lock().unwrap();
        if !entries.contains_key(&id) {
            let (session, record) = self.sessions.records().into_iter().find(|(session, record)| session.id == id && record.user as i32 == user && record.params.app_package_name.is_some())
                .ok_or_else(|| not_found(format!("No valid session with unarchival ID {id} found for user {user}.")))?;
            entries.insert(id, Unarchive { package: record.params.app_package_name.unwrap(), installer: record.installer_package.unwrap_or_default(), installer_uid: session.installer_uid, user, title: String::new(), status: UNSET, listeners: vec![] });
        }
        let entry = entries.get_mut(&id).filter(|entry| entry.user == user).ok_or_else(|| not_found(format!("No valid session with unarchival ID {id} found for user {user}.")))?;
        if caller != 0 && caller != entry.installer_uid { return Err(Exception::security(format!("The caller UID {caller} does not have access to the session with unarchiveId {id}."))) }
        if entry.status != UNSET { return Err(illegal(format!("Unarchival status for ID {id} has already been set or a session has been created for it already by the caller."))) }
        entry.status = status; let entry = entry.clone(); drop(entries);
        for listener in entry.listeners { external.unarchive_status(listener.sender, &entry.package, &entry.installer, &entry.title, user, status, required, action)?; }
        if status != OK { self.operations.abandon_session(id, entry.installer_uid)?; (self.save)()?; self.entries.lock().unwrap().remove(&id); }
        Ok(())
    }
}

pub struct Owner {
    pub store: Arc<NativeStore>, pub external: Arc<External>, pub removal: Arc<removal::Controller>, pub drafts: Arc<Drafts>,
    pub install: Arc<dyn super::pipeline::Owners>, pub archived_record: Arc<dyn Fn(SessionParams, &str, i32, u32) -> Result<Record, Exception> + Send + Sync>,
    pub prepare_archived_session: Arc<dyn Fn(&super::Session, &Record, &ArchivedPackage) -> Result<String, Exception> + Send + Sync>,
}
fn visible<'a>(query: &Query<'a>, name: &str, user: i32) -> Result<&'a crate::package::model::PackageState, Exception> {
    let package = query.state.packages.get(name);
    if query.filtered_including_uninstalled(package, user).map_err(super::policy::unknown)? { return Err(not_found(format!("Package {name} not found."))) }
    package.ok_or_else(|| not_found(format!("Package {name} not found.")))
}
fn caller(query: &Query<'_>, name: &str, user: i32) -> Result<(), Exception> {
    if matches!(query.calling_uid, 0 | 1000 | 2000) { return Ok(()) }
    let uid = query.package_uid_internal(name, 0, user, 1000).map_err(super::policy::unknown)?;
    if uid != query.calling_uid { return Err(Exception::security(format!("Calling package {name} does not belong to uid {}", query.calling_uid))) }
    Ok(())
}
impl Owner {
    fn failed_install(&self, receiver: IntentSender, id: i32, name: &str, status: i32, message: &str) -> Result<(), Exception> {
        self.drafts.operations.abandon_session(id, 1000)?;
        (self.drafts.save)()?;
        self.external.install_status(receiver, id, name, status, Some(message))
    }
    pub fn request_archive(&self, query: &Query<'_>, package: &str, caller_package: &str, uid: u32, pid: i32, user: i32, flags: i32, receiver: IntentSender) -> Result<(), Exception> {
        caller(query, caller_package, user)?;
        let users = if flags & removal::ALL_USERS != 0 { self.external.users()? } else { vec![user] };
        if !self.external.permission("android.permission.DELETE_PACKAGES", pid, uid)? && !self.external.permission("android.permission.REQUEST_DELETE_PACKAGES", pid, uid)? {
            return Err(Exception::security("You need DELETE_PACKAGES or REQUEST_DELETE_PACKAGES permission to request an archival."));
        }
        for &user in &users {
            self.removal.cross_user(query, uid, user, "archiveApp")?;
            let state = visible(query, package, user)?;
            if state.is.system || state.is.updated_system_app { return Err(not_found("System apps cannot be archived.")) }
            if state.users.get(&user).is_some_and(|state| !state.installed) { return Err(not_found(format!("{package} is not installed."))) }
            let installer = removal::archive_installer(query.state, package).ok_or_else(|| not_found("No installer found"))?;
            self.removal.verify_unarchive_receiver(query.state, &installer, user, uid == 2000)?;
            let app_uid = apps_filter::uid(user, state.app_id);
            if self.external.opted_out(package, app_uid)? { return Err(not_found(format!("The app {package} is opted out of archiving."))) }
            let metadata = self.external.collect_archive(package, &installer, user)?;
            let capture = self.store.snapshots.capture();
            let mut state = capture.owner().scanned_user_states(package).and_then(|users| users.get(&user)).cloned().unwrap_or_default();
            state.archive_state = Some(metadata);
            let before=capture.version();
            let result=self.store.user_state(package, user, state);
            self.store.finish_after_unlock(before,result)?;
        }
        self.removal.uninstall(removal::Request { package: package.into(), version: -1, caller_package: Some(caller_package.into()), uid, pid, user,
            flags: removal::KEEP_DATA | removal::ARCHIVE | (flags & removal::ALL_USERS), existing_only: false }, Some(receiver))
    }
    pub fn request_unarchive(&self, query: &Query<'_>, package: &str, caller_package: &str, uid: u32, pid: i32, user: i32, receiver: IntentSender, show_confirmation: bool) -> Result<(), Exception> {
        caller(query, caller_package, user)?; self.removal.cross_user(query, uid, user, "unarchiveApp")?;
        let state = visible(query, package, user)?; let requesting = visible(query, caller_package, user)?;
        let user_state = state.users.get(&user).ok_or_else(|| not_found(format!("Package {package} is not currently archived.")))?;
        let archive = user_state.archive_state.as_ref().filter(|_| !user_state.installed).ok_or_else(|| not_found(format!("Package {package} is not currently archived.")))?;
        let installer = removal::archive_installer(query.state, package).ok_or_else(|| not_found(format!("No installer found to unarchive app {package}.")))?;
        let privileged = self.external.permission("android.permission.INSTALL_PACKAGES", pid, uid)?;
        let requested = requesting.pkg.as_ref().is_some_and(|code| code.requested_permissions.iter().any(|name| name == "android.permission.REQUEST_INSTALL_PACKAGES"));
        if !privileged && !requested { return Err(Exception::security("You need INSTALL_PACKAGES or REQUEST_INSTALL_PACKAGES permission to request an unarchival.")) }
        if !privileged || show_confirmation { return self.external.confirmation(receiver, package, user) }
        let record = (self.drafts.create)(package, &installer, user, caller_package)?;
        let id = self.drafts.attach(record, package, &installer, &archive.installer_title, receiver, &self.external)?;
        self.external.broadcast_unarchive(package, &installer, user, id)
    }
    pub fn install_archived(&self, query: &Query<'_>, uid: u32, pid: i32, archived: ArchivedPackage, mut params: SessionParams, installer: &str, user: i32, receiver: IntentSender) -> Result<(), Exception> {
        self.removal.cross_user(query, uid, user, "installPackageArchived")?;
        if !self.external.permission("android.permission.INSTALL_PACKAGES", pid, uid)? { return Err(Exception::security("You need INSTALL_PACKAGES permission to request archived package install")) }
        if params.data_loader_params.is_some() { return Err(Exception::new(EX_ILLEGAL_ARGUMENT, "Incompatible session param: dataLoaderParams has to be null")) }
        if archived.name.split('.').any(|part| part.is_empty() || !part.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) { return Err(Exception::new(EX_ILLEGAL_ARGUMENT, "Invalid archived package name")) }
        params.install_flags |= ARCHIVED;
        let record = (self.archived_record)(params, installer, user, uid)?;
        let archive_state = self.external.collect_archived(&archived, installer, user)?;
        let id = self.drafts.sessions.create_record(record.clone(), true)?;
        (self.drafts.save)()?;
        let session = self.drafts.sessions.snapshot(id)?;
        let stage = (self.prepare_archived_session)(&session, &record, &archived)?;
        self.drafts.sessions.prepared(id)?;
        self.drafts.sessions.open(id, 1000)?;
        self.drafts.operations.seal_session(id, 1000)?;
        (self.drafts.save)()?;
        let session = self.drafts.sessions.snapshot(id)?;
        let code = super::pipeline::VerifiedCode { session, record, package: archived.package(&stage), signing: archived.collected_signing()? };
        let prepared = match self.install.prepare(vec![code]) {
            Ok(prepared) => prepared,
            Err(failure) => return self.failed_install(receiver, id, &archived.name, failure.legacy_status, &failure.message),
        };
        let mut receipt = match prepared.commit() {
            Ok(receipt) => receipt,
            Err(failure) => return self.failed_install(receiver, id, &archived.name, failure.legacy_status, &format!("committed={}: {}", failure.committed, failure.message)),
        };
        if let Err(failure) = self.install.confirm_publication(&receipt) {
            return self.failed_install(receiver, id, &archived.name, failure.legacy_status, &failure.message);
        }
        let capture = self.store.snapshots.capture(); let mut state = capture.owner().scanned_user_states(&archived.name).and_then(|users| users.get(&user)).cloned().ok_or_else(|| illegal("Archived install user state not published"))?;
        state.installed = false; state.archive_state = Some(archive_state);
        let before=capture.version();
        let result=self.store.user_state(&archived.name, user, state);
        let next=self.store.finish_after_unlock(before,result)?; receipt.generation = next.version();
        self.install.finish(receipt)?;
        self.external.install_status(receiver, id, &archived.name, 1, None)?;
        self.drafts.sessions.close(id, 1000)?; (self.drafts.save)()?; Ok(())
    }
    pub fn report(&self, uid: u32, id: i32, user: i32, status: i32, required: i64, action: Option<IntentSender>) -> Result<(), Exception> {
        self.drafts.report(uid, id, user, status, required, action, &self.external)
    }
}
