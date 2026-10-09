//! C runtime assembly after the native scan and exclusive persistence recovery.
use super::*;
use crate::package::{bootstrap::Bridge, installer, owner, parse::resources::Config};
use std::path::PathBuf;
#[derive(Clone)]
pub struct BootFacts {
    pub properties: crate::system_package_installer_init::Properties,
    pub dependency_installer_enabled: bool, pub density: i32,
    pub storage_manager_package: Option<String>,
    pub fix_system_apps_first_install_time: bool,
    pub boot_apex_changed: bool,
}

pub struct Inputs {
    pub image: PathBuf, pub data: PathBuf, pub apks: Arc<crate::package::write::Apks>,
    pub resource_config: Config,
    pub system_config: crate::package::system_config::SystemConfig,
    pub prepared: crate::system_package_persistence_init::Prepared,
    pub persistence_inputs: crate::system_package_persistence_init::Inputs,
    pub runtime_metadata: owner::runtime_metadata::State,
    pub properties: crate::system_package_installer_init::Properties,
    pub dependency_installer_enabled: bool,
    pub factory_test: bool,
    pub parser_cache: Option<PathBuf>,
    pub apex_results: Vec<crate::package::scan::ApexScanResult>,
    pub registrations: Vec<crate::package::pkg::AndroidPackage>,
    pub density: i32, pub storage_manager_package: Option<String>,
    pub kernel: Arc<crate::package::kernel_mappings::Owner>,
    pub claims: PathBuf,
    pub fix_system_apps_first_install_time: bool,
    pub early_users: Arc<crate::package::early_user_operations::Endpoint>,
    pub early_user_binder: Binder,
    pub decompression: Arc<crate::package::scan::boot_compressed::BootDecompression>,
    pub boot_apex_changed: bool,
}
pub struct Runtime {
    pub constructor_capture: Option<(aim_storage::constructor_capture::Capture,Arc<crate::package::scan_snapshot::query_state::Capture>)>,
    pub persistence: crate::system_package_persistence_init::Installed,
    pub pipeline: Arc<installer::pipeline::Native>,
    pub archive: super::installer_archive::ArchiveOwners,
    pub metadata: super::package_metadata::MetadataWorkers,
    pub verification: super::package_verification_init::VerificationWorkers,
    pub maintenance: crate::package::diagnostics::Worker,
    pub internal_installs: installer::internal_installs::Worker,
    pub relocation: crate::package::move_package::Workers,
    pub application_data: crate::package::application_data::Worker,
    pub user_operations: Binder,
    pub events: Option<crate::package::events::Workers>,
    pub early_users: Arc<crate::package::early_user_operations::Endpoint>,
    pub boot_lifecycle: super::package_boot_lifecycle::Worker,
    pub web_policy: Arc<super::package_web_policy::Runtime>,
    pub original_domains: Arc<super::original_domains::Runtime>,
}
struct ConstructionGuard { system: Arc<System>, bridge: Arc<Bridge>, armed: bool }
impl Drop for ConstructionGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.system.detach_package_bootstrap(&self.bridge) {
                eprintln!("Native package partial construction cleanup failed: {}", error.message);
            }
        }
    }
}
impl System {
    pub fn configure_package_constructor_capture(&self,path:&std::path::Path)->std::result::Result<(),String> {
        let mut sink=self.package_constructor_capture.lock().unwrap();
        if sink.is_some(){return Err("package constructor capture already configured".into());}
        *sink=Some(Arc::new(aim_storage::constructor_capture::Sink::claim(path)?)); Ok(())
    }

    pub fn publish_native_package_services(self: &Arc<Self>, bridge: &Arc<Bridge>, runtime: &Runtime) -> Result<(Binder, Binder)> {
        self.check_package_bootstrap(bridge)?;
        let current = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.installer.as_ref().map(|(owner,_)| owner.clone()))
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native package runtime installer unavailable"))?;
        if !Arc::ptr_eq(&current, &runtime.persistence.installer) { return Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "package service publication uses another runtime")); }
        let pair=self.register_public_package_fronts(bridge)?;
        Ok((pair.public,pair.native))
    }

    pub fn initialize_native_package_runtime(self: &Arc<Self>, bridge: &Arc<Bridge>, inputs: Inputs) -> Result<Runtime> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        self.check_package_bootstrap(bridge)?;
        let mut construction = ConstructionGuard { system: self.clone(), bridge: bridge.clone(), armed: true };
        let events = self.configure_package_events(bridge)?;
        let web_policy = self.initialize_web_instant_policy(bridge)?;
        let effects = self.package_effects_owner(bridge)?;
        self.install_package_customization(bridge, &inputs.apks.platform, inputs.resource_config)?;
        let changes = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge)).map(|current| current.changes.clone())
            .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "runtime bootstrap changed"))?;
        self.new_existing_package_owner(bridge.clone(), effects.clone(), changes,
            Arc::new(installer::existing::Restores::default()))?;
        let policy_leaf = self.package_bootstrap_binder_leaf(bridge, api::GET_INSTALLER_POLICY_BRIDGE)?;
        let policy = crate::system_package_installer_init::policy_source(self, bridge, policy_leaf)?;
        let icons = crate::system_package_persistence_init::IconOwner::new(
            self.package_bootstrap_binder_leaf(bridge, api::GET_INSTALLER_RECOVERY_PRESENTATION)?);
        let publisher = self.package_installer_publisher()?;
        let system = Arc::downgrade(self);
        let attached = bridge.clone();
        let callback: installer::native::Callback = Arc::new(move |event| {
            if let installer::Event::Finished { id, .. } = event {
                if let Some(system) = system.upgrade() {
                    let owner = system.package_bootstrap.lock().unwrap().current.as_ref()
                        .filter(|current| Arc::ptr_eq(&current.bridge, &attached))
                        .and_then(|current| current.installer.as_ref().map(|(owner,_)| owner.clone()));
                    if let Some(owner) = owner {
                        if let Err(error) = owner.record_finished_install_history(id) {
                            eprintln!("Native finished install history failed: {}", error.message);
                        }
                    }
                }
            }
        });
        let persistence = crate::system_package_persistence_init::install(self, bridge,
            inputs.prepared, &inputs.persistence_inputs, self.capture_package_queries()?, inputs.runtime_metadata,
            inputs.system_config.clone(), policy, publisher, callback, self.process.clone(), icons,
            inputs.apks.clone(), inputs.properties.clone(), inputs.dependency_installer_enabled)?;
        let original_domains = self.initialize_original_domain_settings(bridge)?;
        {
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "domain bootstrap replaced"))?;
            current.original_domains = Some(Arc::downgrade(&original_domains));
        }
        self.initialize_package_preferred(bridge, &inputs.image, &inputs.system_config)?;
        let live = inputs.properties.clone();
        let property_error = Arc::new(Mutex::new(None));
        let failed = property_error.clone();
        let property_owner: crate::package::system_config::Properties = Box::new(move |name: &str| {
            match live() { Ok(values) => values.get(name).cloned(), Err(error) => {
                let mut first = failed.lock().unwrap();
                if first.is_none() { *first = Some(error); }
                None
            } }
        });
        let environment = self.construct_package_install_environment(bridge, &inputs.image, &inputs.data,
            inputs.apks.clone(), property_owner, super::package_environment_init::EnvironmentBootInputs {
                factory_test: inputs.factory_test, parser_cache: inputs.parser_cache,
            })?;
        if let Some(error) = property_error.lock().unwrap().take() { return Err(error); }
        let pipeline = self.configure_native_install_pipeline(bridge, &persistence.installer,
            environment)?;
        let removal_store = self.native_removal_store(bridge, persistence.disk.clone())?;
        let removal_leaf = self.package_bootstrap_binder_leaf(bridge, api::GET_INSTALLER_REMOVAL_BRIDGE)?;
        let query_leaf = self.package_bootstrap_binder_leaf(bridge, api::GET_INSTALLER_ARCHIVE_QUERY_BRIDGE)?;
        let archive = self.configure_installer_archive(bridge, removal_store, removal_leaf, query_leaf,
            inputs.data.clone(), inputs.density, inputs.storage_manager_package)?;
        let metadata = self.initialize_package_metadata(bridge, &inputs.data, &inputs.apks,
            inputs.resource_config, &inputs.apex_results, &inputs.registrations)?;
        self.install_package_launch_owner(bridge)?;
        self.install_native_package_mutations(bridge, pipeline.clone())?;
        let system = Arc::downgrade(self); let attached = bridge.clone();
        let commit = Arc::new(move |base: Arc<crate::package::scan_snapshot::query_state::Capture>, plans: Vec<crate::package::write::mutation::Plan>, caller: i32| {
            let system = system.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "application data mutation system stopped"))?;
            system.check_package_bootstrap(&attached)?;
            let install = system.package_install_guard();
            let dependencies = system.package_bootstrap.lock().unwrap().current.as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, &attached)).and_then(|current| current.mutations.clone())
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "application data mutation dependencies unavailable"))?;
            let prepared = crate::package::mutation_dispatch::Prepared { code: 0, plans,
                global: None, finish: crate::package::mutation_dispatch::Finish::None,
                reply: Parcel::new(), distraction_cleanup: Vec::new() };
            system.publish_prepared_package_mutation_guarded(&attached, &base, prepared, &dependencies, caller, install).map(|_| ())
        });
        let application_data = self.install_package_application_data(bridge, commit)?;
        let verification = self.initialize_native_package_verification(bridge, persistence.disk.clone())?;
        let internal_installs = self.initialize_package_internal_owners(bridge, inputs.kernel.clone())?;
        let maintenance = self.initialize_package_maintenance(bridge)?;
        let relocation = self.install_native_package_relocation(bridge, pipeline.clone(), inputs.claims)?;
        let capture = self.capture_package_queries()?;
        let lifecycle = capture.state().system.lifecycle.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "runtime lifecycle unavailable"))?;
        let instant = capture.state().system.instant_registry.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "runtime instant registry unavailable"))?;
        let non_stopped = capture.state().system.initial_non_stopped_system_packages.clone().ok_or_else(|| Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "initial non-stopped package owner unavailable"))?.into_iter().collect();
        let users = crate::system_package_users_init::create(self, bridge, persistence.disk.clone(),
            self.package_preferred_registry(bridge)?, persistence.runtime_metadata.clone(), instant,
            effects, lifecycle, inputs.kernel, inputs.fix_system_apps_first_install_time, non_stopped)?;
        inputs.early_users.attach_after_scan(users)?;
        let user_operations = inputs.early_user_binder;
        let lifecycle_leaf = self.package_bootstrap_binder_leaf(bridge, api::GET_PACKAGE_BOOT_LIFECYCLE_LEAF)?;
        let boot_lifecycle = self.initialize_package_boot_lifecycle(bridge, lifecycle_leaf,
            inputs.decompression, inputs.boot_apex_changed)?;
        self.check_package_bootstrap(bridge)?;
        self.persist_constructor_global_settings(bridge)?;
        let constructor_capture = if let Some(sink)=self.package_constructor_capture.lock().unwrap().clone() {
            let _publication=self.package_install_guard();
            let _disk=persistence.disk.lock().unwrap();
            let retained=self.capture_package_queries()?;
            Some((sink.freeze(&inputs.data,retained.scan().version(),inputs.persistence_inputs.controller_version)
                .map_err(|cause|Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,cause))?,retained))
        } else { None };
        construction.armed = false;
        Ok(Runtime { constructor_capture, persistence, pipeline, archive, metadata, verification,
            maintenance, internal_installs, relocation, application_data, user_operations, events,
            early_users: inputs.early_users, boot_lifecycle, web_policy, original_domains })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.early_users.close();
        if let Err(error) = self.original_domains.close() { eprintln!("Original domain owner close failed: {}", error.message); }
        if let Err(error) = self.web_policy.close() { eprintln!("Native web policy watcher close failed: {}", error.message); }
    }
}
