//! `alarm`: AlarmManagerService for the app host
//! (`AlarmManagerService` at `android-16.0.0_r1`, ADR 0013): apps' alarms
//! with their checks (the calling package, exact alarms' permissions,
//! listeners not repeating, the per-uid limit), the original's minimum
//! futurity and clamped intervals, delivered at the start of their window
//! (alarms are not batched) on the Mac's clocks (elapsed
//! realtime and wall time, as the guest's `SystemClock` and
//! `System.currentTimeMillis` read them) through their PendingIntent (with
//! `EXTRA_ALARM_COUNT`) or their `IAlarmListener`, repeating alarms
//! rescheduled; alarm clocks and `getNextAlarmClock`, with
//! `ACTION_NEXT_ALARM_CLOCK_CHANGED`; and the clock's own broadcasts,
//! `ACTION_TIME_TICK` each minute and `ACTION_DATE_CHANGED` at local
//! midnight. Batching, standby buckets, quotas and idle (doze) are not run:
//! the app host's apps are all top, the device never idles. A wakeup alarm
//! does not wake the Mac, and the time and time zone are the Mac's
//! (`setTime` and `setTimeZone` are not implemented, nor a change of the
//! Mac's clock followed, #1501).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::android_app_ialarmlistener as listener;
use aim_service_aidl::android_app_ialarmmanager as alarm;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

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
#[derive(Clone, Copy)]
pub struct ClockInfo {
    trigger: i64,
    show: Option<Binder>,
}

impl ReadParcelable for ClockInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let trigger = r.read_i64()?;
        // readParcelable: the class, then the PendingIntent.
        let show = if r.read_string16()?.is_some() { r.read_binder()? } else { None };
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
    /// The elapsed time of the start of its window.
    requested: i64,
    interval: i64,
    flags: i32,
    operation: Option<Binder>,
    listener: Option<(Strong, u64)>,
    clock: Option<ClockInfo>,
    uid: i32,
    package: String,
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
    next_clocks: Vec<(i32, Option<i64>)>,
    /// Elapsed times of the next minute and the next local midnight.
    next_tick: i64,
    next_date: i64,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

pub struct AlarmManager {
    host: Arc<AppHost>,
    shared: Arc<Shared>,
}

/// `SystemClock.elapsedRealtime()` and `System.currentTimeMillis()`.
fn elapsed_ms() -> i64 {
    super::clock_ms(libc::CLOCK_MONOTONIC_RAW)
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `convertToElapsed`: a wall time to the elapsed clock.
fn to_elapsed(when: i64, kind: i32) -> i64 {
    if kind == RTC || kind == RTC_WAKEUP { when - wall_ms() + elapsed_ms() } else { when }
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
    let since_midnight = (tm.tm_hour as i64 * 3600 + tm.tm_min as i64 * 60 + tm.tm_sec as i64) * 1000 + wall.rem_euclid(1000);
    (tick, now_elapsed + INTERVAL_DAY - since_midnight)
}

impl AlarmManager {
    pub fn new(host: Arc<AppHost>) -> AlarmManager {
        let now = elapsed_ms();
        let (next_tick, next_date) = next_tick_and_date(now);
        let shared = Arc::new(Shared {
            state: Mutex::new(State { next_tick, next_date, ..Default::default() }),
            changed: Condvar::new(),
        });
        let weak = Arc::downgrade(&host);
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("alarms".into()).spawn(move || run(weak, s));
        AlarmManager { host, shared }
    }

    /// `isExactAlarmChangeEnabled`, `hasUseExactAlarmInternal`,
    /// `hasScheduleExactAlarmInternal` and the core uids' exemption: may
    /// `package` of `uid` set exact alarms?
    fn exact_allowed(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        let Some(ps) = state.packages.get(package) else {
            return Ok(false);
        };
        let compat = &self.host.inputs()?.compat;
        let target = ps.target_sdk_version;
        if !compat.is_enabled(REQUIRE_EXACT_ALARM_PERMISSION, target) || uid.rem_euclid(PER_USER_RANGE as i32) < 10_000 {
            return Ok(true);
        }
        let requested = |p: &str| ps.pkg.as_ref().is_some_and(|pkg| pkg.requested_permissions.iter().any(|r| r == p));
        if compat.is_enabled(ENABLE_USE_EXACT_ALARM, target) && self.host.check_permission(Some("android.permission.USE_EXACT_ALARM"), uid)? == 0 {
            return Ok(true);
        }
        let mode = super::appops_service::mode(&self.host, super::appops_service::OP_SCHEDULE_EXACT_ALARM, uid, Some(package))?;
        Ok(match mode {
            0 => true,
            super::appops_service::MODE_DEFAULT => {
                requested("android.permission.SCHEDULE_EXACT_ALARM")
                    && !compat.is_enabled(SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT, target)
            }
            _ => false,
        })
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
            return Err(Exception::security(format!("Package {package} does not belong to the calling uid {uid}")));
        }
        if interval != 0 && listener.is_some() {
            return Err(Exception::illegal_argument("Repeating alarms cannot use AlarmReceivers"));
        }
        if work_source.is_some_and(|w| !w.is_empty()) && self.host.check_permission(Some("android.permission.UPDATE_DEVICE_STATS"), uid)? != 0 {
            return Err(Exception::security("AlarmManager.set requires android.permission.UPDATE_DEVICE_STATS"));
        }
        if flags & FLAG_IDLE_UNTIL != 0 {
            if uid != 1000 {
                flags &= !FLAG_IDLE_UNTIL;
            } else {
                window = 0;
            }
        }
        flags &= !(FLAG_WAKE_FROM_IDLE | FLAG_ALLOW_WHILE_IDLE_UNRESTRICTED | FLAG_ALLOW_WHILE_IDLE_COMPAT);
        let core = uid.rem_euclid(PER_USER_RANGE as i32) < 10_000;
        if clock.is_some() {
            flags |= FLAG_WAKE_FROM_IDLE;
            window = 0;
        } else if core {
            flags |= FLAG_ALLOW_WHILE_IDLE_UNRESTRICTED;
            flags &= !(FLAG_ALLOW_WHILE_IDLE | FLAG_PRIORITIZE);
        }
        let exact = window == 0;
        if flags & FLAG_PRIORITIZE != 0 {
            if self.host.check_permission(Some("android.permission.SCHEDULE_PRIORITIZED_ALARM"), uid)? != 0 {
                return Err(Exception::security("AlarmManager.setPrioritized requires android.permission.SCHEDULE_PRIORITIZED_ALARM"));
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
            eprintln!("app host: alarm of {package}: needs either a PendingIntent or an AlarmReceiver");
            return Ok(());
        }
        if interval > 0 && interval < MIN_INTERVAL {
            interval = MIN_INTERVAL;
        } else if interval > MAX_INTERVAL {
            interval = MAX_INTERVAL;
        }
        if !(RTC_WAKEUP..=ELAPSED_REALTIME).contains(&kind) {
            return Err(Exception::illegal_argument(format!("Invalid alarm type {kind}")));
        }
        let when = when.max(0);
        let now = elapsed_ms();
        let requested = (now + if core { 0 } else { MIN_FUTURITY }).max(to_elapsed(when, kind));
        let listener = match listener {
            Some(Binder::Handle(h)) => {
                let strong = self.host.process.strong(h);
                let shared = Arc::downgrade(&self.shared);
                let death = self.host.process.link_to_death(
                    &strong,
                    Box::new(move || {
                        if let Some(s) = shared.upgrade() {
                            s.state.lock().unwrap().alarms.retain(|a| a.listener.as_ref().is_none_or(|(l, _)| l.handle != h));
                        }
                    }),
                );
                Some((strong, death))
            }
            _ => None,
        };
        let mut state = self.shared.state.lock().unwrap();
        if state.alarms.iter().filter(|a| a.uid == uid).count() >= MAX_ALARMS_PER_UID && uid != 1000 {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("Maximum limit of concurrent alarms {MAX_ALARMS_PER_UID} reached for uid: {uid}, callingPackage: {package}"),
            ));
        }
        // An alarm of the same operation or listener is replaced.
        remove_matching(&self.host, &mut state, operation, listener.as_ref().map(|(l, _)| l.handle));
        state.alarms.push(Alarm { requested, interval, flags, operation, listener, clock, uid, package });
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
        let mut users: Vec<i32> = state.alarms.iter().map(Alarm::user).chain(state.next_clocks.iter().map(|(u, _)| *u)).collect();
        users.sort_unstable();
        users.dedup();
        for user in users {
            let next = Self::next_alarm_clock(state, user).map(|c| c.trigger);
            let last = state.next_clocks.iter().find(|(u, _)| *u == user).map(|(_, t)| *t);
            if last.flatten() != next || last.is_none() && next.is_some() {
                state.next_clocks.retain(|(u, _)| *u != user);
                state.next_clocks.push((user, next));
                let intent = RawIntent::of(Some(ACTION_NEXT_ALARM_CLOCK_CHANGED), FLAG_RECEIVER_REPLACE_PENDING | FLAG_RECEIVER_FOREGROUND, &[]);
                self.host.broadcasts.send_from_system(&self.host, intent);
            }
        }
    }
}

/// Removes the alarms of `operation` or of the listener `listener`.
fn remove_matching(host: &AppHost, state: &mut State, operation: Option<Binder>, listener: Option<u32>) {
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
        let next = state.alarms.iter().map(|a| a.requested).chain([state.next_tick, state.next_date]).min().unwrap_or(now);
        if next > now {
            let wait = Duration::from_millis((next - now) as u64);
            drop(shared.changed.wait_timeout(state, wait).unwrap());
            continue;
        }
        let Some(host) = host.upgrade() else { return };
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
                due.push((a.operation, None, count, a.package.clone()));
                a.requested = next;
                kept.push(a);
            } else {
                let listener = a.listener.map(|(l, death)| {
                    host.process.clear_death(&l, death);
                    l
                });
                due.push((a.operation, listener, count, a.package));
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
        for (operation, listener, count, package) in due {
            if let Some(op) = operation {
                let fill = RawIntent::of(None, 0, &[(EXTRA_ALARM_COUNT, crate::bundle::Value::Int(count as i32))]);
                if let Err(e) = host.send_intent_sender(1000, None, Some(op), 0, Some(fill), None, None, None, None) {
                    eprintln!("app host: alarm of {package}: {}", e.message);
                }
            }
            if let Some(l) = listener {
                let mut p = Parcel::new();
                listener::DoAlarm { callback: Some(host.process.add_service(Arc::new(Complete))) }.write(&mut p);
                if let Err(s) = l.transact(listener::DO_ALARM, &p, true) {
                    eprintln!("app host: alarm of {package}: doAlarm: status {s}");
                }
            }
        }
        if tick {
            let flags = FLAG_RECEIVER_REGISTERED_ONLY | FLAG_RECEIVER_FOREGROUND | FLAG_RECEIVER_INCLUDE_BACKGROUND;
            host.broadcasts.send_from_system(&host, RawIntent::of(Some(ACTION_TIME_TICK), flags, &[]));
        }
        if date {
            let flags = FLAG_RECEIVER_REPLACE_PENDING | FLAG_RECEIVER_INCLUDE_BACKGROUND | FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
            host.broadcasts.send_from_system(&host, RawIntent::of(Some(ACTION_DATE_CHANGED), flags, &[]));
        }
    }
}

impl Service for AlarmManager {
    fn descriptor(&self) -> &str {
        alarm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        match call.code {
            alarm::SET => {
                let a = alarm::Set::<PendingIntentRef, WorkSource, ClockInfo>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
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
                let a = alarm::Remove::<PendingIntentRef>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
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
                        return Err(Exception::security(format!("Package {package} does not belong to the calling uid {uid}")));
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
                reply(self.host.incoming_user(uid, a.user_id, true).map(|user| {
                    let next = Self::next_alarm_clock(&self.shared.state.lock().unwrap(), user);
                    let mut p = Parcel::new();
                    alarm::write_get_next_alarm_clock_reply(&mut p, next.as_ref());
                    p
                }))
            }
            alarm::GET_NEXT_WAKE_FROM_IDLE_TIME => {
                alarm::GetNextWakeFromIdleTime::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let state = self.shared.state.lock().unwrap();
                let next = state.alarms.iter().filter(|a| a.flags & FLAG_WAKE_FROM_IDLE != 0).map(|a| a.requested).min().unwrap_or(i64::MAX);
                let mut p = Parcel::new();
                alarm::write_get_next_wake_from_idle_time_reply(&mut p, next);
                Ok(p)
            }
            alarm::CAN_SCHEDULE_EXACT_ALARMS => {
                let a = alarm::CanScheduleExactAlarms::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let package = a.package_name.unwrap_or_default();
                    if !self.host.owns_package(uid, &package)? {
                        return Err(Exception::security(format!("Uid {uid} cannot query canScheduleExactAlarms for package {package}")));
                    }
                    let allowed = self.exact_allowed(&package, uid)?;
                    let mut p = Parcel::new();
                    alarm::write_can_schedule_exact_alarms_reply(&mut p, allowed);
                    Ok(p)
                })())
            }
            alarm::HAS_SCHEDULE_EXACT_ALARM => {
                let a = alarm::HasScheduleExactAlarm::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let user = self.host.incoming_user(uid, a.user_id, false)?;
                    let package = a.package_name.unwrap_or_default();
                    let (state, _) = self.host.packages()?;
                    let Some(ps) = state.packages.get(&package) else {
                        return Err(Exception::illegal_argument(format!("Unknown package {package}")));
                    };
                    let target_uid = user * PER_USER_RANGE as i32 + ps.app_id;
                    let mode = super::appops_service::mode(&self.host, super::appops_service::OP_SCHEDULE_EXACT_ALARM, target_uid, Some(&package))?;
                    let requested = ps.pkg.as_ref().is_some_and(|pkg| pkg.requested_permissions.iter().any(|r| r == "android.permission.SCHEDULE_EXACT_ALARM"));
                    let compat = &self.host.inputs()?.compat;
                    let has = match mode {
                        0 => true,
                        super::appops_service::MODE_DEFAULT => requested && !compat.is_enabled(SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT, ps.target_sdk_version),
                        _ => false,
                    };
                    let mut p = Parcel::new();
                    alarm::write_has_schedule_exact_alarm_reply(&mut p, has);
                    Ok(p)
                })())
            }
            code => reply(Err(self.host.unimplemented("alarm", code))),
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
        let info = ClockInfo { trigger: 1234, show: None };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let back = ClockInfo::read_from(&mut p.reader()).unwrap();
        assert_eq!((back.trigger, back.show), (1234, None));
    }
}
