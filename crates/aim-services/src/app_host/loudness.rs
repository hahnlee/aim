//! `audio`'s loudness codec updates for the app host (`LoudnessCodecHelper`
//! at `android-16.0.0_r1`): the codecs a process's `LoudnessCodecController`
//! adds for an audio session (`startLoudnessCodecUpdates`,
//! `addLoudnessCodecInfo`), the players of that session and the device each
//! plays to, and the MPEG-4 and MPEG-D DRC parameters for a codec on that
//! device (`createLoudnessParameters`, by the device's CTA-2075 SPL range),
//! sent to the registered dispatchers when a codec is added and when a
//! player of a started session moves to another device. The SPL range of a
//! Bluetooth output depends on its audio device category, which the app
//! host does not keep (#1850).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::AppHost;
use super::display::parcelable;
use super::playback::PlayerDevice;
use crate::bundle::{self, Value};

const DISPATCHER_DESCRIPTOR: &str = "android.media.ILoudnessCodecUpdatesDispatcher";
/// `dispatchLoudnessCodecParameterChange`, one-way.
const DISPATCH_LOUDNESS_CODEC_PARAMETER_CHANGE: u32 = 1;
/// `SYSTEM_PROPERTY_SPEAKER_SPL_RANGE_SIZE`.
const SPEAKER_SPL_RANGE_SIZE: &str = "audio.loudness.builtin-speaker-spl-range-size";
/// `PackageManager.FEATURE_LEANBACK`, `FEATURE_AUTOMOTIVE`.
const FEATURE_LEANBACK: &str = "android.software.leanback";
const FEATURE_AUTOMOTIVE: &str = "android.hardware.type.automotive";
/// `LoudnessCodecInfo.CodecMetadataType`.
const CODEC_METADATA_TYPE_MPEG_4: i32 = 1;
const CODEC_METADATA_TYPE_MPEG_D: i32 = 2;
/// `MediaFormat.KEY_AAC_DRC_*`.
const KEY_AAC_DRC_TARGET_REFERENCE_LEVEL: &str = "aac-target-ref-level";
const KEY_AAC_DRC_HEAVY_COMPRESSION: &str = "aac-drc-heavy-compression";
const KEY_AAC_DRC_EFFECT_TYPE: &str = "aac-drc-effect-type";
/// `AudioDeviceType.OUT_SPEAKER`, `OUT_HEADPHONE`, `OUT_HEADSET`.
const TYPE_OUT_SPEAKER: i32 = 140;
const TYPE_OUT_HEADPHONE: i32 = 136;
const TYPE_OUT_HEADSET: i32 = 137;

/// `DeviceSplRange`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SplRange {
    Unknown,
    Small,
    Medium,
    Large,
}

/// `stringToSplRange`.
fn spl_range_of(name: &str) -> SplRange {
    match name {
        "large" => SplRange::Large,
        "medium" => SplRange::Medium,
        "small" => SplRange::Small,
        _ => SplRange::Unknown,
    }
}

/// A `LoudnessCodecInfo`: the codec's metadata type and whether it
/// downmixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodecInfo {
    pub metadata_type: i32,
    pub downmixing: bool,
}

impl CodecInfo {
    /// The generated `hashCode` (`Arrays.deepHashCode` of its fields).
    fn hash_code(&self) -> i32 {
        let downmixing = if self.downmixing { 1231 } else { 1237 };
        31i32
            .wrapping_add(self.metadata_type)
            .wrapping_mul(31)
            .wrapping_add(downmixing)
    }
}

impl ReadParcelable for CodecInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        parcelable(r, |r| {
            Ok(CodecInfo {
                metadata_type: r.read_i32()?,
                downmixing: r.read_i32()? != 0,
            })
        })
    }
}

impl WriteParcelable for CodecInfo {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.metadata_type);
        p.write_bool(self.downmixing);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// `LoudnessCodecInputProperties.createLoudnessParameters`.
fn loudness_parameters(range: SplRange, info: CodecInfo) -> Vec<(&'static str, Value)> {
    let mpeg_4 = info.metadata_type == CODEC_METADATA_TYPE_MPEG_4;
    let mpeg_d = info.metadata_type == CODEC_METADATA_TYPE_MPEG_D;
    // The target reference level (an attenuation), the MPEG-4 heavy
    // compression and the MPEG-D DRC effect type.
    let (level, heavy, effect) = match range {
        SplRange::Large => (124, 0, 6),
        SplRange::Small => (64, 1, 3),
        SplRange::Medium | SplRange::Unknown => (96, i32::from(info.downmixing), 6),
    };
    let mut params = vec![(KEY_AAC_DRC_TARGET_REFERENCE_LEVEL, Value::Int(level))];
    if mpeg_4 {
        params.push((KEY_AAC_DRC_HEAVY_COMPRESSION, Value::Int(heavy)));
    } else if mpeg_d {
        params.push((KEY_AAC_DRC_EFFECT_TYPE, Value::Int(effect)));
    }
    params
}

/// An output device as `getDeviceSplRange` tells them apart, by its
/// `AudioDeviceDescription`.
#[derive(Debug, PartialEq)]
enum Output {
    Speaker,
    /// USB headset, wired headphones or headset.
    Headphones,
    /// A2DP, SCO or LE Audio (`AudioSystem.isBluetoothDevice`).
    Bluetooth,
    Other,
}

fn output(kind: i32, connection: &str) -> Output {
    match (kind, connection) {
        (TYPE_OUT_SPEAKER, "") => Output::Speaker,
        (TYPE_OUT_HEADSET, "usb" | "analog") | (TYPE_OUT_HEADPHONE, "analog") => Output::Headphones,
        (_, "bt-a2dp" | "bt-sco" | "bt-le") => Output::Bluetooth,
        _ => Output::Other,
    }
}

/// `LoudnessTrackId`: an audio session of a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TrackId {
    session: i32,
    pid: i32,
}

#[derive(Default)]
struct State {
    /// `mStartedConfigPiids`.
    piids: HashMap<TrackId, BTreeSet<i32>>,
    /// `mStartedConfigInfo`.
    infos: HashMap<TrackId, BTreeSet<Option<CodecInfo>>>,
    /// `mPiidToDeviceIdCache`.
    piid_device: HashMap<i32, i32>,
    /// `mPiidToPidCache`.
    piid_pid: HashMap<i32, i32>,
}

impl State {
    fn started(&self, id: &TrackId) -> bool {
        self.infos.contains_key(id) && self.piids.contains_key(id)
    }

    /// `onClientPidDied`.
    fn client_died(&mut self, pid: i32) {
        let piid_device = &mut self.piid_device;
        self.piid_pid
            .iter()
            .filter(|(_, p)| **p == pid)
            .for_each(|(piid, _)| {
                piid_device.remove(piid);
            });
        self.piids.retain(|id, _| id.pid != pid);
        self.infos.retain(|id, _| id.pid != pid);
    }
}

/// A started session's update: the codec bundles by key.
type Update = Vec<(String, Vec<(&'static str, Value)>)>;

/// `dispatchLoudnessCodecParameterChange`'s bundle: each codec's
/// parameters as a nested `PersistableBundle`.
fn update_bundle(update: &Update) -> Vec<(&str, Value)> {
    update
        .iter()
        .map(|(key, params)| {
            let params = params.iter().map(|(k, v)| (k.to_string(), v.clone()));
            (key.as_str(), Value::PersistableBundle(params.collect()))
        })
        .collect()
}

/// A `PersistableBundle` of parameters, as `getLoudnessParams` returns it.
pub struct Params(pub Vec<(&'static str, Value)>);

impl WriteParcelable for Params {
    fn write_to(&self, p: &mut Parcel) {
        bundle::write(p, &self.0);
    }
}

/// A registered dispatcher: its binder, death link and registering pid.
type Dispatcher = (Arc<Strong>, u64, i32);

/// `LoudnessCodecHelper`.
#[derive(Default)]
pub struct Loudness {
    state: Mutex<State>,
    /// `mLoudnessUpdateDispatchers`, by handle: the binder, its death link
    /// and the pid that registered it.
    dispatchers: Mutex<HashMap<u32, Dispatcher>>,
}

impl Loudness {
    /// `registerLoudnessCodecUpdatesDispatcher`: kept until unregistered
    /// or dead; its process's sessions stop with its death.
    pub fn register(self: &Arc<Self>, host: &AppHost, handle: u32, pid: i32) {
        let strong = Arc::new(host.process.strong(handle));
        let me: Weak<Self> = Arc::downgrade(self);
        let process = host.process.clone();
        let cookie = host.process.link_to_death(
            &strong,
            Box::new(move || {
                let Some(me) = me.upgrade() else { return };
                let removed = me.dispatchers.lock().unwrap().remove(&handle);
                if let Some((strong, cookie, pid)) = removed {
                    process.clear_death(&strong, cookie);
                    me.state.lock().unwrap().client_died(pid);
                }
            }),
        );
        let old = self
            .dispatchers
            .lock()
            .unwrap()
            .insert(handle, (strong, cookie, pid));
        if let Some((strong, cookie, _)) = old {
            host.process.clear_death(&strong, cookie);
        }
    }

    /// `unregisterLoudnessCodecUpdatesDispatcher`.
    pub fn unregister(&self, host: &AppHost, handle: u32) {
        if let Some((strong, cookie, _)) = self.dispatchers.lock().unwrap().remove(&handle) {
            host.process.clear_death(&strong, cookie);
        }
    }

    /// `startLoudnessCodecUpdates`: the session's players known now.
    pub fn start(&self, host: &AppHost, session: i32, pid: i32) {
        let id = TrackId { session, pid };
        let players = host.playback.lock().unwrap().players();
        let mut state = self.state.lock().unwrap();
        if state.infos.contains_key(&id) {
            eprintln!("app host: audio: Already started loudness updates for {id:?}");
            return;
        }
        state.infos.insert(id, BTreeSet::new());
        let mut piids = BTreeSet::new();
        for player in players
            .iter()
            .filter(|p| p.session == session && p.pid == pid)
        {
            piids.insert(player.piid);
            state.piid_pid.insert(player.piid, pid);
        }
        state.piids.insert(id, piids);
    }

    /// `stopLoudnessCodecUpdates`.
    pub fn stop(&self, session: i32, pid: i32) {
        let id = TrackId { session, pid };
        let mut state = self.state.lock().unwrap();
        if !state.infos.contains_key(&id) {
            eprintln!("app host: audio: Loudness updates are already stopped for {id:?}");
            return;
        }
        for piid in state.piids.remove(&id).unwrap_or_default() {
            state.piid_device.remove(&piid);
            state.piid_pid.remove(&piid);
        }
        state.infos.remove(&id);
    }

    /// `addLoudnessCodecInfo`: the codec's parameters for the device of
    /// the session's first player, else for media's device, sent at once.
    pub fn add(
        &self,
        host: &AppHost,
        session: i32,
        codec_hash: i32,
        info: Option<CodecInfo>,
        pid: i32,
    ) -> Result<(), Exception> {
        let id = TrackId { session, pid };
        {
            let mut state = self.state.lock().unwrap();
            if !state.started(&id) {
                eprintln!("app host: audio: Cannot add new loudness info for stopped {id:?}");
                return Ok(());
            }
            state.infos.get_mut(&id).unwrap().insert(info);
        }
        let players = host.playback.lock().unwrap().players();
        let player = players
            .iter()
            .find(|p| p.session == session && p.pid == pid);
        let params = match player {
            None => Some(self.params(host, info)?),
            Some(player) => match device_of(host, player) {
                Some((kind, connection)) => {
                    self.state
                        .lock()
                        .unwrap()
                        .piids
                        .get_mut(&id)
                        .unwrap()
                        .insert(player.piid);
                    Some(codec_parameters(host, kind, &connection, info)?)
                }
                None => None,
            },
        };
        // An empty bundle of parameters still makes the update non-empty.
        if let Some(params) = params {
            self.dispatch(session, &vec![(codec_hash.to_string(), params)]);
        }
        Ok(())
    }

    /// `removeLoudnessCodecInfo`.
    pub fn remove(&self, session: i32, info: Option<CodecInfo>, pid: i32) {
        let id = TrackId { session, pid };
        let mut state = self.state.lock().unwrap();
        if !state.started(&id) {
            eprintln!("app host: audio: Cannot remove loudness info for stopped {id:?}");
            return;
        }
        if !state.infos.get_mut(&id).unwrap().remove(&info) {
            eprintln!("app host: audio: Could not find to remove codecInfo {info:?}");
        }
    }

    /// `getLoudnessParams`: the codec's parameters for the first device
    /// media plays to; none without one.
    pub fn params(
        &self,
        host: &AppHost,
        info: Option<CodecInfo>,
    ) -> Result<Vec<(&'static str, Value)>, Exception> {
        let device = match host.audio_policy.get() {
            Some(policy) => policy.first_media_device().unwrap_or_else(|e| {
                eprintln!("app host: audio: {e}");
                None
            }),
            None => None,
        };
        match device {
            Some((kind, connection)) => codec_parameters(host, kind, &connection, info),
            None => Ok(Vec::new()),
        }
    }

    /// `updateCodecParameters`: a started session's player that moved to
    /// another device has its session's codecs' parameters sent again.
    pub fn players_changed(&self, host: &AppHost, players: &[PlayerDevice]) {
        let ports = host
            .audio_policy
            .get()
            .map(|p| p.output_device_ports().unwrap_or_default())
            .unwrap_or_default();
        let device = |player: &PlayerDevice| {
            let port = player.device?;
            let port = ports.iter().find(|d| d.id == port)?;
            Some((port.id, port.kind, port.connection.clone()))
        };
        let mut moved = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            for player in players {
                let Some((id, kind, connection)) = device(player) else {
                    state.piid_device.remove(&player.piid);
                    continue;
                };
                if state.piid_device.get(&player.piid) == Some(&id) {
                    continue;
                }
                state.piid_device.insert(player.piid, id);
                let track = TrackId {
                    session: player.session,
                    pid: player.pid,
                };
                if state.started(&track) {
                    state.piids.get_mut(&track).unwrap().insert(player.piid);
                    moved.push((track, kind, connection));
                }
            }
        }
        // `updateCodecParametersForConfiguration`.
        for (track, kind, connection) in moved {
            let infos = self.state.lock().unwrap().infos.get(&track).cloned();
            let mut update = Update::new();
            for info in infos.into_iter().flatten().flatten() {
                match codec_parameters(host, kind, &connection, Some(info)) {
                    Ok(params) => update.push((info.hash_code().to_string(), params)),
                    Err(e) => eprintln!("app host: audio: {}", e.message),
                }
            }
            if !update.is_empty() {
                self.dispatch(track.session, &update);
            }
        }
    }

    /// `dispatchNewLoudnessParameters`, to every dispatcher.
    fn dispatch(&self, session: i32, update: &Update) {
        let mut data = Parcel::new();
        data.write_interface_token(DISPATCHER_DESCRIPTOR);
        data.write_i32(session);
        data.write_i32(1);
        bundle::write(&mut data, &update_bundle(update));
        let dispatchers: Vec<Arc<Strong>> = self
            .dispatchers
            .lock()
            .unwrap()
            .values()
            .map(|(strong, _, _)| strong.clone())
            .collect();
        for dispatcher in dispatchers {
            if let Err(s) =
                dispatcher.transact(DISPATCH_LOUDNESS_CODEC_PARAMETER_CHANGE, &data, true)
            {
                eprintln!("app host: audio: Error dispatching for sessionId {session}: {s}");
            }
        }
    }
}

/// The device a player's configuration plays to (`getAudioDeviceInfo`):
/// its first device's port among the output device ports.
fn device_of(host: &AppHost, player: &PlayerDevice) -> Option<(i32, String)> {
    let port = player.device?;
    let ports = host.audio_policy.get()?.output_device_ports().ok()?;
    ports
        .into_iter()
        .find(|d| d.id == port)
        .map(|d| (d.kind, d.connection))
}

/// `getCodecBundle_l` of a codec on a device.
fn codec_parameters(
    host: &AppHost,
    kind: i32,
    connection: &str,
    info: Option<CodecInfo>,
) -> Result<Vec<(&'static str, Value)>, Exception> {
    let range = spl_range(host, output(kind, connection))?;
    let info = info.ok_or_else(|| Exception::new(EX_NULL_POINTER, "codecInfo"))?;
    Ok(loudness_parameters(range, info))
}

/// `getDeviceSplRange`.
fn spl_range(host: &AppHost, output: Output) -> Result<SplRange, Exception> {
    match output {
        Output::Speaker => {
            if let Some(range) = host
                .property(SPEAKER_SPL_RANGE_SIZE)
                .filter(|r| !r.is_empty() && r != "unknown")
            {
                return Ok(spl_range_of(&range));
            }
            Ok(if automotive_or_television(host)? {
                SplRange::Medium
            } else {
                // A phone's, tablet's or watch's.
                SplRange::Small
            })
        }
        Output::Headphones => Ok(SplRange::Large),
        Output::Bluetooth => Err(host.unimplemented_named(
            "audio",
            "a Bluetooth device's audio device category (#1850)",
        )),
        Output::Other => Ok(SplRange::Unknown),
    }
}

/// `isPlatformAutomotive() || isPlatformTelevision()`: the platform
/// `AudioSystem.getPlatformType` finds, a voice capable device being
/// neither.
fn automotive_or_television(host: &AppHost) -> Result<bool, Exception> {
    let packages = &host.inputs()?.packages;
    if packages
        .framework_bool("config_voice_capable")
        .unwrap_or(false)
    {
        return Ok(false);
    }
    let (state, _) = host.packages()?;
    Ok(state
        .system
        .features
        .iter()
        .any(|(n, v)| (n == FEATURE_LEANBACK || n == FEATURE_AUTOMOTIVE) && *v >= 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(metadata_type: i32, downmixing: bool) -> CodecInfo {
        CodecInfo {
            metadata_type,
            downmixing,
        }
    }

    /// `createLoudnessParameters` for each SPL range and metadata type.
    #[test]
    fn parameters_follow_the_spl_range() {
        let level = |range, info| match loudness_parameters(range, info)[0] {
            (KEY_AAC_DRC_TARGET_REFERENCE_LEVEL, Value::Int(level)) => level,
            _ => unreachable!(),
        };
        assert_eq!(level(SplRange::Large, info(1, false)), 124);
        assert_eq!(level(SplRange::Medium, info(1, false)), 96);
        assert_eq!(level(SplRange::Small, info(1, false)), 64);
        assert_eq!(level(SplRange::Unknown, info(1, false)), 96);
        assert_eq!(
            loudness_parameters(SplRange::Medium, info(CODEC_METADATA_TYPE_MPEG_4, true))[1],
            (KEY_AAC_DRC_HEAVY_COMPRESSION, Value::Int(1))
        );
        assert_eq!(
            loudness_parameters(SplRange::Small, info(CODEC_METADATA_TYPE_MPEG_D, false))[1],
            (KEY_AAC_DRC_EFFECT_TYPE, Value::Int(3))
        );
        assert_eq!(
            loudness_parameters(SplRange::Large, info(CODEC_METADATA_TYPE_MPEG_D, true))[1],
            (KEY_AAC_DRC_EFFECT_TYPE, Value::Int(6))
        );
        // AC-3 and the others get the level alone.
        assert_eq!(
            loudness_parameters(SplRange::Large, info(3, false)).len(),
            1
        );
    }

    #[test]
    fn outputs_are_told_apart_as_the_helper_does() {
        assert_eq!(output(TYPE_OUT_SPEAKER, ""), Output::Speaker);
        assert_eq!(output(TYPE_OUT_SPEAKER, "bt-le"), Output::Bluetooth);
        assert_eq!(output(TYPE_OUT_HEADSET, "usb"), Output::Headphones);
        assert_eq!(output(TYPE_OUT_HEADPHONE, "analog"), Output::Headphones);
        assert_eq!(output(TYPE_OUT_HEADPHONE, "bt-a2dp"), Output::Bluetooth);
        // A USB device (not a headset) and HDMI are of an unknown range.
        assert_eq!(output(133, "usb"), Output::Other);
        assert_eq!(output(133, "hdmi"), Output::Other);
        assert_eq!(spl_range_of("medium"), SplRange::Medium);
        assert_eq!(spl_range_of("huge"), SplRange::Unknown);
    }

    /// A `LoudnessCodecInfo` round trips as the AIDL Java backend writes
    /// it, and hashes as its generated `hashCode` does.
    #[test]
    fn codec_infos_parcel_and_hash_as_the_original() {
        let mut p = Parcel::new();
        info(CODEC_METADATA_TYPE_MPEG_D, true).write_to(&mut p);
        assert_eq!(p.data().len(), 12);
        assert_eq!(
            CodecInfo::read_from(&mut p.reader()).unwrap(),
            info(CODEC_METADATA_TYPE_MPEG_D, true)
        );
        // `Arrays.deepHashCode({2, true})`: 31 * (31 + 2) + 1231.
        assert_eq!(info(CODEC_METADATA_TYPE_MPEG_D, true).hash_code(), 2254);
        assert_eq!(info(CODEC_METADATA_TYPE_MPEG_4, false).hash_code(), 2229);
    }

    /// The dispatched bundle: a nested `PersistableBundle` per codec.
    #[test]
    fn an_update_nests_each_codec_s_bundle() {
        let update = vec![(
            "17".to_string(),
            loudness_parameters(SplRange::Small, info(CODEC_METADATA_TYPE_MPEG_4, false)),
        )];
        let mut p = Parcel::new();
        bundle::write(&mut p, &update_bundle(&update));
        let mut r = p.reader();
        r.read_i32().unwrap(); // length
        r.read_i32().unwrap(); // magic
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("17"));
        assert_eq!(r.read_i32().unwrap(), 25); // VAL_PERSISTABLEBUNDLE
        r.read_i32().unwrap(); // length
        r.read_i32().unwrap(); // magic
        assert_eq!(r.read_i32().unwrap(), 2);
    }

    /// A client's death stops its sessions and forgets its players'
    /// devices.
    #[test]
    fn a_dead_client_s_sessions_stop() {
        let mut state = State::default();
        let mine = TrackId {
            session: 9,
            pid: 40,
        };
        let other = TrackId {
            session: 9,
            pid: 41,
        };
        for id in [mine, other] {
            state.infos.insert(id, BTreeSet::new());
            state.piids.insert(id, BTreeSet::new());
        }
        state.piid_pid.insert(5, 40);
        state.piid_device.insert(5, 2);
        state.piid_device.insert(6, 2);
        state.client_died(40);
        assert!(!state.started(&mine) && state.started(&other));
        assert_eq!(state.piid_device.keys().collect::<Vec<_>>(), [&6]);
    }
}
