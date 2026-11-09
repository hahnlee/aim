//! Broadcasts for the app host (`BroadcastController`,
//! `BroadcastQueueModernImpl` and `BroadcastSkipPolicy` at
//! `android-16.0.0_r1`): the receivers apps register, sticky broadcasts,
//! and a broadcast's delivery to the receivers it matches, registered and
//! manifest ones merged by priority, each process's in the order sent. A
//! manifest receiver's process is started for it, and it is sent
//! (`scheduleReceiver`) once the process attached; its process waits for
//! its `finishReceiver` before its next. An ordered broadcast goes to one
//! receiver at a time, each finishing before the next; one not ordered with
//! a result receiver goes to all at once, its result after their finishes.
//! One not ordered whose receivers differ in priority is prioritized
//! (`BroadcastRecord.calculateBlockedUntilBeyondCount`): a receiver waits
//! for the receivers of higher priority before it, across processes only
//! where LIMIT_PRIORITY_SCOPE is disabled with `limit_priority_scope`.
//! A receiver is checked as a broadcast is enqueued, a
//! preflight's checks (`shouldSkipAtEnqueueMessage`) with
//! `avoid_note_op_at_enqueue`, its permissions' and the broadcast's app ops
//! checked rather than noted, and checked again as it is delivered
//! (`shouldSkipReceiver`), its app ops noted then: a receiver whose
//! permission or op went meanwhile is skipped, its delivery finished.
//! Each process's queue waits its turn to run (`updateRunningListLocked`):
//! at most `MAX_RUNNING_PROCESS_QUEUES` run at once, and one more for an
//! urgent broadcast, and of the processes not running only one is started
//! at a time (`mRunningColdStart`), its receivers' deliveries held until
//! its turn. A delivery waited for has a timeout, the foreground or the
//! background one (`dispatchReceivers`): past it, the delivery ends and its
//! process is not responding.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet, VecDeque};
use std::slice;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Strong};
use aim_binder_host::parcel::{
    Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_app_iactivitymanager as am;
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::android_content_iintentreceiver as intent_receiver;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::activity_user::AllowMode;
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
const FLAG_RECEIVER_REPLACE_PENDING: i32 = 0x2000_0000;
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
const LIMIT_PRIORITY_SCOPE_FLAG: &str = "com.android.server.am.limit_priority_scope";
/// `IntentFilter.SYSTEM_HIGH_PRIORITY`.
const SYSTEM_HIGH_PRIORITY: i32 = 1000;
const MAX_RECEIVERS_ALLOWED_PER_APP: usize = 1000;
/// The compat changes broadcasts follow.
const DYNAMIC_RECEIVER_EXPLICIT_EXPORT_REQUIRED: i64 = 161_145_287;
const BLOCK_NULL_ACTION_INTENTS: i64 = 293_560_872;
const LOCK_DOWN_CLOSE_SYSTEM_DIALOGS: i64 = 174_664_365;
/// `BroadcastRecord.LIMIT_PRIORITY_SCOPE`: a receiver's priority orders it
/// only within its process.
const LIMIT_PRIORITY_SCOPE: i64 = 371_307_720;
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
const ACTION_PRE_BOOT_COMPLETED: &str = "android.intent.action.PRE_BOOT_COMPLETED";

/// A delivery timeout started: when, for the process of which start and
/// with which generation, and whether of a foreground broadcast.
#[derive(Debug, PartialEq)]
struct Started {
    at: Instant,
    seq: i64,
    generation: u64,
    foreground: bool,
}

/// `ActivityManagerService.BROADCAST_FG_TIMEOUT` and `BROADCAST_BG_TIMEOUT`,
/// before `ro.hw_timeout_multiplier`.
const BROADCAST_FG_TIMEOUT: Duration = Duration::from_secs(10);
const BROADCAST_BG_TIMEOUT: Duration = Duration::from_secs(60);
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
    enqueue_id: u64,
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
    /// `BroadcastRecord.receivers`, as `isReceiverEquals` tells them
    /// apart: set when enqueued to them, not one at a time in a chain.
    receivers: OnceLock<Vec<ReceiverId>>,
}

impl Record {
    /// `isReplacePending` without a delivery group policy of its sender's
    /// (`BroadcastProcessQueue.enqueueOrReplaceBroadcast`).
    fn replaces_pending(&self) -> bool {
        self.intent.intent.flags & FLAG_RECEIVER_REPLACE_PENDING != 0
            && self.sender.delivery_group.is_none()
    }

    /// `BroadcastRecord.timeoutExempt`: PRE_BOOT_COMPLETED's receivers
    /// take as long as they need (`BroadcastController.broadcastIntentLocked`).
    fn timeout_exempt(&self) -> bool {
        self.intent.intent.action.as_deref() == Some(ACTION_PRE_BOOT_COMPLETED)
    }
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
/// the filter it matched (`BroadcastFilter`) and that filter's priority,
/// its process's start.
#[derive(Clone, Copy)]
struct Registered {
    handle: u32,
    filter: usize,
    priority: i32,
    start_seq: i64,
}

/// A receiver a broadcast goes to: a registered one or a manifest one.
enum Target {
    Registered(Registered),
    Manifest(Arc<Manifest>),
}

/// A receiver by `BroadcastRecord.isReceiverEquals`: a registered one by
/// its registration (its binder's, and which of its filters), a manifest
/// one by its component.
#[derive(Debug, PartialEq)]
enum ReceiverId {
    Registered(u32, usize),
    Manifest(Option<String>, Option<String>),
}

impl Target {
    fn id(&self) -> ReceiverId {
        match self {
            Target::Registered(r) => ReceiverId::Registered(r.handle, r.filter),
            Target::Manifest(m) => {
                let item = &m.info.info.item;
                ReceiverId::Manifest(item.package_name.clone(), item.name.clone())
            }
        }
    }

    /// `BroadcastRecord.getReceiverPriority`.
    fn priority(&self) -> i32 {
        match self {
            Target::Registered(r) => r.priority,
            Target::Manifest(m) => m.priority,
        }
    }
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

/// A broadcast with a result: an ordered one goes to its receivers one
/// after another, each finishing (`finishReceiver`) before the next; one
/// not ordered goes to all of them at once, each process's queue in turn
/// (`BroadcastQueueModernImpl`). Then it goes to its result receiver.
struct Chain {
    record: Arc<Record>,
    /// The receivers to come, and the index of the next among them all.
    targets: VecDeque<Target>,
    next: usize,
    /// Of a broadcast not ordered, the receivers sent it and not finished.
    outstanding: usize,
    result_code: i32,
    result_data: Option<String>,
    result_extras: Option<KeptBundle>,
    aborted: bool,
    /// The result receiver, and the start of its process if an app's.
    result_to: Option<(Strong, Option<i64>)>,
    /// A system service's own result receiver (`IIntentReceiver.Stub`).
    done: Option<Done>,
    shell_done: Option<ShellDone>,
}

/// What a system service of the app host runs when its ordered broadcast
/// has gone through its receivers.
pub type Done = Box<dyn FnOnce() + Send>;
pub(super) type ShellDone = Box<dyn FnOnce(i32, Option<String>, Option<KeptBundle>) + Send>;

/// A delivery a process waits for, by its receiver and the receiver's
/// index among its broadcast's.
enum Delivery {
    /// Sent and assumed delivered.
    Once(Arc<Record>, Registered, usize),
    /// A receiver of the chain with that id, which reports its finish.
    Chained(u64, Registered, usize),
    /// A manifest receiver, of a chain if one, which reports its finish.
    Manifest(Arc<Record>, Arc<Manifest>, Option<u64>, usize),
}

fn same_delivery_group(old: &Delivery, new: &Delivery) -> bool {
    let (old, new) = match (old, new) {
        (Delivery::Once(a, ar, _), Delivery::Once(b, br, _)) if ar.handle == br.handle => (a, b),
        (Delivery::Manifest(a, ar, None, _), Delivery::Manifest(b, br, None, _))
            if ar.uid == br.uid
                && ar.info.info.item.package_name == br.info.info.item.package_name
                && ar.info.info.item.name == br.info.info.item.name =>
        {
            (a, b)
        }
        _ => return false,
    };
    new.sender.delivery_group.is_some()
        && old.sender.delivery_group == new.sender.delivery_group
        && old.sender.uid == new.sender.uid
        && old.user == new.user
        && old.intent.intent.action == new.intent.intent.action
}

#[derive(Default)]
struct State {
    /// `mRegisteredReceivers`, in the order registered.
    receivers: Vec<Receiver>,
    stickies: Vec<Sticky>,
    chains: HashMap<u64, Chain>,
    next_chain: u64,
    next_enqueue: u64,
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
    /// The prioritized broadcasts delivering, by enqueue id.
    tranches: HashMap<u64, Tranches>,
    /// Of the `busy` processes, those whose delivery has a timeout
    /// (`timeoutScheduled`), and its generation, which tells a timer gone
    /// stale.
    timeouts: HashMap<i64, u64>,
    next_timeout: u64,
}

/// Of a prioritized broadcast (`BroadcastRecord.prioritized`), each
/// receiver's `blockedUntilBeyondCount`, which reached a terminal state,
/// and `beyondCount`: how many from the first did.
struct Tranches {
    blocked_until: Vec<i32>,
    terminal: Vec<bool>,
    beyond: usize,
}

impl Tranches {
    /// `isBlocked`.
    fn blocked(&self, index: usize) -> bool {
        (self.beyond as i32) < self.blocked_until[index]
    }

    /// `setDeliveryState` of receiver `index` to a terminal state: whether
    /// `beyondCount` changed.
    fn terminal(&mut self, index: usize) -> bool {
        self.terminal[index] = true;
        let old = self.beyond;
        while self.terminal.get(self.beyond) == Some(&true) {
            self.beyond += 1;
        }
        self.beyond != old
    }
}

/// `BroadcastRecord.calculateBlockedUntilBeyondCount` of a broadcast not
/// ordered to receivers of `priorities`, in order, with
/// `Flags.limitPriorityScope()` `limit_scope` and LIMIT_PRIORITY_SCOPE
/// enabled for receiver `i` as `change_enabled[i]`: until which
/// `beyondCount` each is blocked, -1 for none.
fn blocked_until_beyond_count(
    priorities: &[i32],
    limit_scope: bool,
    change_enabled: &[bool],
) -> Vec<i32> {
    let n = priorities.len();
    let mut blocked = vec![0; n];
    if limit_scope {
        let mut last_tranche_priority = 0;
        let mut current_tranche_priority = 0;
        // The last receiver of the previous tranche, and of it and of the
        // current one the last with the change disabled.
        let mut last_tranche_index = -1;
        let mut last_tranche_disabled = -1;
        let mut current_tranche_disabled = -1;
        for (i, &priority) in priorities.iter().enumerate() {
            if i == 0 {
                current_tranche_priority = priority;
                if !change_enabled[i] {
                    current_tranche_disabled = 0;
                }
                continue;
            }
            if priority != current_tranche_priority {
                if current_tranche_disabled != -1 {
                    last_tranche_disabled = current_tranche_disabled;
                }
                last_tranche_priority = current_tranche_priority;
                last_tranche_index = i as i32 - 1;
                current_tranche_priority = priority;
                current_tranche_disabled = -1;
            }
            if !change_enabled[i] {
                current_tranche_disabled = i as i32;
                blocked[i] = last_tranche_index + 1;
            } else if priority != last_tranche_priority && last_tranche_disabled != -1 {
                blocked[i] = last_tranche_disabled + 1;
            }
        }
        if n > 0
            && (last_tranche_index == -1
                || (last_tranche_disabled == -1 && current_tranche_disabled == -1))
        {
            blocked.fill(-1);
        }
    } else {
        let mut last_priority = 0;
        let mut last_priority_index = 0;
        for (i, &priority) in priorities.iter().enumerate() {
            if i == 0 || priority != last_priority {
                last_priority = priority;
                last_priority_index = i as i32;
            }
            blocked[i] = last_priority_index;
        }
        if n > 0 && blocked[n - 1] == 0 {
            blocked.fill(-1);
        }
    }
    blocked
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
    limits: OnceLock<Limits>,
    changed: Condvar,
    /// The worker that fires the delivery timeouts once due.
    timer: Mutex<Option<mpsc::Sender<Started>>>,
}

impl State {
    fn enqueue_id(&mut self) -> u64 {
        self.next_enqueue += 1;
        self.next_enqueue
    }

    fn pending_through(&self, barrier: u64) -> bool {
        let pending = |d: &Delivery| match d {
            Delivery::Once(r, ..) | Delivery::Manifest(r, ..) => r.enqueue_id <= barrier,
            Delivery::Chained(id, ..) => self
                .chains
                .get(id)
                .is_some_and(|c| c.record.enqueue_id <= barrier),
        };
        self.chains.values().any(|c| c.record.enqueue_id <= barrier)
            || self.queues.values().flatten().any(pending)
            || self.cold.iter().flat_map(|c| &c.deliveries).any(pending)
    }

    /// The delivery at the front of `queue`.
    fn front(&self, queue: &QueueId) -> Option<&Delivery> {
        match queue {
            QueueId::Warm(seq) => self.queues.get(seq).and_then(|q| q.front()),
            QueueId::Cold(process, uid) => self
                .cold
                .iter()
                .find(|c| &c.process == process && c.uid == *uid)
                .and_then(|c| c.deliveries.front()),
        }
    }

    /// The broadcast of `d`, by enqueue id, and its receiver's index.
    fn receiver(&self, d: &Delivery) -> Option<(u64, usize)> {
        match d {
            Delivery::Once(r, _, at) | Delivery::Manifest(r, _, _, at) => Some((r.enqueue_id, *at)),
            Delivery::Chained(id, _, at) => self.chains.get(id).map(|c| (c.record.enqueue_id, *at)),
        }
    }

    /// `allReceiversPending` of an unchained `record`: none of its
    /// receivers sent it or skipped, each still queued and not sent.
    fn all_pending(&self, record: &Arc<Record>) -> bool {
        let Some(receivers) = record.receivers.get() else {
            return false;
        };
        let mut pending = vec![false; receivers.len()];
        let warm = self
            .queues
            .iter()
            .flat_map(|(seq, q)| q.iter().skip(usize::from(self.busy.contains(seq))));
        for d in warm.chain(self.cold.iter().flat_map(|c| &c.deliveries)) {
            if let Delivery::Once(r, _, at) | Delivery::Manifest(r, _, None, at) = d
                && Arc::ptr_eq(r, record)
            {
                pending[*at] = true;
            }
        }
        pending.into_iter().all(|p| p)
    }

    /// `replaceBroadcastInQueue`: of `queue`, newest first, the position of
    /// the delivery `new` replaces in place, if any: the same receiver's of
    /// a filter-equal broadcast from the same sender and user to the same
    /// receivers, all pending (or `replaced` already by `new`'s broadcast,
    /// pending as it was). One of `new`'s own broadcast ends the search, as
    /// a replacement past it would invert their order.
    fn replace_pending(
        &self,
        queue: &VecDeque<Delivery>,
        new: &Delivery,
        replaced: &[Arc<Record>],
    ) -> Option<usize> {
        let (Delivery::Once(record, _, at) | Delivery::Manifest(record, _, None, at)) = new else {
            return None;
        };
        let receivers = record.receivers.get()?;
        if !record.replaces_pending() {
            return None;
        }
        for (i, d) in queue.iter().enumerate().rev() {
            if self
                .receiver(d)
                .is_some_and(|(id, _)| id == record.enqueue_id)
            {
                return None;
            }
            let (Delivery::Once(old, _, old_at) | Delivery::Manifest(old, _, None, old_at)) = d
            else {
                continue;
            };
            if old.sender.uid == record.sender.uid
                && old.user == record.user
                && filter_equals(&old.intent.intent, &record.intent.intent)
                && old.receivers.get() == Some(receivers)
                && receivers[*old_at] == receivers[*at]
                && (replaced.iter().any(|r| Arc::ptr_eq(r, old)) || self.all_pending(old))
            {
                return Some(i);
            }
        }
        None
    }

    /// `isBlocked`: whether `d` waits for receivers of its broadcast before
    /// it.
    fn blocked(&self, d: &Delivery) -> bool {
        self.receiver(d)
            .is_some_and(|(id, at)| self.tranches.get(&id).is_some_and(|t| t.blocked(at)))
    }
}

impl Broadcasts {
    /// Snapshot enqueue order under the queue lock. A chain keeps its original
    /// identity when it advances; later broadcasts do not extend a barrier.
    pub(super) fn wait(&self, barrier: bool) {
        let mut state = self.state.lock().unwrap();
        let through = if barrier {
            state.next_enqueue
        } else {
            u64::MAX
        };
        while state.pending_through(through) {
            state = self.changed.wait(state).unwrap();
        }
    }
}

/// A sender, as BroadcastSkipPolicy checks a receiver for it.
#[derive(Clone)]
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
    max_manifest_sdk: Option<i32>,
    delivery_group: Option<String>,
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
            max_manifest_sdk: None,
            delivery_group: None,
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
fn temp_allowlist(host: &AppHost, uid: i32, record: &Record) -> bool {
    let Some(t) = record.temp_allowlist else {
        return true;
    };
    if t.kind == TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED {
        if let Err(e) = host.temp_allowlist_for_fgs(
            uid,
            super::device_idle::TempAllowance {
                duration_ms: t.duration_ms,
                kind: t.kind,
                reason_code: t.reason,
                reason: None,
                calling_uid: record.sender.uid,
            },
        ) {
            eprintln!("app host: broadcast: temporary allowance: {}", e.message);
            return false;
        }
    }
    true
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
fn merge(manifest: Vec<Arc<Manifest>>, registered: Vec<Registered>) -> Vec<Target> {
    let mut merged = Vec::with_capacity(manifest.len() + registered.len());
    let mut manifest = manifest.into_iter().peekable();
    for r in registered {
        while let Some(m) = manifest.next_if(|m| m.priority > r.priority) {
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
    pub(super) fn instant_app(&self, package: &str, user: i32) -> Result<bool, Exception> {
        let (state, _) = self.packages()?;
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
    pub(super) fn stats_running(&self) -> std::collections::HashSet<i64> {
        self.state.lock().unwrap().busy.clone()
    }
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
        let caller_package = args.caller_package.as_deref().unwrap_or_default();
        if !is_core(uid)
            && !host
                .apps
                .lock()
                .unwrap()
                .get(&start_seq)
                .is_some_and(|app| app.packages.iter().any(|p| p == caller_package))
        {
            return Err(Exception::security(format!(
                "Given caller package {caller_package} is not running in process {pid}:{package}/{uid}"
            )));
        }
        let instant_app = host.instant_app(caller_package, uid / PER_USER_RANGE as i32)?;
        let user = host.handle_incoming_user(
            uid,
            args.user,
            true,
            AllowMode::FullOnly,
            Some("registerReceiver"),
            args.caller_package.as_deref(),
        )?;
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
        let (model, _) = host.packages()?;
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
            package: caller_package.to_string(),
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
                max_manifest_sdk: None,
                delivery_group: None,
            };
            let receiver = st
                .receivers
                .iter()
                .find(|r| r.receiver.handle == handle)
                .unwrap();
            let filter = receiver.filters.len() - 1;
            let priority = receiver.filters[filter].filter.priority;
            if !self.skip(
                host,
                &sender,
                receiver,
                &receiver.filters[filter],
                r.intent.intent.flags,
                preflight,
            )? {
                let replay = Record {
                    enqueue_id: st.enqueue_id(),
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
                    receivers: OnceLock::new(),
                };
                st.queues
                    .entry(start_seq)
                    .or_default()
                    .push_back(Delivery::Once(
                        Arc::new(replay),
                        Registered {
                            handle,
                            filter,
                            priority,
                            start_seq,
                        },
                        0,
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
                Delivery::Chained(id, r, _) if r.handle == handle => Some(*id),
                _ => None,
            });
        if let Some(id) = waiting.filter(|_| st.busy.remove(&seq)) {
            let finished = st.queues.get_mut(&seq).unwrap().pop_front().unwrap();
            Self::delivered(&mut st, &finished);
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
        self.broadcast_with_result(
            host,
            calling_uid,
            calling_pid,
            caller_app,
            caller_package,
            args,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn broadcast_with_result(
        &self,
        host: &AppHost,
        calling_uid: i32,
        calling_pid: i32,
        caller_app: Option<i64>,
        caller_package: Option<String>,
        args: am::BroadcastIntentWithFeature<RawIntent, RawBundle>,
        shell_done: Option<ShellDone>,
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
        let user = host.handle_incoming_user(
            calling_uid,
            args.user_id,
            true,
            AllowMode::NonFull,
            Some("broadcast"),
            caller_package.as_deref(),
        )?;
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
        let (model, _) = host.packages()?;
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
            max_manifest_sdk: None,
            delivery_group: None,
        };
        let record = Arc::new(Record {
            enqueue_id: st.enqueue_id(),
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
            receivers: OnceLock::new(),
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
        let mut registered = Vec::new();
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
        if targets.is_empty() && result_to.is_none() && shell_done.is_none() {
            return Ok(BROADCAST_SUCCESS);
        }
        self.prioritize(host, &mut st, &record, &targets)?;
        match result_to {
            None if !record.ordered && shell_done.is_none() => {
                self.enqueue(host, &mut st, &record, targets)
            }
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
                        next: 0,
                        outstanding: 0,
                        aborted: false,
                        result_to: result_to.map(|h| (host.process.strong(h), result_to_app)),
                        done: None,
                        shell_done,
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
    ) -> Result<Vec<Registered>, Exception> {
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
                targets.push(Registered {
                    handle: r.receiver.handle,
                    filter,
                    priority: f.filter.priority,
                    start_seq: r.start_seq,
                });
            }
        }
        targets.sort_by_key(|t| std::cmp::Reverse(t.priority));
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

    /// `Context.sendBroadcastAsUser(intent, user, null, options)` of a
    /// system service of the app host whose options' delivery group policy
    /// is MOST_RECENT, matching by `key`: a receiver's pending delivery of
    /// the group is replaced by this one.
    pub(super) fn send_from_system_as_user_most_recent(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        key: &str,
    ) {
        let mut sender = Sender::system(&[]);
        sender.delivery_group = Some(key.into());
        self.send_as(host, intent, user, None, sender);
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

    /// Package effects retain AppsFilter restrictions and BroadcastOptions.
    pub(super) fn try_send_package_effect(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
        permissions: &[String],
        temp_allowlist: Option<TempAllowlist>,
    ) -> Result<(), Exception> {
        self.send_with(
            host,
            intent,
            user,
            allow_list,
            Sender::system(permissions),
            temp_allowlist,
            None,
            false,
        )
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
        let serialized = done.is_some();
        if let Err(e) = self.send_with(
            host,
            intent,
            user,
            None,
            Sender::system(required_permissions),
            temp_allowlist,
            done,
            serialized,
        ) {
            eprintln!("app host: a system broadcast: {e:?}");
        }
    }

    /// `ActivityManagerInternal.broadcastIntentWithCallback` of a system
    /// service of the app host, to `user`'s registered receivers when the
    /// intent says so: not serialized, it goes to them all at once, they
    /// cannot abort it, and `done` runs once each has finished.
    pub fn send_from_system_with_callback(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        done: Done,
    ) {
        if let Err(e) = self.send_with(
            host,
            intent,
            user,
            None,
            Sender::system(&[]),
            None,
            Some(done),
            false,
        ) {
            eprintln!("app host: a system broadcast: {e:?}");
        }
    }

    fn send_as(
        &self,
        host: &AppHost,
        intent: RawIntent,
        user: i32,
        allow_list: Option<&[i32]>,
        sender: Sender,
    ) {
        if let Err(e) = self.send_with(host, intent, user, allow_list, sender, None, None, false) {
            eprintln!("app host: a system broadcast: {e:?}");
        }
    }

    /// A system service's broadcast, `serialized` or not: one receiver at
    /// a time when it has a `done`.
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
        serialized: bool,
    ) -> Result<(), Exception> {
        let mut flags = intent.intent.flags | FLAG_EXCLUDE_STOPPED_PACKAGES;
        let allowed_implicit = host.packages().map(|(model, _)| {
            intent
                .intent
                .action
                .as_ref()
                .is_some_and(|a| model.system.allow_implicit_broadcasts.contains(a))
        });
        let allowed_implicit = match allowed_implicit {
            Ok(allowed) => allowed,
            Err(e) => {
                if let Some(done) = done {
                    std::thread::spawn(done);
                }
                return Err(e);
            }
        };
        if allowed_implicit {
            flags |= FLAG_RECEIVER_INCLUDE_BACKGROUND;
        }
        let extended = intent.intent.extended_flags;
        intent.set_flags(flags, extended);
        let binders = intent
            .handles()
            .into_iter()
            .map(|h| host.process.strong(h))
            .collect();
        let preflight = match host.flag(AVOID_NOTE_OP_AT_ENQUEUE) {
            Ok(preflight) => preflight,
            Err(e) => {
                if let Some(done) = done {
                    std::thread::spawn(done);
                }
                return Err(e);
            }
        };
        let mut st = self.state.lock().unwrap();
        let record = Arc::new(Record {
            enqueue_id: st.enqueue_id(),
            urgent: urgent(intent.intent.flags, 0),
            intent,
            binders: Arc::new(binders),
            sender,
            ordered: serialized,
            initial_sticky: false,
            user,
            share_identity: false,
            result_code: 0,
            result_data: None,
            result_extras: None,
            id_for_response_event: 0,
            temp_allowlist,
            receivers: OnceLock::new(),
        });
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
                    .filter(|r| allowed(owner(r.handle).unwrap_or_default()))
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
            })
            .and_then(|targets| {
                self.prioritize(host, &mut st, &record, &targets)?;
                Ok(targets)
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
                        next: 0,
                        outstanding: 0,
                        aborted: false,
                        result_to: None,
                        done: Some(done),
                        shell_done: None,
                    },
                );
                self.advance(host, &mut st, id);
                self.update_running(host, &mut st);
            }
            (Err(e), done) => {
                if let Some(done) = done {
                    std::thread::spawn(done);
                }
                return Err(e);
            }
        }
        drop(st);
        // `broadcastIntentLocked`'s own handling of the zone's change.
        if record.intent.intent.action.as_deref() == Some("android.intent.action.TIMEZONE_CHANGED")
        {
            host.update_time_zone();
        }
        Ok(())
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
        let (_, resolution) = host.packages()?;
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
        if s.max_manifest_sdk
            .is_some_and(|max| app.target_sdk_version > max)
        {
            return Ok(Some(format!(
                "API level filtered: receiving {action} to {name}"
            )));
        }
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
    /// process's, else one waiting for its process to start, including the
    /// system process attached by SystemProcessHost.
    fn manifest_queue(&self, host: &AppHost, m: &Manifest) -> QueueId {
        match host.running_process(&m.process, m.uid) {
            Some((seq, _)) => QueueId::Warm(seq),
            None => QueueId::Cold(m.process.clone(), m.uid),
        }
    }

    /// Puts `delivery` on the queue it goes to, which becomes runnable: in
    /// place of a pending one it replaces, whose broadcast it returns.
    fn queue_delivery(
        &self,
        st: &mut State,
        to: QueueId,
        m: Option<&Manifest>,
        delivery: Delivery,
    ) -> Option<Arc<Record>> {
        self.enqueue_or_replace(st, to, m, delivery, &[])
    }

    /// `enqueueOrReplaceBroadcast`: [`Self::queue_delivery`], with the
    /// broadcasts that the broadcast of `delivery` already `replaced` for
    /// its other receivers.
    fn enqueue_or_replace(
        &self,
        st: &mut State,
        to: QueueId,
        m: Option<&Manifest>,
        delivery: Delivery,
        replaced: &[Arc<Record>],
    ) -> Option<Arc<Record>> {
        let queue = match &to {
            QueueId::Warm(seq) => st.queues.get(seq),
            QueueId::Cold(process, uid) => st
                .cold
                .iter()
                .find(|c| &c.process == process && c.uid == *uid)
                .map(|c| &c.deliveries),
        };
        if let Some(at) = queue.and_then(|q| st.replace_pending(q, &delivery, replaced)) {
            let queue = match &to {
                QueueId::Warm(seq) => st.queues.get_mut(seq),
                QueueId::Cold(process, uid) => st
                    .cold
                    .iter_mut()
                    .find(|c| &c.process == process && c.uid == *uid)
                    .map(|c| &mut c.deliveries),
            };
            let old = std::mem::replace(&mut queue.unwrap()[at], delivery);
            Self::delivered(st, &old);
            Self::make_runnable(st, to);
            let (Delivery::Once(record, ..) | Delivery::Manifest(record, ..)) = old else {
                unreachable!()
            };
            return Some(record);
        }
        let push = |queue: &mut VecDeque<Delivery>, sent: bool, delivery: Delivery| {
            // DELIVERY_GROUP_POLICY_MOST_RECENT, per receiver: the replaced
            // pending deliveries are skipped. The one a busy process was
            // sent stays at the front, untouched, for its finish.
            let pending = queue.split_off(usize::from(sent).min(queue.len()));
            let (replaced, kept): (VecDeque<_>, VecDeque<_>) = pending
                .into_iter()
                .partition(|old| same_delivery_group(old, &delivery));
            queue.extend(kept);
            queue.push_back(delivery);
            replaced
        };
        let replaced = match &to {
            QueueId::Warm(seq) => push(
                st.queues.entry(*seq).or_default(),
                st.busy.contains(seq),
                delivery,
            ),
            QueueId::Cold(process, uid) => match st
                .cold
                .iter_mut()
                .find(|c| &c.process == process && c.uid == *uid)
            {
                Some(c) => push(&mut c.deliveries, false, delivery),
                None => {
                    st.cold.push(Cold {
                        process: process.clone(),
                        uid: *uid,
                        package: m.map(manifest_package).unwrap_or_default(),
                        deliveries: VecDeque::from([delivery]),
                    });
                    VecDeque::new()
                }
            },
        };
        for old in replaced {
            Self::delivered(st, &old);
        }
        Self::make_runnable(st, to);
        None
    }

    /// `updateRunnableList`: the queue waits its turn, unless it runs or
    /// its next delivery is blocked.
    fn make_runnable(st: &mut State, queue: QueueId) {
        if let QueueId::Warm(seq) = queue
            && (st.busy.contains(&seq) || st.cold_start == Some(seq))
        {
            return;
        }
        if st.front(&queue).is_some_and(|d| st.blocked(d)) {
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
        match st.front(queue) {
            Some(Delivery::Once(record, ..) | Delivery::Manifest(record, ..)) => record.urgent,
            Some(Delivery::Chained(id, ..)) => st.chains.get(id).is_some_and(|c| c.record.urgent),
            None => false,
        }
    }

    /// `updateRunningListLocked`: the runnable queues promoted to running
    /// in their order while the limits allow, one process started at a
    /// time; a warm queue sends what it has, a cold one has its process
    /// started for its first receiver not skipped.
    fn update_running(&self, host: &AppHost, st: &mut State) {
        self.changed.notify_all();
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
                // Its front became blocked (a newer delivery of its group
                // replaced the one before): it runs again once unblocked.
                QueueId::Warm(seq)
                    if st.front(&QueueId::Warm(seq)).is_some_and(|d| st.blocked(d)) => {}
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
        // Before systemReady, `startProcessLocked` puts the process on hold
        // (`mProcessesOnHold`): its deliveries wait, not the caller.
        if host.processes_ready.get().is_none() {
            return;
        }
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
            Self::merge_cold(st, seq, cold.deliveries);
            Self::make_runnable(st, QueueId::Warm(seq));
            return;
        }
        while let Some(front) = cold.deliveries.front()
            && !st.blocked(front)
            && let Delivery::Manifest(record, m, ..) = front
            && self.skipped_manifest(host, record, m)
        {
            let skipped = cold.deliveries.pop_front().unwrap();
            Self::delivered(st, &skipped);
            if let Delivery::Manifest(_, _, Some(id), _) = skipped {
                self.advance(host, st, id);
            }
        }
        let Some(front) = cold.deliveries.front() else {
            return;
        };
        // Its process starts once its receiver is unblocked.
        if st.blocked(front) {
            st.cold.push(cold);
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
                Self::merge_cold(st, seq, cold.deliveries);
                st.cold_start = Some(seq);
                self.send_next(host, st, seq);
            }
            Err(e) => {
                eprintln!("app host: unable to launch {process} for broadcast: {e}");
                let failed = cold.deliveries.pop_front().unwrap();
                Self::delivered(st, &failed);
                if let Delivery::Manifest(_, _, Some(id), _) = failed {
                    self.advance(host, st, id);
                }
                if !cold.deliveries.is_empty() {
                    st.cold.push(cold);
                    Self::make_runnable(st, QueueId::Cold(process.to_string(), uid));
                }
            }
        }
    }

    /// The deliveries held for a process that now runs as `seq` join its
    /// queue in the order their broadcasts were enqueued, each side's own
    /// order kept: an older broadcast is not left behind a newer one.
    fn merge_cold(st: &mut State, seq: i64, cold: VecDeque<Delivery>) {
        let enqueued = |st: &State, d: &Delivery| st.receiver(d).map_or(u64::MAX, |(id, _)| id);
        let mut warm = st.queues.remove(&seq).unwrap_or_default();
        let mut merged = VecDeque::with_capacity(warm.len() + cold.len());
        // The delivery it was sent and has not finished stays its first.
        if st.busy.contains(&seq) {
            merged.extend(warm.pop_front());
        }
        for d in cold {
            let id = enqueued(st, &d);
            while let Some(w) = warm.front()
                && enqueued(st, w) <= id
            {
                merged.push_back(warm.pop_front().unwrap());
            }
            merged.push_back(d);
        }
        merged.extend(warm);
        st.queues.insert(seq, merged);
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
        let receivers = targets.iter().map(Target::id).collect();
        if record.receivers.set(receivers).is_err() {
            unreachable!("a broadcast is enqueued once");
        }
        let mut replaced = Vec::new();
        for (at, target) in targets.into_iter().enumerate() {
            let old = match target {
                Target::Registered(r) => self.enqueue_or_replace(
                    st,
                    QueueId::Warm(r.start_seq),
                    None,
                    Delivery::Once(record.clone(), r, at),
                    &replaced,
                ),
                Target::Manifest(m) => {
                    let to = self.manifest_queue(host, &m);
                    let delivery = Delivery::Manifest(record.clone(), m.clone(), None, at);
                    self.enqueue_or_replace(st, to, Some(&m), delivery, &replaced)
                }
            };
            replaced.extend(old.filter(|old| !replaced.iter().any(|r| Arc::ptr_eq(r, old))));
        }
        Self::skip_replaced(st, &replaced);
        self.update_running(host, st);
    }

    /// `skipAndCancelReplacedBroadcasts`: the replaced broadcasts' other
    /// receivers, all pending, are skipped.
    fn skip_replaced(st: &mut State, replaced: &[Arc<Record>]) {
        if replaced.is_empty() {
            return;
        }
        let of_replaced = |d: &Delivery| match d {
            Delivery::Once(r, ..) | Delivery::Manifest(r, _, None, _) => {
                replaced.iter().any(|old| Arc::ptr_eq(old, r))
            }
            _ => false,
        };
        let mut skipped = Vec::new();
        let queues = st.queues.values_mut();
        for queue in queues.chain(st.cold.iter_mut().map(|c| &mut c.deliveries)) {
            let (out, kept): (VecDeque<_>, _) =
                std::mem::take(queue).into_iter().partition(&of_replaced);
            *queue = kept;
            skipped.extend(out);
        }
        // A process left nothing to deliver is not started for it.
        st.cold.retain(|c| !c.deliveries.is_empty());
        let State { runnable, cold, .. } = st;
        runnable.retain(|q| match q {
            QueueId::Warm(_) => true,
            QueueId::Cold(process, uid) => {
                cold.iter().any(|c| &c.process == process && c.uid == *uid)
            }
        });
        for d in skipped {
            Self::delivered(st, &d);
        }
    }

    /// Keeps the tranches of `record` to `targets` if it is prioritized
    /// (`calculateBlockedUntilBeyondCount`, `isPrioritized`): not ordered,
    /// to receivers that differ in priority.
    fn prioritize(
        &self,
        host: &AppHost,
        st: &mut State,
        record: &Record,
        targets: &[Target],
    ) -> Result<(), Exception> {
        let priorities: Vec<i32> = targets.iter().map(Target::priority).collect();
        // One tranche blocks none, whatever the change's states.
        if record.ordered || priorities.windows(2).all(|p| p[0] == p[1]) {
            return Ok(());
        }
        let limit_scope = host.flag(LIMIT_PRIORITY_SCOPE_FLAG)?;
        let change_enabled = if limit_scope {
            targets
                .iter()
                .map(|t| Self::limits_priority_scope(host, t))
                .collect::<Result<_, _>>()?
        } else {
            Vec::new()
        };
        let blocked_until = blocked_until_beyond_count(&priorities, limit_scope, &change_enabled);
        if blocked_until[0] != -1 {
            st.tranches.insert(
                record.enqueue_id,
                Tranches {
                    terminal: vec![false; blocked_until.len()],
                    blocked_until,
                    beyond: 0,
                },
            );
        }
        Ok(())
    }

    /// `calculateChangeStateForReceivers`: whether LIMIT_PRIORITY_SCOPE is
    /// enabled for the application of `target` (`getReceiverAppInfo`: a
    /// manifest receiver's, a registered one's process's).
    fn limits_priority_scope(host: &AppHost, target: &Target) -> Result<bool, Exception> {
        let info = match target {
            Target::Manifest(m) => Some(m.info.info.application_info.clone()),
            Target::Registered(r) => {
                let app = host
                    .apps
                    .lock()
                    .unwrap()
                    .get(&r.start_seq)
                    .map(|a| (a.application_info.clone(), a.package.clone()));
                match app {
                    Some((Some(info), _)) => Some(info),
                    Some((None, package)) => {
                        host.application_info(&package, 0, None)?.map(Arc::new)
                    }
                    None => None,
                }
            }
        };
        let compat = &host.platform()?.compat;
        Ok(match info {
            Some(ai) => compat.is_enabled(
                LIMIT_PRIORITY_SCOPE,
                ai.item.package_name.as_deref(),
                ai.target_sdk_version,
            ),
            None => compat.default_value(LIMIT_PRIORITY_SCOPE),
        })
    }

    /// `setDeliveryState` of receiver `at` of the broadcast of enqueue id
    /// `id` to a terminal state: past a new `beyondCount`, the queues of its
    /// receivers it unblocks become runnable.
    fn terminal(st: &mut State, id: u64, at: usize) {
        let Some(tranches) = st.tranches.get_mut(&id) else {
            return;
        };
        if !tranches.terminal(at) {
            return;
        }
        if tranches.beyond == tranches.terminal.len() {
            st.tranches.remove(&id);
        }
        let warm = st.queues.keys().map(|seq| QueueId::Warm(*seq));
        let cold = st
            .cold
            .iter()
            .map(|c| QueueId::Cold(c.process.clone(), c.uid));
        let queues: Vec<QueueId> = warm
            .chain(cold)
            .filter(|q| {
                st.front(q)
                    .and_then(|d| st.receiver(d))
                    .is_some_and(|(of, _)| of == id)
            })
            .collect();
        for queue in queues {
            Self::make_runnable(st, queue);
        }
    }

    /// [`Self::terminal`] of the receiver of `d`.
    fn delivered(st: &mut State, d: &Delivery) {
        if let Some((id, at)) = st.receiver(d) {
            Self::terminal(st, id, at);
        }
    }

    /// `ActivityManager.broadcastStickyIntent(intent, OP_NONE,
    /// USER_ALL)` of a system service of the app host: kept as the sticky
    /// of all users, in place of one filter-equal to it, then sent as
    /// [`Self::send_from_system`] sends.
    pub fn send_sticky_from_system(&self, host: &AppHost, intent: RawIntent) {
        self.send_sticky_from_system_with_policy(host, intent, None, None);
    }

    pub(super) fn send_sticky_from_system_with_policy(
        &self,
        host: &AppHost,
        mut intent: RawIntent,
        max_manifest_sdk: Option<i32>,
        delivery_group: Option<String>,
    ) {
        let mut sender = Sender::system(&[]);
        sender.max_manifest_sdk = max_manifest_sdk;
        sender.delivery_group = delivery_group;
        let flags = intent.intent.flags | FLAG_EXCLUDE_STOPPED_PACKAGES;
        let extended = intent.intent.extended_flags;
        intent.set_flags(flags, extended);
        let record = Arc::new(Record {
            enqueue_id: 0,
            intent: intent.clone(),
            binders: Arc::new(
                intent
                    .handles()
                    .into_iter()
                    .map(|h| host.process.strong(h))
                    .collect(),
            ),
            sender: sender.clone(),
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
            receivers: OnceLock::new(),
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
        self.send_as(host, intent, USER_ALL, None, sender);
    }

    /// The process of `start_seq` attached: the queue started for it runs
    /// (`onApplicationAttachedLocked`), and another process may be started.
    /// `systemReady`'s start of the processes on hold: the cold queues run.
    pub fn processes_ready(&self, host: &AppHost) {
        let mut st = self.state.lock().unwrap();
        let cold: Vec<_> = st
            .cold
            .iter()
            .map(|c| QueueId::Cold(c.process.clone(), c.uid))
            .collect();
        for queue in cold {
            Self::make_runnable(&mut st, queue);
        }
        self.update_running(host, &mut st);
    }

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
        let user = host.handle_incoming_user(
            uid,
            user,
            true,
            AllowMode::NonFull,
            Some("removeStickyBroadcast"),
            None,
        )?;
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
            Some(Delivery::Chained(id, ..)) => Some(*id),
            Some(Delivery::Manifest(_, _, id, _)) => *id,
            _ => return Ok(()),
        };
        if let Some(chain) = id.and_then(|id| st.chains.get_mut(&id))
            && chain.record.ordered
        {
            chain.result_code = args.result_code;
            chain.result_data = args.result_data;
            chain.result_extras = extras;
            if chain.record.intent.intent.flags & FLAG_RECEIVER_NO_ABORT == 0 {
                chain.aborted = args.abort_broadcast;
            }
        }
        self.finish_active(host, &mut st, seq);
        self.send_next(host, &mut st, seq);
        self.update_running(host, &mut st);
        Ok(())
    }

    /// `finishReceiverActiveLocked`: the delivery the process of `seq` was
    /// sent and waits for ends, its timeout cancelled, and its chain moves
    /// on.
    fn finish_active(&self, host: &AppHost, st: &mut State, seq: i64) {
        st.busy.remove(&seq);
        st.timeouts.remove(&seq);
        let finished = st.queues.get_mut(&seq).unwrap().pop_front().unwrap();
        Self::delivered(st, &finished);
        if let Delivery::Chained(id, ..) | Delivery::Manifest(_, _, Some(id), _) = finished {
            self.advance(host, st, id);
        }
    }

    /// `dispatchReceivers`' timeout of the delivery of `record` the process
    /// of `seq` was sent and waits for: once processes are ready, unless the
    /// broadcast is exempt, the foreground or the background one.
    fn start_timeout(&self, host: &AppHost, st: &mut State, seq: i64, record: &Record) {
        if host.processes_ready.get().is_none() || record.timeout_exempt() {
            return;
        }
        st.next_timeout += 1;
        let generation = st.next_timeout;
        st.timeouts.insert(seq, generation);
        // The worker reads how long, off the queue's lock.
        let started = Started {
            at: Instant::now(),
            seq,
            generation,
            foreground: record.intent.intent.flags & FLAG_RECEIVER_FOREGROUND != 0,
        };
        self.timer
            .lock()
            .unwrap()
            .get_or_insert_with(|| Self::timer(host.me.clone()))
            .send(started)
            .expect("broadcast timer worker");
    }

    /// `BroadcastConstants.TIMEOUT` of the foreground or the background
    /// constants: the `timeout` of the `bcast_fg_constants` or
    /// `bcast_bg_constants` setting (a `KeyValueListParser` list, in
    /// milliseconds), else `BROADCAST_FG_TIMEOUT` or `BROADCAST_BG_TIMEOUT`
    /// times `ro.hw_timeout_multiplier`.
    fn timeout(host: &AppHost, foreground: bool) -> Duration {
        let (name, default) = if foreground {
            ("bcast_fg_constants", BROADCAST_FG_TIMEOUT)
        } else {
            ("bcast_bg_constants", BROADCAST_BG_TIMEOUT)
        };
        let setting = match host.platform() {
            Ok(platform) => platform.settings.get(super::settings::Table::Global, name),
            Err(e) => {
                eprintln!("app host: broadcast timeout: {}", e.message);
                None
            }
        };
        let set = setting.as_deref().and_then(|list| {
            list.split(',')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| k.trim() == "timeout")
                .and_then(|(_, v)| v.trim().parse().ok())
        });
        set.map_or_else(
            || {
                let multiplier = host
                    .property("ro.hw_timeout_multiplier")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                default * multiplier
            },
            Duration::from_millis,
        )
    }

    /// A worker that fires each delivery timeout started once it is due,
    /// until the queue goes.
    fn timer(host: Weak<AppHost>) -> mpsc::Sender<Started> {
        let (sender, started) = mpsc::channel::<Started>();
        std::thread::spawn(move || {
            let mut pending: Vec<(Instant, i64, u64)> = Vec::new();
            loop {
                let received = match pending.iter().map(|(at, ..)| *at).min() {
                    Some(at) => started.recv_timeout(at.saturating_duration_since(Instant::now())),
                    None => started
                        .recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                };
                let Some(host) = host.upgrade() else { return };
                match received {
                    Ok(s) => {
                        let due = s.at + Self::timeout(&host, s.foreground);
                        pending.push((due, s.seq, s.generation));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let now = Instant::now();
                        let due: Vec<_>;
                        (due, pending) = pending.into_iter().partition(|(at, ..)| *at <= now);
                        for (_, seq, generation) in due {
                            host.broadcasts.timed_out(&host, seq, generation);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        sender
    }

    /// `deliveryTimeoutLocked`: the delivery the process of `seq` did not
    /// finish in time ends (`DELIVERY_TIMEOUT`) and its chain or tranche
    /// moves on; its process is not responding (`appNotResponding`), killed
    /// first if its ANR is silent, else its queue waits its turn again
    /// (`demoteFromRunningLocked`). A timeout gone stale, its delivery
    /// finished, does nothing.
    pub(super) fn timed_out(&self, host: &AppHost, seq: i64, generation: u64) {
        let reason = {
            let st = self.state.lock().unwrap();
            if st.timeouts.get(&seq) != Some(&generation) {
                return;
            }
            Self::timeout_reason(&st, st.queues[&seq].front().unwrap())
        };
        // Off the queue's lock: a death finishes what its process had.
        let killed = host.app_not_responding(seq, &reason);
        let mut st = self.state.lock().unwrap();
        if st.timeouts.get(&seq) != Some(&generation) {
            return;
        }
        self.finish_active(host, &mut st, seq);
        // A killed process is sent nothing more; its death retires its queue.
        if killed {
            st.runnable.retain(|q| *q != QueueId::Warm(seq));
        } else if st.queues.get(&seq).is_some_and(|q| !q.is_empty()) {
            Self::make_runnable(&mut st, QueueId::Warm(seq));
        }
        self.update_running(host, &mut st);
    }

    /// `TimeoutRecord.forBroadcastReceiver`'s reason for the delivery `d`,
    /// the intent naming its receiver's component or package.
    fn timeout_reason(st: &State, d: &Delivery) -> String {
        let (record, receiver) = match d {
            Delivery::Manifest(record, m, ..) => (
                Some(record),
                format!(
                    "cmp={}/{}",
                    manifest_package(m),
                    m.info.info.item.name.as_deref().unwrap_or_default()
                ),
            ),
            Delivery::Chained(id, to, _) => (
                st.chains.get(id).map(|c| &c.record),
                st.receivers
                    .iter()
                    .find(|r| r.receiver.handle == to.handle)
                    .map(|r| format!("pkg={}", r.filters[to.filter].package))
                    .unwrap_or_default(),
            ),
            Delivery::Once(record, ..) => (Some(record), String::new()),
        };
        let action = record
            .and_then(|r| r.intent.intent.action.as_deref())
            .map(|a| format!("act={a} "))
            .unwrap_or_default();
        format!("Broadcast of Intent {{ {action}{receiver} }}")
    }

    /// A process died: its receivers go, and what it was to finish is
    /// skipped.
    pub fn process_died(&self, host: &AppHost, start_seq: i64) {
        let mut st = self.state.lock().unwrap();
        st.receivers.retain(|r| r.start_seq != start_seq);
        st.busy.remove(&start_seq);
        st.timeouts.remove(&start_seq);
        if st.cold_start == Some(start_seq) {
            st.cold_start = None;
        }
        st.runnable.retain(|q| *q != QueueId::Warm(start_seq));
        for delivery in st.queues.remove(&start_seq).unwrap_or_default() {
            Self::delivered(&mut st, &delivery);
            if let Delivery::Chained(id, ..) | Delivery::Manifest(_, _, Some(id), _) = delivery {
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
            // It runs again once unblocked.
            if st.blocked(front) {
                return;
            }
            if let Delivery::Manifest(record, m, chain, _) = front {
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
                        self.start_timeout(host, st, start_seq, &record);
                        return;
                    }
                    Some(false) => {
                        let skipped = st.queues.get_mut(&start_seq).unwrap().pop_front().unwrap();
                        Self::delivered(st, &skipped);
                        if let Some(id) = chain {
                            self.advance(host, st, id);
                        }
                        continue;
                    }
                }
            }
            let (record, to, chain) = match front {
                Delivery::Once(record, to, _) => (record.clone(), *to, None),
                Delivery::Chained(id, to, _) => match st.chains.get(id) {
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
                self.start_timeout(host, st, start_seq, &record);
                return;
            }
            let done = st.queues.get_mut(&start_seq).unwrap().pop_front().unwrap();
            Self::delivered(st, &done);
            if let Some(id) = chain {
                self.advance(host, st, id);
            }
        }
    }

    /// Moves chain `id` on to its next receiver, or, past its last or
    /// aborted, to its result receiver. One not ordered is sent to all its
    /// receivers at its start, and each finish counts down to its result.
    fn advance(&self, host: &AppHost, st: &mut State, id: u64) {
        if let Some(chain) = st.chains.get_mut(&id)
            && !chain.record.ordered
        {
            if chain.outstanding > 0 {
                chain.outstanding -= 1;
            } else {
                let record = chain.record.clone();
                let targets = std::mem::take(&mut chain.targets);
                let mut sent = 0;
                for (at, target) in targets.into_iter().enumerate() {
                    match target {
                        Target::Registered(r) => {
                            if !st.receivers.iter().any(|x| x.receiver.handle == r.handle) {
                                Self::terminal(st, record.enqueue_id, at);
                                continue;
                            }
                            let delivery = Delivery::Chained(id, r, at);
                            self.queue_delivery(st, QueueId::Warm(r.start_seq), None, delivery);
                        }
                        Target::Manifest(m) => {
                            let to = self.manifest_queue(host, &m);
                            let delivery =
                                Delivery::Manifest(record.clone(), m.clone(), Some(id), at);
                            self.queue_delivery(st, to, Some(&m), delivery);
                        }
                    }
                    sent += 1;
                }
                st.chains.get_mut(&id).unwrap().outstanding = sent;
            }
            if st.chains[&id].outstanding > 0 {
                return;
            }
        }
        loop {
            let Some(chain) = st.chains.get_mut(&id) else {
                return;
            };
            let at = chain.next;
            let next = if chain.aborted {
                None
            } else {
                chain.next += 1;
                chain.targets.pop_front()
            };
            let Some(target) = next else {
                let mut chain = st.chains.remove(&id).unwrap();
                // Off the queue's lock: it may broadcast again.
                if let Some(done) = chain.done.take() {
                    std::thread::spawn(done);
                }
                if let Some(done) = chain.shell_done.take() {
                    let (code, data, extras) = (
                        chain.result_code,
                        chain.result_data.clone(),
                        chain.result_extras.clone(),
                    );
                    std::thread::spawn(move || done(code, data, extras));
                }
                self.send_result(host, chain);
                return;
            };
            match target {
                Target::Registered(r) => {
                    if !st.receivers.iter().any(|x| x.receiver.handle == r.handle) {
                        continue;
                    }
                    let delivery = Delivery::Chained(id, r, at);
                    self.queue_delivery(st, QueueId::Warm(r.start_seq), None, delivery);
                }
                Target::Manifest(m) => {
                    let record = chain.record.clone();
                    let to = self.manifest_queue(host, &m);
                    let delivery = Delivery::Manifest(record, m.clone(), Some(id), at);
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
        if let Some(uid) = uid
            && !temp_allowlist(host, uid as i32, record)
        {
            return false;
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
        host.apps
            .lock()
            .unwrap()
            .get_mut(&start_seq)?
            .add_package(&manifest_package(m));
        if !temp_allowlist(host, m.uid as i32, record) {
            return Some(false);
        }
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
#[path = "package_internal_events_delivery_tests.rs"]
pub(super) mod internal_package_tests;

#[cfg(test)]
#[path = "system_receiver_tests.rs"]
mod system_receiver_tests;

#[cfg(test)]
#[path = "shutdown_broadcast_tests.rs"]
mod shutdown_broadcast_tests;

#[cfg(test)]
#[path = "priority_tranche_tests.rs"]
mod priority_tranche_tests;

#[cfg(test)]
#[path = "broadcast_timeout_tests.rs"]
mod broadcast_timeout_tests;

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

    /// Before systemReady a cold queue's start is held, not waited for by
    /// its caller; readiness, or the boot's failure, runs it.
    #[test]
    fn cold_starts_are_held_until_processes_are_ready() {
        for ready in [Ok(()), Err(Exception::new(EX_ILLEGAL_STATE, "boot failed"))] {
            let host = super::super::AppHost::new(crate::test_process(1000));
            let broadcasts = &host.broadcasts;
            let mut st = broadcasts.state.lock().unwrap();
            st.cold.push(Cold {
                process: "p".into(),
                uid: 10100,
                package: "p".into(),
                deliveries: VecDeque::new(),
            });
            broadcasts.start_cold(&host, &mut st, "p", 10100);
            assert_eq!(st.cold.len(), 1);
            drop(st);
            host.processes_ready.set(ready).unwrap();
            broadcasts.processes_ready(&host);
            assert!(broadcasts.state.lock().unwrap().cold.is_empty());
            host.process.shutdown().unwrap();
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
        let to = |handle, filter, priority| Registered {
            handle,
            filter,
            priority,
            start_seq: 7,
        };
        let merged = merge(Vec::new(), vec![to(1, 2, 5), to(3, 0, 0)]);
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

#[cfg(test)]
mod wait_tests {
    use super::*;

    #[test]
    fn connectivity_manifest_sdk_cap_does_not_limit_registered_receivers() {
        crate::system::with_discovery_binders(|host, _, _, _| {
            let permissions = super::super::permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
            let mut sender = Sender::system(&[]);
            sender.max_manifest_sdk = Some(23);
            let mut m = system_receiver_tests::receiver("p", "p.Legacy", 10100);
            let intent = RawIntent::new("android.net.conn.CONNECTIVITY_CHANGE", 0, &[]);
            assert!(
                host.broadcasts
                    .skip_manifest(host, &sender, &m.info, &intent.intent, true)
                    .unwrap()
                    .unwrap()
                    .starts_with("API level filtered")
            );
            Arc::make_mut(&mut Arc::get_mut(&mut m).unwrap().info.info.application_info)
                .target_sdk_version = 23;
            let skipped = host
                .broadcasts
                .skip_manifest(host, &sender, &m.info, &intent.intent, true)
                .unwrap();
            assert!(skipped.is_none(), "{skipped:?}");
            // Registered receivers have no SDK gate; production delivery to
            // a modern registered process is exercised by connectivity's fixture.
        });
    }

    #[test]
    fn most_recent_connectivity_group_replaces_only_pending_matching_receiver() {
        let broadcasts = Broadcasts::default();
        let mut state = State::default();
        let make = |id, key: Option<&str>| {
            let mut r = record(id);
            let r_mut = Arc::get_mut(&mut r).unwrap();
            r_mut.ordered = false;
            r_mut.sender.delivery_group = key.map(str::to_string);
            r
        };
        let mut other = registered(1);
        other.handle = 2;
        for (id, key, receiver) in [
            (1, Some("9;-1;mac"), registered(1)),
            (2, Some("9;-1;null"), registered(1)),
            (3, Some("9;-1;mac"), other),
            (4, None, registered(1)),
            (5, Some("9;-1;mac"), registered(1)),
        ] {
            broadcasts.queue_delivery(
                &mut state,
                QueueId::Warm(1),
                None,
                Delivery::Once(make(id, key), receiver, 0),
            );
        }
        let ids: Vec<_> = state.queues[&1]
            .iter()
            .map(|d| match d {
                Delivery::Once(r, ..) => r.enqueue_id,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ids, [2, 3, 4, 5]);
        assert!(!same_delivery_group(
            &Delivery::Chained(0, registered(1), 0),
            &Delivery::Once(make(6, Some("9;-1;mac")), registered(1), 0)
        ));
        let a = system_receiver_tests::receiver("p", "p.Legacy", 10100);
        let mut b = system_receiver_tests::receiver("p", "p.Legacy", 10100);
        Arc::get_mut(&mut b).unwrap().info.info.item.label_res = 42;
        let old = Delivery::Manifest(make(7, Some("9;-1;mac")), a, None, 0);
        assert!(same_delivery_group(
            &old,
            &Delivery::Manifest(make(8, Some("9;-1;mac")), b, None, 0)
        ));
        let other = system_receiver_tests::receiver("p", "p.Other", 10100);
        assert!(!same_delivery_group(
            &old,
            &Delivery::Manifest(make(9, Some("9;-1;mac")), other, None, 0)
        ));
    }

    #[test]
    fn most_recent_group_leaves_the_sent_manifest_delivery_for_its_finish() {
        crate::system::with_discovery_binders(|host, _, _, _| {
            let mut state = State::default();
            let make = |id| {
                let mut r = record(id);
                let r_mut = Arc::get_mut(&mut r).unwrap();
                r_mut.ordered = false;
                r_mut.sender.delivery_group = Some("k".into());
                r
            };
            let m = system_receiver_tests::receiver("p", "p.R", 10100);
            // The process is busy with 1, sent already.
            state.busy.insert(1);
            state.queues.insert(
                1,
                VecDeque::from([Delivery::Manifest(make(1), m.clone(), None, 0)]),
            );
            host.broadcasts.queue_delivery(
                &mut state,
                QueueId::Warm(1),
                None,
                Delivery::Manifest(make(2), m.clone(), None, 0),
            );
            assert_eq!(queued(&state, 1), [1, 2]);
            // A newer one replaces 2, pending, but not the sent 1.
            host.broadcasts.queue_delivery(
                &mut state,
                QueueId::Warm(1),
                None,
                Delivery::Manifest(make(3), m, None, 0),
            );
            assert_eq!(queued(&state, 1), [1, 3]);
            // The finish of 1 ends 1; 3 is sent next.
            host.broadcasts.finish_active(host, &mut state, 1);
            assert!(!state.busy.contains(&1));
            assert_eq!(queued(&state, 1), [3]);
        });
    }

    /// A broadcast not ordered of action `a` and data `data`, its extra
    /// `n`, flagged `flags`, to registered receivers `handles`.
    fn replacing(id: u64, data: &str, n: i32, flags: i32, handles: &[u32]) -> Arc<Record> {
        let mut r = record(id);
        let r_mut = Arc::get_mut(&mut r).unwrap();
        r_mut.ordered = false;
        r_mut.intent = RawIntent::with_data(
            Some("a"),
            Some(&crate::package::uri::Uri::parse(data)),
            None,
            flags,
            &[("n", Value::Int(n))],
        );
        let ids = handles
            .iter()
            .map(|h| ReceiverId::Registered(*h, 0))
            .collect();
        r_mut.receivers.set(ids).unwrap();
        r
    }

    fn queued(state: &State, seq: i64) -> Vec<u64> {
        state.queues[&seq]
            .iter()
            .map(|d| state.receiver(d).unwrap().0)
            .collect()
    }

    #[test]
    fn replace_pending_replaces_a_pending_filter_equal_broadcast_in_place() {
        let broadcasts = Broadcasts::default();
        let mut state = State::default();
        const P: i32 = FLAG_RECEIVER_REPLACE_PENDING;
        // The receiver's process is busy with the first, sent already.
        state.busy.insert(1);
        for (id, data, n, flags) in [
            (1, "content://x", 0, P),
            (2, "content://x", 1, P),
            (3, "content://y", 0, P),
            (4, "content://x", 0, 0),
            (5, "content://x", 2, P), // extras differ: still filter-equal
            (6, "content://y", 0, 0),
        ] {
            let replaced = broadcasts.queue_delivery(
                &mut state,
                QueueId::Warm(1),
                None,
                Delivery::Once(replacing(id, data, n, flags, &[1]), registered(1), 0),
            );
            assert_eq!(replaced.map(|r| r.enqueue_id), (id == 5).then_some(4));
        }
        // 5 took 4's place, the newest filter-equal pending; 2 stays, as do
        // the sent 1 and those of other data.
        assert_eq!(queued(&state, 1), [1, 2, 3, 5, 6]);
        let replaced = broadcasts.queue_delivery(
            &mut state,
            QueueId::Warm(1),
            None,
            Delivery::Once(replacing(7, "content://y", 0, 0, &[1]), registered(1), 0),
        );
        assert!(replaced.is_none());
        assert_eq!(queued(&state, 1), [1, 2, 3, 5, 6, 7]);
    }

    #[test]
    fn replace_pending_needs_the_same_receivers_all_pending() {
        let broadcasts = Broadcasts::default();
        let mut state = State::default();
        const P: i32 = FLAG_RECEIVER_REPLACE_PENDING;
        let mut second = registered(2);
        second.handle = 2;
        // 1 went to receiver 2, its process busy with it, and waits for 1.
        let sent = replacing(1, "content://x", 0, P, &[1, 2]);
        state.busy.insert(2);
        state
            .queues
            .insert(2, VecDeque::from([Delivery::Once(sent.clone(), second, 1)]));
        state
            .queues
            .insert(1, VecDeque::from([Delivery::Once(sent, registered(1), 0)]));
        // 2 goes to receiver 1 alone.
        let only = replacing(2, "content://x", 0, P, &[1]);
        let replaced = broadcasts.queue_delivery(
            &mut state,
            QueueId::Warm(1),
            None,
            Delivery::Once(only, registered(1), 0),
        );
        assert!(replaced.is_none());
        let both = replacing(3, "content://x", 0, P, &[1, 2]);
        let replaced = broadcasts.queue_delivery(
            &mut state,
            QueueId::Warm(1),
            None,
            Delivery::Once(both.clone(), registered(1), 0),
        );
        assert!(replaced.is_none());
        assert_eq!(queued(&state, 1), [1, 2, 3]);

        // Of one all pending, each receiver's delivery is replaced, and
        // what the replacement does not reach is skipped.
        let mut state = State::default();
        let old = replacing(1, "content://x", 0, P, &[1, 2]);
        state.queues.insert(
            1,
            VecDeque::from([Delivery::Once(old.clone(), registered(1), 0)]),
        );
        state
            .queues
            .insert(2, VecDeque::from([Delivery::Once(old, second, 1)]));
        let new = replacing(2, "content://x", 0, P, &[1, 2]);
        let replaced = broadcasts
            .queue_delivery(
                &mut state,
                QueueId::Warm(1),
                None,
                Delivery::Once(new, registered(1), 0),
            )
            .unwrap();
        assert_eq!(queued(&state, 1), [2]);
        Broadcasts::skip_replaced(&mut state, &[replaced]);
        assert!(state.queues[&2].is_empty());

        // Its next receiver's is replaced in place too, though the first
        // one's no longer pends.
        let mut state = State::default();
        let old = replacing(1, "content://x", 0, P, &[1, 2]);
        let later = replacing(2, "content://z", 0, 0, &[2]);
        state.queues.insert(
            1,
            VecDeque::from([Delivery::Once(old.clone(), registered(1), 0)]),
        );
        state.queues.insert(
            2,
            VecDeque::from([
                Delivery::Once(old, second, 1),
                Delivery::Once(later, second, 0),
            ]),
        );
        let new = replacing(3, "content://x", 0, P, &[1, 2]);
        let mut replaced = Vec::new();
        for (seq, to, at) in [(1, registered(1), 0), (2, second, 1)] {
            let old = broadcasts.enqueue_or_replace(
                &mut state,
                QueueId::Warm(seq),
                None,
                Delivery::Once(new.clone(), to, at),
                &replaced,
            );
            assert_eq!(old.as_ref().map(|r| r.enqueue_id), Some(1));
            replaced.extend(old);
        }
        assert_eq!(queued(&state, 1), [3]);
        assert_eq!(queued(&state, 2), [3, 2]);
    }

    fn record(enqueue_id: u64) -> Arc<Record> {
        Arc::new(Record {
            enqueue_id,
            intent: RawIntent::new("fixture.broadcast", 0, &[]),
            binders: Arc::new(vec![]),
            sender: Sender::system(&[]),
            ordered: true,
            initial_sticky: false,
            user: 0,
            share_identity: false,
            result_code: 0,
            result_data: None,
            result_extras: None,
            id_for_response_event: 0,
            temp_allowlist: None,
            urgent: false,
            receivers: OnceLock::new(),
        })
    }
    fn registered(start_seq: i64) -> Registered {
        Registered {
            handle: 1,
            filter: 0,
            priority: 0,
            start_seq,
        }
    }

    #[test]
    fn barrier_tracks_earlier_chains_across_continuations_and_ignores_later_work() {
        let mut state = State::default();
        let earlier = record(state.enqueue_id());
        state.chains.insert(
            0,
            Chain {
                record: earlier.clone(),
                targets: VecDeque::new(),
                next: 0,
                outstanding: 0,
                result_code: 0,
                result_data: None,
                result_extras: None,
                aborted: false,
                result_to: None,
                done: None,
                shell_done: None,
            },
        );
        let barrier = state.next_enqueue;
        let later = record(state.enqueue_id());
        state
            .queues
            .insert(9, VecDeque::from([Delivery::Once(later, registered(9), 0)]));
        assert!(state.pending_through(barrier)); // a chain between receivers
        state
            .queues
            .insert(1, VecDeque::from([Delivery::Chained(0, registered(1), 0)]));
        assert!(state.pending_through(barrier));
        state.queues.remove(&1); // next receiver is queued after the barrier
        state.cold.push(Cold {
            process: "fixture".into(),
            uid: 10100,
            package: "p".into(),
            deliveries: VecDeque::from([Delivery::Once(earlier, registered(1), 0)]),
        });
        state.chains.remove(&0);
        assert!(state.pending_through(barrier)); // cold work is not idle
        let cold = state.cold.pop().unwrap();
        state.queues.insert(1, cold.deliveries);
        state.busy.insert(1);
        assert!(state.pending_through(barrier)); // attached, still waiting for finish
        state.queues.remove(&1);
        state.busy.remove(&1);
        assert!(!state.pending_through(barrier));
        assert!(state.pending_through(u64::MAX)); // global idle includes later work
    }

    #[test]
    fn process_death_wakes_idle_waiters() {
        let host = AppHost::new(crate::test_process(1000));
        {
            let mut state = host.broadcasts.state.lock().unwrap();
            let record = record(state.enqueue_id());
            state.queues.insert(
                7,
                VecDeque::from([Delivery::Once(record, registered(7), 0)]),
            );
            state.busy.insert(7);
        }
        let (send, receive) = std::sync::mpsc::channel();
        let waiting_host = host.clone();
        let waiter = std::thread::spawn(move || {
            waiting_host.broadcasts.wait(false);
            send.send(()).unwrap();
        });
        assert!(
            receive
                .recv_timeout(std::time::Duration::from_millis(30))
                .is_err()
        );
        host.broadcasts.process_died(&host, 7);
        let completed = receive.recv_timeout(std::time::Duration::from_secs(2));
        host.broadcasts.changed.notify_all();
        waiter.join().unwrap();
        completed.unwrap();
    }

    #[test]
    fn a_cold_start_exiting_before_its_watch_is_retired_after_the_queue_lock() {
        use std::time::{Duration, Instant};
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        child.kill().unwrap();
        child.wait().unwrap();
        let host = super::super::tests::host_with_start(7, Arc::new(|| {}));
        host.apps.lock().unwrap().get_mut(&7).unwrap().pid = pid;
        // start_cold watches its start with the state held; the death
        // retires the queue (process_died) under the same lock.
        let (send, receive) = std::sync::mpsc::channel();
        let starter = host.clone();
        std::thread::spawn(move || {
            let state = starter.broadcasts.state.lock().unwrap();
            starter.watch_start(7);
            drop(state);
            send.send(()).unwrap();
        });
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("the start's death re-entered the broadcast state lock");
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.apps.lock().unwrap().contains_key(&7) {
            assert!(
                Instant::now() < deadline,
                "the early exit was not delivered"
            );
            std::thread::yield_now();
        }
    }
}
