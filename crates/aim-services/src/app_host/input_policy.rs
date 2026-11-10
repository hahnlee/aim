//! The window policy's input while the device is not interactive, as
//! `PhoneWindowManager.interceptKeyBeforeQueueing` and
//! `interceptMotionBeforeQueueingNonInteractive` decide it at
//! android-16.0.0_r1 (`WindowWakeUpPolicy`): a wake key's down or a
//! waking motion wakes the device, and only the motion goes on to the
//! windows. The agent's dispatcher asks this for each key and motion it
//! reads while not interactive (InputDispatcher's `interceptKeyBeforeQueueing`
//! and `interceptMotionBeforeQueueing`, `!mInteractive`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::AppHost;
use super::settings::Table;

/// `WindowManagerPolicy.ACTION_PASS_TO_USER`.
pub(super) const ACTION_PASS_TO_USER: i32 = 1;
/// The reply's mark of a wake key's down, whose up the dispatcher drops
/// once interactive (`mPendingWakeKey`).
pub(super) const PENDING_WAKE_KEY: i32 = 2;
/// `WindowManagerPolicy.FLAG_WAKE` (`POLICY_FLAG_WAKE`).
const FLAG_WAKE: u32 = 0x1;
/// `PowerManager.WAKE_REASON_POWER_BUTTON`, `WAKE_REASON_WAKE_KEY`,
/// `WAKE_REASON_WAKE_MOTION`.
const WAKE_REASON_POWER_BUTTON: i32 = 1;
const WAKE_REASON_WAKE_KEY: i32 = 6;
const WAKE_REASON_WAKE_MOTION: i32 = 7;
/// `KeyEvent.KEYCODE_*`.
const KEYCODE_DPAD_UP: i32 = 19;
const KEYCODE_DPAD_CENTER: i32 = 23;
const KEYCODE_POWER: i32 = 26;
const KEYCODE_CAMERA: i32 = 27;
const KEYCODE_MENU: i32 = 82;
const KEYCODE_BACK: i32 = 4;
const KEYCODE_ASSIST: i32 = 219;
const KEYCODE_WAKEUP: i32 = 224;
const KEYCODE_PAIRING: i32 = 225;
const KEYCODE_STEM_PRIMARY: i32 = 264;
const KEYCODE_STEM_1: i32 = 265;
const KEYCODE_STEM_3: i32 = 267;
const KEYCODE_STYLUS_BUTTON_PRIMARY: i32 = 308;
const KEYCODE_STYLUS_BUTTON_TAIL: i32 = 311;

/// `KeyEvent.isWakeKey`.
fn is_wake_key(key_code: i32) -> bool {
    matches!(
        key_code,
        KEYCODE_CAMERA
            | KEYCODE_MENU
            | KEYCODE_PAIRING
            | KEYCODE_WAKEUP
            | KEYCODE_STEM_PRIMARY
            | KEYCODE_STEM_1..=KEYCODE_STEM_3
    )
}

impl AppHost {
    /// The policy's result for a key of `key_code` (`down` or up), or a
    /// motion if `motion` (`down` its initial down), at `when` (ns of the
    /// input clock) with InputReader's `policy_flags`, read while the
    /// device was not interactive: `ACTION_PASS_TO_USER` to dispatch it,
    /// `PENDING_WAKE_KEY` for a wake key's down.
    pub(super) fn intercept_non_interactive(
        &self,
        motion: bool,
        key_code: i32,
        down: bool,
        when: i64,
        policy_flags: u32,
    ) -> Result<i32, Exception> {
        let power = self
            .power_service
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no power service"))?;
        // Woken meanwhile: the interactive policy passes it on.
        if power.interactive() {
            return Ok(ACTION_PASS_TO_USER);
        }
        let framework = self.framework()?;
        let config = |name: &str| {
            framework.framework_bool(name).ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, format!("no framework bool {name}"))
            })
        };
        let settings = &self.platform()?.settings;
        let theater = settings
            .get(Table::Global, "theater_mode_on")
            .is_some_and(|v| v != "0");
        // `WindowWakeUpPolicy.canWakeUp`.
        let can_wake = |allowed_in_theater: bool| !theater || allowed_in_theater;
        let wake = policy_flags & FLAG_WAKE != 0;
        // `shouldDispatchInputWhenNonInteractive` is false: the display is
        // off whenever the device is not interactive (no doze or dream).
        if motion {
            if wake && can_wake(config("config_allowTheaterModeWakeFromMotion")?) {
                power.policy_wake_up(when / 1_000_000, WAKE_REASON_WAKE_MOTION, "MOTION");
                return Ok(ACTION_PASS_TO_USER);
            }
            return Ok(0);
        }
        // `isWakeKeyWhenScreenOff`.
        let when_screen_off = match key_code {
            KEYCODE_DPAD_UP..=KEYCODE_DPAD_CENTER => config("config_wakeOnDpadKeyPress")?,
            KEYCODE_ASSIST => config("config_wakeOnAssistKeyPress")?,
            KEYCODE_BACK => config("config_wakeOnBackKeyPress")?,
            KEYCODE_STYLUS_BUTTON_PRIMARY..=KEYCODE_STYLUS_BUTTON_TAIL => settings
                .get(Table::Secure, "stylus_buttons_enabled")
                .is_none_or(|v| v == "1"),
            _ => true,
        };
        // Not passed on; a wake key wakes the device on its down.
        let wake_key = (wake || is_wake_key(key_code)) && down && when_screen_off;
        if !wake_key {
            return Ok(0);
        }
        // `wakeUpFromWakeKey` (`WindowWakeUpPolicy.wakeUpFromKey`).
        let (allowed, reason, details) = if key_code == KEYCODE_POWER {
            (
                config("config_allowTheaterModeWakeFromPowerKey")?,
                WAKE_REASON_POWER_BUTTON,
                "POWER",
            )
        } else {
            (
                config("config_allowTheaterModeWakeFromKey")?,
                WAKE_REASON_WAKE_KEY,
                "KEY",
            )
        };
        if can_wake(allowed) {
            power.policy_wake_up(when / 1_000_000, reason, details);
        }
        Ok(PENDING_WAKE_KEY)
    }
}
