//! Broadcasts for the app host (`BroadcastController`,
//! `BroadcastQueueModernImpl` and `BroadcastSkipPolicy` at
//! `android-16.0.0_r1`): the receivers apps register, sticky broadcasts,
//! and a broadcast's delivery to the receivers it matches, registered and
//! manifest ones merged by priority, each process's in the order sent. A
//! manifest receiver's process is started for it, and it is sent
//! (`scheduleReceiver`) once the process attached; its process waits for
//! its `finishReceiver` before its next. An ordered broadcast, or one with
//! a result receiver, goes to one receiver at a time, each finishing before
//! the next. A receiver is checked as a broadcast is enqueued, a
//! preflight's checks (`shouldSkipAtEnqueueMessage`) with
//! `avoid_note_op_at_enqueue`, its permissions' and the broadcast's app ops
//! checked rather than noted, and checked again as it is delivered
//! (`shouldSkipReceiver`), its app ops noted then: a receiver whose
//! permission or op went meanwhile is skipped, its delivery finished.
//! Each process's queue waits its turn to run (`updateRunningListLocked`):
//! at most `MAX_RUNNING_PROCESS_QUEUES` run at once, and one more for an
//! urgent broadcast, and of the processes not running only one is started
//! at a time (`mRunningColdStart`), its receivers' deliveries held until
//! its turn.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet, VecDeque};
use std::slice;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Strong};
use aim_binder_host::parcel::{
    Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_app_iactivitymanager as am;
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::android_content_iintentreceiver as intent_receiver;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::app_op_infos::{APP_OP_INFOS, NUM_OP};
use super::appops_service::{self, MODE_ALLOWED, MODE_IGNORED};
use super::kept::{By, KeptBundle, RawBundle, RawIntent};
use super::permission_checker::{self, AttributionSourceState};
use super::{AppHost, PER_USER_RANGE, PROCESS_STATE_TOP, is_isolated};
use crate::bundle::{self, Value};
use crate::package::component_resolver::Info;
use crate::package::info::ActivityInfo;
use crate::package::intent::{FLAG_EXCLUDE_STOPPED_PACKAGES, Intent};
use crate::package::intent_filter::{IntentFilter, Plain};

/// `Context.RECEIVER_*`.
const RECEIVER_VISIBLE_TO_INSTANT_APPS: i32 = 0x1;
const RECEIVER_EXPORTED: i32 = 0x2;
const RECEIVER_NOT_EXPORTED: i32 = 0x4;
/// `Intent.FLAG_RECEIVER_*`, `EXTENDED_FLAG_FILTER_MISMATCH`.
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const FLAG_RECEIVER_NO_ABORT: i32 = 0x0800_0000;
const FLAG_RECEIVER_BOOT_UPGRADE: i32 = 0x0200_0000;
const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
const FLAG_RECEIVER_FROM_SHELL: i32 = 0x0040_0000;
const FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS: i32 = 0x0020_0000;
const EXTENDED_FLAG_FILTER_MISMATCH: i32 = 1;
/// `BroadcastOptions`' flags.
const OPTION_ALLOW_BACKGROUND_ACTIVITY_STARTS: i32 = 1 << 1;
const OPTION_REQUIRE_COMPAT_CHANGE_ENABLED: i32 = 1 << 2;
const OPTION_IS_ALARM_BROADCAST: i32 = 1 << 3;
const OPTION_SHARE_IDENTITY: i32 = 1 << 4;
const OPTION_INTERACTIVE: i32 = 1 << 5;
/// `Intent.FLAG_RECEIVER_FOREGROUND`.
const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
/// `Intent.FLAG_RECEIVER_EXCLUDE_BACKGROUND`, `ActivityInfo.FLAG_*`.
const FLAG_RECEIVER_EXCLUDE_BACKGROUND: i32 = 0x0080_0000;
const FLAG_SINGLE_USER: i32 = 0x4000_0000;
const FLAG_VISIBLE_TO_INSTANT_APP: i32 = 0x0010_0000;
/// `PermissionInfo.PROTECTION_SIGNATURE`, `PROTECTION_FLAG_PRIVILEGED`.
const PROTECTION_SIGNATURE: i32 = 2;
const PROTECTION_FLAG_PRIVILEGED: i32 = 0x10;
/// `AppOpsManager.OP_RUN_IN_BACKGROUND`.
const OP_RUN_IN_BACKGROUND: i32 = 63;
/// `Context.DEVICE_ID_DEFAULT`.
const DEVICE_ID_DEFAULT: i32 = 0;
/// The aconfig flag whose enqueue checks are a preflight's.
const AVOID_NOTE_OP_AT_ENQUEUE: &str = "com.android.server.am.avoid_note_op_at_enqueue";
/// `IntentFilter.SYSTEM_HIGH_PRIORITY`.
const SYSTEM_HIGH_PRIORITY: i32 = 1000;
const MAX_RECEIVERS_ALLOWED_PER_APP: usize = 1000;
/// The compat changes broadcasts follow.
const DYNAMIC_RECEIVER_EXPLICIT_EXPORT_REQUIRED: i64 = 161_145_287;
const BLOCK_NULL_ACTION_INTENTS: i64 = 293_560_872;
const LOCK_DOWN_CLOSE_SYSTEM_DIALOGS: i64 = 174_664_365;
/// `ActivityManager.BROADCAST_*`, `Activity.RESULT_CANCELED`.
const BROADCAST_SUCCESS: i32 = 0;
const BROADCAST_STICKY_CANT_HAVE_PERMISSION: i32 = -1;
const BROADCAST_FAILED_USER_STOPPED: i32 = -2;
const RESULT_CANCELED: i32 = 0;
const USER_ALL: i32 = -1;
const INVALID_UID: i32 = -1;
/// `AppOpsManager.OP_NONE`.
const OP_NONE: i32 = -1;
/// `Process.*_UID`, `FIRST_APPLICATION_UID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const SHELL_UID: i32 = 2000;
const FIRST_APPLICATION_UID: i32 = 10_000;
/// `PermissionInfo.PROTECTION_MASK_BASE`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const BROADCAST_STICKY: &str = "android.permission.BROADCAST_STICKY";

/// A parcelled `IntentFilter`.
pub struct ParcelFilter(IntentFilter);

impl ReadParcelable for ParcelFilter {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        IntentFilter::read(r, &mut Plain).map(ParcelFilter)
    }
}

/// `registerReceiver[WithFeature]`'s arguments.
pub struct Registration {
    pub caller: Option<Binder>,
    pub caller_package: Option<String>,
    pub feature_id: Option<String>,
    pub receiver_id: Option<String>,
    pub receiver: Option<Binder>,
    pub filter: Option<ParcelFilter>,
    pub permission: Option<String>,
    pub user: i32,
    pub flags: i32,
}

/// `ReceiverList`: a receiver a process registered, with its filters.
struct Receiver {
    receiver: Strong,
    start_seq: i64,
    pid: i32,
    uid: i32,
    user: i32,
    filters: Vec<Filter>,
}

/// `BroadcastFilter`.
struct Filter {
    filter: IntentFilter,
    package: String,
    /// `featureId`, `receiverId`.
    feature_id: Option<String>,
    receiver_id: Option<String>,
    permission: Option<String>,
    instant_app: bool,
    visible_to_instant_apps: bool,
    exported: bool,
}

/// `BroadcastRecord`: what each of a broadcast's receivers is sent.
struct Record {
    intent: RawIntent,
    /// The binders among the intent, held.
    binders: Arc<Vec<Strong>>,
    /// The sender, its receivers checked for it at enqueue and delivery.
    sender: Sender,
    ordered: bool,
    /// `initialSticky`: a sticky broadcast sent on to a new receiver.
    initial_sticky: bool,
    user: i32,
    share_identity: bool,
    /// The result receivers start from.
    result_code: i32,
    result_data: Option<String>,
    result_extras: Option<KeptBundle>,
    /// `BroadcastOptions.getIdForResponseEvent`: 0 when none is recorded.
    id_for_response_event: i64,
    /// `BroadcastOptions.setTemporaryAppAllowlist`: each receiver's uid
    /// is allowed for as long when it is sent the broadcast.
    temp_allowlist: Option<TempAllowlist>,
    /// `BroadcastRecord.isUrgent`.
    urgent: bool,
}

/// `BroadcastRecord.calculateUrgent`: a foreground broadcast, an
/// interactive or an alarm's.
fn urgent(intent_flags: i32, option_flags: i32) -> bool {
    intent_flags & FLAG_RECEIVER_FOREGROUND != 0
        || option_flags & (OPTION_INTERACTIVE | OPTION_IS_ALARM_BROADCAST) != 0
}

/// A broadcast's temporary allowlist (`BroadcastOptions`'s duration,
/// type and reason code).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempAllowlist {
    pub duration_ms: i64,
    pub kind: i32,
    pub reason: i32,
}

/// `PowerExemptionManager.TEMPORARY_ALLOW_LIST_TYPE_*`.
pub(super) const TEMPORARY_ALLOW_LIST_TYPE_NONE: i32 = -1;
pub(super) const TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED: i32 = 0;

/// A manifest receiver a broadcast goes to (`ResolveInfo`): its
/// `ActivityInfo`, its process and uid, and its priority.
struct Manifest {
    info: ActivityInfo,
    process: String,
    uid: u32,
    priority: i32,
}

/// A registered receiver a broadcast goes to: its handle, the index of
/// the filter it matched (`BroadcastFilter`), its process's start.
#[derive(Clone, Copy)]
struct Registered {
    handle: u32,
    filter: usize,
    start_seq: i64,
}

/// A receiver a broadcast goes to: a registered one or a manifest one.
enum Target {
    Registered(Registered),
    Manifest(Arc<Manifest>),
}

/// `ActivityManager.APP_START_MODE_*`.
#[derive(PartialEq)]
enum StartMode {
    Normal,
    Delayed,
    DelayedRigid,
    Disabled,
}

/// `StickyBroadcast`.
struct Sticky {
    record: Arc<Record>,
    resolved_type: Option<String>,
}

/// A broadcast going to its receivers one after another, each finishing
/// (`finishReceiver`) before the next, then to its result receiver.
struct Chain {
    record: Arc<Record>,
    /// The receivers to come.
    targets: VecDeque<Target>,
    result_code: i32,
    result_data: Option<String>,
    result_extras: Option<KeptBundle>,
    aborted: bool,
    /// The result receiver, and the start of its process if an app's.
    result_to: Option<(Strong, Option<i64>)>,
    /// A system service's own result receiver (`IIntentReceiver.Stub`).
    done: Option<Done>,
}

/// What a system service of the app host runs when its ordered broadcast
/// has gone through its receivers.
pub type Done = Box<dyn FnOnce() + Send>;

/// A delivery a process waits for, by its receiver.
enum Delivery {
    /// Sent and assumed delivered.
    Once(Arc<Record>, Registered),
    /// A receiver of the chain with that id, which reports its finish.
    Chained(u64, Registered),
    /// A manifest receiver, of a chain if one, which reports its finish.
    Manifest(Arc<Record>, Arc<Manifest>, Option<u64>),
}

#[derive(Default)]
struct State {
    /// `mRegisteredReceivers`, in the order registered.
    receivers: Vec<Receiver>,
    stickies: Vec<Sticky>,
    chains: HashMap<u64, Chain>,
    next_chain: u64,
    /// Each process's deliveries, by start, in order.
    queues: HashMap<i64, VecDeque<Delivery>>,
    /// The processes whose first delivery, a chained or a manifest one, was
    /// sent and waits for its finish.
    busy: HashSet<i64>,
    /// The process queues with a delivery to send, in the order they became
    /// runnable (`mRunnableHead`).
    runnable: VecDeque<QueueId>,
    /// The deliveries to processes not running, each process's in order.
    cold: Vec<Cold>,
    /// The queue whose process is being started (`mRunningColdStart`).
    cold_start: Option<i64>,
}

/// A process queue (`BroadcastProcessQueue`): a process's, by its start,
/// or one not running, by its name and uid.
#[derive(Clone, PartialEq)]
enum QueueId {
    Warm(i64),
    Cold(String, u32),
}

/// The deliveries waiting for a process that does not run.
struct Cold {
    process: String,
    uid: u32,
    package: String,
    deliveries: VecDeque<Delivery>,
}

/// `BroadcastConstants.MAX_RUNNING_PROCESS_QUEUES` and
/// `EXTRA_RUNNING_URGENT_PROCESS_QUEUES`.
#[derive(Clone, Copy)]
struct Limits {
    max_running: usize,
    extra_urgent: usize,
}

/// A runnable queue as `updateRunningListLocked` weighs it: whether its
/// next broadcast is urgent (`isPendingUrgent`), and whether its process
/// is to be started (not `isProcessWarm`).
#[derive(Clone, Copy)]
struct Candidate {
    urgent: bool,
    cold: bool,
}

/// Which of the `runnable` queues, in their order, `updateRunningListLocked`
/// promotes to running next, with `running` queues running (`urgent` of
/// them delivering an urgent broadcast) and a process starting or not.
fn next_to_run(
    runnable: &[Candidate],
    running: usize,
    urgent: usize,
    cold_starting: bool,
    limits: Limits,
) -> Option<usize> {
    // The extra queue lets urgent broadcasts progress; running urgent ones
    // use it first.
    let used_extra = urgent.min(limits.extra_urgent);
    if running + used_extra >= limits.max_running + limits.extra_urgent {
        return None;
    }
    runnable
        .iter()
        .position(|c| (running < limits.max_running || c.urgent) && !(c.cold && cold_starting))
}

#[derive(Default)]
pub struct Broadcasts {
    state: Mutex<State>,
    limits: std::sync::OnceLock<Limits>,
}

/// A sender, as BroadcastSkipPolicy checks a receiver for it.
struct Sender {
    uid: i32,
    /// `callerPackage`, `callerFeatureId`.
    package: Option<String>,
    feature_id: Option<String>,
    /// The uid the `exported` check is of: a sticky broadcast's original
    /// sender's.
    original_uid: i32,
    instant: bool,
    required_permissions: Vec<String>,
    /// Whether the sender named no required permissions (rather than none).
    required_null: bool,
    excluded_permissions: Vec<String>,
    excluded_packages: Vec<String>,
    app_op: i32,
    require_compat_change: Option<(i64, bool)>,
}

impl Sender {
    /// The system's (`android`), requiring `required_permissions`.
    fn system(required_permissions: &[String]) -> Sender {
        Sender {
            uid: SYSTEM_UID,
            package: Some("android".into()),
            feature_id: None,
            original_uid: SYSTEM_UID,
            instant: false,
            required_permissions: required_permissions.to_vec(),
            required_null: required_permissions.is_empty(),
            excluded_permissions: Vec::new(),
            excluded_packages: Vec::new(),
            app_op: OP_NONE,
            require_compat_change: None,
        }
    }
}

/// What a sender's `BroadcastOptions` ask that the app host acts on.
#[derive(Default)]
struct Options {
    flags: i32,
    temporary_allowlist_duration: i64,
    temporary_allowlist_type: i32,
    temporary_allowlist_reason: i32,
    id_for_response_event: i64,
    require_compat_change: Option<i64>,
}

impl Options {
    fn read(options: Option<&RawBundle>) -> Result<Options, Exception> {
        let Some(options) = options else {
            return Ok(Options::default());
        };
        let mut p = Parcel::new();
        options.write_to(&mut p);
        let entries = bundle::read(&mut Reader::new(p.data(), &[]))
            .map_err(|_| Exception::illegal_argument("unreadable broadcast options"))?
            .unwrap_or_default();
        let long = |key: &str| match entries.get(key) {
            Some(Value::Long(v)) => Some(*v),
            _ => None,
        };
        let int = |key: &str| match entries.get(key) {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        };
        Ok(Options {
            flags: match entries.get("android:broadcast.flags") {
                Some(Value::Int(v)) => *v,
                _ => 0,
            },
            temporary_allowlist_duration: long("android:broadcast.temporaryAppAllowlistDuration")
                .unwrap_or(0),
            temporary_allowlist_type: int("android:broadcast.temporaryAppAllowlistType")
                .unwrap_or(TEMPORARY_ALLOW_LIST_TYPE_NONE),
            temporary_allowlist_reason: int("android:broadcast.temporaryAppAllowlistReasonCode")
                .unwrap_or(0),
            id_for_response_event: long("android:broadcast.idForResponseEvent").unwrap_or(0),
            // `CHANGE_INVALID` is no change.
            require_compat_change: long("android:broadcast.requireCompatChangeId")
                .filter(|&id| id != 0),
        })
    }
}

fn strings(list: Option<Vec<Option<String>>>) -> Vec<String> {
    list.unwrap_or_default().into_iter().flatten().collect()
}

/// `scheduleReceiverWarmLocked`'s temporary allowlist of a receiving
/// `uid` (`tempAllowlistUidLocked`): foreground services allowed for the
/// broadcast's duration when its type allows them. Device idle's own
/// temporary allowlist and a freezer's delay are not the app host's, which
/// is never idle and freezes no process.
fn temp_allowlist(host: &AppHost, uid: i32, record: &Record) {
    let Some(t) = record.temp_allowlist else {
        return;
    };
    if t.kind == TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED {
        host.temp_allowlist_for_fgs(
            uid,
            std::time::Duration::from_millis(t.duration_ms.max(0) as u64),
            t.reason,
        );
    }
}

/// `UserHandle.isCore`.
pub(super) fn is_core(uid: i32) -> bool {
    uid >= 0 && uid.rem_euclid(PER_USER_RANGE as i32) < FIRST_APPLICATION_UID
}

/// `ActivityManager.canAccessUnexportedComponents`.
fn can_access_unexported(uid: i32) -> bool {
    matches!(uid.rem_euclid(PER_USER_RANGE as i32), ROOT_UID | SYSTEM_UID)
}

/// Whether `filter` takes `intent` (`IntentResolver.queryIntent`'s match
/// of a candidate): a filter is found by the intent's action, type or
/// scheme, so an intent with none of them finds none.
fn takes(
    filter: &IntentFilter,
    intent: &Intent,
    resolved_type: Option<&str>,
) -> Result<bool, Exception> {
    if intent.action.is_none() && resolved_type.is_none() && intent.scheme().is_none() {
        return Ok(false);
    }
    filter
        .matches(
            intent.action.as_deref(),
            resolved_type,
            intent.scheme(),
            intent.data.as_ref(),
            intent.categories.as_deref(),
            false,
            None,
        )
        .map(|m| m >= 0)
        .map_err(|e| Exception::illegal_argument(format!("{e:?}")))
}

/// `broadcastIntentLocked`'s merge of the manifest receivers into the
/// registered ones by priority, a registered one first on a tie.
fn merge(manifest: Vec<Arc<Manifest>>, registered: Vec<(i32, Registered)>) -> Vec<Target> {
    let mut merged = Vec::with_capacity(manifest.len() + registered.len());
    let mut manifest = manifest.into_iter().peekable();
    for (priority, r) in registered {
        while let Some(m) = manifest.next_if(|m| m.priority > priority) {
            merged.push(Target::Manifest(m));
        }
        merged.push(Target::Registered(r));
    }
    merged.extend(manifest.map(Target::Manifest));
    merged
}

/// `Intent.filterEquals`.
pub(super) fn filter_equals(a: &Intent, b: &Intent) -> bool {
    a.action == b.action
        && a.data.as_ref().map(|d| d.as_str()) == b.data.as_ref().map(|d| d.as_str())
        && a.ty == b.ty
        && a.identifier == b.identifier
        && a.package == b.package
        && a.component == b.component
        && a.categories == b.categories
}

impl AppHost {
    /// Whether `package` is an instant app for `user`.
    fn instant_app(&self, package: &str, user: i32) -> Result<bool, Exception> {
        let (state, _) = self.live_packages()?;
        Ok(state
            .packages
            .get(package)
            .and_then(|ps| ps.users.get(&user.max(0)))
            .is_some_and(|u| u.instant_app))
    }

    /// Whether `uid` holds `permission` (`checkComponentPermission` of no
    /// component).
    fn holds(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        self.check_component_permission(Some(permission), uid, -1, true)
    }

    /// `AppOpsManager.permissionToOpCode` of `permission`, if it has one.
    fn permission_op(&self, permission: &str) -> Result<Option<i32>, Exception> {
        let op = permission_checker::permission_to_op(self, permission)?;
        Ok((op != OP_NONE).then_some(op))
    }

    /// The mode of `op` for `uid`'s `package` and attribution `tag`: checked
    /// (`checkOpNoThrow`) for a preflight, noted (`noteOpNoThrow`)
    /// otherwise.
    fn broadcast_op(
        &self,
        preflight: bool,
        op: i32,
        uid: i32,
        package: Option<&str>,
        tag: Option<&str>,
    ) -> Result<i32, Exception> {
        if preflight {
            appops_service::mode(self, op, uid, package)
        } else {
            appops_service::note_from(self, SYSTEM_UID, op, uid, package, tag)
        }
    }

    /// `hasPermission`: whether each of `sources` holds `permission`, by
    /// `checkPermissionForPreflight` or `checkPermissionForDataDelivery`
    /// with `message`.
    fn has_permission(
        &self,
        permission: &str,
        message: &str,
        preflight: bool,
        sources: &[AttributionSourceState],
    ) -> Result<bool, Exception> {
        for source in sources {
            let granted = if preflight {
                permission_checker::preflight_source(self, permission, source)?
            } else {
                permission_checker::data_delivery(self, permission, source, Some(message))?
            };
            if !granted {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// An `AttributionSource` of `uid`'s `package` and attribution `tag`, of
/// `pid` if known.
fn attribution(uid: i32, pid: i32, package: &str, tag: Option<&str>) -> AttributionSourceState {
    AttributionSourceState {
        pid,
        uid,
        device_id: DEVICE_ID_DEFAULT,
        package: Some(package.to_string()),
        tag: tag.map(str::to_string),
        token: None,
        renounced: None,
        next: None,
    }
}

/// Whether an excluded permission excludes a receiver `granted` it: with an
/// app op, only when its mode (`checkOpNoThrow`) is allowed too.
fn excludes(
    granted: bool,
    op: Option<i32>,
    mode: impl FnOnce(i32) -> Result<i32, Exception>,
) -> Result<bool, Exception> {
    Ok(match op {
        Some(op) => granted && mode(op)? == MODE_ALLOWED,
        None => granted,
    })
}

/// The attribution tags a manifest receiver is checked with
/// (`createAttributionSourcesForResolveInfo`, `noteOpForManifestReceiver`):
/// each of its own, or none.
fn receiver_tags(tags: Option<&[Option<String>]>) -> Vec<Option<&str>> {
    match tags {
        Some(tags) if !tags.is_empty() => tags.iter().map(Option::as_deref).collect(),
        _ => vec![None],
    }
}

/// `AppOpsManager.isValidOp`.
fn valid_op(op: i32) -> bool {
    (0..NUM_OP).contains(&op)
}

/// `AppOpsManager.opToPublicName`, `opToName`: an op's names.
fn op_names(op: i32) -> (&'static str, String) {
    let public = APP_OP_INFOS[op as usize].name;
    (public, public.trim_start_matches("android:").to_uppercase())
}

/// The package of a manifest receiver.
fn manifest_package(m: &Manifest) -> String {
    m.info
        .info
        .application_info
        .item
        .package_name
        .clone()
        .unwrap_or_default()
}

impl Broadcasts {
    /// `registerReceiverWithFeature`: the first sticky broadcast the
    /// filter matches.
    pub fn register(
        &self,
        host: &AppHost,
        call: &Call<'_>,
        args: Registration,
    ) -> Result<Option<RawIntent>, Exception> {
        if is_isolated(call.sender_euid as i32) {
            return Err(Exception::security(
                "Isolated process not allowed to call registerReceiver",
            ));
        }
        let mut filter = args
            .filter
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "filter"))?
            .0;
        let Some((start_seq, package, uid, pid)) = host.app_of_thread(args.caller) else {
            return Ok(None);
        };
        let caller_package = args.caller_package.unwrap_or_default();
        if !is_core(uid) && caller_package != package {
            return Err(Exception::security(format!(
                "Given caller package {caller_package} is not running in process {pid}:{package}/{uid}"
            )));
        }
        let instant_app = host.instant_app(&caller_package, uid / PER_USER_RANGE as i32)?;
        let user = host.incoming_user(uid, args.user, false)?;
        // System internals hear these before apps unless they chose.
        // (An unset priority is no system one.)
        if is_core(uid) && filter.priority == 0 {
            let important = filter.actions.iter().any(|a| {
                [
                    "android.intent.action.USER_",
                    "android.intent.action.PACKAGE_",
                    "android.intent.action.UID_",
                    "android.intent.action.EXTERNAL_",
                    "android.bluetooth.",
                ]
                .iter()
                .any(|p| a.starts_with(p))
                    || a == "android.intent.action.ACTION_SHUTDOWN"
            });
            if important {
                filter.priority = SYSTEM_HIGH_PRIORITY;
            }
        }
        let (model, _) = host.live_packages()?;
        let mut only_protected = true;
        if filter.actions.is_empty() {
            only_protected = false;
        }
        for action in &filter.actions {
            only_protected &= crate::package::query::is_protected_broadcast(&model, action)
                .map_err(|e| host.unimplemented_named("package", e.0))?;
        }
        let mut flags = args.flags;
        if flags & RECEIVER_EXPORTED != 0 && flags & RECEIVER_NOT_EXPORTED != 0 {
            return Err(Exception::illegal_argument(
                "Receiver can't specify both RECEIVER_EXPORTED and RECEIVER_NOT_EXPORTEDflag",
            ));
        }
        let require_explicit =
            host.change_enabled(DYNAMIC_RECEIVER_EXPLICIT_EXPORT_REQUIRED, uid)?;
        if flags & RECEIVER_VISIBLE_TO_INSTANT_APPS != 0
            && flags & RECEIVER_NOT_EXPORTED != 0
            && require_explicit
        {
            return Err(Exception::illegal_argument(
                "Receiver can't specify both RECEIVER_VISIBLE_TO_INSTANT_APPS and RECEIVER_NOT_EXPORTED flag",
            ));
        }
        let explicit = flags & (RECEIVER_EXPORTED | RECEIVER_NOT_EXPORTED) != 0;
        if !only_protected {
            if args.receiver.is_none() && !explicit {
                flags |= RECEIVER_EXPORTED;
            } else if require_explicit && !explicit {
                return Err(Exception::security(format!(
                    "{caller_package}: One of RECEIVER_EXPORTED or RECEIVER_NOT_EXPORTED should be specified when a receiver isn't being registered exclusively for system broadcasts"
                )));
            } else if !require_explicit && flags & RECEIVER_NOT_EXPORTED == 0 {
                flags |= RECEIVER_EXPORTED;
            }
        } else if flags & RECEIVER_NOT_EXPORTED == 0 {
            flags |= RECEIVER_EXPORTED;
        }
        let exported = flags & RECEIVER_EXPORTED != 0;

        let mut st = self.state.lock().unwrap();
        // The stickies of the user and of all users that the filter matches.
        let mut matched: Vec<Arc<Record>> = Vec::new();
        for sticky in &st.stickies {
            let r = &sticky.record;
            if r.user != USER_ALL && r.user != uid / PER_USER_RANGE as i32 {
                continue;
            }
            let action_listed = match &r.intent.intent.action {
                Some(a) => filter.actions.iter().any(|f| f == a),
                None => filter.actions.is_empty(),
            };
            if !action_listed
                || instant_app && r.intent.intent.flags & FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS == 0
            {
                continue;
            }
            let i = &r.intent.intent;
            let m = filter
                .matches(
                    i.action.as_deref(),
                    sticky.resolved_type.as_deref(),
                    i.scheme(),
                    i.data.as_ref(),
                    i.categories.as_deref(),
                    false,
                    None,
                )
                .map_err(|e| Exception::illegal_argument(format!("{e:?}")))?;
            if m >= 0 {
                matched.push(r.clone());
            }
        }
        let Some(Binder::Handle(handle)) = args.receiver else {
            return Ok(matched.first().map(|r| r.intent.clone()));
        };
        match st
            .receivers
            .iter()
            .position(|r| r.receiver.handle == handle)
        {
            Some(at) => {
                let r = &st.receivers[at];
                let mismatch = if r.uid != uid {
                    Some(format!(
                        "uid {uid} was previously registered for uid {}",
                        r.uid
                    ))
                } else if r.pid != pid {
                    Some(format!(
                        "pid {pid} was previously registered for pid {}",
                        r.pid
                    ))
                } else if r.user != user {
                    Some(format!(
                        "user {user} was previously registered for user {}",
                        r.user
                    ))
                } else {
                    None
                };
                if let Some(m) = mismatch {
                    return Err(Exception::illegal_argument(format!(
                        "Receiver requested to register for {m} callerPackage is {caller_package}"
                    )));
                }
            }
            None => {
                let registered = st
                    .receivers
                    .iter()
                    .filter(|r| r.start_seq == start_seq)
                    .count();
                if registered >= MAX_RECEIVERS_ALLOWED_PER_APP {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!(
                            "Too many receivers, total of {registered}, registered for pid: {pid}, callerPackage: {caller_package}"
                        ),
                    ));
                }
                st.receivers.push(Receiver {
                    receiver: host.process.strong(handle),
                    start_seq,
                    pid,
                    uid,
                    user,
                    filters: Vec::new(),
                });
            }
        }
        let receiver = st
            .receivers
            .iter_mut()
            .find(|r| r.receiver.handle == handle)
            .unwrap();
        receiver.filters.push(Filter {
            filter,
            package: caller_package,
            feature_id: args.feature_id,
            receiver_id: args.receiver_id,
            permission: args.permission,
            instant_app,
            visible_to_instant_apps: flags & RECEIVER_VISIBLE_TO_INSTANT_APPS != 0,
            exported,
        });
        // Each matching sticky goes to the new receiver; the first it may
        // have is returned.
        let mut first = None;
        let preflight = host.flag(AVOID_NOTE_OP_AT_ENQUEUE)?;
        for r in matched {
            let calling_uid = r.sender.uid;
            if first.is_none()
                && (exported || calling_uid == uid || can_access_unexported(calling_uid))
            {
                first = Some(r.intent.clone());
            }
            let sender = Sender {
                uid: INVALID_UID,
                package: None,
                feature_id: None,
                original_uid: calling_uid,
                instant: false,
                required_permissions: Vec::new(),
                required_null: true,
                excluded_permissions: Vec::new(),
                excluded_packages: Vec::new(),
                app_op: OP_NONE,
                require_compat_change: None,
            };
            let receiver = st
                .receivers
                .iter()
                .find(|r| r.receiver.handle == handle)
                .unwrap();
            let filter = receiver.filters.len() - 1;
            if !self.skip(
                host,
                &sender,
                receiver,
                &receiver.filters[filter],
                r.intent.intent.flags,
                preflight,
            )? {
                let replay = Record {
                    intent: r.intent.clone(),
                    binders: r.binders.clone(),
                    sender,
                    ordered: false,
                    initial_sticky: true,
                    user: USER_ALL,
                    share_identity: false,
                    result_code: 0,
                    result_data: None,
                    result_extras: None,
                    id_for_response_event: 0,
                    temp_allowlist: None,
                    urgent: urgent(r.intent.intent.flags, 0),
                };
                st.queues
                    .entry(start_seq)
                    .or_default()
                    .push_back(Delivery::Once(
                        Arc::new(replay),
                        Registered {
                            handle,
                            filter,
                            start_seq,
                        },
                    ));
            }
        }
        Self::make_runnable(&mut st, QueueId::Warm(start_seq));
        self.update_running(host, &mut st);
        Ok(first)
    }

    /// `unregisterReceiver`: a delivery it is in the middle of finishes.
    pub fn unregister(&self, host: &AppHost, receiver: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = receiver else {
            return Err(Exception::new(EX_NULL_POINTER, "receiver"));
        };
        let mut st = self.state.lock().unwrap();
        let Some(at) = st
            .receivers
            .iter()
            .position(|r| r.receiver.handle == handle)
        else {
            return Ok(());
        };
        let removed = st.receivers.remove(at);
        let seq = removed.start_seq;
        let waiting = st
            .queues
            .get(&seq)
            .and_then(|q| q.front())
            .and_then(|d| match d {
                Delivery::Chained(id, r) if r.handle == handle => Some(*id),
                _ => None,
            });
        if let Some(id) = waiting.filter(|_| st.busy.remove(&seq)) {
            st.queues.get_mut(&seq).unwrap().pop_front();
            self.advance(host, &mut st, id);
            self.send_next(host, &mut st, seq);
            self.update_running(host, &mut st);
        }
        Ok(())
    }

    /// `broadcastIntentWithFeature` (`broadcastIntentLocked`).
    pub fn broadcast(
        &self,
        host: &AppHost,
        call: &Call<'_>,
        args: am::BroadcastIntentWithFeature<RawIntent, RawBundle>,
    ) -> Result<i32, Exception> {
        let caller = host.app_of_thread(args.caller);
        let (app, package) = (caller.as_ref().map(|c| c.0), caller.map(|c| c.1));
        self.broadcast_as(
            host,
            call.sender_euid as i32,
            call.sender_pid,
            app,
            package,
            args,
        )
    }

    /// `broadcastIntentLocked` of `calling_uid` (of `calling_pid`) from its
    /// process `caller_app` and package, if known:
    /// `broadcastIntentInPackage` for a PendingIntent's creator.
    pub fn broadcast_as(
        &self,
        host: &AppHost,
        calling_uid: i32,
        calling_pid: i32,
        caller_app: Option<i64>,
        caller_package: Option<String>,
        args: am::BroadcastIntentWithFeature<RawIntent, RawBundle>,
    ) -> Result<i32, Exception> {
        if is_isolated(calling_uid) {
            return Err(Exception::security(
                "Isolated process not allowed to call broadcastIntent",
            ));
        }
        let mut intent = args
            .intent
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "intent"))?;
        // `verifyBroadcastLocked`.
        if intent.has_fds() {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Intent",
            ));
        }
        let mut flags = intent.intent.flags;
        if flags & FLAG_RECEIVER_BOOT_UPGRADE != 0 {
            return Err(Exception::illegal_argument(
                "Can't use FLAG_RECEIVER_BOOT_UPGRADE here",
            ));
        }
        if flags & FLAG_RECEIVER_FROM_SHELL != 0 && !matches!(calling_uid, ROOT_UID | SHELL_UID) {
            flags &= !FLAG_RECEIVER_FROM_SHELL;
        }
        let extended_flags = intent.intent.extended_flags & !EXTENDED_FLAG_FILTER_MISMATCH;
        let options = Options::read(args.options.as_ref())?;
        // `enforceBroadcastOptionPermissionsInternal`.
        if calling_uid != SYSTEM_UID {
            if options.flags & OPTION_IS_ALARM_BROADCAST != 0 {
                return Err(Exception::security(
                    "Non-system callers may not flag broadcasts as alarm",
                ));
            }
            if options.flags & OPTION_INTERACTIVE != 0
                && !host.holds(
                    "android.permission.BROADCAST_OPTION_INTERACTIVE",
                    calling_uid,
                )?
            {
                return Err(Exception::security(format!(
                    "Permission Denial: setInteractive() from pid={}, uid={calling_uid} requires android.permission.BROADCAST_OPTION_INTERACTIVE",
                    calling_pid
                )));
            }
        }
        let result_to = match args.result_to {
            Some(Binder::Handle(h)) => Some(h),
            _ => None,
        };
        let caller_instant = match &caller_package {
            Some(p) => host.instant_app(p, calling_uid / PER_USER_RANGE as i32)?,
            None => false,
        };
        if caller_instant {
            flags &= !FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
        }
        // By default broadcasts do not go to stopped apps.
        flags |= FLAG_EXCLUDE_STOPPED_PACKAGES;
        if result_to.is_some() && !args.serialized && !is_core(calling_uid) {
            return Err(Exception::security(format!(
                "Unauthorized unordered resultTo broadcast {} sent from uid {calling_uid}",
                intent.intent.action.as_deref().unwrap_or("null")
            )));
        }
        let user = host.incoming_user(calling_uid, args.user_id, true)?;
        let denied = |permission: &str| {
            Exception::security(format!(
                "Permission Denial: {} broadcast from {} (pid={}, uid={calling_uid}) requires {permission}",
                intent.intent.action.as_deref().unwrap_or("null"),
                caller_package.as_deref().unwrap_or("null"),
                calling_pid
            ))
        };
        if options.temporary_allowlist_duration > 0
            && ![
                "android.permission.CHANGE_DEVICE_IDLE_TEMP_WHITELIST",
                "android.permission.START_ACTIVITIES_FROM_BACKGROUND",
                "android.permission.START_FOREGROUND_SERVICES_FROM_BACKGROUND",
            ]
            .iter()
            .try_fold(false, |held, p| {
                Ok::<_, Exception>(held || host.holds(p, calling_uid)?)
            })?
        {
            return Err(denied(
                "android.permission.CHANGE_DEVICE_IDLE_TEMP_WHITELIST or android.permission.START_ACTIVITIES_FROM_BACKGROUND or android.permission.START_FOREGROUND_SERVICES_FROM_BACKGROUND",
            ));
        }
        if options.flags & OPTION_ALLOW_BACKGROUND_ACTIVITY_STARTS != 0
            && !host.holds(
                "android.permission.START_ACTIVITIES_FROM_BACKGROUND",
                calling_uid,
            )?
        {
            return Err(denied(
                "android.permission.START_ACTIVITIES_FROM_BACKGROUND",
            ));
        }
        if options.id_for_response_event > 0
            && !host.holds(
                "android.permission.ACCESS_BROADCAST_RESPONSE_STATS",
                calling_uid,
            )?
        {
            return Err(Exception::security(format!(
                "recordResponseEventWhileInBackground: Neither user {calling_uid} nor current process has android.permission.ACCESS_BROADCAST_RESPONSE_STATS."
            )));
        }
        let result_to_app = caller_app;
        let mut st = self.state.lock().unwrap();
        // User 0 is the one user, and runs.
        if user != USER_ALL && user != 0 {
            self.cancel_result_to(host, &intent, result_to, result_to_app, user);
            return Ok(BROADCAST_FAILED_USER_STOPPED);
        }
        let (model, _) = host.live_packages()?;
        let action = intent.intent.action.clone();
        let protected = match action.as_deref() {
            Some(a) => crate::package::query::is_protected_broadcast(&model, a)
                .map_err(|e| host.unimplemented_named("package", e.0))?,
            None => false,
        };
        // `isCallerSystem`: the core uids that send system broadcasts; the
        // app host runs no persistent processes.
        let caller_system = matches!(
            calling_uid.rem_euclid(PER_USER_RANGE as i32),
            0 | 1000 | 1001 | 1002 | 1027 | 1068 | 1073
        );
        if !caller_system {
            if protected {
                return Err(Exception::security(format!(
                    "Permission Denial: not allowed to send broadcast {} from pid={}, uid={calling_uid}",
                    action.as_deref().unwrap_or("null"),
                    calling_pid
                )));
            }
            if matches!(
                action.as_deref(),
                Some(
                    "android.appwidget.action.APPWIDGET_CONFIGURE"
                        | "android.appwidget.action.APPWIDGET_UPDATE"
                )
            ) {
                let a = action.as_deref().unwrap();
                let Some(package) = &caller_package else {
                    return Err(Exception::security(format!(
                        "Permission Denial: not allowed to send broadcast {a} from unknown caller."
                    )));
                };
                match &intent.intent.component {
                    Some(c) if c.package != *package => {
                        return Err(Exception::security(format!(
                            "Permission Denial: not allowed to send broadcast {a} to {} from {package}",
                            c.package
                        )));
                    }
                    Some(_) => {}
                    // Confined to the sender's own package.
                    None => {
                        intent = intent
                            .with_package(package)
                            .map_err(|_| Exception::illegal_argument("intent"))?
                    }
                }
            }
        }
        if let Some(a) = &action {
            if model.system.allow_implicit_broadcasts.contains(a) {
                flags |= FLAG_RECEIVER_INCLUDE_BACKGROUND;
            }
            match a.as_str() {
                "com.android.launcher.action.INSTALL_SHORTCUT" => {
                    eprintln!(
                        "app host: broadcast {a} no longer supported. It will not be delivered."
                    );
                    self.cancel_result_to(host, &intent, result_to, result_to_app, user);
                    return Ok(BROADCAST_SUCCESS);
                }
                // `checkCanCloseSystemDialogs`: the permission, or an app
                // before S with a window over the shade (the app host's
                // apps are in front).
                "android.intent.action.CLOSE_SYSTEM_DIALOGS" => {
                    if !host.holds(
                        "android.permission.BROADCAST_CLOSE_SYSTEM_DIALOGS",
                        calling_uid,
                    )? && host.change_enabled(LOCK_DOWN_CLOSE_SYSTEM_DIALOGS, calling_uid)?
                    {
                        return Err(Exception::security(format!(
                            "Permission Denial: android.intent.action.CLOSE_SYSTEM_DIALOGS broadcast from {} (pid={}, uid={calling_uid}) requires android.permission.BROADCAST_CLOSE_SYSTEM_DIALOGS.",
                            caller_package.as_deref().unwrap_or("null"),
                            calling_pid
                        )));
                    }
                }
                _ => {}
            }
        }
        intent.set_flags(flags, extended_flags);
        let required_null = args.required_permissions.is_none();
        let resolved_type = args.resolved_type;
        let binders = intent
            .handles()
            .into_iter()
            .map(|h| host.process.strong(h))
            .collect();
        let result_extras = args
            .map
            .map(|m| KeptBundle::new(m, &host.process))
            .transpose()
            .map_err(|_| Exception::illegal_argument("result extras"))?;
        let sender = Sender {
            uid: calling_uid,
            package: caller_package.clone(),
            feature_id: args.calling_feature_id,
            original_uid: calling_uid,
            instant: caller_instant,
            required_permissions: strings(args.required_permissions),
            required_null,
            excluded_permissions: strings(args.exclude_permissions),
            excluded_packages: strings(args.exclude_packages),
            app_op: args.app_op,
            require_compat_change: options.require_compat_change.map(|id| {
                (
                    id,
                    options.flags & OPTION_REQUIRE_COMPAT_CHANGE_ENABLED != 0,
                )
            }),
        };
        let record = Arc::new(Record {
            urgent: urgent(intent.intent.flags, options.flags),
            intent,
            binders: Arc::new(binders),
            sender,
            ordered: args.serialized,
            initial_sticky: false,
            user,
            share_identity: options.flags & OPTION_SHARE_IDENTITY != 0,
            result_code: args.result_code,
            result_data: args.result_data,
            result_extras,
            id_for_response_event: options.id_for_response_event,
            temp_allowlist: (options.temporary_allowlist_duration > 0).then_some(TempAllowlist {
                duration_ms: options.temporary_allowlist_duration,
                kind: options.temporary_allowlist_type,
                reason: options.temporary_allowlist_reason,
            }),
        });
        if args.sticky {
            if !host.check_component_permission(Some(BROADCAST_STICKY), calling_uid, -1, true)? {
                return Err(Exception::security(format!(
                    "Permission Denial: broadcastIntent() requesting a sticky broadcast from pid={}, uid={calling_uid} requires {BROADCAST_STICKY}",
                    calling_pid
                )));
            }
            if !record.sender.required_permissions.is_empty() {
                self.cancel_result_to(host, &record.intent, result_to, result_to_app, user);
                return Ok(BROADCAST_STICKY_CANT_HAVE_PERMISSION);
            }
            if record.intent.intent.component.is_some() {
                return Err(Exception::security(
                    "Sticky broadcasts can't target a specific component",
                ));
            }
            let i = &record.intent.intent;
            if user != USER_ALL
                && st
                    .stickies
                    .iter()
                    .any(|s| s.record.user == USER_ALL && filter_equals(&s.record.intent.intent, i))
            {
                return Err(Exception::illegal_argument(format!(
                    "Sticky broadcast {} for user {user} conflicts with existing global broadcast",
                    i.action.as_deref().unwrap_or("null")
                )));
            }
            let sticky = Sticky {
                record: record.clone(),
                resolved_type: resolved_type.clone(),
            };
            match st
                .stickies
                .iter()
                .position(|s| s.record.user == user && filter_equals(&s.record.intent.intent, i))
            {
                Some(at) => st.stickies[at] = sticky,
                None => st.stickies.push(sticky),
            }
        }

        let preflight = host.flag(AVOID_NOTE_OP_AT_ENQUEUE)?;
        // The registered receivers, by priority, one filter per receiver.
        let mut registered: Vec<(i32, Registered)> = Vec::new();
        let block_null_action = action.is_none()
            && !can_access_unexported(calling_uid)
            && model
                .system
                .flags
                .iter()
                .any(|(name, on)| name == "android.security.block_null_action_intents" && *on)
            && host.change_enabled(BLOCK_NULL_ACTION_INTENTS, calling_uid)?;
        if record.intent.intent.component.is_none() && !block_null_action {
            registered = self.registered_targets(
                host,
                &st,
                &record.intent.intent,
                resolved_type.as_deref(),
                user,
                &record.sender,
                preflight,
            )?;
        }
        let manifest = if flags & FLAG_RECEIVER_REGISTERED_ONLY == 0 {
            self.manifest_targets(
                host,
                &record.intent.intent,
                resolved_type.as_deref(),
                user,
                &record.sender,
                preflight,
            )?
        } else {
            Vec::new()
        };
        let targets = merge(manifest, registered);
        if targets.is_empty() && result_to.is_none() {
            return Ok(BROADCAST_SUCCESS);
        }
        match result_to {
            None if !record.ordered => self.enqueue(host, &mut st, &record, targets),
            _ => {
                let id = st.next_chain;
                st.next_chain += 1;
                st.chains.insert(
                    id,
                    Chain {
                        result_code: record.result_code,
                        result_data: record.result_data.clone(),
                        result_extras: record.result_extras.clone(),
                        record,
                        targets: targets.into(),
                        aborted: false,
                        result_to: result_to.map(|h| (host.process.strong(h), result_to_app)),
                        done: None,
                    },
                );
                self.advance(host, &mut st, id);
                self.update_running(host, &mut st);
            }
        }
        Ok(BROADCAST_SUCCESS)
    }

    /// The registered receivers `intent` goes to, by priority, one filter
    /// per receiver (`mReceiverResolver.queryIntent`, then
    /// BroadcastSkipPolicy): priority, receiver, its process's start.
    #[allow(clippy::too_many_arguments)]
    fn registered_targets(
        &self,
        host: &AppHost,
        st: &State,
        i: &Intent,
        resolved_type: Option<&str>,
        user: i32,
        sender: &Sender,
        preflight: bool,
    ) -> Result<Vec<(i32, Registered)>, Exception> {
        let mut targets = Vec::new();
        for r in &st.receivers {
            if user != USER_ALL && r.user != USER_ALL && r.user != user {
                continue;
            }
            let mut taken = None;
            for (at, f) in r.filters.iter().enumerate() {
                if i.package.as_ref().is_some_and(|p| *p != f.package)
                    || !takes(&f.filter, i, resolved_type)?
                {
                    continue;
                }
                taken = Some(at);
                break;
            }
            let Some(filter) = taken else { continue };
            let f = &r.filters[filter];
            if !self.skip(host, sender, r, f, i.flags, preflight)? {
                let target = Registered {
                    handle: r.receiver.handle,
                    filter,
                    start_seq: r.start_seq,
                };
                targets.push((f.filter.priority, target));
            }
        }
        targets.sort_by_key(|t| std::cmp::Reverse(t.0));
        Ok(targets)
    }

    /// `sendBroadcastAsUser(intent, UserHandle.ALL)` of a system service of
    /// the app host: to the registered and manifest receivers it matches.
    pub fn send_from_system(&self, host: &AppHost, intent: RawIntent) {
        self.send_as_system(host, intent, USER_ALL, None, &[]);
    }

    /// `Context.sendBroadcastAsUser(intent, user)` of a system service of
    /// the app host.
    pub fn send_from_system_as_user(&self, host: &AppHost, intent: RawIntent, user: i32) {
        self.send_as_system(host, intent, user, None, &[]);
    }

    /// `Context.sendBroadcastAsUser(intent, user, receiverPermission)` of a
    /// system service of the app host: to receivers holding `permission`.
    pub fn send_from_system_with_permission(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        permission: &str,
    ) {
        self.send_as_system(host, intent, user, None, &[permission.to_string()]);
    }

    /// `Context.sendBroadcastAsUser(intent, user, receiverPermission,
    /// options)` of a system service of the app host whose options
    /// require `change` enabled, or disabled, for each receiver's app
    /// (`BroadcastOptions.setRequireCompatChange`).
    pub fn send_from_system_requiring_change(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        permission: &str,
        change: (i64, bool),
    ) {
        let mut sender = Sender::system(&[permission.to_string()]);
        sender.require_compat_change = Some(change);
        self.send_as(host, intent, user, None, sender);
    }

    /// `ActivityManagerInternal.broadcastIntentWithCallback` of
    /// PackageManager: to `user`'s registered receivers holding
    /// `required_permissions`, an application's only if its app id is in
    /// `allow_list` when there is one.
    pub fn send_from_package_manager(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
        required_permissions: &[String],
    ) {
        self.send_as_system(host, intent, user, allow_list, required_permissions);
    }

    fn send_as_system(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
        required_permissions: &[String],
    ) {
        self.send_as(
            host,
            intent,
            user,
            allow_list,
            Sender::system(required_permissions),
        );
    }

    /// `broadcastIntentLocked(..., resultTo, ..., requiredPermissions,
    /// appOp NONE, options, serialized, ...)` of a system service of the
    /// app host (`MY_PID`, `SYSTEM_UID`): to `user`'s receivers holding
    /// `required_permissions`, each allowed for `temp_allowlist` as it is
    /// sent it; serialized, receiver after receiver, when it has a `done`,
    /// which runs once it has gone through them, as the service's
    /// `IIntentReceiver.Stub` would.
    pub fn send_from_system_with(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        required_permissions: &[String],
        temp_allowlist: Option<TempAllowlist>,
        done: Option<Done>,
    ) {
        self.send_with(
            host,
            intent,
            user,
            None,
            Sender::system(required_permissions),
            temp_allowlist,
            done,
        );
    }

    fn send_as(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
        sender: Sender,
    ) {
        self.send_with(host, intent, user, allow_list, sender, None, None);
    }

    /// A system service's broadcast: ordered when it has a `done`.
    #[allow(clippy::too_many_arguments)]
    fn send_with(
        &self,
        host: &AppHost,
        mut intent: RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
        sender: Sender,
        temp_allowlist: Option<TempAllowlist>,
        done: Option<Done>,
    ) {
        let mut flags = intent.intent.flags | FLAG_EXCLUDE_STOPPED_PACKAGES;
        let allowed_implicit = host.live_packages().map(|(model, _)| {
            intent
                .intent
                .action
                .as_ref()
                .is_some_and(|a| model.system.allow_implicit_broadcasts.contains(a))
        });
        if allowed_implicit.unwrap_or(false) {
            flags |= FLAG_RECEIVER_INCLUDE_BACKGROUND;
        }
        let extended = intent.intent.extended_flags;
        intent.set_flags(flags, extended);
        let binders = intent
            .handles()
            .into_iter()
            .map(|h| host.process.strong(h))
            .collect();
        let record = Arc::new(Record {
            urgent: urgent(intent.intent.flags, 0),
            intent,
            binders: Arc::new(binders),
            sender,
            ordered: done.is_some(),
            initial_sticky: false,
            user,
            share_identity: false,
            result_code: 0,
            result_data: None,
            result_extras: None,
            id_for_response_event: 0,
            temp_allowlist,
        });
        let preflight = match host.flag(AVOID_NOTE_OP_AT_ENQUEUE) {
            Ok(preflight) => preflight,
            Err(e) => {
                if let Some(done) = done {
                    std::thread::spawn(done);
                }
                return eprintln!("app host: a system broadcast: {e:?}");
            }
        };
        let mut st = self.state.lock().unwrap();
        let i = &record.intent.intent;
        let allowed = |uid: i32| {
            let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
            allow_list.is_none_or(|allowed| {
                app_id < FIRST_APPLICATION_UID || allowed.binary_search(&app_id).is_ok()
            })
        };
        let targets = self
            .registered_targets(host, &st, i, None, user, &record.sender, preflight)
            .and_then(|registered| {
                let owner = |handle: u32| {
                    st.receivers
                        .iter()
                        .find(|r| r.receiver.handle == handle)
                        .map(|r| r.uid)
                };
                let registered = registered
                    .into_iter()
                    .filter(|(_, r)| allowed(owner(r.handle).unwrap_or_default()))
                    .collect();
                let manifest = if flags & FLAG_RECEIVER_REGISTERED_ONLY == 0 {
                    self.manifest_targets(host, i, None, user, &record.sender, preflight)?
                        .into_iter()
                        .filter(|m| allowed(m.uid as i32))
                        .collect()
                } else {
                    Vec::new()
                };
                Ok(merge(manifest, registered))
            });
        match (targets, done) {
            (Ok(targets), None) => self.enqueue(host, &mut st, &record, targets),
            (Ok(targets), Some(done)) => {
                let id = st.next_chain;
                st.next_chain += 1;
                st.chains.insert(
                    id,
                    Chain {
                        result_code: 0,
                        result_data: None,
                        result_extras: None,
                        record: record.clone(),
                        targets: targets.into(),
                        aborted: false,
                        result_to: None,
                        done: Some(done),
                    },
                );
                self.advance(host, &mut st, id);
                self.update_running(host, &mut st);
            }
            (Err(e), done) => {
                eprintln!("app host: a system broadcast: {e:?}");
                if let Some(done) = done {
                    std::thread::spawn(done);
                }
            }
        }
        drop(st);
        // `broadcastIntentLocked`'s own handling of the zone's change.
        if record.intent.intent.action.as_deref() == Some("android.intent.action.TIMEZONE_CHANGED")
        {
            host.update_time_zone();
        }
    }

    /// `collectReceiverComponents` for `intent`, then BroadcastSkipPolicy's
    /// checks: the manifest receivers it goes to, by priority.
    fn manifest_targets(
        &self,
        host: &AppHost,
        intent: &Intent,
        resolved_type: Option<&str>,
        user: i32,
        s: &Sender,
        preflight: bool,
    ) -> Result<Vec<Arc<Manifest>>, Exception> {
        let (_, resolution) = host.live_packages()?;
        let found = resolution
            .query_intent_receivers(
                intent,
                resolved_type,
                super::STOCK_PM_FLAGS,
                user.max(0),
                SYSTEM_UID,
            )
            .map_err(|e| {
                Exception::new(EX_ILLEGAL_STATE, format!("queryIntentReceivers: {e:?}"))
            })?;
        let mut targets = Vec::new();
        for ri in found {
            let Info::Activity(info) = ri.info else {
                continue;
            };
            if let Some(why) = self.skip_manifest(host, s, &info, intent, preflight)? {
                eprintln!("app host: {why}");
                continue;
            }
            let process = super::providers::process_name(&info.info);
            let uid = info.info.application_info.uid as u32;
            targets.push(Arc::new(Manifest {
                info,
                process,
                uid,
                priority: ri.priority,
            }));
        }
        Ok(targets)
    }

    /// BroadcastSkipPolicy's checks of a manifest receiver: why the
    /// broadcast skips it, if it does.
    fn skip_manifest(
        &self,
        host: &AppHost,
        s: &Sender,
        info: &ActivityInfo,
        intent: &Intent,
        preflight: bool,
    ) -> Result<Option<String>, Exception> {
        let app = &info.info.application_info;
        let (uid, package) = (app.uid, app.item.package_name.clone().unwrap_or_default());
        let name = format!(
            "{package}/{}",
            info.info.item.name.as_deref().unwrap_or_default()
        );
        let action = intent.action.as_deref().unwrap_or("null");
        if let Some((change, enabled)) = s.require_compat_change
            && host.change_enabled(change, uid)? != enabled
        {
            return Ok(Some(format!(
                "Compat change filtered: broadcasting {action} to uid {uid} due to compat change {change}"
            )));
        }
        if !host.check_component_permission(
            info.permission.as_deref(),
            s.uid,
            uid,
            info.info.exported,
        )? {
            return Ok(Some(if info.info.exported {
                format!(
                    "Permission Denial: broadcasting {action} to {name} requires {}",
                    info.permission.as_deref().unwrap_or("null")
                )
            } else {
                format!(
                    "Permission Denial: broadcasting {action} to {name} is not exported from uid {uid}"
                )
            }));
        }
        if let Some(permission) = &info.permission
            && let Some(op) = host.permission_op(permission)?
            && host.broadcast_op(
                preflight,
                op,
                s.uid,
                s.package.as_deref(),
                s.feature_id.as_deref(),
            )? != MODE_ALLOWED
        {
            return Ok(Some(format!(
                "Appop Denial: broadcasting {action} to {name} requires appop {}",
                op_names(op).0
            )));
        }
        if info.flags & FLAG_SINGLE_USER != 0
            && host.check_permission(Some("android.permission.INTERACT_ACROSS_USERS"), uid)? != 0
        {
            return Ok(Some(format!(
                "Permission Denial: Receiver {name} requests FLAG_SINGLE_USER, but app does not hold android.permission.INTERACT_ACROSS_USERS"
            )));
        }
        let user = uid / PER_USER_RANGE as i32;
        let instant = host.instant_app(&package, user)?;
        if instant && s.uid != uid {
            return Ok(Some(format!(
                "Instant App Denial: receiving {action} to {name}: Instant Apps do not support manifest receivers"
            )));
        }
        if s.instant && info.flags & FLAG_VISIBLE_TO_INSTANT_APP == 0 && s.uid != uid {
            return Ok(Some(format!(
                "Instant App Denial: receiving {action} to {name} requires receiver have visibleToInstantApps set"
            )));
        }
        match self.app_start_mode(host, uid, &package, app.target_sdk_version, instant)? {
            StartMode::Normal => {}
            StartMode::Disabled => {
                return Ok(Some(format!(
                    "Background execution disabled: receiving {action} to {name}"
                )));
            }
            _ if self.disallow_background_start(host, intent, s)? => {
                return Ok(Some(format!(
                    "Background execution not allowed: receiving {action} to {name}"
                )));
            }
            _ => {}
        }
        for permission in &s.excluded_permissions {
            let granted = host.check_permission(Some(permission), uid)? == 0;
            if excludes(granted, host.permission_op(permission)?, |op| {
                appops_service::mode(host, op, uid, Some(&package))
            })? {
                return Ok(Some(format!(
                    "Skipping delivery to {package} due to excluded permission {permission}"
                )));
            }
        }
        if s.excluded_packages.contains(&package) {
            return Ok(Some(format!(
                "Skipping delivery of excluded package {action} to {name}"
            )));
        }
        let tags = receiver_tags(info.info.attribution_tags.as_deref());
        if uid != SYSTEM_UID && !s.required_permissions.is_empty() {
            let sources: Vec<_> = tags
                .iter()
                .map(|&tag| attribution(uid, -1, &package, tag))
                .collect();
            let message = format!(
                "Broadcast delivered to {}",
                info.info.item.name.as_deref().unwrap_or_default()
            );
            for permission in &s.required_permissions {
                if !host.has_permission(permission, &message, preflight, &sources)? {
                    return Ok(Some(format!(
                        "Permission Denial: receiving {action} to {name} requires {permission}"
                    )));
                }
            }
        }
        // `noteOpForManifestReceiver`, `checkOpForManifestReceiver`: the
        // op of each attribution tag.
        if s.app_op != OP_NONE && valid_op(s.app_op) {
            for tag in tags {
                if host.broadcast_op(preflight, s.app_op, uid, Some(&package), tag)? != MODE_ALLOWED
                {
                    eprintln!(
                        "app host: Appop Denial: receiving {action} to {name} requires appop {}",
                        op_names(s.app_op).1
                    );
                    return Ok(Some(format!(
                        "Skipping delivery to {package} due to required appop {}",
                        op_names(s.app_op).1
                    )));
                }
            }
        }
        Ok(None)
    }

    /// `getAppStartModeLOSP` with `alwaysRestrict`
    /// (`appRestrictedInBackgroundLOSP`): an instant app's starts are
    /// disabled, an app targeting O or later is restricted, an older one
    /// as its `RUN_IN_BACKGROUND` op says.
    fn app_start_mode(
        &self,
        host: &AppHost,
        uid: i32,
        package: &str,
        target_sdk: i32,
        instant: bool,
    ) -> Result<StartMode, Exception> {
        const O: i32 = 26;
        if instant {
            return Ok(StartMode::Disabled);
        }
        if target_sdk >= O {
            return Ok(StartMode::DelayedRigid);
        }
        Ok(
            match appops_service::note(host, OP_RUN_IN_BACKGROUND, uid, Some(package))? {
                0 => StartMode::Normal,
                MODE_IGNORED => StartMode::Delayed,
                _ => StartMode::DelayedRigid,
            },
        )
    }

    /// `disallowBackgroundStart`: a broadcast excluding the background, or
    /// an implicit one not including it whose required permissions are not
    /// all signature ones (`isSignaturePerm`).
    fn disallow_background_start(
        &self,
        host: &AppHost,
        intent: &Intent,
        s: &Sender,
    ) -> Result<bool, Exception> {
        let mut signature = !s.required_null;
        if signature && !s.required_permissions.is_empty() {
            let permissions = host.permission_manager.get().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "permission owner is unavailable")
            })?;
            for name in &s.required_permissions {
                if !permissions.protection_level(name)?.is_some_and(|level| {
                    level & (PROTECTION_MASK_BASE | PROTECTION_FLAG_PRIVILEGED)
                        == PROTECTION_SIGNATURE
                }) {
                    signature = false;
                    break;
                }
            }
        }
        Ok(intent.flags & FLAG_RECEIVER_EXCLUDE_BACKGROUND != 0
            || intent.component.is_none()
                && intent.package.is_none()
                && intent.flags & FLAG_RECEIVER_INCLUDE_BACKGROUND == 0
                && !signature)
    }

    /// The queue of the process a manifest receiver runs in: a running
    /// process's, else one waiting for its process to start; none for
    /// system_server's own receivers.
    fn manifest_queue(&self, host: &AppHost, m: &Manifest) -> Option<QueueId> {
        // The system process runs the system apps' components declared in
        // it, not system_server's own receivers.
        if super::is_system_server_code(&manifest_package(m), &m.process) {
            let e = host
                .unimplemented_named("activity", "a receiver of system_server's own code (#1518)");
            eprintln!(
                "app host: unable to deliver a broadcast to {}: {}",
                m.process, e.message
            );
            return None;
        }
        Some(match host.running_process(&m.process, m.uid) {
            Some((seq, _)) => QueueId::Warm(seq),
            None => QueueId::Cold(m.process.clone(), m.uid),
        })
    }

    /// Puts `delivery` on the queue it goes to, which becomes runnable.
    fn queue_delivery(
        &self,
        st: &mut State,
        to: QueueId,
        m: Option<&Manifest>,
        delivery: Delivery,
    ) {
        match &to {
            QueueId::Warm(seq) => st.queues.entry(*seq).or_default().push_back(delivery),
            QueueId::Cold(process, uid) => match st
                .cold
                .iter_mut()
                .find(|c| &c.process == process && c.uid == *uid)
            {
                Some(c) => c.deliveries.push_back(delivery),
                None => st.cold.push(Cold {
                    process: process.clone(),
                    uid: *uid,
                    package: m.map(manifest_package).unwrap_or_default(),
                    deliveries: VecDeque::from([delivery]),
                }),
            },
        }
        Self::make_runnable(st, to);
    }

    /// `updateRunnableList`: the queue waits its turn, unless it runs.
    fn make_runnable(st: &mut State, queue: QueueId) {
        if let QueueId::Warm(seq) = queue
            && (st.busy.contains(&seq) || st.cold_start == Some(seq))
        {
            return;
        }
        if !st.runnable.contains(&queue) {
            st.runnable.push_back(queue);
        }
    }

    /// The queue limits, from the native boot namespace's properties
    /// (`BroadcastConstants.updateDeviceConfigConstants`: its `persist.sys.`
    /// override, else `persist.device_config.`): two running queues on a
    /// low-RAM device and four otherwise, and one extra for urgent ones.
    fn limits(&self, host: &AppHost) -> Limits {
        *self.limits.get_or_init(|| {
            let int = |key: &str, default: usize| {
                [
                    format!("persist.sys.activity_manager_native_boot.{key}"),
                    format!("persist.device_config.activity_manager_native_boot.{key}"),
                ]
                .iter()
                .find_map(|name| host.property(name).and_then(|v| v.trim().parse().ok()))
                .unwrap_or(default)
            };
            let low_ram = host.property("ro.config.low_ram").as_deref() == Some("true");
            Limits {
                max_running: int(
                    "bcast_max_running_process_queues",
                    if low_ram { 2 } else { 4 },
                ),
                extra_urgent: int("bcast_extra_running_urgent_process_queues", 1),
            }
        })
    }

    /// `isPendingUrgent`: whether the delivery at the front of `queue` is of
    /// an urgent broadcast.
    fn pending_urgent(st: &State, queue: &QueueId) -> bool {
        let front = match queue {
            QueueId::Warm(seq) => st.queues.get(seq).and_then(|q| q.front()),
            QueueId::Cold(process, uid) => st
                .cold
                .iter()
                .find(|c| &c.process == process && c.uid == *uid)
                .and_then(|c| c.deliveries.front()),
        };
        match front {
            Some(Delivery::Once(record, _) | Delivery::Manifest(record, ..)) => record.urgent,
            Some(Delivery::Chained(id, _)) => st.chains.get(id).is_some_and(|c| c.record.urgent),
            None => false,
        }
    }

    /// `updateRunningListLocked`: the runnable queues promoted to running
    /// in their order while the limits allow, one process started at a
    /// time; a warm queue sends what it has, a cold one has its process
    /// started for its first receiver not skipped.
    fn update_running(&self, host: &AppHost, st: &mut State) {
        let limits = self.limits(host);
        loop {
            let running: Vec<i64> = st.busy.iter().copied().chain(st.cold_start).collect();
            let urgent = running
                .iter()
                .filter(|seq| Self::pending_urgent(st, &QueueId::Warm(**seq)))
                .count();
            let candidates: Vec<Candidate> = st
                .runnable
                .iter()
                .map(|q| Candidate {
                    urgent: Self::pending_urgent(st, q),
                    cold: match q {
                        QueueId::Warm(seq) => Self::needs_start(host, st, *seq),
                        QueueId::Cold(..) => true,
                    },
                })
                .collect();
            let Some(at) = next_to_run(
                &candidates,
                running.len(),
                urgent,
                st.cold_start.is_some(),
                limits,
            ) else {
                return;
            };
            match st.runnable.remove(at).unwrap() {
                QueueId::Warm(seq) if candidates[at].cold => st.cold_start = Some(seq),
                QueueId::Warm(seq) => self.send_next(host, st, seq),
                QueueId::Cold(process, uid) => self.start_cold(host, st, &process, uid),
            }
        }
    }

    /// Whether the process of `seq` is still to attach for the delivery at
    /// the front of its queue (a manifest receiver needs its thread).
    fn needs_start(host: &AppHost, st: &State, seq: i64) -> bool {
        matches!(
            st.queues.get(&seq).and_then(|q| q.front()),
            Some(Delivery::Manifest(..))
        ) && !host
            .apps
            .lock()
            .unwrap()
            .get(&seq)
            .is_some_and(|a| a.thread.is_some())
    }

    /// `scheduleReceiverColdLocked`: the process of a cold queue started
    /// for its first receiver not skipped (`startProcessLocked` with hosting
    /// type `broadcast`), its deliveries waiting for it to attach; the
    /// receiver it could not be started for fails.
    fn start_cold(&self, host: &AppHost, st: &mut State, process: &str, uid: u32) {
        let Some(at) = st
            .cold
            .iter()
            .position(|c| c.process == process && c.uid == uid)
        else {
            return;
        };
        let mut cold = st.cold.remove(at);
        // It may have started meanwhile.
        if let Some((seq, _)) = host.running_process(process, uid) {
            st.queues.entry(seq).or_default().extend(cold.deliveries);
            Self::make_runnable(st, QueueId::Warm(seq));
            return;
        }
        while let Some(Delivery::Manifest(record, m, chain)) = cold.deliveries.front() {
            if !self.skipped_manifest(host, record, m) {
                break;
            }
            let chain = *chain;
            cold.deliveries.pop_front();
            if let Some(id) = chain {
                self.advance(host, st, id);
            }
        }
        if cold.deliveries.is_empty() {
            return;
        }
        let started = host
            .start_process_for(&cold.package, process, None, true)
            .map_err(|e| e.message)
            .and_then(|_| {
                host.running_process(process, uid)
                    .map(|(seq, _)| seq)
                    .ok_or_else(|| "the process is gone".to_string())
            });
        match started {
            Ok(seq) => {
                st.queues.entry(seq).or_default().extend(cold.deliveries);
                st.cold_start = Some(seq);
                self.send_next(host, st, seq);
            }
            Err(e) => {
                eprintln!("app host: unable to launch {process} for broadcast: {e}");
                if let Some(Delivery::Manifest(_, _, Some(id))) = cold.deliveries.pop_front() {
                    self.advance(host, st, id);
                }
                if !cold.deliveries.is_empty() {
                    st.cold.push(cold);
                    Self::make_runnable(st, QueueId::Cold(process.to_string(), uid));
                }
            }
        }
    }

    /// `shouldSkipReceiver`'s `shouldSkipMessage` of a manifest receiver
    /// as `record` is delivered to it, its app ops noted: whether it is
    /// skipped, its delivery finished.
    fn skipped_manifest(&self, host: &AppHost, record: &Record, m: &Manifest) -> bool {
        match self.skip_manifest(host, &record.sender, &m.info, &record.intent.intent, false) {
            Ok(None) => false,
            Ok(Some(why)) => {
                eprintln!("app host: {why}");
                true
            }
            Err(e) => {
                eprintln!(
                    "app host: a broadcast to {} was not checked: {}",
                    m.process, e.message
                );
                true
            }
        }
    }

    /// `shouldSkipReceiver`'s `shouldSkipMessage` of a registered receiver
    /// as `record` is delivered to it, its app ops noted: whether it is
    /// skipped, as one no longer registered is.
    fn skipped_registered(
        &self,
        host: &AppHost,
        st: &State,
        record: &Record,
        to: Registered,
    ) -> bool {
        let Some(r) = st.receivers.iter().find(|r| r.receiver.handle == to.handle) else {
            return true;
        };
        let flags = record.intent.intent.flags;
        match self.skip(host, &record.sender, r, &r.filters[to.filter], flags, false) {
            Ok(skipped) => skipped,
            Err(e) => {
                eprintln!("app host: a broadcast was not checked: {}", e.message);
                true
            }
        }
    }

    /// Queues `record` to each of `targets`, assumed delivered to the
    /// registered ones, and sends each process what it has next.
    fn enqueue(&self, host: &AppHost, st: &mut State, record: &Arc<Record>, targets: Vec<Target>) {
        for target in targets {
            match target {
                Target::Registered(r) => self.queue_delivery(
                    st,
                    QueueId::Warm(r.start_seq),
                    None,
                    Delivery::Once(record.clone(), r),
                ),
                Target::Manifest(m) => {
                    if let Some(to) = self.manifest_queue(host, &m) {
                        let delivery = Delivery::Manifest(record.clone(), m.clone(), None);
                        self.queue_delivery(st, to, Some(&m), delivery);
                    }
                }
            }
        }
        self.update_running(host, st);
    }

    /// `ActivityManager.broadcastStickyIntent(intent, OP_NONE,
    /// USER_ALL)` of a system service of the app host: kept as the sticky
    /// of all users, in place of one filter-equal to it, then sent as
    /// [`Self::send_from_system`] sends.
    pub fn send_sticky_from_system(&self, host: &AppHost, mut intent: RawIntent) {
        let flags = intent.intent.flags | FLAG_EXCLUDE_STOPPED_PACKAGES;
        let extended = intent.intent.extended_flags;
        intent.set_flags(flags, extended);
        let record = Arc::new(Record {
            intent: intent.clone(),
            binders: Arc::new(
                intent
                    .handles()
                    .into_iter()
                    .map(|h| host.process.strong(h))
                    .collect(),
            ),
            sender: Sender::system(&[]),
            ordered: false,
            initial_sticky: false,
            user: USER_ALL,
            share_identity: false,
            result_code: 0,
            result_data: None,
            result_extras: None,
            id_for_response_event: 0,
            temp_allowlist: None,
            urgent: urgent(flags, 0),
        });
        {
            let mut st = self.state.lock().unwrap();
            let sticky = Sticky {
                record,
                resolved_type: None,
            };
            match st.stickies.iter().position(|s| {
                s.record.user == USER_ALL && filter_equals(&s.record.intent.intent, &intent.intent)
            }) {
                Some(at) => st.stickies[at] = sticky,
                None => st.stickies.push(sticky),
            }
        }
        self.send_from_system(host, intent);
    }

    /// The process of `start_seq` attached: the queue started for it runs
    /// (`onApplicationAttachedLocked`), and another process may be started.
    pub fn process_attached(&self, host: &AppHost, start_seq: i64) {
        let mut st = self.state.lock().unwrap();
        if st.cold_start == Some(start_seq) {
            st.cold_start = None;
            self.send_next(host, &mut st, start_seq);
        } else if st.queues.get(&start_seq).is_some_and(|q| !q.is_empty()) {
            Self::make_runnable(&mut st, QueueId::Warm(start_seq));
        }
        self.update_running(host, &mut st);
    }

    /// `unbroadcastIntent`: a sticky broadcast goes.
    pub fn unbroadcast(
        &self,
        host: &AppHost,
        call: &Call<'_>,
        intent: Option<RawIntent>,
        user: i32,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        if intent.as_ref().is_some_and(RawIntent::has_fds) {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Intent",
            ));
        }
        let user = host.incoming_user(uid, user, true)?;
        if !host.check_component_permission(Some(BROADCAST_STICKY), uid, -1, true)? {
            return Err(Exception::security(format!(
                "Permission Denial: unbroadcastIntent() from pid={}, uid={uid} requires {BROADCAST_STICKY}",
                call.sender_pid
            )));
        }
        let intent = intent.ok_or_else(|| Exception::new(EX_NULL_POINTER, "intent"))?;
        let mut st = self.state.lock().unwrap();
        if let Some(at) = st.stickies.iter().position(|s| {
            s.record.user == user && filter_equals(&s.record.intent.intent, &intent.intent)
        }) {
            st.stickies.remove(at);
        }
        Ok(())
    }

    /// `finishReceiver`: the caller's process finished the chained
    /// delivery it was sent; its result goes on to the next receiver.
    pub fn finish(
        &self,
        host: &AppHost,
        args: am::FinishReceiver<RawBundle>,
    ) -> Result<(), Exception> {
        if args.map.as_ref().is_some_and(RawBundle::has_fds) {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Bundle",
            ));
        }
        let Some((seq, ..)) = host.app_of_thread(args.who) else {
            eprintln!("app host: finishReceiver: no app");
            return Ok(());
        };
        let extras = args
            .map
            .map(|m| KeptBundle::new(m, &host.process))
            .transpose()
            .map_err(|_| Exception::illegal_argument("result extras"))?;
        let mut st = self.state.lock().unwrap();
        let id = match st
            .queues
            .get(&seq)
            .and_then(|q| q.front())
            .filter(|_| st.busy.contains(&seq))
        {
            Some(Delivery::Chained(id, _)) => Some(*id),
            Some(Delivery::Manifest(_, _, id)) => *id,
            _ => return Ok(()),
        };
        st.busy.remove(&seq);
        st.queues.get_mut(&seq).unwrap().pop_front();
        if let Some(id) = id {
            if let Some(chain) = st.chains.get_mut(&id)
                && chain.record.ordered
            {
                chain.result_code = args.result_code;
                chain.result_data = args.result_data;
                chain.result_extras = extras;
                if chain.record.intent.intent.flags & FLAG_RECEIVER_NO_ABORT == 0 {
                    chain.aborted = args.abort_broadcast;
                }
            }
            self.advance(host, &mut st, id);
        }
        self.send_next(host, &mut st, seq);
        self.update_running(host, &mut st);
        Ok(())
    }

    /// A process died: its receivers go, and what it was to finish is
    /// skipped.
    pub fn process_died(&self, host: &AppHost, start_seq: i64) {
        let mut st = self.state.lock().unwrap();
        st.receivers.retain(|r| r.start_seq != start_seq);
        st.busy.remove(&start_seq);
        if st.cold_start == Some(start_seq) {
            st.cold_start = None;
        }
        st.runnable.retain(|q| *q != QueueId::Warm(start_seq));
        for delivery in st.queues.remove(&start_seq).unwrap_or_default() {
            if let Delivery::Chained(id, _) | Delivery::Manifest(_, _, Some(id)) = delivery {
                self.advance(host, &mut st, id);
            }
        }
        self.update_running(host, &mut st);
    }

    /// BroadcastSkipPolicy's checks of a registered receiver: whether the
    /// broadcast skips it.
    fn skip(
        &self,
        host: &AppHost,
        s: &Sender,
        r: &Receiver,
        f: &Filter,
        intent_flags: i32,
        preflight: bool,
    ) -> Result<bool, Exception> {
        if let Some((change, enabled)) = s.require_compat_change
            && host.change_enabled(change, r.uid)? != enabled
        {
            return Ok(true);
        }
        if let Some(permission) = &f.permission {
            if !host.holds(permission, s.uid)? {
                return Ok(true);
            }
            if let Some(op) = host.permission_op(permission)?
                && host.broadcast_op(
                    preflight,
                    op,
                    s.uid,
                    s.package.as_deref(),
                    s.feature_id.as_deref(),
                )? != MODE_ALLOWED
            {
                return Ok(true);
            }
        }
        let visible = intent_flags & FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS != 0;
        if (!visible && f.instant_app || !f.visible_to_instant_apps && s.instant) && r.uid != s.uid
        {
            return Ok(true);
        }
        if !s.required_permissions.is_empty() {
            let source = attribution(r.uid, r.pid, &f.package, f.feature_id.as_deref());
            let message = format!(
                "Broadcast delivered to registered receiver {}",
                f.receiver_id.as_deref().unwrap_or("null")
            );
            for permission in &s.required_permissions {
                if !host.has_permission(
                    permission,
                    &message,
                    preflight,
                    slice::from_ref(&source),
                )? {
                    return Ok(true);
                }
            }
        }
        if s.required_permissions.is_empty()
            && !host.check_component_permission(None, r.uid, -1, true)?
        {
            return Ok(true);
        }
        for permission in &s.excluded_permissions {
            let granted = host.holds(permission, r.uid)?;
            if excludes(granted, host.permission_op(permission)?, |op| {
                appops_service::mode(host, op, r.uid, Some(&f.package))
            })? {
                return Ok(true);
            }
        }
        if s.excluded_packages.contains(&f.package) {
            return Ok(true);
        }
        if s.app_op != OP_NONE
            && valid_op(s.app_op)
            && host.broadcast_op(
                preflight,
                s.app_op,
                r.uid,
                Some(&f.package),
                f.feature_id.as_deref(),
            )? != MODE_ALLOWED
        {
            return Ok(true);
        }
        Ok(!f.exported && !host.check_component_permission(None, s.original_uid, r.uid, false)?)
    }

    /// Sends what `start_seq`'s process has next: its assumed deliveries,
    /// up to and with the next chained one, which waits for its finish.
    fn send_next(&self, host: &AppHost, st: &mut State, start_seq: i64) {
        loop {
            if st.busy.contains(&start_seq) {
                return;
            }
            let Some(front) = st.queues.get(&start_seq).and_then(|q| q.front()) else {
                return;
            };
            if let Delivery::Manifest(record, m, chain) = front {
                let (record, m, chain) = (record.clone(), m.clone(), *chain);
                let sent = {
                    let result = chain.and_then(|id| st.chains.get(&id));
                    let (code, data, extras) = match result {
                        Some(c) => (
                            c.result_code,
                            c.result_data.as_deref(),
                            c.result_extras.as_ref(),
                        ),
                        None => (
                            record.result_code,
                            record.result_data.as_deref(),
                            record.result_extras.as_ref(),
                        ),
                    };
                    self.deliver_manifest(host, start_seq, &record, &m, code, data, extras)
                };
                match sent {
                    // Its process has not attached yet.
                    None => return,
                    Some(true) => {
                        st.busy.insert(start_seq);
                        return;
                    }
                    Some(false) => {
                        st.queues.get_mut(&start_seq).unwrap().pop_front();
                        if let Some(id) = chain {
                            self.advance(host, st, id);
                        }
                        continue;
                    }
                }
            }
            let (record, to, chain) = match front {
                Delivery::Once(record, to) => (record.clone(), *to, None),
                Delivery::Chained(id, to) => match st.chains.get(id) {
                    Some(chain) => (chain.record.clone(), *to, Some(*id)),
                    None => {
                        st.queues.get_mut(&start_seq).unwrap().pop_front();
                        continue;
                    }
                },
                Delivery::Manifest(..) => unreachable!(),
            };
            let sent = !self.skipped_registered(host, st, &record, to)
                && match chain.and_then(|id| st.chains.get(&id)) {
                    Some(c) => self.deliver(
                        host,
                        start_seq,
                        to.handle,
                        &record,
                        c.result_code,
                        c.result_data.as_deref(),
                        c.result_extras.as_ref(),
                        false,
                    ),
                    None => self.deliver(
                        host,
                        start_seq,
                        to.handle,
                        &record,
                        record.result_code,
                        record.result_data.as_deref(),
                        record.result_extras.as_ref(),
                        true,
                    ),
                };
            if let (Some(_), true) = (chain, sent) {
                st.busy.insert(start_seq);
                return;
            }
            st.queues.get_mut(&start_seq).unwrap().pop_front();
            if let Some(id) = chain {
                self.advance(host, st, id);
            }
        }
    }

    /// Moves chain `id` on to its next receiver, or, past its last or
    /// aborted, to its result receiver.
    fn advance(&self, host: &AppHost, st: &mut State, id: u64) {
        loop {
            let Some(chain) = st.chains.get_mut(&id) else {
                return;
            };
            let next = if chain.aborted {
                None
            } else {
                chain.targets.pop_front()
            };
            let Some(target) = next else {
                let mut chain = st.chains.remove(&id).unwrap();
                // Off the queue's lock: it may broadcast again.
                if let Some(done) = chain.done.take() {
                    std::thread::spawn(done);
                }
                self.send_result(host, chain);
                return;
            };
            match target {
                Target::Registered(r) => {
                    if !st.receivers.iter().any(|x| x.receiver.handle == r.handle) {
                        continue;
                    }
                    let delivery = Delivery::Chained(id, r);
                    self.queue_delivery(st, QueueId::Warm(r.start_seq), None, delivery);
                }
                Target::Manifest(m) => {
                    let record = chain.record.clone();
                    let Some(to) = self.manifest_queue(host, &m) else {
                        continue;
                    };
                    let delivery = Delivery::Manifest(record, m.clone(), Some(id));
                    self.queue_delivery(st, to, Some(&m), delivery);
                }
            }
            return;
        }
    }

    /// The final result of a chain to its result receiver: through its
    /// process's thread, else to the receiver itself.
    fn send_result(&self, host: &AppHost, chain: Chain) {
        let Some((result_to, app)) = chain.result_to else {
            return;
        };
        let record = &chain.record;
        let extras = chain.result_extras.as_ref().map(|e| By(&e.bundle));
        let sent = match app {
            Some(seq) => {
                self.share_identity_access(host, seq, record);
                let mut p = Parcel::new();
                thread::ScheduleRegisteredReceiver {
                    receiver: Some(result_to.binder()),
                    intent: Some(By(&record.intent)),
                    result_code: chain.result_code,
                    data: chain.result_data.clone(),
                    extras,
                    ordered: false,
                    sticky: false,
                    assume_delivered: true,
                    sending_user: record.user,
                    process_state: PROCESS_STATE_TOP,
                    sent_from_uid: if record.share_identity {
                        record.sender.uid
                    } else {
                        INVALID_UID
                    },
                    sent_from_package: record
                        .share_identity
                        .then(|| record.sender.package.clone())
                        .flatten(),
                }
                .write(&mut p);
                host.call_thread(seq, thread::SCHEDULE_REGISTERED_RECEIVER, &p)
            }
            None => {
                let mut p = Parcel::new();
                intent_receiver::PerformReceive {
                    intent: Some(By(&record.intent)),
                    result_code: chain.result_code,
                    data: chain.result_data.clone(),
                    extras,
                    ordered: false,
                    sticky: false,
                    sending_user: record.user,
                }
                .write(&mut p);
                result_to
                    .transact(intent_receiver::PERFORM_RECEIVE, &p, true)
                    .map(|_| ())
                    .map_err(|s| format!("{s}"))
            }
        };
        if let Err(e) = sent {
            eprintln!("app host: a broadcast's result was not delivered: {e}");
        }
    }

    /// `scheduleCanceledResultTo`.
    fn cancel_result_to(
        &self,
        host: &AppHost,
        intent: &RawIntent,
        result_to: Option<u32>,
        app: Option<i64>,
        user: i32,
    ) {
        let (Some(handle), Some(seq)) = (result_to, app) else {
            return;
        };
        let mut p = Parcel::new();
        thread::ScheduleRegisteredReceiver::<By<RawIntent>, RawBundle> {
            receiver: Some(Binder::Handle(handle)),
            intent: Some(By(intent)),
            result_code: RESULT_CANCELED,
            data: None,
            extras: None,
            ordered: false,
            sticky: false,
            assume_delivered: true,
            sending_user: user,
            process_state: PROCESS_STATE_TOP,
            sent_from_uid: INVALID_UID,
            sent_from_package: None,
        }
        .write(&mut p);
        if let Err(e) = host.call_thread(seq, thread::SCHEDULE_REGISTERED_RECEIVER, &p) {
            eprintln!("app host: a canceled broadcast's result was not delivered: {e}");
        }
    }

    /// `reportUsageStatsBroadcastDispatched` of `record` scheduled to the
    /// process of `start_seq`: with an id for its response event and a
    /// target package (the intent's package or component's), at the uid
    /// state of the process, which is top while it runs.
    fn report_dispatched(&self, host: &AppHost, start_seq: i64, record: &Record) {
        if record.id_for_response_event <= 0 {
            return;
        }
        let intent = &record.intent.intent;
        let Some(package) = intent
            .package
            .as_deref()
            .or(intent.component.as_ref().map(|c| c.package.as_str()))
        else {
            return;
        };
        if !host.apps.lock().unwrap().contains_key(&start_seq) {
            return;
        }
        if let Err(e) = host.report_broadcast_dispatched(
            record.sender.uid,
            package,
            record.user,
            record.id_for_response_event,
            PROCESS_STATE_TOP,
        ) {
            eprintln!("app host: broadcast: response stats: {}", e.message);
        }
    }

    /// With a shared identity, the receiving app of `start_seq` may see the
    /// sender (`grantImplicitAccess` in `scheduleReceiverWarmLocked` and
    /// `scheduleResultTo`).
    fn share_identity_access(&self, host: &AppHost, start_seq: i64, record: &Record) {
        if !record.share_identity {
            return;
        }
        let Some(uid) = host
            .apps
            .lock()
            .unwrap()
            .get(&start_seq)
            .map(|a| a.uid as i32)
        else {
            return;
        };
        if uid == record.sender.uid {
            return;
        }
        if let Err(e) = host.grant_implicit_access(uid, record.sender.uid) {
            eprintln!("app host: broadcast: implicit access: {}", e.message);
        }
    }

    /// `scheduleRegisteredReceiver` of `record` to `receiver` in the
    /// process of `start_seq`: whether it was sent.
    #[allow(clippy::too_many_arguments)]
    fn deliver(
        &self,
        host: &AppHost,
        start_seq: i64,
        receiver: u32,
        record: &Record,
        result_code: i32,
        result_data: Option<&str>,
        result_extras: Option<&KeptBundle>,
        assume_delivered: bool,
    ) -> bool {
        self.share_identity_access(host, start_seq, record);
        self.report_dispatched(host, start_seq, record);
        let uid = host.apps.lock().unwrap().get(&start_seq).map(|a| a.uid);
        if let Some(uid) = uid {
            temp_allowlist(host, uid as i32, record);
        }
        let mut p = Parcel::new();
        thread::ScheduleRegisteredReceiver {
            receiver: Some(Binder::Handle(receiver)),
            intent: Some(By(&record.intent)),
            result_code,
            data: result_data.map(str::to_string),
            extras: result_extras.map(|e| By(&e.bundle)),
            ordered: record.ordered,
            sticky: record.initial_sticky,
            assume_delivered,
            sending_user: record.user,
            process_state: PROCESS_STATE_TOP,
            sent_from_uid: if record.share_identity {
                record.sender.uid
            } else {
                INVALID_UID
            },
            sent_from_package: record
                .share_identity
                .then(|| record.sender.package.clone())
                .flatten(),
        }
        .write(&mut p);
        match host.call_thread(start_seq, thread::SCHEDULE_REGISTERED_RECEIVER, &p) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("app host: a broadcast was not delivered: {e}");
                false
            }
        }
    }

    /// `scheduleReceiver` of `record` to the manifest receiver `m` in the
    /// process of `start_seq`, the intent naming it, unless it is skipped:
    /// whether it was sent, none while the process has not attached.
    #[allow(clippy::too_many_arguments)]
    fn deliver_manifest(
        &self,
        host: &AppHost,
        start_seq: i64,
        record: &Record,
        m: &Manifest,
        result_code: i32,
        result_data: Option<&str>,
        result_extras: Option<&KeptBundle>,
    ) -> Option<bool> {
        let config = {
            let apps = host.apps.lock().unwrap();
            let app = apps.get(&start_seq)?;
            app.thread.as_ref()?;
            app.config.clone()?
        };
        if self.skipped_manifest(host, record, m) {
            return Some(false);
        }
        temp_allowlist(host, m.uid as i32, record);
        self.share_identity_access(host, start_seq, record);
        let compat = super::config::CompatibilityInfo::of(
            &m.info.info.application_info,
            &config,
            host.inputs().ok()?.device_density,
        );
        let package = m
            .info
            .info
            .application_info
            .item
            .package_name
            .clone()
            .unwrap_or_default();
        let intent = record.intent.with_component(
            &package,
            m.info.info.item.name.as_deref().unwrap_or_default(),
        );
        let mut p = Parcel::new();
        thread::ScheduleReceiver {
            intent: Some(By(&intent)),
            info: Some(By(&m.info)),
            compat_info: Some(compat),
            result_code,
            data: result_data.map(str::to_string),
            extras: result_extras.map(|e| By(&e.bundle)),
            ordered: record.ordered,
            assume_delivered: false,
            sending_user: record.user,
            process_state: PROCESS_STATE_TOP,
            sent_from_uid: if record.share_identity {
                record.sender.uid
            } else {
                INVALID_UID
            },
            sent_from_package: record
                .share_identity
                .then(|| record.sender.package.clone())
                .flatten(),
        }
        .write(&mut p);
        Some(
            match host.call_thread(start_seq, thread::SCHEDULE_RECEIVER, &p) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!(
                        "app host: a broadcast was not delivered to {}: {e}",
                        m.process
                    );
                    false
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::uri::Uri;

    fn intent(action: Option<&str>, data: Option<&str>) -> Intent {
        Intent {
            action: action.map(Into::into),
            data: data.map(Uri::parse),
            ..Intent::default()
        }
    }

    /// An excluded permission excludes its holder; one with an app op only
    /// when the op is allowed too, its mode not asked of a receiver without
    /// the permission.
    #[test]
    fn an_excluded_permission_excludes_its_holder_with_its_op_allowed() {
        let mode = |m: i32| move |_: i32| Ok(m);
        assert!(excludes(true, None, mode(MODE_IGNORED)).unwrap());
        assert!(!excludes(false, None, mode(MODE_ALLOWED)).unwrap());
        assert!(excludes(true, Some(4), mode(MODE_ALLOWED)).unwrap());
        assert!(!excludes(true, Some(4), mode(MODE_IGNORED)).unwrap());
        assert!(
            !excludes(false, Some(4), |_| -> Result<i32, Exception> {
                panic!("asked a receiver without the permission")
            })
            .unwrap()
        );
    }

    /// A manifest receiver is checked once per attribution tag, or once
    /// without one.
    #[test]
    fn a_manifest_receiver_is_checked_per_attribution_tag() {
        assert_eq!(receiver_tags(None), vec![None]);
        assert_eq!(receiver_tags(Some(&[])), vec![None]);
        let tags = [Some("a".to_string()), None];
        assert_eq!(receiver_tags(Some(&tags)), vec![Some("a"), None]);
    }

    /// Ops by code: valid ones named as the original names them.
    #[test]
    fn broadcast_ops_are_named() {
        assert!(valid_op(0) && valid_op(NUM_OP - 1));
        assert!(!valid_op(OP_NONE) && !valid_op(NUM_OP));
        assert_eq!(
            op_names(4),
            ("android:read_contacts", "READ_CONTACTS".to_string())
        );
    }

    #[test]
    fn a_filter_takes_what_it_lists_and_an_intent_naming_nothing_finds_none() {
        let mut f = IntentFilter::default();
        f.add_action("a.TICK");
        assert!(takes(&f, &intent(Some("a.TICK"), None), None).unwrap());
        assert!(!takes(&f, &intent(Some("a.TOCK"), None), None).unwrap());
        // Data the filter does not name.
        assert!(!takes(&f, &intent(Some("a.TICK"), Some("content://x/y")), None).unwrap());
        // No action, type or scheme: no filter is a candidate.
        assert!(!takes(&f, &intent(None, None), None).unwrap());
        // No action but a scheme the filter takes: matched (null actions pass).
        f.add_data_scheme("content");
        assert!(takes(&f, &intent(None, Some("content://x/y")), None).unwrap());
    }

    #[test]
    fn intents_are_filter_equal_by_what_they_match_by() {
        let a = intent(Some("a"), Some("content://x"));
        assert!(filter_equals(&a, &intent(Some("a"), Some("content://x"))));
        assert!(!filter_equals(&a, &intent(Some("a"), Some("content://y"))));
        assert!(!filter_equals(
            &a,
            &Intent {
                package: Some("p".into()),
                ..a.clone()
            }
        ));
        // The flags are not compared.
        assert!(filter_equals(
            &a,
            &Intent {
                flags: 0x10,
                ..a.clone()
            }
        ));
    }

    #[test]
    fn options_are_read_from_their_bundle() {
        let mut p = Parcel::new();
        crate::bundle::write(
            &mut p,
            &[
                ("android:broadcast.flags", Value::Int(OPTION_SHARE_IDENTITY)),
                ("android:broadcast.requireCompatChangeId", Value::Long(42)),
                (
                    "android:broadcast.requireAllOfPermissions",
                    Value::StringArray(vec![Some("p".into())]),
                ),
            ],
        );
        let raw = RawBundle::read_from(&mut Reader::new(p.data(), &[])).unwrap();
        let o = Options::read(Some(&raw)).unwrap();
        assert_eq!(
            (
                o.flags,
                o.require_compat_change,
                o.temporary_allowlist_duration
            ),
            (OPTION_SHARE_IDENTITY, Some(42), 0)
        );
    }

    #[test]
    fn core_and_isolated_uids() {
        assert!(is_core(1000) && is_core(101_000) && !is_core(10_154));
        assert!(is_isolated(99_001) && is_isolated(190_001) && !is_isolated(10_154));
        assert!(can_access_unexported(1000) && !can_access_unexported(2000));
    }

    /// A registered target keeps the filter it matched, which its delivery
    /// is checked against again.
    #[test]
    fn registered_targets_keep_their_matched_filter() {
        let to = |handle, filter| Registered {
            handle,
            filter,
            start_seq: 7,
        };
        let merged = merge(Vec::new(), vec![(5, to(1, 2)), (0, to(3, 0))]);
        let kept: Vec<_> = merged
            .iter()
            .map(|t| match t {
                Target::Registered(r) => (r.handle, r.filter, r.start_seq),
                Target::Manifest(_) => panic!("no manifest receiver"),
            })
            .collect();
        assert_eq!(kept, vec![(1, 2, 7), (3, 0, 7)]);
    }

    /// The system sends as `android`, with no op, naming no required
    /// permissions when it requires none.
    #[test]
    fn the_system_sender_requires_what_it_names() {
        let none = Sender::system(&[]);
        assert_eq!(
            (none.uid, none.original_uid, none.package.as_deref()),
            (SYSTEM_UID, SYSTEM_UID, Some("android"))
        );
        assert!(none.required_null && none.app_op == OP_NONE);
        let some = Sender::system(&["android.permission.X".to_string()]);
        assert!(!some.required_null);
        assert_eq!(some.required_permissions, ["android.permission.X"]);
    }

    const LIMITS: Limits = Limits {
        max_running: 4,
        extra_urgent: 1,
    };

    /// A burst of manifest receivers in processes not running, as at
    /// BOOT_COMPLETED: one process starts at a time, at most four queues
    /// run, and they are delivered in the order queued.
    #[test]
    fn a_burst_of_cold_receivers_is_started_one_at_a_time() {
        let cold = Candidate {
            urgent: false,
            cold: true,
        };
        let mut runnable: VecDeque<usize> = (0..12).collect();
        // The queues delivering (attached, their receiver not finished).
        let mut running: VecDeque<usize> = VecDeque::new();
        let mut starting: Option<usize> = None;
        let mut delivered = Vec::new();
        let mut most = 0;
        while !runnable.is_empty() || !running.is_empty() || starting.is_some() {
            let candidates: Vec<Candidate> = runnable.iter().map(|_| cold).collect();
            let active = running.len() + usize::from(starting.is_some());
            if let Some(at) = next_to_run(&candidates, active, 0, starting.is_some(), LIMITS) {
                assert!(starting.is_none(), "a second cold start");
                starting = runnable.remove(at);
            } else if let Some(queue) = starting.take() {
                // The process attached; its receiver is sent.
                delivered.push(queue);
                running.push_back(queue);
            } else {
                // The oldest receiver finished.
                running.pop_front();
            }
            most = most.max(running.len() + usize::from(starting.is_some()));
        }
        assert_eq!(delivered, (0..12).collect::<Vec<_>>());
        assert_eq!(most, 4);
    }

    /// A warm queue runs while a process starts; past the limit only an
    /// urgent broadcast's queue runs, in the one extra slot.
    #[test]
    fn warm_and_urgent_queues_run_beside_a_cold_start() {
        let (warm, cold) = (
            Candidate {
                urgent: false,
                cold: false,
            },
            Candidate {
                urgent: false,
                cold: true,
            },
        );
        let urgent = Candidate {
            urgent: true,
            cold: false,
        };
        assert_eq!(
            next_to_run(&[cold, cold, warm], 1, 0, true, LIMITS),
            Some(2)
        );
        assert_eq!(next_to_run(&[cold], 1, 0, true, LIMITS), None);
        assert_eq!(next_to_run(&[warm, urgent], 4, 0, false, LIMITS), Some(1));
        assert_eq!(next_to_run(&[urgent], 5, 1, false, LIMITS), None);
        // A running urgent queue uses the extra slot first.
        assert_eq!(next_to_run(&[urgent], 4, 1, false, LIMITS), None);
        assert_eq!(next_to_run(&[warm], 3, 0, false, LIMITS), Some(0));
    }

    #[test]
    fn foreground_interactive_and_alarm_broadcasts_are_urgent() {
        assert!(urgent(FLAG_RECEIVER_FOREGROUND, 0));
        assert!(urgent(0, OPTION_INTERACTIVE));
        assert!(urgent(0, OPTION_IS_ALARM_BROADCAST));
        assert!(!urgent(FLAG_RECEIVER_NO_ABORT, OPTION_SHARE_IDENTITY));
    }
}
