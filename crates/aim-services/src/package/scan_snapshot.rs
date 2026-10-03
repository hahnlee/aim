//! Captured native scan owners for the C facade's replica builder (#836).
//! This is not the complete PackageState replica or its visibility policy.
use super::{
    owner::{app_ids::Owner, usage::Usage},
    scan::SigningScan,
};
pub mod endpoint;
pub mod user_record;
pub mod setting_record;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Stale,
    VersionExhausted,
}

/// Scan state and usage belong to this version, including shared UID signing,
/// keysets, libraries, user state and active/factory collected code.
#[derive(Debug)]
pub struct Snapshot {
    version: u64,
    owner: SigningScan,
    usage: Usage,
}

impl Snapshot {
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
    pub fn owner(&self) -> &SigningScan {
        &self.owner
    }
}

pub struct Store {
    current: Mutex<Arc<Snapshot>>,
}

impl Store {
    pub fn new(owner: SigningScan, usage: Usage) -> Result<Self, Error> {
        validate(&owner, &usage)?;
        Ok(Self {
            current: Mutex::new(Arc::new(Snapshot {
                version: 1,
                owner,
                usage,
            })),
        })
    }

    pub fn capture(&self) -> Arc<Snapshot> {
        self.current.lock().unwrap().clone()
    }

    /// The commit owner supplies a completed candidate. An exact captured base
    /// prevents stale or foreign transactions from replacing a newer graph.
    /// Files, broadcasts and permission callbacks still belong to that owner.
    pub fn publish(
        &self,
        base: &Arc<Snapshot>,
        owner: SigningScan,
        usage: Usage,
    ) -> Result<Arc<Snapshot>, Error> {
        let mut current = self.current.lock().unwrap();
        if !Arc::ptr_eq(&current, base) {
            return Err(Error::Stale);
        }
        let version = current
            .version
            .checked_add(1)
            .ok_or(Error::VersionExhausted)?;
        validate(&owner, &usage)?;
        let next = Arc::new(Snapshot {
            version,
            owner,
            usage,
        });
        *current = next.clone();
        Ok(next)
    }
}

fn validate(owner: &SigningScan, usage: &Usage) -> Result<(), Error> {
    let fail = |message: &str| Error::Invalid(message.into());
    if !owner.capture_ready() {
        return Err(fail("scan metadata is not finalized"));
    }
    owner.validate_seinfo().map_err(Error::Invalid)?;
    owner
        .validate_library_dependencies()
        .map_err(Error::Invalid)?;
    owner
        .validate_legacy_permissions()
        .map_err(Error::Invalid)?;
    let package_names: BTreeSet<_> = owner
        .settings
        .packages
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    if usage.names().collect::<BTreeSet<_>>() != package_names {
        return Err(fail("usage package membership differs"));
    }
    for (settings, loaded, active) in [
        (&owner.settings.packages, owner.loaded_packages(), true),
        (
            &owner.settings.disabled_system_packages,
            owner.disabled_loaded_packages(),
            false,
        ),
    ] {
        let mut names = BTreeSet::new();
        for setting in settings {
            let normalized = setting
                .install_source
                .clone()
                .normalized()
                .map_err(Error::Invalid)?;
            if normalized != setting.install_source {
                return Err(fail("install source is not normalized"));
            }

            let mut mime_names = BTreeSet::new();
            if setting
                .mime_groups
                .iter()
                .any(|(name, _)| !mime_names.insert(name))
            {
                return Err(fail("duplicate MIME group owner"));
            }
            if !names.insert(setting.name.as_str()) {
                return Err(fail("duplicate package setting"));
            }
            if active {
                match owner.identities.ids.get(setting.app_id) {
                    Some(Owner::Package(name)) if !setting.shared_user && name == &setting.name => {
                    }
                    Some(Owner::SharedUser(name)) if setting.shared_user => {
                        let group = owner
                            .identities
                            .shared_users
                            .get(name)
                            .ok_or_else(|| fail("missing shared UID owner"))?;
                        if group.app_id != setting.app_id || !group.has_package(&setting.name) {
                            return Err(fail("package shared UID membership differs"));
                        }
                    }
                    _ => return Err(fail("package UID owner differs")),
                }
            }
        }
        for (name, code) in loaded {
            let setting = settings
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| fail("loaded code has no setting"))?;
            if code.package.package_name != *name
                || code.package.path.as_deref() != Some(setting.code_path.as_str())
                || (active && code.package.uid != setting.app_id)
                || code.package.signing_details.as_ref()
                    != Some(
                        &code
                            .collected_signing
                            .parcel_details()
                            .map_err(Error::Invalid)?,
                    )
            {
                return Err(fail("loaded code differs from its owner"));
            }
            let users = if active {
                owner.scanned_user_states(name)
            } else {
                owner.disabled_user_states(name)
            };
            if users.is_none() {
                return Err(fail("loaded code has no captured user state"));
            }
        }
    }
    let mut groups = BTreeSet::new();
    for saved in &owner.settings.shared_users {
        if !groups.insert(saved.name.as_str()) {
            return Err(fail("duplicate shared UID setting"));
        }
        let group = owner
            .identities
            .shared_users
            .get(&saved.name)
            .ok_or_else(|| fail("missing saved shared UID owner"))?;
        if group.app_id != saved.app_id || group.signatures != saved.signatures {
            return Err(fail("saved shared UID signing differs from its owner"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::settings::{Package, Settings};

    fn owner() -> SigningScan {
        SigningScan::new(
            &Default::default(),
            &Settings {
                packages: vec![Package {
                    name: "fixture".into(),
                    app_id: 10100,
                    code_path: "/data/app/fixture".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            36,
        )
        .unwrap()
    }

    #[test]
    fn usage_is_mandatory_matches_membership_and_isolated_between_versions() {
        assert!(matches!(
            Store::new(owner(), Usage::new([])),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Store::new(owner(), Usage::new(["fixture", "foreign"])),
            Err(Error::Invalid(_))
        ));
        let mut usage = Usage::new(["fixture"]);
        usage.apply(b"fixture 17\n").unwrap();
        let store = Store::new(owner(), usage).unwrap();
        let old = store.capture();
        assert!(matches!(
            store.publish(&old, owner(), Usage::new(["foreign"])),
            Err(Error::Invalid(_))
        ));
        assert!(Arc::ptr_eq(&old, &store.capture()));
        assert_eq!(old.usage().times("fixture"), Some(&[17; 8]));
        let mut next = old.usage().clone();
        next.notify("fixture", 2, 29);
        let current = store.publish(&old, owner(), next.clone()).unwrap();
        next.notify("fixture", 2, 44);
        assert_eq!(old.usage().latest_foreground("fixture"), Some(17));
        assert_eq!(current.usage().latest_foreground("fixture"), Some(29));
        assert_eq!(current.usage().latest("foreign"), None);
        assert_eq!(
            store.publish(&old, owner(), next).unwrap_err(),
            Error::Stale
        );
        assert!(Arc::ptr_eq(&current, &store.capture()));
    }

    #[test]
    fn invalid_install_source_cannot_publish_over_a_capture() {
        let store = Store::new(owner(), Usage::new(["fixture"])).unwrap();
        let base = store.capture();
        for signed in [false, true] {
            let mut candidate = base.owner().clone();
            let source = &mut candidate.settings.packages[0].install_source;
            if signed {
                source.initiating_package_signatures = Some(Default::default());
            } else {
                source.installer_uid = 123;
            }
            assert!(matches!(
                store.publish(&base, candidate, base.usage().clone()),
                Err(Error::Invalid(_))
            ));
            assert!(Arc::ptr_eq(&store.capture(), &base));
        }
    }

    #[test]
    fn capture_survives_updates_and_stale_foreign_or_invalid_commits() {
        let store = Store::new(owner(), Usage::new(["fixture"])).unwrap();
        let original = store.capture();
        let foreign = Store::new(owner(), Usage::new(["fixture"]))
            .unwrap()
            .capture();
        assert_eq!(
            store
                .publish(&foreign, owner(), Usage::new(["fixture"]))
                .unwrap_err(),
            Error::Stale
        );
        let mut invalid = owner();
        invalid.settings.packages[0].app_id += 1;
        assert!(matches!(
            store.publish(&original, invalid, original.usage().clone()),
            Err(Error::Invalid(_))
        ));
        assert!(Arc::ptr_eq(&store.capture(), &original));
        let mut candidate = original.owner().clone();
        candidate.settings.packages[0].version_code = 2;
        let next = store
            .publish(&original, candidate.clone(), original.usage().clone())
            .unwrap();
        candidate.settings.packages[0].version_code = 3;
        assert_eq!(original.owner().settings.packages[0].version_code, 0);
        assert_eq!(next.owner().settings.packages[0].version_code, 2);
        assert_eq!(next.version(), 2);
        assert_eq!(
            store
                .publish(&original, candidate, original.usage().clone())
                .unwrap_err(),
            Error::Stale
        );
    }

    #[test]
    fn concurrent_commits_from_one_capture_have_one_winner_and_overflow_preserves_state() {
        let store = Arc::new(Store::new(owner(), Usage::new(["fixture"])).unwrap());
        let base = store.capture();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut handles = vec![];
        for version in [11, 12] {
            let (store, base, barrier) = (store.clone(), base.clone(), barrier.clone());
            handles.push(std::thread::spawn(move || {
                let mut candidate = base.owner().clone();
                candidate.settings.packages[0].version_code = version;
                barrier.wait();
                store.publish(&base, candidate, base.usage().clone())
            }));
        }
        barrier.wait();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(Error::Stale)))
                .count(),
            1
        );
        let exhausted = Arc::new(Snapshot {
            version: u64::MAX,
            owner: owner(),
            usage: Usage::new(["fixture"]),
        });
        *store.current.lock().unwrap() = exhausted.clone();
        assert_eq!(
            store
                .publish(&exhausted, owner(), exhausted.usage().clone())
                .unwrap_err(),
            Error::VersionExhausted
        );
        assert!(Arc::ptr_eq(&store.capture(), &exhausted));
    }
}
