//! The stream volumes AudioService keeps (`AudioService` and its
//! `VolumeStreamState` at `android-16.0.0_r1`), on the one output device
//! the audio HAL has, the speaker. Each stream has its range, its index
//! per device (tenths, as AudioService keeps them) read from the settings
//! AudioService last persisted, and its mute; a stream aliased to another
//! follows it. audioserver gets the ranges and indexes at the start, as
//! `onReinitVolumes` gives them, and each change after. A device of
//! absolute volume (the speaker, once guest-init's volume bridge makes it
//! the Mac's output, `aim_services::volume`) plays at full scale, and its
//! dispatcher gets each change of its streams.
//!
//! Not kept: a change is not persisted (the app host writes no settings,
//! #1474); the ringer and zen modes are read, not changed, so a change
//! that would change them is not implemented (#1475).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::audio_policy::{AudioPolicy, DEVICE_OUT_DEFAULT, DEVICE_OUT_SPEAKER};
use super::settings::Table;
use super::AppHost;
use crate::clip::Raw;

/// `AudioSystem.STREAM_*`.
pub const STREAM_VOICE_CALL: i32 = 0;
pub const STREAM_SYSTEM: i32 = 1;
pub const STREAM_RING: i32 = 2;
pub const STREAM_MUSIC: i32 = 3;
pub const STREAM_ALARM: i32 = 4;
pub const STREAM_NOTIFICATION: i32 = 5;
pub const STREAM_BLUETOOTH_SCO: i32 = 6;
pub const STREAM_SYSTEM_ENFORCED: i32 = 7;
pub const STREAM_DTMF: i32 = 8;
pub const STREAM_TTS: i32 = 9;
pub const STREAM_ACCESSIBILITY: i32 = 10;
pub const STREAM_ASSISTANT: i32 = 11;
const NUM_STREAMS: usize = 12;

/// `AudioService.MAX_STREAM_VOLUME`, `MIN_STREAM_VOLUME`;
/// `AudioSystem.DEFAULT_STREAM_VOLUME`.
const MAX_STREAM_VOLUME: [i32; NUM_STREAMS] = [5, 7, 7, 15, 7, 7, 15, 7, 15, 15, 15, 15];
const MIN_STREAM_VOLUME: [i32; NUM_STREAMS] = [1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0];
const DEFAULT_STREAM_VOLUME: [i32; NUM_STREAMS] = [4, 7, 5, 5, 6, 5, 7, 7, 5, 5, 5, 5];
/// `Settings.System.VOLUME_SETTINGS_INT`: no setting for the enforced
/// system, DTMF and TTS streams.
const VOLUME_SETTINGS_INT: [&str; NUM_STREAMS] = [
    "volume_voice",
    "volume_system",
    "volume_ring",
    "volume_music",
    "volume_alarm",
    "volume_notification",
    "volume_bluetooth_sco",
    "",
    "",
    "",
    "volume_a11y",
    "volume_assistant",
];
/// `AudioService.STREAM_VOLUME_ALIAS_VOICE`, `_DEFAULT`, `_TELEVISION`.
const ALIAS_VOICE: [i32; NUM_STREAMS] = [0, 2, 2, 3, 4, 2, 6, 2, 2, 3, 3, 3];
const ALIAS_DEFAULT: [i32; NUM_STREAMS] = [0, 2, 2, 3, 4, 2, 6, 2, 2, 3, 3, 3];
const ALIAS_TELEVISION: [i32; NUM_STREAMS] = [3, 3, 3, 3, 3, 3, 6, 3, 3, 3, 3, 3];
/// `AudioSystem.DEFAULT_MUTE_STREAMS_AFFECTED`.
const DEFAULT_MUTE_STREAMS_AFFECTED: i32 = (1 << STREAM_MUSIC)
    | (1 << STREAM_RING)
    | (1 << STREAM_NOTIFICATION)
    | (1 << STREAM_SYSTEM)
    | (1 << STREAM_VOICE_CALL)
    | (1 << STREAM_BLUETOOTH_SCO);
/// `AudioManager.ADJUST_*`, `FLAG_*`.
pub const ADJUST_RAISE: i32 = 1;
pub const ADJUST_SAME: i32 = 0;
pub const ADJUST_LOWER: i32 = -1;
pub const ADJUST_MUTE: i32 = -100;
pub const ADJUST_UNMUTE: i32 = 100;
pub const ADJUST_TOGGLE_MUTE: i32 = 101;
const FLAG_ALLOW_RINGER_MODES: i32 = 1 << 1;
const FLAG_FIXED_VOLUME: i32 = 1 << 5;
const FLAG_BLUETOOTH_ABS_VOLUME: i32 = 1 << 6;
const FLAG_ABSOLUTE_VOLUME: i32 = 1 << 13;
/// `AudioManager.RINGER_MODE_NORMAL`, `Settings.Global.ZEN_MODE_OFF`.
const RINGER_MODE_NORMAL: i32 = 2;
const ZEN_MODE_OFF: i32 = 0;
/// The streams the ringer mode mutes unless set
/// (`Settings.System.MODE_RINGER_STREAMS_AFFECTED`'s default).
const DEFAULT_RINGER_STREAMS_AFFECTED: i32 = (1 << STREAM_RING)
    | (1 << STREAM_NOTIFICATION)
    | (1 << STREAM_SYSTEM)
    | (1 << STREAM_SYSTEM_ENFORCED)
    | (1 << STREAM_DTMF);
/// `VolumeInfo.INDEX_NOT_SET`.
const INDEX_NOT_SET: i32 = -100;
/// `IAudioDeviceVolumeDispatcher`'s calls (one-way).
const DISPATCHER_DESCRIPTOR: &str = "android.media.IAudioDeviceVolumeDispatcher";
const DISPATCH_DEVICE_VOLUME_CHANGED: u32 = 1;
const DISPATCH_DEVICE_VOLUME_ADJUSTED: u32 = 2;
/// `AudioDeviceVolumeManager.ADJUST_MODE_NORMAL`.
const ADJUST_MODE_NORMAL: i32 = 0;

/// `rescaleIndex(index, srcMin, srcMax, dstMin, dstMax)`.
fn rescale(index: i32, src_min: i32, src_max: i32, dst_min: i32, dst_max: i32) -> i32 {
    let (src_range, dst_range) = (src_max - src_min, dst_max - dst_min);
    if src_range == 0 {
        return dst_min;
    }
    dst_min + ((index - src_min) * dst_range + src_range / 2) / src_range
}

/// `android.media.VolumeInfo` of a stream.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeInfo {
    pub stream_type: i32,
    pub has_mute_command: bool,
    pub muted: bool,
    pub index: i32,
    pub min: i32,
    pub max: i32,
}

impl ReadParcelable for VolumeInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // A volume group's (no stream type) is not modelled.
        if !r.read_bool()? {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        Ok(VolumeInfo {
            stream_type: r.read_i32()?,
            has_mute_command: r.read_bool()?,
            muted: r.read_bool()?,
            index: r.read_i32()?,
            min: r.read_i32()?,
            max: r.read_i32()?,
        })
    }
}

impl WriteParcelable for VolumeInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_bool(true);
        p.write_i32(self.stream_type);
        p.write_bool(self.has_mute_command);
        p.write_bool(self.muted);
        p.write_i32(self.index);
        p.write_i32(self.min);
        p.write_i32(self.max);
    }
}

/// `android.media.AudioDeviceAttributes`, kept as it arrived: its
/// internal device type (`getInternalType`).
pub struct DeviceAttributes {
    pub internal_type: i32,
    raw: Raw,
}

impl ReadParcelable for DeviceAttributes {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        r.read_i32()?; // role
        r.read_i32()?; // type
        r.read_string16()?; // address
        r.read_string16()?; // name
        let internal_type = r.read_i32()?;
        // Profiles and descriptors (`writeParcelableArray`): none.
        for _ in 0..2 {
            if r.read_i32()? > 0 {
                return Err(aim_binder_host::parcel::BAD_VALUE);
            }
        }
        Ok(DeviceAttributes { internal_type, raw: Raw::capture(r, start) })
    }
}

impl WriteParcelable for DeviceAttributes {
    fn write_to(&self, p: &mut Parcel) {
        self.raw.write(p);
    }
}

/// `AbsoluteVolumeDeviceInfo`.
pub struct Absolute {
    pub device: DeviceAttributes,
    pub volumes: Vec<VolumeInfo>,
    pub dispatcher: Strong,
    pub handles_adjustment: bool,
}

impl Absolute {
    fn matching(&self, stream: i32) -> Option<&VolumeInfo> {
        self.volumes.iter().find(|v| v.stream_type == stream)
    }

    /// `dispatchAbsoluteVolumeChanged` or `…Adjusted`.
    fn dispatch(&self, stream: i32, index: i32, src: &Stream, adjusted: Option<i32>) {
        let Some(info) = self.matching(stream) else { return };
        let index = if info.min == INDEX_NOT_SET || info.max == INDEX_NOT_SET {
            index
        } else {
            rescale(index, src.min, src.max, info.min, info.max)
        };
        let mut p = Parcel::new();
        p.write_interface_token(DISPATCHER_DESCRIPTOR);
        p.write_i32(1);
        self.device.write_to(&mut p);
        p.write_i32(1);
        VolumeInfo { index, ..info.clone() }.write_to(&mut p);
        let code = match adjusted {
            Some(direction) => {
                p.write_i32(direction);
                p.write_i32(ADJUST_MODE_NORMAL);
                DISPATCH_DEVICE_VOLUME_ADJUSTED
            }
            None => DISPATCH_DEVICE_VOLUME_CHANGED,
        };
        if let Err(e) = self.dispatcher.transact(code, &p, true) {
            eprintln!("app host: audio: the absolute volume dispatcher: {e}");
        }
    }
}

/// `VolumeStreamState`.
#[derive(Clone, Debug)]
pub struct Stream {
    stream: i32,
    /// Tenths, as each index.
    min: i32,
    min_no_perm: i32,
    max: i32,
    /// The range of its volume curve in audioserver.
    curve: (i32, i32),
    step_factor: f32,
    setting: &'static str,
    indexes: BTreeMap<i32, i32>,
    pub muted: bool,
}

impl Stream {
    pub fn index(&self, device: i32) -> i32 {
        self.indexes.get(&device).or_else(|| self.indexes.get(&DEVICE_OUT_DEFAULT)).copied().unwrap_or(0)
    }

    pub fn max_index(&self) -> i32 {
        self.max
    }

    pub fn min_index(&self, privileged: bool) -> i32 {
        if privileged { self.min } else { self.min_no_perm }
    }

    /// `getValidIndex`.
    fn valid(&self, index: i32, fixed: bool, has_modify_audio_settings: bool) -> i32 {
        let min = if has_modify_audio_settings { self.min } else { self.min_no_perm };
        if index < min {
            min
        } else if fixed || index > self.max {
            self.max
        } else {
            index
        }
    }
}

/// What AudioService decides the volumes by, from the image and the
/// settings.
pub struct Config<'a> {
    pub voice_capable: bool,
    pub single_volume: bool,
    pub fixed_volume: bool,
    pub assistant_volume: bool,
    /// `replace_stream_bt_sco` and `equal_sco_lea_vc_index_range`.
    pub replace_bt_sco: bool,
    pub equal_sco_lea_vc_range: bool,
    /// `config_audio_alarm_min_vol` with `alarm_min_volume_zero`.
    pub alarm_min: Option<i32>,
    /// `config_audio_notif_vol_steps`, `…_default`; ring's.
    pub notification: (Option<i32>, Option<i32>),
    pub ring: (Option<i32>, Option<i32>),
    pub property: &'a dyn Fn(&str) -> Option<i32>,
    pub setting: &'a dyn Fn(Table, &str) -> Option<i32>,
}

pub struct Volumes {
    pub streams: Vec<Option<Stream>>,
    /// `sStreamVolumeAlias`; -1 for none.
    pub alias: [i32; NUM_STREAMS],
    mute_affected: i32,
    ringer_affected: i32,
    /// Whether the ringer mode is normal and zen mode off: else what they
    /// mute is read, but not changed.
    ringer_normal: bool,
    pub fixed_volume: bool,
    replace_bt_sco: bool,
    pub absolute: Option<Absolute>,
    /// What `setIndex` would broadcast since last taken
    /// (`VOLUME_CHANGED_ACTION` of a stream at its device): stream, alias,
    /// new and old index.
    changed: Vec<(i32, i32, i32, i32)>,
}

impl Volumes {
    /// AudioService's constructor and `readPersistedSettings`: the ranges
    /// and defaults, the aliases, each stream's indexes from its settings.
    /// The policy's volume groups give no range before AudioService sets
    /// them (the HAL has no engine configuration), so the tables, the
    /// properties and the resources do.
    pub fn new(c: &Config<'_>) -> Volumes {
        let mut max = MAX_STREAM_VOLUME;
        let mut min = MIN_STREAM_VOLUME;
        let mut default = DEFAULT_STREAM_VOLUME;
        let prop = |name: &str| (c.property)(name);
        if let Some(v) = prop("ro.config.vc_call_vol_steps") {
            max[0] = v;
        }
        default[0] = prop("ro.config.vc_call_vol_default").filter(|&d| d <= max[0] && d >= min[0]).unwrap_or(max[0] * 3 / 4);
        if let Some(v) = prop("ro.config.media_vol_steps") {
            max[3] = v;
        }
        let television = false;
        default[3] = prop("ro.config.media_vol_default")
            .filter(|&d| d <= max[3] && d >= min[3])
            .unwrap_or(if television { max[3] / 4 } else { max[3] / 3 });
        if let Some(v) = prop("ro.config.alarm_vol_steps") {
            max[4] = v;
        }
        if let Some(v) = c.alarm_min.filter(|&m| m <= max[4]) {
            min[4] = v;
        }
        default[4] = prop("ro.config.alarm_vol_default").filter(|&d| d <= max[4]).unwrap_or(6 * max[4] / 7);
        if let Some(v) = prop("ro.config.system_vol_steps") {
            max[1] = v;
        }
        default[1] = prop("ro.config.system_vol_default").filter(|&d| d <= max[1]).unwrap_or(max[1]);
        if let Some(v) = prop("ro.config.assistant_vol_min") {
            min[11] = v;
        }
        for (s, (steps, def)) in [(STREAM_NOTIFICATION, c.notification), (STREAM_RING, c.ring)] {
            let s = s as usize;
            if let Some(steps) = steps.filter(|&v| v > 0) {
                max[s] = steps;
            }
            if let Some(def) = def.filter(|&d| d <= max[s] && d >= min[s]) {
                default[s] = def;
            }
        }

        // `updateStreamVolumeAlias`.
        let mut alias = if c.single_volume {
            ALIAS_TELEVISION
        } else if c.voice_capable {
            ALIAS_VOICE
        } else {
            ALIAS_DEFAULT
        };
        if !c.single_volume {
            // `mNotifAliasRing` is off: notifications have their own volume.
            alias[STREAM_NOTIFICATION as usize] = STREAM_NOTIFICATION;
            alias[STREAM_DTMF as usize] = if c.voice_capable { STREAM_RING } else { STREAM_MUSIC };
        } else {
            alias[STREAM_DTMF as usize] = STREAM_MUSIC;
        }
        alias[STREAM_ACCESSIBILITY as usize] = STREAM_MUSIC;
        alias[STREAM_ASSISTANT as usize] = if c.assistant_volume { STREAM_ASSISTANT } else { STREAM_MUSIC };
        if c.replace_bt_sco {
            alias[STREAM_BLUETOOTH_SCO as usize] = -1;
        }

        let mut streams: Vec<Option<Stream>> = (0..NUM_STREAMS)
            .map(|s| {
                let a = alias[s];
                (a >= 0).then(|| Stream {
                    stream: s as i32,
                    min: min[s] * 10,
                    min_no_perm: min[s] * 10,
                    max: max[s] * 10,
                    curve: (min[s], max[s]),
                    step_factor: 1.0,
                    setting: VOLUME_SETTINGS_INT[a as usize],
                    indexes: BTreeMap::new(),
                    muted: false,
                })
            })
            .collect();
        // `updateIndexFactors`: the voice call's range is SCO's, mapped
        // onto its curve.
        if (c.replace_bt_sco || c.equal_sco_lea_vc_range)
            && let Some(v) = streams[0].as_mut()
        {
            if max[6] > max[0] {
                v.max = max[6] * 10;
            }
            v.min = min[0] * 10;
            v.min_no_perm = v.min;
            v.step_factor = (v.max - v.min) as f32 / (max[0] * 10 - min[0] * 10) as f32;
        }
        // `readSettings`.
        for s in streams.iter_mut().flatten() {
            if s.stream == STREAM_SYSTEM || s.stream == STREAM_SYSTEM_ENFORCED {
                s.indexes.insert(DEVICE_OUT_DEFAULT, 10 * default[s.stream as usize]);
                continue;
            }
            for device in [DEVICE_OUT_SPEAKER, DEVICE_OUT_DEFAULT] {
                let fallback = (device == DEVICE_OUT_DEFAULT).then_some(default[s.stream as usize]);
                let index = if s.setting.is_empty() {
                    fallback
                } else {
                    let name = if device == DEVICE_OUT_SPEAKER { format!("{}_speaker", s.setting) } else { s.setting.to_string() };
                    (c.setting)(Table::System, &name).or(fallback)
                };
                if let Some(index) = index.filter(|&i| i != -1) {
                    let valid = s.valid(10 * index, c.fixed_volume, true);
                    s.indexes.insert(device, valid);
                }
            }
        }
        let mut mute_affected = (c.setting)(Table::System, "mute_streams_affected").unwrap_or(DEFAULT_MUTE_STREAMS_AFFECTED);
        // `checkMuteAffectedStreams`: a stream that cannot go to 0 is not
        // mutable, but for calls.
        for s in streams.iter().flatten() {
            if s.min > 0 && s.stream != STREAM_VOICE_CALL && s.stream != STREAM_BLUETOOTH_SCO {
                mute_affected &= !(1 << s.stream);
            }
        }
        let ringer_affected = (c.setting)(Table::System, "mode_ringer_streams_affected").unwrap_or(DEFAULT_RINGER_STREAMS_AFFECTED);
        let ringer = (c.setting)(Table::Global, "mode_ringer").unwrap_or(RINGER_MODE_NORMAL);
        let zen = (c.setting)(Table::Global, "zen_mode").unwrap_or(ZEN_MODE_OFF);
        let mut v = Volumes {
            streams,
            alias,
            mute_affected,
            ringer_affected,
            ringer_normal: ringer == RINGER_MODE_NORMAL && zen == ZEN_MODE_OFF,
            fixed_volume: c.fixed_volume,
            replace_bt_sco: c.replace_bt_sco,
            absolute: None,
            changed: Vec::new(),
        };
        // `checkAllAliasStreamVolumes`.
        for s in 0..NUM_STREAMS {
            let a = v.alias[s];
            if a >= 0 && a as usize != s {
                v.set_all_indexes(s, a as usize);
            }
        }
        // `setRingerModeInt` of a silent or vibrating ringer mutes the
        // streams it affects.
        if ringer != RINGER_MODE_NORMAL {
            for s in v.streams.iter_mut().flatten() {
                if ringer_affected & (1 << s.stream) != 0 {
                    s.muted = true;
                }
            }
        }
        v
    }

    /// `replaceBtScoStreamWithVoiceCall`, `ensureValidStreamType`.
    pub fn valid_stream(&self, stream: i32) -> Result<usize, Exception> {
        let stream = if self.replace_bt_sco && stream == STREAM_BLUETOOTH_SCO { STREAM_VOICE_CALL } else { stream };
        if !(0..NUM_STREAMS as i32).contains(&stream) {
            return Err(Exception::illegal_argument(format!("Bad stream type {stream}")));
        }
        Ok(stream as usize)
    }

    /// `getVssForStreamOrDefault`.
    pub fn state(&self, stream: usize) -> Result<&Stream, Exception> {
        self.streams[stream].as_ref().ok_or_else(|| Exception::illegal_argument(format!("No VolumeStreamState for stream {stream}")))
    }

    fn rescale_streams(&self, index: i32, src: usize, dst: usize) -> i32 {
        let (s, d) = (self.streams[src].as_ref().unwrap(), self.streams[dst].as_ref().unwrap());
        rescale(index, s.min, s.max, d.min, d.max)
    }

    /// `setAllIndexes`.
    fn set_all_indexes(&mut self, dst: usize, src: usize) {
        let default = self.rescale_streams(self.streams[src].as_ref().unwrap().index(DEVICE_OUT_DEFAULT), src, dst);
        let d = self.streams[dst].as_mut().unwrap();
        for index in d.indexes.values_mut() {
            *index = default;
        }
        let scaled: Vec<(i32, i32)> = self.streams[src]
            .as_ref()
            .unwrap()
            .indexes
            .iter()
            .map(|(dv, i)| (*dv, self.rescale_streams(*i, src, dst)))
            .collect();
        for (device, index) in scaled {
            self.set_index(dst, index, device, true);
        }
    }

    /// `VolumeStreamState.setIndex`: whether it changed; the streams
    /// aliased to it follow.
    pub fn set_index(&mut self, stream: usize, index: i32, device: i32, has_modify_audio_settings: bool) -> bool {
        let fixed = self.fixed_volume;
        let s = self.streams[stream].as_mut().unwrap();
        let old = s.index(device);
        let index = s.valid(index, fixed, has_modify_audio_settings);
        s.indexes.insert(device, index);
        let changed = old != index;
        let (old, new) = ((old + 5) / 10, (index + 5) / 10);
        if old != new && device == device_for_stream(stream) {
            self.changed.push((stream as i32, self.alias[stream], new, old));
        }
        for other in 0..NUM_STREAMS {
            if other != stream
                && self.alias[other] == stream as i32
                && self.streams[other].is_some()
                && (changed || !self.streams[other].as_ref().unwrap().indexes.contains_key(&device))
            {
                let scaled = self.rescale_streams(index, stream, other);
                self.set_index(other, scaled, device, has_modify_audio_settings);
            }
        }
        changed
    }

    /// `isAbsoluteVolumeDevice`.
    pub fn is_absolute(&self, device: i32) -> bool {
        self.absolute.as_ref().is_some_and(|a| a.device.internal_type == device)
    }

    /// `VolumeStreamState.setStreamVolumeIndex`.
    fn send_index(&self, policy: &AudioPolicy, s: &Stream, index: i32, device: i32) -> Result<(), String> {
        let index = if self.replace_bt_sco && index != 0 {
            (s.min as f32 + (index * 10 - s.min) as f32 / s.step_factor + 5.0) as i32 / 10
        } else {
            index
        };
        policy.set_stream_volume_index(s.stream, device, index, false)
    }

    /// `applyDeviceVolume_syncVSS` (`device`) or `applyAllVolumes` (none).
    /// Nothing is sent before audioserver is connected: [`Volumes::init`]
    /// sends all then.
    pub fn apply(&self, policy: Option<&AudioPolicy>, stream: usize, device: Option<i32>) -> Result<(), String> {
        let Some(policy) = policy else { return Ok(()) };
        let s = self.streams[stream].as_ref().unwrap();
        let level = |device: i32| {
            if s.muted {
                0
            } else if self.is_absolute(device) {
                (s.max + 5) / 10
            } else {
                (s.index(device) + 5) / 10
            }
        };
        match device {
            Some(device) => self.send_index(policy, s, level(device), device),
            None => {
                for &device in s.indexes.keys().filter(|&&d| d != DEVICE_OUT_DEFAULT) {
                    self.send_index(policy, s, level(device), device)?;
                }
                let default = if s.muted { 0 } else { (s.index(DEVICE_OUT_DEFAULT) + 5) / 10 };
                self.send_index(policy, s, default, DEVICE_OUT_DEFAULT)
            }
        }
    }

    /// `onReinitVolumes`: each stream's curve, then its volumes, the last
    /// stream first; the device of absolute volume's.
    pub fn init(&self, policy: &AudioPolicy) -> Result<(), String> {
        if let Some(a) = &self.absolute {
            policy.set_device_absolute_volume_enabled(a.device.internal_type, true, STREAM_MUSIC)?;
        }
        for stream in (0..NUM_STREAMS).rev() {
            let Some(s) = &self.streams[stream] else { continue };
            policy.init_stream_volume(s.stream, s.curve.0, s.curve.1)?;
            self.apply(Some(policy), stream, None)?;
        }
        Ok(())
    }

    pub fn affected_by_mute(&self, stream: usize) -> bool {
        self.mute_affected & (1 << stream) != 0
    }

    /// `VolumeStreamState.isMutable`.
    fn mutable(&self, stream: usize) -> bool {
        let s = self.streams[stream].as_ref().unwrap();
        self.affected_by_mute(stream) && (s.min == 0 || stream as i32 == STREAM_VOICE_CALL)
    }

    /// `muteAliasStreams`: the streams of the alias whose mute changed.
    fn mute_alias_streams(&mut self, alias: usize, muted: bool) -> Vec<usize> {
        let mut changed = Vec::new();
        for s in 0..NUM_STREAMS {
            if self.streams[s].is_some() && self.alias[s] == alias as i32 && self.mutable(s) {
                let st = self.streams[s].as_mut().unwrap();
                if st.muted != muted {
                    st.muted = muted;
                    changed.push(s);
                }
            }
        }
        changed
    }

    /// Whether a change of `alias`'s volume may change the ringer mode or
    /// is decided by zen mode, which the app host does not model.
    fn ringer_involved(&self, alias: usize, flags: i32) -> bool {
        let ui = alias as i32 == self.alias[STREAM_SYSTEM as usize];
        flags & FLAG_ALLOW_RINGER_MODES != 0 || ui || !self.ringer_normal && self.ringer_affected & (1 << alias) != 0
    }
}

/// A volume change's effects outside the state, done once its lock is
/// released: broadcasts.
#[derive(Default)]
pub struct Effects {
    /// `VOLUME_CHANGED_ACTION`: stream, alias, new and old index.
    pub volume_changed: Vec<(i32, i32, i32, i32)>,
    /// `STREAM_MUTE_CHANGED_ACTION`: stream and mute.
    pub mute_changed: Vec<(i32, bool)>,
}

impl Volumes {
    /// The broadcasts of the changes since last taken.
    pub fn take_changes(&mut self, effects: &mut Effects) {
        effects.volume_changed.append(&mut self.changed);
    }
}

/// The device a stream plays on (`getDeviceForStream`): the HAL's one
/// output.
pub fn device_for_stream(_stream: usize) -> i32 {
    DEVICE_OUT_SPEAKER
}

impl Volumes {
    /// `setStreamVolume` past the caller's checks: `index` in the
    /// stream's steps.
    #[allow(clippy::too_many_arguments)]
    pub fn set_stream_volume(
        &mut self,
        host: &AppHost,
        policy: Option<&AudioPolicy>,
        stream: usize,
        index: i32,
        mut flags: i32,
        has_modify_audio_settings: bool,
        effects: &mut Effects,
    ) -> Result<(), Exception> {
        if self.fixed_volume {
            return Ok(());
        }
        let alias = self.alias[stream];
        if alias < 0 {
            return Ok(());
        }
        let alias = alias as usize;
        let device = device_for_stream(stream);
        if flags & FLAG_BLUETOOTH_ABS_VOLUME != 0 {
            return Ok(());
        }
        // `getNewRingerMode`: the UI sounds stream (or a change allowing
        // ringer modes) to 0 changes the ringer mode, as does any change of
        // it while the ringer mode is not normal.
        if self.ringer_involved(alias, flags) && (index == 0 || !self.ringer_normal) {
            return Err(host.unimplemented_named("audio", "a volume change of the ringer mode or zen mode (#1475)"));
        }
        let scaled = self.rescale_streams(index * 10, stream, alias);
        flags &= !FLAG_FIXED_VOLUME;
        // `onSetStreamVolume`: the index, then a volume of 0 mutes and
        // another unmutes.
        if self.set_index(alias, scaled, device, has_modify_audio_settings) {
            self.apply_alias(policy, alias, device);
        }
        for s in self.mute_alias_streams(alias, scaled == 0) {
            self.apply_stream(policy, s);
            effects.mute_changed.push((s as i32, scaled == 0));
        }
        let new = self.state(stream)?.index(device);
        if self.is_absolute(device) && flags & FLAG_ABSOLUTE_VOLUME == 0 {
            let src = self.state(stream)?.clone();
            self.absolute.as_ref().unwrap().dispatch(stream as i32, new, &src, None);
        }
        Ok(())
    }

    /// `adjustStreamVolume` past the caller's checks.
    #[allow(clippy::too_many_arguments)]
    pub fn adjust_stream_volume(
        &mut self,
        host: &AppHost,
        policy: Option<&AudioPolicy>,
        stream: usize,
        direction: i32,
        mut flags: i32,
        has_modify_audio_settings: bool,
        effects: &mut Effects,
    ) -> Result<(), Exception> {
        if self.fixed_volume {
            return Ok(());
        }
        let mute_adjust = matches!(direction, ADJUST_MUTE | ADJUST_UNMUTE | ADJUST_TOGGLE_MUTE);
        if mute_adjust && !self.affected_by_mute(stream) {
            return Ok(());
        }
        let alias = self.alias[stream];
        if alias < 0 {
            return Ok(());
        }
        let alias = alias as usize;
        let device = device_for_stream(alias);
        if flags & FLAG_BLUETOOTH_ABS_VOLUME != 0 {
            return Ok(());
        }
        if self.ringer_involved(alias, flags) {
            return Err(host.unimplemented_named("audio", "a volume adjustment of the ringer mode or zen mode (#1475)"));
        }
        flags &= !FLAG_FIXED_VOLUME;
        let step = {
            let (s, a) = (self.state(stream)?, self.state(alias)?);
            let step = (10.0 * a.step_factor) as i32;
            // `rescaleStep`.
            let (src, dst) = (s.max - s.min, a.max - a.min);
            if src == 0 { 0 } else { (step * dst + src / 2) / src }
        };
        let old = self.state(stream)?.index(device);
        if self.is_absolute(device) && flags & FLAG_ABSOLUTE_VOLUME == 0 {
            let a = self.absolute.as_ref().unwrap();
            if a.handles_adjustment {
                let src = self.state(stream)?.clone();
                a.dispatch(stream as i32, old, &src, Some(direction));
                return Ok(());
            }
        }
        if direction != ADJUST_SAME {
            if mute_adjust {
                let muted = if direction == ADJUST_TOGGLE_MUTE { !self.state(alias)?.muted } else { direction == ADJUST_MUTE };
                for s in self.mute_alias_streams(alias, muted) {
                    self.apply_stream(policy, s);
                    effects.mute_changed.push((s as i32, muted));
                }
            } else {
                let current = self.state(alias)?.index(device);
                let was_muted = self.state(alias)?.muted;
                if self.set_index(alias, current + direction * step, device, has_modify_audio_settings) || was_muted {
                    if was_muted && direction == ADJUST_RAISE {
                        for s in self.mute_alias_streams(alias, false) {
                            effects.mute_changed.push((s as i32, false));
                        }
                    }
                    self.apply_alias(policy, alias, device);
                }
            }
            let new = self.state(stream)?.index(device);
            if self.is_absolute(device) && flags & FLAG_ABSOLUTE_VOLUME == 0 {
                let src = self.state(stream)?.clone();
                self.absolute.as_ref().unwrap().dispatch(stream as i32, new, &src, None);
            }
        }
        Ok(())
    }

    /// `MSG_SET_DEVICE_VOLUME` of `alias`: the streams aliased to it too.
    fn apply_alias(&self, policy: Option<&AudioPolicy>, alias: usize, device: i32) {
        for s in 0..NUM_STREAMS {
            if self.streams[s].is_some()
                && self.alias[s] == alias as i32
                && let Err(e) = self.apply(policy, s, Some(device))
            {
                eprintln!("app host: audio: {e}");
            }
        }
    }

    /// `doMute`: a stream's volumes after its mute changed.
    fn apply_stream(&self, policy: Option<&AudioPolicy>, stream: usize) {
        if let Err(e) = self.apply(policy, stream, None) {
            eprintln!("app host: audio: {e}");
        }
    }

    /// `registerDeviceVolumeDispatcherForAbsoluteVolume` (register): the
    /// device's volumes as the controller has them.
    pub fn register_absolute(&mut self, policy: Option<&AudioPolicy>, absolute: Absolute) {
        let device = absolute.device.internal_type;
        let volumes = absolute.volumes.clone();
        self.absolute = Some(absolute);
        for (stream, info) in volumes.iter().filter(|v| v.index != INDEX_NOT_SET && v.min != INDEX_NOT_SET && v.max != INDEX_NOT_SET).map(|v| (v.stream_type, v)) {
            let Ok(stream) = self.valid_stream(stream) else { continue };
            if self.streams[stream].is_none() {
                continue;
            }
            let s = self.streams[stream].as_ref().unwrap();
            let index = rescale(info.index, info.min, info.max, s.min, s.max);
            self.set_index(stream, index, device, true);
            self.apply_alias(policy, self.alias[stream].max(0) as usize, device);
        }
        // `setDeviceAbsoluteVolumeEnabled` for the stream driving it.
        if let Some(Err(e)) = policy.map(|p| p.set_device_absolute_volume_enabled(device, true, STREAM_MUSIC)) {
            eprintln!("app host: audio: {e}");
        }
        for s in 0..NUM_STREAMS {
            if self.streams[s].is_some() {
                self.apply_stream(policy, s);
            }
        }
    }

    /// The registration goes (unregister, or its dispatcher died).
    pub fn unregister_absolute(&mut self, policy: Option<&AudioPolicy>) {
        if let Some(a) = self.absolute.take() {
            if let Some(Err(e)) = policy.map(|p| p.set_device_absolute_volume_enabled(a.device.internal_type, false, STREAM_MUSIC)) {
                eprintln!("app host: audio: {e}");
            }
            for s in 0..NUM_STREAMS {
                if self.streams[s].is_some() {
                    self.apply_stream(policy, s);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volumes(setting: &dyn Fn(Table, &str) -> Option<i32>) -> Volumes {
        let none = |_: &str| None;
        Volumes::new(&Config {
            voice_capable: true,
            single_volume: false,
            fixed_volume: false,
            assistant_volume: false,
            replace_bt_sco: true,
            equal_sco_lea_vc_range: true,
            alarm_min: Some(0),
            notification: (None, None),
            ring: (None, None),
            property: &none,
            setting,
        })
    }

    #[test]
    fn defaults_follow_audio_service() {
        let v = volumes(&|_, _| None);
        let music = v.state(STREAM_MUSIC as usize).unwrap();
        // 15 steps, a third of them by default.
        assert_eq!((music.max_index(), music.index(DEVICE_OUT_SPEAKER)), (150, 50));
        // The voice call takes SCO's 15 steps over its own curve of 5.
        let voice = v.state(STREAM_VOICE_CALL as usize).unwrap();
        assert_eq!((voice.min_index(true), voice.max_index(), voice.curve), (10, 150, (1, 5)));
        // The alarm may go to 0; notifications have their own volume.
        assert_eq!(v.state(STREAM_ALARM as usize).unwrap().min_index(true), 0);
        assert_eq!(v.alias[STREAM_NOTIFICATION as usize], STREAM_NOTIFICATION);
        assert_eq!(v.alias[STREAM_SYSTEM as usize], STREAM_RING);
        assert_eq!(v.alias[STREAM_BLUETOOTH_SCO as usize], -1);
        assert_eq!(v.valid_stream(STREAM_BLUETOOTH_SCO).unwrap(), STREAM_VOICE_CALL as usize);
        assert!(v.valid_stream(12).is_err());
    }

    #[test]
    fn settings_and_aliases() {
        let v = volumes(&|t, n| match (t, n) {
            (Table::System, "volume_music_speaker") => Some(9),
            (Table::System, "volume_ring_speaker") => Some(3),
            _ => None,
        });
        assert_eq!(v.state(STREAM_MUSIC as usize).unwrap().index(DEVICE_OUT_SPEAKER), 90);
        // TTS follows music, the system stream ring, rescaled.
        assert_eq!(v.state(STREAM_TTS as usize).unwrap().index(DEVICE_OUT_SPEAKER), 90);
        assert_eq!(v.state(STREAM_SYSTEM as usize).unwrap().index(DEVICE_OUT_SPEAKER), 30);
    }

    #[test]
    fn a_change_reaches_its_aliases_and_is_broadcast() {
        let mut v = volumes(&|_, _| None);
        v.changed.clear();
        assert!(v.set_index(STREAM_MUSIC as usize, 120, DEVICE_OUT_SPEAKER, true));
        // Accessibility's range starts at 1: 10 + 120 × 140/150.
        assert_eq!(v.state(STREAM_ACCESSIBILITY as usize).unwrap().index(DEVICE_OUT_SPEAKER), 122);
        let mut e = Effects::default();
        v.take_changes(&mut e);
        assert!(e.volume_changed.contains(&(STREAM_MUSIC, STREAM_MUSIC, 12, 5)));
        // Out of range is clamped.
        v.set_index(STREAM_MUSIC as usize, 400, DEVICE_OUT_SPEAKER, true);
        assert_eq!(v.state(STREAM_MUSIC as usize).unwrap().index(DEVICE_OUT_SPEAKER), 150);
    }

    #[test]
    fn a_silent_ringer_mutes_what_it_affects() {
        let v = volumes(&|t, n| (t == Table::Global && n == "mode_ringer").then_some(0));
        assert!(v.state(STREAM_RING as usize).unwrap().muted);
        assert!(!v.state(STREAM_MUSIC as usize).unwrap().muted);
    }

    #[test]
    fn rescale_rounds_to_nearest() {
        assert_eq!(rescale(50, 0, 150, 0, 70), 23);
        assert_eq!(rescale(5, 0, 0, 3, 9), 3);
    }
}
