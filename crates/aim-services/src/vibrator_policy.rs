//! Acceptance rules from VibrationSettings, android-16.0.0_r1.
//! Adapted from AOSP (Copyright The Android Open Source Project, Apache-2.0).
pub(crate) const ALARM: i32 = 17;
pub(crate) const RINGTONE: i32 = 33;
pub(crate) const NOTIFICATION: i32 = 49;
pub(crate) const COMMUNICATION: i32 = 65;
pub(crate) const TOUCH: i32 = 18;
pub(crate) const PHYSICAL: i32 = 34;
pub(crate) const HARDWARE: i32 = 50;
pub(crate) const ACCESSIBILITY: i32 = 66;
pub(crate) const IME: i32 = 82;

/// Only intensity zero affects acceptance; positive values affect playback.
pub(crate) fn enabled(
    usage: i32,
    keyboard_supported: bool,
    setting: impl Fn(&str, i32) -> i32,
    default: impl Fn(&str) -> bool,
) -> bool {
    let load = |key, fallback| match setting(key, -1) {
        value @ 0..=3 => value > 0,
        _ => fallback,
    };
    let haptic_default = default("HapticFeedback");
    let haptic = load("haptic_feedback_intensity", haptic_default);
    let notification_default = default("NotificationVibration");
    let notification = load("notification_vibration_intensity", notification_default);
    match usage {
        ALARM => load("alarm_vibration_intensity", default("AlarmVibration")),
        RINGTONE => load("ring_vibration_intensity", default("RingVibration")),
        NOTIFICATION => notification,
        COMMUNICATION => notification || notification_default,
        HARDWARE | PHYSICAL => load(
            "hardware_haptic_feedback_intensity",
            haptic || haptic_default,
        ),
        ACCESSIBILITY => haptic || haptic_default,
        TOUCH => setting("haptic_feedback_enabled", 0) != 0 && haptic,
        IME if keyboard_supported => {
            setting("keyboard_vibration_enabled", 1) > 0 && default("KeyboardVibration")
        }
        IME => haptic,
        _ => load("media_vibration_intensity", default("MediaVibration")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn with_off(usage: i32, keyboard: bool, keys: &[&str]) -> bool {
        enabled(
            usage,
            keyboard,
            |key, fallback| if keys.contains(&key) { 0 } else { fallback },
            |_| true,
        )
    }
    #[test]
    fn intensity_zero_and_invalid_values_use_the_usage_specific_default() {
        for usage in [ALARM, RINGTONE, NOTIFICATION, TOUCH, IME, 0] {
            assert!(!enabled(usage, false, |_, _| 0, |_| true));
            assert!(enabled(usage, false, |_, _| 9, |_| true));
        }
    }
    #[test]
    fn communication_accessibility_and_hardware_keep_positive_fallbacks() {
        for usage in [COMMUNICATION, ACCESSIBILITY, HARDWARE, PHYSICAL] {
            assert!(with_off(
                usage,
                false,
                &[
                    "notification_vibration_intensity",
                    "haptic_feedback_intensity"
                ]
            ));
        }
    }
    #[test]
    fn touch_obeys_haptic_feedback_enabled() {
        assert!(!with_off(TOUCH, false, &["haptic_feedback_enabled"]));
    }
    #[test]
    fn keyboard_setting_applies_only_when_supported() {
        assert!(!with_off(IME, true, &["keyboard_vibration_enabled"]));
        assert!(with_off(IME, false, &["keyboard_vibration_enabled"]));
    }
}
