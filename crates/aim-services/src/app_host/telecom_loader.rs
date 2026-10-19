//! TelecomLoaderService for the app host (`android-16.0.0_r1`): at its
//! `PHASE_ACTIVITY_MANAGER_READY` it binds Telecom's `TelecomService`,
//! which runs in the system process (`android:process="system"`, the
//! `android` package's process the app host starts for it), has it create
//! the telecom service (`ITelecomLoader.createTelecomService`) with the
//! system's `InternalServiceRepository`, and publishes that as `telecom`;
//! a lost binding binds again. The default app providers it registers
//! feed only the default permission grant policy, and its role and
//! carrier config listeners grant the SIM call manager's permissions; the
//! app host runs neither the policy nor role changes nor telephony, so
//! there is nothing for them to do.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_os_iservicemanager as sm;
use aim_service_aidl::com_android_internal_telecom_ideviceidlecontrolleradapter as idle;
use aim_service_aidl::com_android_internal_telecom_iinternalserviceretriever as retriever;
use aim_service_aidl::com_android_internal_telecom_itelecomloader as loader;

use super::services::HostConnection;
use super::{AppHost, PER_USER_RANGE, reply};

const SERVICE_ACTION: &str = "com.android.ITelecomService";
const SERVICE_PACKAGE: &str = "com.android.server.telecom";
const SERVICE_CLASS: &str = "com.android.server.telecom.components.TelecomService";
/// `Context.BIND_IMPORTANT | BIND_FOREGROUND_SERVICE | BIND_AUTO_CREATE`.
const BIND_FLAGS: i64 = 0x40 | 0x0400_0000 | 0x1;
const SYSTEM_UID: i32 = 1000;
/// `DeviceIdleController.Constants.MAX_TEMP_APP_ALLOWLIST_DURATION_MS`.
const MAX_TEMP_APP_ALLOWLIST_DURATION: Duration = Duration::from_secs(5 * 60);
/// `PowerExemptionManager.REASON_UNKNOWN`.
const REASON_UNKNOWN: i32 = 0;

pub struct TelecomLoader {
    host: Arc<AppHost>,
    connection: Arc<dyn HostConnection>,
    /// `mServiceConnection` is set: a binding is held.
    bound: Mutex<bool>,
    /// `mServiceRepo`, made once (`setupServiceRepository`).
    repository: OnceLock<Binder>,
}

/// `TelecomServiceConnection`.
struct Connection(Weak<TelecomLoader>);

impl HostConnection for Connection {
    fn connected(&self, service: Option<Binder>, _dead: bool) {
        let Some(loader) = self.0.upgrade() else { return };
        // Off the services' lock, as `ServiceDispatcher` posts it.
        std::thread::spawn(move || match service {
            Some(Binder::Handle(h)) => loader.service_connected(h),
            _ => loader.connect(),
        });
    }
}

impl TelecomLoader {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        Arc::new_cyclic(|me| TelecomLoader {
            host,
            connection: Arc::new(Connection(me.clone())),
            bound: Mutex::new(false),
            repository: OnceLock::new(),
        })
    }

    /// `onBootPhase(PHASE_ACTIVITY_MANAGER_READY)`: `connectToTelecom`.
    pub fn activity_manager_ready(&self) {
        self.connect();
    }

    /// `connectToTelecom`: a binding held is let go, then the service is
    /// bound as the system user.
    fn connect(&self) {
        let mut bound = self.bound.lock().unwrap();
        if *bound {
            self.host.unbind_service_from_host(&self.connection);
            *bound = false;
        }
        match self.host.bind_service_from_host(Some(SERVICE_ACTION), SERVICE_PACKAGE, SERVICE_CLASS, 0, BIND_FLAGS, 0, self.connection.clone()) {
            Ok(b) => *bound = b,
            Err(e) => eprintln!("app host: telecom: bind {SERVICE_PACKAGE}: {}", e.message),
        }
    }

    /// `onServiceConnected`: the telecom service created and published.
    fn service_connected(&self, handle: u32) {
        let published = (|| -> Result<(), String> {
            let repository = self
                .repository
                .get_or_init(|| self.host.process.add_service(Arc::new(Repository { host: self.host.clone(), adapter: OnceLock::new() })))
                .clone();
            let system_ui = self.system_ui_package();
            let mut data = Parcel::new();
            loader::CreateTelecomService { retriever: Some(repository), sys_ui_name: system_ui }.write(&mut data);
            let reply = self.host.process.transact(handle, loader::CREATE_TELECOM_SERVICE, &data, false).map_err(|s| format!("createTelecomService: {s}"))?;
            let service = loader::read_create_telecom_service_reply(&mut reply.reader())
                .map_err(|s| format!("createTelecomService reply: {s}"))?
                .map_err(|e| format!("createTelecomService: {}", e.message))?;
            let Some(service) = service else { return Err("createTelecomService: no service".into()) };
            let mut data = Parcel::new();
            sm::AddService { name: Some("telecom".into()), service: Some(service), allow_isolated: false, dump_priority: 1 << 3 }.write(&mut data);
            let reply = self.host.process.transact(0, sm::ADD_SERVICE, &data, false).map_err(|s| format!("servicemanager: {s}"))?;
            sm::read_add_service_reply(&mut reply.reader())
                .map_err(|s| format!("servicemanager: {s}"))?
                .map_err(|e| format!("addService(telecom): {}", e.message))
        })();
        // `RemoteException`: "Failed linking to death."
        if let Err(e) = published {
            eprintln!("app host: telecom: {e}");
        }
    }

    /// `PackageManagerInternal.getSystemUiServiceComponent`'s package
    /// (`config_systemUIServiceComponent`).
    fn system_ui_package(&self) -> Option<String> {
        let component = self.host.inputs().ok()?.packages.framework_string("config_systemUIServiceComponent")?;
        component.split_once('/').map(|(package, _)| package.to_string())
    }
}

/// `InternalServiceRepository`: the system's internal services for the
/// system process.
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
                if call.sender_euid as i32 != SYSTEM_UID {
                    Err(Exception::security("SYSTEM ONLY API."))
                } else {
                    let adapter = self.adapter.get_or_init(|| self.host.process.add_service(Arc::new(IdleAdapter(self.host.clone()))));
                    retriever::write_get_device_idle_controller_reply(&mut p, Some(adapter.clone()));
                    Ok(())
                }
            }
            code => {
                let name = retriever::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
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
        let Some(info) = self.0.inputs()?.packages.application_info(package, 0) else { return Ok(()) };
        let uid = user * PER_USER_RANGE as i32 + info.uid % PER_USER_RANGE as i32;
        let duration = Duration::from_millis(duration_ms.max(0) as u64).min(MAX_TEMP_APP_ALLOWLIST_DURATION);
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
                let a = idle::ExemptAppTemporarilyForEvent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.package_name.as_deref() {
                    Some(package) => self.exempt(package, a.duration, a.user_handle).map(|()| idle::write_exempt_app_temporarily_for_event_reply(&mut p)),
                    // `getPackageUidAsUser(null, ..)`.
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                }
            }
            code => {
                let name = idle::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.0.unimplemented_named("telecom", name))
            }
        };
        reply(result.map(|()| p))
    }
}
