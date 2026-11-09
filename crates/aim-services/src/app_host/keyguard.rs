//! The keyguard's `ACTION_USER_PRESENT` (`KeyguardViewMediator
//! .sendUserPresentBroadcast` at `android-16.0.0_r1`). The Mac's screen
//! lock is the device's keyguard (#1887): the user is present when it is
//! dismissed, the Mac's screen unlocked, and at start when it is not
//! locked, as the original sends it on waking with no lock screen to
//! show. Sent before the boot has completed, it waits for SystemUI's
//! `onBootCompleted` (`mBootSendUserPresent`). `LockPatternUtils
//! .userPresent`, which reports the unlock to strong auth, is #1888.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Mutex;

use super::AppHost;
use super::kept::RawIntent;

const ACTION_USER_PRESENT: &str = "android.intent.action.USER_PRESENT";
/// `Intent.FLAG_RECEIVER_REPLACE_PENDING`,
/// `FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT`,
/// `FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS`.
const USER_PRESENT_FLAGS: i32 = 0x2000_0000 | 0x0400_0000 | 0x0020_0000;
/// The current user, the app host's only one; it has no profiles.
const USER: i32 = 0;

#[derive(Default)]
pub(super) struct Keyguard(Mutex<State>);

#[derive(Default)]
struct State {
    /// `mBootCompleted`.
    boot_completed: bool,
    /// `mBootSendUserPresent`.
    pending: bool,
    /// Whether the Mac's screen is locked, once told.
    locked: Option<bool>,
}

impl State {
    /// `sendUserPresentBroadcast`: whether to send it now, or else it
    /// waits for the boot.
    fn user_present(&mut self) -> bool {
        self.pending |= !self.boot_completed;
        self.boot_completed
    }
}

impl AppHost {
    /// The Mac's screen as it is at start, then each time it locks or
    /// unlocks: its keyguard dismissed when it is unlocked.
    pub(super) fn mac_screen_locked(&self, locked: bool) {
        let send = {
            let mut state = self.keyguard.0.lock().unwrap();
            let dismissed = !locked && state.locked != Some(false);
            state.locked = Some(locked);
            dismissed && state.user_present()
        };
        if send {
            self.send_user_present();
        }
    }

    /// SystemUI's `onBootCompleted`, on the boot's BOOT_COMPLETED: the
    /// user present while it was booting is sent now.
    pub(super) fn keyguard_boot_completed(&self) {
        let send = {
            let mut state = self.keyguard.0.lock().unwrap();
            state.boot_completed = true;
            std::mem::take(&mut state.pending)
        };
        if send {
            self.send_user_present();
        }
    }

    /// `USER_PRESENT_INTENT` to the current user and each of its profiles,
    /// with `USER_PRESENT_INTENT_OPTIONS`: no permission, the most recent of
    /// its delivery group, which is a filter-equal intent's (this one has
    /// only its action). Their `DEFERRAL_POLICY_UNTIL_ACTIVE` is #2827.
    fn send_user_present(&self) {
        self.broadcasts.send_from_system_as_user_most_recent(
            self,
            RawIntent::new(ACTION_USER_PRESENT, USER_PRESENT_FLAGS, &[]),
            USER,
            ACTION_USER_PRESENT,
        );
    }
}
