//! `jobscheduler`: JobSchedulerService for the app host
//! (`JobSchedulerService`, `JobServiceContext`, `JobConcurrencyManager` and
//! the controllers at `android-16.0.0_r1`, ADR 0013). `schedule`,
//! `enqueue` and `scheduleAsPackage` with the original's checks (the job
//! service is the caller's and requires `BIND_JOB_SERVICE`, persisted jobs
//! need `RECEIVE_BOOT_COMPLETED`, `JobInfo.enforceValidity`, the per-app
//! limit, the persisted-schedule quota, namespaces), cancels, the pending
//! jobs and their reasons; the persisted jobs read from and written to
//! `/data/system/job` (`app_host/job_store.rs`). A job is ready as the
//! controllers decide it on the Mac's state: its delay and deadline on the
//! elapsed clock, periodic jobs with their flex; charging and battery not
//! low from the Mac's battery as BatteryService would report it, storage
//! not low as DeviceStorageMonitorService decides it for the volume of
//! `/data`, both read each minute while a job waits on them; the device
//! never idle (the display is always on); battery saver restricting the
//! jobs of apps not running. The app host has no network yet (#1509): a
//! network constraint is never satisfied. A ready job runs as
//! `JobServiceContext` runs it: its `JobService` bound by the app host
//! (`app_host/services.rs`), `startJob` with an `IJobCallback` of its own,
//! the op timeouts, the runtime limits, its work items, stops with their
//! reasons, rescheduling with back-off and periodic rescheduling; at most
//! the original's concurrency, an app's jobs `TOP` while its process runs.
//! Standby buckets, quotas beyond it, doze, flexibility (off by default),
//! prefetch launch estimates, content triggers, user-initiated jobs and
//! a job's wake lock are not run (#1490), nor URI grants (#1503).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_app_job_ijobcallback as cb;
use aim_service_aidl::android_app_job_ijobscheduler as js;
use aim_service_aidl::android_app_job_ijobservice as job_service;

use super::alarm_service::{elapsed_ms, wall_ms};
use super::job_info::{
    self, BACKOFF_POLICY_EXPONENTIAL, BACKOFF_POLICY_LINEAR, BIAS_DEFAULT, CONSTRAINT_FLAG_BATTERY_NOT_LOW,
    CONSTRAINT_FLAG_CHARGING, CONSTRAINT_FLAG_DEVICE_IDLE, CONSTRAINT_FLAG_STORAGE_NOT_LOW, FLAG_EXEMPT_FROM_APP_STANDBY,
    FLAG_GRANT_URI_PERMISSIONS, FLAG_WILL_BE_FOREGROUND, INTERNAL_STOP_REASON_UNKNOWN, JobInfo, JobParameters,
    JobWorkItem, MAX_BACKOFF_DELAY_MILLIS, MIN_BACKOFF_MILLIS, MIN_FLEX_MILLIS, MIN_PERIOD_MILLIS, NETWORK_BYTES_UNKNOWN,
    PRIORITY_DEFAULT, PRIORITY_HIGH, PRIORITY_LOW, PRIORITY_MAX, PRIORITY_MIN, ReadWorkItem, STOP_REASON_UNDEFINED,
};
use super::job_store::{self, NO_EARLIEST_RUNTIME, NO_LATEST_RUNTIME, Stored};
use super::services::HostConnection;
use super::{AppHost, PER_USER_RANGE, reply};

/// `JobStatus.CONSTRAINT_*` (the first four are `JobInfo`'s).
const CONSTRAINT_CHARGING: i32 = CONSTRAINT_FLAG_CHARGING;
const CONSTRAINT_BATTERY_NOT_LOW: i32 = CONSTRAINT_FLAG_BATTERY_NOT_LOW;
const CONSTRAINT_IDLE: i32 = CONSTRAINT_FLAG_DEVICE_IDLE;
const CONSTRAINT_STORAGE_NOT_LOW: i32 = CONSTRAINT_FLAG_STORAGE_NOT_LOW;
const CONSTRAINT_TIMING_DELAY: i32 = 1 << 31;
const CONSTRAINT_DEADLINE: i32 = 1 << 30;
const CONSTRAINT_CONNECTIVITY: i32 = 1 << 28;
const CONSTRAINT_CONTENT_TRIGGER: i32 = 1 << 26;
const CONSTRAINT_PREFETCH: i32 = 1 << 23;
/// `JobStatus.CONSTRAINTS_OF_INTEREST`, less the flexibility constraint,
/// which flexibility (off: `FcConfig.DEFAULT_APPLIED_CONSTRAINTS` is 0)
/// always satisfies.
const CONSTRAINTS_OF_INTEREST: i32 = CONSTRAINT_CHARGING
    | CONSTRAINT_BATTERY_NOT_LOW
    | CONSTRAINT_STORAGE_NOT_LOW
    | CONSTRAINT_TIMING_DELAY
    | CONSTRAINT_CONNECTIVITY
    | CONSTRAINT_IDLE
    | CONSTRAINT_CONTENT_TRIGGER
    | CONSTRAINT_PREFETCH;
/// The device-wide constraints, read from the Mac.
const DEVICE_CONSTRAINTS: i32 = CONSTRAINT_CHARGING | CONSTRAINT_BATTERY_NOT_LOW | CONSTRAINT_STORAGE_NOT_LOW;
/// `JobStatus.INTERNAL_FLAG_DEMOTED_BY_USER`, `_BY_SYSTEM_UIJ`.
const INTERNAL_FLAG_DEMOTED_BY_USER: i32 = 1 << 1;
const INTERNAL_FLAG_DEMOTED_BY_SYSTEM_UIJ: i32 = 1 << 2;
/// `JobScheduler.RESULT_SUCCESS` and the pending job reasons.
const RESULT_SUCCESS: i32 = 1;
const PENDING_JOB_REASON_INVALID_JOB_ID: i32 = -2;
const PENDING_JOB_REASON_EXECUTING: i32 = -1;
const PENDING_JOB_REASON_UNDEFINED: i32 = 0;
const PENDING_JOB_REASON_APP: i32 = 1;
const PENDING_JOB_REASON_APP_STANDBY: i32 = 2;
const PENDING_JOB_REASON_CONSTRAINT_BATTERY_NOT_LOW: i32 = 4;
const PENDING_JOB_REASON_CONSTRAINT_CHARGING: i32 = 5;
const PENDING_JOB_REASON_CONSTRAINT_CONNECTIVITY: i32 = 6;
const PENDING_JOB_REASON_CONSTRAINT_CONTENT_TRIGGER: i32 = 7;
const PENDING_JOB_REASON_CONSTRAINT_DEVICE_IDLE: i32 = 8;
const PENDING_JOB_REASON_CONSTRAINT_MINIMUM_LATENCY: i32 = 9;
const PENDING_JOB_REASON_CONSTRAINT_PREFETCH: i32 = 10;
const PENDING_JOB_REASON_CONSTRAINT_STORAGE_NOT_LOW: i32 = 11;
const PENDING_JOB_REASON_DEVICE_STATE: i32 = 12;
const PENDING_JOB_REASON_CONSTRAINT_DEADLINE: i32 = 16;
/// `JobParameters.STOP_REASON_*`.
const STOP_REASON_CANCELLED_BY_APP: i32 = 1;
const STOP_REASON_TIMEOUT: i32 = 3;
const STOP_REASON_DEVICE_STATE: i32 = 4;
const STOP_REASON_CONSTRAINT_BATTERY_NOT_LOW: i32 = 5;
const STOP_REASON_CONSTRAINT_CHARGING: i32 = 6;
const STOP_REASON_CONSTRAINT_CONNECTIVITY: i32 = 7;
const STOP_REASON_CONSTRAINT_DEVICE_IDLE: i32 = 8;
const STOP_REASON_CONSTRAINT_STORAGE_NOT_LOW: i32 = 9;
const STOP_REASON_APP_STANDBY: i32 = 12;
const STOP_REASON_USER: i32 = 13;
const STOP_REASON_TIMEOUT_ABANDONED: i32 = 16;
/// `JobParameters.INTERNAL_STOP_REASON_*`.
const INTERNAL_STOP_REASON_CANCELED: i32 = 0;
const INTERNAL_STOP_REASON_CONSTRAINTS_NOT_SATISFIED: i32 = 1;
const INTERNAL_STOP_REASON_TIMEOUT: i32 = 3;
const INTERNAL_STOP_REASON_SUCCESSFUL_FINISH: i32 = 10;
const INTERNAL_STOP_REASON_ANR: i32 = 12;
const INTERNAL_STOP_REASON_TIMEOUT_ABANDONED: i32 = 13;
/// `JobSchedulerService`'s limits and `Constants`' defaults.
const MAX_JOBS_PER_APP: usize = 150;
const MAX_NUM_PERSISTED_JOB_WORK_ITEMS: usize = 100_000;
const MINUTE: i64 = 60 * 1000;
const RUNTIME_FREE_QUOTA_MAX_LIMIT_MS: i64 = 30 * MINUTE;
const RUNTIME_MIN_GUARANTEE_MS: i64 = 10 * MINUTE;
const RUNTIME_MIN_EJ_GUARANTEE_MS: i64 = 3 * MINUTE;
const SYSTEM_STOP_TO_FAILURE_RATIO: i32 = 3;
const ABANDONED_JOB_TIMEOUTS_BEFORE_AGGRESSIVE_BACKOFF: i32 = 3;
const MAX_CPU_ONLY_JOB_BATCH_DELAY_MS: i64 = 31 * MINUTE;
const API_QUOTA_SCHEDULE_COUNT: usize = 250;
const API_QUOTA_SCHEDULE_WINDOW_MS: i64 = MINUTE;
const PERIODIC_JOB_WINDOW_BUFFER: i64 = 30 * MINUTE;
const MAX_ALLOWED_PERIOD_MS: i64 = 365 * 24 * 60 * MINUTE;
/// `JobStore.JOB_PERSIST_DELAY`.
const JOB_PERSIST_DELAY: i64 = 2000;
/// `JobServiceContext`'s op timeouts and duration stamps.
const OP_BIND_TIMEOUT_MILLIS: i64 = 18_000;
const OP_TIMEOUT_MILLIS: i64 = 8000;
const EXECUTION_DURATION_STAMP_PERIOD_MILLIS: i64 = 5 * MINUTE;
/// How often the Mac's battery and storage are read while a job waits on
/// them: healthd's periodic chores (`DEFAULT_PERIODIC_CHORES_INTERVAL_FAST`)
/// and DeviceStorageMonitorService's check (`DEFAULT_CHECK_INTERVAL`).
const DEVICE_READ_INTERVAL: i64 = MINUTE;
/// `StorageManager`'s low storage defaults.
const DEFAULT_STORAGE_THRESHOLD_PERCENT_LOW: i64 = 5;
const DEFAULT_THRESHOLD_MAX_BYTES: i64 = 500 << 20;
/// `Context.BIND_*` a job's service is bound with; `Intent.FLAG_FROM_BACKGROUND`.
const BIND_AUTO_CREATE: i64 = 0x1;
const BIND_NOT_FOREGROUND: i64 = 0x4;
const BIND_NOT_PERCEPTIBLE: i64 = 0x100;
const BIND_ALMOST_PERCEPTIBLE: i64 = 0x1_0000;
const BIND_NOT_APP_COMPONENT_USAGE: i64 = 0x8000;
const FLAG_FROM_BACKGROUND: i32 = 0x4;
/// `PackageManager.MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE`.
const MATCH_DIRECT_BOOT_ANY: i64 = 0x4_0000 | 0x8_0000;
/// Compat changes: `JobInfo`'s and `JobSchedulerService`'s.
const DISALLOW_DEADLINES_FOR_PREFETCH_JOBS: i64 = 194_532_703;
const REJECT_NEGATIVE_NETWORK_ESTIMATES: i64 = 253_665_015;
const ENFORCE_MINIMUM_TIME_WINDOWS: i64 = 311_402_873;
const REJECT_NEGATIVE_DELAYS_AND_DEADLINES: i64 = 323_349_338;
const REQUIRE_NETWORK_CONSTRAINT_FOR_NETWORK_JOB_WORK_ITEMS: i64 = 241_104_082;
const REQUIRE_NETWORK_PERMISSIONS_FOR_CONNECTIVITY_JOBS: i64 = 271_850_009;
const THROW_ON_UNSUPPORTED_BIAS_USAGE: i64 = 300_477_393;
const OVERRIDE_HANDLE_ABANDONED_JOBS: i64 = 372_529_068;
/// The aconfig flags the service reads.
const FLAG_ENFORCE_MINIMUM_TIME_WINDOWS: &str = "android.app.job.enforce_minimum_time_windows";
const FLAG_HANDLE_ABANDONED_JOBS: &str = "android.app.job.handle_abandoned_jobs";
const FLAG_GET_PENDING_JOB_REASONS_API: &str = "android.app.job.get_pending_job_reasons_api";
const FLAG_ENFORCE_SCHEDULE_LIMIT_TO_PROXY_JOBS: &str = "com.android.server.job.enforce_schedule_limit_to_proxy_jobs";
const FLAG_BATCH_ACTIVE_BUCKET_JOBS: &str = "com.android.server.job.batch_active_bucket_jobs";

/// What the controllers read of the Mac.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Device {
    /// `isPowerConnected`, and `isBatteryCharging` (`isConsideredCharging`).
    power_connected: bool,
    charging: bool,
    battery_not_low: bool,
    storage_not_low: bool,
    battery_saver: bool,
}

/// `JobServiceContext.VERB_*`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verb {
    Binding,
    Starting,
    Executing,
    Stopping,
}

/// `JobStatus`.
struct JobStatus {
    job: JobInfo,
    calling_uid: i32,
    source_uid: i32,
    source_user: i32,
    source_package: String,
    source_tag: Option<String>,
    namespace: Option<String>,
    earliest: i64,
    latest: i64,
    original_latest: i64,
    num_failures: i32,
    num_abandoned_failures: i32,
    num_system_stops: i32,
    last_successful_run: i64,
    last_failed_run: i64,
    cumulative_execution: i64,
    internal_flags: i32,
    pending_work: Vec<JobWorkItem>,
    executing_work: Vec<JobWorkItem>,
    next_work_id: i32,
    enqueue_time: i64,
    first_force_batched: i64,
    /// `serviceProcessName`: the job service's process, while it exists.
    process: Option<String>,
    required: i32,
    /// The constraints satisfied at the last evaluation, the battery
    /// saver's restriction, and the readiness they gave.
    satisfied: i32,
    restricted: bool,
    ready: bool,
    /// `getStopReason`: why it last went from ready to not ready.
    reason_unready: i32,
    abandoned: bool,
    tracked: bool,
    /// The binders among its parcelled extras and clip, held.
    _binders: Arc<Vec<Strong>>,
}

/// Who a job runs for: its source package, user and uid when another app
/// scheduled it, else the scheduling app's own.
struct Source {
    package: String,
    user: i32,
    uid: i32,
    tag: Option<String>,
}

impl JobStatus {
    /// `JobStatus`'s core constructor: the network request bound to the
    /// source uid, the required constraints.
    fn new(
        mut job: JobInfo,
        calling_uid: i32,
        source: Source,
        namespace: Option<String>,
        earliest: i64,
        latest: i64,
        binders: Arc<Vec<Strong>>,
    ) -> JobStatus {
        job.network = job.network.map(|n| n.for_uid(source.uid));
        let mut required = job.constraint_flags;
        if job.network.is_some() {
            required |= CONSTRAINT_CONNECTIVITY;
        }
        if earliest != NO_EARLIEST_RUNTIME {
            required |= CONSTRAINT_TIMING_DELAY;
        }
        if latest != NO_LATEST_RUNTIME {
            required |= CONSTRAINT_DEADLINE;
        }
        if job.is_prefetch() {
            required |= CONSTRAINT_PREFETCH;
        }
        if job.trigger_content_uris.is_some() {
            required |= CONSTRAINT_CONTENT_TRIGGER;
        }
        JobStatus {
            job,
            calling_uid,
            source_uid: source.uid,
            source_user: source.user,
            source_package: source.package,
            source_tag: source.tag,
            namespace,
            earliest,
            latest,
            original_latest: latest,
            num_failures: 0,
            num_abandoned_failures: 0,
            num_system_stops: 0,
            last_successful_run: 0,
            last_failed_run: 0,
            cumulative_execution: 0,
            internal_flags: 0,
            pending_work: Vec::new(),
            executing_work: Vec::new(),
            next_work_id: 1,
            enqueue_time: 0,
            first_force_batched: 0,
            process: None,
            required,
            satisfied: 0,
            restricted: false,
            ready: false,
            reason_unready: STOP_REASON_UNDEFINED,
            abandoned: false,
            tracked: false,
            _binders: binders,
        }
    }

    /// `JobStatus(JobStatus, earliest, latest, ...)`: the job rescheduled.
    fn rescheduled(&self, earliest: i64, latest: i64) -> JobStatus {
        let source = Source {
            package: self.source_package.clone(),
            user: self.source_user,
            uid: self.source_uid,
            tag: self.source_tag.clone(),
        };
        let mut j = JobStatus::new(
            self.job.clone(),
            self.calling_uid,
            source,
            self.namespace.clone(),
            earliest,
            latest,
            self._binders.clone(),
        );
        j.internal_flags = self.internal_flags;
        j
    }

    fn user(&self) -> i32 {
        self.calling_uid / PER_USER_RANGE as i32
    }

    fn matches(&self, uid: i32, namespace: Option<&str>, id: i32) -> bool {
        self.tracked && self.calling_uid == uid && self.namespace.as_deref() == namespace && self.job.id == id
    }

    fn has_deadline(&self) -> bool {
        self.latest != NO_LATEST_RUNTIME
    }

    /// `shouldTreatAsExpeditedJob`: every expedited job is within its
    /// quota, the app host keeping none.
    fn expedited(&self) -> bool {
        self.job.is_expedited()
    }

    /// `getEffectivePriority`.
    fn effective_priority(&self) -> i32 {
        let demoted = self.internal_flags & INTERNAL_FLAG_DEMOTED_BY_USER != 0
            || (self.job.is_user_initiated() && self.internal_flags & INTERNAL_FLAG_DEMOTED_BY_SYSTEM_UIJ != 0);
        let raw = self.job.priority.min(if demoted { PRIORITY_HIGH } else { PRIORITY_MAX });
        if self.num_failures < 2 {
            return raw;
        }
        if self.expedited() {
            return PRIORITY_HIGH;
        }
        match self.num_failures / 2 {
            1 => raw.min(PRIORITY_DEFAULT),
            2 => raw.min(PRIORITY_LOW),
            _ => PRIORITY_MIN,
        }
    }

    /// `enqueueWorkLocked`.
    fn enqueue_work(&mut self, mut work: JobWorkItem) {
        work.work_id = self.next_work_id;
        self.next_work_id += 1;
        self.pending_work.push(work);
    }

    /// `dequeueWorkLocked`.
    fn dequeue_work(&mut self) -> Option<JobWorkItem> {
        if self.pending_work.is_empty() {
            return None;
        }
        let mut work = self.pending_work.remove(0);
        work.delivery_count += 1;
        self.executing_work.push(work.clone());
        Some(work)
    }

    fn work_count(&self) -> usize {
        self.pending_work.len() + self.executing_work.len()
    }

    /// The controllers' evaluation (`evaluateControllerStatesLocked`) at
    /// `now` on `device`, the source app `top` or not: the satisfied
    /// constraints, the readiness (`JobStatus.isReady`) and, when it goes
    /// from ready to not ready, why (`constraintToStopReason`).
    fn evaluate(&mut self, now: i64, device: &Device, top: bool) -> bool {
        let mut satisfied = 0;
        // `BatteryController`: a top app's job of default priority or more
        // needs power only.
        let charging = if top && self.effective_priority() >= PRIORITY_DEFAULT {
            device.power_connected
        } else {
            device.charging && device.battery_not_low
        };
        for (constraint, on) in [
            (CONSTRAINT_CHARGING, charging),
            (CONSTRAINT_BATTERY_NOT_LOW, device.battery_not_low),
            (CONSTRAINT_STORAGE_NOT_LOW, device.storage_not_low),
            // `TimeController`.
            (CONSTRAINT_TIMING_DELAY, now >= self.earliest),
            (CONSTRAINT_DEADLINE, now >= self.latest),
        ] {
            if on {
                satisfied |= constraint;
            }
        }
        // `BackgroundJobsController`: battery saver restricts the jobs of
        // apps not active, but expedited ones.
        let restricted = device.battery_saver && !top && !self.expedited();
        let ready = is_ready(satisfied, self.required, self.job.periodic, restricted, self.process.is_some());
        if self.ready && !ready {
            let lost = self.satisfied & !satisfied & self.required;
            self.reason_unready = if !self.restricted && restricted {
                STOP_REASON_DEVICE_STATE
            } else {
                stop_reason_of(lost, self.required)
            };
        } else if !self.ready && ready {
            self.reason_unready = STOP_REASON_UNDEFINED;
        }
        self.satisfied = satisfied;
        self.restricted = restricted;
        self.ready = ready;
        ready
    }

    /// `JobStatus.getPendingJobReasons(null)` of its last evaluation.
    fn pending_reasons(&self, api_reasons: bool) -> Vec<i32> {
        let mut reasons = Vec::new();
        if self.restricted {
            reasons.push(PENDING_JOB_REASON_DEVICE_STATE);
        }
        reasons.extend(constraints_to_pending_reasons(!self.satisfied & self.required, self.required, api_reasons));
        if reasons.is_empty() {
            reasons.push(if self.process.is_some() { PENDING_JOB_REASON_APP } else { PENDING_JOB_REASON_UNDEFINED });
        }
        reasons
    }

    /// What `JobStore` keeps of it.
    fn stored(&self) -> Stored {
        Stored {
            job: self.job.clone(),
            uid: self.calling_uid,
            source_package: Some(self.source_package.clone()),
            source_user: self.source_user,
            namespace: self.namespace.clone(),
            source_tag: self.source_tag.clone(),
            internal_flags: self.internal_flags,
            last_successful_run: self.last_successful_run,
            last_failed_run: self.last_failed_run,
            cumulative_execution: self.cumulative_execution,
            earliest: self.earliest,
            latest: self.latest,
            work: self.executing_work.iter().chain(&self.pending_work).cloned().collect(),
        }
    }
}

/// `JobStatus.isReady(satisfiedConstraints)`: within quota and dynamic
/// constraints, not dozing (the app host keeps no quota, standby or doze);
/// not restricted in the background, its service present; and its
/// deadline passed (not for a periodic job) or its constraints satisfied.
fn is_ready(satisfied: i32, required: i32, periodic: bool, restricted: bool, has_process: bool) -> bool {
    let interest = required & CONSTRAINTS_OF_INTEREST;
    let deadline = !periodic && required & CONSTRAINT_DEADLINE != 0 && satisfied & CONSTRAINT_DEADLINE != 0;
    !restricted && has_process && (deadline || satisfied & interest == interest)
}

/// `constraintToStopReason` of the first of `lost` it maps.
fn stop_reason_of(lost: i32, required: i32) -> i32 {
    let explicit = |c: i32, reason: i32| if required & c != 0 { reason } else { STOP_REASON_APP_STANDBY };
    for (constraint, reason) in [
        (CONSTRAINT_BATTERY_NOT_LOW, explicit(CONSTRAINT_BATTERY_NOT_LOW, STOP_REASON_CONSTRAINT_BATTERY_NOT_LOW)),
        (CONSTRAINT_CHARGING, explicit(CONSTRAINT_CHARGING, STOP_REASON_CONSTRAINT_CHARGING)),
        (CONSTRAINT_CONNECTIVITY, STOP_REASON_CONSTRAINT_CONNECTIVITY),
        (CONSTRAINT_IDLE, explicit(CONSTRAINT_IDLE, STOP_REASON_CONSTRAINT_DEVICE_IDLE)),
        (CONSTRAINT_STORAGE_NOT_LOW, STOP_REASON_CONSTRAINT_STORAGE_NOT_LOW),
    ] {
        if lost & constraint != 0 {
            return reason;
        }
    }
    STOP_REASON_UNDEFINED
}

/// `JobStatus.constraintsToPendingJobReasons` of `unsatisfied`.
fn constraints_to_pending_reasons(unsatisfied: i32, required: i32, api_reasons: bool) -> Vec<i32> {
    let mut reasons = Vec::new();
    let mut add = |r: i32| {
        if !reasons.contains(&r) {
            reasons.push(r);
        }
    };
    for (c, explicit) in [
        (CONSTRAINT_BATTERY_NOT_LOW, PENDING_JOB_REASON_CONSTRAINT_BATTERY_NOT_LOW),
        (CONSTRAINT_CHARGING, PENDING_JOB_REASON_CONSTRAINT_CHARGING),
        (CONSTRAINT_IDLE, PENDING_JOB_REASON_CONSTRAINT_DEVICE_IDLE),
    ] {
        if unsatisfied & c != 0 {
            add(if required & c != 0 { explicit } else { PENDING_JOB_REASON_APP_STANDBY });
        }
    }
    for (c, r) in [
        (CONSTRAINT_CONNECTIVITY, PENDING_JOB_REASON_CONSTRAINT_CONNECTIVITY),
        (CONSTRAINT_CONTENT_TRIGGER, PENDING_JOB_REASON_CONSTRAINT_CONTENT_TRIGGER),
        (CONSTRAINT_PREFETCH, PENDING_JOB_REASON_CONSTRAINT_PREFETCH),
        (CONSTRAINT_STORAGE_NOT_LOW, PENDING_JOB_REASON_CONSTRAINT_STORAGE_NOT_LOW),
        (CONSTRAINT_TIMING_DELAY, PENDING_JOB_REASON_CONSTRAINT_MINIMUM_LATENCY),
    ] {
        if unsatisfied & c != 0 {
            add(r);
        }
    }
    if api_reasons && unsatisfied & CONSTRAINT_DEADLINE != 0 {
        add(PENDING_JOB_REASON_CONSTRAINT_DEADLINE);
    }
    reasons
}

/// `getRescheduleJobForFailureLocked`'s delay: none before the first
/// attempt, else by policy (linear or exponential, from at least
/// `MIN_BACKOFF_MILLIS`), at most `MAX_BACKOFF_DELAY_MILLIS`.
fn backoff_delay(policy: i32, initial: i64, attempts: i32) -> Option<i64> {
    if attempts == 0 {
        return None;
    }
    let backoff = initial.max(MIN_BACKOFF_MILLIS);
    let delay = if policy == BACKOFF_POLICY_LINEAR {
        backoff.saturating_mul(attempts as i64)
    } else {
        // `(long) Math.scalb(backoff, attempts - 1)`, saturating.
        (backoff as f64 * 2f64.powi(attempts - 1)) as i64
    };
    Some(delay.min(MAX_BACKOFF_DELAY_MILLIS))
}

/// `getRescheduleJobForPeriodic`'s next window, earliest and latest, of a
/// job of `interval` and `flex` whose window ended at `original_latest`.
fn periodic_window(interval: i64, flex: i64, original_latest: i64, now: i64) -> (i64, i64) {
    let period = interval.clamp(MIN_PERIOD_MILLIS, MAX_ALLOWED_PERIOD_MS).max(MIN_PERIOD_MILLIS);
    let flex = flex.min(period).max(MIN_FLEX_MILLIS);
    let olrte = if original_latest < 0 || original_latest == NO_LATEST_RUNTIME { now } else { original_latest };
    let diff = (now - olrte).abs();
    let mut buffer = 0;
    let latest = if now > olrte {
        // It ran past its window: the next window after the ones skipped.
        let mut skipped = diff / period + 1;
        if period != flex && (period - flex - (diff % period)) <= flex / 6 {
            skipped += 1;
        }
        olrte + period * skipped
    } else {
        if diff < PERIODIC_JOB_WINDOW_BUFFER && diff < period / 6 {
            buffer = PERIODIC_JOB_WINDOW_BUFFER.min(period / 6 - diff);
        }
        olrte + period
    };
    if latest < now {
        return (now + period - flex, now + period);
    }
    (latest - flex.min(period - buffer), latest)
}

/// The original's `DEFAULT_CONCURRENCY_LIMIT` for the Mac's memory (what
/// `MemInfoReader` reads of the guest's `/proc/meminfo`).
fn default_concurrency_limit() -> usize {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: a u64 out-parameter of its size.
    unsafe {
        libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut bytes as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0)
    };
    let gb = 1_000_000_000;
    match bytes {
        b if b <= 6 * gb => 16,
        b if b <= 8 * gb => 20,
        b if b <= 12 * gb => 32,
        _ => 40,
    }
}

/// Why a job stopped, as `JobParameters.setStopReason` keeps it.
#[derive(Clone)]
struct StopReason {
    reason: i32,
    internal: i32,
    debug: Option<String>,
}

/// A `JobServiceContext` running a job.
struct Context {
    key: u64,
    callback: Arc<JobCallback>,
    callback_binder: Binder,
    conn: Arc<dyn HostConnection>,
    verb: Verb,
    service: Option<Strong>,
    cancelled: bool,
    stop: StopReason,
    override_deadline_expired: bool,
    expedited: bool,
    execution_start: i64,
    last_stamp: i64,
    min_guarantee: i64,
    max_execution: i64,
    timeout_at: i64,
    pending_stop: Option<StopReason>,
}

/// The handler's messages.
enum Message {
    /// `ServiceConnection`'s calls for the context of `callback`.
    Connected { callback: u64, service: Option<Binder>, dead: bool },
    /// `MSG_CHECK_JOB`.
    Check,
    /// The store's write scheduled (`postDelayed(mWriteRunnable)`): the
    /// handler times it.
    Persist,
}

#[derive(Default)]
struct State {
    loaded: bool,
    /// `mReadyToRock`: jobs run once the app host has started.
    running: bool,
    /// Every job tracked or running, by identity.
    jobs: BTreeMap<u64, JobStatus>,
    next_key: u64,
    /// `mPendingJobQueue`.
    pending: Vec<u64>,
    contexts: Vec<Context>,
    next_callback: u64,
    device: Option<(Device, i64)>,
    /// Uids whose file is to be written, and when; whether the legacy
    /// file is to go.
    dirty: BTreeSet<i32>,
    write_at: Option<i64>,
    migrate: bool,
    /// The persisted schedules of each user's package in the last window.
    schedule_calls: HashMap<(i32, String), VecDeque<i64>>,
}

pub struct JobScheduler {
    me: Weak<JobScheduler>,
    host: Arc<AppHost>,
    state: Mutex<State>,
    queue: Mutex<Vec<Message>>,
    wake: Condvar,
    limit: usize,
}

/// The connection of a context to its job's service.
struct Connection {
    scheduler: Weak<JobScheduler>,
    callback: u64,
}

impl HostConnection for Connection {
    fn connected(&self, service: Option<Binder>, dead: bool) {
        if let Some(s) = self.scheduler.upgrade() {
            s.post(Message::Connected { callback: self.callback, service, dead });
        }
    }
}

/// How a context's job last stopped, kept by its callback
/// (`mStoppedReason`, `mStoppedTime`, `mPreviousJobHadSuccessfulFinish`).
struct Stopped {
    reason: Option<String>,
    at: i64,
    successful: bool,
}

/// `JobServiceContext.JobCallback`.
struct JobCallback {
    scheduler: Weak<JobScheduler>,
    id: u64,
    stopped: Mutex<Option<Stopped>>,
}

impl JobScheduler {
    pub fn new(host: Arc<AppHost>) -> Arc<JobScheduler> {
        let scheduler = Arc::new_cyclic(|me| JobScheduler {
            me: me.clone(),
            host,
            state: Mutex::new(State::default()),
            queue: Mutex::new(Vec::new()),
            wake: Condvar::new(),
            limit: default_concurrency_limit(),
        });
        let weak = Arc::downgrade(&scheduler);
        let _ = std::thread::Builder::new().name("jobs".into()).spawn(move || run(weak));
        scheduler
    }

    /// `onBootPhase(PHASE_THIRD_PARTY_APPS_CAN_START)`: the persisted jobs
    /// read, and jobs run from now on.
    pub fn start(&self) {
        let mut state = self.state.lock().unwrap();
        if let Err(e) = self.ensure_loaded(&mut state) {
            eprintln!("app host: jobs: {}", e.message);
            return;
        }
        state.running = true;
        drop(state);
        self.post(Message::Check);
    }

    fn post(&self, m: Message) {
        self.queue.lock().unwrap().push(m);
        self.wake.notify_all();
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    fn change(&self, change: i64, uid: i32) -> Result<bool, Exception> {
        self.host.change_enabled_for_uid(change, uid)
    }

    /// Whether `uid` has a process: the app host's processes are all top.
    fn is_top(&self, uid: i32) -> bool {
        self.host.apps.lock().unwrap().values().any(|a| a.uid as i32 == uid)
    }

    /// `PackageManagerInternal.getPackageUid(package, 0, user)`.
    fn package_uid(&self, package: &str, user: i32) -> Result<Option<i32>, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.packages.get(package).map(|ps| user * PER_USER_RANGE as i32 + ps.app_id))
    }

    /// `getServiceInfo(service, MATCH_DIRECT_BOOT_AWARE | UNAWARE)` in
    /// `user`, as `uid` asks it.
    fn service_info(
        &self,
        service: &(String, String),
        user: i32,
        uid: i32,
    ) -> Result<Option<crate::package::info::ServiceInfo>, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query { state: &state, filter: &resolution.apps_filter, calling_uid: uid };
        let component =
            crate::package::intent::ComponentName { package: service.0.clone(), class: service.1.clone() };
        match query.service_info(&component, MATCH_DIRECT_BOOT_ANY, user) {
            Ok(Ok(info)) => Ok(info),
            Ok(Err(e)) => Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => {
                Err(Exception::new(EX_ILLEGAL_STATE, format!("getServiceInfo: {reason}")))
            }
        }
    }

    /// The process of a job's service (`serviceProcessName`), if it has one.
    fn process_of(&self, j: &JobStatus) -> Option<String> {
        let info = self.service_info(&j.job.service, j.user(), 1000).ok().flatten()?;
        Some(info.info.process_name.unwrap_or_else(|| j.job.service.0.clone()))
    }

    /// The Mac's battery, storage and battery saver, read each
    /// `DEVICE_READ_INTERVAL`.
    fn device(&self, state: &mut State, now: i64) -> Device {
        if let Some((d, at)) = state.device
            && now - at < DEVICE_READ_INTERVAL
        {
            return d;
        }
        let d = self.read_device();
        state.device = Some((d, now));
        d
    }

    /// BatteryService's plug state and `mBatteryLevelLow` (its low warning
    /// level from `low_power_trigger_level` or the framework's, not below
    /// the critical one), DeviceStorageMonitorService's low storage of the
    /// volume of `/data`, and battery saver.
    fn read_device(&self) -> Device {
        let battery = aim_host_health::battery();
        let plugged = battery.ac_online != 0;
        let warning = self.host.inputs().ok().map_or(20, |inputs| {
            let default = inputs.packages.framework_integer("config_lowBatteryWarningLevel").unwrap_or(20);
            let critical = inputs.packages.framework_integer("config_criticalBatteryWarningLevel").unwrap_or(0);
            let set = inputs.settings.get(super::settings::Table::Global, "low_power_trigger_level");
            let level = set.and_then(|v| v.parse().ok()).filter(|&l| l != 0).unwrap_or(default);
            level.max(critical)
        });
        let low = !plugged
            && battery.status != aim_hostcall::health::status::UNKNOWN
            && battery.level_percent <= warning;
        let battery_saver = self.host.power_service.get().is_some_and(|p| p.power_save_mode().unwrap_or(false));
        Device {
            power_connected: plugged,
            charging: plugged,
            battery_not_low: !low,
            storage_not_low: self.storage_not_low(),
            battery_saver,
        }
    }

    /// `StorageManager.getStorageLowBytes` of the data volume against what
    /// it has free.
    fn storage_not_low(&self) -> bool {
        let Ok(inputs) = self.host.inputs() else { return true };
        let Ok(path) = std::ffi::CString::new(inputs.data.as_os_str().as_encoded_bytes()) else { return true };
        // SAFETY: statvfs into a local struct.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
            return true;
        }
        let total = st.f_blocks as i64 * st.f_frsize as i64;
        let usable = st.f_bavail as i64 * st.f_frsize as i64;
        let setting = |name: &str| inputs.settings.get(super::settings::Table::Global, name).and_then(|v| v.parse::<i64>().ok());
        let percent = setting("sys_storage_threshold_percentage").unwrap_or(DEFAULT_STORAGE_THRESHOLD_PERCENT_LOW);
        let max = setting("sys_storage_threshold_max_bytes").unwrap_or(DEFAULT_THRESHOLD_MAX_BYTES);
        usable > (total * percent / 100).min(max)
    }

    /// The JobStore read, once (`JobStore.initAndGet`).
    fn ensure_loaded(&self, state: &mut State) -> Result<(), Exception> {
        if state.loaded {
            return Ok(());
        }
        let dir = self.host.inputs()?.data.join("system/job");
        let (now, wall) = (elapsed_ms(), wall_ms());
        let (stored, migrate) = job_store::read(&dir, now, wall);
        state.loaded = true;
        let count = stored.len();
        for s in stored {
            let uid = s.uid;
            let mut source = Source {
                package: s.job.service.0.clone(),
                user: uid / PER_USER_RANGE as i32,
                uid,
                tag: None,
            };
            if s.source_user != -1
                && let Some(package) = s.source_package
                && let Some(source_uid) = self.package_uid(&package, s.source_user)?
            {
                source = Source { package, user: s.source_user, uid: source_uid, tag: s.source_tag };
            }
            let mut j = JobStatus::new(s.job, uid, source, s.namespace, s.earliest, s.latest, Arc::new(Vec::new()));
            j.last_successful_run = s.last_successful_run;
            j.last_failed_run = s.last_failed_run;
            j.cumulative_execution = s.cumulative_execution;
            j.internal_flags = s.internal_flags;
            for w in s.work {
                j.enqueue_work(w);
            }
            if migrate {
                state.dirty.insert(uid);
            }
            self.track(state, j, now);
        }
        if migrate {
            state.migrate = true;
            state.write_at = Some(now);
        }
        eprintln!("app host: jobs: {count} persisted jobs read");
        Ok(())
    }

    /// `startTrackingJobLocked` (and `JobStore.add`).
    fn track(&self, state: &mut State, mut j: JobStatus, now: i64) -> u64 {
        j.enqueue_time = now;
        j.tracked = true;
        // A user-initiated job (only read from the store: `schedule`
        // refuses them) is kept, not run.
        j.process = if j.job.is_user_initiated() {
            self.host.unimplemented_named("jobscheduler", "user-initiated jobs (#1490)");
            None
        } else {
            self.process_of(&j)
        };
        let persisted = j.job.persisted.then_some(j.calling_uid);
        state.next_key += 1;
        let key = state.next_key;
        state.jobs.insert(key, j);
        if let Some(uid) = persisted {
            self.persist(state, uid, now);
        }
        key
    }

    /// `stopTrackingJobLocked`: its work handed to `incoming`, removed from
    /// the store; whether it was tracked.
    fn stop_tracking(&self, state: &mut State, key: u64, incoming: Option<&mut JobStatus>, write: bool, now: i64) -> bool {
        let running = state.contexts.iter().any(|c| c.key == key);
        let Some(j) = state.jobs.get_mut(&key) else { return false };
        // `JobStatus.stopTrackingJobLocked`.
        if let Some(incoming) = incoming {
            if !j.executing_work.is_empty() {
                incoming.pending_work = std::mem::take(&mut j.executing_work);
            }
            incoming.pending_work.append(&mut j.pending_work);
            incoming.next_work_id = j.next_work_id;
        } else {
            j.pending_work.clear();
            j.executing_work.clear();
        }
        let was = std::mem::replace(&mut j.tracked, false);
        let persisted = j.job.persisted.then_some(j.calling_uid);
        if !running {
            state.jobs.remove(&key);
        }
        state.pending.retain(|k| *k != key);
        if was && write && let Some(uid) = persisted {
            self.persist(state, uid, now);
        }
        was
    }

    /// `JobStore.touchJob` of a job whose work changed.
    fn touch(&self, state: &mut State, key: u64, now: i64) {
        if let Some(uid) = state.jobs.get(&key).filter(|j| j.tracked && j.job.persisted).map(|j| j.calling_uid) {
            self.persist(state, uid, now);
        }
    }

    /// `JobStore.maybeWriteStatusToDiskAsync` for `uid`: written after
    /// `JOB_PERSIST_DELAY`.
    fn persist(&self, state: &mut State, uid: i32, now: i64) {
        state.dirty.insert(uid);
        if state.write_at.is_none() {
            state.write_at = Some(now + JOB_PERSIST_DELAY);
            self.post(Message::Persist);
        }
    }

    fn find(state: &State, uid: i32, namespace: Option<&str>, id: i32) -> Option<u64> {
        state.jobs.iter().find(|(_, j)| j.matches(uid, namespace, id)).map(|(k, _)| *k)
    }

    /// `JobStubs.enforceValidJobRequest`.
    fn enforce_valid_job_request(&self, uid: i32, job: &JobInfo) -> Result<(), Exception> {
        let (package, class) = &job.service;
        let name = format!("ComponentInfo{{{package}/{class}}}");
        let Some(si) = self.service_info(&job.service, uid / PER_USER_RANGE as i32, uid)? else {
            return Err(Exception::illegal_argument(format!(
                "Tried to schedule job for non-existent component: {name}"
            )));
        };
        if si.info.application_info.uid != uid {
            return Err(Exception::illegal_argument(format!("uid {uid} cannot schedule job in {package}")));
        }
        if si.permission.as_deref() != Some("android.permission.BIND_JOB_SERVICE") {
            return Err(Exception::illegal_argument(format!(
                "Scheduled service {name} does not require android.permission.BIND_JOB_SERVICE permission"
            )));
        }
        if job.persisted && self.host.check_permission(Some("android.permission.RECEIVE_BOOT_COMPLETED"), uid)? != 0 {
            return Err(Exception::illegal_argument(
                "Requested job cannot be persisted without holding android.permission.RECEIVE_BOOT_COMPLETED permission",
            ));
        }
        if job.network.is_some()
            && self.change(REQUIRE_NETWORK_PERMISSIONS_FOR_CONNECTIVITY_JOBS, uid)?
            && self.host.check_permission(Some("android.permission.ACCESS_NETWORK_STATE"), uid)? != 0
        {
            return Err(Exception::security(
                "android.permission.ACCESS_NETWORK_STATE required for jobs with a connectivity constraint",
            ));
        }
        Ok(())
    }

    /// `JobStubs.validateJob`.
    fn validate_job(&self, job: &mut JobInfo, uid: i32, work: Option<&JobWorkItem>) -> Result<(), Exception> {
        let reject_negative = self.change(REJECT_NEGATIVE_NETWORK_ESTIMATES, uid)?;
        job.enforce_validity(
            self.change(DISALLOW_DEADLINES_FOR_PREFETCH_JOBS, uid)?,
            reject_negative,
            self.change(ENFORCE_MINIMUM_TIME_WINDOWS, uid)? && self.flag(FLAG_ENFORCE_MINIMUM_TIME_WINDOWS)?,
            self.change(REJECT_NEGATIVE_DELAYS_AND_DEADLINES, uid)?,
        )?;
        if job.flags & FLAG_WILL_BE_FOREGROUND != 0
            && self.host.check_permission(Some("android.permission.CONNECTIVITY_INTERNAL"), uid)? != 0
        {
            return Err(Exception::security(format!(
                "JobScheduler: Neither user {uid} nor current process has android.permission.CONNECTIVITY_INTERNAL."
            )));
        }
        if job.flags & FLAG_EXEMPT_FROM_APP_STANDBY != 0 && uid != 1000 {
            return Err(Exception::security("Job has invalid flags"));
        }
        if job.is_user_initiated() {
            return Err(self.host.unimplemented_named("jobscheduler", "user-initiated jobs (#1490)"));
        }
        if let Some(work) = work {
            work.enforce_validity(reject_negative)?;
            let bytes = [work.network_download_bytes, work.network_upload_bytes, work.minimum_chunk_bytes];
            if bytes.iter().any(|b| *b != NETWORK_BYTES_UNKNOWN) && job.network.is_none() {
                let message = "JobWorkItem implies network usage but job doesn't specify a network constraint";
                if self.change(REQUIRE_NETWORK_CONSTRAINT_FOR_NETWORK_JOB_WORK_ITEMS, uid)? {
                    return Err(Exception::illegal_argument(message));
                }
                eprintln!("app host: jobs: {message}");
            }
            if job.persisted && work.intent.is_some() {
                return Err(Exception::illegal_argument("Cannot persist JobWorkItems with Intents"));
            }
        }
        Ok(())
    }

    /// The `schedule`, `enqueue` and `scheduleAsPackage` stubs, then
    /// `scheduleAsPackage`: `proxy` is the package, user and tag a job is
    /// scheduled for.
    fn schedule(
        &self,
        uid: i32,
        namespace: Option<String>,
        job: Option<job_info::Read>,
        work: Option<Option<ReadWorkItem>>,
        proxy: Option<(Option<String>, i32, Option<String>)>,
    ) -> Result<i32, Exception> {
        let Some(job) = job else {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke virtual method 'android.content.ComponentName android.app.job.JobInfo.getService()' on a null object reference",
            ));
        };
        let mut job = job.map_err(|u| {
            self.host.unimplemented_named("jobscheduler", &format!("a job's network request with {} (#1490)", u.0))
        })?;
        let user = uid / PER_USER_RANGE as i32;
        let proxy = match proxy {
            Some((None, ..)) => {
                return Err(Exception::new(EX_NULL_POINTER, "Must specify a package for scheduleAsPackage()"));
            }
            Some((Some(package), user, tag)) => {
                if self.host.check_permission(Some("android.permission.UPDATE_DEVICE_STATS"), uid)? != 0 {
                    return Err(Exception::security(format!(
                        "Caller uid {uid} not permitted to schedule jobs for other apps"
                    )));
                }
                Some((package, user, tag))
            }
            None => None,
        };
        self.enforce_valid_job_request(uid, &job)?;
        let work = match work {
            Some(None) => return Err(Exception::new(EX_NULL_POINTER, "work is null")),
            Some(Some(w)) => {
                if w.has_fds() {
                    return Err(Exception::illegal_argument("File descriptors passed in Intent"));
                }
                Some(w.keep(&self.host.process))
            }
            None => None,
        };
        self.validate_job(&mut job, uid, work.as_ref())?;
        // `prepareLocked` and `enqueueWorkLocked` grant URIs.
        if job.clip.as_ref().is_some_and(|(_, grants)| grants & FLAG_GRANT_URI_PERMISSIONS != 0)
            || work.as_ref().and_then(JobWorkItem::intent_flags).is_some_and(|f| f & FLAG_GRANT_URI_PERMISSIONS != 0)
        {
            return Err(self.host.unimplemented_named("jobscheduler", "a job's URI grants (#1503)"));
        }
        if job.trigger_content_uris.is_some() {
            return Err(self.host.unimplemented_named("jobscheduler", "content trigger jobs (#1490)"));
        }
        // `validateNamespace`.
        let namespace = match namespace.map(|n| n.trim().to_string()) {
            Some(n) if n.is_empty() => return Err(Exception::illegal_argument("namespace cannot be empty")),
            Some(n) if n.encode_utf16().count() > 1000 => {
                return Err(Exception::illegal_argument("namespace cannot be more than 1000 characters"));
            }
            n => n,
        };
        // `enforceBuilderApiPermissions`.
        if job.bias != BIAS_DEFAULT && self.host.check_permission(Some("android.permission.UPDATE_DEVICE_STATS"), uid)? != 0 {
            if self.change(THROW_ON_UNSUPPORTED_BIAS_USAGE, uid)? {
                return Err(Exception::security("Apps may not call setBias()"));
            }
            eprintln!("app host: jobs: uid {uid} set bias on its job");
            job.bias = BIAS_DEFAULT;
        }
        let binders = Arc::new(job.handles().into_iter().map(|h| self.host.process.strong(h)).collect());
        let mut state = self.state.lock().unwrap();
        self.ensure_loaded(&mut state)?;
        let now = elapsed_ms();
        // The persisted schedules' quota.
        let service_package = job.service.0.clone();
        let proxy_package = proxy.as_ref().map(|(p, ..)| p.clone());
        if job.persisted
            && (self.flag(FLAG_ENFORCE_SCHEDULE_LIMIT_TO_PROXY_JOBS)?
                || proxy_package.as_ref().is_none_or(|p| *p == service_package))
        {
            let package = proxy_package.unwrap_or_else(|| service_package.clone());
            let calls = state.schedule_calls.entry((user, package.clone())).or_default();
            while calls.front().is_some_and(|t| now - t >= API_QUOTA_SCHEDULE_WINDOW_MS) {
                calls.pop_front();
            }
            if calls.len() >= API_QUOTA_SCHEDULE_COUNT {
                eprintln!("app host: jobs: {user}-{package} has called schedule() too many times");
                let (model, _) = self.host.packages()?;
                let debuggable = model
                    .packages
                    .get(&package)
                    .and_then(|ps| ps.pkg.as_ref())
                    .is_some_and(|p| p.is(crate::package::pkg::booleans::DEBUGGABLE));
                if debuggable {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!(
                            "schedule()/enqueue() called more than {API_QUOTA_SCHEDULE_COUNT} times in the past {API_QUOTA_SCHEDULE_WINDOW_MS}ms. See the documentation for more information."
                        ),
                    ));
                }
            }
            calls.push_back(now);
        }
        let existing = Self::find(&state, uid, namespace.as_deref(), job.id);
        // Work added to the same job is enqueued in it.
        if let (Some(work), Some(key)) = (&work, existing) {
            let j = state.jobs.get_mut(&key).unwrap();
            if j.job.same_as(&job) {
                if j.work_count() >= MAX_NUM_PERSISTED_JOB_WORK_ITEMS && j.job.persisted {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("Apps may not persist more than {MAX_NUM_PERSISTED_JOB_WORK_ITEMS} JobWorkItems per job"),
                    ));
                }
                j.enqueue_work(work.clone());
                self.touch(&mut state, key, now);
                return Ok(RESULT_SUCCESS);
            }
        }
        // `JobStatus.createFromJobInfo`.
        let (earliest, latest) = if job.periodic {
            let period = job.interval.clamp(MIN_PERIOD_MILLIS, MAX_ALLOWED_PERIOD_MS).max(MIN_PERIOD_MILLIS);
            let latest = now + period;
            (latest - job.flex.min(period).max(MIN_FLEX_MILLIS), latest)
        } else {
            (
                if job.has_early_constraint { now + job.min_latency } else { NO_EARLIEST_RUNTIME },
                if job.has_late_constraint { now + job.max_execution_delay } else { NO_LATEST_RUNTIME },
            )
        };
        let mut source = Source { package: service_package, user, uid, tag: None };
        if let Some((package, source_user, tag)) = &proxy
            && *source_user != -1
            && let Some(source_uid) = self.package_uid(package, *source_user)?
        {
            source = Source { package: package.clone(), user: *source_user, uid: source_uid, tag: tag.clone() };
        }
        let mut j = JobStatus::new(job, uid, source, namespace, earliest, latest, binders);
        // Jobs on behalf of others do not count toward the app's limit.
        if proxy.is_none() && state.jobs.values().filter(|j| j.tracked && j.calling_uid == uid).count() > MAX_JOBS_PER_APP {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("Apps may not schedule more than {MAX_JOBS_PER_APP} distinct jobs"),
            ));
        }
        if let Some(key) = existing {
            let old = &state.jobs[&key];
            if work.is_some() && old.job.persisted && old.work_count() >= MAX_NUM_PERSISTED_JOB_WORK_ITEMS {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("Apps may not persist more than {MAX_NUM_PERSISTED_JOB_WORK_ITEMS} JobWorkItems per job"),
                ));
            }
            // The job replaces the one of its id.
            self.cancel_impl(&mut state, key, Some(&mut j), STOP_REASON_CANCELLED_BY_APP, INTERNAL_STOP_REASON_CANCELED, "job rescheduled by app", now);
        }
        if let Some(work) = work {
            j.enqueue_work(work);
        }
        let key = self.track(&mut state, j, now);
        if self.ready_to_execute(&mut state, key, now) {
            state.pending.push(key);
            self.run_pending(&mut state, now);
        }
        Ok(RESULT_SUCCESS)
    }

    /// `cancelJobImplLocked`: the job removed (replaced by `incoming`, which
    /// the caller tracks), and stopped if it runs.
    #[allow(clippy::too_many_arguments)]
    fn cancel_impl(
        &self,
        state: &mut State,
        key: u64,
        incoming: Option<&mut JobStatus>,
        reason: i32,
        internal: i32,
        debug: &str,
        now: i64,
    ) {
        self.stop_tracking(state, key, incoming, true, now);
        if let Some(i) = state.contexts.iter().position(|c| c.key == key) {
            self.cancel_context(state, i, StopReason { reason, internal, debug: Some(debug.into()) });
        }
    }

    /// `cancelJobsForUid` of jobs the uid scheduled (in `namespace` only, if
    /// asked).
    fn cancel_for_uid(&self, uid: i32, namespace: Option<Option<String>>, debug: &str) -> Result<(), Exception> {
        if uid == 1000 && namespace.as_ref().is_none_or(Option::is_none) {
            eprintln!("app host: jobs: can't cancel all jobs for system uid");
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        self.ensure_loaded(&mut state)?;
        let now = elapsed_ms();
        let keys: Vec<u64> = state
            .jobs
            .iter()
            .filter(|(_, j)| j.tracked && j.calling_uid == uid)
            .filter(|(_, j)| namespace.as_ref().is_none_or(|n| *n == j.namespace))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            self.cancel_impl(&mut state, key, None, STOP_REASON_CANCELLED_BY_APP, INTERNAL_STOP_REASON_CANCELED, debug, now);
        }
        Ok(())
    }

    /// `isReadyToBeExecutedLocked`: ready, the user started, not pending,
    /// not running.
    fn ready_to_execute(&self, state: &mut State, key: u64, now: i64) -> bool {
        if !state.running {
            return false;
        }
        let device = self.device(state, now);
        let Some(j) = state.jobs.get_mut(&key) else { return false };
        let top = self.is_top(j.source_uid);
        j.tracked && j.evaluate(now, &device, top) && !state.pending.contains(&key) && !state.contexts.iter().any(|c| c.key == key)
    }

    /// `maybeQueueReadyJobsForExecutionLocked`: running jobs no longer
    /// ready stopped, the ready ones queued (those of apps not running
    /// batched, as `MaybeReadyJobQueueFunctor` batches CPU-only jobs), and
    /// `maybeRunPendingJobsLocked`.
    fn check(&self, state: &mut State, now: i64) {
        if !state.running {
            return;
        }
        state.pending.clear();
        let device = self.device(state, now);
        let batch_flag = self.flag(FLAG_BATCH_ACTIVE_BUCKET_JOBS).unwrap_or(false);
        let mut stops = Vec::new();
        let mut unbatched = Vec::new();
        let mut batched = Vec::new();
        let keys: Vec<u64> = state.jobs.keys().copied().collect();
        for key in keys {
            let running = state.contexts.iter().position(|c| c.key == key);
            let j = state.jobs.get_mut(&key).unwrap();
            if !j.tracked {
                continue;
            }
            let top = self.is_top(j.source_uid);
            if !j.evaluate(now, &device, top) {
                if let Some(i) = running {
                    stops.push((i, j.reason_unready));
                }
                continue;
            }
            // `shouldForceBatchJob`, for jobs of no network (which never
            // run here) and no standby buckets (all are active).
            let until_deadline = if j.has_deadline() { j.latest - now } else { i64::MAX };
            let force_batch = !j.expedited()
                && j.num_failures + j.num_system_stops == 0
                && batch_flag
                && self.limit / 3 > 1
                && until_deadline > MAX_CPU_ONLY_JOB_BATCH_DELAY_MS / 2
                && !top
                && j.job.flags & FLAG_EXEMPT_FROM_APP_STANDBY == 0
                && !(j.first_force_batched > 0 && now - j.first_force_batched >= MAX_CPU_ONLY_JOB_BATCH_DELAY_MS);
            if force_batch && j.first_force_batched == 0 {
                j.first_force_batched = now;
            }
            if running.is_none() {
                if force_batch { batched.push(key) } else { unbatched.push(key) }
            }
        }
        for (i, reason) in stops {
            self.cancel_context(
                state,
                i,
                StopReason {
                    reason,
                    internal: INTERNAL_STOP_REASON_CONSTRAINTS_NOT_SATISFIED,
                    debug: Some("cancelled due to unsatisfied constraints".into()),
                },
            );
        }
        // `postProcessLocked`: the batch goes with any other ready job, or
        // once it has `MIN_READY_CPU_ONLY_JOBS_COUNT`.
        if !unbatched.is_empty() || batched.len() >= (self.limit / 3).min(3) {
            unbatched.extend(batched);
        }
        state.pending = unbatched;
        self.run_pending(state, now);
    }

    /// `JobConcurrencyManager.assignJobsToContextsLocked`, for the work
    /// types the app host has: the screen-on total (`screen_on_normal`),
    /// apps not running held to the background share and their packages
    /// to `pkg_concurrency_limit_*`, the expedited and higher priority
    /// first.
    fn run_pending(&self, state: &mut State, now: i64) {
        let max_total = self.limit * 3 / 4;
        let mut queue = std::mem::take(&mut state.pending);
        queue.sort_by_key(|k| {
            let j = &state.jobs[k];
            (!j.expedited(), -j.effective_priority(), j.enqueue_time)
        });
        let mut left = Vec::new();
        for key in queue {
            if state.contexts.len() >= max_total {
                left.push(key);
                continue;
            }
            let j = &state.jobs[&key];
            if !self.is_top(j.source_uid) {
                let expedited = j.expedited();
                let running: Vec<&JobStatus> = state
                    .contexts
                    .iter()
                    .map(|c| &state.jobs[&c.key])
                    .filter(|r| !self.is_top(r.source_uid))
                    .collect();
                let same_package =
                    running.iter().filter(|r| r.source_package == j.source_package && r.expedited() == expedited).count();
                let package_limit = if expedited { 3 } else { self.limit / 2 };
                if (!expedited && running.len() >= self.limit / 2) || same_package >= package_limit {
                    left.push(key);
                    continue;
                }
            }
            if !self.execute(state, key, now) {
                eprintln!("app host: jobs: error executing job {key}");
            }
        }
        state.pending = left;
    }

    /// `JobServiceContext.executeRunnableJob`: the job's service bound.
    fn execute(&self, state: &mut State, key: u64, now: i64) -> bool {
        state.next_callback += 1;
        let id = state.next_callback;
        let callback = Arc::new(JobCallback { scheduler: self.me.clone(), id, stopped: Mutex::new(None) });
        let callback_binder = self.host.process.add_service(callback.clone());
        let conn: Arc<dyn HostConnection> = Arc::new(Connection { scheduler: self.me.clone(), callback: id });
        let j = &state.jobs[&key];
        let expedited = j.expedited();
        let min_guarantee = if expedited { RUNTIME_MIN_EJ_GUARANTEE_MS } else { RUNTIME_MIN_GUARANTEE_MS };
        let max = if expedited { RUNTIME_MIN_GUARANTEE_MS } else { RUNTIME_FREE_QUOTA_MAX_LIMIT_MS };
        let mut flags = BIND_AUTO_CREATE | BIND_NOT_APP_COMPONENT_USAGE | BIND_NOT_FOREGROUND;
        flags |= if expedited { BIND_ALMOST_PERCEPTIBLE } else { BIND_NOT_PERCEPTIBLE };
        let (package, class) = j.job.service.clone();
        let user = j.user();
        let context = Context {
            key,
            callback,
            callback_binder,
            conn: conn.clone(),
            verb: Verb::Binding,
            service: None,
            cancelled: false,
            stop: StopReason { reason: STOP_REASON_UNDEFINED, internal: INTERNAL_STOP_REASON_UNKNOWN, debug: None },
            override_deadline_expired: j.has_deadline() && j.latest < now,
            expedited,
            execution_start: now,
            last_stamp: now,
            min_guarantee,
            max_execution: max.max(min_guarantee),
            timeout_at: now + OP_BIND_TIMEOUT_MILLIS,
            pending_stop: None,
        };
        state.contexts.push(context);
        let bound = self.host.bind_service_from_host(&package, &class, FLAG_FROM_BACKGROUND, flags, user, conn.clone());
        match bound {
            Ok(true) => {
                eprintln!("app host: jobs: running {package}/{class} #{}", state.jobs[&key].job.id);
                true
            }
            other => {
                if let Err(e) = other {
                    eprintln!("app host: jobs: job service {package}/{class} cannot be executed: {}", e.message);
                }
                self.host.unbind_service_from_host(&conn);
                state.contexts.retain(|c| c.key != key);
                false
            }
        }
    }

    /// `onServiceConnected`, `onServiceDisconnected`, `onBindingDied` and
    /// `onNullBinding` of the context of `callback`.
    fn connected(&self, state: &mut State, callback: u64, service: Option<Binder>, dead: bool, now: i64) {
        let Some(i) = state.contexts.iter().position(|c| c.callback.id == callback) else { return };
        match service {
            Some(Binder::Handle(h)) => {
                let strong = self.host.process.strong(h);
                let c = &mut state.contexts[i];
                c.service = Some(strong);
                // `handleServiceBoundLocked`.
                if c.verb != Verb::Binding {
                    return self.close(state, i, false, Some("started job not pending"), now);
                }
                if c.cancelled {
                    return self.close(state, i, true, Some("cancelled while waiting for bind"), now);
                }
                c.verb = Verb::Starting;
                c.timeout_at = now + OP_TIMEOUT_MILLIS;
                self.send(state, i, job_service::START_JOB);
            }
            _ if dead => self.close(state, i, true, Some("unexpectedly disconnected"), now),
            _ => self.close(state, i, false, Some("null binding"), now),
        }
    }

    /// `IJobService.startJob` or `stopJob` with the context's parameters.
    fn send(&self, state: &State, i: usize, code: u32) {
        let c = &state.contexts[i];
        let j = &state.jobs[&c.key];
        let params = JobParameters {
            callback: c.callback_binder,
            namespace: j.namespace.as_deref(),
            job: &j.job,
            override_deadline_expired: c.override_deadline_expired,
            expedited: c.expedited,
            user_initiated: false,
            stop_reason: c.stop.reason,
            internal_stop_reason: c.stop.internal,
            debug_stop_reason: c.stop.debug.as_deref(),
        };
        let mut p = Parcel::new();
        if code == job_service::START_JOB {
            job_service::StartJob { job_params: Some(params) }.write(&mut p);
        } else {
            job_service::StopJob { job_params: Some(params) }.write(&mut p);
        }
        if let Some(Err(s)) = c.service.as_ref().map(|s| s.transact(code, &p, true)) {
            eprintln!("app host: jobs: {}/{}: call {code}: status {s}", j.job.service.0, j.job.service.1);
        }
    }

    /// `doCancelLocked` of context `i`.
    fn cancel_context(&self, state: &mut State, i: usize, stop: StopReason) {
        let now = elapsed_ms();
        let c = &mut state.contexts[i];
        let verb = c.verb;
        if verb == Verb::Stopping {
            return;
        }
        let debug = stop.debug.clone();
        c.stop = stop;
        match verb {
            Verb::Binding | Verb::Starting => {
                c.cancelled = true;
                c.callback.stopped_because(debug, now);
            }
            Verb::Executing => self.send_stop(state, i, debug, now),
            Verb::Stopping => {}
        }
    }

    /// `sendStopMessageLocked`.
    fn send_stop(&self, state: &mut State, i: usize, reason: Option<String>, now: i64) {
        let c = &mut state.contexts[i];
        if c.verb != Verb::Executing {
            return self.close(state, i, false, reason.as_deref(), now);
        }
        c.callback.stopped_because(reason, now);
        c.verb = Verb::Stopping;
        c.timeout_at = now + OP_TIMEOUT_MILLIS;
        self.send(state, i, job_service::STOP_JOB);
    }

    /// `scheduleOpTimeOutLocked` of an executing context: at its minimum
    /// guarantee, then its maximum, stamping its duration every five
    /// minutes.
    fn executing_timeout(c: &Context, now: i64) -> i64 {
        let earliest = c.execution_start + c.min_guarantee;
        let latest = c.execution_start + c.max_execution;
        let until = if now < earliest { earliest - now } else { latest - now };
        now + until.min(EXECUTION_DURATION_STAMP_PERIOD_MILLIS)
    }

    /// `doCallbackLocked`: a start acknowledged, or the job finished.
    fn callback_done(&self, state: &mut State, i: usize, reschedule: bool, reason: Option<&str>, now: i64) {
        match state.contexts[i].verb {
            // `handleStartedLocked`.
            Verb::Starting => {
                let c = &mut state.contexts[i];
                c.verb = Verb::Executing;
                if !reschedule {
                    return self.close(state, i, false, Some("onStartJob returned false"), now);
                }
                if c.cancelled {
                    return self.send_stop(state, i, None, now);
                }
                c.timeout_at = Self::executing_timeout(c, now);
            }
            Verb::Executing | Verb::Stopping => self.close(state, i, reschedule, reason, now),
            Verb::Binding => {}
        }
    }

    /// `handleOpTimeoutLocked` of context `i`.
    fn op_timeout(&self, state: &mut State, i: usize, now: i64) {
        let anr = |debug: &str| StopReason { reason: STOP_REASON_UNDEFINED, internal: INTERNAL_STOP_REASON_ANR, debug: Some(debug.into()) };
        match state.contexts[i].verb {
            Verb::Binding => {
                state.contexts[i].stop = anr("timed out while binding");
                self.close(state, i, true, Some("timed out while binding"), now)
            }
            Verb::Starting => {
                state.contexts[i].stop = anr("timed out while starting");
                self.close(state, i, false, Some("timed out while starting"), now)
            }
            Verb::Stopping => self.close(state, i, true, Some("timed out while stopping"), now),
            Verb::Executing => {
                let key = state.contexts[i].key;
                if let Some(stop) = state.contexts[i].pending_stop.take() {
                    let ready = state.jobs.get_mut(&key).is_some_and(|j| j.ready);
                    if !ready {
                        let debug = stop.debug.clone();
                        state.contexts[i].stop = stop;
                        return self.send_stop(state, i, debug, now);
                    }
                }
                let c = &state.contexts[i];
                let (earliest, latest) = (c.execution_start + c.min_guarantee, c.execution_start + c.max_execution);
                if now >= latest {
                    let j = &state.jobs[&key];
                    let abandoned = j.abandoned
                        && self.flag(FLAG_HANDLE_ABANDONED_JOBS).unwrap_or(false)
                        && !self.change(OVERRIDE_HANDLE_ABANDONED_JOBS, j.source_uid).unwrap_or(true);
                    let (reason, internal, suffix) = if abandoned {
                        (STOP_REASON_TIMEOUT_ABANDONED, INTERNAL_STOP_REASON_TIMEOUT_ABANDONED, " and maybe abandoned")
                    } else {
                        (STOP_REASON_TIMEOUT, INTERNAL_STOP_REASON_TIMEOUT, "")
                    };
                    state.contexts[i].stop =
                        StopReason { reason, internal, debug: Some(format!("client timed out{suffix}")) };
                    self.send_stop(state, i, Some(format!("timeout while executing{suffix}")), now);
                } else if now >= earliest && let Some(why) = self.should_stop(state, i) {
                    state.contexts[i].stop =
                        StopReason { reason: STOP_REASON_DEVICE_STATE, internal: INTERNAL_STOP_REASON_TIMEOUT, debug: Some(why.clone()) };
                    self.send_stop(state, i, Some(why), now);
                } else {
                    // Its execution time stamped.
                    let c = &mut state.contexts[i];
                    let since = now - c.last_stamp;
                    c.last_stamp = now;
                    c.timeout_at = Self::executing_timeout(c, now);
                    if let Some(j) = state.jobs.get_mut(&key) {
                        j.cumulative_execution += since;
                    }
                    self.touch(state, key, now);
                }
            }
        }
    }

    /// `JobConcurrencyManager.shouldStopRunningJobLocked` past a job's
    /// minimum guarantee: battery saver, or ready jobs waiting for its
    /// slot (expedited ones for an expedited job).
    fn should_stop(&self, state: &State, i: usize) -> Option<String> {
        if state.device.is_some_and(|(d, _)| d.battery_saver) {
            return Some("battery saver".into());
        }
        if state.contexts.len() > self.limit * 3 / 4 {
            return Some("too many jobs running".into());
        }
        let expedited = state.contexts[i].expedited;
        let blocked = state.pending.iter().any(|k| !expedited || state.jobs[k].expedited());
        blocked.then(|| format!("blocking {} queue", if expedited { "EJ" } else { "regular" }))
    }

    /// `closeAndCleanupJobLocked`, then `onJobCompletedLocked` and the
    /// pending jobs run in the freed context.
    fn close(&self, state: &mut State, i: usize, reschedule: bool, reason: Option<&str>, now: i64) {
        let c = state.contexts.remove(i);
        c.callback.stopped_because(reason.map(str::to_string), now);
        c.callback.finished(c.stop.internal == INTERNAL_STOP_REASON_SUCCESSFUL_FINISH, now);
        if let Some(j) = state.jobs.get_mut(&c.key) {
            j.cumulative_execution += now - c.last_stamp;
        }
        self.host.unbind_service_from_host(&c.conn);
        self.completed(state, c.key, c.stop.reason, c.stop.internal, reschedule, now);
        self.run_pending(state, now);
    }

    /// `onJobCompletedLocked`: a failed job rescheduled with back-off, a
    /// periodic one for its next window.
    fn completed(&self, state: &mut State, key: u64, stop: i32, internal: i32, reschedule: bool, now: i64) {
        let Some(j) = state.jobs.get(&key) else { return };
        let (tracked, periodic) = (j.tracked, j.job.periodic);
        let mut next = if reschedule { Some(self.reschedule_for_failure(j, stop, internal, now)) } else { None };
        if !tracked {
            // Cancelled while it ran; a job of its id may wait to run.
            state.jobs.remove(&key);
            self.post(Message::Check);
            return;
        }
        if next.is_none() && periodic {
            let (earliest, latest) = periodic_window(j.job.interval, j.job.flex, j.original_latest, now);
            let mut n = j.rescheduled(earliest, latest);
            n.last_successful_run = wall_ms();
            n.last_failed_run = j.last_failed_run;
            next = Some(n);
        }
        self.stop_tracking(state, key, next.as_mut(), !periodic, now);
        if let Some(next) = next {
            self.track(state, next, now);
        }
        self.post(Message::Check);
    }

    /// `getRescheduleJobForFailureLocked`.
    fn reschedule_for_failure(&self, j: &JobStatus, stop: i32, internal: i32, now: i64) -> JobStatus {
        let handle_abandoned = self.flag(FLAG_HANDLE_ABANDONED_JOBS).unwrap_or(false)
            && !self.change(OVERRIDE_HANDLE_ABANDONED_JOBS, j.source_uid).unwrap_or(true);
        let (mut failures, mut abandoned, mut system_stops) = (j.num_failures, j.num_abandoned_failures, j.num_system_stops);
        if matches!(internal, INTERNAL_STOP_REASON_SUCCESSFUL_FINISH | INTERNAL_STOP_REASON_TIMEOUT | INTERNAL_STOP_REASON_ANR)
            || stop == STOP_REASON_USER
        {
            failures += 1;
        } else if handle_abandoned && internal == INTERNAL_STOP_REASON_TIMEOUT_ABANDONED {
            abandoned += 1;
            failures += 1;
        } else {
            system_stops += 1;
        }
        let mut policy = j.job.backoff_policy;
        if handle_abandoned && abandoned > ABANDONED_JOB_TIMEOUTS_BEFORE_AGGRESSIVE_BACKOFF {
            policy = BACKOFF_POLICY_EXPONENTIAL;
        }
        let attempts = failures + system_stops / SYSTEM_STOP_TO_FAILURE_RATIO;
        let earliest = backoff_delay(policy, j.job.initial_backoff, attempts).map_or(NO_EARLIEST_RUNTIME, |d| now + d);
        let mut n = j.rescheduled(earliest, NO_LATEST_RUNTIME);
        n.num_failures = failures;
        n.num_abandoned_failures = abandoned;
        n.num_system_stops = system_stops;
        n.last_successful_run = j.last_successful_run;
        n.last_failed_run = wall_ms();
        n.cumulative_execution = j.cumulative_execution;
        if stop == STOP_REASON_USER {
            n.internal_flags |= INTERNAL_FLAG_DEMOTED_BY_USER;
        }
        if j.job.periodic {
            n.original_latest = j.original_latest;
        }
        n
    }

    /// `getPendingJobReasonsLocked`.
    fn pending_reasons(&self, uid: i32, namespace: Option<&str>, id: i32) -> Result<Vec<i32>, Exception> {
        let api = self.flag(FLAG_GET_PENDING_JOB_REASONS_API)?;
        let mut guard = self.state.lock().unwrap();
        let state = &mut *guard;
        self.ensure_loaded(state)?;
        let Some(key) = Self::find(state, uid, namespace, id) else {
            return Ok(vec![PENDING_JOB_REASON_INVALID_JOB_ID]);
        };
        if state.contexts.iter().any(|c| c.key == key) {
            return Ok(vec![PENDING_JOB_REASON_EXECUTING]);
        }
        let now = elapsed_ms();
        let device = self.device(state, now);
        let j = state.jobs.get_mut(&key).unwrap();
        let top = self.is_top(j.source_uid);
        if !j.evaluate(now, &device, top) {
            return Ok(j.pending_reasons(api));
        }
        if state.pending.contains(&key) {
            return Ok(vec![PENDING_JOB_REASON_DEVICE_STATE]);
        }
        Ok(vec![if j.process.is_some() { PENDING_JOB_REASON_UNDEFINED } else { PENDING_JOB_REASON_APP }])
    }

    /// The tracked jobs `uid` scheduled.
    fn jobs_of(&self, uid: i32) -> Result<Vec<(Option<String>, JobInfo)>, Exception> {
        let mut state = self.state.lock().unwrap();
        self.ensure_loaded(&mut state)?;
        Ok(state
            .jobs
            .values()
            .filter(|j| j.tracked && j.calling_uid == uid)
            .map(|j| (j.namespace.clone(), j.job.clone()))
            .collect())
    }

    /// `checkRunUserInitiatedJobsPermission`: `RUN_USER_INITIATED_JOBS`, an
    /// install-time permission without an app op.
    fn can_run_user_initiated(&self, uid: i32) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some("android.permission.RUN_USER_INITIATED_JOBS"), uid)? == 0)
    }

    /// `IJobCallback`'s calls of `callback`; the data transfer reports
    /// are of network jobs, which do not run (#1509).
    fn callback_call(&self, callback: &JobCallback, call: &mut Call<'_>) -> Reply {
        let mut state = self.state.lock().unwrap();
        let now = elapsed_ms();
        let current = state.contexts.iter().position(|c| c.callback.id == callback.id);
        let mut p = Parcel::new();
        match call.code {
            cb::ACKNOWLEDGE_START_MESSAGE => {
                let a = cb::AcknowledgeStartMessage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(i) = current {
                    self.callback_done(&mut state, i, a.ongoing, Some("finished start"), now);
                }
                cb::write_acknowledge_start_message_reply(&mut p);
            }
            cb::ACKNOWLEDGE_STOP_MESSAGE => {
                let a = cb::AcknowledgeStopMessage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(i) = current {
                    self.callback_done(&mut state, i, a.reschedule, None, now);
                }
                cb::write_acknowledge_stop_message_reply(&mut p);
            }
            cb::JOB_FINISHED => {
                let a = cb::JobFinished::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(i) = current {
                    state.contexts[i].stop = StopReason {
                        reason: STOP_REASON_UNDEFINED,
                        internal: INTERNAL_STOP_REASON_SUCCESSFUL_FINISH,
                        debug: Some("app called jobFinished".into()),
                    };
                    self.callback_done(&mut state, i, a.reschedule, Some("app called jobFinished"), now);
                }
                cb::write_job_finished_reply(&mut p);
            }
            cb::DEQUEUE_WORK => {
                cb::DequeueWork::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let current = match callback.assert_current(current, now) {
                    Ok(c) => c,
                    Err(e) => return reply(Err(e)),
                };
                let work = match current {
                    Some(i) if state.contexts[i].verb != Verb::Stopping => {
                        let key = state.contexts[i].key;
                        let j = state.jobs.get_mut(&key).unwrap();
                        let work = j.dequeue_work();
                        if work.is_none() && j.executing_work.is_empty() {
                            state.contexts[i].stop = StopReason {
                                reason: STOP_REASON_UNDEFINED,
                                internal: INTERNAL_STOP_REASON_SUCCESSFUL_FINISH,
                                debug: Some("last work dequeued".into()),
                            };
                            self.callback_done(&mut state, i, false, Some("last work dequeued"), now);
                        } else if work.is_some() {
                            self.touch(&mut state, key, now);
                        }
                        work
                    }
                    _ => None,
                };
                cb::write_dequeue_work_reply(&mut p, work.as_ref());
            }
            cb::COMPLETE_WORK => {
                let a = cb::CompleteWork::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let done = match callback.assert_current(current, now) {
                    Ok(Some(i)) => {
                        let key = state.contexts[i].key;
                        let j = state.jobs.get_mut(&key).unwrap();
                        let at = j.executing_work.iter().position(|w| w.work_id == a.work_id);
                        if let Some(at) = at {
                            j.executing_work.remove(at);
                            self.touch(&mut state, key, now);
                        }
                        at.is_some()
                    }
                    Ok(None) => true,
                    Err(e) => return reply(Err(e)),
                };
                cb::write_complete_work_reply(&mut p, done);
            }
            cb::HANDLE_ABANDONED_JOB => {
                let a = cb::HandleAbandonedJob::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(i) = current
                    && state.contexts[i].verb == Verb::Executing
                {
                    let key = state.contexts[i].key;
                    if let Some(j) = state.jobs.get_mut(&key).filter(|j| j.job.id == a.job_id) {
                        j.abandoned = true;
                    }
                }
                cb::write_handle_abandoned_job_reply(&mut p);
            }
            code => return reply(Err(self.host.unimplemented("jobscheduler callback", code))),
        }
        Ok(p)
    }

    /// The handler's message `m`.
    fn handle(&self, state: &mut State, m: Message, now: i64) {
        match m {
            Message::Connected { callback, service, dead } => self.connected(state, callback, service, dead, now),
            Message::Check => self.check(state, now),
            Message::Persist => {}
        }
    }

    /// The handler's timers at `now`: due op timeouts, a time constraint or
    /// the Mac's state come due, the write of the persisted jobs; when it
    /// is next to wake, and the jobs to write.
    fn tick(&self, state: &mut State, now: i64) -> (Option<i64>, Vec<(i32, Vec<Stored>)>) {
        while let Some(i) = state.contexts.iter().position(|c| c.timeout_at <= now) {
            self.op_timeout(state, i, now);
        }
        let watching = state
            .jobs
            .values()
            .any(|j| j.tracked && j.required & DEVICE_CONSTRAINTS != 0);
        let before = state.device.map(|(d, _)| d);
        let mut changed = false;
        if state.running && watching {
            changed |= before.is_some_and(|b| b != self.device(state, now));
        }
        // `TimeController`: a delay or deadline passed since the last
        // evaluation.
        changed |= state.jobs.values().any(|j| {
            j.tracked
                && ((j.required & CONSTRAINT_TIMING_DELAY != 0 && j.satisfied & CONSTRAINT_TIMING_DELAY == 0 && now >= j.earliest)
                    || (j.required & CONSTRAINT_DEADLINE != 0 && j.satisfied & CONSTRAINT_DEADLINE == 0 && now >= j.latest))
        });
        if changed {
            self.check(state, now);
        }
        let mut writes = Vec::new();
        if state.write_at.is_some_and(|t| t <= now) {
            state.write_at = None;
            for uid in std::mem::take(&mut state.dirty) {
                let jobs = state
                    .jobs
                    .values()
                    .filter(|j| j.tracked && j.job.persisted && j.calling_uid == uid)
                    .map(JobStatus::stored)
                    .collect();
                writes.push((uid, jobs));
            }
        }
        let mut next: Vec<i64> = state.contexts.iter().map(|c| c.timeout_at).collect();
        next.extend(state.write_at);
        if state.running {
            for j in state.jobs.values().filter(|j| j.tracked) {
                if j.required & CONSTRAINT_TIMING_DELAY != 0 && j.earliest > now {
                    next.push(j.earliest);
                }
                if j.required & CONSTRAINT_DEADLINE != 0 && j.latest > now {
                    next.push(j.latest);
                }
            }
            if watching {
                next.push(now + DEVICE_READ_INTERVAL);
            }
        }
        (next.into_iter().min(), writes)
    }
}

/// The handler (`JobHandler`): messages, timeouts and the controllers'
/// time-based changes, and the store's writes.
fn run(scheduler: Weak<JobScheduler>) {
    let mut wake_at: Option<i64> = None;
    loop {
        let Some(s) = scheduler.upgrade() else { return };
        let messages = {
            let mut queue = s.queue.lock().unwrap();
            if queue.is_empty() {
                let wait = wake_at.map_or(Duration::from_secs(3600), |t| {
                    Duration::from_millis((t - elapsed_ms()).max(0) as u64)
                });
                queue = s.wake.wait_timeout(queue, wait).unwrap().0;
            }
            std::mem::take(&mut *queue)
        };
        let now = elapsed_ms();
        let mut state = s.state.lock().unwrap();
        for m in messages {
            s.handle(&mut state, m, now);
        }
        let (next, writes) = s.tick(&mut state, now);
        wake_at = next;
        let migrate = !writes.is_empty() && std::mem::take(&mut state.migrate);
        drop(state);
        if writes.is_empty() {
            continue;
        }
        let Ok(inputs) = s.host.inputs() else { continue };
        let dir = inputs.data.join("system/job");
        let wall = wall_ms();
        for (uid, jobs) in writes {
            let refs: Vec<&Stored> = jobs.iter().collect();
            if let Err(e) = job_store::write(&dir, uid, &refs, now, wall) {
                eprintln!("app host: jobs: writing the jobs of uid {uid}: {e}");
            }
        }
        if migrate {
            job_store::remove_legacy(&dir);
        }
    }
}

impl JobCallback {
    /// `applyStoppedReasonLocked`.
    fn stopped_because(&self, reason: Option<String>, now: i64) {
        let mut stopped = self.stopped.lock().unwrap();
        if reason.is_some() && stopped.as_ref().is_none_or(|s| s.reason.is_none()) {
            *stopped = Some(Stopped { reason, at: now, successful: false });
        }
    }

    fn finished(&self, successful: bool, now: i64) {
        let mut stopped = self.stopped.lock().unwrap();
        let s = stopped.get_or_insert(Stopped { reason: None, at: now, successful });
        s.successful = successful;
        s.at = now;
    }

    /// `assertCallerLocked`: the context of a current callback; a stale one
    /// is answered quietly within 15 s of an unsuccessful finish, else
    /// refused.
    fn assert_current(&self, current: Option<usize>, now: i64) -> Result<Option<usize>, Exception> {
        if current.is_some() {
            return Ok(current);
        }
        let stopped = self.stopped.lock().unwrap();
        if stopped.as_ref().is_some_and(|s| !s.successful && now - s.at < 15_000) {
            return Ok(None);
        }
        let mut message = "Caller no longer running".to_string();
        if let Some(Stopped { reason: Some(reason), at, .. }) = stopped.as_ref() {
            message += &format!(", last stopped +{}ms because: {reason}", now - at);
        }
        Err(Exception::security(message))
    }
}

impl Service for JobCallback {
    fn descriptor(&self) -> &str {
        cb::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(scheduler) = self.scheduler.upgrade() else { return Err(BAD_VALUE) };
        scheduler.callback_call(self, call)
    }
}

/// A `ParceledListSlice` of jobs.
fn slice(host: &AppHost, jobs: Vec<JobInfo>) -> crate::package::list_slice::Slice<JobInfo> {
    crate::package::list_slice::Slice::new(host.process.clone(), "android.app.job.JobInfo", jobs)
}

impl Service for JobScheduler {
    fn descriptor(&self) -> &str {
        js::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let bad = |_| BAD_VALUE;
        // `JobInfo` reads as `JobInfo.read`, which tells what it does not
        // read apart.
        struct Job(job_info::Read);
        impl aim_service_aidl::ReadParcelable for Job {
            fn read_from(r: &mut aim_binder_host::parcel::Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
                JobInfo::read(r).map(Job)
            }
        }
        let validated = |n: Option<String>| match n.map(|n| n.trim().to_string()) {
            Some(n) if n.is_empty() => Err(Exception::illegal_argument("namespace cannot be empty")),
            n => Ok(n),
        };
        match call.code {
            js::SCHEDULE => {
                let a = js::Schedule::<Job>::read(&mut call.data).map_err(bad)?;
                reply(self.schedule(uid, a.namespace, a.job.map(|j| j.0), None, None).map(|r| {
                    let mut p = Parcel::new();
                    js::write_schedule_reply(&mut p, r);
                    p
                }))
            }
            js::ENQUEUE => {
                let a = js::Enqueue::<Job, ReadWorkItem>::read(&mut call.data).map_err(bad)?;
                reply(self.schedule(uid, a.namespace, a.job.map(|j| j.0), Some(a.work), None).map(|r| {
                    let mut p = Parcel::new();
                    js::write_enqueue_reply(&mut p, r);
                    p
                }))
            }
            js::SCHEDULE_AS_PACKAGE => {
                let a = js::ScheduleAsPackage::<Job>::read(&mut call.data).map_err(bad)?;
                let proxy = Some((a.package_name, a.user_id, a.tag));
                reply(self.schedule(uid, a.namespace, a.job.map(|j| j.0), None, proxy).map(|r| {
                    let mut p = Parcel::new();
                    js::write_schedule_as_package_reply(&mut p, r);
                    p
                }))
            }
            js::CANCEL => {
                let a = js::Cancel::read(&mut call.data).map_err(bad)?;
                reply((|| {
                    let namespace = validated(a.namespace)?;
                    let mut state = self.state.lock().unwrap();
                    self.ensure_loaded(&mut state)?;
                    if let Some(key) = Self::find(&state, uid, namespace.as_deref(), a.job_id) {
                        let debug = format!("cancel() called by app, callingUid={uid} uid={uid} jobId={}", a.job_id);
                        self.cancel_impl(&mut state, key, None, STOP_REASON_CANCELLED_BY_APP, INTERNAL_STOP_REASON_CANCELED, &debug, elapsed_ms());
                    }
                    let mut p = Parcel::new();
                    js::write_cancel_reply(&mut p);
                    Ok(p)
                })())
            }
            js::CANCEL_ALL => {
                js::CancelAll::read(&mut call.data).map_err(bad)?;
                reply(self.cancel_for_uid(uid, None, &format!("cancelAll() called by app, callingUid={uid}")).map(|()| {
                    let mut p = Parcel::new();
                    js::write_cancel_all_reply(&mut p);
                    p
                }))
            }
            js::CANCEL_ALL_IN_NAMESPACE => {
                let a = js::CancelAllInNamespace::read(&mut call.data).map_err(bad)?;
                reply(validated(a.namespace).and_then(|n| {
                    self.cancel_for_uid(uid, Some(n), &format!("cancelAllInNamespace() called by app, callingUid={uid}"))?;
                    let mut p = Parcel::new();
                    js::write_cancel_all_in_namespace_reply(&mut p);
                    Ok(p)
                }))
            }
            js::GET_ALL_PENDING_JOBS => {
                call.data.enforce_interface(js::DESCRIPTOR).map_err(bad)?;
                reply(self.jobs_of(uid).map(|jobs| {
                    // `Map<String, ParceledListSlice<JobInfo>>`: its size,
                    // then each namespace and its slice.
                    let mut by: Vec<(Option<String>, Vec<JobInfo>)> = Vec::new();
                    for (namespace, job) in jobs {
                        match by.iter_mut().find(|(n, _)| *n == namespace) {
                            Some((_, list)) => list.push(job),
                            None => by.push((namespace, vec![job])),
                        }
                    }
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    p.write_i32(by.len() as i32);
                    for (namespace, list) in by {
                        p.write_string16(namespace.as_deref());
                        aim_service_aidl::write_typed(&mut p, Some(&slice(&self.host, list)));
                    }
                    p
                }))
            }
            js::GET_ALL_PENDING_JOBS_IN_NAMESPACE => {
                let a = js::GetAllPendingJobsInNamespace::read(&mut call.data).map_err(bad)?;
                reply(validated(a.namespace).and_then(|namespace| {
                    let jobs = self.jobs_of(uid)?.into_iter().filter(|(n, _)| *n == namespace).map(|(_, j)| j).collect();
                    let mut p = Parcel::new();
                    js::write_get_all_pending_jobs_in_namespace_reply(&mut p, Some(&slice(&self.host, jobs)));
                    Ok(p)
                }))
            }
            js::GET_PENDING_JOB => {
                let a = js::GetPendingJob::read(&mut call.data).map_err(bad)?;
                reply(validated(a.namespace).and_then(|namespace| {
                    let job = self.jobs_of(uid)?.into_iter().find(|(n, j)| *n == namespace && j.id == a.job_id).map(|(_, j)| j);
                    let mut p = Parcel::new();
                    js::write_get_pending_job_reply(&mut p, job.as_ref());
                    Ok(p)
                }))
            }
            js::GET_PENDING_JOB_REASON => {
                let a = js::GetPendingJobReason::read(&mut call.data).map_err(bad)?;
                reply(validated(a.namespace).and_then(|n| {
                    let reasons = self.pending_reasons(uid, n.as_deref(), a.job_id)?;
                    let mut p = Parcel::new();
                    js::write_get_pending_job_reason_reply(&mut p, reasons[0]);
                    Ok(p)
                }))
            }
            js::GET_PENDING_JOB_REASONS => {
                let a = js::GetPendingJobReasons::read(&mut call.data).map_err(bad)?;
                reply(validated(a.namespace).and_then(|n| {
                    let reasons = self.pending_reasons(uid, n.as_deref(), a.job_id)?;
                    let mut p = Parcel::new();
                    js::write_get_pending_job_reasons_reply(&mut p, &Some(reasons));
                    Ok(p)
                }))
            }
            js::CAN_RUN_USER_INITIATED_JOBS => {
                let a = js::CanRunUserInitiatedJobs::read(&mut call.data).map_err(bad)?;
                reply((|| {
                    let package = a.package_name.unwrap_or_default();
                    let user = uid / PER_USER_RANGE as i32;
                    if self.package_uid(&package, user)? != Some(uid) {
                        return Err(Exception::security(format!(
                            "Uid {uid} cannot query canRunUserInitiatedJobs for package {package}"
                        )));
                    }
                    let mut p = Parcel::new();
                    js::write_can_run_user_initiated_jobs_reply(&mut p, self.can_run_user_initiated(uid)?);
                    Ok(p)
                })())
            }
            js::HAS_RUN_USER_INITIATED_JOBS_PERMISSION => {
                let a = js::HasRunUserInitiatedJobsPermission::read(&mut call.data).map_err(bad)?;
                reply((|| {
                    let package = a.package_name.unwrap_or_default();
                    let package_uid = self.package_uid(&package, a.user_id)?.unwrap_or(-1);
                    let core = uid.rem_euclid(PER_USER_RANGE as i32) < 10_000;
                    if uid != package_uid && !core {
                        return Err(Exception::security(format!(
                            "Uid {uid} cannot query hasRunUserInitiatedJobsPermission for package {package}"
                        )));
                    }
                    let mut p = Parcel::new();
                    js::write_has_run_user_initiated_jobs_permission_reply(&mut p, self.can_run_user_initiated(package_uid)?);
                    Ok(p)
                })())
            }
            js::GET_STARTED_JOBS => {
                js::GetStartedJobs::read(&mut call.data).map_err(bad)?;
                reply((|| {
                    if uid != 1000 {
                        return Err(Exception::security("getStartedJobs() is system internal use only."));
                    }
                    let state = self.state.lock().unwrap();
                    let jobs: Vec<Option<JobInfo>> =
                        state.contexts.iter().rev().map(|c| Some(state.jobs[&c.key].job.clone())).collect();
                    let mut p = Parcel::new();
                    js::write_get_started_jobs_reply(&mut p, Some(jobs.as_slice()));
                    Ok(p)
                })())
            }
            code => reply(Err(self.host.unimplemented("jobscheduler", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGGED: Device =
        Device { power_connected: true, charging: true, battery_not_low: true, storage_not_low: true, battery_saver: false };

    fn job(build: impl FnOnce(&mut JobInfo)) -> JobInfo {
        let mut j = JobInfo::persisted_builder(1, "com.example".into(), "com.example.Job".into());
        j.persisted = false;
        build(&mut j);
        j
    }

    fn status(info: JobInfo, earliest: i64, latest: i64) -> JobStatus {
        let source = Source { package: "com.example".into(), user: 0, uid: 10123, tag: None };
        let mut j = JobStatus::new(info, 10123, source, None, earliest, latest, Arc::new(Vec::new()));
        j.process = Some("com.example".into());
        j
    }

    /// `TimeController` and `JobStatus.isReady`: a delay holds a job until
    /// it passes; a deadline makes a one-off job ready whatever else.
    #[test]
    fn time_constraints_decide_readiness() {
        let mut delayed = status(job(|j| j.constraint_flags = CONSTRAINT_FLAG_DEVICE_IDLE), 1000, NO_LATEST_RUNTIME);
        assert!(!delayed.evaluate(999, &PLUGGED, false));
        assert_eq!(delayed.pending_reasons(true), [PENDING_JOB_REASON_CONSTRAINT_DEVICE_IDLE, PENDING_JOB_REASON_CONSTRAINT_MINIMUM_LATENCY]);
        assert!(!delayed.evaluate(1000, &PLUGGED, false));
        let mut deadline = status(job(|j| j.constraint_flags = CONSTRAINT_FLAG_DEVICE_IDLE), NO_EARLIEST_RUNTIME, 5000);
        assert!(!deadline.evaluate(4999, &PLUGGED, false));
        assert!(deadline.evaluate(5000, &PLUGGED, false));
        // A periodic job's deadline does not override its constraints.
        let mut periodic = status(
            job(|j| {
                j.set_periodic(MIN_PERIOD_MILLIS, MIN_PERIOD_MILLIS);
                j.constraint_flags = CONSTRAINT_FLAG_DEVICE_IDLE;
            }),
            0,
            5000,
        );
        assert!(!periodic.evaluate(6000, &PLUGGED, false));
    }

    /// `BatteryController`, the storage controller and battery saver.
    #[test]
    fn device_constraints_decide_readiness() {
        let charging = |j: &mut JobInfo| j.constraint_flags = CONSTRAINT_FLAG_CHARGING | CONSTRAINT_FLAG_STORAGE_NOT_LOW;
        let mut j = status(job(charging), NO_EARLIEST_RUNTIME, NO_LATEST_RUNTIME);
        assert!(j.evaluate(0, &PLUGGED, false));
        let unplugged = Device { power_connected: false, charging: false, ..PLUGGED };
        assert!(!j.evaluate(0, &unplugged, false));
        assert_eq!(j.reason_unready, STOP_REASON_CONSTRAINT_CHARGING);
        assert_eq!(j.pending_reasons(true), [PENDING_JOB_REASON_CONSTRAINT_CHARGING]);
        // A top app's job needs power only, battery low or not.
        let low = Device { battery_not_low: false, ..PLUGGED };
        assert!(j.evaluate(0, &low, true));
        assert!(!j.evaluate(0, &low, false));
        let full = Device { storage_not_low: false, ..PLUGGED };
        assert!(!j.evaluate(0, &full, true));
        let saver = Device { battery_saver: true, ..PLUGGED };
        assert!(j.evaluate(0, &saver, true));
        assert!(!j.evaluate(0, &saver, false));
        assert_eq!(j.pending_reasons(true), [PENDING_JOB_REASON_DEVICE_STATE]);
    }

    /// No network is known: a network job waits (`ConnectivityController`).
    #[test]
    fn a_network_job_waits() {
        let network = job(|j| j.network = Some(job_info::NetworkRequest::restored(&[12], &[], &[])));
        let mut j = status(network, NO_EARLIEST_RUNTIME, NO_LATEST_RUNTIME);
        assert!(!j.evaluate(0, &PLUGGED, true));
        assert_eq!(j.pending_reasons(true), [PENDING_JOB_REASON_CONSTRAINT_CONNECTIVITY]);
        assert_eq!(j.job.network.as_ref().map(|n| n.capabilities), Some(1 << 12));
    }

    /// `getRescheduleJobForFailureLocked`'s delays.
    #[test]
    fn back_off_is_the_originals() {
        assert_eq!(backoff_delay(BACKOFF_POLICY_EXPONENTIAL, 30_000, 0), None);
        assert_eq!(backoff_delay(BACKOFF_POLICY_EXPONENTIAL, 30_000, 1), Some(30_000));
        assert_eq!(backoff_delay(BACKOFF_POLICY_EXPONENTIAL, 30_000, 3), Some(120_000));
        assert_eq!(backoff_delay(BACKOFF_POLICY_LINEAR, 5_000, 3), Some(30_000));
        assert_eq!(backoff_delay(BACKOFF_POLICY_EXPONENTIAL, 30_000, 40), Some(MAX_BACKOFF_DELAY_MILLIS));
    }

    /// `getRescheduleJobForPeriodic`: the next window, a buffer after one
    /// that ran early, skipped windows after a late one.
    #[test]
    fn periodic_jobs_are_rescheduled_as_the_original_does() {
        let hour = 60 * MINUTE;
        // Ran at the end of its window: the next starts a sixth of a
        // period later.
        assert_eq!(periodic_window(hour, hour, 10 * hour, 10 * hour), (10 * hour + 10 * MINUTE, 11 * hour));
        // Ran 5 minutes before its window ended: a buffer of 5 minutes.
        assert_eq!(periodic_window(hour, hour, 10 * hour, 10 * hour - 5 * MINUTE), (10 * hour + 5 * MINUTE, 11 * hour));
        // Ran 2.5 periods late.
        assert_eq!(periodic_window(hour, hour, 10 * hour, 12 * hour + 30 * MINUTE), (12 * hour, 13 * hour));
        // A flex window of 15 minutes run too close to the next.
        let (_, latest) = periodic_window(hour, 15 * MINUTE, 10 * hour, 10 * hour + 44 * MINUTE);
        assert_eq!(latest, 12 * hour);
    }

    #[test]
    fn effective_priority_decays_with_failures() {
        let mut j = status(job(|_| {}), 0, NO_LATEST_RUNTIME);
        assert_eq!(j.effective_priority(), PRIORITY_DEFAULT);
        j.num_failures = 4;
        assert_eq!(j.effective_priority(), PRIORITY_LOW);
        j.num_failures = 6;
        assert_eq!(j.effective_priority(), PRIORITY_MIN);
    }

    #[test]
    fn work_is_handed_to_a_replacement() {
        let mut old = status(job(|_| {}), 0, NO_LATEST_RUNTIME);
        let item = |n| JobWorkItem::persisted(Default::default(), n, -1, -1, -1);
        old.enqueue_work(item(0));
        old.enqueue_work(item(0));
        let first = old.dequeue_work().unwrap();
        assert_eq!((first.work_id, first.delivery_count), (1, 1));
        assert_eq!(old.work_count(), 2);
    }
}
