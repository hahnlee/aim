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
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::com_android_internal_telecom_ideviceidlecontrolleradapter as idle;
use aim_service_aidl::com_android_internal_telecom_iinternalserviceretriever as retriever;
use aim_service_aidl::dev_aim_server_isystemprocess as sp;

use super::{AppHost, PER_USER_RANGE, SYSTEM_UID, reply};

/// `DeviceIdleController.Constants.MAX_TEMP_APP_ALLOWLIST_DURATION_MS`.
const MAX_TEMP_APP_ALLOWLIST_DURATION: Duration = Duration::from_secs(5 * 60);
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
            sp::ATTACH_SYSTEM_PROCESS => {
                let a = sp::AttachSystemProcess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                system().and_then(|()| {
                    self.host
                        .attach_system_process(call, a.application_thread)
                        .map(|()| sp::write_attach_system_process_reply(&mut p))
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

impl IdleAdapter {
    /// `DeviceIdleInternal.addPowerSaveTempWhitelistApp(myUid, ...)` with
    /// `TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED`: the
    /// package's uid allowed foreground service starts for the duration,
    /// at most `MAX_TEMP_APP_ALLOWLIST_DURATION_MS`
    /// (`updateDeviceIdleTempAllowlist`); the app host is never idle, so
    /// the idle exemption itself has no effect. An unknown package is
    /// ignored (`NameNotFoundException`).
    fn exempt(&self, package: &str, duration_ms: i64, user: i32) -> Result<(), Exception> {
        let Some(info) = self.0.inputs()?.packages.application_info(package, 0) else {
            return Ok(());
        };
        let uid = user * PER_USER_RANGE as i32 + info.uid % PER_USER_RANGE as i32;
        let duration =
            Duration::from_millis(duration_ms.max(0) as u64).min(MAX_TEMP_APP_ALLOWLIST_DURATION);
        self.0.temp_allowlist_for_fgs(uid, duration, REASON_UNKNOWN);
        Ok(())
    }
}

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
                match a.package_name.as_deref() {
                    Some(package) => self
                        .exempt(package, a.duration, a.user_handle)
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
