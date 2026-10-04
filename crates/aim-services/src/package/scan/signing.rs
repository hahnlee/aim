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
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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
    pub update_ownership: crate::package::owner::update_ownership::UpdateOwnership,
    pub installers: crate::package::owner::install_sources::Installers,
    pub(super) disabled_users: BTreeMap<String, super::disabled::DisabledUserStates>,
    pub(super) scanned_users: BTreeMap<String, BTreeMap<i32, UserState>>,
    pub(super) loaded: BTreeMap<String, Arc<super::LoadedPackage>>,
    pub(super) disabled_loaded: BTreeMap<String, Arc<super::LoadedPackage>>,
    pub(super) pending_metadata: BTreeSet<String>,
    apex_origins: BTreeMap<String, ScanOrigin>,
    pub(super) seinfo: Option<super::seinfo::Assignments>,
    pub(super) legacy_permissions: Option<super::legacy::Assignments>,
    pub(super) library_dependencies: Option<super::libraries::Assignments>,
    pub(super) shared_processes: Option<super::shared_processes::Assignments>,
    pub(super) replica_runtime: Option<super::replica_runtime::Assignments>,
    pub(super) hidden_api_allowlist: BTreeSet<String>,
    first_api_level: i32,
    parsed: Vec<(String, i32, SigningDetails, bool)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SigningError {
    Rejected(Error),
    Fatal(Error),
    NativeLibrary {
        package: String,
        path: String,
        error: super::NativeLibraryError,
    },
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
    pub(in crate::package) fn capture_ready(&self) -> bool {
        self.pending_metadata.is_empty()
    }
    /// Called at boot after active scans finish; saved-only and disabled code
    /// do not stand in for the original active PackageSetting.getPkg().
    pub fn fix_shared_seinfo_target_sdks_at_boot(&mut self) -> Result<(), String> {
        if !self.capture_ready() {
            return Err("scan metadata is not finalized".into());
        }
        let targets = self
            .loaded
            .iter()
            .map(|(name, code)| (name.clone(), code.package.target_sdk_version))
            .collect();
        for group in self.identities.shared_users.values_mut() {
            group.fix_seinfo_target_sdk_at_boot(&targets);
        }
        Ok(())
    }

    /// Native-parsed active code, admitted only after every scan metadata gate.
    /// Settings and user state remain in their owners; this is not a query replica.
    pub fn loaded_packages(&self) -> &BTreeMap<String, Arc<super::LoadedPackage>> {
        &self.loaded
    }

    /// Verified disabled factory code, kept apart from active UID membership.
    pub fn disabled_loaded_packages(&self) -> &BTreeMap<String, Arc<super::LoadedPackage>> {
        &self.disabled_loaded
    }

    pub(super) fn has_scanned_package(&self, name: &str) -> bool {
        self.loaded.contains_key(name) || self.parsed.iter().any(|(n, _, _, _)| n == name)
    }

    /// Apply the captured per-user state supplied by its commit owner.
    /// Store publication still requires the captured base. Disabled settings
    /// share only the users that the original setting aliases.
    pub fn set_user_state(
        &mut self,
        name: &str,
        user: i32,
        state: UserState,
    ) -> Result<(), String> {
        if user < 0 {
            return Err("package user id is negative".into());
        }
        let users = self
            .scanned_users
            .get_mut(name)
            .ok_or_else(|| "package user owner is not captured".to_string())?;
        users.insert(user, state);
        let users = users.clone();
        self.update_disabled_user_aliases(name, &users);
        Ok(())
    }
    pub fn set_user_runtime(
        &mut self,
        name: &str,
        user: i32,
        runtime: crate::package::owner::user_runtime::State,
    ) -> Result<(), String> {
        let mut state = self
            .scanned_users
            .get(name)
            .ok_or_else(|| "runtime package user owner is not captured".to_string())?
            .get(&user)
            .cloned()
            .unwrap_or_default();
        state.runtime = runtime;
        self.set_user_state(name, user, state)
    }

    /// Withdraw the scan's loaded package and declarations while retaining its
    /// saved setting, UID membership and user state for an ex-system rescan.
    /// Component/permission/property publication belongs to the commit owner.
    pub(super) fn withdraw_scanned_package(&mut self, record: &Record) {
        self.libraries.remove_scan_record(record);
        self.parsed
            .retain(|(name, _, _, _)| name != &record.settings.name);
        self.scanned_users.remove(&record.settings.name);
        self.loaded.remove(&record.settings.name);
        self.pending_metadata.remove(&record.settings.name);
        self.apex_origins.remove(&record.settings.name);
    }
    /// Apply page-size scan policy after ABI/path and installation ownership.
    /// Alignment errors retain existing flags and are returned for reporting.
    pub fn finish_page_size_metadata(
        &mut self,
        mut candidate: NewPackageOutcome,
        apks: &crate::package::write::Apks,
        policy: &super::PageSizeCompatPolicy,
        supported_64: &[String],
        install: super::NativeLibraryInstallPolicy,
        context: super::AbiScanContext<'_>,
    ) -> Result<(NewPackageOutcome, Option<String>), SigningError> {
        let record = &mut candidate.record;
        let at = self.accepted_slot(record, "page-size")?;
        let install = install.package_flags(&record.parsed);
        if record.parsed.path.as_deref() != Some(&record.settings.code_path) {
            return Err(SigningError::Rejected(Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "page-size",
                message: "parsed code path disagrees with accepted setting".into(),
            }));
        }
        let native_root = record.settings.legacy_native_library_path.clone();
        let alignment = || {
            let paths = super::NativeLibraryPaths {
                root: native_root.clone().ok_or("missing native library root")?,
                requires_isa: record.parsed.native_library_root_requires_isa,
                primary: record.parsed.native_library_dir.clone().unwrap_or_default(),
                secondary: record.parsed.secondary_native_library_dir.clone(),
            };
            apks.native_library_alignment(&record.parsed, supported_64, &paths, install)
        };
        let diagnostic = policy
            .apply_setting(
                &record.parsed,
                &mut record.settings,
                context,
                install.page_size,
                supported_64,
                &alignment,
            )
            .map_err(|error| SigningError::NativeLibrary {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                error,
            })?;
        self.settings.packages[at] = record.settings.clone();
        Ok((candidate, diagnostic))
    }

    /// Finish the accepted candidate's scan ABI branch and setting metadata.
    /// Required extraction remains an explicit rejection until its owner has
    /// completed it (#810); factory/reuse phases can commit without copying.
    /// Returns the original both-ABI/non-multiarch diagnostic to the caller.
    pub fn finish_native_library_metadata(
        &mut self,
        candidate: NewPackageOutcome,
        apks: &crate::package::write::Apks,
        policy: &super::AbiPolicy,
        env: &super::NativeLibraryEnvironment<'_>,
        context: super::AbiScanContext<'_>,
    ) -> Result<(NewPackageOutcome, bool), SigningError> {
        self.finish_native_library_phase(candidate, apks, policy, env, context, None)
            .map(|(candidate, mismatch, _)| (candidate, mismatch))
    }

    /// Complete required ordinary-storage copies before committing scan ABI
    /// metadata. The filesystem owner binds the writable root to the derived
    /// guest path; typed failures retain the accepted setting state.
    pub fn finish_native_library_install(
        &mut self,
        candidate: NewPackageOutcome,
        apks: &crate::package::write::Apks,
        policy: &super::AbiPolicy,
        env: &super::NativeLibraryEnvironment<'_>,
        context: super::AbiScanContext<'_>,
        install: super::NativeLibraryInstallPolicy,
        destination: &super::NativeLibraryDestination<'_>,
    ) -> Result<(NewPackageOutcome, bool, Vec<super::NativeLibraryAbiCopy>), SigningError> {
        self.finish_native_library_phase(
            candidate,
            apks,
            policy,
            env,
            context,
            Some((install, destination)),
        )
    }

    fn finish_native_library_phase(
        &mut self,
        mut candidate: NewPackageOutcome,
        apks: &crate::package::write::Apks,
        policy: &super::AbiPolicy,
        env: &super::NativeLibraryEnvironment<'_>,
        context: super::AbiScanContext<'_>,
        installation: Option<(
            super::NativeLibraryInstallPolicy,
            &super::NativeLibraryDestination<'_>,
        )>,
    ) -> Result<(NewPackageOutcome, bool, Vec<super::NativeLibraryAbiCopy>), SigningError> {
        let record = &mut candidate.record;
        let at = self.accepted_slot(record, "native-library")?;
        let reject = |message| {
            SigningError::Rejected(Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "native-library",
                message,
            })
        };
        if record.parsed.path.as_deref() != Some(&record.settings.code_path) {
            return Err(reject(
                "parsed code path disagrees with accepted setting".into(),
            ));
        }
        let error = |error| SigningError::NativeLibrary {
            package: record.settings.name.clone(),
            path: record.settings.code_path.clone(),
            error,
        };
        let scan = apks
            .scan_native_libraries(&record.parsed, policy, env, context)
            .map_err(error)?;
        let mut mismatch = false;
        let mut copies = Vec::new();
        if let Some(scan) = scan {
            if scan.requires_extraction {
                let Some((install, destination)) = installation else {
                    return Err(reject(
                        "native library extraction has not completed (#810)".into(),
                    ));
                };
                if destination.guest_root != scan.paths.root {
                    return Err(reject(
                        "writable native root disagrees with derived guest path".into(),
                    ));
                }
                let install = install.package_flags(&record.parsed);
                for abi in &scan.extraction_abis {
                    let copied = apks
                        .copy_native_libraries_for_supported_abi(
                            &record.parsed,
                            std::slice::from_ref(abi),
                            scan.paths.requires_isa,
                            install,
                            destination,
                        )
                        .map_err(|cause| {
                            let multi = record.parsed.is(booleans::MULTI_ARCH);
                            let code = if multi { cause.code } else { -110 };
                            let message = if multi {
                                let wide =
                                    policy.bit64.contains(abi) || policy.native64.contains(abi);
                                format!(
                                    "Error unpackaging {} bit native libs for multiarch app.",
                                    if wide { 64 } else { 32 }
                                )
                            } else {
                                format!(
                                    "Error unpackaging native libs for app, errorCode={}",
                                    cause.code
                                )
                            };
                            error(super::NativeLibraryError::Copy {
                                code,
                                message,
                                cause,
                            })
                        })?;
                    copies.push(copied);
                }
            }
            mismatch = scan.multi_arch_mismatch;
            scan.apply_metadata(&mut record.parsed);
        }
        context
            .apply_setting(&record.parsed, &mut record.settings)
            .map_err(|error| SigningError::NativeLibrary {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                error,
            })?;
        self.settings.packages[at] = record.settings.clone();
        Ok((candidate, mismatch, copies))
    }

    pub(super) fn accepted_slot(
        &self,
        record: &Record,
        phase: &'static str,
    ) -> Result<usize, SigningError> {
        let reject = || {
            SigningError::Rejected(Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase,
                message: "metadata candidate is stale or was not reconciled".into(),
            })
        };
        let at = self
            .settings
            .packages
            .iter()
            .position(|p| *p == record.settings)
            .ok_or_else(reject)?;
        if record.parsed.package_name != record.settings.name
            || record.identity.internal_name != record.settings.name
            || !self.parsed.iter().any(|(name, id, signing, leaving)| {
                name == &record.settings.name
                    && *id == record.settings.uid_owner_id()
                    && signing == &record.signing
                    && *leaving == record.parsed.is(booleans::LEAVING_SHARED_UID)
            })
        {
            return Err(reject());
        }
        Ok(at)
    }
    /// Read the accepted code's actual file timestamp before finishing metadata.
    /// No original image/data file is written, and read failures leave the scan
    /// candidate unchanged. Full scan reconciliation handles removed code.
    pub fn finish_code_metadata(
        &mut self,
        candidate: NewPackageOutcome,
        apks: &crate::package::write::Apks,
        clock: super::ScanClock,
    ) -> Result<NewPackageOutcome, SigningError> {
        let record = &candidate.record;
        let reject = |message| {
            SigningError::Rejected(Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "code-time",
                message,
            })
        };
        if record.parsed.path.as_deref() != Some(&record.settings.code_path) {
            return Err(reject(
                "parsed code path disagrees with accepted setting".into(),
            ));
        }
        let file_time = apks.scan_file_time(&record.parsed).map_err(reject)?;
        self.finish_metadata(
            candidate,
            super::ScanTime {
                current_time: clock.current_time,
                file_time,
                user_id: clock.user_id,
                update_time: clock.update_time,
            },
        )
    }

    /// Finish timestamp/version/volume metadata of this accepted setting
    /// candidate. The caller obtains time inputs from the scan clock and
    /// verified code owner. ABI metadata has its own accepted stage; final
    /// flags and snapshot publication are separate.
    pub fn finish_metadata(
        &mut self,
        mut candidate: NewPackageOutcome,
        time: super::ScanTime,
    ) -> Result<NewPackageOutcome, SigningError> {
        let record = &mut candidate.record;
        let at = self.accepted_slot(record, "metadata")?;
        let system_directory = if record.parsed.is2(crate::package::pkg::booleans2::APEX) {
            if self.apex_origins.get(&record.settings.name) != Some(&record.origin) {
                return Err(SigningError::Rejected(Error {
                    package: record.settings.name.clone(),
                    path: record.settings.code_path.clone(),
                    phase: "location",
                    message: "APEX metadata has no original scan origin".into(),
                }));
            }
            record.origin == ScanOrigin::SystemDirectory
        } else {
            physical_parse_flags(&record.settings.code_path).map_err(|message| {
                SigningError::Rejected(Error {
                    package: record.settings.name.clone(),
                    path: record.settings.code_path.clone(),
                    phase: "location",
                    message,
                })
            })? & parse::PARSE_IS_SYSTEM_DIR
                != 0
        };
        super::enrich::apply(
            &mut record.settings,
            &record.parsed,
            &mut candidate.users,
            time,
            system_directory,
        );
        self.settings.packages[at] = record.settings.clone();
        self.update_disabled_user_aliases(&record.settings.name, &candidate.users);
        Ok(candidate)
    }

    pub fn new(
        config: &SystemConfig,
        settings: &Settings,
        first_api_level: i32,
    ) -> Result<Self, RestoreError> {
        Self::with_identities(
            config,
            settings,
            first_api_level,
            Bootstrap::restore(config, settings)?,
        )
    }

    /// APEX containers are committed with INVALID_UID, not registered in
    /// AppIdSettingMap. Only verified original inventory identities may bypass
    /// APK UID restoration; the complete Settings graph remains authoritative.
    pub fn new_after_apex(
        config: &SystemConfig,
        settings: &Settings,
        first_api_level: i32,
        apex: &super::ApexImage,
    ) -> Result<Self, RestoreError> {
        let group_matches = |package: &crate::package::settings::Package,
                             source: &super::ApexCode| {
            let declared = selected_shared_user(
                package.shared_user,
                source.parsed.shared_user_id.as_deref(),
                source.parsed.is(booleans::LEAVING_SHARED_UID),
            );
            match package.shared_app_id() {
                Some(id) => {
                    id > 0
                        && settings.shared_users.iter().any(|group| {
                            group.app_id == id && declared == Some(group.name.as_str())
                        })
                }
                None => declared.is_none(),
            }
        };
        for package in &settings.disabled_system_packages {
            let source = apex.packages.iter().find(|code| {
                code.parsed.package_name == package.name
                    && code.info.module_path == package.code_path
            });
            if let Some(source) = source {
                let version = (i64::from(source.parsed.version_code_major) << 32)
                    | i64::from(source.parsed.version_code as u32);
                if package.app_id != package.shared_app_id().unwrap_or(-1)
                    || !group_matches(package, source)
                    || package.version_code != version
                {
                    return Err(RestoreError::Apex(format!(
                        "disabled APEX setting {} disagrees with scanned identity/version or INVALID_UID",
                        package.name
                    )));
                }
            } else if package.app_id < 0 {
                return Err(RestoreError::Apex(format!(
                    "disabled setting {} has no verified INVALID_UID owner",
                    package.name
                )));
            }
        }
        let mut uid_settings = settings.clone();
        uid_settings.packages.clear();
        let mut names = BTreeSet::new();
        for package in &settings.packages {
            if !names.insert(&package.name) {
                return Err(RestoreError::Settings(
                    crate::package::owner::app_ids::Error::DuplicatePackage(package.name.clone()),
                ));
            }
            let source = apex.packages.iter().find(|code| {
                code.parsed.package_name == package.name
                    && code.info.module_path == package.code_path
            });
            if let Some(source) = source {
                let version = (i64::from(source.parsed.version_code_major) << 32)
                    | i64::from(source.parsed.version_code as u32);
                if package.app_id != package.shared_app_id().unwrap_or(-1)
                    || !group_matches(package, source)
                    || package.version_code != version
                {
                    return Err(RestoreError::Apex(format!(
                        "APEX setting {} disagrees with scanned identity/version or INVALID_UID",
                        package.name
                    )));
                }
            } else {
                uid_settings.packages.push(package.clone());
            }
        }
        let mut identities = Bootstrap::restore(config, &uid_settings)?;
        for package in settings.packages.iter().filter(|p| {
            p.shared_user
                && apex.packages.iter().any(|code| {
                    code.parsed.package_name == p.name && code.info.module_path == p.code_path
                })
        }) {
            let group = identities
                .shared_users
                .values_mut()
                .find(|group| Some(group.app_id) == package.shared_app_id())
                .ok_or_else(|| RestoreError::Apex("APEX shared UID owner is missing".into()))?;
            group.add_package(&package.name, package.flags, package.private_flags);
        }
        Self::with_identities(config, settings, first_api_level, identities)
    }

    fn with_identities(
        config: &SystemConfig,
        settings: &Settings,
        first_api_level: i32,
        identities: Bootstrap,
    ) -> Result<Self, RestoreError> {
        let mut restored = settings.clone();
        crate::package::owner::key_sets::restore(&mut restored).map_err(RestoreError::KeySets)?;
        Ok(Self {
            identities,
            settings: restored,
            libraries: Registry::new(config),
            update_ownership: Default::default(),
            installers: crate::package::owner::install_sources::Installers::restore(settings),
            // readDisabledSysPackageLPw creates fresh settings; it does not
            // restore the active package's restriction state into them.
            disabled_users: settings
                .disabled_system_packages
                .iter()
                .map(|p| (p.name.clone(), Default::default()))
                .collect(),
            scanned_users: BTreeMap::new(),
            loaded: BTreeMap::new(),
            disabled_loaded: BTreeMap::new(),
            pending_metadata: BTreeSet::new(),
            apex_origins: BTreeMap::new(),
            seinfo: None,
            legacy_permissions: None,
            library_dependencies: None,
            shared_processes: None,
            replica_runtime: None,
            hidden_api_allowlist: config.hidden_api_allowlist.iter().cloned().collect(),
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
        let identity = Identity::select_for_location(
            &code.parsed,
            &self.settings,
            update.flags & crate::package::settings::FLAG_SYSTEM != 0,
            &code.location,
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
                .find(|g| Some(g.app_id) == original.shared_app_id())
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
        if self.identities.ids.get(original.uid_owner_id()) != Some(&owner) {
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
        let mut parsed = code
            .collected_package()
            .map_err(|e| reject("signatures", &e))?;
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
        next.update_disabled_user_aliases(&record.settings.name, &setting.users);
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
        let selected =
            Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
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
            self.has_scanned_package(name)
        })
        .ok_or_else(|| reject("identity", "no eligible original system package"))?;
        let group = if original.shared_user {
            self.settings
                .shared_users
                .iter()
                .find(|g| Some(g.app_id) == original.shared_app_id())
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
        if self.identities.ids.get(original.uid_owner_id()) != Some(&expected_owner) {
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
        let mut parsed = code
            .collected_package()
            .map_err(|e| reject("signatures", &e))?;
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
        next.update_disabled_user_aliases(&record.settings.name, &setting.users);
        *self = next;
        Ok(NewPackageOutcome {
            record,
            users: setting.users,
            signing,
        })
    }

    /// Prepare verified new system-directory code and reconcile its signers.
    /// Shared-UID admission follows final application metadata. Call
    /// scan_new_system for allocation cleanup across all metadata gates.
    /// Rejection retains the original UID cleanup cursor and an allocated group
    /// until final pruning; this candidate does not publish package/user state.
    pub fn apply_new_system(
        &mut self,
        code: &Code,
        metadata: SettingMetadata,
        users: UserPolicy<'_>,
    ) -> Result<NewPackageOutcome, SigningError> {
        self.remove_stale_disabled_system(code)?;
        let (candidate, mut preparation) = self.prepare_new_system(code, metadata, users)?;
        preparation
            .accept_uid(&candidate.record.settings.name)
            .map_err(SigningError::Fatal)?;
        Ok(candidate)
    }

    pub(super) fn prepare_new_system(
        &mut self,
        code: &Code,
        metadata: SettingMetadata,
        users: UserPolicy<'_>,
    ) -> Result<(NewPackageOutcome, UidScan), SigningError> {
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
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
        if Identity::original_setting(&code.parsed, &self.settings, &|name| {
            self.has_scanned_package(name)
        })
        .is_some()
        {
            return Err(reject(
                "identity",
                "new system scan must adopt the eligible original setting (#804)",
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
        let mut parsed = code
            .collected_package()
            .map_err(|e| reject("signatures", &e))?;
        identity.apply(&mut parsed);
        let mut record = Record {
            settings: setting.package,
            parsed,
            signing: code.signing.clone(),
            identity,
            origin: ScanOrigin::SystemDirectory,
        };
        next.settings.packages.push(record.settings.clone());
        let signing = match next.apply_candidate(&record, None, false) {
            Ok(outcome) => outcome,
            Err(error) => {
                preparation
                    .reject_pending(&record.settings.name)
                    .map_err(SigningError::Fatal)?;
                self.identities = preparation.identities;
                return Err(error);
            }
        };
        record.settings = next.settings.packages.last().unwrap().clone();
        *self = next;
        Ok((
            NewPackageOutcome {
                record,
                users: setting.users,
                signing,
            },
            preparation,
        ))
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
        self.apply_candidate(record, disabled, true)
    }

    pub(super) fn apply_apex_candidate(
        &mut self,
        record: &Record,
        source: &super::ApexCode,
    ) -> Result<SigningOutcome, SigningError> {
        if !record.parsed.is2(crate::package::pkg::booleans2::APEX)
            || record.settings.app_id != -1
            || record.settings.name != source.parsed.package_name
            || record.settings.code_path != source.info.module_path
        {
            return Err(SigningError::Rejected(Error {
                package: record.settings.name.clone(),
                path: record.settings.code_path.clone(),
                phase: "apex-origin",
                message: "container record disagrees with original scan source".into(),
            }));
        }
        let outcome =
            self.apply_candidate_with_flags(record, None, false, Some(source.scan_parse_flags))?;
        self.apex_origins
            .insert(record.settings.name.clone(), record.origin);
        Ok(outcome)
    }

    fn apply_candidate(
        &mut self,
        record: &Record,
        disabled: Option<&Record>,
        admit_member: bool,
    ) -> Result<SigningOutcome, SigningError> {
        self.apply_candidate_with_flags(record, disabled, admit_member, None)
    }

    fn apply_candidate_with_flags(
        &mut self,
        record: &Record,
        disabled: Option<&Record>,
        admit_member: bool,
        apex_parse_flags: Option<i32>,
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
        self.reconcile(
            record,
            check.as_ref(),
            disabled,
            admit_member,
            apex_parse_flags,
        )
    }

    fn reconcile(
        &mut self,
        record: &Record,
        signature_check: Option<&crate::package::settings::Package>,
        disabled: Option<&Record>,
        admit_member: bool,
        apex_parse_flags: Option<i32>,
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
            || previous.shared_app_id() != record.settings.shared_app_id()
            || previous.code_path != record.settings.code_path
            || record.identity.internal_name != previous.name
            || record.parsed.package_name != previous.name
        {
            return Err(reject(
                "identity",
                "saved code or UID ownership changed (#804)".into(),
            ));
        }
        let flags = match apex_parse_flags {
            Some(flags) => flags,
            None => physical_parse_flags(&record.settings.code_path)
                .map_err(|e| reject("location", e))?,
        };
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
                    .find(|g| Some(g.app_id) == previous.shared_app_id())
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
                        .filter(|(name, id, _, _)| {
                            name != &previous.name && *id == previous.uid_owner_id()
                        })
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
        if let Some(group) = &mut group
            && admit_member
        {
            group.add_package_with_code(
                &record.settings.name,
                record.settings.flags,
                record.settings.private_flags,
                Some(record.parsed.target_sdk_version),
            );
        }
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
            record.settings.uid_owner_id(),
            record.signing.clone(),
            record.parsed.is(booleans::LEAVING_SHARED_UID),
        ));
        self.pending_metadata.insert(record.settings.name.clone());
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
            .filter(|(_, p)| p.shared_app_id() == Some(id))
            .map(|(at, _)| at)
            .collect();
        let old: Vec<_> = self
            .settings
            .disabled_system_packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.shared_app_id() == Some(id))
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
        self.settings.packages[active[0]].shared_user_app_id = None;
        if let Some(&at) = old.first() {
            self.settings.disabled_system_packages[at].shared_user = false;
            self.settings.disabled_system_packages[at].shared_user_app_id = None;
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
    #[test]
    fn boot_shared_seinfo_uses_active_code_and_rejects_pending_metadata() {
        use super::*;

        let settings = Settings {
            shared_users: vec![SharedUser {
                name: "android.uid.system".into(),
                app_id: 1000,
                ..Default::default()
            }],
            packages: vec![crate::package::settings::Package {
                name: "active".into(),
                app_id: 1000,
                shared_user: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        let signing = SigningDetails {
            signatures: vec![vec![3]],
            scheme_version: 3,
            public_keys: vec![],
            past_signing_certificates: None,
        };
        let code = |sdk| {
            Arc::new(
                super::super::LoadedPackage::new(
                    crate::package::pkg::AndroidPackage {
                        package_name: "active".into(),
                        target_sdk_version: sdk,
                        signing_details: Some(signing.parcel_details().unwrap()),
                        ..Default::default()
                    },
                    signing.clone(),
                )
                .unwrap(),
            )
        };
        owner.loaded.insert("active".into(), code(35));
        owner.disabled_loaded.insert("active".into(), code(16));
        owner.pending_metadata.insert("active".into());
        let before = owner.clone();
        assert_eq!(
            owner.fix_shared_seinfo_target_sdks_at_boot(),
            Err("scan metadata is not finalized".into())
        );
        assert_eq!(owner, before);
        owner.pending_metadata.clear();
        owner.fix_shared_seinfo_target_sdks_at_boot().unwrap();
        assert_eq!(
            owner.identities.shared_users["android.uid.system"].seinfo_target_sdk(),
            35
        );
        assert_eq!(
            before.identities.shared_users["android.uid.system"].seinfo_target_sdk(),
            10000
        );
        assert_eq!(owner.seinfo("active"), Err("seInfo is not assigned".into()));
        let policy = crate::package::owner::seinfo::Policy::unread();
        owner
            .assign_seinfo_at_boot(&policy, &mut |_| {
                panic!("nonempty shared UID must not query compatibility")
            })
            .unwrap();
        assert_eq!(
            owner.seinfo("active").unwrap(),
            Some("default:privapp:targetSdkVersion=35")
        );
        owner.settings.packages[0].private_flags |= 1 << 18;
        owner.loaded.insert("active".into(), code(19));
        owner
            .assign_seinfo_for_scan(
                "active",
                super::super::SeInfoSetting::Retained,
                &policy,
                &mut |_| panic!("shared runtime SDK"),
            )
            .unwrap();
        let state = owner.seinfo_state("active").unwrap().unwrap();
        assert_eq!(
            owner.identities.shared_users["android.uid.system"].seinfo_target_sdk(),
            35
        );
        assert_eq!(
            state.base.as_deref(),
            Some("default:privapp:targetSdkVersion=35:partition=vendor")
        );
        assert_eq!(
            state.override_label.as_deref(),
            Some("default:privapp:targetSdkVersion=35")
        );
        let retained = owner.clone();
        owner
            .assign_seinfo_for_scan(
                "active",
                super::super::SeInfoSetting::New,
                &policy,
                &mut |_| panic!("shared runtime SDK"),
            )
            .unwrap();
        assert!(
            owner
                .seinfo_state("active")
                .unwrap()
                .unwrap()
                .override_label
                .is_none()
        );
        assert_eq!(
            owner.seinfo("active").unwrap(),
            Some("default:privapp:targetSdkVersion=35:partition=vendor")
        );
        assert_eq!(
            retained.seinfo("active").unwrap(),
            Some("default:privapp:targetSdkVersion=35")
        );
        assert_eq!(owner.seinfo("absent").unwrap(), None);
        let captured = owner.clone();
        owner.settings.packages[0].private_flags = 8;
        assert_eq!(
            owner.seinfo("active"),
            Err("seInfo inputs changed since assignment".into())
        );
        assert_eq!(
            captured.seinfo("active").unwrap(),
            Some("default:privapp:targetSdkVersion=35:partition=vendor")
        );
        owner
            .assign_seinfo_at_boot(&policy, &mut |_| panic!("shared UID"))
            .unwrap();
        assert_eq!(
            owner.seinfo("active").unwrap(),
            Some("default:privapp:targetSdkVersion=19")
        );
    }

    #[test]
    fn binder_scan_lease_pins_old_code_pages_and_close_releases_capture() {
        use super::*;
        use crate::package::scan_snapshot::{
            Store,
            endpoint::{Endpoint, MAX_CHUNK},
        };
        use aim_binder_driver::{
            Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*,
        };
        use aim_binder_host::{
            local::LocalProcess,
            parcel::{Binder, Parcel},
        };
        use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
        struct NoMemory;
        impl GuestProcess for NoMemory {
            fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> {
                Err(errno::EFAULT)
            }
            fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> {
                Err(errno::EFAULT)
            }
            fn get_file(&mut self, _: u32) -> Result<File, Errno> {
                Err(errno::EBADF)
            }
            fn install_file(&mut self, _: File) -> Result<u32, Errno> {
                Err(errno::EBADF)
            }
            fn close_fd(&mut self, _: u32) {
                panic!("unexpected fd")
            }
        }
        struct Processes(Arc<Driver>, Vec<Arc<LocalProcess>>);
        impl Drop for Processes {
            fn drop(&mut self) {
                for p in &self.1 {
                    self.0.release(p.proc_handle());
                }
            }
        }
        let package = crate::package::settings::Package {
            name: "fixture".into(),
            code_path: "/data/app/fixture".into(),
            app_id: 10100,
            ..Default::default()
        };
        let mut owner = SigningScan::new(
            &Default::default(),
            &Settings {
                packages: vec![package.clone()],
                ..Default::default()
            },
            36,
        )
        .unwrap();
        let signing = SigningDetails {
            signatures: vec![vec![3]],
            scheme_version: 3,
            public_keys: vec![],
            past_signing_certificates: Some(vec![(vec![1], 21), (vec![3], 23)]),
        };
        let parsed = crate::package::pkg::AndroidPackage {
            feature_flag_state: Some(Vec::new()),
            package_name: package.name.clone(),
            path: Some(package.code_path.clone()),
            uid: package.app_id,
            signing_details: Some(signing.parcel_details().unwrap()),
            version_name: Some("x".repeat(150_000)),
            ..Default::default()
        };
        owner.loaded.insert(
            package.name.clone(),
            Arc::new(super::super::LoadedPackage::new(parsed, signing).unwrap()),
        );
        let mut user = crate::package::restrictions::UserState {
            stopped: true,
            harmful_app_warning: Some("warning".repeat(30_000)),
            enabled_components: Some(vec!["fixture.Activity".into()]),
            ..Default::default()
        };
        user.archive_state = Some(crate::package::restrictions::ArchiveState {
            installer_title: "installer".into(),
            archive_time: 123,
            activities: vec![crate::package::restrictions::ArchiveActivity {
                title: "no icon".into(),
                original_component_name: "fixture/Activity".into(),
                icon_path: None,
                monochrome_icon_path: Some("/data/mono".into()),
            }],
        });
        user.runtime.set_library_overlay_paths(
            "library".into(),
            Some(crate::package::model::OverlayPaths {
                resource_dirs: vec!["/overlay.apk".into()],
                overlay_paths: vec!["/overlay.apk".into()],
            }),
        );
        user.runtime.override_label_icon(
            crate::package::owner::user_runtime::Component {
                package: "fixture".into(),
                class: "Activity".into(),
            },
            crate::package::owner::user_runtime::LabelIcon {
                label: Some("old label".into()),
                icon: Some(17),
            },
        );
        owner
            .scanned_users
            .insert(package.name.clone(), BTreeMap::from([(10, user)]));
        let mut usage = crate::package::owner::usage::Usage::new(["fixture"]);
        usage.notify("fixture", 0, 17);
        usage.notify("fixture", 2, 29);
        let policy = crate::package::owner::seinfo::Policy::unread();
        owner
            .assign_seinfo_at_boot(&policy, &mut |_| Ok(30))
            .unwrap();
        owner.settings.packages[0].add_mime_types("group".into(), ["text/plain".into()]);
        owner.settings.packages[0].add_nullable_mime_types(None, [None, Some(String::new())]);
        owner.settings.packages[0].uses_static_libraries = vec![("static".into(), 17)];
        owner.settings.packages[0].add_old_path(Some("/data/old"));
        owner.settings.packages[0].set_loading_progress(0.5);
        owner.settings.packages[0].add_old_path(Some(&"x".repeat(150_000)));
        let mut legacy = crate::package::owner::legacy_permissions::Migration::default();
        legacy
            .put(
                10,
                crate::package::owner::legacy_permissions::Permission {
                    name: Some("x".repeat(150_000)),
                    runtime: false,
                    granted: true,
                    flags: 17,
                },
            )
            .unwrap();
        legacy.set_missing(10, true).unwrap();
        owner
            .capture_legacy_permissions(
                &[10, 0],
                BTreeMap::from([(("fixture".into(), false), legacy)]),
                owner
                    .identities
                    .shared_users
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            crate::package::owner::legacy_permissions::Migration::default(),
                        )
                    })
                    .collect(),
            )
            .unwrap();
        owner
            .capture_install_permissions_fixed(BTreeMap::from([(("fixture".into(), false), true)]))
            .unwrap();
        owner
            .capture_leaving_shared_users(BTreeMap::from([(("fixture".into(), false), true)]))
            .unwrap();
        owner
            .complete_library_dependencies(&|_, _| {
                Ok(crate::package::libraries::Policy::pinned(false))
            })
            .unwrap();
        owner
            .capture_transient_states(BTreeMap::from([(
                ("fixture".into(), false),
                crate::package::owner::transient::State {
                    hidden_until_installed: true,
                    updated_system_app: true,
                    apk_in_updated_apex: true,
                    apex_module_name: Some("module".into()),
                },
            )]))
            .unwrap();
        let store = Store::new(owner.clone(), usage).unwrap();
        let base = store.capture();
        for mode in 0..3 {
            let mut invalid = owner.clone();
            let code = &mut Arc::make_mut(invalid.loaded.get_mut("fixture").unwrap()).package;
            match mode {
                0 => code.feature_flag_state = None,
                1 => code.feature_flag_state = Some(vec![None]),
                _ => {
                    code.processes = Some(vec![crate::package::pkg::Process {
                        name: None,
                        ..Default::default()
                    }])
                }
            }
            assert!(matches!(
                store.publish(&base, invalid, base.usage().clone()),
                Err(crate::package::scan_snapshot::Error::Invalid(_))
            ));
            assert!(Arc::ptr_eq(&store.capture(), &base));
        }

        let captured_library =
            crate::package::scan_snapshot::library_record::captured(&base, "fixture")
                .unwrap()
                .unwrap();
        let mut invalid = owner.clone();
        invalid.settings.packages[0]
            .mime_groups
            .push((Some("group".into()), Vec::new()));
        assert!(matches!(
            store.publish(&base, invalid, base.usage().clone()),
            Err(crate::package::scan_snapshot::Error::Invalid(_))
        ));
        assert!(Arc::ptr_eq(&base, &store.capture()));
        let mut stale = owner.clone();
        stale.settings.packages[0].private_flags = 8;
        assert!(matches!(
            store.publish(&base, stale, base.usage().clone()),
            Err(crate::package::scan_snapshot::Error::Invalid(_))
        ));
        assert!(Arc::ptr_eq(&base, &store.capture()));
        let captured_user =
            crate::package::scan_snapshot::user_record::captured(&base, "fixture", false, 10)
                .unwrap()
                .unwrap();
        assert!(captured_user.len() > MAX_CHUNK * 2);
        let captured_setting =
            crate::package::scan_snapshot::setting_record::captured(&base, "fixture", false)
                .unwrap()
                .unwrap();
        assert!(captured_setting.len() > MAX_CHUNK * 2);
        let old = Arc::downgrade(&base);
        let endpoint = Arc::new(Endpoint::new(base.clone()));
        let mut changed_usage = base.usage().clone();
        changed_usage.notify("fixture", 0, 99);
        owner
            .assign_seinfo_at_boot(&policy, &mut |_| Ok(10000))
            .unwrap();
        owner
            .scanned_users
            .get_mut("fixture")
            .unwrap()
            .get_mut(&10)
            .unwrap()
            .harmful_app_warning = Some("new warning".into());
        let mut runtime = owner.scanned_user_states("fixture").unwrap()[&10]
            .runtime
            .clone();
        runtime.reset_label_icons();
        owner.set_user_runtime("fixture", 10, runtime).unwrap();
        assert!(
            owner
                .set_user_runtime("absent", 10, Default::default())
                .is_err()
        );
        assert!(
            owner
                .set_user_runtime("fixture", -1, Default::default())
                .is_err()
        );
        let mut state = owner.scanned_user_states("fixture").unwrap()[&10].clone();
        state.archive_state.as_mut().unwrap().activities[0].icon_path =
            Some("/data/new-icon".into());
        owner.set_user_state("fixture", 10, state).unwrap();
        owner
            .capture_transient_states(BTreeMap::from([(
                ("fixture".into(), false),
                Default::default(),
            )]))
            .unwrap();
        owner.settings.packages[0].remove_old_path(Some("/data/old"));
        owner.settings.packages[0].set_loading_progress(1.0);
        owner.settings.packages[0].remove_old_path(Some(&"x".repeat(150_000)));
        owner.settings.packages[0].add_mime_types("group".into(), ["image/png".into()]);
        owner.settings.packages[0]
            .mime_groups
            .retain(|(name, _)| name.is_some());
        owner.settings.packages[0].uses_static_libraries[0].1 = 29;
        let mut updated_legacy = crate::package::owner::legacy_permissions::Migration::default();
        updated_legacy
            .put(
                10,
                crate::package::owner::legacy_permissions::Permission {
                    name: Some("x".repeat(150_000)),
                    runtime: true,
                    granted: false,
                    flags: 33,
                },
            )
            .unwrap();
        owner
            .capture_legacy_permissions(
                &[10, 0],
                BTreeMap::from([(("fixture".into(), false), updated_legacy)]),
                owner
                    .identities
                    .shared_users
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            crate::package::owner::legacy_permissions::Migration::default(),
                        )
                    })
                    .collect(),
            )
            .unwrap();
        owner
            .capture_install_permissions_fixed(BTreeMap::from([(("fixture".into(), false), true)]))
            .unwrap();
        owner
            .set_install_permissions_fixed("fixture", false, false)
            .unwrap();
        owner
            .capture_leaving_shared_users(BTreeMap::from([(("fixture".into(), false), false)]))
            .unwrap();
        owner
            .set_user_state(
                "fixture",
                11,
                crate::package::restrictions::UserState::default(),
            )
            .unwrap();
        owner
            .complete_library_dependencies(&|_, _| {
                Ok(crate::package::libraries::Policy::pinned(false))
            })
            .unwrap();
        let current = store.publish(&base, owner, changed_usage).unwrap();
        assert_eq!(
            crate::package::scan_snapshot::user_record::ids(&base, "fixture", false).unwrap(),
            Some(vec![10])
        );
        assert_eq!(
            crate::package::scan_snapshot::user_record::ids(&current, "fixture", false).unwrap(),
            Some(vec![10, 11])
        );
        assert_eq!(
            crate::package::scan_snapshot::user_record::ids(&base, "missing", false).unwrap(),
            None
        );
        let mut sparse = base.owner().clone();
        sparse.scanned_users.get_mut("fixture").unwrap().clear();
        sparse
            .complete_library_dependencies(&|_, _| {
                Ok(crate::package::libraries::Policy::pinned(false))
            })
            .unwrap();
        let sparse = Store::new(sparse, base.usage().clone()).unwrap().capture();
        assert_eq!(
            crate::package::scan_snapshot::user_record::ids(&sparse, "fixture", false).unwrap(),
            Some(vec![])
        );
        let mut unresolved = base.owner().clone();
        unresolved.scanned_users.remove("fixture");
        assert!(Store::new(unresolved, base.usage().clone()).is_err());
        assert_eq!(
            base.owner()
                .install_permissions_fixed("fixture", false)
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            current
                .owner()
                .install_permissions_fixed("fixture", false)
                .unwrap(),
            Some(false)
        );
        assert_eq!(
            base.owner()
                .legacy_permissions("fixture", false)
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            17
        );
        assert_eq!(
            current
                .owner()
                .legacy_permissions("fixture", false)
                .unwrap()
                .unwrap()
                .user(10)
                .unwrap()
                .permissions[0]
                .flags,
            33
        );
        assert_eq!(
            base.owner().settings.packages[0]
                .mime_groups
                .iter()
                .find(|(name, _)| name.is_none())
                .unwrap()
                .1,
            [None, Some(String::new())]
        );
        assert!(
            current.owner().settings.packages[0]
                .mime_groups
                .iter()
                .all(|(name, _)| name.is_some())
        );
        assert_eq!(
            base.owner().settings.packages[0]
                .mime_groups
                .iter()
                .find(|(name, _)| name.as_deref() == Some("group"))
                .unwrap()
                .1,
            [Some("text/plain".into())]
        );
        assert_eq!(
            base.owner().settings.packages[0].uses_static_libraries[0].1,
            17
        );
        assert_eq!(
            current.owner().settings.packages[0].uses_static_libraries[0].1,
            29
        );
        assert_eq!(
            base.owner().settings.packages[0].old_paths,
            Some(vec![Some("/data/old".into()), Some("x".repeat(150_000))])
        );
        assert_eq!(current.owner().settings.packages[0].old_paths, Some(vec![]));
        assert!(base.owner().settings.packages[0].is_loading());
        assert!(!current.owner().settings.packages[0].is_loading());

        assert_eq!(
            base.owner().scanned_user_states("fixture").unwrap()[&10]
                .archive_state
                .as_ref()
                .unwrap()
                .activities[0]
                .icon_path,
            None
        );
        assert_eq!(
            current.owner().scanned_user_states("fixture").unwrap()[&10]
                .archive_state
                .as_ref()
                .unwrap()
                .activities[0]
                .icon_path
                .as_deref(),
            Some("/data/new-icon")
        );

        assert!(
            current.owner().scanned_user_states("fixture").unwrap()[&10]
                .runtime
                .overrides()
                .is_none()
        );
        assert_eq!(
            base.owner().scanned_user_states("fixture").unwrap()[&10]
                .runtime
                .overrides()
                .unwrap()[0]
                .1
                .label
                .as_deref(),
            Some("old label")
        );
        assert_ne!(
            crate::package::scan_snapshot::user_record::captured(&current, "fixture", false, 10,)
                .unwrap()
                .unwrap(),
            captured_user
        );
        assert_eq!(
            current.owner().seinfo("fixture").unwrap(),
            Some("default:targetSdkVersion=10000")
        );
        assert_eq!(
            base.owner().seinfo("fixture").unwrap(),
            Some("default:targetSdkVersion=30")
        );
        drop(base);
        let driver = Driver::new();
        let open = |pid, euid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid,
                    security_context: None,
                },
            )
        };
        let server = open(96001, 1000);
        let client = open(96002, 1000);
        let foreign = open(96003, 10100);
        let _processes = Processes(
            driver.clone(),
            vec![server.clone(), client.clone(), foreign.clone()],
        );
        let Binder::Local(ptr) = server.add_service(endpoint) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                server.proc_handle(),
                96001,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        server.start();
        let remote = client.strong(0);
        let request = || {
            let mut p = Parcel::new();
            p.write_interface_token(api::DESCRIPTOR);
            p
        };
        let policy_request = |name: Option<&str>, disabled| {
            let mut p = Parcel::new();
            api::GetHiddenApiEnforcementPolicy {
                package_name: name.map(str::to_owned),
                disabled,
            }
            .write(&mut p);
            p
        };
        let reply = remote
            .transact(
                api::GET_HIDDEN_API_ENFORCEMENT_POLICY,
                &policy_request(Some("fixture"), false),
                false,
            )
            .unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(r.remaining(), 0);
        for (name, factory) in [
            (None, false),
            (Some("missing"), false),
            (Some("fixture"), true),
        ] {
            let reply = remote
                .transact(
                    api::GET_HIDDEN_API_ENFORCEMENT_POLICY,
                    &policy_request(name, factory),
                    false,
                )
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        let mut extra = policy_request(Some("fixture"), false);
        extra.write_i32(0);
        assert!(
            remote
                .transact(api::GET_HIDDEN_API_ENFORCEMENT_POLICY, &extra, false)
                .is_err()
        );
        let transient_request = |name: Option<&str>, disabled| {
            let mut p = Parcel::new();
            api::GetTransientState {
                package_name: name.map(str::to_owned),
                disabled,
            }
            .write(&mut p);
            p
        };
        for (name, disabled, present) in [
            ("fixture", false, true),
            ("fixture", true, false),
            ("missing", false, false),
        ] {
            let reply = remote
                .transact(
                    api::GET_TRANSIENT_STATE,
                    &transient_request(Some(name), disabled),
                    false,
                )
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let bytes = aim_service_aidl::read_byte_array(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            if present {
                let bytes = bytes.unwrap();
                let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
                assert_eq!(r.read_i64().unwrap(), 1);
                assert_eq!(r.read_string16().unwrap().as_deref(), Some(name));
                assert_eq!(r.read_i32().unwrap(), 10100);
                assert!(!r.read_bool().unwrap());
                for _ in 0..3 {
                    assert!(r.read_bool().unwrap());
                }
                assert_eq!(r.read_string16().unwrap().as_deref(), Some("module"));
                assert_eq!(r.remaining(), 0);
            } else {
                assert!(bytes.is_none());
            }
        }
        let reply = remote
            .transact(
                api::GET_TRANSIENT_STATE,
                &transient_request(None, false),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -3
        );
        let mut invalid = transient_request(Some("fixture"), false);
        invalid.write_i32(0);
        assert!(
            remote
                .transact(api::GET_TRANSIENT_STATE, &invalid, false)
                .is_err()
        );
        let version = remote
            .transact(api::GET_VERSION, &request(), false)
            .unwrap();
        let mut r = version.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i64().unwrap(), 1);
        let denied = foreign
            .strong(0)
            .transact(api::CLOSE, &request(), false)
            .unwrap();
        assert_eq!(
            denied.reader().read_exception().unwrap().unwrap_err().code,
            -1
        );
        assert!(old.upgrade().is_some());
        let mut wrong = Parcel::new();
        wrong.write_interface_token("different.Interface");
        assert!(remote.transact(api::GET_VERSION, &wrong, false).is_err());
        let mut trailing = request();
        trailing.write_i32(1);
        assert!(remote.transact(api::CLOSE, &trailing, false).is_err());
        assert!(remote.transact(0x7777, &request(), false).is_err());
        assert!(old.upgrade().is_some());
        for (name, disabled, present) in [
            (Some("fixture"), false, true),
            (Some("fixture"), true, false),
            (Some("missing"), false, false),
        ] {
            let mut p = request();
            p.write_string16(name);
            p.write_bool(disabled);
            let reply = remote.transact(api::GET_SIGNING_STATE, &p, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let bytes = aim_service_aidl::read_byte_array(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            assert_eq!(bytes.is_some(), present);
            if let Some(bytes) = bytes {
                let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
                assert_eq!(r.read_i64().unwrap(), 1);
                assert_eq!(r.read_string16().unwrap().as_deref(), name);
                assert_eq!(r.read_i32().unwrap(), 10100);
                assert!(!r.read_bool().unwrap());
                assert!(r.read_string16().unwrap().is_none());
                assert_eq!(r.read_i32().unwrap(), 0);
                // Saved UNKNOWN is not substituted with the collected code's signer.
                assert!(!r.read_bool().unwrap());
                assert!(!r.read_bool().unwrap());
                assert_eq!(r.remaining(), 0);
            }
        }
        let mut p = request();
        p.write_string16(None);
        p.write_bool(false);
        let reply = remote.transact(api::GET_SIGNING_STATE, &p, false).unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -3
        );
        let mut p = request();
        p.write_string16(Some("fixture"));
        p.write_bool(false);
        p.write_i32(1);
        assert!(remote.transact(api::GET_SIGNING_STATE, &p, false).is_err());
        for (name, expected) in [
            (Some("fixture"), Some("default:targetSdkVersion=30")),
            (Some("missing"), None),
        ] {
            let mut p = request();
            p.write_string16(name);
            let reply = remote.transact(api::GET_SE_INFO, &p, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let bytes = aim_service_aidl::read_byte_array(&mut r).unwrap();
            assert_eq!(r.remaining(), 0);
            if let Some(expected) = expected {
                let bytes = bytes.unwrap();
                let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
                assert_eq!(r.read_i64().unwrap(), 1);
                assert_eq!(r.read_string16().unwrap().as_deref(), name);
                assert_eq!(r.read_string16().unwrap().as_deref(), Some(expected));
                assert!(r.read_string16().unwrap().is_none());
                assert_eq!(r.remaining(), 0);
            } else {
                assert!(bytes.is_none());
            }
        }
        let mut p = request();
        p.write_string16(None);
        let reply = remote.transact(api::GET_SE_INFO, &p, false).unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -3
        );
        let mut p = request();
        p.write_string16(Some("fixture"));
        p.write_i32(1);
        assert!(remote.transact(api::GET_SE_INFO, &p, false).is_err());
        for (name, expected) in [
            (Some("fixture"), Some([17, 0, 29, 0, 0, 0, 0, 0])),
            (Some("missing"), None),
        ] {
            let mut p = request();
            p.write_string16(name);
            let reply = remote.transact(api::GET_USAGE, &p, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let bytes = aim_service_aidl::read_byte_array(&mut r).unwrap();
            if let Some(expected) = expected {
                let bytes = bytes.unwrap();
                let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
                assert_eq!(r.read_i64().unwrap(), 1);
                assert_eq!(r.read_string16().unwrap().as_deref(), name);
                assert!(r.read_bool().unwrap());
                assert_eq!(
                    aim_service_aidl::read_long_array(&mut r).unwrap().unwrap(),
                    expected
                );
                assert_eq!(r.remaining(), 0);
            } else {
                assert!(bytes.is_none());
            }
        }
        let mut p = request();
        p.write_string16(None);
        let reply = remote.transact(api::GET_USAGE, &p, false).unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -3
        );
        let mut p = request();
        p.write_string16(Some("fixture"));
        p.write_i32(123);
        assert!(remote.transact(api::GET_USAGE, &p, false).is_err());
        let mut p = request();
        p.write_bool(false);
        let names = remote.transact(api::GET_PACKAGE_NAMES, &p, false).unwrap();
        let mut r = names.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("fixture"));
        let shared = remote
            .transact(api::GET_SHARED_USER_NAMES, &request(), false)
            .unwrap();
        let mut r = shared.reader();
        r.read_exception().unwrap().unwrap();
        let count = r.read_i32().unwrap();
        assert_eq!(count, 9);
        let mut groups = Vec::new();
        for _ in 0..count {
            groups.push(r.read_string16().unwrap().unwrap());
        }
        assert_eq!(r.remaining(), 0);
        for group in &groups {
            let mut p = request();
            p.write_string16(Some(group));
            let reply = remote
                .transact(api::GET_SHARED_USER_STATE_LENGTH, &p, false)
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let length = r.read_i32().unwrap();
            assert!(length > 0);
            let mut p = request();
            p.write_string16(Some(group));
            p.write_i32(0);
            p.write_i32(length);
            let reply = remote
                .transact(api::GET_SHARED_USER_STATE_CHUNK, &p, false)
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let bytes = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
            assert_eq!(bytes.len(), length as usize);
            let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
            assert_eq!(r.read_i64().unwrap(), 1);
            assert_eq!(r.read_string16().unwrap().as_deref(), Some(group.as_str()));
        }
        for name in [None, Some("unknown.shared")] {
            let mut p = request();
            p.write_string16(name);
            let reply = remote
                .transact(api::GET_SHARED_USER_STATE_LENGTH, &p, false)
                .unwrap();
            let mut r = reply.reader();
            if name.is_none() {
                assert_eq!(r.read_exception().unwrap().unwrap_err().code, -3);
            } else {
                r.read_exception().unwrap().unwrap();
                assert_eq!(r.read_i32().unwrap(), -1);
            }
        }
        for (offset, length) in [(-1, 1), (0, 0), (0, MAX_CHUNK as i32 + 1), (i32::MAX, 1)] {
            let mut p = request();
            p.write_string16(Some(&groups[0]));
            p.write_i32(offset);
            p.write_i32(length);
            let reply = remote
                .transact(api::GET_SHARED_USER_STATE_CHUNK, &p, false)
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        let mut p = request();
        p.write_string16(Some("fixture"));
        p.write_bool(false);
        let length = remote.transact(api::GET_CODE_LENGTH, &p, false).unwrap();
        let mut r = length.reader();
        r.read_exception().unwrap().unwrap();
        let length = r.read_i32().unwrap() as usize;
        assert!(length > MAX_CHUNK * 2);
        let mut bytes = vec![];
        while bytes.len() < length {
            let mut p = request();
            p.write_string16(Some("fixture"));
            p.write_bool(false);
            p.write_i32(bytes.len() as i32);
            p.write_i32(MAX_CHUNK as i32);
            let reply = remote.transact(api::GET_CODE_CHUNK, &p, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let count = r.read_i32().unwrap() as usize;
            assert!(count > 0 && count <= MAX_CHUNK);
            let at = r.position();
            r.skip(count).unwrap();
            bytes.extend_from_slice(&r.since(at).0[..count]);
        }
        assert_eq!(bytes.len(), length);
        let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        assert_eq!(r.read_i64().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("fixture"));
        let cache = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
        let decoded = crate::package::pkg::AndroidPackage::read_cache_entry(&cache).unwrap();
        assert_eq!(decoded.uid, 10100);
        assert_eq!(decoded.version_name.unwrap().len(), 150_000);
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(
            aim_service_aidl::read_byte_array(&mut r).unwrap(),
            Some(vec![1])
        );
        assert_eq!(r.read_i32().unwrap(), 21);
        assert_eq!(
            aim_service_aidl::read_byte_array(&mut r).unwrap(),
            Some(vec![3])
        );
        assert_eq!(r.read_i32().unwrap(), 23);
        for (offset, count) in [
            (-1, 1),
            (0, 0),
            (0, MAX_CHUNK as i32 + 1),
            (length as i32 + 1, 1),
        ] {
            let mut p = request();
            p.write_string16(Some("fixture"));
            p.write_bool(false);
            p.write_i32(offset);
            p.write_i32(count);
            let reply = remote.transact(api::GET_CODE_CHUNK, &p, false).unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        for (name, factory, expected) in [
            (Some("fixture"), false, Some(vec![10])),
            (Some("fixture"), true, None),
            (Some("missing"), false, None),
        ] {
            let mut p = request();
            p.write_string16(name);
            p.write_bool(factory);
            let reply = remote.transact(api::GET_USER_STATE_IDS, &p, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(aim_service_aidl::read_int_array(&mut r).unwrap(), expected);
            assert_eq!(r.remaining(), 0);
        }
        let mut p = request();
        p.write_string16(None);
        p.write_bool(false);
        assert_eq!(
            remote
                .transact(api::GET_USER_STATE_IDS, &p, false)
                .unwrap()
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            -3
        );
        let mut p = request();
        p.write_string16(Some("fixture"));
        p.write_bool(false);
        p.write_i32(1);
        assert!(remote.transact(api::GET_USER_STATE_IDS, &p, false).is_err());

        let library_bytes = captured_library;
        let mut library_request = request();
        library_request.write_string16(Some("fixture"));
        let library_reply = remote
            .transact(api::GET_LIBRARY_STATE_LENGTH, &library_request, false)
            .unwrap();
        let mut library_reader = library_reply.reader();
        library_reader.read_exception().unwrap().unwrap();
        assert_eq!(
            library_reader.read_i32().unwrap(),
            library_bytes.len() as i32
        );
        let mut received = Vec::new();
        while received.len() < library_bytes.len() {
            let mut p = request();
            p.write_string16(Some("fixture"));
            p.write_i32(received.len() as i32);
            p.write_i32(7);
            let reply = remote
                .transact(api::GET_LIBRARY_STATE_CHUNK, &p, false)
                .unwrap();
            let mut reader = reply.reader();
            reader.read_exception().unwrap().unwrap();
            received.extend(
                aim_service_aidl::read_byte_array(&mut reader)
                    .unwrap()
                    .unwrap(),
            );
            assert_eq!(reader.remaining(), 0);
        }
        assert_eq!(received, library_bytes);
        for (offset, length) in [
            (-1, 7),
            (0, 0),
            (0, MAX_CHUNK as i32 + 1),
            (library_bytes.len() as i32 + 1, 7),
        ] {
            let mut p = request();
            p.write_string16(Some("fixture"));
            p.write_i32(offset);
            p.write_i32(length);
            let reply = remote
                .transact(api::GET_LIBRARY_STATE_CHUNK, &p, false)
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        for name in [None, Some("missing")] {
            let mut p = request();
            p.write_string16(name);
            let reply = remote
                .transact(api::GET_LIBRARY_STATE_LENGTH, &p, false)
                .unwrap();
            let mut reader = reply.reader();
            if name.is_none() {
                assert_eq!(reader.read_exception().unwrap().unwrap_err().code, -3);
            } else {
                reader.read_exception().unwrap().unwrap();
                assert_eq!(reader.read_i32().unwrap(), -1);
            }
        }
        let mut p = library_request.clone();
        p.write_i32(1);
        assert!(
            remote
                .transact(api::GET_LIBRARY_STATE_LENGTH, &p, false)
                .is_err()
        );
        let setting_request = |name: Option<&str>, factory: bool| {
            let mut p = request();
            p.write_string16(name);
            p.write_bool(factory);
            p
        };
        let reply = remote
            .transact(
                api::GET_SETTING_LENGTH,
                &setting_request(Some("fixture"), false),
                false,
            )
            .unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap() as usize, captured_setting.len());
        let mut setting_bytes = Vec::new();
        while setting_bytes.len() < captured_setting.len() {
            let mut p = setting_request(Some("fixture"), false);
            p.write_i32(setting_bytes.len() as i32);
            p.write_i32(MAX_CHUNK as i32);
            let reply = remote.transact(api::GET_SETTING_CHUNK, &p, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let chunk = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= MAX_CHUNK);
            assert_eq!(r.remaining(), 0);
            setting_bytes.extend(chunk);
        }
        assert_eq!(setting_bytes, captured_setting);
        assert_eq!(
            &setting_bytes[setting_bytes.len() - 4..],
            &1_i32.to_le_bytes()
        );
        let current_setting =
            crate::package::scan_snapshot::setting_record::captured(&current, "fixture", false)
                .unwrap()
                .unwrap();
        assert_eq!(
            &current_setting[current_setting.len() - 4..],
            &0_i32.to_le_bytes()
        );
        assert_ne!(
            setting_bytes,
            crate::package::scan_snapshot::setting_record::captured(&current, "fixture", false)
                .unwrap()
                .unwrap()
        );
        for (name, factory) in [(Some("absent"), false), (Some("fixture"), true)] {
            let reply = remote
                .transact(
                    api::GET_SETTING_LENGTH,
                    &setting_request(name, factory),
                    false,
                )
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), -1);
        }
        let reply = remote
            .transact(
                api::GET_SETTING_LENGTH,
                &setting_request(None, false),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -3
        );
        for (offset, count) in [
            (-1, 1),
            (0, 0),
            (0, MAX_CHUNK as i32 + 1),
            (captured_setting.len() as i32 + 1, 1),
        ] {
            let mut p = setting_request(Some("fixture"), false);
            p.write_i32(offset);
            p.write_i32(count);
            let reply = remote.transact(api::GET_SETTING_CHUNK, &p, false).unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        let mut p = setting_request(Some("fixture"), false);
        p.write_i32(1);
        assert!(remote.transact(api::GET_SETTING_LENGTH, &p, false).is_err());
        let user_request = |name: Option<&str>, factory: bool, user: i32| {
            let mut p = request();
            p.write_string16(name);
            p.write_bool(factory);
            p.write_i32(user);
            p
        };
        let reply = remote
            .transact(
                api::GET_USER_STATE_LENGTH,
                &user_request(Some("fixture"), false, 10),
                false,
            )
            .unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap() as usize, captured_user.len());
        let mut user_bytes = Vec::new();
        while user_bytes.len() < captured_user.len() {
            let mut p = user_request(Some("fixture"), false, 10);
            p.write_i32(user_bytes.len() as i32);
            p.write_i32(MAX_CHUNK as i32);
            let reply = remote
                .transact(api::GET_USER_STATE_CHUNK, &p, false)
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            let chunk = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
            assert!(!chunk.is_empty() && chunk.len() <= MAX_CHUNK);
            assert_eq!(r.remaining(), 0);
            user_bytes.extend(chunk);
        }
        assert_eq!(user_bytes, captured_user);
        let mut p = user_request(Some("fixture"), false, 0);
        p.write_i32(0);
        p.write_i32(MAX_CHUNK as i32);
        let reply = remote
            .transact(api::GET_USER_STATE_CHUNK, &p, false)
            .unwrap();
        let mut r = reply.reader();
        r.read_exception().unwrap().unwrap();
        let default_bytes = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
        let mut r = aim_binder_host::parcel::Reader::new(&default_bytes, &[]);
        assert_eq!(r.read_i64().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("fixture"));
        assert_eq!(r.read_i32().unwrap(), 10100);
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i64().unwrap(), 0);
        assert_eq!(r.read_i64().unwrap(), 0);
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        for (name, factory) in [("absent", false), ("fixture", true)] {
            let reply = remote
                .transact(
                    api::GET_USER_STATE_LENGTH,
                    &user_request(Some(name), factory, 10),
                    false,
                )
                .unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), -1);
        }
        for (name, user) in [(None, 10), (Some("fixture"), -1)] {
            let reply = remote
                .transact(
                    api::GET_USER_STATE_LENGTH,
                    &user_request(name, false, user),
                    false,
                )
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        for (offset, count) in [
            (-1, 1),
            (0, 0),
            (0, MAX_CHUNK as i32 + 1),
            (captured_user.len() as i32 + 1, 1),
        ] {
            let mut p = user_request(Some("fixture"), false, 10);
            p.write_i32(offset);
            p.write_i32(count);
            let reply = remote
                .transact(api::GET_USER_STATE_CHUNK, &p, false)
                .unwrap();
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                -3
            );
        }
        let mut p = user_request(Some("fixture"), false, 10);
        p.write_i32(123);
        assert!(
            remote
                .transact(api::GET_USER_STATE_LENGTH, &p, false)
                .is_err()
        );
        for _ in 0..2 {
            let reply = remote.transact(api::CLOSE, &request(), false).unwrap();
            reply.reader().read_exception().unwrap().unwrap();
        }
        assert!(old.upgrade().is_none());
        let reply = remote
            .transact(
                api::GET_HIDDEN_API_ENFORCEMENT_POLICY,
                &policy_request(Some("fixture"), false),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -5
        );

        let reply = remote
            .transact(
                api::GET_TRANSIENT_STATE,
                &transient_request(Some("fixture"), false),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -5
        );

        let reply = remote
            .transact(
                api::GET_SETTING_LENGTH,
                &setting_request(Some("fixture"), false),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -5
        );

        let reply = remote
            .transact(
                api::GET_USER_STATE_LENGTH,
                &user_request(Some("fixture"), false, 10),
                false,
            )
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -5
        );
        let reply = remote
            .transact(api::GET_VERSION, &request(), false)
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            -5
        );
    }
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

    #[test]
    fn loaded_code_withdrawal_keeps_saved_and_disabled_owners_and_old_snapshots() {
        use super::*;
        let package = crate::package::settings::Package {
            name: "fixture".into(),
            code_path: "/data/app/fixture".into(),
            app_id: 10100,
            ..Default::default()
        };
        let settings = Settings {
            packages: vec![package.clone()],
            ..Default::default()
        };
        let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
        let signing = SigningDetails {
            signatures: vec![vec![3]],
            scheme_version: 3,
            public_keys: vec![],
            past_signing_certificates: Some(vec![(vec![1], 21), (vec![3], 23)]),
        };
        let parsed = crate::package::pkg::AndroidPackage {
            feature_flag_state: Some(Vec::new()),
            package_name: package.name.clone(),
            path: Some(package.code_path.clone()),
            signing_details: Some(signing.parcel_details().unwrap()),
            ..Default::default()
        };
        let loaded =
            Arc::new(super::super::LoadedPackage::new(parsed.clone(), signing.clone()).unwrap());
        owner.loaded.insert(package.name.clone(), loaded.clone());
        owner.disabled_loaded.insert(package.name.clone(), loaded);
        owner
            .scanned_users
            .insert(package.name.clone(), BTreeMap::new());
        let frozen = owner.clone();
        let record = Record {
            settings: package.clone(),
            parsed,
            signing,
            identity: Identity {
                manifest_name: package.name.clone(),
                internal_name: package.name.clone(),
                real_name: None,
            },
            origin: ScanOrigin::SystemDirectory,
        };
        assert_eq!(
            frozen.loaded_packages()[&package.name]
                .facade_entry()
                .unwrap()
                .past_signing_certificates,
            record.signing.past_signing_certificates
        );
        assert!(owner.remove_package_setting(&package.name).is_err());
        assert_eq!(owner, frozen);
        owner.withdraw_scanned_package(&record);
        assert!(!owner.has_scanned_package(&package.name));
        assert!(owner.loaded_packages().is_empty());
        assert!(owner.scanned_user_states(&package.name).is_none());
        assert_eq!(owner.settings, frozen.settings);
        assert_eq!(owner.identities, frozen.identities);
        assert_eq!(
            owner.disabled_loaded_packages(),
            frozen.disabled_loaded_packages()
        );
        assert_eq!(
            frozen.loaded_packages()[&package.name].package,
            record.parsed
        );
        owner
            .remove_package_setting(&package.name)
            .unwrap()
            .unwrap();
        assert_eq!(
            frozen.loaded_packages()[&package.name].package,
            record.parsed
        );
    }
}
