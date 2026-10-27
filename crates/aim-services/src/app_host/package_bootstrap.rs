//! `IPackageBootstrapBridge` for the app host: the outside decisions and
//! the leaves the native package boot reads, answered as system_server's
//! `PackageBootstrapBridge` answers them (`android-16.0.0_r1`), from the
//! app host's own owners (`boot_policy`'s decisions, the boot leaves, the
//! current package version, the native-library and SDK-library policies,
//! the installer's SELinux labels through the agent, the permission gids
//! of a uid) for the native package boot it runs (served as its own node),
//! with the sub-bridges its construction takes: the package writer's
//! effects, the maintenance owner, and the native installer's build flags,
//! session icon decoder, confirmation leaves, external owners, staging
//! input and file owner, the domain verification settings, the
//! preferred resolver records' leases, the package caches' invalidation
//! the install environment's app-data, permission and completion owners,
//! the installer's preparation, removal and archive queries, the enable
//! bridge, the launch Intent's serialization, the application data
//! owners, the internal persistence, install and storage leaves, and the
//! move's storage leaves, monitor users and payloads, and internal event
//! preparation and the native services' PackageList observer registry.
//! Existing-install completion restores delayed permissions through the
//! permission owner and delivers its status; backup restore has no owner
//! (#1925). The install-existing app-data leaf remains unsupported (#2022).
//! The bridge's other owners, its other sub-bridges
//! among them, are not implemented (#2022).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;

use super::boot_policy;
use super::kept::{RawBundle, RawIntent};
use super::settings::Table;
use super::{AppHost, reply};
use crate::bundle::Value;
use crate::package::bootstrap::ScanUsers;
use crate::package::installer::existing::IntentSender;
use crate::package::pkg::AndroidPackage;
use crate::package::scan::SharedUidMigration;
use crate::package::system_config::{Sdk, SystemConfig};

const SYSTEM_UID: i32 = 1000;
/// `PackageBootstrapBridge.ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES`.
const ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES: i64 = 142191088;
/// `Settings.CURRENT_DATABASE_VERSION`
/// (`DatabaseVersion.SIGNATURE_MALFORMED_RECOVER`).
const CURRENT_DATABASE_VERSION: i32 = 3;
/// `android.content.pm.Flags.sdkLibIndependence`.
const SDK_LIB_INDEPENDENCE: &str = "android.content.pm.sdk_lib_independence";
/// `AppsFilterUtils`' query filtering change (`@EnabledSince(R)`), and
/// DomainVerificationService's restricted verification and its second
/// settings version.
const APPLICATION_QUERY_FILTERING: i64 = 135549675;
const DOMAIN_VERIFICATION_RESTRICTED: i64 = 175408749;
const DOMAIN_VERIFICATION_SETTINGS_V2: i64 = 178111421;
/// The agent's `restorecon(String path)`.
const RESTORECON: u32 = 15;

pub struct PackageBootstrap {
    host: Arc<AppHost>,
    observers: Arc<super::package_observers::Registry>,
    observer_events: OnceLock<aim_binder_host::parcel::Binder>,
    /// ApexManager's `mApexSystemServices`' names, kept across its
    /// notifications.
    apex_system_services: Mutex<Vec<String>>,
    /// `SystemConfig.getInstance()`, read once.
    config: OnceLock<SystemConfig>,
    /// The installer's external owner, made once (`installerExternal`).
    installer_external: OnceLock<aim_binder_host::parcel::Binder>,
    /// The installer's file owner, made once (`installerFiles`).
    installer_files: OnceLock<aim_binder_host::parcel::Binder>,
    /// The domain verification settings, made once
    /// (`installPackageDomainSettings`).
    domain_settings: OnceLock<aim_binder_host::parcel::Binder>,
    /// The install environment's app-data owner, made once
    /// (`packageAppData`).
    app_data: OnceLock<aim_binder_host::parcel::Binder>,
    /// The install environment's permission owner, made once
    /// (`installerPermissions`).
    installer_permissions: OnceLock<aim_binder_host::parcel::Binder>,
    /// The installer's preparation, made once (`installerPreparation`).
    installer_preparation: OnceLock<aim_binder_host::parcel::Binder>,
    /// The installer's removal owners, made once (the external bridge's
    /// `removal`).
    installer_removal: OnceLock<aim_binder_host::parcel::Binder>,
    /// The archive queries, made once (`installerArchiveQueries`).
    installer_archive_queries: OnceLock<aim_binder_host::parcel::Binder>,
    /// The application data owners, made once (`applicationData`).
    application_data: OnceLock<aim_binder_host::parcel::Binder>,
    /// The move's storage leaves, made once (`packageMoves`,
    /// `packageRelocation`).
    package_moves: OnceLock<aim_binder_host::parcel::Binder>,
    package_relocation: OnceLock<aim_binder_host::parcel::Binder>,
    /// The internal storage, made once (`internalStorage`).
    internal_storage: OnceLock<aim_binder_host::parcel::Binder>,
}

/// `Build.FINGERPRINT`: `ro.build.fingerprint`, else made of the build's
/// properties (`deriveFingerprint`).
fn build_fingerprint(property: impl Fn(&str) -> Option<String>) -> String {
    if let Some(fingerprint) = property("ro.build.fingerprint").filter(|f| !f.is_empty()) {
        return fingerprint;
    }
    let get = |name: &str| property(name).unwrap_or_else(|| "unknown".into());
    format!(
        "{}/{}/{}:{}/{}/{}:{}/{}",
        get("ro.product.brand"),
        get("ro.product.name"),
        get("ro.product.device"),
        get("ro.build.version.release"),
        get("ro.build.id"),
        get("ro.build.version.incremental"),
        get("ro.build.type"),
        get("ro.build.tags"),
    )
}

/// `PackageBootVersion.capture`: `Settings.VersionInfo.forceCurrent`'s
/// SDK, database version, `Build.FINGERPRINT` and
/// `PackagePartitions.FINGERPRINT`, parcelled.
fn current_package_version(property: impl Fn(&str) -> Option<String>) -> Vec<u8> {
    let sdk = property("ro.build.version.sdk")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut p = Parcel::new();
    p.write_i32(sdk);
    p.write_i32(CURRENT_DATABASE_VERSION);
    p.write_string16(Some(&build_fingerprint(&property)));
    p.write_string16(Some(&super::app_data::fingerprint(&property)));
    p.data().to_vec()
}

/// `restoreInstallerContext`'s paths: an install's code or staging
/// directory, or the sessions file and its backups, with no `.` or `..`
/// component.
fn installer_context_path(path: &str) -> bool {
    (path.starts_with("/data/app/")
        || path.starts_with("/data/app-staging/")
        || [
            "/data/system/install_sessions.xml",
            "/data/system/install_sessions.xml.new",
            "/data/system/install_sessions.xml.bak",
        ]
        .contains(&path))
        && !path.contains("/../")
        && !path.contains("/./")
        && !path.ends_with("/..")
        && !path.ends_with("/.")
}

/// `PackageCacher.fromCacheEntryStatic` of a call's parsed package.
fn cached_package(cache: Option<Vec<u8>>) -> Result<AndroidPackage, Exception> {
    let cache = cache.ok_or_else(|| Exception::illegal_argument("missing parsed package"))?;
    AndroidPackage::read_cache_entry(&cache)
        .map_err(|e| Exception::illegal_argument(format!("parsed package: {e:?}")))
}

impl PackageBootstrap {
    pub fn new(host: Arc<AppHost>) -> Self {
        let observers = Arc::new(super::package_observers::Registry::default());
        observers
            .add_observer(Arc::new(super::package_observers::Discoveries(
                Arc::downgrade(&host),
            )))
            .expect("new package observer epoch is open");
        let previous = std::mem::replace(
            &mut *host.package_observers.lock().unwrap(),
            Arc::downgrade(&observers),
        )
        .upgrade();
        if let Some(previous) = previous {
            previous.close();
        }
        PackageBootstrap {
            host,
            observers,
            observer_events: OnceLock::new(),
            apex_system_services: Mutex::new(Vec::new()),
            config: OnceLock::new(),
            installer_external: OnceLock::new(),
            installer_files: OnceLock::new(),
            domain_settings: OnceLock::new(),
            app_data: OnceLock::new(),
            installer_permissions: OnceLock::new(),
            installer_preparation: OnceLock::new(),
            installer_removal: OnceLock::new(),
            installer_archive_queries: OnceLock::new(),
            application_data: OnceLock::new(),
            package_moves: OnceLock::new(),
            package_relocation: OnceLock::new(),
            internal_storage: OnceLock::new(),
        }
    }

    /// `platform_compat`'s `getAppConfig(info).isChangeEnabled(change)` of
    /// an `ApplicationInfo` with only `package` and `target_sdk`
    /// (`DomainVerificationUtils.buildMockAppInfo`); `invalid` without them.
    fn app_change(
        &self,
        change: i64,
        package: Option<String>,
        target_sdk: i32,
        invalid: &str,
    ) -> Result<bool, Exception> {
        match package.filter(|n| !n.is_empty()) {
            Some(name) if target_sdk >= 0 => {
                Ok(self
                    .host
                    .platform()?
                    .compat
                    .is_enabled(change, Some(&name), target_sdk))
            }
            _ => Err(Exception::illegal_argument(invalid)),
        }
    }

    /// `LocalServices.getService(PermissionManagerServiceInternal.class)`.
    fn permissions(&self) -> Result<&Arc<super::permission_manager::PermissionManager>, Exception> {
        self.host
            .permission_manager
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable"))
    }

    fn config(&self) -> Result<&SystemConfig, Exception> {
        if let Some(config) = self.config.get() {
            return Ok(config);
        }
        let config = SystemConfig::read(self.host.root()?, &|name| self.host.property(name));
        Ok(self.config.get_or_init(|| config))
    }

    /// `ExistingInstallEffects.complete`: only the public status extra,
    /// with no pending-intent background activity launch grant.
    fn complete_existing_install(
        &self,
        name: Option<&str>,
        user: i32,
        target: Option<IntentSender>,
        status: i32,
        restore_permissions: bool,
    ) -> Result<Option<String>, Exception> {
        if restore_permissions
            && status == 1
            && let Some(name) = name
        {
            self.permissions()?
                .restore_delayed_runtime_permissions(name, user)?;
        }
        if let Some(target) = target {
            let options = RawBundle::empty()
                .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
                .map_err(|_| {
                    Exception::new(EX_ILLEGAL_STATE, "existing install broadcast options")
                })?;
            let intent = RawIntent::with_data(
                None,
                None,
                None,
                0,
                &[(
                    "android.content.pm.extra.STATUS",
                    Value::Int(super::installer_external::install_public_status(status)),
                )],
            );
            let result = self.host.send_intent_sender(
                SYSTEM_UID,
                None,
                target.0,
                0,
                Some(intent),
                None,
                None,
                None,
                Some(options),
            )?;
            if result < 0 {
                return Ok(Some(
                    "android.content.IntentSender$SendIntentException".into(),
                ));
            }
        }
        Ok(None)
    }

    /// `transactBatch`: each of `calls` (a count, then each call's code and
    /// data) answered as its own call, in order, until the replies pass
    /// `BATCH_REPLY_BUDGET`; a call's failure in its reply. The count of
    /// the calls answered, then each reply's bytes.
    fn transact_batch(&self, outer: &Call<'_>, calls: &[u8]) -> Result<Vec<u8>, Exception> {
        /// `PackageBootstrapBridge.BATCH_REPLY_BUDGET`.
        const BATCH_REPLY_BUDGET: usize = 256 * 1024;
        let invalid = |m: &str| Exception::illegal_argument(m);
        let mut r = aim_binder_host::parcel::Reader::new(calls, &[]);
        let count = r.read_i32().map_err(|_| invalid("invalid batch count"))?;
        if count < 0 || count as usize > r.remaining() / 8 {
            return Err(invalid("invalid batch count"));
        }
        let mut out = Parcel::new();
        out.write_i32(0);
        let mut answered = 0;
        while answered < count && (answered == 0 || out.data().len() < BATCH_REPLY_BUDGET) {
            let code = r.read_i32().map_err(|_| invalid("invalid batched call"))? as u32;
            let request = aim_service_aidl::read_byte_array(&mut r)
                .map_err(|_| invalid("invalid batched call"))?;
            let request = match request {
                Some(request) if code != bridge::TRANSACT_BATCH => request,
                _ => return Err(invalid("invalid batched call")),
            };
            let mut inner = Call {
                code,
                flags: 0,
                sender_pid: outer.sender_pid,
                sender_euid: outer.sender_euid,
                data: aim_binder_host::parcel::Reader::new(&request, &[]),
            };
            // `Binder.execTransact` reports a call's failure in its reply.
            let reply = match self.transact(&mut inner) {
                Ok(reply) => reply,
                Err(status) => {
                    let mut reply = Parcel::new();
                    reply.write_exception(&invalid(match status {
                        aim_binder_host::parcel::UNKNOWN_TRANSACTION => "unknown batched call",
                        _ => "malformed batched call",
                    }));
                    reply
                }
            };
            if !reply.objects().is_empty() {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "Tried to marshall a Parcel that contained Binder objects.",
                ));
            }
            aim_service_aidl::write_byte_array(&mut out, Some(reply.data()));
            answered += 1;
        }
        if answered == count && r.remaining() != 0 {
            return Err(invalid("batch trailing data"));
        }
        let mut bytes = out.data().to_vec();
        bytes[..4].copy_from_slice(&answered.to_le_bytes());
        Ok(bytes)
    }

    /// A leaf of this process, a new one for each call as the original
    /// makes them.
    fn leaf(&self, service: Arc<dyn Service>) -> aim_binder_host::parcel::Binder {
        self.host.process.add_service(service)
    }
}

/// `ParcelFileDescriptor` as `writeToParcel` writes it without a comm
/// channel: 0, then the fd.
struct Descriptor(u32);

impl aim_service_aidl::ReadParcelable for Descriptor {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        if r.read_i32()? != 0 {
            return Err(BAD_VALUE);
        }
        r.read_fd().map(Self)
    }
}

/// `PackageManager.INSTALL_ALLOCATE_AGGRESSIVE`, and
/// `StorageManager.FLAG_ALLOCATE_AGGRESSIVE` it translates to.
const INSTALL_ALLOCATE_AGGRESSIVE: i32 = 0x8000;
const FLAG_ALLOCATE_AGGRESSIVE: i32 = 1;

/// `allocateInstallerBytes`: `StorageManager.allocateBytes(fd, bytes,
/// translateAllocateFlags(flags))`, which asks the storage owner for what
/// the file has not allocated yet and then `posix_fallocate`s it to
/// `bytes`, as the guest's fallocate does on a host file: reserved from its
/// physical end, then extended.
fn allocate_installer_bytes(
    host: &AppHost,
    file: &impl std::os::fd::AsRawFd,
    bytes: i64,
    install_flags: i32,
) -> Result<(), Exception> {
    let failed = || super::storage::io_exception(&std::io::Error::last_os_error().to_string());
    let fd = file.as_raw_fd();
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: plain fstat.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return Err(failed());
    }
    let need = bytes - st.st_blocks * 512;
    if need > 0 {
        let flags = if install_flags & INSTALL_ALLOCATE_AGGRESSIVE != 0 {
            FLAG_ALLOCATE_AGGRESSIVE
        } else {
            0
        };
        host.allocate_bytes(None, need, flags, SYSTEM_UID)?;
        let mut store = libc::fstore_t {
            fst_flags: libc::F_ALLOCATEALL,
            fst_posmode: libc::F_PEOFPOSMODE,
            fst_offset: 0,
            fst_length: need,
            fst_bytesalloc: 0,
        };
        // SAFETY: F_PREALLOCATE with a local fstore.
        if unsafe { libc::fcntl(fd, libc::F_PREALLOCATE, &mut store) } < 0 {
            return Err(failed());
        }
    }
    // SAFETY: plain ftruncate.
    if st.st_size < bytes && unsafe { libc::ftruncate(fd, bytes) } < 0 {
        return Err(failed());
    }
    Ok(())
}

impl Service for PackageBootstrap {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    /// `allocateInstallerBytes` takes the staged file's descriptor.
    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforceSystemUid`.
        if call.sender_euid as i32 != SYSTEM_UID {
            return reply(Err(Exception::security(
                "the package bridge serves the system uid only",
            )));
        }
        let mut p = Parcel::new();
        let host = &self.host;
        let result: Result<(), Exception> = match call.code {
            bridge::COMPLETE_EXISTING_PACKAGE_INSTALL => {
                let a =
                    bridge::CompleteExistingPackageInstall::<IntentSender>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                self.complete_existing_install(
                    a.package_name.as_deref(),
                    a.user_id,
                    a.target,
                    a.status,
                    a.restore_permissions,
                )
                .map(|v| bridge::write_complete_existing_package_install_reply(&mut p, &v))
            }
            bridge::RESTORE_EXISTING_PACKAGE_INSTALL => {
                bridge::RestoreExistingPackageInstall::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                // `ExistingInstallEffects.restore`: checkService("backup")
                // finds no owner in the app host (#1925).
                bridge::write_restore_existing_package_install_reply(&mut p, false);
                Ok(())
            }
            bridge::GET_EXISTING_PACKAGE_INSTALL_USER_POLICY => {
                let a = bridge::GetExistingPackageInstallUserPolicy::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let user = a.user_id;
                    if user < 0 {
                        return Err(Exception::illegal_argument(format!(
                            "Invalid userId {user}"
                        )));
                    }
                    let mut record = Parcel::new();
                    record.write_i32(user);
                    record.write_bool(super::user_service::serial_number(host, user)?.is_some());
                    record.write_bool(host.has_user_restriction("no_install_apps", user)?);
                    record.write_bool(host.has_user_restriction("no_debugging_features", user)?);
                    record.write_bool(host.device_policies()?.managed_users().contains(&user));
                    bridge::write_get_existing_package_install_user_policy_reply(
                        &mut p,
                        &Some(record.data().to_vec()),
                    );
                    Ok(())
                })()
            }
            bridge::IS_SHELL_DEBUGGING_RESTRICTED => {
                let a = bridge::IsShellDebuggingRestricted::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if a.user_id < 0 {
                    return reply(Err(Exception::illegal_argument(format!(
                        "Invalid userId {}",
                        a.user_id
                    ))));
                }
                host.has_user_restriction("no_debugging_features", a.user_id)
                    .map(|v| bridge::write_is_shell_debugging_restricted_reply(&mut p, v))
            }
            bridge::ALLOCATE_INSTALLER_BYTES => {
                let a = bridge::AllocateInstallerBytes::<Descriptor>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let fd = a.file.filter(|_| a.length_bytes > 0).ok_or_else(|| {
                        Exception::illegal_argument("invalid installer allocation")
                    })?;
                    let file = host
                        .process
                        .file(fd.0)
                        .and_then(|file| aim_binder_host::server::file_fd(&file))
                        .ok_or_else(|| Exception::illegal_argument("invalid installer file"))?;
                    allocate_installer_bytes(host, &file, a.length_bytes, a.install_flags)
                })()
                .map(|()| bridge::write_allocate_installer_bytes_reply(&mut p))
            }
            // `InstallerUserPolicy.capture`: what `createSessionInternal`
            // asks UserManager and DevicePolicyManager of the user.
            bridge::GET_INSTALLER_USER_POLICY => {
                let a =
                    bridge::GetInstallerUserPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if a.user_id < 0 {
                    return reply(Err(Exception::illegal_argument(format!(
                        "Invalid userId {}",
                        a.user_id
                    ))));
                }
                let user = a.user_id;
                (|| {
                    let mut record = Parcel::new();
                    record.write_i32(user);
                    record.write_bool(super::user_service::user_ids(host, true)?.contains(&user));
                    record.write_bool(host.has_user_restriction("no_install_apps", user)?);
                    record.write_bool(host.has_user_restriction("no_debugging_features", user)?);
                    record.write_bool(host.device_policies()?.managed_users().contains(&user));
                    Ok(record.data().to_vec())
                })()
                .map(|v| bridge::write_get_installer_user_policy_reply(&mut p, &Some(v)))
            }
            bridge::GET_PACKAGE_PROFILE_PARENT => {
                let a =
                    bridge::GetPackageProfileParent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::profile_parent(host, a.user_id).map(|v| {
                    bridge::write_get_package_profile_parent_reply(&mut p, v.unwrap_or(-10000))
                })
            }
            bridge::IS_PARENT_PROFILE_APP_LINKING_ALLOWED => {
                let a = bridge::IsParentProfileAppLinkingAllowed::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.has_user_restriction("allow_parent_profile_app_linking", a.user_id)
                    .map(|v| bridge::write_is_parent_profile_app_linking_allowed_reply(&mut p, v))
            }
            bridge::GET_PACKAGE_ROLE_HOLDERS | bridge::GET_PREFERRED_ROLE_HOLDER => {
                let (role, user) = if call.code == bridge::GET_PACKAGE_ROLE_HOLDERS {
                    let a = bridge::GetPackageRoleHolders::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.role, a.user_id)
                } else {
                    let a = bridge::GetPreferredRoleHolder::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.role, a.user_id)
                };
                if call.code == bridge::GET_PREFERRED_ROLE_HOLDER
                    && host.role_service.get().is_none()
                {
                    bridge::write_get_preferred_role_holder_reply(&mut p, &None);
                    return reply(Ok(p));
                }
                host.role_service
                    .get()
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "role owner unavailable"))
                    .and_then(|owner| owner.role_holders(SYSTEM_UID, role, user))
                    .map(|holders| {
                        if call.code == bridge::GET_PACKAGE_ROLE_HOLDERS {
                            bridge::write_get_package_role_holders_reply(
                                &mut p,
                                &Some(holders.into_iter().map(Some).collect()),
                            );
                        } else {
                            bridge::write_get_preferred_role_holder_reply(
                                &mut p,
                                &holders.first().cloned(),
                            );
                        }
                    })
            }
            bridge::GET_PACKAGE_POLICY_BRIDGE => {
                bridge::GetPackagePolicyBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let owner = self.leaf(Arc::new(super::package_policy::PackagePolicy(host.clone())));
                bridge::write_get_package_policy_bridge_reply(&mut p, Some(owner));
                Ok(())
            }
            bridge::IS_AUTO_REVOKE_WHITELISTED => {
                let a =
                    bridge::IsAutoRevokeWhitelisted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::appops_service::mode(
                    host,
                    super::package_initial_context::OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED,
                    a.calling_uid,
                    a.package_name.as_deref(),
                )
                .map(|mode| {
                    bridge::write_is_auto_revoke_whitelisted_reply(
                        &mut p,
                        mode == super::appops_service::MODE_IGNORED,
                    )
                })
            }
            bridge::GET_PACKAGE_SHELL_READ_LEAF => {
                bridge::GetPackageShellReadLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let owner = self.leaf(Arc::new(super::package_shell_read::ShellRead(host.clone())));
                bridge::write_get_package_shell_read_leaf_reply(&mut p, Some(owner));
                Ok(())
            }
            bridge::GET_PACKAGE_SHELL_POLICY_BRIDGE => {
                bridge::GetPackageShellPolicyBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let owner = self.leaf(Arc::new(super::package_shell_policy::ShellPolicy(
                    host.clone(),
                )));
                bridge::write_get_package_shell_policy_bridge_reply(&mut p, Some(owner));
                Ok(())
            }
            bridge::GET_PACKAGE_OBSERVER_EVENTS_BRIDGE => {
                bridge::GetPackageObserverEventsBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let events = *self.observer_events.get_or_init(|| {
                    self.leaf(Arc::new(super::package_observers::Events(
                        self.observers.clone(),
                    )))
                });
                bridge::write_get_package_observer_events_bridge_reply(&mut p, Some(events));
                Ok(())
            }
            bridge::GET_PACKAGE_INTERNAL_EVENTS_BRIDGE => {
                bridge::GetPackageInternalEventsBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let events = self.leaf(Arc::new(
                    super::package_internal_events::InternalEvents::new(host.clone()),
                ));
                bridge::write_get_package_internal_events_bridge_reply(&mut p, Some(events));
                Ok(())
            }
            bridge::PACKAGE_MONITOR_RESULT => {
                let a =
                    bridge::PackageMonitorResult::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let result = super::package_writes::monitor_result(
                    a.action.as_deref(),
                    a.package_name.as_deref(),
                    a.user_id,
                    a.uid,
                    a.replacing,
                );
                bridge::write_package_monitor_result_reply(&mut p, &Some(result));
                Ok(())
            }
            bridge::PACKAGE_MONITOR_USER => {
                let a = bridge::PackageMonitorUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.handle_incoming_user(
                    a.calling_uid,
                    a.user_id,
                    true,
                    true,
                    Some("registerPackageMonitorCallback"),
                )
                .map(|user| bridge::write_package_monitor_user_reply(&mut p, user))
            }
            bridge::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                bridge::IsTestBaseOnBootclasspath::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.root()
                    .and_then(|root| {
                        boot_policy::test_base_on_bootclasspath(root)
                            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    })
                    .map(|v| bridge::write_is_test_base_on_bootclasspath_reply(&mut p, v))
            }
            // `android.content.pm.Flags.cloudCompilationVerification()`.
            bridge::INSTALLER_CLOUD_COMPILATION_VERIFICATION_ENABLED => {
                bridge::InstallerCloudCompilationVerificationEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.framework().map(|framework| {
                    let enabled = framework
                        .0
                        .flags
                        .get("android.content.pm.cloud_compilation_verification")
                        == Some(&true);
                    bridge::write_installer_cloud_compilation_verification_enabled_reply(
                        &mut p, enabled,
                    )
                })
            }
            // DeviceConfig `package_manager_service`'s pre-verified domain
            // count and length limits.
            bridge::GET_INSTALLER_DOMAIN_LIMITS => {
                bridge::GetInstallerDomainLimits::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let config = host
                        .platform()?
                        .settings
                        .config_flags(Some("package_manager_service/"))?;
                    let limit = |name: &str, default: i64| {
                        config
                            .get(&format!("package_manager_service/{name}"))
                            .and_then(|v| v.as_deref())
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(default)
                    };
                    Ok(vec![
                        limit("pre_verified_domains_count_limit", 1000),
                        limit("pre_verified_domain_length_limit", 256),
                    ])
                })()
                .map(|v| bridge::write_get_installer_domain_limits_reply(&mut p, &Some(v)))
            }
            // `com.android.art.flags.Flags.artServiceV3()`, from the ART
            // APEX's flag storage.
            bridge::IS_INSTALLER_ART_SERVICE_V3ENABLED => {
                bridge::IsInstallerArtServiceV3Enabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.framework().map(|framework| {
                    let enabled = framework
                        .0
                        .flags
                        .get("com.android.art.flags.art_service_v3")
                        == Some(&true);
                    bridge::write_is_installer_art_service_v3enabled_reply(&mut p, enabled)
                })
            }
            // `PackageInstaller.ENABLE_REVOCABLE_FD`.
            bridge::IS_INSTALLER_REVOCABLE_FD_ENABLED => {
                bridge::IsInstallerRevocableFdEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let enabled =
                    super::system_boolean(host.property("fw.revocable_fd").as_deref(), false);
                bridge::write_is_installer_revocable_fd_enabled_reply(&mut p, enabled);
                Ok(())
            }
            // `Build.isDebuggable()`.
            bridge::IS_SIGNING_DEBUGGABLE => {
                bridge::IsSigningDebuggable::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let debuggable = host.property("ro.debuggable").as_deref() == Some("1");
                bridge::write_is_signing_debuggable_reply(&mut p, debuggable);
                Ok(())
            }
            bridge::IS_SHARED_UID_MIGRATION_BEST_EFFORT => {
                bridge::IsSharedUidMigrationBestEffort::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let migration = boot_policy::shared_uid_migration(
                    host.property("ro.build.type").as_deref(),
                    host.property(boot_policy::SHARED_UID_MIGRATION_STRATEGY)
                        .as_deref(),
                );
                bridge::write_is_shared_uid_migration_best_effort_reply(
                    &mut p,
                    migration == SharedUidMigration::BestEffort,
                );
                Ok(())
            }
            bridge::GENERATE_NEW_DOMAIN_ID => {
                bridge::GenerateNewDomainId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `DomainVerificationService.generateNewId`: `UUID.randomUUID()`.
                let id = boot_policy::random_uuid();
                bridge::write_generate_new_domain_id_reply(&mut p, &Some(id.to_vec()));
                Ok(())
            }
            bridge::GET_PACKAGE_SCAN_USERS => {
                bridge::GetPackageScanUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::scan_users(host).map(|users| {
                    let record = ScanUsers { users: Some(users) }.write_original_record();
                    bridge::write_get_package_scan_users_reply(&mut p, &Some(record))
                })
            }
            bridge::GET_APEX_BOOT_INVENTORY => {
                bridge::GetApexBootInventory::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::apex_manager::inventory(&host.process)
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    .map(|inventory| {
                        bridge::write_get_apex_boot_inventory_reply(
                            &mut p,
                            &Some(inventory.write_original_record()),
                        )
                    })
            }
            bridge::NOTIFY_APEX_SCAN_RESULTS => {
                let a =
                    bridge::NotifyApexScanResults::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.scan_results {
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                    Some(bytes) if bytes.len() < 4 => Err(Exception::new(
                        EX_ILLEGAL_ARGUMENT,
                        "missing APEX result count",
                    )),
                    Some(bytes) if bytes.len() & 3 != 0 => Err(Exception::new(
                        EX_ILLEGAL_ARGUMENT,
                        "unaligned APEX scan results",
                    )),
                    Some(bytes) => boot_policy::notify_apex_payload(
                        &Sdk::new(&|name| host.property(name)),
                        &mut self.apex_system_services.lock().unwrap(),
                        &bytes,
                    )
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    .map(|()| bridge::write_notify_apex_scan_results_reply(&mut p)),
                }
            }
            bridge::IS_TEST_BASE_LIBRARY_CHANGE_ENABLED => {
                let a = bridge::IsTestBaseLibraryChangeEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                cached_package(a.package_cache)
                    .and_then(|pkg| {
                        Ok(boot_policy::test_base_change(
                            &host.platform()?.compat,
                            &pkg,
                        ))
                    })
                    .map(|v| bridge::write_is_test_base_library_change_enabled_reply(&mut p, v))
            }
            bridge::GET_SE_INFO_TARGET_SDK_VERSION => {
                let a = bridge::GetSeInfoTargetSdkVersion::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                cached_package(a.package_cache)
                    .and_then(|pkg| {
                        Ok(boot_policy::seinfo_target_sdk(
                            &host.platform()?.compat,
                            &pkg,
                        ))
                    })
                    .map(|v| bridge::write_get_se_info_target_sdk_version_reply(&mut p, v))
            }
            bridge::GET_PACKAGE_INITIAL_CONTEXT_LEAF => {
                bridge::GetPackageInitialContextLeaf::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(
                    super::package_initial_context::InitialContext::new(host.clone()),
                ));
                bridge::write_get_package_initial_context_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF => {
                let a = bridge::GetPackageBootConfigurationLeaf::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(
                    super::package_boot_configuration::BootConfiguration::new(
                        host.clone(),
                        a.factory_test,
                    ),
                ));
                bridge::write_get_package_boot_configuration_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_BOOT_CONTEXT_LEAF => {
                bridge::GetPackageBootContextLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(super::package_boot_context::BootContext::new(host.clone()));
                bridge::write_get_package_boot_context_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_LIFECYCLE_LEAF => {
                bridge::GetPackageLifecycleLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(super::package_boot_context::Lifecycle::new(
                    host.clone(),
                )));
                bridge::write_get_package_lifecycle_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_PACKAGE_BOOT_LIFECYCLE_LEAF => {
                bridge::GetPackageBootLifecycleLeaf::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let leaf = self.leaf(Arc::new(super::package_boot_lifecycle::BootLifecycle::new(
                    host.clone(),
                )));
                bridge::write_get_package_boot_lifecycle_leaf_reply(&mut p, Some(leaf));
                Ok(())
            }
            bridge::GET_CURRENT_PACKAGE_VERSION => {
                bridge::GetCurrentPackageVersion::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let version = current_package_version(|name| host.property(name));
                bridge::write_get_current_package_version_reply(&mut p, &Some(version));
                Ok(())
            }
            bridge::RESTORE_INSTALLER_CONTEXT => {
                let a =
                    bridge::RestoreInstallerContext::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.path.filter(|path| installer_context_path(path)) {
                    None => Err(Exception::illegal_argument(
                        "invalid installer context path",
                    )),
                    // `SELinux.restorecon`, by the agent.
                    Some(path) => host
                        .call_agent(RESTORECON, "restorecon", |p| p.write_string16(Some(&path)))
                        .and_then(|reply| {
                            let mut r = reply.reader();
                            r.read_exception()
                                .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("{s}")))??;
                            r.read_i32()
                                .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("{s}")))
                        })
                        .and_then(|restored| match restored {
                            0 => Err(Exception::new(
                                EX_ILLEGAL_STATE,
                                format!("installer restorecon failed: {path}"),
                            )),
                            _ => Ok(bridge::write_restore_installer_context_reply(&mut p)),
                        }),
                }
            }
            // `PlatformCompat.isChangeEnabledInternal`.
            bridge::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED => {
                let a = bridge::AreNativeLibraryDependenciesEnforced::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.platform().map(|platform| {
                    let enforced = platform.compat.is_enabled(
                        ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES,
                        a.package_name.as_deref(),
                        a.target_sdk,
                    );
                    bridge::write_are_native_library_dependencies_enforced_reply(&mut p, enforced)
                })
            }
            bridge::IS_SDK_LIBRARY_INDEPENDENCE_ENABLED => {
                bridge::IsSdkLibraryIndependenceEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.framework().map(|framework| {
                    let on = framework.0.flags.get(SDK_LIB_INDEPENDENCE) == Some(&true);
                    bridge::write_is_sdk_library_independence_enabled_reply(&mut p, on)
                })
            }
            // `UUID.ENABLE_STRICT_VALIDATION`: `VMRuntime.getSdkVersion()`
            // from U, and the change, which system_server's process has no
            // `AppCompatCallbacks` to disable (`handleBindApplication`
            // installs them; the system process binds no application).
            bridge::IS_DOMAIN_SET_UUID_STRICT_VALIDATION_ENABLED => {
                bridge::IsDomainSetUuidStrictValidationEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let sdk = host
                    .property("ro.build.version.sdk")
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or(0);
                bridge::write_is_domain_set_uuid_strict_validation_enabled_reply(&mut p, sdk >= 34);
                Ok(())
            }
            bridge::IS_APPLICATION_QUERY_FILTERING_ENABLED => {
                let a = bridge::IsApplicationQueryFilteringEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.app_change(
                    APPLICATION_QUERY_FILTERING,
                    a.package_name,
                    a.target_sdk,
                    "invalid query compatibility identity",
                )
                .map(|on| bridge::write_is_application_query_filtering_enabled_reply(&mut p, on))
            }
            bridge::IS_DOMAIN_VERIFICATION_RESTRICTED => {
                let a = bridge::IsDomainVerificationRestricted::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.app_change(
                    DOMAIN_VERIFICATION_RESTRICTED,
                    a.package_name,
                    a.target_sdk,
                    "invalid domain compatibility identity",
                )
                .map(|on| bridge::write_is_domain_verification_restricted_reply(&mut p, on))
            }
            bridge::IS_DOMAIN_VERIFICATION_SETTINGS_V2ENABLED => {
                let a = bridge::IsDomainVerificationSettingsV2Enabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.app_change(
                    DOMAIN_VERIFICATION_SETTINGS_V2,
                    a.package_name,
                    a.target_sdk,
                    "invalid domain compatibility identity",
                )
                .map(|on| bridge::write_is_domain_verification_settings_v2enabled_reply(&mut p, on))
            }
            bridge::TRANSACT_BATCH => {
                let a = bridge::TransactBatch::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.calls {
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                    Some(calls) => self
                        .transact_batch(call, &calls)
                        .map(|bytes| bridge::write_transact_batch_reply(&mut p, &Some(bytes))),
                }
            }
            bridge::GET_PACKAGE_RESOLUTION_POLICY => {
                bridge::GetPackageResolutionPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let policy = self.leaf(Arc::new(
                    super::package_resolution_policy::ResolutionPolicy::new(host.clone()),
                ));
                bridge::write_get_package_resolution_policy_reply(&mut p, Some(policy));
                Ok(())
            }
            bridge::GET_WEB_INSTANT_APPS_STATE => {
                bridge::GetWebInstantAppsState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let state = self.leaf(super::web_instant_apps::WebInstantApps::new(host.clone()));
                bridge::write_get_web_instant_apps_state_reply(&mut p, Some(state));
                Ok(())
            }
            bridge::GET_PACKAGE_MUTATION_BRIDGE => {
                bridge::GetPackageMutationBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let effects = self.leaf(Arc::new(super::package_effects::PackageEffects::new(
                    host.clone(),
                )));
                bridge::write_get_package_mutation_bridge_reply(&mut p, Some(effects));
                Ok(())
            }
            bridge::GET_PACKAGE_MAINTENANCE_BRIDGE => {
                bridge::GetPackageMaintenanceBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let maintenance = self.leaf(Arc::new(
                    super::package_maintenance::PackageMaintenance::new(host.clone()),
                ));
                bridge::write_get_package_maintenance_bridge_reply(&mut p, Some(maintenance));
                Ok(())
            }
            bridge::GET_INSTALLER_EXTERNAL_BRIDGE => {
                bridge::GetInstallerExternalBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let external = *self.installer_external.get_or_init(|| {
                    self.leaf(Arc::new(super::installer_external::InstallerExternal::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_installer_external_bridge_reply(&mut p, Some(external));
                Ok(())
            }
            bridge::GET_INSTALLER_CONFIRMATION_BRIDGE => {
                let a = bridge::GetInstallerConfirmationBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let confirmation = self.leaf(Arc::new(
                    super::installer_confirmation::InstallerConfirmation::new(
                        host.clone(),
                        a.dependency_installer_enabled,
                    ),
                ));
                bridge::write_get_installer_confirmation_bridge_reply(&mut p, Some(confirmation));
                Ok(())
            }
            // `PackageManager.invalidatePackageInfoCache` and
            // `ApplicationPackageManager.invalidateGetPackagesForUidCache`:
            // the app host publishes no nonce for these caches, in its
            // nonce store or as a `cache_key` property, so no process keeps
            // a value they would invalidate.
            bridge::INVALIDATE_PACKAGE_INFO_CACHE => {
                bridge::InvalidatePackageInfoCache::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_invalidate_package_info_cache_reply(&mut p);
                Ok(())
            }
            bridge::INVALIDATE_PACKAGES_FOR_UID_CACHE => {
                bridge::InvalidatePackagesForUidCache::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                bridge::write_invalidate_packages_for_uid_cache_reply(&mut p);
                Ok(())
            }
            bridge::ALLOCATE_PREFERRED_RESOLVER_IDENTITY => {
                bridge::AllocatePreferredResolverIdentity::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let lease = self.leaf(Arc::new(super::preferred_records::Lease::identity()));
                bridge::write_allocate_preferred_resolver_identity_reply(&mut p, Some(lease));
                Ok(())
            }
            bridge::ALLOCATE_PREFERRED_RESOLVER_RECORD => {
                let a = bridge::AllocatePreferredResolverRecord::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                super::preferred_records::allocate(a.record).map(|lease| {
                    let lease = self.leaf(Arc::new(lease));
                    bridge::write_allocate_preferred_resolver_record_reply(&mut p, Some(lease));
                })
            }
            bridge::GET_PACKAGE_APP_DATA_BRIDGE => {
                bridge::GetPackageAppDataBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let app_data = *self.app_data.get_or_init(|| {
                    self.leaf(Arc::new(super::package_app_data::PackageAppData::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_package_app_data_bridge_reply(&mut p, Some(app_data));
                Ok(())
            }
            bridge::GET_INSTALLER_PERMISSION_BRIDGE => {
                bridge::GetInstallerPermissionBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let permissions = *self.installer_permissions.get_or_init(|| {
                    self.leaf(Arc::new(
                        super::installer_permissions::InstallerPermissions::new(host.clone()),
                    ))
                });
                bridge::write_get_installer_permission_bridge_reply(&mut p, Some(permissions));
                Ok(())
            }
            bridge::GET_INSTALLER_COMPLETION_BRIDGE => {
                bridge::GetInstallerCompletionBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let completion = self.leaf(Arc::new(
                    super::installer_completion::InstallerCompletion::new(host.clone()),
                ));
                bridge::write_get_installer_completion_bridge_reply(&mut p, Some(completion));
                Ok(())
            }
            bridge::GET_INSTALLER_PREPARATION_BRIDGE => {
                bridge::GetInstallerPreparationBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let preparation = *self.installer_preparation.get_or_init(|| {
                    self.leaf(Arc::new(
                        super::installer_preparation::InstallerPreparation::new(host.clone()),
                    ))
                });
                bridge::write_get_installer_preparation_bridge_reply(&mut p, Some(preparation));
                Ok(())
            }
            // `Settings.Global.getInt(ephemeral_cookie_max_size_bytes,
            // 16384)`.
            bridge::GET_INSTANT_APP_COOKIE_LIMIT => {
                bridge::GetInstantAppCookieLimit::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.platform().map(|platform| {
                    let limit = platform
                        .settings
                        .get(Table::Global, "ephemeral_cookie_max_size_bytes")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(16384);
                    bridge::write_get_instant_app_cookie_limit_reply(&mut p, limit);
                })
            }
            // A new Bitmap's density: `Bitmap.getDefaultDensity`, the
            // process's `DisplayMetrics.DENSITY_DEVICE`.
            bridge::GET_INSTANT_APP_ICON_DENSITY => {
                bridge::GetInstantAppIconDensity::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.platform().map(|inputs| {
                    bridge::write_get_instant_app_icon_density_reply(&mut p, inputs.device_density);
                })
            }
            bridge::GET_APPLICATION_DATA_BRIDGE => {
                bridge::GetApplicationDataBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let data = *self.application_data.get_or_init(|| {
                    self.leaf(Arc::new(super::application_data::ApplicationData::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_application_data_bridge_reply(&mut p, Some(data));
                Ok(())
            }
            bridge::GET_PACKAGE_APEX_UNINSTALL_BRIDGE => {
                bridge::GetPackageApexUninstallBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let apex = self.leaf(Arc::new(
                    super::package_internal_leaves::ApexUninstall::new(host.clone()),
                ));
                bridge::write_get_package_apex_uninstall_bridge_reply(&mut p, Some(apex));
                Ok(())
            }
            bridge::GET_PERMISSION_PERSISTENCE_BRIDGE => {
                bridge::GetPermissionPersistenceBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let permission = self.leaf(Arc::new(
                    super::package_internal_leaves::PermissionPersistence::new(host.clone()),
                ));
                bridge::write_get_permission_persistence_bridge_reply(&mut p, Some(permission));
                Ok(())
            }
            bridge::GET_PACKAGE_INTERNAL_STORAGE_BRIDGE => {
                bridge::GetPackageInternalStorageBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let storage = *self.internal_storage.get_or_init(|| {
                    self.leaf(Arc::new(
                        super::package_internal_storage::PackageInternalStorage::new(host.clone()),
                    ))
                });
                bridge::write_get_package_internal_storage_bridge_reply(&mut p, Some(storage));
                Ok(())
            }
            bridge::GET_PACKAGE_MOVE_BRIDGE => {
                bridge::GetPackageMoveBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let moves = *self.package_moves.get_or_init(|| {
                    self.leaf(Arc::new(super::package_moves::PackageMove::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_package_move_bridge_reply(&mut p, Some(moves));
                Ok(())
            }
            bridge::GET_PACKAGE_RELOCATION_BRIDGE => {
                bridge::GetPackageRelocationBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let relocation = *self.package_relocation.get_or_init(|| {
                    self.leaf(Arc::new(super::package_moves::PackageRelocation::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_package_relocation_bridge_reply(&mut p, Some(relocation));
                Ok(())
            }
            bridge::GET_PACKAGE_LAUNCH_BRIDGE => {
                bridge::GetPackageLaunchBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let launch = self.leaf(Arc::new(super::package_launch::PackageLaunch));
                bridge::write_get_package_launch_bridge_reply(&mut p, Some(launch));
                Ok(())
            }
            bridge::GET_INSTALLER_REMOVAL_BRIDGE => {
                bridge::GetInstallerRemovalBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let removal = *self.installer_removal.get_or_init(|| {
                    self.leaf(Arc::new(super::installer_removal::InstallerRemoval::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_installer_removal_bridge_reply(&mut p, Some(removal));
                Ok(())
            }
            bridge::GET_INSTALLER_ARCHIVE_QUERY_BRIDGE => {
                bridge::GetInstallerArchiveQueryBridge::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let queries = *self.installer_archive_queries.get_or_init(|| {
                    self.leaf(Arc::new(
                        super::installer_archive_queries::InstallerArchiveQueries::new(
                            host.clone(),
                        ),
                    ))
                });
                bridge::write_get_installer_archive_query_bridge_reply(&mut p, Some(queries));
                Ok(())
            }
            bridge::GET_PACKAGE_ENABLE_BRIDGE => {
                bridge::GetPackageEnableBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let enable = self.leaf(Arc::new(super::package_enable::PackageEnable::new(
                    host.clone(),
                )));
                bridge::write_get_package_enable_bridge_reply(&mut p, Some(enable));
                Ok(())
            }
            bridge::GET_PACKAGE_DOMAIN_SETTINGS => {
                bridge::GetPackageDomainSettings::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let settings = *self.domain_settings.get_or_init(|| {
                    self.leaf(Arc::new(
                        super::package_domain_settings::DomainSettings::new(host.clone()),
                    ))
                });
                bridge::write_get_package_domain_settings_reply(&mut p, Some(settings));
                Ok(())
            }
            bridge::GET_PACKAGE_INSTALLER_FILES => {
                bridge::GetPackageInstallerFiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let files = *self.installer_files.get_or_init(|| {
                    self.leaf(Arc::new(super::installer_files::InstallerFiles::new(
                        host.clone(),
                    )))
                });
                bridge::write_get_package_installer_files_reply(&mut p, Some(files));
                Ok(())
            }
            bridge::GET_NATIVE_STAGING_BRIDGE => {
                bridge::GetNativeStagingBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let staging = self.leaf(Arc::new(super::native_staging::NativeStaging::new(
                    host.clone(),
                )));
                bridge::write_get_native_staging_bridge_reply(&mut p, Some(staging));
                Ok(())
            }
            bridge::GET_INSTALLER_POLICY_BRIDGE => {
                bridge::GetInstallerPolicyBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let policy = self.leaf(Arc::new(
                    super::package_installer_policy::InstallerPolicy::new(host.clone()),
                ));
                bridge::write_get_installer_policy_bridge_reply(&mut p, Some(policy));
                Ok(())
            }
            bridge::GET_INSTALLER_RECOVERY_PRESENTATION => {
                bridge::GetInstallerRecoveryPresentation::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let presentation = self.leaf(Arc::new(
                    super::installer_recovery::InstallerRecovery::new(host.clone()),
                ));
                bridge::write_get_installer_recovery_presentation_reply(&mut p, Some(presentation));
                Ok(())
            }
            bridge::GET_PACKAGE_INSTALLED_PERMISSIONS => {
                let a = bridge::GetPackageInstalledPermissions::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match a.package_name.filter(|n| !n.is_empty()) {
                    None => Err(Exception::illegal_argument("missing permission package")),
                    Some(name) => self
                        .permissions()
                        .and_then(|permissions| permissions.installed_permissions(&name))
                        .map(|names| {
                            let names = names.into_iter().map(Some).collect();
                            bridge::write_get_package_installed_permissions_reply(
                                &mut p,
                                &Some(names),
                            )
                        }),
                }
            }
            bridge::GET_PACKAGE_GRANTED_PERMISSIONS => {
                let a = bridge::GetPackageGrantedPermissions::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match a.package_name.filter(|n| !n.is_empty()) {
                    Some(name) if (0..100_000).contains(&a.app_id) && a.user_id >= 0 => self
                        .permissions()
                        .and_then(|permissions| {
                            permissions.granted_permissions(&name, a.app_id, a.user_id)
                        })
                        .map(|names| {
                            let names = names.into_iter().map(Some).collect();
                            bridge::write_get_package_granted_permissions_reply(
                                &mut p,
                                &Some(names),
                            )
                        }),
                    _ => Err(Exception::illegal_argument("invalid permission identity")),
                }
            }
            bridge::GET_LEGACY_PERMISSION_STATE => {
                let a = bridge::GetLegacyPermissionState::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let users = a.user_ids.unwrap_or_default();
                match super::permission_manager::validate_legacy_identity(a.app_id, &users)
                    .map(|()| host.permission_manager.get())
                {
                    Err(e) => Err(e),
                    Ok(None) => Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        "permission owner is unavailable",
                    )),
                    Ok(Some(permissions)) => self
                        .config()
                        .and_then(|config| {
                            permissions.legacy_permission_state(a.app_id, &users, config)
                        })
                        .map(|record| {
                            bridge::write_get_legacy_permission_state_reply(&mut p, &Some(record))
                        }),
                }
            }
            bridge::GET_PERMISSION_GIDS_FOR_UID => {
                let a =
                    bridge::GetPermissionGidsForUid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if a.uid < 0 {
                    Err(Exception::illegal_argument("negative permission UID"))
                } else {
                    match host.permission_manager.get() {
                        None => Err(Exception::new(
                            EX_ILLEGAL_STATE,
                            "permission owner is unavailable",
                        )),
                        Some(permissions) => self
                            .config()
                            .and_then(|config| permissions.gids_for_uid(a.uid, config))
                            .map(|gids| {
                                bridge::write_get_permission_gids_for_uid_reply(&mut p, &Some(gids))
                            }),
                    }
                }
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(host.unimplemented_named("package bootstrap", &format!("{name} (#2022)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
#[path = "package_bootstrap_existing_tests.rs"]
mod existing_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::bootstrap::Bridge;
    use crate::system::ServiceOwner;

    /// `deriveFingerprint` without `ro.build.fingerprint`, and the paths
    /// `restoreInstallerContext` labels.
    #[test]
    fn the_build_fingerprint_and_installer_paths_as_the_original() {
        let props = |name: &str| match name {
            "ro.product.brand" => Some("google".to_string()),
            "ro.product.name" => Some("sdk".into()),
            "ro.build.type" => Some("user".into()),
            _ => None,
        };
        assert_eq!(
            build_fingerprint(props),
            "google/sdk/unknown:unknown/unknown/unknown:user/unknown"
        );
        assert_eq!(
            build_fingerprint(|n| (n == "ro.build.fingerprint").then(|| "a/b/c".to_string())),
            "a/b/c"
        );
        for ok in [
            "/data/app/vmdl3.tmp",
            "/data/app-staging/session_4",
            "/data/system/install_sessions.xml.bak",
        ] {
            assert!(installer_context_path(ok), "{ok}");
        }
        for bad in [
            "/data/app/../x",
            "/data/app/.",
            "/data/app/a/..",
            "/data/system/packages.xml",
            "/data/app-x",
        ] {
            assert!(!installer_context_path(bad), "{bad}");
        }
    }

    /// The native boot's own bridge (`Bridge`) attached to this one as the
    /// process's node: its handshake and decisions, the users, a parsed
    /// package's decisions, the leaves, and a call not yet served.
    #[test]
    fn the_native_bridge_reads_the_app_hosts_bridge() {
        let Some(root) = aim_paths::original_image_with("system/etc/classpaths/bootclasspath.pb")
        else {
            return;
        };
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID as u32,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let data = std::env::temp_dir().join(format!("aim-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(data.join("system")).unwrap();
        host.configure(
            &root,
            &data,
            &data,
            &data,
            Box::new(|name| {
                Some(
                    match name {
                        "ro.build.version.sdk" => "36",
                        "ro.build.version.codename" => "REL",
                        "persist.sys.locale" => "en-US",
                        "ro.sf.lcd_density" => "320",
                        "ro.build.type" => "user",
                        "ro.debuggable" => "0",
                        _ => return None,
                    }
                    .into(),
                )
            }),
            Box::new(|_, _| {}),
            Box::new(|| Err(std::io::Error::other("no zygote"))),
        )
        .unwrap();
        super::super::user_service::ensure_system_user(&host).unwrap();
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(PackageBootstrap::new(host.clone())))
        else {
            unreachable!()
        };
        let owner = ServiceOwner::Local(process.local_service(ptr).unwrap());
        let bridge = Bridge::new(owner).unwrap();
        assert!(!bridge.signing_debuggable);
        assert_eq!(
            bridge.shared_uid_migration().unwrap(),
            SharedUidMigration::NewInstallOnly
        );
        let users = bridge.scan_users().unwrap().users.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].id, 0);
        let (a, b) = (
            bridge.new_domain_id().unwrap(),
            bridge.new_domain_id().unwrap(),
        );
        assert_ne!(a, b);
        assert_eq!(a[6] >> 4, 4);
        let package = AndroidPackage {
            package_name: "a.b".into(),
            target_sdk_version: 29,
            feature_flag_state: Some(Vec::new()),
            ..Default::default()
        };
        assert!(bridge.seinfo_target_sdk(&package).unwrap() >= 29);
        bridge.test_base_change(&package).unwrap();
        let call = |code: u32| {
            let mut p = Parcel::new();
            p.write_interface_token(bridge::DESCRIPTOR);
            if code == bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF
                || code == bridge::GET_INSTALLER_CONFIRMATION_BRIDGE
            {
                p.write_bool(false);
            }
            match &bridge.owner {
                ServiceOwner::Local(node) => node.transact(code, &p, false).unwrap(),
                ServiceOwner::Remote(_) => unreachable!(),
            }
        };
        for code in [
            bridge::GET_PACKAGE_INITIAL_CONTEXT_LEAF,
            bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF,
            bridge::GET_PACKAGE_BOOT_CONTEXT_LEAF,
            bridge::GET_PACKAGE_LIFECYCLE_LEAF,
            bridge::GET_PACKAGE_BOOT_LIFECYCLE_LEAF,
            bridge::GET_PACKAGE_MUTATION_BRIDGE,
            bridge::GET_INSTALLER_POLICY_BRIDGE,
            bridge::GET_INSTALLER_RECOVERY_PRESENTATION,
            bridge::GET_PACKAGE_MAINTENANCE_BRIDGE,
            bridge::GET_INSTALLER_EXTERNAL_BRIDGE,
            bridge::GET_INSTALLER_CONFIRMATION_BRIDGE,
            bridge::GET_NATIVE_STAGING_BRIDGE,
            bridge::GET_PACKAGE_INSTALLER_FILES,
            bridge::GET_PACKAGE_DOMAIN_SETTINGS,
            bridge::GET_PACKAGE_APP_DATA_BRIDGE,
            bridge::GET_INSTALLER_PERMISSION_BRIDGE,
            bridge::GET_PACKAGE_ENABLE_BRIDGE,
            bridge::GET_INSTALLER_COMPLETION_BRIDGE,
            bridge::GET_INSTALLER_PREPARATION_BRIDGE,
            bridge::GET_INSTALLER_REMOVAL_BRIDGE,
            bridge::GET_INSTALLER_ARCHIVE_QUERY_BRIDGE,
            bridge::GET_PACKAGE_LAUNCH_BRIDGE,
            bridge::GET_APPLICATION_DATA_BRIDGE,
            bridge::GET_PACKAGE_APEX_UNINSTALL_BRIDGE,
            bridge::GET_PERMISSION_PERSISTENCE_BRIDGE,
            bridge::GET_PACKAGE_MOVE_BRIDGE,
            bridge::GET_PACKAGE_RELOCATION_BRIDGE,
            bridge::GET_PACKAGE_INTERNAL_STORAGE_BRIDGE,
        ] {
            let reply = call(code);
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert!(matches!(
                r.read_binder().unwrap(),
                Some(aim_binder_host::parcel::Binder::Local(_))
            ));
        }
        let version = bridge.current_package_version().unwrap();
        assert_eq!((version.sdk_version, version.database_version), (36, 3));
        assert_eq!(
            version.build_fingerprint.as_deref(),
            Some("unknown/unknown/unknown:unknown/unknown/unknown:user/unknown")
        );
        assert_eq!(version.fingerprint.map(|f| f.len()), Some(40));
        let policy = bridge.library_policy("a.b", 36).unwrap();
        let framework = host.framework().unwrap();
        assert_eq!(
            policy.sdk_library_independence,
            framework.0.flags.get(SDK_LIB_INDEPENDENCE) == Some(&true)
        );
        let error = bridge
            .restore_installer_context("/data/app/../system")
            .unwrap_err();
        assert!(format!("{error:?}").contains("invalid installer context path"));
        // No agent has attached: the label cannot be restored.
        assert!(
            bridge
                .restore_installer_context("/data/app/vmdl1.tmp")
                .is_err()
        );
        let gids = |uid: i32| {
            let mut p = Parcel::new();
            bridge::GetPermissionGidsForUid { uid }.write(&mut p);
            let reply = match &bridge.owner {
                ServiceOwner::Local(node) => node
                    .transact(bridge::GET_PERMISSION_GIDS_FOR_UID, &p, false)
                    .unwrap(),
                ServiceOwner::Remote(_) => unreachable!(),
            };
            bridge::read_get_permission_gids_for_uid_reply(&mut reply.reader()).unwrap()
        };
        let e = gids(-1).unwrap_err();
        assert_eq!(e.message, "negative permission UID");
        // The services are not made here: no permission owner.
        let e = gids(10_100).unwrap_err();
        assert_eq!(e.message, "permission owner is unavailable");
        // platform.xml's `INTERNET`: `inet`.
        let bootstrap = PackageBootstrap::new(host.clone());
        let config = bootstrap.config().unwrap();
        assert_eq!(
            config.permissions["android.permission.INTERNET"].gids,
            [3003]
        );
        assert!(config.permission_error.is_none());
        // The resolution policy: user 0 and a user that does not exist are
        // of no one profile group, so INTERACT_ACROSS_PROFILES is not asked.
        let reply = call(bridge::GET_PACKAGE_RESOLUTION_POLICY);
        let Some(aim_binder_host::parcel::Binder::Local(ptr)) =
            bridge::read_get_package_resolution_policy_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        else {
            panic!("the policy is this process's");
        };
        let resolution = host.process.local_service(ptr).unwrap();
        use aim_service_aidl::dev_aim_server_ipackageresolutionpolicy as policy;
        let mut p = Parcel::new();
        policy::ProfilePermission {
            calling_uid: 10_100,
            target_user: 10,
            package_name: Some("a.b".into()),
        }
        .write(&mut p);
        let reply = resolution
            .transact(policy::PROFILE_PERMISSION, &p, false)
            .unwrap();
        assert!(
            !policy::read_profile_permission_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let mut p = Parcel::new();
        policy::IsKnownIsolatedComputeApp { uid: 99_000 }.write(&mut p);
        let reply = resolution
            .transact(policy::IS_KNOWN_ISOLATED_COMPUTE_APP, &p, false)
            .unwrap();
        assert!(
            !policy::read_is_known_isolated_compute_app_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        assert!(super::super::user_service::same_profile_group(&host, 0, 0).unwrap());
        // Domain verification's changes, from the compat config; an empty
        // name is refused.
        let compat = host.platform().unwrap();
        let (code, request) =
            crate::package::bootstrap::domain_verification_restricted_call("a.b", 36);
        let reply = match &bridge.owner {
            ServiceOwner::Local(node) => node.transact(code, &request, false).unwrap(),
            ServiceOwner::Remote(_) => unreachable!(),
        };
        assert_eq!(
            bridge::read_is_domain_verification_restricted_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            compat
                .compat
                .is_enabled(DOMAIN_VERIFICATION_RESTRICTED, Some("a.b"), 36)
        );
        let (code, request) =
            crate::package::bootstrap::domain_verification_restricted_call("", 36);
        let reply = match &bridge.owner {
            ServiceOwner::Local(node) => node.transact(code, &request, false).unwrap(),
            ServiceOwner::Remote(_) => unreachable!(),
        };
        assert_eq!(
            bridge::read_is_domain_verification_restricted_reply(&mut reply.reader())
                .unwrap()
                .unwrap_err()
                .message,
            "invalid domain compatibility identity"
        );
        // Query filtering from R.
        assert!(bridge.application_query_filtering("a.b", 36).unwrap());
        assert!(!bridge.application_query_filtering("a.b", 29).unwrap());
        // A batch: each call answered in its own reply, a failing one with
        // its exception, in order.
        let (gids_code, gids) = crate::package::bootstrap::permission_gids_call(-1);
        let mut debuggable = Parcel::new();
        bridge::IsSigningDebuggable {}.write(&mut debuggable);
        let mut calls = Parcel::new();
        calls.write_i32(2);
        for (code, data) in [
            (gids_code, &gids),
            (bridge::IS_SIGNING_DEBUGGABLE, &debuggable),
        ] {
            calls.write_i32(code as i32);
            aim_service_aidl::write_byte_array(&mut calls, Some(data.data()));
        }
        let mut p = Parcel::new();
        bridge::TransactBatch {
            calls: Some(calls.data().to_vec()),
        }
        .write(&mut p);
        let reply = match &bridge.owner {
            ServiceOwner::Local(node) => node.transact(bridge::TRANSACT_BATCH, &p, false).unwrap(),
            ServiceOwner::Remote(_) => unreachable!(),
        };
        let bytes = bridge::read_transact_batch_reply(&mut reply.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        assert_eq!(r.read_i32().unwrap(), 2);
        let first = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
        let e = bridge::read_get_permission_gids_for_uid_reply(
            &mut aim_binder_host::parcel::Reader::new(&first, &[]),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(e.message, "negative permission UID");
        let second = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
        assert!(
            !bridge::read_is_signing_debuggable_reply(&mut aim_binder_host::parcel::Reader::new(
                &second,
                &[]
            ))
            .unwrap()
            .unwrap()
        );
        // From U, with no app compat callbacks in the system process.
        let reply = call(bridge::IS_DOMAIN_SET_UUID_STRICT_VALIDATION_ENABLED);
        assert!(
            bridge::read_is_domain_set_uuid_strict_validation_enabled_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let reply = call(bridge::GET_PACKAGE_SHUTDOWN_BRIDGE);
        let e = reply.reader().read_exception().unwrap().unwrap_err();
        assert!(e.message.contains("#2022"), "{}", e.message);
        std::fs::remove_dir_all(&data).unwrap();
    }

    /// The bridge uses its argument's caller, not the system transport
    /// uid, and refuses a non-system transport before resolving any user.
    #[test]
    fn package_monitor_user_uses_the_activity_owner() {
        let node = |euid| {
            let process = aim_binder_host::local::LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid,
                    security_context: None,
                },
            );
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(PackageBootstrap::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        for (uid, user, expected) in [
            (1000, -1, -1),
            (0, -2, 0),
            (1000, -3, 0),
            (10_123, 0, 0),
            (1_010_123, 10, 10),
        ] {
            let mut p = Parcel::new();
            bridge::PackageMonitorUser {
                calling_pid: 42,
                calling_uid: uid,
                user_id: user,
            }
            .write(&mut p);
            let answer = node(1000)
                .transact(bridge::PACKAGE_MONITOR_USER, &p, false)
                .unwrap();
            assert_eq!(
                bridge::read_package_monitor_user_reply(&mut answer.reader()).unwrap(),
                Ok(expected)
            );
            let denied = node(10_123)
                .transact(bridge::PACKAGE_MONITOR_USER, &p, false)
                .unwrap();
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
        }
    }

    /// The package caches' invalidation is answered, the system's only.
    #[test]
    fn package_caches_are_invalidated() {
        let node = |euid: u32| {
            let process = aim_binder_host::local::LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid,
                    security_context: None,
                },
            );
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(PackageBootstrap::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let invalidate = |euid: u32| {
            let mut p = Parcel::new();
            bridge::InvalidatePackageInfoCache {}.write(&mut p);
            let reply = node(euid)
                .transact(bridge::INVALIDATE_PACKAGE_INFO_CACHE, &p, false)
                .unwrap();
            bridge::read_invalidate_package_info_cache_reply(&mut reply.reader()).unwrap()
        };
        assert!(invalidate(SYSTEM_UID as u32).is_ok());
        assert_eq!(
            invalidate(10000).unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
    }
}
