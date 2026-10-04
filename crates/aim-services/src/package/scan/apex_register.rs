//! Initial APEX registration from android-16.0.0_r1 InstallPackageHelper and
//! ScanPackageUtils. Copyright (C) The Android Open Source Project, Apache 2.0.
use super::{
    AbiScanContext, AbiScanMode, Error, FirstBootSystemInputs, Identity, NativeLibraryEnvironment,
    NewPackageOutcome, NewSetting, Partition, Record, ScanMetadataCompletion, ScanPolicy,
    SettingMetadata, SettingUpdate, SigningError, SigningScan, Uid, application_flags,
};
use crate::package::{
    bootstrap::ApexPackage, owner::shared_users::ScanOrigin, parse, pkg::AndroidPackage,
    system_config::SystemConfig, write::Apks,
};

#[derive(Clone, Debug)]
pub struct ApexScanResult {
    pub info: ApexPackage,
    pub package: AndroidPackage,
}

impl SigningScan {
    /// Register containers before APK directories. This returns the original
    /// ApexManager notification inputs; notification/publication belong to the
    /// surrounding boot owner. Earlier accepted scans survive a later error.
    pub fn scan_initial_apex(
        &mut self,
        apks: &Apks,
        config: &SystemConfig,
        inputs: &FirstBootSystemInputs<'_>,
    ) -> Result<Vec<ApexScanResult>, SigningError> {
        let mut results = Vec::new();
        for source in &inputs.apex_image.packages {
            let fail = |phase, message: String| {
                SigningError::Rejected(Error {
                    package: source.parsed.package_name.clone(),
                    path: source.info.module_path.clone(),
                    phase,
                    message,
                })
            };
            let mut parsed = source.parsed.clone();
            // addForInitLI refreshes the disabled raw identity before any
            // later selection or admission can reject this source.
            if let Some(disabled) = self
                .settings
                .disabled_system_packages
                .iter_mut()
                .find(|p| p.name == parsed.package_name)
            {
                disabled
                    .transient
                    .apex_module_name
                    .clone_from(&source.info.module_name);
            }
            let identity = Identity::select_for_apex(&parsed, &self.settings);
            let previous = self
                .settings
                .packages
                .iter()
                .find(|p| p.name == identity.internal_name)
                .cloned();
            let disabled = self
                .settings
                .disabled_system_packages
                .iter()
                .find(|p| p.name == identity.internal_name)
                .cloned();
            if parsed.shared_user_id.is_some()
                || identity.real_name.is_some()
                || Identity::original_setting(&parsed, &self.settings, &|name| {
                    self.has_scanned_package(name)
                })
                .is_some()
            {
                return Err(fail(
                    "apex-identity",
                    "APEX shared/original identity transition is not completed (#889/#890)".into(),
                ));
            }
            if previous
                .as_ref()
                .is_some_and(|p| p.app_id != -1 || p.shared_user)
            {
                return Err(fail(
                    "apex-identity",
                    "container replaces an application UID owner".into(),
                ));
            }
            let updated = !source.info.factory || disabled.is_some();
            let policy = container_policy(&source.info.module_path);
            policy
                .apply(
                    &mut parsed,
                    &source.signing,
                    None,
                    updated,
                    apks,
                    inputs.compatibility,
                    None,
                )
                .map_err(|e| fail("apex-policy", e))?;
            super::validate::static_library(&parsed, false)
                .map_err(|e| fail("apex-validation", e))?;
            parsed.signing_details = Some(
                source
                    .signing
                    .parcel_details()
                    .map_err(|e| fail("apex-signatures", e))?,
            );
            identity.apply(&mut parsed);
            let (flags, private_flags) = application_flags(&parsed, updated);
            let metadata = SettingMetadata {
                code_path: source.info.module_path.clone(),
                legacy_native_library_path: None,
                primary_cpu_abi: None,
                secondary_cpu_abi: None,
                version_code: (i64::from(parsed.version_code_major) << 32)
                    | i64::from(parsed.version_code as u32),
                flags,
                private_flags,
                last_modified_time: 0,
                uses_sdk_libraries: super::boot::sdk_libraries(&parsed)
                    .map_err(|e| fail("apex-metadata", e))?,
                uses_static_libraries: super::boot::static_libraries(&parsed)
                    .map_err(|e| fail("apex-metadata", e))?,
                mime_groups: parsed.mime_groups.clone(),
                domain_set_id: (inputs.new_domain_id)().map_err(|e| fail("apex-domain", e))?,
                target_sdk_version: parsed.target_sdk_version,
                restrict_update_hash: parsed.restrict_update_hash.clone(),
            };
            let setting = if let Some(previous) = &previous {
                let users = self.scanned_users.get(&previous.name).ok_or_else(|| {
                    fail(
                        "apex-users",
                        "retained container has no scan user owner".into(),
                    )
                })?;
                NewSetting::update(
                    previous,
                    users,
                    SettingUpdate {
                        code_path: metadata.code_path.clone(),
                        legacy_native_library_path: None,
                        primary_cpu_abi: None,
                        secondary_cpu_abi: None,
                        flags,
                        private_flags,
                        uses_sdk_libraries: metadata.uses_sdk_libraries,
                        uses_static_libraries: metadata.uses_static_libraries,
                        mime_groups: metadata.mime_groups,
                        domain_set_id: metadata.domain_set_id,
                        target_sdk_version: metadata.target_sdk_version,
                        restrict_update_hash: metadata.restrict_update_hash,
                    },
                    inputs.users.users,
                    disabled.is_some(),
                )
            } else {
                let mut users = inputs.users;
                users.install_user = None;
                users.stopped_system_app = super::boot::initial_stopped(
                    &parsed,
                    config,
                    apks.platform
                        .framework_boolean("config_stopSystemPackagesByDefault")
                        .map_err(|e| fail("apex-policy", e))?,
                );
                NewSetting::new(
                    &identity,
                    &Uid {
                        app_id: -1,
                        shared_user: None,
                    },
                    metadata,
                    users,
                )
            };
            let mut staged = self.clone();
            let mut package = setting.package;
            package.transient.updated_system_app |= updated;
            package
                .transient
                .apex_module_name
                .clone_from(&source.info.module_name);
            if let Some(at) = staged
                .settings
                .packages
                .iter()
                .position(|p| p.name == package.name)
            {
                staged.settings.packages[at] = package.clone();
            } else {
                staged.settings.packages.push(package.clone());
            }
            let mut record = Record {
                settings: package,
                parsed,
                signing: source.signing.clone(),
                identity,
                origin: if source.scan_parse_flags & parse::PARSE_IS_SYSTEM_DIR != 0 {
                    ScanOrigin::SystemDirectory
                } else {
                    ScanOrigin::Data
                },
            };
            let signing = staged.apply_apex_candidate(&record, source)?;
            record.settings = staged
                .settings
                .packages
                .iter()
                .find(|p| p.name == record.settings.name)
                .unwrap()
                .clone();
            let env = NativeLibraryEnvironment {
                preferred_abi: inputs.preferred_abi,
                app_lib32_install_dir: inputs.app_lib32_install_dir,
                code_is_directory: false,
                canonical_source: None,
            };
            let completed = staged.finish_scan_metadata(
                NewPackageOutcome {
                    record,
                    users: setting.users,
                    signing,
                },
                apks,
                ScanMetadataCompletion {
                    seinfo: inputs.seinfo,
                    abi_policy: inputs.abi_policy,
                    native_environment: &env,
                    context: AbiScanContext {
                        mode: AbiScanMode::Apex,
                        system: true,
                        updated,
                        override_abi: None,
                        platform_runtime_64bit: None,
                    },
                    install: inputs.install,
                    destination: None,
                    clock: inputs.clock,
                    factory_test: inputs.factory_test,
                },
            )?;
            let name = &completed.candidate.record.settings.name;
            if source.info.factory && !source.info.active {
                staged.disable_system_package(name)?;
            }
            results.push(ApexScanResult {
                info: source.info.clone(),
                package: completed.candidate.record.parsed,
            });
            *self = staged;
        }
        Ok(results)
    }
}

fn container_policy(path: &str) -> ScanPolicy {
    // getSystemPackageScanFlags always includes SYSTEM, even for /data APEX.
    // Partition masks use the actual code path, not the preinstalled origin.
    let partition = [
        Partition::SystemExt,
        Partition::Product,
        Partition::Oem,
        Partition::Odm,
        Partition::Vendor,
        Partition::System,
    ]
    .into_iter()
    .find(|part| {
        let root = format!("/{}", part.name());
        path == root || path.starts_with(&(root + "/"))
    });
    ScanPolicy {
        system: true,
        apex: true,
        privileged: partition
            .is_some_and(|part| path.starts_with(&format!("/{}/priv-app/", part.name()))),
        oem: partition == Some(Partition::Oem),
        vendor: partition == Some(Partition::Vendor),
        odm: partition == Some(Partition::Odm),
        product: partition == Some(Partition::Product),
        system_ext: partition == Some(Partition::SystemExt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn container_flags_follow_code_partition_and_always_include_system_and_apex() {
        for (path, vendor, product, system_ext, privileged) in [
            ("/vendor/apex/a.apex", true, false, false, false),
            ("/product/apex/a.apex", false, true, false, false),
            ("/system_ext/apex/a.apex", false, false, true, false),
            ("/system/priv-app/a.apex", false, false, false, true),
            ("/vendorish/a.apex", false, false, false, false),
            ("/data/apex/active/a.apex", false, false, false, false),
        ] {
            let policy = container_policy(path);
            assert!(policy.system && policy.apex);
            assert_eq!(
                (
                    policy.vendor,
                    policy.product,
                    policy.system_ext,
                    policy.privileged
                ),
                (vendor, product, system_ext, privileged)
            );
        }
    }
}
