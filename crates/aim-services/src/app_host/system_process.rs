//! The system process's part of system_server (`android-16.0.0_r1`),
//! registered as `aim.system_process`: `aim-system`, a process of the
//! system uid that runs `ActivityThread.systemMain` as system_server does
//! (java/device-services, SystemProcessHost), attaches its application
//! thread here as `ActivityManagerService.setSystemProcess` makes
//! system_server's the system process's, and gets TelecomLoaderService's
//! `InternalServiceRepository`, whose DeviceIdleInternal is the app
//! host's.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::com_android_internal_telecom_ideviceidlecontrolleradapter as idle;
use aim_service_aidl::com_android_internal_telecom_iinternalserviceretriever as retriever;
use aim_service_aidl::dev_aim_server_isystemprocess as sp;

use super::{AppHost, SYSTEM_UID, reply};

/// `PowerExemptionManager.REASON_UNKNOWN`.
const REASON_UNKNOWN: i32 = 0;

pub struct SystemProcess {
    host: Arc<AppHost>,
    /// The repository, made once (`setupServiceRepository`).
    repository: OnceLock<Binder>,
}

impl SystemProcess {
    pub fn new(host: Arc<AppHost>) -> Self {
        SystemProcess {
            host,
            repository: OnceLock::new(),
        }
    }
}

impl Service for SystemProcess {
    fn descriptor(&self) -> &str {
        sp::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        // System_server's own process.
        let system = || {
            if call.sender_euid != SYSTEM_UID {
                return Err(Exception::security(format!(
                    "{} is not the system uid",
                    call.sender_euid
                )));
            }
            Ok(())
        };
        let result = match call.code {
            sp::GET_SYNC_ADAPTER_STANDBY_BUCKET => {
                let a = sp::GetSyncAdapterStandbyBucket::read(&mut call.data)?;
                system().and_then(|()| {
                    let package = a
                        .package_name
                        .as_deref()
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
                    let bucket = self.host.app_standby_bucket(package, a.user_id)?;
                    sp::write_get_sync_adapter_standby_bucket_reply(&mut p, bucket);
                    Ok(())
                })
            }
            sp::REPORT_SYNC_SCHEDULED => {
                let a = sp::ReportSyncScheduled::read(&mut call.data)?;
                system().and_then(|()| {
                    let package = a
                        .package_name
                        .as_deref()
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
                    self.host
                        .report_sync_usage(package, a.user_id, a.exempted, false)?;
                    sp::write_report_sync_scheduled_reply(&mut p);
                    Ok(())
                })
            }
            sp::REPORT_EXEMPTED_SYNC_START => {
                let a = sp::ReportExemptedSyncStart::read(&mut call.data)?;
                system().and_then(|()| {
                    let package = a
                        .package_name
                        .as_deref()
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
                    self.host
                        .report_sync_usage(package, a.user_id, true, true)?;
                    sp::write_report_exempted_sync_start_reply(&mut p);
                    Ok(())
                })
            }
            sp::ADD_SYNC_TEMP_ALLOWLIST => {
                let a = sp::AddSyncTempAllowlist::read(&mut call.data)?;
                system().and_then(|()| {
                    if a.sync || !(0..=1).contains(&a.r#type) {
                        return Err(self.host.unimplemented_named(
                            "deviceidle",
                            "local allowlist sync/type (#2016)",
                        ));
                    }
                    let kind = super::device_idle::DeviceIdle::temp_allowlist_type(
                        a.reason_code,
                        a.r#type,
                    )
                    .map_err(|e| self.host.unimplemented_named("deviceidle", &e))?;
                    if kind != -1 {
                        self.host.device_idle().add_temp(
                            a.calling_uid,
                            a.package_name.as_deref(),
                            a.duration_ms,
                            kind,
                            a.user_id,
                            a.reason_code,
                            a.reason,
                        )?;
                    }
                    sp::write_add_sync_temp_allowlist_reply(&mut p);
                    Ok(())
                })
            }
            sp::GET_SYSTEM_SCHEDULED_OWN_JOBS => {
                let a = sp::GetSystemScheduledOwnJobs::read(&mut call.data)?;
                system().and_then(|()| {
                    let jobs = self
                        .host
                        .job_scheduler
                        .get_or_init(|| super::job_scheduler::JobScheduler::new(self.host.clone()));
                    let own = jobs.system_scheduled_own_jobs(a.namespace.as_deref())?;
                    let own: Vec<_> = own.into_iter().map(Some).collect();
                    sp::write_get_system_scheduled_own_jobs_reply(&mut p, Some(&own));
                    Ok(())
                })
            }
            sp::GET_JOB_PERSIST_STATS => {
                sp::GetJobPersistStats::read(&mut call.data)?;
                system().and_then(|()| {
                    let jobs = self
                        .host
                        .job_scheduler
                        .get_or_init(|| super::job_scheduler::JobScheduler::new(self.host.clone()));
                    sp::write_get_job_persist_stats_reply(
                        &mut p,
                        &Some(jobs.persist_stats()?.to_vec()),
                    );
                    Ok(())
                })
            }
            sp::GET_SYNC_ACCOUNTS => {
                sp::GetSyncAccounts::read(&mut call.data)?;
                system().map(|()| {
                    let owner = self
                        .host
                        .account_manager
                        .get_or_init(|| {
                            super::account_service::AccountManager::new(self.host.clone())
                        })
                        .clone();
                    let binder = self
                        .host
                        .process
                        .add_service(Arc::new(super::account_service::SyncAccounts(owner)));
                    sp::write_get_sync_accounts_reply(&mut p, Some(binder));
                })
            }
            sp::GET_PACKAGE_INTERNAL_HOST => {
                sp::GetPackageInternalHost::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let owner = self.host.package_system()?.package_internal_host()?;
                    sp::write_get_package_internal_host_reply(&mut p, Some(owner));
                    Ok(())
                })
            }
            sp::ATTACH_ROLE_SERVICE => {
                let a = sp::AttachRoleService::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let Some(Binder::Handle(handle)) = a.service else {
                        return Err(Exception::illegal_argument("role service not published"));
                    };
                    *self.host.original_role.lock().unwrap() =
                        Some(Arc::new(self.host.process.strong(handle)));
                    sp::write_attach_role_service_reply(&mut p);
                    Ok(())
                })
            }
            sp::GET_ACCESSIBILITY_WINDOW_HOST => {
                sp::GetAccessibilityWindowHost::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().map(|()| {
                    let binder = self
                        .host
                        .process
                        .add_service(super::window_manager::accessibility_host(self.host.clone()));
                    sp::write_get_accessibility_window_host_reply(&mut p, Some(binder));
                })
            }
            sp::ACCESSIBILITY_READY => {
                let a = sp::AccessibilityReady::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    if !self
                        .host
                        .apps
                        .lock()
                        .unwrap()
                        .values()
                        .any(|a| a.process == super::SYSTEM_PROCESS && a.pid == call.sender_pid)
                    {
                        return Err(Exception::security(
                            "accessibility publication belongs to the attached system process",
                        ));
                    }
                    let Some(Binder::Handle(handle)) = a.service else {
                        return Err(Exception::illegal_argument("accessibility not published"));
                    };
                    let remote = Arc::new(self.host.process.strong(handle));
                    let weak = Arc::downgrade(&self.host);
                    self.host.accessibility_ready.publish(remote, |remote| {
                        self.host.process.link_to_death(
                            remote,
                            Box::new(move || {
                                if let Some(host) = weak.upgrade() {
                                    host.accessibility_ready
                                        .failed("accessibility service died (#2529)");
                                }
                            }),
                        );
                    });
                    sp::write_accessibility_ready_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_ACTIVITY_ANIMATIONS => {
                let a =
                    sp::AttachActivityAnimations::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let Some(Binder::Handle(handle)) = a.animations else {
                        return Err(Exception::illegal_argument("no activity animations owner"));
                    };
                    self.host.windows.transitions.lock().unwrap().owner =
                        Some(Arc::new(self.host.process.strong(handle)));
                    sp::write_attach_activity_animations_reply(&mut p);
                    Ok(())
                })
            }
            sp::WRITE_PACKAGE_COMPILER_STATS => {
                sp::WritePackageCompilerStats::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let capture = self.host.package_capture()?;
                    let owner = capture
                        .state()
                        .system
                        .maintenance_diagnostics
                        .as_ref()
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "native compiler statistics owner unavailable",
                            )
                        })?;
                    owner.compiler.write_now();
                    sp::write_write_package_compiler_stats_reply(&mut p);
                    Ok(())
                })
            }
            sp::RESET_PACKAGE_RUNTIME_PERMISSIONS_FOR_USER => {
                let a = sp::ResetPackageRuntimePermissionsForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host
                        .permission_manager
                        .get()
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "permission owner is unavailable",
                            )
                        })?
                        .reset_runtime_permissions_for_user(a.user_id)?;
                    sp::write_reset_package_runtime_permissions_for_user_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_PROVIDER_QUERIES => {
                let a = sp::AttachProviderQueries::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_provider_queries(a.queries)?;
                    sp::write_attach_provider_queries_reply(&mut p);
                    Ok(())
                })
            }
            sp::EXISTING_PACKAGE_PERMISSIONS_INSTALLED => {
                let a = sp::ExistingPackagePermissionsInstalled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let name = a.package_name.as_deref().ok_or_else(|| {
                        Exception::illegal_argument("missing existing permission package")
                    })?;
                    self.host
                        .permission_manager
                        .get()
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "permission owner is unavailable",
                            )
                        })?
                        .existing_package_installed(name, a.allowlisted.as_deref(), a.user_id)?;
                    sp::write_existing_package_permissions_installed_reply(&mut p);
                    Ok(())
                })
            }
            sp::IS_PACKAGE_CE_STORAGE_PREPARED => {
                let a =
                    sp::IsPackageCeStoragePrepared::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().map(|()| {
                    sp::write_is_package_ce_storage_prepared_reply(
                        &mut p,
                        self.host.ce_storage_prepared(a.user_id),
                    )
                })
            }
            sp::ATTACH_PACKAGE_BOOTSTRAP_JAVA => {
                let a =
                    sp::AttachPackageBootstrapJava::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_package_bootstrap_java(a.bridge)?;
                    sp::write_attach_package_bootstrap_java_reply(&mut p);
                    Ok(())
                })
            }
            sp::GET_ACTIVITY_STATS_HOST => {
                let a = sp::GetActivityStatsHost::read(&mut call.data)?;
                system()
                    .and_then(|()| self.host.activity_stats_host(a.snapshot))
                    .map(|backend| {
                        sp::write_get_activity_stats_host_reply(&mut p, Some(backend));
                    })
            }
            sp::ATTACH_STORAGE_CRATES => {
                let a = sp::AttachStorageCrates::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_storage_crates(a.crates)?;
                    sp::write_attach_storage_crates_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_NATIVE_STAGING_BRIDGE => {
                let a =
                    sp::AttachNativeStagingBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_native_staging_bridge(a.bridge)?;
                    sp::write_attach_native_staging_bridge_reply(&mut p);
                    Ok(())
                })
            }
            sp::INITIALIZE_CAMERA_PRIVACY_ALLOWLIST => {
                let a = sp::InitializeCameraPrivacyAllowlist::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let packages = a
                        .packages
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packages"))?;
                    self.host
                        .camera_privacy_allowlist
                        .lock()
                        .unwrap()
                        .get_or_insert(packages);
                    sp::write_initialize_camera_privacy_allowlist_reply(&mut p);
                    Ok(())
                })
            }
            sp::GET_DEX_USE_PACKAGES => {
                sp::GetDexUsePackages::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let owner = self.host.package_system()?;
                    let captures = crate::package::boot_captures::Captures::new(
                        owner,
                        owner.package_bootstrap()?,
                    );
                    let backend = self.host.process.add_service(Arc::new(
                        super::package_maintenance::DexUsePackages(captures),
                    ));
                    sp::write_get_dex_use_packages_reply(&mut p, Some(backend));
                    Ok(())
                })
            }
            sp::ATTACH_DEX_USE_BRIDGE => {
                let a = sp::AttachDexUseBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let owner = match a.bridge {
                        Some(Binder::Handle(handle)) => crate::system::ServiceOwner::Remote(
                            Arc::new(self.host.process.strong(handle)),
                        ),
                        Some(Binder::Local(ptr)) => crate::system::ServiceOwner::Local(
                            self.host.process.local_service(ptr).ok_or_else(|| {
                                Exception::new(EX_NULL_POINTER, "dex-use bridge gone")
                            })?,
                        ),
                        None => return Err(Exception::new(EX_NULL_POINTER, "dex-use bridge")),
                    };
                    *self.host.dex_use_bridge.lock().unwrap() = Some(Arc::new(owner));
                    sp::write_attach_dex_use_bridge_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_WIFI_BRIDGE => {
                let a = sp::AttachWifiBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host
                        .wifi_service
                        .get()
                        .ok_or_else(|| {
                            self.host
                                .unimplemented_named("wifi", "not published (#2261)")
                        })?
                        .attach_bridge(a.bridge)
                        .map(|()| sp::write_attach_wifi_bridge_reply(&mut p))
                })
            }
            sp::ATTACH_USER_SWITCHER_POLICY => {
                let a =
                    sp::AttachUserSwitcherPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_user_switcher_policy(a.policy)?;
                    sp::write_attach_user_switcher_policy_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_DEVICE_POLICY_DEFAULTS => {
                let a =
                    sp::AttachDevicePolicyDefaults::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_device_policy_defaults(a.defaults)?;
                    sp::write_attach_device_policy_defaults_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_SYSTEM_PROCESS => {
                let a = sp::AttachSystemProcess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host
                        .battery_stats
                        .get_or_init(|| {
                            Arc::new(super::battery_stats::BatteryStats::new(&self.host))
                        })
                        .attach_health(a.battery_health_stats)?;
                    self.host
                        .attach_foreground_service_policy(a.foreground_service_policy)?;
                    self.host.attach_safe_labels(a.safe_labels)?;
                    self.host
                        .attach_system_process(call, a.application_thread)
                        .map(|()| sp::write_attach_system_process_reply(&mut p))
                })
            }
            sp::ATTACH_PACKAGE_INTENT_PARSER => {
                let a =
                    sp::AttachPackageIntentParser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let owner = match a.parser {
                        Some(Binder::Handle(h)) => crate::system::ServiceOwner::Remote(Arc::new(
                            self.host.process.strong(h),
                        )),
                        Some(Binder::Local(ptr)) => crate::system::ServiceOwner::Local(
                            self.host.process.local_service(ptr).ok_or_else(|| {
                                Exception::new(EX_NULL_POINTER, "package Intent parser gone")
                            })?,
                        ),
                        None => {
                            return Err(Exception::new(EX_NULL_POINTER, "package Intent parser"));
                        }
                    };
                    *self.host.package_intent_parser.lock().unwrap() = Some(Arc::new(owner));
                    sp::write_attach_package_intent_parser_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_PACKAGE_SHELL_READ => {
                let a = sp::AttachPackageShellRead::read(&mut call.data)?;
                system().and_then(|()| {
                    self.host.attach_package_shell_read(a.owner)?;
                    sp::write_attach_package_shell_read_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_SETTINGS_VALUES => {
                let a = sp::AttachSettingsValues::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.settings_provider()?.attach_values(a.bridge)?;
                    sp::write_attach_settings_values_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_STATS_COMPANION_UID_MAP => {
                let a =
                    sp::AttachStatsCompanionUidMap::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host
                        .stats
                        .get()
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "statscompanion is not published",
                            )
                        })?
                        .attach_uid_map(a.uid_map)
                        .map(|()| sp::write_attach_stats_companion_uid_map_reply(&mut p))
                })
            }
            sp::ATTACH_RECOVERABLE_KEY_STORE_KEYS => {
                let a = sp::AttachRecoverableKeyStoreKeys::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host.attach_recoverable_keys(a.keys)?;
                    sp::write_attach_recoverable_key_store_keys_reply(&mut p);
                    Ok(())
                })
            }
            sp::GET_DOMAIN_VERIFICATION_BACKEND => {
                sp::GetDomainVerificationBackend::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().map(|()| {
                    let backend = self.host.process.add_service(Arc::new(
                        super::domain_verification::Backend::new(self.host.clone()),
                    ));
                    sp::write_get_domain_verification_backend_reply(&mut p, Some(backend));
                })
            }
            sp::ATTACH_DOMAIN_VERIFICATION => {
                let a =
                    sp::AttachDomainVerification::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let frontend = a
                        .frontend
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "frontend"))?;
                    let owner = match frontend {
                        Binder::Handle(handle) => crate::system::ServiceOwner::Remote(Arc::new(
                            self.host.process.strong(handle),
                        )),
                        Binder::Local(ptr) => crate::system::ServiceOwner::Local(
                            self.host
                                .process
                                .local_service(ptr)
                                .ok_or_else(|| Exception::new(EX_NULL_POINTER, "frontend gone"))?,
                        ),
                    };
                    *self.host.domain_frontend.lock().unwrap() = Some(Arc::new(owner));
                    sp::write_attach_domain_verification_reply(&mut p);
                    Ok(())
                })
            }
            sp::ATTACH_SYSTEM_ROUTE_QUERIES
            | sp::ATTACH_AUDIO_POLICY_QUERIES
            | sp::ATTACH_SYNC_ADAPTER_QUERIES => {
                let (queries, slot) = match call.code {
                    sp::ATTACH_AUDIO_POLICY_QUERIES => (
                        sp::AttachAudioPolicyQueries::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?
                            .queries,
                        &self.host.audio_policy_queries,
                    ),
                    sp::ATTACH_SYNC_ADAPTER_QUERIES => (
                        sp::AttachSyncAdapterQueries::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?
                            .queries,
                        &self.host.sync_adapter_queries,
                    ),
                    _ => (
                        sp::AttachSystemRouteQueries::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?
                            .queries,
                        &self.host.system_route_queries,
                    ),
                };
                system().and_then(|()| {
                    let owner = match queries {
                        Some(Binder::Handle(h)) => crate::system::ServiceOwner::Remote(Arc::new(
                            self.host.process.strong(h),
                        )),
                        Some(Binder::Local(ptr)) => crate::system::ServiceOwner::Local(
                            self.host
                                .process
                                .local_service(ptr)
                                .ok_or_else(|| Exception::new(EX_NULL_POINTER, "queries gone"))?,
                        ),
                        None => {
                            return Err(Exception::new(EX_NULL_POINTER, "queries"));
                        }
                    };
                    *slot.lock().unwrap() = Some(Arc::new(owner));
                    p.write_no_exception();
                    Ok(())
                })
            }
            sp::FILTER_SYNC_ADAPTER_PACKAGE_ACCESS => {
                let a = sp::FilterSyncAdapterPackageAccess::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let name = a
                        .package_name
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
                    let filtered = self.host.filter_package_access(&name, a.uid, a.user_id)?;
                    sp::write_filter_sync_adapter_package_access_reply(&mut p, filtered);
                    Ok(())
                })
            }
            sp::IS_DOMAIN_VERIFIER => {
                let a = sp::IsDomainVerifier::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let capture = self.host.package_scan_capture()?;
                    let allowed =
                        super::domain_verification::DomainVerification::new(self.host.clone())
                            .is_caller_verifier(&capture, a.uid)?;
                    sp::write_is_domain_verifier_reply(&mut p, allowed);
                    Ok(())
                })
            }
            sp::GET_INTERNAL_SERVICE_REPOSITORY => {
                sp::GetInternalServiceRepository::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().map(|()| {
                    let repository = self.repository.get_or_init(|| {
                        self.host.process.add_service(Arc::new(Repository {
                            host: self.host.clone(),
                            adapter: OnceLock::new(),
                        }))
                    });
                    sp::write_get_internal_service_repository_reply(&mut p, Some(*repository));
                })
            }
            sp::ATTACH_NETWORK_ACTIVITY_BRIDGE => {
                let a =
                    sp::AttachNetworkActivityBridge::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let Some(Binder::Handle(handle)) = a.bridge else {
                        return Err(Exception::illegal_argument(
                            "network activity bridge missing",
                        ));
                    };
                    let connectivity = self.host.connectivity_service.get().ok_or_else(|| {
                        self.host
                            .unimplemented_named("connectivity", "not published (#1509)")
                    })?;
                    connectivity.attach_activity(self.host.process.strong(handle))?;
                    sp::write_attach_network_activity_bridge_reply(&mut p);
                    Ok(())
                })
            }
            sp::RELEASE_CONNECTIVITY_DIAGNOSTICS_REQUEST => {
                let a = sp::ReleaseConnectivityDiagnosticsRequest::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    let connectivity = self.host.connectivity_service.get().ok_or_else(|| {
                        self.host
                            .unimplemented_named("connectivity", "not published (#1509)")
                    })?;
                    connectivity.release_diagnostics(a.request_id);
                    sp::write_release_connectivity_diagnostics_request_reply(&mut p);
                    Ok(())
                })
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

/// `InternalServiceRepository`: the system's internal services for
/// Telecom.
struct Repository {
    host: Arc<AppHost>,
    /// `mDeviceIdleControllerAdapter`.
    adapter: OnceLock<Binder>,
}

impl Service for Repository {
    fn descriptor(&self) -> &str {
        retriever::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            retriever::GET_DEVICE_IDLE_CONTROLLER => {
                retriever::GetDeviceIdleController::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // `ensureSystemProcess`.
                if call.sender_euid != SYSTEM_UID {
                    Err(Exception::security("SYSTEM ONLY API."))
                } else {
                    let adapter = self.adapter.get_or_init(|| {
                        self.host
                            .process
                            .add_service(Arc::new(IdleAdapter(self.host.clone())))
                    });
                    retriever::write_get_device_idle_controller_reply(&mut p, Some(*adapter));
                    Ok(())
                }
            }
            code => {
                let name = retriever::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("telecom", name))
            }
        };
        reply(result.map(|()| p))
    }
}

/// `mDeviceIdleControllerAdapter`.
struct IdleAdapter(Arc<AppHost>);

impl Service for IdleAdapter {
    fn descriptor(&self) -> &str {
        idle::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            idle::EXEMPT_APP_TEMPORARILY_FOR_EVENT => {
                let a = idle::ExemptAppTemporarilyForEvent::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                // `DeviceIdleInternal.addPowerSaveTempWhitelistApp(myUid,
                // ...)`: foreground service starts allowed for the duration.
                match a.package_name.as_deref() {
                    Some(package) => self
                        .0
                        .device_idle()
                        .add_temp(
                            SYSTEM_UID as i32,
                            Some(package),
                            a.duration,
                            super::device_idle::TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED,
                            a.user_handle,
                            REASON_UNKNOWN,
                            a.reason,
                        )
                        .map(|()| idle::write_exempt_app_temporarily_for_event_reply(&mut p)),
                    // `getPackageUidAsUser(null, ..)`.
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                }
            }
            code => {
                let name = idle::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.0.unimplemented_named("telecom", name))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_policy_routes_require_system_identity_and_propagate_owner_errors() {
        let owner = SystemProcess::new(AppHost::new(crate::test_process(SYSTEM_UID)));
        let mut requests = Vec::new();
        let mut p = Parcel::new();
        sp::GetSyncAdapterStandbyBucket {
            package_name: Some("p".into()),
            user_id: 0,
        }
        .write(&mut p);
        requests.push((sp::GET_SYNC_ADAPTER_STANDBY_BUCKET, p));
        let mut p = Parcel::new();
        sp::ReportSyncScheduled {
            package_name: Some("p".into()),
            user_id: 0,
            exempted: true,
        }
        .write(&mut p);
        requests.push((sp::REPORT_SYNC_SCHEDULED, p));
        let mut p = Parcel::new();
        sp::ReportExemptedSyncStart {
            package_name: Some("p".into()),
            user_id: 0,
        }
        .write(&mut p);
        requests.push((sp::REPORT_EXEMPTED_SYNC_START, p));
        let mut p = Parcel::new();
        sp::AddSyncTempAllowlist {
            calling_uid: 1000,
            package_name: Some("p".into()),
            duration_ms: 100,
            r#type: 1,
            user_id: 0,
            sync: false,
            reason_code: 306,
            reason: Some("sync by top app".into()),
        }
        .write(&mut p);
        requests.push((sp::ADD_SYNC_TEMP_ALLOWLIST, p));
        for (code, p) in requests {
            for uid in [10100, SYSTEM_UID] {
                let reply = owner
                    .transact(&mut Call {
                        code,
                        flags: 0,
                        sender_pid: 1,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap();
                let e = reply.reader().read_exception().unwrap().unwrap_err();
                if uid != SYSTEM_UID {
                    assert_eq!(e.code, aim_binder_host::parcel::EX_SECURITY);
                } else {
                    assert_ne!(e.code, aim_binder_host::parcel::EX_SECURITY);
                }
            }
        }
    }

    #[test]
    fn uid_map_attachment_requires_the_system_uid_and_a_remote_bridge() {
        let host = AppHost::new(crate::test_process(SYSTEM_UID));
        let stats = super::super::stats_manager::Stats::new(host.clone());
        assert!(host.stats.set(stats).is_ok());
        let service = SystemProcess::new(host);
        let mut request = Parcel::new();
        sp::AttachStatsCompanionUidMap { uid_map: None }.write(&mut request);
        for (uid, expected) in [
            (10123, aim_binder_host::parcel::EX_SECURITY),
            (SYSTEM_UID, aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT),
        ] {
            let response = service
                .transact(&mut Call {
                    code: sp::ATTACH_STATS_COMPANION_UID_MAP,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: uid,
                    data: request.reader(),
                })
                .unwrap();
            let error = sp::read_attach_stats_companion_uid_map_reply(&mut response.reader())
                .unwrap()
                .unwrap_err();
            assert_eq!(error.code, expected);
        }
    }
}
