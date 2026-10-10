//! Existing-code move adapter for the concrete native installation pipeline.
//! AOSP android-16.0.0_r1 InstallingSession/FileInstallArgs, Apache-2.0.
use super::{codec::SessionParams, pipeline::{self, Owners, PreparedInstall}, Parameters, Record, Session};
use crate::package::{installer::native::QuerySource, move_package::{self, Failure, InstallOwner, Plan, PreparedMove, Receipt}, sign::SigningDetails};
use std::{collections::BTreeSet, fs::{self, File, OpenOptions}, io, os::unix::{fs::MetadataExt, fs::symlink}, path::{Path, PathBuf}, sync::{Arc, Mutex}};

pub struct Adapter {
    native: Arc<pipeline::Native>, source: QuerySource, claims: PathBuf,
    errors: Arc<Mutex<Vec<String>>>,
}
impl Adapter {
    /// `claims` is the native coordinator's exclusively owned metadata directory,
    /// outside code paths. It must never name the original image or user data.
    pub fn new(native: Arc<pipeline::Native>, source: QuerySource, claims: PathBuf) -> Arc<Self> {
        Arc::new(Self { native, source, claims, errors: Arc::new(Mutex::new(vec![])) })
    }
    pub fn diagnostics(&self) -> Vec<String> { self.errors.lock().unwrap().clone() }
}
fn error(message: impl Into<String>) -> Failure { Failure { status: move_package::INTERNAL, message: message.into(), committed: false } }
fn io_error(error: io::Error) -> Failure {
    Failure { status: if error.raw_os_error() == Some(libc::ENOSPC) { move_package::STORAGE } else { move_package::INTERNAL }, message: error.to_string(), committed: false }
}
fn installed_path(plan: &Plan) -> Result<String, Failure> {
    let root = match plan.destination_volume.as_deref() {
        None => "/data/app".to_owned(),
        Some(uuid) if plan.complete && !uuid.is_empty() && !uuid.contains(['/', '\\', '\0']) => format!("/mnt/expand/{uuid}/app"),
        Some("primary_physical") => return Err(error("Physical destination is selected by the native install reservation")),
        _ => return Err(error("Invalid private move destination")),
    };
    let code = Path::new(&plan.code_path); let parent = code.parent().ok_or_else(|| error("Move source code has no parent"))?;
    let name = code.file_name().and_then(|name| name.to_str()).ok_or_else(|| error("Move source code name is invalid"))?;
    let parent_name = parent.file_name().and_then(|name| name.to_str()).unwrap_or("");
    Ok(if parent_name.starts_with("~~") { format!("{root}/{parent_name}/{name}") } else { format!("{root}/{name}") })
}
struct Claim {
    path: PathBuf, _file: File, code: Option<(PathBuf, u64, u64)>, errors: Arc<Mutex<Vec<String>>>,
}
impl Claim {
    fn capture(&mut self, code: &Path) -> Result<(), Failure> {
        let metadata = fs::symlink_metadata(code).map_err(io_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err(error("Move output is not an owned code directory")) }
        self.code = Some((code.into(), metadata.dev(), metadata.ino())); Ok(())
    }
    fn cleanup_code(&mut self) -> Result<(), Failure> {
        if let Some((path, dev, ino)) = self.code.take() {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.dev() == dev && metadata.ino() == ino && metadata.is_dir() && !metadata.file_type().is_symlink() => fs::remove_dir_all(&path).map_err(io_error)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                _ => return Err(error("Move code ownership changed before cleanup")),
            }
        }
        Ok(())
    }
}
impl Drop for Claim {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup_code() { self.errors.lock().unwrap().push(error.message); }
        if let Err(error) = fs::remove_file(&self.path) { self.errors.lock().unwrap().push(format!("Move claim cleanup: {error}")); }
    }
}
struct Prepared {
    native: Arc<pipeline::Native>, source: QuerySource, plan: Plan, claim: Claim,
    source_host: PathBuf, source_identity: (u64, u64), signing: SigningDetails,
    target_guest: Option<String>, target_host: Option<PathBuf>, copied: bool,
}
impl InstallOwner for Adapter {
    fn prepare(&self, plan: &Plan) -> Result<Box<dyn PreparedMove>, Failure> {
        let base = self.native.snapshots.capture();
        let setting = base.owner().settings.packages.iter().find(|setting| setting.name == plan.package).ok_or_else(|| error("Move base setting missing"))?;
        if setting.version_code != plan.version || setting.code_path != plan.code_path || setting.volume_uuid != plan.source_volume || setting.app_id != plan.app_id {
            return Err(error("Move base identity changed"));
        }
        let source_host = (self.native.apks.files)(&plan.code_path).ok_or_else(|| error("Move source VFS owner missing"))?;
        let metadata = fs::symlink_metadata(&source_host).map_err(io_error)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err(error("Move source is not a cluster directory")) }
        let parsed = self.native.apks.parsed_path(&plan.code_path, 0).map_err(error)?;
        let signing = self.native.apks.signing_details(&parsed).map_err(error)?;
        let accepted = base.owner().loaded_packages().get(&plan.package).ok_or_else(|| error("Move accepted code missing"))?;
        if signing != accepted.collected_signing || parsed.package_name != accepted.package.package_name
            || (((parsed.version_code_major as i64) << 32) | parsed.version_code as u32 as i64) != plan.version {
            return Err(error("Move source APK identity/signers changed"));
        }
        fs::create_dir_all(&self.claims).map_err(io_error)?;
        let claim_path = self.claims.join(format!("move-{}", plan.move_id));
        let file = OpenOptions::new().create_new(true).read(true).write(true).open(&claim_path).map_err(io_error)?;
        let claim = Claim { path: claim_path, _file: file, code: None, errors: self.errors.clone() };
        let (target_guest, target_host) = if plan.complete {
            let guest = installed_path(plan)?; let host = (self.native.apks.files)(&guest).ok_or_else(|| error("Move destination VFS owner missing"))?;
            if fs::symlink_metadata(&host).is_ok() { return Err(error("Move destination already exists")) }
            (Some(guest), Some(host))
        } else { (None, None) };
        Ok(Box::new(Prepared { native: self.native.clone(), source: self.source.clone(), plan: plan.clone(), claim,
            source_host, source_identity: (metadata.dev(), metadata.ino()), signing, target_guest, target_host, copied: false }))
    }
    fn finish(&self, plan: &Plan, receipt: &Receipt) -> Result<(), Failure> {
        let state = (self.source)().map_err(|error| Failure { status: move_package::INTERNAL, message: error.message, committed: true })?;
        let package = state.packages.get(&plan.package).ok_or_else(|| error("Moved package missing before finish"))?;
        if state.generation != receipt.generation || package.path != receipt.path || package.volume_uuid != receipt.volume {
            return Err(Failure { status: move_package::INTERNAL, message: "Move finish publication changed".into(), committed: true });
        }
        let install = pipeline::PublishedInstall { generation: receipt.generation, packages: plan.users.iter().map(|user| pipeline::InstalledPackage {
            name: plan.package.clone(), version_code: plan.version, user: *user as u32,
        }).collect(), verified_sessions:Vec::new(), new_installations:Default::default() };
        self.native.finish(install).map_err(|error| Failure { status: move_package::INTERNAL, message: error.message, committed: true })
    }
}
impl PreparedMove for Prepared {
    fn copy_code(&mut self) -> Result<(), Failure> {
        if self.plan.complete { return Err(error("Complete move uses the installd relocation owner")) }
        let guest = format!("/data/app/aim-move-{}.tmp", self.plan.move_id);
        let path = (self.native.apks.files)(&guest).ok_or_else(|| error("Move staging VFS view missing"))?;
        fs::create_dir(&path).map_err(io_error)?;
        self.claim.capture(&path)?;
        copy_cluster(&self.source_host, &path).map_err(io_error)?;
        // The guest stage is mapped by the same Files owner as the parser.
        self.target_guest = Some(guest); self.target_host = Some(path); self.copied = true; Ok(())
    }
    fn commit(mut self: Box<Self>) -> Result<Receipt, Failure> {
        let current = fs::symlink_metadata(&self.source_host).map_err(io_error)?;
        if (current.dev(), current.ino()) != self.source_identity { return Err(error("Move source cluster changed")) }
        let target_guest = self.target_guest.as_ref().ok_or_else(|| error("Move code has not been copied"))?;
        let target_host = self.target_host.as_ref().ok_or_else(|| error("Move target absent"))?;
        if !self.plan.complete && !self.copied { return Err(error("Physical move requires completed code copy")) }
        if self.plan.complete { self.claim.capture(target_host)?; }
        let parsed = self.native.apks.parsed_path(target_guest, 0).map_err(error)?;
        let moved_signing = self.native.apks.signing_details(&parsed).map_err(error)?;
        if moved_signing != self.signing || (((parsed.version_code_major as i64) << 32) | parsed.version_code as u32 as i64) != self.plan.version {
            return Err(error("Relocated APK identity/signers differ from accepted source"));
        }
        let params = SessionParams { mode: 1, install_flags: self.plan.install_flags, app_package_name: Some(parsed.package_name.clone()),
            abi_override: self.plan.abi_override.clone(), volume_uuid: self.plan.destination_volume.clone(), required_installed_version_code: self.plan.version,
            installer_package_name: self.plan.install_source.installer.clone(), package_source: 0, ..Default::default() };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| self::error(error.to_string()))?.as_millis();
        let now = i64::try_from(now).map_err(|_| error("Move install clock exceeds original long range"))?;
        // InstallingSession is internal, not a published PackageInstallerSession.
        // Its sealed code witness is the verified immutable move-owned cluster.
        let session = Session { id: self.plan.move_id, installer_uid: 1000, original_installer_uid: 1000,
            committed: true, committed_millis: now, resolved_package: Some(self.plan.package.clone()), validated_target_sdk: Some(parsed.target_sdk_version), checksums: Default::default(),
            user: self.plan.install_user as u32, parameters: Parameters { multi_package: false, staged: false, install_flags: self.plan.install_flags, application_enabled_setting_persistent: false },
            parent: -1, children: BTreeSet::new(), active_count: 1, prepared: true, sealed: true, destroyed: false, client_progress: 0.0, reported_progress: 0.0,
            has_app_metadata: false, pre_verified_domains: None, installation_files: vec![] };
        let record = Record { params, installer_uid: 1000, user: self.plan.install_user as u32,
            installer_package: self.plan.install_source.installer.clone(), installer_attribution_tag: self.plan.install_source.installer_attribution_tag.clone(),
            created_millis: now, initiating_package: self.plan.install_source.initiating_package.clone(), originating_package: self.plan.install_source.originating_package.clone(),
            installer_package_uid: self.plan.install_source.installer_uid };
        let prepared = self.native.prepare(vec![pipeline::VerifiedCode { session, record, package: parsed, signing: moved_signing }])
            .map_err(|error| move_package::install_failure(error.legacy_status, error.committed, error.message))?;
        let published = match prepared.commit() {
            Ok(receipt) => receipt,
            Err(error) => {
                if error.committed { self.claim.code = None; }
                return Err(move_package::install_failure(error.legacy_status, error.committed, error.message));
            }
        };
        // Once global state committed, a later callback/receipt failure must
        // never let a rollback guard delete potentially published code.
        let staged_code = self.claim.code.take();
        self.native.confirm_publication(&published).map_err(|error| move_package::install_failure(error.legacy_status, true, error.message))?;
        let state = (self.source)().map_err(|error| Failure { status: move_package::INTERNAL, message: error.message, committed: true })?;
        let package = state.packages.get(&self.plan.package).ok_or_else(|| error("Move native publication missing"))?;
        if state.generation != published.generation || package.volume_uuid != self.plan.destination_volume || package.version_code != self.plan.version
            || !self.plan.users.iter().all(|user| package.users.get(user).is_none_or(|state| state.installed)) {
            self.claim.code = None;
            return Err(Failure { status: move_package::INTERNAL, message: "Move install publication differs".into(), committed: true });
        }
        let receipt = Receipt { generation: state.generation, package: package.name.clone(), version: package.version_code, path: package.path.clone(), volume: package.volume_uuid.clone() };
        if package.path != *target_guest {
            self.claim.code = staged_code;
            self.claim.cleanup_code().map_err(|mut error| { error.committed = true; error })?;
        }
        Ok(receipt)
    }
}
fn copy_cluster(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?; let kind = entry.file_type()?; let target = to.join(entry.file_name());
        if kind.is_dir() { fs::create_dir(&target)?; copy_cluster(&entry.path(), &target)?; }
        else if kind.is_symlink() { symlink(fs::read_link(entry.path())?, target)?; }
        else if kind.is_file() { fs::copy(entry.path(), target)?; }
        else { return Err(io::Error::new(io::ErrorKind::InvalidData, "Move cluster contains a non-file object")); }
    }
    Ok(())
}
