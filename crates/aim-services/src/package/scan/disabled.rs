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

impl SigningScan {
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
        let identity = Identity::select(&code.parsed, &self.settings, true);
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

    pub(super) fn update_disabled_user_aliases(
        &mut self,
        name: &str,
        users: &BTreeMap<i32, UserState>,
    ) {
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
        let identity = Identity::select(&code.parsed, &self.settings, true);
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
                    .find(|g| g.app_id == active.app_id)
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
