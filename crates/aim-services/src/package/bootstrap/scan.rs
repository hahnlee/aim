//! Original boot owners connected to native initial scanning (#836/#885).
use super::{ApexInventory, Bridge, OwnerError, ScanUsers};
use crate::package::{
    owner::seinfo::Policy,
    scan::{
        AbiPolicy, ApexImage, FirstBootSystemInputs, Image, LibraryCompatibility,
        NativeLibraryInstallPolicy, PolicyBridgeError, SavedSystemScanInputs, ScanClock,
        SeInfoScan, SharedUidMigration, SigningError, SigningScan, SystemImagePackages,
        SystemImageScan, UserPolicy,
    },
    system_config::SystemConfig,
    write::Apks,
};

#[derive(Debug)]
pub enum BootError {
    Owner(OwnerError),
    Compatibility(PolicyBridgeError),
    Scan(SigningError),
}

/// Image and invocation policy, independent of original service decisions.
pub struct ScanPolicy<'a> {
    pub seinfo: &'a Policy,
    pub apex_parse_flags: i32,
    pub first_api_level: i32,
    pub vendor_sdk: i32,
    pub abi: &'a AbiPolicy,
    pub preferred_abi: &'a str,
    pub app_lib32_install_dir: &'a str,
    pub platform_runtime_64bit: bool,
    pub install: NativeLibraryInstallPolicy,
    pub clock: ScanClock,
    pub factory_test: bool,
    pub install_user: Option<i32>,
    pub allow_install: bool,
    pub instant_app: bool,
    pub virtual_preload: bool,
    pub stopped_system_app: bool,
}

#[derive(Debug)]
pub struct SavedSystemPhase {
    pub apex: Vec<crate::package::scan::ApexScanResult>,
    pub system: SystemImagePackages,
}

/// All captured inputs belong to this exact retained original bridge.
pub struct BootOwners<'a> {
    bridge: &'a Bridge,
    config: &'a SystemConfig,
    apex: ApexInventory,
    users: ScanUsers,
    migration: SharedUidMigration,
    compatibility: LibraryCompatibility,
}

impl Bridge {
    pub fn resolve_boot<'a>(
        &'a self,
        config: &'a SystemConfig,
        properties: &dyn Fn(&str) -> Option<String>,
    ) -> Result<BootOwners<'a>, BootError> {
        let compatibility = self
            .library_compatibility(config, properties)
            .map_err(BootError::Compatibility)?;
        let migration = self.shared_uid_migration().map_err(BootError::Owner)?;
        let users = self.scan_users().map_err(BootError::Owner)?;
        let apex = self.apex_inventory().map_err(BootError::Owner)?;
        Ok(BootOwners {
            bridge: self,
            config,
            apex,
            users,
            migration,
            compatibility,
        })
    }
}

impl BootOwners<'_> {
    pub fn apex(&self) -> &ApexInventory {
        &self.apex
    }
    pub fn users(&self) -> &ScanUsers {
        &self.users
    }
    pub fn migration(&self) -> SharedUidMigration {
        self.migration
    }

    /// A completed initial scan is still unpublished: data reconciliation,
    /// permission/runtime owners and final snapshot publication follow it.
    pub fn scan_first_boot(
        self,
        apks: &Apks,
        policy: ScanPolicy<'_>,
    ) -> Result<SystemImageScan, BootError> {
        self.with_scan_inputs(apks, policy, |inputs| {
            SystemImageScan::first_boot(
                || Image::load(apks, &self.apex.scan_apexes()),
                apks,
                self.config,
                inputs,
            )
        })
    }

    /// The restored owner remains authoritative after earlier container/resource
    /// effects even when a later system package or notification fails.
    pub fn scan_saved_system(
        self,
        owner: &mut SigningScan,
        apks: &Apks,
        policy: ScanPolicy<'_>,
        saved: SavedSystemScanInputs<'_>,
    ) -> Result<SavedSystemPhase, BootError> {
        self.with_scan_inputs(apks, policy, |inputs| {
            let apex = owner.scan_initial_apex(apks, self.config, &inputs)?;
            (inputs.notify_apex_scan)(&apex).map_err(|message| {
                SigningError::Rejected(crate::package::scan::Error {
                    package: String::new(),
                    path: String::new(),
                    phase: "apex-notification",
                    message,
                })
            })?;
            let image =
                Image::load(apks, &self.apex.scan_apexes()).map_err(SigningError::Rejected)?;
            let system = owner.scan_saved_system_image(image, apks, self.config, inputs, saved)?;
            Ok(SavedSystemPhase { apex, system })
        })
    }

    fn with_scan_inputs<T>(
        &self,
        apks: &Apks,
        policy: ScanPolicy<'_>,
        run: impl FnOnce(FirstBootSystemInputs<'_>) -> Result<T, SigningError>,
    ) -> Result<T, BootError> {
        let apex_image = ApexImage::load(apks, &self.apex, policy.apex_parse_flags)
            .map_err(|error| BootError::Scan(SigningError::Rejected(error)))?;
        let notify = |results: &[crate::package::scan::ApexScanResult]| {
            self.bridge
                .notify_apex_scan(results)
                .map_err(|error| format!("original APEX notification: {error:?}"))
        };
        let domain = || {
            self.bridge
                .new_domain_id()
                .map_err(|error| format!("original domain owner: {error:?}"))
        };
        run(FirstBootSystemInputs {
            seinfo: SeInfoScan {
                policy: policy.seinfo,
                compatibility: self.bridge,
            },
            apex_image: &apex_image,
            notify_apex_scan: &notify,
            first_api_level: policy.first_api_level,
            vendor_sdk: policy.vendor_sdk,
            shared_uid_migration: self.migration,
            abi_policy: policy.abi,
            compatibility: &self.compatibility,
            preferred_abi: policy.preferred_abi,
            app_lib32_install_dir: policy.app_lib32_install_dir,
            platform_runtime_64bit: policy.platform_runtime_64bit,
            install: policy.install,
            clock: policy.clock,
            factory_test: policy.factory_test,
            users: UserPolicy {
                install_user: policy.install_user,
                users: self.users.users.as_deref(),
                allow_install: policy.allow_install,
                instant_app: policy.instant_app,
                virtual_preload: policy.virtual_preload,
                stopped_system_app: policy.stopped_system_app,
            },
            new_domain_id: &domain,
        })
        .map_err(BootError::Scan)
    }
}
