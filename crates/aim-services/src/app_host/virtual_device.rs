//! Android 16 VirtualDeviceManagerService.VirtualDeviceManagerImpl read paths
//! with an empty mVirtualDevices. No app-host owner creates virtual devices.
//! Source: frameworks/base/services/companion/java/com/android/server/companion/
//! virtual/VirtualDeviceManagerService.java at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::{AppHost, reply};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
use aim_service_aidl::{WriteParcelable, android_companion_virtual_ivirtualdevicemanager as vd};
use std::sync::Arc;

enum NoDevice {}
impl WriteParcelable for NoDevice {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

pub struct VirtualDeviceManager(pub Arc<AppHost>);
impl Service for VirtualDeviceManager {
    fn descriptor(&self) -> &str {
        vd::DESCRIPTOR
    }
    fn transact(&self, c: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match c.code {
            vd::GET_VIRTUAL_DEVICES => {
                vd::GetVirtualDevices::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_get_virtual_devices_reply::<NoDevice>(&mut p, Some(&[]));
            }
            vd::GET_VIRTUAL_DEVICE => {
                vd::GetVirtualDevice::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_get_virtual_device_reply::<NoDevice>(&mut p, None);
            }
            vd::GET_DEVICE_ID_FOR_DISPLAY_ID => {
                vd::GetDeviceIdForDisplayId::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_get_device_id_for_display_id_reply(&mut p, 0);
            }
            vd::GET_DEVICE_POLICY => {
                vd::GetDevicePolicy::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_get_device_policy_reply(&mut p, 0); // DEVICE_POLICY_DEFAULT
            }
            vd::IS_VALID_VIRTUAL_DEVICE_ID => {
                vd::IsValidVirtualDeviceId::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_is_valid_virtual_device_id_reply(&mut p, false);
            }
            vd::GET_AUDIO_PLAYBACK_SESSION_ID => {
                vd::GetAudioPlaybackSessionId::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_get_audio_playback_session_id_reply(&mut p, 0); // AUDIO_SESSION_ID_GENERATE
            }
            vd::GET_AUDIO_RECORDING_SESSION_ID => {
                vd::GetAudioRecordingSessionId::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_get_audio_recording_session_id_reply(&mut p, 0);
            }
            vd::IS_VIRTUAL_DEVICE_OWNED_MIRROR_DISPLAY => {
                vd::IsVirtualDeviceOwnedMirrorDisplay::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                vd::write_is_virtual_device_owned_mirror_display_reply(&mut p, false);
            }
            vd::GET_DISPLAY_NAME_FOR_PERSISTENT_DEVICE_ID | vd::GET_ALL_PERSISTENT_DEVICE_IDS => {
                if c.code == vd::GET_DISPLAY_NAME_FOR_PERSISTENT_DEVICE_ID {
                    vd::GetDisplayNameForPersistentDeviceId::read(&mut c.data)
                        .map_err(|_| BAD_VALUE)?;
                } else {
                    vd::GetAllPersistentDeviceIds::read(&mut c.data).map_err(|_| BAD_VALUE)?;
                }
                // These read CDM associations, independently of live virtual devices.
                if let Err(e) =
                    super::companion_device::CompanionDevice::new(self.0.clone()).no_associations(0)
                {
                    return reply(Err(e));
                }
                if c.code == vd::GET_ALL_PERSISTENT_DEVICE_IDS {
                    vd::write_get_all_persistent_device_ids_reply(&mut p, &Some(vec![]));
                } else {
                    // TextUtils.writeToParcel(null): kind=1, null String8.
                    p.write_no_exception();
                    p.write_i32(1);
                    p.write_string8(None);
                }
            }
            code => {
                return reply(Err(self.0.unimplemented(
                    "virtualdevice",
                    vd::DESCRIPTOR,
                    code,
                )));
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION;

    fn call(s: &dyn Service, code: u32, uid: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
        let mut p = Parcel::new();
        p.write_interface_token(s.descriptor());
        args(&mut p);
        s.transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap()
    }
    fn exception(p: &Parcel) -> i32 {
        p.reader().read_exception().unwrap().unwrap_err().code
    }
    fn int(p: &Parcel) -> i32 {
        let mut r = p.reader();
        r.read_exception().unwrap().unwrap();
        let value = r.read_i32().unwrap();
        assert_eq!(r.remaining(), 0);
        value
    }

    #[test]
    fn virtual_device_reads_match_the_original_empty_inventory() {
        crate::system::with_discovery_binders(|host, _, _, _| {
            let s = VirtualDeviceManager(host.clone());
            for id in [i32::MIN, -1, 0, 1, 42, i32::MAX] {
                for code in [
                    vd::GET_DEVICE_ID_FOR_DISPLAY_ID,
                    vd::GET_DEVICE_POLICY,
                    vd::IS_VALID_VIRTUAL_DEVICE_ID,
                    vd::GET_AUDIO_PLAYBACK_SESSION_ID,
                    vd::GET_AUDIO_RECORDING_SESSION_ID,
                    vd::IS_VIRTUAL_DEVICE_OWNED_MIRROR_DISPLAY,
                    vd::GET_VIRTUAL_DEVICE,
                ] {
                    assert_eq!(
                        int(&call(&s, code, 10123, |p| {
                            p.write_i32(id);
                            if code == vd::GET_DEVICE_POLICY {
                                p.write_i32(id);
                            }
                        })),
                        0
                    );
                }
            }
            for code in [vd::GET_VIRTUAL_DEVICES, vd::GET_ALL_PERSISTENT_DEVICE_IDS] {
                assert_eq!(int(&call(&s, code, 10123, |_| {})), 0);
            }
            let p = call(
                &s,
                vd::GET_DISPLAY_NAME_FOR_PERSISTENT_DEVICE_ID,
                10123,
                |p| p.write_string16(Some("companion:123")),
            );
            let mut r = p.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 1);
            assert!(r.read_string8().unwrap().is_none());
            assert_eq!(r.remaining(), 0);
            assert_eq!(
                exception(&call(&s, vd::CREATE_VIRTUAL_DEVICE, 1000, |_| {})),
                EX_UNSUPPORTED_OPERATION
            );
        });
    }

    #[test]
    fn virtual_device_persistent_reads_guard_both_association_stores() {
        crate::system::with_discovery_binders(|host, _, _, _| {
            let s = VirtualDeviceManager(host.clone());
            for store in [
                "system_de/0/companion_device_manager.xml",
                "system/users/0/companion_device_manager_associations.xml",
            ] {
                let path = host.data().unwrap().join(store);
                assert!(!path.exists());
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "<associations/>\n").unwrap();
                for code in [
                    vd::GET_DISPLAY_NAME_FOR_PERSISTENT_DEVICE_ID,
                    vd::GET_ALL_PERSISTENT_DEVICE_IDS,
                ] {
                    let p = call(&s, code, 10123, |p| {
                        if code == vd::GET_DISPLAY_NAME_FOR_PERSISTENT_DEVICE_ID {
                            p.write_string16(Some("companion:123"));
                        }
                    });
                    let e = p.reader().read_exception().unwrap().unwrap_err();
                    assert_eq!(e.code, EX_UNSUPPORTED_OPERATION);
                    assert!(e.message.contains("persisted associations (#1790)"));
                }
                // Live inventory is independent of persisted companion associations.
                assert_eq!(int(&call(&s, vd::GET_VIRTUAL_DEVICES, 10123, |_| {})), 0);
                std::fs::remove_file(path).unwrap();
                assert_eq!(
                    int(&call(&s, vd::GET_ALL_PERSISTENT_DEVICE_IDS, 10123, |_| {})),
                    0
                );
            }
        });
    }
}
