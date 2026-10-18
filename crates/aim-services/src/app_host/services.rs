//! Apps' services in the app host, as `ActiveServices` runs them at
//! `android-16.0.0_r1`: `startService` and `stopService`, `bindService`
//! and `unbindService`, a service created in its app's process (started
//! for it when it does not run) with `scheduleCreateService`, its starts
//! delivered with `scheduleServiceArgs` and its bindings requested with
//! `scheduleBindService`, the binder it publishes passed to its
//! connections, and the service destroyed (`scheduleStopService`) once it
//! is neither started nor bound to with `BIND_AUTO_CREATE`. When its
//! process dies, its connections are told and a service still needed is
//! started again. The app host binds services itself too, as the system
//! does (`bindServiceAsUser` from system_server, a connection of its
//! own). A service runs in its process (`android:process`), started for
//! it when it does not run. Isolated services and foreground services are
//! not run (#1492). `getServices` lists them as
//! `getRunningServiceInfoLocked` does.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{
    Binder, EX_BAD_PARCELABLE, EX_ILLEGAL_STATE, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::activity_start::Component;
use super::broadcasts::filter_equals;
use super::kept::{By, KeptIntent, RawIntent};
use super::{AppHost, PROCESS_STATE_TOP, STOCK_PM_FLAGS};
use crate::package::info::{ServiceInfo, java_hash};
use crate::package::list_slice::Slice;

/// `Context.BIND_AUTO_CREATE`; `BIND_EXTERNAL_SERVICE` (both forms).
const BIND_AUTO_CREATE: i64 = 0x1;
const BIND_EXTERNAL_SERVICE: i64 = 0x8000_0000;
const BIND_EXTERNAL_SERVICE_LONG: i64 = 1 << 62;
/// `ServiceInfo.FLAG_ISOLATED_PROCESS`.
const FLAG_ISOLATED_PROCESS: i32 = 0x1;
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
    info: ServiceInfo,
    /// Its process's start, and whether it was created there.
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

#[derive(Default)]
pub struct Services {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    records: Vec<Record>,
    connections: Vec<Connection>,
    next_bind_seq: i64,
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
/// is foreground (#1492) or runs in the system's process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunningServiceInfo {
    package: String,
    class: String,
    pid: i32,
    uid: i32,
    process: String,
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

/// `RunningServiceInfo.FLAG_STARTED`, `FLAG_PERSISTENT_PROCESS`.
const FLAG_STARTED: i32 = 1 << 0;
const FLAG_PERSISTENT_PROCESS: i32 = 1 << 3;

impl WriteParcelable for RunningServiceInfo {
    fn write_to(&self, p: &mut Parcel) {
        Component(&self.package, &self.class).write_to(p);
        p.write_i32(self.pid);
        p.write_i32(self.uid);
        p.write_string16(Some(&self.process));
        p.write_i32(0); // not foreground
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
        (count, (delay * SERVICE_RESTART_DURATION_FACTOR).max(min))
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
                at + SERVICE_MIN_RESTART_TIME_BETWEEN > *t && at < *t + SERVICE_MIN_RESTART_TIME_BETWEEN
            });
            match near {
                Some(t) => at = t + SERVICE_MIN_RESTART_TIME_BETWEEN,
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
    /// `uid`, and whether the caller may reach it.
    fn retrieve_service(
        &self,
        intent: &RawIntent,
        resolved_type: Option<&str>,
        uid: i32,
        user: i32,
        instance: Option<&str>,
    ) -> Result<Lookup, Exception> {
        let (_, resolution) = self.packages()?;
        let found = resolution
            .resolve_service(&intent.intent, resolved_type, STOCK_PM_FLAGS, user, uid)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("resolveService: {e:?}")))?;
        let Some(crate::package::component_resolver::Info::Service(info)) = found.map(|r| r.info)
        else {
            return Ok(Lookup::None);
        };
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
        if info.flags & FLAG_ISOLATED_PROCESS != 0 {
            return Err(self.unimplemented_named("activity", "an isolated service (#1492)"));
        }
        Ok(Lookup::Found(Box::new(info)))
    }

    /// The record of `info`, made if it has none (`ServiceMap`).
    fn service_record(
        state: &mut State,
        info: ServiceInfo,
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
            .find(|r| r.package == package && r.class == class)
        {
            return r.token;
        }
        let token = process.add_service(Arc::new(super::activity::Token));
        state.records.push(Record {
            token,
            package,
            class,
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
        });
        token
    }

    /// `startServiceLocked`: the service's name, `!` and why if the
    /// caller may not start it, none if there is no such service.
    pub(super) fn start_service(
        &self,
        caller: Option<Binder>,
        uid: i32,
        intent: RawIntent,
        resolved_type: Option<String>,
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
            match self.retrieve_service(&intent, resolved_type.as_deref(), uid, user, None)? {
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
        let kept = KeptIntent::new(intent, &self.process);
        let mut state = self.services.state.lock().unwrap();
        let token = Self::service_record(&mut state, info, &self.process);
        let r = state.record(token).unwrap();
        r.start_requested = true;
        r.stop_if_killed = false;
        let id = r.next_start_id();
        r.pending.push(StartItem {
            id,
            intent: Some(kept),
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
            match self.retrieve_service(&intent, resolved_type.as_deref(), uid, user, None)? {
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
            .find(|r| r.package == package && r.class == class)
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
                r.delivered.drain(..=at);
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
        activity: Option<Binder>,
        intent: RawIntent,
        resolved_type: Option<String>,
        connection: Option<Binder>,
        flags: i64,
        instance: Option<String>,
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
        if flags & (BIND_EXTERNAL_SERVICE | BIND_EXTERNAL_SERVICE_LONG) != 0 {
            return Err(
                self.unimplemented_named("activity", "an external service's binding (#1492)")
            );
        }
        let user = self.incoming_user(uid, user, false)?;
        let info = match self.retrieve_service(
            &intent,
            resolved_type.as_deref(),
            uid,
            user,
            instance.as_deref(),
        )? {
            Lookup::None => return Ok(0),
            Lookup::Denied(_) => return Ok(-1),
            Lookup::Found(info) => *info,
        };
        let conn = Conn::App(self.process.strong(conn));
        self.bind_found(intent, info, conn, Some(client), activity, flags, client_label)?;
        Ok(1)
    }

    /// `bindServiceAsUser` of the system: the service `package`/`class`
    /// of `user` bound with `bind_flags` for `conn`, by an intent of
    /// `flags`; whether there is one to bind.
    pub(super) fn bind_service_from_host(
        &self,
        package: &str,
        class: &str,
        flags: i32,
        bind_flags: i64,
        user: i32,
        conn: Arc<dyn HostConnection>,
    ) -> Result<bool, Exception> {
        let intent = RawIntent::of(None, flags, &[]).with_component(package, class);
        let info = match self.retrieve_service(&intent, None, 1000, user, None)? {
            Lookup::Found(info) => *info,
            Lookup::None | Lookup::Denied(_) => return Ok(false),
        };
        self.bind_found(intent, info, Conn::Host(conn), None, None, bind_flags, 0)?;
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
        conn: Conn,
        client: Option<i64>,
        activity: Option<u64>,
        flags: i64,
        client_label: i32,
    ) -> Result<(), Exception> {
        let kept = KeptIntent::new(intent, &self.process);
        let mut state = self.services.state.lock().unwrap();
        let token = Self::service_record(&mut state, info, &self.process);
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
        let process = super::providers::process_name(&r.info.info);
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
                let pid = self.start_process_for(&package, &process, None)?;
                let seq = self
                    .apps
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(_, a)| a.pid == pid)
                    .map(|(seq, _)| *seq);
                state.record(token).unwrap().app = seq;
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
        let compat = super::config::CompatibilityInfo::of(&r.info.info.application_info, &config);
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
        let Some(r) = state.record(token) else { return };
        let Some(app) = r.app.filter(|_| r.created) else {
            return;
        };
        let n = r.pending.len();
        let mut args = Vec::new();
        for mut s in std::mem::take(&mut r.pending) {
            if s.intent.is_none() && n > 1 {
                continue;
            }
            s.delivery_count += 1;
            s.delivered_time = Some(Instant::now());
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
        // Its connections learn it is gone.
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.connections)
            .into_iter()
            .partition(|c| c.service == token);
        state.connections = kept;
        for c in &gone {
            connected(&c.conn, &r.package, &r.class, None, true);
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
    /// over when the service started long enough ago. None of the app
    /// host's services is foreground, so none is tried again after its
    /// process crashed too often.
    pub(super) fn services_crashed(&self, start_seq: i64) {
        let now = Instant::now();
        let mut state = self.services.state.lock().unwrap();
        for r in state.records.iter_mut().filter(|r| r.app == Some(start_seq) && r.created) {
            if r.restart_time.is_none_or(|t| now > t + MIN_CRASH_INTERVAL) {
                r.crash_count = 1;
            } else {
                r.crash_count += 1;
            }
        }
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
            let delay = if !allow_restart {
                None
            } else if r.crash_count >= BOUND_SERVICE_MAX_CRASH_RETRY {
                eprintln!("app host: service {package}/{class} crashed {} times, stopping", r.crash_count);
                None
            } else {
                r.schedule_restart(auto, now)
            };
            for c in state.connections.iter().filter(|c| c.service == token) {
                connected(&c.conn, &package, &class, None, true);
            }
            match delay {
                Some(delay) => {
                    let at = state.stagger(token, now + delay);
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
        assert_eq!(r.read_i32().unwrap(), 0); // not foreground
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
