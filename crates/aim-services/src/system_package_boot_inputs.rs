//! Owned image scan policy and native first-capture service bindings.
use super::*;
use crate::package::{
    bootstrap::ScanPolicy,
    model,
    owner::seinfo,
    parse::{Platform, resources::Config},
    scan::{
        AbiPolicy, CertificateScanPolicy, NativeLibraryInstallPolicy, ScanClock, SupportedAbis,
    },
};
use std::{path::Path, sync::Arc};
#[derive(Clone)]
pub struct Cli {
    pub factory_test: bool,
    pub install_user: Option<i32>,
    pub allow_install: bool,
    pub instant_app: bool,
    pub virtual_preload: bool,
    pub stopped_system_app: bool,
    pub compat_16kb_disabled: bool,
    pub update_time: bool,
    pub scan_user: i32,
    pub parser_cache: Option<std::path::PathBuf>,
}
pub struct ImageScanInputs {
    pub abi: AbiPolicy,
    pub seinfo: seinfo::Policy,
    pub preferred_abi: String,
    pub app_lib32: String,
    pub runtime_64: bool,
    pub install: NativeLibraryInstallPolicy,
    pub clock: ScanClock,
    pub first_api: i32,
    pub vendor_sdk: i32,
    pub certificates: CertificateScanPolicy,
    pub cli: Cli,
}
impl ImageScanInputs {
    // PackageParser.PARSE_APEX at the pinned Android version is 1 << 10.
    pub fn policy(&self) -> ScanPolicy<'_> {
        ScanPolicy {
            certificates: self.certificates,
            seinfo: &self.seinfo,
            apex_parse_flags: 1 << 10,
            first_api_level: self.first_api,
            vendor_sdk: self.vendor_sdk,
            abi: &self.abi,
            preferred_abi: &self.preferred_abi,
            app_lib32_install_dir: &self.app_lib32,
            platform_runtime_64bit: self.runtime_64,
            install: self.install,
            clock: self.clock,
            factory_test: self.cli.factory_test,
            install_user: self.cli.install_user,
            allow_install: self.cli.allow_install,
            instant_app: self.cli.instant_app,
            virtual_preload: self.cli.virtual_preload,
            stopped_system_app: self.cli.stopped_system_app,
        }
    }
}

/// The platform facts the boot scan reads from the running system
/// (`PackageManagerService`'s constructor at android-16.0.0_r1:
/// `Build.SUPPORTED_ABIS` and its 32- and 64-bit lists,
/// `VMRuntime.is64Bit`, the 32-bit app library directory, the current
/// time, `Os.sysconf(_SC_PAGESIZE)` and `Build.IS_DEBUGGABLE`).
pub struct ScanFacts {
    pub abis: Vec<String>,
    pub abis32: Vec<String>,
    pub abis64: Vec<String>,
    pub runtime_64: bool,
    pub app_lib32: String,
    pub current_time: i64,
    pub page_size: i64,
    pub debuggable: bool,
}

/// Where the boot scan takes its platform facts and live properties from.
pub trait PlatformFacts {
    fn scan_facts(&self) -> Result<ScanFacts>;
    /// A live property, none when unset or empty.
    fn property(&self, name: &str) -> Result<Option<String>>;
}

impl ImageScanInputs {
    pub fn load(
        image: &Path,
        platform: &Platform,
        facts: &dyn PlatformFacts,
        lifecycle: &crate::package::lifecycle::Owner,
        cli: Cli,
    ) -> Result<Self> {
        let ScanFacts {
            abis: all,
            abis32: bit32,
            abis64: bit64,
            runtime_64,
            app_lib32,
            current_time,
            page_size,
            debuggable,
        } = facts.scan_facts()?;
        let first_error = std::cell::RefCell::new(None);
        let prop = |name: &str| match facts.property(name) {
            Ok(value) => value,
            Err(error) => {
                first_error.borrow_mut().get_or_insert(error);
                None
            }
        };
        let abi = AbiPolicy::from_platform(
            platform,
            &all,
            &SupportedAbis {
                bit32: &bit32,
                bit64: &bit64,
            },
            &prop,
        )
        .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
        if let Some(error) = first_error.into_inner() {
            return Err(error);
        }
        let number = |name: &str| -> Result<i32> {
            facts
                .property(name)?
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("actual {name} absent"),
                    )
                })?
                .parse()
                .map_err(|_| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("invalid actual {name}"),
                    )
                })
        };
        let first_api = number("ro.product.first_api_level")?;
        let vendor_sdk = number("ro.vendor.api_level")?;
        let seinfo = seinfo::Policy::load(image).map_err(|message| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
        })?;
        let preferred_abi = all.first().cloned().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "original supported ABI list empty",
            )
        })?;
        Ok(Self {
            abi,
            seinfo,
            preferred_abi,
            app_lib32,
            runtime_64,
            install: NativeLibraryInstallPolicy {
                page_size: page_size as u64,
                extract: false,
                debuggable,
                compat_16kb_disabled: cli.compat_16kb_disabled,
                manifest_compat_disabled: false,
            },
            clock: ScanClock {
                current_time,
                user_id: cli.scan_user,
                update_time: cli.update_time,
            },
            first_api,
            vendor_sdk,
            certificates: CertificateScanPolicy {
                upgrade: lifecycle.device_upgrading(),
                pre_n_mr1_upgrade: lifecycle.device_upgrading()
                    && lifecycle.prior_sdk_version() < 25,
            },
            cli,
        })
    }
}
pub fn image_classes(image: &Path) -> Result<Arc<aim_android_image::linkage::Hierarchy>> {
    let mut jars = aim_android_image::classpath::jars(
        image,
        "bootclasspath.pb",
        aim_android_image::classpath::BOOTCLASSPATH,
    )
    .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
    jars.extend(
        aim_android_image::classpath::jars(
            image,
            "systemserverclasspath.pb",
            aim_android_image::classpath::SYSTEMSERVERCLASSPATH,
        )
        .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?,
    );
    let hierarchy = aim_android_image::linkage::ClassPath::read(image, &jars)
        .and_then(|classpath| classpath.hierarchy())
        .map_err(|message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))?;
    Ok(Arc::new(hierarchy))
}

impl System {
    pub fn initial_runtime_system(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        initial: Arc<crate::system::package_boot_scan::InitialOwner>,
        platform: &Platform,
        config: Config,
        disk: &crate::package::owner::Store,
        lifecycle: Arc<crate::package::lifecycle::Owner>,
        mut image: model::System,
    ) -> Result<model::System> {
        use aim_service_aidl::dev_aim_server_ipackageinitialcontextleaf as api;
        let user_policy = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .map(|current| current.user_policy.clone())
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "initial UM policy bootstrap absent",
                )
            })?;
        let weak = Arc::downgrade(self);
        let epoch = bridge.clone();
        image.runtime_permission_queries = Some(Arc::new(
            crate::package::owner::runtime_metadata::Queries(Box::new(move |user| {
                let system = weak.upgrade().ok_or("runtime permission System stopped")?;
                system
                    .check_package_bootstrap(&epoch)
                    .map_err(|error| error.message)?;
                let metadata = {
                    let state = system.package_bootstrap.lock().unwrap();
                    state
                        .current
                        .as_ref()
                        .filter(|current| Arc::ptr_eq(&current.bridge, &epoch))
                        .and_then(|current| current.runtime_metadata.clone())
                        .ok_or("runtime permission metadata owner unavailable")?
                };
                let value = metadata.lock().unwrap().upgrade_needed(user);
                Ok(value)
            })),
        ));
        let resolution_leaf = self.package_bootstrap_binder_leaf(
            bridge,
            aim_service_aidl::dev_aim_server_ipackagebootstrapbridge::GET_PACKAGE_RESOLUTION_POLICY,
        )?;
        image.resolution_policy =
            Some(crate::package::resolve::policy::Owner::new(resolution_leaf));
        image.lifecycle = Some(lifecycle);
        image.user_policy = Some(user_policy);
        image.uninstall_blocks = Some(Arc::new(
            crate::package::mutations::UninstallBlocks::from_state(disk.state()),
        ));
        let source = initial.clone();
        image.permission_groups = Some(Arc::new(crate::package::permission_groups::Owner::new(
            Box::new(move |name, flags| {
                let bytes = source.invoke(
                    api::GET_PERMISSION_GROUP,
                    |p| {
                        p.write_string16(name);
                        p.write_i32(flags);
                    },
                    aim_service_aidl::read_byte_array,
                )?;
                Ok(bytes.map(|bytes| {
                    let mut parcel = Parcel::new();
                    parcel.write_raw(&bytes, &[]);
                    crate::package::permission_groups::Group::from_owned_parcel(parcel)
                }))
            }),
        )));
        let disabled = initial.clone();
        let revoke = initial.clone();
        let admin = initial.clone();
        let location = initial;
        image.security_policy = Some(Arc::new(
            crate::package::security_policy::Owner::new(
                platform,
                config,
                Box::new(move |name, uid, user| {
                    disabled.invoke(
                        api::IS_INSTALL_DISABLED,
                        |p| {
                            p.write_string16(Some(name));
                            p.write_i32(uid);
                            p.write_i32(user);
                        },
                        |r| r.read_bool(),
                    )
                }),
                Box::new(move |uid, name| {
                    revoke.invoke(
                        api::GET_AUTO_REVOKE,
                        |p| {
                            p.write_string16(name);
                            p.write_i32(uid);
                        },
                        |r| r.read_i32(),
                    )
                }),
                Box::new(move |name| {
                    let bytes = admin
                        .invoke(
                            api::GET_ADMIN_FACTS,
                            |p| p.write_string16(name),
                            aim_service_aidl::read_byte_array,
                        )?
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "original admin facts absent",
                            )
                        })?;
                    let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
                    let error = |code| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("admin facts parcel: {code}"),
                        )
                    };
                    let managers_present = reader.read_bool().map_err(error)?;
                    let device_owner = reader.read_string16().map_err(error)?;
                    let active_admin_users = aim_service_aidl::read_int_array(&mut reader)
                        .map_err(error)?
                        .ok_or_else(|| error(-22))?
                        .into_iter()
                        .collect();
                    let managed_role_holder_users = aim_service_aidl::read_int_array(&mut reader)
                        .map_err(error)?
                        .ok_or_else(|| error(-22))?
                        .into_iter()
                        .collect();
                    if reader.remaining() != 0 {
                        return Err(error(-22));
                    }
                    Ok(crate::package::security_policy::AdminFacts {
                        managers_present,
                        device_owner,
                        active_admin_users,
                        managed_role_holder_users,
                    })
                }),
                Box::new(move || {
                    location.invoke(api::GET_INSTALL_LOCATION, |_| {}, |r| r.read_i32())
                }),
            )
            .map_err(|message| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
            })?,
        ));
        Ok(image)
    }
}

impl System {
    /// Own every borrowed scan input until the single native first capture completes.
    pub fn finish_raw_from_image(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        mut begin: crate::system::package_boot_scan::Begun,
        apks: &crate::package::write::Apks,
        cli: Cli,
        config: Config,
    ) -> Result<crate::system::package_boot_scan::RawScanned> {
        use crate::package::scan::{
            NativeLibraryDestination, NativeLibraryEnvironment, NativeLibraryPaths,
        };
        use aim_service_aidl::dev_aim_server_ipackageinitialcontextleaf as api;
        use std::collections::{BTreeMap, BTreeSet};
        let failed = |message| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message);
        let context = begin
            .original_context
            .take()
            .ok_or_else(|| failed("original boot-context owner not bound".into()))?;
        let inputs = ImageScanInputs::load(
            &begin.image,
            &apks.platform,
            begin.initial.as_ref(),
            &begin.lifecycle.owner,
            cli,
        )?;
        let classes = image_classes(&begin.image)?;
        let runtime = {
            let disk = begin.prepared.disk.lock().unwrap();
            self.initial_runtime_system(
                bridge,
                begin.initial.clone(),
                &apks.platform,
                config,
                &disk,
                begin.lifecycle.owner.clone(),
                begin.model_system.clone(),
            )?
        };
        // InitAppsHelper.initNonSystemApps scans only PMS.mAppInstallDir
        // (/data/app). Adopted volumes are loaded by the later mount listener;
        // this is the initial scan scope, not an inventory of mounted volumes.
        let additional_data_volumes: &[String] = &[];
        let resources = crate::package::owner::resources::CodeResources::with_system(
            self.clone(),
            begin.data.clone(),
            inputs.cli.parser_cache.clone(),
        );
        let candidates = crate::package::scan::DataImage::parse(apks, additional_data_volumes)
            .map_err(|error| failed(format!("native data inventory: {error:?}")))?;
        let mut roots = BTreeMap::new();
        let mut incremental = BTreeSet::new();
        for candidate in &candidates.packages {
            let package = &candidate.code.parsed;
            let path = package
                .path
                .as_deref()
                .ok_or_else(|| failed("data package code path absent".into()))?;
            if begin.initial.invoke(
                api::IS_INCREMENTAL_PATH,
                |p| p.write_string16(Some(path)),
                |r| r.read_bool(),
            )? {
                incremental.insert(path.to_owned());
                incremental.insert(package.package_name.clone());
            }
            let host = (apks.files)(path)
                .ok_or_else(|| failed(format!("unmapped data package: {path}")))?;
            let paths = NativeLibraryPaths::derive(
                package,
                &NativeLibraryEnvironment {
                    preferred_abi: &inputs.preferred_abi,
                    app_lib32_install_dir: &inputs.app_lib32,
                    code_is_directory: host.is_dir(),
                    canonical_source: None,
                },
                false,
                false,
            )
            .map_err(failed)?;
            let writable = (apks.files)(&paths.root)
                .ok_or_else(|| failed(format!("unmapped native library root: {}", paths.root)))?;
            let data = begin
                .data
                .canonicalize()
                .map_err(|error| failed(error.to_string()))?;
            let mut ancestor = writable.as_path();
            while !ancestor.exists() {
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| failed("native library writable ancestor absent".into()))?;
            }
            if !ancestor
                .canonicalize()
                .map_err(|error| failed(error.to_string()))?
                .starts_with(&data)
            {
                return Err(failed("native library root outside disposable data".into()));
            }
            let existing = if host.is_dir() {
                path.to_owned()
            } else {
                path.rsplit_once('/')
                    .ok_or_else(|| failed("data code parent absent".into()))?
                    .0
                    .to_owned()
            };
            let bytes = begin
                .initial
                .invoke(
                    api::NATIVE_LIBRARY_INODE,
                    |p| p.write_string16(Some(&existing)),
                    aim_service_aidl::read_byte_array,
                )?
                .ok_or_else(|| failed("native library inode absent".into()))?;
            let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
            let decode = |code| failed(format!("native library inode parcel: {code}"));
            let uid = reader.read_i32().map_err(decode)?;
            let gid = reader.read_i32().map_err(decode)?;
            let mode = reader.read_i32().map_err(decode)?;
            if uid < 0 || gid < 0 || mode < 0 || reader.remaining() != 0 {
                return Err(failed("invalid native library inode".into()));
            }
            roots.insert(
                package.package_name.clone(),
                (
                    paths.root,
                    writable,
                    aim_storage::guest_inode::GuestInode {
                        uid: Some(uid as u32),
                        gid: Some(gid as u32),
                        mode: Some(mode as u32),
                    },
                ),
            );
        }
        let clock_source = begin.initial.clone();
        let zip_clock = move |time: u32| -> std::result::Result<std::time::SystemTime, String> {
            let millis = clock_source
                .invoke(
                    api::ZIP_ENTRY_TIME,
                    |p| p.write_i32(time as i32),
                    |r| r.read_i64(),
                )
                .map_err(|error| format!("original zip clock: {error:?}"))?;
            let duration = std::time::Duration::from_millis(millis.unsigned_abs());
            if millis >= 0 {
                std::time::UNIX_EPOCH.checked_add(duration)
            } else {
                std::time::UNIX_EPOCH.checked_sub(duration)
            }
            .ok_or_else(|| "zip clock overflow".into())
        };
        let label_bridge = bridge.clone();
        let mapped = &roots;
        let label = move |host: &std::path::Path| -> std::result::Result<(), String> {
            let (guest, root, _) = mapped
                .values()
                .find(|(_, root, _)| host.starts_with(root))
                .ok_or("native library label outside owned roots")?;
            let tail = host.strip_prefix(root).map_err(|error| error.to_string())?;
            let guest = if tail.as_os_str().is_empty() {
                guest.clone()
            } else {
                format!(
                    "{guest}/{}",
                    tail.to_str().ok_or("native library path UTF-8")?
                )
            };
            label_bridge
                .restore_installer_context(&guest)
                .map_err(|error| format!("native library SELinux owner: {error:?}"))
        };
        let destinations = roots
            .iter()
            .map(|(name, (guest, root, inode))| {
                (
                    name.clone(),
                    NativeLibraryDestination {
                        guest_root: guest,
                        root,
                        owner: *inode,
                        zip_time: &zip_clock,
                        restorecon: &label,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let old_stubs = begin
            .prepared
            .settings
            .packages
            .iter()
            .filter(|setting| setting.code_path.ends_with("-Stub"))
            .map(|setting| setting.name.clone())
            .collect::<BTreeSet<_>>();
        // Data scan derives expecting-better from accepted factory state internally.
        let expecting_better = BTreeSet::new();
        let source = begin.initial.clone();
        let is_incremental = move |path: &str| {
            source
                .invoke(
                    api::IS_INCREMENTAL_PATH,
                    |p| p.write_string16(Some(path)),
                    |r| r.read_bool(),
                )
                .map_err(|error| format!("incremental filesystem owner: {error:?}"))
        };
        // The boot context as system_server's leaf captures it (UserManager's
        // users, the resolver resources, compatibility mode), with the leaf
        // as the live settings owner.
        self.check_package_bootstrap(bridge)?;
        let settings_leaf = Arc::new(self.binder_process().strong(context.handle));
        let original = crate::package::scan_snapshot::boot_context::Original::capture(&context)
            .map_err(failed)?
            .bind_settings(settings_leaf);
        original
            .resolver
            .settings_owner
            .as_ref()
            .ok_or_else(|| failed("native initial settings owner absent".into()))?
            .register_updates(&self.binder_process())?;
        self.finish_raw_native_package_scan(
            bridge,
            begin,
            original,
            runtime,
            classes,
            apks,
            inputs.policy(),
            &resources,
            additional_data_volumes,
            &old_stubs,
            &incremental,
            &expecting_better,
            &destinations,
            &is_incremental,
            &zip_clock,
            config,
        )
    }
}
