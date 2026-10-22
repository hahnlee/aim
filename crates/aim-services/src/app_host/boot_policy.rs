//! The package boot scan's decisions outside the package model (#2021),
//! as the app host makes them where system_server would
//! (`android-16.0.0_r1`): `PackageBackwardCompatibility`'s test base on
//! the boot classpath, `SharedUidMigration`, the users of
//! `Settings.getAllUsers`, ApexManager's inventory and
//! `notifyScanResult`, `DomainVerificationService.generateNewId`,
//! `AndroidTestBaseUpdater`'s compat change and `SELinuxMMAC`'s target
//! SDK, for the app host's bootstrap bridge (`package_bootstrap`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;

use super::compat::CompatConfig;
use crate::package::bootstrap::ApexPackage;
use crate::package::pkg::AndroidPackage;
use crate::package::scan::SharedUidMigration;
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
pub(super) const SHARED_UID_MIGRATION_STRATEGY: &str =
    "persist.debug.pm.shared_uid_migration_strategy";

/// `PackageBackwardCompatibility`'s `bootClassPathContainsATB`: the boot
/// class loader (`ParsingPackage`'s) does not find
/// `AndroidTestBaseUpdater`.
pub(super) fn test_base_on_bootclasspath(root: &Path) -> Result<bool, String> {
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
pub(super) fn shared_uid_migration(
    build_type: Option<&str>,
    strategy: Option<&str>,
) -> SharedUidMigration {
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
pub(super) fn seinfo_target_sdk(compat: &CompatConfig, package: &AndroidPackage) -> i32 {
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
fn check_apex_scan<'r>(
    sdk: &Sdk,
    services: &mut Vec<String>,
    results: impl IntoIterator<Item = (&'r ApexPackage, &'r AndroidPackage)>,
) -> Result<(), String> {
    let mut modules = std::collections::BTreeSet::new();
    for (info, package) in results {
        for service in &package.apex_system_services {
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
            if info.active {
                let name = service.name.clone().unwrap_or_default();
                if services.contains(&name) {
                    return Err(format!(
                        "Duplicate apex-system-service {name} from {}",
                        info.module_path
                    ));
                }
                services.push(name);
            }
        }
        if info.active && !modules.insert(info.module_name.clone()) {
            return Err(format!(
                "Two active packages have the same APEX module name: {}",
                info.module_name.as_deref().unwrap_or("null")
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

/// `REMOVE_ANDROID_TEST_BASE` for the package's application
/// (`isChangeEnabled(133396946, generateAppInfoWithoutState(pkg))`).
pub(super) fn test_base_change(compat: &CompatConfig, package: &AndroidPackage) -> bool {
    compat.is_enabled(
        REMOVE_ANDROID_TEST_BASE,
        Some(&package.package_name),
        package.target_sdk_version,
    )
}

/// `ApexBootFeed.notifyScanResults` of the payload the boot scan sends
/// (`ApexScanResult::notification_payload`), ApexManager's system
/// services kept in `services`.
pub(super) fn notify_apex_payload(
    sdk: &Sdk,
    services: &mut Vec<String>,
    payload: &[u8],
) -> Result<(), String> {
    let results = crate::package::scan::ApexScanResult::read_notification_payload(payload)?;
    check_apex_scan(
        sdk,
        services,
        results.iter().map(|(info, package)| (info, package)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::ApexSystemService;
    use crate::package::scan::ApexScanResult;

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

    fn result(
        module: &str,
        active: bool,
        services: &[(&str, Option<&str>)],
    ) -> (ApexPackage, AndroidPackage) {
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
        (
            ApexPackage {
                module_name: Some(module.into()),
                module_path: format!("/system/apex/{module}.apex"),
                preinstalled_path: format!("/system/apex/{module}.apex"),
                version_code: 1,
                factory: true,
                active,
                active_changed: false,
            },
            package,
        )
    }

    fn pairs(
        results: &[(ApexPackage, AndroidPackage)],
    ) -> impl Iterator<Item = (&ApexPackage, &AndroidPackage)> {
        results.iter().map(|(info, package)| (info, package))
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
        assert!(check_apex_scan(&sdk, &mut services, pairs(&ok)).is_ok());
        assert_eq!(services, ["s"]);
        let mut services = Vec::new();
        assert!(
            check_apex_scan(
                &sdk,
                &mut services,
                pairs(&[
                    result("a", true, &[("s", None)]),
                    result("b", true, &[("s", None)])
                ])
            )
            .unwrap_err()
            .contains("Duplicate apex-system-service s")
        );
        let mut services = Vec::new();
        assert!(
            check_apex_scan(
                &sdk,
                &mut services,
                pairs(&[result("a", true, &[]), result("a", true, &[])])
            )
            .unwrap_err()
            .contains("same APEX module name: a")
        );
    }

    /// The boot scan's notification read back as the bridge receives it:
    /// each APEX and its parsed package, past signers skipped.
    #[test]
    fn the_apex_notification_is_read_back() {
        let (info, mut package) = result("a", true, &[("s", None)]);
        package.feature_flag_state = Some(Vec::new());
        let mut p = aim_binder_host::parcel::Parcel::new();
        p.write_i32(1);
        p.write_string16(info.module_name.as_deref());
        p.write_string16(Some(&info.module_path));
        p.write_string16(Some(&info.preinstalled_path));
        p.write_i64(info.version_code);
        p.write_bool(info.factory);
        p.write_bool(info.active);
        p.write_bool(info.active_changed);
        aim_service_aidl::write_byte_array(&mut p, Some(&package.to_cache_entry().unwrap().bytes));
        p.write_i32(1);
        aim_service_aidl::write_byte_array(&mut p, Some(b"certificate"));
        p.write_i32(0);
        let read = ApexScanResult::read_notification_payload(p.data()).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].0, info);
        assert_eq!(read[0].1.package_name, package.package_name);
        assert_eq!(read[0].1.apex_system_services.len(), 1);
        assert!(ApexScanResult::read_notification_payload(&p.data()[..8]).is_err());
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
