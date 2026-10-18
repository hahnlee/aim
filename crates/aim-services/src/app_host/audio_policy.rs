//! audioserver's side of the stream volumes: the `IAudioPolicyService` and
//! `IAudioFlingerService` calls AudioService makes through `AudioSystem`
//! (`media/libaudioclient/aidl` at `android-16.0.0_r1`), in the
//! interfaces' order.

use std::time::{Duration, Instant};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::Parcel;

const POLICY: &str = "media.audio_policy";
const POLICY_DESCRIPTOR: &str = "android.media.IAudioPolicyService";
const SET_DEVICE_ABSOLUTE_VOLUME_ENABLED: u32 = 17;
const INIT_STREAM_VOLUME: u32 = 18;
const SET_STREAM_VOLUME_INDEX: u32 = 19;
const GET_PERMISSION_CONTROLLER: u32 = 104;
const FLINGER: &str = "media.audio_flinger";
const FLINGER_DESCRIPTOR: &str = "android.media.IAudioFlingerService";
const NEW_AUDIO_UNIQUE_ID: u32 = 34;
/// One-way.
const SYSTEM_READY: u32 = 53;
/// `AudioUniqueIdUse.CLIENT`: the ids of players and recorders.
const USE_CLIENT: i32 = 7;
/// How long audioserver may take to register: its HAL waits up to 3 s for
/// the Mac's devices (docs/audio.md, "Start-up").
const PATIENCE: Duration = Duration::from_secs(30);

/// `AudioSystem.DEVICE_OUT_SPEAKER`, `DEVICE_OUT_DEFAULT`.
pub const DEVICE_OUT_SPEAKER: i32 = 0x2;
pub const DEVICE_OUT_DEFAULT: i32 = 0x4000_0000;
/// `AudioDeviceType.OUT_SPEAKER`, `OUT_DEFAULT`.
const TYPE_OUT_SPEAKER: i32 = 140;
const TYPE_OUT_DEFAULT: i32 = 129;

pub struct AudioPolicy {
    policy: Strong,
    flinger: Strong,
}

/// A structured parcelable as the AIDL backends write it: its size
/// first, then its fields.
fn structured(p: &mut Parcel, fields: impl FnOnce(&mut Parcel)) {
    let start = p.position();
    p.write_i32(0);
    fields(p);
    p.set_i32_at(start, (p.position() - start) as i32);
}

/// `AudioDeviceDescription` of a legacy output device (the legacy
/// `audio_devices_t` to AIDL conversion), as a non-null parcelable.
fn device_description(p: &mut Parcel, device: i32) {
    let kind = match device {
        DEVICE_OUT_SPEAKER => TYPE_OUT_SPEAKER,
        DEVICE_OUT_DEFAULT => TYPE_OUT_DEFAULT,
        other => unreachable!("output device {other:#x} is not the HAL's"),
    };
    p.write_i32(1);
    structured(p, |p| {
        p.write_i32(kind);
        p.write_string16(Some("")); // no connection
    });
}

impl AudioPolicy {
    /// audioserver's services, once registered.
    pub fn connect(process: &LocalProcess) -> Result<AudioPolicy, String> {
        let deadline = Instant::now() + PATIENCE;
        let service = |name: &str| loop {
            if let Ok(s) = super::display::check_service(process, name) {
                return Ok(s);
            }
            if Instant::now() > deadline {
                return Err(format!("{name} did not register"));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        Ok(AudioPolicy { policy: service(POLICY)?, flinger: service(FLINGER)? })
    }

    fn call(&self, code: u32, name: &str, write: impl FnOnce(&mut Parcel)) -> Result<(), String> {
        let mut data = Parcel::new();
        data.write_interface_token(POLICY_DESCRIPTOR);
        write(&mut data);
        let reply = self.policy.transact(code, &data, false).map_err(|s| format!("{name}: {s}"))?;
        match reply.reader().read_exception() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("{name}: {e:?}")),
            Err(s) => Err(format!("{name}: {s}")),
        }
    }

    /// `AudioSystem.initStreamVolume`.
    pub fn init_stream_volume(&self, stream: i32, min: i32, max: i32) -> Result<(), String> {
        self.call(INIT_STREAM_VOLUME, "initStreamVolume", |p| {
            p.write_i32(stream);
            p.write_i32(min);
            p.write_i32(max);
        })
    }

    /// `AudioSystem.setStreamVolumeIndexAS`.
    pub fn set_stream_volume_index(&self, stream: i32, device: i32, index: i32, muted: bool) -> Result<(), String> {
        self.call(SET_STREAM_VOLUME_INDEX, "setStreamVolumeIndex", |p| {
            p.write_i32(stream);
            device_description(p, device);
            p.write_i32(index);
            p.write_bool(muted);
        })
    }

    /// `AudioSystem.setDeviceAbsoluteVolumeEnabled` of `device` at no
    /// address.
    pub fn set_device_absolute_volume_enabled(&self, device: i32, enabled: bool, stream: i32) -> Result<(), String> {
        self.call(SET_DEVICE_ABSOLUTE_VOLUME_ENABLED, "setDeviceAbsoluteVolumeEnabled", |p| {
            p.write_i32(1);
            structured(p, |p| {
                device_description(p, device);
                // `AudioDeviceAddress.id("")`.
                p.write_i32(1);
                p.write_i32(0);
                p.write_string16(Some(""));
            });
            p.write_bool(enabled);
            p.write_i32(stream);
        })
    }

    /// `getPermissionController`: audioserver's native permission
    /// controller.
    pub fn permission_controller(&self, process: &LocalProcess) -> Result<Strong, String> {
        let mut data = Parcel::new();
        data.write_interface_token(POLICY_DESCRIPTOR);
        let reply = self.policy.transact(GET_PERMISSION_CONTROLLER, &data, false).map_err(|s| format!("getPermissionController: {s}"))?;
        let mut r = reply.reader();
        match r.read_exception() {
            Ok(Ok(())) => match r.read_binder() {
                Ok(Some(aim_binder_host::parcel::Binder::Handle(handle))) => Ok(process.strong(handle)),
                Ok(_) => Err("getPermissionController: no controller".into()),
                Err(s) => Err(format!("getPermissionController: {s}")),
            },
            Ok(Err(e)) => Err(format!("getPermissionController: {e:?}")),
            Err(s) => Err(format!("getPermissionController: {s}")),
        }
    }

    /// `AudioSystem.newAudioPlayerId`, `newAudioRecorderId`: an id of
    /// AudioFlinger's for a client's player or recorder.
    pub fn new_client_id(&self) -> Result<i32, String> {
        let mut data = Parcel::new();
        data.write_interface_token(FLINGER_DESCRIPTOR);
        data.write_i32(USE_CLIENT);
        let reply = self.flinger.transact(NEW_AUDIO_UNIQUE_ID, &data, false).map_err(|s| format!("newAudioUniqueId: {s}"))?;
        let mut r = reply.reader();
        match r.read_exception() {
            Ok(Ok(())) => r.read_i32().map_err(|s| format!("newAudioUniqueId: {s}")),
            Ok(Err(e)) => Err(format!("newAudioUniqueId: {e:?}")),
            Err(s) => Err(format!("newAudioUniqueId: {s}")),
        }
    }

    /// `AudioSystem.systemReady`.
    pub fn system_ready(&self) -> Result<(), String> {
        let mut data = Parcel::new();
        data.write_interface_token(FLINGER_DESCRIPTOR);
        self.flinger.transact(SYSTEM_READY, &data, true).map(|_| ()).map_err(|s| format!("systemReady: {s}"))
    }
}
