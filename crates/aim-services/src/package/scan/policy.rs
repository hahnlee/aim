//! Manifest restrictions from ScanPackageUtils.applyPolicy at android-16.0.0_r1.
//! Library compatibility (#808) and boot/version selection (#707) are separate owners.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::{Location, Partition};
use crate::package::{
    info,
    owner::shared_users::Bootstrap,
    pkg::{AndroidPackage, booleans as b, booleans2 as b2},
    settings,
    sign::SigningDetails,
    write::Apks,
};

/// Effective SCAN_AS_* inputs. The scan owner supplies shared-UID privilege
/// adjustments before applying policy; physical data alone has no system flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanPolicy {
    pub system: bool,
    pub privileged: bool,
    pub oem: bool,
    pub vendor: bool,
    pub product: bool,
    pub system_ext: bool,
    pub odm: bool,
    pub apex: bool,
}

impl ScanPolicy {
    /// InstallPackageHelper.adjustScanFlags's shared-UID privilege exception.
    /// The group owner supplies its current aggregate flags; saved XML alone
    /// does not persist all group flags. Platform-signed members are exempt.
    pub fn adjust_shared_uid_privilege(
        &mut self,
        pkg: &AndroidPackage,
        signing: &SigningDetails,
        platform: &SigningDetails,
        groups: &Bootstrap,
        vendor_sdk: i32,
    ) {
        if self.needs_shared_uid_privilege_check(pkg, groups, vendor_sdk)
            && !current_signers_match(platform, signing)
        {
            self.privileged = true;
        }
    }

    pub(super) fn needs_shared_uid_privilege_check(
        self,
        pkg: &AndroidPackage,
        groups: &Bootstrap,
        vendor_sdk: i32,
    ) -> bool {
        !self.privileged
            && !pkg.is(b::PRIVILEGED)
            && !pkg.is(b::LEAVING_SHARED_UID)
            && !(self.vendor && vendor_sdk < 28)
            && pkg
                .shared_user_id
                .as_ref()
                .and_then(|name| groups.shared_users.get(name))
                .is_some_and(|g| g.private_flags & settings::PRIVATE_FLAG_PRIVILEGED != 0)
    }
    pub fn for_location(location: &Location) -> Self {
        Self {
            system: location.partition != Partition::Data,
            privileged: location.privileged(),
            oem: location.partition == Partition::Oem,
            vendor: location.partition == Partition::Vendor,
            product: location.partition == Partition::Product,
            system_ext: location.partition == Partition::SystemExt,
            odm: location.partition == Partition::Odm,
            apex: false,
        }
    }

    /// adjustScanFlagsWithPackageSetting's system inheritance. The owner
    /// selects the disabled original, or the existing system setting only
    /// for SCAN_NEW_INSTALL with no disabled original.
    /// Supply refreshed factory flags when available. A disappeared factory
    /// retains its restored attributes until the ex-system data rescan.
    pub fn inherit_system_setting(&mut self, original: &settings::Package) {
        self.system = true;
        for (value, mask) in [
            (&mut self.privileged, 1 << 3),
            (&mut self.oem, 1 << 17),
            (&mut self.vendor, 1 << 18),
            (&mut self.product, 1 << 19),
            (&mut self.system_ext, 1 << 21),
            (&mut self.odm, 1 << 30),
        ] {
            *value |= original.private_flags & mask != 0;
        }
    }

    /// Apply only manifest/component restrictions and platform signing policy.
    /// Must precede validation/reconciliation. Does not claim to perform
    /// PackageBackwardCompatibility.modifySharedLibraries (#808) or ABI derivation.
    pub fn apply_manifest(
        self,
        pkg: &mut AndroidPackage,
        signing: &SigningDetails,
        platform: Option<&SigningDetails>,
        updated_system_app: bool,
        apks: &Apks,
    ) -> Result<(), String> {
        // Read before mutation so an inaccessible compressed inventory rejects
        // atomically. Unlike Java File.listFiles, errors remain observable.
        let compressed = self.system && apks.scan_compressed_files_exist(pkg)?;
        self.apply_components(pkg, signing, platform, updated_system_app, compressed);
        Ok(())
    }

    /// Full manifest and library policy with atomic rejection. The caller
    /// resolves boot-classpath/PlatformCompat inputs before reconciliation.
    pub fn apply(
        self,
        pkg: &mut AndroidPackage,
        signing: &SigningDetails,
        platform: Option<&SigningDetails>,
        updated_system_app: bool,
        apks: &Apks,
        compatibility: &super::LibraryCompatibility,
        remove_test_base: Option<bool>,
    ) -> Result<(), String> {
        let mut next = pkg.clone();
        self.apply_manifest(&mut next, signing, platform, updated_system_app, apks)?;
        compatibility.apply(
            &mut next,
            self.system || updated_system_app,
            updated_system_app,
            remove_test_base,
        )?;
        *pkg = next;
        Ok(())
    }

    fn apply_components(
        self,
        pkg: &mut AndroidPackage,
        signing: &SigningDetails,
        platform: Option<&SigningDetails>,
        updated_system_app: bool,
        compressed: bool,
    ) {
        if self.system {
            pkg.booleans |= b::SYSTEM;
            if pkg.is(b::DIRECT_BOOT_AWARE) {
                for c in pkg.activities.iter_mut().chain(&mut pkg.receivers) {
                    c.main.direct_boot_aware = true;
                }
                for c in &mut pkg.services {
                    c.main.direct_boot_aware = true;
                }
                for c in &mut pkg.providers {
                    c.main.direct_boot_aware = true;
                }
            }
            if compressed {
                pkg.booleans2 |= b2::STUB;
            }
        } else {
            pkg.protected_broadcasts.clear();
            pkg.booleans &= !(b::CORE_APP
                | b::PERSISTENT
                | b::DEFAULT_TO_DEVICE_PROTECTED_STORAGE
                | b::DIRECT_BOOT_AWARE);
            for group in &mut pkg.permission_groups {
                group.priority = 0;
            }
        }
        if !self.privileged {
            // ActivityInfo.FLAG_SINGLE_USER (also used by services/providers).
            const SINGLE_USER: i32 = 0x40000000;
            for c in &mut pkg.receivers {
                if c.main.component.flags & SINGLE_USER != 0 {
                    c.main.exported = false;
                }
            }
            for c in &mut pkg.services {
                if c.main.component.flags & SINGLE_USER != 0 {
                    c.main.exported = false;
                }
            }
            for c in &mut pkg.providers {
                if c.main.component.flags & SINGLE_USER != 0 {
                    c.main.exported = false;
                }
            }
        }
        for (flag, value) in [
            (b::PRIVILEGED, self.privileged),
            (b::OEM, self.oem),
            (b::VENDOR, self.vendor),
            (b::PRODUCT, self.product),
            (b::SYSTEM_EXT, self.system_ext),
            (b::ODM, self.odm),
            (
                b::SIGNED_WITH_PLATFORM_KEY,
                pkg.package_name == "android"
                    || platform.is_some_and(|p| current_signers_match(p, signing)),
            ),
        ] {
            pkg.booleans = (pkg.booleans & !flag) | if value { flag } else { 0 };
        }
        pkg.booleans2 = (pkg.booleans2 & !b2::APEX) | if self.apex { b2::APEX } else { 0 };
        if !self.system && !updated_system_app {
            pkg.original_packages.get_or_insert_with(Vec::new).clear();
            pkg.adopt_permissions.clear();
        }
    }
}

fn current_signers_match(a: &SigningDetails, b: &SigningDetails) -> bool {
    !a.signatures.is_empty()
        && a.signatures.len() == b.signatures.len()
        && a.signatures.iter().all(|s| b.signatures.contains(s))
        && b.signatures.iter().all(|s| a.signatures.contains(s))
}

/// ScanPackageUtils' final ApplicationInfo flags, from the policy-adjusted
/// package and setting owner. No persisted public/private flags are copied.
pub fn application_flags(pkg: &AndroidPackage, updated_system_app: bool) -> (i32, i32) {
    (
        info::base_flags(pkg)
            | if updated_system_app {
                info::FLAG_UPDATED_SYSTEM_APP
            } else {
                0
            },
        info::base_private_flags(pkg),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::pkg::{Activity, PermissionGroup, Provider, Service};

    fn signer(certs: &[u8]) -> SigningDetails {
        SigningDetails {
            current_flags: Vec::new(),
            signatures: certs.iter().map(|c| vec![*c]).collect(),
            scheme_version: 3,
            public_keys: vec![],
            past_signing_certificates: None,
        }
    }

    fn manifest() -> AndroidPackage {
        let mut activity = Activity::default();
        activity.main.exported = true;
        activity.main.component.flags = 0x40000000;
        let mut service = Service::default();
        service.main = activity.main.clone();
        let mut provider = Provider::default();
        provider.main = activity.main.clone();
        AndroidPackage {
            package_name: "fixture".into(),
            booleans: b::CORE_APP
                | b::PERSISTENT
                | b::DIRECT_BOOT_AWARE
                | b::DEFAULT_TO_DEVICE_PROTECTED_STORAGE
                | b::PARTIALLY_DIRECT_BOOT_AWARE
                | b::PRIVILEGED
                | b::VENDOR
                | b::SYSTEM_EXT
                | b::SIGNED_WITH_PLATFORM_KEY,
            booleans2: b2::APEX,
            activities: vec![activity.clone()],
            receivers: vec![activity],
            services: vec![service],
            providers: vec![provider],
            permission_groups: vec![PermissionGroup {
                priority: 4,
                ..Default::default()
            }],
            protected_broadcasts: vec!["broadcast".into()],
            original_packages: Some(vec![Some("old".into())]),
            adopt_permissions: vec!["old".into()],
            ..Default::default()
        }
    }

    #[test]
    fn apk_in_apex_keeps_partition_policy_without_becoming_an_apex_package() {
        let location = Location {
            path: "/apex/module/app/p".into(),
            partition: Partition::Vendor,
            kind: super::super::Kind::App,
            apex: Some(super::super::Apex {
                module_name: Some("module".into()),
                mount_path: "/apex/module".into(),
                partition: Partition::Vendor,
                factory: false,
                active_changed: true,
            }),
        };
        let policy = ScanPolicy::for_location(&location);
        assert!(policy.system && policy.vendor && !policy.apex);
        assert_eq!(
            location.parse_flags(),
            crate::package::parse::PARSE_IS_SYSTEM_DIR | crate::package::parse::PARSE_APK_IN_APEX
        );
        let mut pkg = manifest();
        pkg.booleans2 |= b2::APEX;
        policy.apply_components(&mut pkg, &signer(&[1]), None, false, false);
        assert!(pkg.is(b::SYSTEM | b::VENDOR));
        assert!(!pkg.is2(b2::APEX));
        ScanPolicy {
            apex: true,
            ..policy
        }
        .apply_components(&mut pkg, &signer(&[1]), None, false, false);
        assert!(pkg.is2(b2::APEX));
    }

    #[test]
    fn data_policy_restricts_manifest_without_erasing_component_direct_boot() {
        let mut pkg = manifest();
        pkg.receivers[0].main.direct_boot_aware = true;
        ScanPolicy::default().apply_components(&mut pkg, &signer(&[1]), None, false, false);
        assert_eq!(pkg.booleans, b::PARTIALLY_DIRECT_BOOT_AWARE);
        assert!(!pkg.is2(b2::APEX));
        assert!(pkg.activities[0].main.exported);
        assert!(!pkg.receivers[0].main.exported);
        assert!(!pkg.services[0].main.exported);
        assert!(!pkg.providers[0].main.exported);
        assert!(pkg.receivers[0].main.direct_boot_aware);
        assert!(!pkg.activities[0].main.direct_boot_aware);
        assert!(pkg.protected_broadcasts.is_empty());
        assert_eq!(pkg.permission_groups[0].priority, 0);
        assert_eq!(pkg.original_packages, Some(vec![]));
        assert!(pkg.adopt_permissions.is_empty());
        let mut updated = manifest();
        ScanPolicy::default().apply_components(&mut updated, &signer(&[1]), None, true, false);
        assert_eq!(updated.original_packages, Some(vec![Some("old".into())]));
        assert_eq!(updated.adopt_permissions, ["old"]);
        assert!(!updated.is(b::SYSTEM));
        assert_eq!(application_flags(&updated, true).0 & (1 | 128), 128);
    }

    #[test]
    fn system_policy_sets_partition_components_and_current_platform_signers() {
        let location = Location {
            path: "/product/priv-app/fixture".into(),
            partition: Partition::Product,
            kind: super::super::Kind::PrivApp,
            apex: None,
        };
        let policy = ScanPolicy::for_location(&location);
        let mut pkg = manifest();
        policy.apply_components(
            &mut pkg,
            &signer(&[2, 1]),
            Some(&signer(&[1, 2])),
            false,
            true,
        );
        let mask = b::SYSTEM | b::PRIVILEGED | b::PRODUCT | b::SIGNED_WITH_PLATFORM_KEY;
        assert_eq!(pkg.booleans & mask, mask);
        assert!(!pkg.is(b::VENDOR | b::SYSTEM_EXT));
        assert!(pkg.is2(b2::STUB));
        assert!(pkg.activities[0].main.direct_boot_aware);
        assert!(pkg.receivers[0].main.direct_boot_aware);
        assert!(pkg.services[0].main.direct_boot_aware);
        assert!(pkg.providers[0].main.direct_boot_aware);
        assert!(pkg.receivers[0].main.exported);
        assert_eq!(pkg.permission_groups[0].priority, 4);
        let (public, private) = application_flags(&pkg, true);
        assert_eq!(public & (1 | 128 | 8), 1 | 128 | 8);
        assert_eq!(
            private & ((1 << 3) | (1 << 19) | (1 << 20)),
            (1 << 3) | (1 << 19) | (1 << 20)
        );
        let mut rotated = signer(&[3]);
        rotated.past_signing_certificates = Some(vec![(vec![1], 31), (vec![3], 31)]);
        policy.apply_components(&mut pkg, &rotated, Some(&signer(&[1])), false, false);
        assert!(!pkg.is(b::SIGNED_WITH_PLATFORM_KEY));
        assert!(pkg.is2(b2::STUB)); // applyPolicy does not clear a previously set stub.
        pkg.package_name = "android".into();
        policy.apply_components(&mut pkg, &signer(&[3]), None, false, false);
        assert!(pkg.is(b::SIGNED_WITH_PLATFORM_KEY));
    }

    #[test]
    fn disabled_original_inherits_only_scan_partition_and_privilege_bits() {
        let original = settings::Package {
            flags: -1,
            private_flags: -1,
            ..Default::default()
        };
        let mut policy = ScanPolicy::default();
        policy.inherit_system_setting(&original);
        assert_eq!(
            policy,
            ScanPolicy {
                system: true,
                privileged: true,
                oem: true,
                vendor: true,
                product: true,
                system_ext: true,
                odm: true,
                apex: false,
            }
        );
        let mut pkg = manifest();
        policy.apply_components(&mut pkg, &signer(&[1]), None, true, false);
        assert!(!pkg.is(b::SIGNED_WITH_PLATFORM_KEY));
        assert!(!pkg.is2(b2::APEX));
        assert_eq!(application_flags(&pkg, true).1 & (1 << 9), 0);
    }

    #[test]
    fn shared_uid_privilege_honors_platform_vendor_and_leaving_exemptions() {
        let groups = Bootstrap::restore(&Default::default(), &Default::default()).unwrap();
        let pkg = AndroidPackage {
            shared_user_id: Some("android.uid.bluetooth".into()),
            ..Default::default()
        };
        for (vendor, sdk, leaving, same_key, expected) in [
            (false, 27, false, false, true),
            (true, 27, false, false, false),
            (true, 28, false, false, true),
            (false, 36, true, false, false),
            (false, 36, false, true, false),
        ] {
            let mut pkg = pkg.clone();
            if leaving {
                pkg.booleans |= b::LEAVING_SHARED_UID;
            }
            let mut policy = ScanPolicy {
                vendor,
                ..Default::default()
            };
            policy.adjust_shared_uid_privilege(
                &pkg,
                &signer(&[if same_key { 1 } else { 2 }]),
                &signer(&[1]),
                &groups,
                sdk,
            );
            assert_eq!(policy.privileged, expected);
        }
        let mut policy = ScanPolicy::default();
        let mut unknown = pkg.clone();
        unknown.shared_user_id = Some("absent".into());
        policy.adjust_shared_uid_privilege(&unknown, &signer(&[2]), &signer(&[1]), &groups, 36);
        assert!(!policy.privileged);
    }
}
