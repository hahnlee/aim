//! What SettingsProvider checks of an app's system settings at
//! `android-16.0.0_r1`: `Settings.System.PUBLIC_SETTINGS` and
//! `PRIVATE_SETTINGS`, and the validators of the values
//! (`SystemSettingsValidators.VALIDATORS`, with `SettingsValidators`'
//! common ones), by the settings' names.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::{EX_NULL_POINTER, Exception};

/// `Settings.System.PUBLIC_SETTINGS`.
pub const PUBLIC_SETTINGS: &[&str] = &[
    "end_button_behavior",
    "wifi_use_static_ip",
    "wifi_static_ip",
    "wifi_static_gateway",
    "wifi_static_netmask",
    "wifi_static_dns1",
    "wifi_static_dns2",
    "bluetooth_discoverability",
    "bluetooth_discoverability_timeout",
    "next_alarm_formatted",
    "font_scale",
    "system_locales",
    "dim_screen",
    "screen_off_timeout",
    "screen_brightness",
    "screen_brightness_mode",
    "mode_ringer_streams_affected",
    "mute_streams_affected",
    "vibrate_on",
    "volume_ring",
    "volume_system",
    "volume_voice",
    "volume_music",
    "volume_alarm",
    "volume_notification",
    "volume_bluetooth_sco",
    "volume_assistant",
    "ringtone",
    "notification_sound",
    "alarm_alert",
    "auto_replace",
    "auto_caps",
    "auto_punctuate",
    "show_password",
    "SHOW_GTALK_SERVICE_STATUS",
    "wallpaper_activity",
    "time_12_24",
    "date_format",
    "setup_wizard_has_run",
    "accelerometer_rotation",
    "user_rotation",
    "dtmf_tone",
    "sound_effects_enabled",
    "haptic_feedback_enabled",
    "show_web_suggestions",
    "vibrate_when_ringing",
    "apply_ramping_ringer",
];

/// `Settings.System.PRIVATE_SETTINGS`.
pub const PRIVATE_SETTINGS: &[&str] = &[
    "wifi_use_static_ip",
    "end_button_behavior",
    "advanced_settings",
    "wear_accessibility_gesture_enabled",
    "wear_accessibility_gesture_enabled_during_oobe",
    "wear_tts_prewarm_enabled",
    "screen_auto_brightness_adj",
    "vibrate_input_devices",
    "volume_master",
    "master_mono",
    "master_balance",
    "notifications_use_ring_volume",
    "vibrate_in_silent",
    "media_button_receiver",
    "hide_rotation_lock_toggle_for_accessibility",
    "dtmf_tone_type",
    "hearing_aid",
    "tty_mode",
    "notification_light_pulse",
    "pointer_location",
    "show_touches",
    "show_key_presses",
    "window_orientation_listener_log",
    "power_sounds_enabled",
    "dock_sounds_enabled",
    "lockscreen_sounds_enabled",
    "lockscreen.disabled",
    "low_battery_sound",
    "desk_dock_sound",
    "desk_undock_sound",
    "car_dock_sound",
    "car_undock_sound",
    "lock_sound",
    "unlock_sound",
    "sip_receive_calls",
    "sip_call_options",
    "SIP_ALWAYS",
    "SIP_ADDRESS_ONLY",
    "SIP_ASK_ME_EACH_TIME",
    "pointer_speed",
    "pointer_fill_style",
    "pointer_stroke_style",
    "pointer_scale",
    "lock_to_app_enabled",
    "egg_mode",
    "status_bar_show_battery_percent",
    "display_color_mode",
    "display_color_mode_vendor_hint",
    "locale_preferences",
    "touchpad_pointer_speed",
    "touchpad_natural_scrolling",
    "touchpad_tap_to_click",
    "touchpad_tap_dragging",
    "touchpad_right_click_zone",
    "touchpad_system_gestures",
    "touchpad_acceleration_enabled",
    "camera_flash_notification",
    "screen_flash_notification",
    "screen_flash_notification_color_global",
    "device_font_scale",
    "mouse_reverse_vertical_scrolling",
    "mouse_swap_primary_button",
    "mouse_pointer_acceleration_enabled",
    "preferred_region",
    "mouse_scrolling_acceleration",
    "mouse_scrolling_speed",
];

/// A `Validator`.
#[derive(Clone, Copy)]
enum Validator {
    /// `DiscreteValueValidator`; `BOOLEAN_VALIDATOR` is `{"0", "1"}`.
    Discrete(&'static [&'static str]),
    AnyString,
    NonNegativeInteger,
    AnyInteger,
    NonNegativeFloat,
    /// `URI_VALIDATOR`: `Uri.decode` takes any string.
    Uri,
    ComponentName,
    LenientIpAddress,
    InclusiveIntegerRange(i32, i32),
    InclusiveFloatRange(f32, f32),
    StayOnWhilePluggedIn,
    NextAlarmFormatted,
    DisplayColorMode,
    WallpaperActivity,
    EggMode,
}

use Validator::*;

const BOOLEAN: Validator = Discrete(&["0", "1"]);
const FONT_SCALE: Validator = InclusiveFloatRange(0.25, 5.0);
const VIBRATION_INTENSITY: Validator = InclusiveIntegerRange(0, 3);
/// `BatteryManager.BATTERY_PLUGGED_AC`, `_USB`, `_WIRELESS`, `_DOCK`.
const AC: i32 = 1;
const USB: i32 = 2;
const WIRELESS: i32 = 4;
const DOCK: i32 = 8;
/// `ColorDisplayManager.COLOR_MODE_NATURAL`, `COLOR_MODE_AUTOMATIC`,
/// `VENDOR_COLOR_MODE_RANGE_MIN`, `VENDOR_COLOR_MODE_RANGE_MAX`.
const COLOR_MODES: std::ops::RangeInclusive<i32> = 0..=3;
const VENDOR_COLOR_MODES: std::ops::RangeInclusive<i32> = 256..=511;

/// `SystemSettingsValidators.VALIDATORS`.
const VALIDATORS: &[(&str, Validator)] = &[
    ("stay_on_while_plugged_in", StayOnWhilePluggedIn),
    ("end_button_behavior", InclusiveIntegerRange(0, 3)),
    ("wifi_use_static_ip", BOOLEAN),
    ("bluetooth_discoverability", InclusiveIntegerRange(0, 2)),
    ("bluetooth_discoverability_timeout", NonNegativeInteger),
    ("next_alarm_formatted", NextAlarmFormatted),
    ("device_font_scale", FONT_SCALE),
    ("font_scale", FONT_SCALE),
    ("dim_screen", BOOLEAN),
    ("display_color_mode", DisplayColorMode),
    ("display_color_mode_vendor_hint", AnyString),
    ("screen_off_timeout", NonNegativeInteger),
    ("screen_brightness_mode", BOOLEAN),
    // `SCREEN_BRIGHTNESS_AUTOMATIC_BRIGHT` to `_DIM`.
    ("screen_brightness_for_als", InclusiveIntegerRange(1, 3)),
    ("adaptive_sleep", BOOLEAN),
    ("mode_ringer_streams_affected", NonNegativeInteger),
    ("mute_streams_affected", NonNegativeInteger),
    ("vibrate_on", BOOLEAN),
    ("apply_ramping_ringer", BOOLEAN),
    ("alarm_vibration_intensity", VIBRATION_INTENSITY),
    ("media_vibration_intensity", VIBRATION_INTENSITY),
    ("notification_vibration_intensity", VIBRATION_INTENSITY),
    ("ring_vibration_intensity", VIBRATION_INTENSITY),
    ("haptic_feedback_intensity", VIBRATION_INTENSITY),
    ("hardware_haptic_feedback_intensity", VIBRATION_INTENSITY),
    ("keyboard_vibration_enabled", BOOLEAN),
    ("haptic_feedback_enabled", BOOLEAN),
    ("ringtone", Uri),
    ("notification_sound", Uri),
    ("fold_lock_behavior_setting", AnyString),
    ("alarm_alert", Uri),
    ("auto_replace", BOOLEAN),
    ("auto_caps", BOOLEAN),
    ("auto_punctuate", BOOLEAN),
    ("show_password", BOOLEAN),
    ("auto_time", BOOLEAN),
    ("auto_time_zone", BOOLEAN),
    ("SHOW_GTALK_SERVICE_STATUS", BOOLEAN),
    ("wallpaper_activity", WallpaperActivity),
    ("time_12_24", Discrete(&["12", "24"])),
    ("setup_wizard_has_run", BOOLEAN),
    ("accelerometer_rotation", BOOLEAN),
    ("user_rotation", InclusiveIntegerRange(0, 3)),
    ("dtmf_tone", BOOLEAN),
    ("sound_effects_enabled", BOOLEAN),
    ("power_sounds_enabled", BOOLEAN),
    ("dock_sounds_enabled", BOOLEAN),
    ("show_web_suggestions", BOOLEAN),
    ("advanced_settings", BOOLEAN),
    ("screen_auto_brightness_adj", InclusiveFloatRange(-1.0, 1.0)),
    ("vibrate_input_devices", BOOLEAN),
    ("master_mono", BOOLEAN),
    ("master_balance", InclusiveFloatRange(-1.0, 1.0)),
    ("notifications_use_ring_volume", BOOLEAN),
    ("vibrate_in_silent", BOOLEAN),
    ("media_button_receiver", ComponentName),
    ("hide_rotation_lock_toggle_for_accessibility", BOOLEAN),
    ("vibrate_when_ringing", BOOLEAN),
    ("dtmf_tone_type", BOOLEAN),
    ("hearing_aid", BOOLEAN),
    ("tty_mode", InclusiveIntegerRange(0, 3)),
    ("notification_light_pulse", BOOLEAN),
    ("pointer_location", BOOLEAN),
    ("show_touches", BOOLEAN),
    ("show_key_presses", BOOLEAN),
    ("touchpad_visualizer", BOOLEAN),
    ("show_rotary_input", BOOLEAN),
    ("window_orientation_listener_log", BOOLEAN),
    ("lockscreen_sounds_enabled", BOOLEAN),
    ("lockscreen.disabled", BOOLEAN),
    ("sip_receive_calls", BOOLEAN),
    ("sip_call_options", Discrete(&["SIP_ALWAYS", "SIP_ADDRESS_ONLY"])),
    ("SIP_ALWAYS", BOOLEAN),
    ("SIP_ADDRESS_ONLY", BOOLEAN),
    ("SIP_ASK_ME_EACH_TIME", BOOLEAN),
    ("pointer_speed", InclusiveFloatRange(-7.0, 7.0)),
    ("touchpad_three_finger_tap_customization", NonNegativeInteger),
    // `PointerIcon.POINTER_ICON_VECTOR_STYLE_FILL_BEGIN` to `_END`,
    // `_STROKE_BEGIN` to `_END`, `DEFAULT_POINTER_SCALE` to
    // `LARGE_POINTER_SCALE`.
    ("pointer_fill_style", InclusiveIntegerRange(0, 5)),
    ("pointer_stroke_style", InclusiveIntegerRange(0, 2)),
    ("pointer_scale", InclusiveFloatRange(1.0, 2.5)),
    ("mouse_reverse_vertical_scrolling", BOOLEAN),
    ("mouse_swap_primary_button", BOOLEAN),
    ("mouse_scrolling_acceleration", BOOLEAN),
    ("mouse_pointer_acceleration_enabled", BOOLEAN),
    ("mouse_scrolling_speed", InclusiveIntegerRange(-7, 7)),
    ("touchpad_pointer_speed", InclusiveIntegerRange(-7, 7)),
    ("touchpad_natural_scrolling", BOOLEAN),
    ("touchpad_tap_to_click", BOOLEAN),
    ("touchpad_tap_dragging", BOOLEAN),
    ("touchpad_right_click_zone", BOOLEAN),
    ("touchpad_system_gestures", BOOLEAN),
    ("touchpad_acceleration_enabled", BOOLEAN),
    ("lock_to_app_enabled", BOOLEAN),
    ("egg_mode", EggMode),
    ("wifi_static_ip", LenientIpAddress),
    ("wifi_static_gateway", LenientIpAddress),
    ("wifi_static_netmask", LenientIpAddress),
    ("wifi_static_dns1", LenientIpAddress),
    ("wifi_static_dns2", LenientIpAddress),
    ("status_bar_show_battery_percent", BOOLEAN),
    ("wear_accessibility_gesture_enabled", BOOLEAN),
    ("wear_accessibility_gesture_enabled_during_oobe", BOOLEAN),
    ("wear_tts_prewarm_enabled", BOOLEAN),
    ("cw_bt_settings_pref", BOOLEAN),
    ("unread_notification_dot_indicator", BOOLEAN),
    ("auto_launch_media_controls", BOOLEAN),
    ("locale_preferences", AnyString),
    ("camera_flash_notification", BOOLEAN),
    ("screen_flash_notification", BOOLEAN),
    ("screen_flash_notification_color_global", AnyInteger),
    ("peak_refresh_rate", NonNegativeFloat),
    ("min_refresh_rate", NonNegativeFloat),
    ("notification_cooldown_enabled", BOOLEAN),
    ("notification_cooldown_all", BOOLEAN),
    ("notification_cooldown_vibrate_unlocked", BOOLEAN),
    ("preferred_region", AnyString),
    ("cv_enabled", InclusiveIntegerRange(0, 1)),
];

/// `Integer.parseInt`, `Long.parseLong` (an optional sign and digits);
/// `Float.parseFloat`.
fn int(value: &str) -> Option<i32> {
    value.parse().ok()
}

fn float(value: &str) -> Option<f32> {
    aim_android_xml::double::parse_float(value)
}

/// `ComponentName.unflattenFromString(value) != null`.
fn component_name(value: &str) -> bool {
    value.find('/').is_some_and(|sep| sep + 1 < value.len())
}

impl Validator {
    /// `validate`; an error where the original throws.
    fn validate(self, value: Option<&str>) -> Result<bool, Exception> {
        Ok(match (self, value) {
            (Discrete(values), value) => value.is_none_or(|v| values.contains(&v)),
            (AnyString | Uri, _) => true,
            (NonNegativeInteger | AnyInteger | NonNegativeFloat | LenientIpAddress, None) => true,
            (InclusiveIntegerRange(..) | InclusiveFloatRange(..) | NextAlarmFormatted, None) => true,
            (NonNegativeInteger, Some(v)) => int(v).is_some_and(|v| v >= 0),
            (AnyInteger, Some(v)) => int(v).is_some(),
            (NonNegativeFloat, Some(v)) => float(v).is_some_and(|v| v >= 0.0),
            (ComponentName, value) => value.is_some_and(component_name),
            // `MAX_IPV6_LENGTH`.
            (LenientIpAddress, Some(v)) => v.encode_utf16().count() <= 45,
            (InclusiveIntegerRange(min, max), Some(v)) => int(v).is_some_and(|v| (min..=max).contains(&v)),
            (InclusiveFloatRange(min, max), Some(v)) => float(v).is_some_and(|v| v >= min && v <= max),
            (StayOnWhilePluggedIn, value) => value.and_then(int).is_some_and(|v| {
                [0, AC, USB, WIRELESS, DOCK, AC | USB, AC | WIRELESS, USB | WIRELESS, AC | USB | WIRELESS, AC | DOCK, USB | DOCK]
                    .contains(&v)
            }),
            (NextAlarmFormatted, Some(v)) => v.encode_utf16().count() < 1000,
            (DisplayColorMode, value) => {
                value.and_then(int).is_some_and(|v| COLOR_MODES.contains(&v) || VENDOR_COLOR_MODES.contains(&v))
            }
            (WallpaperActivity, Some(v)) => v.encode_utf16().count() <= 1000 && component_name(v),
            (WallpaperActivity, None) => {
                return Err(Exception::new(EX_NULL_POINTER, "ComponentName.unflattenFromString(null)"));
            }
            (EggMode, value) => value.and_then(|v| v.parse::<i64>().ok()).is_some_and(|v| v >= 0),
        })
    }
}

#[cfg(test)]
pub(super) fn validated_names() -> impl Iterator<Item = &'static str> {
    VALIDATORS.iter().map(|(name, _)| *name)
}

/// `validateSystemSettingValue`: an `IllegalArgumentException` for a value
/// its setting's validator refuses.
pub fn validate_system_value(name: &str, value: Option<&str>) -> Result<(), Exception> {
    let Some((_, validator)) = VALIDATORS.iter().find(|(n, _)| *n == name) else {
        return Ok(());
    };
    if validator.validate(value)? {
        return Ok(());
    }
    Err(Exception::illegal_argument(format!(
        "Invalid value: {} for setting: {name}",
        value.unwrap_or("null")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SettingsValidatorsTest`, `SystemSettingsValidatorsTest`.
    #[test]
    fn values_are_validated() {
        let ok = |name, value| validate_system_value(name, value).is_ok();
        assert!(ok("screen_off_timeout", Some("60000")));
        assert!(!ok("screen_off_timeout", Some("-1")));
        assert!(ok("screen_off_timeout", None));
        assert!(ok("haptic_feedback_enabled", Some("1")));
        assert!(!ok("haptic_feedback_enabled", Some("true")));
        assert!(ok("font_scale", Some("1.3")));
        assert!(!ok("font_scale", Some("5.5")));
        assert!(!ok("font_scale", Some("NaN")));
        assert!(ok("time_12_24", None));
        assert!(!ok("time_12_24", Some("13")));
        assert!(ok("stay_on_while_plugged_in", Some("3")));
        assert!(!ok("stay_on_while_plugged_in", Some("12")));
        assert!(!ok("stay_on_while_plugged_in", None));
        assert!(ok("display_color_mode", Some("300")));
        assert!(!ok("display_color_mode", Some("4")));
        assert!(ok("media_button_receiver", Some("a/.B")));
        assert!(!ok("media_button_receiver", Some("a/")));
        assert!(ok("egg_mode", Some("5000000000")));
        assert!(validate_system_value("wallpaper_activity", None).unwrap_err().code == EX_NULL_POINTER);
        assert!(ok("anything_else", Some("x")));
        assert_eq!(
            validate_system_value("dim_screen", Some("2")).unwrap_err().message,
            "Invalid value: 2 for setting: dim_screen"
        );
    }
}
