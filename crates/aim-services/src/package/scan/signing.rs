//! Ordered signing reconciliation for saved and new system APK identities.
//! Candidate state is separate from the published snapshot and disk.
//! New/removed package reconciliation and side effects remain under #702.
//! Shared UID migration is ported from android-16.0.0_r1 Settings,
//! SharedUserSetting and SharedUidMigration.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    Code, Error, Identity, Record, SettingMetadata, UidScan, UserPolicy, authorize,
    physical_parse_flags,
};
use crate::package::{
    libraries::Registry,
    owner::app_ids::Owner,
    owner::shared_users::{Bootstrap, RestoreError, ScanOrigin, SignatureError, saved_signatures},
    parse,
    pkg::booleans,
    restrictions::UserState,
    settings::{Settings, SharedUser},
    sign::SigningDetails,
    system_config::SystemConfig,
};
use std::collections::BTreeMap;

/// SharedUidMigration permits only these two strategies in the pinned image.
/// The owner supplies image policy; this does not read a host environment flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SharedUidMigration {
    #[default]
    NewInstallOnly,
    BestEffort,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SigningScan {
    pub settings: Settings,
    pub identities: Bootstrap,
    pub libraries: Registry,
    first_api_level: i32,
    parsed: Vec<(String, i32, SigningDetails, bool)>,
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

/// Accepted setting candidate; scan enrichment and publication follow.
#[derive(Debug)]
pub struct NewPackageOutcome {
    pub record: Record,
    pub users: BTreeMap<i32, UserState>,
    pub signing: SigningOutcome,
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
            libraries: Registry::new(config),
            first_api_level,
            parsed: Vec::new(),
        })
    }

    /// Reconcile initial-scan setting updates from verified code. This keeps
    /// the saved UID and signatures through Settings.updatePackageSetting,
    /// then commits metadata and library declarations with signer state.
    /// Remaining ScanPackageUtils enrichment and publication follow this phase.
    pub fn apply_existing(
        &mut self,
        code: &Code,
        update: super::SettingUpdate,
        saved_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
        all_users: Option<&[super::User]>,
        disabled: Option<&Record>,
    ) -> Result<NewPackageOutcome, SigningError> {
        let flags = physical_parse_flags(&code.location.path).map_err(|message| {
            SigningError::Rejected(Error {
                package: code.parsed.package_name.clone(),
                path: code.location.path.clone(),
                phase: "location",
                message,
            })
        })?;
        let identity = Identity::select(
            &code.parsed,
            &self.settings,
            update.flags & crate::package::settings::FLAG_SYSTEM != 0,
        );
        let reject = |phase, message: &str| {
            SigningError::Rejected(Error {
                package: identity.internal_name.clone(),
                path: code.location.path.clone(),
                phase,
                message: message.into(),
            })
        };
        if update.code_path != code.location.path {
            return Err(reject(
                "location",
                "setting update disagrees with physical code path",
            ));
        }
        super::validate::static_library(&code.parsed, false)
            .map_err(|e| reject("validation", &e))?;
        let at = self
            .settings
            .packages
            .iter()
            .position(|p| p.name == identity.internal_name)
            .ok_or_else(|| reject("identity", "setting update has no saved package"))?;
        let original = &self.settings.packages[at];
        let group = if original.shared_user {
            self.settings
                .shared_users
                .iter()
                .find(|g| g.app_id == original.app_id)
                .map(|g| g.name.as_str())
        } else {
            None
        };
        if group
            != selected_shared_user(
                original.shared_user,
                code.parsed.shared_user_id.as_deref(),
                code.parsed.is(booleans::LEAVING_SHARED_UID),
            )
        {
            return Err(reject(
                "identity",
                "setting update requires replacing UID ownership (#804)",
            ));
        }
        let owner = match group {
            Some(name) => Owner::SharedUser(name.into()),
            None => Owner::Package(original.name.clone()),
        };
        if self.identities.ids.get(original.app_id) != Some(&owner) {
            return Err(reject("identity", "saved package no longer owns its UID"));
        }
        let users = saved_users
            .get(&original.name)
            .ok_or_else(|| reject("setting", "saved package user states were not supplied"))?;
        let setting = super::NewSetting::update(
            original,
            users,
            update,
            all_users,
            self.settings
                .disabled_system_packages
                .iter()
                .any(|p| p.name == original.name),
        );
        let mut parsed = code.parsed.clone();
        identity.apply(&mut parsed);
        let mut record = Record {
            settings: setting.package,
            parsed,
            signing: code.signing.clone(),
            identity,
            origin: if flags & parse::PARSE_IS_SYSTEM_DIR != 0 {
                ScanOrigin::SystemDirectory
            } else {
                ScanOrigin::Data
            },
        };
        let mut next = self.clone();
        next.settings.packages[at] = record.settings.clone();
        let signing = next.apply_with_disabled(&record, disabled)?;
        record.settings = next.settings.packages[at].clone();
        *self = next;
        Ok(NewPackageOutcome {
            record,
            users: setting.users,
            signing,
        })
    }

    /// Adopt an unscanned original system package's setting and user states.
    /// The caller supplies the complete saved user-state map by package name.
    /// Code verification precedes this phase; publication and transfer side
    /// effects follow only after the complete scan candidate succeeds.
    pub fn apply_original_system(
        &mut self,
        code: &Code,
        metadata: SettingMetadata,
        original_users: &BTreeMap<String, BTreeMap<i32, UserState>>,
    ) -> Result<NewPackageOutcome, SigningError> {
        let reject = |phase, message: &str| {
            SigningError::Rejected(Error {
                package: code.parsed.package_name.clone(),
                path: code.location.path.clone(),
                phase,
                message: message.into(),
            })
        };
        let flags =
            physical_parse_flags(&code.location.path).map_err(|e| reject("location", &e))?;
        if flags & parse::PARSE_IS_SYSTEM_DIR == 0
            || metadata.flags & crate::package::settings::FLAG_SYSTEM == 0
            || metadata.code_path != code.location.path
        {
            return Err(reject(
                "location",
                "original adoption requires matching system code",
            ));
        }
        let selected = Identity::select(&code.parsed, &self.settings, true);
        if self
            .settings
            .packages
            .iter()
            .any(|p| p.name == selected.internal_name)
        {
            return Err(reject(
                "identity",
                "incoming package already has a saved setting",
            ));
        }
        super::validate::static_library(&code.parsed, false)
            .map_err(|e| reject("validation", &e))?;
        let original = Identity::original_setting(&code.parsed, &self.settings, &|name| {
            self.parsed.iter().any(|(n, _, _, _)| n == name)
        })
        .ok_or_else(|| reject("identity", "no eligible original system package"))?;
        let group = if original.shared_user {
            self.settings
                .shared_users
                .iter()
                .find(|g| g.app_id == original.app_id)
                .map(|g| g.name.as_str())
        } else {
            None
        };
        if group
            != selected_shared_user(
                original.shared_user,
                code.parsed.shared_user_id.as_deref(),
                code.parsed.is(booleans::LEAVING_SHARED_UID),
            )
        {
            return Err(reject(
                "identity",
                "original adoption requires replacing UID ownership (#804)",
            ));
        }
        let expected_owner = match group {
            Some(name) => Owner::SharedUser(name.into()),
            None => Owner::Package(original.name.clone()),
        };
        if self.identities.ids.get(original.app_id) != Some(&expected_owner) {
            return Err(reject(
                "identity",
                "original setting no longer owns its UID",
            ));
        }
        let users = original_users
            .get(&original.name)
            .ok_or_else(|| reject("setting", "original package user states were not supplied"))?;
        let setting = super::NewSetting::adopt(original, users, &selected.manifest_name, metadata);
        let identity = Identity {
            manifest_name: selected.manifest_name,
            internal_name: original.name.clone(),
            real_name: setting.package.real_name.clone(),
        };
        let mut parsed = code.parsed.clone();
        identity.apply(&mut parsed);
        let mut record = Record {
            settings: setting.package,
            parsed,
            signing: code.signing.clone(),
            identity,
            origin: ScanOrigin::SystemDirectory,
        };
        let mut next = self.clone();
        let at = next
            .settings
            .packages
            .iter()
            .position(|p| p.name == record.settings.name)
            .unwrap();
        next.settings.packages[at] = record.settings.clone();
        let signing = next.apply(&record)?;
        record.settings = next.settings.packages[at].clone();
        // addRenamedPackageLPw replaces the old ArrayMap value on acceptance.
        next.settings
            .renamed_packages
            .retain(|(new, _)| new != &record.identity.manifest_name);
        next.settings.renamed_packages.push((
            record.identity.manifest_name.clone(),
            record.settings.name.clone(),
        ));
        *self = next;
        Ok(NewPackageOutcome {
            record,
            users: setting.users,
            signing,
        })
    }

    /// Reconcile verified new system-directory code through UID preparation,
    /// setting construction, INSTALL shared-UID gates, merge and signer commit.
    /// Rejection publishes no package/user state, but retains the original UID
    /// cleanup cursor and an allocated group until final pruning.
    pub fn apply_new_system(
        &mut self,
        code: &Code,
        metadata: SettingMetadata,
        users: UserPolicy<'_>,
    ) -> Result<NewPackageOutcome, SigningError> {
        let identity = Identity::select(&code.parsed, &self.settings, true);
        let reject = |phase, message: &str| {
            SigningError::Rejected(Error {
                package: identity.internal_name.clone(),
                path: code.location.path.clone(),
                phase,
                message: message.into(),
            })
        };
        let flags =
            physical_parse_flags(&code.location.path).map_err(|e| reject("location", &e))?;
        if flags & parse::PARSE_IS_SYSTEM_DIR == 0 || metadata.code_path != code.location.path {
            return Err(reject(
                "location",
                "new system setting disagrees with physical code path",
            ));
        }
        if self
            .settings
            .packages
            .iter()
            .any(|p| p.name == identity.internal_name)
        {
            return Err(reject(
                "identity",
                "new system package already has a saved setting",
            ));
        }
        super::validate::static_library(&code.parsed, users.instant_app)
            .map_err(|e| reject("validation", &e))?;
        let mut preparation = UidScan::with_identities(&self.settings, self.identities.clone());
        let (identity, _) = preparation.apply(code).map_err(SigningError::Rejected)?;
        let setting = preparation
            .new_setting(&identity, metadata, users)
            .map_err(SigningError::Rejected)?;
        let mut next = self.clone();
        next.identities = preparation.identities.clone();
        if let Some(name) = &preparation.packages[&identity.internal_name].shared_user {
            let group = &next.identities.shared_users[name];
            if !next.settings.shared_users.iter().any(|g| g.name == *name) {
                next.settings.shared_users.push(SharedUser {
                    name: name.clone(),
                    app_id: group.app_id,
                    flags: group.flags,
                    signatures: group.signatures.clone(),
                });
            }
        }
        let mut parsed = code.parsed.clone();
        identity.apply(&mut parsed);
        let mut record = Record {
            settings: setting.package,
            parsed,
            signing: code.signing.clone(),
            identity,
            origin: ScanOrigin::SystemDirectory,
        };
        next.settings.packages.push(record.settings.clone());
        let signing = match next.apply(&record) {
            Ok(outcome) => outcome,
            Err(error) => {
                preparation
                    .reject_pending(&record.settings.name)
                    .map_err(SigningError::Fatal)?;
                self.identities = preparation.identities;
                return Err(error);
            }
        };
        // No fallible work follows publication into this scan candidate.
        preparation
            .accept_uid(&record.settings.name)
            .map_err(SigningError::Fatal)?;
        record.settings = next.settings.packages.last().unwrap().clone();
        *self = next;
        Ok(NewPackageOutcome {
            record,
            users: setting.users,
            signing,
        })
    }

    /// Apply one verified saved record in the caller's actual scan order.
    /// Reconciliation and signer commit are atomic for this record. A
    /// later failure preserves earlier committed candidate records; the
    /// caller may discard the whole candidate after a fatal system error.
    pub fn apply(&mut self, record: &Record) -> Result<SigningOutcome, SigningError> {
        self.apply_with_disabled(record, None)
    }

    /// Initial boot scan uses declarations from prior accepted records. A
    /// dynamic updated-system provider needs its verified disabled original.
    /// SCAN_INITIAL skips upgrade-keyset handling as in the original.
    pub fn apply_with_disabled(
        &mut self,
        record: &Record,
        disabled: Option<&Record>,
    ) -> Result<SigningOutcome, SigningError> {
        let check = self
            .libraries
            .latest_static_setting(&record.parsed, &self.settings)
            .cloned();
        if record.parsed.static_shared_library_name.is_some()
            && (record.parsed.shared_user_id.is_some()
                || check.as_ref().is_some_and(|p| p.shared_user))
        {
            return Err(SigningError::Rejected(Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "identity",
                message: "shared UID is not allowed in a static shared library".into(),
            }));
        }
        if let Some(old) = disabled {
            if old.settings.name != record.settings.name
                || old.identity.internal_name != record.settings.name
                || old.parsed.package_name != record.settings.name
                || !self
                    .settings
                    .disabled_system_packages
                    .iter()
                    .any(|p| *p == old.settings)
            {
                return Err(SigningError::Rejected(Error {
                    package: record.settings.name.clone(),
                    path: record.settings.code_path.clone(),
                    phase: "identity",
                    message: "disabled library original disagrees with saved identity".into(),
                }));
            }
        }
        self.reconcile(record, check.as_ref(), disabled)
    }

    fn reconcile(
        &mut self,
        record: &Record,
        signature_check: Option<&crate::package::settings::Package>,
        disabled: Option<&Record>,
    ) -> Result<SigningOutcome, SigningError> {
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
        let normal = authorize::with_disabled(
            signature_check.unwrap_or(previous),
            &record.signing,
            &self.settings,
            self.settings
                .disabled_system_packages
                .iter()
                .find(|p| p.name == previous.name),
        );
        let mut mismatch = None;
        match normal {
            Ok(()) => {
                if let Some(group) = &mut group {
                    let others: Vec<_> = self
                        .parsed
                        .iter()
                        .filter(|(name, id, _, _)| name != &previous.name && *id == previous.app_id)
                        .map(|(_, _, details, _)| details.clone())
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
        let mut libraries = self.libraries.clone();
        libraries
            .add_scan_record(
                record,
                disabled,
                self.settings
                    .disabled_system_packages
                    .iter()
                    .any(|p| p.name == previous.name),
            )
            .map_err(|e| reject("libraries", e.0.into()))?;
        // All fallible work finishes before changing this candidate.
        self.settings.packages[at].signatures = Some(signatures);
        self.libraries = libraries;
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
            .retain(|(name, _, _, _)| name != &record.settings.name);
        self.parsed.push((
            record.settings.name.clone(),
            record.settings.app_id,
            record.signing.clone(),
            record.parsed.is(booleans::LEAVING_SHARED_UID),
        ));
        Ok(SigningOutcome {
            system_signature_mismatch: mismatch,
        })
    }

    /// Settings.checkAndConvertSharedUserSettingsLPw and SharedUserSetting's
    /// isSingleUser. Only an accepted parsed active member can leave. A lone
    /// disabled version must also have parsed successfully and be leaving.
    /// This changes candidate UID ownership, not disk or published queries.
    pub fn migrate_single_shared_user(
        &mut self,
        name: &str,
        strategy: SharedUidMigration,
        disabled: &BTreeMap<String, Record>,
    ) -> Result<bool, Error> {
        if strategy != SharedUidMigration::BestEffort {
            return Ok(false);
        }
        let group = self
            .identities
            .shared_users
            .get(name)
            .ok_or_else(|| Error {
                package: name.into(),
                path: String::new(),
                phase: "identity",
                message: "shared UID migration group disappeared (#803)".into(),
            })?;
        let id = group.app_id;
        let active: Vec<_> = self
            .settings
            .packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.shared_user && p.app_id == id)
            .map(|(at, _)| at)
            .collect();
        let old: Vec<_> = self
            .settings
            .disabled_system_packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.shared_user && p.app_id == id)
            .map(|(at, _)| at)
            .collect();
        if active.len() != 1 || old.len() > 1 {
            return Ok(false);
        }
        let package = &self.settings.packages[active[0]];
        if !self
            .parsed
            .iter()
            .any(|(n, app_id, _, leaving)| n == &package.name && *app_id == id && *leaving)
        {
            return Ok(false);
        }
        if let Some(&at) = old.first() {
            let saved = &self.settings.disabled_system_packages[at];
            if !disabled.get(&saved.name).is_some_and(|r| {
                r.settings == *saved
                    && r.parsed.package_name == saved.name
                    && r.parsed.is(booleans::LEAVING_SHARED_UID)
            }) {
                return Ok(false);
            }
        }
        // replaceSetting retains the same app ID and allocation cursor. All
        // fallible work precedes unlinking either active or disabled settings.
        self.identities
            .ids
            .replace(id, Owner::Package(package.name.clone()))
            .map_err(|e| Error {
                package: package.name.clone(),
                path: package.code_path.clone(),
                phase: "identity",
                message: format!("shared UID migration failed: {e:?}"),
            })?;
        self.settings.packages[active[0]].shared_user = false;
        if let Some(&at) = old.first() {
            self.settings.disabled_system_packages[at].shared_user = false;
        }
        self.settings.shared_users.retain(|g| g.name != name);
        self.identities.shared_users.remove(name);
        Ok(true)
    }
}

pub(super) fn selected_shared_user(
    saved_shared: bool,
    declared: Option<&str>,
    leaving: bool,
) -> Option<&str> {
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
