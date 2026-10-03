//! Known data APK boot admission, ported from android-16.0.0_r1
//! InstallPackageHelper.assertPackageIsValid/adjustScanFlags and ScanPackageUtils.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiScanContext, AbiScanMode, Code, CompletedScanMetadata, DataCode, DisabledSystemMetadata,
    Error, Identity, Kind, LibraryCompatibility, Partition, ScanMetadataCompletion, ScanPolicy,
    SettingUpdate, SigningError, SigningScan, User, application_flags,
};
use crate::package::{restrictions::UserState, sign::SigningDetails, write::Apks};
use std::collections::{BTreeMap, BTreeSet};

/// Policy and metadata owners resolved before boot data admission.
pub struct DataScanInputs<'a> {
    pub factory: Option<&'a DisabledSystemMetadata>,
    pub platform: &'a SigningDetails,
    pub vendor_sdk: i32,
    pub compatibility: &'a LibraryCompatibility,
    pub remove_test_base: Option<bool>,
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

impl SigningScan {
    /// Remove rejected parse/signature inputs using their original scan paths.
    /// Earlier deletions and pending resource retries survive a later failure.
    pub fn clean_invalid_data_inputs(
        image: &super::DataImage,
        resources: &crate::package::owner::resources::CodeResources,
        incremental_paths: &BTreeSet<String>,
    ) -> Result<(), SigningError> {
        for rejected in &image.rejected {
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
        let system_identity = Identity::select(&raw.parsed, &self.settings, true);
        let factory_setting = self
            .settings
            .disabled_system_packages
            .iter()
            .find(|p| p.name == system_identity.internal_name);
        match (factory_setting, inputs.factory) {
            (Some(setting), Some(factory)) if setting == &factory.record.settings => {}
            (None, None) => {}
            _ => {
                return Err(fatal(
                    "factory",
                    "verified factory metadata disagrees with current disabled setting".into(),
                ));
            }
        }
        let updated = factory_setting.is_some();
        let identity = Identity::select(&raw.parsed, &self.settings, updated);
        let previous = self.settings.packages.iter()
            .find(|p| p.name == identity.internal_name).cloned()
            .ok_or_else(|| fail("require-known", "Application package not found; ignoring (INSTALL_FAILED_INVALID_INSTALL_LOCATION)".into()))?;
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
        if let Some(factory) = inputs.factory {
            policy.inherit_system_setting(&factory.record.settings);
        }
        policy.adjust_shared_uid_privilege(
            &code.parsed,
            &code.signing,
            inputs.platform,
            &self.identities,
            inputs.vendor_sdk,
        );
        policy
            .apply(
                &mut code.parsed,
                &code.signing,
                Some(inputs.platform),
                updated,
                apks,
                inputs.compatibility,
                inputs.remove_test_base,
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
