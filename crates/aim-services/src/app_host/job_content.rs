//! `ContentObserverController` of the app host's `jobscheduler`
//! (`android-16.0.0_r1`): each content trigger job's `JobInstance`, fed by
//! the observers of its trigger URIs (one per user and trigger, shared by
//! the jobs that name it, registered with the app host's ContentService as
//! system_server's `ContentObserver`). An observer's change adds its URI
//! (up to `MAX_URIS_REPORTED`) and authority to each of its jobs, whose
//! content trigger is satisfied after the job's update delay (at once from
//! `URIS_URGENT_THRESHOLD` URIs), at the latest after its maximum delay.
//! The changes go to the job when it runs (`prepareForExecutionLocked`),
//! to a job replacing it, and to its rescheduling after a failure.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, HashMap};

use aim_binder_host::parcel::Exception;

use super::job_info::TriggerContentUri;
use crate::package::info::java_hash;
use crate::package::uri::Uri;

/// `JobInfo.TriggerContentUri.FLAG_NOTIFY_FOR_DESCENDANTS`.
const FLAG_NOTIFY_FOR_DESCENDANTS: i32 = 1;
/// `MAX_URIS_REPORTED`, `URIS_URGENT_THRESHOLD`.
const MAX_URIS_REPORTED: usize = 50;
const URIS_URGENT_THRESHOLD: usize = 40;
/// `JobStatus.DEFAULT_TRIGGER_UPDATE_DELAY`, `MIN_TRIGGER_UPDATE_DELAY`,
/// `DEFAULT_TRIGGER_MAX_DELAY`, `MIN_TRIGGER_MAX_DELAY`.
const DEFAULT_TRIGGER_UPDATE_DELAY: i64 = 10 * 1000;
const MIN_TRIGGER_UPDATE_DELAY: i64 = 500;
const DEFAULT_TRIGGER_MAX_DELAY: i64 = 2 * 60 * 1000;
const MIN_TRIGGER_MAX_DELAY: i64 = 1000;

/// `JobStatus.getTriggerContentUpdateDelay` of the job's own.
pub(super) fn update_delay(time: i64) -> i64 {
    if time < 0 { DEFAULT_TRIGGER_UPDATE_DELAY } else { time.max(MIN_TRIGGER_UPDATE_DELAY) }
}

/// `JobStatus.getTriggerContentMaxDelay` of the job's own.
pub(super) fn max_delay(time: i64) -> i64 {
    if time < 0 { DEFAULT_TRIGGER_MAX_DELAY } else { time.max(MIN_TRIGGER_MAX_DELAY) }
}

/// `ArraySet.add`: once, in the order of its hash (`hashCode` of the
/// string `key` gives), after those of the same hash.
fn add<T>(set: &mut Vec<T>, item: T, key: impl Fn(&T) -> &str) {
    if set.iter().any(|i| key(i) == key(&item)) {
        return;
    }
    let hash = java_hash(key(&item));
    let at = set.iter().position(|i| java_hash(key(i)) > hash).unwrap_or(set.len());
    set.insert(at, item);
}

/// `ArraySet.addAll` of `from` into `into`, created if none.
fn add_all<T>(into: &mut Option<Vec<T>>, from: Vec<T>, key: impl Fn(&T) -> &str + Copy) {
    let set = into.get_or_insert_with(Vec::new);
    for item in from {
        add(set, item, key);
    }
}

fn uri_key(u: &Uri) -> &str {
    u.as_str()
}

fn str_key(s: &String) -> &str {
    s
}

/// The changes reported to a job: `changedUris` and `changedAuthorities`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Changes {
    pub uris: Option<Vec<Uri>>,
    pub authorities: Option<Vec<String>>,
}

/// What a `JobStatus` holds for the controller.
#[derive(Debug, Default)]
pub(super) struct JobContent {
    /// `TRACKING_CONTENT`.
    tracked: bool,
    /// `contentObserverJobInstance`.
    instance: Option<u64>,
    /// `changedUris`, `changedAuthorities`.
    pub changes: Changes,
    /// `CONSTRAINT_CONTENT_TRIGGER`'s satisfaction.
    pub satisfied: bool,
    /// What the job it replaces had pending, given over by
    /// `maybeStopTrackingJobLocked`, and that job's instance, detached once
    /// this one is tracked (`lastJob`).
    carried: Option<Changes>,
    last: Option<u64>,
}

/// A content trigger job's triggers: its trigger URIs, its source user and
/// its delays.
pub(super) struct Spec<'a> {
    pub triggers: &'a [TriggerContentUri],
    pub user: i32,
    pub update_delay: i64,
    pub max_delay: i64,
}

/// `ObserverInstance`'s key: the user and the trigger (`TriggerContentUri`
/// equal by URI and flags).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ObserverKey {
    pub user: i32,
    pub uri: String,
    pub flags: i32,
}

/// The registration of an observer with ContentService.
pub(super) trait Observe {
    /// `registerContentObserver(uri, descendants, observer, user)` of the
    /// observer of `key`: its registration.
    fn register(&self, uri: &Uri, descendants: bool, key: &ObserverKey) -> Result<u64, Exception>;
    /// `unregisterContentObserver`.
    fn unregister(&self, registration: u64);
}

/// `ObserverInstance`.
struct Observer {
    registration: u64,
    /// `mJobs`.
    instances: Vec<u64>,
}

/// `JobInstance`.
#[derive(Debug, Default)]
struct Instance {
    /// The job it feeds.
    job: Option<u64>,
    /// `mMyObservers`.
    observers: Vec<ObserverKey>,
    /// `mChangedUris`, `mChangedAuthorities`.
    changes: Changes,
    update_delay: i64,
    max_delay: i64,
    /// `mTriggerPending`, and when `mExecuteRunner` and `mTimeoutRunner`
    /// run.
    trigger_pending: bool,
    execute_at: Option<i64>,
    timeout_at: Option<i64>,
}

#[derive(Default)]
pub(super) struct Controller {
    /// `mObservers`.
    observers: BTreeMap<ObserverKey, Observer>,
    instances: HashMap<u64, Instance>,
    next: u64,
}

impl Controller {
    /// `new JobInstance(jobStatus)`: an observer of each trigger, the
    /// user's existing one or one registered.
    fn new_instance(&mut self, spec: &Spec<'_>, observe: &dyn Observe) -> Result<u64, Exception> {
        self.next += 1;
        let id = self.next;
        let mut instance =
            Instance { update_delay: spec.update_delay, max_delay: spec.max_delay, ..Default::default() };
        for trigger in spec.triggers {
            let uri = trigger.uri.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "uri"))?;
            let key = ObserverKey { user: spec.user, uri: uri.as_str().to_string(), flags: trigger.flags };
            if !self.observers.contains_key(&key) {
                let registration = match observe.register(uri, trigger.flags & FLAG_NOTIFY_FOR_DESCENDANTS != 0, &key) {
                    Ok(r) => r,
                    Err(e) => {
                        self.instances.insert(id, instance);
                        self.detach(id, observe);
                        return Err(e);
                    }
                };
                self.observers.insert(key.clone(), Observer { registration, instances: Vec::new() });
            }
            self.observers.get_mut(&key).unwrap().instances.push(id);
            instance.observers.push(key);
        }
        self.instances.insert(id, instance);
        Ok(id)
    }

    /// `maybeStartTrackingJobLocked` of the content trigger job `key`
    /// (`spec`): its instance, the changes its last run did not take or the
    /// job it replaces had pending, and its constraint satisfied if there
    /// are any.
    pub fn start_tracking(&mut self, key: u64, spec: &Spec<'_>, c: &mut JobContent, observe: &dyn Observe) -> Result<(), Exception> {
        let id = match c.instance {
            Some(id) => id,
            None => self.new_instance(spec, observe)?,
        };
        c.instance = Some(id);
        c.tracked = true;
        let instance = self.instances.get_mut(&id).unwrap();
        instance.job = Some(key);
        if let Some(carried) = c.carried.take() {
            instance.changes = carried;
        }
        let mut pending = instance.changes.authorities.is_some();
        if let Some(authorities) = c.changes.authorities.take() {
            pending = true;
            add_all(&mut instance.changes.authorities, authorities, str_key);
            if let Some(uris) = c.changes.uris.take() {
                add_all(&mut instance.changes.uris, uris, uri_key);
            }
        }
        c.changes = Changes::default();
        c.satisfied = pending;
        Ok(())
    }

    /// What `maybeStartTrackingJobLocked` does for every job: the instance
    /// of the job it replaced (`lastJob`) detached.
    pub fn forget_last(&mut self, c: &mut JobContent, observe: &dyn Observe) {
        if let Some(last) = c.last.take() {
            self.detach(last, observe);
        }
    }

    /// `prepareForExecutionLocked`: the instance's changes are the job's.
    pub fn prepare(&mut self, c: &mut JobContent) {
        if let Some(instance) = c.instance.and_then(|id| self.instances.get_mut(&id)) {
            c.changes = std::mem::take(&mut instance.changes);
        }
    }

    /// `unprepareFromExecutionLocked`: the job's changes back with the
    /// instance's.
    pub fn unprepare(&mut self, c: &mut JobContent) {
        if let Some(instance) = c.instance.and_then(|id| self.instances.get_mut(&id)) {
            let changes = std::mem::take(&mut c.changes);
            match (&mut instance.changes.uris, changes.uris) {
                (None, uris) => instance.changes.uris = uris,
                (Some(_), Some(uris)) => add_all(&mut instance.changes.uris, uris, uri_key),
                (Some(_), None) => {}
            }
            match (&mut instance.changes.authorities, changes.authorities) {
                (None, authorities) => instance.changes.authorities = authorities,
                (Some(_), Some(authorities)) => add_all(&mut instance.changes.authorities, authorities, str_key),
                (Some(_), None) => {}
            }
        }
    }

    /// `maybeStopTrackingJobLocked`: its trigger unscheduled; replaced by
    /// `incoming`, its pending changes given over and its observers kept
    /// until `incoming` is tracked; else its instance detached.
    pub fn stop_tracking(&mut self, c: &mut JobContent, incoming: Option<&mut JobContent>, observe: &dyn Observe) {
        if !std::mem::take(&mut c.tracked) {
            return;
        }
        let Some(id) = c.instance else { return };
        self.unschedule(id);
        match incoming {
            Some(incoming) => {
                let instance = self.instances.get_mut(&id).unwrap();
                if instance.changes.authorities.is_some() {
                    incoming.carried = Some(std::mem::take(&mut instance.changes));
                }
                incoming.last = Some(id);
            }
            None => {
                self.detach(id, observe);
                c.instance = None;
            }
        }
    }

    /// `rescheduleForFailureLocked`: the changes reported to the failed run
    /// reported again.
    pub fn reschedule_for_failure(new: &mut JobContent, failed: &JobContent) {
        new.changes = failed.changes.clone();
    }

    /// `ObserverInstance.onChange(selfChange, uri)` of the observer `key`:
    /// the change recorded in each of its jobs, whose trigger is
    /// scheduled.
    pub fn on_change(&mut self, key: &ObserverKey, uri: &Uri, now: i64) {
        let Some(observer) = self.observers.get(key) else { return };
        for id in observer.instances.clone() {
            let Some(instance) = self.instances.get_mut(&id) else { continue };
            let uris = instance.changes.uris.get_or_insert_with(Vec::new);
            if uris.len() < MAX_URIS_REPORTED {
                add(uris, uri.clone(), uri_key);
            }
            if let Some(authority) = uri.authority() {
                add(instance.changes.authorities.get_or_insert_with(Vec::new), authority, str_key);
            }
            // `scheduleLocked`.
            if !instance.trigger_pending {
                instance.trigger_pending = true;
                instance.timeout_at = Some(now + instance.max_delay);
            }
            let urgent = instance.changes.uris.as_ref().is_some_and(|u| u.len() >= URIS_URGENT_THRESHOLD);
            instance.execute_at = Some(if urgent { now } else { now + instance.update_delay });
        }
    }

    /// `unscheduleLocked`.
    fn unschedule(&mut self, id: u64) {
        if let Some(instance) = self.instances.get_mut(&id)
            && instance.trigger_pending
        {
            instance.execute_at = None;
            instance.timeout_at = None;
            instance.trigger_pending = false;
        }
    }

    /// `trigger()` of each instance whose runner is due at `now`: the jobs
    /// whose content trigger is now satisfied.
    pub fn due(&mut self, now: i64) -> Vec<u64> {
        let due: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, i)| i.execute_at.into_iter().chain(i.timeout_at).any(|t| t <= now))
            .map(|(id, _)| *id)
            .collect();
        let mut jobs = Vec::new();
        for id in due {
            let instance = &self.instances[&id];
            if instance.trigger_pending {
                jobs.extend(instance.job);
                self.unschedule(id);
            }
        }
        jobs
    }

    /// When the next runner is due.
    pub fn next_due(&self) -> Option<i64> {
        self.instances.values().flat_map(|i| i.execute_at.into_iter().chain(i.timeout_at)).min()
    }

    /// `detachLocked`: off its observers, those left without jobs
    /// unregistered.
    fn detach(&mut self, id: u64, observe: &dyn Observe) {
        let Some(instance) = self.instances.remove(&id) else { return };
        for key in instance.observers {
            let Some(observer) = self.observers.get_mut(&key) else { continue };
            if let Some(at) = observer.instances.iter().position(|i| *i == id) {
                observer.instances.remove(at);
            }
            if observer.instances.is_empty() {
                observe.unregister(observer.registration);
                self.observers.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// ContentService's registrations.
    #[derive(Default)]
    struct Registrar {
        registered: RefCell<Vec<(String, bool, i32)>>,
        unregistered: RefCell<Vec<u64>>,
    }

    impl Observe for Registrar {
        fn register(&self, uri: &Uri, descendants: bool, key: &ObserverKey) -> Result<u64, Exception> {
            if uri.authority().is_none() {
                return Err(Exception::illegal_argument(format!("Invalid Uri ({}) used for observer", uri.as_str())));
            }
            let mut r = self.registered.borrow_mut();
            r.push((uri.as_str().to_string(), descendants, key.user));
            Ok(r.len() as u64)
        }

        fn unregister(&self, registration: u64) {
            self.unregistered.borrow_mut().push(registration);
        }
    }

    fn trigger(uri: &str, flags: i32) -> TriggerContentUri {
        TriggerContentUri { uri: Some(Uri::parse(uri)), flags }
    }

    fn spec(triggers: &[TriggerContentUri]) -> Spec<'_> {
        Spec { triggers, user: 0, update_delay: update_delay(-1), max_delay: max_delay(-1) }
    }

    fn key(uri: &str, flags: i32) -> ObserverKey {
        ObserverKey { user: 0, uri: uri.into(), flags }
    }

    const CONTACTS: &str = "content://com.android.contacts/contacts";

    /// `JobStatus`'s delays: the defaults for none, at least the minimums.
    #[test]
    fn delays_are_the_originals() {
        assert_eq!((update_delay(-1), max_delay(-1)), (10_000, 120_000));
        assert_eq!((update_delay(100), max_delay(100)), (500, 1000));
        assert_eq!((update_delay(3000), max_delay(60_000)), (3000, 60_000));
    }

    /// Jobs share an observer per user and trigger (URI and flags), which
    /// goes once its last job is detached.
    #[test]
    fn observers_are_shared_and_unregistered_with_their_last_job() {
        let r = Registrar::default();
        let mut c = Controller::default();
        let triggers = [trigger(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS)];
        let (mut a, mut b) = (JobContent::default(), JobContent::default());
        c.start_tracking(1, &spec(&triggers), &mut a, &r).unwrap();
        c.start_tracking(2, &spec(&triggers), &mut b, &r).unwrap();
        let other = [trigger(CONTACTS, 0)];
        let mut d = JobContent::default();
        c.start_tracking(3, &spec(&other), &mut d, &r).unwrap();
        assert_eq!(*r.registered.borrow(), [(CONTACTS.to_string(), true, 0), (CONTACTS.to_string(), false, 0)]);
        assert!(!a.satisfied);
        c.stop_tracking(&mut a, None, &r);
        assert!(r.unregistered.borrow().is_empty());
        c.stop_tracking(&mut b, None, &r);
        assert_eq!(*r.unregistered.borrow(), [1]);
        // An observer that cannot be registered fails the job's tracking.
        let bad = [trigger("content:/nothing", 0)];
        let mut e = JobContent::default();
        assert!(c.start_tracking(4, &spec(&bad), &mut e, &r).is_err());
    }

    /// A change is recorded in each job of its observer and satisfies the
    /// trigger after the update delay, pushed back by each change but not
    /// past the maximum delay.
    #[test]
    fn changes_trigger_after_their_delays() {
        let r = Registrar::default();
        let mut c = Controller::default();
        let triggers = [trigger(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS)];
        let mut j = JobContent::default();
        c.start_tracking(7, &spec(&triggers), &mut j, &r).unwrap();
        let k = key(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS);
        c.on_change(&k, &Uri::parse("content://com.android.contacts/contacts/1"), 0);
        assert_eq!(c.next_due(), Some(10_000));
        assert!(c.due(9_999).is_empty());
        for t in (5_000..=115_000).step_by(5_000) {
            c.on_change(&k, &Uri::parse("content://com.android.contacts/contacts/1"), t);
            assert!(c.due(t).is_empty());
        }
        assert_eq!(c.due(120_000), [7]);
        assert_eq!(c.next_due(), None);
        // The job takes the changes when it runs.
        c.prepare(&mut j);
        assert_eq!(j.changes.uris.as_ref().unwrap().len(), 1);
        assert_eq!(j.changes.authorities.as_deref(), Some(&["com.android.contacts".to_string()][..]));
    }

    /// At most `MAX_URIS_REPORTED` URIs, kept as an `ArraySet` keeps them;
    /// from `URIS_URGENT_THRESHOLD` the trigger is at once.
    #[test]
    fn uris_are_reported_up_to_the_limit_and_urgently() {
        let r = Registrar::default();
        let mut c = Controller::default();
        let triggers = [trigger(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS)];
        let mut j = JobContent::default();
        c.start_tracking(7, &spec(&triggers), &mut j, &r).unwrap();
        let k = key(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS);
        for n in 0..39 {
            c.on_change(&k, &Uri::parse(&format!("{CONTACTS}/{n}")), 0);
        }
        assert_eq!(c.next_due(), Some(10_000));
        c.on_change(&k, &Uri::parse(&format!("{CONTACTS}/39")), 1);
        assert_eq!(c.due(1), [7]);
        for n in 40..60 {
            c.on_change(&k, &Uri::parse(&format!("{CONTACTS}/{n}")), 2);
        }
        c.prepare(&mut j);
        let uris = j.changes.uris.unwrap();
        assert_eq!(uris.len(), MAX_URIS_REPORTED);
        let hashes: Vec<i32> = uris.iter().map(|u| java_hash(u.as_str())).collect();
        assert!(hashes.windows(2).all(|w| w[0] <= w[1]));
    }

    /// A job replacing another takes over its pending changes and its
    /// observers, which stay registered between the two.
    #[test]
    fn a_replacing_job_takes_the_pending_changes() {
        let r = Registrar::default();
        let mut c = Controller::default();
        let triggers = [trigger(CONTACTS, 0)];
        let mut old = JobContent::default();
        c.start_tracking(1, &spec(&triggers), &mut old, &r).unwrap();
        c.on_change(&key(CONTACTS, 0), &Uri::parse(CONTACTS), 0);
        let mut new = JobContent::default();
        c.stop_tracking(&mut old, Some(&mut new), &r);
        c.start_tracking(2, &spec(&triggers), &mut new, &r).unwrap();
        c.forget_last(&mut new, &r);
        assert!(new.satisfied);
        assert!(r.unregistered.borrow().is_empty());
        assert_eq!(r.registered.borrow().len(), 1);
        // Its trigger was unscheduled with the old job.
        assert!(c.due(i64::MAX).is_empty());
        c.prepare(&mut new);
        assert_eq!(new.changes.uris.as_ref().map(Vec::len), Some(1));
    }

    /// A failed run's changes are reported to its rescheduling, merged
    /// with those since.
    #[test]
    fn a_failed_runs_changes_are_reported_again() {
        let r = Registrar::default();
        let mut c = Controller::default();
        let triggers = [trigger(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS)];
        let k = key(CONTACTS, FLAG_NOTIFY_FOR_DESCENDANTS);
        let mut run = JobContent::default();
        c.start_tracking(1, &spec(&triggers), &mut run, &r).unwrap();
        c.on_change(&k, &Uri::parse(&format!("{CONTACTS}/1")), 0);
        c.prepare(&mut run);
        c.on_change(&k, &Uri::parse(&format!("{CONTACTS}/2")), 1);
        let mut next = JobContent::default();
        Controller::reschedule_for_failure(&mut next, &run);
        c.stop_tracking(&mut run, Some(&mut next), &r);
        c.start_tracking(2, &spec(&triggers), &mut next, &r).unwrap();
        c.forget_last(&mut next, &r);
        assert!(next.satisfied);
        c.prepare(&mut next);
        let mut uris: Vec<&str> = next.changes.uris.as_ref().unwrap().iter().map(Uri::as_str).collect();
        uris.sort();
        assert_eq!(uris, [format!("{CONTACTS}/1"), format!("{CONTACTS}/2")]);
        // A run that could not start gives its changes back.
        c.on_change(&k, &Uri::parse(&format!("{CONTACTS}/3")), 2);
        c.unprepare(&mut next);
        c.prepare(&mut next);
        assert_eq!(next.changes.uris.as_ref().map(Vec::len), Some(3));
    }
}
