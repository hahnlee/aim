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
pub mod locale;
pub mod location;
pub mod media;
mod mirror;
mod nonces;
pub mod notifications;
pub mod package;
pub mod statusbar;
mod pasteboard;
mod service_host;
mod settings;
pub mod shadow;
mod shell;
mod system;
pub use system::System;
pub use system::package_runtime::BootFacts as PackageBootFacts;
pub use system::package_lifecycle as system_package_lifecycle;
pub mod system_package_installer_init;
pub mod system_package_lifecycle_init;
pub mod system_package_users_init;
pub mod system_package_persistence_init;
pub use system::package_boot_inputs as system_package_boot_inputs;
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
    package_runtime: std::sync::Mutex<Option<Arc<system::package_runtime::Runtime>>>,
    package_constructing: std::sync::atomic::AtomicBool,
}

impl NativeServices {
    pub fn configure_package_constructor_capture(&self,path:&std::path::Path)->Result<(),String> {
        self.system.configure_package_constructor_capture(path)
    }

    pub fn configure_package_image(&self, image: &std::path::Path, data: &std::path::Path,
        original_roots: &[std::path::PathBuf],
    ) -> Result<(), String> {
        self.system.configure_native_package_image(image, data, original_roots)
            .map_err(|error| error.message)
    }
    pub fn configure_package_boot(&self, policy: system::package_boot_inputs::Cli,
        resources: package::parse::resources::Config,
        persistence: system_package_persistence_init::Inputs, facts: system::package_runtime::BootFacts,
    ) -> Result<(), String> {
        self.system.configure_native_package_boot_policy(policy, resources).map_err(|error| error.message)?;
        self.system.configure_native_package_persistence(persistence).map_err(|error| error.message)?;
        self.system.configure_native_package_runtime_facts(facts).map_err(|error| error.message)
    }
    pub fn configure_package_property_area(&self, path: &std::path::Path) -> Result<(), String> {
        self.system.configure_native_package_property_area(path).map_err(|error| error.message)
    }
    /// Creates the services named `names` (the binder names of
    /// `image/native-services`) as nodes of a new binder process.
    pub fn new(driver: &Arc<Driver>, names: &[String]) -> Result<Self, String> {
        Self::new_for_namespace(driver,names,std::process::id() as i32)
    }
    pub fn new_for_namespace(driver:&Arc<Driver>,names:&[String],guest_pid:i32)->Result<Self,String>{
        if guest_pid<=0{return Err("invalid native service process identifier".into());}
        let process = LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid: guest_pid,
                euid: SYSTEM_UID,
                security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
            },
        );
        let system = system::System::new(
            process.clone(),
            &[&clipboard::APP_OPS[..], &location::APP_OPS[..]].concat(),
        );
        let settings = settings::Settings::new(process.clone(), system.clone());
        // The device's languages follow the Mac's, whichever services are
        // native.
        locale::start(process.clone(), &system);
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
                // Package endpoints are published only by the completed C boot
                // session. The explicit image name enables its configuration.
                "package" => { system.request_native_package_manager(); Vec::new() },
                other => return Err(format!("no native implementation of `{other}`")),
            };
            for (name, service) in published {
                services.push((name.to_string(), process.add_service(service)));
            }
        }
        process.start();
        Ok(Self { system, services, package_runtime: std::sync::Mutex::new(None),
            package_constructing: std::sync::atomic::AtomicBool::new(false) })
    }

    /// Compares the original services `names` with their native models
    /// ([`shadow`]), logging each call to `log`; `image` is the guest's
    /// root, `props` reads its properties and `files` its files.
    pub fn shadow(
        &self,
        driver: &Arc<Driver>,
        names: &[String],
        log: &std::path::Path,
        image: &std::path::Path,
        props: package::system_config::Properties,
        files: package::write::Files,
    ) -> Result<(), String> {
        shadow::start(driver, &self.system, names, log, image, props, files)
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

    pub fn initialize_package_runtime(&self, bridge: &Arc<package::bootstrap::Bridge>,
        inputs: system::package_runtime::Inputs,
    ) -> Result<(Binder, Binder), String> {
        if self.package_constructing.compare_exchange(false, true, std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire).is_err() { return Err("native package construction already running".into()); }
        struct Construction<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Construction<'_> {
            fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::Release); }
        }
        let _construction = Construction(&self.package_constructing);
        if self.package_runtime.lock().unwrap().is_some() { return Err("native package runtime already initialized".into()); }
        let runtime = self.system.initialize_native_package_runtime(bridge, inputs)
            .map_err(|error| format!("native package runtime: {}", error.message))?;
        let mut slot = self.package_runtime.lock().unwrap();
        if slot.is_some() { return Err("native package runtime initialized concurrently".into()); }
        let runtime = Arc::new(runtime);
        *slot = Some(runtime.clone());
        drop(slot);
        let endpoints = match self.system.publish_native_package_services(bridge, &runtime) {
            Ok(endpoints) => endpoints,
            Err(error) => {
                if let Err(cleanup) = self.system.detach_package_bootstrap(bridge) {
                    eprintln!("native package publication cleanup: {}", cleanup.message);
                }
                let retained = self.package_runtime.lock().unwrap().take();
                drop(retained);
                drop(runtime);
                return Err(format!("native package publication: {}", error.message));
            }
        };
        Ok(endpoints)
    }
}

impl Drop for NativeServices {
    fn drop(&mut self) {
        // BootSession retains its Runtime itself; that constructor does not
        // populate package_runtime. Retire the attached epoch in either path.
        if let Ok(bridge) = self.system.package_bootstrap() {
            if let Err(error) = self.system.detach_package_bootstrap(&bridge) {
                eprintln!("native package shutdown cleanup: {}", error.message);
            }
        }
        self.package_runtime.get_mut().unwrap().take();
        if let Err(error) = self.system.process().shutdown() {
            eprintln!("native Binder shutdown: {error}");
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::{fs, os::fd::AsRawFd};

    struct DataKeeper(Arc<fs::File>);
    impl aim_binder_host::local::Service for DataKeeper {
        fn descriptor(&self) -> &str { "test.IDataKeeper" }
        fn transact(&self, _: &mut aim_binder_host::local::Call<'_>) -> aim_binder_host::local::Reply {
            let _ = self.0.metadata().unwrap();
            Ok(aim_binder_host::parcel::Parcel::new())
        }
    }

    #[test]
    fn native_services_drop_releases_its_registered_data_file() {
        let path = std::env::temp_dir().join(format!("aim-native-services-stop-{}", std::process::id()));
        fs::write(&path, b"owned service data").unwrap();
        let file = Arc::new(fs::File::open(&path).unwrap());
        let fd = file.as_raw_fd();
        let weak = Arc::downgrade(&file);
        let services = NativeServices::new(&Driver::new(), &[]).unwrap();
        services.system.process().add_service(Arc::new(DataKeeper(file)));
        assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
        drop(services);
        assert!(weak.upgrade().is_none());
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
        fs::remove_file(path).unwrap();
    }
}
