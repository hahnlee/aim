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
                    let capture = self.host.package_capture()?;
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
