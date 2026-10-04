//! UID preparation before scan reconciliation, ported from pinned
//! android-16.0.0_r1 InstallPackageHelper, Settings and ScanPackageUtils.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Code, Error, Identity, NewSetting, SettingMetadata, UserPolicy};
use crate::package::{
    owner::{
        app_ids::Owner,
        shared_users::{Bootstrap, RestoreError},
    },
    pkg::booleans,
    settings::Settings,
    system_config::SystemConfig,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uid {
    pub app_id: i32,
    pub shared_user: Option<String>,
}

/// UID ownership candidates, not persisted PackageSettings or query records.
/// Signature reconciliation and complete metadata construction follow this phase.
#[derive(Clone, Debug, PartialEq)]
pub struct UidScan {
    pub identities: Bootstrap,
    settings: Settings,
    pub packages: BTreeMap<String, Uid>,
    pending: BTreeSet<String>,
}

impl UidScan {
    pub fn new(config: &SystemConfig, settings: &Settings) -> Result<Self, RestoreError> {
        let identities = Bootstrap::restore(config, settings)?;
        Ok(Self::with_identities(settings, identities))
    }

    /// Preserve the current scan's allocation cursor and signing markers.
    pub(super) fn with_identities(settings: &Settings, identities: Bootstrap) -> Self {
        let packages = settings
            .packages
            .iter()
            .map(|p| {
                let shared_user = if p.shared_user {
                    Some(
                        settings
                            .shared_users
                            .iter()
                            .find(|g| Some(g.app_id) == p.shared_app_id())
                            .unwrap()
                            .name
                            .clone(),
                    )
                } else {
                    None
                };
                (
                    p.name.clone(),
                    Uid {
                        app_id: p.app_id,
                        shared_user,
                    },
                )
            })
            .collect();
        Self {
            identities,
            settings: settings.clone(),
            packages,
            pending: BTreeSet::new(),
        }
    }

    /// Apply a fully parsed and verified system-directory candidate. Duplicate
    /// image names retain one identity; code/version selection is reconciliation.
    /// A changed existing group requires replacement metadata and is rejected
    /// here until that owner transition is implemented (#804).
    pub fn apply(&mut self, code: &Code) -> Result<(Identity, Uid), Error> {
        let identity =
            Identity::select_for_location(&code.parsed, &self.settings, true, &code.location);
        let previous = self.packages.get(&identity.internal_name);
        let shared_user = super::signing::selected_shared_user(
            previous.is_some_and(|p| p.shared_user.is_some()),
            code.parsed.shared_user_id.as_deref(),
            code.parsed.is(booleans::LEAVING_SHARED_UID),
        )
        .map(str::to_owned);
        let fail = |message| Error {
            package: identity.internal_name.clone(),
            path: code.location.path.clone(),
            phase: "identity",
            message,
        };
        if let Some(previous) = previous {
            if previous.shared_user != shared_user {
                return Err(fail(
                    "manifest shared UID requires replacement ownership (#804)".into(),
                ));
            }
            return Ok((identity.clone(), previous.clone()));
        }
        if self
            .settings
            .disabled_system_packages
            .iter()
            .any(|p| p.name == identity.internal_name)
            || code.parsed.original_packages.as_ref().is_some_and(|names| {
                names
                    .iter()
                    .flatten()
                    .any(|name| self.settings.packages.iter().any(|p| p.name == *name))
            })
        {
            return Err(fail(
                "new setting requires disabled/original package metadata adoption (#804)".into(),
            ));
        }
        let app_id = match &shared_user {
            Some(name) => {
                self.identities
                    .get_shared_user(name, 0, 0, true)
                    .map_err(|e| fail(format!("cannot allocate shared UID: {e:?}")))?
                    .unwrap()
                    .app_id
            }
            None => self
                .identities
                .ids
                .acquire(Owner::Package(identity.internal_name.clone()))
                .map_err(|e| fail(format!("cannot allocate package UID: {e:?}")))?,
        };
        let uid = Uid {
            app_id,
            shared_user,
        };
        self.packages
            .insert(identity.internal_name.clone(), uid.clone());
        self.pending.insert(identity.internal_name.clone());
        Ok((identity, uid))
    }

    /// Construct only a newly allocated pending setting. Existing settings need
    /// the update/adoption owner rather than the new-package constructor (#804).
    /// Keeps the UID pending until signing and scan enrichment also succeed.
    pub fn new_setting(
        &self,
        identity: &Identity,
        metadata: SettingMetadata,
        users: UserPolicy<'_>,
    ) -> Result<NewSetting, Error> {
        let name = &identity.internal_name;
        let fail = |message: &str| Error {
            package: name.clone(),
            path: metadata.code_path.clone(),
            phase: "setting",
            message: message.into(),
        };
        if !self.pending.contains(name) {
            return Err(fail("new setting requires a pending UID preparation"));
        }
        let uid = self
            .packages
            .get(name)
            .ok_or_else(|| fail("package has no UID preparation"))?;
        let owner = match &uid.shared_user {
            Some(group) => Owner::SharedUser(group.clone()),
            None => Owner::Package(name.clone()),
        };
        if self.identities.ids.get(uid.app_id) != Some(&owner) {
            return Err(fail("new setting UID preparation no longer owns its slot"));
        }
        Ok(NewSetting::new(identity, uid, metadata, users))
    }

    /// After signature and metadata reconciliation succeeds, retain the UID
    /// preparation. Returns whether a new preparation was pending. No disk
    /// or query state is published here.
    pub fn accept_uid(&mut self, name: &str) -> Result<bool, Error> {
        if !self.packages.contains_key(name) {
            return Err(uid_error(name, "package has no UID preparation"));
        }
        Ok(self.pending.remove(name))
    }

    /// Discard a rejected new package. Original cleanUpAppIdCreation removes
    /// only an independently allocated package slot. Shared group slots remain
    /// until final pruning, as their optimistic package registration was not
    /// a new slot. Restored and accepted package identities remain untouched.
    pub fn reject_pending(&mut self, name: &str) -> Result<bool, Error> {
        let uid = self
            .packages
            .get(name)
            .ok_or_else(|| uid_error(name, "package has no UID preparation"))?;
        if !self.pending.contains(name) {
            return Ok(false);
        }
        let owner = match &uid.shared_user {
            Some(group) => Owner::SharedUser(group.clone()),
            None => Owner::Package(name.into()),
        };
        if self.identities.ids.get(uid.app_id) != Some(&owner) {
            return Err(uid_error(
                name,
                "rejected UID preparation no longer owns its slot",
            ));
        }
        if uid.shared_user.is_none() {
            self.identities.ids.remove(uid.app_id);
        }
        self.pending.remove(name);
        self.packages.remove(name);
        Ok(true)
    }

    /// Settings.pruneSharedUsersLPw after every preparation is resolved.
    /// Accepted active identities and saved disabled members retain groups.
    pub fn prune_unused_groups(&mut self) -> Result<Vec<String>, Error> {
        if !self.pending.is_empty() {
            return Err(uid_error(
                "",
                "UID preparations remain unresolved before pruning",
            ));
        }
        let used = self
            .packages
            .values()
            .filter(|p| p.shared_user.is_some())
            .map(|p| p.app_id)
            .chain(
                self.settings
                    .disabled_system_packages
                    .iter()
                    .filter(|p| p.shared_user)
                    .map(|p| p.app_id),
            )
            .collect();
        Ok(self.identities.prune_unreferenced(&used))
    }
}

fn uid_error(name: &str, message: &str) -> Error {
    Error {
        package: name.into(),
        path: String::new(),
        phase: "identity",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        pkg::AndroidPackage,
        scan::{Kind, Location, Partition},
        settings::Package,
        sign::SigningDetails,
    };
    fn code(name: &str) -> Code {
        Code {
            location: Location {
                path: "/system/app/input.apk".into(),
                partition: Partition::System,
                kind: Kind::App,
                apex: None,
            },
            parsed: AndroidPackage {
                package_name: name.into(),
                ..Default::default()
            },
            signing: SigningDetails::from_saved(&Default::default()).unwrap(),
        }
    }

    #[test]
    fn restored_and_static_package_uids_keep_names_and_failures_atomic() {
        let saved = Package {
            name: "known".into(),
            code_path: "/system/app/known.apk".into(),
            app_id: 10002,
            ..Default::default()
        };
        let settings = Settings {
            packages: vec![saved.clone()],
            ..Default::default()
        };
        let mut scan = UidScan::new(&Default::default(), &settings).unwrap();
        let snapshot = scan.clone();
        assert_eq!(scan.apply(&code("known")).unwrap().1.app_id, 10002);
        assert_eq!(scan, snapshot);
        let mut changed = code("known");
        changed.parsed.shared_user_id = Some("other".into());
        assert!(scan.apply(&changed).is_err());
        assert_eq!(scan, snapshot);
        let mut library = code("library");
        library.parsed.static_shared_library_name = Some("lib".into());
        library.parsed.static_shared_lib_version = 7;
        let (identity, uid) = scan.apply(&library).unwrap();
        assert_eq!(identity.internal_name, "library_7");
        assert_eq!(uid.app_id, 10000);
        assert_eq!(
            scan.identities.ids.get(10000),
            Some(&Owner::Package("library_7".into()))
        );
        library.location.path = "/apex/mount/app/library.apk".into();
        library.location.apex = Some(super::super::Apex {
            module_name: Some("raw.module".into()),
            mount_path: "/apex/mount".into(),
            partition: Partition::System,
            factory: true,
            active_changed: false,
        });
        let (identity, apex_uid) = scan.apply(&library).unwrap();
        assert_eq!(identity.internal_name, "library");
        assert_ne!(apex_uid.app_id, uid.app_id);
        let accepted = scan.clone();
        library.parsed.static_shared_lib_version += 1;
        assert_eq!(scan.apply(&library).unwrap().1.app_id, apex_uid.app_id);
        assert_eq!(scan, accepted);
        let mut original = code("renamed");
        original.parsed.original_packages = Some(vec![Some("known".into())]);
        let snapshot = scan.clone();
        assert!(scan.apply(&original).is_err());
        assert_eq!(scan, snapshot);
        let disabled = Settings {
            disabled_system_packages: vec![saved],
            ..Default::default()
        };
        let mut scan = UidScan::new(&Default::default(), &disabled).unwrap();
        let snapshot = scan.clone();
        assert!(scan.apply(&code("known")).is_err());
        assert_eq!(scan, snapshot);
    }

    #[test]
    fn allocation_exhaustion_does_not_create_a_package_or_group() {
        let mut scan = UidScan::new(&Default::default(), &Default::default()).unwrap();
        for _ in 10000..=19999 {
            scan.identities
                .ids
                .acquire(Owner::Package("filler".into()))
                .unwrap();
        }
        let snapshot = scan.clone();
        assert!(
            scan.apply(&code("exhausted"))
                .unwrap_err()
                .message
                .contains("Exhausted")
        );
        assert_eq!(scan, snapshot);
        let mut shared = code("exhausted");
        shared.parsed.shared_user_id = Some("exhausted.group".into());
        assert!(scan.apply(&shared).is_err());
        assert_eq!(scan, snapshot);
    }

    #[test]
    fn new_members_share_one_slot_and_existing_leaving_members_retain_it() {
        let mut scan = UidScan::new(&Default::default(), &Default::default()).unwrap();
        let mut first = code("first");
        first.parsed.shared_user_id = Some("group".into());
        let uid = scan.apply(&first).unwrap().1;
        let mut second = code("second");
        second.parsed.shared_user_id = Some("group".into());
        assert_eq!(scan.apply(&second).unwrap().1, uid);
        assert_eq!(
            scan.identities.ids.get(uid.app_id),
            Some(&Owner::SharedUser("group".into()))
        );
        first.parsed.booleans |= booleans::LEAVING_SHARED_UID;
        let before = scan.clone();
        assert_eq!(scan.apply(&first).unwrap().1, uid);
        assert_eq!(scan, before);
        assert_eq!(
            scan.identities.ids.acquire(Owner::Package("next".into())),
            Ok(10001)
        );
    }

    #[test]
    fn rejected_independent_preparation_releases_only_its_slot_and_advances_cursor() {
        let settings = Settings {
            packages: vec![Package {
                name: "saved".into(),
                app_id: 10002,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut scan = UidScan::new(&Default::default(), &settings).unwrap();
        assert_eq!(scan.apply(&code("failed")).unwrap().1.app_id, 10000);
        assert_eq!(scan.apply(&code("accepted")).unwrap().1.app_id, 10001);
        assert!(scan.accept_uid("accepted").unwrap());
        let before = scan.clone();
        assert!(!scan.reject_pending("saved").unwrap());
        assert!(!scan.reject_pending("accepted").unwrap());
        assert!(scan.accept_uid("missing").is_err());
        assert!(scan.reject_pending("missing").is_err());
        assert_eq!(scan, before);
        assert!(scan.reject_pending("failed").unwrap());
        assert_eq!(scan.identities.ids.get(10000), None);
        assert_eq!(
            scan.identities.ids.get(10001),
            Some(&Owner::Package("accepted".into()))
        );
        assert_eq!(
            scan.identities.ids.get(10002),
            Some(&Owner::Package("saved".into()))
        );
        assert_eq!(scan.apply(&code("next")).unwrap().1.app_id, 10003);
        assert_eq!(
            before.identities.ids.get(10000),
            Some(&Owner::Package("failed".into()))
        );
        scan.identities
            .ids
            .replace(10003, Owner::Package("other".into()))
            .unwrap();
        let before = scan.clone();
        assert!(scan.reject_pending("next").is_err());
        assert_eq!(scan, before);
    }

    #[test]
    fn rejected_shared_members_keep_group_slots_until_completed_scan_pruning() {
        let mut scan = UidScan::new(&Default::default(), &Default::default()).unwrap();
        let mut first = code("first");
        first.parsed.shared_user_id = Some("group".into());
        let uid = scan.apply(&first).unwrap().1;
        let before = scan.clone();
        assert!(scan.prune_unused_groups().is_err());
        assert_eq!(scan, before);
        assert!(scan.reject_pending("first").unwrap());
        assert_eq!(
            scan.identities.ids.get(uid.app_id),
            Some(&Owner::SharedUser("group".into()))
        );
        let mut second = code("second");
        second.parsed.shared_user_id = Some("group".into());
        assert_eq!(scan.apply(&second).unwrap().1, uid);
        assert!(scan.accept_uid("second").unwrap());
        scan.prune_unused_groups().unwrap();
        assert!(scan.identities.shared_users.contains_key("group"));
        assert_eq!(
            scan.identities.ids.get(uid.app_id),
            Some(&Owner::SharedUser("group".into()))
        );

        let mut scan = UidScan::new(&Default::default(), &Default::default()).unwrap();
        scan.apply(&first).unwrap();
        scan.reject_pending("first").unwrap();
        assert!(
            scan.prune_unused_groups()
                .unwrap()
                .contains(&"group".into())
        );
        assert_eq!(scan.identities.ids.get(10000), None);
        assert_eq!(scan.apply(&code("next")).unwrap().1.app_id, 10001);
    }

    #[test]
    fn final_uid_pruning_retains_disabled_only_saved_members() {
        let settings = Settings {
            shared_users: vec![crate::package::settings::SharedUser {
                name: "disabled".into(),
                app_id: 10004,
                ..Default::default()
            }],
            disabled_system_packages: vec![Package {
                name: "old".into(),
                app_id: 10004,
                shared_user: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut scan = UidScan::new(&Default::default(), &settings).unwrap();
        let before = scan.clone();
        let removed = scan.prune_unused_groups().unwrap();
        assert!(!removed.contains(&"disabled".into()));
        assert_eq!(scan.identities.shared_users.len(), 1);
        assert_eq!(
            scan.identities.ids.get(10004),
            Some(&Owner::SharedUser("disabled".into()))
        );
        assert_eq!(scan.apply(&code("next")).unwrap().1.app_id, 10000);
        assert_eq!(before.settings, settings);
        assert_eq!(scan.settings, settings);
    }
}
