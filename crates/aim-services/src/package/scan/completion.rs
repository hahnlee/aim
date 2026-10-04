//! Accepted scan completion after identity/signing reconciliation (#702/#810).
use super::{
    AbiPolicy, AbiScanContext, NativeLibraryAbiCopy, NativeLibraryDestination,
    NativeLibraryEnvironment, NativeLibraryError, NativeLibraryInstallPolicy, NewPackageOutcome,
    PageSizeCompatPolicy, ScanClock, SigningError, SigningScan,
};
use crate::package::{restrictions::UserState, write::Apks};
use std::collections::BTreeMap;

/// Image, filesystem, VM and clock inputs from the corresponding owners.
pub struct ScanMetadataCompletion<'a> {
    pub seinfo: super::SeInfoScan<'a>,
    pub abi_policy: &'a AbiPolicy,
    pub native_environment: &'a NativeLibraryEnvironment<'a>,
    pub context: AbiScanContext<'a>,
    pub install: NativeLibraryInstallPolicy,
    pub destination: Option<&'a NativeLibraryDestination<'a>>,
    pub clock: ScanClock,
    /// Original SystemServer factory-test mode, supplied by its boot owner.
    pub factory_test: bool,
}

#[derive(Debug)]
pub struct CompletedScanMetadata {
    pub candidate: NewPackageOutcome,
    pub copies: Vec<NativeLibraryAbiCopy>,
    pub multi_arch_mismatch: bool,
    pub alignment_diagnostic: Option<String>,
}

impl SigningScan {
    /// Retained UID scans publish signer, library and setting changes only
    /// after every metadata gate succeeds. Copied files still belong to the
    /// install owner's cleanup transaction.
    pub fn scan_existing(
        &mut self,
        code: &super::Code,
        update: super::SettingUpdate,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[super::User]>,
        disabled: Option<&super::Record>,
        apks: &Apks,
        inputs: ScanMetadataCompletion<'_>,
    ) -> Result<CompletedScanMetadata, SigningError> {
        let mut staged = self.clone();
        let candidate = staged.apply_existing(code, update, saved_users, all_users, disabled)?;
        let completed = staged.finish_scan_metadata(candidate, apks, inputs)?;
        *self = staged;
        Ok(completed)
    }

    /// Stage a new system package through all metadata gates before admitting
    /// its final shared-UID member. A rejected independent allocation advances
    /// the original cleanup cursor; a new shared group survives until pruning.
    pub fn scan_new_system(
        &mut self,
        code: &super::Code,
        metadata: super::SettingMetadata,
        users: super::UserPolicy<'_>,
        apks: &Apks,
        inputs: ScanMetadataCompletion<'_>,
    ) -> Result<CompletedScanMetadata, SigningError> {
        self.remove_stale_disabled_system(code)?;
        let mut staged = self.clone();
        let (candidate, mut preparation) = match staged.prepare_new_system(code, metadata, users) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.identities = staged.identities;
                return Err(error);
            }
        };
        let name = candidate.record.settings.name.clone();
        let completed = match staged.finish_scan_metadata(candidate, apks, inputs) {
            Ok(completed) => completed,
            Err(error) => {
                preparation
                    .reject_pending(&name)
                    .map_err(SigningError::Fatal)?;
                self.identities = preparation.identities;
                return Err(error);
            }
        };
        preparation.accept_uid(&name).map_err(SigningError::Fatal)?;
        *self = staged;
        Ok(completed)
    }

    /// Complete ABI/copy, page-size, code, application and seInfo metadata.
    /// Only the finished candidate becomes accepted state. Files copied before
    /// a later error require cleanup by the install owner; this does not persist
    /// settings, publish a query replica or make filesystem rollback implicit.
    pub fn finish_scan_metadata(
        &mut self,
        candidate: NewPackageOutcome,
        apks: &Apks,
        mut inputs: ScanMetadataCompletion<'_>,
    ) -> Result<CompletedScanMetadata, SigningError> {
        self.accepted_slot(&candidate.record, "scan-completion")?;
        inputs.context.updated |= candidate.record.settings.transient.updated_system_app;
        let page_policy =
            PageSizeCompatPolicy::from_platform(&apks.platform).map_err(|message| {
                SigningError::NativeLibrary {
                    package: candidate.record.settings.name.clone(),
                    path: candidate.record.settings.code_path.clone(),
                    error: NativeLibraryError::Input(message),
                }
            })?;
        let mut staged = self.clone();
        let (candidate, multi_arch_mismatch, copies) = match inputs.destination {
            Some(destination) => staged.finish_native_library_install(
                candidate,
                apks,
                inputs.abi_policy,
                inputs.native_environment,
                inputs.context,
                inputs.install,
                destination,
            )?,
            None => {
                let (candidate, mismatch) = staged.finish_native_library_metadata(
                    candidate,
                    apks,
                    inputs.abi_policy,
                    inputs.native_environment,
                    inputs.context,
                )?;
                (candidate, mismatch, Vec::new())
            }
        };
        let (candidate, alignment_diagnostic) = staged.finish_page_size_metadata(
            candidate,
            apks,
            &page_policy,
            &inputs.abi_policy.bit64,
            inputs.install,
            inputs.context,
        )?;
        let candidate = staged.finish_code_metadata(candidate, apks, inputs.clock)?;
        let candidate = staged.finish_application_metadata(
            candidate,
            inputs.factory_test,
            inputs.context.updated,
        )?;
        let mut candidate = staged.finish_key_set_metadata(candidate)?;
        staged.accepted_slot(&candidate.record, "package-finalization")?;
        // commitReconciledScanResultLocked sets the assigned appId only after
        // setting/UID creation. The object is retained before query publication.
        candidate.record.parsed.uid = candidate.record.settings.app_id;
        let loaded = super::LoadedPackage::new(
            candidate.record.parsed.clone(),
            candidate.record.signing.clone(),
        )
        .map_err(|message| {
            SigningError::Rejected(super::Error {
                package: candidate.record.settings.name.clone(),
                path: candidate.record.settings.code_path.clone(),
                phase: "package-finalization",
                message,
            })
        })?;
        staged
            .update_ownership
            .queue(&candidate.record.settings, &candidate.record.parsed);
        let name = &candidate.record.settings.name;
        if self.loaded.contains_key(name) && !self.has_seinfo_assignment(name) {
            return Err(SigningError::Rejected(super::Error {
                package: name.clone(),
                path: candidate.record.settings.code_path.clone(),
                phase: "seinfo",
                message: "retained loaded package has no seInfo assignment".into(),
            }));
        }
        staged.loaded.insert(
            candidate.record.settings.name.clone(),
            std::sync::Arc::new(loaded),
        );
        staged.pending_metadata.remove(name);
        let setting = staged.seinfo_setting_for_scan(name).map_err(|message| {
            SigningError::Rejected(super::Error {
                package: name.clone(),
                path: candidate.record.settings.code_path.clone(),
                phase: "seinfo",
                message,
            })
        })?;
        staged
            .assign_seinfo_for_scan(name, setting, inputs.seinfo.policy, &mut |package| {
                inputs.seinfo.compatibility.target_sdk(package)
            })
            .map_err(|message| {
                SigningError::Rejected(super::Error {
                    package: name.clone(),
                    path: candidate.record.settings.code_path.clone(),
                    phase: "seinfo",
                    message,
                })
            })?;
        *self = staged;
        Ok(CompletedScanMetadata {
            candidate,
            copies,
            multi_arch_mismatch,
            alignment_diagnostic,
        })
    }

    /// Register the verified package's signing public keys only after complete
    /// metadata succeeds. Manifest keyset parsing remains explicit (#824).
    fn finish_key_set_metadata(
        &mut self,
        mut candidate: NewPackageOutcome,
    ) -> Result<NewPackageOutcome, SigningError> {
        let record = &mut candidate.record;
        let at = self.accepted_slot(record, "keysets")?;
        let fail = |message| {
            SigningError::Fatal(super::Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "keysets",
                message,
            })
        };
        let defined = record
            .parsed
            .key_set_mapping
            .as_ref()
            .map(|mapping| {
                mapping
                    .iter()
                    .map(|(alias, keys)| {
                        let alias = alias.clone().ok_or("null defined keyset alias")?;
                        let keys = keys.as_ref().ok_or("null defined public-key set")?;
                        let keys = keys
                            .iter()
                            .map(|key| {
                                crate::package::sign::deserialize_public_key(
                                    key.as_ref().ok_or("null defined public key")?,
                                )
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        Ok((alias, keys))
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()
            .map_err(fail)?;
        crate::package::owner::key_sets::register(
            &mut self.settings,
            &record.settings.name,
            &record.signing.public_keys,
            defined.as_deref(),
            &record.parsed.upgrade_key_sets,
        )
        .map_err(fail)?;
        record.settings.key_set_data = self.settings.packages[at].key_set_data.clone();
        Ok(candidate)
    }

    /// Final ScanPackageUtils factory-test and ApplicationInfo flag enrichment.
    /// Saved bitfields are replaced by the adjusted parsed package and the
    /// setting owner's updated-system state. Publication remains a later phase.
    pub fn finish_application_metadata(
        &mut self,
        mut candidate: NewPackageOutcome,
        factory_test: bool,
        updated_system_app: bool,
    ) -> Result<NewPackageOutcome, SigningError> {
        let record = &mut candidate.record;
        let at = self.accepted_slot(record, "application-flags")?;
        let reject = || {
            SigningError::Rejected(super::Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "application-flags",
                message: "accepted shared UID owner is missing".into(),
            })
        };
        let mut shared = if record.settings.shared_user {
            let name = &self
                .settings
                .shared_users
                .iter()
                .find(|g| g.app_id == record.settings.app_id)
                .ok_or_else(reject)?
                .name;
            Some((
                name.clone(),
                self.identities
                    .shared_users
                    .get(name)
                    .ok_or_else(reject)?
                    .clone(),
            ))
        } else {
            None
        };
        super::enrich::application(
            &mut record.settings,
            &mut record.parsed,
            factory_test,
            updated_system_app,
        );
        if let Some((name, mut group)) = shared.take() {
            // First admission uses final flags. A retained member's mutable
            // setting changes in place without re-ORing cached group flags.
            group.add_package(
                &record.settings.name,
                record.settings.flags,
                record.settings.private_flags,
            );
            self.identities.shared_users.insert(name, group);
        }
        self.settings.packages[at] = record.settings.clone();
        Ok(candidate)
    }
}
