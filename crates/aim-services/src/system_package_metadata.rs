//! Native C constructor metadata owners. Include as a child of `system` so
//! prepared captures can be published through the same bootstrap/version gate.
use super::System;
use crate::package::{
    bootstrap::Bridge, instant, module_metadata::ApexLinks, parse::resources::Config,
    pkg::AndroidPackage, scan::ApexScanResult, write::Apks,
};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
use std::{path::Path, sync::Arc};

pub struct MetadataWorkers {
    /// Retain with the constructor's other workers and drop outside System locks.
    pub instant: instant::Worker,
}

impl System {
    /// Invoke once after native initial scanning and before publishing the full
    /// facade. `registrations` is the native APK scan notification order, not a
    /// sorted reconstruction of package names or original PMS/feed state.
    pub fn initialize_package_metadata(
        &self,
        bridge: &Arc<Bridge>,
        data: &Path,
        apks: &Apks,
        config: Config,
        apex_results: &[ApexScanResult],
        registrations: &[AndroidPackage],
    ) -> Result<MetadataWorkers, Exception> {
        let fail = |message: String| Exception::new(EX_ILLEGAL_STATE, message);
        self.check_package_bootstrap(bridge)?;
        let data = std::fs::canonicalize(data).map_err(|error| fail(error.to_string()))?;
        if !data.is_dir() {
            return Err(fail(
                "native metadata writable root is not a directory".into(),
            ));
        }
        let install = self.package_install_guard();
        let capture = self.capture_package_queries()?;
        if capture.state().system.module_metadata.is_some()
            || capture.state().system.app_metadata_files.is_some()
            || capture.state().system.instant_registry.is_some()
        {
            return Err(fail(
                "native package metadata owners already initialized".into(),
            ));
        }
        let inventory = capture
            .state()
            .apex_inventory
            .as_ref()
            .ok_or_else(|| fail("native accepted APEX inventory unavailable".into()))?;
        let loaded = capture.scan().owner().loaded_packages();
        let mut seen = std::collections::BTreeSet::new();
        for package in registrations {
            if !seen.insert(package.package_name.as_str()) {
                return Err(fail(
                    "native APK notification order contains duplicate packages".into(),
                ));
            }
            let accepted = loaded.get(&package.package_name).ok_or_else(|| {
                fail("APK notification is absent from accepted native scan".into())
            })?;
            if accepted.package.base_apk_path != package.base_apk_path
                || accepted.package.path != package.path
                || accepted.package.version_code != package.version_code
            {
                return Err(fail(
                    "APK notification differs from accepted native code generation".into(),
                ));
            }
        }
        for (name, accepted) in loaded {
            if accepted.package.booleans2 & crate::package::pkg::booleans2::APEX == 0
                && !seen.contains(name.as_str())
            {
                return Err(fail(
                    "accepted native APK is missing from scan notification order".into(),
                ));
            }
        }
        let apex = ApexLinks::from_scan(apex_results, inventory, registrations).map_err(fail)?;
        let files = |guest: &str| {
            (apks.files)(guest)
                .ok_or_else(|| format!("accepted module APK VFS mapping unavailable: {guest}"))
        };
        self.check_package_bootstrap(bridge)?;
        {
            let mut state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_mut()
                .filter(|current| {
                    Arc::ptr_eq(&current.bridge, bridge)
                })
                .ok_or_else(|| fail("module metadata bootstrap generation changed".into()))?;
            let latest=current.queries.clone().ok_or_else(||fail("module metadata current capture unavailable".into()))?;
            if capture.scan().owner().loaded_packages()!=latest.scan().owner().loaded_packages() {
                return Err(fail("module metadata accepted code changed during initialization".into()));
            }
            if latest.state().system.module_metadata.is_some(){return Err(fail("module metadata owner already initialized".into()));}
            let mut update=latest.prepare_package_update(latest.scan().owner().clone()).map_err(fail)?;
            update.capture=update.capture.load_module_metadata(&apks.platform,config,&files,&apex).map_err(fail)?;
            current.publish_snapshot(update.store);
            current.queries = Some(update.capture.clone());
            if let Some(page) = &current.version_page {
                page.publish(update.capture.scan().version());
            }
            state.version = update.capture.scan().version();
        }
        drop(install);
        // These methods construct the actual native FD and cookie owners, then
        // atomically install their generations. The instant configuration comes
        // from retained original resources/settings, never a guessed density or
        // maximum cookie size. Its thread guard is returned to the constructor.
        self.install_package_app_metadata_files(bridge, &data)?;
        let worker = self.install_package_instant_registry(bridge, &data)?;
        Ok(MetadataWorkers { instant: worker })
    }
}
