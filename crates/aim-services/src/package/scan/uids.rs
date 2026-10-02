//! UID preparation before scan reconciliation, ported from pinned
//! android-16.0.0_r1 InstallPackageHelper, Settings and ScanPackageUtils.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Code, Error, Identity};
use crate::package::{
    owner::{
        app_ids::Owner,
        shared_users::{Bootstrap, RestoreError},
    },
    pkg::booleans,
    settings::Settings,
    system_config::SystemConfig,
};
use std::collections::BTreeMap;

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
}

impl UidScan {
    pub fn new(config: &SystemConfig, settings: &Settings) -> Result<Self, RestoreError> {
        let identities = Bootstrap::restore(config, settings)?;
        let packages = settings
            .packages
            .iter()
            .map(|p| {
                let shared_user = if p.shared_user {
                    Some(
                        settings
                            .shared_users
                            .iter()
                            .find(|g| g.app_id == p.app_id)
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
        Ok(Self {
            identities,
            settings: settings.clone(),
            packages,
        })
    }

    /// Apply a fully parsed and verified system-directory candidate. Duplicate
    /// image names retain one identity; code/version selection is reconciliation.
    /// A changed existing group requires replacement metadata and is rejected
    /// here until that owner transition is implemented (#804).
    pub fn apply(&mut self, code: &Code) -> Result<(Identity, Uid), Error> {
        let identity = Identity::select(&code.parsed, &self.settings, true);
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
        Ok((identity, uid))
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
}
