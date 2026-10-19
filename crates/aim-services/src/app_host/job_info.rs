//! The parcelables of `jobscheduler` (`android.app.job` at
//! `android-16.0.0_r1`): `JobInfo` as `JobInfo(Parcel)` reads it and
//! `writeToParcel` writes it, with `enforceValidity`, the `JobInfo.Builder`
//! a persisted job is rebuilt with (`JobStore`), and its required network
//! (`super::network`'s `NetworkRequest`); `JobWorkItem`; and the `JobParameters` a job is started with.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::local::LocalProcess;
use aim_binder_host::parcel::{BAD_TYPE, BAD_VALUE, Binder, Exception, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::kept::{KeptIntent, RawIntent};
use super::network::{NetworkCapabilities, NetworkRequest, RequestType};
use crate::clip::{self, Raw};
use crate::package::uri::Uri;
use crate::package::restrictions::persistable::Bundle;

/// `JobInfo.NETWORK_BYTES_UNKNOWN`.
pub const NETWORK_BYTES_UNKNOWN: i64 = -1;
/// `JobInfo.DEFAULT_INITIAL_BACKOFF_MILLIS`, `MAX_BACKOFF_DELAY_MILLIS`,
/// `MIN_BACKOFF_MILLIS`.
pub const DEFAULT_INITIAL_BACKOFF_MILLIS: i64 = 30_000;
pub const MAX_BACKOFF_DELAY_MILLIS: i64 = 5 * 60 * 60 * 1000;
pub const MIN_BACKOFF_MILLIS: i64 = 10_000;
/// `JobInfo.BACKOFF_POLICY_*`; the default is exponential.
pub const BACKOFF_POLICY_LINEAR: i32 = 0;
pub const BACKOFF_POLICY_EXPONENTIAL: i32 = 1;
/// `JobInfo.getMinPeriodMillis`, `getMinFlexMillis`.
pub const MIN_PERIOD_MILLIS: i64 = 15 * 60 * 1000;
pub const MIN_FLEX_MILLIS: i64 = 5 * 60 * 1000;
/// `JobInfo.MIN_ALLOWED_TIME_WINDOW_MILLIS`.
const MIN_ALLOWED_TIME_WINDOW_MILLIS: i64 = MIN_PERIOD_MILLIS;
/// `JobInfo.PRIORITY_*`.
pub const PRIORITY_MIN: i32 = 100;
pub const PRIORITY_LOW: i32 = 200;
pub const PRIORITY_DEFAULT: i32 = 300;
pub const PRIORITY_HIGH: i32 = 400;
pub const PRIORITY_MAX: i32 = 500;
/// `JobInfo.BIAS_DEFAULT`.
pub const BIAS_DEFAULT: i32 = 0;
/// `JobInfo.FLAG_*`.
pub const FLAG_WILL_BE_FOREGROUND: i32 = 1 << 0;
pub const FLAG_IMPORTANT_WHILE_FOREGROUND: i32 = 1 << 1;
pub const FLAG_PREFETCH: i32 = 1 << 2;
pub const FLAG_EXEMPT_FROM_APP_STANDBY: i32 = 1 << 3;
pub const FLAG_EXPEDITED: i32 = 1 << 4;
pub const FLAG_USER_INITIATED: i32 = 1 << 5;
/// `JobInfo.CONSTRAINT_FLAG_*`.
pub const CONSTRAINT_FLAG_CHARGING: i32 = 1 << 0;
pub const CONSTRAINT_FLAG_BATTERY_NOT_LOW: i32 = 1 << 1;
pub const CONSTRAINT_FLAG_DEVICE_IDLE: i32 = 1 << 2;
pub const CONSTRAINT_FLAG_STORAGE_NOT_LOW: i32 = 1 << 3;
/// `JobInfo.MAX_NUM_DEBUG_TAGS`, `MAX_DEBUG_TAG_LENGTH`, and
/// `MAX_TRACE_TAG_LENGTH` (`Trace.MAX_SECTION_NAME_LEN`).
pub const MAX_NUM_DEBUG_TAGS: usize = 32;
const MAX_DEBUG_TAG_LENGTH: usize = 127;
const MAX_TRACE_TAG_LENGTH: usize = 127;
/// `JobParameters.STOP_REASON_UNDEFINED`, `INTERNAL_STOP_REASON_UNKNOWN`.
pub const STOP_REASON_UNDEFINED: i32 = 0;
pub const INTERNAL_STOP_REASON_UNKNOWN: i32 = -1;
/// `NetworkCapabilities.MAX_NET_CAPABILITY` (`NOT_BANDWIDTH_CONSTRAINED`),
/// `MAX_TRANSPORT` (`TRANSPORT_SATELLITE`).
const MAX_NET_CAPABILITY: u32 = 37;
const MAX_TRANSPORT: u32 = 10;
/// `ConnectivityManager.TYPE_NONE`, `REQUEST_ID_UNSET`.
const TYPE_NONE: i32 = -1;
const REQUEST_ID_UNSET: i32 = 0;
/// `ComponentName`'s class.
const COMPONENT_NAME: &str = "android.content.ComponentName";
/// `Intent.FLAG_GRANT_READ_URI_PERMISSION`, `FLAG_GRANT_WRITE_URI_PERMISSION`.
pub const FLAG_GRANT_URI_PERMISSIONS: i32 = 0x1 | 0x2;

/// `JobInfo.TriggerContentUri`: its URI (none if parcelled as null) and
/// flags.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerContentUri {
    pub uri: Option<Uri>,
    pub flags: i32,
}

/// `readPersistableBundle` as `JobInfo` and `JobWorkItem` keep it: a null
/// bundle as `PersistableBundle.EMPTY`.
fn read_persistable(r: &mut Reader<'_>) -> Result<Bundle> {
    let at = r.position();
    if r.read_i32()? < 0 {
        return Ok(Bundle::default());
    }
    r.set_position(at);
    Bundle::read_from(r)
}

fn write_persistable(p: &mut Parcel, b: &Bundle) {
    // Bundles read from a parcel or from XML have no duplicate keys.
    let bytes = b.parcel().expect("a persistable bundle that was read writes");
    p.write_raw(bytes.data(), &[]);
}

/// The values of the bits set in `bits`, lowest first
/// (`BitUtils.unpackBits`).
pub fn unpack_bits(bits: u64) -> Vec<i32> {
    (0..64).filter(|b| bits & (1 << b) != 0).collect()
}

/// A parcelable that a job's `NetworkRequest` holds and the app host does
/// not read: a network specifier other than Ethernet's or transport info.
pub struct Unsupported(pub &'static str);

/// A job's `NetworkRequest`; `Err(Ok(..))` for what is not read.
fn read_request(r: &mut Reader<'_>) -> Result<std::result::Result<NetworkRequest, Unsupported>> {
    match NetworkRequest::read_from(r) {
        Ok(n) => Ok(Ok(n)),
        Err(BAD_TYPE) => Ok(Err(Unsupported("a network specifier or transport info"))),
        Err(e) => Err(e),
    }
}

/// The request `JobStore` rebuilds from what it persisted:
/// `new NetworkRequest.Builder().clearCapabilities()`, the capabilities,
/// forbidden capabilities and transports added, then `build()`.
pub fn restored_request(capabilities: &[i32], forbidden: &[i32], transports: &[i32]) -> NetworkRequest {
    let mut caps = NetworkCapabilities::cleared();
    // Invalid capabilities and transports are logged and dropped.
    for &c in capabilities.iter().filter(|&&c| (0..=MAX_NET_CAPABILITY as i32).contains(&c)) {
        caps.add(c as u32);
    }
    for &c in forbidden.iter().filter(|&&c| (0..=MAX_NET_CAPABILITY as i32).contains(&c)) {
        caps.forbid(c as u32);
    }
    for &t in transports.iter().filter(|&&t| (0..=MAX_TRANSPORT as i32).contains(&t)) {
        caps.transports |= 1 << t;
    }
    caps.mark_restricted();
    NetworkRequest { caps, legacy_type: TYPE_NONE, request_id: REQUEST_ID_UNSET, kind: RequestType::None }
}

/// `JobStatus`'s copy of `request` for its source uid: `new
/// NetworkRequest.Builder(request).setUids({uid..uid}).build()`.
pub fn request_for_uid(request: &NetworkRequest, uid: i32) -> NetworkRequest {
    let mut caps = request.caps.clone();
    caps.uids = Some(vec![(uid, uid)]);
    caps.mark_restricted();
    NetworkRequest { caps, legacy_type: TYPE_NONE, request_id: REQUEST_ID_UNSET, kind: RequestType::None }
}

/// `JobInfo`.
#[derive(Clone)]
pub struct JobInfo {
    pub id: i32,
    pub extras: Bundle,
    /// `transientExtras` and `clipData` (with its grant flags) as they
    /// were parcelled: passed on as they are.
    transient_extras: Raw,
    pub clip: Option<(Raw, i32)>,
    pub service: (String, String),
    pub constraint_flags: i32,
    /// `triggerContentUris` as parcelled, if any, and as read (a null
    /// element none).
    pub trigger_content_uris: Option<Raw>,
    pub triggers: Vec<Option<TriggerContentUri>>,
    pub trigger_content_update_delay: i64,
    pub trigger_content_max_delay: i64,
    pub network: Option<NetworkRequest>,
    pub network_download_bytes: i64,
    pub network_upload_bytes: i64,
    pub minimum_network_chunk_bytes: i64,
    pub min_latency: i64,
    pub max_execution_delay: i64,
    pub periodic: bool,
    pub persisted: bool,
    pub interval: i64,
    pub flex: i64,
    pub initial_backoff: i64,
    pub backoff_policy: i32,
    pub has_early_constraint: bool,
    pub has_late_constraint: bool,
    pub bias: i32,
    pub priority: i32,
    pub flags: i32,
    pub debug_tags: Vec<String>,
    pub trace_tag: Option<String>,
}

/// A `JobInfo` read, or what of it is not read.
pub type Read = std::result::Result<JobInfo, Unsupported>;

impl JobInfo {
    /// `JobInfo(Parcel)`.
    pub fn read(r: &mut Reader<'_>) -> Result<Read> {
        let id = r.read_i32()?;
        let extras = read_persistable(r)?;
        let start = r.position();
        clip::bundle(r)?;
        let transient_extras = Raw::capture(r, start);
        let clip = if r.read_i32()? != 0 {
            let start = r.position();
            clip::ClipData::read_from(r)?;
            let raw = Raw::capture(r, start);
            Some((raw, r.read_i32()?))
        } else {
            None
        };
        if r.read_string16()?.as_deref() != Some(COMPONENT_NAME) {
            return Err(BAD_VALUE);
        }
        let service = (r.read_string16()?.ok_or(BAD_VALUE)?, r.read_string16()?.ok_or(BAD_VALUE)?);
        let constraint_flags = r.read_i32()?;
        let start = r.position();
        let n = r.read_i32()?;
        let mut triggers = Vec::new();
        for _ in 0..n.max(0) {
            triggers.push(if r.read_i32()? != 0 {
                let uri = Uri::read(r, &mut crate::package::intent_filter::Plain)?;
                Some(TriggerContentUri { uri, flags: r.read_i32()? })
            } else {
                None
            });
        }
        let trigger_content_uris = (n >= 0).then(|| Raw::capture(r, start));
        let trigger_content_update_delay = r.read_i64()?;
        let trigger_content_max_delay = r.read_i64()?;
        let network = if r.read_i32()? != 0 {
            match read_request(r)? {
                Ok(n) => Some(n),
                Err(u) => return Ok(Err(u)),
            }
        } else {
            None
        };
        let mut job = JobInfo {
            id,
            extras,
            transient_extras,
            clip,
            service,
            constraint_flags,
            trigger_content_uris,
            triggers,
            trigger_content_update_delay,
            trigger_content_max_delay,
            network,
            network_download_bytes: r.read_i64()?,
            network_upload_bytes: r.read_i64()?,
            minimum_network_chunk_bytes: r.read_i64()?,
            min_latency: r.read_i64()?,
            max_execution_delay: r.read_i64()?,
            periodic: r.read_i32()? == 1,
            persisted: r.read_i32()? == 1,
            interval: r.read_i64()?,
            flex: r.read_i64()?,
            initial_backoff: r.read_i64()?,
            backoff_policy: r.read_i32()?,
            has_early_constraint: r.read_i32()? == 1,
            has_late_constraint: r.read_i32()? == 1,
            bias: r.read_i32()?,
            priority: r.read_i32()?,
            flags: r.read_i32()?,
            debug_tags: Vec::new(),
            trace_tag: None,
        };
        for _ in 0..r.read_i32()?.max(0) {
            // A null tag is a malformed parcel.
            let tag = r.read_string16()?.ok_or(BAD_VALUE)?;
            if !job.debug_tags.contains(&tag) {
                job.debug_tags.push(tag);
            }
        }
        job.trace_tag = r.read_string16()?;
        Ok(Ok(job))
    }

    /// `new JobInfo.Builder(id, service)` with `setPersisted(true)`, as
    /// `JobStore` starts a persisted job's.
    pub fn persisted_builder(id: i32, package: String, class: String) -> JobInfo {
        let mut empty = Parcel::new();
        empty.write_i32(0); // `Bundle.EMPTY`
        JobInfo {
            id,
            extras: Bundle::default(),
            transient_extras: Raw::of(&empty),
            clip: None,
            service: (package, class),
            constraint_flags: 0,
            trigger_content_uris: None,
            triggers: Vec::new(),
            trigger_content_update_delay: -1,
            trigger_content_max_delay: -1,
            network: None,
            network_download_bytes: NETWORK_BYTES_UNKNOWN,
            network_upload_bytes: NETWORK_BYTES_UNKNOWN,
            minimum_network_chunk_bytes: NETWORK_BYTES_UNKNOWN,
            min_latency: 0,
            max_execution_delay: 0,
            periodic: false,
            persisted: true,
            interval: 0,
            flex: 0,
            initial_backoff: DEFAULT_INITIAL_BACKOFF_MILLIS,
            backoff_policy: BACKOFF_POLICY_EXPONENTIAL,
            has_early_constraint: false,
            has_late_constraint: false,
            bias: BIAS_DEFAULT,
            priority: PRIORITY_DEFAULT,
            flags: 0,
            debug_tags: Vec::new(),
            trace_tag: None,
        }
    }

    /// `Builder.setPeriodic(interval, flex)`, both raised to their minimums.
    pub fn set_periodic(&mut self, interval: i64, flex: i64) {
        let interval = interval.max(MIN_PERIOD_MILLIS);
        let min_flex = (5 * interval / 100).max(MIN_FLEX_MILLIS);
        self.periodic = true;
        self.interval = interval;
        self.flex = flex.max(min_flex);
        self.has_early_constraint = true;
        self.has_late_constraint = true;
    }

    /// `Builder.setBackoffCriteria`, the delay raised to its minimum.
    pub fn set_backoff(&mut self, initial: i64, policy: i32) {
        self.initial_backoff = initial.max(MIN_BACKOFF_MILLIS);
        self.backoff_policy = policy;
    }

    pub fn is_expedited(&self) -> bool {
        self.flags & FLAG_EXPEDITED != 0
    }

    pub fn is_user_initiated(&self) -> bool {
        self.flags & FLAG_USER_INITIATED != 0
    }

    pub fn is_prefetch(&self) -> bool {
        self.flags & FLAG_PREFETCH != 0
    }

    /// Whether its transient extras are empty (`Bundle.isEmpty`): a
    /// parcelled bundle has a length only when it has entries.
    fn transient_extras_empty(&self) -> bool {
        self.transient_extras.parcel().reader().read_i32().is_ok_and(|n| n <= 0)
    }

    /// The handles of the binders among its parcelled parts.
    pub fn handles(&self) -> Vec<u32> {
        let clip = self.clip.iter().flat_map(|(raw, _)| raw.handles());
        self.transient_extras.handles().chain(clip).collect()
    }

    /// `JobInfo.equals`. Two parcelled clips are never the same object.
    pub fn same_as(&self, o: &JobInfo) -> bool {
        let mut tags = self.debug_tags.clone();
        let mut others = o.debug_tags.clone();
        tags.sort();
        others.sort();
        self.id == o.id
            && self.extras == o.extras
            && self.transient_extras == o.transient_extras
            && self.clip.is_none()
            && o.clip.is_none()
            && self.service == o.service
            && self.constraint_flags == o.constraint_flags
            && self.trigger_content_uris == o.trigger_content_uris
            && self.trigger_content_update_delay == o.trigger_content_update_delay
            && self.trigger_content_max_delay == o.trigger_content_max_delay
            && self.has_early_constraint == o.has_early_constraint
            && self.has_late_constraint == o.has_late_constraint
            && self.network == o.network
            && self.network_download_bytes == o.network_download_bytes
            && self.network_upload_bytes == o.network_upload_bytes
            && self.minimum_network_chunk_bytes == o.minimum_network_chunk_bytes
            && self.min_latency == o.min_latency
            && self.max_execution_delay == o.max_execution_delay
            && self.periodic == o.periodic
            && self.persisted == o.persisted
            && self.interval == o.interval
            && self.flex == o.flex
            && self.initial_backoff == o.initial_backoff
            && self.backoff_policy == o.backoff_policy
            && self.bias == o.bias
            && self.priority == o.priority
            && self.flags == o.flags
            && tags == others
            && self.trace_tag == o.trace_tag
    }

    /// `JobInfo.enforceValidity(disallowPrefetchDeadlines,
    /// rejectNegativeNetworkEstimates, enforceMinimumTimeWindows,
    /// rejectNegativeDelaysAndDeadlines)`; the debug and trace tags
    /// trimmed as it trims them. `enforce_minimum_time_windows` is the
    /// compat change and the aconfig flag together.
    pub fn enforce_validity(
        &mut self,
        disallow_prefetch_deadlines: bool,
        reject_negative_network_estimates: bool,
        enforce_minimum_time_windows: bool,
        reject_negative_delays_and_deadlines: bool,
    ) -> std::result::Result<(), Exception> {
        let fail = |m: String| Err(Exception::illegal_argument(m));
        let (down, up, chunk) =
            (self.network_download_bytes, self.network_upload_bytes, self.minimum_network_chunk_bytes);
        if (down > 0 || up > 0 || chunk > 0) && self.network.is_none() {
            return fail("Can't provide estimated network usage without requiring a network".into());
        }
        if self.network.is_some() && reject_negative_network_estimates {
            if up != NETWORK_BYTES_UNKNOWN && up < 0 {
                return fail(format!("Invalid network upload bytes: {up}"));
            }
            if down != NETWORK_BYTES_UNKNOWN && down < 0 {
                return fail(format!("Invalid network download bytes: {down}"));
            }
        }
        check_chunk(down, up, chunk)?;
        if reject_negative_delays_and_deadlines {
            if self.min_latency < 0 {
                return fail(format!("Minimum latency is negative: {}", self.min_latency));
            }
            if self.max_execution_delay < 0 {
                return fail(format!("Override deadline is negative: {}", self.max_execution_delay));
            }
        }
        let has_deadline = self.max_execution_delay != 0;
        let has_triggers = self.trigger_content_uris.is_some();
        if self.periodic {
            if has_deadline {
                return fail("Can't call setOverrideDeadline() on a periodic job.".into());
            }
            if self.min_latency != 0 {
                return fail("Can't call setMinimumLatency() on a periodic job".into());
            }
            if has_triggers {
                return fail("Can't call addTriggerContentUri() on a periodic job".into());
            }
        }
        if disallow_prefetch_deadlines && has_deadline && self.is_prefetch() {
            return fail("Can't call setOverrideDeadline() on a prefetch job.".into());
        }
        if self.persisted {
            // A request with a specifier is not read (`Unsupported`).
            if has_triggers {
                return fail("Can't call addTriggerContentUri() on a persisted job".into());
            }
            if !self.transient_extras_empty() {
                return fail("Can't call setTransientExtras() on a persisted job".into());
            }
            if self.clip.is_some() {
                return fail("Can't call setClipData() on a persisted job".into());
            }
        }
        if self.flags & FLAG_IMPORTANT_WHILE_FOREGROUND != 0 {
            if self.has_early_constraint {
                return fail("An important while foreground job cannot have a time delay".into());
            }
            if self.priority != PRIORITY_HIGH && self.priority != PRIORITY_DEFAULT {
                return fail("An important while foreground job must be high or default priority. Don't mark unimportant tasks as important while foreground.".into());
            }
        }
        let (expedited, user_initiated) = (self.is_expedited(), self.is_user_initiated());
        match self.priority {
            PRIORITY_MAX if !(expedited || user_initiated) => {
                return fail("Only expedited or user-initiated jobs can have max priority".into());
            }
            PRIORITY_HIGH if self.is_prefetch() => return fail("Prefetch jobs cannot be high priority".into()),
            PRIORITY_HIGH if self.periodic => return fail("Periodic jobs cannot be high priority".into()),
            PRIORITY_MAX | PRIORITY_HIGH | PRIORITY_DEFAULT | PRIORITY_LOW | PRIORITY_MIN => {}
            p => return fail(format!("Invalid priority level provided: {p}")),
        }
        let functional = self.network.is_some() || self.constraint_flags != 0 || has_triggers;
        if self.has_late_constraint && !self.periodic && functional {
            let start = if self.has_early_constraint { self.min_latency } else { 0 };
            if self.max_execution_delay - start < MIN_ALLOWED_TIME_WINDOW_MILLIS && enforce_minimum_time_windows {
                let (package, class) = &self.service;
                return fail(format!(
                    "Time window too short. Constraints unlikely to be satisfied. Increase deadline to a reasonable duration. Job '{package}/{class}#{}' has delay={start}, deadline={}",
                    self.id, self.max_execution_delay
                ));
            }
        }
        if expedited {
            if self.has_early_constraint {
                return fail("An expedited job cannot have a time delay".into());
            }
            if self.has_late_constraint {
                return fail("An expedited job cannot have a deadline".into());
            }
            if self.periodic {
                return fail("An expedited job cannot be periodic".into());
            }
            if user_initiated {
                return fail("An expedited job cannot be user-initiated".into());
            }
            if self.priority != PRIORITY_MAX && self.priority != PRIORITY_HIGH {
                return fail("An expedited job must be high or max priority. Don't use expedited jobs for unimportant tasks.".into());
            }
            if self.constraint_flags & !CONSTRAINT_FLAG_STORAGE_NOT_LOW != 0
                || self.flags & !(FLAG_EXPEDITED | FLAG_EXEMPT_FROM_APP_STANDBY) != 0
            {
                return fail("An expedited job can only have network and storage-not-low constraints".into());
            }
            if has_triggers {
                return fail("Can't call addTriggerContentUri() on an expedited job".into());
            }
        }
        if user_initiated {
            if self.has_early_constraint {
                return fail("A user-initiated job cannot have a time delay".into());
            }
            if self.has_late_constraint {
                return fail("A user-initiated job cannot have a deadline".into());
            }
            if self.periodic {
                return fail("A user-initiated job cannot be periodic".into());
            }
            if self.is_prefetch() {
                return fail("A user-initiated job cannot also be a prefetch job".into());
            }
            if self.priority != PRIORITY_MAX {
                return fail("A user-initiated job must be max priority.".into());
            }
            if self.constraint_flags & CONSTRAINT_FLAG_DEVICE_IDLE != 0 {
                return fail("A user-initiated job cannot have a device-idle constraint".into());
            }
            if has_triggers {
                return fail("Can't call addTriggerContentUri() on a user-initiated job".into());
            }
            if self.network.is_none() {
                return fail("A user-initiated data transfer job must specify a valid network type".into());
            }
        }
        if self.debug_tags.len() > MAX_NUM_DEBUG_TAGS {
            return fail(format!("Can't have more than {MAX_NUM_DEBUG_TAGS} tags"));
        }
        let mut tags = Vec::new();
        for tag in &self.debug_tags {
            let tag = validate_debug_tag(tag)?;
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        self.debug_tags = tags;
        validate_trace_tag(self.trace_tag.as_deref())?;
        Ok(())
    }
}

/// `enforceValidity`'s check of a minimum chunk against the estimates.
fn check_chunk(down: i64, up: i64, chunk: i64) -> std::result::Result<(), Exception> {
    let transfer = if up == NETWORK_BYTES_UNKNOWN {
        down
    } else {
        up + if down == NETWORK_BYTES_UNKNOWN { 0 } else { down }
    };
    if chunk != NETWORK_BYTES_UNKNOWN && transfer != NETWORK_BYTES_UNKNOWN && chunk > transfer {
        return Err(Exception::illegal_argument("Minimum chunk size can't be greater than estimated network usage"));
    }
    if chunk != NETWORK_BYTES_UNKNOWN && chunk <= 0 {
        return Err(Exception::illegal_argument("Minimum chunk size must be positive"));
    }
    Ok(())
}

/// `JobInfo.validateDebugTag`: the tag trimmed.
pub fn validate_debug_tag(tag: &str) -> std::result::Result<String, Exception> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(Exception::illegal_argument("debug tag cannot be empty"));
    }
    if tag.encode_utf16().count() > MAX_DEBUG_TAG_LENGTH {
        return Err(Exception::illegal_argument(format!(
            "debug tag cannot be more than {MAX_DEBUG_TAG_LENGTH} characters"
        )));
    }
    Ok(tag.to_string())
}

/// `JobInfo.validateTraceTag`.
fn validate_trace_tag(tag: Option<&str>) -> std::result::Result<(), Exception> {
    let Some(tag) = tag else { return Ok(()) };
    let tag = tag.trim();
    if tag.is_empty() {
        return Err(Exception::illegal_argument("trace tag cannot be empty"));
    }
    if tag.encode_utf16().count() > MAX_TRACE_TAG_LENGTH {
        return Err(Exception::illegal_argument(format!(
            "traceTag tag cannot be more than {MAX_TRACE_TAG_LENGTH} characters"
        )));
    }
    if tag.contains(['|', '\n', '\0']) {
        return Err(Exception::illegal_argument("Trace tag cannot contain |, \\n, or \\0"));
    }
    Ok(())
}

impl WriteParcelable for JobInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.id);
        write_persistable(p, &self.extras);
        self.transient_extras.write(p);
        match &self.clip {
            Some((clip, grants)) => {
                p.write_i32(1);
                clip.write(p);
                p.write_i32(*grants);
            }
            None => p.write_i32(0),
        }
        p.write_string16(Some(COMPONENT_NAME));
        p.write_string16(Some(&self.service.0));
        p.write_string16(Some(&self.service.1));
        p.write_i32(self.constraint_flags);
        match &self.trigger_content_uris {
            Some(raw) => raw.write(p),
            None => p.write_i32(-1),
        }
        p.write_i64(self.trigger_content_update_delay);
        p.write_i64(self.trigger_content_max_delay);
        match &self.network {
            Some(n) => {
                p.write_i32(1);
                n.write_to(p);
            }
            None => p.write_i32(0),
        }
        p.write_i64(self.network_download_bytes);
        p.write_i64(self.network_upload_bytes);
        p.write_i64(self.minimum_network_chunk_bytes);
        p.write_i64(self.min_latency);
        p.write_i64(self.max_execution_delay);
        p.write_i32(self.periodic as i32);
        p.write_i32(self.persisted as i32);
        p.write_i64(self.interval);
        p.write_i64(self.flex);
        p.write_i64(self.initial_backoff);
        p.write_i32(self.backoff_policy);
        p.write_i32(self.has_early_constraint as i32);
        p.write_i32(self.has_late_constraint as i32);
        p.write_i32(self.bias);
        p.write_i32(self.priority);
        p.write_i32(self.flags);
        p.write_i32(self.debug_tags.len() as i32);
        for tag in &self.debug_tags {
            p.write_string16(Some(tag));
        }
        p.write_string16(self.trace_tag.as_deref());
    }
}

/// `JobWorkItem`.
#[derive(Clone)]
pub struct JobWorkItem {
    pub intent: Option<KeptIntent>,
    pub extras: Bundle,
    pub network_download_bytes: i64,
    pub network_upload_bytes: i64,
    pub minimum_chunk_bytes: i64,
    pub delivery_count: i32,
    pub work_id: i32,
}

/// A `JobWorkItem` as it arrives in a parcel, its intent not yet kept.
pub struct ReadWorkItem {
    intent: Option<RawIntent>,
    extras: Bundle,
    down: i64,
    up: i64,
    chunk: i64,
    delivery_count: i32,
    work_id: i32,
}

impl ReadParcelable for ReadWorkItem {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let intent = if r.read_i32()? != 0 { Some(RawIntent::read_from(r)?) } else { None };
        Ok(ReadWorkItem {
            intent,
            extras: read_persistable(r)?,
            down: r.read_i64()?,
            up: r.read_i64()?,
            chunk: r.read_i64()?,
            delivery_count: r.read_i32()?,
            work_id: r.read_i32()?,
        })
    }
}

impl ReadWorkItem {
    /// The item, its intent's binders held.
    pub fn keep(self, process: &LocalProcess) -> JobWorkItem {
        JobWorkItem {
            intent: self.intent.map(|i| KeptIntent::new(i, process)),
            extras: self.extras,
            network_download_bytes: self.down,
            network_upload_bytes: self.up,
            minimum_chunk_bytes: self.chunk,
            delivery_count: self.delivery_count,
            work_id: self.work_id,
        }
    }

    /// `Intent.hasFileDescriptors` of its intent.
    pub fn has_fds(&self) -> bool {
        self.intent.as_ref().is_some_and(RawIntent::has_fds)
    }
}

impl JobWorkItem {
    /// `JobWorkItem.Builder().build()` of what `JobStore` persisted.
    pub fn persisted(extras: Bundle, delivery_count: i32, down: i64, up: i64, chunk: i64) -> JobWorkItem {
        JobWorkItem {
            intent: None,
            extras,
            network_download_bytes: down,
            network_upload_bytes: up,
            minimum_chunk_bytes: chunk,
            delivery_count,
            work_id: 0,
        }
    }

    /// `JobWorkItem.enforceValidity`.
    pub fn enforce_validity(&self, reject_negative_network_estimates: bool) -> std::result::Result<(), Exception> {
        let (down, up) = (self.network_download_bytes, self.network_upload_bytes);
        if reject_negative_network_estimates {
            if up != NETWORK_BYTES_UNKNOWN && up < 0 {
                return Err(Exception::illegal_argument(format!("Invalid network upload bytes: {up}")));
            }
            if down != NETWORK_BYTES_UNKNOWN && down < 0 {
                return Err(Exception::illegal_argument(format!("Invalid network download bytes: {down}")));
            }
        }
        check_chunk(down, up, self.minimum_chunk_bytes)
    }

    /// Its intent's flags.
    pub fn intent_flags(&self) -> Option<i32> {
        self.intent.as_ref().map(|k| k.intent.intent.flags)
    }
}

impl WriteParcelable for JobWorkItem {
    fn write_to(&self, p: &mut Parcel) {
        match &self.intent {
            Some(k) => {
                p.write_i32(1);
                k.intent.write_to(p);
            }
            None => p.write_i32(0),
        }
        write_persistable(p, &self.extras);
        p.write_i64(self.network_download_bytes);
        p.write_i64(self.network_upload_bytes);
        p.write_i64(self.minimum_chunk_bytes);
        p.write_i32(self.delivery_count);
        p.write_i32(self.work_id);
    }
}

/// The `JobParameters` a job is started and stopped with.
pub struct JobParameters<'a> {
    pub callback: Binder,
    pub namespace: Option<&'a str>,
    pub job: &'a JobInfo,
    pub override_deadline_expired: bool,
    pub expedited: bool,
    pub user_initiated: bool,
    /// The changes that triggered it (`changedUris`,
    /// `changedAuthorities`).
    pub triggered_uris: Option<&'a [Uri]>,
    pub triggered_authorities: Option<&'a [String]>,
    pub stop_reason: i32,
    pub internal_stop_reason: i32,
    pub debug_stop_reason: Option<&'a str>,
}

impl WriteParcelable for JobParameters<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.job.id);
        p.write_string16(self.namespace);
        write_persistable(p, &self.job.extras);
        self.job.transient_extras.write(p);
        match &self.job.clip {
            Some((clip, grants)) => {
                p.write_i32(1);
                clip.write(p);
                p.write_i32(*grants);
            }
            None => p.write_i32(0),
        }
        p.write_binder(Some(self.callback));
        p.write_i32(self.override_deadline_expired as i32);
        p.write_bool(self.expedited);
        p.write_bool(self.user_initiated);
        // `writeTypedArray`, `writeStringArray`.
        match self.triggered_uris {
            Some(uris) => {
                p.write_i32(uris.len() as i32);
                for uri in uris {
                    p.write_i32(1);
                    uri.write(p);
                }
            }
            None => p.write_i32(-1),
        }
        match self.triggered_authorities {
            Some(authorities) => {
                p.write_i32(authorities.len() as i32);
                for a in authorities {
                    p.write_string16(Some(a));
                }
            }
            None => p.write_i32(-1),
        }
        p.write_i32(0); // no network
        p.write_i32(self.stop_reason);
        p.write_i32(self.internal_stop_reason);
        p.write_string16(self.debug_stop_reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::restrictions::persistable::Value;

    /// What `JobInfo.Builder(7, new ComponentName("com.example",
    /// "com.example.Sync")).setRequiredNetworkType(NETWORK_TYPE_ANY)
    /// .setRequiresCharging(true).setPeriodic(HOUR).build().writeToParcel`
    /// writes, extras `{count: 3}`, from an app of uid 10123.
    fn written() -> Parcel {
        let mut p = Parcel::new();
        p.write_i32(7);
        let extras = Bundle { entries: vec![(Some("count".into()), Value::Int(3))] };
        p.write_raw(extras.parcel().unwrap().data(), &[]);
        p.write_i32(0); // transient extras: Bundle.EMPTY
        p.write_i32(0); // no clip
        p.write_string16(Some(COMPONENT_NAME));
        p.write_string16(Some("com.example"));
        p.write_string16(Some("com.example.Sync"));
        p.write_i32(CONSTRAINT_FLAG_CHARGING);
        p.write_i32(-1); // no trigger URIs
        p.write_i64(-1);
        p.write_i64(-1);
        p.write_i32(1); // a network request:
        p.write_i64((1 << 12) | (1 << 16) | (1 << 14) | (1 << 28)); // INTERNET, VALIDATED, TRUSTED, NOT_VCN_MANAGED
        p.write_i64(0);
        p.write_i64(0);
        p.write_i32(0);
        p.write_i32(0);
        p.write_string16(None);
        p.write_string16(None);
        p.write_i32(i32::MIN); // SIGNAL_STRENGTH_UNSPECIFIED
        p.write_i32(1); // the app's uid
        p.write_string16(Some("android.net.UidRange"));
        p.write_i32(10123);
        p.write_i32(10123);
        p.write_i32(0); // allowed uids
        p.write_string16(None);
        p.write_bool(false);
        p.write_i32(0); // administrator uids
        p.write_i32(-1);
        p.write_i32(-1);
        p.write_string16(None);
        p.write_i32(0); // sub ids
        p.write_i32(-1); // underlying networks
        p.write_i32(0);
        p.write_i32(0);
        p.write_bool(false);
        p.write_i32(TYPE_NONE);
        p.write_i32(REQUEST_ID_UNSET);
        p.write_string16(Some("NONE"));
        for v in [NETWORK_BYTES_UNKNOWN, NETWORK_BYTES_UNKNOWN, NETWORK_BYTES_UNKNOWN, 0, 0] {
            p.write_i64(v);
        }
        p.write_i32(1); // periodic
        p.write_i32(0); // not persisted
        p.write_i64(60 * 60 * 1000);
        p.write_i64(60 * 60 * 1000);
        p.write_i64(DEFAULT_INITIAL_BACKOFF_MILLIS);
        p.write_i32(BACKOFF_POLICY_EXPONENTIAL);
        p.write_i32(1);
        p.write_i32(1);
        p.write_i32(BIAS_DEFAULT);
        p.write_i32(PRIORITY_DEFAULT);
        p.write_i32(0);
        p.write_i32(1);
        p.write_string16(Some("sync"));
        p.write_string16(None);
        p
    }

    #[test]
    fn job_info_reads_and_writes_as_the_original() {
        let p = written();
        let mut r = p.reader();
        let Ok(job) = JobInfo::read(&mut r).unwrap() else { panic!("not read") };
        assert_eq!(r.remaining(), 0);
        assert_eq!((job.id, job.service.0.as_str(), job.constraint_flags), (7, "com.example", CONSTRAINT_FLAG_CHARGING));
        assert!(job.periodic && job.has_early_constraint && !job.persisted);
        assert_eq!(job.extras.entries, vec![(Some("count".into()), Value::Int(3))]);
        let network = job.network.as_ref().unwrap();
        assert_eq!(unpack_bits(network.caps.capabilities), [12, 14, 16, 28]);
        assert_eq!(job.debug_tags, ["sync"]);
        let mut back = Parcel::new();
        job.write_to(&mut back);
        assert_eq!(back.data(), p.data());
        assert!(job.same_as(&job.clone()));
    }

    #[test]
    fn a_network_specifier_is_not_read() {
        let mut p = Parcel::new();
        for _ in 0..3 {
            p.write_i64(0);
        }
        p.write_i32(0);
        p.write_i32(0);
        p.write_string16(Some("android.net.wifi.WifiNetworkSpecifier"));
        assert!(matches!(read_request(&mut p.reader()), Ok(Err(Unsupported(_)))));
    }

    /// `JobStore`'s rebuilt request: what was persisted, with
    /// NOT_RESTRICTED dropped from a request of restricted capabilities.
    #[test]
    fn a_persisted_request_is_rebuilt_as_the_builder_builds_it() {
        let r = restored_request(&[12, 13, 16], &[15], &[1]);
        assert_eq!(unpack_bits(r.caps.capabilities), [12, 13, 16]);
        assert_eq!((unpack_bits(r.caps.forbidden), unpack_bits(r.caps.transports)), (vec![15], vec![1]));
        assert_eq!(r.caps.uids, None);
        let ims = restored_request(&[4, 13], &[], &[]);
        assert_eq!(unpack_bits(ims.caps.capabilities), [4]);
        assert_eq!(request_for_uid(&r, 10123).caps.uids, Some(vec![(10123, 10123)]));
    }

    #[test]
    fn validity_is_enforced_as_the_original_enforces_it() {
        let p = written();
        let Ok(mut job) = JobInfo::read(&mut p.reader()).unwrap() else { panic!() };
        assert!(job.enforce_validity(true, true, true, true).is_ok());
        job.max_execution_delay = 1;
        let e = job.enforce_validity(true, true, true, true).unwrap_err();
        assert_eq!(e.message, "Can't call setOverrideDeadline() on a periodic job.");
        let mut job = JobInfo::persisted_builder(1, "p".into(), "p.S".into());
        job.flags = FLAG_EXPEDITED;
        assert_eq!(
            job.enforce_validity(true, true, true, true).unwrap_err().message,
            "An expedited job must be high or max priority. Don't use expedited jobs for unimportant tasks."
        );
        job.priority = PRIORITY_HIGH;
        assert!(job.enforce_validity(true, true, true, true).is_ok());
        job.debug_tags = vec!["  a ".into(), "a".into()];
        job.enforce_validity(true, true, true, true).unwrap();
        assert_eq!(job.debug_tags, ["a"]);
    }

    /// `JobParameters.writeToParcel` of a job just started.
    #[test]
    fn parameters_are_written_as_the_original_writes_them() {
        let job = JobInfo::persisted_builder(4, "p".into(), "p.S".into());
        let params = JobParameters {
            callback: Binder::Local(0x10),
            namespace: Some("ns"),
            job: &job,
            override_deadline_expired: true,
            expedited: false,
            user_initiated: false,
            triggered_uris: None,
            triggered_authorities: None,
            stop_reason: STOP_REASON_UNDEFINED,
            internal_stop_reason: INTERNAL_STOP_REASON_UNKNOWN,
            debug_stop_reason: None,
        };
        let mut p = Parcel::new();
        params.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 4);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("ns"));
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap(), r.read_i32().unwrap()), (0, 0, 0));
        assert_eq!(r.read_binder().unwrap(), Some(Binder::Local(0x10)));
        let ints: Vec<i32> = (0..8).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(ints, [1, 0, 0, -1, -1, 0, 0, -1]);
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.remaining(), 0);
    }

    /// The changes that triggered a content trigger job, as
    /// `writeTypedArray` and `writeStringArray` write them.
    #[test]
    fn triggered_changes_are_written_with_the_parameters() {
        let job = JobInfo::persisted_builder(4, "p".into(), "p.S".into());
        let uris = [Uri::parse("content://com.android.contacts/contacts/7")];
        let authorities = ["com.android.contacts".to_string()];
        let params = JobParameters {
            callback: Binder::Local(0x10),
            namespace: None,
            job: &job,
            override_deadline_expired: false,
            expedited: false,
            user_initiated: false,
            triggered_uris: Some(&uris),
            triggered_authorities: Some(&authorities),
            stop_reason: STOP_REASON_UNDEFINED,
            internal_stop_reason: INTERNAL_STOP_REASON_UNKNOWN,
            debug_stop_reason: None,
        };
        let mut p = Parcel::new();
        params.write_to(&mut p);
        let mut r = p.reader();
        r.read_i32().unwrap();
        r.read_string16().unwrap();
        (0..3).for_each(|_| assert_eq!(r.read_i32().unwrap(), 0));
        r.read_binder().unwrap();
        (0..3).for_each(|_| {
            r.read_i32().unwrap();
        });
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 1));
        let uri = Uri::read(&mut r, &mut crate::package::intent_filter::Plain).unwrap().unwrap();
        assert_eq!(uri.as_str(), "content://com.android.contacts/contacts/7");
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("com.android.contacts"));
    }

    #[test]
    fn work_items_round_trip() {
        let mut p = Parcel::new();
        p.write_i32(0); // no intent
        p.write_i32(0); // empty extras
        p.write_i64(100);
        p.write_i64(-1);
        p.write_i64(-1);
        p.write_i32(2);
        p.write_i32(9);
        let item = ReadWorkItem::read_from(&mut p.reader()).unwrap();
        let item = JobWorkItem {
            intent: None,
            extras: item.extras,
            network_download_bytes: item.down,
            network_upload_bytes: item.up,
            minimum_chunk_bytes: item.chunk,
            delivery_count: item.delivery_count,
            work_id: item.work_id,
        };
        let mut back = Parcel::new();
        item.write_to(&mut back);
        assert_eq!(back.data(), p.data());
        assert!(item.enforce_validity(true).is_ok());
    }
}
