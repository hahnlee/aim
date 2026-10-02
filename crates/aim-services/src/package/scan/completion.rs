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
}

#[derive(Debug)]
pub struct CompletedScanMetadata {
    pub candidate: NewPackageOutcome,
    pub copies: Vec<NativeLibraryAbiCopy>,
    pub multi_arch_mismatch: bool,
    pub alignment_diagnostic: Option<String>,
}

impl SigningScan {
    /// Complete ABI/copy, page-size and code metadata in original stage order.
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
        *self = staged;
        Ok(CompletedScanMetadata {
            candidate,
            copies,
            multi_arch_mismatch,
            alignment_diagnostic,
        })
    }
}
