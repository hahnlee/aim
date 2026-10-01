//! Android's system services as native macOS implementations (ADR 0013).
//!
//! A native service implements a platform service's AIDL interface at the
//! pinned image's version (the codes and parcels generated into
//! `aim-service-aidl`), is backed by macOS where macOS owns the function,
//! and is registered with the original servicemanager under the original
//! name; SystemServer no longer starts the original
//! (`image/native-services`). Apps and the framework code in their
//! processes cannot tell the difference.
//!
//! The services run in the process that hosts the binder driver
//! (guest-init), as one binder process with the system uid
//! ([`aim_binder_host::local`]). docs/system-services.md tracks which
//! services are native and how they conform.

mod bundle;
pub mod clip;
pub mod clipboard;
pub mod location;
pub mod media;
mod mirror;
mod nonces;
pub mod notifications;
pub mod statusbar;
mod pasteboard;
mod service_host;
mod settings;
mod shell;
mod system;
pub mod thermal;
pub mod uimode;
pub mod vibrator;
pub mod volume;

use std::sync::Arc;

use aim_binder_driver::{Credentials, Device, Driver};
use aim_binder_host::local::{LocalProcess, Service};
use aim_binder_host::parcel::Binder;

/// `Process.SYSTEM_UID`: the native services act as system_server did.
const SYSTEM_UID: u32 = 1000;
/// The name the service host registers with servicemanager, where the
/// device's system service in system_server finds it
/// (java/device-services).
const SERVICE_HOST: &str = "aim.service_host";
/// system_server's SELinux context, which servicemanager checks the
/// registration of these names against.
const SYSTEM_SERVER_CONTEXT: &str = "u:r:system_server:s0";

/// The native services of a boot.
pub struct NativeServices {
    system: Arc<system::System>,
    services: Vec<(String, Binder)>,
}

impl NativeServices {
    /// Creates the services named `names` (the binder names of
    /// `image/native-services`) as nodes of a new binder process.
    pub fn new(driver: &Arc<Driver>, names: &[String]) -> Result<Self, String> {
        let process = LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
            },
        );
        let system = system::System::new(
            process.clone(),
            &[&clipboard::APP_OPS[..], &location::APP_OPS[..]].concat(),
        );
        let settings = settings::Settings::new(process.clone(), system.clone());
        // First, so system_server finds it whichever services are native.
        let mut services = vec![(
            SERVICE_HOST.to_string(),
            process.add_service(Arc::new(service_host::ServiceHost::new(
                process.clone(),
                &system,
            ))),
        )];
        for name in names {
            // A service and the names it is published under, as its
            // original publishes them.
            let published: Vec<(&str, Arc<dyn Service>)> = match name.as_str() {
                "clipboard" => vec![(
                    "clipboard",
                    clipboard::ClipboardService::new(
                        process.clone(),
                        system.clone(),
                        settings.clone(),
                    ),
                )],
                "location" => vec![(
                    "location",
                    location::LocationManagerService::new(
                        process.clone(),
                        system.clone(),
                        settings.clone(),
                    ),
                )],
                "vibrator_manager" => vec![
                    (
                        "vibrator_manager",
                        vibrator::VibratorManagerService::new(process.clone(), system.clone()),
                    ),
                    (
                        "external_vibrator_service",
                        Arc::new(vibrator::ExternalVibratorService),
                    ),
                ],
                "uimode" => vec![(
                    "uimode",
                    uimode::UiModeManagerService::new(
                        process.clone(),
                        system.clone(),
                        settings.clone(),
                    ),
                )],
                "thermalservice" => vec![(
                    "thermalservice",
                    thermal::ThermalManagerService::new(process.clone(), system.clone()),
                )],
                other => return Err(format!("no native implementation of `{other}`")),
            };
            for (name, service) in published {
                services.push((name.to_string(), process.add_service(service)));
            }
        }
        process.start();
        Ok(Self { system, services })
    }

    /// Registers every service with servicemanager, which must be ready
    /// (`servicemanager.ready`); again after it restarted.
    pub fn register(&self) -> Result<(), String> {
        for (name, binder) in &self.services {
            self.system
                .add_service(name, *binder)
                .map_err(|e| format!("addService({name}): {}", e.message))?;
        }
        Ok(())
    }
}
