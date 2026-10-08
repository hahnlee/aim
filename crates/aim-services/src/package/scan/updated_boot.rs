//! Updated-system source completion in the native initial scan (#702/#816).
//! Ported from android-16.0.0_r1 InstallPackageHelper; Copyright (C) The
//! Android Open Source Project, Apache License 2.0.
use super::{
    AbiScanContext, AbiScanMode, Code, CompletedScanMetadata, Error, Identity,
    LibraryCompatibility, ScanMetadataCompletion, ScanPolicy, SettingUpdate, SigningError,
    SigningScan, UpdatedSystemScan, UpdatedSystemSource, User, application_flags,
};
use crate::package::{
    owner::resources::CodeResources, restrictions::UserState, sign::SigningDetails,
};
use std::collections::BTreeMap;

pub struct UpdatedSystemBootInputs<'a> {
    pub certificates: super::CertificateScanPolicy,
    pub completion: ScanMetadataCompletion<'a>,
    pub compatibility: &'a LibraryCompatibility,
    pub platform: Option<&'a SigningDetails>,
    pub vendor_sdk: i32,
    pub remove_test_base: Option<bool>,
    pub resources: &'a CodeResources,
    pub incremental: bool,
    pub new_domain_id: &'a dyn Fn() -> Result<[u8; 16], String>,
}

#[derive(Debug)]
pub enum UpdatedSystemBootOutcome {
    /// The caller scans the data code later, using the retained factory record
    /// for its signer gate. No active image candidate is admitted here.
    KeepData,
    Factory(CompletedScanMetadata),
}

impl SigningScan {
    /// Complete scan_updated_system's source decision with real cleanup,
    /// setting enable, fresh non-update manifest/library policy and the active
    /// factory scan. Code must be the original raw parsed image input.
    /// Resource/enable effects survive a later policy or scan failure, as in
    /// the original initial scan; publication and persistence still follow.
    pub fn complete_updated_system_boot(
        &mut self,
        selected: &UpdatedSystemScan,
        code: &Code,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[User]>,
        apks: &crate::package::write::Apks,
        inputs: UpdatedSystemBootInputs<'_>,
    ) -> Result<UpdatedSystemBootOutcome, SigningError> {
        let factory = &selected.factory.record;
        let reject = |message: String| {
            SigningError::Rejected(Error {
                package: factory.settings.name.clone(),
                path: code.location.path.clone(),
                phase: "updated-system-boot",
                message,
            })
        };
        let version = (i64::from(code.parsed.version_code_major) << 32)
            | i64::from(code.parsed.version_code as u32);
        if Identity::select_for_location(&code.parsed, &self.settings, true, &code.location)
            != factory.identity
            || code.location.path != factory.settings.code_path
            || version != factory.settings.version_code
            || !factory.signing.unknown && code.signing != factory.signing
            || self
                .settings
                .packages
                .iter()
                .find(|p| p.name == factory.settings.name)
                != Some(&selected.active)
            || !self
                .settings
                .disabled_system_packages
                .iter()
                .any(|p| p == &factory.settings)
        {
            return Err(reject(
                "source completion requires the selected raw factory code".into(),
            ));
        }
        if selected.source == UpdatedSystemSource::KeepData {
            return Ok(UpdatedSystemBootOutcome::KeepData);
        }
        if !matches!(inputs.completion.context.mode, AbiScanMode::Existing { .. }) {
            return Err(reject(
                "factory boot restoration requires initial-scan ABI ownership".into(),
            ));
        }
        let enabled = self.restore_updated_system_setting_with_id(
            selected,
            inputs.resources,
            inputs.incremental,
            inputs.new_domain_id,
        )?;
        let collected = self.collect_selected_initial_code(
            code,
            apks,
            inputs.certificates,
            Some(&selected.active),
        )?;
        self.scan_enabled_factory(
            factory,
            &collected,
            enabled,
            saved_users,
            all_users,
            apks,
            inputs,
        )
        .map(UpdatedSystemBootOutcome::Factory)
    }

    pub(super) fn scan_enabled_factory(
        &mut self,
        factory: &super::Record,
        code: &Code,
        enabled: crate::package::settings::Package,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[User]>,
        apks: &crate::package::write::Apks,
        inputs: UpdatedSystemBootInputs<'_>,
    ) -> Result<CompletedScanMetadata, SigningError> {
        let reject = |message: String| {
            SigningError::Rejected(Error {
                package: factory.settings.name.clone(),
                path: code.location.path.clone(),
                phase: "updated-system-boot",
                message,
            })
        };
        if self
            .settings
            .packages
            .iter()
            .find(|p| p.name == enabled.name)
            != Some(&enabled)
        {
            return Err(reject(
                "enabled factory setting changed before admission".into(),
            ));
        }
        let AbiScanMode::Existing {
            first_boot_or_upgrade,
            old_was_stub,
            ..
        } = inputs.completion.context.mode
        else {
            return Err(reject(
                "factory boot restoration requires initial-scan ABI ownership".into(),
            ));
        };
        let mut parsed = code.parsed.clone();
        let mut policy = ScanPolicy::for_location(&code.location);
        if let Some(platform) = inputs.platform {
            policy.adjust_shared_uid_privilege(
                &parsed,
                &code.signing,
                platform,
                &self.identities,
                inputs.vendor_sdk,
            );
        } else if policy.needs_shared_uid_privilege_check(
            &parsed,
            &self.identities,
            inputs.vendor_sdk,
        ) {
            return Err(reject(
                "shared UID privilege requires platform signing ownership".into(),
            ));
        }
        policy
            .apply(
                &mut parsed,
                &code.signing,
                inputs.platform,
                false,
                apks,
                inputs.compatibility,
                inputs.remove_test_base,
            )
            .map_err(reject)?;
        let identity = Identity::select_for_location(&parsed, &self.settings, true, &code.location);
        let admission = self
            .settings
            .packages
            .iter()
            .find(|p| p.name == identity.internal_name)
            .cloned();
        let (flags, private_flags) = application_flags(&parsed, false);
        let update = SettingUpdate {
            code_path: code.location.path.clone(),
            legacy_native_library_path: admission
                .as_ref()
                .and_then(|p| p.legacy_native_library_path.clone()),
            primary_cpu_abi: admission.as_ref().and_then(|p| p.primary_cpu_abi.clone()),
            secondary_cpu_abi: admission.as_ref().and_then(|p| p.secondary_cpu_abi.clone()),
            flags,
            private_flags,
            uses_sdk_libraries: super::boot::sdk_libraries(&parsed).map_err(reject)?,
            uses_static_libraries: super::boot::static_libraries(&parsed).map_err(reject)?,
            mime_groups: parsed.mime_groups.clone(),
            domain_set_id: (inputs.new_domain_id)().map_err(reject)?,
            target_sdk_version: parsed.target_sdk_version,
            restrict_update_hash: parsed.restrict_update_hash.clone(),
        };
        let code = Code {
            location: code.location.clone(),
            parsed,
            signing: code.signing.clone(),
        };
        let completion = ScanMetadataCompletion {
            scan_as_instant_app:inputs.completion.scan_as_instant_app||super::permission_admissions::boot_scan_as_instant(Some(saved_users),&factory.settings.name),
            context: AbiScanContext {
                mode: AbiScanMode::Existing {
                    first_boot_or_upgrade,
                    old_was_stub,
                    saved: admission.as_ref(),
                },
                system: true,
                updated: false,
                ..inputs.completion.context
            },
            ..inputs.completion
        };
        let completed = if admission.is_some() {
            self.scan_existing(
                &code,
                update,
                saved_users,
                all_users,
                None,
                apks,
                completion,
            )?
        } else {
            let metadata = super::SettingMetadata {
                code_path: update.code_path,
                legacy_native_library_path: None,
                primary_cpu_abi: None,
                secondary_cpu_abi: None,
                version_code: (i64::from(code.parsed.version_code_major) << 32)
                    | i64::from(code.parsed.version_code as u32),
                flags: update.flags,
                private_flags: update.private_flags,
                last_modified_time: 0,
                uses_sdk_libraries: update.uses_sdk_libraries,
                uses_static_libraries: update.uses_static_libraries,
                mime_groups: update.mime_groups,
                domain_set_id: update.domain_set_id,
                target_sdk_version: update.target_sdk_version,
                restrict_update_hash: update.restrict_update_hash,
            };
            self.scan_original_system(&code, metadata, saved_users, apks, completion)?
        };
        Ok(completed)
    }
}
