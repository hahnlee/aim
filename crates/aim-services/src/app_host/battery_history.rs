//! `BatteryStatsHistory` at `android-16.0.0_r1`, with its store
//! (`BatteryHistoryDirectory`, `/data/system/battery-history`) and clock
//! (`MonotonicClock`, `/data/system/monotonic_clock.xml`): the history
//! buffer of `HistoryItem`s, written as deltas (`writeHistoryDelta`) and
//! merged with the item before as the original merges them, its tag pool,
//! and its fragments: the buffer is backed up into the active fragment and
//! a full buffer starts the next one, a completed fragment compressed and
//! the oldest trimmed past the store's size. The items the app host
//! records carry the battery state, the state flags and their events;
//! wake lock and wake reason tags, step details, power stats and process
//! state changes have no producer here (#1928).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use aim_android_xml::{Element, Value};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result as ParcelResult};
use aim_storage::guest_inode::{self, GuestInode};

use super::usage_database::{atomic_delete, atomic_write, mkdirs};

/// The on-disk parcel version.
const VERSION: i32 = 214;

const DELTA_TIME_LONG: i32 = 0x7ffff;
const DELTA_TIME_INT: i32 = 0x7fffe;
const DELTA_TIME_ABS: i32 = 0x7fffd;
const DELTA_BATTERY_LEVEL_FLAG: i32 = 0x0008_0000;
const DELTA_STATE_FLAG: i32 = 0x0010_0000;
const DELTA_STATE2_FLAG: i32 = 0x0020_0000;
const DELTA_EVENT_FLAG: i32 = 0x0080_0000;
const DELTA_BATTERY_CHARGE_FLAG: i32 = 0x0100_0000;
const DELTA_STATE_MASK: i32 = 0xfe00_0000_u32 as i32;
const STATE_BATTERY_MASK: i32 = 0xff00_0000_u32 as i32;
const STATE_BATTERY_STATUS_MASK: i32 = 0x7;
const STATE_BATTERY_STATUS_SHIFT: i32 = 29;
const STATE_BATTERY_HEALTH_MASK: i32 = 0x7;
const STATE_BATTERY_HEALTH_SHIFT: i32 = 26;
const STATE_BATTERY_PLUG_MASK: i32 = 0x3;
const STATE_BATTERY_PLUG_SHIFT: i32 = 24;
const BATTERY_LEVEL_LEVEL_MASK: i32 = 0xff00_0000_u32 as i32;
const BATTERY_LEVEL_LEVEL_SHIFT: i32 = 24;
const BATTERY_LEVEL_TEMP_MASK: i32 = 0x00ff_8000;
const BATTERY_LEVEL_TEMP_SHIFT: i32 = 15;
const BATTERY_LEVEL_VOLT_MASK: i32 = 0x0000_7ffc;
const BATTERY_LEVEL_VOLT_SHIFT: i32 = 2;
const BATTERY_LEVEL_OVERFLOW_FLAG: i32 = 0x2;
const BATTERY_LEVEL2_TEMP_MASK: i32 = 0xffff_0000_u32 as i32;
const BATTERY_LEVEL2_TEMP_SHIFT: i32 = 16;
const BATTERY_LEVEL2_VOLT_MASK: i32 = 0x0000_ffff;
const TAG_FIRST_OCCURRENCE_FLAG: i32 = 0x8000;
const HISTORY_TAG_INDEX_LIMIT: i32 = 0x7ffe;
const MAX_HISTORY_TAG_STRING_LENGTH: usize = 1024;
const STATE2_EXTENSIONS_FLAG: i32 = 1 << 17;
/// `BatteryManager.BATTERY_PLUGGED_AC`, `_USB`, `_WIRELESS`.
const PLUGGED_AC: i8 = 1;
const PLUGGED_USB: i8 = 2;
const PLUGGED_WIRELESS: i8 = 4;

/// `HistoryItem.CMD_*`.
pub const CMD_UPDATE: i8 = 0;
pub const CMD_NULL: i8 = -1;
pub const CMD_START: i8 = 4;
pub const CMD_CURRENT_TIME: i8 = 5;
pub const CMD_RESET: i8 = 7;
/// `HistoryItem`'s states this history records.
pub const STATE_AUDIO_ON_FLAG: i32 = 1 << 22;
pub const STATE_BATTERY_PLUGGED_FLAG: i32 = 1 << 19;
pub const STATE2_VIDEO_ON_FLAG: i32 = 1 << 30;
pub const STATE2_CHARGING_FLAG: i32 = 1 << 24;
const STATE2_GPS_SIGNAL_QUALITY_SHIFT: i32 = 7;
const GNSS_SIGNAL_QUALITY_NONE: i32 = 2;
/// `HistoryItem.EVENT_*`.
pub const EVENT_NONE: i32 = 0;
pub const EVENT_STATE_CHANGE: i32 = 0x15;
pub const EVENT_FLAG_START: i32 = 0x8000;
pub const EVENT_FLAG_FINISH: i32 = 0x4000;

/// `BatteryHistoryDirectory`'s files: their suffix and magic numbers.
const FILE_SUFFIX: &str = ".bh";
const FILE_FORMAT_PARCEL: &[u8; 4] = b"PRCL";
const FILE_FORMAT_COMPRESSED_PARCEL: &[u8; 4] = b"GZIP";
/// What system_server creates in `/data/system` (umask 077).
const SYSTEM_FILE: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o600),
};

/// `HistoryTag`, as the pool keys it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HistoryTag {
    pub string: String,
    pub uid: i32,
}

/// `HistoryItem`: the fields the app host records.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryItem {
    pub time: i64,
    pub cmd: i8,
    pub battery_level: i8,
    pub battery_status: i8,
    pub battery_health: i8,
    pub battery_plug_type: i8,
    pub battery_temperature: i16,
    pub battery_voltage: i16,
    pub battery_charge_uah: i32,
    pub states: i32,
    pub states2: i32,
    pub event_code: i32,
    pub event_tag: Option<HistoryTag>,
    pub tags_first_occurrence: bool,
    pub current_time: i64,
}

impl Default for HistoryItem {
    /// `new HistoryItem()`.
    fn default() -> Self {
        HistoryItem {
            time: 0,
            cmd: CMD_NULL,
            battery_level: 0,
            battery_status: 0,
            battery_health: 0,
            battery_plug_type: 0,
            battery_temperature: 0,
            battery_voltage: 0,
            battery_charge_uah: 0,
            states: 0,
            states2: 0,
            event_code: EVENT_NONE,
            event_tag: None,
            tags_first_occurrence: false,
            current_time: 0,
        }
    }
}

impl HistoryItem {
    /// `clear`.
    fn cleared() -> Self {
        HistoryItem {
            states2: GNSS_SIGNAL_QUALITY_NONE << STATE2_GPS_SIGNAL_QUALITY_SHIFT,
            ..HistoryItem::default()
        }
    }

    /// `writeToParcel`: the whole item.
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.time);
        let bat = (self.cmd as i32 & 0xff)
            | ((self.battery_level as i32) << 8) & 0xff00
            | ((self.battery_status as i32) << 16) & 0xf0000
            | ((self.battery_health as i32) << 20) & 0xf00000
            | ((self.battery_plug_type as i32) << 24) & 0xf000000
            | if self.event_code != EVENT_NONE {
                0x4000_0000
            } else {
                0
            };
        p.write_i32(bat);
        p.write_i32(
            (self.battery_temperature as i32 & 0xffff)
                | ((self.battery_voltage as i32) << 16) & 0xffff_0000_u32 as i32,
        );
        p.write_i32(self.battery_charge_uah);
        // The modem and wifi rail charges, which no rail meters here.
        write_f64(p, 0.0);
        write_f64(p, 0.0);
        p.write_i32(self.states);
        p.write_i32(self.states2);
        if self.event_code != EVENT_NONE {
            p.write_i32(self.event_code);
            write_tag(p, self.event_tag.as_ref());
        }
        if self.cmd == CMD_CURRENT_TIME || self.cmd == CMD_RESET {
            p.write_i64(self.current_time);
        }
    }
}

/// `Parcel.writeDouble`.
fn write_f64(p: &mut Parcel, value: f64) {
    p.write_i64(value.to_bits() as i64);
}

/// `HistoryTag.writeToParcel`.
fn write_tag(p: &mut Parcel, tag: Option<&HistoryTag>) {
    let tag = tag.expect("an event has its tag");
    p.write_string16(Some(&tag.string));
    p.write_i32(tag.uid);
}

/// `signedValueFits`.
fn signed_value_fits(value: i32, mask: i32, shift: i32) -> bool {
    let mask = ((mask as u32) >> shift) as i32;
    let msb_and_lost_bits_mask = !(((mask as u32) >> 1) as i32);
    let msb_and_lost_bits = value & msb_and_lost_bits_mask;
    msb_and_lost_bits == 0 || msb_and_lost_bits == msb_and_lost_bits_mask
}

/// `buildBatteryLevelInt`, with the original's pairing of masks and
/// shifts in its overflow check.
fn battery_level_int(cur: &HistoryItem, prev: &HistoryItem) -> i32 {
    let level_delta = cur.battery_level as i32 - prev.battery_level as i32;
    let temp_delta = cur.battery_temperature as i32 - prev.battery_temperature as i32;
    let volt_delta = cur.battery_voltage as i32 - prev.battery_voltage as i32;
    let overflow = !signed_value_fits(
        temp_delta,
        BATTERY_LEVEL_TEMP_MASK,
        BATTERY_LEVEL_VOLT_SHIFT,
    ) || !signed_value_fits(
        volt_delta,
        BATTERY_LEVEL_VOLT_MASK,
        BATTERY_LEVEL_TEMP_SHIFT,
    );
    let mut batt =
        level_delta.wrapping_shl(BATTERY_LEVEL_LEVEL_SHIFT as u32) & BATTERY_LEVEL_LEVEL_MASK;
    if overflow {
        batt |= BATTERY_LEVEL_OVERFLOW_FLAG;
    } else {
        batt |= temp_delta.wrapping_shl(BATTERY_LEVEL_TEMP_SHIFT as u32) & BATTERY_LEVEL_TEMP_MASK;
        batt |= volt_delta.wrapping_shl(BATTERY_LEVEL_VOLT_SHIFT as u32) & BATTERY_LEVEL_VOLT_MASK;
    }
    batt
}

/// `buildExtendedBatteryLevelInt`.
fn extended_battery_level_int(cur: &HistoryItem) -> i32 {
    ((cur.battery_temperature as i32).wrapping_shl(BATTERY_LEVEL2_TEMP_SHIFT as u32)
        & BATTERY_LEVEL2_TEMP_MASK)
        | (cur.battery_voltage as i32 & BATTERY_LEVEL2_VOLT_MASK)
}

/// `buildStateInt`.
fn state_int(h: &HistoryItem) -> i32 {
    let plug = if h.battery_plug_type & PLUGGED_AC != 0 {
        1
    } else if h.battery_plug_type & PLUGGED_USB != 0 {
        2
    } else if h.battery_plug_type & PLUGGED_WIRELESS != 0 {
        3
    } else {
        0
    };
    ((h.battery_status as i32 & STATE_BATTERY_STATUS_MASK) << STATE_BATTERY_STATUS_SHIFT)
        | ((h.battery_health as i32 & STATE_BATTERY_HEALTH_MASK) << STATE_BATTERY_HEALTH_SHIFT)
        | ((plug & STATE_BATTERY_PLUG_MASK) << STATE_BATTERY_PLUG_SHIFT)
        | (h.states & !STATE_BATTERY_MASK)
}

/// The history tag pool (`mHistoryTagPool`): each tag's index, with
/// `TAG_FIRST_OCCURRENCE_FLAG` while the current buffer has not written
/// it.
#[derive(Debug, Default)]
struct TagPool {
    tags: HashMap<HistoryTag, i32>,
    next: i32,
}

impl TagPool {
    /// `writeHistoryTag`.
    fn index(&mut self, tag: &mut HistoryTag) -> i32 {
        if tag.string.chars().count() > MAX_HISTORY_TAG_STRING_LENGTH {
            tag.string = tag
                .string
                .chars()
                .take(MAX_HISTORY_TAG_STRING_LENGTH)
                .collect();
        }
        if let Some(idx) = self.tags.get_mut(tag) {
            let found = *idx;
            *idx &= !TAG_FIRST_OCCURRENCE_FLAG;
            found
        } else if self.next < HISTORY_TAG_INDEX_LIMIT {
            let idx = self.next;
            self.tags.insert(tag.clone(), idx);
            self.next += 1;
            idx | TAG_FIRST_OCCURRENCE_FLAG
        } else {
            HISTORY_TAG_INDEX_LIMIT | TAG_FIRST_OCCURRENCE_FLAG
        }
    }

    /// A new buffer has written none of them.
    fn mark_unwritten(&mut self) {
        for idx in self.tags.values_mut() {
            *idx |= TAG_FIRST_OCCURRENCE_FLAG;
        }
    }
}

/// `writeHistoryDelta`: `cur` after `last`, or whole if it is no update.
fn write_delta(dest: &mut Parcel, pool: &mut TagPool, cur: &mut HistoryItem, last: &HistoryItem) {
    if cur.cmd != CMD_UPDATE {
        dest.write_i32(DELTA_TIME_ABS);
        cur.write_to(dest);
        return;
    }
    let delta_time = cur.time - last.time;
    let battery_level_int = battery_level_int(cur, last);
    let last_state_int = state_int(last);
    let delta_time_token = if !(0..=i32::MAX as i64).contains(&delta_time) {
        DELTA_TIME_LONG
    } else if delta_time >= DELTA_TIME_ABS as i64 {
        DELTA_TIME_INT
    } else {
        delta_time as i32
    };
    let mut first_token = delta_time_token | (cur.states & DELTA_STATE_MASK);
    let battery_level_int_changed = battery_level_int != 0;
    if battery_level_int_changed {
        first_token |= DELTA_BATTERY_LEVEL_FLAG;
    }
    let state_int = state_int(cur);
    let state_int_changed = state_int != last_state_int;
    if state_int_changed {
        first_token |= DELTA_STATE_FLAG;
    }
    // No extensions: the item has no power stats or process state change.
    cur.states2 &= !STATE2_EXTENSIONS_FLAG;
    let state2_int_changed = cur.states2 != last.states2;
    if state2_int_changed {
        first_token |= DELTA_STATE2_FLAG;
    }
    if cur.event_code != EVENT_NONE {
        first_token |= DELTA_EVENT_FLAG;
    }
    let battery_charge_changed = cur.battery_charge_uah != last.battery_charge_uah;
    if battery_charge_changed {
        first_token |= DELTA_BATTERY_CHARGE_FLAG;
    }
    dest.write_i32(first_token);
    if delta_time_token == DELTA_TIME_INT {
        dest.write_i32(delta_time as i32);
    } else if delta_time_token == DELTA_TIME_LONG {
        dest.write_i64(delta_time);
    }
    if battery_level_int_changed {
        dest.write_i32(battery_level_int);
        if battery_level_int & BATTERY_LEVEL_OVERFLOW_FLAG != 0 {
            dest.write_i32(extended_battery_level_int(cur));
        }
    }
    if state_int_changed {
        dest.write_i32(state_int);
    }
    if state2_int_changed {
        dest.write_i32(cur.states2);
    }
    cur.tags_first_occurrence = false;
    if cur.event_code != EVENT_NONE {
        let tag = cur.event_tag.as_mut().expect("an event has its tag");
        let index = pool.index(tag);
        dest.write_i32((cur.event_code & 0xffff) | (index << 16));
        if index & TAG_FIRST_OCCURRENCE_FLAG != 0 {
            write_tag(dest, Some(tag));
            cur.tags_first_occurrence = true;
        }
    }
    if battery_charge_changed {
        dest.write_i32(cur.battery_charge_uah);
    }
    write_f64(dest, 0.0);
    write_f64(dest, 0.0);
}

/// `MonotonicClock`: the elapsed realtime shifted to keep increasing
/// across boots, the shift kept in its file.
#[derive(Debug)]
pub struct MonotonicClock {
    file: PathBuf,
    timeshift: i64,
}

impl MonotonicClock {
    pub fn new(file: PathBuf, elapsed: i64) -> Self {
        let timeshift = std::fs::read(&file)
            .ok()
            .and_then(|bytes| {
                let saved = aim_android_xml::read(&bytes)
                    .map_err(|e| {
                        eprintln!(
                            "app host: Cannot load monotonic clock from {}: {e}",
                            file.display()
                        )
                    })
                    .ok()?;
                (saved.name == "monotonic_time")
                    .then(|| saved.long("timeshift").ok().flatten())
                    .flatten()
            })
            .map_or(0, |saved| saved - elapsed);
        MonotonicClock { file, timeshift }
    }

    /// `monotonicTime(elapsedRealtimeMs)`.
    pub fn time(&self, elapsed: i64) -> i64 {
        self.timeshift + elapsed
    }

    /// `write`: the current monotonic time, which the next boot starts
    /// from.
    fn write(&self, elapsed: i64) {
        let root = Element {
            name: "monotonic_time".into(),
            attrs: vec![("timeshift".into(), Value::Long(self.time(elapsed)))],
            content: Vec::new(),
        };
        let written = aim_android_xml::abx::write(&root)
            .and_then(|bytes| atomic_write(&self.file, &bytes).map_err(|e| e.to_string()));
        if let Err(e) = written {
            eprintln!(
                "app host: Cannot write monotonic clock to {}: {e}",
                self.file.display()
            );
        }
    }
}

/// `BatteryHistoryDirectory`: the fragments, by their monotonic start
/// time, oldest first.
#[derive(Debug)]
pub struct Directory {
    dir: PathBuf,
    max_size: u64,
    compression: bool,
    fragments: Option<Vec<i64>>,
}

impl Directory {
    pub fn new(dir: PathBuf, max_size: u64, compression: bool) -> Self {
        Directory {
            dir,
            max_size,
            compression,
            fragments: None,
        }
    }

    fn path(&self, fragment: i64) -> PathBuf {
        self.dir.join(format!("{fragment}{FILE_SUFFIX}"))
    }

    /// `ensureInitialized`: the files named by their time; others are
    /// removed.
    fn fragments(&mut self) -> &mut Vec<i64> {
        if self.fragments.is_none() {
            if let Err(e) = mkdirs(&self.dir) {
                eprintln!(
                    "app host: HistoryDir does not exist: {}: {e}",
                    self.dir.display()
                );
            }
            let mut found = Vec::new();
            for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let time = name
                    .rfind(FILE_SUFFIX)
                    .filter(|&b| b > 0)
                    .and_then(|b| name[..b].parse::<i64>().ok());
                match time {
                    Some(t) if !found.contains(&t) => found.push(t),
                    Some(_) => {}
                    None => {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
            found.sort();
            self.fragments = Some(found);
        }
        self.fragments.as_mut().expect("initialized")
    }

    fn latest(&mut self) -> Option<i64> {
        self.fragments().last().copied()
    }

    fn has_completed(&mut self) -> bool {
        self.fragments().len() > 1
    }

    /// `createFragment`: an empty file starting at `time`.
    fn create(&mut self, time: i64) -> i64 {
        self.fragments();
        let path = self.path(time);
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|_| guest_inode::record(&path, SYSTEM_FILE));
        if let Err(e) = created
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            eprintln!(
                "app host: Could not create history file: {}: {e}",
                path.display()
            );
        }
        self.fragments().push(time);
        time
    }

    /// `writeFragment`: the raw bytes, then, for a completed fragment,
    /// compressed and the oldest trimmed.
    fn write(&mut self, fragment: i64, data: &[u8], complete: bool) {
        let path = self.path(fragment);
        let mut bytes = FILE_FORMAT_PARCEL.to_vec();
        bytes.extend_from_slice(&(data.len() as i32).to_be_bytes());
        bytes.extend_from_slice(data);
        if let Err(e) = atomic_write(&path, &bytes) {
            return eprintln!("app host: Error writing battery history fragment: {e}");
        }
        if complete {
            if self.compression && !data.is_empty() {
                let mut bytes = FILE_FORMAT_COMPRESSED_PARCEL.to_vec();
                bytes.extend_from_slice(&(data.len() as i32).to_be_bytes());
                let mut gz = flate2::write::GzEncoder::new(bytes, flate2::Compression::fast());
                let compressed = gz
                    .write_all(data)
                    .and_then(|()| gz.finish())
                    .and_then(|bytes| atomic_write(&path, &bytes));
                if let Err(e) = compressed {
                    eprintln!(
                        "app host: Error compressing battery history chunk {}: {e}",
                        path.display()
                    );
                }
            }
            self.trim();
        }
    }

    /// `readFragment`: its bytes, empty for a new file; a missing or
    /// invalid one is removed.
    fn read(&mut self, fragment: i64) -> Option<Vec<u8>> {
        let path = self.path(fragment);
        let read = || -> std::io::Result<Vec<u8>> {
            let bytes = std::fs::read(&path)?;
            if bytes.is_empty() {
                return Ok(bytes);
            }
            let invalid = || std::io::Error::other("Invalid battery history file format");
            let (header, rest) = bytes.split_at_checked(4).ok_or_else(invalid)?;
            let (size, rest) = rest.split_at_checked(4).ok_or_else(invalid)?;
            let size = i32::from_be_bytes(size.try_into().expect("four bytes"));
            if !(0..=10_000_000).contains(&size) {
                return Err(invalid());
            }
            let mut data = vec![0; size as usize];
            match header {
                h if h == FILE_FORMAT_COMPRESSED_PARCEL => {
                    flate2::read::GzDecoder::new(rest).read_exact(&mut data)?
                }
                h if h == FILE_FORMAT_PARCEL => {
                    data.copy_from_slice(rest.get(..size as usize).ok_or_else(invalid)?)
                }
                _ => return Err(invalid()),
            }
            Ok(data)
        };
        match read() {
            Ok(data) => Some(data),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("app host: Error reading file {}: {e}", path.display());
                }
                self.fragments().retain(|&f| f != fragment);
                atomic_delete(&path);
                None
            }
        }
    }

    /// `reset`: every fragment removed.
    fn reset(&mut self) {
        for f in std::mem::take(self.fragments()) {
            atomic_delete(&self.path(f));
        }
    }

    /// `trim`: the oldest fragments removed while the store is larger
    /// than its maximum, the newest kept.
    fn trim(&mut self) {
        let len = |f: i64, d: &Directory| std::fs::metadata(d.path(f)).map_or(0, |m| m.len());
        let fragments = self.fragments().clone();
        let mut size: u64 = fragments.iter().map(|&f| len(f, self)).sum();
        for &oldest in &fragments[..fragments.len().saturating_sub(1)] {
            if size <= self.max_size {
                break;
            }
            size -= len(oldest, self);
            atomic_delete(&self.path(oldest));
            self.fragments().remove(0);
        }
    }
}

/// A persisted history buffer (`writeHistoryBuffer`).
struct Saved {
    start: i64,
    end: i64,
    size: i64,
    buffer: Vec<u8>,
}

/// `readHistoryBuffer`'s header and buffer; `None` for another version.
fn read_history_buffer(data: &[u8], max_buffer: usize) -> ParcelResult<Option<Saved>> {
    let mut r = Reader::new(data, &[]);
    if r.read_i32()? != VERSION {
        return Ok(None);
    }
    let start = r.read_i64()?;
    let end = r.read_i64()?;
    let size = r.read_i64()?;
    let len = r.read_i32()?;
    if len < 0 || len as usize >= max_buffer * 100 || len & !3 != len {
        return Err(BAD_VALUE);
    }
    let at = r.position();
    let buffer = data.get(at..at + len as usize).ok_or(BAD_VALUE)?;
    Ok(Some(Saved {
        start,
        end,
        size,
        buffer: buffer.to_vec(),
    }))
}

/// `BatteryStatsHistory`, writable: what it keeps in memory, its clock and
/// its store.
#[derive(Debug)]
pub struct History {
    buffer: Vec<u8>,
    max_buffer: usize,
    store: Directory,
    clock: MonotonicClock,
    active: i64,
    cur: HistoryItem,
    last_written: HistoryItem,
    last_last_written: HistoryItem,
    pool: TagPool,
    last_pos: Option<usize>,
    buffer_start_time: i64,
    monotonic_end_time: i64,
    monotonic_size: i64,
    have_battery_level: bool,
    recording: bool,
}

/// `System.currentTimeMillis`.
fn current_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl History {
    /// The history of `store` at elapsed `now`, continuing its latest
    /// fragment or a new one.
    pub fn new(store: Directory, clock: MonotonicClock, max_buffer: usize, now: i64) -> Self {
        let mut h = History {
            buffer: Vec::new(),
            max_buffer,
            store,
            active: 0,
            buffer_start_time: clock.time(now),
            clock,
            cur: HistoryItem::default(),
            last_written: HistoryItem::cleared(),
            last_last_written: HistoryItem::cleared(),
            pool: TagPool::default(),
            last_pos: None,
            monotonic_end_time: 0,
            monotonic_size: 0,
            have_battery_level: false,
            recording: false,
        };
        h.active = match h.store.latest() {
            Some(latest) => latest,
            None => h.store.create(h.clock.time(now)),
        };
        h
    }

    /// `initHistoryBuffer`.
    fn init_buffer(&mut self, now: i64) {
        self.buffer_start_time = self.clock.time(now);
        self.buffer.clear();
        self.last_last_written = HistoryItem::cleared();
        self.last_written = HistoryItem::cleared();
        self.pool = TagPool::default();
        self.last_pos = None;
    }

    /// `reset`: the buffer and every fragment cleared.
    pub fn reset(&mut self, now: i64) {
        self.monotonic_size = 0;
        self.init_buffer(now);
        self.store.reset();
        self.active = self.store.create(self.buffer_start_time);
    }

    /// `readSummary`: the active fragment's buffer, if it has one; false
    /// when it cannot be read.
    pub fn read_summary(&mut self, now: i64) -> bool {
        let Some(data) = self.store.read(self.active) else {
            return false;
        };
        if data.is_empty() {
            return true;
        }
        match read_history_buffer(&data, self.max_buffer) {
            Ok(Some(saved)) => {
                self.buffer_start_time = saved.start;
                self.monotonic_end_time = saved.end;
                self.monotonic_size = saved.size;
                self.buffer = saved.buffer;
                true
            }
            Ok(None) => {
                eprintln!(
                    "app host: BatteryStats: readHistoryBuffer: other version; erasing old stats"
                );
                true
            }
            Err(_) => {
                eprintln!("app host: BatteryStatsHistory: Error reading battery history");
                self.reset(now);
                false
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// `continueRecordingHistory`: a start and the time, if there is
    /// history to continue.
    pub fn continue_recording(&mut self, now: i64) {
        if self.buffer.is_empty() && !self.store.has_completed() {
            return;
        }
        self.recording = true;
        let cur = std::mem::take(&mut self.cur);
        self.cur = self.write_cmd(now, cur, CMD_START);
        self.start_recording(now, false);
    }

    /// `startRecordingHistory`: the wall clock time, as a reset or as a
    /// time change.
    pub fn start_recording(&mut self, now: i64, reset: bool) {
        self.recording = true;
        self.cur.current_time = current_time();
        let cur = std::mem::take(&mut self.cur);
        self.cur = self.write_cmd(now, cur, if reset { CMD_RESET } else { CMD_CURRENT_TIME });
        self.cur.current_time = 0;
    }

    /// `setBatteryState(charging, status, level, chargeUah)`.
    pub fn set_initial_battery_state(
        &mut self,
        charging: bool,
        status: i32,
        level: i32,
        charge: i32,
    ) {
        self.have_battery_level = true;
        self.set_charging_state(charging);
        self.cur.battery_status = status as i8;
        self.cur.battery_level = level as i8;
        self.cur.battery_charge_uah = charge;
    }

    /// `setBatteryState(status, level, health, plugType, temperature,
    /// voltageMv, chargeUah)`.
    #[allow(clippy::too_many_arguments)]
    pub fn set_battery_state(
        &mut self,
        status: i32,
        level: i32,
        health: i32,
        plug_type: i32,
        temperature: i32,
        voltage: i32,
        charge: i32,
    ) {
        self.have_battery_level = true;
        self.cur.battery_status = status as i8;
        self.cur.battery_level = level as i8;
        self.cur.battery_health = health as i8;
        self.cur.battery_plug_type = plug_type as i8;
        self.cur.battery_temperature = temperature as i16;
        self.cur.battery_voltage = voltage as i16;
        self.cur.battery_charge_uah = charge;
    }

    /// `setPluggedInState`.
    pub fn set_plugged_in_state(&mut self, plugged_in: bool) {
        set_flags(&mut self.cur.states, STATE_BATTERY_PLUGGED_FLAG, plugged_in);
    }

    /// `setChargingState`.
    pub fn set_charging_state(&mut self, charging: bool) {
        set_flags(&mut self.cur.states2, STATE2_CHARGING_FLAG, charging);
    }

    /// `recordBatteryState`.
    pub fn record_battery_state(&mut self, now: i64, level: i32, plugged: bool) {
        self.cur.battery_level = level as i8;
        self.set_plugged_in_state(plugged);
        self.write_item(now);
    }

    /// `recordStateStartEvent`/`recordStateStopEvent`, the latter as
    /// `on` false, with their state change event if `event` names it.
    pub fn record_state(&mut self, now: i64, flags: i32, on: bool, event: Option<(i32, &str)>) {
        set_flags(&mut self.cur.states, flags, on);
        self.set_event(on, event);
        self.write_item(now);
    }

    /// `recordState2StartEvent`/`recordState2StopEvent`.
    pub fn record_state2(&mut self, now: i64, flags: i32, on: bool, event: Option<(i32, &str)>) {
        set_flags(&mut self.cur.states2, flags, on);
        self.set_event(on, event);
        self.write_item(now);
    }

    fn set_event(&mut self, start: bool, event: Option<(i32, &str)>) {
        if let Some((uid, name)) = event {
            self.cur.event_code = EVENT_STATE_CHANGE
                | if start {
                    EVENT_FLAG_START
                } else {
                    EVENT_FLAG_FINISH
                };
            self.cur.event_tag = Some(HistoryTag {
                string: name.into(),
                uid,
            });
        }
    }

    /// `writeHistoryItem(elapsedRealtimeMs, uptimeMs)`: the current item,
    /// merged into the last one written when only states that toggled
    /// neither now nor before it changed, within a second.
    pub fn write_item(&mut self, mut now: i64) {
        if !self.have_battery_level || !self.recording {
            return;
        }
        let (cur, last, last_last) = (&self.cur, &self.last_written, &self.last_last_written);
        let time_diff = self.clock.time(now) - last.time;
        let diff_states = last.states ^ cur.states;
        let diff_states2 = last.states2 ^ cur.states2;
        let last_diff_states = last.states ^ last_last.states;
        let last_diff_states2 = last.states2 ^ last_last.states2;
        if let Some(pos) = self.last_pos
            && last.cmd == CMD_UPDATE
            && time_diff < 1000
            && diff_states & last_diff_states == 0
            && diff_states2 & last_diff_states2 == 0
            && !last.tags_first_occurrence
            && !cur.tags_first_occurrence
            && (last.event_code == EVENT_NONE || cur.event_code == EVENT_NONE)
            && last.battery_level == cur.battery_level
            && last.battery_status == cur.battery_status
            && last.battery_health == cur.battery_health
            && last.battery_plug_type == cur.battery_plug_type
            && last.battery_temperature == cur.battery_temperature
            && last.battery_voltage == cur.battery_voltage
        {
            self.monotonic_size -= (self.buffer.len() - pos) as i64;
            self.buffer.truncate(pos);
            self.last_pos = None;
            now -= time_diff;
            if self.last_written.event_code != EVENT_NONE {
                self.cur.event_code = self.last_written.event_code;
                self.cur.event_tag = self.last_written.event_tag.clone();
            }
            self.last_written = self.last_last_written.clone();
        }
        if self.buffer.len() >= self.max_buffer {
            // `maybeFlushBufferAndWriteHistoryItem`.
            let copy = self.cur.clone();
            self.start_next_fragment(now);
            self.start_recording(now, false);
            self.write_cmd(now, copy, CMD_UPDATE);
            return;
        }
        if self.buffer.is_empty() {
            // The history starts with a time stamp.
            let copy = HistoryItem {
                current_time: current_time(),
                event_code: EVENT_NONE,
                event_tag: None,
                tags_first_occurrence: false,
                ..self.cur.clone()
            };
            self.write_cmd(now, copy, CMD_RESET);
        }
        let cur = std::mem::take(&mut self.cur);
        self.cur = self.write_cmd(now, cur, CMD_UPDATE);
    }

    /// `writeHistoryItem(elapsedRealtimeMs, uptimeMs, cur, cmd)`: `cur`
    /// written as `cmd` at `now`, given back without its event.
    fn write_cmd(&mut self, now: i64, mut cur: HistoryItem, cmd: i8) -> HistoryItem {
        self.last_pos = Some(self.buffer.len());
        self.last_last_written = self.last_written.clone();
        let has_tags = self.last_written.tags_first_occurrence || cur.tags_first_occurrence;
        let mut written = HistoryItem {
            time: self.clock.time(now),
            cmd,
            tags_first_occurrence: has_tags,
            ..cur.clone()
        };
        let mut p = Parcel::new();
        self.monotonic_end_time = written.time;
        write_delta(
            &mut p,
            &mut self.pool,
            &mut written,
            &self.last_last_written,
        );
        self.buffer.extend_from_slice(p.data());
        self.monotonic_size += p.data().len() as i64;
        self.last_written = written;
        cur.event_code = EVENT_NONE;
        cur.event_tag = None;
        cur.tags_first_occurrence = false;
        cur
    }

    /// `startNextFragment`: the buffer written as a completed fragment, and
    /// a new one begun.
    fn start_next_fragment(&mut self, now: i64) {
        self.write_history(now, true);
        let start = self.clock.time(now);
        self.active = self.store.create(start);
        self.buffer_start_time = start;
        self.buffer.clear();
        self.last_pos = None;
        self.last_written = HistoryItem::cleared();
        self.last_last_written = HistoryItem::cleared();
        self.pool.mark_unwritten();
    }

    /// `writeHistory`: the monotonic clock, then the buffer backed up into
    /// the active fragment (`writeHistoryBuffer`).
    pub fn write_history(&mut self, now: i64, complete: bool) {
        self.clock.write(now);
        let mut p = Parcel::new();
        p.write_i32(VERSION);
        p.write_i64(self.buffer_start_time);
        p.write_i64(self.monotonic_end_time);
        p.write_i64(self.monotonic_size);
        p.write_i32(self.buffer.len() as i32);
        let mut data = p.data().to_vec();
        data.extend_from_slice(&self.buffer);
        self.store.write(self.active, &data, complete);
    }
}

fn set_flags(states: &mut i32, flags: i32, on: bool) {
    if on {
        *states |= flags;
    } else {
        *states &= !flags;
    }
}

/// The store's directory and the clock's file under `/data/system`.
pub fn paths(system: &Path) -> (PathBuf, PathBuf) {
    (
        system.join("battery-history"),
        system.join("monotonic_clock.xml"),
    )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::app_host) fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aim-battery-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(in crate::app_host) fn history(dir: &Path, max_buffer: usize, now: i64) -> History {
        let (store, clock) = paths(dir);
        History::new(
            Directory::new(store, 4 * 1024 * 1024, true),
            MonotonicClock::new(clock, now),
            max_buffer,
            now,
        )
    }

    /// An event: its code, tag index and, at its first occurrence, the tag.
    type DecodedEvent = (i32, i32, Option<(String, i32)>);

    /// A reader of the original's delta format (`readHistoryDelta`), for
    /// what these tests write.
    #[derive(Debug, Default)]
    pub(in crate::app_host) struct Decoded {
        pub cmd: i8,
        pub time: i64,
        pub level: i32,
        pub states: i32,
        pub states2: i32,
        event: Option<DecodedEvent>,
    }

    impl History {
        pub(in crate::app_host) fn buffer(&self) -> &[u8] {
            &self.buffer
        }
    }

    pub(in crate::app_host) fn decode(buffer: &[u8]) -> Vec<Decoded> {
        let mut r = Reader::new(buffer, &[]);
        let mut out: Vec<Decoded> = Vec::new();
        while r.remaining() > 0 {
            let first = r.read_i32().unwrap();
            let mut d = Decoded::default();
            let last = out.last();
            if first & 0x7ffff == DELTA_TIME_ABS {
                d.time = r.read_i64().unwrap();
                let bat = r.read_i32().unwrap();
                d.cmd = bat as i8;
                d.level = (bat >> 8) & 0xff;
                r.read_i32().unwrap();
                r.read_i32().unwrap();
                r.read_i64().unwrap();
                r.read_i64().unwrap();
                d.states = r.read_i32().unwrap();
                d.states2 = r.read_i32().unwrap();
                if bat & 0x4000_0000 != 0 {
                    let code = r.read_i32().unwrap();
                    let s = r.read_string16().unwrap().unwrap();
                    let uid = r.read_i32().unwrap();
                    d.event = Some((code, -1, Some((s, uid))));
                }
                if d.cmd == CMD_CURRENT_TIME || d.cmd == CMD_RESET {
                    r.read_i64().unwrap();
                }
            } else {
                let last = last.unwrap();
                d.cmd = CMD_UPDATE;
                let token = first & 0x7ffff;
                d.time = last.time
                    + match token {
                        DELTA_TIME_INT => r.read_i32().unwrap() as i64,
                        DELTA_TIME_LONG => r.read_i64().unwrap(),
                        t => t as i64,
                    };
                d.level = last.level;
                let mut state_int = None;
                if first & DELTA_BATTERY_LEVEL_FLAG != 0 {
                    let b = r.read_i32().unwrap();
                    d.level += b >> BATTERY_LEVEL_LEVEL_SHIFT;
                    if b & BATTERY_LEVEL_OVERFLOW_FLAG != 0 {
                        r.read_i32().unwrap();
                    }
                }
                if first & DELTA_STATE_FLAG != 0 {
                    state_int = Some(r.read_i32().unwrap());
                }
                d.states = (first & DELTA_STATE_MASK)
                    | (state_int.unwrap_or(last.states) & !DELTA_STATE_MASK);
                d.states2 = if first & DELTA_STATE2_FLAG != 0 {
                    r.read_i32().unwrap()
                } else {
                    last.states2
                };
                if first & DELTA_EVENT_FLAG != 0 {
                    let ci = r.read_i32().unwrap();
                    let index = (ci >> 16) & 0xffff;
                    let tag = (index & TAG_FIRST_OCCURRENCE_FLAG != 0).then(|| {
                        let s = r.read_string16().unwrap().unwrap();
                        (s, r.read_i32().unwrap())
                    });
                    d.event = Some((ci & 0xffff, index & !TAG_FIRST_OCCURRENCE_FLAG, tag));
                }
                if first & DELTA_BATTERY_CHARGE_FLAG != 0 {
                    r.read_i32().unwrap();
                }
                r.read_i64().unwrap();
                r.read_i64().unwrap();
            }
            out.push(d);
        }
        out
    }

    #[test]
    fn the_first_item_follows_a_reset_and_a_change_is_a_delta() {
        let dir = temp_dir("first");
        let mut h = history(&dir, 128 * 1024, 1_000);
        h.set_initial_battery_state(true, 3, 50, 0);
        h.start_recording(1_000, true);
        h.record_battery_state(2_000, 49, false);
        h.record_state(5_000, STATE_AUDIO_ON_FLAG, true, Some((10_123, "audio")));
        let items = decode(&h.buffer);
        let cmds: Vec<i8> = items.iter().map(|d| d.cmd).collect();
        assert_eq!(cmds, [CMD_RESET, CMD_UPDATE, CMD_UPDATE]);
        assert_eq!(items[1].level, 49);
        assert_eq!(items[2].time - items[1].time, 3_000);
        assert_ne!(items[2].states & STATE_AUDIO_ON_FLAG, 0);
        assert_eq!(
            items[2].event,
            Some((
                EVENT_STATE_CHANGE | EVENT_FLAG_START,
                0,
                Some(("audio".into(), 10_123))
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tag_is_written_once_per_buffer() {
        let dir = temp_dir("tags");
        let mut h = history(&dir, 128 * 1024, 0);
        h.set_initial_battery_state(true, 3, 50, 0);
        h.start_recording(0, true);
        h.record_state2(2_000, STATE2_VIDEO_ON_FLAG, true, Some((10_001, "video")));
        h.record_state2(4_000, STATE2_VIDEO_ON_FLAG, false, Some((10_001, "video")));
        let items = decode(&h.buffer);
        let tags: Vec<_> = items.iter().filter_map(|d| d.event.clone()).collect();
        assert_eq!(tags.len(), 2);
        assert!(tags[0].2.is_some());
        assert_eq!(tags[1].2, None, "the second occurrence is its index only");
        assert_eq!(tags[1].0, EVENT_STATE_CHANGE | EVENT_FLAG_FINISH);
        assert_eq!(items[2].states2 & STATE2_VIDEO_ON_FLAG, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_within_a_second_merges_into_the_last() {
        let dir = temp_dir("merge");
        let mut h = history(&dir, 128 * 1024, 0);
        h.set_initial_battery_state(true, 3, 50, 0);
        h.start_recording(0, true);
        h.record_battery_state(2_000, 50, false);
        h.record_state(5_000, STATE_AUDIO_ON_FLAG, true, None);
        h.record_state2(5_400, STATE2_VIDEO_ON_FLAG, true, None);
        let items = decode(&h.buffer);
        assert_eq!(
            items.len(),
            3,
            "the video start merged into the audio start"
        );
        assert_eq!(items[2].time - items[1].time, 3_000);
        assert_ne!(items[2].states & STATE_AUDIO_ON_FLAG, 0);
        assert_ne!(items[2].states2 & STATE2_VIDEO_ON_FLAG, 0);
        // A bit that changed into the last item toggling back is kept.
        h.record_state2(5_600, STATE2_VIDEO_ON_FLAG, false, None);
        assert_eq!(decode(&h.buffer).len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_buffer_is_persisted_and_read_back_on_the_next_boot() {
        let dir = temp_dir("persist");
        let mut h = history(&dir, 128 * 1024, 10_000);
        h.set_initial_battery_state(true, 3, 50, 0);
        h.start_recording(10_000, true);
        h.record_battery_state(12_000, 49, false);
        h.write_history(13_000, false);
        let (store, clock) = paths(&dir);
        let files: Vec<_> = std::fs::read_dir(&store)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".bh"))
            .collect();
        assert_eq!(files, ["10000.bh"]);
        let raw = std::fs::read(store.join("10000.bh")).unwrap();
        assert_eq!(&raw[..4], b"PRCL");
        assert_eq!(
            i32::from_be_bytes(raw[4..8].try_into().unwrap()) as usize,
            raw.len() - 8
        );
        assert_eq!(i32::from_le_bytes(raw[8..12].try_into().unwrap()), VERSION);
        let saved = aim_android_xml::read(&std::fs::read(clock).unwrap()).unwrap();
        assert_eq!(saved.long("timeshift").unwrap(), Some(13_000));

        // The next boot's elapsed time starts again; the monotonic time
        // continues from the saved one.
        let mut next = history(&dir, 128 * 1024, 100);
        assert_eq!(next.clock.time(100), 13_000);
        assert!(next.read_summary(100));
        assert_eq!(next.buffer, h.buffer);
        next.continue_recording(100);
        let items = decode(&next.buffer);
        let cmds: Vec<i8> = items.iter().map(|d| d.cmd).collect();
        assert_eq!(cmds, [CMD_RESET, CMD_UPDATE, CMD_START, CMD_CURRENT_TIME]);
        assert_eq!(items[2].time, 13_000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_buffer_completes_its_fragment_compressed() {
        let dir = temp_dir("full");
        let mut h = history(&dir, 64, 0);
        h.set_initial_battery_state(true, 3, 50, 0);
        h.start_recording(0, true);
        for i in 1..=6 {
            h.record_battery_state(i * 2_000, 50 - i as i32, false);
        }
        let (store, _) = paths(&dir);
        let mut names: Vec<_> = std::fs::read_dir(&store)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert!(names.len() >= 2, "{names:?}");
        let first = std::fs::read(store.join("0.bh")).unwrap();
        assert_eq!(&first[..4], b"GZIP");
        let mut s = Directory::new(store, 4 * 1024 * 1024, true);
        let data = s.read(0).unwrap();
        let Saved { start, buffer, .. } = read_history_buffer(&data, 64).unwrap().unwrap();
        assert_eq!(start, 0);
        assert_eq!(decode(&buffer)[0].cmd, CMD_RESET);
        // The new fragment starts with the time.
        assert_eq!(decode(&h.buffer)[0].cmd, CMD_CURRENT_TIME);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_oldest_fragments_are_trimmed_past_the_maximum() {
        let dir = temp_dir("trim");
        let mut s = Directory::new(dir.join("battery-history"), 100, false);
        for t in [1, 2, 3] {
            s.create(t);
            s.write(t, &[0; 60], false);
        }
        s.write(3, &[0; 60], true);
        assert_eq!(s.fragments().clone(), [3]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_level_int_flags_an_overflowing_voltage() {
        let last = HistoryItem::default();
        let cur = HistoryItem {
            battery_level: 3,
            battery_voltage: 4200,
            ..HistoryItem::default()
        };
        let b = battery_level_int(&cur, &last);
        assert_eq!(b >> BATTERY_LEVEL_LEVEL_SHIFT, 3);
        assert_ne!(b & BATTERY_LEVEL_OVERFLOW_FLAG, 0);
        assert_eq!(extended_battery_level_int(&cur), 4200);
    }
}
