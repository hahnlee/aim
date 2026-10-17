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
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use crate::bundle::{self, Value};
use crate::clip::Raw;

/// `AudioPlaybackConfiguration.PLAYER_*`.
pub const PLAYER_PIID_INVALID: i32 = -1;
const PLAYER_UPID_INVALID: i32 = -1;
const PLAYER_TYPE_UNKNOWN: i32 = -1;
const PLAYER_TYPE_JAM_SOUNDPOOL: i32 = 3;
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
const USAGE_NOTIFICATION: i32 = 5;
const USAGE_CALL_ASSISTANT: i32 = 17;
const SYSTEM_USAGE_OFFSET: i32 = 1000;
const FLAG_AUDIBILITY_ENFORCED: i32 = 1;
const FLAG_HW_AV_SYNC: i32 = 1 << 4;
const FLAG_LOW_LATENCY: i32 = 1 << 8;
const FLAG_NO_MEDIA_PROJECTION: i32 = 1 << 10;
const FLAG_NO_SYSTEM_CAPTURE: i32 = 1 << 12;
const FLAG_ALL_PUBLIC: i32 = FLAG_AUDIBILITY_ENFORCED | FLAG_HW_AV_SYNC | FLAG_LOW_LATENCY;
const FLATTEN_TAGS: i32 = 1;
const ATTR_PARCEL_IS_NULL_BUNDLE: i32 = -1977;
const ATTR_PARCEL_IS_VALID_BUNDLE: i32 = 1980;
/// `MediaRecorder.AudioSource.AUDIO_SOURCE_INVALID`.
const AUDIO_SOURCE_INVALID: i32 = -1;

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
        match r.read_i32()? {
            ATTR_PARCEL_IS_NULL_BUNDLE => {}
            ATTR_PARCEL_IS_VALID_BUNDLE => {
                crate::clip::bundle(r)?;
            }
            _ => return Err(BAD_VALUE),
        }
        Ok(Attributes { usage, content_type, flags, raw: Raw::capture(r, start) })
    }
}

impl WriteParcelable for Attributes {
    fn write_to(&self, p: &mut Parcel) {
        self.raw.write(p);
    }
}

impl Attributes {
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
    /// The `IPlayer`, held with its death link, until released.
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
        p.write_binder(self.player.as_ref().map(|(s, _)| s.binder()));
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

#[derive(Default)]
pub struct Playback {
    players: BTreeMap<i32, Player>,
    /// `mPiidToPortId`.
    port_ids: BTreeMap<i32, i32>,
    clients: Vec<Client>,
    recorders: BTreeMap<i32, Recorder>,
}

impl Playback {
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
            p.player = None;
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
        self.players.values().filter(|p| p.player.as_ref().is_some_and(|(s, _)| s.handle == handle)).map(|p| p.piid).collect()
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

    /// `getUsage`: a system usage is unknown.
    fn public_usage(a: &Attributes) -> i32 {
        if is_system_usage(a.usage) { 0 } else { a.usage }
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
        assert_eq!((public_usage(&a), a.flags), (1, 0));
    }

    #[test]
    fn a_sound_pool_reports_only_its_release() {
        let mut pb = Playback::default();
        pb.track(5, PlayerIdCard { player_type: PLAYER_TYPE_JAM_SOUNDPOOL, ..card() }, 10_154, 77, None);
        assert!(!pb.event(5, PLAYER_STATE_STARTED, &[], 10_154));
        assert!(pb.event(5, PLAYER_STATE_RELEASED, &[], 10_154));
    }
}
