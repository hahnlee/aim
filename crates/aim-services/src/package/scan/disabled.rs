//! Disabled factory scan, ported from android-16.0.0_r1
//! InstallPackageHelper.scanPackageForInitLI and ScanPackageUtils (#702/#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiScanMode, Code, Error, Identity, NativeLibraryError, NewSetting, PageSizeCompatPolicy,
    Record, ScanMetadataCompletion, ScanTime, SettingUpdate, SigningError, SigningScan,
    physical_parse_flags,
};
use crate::package::{
    owner::shared_users::ScanOrigin, parse, restrictions::UserState, write::Apks,
};
use std::collections::BTreeMap;

/// Factory metadata is not an active signer/library admission. Its verified
/// code is retained separately for the later updated-data signature gate.
#[derive(Debug)]
pub struct DisabledSystemMetadata {
    pub record: Record,
    pub users: BTreeMap<i32, UserState>,
    pub multi_arch_mismatch: bool,
    pub alignment_diagnostic: Option<String>,
}

impl SigningScan {
    /// Refresh a disabled factory setting without registering its libraries,
    /// replacing live signatures or adding it to a shared UID's active members.
    /// Manifest policy must already have run with updated-system policy enabled.
    pub fn scan_disabled_system(
        &mut self,
        code: &Code,
        update: SettingUpdate,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[super::User]>,
        apks: &Apks,
        inputs: ScanMetadataCompletion<'_>,
    ) -> Result<DisabledSystemMetadata, SigningError> {
        let identity = Identity::select(&code.parsed, &self.settings, true);
        let reject = |phase, message: String| {
            SigningError::Rejected(Error {
                package: identity.internal_name.clone(),
                path: code.location.path.clone(),
                phase,
                message,
            })
        };
        let flags = physical_parse_flags(&code.location.path).map_err(|e| reject("location", e))?;
        if flags & parse::PARSE_IS_SYSTEM_DIR == 0
            || update.code_path != code.location.path
            || code.parsed.path.as_deref() != Some(&*code.location.path)
        {
            return Err(reject(
                "location",
                "disabled factory code is not at its system path".into(),
            ));
        }
        let at = self
            .settings
            .disabled_system_packages
            .iter()
            .position(|p| p.name == identity.internal_name)
            .ok_or_else(|| reject("setting", "disabled factory setting is missing".into()))?;
        let saved = &self.settings.disabled_system_packages[at];
        let updated = saved.flags & crate::package::info::FLAG_UPDATED_SYSTEM_APP != 0;
        if !inputs.context.system
            || inputs.context.updated != updated
            || inputs.destination.is_some()
            || !matches!(inputs.context.mode, AbiScanMode::Existing { saved: Some(p), .. } if p == saved)
        {
            return Err(reject(
                "metadata",
                "factory completion does not describe the disabled setting".into(),
            ));
        }
        let group = if saved.shared_user {
            Some(
                self.settings
                    .shared_users
                    .iter()
                    .find(|g| g.app_id == saved.app_id)
                    .ok_or_else(|| {
                        reject("identity", "disabled shared UID owner is missing".into())
                    })?
                    .name
                    .as_str(),
            )
        } else {
            None
        };
        if group
            != super::signing::selected_shared_user(
                saved.shared_user,
                code.parsed.shared_user_id.as_deref(),
                code.parsed
                    .is(crate::package::pkg::booleans::LEAVING_SHARED_UID),
            )
        {
            return Err(reject(
                "identity",
                "factory scan requires replacing UID ownership (#804)".into(),
            ));
        }
        super::validate::static_library(&code.parsed, false)
            .map_err(|e| reject("validation", e))?;
        let users = saved_users.get(&saved.name).ok_or_else(|| {
            reject(
                "setting",
                "disabled package user states were not supplied".into(),
            )
        })?;
        let mut setting = NewSetting::update(saved, users, update, all_users, false);
        let mut parsed = code.parsed.clone();
        identity.apply(&mut parsed);
        let native_error = |error| SigningError::NativeLibrary {
            package: saved.name.clone(),
            path: code.location.path.clone(),
            error,
        };
        let scan = apks
            .scan_native_libraries(
                &parsed,
                inputs.abi_policy,
                inputs.native_environment,
                inputs.context,
            )
            .map_err(native_error)?;
        let mut multi_arch_mismatch = false;
        if let Some(scan) = scan {
            if scan.requires_extraction {
                return Err(reject(
                    "native-library",
                    "disabled factory scan requires native library extraction (#810)".into(),
                ));
            }
            multi_arch_mismatch = scan.multi_arch_mismatch;
            scan.apply_metadata(&mut parsed);
        }
        inputs
            .context
            .apply_setting(&parsed, &mut setting.package)
            .map_err(native_error)?;
        let policy = PageSizeCompatPolicy::from_platform(&apks.platform)
            .map_err(|e| native_error(NativeLibraryError::Input(e)))?;
        let paths =
            super::NativeLibraryPaths::derive(&parsed, inputs.native_environment, true, updated)
                .map_err(|e| native_error(NativeLibraryError::Input(e)))?;
        let install = inputs.install.package_flags(&parsed);
        let alignment_diagnostic = policy
            .apply_setting(
                &parsed,
                &mut setting.package,
                inputs.context,
                inputs.install.page_size,
                &inputs.abi_policy.bit64,
                &|| {
                    apks.native_library_alignment(
                        &parsed,
                        &inputs.abi_policy.bit64,
                        &paths,
                        install,
                    )
                },
            )
            .map_err(native_error)?;
        let file_time = apks
            .scan_file_time(&parsed)
            .map_err(|e| reject("code-time", e))?;
        super::enrich::apply(
            &mut setting.package,
            &parsed,
            &mut setting.users,
            ScanTime {
                current_time: -1,
                file_time,
                user_id: inputs.clock.user_id,
                update_time: inputs.clock.update_time,
            },
            true,
        );
        super::enrich::application(
            &mut setting.package,
            &mut parsed,
            inputs.factory_test,
            updated,
        );
        // scanPackageOnly preserves saved signatures. Strict recollection for
        // selected updated-system packages belongs to the version selector.
        self.settings.disabled_system_packages[at] = setting.package.clone();
        Ok(DisabledSystemMetadata {
            record: Record {
                settings: setting.package,
                parsed,
                signing: code.signing.clone(),
                identity,
                origin: ScanOrigin::SystemDirectory,
            },
            users: setting.users,
            multi_arch_mismatch,
            alignment_diagnostic,
        })
    }
}
