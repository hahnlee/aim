//! `input`: InputManager for the app host, what an app's process asks of
//! it (`InputManagerService` at `android-16.0.0_r1`): the velocity
//! tracker's strategy, and the input devices: InputReader's virtual
//! keyboard (`EventHub::createVirtualKeyboardLocked`), whose key character
//! map an app's `KeyCharacterMap.load` falls back to, and the built-in
//! keyboard the app host agent reads (`aim-keyboard`, InputReader's
//! `BUILT_IN_KEYBOARD_ID`), as the agent finds and classifies it, with its
//! key character map (no keyboard layout overlay: the app host has no
//! input method subtypes to pick one by). The touchscreen and the mouse are
//! not modelled yet (#1434). The dispatcher's key repeat follows the
//! secure settings `InputSettingsObserver` watches. Its shell command is
//! `input_shell.rs`.

use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_hardware_input_iinputmanager as im;
use sha1::{Digest, Sha1};

use aim_input_transport::keychars::KeyCharacterMap;

use super::content_service::LocalObserver;
use super::settings::Table;
use super::{AppHost, reply};
use crate::package::uri::Uri;

/// `ReservedInputDeviceId::VIRTUAL_KEYBOARD_ID`.
const VIRTUAL_KEYBOARD_ID: i32 = -1;
const BLUETOOTH: &str = "android.permission.BLUETOOTH";
/// The agent's `getKeyboard()` and `setKeyRepeatConfiguration(long
/// timeoutNanos, long delayNanos, boolean enabled)`.
const GET_KEYBOARD: u32 = 10;
const SET_KEY_REPEAT_CONFIGURATION: u32 = 12;
/// `Settings.Secure.KEY_REPEAT_*`, and `ViewConfiguration`'s defaults
/// (`DEFAULT_KEY_REPEAT_TIMEOUT_MS`, the long press timeout, and
/// `DEFAULT_KEY_REPEAT_DELAY_MS`).
const KEY_REPEAT_ENABLED: &str = "key_repeat_enabled";
const KEY_REPEAT_TIMEOUT_MS: &str = "key_repeat_timeout_ms";
const KEY_REPEAT_DELAY_MS: &str = "key_repeat_delay_ms";
const DEFAULT_KEY_REPEAT_TIMEOUT_MS: i64 = 400;
const DEFAULT_KEY_REPEAT_DELAY_MS: i64 = 50;
/// `com.android.input.flags.keyboard_repeat_keys`: the dispatcher heeds
/// whether keys repeat.
const KEYBOARD_REPEAT_KEYS: &str = "com.android.input.flags.keyboard_repeat_keys";
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
    vendor: i32,
    product: i32,
    bus: i32,
    descriptor: String,
    is_external: bool,
    sources: i32,
    keyboard_type: i32,
    key_character_map: KeyCharacterMap,
    associated_display: i32,
    supported_keys: Option<Vec<i32>>,
}

impl WriteParcelable for InputDevice {
    fn write_to(&self, p: &mut Parcel) {
        // `KeyCharacterMap.writeToParcel`: its device, and the map.
        p.write_i32(self.id);
        p.write_bool(true);
        write_key_character_map(&self.key_character_map, p);
        p.write_i32(self.id);
        p.write_i32(self.generation);
        p.write_i32(0); // controller number
        p.write_string16(Some(&self.name));
        p.write_i32(self.vendor);
        p.write_i32(self.product);
        p.write_i32(self.bus);
        p.write_string16(Some(&self.descriptor));
        p.write_i32(self.is_external as i32);
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

/// `KeyCharacterMap::writeToParcel`.
fn write_key_character_map(map: &KeyCharacterMap, p: &mut Parcel) {
    p.write_string8(Some(&map.load_file_name));
    p.write_i32(map.kind as i32);
    p.write_bool(map.layout_overlay_applied);
    p.write_i32(map.keys.len() as i32);
    for (code, key) in &map.keys {
        p.write_i32(*code);
        p.write_i32(key.label.into());
        p.write_i32(key.number.into());
        for b in &key.behaviors {
            p.write_i32(1);
            p.write_i32(b.meta_state);
            p.write_i32(b.character.into());
            p.write_i32(b.fallback_key_code);
            p.write_i32(b.replacement_key_code);
        }
        p.write_i32(0);
    }
    p.write_i32(0); // no key remapping
    for codes in [&map.keys_by_scan_code, &map.keys_by_usage_code] {
        p.write_i32(codes.len() as i32);
        for (from, to) in codes {
            p.write_i32(*from);
            p.write_i32(*to);
        }
    }
}

/// `EventHub`'s `generateDescriptor`: the SHA-1 of the raw descriptor.
fn sha1_hex(raw: &str) -> String {
    let digest = Sha1::digest(raw.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The virtual keyboard's descriptor, of its unique id and name.
fn descriptor(unique_id: &str, name: &str) -> String {
    sha1_hex(&format!(":0000:0000:uniqueId:{unique_id}name:{name}"))
}

/// The virtual keyboard's key character map: `Virtual.kcm` from the image,
/// as `KeyMap::load` finds it by the device's name.
pub(super) fn virtual_key_character_map(host: &AppHost) -> Result<KeyCharacterMap, Exception> {
    let root = host.root()?;
    let (path, contents) = CONFIGURATION_DIRS
        .iter()
        .find_map(|dir| {
            let path = format!("/{dir}/keychars/Virtual.kcm");
            std::fs::read(root.join(&path[1..])).ok().map(|c| (path, c))
        })
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no Virtual.kcm"))?;
    KeyCharacterMap::load(&path, &contents).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
}

/// The image's or the data volume's file at a guest path.
fn guest_file(host: &AppHost, path: &str) -> Result<Vec<u8>, Exception> {
    let host_path = match path.strip_prefix("/data/") {
        Some(rest) => host.data()?.join(rest),
        None => host.root()?.join(path.trim_start_matches('/')),
    };
    std::fs::read(&host_path).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{path}: {e}")))
}

/// `InputSettingsObserver.updateKeyRepeatInfo`: the key repeat settings,
/// given to the dispatcher (the agent).
pub(super) fn update_key_repeat_info(host: &AppHost) -> Result<(), Exception> {
    let settings = &host.inputs()?.settings;
    let int = |name: &str, default: i64| {
        settings
            .get(Table::Secure, name)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(default)
    };
    let enabled = int(KEY_REPEAT_ENABLED, 1) != 0;
    let timeout = int(KEY_REPEAT_TIMEOUT_MS, DEFAULT_KEY_REPEAT_TIMEOUT_MS);
    let delay = int(KEY_REPEAT_DELAY_MS, DEFAULT_KEY_REPEAT_DELAY_MS);
    // The dispatcher heeds `enabled` with the flag only (`notifyKey`).
    let (state, _) = host.packages()?;
    let flag = state
        .system
        .flags
        .iter()
        .any(|(n, on)| n == KEYBOARD_REPEAT_KEYS && *on);
    host.call_agent(
        SET_KEY_REPEAT_CONFIGURATION,
        "setKeyRepeatConfiguration",
        |p| {
            p.write_i64(timeout * 1_000_000);
            p.write_i64(delay * 1_000_000);
            p.write_bool(enabled || !flag);
        },
    )
    .map(|_| ())
}

/// The key repeat settings' observer (`InputSettingsObserver`).
struct KeyRepeatObserver(std::sync::Weak<AppHost>);

impl LocalObserver for KeyRepeatObserver {
    fn on_change(&self, _self_change: bool, _uris: &[Uri], _flags: i32, _user: i32) {
        if let Some(host) = self.0.upgrade()
            && let Err(e) = update_key_repeat_info(&host)
        {
            eprintln!("app host: input: key repeat settings: {}", e.message);
        }
    }
}

/// `InputSettingsObserver.registerAndUpdate` of the key repeat settings,
/// once the dispatcher (the agent) is there.
pub(super) fn watch_key_repeat(host: &Arc<AppHost>) -> Result<(), Exception> {
    let content = host
        .content_service
        .get()
        .cloned()
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no content service"))?;
    let observer: Arc<dyn LocalObserver> = Arc::new(KeyRepeatObserver(Arc::downgrade(host)));
    for name in [
        KEY_REPEAT_TIMEOUT_MS,
        KEY_REPEAT_DELAY_MS,
        KEY_REPEAT_ENABLED,
    ] {
        let uri = Uri::parse(&format!("content://settings/secure/{name}"));
        content.register_local(&uri, false, 0, observer.clone())?;
    }
    update_key_repeat_info(host)
}

pub struct InputManager {
    host: Arc<AppHost>,
    virtual_keyboard: OnceLock<Arc<InputDevice>>,
    keyboard: OnceLock<Option<Arc<InputDevice>>>,
    listeners: Mutex<Vec<Binder>>,
}

impl InputManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        InputManager {
            host,
            virtual_keyboard: OnceLock::new(),
            keyboard: OnceLock::new(),
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// The virtual keyboard: `Virtual.kcm` from the image, as
    /// `KeyMap::load` finds it by the device's name.
    fn virtual_keyboard(&self) -> Result<Arc<InputDevice>, Exception> {
        if let Some(device) = self.virtual_keyboard.get() {
            return Ok(device.clone());
        }
        let map = virtual_key_character_map(&self.host)?;
        let device = Arc::new(InputDevice {
            id: VIRTUAL_KEYBOARD_ID,
            generation: 1,
            name: "Virtual".into(),
            vendor: 0,
            product: 0,
            bus: 0,
            descriptor: descriptor("<virtual>", "Virtual"),
            is_external: false,
            sources: SOURCE_KEYBOARD_DPAD,
            keyboard_type: KEYBOARD_TYPE_ALPHABETIC,
            key_character_map: map,
            associated_display: DISPLAY_NONE,
            // EventHub's virtual device has no EV_KEY bits. Load its layout
            // to distinguish a successful lookup from a device with no layout.
            supported_keys: virtual_key_layout(&self.host)?.map(|_| Vec::new()),
        });
        Ok(self.virtual_keyboard.get_or_init(|| device).clone())
    }

    /// The built-in keyboard, as the agent (InputReader) found it: none if
    /// it found none. Its devices are the display's, which do not change.
    fn keyboard(&self) -> Result<Option<Arc<InputDevice>>, Exception> {
        if let Some(device) = self.keyboard.get() {
            return Ok(device.clone());
        }
        let reply = self.host.call_agent(GET_KEYBOARD, "getKeyboard", |_| {})?;
        let mut r = reply.reader();
        let bad = |e| Exception::new(EX_ILLEGAL_STATE, format!("getKeyboard: {e}"));
        r.read_exception().map_err(bad)??;
        let device = if r.read_i32().map_err(bad)? == 0 {
            None
        } else {
            let id = r.read_i32().map_err(bad)?;
            let name = r.read_string16().map_err(bad)?.unwrap_or_default();
            let unique_id = r.read_string16().map_err(bad)?.unwrap_or_default();
            let mut int = || r.read_i32().map_err(bad);
            let (bus, vendor, product, version) = (int()?, int()?, int()?, int()?);
            let (sources, keyboard_type, is_external) = (int()?, int()?, int()? != 0);
            let kcm = r.read_string16().map_err(bad)?.unwrap_or_default();
            let supported_keys = aim_service_aidl::read_int_array(&mut r).map_err(bad)?;
            let identifier = aim_input_transport::keyboard::Identifier {
                name: name.clone(),
                bus: bus as u16,
                vendor: vendor as u16,
                product: product as u16,
                version: version as u16,
                unique_id,
            };
            let map = KeyCharacterMap::load(&kcm, &guest_file(&self.host, &kcm)?)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            Some(Arc::new(InputDevice {
                id,
                generation: 2,
                name,
                vendor,
                product,
                bus,
                descriptor: sha1_hex(&identifier.raw_descriptor()),
                is_external,
                sources,
                keyboard_type,
                key_character_map: map,
                associated_display: DISPLAY_NONE,
                supported_keys,
            }))
        };
        Ok(self.keyboard.get_or_init(|| device).clone())
    }
}

fn virtual_key_layout(
    host: &AppHost,
) -> Result<Option<aim_input_transport::keylayout::KeyLayoutMap>, Exception> {
    let root = host.root()?;
    let files = aim_input_transport::keyboard::ConfigFiles {
        prefixes: CONFIGURATION_DIRS
            .iter()
            .map(|p| format!("/{p}/"))
            .collect(),
        readable: &|p| root.join(p.trim_start_matches('/')).is_file(),
    };
    let id = aim_input_transport::keyboard::Identifier {
        name: "Virtual".into(),
        unique_id: "<virtual>".into(),
        bus: 0,
        vendor: 0,
        product: 0,
        version: 0,
    };
    aim_input_transport::keyboard::KeyMap::load(&id, &[], &files, &|p| guest_file(host, p).ok())
        .map(|map| map.layout)
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
}

impl InputManager {
    fn has_keys(
        &self,
        device: i32,
        sources: i32,
        codes: Option<Vec<i32>>,
        count: i32,
    ) -> Result<im::HasKeysReply, Exception> {
        let codes = codes.ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "keyCodes must not be null",
            )
        })?;
        if count < 0 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "keyExists must not be null",
            ));
        }
        if (count as usize) < codes.len() {
            return Err(Exception::illegal_argument(
                "keyExists must be at least as large as keyCodes",
            ));
        }
        let mut result = im::HasKeysReply {
            result: false,
            key_exists: Some(vec![false; count as usize]),
        };
        // nativeHasKeys rejects a larger array too (without a Java exception).
        if count as usize != codes.len() {
            return Ok(result);
        }
        // sourcesMatchMask excludes AINPUT_SOURCE_CLASS_MASK. Negative ids
        // select all devices, including the virtual keyboard, as InputReader does.
        let mut mark = |d: &InputDevice| {
            if (device < 0 || device == d.id) && d.sources & sources & !0xff != 0 {
                if let Some(keys) = &d.supported_keys {
                    result.result = true;
                    for (out, code) in result.key_exists.as_mut().unwrap().iter_mut().zip(&codes) {
                        *out |= keys.contains(code);
                    }
                }
            }
        };
        if device < 0 {
            mark(self.virtual_keyboard()?.as_ref());
        }
        if let Some(keyboard) = self.keyboard()? {
            mark(&keyboard);
        }
        Ok(result)
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
            im::HAS_KEYS => {
                let a = im::HasKeys::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // Java AIDL sends the allocated out-array's length after the inputs.
                let count = call.data.read_i32().map_err(|_| BAD_VALUE)?;
                return reply(
                    self.has_keys(a.device_id, a.source_mask, a.key_codes, count)
                        .map(|value| {
                            im::write_has_keys_reply(&mut p, &value);
                            p
                        }),
                );
            }
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
                let mut ids = vec![VIRTUAL_KEYBOARD_ID];
                match self.keyboard() {
                    Ok(keyboard) => ids.extend(keyboard.map(|k| k.id)),
                    Err(e) => return reply(Err(e)),
                }
                im::write_get_input_device_ids_reply(&mut p, &Some(ids));
            }
            im::GET_INPUT_DEVICE => {
                let args = im::GetInputDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let device = match args.device_id {
                    VIRTUAL_KEYBOARD_ID => match self.virtual_keyboard() {
                        Ok(device) => Some(device),
                        Err(e) => return reply(Err(e)),
                    },
                    id => match self.keyboard() {
                        Ok(keyboard) => keyboard.filter(|k| k.id == id),
                        Err(e) => return reply(Err(e)),
                    },
                };
                im::write_get_input_device_reply(&mut p, device.as_deref());
            }
            // `@EnforcePermission(BLUETOOTH)`, then EventHub's Bluetooth
            // address of the device: none, as no device the app host has
            // is a Bluetooth one (the built-in keyboard is `BUS_VIRTUAL`).
            im::GET_INPUT_DEVICE_BLUETOOTH_ADDRESS => {
                im::GetInputDeviceBluetoothAddress::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                match self.host.check_permission(Some(BLUETOOTH), uid) {
                    Ok(0) => im::write_get_input_device_bluetooth_address_reply(&mut p, &None),
                    Ok(_) => {
                        return reply(Err(Exception::security(format!(
                            "Access denied, requires: {BLUETOOTH}"
                        ))));
                    }
                    Err(e) => return reply(Err(e)),
                }
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
            code => {
                return reply(Err(self.host.unimplemented(
                    "input",
                    self.descriptor(),
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

    /// InputManagerService.hasKeys / InputReader.markSupportedKeyCodesLocked:
    /// no permission is required; ids, source classes, out-array and errors
    /// follow the original Binder contract.
    #[test]
    fn has_keys_reads_devices_and_validates_the_out_array_without_a_permission_gate() {
        crate::system::with_discovery_session_flags(&[], |host| {
            let manager = InputManager::new(host.clone());
            let device = |id, keys| {
                Arc::new(InputDevice {
                    id,
                    generation: 1,
                    name: "test".into(),
                    vendor: 0,
                    product: 0,
                    bus: 0,
                    descriptor: "test".into(),
                    is_external: false,
                    sources: SOURCE_KEYBOARD_DPAD,
                    keyboard_type: KEYBOARD_TYPE_ALPHABETIC,
                    key_character_map: KeyCharacterMap::load("test.kcm", b"type FULL\n").unwrap(),
                    associated_display: DISPLAY_NONE,
                    supported_keys: keys,
                })
            };
            manager
                .virtual_keyboard
                .set(device(-1, Some(vec![])))
                .ok()
                .unwrap();
            manager
                .keyboard
                .set(Some(device(0, Some(vec![29, 66]))))
                .ok()
                .unwrap();
            let call = |uid, id, mask, codes, count| {
                let mut data = Parcel::new();
                im::HasKeys {
                    device_id: id,
                    source_mask: mask,
                    key_codes: codes,
                }
                .write(&mut data);
                data.write_i32(count);
                let p = manager
                    .transact(&mut Call {
                        code: im::HAS_KEYS,
                        flags: 0,
                        sender_pid: 1,
                        sender_euid: uid,
                        data: data.reader(),
                    })
                    .unwrap();
                im::read_has_keys_reply(&mut p.reader()).unwrap()
            };
            for uid in [0, 1000, 10100, 110100] {
                for id in [-2, -1, 0] {
                    let r = call(uid, id, 0x101, Some(vec![29, 30, 66, -1]), 4).unwrap();
                    assert!(r.result);
                    assert_eq!(r.key_exists.unwrap(), [true, false, true, false]);
                }
            }
            for (id, mask) in [(99, -1), (0, 0), (0, 1), (0, 0x1002)] {
                let r = call(10100, id, mask, Some(vec![29]), 1).unwrap();
                assert!(!r.result);
                assert_eq!(r.key_exists.unwrap(), [false]);
            }
            assert!(call(10100, 0, -1, Some(vec![]), 0).unwrap().result);
            let larger = call(10100, 0, -1, Some(vec![29]), 2).unwrap();
            assert!(!larger.result);
            assert_eq!(larger.key_exists.unwrap(), [false, false]);
            for (keys, count, code) in [
                (None, 1, aim_binder_host::parcel::EX_NULL_POINTER),
                (Some(vec![29]), -1, aim_binder_host::parcel::EX_NULL_POINTER),
                (
                    Some(vec![29, 66]),
                    1,
                    aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT,
                ),
            ] {
                assert_eq!(call(10100, -1, -1, keys, count).unwrap_err().code, code);
            }
            let no_keys = InputManager::new(host.clone());
            no_keys
                .virtual_keyboard
                .set(device(-1, Some(vec![])))
                .ok()
                .unwrap();
            no_keys.keyboard.set(None).ok().unwrap();
            let result = no_keys.has_keys(-1, 0x101, Some(vec![29]), 1).unwrap();
            assert!(result.result); // virtual layout exists, but has no EV_KEY bits
            assert_eq!(result.key_exists.unwrap(), [false]);
            let no_layout = InputManager::new(host.clone());
            no_layout
                .virtual_keyboard
                .set(device(-1, None))
                .ok()
                .unwrap();
            no_layout.keyboard.set(None).ok().unwrap();
            assert!(
                !no_layout
                    .has_keys(-1, 0x101, Some(vec![29]), 1)
                    .unwrap()
                    .result
            );
        });
    }

    #[test]
    fn the_virtual_keyboard_has_the_original_descriptor() {
        // The pinned image's virtual keyboard (`getInputDevice(-1)`).
        assert_eq!(
            descriptor("<virtual>", "Virtual"),
            "a718a782d34bc767f4689c232d64d527998ea7fd"
        );
    }

    /// `KeyCharacterMap::writeToParcel` of four keys and a scan code map.
    #[test]
    fn a_key_character_map_parcels_as_the_original() {
        let kcm = "type FULL\nkey A {\n label: 'A'\n base: 'a'\n shift, capslock: 'A'\n shift+capslock: 'a'\n}\nkey 1 {\n label: '1'\n base: '1'\n shift: '!'\n}\nkey SPACE {\n label: ' '\n base: ' '\n alt, meta: fallback SEARCH\n ctrl: fallback LANGUAGE_SWITCH\n}\nkey C {\n alt: '\\u00e7'\n}\nmap key 0x31 SPACE\n";
        let map = KeyCharacterMap::load("/x.kcm", kcm.as_bytes()).unwrap();
        let mut p = Parcel::new();
        write_key_character_map(&map, &mut p);
        // Name, type, overlay, 4 keys (each code, label, number, behaviors of
        // 5 ints, end), no remapping, one scan code, no usage.
        let behaviors = 4 + 2 + 4 + 1;
        assert_eq!(
            p.data().len(),
            4 + 8 + 4 + 4 + 4 + 4 * (4 * 4) + behaviors * 20 + 4 + 4 + 8 + 4
        );
    }

    /// The built-in keyboard's descriptor, of its name alone.
    #[test]
    fn the_keyboard_has_its_descriptor() {
        let id = aim_input_transport::keyboard::Identifier {
            name: "aim-keyboard".into(),
            bus: 6,
            ..Default::default()
        };
        assert_eq!(
            sha1_hex(&id.raw_descriptor()),
            sha1_hex(":0000:0000:name:aim-keyboard")
        );
    }
}
