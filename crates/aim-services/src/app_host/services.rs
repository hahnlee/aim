//! Apps' services in the app host, as `ActiveServices` runs them at
//! `android-16.0.0_r1`: `startService` and `stopService`, `bindService`
//! and `unbindService`, a service created in its app's process (started
//! for it when it does not run) with `scheduleCreateService`, its starts
//! delivered with `scheduleServiceArgs` (each holding the URI grants of
//! its intent while delivered) and its bindings requested with
//! `scheduleBindService`, the binder it publishes passed to its
//! connections, and the service destroyed (`scheduleStopService`) once it
//! is neither started nor bound to with `BIND_AUTO_CREATE`. When its
//! process dies, its connections are told and a service still needed is
//! started again. The app host binds services itself too, as the system
//! does (`bindServiceAsUser` from system_server, a connection of its
//! own). A service runs in its process (`android:process`), started for
//! it when it does not run. An isolated service runs in an isolated process
//! of its own (the WebView provider's started by the WebView zygote); an
//! external one (`BIND_EXTERNAL_SERVICE`) runs under its client's package
//! and uid. Shared isolated processes and the app zygote are #1492.
//! `getServices` lists them as `getRunningServiceInfoLocked` does.
//!
//! A service becomes foreground (`setServiceForegroundLocked`) with the
//! original's checks: FOREGROUND_SERVICE, its type a subset of its
//! manifest's and allowed by `ForegroundServiceTypePolicy`
//! ([`fgs_types`](super::fgs_types)), START_FOREGROUND's op, and whether
//! its app may start one from the background (`setFgsRestrictionLocked`'s
//! reasons). Each of the app host's processes is top, so an app with a
//! running process may, as may one an alarm or a service launch
//! temporarily allowed (`mFgsStartTempAllowList`); the process state stays
//! top, above PROCESS_STATE_FOREGROUND_SERVICE. Its notification is
//! posted as its app's, deferred as `shouldShowFgsNotificationLocked`
//! defers it, and cancelled or unflagged when it stops being foreground.
//! A service started with `startForegroundService` that does not call
//! `startForeground` in time is stopped and its app crashed. Short and
//! time-limited services' timeouts, and the ANRs, are not run (#1551).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{
    Binder, EX_BAD_PARCELABLE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::activity_start::Component;
use super::appops_service::{self as appops, MODE_ALLOWED, MODE_DEFAULT};
use super::broadcasts::filter_equals;
use super::fgs_types::{self, Check};
use super::kept::{By, KeptIntent, RawIntent};
use super::notification_service::{Enqueue, Incoming};
use super::uri_grants::{NeededUriGrants, Owner};
use super::{AppHost, PER_USER_RANGE, PROCESS_STATE_TOP, STOCK_PM_FLAGS};
use crate::bundle::Value;
use crate::package::info::{ServiceInfo, java_hash};
use crate::package::list_slice::Slice;

/// `Context.BIND_AUTO_CREATE`; `BIND_EXTERNAL_SERVICE` (both forms).
const BIND_AUTO_CREATE: i64 = 0x1;
const BIND_EXTERNAL_SERVICE: i64 = 0x8000_0000;
const BIND_EXTERNAL_SERVICE_LONG: i64 = 1 << 62;
/// `ServiceInfo.FLAG_ISOLATED_PROCESS`, `FLAG_EXTERNAL_SERVICE`.
const FLAG_ISOLATED_PROCESS: i32 = 0x2;
const FLAG_EXTERNAL_SERVICE: i32 = 0x4;
/// `Context.BIND_SHARED_ISOLATED_PROCESS`, `BIND_PACKAGE_ISOLATED_PROCESS`.
const BIND_SHARED_ISOLATED_PROCESS: i64 = 0x2000;
const BIND_PACKAGE_ISOLATED_PROCESS: i64 = 1 << 14;
/// `Process.SYSTEM_UID`.
const SYSTEM_UID: i32 = 1000;
/// `ActivityThread.SERVICE_DONE_EXECUTING_START`.
const SERVICE_DONE_EXECUTING_START: i32 = 1;
/// `Service.START_*` results.
const START_STICKY_COMPATIBILITY: i32 = 0;
const START_STICKY: i32 = 1;
const START_NOT_STICKY: i32 = 2;
const START_REDELIVER_INTENT: i32 = 3;
const START_TASK_REMOVED_COMPLETE: i32 = 1000;
/// `Service.START_FLAG_REDELIVERY`, `START_FLAG_RETRY`.
const START_FLAG_REDELIVERY: i32 = 1;
const START_FLAG_RETRY: i32 = 2;
/// `IServiceConnection.connected`.
const CONNECTION_DESCRIPTOR: &str = "android.app.IServiceConnection";
const CONNECTED: u32 = 1;
/// `ActivityManagerConstants`: the first restart's delay, how long a
/// service must have run for its restarts to start over, the factor of
/// each further delay, the least time between two services' restarts, how
/// often a service may crash before it is not restarted, and how soon a
/// second crash counts as one more (`MIN_CRASH_INTERVAL`).
const SERVICE_RESTART_DURATION: Duration = Duration::from_secs(1);
const SERVICE_RESET_RUN_DURATION: Duration = Duration::from_secs(60);
const SERVICE_RESTART_DURATION_FACTOR: u32 = 4;
const SERVICE_MIN_RESTART_TIME_BETWEEN: Duration = Duration::from_secs(10);
const BOUND_SERVICE_CRASH_RESTART_DURATION: Duration = Duration::from_secs(30 * 60);
const BOUND_SERVICE_MAX_CRASH_RETRY: i32 = 16;
pub(super) const MIN_CRASH_INTERVAL: Duration = Duration::from_secs(60);
/// `ServiceRecord.MAX_DELIVERY_COUNT`, `MAX_DONE_EXECUTING_COUNT`.
const MAX_DELIVERY_COUNT: i32 = 3;
const MAX_DONE_EXECUTING_COUNT: i32 = 6;
/// `Service.STOP_FOREGROUND_REMOVE`, `STOP_FOREGROUND_DETACH`.
const STOP_FOREGROUND_REMOVE: i32 = 1;
const STOP_FOREGROUND_DETACH: i32 = 2;
/// `ActivityManagerConstants`: how long a service started with
/// `startForegroundService` has to call `startForeground`, how long after
/// its creation a first `startForeground` asks again whether its app may,
/// how long a foreground service's notification is deferred, and how long
/// its app's are then not (`mFgsNotificationDeferralExclusionTime`).
const SERVICE_START_FOREGROUND_TIMEOUT: Duration = Duration::from_secs(30);
const FGS_START_FOREGROUND_TIMEOUT_MS: i64 = 10_000;
const FGS_NOTIFICATION_DEFERRAL_INTERVAL: Duration = Duration::from_secs(10);
const FGS_NOTIFICATION_DEFERRAL_EXCLUSION_TIME: Duration = Duration::from_secs(120);
/// `ActiveServices.FGS_IMMEDIATE_DISPLAY_MASK`.
const FGS_IMMEDIATE_DISPLAY_MASK: i32 = fgs_types::TYPE_MEDIA_PLAYBACK
    | fgs_types::TYPE_PHONE_CALL
    | fgs_types::TYPE_CONNECTED_DEVICE
    | fgs_types::TYPE_MEDIA_PROJECTION;
/// `AlarmManagerService`'s `ALLOW_WHILE_IDLE_WHITELIST_DURATION`.
pub(super) const ALLOW_WHILE_IDLE_ALLOWLIST_DURATION: Duration = Duration::from_secs(10);
/// The compat changes of foreground services: their restriction from the
/// background, its exception, the time-limited types, and overlays no
/// longer exempting.
const FGS_BG_START_RESTRICTION_CHANGE_ID: i64 = 170_668_199;
const FGS_START_EXCEPTION_CHANGE_ID: i64 = 174_041_399;
const FGS_INTRODUCE_TIME_LIMITS: i64 = 317_799_821;
const FGS_SAW_RESTRICTIONS: i64 = 319_471_980;
/// The aconfig flags of the time-limited types' callback and of the
/// overlay restriction.
const INTRODUCE_NEW_SERVICE_ONTIMEOUT_CALLBACK: &str = "android.app.introduce_new_service_ontimeout_callback";
const FGS_DISABLE_SAW: &str = "com.android.server.am.fgs_disable_saw";
/// `PowerExemptionManager.REASON_*`.
const REASON_DENIED: i32 = -1;
const REASON_PROC_STATE_TOP: i32 = 12;
const REASON_SYSTEM_UID: i32 = 51;
const REASON_BACKGROUND_ACTIVITY_PERMISSION: i32 = 58;
const REASON_BACKGROUND_FGS_PERMISSION: i32 = 59;
const REASON_SYSTEM_ALERT_WINDOW_PERMISSION: i32 = 62;
const REASON_DEVICE_DEMO_MODE: i32 = 63;
const REASON_OP_ACTIVATE_VPN: i32 = 68;
const REASON_OP_ACTIVATE_PLATFORM_VPN: i32 = 69;
const REASON_CURRENT_INPUT_METHOD: i32 = 71;
pub(super) const REASON_ALARM_MANAGER_ALARM_CLOCK: i32 = 301;
pub(super) const REASON_ALARM_MANAGER_WHILE_IDLE: i32 = 302;
const REASON_SERVICE_LAUNCH: i32 = 303;
const REASON_SYSTEM_EXEMPT_APP_OP: i32 = 327;
/// `RemoteServiceException`s' type ids.
const FOREGROUND_SERVICE_DID_NOT_START_IN_TIME: i32 = 1;
const CANNOT_POST_FOREGROUND_SERVICE_NOTIFICATION: i32 = 2;
/// `Build.VERSION_CODES` the foreground service checks compare with.
const LOLLIPOP: i32 = 21;
const O: i32 = 26;
const O_MR1: i32 = 27;
const P: i32 = 28;
/// `ApplicationInfo.PRIVATE_FLAG_INSTANT`.
const PRIVATE_FLAG_INSTANT: i32 = 1 << 7;

/// A `ComponentName` read.
pub struct Name {
    pub package: String,
    pub class: String,
}

impl ReadParcelable for Name {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Name {
            package: r.read_string16()?.unwrap_or_default(),
            class: r.read_string16()?.unwrap_or_default(),
        })
    }
}

/// `ServiceRecord.StartItem`.
struct StartItem {
    id: i32,
    intent: Option<KeptIntent>,
    /// The URI grants its intent carries, and their owner once delivered
    /// (`neededGrants`, `uriPermissions`).
    grants: Option<(NeededUriGrants, Owner)>,
    delivery_count: i32,
    done_executing_count: i32,
    delivered_time: Option<Instant>,
}

/// `IntentBindRecord`: the bindings of one intent (by `filterEquals`).
struct Binding {
    intent: KeptIntent,
    binder: Option<Strong>,
    requested: bool,
    received: bool,
    has_bound: bool,
    do_rebind: bool,
}

/// `ServiceRecord`.
struct Record {
    /// The token its process knows it by.
    token: Binder,
    package: String,
    class: String,
    /// The instance it was bound as (`bindServiceInstance`): an isolated
    /// service has a record, so a process, per instance.
    instance: Option<String>,
    info: ServiceInfo,
    /// Its process's start (an isolated service's own process), and
    /// whether it was created there.
    app: Option<i64>,
    created: bool,
    start_requested: bool,
    stop_if_killed: bool,
    call_start: bool,
    last_start_id: i32,
    pending: Vec<StartItem>,
    delivered: Vec<StartItem>,
    bindings: Vec<Binding>,
    /// Its restarts' count and delay, when it last started in a process,
    /// when it is to restart, and its crashes.
    restart_count: i32,
    restart_delay: Duration,
    restart_time: Option<Instant>,
    next_restart_time: Option<Instant>,
    crash_count: i32,
    /// When it was made, `elapsedRealtime` and `uptimeMillis`.
    create_real_time: i64,
    last_activity: i64,
    fg: Foreground,
}

/// `ActiveServices.ServiceNotificationPolicy`: whether a notification is
/// a foreground service's, and if so whether it shows now or only as an
/// update of one that shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FgsPolicy {
    NotForegroundService,
    ShowImmediately,
    UpdateOnly,
}

/// What becomes of a foreground service's notification when it stops
/// being foreground (`setServiceForegroundInnerLocked` with no id).
#[derive(Debug, PartialEq)]
enum Stopped {
    /// `STOP_FOREGROUND_REMOVE`: cancelled.
    Cancel,
    /// It stays, no longer a foreground service's
    /// (`dropFgsNotificationStateLocked`), posted first if it was
    /// deferred.
    Unflag { post: bool },
    /// An app targeting an SDK before L keeps it as it is.
    Keep,
}

/// The notification of a foreground service: its app's package and uid,
/// and its id.
#[derive(Clone)]
struct FgsNotification {
    package: String,
    uid: i32,
    id: i32,
}

/// A service's foreground state (`ServiceRecord`'s).
struct Foreground {
    /// `fgRequired`: started with `startForegroundService`, and `fgWaiting`,
    /// the generation of its timeout while it runs.
    required: bool,
    waiting: Option<u64>,
    /// `isForeground`, `foregroundId`, `foregroundNoti`,
    /// `foregroundServiceType`, `mStartForegroundCount`.
    is_foreground: bool,
    id: i32,
    notification: Option<Incoming>,
    service_type: i32,
    start_count: i32,
    /// Why it may be foreground (`mAllowStart_*`) and have while-in-use
    /// permissions (`mAllowWiu_*`), of its starts and of its bindings.
    allow_start: (i32, i32),
    allow_wiu: (i32, i32),
    /// Whether a device or profile owner's exemption, not answered, may
    /// have allowed what was denied.
    owner_unknown: bool,
    /// Its notification's deferral: `mFgsNotificationDeferred`,
    /// `fgDisplayTime`, `mFgsNotificationShown`, and whether it is in
    /// `mPendingFgsNotifications`.
    deferred: bool,
    display_time: Option<Instant>,
    shown: bool,
    pending: bool,
}

impl Default for Foreground {
    fn default() -> Self {
        Foreground {
            required: false,
            waiting: None,
            is_foreground: false,
            id: 0,
            notification: None,
            service_type: fgs_types::TYPE_NONE,
            start_count: 0,
            allow_start: (REASON_DENIED, REASON_DENIED),
            allow_wiu: (REASON_DENIED, REASON_DENIED),
            owner_unknown: false,
            deferred: false,
            display_time: None,
            shown: false,
            pending: false,
        }
    }
}

/// `ServiceRecord.reasonOr`.
fn reason_or(a: i32, b: i32) -> i32 {
    if a != REASON_DENIED { a } else { b }
}

impl Foreground {
    /// `isFgsAllowedStart`, `isFgsAllowedWiu_forStart`.
    fn allowed_start(&self) -> bool {
        reason_or(self.allow_start.0, self.allow_start.1) != REASON_DENIED
    }

    fn allowed_wiu(&self) -> bool {
        reason_or(self.allow_wiu.0, self.allow_wiu.1) != REASON_DENIED
    }

    /// `resetFgsRestrictionLocked`.
    fn reset_restriction(&mut self) {
        self.allow_start = (REASON_DENIED, REASON_DENIED);
        self.allow_wiu = (REASON_DENIED, REASON_DENIED);
        self.owner_unknown = false;
    }

    /// Stops being foreground with `flags` (`STOP_FOREGROUND_*`), in an app
    /// targeting `target_sdk`: what becomes of its notification, its id and
    /// notification forgotten when removed or detached.
    fn stop(&mut self, flags: i32, target_sdk: i32) -> Stopped {
        let stopped = if flags & STOP_FOREGROUND_REMOVE != 0 {
            Stopped::Cancel
        } else if target_sdk >= LOLLIPOP {
            Stopped::Unflag { post: !self.shown }
        } else {
            Stopped::Keep
        };
        if flags & STOP_FOREGROUND_REMOVE != 0 || (target_sdk >= LOLLIPOP && flags & STOP_FOREGROUND_DETACH != 0) {
            self.id = 0;
            self.notification = None;
        }
        self.is_foreground = false;
        self.service_type = fgs_types::TYPE_NONE;
        self.reset_restriction();
        stopped
    }

    /// `shouldShowFgsNotificationLocked` at `now`, the app's notifications
    /// not deferred until `eligible` (`withinFgsDeferRateLimit`).
    fn show_now(&self, now: Instant, eligible: Option<Instant>, force_deferred: bool, immediately: bool) -> bool {
        if self.deferred && self.display_time.is_some_and(|t| now >= t) {
            return true;
        }
        let within_deferral = self.display_time.is_some_and(|t| now < t);
        if !within_deferral && eligible.is_some_and(|e| now < e) {
            return true;
        }
        if self.shown {
            return true;
        }
        !force_deferred && (immediately || self.service_type & FGS_IMMEDIATE_DISPLAY_MASK != 0)
    }
}

/// `startForeground`'s type (`foregroundServiceType`) as it takes effect:
/// the manifest's for `FOREGROUND_SERVICE_TYPE_MANIFEST`, which it must be
/// a subset of, and short-service dropped when combined with another.
fn effective_type(requested: i32, manifest: i32) -> Result<i32, Exception> {
    let mut ty = if requested == fgs_types::TYPE_MANIFEST { manifest } else { requested };
    if ty & manifest != ty {
        return Err(Exception::illegal_argument(format!(
            "foregroundServiceType 0x{ty:08X} is not a subset of foregroundServiceType attribute 0x{manifest:08X} in service element of manifest file"
        )));
    }
    if ty & fgs_types::TYPE_SHORT_SERVICE != 0 && ty != fgs_types::TYPE_SHORT_SERVICE {
        eprintln!("app host: startForeground(): FOREGROUND_SERVICE_TYPE_SHORT_SERVICE is combined with other types. SHORT_SERVICE will be ignored.");
        ty &= !fgs_types::TYPE_SHORT_SERVICE;
    }
    Ok(ty)
}

/// A parcelable exception of `class` (its `writeToParcel` writes its
/// message).
fn parcelable_exception(class: &str, message: String) -> Exception {
    let mut p = Parcel::new();
    p.write_string16(Some(class));
    p.write_string16(Some(&message));
    Exception::parcelable(Some(&message), &p).expect("a message has no objects")
}

/// `ComponentName.flattenToShortString`.
fn short_name(package: &str, class: &str) -> String {
    match class.strip_prefix(package).filter(|rest| rest.starts_with('.')) {
        Some(rest) => format!("{package}/{rest}"),
        None => format!("{package}/{class}"),
    }
}

/// `ProcessRecord.toString`, without its identity hash.
fn process_record(pid: i32, process: &str, uid: i32) -> String {
    let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
    let who = if uid < 10_000 {
        uid.to_string()
    } else if app_id >= 10_000 {
        format!("u{}a{}", uid / PER_USER_RANGE as i32, app_id - 10_000)
    } else {
        format!("u{}s{app_id}", uid / PER_USER_RANGE as i32)
    };
    format!("ProcessRecord{{{pid}:{process}/{who}}}")
}

/// The app host's end of a binding it makes (the system's
/// `ServiceConnection`).
pub(super) trait HostConnection: Send + Sync {
    /// `IServiceConnection.connected`: the service's binder, or none and
    /// whether the binding died. It is called with the services locked,
    /// so it hands the call on (as `ServiceDispatcher` posts it to a
    /// handler) rather than act on it.
    fn connected(&self, service: Option<Binder>, dead: bool);
}

/// A connection's `IServiceConnection`: an app's, or the app host's own.
enum Conn {
    App(Strong),
    Host(Arc<dyn HostConnection>),
}

impl Conn {
    /// Whether it is the same `IServiceConnection`.
    fn same(&self, other: &Conn) -> bool {
        match (self, other) {
            (Conn::App(a), Conn::App(b)) => a.handle == b.handle,
            (Conn::Host(a), Conn::Host(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// `ConnectionRecord`.
struct Connection {
    conn: Conn,
    /// Its service's token, and the intent of its binding.
    service: Binder,
    intent: crate::package::intent::Intent,
    /// The client's process (none for the app host's own), and the
    /// activity it bound from.
    client: Option<i64>,
    activity: Option<u64>,
    flags: i64,
    /// What a system client said the binding is for (`clientLabel`).
    client_label: i32,
}

/// A job of the activity manager's handler.
type Job = Box<dyn FnOnce(&Arc<AppHost>) + Send>;

#[derive(Default)]
pub struct Services {
    state: Mutex<State>,
    /// The handler (`mAm.mHandler`) the services' notifications are posted
    /// and cancelled on, in order and outside their lock.
    handler: Mutex<Option<mpsc::Sender<Job>>>,
}

#[derive(Default)]
struct State {
    records: Vec<Record>,
    connections: Vec<Connection>,
    next_bind_seq: i64,
    /// `mFgsStartTempAllowList`: until when a uid may start foreground
    /// services, and why.
    fgs_temp_allowlist: HashMap<i32, (Instant, i32)>,
    /// `mFgsDeferralEligible`: until when a uid's foreground services'
    /// notifications show at once.
    deferral_eligible: HashMap<i32, Instant>,
    /// The generation of the last `startForegroundService` timeout.
    next_timeout: u64,
}

/// One start's arguments (`ServiceStartArgs`).
struct StartArgs {
    id: i32,
    flags: i32,
    intent: Option<RawIntent>,
}

impl WriteParcelable for StartArgs {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0); // not for a removed task
        p.write_i32(self.id);
        p.write_i32(self.flags);
        aim_service_aidl::write_typed(p, self.intent.as_ref().map(By).as_ref());
    }
}

/// `ActivityManager.RunningServiceInfo`. None of the app host's services
/// runs in the system's process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunningServiceInfo {
    package: String,
    class: String,
    pid: i32,
    uid: i32,
    process: String,
    foreground: bool,
    active_since: i64,
    started: bool,
    client_count: i32,
    crash_count: i32,
    last_activity_time: i64,
    restarting: i64,
    flags: i32,
    client_package: Option<String>,
    client_label: i32,
}

/// `RunningServiceInfo.FLAG_STARTED`, `FLAG_FOREGROUND`,
/// `FLAG_PERSISTENT_PROCESS`.
const FLAG_STARTED: i32 = 1 << 0;
const FLAG_FOREGROUND: i32 = 1 << 1;
const FLAG_PERSISTENT_PROCESS: i32 = 1 << 3;

impl WriteParcelable for RunningServiceInfo {
    fn write_to(&self, p: &mut Parcel) {
        Component(&self.package, &self.class).write_to(p);
        p.write_i32(self.pid);
        p.write_i32(self.uid);
        p.write_string16(Some(&self.process));
        p.write_i32(self.foreground as i32);
        p.write_i64(self.active_since);
        p.write_i32(self.started as i32);
        p.write_i32(self.client_count);
        p.write_i32(self.crash_count);
        p.write_i64(self.last_activity_time);
        p.write_i64(self.restarting);
        p.write_i32(self.flags);
        p.write_string16(self.client_package.as_deref());
        p.write_i32(self.client_label);
    }
}

/// A service as `getRunningServiceInfoLocked` sees it: its info
/// (`makeRunningServiceInfoLocked`), its user, the uid of the process it
/// runs in, and when it restarts if it is restarting (uptime
/// milliseconds).
struct Running {
    info: RunningServiceInfo,
    user: i32,
    app_uid: Option<i32>,
    restarting: Option<i64>,
}

/// `getRunningServiceInfoLocked` for `uid`: at most `max` of `services`,
/// in their map's order (`ServiceMap`'s `ArrayMap`, by their names'
/// hashes), with INTERACT_ACROSS_USERS_FULL (`across_users`) those of each
/// running user (user 0 alone runs), else those of the caller's user in a
/// process of its uid, or all of its user's when `allowed`; then those of
/// `services` restarting, in their order and picked alike, again with when
/// they restart.
fn pick(services: &[Running], max: i32, uid: i32, allowed: bool, across_users: bool) -> Vec<RunningServiceInfo> {
    let user = uid / super::PER_USER_RANGE as i32;
    let visible = |s: &Running| allowed || s.app_uid == Some(uid);
    let hash = |s: &&Running| java_hash(&s.info.package).wrapping_add(java_hash(&s.info.class));
    let mut map: Vec<&Running> = services.iter().collect();
    map.sort_by_key(hash);
    let listed = map
        .into_iter()
        .filter(|s| if across_users { s.user == 0 } else { s.user == user && visible(s) })
        .map(|s| s.info.clone());
    let restarting = services.iter().filter_map(|s| {
        let at = s.restarting.filter(|_| across_users || s.user == user && visible(s))?;
        Some(RunningServiceInfo { restarting: at, ..s.info.clone() })
    });
    listed.chain(restarting).take(max.max(0) as usize).collect()
}

/// The restart count and delay after a service with `count` restarts,
/// the last `delay`, `crashes` and its last start at `started` dies:
/// `min` at first and after it ran `reset`, else four times the last,
/// half an hour per crash beyond the first.
fn back_off(
    count: i32,
    delay: Duration,
    crashes: i32,
    started: Option<Instant>,
    min: Duration,
    reset: Duration,
    now: Instant,
) -> (i32, Duration) {
    if delay.is_zero() {
        (count + 1, min)
    } else if crashes > 1 {
        (count, BOUND_SERVICE_CRASH_RESTART_DURATION * (crashes - 1) as u32)
    } else if started.is_none_or(|t| now > t + reset) {
        (1, min)
    } else {
        (count, delay.checked_mul(SERVICE_RESTART_DURATION_FACTOR).unwrap_or(Duration::MAX).max(min))
    }
}

impl Record {
    /// `scheduleServiceRestartLocked`'s back-off: its delivered starts
    /// pending again (but those delivered or done too often), and the
    /// delay before it starts again, none when nothing needs it (`auto`:
    /// a connection creates it).
    fn schedule_restart(&mut self, auto: bool, now: Instant) -> Option<Duration> {
        let mut min = SERVICE_RESTART_DURATION;
        let mut reset = SERVICE_RESET_RUN_DURATION;
        let mut canceled = false;
        let mut again = Vec::new();
        for s in std::mem::take(&mut self.delivered) {
            // A start without an intent is made again if needed.
            if s.intent.is_none() {
                continue;
            }
            if s.delivery_count < MAX_DELIVERY_COUNT && s.done_executing_count < MAX_DONE_EXECUTING_COUNT {
                let dur = s.delivered_time.map_or(Duration::ZERO, |t| now - t) * 2;
                min = min.max(dur);
                reset = reset.max(dur);
                again.push(s);
            } else {
                eprintln!("app host: canceling start item {} in service {}/{}", s.id, self.package, self.class);
                canceled = true;
            }
        }
        self.pending.splice(0..0, again);
        // `canStopIfKilled`.
        let stop = self.start_requested && (self.stop_if_killed || canceled) && self.pending.is_empty();
        if stop && !auto {
            return None;
        }
        (self.restart_count, self.restart_delay) =
            back_off(self.restart_count, self.restart_delay, self.crash_count, self.restart_time, min, reset, now);
        if self.start_requested && self.stop_if_killed && self.pending.is_empty() {
            self.start_requested = false;
        }
        Some(self.restart_delay)
    }

    /// The notification it shows as a foreground service, if any.
    fn fgs_notification(&self) -> Option<FgsNotification> {
        self.fg.notification.as_ref().map(|_| FgsNotification {
            package: self.package.clone(),
            uid: self.info.info.application_info.uid,
            id: self.fg.id,
        })
    }

    /// `ServiceRecord.makeNextStartId`.
    fn next_start_id(&mut self) -> i32 {
        self.last_start_id = next_id(self.last_start_id);
        self.last_start_id
    }

    /// `findDeliveredStart`: the start delivered with `id`, removed when
    /// asked (with those before it, `remove` for a done start).
    fn take_delivered(&mut self, id: i32, remove: bool) -> Option<usize> {
        let at = self.delivered.iter().position(|s| s.id == id)?;
        if remove {
            self.delivered.remove(at);
        }
        Some(at)
    }

    fn binding(&mut self, intent: &crate::package::intent::Intent) -> Option<&mut Binding> {
        self.bindings
            .iter_mut()
            .find(|b| filter_equals(&b.intent.intent.intent, intent))
    }
}

impl State {
    /// When `token` restarts, `at` or later, no two services' restarts
    /// closer than `SERVICE_MIN_RESTART_TIME_BETWEEN`.
    fn stagger(&self, token: Binder, mut at: Instant) -> Instant {
        loop {
            let near = self.records.iter().filter(|r| r.token != token).filter_map(|r| r.next_restart_time.filter(|_| r.app.is_none())).find(|t| {
                at.checked_add(SERVICE_MIN_RESTART_TIME_BETWEEN).is_none_or(|a| a > *t)
                    && t.checked_add(SERVICE_MIN_RESTART_TIME_BETWEEN).is_none_or(|b| at < b)
            });
            match near.and_then(|t| t.checked_add(SERVICE_MIN_RESTART_TIME_BETWEEN)) {
                Some(later) => at = later,
                None => return at,
            }
        }
    }

    fn record(&mut self, token: Binder) -> Option<&mut Record> {
        self.records.iter_mut().find(|r| r.token == token)
    }

    /// `hasAutoCreateConnections`.
    fn auto_created(&self, token: Binder) -> bool {
        self.connections
            .iter()
            .any(|c| c.service == token && c.flags & BIND_AUTO_CREATE != 0)
    }

    /// The connections of a binding.
    fn of_binding<'a>(
        &'a self,
        token: Binder,
        intent: &'a crate::package::intent::Intent,
    ) -> impl Iterator<Item = &'a Connection> {
        self.connections
            .iter()
            .filter(move |c| c.service == token && filter_equals(&c.intent, intent))
    }
}

/// `IServiceConnection.connected(name, service, dead)`, one-way.
fn connected(conn: &Conn, package: &str, class: &str, service: Option<Binder>, dead: bool) {
    let conn = match conn {
        Conn::App(conn) => conn,
        Conn::Host(conn) => return conn.connected(service, dead),
    };
    let mut p = Parcel::new();
    p.write_interface_token(CONNECTION_DESCRIPTOR);
    aim_service_aidl::write_typed(&mut p, Some(&Component(package, class)));
    p.write_binder(service);
    p.write_bool(dead);
    if let Err(s) = conn.transact(CONNECTED, &p, true) {
        eprintln!("app host: service {package}/{class}: connected: status {s}");
    }
}

/// What a lookup of a service found (`ServiceLookupResult`).
enum Lookup {
    /// No such service.
    None,
    /// One the caller may not reach, and why.
    Denied(String),
    Found(Box<ServiceInfo>),
}

impl AppHost {
    /// `retrieveServiceLocked`: the service `intent` resolves to for
    /// `uid`, and whether the caller may reach it; the unsafe intent events
    /// of a caller's start, bind or stop told to its process `pid`'s
    /// strict mode callback.
    #[allow(clippy::too_many_arguments)]
    fn retrieve_service(
        &self,
        intent: &RawIntent,
        resolved_type: Option<&str>,
        uid: i32,
        pid: Option<i32>,
        user: i32,
        instance: Option<&str>,
        external: Option<&str>,
    ) -> Result<Lookup, Exception> {
        let (_, resolution) = self.packages()?;
        let report = std::cell::RefCell::new(Vec::new());
        let found = resolution
            .resolve_service_for_start(&intent.intent, resolved_type, STOCK_PM_FLAGS, user, uid, &report)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("resolveService: {e:?}")))?;
        self.report_unsafe_intent(pid, intent, report.into_inner());
        let Some(crate::package::component_resolver::Info::Service(mut info)) = found.map(|r| r.info)
        else {
            return Ok(Lookup::None);
        };
        let name = || format!("ComponentInfo{{{}/{}}}", info.info.item.package_name.as_deref().unwrap_or_default(), info.info.item.name.as_deref().unwrap_or_default());
        // An external service runs under its client's application: its
        // package name and uid (with the defining package's code).
        match (info.flags & FLAG_EXTERNAL_SERVICE != 0, external) {
            (true, Some(calling_package)) => {
                let class = info.info.item.name.clone().unwrap_or_default();
                if !info.info.exported {
                    return Err(Exception::security(format!("BIND_EXTERNAL_SERVICE failed, {class} is not exported")));
                }
                if info.flags & FLAG_ISOLATED_PROCESS == 0 {
                    return Err(Exception::security(format!("BIND_EXTERNAL_SERVICE failed, {class} is not an isolatedProcess")));
                }
                if !self.owns_package(uid, calling_package)? {
                    return Err(Exception::security("BIND_EXTERNAL_SERVICE failed, calling package not owned by calling UID "));
                }
                let (state, _) = self.packages()?;
                let query = crate::package::query::Query { state: &state, filter: &resolution.apps_filter, calling_uid: 1000 };
                let client = query
                    .application_info(calling_package, STOCK_PM_FLAGS, user)
                    .map_err(|e| self.unimplemented_named("activity", &format!("an external service's client: {} (#1492)", e.0)))??
                    .ok_or_else(|| Exception::security(format!("BIND_EXTERNAL_SERVICE failed, could not resolve client package {calling_package}")))?;
                let mut app = (*info.info.application_info).clone();
                app.item.package_name = client.item.package_name.clone();
                app.uid = client.uid;
                info.info.application_info = Arc::new(app);
            }
            (true, None) => return Err(Exception::security(format!("BIND_EXTERNAL_SERVICE required for {}", name()))),
            (false, Some(_)) => return Err(Exception::security(format!("BIND_EXTERNAL_SERVICE failed, {} is not an externalService", name()))),
            (false, None) => {}
        }
        if instance.is_some() && info.flags & FLAG_ISOLATED_PROCESS == 0 {
            return Err(Exception::illegal_argument(format!(
                "Can't use instance name '{}' with non-isolated service '{}'",
                instance.unwrap_or_default(),
                info.info.item.name.as_deref().unwrap_or_default()
            )));
        }
        let target = info.info.application_info.uid;
        if !self.check_component_permission(
            info.permission.as_deref(),
            uid,
            target,
            info.info.exported,
        )? {
            return Ok(Lookup::Denied(if info.info.exported {
                info.permission.clone().unwrap_or_default()
            } else {
                format!("not exported from uid {target}")
            }));
        }
        Ok(Lookup::Found(Box::new(info)))
    }

    /// The record of `info`, made if it has none (`ServiceMap`).
    fn service_record(
        state: &mut State,
        info: ServiceInfo,
        instance: Option<String>,
        process: &Arc<aim_binder_host::local::LocalProcess>,
    ) -> Binder {
        let package = info
            .info
            .application_info
            .item
            .package_name
            .clone()
            .unwrap_or_default();
        let class = info.info.item.name.clone().unwrap_or_default();
        if let Some(r) = state
            .records
            .iter()
            .find(|r| r.package == package && r.class == class && r.instance == instance)
        {
            return r.token;
        }
        let token = process.add_service(Arc::new(super::activity::Token));
        state.records.push(Record {
            token,
            package,
            class,
            instance,
            info,
            app: None,
            created: false,
            start_requested: false,
            stop_if_killed: false,
            call_start: false,
            last_start_id: 0,
            pending: Vec::new(),
            delivered: Vec::new(),
            bindings: Vec::new(),
            restart_count: 0,
            restart_delay: Duration::ZERO,
            restart_time: None,
            next_restart_time: None,
            crash_count: 0,
            create_real_time: super::clock_ms(libc::CLOCK_MONOTONIC_RAW),
            last_activity: super::clock_ms(libc::CLOCK_UPTIME_RAW),
            fg: Foreground::default(),
        });
        token
    }

    /// `startServiceLocked`, of a foreground service (`fg_required`,
    /// `startForegroundService`) or not, by `uid` of `calling_package`: the
    /// service's name, `!` (or `!!`) and why if it may not be started, none
    /// if there is no such service or a foreground one is silently not.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_service(
        &self,
        caller: Option<Binder>,
        uid: i32,
        pid: Option<i32>,
        intent: RawIntent,
        resolved_type: Option<String>,
        mut fg_required: bool,
        calling_package: Option<&str>,
        user: i32,
    ) -> Result<Option<(String, String)>, Exception> {
        if intent.has_fds() {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Intent",
            ));
        }
        if caller.is_some() && self.app_of_thread(caller).is_none() {
            return Err(Exception::security(
                "Unable to find app for caller when starting service",
            ));
        }
        let user = self.incoming_user(uid, user, false)?;
        let info =
            match self.retrieve_service(&intent, resolved_type.as_deref(), uid, pid, user, None, None)? {
                Lookup::None => return Ok(None),
                Lookup::Denied(why) => {
                    return Ok(Some((
                        "!".into(),
                        if why.is_empty() {
                            "private to package".into()
                        } else {
                            why
                        },
                    )));
                }
                Lookup::Found(info) => *info,
            };
        // `startServiceInnerLocked`: the intent's grants, checked for the
        // caller (before the services are locked: a provider may be asked).
        let package = info.info.application_info.item.package_name.as_deref().unwrap_or_default();
        let grants = self.check_grant_from_intent(&intent, uid, package, user)?.map(|n| (n, self.new_start_owner()));
        let kept = KeptIntent::new(intent, &self.process);
        let mut state = self.services.state.lock().unwrap();
        let token = Self::service_record(&mut state, info, None, &self.process);
        self.set_fgs_restriction(&mut state, token, calling_package, uid, false)?;
        let r = state.record(token).unwrap();
        let (app_uid, package, target_sdk) =
            (r.info.info.application_info.uid, r.package.clone(), r.info.info.application_info.target_sdk_version);
        if fg_required {
            if !r.fg.allowed_start() && self.bg_fgs_restricted(app_uid, uid)? {
                if r.fg.owner_unknown {
                    return Err(self.owner_exemption());
                }
                let message = format!(
                    "startForegroundService() not allowed due to mAllowStartForeground false: service {}",
                    short_name(&r.package, &r.class)
                );
                eprintln!("app host: {message}");
                if self.change_enabled(FGS_START_EXCEPTION_CHANGE_ID, uid)? {
                    return Err(parcelable_exception("android.app.ForegroundServiceStartNotAllowedException", message));
                }
                return Ok(None);
            }
            match appops::mode(self, appops::OP_START_FOREGROUND, app_uid, Some(&package))? {
                MODE_ALLOWED | MODE_DEFAULT => {}
                // Started as a service that is not foreground; the app
                // host's apps are all allowed to start those.
                appops::MODE_IGNORED => {
                    eprintln!("app host: startForegroundService not allowed due to app op: service {package}");
                    fg_required = false;
                }
                _ => return Ok(Some(("!!".into(), "foreground not allowed as per app op".into()))),
            }
            // Only an app targeting O or later must call startForeground.
            fg_required &= target_sdk >= O;
        }
        let r = state.record(token).unwrap();
        r.start_requested = true;
        r.stop_if_killed = false;
        r.fg.required = fg_required;
        let id = r.next_start_id();
        r.pending.push(StartItem {
            id,
            intent: Some(kept),
            grants,
            delivery_count: 0,
            done_executing_count: 0,
            delivered_time: None,
        });
        let name = (r.package.clone(), r.class.clone());
        self.bring_up(&mut state, token)?;
        Ok(Some(name))
    }

    /// `stopServiceLocked`: 1 if a started service was stopped, 0 if it was
    /// not started, -1 if there is none.
    pub(super) fn stop_service(
        &self,
        uid: i32,
        pid: i32,
        intent: RawIntent,
        resolved_type: Option<String>,
        user: i32,
    ) -> Result<i32, Exception> {
        if intent.has_fds() {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Intent",
            ));
        }
        let user = self.incoming_user(uid, user, false)?;
        let info =
            match self.retrieve_service(&intent, resolved_type.as_deref(), uid, Some(pid), user, None, None)? {
                Lookup::Found(info) => info,
                Lookup::Denied(why) => {
                    return Err(Exception::security(format!(
                        "Not allowed to stop service: {why}"
                    )));
                }
                Lookup::None => return Ok(-1),
            };
        let class = info.info.item.name.clone().unwrap_or_default();
        let package = info
            .info
            .application_info
            .item
            .package_name
            .clone()
            .unwrap_or_default();
        let mut state = self.services.state.lock().unwrap();
        let Some(r) = state
            .records
            .iter_mut()
            .find(|r| r.package == package && r.class == class && r.instance.is_none())
        else {
            return Ok(0);
        };
        if !r.start_requested {
            return Ok(0);
        }
        r.start_requested = false;
        let token = r.token;
        self.bring_down_if_needed(&mut state, token);
        Ok(1)
    }

    /// `stopServiceTokenLocked` (`stopSelf`): whether it was stopped.
    pub(super) fn stop_service_token(
        &self,
        name: Option<Name>,
        token: Option<Binder>,
        start_id: i32,
    ) -> bool {
        let mut state = self.services.state.lock().unwrap();
        let Some(r) = token.and_then(|t| state.record(t)) else {
            return false;
        };
        if name.is_none_or(|n| n.package != r.package || n.class != r.class) {
            return false;
        }
        if start_id >= 0 {
            // The starts delivered up to this one are done.
            if let Some(at) = r.delivered.iter().position(|s| s.id == start_id) {
                for s in r.delivered.drain(..=at) {
                    self.release_start_grants(&s);
                }
            }
            if r.last_start_id != start_id {
                return false;
            }
        }
        r.start_requested = false;
        let token = r.token;
        self.bring_down_if_needed(&mut state, token);
        true
    }

    /// `bindServiceLocked`: 1 if bound, 0 if the binding is refused, -1 if
    /// there is no such service.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_service(
        &self,
        caller: Option<Binder>,
        uid: i32,
        pid: i32,
        activity: Option<Binder>,
        intent: RawIntent,
        resolved_type: Option<String>,
        connection: Option<Binder>,
        flags: i64,
        instance: Option<String>,
        calling_package: Option<String>,
        user: i32,
    ) -> Result<i32, Exception> {
        if intent.has_fds() {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Intent",
            ));
        }
        let Some((client, _, client_uid, _)) = self.app_of_thread(caller) else {
            return Err(Exception::security(
                "Unable to find app for caller when binding service",
            ));
        };
        let Some(Binder::Handle(conn)) = connection else {
            return Err(Exception::illegal_argument("connection is null"));
        };
        // `getServiceConnectionsHolder`: the activity must be known.
        let activity = match activity {
            Some(Binder::Local(token))
                if self.activities.records.lock().unwrap().contains_key(&token) =>
            {
                Some(token)
            }
            Some(_) => return Ok(0),
            None => None,
        };
        // A system client may say what the binding is for, its intent then
        // only its filter.
        let bad = |e| Exception::new(EX_BAD_PARCELABLE, format!("bindService: intent: {e:?}"));
        let client_label = if client_uid == SYSTEM_UID { intent.client_label().map_err(bad)? } else { 0 };
        let intent = if client_label != 0 { intent.clone_filter().map_err(bad)? } else { intent };
        let external = flags & (BIND_EXTERNAL_SERVICE | BIND_EXTERNAL_SERVICE_LONG) != 0;
        if flags & (BIND_SHARED_ISOLATED_PROCESS | BIND_PACKAGE_ISOLATED_PROCESS) != 0 {
            return Err(
                self.unimplemented_named("activity", "a binding into a shared isolated process (#1492)")
            );
        }
        let user = self.incoming_user(uid, user, false)?;
        let info = match self.retrieve_service(
            &intent,
            resolved_type.as_deref(),
            uid,
            Some(pid),
            user,
            instance.as_deref(),
            external.then_some(calling_package.as_deref().unwrap_or_default()),
        )? {
            Lookup::None => return Ok(0),
            Lookup::Denied(_) => return Ok(-1),
            Lookup::Found(info) => *info,
        };
        let conn = Conn::App(self.process.strong(conn));
        self.bind_found(intent, info, instance, conn, Some(client), activity, flags, client_label)?;
        Ok(1)
    }

    /// `bindServiceAsUser` of the system: the service `package`/`class`
    /// of `user` bound with `bind_flags` for `conn`, by an intent of
    /// `action` and `flags`; whether there is one to bind.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_service_from_host(
        &self,
        action: Option<&str>,
        package: &str,
        class: &str,
        flags: i32,
        bind_flags: i64,
        user: i32,
        conn: Arc<dyn HostConnection>,
    ) -> Result<bool, Exception> {
        let intent = RawIntent::of(action, flags, &[]).with_component(package, class);
        let info = match self.retrieve_service(&intent, None, 1000, None, user, None, None)? {
            Lookup::Found(info) => *info,
            Lookup::None | Lookup::Denied(_) => return Ok(false),
        };
        self.bind_found(intent, info, None, Conn::Host(conn), None, None, bind_flags, 0)?;
        Ok(true)
    }

    /// `unbindService` of a connection of the app host.
    pub(super) fn unbind_service_from_host(&self, conn: &Arc<dyn HostConnection>) {
        let mut state = self.services.state.lock().unwrap();
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.connections)
            .into_iter()
            .partition(|c| matches!(&c.conn, Conn::Host(h) if Arc::ptr_eq(h, conn)));
        state.connections = kept;
        for c in &gone {
            self.connection_removed(&mut state, c);
        }
    }

    /// The rest of `bindServiceLocked`, for the service `info` found: the
    /// connection made, the service brought up if it auto-creates, and its
    /// binder passed or asked for.
    #[allow(clippy::too_many_arguments)]
    fn bind_found(
        &self,
        intent: RawIntent,
        info: ServiceInfo,
        instance: Option<String>,
        conn: Conn,
        client: Option<i64>,
        activity: Option<u64>,
        flags: i64,
        client_label: i32,
    ) -> Result<(), Exception> {
        let kept = KeptIntent::new(intent, &self.process);
        let mut state = self.services.state.lock().unwrap();
        let token = Self::service_record(&mut state, info, instance, &self.process);
        let r = state.record(token).unwrap();
        if r.binding(&kept.intent.intent).is_none() {
            r.bindings.push(Binding {
                intent: kept.clone(),
                binder: None,
                requested: false,
                received: false,
                has_bound: false,
                do_rebind: false,
            });
        }
        let (package, class) = (r.package.clone(), r.class.clone());
        state.connections.push(Connection {
            conn,
            service: token,
            intent: kept.intent.intent.clone(),
            client,
            activity,
            flags,
            client_label,
        });
        if flags & BIND_AUTO_CREATE != 0 {
            self.bring_up(&mut state, token)?;
        }
        let r = state.record(token).unwrap();
        let created = r.created;
        let b = r.binding(&kept.intent.intent).unwrap();
        if created && b.received {
            let binder = b.binder.as_ref().map(|s| Binder::Handle(s.handle));
            let rebind = b.do_rebind;
            let conn = state.connections.last().unwrap();
            connected(&conn.conn, &package, &class, binder, false);
            if rebind && state.of_binding(token, &kept.intent.intent).count() == 1 {
                self.request_binding(&mut state, token, &kept.intent.intent, true);
            }
        } else if !b.requested {
            self.request_binding(&mut state, token, &kept.intent.intent, false);
        }
        Ok(())
    }

    /// `unbindServiceLocked`: whether the connection was bound.
    pub(super) fn unbind_service(&self, connection: Option<Binder>) -> bool {
        let Some(Binder::Handle(conn)) = connection else {
            return false;
        };
        let mut state = self.services.state.lock().unwrap();
        let removed: Vec<Connection> = {
            let (gone, kept) = std::mem::take(&mut state.connections)
                .into_iter()
                .partition(|c| matches!(&c.conn, Conn::App(s) if s.handle == conn));
            state.connections = kept;
            gone
        };
        if removed.is_empty() {
            return false;
        }
        for c in &removed {
            self.connection_removed(&mut state, c);
        }
        true
    }

    /// `updateServiceGroupLocked` of an app's connection. A group and
    /// importance order cached isolated processes for OomAdjuster, which
    /// the app host, whose processes are all top, does not have (#1738):
    /// they change nothing.
    pub(super) fn update_service_group(&self, connection: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(conn)) = connection else {
            return Err(Exception::new(EX_NULL_POINTER, "connection"));
        };
        let state = self.services.state.lock().unwrap();
        if !state.connections.iter().any(|c| matches!(&c.conn, Conn::App(s) if s.handle == conn)) {
            return Err(Exception::illegal_argument(format!("Could not find connection for android.os.BinderProxy@{conn:x}")));
        }
        Ok(())
    }

    /// `removeConnectionLocked`'s effects of `c`'s removal: the last
    /// connection of a binding unbinds it, and the service goes if no
    /// longer needed.
    fn connection_removed(&self, state: &mut State, c: &Connection) {
        let left = state.of_binding(c.service, &c.intent).count();
        if let Some(r) = state.record(c.service) {
            let app = r.app.filter(|_| r.created);
            let (token, name) = (r.token, format!("{}/{}", r.package, r.class));
            if let (Some(app), Some(b)) = (app, r.binding(&c.intent))
                && left == 0
                && b.has_bound
            {
                b.has_bound = false;
                b.do_rebind = false;
                let mut p = Parcel::new();
                thread::ScheduleUnbindService {
                    token: Some(token),
                    intent: Some(By(&b.intent.intent)),
                }
                .write(&mut p);
                if let Err(e) = self.call_thread(app, thread::SCHEDULE_UNBIND_SERVICE, &p) {
                    eprintln!("app host: service {name}: unbind: {e}");
                }
            }
        }
        if c.flags & BIND_AUTO_CREATE != 0 {
            self.bring_down_if_needed(state, c.service);
        }
    }

    /// `publishServiceLocked`: the service's binder for `intent`, passed
    /// to its connections.
    pub(super) fn publish_service(
        &self,
        token: Option<Binder>,
        intent: Option<RawIntent>,
        service: Option<Binder>,
    ) {
        let (Some(token), Some(intent)) = (token, intent) else {
            return;
        };
        let mut state = self.services.state.lock().unwrap();
        let held = match service {
            Some(Binder::Handle(h)) => Some(self.process.strong(h)),
            _ => None,
        };
        let Some(r) = state.record(token) else { return };
        let (package, class) = (r.package.clone(), r.class.clone());
        let Some(b) = r.binding(&intent.intent) else {
            return;
        };
        if b.received {
            return;
        }
        let binder = held.as_ref().map(|s| Binder::Handle(s.handle));
        b.binder = held;
        b.requested = true;
        b.received = true;
        for c in state.of_binding(token, &intent.intent) {
            connected(&c.conn, &package, &class, binder, false);
        }
    }

    /// `unbindFinishedLocked`: the service's `onUnbind` returned; a binding
    /// whose clients came back meanwhile is bound again, else it is marked
    /// to be rebound (`onUnbind` returned true).
    pub(super) fn unbind_finished(&self, token: Option<Binder>, intent: Option<RawIntent>) {
        let (Some(token), Some(intent)) = (token, intent) else {
            return;
        };
        let mut state = self.services.state.lock().unwrap();
        let clients = state.of_binding(token, &intent.intent).count();
        let Some(b) = state.record(token).and_then(|r| r.binding(&intent.intent)) else {
            return;
        };
        if clients > 0 {
            self.request_binding(&mut state, token, &intent.intent, true);
        } else {
            b.do_rebind = true;
        }
    }

    /// `serviceDoneExecutingLocked` of a start: what its result asks for a
    /// restart, and the start done.
    pub(super) fn service_done_executing(
        &self,
        token: Option<Binder>,
        kind: i32,
        start_id: i32,
        res: i32,
    ) -> Result<(), Exception> {
        if kind != SERVICE_DONE_EXECUTING_START {
            return Ok(());
        }
        let mut state = self.services.state.lock().unwrap();
        let Some(r) = token.and_then(|t| state.record(t)) else {
            return Ok(());
        };
        r.call_start = true;
        match res {
            START_STICKY_COMPATIBILITY | START_STICKY => {
                r.take_delivered(start_id, true);
                r.stop_if_killed = false;
            }
            START_NOT_STICKY => {
                r.take_delivered(start_id, true);
                if r.last_start_id == start_id {
                    r.stop_if_killed = true;
                }
            }
            START_REDELIVER_INTENT => {
                if let Some(at) = r.take_delivered(start_id, false) {
                    r.delivered[at].delivery_count = 0;
                    r.delivered[at].done_executing_count += 1;
                    r.stop_if_killed = true;
                }
            }
            START_TASK_REMOVED_COMPLETE => {
                r.take_delivered(start_id, true);
            }
            _ => {
                return Err(Exception::illegal_argument(format!(
                    "Unknown service start result: {res}"
                )));
            }
        }
        if res == START_STICKY_COMPATIBILITY {
            r.call_start = false;
        }
        Ok(())
    }

    /// `bringUpServiceLocked`: the service created and its starts sent in
    /// its process, which is started if it does not run.
    fn bring_up(&self, state: &mut State, token: Binder) -> Result<(), Exception> {
        let r = state.record(token).unwrap();
        if r.created {
            self.send_args(state, token);
            return Ok(());
        }
        // No longer restarting.
        r.next_restart_time = None;
        if r.app.is_some() {
            // Waiting for its process to attach.
            return Ok(());
        }
        let package = r.package.clone();
        // The `android` package's services are system_server's own code,
        // which no process the app host starts can load (#1518).
        if package == super::SYSTEM_PACKAGE {
            return Err(self.unimplemented_named("activity", "a service of system_server's own code (#1518)"));
        }
        let process = super::providers::process_name(&r.info.info);
        // `getProcessNameForService`: an isolated service runs alone in a
        // new isolated process named after it.
        if r.info.flags & FLAG_ISOLATED_PROCESS != 0 {
            let process = format!("{process}:{}", r.class);
            // The WebView provider's services start by the WebView zygote
            // (`HostingRecord.byWebviewZygote`).
            let defining = r.info.info.item.package_name.as_deref();
            let (seq, _) = if defining.is_some() && self.webview_zygote_package().as_deref() == defining {
                self.start_webview_service_process(r.info.info.application_info.clone(), &process)?
            } else {
                self.start_isolated_process(&package, &process)?
            };
            state.record(token).unwrap().app = Some(seq);
            return Ok(());
        }
        let running = self.running_process(&process, r.info.info.application_info.uid as u32);
        match running {
            Some((seq, attached)) => {
                state.record(token).unwrap().app = Some(seq);
                if attached {
                    self.real_start(state, token);
                }
            }
            None => {
                // `startProcessLocked` with hosting type `service`.
                let pid = self.start_process_for(&package, &process, None, false)?;
                let seq = self
                    .apps
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(_, a)| a.pid == pid)
                    .map(|(seq, _)| *seq);
                let r = state.record(token).unwrap();
                r.app = seq;
                // `fg-service-launch`: its app may make it foreground.
                if r.fg.required {
                    let uid = r.info.info.application_info.uid;
                    Self::temp_allowlist(state, uid, SERVICE_START_FOREGROUND_TIMEOUT, REASON_SERVICE_LAUNCH);
                }
            }
        }
        Ok(())
    }

    /// `realStartServiceLocked`: `scheduleCreateService`, then its
    /// bindings and starts.
    fn real_start(&self, state: &mut State, token: Binder) {
        let r = state.record(token).unwrap();
        let Some(app) = r.app else { return };
        let config = self
            .apps
            .lock()
            .unwrap()
            .get(&app)
            .and_then(|a| a.config.clone());
        let Some(config) = config else { return };
        r.restart_time = Some(Instant::now());
        let Ok(inputs) = self.inputs() else { return };
        let compat = super::config::CompatibilityInfo::of(&r.info.info.application_info, &config, inputs.device_density);
        let mut p = Parcel::new();
        thread::ScheduleCreateService {
            token: Some(r.token),
            info: Some(By(&r.info)),
            compat_info: Some(compat),
            process_state: PROCESS_STATE_TOP,
        }
        .write(&mut p);
        if let Err(e) = self.call_thread(app, thread::SCHEDULE_CREATE_SERVICE, &p) {
            eprintln!("app host: service {}/{}: create: {e}", r.package, r.class);
            return;
        }
        eprintln!("app host: service {}/{} created", r.package, r.class);
        r.created = true;
        let intents: Vec<_> = r
            .bindings
            .iter()
            .map(|b| b.intent.intent.intent.clone())
            .collect();
        for intent in intents {
            if state.of_binding(token, &intent).next().is_some() {
                self.request_binding(state, token, &intent, false);
            }
        }
        let r = state.record(token).unwrap();
        // A sticky service started again gets a start without an intent.
        if r.start_requested && r.call_start && r.pending.is_empty() {
            let id = r.next_start_id();
            r.pending.push(StartItem {
                id,
                intent: None,
                grants: None,
                delivery_count: 0,
                done_executing_count: 0,
                delivered_time: None,
            });
        }
        self.send_args(state, token);
    }

    /// `requestServiceBindingLocked`: `scheduleBindService` of a binding
    /// with clients not yet asked for (or to be asked again).
    fn request_binding(
        &self,
        state: &mut State,
        token: Binder,
        intent: &crate::package::intent::Intent,
        rebind: bool,
    ) {
        let clients = state.of_binding(token, intent).count();
        state.next_bind_seq += 1;
        let seq = state.next_bind_seq;
        let Some(r) = state.record(token) else { return };
        let Some(app) = r.app.filter(|_| r.created) else {
            return;
        };
        let r_token = r.token;
        let name = format!("{}/{}", r.package, r.class);
        let Some(b) = r.binding(intent) else { return };
        if (b.requested && !rebind) || clients == 0 {
            return;
        }
        let mut p = Parcel::new();
        thread::ScheduleBindService {
            token: Some(r_token),
            intent: Some(By(&b.intent.intent)),
            rebind,
            process_state: PROCESS_STATE_TOP,
            bind_seq: seq,
        }
        .write(&mut p);
        if let Err(e) = self.call_thread(app, thread::SCHEDULE_BIND_SERVICE, &p) {
            eprintln!("app host: service {name}: bind: {e}");
            return;
        }
        if !rebind {
            b.requested = true;
        }
        b.has_bound = true;
        b.do_rebind = false;
    }

    /// `sendServiceArgsLocked`: the pending starts delivered.
    fn send_args(&self, state: &mut State, token: Binder) {
        let generation = state.next_timeout + 1;
        let Some(r) = state.record(token) else { return };
        let Some(app) = r.app.filter(|_| r.created) else {
            return;
        };
        let n = r.pending.len();
        let mut args = Vec::new();
        let mut timeout = false;
        for mut s in std::mem::take(&mut r.pending) {
            if s.intent.is_none() && n > 1 {
                continue;
            }
            s.delivery_count += 1;
            s.delivered_time = Some(Instant::now());
            if let Some((needed, owner)) = &s.grants
                && let Err(e) = self.grant_from_intent(Some(needed), Some(*owner))
            {
                eprintln!("app host: service {}/{}: URI grants: {}", r.package, r.class, e.message);
            }
            // A service started with `startForegroundService` must call
            // `startForeground` in time
            // (`scheduleServiceForegroundTransitionTimeoutLocked`).
            if r.fg.required && r.fg.waiting.is_none() {
                if r.fg.is_foreground {
                    r.fg.required = false;
                } else {
                    timeout = true;
                    r.fg.waiting = Some(generation);
                }
            }
            let mut flags = 0;
            if s.delivery_count > 1 {
                flags |= START_FLAG_RETRY;
            }
            if s.done_executing_count > 0 {
                flags |= START_FLAG_REDELIVERY;
            }
            args.push(StartArgs {
                id: s.id,
                flags,
                intent: s.intent.as_ref().map(|k| k.intent.clone()),
            });
            r.delivered.push(s);
        }
        if args.is_empty() {
            return;
        }
        let mut p = Parcel::new();
        thread::ScheduleServiceArgs {
            token: Some(r.token),
            args: Some(Slice::new(
                self.process.clone(),
                "android.app.ServiceStartArgs",
                args,
            )),
        }
        .write(&mut p);
        if let Err(e) = self.call_thread(app, thread::SCHEDULE_SERVICE_ARGS, &p) {
            eprintln!("app host: service {}/{}: start: {e}", r.package, r.class);
        }
        if timeout {
            state.next_timeout = generation;
            let me = self.me.clone();
            std::thread::spawn(move || {
                std::thread::sleep(SERVICE_START_FOREGROUND_TIMEOUT);
                if let Some(host) = me.upgrade() {
                    host.service_foreground_timeout(token, generation);
                }
            });
        }
    }

    /// `StartItem.removeUriPermissionsLocked`.
    fn release_start_grants(&self, s: &StartItem) {
        if let Some((_, owner)) = &s.grants {
            self.release_uri_grants(*owner);
        }
    }

    /// `bringDownServiceIfNeededLocked`: a service neither started nor
    /// bound to with `BIND_AUTO_CREATE` is destroyed (`bringDownServiceLocked`).
    fn bring_down_if_needed(&self, state: &mut State, token: Binder) {
        let needed =
            state.record(token).is_some_and(|r| r.start_requested) || state.auto_created(token);
        if needed {
            return;
        }
        let Some(at) = state.records.iter().position(|r| r.token == token) else {
            return;
        };
        let r = state.records.remove(at);
        // `clearDeliveredStartsLocked`.
        for s in &r.delivered {
            self.release_start_grants(s);
        }
        // Its connections learn it is gone.
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.connections)
            .into_iter()
            .partition(|c| c.service == token);
        state.connections = kept;
        for c in &gone {
            connected(&c.conn, &r.package, &r.class, None, true);
        }
        // Brought down before it called `startForeground`, which is not
        // allowed: its app is crashed (`SERVICE_FOREGROUND_CRASH_MSG`).
        if r.fg.required
            && let Some(seq) = r.app
        {
            eprintln!("app host: bringing down service while still waiting for start foreground: {}", short_name(&r.package, &r.class));
            let message = format!(
                "Context.startForegroundService() did not then call Service.startForeground(): ServiceRecord{{u{} {}}}",
                r.info.info.application_info.uid / PER_USER_RANGE as i32,
                short_name(&r.package, &r.class)
            );
            let extras = vec![("serviceclassname".to_string(), Value::String(Some(r.class.clone())))];
            self.post_job(move |host| host.crash_application(seq, &message, FOREGROUND_SERVICE_DID_NOT_START_IN_TIME, extras));
        }
        if let Some(shown) = r.fgs_notification() {
            self.cancel_foreground_notification(state, token, &shown);
        }
        let Some(app) = r.app.filter(|_| r.created) else {
            return;
        };
        for b in r.bindings.iter().filter(|b| b.has_bound) {
            let mut p = Parcel::new();
            thread::ScheduleUnbindService {
                token: Some(r.token),
                intent: Some(By(&b.intent.intent)),
            }
            .write(&mut p);
            let _ = self.call_thread(app, thread::SCHEDULE_UNBIND_SERVICE, &p);
        }
        let mut p = Parcel::new();
        thread::ScheduleStopService {
            token: Some(r.token),
        }
        .write(&mut p);
        match self.call_thread(app, thread::SCHEDULE_STOP_SERVICE, &p) {
            Ok(()) => eprintln!("app host: service {}/{} destroyed", r.package, r.class),
            Err(e) => eprintln!("app host: service {}/{}: stop: {e}", r.package, r.class),
        }
    }

    /// `attachApplicationLocked`: the services waiting for the process of
    /// `start_seq` are created in it.
    pub(super) fn services_attached(&self, start_seq: i64) {
        let mut state = self.services.state.lock().unwrap();
        let waiting: Vec<Binder> = state
            .records
            .iter()
            .filter(|r| r.app == Some(start_seq) && !r.created)
            .map(|r| r.token)
            .collect();
        for token in waiting {
            self.real_start(&mut state, token);
        }
    }

    /// The process of `start_seq` crashed (`incServiceCrashCountLocked`):
    /// the crash count of each service running in it goes up, or starts
    /// over when the service started long enough ago. Whether to try its
    /// services again after it crashed too often: a foreground one that
    /// did not crash too often is.
    pub(super) fn services_crashed(&self, start_seq: i64) -> bool {
        let now = Instant::now();
        let mut state = self.services.state.lock().unwrap();
        let mut try_again = false;
        for r in state.records.iter_mut().filter(|r| r.app == Some(start_seq) && r.created) {
            if r.restart_time.is_none_or(|t| now > t + MIN_CRASH_INTERVAL) {
                r.crash_count = 1;
            } else {
                r.crash_count += 1;
            }
            try_again |= r.crash_count < BOUND_SERVICE_MAX_CRASH_RETRY && r.fg.is_foreground;
        }
        try_again
    }

    /// The process of `start_seq` died (`killServicesLocked`): its
    /// connections go, its services' clients are told, and each service
    /// still needed is started again after its back-off
    /// (`scheduleServiceRestartLocked`), unless `allow_restart` is off or
    /// it crashed too often.
    pub(super) fn services_process_died(self: &Arc<Self>, start_seq: i64, allow_restart: bool) {
        let mut state = self.services.state.lock().unwrap();
        // Its services are no longer in it, so that its connections to them
        // are not unbound in it.
        let tokens: Vec<Binder> = state
            .records
            .iter_mut()
            .filter(|r| r.app == Some(start_seq))
            .map(|r| {
                r.app = None;
                r.created = false;
                r.token
            })
            .collect();
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.connections)
            .into_iter()
            .partition(|c| c.client == Some(start_seq));
        state.connections = kept;
        for c in &gone {
            self.connection_removed(&mut state, c);
        }
        let now = Instant::now();
        let mut restart = Vec::new();
        for token in tokens {
            let auto = state.auto_created(token);
            // Brought down with its last connection.
            let Some(r) = state.record(token) else { continue };
            for b in &mut r.bindings {
                b.binder = None;
                b.requested = false;
                b.received = false;
                b.has_bound = false;
            }
            let (package, class) = (r.package.clone(), r.class.clone());
            // Its delivered starts' grants go, whether they are delivered
            // again or it is brought down.
            for s in &r.delivered {
                self.release_start_grants(s);
            }
            let delay = if !allow_restart {
                None
            } else if r.crash_count >= BOUND_SERVICE_MAX_CRASH_RETRY {
                eprintln!("app host: service {package}/{class} crashed {} times, stopping", r.crash_count);
                None
            } else {
                r.schedule_restart(auto, now)
            };
            // Its notification goes, and the wait for `startForeground`
            // ends: a restarted service calls it again.
            r.fg.waiting = None;
            if let Some(shown) = r.fgs_notification() {
                self.cancel_foreground_notification(&state, token, &shown);
            }
            for c in state.connections.iter().filter(|c| c.service == token) {
                connected(&c.conn, &package, &class, None, true);
            }
            // A back-off past what an `Instant` holds (a service that kept
            // crashing) is a restart that never comes.
            match delay.map(|d| (d, now.checked_add(d))) {
                Some((delay, None)) => {
                    let r = state.record(token).unwrap();
                    r.restart_delay = delay;
                    r.next_restart_time = None;
                    eprintln!("app host: not restarting crashed service {package}/{class}: its back-off is {delay:?}");
                }
                Some((_, Some(at))) => {
                    let at = state.stagger(token, at);
                    let r = state.record(token).unwrap();
                    r.restart_delay = at - now;
                    r.next_restart_time = Some(at);
                    eprintln!("app host: scheduling restart of crashed service {package}/{class} in {:?}", r.restart_delay);
                    restart.push((token, at));
                }
                None => state.records.retain(|r| r.token != token),
            }
        }
        drop(state);
        for (token, at) in restart {
            let me = Arc::downgrade(self);
            std::thread::spawn(move || {
                std::thread::sleep(at.saturating_duration_since(Instant::now()));
                let Some(host) = me.upgrade() else { return };
                let mut state = host.services.state.lock().unwrap();
                // `performServiceRestartLocked`, unless it started since.
                if state.record(token).is_some_and(|r| r.app.is_none() && r.next_restart_time == Some(at))
                    && let Err(e) = host.bring_up(&mut state, token)
                {
                    eprintln!("app host: restarting a service: {}", e.message);
                }
            });
        }
    }

    /// `getServices` (`ActivityManager.getRunningServices`) of `uid`: the
    /// services it may see, at most `max`. The call's flags are unused, as
    /// in the original.
    pub(super) fn running_services(&self, uid: i32, max: i32) -> Result<Vec<Option<RunningServiceInfo>>, Exception> {
        if super::is_isolated(uid) {
            return Err(Exception::security("Isolated process not allowed to call getServices"));
        }
        let across_users = self.check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)? == 0;
        let allowed = self.get_tasks_allowed(uid)?;
        let (now, uptime) = (Instant::now(), super::clock_ms(libc::CLOCK_UPTIME_RAW));
        let state = self.services.state.lock().unwrap();
        let apps = self.apps.lock().unwrap();
        let services: Vec<Running> = state
            .records
            .iter()
            .map(|r| {
                let app = r.app.filter(|_| r.created).and_then(|seq| apps.get(&seq));
                let clients: Vec<&Connection> = state.connections.iter().filter(|c| c.service == r.token).collect();
                // Its connections, by their `IServiceConnection`.
                let client_count = clients.iter().enumerate().filter(|(i, c)| !clients[..*i].iter().any(|d| d.conn.same(&c.conn))).count();
                let labelled = clients.iter().rev().find(|c| c.client_label != 0);
                let mut flags = if r.start_requested { FLAG_STARTED } else { 0 };
                if r.fg.is_foreground {
                    flags |= FLAG_FOREGROUND;
                }
                if app.is_some_and(|a| a.persistent) {
                    flags |= FLAG_PERSISTENT_PROCESS;
                }
                let uid = r.info.info.application_info.uid;
                Running {
                    info: RunningServiceInfo {
                        package: r.package.clone(),
                        class: r.class.clone(),
                        pid: app.map_or(0, |a| a.pid),
                        uid,
                        process: super::providers::process_name(&r.info.info),
                        foreground: r.fg.is_foreground,
                        active_since: r.create_real_time,
                        started: r.start_requested,
                        client_count: client_count as i32,
                        crash_count: r.crash_count,
                        last_activity_time: r.last_activity,
                        restarting: 0,
                        flags,
                        client_package: labelled.and_then(|c| apps.get(&c.client?)).map(|a| a.package.clone()),
                        client_label: labelled.map_or(0, |c| c.client_label),
                    },
                    user: uid / super::PER_USER_RANGE as i32,
                    app_uid: app.map(|a| a.uid as i32),
                    restarting: r.next_restart_time.map(|at| uptime + at.saturating_duration_since(now).as_millis() as i64),
                }
            })
            .collect();
        Ok(pick(&services, max, uid, allowed, across_users).into_iter().map(Some).collect())
    }

    /// The connections made from activity `token` go with it
    /// (`ActivityServiceConnectionsHolder.disconnectActivityFromServices`).
    pub(super) fn activity_connections_gone(&self, token: u64) {
        let mut state = self.services.state.lock().unwrap();
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.connections)
            .into_iter()
            .partition(|c| c.activity == Some(token));
        state.connections = kept;
        for c in &gone {
            self.connection_removed(&mut state, c);
        }
    }
}

/// What `setServiceForegroundInnerLocked` asks of the system to check a
/// foreground service's type, for its app.
struct TypeChecker<'a> {
    host: &'a Arc<AppHost>,
    uid: i32,
    package: &'a str,
}

impl fgs_types::Checker for TypeChecker<'_> {
    fn change_enabled(&self, change: i64) -> Result<bool, Exception> {
        self.host.change_enabled(change, self.uid)
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        self.host.flag(name)
    }

    fn regular(&self, permission: &str, allow_while_in_use: bool) -> Result<bool, Exception> {
        super::permission_checker::fgs_type_permission(self.host, permission, self.uid, self.package, allow_while_in_use)
    }

    /// The app host's ops answer a foreground mode as the uid's state
    /// decides it (each of its processes is top).
    fn app_op(&self, op: i32, _: bool) -> Result<bool, Exception> {
        Ok(appops::mode(self.host, op, self.uid, Some(self.package))? == MODE_ALLOWED)
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("activity", &format!("{what} (#1552)"))
    }
}

impl AppHost {
    /// Runs `job` on the activity manager's handler, after the jobs before
    /// it and outside the services' lock.
    fn post_job(&self, job: impl FnOnce(&Arc<AppHost>) + Send + 'static) {
        let mut handler = self.services.handler.lock().unwrap();
        let sender = handler.get_or_insert_with(|| {
            let (sender, jobs) = mpsc::channel::<Job>();
            let me = self.me.clone();
            std::thread::spawn(move || {
                for job in jobs {
                    let Some(host) = me.upgrade() else { return };
                    job(&host);
                }
            });
            sender
        });
        let _ = sender.send(Box::new(job));
    }

    /// Whether an aconfig flag of the image is on.
    pub(super) fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    /// `tempAllowlistUidLocked` for foreground services: `uid` may start
    /// them for `duration` (or longer, if it already may), for `reason`.
    fn temp_allowlist(state: &mut State, uid: i32, duration: Duration, reason: i32) {
        let until = Instant::now() + duration;
        let entry = state.fgs_temp_allowlist.entry(uid).or_insert((until, reason));
        if entry.0 < until {
            *entry = (until, reason);
        }
    }

    /// `tempAllowlistForPendingIntent`, of a PendingIntent sent with
    /// options that allow its creator `uid` foreground services (an
    /// alarm's).
    pub(super) fn temp_allowlist_for_fgs(&self, uid: i32, duration: Duration, reason: i32) {
        Self::temp_allowlist(&mut self.services.state.lock().unwrap(), uid, duration, reason);
    }

    /// `isAllowlistedForFgsStartLOSP`: why `uid` may start foreground
    /// services now, if it is temporarily allowed.
    fn temp_allowlisted(state: &State, uid: i32) -> Option<i32> {
        state.fgs_temp_allowlist.get(&uid).filter(|(until, _)| Instant::now() < *until).map(|(_, reason)| *reason)
    }

    /// `isBgFgsRestrictionEnabled`: whether a service of `service_uid`,
    /// started or made foreground by `calling_uid`, may not become
    /// foreground from the background (both target S or later).
    fn bg_fgs_restricted(&self, service_uid: i32, calling_uid: i32) -> Result<bool, Exception> {
        if !self.change_enabled(FGS_BG_START_RESTRICTION_CHANGE_ID, service_uid)? {
            return Ok(false);
        }
        Ok(calling_uid == SYSTEM_UID || self.change_enabled(FGS_BG_START_RESTRICTION_CHANGE_ID, calling_uid)?)
    }

    /// A device or profile owner's exemption (`isDeviceOwner`,
    /// `isProfileOwner`), which the app host does not answer, when it
    /// would decide.
    fn owner_exemption(&self) -> Exception {
        self.unimplemented_named("activity", "a device or profile owner's foreground service exemption (#1553)")
    }

    /// `shouldAllowFgsWhileInUsePermissionLocked` for `calling_uid`. A uid
    /// with a process is top in the app host, and so visible and within
    /// its activities' grace period. No while-in-use temporary allowlist
    /// (MediaSession's, telephony's) and no instrumentation runs, and the
    /// allowlisted packages are the attention and system captions
    /// services', which the app host does not run.
    fn fgs_while_in_use_reason(&self, calling_uid: i32) -> Result<i32, Exception> {
        if self.uid_running(calling_uid) {
            return Ok(REASON_PROC_STATE_TOP);
        }
        // Root, the system, NFC and the shell.
        if matches!(calling_uid.rem_euclid(PER_USER_RANGE as i32), 0 | 1000 | 1027 | 2000) {
            return Ok(REASON_SYSTEM_UID);
        }
        if self.check_permission(Some("android.permission.START_ACTIVITIES_FROM_BACKGROUND"), calling_uid)? == 0 {
            return Ok(REASON_BACKGROUND_ACTIVITY_PERMISSION);
        }
        // A device owner's comes last (`set_fgs_restriction`).
        Ok(REASON_DENIED)
    }

    /// `shouldAllowFgsStartForegroundWithBindingCheckLocked` for
    /// `calling_uid` of `calling_package`, after its while-in-use reason
    /// `wiu`, `allowlisted` if it is temporarily allowed. Past the first
    /// check the uid has no process, so none of its processes is top or
    /// bound by one (`canBindingClientStartFgsLocked`) or shows an overlay;
    /// no companion device is associated (no CompanionDeviceManager runs),
    /// and `mFgsAllowOptOut` is off.
    fn fgs_start_reason(&self, wiu: i32, calling_package: Option<&str>, calling_uid: i32, allowlisted: Option<i32>) -> Result<i32, Exception> {
        if wiu != REASON_DENIED {
            return Ok(wiu);
        }
        let held = |p: &str| self.check_permission(Some(p), calling_uid).map(|r| r == 0);
        if held("android.permission.START_FOREGROUND_SERVICES_FROM_BACKGROUND")? {
            return Ok(REASON_BACKGROUND_FGS_PERMISSION);
        }
        // `hasSystemAlertWindowPermission`; from V an overlay must show.
        if let Some(package) = calling_package {
            let saw = match appops::note(self, appops::OP_SYSTEM_ALERT_WINDOW, calling_uid, Some(package))? {
                MODE_DEFAULT => held("android.permission.SYSTEM_ALERT_WINDOW")?,
                mode => mode == MODE_ALLOWED,
            };
            if saw && !(self.flag(FGS_DISABLE_SAW)? && self.change_enabled(FGS_SAW_RESTRICTIONS, calling_uid)?) {
                return Ok(REASON_SYSTEM_ALERT_WINDOW_PERMISSION);
            }
        }
        if let Some(reason) = allowlisted {
            return Ok(reason);
        }
        let settings = &self.inputs()?.settings;
        if settings.get(super::settings::Table::Global, "device_demo_mode").and_then(|v| v.parse::<i32>().ok()).is_some_and(|v| v > 0) {
            return Ok(REASON_DEVICE_DEMO_MODE);
        }
        // A profile owner's comes last (`set_fgs_restriction`).
        let Some(package) = calling_package else { return Ok(REASON_DENIED) };
        let allowed = |op: i32| appops::mode(self, op, calling_uid, Some(package)).map(|m| m == MODE_ALLOWED);
        if allowed(appops::OP_SYSTEM_EXEMPT_FROM_POWER_RESTRICTIONS)? {
            return Ok(REASON_SYSTEM_EXEMPT_APP_OP);
        }
        if allowed(appops::OP_ACTIVATE_VPN)? {
            return Ok(REASON_OP_ACTIVATE_VPN);
        }
        if allowed(appops::OP_ACTIVATE_PLATFORM_VPN)? {
            return Ok(REASON_OP_ACTIVATE_PLATFORM_VPN);
        }
        let input_method = settings.get(super::settings::Table::Secure, "default_input_method");
        if input_method.as_deref().and_then(|m| m.split_once('/')).is_some_and(|(p, _)| p == package) {
            return Ok(REASON_CURRENT_INPUT_METHOD);
        }
        Ok(REASON_DENIED)
    }

    /// `setFgsRestrictionLocked` of a start (not a binding) by
    /// `calling_uid` of `calling_package`: the reasons the service has not
    /// yet computed, kept as its start's, or its bindings' for a bound
    /// service made foreground (`for_bound`). The bindings' own check
    /// (`shouldAllowFgsWhileInUsePermissionByBindingsLocked`) looks at the
    /// caller's processes, which are top here, so it allows nothing the
    /// start did not. Where nothing else allowed and the device has an
    /// owner, whose exemption is not answered, that is noted, for when it
    /// decides.
    fn set_fgs_restriction(
        &self,
        state: &mut State,
        token: Binder,
        calling_package: Option<&str>,
        calling_uid: i32,
        for_bound: bool,
    ) -> Result<(), Exception> {
        let allowlisted = Self::temp_allowlisted(state, calling_uid);
        let fg = &state.record(token).unwrap().fg;
        let (mut wiu, mut start) = (fg.allow_wiu.0, fg.allow_start.0);
        if wiu == REASON_DENIED || start == REASON_DENIED {
            let w = self.fgs_while_in_use_reason(calling_uid)?;
            let s = self.fgs_start_reason(w, calling_package, calling_uid, allowlisted)?;
            (wiu, start) = (reason_or(wiu, w), reason_or(start, s));
        }
        let owner_unknown = (wiu == REASON_DENIED || start == REASON_DENIED) && self.device_policies()?.owned();
        let fg = &mut state.record(token).unwrap().fg;
        fg.owner_unknown = owner_unknown;
        if for_bound {
            fg.allow_wiu.1 = reason_or(fg.allow_wiu.1, wiu);
            fg.allow_start.1 = reason_or(fg.allow_start.1, start);
        } else {
            (fg.allow_wiu.0, fg.allow_start.0) = (wiu, start);
        }
        fg.allow_start.1 = reason_or(fg.allow_start.1, fg.allow_wiu.1);
        Ok(())
    }

    /// `setServiceForegroundLocked`: the service of `name` and `token` (in
    /// the caller's user) made foreground with notification `id`
    /// (`startForeground`), or no longer (`stopForeground`, `id` 0, with
    /// `flags`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn set_service_foreground(
        self: &Arc<Self>,
        calling_uid: i32,
        name: Option<Name>,
        token: Option<Binder>,
        id: i32,
        notification: Option<Incoming>,
        flags: i32,
        service_type: i32,
    ) -> Result<(), Exception> {
        let mut state = self.services.state.lock().unwrap();
        let user = calling_uid / PER_USER_RANGE as i32;
        let found = token.and_then(|t| state.record(t)).filter(|r| {
            name.as_ref().is_some_and(|n| n.package == r.package && n.class == r.class)
                && r.info.info.application_info.uid / PER_USER_RANGE as i32 == user
        });
        let Some(token) = found.map(|r| r.token) else {
            return Ok(());
        };
        if id != 0 {
            let notification = notification.ok_or_else(|| Exception::illegal_argument("null notification"))?;
            self.start_foreground(&mut state, token, id, notification, service_type, calling_uid)
        } else {
            self.stop_foreground(&mut state, token, flags);
            Ok(())
        }
    }

    /// `getForegroundServiceTypeLocked`.
    pub(super) fn foreground_service_type(&self, calling_uid: i32, name: Option<Name>, token: Option<Binder>) -> i32 {
        let mut state = self.services.state.lock().unwrap();
        let user = calling_uid / PER_USER_RANGE as i32;
        token
            .and_then(|t| state.record(t))
            .filter(|r| {
                name.as_ref().is_some_and(|n| n.package == r.package && n.class == r.class)
                    && r.info.info.application_info.uid / PER_USER_RANGE as i32 == user
            })
            .map_or(fgs_types::TYPE_NONE, |r| r.fg.service_type)
    }

    /// `setServiceForegroundInnerLocked` with an id: its checks, then the
    /// service foreground and its notification posted.
    fn start_foreground(
        self: &Arc<Self>,
        state: &mut State,
        token: Binder,
        id: i32,
        notification: Incoming,
        requested_type: i32,
        calling_uid: i32,
    ) -> Result<(), Exception> {
        let r = state.record(token).unwrap();
        let app_info = &r.info.info.application_info;
        let (uid, target_sdk, package, class) = (app_info.uid, app_info.target_sdk_version, r.package.clone(), r.class.clone());
        let instant = app_info.private_flags & PRIVATE_FLAG_INSTANT != 0;
        let Some(seq) = r.app else {
            return Err(Exception::new(EX_ILLEGAL_STATE, format!("startForeground of {package}/{class}, which does not run")));
        };
        let (pid, process) = self.apps.lock().unwrap().get(&seq).map(|a| (a.pid, a.process.clone())).unwrap_or_default();
        let short = short_name(&package, &class);
        if instant {
            return Err(self.unimplemented_named("activity", "an instant app's foreground service (#1552)"));
        }
        if target_sdk >= P && self.check_permission(Some("android.permission.FOREGROUND_SERVICE"), uid)? != 0 {
            return Err(Exception::security(format!(
                "Permission Denial: startForeground from pid={pid}, uid={uid} requires android.permission.FOREGROUND_SERVICE"
            )));
        }
        if !notification.has_small_icon() {
            // `postNotification` makes one naming the app.
            return Err(self.unimplemented_named("activity", "a foreground service's notification without an icon (#1552)"));
        }
        let manifest_type = r.info.foreground_service_type;
        let skip_check = super::system_boolean(self.property("debug.skip_fgs_manifest_type_check").as_deref(), false);
        let mut service_type = match effective_type(requested_type, manifest_type) {
            Err(_) if skip_check => requested_type,
            result => result?,
        };
        if service_type == fgs_types::TYPE_MANIFEST {
            service_type = manifest_type;
        }
        // It called `startForeground` as required.
        if r.fg.required {
            r.fg.required = false;
            r.fg.waiting = None;
        }
        let mut ignore = false;
        match appops::mode(self, appops::OP_START_FOREGROUND, uid, Some(&package))? {
            MODE_ALLOWED | MODE_DEFAULT => {}
            appops::MODE_IGNORED => {
                eprintln!("app host: Service.startForeground() not allowed due to app op: service {short}");
                ignore = true;
            }
            _ => return Err(Exception::security("Foreground not allowed as per app op")),
        }
        // An app restricted in the background may still make a service
        // foreground while top (`isForegroundServiceAllowedInBackgroundRestricted`),
        // which each process here is.
        let restricted = self.bg_fgs_restricted(uid, calling_uid)?;
        if !ignore {
            if service_type == fgs_types::TYPE_SHORT_SERVICE {
                return Err(self.unimplemented_named("activity", "a short foreground service (#1551)"));
            }
            let r = state.record(token).unwrap();
            let elapsed = super::clock_ms(libc::CLOCK_MONOTONIC_RAW) - r.create_real_time;
            let (start_count, fg_required, start_requested) = (r.fg.start_count, r.fg.required, r.start_requested);
            let time_limited = service_type & (fgs_types::TYPE_DATA_SYNC | fgs_types::TYPE_MEDIA_PROCESSING) != 0
                && self.change_enabled(FGS_INTRODUCE_TIME_LIMITS, uid)?
                && self.flag(INTRODUCE_NEW_SERVICE_ONTIMEOUT_CALLBACK)?;
            // Whether its app may still start a foreground service is asked
            // again: for a time-limited type (whose limits are not run,
            // #1551), a first start long after its creation, and any later.
            let mut recalculated = true;
            if time_limited {
                r.fg.allow_start.0 = REASON_DENIED;
                r.fg.allow_start.1 = REASON_DENIED;
            } else if start_count == 0 {
                recalculated = !fg_required && elapsed > FGS_START_FOREGROUND_TIMEOUT_MS;
                if recalculated {
                    r.fg.reset_restriction();
                }
            }
            if recalculated {
                self.set_fgs_restriction(state, token, Some(&package), uid, false)?;
            } else if !start_requested {
                self.set_fgs_restriction(state, token, Some(&package), uid, true)?;
            }
            let r = state.record(token).unwrap();
            if !r.fg.allowed_start() && restricted {
                if r.fg.owner_unknown {
                    return Err(self.owner_exemption());
                }
                let message = format!("Service.startForeground() not allowed due to mAllowStartForeground false: service {short}");
                eprintln!("app host: {message}");
                ignore = true;
                if self.change_enabled(FGS_START_EXCEPTION_CHANGE_ID, uid)? {
                    return Err(parcelable_exception("android.app.ForegroundServiceStartNotAllowedException", message));
                }
            }
        }
        if !ignore {
            let fg = &state.record(token).unwrap().fg;
            let wiu = fg.allowed_wiu();
            if !wiu && fg.owner_unknown {
                return Err(self.owner_exemption());
            }
            self.validate_types(service_type, requested_type, wiu, uid, pid, &process, &package, target_sdk)?;
        }
        if ignore {
            return Ok(());
        }
        let r = state.record(token).unwrap();
        if r.fg.id != id
            && let Some(shown) = r.fgs_notification()
        {
            self.cancel_foreground_notification(state, token, &shown);
        }
        let r = state.record(token).unwrap();
        r.fg.id = id;
        r.fg.notification = Some(notification.foreground_service());
        if r.fg.is_foreground && r.fg.service_type != service_type {
            eprintln!("app host: FGS type change for {short} from {} to {service_type}", r.fg.service_type);
        }
        r.fg.service_type = service_type;
        if !r.fg.is_foreground {
            r.fg.is_foreground = true;
            r.fg.start_count += 1;
        }
        self.post_foreground_notification(r);
        Ok(())
    }

    /// `validateForegroundServiceType` of each of `service_type`'s types
    /// (from the highest), an unknown one taken as special use when that
    /// is among them, for the app of `uid`, allowed while-in-use
    /// permissions or not (`wiu`).
    #[allow(clippy::too_many_arguments)]
    fn validate_types(
        self: &Arc<Self>,
        service_type: i32,
        requested_type: i32,
        wiu: bool,
        uid: i32,
        pid: i32,
        process: &str,
        package: &str,
        target_sdk: i32,
    ) -> Result<(), Exception> {
        let checker = TypeChecker { host: self, uid, package };
        let default_to = if service_type & fgs_types::TYPE_SPECIAL_USE != 0 { fgs_types::TYPE_SPECIAL_USE } else { fgs_types::TYPE_NONE };
        let types: Vec<i32> = if service_type == fgs_types::TYPE_NONE {
            vec![fgs_types::TYPE_NONE]
        } else {
            (0..32).rev().map(|bit| 1i32 << bit).filter(|t| service_type & t != 0).collect()
        };
        let app = process_record(pid, process, uid);
        for ty in types {
            let policy = fgs_types::policy_of(ty, default_to)?;
            let label = fgs_types::label(ty);
            let foreground_only = if policy.foreground_only {
                " and the app must be in the eligible state/exemptions to access the foreground only permission"
            } else {
                ""
            };
            match fgs_types::check(policy, &checker, wiu)? {
                Check::Ok => {}
                Check::Deprecated => eprintln!("app host: Starting FGS with type {label} callerApp={app} targetSDK={target_sdk}"),
                Check::PermissionDeniedPermissive => eprintln!(
                    "app host: Starting FGS with type {label} callerApp={app} targetSDK={target_sdk} requiredPermissions={}{foreground_only}",
                    policy.permission_string()
                ),
                Check::Disabled if requested_type == fgs_types::TYPE_MANIFEST && ty == fgs_types::TYPE_NONE => {
                    return Err(parcelable_exception(
                        "android.app.MissingForegroundServiceTypeException",
                        format!("Starting FGS without a type  callerApp={app} targetSDK={target_sdk}"),
                    ));
                }
                Check::Disabled => {
                    return Err(parcelable_exception(
                        "android.app.InvalidForegroundServiceTypeException",
                        format!("Starting FGS with type {label} callerApp={app} targetSDK={target_sdk} has been prohibited"),
                    ));
                }
                Check::PermissionDeniedEnforced => {
                    return Err(Exception::security(format!(
                        "Starting FGS with type {label} callerApp={app} targetSDK={target_sdk} requires permissions: {}{foreground_only}",
                        policy.permission_string()
                    )));
                }
            }
        }
        Ok(())
    }

    /// `setServiceForegroundInnerLocked` without an id: a foreground
    /// service no longer is, its notification cancelled, unflagged or
    /// kept as `flags` say.
    fn stop_foreground(&self, state: &mut State, token: Binder, flags: i32) {
        let r = state.record(token).unwrap();
        if !r.fg.is_foreground {
            return;
        }
        let target_sdk = r.info.info.application_info.target_sdk_version;
        // Its notification as it was, which a deferred one is posted as.
        let shown = r.fgs_notification();
        let post = r.app.zip(r.fg.notification.clone());
        let stopped = r.fg.stop(flags, target_sdk);
        let Some(shown) = shown else { return };
        match stopped {
            Stopped::Cancel => self.cancel_foreground_notification(state, token, &shown),
            Stopped::Unflag { post: deferred } => {
                if let (true, Some((seq, notification))) = (deferred, post) {
                    self.post_notification(token, &shown, seq, notification);
                }
                // `dropFgsNotificationStateLocked`: unless another
                // foreground service of the app shows it.
                if !Self::shared(state, token, &shown) {
                    let FgsNotification { package, uid, id } = shown;
                    self.post_job(move |host| {
                        let Some(nms) = host.notification_service.get() else { return };
                        if let Err(e) = nms.remove_foreground_service_flag(&package, id, uid / PER_USER_RANGE as i32) {
                            eprintln!("app host: unflagging the notification of a service of {package}: {}", e.message);
                        }
                    });
                }
            }
            Stopped::Keep => {}
        }
    }

    /// Whether a foreground service of the app other than `token` shows
    /// the notification `n`.
    fn shared(state: &State, token: Binder, n: &FgsNotification) -> bool {
        state
            .records
            .iter()
            .any(|o| o.token != token && o.fg.is_foreground && o.fg.id == n.id && o.package == n.package)
    }

    /// `cancelForegroundNotificationLocked` of the notification `n` of the
    /// service `token`: cancelled as the system (`cancelNotification`),
    /// unless another foreground service of the app shows it.
    fn cancel_foreground_notification(&self, state: &State, token: Binder, n: &FgsNotification) {
        if Self::shared(state, token, n) {
            return;
        }
        let FgsNotification { package, uid, id } = n.clone();
        self.post_job(move |host| {
            let Some(nms) = host.notification_service.get() else { return };
            if let Err(e) = nms.cancel_foreground_service_notification(&package, id, uid / PER_USER_RANGE as i32) {
                eprintln!("app host: canceling the notification of a service of {package}: {}", e.message);
            }
        });
    }

    /// `ServiceRecord.postNotification` of a foreground service `r`.
    fn post_foreground_notification(&self, r: &Record) {
        if let (true, Some(shown), Some(seq), Some(notification)) =
            (r.fg.is_foreground, r.fgs_notification(), r.app, r.fg.notification.clone())
        {
            self.post_notification(r.token, &shown, seq, notification);
        }
    }

    /// `ServiceRecord.postNotification`'s post, on the handler, of the
    /// service `token` in the process of `seq`: as its app's, which must
    /// have its channel; one that cannot be posted stops the service and
    /// crashes its app (`killMisbehavingService`).
    fn post_notification(&self, token: Binder, n: &FgsNotification, seq: i64, notification: Incoming) {
        let pid = self.apps.lock().unwrap().get(&seq).map_or(0, |a| a.pid);
        let FgsNotification { package, uid, id } = n.clone();
        self.post_job(move |host| {
            let Some(nms) = host.notification_service.get() else { return };
            if !notification.has_small_icon() {
                // The original posts one naming the app.
                eprintln!("app host: {}", host.unimplemented_named("activity", "a foreground service's notification without an icon (#1552)").message);
                return;
            }
            let posted = (|| {
                let target_sdk = host.packages()?.0.packages.get(&package).map_or(O_MR1, |p| p.target_sdk_version);
                if !nms.has_channel(&package, uid, notification.channel_id())? && target_sdk >= O_MR1 {
                    return Err(Exception::new(EX_ILLEGAL_STATE, "invalid channel for service notification"));
                }
                nms.enqueue_internal(Enqueue {
                    package: package.clone(),
                    op_package: package.clone(),
                    calling_uid: uid,
                    pid,
                    tag: None,
                    id,
                    notification,
                    incoming_user: uid / PER_USER_RANGE as i32,
                })
            })();
            if let Err(e) = posted {
                eprintln!("app host: error showing notification for service of {package}: {}", e.message);
                host.kill_misbehaving_service(token, seq);
            }
        });
    }

    /// `killMisbehavingService`: the service `token` stopped, and the app
    /// of `seq` crashed for its notification.
    fn kill_misbehaving_service(&self, token: Binder, seq: i64) {
        {
            let mut state = self.services.state.lock().unwrap();
            if let Some(r) = state.record(token) {
                r.start_requested = false;
                self.bring_down_if_needed(&mut state, token);
            }
        }
        self.crash_application(seq, "Bad notification for startForeground", CANNOT_POST_FOREGROUND_SERVICE_NOTIFICATION, Vec::new());
    }

    /// `AppErrors.scheduleAppCrashLocked` of the process of `seq`: it is
    /// asked to crash (`scheduleCrash`) with `message`, its
    /// `RemoteServiceException`'s type and `extras`.
    fn crash_application(&self, seq: i64, message: &str, type_id: i32, extras: Vec<(String, Value)>) {
        let mut p = Parcel::new();
        thread::ScheduleCrash {
            msg: Some(message.to_string()),
            type_id,
            extras: Some(super::bind::Bundle(extras)),
        }
        .write(&mut p);
        if let Err(e) = self.call_thread(seq, thread::SCHEDULE_CRASH, &p) {
            eprintln!("app host: crashing the process of start {seq}: {e}");
        }
    }

    /// `serviceForegroundTimeout`: a service started with
    /// `startForegroundService` that did not call `startForeground` (its
    /// wait `generation`) is stopped, which crashes its app.
    fn service_foreground_timeout(&self, token: Binder, generation: u64) {
        let mut state = self.services.state.lock().unwrap();
        let Some(r) = state.record(token).filter(|r| r.fg.required && r.fg.waiting == Some(generation)) else {
            return;
        };
        eprintln!("app host: service foreground-required timeout for {}", short_name(&r.package, &r.class));
        r.fg.waiting = None;
        r.start_requested = false;
        self.bring_down_if_needed(&mut state, token);
        // The ANR that follows is not raised (#1551).
    }

    /// `applyForegroundServiceNotificationLocked` of `notification`, posted
    /// with `tag` and `id` by `package` in `user`: a foreground service's
    /// keeps it, flagged, as its notification, shown now or deferred.
    pub(super) fn apply_foreground_service_notification(
        &self,
        notification: &Incoming,
        tag: Option<&str>,
        id: i32,
        package: &str,
        user: i32,
    ) -> FgsPolicy {
        if tag.is_some() {
            return FgsPolicy::NotForegroundService;
        }
        let mut state = self.services.state.lock().unwrap();
        let found = state.records.iter().find(|r| {
            r.fg.is_foreground
                && r.fg.id == id
                && r.package == package
                && r.info.info.application_info.uid / PER_USER_RANGE as i32 == user
        });
        let Some((token, uid)) = found.map(|r| (r.token, r.info.info.application_info.uid)) else {
            return FgsPolicy::NotForegroundService;
        };
        let now = Instant::now();
        let eligible = state.deferral_eligible.get(&uid).copied();
        let r = state.record(token).unwrap();
        r.fg.notification = Some(notification.foreground_service());
        if r.fg.show_now(now, eligible, notification.force_deferred(), notification.show_foreground_immediately()) {
            r.fg.deferred = false;
            return FgsPolicy::ShowImmediately;
        }
        self.start_fgs_deferral_timer(&mut state, token, now);
        FgsPolicy::UpdateOnly
    }

    /// `startFgsDeferralTimerLocked`: the notification of `token` shown
    /// after the deferral, or with another of its app's deferred before;
    /// the app's are then not deferred for a while.
    fn start_fgs_deferral_timer(&self, state: &mut State, token: Binder, now: Instant) {
        let r = state.records.iter().find(|r| r.token == token).unwrap();
        if r.fg.pending {
            return;
        }
        let uid = r.info.info.application_info.uid;
        let when = state
            .records
            .iter()
            .filter(|o| o.fg.pending && o.info.info.application_info.uid == uid)
            .filter_map(|o| o.fg.display_time)
            .fold(now + FGS_NOTIFICATION_DEFERRAL_INTERVAL, Instant::min);
        state.deferral_eligible.insert(uid, when + FGS_NOTIFICATION_DEFERRAL_EXCLUSION_TIME);
        let r = state.record(token).unwrap();
        r.fg.display_time = Some(when);
        r.fg.deferred = true;
        r.fg.shown = false;
        r.fg.pending = true;
        let me = self.me.clone();
        std::thread::spawn(move || {
            std::thread::sleep(when.saturating_duration_since(Instant::now()));
            if let Some(host) = me.upgrade() {
                host.post_deferred_fgs_notifications();
            }
        });
    }

    /// `mPostDeferredFGSNotifications`: the deferred notifications whose
    /// time came, of services still foreground, posted.
    fn post_deferred_fgs_notifications(&self) {
        let now = Instant::now();
        let mut state = self.services.state.lock().unwrap();
        let due: Vec<Binder> = state
            .records
            .iter()
            .filter(|r| r.fg.pending && r.fg.display_time.is_some_and(|t| t <= now))
            .map(|r| r.token)
            .collect();
        for token in due {
            let r = state.record(token).unwrap();
            r.fg.pending = false;
            if r.fg.is_foreground && r.app.is_some() {
                self.post_foreground_notification(r);
                r.fg.shown = true;
            }
        }
    }

    /// `onForegroundServiceNotificationUpdateLocked`: a notification of
    /// `package` with `id` in `user` was posted (`shown`) or kept back; a
    /// deferred one shown is no longer pending.
    pub(super) fn foreground_service_notification_updated(&self, shown: bool, id: i32, package: &str, user: i32) {
        if !shown {
            return;
        }
        let mut state = self.services.state.lock().unwrap();
        for r in state.records.iter_mut().filter(|r| {
            r.fg.pending && r.fg.id == id && r.package == package && r.info.info.application_info.uid / PER_USER_RANGE as i32 == user
        }) {
            r.fg.shown = true;
            r.fg.deferred = false;
            r.fg.pending = false;
        }
    }
}

/// The start id after `last`: from 1, wrapping past `i32::MAX` to 1.
fn next_id(last: i32) -> i32 {
    last.wrapping_add(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `scheduleServiceRestartLocked`'s delays: one second, then four
    /// times the last while it keeps dying soon, one second again once it
    /// ran a minute, half an hour per crash beyond the first.
    #[test]
    fn restarts_back_off_as_the_original() {
        let now = Instant::now();
        let (min, reset) = (SERVICE_RESTART_DURATION, SERVICE_RESET_RUN_DURATION);
        let soon = Some(now - Duration::from_secs(2));
        assert_eq!(back_off(0, Duration::ZERO, 0, None, min, reset, now), (1, min));
        assert_eq!(back_off(1, min, 0, soon, min, reset, now), (1, min * 4));
        assert_eq!(back_off(1, min * 4, 0, soon, min, reset, now), (1, min * 16));
        assert_eq!(back_off(3, min * 16, 0, Some(now - reset * 2), min, reset, now), (1, min));
        assert_eq!(back_off(1, min, 3, soon, min, reset, now), (1, BOUND_SERVICE_CRASH_RESTART_DURATION * 2));
        // A service that keeps crashing soon after its start backs off
        // without overflowing.
        assert_eq!(back_off(30, Duration::MAX / 2, 1, soon, min, reset, now), (30, Duration::MAX));
    }

    /// `RunningServiceInfo.writeToParcel`.
    #[test]
    fn running_service_info_is_written_as_the_original_writes_it() {
        let info = RunningServiceInfo {
            package: "p".into(),
            class: "p.S".into(),
            pid: 42,
            uid: 10100,
            process: "p:remote".into(),
            foreground: true,
            active_since: 5,
            started: true,
            client_count: 2,
            crash_count: 1,
            last_activity_time: 6,
            restarting: 7,
            flags: FLAG_STARTED,
            client_package: Some("android".into()),
            client_label: 0x0104_0000,
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!((r.read_string16().unwrap(), r.read_string16().unwrap()), (Some("p".into()), Some("p.S".into())));
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (42, 10100));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p:remote"));
        assert_eq!(r.read_i32().unwrap(), 1); // foreground
        assert_eq!(r.read_i64().unwrap(), 5);
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()], [1, 2, 1]);
        assert_eq!([r.read_i64().unwrap(), r.read_i64().unwrap()], [6, 7]);
        assert_eq!(r.read_i32().unwrap(), FLAG_STARTED);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android"));
        assert_eq!(r.read_i32().unwrap(), 0x0104_0000);
        assert_eq!(r.remaining(), 0);
    }

    /// `getRunningServiceInfoLocked`: the caller's uid's services, its
    /// user's when allowed, every running user's across users, in the
    /// map's order and then the restarting ones again, at most `max`.
    #[test]
    fn running_services_are_picked_as_the_original_picks_them() {
        let service = |class: &str, uid: i32, app_uid: Option<i32>, restarting: Option<i64>| Running {
            info: RunningServiceInfo { package: "p".into(), class: class.into(), uid, ..Default::default() },
            user: uid / 100000,
            app_uid,
            restarting,
        };
        // By hash, "p.B" comes before "p.C".
        let services = [
            service("p.C", 10100, Some(10100), None),
            service("p.B", 10101, Some(10101), None),
            service("p.R", 10100, None, Some(99)),
            service("p.U", 1010100, Some(1010100), None),
        ];
        let names = |list: Vec<RunningServiceInfo>| list.into_iter().map(|i| (i.class, i.restarting)).collect::<Vec<_>>();
        let n = |s: &str, at: i64| (s.to_string(), at);
        assert_eq!(names(pick(&services, 10, 10100, false, false)), [n("p.C", 0)]);
        assert_eq!(names(pick(&services, 10, 10100, true, false)), [n("p.B", 0), n("p.C", 0), n("p.R", 0), n("p.R", 99)]);
        assert_eq!(names(pick(&services, 10, 10100, false, true)), [n("p.B", 0), n("p.C", 0), n("p.R", 0), n("p.R", 99)]);
        assert_eq!(names(pick(&services, 2, 10100, true, false)), [n("p.B", 0), n("p.C", 0)]);
        assert!(pick(&services, -1, 10100, true, true).is_empty());
    }

    /// `setServiceForegroundInnerLocked`'s type: the manifest's for
    /// `FOREGROUND_SERVICE_TYPE_MANIFEST`, a subset of it, short-service
    /// dropped when combined.
    #[test]
    fn a_foreground_type_must_be_the_manifests() {
        use fgs_types::*;
        let manifest = TYPE_SPECIAL_USE | TYPE_SHORT_SERVICE | TYPE_DATA_SYNC;
        assert_eq!(effective_type(TYPE_MANIFEST, TYPE_SPECIAL_USE).unwrap(), TYPE_SPECIAL_USE);
        assert_eq!(effective_type(TYPE_DATA_SYNC, manifest).unwrap(), TYPE_DATA_SYNC);
        assert_eq!(effective_type(TYPE_NONE, TYPE_NONE).unwrap(), TYPE_NONE);
        assert_eq!(effective_type(TYPE_MANIFEST, manifest).unwrap(), TYPE_SPECIAL_USE | TYPE_DATA_SYNC);
        assert_eq!(effective_type(TYPE_SHORT_SERVICE, manifest).unwrap(), TYPE_SHORT_SERVICE);
        let e = effective_type(TYPE_MEDIA_PLAYBACK, TYPE_SPECIAL_USE).unwrap_err();
        assert_eq!(
            e.message,
            "foregroundServiceType 0x00000002 is not a subset of foregroundServiceType attribute 0x40000000 in service element of manifest file"
        );
    }

    /// `stopForeground`: removed, the notification is cancelled and
    /// forgotten; detached, unflagged and forgotten; else unflagged (and
    /// posted if still deferred), but kept as it is before L; the service
    /// no longer foreground, of no type, its restriction reset.
    #[test]
    fn stopping_foreground_cancels_detaches_or_keeps_the_notification() {
        let foreground = |shown: bool| Foreground {
            is_foreground: true,
            id: 7,
            service_type: fgs_types::TYPE_SPECIAL_USE,
            allow_start: (REASON_PROC_STATE_TOP, REASON_DENIED),
            allow_wiu: (REASON_PROC_STATE_TOP, REASON_DENIED),
            shown,
            ..Default::default()
        };
        let mut fg = foreground(true);
        assert_eq!(fg.stop(STOP_FOREGROUND_REMOVE, 34), Stopped::Cancel);
        assert_eq!((fg.is_foreground, fg.id, fg.service_type), (false, 0, fgs_types::TYPE_NONE));
        assert!(!fg.allowed_start() && !fg.allowed_wiu());
        let mut fg = foreground(false);
        assert_eq!(fg.stop(STOP_FOREGROUND_DETACH, 34), Stopped::Unflag { post: true });
        assert_eq!(fg.id, 0);
        let mut fg = foreground(true);
        assert_eq!(fg.stop(0, 34), Stopped::Unflag { post: false });
        assert_eq!(fg.id, 7);
        let mut fg = foreground(true);
        assert_eq!(fg.stop(STOP_FOREGROUND_DETACH, 19), Stopped::Keep);
        assert_eq!((fg.id, fg.is_foreground), (7, false));
    }

    /// `shouldShowFgsNotificationLocked`: shown at once if asked, for a
    /// media, call or navigation notification or one with actions, for an
    /// immediate type, when already shown, at the deferral's end, or while
    /// the app's are not deferred; else deferred, as when the app forces
    /// it.
    #[test]
    fn foreground_notifications_are_deferred_as_the_original_defers_them() {
        let now = Instant::now();
        let fg = Foreground { is_foreground: true, service_type: fgs_types::TYPE_SPECIAL_USE, ..Default::default() };
        assert!(!fg.show_now(now, None, false, false));
        assert!(fg.show_now(now, None, false, true));
        assert!(!fg.show_now(now, None, true, true));
        assert!(fg.show_now(now, Some(now + Duration::from_secs(1)), true, false));
        let media = Foreground { service_type: fgs_types::TYPE_MEDIA_PLAYBACK, ..Default::default() };
        assert!(media.show_now(now, None, false, false));
        let shown = Foreground { shown: true, ..Default::default() };
        assert!(shown.show_now(now, None, true, false));
        // Within its own deferral, the app's exclusion does not show it.
        let deferred = Foreground { deferred: true, display_time: Some(now + Duration::from_secs(5)), ..Default::default() };
        assert!(!deferred.show_now(now, Some(now + Duration::from_secs(60)), false, false));
        assert!(deferred.show_now(now + Duration::from_secs(5), None, false, false));
    }

    /// The reasons of a start and of its bindings: a start's first.
    #[test]
    fn a_service_may_start_foreground_for_its_start_or_its_bindings() {
        let mut fg = Foreground::default();
        assert!(!fg.allowed_start());
        fg.allow_start.1 = REASON_SYSTEM_UID;
        assert!(fg.allowed_start());
        assert_eq!(reason_or(REASON_PROC_STATE_TOP, REASON_SYSTEM_UID), REASON_PROC_STATE_TOP);
        assert_eq!(reason_or(REASON_DENIED, REASON_SYSTEM_UID), REASON_SYSTEM_UID);
    }

    #[test]
    fn names_are_written_as_the_original_writes_them() {
        assert_eq!(short_name("com.android.deskclock", "com.android.deskclock.timer.TimerService"), "com.android.deskclock/.timer.TimerService");
        assert_eq!(short_name("p", "q.S"), "p/q.S");
        assert_eq!(process_record(42, "p", 10_123), "ProcessRecord{42:p/u0a123}");
        assert_eq!(process_record(42, "system", 1000), "ProcessRecord{42:system/1000}");
        let e = parcelable_exception("android.app.ForegroundServiceStartNotAllowedException", "m".into());
        assert_eq!((e.code, e.message.as_str()), (aim_binder_host::parcel::EX_PARCELABLE, "m"));
    }

    #[test]
    fn start_ids_grow_and_skip_zero() {
        assert_eq!((next_id(0), next_id(1)), (1, 2));
        assert_eq!(next_id(i32::MAX), 1);
    }

    /// `ServiceStartArgs.writeToParcel`: not for a removed task, its id,
    /// flags, and its intent or none.
    #[test]
    fn start_args_are_written_as_the_original_writes_them() {
        let mut p = Parcel::new();
        StartArgs {
            id: 3,
            flags: START_FLAG_RETRY,
            intent: None,
        }
        .write_to(&mut p);
        let mut r = p.reader();
        let ints: Vec<i32> = (0..4).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(ints, [0, 3, START_FLAG_RETRY, 0]);
        assert_eq!(r.remaining(), 0);
    }
}
