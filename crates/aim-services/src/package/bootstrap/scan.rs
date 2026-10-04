//! Original boot owners connected to native initial scanning (#836/#885).
use super::{ApexInventory, Bridge, OwnerError, ScanUsers};
use crate::package::{
    owner::seinfo::Policy,
    scan::{
        AbiPolicy, ApexImage, FirstBootSystemInputs, Image, LibraryCompatibility,
        NativeLibraryInstallPolicy, PolicyBridgeError, ScanClock, SeInfoScan, SharedUidMigration,
        SigningError, SystemImageScan, UserPolicy,
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
        SystemImageScan::first_boot(
            || Image::load(apks, &self.apex.scan_apexes()),
            apks,
            self.config,
            FirstBootSystemInputs {
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
            },
        )
        .map_err(BootError::Scan)
    }
}
