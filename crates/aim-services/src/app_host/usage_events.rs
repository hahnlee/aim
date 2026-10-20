//! UsageStatsService's usage events and stats of user 0 for the app host
//! (`android-16.0.0_r1`): `UserUsageStatsService` over its database
//! (`usage_database`): the latest intervals loaded at the user's unlock
//! (`init`, with the device's shutdown and startup events), the events the
//! app host reports taken into each interval's current `IntervalStats` as
//! `reportEvent` takes them (the daily one's event list, every interval's
//! `UsageStats` per package), rolled over at the daily interval's end,
//! persisted (`persistActiveStats`), and the queries over the intervals on
//! disk and in memory (`queryStats`). The events the app host reports are
//! its activities' and the user's unlock (#1843 for the rest).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

use super::usage_database::{Database, Init};

/// `UsageEvents.Event`'s types.
pub const ACTIVITY_RESUMED: i32 = 1;
pub const ACTIVITY_PAUSED: i32 = 2;
const END_OF_DAY: i32 = 3;
pub const CONFIGURATION_CHANGE: i32 = 5;
const SYSTEM_INTERACTION: i32 = 6;
pub const USER_INTERACTION: i32 = 7;
pub const SHORTCUT_INVOCATION: i32 = 8;
const CHOOSER_ACTION: i32 = 9;
const NOTIFICATION_SEEN: i32 = 10;
pub const STANDBY_BUCKET_CHANGED: i32 = 11;
pub const NOTIFICATION_INTERRUPTION: i32 = 12;
const SCREEN_INTERACTIVE: i32 = 15;
const SCREEN_NON_INTERACTIVE: i32 = 16;
const KEYGUARD_SHOWN: i32 = 17;
const KEYGUARD_HIDDEN: i32 = 18;
pub const ACTIVITY_STOPPED: i32 = 23;
pub const ACTIVITY_DESTROYED: i32 = 24;
const FLUSH_TO_DISK: i32 = 25;
const DEVICE_SHUTDOWN: i32 = 26;
const DEVICE_STARTUP: i32 = 27;
pub const USER_UNLOCKED: i32 = 28;
pub const LOCUS_ID_SET: i32 = 30;
const APP_COMPONENT_USED: i32 = 31;
/// `Event.FLAG_IS_PACKAGE_INSTANT_APP`.
pub const FLAG_IS_PACKAGE_INSTANT_APP: i32 = 1 << 0;
/// `UsageEvents.INSTANT_APP_PACKAGE_NAME`, `INSTANT_APP_CLASS_NAME`,
/// `OBFUSCATED_NOTIFICATION_CHANNEL_ID`.
pub const INSTANT_APP_PACKAGE_NAME: &str = "android.instant_app";
const INSTANT_APP_CLASS_NAME: &str = "android.instant_class";
const OBFUSCATED_NOTIFICATION_CHANNEL_ID: &str = "unknown_channel_id";
/// `Event.DEVICE_EVENT_PACKAGE_NAME`.
pub const DEVICE_EVENT_PACKAGE_NAME: &str = "android";

/// `UsageStatsManager.INTERVAL_*`.
pub const INTERVAL_DAILY: usize = 0;
pub const INTERVAL_COUNT: usize = 4;
pub const INTERVAL_BEST: i32 = 4;
/// `UnixCalendar`'s lengths, `UserUsageStatsService.INTERVAL_LENGTH`.
const DAY_IN_MILLIS: i64 = 24 * 60 * 60 * 1000;
pub const INTERVAL_LENGTH: [i64; INTERVAL_COUNT] = [
    DAY_IN_MILLIS,
    7 * DAY_IN_MILLIS,
    30 * DAY_IN_MILLIS,
    365 * DAY_IN_MILLIS,
];
/// `UsageStatsService.TIME_CHANGE_THRESHOLD_MILLIS`.
const TIME_CHANGE_THRESHOLD_MILLIS: i64 = 2 * 1000;
/// `IntervalStats.CURRENT_MAJOR_VERSION`, `CURRENT_MINOR_VERSION`.
const CURRENT_MAJOR_VERSION: i32 = 1;
const CURRENT_MINOR_VERSION: i32 = 1;

/// What an event of a type carries beyond the fields every event has.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Extra {
    #[default]
    None,
    /// `mConfiguration`, as its `ConfigurationProto`.
    Configuration(Vec<u8>),
    ShortcutId(String),
    /// `mBucketAndReason`.
    Bucket(i32),
    ChannelId(String),
    LocusId(String),
    /// `mExtras` of a user interaction: its category and action.
    Interaction(String, String),
}

/// A `UsageEvents.Event`.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub package: String,
    pub class: Option<String>,
    pub instance_id: i32,
    pub task_root_package: Option<String>,
    pub task_root_class: Option<String>,
    pub event_type: i32,
    pub time_stamp: i64,
    pub flags: i32,
    pub extra: Extra,
}

impl Event {
    /// An event of the device (`DEVICE_EVENT_PACKAGE_NAME`).
    fn device(event_type: i32, time_stamp: i64) -> Event {
        Event {
            package: DEVICE_EVENT_PACKAGE_NAME.into(),
            class: None,
            instance_id: 0,
            task_root_package: None,
            task_root_class: None,
            event_type,
            time_stamp,
            flags: 0,
            extra: Extra::None,
        }
    }

    /// `getObfuscatedIfInstantApp`.
    pub fn obfuscated_if_instant_app(&self) -> Event {
        if self.flags & FLAG_IS_PACKAGE_INSTANT_APP == 0 {
            return self.clone();
        }
        Event {
            package: INSTANT_APP_PACKAGE_NAME.into(),
            class: Some(INSTANT_APP_CLASS_NAME.into()),
            ..self.clone()
        }
    }

    /// `getObfuscatedNotificationEvent`.
    pub fn obfuscated_notification_event(&self) -> Event {
        let extra = match self.extra {
            Extra::ChannelId(_) => Extra::ChannelId(OBFUSCATED_NOTIFICATION_CHANNEL_ID.into()),
            ref e => e.clone(),
        };
        Event {
            extra,
            ..self.clone()
        }
    }

    /// Whether it is a notification event (`NOTIFICATION_SEEN`,
    /// `NOTIFICATION_INTERRUPTION`).
    pub fn is_notification(&self) -> bool {
        matches!(
            self.event_type,
            NOTIFICATION_SEEN | NOTIFICATION_INTERRUPTION
        )
    }
}

/// A package's `UsageStats` in an interval.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageStats {
    pub package: String,
    pub begin_time_stamp: i64,
    pub end_time_stamp: i64,
    pub last_time_used: i64,
    pub last_time_visible: i64,
    pub last_time_component_used: i64,
    pub last_time_foreground_service_used: i64,
    pub total_time_in_foreground: i64,
    pub total_time_visible: i64,
    pub total_time_foreground_service_used: i64,
    pub launch_count: i32,
    pub app_launch_count: i32,
    pub last_event: i32,
    /// `mActivities`: each instance's last event, by instance id.
    pub activities: BTreeMap<i32, i32>,
    /// `mChooserCounts`: each action's counts by category.
    pub chooser_counts: BTreeMap<String, BTreeMap<String, i32>>,
}

impl UsageStats {
    fn has_foreground_activity(&self) -> bool {
        self.activities.values().any(|&e| e == ACTIVITY_RESUMED)
    }

    fn has_visible_activity(&self) -> bool {
        self.activities
            .values()
            .any(|&e| e == ACTIVITY_RESUMED || e == ACTIVITY_PAUSED)
    }

    fn increment_time_used(&mut self, time_stamp: i64) {
        if time_stamp > self.last_time_used {
            self.total_time_in_foreground += time_stamp - self.last_time_used;
            self.last_time_used = time_stamp;
        }
    }

    fn increment_time_visible(&mut self, time_stamp: i64) {
        if time_stamp > self.last_time_visible {
            self.total_time_visible += time_stamp - self.last_time_visible;
            self.last_time_visible = time_stamp;
        }
    }

    /// `updateActivity`.
    fn update_activity(&mut self, time_stamp: i64, event_type: i32, instance_id: i32) {
        match self.activities.get(&instance_id) {
            Some(&ACTIVITY_RESUMED) => {
                self.increment_time_used(time_stamp);
                self.increment_time_visible(time_stamp);
            }
            Some(&ACTIVITY_PAUSED) => self.increment_time_visible(time_stamp),
            _ => {}
        }
        match event_type {
            ACTIVITY_RESUMED => {
                if !self.has_visible_activity() {
                    self.last_time_used = time_stamp;
                    self.last_time_visible = time_stamp;
                } else if !self.has_foreground_activity() {
                    self.last_time_used = time_stamp;
                }
                self.activities.insert(instance_id, event_type);
            }
            ACTIVITY_PAUSED => {
                if !self.has_visible_activity() {
                    self.last_time_visible = time_stamp;
                }
                self.activities.insert(instance_id, event_type);
            }
            _ => {
                self.activities.remove(&instance_id);
            }
        }
    }

    /// `update`, its foreground services left out: the app host reports
    /// none, and they are not persisted.
    fn update(&mut self, time_stamp: i64, event_type: i32, instance_id: i32) {
        match event_type {
            ACTIVITY_RESUMED | ACTIVITY_PAUSED | ACTIVITY_STOPPED | ACTIVITY_DESTROYED => {
                self.update_activity(time_stamp, event_type, instance_id)
            }
            END_OF_DAY | DEVICE_SHUTDOWN | FLUSH_TO_DISK => {
                if self.has_foreground_activity() {
                    self.increment_time_used(time_stamp);
                }
                if self.has_visible_activity() {
                    self.increment_time_visible(time_stamp);
                }
            }
            USER_INTERACTION => {
                if self.has_foreground_activity() {
                    self.increment_time_used(time_stamp);
                } else {
                    self.last_time_used = time_stamp;
                }
                if self.has_visible_activity() {
                    self.increment_time_visible(time_stamp);
                } else {
                    self.last_time_visible = time_stamp;
                }
            }
            APP_COMPONENT_USED => self.last_time_component_used = time_stamp,
            _ => {}
        }
        self.end_time_stamp = time_stamp;
        if event_type == ACTIVITY_RESUMED {
            self.launch_count += 1;
        }
    }

    /// `getObfuscatedForInstantApp`.
    pub fn obfuscated_for_instant_app(&self) -> UsageStats {
        UsageStats {
            package: INSTANT_APP_PACKAGE_NAME.into(),
            ..self.clone()
        }
    }
}

impl WriteParcelable for UsageStats {
    fn write_to(&self, p: &mut Parcel) {
        use crate::bundle::Value;
        p.write_string16(Some(&self.package));
        p.write_i64(self.begin_time_stamp);
        p.write_i64(self.end_time_stamp);
        p.write_i64(self.last_time_used);
        p.write_i64(self.last_time_visible);
        p.write_i64(self.last_time_component_used);
        p.write_i64(self.last_time_foreground_service_used);
        p.write_i64(self.total_time_in_foreground);
        p.write_i64(self.total_time_visible);
        p.write_i64(self.total_time_foreground_service_used);
        p.write_i32(self.launch_count);
        p.write_i32(self.app_launch_count);
        p.write_i32(self.last_event);
        // The chooser counts, a bundle of each action's.
        let counts: Vec<(&str, Value)> = self
            .chooser_counts
            .iter()
            .map(|(action, categories)| {
                let entries = categories
                    .iter()
                    .map(|(c, &n)| (c.clone(), Value::Int(n)))
                    .collect();
                (action.as_str(), Value::Bundle(entries))
            })
            .collect();
        crate::bundle::write(p, &counts);
        p.write_i32(self.activities.len() as i32);
        for (id, event) in &self.activities {
            p.write_i32(*id);
            p.write_i32(*event);
        }
        // No foreground services (`mForegroundServices`).
        crate::bundle::write(p, &[]);
    }
}

/// `IntervalStats.EventTracker`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracker {
    pub cur_start_time: i64,
    pub last_event_time: i64,
    pub duration: i64,
    pub count: i32,
}

impl Tracker {
    fn commit_time(&mut self, time_stamp: i64) {
        if self.cur_start_time != 0 {
            self.duration += time_stamp - self.cur_start_time;
            self.cur_start_time = 0;
        }
    }

    fn update(&mut self, time_stamp: i64) {
        if self.cur_start_time == 0 {
            self.count += 1;
        }
        self.commit_time(time_stamp);
        self.cur_start_time = time_stamp;
        self.last_event_time = time_stamp;
    }
}

/// A configuration's `ConfigurationStats`, the configuration kept as its
/// `ConfigurationProto`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigurationStats {
    pub config: Vec<u8>,
    pub last_time_active: i64,
    pub total_time_active: i64,
    pub activation_count: i32,
}

/// The trackers of `IntervalStats`, in their order: interactive,
/// non-interactive, keyguard shown, keyguard hidden.
const INTERACTIVE: usize = 0;
const NON_INTERACTIVE: usize = 1;
const KEYGUARD_SHOWN_TRACKER: usize = 2;
const KEYGUARD_HIDDEN_TRACKER: usize = 3;

/// An interval's `IntervalStats`.
#[derive(Clone, Debug, PartialEq)]
pub struct IntervalStats {
    pub major_version: i32,
    pub minor_version: i32,
    pub begin_time: i64,
    pub end_time: i64,
    pub last_time_saved: i64,
    pub trackers: [Tracker; 4],
    pub package_stats: BTreeMap<String, UsageStats>,
    /// `configurations`, in the order they were read or made.
    pub configurations: Vec<ConfigurationStats>,
    /// `activeConfiguration`, by its place in `configurations`.
    pub active_configuration: Option<usize>,
    /// `events`, ordered by time (`EventList`).
    pub events: Vec<Event>,
}

impl IntervalStats {
    /// An `IntervalStats` of `begin_time` to be read.
    pub fn empty(begin_time: i64) -> Self {
        IntervalStats {
            major_version: CURRENT_MAJOR_VERSION,
            minor_version: CURRENT_MINOR_VERSION,
            begin_time,
            end_time: 0,
            last_time_saved: 0,
            trackers: Default::default(),
            package_stats: BTreeMap::new(),
            configurations: Vec::new(),
            active_configuration: None,
            events: Vec::new(),
        }
    }

    /// A new interval of `now` (`loadActiveStats` with no good fit).
    fn new(now: i64) -> Self {
        IntervalStats {
            end_time: now + 1,
            ..IntervalStats::empty(now)
        }
    }

    fn usage_stats(&mut self, package: &str) -> &mut UsageStats {
        let begin_time = self.begin_time;
        let end_time = self.end_time;
        self.package_stats
            .entry(package.to_string())
            .or_insert_with(|| UsageStats {
                package: package.into(),
                begin_time_stamp: begin_time,
                end_time_stamp: end_time,
                ..Default::default()
            })
    }

    /// `update`: `DEVICE_SHUTDOWN` and `FLUSH_TO_DISK` to every package.
    fn update(&mut self, package: &str, time_stamp: i64, event_type: i32, instance_id: i32) {
        if event_type == DEVICE_SHUTDOWN || event_type == FLUSH_TO_DISK {
            for s in self.package_stats.values_mut() {
                s.update(time_stamp, event_type, instance_id);
            }
        } else {
            self.usage_stats(package)
                .update(time_stamp, event_type, instance_id);
        }
        if time_stamp > self.end_time {
            self.end_time = time_stamp;
        }
    }

    /// `EventList.insert`: after the events of the same time.
    pub fn add_event(&mut self, event: Event) {
        let at = self
            .events
            .partition_point(|e| e.time_stamp <= event.time_stamp);
        if event.time_stamp > self.end_time {
            self.end_time = event.time_stamp;
        }
        self.events.insert(at, event);
    }

    /// `updateConfigurationStats`.
    fn update_configuration_stats(&mut self, config: Option<&[u8]>, time_stamp: i64) {
        if let Some(active) = self.active_configuration {
            let c = &mut self.configurations[active];
            c.total_time_active += time_stamp - c.last_time_active;
            c.last_time_active = time_stamp - 1;
        }
        if let Some(config) = config {
            // `getOrCreateConfigurationStats`: by `Configuration.equals`.
            let wanted = super::config::Configuration::from_proto(config).ok();
            let at = match self.configurations.iter().position(|c| {
                c.config == config
                    || (wanted.is_some()
                        && super::config::Configuration::from_proto(&c.config).ok() == wanted)
            }) {
                Some(at) => at,
                None => {
                    self.configurations.push(ConfigurationStats {
                        config: config.to_vec(),
                        ..Default::default()
                    });
                    self.configurations.len() - 1
                }
            };
            let c = &mut self.configurations[at];
            c.last_time_active = time_stamp;
            c.activation_count += 1;
            self.active_configuration = Some(at);
        }
        if time_stamp > self.end_time {
            self.end_time = time_stamp;
        }
    }

    /// `commitTime`: the trackers' running times.
    fn commit_time(&mut self, time_stamp: i64) {
        for t in &mut self.trackers {
            t.commit_time(time_stamp);
        }
    }

    /// `firstIndexOnOrAfter`.
    pub fn first_index_on_or_after(&self, time: i64) -> usize {
        self.events.partition_point(|e| e.time_stamp < time)
    }
}

/// What a change of the user's stats asks of its service
/// (`StatsUpdatedListener`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Notified {
    /// `onStatsUpdated`: the first change since they were persisted.
    pub updated: bool,
    /// `onStatsReloaded`.
    pub reloaded: bool,
}

/// `UserUsageStatsService` of user 0.
pub struct UserStats {
    pub current: [IntervalStats; INTERVAL_COUNT],
    daily_expiry: i64,
    last_backgrounded_package: Option<String>,
    db: Database,
    stats_changed: bool,
    /// The chooser counts' retention, in days.
    retention_days: i64,
    /// `mRealTimeSnapshot`, `mSystemTimeSnapshot`.
    real_time_snapshot: i64,
    system_time_snapshot: i64,
}

/// The clocks `UserUsageStatsService` reads: the elapsed realtime and the
/// wall time.
pub type Clocks = (i64, i64);

impl UserStats {
    /// `UserUsageStatsService` and its `init` over the database in `dir`:
    /// the mappings read, the database initialized (pruned after an
    /// upgrade of the installed packages), the latest intervals loaded,
    /// and the shutdown and startup of the device added to the day's
    /// events.
    pub fn init(
        dir: &Path,
        (realtime, now): Clocks,
        fingerprint: &str,
        retention_days: i64,
        installed: &HashMap<String, i64>,
    ) -> Result<(UserStats, Init, Notified), String> {
        let mut db = Database::new(dir);
        db.read_mappings();
        let init = db.init(now, fingerprint, retention_days)?;
        if init.upgrade_performed {
            db.prune_packages_data_on_upgrade(installed);
        }
        let latest: [Option<IntervalStats>; INTERVAL_COUNT] = std::array::from_fn(|i| db.latest(i));
        let all = latest.iter().all(Option::is_some);
        let mut user = UserStats {
            current: latest.map(|s| s.unwrap_or_else(|| IntervalStats::new(now))),
            daily_expiry: 0,
            last_backgrounded_package: None,
            db,
            stats_changed: false,
            retention_days,
            real_time_snapshot: realtime,
            system_time_snapshot: now,
        };
        let mut notified = Notified::default();
        if all {
            user.update_rollover_deadline();
        } else {
            notified.reloaded = user.load_active_stats(now);
        }
        let daily = &mut user.current[INTERVAL_DAILY];
        let shutdown = daily.last_time_saved.max(daily.end_time);
        daily.add_event(Event::device(DEVICE_SHUTDOWN, shutdown));
        daily.add_event(Event::device(DEVICE_STARTUP, now));
        Ok((user, init, notified))
    }

    /// `checkAndGetTimeLocked`: the wall time, the stats moved with it
    /// when it changed by more than the threshold
    /// (`ENABLE_TIME_CHANGE_CORRECTION`).
    pub fn check_and_get_time(&mut self, (realtime, now): Clocks, notified: &mut Notified) -> i64 {
        let expected = (realtime - self.real_time_snapshot) + self.system_time_snapshot;
        if (now - expected).abs() > TIME_CHANGE_THRESHOLD_MILLIS {
            eprintln!(
                "app host: usagestats: User[0] Time changed in by {} seconds",
                (now - expected) / 1000
            );
            // `onTimeChanged`.
            self.persist();
            self.db.on_time_changed(now - expected);
            notified.reloaded |= self.load_active_stats(now);
            self.real_time_snapshot = realtime;
            self.system_time_snapshot = now;
        }
        now
    }

    /// `reportEvent` of an event the app host reports, its time already
    /// the wall time.
    pub fn report(&mut self, mut event: Event, clocks: Clocks) -> Notified {
        let mut notified = Notified::default();
        if event.event_type != USER_INTERACTION && event.event_type != APP_COMPONENT_USED {
            self.check_and_get_time(clocks, &mut notified);
        }
        if event.time_stamp >= self.daily_expiry {
            notified.reloaded |= self.rollover(event.time_stamp);
        }
        let private = matches!(
            event.event_type,
            SYSTEM_INTERACTION
                | ACTIVITY_DESTROYED
                | FLUSH_TO_DISK
                | DEVICE_SHUTDOWN
                | APP_COMPONENT_USED
        );
        // A configuration change's event keeps the delta from the daily
        // interval's active configuration; the stats keep the whole one.
        let mut new_full_config = None;
        if event.event_type == CONFIGURATION_CHANGE
            && let Extra::Configuration(full) = &event.extra
        {
            let full = full.clone();
            let daily = &self.current[INTERVAL_DAILY];
            if let Some(active) = daily.active_configuration
                && let (Ok(base), Ok(change)) = (
                    super::config::Configuration::from_proto(&daily.configurations[active].config),
                    super::config::Configuration::from_proto(&full),
                )
            {
                event.extra = Extra::Configuration(
                    super::config::Configuration::generate_delta(&base, &change).to_proto(),
                );
            }
            new_full_config = Some(full);
        }
        let mut increment_app_launch = false;
        match event.event_type {
            ACTIVITY_RESUMED => {
                increment_app_launch =
                    self.last_backgrounded_package.as_deref() != Some(event.package.as_str())
            }
            ACTIVITY_PAUSED => self.last_backgrounded_package = Some(event.package.clone()),
            _ => {}
        }
        for stats in &mut self.current {
            match event.event_type {
                SCREEN_INTERACTIVE => {
                    stats.trackers[INTERACTIVE].update(event.time_stamp);
                    stats.trackers[NON_INTERACTIVE].commit_time(event.time_stamp);
                }
                SCREEN_NON_INTERACTIVE => {
                    stats.trackers[NON_INTERACTIVE].update(event.time_stamp);
                    stats.trackers[INTERACTIVE].commit_time(event.time_stamp);
                }
                KEYGUARD_SHOWN => {
                    stats.trackers[KEYGUARD_SHOWN_TRACKER].update(event.time_stamp);
                    stats.trackers[KEYGUARD_HIDDEN_TRACKER].commit_time(event.time_stamp);
                }
                KEYGUARD_HIDDEN => {
                    stats.trackers[KEYGUARD_HIDDEN_TRACKER].update(event.time_stamp);
                    stats.trackers[KEYGUARD_SHOWN_TRACKER].commit_time(event.time_stamp);
                }
                CONFIGURATION_CHANGE => {
                    if let Some(full) = &new_full_config {
                        stats.update_configuration_stats(Some(full), event.time_stamp);
                    }
                }
                _ => {
                    stats.update(
                        &event.package,
                        event.time_stamp,
                        event.event_type,
                        event.instance_id,
                    );
                    if increment_app_launch {
                        stats.usage_stats(&event.package).app_launch_count += 1;
                    }
                }
            }
        }
        if !private {
            self.current[INTERVAL_DAILY].add_event(event);
        }
        notified.updated |= self.notify_stats_changed();
        notified
    }

    /// `notifyStatsChanged`: whether it is the first change since the
    /// stats were persisted.
    fn notify_stats_changed(&mut self) -> bool {
        !std::mem::replace(&mut self.stats_changed, true)
    }

    /// `persistActiveStats`: the mappings, then each interval, when they
    /// changed.
    pub fn persist(&mut self) {
        if !self.stats_changed {
            return;
        }
        eprintln!("app host: usagestats: User[0] Flushing usage stats to disk");
        match self.db.persist(&mut self.current) {
            Ok(()) => self.stats_changed = false,
            Err(e) => {
                eprintln!("app host: usagestats: User[0] Failed to persist active stats: {e}")
            }
        }
    }

    /// `rolloverStats`: each interval ends at the day's end, its open
    /// activities ended for the day and its active configuration's time
    /// committed; they are persisted, the database pruned, the intervals
    /// loaded again (one whose length has not passed continues), and the
    /// activities continue into them with the configuration. Whether the
    /// stats were reloaded.
    fn rollover(&mut self, now: i64) -> bool {
        let end = self.daily_expiry - 1;
        let previous_config = {
            let daily = &self.current[INTERVAL_DAILY];
            daily
                .active_configuration
                .map(|at| daily.configurations[at].config.clone())
        };
        let mut continuing: BTreeMap<String, BTreeMap<i32, i32>> = BTreeMap::new();
        for stats in &mut self.current {
            let packages: Vec<String> = stats
                .package_stats
                .values()
                .filter(|s| !s.activities.is_empty())
                .map(|s| s.package.clone())
                .collect();
            for package in packages {
                continuing.insert(
                    package.clone(),
                    stats.package_stats[&package].activities.clone(),
                );
                stats.update(&package, end, END_OF_DAY, 0);
                self.stats_changed = true;
            }
            stats.update_configuration_stats(None, end);
            stats.commit_time(end);
        }
        self.persist();
        self.db.prune(now, self.retention_days);
        let reloaded = self.load_active_stats(now);
        let begin = self.current[INTERVAL_DAILY].begin_time;
        for (package, activities) in continuing {
            for stats in &mut self.current {
                for (&id, &event) in &activities {
                    stats.update(&package, begin, event, id);
                }
                stats.update_configuration_stats(previous_config.as_deref(), begin);
                self.stats_changed = true;
            }
        }
        self.persist();
        reloaded
    }

    /// `loadActiveStats`: each interval's latest stats while its length
    /// has not passed, else a new interval; the rollover deadline set.
    fn load_active_stats(&mut self, now: i64) -> bool {
        for interval in 0..INTERVAL_COUNT {
            self.current[interval] = match self.db.latest(interval) {
                Some(stats) if now < stats.begin_time + INTERVAL_LENGTH[interval] => stats,
                _ => IntervalStats::new(now),
            };
        }
        self.stats_changed = false;
        self.update_rollover_deadline();
        true
    }

    /// `updateRolloverDeadline`: a day after the daily interval's begin.
    fn update_rollover_deadline(&mut self) {
        self.daily_expiry = self.current[INTERVAL_DAILY].begin_time + DAY_IN_MILLIS;
    }

    /// `queryStats`: the interval (the best fit for `INTERVAL_BEST`), then
    /// `combine` over its stats on disk before the current interval and
    /// over the current one where they cover `[begin, end)`; none for a
    /// bad interval or a range after the current one.
    pub fn query_stats<T>(
        &mut self,
        interval: i32,
        begin: i64,
        end: i64,
        skip_events: bool,
        mut combine: impl FnMut(&IntervalStats, &mut Vec<T>) -> bool,
    ) -> Option<Vec<T>> {
        let interval = match interval {
            INTERVAL_BEST => match self.db.best_fit_bucket(begin) {
                i if i < 0 => INTERVAL_DAILY as i32,
                i => i,
            },
            i => i,
        };
        if interval < 0 || interval >= INTERVAL_COUNT as i32 {
            return None;
        }
        let interval = interval as usize;
        let (current_begin, current_end) = {
            let c = &self.current[interval];
            (c.begin_time, c.end_time)
        };
        if begin >= current_end {
            return None;
        }
        let truncated_end = current_begin.min(end);
        let mut results = Vec::new();
        self.db
            .query(interval, begin, truncated_end, skip_events, |stats| {
                combine(stats, &mut results)
            });
        if begin < current_end && end > current_begin {
            self.db.filter_stats(&mut self.current[interval]);
            combine(&self.current[interval], &mut results);
        }
        Some(results)
    }
}

/// `UsageEvents` of `events` with the sorted names they use, written as
/// the original writes it with `use_parceled_list` on
/// (`ParcelableUsageEventList`) or off (a blob of the events against the
/// string table); an error when it does not fit one transaction.
pub fn write_usage_events(
    p: &mut Parcel,
    events: &[Event],
    include_task_roots: bool,
    parceled_list: bool,
) -> Result<(), &'static str> {
    /// `IBinder.getSuggestedMaxIpcSizeBytes`.
    const MAX_IPC_SIZE: usize = 64 * 1024;
    /// `Parcel.BLOB_INPLACE_LIMIT`.
    const BLOB_INPLACE_LIMIT: usize = 16 * 1024;
    const BEYOND_ONE_TRANSACTION: &str = "usage events beyond one transaction (#1843)";
    if parceled_list {
        p.write_i32(events.len() as i32);
        p.write_i32(0);
        p.write_string16(Some("android.app.usage.ParcelableUsageEventList"));
        p.write_i32(events.len() as i32);
        for e in events {
            if p.data().len() >= MAX_IPC_SIZE {
                return Err(BEYOND_ONE_TRANSACTION);
            }
            p.write_i32(1);
            p.write_string16(Some(&e.package));
            p.write_string16(e.class.as_deref());
            p.write_i32(e.instance_id);
            p.write_string16(e.task_root_package.as_deref());
            p.write_string16(e.task_root_class.as_deref());
            p.write_i32(e.event_type);
            p.write_i64(e.time_stamp);
            write_extra(p, e)?;
            p.write_i32(e.flags);
        }
        return Ok(());
    }
    let mut names: Vec<&str> = Vec::new();
    for e in events {
        names.push(&e.package);
        names.extend(e.class.as_deref());
        if include_task_roots {
            names.extend(e.task_root_package.as_deref());
            names.extend(e.task_root_class.as_deref());
        }
    }
    names.sort_unstable();
    names.dedup();
    // The blob is the parcel the original marshals: count, index, the
    // string pool and the events' length, position and data.
    let mut data = Parcel::new();
    data.write_i32(events.len() as i32);
    data.write_i32(0);
    if !events.is_empty() {
        data.write_i32(names.len() as i32);
        for n in &names {
            data.write_string16(Some(n));
        }
        let index = |s: Option<&str>| s.map_or(-1, |s| names.binary_search(&s).unwrap() as i32);
        let mut list = Parcel::new();
        for e in events {
            list.write_i32(index(Some(&e.package)));
            list.write_i32(index(e.class.as_deref()));
            list.write_i32(e.instance_id);
            let roots = |s: Option<&str>| if include_task_roots { index(s) } else { -1 };
            list.write_i32(roots(e.task_root_package.as_deref()));
            list.write_i32(roots(e.task_root_class.as_deref()));
            list.write_i32(e.event_type);
            list.write_i64(e.time_stamp);
            write_extra(&mut list, e)?;
            list.write_i32(e.flags);
        }
        data.write_i32(list.data().len() as i32);
        data.write_i32(0);
        data.write_raw(list.data(), &[]);
    }
    let bytes = data.data();
    if bytes.len() > BLOB_INPLACE_LIMIT {
        return Err(BEYOND_ONE_TRANSACTION);
    }
    // `writeBlob`: the length, then `BLOB_INPLACE` and the bytes.
    p.write_i32(bytes.len() as i32);
    p.write_i32(0);
    p.write_raw(bytes, &[]);
    Ok(())
}

/// `writeEventToParcel`'s fields of the event's type.
fn write_extra(p: &mut Parcel, e: &Event) -> Result<(), &'static str> {
    match (e.event_type, &e.extra) {
        // `mConfiguration.writeToParcel` of the configuration the stats
        // file kept as its `ConfigurationProto`.
        (CONFIGURATION_CHANGE, Extra::Configuration(proto)) => {
            super::config::Configuration::from_proto(proto)
                .map_err(|_| "a configuration change's malformed ConfigurationProto")?
                .write_to(p);
        }
        (CONFIGURATION_CHANGE, _) => super::config::Configuration::from_proto(&[])
            .expect("an empty message reads")
            .write_to(p),
        (SHORTCUT_INVOCATION, Extra::ShortcutId(s))
        | (NOTIFICATION_INTERRUPTION, Extra::ChannelId(s))
        | (LOCUS_ID_SET, Extra::LocusId(s)) => p.write_string16(Some(s)),
        (STANDBY_BUCKET_CHANGED, Extra::Bucket(b)) => p.write_i32(*b),
        (STANDBY_BUCKET_CHANGED, _) => p.write_i32(0),
        // Its action, content type and annotations, which are not
        // persisted, so none for an event read from disk, the only kind the
        // app host has.
        (CHOOSER_ACTION, _) => {
            p.write_string16(None);
            p.write_string16(None);
            p.write_i32(-1);
        }
        (USER_INTERACTION, Extra::Interaction(category, action)) => {
            use crate::bundle::Value;
            p.write_i32(1);
            crate::bundle::write(
                p,
                &[
                    (
                        "android.app.usage.extra.EVENT_CATEGORY",
                        Value::String(Some(category.clone())),
                    ),
                    (
                        "android.app.usage.extra.EVENT_ACTION",
                        Value::String(Some(action.clone())),
                    ),
                ],
            );
        }
        (USER_INTERACTION, _) => p.write_i32(0),
        (SHORTCUT_INVOCATION | NOTIFICATION_INTERRUPTION | LOCUS_ID_SET, _) => {
            p.write_string16(None)
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(package: &str, event_type: i32, time_stamp: i64, instance_id: i32) -> Event {
        Event {
            package: package.into(),
            class: Some(format!("{package}.A")),
            instance_id,
            task_root_package: Some(package.into()),
            task_root_class: Some(format!("{package}.A")),
            event_type,
            time_stamp,
            flags: 0,
            extra: Extra::None,
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aim-usage-events-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A user of a new database at `now`, its clocks at that time.
    fn user(dir: &Path, now: i64) -> UserStats {
        UserStats::init(dir, (now, now), "16;REL;1", 14, &HashMap::new())
            .unwrap()
            .0
    }

    /// Foreground and visible time accrue from a resume to the pause and
    /// the stop, as `UsageStats.updateActivity` counts them.
    #[test]
    fn activity_events_accrue_time_as_the_original() {
        let dir = temp("accrue");
        let mut user = user(&dir, 1000);
        let clocks = |t| (t, t);
        user.report(event("p", ACTIVITY_RESUMED, 2000, 7), clocks(2000));
        user.report(event("p", ACTIVITY_PAUSED, 5000, 7), clocks(5000));
        user.report(event("p", ACTIVITY_STOPPED, 6000, 7), clocks(6000));
        user.report(event("p", ACTIVITY_DESTROYED, 6500, 7), clocks(6500));
        let s = &user.current[INTERVAL_DAILY].package_stats["p"];
        assert_eq!(
            (s.total_time_in_foreground, s.total_time_visible),
            (3000, 4000)
        );
        assert_eq!((s.launch_count, s.app_launch_count), (1, 1));
        assert!(s.activities.is_empty());
        // The device's startup, and its shutdown at the new interval's end
        // (a millisecond after its begin), then the events but the private
        // destroyed one.
        let types: Vec<i32> = user.current[INTERVAL_DAILY]
            .events
            .iter()
            .map(|e| e.event_type)
            .collect();
        assert_eq!(
            types,
            [
                DEVICE_STARTUP,
                DEVICE_SHUTDOWN,
                ACTIVITY_RESUMED,
                ACTIVITY_PAUSED,
                ACTIVITY_STOPPED
            ]
        );
        assert_eq!(user.current[INTERVAL_DAILY].end_time, 6500);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A resume of the package last paused is no new app launch.
    #[test]
    fn returning_to_the_last_package_is_no_app_launch() {
        let dir = temp("launch");
        let mut user = user(&dir, 0);
        user.report(event("p", ACTIVITY_RESUMED, 1, 1), (1, 1));
        user.report(event("p", ACTIVITY_PAUSED, 2, 1), (2, 2));
        user.report(event("p", ACTIVITY_RESUMED, 3, 2), (3, 3));
        assert_eq!(
            user.current[INTERVAL_DAILY].package_stats["p"].app_launch_count,
            1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// At the day's end the intervals are persisted and a new daily
    /// interval begins with the open activities continuing; the weekly
    /// one goes on, loaded again from disk without its open activities
    /// (which are not persisted); a query over both days reads the first
    /// from its file.
    #[test]
    fn the_day_rolls_over_with_open_activities() {
        let dir = temp("rollover");
        let mut user = user(&dir, 0);
        user.report(event("p", ACTIVITY_RESUMED, 1000, 1), (1000, 1000));
        let t = DAY_IN_MILLIS + 5;
        user.report(event("q", ACTIVITY_RESUMED, t, 2), (t, t));
        let daily = &user.current[INTERVAL_DAILY];
        assert_eq!(daily.begin_time, t);
        assert_eq!(
            daily.package_stats["p"].activities.get(&1),
            Some(&ACTIVITY_RESUMED)
        );
        assert_eq!(daily.events.len(), 1);
        assert_eq!(user.current[1].begin_time, 0);
        assert_eq!(
            user.current[1].package_stats["p"].total_time_in_foreground,
            // To the day's end.
            DAY_IN_MILLIS - 1 - 1000
        );
        let events = user
            .query_stats(
                INTERVAL_DAILY as i32,
                0,
                2 * DAY_IN_MILLIS,
                false,
                |s, out| {
                    out.extend(s.events.iter().map(|e| (e.package.clone(), e.event_type)));
                    true
                },
            )
            .unwrap();
        assert_eq!(
            events,
            [
                ("android".into(), DEVICE_STARTUP),
                ("android".into(), DEVICE_SHUTDOWN),
                ("p".into(), ACTIVITY_RESUMED),
                ("q".into(), ACTIVITY_RESUMED)
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The intervals persisted are loaded at the next start, the day
    /// still current, with the shutdown at the last save (the file's
    /// modification, after these tests' clocks).
    #[test]
    fn persisted_intervals_continue_at_the_next_start() {
        let dir = temp("restart");
        let mut first = user(&dir, 0);
        first.report(event("p", ACTIVITY_RESUMED, 100, 1), (100, 100));
        first.persist();
        let second = user(&dir, 500);
        let daily = &second.current[INTERVAL_DAILY];
        assert_eq!(daily.begin_time, 0);
        assert!(daily.package_stats.contains_key("p"));
        let types: Vec<i32> = daily.events.iter().map(|e| e.event_type).collect();
        assert_eq!(
            types,
            [
                DEVICE_STARTUP,
                DEVICE_SHUTDOWN,
                ACTIVITY_RESUMED,
                DEVICE_STARTUP,
                DEVICE_SHUTDOWN
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The blob form: the length and `BLOB_INPLACE`, then the marshalled
    /// count, index, sorted string pool and events.
    #[test]
    fn events_are_written_against_the_sorted_names() {
        let mut p = Parcel::new();
        let e = event("p", ACTIVITY_RESUMED, 9, 3);
        write_usage_events(&mut p, &[e], false, false).unwrap();
        let mut r = p.reader();
        let len = r.read_i32().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(len as usize, p.data().len() - 8);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 0));
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p.A"));
        let list = r.read_i32().unwrap();
        assert_eq!((list, r.read_i32().unwrap()), (36, 0));
        let ints: Vec<i32> = (0..6).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(ints, [0, 1, 3, -1, -1, ACTIVITY_RESUMED]);
        assert_eq!(r.read_i64().unwrap(), 9);
        assert_eq!(r.read_i32().unwrap(), 0);
    }
}
