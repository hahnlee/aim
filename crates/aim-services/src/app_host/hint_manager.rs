//! `performance_hint`: HintManagerService for the app host
//! (`android-16.0.0_r1`), the client data every process's
//! `APerformanceHintManager` registers for (`registerClient`,
//! `getClientData`): the Power HAL's version, preferred rate and support
//! info, as the service reads them at its start; the FMQ session channel
//! as far as a HAL without a separate event flag goes (none: the service
//! closes the HAL's channel and falls back for good); and hint sessions
//! (`createHintSessionWithConfig`, `AppHintSession`): the caller's threads
//! checked as `checkTidValid` checks them (each `/proc/<tid>/status`, read
//! by the app host agent), a session of the HAL made
//! (`createHintSessionWithConfig`, else `createHintSession`) and called as
//! the JNI's `PowerHintSessionWrapper` calls it, closed when its token
//! dies or its uid is gone, the graphics pipeline threads of each uid
//! counted, and sessions associated to layers through the session manager
//! SurfaceFlinger passes. A uid of the app host is foreground (top) from
//! its first process until it is gone, so no session is paused
//! (`updateHintAllowedByProcState`), and the dead thread cleanup of a uid
//! leaving the foreground never runs. The threads' SCHED_RESET_ON_FORK
//! (#1830), the statsd atoms (#1829), a HAL's channel with an event flag,
//! headroom and a Power HAL before version 6 are not implemented (#1689).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_SERVICE_SPECIFIC,
    EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader, Result as ParcelResult,
    UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_os_ihintmanager as hm;
use aim_service_aidl::android_os_ihintsession as hs;
use aim_service_aidl::{ReadParcelable, WriteParcelable, read_int_array, write_int_array};

use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::query::Query;

/// The Power HAL (`IPower/default`), its descriptor and the calls the
/// service makes.
const POWER: &str = "android.hardware.power.IPower/default";
const POWER_DESCRIPTOR: &str = "android.hardware.power.IPower";
const CREATE_HINT_SESSION: u32 = 5;
const GET_HINT_SESSION_PREFERRED_RATE: u32 = 6;
const CREATE_HINT_SESSION_WITH_CONFIG: u32 = 7;
const GET_SESSION_CHANNEL: u32 = 8;
const CLOSE_SESSION_CHANNEL: u32 = 9;
const GET_SUPPORT_INFO: u32 = 10;
/// The HAL's `IPowerHintSession` and the calls `PowerHintSessionWrapper`
/// forwards, each with the interface version it needs (`FWD_CALL`).
const POWER_HINT_SESSION_DESCRIPTOR: &str = "android.hardware.power.IPowerHintSession";
const UPDATE_TARGET_WORK_DURATION: (u32, i32) = (1, 2);
const REPORT_ACTUAL_WORK_DURATION: (u32, i32) = (2, 2);
const CLOSE: (u32, i32) = (5, 2);
const SEND_HINT: (u32, i32) = (6, 4);
const SET_THREADS: (u32, i32) = (7, 4);
const SET_MODE: (u32, i32) = (8, 5);
/// SurfaceFlinger's `android.adpf.ISessionManager` and its one-way calls.
const SESSION_MANAGER_DESCRIPTOR: &str = "android.adpf.ISessionManager";
const ASSOCIATE_SESSION_TO_LAYERS: u32 = 1;
const TRACKED_SESSIONS_DIED: u32 = 2;
/// The aconfig flags the service reads: the FMQ channel's
/// (`Flags.adpfUseFmqChannel`), and `adpfSessionTag`,
/// `powerhintThreadCleanup` and `resetOnForkEnabled` of its own package.
const ADPF_USE_FMQ_CHANNEL: &str = "android.os.adpf_use_fmq_channel";
const ADPF_SESSION_TAG: &str = "com.android.server.power.hint.adpf_session_tag";
const POWERHINT_THREAD_CLEANUP: &str = "com.android.server.power.hint.powerhint_thread_cleanup";
const RESET_ON_FORK_ENABLED: &str = "com.android.server.power.hint.reset_on_fork_enabled";
/// A stable interface's `getInterfaceVersion`
/// (`IBinder.FIRST_CALL_TRANSACTION + 16777214`).
const GET_INTERFACE_VERSION: u32 = 1 + 16_777_214;
/// `MAX_GRAPHICS_PIPELINE_THREADS_COUNT`,
/// `DEFAULT_MAX_CPU_HEADROOM_THREADS_COUNT`.
const MAX_GRAPHICS_PIPELINE_THREADS: i32 = 5;
const MAX_CPU_HEADROOM_THREADS: i32 = 5;
/// `SessionMode.GRAPHICS_PIPELINE`, `AUTO_CPU`, `AUTO_GPU`;
/// `SessionTag.GAME`, `APP`; `ApplicationInfo.CATEGORY_GAME`;
/// `PackageManager.MATCH_ALL`; `Process.SYSTEM_UID`.
const GRAPHICS_PIPELINE: i32 = 1;
const AUTO_CPU: i32 = 2;
const AUTO_GPU: i32 = 3;
const TAG_GAME: i32 = 3;
const TAG_APP: i32 = 4;
const CATEGORY_GAME: i32 = 0;
const MATCH_ALL: i64 = 0x0002_0000;
const SYSTEM_UID: i32 = 1000;
/// The agent's `readProcStatus(int tid)` (`daemons/app-host-agent`).
const READ_PROC_STATUS: u32 = 9;

/// `IHintManager.HintManagerClientData`, with the HAL's `SupportInfo` as
/// it parcelled it (its words, its size first).
#[derive(Clone, Debug, PartialEq)]
struct ClientData {
    power_hal_version: i32,
    preferred_rate_nanos: i64,
    support_info: Vec<i32>,
}

impl WriteParcelable for ClientData {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.power_hal_version);
        p.write_i32(MAX_GRAPHICS_PIPELINE_THREADS);
        p.write_i32(MAX_CPU_HEADROOM_THREADS);
        p.write_i64(self.preferred_rate_nanos);
        p.write_i32(1);
        for &w in &self.support_info {
            p.write_i32(w);
        }
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// `SessionCreationConfig`, of which `readFromParcel` reads the fields
/// its size covers.
#[derive(Debug, Default, PartialEq)]
struct CreationConfig {
    tids: Option<Vec<i32>>,
    target_work_duration_nanos: i64,
    modes_to_enable: Option<Vec<i32>>,
    layer_tokens: Option<Vec<Option<Binder>>>,
}

impl ReadParcelable for CreationConfig {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let size = r.read_i32()?;
        if size < 4 {
            return Err(BAD_VALUE);
        }
        let end = start + size as usize;
        let mut config = CreationConfig::default();
        if r.position() < end {
            config.tids = read_int_array(r)?;
        }
        if r.position() < end {
            config.target_work_duration_nanos = r.read_i64()?;
        }
        if r.position() < end {
            config.modes_to_enable = read_int_array(r)?;
        }
        if r.position() < end {
            config.layer_tokens = read_binder_array(r)?;
        }
        r.set_position(end);
        Ok(config)
    }
}

/// `createBinderArray`: a count (-1 for null), then each binder.
fn read_binder_array(r: &mut Reader<'_>) -> ParcelResult<Option<Vec<Option<Binder>>>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Ok(None);
    }
    (0..n)
        .map(|_| r.read_binder())
        .collect::<ParcelResult<_>>()
        .map(Some)
}

fn write_binder_array(p: &mut Parcel, binders: &[Option<Binder>]) {
    p.write_i32(binders.len() as i32);
    for &b in binders {
        p.write_binder(b);
    }
}

/// A structured parcelable's size, then what `write` writes.
fn write_sized(p: &mut Parcel, write: impl FnOnce(&mut Parcel)) {
    let start = p.position();
    p.write_i32(0);
    write(p);
    p.set_i32_at(start, (p.position() - start) as i32);
}

/// `IHintManager.SessionCreationReturn`.
struct CreationReturn {
    session: Binder,
    pipeline_thread_limit_exceeded: bool,
}

impl WriteParcelable for CreationReturn {
    fn write_to(&self, p: &mut Parcel) {
        write_sized(p, |p| {
            p.write_binder(Some(self.session));
            p.write_bool(self.pipeline_thread_limit_exceeded);
        });
    }
}

/// The HAL's `SessionConfig`.
struct SessionConfig {
    id: i64,
}

impl WriteParcelable for SessionConfig {
    fn write_to(&self, p: &mut Parcel) {
        write_sized(p, |p| p.write_i64(self.id));
    }
}

/// The HAL's `WorkDuration`, as an app writes it and as the JNI hands it
/// on (without its intended present time).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct WorkDuration {
    time_stamp_nanos: i64,
    duration_nanos: i64,
    work_period_start_timestamp_nanos: i64,
    cpu_duration_nanos: i64,
    gpu_duration_nanos: i64,
}

impl ReadParcelable for WorkDuration {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let size = r.read_i32()?;
        if size < 4 {
            return Err(BAD_VALUE);
        }
        let end = start + size as usize;
        let mut fields = [0i64; 5];
        for f in &mut fields {
            if r.position() >= end {
                break;
            }
            *f = r.read_i64()?;
        }
        r.set_position(end);
        let [
            time_stamp_nanos,
            duration_nanos,
            work_period_start_timestamp_nanos,
            cpu_duration_nanos,
            gpu_duration_nanos,
        ] = fields;
        Ok(WorkDuration {
            time_stamp_nanos,
            duration_nanos,
            work_period_start_timestamp_nanos,
            cpu_duration_nanos,
            gpu_duration_nanos,
        })
    }
}

impl WriteParcelable for WorkDuration {
    fn write_to(&self, p: &mut Parcel) {
        write_sized(p, |p| {
            p.write_i64(self.time_stamp_nanos);
            p.write_i64(self.duration_nanos);
            p.write_i64(self.work_period_start_timestamp_nanos);
            p.write_i64(self.cpu_duration_nanos);
            p.write_i64(self.gpu_duration_nanos);
            p.write_i64(0); // intendedPresentTimestampNanos
        });
    }
}

/// Whether a HAL's `ChannelConfig` has an `eventFlagDescriptor`: past its
/// size and its `channelDescriptor` (present, then sized), whether the
/// nullable one is there.
fn has_event_flag(r: &mut Reader<'_>) -> ParcelResult<bool> {
    if r.read_i32()? == 0 {
        return Err(BAD_VALUE);
    }
    r.read_i32()?;
    if r.read_i32()? == 0 {
        return Err(BAD_VALUE);
    }
    let start = r.position();
    let size = r.read_i32()?;
    if size < 4 {
        return Err(BAD_VALUE);
    }
    r.set_position(start + size as usize);
    Ok(r.read_i32()? != 0)
}

/// A parcelable's words as `r` holds them: its size, then the rest.
fn parcelable_words(r: &mut Reader<'_>) -> ParcelResult<Vec<i32>> {
    let size = r.read_i32()?;
    if size < 4 || size % 4 != 0 {
        return Err(BAD_VALUE);
    }
    let mut words = vec![size];
    for _ in 1..size / 4 {
        words.push(r.read_i32()?);
    }
    Ok(words)
}

/// A call of the HAL: its reply's status, then what `read` reads.
fn hal_call<T>(
    hal: &Strong,
    code: u32,
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
) -> Result<Result<T, Exception>, Exception> {
    hal_call_with(hal, code, |_| {}, read)
}

fn hal_call_with<T>(
    hal: &Strong,
    code: u32,
    write: impl FnOnce(&mut Parcel),
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
) -> Result<Result<T, Exception>, Exception> {
    let failed = |s| {
        Exception::new(
            EX_ILLEGAL_STATE,
            format!("Could not contact PowerHAL! (#{code}: status {s})"),
        )
    };
    let mut data = Parcel::new();
    data.write_interface_token(POWER_DESCRIPTOR);
    write(&mut data);
    let reply = hal.transact(code, &data, false).map_err(failed)?;
    let mut r = reply.reader();
    Ok(match r.read_exception().map_err(failed)? {
        Ok(()) => Ok(read(&mut r).map_err(failed)?),
        Err(e) => Err(e),
    })
}

/// A HAL session creation's `HalResult`: unsupported for
/// EX_UNSUPPORTED_OPERATION or a call the HAL does not know, else failed
/// with its message.
enum Created {
    Session(HalSession, i64),
    Unsupported(String),
    Failed(String),
}

/// `createHintSessionWithConfig` (with the session's tag) or
/// `createHintSession` of the HAL: its `IPowerHintSession` and, with a
/// config, the config's id.
fn hal_create(
    hal: &Strong,
    tgid: i32,
    uid: i32,
    tids: &[i32],
    duration_nanos: i64,
    tag: Option<i32>,
) -> Created {
    let mut data = Parcel::new();
    data.write_interface_token(POWER_DESCRIPTOR);
    data.write_i32(tgid);
    data.write_i32(uid);
    write_int_array(&mut data, Some(tids));
    data.write_i64(duration_nanos);
    let code = match tag {
        Some(tag) => {
            data.write_i32(tag);
            CREATE_HINT_SESSION_WITH_CONFIG
        }
        None => CREATE_HINT_SESSION,
    };
    let reply = match hal.transact(code, &data, false) {
        Ok(reply) => reply,
        Err(UNKNOWN_TRANSACTION) => return Created::Unsupported("unknown transaction".into()),
        Err(s) => return Created::Failed(format!("status {s}")),
    };
    let mut r = reply.reader();
    let read = |r: &mut Reader<'_>| -> ParcelResult<(Option<Binder>, i64)> {
        let session = r.read_binder()?;
        let id = match tag {
            Some(_) if r.read_i32()? != 0 => {
                r.read_i32()?; // its size
                r.read_i64()?
            }
            _ => -1,
        };
        Ok((session, id))
    };
    match r.read_exception() {
        Ok(Ok(())) => match read(&mut r) {
            Ok((session, id)) => {
                let session = session.and_then(|b| reply.retain_remote_binder(b).ok());
                Created::Session(HalSession::new(session), id)
            }
            Err(s) => Created::Failed(format!("reply: status {s}")),
        },
        Ok(Err(e)) if e.code == EX_UNSUPPORTED_OPERATION => Created::Unsupported(e.message),
        Ok(Err(e)) => Created::Failed(e.message),
        Err(s) => Created::Failed(format!("reply: status {s}")),
    }
}

/// The JNI's `PowerHintSessionWrapper`: the HAL's session, if it gave
/// one, and its interface version, below which a call is not made.
struct HalSession {
    session: Option<Strong>,
    version: i32,
}

impl HalSession {
    fn new(session: Option<Strong>) -> Self {
        let version = session.as_ref().map_or(0, |s| {
            let mut data = Parcel::new();
            data.write_interface_token(POWER_HINT_SESSION_DESCRIPTOR);
            s.transact(GET_INTERFACE_VERSION, &data, false)
                .ok()
                .and_then(|reply| {
                    let mut r = reply.reader();
                    match r.read_exception() {
                        Ok(Ok(())) => r.read_i32().ok(),
                        _ => None,
                    }
                })
                .unwrap_or(0)
        });
        HalSession { session, version }
    }

    /// A call `(code, version)` of the session, one-way but `setThreads`;
    /// its result is not looked at, as the JNI does not.
    fn call(&self, (code, version): (u32, i32), write: impl FnOnce(&mut Parcel)) {
        let Some(session) = self.session.as_ref().filter(|_| self.version >= version) else {
            return;
        };
        let mut data = Parcel::new();
        data.write_interface_token(POWER_HINT_SESSION_DESCRIPTOR);
        write(&mut data);
        if let Err(s) = session.transact(code, &data, code != SET_THREADS.0) {
            eprintln!("app host: performance_hint: hint session call {code}: status {s}");
        }
    }
}

/// `Process.readProcLines` over `text`: each key's number, 0 for a key
/// not found.
fn read_proc_lines(text: &[u8], keys: &[&str]) -> Vec<i64> {
    let mut out = vec![0; keys.len()];
    let mut found = 0;
    let mut p = 0;
    let at = |i: usize| text.get(i).copied().unwrap_or(0);
    while at(p) != 0 && found < keys.len() {
        let mut skip_to_eol = true;
        for (i, key) in keys.iter().enumerate() {
            if !text[p..].starts_with(key.as_bytes()) {
                continue;
            }
            p += key.len();
            while at(p) == b' ' || at(p) == b'\t' {
                p += 1;
            }
            let num = p;
            while at(p).is_ascii_digit() {
                p += 1;
            }
            skip_to_eol = at(p) != b'\n';
            out[i] = std::str::from_utf8(&text[num..p])
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            if at(p) != 0 {
                p += 1;
            }
            found += 1;
            break;
        }
        if skip_to_eol {
            while at(p) != 0 && at(p) != b'\n' {
                p += 1;
            }
            if at(p) == b'\n' {
                p += 1;
            }
        }
    }
    out
}

/// `checkTidValid`: the first of `tids` that is neither a thread of the
/// caller's process (`tgid`, with `check_tgid`) nor of its `uid` nor of
/// an isolated process of it; `status` reads a thread's uid and process.
fn invalid_tid(
    uid: i32,
    tgid: i32,
    tids: &[i32],
    check_tgid: bool,
    mut status: impl FnMut(i32) -> Result<(i32, i32), Exception>,
    isolated: impl FnOnce() -> Option<Vec<i32>>,
) -> Result<Option<i32>, Exception> {
    let mut isolated = Some(isolated);
    let mut isolated_pids: Option<Vec<i32>> = None;
    for &tid in tids {
        let (uid_of_thread, pid_of_thread) = status(tid)?;
        if check_tgid && pid_of_thread == tgid {
            continue;
        }
        if uid_of_thread == uid {
            continue;
        }
        if isolated_pids.is_none() {
            // To avoid deadlock, do not call into AMS if the call is from
            // the system.
            if uid % PER_USER_RANGE as i32 == SYSTEM_UID {
                return Ok(Some(tid));
            }
            match isolated.take().and_then(|f| f()) {
                Some(pids) => isolated_pids = Some(pids),
                None => return Ok(Some(tid)),
            }
        }
        if isolated_pids
            .as_ref()
            .is_some_and(|p| p.contains(&pid_of_thread))
        {
            continue;
        }
        return Ok(Some(tid));
    }
    Ok(None)
}

/// `formatTidCheckErrMsg`, with `Arrays.toString` of the list.
fn tid_error(uid: i32, tids: &[i32], tid: i32) -> Exception {
    let list = tids
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Exception::security(format!(
        "Tid{tid} from list [{list}] doesn't belong to the calling application {uid}"
    ))
}

/// `validateWorkDuration`.
fn validate_work_duration(d: &WorkDuration) -> Result<(), Exception> {
    let fail = |m: String| Err(Exception::illegal_argument(m));
    if d.duration_nanos <= 0 {
        return fail(format!(
            "Actual total duration ({}) should be greater than 0",
            d.duration_nanos
        ));
    }
    if d.work_period_start_timestamp_nanos < 0 {
        return fail(format!(
            "Work period start timestamp ({}) should be greater than 0",
            d.work_period_start_timestamp_nanos
        ));
    }
    if d.cpu_duration_nanos < 0 {
        return fail(format!(
            "Actual CPU duration ({}) should be greater than or equal to 0",
            d.cpu_duration_nanos
        ));
    }
    if d.gpu_duration_nanos < 0 {
        return fail(format!(
            "Actual GPU duration ({}) should greater than or equal to 0",
            d.gpu_duration_nanos
        ));
    }
    if d.cpu_duration_nanos + d.gpu_duration_nanos <= 0 {
        return fail(format!(
            "The actual CPU duration ({}) and the actual GPU duration ({}) should not both be 0",
            d.cpu_duration_nanos, d.gpu_duration_nanos
        ));
    }
    Ok(())
}

/// `ThreadUsageTracker`s of a uid: each thread and whether it is of a
/// graphics pipeline session.
type Threads = Vec<(i32, bool)>;

/// Removes the trackers of `tids`, as the original's loop over the set
/// removes them (an element after a removed one is not looked at).
fn remove_threads(threads: &mut Threads, tids: &[i32]) {
    let mut i = 0;
    while i < threads.len() {
        if tids.contains(&threads[i].0) {
            threads.remove(i);
        }
        i += 1;
    }
}

/// `tooManyPipelineThreads`.
fn too_many_pipeline_threads(threads: Option<&Threads>) -> bool {
    threads.is_some_and(|t| {
        t.iter().filter(|(_, gp)| *gp).count() > MAX_GRAPHICS_PIPELINE_THREADS as usize
    })
}

fn not_empty(what: &str) -> Exception {
    Exception::illegal_argument(what)
}

pub struct HintManager {
    host: Arc<AppHost>,
    me: Weak<HintManager>,
    data: OnceLock<ClientData>,
    /// `mFMQUsesIntegratedEventFlag`: the HAL's channel had no event flag.
    integrated_event_flag: AtomicBool,
    /// `mConfigCreationSupport`: whether the HAL makes sessions with a
    /// config.
    config_creation: AtomicBool,
    /// `mActiveSessions`.
    active: Mutex<Vec<Arc<AppHintSession>>>,
    /// The sessions by their binders, which `setHintSessionThreads` and
    /// `getHintSessionThreadIds` are handed.
    by_binder: Mutex<HashMap<u64, Arc<AppHintSession>>>,
    /// `mThreadsUsageMap`, by uid.
    threads: Mutex<HashMap<i32, Threads>>,
    /// `mSessionManager`, SurfaceFlinger's.
    session_manager: Mutex<Option<Arc<Strong>>>,
}

impl HintManager {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        Arc::new_cyclic(|me| HintManager {
            host,
            me: me.clone(),
            data: OnceLock::new(),
            integrated_event_flag: AtomicBool::new(false),
            config_creation: AtomicBool::new(true),
            active: Mutex::new(Vec::new()),
            by_binder: Mutex::new(HashMap::new()),
            threads: Mutex::new(HashMap::new()),
            session_manager: Mutex::new(None),
        })
    }

    fn session_manager(&self) -> Option<Arc<Strong>> {
        self.session_manager.lock().unwrap().clone()
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    /// Whether the FMQ channel is in use: a HAL from version 5 and the
    /// flag on.
    fn uses_channel(&self) -> Result<bool, Exception> {
        Ok(self.client_data()?.power_hal_version >= 5 && self.flag(ADPF_USE_FMQ_CHANNEL)?)
    }

    fn hal(&self) -> Result<Strong, Exception> {
        super::display::check_service(&self.host.process, POWER)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
    }

    /// `getSessionChannel` of the caller's process (`tgid`) and `uid`:
    /// none without the channel or once a HAL showed it has no event flag;
    /// else the HAL's channel is opened, and closed again when it has none.
    fn session_channel(&self, tgid: i32, uid: i32, token: Option<Binder>) -> Result<(), Exception> {
        if !self.uses_channel()? || self.integrated_event_flag.load(Ordering::Acquire) {
            return Ok(());
        }
        if token.is_none() {
            return Err(Exception::new(EX_NULL_POINTER, "token"));
        }
        let hal = self.hal()?;
        let ids = |p: &mut Parcel| {
            p.write_i32(tgid);
            p.write_i32(uid);
        };
        let event_flag = hal_call_with(&hal, GET_SESSION_CHANNEL, ids, has_event_flag)??;
        // `closeSessionChannel`, which closes the HAL's.
        let mut data = Parcel::new();
        data.write_interface_token(POWER_DESCRIPTOR);
        ids(&mut data);
        hal.transact(CLOSE_SESSION_CHANNEL, &data, true)
            .map_err(|s| {
                Exception::new(EX_ILLEGAL_STATE, format!("closeSessionChannel: status {s}"))
            })?;
        if event_flag {
            return Err(self.host.unimplemented_named(
                "performance_hint",
                "a session channel with an event flag (#1689)",
            ));
        }
        self.integrated_event_flag.store(true, Ordering::Release);
        Ok(())
    }

    /// What the constructor reads: `halGetHintSessionPreferredRate` (-1
    /// when the HAL has none) and `getSupportInfo`.
    fn client_data(&self) -> Result<ClientData, Exception> {
        if let Some(d) = self.data.get() {
            return Ok(d.clone());
        }
        let hal = self.hal()?;
        let preferred_rate_nanos =
            hal_call(&hal, GET_HINT_SESSION_PREFERRED_RATE, |r| r.read_i64())?.unwrap_or(-1);
        let power_hal_version = hal_call(&hal, GET_INTERFACE_VERSION, |r| r.read_i32())??;
        if power_hal_version < 6 {
            return Err(self.host.unimplemented_named(
                "performance_hint",
                "the support info of a Power HAL before version 6 (#1689)",
            ));
        }
        let support_info = hal_call(&hal, GET_SUPPORT_INFO, |r| {
            if r.read_i32()? == 0 {
                return Err(BAD_VALUE);
            }
            parcelable_words(r)
        })??;
        Ok(self
            .data
            .get_or_init(|| ClientData {
                power_hal_version,
                preferred_rate_nanos,
                support_info,
            })
            .clone())
    }

    /// `checkTidValid` of `tids` for the caller (`uid`, its process
    /// `tgid`, which binder reports as its pid), each thread's uid and
    /// process read from its `/proc/<tid>/status` by the agent.
    fn check_tids(&self, uid: i32, tgid: i32, tids: &[i32]) -> Result<(), Exception> {
        let status = |tid: i32| -> Result<(i32, i32), Exception> {
            let reply = self
                .host
                .call_agent(READ_PROC_STATUS, "readProcStatus", |p| p.write_i32(tid))?;
            let mut r = reply.reader();
            let text = r
                .read_exception()
                .ok()
                .and_then(|_| aim_service_aidl::read_byte_array(&mut r).ok());
            let text =
                text.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "readProcStatus reply"))?;
            // A status that cannot be opened reads as zeros.
            let values = read_proc_lines(&text.unwrap_or_default(), &["Uid:", "Tgid:"]);
            Ok((values[0] as i32, values[1] as i32))
        };
        let check_tgid = self.flag(POWERHINT_THREAD_CLEANUP)?;
        match invalid_tid(uid, tgid, tids, check_tgid, status, || {
            self.host.isolated_processes(uid)
        })? {
            Some(tid) => Err(tid_error(uid, tids, tid)),
            None => Ok(()),
        }
    }

    /// `resetOnForkEnabled`'s SCHED_RESET_ON_FORK of `tids`, whose failure
    /// the original logs and goes on from.
    fn reset_on_fork(&self) -> Result<(), Exception> {
        if self.flag(RESET_ON_FORK_ENABLED)? {
            // Logged once; the session goes on, as it does after the
            // original's failure.
            self.host.unimplemented_named(
                "performance_hint",
                "SCHED_RESET_ON_FORK of a session's threads (#1830)",
            );
        }
        Ok(())
    }

    /// `getUidApplicationCategory`: the category of the package
    /// `getNameForUid` names, `CATEGORY_UNDEFINED` (-1) for none.
    fn category(&self, uid: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let name = query
            .name_for_uid(uid)
            .map_err(|e| self.host.unimplemented_named("performance_hint", e.0))?;
        Ok(name
            .and_then(|n| {
                self.host
                    .inputs()
                    .ok()?
                    .packages
                    .application_info(&n, MATCH_ALL)
            })
            .map_or(-1, |ai| ai.category))
    }

    /// `createHintSessionWithConfig` of the caller `uid` (its process
    /// `tgid`): the session, whether the uid's graphics pipeline threads
    /// are too many, and the HAL's config id.
    fn create_session(
        &self,
        uid: i32,
        tgid: i32,
        token: Option<Binder>,
        mut tag: i32,
        config: Option<CreationConfig>,
    ) -> Result<(Binder, bool, i64), Exception> {
        if self.client_data()?.preferred_rate_nanos == -1 {
            return Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "PowerHintSessions are not supported!",
            ));
        }
        let token = token.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let config = config.ok_or_else(|| {
            Exception::new(
                EX_NULL_POINTER,
                "Attempt to read from field 'int[] android.os.SessionCreationConfig.tids' on a null object reference",
            )
        })?;
        let tids = config
            .tids
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        if tids.is_empty() {
            return Err(not_empty("tids should not be empty."));
        }
        let duration_nanos = config.target_work_duration_nanos;
        let modes = config.modes_to_enable.unwrap_or_default();
        let graphics_pipeline = modes.contains(&GRAPHICS_PIPELINE);
        let auto_timed = modes.iter().any(|m| *m == AUTO_CPU || *m == AUTO_GPU);
        if auto_timed && !graphics_pipeline {
            return Err(Exception::illegal_argument(
                "graphics pipeline mode not enabled for an automatically timed session",
            ));
        }
        self.check_tids(uid, tgid, &tids)?;
        self.reset_on_fork()?;
        if self.flag(ADPF_SESSION_TAG)? && tag == TAG_APP {
            tag = if self.category(uid)? == CATEGORY_GAME {
                TAG_GAME
            } else {
                TAG_APP
            };
        }
        let hal = self.hal()?;
        let mut created = None;
        if self.config_creation.load(Ordering::Acquire) {
            match hal_create(&hal, tgid, uid, &tids, duration_nanos, Some(tag)) {
                Created::Session(session, id) => created = Some((session, id)),
                Created::Unsupported(_) => self.config_creation.store(false, Ordering::Release),
                Created::Failed(m) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("createHintSessionWithConfig failed: {m}"),
                    ));
                }
            }
        }
        let (hal_session, id) = match created {
            Some(created) => created,
            None => match hal_create(&hal, tgid, uid, &tids, duration_nanos, None) {
                Created::Session(session, _) => (session, -1),
                Created::Unsupported(m) => {
                    return Err(Exception::new(
                        EX_UNSUPPORTED_OPERATION,
                        format!("createHintSession unsupported: {m}"),
                    ));
                }
                Created::Failed(m) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("createHintSession failed: {m}"),
                    ));
                }
            },
        };
        let session = Arc::new(AppHintSession {
            manager: self.me.clone(),
            uid,
            token,
            session_id: (id != -1).then_some(id as i32),
            state: Mutex::new(SessionState {
                hal: Some(hal_session),
                token_death: None,
                thread_ids: tids.clone(),
                graphics_pipeline: false,
                tracked_by_sf: false,
            }),
        });
        session.link_token(&self.host);
        let Binder::Local(ptr) = self.host.process.add_service(session.clone()) else {
            unreachable!("a local binder")
        };
        self.by_binder.lock().unwrap().insert(ptr, session.clone());
        self.active.lock().unwrap().push(session.clone());
        for mode in modes {
            session.set_mode(mode, true)?;
        }
        if let Some(layers) = config.layer_tokens.filter(|l| !l.is_empty()) {
            session.associate_to_layers(&layers)?;
        }
        let exceeded = {
            let mut threads = self.threads.lock().unwrap();
            let set = threads.entry(uid).or_default();
            set.extend(tids.iter().map(|&t| (t, graphics_pipeline)));
            too_many_pipeline_threads(Some(set))
        };
        Ok((Binder::Local(ptr), exceeded, id))
    }

    /// The session of a binder handed back: none for null, and the
    /// original's `ClassCastException` (which no exception code carries)
    /// for another's.
    fn session_of(
        &self,
        binder: Option<Binder>,
        method: &str,
    ) -> Result<Arc<AppHintSession>, Reply> {
        match binder {
            None => Err(reply(Err(Exception::new(
                EX_NULL_POINTER,
                format!("Attempt to invoke virtual method '{method}' on a null object reference"),
            )))),
            Some(Binder::Local(ptr)) => match self.by_binder.lock().unwrap().get(&ptr) {
                Some(s) => Ok(s.clone()),
                None => Err(Err(UNKNOWN_TRANSACTION)),
            },
            Some(Binder::Handle(_)) => Err(Err(UNKNOWN_TRANSACTION)),
        }
    }

    /// `MyUidObserver.onUidGone`: the uid's sessions are closed, those
    /// SurfaceFlinger tracks told to it first, per token.
    pub(super) fn uid_gone(&self, uid: i32) {
        let sessions: Vec<Arc<AppHintSession>> = self
            .active
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.uid == uid)
            .cloned()
            .collect();
        let mut tokens: Vec<Binder> = Vec::new();
        for s in &sessions {
            if !tokens.contains(&s.token) {
                tokens.push(s.token);
            }
        }
        for token in tokens.into_iter().rev() {
            let set: Vec<&Arc<AppHintSession>> =
                sessions.iter().filter(|s| s.token == token).collect();
            let died: Vec<i32> = set
                .iter()
                .filter_map(|s| {
                    let mut st = s.state.lock().unwrap();
                    let tracked = std::mem::replace(&mut st.tracked_by_sf, false);
                    s.session_id.filter(|_| tracked)
                })
                .collect();
            if let Some(manager) = self.session_manager()
                && let Err(s) = tracked_sessions_died(&manager, &died)
            {
                eprintln!(
                    "app host: performance_hint: Failed to communicate with SessionManager: status {s}"
                );
            }
            for s in set {
                if let Err(e) = s.close() {
                    eprintln!(
                        "app host: performance_hint: closing a session of uid {uid}: {}",
                        e.message
                    );
                }
            }
        }
    }
}

/// `ISessionManager.trackedSessionsDied`.
fn tracked_sessions_died(
    manager: &Strong,
    ids: &[i32],
) -> Result<(), aim_binder_host::parcel::StatusCode> {
    let mut data = Parcel::new();
    data.write_interface_token(SESSION_MANAGER_DESCRIPTOR);
    write_int_array(&mut data, Some(ids));
    manager
        .transact(TRACKED_SESSIONS_DIED, &data, true)
        .map(drop)
}

/// A session's state (`AppHintSession`'s, under its lock).
struct SessionState {
    /// The HAL's session; none once closed (`mHalSessionPtr` 0).
    hal: Option<HalSession>,
    /// The token's death link.
    token_death: Option<(Strong, u64)>,
    thread_ids: Vec<i32>,
    graphics_pipeline: bool,
    tracked_by_sf: bool,
}

/// `AppHintSession`: an app's `IHintSession`.
struct AppHintSession {
    manager: Weak<HintManager>,
    uid: i32,
    token: Binder,
    session_id: Option<i32>,
    state: Mutex<SessionState>,
}

impl AppHintSession {
    /// `token.linkToDeath(this)`: the session closes when its token dies
    /// (a binder of the app host's own never does).
    fn link_token(self: &Arc<Self>, host: &AppHost) {
        let Binder::Handle(handle) = self.token else {
            return;
        };
        let strong = host.process.strong(handle);
        let me = Arc::downgrade(self);
        let cookie = host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(s) = me.upgrade()
                    && let Err(e) = s.close()
                {
                    eprintln!(
                        "app host: performance_hint: a session's token died: {}",
                        e.message
                    );
                }
            }),
        );
        self.state.lock().unwrap().token_death = Some((strong, cookie));
    }

    fn manager(&self) -> Result<Arc<HintManager>, Exception> {
        self.manager
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "performance_hint is gone"))
    }

    fn update_target_work_duration(&self, nanos: i64) -> Result<(), Exception> {
        let st = self.state.lock().unwrap();
        let Some(hal) = &st.hal else { return Ok(()) };
        if nanos < 0 {
            return Err(Exception::illegal_argument(
                "Expected the target duration to be greater than or equal to 0.",
            ));
        }
        hal.call(UPDATE_TARGET_WORK_DURATION, |p| p.write_i64(nanos));
        Ok(())
    }

    /// `reportActualWorkDuration` (the legacy form) and
    /// `reportActualWorkDuration2`, both to the HAL's
    /// `reportActualWorkDuration`.
    fn report(&self, durations: &[WorkDuration]) {
        let st = self.state.lock().unwrap();
        if let Some(hal) = &st.hal {
            hal.call(REPORT_ACTUAL_WORK_DURATION, |p| {
                p.write_i32(durations.len() as i32);
                for d in durations {
                    p.write_i32(1);
                    d.write_to(p);
                }
            });
        }
    }

    fn report_actual_work_duration(
        &self,
        actual: Option<Vec<i64>>,
        stamps: Option<Vec<i64>>,
    ) -> Result<(), Exception> {
        if self.state.lock().unwrap().hal.is_none() {
            return Ok(());
        }
        let null = || Exception::new(EX_NULL_POINTER, "Attempt to get length of null array");
        let actual = actual.ok_or_else(null)?;
        if actual.is_empty() {
            return Err(not_empty("the count of hint durations shouldn't be 0."));
        }
        let stamps = stamps.ok_or_else(null)?;
        if actual.len() != stamps.len() {
            return Err(Exception::illegal_argument(
                "The length of durations and timestamps should be the same.",
            ));
        }
        if let Some((i, d)) = actual.iter().enumerate().find(|(_, d)| **d <= 0) {
            return Err(Exception::illegal_argument(format!(
                "durations[{i}]={d} should be greater than 0"
            )));
        }
        let durations: Vec<WorkDuration> = actual
            .iter()
            .zip(&stamps)
            .map(|(&duration_nanos, &time_stamp_nanos)| WorkDuration {
                time_stamp_nanos,
                duration_nanos,
                ..WorkDuration::default()
            })
            .collect();
        self.report(&durations);
        Ok(())
    }

    fn report_actual_work_duration2(
        &self,
        durations: Option<Vec<Option<WorkDuration>>>,
    ) -> Result<(), Exception> {
        if self.state.lock().unwrap().hal.is_none() {
            return Ok(());
        }
        let durations = durations.ok_or_else(|| {
            Exception::new(EX_NULL_POINTER, "Attempt to get length of null array")
        })?;
        if durations.is_empty() {
            return Err(not_empty("the count of work durations shouldn't be 0."));
        }
        let durations = durations
            .into_iter()
            .map(|d| {
                let d = d.ok_or_else(|| Exception::new(EX_NULL_POINTER, "workDuration"))?;
                validate_work_duration(&d).map(|()| d)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.report(&durations);
        Ok(())
    }

    /// `close`: the HAL's session closed, the token unlinked, SurfaceFlinger
    /// told if it tracks it, and the session and (of a graphics pipeline
    /// one) its threads forgotten.
    fn close(&self) -> Result<(), Exception> {
        let manager = self.manager()?;
        let (threads, graphics_pipeline) = {
            let mut st = self.state.lock().unwrap();
            let Some(hal) = st.hal.take() else {
                return Ok(());
            };
            hal.call(CLOSE, |_| {});
            if let Some((strong, cookie)) = st.token_death.take() {
                manager.host.process.clear_death(&strong, cookie);
            }
            if st.tracked_by_sf {
                match manager.session_manager() {
                    Some(sm) => {
                        tracked_sessions_died(
                            &sm,
                            &self.session_id.into_iter().collect::<Vec<_>>(),
                        )
                        .map_err(|_| {
                            Exception::new(
                                EX_ILLEGAL_STATE,
                                "Could not communicate with SessionManager",
                            )
                        })?;
                        st.tracked_by_sf = false;
                    }
                    None => eprintln!(
                        "app host: performance_hint: SessionManager is null but there are tracked sessions"
                    ),
                }
            }
            (st.thread_ids.clone(), st.graphics_pipeline)
        };
        manager
            .active
            .lock()
            .unwrap()
            .retain(|s| !std::ptr::eq(Arc::as_ptr(s), self));
        if graphics_pipeline {
            let mut all = manager.threads.lock().unwrap();
            if let Some(set) = all.get_mut(&self.uid) {
                remove_threads(set, &threads);
                if set.is_empty() {
                    all.remove(&self.uid);
                }
            }
        }
        Ok(())
    }

    fn send_hint(&self, hint: i32) -> Result<(), Exception> {
        let st = self.state.lock().unwrap();
        let Some(hal) = &st.hal else { return Ok(()) };
        if hint < 0 {
            return Err(Exception::illegal_argument(
                "the hint ID value should be greater than zero.",
            ));
        }
        hal.call(SEND_HINT, |p| p.write_i32(hint));
        Ok(())
    }

    fn set_mode(&self, mode: i32, enabled: bool) -> Result<(), Exception> {
        let mut st = self.state.lock().unwrap();
        let Some(hal) = &st.hal else { return Ok(()) };
        if mode < 0 {
            return Err(Exception::illegal_argument(
                "the mode Id value should be greater than zero.",
            ));
        }
        hal.call(SET_MODE, |p| {
            p.write_i32(mode);
            p.write_bool(enabled);
        });
        if mode == GRAPHICS_PIPELINE {
            st.graphics_pipeline = enabled;
        }
        Ok(())
    }

    /// `associateToLayers`, through SurfaceFlinger's session manager.
    fn associate_to_layers(&self, layers: &[Option<Binder>]) -> Result<(), Exception> {
        let (Some(sm), Some(id)) = (self.manager()?.session_manager(), self.session_id) else {
            return Ok(());
        };
        let mut st = self.state.lock().unwrap();
        if !layers.is_empty() {
            st.tracked_by_sf = true;
        }
        let mut data = Parcel::new();
        data.write_interface_token(SESSION_MANAGER_DESCRIPTOR);
        data.write_i32(id);
        data.write_i32(self.uid);
        write_binder_array(&mut data, layers);
        sm.transact(ASSOCIATE_SESSION_TO_LAYERS, &data, true)
            .map(drop)
            .map_err(|_| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "Could not communicate with SessionManager",
                )
            })
    }

    /// `setThreads` from `setHintSessionThreads` of the caller (`uid`, its
    /// process `tgid`).
    fn set_threads(&self, uid: i32, tgid: i32, tids: Option<Vec<i32>>) -> Result<(), Exception> {
        let tids = tids.ok_or_else(|| {
            Exception::new(EX_NULL_POINTER, "Attempt to get length of null array")
        })?;
        if tids.is_empty() {
            return Err(not_empty("Thread id list can't be empty."));
        }
        let manager = self.manager()?;
        let mut st = self.state.lock().unwrap();
        if let Some(hal) = &st.hal {
            manager.check_tids(uid, tgid, &tids)?;
            manager.reset_on_fork()?;
            hal.call(SET_THREADS, |p| write_int_array(p, Some(&tids)));
            let mut threads = manager.threads.lock().unwrap();
            let set = threads.entry(uid).or_default();
            remove_threads(set, &st.thread_ids);
            set.extend(tids.iter().map(|&t| (t, st.graphics_pipeline)));
            st.thread_ids = tids;
        }
        drop(st);
        if too_many_pipeline_threads(manager.threads.lock().unwrap().get(&uid)) {
            // A success, which the original fails anyway.
            return Err(Exception {
                code: EX_SERVICE_SPECIFIC,
                message: "Not enough available graphics pipeline threads.".into(),
                service_specific: 5,
                parcelable: None,
            });
        }
        Ok(())
    }
}

impl Service for AppHintSession {
    fn descriptor(&self) -> &str {
        hs::DESCRIPTOR
    }

    /// Its calls, all one-way: a failure is the caller's to log.
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let result = match call.code {
            hs::UPDATE_TARGET_WORK_DURATION => {
                let a =
                    hs::UpdateTargetWorkDuration::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.update_target_work_duration(a.target_duration_nanos)
            }
            hs::REPORT_ACTUAL_WORK_DURATION => {
                let a =
                    hs::ReportActualWorkDuration::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.report_actual_work_duration(a.actual_duration_nanos, a.time_stamp_nanos)
            }
            hs::CLOSE => {
                hs::Close::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.close()
            }
            hs::SEND_HINT => {
                let a = hs::SendHint::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.send_hint(a.hint)
            }
            hs::SET_MODE => {
                let a = hs::SetMode::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_mode(a.mode, a.enabled)
            }
            hs::REPORT_ACTUAL_WORK_DURATION2 => {
                let a = hs::ReportActualWorkDuration2::<WorkDuration>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.report_actual_work_duration2(a.work_durations)
            }
            hs::ASSOCIATE_TO_LAYERS => {
                call.data
                    .enforce_interface(hs::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                match read_binder_array(&mut call.data).map_err(|_| BAD_VALUE)? {
                    Some(layers) => self.associate_to_layers(&layers),
                    None => Ok(()),
                }
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| Parcel::new()))
    }
}

impl Service for HintManager {
    fn descriptor(&self) -> &str {
        hm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            hm::CREATE_HINT_SESSION_WITH_CONFIG => {
                let a = hm::CreateHintSessionWithConfig::<CreationConfig>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.create_session(uid, call.sender_pid, a.token, a.tag, a.creation_config)
                    .map(|(session, exceeded, id)| {
                        let value = hm::CreateHintSessionWithConfigReply {
                            result: Some(CreationReturn {
                                session,
                                pipeline_thread_limit_exceeded: exceeded,
                            }),
                            config: Some(SessionConfig { id }),
                        };
                        hm::write_create_hint_session_with_config_reply(&mut p, &value);
                    })
            }
            hm::SET_HINT_SESSION_THREADS => {
                let a = hm::SetHintSessionThreads::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let method = "void com.android.server.power.hint.HintManagerService$AppHintSession.setThreads(int[])";
                let session = match self.session_of(a.hint_session, method) {
                    Ok(s) => s,
                    Err(r) => return r,
                };
                session
                    .set_threads(uid, call.sender_pid, a.tids)
                    .map(|()| hm::write_set_hint_session_threads_reply(&mut p))
            }
            hm::GET_HINT_SESSION_THREAD_IDS => {
                let a = hm::GetHintSessionThreadIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let method = "int[] com.android.server.power.hint.HintManagerService$AppHintSession.getThreadIds()";
                let session = match self.session_of(a.hint_session, method) {
                    Ok(s) => s,
                    Err(r) => return r,
                };
                let ids = session.state.lock().unwrap().thread_ids.clone();
                hm::write_get_hint_session_thread_ids_reply(&mut p, &Some(ids));
                Ok(())
            }
            hm::GET_SESSION_CHANNEL => {
                let a = hm::GetSessionChannel::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.session_channel(call.sender_pid, uid, a.token)
                    .map(|()| hm::write_get_session_channel_reply::<ClientData>(&mut p, None))
            }
            // A channel is never kept open, so there is none to close
            // (`removeChannelItem`).
            hm::CLOSE_SESSION_CHANNEL => Ok(()),
            // Only the system's own (`Process.myUid()`) is taken.
            hm::PASS_SESSION_MANAGER_BINDER => {
                let a =
                    hm::PassSessionManagerBinder::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if uid == SYSTEM_UID {
                    *self.session_manager.lock().unwrap() = match a.session_manager {
                        Some(Binder::Handle(handle)) => {
                            Some(Arc::new(self.host.process.strong(handle)))
                        }
                        _ => None,
                    };
                }
                Ok(())
            }
            hm::REGISTER_CLIENT => {
                hm::RegisterClient::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.client_data()
                    .map(|d| hm::write_register_client_reply(&mut p, Some(&d)))
            }
            hm::GET_CLIENT_DATA => {
                hm::GetClientData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.client_data()
                    .map(|d| hm::write_get_client_data_reply(&mut p, Some(&d)))
            }
            code => {
                let name = hm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("performance_hint", &format!("{name} (#1689)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `HintManagerClientData.writeToParcel`: its size, the fields in
    /// order, and the support info as the HAL wrote it.
    #[test]
    fn client_data_parcels_as_the_original() {
        let d = ClientData {
            power_hal_version: 6,
            preferred_rate_nanos: 1_000_000,
            support_info: vec![12, 1, 0],
        };
        let mut p = Parcel::new();
        d.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 4 * 4 + 8 + 4 + 12);
        assert_eq!(
            (
                r.read_i32().unwrap(),
                r.read_i32().unwrap(),
                r.read_i32().unwrap()
            ),
            (6, 5, 5)
        );
        assert_eq!(r.read_i64().unwrap(), 1_000_000);
        assert_eq!(r.read_i32().unwrap(), 1);
        let mut words = Reader::new(&p.data()[r.position()..], &[]);
        assert_eq!(parcelable_words(&mut words).unwrap(), [12, 1, 0]);
        assert_eq!(words.remaining(), 0);
    }

    /// A HAL's `ChannelConfig` reply: present, its size, its present
    /// `channelDescriptor` (skipped by its size), then whether an event
    /// flag descriptor follows.
    #[test]
    fn channel_config_tells_whether_it_has_an_event_flag() {
        for flag in [0, 1] {
            let mut p = Parcel::new();
            p.write_i32(1);
            p.write_i32(36);
            p.write_i32(1);
            for w in [12, 7, 9] {
                p.write_i32(w);
            }
            p.write_i32(flag);
            let mut r = p.reader();
            assert_eq!(has_event_flag(&mut r).unwrap(), flag == 1);
        }
    }

    /// `SessionCreationConfig.writeToParcel`'s fields are read, and an
    /// older writer's shorter parcelable leaves the rest unset.
    #[test]
    fn creation_config_reads_the_fields_its_size_covers() {
        let mut p = Parcel::new();
        write_sized(&mut p, |p| {
            write_int_array(p, Some(&[7, 8]));
            p.write_i64(16_666_666);
            write_int_array(p, Some(&[GRAPHICS_PIPELINE]));
            write_binder_array(p, &[]);
        });
        p.write_i32(42);
        let mut r = p.reader();
        let c = CreationConfig::read_from(&mut r).unwrap();
        assert_eq!(c.tids.as_deref(), Some(&[7, 8][..]));
        assert_eq!(c.target_work_duration_nanos, 16_666_666);
        assert_eq!(c.modes_to_enable.as_deref(), Some(&[GRAPHICS_PIPELINE][..]));
        assert_eq!(c.layer_tokens, Some(Vec::new()));
        assert_eq!(r.read_i32().unwrap(), 42);

        let mut p = Parcel::new();
        write_sized(&mut p, |p| write_int_array(p, Some(&[7])));
        let c = CreationConfig::read_from(&mut p.reader()).unwrap();
        assert_eq!(
            (c.tids.as_deref(), c.target_work_duration_nanos),
            (Some(&[7][..]), 0)
        );
        assert_eq!((c.modes_to_enable, c.layer_tokens), (None, None));
    }

    /// The reply's `SessionCreationReturn` and `SessionConfig`, each
    /// sized as a structured parcelable writes itself.
    #[test]
    fn creation_reply_parcels_as_the_stub_writes_it() {
        let mut p = Parcel::new();
        let value = hm::CreateHintSessionWithConfigReply {
            result: Some(CreationReturn {
                session: Binder::Local(0x40),
                pipeline_thread_limit_exceeded: true,
            }),
            config: Some(SessionConfig { id: 9 }),
        };
        hm::write_create_hint_session_with_config_reply(&mut p, &value);
        let mut r = p.reader();
        assert!(r.read_exception().unwrap().is_ok());
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_i32().unwrap(), 4 + 24 + 4 + 4); // the binder, its stability, the boolean
        assert_eq!(r.read_binder().unwrap(), Some(Binder::Local(0x40)));
        assert!(r.read_bool().unwrap());
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 12));
        assert_eq!(r.read_i64().unwrap(), 9);
        assert_eq!(r.remaining(), 0);
    }

    /// An app's `WorkDuration` is read by its size; the HAL is handed it
    /// with every field written and the intended present time zero.
    #[test]
    fn work_durations_round_trip_without_the_present_time() {
        let mut p = Parcel::new();
        write_sized(&mut p, |p| {
            for v in [1, 2, 3, 4, 5, 6] {
                p.write_i64(v);
            }
        });
        let d = WorkDuration::read_from(&mut p.reader()).unwrap();
        assert_eq!(
            d,
            WorkDuration {
                time_stamp_nanos: 1,
                duration_nanos: 2,
                work_period_start_timestamp_nanos: 3,
                cpu_duration_nanos: 4,
                gpu_duration_nanos: 5,
            }
        );
        let mut q = Parcel::new();
        d.write_to(&mut q);
        let mut r = q.reader();
        assert_eq!(r.read_i32().unwrap(), 52);
        let fields: Vec<i64> = (0..6).map(|_| r.read_i64().unwrap()).collect();
        assert_eq!(fields, [1, 2, 3, 4, 5, 0]);
    }

    /// `validateWorkDuration`'s checks, in its order.
    #[test]
    fn work_durations_are_validated_as_the_original() {
        let ok = WorkDuration {
            duration_nanos: 10,
            cpu_duration_nanos: 4,
            ..WorkDuration::default()
        };
        assert!(validate_work_duration(&ok).is_ok());
        let message = |d: WorkDuration| validate_work_duration(&d).unwrap_err().message;
        assert_eq!(
            message(WorkDuration {
                duration_nanos: 0,
                ..ok
            }),
            "Actual total duration (0) should be greater than 0"
        );
        assert_eq!(
            message(WorkDuration {
                cpu_duration_nanos: 0,
                ..ok
            }),
            "The actual CPU duration (0) and the actual GPU duration (0) should not both be 0"
        );
        assert_eq!(
            message(WorkDuration {
                gpu_duration_nanos: -1,
                ..ok
            }),
            "Actual GPU duration (-1) should greater than or equal to 0"
        );
    }

    /// `readProcLines` of a `/proc/<tid>/status`: the real uid (the first
    /// number) and the thread group, 0 for a key that is missing.
    #[test]
    fn proc_lines_read_each_keys_first_number() {
        let status = b"Name:\tRenderThread\nUmask:\t0022\nState:\tS (sleeping)\nTgid:\t4210\nPid:\t4233\nUid:\t10154\t10154\t10154\t10154\nGid:\t10154\n";
        assert_eq!(read_proc_lines(status, &["Uid:", "Tgid:"]), [10154, 4210]);
        assert_eq!(read_proc_lines(b"Name:\tx\n", &["Uid:", "Tgid:"]), [0, 0]);
        assert_eq!(read_proc_lines(b"", &["Uid:"]), [0]);
    }

    /// `checkTidValid`: the caller's process's threads, then its uid's,
    /// then its isolated processes'; another's thread is the first
    /// invalid one, and the system's isolated processes are not asked.
    #[test]
    fn tids_are_checked_as_check_tid_valid_checks_them() {
        let threads = |tid: i32| -> Result<(i32, i32), Exception> {
            Ok(match tid {
                11 | 12 => (10154, 10),
                21 => (10154, 20),
                31 => (99001, 30),
                _ => (10200, 40),
            })
        };
        let isolated = || Some(vec![30]);
        assert_eq!(
            invalid_tid(10154, 10, &[11, 12, 21], true, threads, isolated).unwrap(),
            None
        );
        assert_eq!(
            invalid_tid(10154, 10, &[11, 31], true, threads, isolated).unwrap(),
            None
        );
        assert_eq!(
            invalid_tid(10154, 10, &[11, 41, 12], true, threads, isolated).unwrap(),
            Some(41)
        );
        assert_eq!(
            invalid_tid(10154, 10, &[31], true, threads, || None).unwrap(),
            Some(31)
        );
        assert_eq!(
            invalid_tid(1000, 10, &[41], true, threads, || panic!("not asked")).unwrap(),
            Some(41)
        );
        assert_eq!(
            tid_error(10154, &[11, 41], 41).message,
            "Tid41 from list [11, 41] doesn't belong to the calling application 10154"
        );
    }

    /// The trackers of a session's threads are dropped as the original's
    /// loop drops them, and more than five graphics pipeline threads of a
    /// uid are too many.
    #[test]
    fn graphics_pipeline_threads_are_counted() {
        let mut threads: Threads = vec![(1, true), (2, true), (3, false)];
        remove_threads(&mut threads, &[1, 2]);
        assert_eq!(threads, [(2, true), (3, false)]);
        let mut threads: Threads = (0..6).map(|t| (t, true)).collect();
        assert!(too_many_pipeline_threads(Some(&threads)));
        threads.pop();
        assert!(!too_many_pipeline_threads(Some(&threads)));
        assert!(!too_many_pipeline_threads(None));
    }
}
