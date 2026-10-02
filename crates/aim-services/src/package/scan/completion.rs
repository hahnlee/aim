//! Accepted scan completion after identity/signing reconciliation (#702/#810).
use super::{
    AbiPolicy, AbiScanContext, NativeLibraryAbiCopy, NativeLibraryDestination,
    NativeLibraryEnvironment, NativeLibraryError, NativeLibraryInstallPolicy, NewPackageOutcome,
    PageSizeCompatPolicy, ScanClock, SigningError, SigningScan,
};
use crate::package::write::Apks;

/// Image, filesystem, VM and clock inputs from the corresponding owners.
pub struct ScanMetadataCompletion<'a> {
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
    /// Complete ABI/copy, page-size, code and final application metadata.
    /// Only the finished candidate becomes accepted state. Files copied before
    /// a later error require cleanup by the install owner; this does not persist
    /// settings, publish a query replica or make filesystem rollback implicit.
    pub fn finish_scan_metadata(
        &mut self,
        candidate: NewPackageOutcome,
        apks: &Apks,
        inputs: ScanMetadataCompletion<'_>,
    ) -> Result<CompletedScanMetadata, SigningError> {
        self.accepted_slot(&candidate.record, "scan-completion")?;
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
        *self = staged;
        Ok(CompletedScanMetadata {
            candidate,
            copies,
            multi_arch_mismatch,
            alignment_diagnostic,
        })
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
            // A retained member's mutable setting changes in place in AOSP.
            // Refresh its removal inputs without re-ORing cached group flags.
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
