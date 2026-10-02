//! Ordered signing reconciliation for already-saved APK identities.
//! Candidate state is separate from the published snapshot and disk.
//! New/removed package reconciliation and side effects remain under #702.
use super::{Error, Record, authorize, physical_parse_flags};
use crate::package::{
    owner::shared_users::{Bootstrap, RestoreError, ScanOrigin, SignatureError, saved_signatures},
    parse,
    pkg::booleans,
    settings::Settings,
    sign::SigningDetails,
    system_config::SystemConfig,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SigningScan {
    pub settings: Settings,
    pub identities: Bootstrap,
    first_api_level: i32,
    parsed: Vec<(String, i32, SigningDetails)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SigningError {
    Rejected(Error),
    Fatal(Error),
}

#[derive(Debug, PartialEq, Eq)]
pub struct SigningOutcome {
    /// The caller must report the original OTA signature mismatch.
    pub system_signature_mismatch: Option<String>,
}

impl SigningScan {
    pub fn new(
        config: &SystemConfig,
        settings: &Settings,
        first_api_level: i32,
    ) -> Result<Self, RestoreError> {
        Ok(Self {
            identities: Bootstrap::restore(config, settings)?,
            settings: settings.clone(),
            first_api_level,
            parsed: Vec::new(),
        })
    }

    /// Apply one verified saved record in the caller's actual scan order.
    /// Reconciliation and signer commit are atomic for this record. A
    /// later failure preserves earlier committed candidate records; the
    /// caller may discard the whole candidate after a fatal system error.
    pub fn apply(&mut self, record: &Record) -> Result<SigningOutcome, SigningError> {
        let fail = |phase, message| Error {
            package: record.settings.name.clone(),
            path: record.settings.code_path.clone(),
            phase,
            message,
        };
        let reject = |phase, message| SigningError::Rejected(fail(phase, message));
        let at = self
            .settings
            .packages
            .iter()
            .position(|p| p.name == record.settings.name)
            .ok_or_else(|| reject("identity", "package has no saved identity (#804)".into()))?;
        let previous = &self.settings.packages[at];
        if previous.app_id != record.settings.app_id
            || previous.shared_user != record.settings.shared_user
            || previous.code_path != record.settings.code_path
            || record.identity.internal_name != previous.name
            || record.parsed.package_name != previous.name
        {
            return Err(reject(
                "identity",
                "saved code or UID ownership changed (#804)".into(),
            ));
        }
        let flags =
            physical_parse_flags(&record.settings.code_path).map_err(|e| reject("location", e))?;
        let origin = if flags & parse::PARSE_IS_SYSTEM_DIR != 0 {
            ScanOrigin::SystemDirectory
        } else {
            ScanOrigin::Data
        };
        if record.origin != origin {
            return Err(reject(
                "location",
                "record origin disagrees with physical code path".into(),
            ));
        }
        let group_name = if previous.shared_user {
            Some(
                self.settings
                    .shared_users
                    .iter()
                    .find(|g| g.app_id == previous.app_id)
                    .ok_or_else(|| {
                        reject("identity", "shared UID group disappeared (#803)".into())
                    })?
                    .name
                    .clone(),
            )
        } else {
            None
        };
        // InstallPackageHelper.scanPackageNewLI ignores a leaving declaration
        // only once the saved package no longer owns a shared UID. A changed
        // group makes ScanPackageUtils replace the PackageSetting, rather than
        // reuse its UID. This saved-identity phase cannot allocate that replacement.
        let declared_group = selected_shared_user(
            previous.shared_user,
            record.parsed.shared_user_id.as_deref(),
            record.parsed.is(booleans::LEAVING_SHARED_UID),
        );
        if group_name.as_deref() != declared_group {
            return Err(reject(
                "identity",
                "manifest shared UID requires replacing saved UID ownership (#804)".into(),
            ));
        }
        let mut group = match &group_name {
            Some(name) => Some(
                self.identities
                    .shared_users
                    .get(name)
                    .ok_or_else(|| {
                        reject("identity", "shared UID ownership disappeared (#803)".into())
                    })?
                    .clone(),
            ),
            None => None,
        };
        let normal = authorize::saved(previous, &record.signing, &self.settings);
        let mut mismatch = None;
        match normal {
            Ok(()) => {
                if let Some(group) = &mut group {
                    let others: Vec<_> = self
                        .parsed
                        .iter()
                        .filter(|(name, id, _)| name != &previous.name && *id == previous.app_id)
                        .map(|(_, _, details)| details.clone())
                        .collect();
                    group
                        .merge_authorized_lineage(&record.signing, &others)
                        .map_err(|e| reject("signatures", e))?;
                }
            }
            Err(message) => {
                if origin != ScanOrigin::SystemDirectory {
                    return Err(reject("authorization", message));
                }
                if let Some(group) = &mut group {
                    group
                        .replace_after_signature_failure(
                            &record.signing,
                            origin,
                            self.first_api_level,
                        )
                        .map_err(|e| match e {
                            SignatureError::FatalSystemMismatch => SigningError::Fatal(fail(
                                "authorization",
                                "inconsistent system shared UID signatures".into(),
                            )),
                            SignatureError::Rejected { code } => reject(
                                "authorization",
                                format!("inconsistent system shared UID signatures ({code})"),
                            ),
                            SignatureError::Certificates(why) => reject("signatures", why),
                            SignatureError::NonSystemMismatch => {
                                reject("authorization", message.clone())
                            }
                        })?;
                }
                mismatch = Some(message);
            }
        }
        let signatures = saved_signatures(&record.signing).map_err(|e| reject("signatures", e))?;
        if let Some(group) = &mut group {
            group
                .commit_initial_signatures(&record.signing)
                .map_err(|e| reject("signatures", e))?;
        }
        // All fallible work finishes before changing this candidate.
        self.settings.packages[at].signatures = Some(signatures);
        if let (Some(name), Some(group)) = (group_name, group) {
            let saved = self
                .settings
                .shared_users
                .iter_mut()
                .find(|g| g.name == name)
                .unwrap();
            saved.signatures = group.signatures.clone();
            self.identities.shared_users.insert(name, group);
        }
        self.parsed
            .retain(|(name, _, _)| name != &record.settings.name);
        self.parsed.push((
            record.settings.name.clone(),
            record.settings.app_id,
            record.signing.clone(),
        ));
        Ok(SigningOutcome {
            system_signature_mismatch: mismatch,
        })
    }
}

fn selected_shared_user(saved_shared: bool, declared: Option<&str>, leaving: bool) -> Option<&str> {
    if !saved_shared && leaving {
        None
    } else {
        declared
    }
}

#[cfg(test)]
mod tests {
    use super::selected_shared_user;

    #[test]
    fn shared_user_selection_retains_existing_members_until_migration() {
        for (saved_shared, declared, leaving, selected) in [
            (false, None, false, None),
            (false, None, true, None),
            (true, None, false, None),
            (true, None, true, None),
            (false, Some("uid"), false, Some("uid")),
            (false, Some("uid"), true, None),
            (true, Some("uid"), false, Some("uid")),
            (true, Some("uid"), true, Some("uid")),
        ] {
            assert_eq!(
                selected_shared_user(saved_shared, declared, leaving),
                selected
            );
        }
    }
}
