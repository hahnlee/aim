//! ScanPackageUtils.assertStaticSharedLibraryIsValid at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::pkg::AndroidPackage;

pub(super) fn static_library(pkg: &AndroidPackage, instant_app: bool) -> Result<(), String> {
    if pkg.static_shared_library_name.is_none() {
        return Ok(());
    }
    for (invalid, why) in [
        (
            pkg.target_sdk_version < 26,
            "static shared libraries must target SDK 26 or higher",
        ),
        (
            instant_app,
            "static shared libraries cannot be instant apps",
        ),
        (
            pkg.original_packages
                .as_ref()
                .is_some_and(|n| !n.is_empty()),
            "static shared libraries cannot be renamed",
        ),
        (
            !pkg.library_names.is_empty(),
            "static shared libraries cannot declare dynamic libraries",
        ),
        (
            pkg.shared_user_id.is_some(),
            "shared UID is not allowed in a static shared library",
        ),
        (
            !pkg.activities.is_empty(),
            "static shared libraries cannot declare activities",
        ),
        (
            !pkg.services.is_empty(),
            "static shared libraries cannot declare services",
        ),
        (
            !pkg.providers.is_empty(),
            "static shared libraries cannot declare content providers",
        ),
        (
            !pkg.receivers.is_empty(),
            "static shared libraries cannot declare receivers",
        ),
        (
            !pkg.permission_groups.is_empty(),
            "static shared libraries cannot declare permission groups",
        ),
        (
            !pkg.attributions.is_empty(),
            "static shared libraries cannot declare attributions",
        ),
        (
            !pkg.permissions.is_empty(),
            "static shared libraries cannot declare permissions",
        ),
        (
            !pkg.protected_broadcasts.is_empty(),
            "static shared libraries cannot declare protected broadcasts",
        ),
        (
            pkg.overlay_target.is_some(),
            "static shared libraries cannot be overlay targets",
        ),
    ] {
        if invalid {
            return Err(why.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_constraints_accept_empty_optional_lists_and_reject_each_forbidden_declaration() {
        let valid = AndroidPackage {
            target_sdk_version: 26,
            static_shared_library_name: Some("static".into()),
            original_packages: Some(vec![]),
            ..Default::default()
        };
        assert!(static_library(&valid, false).is_ok());
        assert!(static_library(&valid, true).is_err());
        for edit in [
            |p: &mut AndroidPackage| p.target_sdk_version = 25,
            |p: &mut AndroidPackage| p.original_packages = Some(vec![None]),
            |p: &mut AndroidPackage| p.library_names.push("dynamic".into()),
            |p: &mut AndroidPackage| p.shared_user_id = Some("group".into()),
            |p: &mut AndroidPackage| p.activities.push(Default::default()),
            |p: &mut AndroidPackage| p.services.push(Default::default()),
            |p: &mut AndroidPackage| p.providers.push(Default::default()),
            |p: &mut AndroidPackage| p.receivers.push(Default::default()),
            |p: &mut AndroidPackage| p.permission_groups.push(Default::default()),
            |p: &mut AndroidPackage| p.attributions.push(Default::default()),
            |p: &mut AndroidPackage| p.permissions.push(Default::default()),
            |p: &mut AndroidPackage| p.protected_broadcasts.push("protected".into()),
            |p: &mut AndroidPackage| p.overlay_target = Some("target".into()),
        ] {
            let mut pkg = valid.clone();
            edit(&mut pkg);
            assert!(static_library(&pkg, false).is_err());
            pkg.static_shared_library_name = None;
            assert!(static_library(&pkg, true).is_ok());
        }
    }
}
