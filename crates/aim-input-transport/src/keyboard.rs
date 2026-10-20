//! A keyboard, as InputReader and InputDispatcher handle one at
//! `android-16.0.0_r1`: its configuration found by name
//! (`getInputDeviceConfigurationFilePathByDeviceIdentifier`, `KeyMap::load`),
//! its classes (`EventHub::openDeviceLocked`), its keys mapped and its meta
//! state kept (`KeyboardInputMapper`, `EventHub::mapKey`, `updateMetaState`),
//! the dispatcher's key repeat (`dispatchKeyLocked`,
//! `synthesizeKeyRepeatLocked`), and the `InputMessage` a key is published
//! as (`InputPublisher::publishKeyEvent`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use crate::keychars::KeyCharacterMap;
use crate::keylayout::{
    KeyLayoutMap, POLICY_FLAG_FUNCTION, POLICY_FLAG_GESTURE, POLICY_FLAG_VIRTUAL,
};

/// `AMETA_*`.
pub const AMETA_SHIFT_ON: i32 = 0x01;
pub const AMETA_ALT_ON: i32 = 0x02;
pub const AMETA_SYM_ON: i32 = 0x04;
pub const AMETA_FUNCTION_ON: i32 = 0x08;
pub const AMETA_ALT_LEFT_ON: i32 = 0x10;
pub const AMETA_ALT_RIGHT_ON: i32 = 0x20;
pub const AMETA_SHIFT_LEFT_ON: i32 = 0x40;
pub const AMETA_SHIFT_RIGHT_ON: i32 = 0x80;
pub const AMETA_CTRL_ON: i32 = 0x1000;
pub const AMETA_CTRL_LEFT_ON: i32 = 0x2000;
pub const AMETA_CTRL_RIGHT_ON: i32 = 0x4000;
pub const AMETA_META_ON: i32 = 0x10000;
pub const AMETA_META_LEFT_ON: i32 = 0x20000;
pub const AMETA_META_RIGHT_ON: i32 = 0x40000;
pub const AMETA_CAPS_LOCK_ON: i32 = 0x100000;
pub const AMETA_NUM_LOCK_ON: i32 = 0x200000;
pub const AMETA_SCROLL_LOCK_ON: i32 = 0x400000;

/// The `AKEYCODE_*` the reader and the shell treat specially.
pub const AKEYCODE_UNKNOWN: i32 = 0;
pub const AKEYCODE_DPAD_UP: i32 = 19;
pub const AKEYCODE_DPAD_DOWN: i32 = 20;
pub const AKEYCODE_DPAD_LEFT: i32 = 21;
pub const AKEYCODE_DPAD_RIGHT: i32 = 22;
pub const AKEYCODE_DPAD_CENTER: i32 = 23;
pub const AKEYCODE_Q: i32 = 45;
pub const AKEYCODE_ALT_LEFT: i32 = 57;
pub const AKEYCODE_ALT_RIGHT: i32 = 58;
pub const AKEYCODE_SHIFT_LEFT: i32 = 59;
pub const AKEYCODE_SHIFT_RIGHT: i32 = 60;
pub const AKEYCODE_SYM: i32 = 63;
pub const AKEYCODE_CTRL_LEFT: i32 = 113;
pub const AKEYCODE_CTRL_RIGHT: i32 = 114;
pub const AKEYCODE_CAPS_LOCK: i32 = 115;
pub const AKEYCODE_SCROLL_LOCK: i32 = 116;
pub const AKEYCODE_META_LEFT: i32 = 117;
pub const AKEYCODE_META_RIGHT: i32 = 118;
pub const AKEYCODE_FUNCTION: i32 = 119;
pub const AKEYCODE_NUM_LOCK: i32 = 143;

/// `DPAD_REQUIRED_KEYCODES`, `GAMEPAD_KEYCODES` (KeyCodeClassifications.h):
/// `BUTTON_A` to `BUTTON_MODE`.
const DPAD_REQUIRED_KEYCODES: [i32; 5] = [
    AKEYCODE_DPAD_UP,
    AKEYCODE_DPAD_DOWN,
    AKEYCODE_DPAD_LEFT,
    AKEYCODE_DPAD_RIGHT,
    AKEYCODE_DPAD_CENTER,
];
const GAMEPAD_KEYCODES: std::ops::RangeInclusive<i32> = 96..=110;

/// `AINPUT_SOURCE_KEYBOARD`, `_DPAD`, `_GAMEPAD`.
pub const SOURCE_KEYBOARD: i32 = 0x101;
pub const SOURCE_DPAD: i32 = 0x201;
pub const SOURCE_GAMEPAD: i32 = 0x401;
/// `AKEY_EVENT_ACTION_*`.
pub const ACTION_DOWN: i32 = 0;
pub const ACTION_UP: i32 = 1;
/// `AKEY_EVENT_FLAG_*`.
const FLAG_KEEP_TOUCH_MODE: i32 = 0x4;
pub const FLAG_FROM_SYSTEM: i32 = 0x8;
const FLAG_VIRTUAL_HARD_KEY: i32 = 0x40;
const FLAG_LONG_PRESS: i32 = 0x80;
/// `KeyboardType` (`InputDevice.KEYBOARD_TYPE_*`).
pub const KEYBOARD_TYPE_NON_ALPHABETIC: i32 = 1;
pub const KEYBOARD_TYPE_ALPHABETIC: i32 = 2;
/// `ui::LogicalDisplayId::INVALID`: a keyboard not tied to a display.
pub const DISPLAY_INVALID: i32 = -1;
/// `ReservedInputDeviceId::BUILT_IN_KEYBOARD_ID`, `VIRTUAL_KEYBOARD_ID`.
pub const BUILT_IN_KEYBOARD_ID: i32 = 0;
pub const VIRTUAL_KEYBOARD_ID: i32 = -1;

/// `<linux/input-event-codes.h>`.
const BTN_MISC: u16 = 0x100;
const BTN_MOUSE: u16 = 0x110;
const BTN_JOYSTICK: u16 = 0x120;
const BTN_DIGI: u16 = 0x140;
const BTN_STYLUS3: u16 = 0x149;
const BTN_STYLUS: u16 = 0x14b;
const BTN_STYLUS2: u16 = 0x14c;
const BTN_WHEEL: u16 = 0x150;
pub const KEY_MAX: u16 = 0x2ff;

/// `InputMessage::Type::KEY`.
const KEY: u32 = 0;

/// `normalizeMetaState`: a side's modifier sets its either-side one.
pub fn normalize_meta_state(meta: i32) -> i32 {
    let mut meta = meta;
    for (sides, either) in [
        (AMETA_ALT_LEFT_ON | AMETA_ALT_RIGHT_ON, AMETA_ALT_ON),
        (AMETA_SHIFT_LEFT_ON | AMETA_SHIFT_RIGHT_ON, AMETA_SHIFT_ON),
        (AMETA_CTRL_LEFT_ON | AMETA_CTRL_RIGHT_ON, AMETA_CTRL_ON),
        (AMETA_META_LEFT_ON | AMETA_META_RIGHT_ON, AMETA_META_ON),
    ] {
        if meta & sides != 0 {
            meta |= either;
        }
    }
    meta
}

/// `updateMetaState`: a modifier held while down (releasing any one
/// releases its either-side state), a lock toggled on its up.
pub fn update_meta_state(key_code: i32, down: bool, old: i32) -> i32 {
    let ephemeral = |mask: i32| {
        let new = if down {
            old | mask
        } else {
            old & !(mask | AMETA_ALT_ON | AMETA_SHIFT_ON | AMETA_CTRL_ON | AMETA_META_ON)
        };
        normalize_meta_state(new)
    };
    let locked = |mask: i32| if down { old } else { old ^ mask };
    match key_code {
        AKEYCODE_ALT_LEFT => ephemeral(AMETA_ALT_LEFT_ON),
        AKEYCODE_ALT_RIGHT => ephemeral(AMETA_ALT_RIGHT_ON),
        AKEYCODE_SHIFT_LEFT => ephemeral(AMETA_SHIFT_LEFT_ON),
        AKEYCODE_SHIFT_RIGHT => ephemeral(AMETA_SHIFT_RIGHT_ON),
        AKEYCODE_SYM => ephemeral(AMETA_SYM_ON),
        AKEYCODE_FUNCTION => ephemeral(AMETA_FUNCTION_ON),
        AKEYCODE_CTRL_LEFT => ephemeral(AMETA_CTRL_LEFT_ON),
        AKEYCODE_CTRL_RIGHT => ephemeral(AMETA_CTRL_RIGHT_ON),
        AKEYCODE_META_LEFT => ephemeral(AMETA_META_LEFT_ON),
        AKEYCODE_META_RIGHT => ephemeral(AMETA_META_RIGHT_ON),
        AKEYCODE_CAPS_LOCK => locked(AMETA_CAPS_LOCK_ON),
        AKEYCODE_NUM_LOCK => locked(AMETA_NUM_LOCK_ON),
        AKEYCODE_SCROLL_LOCK => locked(AMETA_SCROLL_LOCK_ON),
        _ => old,
    }
}

/// A key event to publish (`KeyEntry`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub device: i32,
    pub source: i32,
    pub display: i32,
    pub action: i32,
    pub flags: i32,
    pub key_code: i32,
    pub scan_code: i32,
    pub meta_state: i32,
    pub repeat_count: i32,
    pub down_time: i64,
    pub event_time: i64,
}

impl KeyEvent {
    /// `InputMessage` of type `KEY`, as `InputPublisher::publishKeyEvent`
    /// fills it.
    pub fn message(&self, seq: u32, id: i32) -> Vec<u8> {
        let mut m = Vec::with_capacity(8 + 96);
        m.extend_from_slice(&KEY.to_ne_bytes());
        m.extend_from_slice(&seq.to_ne_bytes());
        m.extend_from_slice(&id.to_ne_bytes());
        m.extend_from_slice(&[0; 4]); // empty1
        m.extend_from_slice(&self.event_time.to_ne_bytes());
        for v in [self.device, self.source, self.display] {
            m.extend_from_slice(&v.to_ne_bytes());
        }
        m.extend_from_slice(&[0; 32]); // no HMAC
        for v in [
            self.action,
            self.flags,
            self.key_code,
            self.scan_code,
            self.meta_state,
            self.repeat_count,
        ] {
            m.extend_from_slice(&v.to_ne_bytes());
        }
        m.extend_from_slice(&[0; 4]); // empty2
        m.extend_from_slice(&self.down_time.to_ne_bytes());
        m
    }
}

/// `InputDeviceIdentifier`, as `EVIOCGNAME`, `EVIOCGID` and `EVIOCGUNIQ`
/// fill it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identifier {
    pub name: String,
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    pub unique_id: String,
}

impl Identifier {
    /// `getCanonicalName`: what is not alphanumeric, `-` or `_` made `_`.
    pub fn canonical_name(&self) -> String {
        self.name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// `generateDescriptor`'s raw descriptor (before its SHA-1), of the
    /// first device with it (no nonce).
    pub fn raw_descriptor(&self) -> String {
        let mut raw = format!(":{:04x}:{:04x}:", self.vendor, self.product);
        if !self.unique_id.is_empty() {
            raw += "uniqueId:";
            raw += &self.unique_id;
        }
        if self.vendor == 0 && self.product == 0 && !self.name.is_empty() {
            raw += "name:";
            raw += &self.name;
        }
        raw
    }
}

/// `InputDeviceConfigurationFileType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigFile {
    Configuration,
    KeyLayout,
    KeyCharacterMap,
}

impl ConfigFile {
    fn relative(self, name: &str) -> String {
        match self {
            ConfigFile::Configuration => format!("idc/{name}.idc"),
            ConfigFile::KeyLayout => format!("keylayout/{name}.kl"),
            ConfigFile::KeyCharacterMap => format!("keychars/{name}.kcm"),
        }
    }
}

/// Where configuration files are searched (`getInputDeviceConfiguration
/// FilePathByName`): the partitions' `usr/` directories, the APEX of
/// `input_device.config_file.apex` if set, `$ANDROID_ROOT/usr/`, then
/// `$ANDROID_DATA/system/devices/`; `readable` is `access(R_OK)`.
pub struct ConfigFiles<'a> {
    pub prefixes: Vec<String>,
    pub readable: &'a dyn Fn(&str) -> bool,
}

impl ConfigFiles<'_> {
    /// The standard prefixes, before the user repository.
    pub fn prefixes(
        apex: Option<&str>,
        android_root: Option<&str>,
        android_data: &str,
    ) -> Vec<String> {
        let mut prefixes: Vec<String> = [
            "/product/usr/",
            "/system_ext/usr/",
            "/odm/usr/",
            "/vendor/usr/",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        if let Some(apex) = apex.filter(|a| !a.is_empty()) {
            prefixes.push(format!("/apex/{apex}/etc/usr/"));
        }
        if let Some(root) = android_root {
            prefixes.push(format!("{root}/usr/"));
        }
        prefixes.push(format!("{android_data}/system/devices/"));
        prefixes
    }

    /// `getInputDeviceConfigurationFilePathByName`.
    pub fn by_name(&self, name: &str, kind: ConfigFile) -> Option<String> {
        self.prefixes
            .iter()
            .map(|prefix| format!("{prefix}{}", kind.relative(name)))
            .find(|path| (self.readable)(path))
    }

    /// `getInputDeviceConfigurationFilePathByDeviceIdentifier`.
    pub fn by_identifier(&self, id: &Identifier, kind: ConfigFile, suffix: &str) -> Option<String> {
        if id.vendor != 0 && id.product != 0 {
            if id.version != 0 {
                let name = format!(
                    "Vendor_{:04x}_Product_{:04x}_Version_{:04x}{suffix}",
                    id.vendor, id.product, id.version
                );
                if let Some(path) = self.by_name(&name, kind) {
                    return Some(path);
                }
            }
            let name = format!(
                "Vendor_{:04x}_Product_{:04x}{suffix}",
                id.vendor, id.product
            );
            if let Some(path) = self.by_name(&name, kind) {
                return Some(path);
            }
        }
        self.by_name(&format!("{}{suffix}", id.canonical_name()), kind)
    }
}

/// `PropertyMap::load`: `key = value` lines, `#` comments.
pub fn property_map(contents: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut map: Vec<(String, String)> = Vec::new();
    for (n, line) in String::from_utf8_lossy(contents).lines().enumerate() {
        let line = line.trim_start_matches([' ', '\t', '\r']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let end = line.find([' ', '\t', '\r', '=']).unwrap_or(line.len());
        let (key, rest) = line.split_at(end);
        if key.is_empty() {
            return Err(format!("line {}: Expected non-empty property key.", n + 1));
        }
        let rest = rest.trim_start_matches([' ', '\t', '\r']);
        let Some(value) = rest.strip_prefix('=') else {
            return Err(format!(
                "line {}: Expected '=' between property key and value.",
                n + 1
            ));
        };
        let value = value.trim_matches([' ', '\t', '\r']);
        if value.contains(['\\', '"']) {
            return Err(format!(
                "line {}: Found reserved character in property value.",
                n + 1
            ));
        }
        map.retain(|(k, _)| k != key);
        map.push((key.to_string(), value.to_string()));
    }
    Ok(map)
}

/// `PropertyMap::getBool`: a value that parses as an integer, non-zero.
pub fn property_bool(map: &[(String, String)], key: &str) -> Option<bool> {
    let value = map.iter().find(|(k, _)| k == key)?.1.as_str();
    crate::keychars::strtol(value).map(|v| v != 0)
}

pub fn property_string<'a>(map: &'a [(String, String)], key: &str) -> Option<&'a str> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `KeyMap`: the key layout and key character map a device loads, with
/// their files.
#[derive(Clone, Debug, Default)]
pub struct KeyMap {
    pub layout: Option<KeyLayoutMap>,
    pub characters: Option<KeyCharacterMap>,
}

impl KeyMap {
    /// `KeyMap::load`: the configuration's `keyboard.layout` and
    /// `keyboard.characterMap`, else by the device's identifier, then
    /// `Generic`, then `Virtual`. `read` reads a file; a layout that needs
    /// kernel options is refused (`kernelConfigsArePresent`, which reads
    /// the running kernel's configuration).
    pub fn load(
        id: &Identifier,
        config: &[(String, String)],
        files: &ConfigFiles<'_>,
        read: &dyn Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<KeyMap, String> {
        let mut map = KeyMap::default();
        let mut errors = Vec::new();
        let mut layout = |map: &mut KeyMap, path: Option<String>| {
            let Some(path) = path else { return };
            match read(&path).map(|c| KeyLayoutMap::load(&path, &c)) {
                Some(Ok(layout)) if layout.required_kernel_configs.is_empty() => {
                    map.layout = Some(layout)
                }
                Some(Ok(_)) => errors.push(format!(
                    "{path}: a layout requiring kernel options is not implemented (#1434)"
                )),
                Some(Err(e)) => errors.push(e),
                None => errors.push(format!("{path}: unreadable")),
            }
        };
        let characters = |map: &mut KeyMap, path: Option<String>| {
            let Some(path) = path else { return };
            if let Some(Ok(kcm)) = read(&path).map(|c| KeyCharacterMap::load(&path, &c)) {
                map.characters = Some(kcm);
            }
        };
        let by = |name: &str, kind| {
            if name.is_empty() {
                files.by_identifier(id, kind, "")
            } else {
                files.by_name(name, kind)
            }
        };
        if let Some(name) = property_string(config, "keyboard.layout") {
            layout(&mut map, by(name, ConfigFile::KeyLayout));
        }
        if let Some(name) = property_string(config, "keyboard.characterMap") {
            characters(&mut map, by(name, ConfigFile::KeyCharacterMap));
        }
        for name in ["", "Generic", "Virtual"] {
            if map.layout.is_some() && map.characters.is_some() {
                break;
            }
            if map.layout.is_none() {
                layout(&mut map, by(name, ConfigFile::KeyLayout));
            }
            if map.characters.is_none() {
                characters(&mut map, by(name, ConfigFile::KeyCharacterMap));
            }
        }
        if map.layout.is_some() && map.characters.is_some() {
            Ok(map)
        } else {
            Err(format!(
                "Could not determine key map for device '{}' ({})",
                id.name,
                errors.join("; ")
            ))
        }
    }
}

/// What EventHub makes of a keyboard: its classes, as far as keys go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Classes {
    pub keyboard: bool,
    pub alphakey: bool,
    pub dpad: bool,
    pub gamepad: bool,
}

impl Classes {
    /// `KeyboardInputMapper`'s sources (`InputDevice::populateMappers`).
    pub fn sources(self) -> i32 {
        let mut sources = 0;
        if self.keyboard {
            sources |= SOURCE_KEYBOARD;
        }
        if self.dpad {
            sources |= SOURCE_DPAD;
        }
        if self.gamepad {
            sources |= SOURCE_GAMEPAD;
        }
        sources
    }

    /// `KeyboardClassifier::notifyKeyboardChanged` without the Rust
    /// classifier: alphabetic with a Q key.
    pub fn keyboard_type(self) -> i32 {
        if self.alphakey {
            KEYBOARD_TYPE_ALPHABETIC
        } else {
            KEYBOARD_TYPE_NON_ALPHABETIC
        }
    }
}

/// A keyboard's keys and the map they go through: `has_key(code)` is its
/// `EV_KEY` bit, `has_msc_scan` its `MSC_SCAN` bit.
pub struct Keyboard<'a> {
    pub map: &'a KeyMap,
    pub has_key: &'a dyn Fn(u16) -> bool,
    pub has_msc_scan: bool,
}

impl Keyboard<'_> {
    /// `hasKeycodeInternalLocked`.
    fn has_key_code_internal(&self, key_code: i32) -> bool {
        let Some(layout) = &self.map.layout else {
            return false;
        };
        layout
            .find_scan_codes_for_key(key_code)
            .into_iter()
            .any(|sc| (0..=KEY_MAX as i32).contains(&sc) && (self.has_key)(sc as u16))
            || (!layout.find_usage_codes_for_key(key_code).is_empty() && self.has_msc_scan)
    }

    /// `hasKeycodeLocked`: the key, or a key remapped to it.
    pub fn has_key_code(&self, key_code: i32) -> bool {
        self.has_key_code_internal(key_code)
            || self.map.characters.as_ref().is_some_and(|kcm| {
                kcm.find_key_codes_mapped_to_key_code(key_code)
                    .into_iter()
                    .any(|from| self.has_key_code_internal(from))
            })
    }

    /// The classes `openDeviceLocked` gives it.
    pub fn classes(&self) -> Classes {
        if !has_keys(self.has_key) {
            return Classes::default();
        }
        Classes {
            keyboard: true,
            alphakey: self.has_key_code(AKEYCODE_Q),
            dpad: DPAD_REQUIRED_KEYCODES.iter().all(|&k| self.has_key_code(k)),
            gamepad: GAMEPAD_KEYCODES.into_iter().any(|k| self.has_key_code(k)),
        }
    }
}

/// Whether a device of these `EV_KEY` bits has keyboard keys
/// (`haveKeyboardKeys`), gamepad or stylus buttons: the `KEYBOARD` class.
pub fn has_keys(has_key: &dyn Fn(u16) -> bool) -> bool {
    let any = |range: std::ops::Range<u16>| range.into_iter().any(has_key);
    any(0..BTN_MISC)
        || any(BTN_WHEEL..KEY_MAX + 1)
        || any(BTN_MISC..BTN_MOUSE)
        || any(BTN_JOYSTICK..BTN_DIGI)
        || [BTN_STYLUS, BTN_STYLUS2, BTN_STYLUS3]
            .into_iter()
            .any(has_key)
}

/// `isEligibleBuiltInKeyboard`: a key character map that is not of a
/// special function keyboard, and `keyboard.builtIn` or a `-keypad` name.
pub fn eligible_built_in(id: &Identifier, config: &[(String, String)], map: &KeyMap) -> bool {
    let Some(kcm) = &map.characters else {
        return false;
    };
    if property_bool(config, "keyboard.specialFunction").unwrap_or(false)
        || kcm.kind == crate::keychars::KeyboardType::SpecialFunction
    {
        return false;
    }
    property_bool(config, "keyboard.builtIn").unwrap_or(false) || id.name.contains("-keypad")
}

/// `isSupportedScanCode`.
pub fn supported_scan_code(code: u16) -> bool {
    code < BTN_MOUSE
        || (BTN_JOYSTICK..BTN_DIGI).contains(&code)
        || code == BTN_STYLUS
        || code == BTN_STYLUS2
        || code == BTN_STYLUS3
        || code >= BTN_WHEEL
}

/// A key held down (`KeyboardInputMapper::KeyDown`).
#[derive(Clone, Copy, Debug)]
struct KeyDown {
    key_code: i32,
    scan_code: i32,
    down_time: i64,
    flags: i32,
}

/// `KeyboardInputMapper` of a keyboard not tied to a display, with the
/// dispatcher's `notifyKey` applied to what it notifies.
pub struct Mapper {
    pub device: i32,
    pub sources: i32,
    downs: Vec<KeyDown>,
    meta_state: i32,
}

impl Mapper {
    pub fn new(device: i32, sources: i32) -> Mapper {
        Mapper {
            device,
            sources,
            downs: Vec::new(),
            meta_state: 0,
        }
    }

    /// `processKey` of an `EV_KEY` of `scan_code` (and the HID usage
    /// before it) at `when`: the key event, or none for an up of a key not
    /// down. A key of no mapping is `AKEYCODE_UNKNOWN`.
    pub fn process_key(
        &mut self,
        map: &KeyMap,
        when: i64,
        down: bool,
        scan_code: i32,
        usage_code: i32,
    ) -> Option<KeyEvent> {
        let mut flags = FLAG_FROM_SYSTEM;
        // `EventHub::mapKey`.
        let mapped = map
            .characters
            .as_ref()
            .and_then(|kcm| kcm.map_key(scan_code, usage_code))
            .map(|code| (code, 0))
            .or_else(|| {
                map.layout
                    .as_ref()?
                    .map_key(scan_code, usage_code)
                    .map(|k| (k.key_code, k.flags))
            });
        let (mut key_code, mut key_meta, policy) = match mapped {
            Some((code, policy)) => {
                let (code, meta) = match &map.characters {
                    Some(kcm) => {
                        kcm.apply_key_behavior(kcm.apply_key_remapping(code), self.meta_state)
                    }
                    None => (code, self.meta_state),
                };
                (code, meta, policy)
            }
            None => (AKEYCODE_UNKNOWN, self.meta_state, 0),
        };
        let mut down_time = when;
        let held = self.downs.iter().position(|d| d.scan_code == scan_code);
        if down {
            if let Some(i) = held {
                key_code = self.downs[i].key_code;
                down_time = self.downs[i].down_time;
                flags = self.downs[i].flags;
            } else {
                if policy & POLICY_FLAG_GESTURE != 0 {
                    flags |= FLAG_KEEP_TOUCH_MODE;
                }
                self.downs.push(KeyDown {
                    key_code,
                    scan_code,
                    down_time: when,
                    flags,
                });
            }
        } else {
            let i = held?;
            let d = self.downs.remove(i);
            (key_code, down_time, flags) = (d.key_code, d.down_time, d.flags);
        }
        let new = update_meta_state(key_code, down, self.meta_state);
        if new != self.meta_state {
            self.meta_state = new;
            key_meta = new;
        }
        // `InputDispatcher::notifyKey`.
        if policy & POLICY_FLAG_VIRTUAL != 0 {
            flags |= FLAG_VIRTUAL_HARD_KEY;
        }
        if policy & POLICY_FLAG_FUNCTION != 0 {
            key_meta |= AMETA_FUNCTION_ON;
        }
        Some(KeyEvent {
            device: self.device,
            source: self.sources & (SOURCE_KEYBOARD | SOURCE_DPAD | SOURCE_GAMEPAD),
            display: DISPLAY_INVALID,
            action: if down { ACTION_DOWN } else { ACTION_UP },
            flags,
            key_code,
            scan_code,
            meta_state: key_meta,
            repeat_count: 0,
            down_time,
            event_time: when,
        })
    }
}

/// `setKeyRepeatConfiguration`: the time a key is held before it repeats,
/// between repeats (ns), and whether keys repeat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatConfiguration {
    pub timeout: i64,
    pub delay: i64,
    pub enabled: bool,
}

impl Default for RepeatConfiguration {
    /// `InputDispatcherConfiguration`'s, until the policy sets them:
    /// `ViewConfiguration.getKeyRepeatTimeout()` and `getKeyRepeatDelay()`.
    fn default() -> Self {
        RepeatConfiguration {
            timeout: 400_000_000,
            delay: 50_000_000,
            enabled: true,
        }
    }
}

/// `mKeyRepeatState`: the last key down and when it repeats next.
#[derive(Default)]
pub struct Repeat {
    last: Option<KeyEvent>,
    next: Option<i64>,
}

impl Repeat {
    /// `dispatchKeyLocked`'s preprocessing of a trusted key not synthesized
    /// here: a down starts the repeat timer (a second down of the same key
    /// is the driver's own repeat), an up of its device stops it. Returns
    /// the event with its repeat count and long press flag.
    pub fn dispatch(&mut self, config: RepeatConfiguration, mut e: KeyEvent) -> KeyEvent {
        if e.action == ACTION_DOWN && config.enabled {
            match &self.last {
                Some(last) if last.key_code == e.key_code && last.device == e.device => {
                    e.repeat_count = last.repeat_count + 1;
                    self.next = None;
                }
                _ => self.next = Some(e.event_time + config.timeout),
            }
            self.last = Some(e.clone());
        } else if e.action == ACTION_UP
            && self
                .last
                .as_ref()
                .is_some_and(|last| last.device != e.device)
        {
            // The key of another device is still down.
        } else {
            *self = Repeat::default();
        }
        e.flags = long_press(e.flags, e.repeat_count);
        e
    }

    /// When the next repeat is due.
    pub fn next(&self) -> Option<i64> {
        self.next.filter(|_| self.last.is_some())
    }

    /// `synthesizeKeyRepeatLocked` at `now`, if due.
    pub fn synthesize(&mut self, config: RepeatConfiguration, now: i64) -> Option<KeyEvent> {
        if self.next()? > now {
            return None;
        }
        let last = self.last.as_mut()?;
        last.repeat_count += 1;
        last.event_time = now;
        self.next = Some(now + config.delay);
        let mut e = last.clone();
        e.flags = long_press(e.flags, e.repeat_count);
        Some(e)
    }

    /// `resetKeyRepeatLocked`, as a device's reset does.
    pub fn reset(&mut self) {
        *self = Repeat::default();
    }
}

/// The long press flag of the first repeat.
fn long_press(flags: i32, repeat_count: i32) -> i32 {
    if repeat_count == 1 {
        flags | FLAG_LONG_PRESS
    } else {
        flags & !FLAG_LONG_PRESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KL: &str = "key 30 A\nkey 42 SHIFT_LEFT\nkey 58 CAPS_LOCK\nkey 103 DPAD_UP\nkey 108 DPAD_DOWN\nkey 105 DPAD_LEFT\nkey 106 DPAD_RIGHT\nkey 353 DPAD_CENTER\nkey 16 Q\nkey 464 FUNCTION FUNCTION\n";
    const KCM: &str = "type FULL\nkey A {\n label: 'A'\n base: 'a'\n shift, capslock: 'A'\n}\nkey Q {\n label: 'Q'\n base: 'q'\n shift: 'Q'\n}\n";

    fn map() -> KeyMap {
        KeyMap {
            layout: Some(KeyLayoutMap::load("/Generic.kl", KL.as_bytes()).unwrap()),
            characters: Some(KeyCharacterMap::load("/Generic.kcm", KCM.as_bytes()).unwrap()),
        }
    }

    #[test]
    fn keys_map_with_their_meta_state() {
        let map = map();
        let mut m = Mapper::new(0, SOURCE_KEYBOARD | SOURCE_DPAD);
        let shift = m.process_key(&map, 1, true, 42, 0).unwrap();
        assert_eq!(
            (shift.key_code, shift.meta_state),
            (59, AMETA_SHIFT_ON | AMETA_SHIFT_LEFT_ON)
        );
        let a = m.process_key(&map, 2, true, 30, 0).unwrap();
        assert_eq!(
            (a.key_code, a.meta_state, a.flags, a.source, a.display),
            (29, 0x41, FLAG_FROM_SYSTEM, 0x301, DISPLAY_INVALID)
        );
        let up = m.process_key(&map, 3, false, 30, 0).unwrap();
        assert_eq!((up.action, up.down_time), (ACTION_UP, 2));
        // An up of a key not down is dropped.
        assert!(m.process_key(&map, 4, false, 30, 0).is_none());
        let shift_up = m.process_key(&map, 5, false, 42, 0).unwrap();
        assert_eq!(shift_up.meta_state, 0);
        // Caps Lock toggles on its up.
        m.process_key(&map, 6, true, 58, 0);
        let caps = m.process_key(&map, 7, false, 58, 0).unwrap();
        assert_eq!(caps.meta_state, AMETA_CAPS_LOCK_ON);
        // A function key carries the function modifier.
        let f = m.process_key(&map, 8, true, 464, 0).unwrap();
        assert_eq!(f.meta_state & AMETA_FUNCTION_ON, AMETA_FUNCTION_ON);
        // An unmapped key is unknown.
        assert_eq!(m.process_key(&map, 9, true, 2, 0).unwrap().key_code, 0);
    }

    #[test]
    fn a_keyboard_with_a_dpad_has_its_classes() {
        let map = map();
        let keys = [30u16, 42, 58, 103, 108, 105, 106, 353, 16];
        let has_key = |c: u16| keys.contains(&c);
        let k = Keyboard {
            map: &map,
            has_key: &has_key,
            has_msc_scan: false,
        };
        let classes = k.classes();
        assert_eq!(
            classes,
            Classes {
                keyboard: true,
                alphakey: true,
                dpad: true,
                gamepad: false
            }
        );
        assert_eq!(
            (classes.sources(), classes.keyboard_type()),
            (0x301, KEYBOARD_TYPE_ALPHABETIC)
        );
        // Without the center key, no D-pad.
        let no_center = |c: u16| c != 353 && keys.contains(&c);
        let k = Keyboard {
            map: &map,
            has_key: &no_center,
            has_msc_scan: false,
        };
        assert!(!k.classes().dpad);
    }

    #[test]
    fn a_held_key_repeats_after_the_timeout() {
        let config = RepeatConfiguration::default();
        let mut r = Repeat::default();
        let mut m = Mapper::new(0, SOURCE_KEYBOARD);
        let map = map();
        let down = m.process_key(&map, 1_000, true, 30, 0).unwrap();
        let first = r.dispatch(config, down);
        assert_eq!(first.repeat_count, 0);
        assert_eq!(r.next(), Some(1_000 + 400_000_000));
        assert!(r.synthesize(config, 1_000).is_none());
        let repeat = r.synthesize(config, 400_001_000).unwrap();
        assert_eq!(
            (repeat.repeat_count, repeat.flags & FLAG_LONG_PRESS),
            (1, FLAG_LONG_PRESS)
        );
        let again = r.synthesize(config, 450_001_000).unwrap();
        assert_eq!((again.repeat_count, again.flags & FLAG_LONG_PRESS), (2, 0));
        let up = m.process_key(&map, 460_000_000, false, 30, 0).unwrap();
        r.dispatch(config, up);
        assert_eq!(r.next(), None);
    }

    #[test]
    fn a_key_message_has_the_published_layout() {
        let e = KeyEvent {
            device: 0,
            source: SOURCE_KEYBOARD,
            display: DISPLAY_INVALID,
            action: ACTION_DOWN,
            flags: FLAG_FROM_SYSTEM,
            key_code: 29,
            scan_code: 30,
            meta_state: 0,
            repeat_count: 0,
            down_time: 7,
            event_time: 9,
        };
        let m = e.message(1, 2);
        // The header, then `Body::Key` (96 bytes).
        assert_eq!(m.len(), 8 + 96);
        let int = |at: usize| i32::from_ne_bytes(m[at..at + 4].try_into().unwrap());
        let long = |at: usize| i64::from_ne_bytes(m[at..at + 8].try_into().unwrap());
        assert_eq!((int(0), int(4), int(8)), (0, 1, 2));
        assert_eq!(long(16), 9);
        assert_eq!((int(24), int(28), int(32)), (0, SOURCE_KEYBOARD, -1));
        assert_eq!((int(68), int(72), int(76)), (0, FLAG_FROM_SYSTEM, 29));
        assert_eq!(long(96), 7);
    }

    #[test]
    fn configuration_is_found_by_name() {
        let present = [
            "/vendor/usr/idc/aim-keyboard.idc",
            "/system/usr/keylayout/Generic.kl",
        ];
        let readable = |p: &str| present.contains(&p);
        let files = ConfigFiles {
            prefixes: ConfigFiles::prefixes(None, Some("/system"), "/data"),
            readable: &readable,
        };
        let id = Identifier {
            name: "aim-keyboard".into(),
            ..Default::default()
        };
        assert_eq!(
            files
                .by_identifier(&id, ConfigFile::Configuration, "")
                .as_deref(),
            Some("/vendor/usr/idc/aim-keyboard.idc")
        );
        assert_eq!(
            files.by_name("Generic", ConfigFile::KeyLayout).as_deref(),
            Some("/system/usr/keylayout/Generic.kl")
        );
        assert_eq!(id.raw_descriptor(), ":0000:0000:name:aim-keyboard");
        let config = property_map(b"# c\ndevice.internal = 1\nkeyboard.builtIn=1\n").unwrap();
        assert_eq!(property_bool(&config, "keyboard.builtIn"), Some(true));
        assert!(eligible_built_in(&id, &config, &map()));
        assert!(property_map(b"a 1\n").is_err());
    }
}
