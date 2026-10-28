//! The device's and user 0's boot as the app host finishes it, in the
//! original's order (`android-16.0.0_r1`): `ActivityManagerService
//! .systemReady`'s broadcasts of the system user's start, then
//! `finishBooting` (`sys.boot_completed`, `dev.bootcomplete`) and
//! `UserController`'s `onBootComplete` → `finishUserBoot`
//! (`LOCKED_BOOT_COMPLETED` while the user is locked) → `maybeUnlockUser`
//! → `finishUserUnlocked` (`USER_UNLOCKED`, then `PRE_BOOT_COMPLETED` to the
//! system apps after a system update, `PreBootBroadcaster`) →
//! `finishUserUnlockedCompleted` (`onUserLoggedIn`, `BOOT_COMPLETED`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::dev_aim_server_iprebootnotifications as notification;
use std::sync::Arc;

use super::AppHost;
use super::broadcasts::{
    Done, TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED, TempAllowlist,
};
use super::kept::RawIntent;
use super::settings::Table;
use super::storage::StartedUser;
use crate::bundle::Value;
use crate::package::component_resolver::Info;

const USER: i32 = 0;
const USER_ALL: i32 = -1;
/// `Intent.EXTRA_USER_HANDLE`.
const EXTRA_USER_HANDLE: &str = "android.intent.extra.user_handle";
/// `Intent.FLAG_*`.
const FLAG_DEBUG_TRIAGED_MISSING: i32 = 0x0000_0100;
const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
const FLAG_RECEIVER_BOOT_UPGRADE: i32 = 0x0200_0000;
const FLAG_RECEIVER_NO_ABORT: i32 = 0x0800_0000;
const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const FLAG_RECEIVER_OFFLOAD: i32 = 0x8000_0000_u32 as i32;
const RECEIVE_BOOT_COMPLETED: &str = "android.permission.RECEIVE_BOOT_COMPLETED";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
/// `PowerExemptionManager.REASON_*`.
const REASON_BOOT_COMPLETED: i32 = 200;
const REASON_PRE_BOOT_COMPLETED: i32 = 201;
const REASON_LOCKED_BOOT_COMPLETED: i32 = 202;
/// `ActivityManagerConstants.DEFAULT_BOOT_TIME_TEMP_ALLOWLIST_DURATION`.
const DEFAULT_BOOT_TIME_TEMP_ALLOWLIST_DURATION: i64 = 20 * 1000;
/// `PackageManager.MATCH_SYSTEM_ONLY`.
const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
const SYSTEM_UID: i32 = 1000;

/// A user broadcast's intent: its action and flags, the user's handle.
fn user_intent(action: &str, flags: i32) -> RawIntent {
    RawIntent::new(action, flags, &[(EXTRA_USER_HANDLE, Value::Int(USER))])
}

/// The intents of a booting user, as `UserController` makes them.
fn locked_boot_completed() -> RawIntent {
    user_intent(
        "android.intent.action.LOCKED_BOOT_COMPLETED",
        FLAG_RECEIVER_NO_ABORT | FLAG_RECEIVER_OFFLOAD | FLAG_RECEIVER_INCLUDE_BACKGROUND,
    )
}

fn boot_completed() -> RawIntent {
    user_intent(
        "android.intent.action.BOOT_COMPLETED",
        FLAG_RECEIVER_NO_ABORT | FLAG_RECEIVER_INCLUDE_BACKGROUND | FLAG_RECEIVER_OFFLOAD,
    )
}

fn user_unlocked() -> RawIntent {
    user_intent(
        "android.intent.action.USER_UNLOCKED",
        FLAG_RECEIVER_REGISTERED_ONLY | FLAG_RECEIVER_FOREGROUND,
    )
}

/// `PreBootBroadcaster`'s intent.
fn pre_boot_completed() -> RawIntent {
    RawIntent::new(
        "android.intent.action.PRE_BOOT_COMPLETED",
        FLAG_RECEIVER_BOOT_UPGRADE | FLAG_DEBUG_TRIAGED_MISSING,
        &[],
    )
}

impl AppHost {
    /// `ActivityManagerInternal.getBootTimeTempAllowListDuration`
    /// (DeviceConfig `activity_manager/boot_time_temp_allowlist_duration`).
    pub(super) fn boot_time_temp_allowlist(&self, reason: i32) -> TempAllowlist {
        let duration_ms = self
            .inputs()
            .ok()
            .and_then(|i| {
                i.settings
                    .get(
                        Table::Config,
                        "activity_manager/boot_time_temp_allowlist_duration",
                    )
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(DEFAULT_BOOT_TIME_TEMP_ALLOWLIST_DURATION);
        // `getTemporaryAppAllowlistBroadcastOptions`.
        TempAllowlist {
            duration_ms,
            kind: TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED,
            reason,
        }
    }

    /// `systemReady`'s `sendUserStartedBroadcast` and
    /// `sendUserStartingBroadcast` of the system user: to registered
    /// receivers only, the latter to all users' holding
    /// INTERACT_ACROSS_USERS, its result receiver doing nothing.
    pub(super) fn send_user_start_broadcasts(&self) {
        self.broadcasts.send_from_system_as_user(
            self,
            user_intent(
                "android.intent.action.USER_STARTED",
                FLAG_RECEIVER_REGISTERED_ONLY | FLAG_RECEIVER_FOREGROUND,
            ),
            USER,
        );
        self.broadcasts.send_from_system_with(
            self,
            user_intent(
                "android.intent.action.USER_STARTING",
                FLAG_RECEIVER_REGISTERED_ONLY,
            ),
            USER_ALL,
            &[INTERACT_ACROSS_USERS.to_string()],
            None,
            None,
        );
    }

    /// `finishBooting` after the services' `PHASE_BOOT_COMPLETED`: the
    /// boot's properties, then `onBootComplete`'s `finishUserBoot` of the
    /// system user, `LOCKED_BOOT_COMPLETED` while it is locked, its unlock,
    /// and the rest of its boot as `UserController` goes through it.
    /// `unlocking` runs the services' `onUserUnlocking`
    /// (`USER_UNLOCK_MSG`); the activity-task owner's `onUserUnlocked`
    /// follows in `finish_user_unlocked_completed` (`USER_UNLOCKED_MSG`).
    pub(super) fn finish_booting(
        self: &Arc<Self>,
        user: StartedUser,
        unlocking: impl FnOnce(),
    ) -> Result<(), String> {
        let capture = self.package_capture().map_err(|error| error.message)?;
        capture
            .state()
            .system
            .lifecycle
            .as_ref()
            .ok_or("boot preferred broadcast lifecycle unavailable")?
            .complete_boot_preferred_activity_changes(|| {
                self.broadcasts.send_from_system_as_user(
                    self,
                    RawIntent::new(
                        "android.intent.action.ACTION_PREFERRED_ACTIVITY_CHANGED",
                        0x0400_0000,
                        &[("android.intent.extra.user_handle", Value::Int(USER_ALL))],
                    ),
                    USER_ALL,
                );
                Ok(())
            })?;
        // "Tell anyone interested that we are done booting!"
        self.set_property("sys.boot_completed", "1");
        self.set_property("dev.bootcomplete", "1");
        // `finishUserBoot`: STATE_RUNNING_LOCKED, the locked boot's
        // broadcast to receivers holding RECEIVE_BOOT_COMPLETED (the
        // direct boot aware, the user being locked); AMS's result receiver
        // only times the boot.
        self.broadcasts.send_from_system_with(
            self,
            locked_boot_completed(),
            USER,
            &[RECEIVE_BOOT_COMPLETED.to_string()],
            Some(self.boot_time_temp_allowlist(REASON_LOCKED_BOOT_COMPLETED)),
            None,
        );
        // `maybeUnlockUser` → `finishUserUnlocking`: the CE storage, the
        // user unlocking.
        self.unlock_user_storage(&user)?;
        unlocking();
        // `finishUserUnlocked`: STATE_RUNNING_UNLOCKED (the app host runs
        // no persistent apps or encryption unaware providers to start), to
        // the registered receivers.
        self.broadcasts
            .send_from_system_as_user(self, user_unlocked(), USER);
        // A system update's pre-boot receivers first.
        let updated = user.updated || self.mock_upgrade();
        let host = self.clone();
        let completed: Done = Box::new(move || host.finish_user_unlocked_completed(user));
        if updated {
            self.send_pre_boot(completed);
        } else {
            completed();
        }
        Ok(())
    }

    /// `finishUserUnlockedCompleted`: the user logged in, then
    /// BOOT_COMPLETED to receivers holding RECEIVE_BOOT_COMPLETED (no
    /// widgets to start first: `startUserWidgets` updates bound widgets
    /// only, and none is bound, #1789).
    fn finish_user_unlocked_completed(self: &Arc<Self>, user: StartedUser) {
        if let Err(e) = self.user_logged_in(&user) {
            eprintln!("app host: user: logged in: {e}");
        }
        // `finishUserUnlockedCompleted` queues USER_UNLOCKED_MSG after
        // `onUserLoggedIn`, before posting BOOT_COMPLETED.
        if let Err(e) = self.activity_task_user_unlocked(USER) {
            eprintln!("app host: activity task: user unlocked: {}", e.message);
        }
        eprintln!("app host: Posting BOOT_COMPLETED user #{USER}");
        self.broadcasts.send_from_system_with(
            self,
            boot_completed(),
            USER,
            &[RECEIVE_BOOT_COMPLETED.to_string()],
            Some(self.boot_time_temp_allowlist(REASON_BOOT_COMPLETED)),
            None,
        );
    }

    /// `PreBootBroadcaster`: PRE_BOOT_COMPLETED to each system app's
    /// receiver of it in turn, by its component, serialized, then `done`.
    fn send_pre_boot(self: &Arc<Self>, done: Done) {
        let targets = match self.pre_boot_receivers() {
            Ok(targets) => targets,
            Err(e) => {
                eprintln!("app host: pre-boot receivers: {e}");
                return done();
            }
        };
        self.send_pre_boot_from(Arc::new(targets), 0, done);
    }

    fn send_pre_boot_from(
        self: &Arc<Self>,
        targets: Arc<Vec<(String, String)>>,
        at: usize,
        done: Done,
    ) {
        if let Err(e) = self.pre_boot_notification(targets.len(), at) {
            eprintln!("app host: pre-boot notification: {e}");
        }
        let Some((package, class)) = targets.get(at) else {
            return done();
        };
        eprintln!("app host: Pre-boot of {package}/{class} for user {USER}");
        let host = self.clone();
        let rest = targets.clone();
        self.broadcasts.send_from_system_with(
            self,
            pre_boot_completed().with_component(package, class),
            USER,
            &[],
            Some(self.boot_time_temp_allowlist(REASON_PRE_BOOT_COMPLETED)),
            Some(Box::new(move || {
                host.send_pre_boot_from(rest, at + 1, done)
            })),
        );
    }

    fn pre_boot_notification(&self, total: usize, index: usize) -> Result<(), String> {
        let service = super::display::check_service(&self.process, "aim.pre_boot_notifications")?;
        let mut p = Parcel::new();
        notification::Update {
            user_id: USER,
            total: total
                .try_into()
                .map_err(|_| "too many pre-boot receivers")?,
            index: index
                .try_into()
                .map_err(|_| "too many pre-boot receivers")?,
        }
        .write(&mut p);
        let reply = service
            .transact(notification::UPDATE, &p, false)
            .map_err(|e| format!("update: {e}"))?;
        notification::read_update_reply(&mut reply.reader())
            .map_err(|e| format!("update reply: {e}"))?
            .map_err(|e| e.message)
    }

    /// `persist.pm.mock-upgrade`, which sends the pre-boot broadcast as
    /// after a system update.
    fn mock_upgrade(&self) -> bool {
        self.property("persist.pm.mock-upgrade")
            .is_some_and(|v| property_true(&v))
    }

    /// `queryBroadcastReceiversAsUser(PRE_BOOT_COMPLETED,
    /// MATCH_SYSTEM_ONLY, user)`: their packages and classes, in order.
    fn pre_boot_receivers(&self) -> Result<Vec<(String, String)>, String> {
        let (_, resolution) = self.packages().map_err(|e| e.message)?;
        let intent = pre_boot_completed();
        let found = resolution
            .query_intent_receivers(&intent.intent, None, MATCH_SYSTEM_ONLY, USER, SYSTEM_UID)
            .map_err(|e| format!("{e:?}"))?;
        Ok(found
            .into_iter()
            .filter_map(|ri| match ri.info {
                Info::Activity(info) => Some((
                    info.info.application_info.item.package_name.clone()?,
                    info.info.item.name.clone()?,
                )),
                _ => None,
            })
            .collect())
    }
}

/// `SystemProperties.getBoolean`'s true values.
fn property_true(value: &str) -> bool {
    matches!(value, "1" | "y" | "yes" | "on" | "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The broadcasts' flags and extra, as `UserController` sets them.
    #[test]
    fn user_broadcasts_are_made_as_the_original_makes_them() {
        let locked = locked_boot_completed();
        assert_eq!(
            locked.intent.action.as_deref(),
            Some("android.intent.action.LOCKED_BOOT_COMPLETED")
        );
        assert_eq!(locked.intent.flags as u32, 0x8900_0000);
        let boot = boot_completed();
        assert_eq!(boot.intent.flags as u32, 0x8900_0000);
        assert_eq!(user_unlocked().intent.flags as u32, 0x5000_0000);
        assert_eq!(pre_boot_completed().intent.flags, 0x0200_0100);
    }

    #[test]
    fn mock_upgrade_reads_as_system_properties_reads_a_boolean() {
        assert!(property_true("1") && property_true("on") && property_true("true"));
        assert!(!property_true("0") && !property_true("") && !property_true("TRUE"));
    }
}
