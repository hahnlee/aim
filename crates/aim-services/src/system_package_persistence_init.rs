//! Ordered native data ownership, Settings/runtime/session recovery and binding.
//! The native constructor calls this after stopping the original PMS writer.
use crate::{system::System, package::{self, bootstrap::Bridge, owner::{self, recovery::{ReadStage, ReadError}}, settings::{Settings, PackageReadAttempt, ReadOwners}, installer}};
use aim_binder_host::{local::{LocalProcess, Strong}, parcel::{Exception, Parcel, EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_iinstallerrecoverypresentation as presentation;
use aim_storage::guest_inode::GuestInode;
use std::{collections::BTreeMap, fs::{self, File, OpenOptions}, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::PathBuf, sync::{Arc, Mutex}};

#[derive(Clone)]
pub struct Inputs {
    pub data: PathBuf, pub original_roots: Vec<PathBuf>, pub original_writer_stopped: bool,
    pub users: Vec<u32>, pub session_inode: GuestInode, pub stage_inode: GuestInode,
    pub runtime_inodes: BTreeMap<u32, GuestInode>, pub controller_version: i64,
}
pub struct Prepared {
    pub disk: Arc<Mutex<owner::Store>>, pub settings: Settings,
    pub retained_versions: Vec<package::settings::Version>,
    pub ids: owner::app_ids::AppIds, pub read_attempt: PackageReadAttempt,
    pub report: owner::recovery::Report, data: PathBuf, lease: DataLease,
}
pub struct Installed {
    pub disk: Arc<Mutex<owner::Store>>, pub installer: Arc<installer::native::NativeOwners>,
    pub runtime_metadata: Arc<Mutex<owner::runtime_metadata::State>>,
    pub installer_workers: crate::system::InstallerWorkers,
    pub runtime_worker: owner::runtime_metadata::worker::Worker,
    pub retained_versions: Vec<package::settings::Version>, lease: DataLease,
}
struct DataLease { path: PathBuf, _file: File, identity: (u64, u64) }
impl Drop for DataLease {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.identity) {
            if let Err(error) = fs::remove_file(&self.path) { eprintln!("native package data lease cleanup: {error}"); }
        }
    }
}
pub type ReadCompletion = Box<dyn FnMut(&mut Settings, &mut owner::app_ids::AppIds, &mut PackageReadAttempt) -> Result<(), ReadError>>;
pub fn prepare(system: &Arc<System>, bridge: &Arc<Bridge>, inputs: &Inputs, config: &package::system_config::SystemConfig,
    readers: &mut impl ReadOwners, mut complete: ReadCompletion) -> Result<Prepared, Exception> {
    prepare_native(system,bridge,inputs,config,readers,complete,None)
}
fn prepare_native(system:&Arc<System>,bridge:&Arc<Bridge>,inputs:&Inputs,config:&package::system_config::SystemConfig,
    readers:&mut impl ReadOwners,mut complete:ReadCompletion,
    native_readers:Option<package::settings::native_read::SharedReaders>)->Result<Prepared,Exception>{
    if !inputs.original_writer_stopped { return Err(illegal("Native persistence requires original PMS writer stopped")) }
    let data = inputs.data.canonicalize().map_err(io_error)?;
    if !data.is_dir() { return Err(illegal("Native data owner directory is absent")) }
    for original in &inputs.original_roots {
        let original = original.canonicalize().map_err(io_error)?;
        if data.starts_with(&original) || original.starts_with(&data) { return Err(illegal("Native writable data overlaps original image inputs")) }
    }
    for inode in [&inputs.session_inode, &inputs.stage_inode] {
        if inode.uid.is_none() || inode.gid.is_none() || inode.mode.is_none() { return Err(illegal("Original installer inode creation metadata is incomplete")) }
    }
    if inputs.users.iter().any(|user| !inputs.runtime_inodes.contains_key(user)) { return Err(illegal("Runtime permission inode owner missing user")) }
    fs::create_dir_all(data.join("system")).map_err(io_error)?;
    let path = data.join("system/.aim-native-package-writer");
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .custom_flags(libc::O_EXLOCK | libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(&path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || fs::symlink_metadata(&path).map_err(io_error)?.ino() != metadata.ino() {
        return Err(illegal("Native package writer lease identity changed"));
    }
    let lease = DataLease { path, _file: file, identity: (metadata.dev(), metadata.ino()) };
    let mut settings = Settings::default(); let mut ids = owner::app_ids::AppIds::default();
    system.initialize_package_shared_users(bridge, config, &mut settings, &mut ids, readers)?;
    let mut attempt = PackageReadAttempt::default(); let mut retained_versions = Vec::new();
    let strict = bridge.domain_uuid_strict_validation().map_err(|error| illegal(format!("Settings UUID policy: {error:?}")))?;
    let current=bridge.current_package_version().map_err(|error|illegal(format!("Settings build owner: {error:?}")))?;
    system.check_package_bootstrap(bridge)?;
    let plan=owner::recovery::Plan::inspect(&data).map_err(|error|illegal(error.to_string()))?;
    let native_completion=native_readers.clone();
    let plan=if let Some(readers)=native_readers{plan.with_native_readers(readers)}else{plan};
    let (store, report) = plan.recover_boot_frontend(&inputs.users, &mut settings,&current, |stage, settings| {
        system.check_package_bootstrap(bridge).map_err(|error|ReadError::Owner(format!("Settings bootstrap owner: {error:?}")))?;
        match stage {
            ReadStage::File(bytes) => {
                let result = settings.read_owned_document(bytes, &mut ids, &mut attempt, strict, readers);
                // Retain the restored image identity before readLPw forces the
                // current build. Saved/factory scan policy consumes this copy.
                if result.is_ok() { retained_versions = settings.versions.clone(); }
                result
            }
            ReadStage::Complete => { complete(settings, &mut ids, &mut attempt)?; Ok(None) },
        }
    }).map_err(|error| illegal(format!("Exclusive native Settings recovery: {error}")))?;
    if report.first_boot {
        if let Some(readers)=native_completion {readers.complete(&mut settings,&mut ids,&mut attempt).map_err(|error|illegal(error.to_string()))?;}
    }
    Ok(Prepared { disk: Arc::new(Mutex::new(store)), settings, retained_versions, ids, read_attempt: attempt, report, data, lease })
}

pub struct OwnedPrepared {
    pub prepared:Prepared,
    pub readers:package::settings::native_read::SharedReaders,
}
/// Production constructor: no externally supplied Settings parser or completion.
pub fn construct_package_persistence_owned(system:&Arc<System>,bridge:&Arc<Bridge>,inputs:&Inputs,
    config:&package::system_config::SystemConfig)->Result<OwnedPrepared,Exception>{
    let users=inputs.users.iter().map(|user|i32::try_from(*user).map_err(|_|illegal("Settings user outside Android range"))).collect::<Result<Vec<_>,_>>()?;
    let mut readers=package::settings::native_read::SharedReaders::new(users).map_err(|error|illegal(error.to_string()))?;
    let completion=readers.clone();
    let complete:ReadCompletion=Box::new(move|settings,ids,attempt|completion.complete(settings,ids,attempt));
    let retained=readers.clone();
    let prepared=prepare_native(system,bridge,inputs,config,&mut readers,complete,Some(retained))?;
    Ok(OwnedPrepared{prepared,readers})
}

pub struct IconOwner { node: Strong }
impl IconOwner {
    pub fn new(node: Strong) -> Arc<Self> { Arc::new(Self { node }) }
    fn decode(&self, png: &[u8]) -> Result<Option<installer::codec::Object>, Exception> {
        let mut request = Parcel::new(); request.write_interface_token(presentation::DESCRIPTOR);
        aim_service_aidl::write_byte_array(&mut request, Some(png));
        let reply = self.node.transact(presentation::DECODE_ICON, &request, false).map_err(|code| illegal(format!("Installer icon owner: {code}")))?;
        let mut reader = reply.reader(); reader.read_exception().map_err(|code| illegal(format!("Installer icon reply: {code}")))??;
        let bytes = aim_service_aidl::read_byte_array(&mut reader).map_err(|code| illegal(format!("Installer icon bytes: {code}")))?;
        if reader.remaining() != 0 { return Err(illegal("Installer icon owner sent trailing bytes")) }
        bytes.map(|bytes| installer::codec::recovered_bitmap(&bytes).map_err(|code| illegal(format!("Original Bitmap parcel: {code}")))).transpose().map(Option::flatten)
    }
}

/// Complete scan/runtime restoration before installing persistence, then recover
/// installer sessions and their original Bitmap presentation before publication.
pub fn install(system: &Arc<System>, bridge: &Arc<Bridge>, prepared: Prepared, inputs: &Inputs,
    capture: Arc<package::scan_snapshot::query_state::Capture>, runtime: owner::runtime_metadata::State,
    config: package::system_config::SystemConfig, policy: installer::native::PolicySource,
    publisher: installer::native::Publisher, callback: installer::native::Callback,
    process: Arc<LocalProcess>, icons: Arc<IconOwner>, apks: Arc<package::write::Apks>,
    properties: crate::system_package_installer_init::Properties, dependency_installer_enabled: bool) -> Result<Installed, Exception> {
    let Prepared { disk, retained_versions, data, lease, .. } = prepared;
    let mut runtime = runtime;
    system.set_runtime_permission_controller_version(bridge, &mut runtime, inputs.controller_version)?;
    // RuntimePermissionsPersistence moved with native Settings. Claim every
    // restored user's original AtomicFile inputs while Prepared still owns the
    // exclusive writer lease, before publishing metadata or starting its timer.
    if !inputs.original_writer_stopped{return Err(illegal("Runtime permission handoff requires original Settings writer stopped"));}
    system.check_package_bootstrap(bridge)?;
    disk.lock().unwrap().claim_runtime_permission_inventory(&inputs.users)
        .map_err(|error|illegal(format!("Runtime permission file handoff: {}",error.message)))?;
    system.check_package_bootstrap(bridge)?;
    // Settings.writeLPr writes daemon metadata and queues every runtime user.
    // No permission/user Binder callback runs while the disk mutex is held.
    let users = system.native_boot_persistence_users(bridge)?;
    let configured_users = inputs.users.iter().map(|user|*user as i32).collect::<std::collections::BTreeSet<_>>();
    if users.all.iter().copied().collect::<std::collections::BTreeSet<_>>() != configured_users {
        return Err(illegal("Boot runtime persistence user handoff changed"));
    }
    let entries = system.prepare_package_list_from_scan(bridge, &capture, &users.active)
        .map_err(|error|illegal(format!("Boot package-list rows committed={}: {}",error.committed,error.message)))?;
    {
        let _install = system.package_install_guard();
        users.revalidate(system, bridge)?;
        let mut store = disk.lock().unwrap();
        store.validate_committed_scan(capture.scan().owner())
            .and_then(|()|store.commit_package_list(&entries))
            .map_err(|error|illegal(format!("Boot packages.list committed={}: {}",error.committed,error.message)))?;
        system.check_package_bootstrap(bridge).map_err(|error|illegal(format!("Boot packages.list committed=true: {}",error.message)))?;
    }
    runtime.request_all_user_writes(&users.all).map_err(illegal)?;
    system.install_package_persistence(bridge, &capture, disk.clone())?;
    let runtime_metadata = Arc::new(Mutex::new(runtime));
    system.install_runtime_permission_metadata(bridge, runtime_metadata.clone())?;
    let session_disk = installer::storage::Store::open(data, inputs.session_inode, inputs.stage_inode,
        system.package_installer_labeler(bridge)?).map_err(|error| illegal(format!("Installer session storage: {error}")))?;
    let sessions = Arc::new(installer::Sessions::default());
    let weak_system = Arc::downgrade(system); let query_bridge = bridge.clone();
    let source: installer::native::QuerySource = Arc::new(move || {
        let system = weak_system.upgrade().ok_or_else(|| illegal("Native installer system stopped"))?;
        let current = system.package_bootstrap()?;
        if !Arc::ptr_eq(&current, &query_bridge) { return Err(illegal("Native installer bootstrap changed")) }
        Ok(system.capture_package_queries()?.state().clone())
    });
    let policy = system.package_installer_policy_source(bridge, policy)?;
    let installer = installer::native::NativeOwners::open(sessions, source, policy, config, session_disk, publisher, callback, process)?;
    let decoder = icons.clone(); installer.restore_icons(Arc::new(move |bytes| decoder.decode(bytes)))?;
    crate::system_package_installer_init::configure_existing(system, bridge, &installer, apks, properties, dependency_installer_enabled)?;
    let installer_workers = system.install_package_installer(bridge, &capture, installer.clone())?;
    let runtime_worker = system.start_runtime_permission_worker(bridge, disk.clone(), inputs.runtime_inodes.clone())?;
    Ok(Installed { disk, installer, runtime_metadata, installer_workers, runtime_worker, retained_versions, lease })
}
/// Runtime metadata is restored against the actual native scan before the root
/// constructor publishes its query capture. Existing files are never reseeded.
pub fn restore_runtime(system: &Arc<System>, bridge: &Arc<Bridge>, prepared: &Prepared,
    scan: &mut package::scan::SigningScan, config: &package::system_config::SystemConfig) -> Result<owner::runtime_metadata::State, Exception> {
    system.restore_package_runtime_permissions(bridge, &prepared.disk.lock().unwrap(), scan, config)
}
/// One constructor entry: recovery is complete before the native scan builder
/// sees input. The builder performs scan/factory policy and calls restore_runtime
/// before publishing; installation then binds that exact published capture.
pub fn initialize(system: &Arc<System>, bridge: &Arc<Bridge>, inputs: &Inputs,
    config: package::system_config::SystemConfig, readers: &mut impl ReadOwners, complete: ReadCompletion,
    build: impl FnOnce(&Prepared) -> Result<(Arc<package::scan_snapshot::query_state::Capture>, owner::runtime_metadata::State), Exception>,
    policy: installer::native::PolicySource, publisher: installer::native::Publisher, callback: installer::native::Callback,
    process: Arc<LocalProcess>, icons: Arc<IconOwner>, apks: Arc<package::write::Apks>,
    properties: crate::system_package_installer_init::Properties, dependency_installer_enabled: bool) -> Result<Installed, Exception> {
    let prepared = prepare(system, bridge, inputs, &config, readers, complete)?;
    let (capture, runtime) = build(&prepared)?;
    install(system, bridge, prepared, inputs, capture, runtime, config, policy, publisher, callback,
        process, icons, apks, properties, dependency_installer_enabled)
}
fn illegal(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
fn io_error(error: std::io::Error) -> Exception { illegal(format!("Native data ownership: {error}")) }

#[cfg(test)]
pub(crate) fn epoch_fixture_prepared(data:&std::path::Path)->Result<Prepared,Exception>{
    let data=data.canonicalize().map_err(io_error)?;
    fs::create_dir_all(data.join("system")).map_err(io_error)?;
    let path=data.join("system/.aim-native-package-writer");
    let file=OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .custom_flags(libc::O_EXLOCK|libc::O_NONBLOCK|libc::O_NOFOLLOW).open(&path).map_err(io_error)?;
    let metadata=file.metadata().map_err(io_error)?;
    let lease=DataLease{path,_file:file,identity:(metadata.dev(),metadata.ino())};
    let disk=owner::Store::create(&data,&[0]).map_err(illegal)?;
    Ok(Prepared{disk:Arc::new(Mutex::new(disk)),settings:Settings::default(),retained_versions:vec![],
        ids:owner::app_ids::AppIds::default(),read_attempt:PackageReadAttempt::default(),
        report:owner::recovery::Report{first_boot:true,events:vec![]},data,lease})
}
