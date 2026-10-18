//! `audio`: AudioService for the app host (`AudioService` at
//! `android-16.0.0_r1`): the stream volumes ([`super::volume`]), which
//! audioserver plays at, with the checks AudioService makes of a caller;
//! the sound effects (`SoundEffectsHelper`): each effect's file as the
//! framework's `audio_assets.xml` names it, played by the agent at the
//! effects' attenuation while `Settings.System.SOUND_EFFECTS_ENABLED` is
//! set and the system stream is not muted. Volume hardening blocks nothing: AudioService
//! enables it only on a test's request (`setEnableHardening`).
//!
//! Audio focus ([`super::audio_focus`]) with AudioService's checks of a
//! request: a system usage's permission, a lock only for the call's
//! client with MODIFY_PHONE_STATE (no audio policy registers to lock it),
//! and the focus hardening of an app without MODIFY_AUDIO_SETTINGS_PRIVILEGED
//! or MODIFY_AUDIO_ROUTING (`HardeningEnforcer.blockFocusMethod`): one of
//! an SDK from Vanilla Ice Cream whose TAKE_AUDIO_FOCUS op is not allowed
//! fails; CONTROL_AUDIO, which the original also notes, decides only with
//! all hardening on. An accessibility request forces a duck only for root,
//! as the app host runs no accessibility services.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::{ParcelValue, android_media_iaudioservice as audio};

use super::audio_focus::{self, Focus, FocusInfo, Request};
use super::audio_policy::AudioPolicy;
use super::kept::RawBundle;
use super::playback::{self, Attributes, Playback, PlayerIdCard};
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
/// `AudioSystem.MODE_NORMAL`.
const MODE_NORMAL: i32 = 0;
const REMOTE_AUDIO_PLAYBACK: &str = "android.permission.REMOTE_AUDIO_PLAYBACK";
const CHANGE_ACCESSIBILITY_VOLUME: &str = "android.permission.CHANGE_ACCESSIBILITY_VOLUME";
const BLUETOOTH_PRIVILEGED: &str = "android.permission.BLUETOOTH_PRIVILEGED";
/// The permissions the focus calls check.
const MODIFY_AUDIO_SETTINGS_PRIVILEGED: &str = "android.permission.MODIFY_AUDIO_SETTINGS_PRIVILEGED";
const CALL_AUDIO_INTERCEPTION: &str = "android.permission.CALL_AUDIO_INTERCEPTION";
/// `AudioAttributes.FLAG_CALL_REDIRECTION`, `USAGE_CALL_ASSISTANT` (the
/// one supported system usage), `USAGE_ASSISTANCE_ACCESSIBILITY`.
const FLAG_CALL_REDIRECTION: i32 = 1 << 16;
const USAGE_CALL_ASSISTANT: i32 = 17;
const USAGE_ASSISTANCE_ACCESSIBILITY: i32 = 11;
/// `Build.VERSION_CODES.VANILLA_ICE_CREAM`.
const VANILLA_ICE_CREAM: i32 = 35;
/// `Process.FIRST_APPLICATION_UID`.
const FIRST_APPLICATION_UID: i32 = 10_000;
/// `Settings.System.MULTI_AUDIO_FOCUS_ENABLED`.
const MULTI_AUDIO_FOCUS_ENABLED: &str = "multi_audio_focus_enabled";
/// `IBinder.PING_TRANSACTION`.
const PING_TRANSACTION: u32 = u32::from_be_bytes(*b"_PNG");
/// `AudioService.STREAM_VOLUME_OPS`.
const STREAM_VOLUME_OPS: [i32; 12] = [34, 36, 35, 36, 37, 38, 39, 36, 36, 36, 64, 36];
/// `Intent.FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT`, `FLAG_RECEIVER_FOREGROUND`.
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
const SYSTEM_UID: i32 = 1000;
/// `UserHandle.USER_CURRENT`: user 0, the one user.
const USER_CURRENT: i32 = -2;

/// `AudioManager.FX_*`, by index: the names `audio_assets.xml` uses.
const EFFECTS: [&str; 16] = [
    "FX_KEY_CLICK",
    "FX_FOCUS_NAVIGATION_UP",
    "FX_FOCUS_NAVIGATION_DOWN",
    "FX_FOCUS_NAVIGATION_LEFT",
    "FX_FOCUS_NAVIGATION_RIGHT",
    "FX_KEYPRESS_STANDARD",
    "FX_KEYPRESS_SPACEBAR",
    "FX_KEYPRESS_DELETE",
    "FX_KEYPRESS_RETURN",
    "FX_KEYPRESS_INVALID",
    "FX_BACK",
    "FX_HOME",
    "FX_FOCUS_NAVIGATION_REPEAT_1",
    "FX_FOCUS_NAVIGATION_REPEAT_2",
    "FX_FOCUS_NAVIGATION_REPEAT_3",
    "FX_FOCUS_NAVIGATION_REPEAT_4",
];
const FX_HOME: usize = 11;
/// `SoundEffectsHelper.SOUND_EFFECTS_PATH`, below the product partition,
/// else the system's.
const SOUND_EFFECTS_PATH: &str = "media/audio/ui/";
/// The agent's `playSoundEffect(String path, float gain)`.
const PLAY_SOUND_EFFECT: u32 = 6;

/// `SoundEffectsHelper.loadSoundAssets`: each effect's file (the default
/// `Effect_Tick.ogg`, else as `audio_assets.xml` version 1.0 names it),
/// and whether all navigation repeat effects and the home effect were
/// named, which enables them.
fn sound_assets(xml: Option<&[u8]>) -> ([String; 16], bool, bool) {
    let mut files: [String; 16] = std::array::from_fn(|_| "Effect_Tick.ogg".to_string());
    let mut named = [0; 16];
    if let Some(root) = xml.and_then(|x| aim_apps::res::xml(x).ok())
        && root.name == "audio_assets"
        && matches!(root.named("version"), Some(aim_apps::res::Value::String(v)) if v == "1.0")
    {
        for e in &root.children {
            if e.name != "asset" {
                continue;
            }
            let (Some(aim_apps::res::Value::String(id)), Some(aim_apps::res::Value::String(file))) = (e.named("id"), e.named("file")) else {
                continue;
            };
            if let Some(fx) = EFFECTS.iter().position(|n| n == id) {
                files[fx] = file.clone();
                named[fx] += 1;
            }
        }
    }
    (files, named[12..16].iter().sum::<i32>() == 4, named[FX_HOME] > 0)
}

pub struct AudioService {
    host: Arc<AppHost>,
    navigation_repeat: AtomicBool,
    home_sound: AtomicBool,
    /// The effects' files, read at the first call that needs them.
    effects: std::sync::OnceLock<[String; 16]>,
    /// The `IAudioManagerNative` node audioserver gets.
    native: Binder,
    /// The ringtone player SystemUI sets (`mRingtonePlayer`): none in the
    /// app host until one is set.
    ringtone_player: Mutex<Option<Strong>>,
}

impl AudioService {
    pub fn new(host: Arc<AppHost>) -> Self {
        let native = host.process.add_service(Arc::new(AudioManagerNative(host.clone())));
        AudioService {
            host,
            navigation_repeat: AtomicBool::new(false),
            home_sound: AtomicBool::new(false),
            effects: std::sync::OnceLock::new(),
            native,
            ringtone_player: Mutex::new(None),
        }
    }

    /// The effects' files; loading them sets the navigation repeat and
    /// home effects as `loadSoundAssets` does.
    fn effects(&self) -> Result<&[String; 16], Exception> {
        if let Some(e) = self.effects.get() {
            return Ok(e);
        }
        let xml = self.host.inputs()?.packages.framework_file(self.host.root()?, "xml", "audio_assets");
        let (files, repeat, home) = sound_assets(xml.as_deref());
        if repeat {
            self.navigation_repeat.store(true, Ordering::Relaxed);
        }
        if home {
            self.home_sound.store(true, Ordering::Relaxed);
        }
        Ok(self.effects.get_or_init(|| files))
    }

    /// `playSoundEffectVolume`: nothing while the system stream is muted
    /// or for an unknown effect; `volume` below 0 is the effects'
    /// attenuation (`config_soundEffectVolumeDb`).
    fn play_effect(&self, effect: i32, volume: f32) -> Result<(), Exception> {
        {
            let guard = self.host.volumes()?;
            if guard.as_ref().unwrap().state(volume::STREAM_SYSTEM as usize)?.muted {
                return Ok(());
            }
        }
        let Some(file) = usize::try_from(effect).ok().and_then(|e| self.effects().map(|f| f.get(e).cloned()).transpose()) else {
            return Ok(());
        };
        let file = file?;
        let gain = if volume < 0.0 {
            let db = self.host.inputs()?.packages.framework_integer("config_soundEffectVolumeDb").unwrap_or(0);
            10f32.powf(db as f32 / 20.0)
        } else {
            // As AudioService passes it on, in thousandths.
            (volume * 1000.0) as i32 as f32 / 1000.0
        };
        let root = self.host.root()?;
        let partition = if root.join("product").join(SOUND_EFFECTS_PATH).join(&file).is_file() { "product" } else { "system" };
        let path = format!("/{partition}/{SOUND_EFFECTS_PATH}{file}");
        self.host.call_agent(PLAY_SOUND_EFFECT, "playSoundEffect", |p| {
            p.write_string16(Some(&path));
            p.write_f32(gain);
        })?;
        Ok(())
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

    /// AudioService's start against audioserver: the permission state
    /// (`registerOnStartTask`'s), the volumes (`onSystemReady`,
    /// `onReinitVolumes`), then AudioFlinger's system ready.
    pub(super) fn start_audio(&self) -> Result<(), String> {
        let policy = AudioPolicy::connect(&self.process)?;
        let policy = self.audio_policy.get_or_init(|| policy);
        self.populate_audio_permissions(&policy.permission_controller(&self.process)?)?;
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

    /// Runs `f` on the focus stack and the players, then sends the
    /// players' calls, clears the death links of the requesters released
    /// and schedules the timers asked for.
    fn with_focus<T>(&self, f: impl FnOnce(&mut Focus<Strong, (Strong, u64)>, &mut Playback) -> T) -> T {
        let mut focus = self.focus.lock().unwrap();
        let out = {
            let mut pb = self.playback.lock().unwrap();
            let out = f(&mut focus, &mut pb);
            pb.send();
            out
        };
        self.settle_focus(focus);
        out
    }

    /// What `with_focus` does once the players are unlocked.
    fn settle_focus(&self, mut focus: MutexGuard<'_, Focus<Strong, (Strong, u64)>>) {
        for link in focus.take_released() {
            playback::clear(&self.process, Some(link));
        }
        for timer in focus.take_timers() {
            let me = self.me.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(timer.delay_ms()));
                if let Some(host) = me.upgrade() {
                    host.with_focus(|f, pb| f.fire(timer, pb));
                }
            });
        }
    }

    /// `isValidAudioAttributesUsage`: a system usage is the supported one,
    /// of a caller with MODIFY_AUDIO_ROUTING, or redirecting a call with
    /// CALL_AUDIO_INTERCEPTION.
    fn valid_focus_usage(&self, attributes: &Attributes, uid: i32) -> Result<bool, Exception> {
        if !playback::is_system_usage(attributes.usage) {
            return Ok(true);
        }
        Ok(attributes.usage == USAGE_CALL_ASSISTANT
            && (attributes.flags() & FLAG_CALL_REDIRECTION != 0 && self.granted(CALL_AUDIO_INTERCEPTION, uid)?
                || self.granted(MODIFY_AUDIO_ROUTING, uid)?))
    }

    /// `HardeningEnforcer.blockFocusMethod`: a caller targeting Vanilla
    /// Ice Cream or later fails without TAKE_AUDIO_FOCUS; an empty
    /// package is the uid's first (`getPackNameForUid`).
    fn focus_blocked(&self, uid: i32, package: &str, sdk: i32) -> Result<bool, Exception> {
        let package = if package.is_empty() {
            let (state, resolution) = self.packages()?;
            let query = crate::package::query::Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
            let names = query.packages_for_uid(uid).map_err(|e| Exception::new(aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION, format!("getPackagesForUid: {}", e.0)))?;
            names.into_iter().flatten().flatten().find(|n| !n.is_empty()).unwrap_or_else(|| format!("[{uid}]"))
        } else {
            package.to_string()
        };
        let allowed = super::appops_service::note(self, super::appops_service::OP_TAKE_AUDIO_FOCUS, uid, Some(&package))? == 0;
        Ok(!allowed && sdk >= VANILLA_ICE_CREAM)
    }

    /// `requestAudioFocus`, then `MediaFocusControl`'s.
    fn request_focus(&self, uid: i32, a: audio::RequestAudioFocus<Attributes>) -> Result<i32, Exception> {
        if a.flags & audio_focus::AUDIOFOCUS_FLAG_TEST != 0 {
            return Err(Exception::illegal_argument("Invalid test flag"));
        }
        if let Some(aa) = &a.aa
            && !self.valid_focus_usage(aa, uid)?
        {
            return Ok(audio_focus::AUDIOFOCUS_REQUEST_FAILED);
        }
        if a.flags & audio_focus::AUDIOFOCUS_FLAG_LOCK != 0 {
            if a.client_id.as_deref() == Some(audio_focus::IN_VOICE_COMM_FOCUS_ID) {
                if !self.granted(MODIFY_PHONE_STATE, uid)? {
                    return Ok(audio_focus::AUDIOFOCUS_REQUEST_FAILED);
                }
            } else if a.pcb.is_none() {
                return Err(Exception::new(EX_NULL_POINTER, "pcb"));
            } else {
                // Only a registered audio policy locks focus, and none is.
                return Ok(audio_focus::AUDIOFOCUS_REQUEST_FAILED);
            }
        }
        let (Some(package), Some(client_id), Some(attributes)) = (a.calling_package_name, a.client_id, a.aa) else {
            return Ok(audio_focus::AUDIOFOCUS_REQUEST_FAILED);
        };
        let overrides = self.granted(MODIFY_AUDIO_SETTINGS_PRIVILEGED, uid)? || self.granted(MODIFY_AUDIO_ROUTING, uid)? || uid < FIRST_APPLICATION_UID;
        if !overrides && self.focus_blocked(uid, &package, a.sdk)? {
            return Ok(audio_focus::AUDIOFOCUS_REQUEST_FAILED);
        }
        // `forceFocusDuckingForAccessibility`.
        let force_duck = attributes.public_usage() == USAGE_ASSISTANCE_ACCESSIBILITY
            && a.focus_req_type == audio_focus::AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK
            && attributes.force_ducking()
            && uid == 0;
        let handle = |b: Option<Binder>, name: &str| match b {
            None => Ok(None),
            Some(Binder::Handle(h)) => Ok(Some(h)),
            Some(Binder::Local(_)) => Err(self.unimplemented_named("audio", &format!("focus requests with a {name} of the app host's"))),
        };
        let Some(source) = handle(a.cb, "token")? else {
            return Err(Exception::new(EX_NULL_POINTER, "cb"));
        };
        let dispatcher = handle(a.fd, "dispatcher")?.map(|h| self.process.strong(h));
        let cb = self.process.strong(source);
        if cb.transact(PING_TRANSACTION, &Parcel::new(), false).is_err() {
            return Ok(audio_focus::AUDIOFOCUS_REQUEST_FAILED);
        }
        if self.inputs()?.settings.get(Table::System, MULTI_AUDIO_FOCUS_ENABLED).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) != 0 {
            return Err(self.unimplemented_named("audio", "multiple audio focus"));
        }
        let request = Request {
            attributes,
            gain: a.focus_req_type,
            source,
            dispatcher,
            client_id,
            package,
            flags: a.flags,
            sdk: a.sdk,
            force_duck,
            uid,
        };
        let me = self.me.clone();
        let link = || {
            let cookie = self.process.link_to_death(
                &cb,
                Box::new(move || {
                    if let Some(host) = me.upgrade() {
                        host.with_focus(|f, pb| f.died(source, pb));
                    }
                }),
            );
            (cb, cookie)
        };
        Ok(self.with_focus(|f, pb| f.request(request, link, pb)))
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
                // A player that starts while its app is ducked or faded is too.
                pb.send();
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
            audio::REQUEST_AUDIO_FOCUS => {
                let a = audio::RequestAudioFocus::<Attributes>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.request_focus(uid, a) {
                    Ok(result) => audio::write_request_audio_focus_reply(&mut p, result),
                    Err(e) => return reply(Err(e)),
                }
            }
            audio::ABANDON_AUDIO_FOCUS => {
                let a = audio::AbandonAudioFocus::<Attributes>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let valid = match a.aa.as_ref().map(|aa| host.valid_focus_usage(aa, uid)) {
                    Some(Err(e)) => return reply(Err(e)),
                    Some(Ok(valid)) => valid,
                    None => true,
                };
                // Telecom's abandon waits for its mode's reset, of which
                // none is pending: the app host sets no modes (`setMode`).
                let result = match (valid, a.client_id) {
                    (false, _) => audio_focus::AUDIOFOCUS_REQUEST_FAILED,
                    (true, None) => return reply(Err(Exception::new(EX_NULL_POINTER, "clientId"))),
                    (true, Some(client_id)) => host.with_focus(|f, pb| f.abandon(&client_id, pb)),
                };
                audio::write_abandon_audio_focus_reply(&mut p, result);
            }
            audio::UNREGISTER_AUDIO_FOCUS_CLIENT => {
                let a = audio::UnregisterAudioFocusClient::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = host.with_focus(|f, pb| match a.client_id {
                    Some(client_id) => {
                        f.unregister(&client_id, pb);
                        Ok(())
                    }
                    // A null client matches no entry, but compares with them.
                    None if f.is_empty() => Ok(()),
                    None => Err(Exception::new(EX_NULL_POINTER, "clientId")),
                }) {
                    return reply(Err(e));
                }
                audio::write_unregister_audio_focus_client_reply(&mut p);
            }
            audio::GET_CURRENT_AUDIO_FOCUS => {
                audio::GetCurrentAudioFocus::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                audio::write_get_current_audio_focus_reply(&mut p, host.focus.lock().unwrap().current());
            }
            audio::GET_FOCUS_RAMP_TIME_MS => {
                let a = audio::GetFocusRampTimeMs::<Attributes>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(attr) = a.attr else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "attr")));
                };
                audio::write_get_focus_ramp_time_ms_reply(&mut p, audio_focus::ramp_time_ms(&attr));
            }
            audio::GET_FOCUS_DUCKED_UIDS_FOR_TEST => {
                audio::GetFocusDuckedUidsForTest::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.granted(QUERY_AUDIO_STATE, uid) {
                    Ok(true) => {}
                    Ok(false) => return reply(Err(Exception::security(format!("Access denied, requires: {QUERY_AUDIO_STATE}")))),
                    Err(e) => return reply(Err(e)),
                }
                let uids = host.playback.lock().unwrap().ducked_uids().into_iter().map(ParcelValue::Integer).collect();
                audio::write_get_focus_ducked_uids_for_test_reply(&mut p, &Some(uids));
            }
            // `getMode`: the mode owner's (`getAudioModeOwnerHandler`); the
            // app host takes no `setMode`, so no app owns the mode.
            audio::GET_MODE => {
                audio::GetMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                audio::write_get_mode_reply(&mut p, MODE_NORMAL);
            }
            audio::SET_RINGTONE_PLAYER => {
                let a = audio::SetRingtonePlayer::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.granted(REMOTE_AUDIO_PLAYBACK, uid) {
                    Ok(true) => {}
                    Ok(false) => return reply(Err(Exception::security(format!("Access denied, requires: {REMOTE_AUDIO_PLAYBACK}")))),
                    Err(e) => return reply(Err(e)),
                }
                let player = match a.player {
                    Some(Binder::Handle(handle)) => Some(host.process.strong(handle)),
                    None => None,
                    Some(_) => return Err(BAD_VALUE),
                };
                *self.ringtone_player.lock().unwrap() = player;
                audio::write_set_ringtone_player_reply(&mut p);
            }
            audio::GET_RINGTONE_PLAYER => {
                audio::GetRingtonePlayer::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let player = self.ringtone_player.lock().unwrap().as_ref().map(Strong::binder);
                audio::write_get_ringtone_player_reply(&mut p, player);
            }
            audio::GET_FOCUS_STACK => {
                audio::GetFocusStack::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match host.granted(MODIFY_AUDIO_ROUTING, uid) {
                    Ok(true) => {}
                    Ok(false) => return reply(Err(Exception::security(format!("Access denied, requires: {MODIFY_AUDIO_ROUTING}")))),
                    Err(e) => return reply(Err(e)),
                }
                let infos: Vec<Option<FocusInfo>> = host.focus.lock().unwrap().infos();
                audio::write_get_focus_stack_reply(&mut p, Some(&infos));
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
                if let Err(e) = self.effects() {
                    return reply(Err(e));
                }
                audio::write_are_navigation_repeat_sound_effects_enabled_reply(&mut p, self.navigation_repeat.load(Ordering::Relaxed));
            }
            audio::SET_NAVIGATION_REPEAT_SOUND_EFFECTS_ENABLED => {
                let args = audio::SetNavigationRepeatSoundEffectsEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.effects() {
                    return reply(Err(e));
                }
                self.navigation_repeat.store(args.enabled, Ordering::Relaxed);
            }
            audio::IS_HOME_SOUND_EFFECT_ENABLED => {
                audio::IsHomeSoundEffectEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.effects() {
                    return reply(Err(e));
                }
                audio::write_is_home_sound_effect_enabled_reply(&mut p, self.home_sound.load(Ordering::Relaxed));
            }
            audio::SET_HOME_SOUND_EFFECT_ENABLED => {
                let args = audio::SetHomeSoundEffectEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.effects() {
                    return reply(Err(e));
                }
                self.home_sound.store(args.enabled, Ordering::Relaxed);
            }
            // One-way, as the next: `querySoundEffectsEnabled` of the user.
            audio::PLAY_SOUND_EFFECT => {
                let args = audio::PlaySoundEffect::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let enabled = match self.host.inputs() {
                    Ok(inputs) => inputs.settings.get(Table::System, SOUND_EFFECTS_ENABLED).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) != 0,
                    Err(e) => return reply(Err(e)),
                };
                if enabled && matches!(args.user_id, 0 | USER_CURRENT) && let Err(e) = self.play_effect(args.effect_type, -1.0) {
                    eprintln!("app host: sound effect {}: {e:?}", args.effect_type);
                }
            }
            audio::PLAY_SOUND_EFFECT_VOLUME => {
                let args = audio::PlaySoundEffectVolume::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.play_effect(args.effect_type, args.volume) {
                    eprintln!("app host: sound effect {}: {e:?}", args.effect_type);
                }
            }
            code => return reply(Err(self.host.unimplemented("audio", code))),
        }
        Ok(p)
    }
}
