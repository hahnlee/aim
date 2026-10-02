//! Initial system APK scan, ported from android-16.0.0_r1 InitAppsHelper,
//! InstallPackageHelper and ScanPackageUtils (#702/#707/#812).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiPolicy, AbiScanContext, AbiScanMode, CompletedScanMetadata, Error, Image, Kind,
    LibraryCompatibility, NativeLibraryEnvironment, NativeLibraryInstallPolicy, ScanClock,
    ScanMetadataCompletion, ScanPolicy, SettingMetadata, SigningError, SigningScan, UserPolicy,
    application_flags,
};
use crate::package::{
    pkg::AndroidPackage, settings::UsesSdkLibrary, system_config::SystemConfig, write::Apks,
};

/// Boot/image owners' inputs, resolved before starting the package scan.
pub struct FirstBootSystemInputs<'a> {
    /// Settings/UIDs prepared by the preceding APEX scan, not saved APK state.
    pub apex_settings: &'a crate::package::settings::Settings,
    pub first_api_level: i32,
    pub vendor_sdk: i32,
    pub abi_policy: &'a AbiPolicy,
    pub compatibility: &'a LibraryCompatibility,
    pub preferred_abi: &'a str,
    pub app_lib32_install_dir: &'a str,
    pub platform_runtime_64bit: bool,
    pub install: NativeLibraryInstallPolicy,
    pub clock: ScanClock,
    pub factory_test: bool,
    pub users: UserPolicy<'a>,
    /// DomainVerificationManagerInternal.generateNewId, supplied by its owner.
    pub new_domain_id: &'a dyn Fn() -> Result<[u8; 16], String>,
}

/// Completed system APK candidates in scan order, after APEX identity setup.
/// Data APK selection, library graph finalization, persistence and publication
/// follow this phase; these candidates are not a served query snapshot.
#[derive(Debug)]
pub struct SystemImageScan {
    pub owner: SigningScan,
    pub packages: Vec<CompletedScanMetadata>,
    pub rejected: Vec<super::Rejected>,
}

impl SystemImageScan {
    /// Consume the image's physical directory order without persisted package
    /// order or a parser feed. A failed owner/policy gate returns an error,
    /// rather than admitting an incomplete package or publishing partial state.
    pub fn first_boot(
        image: Image,
        apks: &Apks,
        config: &SystemConfig,
        inputs: FirstBootSystemInputs<'_>,
    ) -> Result<Self, SigningError> {
        let fail = |package: String, path: String, phase, message| {
            SigningError::Rejected(Error {
                package,
                path,
                phase,
                message,
            })
        };
        if inputs
            .apex_settings
            .packages
            .iter()
            .chain(&inputs.apex_settings.disabled_system_packages)
            .any(|package| {
                !package.code_path.ends_with(".apex") && !package.code_path.ends_with(".capex")
            })
        {
            return Err(fail(
                String::new(),
                String::new(),
                "apex",
                "initial APEX ownership contains APK settings".into(),
            ));
        }
        let platform = image
            .packages
            .iter()
            .find(|code| {
                code.location.kind == Kind::Framework && code.parsed.package_name == "android"
            })
            .ok_or_else(|| {
                fail(
                    "android".into(),
                    "/system/framework".into(),
                    "framework",
                    "framework package was not loaded".into(),
                )
            })?
            .signing
            .clone();
        let should_stop_system_packages = apks
            .platform
            .framework_boolean("config_stopSystemPackagesByDefault")
            .map_err(|message| fail(String::new(), String::new(), "image-policy", message))?;
        let mut owner = SigningScan::new(config, inputs.apex_settings, inputs.first_api_level)
            .map_err(|error| {
                fail(
                    String::new(),
                    String::new(),
                    "settings",
                    format!("cannot initialize scan ownership: {error:?}"),
                )
            })?;
        let mut packages = Vec::new();
        let mut platform_loaded = false;
        for mut code in image.packages {
            let mut policy = ScanPolicy::for_location(&code.location);
            if !platform_loaded
                && policy.needs_shared_uid_privilege_check(
                    &code.parsed,
                    &owner.identities,
                    inputs.vendor_sdk,
                )
            {
                return Err(fail(
                    code.parsed.package_name.clone(),
                    code.location.path.clone(),
                    "policy",
                    "shared UID privilege requires the scanned platform signing owner".into(),
                ));
            }
            policy.adjust_shared_uid_privilege(
                &code.parsed,
                &code.signing,
                &platform,
                &owner.identities,
                inputs.vendor_sdk,
            );
            policy
                .apply(
                    &mut code.parsed,
                    &code.signing,
                    platform_loaded.then_some(&platform),
                    false,
                    apks,
                    inputs.compatibility,
                    None,
                )
                .map_err(|message| {
                    fail(
                        code.parsed.package_name.clone(),
                        code.location.path.clone(),
                        "policy",
                        message,
                    )
                })?;
            let reject = |phase, message| {
                fail(
                    code.parsed.package_name.clone(),
                    code.location.path.clone(),
                    phase,
                    message,
                )
            };
            let host = (apks.files)(&code.location.path)
                .ok_or_else(|| reject("location", "scan code path not mapped".into()))?;
            let code_is_directory = std::fs::metadata(host)
                .map_err(|e| reject("location", e.to_string()))?
                .is_dir();
            let native_environment = NativeLibraryEnvironment {
                preferred_abi: inputs.preferred_abi,
                app_lib32_install_dir: inputs.app_lib32_install_dir,
                code_is_directory,
                canonical_source: None,
            };
            let (flags, private_flags) = application_flags(&code.parsed, false);
            let metadata = SettingMetadata {
                code_path: code.location.path.clone(),
                legacy_native_library_path: None,
                primary_cpu_abi: None,
                secondary_cpu_abi: None,
                version_code: (i64::from(code.parsed.version_code_major) << 32)
                    | i64::from(code.parsed.version_code as u32),
                flags,
                private_flags,
                last_modified_time: 0,
                uses_sdk_libraries: sdk_libraries(&code.parsed)
                    .map_err(|e| reject("metadata", e))?,
                uses_static_libraries: static_libraries(&code.parsed)
                    .map_err(|e| reject("metadata", e))?,
                mime_groups: code.parsed.mime_groups.clone(),
                domain_set_id: (inputs.new_domain_id)().map_err(|e| reject("domain", e))?,
                target_sdk_version: code.parsed.target_sdk_version,
                restrict_update_hash: code.parsed.restrict_update_hash.clone(),
            };
            let is_platform =
                code.location.kind == Kind::Framework && code.parsed.package_name == "android";
            let mut users = inputs.users;
            users.stopped_system_app =
                initial_stopped(&code.parsed, config, should_stop_system_packages);
            let completed = owner.scan_new_system(
                &code,
                metadata,
                users,
                apks,
                ScanMetadataCompletion {
                    abi_policy: inputs.abi_policy,
                    native_environment: &native_environment,
                    context: AbiScanContext {
                        mode: AbiScanMode::Existing {
                            first_boot_or_upgrade: true,
                            old_was_stub: false,
                            saved: None,
                        },
                        system: true,
                        updated: false,
                        override_abi: None,
                        platform_runtime_64bit: is_platform
                            .then_some(inputs.platform_runtime_64bit),
                    },
                    install: inputs.install,
                    destination: None,
                    clock: inputs.clock,
                    factory_test: inputs.factory_test,
                },
            )?;
            platform_loaded |= is_platform;
            packages.push(completed);
        }
        Ok(Self {
            owner,
            packages,
            rejected: image.rejected,
        })
    }
}

fn initial_stopped(pkg: &AndroidPackage, config: &SystemConfig, enabled: bool) -> bool {
    enabled
        && pkg.package_name != "android"
        && !pkg.is(crate::package::pkg::booleans::OVERLAY_IS_STATIC)
        && !config
            .initial_non_stopped_system_packages
            .contains(&pkg.package_name)
        && pkg.activities.iter().any(|activity| {
            activity.main.enabled
                && activity.main.exported
                && activity.main.component.intents.iter().any(|intent| {
                    intent.filter.categories.as_ref().is_some_and(|categories| {
                        categories
                            .iter()
                            .any(|category| category == "android.intent.category.LAUNCHER")
                    })
                })
        })
}

pub(super) fn sdk_libraries(pkg: &AndroidPackage) -> Result<Vec<UsesSdkLibrary>, String> {
    let versions = pkg
        .uses_sdk_libraries_versions_major
        .as_deref()
        .unwrap_or_default();
    let optional = pkg
        .uses_sdk_libraries_optional
        .as_deref()
        .unwrap_or_default();
    if versions.len() != pkg.uses_sdk_libraries.len() || optional.len() != versions.len() {
        return Err("SDK library metadata arrays disagree".into());
    }
    Ok(pkg
        .uses_sdk_libraries
        .iter()
        .zip(versions)
        .zip(optional)
        .map(|((name, version), optional)| UsesSdkLibrary {
            name: name.clone(),
            version_major: *version,
            optional: *optional,
        })
        .collect())
}

pub(super) fn static_libraries(pkg: &AndroidPackage) -> Result<Vec<(String, i64)>, String> {
    let versions = pkg
        .uses_static_libraries_versions
        .as_deref()
        .unwrap_or_default();
    if versions.len() != pkg.uses_static_libraries.len() {
        return Err("static library metadata arrays disagree".into());
    }
    Ok(pkg
        .uses_static_libraries
        .iter()
        .cloned()
        .zip(versions.iter().copied())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        intent_filter::ParsedIntentInfo,
        pkg::{Activity, booleans},
    };

    #[test]
    fn initial_stopped_requires_an_enabled_exported_launcher_and_image_policy() {
        let mut pkg = AndroidPackage {
            package_name: "fixture".into(),
            ..Default::default()
        };
        let mut activity = Activity::default();
        let mut intent = ParsedIntentInfo::default();
        intent.filter.categories = Some(vec!["android.intent.category.LAUNCHER".into()]);
        // The original checks categories only; it does not require MAIN.
        intent.filter.actions.push("fixture.other.ACTION".into());
        activity.main.component.intents.push(intent);
        pkg.activities.push(activity);
        let config = SystemConfig::default();
        for enabled in [false, true] {
            for exported in [false, true] {
                for policy in [false, true] {
                    pkg.activities[0].main.enabled = enabled;
                    pkg.activities[0].main.exported = exported;
                    assert_eq!(
                        initial_stopped(&pkg, &config, policy),
                        enabled && exported && policy
                    );
                }
            }
        }
        let mut config = config;
        config
            .initial_non_stopped_system_packages
            .insert("fixture".into());
        assert!(!initial_stopped(&pkg, &config, true));
        config.initial_non_stopped_system_packages.clear();
        pkg.booleans |= booleans::OVERLAY_IS_STATIC;
        assert!(!initial_stopped(&pkg, &config, true));
        pkg.booleans &= !booleans::OVERLAY_IS_STATIC;
        pkg.package_name = "android".into();
        assert!(!initial_stopped(&pkg, &config, true));
        pkg.package_name = "fixture".into();
        pkg.activities[0].main.component.intents[0]
            .filter
            .categories = Some(vec!["android.intent.category.DEFAULT".into()]);
        assert!(!initial_stopped(&pkg, &config, true));
    }

    #[test]
    fn constructor_library_inputs_reject_misaligned_arrays() {
        let mut pkg = AndroidPackage::default();
        assert!(sdk_libraries(&pkg).unwrap().is_empty());
        assert!(static_libraries(&pkg).unwrap().is_empty());
        pkg.uses_sdk_libraries.push("sdk".into());
        assert!(sdk_libraries(&pkg).is_err());
        pkg.uses_sdk_libraries_versions_major = Some(vec![7]);
        assert!(sdk_libraries(&pkg).is_err());
        pkg.uses_sdk_libraries_optional = Some(vec![true]);
        assert_eq!(
            sdk_libraries(&pkg).unwrap(),
            vec![UsesSdkLibrary {
                name: "sdk".into(),
                version_major: 7,
                optional: true
            }]
        );
        pkg.uses_static_libraries.push("static".into());
        assert!(static_libraries(&pkg).is_err());
        pkg.uses_static_libraries_versions = Some(vec![9]);
        assert_eq!(static_libraries(&pkg).unwrap(), vec![("static".into(), 9)]);
    }
}
