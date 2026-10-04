//! Shared UID bootstrap and normal signing reconciliation, ported from
//! pinned PackageManagerService, Settings and ReconcilePackageUtils (#803).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::app_ids::{AppIds, Error, Owner};
use crate::package::settings::{Settings, Signatures};
use crate::package::sign::{self, History, JoinType, MergeRule, SigningDetails};
use crate::package::system_config::SystemConfig;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct SharedUser {
    pub app_id: i32,
    pub flags: i32,
    pub private_flags: i32,
    uid_flags: i32,
    uid_private_flags: i32,
    packages: BTreeMap<String, (i32, i32)>,
    retained: BTreeMap<String, std::sync::Arc<super::app_ids::DetachedSetting>>,
    pub signatures: Option<Signatures>,
    /// Per-scan state: None before reconciliation, false after a normal
    /// check, true after an OTA signer replacement. Never persisted.
    pub signatures_changed: Option<bool>,
    /// SharedUserSetting's boot-fixed seInfo SDK, initially CUR_DEVELOPMENT.
    seinfo_target_sdk: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanOrigin {
    SystemDirectory,
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureError {
    NonSystemMismatch,
    Rejected { code: i32 },
    FatalSystemMismatch,
    Certificates(String),
}

pub(in crate::package) fn saved_signatures(details: &SigningDetails) -> Result<Signatures, String> {
    if details.signatures.is_empty() {
        return Err("verified package has no signing certificates".into());
    }
    Ok(Signatures {
        scheme_version: details.scheme_version,
        signatures: details.signatures.clone(),
        past_signatures: details.past_signing_certificates.clone(),
        public_keys: Some(
            sign::serialize_public_keys(&details.public_keys)?
                .into_iter()
                .map(Some)
                .collect(),
        ),
    })
}

impl SharedUser {
    pub(in crate::package) fn package_names(&self) -> impl Iterator<Item = &str> {
        self.packages
            .keys()
            .chain(self.retained.keys())
            .map(String::as_str)
    }
    pub(in crate::package) fn has_package(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }
    pub fn member_count(&self) -> usize {
        self.packages.len() + self.retained.len()
    }

    pub fn retained_setting(&self, name: &str) -> Option<&super::app_ids::DetachedSetting> {
        self.retained.get(name).map(std::sync::Arc::as_ref)
    }

    /// Original adoption copies the setting without removing the old member.
    /// The prior member has no parsed code; retain its actual setting state.
    pub(in crate::package) fn retain_unparsed_setting(
        &mut self,
        value: super::app_ids::DetachedSetting,
    ) -> Result<(), String> {
        let package = &value.package;
        if !package.shared_user
            || package.shared_app_id() != Some(self.app_id)
            || self.packages.get(&package.name) != Some(&(package.flags, package.private_flags))
            || self.retained.contains_key(&package.name)
            || value.users.keys().any(|id| *id < 0)
            || !value
                .user_aliases
                .iter()
                .all(|id| value.users.contains_key(id))
            || value
                .legacy
                .as_ref()
                .is_some_and(|state| state.app_id() != package.app_id)
        {
            return Err("retained shared setting differs from its member owner".into());
        }
        self.packages.remove(&value.package.name);
        self.retained
            .insert(value.package.name.clone(), std::sync::Arc::new(value));
        Ok(())
    }

    pub(in crate::package) fn update_user_aliases(
        &mut self,
        name: &str,
        users: &BTreeMap<i32, crate::package::restrictions::UserState>,
    ) {
        if let Some(value) = self.retained.get_mut(name) {
            std::sync::Arc::make_mut(value).update_user_aliases(users);
        }
    }

    pub(in crate::package) fn detach_user_aliases(&mut self, name: &str) {
        if let Some(value) = self.retained.get_mut(name) {
            std::sync::Arc::make_mut(value).user_aliases.clear();
        }
    }

    pub(in crate::package) fn validate_retained(&self) -> Result<(), String> {
        for (name, value) in &self.retained {
            if value.package.name != *name
                || !value.package.shared_user
                || value.package.shared_app_id() != Some(self.app_id)
                || value.users.keys().any(|id| *id < 0)
                || !value
                    .user_aliases
                    .iter()
                    .all(|id| value.users.contains_key(id))
                || value
                    .legacy
                    .as_ref()
                    .is_some_and(|state| state.app_id() != value.package.app_id)
            {
                return Err("retained shared setting identity differs".into());
            }
        }
        Ok(())
    }

    pub fn new(app_id: i32, flags: i32, private_flags: i32) -> Self {
        Self {
            app_id,
            flags,
            private_flags,
            uid_flags: flags,
            uid_private_flags: private_flags,
            packages: BTreeMap::new(),
            retained: BTreeMap::new(),
            signatures: None,
            signatures_changed: None,
            seinfo_target_sdk: 10000,
        }
    }

    pub fn seinfo_target_sdk(&self) -> i32 {
        self.seinfo_target_sdk
    }

    /// Restore unparsed settings without substituting saved manifest metadata
    /// for PackageSetting.getPkg(), which is null at this point.
    pub fn add_package(&mut self, name: &str, flags: i32, private_flags: i32) -> bool {
        self.add_package_with_code(name, flags, private_flags, None)
    }

    /// SharedUserSetting.addPackage: the first actual parsed member seeds the
    /// SDK. Re-adding a setting updates its tracked flags without OR-ing again.
    pub fn add_package_with_code(
        &mut self,
        name: &str,
        flags: i32,
        private_flags: i32,
        target_sdk: Option<i32>,
    ) -> bool {
        if self.member_count() == 0 {
            if let Some(target) = target_sdk {
                self.seinfo_target_sdk = target;
            }
        }
        let added = self
            .packages
            .insert(name.into(), (flags, private_flags))
            .is_none();
        if added {
            self.flags |= flags;
            self.private_flags |= private_flags;
        }
        added
    }

    /// Boot's fixSeInfoLocked considers only members with actual parsed code.
    /// Runtime additions/removals do not recompute this value. Label overrides
    /// from that original method are populated separately by the seInfo owner (#838).
    pub fn fix_seinfo_target_sdk_at_boot(&mut self, parsed_targets: &BTreeMap<String, i32>) {
        for name in self.packages.keys() {
            if let Some(target) = parsed_targets.get(name) {
                self.seinfo_target_sdk = self.seinfo_target_sdk.min(*target);
            }
        }
    }

    /// SharedUserSetting.removePackage: only active members contribute flags;
    /// constructor UID flags survive the removal of the last member.
    pub fn remove_package(&mut self, name: &str) -> bool {
        let Some((flags, private_flags)) = self.packages.remove(name) else {
            return false;
        };
        if self.flags & flags != 0 {
            self.flags = self
                .packages
                .values()
                .map(|p| p.0)
                .chain(self.retained.values().map(|p| p.package.flags))
                .fold(self.uid_flags, |v, flags| v | flags);
        }
        if self.private_flags & private_flags != 0 {
            self.private_flags = self
                .packages
                .values()
                .map(|p| p.1)
                .chain(self.retained.values().map(|p| p.package.private_flags))
                .fold(self.uid_private_flags, |v, flags| v | flags);
        }
        true
    }

    /// ReconcilePackageUtils's normal, already-authorized merge. Members
    /// are the other parsed packages in this group, in owner scan order.
    /// Errors leave the group and its scan state intact.
    pub fn merge_authorized_lineage(
        &mut self,
        candidate: &SigningDetails,
        other_members: &[SigningDetails],
    ) -> Result<bool, String> {
        let unknown = Signatures::default();
        let previous = SigningDetails::from_saved(self.signatures.as_ref().unwrap_or(&unknown))?;
        let merged = previous.merge_lineage_with(candidate, MergeRule::OtherCapability)?;
        if matches!(&merged, Cow::Borrowed(s) if std::ptr::eq(*s, &previous)) {
            self.signatures_changed.get_or_insert(false);
            return Ok(false);
        }
        let mut merged = merged.into_owned();
        for member in other_members {
            merged = merged
                .merge_lineage_with(member, MergeRule::RestrictedCapability)?
                .into_owned();
        }
        self.signatures = Some(saved_signatures(&merged)?);
        self.signatures_changed.get_or_insert(false);
        Ok(true)
    }

    /// The physical-system exception in ReconcilePackageUtils, invoked
    /// only after the normal signature gates failed. A saved FLAG_SYSTEM
    /// on a /data APK does not select SystemDirectory. Existing members
    /// are irrelevant to the SYSTEM join rule.
    pub fn replace_after_signature_failure(
        &mut self,
        candidate: &SigningDetails,
        origin: ScanOrigin,
        first_api_level: i32,
    ) -> Result<(), SignatureError> {
        if origin != ScanOrigin::SystemDirectory {
            return Err(SignatureError::NonSystemMismatch);
        }
        let unknown = Signatures::default();
        let previous = self.signatures.as_ref().unwrap_or(&unknown);
        if self.signatures_changed.is_some()
            && !History::verified(candidate).can_join_shared_user(
                &History::saved(previous),
                JoinType::System,
                &[],
            )
        {
            return Err(if first_api_level <= 29 {
                SignatureError::Rejected {
                    code: sign::INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES,
                }
            } else {
                SignatureError::FatalSystemMismatch
            });
        }
        let signatures = saved_signatures(candidate).map_err(SignatureError::Certificates)?;
        self.signatures = Some(signatures);
        self.signatures_changed = Some(true);
        Ok(())
    }

    /// Settings.insertPackageSettingLPw initializes unknown group
    /// signatures during commit, after reconciliation has checked the
    /// candidate. Existing signing details and the scan marker survive.
    pub fn commit_initial_signatures(
        &mut self,
        candidate: &SigningDetails,
    ) -> Result<bool, String> {
        if self
            .signatures
            .as_ref()
            .is_some_and(|s| !s.signatures.is_empty())
        {
            return Ok(false);
        }
        self.signatures = Some(saved_signatures(candidate)?);
        Ok(true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    InvalidName,
    InvalidOemId,
    ConflictingName { existing_id: i32 },
    Slot(Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub name: String,
    pub app_id: i32,
    pub reason: Rejection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bootstrap {
    pub ids: AppIds,
    pub shared_users: BTreeMap<String, SharedUser>,
    pub rejected: Vec<Rejected>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    Settings(Error),
    Conflict(Rejected),
    KeySets(String),
    Apex(String),
}

impl Bootstrap {
    /// Settings.getSharedUserLPw: lookup never changes an existing group;
    /// creation reserves an automatic UID before exposing the new group.
    pub fn get_shared_user(
        &mut self,
        name: &str,
        flags: i32,
        private_flags: i32,
        create: bool,
    ) -> Result<Option<&SharedUser>, Error> {
        if !self.shared_users.contains_key(name) && create {
            let app_id = self.ids.acquire(Owner::SharedUser(name.into()))?;
            self.shared_users
                .insert(name.into(), SharedUser::new(app_id, flags, private_flags));
        }
        Ok(self.shared_users.get(name))
    }

    /// Seed before Settings or APK reconciliation. No persistence or
    /// query snapshot is changed; rejected OEM records remain visible.
    pub fn new(config: &SystemConfig) -> Self {
        let mut boot = Self {
            ids: AppIds::default(),
            shared_users: BTreeMap::new(),
            rejected: Vec::new(),
        };
        for (name, id) in [
            ("android.uid.system", 1000),
            ("android.uid.phone", 1001),
            ("android.uid.log", 1007),
            ("android.uid.nfc", 1027),
            ("android.uid.bluetooth", 1002),
            ("android.uid.shell", 2000),
            ("android.uid.se", 1031),
            ("android.uid.networkstack", 1073),
            ("android.uid.uwb", 1083),
        ] {
            boot.register(name, id, 1, 8)
                .expect("distinct platform shared users");
        }
        for (name, id) in &config.oem_defined_uids {
            let result = if !name.starts_with("android.uid") {
                Err(Rejection::InvalidName)
            } else if !(2900..=2999).contains(id) {
                Err(Rejection::InvalidOemId)
            } else {
                boot.register(name, *id, 1, 8)
            };
            if let Err(reason) = result {
                boot.rejected.push(Rejected {
                    name: name.clone(),
                    app_id: *id,
                    reason,
                });
            }
        }
        boot
    }

    /// Merge already-decoded settings into the pre-scan identity input.
    /// Existing seed flags survive, as addSharedUserLPw returns the old
    /// group; saved signatures are loaded into that group. Conflicting
    /// settings fail the candidate rather than remapping any package.
    pub fn restore(config: &SystemConfig, settings: &Settings) -> Result<Self, RestoreError> {
        let mut boot = Self::new(config);
        AppIds::restore(&Settings {
            shared_users: settings.shared_users.clone(),
            ..Default::default()
        })
        .map_err(RestoreError::Settings)?;
        for saved in &settings.shared_users {
            boot.register(&saved.name, saved.app_id, saved.flags, 0)
                .map_err(|reason| {
                    RestoreError::Conflict(Rejected {
                        name: saved.name.clone(),
                        app_id: saved.app_id,
                        reason,
                    })
                })?;
            boot.shared_users.get_mut(&saved.name).unwrap().signatures = saved.signatures.clone();
        }
        AppIds::restore_seeded(settings, &boot.ids).map_err(RestoreError::Settings)?;
        for package in &settings.packages {
            if package.shared_user {
                let name = match boot.ids.get(package.uid_owner_id()) {
                    Some(Owner::SharedUser(name)) => name.clone(),
                    _ => unreachable!("AppIds::restore validated shared UID ownership"),
                };
                boot.shared_users.get_mut(&name).unwrap().add_package(
                    &package.name,
                    package.flags,
                    package.private_flags,
                );
            } else {
                boot.ids
                    .register_existing(package.app_id, Owner::Package(package.name.clone()))
                    .map_err(RestoreError::Settings)?;
            }
        }
        Ok(boot)
    }

    /// After reconciliation, active or disabled members keep a group,
    /// including packages uninstalled for every user. Removing empty
    /// groups also advances the original runtime allocation cursor.
    pub fn prune_unused(&mut self, settings: &Settings) -> Vec<String> {
        let used: BTreeSet<_> = settings
            .packages
            .iter()
            .chain(&settings.disabled_system_packages)
            .filter(|p| p.shared_user)
            .map(|p| p.uid_owner_id())
            .collect();
        self.prune_unreferenced(&used)
    }

    pub(in crate::package) fn prune_unreferenced(&mut self, used: &BTreeSet<i32>) -> Vec<String> {
        let removed: Vec<_> = self
            .shared_users
            .iter()
            .filter(|(_, group)| !used.contains(&group.app_id))
            .map(|(name, _)| name.clone())
            .collect();
        for name in &removed {
            let group = self.shared_users.remove(name).unwrap();
            self.ids.remove(group.app_id);
        }
        removed
    }

    fn register(
        &mut self,
        name: &str,
        app_id: i32,
        flags: i32,
        private_flags: i32,
    ) -> Result<(), Rejection> {
        if let Some(old) = self.shared_users.get(name) {
            return if old.app_id == app_id {
                Ok(())
            } else {
                Err(Rejection::ConflictingName {
                    existing_id: old.app_id,
                })
            };
        }
        self.ids
            .register_existing(app_id, Owner::SharedUser(name.into()))
            .map_err(Rejection::Slot)?;
        self.shared_users
            .insert(name.into(), SharedUser::new(app_id, flags, private_flags));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{Package, SharedUser as SavedGroup};

    #[test]
    fn retained_unparsed_members_keep_instance_flags_sdk_and_old_versions() {
        let mut group = SharedUser::new(10000, 8, 16);
        group.add_package("same", 1, 2);
        let prior = group.clone();
        let retained = crate::package::owner::app_ids::DetachedSetting {
            package: crate::package::settings::Package {
                name: "same".into(),
                app_id: 10000,
                shared_user: true,
                flags: 1,
                private_flags: 2,
                ..Default::default()
            },
            users: Default::default(),
            user_aliases: Default::default(),
            legacy: None,
            install_fixed: None,
            runtime: None,
        };
        let mut foreign = retained.clone();
        foreign.package.app_id = 10001;
        assert!(group.retain_unparsed_setting(foreign).is_err());
        assert_eq!(group, prior);
        group.retain_unparsed_setting(retained.clone()).unwrap();
        group.add_package_with_code("same", 4, 32, Some(36));
        assert_eq!(group.member_count(), 2);
        assert_eq!(
            group.package_names().collect::<Vec<_>>(),
            vec!["same", "same"]
        );
        assert_eq!(group.seinfo_target_sdk(), 10000);
        assert_eq!((group.flags, group.private_flags), (13, 50));
        assert_eq!(prior.member_count(), 1);
        assert_eq!(prior.retained_setting("same"), None);
        let captured = group.clone();
        assert!(group.remove_package("same"));
        assert_eq!(group.member_count(), 1);
        assert!(!group.has_package("same"));
        assert_eq!((group.flags, group.private_flags), (9, 18));
        assert_eq!(group.retained_setting("same"), Some(&retained));
        group.validate_retained().unwrap();
        assert_eq!(captured.member_count(), 2);
        assert_eq!((captured.flags, captured.private_flags), (13, 50));
        assert_eq!(captured.retained_setting("same"), Some(&retained));
    }

    #[test]
    fn runtime_shared_id_owns_membership_without_reserving_package_app_id() {
        let mut settings = Settings {
            packages: vec![Package {
                name: "container".into(),
                app_id: 10123,
                shared_user: true,
                shared_user_app_id: Some(1000),
                flags: 1,
                private_flags: 8,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut boot = Bootstrap::restore(&SystemConfig::default(), &settings).unwrap();
        assert!(boot.shared_users["android.uid.system"].has_package("container"));
        assert_eq!(boot.shared_users["android.uid.system"].flags, 1);
        assert_eq!(boot.shared_users["android.uid.system"].private_flags, 8);
        assert_eq!(boot.ids.get(10123), None);
        assert_eq!(
            boot.ids
                .acquire(Owner::Package("first.apk".into()))
                .unwrap(),
            10000
        );
        settings.disabled_system_packages = settings.packages.clone();
        settings.packages.clear();
        boot.prune_unused(&settings);
        assert!(boot.shared_users.contains_key("android.uid.system"));
        settings.packages = settings.disabled_system_packages.clone();
        settings.packages[0].shared_user_app_id = Some(9999);
        assert!(Bootstrap::restore(&SystemConfig::default(), &settings).is_err());
        settings.packages[0].shared_user_app_id = Some(-1);
        assert!(Bootstrap::restore(&SystemConfig::default(), &settings).is_err());
    }

    #[test]
    fn seinfo_sdk_tracks_first_code_boot_minimum_and_runtime_lifetime() {
        let mut group = SharedUser::new(10100, 0, 0);
        assert_eq!(group.seinfo_target_sdk(), 10000);
        group.add_package("absent", 0, 0);
        group.add_package_with_code("a", 0, 0, Some(36));
        assert_eq!(group.seinfo_target_sdk(), 10000);
        group.fix_seinfo_target_sdk_at_boot(&[("a".into(), 36), ("foreign".into(), 1)].into());
        assert_eq!(group.seinfo_target_sdk(), 36);
        group.add_package_with_code("b", 0, 0, Some(28));
        assert_eq!(group.seinfo_target_sdk(), 36);
        group.fix_seinfo_target_sdk_at_boot(&[("a".into(), 36), ("b".into(), 28)].into());
        assert_eq!(group.seinfo_target_sdk(), 28);
        let captured = group.clone();
        group.add_package_with_code("c", 0, 0, Some(19));
        assert_eq!(group.seinfo_target_sdk(), 28);
        for name in ["a", "b", "c", "absent"] {
            group.remove_package(name);
        }
        assert_eq!(group.seinfo_target_sdk(), 28);
        group.add_package_with_code("d", 0, 0, Some(35));
        assert_eq!(group.seinfo_target_sdk(), 35);
        group.add_package_with_code("e", 0, 0, Some(24));
        assert_eq!(group.seinfo_target_sdk(), 35);
        group.fix_seinfo_target_sdk_at_boot(&[("d".into(), 35), ("e".into(), 24)].into());
        assert_eq!(group.seinfo_target_sdk(), 24);
        assert_eq!(captured.seinfo_target_sdk(), 28);
        let mut reboot = SharedUser::new(10100, 0, 0);
        reboot.add_package("d", 0, 0);
        reboot.add_package("e", 0, 0);
        reboot.fix_seinfo_target_sdk_at_boot(&BTreeMap::new());
        assert_eq!(reboot.seinfo_target_sdk(), 10000);
    }

    #[test]
    fn active_member_flags_preserve_uid_seeds_duplicate_state_and_removal_conditions() {
        let mut group = SharedUser::new(10001, 64, 8);
        assert!(group.add_package("a", 1, 32));
        assert!(group.add_package("b", 128, 16));
        assert_eq!((group.flags, group.private_flags), (193, 56));
        assert!(!group.add_package("a", 2, 0));
        assert_eq!((group.flags, group.private_flags), (193, 56));
        assert!(group.remove_package("a"));
        // Its current bits were never aggregated, so removal does not rebuild.
        assert_eq!((group.flags, group.private_flags), (193, 56));
        assert!(!group.remove_package("a"));
        assert!(group.remove_package("b"));
        assert_eq!((group.flags, group.private_flags), (64, 8));
        assert!(group.add_package("a", 1, 32));
        assert!(!group.add_package("a", 1, 16));
        assert!(group.add_package("b", 128, 32));
        assert!(group.remove_package("b"));
        assert_eq!((group.flags, group.private_flags), (65, 24));
    }

    #[test]
    fn restoration_aggregates_active_members_without_disabled_package_flags() {
        use crate::package::settings::{Package, SharedUser as Saved};
        let settings = Settings {
            shared_users: vec![Saved {
                name: "group".into(),
                app_id: 10001,
                flags: 1,
                ..Default::default()
            }],
            packages: vec![Package {
                name: "active".into(),
                app_id: 10001,
                shared_user: true,
                flags: 64,
                private_flags: 8,
                ..Default::default()
            }],
            disabled_system_packages: vec![Package {
                name: "old".into(),
                app_id: 10001,
                shared_user: true,
                flags: 128,
                private_flags: 16,
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = settings.clone();
        let mut boot = Bootstrap::restore(&Default::default(), &settings).unwrap();
        let group = boot.shared_users.get_mut("group").unwrap();
        assert_eq!((group.flags, group.private_flags), (65, 8));
        assert!(!group.remove_package("old"));
        assert!(group.remove_package("active"));
        assert_eq!((group.flags, group.private_flags), (1, 0));
        assert_eq!(settings, before);
    }

    #[test]
    fn dynamic_shared_user_lookup_and_exhaustion_preserve_existing_ownership() {
        let mut boot = Bootstrap::new(&Default::default());
        let snapshot = boot.clone();
        assert_eq!(boot.get_shared_user("missing", 3, 4, false), Ok(None));
        assert_eq!(boot, snapshot);
        assert_eq!(
            boot.get_shared_user("android.uid.system", 0, 0, true)
                .unwrap()
                .unwrap(),
            &snapshot.shared_users["android.uid.system"]
        );
        assert_eq!(boot, snapshot);
        let created = boot
            .get_shared_user("new.group", 3, 4, true)
            .unwrap()
            .unwrap()
            .clone();
        assert_eq!(
            (created.app_id, created.flags, created.private_flags),
            (10000, 3, 4)
        );
        assert!(created.signatures.is_none() && created.signatures_changed.is_none());
        assert_eq!(
            boot.ids.get(10000),
            Some(&Owner::SharedUser("new.group".into()))
        );
        assert_eq!(
            boot.get_shared_user("new.group", 1, 8, true).unwrap(),
            Some(&created)
        );
        for _ in 10001..=19999 {
            boot.ids.acquire(Owner::Package("filler".into())).unwrap();
        }
        let snapshot = boot.clone();
        assert_eq!(
            boot.get_shared_user("exhausted", 0, 0, true),
            Err(Error::Exhausted)
        );
        assert_eq!(boot, snapshot);
        assert_eq!(
            boot.get_shared_user("new.group", 0, 0, true).unwrap(),
            Some(&created)
        );
    }

    #[test]
    fn invalid_saved_merge_certificates_leave_the_group_unchanged() {
        let mut group = SharedUser::new(10001, 1, 8);
        group.signatures = Some(Signatures {
            signatures: vec![vec![1, 2, 3]],
            ..Default::default()
        });
        let before = group.clone();
        let unknown = SigningDetails::from_saved(&Default::default()).unwrap();
        assert!(group.merge_authorized_lineage(&unknown, &[]).is_err());
        assert_eq!(group, before);
    }

    #[test]
    fn restoration_preserves_seed_flags_saved_signers_members_and_pruning_cursor() {
        let signing = Signatures {
            scheme_version: 3,
            signatures: vec![vec![1, 2, 3]],
            ..Default::default()
        };
        let mut settings = Settings::default();
        settings.shared_users = [
            ("android.uid.system", 1000),
            ("active", 10002),
            ("disabled", 10004),
            ("empty", 10005),
        ]
        .map(|(name, app_id)| SavedGroup {
            name: name.into(),
            app_id,
            flags: 0,
            signatures: Some(signing.clone()),
        })
        .into();
        let package = |name: &str, app_id, shared_user| Package {
            name: name.into(),
            app_id,
            shared_user,
            ..Default::default()
        };
        settings.packages = vec![
            package("system", 1000, true),
            package("a", 10002, true),
            package("b", 10002, true),
            package("standalone", 10000, false),
        ];
        settings.disabled_system_packages = vec![package("old", 10004, true)];
        let before = settings.clone();
        let mut boot = Bootstrap::restore(&Default::default(), &settings).unwrap();
        assert_eq!(boot.shared_users["android.uid.system"].flags, 1);
        assert_eq!(boot.shared_users["android.uid.system"].private_flags, 8);
        assert_eq!(
            boot.shared_users["android.uid.system"].signatures,
            Some(signing.clone())
        );
        assert_eq!(boot.shared_users["active"].flags, 0);
        assert_eq!(boot.shared_users["active"].private_flags, 0);
        let snapshot = boot.clone();
        let removed = boot.prune_unused(&settings);
        assert!(removed.contains(&"empty".into()));
        assert!(removed.contains(&"android.uid.log".into()));
        assert_eq!(boot.shared_users.len(), 3);
        assert_eq!(boot.shared_users["disabled"].signatures, Some(signing));
        assert_eq!(
            boot.ids.get(10000),
            Some(&Owner::Package("standalone".into()))
        );
        assert_eq!(boot.ids.get(10005), None);
        assert!(snapshot.shared_users.contains_key("empty"));
        assert_eq!(boot.ids.acquire(Owner::Package("new".into())), Ok(10006));
        assert!(boot.prune_unused(&settings).is_empty());
        assert_eq!(settings, before);
    }

    #[test]
    fn conflicting_saved_identity_fails_without_remapping_or_mutation() {
        let mut settings = Settings::default();
        settings.shared_users.push(SavedGroup {
            name: "android.uid.system".into(),
            app_id: 1001,
            ..Default::default()
        });
        let before = settings.clone();
        assert_eq!(
            Bootstrap::restore(&Default::default(), &settings),
            Err(RestoreError::Conflict(Rejected {
                name: "android.uid.system".into(),
                app_id: 1001,
                reason: Rejection::ConflictingName { existing_id: 1000 },
            }))
        );
        assert_eq!(settings, before);
        settings.shared_users.clear();
        settings.packages.push(Package {
            name: "conflicting-app".into(),
            app_id: 1000,
            ..Default::default()
        });
        assert!(matches!(
            Bootstrap::restore(&Default::default(), &settings),
            Err(RestoreError::Settings(Error::Occupied { .. }))
        ));
    }

    #[test]
    fn seeds_fixed_ids_and_retains_oem_rejections_without_reassigning() {
        let mut config = SystemConfig::default();
        config.oem_defined_uids = [
            ("android.uid.oem", 2900),
            ("android.uid.collision", 2900),
            ("android.uid.system", 2901),
            ("other", 2902),
            ("android.uid.low", 2899),
            ("android.uid.high", 3000),
            ("android.uid", 2999),
        ]
        .map(|(name, id)| (name.into(), id))
        .into();
        let mut boot = Bootstrap::new(&config);
        assert_eq!(boot.shared_users.len(), 11);
        assert_eq!(boot.shared_users["android.uid.system"].app_id, 1000);
        assert_eq!(
            boot.shared_users["android.uid.oem"],
            SharedUser::new(2900, 1, 8)
        );
        assert_eq!(boot.rejected.len(), 5);
        assert!(matches!(
            boot.rejected[0].reason,
            Rejection::Slot(Error::Occupied { .. })
        ));
        assert_eq!(
            boot.rejected[1].reason,
            Rejection::ConflictingName { existing_id: 1000 }
        );
        assert_eq!(boot.rejected[2].reason, Rejection::InvalidName);
        assert_eq!(boot.rejected[3].reason, Rejection::InvalidOemId);
        assert_eq!(boot.rejected[4].reason, Rejection::InvalidOemId);
        assert_eq!(boot.ids.acquire(Owner::Package("new".into())), Ok(10000));
    }
}
