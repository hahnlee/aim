//! Initial system APK scan, ported from android-16.0.0_r1 InitAppsHelper,
//! InstallPackageHelper and ScanPackageUtils (#702/#707/#812).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiPolicy, AbiScanContext, AbiScanMode, Code, CompletedScanMetadata, DisabledSystemMetadata,
    Error, Identity, Image, Kind, LibraryCompatibility, NativeLibraryEnvironment,
    NativeLibraryInstallPolicy, ScanClock, ScanMetadataCompletion, ScanPolicy, SettingMetadata,
    SettingUpdate, SigningError, SigningScan, UpdatedSystemBootInputs, UpdatedSystemBootOutcome,
    UserPolicy, application_flags,
};
use crate::package::{
    pkg::AndroidPackage, settings::UsesSdkLibrary, system_config::SystemConfig, write::Apks,
};
use std::collections::{BTreeMap, BTreeSet};

/// Boot/image owners' inputs, resolved before starting the package scan.
pub struct FirstBootSystemInputs<'a> {
    pub seinfo: super::SeInfoScan<'a>,
    /// Verified original container inputs establishing UID-free ownership.
    pub apex_image: &'a super::ApexImage,
    /// ApexManager.notifyScanResult must finish before any APK directory scan.
    pub notify_apex_scan: &'a dyn Fn(&[super::ApexScanResult]) -> Result<(), String>,
    pub first_api_level: i32,
    pub certificates: super::CertificateScanPolicy,
    pub vendor_sdk: i32,
    /// SharedUidMigration image policy, resolved by the boot owner.
    pub shared_uid_migration: super::SharedUidMigration,
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

/// Restored APK/user and resource owners after APEX identity setup.
/// The mutable SigningScan supplied by the caller remains authoritative even
/// when a later package fails after earlier metadata or resource effects.
pub struct SavedSystemScanInputs<'a> {
    pub users: &'a BTreeMap<String, BTreeMap<i32, crate::package::restrictions::UserState>>,
    pub first_boot_or_upgrade: bool,
    pub old_stub_packages: &'a BTreeSet<String>,
    pub incremental_packages: &'a BTreeSet<String>,
    pub resources: &'a crate::package::owner::resources::CodeResources,
}

#[derive(Debug)]
pub struct SystemImagePackages {
    pub packages: Vec<CompletedScanMetadata>,
    /// Refreshed factories whose selected data code must be scanned next.
    pub retained_data: Vec<DisabledSystemMetadata>,
    /// Raw code remains available for checkExistingBetterPackages fallback.
    pub retained_code: Vec<Code>,
    pub rejected: Vec<super::Rejected>,
}

impl SigningScan {
    pub fn scan_saved_parsed_system_image(
        &mut self,
        image: Image<()>,
        apks: &Apks,
        config: &SystemConfig,
        inputs: FirstBootSystemInputs<'_>,
        saved: SavedSystemScanInputs<'_>,
    ) -> Result<SystemImagePackages, SigningError> {
        scan_system_image(self, image, apks, config, inputs, Some(saved))
    }

    pub fn scan_saved_system_image(
        &mut self,
        image: Image,
        apks: &Apks,
        config: &SystemConfig,
        inputs: FirstBootSystemInputs<'_>,
        saved: SavedSystemScanInputs<'_>,
    ) -> Result<SystemImagePackages, SigningError> {
        scan_system_image(self, image, apks, config, inputs, Some(saved))
    }
}

/// Completed system APK candidates in scan order, after APEX identity setup.
/// Data APK selection, library graph finalization, persistence and publication
/// follow this phase; these candidates are not a served query snapshot.
#[derive(Debug)]
pub struct SystemImageScan {
    pub owner: SigningScan,
    pub packages: Vec<CompletedScanMetadata>,
    pub apex: Vec<super::ApexScanResult>,
    pub rejected: Vec<super::Rejected>,
}

impl SystemImageScan {
    /// Read APK directories only after APEX registration and original notification.
    /// The loader preserves physical directory order without a parser feed.
    pub fn first_boot(
        load_image: impl FnOnce() -> Result<Image, Error>,
        apks: &Apks,
        config: &SystemConfig,
        inputs: FirstBootSystemInputs<'_>,
    ) -> Result<Self, SigningError> {
        Self::first_boot_inputs(load_image, apks, config, inputs)
    }

    pub fn first_boot_parsed(
        load_image: impl FnOnce() -> Result<Image<()>, Error>,
        apks: &Apks,
        config: &SystemConfig,
        inputs: FirstBootSystemInputs<'_>,
    ) -> Result<Self, SigningError> {
        Self::first_boot_inputs(load_image, apks, config, inputs)
    }

    fn first_boot_inputs<S>(
        load_image: impl FnOnce() -> Result<Image<S>, Error>,
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
        let mut owner = SigningScan::new(config, &Default::default(), inputs.first_api_level)
            .map_err(|error| {
                fail(
                    String::new(),
                    String::new(),
                    "settings",
                    format!("cannot initialize scan ownership: {error:?}"),
                )
            })?;
        let apex = owner.scan_initial_apex(apks, config, &inputs)?;
        (inputs.notify_apex_scan)(&apex)
            .map_err(|message| fail(String::new(), String::new(), "apex-notification", message))?;
        let image = load_image().map_err(SigningError::Rejected)?;
        let batch = scan_system_image(&mut owner, image, apks, config, inputs, None)?;
        Ok(Self {
            owner,
            packages: batch.packages,
            apex,
            rejected: batch.rejected,
        })
    }
}

fn scan_system_image<S>(
    owner: &mut SigningScan,
    image: Image<S>,
    apks: &Apks,
    config: &SystemConfig,
    inputs: FirstBootSystemInputs<'_>,
    saved: Option<SavedSystemScanInputs<'_>>,
) -> Result<SystemImagePackages, SigningError> {
    let fail = |package: String, path: String, phase, message| {
        SigningError::Rejected(Error {
            package,
            path,
            phase,
            message,
        })
    };
    if !image
        .packages
        .iter()
        .any(|code| code.location.kind == Kind::Framework && code.parsed.package_name == "android")
    {
        return Err(fail(
            "android".into(),
            "/system/framework".into(),
            "framework",
            "framework package was not loaded".into(),
        ));
    }
    let mut platform = crate::package::sign::SigningDetails::unknown();
    let should_stop_system_packages = apks
        .platform
        .framework_boolean("config_stopSystemPackagesByDefault")
        .map_err(|message| fail(String::new(), String::new(), "image-policy", message))?;
    let mut rejected = image.rejected;
    let mut packages = Vec::new();
    let mut retained_data = Vec::new();
    let mut retained_code = Vec::new();
    let mut old_stub_packages = saved
        .as_ref()
        .map(|s| s.old_stub_packages.clone())
        .unwrap_or_default();
    let mut platform_loaded = false;
    for input in image.packages {
        // Parser inputs have UNKNOWN signing until this selected scan owner
        // collects certificates. Disabled factory refresh precedes collection.
        let mut code = Code {
            location: input.location,
            parsed: input.parsed,
            signing: crate::package::sign::SigningDetails::unknown(),
        };
        if let Err(error) = super::process_policy::validate(&code.parsed) {
            rejected.push(super::Rejected { location: code.location.clone(), reason: error.message });
            continue;
        }
        owner.refresh_init_apex(&code);
        let identity =
            Identity::select_for_location(&code.parsed, &owner.settings, true, &code.location);
        let active = owner
            .settings
            .packages
            .iter()
            .find(|p| p.name == identity.internal_name)
            .cloned();
        let original = Identity::original_setting(&code.parsed, &owner.settings, &|name| {
            owner.has_scanned_package(name)
        })
        .cloned();
        // scanPackageForInitLI selects the original for source policy, while
        // scanPackageNew retains the incoming setting for final admission.
        let source_setting = original.as_ref().or(active.as_ref());
        let disabled_name = source_setting
            .map(|p| p.name.as_str())
            .unwrap_or(&identity.internal_name);
        let factory = owner
            .settings
            .disabled_system_packages
            .iter()
            .find(|p| p.name == disabled_name)
            .cloned();
        match owner.prepare_initial_shared_user(&code) {
            Ok(()) => {}
            Err(SigningError::Rejected(error))
                if !(code.location.kind == Kind::Framework
                    && code.parsed.package_name == "android") =>
            {
                rejected.push(super::Rejected {
                    location: code.location.clone(),
                    reason: error.message,
                });
                continue;
            }
            Err(error) => return Err(error),
        }
        if source_setting.is_some_and(|p| p.flags & crate::package::settings::FLAG_SYSTEM == 0) {
            return Err(fail(
                identity.internal_name,
                code.location.path.clone(),
                "system-source",
                "non-system promotion requires its removal/hide owner (#702)".into(),
            ));
        }
        let updated = source_setting.is_some() && factory.is_some();
        if !updated && owner.has_scanned_package(&identity.internal_name) {
            rejected.push(super::Rejected {
                location: code.location.clone(),
                reason: format!("Application package {} already installed; skipping duplicate (INSTALL_FAILED_DUPLICATE_PACKAGE)", identity.internal_name),
            });
            continue;
        }
        if active.is_none() && original.is_none() && factory.is_some() {
            owner.remove_stale_disabled_system(&code)?;
        }
        if !updated {
            code = match owner.collect_initial_code(&code, apks, inputs.certificates) {
                Ok(code) => code,
                Err(SigningError::Rejected(error))
                    if !(code.location.kind == Kind::Framework
                        && code.parsed.package_name == "android") =>
                {
                    rejected.push(super::Rejected {
                        location: code.location.clone(),
                        reason: error.message,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            if code.location.kind == Kind::Framework && code.parsed.package_name == "android" {
                platform = code.signing.clone();
            }
        }
        let raw = updated.then(|| Code {
            location: code.location.clone(),
            parsed: code.parsed.clone(),
            signing: code.signing.clone(),
        });
        let previous = if updated {
            factory.as_ref()
        } else {
            active.as_ref()
        };
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
                updated,
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
        let (flags, private_flags) = application_flags(&code.parsed, updated);
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
            uses_sdk_libraries: sdk_libraries(&code.parsed).map_err(|e| reject("metadata", e))?,
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
        let completion = ScanMetadataCompletion {
            scan_as_instant_app:super::permission_admissions::boot_scan_as_instant(saved.as_ref().map(|saved|saved.users),&identity.internal_name),
            seinfo: inputs.seinfo,
            abi_policy: inputs.abi_policy,
            native_environment: &native_environment,
            context: AbiScanContext {
                mode: AbiScanMode::Existing {
                    first_boot_or_upgrade: saved.as_ref().is_none_or(|s| s.first_boot_or_upgrade),
                    // The disabled-factory ScanRequest has a null oldPkg.
                    old_was_stub: !updated
                        && previous.is_some_and(|p| old_stub_packages.contains(&p.name)),
                    saved: previous,
                },
                system: true,
                updated: if updated {
                    factory.as_ref().unwrap().transient.updated_system_app
                } else {
                    false
                },
                override_abi: None,
                platform_runtime_64bit: is_platform.then_some(inputs.platform_runtime_64bit),
            },
            install: inputs.install,
            destination: None,
            clock: inputs.clock,
            factory_test: inputs.factory_test,
        };
        let update = || SettingUpdate {
            code_path: metadata.code_path.clone(),
            legacy_native_library_path: previous.and_then(|p| p.legacy_native_library_path.clone()),
            primary_cpu_abi: previous.and_then(|p| p.primary_cpu_abi.clone()),
            secondary_cpu_abi: previous.and_then(|p| p.secondary_cpu_abi.clone()),
            flags: metadata.flags,
            private_flags: metadata.private_flags,
            uses_sdk_libraries: metadata.uses_sdk_libraries.clone(),
            uses_static_libraries: metadata.uses_static_libraries.clone(),
            mime_groups: metadata.mime_groups.clone(),
            domain_set_id: metadata.domain_set_id,
            target_sdk_version: metadata.target_sdk_version,
            restrict_update_hash: metadata.restrict_update_hash.clone(),
        };
        let completed = if updated {
            let saved = saved.as_ref().ok_or_else(|| {
                reject(
                    "settings",
                    "updated system code requires restored user/resource owners".into(),
                )
            })?;
            let selected = owner.scan_updated_system(
                &code,
                update(),
                users.users,
                config,
                apks,
                completion,
            )?;
            let completion = ScanMetadataCompletion {
                scan_as_instant_app:super::permission_admissions::boot_scan_as_instant(Some(saved.users),&identity.internal_name),
                seinfo: inputs.seinfo,
                abi_policy: inputs.abi_policy,
                native_environment: &native_environment,
                context: AbiScanContext {
                    mode: AbiScanMode::Existing {
                        first_boot_or_upgrade: saved.first_boot_or_upgrade,
                        old_was_stub: active
                            .as_ref()
                            .is_some_and(|p| old_stub_packages.contains(&p.name)),
                        saved: active.as_ref(),
                    },
                    system: true,
                    updated: false,
                    override_abi: None,
                    platform_runtime_64bit: None,
                },
                install: inputs.install,
                destination: None,
                clock: inputs.clock,
                factory_test: inputs.factory_test,
            };
            match owner.complete_updated_system_boot(
                &selected,
                raw.as_ref().unwrap(),
                saved.users,
                users.users,
                apks,
                UpdatedSystemBootInputs {
                    certificates: inputs.certificates,
                    completion,
                    compatibility: inputs.compatibility,
                    platform: platform_loaded.then_some(&platform),
                    vendor_sdk: inputs.vendor_sdk,
                    remove_test_base: None,
                    resources: saved.resources,
                    incremental: active
                        .as_ref()
                        .is_some_and(|p| saved.incremental_packages.contains(&p.name)),
                    new_domain_id: inputs.new_domain_id,
                },
            )? {
                UpdatedSystemBootOutcome::KeepData => {
                    retained_data.push(selected.factory);
                    retained_code.push(raw.unwrap());
                    continue;
                }
                UpdatedSystemBootOutcome::Factory(completed) => completed,
            }
        } else if active.is_some() {
            let saved = saved.as_ref().ok_or_else(|| {
                reject(
                    "settings",
                    "existing system code requires restored user states".into(),
                )
            })?;
            owner.scan_existing(
                &code,
                update(),
                saved.users,
                users.users,
                None,
                apks,
                completion,
            )?
        } else if original.is_some() {
            let saved = saved.as_ref().ok_or_else(|| {
                reject(
                    "settings",
                    "original adoption requires restored user states".into(),
                )
            })?;
            owner.scan_original_system(&code, metadata, saved.users, apks, completion)?
        } else {
            owner.scan_new_system(&code, metadata, users, apks, completion)?
        };
        platform_loaded |= is_platform;
        let record = &completed.candidate.record;
        if record.parsed.is2(crate::package::pkg::booleans2::STUB) {
            old_stub_packages.insert(record.settings.name.clone());
        } else {
            old_stub_packages.remove(&record.settings.name);
        }
        packages.push(completed);
    }
    Ok(SystemImagePackages {
        packages,
        retained_data,
        retained_code,
        rejected,
    })
}

pub(super) fn initial_stopped(pkg: &AndroidPackage, config: &SystemConfig, enabled: bool) -> bool {
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
