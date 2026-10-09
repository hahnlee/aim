//! Production code reservation and native installation environment (#986/#1114).
//! Files are copied into exclusively reserved destinations; the session's sealed
//! source and published old code remain intact until durable publication.
use super::pipeline::{self, Failure, PublishedInstall, VerifiedCode};
use crate::package::{
    owner::seinfo,
    scan,
    scan_snapshot::{Snapshot, Store},
    settings,
    write::Apks,
};
use aim_binder_host::parcel::Exception;
use aim_storage::guest_inode::{self, GuestInode};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub struct AppDataResult {
    pub ce_inode: i64,
    pub de_inode: i64,
    pub newly_created: bool,
}
pub type AppDataCreate =
    Arc<dyn Fn(crate::package::users::AppData) -> Result<AppDataResult, Exception> + Send + Sync>;
pub type AppDataFlags = Arc<dyn Fn(i32) -> Result<i32, Exception> + Send + Sync>;
pub type AppDataRollback = Arc<dyn Fn(&str, i32, i64) -> Result<(), Exception> + Send + Sync>;
pub type PostInstallUsers = Arc<dyn Fn() -> Result<Vec<i32>, Exception> + Send + Sync>;
pub type CodeCacheClear = Arc<dyn Fn(&str, &[i32]) -> Result<(), Exception> + Send + Sync>;
pub type AppDataCommit = Arc<dyn Fn(&str, i32, i64) -> Result<(), Exception> + Send + Sync>;
pub type RuntimePrepare =
    Arc<dyn Fn(&mut scan::live_install::CompletedAdmission, &Arc<Snapshot>) -> Result<(), Exception> + Send + Sync>;
pub type ReservationRelease = Arc<dyn Fn(&str, bool) -> Result<(), Exception> + Send + Sync>;
pub type QueryPublication = Arc<dyn Fn(&Arc<Snapshot>) -> Result<(), String> + Send + Sync>;
pub type Completion = Arc<dyn Fn(PublishedInstall) -> Result<(), Exception> + Send + Sync>;
pub type Labeler = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;
pub type Metadata = Arc<
    dyn Fn(&VerifiedCode, &str, &Snapshot) -> Result<scan::SettingMetadata, String> + Send + Sync,
>;
pub type InstallSource = Arc<
    dyn Fn(&VerifiedCode, &Snapshot) -> Result<settings::InstallSource, Exception> + Send + Sync,
>;
pub type LibraryPolicy =
    Arc<dyn Fn(&VerifiedCode) -> Result<scan::NativeLibraryInstallPolicy, String> + Send + Sync>;
pub type ZipClock = Arc<dyn Fn(u32) -> Result<SystemTime, String> + Send + Sync>;
/// All image/kernel/service values are supplied by their actual native owners.
/// None is inferred from host ownership, environment flags or original PMS writes.
pub struct Config {
    pub data: PathBuf,
    pub apks: Arc<Apks>,
    pub snapshots: Arc<Store>,
    pub users: Vec<scan::User>,
    pub build_debuggable: bool,
    pub cross_user_suspensions: bool,
    pub factory_test: bool,
    pub app_data_flags: AppDataFlags,
    pub directory_inode: GuestInode,
    pub file_inode: GuestInode,
    pub abi: scan::AbiPolicy,
    pub library_compatibility: Arc<scan::LibraryCompatibility>,
    pub vendor_sdk: i32,
    pub remove_test_base: Arc<
        dyn Fn(&crate::package::pkg::AndroidPackage, bool) -> Result<Option<bool>, String>
            + Send
            + Sync,
    >,
    pub preferred_abi: String,
    pub app_lib32_dir: String,
    pub seinfo: seinfo::Policy,
    pub compatibility: Arc<dyn scan::SeInfoCompatibility + Send + Sync>,
    pub labeler: Labeler,
    pub metadata: Metadata,
    pub install_source: InstallSource,
    pub library_policy: LibraryPolicy,
    pub zip_clock: ZipClock,
    pub app_data: AppDataCreate,
    pub rollback_app_data: AppDataRollback,
    pub commit_app_data: AppDataCommit,
    pub clear_code_cache: CodeCacheClear,
    pub post_install_users: PostInstallUsers,
    pub permissions: RuntimePrepare,
    pub release_permissions: ReservationRelease,
    pub effects: Arc<crate::package::effects::Owner>,
    pub code_resources: Arc<crate::package::owner::resources::CodeResources>,
    pub publish: QueryPublication,
    pub completion: Completion,
}
pub struct Owner {
    config: Arc<Config>,
    errors: Arc<Mutex<Vec<String>>>,
    pending: Arc<super::post_install::Pending>,
}
impl Owner {
    pub(crate) fn config(&self) -> Arc<Config> {
        self.config.clone()
    }
    pub fn new(config: Config) -> Result<Arc<Self>, String> {
        if config.users.is_empty() || config.users.iter().any(|user| user.id < 0) {
            return Err("Installation users are unresolved".into());
        }
        let data = fs::canonicalize(&config.data).map_err(|error| error.to_string())?;
        if !data.is_dir()
            || config.directory_inode.uid.is_none()
            || config.directory_inode.gid.is_none()
            || config.directory_inode.mode.is_none()
            || config.file_inode.uid.is_none()
            || config.file_inode.gid.is_none()
            || config.file_inode.mode.is_none()
        {
            return Err("Installation writable root/inode owner is incomplete".into());
        }
        Ok(Arc::new(Self {
            config: Arc::new(Config { data, ..config }),
            errors: Arc::new(Mutex::new(Vec::new())),
            pending: Arc::new(super::post_install::Pending::default()),
        }))
    }
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}
struct Member {
    prior_visibility: Vec<i32>,
    code: VerifiedCode,
    host: PathBuf,
    guest: String,
    metadata: scan::SettingMetadata,
    install: scan::NativeLibraryInstallPolicy,
}
struct Reserved {
    config: Arc<Config>,
    members: Vec<Member>,
    base: Arc<Snapshot>,
    created: Mutex<Vec<(String, i32, i64)>>,
    committed: AtomicBool,
    errors: Arc<Mutex<Vec<String>>>,
    pending: Arc<super::post_install::Pending>,
}
fn failure(status: i32, message: impl ToString) -> Failure {
    Failure {
        legacy_status: status,
        committed: false,
        message: message.to_string(),
    }
}
fn token() -> String {
    let mut bytes = [0u8; 16];
    unsafe {
        libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len());
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn guest_host(data: &Path, guest: &str) -> Result<PathBuf, Failure> {
    let suffix = guest
        .strip_prefix("/data/")
        .ok_or_else(|| failure(-2, "Install source is outside writable data"))?;
    if suffix.split('/').any(|part| matches!(part, "." | "..")) {
        return Err(failure(-2, "Install source has traversal components"));
    }
    Ok(data.join(suffix))
}
fn relocate(path: &str, old: &str, new: &str) -> Result<String, Failure> {
    if path == old {
        return Ok(new.into());
    }
    let suffix = path
        .strip_prefix(old)
        .filter(|suffix| suffix.starts_with('/'))
        .ok_or_else(|| failure(-2, "APK path does not belong to its staging directory"))?;
    Ok(format!("{new}{suffix}"))
}
fn copy_tree(source: &Path, target: &Path, config: &Config) -> Result<(), Failure> {
    for entry in fs::read_dir(source).map_err(|error| failure(-4, error))? {
        let entry = entry.map_err(|error| failure(-4, error))?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| failure(-4, error))?;
        let destination = target.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir(&destination).map_err(|error| failure(-4, error))?;
            record_inode(&destination, &config.directory_inode)
                .map_err(|error| failure(-4, error))?;
            (config.labeler)(&destination).map_err(|error| failure(-4, error))?;
            copy_tree(&entry.path(), &destination, config)?;
        } else if metadata.is_file() {
            use std::os::unix::fs::OpenOptionsExt;
            let mut source = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(entry.path())
                .map_err(|error| failure(-4, error))?;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o644)
                .open(&destination)
                .map_err(|error| failure(-4, error))?;
            io::copy(&mut source, &mut file).map_err(|error| failure(-4, error))?;
            record_inode(&destination, &config.file_inode).map_err(|error| failure(-4, error))?;
            (config.labeler)(&destination).map_err(|error| failure(-4, error))?;
            file.sync_all().map_err(|error| failure(-4, error))?;
        } else {
            return Err(failure(
                -2,
                "Installation code contains a non-regular filesystem entry",
            ));
        }
    }
    fs::File::open(target)
        .and_then(|file| file.sync_all())
        .map_err(|error| failure(-4, error))
}
pub(crate) fn capture_visibility_checked(
    base:&Arc<Snapshot>,snapshots:&crate::package::scan_snapshot::Store,names:&[String],
    mut users:impl FnMut()->Result<Vec<i32>,Failure>,
    mut capture:impl FnMut(&crate::package::settings::Package,&[i32])->Result<(u64,Vec<i32>),Failure>,
)->Result<Vec<Vec<i32>>,Failure>{
    let targets=names.iter().cloned().collect();
    for _ in 0..8 {
        let current=snapshots.capture();
        if !base.same_lineage(&current){return Err(failure(-110,"pre-install visibility capture lineage changed"));}
        let mut checked=base.owner().clone();
        if let Some(conflict)=crate::package::scan_snapshot::install_context::install_user_rebase_conflict(
            base,&current,&mut checked,&targets).map_err(|error|failure(-110,error))? {
            return Err(failure(-110,format!("pre-install visibility owner changed: base={} current={} metadata={}->{}: {conflict}",
                base.version(),current.version(),base.metadata_revision(),current.metadata_revision())));
        }
        let all_users=users()?;let mut values=Vec::with_capacity(names.len());let mut coherent=true;
        for name in names {
            if let Some(setting)=current.owner().settings.packages.iter().find(|setting|&setting.name==name){
                let(version,allowlist)=capture(setting,&all_users)?;
                super::post_install::validate_visibility(&all_users,&allowlist).map_err(|error|failure(-110,error.message))?;
                if version!=current.version(){coherent=false;break;}
                values.push(allowlist);
            }else{values.push(Vec::new());}
        }
        if coherent&&Arc::ptr_eq(&current,&snapshots.capture())&&users()?==all_users{return Ok(values);}
        // Only the read-only policy capture repeats; code/app-data/permission
        // preparation and the original admission base retain their ownership.
    }
    let current=snapshots.capture();
    Err(failure(-110,format!("pre-install visibility capture did not stabilize: base={} current={} metadata={}->{}",
        base.version(),current.version(),base.metadata_revision(),current.metadata_revision())))
}

impl pipeline::Environment for Owner {
    fn reserve(
        &self,
        code: Vec<VerifiedCode>,
        base: &Arc<Snapshot>,
    ) -> Result<Box<dyn pipeline::Reservation>, Failure> {
        let current=self.config.snapshots.capture();
        if !base.same_lineage(&current){return Err(failure(-110,"Installation base lineage changed"));}
        if !Arc::ptr_eq(base,&current) {
            let targets=code.iter().map(|member|if member.package.static_shared_library_name.is_some(){
                format!("{}_{}",member.package.package_name,member.package.static_shared_lib_version)
            }else{member.package.package_name.clone()}).collect();
            let mut checked=base.owner().clone();
            if let Some(conflict)=crate::package::scan_snapshot::install_context::install_user_rebase_conflict(
                base,&current,&mut checked,&targets).map_err(|error|failure(-110,error))? {
                return Err(failure(-110,format!("Installation base owner changed: {conflict}")));
            }
        }
        let mut reserved = Reserved {
            config: self.config.clone(),
            members: Vec::new(),
            base: base.clone(),
            created: Mutex::new(Vec::new()),
            committed: AtomicBool::new(false),
            errors: self.errors.clone(),
            pending: self.pending.clone(),
        };
        let directory = self.config.data.join("app");
        if !directory.is_dir() {
            return Err(failure(-4, "Native /data/app owner is unavailable"));
        }
        for mut code in code {
            let internal = if code.package.static_shared_library_name.is_some() {
                format!(
                    "{}_{}",
                    code.package.package_name, code.package.static_shared_lib_version
                )
            } else {
                code.package.package_name.clone()
            };
            let installed = base
                .owner()
                .settings
                .disabled_system_packages
                .iter()
                .find(|package| package.name == internal)
                .or_else(|| {
                    base.owner()
                        .settings
                        .packages
                        .iter()
                        .find(|package| package.name == internal)
                });
            let mut policy = scan::ScanPolicy::default();
            if let Some(installed) =
                installed.filter(|package| package.flags & settings::FLAG_SYSTEM != 0)
            {
                policy.inherit_system_setting(installed);
            }
            let platform = base
                .owner()
                .loaded_packages()
                .get("android")
                .ok_or_else(|| failure(-110, "Platform signing owner unavailable"))?;
            policy.adjust_shared_uid_privilege(
                &code.package,
                &code.signing,
                &platform.collected_signing,
                &base.owner().identities,
                self.config.vendor_sdk,
            );
            let remove_test_base = if policy.system
                || self
                    .config
                    .library_compatibility
                    .test_base_on_bootclasspath()
            {
                None
            } else {
                (self.config.remove_test_base)(&code.package, policy.system)
                    .map_err(|error| failure(-110, error))?
            };
            policy
                .apply(
                    &mut code.package,
                    &code.signing,
                    Some(&platform.collected_signing),
                    policy.system,
                    &self.config.apks,
                    &self.config.library_compatibility,
                    remove_test_base,
                )
                .map_err(|error| failure(-2, error))?;
            let old = code
                .package
                .path
                .clone()
                .ok_or_else(|| failure(-2, "Verified package has no staging path"))?;
            let source = guest_host(&self.config.data, &old)?;
            if !fs::symlink_metadata(&source)
                .map_err(|error| failure(-4, error))?
                .is_dir()
            {
                return Err(failure(-2, "Install staging path is not a directory"));
            }
            let name = &code.package.package_name;
            if name.is_empty() || name.contains(['/', '\\', '\0']) {
                return Err(failure(-2, "Invalid installation package name"));
            }
            let basename = format!("{name}-{}", token());
            let host = directory.join(&basename);
            let guest = format!("/data/app/{basename}");
            fs::create_dir(&host).map_err(|error| failure(-4, error))?;
            // Retain cleanup ownership immediately, including metadata/copy failures.
            let metadata = (self.config.metadata)(&code, &guest, base).map_err(|error| {
                if let Err(cleanup) = fs::remove_dir_all(&host) {
                    self.errors.lock().unwrap().push(format!(
                        "Unadmitted code cleanup {}: {cleanup}",
                        host.display()
                    ));
                }
                failure(-110, error)
            })?;
            let install = (self.config.library_policy)(&code).map_err(|error| {
                if let Err(cleanup) = (self.config.release_permissions)(&guest, false) {
                    self.errors.lock().unwrap().push(format!("Install permission rollback {guest}: {}", cleanup.message));
                }
                if let Err(cleanup) = fs::remove_dir_all(&host) {
                    self.errors.lock().unwrap().push(format!(
                        "Unadmitted code cleanup {}: {cleanup}",
                        host.display()
                    ));
                }
                failure(-110, error)
            })?;
            reserved.members.push(Member {
                prior_visibility: Vec::new(),
                code,
                host: host.clone(),
                guest: guest.clone(),
                metadata,
                install,
            });
            record_inode(&host, &self.config.directory_inode)
                .map_err(|error| failure(-4, error))?;
            (self.config.labeler)(&host).map_err(|error| failure(-4, error))?;
            copy_tree(&source, &host, &self.config)?;
            let member = reserved.members.last_mut().unwrap();
            member.code.package.path = Some(guest.clone());
            member.code.package.base_apk_path = member
                .code
                .package
                .base_apk_path
                .as_deref()
                .map(|path| relocate(path, &old, &guest))
                .transpose()?;
            if let Some(paths) = &mut member.code.package.split_code_paths {
                for path in paths {
                    let original = path
                        .as_ref()
                        .ok_or_else(|| failure(-2, "Null install split path"))?;
                    *path = Some(relocate(original, &old, &guest)?);
                }
            }
            prepare_native_libraries(member, &self.config)?;
        }
        let names=reserved.members.iter().map(|member| {
            if member.code.package.static_shared_library_name.is_some() {
                format!("{}_{}",member.code.package.package_name,member.code.package.static_shared_lib_version)
            } else { member.code.package.package_name.clone() }
        }).collect::<Vec<_>>();
        let visibility=capture_visibility_checked(base,&self.config.snapshots,&names,
            ||(self.config.post_install_users)().map_err(|error|failure(-110,error.message)),
            |setting,users|self.config.effects.capture_post_install_visibility(setting,users)
                .map_err(|error|failure(-110,format!("Original pre-install visibility: {}",error.message))))?;
        for(member,values)in reserved.members.iter_mut().zip(visibility){member.prior_visibility=values;}
        Ok(Box::new(reserved))
    }
    fn publish_queries(&self, snapshot: &Arc<Snapshot>) -> Result<(), String> {
        (self.config.publish)(snapshot)
    }
    fn finish(&self, receipt: PublishedInstall) -> Result<(), Exception> {
        self.pending.complete(receipt.generation,|| (self.config.completion)(receipt),|plan| {
            plan.validate(&self.config.snapshots.capture())?;
            self.config.effects.post_install(plan)
        },|old| self.config.code_resources.clean(old,false).map_err(|message|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,format!("Committed install old code cleanup: {message}"))))
    }
}
// SCAN_NEW_INSTALL consumes ABI metadata already derived by installation.
fn prepare_native_libraries(member: &mut Member, config: &Config) -> Result<(), Failure> {
    let root = member.host.join("lib");
    let guest_root = format!("{}/lib", member.guest);
    let clock = |time| (config.zip_clock)(time);
    let label = |path: &Path| (config.labeler)(path);
    let destination = scan::NativeLibraryDestination {
        guest_root: &guest_root,
        root: &root,
        owner: config.file_inode,
        zip_time: &clock,
        restorecon: &label,
    };
    let package = &mut member.code.package;
    let override_abi = member.code.record.params.abi_override.as_deref();
    prepare_native_package(&config.apks, package, &config.abi, &scan::NativeLibraryEnvironment {
        preferred_abi: &config.preferred_abi,
        app_lib32_install_dir: &config.app_lib32_dir,
        code_is_directory: true,
        canonical_source: None,
    }, override_abi, member.install, &destination)?;
    member.metadata.primary_cpu_abi = package.primary_cpu_abi.clone();
    member.metadata.secondary_cpu_abi = package.secondary_cpu_abi.clone();
    member.metadata.legacy_native_library_path = package.native_library_root_dir.clone();
    Ok(())
}

fn prepare_native_package(
    apks: &Apks,
    package: &mut crate::package::pkg::AndroidPackage,
    policy: &scan::AbiPolicy,
    environment: &scan::NativeLibraryEnvironment<'_>,
    override_abi: Option<&str>,
    install: scan::NativeLibraryInstallPolicy,
    destination: &scan::NativeLibraryDestination<'_>,
) -> Result<(), Failure> {
    apks.copy_native_libraries_with_override(
        package, policy, override_abi, install, destination,
    ).map_err(|error| failure(error.code, error.message))?;
    let libraries = apks.native_library_scan(
        package, policy, environment, false, false,
        override_abi.filter(|value| *value != "-"),
    ).map_err(|error| failure(-110, format!("Install native library metadata: {error:?}")))?;
    libraries.apply_metadata(package);
    Ok(())
}

impl pipeline::Reservation for Reserved {
    fn requests(&self) -> Result<Vec<scan::live_install::Request>, Failure> {
        self.members
            .iter()
            .map(|member| {
                let code = &member.code;
                let record = &code.record;
                Ok(scan::live_install::Request {
                    code: code.package.clone(),
                    signing: code.signing.clone(),
                    metadata: member.metadata.clone(),
                    install_flags: record.params.install_flags,
                    required_installed_version: record.params.required_installed_version_code,
                    rollback: record.params.install_reason == 5,
                    source: (self.config.install_source)(code, &self.base)
                        .map_err(|error| failure(-110, error.message))?,
                    install_user: if record.params.install_flags & 0x40 != 0 {
                        -1
                    } else {
                        record.user as i32
                    },
                    now: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|error| failure(-110, error))?
                        .as_millis() as i64,
                })
            })
            .collect()
    }
    fn users(&self) -> &[scan::User] {
        &self.config.users
    }
    fn build_debuggable(&self) -> bool {
        self.config.build_debuggable
    }
    fn complete_metadata(
        &self,
        admission: scan::live_install::Admission,
        apks: &Apks,
    ) -> Result<scan::live_install::CompletedAdmission, Failure> {
        let environments: Vec<_> = self
            .members
            .iter()
            .map(|_| scan::NativeLibraryEnvironment {
                preferred_abi: &self.config.preferred_abi,
                app_lib32_install_dir: &self.config.app_lib32_dir,
                code_is_directory: true,
                canonical_source: None,
            })
            .collect();
        let guest_roots: Vec<_> = self
            .members
            .iter()
            .map(|member| format!("{}/lib", member.guest))
            .collect();
        let host_roots: Vec<_> = self
            .members
            .iter()
            .map(|member| member.host.join("lib"))
            .collect();
        for root in &host_roots {
            fs::create_dir_all(root).map_err(|error| failure(-4, error))?;
            record_inode(root, &self.config.directory_inode).map_err(|error| failure(-4, error))?;
            (self.config.labeler)(root).map_err(|error| failure(-4, error))?;
        }
        let restorecon = |path: &Path| (self.config.labeler)(path);
        let zip_time = |time: u32| (self.config.zip_clock)(time);
        let destinations: Vec<_> = host_roots
            .iter()
            .zip(&guest_roots)
            .map(|(root, guest)| scan::NativeLibraryDestination {
                guest_root: guest,
                root,
                owner: self.config.file_inode,
                zip_time: &zip_time,
                restorecon: &restorecon,
            })
            .collect();
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| failure(-110, error))?
            .as_millis() as i64;
        let inputs: Vec<_> = self
            .members
            .iter()
            .enumerate()
            .map(|(index, member)| {
                let internal = if member.code.package.static_shared_library_name.is_some() {
                    format!(
                        "{}_{}",
                        member.code.package.package_name,
                        member.code.package.static_shared_lib_version
                    )
                } else {
                    member.code.package.package_name.clone()
                };
                let old = self
                    .base
                    .owner()
                    .settings
                    .packages
                    .iter()
                    .find(|package| package.name == internal);
                scan::ScanMetadataCompletion {
                    seinfo: scan::SeInfoScan {
                        policy: &self.config.seinfo,
                        compatibility: self.config.compatibility.as_ref(),
                    },
                    abi_policy: &self.config.abi,
                    native_environment: &environments[index],
                    context: scan::AbiScanContext {
                        mode: scan::AbiScanMode::Install { moved: None },
                        system: old
                            .is_some_and(|package| package.flags & settings::FLAG_SYSTEM != 0),
                        updated: old
                            .is_some_and(|package| package.flags & settings::FLAG_SYSTEM != 0),
                        override_abi: member.code.record.params.abi_override.as_deref(),
                        platform_runtime_64bit: None,
                    },
                    install: member.install,
                    destination: Some(&destinations[index]),
                    clock: scan::ScanClock {
                        current_time,
                        user_id: member.code.record.user as i32,
                        update_time: true,
                    },
                    factory_test: self.config.factory_test,
                    scan_as_instant_app: member.code.record.params.install_flags & 0x800 != 0
                        || self
                            .base
                            .owner()
                            .scanned_user_states(&internal)
                            .and_then(|users| {
                                let user = if member.code.record.params.install_flags & 0x40 != 0 {
                                    -1
                                } else {
                                    member.code.record.user as i32
                                };
                                users.get(&user)
                            })
                            .is_some_and(|state| state.instant_app),
                }
            })
            .collect();
        admission
            .complete_metadata(apks, inputs)
            .map_err(|error| failure(-110, format!("Live metadata completion: {:?}", error.error)))
    }
    fn prepare_runtime(
        &self,
        admission: &mut scan::live_install::CompletedAdmission,
    ) -> Result<(), Failure> {
        // Permission/access preparation is performed by its actual owner before
        // any app-data operation. It populates the candidate's runtime owners.
        (self.config.permissions)(admission, &self.base).map_err(|error| failure(-110, error.message))?;
        for metadata in &mut admission.completed {
            let setting = &metadata.candidate.record.settings;
            let seinfo = admission
                .owner
                .seinfo(&setting.name)
                .map_err(|error| failure(-110, error))?
                .map(str::to_owned);
            for (user, state) in &mut metadata.candidate.users {
                if !state.installed {
                    continue;
                }
                let flags = (self.config.app_data_flags)(*user)
                    .map_err(|error| failure(-110, error.message))?
                    | if !metadata
                        .candidate
                        .record
                        .parsed
                        .uses_sdk_libraries
                        .is_empty()
                    {
                        8
                    } else {
                        0
                    };
                if flags & 3 == 0 {
                    continue;
                }
                let user_seinfo = seinfo.as_ref().map(|base| {
                    format!(
                        "{}{}:complete",
                        base,
                        if state.instant_app {
                            ":ephemeralapp"
                        } else {
                            ""
                        }
                    )
                });
                let result = (self.config.app_data)(crate::package::users::AppData {
                    volume_uuid: setting.volume_uuid.clone(),
                    package: setting.name.clone(),
                    user: *user,
                    flags,
                    app_id: setting.app_id,
                    seinfo: user_seinfo,
                    target_sdk_version: setting.target_sdk_version,
                })
                .map_err(|error| failure(-110, error.message))?;
                if flags & 2 != 0 && result.ce_inode != -1 {
                    state.ce_data_inode = result.ce_inode;
                }
                if flags & 1 != 0 && result.de_inode != -1 {
                    state.de_data_inode = result.de_inode;
                }
                if result.newly_created {
                    self.created.lock().unwrap().push((
                        setting.name.clone(),
                        *user,
                        result.ce_inode,
                    ));
                }
            }
            for (user, state) in &metadata.candidate.users {
                admission
                    .owner
                    .set_user_state(&setting.name, *user, state.clone())
                    .map_err(|error| failure(-110, error))?;
            }
        }
        Ok(())
    }
    fn cross_user_suspensions(&self) -> bool {
        self.config.cross_user_suspensions
    }
    fn mark_committed(&self, _: &Arc<Snapshot>) {
        self.committed.store(true, Ordering::Release);
        let created = self.created.lock().unwrap().drain(..).collect::<Vec<_>>();
        for (name, user, inode) in created {
            if let Err(error) = (self.config.commit_app_data)(&name, user, inode) {
                self.errors.lock().unwrap().push(format!(
                    "Install app-data commit {name}:{user}: {}",
                    error.message
                ));
            }
        }
    }
    fn publication_finished(&self, snapshot: &Arc<Snapshot>) -> Result<(), Exception> {
        let all_users=(self.config.post_install_users)()?;
        let mut plans=Vec::new();
        for member in &self.members {
            let name=if member.code.package.static_shared_library_name.is_some(){format!("{}_{}",member.code.package.package_name,member.code.package.static_shared_lib_version)}else{member.code.package.package_name.clone()};
            let mut plan=super::post_install::Plan::capture(&self.base,snapshot,&name,member.code.record.user as i32,
                member.code.record.params.install_flags,if member.code.record.params.data_loader_params.is_some(){1}else{0},member.code.package.static_shared_library_name.is_some(),&all_users)?;
            plan.prior_visibility=member.prior_visibility.clone();
            plan.validate(&self.config.snapshots.capture())?;
            // Original setPrepareResult(clearCodeCache=replace), after committed
            // app-data preparation and before either ART completion branch.
            if plan.replacing {(self.config.clear_code_cache)(&name,&plan.cache_users)?;}
            plans.push(plan);
        }
        self.pending.retain(snapshot.version(),plans)
    }
}
impl Drop for Reserved {
    fn drop(&mut self) {
        let committed = self.committed.load(Ordering::Acquire);
        for member in &self.members {
            if let Err(error) = (self.config.release_permissions)(&member.guest, committed) {
                self.errors.lock().unwrap().push(format!(
                    "Install permission release {}: {}", member.guest, error.message
                ));
            }
        }
        if committed {
            return;
        }
        for (name, user, inode) in self.created.lock().unwrap().drain(..).rev() {
            if let Err(error) = (self.config.rollback_app_data)(&name, user, inode) {
                self.errors.lock().unwrap().push(format!(
                    "Install app-data rollback {name}:{user}: {}",
                    error.message
                ));
            }
        }
        for member in &self.members {
            if let Err(error) = fs::remove_dir_all(&member.host) {
                if error.kind() != io::ErrorKind::NotFound {
                    self.errors
                        .lock()
                        .unwrap()
                        .push(format!("Install code rollback {}: {error}", member.guest));
                }
            }
        }
    }
}

fn record_inode(path: &Path, inode: &GuestInode) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = inode.mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))?;
    }
    guest_inode::record(path, *inode)
}

#[cfg(test)]
mod native_library_tests {
    use super::*;
    use crate::package::{parse::Platform, pkg::{AndroidPackage, booleans}};
    fn archive() -> Vec<u8> {
        let name = b"lib/arm64-v8a/libfixture.so";
        let payload = b"original native library bytes";
        let extra = 4096 - 30 - name.len();
        let mut local = vec![0; 30];
        local[..4].copy_from_slice(&0x04034b50u32.to_le_bytes());
        local[14..18].copy_from_slice(&crc32fast::hash(payload).to_le_bytes());
        local[18..22].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        local[22..26].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        local[28..30].copy_from_slice(&(extra as u16).to_le_bytes());
        let mut central = vec![0; 46];
        central[..4].copy_from_slice(&0x02014b50u32.to_le_bytes());
        central[16..28].copy_from_slice(&local[14..26]);
        central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(name);
        local.resize(4096, 0);
        local.extend_from_slice(payload);
        let offset = local.len();
        local.extend_from_slice(&central);
        local.extend_from_slice(name);
        let mut end = vec![0; 22];
        end[..4].copy_from_slice(&0x06054b50u32.to_le_bytes());
        end[8..10].copy_from_slice(&1u16.to_le_bytes());
        end[10..12].copy_from_slice(&1u16.to_le_bytes());
        end[12..16].copy_from_slice(&((46 + name.len()) as u32).to_le_bytes());
        end[16..20].copy_from_slice(&(offset as u32).to_le_bytes());
        local.extend_from_slice(&end);
        local
    }
    #[test]
    fn live_install_prepares_selected_abi_and_extract_or_direct_libraries() {
        struct Data(PathBuf);
        impl Drop for Data { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
        let data = Data(std::env::temp_dir().join(format!("aim-install-native-{}-{}",std::process::id(),token())));
        fs::create_dir(&data.0).unwrap();
        let apk = data.0.join("base.apk");
        let original = archive();
        fs::write(&apk, &original).unwrap();
        let files = apk.clone();
        let apks = Apks {
            signing_overrides: None,
            files: Box::new(move |path| (path == "/data/app/fixture/base.apk").then(||files.clone())),
            platform: Platform::load(&aim_paths::derived_image(),Default::default()).unwrap(),
        };
        let policy = scan::AbiPolicy {
            all: vec!["arm64-v8a".into()], bit32: vec![], bit64: vec!["arm64-v8a".into()],
            native32: vec![], native64: vec!["arm64-v8a".into()], force_multi_arch_match: false,
        };
        let env = scan::NativeLibraryEnvironment {
            preferred_abi:"arm64-v8a",app_lib32_install_dir:"/data/app-lib",code_is_directory:true,canonical_source:None,
        };
        for extract in [false,true] {
            let root = data.0.join(if extract {"extract"} else {"direct"});
            let destination = scan::NativeLibraryDestination {
                guest_root:"/data/app/fixture/lib",root:&root,
                owner:GuestInode {uid:Some(1000),gid:Some(1000),mode:None},
                zip_time:&|_|Ok(UNIX_EPOCH),restorecon:&|_|Ok(()),
            };
            let install = scan::NativeLibraryInstallPolicy {
                page_size:4096,extract,debuggable:false,compat_16kb_disabled:false,manifest_compat_disabled:false,
            };
            let mut package = AndroidPackage {
                package_name:"fixture".into(), path:Some("/data/app/fixture".into()),
                base_apk_path:Some("/data/app/fixture/base.apk".into()),
                booleans:if extract {booleans::EXTRACT_NATIVE_LIBS} else {0}, ..Default::default()
            };
            prepare_native_package(&apks,&mut package,&policy,&env,Some("-"),install,&destination).unwrap();
            assert_eq!(package.primary_cpu_abi.as_deref(),Some("arm64-v8a"));
            assert_eq!(package.native_library_dir.as_deref(),Some("/data/app/fixture/lib/arm64"));
            let lib = root.join("arm64/libfixture.so");
            if extract {
                assert_eq!(fs::read(&lib).unwrap(),b"original native library bytes");
                let inode = guest_inode::read(&lib).unwrap().unwrap();
                assert_eq!((inode.uid,inode.gid),(Some(1000),Some(1000)));
            } else { assert!(!lib.exists()); }
            let error = prepare_native_package(&apks,&mut package,&policy,&env,Some("x86"),install,&destination).unwrap_err();
            assert_eq!(error.legacy_status,-113);
            assert_eq!(fs::read(&apk).unwrap(),original);
        }
    }
}
