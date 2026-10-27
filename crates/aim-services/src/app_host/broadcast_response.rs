//! UsageStatsService's broadcast response stats for the app host
//! (`BroadcastResponseStatsTracker` at `android-16.0.0_r1`): a broadcast
//! sent with an id for its response event (`BroadcastOptions
//! .recordResponseEventWhileInBackground`) to an app in the background
//! is noted, and the notifications the app then posts, updates or
//! cancels within the response window count as its response, per sender
//! uid, target user, package and id; `queryBroadcastResponseStats`,
//! `clearBroadcastResponseStats` and `clearBroadcastEvents` with their
//! checks. The windows, the foreground threshold and the exempted roles
//! and permissions are the `app_standby` DeviceConfig namespace's, as
//! AppStandbyController reads them.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use aim_binder_host::parcel::{Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use super::activity_user;
use super::settings::Table;
use super::{AppHost, PER_USER_RANGE};
use crate::package::info::java_hash;

const ACCESS_BROADCAST_RESPONSE_STATS: &str = "android.permission.ACCESS_BROADCAST_RESPONSE_STATS";
const DUMP: &str = "android.permission.DUMP";
const ONE_MINUTE: i64 = 60_000;
/// `ConstantsObserver.DEFAULT_*` of the broadcast response stats.
const DEFAULT_RESPONSE_WINDOW_MS: i64 = 2 * ONE_MINUTE;
/// `ActivityManager.PROCESS_STATE_TOP`.
const DEFAULT_FG_THRESHOLD_STATE: i32 = 2;
const DEFAULT_SESSIONS_MS: i64 = 2 * ONE_MINUTE;
const DEFAULT_SESSIONS_WITH_RESPONSE_MS: i64 = 2 * ONE_MINUTE;

/// A notification event (`NOTIFICATION_EVENT_TYPE_*`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Notified {
    Posted,
    Updated,
    Cancelled,
}

/// The constants the tracker asks AppStandbyController.
#[derive(Clone, Debug, PartialEq)]
struct Constants {
    response_window_ms: i64,
    fg_threshold_state: i32,
    sessions_ms: i64,
    sessions_with_response_ms: i64,
    note_all_sessions: bool,
    exempted_roles: Vec<String>,
    exempted_permissions: Vec<String>,
}

impl Constants {
    /// `processProperties` of the broadcast response keys of `property`.
    fn read(property: &dyn Fn(&str) -> Option<String>) -> Constants {
        let long = |key: &str, default: i64| {
            property(key)
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(default)
        };
        // `splitPipeSeparatedString`: empty parts dropped.
        let list = |key: &str| -> Vec<String> {
            property(key)
                .unwrap_or_default()
                .split('|')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        };
        Constants {
            response_window_ms: long(
                "broadcast_response_window_timeout_ms",
                DEFAULT_RESPONSE_WINDOW_MS,
            ),
            fg_threshold_state: property("broadcast_response_fg_threshold_state")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(DEFAULT_FG_THRESHOLD_STATE),
            sessions_ms: long("broadcast_sessions_duration_ms", DEFAULT_SESSIONS_MS),
            sessions_with_response_ms: long(
                "broadcast_sessions_with_response_duration_ms",
                DEFAULT_SESSIONS_WITH_RESPONSE_MS,
            ),
            note_all_sessions: property("note_response_event_for_all_broadcast_sessions")
                .map_or(true, |v| v == "true"),
            // The keys as the original spells them.
            exempted_roles: list("brodacast_response_exempted_roles"),
            exempted_permissions: list("brodacast_response_exempted_permissions"),
        }
    }
}

/// `BroadcastEvent`: a sender, a target and the id, and when it was sent.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    source_uid: i32,
    target_package: String,
    target_user: i32,
    id: i64,
}

impl Key {
    /// `Objects.hash(mSourceUid, mTargetPackage, mTargetUserId,
    /// mIdForResponseEvent)`, which orders the `ArrayMap`s.
    fn java_hash(&self) -> i32 {
        let long = (self.id ^ ((self.id as u64) >> 32) as i64) as i32;
        [
            self.source_uid,
            java_hash(&self.target_package),
            self.target_user,
            long,
        ]
        .iter()
        .fold(1i32, |h, &v| h.wrapping_mul(31).wrapping_add(v))
    }
}

/// `BroadcastResponseStats`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Stats {
    package: String,
    id: i64,
    dispatched: i32,
    posted: i32,
    updated: i32,
    cancelled: i32,
}

impl WriteParcelable for Stats {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(Some(&self.package));
        p.write_i64(self.id);
        for count in [self.dispatched, self.posted, self.updated, self.cancelled] {
            p.write_i32(count);
        }
    }
}

/// `BroadcastResponseStatsList`: its stats as a typed list in a blob.
pub(super) struct StatsList(pub Vec<Stats>);

impl StatsList {
    /// The typed list the blob holds.
    fn marshalled(&self) -> Parcel {
        let mut data = Parcel::new();
        data.write_i32(self.0.len() as i32);
        for s in &self.0 {
            data.write_i32(1);
            s.write_to(&mut data);
        }
        data
    }
}

impl WriteParcelable for StatsList {
    fn write_to(&self, p: &mut Parcel) {
        let data = self.marshalled();
        // `writeBlob`: its length, then in place (`BLOB_INPLACE`).
        let bytes = data.data();
        p.write_i32(bytes.len() as i32);
        p.write_i32(0);
        p.write_raw(bytes, &[]);
    }
}

/// The tracker's state.
#[derive(Default)]
struct State {
    /// `mUserBroadcastEvents`: by user, the events of each target
    /// package, with their dispatch times.
    events: HashMap<i32, HashMap<String, Vec<(Key, VecDeque<i64>)>>>,
    /// `mUserResponseStats`: by sender uid and target user, in `ArrayMap`
    /// order.
    stats: HashMap<i32, HashMap<i32, Vec<(Key, Stats)>>>,
}

impl State {
    /// `getOrCreateBroadcastResponseStats`.
    fn stats_of(&mut self, key: &Key) -> &mut Stats {
        let list = self
            .stats
            .entry(key.source_uid)
            .or_default()
            .entry(key.target_user)
            .or_default();
        let at = match list.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => {
                // `ArrayMap.put`: by hash, after those of the same hash.
                let hash = key.java_hash();
                let at = list
                    .iter()
                    .position(|(k, _)| k.java_hash() > hash)
                    .unwrap_or(list.len());
                let stats = Stats {
                    package: key.target_package.clone(),
                    id: key.id,
                    ..Default::default()
                };
                list.insert(at, (key.clone(), stats));
                at
            }
        };
        &mut list[at].1
    }

    /// `recordAndPruneOldBroadcastDispatchTimestamps`: the dispatches
    /// older than the response window count as unanswered sessions.
    fn prune(&mut self, key: &Key, times: &mut VecDeque<i64>, c: &Constants, now: i64) {
        let mut session_end = 0;
        while let Some(&t) = times.front() {
            if t >= now - c.response_window_ms {
                break;
            }
            if t >= session_end {
                self.stats_of(key).dispatched += 1;
                session_end = t + c.sessions_ms;
            }
            times.pop_front();
        }
    }

    /// `reportBroadcastDispatchEvent` past its checks.
    fn dispatched(&mut self, key: Key, at: i64, c: &Constants, now: i64) {
        let events = self
            .events
            .entry(key.target_user)
            .or_default()
            .entry(key.target_package.clone())
            .or_default();
        let at_index = match events.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                events.push((key.clone(), VecDeque::new()));
                events.len() - 1
            }
        };
        let mut times = std::mem::take(&mut events[at_index].1);
        times.push_back(at);
        self.prune(&key, &mut times, c, now);
        if let Some(e) = self
            .events
            .get_mut(&key.target_user)
            .and_then(|p| p.get_mut(&key.target_package))
            .and_then(|e| e.iter_mut().find(|(k, _)| *k == key))
        {
            e.1 = times;
        }
    }

    /// `reportNotificationEvent`.
    fn notified(
        &mut self,
        event: Notified,
        package: &str,
        user: i32,
        at: i64,
        c: &Constants,
        now: i64,
    ) {
        let Some(events) = self.events.get_mut(&user).and_then(|p| p.remove(package)) else {
            return;
        };
        let mut kept = Vec::new();
        for (key, mut times) in events.into_iter().rev() {
            self.prune(&key, &mut times, c, now);
            let mut session_end = 0;
            while let Some(&t) = times.front() {
                if t > at {
                    break;
                }
                if at - t <= c.response_window_ms && t >= session_end {
                    if session_end != 0 && !c.note_all_sessions {
                        break;
                    }
                    let stats = self.stats_of(&key);
                    stats.dispatched += 1;
                    match event {
                        Notified::Posted => stats.posted += 1,
                        Notified::Updated => stats.updated += 1,
                        Notified::Cancelled => stats.cancelled += 1,
                    }
                    session_end = t + c.sessions_with_response_ms;
                }
                times.pop_front();
            }
            if !times.is_empty() {
                kept.push((key, times));
            }
        }
        if !kept.is_empty() {
            kept.reverse();
            self.events
                .entry(user)
                .or_default()
                .insert(package.to_string(), kept);
        }
    }

    /// `queryBroadcastResponseStats`: the caller's stats of `user`, of
    /// `package` (any if none) and `id` (any if 0), as
    /// `populateAllBroadcastResponseStats` walks them.
    fn query(&self, uid: i32, package: Option<&str>, id: i64, user: i32) -> Vec<Stats> {
        let Some(list) = self.stats.get(&uid).and_then(|u| u.get(&user)) else {
            return Vec::new();
        };
        list.iter()
            .rev()
            .filter(|(k, _)| id == 0 || k.id == id)
            .filter(|(k, _)| package.is_none_or(|p| p == k.target_package))
            .map(|(_, s)| s.clone())
            .collect()
    }

    /// `clearBroadcastResponseStats`.
    fn clear_stats(&mut self, uid: i32, package: Option<&str>, id: i64, user: i32) {
        if let Some(list) = self.stats.get_mut(&uid).and_then(|u| u.get_mut(&user)) {
            list.retain(|(k, _)| {
                !((id == 0 || k.id == id) && package.is_none_or(|p| p == k.target_package))
            });
        }
    }

    /// `clearBroadcastEvents`: the caller's events of `user`.
    fn clear_events(&mut self, uid: i32, user: i32) {
        if let Some(packages) = self.events.get_mut(&user) {
            for events in packages.values_mut() {
                events.retain(|(k, _)| k.source_uid != uid);
            }
            packages.retain(|_, e| !e.is_empty());
        }
    }
}

/// The tracker.
#[derive(Default)]
pub(super) struct BroadcastResponses {
    state: Mutex<State>,
}

/// `SystemClock.elapsedRealtime`.
fn elapsed_ms() -> i64 {
    super::clock_ms(libc::CLOCK_MONOTONIC_RAW)
}

impl AppHost {
    fn response_constants(&self) -> Result<Constants, Exception> {
        let inputs = self.inputs()?;
        Ok(Constants::read(&|key: &str| {
            inputs
                .settings
                .get(Table::Config, &format!("app_standby/{key}"))
        }))
    }

    /// `getPackageUidAsUser`.
    fn uid_of(&self, package: &str, user: i32) -> Result<Option<i32>, Exception> {
        let (state, _) = self.packages()?;
        Ok(state
            .packages
            .get(package)
            .filter(|ps| ps.users.get(&user).is_none_or(|u| u.installed))
            .map(|ps| user * PER_USER_RANGE as i32 + ps.app_id))
    }

    /// `doesPackageHoldExemptedRole` (the role holders of user 0, the
    /// app host's only user) or `doesPackageHoldExemptedPermission`.
    fn response_exempted(
        &self,
        package: &str,
        user: i32,
        c: &Constants,
    ) -> Result<bool, Exception> {
        if let Some(roles) = self.role_service.get() {
            for role in &c.exempted_roles {
                if user == 0 && roles.holders_of(role)?.iter().any(|h| h == package) {
                    return Ok(true);
                }
            }
        }
        let Some(uid) = self.uid_of(package, user)? else {
            return Ok(false);
        };
        for permission in &c.exempted_permissions {
            if self.check_permission(Some(permission), uid)? == 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `UsageStatsManagerInternal.reportBroadcastDispatched` of a
    /// broadcast of `source_uid` with `id` to `package` of `user`, whose
    /// process is in `process_state`.
    pub(super) fn report_broadcast_dispatched(
        &self,
        source_uid: i32,
        package: &str,
        user: i32,
        id: i64,
        process_state: i32,
    ) -> Result<(), Exception> {
        let c = self.response_constants()?;
        if process_state <= c.fg_threshold_state || self.response_exempted(package, user, &c)? {
            return Ok(());
        }
        let now = elapsed_ms();
        let key = Key {
            source_uid,
            target_package: package.to_string(),
            target_user: user,
            id,
        };
        self.broadcast_responses
            .state
            .lock()
            .unwrap()
            .dispatched(key, now, &c, now);
        Ok(())
    }

    /// `reportNotificationPosted`, `Updated` or `Removed` of `package`'s
    /// notification of `user`.
    pub(super) fn report_notification(&self, event: Notified, package: &str, user: i32) {
        let c = match self.response_constants() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("app host: broadcast responses: {}", e.message);
                return;
            }
        };
        let now = elapsed_ms();
        self.broadcast_responses
            .state
            .lock()
            .unwrap()
            .notified(event, package, user, now, &c, now);
    }

    /// The checks of the broadcast response calls: the calling package,
    /// a non-negative id, `permission`, and the user `handleIncomingUser`
    /// gives.
    fn response_call(
        &self,
        uid: i32,
        calling_package: Option<&str>,
        id: i64,
        user: i32,
        name: &str,
    ) -> Result<i32, Exception> {
        if calling_package.is_none() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "callingPackage",
            ));
        }
        if id < 0 {
            return Err(Exception::illegal_argument("id needs to be >=0"));
        }
        if self.check_permission(Some(ACCESS_BROADCAST_RESPONSE_STATS), uid)? != 0 {
            return Err(Exception::security(format!(
                "{name}: Neither user {uid} nor current process has {ACCESS_BROADCAST_RESPONSE_STATS}."
            )));
        }
        self.handle_incoming_user(
            uid,
            user,
            false,
            activity_user::AllowMode::NonFull,
            Some(name),
            None,
        )
    }

    /// `queryBroadcastResponseStats`.
    pub(super) fn query_broadcast_response_stats(
        &self,
        uid: i32,
        package: Option<&str>,
        id: i64,
        calling_package: Option<&str>,
        user: i32,
    ) -> Result<StatsList, Exception> {
        let user = self.response_call(
            uid,
            calling_package,
            id,
            user,
            "queryBroadcastResponseStats",
        )?;
        let state = self.broadcast_responses.state.lock().unwrap();
        let list = StatsList(state.query(uid, package, id, user));
        // `Parcel.BLOB_INPLACE_LIMIT`: a larger blob goes in ashmem.
        if list.marshalled().data().len() > 16 * 1024 {
            return Err(self.unimplemented_named(
                "usagestats",
                "broadcast response stats beyond an in-place blob (#1723)",
            ));
        }
        Ok(list)
    }

    /// `clearBroadcastResponseStats`.
    pub(super) fn clear_broadcast_response_stats(
        &self,
        uid: i32,
        package: Option<&str>,
        id: i64,
        calling_package: Option<&str>,
        user: i32,
    ) -> Result<(), Exception> {
        let user = self.response_call(
            uid,
            calling_package,
            id,
            user,
            "clearBroadcastResponseStats",
        )?;
        let mut state = self.broadcast_responses.state.lock().unwrap();
        state.clear_stats(uid, package, id, user);
        Ok(())
    }

    /// `clearBroadcastEvents` (its user named as the original names it).
    pub(super) fn clear_broadcast_events(
        &self,
        uid: i32,
        calling_package: Option<&str>,
        user: i32,
    ) -> Result<(), Exception> {
        if calling_package.is_none() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "callingPackage",
            ));
        }
        if self.check_permission(Some(ACCESS_BROADCAST_RESPONSE_STATS), uid)? != 0 {
            return Err(Exception::security(format!(
                "clearBroadcastEvents: Neither user {uid} nor current process has {ACCESS_BROADCAST_RESPONSE_STATS}."
            )));
        }
        let user = self.handle_incoming_user(
            uid,
            user,
            false,
            activity_user::AllowMode::NonFull,
            Some("clearBroadcastResponseStats"),
            None,
        )?;
        self.broadcast_responses
            .state
            .lock()
            .unwrap()
            .clear_events(uid, user);
        Ok(())
    }

    /// `isPackageExemptedFromBroadcastResponseStats` (DUMP).
    pub(super) fn package_exempted_from_broadcast_response_stats(
        &self,
        uid: i32,
        calling_package: Option<&str>,
        user: i32,
    ) -> Result<bool, Exception> {
        let Some(package) = calling_package else {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "callingPackage",
            ));
        };
        if self.check_permission(Some(DUMP), uid)? != 0 {
            return Err(Exception::security(format!(
                "isPackageExemptedFromBroadcastResponseStats: Neither user {uid} nor current process has {DUMP}."
            )));
        }
        let c = self.response_constants()?;
        self.response_exempted(package, user, &c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constants() -> Constants {
        Constants::read(&|_| None)
    }

    fn key(id: i64) -> Key {
        Key {
            source_uid: 10_100,
            target_package: "com.t".into(),
            target_user: 0,
            id,
        }
    }

    /// A notification within the window answers each broadcast session
    /// once; later ones count as further sessions.
    #[test]
    fn notifications_answer_the_broadcasts_before_them() {
        let c = constants();
        let mut s = State::default();
        s.dispatched(key(7), 1_000, &c, 1_000);
        s.dispatched(key(7), 2_000, &c, 2_000);
        s.notified(Notified::Posted, "com.t", 0, 5_000, &c, 5_000);
        let stats = s.query(10_100, None, 0, 0);
        assert_eq!(stats.len(), 1);
        assert_eq!(
            (stats[0].dispatched, stats[0].posted, stats[0].id),
            (1, 1, 7)
        );
        // The events were consumed.
        assert!(s.events.get(&0).is_none_or(|p| p.is_empty()));
    }

    /// A dispatch older than the window counts as an unanswered session
    /// when the next one comes.
    #[test]
    fn unanswered_broadcasts_count_once_the_window_passes() {
        let c = constants();
        let mut s = State::default();
        s.dispatched(key(3), 0, &c, 0);
        let later = DEFAULT_RESPONSE_WINDOW_MS + 1;
        s.dispatched(key(3), later, &c, later);
        let stats = s.query(10_100, Some("com.t"), 3, 0);
        assert_eq!((stats[0].dispatched, stats[0].posted), (1, 0));
        s.clear_stats(10_100, None, 3, 0);
        assert!(s.query(10_100, None, 0, 0).is_empty());
        // Another sender sees none.
        assert!(s.query(10_200, None, 0, 0).is_empty());
    }

    #[test]
    fn the_list_is_a_blob_of_a_typed_list() {
        let list = StatsList(vec![Stats {
            package: "p".into(),
            id: 1,
            dispatched: 2,
            ..Default::default()
        }]);
        let mut p = Parcel::new();
        list.write_to(&mut p);
        let mut r = p.reader();
        let len = r.read_i32().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 1));
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("p"));
        assert!(len > 0);
    }
}
