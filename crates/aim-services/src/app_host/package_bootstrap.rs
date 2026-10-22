//! `IPackageBootstrapBridge` for the app host: the outside decisions and
//! the leaves the native package boot reads, answered as system_server's
//! `PackageBootstrapBridge` answers them (`android-16.0.0_r1`), from the
//! app host's own owners (`boot_policy`'s decisions, the boot leaves, the
//! current package version, the native-library and SDK-library policies,
//! the installer's SELinux labels through the agent, the permission gids
//! of a uid, the package writer's effects, the installer's build flags and
//! its sessions' icon decoder) for the native package boot it runs
//! (served as its own node). The bridge's other owners, its other
//! sub-bridges among them, are not implemented (#2022).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;

use super::boot_policy;
use super::{AppHost, reply};
use crate::package::bootstrap::ScanUsers;
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
    /// ApexManager's `mApexSystemServices`' names, kept across its
    /// notifications.
    apex_system_services: Mutex<Vec<String>>,
    /// `SystemConfig.getInstance()`, read once.
    config: OnceLock<SystemConfig>,
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
        PackageBootstrap {
            host,
            apex_system_services: Mutex::new(Vec::new()),
            config: OnceLock::new(),
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

impl Service for PackageBootstrap {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
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
            bridge::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                bridge::IsTestBaseOnBootclasspath::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.root()
                    .and_then(|root| {
                        boot_policy::test_base_on_bootclasspath(root)
                            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
                    })
                    .map(|v| bridge::write_is_test_base_on_bootclasspath_reply(&mut p, v))
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
            if code == bridge::GET_PACKAGE_BOOT_CONFIGURATION_LEAF {
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
        let reply = call(bridge::GET_PACKAGE_POLICY_BRIDGE);
        let e = reply.reader().read_exception().unwrap().unwrap_err();
        assert!(e.message.contains("#2022"), "{}", e.message);
        std::fs::remove_dir_all(&data).unwrap();
    }
}
