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
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct DisabledUserStates {
    users: BTreeMap<i32, UserState>,
    aliases: BTreeSet<i32>,
}

impl DisabledUserStates {
    fn copied(users: &BTreeMap<i32, UserState>) -> Self {
        Self {
            users: users.clone(),
            aliases: users.keys().copied().collect(),
        }
    }
    fn update_active(&mut self, users: &BTreeMap<i32, UserState>) {
        self.aliases.retain(|id| {
            if let Some(user) = users.get(id) {
                self.users.insert(*id, user.clone());
                true
            } else {
                false
            }
        });
    }
}

/// Explicit sparse owners and original active-user aliases at the import boundary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CapturedUsers {
    pub states: BTreeMap<i32, UserState>,
    pub active_aliases: BTreeSet<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OriginalUserScope {
    pub name: String,
    pub app_id: i32,
    pub path: String,
    pub version: i64,
    pub factory: bool,
    pub users: BTreeSet<i32>,
    pub active_aliases: BTreeSet<i32>,
}
impl OriginalUserScope {
    pub fn read_original_record(bytes: &[u8]) -> aim_binder_host::parcel::Result<Self> {
        use aim_binder_host::parcel::{BAD_VALUE, Reader};
        let mut r = Reader::new(bytes, &[]);
        let name = r.read_string16()?.ok_or(BAD_VALUE)?;
        let app_id = r.read_i32()?;
        let path = r.read_string16()?.ok_or(BAD_VALUE)?;
        let version = r.read_i64()?;
        let boolean = |r: &mut Reader<'_>| match r.read_i32()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(BAD_VALUE),
        };
        let factory = boolean(&mut r)?;
        let count = r.read_i32()?;
        if count < 0 || count as usize > r.remaining() / 8 {
            return Err(BAD_VALUE);
        }
        let mut users = BTreeSet::new();
        let mut active_aliases = BTreeSet::new();
        let mut previous = None;
        for _ in 0..count {
            let id = r.read_i32()?;
            let alias = boolean(&mut r)?;
            if id < 0 || previous.is_some_and(|p| p >= id) || alias && !factory {
                return Err(BAD_VALUE);
            }
            users.insert(id);
            if alias {
                active_aliases.insert(id);
            }
            previous = Some(id);
        }
        if r.remaining() != 0 || bytes.len() % 4 != 0 {
            return Err(BAD_VALUE);
        }
        Ok(Self {
            name,
            app_id,
            path,
            version,
            factory,
            users,
            active_aliases,
        })
    }
}

impl SigningScan {
    /// Bind original alias provenance to already captured complete native user
    /// values. Query projections never substitute for those full owners.
    pub fn capture_original_user_aliases(
        &mut self,
        source: &crate::package::model::State,
    ) -> Result<(), String> {
        let mut values = BTreeMap::new();
        for (settings, factory) in [
            (&self.settings.packages, false),
            (&self.settings.disabled_system_packages, true),
        ] {
            for setting in settings {
                let input = source
                    .user_scopes
                    .get(&(setting.name.clone(), factory))
                    .ok_or("missing original user scope")?;
                let states = if factory {
                    self.disabled_user_states(&setting.name)
                } else {
                    self.scanned_user_states(&setting.name)
                }
                .ok_or("complete native user states are not captured")?;
                if input.name != setting.name
                    || input.app_id != setting.app_id
                    || input.path != setting.code_path
                    || input.version != setting.version_code
                    || input.factory != factory
                    || states.keys().copied().collect::<BTreeSet<_>>() != input.users
                {
                    return Err(format!("original user scope differs: {}", setting.name));
                }
                values.insert(
                    (setting.name.clone(), factory),
                    CapturedUsers {
                        states: states.clone(),
                        active_aliases: input.active_aliases.clone(),
                    },
                );
            }
        }
        if values.keys().ne(source.user_scopes.keys()) {
            return Err("original user scope inventory differs".into());
        }
        self.capture_user_states(values)
    }

    pub fn capture_user_states(
        &mut self,
        values: BTreeMap<(String, bool), CapturedUsers>,
    ) -> Result<(), String> {
        let expected: BTreeSet<_> = self
            .settings
            .packages
            .iter()
            .map(|p| (p.name.clone(), false))
            .chain(
                self.settings
                    .disabled_system_packages
                    .iter()
                    .map(|p| (p.name.clone(), true)),
            )
            .collect();
        if values.keys().cloned().collect::<BTreeSet<_>>() != expected {
            return Err("captured user inventory differs".into());
        }
        for ((name, factory), users) in &values {
            if users.states.keys().any(|id| *id < 0)
                || !*factory && !users.active_aliases.is_empty()
            {
                return Err("invalid captured user identity/alias scope".into());
            }
            for id in &users.active_aliases {
                let active = values
                    .get(&(name.clone(), false))
                    .and_then(|u| u.states.get(id))
                    .ok_or("captured user alias has no active owner")?;
                if users.states.get(id) != Some(active) {
                    return Err("captured user alias differs from active owner".into());
                }
            }
        }
        let mut active = BTreeMap::new();
        let mut disabled = BTreeMap::new();
        for ((name, factory), users) in values {
            if factory {
                disabled.insert(
                    name,
                    DisabledUserStates {
                        users: users.states,
                        aliases: users.active_aliases,
                    },
                );
            } else {
                active.insert(name, users.states);
            }
        }
        self.scanned_users = active;
        self.disabled_users = disabled;
        Ok(())
    }
}

/// Factory metadata is not an active signer/library admission. Its verified
/// code is retained separately for the later updated-data signature gate.
#[derive(Debug)]
pub struct DisabledSystemMetadata {
    pub record: Record,
    pub users: BTreeMap<i32, UserState>,
    pub multi_arch_mismatch: bool,
    pub alignment_diagnostic: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdatedSystemSource {
    /// Continue with the installed data package after refreshing factory state.
    KeepData,
    /// Resource cleanup and enabling the factory setting must precede its
    /// active scan. This outcome has not replaced the active package.
    RestoreFactory,
}

#[derive(Debug)]
pub struct UpdatedSystemScan {
    pub(super) active: crate::package::settings::Package,
    pub factory: DisabledSystemMetadata,
    pub source: UpdatedSystemSource,
}

fn updated_system_source(
    same_path: bool,
    image_version: i64,
    data_version: i64,
    shared_uid_changed: bool,
) -> UpdatedSystemSource {
    if !same_path && (image_version > data_version || shared_uid_changed) {
        UpdatedSystemSource::RestoreFactory
    } else {
        UpdatedSystemSource::KeepData
    }
}

impl SigningScan {
    /// scanPackageForInitLI drops a stale disabled setting before attempting
    /// the fresh system scan. The removal survives subsequent scan rejection;
    /// shared UID allocation survives until the normal pruning phase.
    pub fn remove_stale_disabled_system(&mut self, code: &Code) -> Result<bool, SigningError> {
        let flags = physical_parse_flags(&code.location.path).map_err(|message| {
            SigningError::Rejected(Error {
                package: code.parsed.package_name.clone(),
                path: code.location.path.clone(),
                phase: "location",
                message,
            })
        })?;
        if flags & parse::PARSE_IS_SYSTEM_DIR == 0 {
            return Err(SigningError::Rejected(Error {
                package: code.parsed.package_name.clone(),
                path: code.location.path.clone(),
                phase: "location",
                message: "stale factory recovery requires system code".into(),
            }));
        }
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
        if self
            .settings
            .packages
            .iter()
            .any(|p| p.name == identity.internal_name)
            || Identity::original_setting(&code.parsed, &self.settings, &|name| {
                self.has_scanned_package(name)
            })
            .is_some()
        {
            return Ok(false);
        }
        let count = self.settings.disabled_system_packages.len();
        self.settings
            .disabled_system_packages
            .retain(|p| p.name != identity.internal_name);
        let removed = count != self.settings.disabled_system_packages.len();
        if removed {
            self.disabled_users.remove(&identity.internal_name);
            self.disabled_loaded.remove(&identity.internal_name);
        }
        Ok(removed)
    }

    /// Apply a selected factory restoration only after the real installer and
    /// cache cleanup succeeds. A partial resource failure preserves settings;
    /// the resource owner retains pending directory cleanup for retry (#816).
    pub fn restore_updated_system_setting(
        &mut self,
        selected: &UpdatedSystemScan,
        resources: &crate::package::owner::resources::CodeResources,
        incremental: bool,
        domain_set_id: [u8; 16],
    ) -> Result<crate::package::settings::Package, SigningError> {
        self.restore_updated_system_setting_with_id(selected, resources, incremental, &|| {
            Ok(domain_set_id)
        })
    }

    pub(super) fn restore_updated_system_setting_with_id(
        &mut self,
        selected: &UpdatedSystemScan,
        resources: &crate::package::owner::resources::CodeResources,
        incremental: bool,
        new_domain_id: &dyn Fn() -> Result<[u8; 16], String>,
    ) -> Result<crate::package::settings::Package, SigningError> {
        let factory = &selected.factory.record.settings;
        let reject = |message: String| {
            SigningError::Rejected(Error {
                package: factory.name.clone(),
                path: factory.code_path.clone(),
                phase: "factory-restoration",
                message,
            })
        };
        if selected.source != UpdatedSystemSource::RestoreFactory
            || !self
                .settings
                .disabled_system_packages
                .iter()
                .any(|p| p == factory)
        {
            return Err(reject(
                "factory restoration selection is stale or retains data".into(),
            ));
        }
        let active = self
            .settings
            .packages
            .iter()
            .find(|p| p.name == factory.name)
            .ok_or_else(|| reject("factory restoration has no active data setting".into()))?;
        if active != &selected.active {
            return Err(reject(
                "active data setting changed after source selection".into(),
            ));
        }
        resources
            .clean(&active.code_path, incremental)
            .map_err(reject)?;
        let domain_set_id = new_domain_id().map_err(reject)?;
        self.enable_system_setting(&factory.name, domain_set_id)
            .ok_or_else(|| reject("factory setting could not reserve its original UID".into()))
    }

    /// Settings.enableSystemPackageLPw. The caller must first clean the old
    /// code resources, then scan the factory code against this active setting
    /// (#816/#702). This transition alone does not publish factory code.
    /// Domain verification supplies a fresh ID even when the active setting
    /// is reused. Original duplicate-UID failure still removes the disabled
    /// setting; it does not replace the first active package or its users.
    pub fn enable_system_setting(
        &mut self,
        name: &str,
        domain_set_id: [u8; 16],
    ) -> Option<crate::package::settings::Package> {
        use crate::package::{owner::app_ids::Owner, settings};
        let at = self
            .settings
            .disabled_system_packages
            .iter()
            .position(|p| p.name == name)?;
        let factory = self.settings.disabled_system_packages.remove(at);
        self.disabled_users.remove(name);
        self.disabled_loaded.remove(name);
        let active = self.settings.packages.iter().position(|p| p.name == name);
        let at = match active {
            Some(at) if self.settings.packages[at].app_id == factory.app_id => at,
            Some(_) => return None,
            None => {
                // addPackageLPw constructs a package slot even for a disabled
                // shared-UID setting; a reserved shared slot rejects it.
                self.identities
                    .ids
                    .register_existing(factory.app_id, Owner::Package(name.into()))
                    .ok()?;
                self.settings.packages.push(settings::Package {
                    name: name.into(),
                    real_name: factory.real_name.clone(),
                    code_path: factory.code_path.clone(),
                    app_id: factory.app_id,
                    flags: factory.flags,
                    private_flags: factory.private_flags,
                    domain_set_id: Some(super::setting::domain_id(domain_set_id)),
                    category_hint: -1,
                    key_set_data: settings::KeySetData {
                        proper_signing_key_set: -1,
                        ..Default::default()
                    },
                    ..Default::default()
                });
                self.scanned_users.insert(name.into(), BTreeMap::new());
                self.settings.packages.len() - 1
            }
        };
        let p = &mut self.settings.packages[at];
        p.legacy_native_library_path = factory.legacy_native_library_path;
        p.primary_cpu_abi = factory.primary_cpu_abi;
        p.secondary_cpu_abi = factory.secondary_cpu_abi;
        p.cpu_abi_override = factory.cpu_abi_override;
        p.version_code = factory.version_code;
        p.uses_sdk_libraries = factory.uses_sdk_libraries;
        p.uses_static_libraries = factory.uses_static_libraries;
        p.mime_groups = factory.mime_groups;
        p.app_metadata_file_path = factory.app_metadata_file_path;
        p.app_metadata_source = factory.app_metadata_source;
        p.target_sdk_version = factory.target_sdk_version;
        p.restrict_update_hash = factory.restrict_update_hash;
        p.scanned_as_stopped_system_app = factory.scanned_as_stopped_system_app;
        p.transient.updated_system_app = false;
        p.install_source = factory.install_source;
        Some(p.clone())
    }

    pub fn scanned_user_states(&self, name: &str) -> Option<&BTreeMap<i32, UserState>> {
        self.scanned_users.get(name)
    }
    pub fn disabled_user_states(&self, name: &str) -> Option<&BTreeMap<i32, UserState>> {
        self.disabled_users.get(name).map(|state| &state.users)
    }

    /// Settings.disableSystemPackageLPw(replaced=true): copy the loaded factory
    /// before marking the active setting updated. Disabled shared membership is
    /// retained by disabled_system_packages, without changing active UID flags.
    pub fn disable_system_package(&mut self, name: &str) -> Result<bool, SigningError> {
        let Some(at) = self.settings.packages.iter().position(|p| p.name == name) else {
            return Ok(false);
        };
        let package = &self.settings.packages[at];
        if self
            .settings
            .disabled_system_packages
            .iter()
            .any(|p| p.name == name)
            || !self.loaded.contains_key(name)
            || package.flags & crate::package::settings::FLAG_SYSTEM == 0
            || package.transient.updated_system_app
        {
            return Ok(false);
        }
        let reject = |message: &str| {
            SigningError::Rejected(Error {
                package: name.into(),
                path: package.code_path.clone(),
                phase: "disable-system",
                message: message.into(),
            })
        };
        if self.pending_metadata.contains(name) {
            return Err(reject("factory scan metadata is not finalized"));
        }
        let users = self
            .scanned_users
            .get(name)
            .ok_or_else(|| reject("loaded factory user owner is missing"))?;
        if package.shared_user {
            match self.identities.ids.get(package.uid_owner_id()) {
                Some(crate::package::owner::app_ids::Owner::SharedUser(group))
                    if self
                        .identities
                        .shared_users
                        .get(group)
                        .is_some_and(|owner| owner.has_package(name)) => {}
                _ => return Err(reject("loaded factory shared UID owner is missing")),
            }
        }
        self.validate_legacy_permissions()
            .map_err(|message| reject(&message))?;
        let disabled = package.clone();
        let users = DisabledUserStates::copied(users);
        let loaded = self.loaded[name].clone();
        self.copy_disabled_legacy(name);
        self.settings.disabled_system_packages.push(disabled);
        self.disabled_users.insert(name.into(), users);
        self.disabled_loaded.insert(name.into(), loaded);
        self.settings.packages[at].transient.updated_system_app = true;
        Ok(true)
    }

    /// disableSystemPackageLPw(replaced=true) retains aliases to the accepted
    /// factory's existing users. Settings/updated-system transition and
    /// resource effects belong to the surrounding disable owner (#702).
    pub fn copy_disabled_user_states(
        &mut self,
        completed: &super::CompletedScanMetadata,
    ) -> Result<(), SigningError> {
        let candidate = &completed.candidate;
        self.accepted_slot(&candidate.record, "disabled-users")?;
        let package = &candidate.record.settings;
        if self.scanned_users.get(&package.name) != Some(&candidate.users)
            || self.loaded.get(&package.name).is_none_or(|pkg| {
                pkg.package != candidate.record.parsed
                    || pkg.collected_signing != candidate.record.signing
            })
            || candidate.record.origin != ScanOrigin::SystemDirectory
            || !self
                .settings
                .disabled_system_packages
                .iter()
                .any(|p| p == package)
        {
            return Err(SigningError::Rejected(Error {
                package: package.name.clone(),
                path: package.code_path.clone(),
                phase: "disabled-users",
                message: "disabled copy does not match the accepted factory setting".into(),
            }));
        }
        self.disabled_users.insert(
            package.name.clone(),
            DisabledUserStates::copied(&candidate.users),
        );
        self.disabled_loaded
            .insert(package.name.clone(), self.loaded[&package.name].clone());
        Ok(())
    }

    pub(super) fn detach_retained_user_aliases(&mut self, name: &str) {
        self.identities.ids.detach_user_aliases(name);
        for group in self.identities.shared_users.values_mut() {
            group.detach_user_aliases(name);
        }
    }

    pub(super) fn detach_disabled_user_aliases(&mut self, name: &str) {
        self.detach_retained_user_aliases(name);
        if let Some(state) = self.disabled_users.get_mut(name) {
            state.aliases.clear();
        }
    }

    pub(super) fn update_disabled_user_aliases(
        &mut self,
        name: &str,
        users: &BTreeMap<i32, UserState>,
    ) {
        self.identities.ids.update_user_aliases(name, users);
        for group in self.identities.shared_users.values_mut() {
            group.update_user_aliases(name, users);
        }
        self.scanned_users.insert(name.into(), users.clone());
        if let Some(state) = self.disabled_users.get_mut(name) {
            state.update_active(users);
        }
    }

    /// Factory refresh followed by scanPackageForInitLI's updated-system
    /// source decision. Only factory metadata is committed here; a restore
    /// outcome requires resource cleanup, enableSystemPackage and active scan.
    pub fn scan_updated_system(
        &mut self,
        code: &Code,
        update: SettingUpdate,
        all_users: Option<&[super::User]>,
        config: &crate::package::system_config::SystemConfig,
        apks: &Apks,
        inputs: ScanMetadataCompletion<'_>,
    ) -> Result<UpdatedSystemScan, SigningError> {
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
        let reject = |message: String| {
            SigningError::Rejected(Error {
                package: identity.internal_name.clone(),
                path: code.location.path.clone(),
                phase: "system-source",
                message,
            })
        };
        let active = self
            .settings
            .packages
            .iter()
            .find(|p| p.name == identity.internal_name)
            .ok_or_else(|| reject("updated-system selection has no active setting".into()))?;
        let group = if active.shared_user {
            Some(
                self.settings
                    .shared_users
                    .iter()
                    .find(|g| Some(g.app_id) == active.shared_app_id())
                    .ok_or_else(|| reject("active shared UID owner is missing".into()))?
                    .name
                    .as_str(),
            )
        } else {
            None
        };
        let selected = super::signing::selected_shared_user(
            active.shared_user,
            code.parsed.shared_user_id.as_deref(),
            code.parsed
                .is(crate::package::pkg::booleans::LEAVING_SHARED_UID),
        );
        let version = (i64::from(code.parsed.version_code_major) << 32)
            | i64::from(code.parsed.version_code as u32);
        let source = updated_system_source(
            active.code_path == code.location.path,
            version,
            active.version_code,
            group != selected,
        );
        let active = active.clone();
        let mut staged = self.clone();
        let mut factory = staged.scan_disabled_system(code, update, all_users, apks, inputs)?;
        if source == UpdatedSystemSource::KeepData
            && config
                .preinstall_packages_with_strict_signature_check
                .contains(&factory.record.parsed.package_name)
        {
            let signatures = crate::package::owner::shared_users::saved_signatures(&code.signing)
                .map_err(reject)?;
            factory.record.settings.signatures = Some(signatures);
            let saved = staged
                .settings
                .disabled_system_packages
                .iter_mut()
                .find(|p| p.name == factory.record.settings.name)
                .expect("factory scan retained its disabled setting");
            saved
                .signatures
                .clone_from(&factory.record.settings.signatures);
        }
        *self = staged;
        Ok(UpdatedSystemScan {
            active,
            factory,
            source,
        })
    }

    /// Refresh a disabled factory setting without registering its libraries,
    /// replacing live signatures or adding it to a shared UID's active members.
    /// Manifest policy must already have run with updated-system policy enabled.
    /// User states belong to the disabled setting; restored and live copied
    /// factory settings have different user-state provenance (#815).
    pub fn scan_disabled_system(
        &mut self,
        code: &Code,
        update: SettingUpdate,
        all_users: Option<&[super::User]>,
        apks: &Apks,
        inputs: ScanMetadataCompletion<'_>,
    ) -> Result<DisabledSystemMetadata, SigningError> {
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
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
        let updated = saved.transient.updated_system_app;
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
                    .find(|g| Some(g.app_id) == saved.shared_app_id())
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
        let users = self.disabled_users.get(&saved.name).ok_or_else(|| {
            reject(
                "setting",
                "disabled package user states were not supplied".into(),
            )
        })?;
        let mut setting = NewSetting::update(saved, &users.users, update, all_users, false);
        let mut parsed = code
            .collected_package()
            .map_err(|e| reject("signatures", e))?;
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
        let loaded = super::LoadedPackage::new(parsed.clone(), code.signing.clone())
            .map_err(|message| reject("package-finalization", message))?;
        // scanPackageOnly preserves saved signatures. Strict recollection for
        // selected updated-system packages belongs to the version selector.
        self.settings.disabled_system_packages[at] = setting.package.clone();
        self.disabled_loaded
            .insert(setting.package.name.clone(), std::sync::Arc::new(loaded));
        self.disabled_users
            .get_mut(&setting.package.name)
            .expect("disabled user setting was validated")
            .users
            .clone_from(&setting.users);
        let state = &self.disabled_users[&setting.package.name];
        if let Some(active) = self.scanned_users.get_mut(&setting.package.name) {
            for id in &state.aliases {
                if let (Some(current), Some(user)) = (active.get_mut(id), state.users.get(id)) {
                    current.clone_from(user);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_factory_requires_complete_owners_and_preserves_active_uid_membership() {
        use crate::package::{
            pkg::AndroidPackage,
            settings::{Package, Settings, SharedUser},
            sign::SigningDetails,
        };
        for shared in [false, true] {
            let package = Package {
                name: "factory".into(),
                code_path: "/system/apex/factory.apex".into(),
                app_id: 10000,
                shared_user: shared,
                flags: 1,
                ..Default::default()
            };
            let settings = Settings {
                packages: vec![package.clone()],
                shared_users: if shared {
                    vec![SharedUser {
                        name: "factory.uid".into(),
                        app_id: 10000,
                        flags: 1,
                        signatures: None,
                    }]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let mut owner = SigningScan::new(&Default::default(), &settings, 36).unwrap();
            assert!(!owner.disable_system_package("factory").unwrap());
            let signing = SigningDetails {
                current_flags: Vec::new(),
                signatures: vec![vec![3]],
                scheme_version: 3,
                public_keys: Vec::new(),
                past_signing_certificates: None,
            };
            let parsed = AndroidPackage {
                package_name: "factory".into(),
                feature_flag_state: Some(Vec::new()),
                signing_details: Some(signing.parcel_details().unwrap()),
                ..Default::default()
            };
            owner.loaded.insert(
                "factory".into(),
                std::sync::Arc::new(super::super::LoadedPackage::new(parsed, signing).unwrap()),
            );
            let before = owner.clone();
            assert!(owner.disable_system_package("factory").is_err());
            assert_eq!(owner, before);
            owner
                .scanned_users
                .insert("factory".into(), Default::default());
            owner.pending_metadata.insert("factory".into());
            let before = owner.clone();
            assert!(owner.disable_system_package("factory").is_err());
            assert_eq!(owner, before);
            owner.pending_metadata.clear();
            if shared {
                let mut broken = owner.clone();
                broken.identities.remove_shared_user("factory.uid");
                let before = broken.clone();
                assert!(broken.disable_system_package("factory").is_err());
                assert_eq!(broken, before);
            }
            for updated in [false, true] {
                let mut ineligible = owner.clone();
                if updated {
                    ineligible.settings.packages[0].transient.updated_system_app = true;
                } else {
                    ineligible.settings.packages[0].flags &= !1;
                }
                let before = ineligible.clone();
                assert!(!ineligible.disable_system_package("factory").unwrap());
                assert_eq!(ineligible, before);
            }
            let identities = owner.identities.clone();
            assert!(owner.disable_system_package("factory").unwrap());
            assert_eq!(owner.settings.disabled_system_packages, vec![package]);
            assert!(owner.settings.packages[0].transient.updated_system_app);
            assert_eq!(owner.identities, identities);
            let before = owner.clone();
            assert!(!owner.disable_system_package("factory").unwrap());
            assert_eq!(owner, before);
        }
    }
    #[test]
    fn original_scope_aliases_bind_complete_owned_values_and_reject_foreign_inputs() {
        use crate::package::settings::{Package, Settings};
        let active = Package {
            name: "p".into(),
            app_id: 10123,
            code_path: "/data/p".into(),
            version_code: 7,
            ..Default::default()
        };
        let factory = Package {
            code_path: "/system/p".into(),
            ..active.clone()
        };
        let mut owner = SigningScan::new(
            &Default::default(),
            &Settings {
                packages: vec![active],
                disabled_system_packages: vec![factory],
                ..Default::default()
            },
            36,
        )
        .unwrap();
        let read = |status| {
            crate::package::restrictions::Restrictions::parse(
                &aim_android_xml::read(format!(
                    "<package-restrictions><pkg name='p' domainVerificationStatus='{status}'/></package-restrictions>"
                ).as_bytes()).unwrap()
            ).unwrap()
        };
        let active_restrictions = read(2);
        let factory_restrictions = read(3);
        let original_user = active_restrictions.packages[0].1.clone();
        assert_eq!(original_user, factory_restrictions.packages[0].1);
        let mut states = BTreeMap::new();
        states.insert(
            ("p".into(), false),
            CapturedUsers {
                states: BTreeMap::from([(0, original_user.clone())]),
                active_aliases: BTreeSet::new(),
            },
        );
        states.insert(
            ("p".into(), true),
            CapturedUsers {
                states: BTreeMap::from([
                    (0, factory_restrictions.packages[0].1.clone()),
                    (10, UserState::default()),
                ]),
                active_aliases: BTreeSet::new(),
            },
        );
        owner.capture_user_states(states).unwrap();
        let mut source = crate::package::model::State::default();
        for factory in [false, true] {
            source.user_scopes.insert(
                ("p".into(), factory),
                OriginalUserScope {
                    name: "p".into(),
                    app_id: 10123,
                    path: if factory {
                        "/system/p".into()
                    } else {
                        "/data/p".into()
                    },
                    version: 7,
                    factory,
                    users: if factory {
                        BTreeSet::from([0, 10])
                    } else {
                        BTreeSet::from([0])
                    },
                    active_aliases: if factory {
                        BTreeSet::from([0])
                    } else {
                        BTreeSet::new()
                    },
                },
            );
        }
        owner.capture_original_user_aliases(&source).unwrap();
        let retained = owner.clone();
        for mode in 0..6 {
            let mut bad = source.clone();
            let scope = bad.user_scopes.get_mut(&("p".into(), true)).unwrap();
            match mode {
                0 => scope.app_id += 1,
                1 => scope.path = "other".into(),
                2 => scope.version += 1,
                3 => {
                    scope.users.remove(&10);
                }
                4 => {
                    scope.active_aliases.insert(10);
                }
                _ => {
                    bad.user_scopes.remove(&("p".into(), false));
                }
            }
            assert!(owner.capture_original_user_aliases(&bad).is_err());
            assert_eq!(owner, retained);
        }
        let mut updated = UserState::default();
        updated.enabled = 2;
        owner.set_user_state("p", 0, updated.clone()).unwrap();
        assert_eq!(owner.disabled_user_states("p").unwrap()[&0], updated);
        assert_eq!(
            owner.disabled_user_states("p").unwrap()[&10],
            UserState::default()
        );
        assert_eq!(
            retained.disabled_user_states("p").unwrap()[&0],
            original_user
        );
        assert_eq!(active_restrictions.legacy_domain_states, [("p".into(), 2)]);
        assert_eq!(factory_restrictions.legacy_domain_states, [("p".into(), 3)]);
    }
    #[test]
    fn original_user_scope_codec_rejects_invalid_order_aliases_and_frames() {
        use aim_binder_host::parcel::Parcel;
        let record = |factory, entries: &[(i32, i32)]| {
            let mut p = Parcel::new();
            p.write_string16(Some("p"));
            p.write_i32(10123);
            p.write_string16(Some("/system/p"));
            p.write_i64(7);
            p.write_bool(factory);
            p.write_i32(entries.len() as i32);
            for (id, alias) in entries {
                p.write_i32(*id);
                p.write_i32(*alias);
            }
            p.data().to_vec()
        };
        let good = record(true, &[(0, 1), (10, 0)]);
        assert_eq!(
            OriginalUserScope::read_original_record(&good)
                .unwrap()
                .active_aliases,
            BTreeSet::from([0])
        );
        for bad in [
            record(false, &[(0, 1)]),
            record(true, &[(0, 2)]),
            record(true, &[(-1, 0)]),
            record(true, &[(10, 0), (0, 0)]),
            record(true, &[(0, 0), (0, 0)]),
        ] {
            assert!(OriginalUserScope::read_original_record(&bad).is_err());
        }
        let mut tail = good.clone();
        tail.extend_from_slice(&0i32.to_le_bytes());
        assert!(OriginalUserScope::read_original_record(&tail).is_err());
        assert!(OriginalUserScope::read_original_record(&good[..good.len() - 1]).is_err());
    }
    #[test]
    fn enable_factory_preserves_active_identity_and_restrictions_until_rescan() {
        use crate::package::{
            settings::{Package, Settings},
            system_config::SystemConfig,
        };
        let active = Package {
            name: "p".into(),
            code_path: "/data/app/p".into(),
            app_id: 10001,
            flags: 129,
            domain_set_id: Some("active-domain".into()),
            version_code: 9,
            last_update_time: 123,
            category_hint: 7,
            ..Default::default()
        };
        let factory = Package {
            name: "p".into(),
            code_path: "/system/app/p".into(),
            app_id: 10001,
            flags: 1,
            version_code: 10,
            target_sdk_version: 35,
            primary_cpu_abi: Some("arm64-v8a".into()),
            mime_groups: vec![(Some("group".into()), vec![Some("image/png".into())])],
            ..Default::default()
        };
        let mut owner = SigningScan::new(
            &SystemConfig::default(),
            &Settings {
                packages: vec![active.clone()],
                disabled_system_packages: vec![factory.clone()],
                ..Default::default()
            },
            35,
        )
        .unwrap();
        let users = BTreeMap::from([(
            0,
            UserState {
                stopped: true,
                first_install_time: 456,
                ..Default::default()
            },
        )]);
        owner.scanned_users.insert("p".into(), users.clone());
        let ids = owner.identities.clone();
        let enabled = owner.enable_system_setting("p", [3; 16]).unwrap();
        let mut expected = active;
        expected.version_code = 10;
        expected.target_sdk_version = 35;
        expected.primary_cpu_abi = factory.primary_cpu_abi;
        expected.mime_groups = factory.mime_groups;
        assert_eq!(enabled, expected);
        assert_eq!(owner.scanned_users["p"], users);
        assert_eq!(owner.identities, ids);
        assert!(owner.settings.disabled_system_packages.is_empty());
        assert!(owner.disabled_user_states("p").is_none());
        let unchanged = owner.clone();
        assert!(owner.enable_system_setting("p", [4; 16]).is_none());
        assert_eq!(owner, unchanged);
    }

    #[test]
    fn enable_factory_obeys_add_package_slot_failures_and_fresh_defaults() {
        use crate::package::{
            owner::app_ids::Owner,
            settings::{Package, Settings},
            system_config::SystemConfig,
        };
        let factory = Package {
            name: "p".into(),
            code_path: "/system/app/p".into(),
            app_id: 10001,
            flags: 1,
            version_code: 10,
            last_update_time: 123,
            shared_user: true,
            ..Default::default()
        };
        let base = Settings {
            disabled_system_packages: vec![factory.clone()],
            ..Default::default()
        };
        let mut fresh = SigningScan::new(&SystemConfig::default(), &base, 35).unwrap();
        let p = fresh.enable_system_setting("p", [3; 16]).unwrap();
        assert_eq!(
            p.domain_set_id.as_deref(),
            Some("03030303-0303-0303-0303-030303030303")
        );
        assert_eq!(p.code_path, factory.code_path);
        assert_eq!(p.version_code, 10);
        assert_eq!(p.last_update_time, 0);
        assert!(!p.shared_user);
        assert_eq!(p.category_hint, -1);
        assert!(p.is_loading());
        assert_eq!(
            fresh.identities.ids.get(10001),
            Some(&Owner::Package("p".into()))
        );
        assert!(fresh.scanned_users["p"].is_empty());
        let mut occupied = SigningScan::new(&SystemConfig::default(), &base, 35).unwrap();
        occupied
            .identities
            .ids
            .register_existing(10001, Owner::SharedUser("group".into()))
            .unwrap();
        let ids = occupied.identities.clone();
        assert!(occupied.enable_system_setting("p", [3; 16]).is_none());
        assert_eq!(occupied.identities, ids);
        assert!(occupied.settings.packages.is_empty());
        assert!(occupied.settings.disabled_system_packages.is_empty());
        let mut duplicate = SigningScan::new(
            &SystemConfig::default(),
            &Settings {
                packages: vec![Package {
                    name: "p".into(),
                    app_id: 10002,
                    ..Default::default()
                }],
                ..base
            },
            35,
        )
        .unwrap();
        let active = duplicate.settings.packages.clone();
        assert!(duplicate.enable_system_setting("p", [3; 16]).is_none());
        assert_eq!(duplicate.settings.packages, active);
        assert!(duplicate.settings.disabled_system_packages.is_empty());
    }

    #[test]
    fn copied_users_share_existing_states_but_not_later_insertions_or_restored_states() {
        let mut active = BTreeMap::from([(
            0,
            UserState {
                first_install_time: 123,
                ..Default::default()
            },
        )]);
        let mut copied = DisabledUserStates::copied(&active);
        let mut restored = DisabledUserStates::default();
        active.get_mut(&0).unwrap().first_install_time = 456;
        active.insert(
            10,
            UserState {
                first_install_time: 789,
                ..Default::default()
            },
        );
        copied.update_active(&active);
        restored.update_active(&active);
        assert_eq!(copied.users[&0].first_install_time, 456);
        assert_eq!(copied.users.len(), 1);
        assert!(restored.users.is_empty());
        active.remove(&0);
        copied.update_active(&active);
        assert_eq!(copied.users[&0].first_install_time, 456);
        active.insert(
            0,
            UserState {
                first_install_time: 999,
                ..Default::default()
            },
        );
        copied.update_active(&active);
        assert_eq!(copied.users[&0].first_install_time, 456);
    }

    #[test]
    fn updated_system_source_requires_path_change_and_strictly_newer_version_or_uid_change() {
        use UpdatedSystemSource::{KeepData, RestoreFactory};
        for (same_path, image, data, changed, expected) in [
            (false, 4, 5, false, KeepData),
            (false, 5, 5, false, KeepData),
            (false, 6, 5, false, RestoreFactory),
            (false, 4, 5, true, RestoreFactory),
            (true, 6, 5, false, KeepData),
            (true, 4, 5, true, KeepData),
            (false, i64::MAX, i64::MAX - 1, false, RestoreFactory),
            (false, i64::MIN, i64::MAX, false, KeepData),
        ] {
            assert_eq!(
                updated_system_source(same_path, image, data, changed),
                expected
            );
        }
    }
}
