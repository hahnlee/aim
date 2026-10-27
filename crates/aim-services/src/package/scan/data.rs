//! Known data APK boot admission, ported from android-16.0.0_r1
//! InstallPackageHelper.assertPackageIsValid/adjustScanFlags,
//! cleanupDisabledPackageSettings and ScanPackageUtils.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiScanContext, AbiScanMode, Code, CompletedScanMetadata, DataCode, DisabledSystemMetadata,
    Error, Identity, Kind, LibraryCompatibility, Partition, ScanMetadataCompletion, ScanPolicy,
    SettingUpdate, SigningError, SigningScan, User, application_flags,
};
use crate::package::{restrictions::UserState, sign::SigningDetails, write::Apks};
use std::collections::{BTreeMap, BTreeSet};

/// Pre-publication side owners for RemovePackageHelper's USER_ALL path.
/// Static AppsFilter relations are built from the surviving scan inventory;
/// interaction grants and restored preferred choices already have live owners.
pub struct BootRemovalOwners<'a> {
    pub implicit: &'a mut crate::package::apps_filter::ImplicitAccess,
    pub preferred: &'a std::sync::Mutex<crate::package::owner::Store>,
    pub keystore: &'a crate::package::owner::keystore::KeystoreCleanup,
    /// Post the background notification even if a later removal step fails.
    /// Home updates are ignored before systemReady by PreferredActivityHelper.
    pub preferred_changed: &'a dyn Fn(),
    pub permissions:
        &'a dyn Fn(&SigningScan, &crate::package::settings::Package, &[User]) -> Result<(), String>,
    pub migration: super::SharedUidMigration,
}

/// Policy and metadata owners resolved before boot data admission.
pub struct DataScanInputs<'a> {
    pub factory: Option<&'a DisabledSystemMetadata>,
    pub platform: &'a SigningDetails,
    pub vendor_sdk: i32,
    pub compatibility: &'a LibraryCompatibility,
    /// Called only for non-system code after manifest policy, when test.base
    /// is absent from the boot classpath.
    pub remove_test_base:
        &'a dyn Fn(&crate::package::pkg::AndroidPackage, bool) -> Result<Option<bool>, String>,
    /// InitAppsHelper's selected system packages awaiting a better data APK.
    pub expecting_better: &'a BTreeSet<String>,
    pub new_domain_id: &'a dyn Fn() -> Result<[u8; 16], String>,
    /// The saved setting and effective system/update ABI flags are rebound
    /// here to current ownership, rather than trusted from the caller.
    pub completion: ScanMetadataCompletion<'a>,
}

#[derive(Debug)]
pub enum DataCandidateOutcome {
    Accepted(CompletedScanMetadata),
    /// Actual code removal succeeded. Settings/factory fallback follows later.
    Removed(SigningError),
}

/// The boot owners for data iteration and missing-update factory recovery.
pub struct DataImageScanInputs<'a> {
    pub certificates: super::CertificateScanPolicy,
    pub seinfo: super::SeInfoScan<'a>,
    pub factories: &'a super::SystemImagePackages,
    pub platform: &'a SigningDetails,
    pub vendor_sdk: i32,
    pub abi_policy: &'a super::AbiPolicy,
    pub compatibility: &'a LibraryCompatibility,
    pub preferred_abi: &'a str,
    pub app_lib32_install_dir: &'a str,
    pub install: super::NativeLibraryInstallPolicy,
    pub clock: super::ScanClock,
    pub factory_test: bool,
    pub users: &'a BTreeMap<String, BTreeMap<i32, UserState>>,
    pub all_users: Option<&'a [User]>,
    pub first_boot_or_upgrade: bool,
    pub old_stub_packages: &'a BTreeSet<String>,
    pub expecting_better: &'a BTreeSet<String>,
    pub is_incremental: &'a dyn Fn(&str) -> Result<bool, String>,
    pub remove_test_base:
        &'a dyn Fn(&crate::package::pkg::AndroidPackage, bool) -> Result<Option<bool>, String>,
    pub destinations: &'a BTreeMap<String, super::NativeLibraryDestination<'a>>,
    pub resources: &'a crate::package::owner::resources::CodeResources,
    pub removal: Option<BootRemovalOwners<'a>>,
    pub new_domain_id: &'a dyn Fn() -> Result<[u8; 16], String>,
}

#[derive(Debug)]
pub struct DataImagePackages {
    pub packages: Vec<CompletedScanMetadata>,
    pub recovered: Vec<CompletedScanMetadata>,
    /// Original checkExistingBetterPackages reports these and proceeds.
    /// System code and earlier setting-enable effects remain intact.
    pub factory_rejected: Vec<(String, SigningError)>,
    pub rejected: Vec<super::Rejected>,
    pub removed: Vec<(String, SigningError)>,
}

impl SigningScan {
    /// Iterate verified data candidates, rescan surviving ex-system updates,
    /// then recover factories whose data code was absent or rejected. Earlier
    /// package/resource effects remain authoritative on a later error. This does
    /// not persist or publish a replica.
    pub fn scan_data_image(
        &mut self,
        image: super::DataImage,
        apks: &Apks,
        inputs: DataImageScanInputs<'_>,
    ) -> Result<DataImagePackages, SigningError> {
        self.scan_data_inputs(image, apks, inputs)
    }

    /// Collect each parsed candidate after selecting its current settings.
    pub fn scan_parsed_data_image(
        &mut self,
        image: super::DataImage<()>,
        apks: &Apks,
        inputs: DataImageScanInputs<'_>,
    ) -> Result<DataImagePackages, SigningError> {
        self.scan_data_inputs(image, apks, inputs)
    }

    fn scan_data_inputs<S>(
        &mut self,
        image: super::DataImage<S>,
        apks: &Apks,
        mut inputs: DataImageScanInputs<'_>,
    ) -> Result<DataImagePackages, SigningError> {
        let fatal = |package: String, path: String, phase, message| {
            SigningError::Fatal(Error {
                package,
                path,
                phase,
                message,
            })
        };
        if inputs.factories.retained_data.len() != inputs.factories.retained_code.len() {
            return Err(fatal(
                String::new(),
                String::new(),
                "factory",
                "retained factory code inventory disagrees with metadata".into(),
            ));
        }
        let mut expecting_better = inputs.expecting_better.clone();
        expecting_better.extend(
            inputs
                .factories
                .retained_data
                .iter()
                .map(|f| f.record.settings.name.clone()),
        );
        let mut packages = Vec::new();
        let mut admitted_code = BTreeMap::new();
        let mut recovered = Vec::new();
        let mut factory_rejected = Vec::new();
        let mut removed = Vec::new();
        let mut incremental = BTreeSet::new();
        // prepareSystemPackageCleanUp removes disappeared, non-updated system
        // settings before a data APK can be considered known (#822).
        let disappeared: Vec<_> = self
            .settings
            .packages
            .iter()
            .rev()
            .filter(|p| {
                p.flags & crate::package::settings::FLAG_SYSTEM != 0
                    && !self.has_scanned_package(&p.name)
                    && !self
                        .settings
                        .disabled_system_packages
                        .iter()
                        .any(|d| d.name == p.name)
            })
            .cloned()
            .collect();
        for package in disappeared {
            Self::destroy_removed_boot_storage(&package, &inputs)?;
            self.clear_removed_boot_metadata(&package)?;
            self.remove_boot_package_state(&package, &mut inputs)?;
        }
        for rejected in &image.rejected {
            if (inputs.is_incremental)(&rejected.location.path).map_err(|e| {
                fatal(
                    String::new(),
                    rejected.location.path.clone(),
                    "incremental",
                    e,
                )
            })? {
                incremental.insert(rejected.location.path.clone());
            }
        }
        Self::clean_data_rejections(&image.rejected, inputs.resources, &incremental)?;
        for entry in image.packages {
            let scan_incremental = (inputs.is_incremental)(&entry.scan_path)
                .map_err(|e| fatal(String::new(), entry.scan_path.clone(), "incremental", e))?;
            let collected = match self.collect_initial_code(&entry.code, apks, inputs.certificates)
            {
                Ok(code) => code,
                Err(error @ SigningError::Rejected(_)) => {
                    inputs
                        .resources
                        .clean(&entry.scan_path, scan_incremental)
                        .map_err(|e| {
                            fatal(String::new(), entry.scan_path.clone(), "data-cleanup", e)
                        })?;
                    removed.push((entry.scan_path, error));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let entry = DataCode {
                scan_path: entry.scan_path,
                code: collected,
            };
            let code = &entry.code;
            let identity =
                Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
            let factory = inputs
                .factories
                .retained_data
                .iter()
                .rev()
                .find(|f| f.record.settings.name == identity.internal_name);
            let host = (apks.files)(&code.location.path).ok_or_else(|| {
                fatal(
                    identity.internal_name.clone(),
                    code.location.path.clone(),
                    "location",
                    "data code path not mapped".into(),
                )
            })?;
            let environment = super::NativeLibraryEnvironment {
                preferred_abi: inputs.preferred_abi,
                app_lib32_install_dir: inputs.app_lib32_install_dir,
                code_is_directory: std::fs::metadata(host)
                    .map_err(|e| {
                        fatal(
                            identity.internal_name.clone(),
                            code.location.path.clone(),
                            "location",
                            e.to_string(),
                        )
                    })?
                    .is_dir(),
                canonical_source: None,
            };
            let incremental = scan_incremental;
            let completed = self.scan_data_candidate(
                &entry,
                inputs.users,
                inputs.all_users,
                apks,
                DataScanInputs {
                    factory,
                    platform: inputs.platform,
                    vendor_sdk: inputs.vendor_sdk,
                    compatibility: inputs.compatibility,
                    remove_test_base: inputs.remove_test_base,
                    expecting_better: &expecting_better,
                    new_domain_id: inputs.new_domain_id,
                    completion: ScanMetadataCompletion {
                        scan_as_instant_app: super::permission_admissions::boot_scan_as_instant(
                            Some(inputs.users),
                            &identity.internal_name,
                        ),
                        seinfo: inputs.seinfo,
                        abi_policy: inputs.abi_policy,
                        native_environment: &environment,
                        context: AbiScanContext {
                            mode: AbiScanMode::Existing {
                                first_boot_or_upgrade: inputs.first_boot_or_upgrade,
                                old_was_stub: inputs
                                    .old_stub_packages
                                    .contains(&identity.internal_name),
                                saved: None,
                            },
                            system: false,
                            updated: false,
                            override_abi: None,
                            platform_runtime_64bit: None,
                        },
                        install: inputs.install,
                        destination: inputs.destinations.get(&identity.internal_name),
                        clock: inputs.clock,
                        factory_test: inputs.factory_test,
                    },
                },
                inputs.resources,
                incremental,
            )?;
            match completed {
                DataCandidateOutcome::Accepted(completed) => {
                    admitted_code.insert(
                        completed.candidate.record.settings.name.clone(),
                        code.clone(),
                    );
                    packages.push(completed);
                }
                DataCandidateOutcome::Removed(error) => removed.push((entry.scan_path, error)),
            }
        }
        // cleanupDisabledPackageSettings runs after data admission and before
        // checkExistingBetterPackages. A missing factory retains its persisted
        // attributes for the first data scan, then loses them on this rescan.
        let disappeared: Vec<_> = self
            .settings
            .disabled_system_packages
            .iter()
            .filter(|p| {
                !inputs
                    .factories
                    .retained_data
                    .iter()
                    .any(|f| f.record.settings.name == p.name)
                    && !inputs
                        .factories
                        .packages
                        .iter()
                        .any(|f| f.candidate.record.settings.name == p.name)
            })
            .map(|p| p.name.clone())
            .collect();
        for name in disappeared.into_iter().rev() {
            self.settings
                .disabled_system_packages
                .retain(|p| p.name != name);
            self.disabled_users.remove(&name);
            let Some(at) = packages
                .iter()
                .position(|p| p.candidate.record.settings.name == name)
            else {
                let Some(package) = self
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == name)
                    .cloned()
                else {
                    continue;
                };
                Self::destroy_removed_boot_storage(&package, &inputs)?;
                self.clear_removed_boot_metadata(&package)?;
                self.remove_boot_package_state(&package, &mut inputs)?;
                continue;
            };
            let previous = packages.remove(at);
            self.withdraw_scanned_package(&previous.candidate.record);
            // InstallPackageHelper clears the retained runtime bit before
            // rescanning a former system update as an ordinary data APK.
            if let Some(setting) = self.settings.packages.iter_mut().find(|p| p.name == name) {
                setting.transient.updated_system_app = false;
            }
            let raw = &admitted_code[&name];
            let host = (apks.files)(&raw.location.path).ok_or_else(|| {
                fatal(
                    name.clone(),
                    raw.location.path.clone(),
                    "location",
                    "data code path not mapped".into(),
                )
            })?;
            let environment = super::NativeLibraryEnvironment {
                preferred_abi: inputs.preferred_abi,
                app_lib32_install_dir: inputs.app_lib32_install_dir,
                code_is_directory: std::fs::metadata(host)
                    .map_err(|e| {
                        fatal(
                            name.clone(),
                            raw.location.path.clone(),
                            "location",
                            e.to_string(),
                        )
                    })?
                    .is_dir(),
                canonical_source: None,
            };
            let mut users = inputs.users.clone();
            users.insert(name.clone(), previous.candidate.users);
            let completed = self.scan_known_data(
                raw,
                &users,
                inputs.all_users,
                apks,
                DataScanInputs {
                    factory: None,
                    platform: inputs.platform,
                    vendor_sdk: inputs.vendor_sdk,
                    compatibility: inputs.compatibility,
                    remove_test_base: inputs.remove_test_base,
                    expecting_better: &expecting_better,
                    new_domain_id: inputs.new_domain_id,
                    completion: ScanMetadataCompletion {
                        scan_as_instant_app: super::permission_admissions::boot_scan_as_instant(
                            Some(&users),
                            &name,
                        ),
                        seinfo: inputs.seinfo,
                        abi_policy: inputs.abi_policy,
                        native_environment: &environment,
                        context: AbiScanContext {
                            mode: AbiScanMode::Existing {
                                first_boot_or_upgrade: inputs.first_boot_or_upgrade,
                                old_was_stub: inputs.old_stub_packages.contains(&name),
                                saved: None,
                            },
                            system: false,
                            updated: false,
                            override_abi: None,
                            platform_runtime_64bit: None,
                        },
                        install: inputs.install,
                        destination: inputs.destinations.get(&name),
                        clock: inputs.clock,
                        factory_test: inputs.factory_test,
                    },
                },
            )?;
            packages.push(completed);
        }
        for (index, (factory, code)) in inputs
            .factories
            .retained_data
            .iter()
            .zip(&inputs.factories.retained_code)
            .enumerate()
        {
            let setting = &factory.record.settings;
            if inputs.factories.retained_data[index + 1..]
                .iter()
                .any(|f| f.record.settings.name == setting.name)
            {
                continue;
            }
            if self.has_scanned_package(&setting.name) {
                continue;
            }
            if self
                .settings
                .disabled_system_packages
                .iter()
                .find(|p| p.name == setting.name)
                != Some(setting)
            {
                return Err(fatal(
                    setting.name.clone(),
                    code.location.path.clone(),
                    "factory",
                    "fallback factory is no longer current".into(),
                ));
            }
            let version = (i64::from(code.parsed.version_code_major) << 32)
                | i64::from(code.parsed.version_code as u32);
            if Identity::select_for_location(&code.parsed, &self.settings, true, &code.location)
                != factory.record.identity
                || code.location.path != setting.code_path
                || version != setting.version_code
                || !factory.record.signing.unknown && code.signing != factory.record.signing
            {
                return Err(fatal(
                    setting.name.clone(),
                    code.location.path.clone(),
                    "factory",
                    "fallback requires original raw factory code".into(),
                ));
            }
            let domain_id = (inputs.new_domain_id)().map_err(|e| {
                fatal(
                    setting.name.clone(),
                    code.location.path.clone(),
                    "domain",
                    e,
                )
            })?;
            let enabled = self
                .enable_system_setting(&setting.name, domain_id)
                .ok_or_else(|| {
                    fatal(
                        setting.name.clone(),
                        code.location.path.clone(),
                        "factory",
                        "factory setting could not be enabled".into(),
                    )
                })?;
            let parsed =
                match apks.checked_parsed_path(&code.location.path, code.location.parse_flags()) {
                    Ok(parsed) => parsed,
                    Err(
                        crate::package::parse::Error::Parse(message)
                        | crate::package::parse::Error::Status(_, message),
                    ) => {
                        factory_rejected.push((
                            code.location.path.clone(),
                            SigningError::Rejected(Error {
                                package: setting.name.clone(),
                                path: code.location.path.clone(),
                                phase: "parse",
                                message,
                            }),
                        ));
                        continue;
                    }
                    Err(error) => {
                        return Err(fatal(
                            setting.name.clone(),
                            code.location.path.clone(),
                            "parse",
                            error.to_string(),
                        ));
                    }
                };
            let raw = Code {
                location: code.location.clone(),
                parsed,
                signing: (),
            };
            let collected = match self.collect_initial_code(&raw, apks, inputs.certificates) {
                Ok(code) => code,
                Err(error @ SigningError::Rejected(_)) => {
                    factory_rejected.push((code.location.path.clone(), error));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let code = &collected;
            let host = (apks.files)(&code.location.path).ok_or_else(|| {
                fatal(
                    setting.name.clone(),
                    code.location.path.clone(),
                    "location",
                    "factory code path not mapped".into(),
                )
            })?;
            let environment = super::NativeLibraryEnvironment {
                preferred_abi: inputs.preferred_abi,
                app_lib32_install_dir: inputs.app_lib32_install_dir,
                code_is_directory: std::fs::metadata(host)
                    .map_err(|e| {
                        fatal(
                            setting.name.clone(),
                            code.location.path.clone(),
                            "location",
                            e.to_string(),
                        )
                    })?
                    .is_dir(),
                canonical_source: None,
            };
            let completed = self.scan_enabled_factory(
                &factory.record,
                code,
                enabled,
                inputs.users,
                inputs.all_users,
                apks,
                super::UpdatedSystemBootInputs {
                    certificates: inputs.certificates,
                    completion: ScanMetadataCompletion {
                        scan_as_instant_app: super::permission_admissions::boot_scan_as_instant(
                            Some(inputs.users),
                            &code.parsed.package_name,
                        ),
                        seinfo: inputs.seinfo,
                        abi_policy: inputs.abi_policy,
                        native_environment: &environment,
                        context: AbiScanContext {
                            mode: AbiScanMode::Existing {
                                first_boot_or_upgrade: inputs.first_boot_or_upgrade,
                                old_was_stub: false,
                                saved: None,
                            },
                            system: true,
                            updated: false,
                            override_abi: None,
                            platform_runtime_64bit: None,
                        },
                        install: inputs.install,
                        destination: inputs.destinations.get(&setting.name),
                        clock: inputs.clock,
                        factory_test: inputs.factory_test,
                    },
                    compatibility: inputs.compatibility,
                    platform: Some(inputs.platform),
                    vendor_sdk: inputs.vendor_sdk,
                    remove_test_base: None,
                    resources: inputs.resources,
                    incremental: false,
                    new_domain_id: inputs.new_domain_id,
                },
            );
            match completed {
                Ok(completed) => recovered.push(completed),
                Err(SigningError::Rejected(reason))
                    if matches!(reason.phase, "validation" | "authorization") =>
                {
                    factory_rejected
                        .push((code.location.path.clone(), SigningError::Rejected(reason)));
                }
                Err(
                    error @ SigningError::NativeLibrary {
                        error: super::NativeLibraryError::Selection(_),
                        ..
                    },
                ) => factory_rejected.push((code.location.path.clone(), error)),
                Err(SigningError::Rejected(error)) => return Err(SigningError::Fatal(error)),
                Err(error) => return Err(error),
            }
        }
        Ok(DataImagePackages {
            packages,
            recovered,
            factory_rejected,
            rejected: image.rejected,
            removed,
        })
    }

    fn destroy_removed_boot_storage(
        package: &crate::package::settings::Package,
        inputs: &DataImageScanInputs<'_>,
    ) -> Result<(), SigningError> {
        let fail = |message: String| {
            SigningError::Fatal(Error {
                package: package.name.clone(),
                path: package.code_path.clone(),
                phase: "package-data",
                message,
            })
        };
        let users = inputs
            .all_users
            .ok_or_else(|| fail("removed package requires the resolved user inventory".into()))?;
        let states = inputs
            .users
            .get(&package.name)
            .ok_or_else(|| fail("removed package user inode state was not supplied".into()))?;
        inputs
            .resources
            .destroy_boot_app_storage(package, users, states)
            .map_err(fail)
    }

    fn clear_removed_boot_metadata(
        &mut self,
        package: &crate::package::settings::Package,
    ) -> Result<(), SigningError> {
        self.settings
            .domain_verification
            .clear_package(&package.name);
        crate::package::owner::key_sets::clear_package(&mut self.settings, &package.name).map_err(
            |message| {
                SigningError::Fatal(Error {
                    package: package.name.clone(),
                    path: package.code_path.clone(),
                    phase: "keysets",
                    message,
                })
            },
        )?;
        self.update_ownership.remove(&package.name);
        Ok(())
    }

    fn remove_boot_package_state(
        &mut self,
        package: &crate::package::settings::Package,
        inputs: &mut DataImageScanInputs<'_>,
    ) -> Result<(), SigningError> {
        let fail = |phase, message| {
            SigningError::Fatal(Error {
                package: package.name.clone(),
                path: package.code_path.clone(),
                phase,
                message,
            })
        };
        let owners = inputs.removal.as_mut().ok_or_else(|| {
            fail(
                "unsupported-operation",
                "boot package filter/preferred/keystore owners unavailable (#798)".into(),
            )
        })?;
        // destroy_removed_boot_storage validated the complete inventory first.
        let users = inputs.all_users.ok_or_else(|| {
            fail(
                "package-state",
                "removed package resolved users unavailable".into(),
            )
        })?;
        self.finish_boot_package_removal(package, users, owners)
    }

    fn finish_boot_package_removal(
        &mut self,
        package: &crate::package::settings::Package,
        users: &[User],
        owners: &mut BootRemovalOwners<'_>,
    ) -> Result<(), SigningError> {
        let fail = |phase, message| {
            SigningError::Fatal(Error {
                package: package.name.clone(),
                path: package.code_path.clone(),
                phase,
                message,
            })
        };
        let ids = users.iter().map(|user| user.id).collect::<Vec<_>>();
        owners.implicit.remove_package(package.uid_owner_id(), &ids);
        let changed = match owners
            .preferred
            .lock()
            .unwrap()
            .clear_all_package_preferred_activities(Some(&package.name))
        {
            Ok(changed) => changed,
            Err(error) => {
                if !error.changed_users.is_empty() {
                    (owners.preferred_changed)();
                }
                return Err(fail("preferred", error.error.to_string()));
            }
        };
        if !changed.is_empty() {
            (owners.preferred_changed)();
        }
        owners
            .keystore
            .post(package.app_id, &ids)
            .map_err(|message| fail("keystore", message))?;
        // The setting/UID stage precedes onPackageUninstalled. Its shared UID
        // owner retains surviving active and disabled members, as Settings does.
        self.remove_package_setting(&package.name)?;
        (owners.permissions)(self, package, users)
            .map_err(|message| fail("permissions", message))?;
        if let Some(group) = self
            .settings
            .shared_users
            .iter()
            .find(|group| Some(group.app_id) == package.shared_app_id())
            .map(|group| group.name.clone())
        {
            let disabled = self
                .settings
                .disabled_system_packages
                .iter()
                .filter_map(|setting| {
                    let code = self.disabled_loaded.get(&setting.name)?;
                    Some((
                        setting.name.clone(),
                        super::Record {
                            settings: setting.clone(),
                            parsed: code.package.clone(),
                            signing: code.collected_signing.clone(),
                            identity: Identity {
                                manifest_name: code.package.package_name.clone(),
                                internal_name: setting.name.clone(),
                                real_name: setting.real_name.clone(),
                            },
                            origin:
                                crate::package::owner::shared_users::ScanOrigin::SystemDirectory,
                        },
                    ))
                })
                .collect();
            self.migrate_single_shared_user(&group, owners.migration, &disabled)
                .map_err(SigningError::Fatal)?;
        }
        self.remove_renamed_package(package.real_name.as_deref());
        Ok(())
    }

    /// Remove rejected parse/signature inputs using their original scan paths.
    /// Earlier deletions and pending resource retries survive a later failure.
    pub fn clean_invalid_data_inputs(
        image: &super::DataImage,
        resources: &crate::package::owner::resources::CodeResources,
        incremental_paths: &BTreeSet<String>,
    ) -> Result<(), SigningError> {
        Self::clean_data_rejections(&image.rejected, resources, incremental_paths)
    }

    fn clean_data_rejections(
        rejected: &[super::Rejected],
        resources: &crate::package::owner::resources::CodeResources,
        incremental_paths: &BTreeSet<String>,
    ) -> Result<(), SigningError> {
        for rejected in rejected {
            resources
                .clean(
                    &rejected.location.path,
                    incremental_paths.contains(&rejected.location.path),
                )
                .map_err(|message| {
                    SigningError::Fatal(Error {
                        package: String::new(),
                        path: rejected.location.path.clone(),
                        phase: "data-cleanup",
                        message,
                    })
                })?;
        }
        Ok(())
    }

    /// Initial data admission plus actual invalid-code cleanup. Unimplemented
    /// or unavailable owner behavior stops the scan without deleting valid code.
    pub fn scan_data_candidate(
        &mut self,
        candidate: &DataCode,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[User]>,
        apks: &Apks,
        inputs: DataScanInputs<'_>,
        resources: &crate::package::owner::resources::CodeResources,
        incremental: bool,
    ) -> Result<DataCandidateOutcome, SigningError> {
        let path = &candidate.code.location.path;
        if path != &candidate.scan_path && !path.starts_with(&format!("{}/", candidate.scan_path)) {
            return Err(SigningError::Fatal(Error {
                package: candidate.code.parsed.package_name.clone(),
                path: candidate.scan_path.clone(),
                phase: "data-cleanup",
                message: "scan path does not own parsed data code".into(),
            }));
        }
        let error =
            match self.scan_known_data(&candidate.code, saved_users, all_users, apks, inputs) {
                Ok(accepted) => return Ok(DataCandidateOutcome::Accepted(accepted)),
                Err(SigningError::Rejected(error))
                    if matches!(
                        error.phase,
                        "require-known" | "validation" | "authorization"
                    ) =>
                {
                    SigningError::Rejected(error)
                }
                Err(
                    error @ SigningError::NativeLibrary {
                        error: super::NativeLibraryError::Selection(_),
                        ..
                    },
                ) => error,
                Err(SigningError::Rejected(error)) => return Err(SigningError::Fatal(error)),
                Err(error) => return Err(error),
            };
        resources
            .clean(&candidate.scan_path, incremental)
            .map_err(|message| {
                SigningError::Fatal(Error {
                    package: candidate.code.parsed.package_name.clone(),
                    path: candidate.scan_path.clone(),
                    phase: "data-cleanup",
                    message,
                })
            })?;
        Ok(DataCandidateOutcome::Removed(error))
    }

    /// Admit an already known data candidate through policy, signer/UID/library
    /// reconciliation and complete metadata. Failure leaves candidate settings
    /// unchanged; invalid-code cleanup and factory fallback belong to the loop.
    pub fn scan_known_data(
        &mut self,
        raw: &Code,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[User]>,
        apks: &Apks,
        inputs: DataScanInputs<'_>,
    ) -> Result<CompletedScanMetadata, SigningError> {
        let fail = |phase, message: String| {
            SigningError::Rejected(Error {
                package: raw.parsed.package_name.clone(),
                path: raw.location.path.clone(),
                phase,
                message,
            })
        };
        super::process_policy::validate(&raw.parsed)
            .map_err(|error| fail("validation", error.message))?;
        if raw.location.partition != Partition::Data
            || raw.location.kind != Kind::App
            || raw.location.apex.is_some()
            || raw.parsed.path.as_deref() != Some(raw.location.path.as_str())
            || super::physical_parse_flags(&raw.location.path).map_err(|e| fail("location", e))?
                != 0
            || raw
                .location
                .path
                .split('/')
                .skip(1)
                .any(|s| s.is_empty() || matches!(s, "." | ".."))
        {
            return Err(fail(
                "location",
                "data scan requires a canonical physical data APK".into(),
            ));
        }
        let fatal = |phase, message| {
            SigningError::Fatal(Error {
                package: raw.parsed.package_name.clone(),
                path: raw.location.path.clone(),
                phase,
                message,
            })
        };
        let system_identity =
            Identity::select_for_location(&raw.parsed, &self.settings, true, &raw.location);
        let factory_setting = self
            .settings
            .disabled_system_packages
            .iter()
            .find(|p| p.name == system_identity.internal_name);
        match (factory_setting, inputs.factory) {
            (Some(setting), Some(factory)) if setting == &factory.record.settings => {}
            // A disappeared factory has only its persisted setting at boot.
            // Authorization still checks its saved certificates; dynamic
            // declarations requiring parsed factory metadata fail explicitly.
            (Some(_), None) => {}
            (None, None) => {}
            _ => {
                return Err(fatal(
                    "factory",
                    "verified factory metadata disagrees with current disabled setting".into(),
                ));
            }
        }
        let updated = factory_setting.is_some();
        let identity =
            Identity::select_for_location(&raw.parsed, &self.settings, updated, &raw.location);
        let previous = self.settings.packages.iter()
            .find(|p| p.name == identity.internal_name).cloned()
            .ok_or_else(|| fail("require-known", "Application package not found; ignoring (INSTALL_FAILED_INVALID_INSTALL_LOCATION)".into()))?;
        if self.has_scanned_package(&identity.internal_name) {
            return Err(fail(
                "validation",
                format!(
                    "Application package {} already installed; skipping duplicate (INSTALL_FAILED_DUPLICATE_PACKAGE)",
                    identity.internal_name
                ),
            ));
        }
        if !inputs.expecting_better.contains(&identity.internal_name)
            && previous.code_path != raw.location.path
        {
            return Err(fail(
                "require-known",
                format!(
                    "Application package found at {} but expected at {}; ignoring (INSTALL_FAILED_PACKAGE_CHANGED)",
                    raw.location.path, previous.code_path
                ),
            ));
        }
        let AbiScanMode::Existing {
            first_boot_or_upgrade,
            old_was_stub,
            ..
        } = inputs.completion.context.mode
        else {
            return Err(fatal(
                "native-library",
                "boot data scan requires existing-package ABI mode".into(),
            ));
        };
        if !saved_users.contains_key(&previous.name) {
            return Err(fatal(
                "setting",
                "saved package user states were not supplied".into(),
            ));
        }
        let mut code = Code {
            location: raw.location.clone(),
            parsed: raw.parsed.clone(),
            signing: raw.signing.clone(),
        };
        let mut policy = ScanPolicy::default();
        if let Some(factory) = factory_setting {
            policy.inherit_system_setting(factory);
        }
        policy.adjust_shared_uid_privilege(
            &code.parsed,
            &code.signing,
            inputs.platform,
            &self.identities,
            inputs.vendor_sdk,
        );
        policy
            .apply_manifest(
                &mut code.parsed,
                &code.signing,
                Some(inputs.platform),
                updated,
                apks,
            )
            .map_err(|e| fatal("policy", e))?;
        let remove_test_base = if policy.system || inputs.compatibility.test_base_on_bootclasspath {
            None
        } else {
            (inputs.remove_test_base)(&code.parsed, policy.system)
                .map_err(|e| fatal("policy", e))?
        };
        inputs
            .compatibility
            .apply(
                &mut code.parsed,
                policy.system || updated,
                updated,
                remove_test_base,
            )
            .map_err(|e| fatal("policy", e))?;
        let (flags, private_flags) = application_flags(&code.parsed, updated);
        let update = SettingUpdate {
            code_path: code.location.path.clone(),
            legacy_native_library_path: previous.legacy_native_library_path.clone(),
            primary_cpu_abi: previous.primary_cpu_abi.clone(),
            secondary_cpu_abi: previous.secondary_cpu_abi.clone(),
            flags,
            private_flags,
            uses_sdk_libraries: super::boot::sdk_libraries(&code.parsed)
                .map_err(|e| fail("metadata", e))?,
            uses_static_libraries: super::boot::static_libraries(&code.parsed)
                .map_err(|e| fail("metadata", e))?,
            mime_groups: code.parsed.mime_groups.clone(),
            domain_set_id: (inputs.new_domain_id)().map_err(|e| fatal("domain", e))?,
            target_sdk_version: code.parsed.target_sdk_version,
            restrict_update_hash: code.parsed.restrict_update_hash.clone(),
        };
        let completion = ScanMetadataCompletion {
            scan_as_instant_app: inputs.completion.scan_as_instant_app
                || super::permission_admissions::boot_scan_as_instant(
                    Some(saved_users),
                    &previous.name,
                ),
            context: AbiScanContext {
                mode: AbiScanMode::Existing {
                    first_boot_or_upgrade,
                    old_was_stub,
                    saved: Some(&previous),
                },
                system: policy.system,
                updated,
                platform_runtime_64bit: None,
                ..inputs.completion.context
            },
            ..inputs.completion
        };
        self.scan_existing(
            &code,
            update,
            saved_users,
            all_users,
            inputs.factory.map(|f| &f.record),
            apks,
            completion,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::{Store, keystore::KeystoreCleanup};
    use aim_binder_driver::{Credentials, Device, Driver};
    use aim_binder_host::local::LocalProcess;
    use std::{fs, path::PathBuf, sync::Mutex};

    struct RemovalFixture {
        data: PathBuf,
        driver: std::sync::Arc<Driver>,
        process: std::sync::Arc<LocalProcess>,
        disk: Mutex<Store>,
    }
    impl RemovalFixture {
        fn new(shared: bool) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let data = std::env::temp_dir().join(format!(
                "aim-boot-removal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(data.join("system/users/0")).unwrap();
            fs::create_dir_all(data.join("system/users/10")).unwrap();
            let uid = if shared {
                "sharedUserId='10100'"
            } else {
                "userId='10100'"
            };
            let group = if shared {
                "<shared-user name='group' userId='10100'/>"
            } else {
                ""
            };
            let other = if shared {
                "sharedUserId='10100'"
            } else {
                "userId='10101'"
            };
            fs::write(data.join("system/packages.xml"), format!(
                "<packages><package name='a' realName='real.a' codePath='/system/app/a' publicFlags='1' {uid}/><package name='b' codePath='/system/app/b' {other}/>{group}<renamed-package new='real.a' old='a'/></packages>"
            )).unwrap();
            for user in [0, 10] {
                fs::write(data.join(format!("system/users/{user}/package-restrictions.xml")),
                    b"<package-restrictions><pkg name='a' inst='true'/><pkg name='b' stopped='true'/><preferred-activities><item name='a/.Always' always='true' set='0'><filter/></item><item name='a/.Last' always='false' set='0'><filter/></item><item name='b/.Main' always='true' set='0'><filter/></item></preferred-activities><persistent-preferred-activities><item name='a/.Policy'><filter/></item></persistent-preferred-activities><future/></package-restrictions>"
                ).unwrap();
            }
            let disk = Mutex::new(Store::open(&data, &[0, 10]).unwrap().unwrap());
            let driver = Driver::new();
            let process = LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid: 822,
                    euid: 1000,
                    security_context: None,
                },
            );
            Self {
                data,
                driver,
                process,
                disk,
            }
        }
        fn scan(&self) -> SigningScan {
            SigningScan::new(
                &Default::default(),
                &self.disk.lock().unwrap().state().settings,
                36,
            )
            .unwrap()
        }
        fn permissions(&self) -> crate::app_host::permission_manager::PermissionManager {
            let host = crate::app_host::AppHost::new(self.process.clone());
            crate::app_host::configure_test_host(
                &host,
                &self.data.join("image"),
                &self.data,
                &self.data,
                Box::new(|_| None),
            );
            for (relative, xml) in [
                (
                    "misc/apexdata/com.android.permission/access.abx",
                    "<access><permissions><permission name='a.NORMAL' packageName='a' protectionLevel='0' type='0'/><permission name='b.NORMAL' packageName='b' protectionLevel='0' type='0'/></permissions><permission-trees><permission name='a.tree' packageName='a' protectionLevel='0' type='0'/></permission-trees></access>",
                ),
                (
                    "misc_de/0/apexdata/com.android.permission/access.abx",
                    "<access><package-versions><package name='a' version='15'/><package name='b' version='15'/></package-versions><app-id-permissions><app-id id='10100'><permission name='b.NORMAL' flags='1'/></app-id><app-id id='10101'><permission name='b.NORMAL' flags='1'/><permission name='a.NORMAL' flags='1'/></app-id></app-id-permissions></access>",
                ),
            ] {
                let path = self.data.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(
                    path,
                    aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap())
                        .unwrap(),
                )
                .unwrap();
            }
            let user0 = self
                .data
                .join("misc_de/0/apexdata/com.android.permission/access.abx");
            let user10 = self
                .data
                .join("misc_de/10/apexdata/com.android.permission/access.abx");
            fs::create_dir_all(user10.parent().unwrap()).unwrap();
            fs::copy(user0, user10).unwrap();
            crate::app_host::permission_manager::PermissionManager::new(host)
        }
    }
    impl Drop for RemovalFixture {
        fn drop(&mut self) {
            self.driver.release(self.process.proc_handle());
            fs::remove_dir_all(&self.data).unwrap();
        }
    }
    fn users() -> [User; 2] {
        [0, 10].map(|id| User {
            id,
            pre_created: id == 10,
            adb_install_disallowed: false,
        })
    }

    // RemovePackageHelper.clearPackageStateForUserLIF/removePackageDataLIF:
    // side owners precede setting deletion; permission uninstall follows it.
    #[test]
    fn disappeared_system_boot_removal_completes_through_modern_permission_owner() {
        for shared in [false, true] {
            let fixture = RemovalFixture::new(shared);
            let mut scan = fixture.scan();
            let package = scan.settings.packages[0].clone();
            let unrelated = scan.settings.packages[1].clone();
            let keystore = KeystoreCleanup::new(fixture.process.clone());
            let mut implicit = crate::package::apps_filter::ImplicitAccess::default();
            for (from, to) in [(10100, 10101), (10101, 10100), (1010100, 1010101)] {
                implicit.grant(from, to, false);
                implicit.grant(from, to, true);
            }
            let mut expected = crate::package::apps_filter::ImplicitAccess::default();
            implicit.grant(1110100, 1110101, true);
            expected.grant(1110100, 1110101, true);
            let preferred_changes = std::cell::Cell::new(0);
            let preferred_changed = || preferred_changes.set(preferred_changes.get() + 1);
            let permissions = fixture.permissions();
            let uninstall = |scan: &SigningScan,
                             package: &crate::package::settings::Package,
                             users: &[User]| {
                assert!(
                    !scan
                        .settings
                        .packages
                        .iter()
                        .any(|p| p.name == package.name)
                );
                assert_eq!(scan.settings.renamed_packages.len(), 1);
                assert_eq!(preferred_changes.get(), 1);
                assert_eq!(keystore.pending(), 2);
                permissions
                    .remove_boot_permissions(scan, package, users)
                    .map_err(|error| error.message)
            };
            scan.clear_removed_boot_metadata(&package).unwrap();
            scan.finish_boot_package_removal(
                &package,
                &users(),
                &mut BootRemovalOwners {
                    implicit: &mut implicit,
                    preferred: &fixture.disk,
                    keystore: &keystore,
                    preferred_changed: &preferred_changed,
                    permissions: &uninstall,
                    migration: super::super::SharedUidMigration::BestEffort,
                },
            )
            .unwrap();
            assert_eq!(implicit, expected);
            assert_eq!(preferred_changes.get(), 1);
            assert_eq!(keystore.pending(), 2);
            assert_eq!(scan.settings.packages, [unrelated]);
            assert_eq!(scan.identities.ids.get(10100).is_some(), shared);
            assert_eq!(scan.settings.shared_users.len(), usize::from(shared));
            assert!(scan.settings.renamed_packages.is_empty());
            let access = permissions.retained_access().unwrap();
            assert!(!access.system.permissions.iter().any(|p| p.package == "a"));
            assert!(
                !access
                    .system
                    .permission_trees
                    .iter()
                    .any(|p| p.package == "a")
            );
            assert!(
                !access
                    .user0
                    .package_versions
                    .iter()
                    .any(|(name, _)| name == "a")
            );
            assert_eq!(
                access.user0.permission_flags(10100, "b.NORMAL"),
                if shared { 1 } else { 0 }
            );
            assert_eq!(access.user0.permission_flags(10101, "b.NORMAL"), 1);
            assert_eq!(access.user0.permission_flags(10101, "a.NORMAL"), 0);
            if !shared {
                assert!(
                    !access
                        .user0
                        .app_id_permissions
                        .iter()
                        .any(|(id, _)| *id == 10100)
                );
            }
            for user in [0, 10] {
                let reloaded = aim_android_xml::read(
                    &fs::read(fixture.data.join(format!(
                        "misc_de/{user}/apexdata/com.android.permission/access.abx"
                    )))
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    crate::package::permissions::AccessUser::parse(&reloaded).unwrap(),
                    *access.user0
                );
            }
            let reopened = Store::open(&fixture.data, &[0, 10]).unwrap().unwrap();
            for (_, preferred) in reopened.native_preferred_users().unwrap() {
                let choices = preferred.preferred.entries();
                assert_eq!(choices.len(), 2);
                assert!(
                    choices
                        .iter()
                        .any(|p| p.component.package == "a" && !p.always)
                );
                assert!(choices.iter().any(|p| p.component.package == "b"));
                assert_eq!(preferred.persistent.entries().len(), 1);
            }
            // Boot's writeSettings=false leaves the global write to its caller.
            assert_eq!(reopened.state().settings.packages.len(), 2);
        }
    }

    #[test]
    fn permission_failure_preserves_posted_work_and_stops_before_rename_cleanup() {
        let fixture = RemovalFixture::new(false);
        let mut scan = fixture.scan();
        let package = scan.settings.packages[0].clone();
        let permissions = fixture.permissions();
        let before = permissions.retained_access().unwrap();
        fs::create_dir(
            fixture
                .data
                .join("misc_de/0/apexdata/com.android.permission/access.abx.new"),
        )
        .unwrap();
        let keystore = KeystoreCleanup::new(fixture.process.clone());
        let mut implicit = Default::default();
        let posted = std::cell::Cell::new(0);
        let error = scan
            .finish_boot_package_removal(
                &package,
                &users(),
                &mut BootRemovalOwners {
                    implicit: &mut implicit,
                    preferred: &fixture.disk,
                    keystore: &keystore,
                    preferred_changed: &|| posted.set(posted.get() + 1),
                    permissions: &|scan, package, users| {
                        permissions
                            .remove_boot_permissions(scan, package, users)
                            .map_err(|error| error.message)
                    },
                    migration: super::super::SharedUidMigration::BestEffort,
                },
            )
            .unwrap_err();
        assert!(matches!(error, SigningError::Fatal(e) if e.phase == "permissions"));
        assert_eq!(posted.get(), 1);
        assert_eq!(keystore.pending(), 2);
        assert!(!scan.settings.packages.iter().any(|p| p.name == "a"));
        assert_eq!(scan.settings.renamed_packages.len(), 1);
        let after = permissions.retained_access().unwrap();
        assert_eq!(after.user0, before.user0);
        assert!(!after.system.permissions.iter().any(|p| p.package == "a"));
    }

    #[test]
    fn preferred_failure_keeps_setting_and_does_not_post_keystore_work() {
        let fixture = RemovalFixture::new(false);
        let mut scan = fixture.scan();
        let package = scan.settings.packages[0].clone();
        let original = scan.clone();
        let path = fixture
            .data
            .join("system/users/10/package-restrictions.xml");
        let external = b"<package-restrictions external='preserve'/>";
        fs::write(&path, external).unwrap();
        let mut implicit = crate::package::apps_filter::ImplicitAccess::default();
        implicit.grant(10100, 10101, true);
        let keystore = KeystoreCleanup::new(fixture.process.clone());
        let preferred_changes = std::cell::Cell::new(0);
        let preferred_changed = || preferred_changes.set(preferred_changes.get() + 1);
        let error = scan
            .finish_boot_package_removal(
                &package,
                &users(),
                &mut BootRemovalOwners {
                    implicit: &mut implicit,
                    preferred: &fixture.disk,
                    keystore: &keystore,
                    preferred_changed: &preferred_changed,
                    permissions: &|_, _, _| panic!("preferred failure precedes permissions"),
                    migration: super::super::SharedUidMigration::BestEffort,
                },
            )
            .unwrap_err();
        assert!(matches!(error, SigningError::Fatal(e) if e.phase == "preferred"));
        assert_eq!(scan, original);
        assert_eq!(implicit, Default::default());
        assert_eq!(preferred_changes.get(), 1);
        assert_eq!(keystore.pending(), 0);
        assert_eq!(fs::read(path).unwrap(), external);
    }

    #[test]
    fn domain_removal_survives_a_later_keyset_owner_failure() {
        let root = aim_android_xml::read(b"<packages><package name='a' codePath='/system/app/a' userId='10100'><proper-signing-keyset identifier='7'/></package><domain-verifications><active><package-state packageName='a' id='00000000-0000-0000-0000-000000000001'/></active></domain-verifications></packages>").unwrap();
        let settings = crate::package::settings::Settings::parse(&root).unwrap();
        let package = settings.packages[0].clone();
        let mut restored = settings.clone();
        restored.packages[0].key_set_data = Default::default();
        let mut owner = SigningScan::new(
            &crate::package::system_config::SystemConfig::default(),
            &restored,
            36,
        )
        .unwrap();
        let restored_key_sets = owner.settings.key_sets.clone();
        owner.settings.packages[0].key_set_data = package.key_set_data.clone();
        // A native pool/counter invariant failure, not an absent saved role.
        owner.settings.key_sets.reference_counts = Some(std::collections::BTreeMap::from([(7, 1)]));
        let broken_key_sets = owner.settings.key_sets.clone();
        owner
            .update_ownership
            .add("a", &["shared".into(), "only-a".into()]);
        owner.update_ownership.add("b", &["shared".into()]);
        assert!(
            matches!(owner.clear_removed_boot_metadata(&package), Err(SigningError::Fatal(e)) if e.phase == "keysets")
        );
        assert!(owner.settings.domain_verification.active.is_empty());
        assert_eq!(owner.settings.packages, settings.packages);
        assert_eq!(owner.settings.key_sets, broken_key_sets);
        assert_eq!(owner.update_ownership.is_provider(Some("a")), Ok(true));
        assert_eq!(owner.update_ownership.is_denylisted("only-a"), Ok(true));
        owner.settings.key_sets = restored_key_sets;
        owner.clear_removed_boot_metadata(&package).unwrap();
        assert_eq!(owner.update_ownership.is_provider(Some("a")), Ok(false));
        assert_eq!(owner.update_ownership.is_denylisted("only-a"), Ok(false));
        assert_eq!(owner.update_ownership.is_denylisted("shared"), Ok(true));
    }
}
