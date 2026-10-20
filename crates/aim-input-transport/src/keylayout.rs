//! Key layout maps (`.kl` files), as `KeyLayoutMap::load` parses them at
//! `android-16.0.0_r1` (frameworks/native/libs/input/KeyLayoutMap.cpp, the
//! labels of `InputEventLabels.cpp`): a device's scan codes and HID usages
//! to key codes and policy flags, its axes, LEDs and sensors.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, BTreeSet};

use crate::keychars::{Tokenizer, WHITESPACE, key_code_by_label, strtol};

/// `POLICY_FLAG_*` a layout gives a key.
pub const POLICY_FLAG_WAKE: u32 = 0x01;
pub const POLICY_FLAG_VIRTUAL: u32 = 0x02;
pub const POLICY_FLAG_FUNCTION: u32 = 0x04;
pub const POLICY_FLAG_GESTURE: u32 = 0x08;
pub const POLICY_FLAG_FALLBACK_USAGE_MAPPING: u32 = 0x10;

/// `FLAGS_SEQUENCE`.
const FLAGS: &[(&str, u32)] = &[
    ("VIRTUAL", POLICY_FLAG_VIRTUAL),
    ("FUNCTION", POLICY_FLAG_FUNCTION),
    ("GESTURE", POLICY_FLAG_GESTURE),
    ("WAKE", POLICY_FLAG_WAKE),
    ("FALLBACK_USAGE_MAPPING", POLICY_FLAG_FALLBACK_USAGE_MAPPING),
];

/// `LEDS_SEQUENCE`: `ALED_*` in order from 0.
const LEDS: &[&str] = &[
    "NUM_LOCK",
    "CAPS_LOCK",
    "SCROLL_LOCK",
    "COMPOSE",
    "KANA",
    "SLEEP",
    "SUSPEND",
    "MUTE",
    "MISC",
    "MAIL",
    "CHARGING",
    "CONTROLLER_1",
    "CONTROLLER_2",
    "CONTROLLER_3",
    "CONTROLLER_4",
];

/// `AXES_SEQUENCE`: `AMOTION_EVENT_AXIS_*` in order from 0.
const AXES: &[&str] = &[
    "X",
    "Y",
    "PRESSURE",
    "SIZE",
    "TOUCH_MAJOR",
    "TOUCH_MINOR",
    "TOOL_MAJOR",
    "TOOL_MINOR",
    "ORIENTATION",
    "VSCROLL",
    "HSCROLL",
    "Z",
    "RX",
    "RY",
    "RZ",
    "HAT_X",
    "HAT_Y",
    "LTRIGGER",
    "RTRIGGER",
    "THROTTLE",
    "RUDDER",
    "WHEEL",
    "GAS",
    "BRAKE",
    "DISTANCE",
    "TILT",
    "SCROLL",
    "RELATIVE_X",
    "RELATIVE_Y",
    "RESERVED_29",
    "RESERVED_30",
    "RESERVED_31",
    "GENERIC_1",
    "GENERIC_2",
    "GENERIC_3",
    "GENERIC_4",
    "GENERIC_5",
    "GENERIC_6",
    "GENERIC_7",
    "GENERIC_8",
    "GENERIC_9",
    "GENERIC_10",
    "GENERIC_11",
    "GENERIC_12",
    "GENERIC_13",
    "GENERIC_14",
    "GENERIC_15",
    "GENERIC_16",
    "GESTURE_X_OFFSET",
    "GESTURE_Y_OFFSET",
    "GESTURE_SCROLL_X_DISTANCE",
    "GESTURE_SCROLL_Y_DISTANCE",
    "GESTURE_PINCH_SCALE_FACTOR",
    "GESTURE_SWIPE_FINGER_COUNT",
];

/// `InputDeviceSensorType` names a `sensor` line takes.
const SENSORS: &[&str] = &[
    "ACCELEROMETER",
    "MAGNETIC_FIELD",
    "ORIENTATION",
    "GYROSCOPE",
    "LIGHT",
    "PRESSURE",
    "TEMPERATURE",
    "PROXIMITY",
    "GRAVITY",
    "LINEAR_ACCELERATION",
    "ROTATION_VECTOR",
    "RELATIVE_HUMIDITY",
    "AMBIENT_TEMPERATURE",
    "MAGNETIC_FIELD_UNCALIBRATED",
    "GAME_ROTATION_VECTOR",
    "GYROSCOPE_UNCALIBRATED",
    "SIGNIFICANT_MOTION",
];

/// A key's code and policy flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub key_code: i32,
    pub flags: u32,
}

/// `AxisInfo`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Normal(i32),
    Invert(i32),
    Split { value: i32, low: i32, high: i32 },
}

/// A parsed key layout map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyLayoutMap {
    pub load_file_name: String,
    pub keys_by_scan_code: BTreeMap<i32, Key>,
    pub keys_by_usage_code: BTreeMap<i32, Key>,
    pub axes: BTreeMap<i32, (Axis, Option<i32>)>,
    pub leds_by_scan_code: BTreeMap<i32, i32>,
    pub leds_by_usage_code: BTreeMap<i32, i32>,
    pub sensors_by_abs_code: BTreeMap<i32, (usize, usize)>,
    /// `requires_kernel_config`'s options, which the map needs the kernel
    /// built with (`kernelConfigsArePresent`).
    pub required_kernel_configs: BTreeSet<String>,
}

fn int(token: &str) -> Result<i32, String> {
    strtol(token).ok_or_else(|| format!("Could not parse {token}"))
}

fn axis(token: &str) -> Result<i32, String> {
    AXES.iter()
        .position(|a| *a == token)
        .map(|i| i as i32)
        .ok_or_else(|| format!("Expected axis label, got '{token}'."))
}

impl KeyLayoutMap {
    /// `KeyLayoutMap::load` of `contents`, as far as the parser goes: the
    /// caller checks `required_kernel_configs`.
    pub fn load(file_name: &str, contents: &[u8]) -> Result<KeyLayoutMap, String> {
        let mut map = KeyLayoutMap {
            load_file_name: file_name.to_string(),
            ..Default::default()
        };
        let mut t = Tokenizer::new(contents);
        let err = |t: &Tokenizer<'_>, what: String| format!("{file_name}:{}: {what}", t.line);
        while !t.is_eof() {
            t.skip_delimiters(WHITESPACE);
            if !t.is_eol() && t.peek_char() != b'#' {
                let keyword = t.next_token(WHITESPACE);
                t.skip_delimiters(WHITESPACE);
                let parsed = match keyword.as_str() {
                    "key" => map.parse_key(&mut t),
                    "axis" => map.parse_axis(&mut t),
                    "led" => map.parse_led(&mut t),
                    "sensor" => map.parse_sensor(&mut t),
                    "requires_kernel_config" => map.parse_required_kernel_config(&mut t),
                    other => Err(format!("Expected keyword, got '{other}'.")),
                };
                parsed.map_err(|e| err(&t, e))?;
                t.skip_delimiters(WHITESPACE);
                if !t.is_eol() && t.peek_char() != b'#' {
                    return Err(err(&t, "Expected end of line or trailing comment.".into()));
                }
            }
            t.next_line();
        }
        Ok(map)
    }

    /// `key [usage] <code> <key code label> [flags...]`.
    fn parse_key(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        let mut code_token = t.next_token(WHITESPACE);
        let usage = code_token == "usage";
        if usage {
            t.skip_delimiters(WHITESPACE);
            code_token = t.next_token(WHITESPACE);
        }
        let code = int(&code_token)?;
        let map = if usage {
            &mut self.keys_by_usage_code
        } else {
            &mut self.keys_by_scan_code
        };
        if map.contains_key(&code) {
            return Err(format!("Duplicate entry for key code '{code_token}'."));
        }
        t.skip_delimiters(WHITESPACE);
        let label = t.next_token(WHITESPACE);
        let key_code = key_code_by_label(&label)
            .ok_or_else(|| format!("Expected key code label, got '{label}'."))?;
        let mut flags = 0;
        loop {
            t.skip_delimiters(WHITESPACE);
            if t.is_eol() || t.peek_char() == b'#' {
                break;
            }
            let token = t.next_token(WHITESPACE);
            let flag = FLAGS
                .iter()
                .find(|(label, _)| *label == token)
                .map(|(_, f)| *f)
                .ok_or_else(|| format!("Expected key flag label, got '{token}'."))?;
            if flags & flag != 0 {
                return Err(format!("Duplicate key flag '{token}'."));
            }
            flags |= flag;
        }
        map.insert(code, Key { key_code, flags });
        Ok(())
    }

    /// `axis <scan code> [invert <axis> | split <value> <low> <high> |
    /// <axis>] [flat <value>]`.
    fn parse_axis(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        let scan_token = t.next_token(WHITESPACE);
        let scan_code = int(&scan_token)?;
        if self.axes.contains_key(&scan_code) {
            return Err(format!(
                "Duplicate entry for axis scan code '{scan_token}'."
            ));
        }
        t.skip_delimiters(WHITESPACE);
        let token = t.next_token(WHITESPACE);
        let info = match token.as_str() {
            "invert" => {
                t.skip_delimiters(WHITESPACE);
                Axis::Invert(axis(&t.next_token(WHITESPACE))?)
            }
            "split" => {
                t.skip_delimiters(WHITESPACE);
                let value = int(&t.next_token(WHITESPACE))?;
                t.skip_delimiters(WHITESPACE);
                let low = axis(&t.next_token(WHITESPACE))?;
                t.skip_delimiters(WHITESPACE);
                let high = axis(&t.next_token(WHITESPACE))?;
                Axis::Split { value, low, high }
            }
            other => Axis::Normal(axis(other)?),
        };
        let mut flat = None;
        loop {
            t.skip_delimiters(WHITESPACE);
            if t.is_eol() || t.peek_char() == b'#' {
                break;
            }
            let keyword = t.next_token(WHITESPACE);
            if keyword != "flat" {
                return Err(format!("Expected keyword 'flat', got '{keyword}'."));
            }
            t.skip_delimiters(WHITESPACE);
            flat = Some(int(&t.next_token(WHITESPACE))?);
        }
        self.axes.insert(scan_code, (info, flat));
        Ok(())
    }

    /// `led [usage] <code> <led label>`.
    fn parse_led(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        let mut code_token = t.next_token(WHITESPACE);
        let usage = code_token == "usage";
        if usage {
            t.skip_delimiters(WHITESPACE);
            code_token = t.next_token(WHITESPACE);
        }
        let code = int(&code_token)?;
        let map = if usage {
            &mut self.leds_by_usage_code
        } else {
            &mut self.leds_by_scan_code
        };
        if map.contains_key(&code) {
            return Err(format!("Duplicate entry for led code '{code_token}'."));
        }
        t.skip_delimiters(WHITESPACE);
        let label = t.next_token(WHITESPACE);
        let led = LEDS
            .iter()
            .position(|l| *l == label)
            .ok_or_else(|| format!("Expected LED code label, got '{label}'."))?;
        map.insert(code, led as i32);
        Ok(())
    }

    /// `sensor <abs code> <sensor type> <X|Y|Z>`.
    fn parse_sensor(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        let code_token = t.next_token(WHITESPACE);
        let code = int(&code_token)?;
        if self.sensors_by_abs_code.contains_key(&code) {
            return Err(format!(
                "Duplicate entry for sensor abs code '{code_token}'."
            ));
        }
        t.skip_delimiters(WHITESPACE);
        let kind = t.next_token(WHITESPACE);
        let kind = SENSORS
            .iter()
            .position(|s| *s == kind)
            .ok_or_else(|| format!("Expected sensor type label, got '{kind}'."))?;
        t.skip_delimiters(WHITESPACE);
        let index = match t.next_token(WHITESPACE).as_str() {
            "X" => 0,
            "Y" => 1,
            "Z" => 2,
            other => return Err(format!("Expected sensor data index label, got '{other}'.")),
        };
        self.sensors_by_abs_code.insert(code, (kind, index));
        Ok(())
    }

    fn parse_required_kernel_config(&mut self, t: &mut Tokenizer<'_>) -> Result<(), String> {
        let name = t.next_token(WHITESPACE);
        if !self.required_kernel_configs.insert(name.clone()) {
            return Err(format!(
                "Duplicate entry for required kernel config {name}."
            ));
        }
        Ok(())
    }

    /// `mapKey`: the usage's key, else the scan code's.
    pub fn map_key(&self, scan_code: i32, usage_code: i32) -> Option<Key> {
        if usage_code != 0
            && let Some(key) = self.keys_by_usage_code.get(&usage_code)
        {
            return Some(*key);
        }
        if scan_code != 0 {
            return self.keys_by_scan_code.get(&scan_code).copied();
        }
        None
    }

    /// `findScanCodesForKey`: the scan codes of `key_code` but function
    /// keys'.
    pub fn find_scan_codes_for_key(&self, key_code: i32) -> Vec<i32> {
        self.keys_by_scan_code
            .iter()
            .filter(|(_, k)| k.key_code == key_code && k.flags & POLICY_FLAG_FUNCTION == 0)
            .map(|(&code, _)| code)
            .collect()
    }

    /// `findUsageCodesForKey`: the usages of `key_code` but fallback
    /// mappings.
    pub fn find_usage_codes_for_key(&self, key_code: i32) -> Vec<i32> {
        self.keys_by_usage_code
            .iter()
            .filter(|(_, k)| {
                k.key_code == key_code && k.flags & POLICY_FLAG_FALLBACK_USAGE_MAPPING == 0
            })
            .map(|(&code, _)| code)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# A sample.\nkey 30   A\nkey 116  POWER  WAKE\nkey usage 0x0c0067 WINDOW FALLBACK_USAGE_MAPPING\nkey 465  ESCAPE  FUNCTION\naxis 0x00 X\naxis 0x02 split 0x80 LTRIGGER RTRIGGER flat 4\naxis 0x03 invert RX\nled 0x01 CAPS_LOCK\nsensor 0x00 ACCELEROMETER X\n";

    #[test]
    fn parses_layouts_as_the_original() {
        let map = KeyLayoutMap::load("/x.kl", SAMPLE.as_bytes()).unwrap();
        assert_eq!(
            map.map_key(30, 0),
            Some(Key {
                key_code: 29,
                flags: 0
            })
        );
        assert_eq!(
            map.map_key(116, 0),
            Some(Key {
                key_code: 26,
                flags: POLICY_FLAG_WAKE
            })
        );
        // A usage is looked up before the scan code.
        assert_eq!(map.map_key(30, 0x0c0067).map(|k| k.key_code), Some(171));
        // Fallback usages and function keys are not found for a key code.
        assert!(map.find_usage_codes_for_key(171).is_empty());
        assert!(map.find_scan_codes_for_key(111).is_empty());
        assert_eq!(map.find_scan_codes_for_key(29), [30]);
        assert_eq!(
            map.axes[&2],
            (
                Axis::Split {
                    value: 0x80,
                    low: 17,
                    high: 18
                },
                Some(4)
            )
        );
        assert_eq!(map.leds_by_scan_code[&1], 1);
        assert_eq!(map.sensors_by_abs_code[&0], (0, 0));
    }

    #[test]
    fn refuses_malformed_layouts() {
        assert!(KeyLayoutMap::load("/x.kl", b"key 30 A\nkey 30 B\n").is_err());
        assert!(KeyLayoutMap::load("/x.kl", b"key 30 NOT_A_KEY\n").is_err());
        assert!(KeyLayoutMap::load("/x.kl", b"key 30 A WAKE WAKE\n").is_err());
        assert!(KeyLayoutMap::load("/x.kl", b"bogus 1\n").is_err());
    }
}
