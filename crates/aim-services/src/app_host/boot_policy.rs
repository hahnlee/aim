//! The package boot scan's decisions outside the package model (#2021),
//! as the app host makes them where system_server would
//! (`android-16.0.0_r1`): `PackageBackwardCompatibility`'s test base on
//! the boot classpath, `SharedUidMigration`, the users of
//! `Settings.getAllUsers`, ApexManager's inventory and
//! `notifyScanResult`, `DomainVerificationService.generateNewId`,
//! `AndroidTestBaseUpdater`'s compat change and `SELinuxMMAC`'s target
//! SDK. The default image takes the same decisions through the bootstrap
//! bridge.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;
use std::sync::Mutex;

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::AppHost;
use super::compat::CompatConfig;
use crate::package::bootstrap::{ApexInventory, BootPolicy, OwnerError, ScanUsers};
use crate::package::pkg::AndroidPackage;
use crate::package::scan::{ApexScanResult, SeInfoCompatibility, SharedUidMigration, User};
use crate::package::system_config::Sdk;

/// `PackageBackwardCompatibility.addUpdaterForAndroidTestBase`'s class,
/// only on a boot classpath without `android.test.base`.
const ANDROID_TEST_BASE_UPDATER: &str = "Landroid/content/pm/AndroidTestBaseUpdater;";
/// `AndroidTestBaseUpdater.REMOVE_ANDROID_TEST_BASE`.
const REMOVE_ANDROID_TEST_BASE: i64 = 133_396_946;
/// `SELinuxMMAC.SELINUX_LATEST_CHANGES`, `SELINUX_R_CHANGES`.
const SELINUX_LATEST_CHANGES: i64 = 143_539_591;
const SELINUX_R_CHANGES: i64 = 168_782_947;
/// `Build.VERSION_CODES.CUR_DEVELOPMENT`, `R`.
const CUR_DEVELOPMENT: i32 = 10_000;
const R: i32 = 30;
/// `SharedUidMigration.PROPERTY_KEY`.
const SHARED_UID_MIGRATION_STRATEGY: &str = "persist.debug.pm.shared_uid_migration_strategy";

/// What the decisions read, taken once for a boot scan.
pub struct NativeBootPolicy<'a> {
    compat: &'a CompatConfig,
    sdk: Sdk,
    test_base_on_bootclasspath: bool,
    migration: SharedUidMigration,
    users: Vec<User>,
    apex: ApexInventory,
    /// ApexManager's `mApexSystemServices`' names, kept across scans.
    apex_system_services: Mutex<Vec<String>>,
}

impl<'a> NativeBootPolicy<'a> {
    /// The decisions for the app host's image, its properties and users,
    /// apexd's packages, and `compat`.
    pub fn new(host: &AppHost, compat: &'a CompatConfig) -> Result<Self, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let root = &host
            .configured
            .get()
            .ok_or_else(|| state("app host not configured".into()))?
            .root;
        Ok(Self {
            compat,
            sdk: Sdk::new(&|name| host.property(name)),
            test_base_on_bootclasspath: test_base_on_bootclasspath(root).map_err(state)?,
            migration: shared_uid_migration(
                host.property("ro.build.type").as_deref(),
                host.property(SHARED_UID_MIGRATION_STRATEGY).as_deref(),
            ),
            users: super::user_service::scan_users(host)?,
            apex: super::apex_manager::inventory(&host.process).map_err(state)?,
            apex_system_services: Mutex::new(Vec::new()),
        })
    }
}

/// `PackageBackwardCompatibility`'s `bootClassPathContainsATB`: the boot
/// class loader (`ParsingPackage`'s) does not find
/// `AndroidTestBaseUpdater`.
fn test_base_on_bootclasspath(root: &Path) -> Result<bool, String> {
    let jars = aim_android_image::classpath::jars(
        root,
        "bootclasspath.pb",
        aim_android_image::classpath::BOOTCLASSPATH,
    )?;
    Ok(!aim_android_image::linkage::ClassPath::read(root, &jars)?
        .defines(ANDROID_TEST_BASE_UPDATER)?)
}

/// `SharedUidMigration.applyStrategy(BEST_EFFORT)`:
/// `ENABLE_SHARED_UID_MIGRATION` holds, and on a userdebug build the
/// property's strategy (`getCurrentStrategy`, at most BEST_EFFORT) is it.
fn shared_uid_migration(build_type: Option<&str>, strategy: Option<&str>) -> SharedUidMigration {
    const NEW_INSTALL_ONLY: i32 = 1;
    const BEST_EFFORT: i32 = 2;
    let current = if build_type != Some("userdebug") {
        NEW_INSTALL_ONLY
    } else {
        match strategy.and_then(super::activity::parse_int) {
            Some(s) if (NEW_INSTALL_ONLY..=BEST_EFFORT).contains(&s) => s,
            _ => NEW_INSTALL_ONLY,
        }
    };
    if current >= BEST_EFFORT {
        SharedUidMigration::BestEffort
    } else {
        SharedUidMigration::NewInstallOnly
    }
}

/// `SELinuxMMAC.getTargetSdkVersionForSeInfo`.
fn seinfo_target_sdk(compat: &CompatConfig, package: &AndroidPackage) -> i32 {
    let enabled = |change| {
        compat.is_enabled(
            change,
            Some(&package.package_name),
            package.target_sdk_version,
        )
    };
    if enabled(SELINUX_LATEST_CHANGES) {
        CUR_DEVELOPMENT.max(package.target_sdk_version)
    } else if enabled(SELINUX_R_CHANGES) {
        R.max(package.target_sdk_version)
    } else {
        package.target_sdk_version
    }
}

/// `ApexManagerImpl.notifyScanResultLocked`'s checks: an active APEX's
/// system services for this SDK (`UnboundedSdkLevel`) are new, and no two
/// active packages have one module name.
fn check_apex_scan(
    sdk: &Sdk,
    services: &mut Vec<String>,
    results: &[ApexScanResult],
) -> Result<(), String> {
    let mut modules = std::collections::BTreeSet::new();
    for result in results {
        for service in &result.package.apex_system_services {
            if let Some(min) = &service.min_sdk_version
                && !sdk.at_least_checked(min)?
            {
                continue;
            }
            if let Some(max) = &service.max_sdk_version
                && !sdk.at_most_checked(max)?
            {
                continue;
            }
            if result.info.active {
                let name = service.name.clone().unwrap_or_default();
                if services.contains(&name) {
                    return Err(format!(
                        "Duplicate apex-system-service {name} from {}",
                        result.info.module_path
                    ));
                }
                services.push(name);
            }
        }
        if result.info.active && !modules.insert(result.info.module_name.clone()) {
            return Err(format!(
                "Two active packages have the same APEX module name: {}",
                result.info.module_name.as_deref().unwrap_or("null")
            ));
        }
    }
    Ok(())
}

/// `UUID.randomUUID()`'s sixteen bytes, most significant first.
pub(super) fn random_uuid() -> [u8; 16] {
    let mut b = [0u8; 16];
    // SAFETY: fills the buffer it is given.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    b
}

impl SeInfoCompatibility for NativeBootPolicy<'_> {
    fn target_sdk(&self, package: &AndroidPackage) -> Result<i32, String> {
        Ok(seinfo_target_sdk(self.compat, package))
    }
}

impl BootPolicy for NativeBootPolicy<'_> {
    fn test_base_on_bootclasspath(&self) -> bool {
        self.test_base_on_bootclasspath
    }
    fn shared_uid_migration(&self) -> Result<SharedUidMigration, OwnerError> {
        Ok(self.migration)
    }
    fn scan_users(&self) -> Result<ScanUsers, OwnerError> {
        Ok(ScanUsers {
            users: Some(self.users.clone()),
        })
    }
    fn apex_inventory(&self) -> Result<ApexInventory, OwnerError> {
        Ok(self.apex.clone())
    }
    fn notify_apex_scan(&self, results: &[ApexScanResult]) -> Result<(), OwnerError> {
        check_apex_scan(
            &self.sdk,
            &mut self.apex_system_services.lock().unwrap(),
            results,
        )
        .map_err(OwnerError::Code)
    }
    fn new_domain_id(&self) -> Result<[u8; 16], OwnerError> {
        // `generateNewId`: `UUID.randomUUID()`.
        Ok(random_uuid())
    }
    fn remove_test_base(
        &self,
        package: &AndroidPackage,
        is_system: bool,
    ) -> Result<Option<bool>, OwnerError> {
        if self.test_base_on_bootclasspath || is_system {
            return Ok(None);
        }
        Ok(Some(self.compat.is_enabled(
            REMOVE_ANDROID_TEST_BASE,
            Some(&package.package_name),
            package.target_sdk_version,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::bootstrap::ApexPackage;
    use crate::package::pkg::ApexSystemService;

    /// Only a userdebug build reads the property, and only its first two
    /// strategies count (`getCurrentStrategy`).
    #[test]
    fn shared_uid_migration_follows_the_build_and_property() {
        use SharedUidMigration::*;
        assert_eq!(
            shared_uid_migration(Some("user"), Some("2")),
            NewInstallOnly
        );
        assert_eq!(
            shared_uid_migration(Some("userdebug"), None),
            NewInstallOnly
        );
        assert_eq!(
            shared_uid_migration(Some("userdebug"), Some("2")),
            BestEffort
        );
        assert_eq!(
            shared_uid_migration(Some("userdebug"), Some("0x2")),
            BestEffort
        );
        assert_eq!(
            shared_uid_migration(Some("userdebug"), Some("3")),
            NewInstallOnly
        );
        assert_eq!(
            shared_uid_migration(Some("userdebug"), Some("x")),
            NewInstallOnly
        );
    }

    fn package(name: &str, target_sdk: i32) -> AndroidPackage {
        AndroidPackage {
            package_name: name.into(),
            target_sdk_version: target_sdk,
            ..Default::default()
        }
    }

    /// Undeclared changes are enabled, so the latest SELinux domain wins;
    /// a declared change gates by the target SDK.
    #[test]
    fn seinfo_target_sdk_follows_the_selinux_changes() {
        let undeclared = CompatConfig::default();
        assert_eq!(
            seinfo_target_sdk(&undeclared, &package("a", 29)),
            CUR_DEVELOPMENT
        );
    }

    fn result(module: &str, active: bool, services: &[(&str, Option<&str>)]) -> ApexScanResult {
        let mut package = package(&format!("com.{module}"), 36);
        package.apex_system_services = services
            .iter()
            .map(|(name, min)| ApexSystemService {
                name: Some((*name).into()),
                jar_path: None,
                min_sdk_version: min.map(Into::into),
                max_sdk_version: None,
                init_order: 0,
            })
            .collect();
        ApexScanResult {
            info: ApexPackage {
                module_name: Some(module.into()),
                module_path: format!("/system/apex/{module}.apex"),
                preinstalled_path: format!("/system/apex/{module}.apex"),
                version_code: 1,
                factory: true,
                active,
                active_changed: false,
            },
            package,
            signing: crate::package::sign::SigningDetails::unknown(),
        }
    }

    /// A service name taken twice by active APEXes, or a module active
    /// twice, fails the scan; inactive ones and services above the SDK do
    /// not count.
    #[test]
    fn apex_scan_checks_follow_apex_manager() {
        let sdk = Sdk::new(&|name| match name {
            "ro.build.version.sdk" => Some("36".into()),
            "ro.build.version.codename" => Some("REL".into()),
            _ => None,
        });
        let mut services = Vec::new();
        let ok = [
            result("a", true, &[("s", None)]),
            result("a", false, &[("s", None)]),
            result("b", true, &[("s", Some("37"))]),
        ];
        assert!(check_apex_scan(&sdk, &mut services, &ok).is_ok());
        assert_eq!(services, ["s"]);
        let mut services = Vec::new();
        assert!(
            check_apex_scan(
                &sdk,
                &mut services,
                &[
                    result("a", true, &[("s", None)]),
                    result("b", true, &[("s", None)])
                ]
            )
            .unwrap_err()
            .contains("Duplicate apex-system-service s")
        );
        let mut services = Vec::new();
        assert!(
            check_apex_scan(
                &sdk,
                &mut services,
                &[result("a", true, &[]), result("a", true, &[])]
            )
            .unwrap_err()
            .contains("same APEX module name: a")
        );
    }

    /// The pinned image's boot classpath has the updater exactly when it
    /// lacks `android.test.base` (REMOVE_ATB_FROM_BCP), as the original's
    /// lookup assumes.
    #[test]
    fn test_base_location_follows_the_boot_classpath() {
        let Some(root) = aim_paths::original_image_with("system/etc/classpaths/bootclasspath.pb")
        else {
            return;
        };
        let jars = aim_android_image::classpath::jars(
            &root,
            "bootclasspath.pb",
            aim_android_image::classpath::BOOTCLASSPATH,
        )
        .unwrap();
        let classes = aim_android_image::linkage::ClassPath::read(&root, &jars).unwrap();
        assert_eq!(
            test_base_on_bootclasspath(&root).unwrap(),
            classes.defines("Landroid/test/AndroidTestCase;").unwrap()
        );
    }

    /// A version 4 UUID's version and variant bits.
    #[test]
    fn domain_ids_are_random_uuids() {
        let id = random_uuid();
        assert_eq!(id[6] >> 4, 4);
        assert_eq!(id[8] >> 6, 2);
        assert_ne!(random_uuid(), id);
    }
}
