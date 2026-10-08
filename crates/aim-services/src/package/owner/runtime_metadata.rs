//! Settings.RuntimePermissionPersistence metadata, Android 16 r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::legacy_permissions::Metadata;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

mod schedule;
mod internal;
mod user;
pub mod worker;

/// Constructor sparse defaults are version 0, null fingerprint and upgrade=true.
/// Write requests must be consumed by the runtime persistence scheduler.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    versions: BTreeMap<i32, i32>,
    creation_inodes:BTreeMap<u32,aim_storage::guest_inode::GuestInode>,
    fingerprints: BTreeMap<i32, Option<String>>,
    upgrade_needed: BTreeMap<i32, bool>,
    extended_fingerprint: Option<String>,
    writes: BTreeSet<i32>,
    schedule: schedule::Schedule,
    wake: Option<worker::Wake>,
}

fn validate_creation_inode(user:u32,inode:aim_storage::guest_inode::GuestInode)->Result<(),String>{
    if user>i32::MAX as u32||inode.uid.is_none()||inode.gid.is_none()||inode.mode.is_none_or(|mode|mode&!0o7777!=0){return Err("runtime permission creation metadata is incomplete".into());}Ok(())
}

impl State {
    /// Bind observations supplied by the actual original creators. Re-observing
    /// a constructor entry verifies identity; it never drops later users.
    pub fn bind_creation_metadata(&mut self,inodes:&BTreeMap<u32,aim_storage::guest_inode::GuestInode>)->Result<(),String>{
        for (user,inode) in inodes {
            validate_creation_inode(*user,*inode)?;
            if self.creation_inodes.get(user).is_some_and(|existing|existing!=inode){return Err("runtime creation owner changed without user handoff".into());}
        }
        self.creation_inodes.extend(inodes.iter().map(|(user,inode)|(*user,*inode)));Ok(())
    }
    /// Called only after original user app-data creation has completed and the
    /// native file owner observed this user's actual file/probe metadata.
    pub fn publish_user_creation_inode(&mut self,user:u32,inode:aim_storage::guest_inode::GuestInode)->Result<(),String>{
        validate_creation_inode(user,inode)?;
        if self.creation_inodes.contains_key(&user){return Err("runtime creation owner already handed off".into());}
        self.creation_inodes.insert(user,inode);
        if let Some(wake)=&self.wake{wake.notify();}
        Ok(())
    }
    pub fn creation_inode(&self,user:u32)->Option<aim_storage::guest_inode::GuestInode>{self.creation_inodes.get(&user).copied()}
    pub fn creation_metadata(&self)->BTreeMap<u32,aim_storage::guest_inode::GuestInode>{self.creation_inodes.clone()}
    pub fn restore(&mut self, metadata: &Metadata) {
        for (&user, state) in &metadata.users {
            if let Some(version) = state.version {
                self.versions.insert(user, version);
                self.fingerprints.insert(user, state.fingerprint.clone());
            }
            if state.rewrite_requested {
                self.request_write(user);
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
        self.request_write(user);
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
        self.request_write(user);
        Ok(())
    }
    fn request_write(&mut self, user: i32) {
        self.writes.insert(user);
        let delay = Duration::from_millis(700 + u64::from(unsafe { libc::arc4random_uniform(600) }));
        self.schedule.request(user, Instant::now(), delay);
        if let Some(wake)=&self.wake { wake.notify(); }
    }
    pub fn next_write_deadline(&self) -> Option<Instant> {
        self.schedule.next()
    }
    pub fn due_write_requests(&self, now: Instant) -> Vec<i32> {
        self.schedule.due(now)
    }
    pub fn pending_write_requests(&self) -> Vec<i32> {
        self.writes.iter().copied().collect()
    }
    pub(crate) fn acknowledge_write(&mut self, user: i32) {
        self.writes.remove(&user);
        self.schedule.remove(user);
    }
    pub(crate) fn flush_with(
        &mut self,
        mut write: impl FnMut(i32, &Self) -> Result<u32, super::WriteError>,
    ) -> Result<Vec<u32>, FlushError> {
        self.flush_selected(self.pending_write_requests(), &mut write)
    }
    pub(crate) fn flush_due_with(
        &mut self,
        now: Instant,
        write: impl FnMut(i32, &Self) -> Result<u32, super::WriteError>,
    ) -> Result<Vec<u32>, FlushError> {
        self.flush_selected(self.due_write_requests(now), write)
    }
    fn flush_selected(
        &mut self,
        users: Vec<i32>,
        mut write: impl FnMut(i32, &Self) -> Result<u32, super::WriteError>,
    ) -> Result<Vec<u32>, FlushError> {
        let mut completed = Vec::new();
        for user in users {
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
        if let Ok(id)=u32::try_from(user){self.creation_inodes.remove(&id);}
        self.versions.remove(&user);
        self.fingerprints.remove(&user);
        self.upgrade_needed.remove(&user);
        self.writes.remove(&user);
        self.schedule.remove(user);
    }
}

impl super::Store {
    /// Restore saved migration roles and their runtime metadata as one candidate.
    /// First-boot false continuations must keep constructor metadata instead.
    pub(crate) fn restore_runtime_permission_owners(
        &self,
        scan: &mut crate::package::scan::SigningScan,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<State, super::WriteError> {
        if !self.settings_present || !self.unread_restrictions.is_empty() {
            return Err(super::WriteError::before(
                "runtime restoration was skipped by the boot reader",
            ));
        }
        let mut candidate = scan.clone();
        candidate
            .restore_owned_legacy_permissions_from_data(&self.data,&self.state,config,&self.settings_document)
            .map_err(super::WriteError::before)?;
        let restored = candidate
            .legacy_restoration_metadata()
            .map_err(super::WriteError::before)?
            .ok_or_else(|| {
                super::WriteError::before("runtime restoration metadata is unavailable")
            })?;
        let expected = self
            .state
            .users
            .iter()
            .map(|(id, _)| i32::try_from(*id).map_err(super::WriteError::before))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if restored.users.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(super::WriteError::before(
                "runtime restored user inventory differs",
            ));
        }
        let mut metadata = State::default();
        metadata.restore(restored);
        *scan = candidate;
        Ok(metadata)
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
    fn store_runtime_restore_carries_versions_and_rewrites_without_partial_scan_changes() {
        use crate::package::{
            owner::{Store, tests::Data},
            scan::SigningScan,
            system_config::SystemConfig,
        };
        let data = Data::new();
        data.settings();
        let dir = data.0.join("misc_de/0/apexdata/com.android.permission");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("runtime-permissions.xml"),b"<runtime-permissions version='7' fingerprint='saved'><package name='example.app'/></runtime-permissions>").unwrap();
        let store = Store::open(&data.0, &[0, 10]).unwrap().unwrap();
        let mut scan =
            SigningScan::new(&SystemConfig::default(), &store.state.settings, 36).unwrap();
        let metadata = store
            .restore_runtime_permission_owners(&mut scan, &SystemConfig::default())
            .unwrap();
        assert_eq!(metadata.version(0), 7);
        assert_eq!(metadata.fingerprint(0), Some("saved"));
        assert_eq!(metadata.version(10), 0);
        assert_eq!(metadata.pending_write_requests(), [10]);
        assert!(scan.legacy_restoration_metadata().unwrap().is_some());
        let before = scan.clone();
        scan.settings.packages[0].app_id += 1;
        let changed = scan.clone();
        assert!(
            store
                .restore_runtime_permission_owners(&mut scan, &SystemConfig::default())
                .is_err()
        );
        assert_eq!(scan, changed);
        assert_ne!(scan, before);
        let fresh = Data::new();
        let mut settings = crate::package::settings::Settings::default();
        let current = crate::package::settings::Version {
            sdk_version: 36,
            database_version: 3,
            ..Default::default()
        };
        let (store, _) = super::super::recovery::Plan::inspect(&fresh.0)
            .unwrap()
            .recover_boot(&[0], &mut settings, &current, |_, _| panic!())
            .unwrap();
        let mut scan = SigningScan::new(&SystemConfig::default(), &settings, 36).unwrap();
        let before = scan.clone();
        assert!(
            store
                .restore_runtime_permission_owners(&mut scan, &SystemConfig::default())
                .is_err()
        );
        assert_eq!(scan, before);
    }

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

/// Settings metadata is a live original owner shared by retained Computers.
pub struct Queries(pub Box<dyn Fn(i32) -> Result<bool, String> + Send + Sync>);
impl std::fmt::Debug for Queries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("RuntimePermissionQueries") }
}
impl PartialEq for Queries { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
