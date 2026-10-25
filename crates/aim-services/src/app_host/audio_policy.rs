//! audioserver's side of the stream volumes: the `IAudioPolicyService` and
//! `IAudioFlingerService` calls AudioService makes through `AudioSystem`
//! (`media/libaudioclient/aidl` at `android-16.0.0_r1`), in the
//! interfaces' order.

use std::time::{Duration, Instant};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Parcel, Reader, Result as ParcelResult};

use super::display::{parcelable, present};

const POLICY: &str = "media.audio_policy";
const POLICY_DESCRIPTOR: &str = "android.media.IAudioPolicyService";
const SET_DEVICE_ABSOLUTE_VOLUME_ENABLED: u32 = 17;
const INIT_STREAM_VOLUME: u32 = 18;
const GET_DEVICES_FOR_ATTRIBUTES: u32 = 26;
const SET_STREAM_VOLUME_INDEX: u32 = 19;
const LIST_AUDIO_PORTS: u32 = 44;
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
pub(super) const PATIENCE: Duration = Duration::from_secs(30);

/// `AudioSystem.DEVICE_OUT_SPEAKER`, `DEVICE_OUT_DEFAULT`.
pub const DEVICE_OUT_SPEAKER: i32 = 0x2;
pub const DEVICE_OUT_DEFAULT: i32 = 0x4000_0000;
/// `AudioDeviceType.OUT_SPEAKER`, `OUT_DEFAULT`.
const TYPE_OUT_SPEAKER: i32 = 140;
const TYPE_OUT_DEFAULT: i32 = 129;
/// `AudioPortRole.SINK`, `AudioPortType.DEVICE`.
const ROLE_SINK: i32 = 2;
const PORT_TYPE_DEVICE: i32 = 1;
/// `AudioContentType.MUSIC`, `AudioUsage.MEDIA`,
/// `AudioSource.SYS_RESERVED_INVALID`: the AIDL of the attributes
/// `USAGE_MEDIA` and `CONTENT_TYPE_MUSIC` build.
const CONTENT_TYPE_MUSIC: i32 = 2;
const USAGE_MEDIA: i32 = 1;
const SOURCE_INVALID: i32 = -1;
/// `AudioPortExt`'s tag of `device`.
const EXT_DEVICE: i32 = 1;
/// `MAX_PORT_GENERATION_SYNC_ATTEMPTS` of `android_media_AudioSystem.cpp`.
const PORT_GENERATION_SYNC_ATTEMPTS: usize = 5;

/// A device port: its id and its `AudioDeviceDescription`'s type and
/// connection.
#[derive(Debug, PartialEq)]
pub struct DevicePort {
    pub id: i32,
    pub kind: i32,
    pub connection: String,
}

/// `AudioDeviceBroker.VALID_COMMUNICATION_DEVICE_TYPES`, as the
/// descriptions of the legacy devices of each (`getAudioDevicePairs`):
/// `AudioDeviceType` and connection.
const COMMUNICATION_DEVICES: [(i32, &str); 16] = [
    (TYPE_OUT_SPEAKER, ""),        // BUILTIN_SPEAKER
    (OUT_DEVICE, "bt-sco"),        // BLUETOOTH_SCO
    (OUT_HEADSET, "bt-sco"),       // BLUETOOTH_SCO (headset)
    (OUT_CARKIT, "bt-sco"),        // BLUETOOTH_SCO (car kit)
    (OUT_HEADSET, "analog"),       // WIRED_HEADSET
    (OUT_HEADSET, "usb"),          // USB_HEADSET
    (OUT_SPEAKER_EARPIECE, ""),    // BUILTIN_EARPIECE
    (OUT_HEADPHONE, "analog"),     // WIRED_HEADPHONES
    (OUT_HEARING_AID, "wireless"), // HEARING_AID
    (OUT_HEADSET, "bt-le"),        // BLE_HEADSET
    (OUT_DEVICE, "usb"),           // USB_DEVICE
    (TYPE_OUT_SPEAKER, "bt-le"),   // BLE_SPEAKER
    (OUT_DEVICE, "analog"),        // LINE_ANALOG
    (OUT_DEVICE, "hdmi"),          // HDMI
    (OUT_LINE_AUX, ""),            // AUX_LINE
    (OUT_DEVICE, ""),              // BUS (`OUT_BUS`)
];
/// The `AudioDeviceType`s of the communication devices.
const OUT_DEVICE: i32 = 133;
const OUT_CARKIT: i32 = 132;
const OUT_HEADPHONE: i32 = 136;
const OUT_HEADSET: i32 = 137;
const OUT_HEARING_AID: i32 = 138;
const OUT_LINE_AUX: i32 = 139;
const OUT_SPEAKER_EARPIECE: i32 = 141;

impl DevicePort {
    /// `AudioDeviceBroker.isValidCommunicationDevice` of an output port.
    pub fn is_communication_device(&self) -> bool {
        COMMUNICATION_DEVICES
            .iter()
            .any(|&(kind, connection)| kind == self.kind && connection == self.connection)
    }
}

/// An array of non-null structured parcelables, skipped.
fn skip_parcelables(r: &mut Reader<'_>) -> ParcelResult<()> {
    for _ in 0..r.read_i32()?.max(0) {
        present(r)?;
        parcelable(r, |_| Ok(()))?;
    }
    Ok(())
}

/// A union's non-null marker and tag, its value of an `int`.
fn skip_int_union(r: &mut Reader<'_>) -> ParcelResult<()> {
    present(r)?;
    r.read_i32()?;
    r.read_i32()?;
    Ok(())
}

/// An `AudioPortFw` as far as its device: none for a port of another
/// kind.
fn read_device_port(r: &mut Reader<'_>) -> ParcelResult<Option<DevicePort>> {
    present(r)?;
    parcelable(r, |r| {
        present(r)?;
        parcelable(r, |r| {
            let id = r.read_i32()?;
            r.read_string16()?; // name
            skip_parcelables(r)?; // profiles
            skip_int_union(r)?; // flags
            skip_parcelables(r)?; // extraAudioDescriptors
            skip_parcelables(r)?; // gains
            present(r)?;
            if r.read_i32()? != EXT_DEVICE {
                return Ok(None);
            }
            present(r)?;
            parcelable(r, |r| {
                present(r)?;
                parcelable(r, |r| {
                    present(r)?;
                    parcelable(r, |r| {
                        Ok(Some(DevicePort {
                            id,
                            kind: r.read_i32()?,
                            connection: r.read_string16()?.unwrap_or_default(),
                        }))
                    })
                })
            })
        })
    })
}

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
        Ok(AudioPolicy {
            policy: service(POLICY)?,
            flinger: service(FLINGER)?,
        })
    }

    fn call(&self, code: u32, name: &str, write: impl FnOnce(&mut Parcel)) -> Result<(), String> {
        let mut data = Parcel::new();
        data.write_interface_token(POLICY_DESCRIPTOR);
        write(&mut data);
        let reply = self
            .policy
            .transact(code, &data, false)
            .map_err(|s| format!("{name}: {s}"))?;
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
    pub fn set_stream_volume_index(
        &self,
        stream: i32,
        device: i32,
        index: i32,
        muted: bool,
    ) -> Result<(), String> {
        self.call(SET_STREAM_VOLUME_INDEX, "setStreamVolumeIndex", |p| {
            p.write_i32(stream);
            device_description(p, device);
            p.write_i32(index);
            p.write_bool(muted);
        })
    }

    /// `AudioSystem.setDeviceAbsoluteVolumeEnabled` of `device` at no
    /// address.
    pub fn set_device_absolute_volume_enabled(
        &self,
        device: i32,
        enabled: bool,
        stream: i32,
    ) -> Result<(), String> {
        self.call(
            SET_DEVICE_ABSOLUTE_VOLUME_ENABLED,
            "setDeviceAbsoluteVolumeEnabled",
            |p| {
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
            },
        )
    }

    /// `listAudioPorts(SINK, DEVICE)` up to `count` ports: its generation,
    /// the number of ports and the ports.
    fn list_sink_device_ports(&self, count: i32) -> Result<(i32, i32, Vec<DevicePort>), String> {
        let mut data = Parcel::new();
        data.write_interface_token(POLICY_DESCRIPTOR);
        data.write_i32(ROLE_SINK);
        data.write_i32(PORT_TYPE_DEVICE);
        data.write_i32(1);
        structured(&mut data, |p| p.write_i32(count));
        data.write_i32(0); // the out vector's size
        let reply = self
            .policy
            .transact(LIST_AUDIO_PORTS, &data, false)
            .map_err(|s| format!("listAudioPorts: {s}"))?;
        let mut r = reply.reader();
        let read = |r: &mut Reader<'_>| -> ParcelResult<(i32, i32, Vec<DevicePort>)> {
            let generation = r.read_i32()?;
            present(r)?;
            let total = parcelable(r, |r| r.read_i32())?;
            let mut ports = Vec::new();
            for _ in 0..r.read_i32()?.max(0) {
                ports.extend(read_device_port(r)?);
            }
            Ok((generation, total, ports))
        };
        match r.read_exception() {
            Ok(Ok(())) => read(&mut r).map_err(|s| format!("listAudioPorts reply: {s}")),
            Ok(Err(e)) => Err(format!("listAudioPorts: {e:?}")),
            Err(s) => Err(format!("listAudioPorts: {s}")),
        }
    }

    /// The output device ports, as `AudioSystem.listAudioPorts` lists
    /// them: their number, then the ports, again while the generation
    /// changes between the two.
    pub fn output_device_ports(&self) -> Result<Vec<DevicePort>, String> {
        for _ in 0..=PORT_GENERATION_SYNC_ATTEMPTS {
            let (before, total, _) = self.list_sink_device_ports(0)?;
            if total == 0 {
                return Ok(Vec::new());
            }
            let (generation, _, ports) = self.list_sink_device_ports(total)?;
            if generation == before {
                return Ok(ports);
            }
        }
        Err("listAudioPorts: the ports kept changing".into())
    }

    /// `getPermissionController`: audioserver's native permission
    /// controller.
    pub fn permission_controller(&self, process: &LocalProcess) -> Result<Strong, String> {
        let mut data = Parcel::new();
        data.write_interface_token(POLICY_DESCRIPTOR);
        let reply = self
            .policy
            .transact(GET_PERMISSION_CONTROLLER, &data, false)
            .map_err(|s| format!("getPermissionController: {s}"))?;
        let mut r = reply.reader();
        match r.read_exception() {
            Ok(Ok(())) => match r.read_binder() {
                Ok(Some(aim_binder_host::parcel::Binder::Handle(handle))) => {
                    Ok(process.strong(handle))
                }
                Ok(_) => Err("getPermissionController: no controller".into()),
                Err(s) => Err(format!("getPermissionController: {s}")),
            },
            Ok(Err(e)) => Err(format!("getPermissionController: {e:?}")),
            Err(s) => Err(format!("getPermissionController: {s}")),
        }
    }

    /// `AudioSystem.getDevicesForAttributes` of media's attributes
    /// (`USAGE_MEDIA`, `CONTENT_TYPE_MUSIC`), not for volume: the first
    /// device's `AudioDeviceDescription` type and connection, if any.
    pub fn first_media_device(&self) -> Result<Option<(i32, String)>, String> {
        let mut data = Parcel::new();
        data.write_interface_token(POLICY_DESCRIPTOR);
        data.write_i32(1);
        structured(&mut data, |p| {
            p.write_i32(CONTENT_TYPE_MUSIC);
            p.write_i32(USAGE_MEDIA);
            p.write_i32(SOURCE_INVALID);
            p.write_i32(0); // no flags
            p.write_i32(0); // no tags
        });
        data.write_bool(false);
        let reply = self
            .policy
            .transact(GET_DEVICES_FOR_ATTRIBUTES, &data, false)
            .map_err(|s| format!("getDevicesForAttributes: {s}"))?;
        let mut r = reply.reader();
        let read = |r: &mut Reader<'_>| -> ParcelResult<Option<(i32, String)>> {
            if r.read_i32()? <= 0 {
                return Ok(None);
            }
            // `AudioDevice`: its description; its address is skipped.
            present(r)?;
            parcelable(r, |r| {
                present(r)?;
                parcelable(r, |r| {
                    Ok(Some((
                        r.read_i32()?,
                        r.read_string16()?.unwrap_or_default(),
                    )))
                })
            })
        };
        match r.read_exception() {
            Ok(Ok(())) => read(&mut r).map_err(|s| format!("getDevicesForAttributes reply: {s}")),
            Ok(Err(e)) => Err(format!("getDevicesForAttributes: {e:?}")),
            Err(s) => Err(format!("getDevicesForAttributes: {s}")),
        }
    }

    /// `AudioSystem.newAudioPlayerId`, `newAudioRecorderId`: an id of
    /// AudioFlinger's for a client's player or recorder.
    pub fn new_client_id(&self) -> Result<i32, String> {
        let mut data = Parcel::new();
        data.write_interface_token(FLINGER_DESCRIPTOR);
        data.write_i32(USE_CLIENT);
        let reply = self
            .flinger
            .transact(NEW_AUDIO_UNIQUE_ID, &data, false)
            .map_err(|s| format!("newAudioUniqueId: {s}"))?;
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
        self.flinger
            .transact(SYSTEM_READY, &data, true)
            .map(|_| ())
            .map_err(|s| format!("systemReady: {s}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `AudioPortFw` as audioserver writes it: the device's
    /// description is read past the port's arrays and unions, and its
    /// `sys` is skipped.
    #[test]
    fn device_ports_are_read_to_their_description() {
        let mut p = Parcel::new();
        let port = |p: &mut Parcel, ext: i32| {
            p.write_i32(1);
            structured(p, |p| {
                p.write_i32(1);
                structured(p, |p| {
                    p.write_i32(13); // id
                    p.write_string16(Some("Speaker"));
                    p.write_i32(1); // profiles
                    p.write_i32(1);
                    structured(p, |p| p.write_string16(Some("pcm")));
                    p.write_i32(1); // flags: output
                    p.write_i32(1);
                    p.write_i32(0);
                    p.write_i32(0); // extraAudioDescriptors
                    p.write_i32(-1); // gains
                    p.write_i32(1); // ext
                    p.write_i32(ext);
                    if ext == EXT_DEVICE {
                        p.write_i32(1);
                        structured(p, |p| {
                            p.write_i32(1);
                            structured(p, |p| {
                                device_description(p, DEVICE_OUT_SPEAKER);
                                p.write_i32(1); // address
                                p.write_i32(0);
                                p.write_string16(Some(""));
                            });
                            p.write_i32(0); // flags
                        });
                    } else {
                        p.write_i32(5); // session
                    }
                });
                p.write_i32(1); // sys
                structured(p, |p| {
                    p.write_i32(ROLE_SINK);
                    p.write_i32(PORT_TYPE_DEVICE);
                });
            });
        };
        port(&mut p, EXT_DEVICE);
        port(&mut p, 3);
        p.write_i32(42);
        let mut r = p.reader();
        let speaker = read_device_port(&mut r).unwrap().unwrap();
        assert_eq!(
            speaker,
            DevicePort {
                id: 13,
                kind: TYPE_OUT_SPEAKER,
                connection: String::new()
            }
        );
        assert!(speaker.is_communication_device());
        assert_eq!(read_device_port(&mut r).unwrap(), None);
        assert_eq!(r.read_i32().unwrap(), 42);
        assert!(
            !DevicePort {
                id: 1,
                kind: OUT_DEVICE,
                connection: "bt-a2dp".into()
            }
            .is_communication_device()
        );
    }
}
