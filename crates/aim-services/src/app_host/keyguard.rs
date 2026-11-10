//! The keyguard's `ACTION_USER_PRESENT` (`KeyguardViewMediator
//! .sendUserPresentBroadcast` at `android-16.0.0_r1`). The Mac's screen
//! lock is the device's keyguard (#1887): the user is present when it is
//! dismissed, the Mac's screen unlocked, and at start or on waking while
//! it is not locked, as the original sends it on waking with no lock
//! screen to show. Unlocked while Android sleeps (it sleeps while the Mac
//! is locked, docs/power.md), the lock screen is dismissed once the wake
//! has turned the screen on: one USER_PRESENT, after SCREEN_ON, as a
//! device's keyguard sends it when dismissed after a wake. Sent before the boot has completed, it waits for SystemUI's
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
    /// Whether Android is asleep (PowerManager's interactive state).
    asleep: bool,
    /// Unlocked while asleep: dismissed once the screen is on.
    dismissed_asleep: bool,
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
    /// The Mac's screen as read at start, after its changes are watched:
    /// a change heard meanwhile is newer, and the read is dropped.
    pub(super) fn mac_screen_at_start(&self, locked: bool) {
        if self.keyguard.0.lock().unwrap().locked.is_none() {
            self.mac_screen_locked(locked);
        }
    }

    /// The Mac's screen locked or unlocked: its keyguard dismissed when it
    /// is unlocked.
    pub(super) fn mac_screen_locked(&self, locked: bool) {
        let send = {
            let mut state = self.keyguard.0.lock().unwrap();
            let dismissed = !locked && state.locked != Some(false);
            state.locked = Some(locked);
            state.dismissed_asleep = dismissed && state.asleep;
            dismissed && !state.asleep && state.user_present()
        };
        if send {
            self.send_user_present();
        }
    }

    /// `maybeSendUserPresentBroadcast` on waking: with no lock screen to
    /// show, the Mac's screen unlocked, the user is present.
    pub(super) fn keyguard_woke(&self) {
        let send = {
            let mut state = self.keyguard.0.lock().unwrap();
            state.asleep = false;
            !state.dismissed_asleep && state.locked == Some(false) && state.user_present()
        };
        if send {
            self.send_user_present();
        }
    }

    /// Notifier's SCREEN_ON, sent: a lock screen dismissed while asleep
    /// has now been shown and dismissed.
    pub(super) fn keyguard_screen_on(&self) {
        let send = {
            let mut state = self.keyguard.0.lock().unwrap();
            std::mem::take(&mut state.dismissed_asleep) && state.user_present()
        };
        if send {
            self.send_user_present();
        }
    }

    /// The policy's `startedGoingToSleep`.
    pub(super) fn keyguard_slept(&self) {
        self.keyguard.0.lock().unwrap().asleep = true;
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
