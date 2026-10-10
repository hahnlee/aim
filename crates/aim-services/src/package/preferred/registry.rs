//! Live preferred resolver object identities, supplied by their actual runtime
//! owner. Stages allocate replacement identities before the disk owner's lock.
use super::{Edit, Preferred, Selection};
use crate::package::apps_filter::NotModelled;
use crate::package::query::preferred::{Owner, Snapshot};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub trait IdentityLease: Send + Sync {
    fn typed_record(&self) -> Option<(i32, aim_binder_host::parcel::Binder)>;
}
impl IdentityLease for aim_binder_host::local::Strong {
    fn typed_record(&self) -> Option<(i32, aim_binder_host::parcel::Binder)> {
        None
    }
}
#[cfg(test)]
impl IdentityLease for () {
    fn typed_record(&self) -> Option<(i32, aim_binder_host::parcel::Binder)> {
        None
    }
}

pub struct Identity {
    pub hash: i32,
    /// Keeps the original runtime object alive for the registry lifetime.
    pub lease: Arc<dyn IdentityLease>,
}
impl Clone for Identity {
    fn clone(&self) -> Self {
        Self {
            hash: self.hash,
            lease: self.lease.clone(),
        }
    }
}
pub trait IdentityProvider: Send + Sync {
    fn allocate(&self, record: super::records::Record<'_>) -> Result<Identity, String>;
}
#[derive(Clone)]
struct User {
    snapshot: Snapshot,
    preferred: Vec<Identity>,
    persistent: Vec<Identity>,
    cross_profile: Vec<Identity>,
}
struct State {
    revoked: bool,
    generation: u64,
    users: BTreeMap<i32, User>,
}
pub struct Registry {
    identities: Arc<dyn IdentityProvider>,
    state: Arc<Mutex<State>>,
}
/// Disk persistence consumes this staged snapshot and the changed user. Publish
/// only after its file commit succeeds and the source generation is current.
pub struct Stage {
    pub user: i32,
    pub before_generation: u64,
    pub preferred: Arc<Preferred>,
    pub preferred_order: Vec<usize>,
    pub persistent_order: Vec<usize>,
    pub cross_profile_order: Vec<usize>,
    replacement: User,
}
fn order(identities: &[Identity]) -> Vec<usize> {
    let mut order: Vec<_> = (0..identities.len()).collect();
    order.sort_by_key(|&index| identities[index].hash);
    order
}
impl Registry {
    pub fn new(identities: Arc<dyn IdentityProvider>) -> Self {
        Self {
            identities,
            state: Arc::new(Mutex::new(State {
                revoked: false,
                generation: 0,
                users: BTreeMap::new(),
            })),
        }
    }
    pub fn from_store(
        identities: Arc<dyn IdentityProvider>,
        store: &crate::package::owner::Store,
    ) -> Result<Self, String> {
        let registry = Self::new(identities);
        for (user, preferred) in store.native_preferred_users()? {
            registry.insert_user(user, preferred)?;
        }
        Ok(registry)
    }

    /// Initial native owner state from accepted Settings parsing. The provider
    /// creates real runtime identities; XML item order is never used as a hash.
    pub fn insert_user(&self, user: i32, preferred: Preferred) -> Result<(), String> {
        let identities = preferred
            .preferred
            .entries()
            .iter()
            .map(|record| {
                self.identities
                    .allocate(super::records::Record::Preferred(record))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let persistent = preferred
            .persistent
            .entries()
            .iter()
            .map(|record| {
                self.identities
                    .allocate(super::records::Record::Persistent(record))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cross_profile = preferred
            .cross_profile
            .entries()
            .iter()
            .map(|record| {
                self.identities
                    .allocate(super::records::Record::CrossProfile(record))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut state = self.state.lock().unwrap();
        if state.revoked {
            return Err("preferred registry is revoked".into());
        }
        if state.users.contains_key(&user) {
            return Err("preferred user already registered".into());
        }
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or("preferred generation exhausted")?;
        let snapshot = Snapshot {
            generation: state.generation,
            state: Arc::new(preferred),
            preferred_order: order(&identities),
        };
        state.users.insert(
            user,
            User {
                snapshot,
                preferred: identities,
                persistent,
                cross_profile,
            },
        );
        Ok(())
    }
    /// Capture under the System publication gate alongside package state.
    /// The resulting owner always serves this exact generation, until revoked.
    pub fn capture(&self) -> Result<Captured, String> {
        let state = self.state.lock().unwrap();
        if state.revoked {
            return Err("preferred registry is revoked".into());
        }
        Ok(Captured {
            owner: self.state.clone(),
            users: state
                .users
                .iter()
                .map(|(&user, record)| (user, record.clone()))
                .collect(),
        })
    }

    pub(crate) fn user_ids(&self) -> Result<Vec<i32>, String> {
        let state = self.state.lock().unwrap();
        if state.revoked { return Err("preferred registry is revoked".into()); }
        Ok(state.users.keys().copied().collect())
    }

    pub fn prepare_selection(
        &self,
        user: i32,
        generation: u64,
        selection: &Selection,
    ) -> Result<Option<Stage>, String> {
        if selection.edits.is_empty() {
            return Ok(None);
        }
        let old = {
            let state = self.state.lock().unwrap();
            if state.revoked {
                return Err("preferred registry is revoked".into());
            }
            let old = state.users.get(&user).ok_or("preferred user missing")?;
            if old.snapshot.generation != generation {
                return Err("preferred selection snapshot changed".into());
            }
            old.clone()
        };
        let mut preferred = old.snapshot.state.as_ref().clone();
        preferred
            .apply_selection(selection)
            .map_err(str::to_owned)?;
        let removed = |index| {
            selection.edits.iter().any(|edit| match edit {
                Edit::Remove { registration } | Edit::Replace { registration, .. } => {
                    *registration == index
                }
            })
        };
        let mut identities: Vec<_> = old
            .preferred
            .iter()
            .enumerate()
            .filter(|(index, _)| !removed(*index))
            .map(|(_, identity)| identity.clone())
            .collect();
        for edit in &selection.edits {
            if let Edit::Replace { activity, .. } = edit {
                identities.push(
                    self.identities
                        .allocate(super::records::Record::Preferred(activity))?,
                );
            }
        }
        let preferred = Arc::new(preferred);
        let preferred_order = order(&identities);
        let replacement = User {
            snapshot: Snapshot {
                generation,
                state: preferred.clone(),
                preferred_order: preferred_order.clone(),
            },
            preferred: identities,
            persistent: old.persistent,
            cross_profile: old.cross_profile,
        };
        Ok(Some(Stage {
            user,
            before_generation: generation,
            preferred,
            preferred_order,
            persistent_order: order(&replacement.persistent),
            cross_profile_order: order(&replacement.cross_profile),
            replacement,
        }))
    }
    pub fn prepare_cross_profile_target_removal(
        &self,
        user: i32,
        target: i32,
    ) -> Result<Option<Stage>, String> {
        let old = {
            let state = self.state.lock().unwrap();
            if state.revoked {
                return Err("preferred registry is revoked".into());
            }
            let Some(old) = state.users.get(&user) else {
                return Ok(None);
            };
            old.clone()
        };
        let mut preferred = old.snapshot.state.as_ref().clone();
        if !preferred.remove_cross_profile_target(target) {
            return Ok(None);
        }
        let identities = old
            .snapshot
            .state
            .cross_profile
            .entries()
            .iter()
            .zip(&old.cross_profile)
            .filter(|(filter, _)| filter.target_user_id != target)
            .map(|(_, identity)| identity.clone())
            .collect();
        let preferred = Arc::new(preferred);
        let replacement = User {
            snapshot: Snapshot {
                generation: old.snapshot.generation,
                state: preferred.clone(),
                preferred_order: old.snapshot.preferred_order.clone(),
            },
            preferred: old.preferred,
            persistent: old.persistent,
            cross_profile: identities,
        };
        Ok(Some(Stage {
            user,
            before_generation: replacement.snapshot.generation,
            preferred,
            preferred_order: replacement.snapshot.preferred_order.clone(),
            persistent_order: order(&replacement.persistent),
            cross_profile_order: order(&replacement.cross_profile),
            replacement,
        }))
    }

    /// User cleanup calls this under its System publication gate, after that
    /// user's disk state has been removed. Captured snapshots keep their leases.
    pub fn remove_user(&self, user: i32) -> Result<bool, String> {
        let removed = {
            let mut state = self.state.lock().unwrap();
            if state.revoked {
                return Err("preferred registry is revoked".into());
            }
            if !state.users.contains_key(&user) {
                return Ok(false);
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or("preferred generation exhausted")?;
            state.users.remove(&user)
        };
        drop(removed);
        Ok(true)
    }

    /// Preview under the System publication gate, after all identity allocation.
    /// This is the exact captured generation publish(stage) will install if no
    /// other publication is permitted between preview, disk commit and publish.
    pub fn preview(&self, stage: &Stage) -> Result<Captured, String> {
        let state = self.state.lock().unwrap();
        if state.revoked
            || state
                .users
                .get(&stage.user)
                .is_none_or(|user| user.snapshot.generation != stage.before_generation)
        {
            return Err("preferred stage no longer current".into());
        }
        let generation = state
            .generation
            .checked_add(1)
            .ok_or("preferred generation exhausted")?;
        let mut users = state.users.clone();
        let mut replacement = stage.replacement.clone();
        replacement.snapshot.generation = generation;
        users.insert(stage.user, replacement);
        Ok(Captured {
            owner: self.state.clone(),
            users,
        })
    }

    /// The caller holds its persistence/publication gate. Validate *before* a
    /// disk write too, then publish under that same gate after the write.
    pub fn validate(&self, stage: &Stage) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        if state.revoked
            || state
                .users
                .get(&stage.user)
                .is_none_or(|user| user.snapshot.generation != stage.before_generation)
        {
            return Err("preferred stage no longer current".into());
        }
        Ok(())
    }
    pub fn publish(&self, mut stage: Stage) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.revoked
            || state
                .users
                .get(&stage.user)
                .is_none_or(|user| user.snapshot.generation != stage.before_generation)
        {
            return Err("preferred stage no longer current".into());
        }
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or("preferred generation exhausted")?;
        stage.replacement.snapshot.generation = state.generation;
        state.users.insert(stage.user, stage.replacement);
        Ok(())
    }
    pub fn revoke(&self) {
        let users = {
            let mut state = self.state.lock().unwrap();
            state.revoked = true;
            std::mem::take(&mut state.users)
        };
        drop(users);
    }
}
impl Owner for Registry {
    fn snapshot(&self, user: i32) -> Result<Option<Snapshot>, NotModelled> {
        let state = self.state.lock().unwrap();
        if state.revoked {
            return Err(NotModelled("preferred registry revoked"));
        }
        Ok(state.users.get(&user).map(|user| user.snapshot.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        intent_filter::IntentFilter,
        preferred::{PreferredActivity, unflatten},
    };
    struct Provider(Mutex<std::collections::VecDeque<i32>>);
    impl IdentityProvider for Provider {
        fn allocate(&self, _record: super::super::records::Record<'_>) -> Result<Identity, String> {
            Ok(Identity {
                hash: self
                    .0
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or("identity owner exhausted")?,
                lease: Arc::new(()),
            })
        }
    }
    fn activity(name: &str) -> PreferredActivity {
        let mut filter = IntentFilter::default();
        filter.add_action("view");
        PreferredActivity::new(
            filter,
            0x100000,
            None,
            unflatten(&format!("{name}/.Main")).unwrap(),
            true,
        )
    }
    #[test]
    fn removal_clear_publishes_selected_user_and_preserves_other_resolver_families() {
        let registry = Registry::new(Arc::new(Provider(Mutex::new((1..=8).collect()))));
        let mut initial = Preferred::default();
        initial.add_preferred(activity("removed"), false);
        initial.add_preferred(activity("other"), false);
        let mut last = activity("removed"); last.always = false; last.filter.add_action("last");
        initial.add_preferred(last, false);
        initial.persistent_resolver_present = true;
        initial.persistent.add(super::super::PersistentPreferredActivity {
            filter: activity("removed").filter,
            component: unflatten("removed/.Main").unwrap(), set_by_dpm: true,
        });
        registry.insert_user(0, initial.clone()).unwrap();
        registry.insert_user(10, initial).unwrap();
        let before = registry.capture().unwrap();
        let snapshot = registry.snapshot(0).unwrap().unwrap();
        let stage = registry.prepare_mutation(0, snapshot.generation,
            &super::super::Mutation::Clear { package: Some("removed".into()) }).unwrap().stage.unwrap();
        let bytes = aim_android_xml::abx::write(&stage.preferred.preferred_document(&stage.preferred_order,true).unwrap()).unwrap();
        let restored = Preferred::parse(Some(&bytes), None);
        assert_eq!(restored.preferred.entries().len(), 2);
        registry.publish(stage).unwrap();
        let current = registry.snapshot(0).unwrap().unwrap();
        assert_eq!(current.state.preferred.entries().len(), 2);
        assert!(current.state.preferred.entries().iter().any(|entry| entry.component.package=="removed" && !entry.always));
        assert_eq!(current.state.persistent.entries().len(), 1);
        assert_eq!(registry.snapshot(10).unwrap().unwrap().state.preferred.entries().len(), 3);
        assert_eq!(before.snapshot(0).unwrap().unwrap().state.preferred.entries().len(), 3);
        for stage in registry.prepare_clear_all(Some("removed")).unwrap() { registry.publish(stage).unwrap(); }
        assert_eq!(registry.snapshot(10).unwrap().unwrap().state.preferred.entries().len(), 2);
        assert_eq!(registry.snapshot(10).unwrap().unwrap().state.persistent.entries().len(), 1);
    }

    #[test]
    fn signed_identity_collisions_stale_stages_and_revocation() {
        let registry = Registry::new(Arc::new(Provider(Mutex::new([-1, 33, -1, -7, 44].into()))));
        let mut preferred = Preferred::default();
        for name in ["one", "two", "three"] {
            preferred.add_preferred(activity(name), false);
        }
        registry.insert_user(0, preferred).unwrap();
        let captured = registry.capture().unwrap();
        let snapshot = captured.snapshot(0).unwrap().unwrap();
        assert_eq!(snapshot.preferred_order, [0, 2, 1]);
        let selection = Selection {
            chosen: None,
            edits: vec![Edit::Replace {
                registration: 1,
                activity: activity("new"),
            }],
        };
        let first = registry
            .prepare_selection(0, snapshot.generation, &selection)
            .unwrap()
            .unwrap();
        let stale = registry
            .prepare_selection(0, snapshot.generation, &selection)
            .unwrap()
            .unwrap();
        assert_eq!(first.preferred_order, [2, 0, 1]);
        assert_eq!(
            registry
                .snapshot(0)
                .unwrap()
                .unwrap()
                .state
                .preferred
                .entries()[1]
                .component
                .package,
            "two"
        );
        registry.validate(&first).unwrap();
        registry.publish(first).unwrap();
        assert!(registry.validate(&stale).is_err());
        assert!(registry.publish(stale).is_err());
        assert_eq!(
            captured
                .snapshot(0)
                .unwrap()
                .unwrap()
                .state
                .preferred
                .entries()[1]
                .component
                .package,
            "two"
        );
        registry.revoke();
        assert!(registry.snapshot(0).is_err());
        assert!(captured.snapshot(0).is_err());
        assert_eq!(
            snapshot.state.preferred.entries()[1].component.package,
            "two"
        );
    }
}

pub struct Captured {
    owner: Arc<Mutex<State>>,
    users: BTreeMap<i32, User>,
}
impl Captured {
    /// Immutable data for building the same capture's resolver indexes. The
    /// enclosing native capture guard owns revocation checks at request entry.
    pub fn user_states(&self) -> Vec<(i32, Arc<Preferred>)> {
        self.users
            .iter()
            .map(|(&user, record)| (user, record.snapshot.state.clone()))
            .collect()
    }
}
impl Owner for Captured {
    fn snapshot(&self, user: i32) -> Result<Option<Snapshot>, NotModelled> {
        if self.owner.lock().unwrap().revoked {
            return Err(NotModelled("preferred registry revoked"));
        }
        Ok(self.users.get(&user).map(|record| record.snapshot.clone()))
    }
}

impl std::fmt::Debug for Captured {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreferredCaptured")
            .field("owner", &Arc::as_ptr(&self.owner))
            .field(
                "generations",
                &self
                    .users
                    .iter()
                    .map(|(user, record)| (*user, record.snapshot.generation))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
impl PartialEq for Captured {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
            && self.users.len() == other.users.len()
            && self.users.iter().all(|(user, record)| {
                other.users.get(user).is_some_and(|other| {
                    record.snapshot.generation == other.snapshot.generation
                        && Arc::ptr_eq(&record.snapshot.state, &other.snapshot.state)
                })
            })
    }
}

pub type Commit = Arc<dyn Fn(i32, u64, &Selection) -> Result<(), NotModelled> + Send + Sync>;
pub struct Handle {
    pub captured: Arc<Captured>,
    commit: Commit,
    pub actions: Arc<dyn Actions>,
}
impl Handle {
    pub fn new(captured: Arc<Captured>, commit: Commit, actions: Arc<dyn Actions>) -> Self {
        Self {
            captured,
            commit,
            actions,
        }
    }
}
impl std::fmt::Debug for Handle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreferredHandle")
            .field("captured", &self.captured)
            .field("commit", &(Arc::as_ptr(&self.commit) as *const ()))
            .finish()
    }
}
impl PartialEq for Handle {
    fn eq(&self, other: &Self) -> bool {
        self.captured == other.captured
            && Arc::ptr_eq(&self.commit, &other.commit)
            && Arc::ptr_eq(&self.actions, &other.actions)
    }
}
impl Owner for Handle {
    fn snapshot(&self, user: i32) -> Result<Option<Snapshot>, NotModelled> {
        self.captured.snapshot(user)
    }
}
impl crate::package::query::preferred::MutationOwner for Handle {
    fn commit_selection(
        &self,
        user: i32,
        generation: u64,
        selection: &Selection,
    ) -> Result<(), NotModelled> {
        (self.commit)(user, generation, selection)
    }
}

pub struct PreparedMutation {
    pub stage: Option<Stage>,
    pub changed: bool,
    pub effects: super::MutationEffects,
}
impl Registry {
    pub fn prepare_mutation(
        &self,
        user: i32,
        generation: u64,
        mutation: &super::Mutation,
    ) -> Result<PreparedMutation, String> {
        use super::{Mutation, MutationEffects, filter_equals};
        let old = {
            let state = self.state.lock().unwrap();
            if state.revoked {
                return Err("preferred registry is revoked".into());
            }
            let old = state
                .users
                .get(&user)
                .ok_or("native preferred user missing")?;
            if old.snapshot.generation != generation {
                return Err("preferred mutation snapshot changed".into());
            }
            old.clone()
        };
        let mut preferred: Vec<_> = old
            .snapshot
            .state
            .preferred
            .entries()
            .iter()
            .cloned()
            .zip(old.preferred.clone())
            .collect();
        let mut persistent: Vec<_> = old
            .snapshot
            .state
            .persistent
            .entries()
            .iter()
            .cloned()
            .zip(old.persistent.clone())
            .collect();
        let mut cross: Vec<_> = old
            .snapshot
            .state
            .cross_profile
            .entries()
            .iter()
            .cloned()
            .zip(old.cross_profile.clone())
            .collect();
        let mut effects = MutationEffects::default();
        let changed;
        let home = |filter: &super::IntentFilter| {
            filter.has_action("android.intent.action.MAIN")
                && filter.has_category("android.intent.category.HOME")
                && filter.has_category("android.intent.category.DEFAULT")
        };
        match mutation {
            Mutation::Add {
                activity,
                remove_existing,
            } => {
                if activity.filter.actions.is_empty() {
                    return Ok(PreparedMutation {
                        stage: None,
                        changed: false,
                        effects,
                    });
                }
                if *remove_existing {
                    preferred.retain(|(old, _)| !filter_equals(&old.filter, &activity.filter));
                }
                preferred.push((
                    activity.clone(),
                    self.identities
                        .allocate(super::records::Record::Preferred(activity))?,
                ));
                changed = true;
                effects.update_home = home(&activity.filter);
                effects.preferred_changed_broadcast = true;
                effects.broadcast_if_home_unsent = true;
            }
            Mutation::Replace { activity } => {
                let mut validate = old.snapshot.state.as_ref().clone();
                changed = validate
                    .replace_preferred(activity.clone())
                    .map_err(str::to_owned)?;
                if changed {
                    preferred.retain(|(old, _)| !filter_equals(&old.filter, &activity.filter));
                    preferred.push((
                        activity.clone(),
                        self.identities
                            .allocate(super::records::Record::Preferred(activity))?,
                    ));
                }
                effects.update_home = changed && home(&activity.filter);
                effects.preferred_changed_broadcast = changed;
                effects.broadcast_if_home_unsent = true;
            }
            Mutation::SetLast {
                selection,
                activity,
            } => {
                let stage = self.prepare_selection(user, generation, selection)?;
                if let Some(stage) = stage {
                    preferred = stage
                        .replacement
                        .snapshot
                        .state
                        .preferred
                        .entries()
                        .iter()
                        .cloned()
                        .zip(stage.replacement.preferred)
                        .collect();
                }
                if !activity.filter.actions.is_empty() {
                    preferred.push((
                        activity.clone(),
                        self.identities
                            .allocate(super::records::Record::Preferred(activity))?,
                    ));
                }
                changed = !selection.edits.is_empty() || !activity.filter.actions.is_empty();
                effects.update_home = home(&activity.filter);
                effects.preferred_changed_broadcast = !activity.filter.actions.is_empty();
                effects.broadcast_if_home_unsent = true;
            }
            Mutation::Clear { package } => {
                let before = preferred.len();
                preferred.retain(|(old, _)| {
                    !package
                        .as_deref()
                        .is_none_or(|name| old.always && old.component.package == name)
                });
                changed = before != preferred.len();
                effects.update_home = changed;
                effects.preferred_changed_broadcast = changed;
            }
            Mutation::AddPersistent(activity) => {
                if activity.filter.actions.is_empty() {
                    return Ok(PreparedMutation {
                        stage: None,
                        changed: false,
                        effects,
                    });
                }
                persistent.push((
                    activity.clone(),
                    self.identities
                        .allocate(super::records::Record::Persistent(activity))?,
                ));
                changed = true;
                effects.update_home = home(&activity.filter);
                effects.preferred_changed_broadcast = true;
            }
            Mutation::ClearPersistentPackage(package) => {
                let before = persistent.len();
                persistent.retain(|(old, _)| old.component.package != *package);
                changed = before != persistent.len();
                effects.update_home = changed;
                effects.preferred_changed_broadcast = changed;
            }
            Mutation::ClearPersistentFilter(filter) => {
                let before = persistent.len();
                persistent.retain(|(old, _)| !filter_equals(&old.filter, filter));
                changed = before != persistent.len();
                effects.update_home = changed;
                effects.preferred_changed_broadcast = changed;
            }
            Mutation::CrossAdd(activity) => {
                changed = !activity.filter.actions.is_empty()
                    && !cross.iter().any(|(old, _)| {
                        filter_equals(&old.filter, &activity.filter)
                            && old.equals_ignore_filter(activity)
                    });
                if changed {
                    cross.push((
                        activity.clone(),
                        self.identities
                            .allocate(super::records::Record::CrossProfile(activity))?,
                    ));
                }
            }
            Mutation::CrossRemove {
                filter,
                owner_package,
                target_user,
                flags,
            } => {
                let identities: Vec<_> =
                    cross.iter().map(|(_, identity)| identity.clone()).collect();
                let index = order(&identities).into_iter().find(|&index| {
                    let old = &cross[index].0;
                    filter_equals(&old.filter, filter)
                        && old.owner_package == *owner_package
                        && old.target_user_id == *target_user
                        && old.flags == *flags
                });
                changed = index.is_some();
                if let Some(index) = index {
                    cross.remove(index);
                }
            }
            Mutation::CrossClear {
                owner_package,
                accessible_targets,
            } => {
                let before = cross.len();
                cross.retain(|(old, _)| {
                    !(old.owner_package == *owner_package
                        && accessible_targets.contains(&old.target_user_id))
                });
                changed = before != cross.len();
            }
            Mutation::Restore(restored) => {
                for activity in restored.preferred.entries() {
                    preferred.push((
                        activity.clone(),
                        self.identities
                            .allocate(super::records::Record::Preferred(activity))?,
                    ));
                }
                changed = !restored.preferred.entries().is_empty();
                effects.update_home = true;
            }
            Mutation::ApplyDefaults { defaults } => {
                changed = !defaults.is_empty();
                for activity in defaults {
                    preferred.retain(|(old, _)| !filter_equals(&old.filter, &activity.filter));
                    preferred.push((
                        activity.clone(),
                        self.identities
                            .allocate(super::records::Record::Preferred(activity))?,
                    ));
                }
            }
            Mutation::Reset { defaults } => {
                let before = preferred.len();
                preferred.clear();
                for activity in defaults {
                    preferred.push((
                        activity.clone(),
                        self.identities
                            .allocate(super::records::Record::Preferred(activity))?,
                    ));
                }
                changed = true;
                effects.preferred_changed_broadcast = before > 0;
                effects.broadcast_before_reset = true;
                effects.update_home = true;
                effects.reset_domain_user = true;
                effects.reset_runtime_permissions = true;
                effects.reset_network_policies = true;
            }
        }
        let create_cross_resolver = matches!(
            mutation,
            Mutation::CrossRemove { .. } | Mutation::CrossClear { .. }
        ) && !old.snapshot.state.cross_profile_resolver_present;
        let force_write = matches!(mutation, Mutation::CrossClear { .. });
        if !changed && !create_cross_resolver && !force_write {
            return Ok(PreparedMutation {
                stage: None,
                changed,
                effects,
            });
        }
        let mut state = Preferred::default();
        state.preferred_resolver_present =
            old.snapshot.state.preferred_resolver_present || !preferred.is_empty();
        state.persistent_resolver_present =
            old.snapshot.state.persistent_resolver_present || !persistent.is_empty();
        state.cross_profile_resolver_present = old.snapshot.state.cross_profile_resolver_present
            || !cross.is_empty()
            || create_cross_resolver;
        let mut pref_ids = Vec::new();
        let mut persistent_ids = Vec::new();
        let mut cross_ids = Vec::new();
        for (activity, identity) in preferred {
            state.preferred.add(activity);
            pref_ids.push(identity);
        }
        for (activity, identity) in persistent {
            state.persistent.add(activity);
            persistent_ids.push(identity);
        }
        for (activity, identity) in cross {
            state.cross_profile.add(activity);
            cross_ids.push(identity);
        }
        let state = Arc::new(state);
        let preferred_order = order(&pref_ids);
        let replacement = User {
            snapshot: Snapshot {
                generation,
                state: state.clone(),
                preferred_order: preferred_order.clone(),
            },
            preferred: pref_ids,
            persistent: persistent_ids,
            cross_profile: cross_ids,
        };
        Ok(PreparedMutation {
            stage: Some(Stage {
                user,
                before_generation: generation,
                preferred: state,
                preferred_order,
                persistent_order: order(&replacement.persistent),
                cross_profile_order: order(&replacement.cross_profile),
                replacement,
            }),
            changed,
            effects,
        })
    }
}

#[derive(Debug)]
pub enum ActionError {
    Exception(aim_binder_host::parcel::Exception),
    Unavailable(NotModelled),
}
impl From<NotModelled> for ActionError {
    fn from(value: NotModelled) -> Self {
        Self::Unavailable(value)
    }
}
/// Independent native/remaining-original owners. Every method is mandatory;
/// the PackageManager constructor binds these before accepting this cohort.
pub trait Actions: Send + Sync {
    /// Original Context permission check, including live shell delegation.
    fn context_permission(&self, calling_uid: i32, name: &str) -> Result<bool, ActionError>;
    fn commit_after_selection(
        &self,
        user: i32,
        mutation: &super::Mutation,
        calling_uid: i32,
    ) -> Result<bool, ActionError>;
    fn commit_mutation(
        &self,
        user: i32,
        generation: u64,
        mutation: &super::Mutation,
        calling_uid: i32,
    ) -> Result<bool, ActionError>;
    fn cross_access(
        &self,
        calling_uid: i32,
        source: i32,
        target: i32,
        adding: bool,
    ) -> Result<i32, ActionError>;
    fn cross_accessible(
        &self,
        calling_uid: i32,
        source: i32,
        target: i32,
    ) -> Result<bool, ActionError>;
    fn enforce_shell_restriction(&self, calling_uid: i32, user: i32) -> Result<(), ActionError>;
    fn reconcile_home(&self, user: i32, calling_uid: i32) -> Result<bool, ActionError>;
    fn commit_home_selection(
        &self,
        user: i32,
        generation: u64,
        selection: &Selection,
    ) -> Result<(), ActionError>;
    fn default_home(&self, user: i32) -> Result<Option<String>, ActionError>;
    fn default_browser(&self, user: i32) -> Result<Option<String>, ActionError>;
    fn restore_browser(&self, user: i32, package: &str, installed: bool)
    -> Result<(), ActionError>;
    fn default_preferences(&self, user: i32) -> Result<Vec<super::PreferredActivity>, ActionError>;
}

impl Registry {
    /// Settings USER_ALL iteration is ascending SparseArray user order and its
    /// shared removal list keeps later existing resolvers marked changed.
    pub fn prepare_clear_all(&self, package: Option<&str>) -> Result<Vec<Stage>, String> {
        let users = {
            let state = self.state.lock().unwrap();
            if state.revoked {
                return Err("preferred registry is revoked".into());
            }
            state
                .users
                .iter()
                .filter(|(_, user)| {
                    user.snapshot.state.preferred_resolver_present
                        || !user.snapshot.state.preferred.entries().is_empty()
                })
                .map(|(&user, record)| (user, record.snapshot.generation))
                .collect::<Vec<_>>()
        };
        let mut removed = false;
        let mut stages = Vec::new();
        for (user, generation) in users {
            let prepared = self.prepare_mutation(
                user,
                generation,
                &super::Mutation::Clear {
                    package: package.map(str::to_owned),
                },
            )?;
            removed |= prepared.changed;
            if let Some(stage) = prepared.stage {
                stages.push(stage);
            } else if removed {
                let old = {
                    let state = self.state.lock().unwrap();
                    let old = state.users.get(&user).ok_or("preferred user changed")?;
                    if old.snapshot.generation != generation {
                        return Err("preferred user changed".into());
                    }
                    old.clone()
                };
                stages.push(Stage {
                    user,
                    before_generation: generation,
                    preferred: old.snapshot.state.clone(),
                    preferred_order: old.snapshot.preferred_order.clone(),
                    persistent_order: order(&old.persistent),
                    cross_profile_order: order(&old.cross_profile),
                    replacement: old,
                });
            }
        }
        Ok(stages)
    }
}

impl Captured {
    /// Tokens in native registration order, retaining the exact original record
    /// objects used for every typed resolver in this captured generation.
    /// None preserves an absent resolver; Some(empty) preserves an existing one.
    pub fn record_tokens(
        &self,
        user: i32,
        kind: i32,
    ) -> Result<Option<Vec<aim_binder_host::parcel::Binder>>, String> {
        use super::records::{CROSS_PROFILE, PERSISTENT, PREFERRED};
        if self.owner.lock().unwrap().revoked {
            return Err("preferred registry revoked".into());
        }
        let Some(user) = self.users.get(&user) else {
            return Ok(None);
        };
        let (present, identities) = match kind {
            PREFERRED => (
                user.snapshot.state.preferred_resolver_present,
                &user.preferred,
            ),
            PERSISTENT => (
                user.snapshot.state.persistent_resolver_present,
                &user.persistent,
            ),
            CROSS_PROFILE => (
                user.snapshot.state.cross_profile_resolver_present,
                &user.cross_profile,
            ),
            _ => return Err("unknown preferred resolver record kind".into()),
        };
        if !present && identities.is_empty() {
            return Ok(None);
        }
        identities
            .iter()
            .map(|identity| {
                let (actual_kind, binder) = identity
                    .lease
                    .typed_record()
                    .ok_or("untyped preferred resolver identity")?;
                if actual_kind != kind {
                    return Err("preferred resolver lease kind mismatch".into());
                }
                Ok(binder)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

pub struct NewUserStage {
    pub user: i32,
    pub preferred: Arc<Preferred>,
    pub preferred_order: Vec<usize>,
    pub persistent_order: Vec<usize>,
    pub cross_profile_order: Vec<usize>,
    base_generation: u64,
    replacement: User,
}
impl Registry {
    pub fn prepare_new_user(
        &self,
        user: i32,
        preferred: Preferred,
    ) -> Result<NewUserStage, String> {
        let base_generation = {
            let state = self.state.lock().unwrap();
            if state.revoked || state.users.contains_key(&user) {
                return Err("preferred new-user owner unavailable or already registered".into());
            }
            state.generation
        };
        let allocate_preferred = preferred
            .preferred
            .entries()
            .iter()
            .map(|record| {
                self.identities
                    .allocate(super::records::Record::Preferred(record))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let persistent = preferred
            .persistent
            .entries()
            .iter()
            .map(|record| {
                self.identities
                    .allocate(super::records::Record::Persistent(record))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cross = preferred
            .cross_profile
            .entries()
            .iter()
            .map(|record| {
                self.identities
                    .allocate(super::records::Record::CrossProfile(record))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let preferred = Arc::new(preferred);
        let preferred_order = order(&allocate_preferred);
        let replacement = User {
            snapshot: Snapshot {
                generation: base_generation
                    .checked_add(1)
                    .ok_or("preferred generation exhausted")?,
                state: preferred.clone(),
                preferred_order: preferred_order.clone(),
            },
            preferred: allocate_preferred,
            persistent,
            cross_profile: cross,
        };
        Ok(NewUserStage {
            user,
            preferred,
            preferred_order,
            persistent_order: order(&replacement.persistent),
            cross_profile_order: order(&replacement.cross_profile),
            base_generation,
            replacement,
        })
    }
    pub fn preview_new_user(&self, stage: &NewUserStage) -> Result<Captured, String> {
        let state = self.state.lock().unwrap();
        if state.revoked
            || state.generation != stage.base_generation
            || state.users.contains_key(&stage.user)
        {
            return Err("preferred new-user stage changed".into());
        }
        let mut users = state.users.clone();
        users.insert(stage.user, stage.replacement.clone());
        Ok(Captured {
            owner: self.state.clone(),
            users,
        })
    }
    pub fn publish_new_user(&self, stage: NewUserStage) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.revoked
            || state.generation != stage.base_generation
            || state.users.contains_key(&stage.user)
        {
            return Err("preferred new-user stage changed".into());
        }
        state.generation = stage.replacement.snapshot.generation;
        state.users.insert(stage.user, stage.replacement);
        Ok(())
    }
}

impl Captured {
    pub fn record_order(&self, user: i32, kind: i32) -> Result<Vec<usize>, String> {
        use super::records::{CROSS_PROFILE, PERSISTENT, PREFERRED};
        if self.owner.lock().unwrap().revoked {
            return Err("preferred registry revoked".into());
        }
        let Some(record) = self.users.get(&user) else {
            return Ok(Vec::new());
        };
        match kind {
            PREFERRED => Ok(record.snapshot.preferred_order.clone()),
            PERSISTENT => Ok(order(&record.persistent)),
            CROSS_PROFILE => Ok(order(&record.cross_profile)),
            _ => Err("unknown preferred resolver kind".into()),
        }
    }
}
impl Registry {
    pub fn user_states(&self) -> Vec<(i32, Arc<Preferred>)> {
        self.state
            .lock()
            .unwrap()
            .users
            .iter()
            .map(|(&user, record)| (user, record.snapshot.state.clone()))
            .collect()
    }
}
