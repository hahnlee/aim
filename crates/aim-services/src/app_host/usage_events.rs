//! UsageStatsService's usage events and stats of user 0 for the app host
//! (`android-16.0.0_r1`): the events the app host reports, as
//! `UserUsageStatsService.reportEvent` takes them into each interval's
//! current `IntervalStats` (the daily one's event list, every interval's
//! `UsageStats` per package), rolled over at the daily interval's end, and
//! the queries that the current intervals answer (`queryStats`'s in-memory
//! part). The intervals are not persisted and earlier ones are not read
//! (`UsageStatsDatabase`, #1842); the events the app host reports are its
//! activities' (#1843 for the rest).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

/// `UsageEvents.Event`'s types.
pub const ACTIVITY_RESUMED: i32 = 1;
pub const ACTIVITY_PAUSED: i32 = 2;
const END_OF_DAY: i32 = 3;
pub const ACTIVITY_STOPPED: i32 = 23;
pub const ACTIVITY_DESTROYED: i32 = 24;
/// `Event.FLAG_IS_PACKAGE_INSTANT_APP`.
pub const FLAG_IS_PACKAGE_INSTANT_APP: i32 = 1 << 0;
/// `UsageEvents.INSTANT_APP_PACKAGE_NAME`, `INSTANT_APP_CLASS_NAME`.
pub const INSTANT_APP_PACKAGE_NAME: &str = "android.instant_app";
const INSTANT_APP_CLASS_NAME: &str = "android.instant_class";

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

/// A `UsageEvents.Event` of the types the app host reports, which carry
/// no type-specific data.
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
}

impl Event {
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

    /// `update` for the events the app host reports and the day's end.
    fn update(&mut self, time_stamp: i64, event_type: i32, instance_id: i32) {
        match event_type {
            ACTIVITY_RESUMED | ACTIVITY_PAUSED | ACTIVITY_STOPPED | ACTIVITY_DESTROYED => {
                self.update_activity(time_stamp, event_type, instance_id)
            }
            END_OF_DAY => {
                if self.has_foreground_activity() {
                    self.increment_time_used(time_stamp);
                }
                if self.has_visible_activity() {
                    self.increment_time_visible(time_stamp);
                }
            }
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
        // No chooser counts (`mChooserCounts`).
        crate::bundle::write(p, &[]);
        p.write_i32(self.activities.len() as i32);
        for (id, event) in &self.activities {
            p.write_i32(*id);
            p.write_i32(*event);
        }
        // No foreground services (`mForegroundServices`).
        crate::bundle::write(p, &[]);
    }
}

/// An interval's `IntervalStats`.
#[derive(Clone, Debug, PartialEq)]
pub struct IntervalStats {
    pub begin_time: i64,
    pub end_time: i64,
    pub package_stats: BTreeMap<String, UsageStats>,
    /// `events`, ordered by time (`EventList`).
    pub events: Vec<Event>,
}

impl IntervalStats {
    fn new(begin_time: i64) -> Self {
        IntervalStats {
            begin_time,
            end_time: begin_time + 1,
            package_stats: BTreeMap::new(),
            events: Vec::new(),
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

    fn update(&mut self, package: &str, time_stamp: i64, event_type: i32, instance_id: i32) {
        self.usage_stats(package)
            .update(time_stamp, event_type, instance_id);
        if time_stamp > self.end_time {
            self.end_time = time_stamp;
        }
    }

    /// `EventList.insert`: after the events of the same time.
    fn add_event(&mut self, event: Event) {
        let at = self
            .events
            .partition_point(|e| e.time_stamp <= event.time_stamp);
        if event.time_stamp > self.end_time {
            self.end_time = event.time_stamp;
        }
        self.events.insert(at, event);
    }

    /// `commitTime`.
    fn commit_time(&mut self, time_stamp: i64) {
        if time_stamp > self.end_time {
            self.end_time = time_stamp;
        }
    }

    /// `firstIndexOnOrAfter`.
    pub fn first_index_on_or_after(&self, time: i64) -> usize {
        self.events.partition_point(|e| e.time_stamp < time)
    }
}

/// `UserUsageStatsService`'s current intervals.
#[derive(Debug)]
pub struct UserStats {
    pub current: [IntervalStats; INTERVAL_COUNT],
    daily_expiry: i64,
    last_backgrounded_package: Option<String>,
}

impl UserStats {
    /// `loadActiveStats` without stats on disk to load: new intervals.
    pub fn new(now: i64) -> Self {
        UserStats {
            current: std::array::from_fn(|_| IntervalStats::new(now)),
            daily_expiry: now + DAY_IN_MILLIS,
            last_backgrounded_package: None,
        }
    }

    /// `reportEvent` of an activity event.
    pub fn report(&mut self, event: Event) {
        if event.time_stamp >= self.daily_expiry {
            self.rollover(event.time_stamp);
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
        // ACTIVITY_DESTROYED is a private event.
        if event.event_type != ACTIVITY_DESTROYED {
            self.current[INTERVAL_DAILY].add_event(event);
        }
    }

    /// `rolloverStats`: each interval ends at the day's end, its open
    /// activities ended for the day; an interval whose length has not
    /// passed continues (`loadActiveStats` of the one just ended), the
    /// others start anew, and the activities continue into them.
    fn rollover(&mut self, now: i64) {
        let end = self.daily_expiry - 1;
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
            }
            stats.commit_time(end);
        }
        for (interval, stats) in self.current.iter_mut().enumerate() {
            if now >= stats.begin_time + INTERVAL_LENGTH[interval] {
                *stats = IntervalStats::new(now);
            }
        }
        self.daily_expiry = self.current[INTERVAL_DAILY].begin_time + DAY_IN_MILLIS;
        let begin = self.current[INTERVAL_DAILY].begin_time;
        for (package, activities) in continuing {
            for stats in &mut self.current {
                for (&id, &event) in &activities {
                    stats.update(&package, begin, event, id);
                }
            }
        }
    }
}

/// `UsageEvents` of `events` with the sorted names they use, written as
/// the original writes it with `use_parceled_list` on
/// (`ParcelableUsageEventList`) or off (a blob of the events against the
/// string table); `None` when it does not fit one transaction.
pub fn write_usage_events(
    p: &mut Parcel,
    events: &[Event],
    include_task_roots: bool,
    parceled_list: bool,
) -> Option<()> {
    /// `IBinder.getSuggestedMaxIpcSizeBytes`.
    const MAX_IPC_SIZE: usize = 64 * 1024;
    /// `Parcel.BLOB_INPLACE_LIMIT`.
    const BLOB_INPLACE_LIMIT: usize = 16 * 1024;
    if parceled_list {
        p.write_i32(events.len() as i32);
        p.write_i32(0);
        p.write_string16(Some("android.app.usage.ParcelableUsageEventList"));
        p.write_i32(events.len() as i32);
        for e in events {
            if p.data().len() >= MAX_IPC_SIZE {
                return None;
            }
            p.write_i32(1);
            p.write_string16(Some(&e.package));
            p.write_string16(e.class.as_deref());
            p.write_i32(e.instance_id);
            p.write_string16(e.task_root_package.as_deref());
            p.write_string16(e.task_root_class.as_deref());
            p.write_i32(e.event_type);
            p.write_i64(e.time_stamp);
            p.write_i32(e.flags);
        }
        return Some(());
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
            list.write_i32(e.flags);
        }
        data.write_i32(list.data().len() as i32);
        data.write_i32(0);
        data.write_raw(list.data(), &[]);
    }
    let bytes = data.data();
    if bytes.len() > BLOB_INPLACE_LIMIT {
        return None;
    }
    // `writeBlob`: the length, then `BLOB_INPLACE` and the bytes.
    p.write_i32(bytes.len() as i32);
    p.write_i32(0);
    p.write_raw(bytes, &[]);
    Some(())
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
        }
    }

    /// Foreground and visible time accrue from a resume to the pause and
    /// the stop, as `UsageStats.updateActivity` counts them.
    #[test]
    fn activity_events_accrue_time_as_the_original() {
        let mut user = UserStats::new(1000);
        user.report(event("p", ACTIVITY_RESUMED, 2000, 7));
        user.report(event("p", ACTIVITY_PAUSED, 5000, 7));
        user.report(event("p", ACTIVITY_STOPPED, 6000, 7));
        user.report(event("p", ACTIVITY_DESTROYED, 6500, 7));
        let s = &user.current[INTERVAL_DAILY].package_stats["p"];
        assert_eq!(
            (s.total_time_in_foreground, s.total_time_visible),
            (3000, 4000)
        );
        assert_eq!((s.launch_count, s.app_launch_count), (1, 1));
        assert!(s.activities.is_empty());
        // The destroyed event is private.
        assert_eq!(user.current[INTERVAL_DAILY].events.len(), 3);
        assert_eq!(user.current[INTERVAL_DAILY].end_time, 6500);
    }

    /// A resume of the package last paused is no new app launch.
    #[test]
    fn returning_to_the_last_package_is_no_app_launch() {
        let mut user = UserStats::new(0);
        user.report(event("p", ACTIVITY_RESUMED, 1, 1));
        user.report(event("p", ACTIVITY_PAUSED, 2, 1));
        user.report(event("p", ACTIVITY_RESUMED, 3, 2));
        assert_eq!(
            user.current[INTERVAL_DAILY].package_stats["p"].app_launch_count,
            1
        );
    }

    /// At the day's end a new daily interval begins with the open
    /// activities continuing; the weekly one goes on.
    #[test]
    fn the_day_rolls_over_with_open_activities() {
        let mut user = UserStats::new(0);
        user.report(event("p", ACTIVITY_RESUMED, 1000, 1));
        user.report(event("q", ACTIVITY_RESUMED, DAY_IN_MILLIS + 5, 2));
        let daily = &user.current[INTERVAL_DAILY];
        assert_eq!(daily.begin_time, DAY_IN_MILLIS + 5);
        assert_eq!(
            daily.package_stats["p"].activities.get(&1),
            Some(&ACTIVITY_RESUMED)
        );
        assert_eq!(daily.events.len(), 1);
        assert_eq!(user.current[1].begin_time, 0);
        assert_eq!(
            user.current[1].package_stats["p"].total_time_in_foreground,
            // To the day's end, then from it to the activity's
            // continuation in the new day.
            DAY_IN_MILLIS - 1 - 1000 + 6
        );
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
