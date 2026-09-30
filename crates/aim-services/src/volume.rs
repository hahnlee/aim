//! The Mac's output volume as Android's media volume (docs/audio.md,
//! "Volume"; D7 of docs/m1-shell.md).
//!
//! The audio HAL's speaker, Android's one output device, is made an
//! absolute-volume device: the platform's mechanism for a sink that owns
//! its volume, as HdmiControlService uses it for a TV or an audio system.
//! A controller of the system uid registers an `IAudioDeviceVolumeDispatcher`
//! for the speaker (`IAudioService.registerDeviceVolumeDispatcherForAbsoluteVolume`).
//! AudioService then plays the speaker at full scale, and hands every
//! change of the media volume (an app, Settings, a key) to the dispatcher,
//! which sets the Mac's output volume. What changes on the Mac (its keys,
//! the menu bar, another app, the choice of output device) comes back as
//! `setStreamVolume` and, for mute, `adjustStreamVolume` of STREAM_MUSIC
//! with `FLAG_ABSOLUTE_VOLUME`, which AudioService does not send back and
//! which shows no volume UI. The registration is made again whenever
//! system_server restarts.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_driver::{Credentials, Device, Driver};
use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader, Result as ParcelResult};
use aim_host_audio::volume::{self as mac, Level};
use aim_service_aidl::{
    ReadParcelable, Returned, WriteParcelable,
    android_media_iaudiodevicevolumedispatcher as dispatcher, android_media_iaudioservice as audio,
    android_os_iservicemanager as sm,
};

use crate::{SYSTEM_SERVER_CONTEXT, SYSTEM_UID};

/// `AudioManager.STREAM_MUSIC`.
const STREAM_MUSIC: i32 = 3;
/// `AudioManager.FLAG_ABSOLUTE_VOLUME`: the change comes from the device.
const FLAG_ABSOLUTE_VOLUME: i32 = 1 << 13;
/// `AudioManager.ADJUST_MUTE`, `ADJUST_UNMUTE`.
const ADJUST_MUTE: i32 = -100;
const ADJUST_UNMUTE: i32 = 100;
/// `AudioManager.DEVICE_VOLUME_BEHAVIOR_ABSOLUTE`.
const BEHAVIOR_ABSOLUTE: i32 = 3;
/// `AudioDeviceAttributes.ROLE_OUTPUT`, `AudioDeviceInfo.TYPE_BUILTIN_SPEAKER`
/// and `AudioSystem.DEVICE_OUT_SPEAKER`: the HAL's output device.
const ROLE_OUTPUT: i32 = 2;
const TYPE_BUILTIN_SPEAKER: i32 = 2;
const DEVICE_OUT_SPEAKER: i32 = 0x2;
/// The dispatcher's volume range: the Mac's volume in thousandths.
const RANGE: i32 = 1000;
/// The package the calls are made as.
const PACKAGE: &str = "android";
/// How often the controller looks for AudioService while it is not there.
const RETRY: Duration = Duration::from_secs(1);

/// `android.media.AudioDeviceAttributes`, without profiles or descriptors.
#[derive(Debug)]
struct AudioDeviceAttributes {
    role: i32,
    kind: i32,
    address: Option<String>,
    name: Option<String>,
    native_type: i32,
}

impl WriteParcelable for AudioDeviceAttributes {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.role);
        p.write_i32(self.kind);
        p.write_string16(self.address.as_deref());
        p.write_string16(self.name.as_deref());
        p.write_i32(self.native_type);
        // `writeParcelableArray` of no AudioProfile, no AudioDescriptor.
        p.write_i32(0);
        p.write_i32(0);
    }
}

impl ReadParcelable for AudioDeviceAttributes {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let device = Self {
            role: r.read_i32()?,
            kind: r.read_i32()?,
            address: r.read_string16()?,
            name: r.read_string16()?,
            native_type: r.read_i32()?,
        };
        // AudioService hands back the device as registered: no profiles,
        // no descriptors (`readParcelableArray`: a count, -1 for null).
        for _ in 0..2 {
            if r.read_i32()? > 0 {
                return Err(BAD_VALUE);
            }
        }
        Ok(device)
    }
}

/// `android.media.VolumeInfo` of a stream (not a volume group).
#[derive(Debug)]
struct VolumeInfo {
    stream_type: i32,
    index: i32,
    min: i32,
    max: i32,
}

impl WriteParcelable for VolumeInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_bool(true); // mUsesStreamType
        p.write_i32(self.stream_type);
        p.write_bool(false); // mHasMuteCommand
        p.write_bool(false); // mIsMuted
        p.write_i32(self.index);
        p.write_i32(self.min);
        p.write_i32(self.max);
    }
}

impl ReadParcelable for VolumeInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let _uses_stream_type = r.read_bool()?;
        let stream_type = r.read_i32()?;
        let _has_mute_command = r.read_bool()?;
        let _muted = r.read_bool()?;
        // A volume group's own parcel follows when it has no stream type;
        // AudioService sends the controller's own VolumeInfo back.
        Ok(Self {
            stream_type,
            index: r.read_i32()?,
            min: r.read_i32()?,
            max: r.read_i32()?,
        })
    }
}

/// What the worker handles, in order.
enum Event {
    /// The Mac's level may have changed.
    Mac,
    /// Android's media volume, as a fraction of its range.
    Android(f32),
}

/// AudioService while registered with it, and STREAM_MUSIC's index range.
struct Registered {
    audio: Arc<Strong>,
    min: i32,
    max: i32,
    /// What Android was last told or told us: the index and mute.
    last: Option<(i32, bool)>,
}

struct Controller {
    process: Arc<LocalProcess>,
    dispatcher: Binder,
    events: Mutex<Sender<Event>>,
    state: Mutex<State>,
    /// Signalled when AudioService dies.
    gone: Condvar,
}

struct State {
    registered: Option<Registered>,
    /// How often AudioService died.
    deaths: u64,
}

/// The dispatcher node.
struct Dispatcher(Weak<Controller>);

/// Starts the controller in the background: once AudioService is
/// published, if the Mac's output has a volume control, it registers (again
/// after system_server restarts) and follows the Mac from then on. CoreAudio
/// is not touched before, so a boot without system_server makes no
/// CoreAudio client.
pub fn start(driver: &Arc<Driver>) {
    let driver = driver.clone();
    let _ = std::thread::Builder::new()
        .name("volume".into())
        .spawn(move || {
            let process = LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid: std::process::id() as i32,
                    euid: SYSTEM_UID,
                    security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
                },
            );
            let (events, received) = channel();
            let controller = Arc::new_cyclic(|this: &Weak<Controller>| Controller {
                dispatcher: process.add_service(Arc::new(Dispatcher(this.clone()))),
                process: process.clone(),
                events: Mutex::new(events),
                state: Mutex::new(State {
                    registered: None,
                    deaths: 0,
                }),
                gone: Condvar::new(),
            });
            process.start();
            controller.follow_audio(received);
        });
}

impl Controller {
    /// Registers with AudioService whenever it is published.
    fn follow_audio(self: &Arc<Self>, events: Receiver<Event>) {
        let mut events = Some(events);
        loop {
            let Some(audio) = self.find("audio") else {
                std::thread::sleep(RETRY);
                continue;
            };
            if let Some(events) = events.take() {
                if mac::level().is_none() {
                    eprintln!("guest-init: volume: the Mac's output has no volume control");
                    return;
                }
                let sender = Mutex::new(self.events.lock().unwrap().clone());
                mac::watch(Box::new(move || {
                    let _ = sender.lock().unwrap().send(Event::Mac);
                }));
                let worker = self.clone();
                let _ = std::thread::Builder::new()
                    .name("volume-mac".into())
                    .spawn(move || worker.serve(events));
            }
            let audio = Arc::new(audio);
            let deaths = self.state.lock().unwrap().deaths;
            let this = Arc::downgrade(self);
            self.process.link_to_death(
                &audio,
                Box::new(move || {
                    if let Some(c) = this.upgrade() {
                        let mut state = c.state.lock().unwrap();
                        state.registered = None;
                        state.deaths += 1;
                        c.gone.notify_all();
                    }
                }),
            );
            match self.register(&audio) {
                Ok(registered) => {
                    self.state.lock().unwrap().registered = Some(registered);
                    eprintln!("guest-init: volume: the speaker follows the Mac's output volume");
                    // The Mac's mute, which the registration does not carry.
                    self.send(Event::Mac);
                }
                // Tried again with the next AudioService.
                Err(e) => eprintln!("guest-init: volume: {e}"),
            }
            let mut state = self.state.lock().unwrap();
            while state.deaths == deaths {
                state = self.gone.wait(state).unwrap();
            }
        }
    }

    /// Makes the speaker an absolute-volume device at the Mac's volume.
    fn register(&self, audio: &Arc<Strong>) -> Result<Registered, String> {
        let mut data = Parcel::new();
        audio::GetStreamMinVolume {
            stream_type: STREAM_MUSIC,
        }
        .write(&mut data);
        let min = call(
            audio,
            audio::GET_STREAM_MIN_VOLUME,
            &data,
            audio::read_get_stream_min_volume_reply,
        )
        .map_err(|e| format!("getStreamMinVolume: {e}"))?;
        let mut data = Parcel::new();
        audio::GetStreamMaxVolume {
            stream_type: STREAM_MUSIC,
        }
        .write(&mut data);
        let max = call(
            audio,
            audio::GET_STREAM_MAX_VOLUME,
            &data,
            audio::read_get_stream_max_volume_reply,
        )
        .map_err(|e| format!("getStreamMaxVolume: {e}"))?;
        let level = mac::level().ok_or("the Mac's output has no volume control")?;
        let mut data = Parcel::new();
        audio::RegisterDeviceVolumeDispatcherForAbsoluteVolume {
            register: true,
            cb: Some(self.dispatcher),
            package_name: Some(PACKAGE.into()),
            device: Some(speaker()),
            volumes: Some(vec![Some(VolumeInfo {
                stream_type: STREAM_MUSIC,
                index: (level.volume * RANGE as f32).round() as i32,
                min: 0,
                max: RANGE,
            })]),
            handlesvolume_adjustment: false,
            volume_behavior: BEHAVIOR_ABSOLUTE,
        }
        .write(&mut data);
        call(
            audio,
            audio::REGISTER_DEVICE_VOLUME_DISPATCHER_FOR_ABSOLUTE_VOLUME,
            &data,
            audio::read_register_device_volume_dispatcher_for_absolute_volume_reply,
        )
        .map_err(|e| format!("registerDeviceVolumeDispatcherForAbsoluteVolume: {e}"))?;
        Ok(Registered {
            audio: audio.clone(),
            min,
            max,
            last: None,
        })
    }

    fn send(&self, event: Event) {
        let _ = self.events.lock().unwrap().send(event);
    }

    /// Applies each change to the other side, one at a time: CoreAudio is
    /// called only here, never on a binder thread.
    fn serve(&self, events: Receiver<Event>) {
        while let Ok(event) = events.recv() {
            match event {
                Event::Android(volume) => {
                    mac::set_volume(volume);
                    // Raising the volume unmutes, as the Mac's keys do.
                    if volume > 0.0 && mac::level().is_some_and(|l| l.muted) {
                        mac::set_muted(false);
                    }
                }
                Event::Mac => {
                    if let Some(level) = mac::level() {
                        self.tell_android(level);
                    }
                }
            }
        }
    }

    /// Sets STREAM_MUSIC to the Mac's level, as the device's own change.
    fn tell_android(&self, level: Level) {
        let (audio, index, muted, last) = {
            let mut state = self.state.lock().unwrap();
            let Some(r) = state.registered.as_mut() else {
                return;
            };
            let index = r.min + (level.volume * (r.max - r.min) as f32).round() as i32;
            let last = r.last.replace((index, level.muted));
            (r.audio.clone(), index, level.muted, last)
        };
        if last.is_none_or(|(i, _)| i != index) {
            let mut data = Parcel::new();
            audio::SetStreamVolumeWithAttribution {
                stream_type: STREAM_MUSIC,
                index,
                flags: FLAG_ABSOLUTE_VOLUME,
                calling_package: Some(PACKAGE.into()),
                attribution_tag: None,
            }
            .write(&mut data);
            if let Err(e) = call(
                &audio,
                audio::SET_STREAM_VOLUME_WITH_ATTRIBUTION,
                &data,
                audio::read_set_stream_volume_with_attribution_reply,
            ) {
                eprintln!("guest-init: volume: setStreamVolume: {e}");
            }
        }
        if last.is_none_or(|(_, m)| m != muted) {
            let mut data = Parcel::new();
            audio::AdjustStreamVolumeWithAttribution {
                stream_type: STREAM_MUSIC,
                direction: if muted { ADJUST_MUTE } else { ADJUST_UNMUTE },
                flags: FLAG_ABSOLUTE_VOLUME,
                calling_package: Some(PACKAGE.into()),
                attribution_tag: None,
            }
            .write(&mut data);
            if let Err(e) = call(
                &audio,
                audio::ADJUST_STREAM_VOLUME_WITH_ATTRIBUTION,
                &data,
                audio::read_adjust_stream_volume_with_attribution_reply,
            ) {
                eprintln!("guest-init: volume: adjustStreamVolume: {e}");
            }
        }
    }

    /// AudioService changed the media volume: `index` of 0 to [`RANGE`].
    fn android_changed(&self, index: i32) {
        {
            let mut state = self.state.lock().unwrap();
            let Some(r) = state.registered.as_mut() else {
                return;
            };
            // What Android now has, so the Mac's echo of it is not sent
            // back: the index, and unmuted unless it is 0.
            let android = r.min + (index * (r.max - r.min) + RANGE / 2) / RANGE;
            let muted = index == 0 && r.last.is_some_and(|(_, m)| m);
            r.last = Some((android, muted));
        }
        self.send(Event::Android(index.clamp(0, RANGE) as f32 / RANGE as f32));
    }

    /// `name` from servicemanager.
    fn find(&self, name: &str) -> Option<Strong> {
        let mut data = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut data);
        let reply = self
            .process
            .transact(0, sm::CHECK_SERVICE, &data, false)
            .ok()?;
        let Ok(Ok(Some(Binder::Handle(h)))) = sm::read_check_service_reply(&mut reply.reader())
        else {
            return None;
        };
        Some(self.process.strong(h))
    }
}

/// The HAL's output device, as AudioService names it.
fn speaker() -> AudioDeviceAttributes {
    AudioDeviceAttributes {
        role: ROLE_OUTPUT,
        kind: TYPE_BUILTIN_SPEAKER,
        address: Some(String::new()),
        name: Some(String::new()),
        native_type: DEVICE_OUT_SPEAKER,
    }
}

/// A call to AudioService and its reply.
fn call<T>(
    audio: &Strong,
    code: u32,
    data: &Parcel,
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<Returned<T>>,
) -> Result<T, String> {
    let reply = audio
        .transact(code, data, false)
        .map_err(|s| format!("status {s}"))?;
    match read(&mut reply.reader()) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.message),
        Err(s) => Err(format!("status {s}")),
    }
}

impl Service for Dispatcher {
    fn descriptor(&self) -> &str {
        dispatcher::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // Only AudioService, in system_server, calls it.
        if call.sender_euid != SYSTEM_UID {
            return Ok(Parcel::new());
        }
        let Some(controller) = self.0.upgrade() else {
            return Ok(Parcel::new());
        };
        let volume = match call.code {
            dispatcher::DISPATCH_DEVICE_VOLUME_CHANGED => {
                dispatcher::DispatchDeviceVolumeChanged::<AudioDeviceAttributes, VolumeInfo>::read(
                    &mut call.data,
                )?
                .vol
            }
            // Only with `handlesVolumeAdjustment`, which the registration
            // does not ask for.
            dispatcher::DISPATCH_DEVICE_VOLUME_ADJUSTED => None,
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        if let Some(v) = volume.filter(|v| v.stream_type == STREAM_MUSIC && v.max > v.min) {
            controller.android_changed((v.index - v.min) * RANGE / (v.max - v.min));
        }
        Ok(Parcel::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layouts of `VolumeInfo.writeToParcel` and
    /// `AudioDeviceAttributes.writeToParcel`, read back as AudioService
    /// hands them to the dispatcher.
    #[test]
    fn parcels_as_java_writes_them() {
        let mut p = Parcel::new();
        dispatcher::DispatchDeviceVolumeChanged {
            device: Some(speaker()),
            vol: Some(VolumeInfo {
                stream_type: STREAM_MUSIC,
                index: 250,
                min: 0,
                max: RANGE,
            }),
        }
        .write(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        let call =
            dispatcher::DispatchDeviceVolumeChanged::<AudioDeviceAttributes, VolumeInfo>::read(
                &mut r,
            )
            .unwrap();
        let device = call.device.unwrap();
        assert_eq!(
            (device.role, device.kind, device.native_type),
            (ROLE_OUTPUT, TYPE_BUILTIN_SPEAKER, DEVICE_OUT_SPEAKER)
        );
        let vol = call.vol.unwrap();
        assert_eq!(
            (vol.stream_type, vol.index, vol.min, vol.max),
            (STREAM_MUSIC, 250, 0, RANGE)
        );
        assert_eq!(r.remaining(), 0);
        // mUsesStreamType, mStreamType, mHasMuteCommand, mIsMuted, then
        // the index and its range: seven words after the non-null marker.
        let mut v = Parcel::new();
        VolumeInfo {
            stream_type: STREAM_MUSIC,
            index: 1,
            min: 0,
            max: 2,
        }
        .write_to(&mut v);
        let words: Vec<i32> = v
            .data()
            .chunks(4)
            .map(|w| i32::from_ne_bytes(w.try_into().unwrap()))
            .collect();
        assert_eq!(words, [1, STREAM_MUSIC, 0, 0, 1, 0, 2]);
    }
}
