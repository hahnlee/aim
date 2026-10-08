//! Actual native compressed-system and system-install-state producers.
//! Ported from AOSP android-16.0.0_r1 InstallPackageHelper/PMS, Apache-2.0.
use super::{installer::{self, codec::SessionParams, pipeline::{self, Owners}, existing, removal::NativeStore}, query, scan_snapshot::Snapshot, pkg::booleans2};
use aim_binder_host::{local::{LocalProcess, Strong, Service}, parcel::{Exception, Parcel, BAD_VALUE, EX_ILLEGAL_STATE}};
use aim_service_aidl::{android_content_pm_ipackagemanager as pm, android_content_pm_ipackagedeleteobserver2 as observer, dev_aim_server_ipackageenablebridge as api};
use std::{collections::BTreeSet, fs::{self, File, OpenOptions}, io::{self, Write}, os::unix::fs::{MetadataExt, PermissionsExt}, path::PathBuf, sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}}};

pub struct Bridge { node: Strong }
impl Bridge {
    pub fn new(node: Strong) -> Arc<Self> { Arc::new(Self { node }) }
    pub fn quarantine_enabled(&self) -> Result<bool, Exception> {
        let mut request = Parcel::new(); request.write_interface_token(api::DESCRIPTOR);
        let reply = self.node.transact(api::QUARANTINE_ENABLED, &request, false).map_err(|code| illegal(format!("Quarantine flag owner: {code}")))?;
        let mut reader = reply.reader(); reader.read_exception().map_err(|code| illegal(format!("Quarantine flag reply: {code}")))??;
        let enabled = reader.read_bool().map_err(|code| illegal(format!("Quarantine flag value: {code}")))?;
        if reader.remaining() != 0 { return Err(illegal("Quarantine flag owner sent trailing bytes")) } Ok(enabled)
    }
    fn clear_code_cache(&self, name: &str, users: &[i32]) -> Result<(), Exception> {
        let mut request = Parcel::new(); request.write_interface_token(api::DESCRIPTOR);
        request.write_string16(Some(name)); aim_service_aidl::write_int_array(&mut request, Some(users));
        let reply = self.node.transact(api::CLEAR_CODE_CACHE, &request, false).map_err(|code| illegal(format!("Compressed code cache owner: {code}")))?;
        let mut reader = reply.reader(); reader.read_exception().map_err(|code| illegal(format!("Compressed cache reply: {code}")))??;
        if reader.remaining() != 0 { return Err(illegal("Compressed cache reply has trailing bytes")) } Ok(())
    }
}
pub struct Owner {
    pub native: Arc<pipeline::Native>, pub store: Arc<NativeStore>, pub source: installer::native::QuerySource,
    pub existing: Arc<existing::Owner>, pub deletion: Arc<query::removal::Owner>,
    pub effects: Arc<super::effects::Owner>, pub lifecycle: Arc<super::lifecycle::Owner>,
    pub bridge: Arc<Bridge>, pub process: Arc<LocalProcess>, pub gate: Arc<Mutex<()>>,
    errors: Arc<Mutex<Vec<String>>>,
}
impl Owner {
    pub fn new(native: Arc<pipeline::Native>, store: Arc<NativeStore>, source: installer::native::QuerySource, existing: Arc<existing::Owner>, deletion: Arc<query::removal::Owner>, effects: Arc<super::effects::Owner>, lifecycle: Arc<super::lifecycle::Owner>, bridge: Arc<Bridge>, process: Arc<LocalProcess>, gate: Arc<Mutex<()>>) -> Arc<Self> {
        Arc::new(Self { native, store, source, existing, deletion, effects, lifecycle, bridge, process, gate, errors: Arc::new(Mutex::new(vec![])) })
    }
    pub fn diagnostics(&self) -> Vec<String> { self.errors.lock().unwrap().clone() }
    pub fn system_install_state(&self, name: &str, installed: bool, user: i32) -> Result<bool, Exception> {
        let state = (self.source)()?;
        let Some(package) = state.packages.get(name).filter(|package| package.is.system && package.pkg.is_some()) else { return Ok(false) };
        if super::info::user_state(package, user).installed == installed { return Ok(false) }
        if installed {
            // The source reports accepted state-change work independently of the
            // installer result; this still invokes the real native existing owner.
            self.existing.install_existing(1000, 0, existing::Request { package: Some(name.into()), user, flags: 0x00400000, reason: 3, allowlisted_permissions: None, receiver: None })?;
        } else {
            let callback = self.process.add_service(Arc::new(DeleteResult { errors: self.errors.clone() }));
            let mut data = Parcel::new(); data.write_interface_token(pm::DESCRIPTOR); data.write_i32(1);
            data.write_string16(Some(name)); data.write_i64(-1); data.write_binder(Some(callback)); data.write_i32(user); data.write_i32(installer::removal::SYSTEM_APP);
            let reply = self.deletion.dispatch(1000, 0, pm::DELETE_PACKAGE_VERSIONED, &mut aim_binder_host::parcel::Reader::new(data.data(), data.objects())).ok_or_else(|| illegal("Native system deletion route unavailable"))?
                .map_err(|code| illegal(format!("System deletion transport: {code}")))?;
            aim_binder_host::parcel::Reader::new(reply.data(), reply.objects()).read_exception().map_err(|code| illegal(format!("System deletion reply: {code}")))??;
        }
        Ok(true)
    }
    pub fn enable_compressed(&self, name: &str, user: i32) -> Result<bool, Exception> {
        let before_version = self.store.snapshots.capture().version();
        let gate = self.gate.lock().unwrap();
        let result = (|| -> Result<bool, Exception> {
        let before = self.native.snapshots.capture();
        let setting = before.owner().settings.packages.iter().find(|package| package.name == name).ok_or_else(|| illegal("Compressed package setting missing"))?;
        let accepted = before.owner().loaded_packages().get(name).ok_or_else(|| illegal("Compressed stub code missing"))?;
        if setting.flags & super::settings::FLAG_SYSTEM == 0 || !accepted.package.is2(booleans2::STUB) { return Err(illegal("Compressed enable requires an accepted system stub")) }
        let freeze = self.lifecycle.freeze(name.into()).map_err(illegal)?;
        self.effects.kill(name, setting.app_id, -1, "setEnabledSetting", 16)?;
        let result = self.expand(name, user, &before);
        match result {
            Ok(()) => { drop(freeze); Ok(true) }
            Err(error) => {
                self.errors.lock().unwrap().push(format!("Compressed enable failure: {error:?}"));
                self.restore_stub(name, &before)?;
                let capture = self.native.snapshots.capture();
                let mut state = capture.owner().scanned_user_states(name).and_then(|users| users.get(&0)).cloned().unwrap_or_default();
                state.enabled = 2; state.last_disable_app_caller = Some("android".into());
                self.store.user_state(name, 0, state)?;
                drop(freeze); Ok(false)
            }
        }
        })();
        drop(gate);
        self.store.finish_after_unlock(before_version, result)
    }
    fn expand(&self, name: &str, user: i32, before: &Arc<Snapshot>) -> Result<(), Exception> {
        let setting = before.owner().settings.packages.iter().find(|package| package.name == name).unwrap();
        let stub_path = std::path::Path::new(&setting.code_path);
        let stub_name = stub_path.file_name().and_then(|name| name.to_str()).ok_or_else(|| illegal("Invalid stub code path"))?;
        let base_name = stub_name.strip_suffix("-Stub").ok_or_else(|| illegal("System stub has no compressed sibling"))?;
        let compressed = stub_path.parent().ok_or_else(|| illegal("Stub code has no parent"))?.join(base_name);
        let compressed = compressed.to_str().ok_or_else(|| illegal("Compressed path is not UTF-8"))?;
        let source = (self.native.apks.files)(compressed).ok_or_else(|| illegal("Compressed inventory VFS owner unavailable"))?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let stage_guest = format!("/data/app/aim-compressed-{}.tmp", NEXT.fetch_add(1, Ordering::Relaxed));
        let stage = (self.native.apks.files)(&stage_guest).ok_or_else(|| illegal("Compressed stage VFS owner unavailable"))?;
        fs::create_dir(&stage).map_err(io_error)?; fs::set_permissions(&stage, fs::Permissions::from_mode(0o755)).map_err(io_error)?;
        let metadata = fs::symlink_metadata(&stage).map_err(io_error)?;
        let mut stage = Stage { path: stage, dev: metadata.dev(), ino: metadata.ino(), keep: false, errors: self.errors.clone() };
        let mut count = 0;
        for entry in fs::read_dir(source).map_err(io_error)? {
            let entry = entry.map_err(io_error)?; let file_name = entry.file_name().into_string().map_err(|_| illegal("Compressed file name is not UTF-8"))?;
            if !file_name.to_ascii_lowercase().ends_with(".gz") { continue }
            let destination = &file_name[..file_name.len() - 3];
            if destination.is_empty() { return Err(illegal("Compressed output name is empty")) }
            let source = File::open(entry.path()).map_err(io_error)?;
            let path = stage.path.join(destination); let temporary = stage.path.join(format!("{destination}.new"));
            let mut output = OpenOptions::new().create_new(true).write(true).open(&temporary).map_err(io_error)?;
            let mut gzip = flate2::read::MultiGzDecoder::new(source); io::copy(&mut gzip, &mut output).map_err(io_error)?;
            output.flush().map_err(io_error)?; output.sync_all().map_err(io_error)?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).map_err(io_error)?;
            fs::rename(&temporary, path).map_err(io_error)?; count += 1;
        }
        if count == 0 { return Err(illegal("No compressed package artifacts")) }
        let package = self.native.apks.parsed_path(&stage_guest, 0).map_err(illegal)?;
        if package.package_name != name || package.is2(booleans2::STUB) { return Err(illegal("Expanded code is not the expected runnable package")) }
        let signing = self.native.apks.signing_details(&package).map_err(illegal)?;
        let mut scan = before.owner().clone(); scan.disable_system_package(name).map_err(|error| illegal(format!("Compressed factory retention: {error:?}")))?;
        let publication = self.native.snapshots.publish_after(before, scan, before.usage().clone(), |snapshot| self.native.disk.lock().unwrap().commit_scan_settings(snapshot));
        let retained = self.publish(before, publication)?;
        let flags = 0x2 | 0x10;
        let now = clock()?;
        let session = internal_session(user, flags, package.target_sdk_version, now)?;
        let record = installer::Record { params: SessionParams { mode: 1, install_flags: flags, app_package_name: Some(name.into()), required_installed_version_code: setting.version_code,
            application_enabled_setting_persistent: true, ..Default::default() }, installer_uid: 1000, user: user as u32, installer_package: Some("android".into()), installer_attribution_tag: None,
            created_millis: now, initiating_package: None, originating_package: None, installer_package_uid: 1000 };
        let prepared = self.native.prepare(vec![pipeline::VerifiedCode { session, record, package, signing }]).map_err(|error| illegal(error.message))?;
        let receipt = match prepared.commit() {
            Ok(receipt) => receipt,
            Err(error) => { if error.committed { stage.keep = true; } return Err(illegal(format!("Compressed publication committed={}: {}", error.committed, error.message))); }
        };
        stage.keep = true;
        let state = (self.source)()?; let active = state.packages.get(name).ok_or_else(|| illegal("Expanded package not published"))?;
        if state.generation != receipt.generation || state.generation <= retained.version() || active.pkg.as_ref().is_none_or(|code| code.is2(booleans2::STUB)) { stage.keep = true; return Err(illegal("Expanded publication does not replace the stub")) }
        if active.path != stage_guest { stage.keep = false; }
        self.native.confirm_publication(&receipt).map_err(|error| illegal(error.message))?;
        self.native.finish(receipt)?;
        let users: Vec<_> = active.users.keys().copied().collect(); self.bridge.clear_code_cache(name, &users)
    }
    fn publish(&self, base: &Arc<Snapshot>, result: Result<Arc<Snapshot>, super::scan_snapshot::CommitError>) -> Result<Arc<Snapshot>, Exception> {
        match result {
            Ok(next) => { (self.store.publish)(base, &next)?; Ok(next) },
            Err(super::scan_snapshot::CommitError::Disk { snapshot: Some(next), error }) => { (self.store.publish)(base, &next)?; Err(illegal(format!("Compressed metadata committed: {}", error.message))) },
            Err(error) => Err(illegal(format!("Compressed metadata publication: {error:?}"))),
        }
    }
    fn restore_stub(&self, name: &str, before: &Arc<Snapshot>) -> Result<(), Exception> {
        let current = self.native.snapshots.capture();
        let restored = before.owner().clone(); let users: Vec<_> = restored.scanned_user_states(name).into_iter().flat_map(|users| users.keys()).copied().collect();
        let result = self.native.snapshots.publish_after(&current, restored, before.usage().clone(), |snapshot| {
            let mut disk = self.native.disk.lock().unwrap(); disk.commit_scan_settings(snapshot)?;
            for user in &users { disk.commit_live_install_restrictions(snapshot.owner(), *user as u32, &BTreeSet::from([name.to_owned()]), super::restrictions::PINNED_CROSS_USER_SUSPENSIONS)?; }
            Ok(())
        });
        self.publish(&current, result)?; Ok(())
    }
}
struct DeleteResult { errors: Arc<Mutex<Vec<String>>> }
impl Service for DeleteResult {
    fn descriptor(&self) -> &str { observer::DESCRIPTOR }
    fn transact(&self, call: &mut aim_binder_host::local::Call<'_>) -> aim_binder_host::local::Reply {
        if call.code != observer::ON_PACKAGE_DELETED { return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION) }
        let args = observer::OnPackageDeleted::read(&mut call.data)?; if call.data.remaining() != 0 { return Err(BAD_VALUE) }
        if args.return_code != 1 { self.errors.lock().unwrap().push(format!("System package deletion result {}: {:?}", args.return_code, args.msg)); }
        Ok(Parcel::new())
    }
}
struct Stage { path: PathBuf, dev: u64, ino: u64, keep: bool, errors: Arc<Mutex<Vec<String>>> }
impl Drop for Stage {
    fn drop(&mut self) {
        if self.keep { return }
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.dev() == self.dev && metadata.ino() == self.ino && metadata.is_dir() && !metadata.file_type().is_symlink() => {
                if let Err(error) = fs::remove_dir_all(&self.path) { self.errors.lock().unwrap().push(format!("Compressed stage cleanup: {error}")); }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            _ => self.errors.lock().unwrap().push("Compressed stage ownership changed".into()),
        }
    }
}
fn internal_session(user: i32, flags: i32, sdk: i32, now: i64) -> Result<installer::Session, Exception> {
    let user = u32::try_from(user).map_err(|_| illegal("Negative compressed install user"))?;
    Ok(installer::Session { id: -1, installer_uid: 1000, original_installer_uid: 1000, committed: true, committed_millis: now, resolved_package: None, validated_target_sdk: Some(sdk), checksums: Default::default(),
        user, parameters: installer::Parameters { multi_package: false, staged: false, install_flags: flags, application_enabled_setting_persistent: true }, parent: -1, children: BTreeSet::new(), active_count: 1,
        prepared: true, sealed: true, destroyed: false, client_progress: 0.0, reported_progress: 0.0, has_app_metadata: false, pre_verified_domains: None, installation_files: vec![] })
}
fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
fn io_error(error: io::Error) -> Exception { illegal(format!("Compressed package I/O: {error}")) }

fn clock() -> Result<i64, Exception> {
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| illegal(error.to_string()))?;
    i64::try_from(time.as_millis()).map_err(|_| illegal("Compressed install clock exceeds original long range"))
}
