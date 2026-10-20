//! `usagestats`: UsageStatsService for the app host (`android-16.0.0_r1`):
//! whether app standby is on, whether an app is inactive and an app's own
//! standby bucket, as AppStandbyController answers them (`app_standby`);
//! the usage events and stats of the current intervals (`usage_events`):
//! `queryEvents`, `queryEventsForUser`, `queryEventsForPackage`,
//! `queryEventsForPackageForUser` and `queryUsageStats`. Another app's
//! bucket and the rest are not implemented (#1723).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_usage_iusagestatsmanager as us;

use super::usage_database::{self, DEFAULT_SELECTION_LOG_RETENTION_LEN, Init};
use super::usage_events::{self, Clocks, Event, Extra, INTERVAL_DAILY, Notified, UserStats};
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
const ACCESS_LOCUS_ID_USAGE_STATS: &str = "android.permission.ACCESS_LOCUS_ID_USAGE_STATS";
const MANAGE_NOTIFICATIONS: &str = "android.permission.MANAGE_NOTIFICATIONS";

/// `System.currentTimeMillis`.
pub(super) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `UsageStatsService.FLUSH_INTERVAL`.
const FLUSH_INTERVAL: Duration = Duration::from_secs(20 * 60);
/// `UsageStatsDatabase.BACKUP_VERSION`, which the migration status file
/// records.
const BACKUP_VERSION: i32 = 4;

/// The clocks usage stats read (`SystemClock.elapsedRealtime`,
/// `System.currentTimeMillis`).
fn clocks() -> Clocks {
    (super::alarm_service::elapsed_ms(), now_millis())
}

impl AppHost {
    /// `f` over user 0's usage stats, unlocked when first needed
    /// (`onUserUnlocked`), and what they then ask of the service done.
    pub(super) fn with_usage<T>(
        &self,
        f: impl FnOnce(&mut UserStats, Clocks, &mut Notified) -> T,
    ) -> Result<T, Exception> {
        let mut notified = Notified::default();
        let mut new_update = false;
        let result = {
            let mut usage = self.usage.lock().unwrap();
            if usage.is_none() {
                let (user, init, unlocked) = self.unlock_usage()?;
                *usage = Some(user);
                new_update = init.new_update;
                notified = unlocked;
            }
            f(usage.as_mut().unwrap(), clocks(), &mut notified)
        };
        self.usage_notified(notified, new_update);
        Ok(result)
    }

    /// `onUserUnlocked` of user 0's usage stats: the stats migrated to
    /// system CE (none to copy, #1869), no pending events (#1869), the
    /// user's service initialized, the unlock reported and system DE's
    /// stats deleted. `UsageStatsIdleService`'s mappings job is the
    /// original's (#1518).
    fn unlock_usage(&self) -> Result<(UserStats, Init, Notified), Exception> {
        let data = self.data()?;
        let dir = data.join("system_ce/0/usagestats");
        let fail = |e: String| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("usagestats: {e}"),
            )
        };
        let installed = self.usage_installed_packages()?;
        self.migrate_usage_stats(data, &dir).map_err(fail)?;
        let pending = data.join("system_de/0/usagestats");
        if std::fs::read_dir(&pending).is_ok_and(|mut d| d.next().is_some()) {
            return Err(self.unimplemented_named(
                "usagestats",
                "events reported while the user was locked (#1869)",
            ));
        }
        let property = |name: &str| self.property(name).unwrap_or_default();
        let fingerprint = format!(
            "{};{};{}",
            property("ro.build.version.release"),
            property("ro.build.version.codename"),
            property("ro.build.version.incremental"),
        );
        let retention = self
            .property("ro.usagestats.chooser.retention")
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_SELECTION_LOG_RETENTION_LEN);
        let (realtime, now) = clocks();
        let (mut user, init, mut notified) =
            UserStats::init(&dir, (realtime, now), &fingerprint, retention, &installed).map_err(
                |e| match e.contains("(#1866)") {
                    true => self.unimplemented_named("usagestats", &e),
                    false => fail(e),
                },
            )?;
        let unlock = Event {
            package: usage_events::DEVICE_EVENT_PACKAGE_NAME.into(),
            class: None,
            instance_id: 0,
            task_root_package: None,
            task_root_class: None,
            event_type: usage_events::USER_UNLOCKED,
            time_stamp: now,
            flags: 0,
            extra: Extra::None,
        };
        let reported = user.report(unlock, clocks());
        notified.updated |= reported.updated;
        notified.reloaded |= reported.reloaded;
        let _ = std::fs::remove_dir_all(&pending);
        Ok((user, init, notified))
    }

    /// `migrateStatsToSystemCeIfNeededLocked`: the stats directory made;
    /// done once the status file records the migration, else marked done
    /// when there is no legacy directory to copy (#1869).
    fn migrate_usage_stats(&self, data: &Path, dir: &Path) -> Result<(), String> {
        let legacy = data.join("system/usagestats/0");
        usage_database::mkdirs(dir)
            .map_err(|e| format!("Usage stats directory does not exist: {e}"))?;
        let migrated = dir.join("migrated");
        if let Ok(text) = std::fs::read_to_string(&migrated) {
            match text.lines().next().and_then(|l| l.parse::<i32>().ok()) {
                Some(version) if version >= BACKUP_VERSION => {
                    let _ = std::fs::remove_dir_all(&legacy);
                    return Ok(());
                }
                Some(_) => {}
                None => {
                    eprintln!(
                        "app host: usagestats: Failed to read migration status file, possibly corrupted."
                    );
                    std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
                    usage_database::mkdirs(dir).map_err(|e| e.to_string())?;
                }
            }
        }
        if legacy.exists() {
            return Err("copying stats from /data/system/usagestats (#1869)".into());
        }
        usage_database::write_lines(&migrated, &[&BACKUP_VERSION.to_string()])
            .map_err(|e| format!("Failed to write migrated status file: {e}"))
    }

    /// `getInstalledPackages` of user 0, with `MATCH_UNINSTALLED_PACKAGES`:
    /// each package's first install time.
    fn usage_installed_packages(&self) -> Result<HashMap<String, i64>, Exception> {
        let (state, resolution) = self.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let packages = query
            .installed_packages(crate::package::info::flags::MATCH_UNINSTALLED_PACKAGES, 0)
            .map_err(|e| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("usagestats: installed packages: {}", e.0),
                )
            })??;
        Ok(packages
            .into_iter()
            .filter_map(|p| Some((p.package_name?, p.first_install_time)))
            .collect())
    }

    /// `UserUsageStatsService.queryUsageStats`: the packages' stats of each
    /// interval in the range (`queryStats` without events); none for an
    /// invalid range.
    pub(super) fn usage_stats_in_range(
        &self,
        interval: i32,
        begin: i64,
        end: i64,
    ) -> Result<Option<Vec<usage_events::UsageStats>>, Exception> {
        self.with_usage(|user, clocks, notified| {
            let now = user.check_and_get_time(clocks, notified);
            if !(begin <= now && begin < end) {
                return None;
            }
            user.query_stats(interval, begin, end, true, |stats, out| {
                out.extend(stats.package_stats.values().cloned());
                true
            })
        })
    }

    /// `reportEvent` of an event the app host reports for user 0.
    pub(super) fn report_usage_stats_event(&self, event: Event) {
        let reported = self.with_usage(|user, clocks, notified| {
            let n = user.report(event, clocks);
            notified.updated |= n.updated;
            notified.reloaded |= n.reloaded;
        });
        if let Err(e) = reported {
            eprintln!("app host: usagestats: {}", e.message);
        }
    }

    /// The service's listener of the user's stats: a flush to disk after
    /// `FLUSH_INTERVAL` when they first change (`onStatsUpdated`), the
    /// apps' standby buckets checked when they reload (`onStatsReloaded`),
    /// and the system apps' standby defaults after a system update
    /// (`onNewUpdate`).
    fn usage_notified(&self, notified: Notified, new_update: bool) {
        let me = self.me.clone();
        if notified.updated {
            std::thread::spawn(move || {
                std::thread::sleep(FLUSH_INTERVAL);
                if let Some(host) = me.upgrade() {
                    let _ = host.with_usage(|user, _, _| user.persist());
                }
            });
        }
        if notified.reloaded || new_update {
            let me = self.me.clone();
            std::thread::spawn(move || {
                if let Some(host) = me.upgrade() {
                    host.standby_usage_notified(new_update);
                }
            });
        }
    }
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

    /// The daily events in `[begin, end)` (`queryStats` of
    /// `INTERVAL_DAILY`), when the range is valid (`validRange`).
    fn events_in_range(
        &self,
        begin: i64,
        end: i64,
        keep: impl Fn(&Event) -> bool,
    ) -> Result<Option<Vec<Event>>, Exception> {
        self.host.with_usage(|user, clocks, notified| {
            let now = user.check_and_get_time(clocks, notified);
            if !(begin <= now && begin < end) {
                return None;
            }
            user.query_stats(INTERVAL_DAILY as i32, begin, end, false, |stats, out| {
                for e in &stats.events[stats.first_index_on_or_after(begin)..] {
                    if e.time_stamp >= end {
                        return false;
                    }
                    if keep(e) {
                        out.push(e.clone());
                    }
                }
                true
            })
        })
    }

    /// `UserUsageStatsService.queryEvents` with `queryEventsHelper`'s
    /// flags: shortcut invocations hidden from a caller without shortcut
    /// host permission (not implemented, #1870), locus events from one
    /// without ACCESS_LOCUS_ID_USAGE_STATS, notification events' channels
    /// obfuscated for one without MANAGE_NOTIFICATIONS, instant apps for
    /// one that may not see them.
    fn query_events(&self, uid: i32, begin: i64, end: i64) -> Result<Option<Written>, Exception> {
        let Some(events) = self.events_in_range(begin, end, |_| true)? else {
            return Ok(None);
        };
        let permitted = |permission: &str| -> Result<bool, Exception> {
            Ok(uid == SYSTEM_UID || self.host.check_permission(Some(permission), uid)? == 0)
        };
        if events
            .iter()
            .any(|e| e.event_type == usage_events::SHORTCUT_INVOCATION)
        {
            return Err(self.host.unimplemented_named(
                "usagestats",
                "shortcut invocation events for a caller (#1870)",
            ));
        }
        let hide_locus = events
            .iter()
            .any(|e| e.event_type == usage_events::LOCUS_ID_SET)
            && !permitted(ACCESS_LOCUS_ID_USAGE_STATS)?;
        let obfuscate_notifications =
            events.iter().any(Event::is_notification) && !permitted(MANAGE_NOTIFICATIONS)?;
        let obfuscate_instant = events
            .iter()
            .any(|e| e.flags & usage_events::FLAG_IS_PACKAGE_INSTANT_APP != 0)
            && !self.can_access_instant_apps(uid)?;
        let events: Vec<Event> = events
            .into_iter()
            .filter(|e| !(hide_locus && e.event_type == usage_events::LOCUS_ID_SET))
            .map(|e| match obfuscate_notifications && e.is_notification() {
                true => e.obfuscated_notification_event(),
                false => e,
            })
            .map(|e| match obfuscate_instant {
                true => e.obfuscated_if_instant_app(),
                false => e,
            })
            .collect();
        if events.is_empty() {
            return Ok(None);
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
        match self
            .events_in_range(begin, end, |e| e.package == package)?
            .filter(|e| !e.is_empty())
        {
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
            .map_err(|what| self.host.unimplemented_named("usagestats", what))?;
        Ok(Written(p))
    }

    /// `queryUsageStats`: each interval's stats in the range, the instant
    /// apps' names obfuscated by their current state for a caller that may
    /// not see them.
    fn query_usage_stats(
        &self,
        uid: i32,
        interval: i32,
        begin: i64,
        end: i64,
    ) -> Result<Option<Vec<usage_events::UsageStats>>, Exception> {
        let Some(mut stats) = self.host.usage_stats_in_range(interval, begin, end)? else {
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
