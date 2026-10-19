//! `usagestats`: UsageStatsService for the app host (`android-16.0.0_r1`):
//! whether app standby is on, whether an app is inactive and an app's own
//! standby bucket, as AppStandbyController answers them (`app_standby`);
//! the usage events and stats of the current intervals (`usage_events`):
//! `queryEvents`, `queryEventsForUser`, `queryEventsForPackage`,
//! `queryEventsForPackageForUser` and `queryUsageStats`. Another app's
//! bucket and the rest are not implemented (#1723).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_usage_iusagestatsmanager as us;

use super::usage_events::{
    self, Event, INTERVAL_BEST, INTERVAL_COUNT, INTERVAL_DAILY, INTERVAL_LENGTH, IntervalStats,
};
use super::{AppHost, reply};
use crate::package::list_slice::Slice;
use crate::package::query::Query;

const SYSTEM_UID: i32 = 1000;
/// `AppOpsManager.OP_GET_USAGE_STATS`.
const OP_GET_USAGE_STATS: i32 = 43;
const PACKAGE_USAGE_STATS: &str = "android.permission.PACKAGE_USAGE_STATS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const ACCESS_INSTANT_APPS: &str = "android.permission.ACCESS_INSTANT_APPS";
const VIEW_INSTANT_APPS: &str = "android.permission.VIEW_INSTANT_APPS";
const PER_USER_RANGE: i32 = 100_000;
const FIRST_APPLICATION_UID: i32 = 10_000;
/// The flag of `UsageEvents`' parcelled form.
const USE_PARCELED_LIST: &str = "android.app.usage.use_parceled_list";
/// `UsageStatsDatabase`'s interval directories.
const INTERVAL_DIRS: [&str; INTERVAL_COUNT] = ["daily", "weekly", "monthly", "yearly"];

/// `System.currentTimeMillis`.
pub(super) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The begin times of the stats files of each interval on disk
/// (`mSortedStatFiles`), which are named by them.
fn stat_files(dir: &Path) -> [Vec<i64>; INTERVAL_COUNT] {
    std::array::from_fn(|i| {
        let mut times: Vec<i64> = std::fs::read_dir(dir.join(INTERVAL_DIRS[i]))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        times.sort_unstable();
        times
    })
}

/// `findBestFitBucket`: the interval with a file beginning closest on or
/// before `begin`; -1 with none.
fn best_fit_bucket(files: &[Vec<i64>; INTERVAL_COUNT], begin: i64) -> i32 {
    let mut best = -1;
    let mut smallest = i64::MAX;
    for i in (0..INTERVAL_COUNT).rev() {
        if let Some(&key) = files[i].iter().rev().find(|&&t| t <= begin) {
            let diff = (key - begin).abs();
            if diff < smallest {
                smallest = diff;
                best = i as i32;
            }
        }
    }
    best
}

/// A parcel written ahead, written as is.
struct Written(Parcel);

impl WriteParcelable for Written {
    fn write_to(&self, p: &mut Parcel) {
        p.write_raw(self.0.data(), &[]);
    }
}

pub struct UsageStats {
    host: Arc<AppHost>,
}

impl UsageStats {
    pub fn new(host: Arc<AppHost>) -> Self {
        UsageStats { host }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("usagestats", &format!("{what} (#1723)"))
    }

    /// `hasQueryPermission`: the system, or a caller allowed
    /// `OP_GET_USAGE_STATS` (noted), by default holding
    /// `PACKAGE_USAGE_STATS`.
    fn has_query_permission(&self, uid: i32, package: &str) -> Result<bool, Exception> {
        if uid == SYSTEM_UID {
            return Ok(true);
        }
        match super::appops_service::note(&self.host, OP_GET_USAGE_STATS, uid, Some(package))? {
            super::appops_service::MODE_ERRORED => Err(Exception::security(format!(
                "{package} from uid {uid} not allowed to perform GET_USAGE_STATS"
            ))),
            super::appops_service::MODE_DEFAULT => {
                Ok(self.host.check_permission(Some(PACKAGE_USAGE_STATS), uid)? == 0)
            }
            mode => Ok(mode == super::appops_service::MODE_ALLOWED),
        }
    }

    /// `isAppInactive`: of the caller's own package, or any for a caller
    /// that may query usage; an instant app, which only some callers may
    /// see, is not implemented.
    fn app_inactive(
        &self,
        uid: i32,
        package: &str,
        user: i32,
        calling_package: &str,
    ) -> Result<bool, Exception> {
        let user = self.host.incoming_user(uid, user, false)?;
        if package == calling_package {
            let (state, resolution) = self.host.packages()?;
            let query = Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: SYSTEM_UID,
            };
            let actual = query
                .package_uid(calling_package, 0, user)
                .map_err(|e| self.unimplemented(&format!("isAppInactive: {}", e.0)))??;
            if actual != uid {
                return Ok(false);
            }
        } else if !self.has_query_permission(uid, calling_package)? {
            return Ok(false);
        }
        let (state, _) = self.host.packages()?;
        if state
            .packages
            .get(package)
            .and_then(|ps| ps.users.get(&user))
            .is_some_and(|u| u.instant_app)
        {
            return Err(self.unimplemented("isAppInactive of an instant app"));
        }
        self.host.app_idle_filtered(package, user)
    }

    /// `getAppStandbyBucket`: of the caller's own package, or any for a
    /// caller that may query usage (`hasQueryPermission`).
    fn app_standby_bucket(&self, uid: i32, package: &str, user: i32) -> Result<i32, Exception> {
        let user = self.host.incoming_user(uid, user, false)?;
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let package_uid = query
            .package_uid(package, 0, user)
            .map_err(|e| self.unimplemented(&format!("getAppStandbyBucket: {}", e.0)))??;
        if package_uid != uid && uid != SYSTEM_UID {
            return Err(self.unimplemented("getAppStandbyBucket of another app"));
        }
        if package_uid < 0 {
            return Err(Exception::illegal_argument(format!(
                "Cannot get standby bucket for non existent package ({package})"
            )));
        }
        self.host.app_standby_bucket(package, user)
    }
}

impl UsageStats {
    /// `queryStats` of the current interval: `f` over it when it is in
    /// range. The part before it is on disk (#1842): a range that a stats
    /// file there may cover is not implemented, a file ending within its
    /// interval's length.
    fn query_stats<T>(
        &self,
        interval: i32,
        begin: i64,
        end: i64,
        f: impl FnOnce(&IntervalStats) -> T,
    ) -> Result<Option<T>, Exception> {
        let now = now_millis();
        // `validRange`.
        if !(begin <= now && begin < end) {
            return Ok(None);
        }
        let files = stat_files(&self.host.data()?.join("system_ce/0/usagestats"));
        let interval = match interval {
            INTERVAL_BEST => match best_fit_bucket(&files, begin) {
                i if i < 0 => INTERVAL_DAILY as i32,
                i => i,
            },
            i => i,
        };
        if interval < 0 || interval >= INTERVAL_COUNT as i32 {
            return Ok(None);
        }
        let interval = interval as usize;
        let mut usage = self.host.usage.lock().unwrap();
        let user = usage.get_or_insert_with(|| usage_events::UserStats::new(now));
        let current = &user.current[interval];
        if begin >= current.end_time {
            return Ok(None);
        }
        let truncated_end = current.begin_time.min(end);
        if files[interval]
            .iter()
            .any(|&t| t < truncated_end && t + INTERVAL_LENGTH[interval] > begin)
        {
            return Err(self.host.unimplemented_named(
                "usagestats",
                "the stats of earlier intervals on disk (#1842)",
            ));
        }
        Ok((begin < current.end_time && end > current.begin_time).then(|| f(current)))
    }

    /// `canAccessInstantApps`, asked only when an instant app's events
    /// or stats are in the answer.
    fn can_access_instant_apps(&self, uid: i32) -> Result<bool, Exception> {
        if uid < FIRST_APPLICATION_UID
            || self.host.check_permission(Some(ACCESS_INSTANT_APPS), uid)? == 0
        {
            return Ok(true);
        }
        if self.host.check_permission(Some(VIEW_INSTANT_APPS), uid)? == 0 {
            return Err(self.host.unimplemented_named(
                "usagestats",
                "instant apps' usage for a holder of VIEW_INSTANT_APPS (#1843)",
            ));
        }
        Ok(false)
    }

    /// `UserUsageStatsService.queryEvents` with `queryEventsHelper`'s
    /// flags: the app host records no shortcut, locus or notification
    /// events, so only instant apps' obfuscation applies.
    fn query_events(&self, uid: i32, begin: i64, end: i64) -> Result<Option<Written>, Exception> {
        let events = self.query_stats(INTERVAL_DAILY as i32, begin, end, |stats| {
            stats.events[stats.first_index_on_or_after(begin)..]
                .iter()
                .take_while(|e| e.time_stamp < end)
                .cloned()
                .collect::<Vec<Event>>()
        })?;
        let Some(mut events) = events.filter(|e| !e.is_empty()) else {
            return Ok(None);
        };
        if events
            .iter()
            .any(|e| e.flags & usage_events::FLAG_IS_PACKAGE_INSTANT_APP != 0)
            && !self.can_access_instant_apps(uid)?
        {
            events = events
                .iter()
                .map(Event::obfuscated_if_instant_app)
                .collect();
        }
        self.usage_events(&events, true).map(Some)
    }

    /// `UserUsageStatsService.queryEventsForPackage`.
    fn query_events_for_package(
        &self,
        begin: i64,
        end: i64,
        package: &str,
        include_task_root: bool,
    ) -> Result<Option<Written>, Exception> {
        let events = self.query_stats(INTERVAL_DAILY as i32, begin, end, |stats| {
            stats.events[stats.first_index_on_or_after(begin)..]
                .iter()
                .take_while(|e| e.time_stamp < end)
                .filter(|e| e.package == package)
                .cloned()
                .collect::<Vec<Event>>()
        })?;
        match events.filter(|e| !e.is_empty()) {
            Some(events) => self.usage_events(&events, include_task_root).map(Some),
            None => Ok(None),
        }
    }

    /// `UsageEvents` of `events`, as the image's `use_parceled_list`
    /// parcels it.
    fn usage_events(
        &self,
        events: &[Event],
        include_task_roots: bool,
    ) -> Result<Written, Exception> {
        let mut p = Parcel::new();
        let parceled_list = self.host.flag(USE_PARCELED_LIST)?;
        usage_events::write_usage_events(&mut p, events, include_task_roots, parceled_list)
            .ok_or_else(|| {
                self.host.unimplemented_named(
                    "usagestats",
                    "usage events beyond one transaction (#1843)",
                )
            })?;
        Ok(Written(p))
    }

    /// `queryUsageStats`, the instant apps' names obfuscated by their
    /// current state for a caller that may not see them.
    fn query_usage_stats(
        &self,
        uid: i32,
        interval: i32,
        begin: i64,
        end: i64,
    ) -> Result<Option<Vec<usage_events::UsageStats>>, Exception> {
        let stats = self.query_stats(interval, begin, end, |stats| {
            stats.package_stats.values().cloned().collect::<Vec<_>>()
        })?;
        let Some(mut stats) = stats else {
            return Ok(None);
        };
        let (state, _) = self.host.packages()?;
        let instant = |package: &str| {
            state
                .packages
                .get(package)
                .and_then(|ps| ps.users.get(&0))
                .is_some_and(|u| u.instant_app)
        };
        if stats.iter().any(|s| instant(&s.package)) && !self.can_access_instant_apps(uid)? {
            for s in &mut stats {
                if instant(&s.package) {
                    *s = s.obfuscated_for_instant_app();
                }
            }
        }
        Ok(Some(stats))
    }

    /// `checkCallerIsSameApp`: `package` is the caller's.
    fn check_caller_is_same_app(&self, uid: i32, package: &str) -> Result<(), Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let actual = query
            .package_uid(package, 0, uid / PER_USER_RANGE)
            .map_err(|e| self.unimplemented(&format!("checkCallerIsSameApp: {}", e.0)))??;
        if actual != uid {
            return Err(Exception::security(format!(
                "Calling uid {uid} cannot query eventsfor package {package}"
            )));
        }
        Ok(())
    }

    /// Another user than the caller's needs INTERACT_ACROSS_USERS_FULL.
    fn enforce_other_user(&self, uid: i32, user: i32) -> Result<(), Exception> {
        if user != uid / PER_USER_RANGE
            && self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "No permission to query usage stats for this user: Neither user {uid} nor current process has {INTERACT_ACROSS_USERS_FULL}."
            )));
        }
        Ok(())
    }
}

impl Service for UsageStats {
    fn descriptor(&self) -> &str {
        us::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        match call.code {
            us::IS_APP_STANDBY_ENABLED => {
                us::IsAppStandbyEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.host.app_idle_enabled() {
                    Ok(enabled) => us::write_is_app_standby_enabled_reply(&mut p, enabled),
                    Err(e) => return reply(Err(e)),
                }
            }
            us::IS_APP_INACTIVE => {
                let a = us::IsAppInactive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(package) = a.package_name.as_deref() else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "packageName")));
                };
                match self.app_inactive(
                    uid,
                    package,
                    a.user_id,
                    a.calling_package.as_deref().unwrap_or_default(),
                ) {
                    Ok(inactive) => us::write_is_app_inactive_reply(&mut p, inactive),
                    Err(e) => return reply(Err(e)),
                }
            }
            us::GET_APP_STANDBY_BUCKET => {
                let a = us::GetAppStandbyBucket::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.app_standby_bucket(
                    uid,
                    a.package_name.as_deref().unwrap_or_default(),
                    a.user_id,
                ) {
                    Ok(bucket) => us::write_get_app_standby_bucket_reply(&mut p, bucket),
                    Err(e) => return reply(Err(e)),
                }
            }
            us::QUERY_EVENTS => {
                let a = us::QueryEvents::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.calling_package.as_deref().unwrap_or_default();
                let events = match self.has_query_permission(uid, package) {
                    Ok(true) => self.query_events(uid, a.begin_time, a.end_time),
                    Ok(false) => Ok(None),
                    Err(e) => Err(e),
                };
                match events {
                    Ok(events) => us::write_query_events_reply(&mut p, events.as_ref()),
                    Err(e) => return reply(Err(e)),
                }
            }
            // User 0 is the app host's only user (`queryEventsHelper` of
            // another finds it stopped: none).
            us::QUERY_EVENTS_FOR_USER => {
                let a = us::QueryEventsForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.calling_package.as_deref().unwrap_or_default();
                let events = match self.has_query_permission(uid, package) {
                    Ok(true) => {
                        self.enforce_other_user(uid, a.user_id)
                            .and_then(|()| match a.user_id {
                                0 => self.query_events(uid, a.begin_time, a.end_time),
                                _ => Ok(None),
                            })
                    }
                    Ok(false) => Ok(None),
                    Err(e) => Err(e),
                };
                match events {
                    Ok(events) => us::write_query_events_for_user_reply(&mut p, events.as_ref()),
                    Err(e) => return reply(Err(e)),
                }
            }
            us::QUERY_EVENTS_FOR_PACKAGE => {
                let a = us::QueryEventsForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.calling_package.as_deref().unwrap_or_default();
                let events = self.check_caller_is_same_app(uid, package).and_then(|()| {
                    let include_task_root = self.has_query_permission(uid, package)?;
                    self.query_events_for_package(
                        a.begin_time,
                        a.end_time,
                        package,
                        include_task_root,
                    )
                });
                match events {
                    Ok(events) => us::write_query_events_for_package_reply(&mut p, events.as_ref()),
                    Err(e) => return reply(Err(e)),
                }
            }
            us::QUERY_EVENTS_FOR_PACKAGE_FOR_USER => {
                let a = us::QueryEventsForPackageForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let calling = a.calling_package.as_deref().unwrap_or_default();
                let pkg = a.pkg.as_deref().unwrap_or_default();
                let events = match self.has_query_permission(uid, calling) {
                    Ok(true) => self.enforce_other_user(uid, a.user_id).and_then(|()| {
                        // `checkCallerIsSystemOrSameApp`.
                        if uid % PER_USER_RANGE != SYSTEM_UID {
                            self.check_caller_is_same_app(uid, pkg)?;
                        }
                        match a.user_id {
                            0 => self.query_events_for_package(a.begin_time, a.end_time, pkg, true),
                            _ => Ok(None),
                        }
                    }),
                    Ok(false) => Ok(None),
                    Err(e) => Err(e),
                };
                match events {
                    Ok(events) => {
                        us::write_query_events_for_package_for_user_reply(&mut p, events.as_ref())
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            us::QUERY_USAGE_STATS => {
                let a = us::QueryUsageStats::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.calling_package.as_deref().unwrap_or_default();
                let stats = match self.has_query_permission(uid, package) {
                    Ok(true) => self
                        .host
                        .incoming_user(uid, a.user_id, false)
                        .and_then(|user| match user {
                            0 => {
                                self.query_usage_stats(uid, a.bucket_type, a.begin_time, a.end_time)
                            }
                            _ => Ok(None),
                        }),
                    Ok(false) => Ok(None),
                    Err(e) => Err(e),
                };
                match stats {
                    Ok(stats) => {
                        let slice = stats.map(|s| {
                            Slice::new(self.host.process.clone(), "android.app.usage.UsageStats", s)
                        });
                        us::write_query_usage_stats_reply(&mut p, slice.as_ref())
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                let name = us::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                return reply(Err(self.unimplemented(name)));
            }
        }
        Ok(p)
    }
}
