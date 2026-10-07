//! Settings.RuntimePermissionPersistence metadata, Android 16 r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::legacy_permissions::Metadata;
use std::collections::{BTreeMap, BTreeSet};

/// Constructor sparse defaults are version 0, null fingerprint and upgrade=true.
/// Write requests must be consumed by the runtime persistence scheduler.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    versions: BTreeMap<i32, i32>,
    fingerprints: BTreeMap<i32, Option<String>>,
    upgrade_needed: BTreeMap<i32, bool>,
    extended_fingerprint: Option<String>,
    writes: BTreeSet<i32>,
}

impl State {
    pub fn restore(&mut self, metadata: &Metadata) {
        for (&user, state) in &metadata.users {
            if let Some(version) = state.version {
                self.versions.insert(user, version);
                self.fingerprints.insert(user, state.fingerprint.clone());
            }
            if state.rewrite_requested {
                self.writes.insert(user);
            }
        }
    }
    pub fn version(&self, user: i32) -> i32 {
        self.versions.get(&user).copied().unwrap_or(0)
    }
    pub fn fingerprint(&self, user: i32) -> Option<&str> {
        self.fingerprints.get(&user).and_then(|v| v.as_deref())
    }
    pub fn upgrade_needed(&self, user: i32) -> bool {
        self.upgrade_needed.get(&user).copied().unwrap_or(true)
    }
    pub fn set_version(&mut self, user: i32, version: i32) {
        self.versions.insert(user, version);
        self.writes.insert(user);
    }
    pub fn set_controller_version(&mut self, partition_fingerprint: &str, version: i64) {
        let extended = format!("{partition_fingerprint}?pc_version={version}");
        for (&user, fingerprint) in &self.fingerprints {
            self.upgrade_needed
                .insert(user, fingerprint.as_deref() != Some(extended.as_str()));
        }
        self.extended_fingerprint = Some(extended);
    }
    pub fn update_fingerprint(&mut self, user: i32) -> Result<(), String> {
        let fingerprint = self
            .extended_fingerprint
            .clone()
            .ok_or("permission controller version is not set")?;
        self.fingerprints.insert(user, Some(fingerprint));
        self.upgrade_needed.insert(user, false);
        self.writes.insert(user);
        Ok(())
    }
    pub fn pending_write_requests(&self) -> Vec<i32> {
        self.writes.iter().copied().collect()
    }
    pub(crate) fn acknowledge_write(&mut self, user: i32) {
        self.writes.remove(&user);
    }
    pub(crate) fn flush_with(
        &mut self,
        mut write: impl FnMut(i32, &Self) -> Result<u32, super::WriteError>,
    ) -> Result<Vec<u32>, FlushError> {
        let mut completed = Vec::new();
        for user in self.pending_write_requests() {
            match write(user, self) {
                Ok(id) => {
                    self.acknowledge_write(user);
                    completed.push(id);
                }
                Err(error) => {
                    return Err(FlushError {
                        user,
                        completed,
                        error,
                    });
                }
            }
        }
        Ok(completed)
    }
    pub fn remove_user(&mut self, user: i32) {
        self.versions.remove(&user);
        self.fingerprints.remove(&user);
        self.upgrade_needed.remove(&user);
        self.writes.remove(&user);
    }
}

/// Earlier users may have committed even when a later requested write failed.
#[derive(Debug)]
pub struct FlushError {
    pub user: i32,
    pub completed: Vec<u32>,
    pub error: super::WriteError,
}
impl std::fmt::Display for FlushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "runtime user {}: {}", self.user, self.error)
    }
}
impl std::error::Error for FlushError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::legacy_permissions::UserMetadata;

    #[test]
    fn flush_retains_committed_failures_and_later_requests_until_success() {
        let mut state = State::default();
        state.set_version(0, 7);
        state.set_version(10, 9);
        state.set_version(11, 12);
        let error = state
            .flush_with(|user, meta| {
                assert_eq!(meta.version(user), if user == 0 { 7 } else { 9 });
                if user == 10 {
                    Err(super::super::WriteError {
                        committed: true,
                        message: "reserve failure".into(),
                    })
                } else {
                    Ok(user as u32)
                }
            })
            .unwrap_err();
        assert_eq!(error.completed, [0]);
        assert_eq!(error.user, 10);
        assert!(error.error.committed);
        assert_eq!(state.pending_write_requests(), [10, 11]);
        assert_eq!(
            state.flush_with(|user, _| Ok(user as u32)).unwrap(),
            [10, 11]
        );
        assert!(state.pending_write_requests().is_empty());
    }

    #[test]
    fn sparse_metadata_restores_null_presence_and_controller_upgrade_transitions() {
        let mut state = State::default();
        assert_eq!(state.version(0), 0);
        assert!(state.upgrade_needed(0));
        assert!(state.update_fingerprint(0).is_err());
        assert!(state.pending_write_requests().is_empty());
        let metadata = Metadata {
            install_permissions_fixed: Default::default(),
            users: BTreeMap::from([
                (
                    0,
                    UserMetadata {
                        version: Some(-1),
                        fingerprint: None,
                        rewrite_requested: false,
                    },
                ),
                (
                    10,
                    UserMetadata {
                        version: Some(7),
                        fingerprint: Some("part?pc_version=3".into()),
                        rewrite_requested: false,
                    },
                ),
                (
                    11,
                    UserMetadata {
                        version: None,
                        fingerprint: None,
                        rewrite_requested: true,
                    },
                ),
            ]),
        };
        state.restore(&metadata);
        state.set_controller_version("part", 3);
        assert_eq!(state.version(0), -1);
        assert!(state.upgrade_needed(0));
        assert!(!state.upgrade_needed(10));
        assert!(state.upgrade_needed(11));
        assert_eq!(state.pending_write_requests(), [11]);
        state.acknowledge_write(11);
        state.set_version(10, 9);
        state.update_fingerprint(0).unwrap();
        assert_eq!(state.fingerprint(0), Some("part?pc_version=3"));
        assert!(!state.upgrade_needed(0));
        assert_eq!(state.pending_write_requests(), [0, 10]);
        state.set_controller_version("part", 4);
        assert!(state.upgrade_needed(0));
        assert!(state.upgrade_needed(10));
        state.remove_user(0);
        assert_eq!(state.version(0), 0);
        assert_eq!(state.fingerprint(0), None);
        assert!(state.upgrade_needed(0));
    }
}
