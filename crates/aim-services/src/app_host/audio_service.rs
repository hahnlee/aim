//! `audio`: AudioService for the app host (`AudioService` at
//! `android-16.0.0_r1`): the stream volumes ([`super::volume`]), which
//! audioserver plays at, with the checks AudioService makes of a caller;
//! whether navigation repeats sound; and the sound effects, which play
//! only while `Settings.System.SOUND_EFFECTS_ENABLED` is set. The app host
//! plays none yet (#1433). Volume hardening blocks nothing: AudioService
//! enables it only on a test's request (`setEnableHardening`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_media_iaudioservice as audio;

use super::audio_policy::AudioPolicy;
use super::kept::RawBundle;
use super::playback::{self, Attributes, PlayerIdCard};
use super::kept::RawIntent;
use super::settings::Table;
use super::volume::{self, Absolute, Config, DeviceAttributes, Effects, VolumeInfo, Volumes};
use super::{AppHost, reply};
use crate::bundle::Value;

/// `Settings.System.SOUND_EFFECTS_ENABLED`.
const SOUND_EFFECTS_ENABLED: &str = "sound_effects_enabled";
/// The permissions the volume calls check.
const MODIFY_AUDIO_SETTINGS: &str = "android.permission.MODIFY_AUDIO_SETTINGS";
const MODIFY_AUDIO_ROUTING: &str = "android.permission.MODIFY_AUDIO_ROUTING";
const MODIFY_PHONE_STATE: &str = "android.permission.MODIFY_PHONE_STATE";
const QUERY_AUDIO_STATE: &str = "android.permission.QUERY_AUDIO_STATE";
const CHANGE_ACCESSIBILITY_VOLUME: &str = "android.permission.CHANGE_ACCESSIBILITY_VOLUME";
const BLUETOOTH_PRIVILEGED: &str = "android.permission.BLUETOOTH_PRIVILEGED";
/// `AudioService.STREAM_VOLUME_OPS`.
const STREAM_VOLUME_OPS: [i32; 12] = [34, 36, 35, 36, 37, 38, 39, 36, 36, 36, 64, 36];
/// `Intent.FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT`, `FLAG_RECEIVER_FOREGROUND`.
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
const SYSTEM_UID: i32 = 1000;

pub struct AudioService {
    host: Arc<AppHost>,
    navigation_repeat: AtomicBool,
    /// The `IAudioManagerNative` node audioserver gets.
    native: Binder,
}

impl AudioService {
    pub fn new(host: Arc<AppHost>) -> Self {
        let native = host.process.add_service(Arc::new(AudioManagerNative(host.clone())));
        AudioService { host, navigation_repeat: AtomicBool::new(false), native }
    }
}

/// `IAudioManagerNative` (`frameworks/av/aidl`), what AudioFlinger tells
/// AudioService, in the interface's order.
struct AudioManagerNative(Arc<AppHost>);

const NATIVE_DESCRIPTOR: &str = "android.media.IAudioManagerNative";
const PLAYBACK_HARDENING_EVENT: u32 = 1;
const PERMISSION_UPDATE_BARRIER: u32 = 2;
const PORT_MUTE_EVENT: u32 = 3;

impl Service for AudioManagerNative {
    fn descriptor(&self) -> &str {
        NATIVE_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            // AudioService's barrier waits for the permission updates it
            // has queued for audioserver; the app host sends none.
            PERMISSION_UPDATE_BARRIER => {
                call.data.enforce_interface(NATIVE_DESCRIPTOR).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
            // One-way: a port's mute, from audioserver.
            PORT_MUTE_EVENT => {
                call.data.enforce_interface(NATIVE_DESCRIPTOR).map_err(|_| BAD_VALUE)?;
                let (port, event) = (call.data.read_i32().map_err(|_| BAD_VALUE)?, call.data.read_i32().map_err(|_| BAD_VALUE)?);
                if super::broadcasts::is_core(call.sender_euid as i32) {
                    let mut pb = self.0.playback.lock().unwrap();
                    if pb.port_mute(port, event) {
                        pb.dispatch(false);
                    }
                }
                Ok(Parcel::new())
            }
            // One-way: AudioService only counts these in its metrics.
            PLAYBACK_HARDENING_EVENT => Ok(Parcel::new()),
            code => reply(Err(self.0.unimplemented("IAudioManagerNative", code))),
        }
    }
}

impl AppHost {
    /// The stream volumes, made at the first call from the image and the
    /// settings.
    fn volumes(&self) -> Result<MutexGuard<'_, Option<Volumes>>, Exception> {
        let mut volumes = self.volumes.lock().unwrap();
        if volumes.is_none() {
            let inputs = self.inputs()?;
            let (state, _) = self.packages()?;
            let configured = self.configured.get().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "app host not configured"))?;
            let flag = |name: &str| state.system.flags.iter().any(|(n, on)| n == name && *on);
            let resource = |name: &str| inputs.packages.framework_bool(name).unwrap_or(false);
            if resource("config_handleVolumeAliasesUsingVolumeGroups") {
                return Err(self.unimplemented_named("audio", "volume aliases by volume groups"));
            }
            let integer = |name: &str| inputs.packages.framework_integer(name);
            let property = |name: &str| (configured.properties)(name).and_then(|v| v.parse().ok());
            let setting = |table: Table, name: &str| inputs.settings.get(table, name).and_then(|v| v.parse().ok());
            *volumes = Some(Volumes::new(&Config {
                // `TelephonyManager.isVoiceCapable`.
                voice_capable: resource("config_voice_capable"),
                single_volume: resource("config_single_volume"),
                fixed_volume: resource("config_useFixedVolume"),
                assistant_volume: resource("config_useAssistantVolume"),
                replace_bt_sco: flag("com.android.media.audio.replace_stream_bt_sco"),
                equal_sco_lea_vc_range: flag("com.android.media.audio.equal_sco_lea_vc_index_range"),
                alarm_min: flag("com.android.media.audio.alarm_min_volume_zero").then(|| integer("config_audio_alarm_min_vol")).flatten(),
                notification: (integer("config_audio_notif_vol_steps"), integer("config_audio_notif_vol_default")),
                ring: (integer("config_audio_ring_vol_steps"), integer("config_audio_ring_vol_default")),
                property: &property,
                setting: &setting,
            }));
        }
        Ok(volumes)
    }

    /// AudioService's start against audioserver (`onSystemReady`,
    /// `onReinitVolumes`): the volumes, then AudioFlinger's system ready.
    pub(super) fn start_audio(&self) -> Result<(), String> {
        let policy = AudioPolicy::connect(&self.process)?;
        let policy = self.audio_policy.get_or_init(|| policy);
        {
            let volumes = self.volumes().map_err(|e| format!("{e:?}"))?;
            volumes.as_ref().unwrap().init(policy)?;
        }
        policy.system_ready()?;
        eprintln!("app host: audio volumes initialized");
        Ok(())
    }

    /// `checkCallingOrSelfPermission`.
    fn granted(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.check_permission(Some(permission), uid)? == 0)
    }

    /// `sendBroadcastToAll` of what a volume change does.
    fn volume_effects(&self, effects: Effects) {
        let flags = FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT | FLAG_RECEIVER_FOREGROUND;
        for (stream, alias, index, old) in effects.volume_changed {
            let intent = RawIntent::new(
                "android.media.VOLUME_CHANGED_ACTION",
                flags,
                &[
                    ("android.media.EXTRA_VOLUME_STREAM_TYPE", Value::Int(stream)),
                    ("android.media.EXTRA_VOLUME_STREAM_VALUE", Value::Int(index)),
                    ("android.media.EXTRA_PREV_VOLUME_STREAM_VALUE", Value::Int(old)),
                    ("android.media.EXTRA_VOLUME_STREAM_TYPE_ALIAS", Value::Int(alias)),
                ],
            );
            self.broadcasts.send_from_system(self, intent);
        }
        for (stream, muted) in effects.mute_changed {
            let intent = RawIntent::new(
                "android.media.STREAM_MUTE_CHANGED_ACTION",
                flags,
                &[
                    ("android.media.EXTRA_STREAM_VOLUME_MUTED", Value::Bool(muted)),
                    ("android.media.EXTRA_VOLUME_STREAM_TYPE", Value::Int(stream)),
                ],
            );
            self.broadcasts.send_from_system(self, intent);
        }
    }

    /// `checkNoteAppOp` of a stream's volume op.
    fn volume_op_allowed(&self, stream: usize, uid: i32, package: Option<&str>) -> Result<bool, Exception> {
        Ok(super::appops_service::note(self, STREAM_VOLUME_OPS[stream], uid, package)? == 0)
    }

    /// `setStreamVolumeWithAttribution`.
    fn set_stream_volume(&self, uid: i32, args: audio::SetStreamVolumeWithAttribution) -> Result<(), Exception> {
        let mut effects = Effects::default();
        {
            let mut guard = self.volumes()?;
            let v = guard.as_mut().unwrap();
            let stream = v.valid_stream(args.stream_type)?;
            if stream as i32 == volume::STREAM_ACCESSIBILITY && !self.granted(CHANGE_ACCESSIBILITY_VOLUME, uid)? {
                return Ok(());
            }
            if stream as i32 == volume::STREAM_VOICE_CALL && args.index == 0 && !self.granted(MODIFY_PHONE_STATE, uid)? {
                return Ok(());
            }
            if stream as i32 == volume::STREAM_ASSISTANT && !self.granted(MODIFY_AUDIO_ROUTING, uid)? {
                return Ok(());
            }
            let alias = v.alias[stream];
            if alias >= 0 && !self.volume_op_allowed(alias as usize, uid, args.calling_package.as_deref())? {
                return Ok(());
            }
            let has_modify = self.granted(MODIFY_AUDIO_SETTINGS, uid)?;
            v.set_stream_volume(self, self.audio_policy.get(), stream, args.index, args.flags, has_modify, &mut effects)?;
            v.take_changes(&mut effects);
        }
        self.volume_effects(effects);
        Ok(())
    }

    /// `adjustStreamVolumeWithAttribution`.
    fn adjust_stream_volume(&self, uid: i32, args: audio::AdjustStreamVolumeWithAttribution) -> Result<(), Exception> {
        let mut effects = Effects::default();
        {
            let mut guard = self.volumes()?;
            let v = guard.as_mut().unwrap();
            let stream = v.valid_stream(args.stream_type)?;
            if stream as i32 == volume::STREAM_ACCESSIBILITY && !self.granted(CHANGE_ACCESSIBILITY_VOLUME, uid)? {
                return Ok(());
            }
            let d = args.direction;
            if !matches!(d, volume::ADJUST_LOWER | volume::ADJUST_RAISE | volume::ADJUST_SAME | volume::ADJUST_MUTE | volume::ADJUST_UNMUTE | volume::ADJUST_TOGGLE_MUTE) {
                return Err(Exception::illegal_argument(format!("Bad direction {d}")));
            }
            let mute_adjust = matches!(d, volume::ADJUST_MUTE | volume::ADJUST_UNMUTE | volume::ADJUST_TOGGLE_MUTE);
            if mute_adjust && stream as i32 == volume::STREAM_VOICE_CALL && !self.granted(MODIFY_PHONE_STATE, uid)? {
                return Ok(());
            }
            if stream as i32 == volume::STREAM_ASSISTANT && !self.granted(MODIFY_AUDIO_ROUTING, uid)? {
                return Ok(());
            }
            let alias = v.alias[stream];
            if alias >= 0 && !self.volume_op_allowed(alias as usize, uid, args.calling_package.as_deref())? {
                return Ok(());
            }
            let has_modify = self.granted(MODIFY_AUDIO_SETTINGS, uid)?;
            v.adjust_stream_volume(self, self.audio_policy.get(), stream, d, args.flags, has_modify, &mut effects)?;
            v.take_changes(&mut effects);
        }
        self.volume_effects(effects);
        Ok(())
    }

    /// `validateAudioAttributesUsage`: a system usage takes
    /// MODIFY_AUDIO_ROUTING, and is one of the supported (the call
    /// assistant's).
    fn validate_usage(&self, attributes: &Attributes, uid: i32) -> Result<(), Exception> {
        if !playback::is_system_usage(attributes.usage) {
            return Ok(());
        }
        if !self.granted(MODIFY_AUDIO_ROUTING, uid)? {
            return Err(Exception::security("Missing MODIFY_AUDIO_ROUTING permission"));
        }
        if attributes.usage != 17 {
            return Err(Exception::illegal_argument(format!("Unsupported usage {}", attributes.usage)));
        }
        Ok(())
    }

    /// `trackPlayer`: the player's id from AudioFlinger, or
    /// `PLAYER_PIID_INVALID` before audioserver is there.
    fn track_player(&self, call: &Call<'_>, card: Option<PlayerIdCard>) -> Result<i32, Exception> {
        let uid = call.sender_euid as i32;
        let Some(card) = card else {
            return Err(Exception::new(EX_NULL_POINTER, "pic"));
        };
        self.validate_usage(&card.attributes, uid)?;
        let Some(policy) = self.audio_policy.get() else {
            return Ok(playback::PLAYER_PIID_INVALID);
        };
        let piid = match policy.new_client_id() {
            Ok(id) if id != 0 => id,
            Ok(_) => return Ok(playback::PLAYER_PIID_INVALID),
            Err(e) => return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, e)),
        };
        // `IPlayerShell.monitorDeath`: a player whose process dies is
        // released.
        let player = card.player.map(|handle| {
            let strong = self.process.strong(handle);
            let me = self.me.clone();
            let cookie = self.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(host) = me.upgrade() {
                        let mut pb = host.playback.lock().unwrap();
                        for piid in pb.players_of(handle) {
                            let (changed, _) = pb.release(piid, 0);
                            if changed {
                                pb.dispatch(true);
                            }
                        }
                    }
                }),
            );
            (strong, cookie)
        });
        self.playback.lock().unwrap().track(piid, card, uid, call.sender_pid, player);
        Ok(piid)
    }

    /// `trackRecorder`.
    fn track_recorder(&self, recorder: Option<Binder>) -> Result<i32, Exception> {
        let Some(Binder::Handle(handle)) = recorder else {
            return Ok(playback::RECORD_RIID_INVALID);
        };
        let Some(policy) = self.audio_policy.get() else {
            return Ok(playback::RECORD_RIID_INVALID);
        };
        let riid = policy.new_client_id().map_err(|e| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, e))?;
        let strong = self.process.strong(handle);
        let me = self.me.clone();
        let cookie = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(host) = me.upgrade() {
                    host.playback.lock().unwrap().release_recorder(riid);
                }
            }),
        );
        self.playback.lock().unwrap().track_recorder(riid, strong, cookie);
        Ok(riid)
    }

    /// `registerDeviceVolumeDispatcherForAbsoluteVolume`.
    fn register_absolute(&self, uid: i32, args: audio::RegisterDeviceVolumeDispatcherForAbsoluteVolume<DeviceAttributes, VolumeInfo>) -> Result<(), Exception> {
        if !self.granted(MODIFY_AUDIO_ROUTING, uid)? && !self.granted(BLUETOOTH_PRIVILEGED, uid)? {
            return Err(Exception::security("Missing MODIFY_AUDIO_ROUTING or BLUETOOTH_PRIVILEGED permissions"));
        }
        let device = args.device.ok_or_else(|| Exception::new(EX_NULL_POINTER, "device"))?;
        let volumes: Vec<VolumeInfo> = args.volumes.ok_or_else(|| Exception::new(EX_NULL_POINTER, "volumes"))?.into_iter().flatten().collect();
        let mut effects = Effects::default();
        {
            let mut guard = self.volumes()?;
            let v = guard.as_mut().unwrap();
            if !args.register {
                if v.absolute.as_ref().is_some_and(|a| a.device.internal_type == device.internal_type) {
                    v.unregister_absolute(self.audio_policy.get());
                }
                return Ok(());
            }
            let Some(Binder::Handle(handle)) = args.cb else {
                return Err(Exception::new(EX_NULL_POINTER, "cb"));
            };
            if device.internal_type != super::audio_policy::DEVICE_OUT_SPEAKER {
                return Err(self.unimplemented_named("audio", &format!("absolute volume of device {:#x}", device.internal_type)));
            }
            let dispatcher = self.process.strong(handle);
            // `AbsoluteVolumeDeviceInfo` unregisters when its dispatcher dies.
            let me = self.me.clone();
            self.process.link_to_death(
                &dispatcher,
                Box::new(move || {
                    if let Some(host) = me.upgrade()
                        && let Some(v) = host.volumes.lock().unwrap().as_mut()
                        && v.absolute.as_ref().is_some_and(|a| a.dispatcher.handle == handle)
                    {
                        v.unregister_absolute(host.audio_policy.get());
                    }
                }),
            );
            v.register_absolute(self.audio_policy.get(), Absolute { device, volumes, dispatcher, handles_adjustment: args.handlesvolume_adjustment });
            v.take_changes(&mut effects);
        }
        self.volume_effects(effects);
        Ok(())
    }
}

impl Service for AudioService {
    fn descriptor(&self) -> &str {
        audio::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let uid = call.sender_euid as i32;
        let host = &self.host;
        // A stream's state read under the volumes' lock.
        let read = |stream: i32, f: &dyn Fn(&Volumes, usize) -> Result<i32, Exception>| -> Result<i32, Exception> {
            let guard = host.volumes()?;
            let v = guard.as_ref().unwrap();
            let stream = v.valid_stream(stream)?;
            f(v, stream)
        };
        match call.code {
            audio::TRACK_PLAYER => {
                let a = audio::TrackPlayer::<PlayerIdCard>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.track_player(call, a.pic) {
                    Ok(piid) => audio::write_track_player_reply(&mut p, piid),
                    Err(e) => return reply(Err(e)),
                }
            }
            // One-way, as the rest of the players' and recorders' events.
            audio::PLAYER_ATTRIBUTES => {
                let a = audio::PlayerAttributes::<Attributes>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(attr) = a.attr
                    && host.validate_usage(&attr, uid).is_ok()
                {
                    let mut pb = host.playback.lock().unwrap();
                    if pb.attributes(a.piid, attr, uid) {
                        pb.dispatch(false);
                    }
                }
            }
            audio::PLAYER_EVENT => {
                let a = audio::PlayerEvent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut pb = host.playback.lock().unwrap();
                if pb.event(a.piid, a.event, &a.event_id.unwrap_or_default(), uid) {
                    pb.dispatch(a.event == 0);
                }
            }
            audio::RELEASE_PLAYER => {
                let a = audio::ReleasePlayer::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut pb = host.playback.lock().unwrap();
                let (changed, link) = pb.release(a.piid, uid);
                playback::clear(&host.process, link);
                if changed {
                    pb.dispatch(true);
                }
            }
            audio::PLAYER_SESSION_ID => {
                let a = audio::PlayerSessionId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // An invalid session throws, which a one-way caller does
                // not hear.
                if a.session_id > 0 {
                    let mut pb = host.playback.lock().unwrap();
                    if pb.session(a.piid, a.session_id, uid) {
                        pb.dispatch(false);
                    }
                }
            }
            audio::PORT_EVENT => {
                let a = audio::PortEvent::<RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if super::broadcasts::is_core(uid) {
                    let extras = a.extras.as_ref().and_then(playback::read_extras);
                    let mut pb = host.playback.lock().unwrap();
                    if pb.port_event(a.port_id, a.event, extras.as_ref()) {
                        pb.dispatch(false);
                    }
                }
            }
            // PlaybackActivityMonitor does nothing with it.
            audio::PLAYER_HAS_OP_PLAY_AUDIO => {
                audio::PlayerHasOpPlayAudio::read(&mut call.data).map_err(|_| BAD_VALUE)?;
            }
            audio::TRACK_RECORDER => {
                let a = audio::TrackRecorder::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.track_recorder(a.recorder) {
                    Ok(riid) => audio::write_track_recorder_reply(&mut p, riid),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::RECORDER_EVENT => {
                let a = audio::RecorderEvent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.playback.lock().unwrap().recorder_event(a.riid, a.event);
            }
            audio::RELEASE_RECORDER => {
                let a = audio::ReleaseRecorder::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let link = host.playback.lock().unwrap().release_recorder(a.riid);
                playback::clear(&host.process, link);
            }
            audio::REGISTER_PLAYBACK_CALLBACK => {
                let a = audio::RegisterPlaybackCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(handle)) = a.pcdb {
                    let privileged = match host.granted(MODIFY_AUDIO_ROUTING, uid) {
                        Ok(v) => v,
                        Err(e) => return reply(Err(e)),
                    };
                    let strong = host.process.strong(handle);
                    let me = host.me.clone();
                    let cookie = host.process.link_to_death(
                        &strong,
                        Box::new(move || {
                            if let Some(host) = me.upgrade() {
                                host.playback.lock().unwrap().remove_client(handle);
                            }
                        }),
                    );
                    host.playback.lock().unwrap().add_client(strong, cookie, privileged);
                }
                audio::write_register_playback_callback_reply(&mut p);
            }
            audio::UNREGISTER_PLAYBACK_CALLBACK => {
                let a = audio::UnregisterPlaybackCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(handle)) = a.pcdb {
                    for link in host.playback.lock().unwrap().remove_client(handle) {
                        playback::clear(&host.process, Some(link));
                    }
                }
            }
            audio::GET_ACTIVE_PLAYBACK_CONFIGURATIONS => {
                audio::GetActivePlaybackConfigurations::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let privileged = match host.granted(MODIFY_AUDIO_ROUTING, uid) {
                    Ok(v) => v,
                    Err(e) => return reply(Err(e)),
                };
                p.write_no_exception();
                host.playback.lock().unwrap().write_configurations(&mut p, privileged);
            }
            audio::GET_NATIVE_INTERFACE => {
                audio::GetNativeInterface::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                p.write_no_exception();
                p.write_binder(Some(self.native));
            }
            audio::GET_STREAM_VOLUME => {
                let args = audio::GetStreamVolume::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // By convention 0 when muted.
                match read(args.stream_type, &|v, s| {
                    let st = v.state(s)?;
                    Ok(if st.muted { 0 } else { (st.index(volume::device_for_stream(s)) + 5) / 10 })
                }) {
                    Ok(index) => audio::write_get_stream_volume_reply(&mut p, index),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::GET_STREAM_MAX_VOLUME => {
                let args = audio::GetStreamMaxVolume::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match read(args.stream_type, &|v, s| Ok((v.state(s)?.max_index() + 5) / 10)) {
                    Ok(max) => audio::write_get_stream_max_volume_reply(&mut p, max),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::GET_STREAM_MIN_VOLUME => {
                let args = audio::GetStreamMinVolume::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let privileged = match host.granted(MODIFY_AUDIO_SETTINGS, uid).and_then(|s| Ok(s || uid == SYSTEM_UID || host.granted(MODIFY_AUDIO_ROUTING, uid)?)) {
                    Ok(privileged) => privileged,
                    Err(e) => return reply(Err(e)),
                };
                match read(args.stream_type, &|v, s| Ok((v.state(s)?.min_index(privileged) + 5) / 10)) {
                    Ok(min) => audio::write_get_stream_min_volume_reply(&mut p, min),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::IS_STREAM_MUTE => {
                let args = audio::IsStreamMute::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match read(args.stream_type, &|v, s| Ok(v.state(s)?.muted as i32)) {
                    Ok(muted) => audio::write_is_stream_mute_reply(&mut p, muted != 0),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::IS_STREAM_AFFECTED_BY_MUTE => {
                let args = audio::IsStreamAffectedByMute::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match read(args.stream_type, &|v, s| Ok(v.affected_by_mute(s) as i32)) {
                    Ok(affected) => audio::write_is_stream_affected_by_mute_reply(&mut p, affected != 0),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::GET_LAST_AUDIBLE_STREAM_VOLUME => {
                let args = audio::GetLastAudibleStreamVolume::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.granted(QUERY_AUDIO_STATE, uid) {
                    Ok(true) => {}
                    Ok(false) => return reply(Err(Exception::security(format!("uid {uid} does not have {QUERY_AUDIO_STATE}.")))),
                    Err(e) => return reply(Err(e)),
                }
                match read(args.stream_type, &|v, s| Ok((v.state(s)?.index(volume::device_for_stream(s)) + 5) / 10)) {
                    Ok(index) => audio::write_get_last_audible_stream_volume_reply(&mut p, index),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::SET_STREAM_VOLUME_WITH_ATTRIBUTION => {
                let args = audio::SetStreamVolumeWithAttribution::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = host.set_stream_volume(uid, args) {
                    return reply(Err(e));
                }
                audio::write_set_stream_volume_with_attribution_reply(&mut p);
            }
            audio::ADJUST_STREAM_VOLUME_WITH_ATTRIBUTION => {
                let args = audio::AdjustStreamVolumeWithAttribution::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = host.adjust_stream_volume(uid, args) {
                    return reply(Err(e));
                }
                audio::write_adjust_stream_volume_with_attribution_reply(&mut p);
            }
            audio::REGISTER_DEVICE_VOLUME_DISPATCHER_FOR_ABSOLUTE_VOLUME => {
                let args = audio::RegisterDeviceVolumeDispatcherForAbsoluteVolume::<DeviceAttributes, VolumeInfo>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = host.register_absolute(uid, args) {
                    return reply(Err(e));
                }
                audio::write_register_device_volume_dispatcher_for_absolute_volume_reply(&mut p);
            }
            audio::ARE_NAVIGATION_REPEAT_SOUND_EFFECTS_ENABLED => {
                audio::AreNavigationRepeatSoundEffectsEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                audio::write_are_navigation_repeat_sound_effects_enabled_reply(&mut p, self.navigation_repeat.load(Ordering::Relaxed));
            }
            audio::SET_NAVIGATION_REPEAT_SOUND_EFFECTS_ENABLED => {
                let args = audio::SetNavigationRepeatSoundEffectsEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.navigation_repeat.store(args.enabled, Ordering::Relaxed);
            }
            audio::PLAY_SOUND_EFFECT => {
                audio::PlaySoundEffect::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let enabled = match self.host.inputs() {
                    Ok(inputs) => inputs.settings.get(Table::System, SOUND_EFFECTS_ENABLED).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) != 0,
                    Err(e) => return reply(Err(e)),
                };
                if enabled {
                    return reply(Err(self.host.unimplemented_named("audio", "playSoundEffect with sound effects enabled")));
                }
            }
            code => return reply(Err(self.host.unimplemented("audio", code))),
        }
        Ok(p)
    }
}
