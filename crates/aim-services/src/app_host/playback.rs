//! The players and recorders AudioService monitors
//! (`PlaybackActivityMonitor`, `AudioPlaybackConfiguration`,
//! `RecordingActivityMonitor` at `android-16.0.0_r1`): each player an app
//! makes (an AudioTrack, a MediaPlayer, a SoundPool) registers with its id
//! card and gets an id from AudioFlinger; its events, attributes, session
//! and the events of its port in audioserver keep its configuration, which
//! the playback callbacks get (anonymized but for MODIFY_AUDIO_ROUTING's
//! holders). A player or recorder goes when released or when its process
//! dies. Recording configurations, which audioserver's policy client
//! reports, are not kept (#1480).
//!
//! It enforces audio focus on the players (`PlayerFocusEnforcer`): the
//! players of an app that loses focus to a duck are ducked
//! (`DuckingManager`), those of one that loses it for good are faded out
//! first (`FadeOutManager`, with the default `FadeManagerConfiguration`
//! that `enable_fade_manager_configuration` selects: media and games, but
//! not speech, AAudio or SoundPool players, over 2 s, faded back in over
//! 1 s), and media and game players are muted during a call; each through
//! the player's `IPlayer` (a volume shaper, or its volume), and each
//! player that starts while its app is ducked or faded is too.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::android_media_iplayer as iplayer;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use crate::bundle::{self, Value};
use crate::clip::Raw;

/// `AudioPlaybackConfiguration.PLAYER_*`.
pub const PLAYER_PIID_INVALID: i32 = -1;
const PLAYER_UPID_INVALID: i32 = -1;
const PLAYER_TYPE_UNKNOWN: i32 = -1;
const PLAYER_TYPE_JAM_SOUNDPOOL: i32 = 3;
const PLAYER_TYPE_AAUDIO: i32 = 13;
const PLAYER_STATE_RELEASED: i32 = 0;
const PLAYER_STATE_IDLE: i32 = 1;
const PLAYER_STATE_STARTED: i32 = 2;
const PLAYER_UPDATE_DEVICE_ID: i32 = 5;
const PLAYER_UPDATE_PORT_ID: i32 = 6;
const PLAYER_UPDATE_FORMAT: i32 = 8;
/// `MUTED_BY_CLIENT_VOLUME`, `MUTED_BY_VOLUME_SHAPER`,
/// `MUTED_BY_OP_PLAY_AUDIO`: what makes a started player inactive.
const MUTE_AFFECTING_ACTIVE: i32 = (1 << 4) | (1 << 5) | (1 << 3);
/// `AudioSystem.AUDIO_SESSION_ALLOCATE`.
const AUDIO_SESSION_ALLOCATE: i32 = 0;
/// `AudioManager.RECORD_RIID_INVALID`, `RECORDER_STATE_STARTED`.
pub const RECORD_RIID_INVALID: i32 = -1;
const RECORDER_STATE_STARTED: i32 = 0;
/// `PlayMonitorClient.MAX_ERRORS`.
const MAX_ERRORS: u32 = 5;
const DISPATCHER_DESCRIPTOR: &str = "android.media.IPlaybackConfigDispatcher";
const DISPATCH_PLAYBACK_CONFIG_CHANGE: u32 = 1;
/// The format extras of a port event (`AudioPlaybackConfiguration
/// .EXTRA_PLAYER_EVENT_*`).
const EXTRA_SPATIALIZED: &str = "android.media.extra.PLAYER_EVENT_SPATIALIZED";
const EXTRA_CHANNEL_MASK: &str = "android.media.extra.PLAYER_EVENT_CHANNEL_MASK";
const EXTRA_SAMPLE_RATE: &str = "android.media.extra.PLAYER_EVENT_SAMPLE_RATE";
/// `AudioAttributes` constants.
pub const USAGE_MEDIA: i32 = 1;
const USAGE_NOTIFICATION: i32 = 5;
const USAGE_GAME: i32 = 14;
const CONTENT_TYPE_SPEECH: i32 = 1;
const USAGE_CALL_ASSISTANT: i32 = 17;
const SYSTEM_USAGE_OFFSET: i32 = 1000;
const FLAG_AUDIBILITY_ENFORCED: i32 = 1;
const FLAG_HW_AV_SYNC: i32 = 1 << 4;
const FLAG_LOW_LATENCY: i32 = 1 << 8;
const FLAG_NO_MEDIA_PROJECTION: i32 = 1 << 10;
const FLAG_NO_SYSTEM_CAPTURE: i32 = 1 << 12;
const FLAG_ALL_PUBLIC: i32 = FLAG_AUDIBILITY_ENFORCED | FLAG_HW_AV_SYNC | FLAG_LOW_LATENCY;
/// `FLAG_ALL_API_SET`: the public flags, `FLAG_BYPASS_INTERRUPTION_POLICY`
/// and `FLAG_BYPASS_MUTE`.
const FLAG_ALL_API_SET: i32 = FLAG_ALL_PUBLIC | 1 << 6 | 1 << 7;
const FLATTEN_TAGS: i32 = 1;
const ATTR_PARCEL_IS_NULL_BUNDLE: i32 = -1977;
const ATTR_PARCEL_IS_VALID_BUNDLE: i32 = 1980;
/// `MediaRecorder.AudioSource.AUDIO_SOURCE_INVALID`.
const AUDIO_SOURCE_INVALID: i32 = -1;
/// `AudioFocusRequest.KEY_ACCESSIBILITY_FORCE_FOCUS_DUCKING`.
const KEY_FORCE_DUCKING: &str = "a11y_force_ducking";
/// `AudioManager.AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS`.
pub const AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS: i32 = 1 << 1;
/// `PlaybackActivityMonitor.UNDUCKABLE_PLAYER_TYPES`, which are also
/// `FadeManagerConfiguration`'s default unfadeable ones.
const UNDUCKABLE_PLAYER_TYPES: [i32; 2] = [PLAYER_TYPE_AAUDIO, PLAYER_TYPE_JAM_SOUNDPOOL];
/// `MediaFocusControl.USAGES_TO_MUTE_IN_RING_OR_CALL`.
const USAGES_TO_MUTE_IN_RING_OR_CALL: [i32; 2] = [USAGE_MEDIA, USAGE_GAME];
/// `FadeManagerConfiguration.Builder.DEFAULT_DELAY_FADE_IN_OFFENDERS_MS`.
pub const FADE_IN_DELAY_FOR_OFFENDERS_MS: u64 = 2000;
/// `FadeManagerConfiguration.DEFAULT_FADE_OUT_DURATION_MS`.
const FADE_OUT_DURATION_MS: u64 = 2000;

/// `VolumeShaper.Configuration`s the monitor applies, as `toParcelable`
/// writes them: an id alone, or a scale with its duration and curve
/// (times and volumes), clock timed and cubic, as their builders make
/// them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shaper {
    id: i32,
    scale: Option<(f64, &'static [(f32, f32)])>,
}

/// `PlaybackActivityMonitor.DUCK_VSHAPE`: to -14 dB over the focus ramp
/// time of a notification (`getFocusRampTimeMs`).
const DUCK: Shaper = Shaper { id: 1, scale: Some((500.0, &[(0.0, 1.0), (1.0, 0.2)])) };
/// `STRONG_DUCK_VSHAPE`: to -35 dB.
const STRONG_DUCK: Shaper = Shaper { id: 4, scale: Some((500.0, &[(0.0, 1.0), (1.0, 0.017783)])) };
/// The default `FadeManagerConfiguration`'s fade out and in of a fadeable
/// usage (`createVolShaperConfigForDuration`, `VOLUME_SHAPER_SYSTEM_FADE_ID`).
const FADE_OUT: Shaper = Shaper { id: 2, scale: Some((FADE_OUT_DURATION_MS as f64, &[(0.0, 1.0), (0.25, 0.65), (1.0, 0.0)])) };
const FADE_IN: Shaper = Shaper { id: 2, scale: Some((1000.0, &[(0.0, 0.0), (0.5, 0.3), (1.0, 1.0)])) };

impl WriteParcelable for Shaper {
    /// `VolumeShaperConfiguration`, and its `InterpolatorConfig`, as
    /// structured parcelables: each its size first.
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.scale.is_some() as i32); // `ID` or `SCALE`
        p.write_i32(self.id);
        match self.scale {
            None => {
                p.write_i32(0);
                p.write_i64(0f64.to_bits() as i64);
                p.write_i32(0);
            }
            Some((duration, curve)) => {
                p.write_i32(1 << 1); // `CLOCK_TIME`
                p.write_i64(duration.to_bits() as i64);
                p.write_i32(1);
                let interpolator = p.position();
                p.write_i32(0);
                p.write_i32(2); // `CUBIC`
                p.write_f32(0.0);
                p.write_f32(0.0);
                p.write_i32(curve.len() as i32 * 2);
                for (time, volume) in curve {
                    p.write_f32(*time);
                    p.write_f32(*volume);
                }
                p.set_i32_at(interpolator, (p.position() - interpolator) as i32);
            }
        }
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// A `VolumeShaper.Operation` the monitor applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Operation {
    /// `PLAY_CREATE_IF_NEEDED`, or `PLAY_SKIP_RAMP` to its end at once.
    Play { skip_ramp: bool },
    /// `Operation.REVERSE`.
    Reverse,
    /// `replace(id, true)`: joins the shaper of `id`.
    Join(i32),
}

impl WriteParcelable for Operation {
    /// `VolumeShaperOperation`: its flags as `flagsToAidl` sets them.
    fn write_to(&self, p: &mut Parcel) {
        let (flags, replace, offset) = match *self {
            Operation::Play { skip_ramp } => (1 << 4, -1, if skip_ramp { 1.0 } else { f32::NAN }),
            Operation::Reverse => (1 << 0, -1, f32::NAN),
            Operation::Join(id) => (1 << 2, id, f32::NAN),
        };
        let start = p.position();
        p.write_i32(0);
        p.write_i32(flags);
        p.write_i32(replace);
        p.write_f32(offset);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// An `IPlayer` call (one-way), as `PlayerProxy` makes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerCall {
    Shape(Shaper, Operation),
    Volume(f32),
}

/// `FadeConfigurations.isFadeable` of the default configuration.
fn fadeable(attributes: &Attributes, player_type: i32) -> bool {
    !UNDUCKABLE_PLAYER_TYPES.contains(&player_type)
        && attributes.content_type != CONTENT_TYPE_SPEECH
        && [USAGE_MEDIA, USAGE_GAME].contains(&attributes.usage)
}

/// `getFadeOutDurationOnFocusLossMillis` of a focus request's attributes.
pub fn fade_out_duration_ms(attributes: &Attributes) -> u64 {
    if fadeable(attributes, PLAYER_TYPE_UNKNOWN) { FADE_OUT_DURATION_MS } else { 0 }
}

/// `getFadeOutVolumeShaperConfig` and `getFadeInVolumeShaperConfig`: the
/// fadeable usages' shapers.
fn fade_shapers(attributes: &Attributes) -> Option<(Shaper, Shaper)> {
    [USAGE_MEDIA, USAGE_GAME].contains(&attributes.usage).then_some((FADE_OUT, FADE_IN))
}

/// `AudioAttributes.isSystemUsage`.
pub fn is_system_usage(usage: i32) -> bool {
    usage == USAGE_CALL_ASSISTANT || (SYSTEM_USAGE_OFFSET..=SYSTEM_USAGE_OFFSET + 4).contains(&usage)
}

/// `android.media.AudioAttributes`, kept as written.
#[derive(Clone)]
pub struct Attributes {
    /// `getSystemUsage`.
    pub usage: i32,
    content_type: i32,
    flags: i32,
    /// Its bundle's `KEY_ACCESSIBILITY_FORCE_FOCUS_DUCKING`.
    force_ducking: bool,
    raw: Raw,
}

impl ReadParcelable for Attributes {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let usage = r.read_i32()?;
        let content_type = r.read_i32()?;
        r.read_i32()?; // source
        let flags = r.read_i32()?;
        if r.read_i32()? & FLATTEN_TAGS == FLATTEN_TAGS {
            r.read_string16()?;
        } else {
            for _ in 0..r.read_i32()?.max(0) {
                r.read_string16()?;
            }
        }
        let force_ducking = match r.read_i32()? {
            ATTR_PARCEL_IS_NULL_BUNDLE => false,
            ATTR_PARCEL_IS_VALID_BUNDLE => bundle::read(r)?.is_some_and(|b| b.get(KEY_FORCE_DUCKING) == Some(&Value::Bool(true))),
            _ => return Err(BAD_VALUE),
        };
        Ok(Attributes { usage, content_type, flags, force_ducking, raw: Raw::capture(r, start) })
    }
}

impl WriteParcelable for Attributes {
    fn write_to(&self, p: &mut Parcel) {
        self.raw.write(p);
    }
}

impl Attributes {
    /// `new AudioAttributes.Builder().setUsage(usage).setContentType(content_type)
    /// .setFlags(flags).build()`: the flags the builder takes, the
    /// communication notification usages folded into one.
    pub fn built(usage: i32, content_type: i32, flags: i32) -> Attributes {
        let usage = if (7..=9).contains(&usage) { USAGE_NOTIFICATION } else { usage };
        let flags = flags & FLAG_ALL_API_SET;
        let mut p = Parcel::new();
        p.write_i32(usage);
        p.write_i32(content_type);
        p.write_i32(AUDIO_SOURCE_INVALID);
        p.write_i32(flags);
        p.write_i32(0); // not flattened
        p.write_i32(0); // no tags
        p.write_i32(ATTR_PARCEL_IS_NULL_BUNDLE);
        let raw = Raw::of(&p);
        Attributes { usage, content_type, flags, force_ducking: false, raw }
    }

    /// `getUsage`: a system usage is unknown.
    pub fn public_usage(&self) -> i32 {
        if is_system_usage(self.usage) { 0 } else { self.usage }
    }

    pub fn force_ducking(&self) -> bool {
        self.force_ducking
    }

    pub fn content_type(&self) -> i32 {
        self.content_type
    }

    pub fn flags(&self) -> i32 {
        self.flags
    }

    /// `anonymizedCopy`'s: built of the usage, content type, public flags
    /// and whether anyone may capture it.
    fn write_anonymized(&self, p: &mut Parcel) {
        // A system usage is set as one, another as itself; `build()` folds the communication notifications into one.
        let usage = if (7..=9).contains(&self.usage) { USAGE_NOTIFICATION } else { self.usage };
        let mut flags = self.flags & FLAG_ALL_PUBLIC;
        if self.flags & FLAG_NO_SYSTEM_CAPTURE != 0 || self.flags & FLAG_NO_MEDIA_PROJECTION != 0 {
            flags |= FLAG_NO_MEDIA_PROJECTION | FLAG_NO_SYSTEM_CAPTURE;
        }
        p.write_i32(usage);
        p.write_i32(self.content_type);
        p.write_i32(AUDIO_SOURCE_INVALID);
        p.write_i32(flags);
        p.write_i32(0); // not flattened
        p.write_i32(0); // no tags
        p.write_i32(ATTR_PARCEL_IS_NULL_BUNDLE);
    }

}

/// `PlayerBase.PlayerIdCard`.
pub struct PlayerIdCard {
    pub player_type: i32,
    pub attributes: Attributes,
    pub player: Option<u32>,
    pub session: i32,
}

impl ReadParcelable for PlayerIdCard {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let player_type = r.read_i32()?;
        let attributes = Attributes::read_from(r)?;
        let player = match r.read_binder()? {
            Some(aim_binder_host::parcel::Binder::Handle(h)) => Some(h),
            _ => None,
        };
        Ok(PlayerIdCard { player_type, attributes, player, session: r.read_i32()? })
    }
}

/// `AudioPlaybackConfiguration`.
struct Player {
    piid: i32,
    player_type: i32,
    uid: i32,
    pid: i32,
    state: i32,
    device_ids: Vec<i32>,
    muted: i32,
    session: i32,
    attributes: Attributes,
    /// The `IPlayer`'s handle (`getPlayerProxy`), and it held with its
    /// death link, until released.
    iplayer: Option<u32>,
    player: Option<(Strong, u64)>,
    /// `FormatInfo`: spatialized, channel mask, sample rate.
    format: (bool, i32, i32),
}

impl Player {
    /// `isActive`.
    fn active(&self) -> bool {
        self.state == PLAYER_STATE_STARTED && self.muted & MUTE_AFFECTING_ACTIVE == 0
    }

    /// `writeToParcel`, or its anonymized copy's.
    fn write(&self, p: &mut Parcel, anonymized: bool) {
        p.write_i32(self.piid);
        if anonymized {
            p.write_i32(0); // no devices
            p.write_i32(0); // muted
            p.write_i32(PLAYER_TYPE_UNKNOWN);
            p.write_i32(PLAYER_UPID_INVALID);
            p.write_i32(PLAYER_UPID_INVALID);
            p.write_i32(self.state);
            self.attributes.write_anonymized(p);
            p.write_binder(None);
            p.write_i32(AUDIO_SESSION_ALLOCATE);
            p.write_bool(false);
            p.write_i32(0);
            p.write_i32(0);
            return;
        }
        p.write_i32(self.device_ids.len() as i32);
        for id in &self.device_ids {
            p.write_i32(*id);
        }
        p.write_i32(self.muted);
        p.write_i32(self.player_type);
        p.write_i32(self.uid);
        p.write_i32(self.pid);
        p.write_i32(self.state);
        self.attributes.write_to(p);
        p.write_binder(self.iplayer.map(aim_binder_host::parcel::Binder::Handle));
        p.write_i32(self.session);
        p.write_bool(self.format.0);
        p.write_i32(self.format.1);
        p.write_i32(self.format.2);
    }
}

/// `PlayMonitorClient`.
struct Client {
    dispatcher: Strong,
    death: u64,
    privileged: bool,
    errors: u32,
}

/// `RecordingState` of a tracked recorder.
struct Recorder {
    recorder: Strong,
    death: u64,
    started: bool,
}

/// `AudioPlayerStateMonitor`, a privileged playback callback of the
/// system server: the uids with an active player (`mActiveAudioUids`),
/// the uids that started one, the latest first
/// (`mSortedAudioPlaybackClientUids`), the active players it last heard
/// of by piid with their uid and type, and its listener (MediaSessionService's),
/// told of the uid and type of each player whose activeness changed.
#[derive(Default)]
pub struct StateMonitor {
    active_uids: Vec<i32>,
    sorted_uids: Vec<i32>,
    previous: BTreeMap<i32, (i32, i32)>,
    listener: Option<Box<dyn Fn(i32, i32) + Send>>,
}

impl StateMonitor {
    /// `registerListener`.
    pub fn set_listener(&mut self, listener: Box<dyn Fn(i32, i32) + Send>) {
        self.listener = Some(listener);
    }

    /// `getSortedAudioPlaybackClientUids`.
    pub fn sorted_uids(&self) -> Vec<i32> {
        self.sorted_uids.clone()
    }

    /// `isPlaybackActive`.
    pub fn is_playback_active(&self, uid: i32) -> bool {
        self.active_uids.contains(&uid)
    }

    /// `cleanUpAudioPlaybackUids`: the inactive uids of the user of
    /// `media_button_uid` played after it, forgotten.
    pub fn clean_up(&mut self, media_button_uid: i32) {
        const PER_USER: i32 = super::PER_USER_RANGE as i32;
        let user = media_button_uid / PER_USER;
        let mut i = self.sorted_uids.len();
        while i > 0 {
            i -= 1;
            let uid = self.sorted_uids[i];
            if uid == media_button_uid {
                break;
            }
            if uid / PER_USER == user && !self.active_uids.contains(&uid) {
                self.sorted_uids.remove(i);
            }
        }
    }

    /// `onPlaybackConfigChanged` of the players `configs` (piid, uid, type,
    /// active), in their order.
    fn changed(&mut self, configs: &[(i32, i32, i32, bool)]) {
        self.active_uids.clear();
        let mut active = BTreeMap::new();
        for &(piid, uid, kind, is_active) in configs {
            if is_active {
                if !self.active_uids.contains(&uid) {
                    self.active_uids.push(uid);
                }
                active.insert(piid, (uid, kind));
            }
        }
        for (piid, &(uid, _)) in &active {
            if self.previous.contains_key(piid) {
                continue;
            }
            match self.sorted_uids.iter().position(|&u| u == uid) {
                Some(0) => continue,
                Some(i) => {
                    self.sorted_uids.remove(i);
                }
                None => {}
            }
            self.sorted_uids.insert(0, uid);
        }
        // The first active uid moved to the front, those before it shifted.
        if !self.active_uids.is_empty() && !self.active_uids.contains(&self.sorted_uids[0]) {
            let first = (1..self.sorted_uids.len()).find(|&i| self.active_uids.contains(&self.sorted_uids[i]));
            // With none found, the original puts -1 first.
            let uid = first.map_or(-1, |i| self.sorted_uids[i]);
            for i in (1..=first.unwrap_or(0)).rev() {
                self.sorted_uids[i] = self.sorted_uids[i - 1];
            }
            self.sorted_uids[0] = uid;
        }
        let mut told = Vec::new();
        for &(piid, uid, kind, is_active) in configs {
            let was_active = self.previous.remove(&piid).is_some();
            if was_active != is_active {
                told.push((uid, kind));
            }
        }
        told.extend(self.previous.values().copied());
        self.previous = active;
        if let Some(listener) = &self.listener {
            for (uid, kind) in told {
                listener(uid, kind);
            }
        }
    }
}

#[derive(Default)]
pub struct Playback {
    /// The system server's `AudioPlayerStateMonitor`.
    pub monitor: StateMonitor,
    players: BTreeMap<i32, Player>,
    /// `mPiidToPortId`.
    port_ids: BTreeMap<i32, i32>,
    clients: Vec<Client>,
    recorders: BTreeMap<i32, Recorder>,
    /// `RecordingActivityMonitor.mClients`: each dispatcher, its death
    /// link and whether it is privileged. As the recording configurations
    /// are not kept (#1480), none changes and none is told.
    recording_clients: Vec<(Strong, u64, bool)>,
    /// `DuckingManager`: by uid, whether its duck is strong and the
    /// players ducked.
    duckers: BTreeMap<i32, (bool, Vec<i32>)>,
    /// `FadeOutManager`: by uid, the players faded out.
    faded: BTreeMap<i32, Vec<i32>>,
    /// `mMutedPlayers`: those muted for a call.
    muted: Vec<i32>,
    /// The `IPlayer` calls made, to send ([`Playback::send`]).
    calls: Vec<(i32, PlayerCall)>,
}

impl Playback {
    /// `AudioPlayerStateMonitor.isPlaybackActive`: an active player of
    /// `uid` (`mActiveAudioUids`).
    pub fn uid_active(&self, uid: i32) -> bool {
        self.players.values().any(|p| p.uid == uid && p.active())
    }

    /// `trackPlayer` of a caller of `uid`, `pid` given `piid`: the player's
    /// `IPlayer` and death link, made by the caller.
    pub fn track(&mut self, piid: i32, card: PlayerIdCard, uid: i32, pid: i32, player: Option<(Strong, u64)>) {
        self.players.insert(
            piid,
            Player {
                piid,
                player_type: card.player_type,
                uid,
                pid,
                state: PLAYER_STATE_IDLE,
                device_ids: Vec::new(),
                muted: 0,
                session: card.session,
                attributes: card.attributes,
                iplayer: card.player,
                player,
                format: (false, 0, 0),
            },
        );
    }

    /// `checkConfigurationCaller`: a player is changed by its own uid, or
    /// by root's (audioserver relays as itself).
    fn caller_may(&self, piid: i32, uid: i32) -> bool {
        self.players.get(&piid).is_some_and(|p| uid == 0 || p.uid == uid)
    }

    /// `playerAttributes`: whether it changed.
    pub fn attributes(&mut self, piid: i32, attributes: Attributes, uid: i32) -> bool {
        if !self.caller_may(piid, uid) {
            return false;
        }
        let p = self.players.get_mut(&piid).unwrap();
        let changed = p.attributes.raw != attributes.raw;
        p.attributes = attributes;
        changed
    }

    /// `playerSessionId`.
    pub fn session(&mut self, piid: i32, session: i32, uid: i32) -> bool {
        if !self.caller_may(piid, uid) {
            return false;
        }
        let p = self.players.get_mut(&piid).unwrap();
        let changed = p.session != session;
        p.session = session;
        changed
    }

    /// `playerEvent`: whether the configurations changed.
    pub fn event(&mut self, piid: i32, event: i32, values: &[i32], uid: i32) -> bool {
        let Some(p) = self.players.get(&piid) else { return false };
        if event == PLAYER_UPDATE_PORT_ID {
            if let Some(port) = values.first() {
                self.port_ids.insert(piid, *port);
            }
            return false;
        }
        // A SoundPool reports nothing but its release.
        if p.player_type == PLAYER_TYPE_JAM_SOUNDPOOL && event != PLAYER_STATE_RELEASED {
            return false;
        }
        if !self.caller_may(piid, uid) {
            return false;
        }
        let p = self.players.get_mut(&piid).unwrap();
        // `handleStateEvent`.
        let mut changed = false;
        if event != PLAYER_UPDATE_DEVICE_ID {
            changed = p.state != event;
            p.state = event;
        }
        if event == PLAYER_STATE_STARTED || event == PLAYER_UPDATE_DEVICE_ID {
            changed |= p.device_ids != values;
            p.device_ids = values.to_vec();
        }
        if changed && event == PLAYER_STATE_RELEASED {
            p.iplayer = None;
            p.player = None;
        }
        if changed && event == PLAYER_STATE_STARTED {
            self.check_duck(piid);
            self.check_fade(piid);
        }
        changed
    }

    /// `releasePlayer`: the player and its `IPlayer` go. Returns the death
    /// link to clear.
    pub fn release(&mut self, piid: i32, uid: i32) -> (bool, Option<(Strong, u64)>) {
        if piid == PLAYER_PIID_INVALID || !self.caller_may(piid, uid) {
            return (false, None);
        }
        let mut p = self.players.remove(&piid).unwrap();
        self.port_ids.remove(&piid);
        // `removeReleased`.
        if let Some((_, ducked)) = self.duckers.get_mut(&p.uid) {
            ducked.retain(|d| *d != piid);
        }
        if let Some(faded) = self.faded.get_mut(&p.uid) {
            faded.retain(|f| *f != piid);
        }
        // `handleStateEvent(PLAYER_STATE_RELEASED)`.
        let changed = p.state != PLAYER_STATE_RELEASED;
        (changed, p.player.take())
    }

    fn by_port(&self, port: i32) -> Option<i32> {
        self.port_ids.iter().find(|(_, p)| **p == port).map(|(piid, _)| *piid)
    }

    /// `portMuteEvent` from audioserver: `handleMutedEvent`.
    pub fn port_mute(&mut self, port: i32, muted: i32) -> bool {
        let Some(p) = self.by_port(port).and_then(|piid| self.players.get_mut(&piid)) else { return false };
        if p.player_type == PLAYER_TYPE_JAM_SOUNDPOOL {
            return false;
        }
        let changed = p.muted != muted;
        p.muted = muted;
        changed
    }

    /// `portEvent` from audioserver: a format update (`handleFormatEvent`).
    pub fn port_event(&mut self, port: i32, event: i32, extras: Option<&std::collections::HashMap<String, Value>>) -> bool {
        let Some(p) = self.by_port(port).and_then(|piid| self.players.get_mut(&piid)) else { return false };
        if p.player_type == PLAYER_TYPE_JAM_SOUNDPOOL || event != PLAYER_UPDATE_FORMAT {
            return false;
        }
        let Some(extras) = extras else { return false };
        let int = |k: &str| match extras.get(k) {
            Some(Value::Int(v)) => *v,
            _ => 0,
        };
        let format = (matches!(extras.get(EXTRA_SPATIALIZED), Some(Value::Bool(true))), int(EXTRA_CHANNEL_MASK), int(EXTRA_SAMPLE_RATE));
        let changed = p.format != format;
        p.format = format;
        changed
    }

    /// `registerPlaybackCallback`.
    pub fn add_client(&mut self, dispatcher: Strong, death: u64, privileged: bool) {
        self.clients.push(Client { dispatcher, death, privileged, errors: 0 });
    }

    /// `RecordingActivityMonitor.registerRecordingCallback`.
    pub fn add_recording_client(&mut self, dispatcher: Strong, death: u64, privileged: bool) {
        self.recording_clients.push((dispatcher, death, privileged));
    }

    /// `unregisterRecordingCallback`: the death links to clear.
    pub fn remove_recording_client(&mut self, handle: u32) -> Vec<(Strong, u64)> {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.recording_clients).into_iter().partition(|(d, _, _)| d.handle == handle);
        self.recording_clients = kept;
        gone.into_iter().map(|(d, death, _)| (d, death)).collect()
    }

    /// `unregisterPlaybackCallback`: the death links to clear.
    pub fn remove_client(&mut self, handle: u32) -> Vec<(Strong, u64)> {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.clients).into_iter().partition(|c| c.dispatcher.handle == handle);
        self.clients = kept;
        gone.into_iter().map(|c| (c.dispatcher, c.death)).collect()
    }

    /// `getActivePlaybackConfigurations`: all of them, or the active ones
    /// anonymized.
    pub fn write_configurations(&self, p: &mut Parcel, privileged: bool) {
        let players: Vec<&Player> = self.players.values().filter(|pl| privileged || pl.active()).collect();
        p.write_i32(players.len() as i32);
        for pl in players {
            p.write_i32(1);
            pl.write(p, !privileged);
        }
    }

    /// `dispatchPlaybackChange` to each client of fewer than
    /// [`MAX_ERRORS`] failed dispatches.
    pub fn dispatch(&mut self, released: bool) {
        if self.players.is_empty() {
            return;
        }
        let mut errors = Vec::new();
        for (i, c) in self.clients.iter().enumerate().filter(|(_, c)| c.errors < MAX_ERRORS) {
            let mut p = Parcel::new();
            p.write_interface_token(DISPATCHER_DESCRIPTOR);
            self.write_configurations(&mut p, c.privileged);
            p.write_bool(c.privileged && released);
            if c.dispatcher.transact(DISPATCH_PLAYBACK_CONFIG_CHANGE, &p, true).is_err() {
                errors.push(i);
            }
        }
        for i in errors {
            self.clients[i].errors += 1;
        }
        let configs: Vec<_> = self.players.values().map(|p| (p.piid, p.uid, p.player_type, p.active())).collect();
        self.monitor.changed(&configs);
    }

    /// `trackRecorder`.
    pub fn track_recorder(&mut self, riid: i32, recorder: Strong, death: u64) {
        self.recorders.insert(riid, Recorder { recorder, death, started: false });
    }

    /// `recorderEvent`.
    pub fn recorder_event(&mut self, riid: i32, event: i32) {
        if let Some(r) = self.recorders.get_mut(&riid) {
            r.started = event == RECORDER_STATE_STARTED;
        }
    }

    /// `releaseRecorder`: the death link to clear.
    pub fn release_recorder(&mut self, riid: i32) -> Option<(Strong, u64)> {
        self.recorders.remove(&riid).map(|r| (r.recorder, r.death))
    }

    /// The players of a dead process (`IPlayer` deaths).
    pub fn players_of(&self, handle: u32) -> Vec<i32> {
        self.players.values().filter(|p| p.iplayer == Some(handle)).map(|p| p.piid).collect()
    }

    /// A call to a player's `IPlayer`; none without one (`getPlayerProxy`
    /// is null).
    fn call(&mut self, piid: i32, call: PlayerCall) -> bool {
        if self.players.get(&piid).is_none_or(|p| p.iplayer.is_none()) {
            return false;
        }
        self.calls.push((piid, call));
        true
    }

    /// Sends the `IPlayer` calls made.
    pub fn send(&mut self) {
        for (piid, call) in std::mem::take(&mut self.calls) {
            let Some((player, _)) = self.players.get(&piid).and_then(|p| p.player.as_ref()) else { continue };
            let mut p = Parcel::new();
            let code = match call {
                PlayerCall::Shape(shaper, operation) => {
                    iplayer::ApplyVolumeShaper { configuration: Some(shaper), operation: Some(operation) }.write(&mut p);
                    iplayer::APPLY_VOLUME_SHAPER
                }
                PlayerCall::Volume(vol) => {
                    iplayer::SetVolume { vol }.write(&mut p);
                    iplayer::SET_VOLUME
                }
            };
            if let Err(e) = player.transact(code, &p, true) {
                eprintln!("app host: player {piid}: {call:?}: {e:?}");
            }
        }
    }

    /// `DuckedApp.addDuck`.
    fn add_duck(&mut self, uid: i32, piid: i32, skip_ramp: bool) {
        let Some(&(strong, ref ducked)) = self.duckers.get(&uid) else { return };
        if ducked.contains(&piid) {
            return;
        }
        if self.call(piid, PlayerCall::Shape(if strong { STRONG_DUCK } else { DUCK }, Operation::Play { skip_ramp })) {
            self.duckers.get_mut(&uid).unwrap().1.push(piid);
        }
    }

    /// `DuckingManager.checkDuck` of a player that started.
    fn check_duck(&mut self, piid: i32) {
        let uid = self.players[&piid].uid;
        self.add_duck(uid, piid, true);
    }

    /// `duckPlayers`: the started players of `loser_uid` (not also the
    /// winner's) ducked, strongly for an assistant; none when one of them
    /// speaks (unless forced) or cannot be ducked, and the loser is then
    /// told instead.
    pub fn duck_players(&mut self, winner_uid: i32, loser_uid: i32, strong: bool, force: bool) -> bool {
        if self.players.is_empty() {
            return true;
        }
        let mut to_duck = Vec::new();
        for p in self.players.values().filter(|p| p.uid != winner_uid && p.uid == loser_uid && p.state == PLAYER_STATE_STARTED) {
            if !force && p.attributes.content_type == CONTENT_TYPE_SPEECH || UNDUCKABLE_PLAYER_TYPES.contains(&p.player_type) {
                return false;
            }
            to_duck.push(p.piid);
        }
        // `duckUid`: the uid is ducked even without players, for those
        // that start.
        self.duckers.entry(loser_uid).or_insert((strong, Vec::new()));
        for piid in to_duck {
            self.add_duck(loser_uid, piid, false);
        }
        true
    }

    /// `DuckingManager.unduckUid`.
    fn unduck_uid(&mut self, uid: i32) {
        let Some((strong, ducked)) = self.duckers.remove(&uid) else { return };
        let id = if strong { STRONG_DUCK.id } else { DUCK.id };
        for piid in ducked {
            self.call(piid, PlayerCall::Shape(Shaper { id, scale: None }, Operation::Reverse));
        }
    }

    /// `FadedOutApp.addFade`.
    fn add_fade(&mut self, uid: i32, piid: i32, skip_ramp: bool) {
        let Some(faded) = self.faded.get(&uid) else { return };
        if faded.contains(&piid) {
            return;
        }
        if self.call(piid, PlayerCall::Shape(FADE_OUT, Operation::Play { skip_ramp })) {
            self.faded.get_mut(&uid).unwrap().push(piid);
        }
    }

    /// `FadeOutManager.checkFade` of a player that started.
    fn check_fade(&mut self, piid: i32) {
        let p = &self.players[&piid];
        if fade_shapers(&p.attributes).is_some() {
            self.add_fade(p.uid, piid, true);
        }
    }

    /// `fadeOutPlayers`: the started players of `loser_uid` (not also the
    /// winner's) faded out, unless the winner's request is speech, the
    /// loser pauses on a duck, or one of them is not fadeable. Whether
    /// some are.
    pub fn fade_out_players(&mut self, winner_uid: i32, winner_content_type: i32, loser_uid: i32, loser_flags: i32) -> bool {
        if self.players.is_empty() {
            return false;
        }
        // `canCauseFadeOut`.
        if winner_content_type == CONTENT_TYPE_SPEECH || loser_flags & AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS != 0 {
            return false;
        }
        let mut to_fade = Vec::new();
        for p in self.players.values().filter(|p| p.uid != winner_uid && p.uid == loser_uid && p.state == PLAYER_STATE_STARTED) {
            if !fadeable(&p.attributes, p.player_type) {
                return false;
            }
            to_fade.push(p.piid);
        }
        if to_fade.is_empty() {
            return false;
        }
        self.faded.entry(loser_uid).or_default();
        for piid in to_fade {
            self.add_fade(loser_uid, piid, false);
        }
        true
    }

    /// `FadeOutManager.unfadeOutUid`: each faded player faded back in,
    /// joining its fade out; one whose usage no longer fades is left, as
    /// the original's null shaper fails.
    fn unfade_uid(&mut self, uid: i32) {
        let Some(faded) = self.faded.remove(&uid) else { return };
        for piid in faded {
            let Some(p) = self.players.get(&piid) else { continue };
            match fade_shapers(&p.attributes) {
                Some((out, fade_in)) => {
                    self.call(piid, PlayerCall::Shape(fade_in, Operation::Join(out.id)));
                }
                None => eprintln!("app host: fading in player {piid}: no shaper for its usage"),
            }
        }
    }

    /// `restoreVShapedPlayers` of a winner's uid.
    pub fn restore(&mut self, uid: i32) {
        self.unduck_uid(uid);
        self.unfade_uid(uid);
    }

    /// `forgetUid`.
    pub fn forget_uid(&mut self, uid: i32) {
        self.unfade_uid(uid);
        self.unduck_uid(uid);
    }

    /// `mutePlayersForCall`: media and game players.
    pub fn mute_for_call(&mut self) {
        let piids: Vec<i32> = self.players.values().filter(|p| USAGES_TO_MUTE_IN_RING_OR_CALL.contains(&p.attributes.public_usage())).map(|p| p.piid).collect();
        for piid in piids {
            if self.call(piid, PlayerCall::Volume(0.0)) {
                self.muted.push(piid);
            }
        }
    }

    /// `unmutePlayersForCall`.
    pub fn unmute_for_call(&mut self) {
        for piid in std::mem::take(&mut self.muted) {
            self.call(piid, PlayerCall::Volume(1.0));
        }
    }

    /// The calls made, taken unsent.
    #[cfg(test)]
    pub(super) fn take_calls(&mut self) -> Vec<(i32, PlayerCall)> {
        std::mem::take(&mut self.calls)
    }

    /// `getFocusDuckedUids`.
    pub fn ducked_uids(&self) -> Vec<i32> {
        self.duckers.keys().copied().collect()
    }
}

/// A port event's extras (`PersistableBundle`, as a Bundle is written).
pub fn read_extras(raw: &crate::app_host::kept::RawBundle) -> Option<std::collections::HashMap<String, Value>> {
    let mut p = Parcel::new();
    raw.write_to(&mut p);
    bundle::read(&mut Reader::new(p.data(), &[])).ok().flatten()
}

/// Clears a death link of `process`.
pub fn clear(process: &LocalProcess, link: Option<(Strong, u64)>) {
    if let Some((strong, cookie)) = link {
        process.clear_death(&strong, cookie);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes(usage: i32, flags: i32) -> Attributes {
        let mut p = Parcel::new();
        p.write_i32(usage);
        p.write_i32(2); // music
        p.write_i32(AUDIO_SOURCE_INVALID);
        p.write_i32(flags);
        p.write_i32(0);
        p.write_i32(1);
        p.write_string16(Some("tag"));
        p.write_i32(ATTR_PARCEL_IS_NULL_BUNDLE);
        Attributes::read_from(&mut Reader::new(p.data(), &[])).unwrap()
    }

    fn card() -> PlayerIdCard {
        PlayerIdCard { player_type: 1, attributes: attributes(1, 0), player: None, session: 9 }
    }

    #[test]
    fn a_player_reports_its_states() {
        let mut pb = Playback::default();
        pb.track(5, card(), 10_154, 77, None);
        // Only its own uid (or root) changes it.
        assert!(!pb.event(5, PLAYER_STATE_STARTED, &[2], 10_155));
        assert!(pb.event(5, PLAYER_STATE_STARTED, &[2], 10_154));
        assert!(!pb.event(5, PLAYER_STATE_STARTED, &[2], 10_154));
        assert!(pb.players[&5].active());
        // A port id maps audioserver's port events to it.
        assert!(!pb.event(5, PLAYER_UPDATE_PORT_ID, &[40], 0));
        assert!(pb.port_mute(40, 1 << 4));
        assert!(!pb.players[&5].active());
        assert!(pb.release(5, 10_154).0);
        assert!(pb.players.is_empty() && pb.port_ids.is_empty());
    }

    #[test]
    fn the_public_sees_active_players_anonymized() {
        let mut pb = Playback::default();
        pb.track(5, card(), 10_154, 77, None);
        pb.track(6, card(), 10_154, 77, None);
        pb.event(5, PLAYER_STATE_STARTED, &[2], 10_154);
        let mut p = Parcel::new();
        pb.write_configurations(&mut p, false);
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 5)); // piid
        assert_eq!(r.read_i32().unwrap(), 0); // no devices
        r.read_i32().unwrap();
        assert_eq!(r.read_i32().unwrap(), PLAYER_TYPE_UNKNOWN);
        assert_eq!(r.read_i32().unwrap(), PLAYER_UPID_INVALID);
        r.read_i32().unwrap();
        assert_eq!(r.read_i32().unwrap(), PLAYER_STATE_STARTED);
        let a = Attributes::read_from(&mut r).unwrap();
        assert_eq!((a.public_usage(), a.flags), (1, 0));
    }

    #[test]
    fn a_sound_pool_reports_only_its_release() {
        let mut pb = Playback::default();
        pb.track(5, PlayerIdCard { player_type: PLAYER_TYPE_JAM_SOUNDPOOL, ..card() }, 10_154, 77, None);
        assert!(!pb.event(5, PLAYER_STATE_STARTED, &[], 10_154));
        assert!(pb.event(5, PLAYER_STATE_RELEASED, &[], 10_154));
    }

    /// A player of `uid` with an `IPlayer`, of `usage` and `content_type`.
    fn track(pb: &mut Playback, piid: i32, uid: i32, usage: i32, content_type: i32, player_type: i32) {
        let card = PlayerIdCard { player_type, attributes: Attributes::built(usage, content_type, 0), player: Some(piid as u32), session: 9 };
        pb.track(piid, card, uid, 77, None);
    }

    #[test]
    fn a_duck_shapes_the_losers_started_players_and_those_that_start() {
        let mut pb = Playback::default();
        track(&mut pb, 5, 10_154, USAGE_MEDIA, 2, 1);
        track(&mut pb, 6, 10_154, USAGE_MEDIA, 2, 1);
        track(&mut pb, 7, 10_155, USAGE_MEDIA, 2, 1);
        pb.event(5, PLAYER_STATE_STARTED, &[], 10_154);
        pb.event(7, PLAYER_STATE_STARTED, &[], 10_155);
        assert!(pb.duck_players(10_155, 10_154, false, false));
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Shape(DUCK, Operation::Play { skip_ramp: false }))]);
        assert_eq!(pb.ducked_uids(), [10_154]);
        // One that starts while ducked is ducked at once.
        pb.event(6, PLAYER_STATE_STARTED, &[], 10_154);
        assert_eq!(pb.take_calls(), [(6, PlayerCall::Shape(DUCK, Operation::Play { skip_ramp: true }))]);
        // The winner's own players are not ducked; a released one is forgotten.
        pb.release(6, 10_154);
        pb.restore(10_155);
        assert!(pb.take_calls().is_empty());
        pb.restore(10_154);
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Shape(Shaper { id: 1, scale: None }, Operation::Reverse))]);
        assert!(pb.ducked_uids().is_empty());
    }

    #[test]
    fn speech_and_unduckable_players_are_not_ducked() {
        let mut pb = Playback::default();
        track(&mut pb, 5, 10_154, USAGE_MEDIA, CONTENT_TYPE_SPEECH, 1);
        pb.event(5, PLAYER_STATE_STARTED, &[], 10_154);
        assert!(!pb.duck_players(10_155, 10_154, false, false));
        assert!(pb.ducked_uids().is_empty());
        // A forced duck (accessibility's) ducks speech, strongly for an assistant.
        assert!(pb.duck_players(10_155, 10_154, true, true));
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Shape(STRONG_DUCK, Operation::Play { skip_ramp: false }))]);
        track(&mut pb, 6, 10_156, USAGE_MEDIA, 2, PLAYER_TYPE_AAUDIO);
        pb.event(6, PLAYER_STATE_STARTED, &[], 10_156);
        assert!(!pb.duck_players(10_155, 10_156, false, true));
        // A player without an `IPlayer` is not ducked.
        pb.track(8, card(), 10_157, 77, None);
        pb.event(8, PLAYER_STATE_STARTED, &[], 10_157);
        assert!(pb.duck_players(10_155, 10_157, false, false));
        assert!(pb.take_calls().is_empty());
    }

    #[test]
    fn a_loss_fades_media_out_and_back_in() {
        let mut pb = Playback::default();
        track(&mut pb, 5, 10_154, USAGE_MEDIA, 2, 1);
        assert!(!pb.fade_out_players(10_155, 2, 10_154, 0), "nothing plays");
        pb.event(5, PLAYER_STATE_STARTED, &[], 10_154);
        assert!(!pb.fade_out_players(10_155, CONTENT_TYPE_SPEECH, 10_154, 0));
        assert!(!pb.fade_out_players(10_155, 2, 10_154, AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS));
        assert!(pb.fade_out_players(10_155, 2, 10_154, 0));
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Shape(FADE_OUT, Operation::Play { skip_ramp: false }))]);
        pb.forget_uid(10_154);
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Shape(FADE_IN, Operation::Join(2)))]);
        // An alarm is not faded.
        track(&mut pb, 6, 10_156, 4, 2, 1);
        pb.event(6, PLAYER_STATE_STARTED, &[], 10_156);
        assert!(!pb.fade_out_players(10_155, 2, 10_156, 0));
        assert_eq!(fade_out_duration_ms(&Attributes::built(USAGE_GAME, 2, 0)), 2000);
        assert_eq!(fade_out_duration_ms(&Attributes::built(4, 2, 0)), 0);
    }

    #[test]
    fn a_call_mutes_media_and_games() {
        let mut pb = Playback::default();
        track(&mut pb, 5, 10_154, USAGE_MEDIA, 2, 1);
        track(&mut pb, 6, 10_154, USAGE_GAME, 2, 1);
        track(&mut pb, 7, 10_154, USAGE_NOTIFICATION, 4, 1);
        pb.mute_for_call();
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Volume(0.0)), (6, PlayerCall::Volume(0.0))]);
        pb.unmute_for_call();
        assert_eq!(pb.take_calls(), [(5, PlayerCall::Volume(1.0)), (6, PlayerCall::Volume(1.0))]);
        pb.unmute_for_call();
        assert!(pb.take_calls().is_empty());
    }

    #[test]
    fn shapers_are_written_as_structured_parcelables() {
        let mut p = Parcel::new();
        DUCK.write_to(&mut p);
        Shaper { id: 1, scale: None }.write_to(&mut p);
        Operation::Play { skip_ramp: true }.write_to(&mut p);
        Operation::Join(2).write_to(&mut p);
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(r.read_i32().unwrap(), 4 * 5 + 8 + 4 * 5 + 4 * 4); // its size
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 1, 1 << 1)); // SCALE, id, CLOCK_TIME
        assert_eq!(f64::from_bits(r.read_i64().unwrap() as u64), 500.0);
        assert_eq!(r.read_i32().unwrap(), 1); // an interpolator
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (4 * 5 + 4 * 4, 2)); // its size, CUBIC
        assert_eq!((r.read_f32().unwrap(), r.read_f32().unwrap(), r.read_i32().unwrap()), (0.0, 0.0, 4));
        let xy: Vec<f32> = (0..4).map(|_| r.read_f32().unwrap()).collect();
        assert_eq!(xy, [0.0, 1.0, 1.0, 0.2]);
        // An id alone: no interpolator.
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()), (4 * 4 + 8 + 4, 0, 1, 0));
        assert_eq!((r.read_i64().unwrap(), r.read_i32().unwrap()), (0, 0));
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_f32().unwrap()), (16, 1 << 4, -1, 1.0));
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()), (16, 1 << 2, 2));
        assert!(r.read_f32().unwrap().is_nan());
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn attributes_carry_the_accessibility_duck() {
        let mut p = Parcel::new();
        p.write_i32(11);
        p.write_i32(1);
        p.write_i32(AUDIO_SOURCE_INVALID);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(0);
        p.write_i32(ATTR_PARCEL_IS_VALID_BUNDLE);
        bundle::write(&mut p, &[(KEY_FORCE_DUCKING, Value::Bool(true))]);
        let a = Attributes::read_from(&mut Reader::new(p.data(), &[])).unwrap();
        assert!(a.force_ducking());
        assert!(!attributes(11, 0).force_ducking());
        assert_eq!(Attributes::built(1001, 0, 0).public_usage(), 0);
    }

    /// `AudioPlayerStateMonitor`: a newly active player's uid first, the
    /// first active uid moved to the front when the front one stops, the
    /// listener told of each player whose activeness changed, and the
    /// inactive uids played after the media button uid cleaned up.
    #[test]
    fn the_state_monitor_sorts_uids_by_playback() {
        let told = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut m = StateMonitor::default();
        let t = told.clone();
        m.set_listener(Box::new(move |uid, kind| t.lock().unwrap().push((uid, kind))));
        m.changed(&[(1, 10_100, 2, true)]);
        m.changed(&[(1, 10_100, 2, true), (2, 10_200, 2, true)]);
        assert_eq!(m.sorted_uids(), [10_200, 10_100]);
        m.changed(&[(1, 10_100, 2, true), (2, 10_200, 2, false)]);
        assert_eq!(m.sorted_uids(), [10_100, 10_200]);
        assert!(m.is_playback_active(10_100) && !m.is_playback_active(10_200));
        // A player gone is told too.
        m.changed(&[]);
        assert_eq!(*told.lock().unwrap(), [(10_100, 2), (10_200, 2), (10_200, 2), (10_100, 2)]);
        m.changed(&[(3, 10_300, 2, true)]);
        m.changed(&[]);
        assert_eq!(m.sorted_uids(), [10_300, 10_100, 10_200]);
        m.clean_up(10_100);
        assert_eq!(m.sorted_uids(), [10_300, 10_100]);
    }
}
