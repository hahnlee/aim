//! `input`: InputManager for the app host, what an app's process asks of
//! it (`InputManagerService` at `android-16.0.0_r1`): the velocity
//! tracker's strategy, and the input devices. The devices are InputReader's
//! virtual keyboard (`EventHub::createVirtualKeyboardLocked`), whose key
//! character map an app's `KeyCharacterMap.load` falls back to; the
//! touchscreen, keyboard and mouse InputReader would find are not modelled
//! yet (#1434). Its shell command is `input_shell.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_hardware_input_iinputmanager as im;
use sha1::{Digest, Sha1};

use super::keychars::KeyCharacterMap;
use super::{AppHost, reply};

/// `ReservedInputDeviceId::VIRTUAL_KEYBOARD_ID`.
const VIRTUAL_KEYBOARD_ID: i32 = -1;
/// `InputDevice.SOURCE_KEYBOARD | SOURCE_DPAD`; `KEYBOARD_TYPE_ALPHABETIC`;
/// `ADISPLAY_ID_NONE`.
const SOURCE_KEYBOARD_DPAD: i32 = 0x101 | 0x201;
const KEYBOARD_TYPE_ALPHABETIC: i32 = 2;
const DISPLAY_NONE: i32 = -1;
/// The input device configuration directories, in the order
/// `getInputDeviceConfigurationFilePathByName` searches them.
const CONFIGURATION_DIRS: &[&str] = &[
    "product/usr",
    "system_ext/usr",
    "odm/usr",
    "vendor/usr",
    "system/usr",
];

/// `DeviceConfig.NAMESPACE_INPUT_NATIVE_BOOT`,
/// `InputManagerService.VELOCITYTRACKER_STRATEGY_PROPERTY`.
const NAMESPACE_INPUT_NATIVE_BOOT: &str = "input_native_boot";
const VELOCITYTRACKER_STRATEGY: &str = "velocitytracker_strategy";

/// An input device as `InputDevice.writeToParcel` writes it (one without
/// motion ranges, vibrator, battery or the like).
pub struct InputDevice {
    id: i32,
    generation: i32,
    name: String,
    descriptor: String,
    sources: i32,
    keyboard_type: i32,
    key_character_map: KeyCharacterMap,
    associated_display: i32,
}

impl WriteParcelable for InputDevice {
    fn write_to(&self, p: &mut Parcel) {
        // `KeyCharacterMap.writeToParcel`: its device, and the map.
        p.write_i32(self.id);
        p.write_bool(true);
        self.key_character_map.write_to(p);
        p.write_i32(self.id);
        p.write_i32(self.generation);
        p.write_i32(0); // controller number
        p.write_string16(Some(&self.name));
        p.write_i32(0); // vendor
        p.write_i32(0); // product
        p.write_i32(0); // bus
        p.write_string16(Some(&self.descriptor));
        p.write_i32(0); // built in
        p.write_i32(self.sources);
        p.write_i32(self.keyboard_type);
        p.write_string8(None); // keyboard language tag
        p.write_string8(None); // keyboard layout type
        (0..5).for_each(|_| p.write_i32(0)); // vibrator, microphone, button under pad, sensor, battery
        p.write_i32(-1); // `HostUsiVersion`: none
        p.write_i32(-1);
        p.write_i32(self.associated_display);
        p.write_i32(1); // enabled
        p.write_i32(0); // no motion ranges
        p.write_bool(false); // no smooth scrolling
    }
}

/// `EventHub`'s `generateDescriptor` of a device without vendor or
/// product: the SHA-1 of its unique id and name.
fn descriptor(unique_id: &str, name: &str) -> String {
    let digest = Sha1::digest(format!(":0000:0000:uniqueId:{unique_id}name:{name}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct InputManager {
    host: Arc<AppHost>,
    virtual_keyboard: OnceLock<Arc<InputDevice>>,
    listeners: Mutex<Vec<Binder>>,
}

impl InputManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        InputManager {
            host,
            virtual_keyboard: OnceLock::new(),
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// The virtual keyboard: `Virtual.kcm` from the image, as
    /// `KeyMap::load` finds it by the device's name.
    fn virtual_keyboard(&self) -> Result<Arc<InputDevice>, Exception> {
        if let Some(device) = self.virtual_keyboard.get() {
            return Ok(device.clone());
        }
        let root = self.host.root()?;
        let (path, contents) = CONFIGURATION_DIRS
            .iter()
            .find_map(|dir| {
                let path = format!("/{dir}/keychars/Virtual.kcm");
                std::fs::read(root.join(&path[1..])).ok().map(|c| (path, c))
            })
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no Virtual.kcm"))?;
        let map = KeyCharacterMap::load(&path, &contents)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        let device = Arc::new(InputDevice {
            id: VIRTUAL_KEYBOARD_ID,
            generation: 1,
            name: "Virtual".into(),
            descriptor: descriptor("<virtual>", "Virtual"),
            sources: SOURCE_KEYBOARD_DPAD,
            keyboard_type: KEYBOARD_TYPE_ALPHABETIC,
            key_character_map: map,
            associated_display: DISPLAY_NONE,
        });
        Ok(self.virtual_keyboard.get_or_init(|| device).clone())
    }
}

impl Service for InputManager {
    fn descriptor(&self) -> &str {
        im::DESCRIPTOR
    }

    /// A shell command's input and outputs.
    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            im::GET_VELOCITY_TRACKER_STRATEGY => {
                im::GetVelocityTrackerStrategy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // The device config's, as the original read it at its start.
                let inputs = match self.host.inputs() {
                    Ok(inputs) => inputs,
                    Err(e) => return reply(Err(e)),
                };
                let strategy = inputs
                    .settings
                    .device_config(NAMESPACE_INPUT_NATIVE_BOOT, VELOCITYTRACKER_STRATEGY);
                im::write_get_velocity_tracker_strategy_reply(&mut p, &strategy);
            }
            im::GET_INPUT_DEVICE_IDS => {
                im::GetInputDeviceIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                im::write_get_input_device_ids_reply(&mut p, &Some(vec![VIRTUAL_KEYBOARD_ID]));
            }
            im::GET_INPUT_DEVICE => {
                let args = im::GetInputDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let device = match args.device_id {
                    VIRTUAL_KEYBOARD_ID => match self.virtual_keyboard() {
                        Ok(device) => Some(device),
                        Err(e) => return reply(Err(e)),
                    },
                    _ => None,
                };
                im::write_get_input_device_reply(&mut p, device.as_deref());
            }
            // The devices do not change: listeners are kept, never called.
            im::REGISTER_INPUT_DEVICES_CHANGED_LISTENER => {
                let args = im::RegisterInputDevicesChangedListener::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.listeners.lock().unwrap().extend(args.listener);
                im::write_register_input_devices_changed_listener_reply(&mut p);
            }
            crate::shell::SHELL_COMMAND_TRANSACTION => {
                let mut command =
                    crate::shell::ShellCommand::read(&self.host.process, &mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                if command.has_output() {
                    let result =
                        super::input_shell::run(&self.host, call.sender_euid as i32, &mut command);
                    command.finish(result);
                }
                p.write_no_exception();
            }
            code => return reply(Err(self.host.unimplemented("input", code))),
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_virtual_keyboard_has_the_original_descriptor() {
        // The pinned image's virtual keyboard (`getInputDevice(-1)`).
        assert_eq!(
            descriptor("<virtual>", "Virtual"),
            "a718a782d34bc767f4689c232d64d527998ea7fd"
        );
    }
}
