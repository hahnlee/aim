//! `alarm`: AlarmManagerService for the app host
//! (`AlarmManagerService` at `android-16.0.0_r1`, ADR 0013): apps' alarms
//! with their checks (the calling package, exact alarms' permissions,
//! listeners not repeating, the per-uid limit), the original's minimum
//! futurity and clamped intervals, delivered at the start of their window
//! (alarms are not batched) on the Mac's clocks (elapsed
//! realtime and wall time, as the guest's `SystemClock` and
//! `System.currentTimeMillis` read them) through their PendingIntent (with
//! `EXTRA_ALARM_COUNT`) or their `IAlarmListener`, repeating alarms
//! rescheduled, an exact alarm's PendingIntent (before exact alarms
//! needed a permission, an alarm clock's or one allowed while idle) letting
//! its app start foreground services for ten seconds; alarm clocks and `getNextAlarmClock`, with
//! `ACTION_NEXT_ALARM_CLOCK_CHANGED`; and the clock's own broadcasts,
//! `ACTION_TIME_TICK` each minute and `ACTION_DATE_CHANGED` at local
//! midnight. A change of the Mac's clock is noticed as `AlarmThread`
//! notices one (more than a second off the elapsed clock), RTC alarms then
//! moved to it and `ACTION_TIME_CHANGED` sent. The time zone follows the
//! Mac's, as the vendor init has the original's follow it
//! (`vendor.aim.mac.time_zone`, docs/mac-settings.md): `setTimeZoneImpl`
//! sets `persist.sys.timezone` and sends `ACTION_TIMEZONE_CHANGED`; apps may
//! set it too (`setTimeZone`). Batching, standby buckets, quotas and idle
//! (doze) are not run: the app host's apps are all top, the device never
//! idles. A wakeup alarm does not wake the Mac; the time is the Mac's
//! (`setTime` is not implemented, #1501).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_app_ialarmlistener as listener;
use aim_service_aidl::android_app_ialarmmanager as alarm;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::activity_user::AllowMode;
use super::kept::RawIntent;
use super::power_service::WorkSource;
use super::{AppHost, PER_USER_RANGE, reply};

/// `AlarmManager`'s types.
const RTC_WAKEUP: i32 = 0;
const RTC: i32 = 1;
const ELAPSED_REALTIME: i32 = 3;
/// `AlarmManager.FLAG_*` the service reads or sets.
const FLAG_STANDALONE: i32 = 1 << 0;
const FLAG_WAKE_FROM_IDLE: i32 = 1 << 1;
const FLAG_ALLOW_WHILE_IDLE: i32 = 1 << 2;
const FLAG_ALLOW_WHILE_IDLE_UNRESTRICTED: i32 = 1 << 3;
const FLAG_IDLE_UNTIL: i32 = 1 << 4;
const FLAG_ALLOW_WHILE_IDLE_COMPAT: i32 = 1 << 5;
const FLAG_PRIORITIZE: i32 = 1 << 6;
/// `AlarmManagerService.Constants`' defaults.
const MIN_FUTURITY: i64 = 5 * 1000;
const MIN_INTERVAL: i64 = 60 * 1000;
const INTERVAL_HOUR: i64 = 60 * 60 * 1000;
const INTERVAL_DAY: i64 = 24 * INTERVAL_HOUR;
const MAX_INTERVAL: i64 = 365 * INTERVAL_DAY;
const MAX_ALARMS_PER_UID: usize = 500;
/// Compat changes: `AlarmManager.REQUIRE_EXACT_ALARM_PERMISSION`,
/// `ENABLE_USE_EXACT_ALARM`, `SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT`.
const REQUIRE_EXACT_ALARM_PERMISSION: i64 = 171_306_433;
const ENABLE_USE_EXACT_ALARM: i64 = 218_533_173;
const SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT: i64 = 226_439_802;
/// `Intent.EXTRA_ALARM_COUNT`; the clock's broadcasts and their flags.
const EXTRA_ALARM_COUNT: &str = "android.intent.extra.ALARM_COUNT";
const ACTION_TIME_TICK: &str = "android.intent.action.TIME_TICK";
const ACTION_DATE_CHANGED: &str = "android.intent.action.DATE_CHANGED";
const ACTION_NEXT_ALARM_CLOCK_CHANGED: &str = "android.app.action.NEXT_ALARM_CLOCK_CHANGED";
const ACTION_TIME_CHANGED: &str = "android.intent.action.TIME_SET";
const ACTION_TIMEZONE_CHANGED: &str = "android.intent.action.TIMEZONE_CHANGED";
const EXTRA_TIMEZONE: &str = "time-zone";
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
/// The Mac's zone (guest-init sets it), the device's, and how sure it is
/// (`SystemTimeZone`, `TIME_ZONE_CONFIDENCE_HIGH`).
const MAC_TIME_ZONE: &str = "vendor.aim.mac.time_zone";
const TIME_ZONE: &str = "persist.sys.timezone";
const TIME_ZONE_CONFIDENCE: &str = "persist.sys.timezone_confidence";
const TIME_ZONE_CONFIDENCE_HIGH: i32 = 100;
/// How often the alarms' thread looks at the clock and the Mac's zone.
const POLL_MS: i64 = 2000;
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const FLAG_RECEIVER_REPLACE_PENDING: i32 = 0x2000_0000;
const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
const FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS: i32 = 0x0020_0000;
const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
/// `IAlarmCompleteListener`'s descriptor.
const COMPLETE_DESCRIPTOR: &str = "android.app.IAlarmCompleteListener";

/// A parcelled `PendingIntent`: its `IIntentSender`.
pub struct PendingIntentRef(Option<Binder>);

impl ReadParcelable for PendingIntentRef {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(PendingIntentRef(r.read_binder()?))
    }
}

/// `AlarmManager.AlarmClockInfo`: its trigger time (wall) and the
/// PendingIntent that shows it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ClockInfo {
    trigger: i64,
    show: Option<Binder>,
}

impl ReadParcelable for ClockInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let trigger = r.read_i64()?;
        // readParcelable: the class, then the PendingIntent.
        let show = if r.read_string16()?.is_some() {
            r.read_binder()?
        } else {
            None
        };
        Ok(ClockInfo { trigger, show })
    }
}

impl WriteParcelable for ClockInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.trigger);
        match self.show {
            Some(b) => {
                p.write_string16(Some("android.app.PendingIntent"));
                p.write_binder(Some(b));
            }
            None => p.write_string16(None),
        }
    }
}

/// An alarm (`Alarm`).
/// It is delivered at the start of its window, since alarms are not
/// batched.
struct Alarm {
    /// The elapsed time of the start of its window, and an RTC alarm's
    /// wall time (`origWhen`), which a change of the clock moves it to.
    requested: i64,
    rtc_when: Option<i64>,
    interval: i64,
    flags: i32,
    operation: Option<Binder>,
    listener: Option<(Strong, u64)>,
    clock: Option<ClockInfo>,
    uid: i32,
    package: String,
    /// Why its delivery lets its PendingIntent's app start foreground
    /// services for a while, if it does.
    fgs_allowlist: Option<i32>,
}

impl Alarm {
    fn user(&self) -> i32 {
        self.uid / PER_USER_RANGE as i32
    }
}

#[derive(Default)]
struct State {
    alarms: Vec<Alarm>,
    /// The next alarm clock last broadcast, by user.
    next_clocks: Vec<(i32, Option<ClockInfo>)>,
    /// Elapsed times of the next minute and the next local midnight.
    next_tick: i64,
    next_date: i64,
    /// The wall and elapsed times the clock was last right at
    /// (`mLastTimeChangeClockTime`, `mLastTimeChangeRealtime`).
    last_clock: (i64, i64),
    /// The Mac's zone last followed.
    mac_zone: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub struct AlarmManager {
    host: Arc<AppHost>,
    shared: Arc<Shared>,
}

unsafe extern "C" {
    fn tzset();
}

/// `SystemClock.elapsedRealtime()` and `System.currentTimeMillis()`.
pub(super) fn elapsed_ms() -> i64 {
    super::clock_ms(libc::CLOCK_MONOTONIC_RAW)
}

pub(super) fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `convertToElapsed`: a wall time to the elapsed clock.
fn to_elapsed(when: i64, kind: i32) -> i64 {
    if kind == RTC || kind == RTC_WAKEUP {
        when - wall_ms() + elapsed_ms()
    } else {
        when
    }
}

/// The elapsed time of the next minute boundary of the wall clock, and of
/// the next local midnight.
fn next_tick_and_date(now_elapsed: i64) -> (i64, i64) {
    let wall = wall_ms();
    let tick = now_elapsed + (60_000 - wall.rem_euclid(60_000));
    let secs = (wall / 1000) as libc::time_t;
    // SAFETY: localtime_r into a local struct.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    let since_midnight = (tm.tm_hour as i64 * 3600 + tm.tm_min as i64 * 60 + tm.tm_sec as i64)
        * 1000
        + wall.rem_euclid(1000);
    (tick, now_elapsed + INTERVAL_DAY - since_midnight)
}

impl AlarmManager {
    pub fn new(host: Arc<AppHost>) -> AlarmManager {
        let now = elapsed_ms();
        let (next_tick, next_date) = next_tick_and_date(now);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                next_tick,
                next_date,
                last_clock: (wall_ms(), now),
                ..Default::default()
            }),
            changed: Condvar::new(),
        });
        let weak = Arc::downgrade(&host);
        let s = shared.clone();
        let _ = std::thread::Builder::new()
            .name("alarms".into())
            .spawn(move || run(weak, s));
        AlarmManager { host, shared }
    }

    /// `AlarmManagerInternal.removeAlarmsForUid` (`removeForStoppedLocked`):
    /// the uid's alarms whose app's starts are disabled
    /// (`isAppStartModeDisabled`: an instant app whose uid runs no
    /// process), none of the system uid's.
    pub(super) fn remove_alarms_for_uid(&self, uid: i32) -> Result<(), Exception> {
        if uid == 1000 {
            return Ok(());
        }
        let (packages, _) = self.host.packages()?;
        let runs = self.host.uid_runs(uid);
        let user = uid.div_euclid(PER_USER_RANGE as i32);
        let mut state = self.shared.state.lock().unwrap();
        state.alarms.retain(|a| {
            let disabled = a.uid == uid
                && !runs
                && packages
                    .packages
                    .get(&a.package)
                    .is_some_and(|ps| crate::package::info::user_state(ps, user).instant_app);
            if disabled && let Some((l, death)) = &a.listener {
                self.host.process.clear_death(l, *death);
            }
            !disabled
        });
        self.alarm_clocks_changed(&mut state);
        Ok(())
    }

    /// `isExactAlarmChangeEnabled`: whether `package` needs a permission
    /// for exact alarms.
    fn exact_alarm_change_enabled(&self, package: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        let compat = &self.host.inputs()?.compat;
        Ok(state.packages.get(package).is_some_and(|ps| {
            compat.is_enabled(
                REQUIRE_EXACT_ALARM_PERMISSION,
                Some(package),
                ps.target_sdk_version,
            )
        }))
    }

    /// `set`'s permission for an exact alarm that needs one:
    /// `isExactAlarmChangeEnabled`, then `hasUseExactAlarmInternal`,
    /// `hasScheduleExactAlarmInternal` or
    /// `isExemptFromExactAlarmPermissionNoLock` (`canScheduleExactAlarms`
    /// asks the same).
    fn exact_allowed(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        Ok(!self.exact_alarm_change_enabled(package)?
            || self.has_use_exact_alarm(package, uid)?
            || self.has_schedule_exact_alarm(package, uid)?
            || self.exempt_from_exact_alarm_permission(uid)?)
    }

    /// `hasUseExactAlarmInternal`.
    fn has_use_exact_alarm(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        let compat = &self.host.inputs()?.compat;
        let enabled = state.packages.get(package).is_some_and(|ps| {
            compat.is_enabled(ENABLE_USE_EXACT_ALARM, Some(package), ps.target_sdk_version)
        });
        Ok(enabled
            && super::permission_checker::preflight(
                &self.host,
                "android.permission.USE_EXACT_ALARM",
                uid,
                package,
            )?)
    }

    /// `hasScheduleExactAlarmInternal`: the app id among
    /// `mExactAlarmCandidates` (an installed package of it requests
    /// SCHEDULE_EXACT_ALARM), the change enabled, then the permission when
    /// denied by default, else the op's mode, default or allowed.
    fn has_schedule_exact_alarm(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        const SCHEDULE_EXACT_ALARM: &str = "android.permission.SCHEDULE_EXACT_ALARM";
        let (state, _) = self.host.packages()?;
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        let candidate = state.packages.values().any(|ps| {
            ps.app_id == app_id
                && ps.pkg.as_ref().is_some_and(|pkg| {
                    pkg.requested_permissions
                        .iter()
                        .any(|r| r == SCHEDULE_EXACT_ALARM)
                })
        });
        if !candidate || !self.exact_alarm_change_enabled(package)? {
            return Ok(false);
        }
        let compat = &self.host.inputs()?.compat;
        let denied_by_default = state.packages.get(package).is_some_and(|ps| {
            compat.is_enabled(
                SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT,
                Some(package),
                ps.target_sdk_version,
            )
        });
        if denied_by_default {
            return super::permission_checker::preflight(
                &self.host,
                SCHEDULE_EXACT_ALARM,
                uid,
                package,
            );
        }
        let mode = super::appops_service::mode(
            &self.host,
            super::appops_service::OP_SCHEDULE_EXACT_ALARM,
            uid,
            Some(package),
        )?;
        Ok(mode == super::appops_service::MODE_DEFAULT || mode == 0)
    }

    /// `isExemptFromExactAlarmPermissionNoLock`: SystemUI's app, a core
    /// uid, or an app on the power save allowlist
    /// (`DeviceIdleController.isAppOnWhitelist`).
    fn exempt_from_exact_alarm_permission(&self, uid: i32) -> Result<bool, Exception> {
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        Ok(self.system_ui_app_id()? == Some(app_id)
            || app_id < 10_000
            || self.host.device_idle().app_on_allowlist(app_id)?)
    }

    /// `Injector.getSystemUiUid`'s app: the package of
    /// `config_systemUIServiceComponent`, a system package.
    fn system_ui_app_id(&self) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.packages()?;
        let Some(component) = self
            .host
            .framework()?
            .framework_string("config_systemUIServiceComponent")
        else {
            return Ok(None);
        };
        let package = component.split('/').next().unwrap_or_default();
        Ok(state
            .packages
            .get(package)
            .filter(|ps| {
                ps.pkg
                    .as_deref()
                    .is_some_and(|p| p.is(crate::package::pkg::booleans::SYSTEM))
            })
            .map(|ps| ps.app_id))
    }

    /// `set`: its checks, then `setImpl`.
    #[allow(clippy::too_many_arguments)]
    fn set(
        &self,
        call: &Call<'_>,
        package: Option<String>,
        kind: i32,
        when: i64,
        mut window: i64,
        mut interval: i64,
        mut flags: i32,
        operation: Option<Binder>,
        listener: Option<Binder>,
        work_source: Option<WorkSource>,
        clock: Option<ClockInfo>,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        let package = package.unwrap_or_default();
        if !self.host.owns_package(uid, &package)? {
            return Err(Exception::security(format!(
                "Package {package} does not belong to the calling uid {uid}"
            )));
        }
        if interval != 0 && listener.is_some() {
            return Err(Exception::illegal_argument(
                "Repeating alarms cannot use AlarmReceivers",
            ));
        }
        if work_source.is_some_and(|w| !w.is_empty())
            && self
                .host
                .check_permission(Some("android.permission.UPDATE_DEVICE_STATS"), uid)?
                != 0
        {
            return Err(Exception::security(
                "AlarmManager.set requires android.permission.UPDATE_DEVICE_STATS",
            ));
        }
        if flags & FLAG_IDLE_UNTIL != 0 {
            if uid != 1000 {
                flags &= !FLAG_IDLE_UNTIL;
            } else {
                window = 0;
            }
        }
        flags &= !(FLAG_WAKE_FROM_IDLE
            | FLAG_ALLOW_WHILE_IDLE_UNRESTRICTED
            | FLAG_ALLOW_WHILE_IDLE_COMPAT);
        let core = uid.rem_euclid(PER_USER_RANGE as i32) < 10_000;
        if clock.is_some() {
            flags |= FLAG_WAKE_FROM_IDLE;
            window = 0;
        } else if core {
            flags |= FLAG_ALLOW_WHILE_IDLE_UNRESTRICTED;
            flags &= !(FLAG_ALLOW_WHILE_IDLE | FLAG_PRIORITIZE);
        }
        let exact = window == 0;
        // Its delivery may let its app start foreground services for a
        // while (`mOptsWithFgs`, `mOptsWithFgsForAlarmClock`): an exact
        // alarm's, or before exact alarms needed a permission, an alarm
        // clock's or one allowed while idle.
        let fgs_allowlist = if flags & FLAG_PRIORITIZE != 0 {
            None
        } else if self.exact_alarm_change_enabled(&package)? {
            exact.then_some(if clock.is_some() {
                super::services::REASON_ALARM_MANAGER_ALARM_CLOCK
            } else {
                super::services::REASON_ALARM_MANAGER_WHILE_IDLE
            })
        } else {
            (flags & FLAG_ALLOW_WHILE_IDLE != 0 || clock.is_some())
                .then_some(super::services::REASON_ALARM_MANAGER_WHILE_IDLE)
        };
        if flags & FLAG_PRIORITIZE != 0 {
            if self
                .host
                .check_permission(Some("android.permission.SCHEDULE_PRIORITIZED_ALARM"), uid)?
                != 0
            {
                return Err(Exception::security(
                    "AlarmManager.setPrioritized requires android.permission.SCHEDULE_PRIORITIZED_ALARM",
                ));
            }
            flags &= !FLAG_ALLOW_WHILE_IDLE;
        // A listener's exact alarm needs no permission.
        } else if exact && listener.is_none() && !self.exact_allowed(&package, uid)? {
            return Err(Exception::security(format!(
                "Caller {package} needs to hold android.permission.SCHEDULE_EXACT_ALARM or android.permission.USE_EXACT_ALARM to set exact alarms."
            )));
        }
        if exact {
            flags |= FLAG_STANDALONE;
        }
        // `setImpl`, which drops (and logs) an alarm of neither or both.
        if operation.is_none() == listener.is_none() {
            eprintln!(
                "app host: alarm of {package}: needs either a PendingIntent or an AlarmReceiver"
            );
            return Ok(());
        }
        if interval > 0 && interval < MIN_INTERVAL {
            interval = MIN_INTERVAL;
        } else if interval > MAX_INTERVAL {
            interval = MAX_INTERVAL;
        }
        if !(RTC_WAKEUP..=ELAPSED_REALTIME).contains(&kind) {
            return Err(Exception::illegal_argument(format!(
                "Invalid alarm type {kind}"
            )));
        }
        let when = when.max(0);
        let now = elapsed_ms();
        let requested = (now + if core { 0 } else { MIN_FUTURITY }).max(to_elapsed(when, kind));
        let listener = match listener {
            Some(Binder::Handle(h)) => {
                let strong = self.host.process.strong(h);
                let shared = Arc::downgrade(&self.shared);
                let death =
                    self.host.process.link_to_death(
                        &strong,
                        Box::new(move || {
                            if let Some(s) = shared.upgrade() {
                                s.state.lock().unwrap().alarms.retain(|a| {
                                    a.listener.as_ref().is_none_or(|(l, _)| l.handle != h)
                                });
                            }
                        }),
                    );
                Some((strong, death))
            }
            _ => None,
        };
        let mut state = self.shared.state.lock().unwrap();
        if state.alarms.iter().filter(|a| a.uid == uid).count() >= MAX_ALARMS_PER_UID && uid != 1000
        {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!(
                    "Maximum limit of concurrent alarms {MAX_ALARMS_PER_UID} reached for uid: {uid}, callingPackage: {package}"
                ),
            ));
        }
        // An alarm of the same operation or listener is replaced.
        remove_matching(
            &self.host,
            &mut state,
            operation,
            listener.as_ref().map(|(l, _)| l.handle),
        );
        let rtc_when = matches!(kind, RTC | RTC_WAKEUP).then_some(when);
        state.alarms.push(Alarm {
            requested,
            rtc_when,
            interval,
            flags,
            operation,
            listener,
            clock,
            uid,
            package,
            fgs_allowlist,
        });
        self.alarm_clocks_changed(&mut state);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// The next alarm clock of `user`.
    fn next_alarm_clock(state: &State, user: i32) -> Option<ClockInfo> {
        state
            .alarms
            .iter()
            .filter(|a| a.user() == user)
            .filter_map(|a| a.clock)
            .min_by_key(|c| c.trigger)
    }

    /// `updateNextAlarmClockLocked`: users whose next alarm clock changed
    /// hear `ACTION_NEXT_ALARM_CLOCK_CHANGED`.
    fn alarm_clocks_changed(&self, state: &mut State) {
        let mut users: Vec<i32> = state
            .alarms
            .iter()
            .map(Alarm::user)
            .chain(state.next_clocks.iter().map(|(u, _)| *u))
            .collect();
        users.sort_unstable();
        users.dedup();
        for user in users {
            let next = Self::next_alarm_clock(state, user);
            let last = state
                .next_clocks
                .iter()
                .find(|(u, _)| *u == user)
                .map(|(_, t)| *t);
            if last.flatten() != next || last.is_none() && next.is_some() {
                state.next_clocks.retain(|(u, _)| *u != user);
                state.next_clocks.push((user, next));
                let intent = RawIntent::of(
                    Some(ACTION_NEXT_ALARM_CLOCK_CHANGED),
                    FLAG_RECEIVER_REPLACE_PENDING | FLAG_RECEIVER_FOREGROUND,
                    &[],
                );
                self.host.broadcasts.send_from_system(&self.host, intent);
            }
        }
    }
}

/// Removes the alarms of `operation` or of the listener `listener`.
fn remove_matching(
    host: &AppHost,
    state: &mut State,
    operation: Option<Binder>,
    listener: Option<u32>,
) {
    state.alarms.retain(|a| {
        let gone = (operation.is_some() && a.operation == operation)
            || (listener.is_some() && a.listener.as_ref().map(|(l, _)| l.handle) == listener);
        if gone && let Some((l, death)) = &a.listener {
            host.process.clear_death(l, *death);
        }
        !gone
    });
}

/// The listener's `IAlarmCompleteListener`: delivery needs no wake lock
/// here (the app host keeps none for it), so its completion only ends it.
struct Complete;

impl Service for Complete {
    fn descriptor(&self) -> &str {
        COMPLETE_DESCRIPTOR
    }

    fn transact(&self, _call: &mut Call<'_>) -> Reply {
        Ok(Parcel::new())
    }
}

/// The alarms' thread: due alarms are delivered (`triggerAlarmsLocked`),
/// repeating ones rescheduled, and the clock's broadcasts sent.
fn run(host: Weak<AppHost>, shared: Arc<Shared>) {
    loop {
        let mut state = shared.state.lock().unwrap();
        let now = elapsed_ms();
        let Some(host) = host.upgrade() else { return };
        follow_mac_zone(&host, &mut state, now);
        let time_changed = clock_changed(&mut state, now);
        let next = state
            .alarms
            .iter()
            .map(|a| a.requested)
            .chain([state.next_tick, state.next_date])
            .min()
            .unwrap_or(now);
        if next > now && !time_changed {
            drop(host);
            let wait = Duration::from_millis((next - now).min(POLL_MS) as u64);
            drop(shared.changed.wait_timeout(state, wait).unwrap());
            continue;
        }
        let mut due = Vec::new();
        let mut kept = Vec::new();
        for mut a in std::mem::take(&mut state.alarms) {
            if a.requested > now {
                kept.push(a);
                continue;
            }
            let mut count = 1;
            if a.interval > 0 {
                // The periods it missed, and the next one.
                count += (now - a.requested) / a.interval;
                let next = a.requested + count * a.interval;
                // Repeating alarms have no listener (`set` refuses one).
                due.push((a.operation, None, count, a.package.clone(), a.fgs_allowlist));
                a.requested = next;
                a.rtc_when = a.rtc_when.map(|w| w + count * a.interval);
                kept.push(a);
            } else {
                let listener = a.listener.map(|(l, death)| {
                    host.process.clear_death(&l, death);
                    l
                });
                due.push((a.operation, listener, count, a.package, a.fgs_allowlist));
            }
        }
        state.alarms = kept;
        let tick = state.next_tick <= now;
        let date = state.next_date <= now;
        if tick || date {
            let (t, d) = next_tick_and_date(now);
            state.next_tick = t;
            state.next_date = d;
        }
        let service = host.alarm_service.get().cloned();
        if let Some(service) = &service {
            service.alarm_clocks_changed(&mut state);
        }
        drop(state);
        for (operation, listener, count, package, fgs_allowlist) in due {
            if let Some(op) = operation {
                // Sent with its options' temporary allowlist, for the
                // PendingIntent's creator (`tempAllowlistForPendingIntent`,
                // whose `pushTempAllowlist` tells DeviceIdleController).
                if let (Some(reason), Some(uid)) = (fgs_allowlist, host.intent_sender_uid(op)) {
                    let duration = super::services::ALLOW_WHILE_IDLE_ALLOWLIST_DURATION;
                    if let Err(e) = host.device_idle().add_temp_direct(
                        uid,
                        duration.as_millis() as i64,
                        super::device_idle::TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED,
                        reason,
                        None,
                    ) {
                        eprintln!(
                            "app host: alarm: temporary allowlist of {uid}: {}",
                            e.message
                        );
                    }
                }
                let fill = RawIntent::of(
                    None,
                    0,
                    &[(EXTRA_ALARM_COUNT, crate::bundle::Value::Int(count as i32))],
                );
                if let Err(e) = host.send_intent_sender(
                    1000,
                    None,
                    Some(op),
                    0,
                    Some(fill),
                    None,
                    None,
                    None,
                    None,
                ) {
                    eprintln!("app host: alarm of {package}: {}", e.message);
                }
            }
            if let Some(l) = listener {
                let mut p = Parcel::new();
                listener::DoAlarm {
                    callback: Some(host.process.add_service(Arc::new(Complete))),
                }
                .write(&mut p);
                if let Err(s) = l.transact(listener::DO_ALARM, &p, true) {
                    eprintln!("app host: alarm of {package}: doAlarm: status {s}");
                }
            }
        }
        if tick {
            let flags = FLAG_RECEIVER_REGISTERED_ONLY
                | FLAG_RECEIVER_FOREGROUND
                | FLAG_RECEIVER_INCLUDE_BACKGROUND;
            host.broadcasts
                .send_from_system(&host, RawIntent::of(Some(ACTION_TIME_TICK), flags, &[]));
        }
        if date {
            let flags = FLAG_RECEIVER_REPLACE_PENDING
                | FLAG_RECEIVER_INCLUDE_BACKGROUND
                | FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
            host.broadcasts
                .send_from_system(&host, RawIntent::of(Some(ACTION_DATE_CHANGED), flags, &[]));
        }
        if time_changed {
            let flags = FLAG_RECEIVER_REPLACE_PENDING
                | FLAG_RECEIVER_FOREGROUND
                | FLAG_RECEIVER_INCLUDE_BACKGROUND
                | FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
            host.broadcasts
                .send_from_system(&host, RawIntent::of(Some(ACTION_TIME_CHANGED), flags, &[]));
        }
    }
}

/// `AlarmThread`'s time change: the wall clock more than a second off
/// where the elapsed clock puts it moves the RTC alarms
/// (`rebatchAllAlarms`), the next minute and midnight; whether it changed.
fn clock_changed(state: &mut State, now: i64) -> bool {
    let wall = wall_ms();
    let (last_wall, last_elapsed) = state.last_clock;
    let expected = last_wall + (now - last_elapsed);
    state.last_clock = (wall, now);
    if (wall - expected).abs() <= 1000 {
        return false;
    }
    eprintln!("app host: the clock changed by {} ms", wall - expected);
    for a in &mut state.alarms {
        if let Some(when) = a.rtc_when {
            a.requested = to_elapsed(when, RTC);
        }
    }
    (state.next_tick, state.next_date) = next_tick_and_date(now);
    true
}

/// The Mac's zone, followed when it changes.
fn follow_mac_zone(host: &AppHost, state: &mut State, now: i64) {
    if let Some(zone) = host.property(MAC_TIME_ZONE)
        && state.mac_zone.as_ref() != Some(&zone)
    {
        state.mac_zone = Some(zone.clone());
        set_time_zone(host, state, now, &zone);
    }
}

/// `setTimeZoneImpl` with `SystemTimeZone.setTimeZoneId`: a zone of the
/// image's tzdata becomes the device's, with high confidence; a new one
/// moves midnight and is broadcast.
fn set_time_zone(host: &AppHost, state: &mut State, now: i64, zone: &str) {
    if zone.is_empty() || !host.valid_time_zone(zone) {
        eprintln!("app host: setTimeZoneId: Invalid time zone ID. timeZoneId={zone}");
        return;
    }
    let changed = host.property(TIME_ZONE).as_deref() != Some(zone);
    if changed {
        host.set_property(TIME_ZONE, zone);
    }
    if host
        .property(TIME_ZONE_CONFIDENCE)
        .and_then(|c| c.parse().ok())
        != Some(TIME_ZONE_CONFIDENCE_HIGH)
    {
        host.set_property(TIME_ZONE_CONFIDENCE, &TIME_ZONE_CONFIDENCE_HIGH.to_string());
    }
    if !changed {
        return;
    }
    // The date changes at the zone's midnight (the Mac's, re-read).
    // SAFETY: tzset re-reads the process's zone.
    unsafe { tzset() };
    state.next_date = next_tick_and_date(now).1;
    let flags = FLAG_RECEIVER_REPLACE_PENDING
        | FLAG_RECEIVER_INCLUDE_BACKGROUND
        | FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT
        | FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
    let extras = [(
        EXTRA_TIMEZONE,
        crate::bundle::Value::String(Some(zone.to_string())),
    )];
    host.broadcasts.send_from_system(
        host,
        RawIntent::of(Some(ACTION_TIMEZONE_CHANGED), flags, &extras),
    );
}

impl Service for AlarmManager {
    fn descriptor(&self) -> &str {
        alarm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        match call.code {
            alarm::SET => {
                let a = alarm::Set::<PendingIntentRef, WorkSource, ClockInfo>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.set(
                        call,
                        a.calling_package,
                        a.r#type,
                        a.trigger_at_time,
                        a.window_length,
                        a.interval,
                        a.flags,
                        a.operation.and_then(|o| o.0),
                        a.listener,
                        a.work_source,
                        a.alarm_clock,
                    )
                    .map(|()| {
                        let mut p = Parcel::new();
                        alarm::write_set_reply(&mut p);
                        p
                    }),
                )
            }
            alarm::REMOVE => {
                let a = alarm::Remove::<PendingIntentRef>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let operation = a.operation.and_then(|o| o.0);
                let listener = match a.listener {
                    Some(Binder::Handle(h)) => Some(h),
                    _ => None,
                };
                if operation.is_some() || listener.is_some() {
                    let mut state = self.shared.state.lock().unwrap();
                    remove_matching(&self.host, &mut state, operation, listener);
                    self.alarm_clocks_changed(&mut state);
                }
                let mut p = Parcel::new();
                alarm::write_remove_reply(&mut p);
                Ok(p)
            }
            alarm::REMOVE_ALL => {
                let a = alarm::RemoveAll::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let package = a.package_name.unwrap_or_default();
                    if !self.host.owns_package(uid, &package)? {
                        return Err(Exception::security(format!(
                            "Package {package} does not belong to the calling uid {uid}"
                        )));
                    }
                    let mut state = self.shared.state.lock().unwrap();
                    let host = &self.host;
                    state.alarms.retain(|a| {
                        if a.uid != uid {
                            return true;
                        }
                        if let Some((l, death)) = &a.listener {
                            host.process.clear_death(l, *death);
                        }
                        false
                    });
                    self.alarm_clocks_changed(&mut state);
                    let mut p = Parcel::new();
                    alarm::write_remove_all_reply(&mut p);
                    Ok(p)
                })())
            }
            alarm::GET_NEXT_ALARM_CLOCK => {
                let a = alarm::GetNextAlarmClock::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.host
                        .handle_incoming_user(
                            uid,
                            a.user_id,
                            false,
                            AllowMode::NonFull,
                            Some("getNextAlarmClock"),
                            None,
                        )
                        .map(|user| {
                            let next =
                                Self::next_alarm_clock(&self.shared.state.lock().unwrap(), user);
                            let mut p = Parcel::new();
                            alarm::write_get_next_alarm_clock_reply(&mut p, next.as_ref());
                            p
                        }),
                )
            }
            alarm::GET_NEXT_WAKE_FROM_IDLE_TIME => {
                alarm::GetNextWakeFromIdleTime::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let state = self.shared.state.lock().unwrap();
                let next = state
                    .alarms
                    .iter()
                    .filter(|a| a.flags & FLAG_WAKE_FROM_IDLE != 0)
                    .map(|a| a.requested)
                    .min()
                    .unwrap_or(i64::MAX);
                let mut p = Parcel::new();
                alarm::write_get_next_wake_from_idle_time_reply(&mut p, next);
                Ok(p)
            }
            alarm::CAN_SCHEDULE_EXACT_ALARMS => {
                let a =
                    alarm::CanScheduleExactAlarms::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let package = a.package_name.unwrap_or_default();
                    if !self.host.owns_package(uid, &package)? {
                        return Err(Exception::security(format!(
                            "Uid {uid} cannot query canScheduleExactAlarms for package {package}"
                        )));
                    }
                    let allowed = self.exact_allowed(&package, uid)?;
                    let mut p = Parcel::new();
                    alarm::write_can_schedule_exact_alarms_reply(&mut p, allowed);
                    Ok(p)
                })())
            }
            alarm::HAS_SCHEDULE_EXACT_ALARM => {
                let a =
                    alarm::HasScheduleExactAlarm::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    const FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
                    if uid / PER_USER_RANGE as i32 != a.user_id
                        && self.host.check_permission(Some(FULL), uid)? != 0
                    {
                        return Err(Exception::security(format!(
                            "hasScheduleExactAlarm: Neither user {uid} nor current process has {FULL}."
                        )));
                    }
                    let package = a.package_name.unwrap_or_default();
                    let (state, _) = self.host.packages()?;
                    // `getPackageUid`: -1 for a package not installed.
                    let target_uid = state
                        .packages
                        .get(&package)
                        .map_or(-1, |ps| a.user_id * PER_USER_RANGE as i32 + ps.app_id);
                    if uid != target_uid && uid.rem_euclid(PER_USER_RANGE as i32) >= 10_000 {
                        return Err(Exception::security(format!(
                            "Uid {uid} cannot query hasScheduleExactAlarm for package {package}"
                        )));
                    }
                    let has =
                        target_uid > 0 && self.has_schedule_exact_alarm(&package, target_uid)?;
                    let mut p = Parcel::new();
                    alarm::write_has_schedule_exact_alarm_reply(&mut p, has);
                    Ok(p)
                })())
            }
            alarm::SET_TIME_ZONE => {
                let a = alarm::SetTimeZone::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if self
                    .host
                    .check_permission(Some("android.permission.SET_TIME_ZONE"), uid)
                    .map_err(|_| BAD_VALUE)?
                    != 0
                {
                    return reply(Err(Exception::security(format!(
                        "setTimeZone: Neither user {uid} nor current process has android.permission.SET_TIME_ZONE."
                    ))));
                }
                set_time_zone(
                    &self.host,
                    &mut self.shared.state.lock().unwrap(),
                    elapsed_ms(),
                    a.zone.as_deref().unwrap_or_default(),
                );
                let mut p = Parcel::new();
                alarm::write_set_time_zone_reply(&mut p);
                Ok(p)
            }
            code => reply(Err(self.host.unimplemented(
                "alarm",
                self.descriptor(),
                code,
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The next minute and the next midnight are ahead, within a minute
    /// and a day.
    #[test]
    fn the_clock_broadcasts_are_ahead() {
        let (tick, date) = next_tick_and_date(1_000);
        assert!((1_001..=61_000).contains(&tick));
        assert!((1_001..=1_000 + INTERVAL_DAY).contains(&date));
    }

    #[test]
    fn alarm_clock_info_round_trips() {
        let info = ClockInfo {
            trigger: 1234,
            show: None,
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let back = ClockInfo::read_from(&mut p.reader()).unwrap();
        assert_eq!((back.trigger, back.show), (1234, None));
    }
}
