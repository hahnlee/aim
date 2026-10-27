//! The vibrator's policy inputs from the app host's existing owners.
use super::{AppHost, settings::Table};
use crate::vibrator::{VibrationAttributes, policy};
use aim_binder_host::parcel::Exception;

impl AppHost {
    pub(crate) fn vibration_ignored(
        &self,
        _uid: i32,
        device_id: i32,
        attrs: &VibrationAttributes,
    ) -> Result<bool, Exception> {
        let inputs = self.platform()?;
        let framework = self.framework()?;
        let setting = |key: &str, fallback| {
            inputs
                .settings
                .get(Table::System, key)
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(fallback)
        };
        let default = |kind: &str| {
            framework
                .framework_integer(&format!("config_default{kind}Intensity"))
                .filter(|v| (0..=3).contains(v))
                .unwrap_or(2)
                > 0
        };
        // Every native activity process is TOP; an unknown/gone UID uses
        // VibrationUidObserver's IMPORTANT_FOREGROUND default. No virtual
        // device runs in this host (the virtual-device owner is absent).
        Ok(device_id != 0 && device_id != -1
            || self.vibration_power_save()?
                && !matches!(
                    attrs.usage,
                    policy::ALARM
                        | policy::RINGTONE
                        | policy::COMMUNICATION
                        | policy::HARDWARE
                        | policy::PHYSICAL
                )
            || attrs.flags & 2 == 0
                && ((setting("vibrate_on", 1) <= 0 && attrs.usage != policy::ACCESSIBILITY)
                    || !policy::enabled(
                        attrs.usage,
                        framework
                            .framework_bool("config_keyboardVibrationSettingsSupported")
                            .unwrap_or(false),
                        setting,
                        default,
                    ))
            || attrs.flags & 1 == 0
                && matches!(attrs.usage, policy::RINGTONE | policy::NOTIFICATION)
                && inputs.settings.get(Table::Global, "mode_ringer").as_deref() == Some("0")
            || framework
                .framework_bool("config_ignoreVibrationsOnWirelessCharger")
                .unwrap_or(false)
                && self
                    .battery_service
                    .get()
                    .is_some_and(|b| b.on_wireless_charger()))
    }

    pub(crate) fn haptic_attributes(
        &self,
        constant: i32,
        input_source: i32,
        flags: i32,
        priv_flags: i32,
    ) -> Result<Option<VibrationAttributes>, Exception> {
        let framework = self.framework()?;
        if constant == 9
            && !framework
                .framework_bool("config_enableHapticTextHandle")
                .unwrap_or(false)
        {
            return Ok(None);
        }
        if constant == 10001
            && framework
                .framework_int_array("config_safeModeEnabledVibePattern")
                .is_none_or(|v| v.iter().any(|t| *t < 0) || !v.iter().any(|t| *t > 0))
        {
            return Ok(None);
        }
        let (state, _) = self.packages()?;
        let flag = |name: &str| {
            state
                .system
                .flags
                .iter()
                .any(|(n, enabled)| n == name && *enabled)
        };
        let input_source =
            if flag("android.os.vibrator.haptic_feedback_input_source_customization_enabled") {
                input_source
            } else {
                0
            };
        let mut attrs =
            crate::vibrator::haptic_attributes(constant, input_source, flags, priv_flags);
        if !flag("android.view.flags.scroll_feedback_api") {
            if let Some(attrs) = &mut attrs {
                attrs.flags &= !1;
            }
        }
        Ok(attrs)
    }
    fn vibration_power_save(&self) -> Result<bool, Exception> {
        let inputs = self.platform()?;
        let framework = self.framework()?;
        if !framework
            .framework_bool("config_batterySaverSupported")
            .unwrap_or(true)
            || inputs.settings.get(Table::Global, "low_power").as_deref() != Some("1")
            || inputs
                .settings
                .get(Table::Secure, "accessibility_enabled")
                .as_deref()
                == Some("1")
        {
            return Ok(false);
        }
        let configured = inputs
            .settings
            .config_flags(Some("battery_saver/"))?
            .get("battery_saver/disable_vibration")
            .and_then(|v| v.as_deref())
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|| {
                framework
                    .framework_bool("config_batterySaver_full_disableVibration")
                    .unwrap_or(true)
            });
        Ok(policy_bool(
            inputs
                .settings
                .get(Table::Global, "battery_saver_constants")
                .as_deref(),
            configured,
        ))
    }
}

/// KeyValueListParser.getBoolean uses Boolean.parseBoolean; malformed lists
/// are cleared, and duplicate keys replace earlier values.
fn policy_bool(constants: Option<&str>, default: bool) -> bool {
    let mut value = None;
    for entry in constants.unwrap_or("").split(',').filter(|s| !s.is_empty()) {
        let Some((name, v)) = entry.split_once('=') else {
            return default;
        };
        if name.trim() == "disable_vibration" {
            value = Some(v.trim().eq_ignore_ascii_case("true"));
        }
    }
    value.unwrap_or(default)
}
