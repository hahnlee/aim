//! Shared UID bootstrap and normal signing reconciliation, ported from
//! pinned PackageManagerService, Settings and ReconcilePackageUtils (#803).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::app_ids::{AppIds, Error, Owner};
use crate::package::settings::{Settings, Signatures};
use crate::package::sign::{self, MergeRule, SigningDetails};
use crate::package::system_config::SystemConfig;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct SharedUser {
    pub app_id: i32,
    pub flags: i32,
    pub private_flags: i32,
    pub signatures: Option<Signatures>,
}

impl SharedUser {
    /// ReconcilePackageUtils's normal, already-authorized merge. Members
    /// are the other parsed packages in this group, in owner scan order.
    /// Unknown initialization and OTA signature replacement are separate
    /// commit/reconciliation steps (#803). Errors leave the group intact.
    pub fn merge_authorized_lineage(
        &mut self,
        candidate: &SigningDetails,
        other_members: &[SigningDetails],
    ) -> Result<bool, String> {
        let unknown = Signatures::default();
        let previous = SigningDetails::from_saved(self.signatures.as_ref().unwrap_or(&unknown))?;
        let merged = previous.merge_lineage_with(candidate, MergeRule::OtherCapability)?;
        if matches!(&merged, Cow::Borrowed(s) if std::ptr::eq(*s, &previous)) {
            return Ok(false);
        }
        let mut merged = merged.into_owned();
        for member in other_members {
            merged = merged
                .merge_lineage_with(member, MergeRule::RestrictedCapability)?
                .into_owned();
        }
        let public_keys = sign::serialize_public_keys(&merged.public_keys)?;
        self.signatures = Some(Signatures {
            scheme_version: merged.scheme_version,
            signatures: merged.signatures,
            past_signatures: merged.past_signing_certificates,
            public_keys: Some(public_keys.into_iter().map(Some).collect()),
        });
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
}

impl Bootstrap {
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
        AppIds::restore(settings).map_err(RestoreError::Settings)?;
        let mut boot = Self::new(config);
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
        for package in &settings.packages {
            if !package.shared_user {
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
            .map(|p| p.app_id)
            .collect();
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
        self.shared_users.insert(
            name.into(),
            SharedUser {
                app_id,
                flags,
                private_flags,
                signatures: None,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{Package, SharedUser as SavedGroup};

    #[test]
    fn invalid_saved_merge_certificates_leave_the_group_unchanged() {
        let mut group = SharedUser {
            app_id: 10001,
            flags: 1,
            private_flags: 8,
            signatures: Some(Signatures {
                signatures: vec![vec![1, 2, 3]],
                ..Default::default()
            }),
        };
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
            SharedUser {
                app_id: 2900,
                flags: 1,
                private_flags: 8,
                signatures: None,
            }
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
