//! Initial certificate collection, android-16.0.0_r1 InstallPackageHelper
//! and PackageManagerService. Copyright (C) The Android Open Source Project,
//! Apache License 2.0.
use super::{Code, Error, Identity, SigningError, SigningScan};
use crate::package::{
    parse,
    pkg::booleans,
    write::{ApkSigningError, Apks, CertificateCollection},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct CertificateScanPolicy {
    pub upgrade: bool,
    pub pre_n_mr1_upgrade: bool,
}

impl SigningScan {
    /// Resolve the current setting and volume owner immediately before the
    /// next initial-scan collection. Earlier package/version effects remain.
    pub fn collect_initial_code<S>(
        &mut self,
        code: &Code<S>,
        apks: &Apks,
        policy: CertificateScanPolicy,
    ) -> Result<Code, SigningError> {
        let system = code.location.parse_flags() & parse::PARSE_IS_SYSTEM_DIR != 0;
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, system, &code.location);
        let original = Identity::original_setting(&code.parsed, &self.settings, &|name| {
            self.has_scanned_package(name)
        });
        let saved = original
            .or_else(|| {
                self.settings
                    .packages
                    .iter()
                    .find(|p| p.name == identity.internal_name)
            })
            .cloned();
        self.collect_selected_initial_code(code, apks, policy, saved.as_ref())
    }

    /// Initial scan retains its selected setting across enableSystemPackage;
    /// certificate cache decisions still use that pre-enable owner.
    pub(super) fn collect_selected_initial_code<S>(
        &mut self,
        code: &Code<S>,
        apks: &Apks,
        policy: CertificateScanPolicy,
        saved: Option<&crate::package::settings::Package>,
    ) -> Result<Code, SigningError> {
        let system = code.location.parse_flags() & parse::PARSE_IS_SYSTEM_DIR != 0;
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, system, &code.location);
        let force_collect = if system {
            policy.upgrade
        } else {
            saved
                .as_ref()
                .is_some_and(|p| self.strict_signature_packages.contains(&p.name))
        };
        let volume = if code.parsed.is(booleans::EXTERNAL_STORAGE) {
            Some(
                code.parsed
                    .volume_uuid
                    .clone()
                    .filter(|uuid| !uuid.is_empty())
                    .unwrap_or_else(|| "primary_physical".into()),
            )
        } else {
            None
        };
        let database_version = self
            .settings
            .find_or_create_version(volume)
            .database_version;
        let error = |message| Error {
            package: identity.internal_name.clone(),
            path: code.location.path.clone(),
            phase: "certificates",
            message,
        };
        let signing = apks
            .checked_collect_signing_details(
                &code.parsed,
                CertificateCollection {
                    saved,
                    database_version,
                    force_collect,
                    skip_verify: system,
                    pre_n_mr1_upgrade: policy.pre_n_mr1_upgrade,
                },
            )
            .map_err(|reason| match reason {
                ApkSigningError::Invalid(reason) => {
                    SigningError::Rejected(error(reason.to_string()))
                }
                ApkSigningError::Input(message) => SigningError::Fatal(error(message)),
            })?;
        let mut parsed = code.parsed.clone();
        parsed.signing_details = Some(
            signing
                .parcel_details()
                .map_err(|message| SigningError::Fatal(error(message)))?,
        );
        Ok(Code {
            location: code.location.clone(),
            parsed,
            signing,
        })
    }
}
