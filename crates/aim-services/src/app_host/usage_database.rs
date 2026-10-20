//! UsageStatsDatabase of user 0 for the app host (`android-16.0.0_r1`):
//! the intervals' stats files under `/data/system_ce/0/usagestats`
//! (`daily`, `weekly`, `monthly`, `yearly`, each file named by its
//! interval's begin time), written and read in the database's version 5
//! form (`UsageStatsProtoV2`) against the package token mappings
//! (`mappings`, `PackagesTokenData`), with the version file, the files'
//! index and pruning. Databases of an earlier version, which the original
//! upgrades through a backup and breadcrumb (XML and the first protobuf
//! form), are not read (#1866).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use aim_storage::guest_inode::{self, GuestInode};

use super::exit_info::{Value, fields};
use super::usage_events::{
    CONFIGURATION_CHANGE, ConfigurationStats, Event, Extra, INTERVAL_COUNT, IntervalStats,
    LOCUS_ID_SET, NOTIFICATION_INTERRUPTION, SHORTCUT_INVOCATION, STANDBY_BUCKET_CHANGED,
    USER_INTERACTION, UsageStats,
};

/// `UsageStatsDatabase.DEFAULT_CURRENT_VERSION`.
pub const CURRENT_VERSION: i32 = 5;
/// `MAX_FILES_PER_INTERVAL_TYPE`.
const MAX_FILES_PER_INTERVAL_TYPE: [usize; INTERVAL_COUNT] = [100, 50, 12, 10];
/// The interval directories, in the intervals' order.
pub const INTERVAL_DIRS: [&str; INTERVAL_COUNT] = ["daily", "weekly", "monthly", "yearly"];
/// `AtomicFile`'s legacy backup and new file.
const BAK_SUFFIX: &str = ".bak";
const NEW_SUFFIX: &str = ".new";
/// `UsageStatsXml.CHECKED_IN_SUFFIX`: a daily file checked in.
const CHECKED_IN_SUFFIX: &str = "-c";
/// `PackagesTokenData.UNASSIGNED_TOKEN`, `PACKAGE_NAME_INDEX`.
pub const UNASSIGNED_TOKEN: i32 = -1;
const PACKAGE_NAME_INDEX: i32 = 0;
/// `UnixCalendar`'s units.
const DAY_IN_MILLIS: i64 = 24 * 60 * 60 * 1000;
const WEEK_IN_MILLIS: i64 = 7 * DAY_IN_MILLIS;
const MONTH_IN_MILLIS: i64 = 30 * DAY_IN_MILLIS;
const YEAR_IN_MILLIS: i64 = 365 * DAY_IN_MILLIS;
const ONE_HOUR_MS: i64 = 60 * 60 * 1000;
/// `SELECTION_LOG_RETENTION_LEN`'s default, in days
/// (`ro.usagestats.chooser.retention`).
pub const DEFAULT_SELECTION_LOG_RETENTION_LEN: i64 = 14;

/// The system's owner of what the database writes, as system_server
/// creates it (`File.mkdirs` under its umask, `AtomicFile`).
const SYSTEM_DIR: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o700),
};
const SYSTEM_FILE: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o600),
};

/// `PackagesTokenData`: each package's strings (its name first, then the
/// classes, shortcut ids, channel ids and the rest tokenized for it) by
/// the package's token.
#[derive(Debug, Default)]
pub struct Tokens {
    counter: i32,
    tokens_to_packages: BTreeMap<i32, Vec<String>>,
    packages_to_tokens: HashMap<String, HashMap<String, i32>>,
    removed_packages: HashMap<String, i64>,
}

impl Tokens {
    pub fn new() -> Self {
        Tokens {
            counter: 1,
            ..Default::default()
        }
    }

    /// `getPackageTokenOrAdd`: unassigned for a package removed after
    /// `time_stamp`.
    pub fn package_token_or_add(&mut self, package: &str, time_stamp: i64) -> i32 {
        if self
            .removed_packages
            .get(package)
            .is_some_and(|&removed| removed > time_stamp)
        {
            return UNASSIGNED_TOKEN;
        }
        let tokens = self.packages_to_tokens.entry(package.into()).or_default();
        if let Some(&token) = tokens.get(package) {
            return token;
        }
        let token = self.counter;
        self.counter += 1;
        tokens.insert(package.into(), token);
        self.tokens_to_packages.insert(token, vec![package.into()]);
        token
    }

    /// `getTokenOrAdd` of `key` in the strings of `package`.
    pub fn token_or_add(&mut self, package_token: i32, package: &str, key: &str) -> i32 {
        if package == key {
            return PACKAGE_NAME_INDEX;
        }
        let tokens = self.packages_to_tokens.entry(package.into()).or_default();
        if let Some(&token) = tokens.get(key) {
            return token;
        }
        let strings = self.tokens_to_packages.entry(package_token).or_default();
        let token = strings.len() as i32;
        tokens.insert(key.into(), token);
        strings.push(key.into());
        token
    }

    /// `getPackageString`.
    pub fn package_string(&self, package_token: i32) -> Option<&str> {
        self.tokens_to_packages
            .get(&package_token)
            .and_then(|s| s.first())
            .map(String::as_str)
    }

    /// `getString`.
    pub fn string(&self, package_token: i32, token: i32) -> Option<&str> {
        let strings = self.tokens_to_packages.get(&package_token)?;
        usize::try_from(token)
            .ok()
            .and_then(|t| strings.get(t))
            .map(String::as_str)
    }

    /// `removePackage`: the package's token, unassigned if it had none.
    #[cfg(test)]
    pub fn remove_package(&mut self, package: &str, time_removed: i64) -> i32 {
        self.removed_packages.insert(package.into(), time_removed);
        let Some(tokens) = self.packages_to_tokens.remove(package) else {
            return UNASSIGNED_TOKEN;
        };
        let token = tokens[package];
        self.tokens_to_packages.remove(&token);
        token
    }

    /// `readObfuscatedData`, then the packages' token maps rebuilt
    /// (`readMappingsLocked`).
    fn read(bytes: &[u8]) -> Result<Tokens, String> {
        let mut t = Tokens::new();
        fields(bytes, |number, value| {
            match number {
                1 => t.counter = value.int()?,
                2 => {
                    let mut key = UNASSIGNED_TOKEN;
                    let mut strings = Vec::new();
                    fields(value.bytes()?, |n, v| {
                        match n {
                            1 => key = v.int()? - 1,
                            2 => strings.push(v.string()?),
                            _ => {}
                        }
                        Ok(())
                    })?;
                    if key != UNASSIGNED_TOKEN {
                        t.tokens_to_packages.insert(key, strings);
                    }
                }
                _ => {}
            }
            Ok(())
        })?;
        for (&token, strings) in &t.tokens_to_packages {
            let Some(package) = strings.first() else {
                continue;
            };
            let mut map = HashMap::new();
            map.insert(package.clone(), token);
            for (j, s) in strings.iter().enumerate().skip(1) {
                map.insert(s.clone(), j as i32);
            }
            t.packages_to_tokens.insert(package.clone(), map);
        }
        Ok(t)
    }

    /// `writeObfuscatedData`.
    fn write(&self) -> Vec<u8> {
        let mut out = Out::default();
        out.varint(1, self.counter.into());
        for (&token, strings) in &self.tokens_to_packages {
            out.message(2, |m| {
                m.varint(1, (token + 1).into());
                for s in strings {
                    m.repeated_string(2, s);
                }
            });
        }
        out.0
    }
}

/// `ProtoOutputStream`'s encoding: a singular scalar only when it is not
/// zero, false or empty; a repeated string always; a message only when it
/// is not empty (`endObjectImpl` drops an empty one).
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn raw_varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    /// An `int32` (sign-extended) or `int64` field.
    fn varint(&mut self, number: u64, value: i64) {
        if value != 0 {
            self.raw_varint(number << 3);
            self.raw_varint(value as u64);
        }
    }

    fn boolean(&mut self, number: u64, value: bool) {
        self.varint(number, value.into());
    }

    fn repeated_string(&mut self, number: u64, value: &str) {
        self.bytes(number, value.as_bytes());
    }

    fn bytes(&mut self, number: u64, value: &[u8]) {
        self.raw_varint(number << 3 | 2);
        self.raw_varint(value.len() as u64);
        self.0.extend_from_slice(value);
    }

    fn message(&mut self, number: u64, f: impl FnOnce(&mut Out)) {
        let mut m = Out::default();
        f(&mut m);
        self.object(number, &m.0);
    }

    /// A message already encoded, such as a kept `ConfigurationProto`.
    fn object(&mut self, number: u64, value: &[u8]) {
        if !value.is_empty() {
            self.bytes(number, value);
        }
    }

    /// `writeOffsetTimestamp`: the time after the begin time, kept from an
    /// hour before it (rollover's grace).
    fn offset_timestamp(&mut self, number: u64, time: i64, begin: i64) {
        if time > begin - ONE_HOUR_MS {
            self.varint(number, offset_timestamp(time, begin));
        }
    }
}

/// `getOffsetTimestamp`: one more when it is 0, so it is written.
fn offset_timestamp(time: i64, offset: i64) -> i64 {
    match time - offset {
        0 => 1,
        t => t,
    }
}

fn long(value: &Value<'_>) -> Result<i64, String> {
    match value {
        Value::Varint(v) => Ok(*v as i64),
        _ => Err("wire type mismatch".into()),
    }
}

/// An event as `parseEvent` reads it: its tokens, and the token kept for
/// its type's string.
struct ObfuscatedEvent {
    package: i32,
    class: i32,
    time_stamp: i64,
    flags: i32,
    event_type: i32,
    config: Option<Vec<u8>>,
    shortcut: i32,
    bucket: i32,
    channel: i32,
    instance_id: i32,
    task_root_package: i32,
    task_root_class: i32,
    locus: i32,
    interaction: Option<(i32, i32)>,
}

fn parse_event(bytes: &[u8], begin: i64) -> Result<Option<ObfuscatedEvent>, String> {
    let mut e = ObfuscatedEvent {
        package: UNASSIGNED_TOKEN,
        class: UNASSIGNED_TOKEN,
        time_stamp: 0,
        flags: 0,
        event_type: 0,
        config: None,
        shortcut: UNASSIGNED_TOKEN,
        bucket: 0,
        channel: UNASSIGNED_TOKEN,
        instance_id: 0,
        task_root_package: UNASSIGNED_TOKEN,
        task_root_class: UNASSIGNED_TOKEN,
        locus: UNASSIGNED_TOKEN,
        interaction: None,
    };
    fields(bytes, |number, v| {
        match number {
            1 => e.package = v.int()? - 1,
            2 => e.class = v.int()? - 1,
            3 => e.time_stamp = begin + long(&v)?,
            4 => e.flags = v.int()?,
            5 => e.event_type = v.int()?,
            6 => e.config = Some(v.bytes()?.to_vec()),
            7 => e.shortcut = v.int()? - 1,
            8 => e.bucket = v.int()?,
            9 => e.channel = v.int()? - 1,
            10 => e.instance_id = v.int()?,
            11 => e.task_root_package = v.int()? - 1,
            12 => e.task_root_class = v.int()? - 1,
            13 => e.locus = v.int()? - 1,
            14 => {
                // `parseUserInteractionEventExtras`.
                let (mut category, mut action) = (UNASSIGNED_TOKEN, UNASSIGNED_TOKEN);
                fields(v.bytes()?, |n, v| {
                    match n {
                        1 => category = v.int()? - 1,
                        2 => action = v.int()? - 1,
                        _ => {}
                    }
                    Ok(())
                })?;
                e.interaction = Some((category, action));
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok((e.package != UNASSIGNED_TOKEN).then_some(e))
}

/// `UsageStatsProtoV2.read` of an interval beginning at `begin`, then
/// `deobfuscateData` against `tokens`; the events left out with
/// `skip_events`.
pub fn read_interval(
    bytes: &[u8],
    begin: i64,
    tokens: &Tokens,
    skip_events: bool,
) -> Result<IntervalStats, String> {
    let mut stats = IntervalStats::empty(begin);
    let mut packages: BTreeMap<i32, ParsedUsageStats> = BTreeMap::new();
    let mut events = Vec::new();
    fields(bytes, |number, v| {
        match number {
            1 => stats.end_time = begin + long(&v)?,
            2 => stats.major_version = v.int()?,
            3 => stats.minor_version = v.int()?,
            10..=13 => {
                let tracker = &mut stats.trackers[(number - 10) as usize];
                fields(v.bytes()?, |n, v| {
                    match n {
                        1 => tracker.count = v.int()?,
                        2 => tracker.duration = long(&v)?,
                        _ => {}
                    }
                    Ok(())
                })?;
            }
            20 => {
                let (token, s) = parse_usage_stats(v.bytes()?, begin)?;
                if token != UNASSIGNED_TOKEN {
                    packages.insert(token, s);
                }
            }
            21 => {
                // `loadConfigStats`: a configuration read again is the
                // same entry (`getOrCreateConfigurationStats`).
                let mut c = ConfigurationStats::default();
                let mut active = false;
                fields(v.bytes()?, |n, v| {
                    match n {
                        1 => c.config = v.bytes()?.to_vec(),
                        2 => c.last_time_active = begin + long(&v)?,
                        3 => c.total_time_active = long(&v)?,
                        4 => c.activation_count = v.int()?,
                        5 => active = long(&v)? != 0,
                        _ => {}
                    }
                    Ok(())
                })?;
                let at = match stats
                    .configurations
                    .iter()
                    .position(|x| x.config == c.config)
                {
                    Some(at) => {
                        stats.configurations[at] = c;
                        at
                    }
                    None => {
                        stats.configurations.push(c);
                        stats.configurations.len() - 1
                    }
                };
                if active {
                    stats.active_configuration = Some(at);
                }
            }
            22 if !skip_events => {
                if let Some(e) = parse_event(v.bytes()?, begin)? {
                    events.push(e);
                }
            }
            _ => {}
        }
        Ok(())
    })?;
    // `deobfuscateUsageStats`: stats of a token without a package are left
    // out.
    for (token, (mut s, chooser)) in packages {
        let Some(package) = tokens.package_string(token) else {
            continue;
        };
        s.package = package.into();
        s.begin_time_stamp = stats.begin_time;
        s.end_time_stamp = stats.end_time;
        for (action, counts) in chooser {
            let Some(action) = tokens.string(token, action) else {
                continue;
            };
            let entry = s.chooser_counts.entry(action.into()).or_default();
            for (category, count) in counts {
                if let Some(category) = tokens.string(token, category) {
                    entry.insert(category.into(), count);
                }
            }
        }
        stats.package_stats.insert(s.package.clone(), s);
    }
    for e in events {
        if let Some(event) = deobfuscate_event(e, tokens) {
            stats.add_event(event);
        }
    }
    Ok(stats)
}

/// A package's stats and its chooser counts by token (`parseUsageStats`).
type ParsedUsageStats = (UsageStats, Vec<(i32, Vec<(i32, i32)>)>);

fn parse_usage_stats(bytes: &[u8], begin: i64) -> Result<(i32, ParsedUsageStats), String> {
    let mut token = UNASSIGNED_TOKEN;
    let mut s = UsageStats::default();
    let mut chooser: Vec<(i32, Vec<(i32, i32)>)> = Vec::new();
    fields(bytes, |number, v| {
        match number {
            1 => token = v.int()? - 1,
            3 => s.last_time_used = begin + long(&v)?,
            4 => s.total_time_in_foreground = long(&v)?,
            6 => s.app_launch_count = v.int()?,
            7 => {
                // `loadChooserCounts`: counts without an action are left
                // out.
                let mut action = None;
                let mut counts = Vec::new();
                fields(v.bytes()?, |n, v| {
                    match n {
                        1 => action = Some(v.int()? - 1),
                        2 => {
                            let (mut category, mut count) = (UNASSIGNED_TOKEN, 0);
                            fields(v.bytes()?, |n, v| {
                                match n {
                                    1 => category = v.int()? - 1,
                                    2 => count = v.int()?,
                                    _ => {}
                                }
                                Ok(())
                            })?;
                            if category != UNASSIGNED_TOKEN {
                                counts.push((category, count));
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                })?;
                if let Some(action) = action {
                    match chooser.iter_mut().find(|(a, _)| *a == action) {
                        Some((_, c)) => c.extend(counts),
                        None => chooser.push((action, counts)),
                    }
                }
            }
            8 => s.last_time_foreground_service_used = begin + long(&v)?,
            9 => s.total_time_foreground_service_used = long(&v)?,
            10 => s.last_time_visible = begin + long(&v)?,
            11 => s.total_time_visible = long(&v)?,
            12 => s.last_time_component_used = begin + long(&v)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok((token, (s, chooser)))
}

/// `deobfuscateEvents` of one event: none when a string it needs has no
/// token.
fn deobfuscate_event(e: ObfuscatedEvent, tokens: &Tokens) -> Option<Event> {
    let package = tokens.package_string(e.package)?.to_string();
    let string = |t: i32| {
        (t != UNASSIGNED_TOKEN)
            .then(|| tokens.string(e.package, t).map(String::from))
            .flatten()
    };
    let extra = match e.event_type {
        CONFIGURATION_CHANGE => Extra::Configuration(e.config.unwrap_or_default()),
        SHORTCUT_INVOCATION => Extra::ShortcutId(tokens.string(e.package, e.shortcut)?.into()),
        NOTIFICATION_INTERRUPTION => Extra::ChannelId(tokens.string(e.package, e.channel)?.into()),
        LOCUS_ID_SET => Extra::LocusId(tokens.string(e.package, e.locus)?.into()),
        STANDBY_BUCKET_CHANGED => Extra::Bucket(e.bucket),
        USER_INTERACTION => match e.interaction {
            Some((category, action)) => {
                let category = tokens.string(e.package, category).unwrap_or_default();
                let action = tokens.string(e.package, action).unwrap_or_default();
                if category.is_empty() || action.is_empty() {
                    return None;
                }
                Extra::Interaction(category.into(), action.into())
            }
            None => Extra::None,
        },
        _ => Extra::None,
    };
    Some(Event {
        class: string(e.class),
        task_root_package: string(e.task_root_package),
        task_root_class: string(e.task_root_class),
        package,
        instance_id: e.instance_id,
        event_type: e.event_type,
        time_stamp: e.time_stamp,
        flags: e.flags,
        extra,
    })
}

/// `obfuscateData` of `stats` against `tokens` (the events of packages
/// removed since dropped), then `UsageStatsProtoV2.write`.
pub fn write_interval(stats: &mut IntervalStats, tokens: &mut Tokens) -> Vec<u8> {
    let begin = stats.begin_time;
    let mut packages: BTreeMap<i32, &UsageStats> = BTreeMap::new();
    let mut chooser: HashMap<String, BTreeMap<i32, BTreeMap<i32, i32>>> = HashMap::new();
    for (name, s) in &stats.package_stats {
        let token = tokens.package_token_or_add(name, s.end_time_stamp);
        if token == UNASSIGNED_TOKEN {
            continue;
        }
        let mut by_action = BTreeMap::new();
        for (action, categories) in &s.chooser_counts {
            let mut counts = BTreeMap::new();
            for (category, &count) in categories {
                counts.insert(tokens.token_or_add(token, name, category), count);
            }
            by_action.insert(tokens.token_or_add(token, name, action), counts);
        }
        chooser.insert(name.clone(), by_action);
        packages.insert(token, s);
    }
    stats
        .events
        .retain(|e| tokens.package_token_or_add(&e.package, e.time_stamp) != UNASSIGNED_TOKEN);
    let mut out = Out::default();
    out.varint(1, offset_timestamp(stats.end_time, begin));
    out.varint(2, stats.major_version.into());
    out.varint(3, stats.minor_version.into());
    for (i, tracker) in stats.trackers.iter().enumerate() {
        out.message(10 + i as u64, |m| {
            m.varint(1, tracker.count.into());
            m.varint(2, tracker.duration);
        });
    }
    for (&token, s) in &packages {
        out.message(20, |m| {
            m.varint(1, (token + 1).into());
            m.offset_timestamp(3, s.last_time_used, begin);
            m.varint(4, s.total_time_in_foreground);
            m.offset_timestamp(8, s.last_time_foreground_service_used, begin);
            m.varint(9, s.total_time_foreground_service_used);
            m.offset_timestamp(10, s.last_time_visible, begin);
            m.varint(11, s.total_time_visible);
            m.offset_timestamp(12, s.last_time_component_used, begin);
            m.varint(6, s.app_launch_count.into());
            // `writeChooserCounts`: actions with counts above zero.
            for (action, counts) in &chooser[&s.package] {
                if counts.is_empty() {
                    continue;
                }
                m.message(7, |a| {
                    a.varint(1, (action + 1).into());
                    for (category, &count) in counts {
                        if count > 0 {
                            a.message(2, |c| {
                                c.varint(1, (category + 1).into());
                                c.varint(2, count.into());
                            });
                        }
                    }
                });
            }
        });
    }
    for (i, c) in stats.configurations.iter().enumerate() {
        out.message(21, |m| {
            m.object(1, &c.config);
            m.offset_timestamp(2, c.last_time_active, begin);
            m.varint(3, c.total_time_active);
            m.varint(4, c.activation_count.into());
            m.boolean(5, stats.active_configuration == Some(i));
        });
    }
    for e in &stats.events {
        let token = tokens.package_token_or_add(&e.package, e.time_stamp);
        let mut add = |s: &Option<String>| match s.as_deref() {
            Some(s) if !s.is_empty() => tokens.token_or_add(token, &e.package, s),
            _ => UNASSIGNED_TOKEN,
        };
        let class = add(&e.class);
        let task_root_package = add(&e.task_root_package);
        let task_root_class = add(&e.task_root_class);
        let mut string = |s: &str| {
            if s.is_empty() {
                UNASSIGNED_TOKEN
            } else {
                tokens.token_or_add(token, &e.package, s)
            }
        };
        let extra = match &e.extra {
            Extra::ShortcutId(s) | Extra::ChannelId(s) | Extra::LocusId(s) => {
                (string(s), UNASSIGNED_TOKEN)
            }
            Extra::Interaction(category, action) => (string(category), string(action)),
            _ => (UNASSIGNED_TOKEN, UNASSIGNED_TOKEN),
        };
        out.message(22, |m| {
            m.varint(1, (token + 1).into());
            if class != UNASSIGNED_TOKEN {
                m.varint(2, (class + 1).into());
            }
            m.offset_timestamp(3, e.time_stamp, begin);
            m.varint(4, e.flags.into());
            m.varint(5, e.event_type.into());
            m.varint(10, e.instance_id.into());
            if task_root_package != UNASSIGNED_TOKEN {
                m.varint(11, (task_root_package + 1).into());
            }
            if task_root_class != UNASSIGNED_TOKEN {
                m.varint(12, (task_root_class + 1).into());
            }
            match (&e.extra, e.event_type) {
                (Extra::Configuration(config), CONFIGURATION_CHANGE) => m.object(6, config),
                (Extra::Bucket(bucket), STANDBY_BUCKET_CHANGED) => m.varint(8, (*bucket).into()),
                (Extra::ShortcutId(_), SHORTCUT_INVOCATION) if extra.0 != UNASSIGNED_TOKEN => {
                    m.varint(7, (extra.0 + 1).into())
                }
                (Extra::LocusId(_), LOCUS_ID_SET) if extra.0 != UNASSIGNED_TOKEN => {
                    m.varint(13, (extra.0 + 1).into())
                }
                (Extra::ChannelId(_), NOTIFICATION_INTERRUPTION) if extra.0 != UNASSIGNED_TOKEN => {
                    m.varint(9, (extra.0 + 1).into())
                }
                (Extra::Interaction(..), USER_INTERACTION)
                    if extra.0 != UNASSIGNED_TOKEN && extra.1 != UNASSIGNED_TOKEN =>
                {
                    m.message(14, |x| {
                        x.varint(1, (extra.0 + 1).into());
                        x.varint(2, (extra.1 + 1).into());
                    })
                }
                _ => {}
            }
        });
    }
    out.0
}

/// `parseBeginTime`: the leading digits of a file's name.
fn parse_begin_time(name: &str) -> Option<i64> {
    let digits = name
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(name.len());
    name[..digits].parse().ok()
}

/// `AtomicFile.openRead`: its legacy backup, if any, in its place first.
pub(super) fn atomic_read(path: &Path) -> std::io::Result<Vec<u8>> {
    let bak = suffixed(path, BAK_SUFFIX);
    if bak.exists() {
        std::fs::rename(&bak, path)?;
    }
    let new = suffixed(path, NEW_SUFFIX);
    if new.exists() && path.exists() {
        std::fs::remove_file(&new)?;
    }
    std::fs::read(path)
}

/// `AtomicFile.startWrite`/`finishWrite`: the bytes to a new file, synced,
/// then renamed over the file, owned by the system.
pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let bak = suffixed(path, BAK_SUFFIX);
    if bak.exists() {
        std::fs::rename(&bak, path)?;
    }
    let new = suffixed(path, NEW_SUFFIX);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    file.write_all(bytes)?;
    crate::fsync(&file)?;
    guest_inode::record(&new, SYSTEM_FILE)?;
    std::fs::rename(&new, path)
}

/// `AtomicFile.delete`.
pub(super) fn atomic_delete(path: &Path) {
    for p in [
        path.to_path_buf(),
        suffixed(path, NEW_SUFFIX),
        suffixed(path, BAK_SUFFIX),
    ] {
        let _ = std::fs::remove_file(p);
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// `File.mkdirs` as the system: the directory and missing parents.
pub fn mkdirs(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        mkdirs(parent)?;
    }
    match std::fs::create_dir(dir) {
        Ok(()) => guest_inode::record(dir, SYSTEM_DIR),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// A file of text lines written as `BufferedWriter` writes them, owned by
/// the system (the version and the migration status files).
pub fn write_lines(path: &Path, lines: &[&str]) -> std::io::Result<()> {
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    std::fs::write(path, text)?;
    guest_inode::record(path, SYSTEM_FILE)
}

/// What `init` found of the version file.
#[derive(Debug, Default, PartialEq)]
pub struct Init {
    /// `isNewUpdate`: no build fingerprint, or another one.
    pub new_update: bool,
    /// `wasUpgradePerformed`: a backups directory was left, and deleted.
    pub upgrade_performed: bool,
}

/// `UsageStatsDatabase`.
pub struct Database {
    dir: PathBuf,
    /// `mSortedStatFiles`: each interval's files by begin time.
    files: [BTreeMap<i64, PathBuf>; INTERVAL_COUNT],
    pub tokens: Tokens,
}

impl Database {
    pub fn new(dir: &Path) -> Database {
        Database {
            dir: dir.to_path_buf(),
            files: Default::default(),
            tokens: Tokens::new(),
        }
    }

    fn interval_dir(&self, interval: usize) -> PathBuf {
        self.dir.join(INTERVAL_DIRS[interval])
    }

    /// `readMappingsLocked`: none read when the file is missing or cannot
    /// be read, as the original goes on with the mappings it has.
    pub fn read_mappings(&mut self) {
        let path = self.dir.join("mappings");
        if !path.exists() {
            return;
        }
        match atomic_read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| Tokens::read(&b))
        {
            Ok(tokens) => self.tokens = tokens,
            Err(e) => eprintln!(
                "app host: usagestats: Failed to read the obfuscated packages mapping file: {e}"
            ),
        }
    }

    /// `writeMappingsLocked`.
    pub fn write_mappings(&self) -> std::io::Result<()> {
        atomic_write(&self.dir.join("mappings"), &self.tokens.write())
    }

    /// `init`: the directories made, the version checked (a database of
    /// another version is not upgraded, #1866), the files indexed (pruned
    /// after an upgrade) and those of the future deleted.
    pub fn init(
        &mut self,
        now: i64,
        fingerprint: &str,
        retention_days: i64,
    ) -> Result<Init, String> {
        for i in 0..INTERVAL_COUNT {
            let dir = self.interval_dir(i);
            mkdirs(&dir)
                .map_err(|e| format!("Failed to create directory {}: {e}", dir.display()))?;
        }
        let init = self.check_version_and_build(fingerprint)?;
        if init.upgrade_performed {
            self.prune(now, retention_days);
        } else {
            self.index_files();
        }
        for files in &mut self.files {
            let future: Vec<i64> = files.range(now..).map(|(&t, _)| t).collect();
            for t in future {
                atomic_delete(&files.remove(&t).unwrap());
            }
        }
        Ok(init)
    }

    /// `checkVersionAndBuildLocked`.
    fn check_version_and_build(&mut self, fingerprint: &str) -> Result<Init, String> {
        let version_file = self.dir.join("version");
        let text = std::fs::read_to_string(&version_file).ok();
        let mut lines = text.as_deref().map(str::lines).into_iter().flatten();
        let version = lines.next().and_then(|l| l.parse::<i32>().ok());
        let build = if version.is_some() {
            lines.next()
        } else {
            None
        };
        let version = version.unwrap_or(0);
        let mut init = Init {
            new_update: build != Some(fingerprint),
            upgrade_performed: false,
        };
        if self.dir.join("breadcrumb").exists() {
            return Err("an interrupted version upgrade (#1866)".into());
        }
        if version != CURRENT_VERSION {
            // `doUpgradeLocked`: a database before version 2 is deleted,
            // a later one upgraded from a backup.
            if version >= 2 {
                return Err(format!(
                    "upgrading the database from version {version} (#1866)"
                ));
            }
            for i in 0..INTERVAL_COUNT {
                for entry in std::fs::read_dir(self.interval_dir(i))
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        if version != CURRENT_VERSION || init.new_update {
            write_lines(&version_file, &[&CURRENT_VERSION.to_string(), fingerprint])
                .map_err(|e| format!("Failed to write new version: {e}"))?;
        }
        // `KEEP_BACKUP_DIR` is off: what an upgrade left is cleaned up.
        let backups = self.dir.join("backups");
        if backups.exists() {
            init.upgrade_performed = true;
            let _ = std::fs::remove_dir_all(&backups);
        }
        Ok(init)
    }

    /// `indexFilesLocked`, keeping at most `MAX_FILES_PER_INTERVAL_TYPE`
    /// of each interval.
    fn index_files(&mut self) {
        for i in 0..INTERVAL_COUNT {
            let mut files = BTreeMap::new();
            for entry in std::fs::read_dir(self.interval_dir(i))
                .into_iter()
                .flatten()
                .flatten()
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(BAK_SUFFIX) {
                    continue;
                }
                if let Some(t) = parse_begin_time(&name) {
                    files.insert(t, entry.path());
                }
            }
            while files.len() > MAX_FILES_PER_INTERVAL_TYPE[i] {
                let (_, oldest) = files.pop_first().unwrap();
                atomic_delete(&oldest);
            }
            self.files[i] = files;
        }
    }

    /// `prunePackagesDataOnUpgrade`: the stats and events of packages not
    /// installed at their time rewritten out of every file.
    pub fn prune_packages_data_on_upgrade(&mut self, installed: &HashMap<String, i64>) {
        if installed.is_empty() {
            return;
        }
        let is_installed =
            |package: &str, time: i64| installed.get(package).is_some_and(|&t| t <= time);
        for i in 0..INTERVAL_COUNT {
            let files: Vec<(i64, PathBuf)> =
                self.files[i].iter().map(|(&t, p)| (t, p.clone())).collect();
            for (begin, path) in files {
                let pruned = self.read(&path, begin, false).and_then(|mut stats| {
                    let before = (stats.package_stats.len(), stats.events.len());
                    stats
                        .package_stats
                        .retain(|name, s| is_installed(name, s.end_time_stamp));
                    stats
                        .events
                        .retain(|e| is_installed(&e.package, e.time_stamp));
                    if before == (stats.package_stats.len(), stats.events.len()) {
                        return Ok(());
                    }
                    let bytes = write_interval(&mut stats, &mut self.tokens);
                    atomic_write(&path, &bytes).map_err(|e| e.to_string())
                });
                if let Err(e) = pruned {
                    eprintln!("app host: usagestats: Failed to prune data from: {e}");
                }
            }
        }
    }

    /// `onTimeChanged`: each file renamed by `diff`, deleted if it would
    /// begin before the epoch, then the files indexed again.
    pub fn on_time_changed(&mut self, diff: i64) {
        for files in &mut self.files {
            for (&begin, path) in files.iter() {
                let new_time = begin + diff;
                if new_time < 0 {
                    atomic_delete(path);
                    continue;
                }
                let _ = atomic_read(path);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut new_name = new_time.to_string();
                if name.ends_with(CHECKED_IN_SUFFIX) {
                    new_name.push_str(CHECKED_IN_SUFFIX);
                }
                let _ = std::fs::rename(path, path.with_file_name(new_name));
            }
            files.clear();
        }
        self.index_files();
    }

    /// The begin times of `interval`'s files.
    #[cfg(test)]
    pub fn begin_times(&self, interval: usize) -> impl Iterator<Item = i64> + '_ {
        self.files[interval].keys().copied()
    }

    /// `readLocked`: the file's stats, its last modification the time it
    /// was saved.
    fn read(&self, path: &Path, begin: i64, skip_events: bool) -> Result<IntervalStats, String> {
        let bytes = atomic_read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut stats = read_interval(&bytes, begin, &self.tokens, skip_events)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        stats.last_time_saved = modified_millis(path);
        Ok(stats)
    }

    /// `getLatestUsageStats`: none without a file or when it cannot be
    /// read.
    pub fn latest(&self, interval: usize) -> Option<IntervalStats> {
        let (&begin, path) = self.files[interval].last_key_value()?;
        match self.read(path, begin, false) {
            Ok(stats) => Some(stats),
            Err(e) => {
                eprintln!("app host: usagestats: Failed to read usage stats file: {e}");
                None
            }
        }
    }

    /// `queryUsageStats`: `combine` over the stats of the files that cover
    /// `[begin, end)`, until it returns false; a file that cannot be read
    /// is passed over.
    pub fn query(
        &self,
        interval: usize,
        begin: i64,
        end: i64,
        skip_events: bool,
        mut combine: impl FnMut(&IntervalStats) -> bool,
    ) {
        if end <= begin {
            return;
        }
        let files = &self.files[interval];
        // The files from the last on or before `begin` to the last before
        // `end` (exclusive).
        let start = files.range(..=begin).next_back().map(|(&t, _)| t);
        let range: Vec<(i64, &PathBuf)> = match start {
            Some(start) => files.range(start..end).map(|(&t, p)| (t, p)).collect(),
            None => files.range(..end).map(|(&t, p)| (t, p)).collect(),
        };
        for (t, path) in range {
            match self.read(path, t, skip_events) {
                Ok(stats) => {
                    if begin < stats.end_time && !combine(&stats) {
                        break;
                    }
                }
                Err(e) => eprintln!("app host: usagestats: Failed to read usage stats file: {e}"),
            }
        }
    }

    /// `findBestFitBucket`.
    pub fn best_fit_bucket(&self, begin: i64) -> i32 {
        let mut best = -1;
        let mut smallest = i64::MAX;
        for i in (0..INTERVAL_COUNT).rev() {
            if let Some((&key, _)) = self.files[i].range(..=begin).next_back() {
                let diff = (key - begin).abs();
                if diff < smallest {
                    smallest = diff;
                    best = i as i32;
                }
            }
        }
        best
    }

    /// `putUsageStats`: the stats written to their interval's file of their
    /// begin time; the time they were saved noted.
    #[cfg(test)]
    pub fn put(&mut self, interval: usize, stats: &mut IntervalStats) -> std::io::Result<()> {
        let bytes = write_interval(stats, &mut self.tokens);
        self.put_bytes(interval, stats, &bytes)
    }

    fn put_bytes(
        &mut self,
        interval: usize,
        stats: &mut IntervalStats,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        let dir = self.interval_dir(interval);
        let path = self.files[interval]
            .entry(stats.begin_time)
            .or_insert_with(|| dir.join(stats.begin_time.to_string()))
            .clone();
        atomic_write(&path, bytes)?;
        stats.last_time_saved = modified_millis(&path);
        Ok(())
    }

    /// `persistActiveStats`' writes: the current intervals obfuscated
    /// (`obfuscateCurrentStats`), the mappings written, then each interval
    /// (`putUsageStats`).
    pub fn persist(
        &mut self,
        current: &mut [IntervalStats; INTERVAL_COUNT],
    ) -> std::io::Result<()> {
        let bytes: Vec<Vec<u8>> = current
            .iter_mut()
            .map(|stats| write_interval(stats, &mut self.tokens))
            .collect();
        self.write_mappings()?;
        for (interval, (stats, bytes)) in current.iter_mut().zip(bytes).enumerate() {
            self.put_bytes(interval, stats, &bytes)?;
        }
        Ok(())
    }

    /// `filterStats`: the stats and events of packages removed after them
    /// left out.
    pub fn filter_stats(&self, stats: &mut IntervalStats) {
        let removed = &self.tokens.removed_packages;
        if removed.is_empty() {
            return;
        }
        stats.package_stats.retain(|name, s| {
            removed
                .get(name)
                .is_none_or(|&time| s.end_time_stamp >= time)
        });
        stats.events.retain(|e| {
            removed
                .get(&e.package)
                .is_none_or(|&time| time <= e.time_stamp)
        });
    }

    /// `prune`: files older than each interval's retention deleted, the
    /// chooser counts of files older than `retention_days` cleared, then
    /// the files indexed again.
    pub fn prune(&mut self, now: i64, retention_days: i64) {
        let expiry = [
            now - 10 * DAY_IN_MILLIS,
            now - 4 * WEEK_IN_MILLIS,
            now - 6 * MONTH_IN_MILLIS,
            now - 2 * YEAR_IN_MILLIS,
        ];
        for i in (0..INTERVAL_COUNT).rev() {
            for entry in std::fs::read_dir(self.interval_dir(i))
                .into_iter()
                .flatten()
                .flatten()
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                if parse_begin_time(&name).unwrap_or(0) < expiry[i] {
                    atomic_delete(&entry.path());
                }
            }
        }
        let chooser_expiry = now - retention_days * DAY_IN_MILLIS;
        for i in 0..INTERVAL_COUNT {
            for entry in std::fs::read_dir(self.interval_dir(i))
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                let begin = parse_begin_time(&name).unwrap_or(0);
                if begin >= chooser_expiry {
                    continue;
                }
                let pruned = self.read(&path, begin, false).and_then(|mut stats| {
                    if stats
                        .package_stats
                        .values()
                        .all(|s| s.chooser_counts.is_empty())
                    {
                        return Ok(());
                    }
                    for s in stats.package_stats.values_mut() {
                        s.chooser_counts.clear();
                    }
                    let bytes = write_interval(&mut stats, &mut self.tokens);
                    atomic_write(&path, &bytes).map_err(|e| e.to_string())
                });
                if let Err(e) = pruned {
                    eprintln!(
                        "app host: usagestats: Failed to delete chooser counts from usage stats file: {e}"
                    );
                }
            }
        }
        self.index_files();
    }
}

/// A file's last modification in milliseconds (`File.lastModified`).
fn modified_millis(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_host::usage_events::ACTIVITY_RESUMED;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aim-usage-database-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn event(package: &str, event_type: i32, time_stamp: i64, extra: Extra) -> Event {
        Event {
            package: package.into(),
            class: Some(format!("{package}.A")),
            instance_id: -7,
            task_root_package: Some(package.into()),
            task_root_class: Some(format!("{package}.A")),
            event_type,
            time_stamp,
            flags: 0,
            extra,
        }
    }

    /// An interval written against new tokens and read back is the same,
    /// its events' strings and a configuration's bytes kept.
    #[test]
    fn an_interval_round_trips() {
        let begin = 1_000_000;
        let mut stats = IntervalStats::empty(begin);
        stats.end_time = begin + 5000;
        stats.trackers[0].count = 2;
        stats.trackers[0].duration = 300;
        let mut s = UsageStats {
            package: "p".into(),
            last_time_used: begin + 10,
            total_time_in_foreground: 40,
            last_time_visible: begin + 5,
            app_launch_count: 3,
            ..Default::default()
        };
        s.chooser_counts
            .entry("send".into())
            .or_default()
            .insert("text".into(), 2);
        stats.package_stats.insert("p".into(), s);
        stats.configurations.push(ConfigurationStats {
            config: vec![0x08, 0x01],
            last_time_active: begin + 7,
            total_time_active: 9,
            activation_count: 1,
        });
        stats.active_configuration = Some(0);
        stats.add_event(event("p", ACTIVITY_RESUMED, begin + 10, Extra::None));
        stats.add_event(event(
            "p",
            SHORTCUT_INVOCATION,
            begin + 20,
            Extra::ShortcutId("s1".into()),
        ));
        stats.add_event(event(
            "p",
            STANDBY_BUCKET_CHANGED,
            begin + 30,
            Extra::Bucket(0x1e),
        ));
        let mut tokens = Tokens::new();
        let bytes = write_interval(&mut stats.clone(), &mut tokens);
        let read = read_interval(&bytes, begin, &tokens, false).unwrap();
        let mut want = stats.clone();
        for s in want.package_stats.values_mut() {
            s.begin_time_stamp = begin;
            s.end_time_stamp = want.end_time;
        }
        assert_eq!(read, want);
        // The mappings read back give the same tokens.
        let again = Tokens::read(&tokens.write()).unwrap();
        assert_eq!(again.tokens_to_packages, tokens.tokens_to_packages);
        assert_eq!(read_interval(&bytes, begin, &again, false).unwrap(), want);
    }

    /// Events and stats of a removed package are not written.
    #[test]
    fn removed_packages_are_left_out() {
        let mut tokens = Tokens::new();
        tokens.package_token_or_add("gone", 0);
        tokens.remove_package("gone", 100);
        let mut stats = IntervalStats::empty(0);
        stats.add_event(event("gone", ACTIVITY_RESUMED, 50, Extra::None));
        stats.add_event(event("kept", ACTIVITY_RESUMED, 60, Extra::None));
        let bytes = write_interval(&mut stats, &mut tokens);
        let read = read_interval(&bytes, 0, &tokens, false).unwrap();
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].package, "kept");
    }

    /// A new database writes its version, indexes its files, finds the
    /// latest of each interval and the best bucket, and deletes files of
    /// the future.
    #[test]
    fn the_database_indexes_and_queries_its_files() {
        let dir = temp("index");
        let mut db = Database::new(&dir);
        assert_eq!(
            db.init(10 * DAY_IN_MILLIS, "16;REL;1", 14).unwrap(),
            Init {
                new_update: true,
                upgrade_performed: false
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("version")).unwrap(),
            "5\n16;REL;1\n"
        );
        for begin in [DAY_IN_MILLIS, 2 * DAY_IN_MILLIS] {
            let mut stats = IntervalStats::empty(begin);
            stats.end_time = begin + 100;
            stats.add_event(event("p", ACTIVITY_RESUMED, begin + 50, Extra::None));
            db.put(0, &mut stats).unwrap();
        }
        db.write_mappings().unwrap();
        let mut again = Database::new(&dir);
        again.read_mappings();
        // A file of the future is deleted at init.
        std::fs::write(
            dir.join("daily").join((20 * DAY_IN_MILLIS).to_string()),
            b"",
        )
        .unwrap();
        assert_eq!(
            again.init(10 * DAY_IN_MILLIS, "16;REL;1", 14).unwrap(),
            Init {
                new_update: false,
                upgrade_performed: false
            }
        );
        assert_eq!(
            again.begin_times(0).collect::<Vec<_>>(),
            [DAY_IN_MILLIS, 2 * DAY_IN_MILLIS]
        );
        assert_eq!(again.latest(0).unwrap().begin_time, 2 * DAY_IN_MILLIS);
        assert_eq!(again.best_fit_bucket(2 * DAY_IN_MILLIS + 5), 0);
        let mut seen = Vec::new();
        again.query(0, DAY_IN_MILLIS + 60, 3 * DAY_IN_MILLIS, false, |s| {
            seen.push((s.begin_time, s.events[0].package.clone()));
            true
        });
        assert_eq!(
            seen,
            [(DAY_IN_MILLIS, "p".into()), (2 * DAY_IN_MILLIS, "p".into())]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An interval and mappings as the original writes them (an empty
    /// tracker left out, offsets from the begin time, tokens one more) are
    /// read and written back to the same bytes.
    #[test]
    fn the_original_form_round_trips() {
        let mappings = [0x08, 0x02, 0x12, 0x05, 0x08, 0x01, 0x12, 0x01, b'p'];
        let mut tokens = Tokens::read(&mappings).unwrap();
        assert_eq!(tokens.write(), mappings);
        let interval = [
            0x08, 0x06, 0x10, 0x01, 0x18, 0x01, // end, major and minor versions
            0x52, 0x04, 0x08, 0x01, 0x10, 0x05, // interactive: once, for 5 ms
            0xa2, 0x01, 0x06, 0x08, 0x01, 0x18, 0x03, 0x20, 0x04, // "p"'s stats
            0xb2, 0x01, 0x06, 0x08, 0x01, 0x18, 0x02, 0x28, 0x01, // its resume
        ];
        // A begin time of the wall clock: the unset times (0) are long
        // before it, so not written.
        let begin = 1_792_452_905_315;
        let mut stats = read_interval(&interval, begin, &tokens, false).unwrap();
        assert_eq!(stats.end_time, begin + 6);
        assert_eq!(
            (stats.trackers[0].count, stats.trackers[0].duration),
            (1, 5)
        );
        let p = &stats.package_stats["p"];
        assert_eq!(
            (p.last_time_used, p.total_time_in_foreground),
            (begin + 3, 4)
        );
        assert_eq!(
            (stats.events[0].event_type, stats.events[0].time_stamp),
            (ACTIVITY_RESUMED, begin + 2)
        );
        assert_eq!(write_interval(&mut stats, &mut tokens), interval);
    }

    /// A database of an earlier version is not upgraded.
    #[test]
    fn earlier_versions_are_not_upgraded() {
        let dir = temp("version");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("version"), "4\n16;REL;1\n").unwrap();
        assert!(
            Database::new(&dir)
                .init(0, "16;REL;1", 14)
                .unwrap_err()
                .contains("#1866")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
