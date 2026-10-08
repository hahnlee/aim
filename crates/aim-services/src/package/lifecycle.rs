//! Native PMS lifecycle state, from PackageManagerService at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{settings::Settings, system_config::Properties};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
};

pub type CeStorage = Box<dyn Fn(i32) -> Result<bool, String> + Send + Sync>;
/// Atomically publish a new scan/query generation with this complete freezer map.
/// Called without the lifecycle state mutex. An error must leave publication unchanged.
pub type FrozenPublisher =
    Arc<dyn Fn(u64, BTreeMap<String, i32>) -> Result<(), String> + Send + Sync>;

/// A bootstrap lifetime, shared by native public queries and captured facades.
/// Construct before forceCurrent overwrites the restored internal VersionInfo.
pub struct Owner {
    first_boot: bool,
    upgrade: bool,
    prior_sdk: i32,
    properties: Properties,
    ce_storage: CeStorage,
    state: Mutex<Live>,
    publication: Mutex<Option<FrozenPublisher>>,
}
#[derive(Default)]
struct Live {
    safe_mode: bool,
    system_ready: bool,
    boot_dexopt_start_nanos:Option<i64>,
    frozen: BTreeMap<String, usize>,
    revision: u64,
    publication_error: Option<String>,
}
impl fmt::Debug for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackageLifecycle")
            .field("first_boot", &self.first_boot)
            .field("upgrade", &self.upgrade)
            .field("prior_sdk", &self.prior_sdk)
            .finish_non_exhaustive()
    }
}
impl PartialEq for Owner {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Owner {
    pub fn from_settings(
        settings: &Settings,
        settings_read_succeeded: bool,
        current_partition_fingerprint: &str,
        properties: Properties,
        ce_storage: CeStorage,
    ) -> Result<Arc<Self>, String> {
        let version = settings
            .versions
            .iter()
            .find(|v| v.volume_uuid.is_none())
            .ok_or("missing original internal VersionInfo lifecycle owner")?;
        let upgrade = version.fingerprint.as_deref() != Some(current_partition_fingerprint);
        Ok(Arc::new(Self {
            first_boot: !settings_read_succeeded,
            upgrade,
            prior_sdk: if upgrade { version.sdk_version } else { -1 },
            properties,
            ce_storage,
            state: Mutex::new(Live::default()),
            publication: Mutex::new(None),
        }))
    }
    pub fn first_boot(&self) -> bool {
        self.first_boot
    }
    pub fn prior_sdk_version(&self) -> i32 {
        self.prior_sdk
    }
    pub fn device_upgrading(&self) -> bool {
        self.upgrade
            || (self.properties)("persist.pm.mock-upgrade")
                .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "y" | "yes" | "on"))
    }
    pub fn is_upgrading_from_lower_than(&self, sdk: i32) -> bool {
        self.upgrade && self.prior_sdk < sdk
    }
    pub fn is_system_ready(&self) -> bool {
        self.state.lock().unwrap().system_ready
    }
    pub fn safe_mode(&self) -> bool {
        self.state.lock().unwrap().safe_mode
    }
    pub fn enter_safe_mode(&self, uid: i32) -> Result<(), String> {
        if uid != 0 && uid != 1000 {
            return Err("Only the system can request entering safe mode".into());
        }
        let mut state = self.state.lock().unwrap();
        if !state.system_ready {
            state.safe_mode = true;
        }
        Ok(())
    }
    /// Invoke at original systemReady's mSystemReady assignment, after release
    /// of compressed blocks and before its observers and service callbacks.
    pub fn system_ready(&self, uid: i32) -> Result<(), String> {
        if uid != 0 && uid != 1000 {
            return Err("Only the system can claim the system is ready".into());
        }
        self.state.lock().unwrap().system_ready = true;
        Ok(())
    }
    /// StorageManager.isCeStorageUnlocked is independent of UM unlock progress.
    /// Supplied by the actual Java System.nanoTime call immediately before ART onBoot.
    pub fn note_boot_dexopt_start_nanos(&self,start:i64)->Result<(),String>{
        let mut state=self.state.lock().unwrap();
        state.boot_dexopt_start_nanos=Some(start);Ok(())
    }
    pub fn boot_dexopt_start_nanos(&self)->Result<i64,String>{
        self.state.lock().unwrap().boot_dexopt_start_nanos.ok_or_else(||"actual Java boot dexopt start has not been recorded".into())
    }
    pub fn ce_storage_unlocked(&self, user: i32) -> Result<bool, String> {
        (self.ce_storage)(user)
    }
    pub fn is_frozen(&self, name: &str) -> bool {
        self.state.lock().unwrap().frozen.contains_key(name)
    }
    pub fn checked_is_frozen(&self, name: &str) -> Result<bool, String> {
        let state = self.state.lock().unwrap();
        if let Some(error) = &state.publication_error { return Err(error.clone()); }
        Ok(state.frozen.contains_key(name))
    }

    pub fn frozen_snapshot(&self) -> Result<BTreeMap<String, i32>, String> {
        let state = self.state.lock().unwrap();
        if let Some(error) = &state.publication_error {
            return Err(error.clone());
        }
        state
            .frozen
            .iter()
            .map(|(name, count)| {
                i32::try_from(*count)
                    .map(|count| (name.clone(), count))
                    .map_err(|_| "frozen package reference count exceeds original int range".into())
            })
            .collect()
    }
    pub fn check_frozen_publication(&self) -> Result<(), String> {
        self.state
            .lock()
            .unwrap()
            .publication_error
            .clone()
            .map_or(Ok(()), Err)
    }
    /// Claim the native PackageFreezer registry before invoking its real app
    /// kill/wait owner. The guard releases the claim on every failure path.
    pub fn install_frozen_publisher(&self, publisher: FrozenPublisher) -> Result<(), String> {
        let mut publication = self.publication.lock().unwrap();
        if publication.is_some() {
            return Err("freezer publication owner already installed".into());
        }
        let (revision, snapshot) = {
            let state = self.state.lock().unwrap();
            if let Some(error) = &state.publication_error {
                return Err(error.clone());
            }
            (state.revision, frozen_map(&state)?)
        };
        publisher(revision, snapshot)?;
        *publication = Some(publisher);
        Ok(())
    }
    pub fn freeze(self: &Arc<Self>, name: String) -> Result<Freeze, String> {
        self.change_frozen(&name, true)?;
        Ok(Freeze {
            owner: self.clone(),
            name,
            active: true,
        })
    }
    fn change_frozen(&self, name: &str, acquire: bool) -> Result<(), String> {
        // Serialize notifications, including nested releases, without holding the
        // state mutex across root's generation publication callback.
        let publication = self.publication.lock().unwrap();
        let publisher = publication
            .as_ref()
            .ok_or("freezer publication owner unavailable")?;
        let (old, revision, snapshot) = {
            let mut state = self.state.lock().unwrap();
            if let Some(error) = state.publication_error.clone() {
                if !acquire {
                    if let Some(count) = state.frozen.get_mut(name) {
                        *count -= 1;
                        if *count == 0 {
                            state.frozen.remove(name);
                        }
                    }
                }
                return Err(error);
            }
            let old = state.frozen.get(name).copied();
            let count = if acquire {
                old.unwrap_or(0)
                    .checked_add(1)
                    .filter(|count| *count <= i32::MAX as usize)
                    .ok_or("frozen package reference count exceeds original int range")?
            } else {
                old.ok_or("owned frozen package vanished")?
                    .checked_sub(1)
                    .ok_or("invalid frozen package reference count")?
            };
            let revision = state
                .revision
                .checked_add(1)
                .ok_or("freezer revision exhausted")?;
            if count == 0 {
                state.frozen.remove(name);
            } else {
                state.frozen.insert(name.to_owned(), count);
            }
            (old, revision, frozen_map(&state)?)
        };
        if let Err(error) = publisher(revision, snapshot) {
            let mut state = self.state.lock().unwrap();
            // Acquiring failed: no guard exists, so restore its unclaimed count.
            // Releasing failed: the guard is ending, so retain the released live
            // count and poison publication until the owner is replaced.
            if acquire {
                if let Some(count) = old {
                    state.frozen.insert(name.to_owned(), count);
                } else {
                    state.frozen.remove(name);
                }
            }
            state.publication_error =
                Some(format!("freezer generation publication failed: {error}"));
            return Err(state.publication_error.clone().unwrap());
        }
        self.state.lock().unwrap().revision = revision;
        Ok(())
    }
}

fn frozen_map(state: &Live) -> Result<BTreeMap<String, i32>, String> {
    state
        .frozen
        .iter()
        .map(|(name, count)| {
            i32::try_from(*count)
                .map(|count| (name.clone(), count))
                .map_err(|_| "frozen package reference count exceeds original int range".into())
        })
        .collect()
}

pub struct Freeze {
    owner: Arc<Owner>,
    name: String,
    active: bool,
}
impl Freeze {
    /// Explicit close lets successful operations return a publication failure.
    pub fn close(mut self) -> Result<(), String> {
        self.active = false;
        self.owner.change_frozen(&self.name, false)
    }
}
impl Drop for Freeze {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            if let Err(error) = self.owner.change_frozen(&self.name, false) {
                // Drop cannot return an error; make it a persistent owner fault.
                self.owner.state.lock().unwrap().publication_error = Some(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::settings::Version;
    use super::*;
    fn owner(read: bool, old: Option<&str>, sdk: i32) -> Arc<Owner> {
        Owner::from_settings(
            &Settings {
                versions: vec![Version {
                    fingerprint: old.map(str::to_owned),
                    sdk_version: sdk,
                    ..Default::default()
                }],
                ..Default::default()
            },
            read,
            "current",
            Box::new(|_| None),
            Box::new(|_| Err("missing CE storage owner".into())),
        )
        .unwrap()
    }
    #[test]
    fn restored_versions_define_first_boot_upgrade_and_prior_sdk_independently() {
        for read in [false, true] {
            for fingerprint in [None, Some("old"), Some("current")] {
                let owner = owner(read, fingerprint, 35);
                assert_eq!(owner.first_boot(), !read);
                assert_eq!(owner.device_upgrading(), fingerprint != Some("current"));
                assert_eq!(
                    owner.prior_sdk_version(),
                    if fingerprint == Some("current") {
                        -1
                    } else {
                        35
                    }
                );
            }
        }
        assert!(
            Owner::from_settings(
                &Settings::default(),
                true,
                "current",
                Box::new(|_| None),
                Box::new(|_| Ok(false))
            )
            .is_err()
        );
    }
    #[test]
    fn mock_upgrade_property_is_live_and_uses_android_boolean_values() {
        let value = Arc::new(Mutex::new(None));
        let property = value.clone();
        let owner = Owner::from_settings(
            &Settings {
                versions: vec![Version {
                    fingerprint: Some("current".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            true,
            "current",
            Box::new(move |name| {
                assert_eq!(name, "persist.pm.mock-upgrade");
                property.lock().unwrap().clone()
            }),
            Box::new(|user| Ok(user == 10)),
        )
        .unwrap();
        for (input, expected) in [
            (None, false),
            (Some("true"), true),
            (Some("1"), true),
            (Some("yes"), true),
            (Some("on"), true),
            (Some("y"), true),
            (Some("0"), false),
            (Some("TRUE"), false),
            (Some("invalid"), false),
        ] {
            *value.lock().unwrap() = input.map(str::to_owned);
            assert_eq!(owner.device_upgrading(), expected);
        }
        assert!(owner.ce_storage_unlocked(10).unwrap());
        assert!(!owner.ce_storage_unlocked(0).unwrap());
        assert_eq!(owner.prior_sdk_version(), -1);
    }
    #[test]
    fn safe_mode_only_enters_before_ready_and_checks_actual_uid() {
        let owner = owner(true, Some("current"), 36);
        assert!(owner.enter_safe_mode(101000).is_err());
        assert!(owner.system_ready(101000).is_err());
        assert!(!owner.safe_mode());
        owner.enter_safe_mode(0).unwrap();
        owner.system_ready(1000).unwrap();
        assert!(owner.safe_mode());
        let late = self::owner(true, Some("current"), 36);
        late.system_ready(0).unwrap();
        late.enter_safe_mode(1000).unwrap();
        assert!(!late.safe_mode());
    }
    #[test]
    fn nested_freezer_claims_and_missing_ce_owner_do_not_fake_startability() {
        let owner = owner(true, Some("current"), 36);
        assert!(owner.freeze("p".into()).is_err());
        assert!(!owner.is_frozen("p"));
        let generations = Arc::new(Mutex::new(Vec::new()));
        let published = generations.clone();
        owner.install_frozen_publisher(Arc::new(move |revision, frozen| {
            published.lock().unwrap().push((revision, frozen));
            Ok(())
        })).unwrap();
        let a = owner.freeze("p".into()).unwrap();
        let b = owner.freeze("p".into()).unwrap();
        assert!(owner.is_frozen("p"));
        assert_eq!(owner.frozen_snapshot().unwrap()["p"], 2);
        a.close().unwrap();
        assert!(owner.is_frozen("p"));
        assert_eq!(owner.frozen_snapshot().unwrap()["p"], 1);
        drop(b);
        assert!(!owner.is_frozen("p"));
        assert_eq!(*generations.lock().unwrap(), vec![
            (0, BTreeMap::new()),
            (1, BTreeMap::from([("p".into(), 1)])),
            (2, BTreeMap::from([("p".into(), 2)])),
            (3, BTreeMap::from([("p".into(), 1)])),
            (4, BTreeMap::new()),
        ]);
        assert_eq!(
            owner.ce_storage_unlocked(0),
            Err("missing CE storage owner".into())
        );
    }
}
