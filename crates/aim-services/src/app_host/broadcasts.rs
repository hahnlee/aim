//! Broadcasts for the app host (`BroadcastController`,
//! `BroadcastQueueModernImpl` and `BroadcastSkipPolicy` at
//! `android-16.0.0_r1`): the receivers apps register, sticky broadcasts,
//! and a broadcast's delivery to the registered receivers it matches, each
//! process's in the order sent. An ordered broadcast, or one with a result
//! receiver, goes to one receiver at a time, each finishing before the
//! next. Manifest receivers are not delivered to (#1462), nor are the app
//! ops of a broadcast's permissions decided (#1463).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Strong};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::android_app_iactivitymanager as am;
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::android_content_iintentreceiver as intent_receiver;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::kept::{By, KeptBundle, RawBundle, RawIntent};
use super::{AppHost, PER_USER_RANGE, PROCESS_STATE_TOP, is_isolated};
use crate::bundle::{self, Value};
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
/// `PermissionInfo.PROTECTION_*`.
const PROTECTION_MASK_BASE: i32 = 0xf;
const PROTECTION_DANGEROUS: i32 = 1;
const PROTECTION_FLAG_APPOP: i32 = 0x40;
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
    calling_uid: i32,
    calling_package: Option<String>,
    ordered: bool,
    /// `initialSticky`: a sticky broadcast sent on to a new receiver.
    initial_sticky: bool,
    user: i32,
    share_identity: bool,
    /// The result receivers start from.
    result_code: i32,
    result_data: Option<String>,
    result_extras: Option<KeptBundle>,
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
    /// The receivers to come, and their processes' starts.
    targets: VecDeque<(u32, i64)>,
    result_code: i32,
    result_data: Option<String>,
    result_extras: Option<KeptBundle>,
    aborted: bool,
    /// The result receiver, and the start of its process if an app's.
    result_to: Option<(Strong, Option<i64>)>,
}

/// A delivery a process waits for, by its receiver.
enum Delivery {
    /// Sent and assumed delivered.
    Once(Arc<Record>, u32),
    /// A receiver of the chain with that id, which reports its finish.
    Chained(u64, u32),
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
    /// The processes whose first delivery is a chained one sent, which
    /// waits for its finish.
    busy: HashSet<i64>,
}

#[derive(Default)]
pub struct Broadcasts {
    state: Mutex<State>,
}

/// A sender, as BroadcastSkipPolicy checks a registered receiver for it.
struct Sender<'a> {
    uid: i32,
    /// The uid the `exported` check is of: a sticky broadcast's original
    /// sender's.
    original_uid: i32,
    instant: bool,
    required_permissions: &'a [String],
    excluded_permissions: &'a [String],
    excluded_packages: &'a [String],
    app_op: i32,
    require_compat_change: Option<(i64, bool)>,
}

/// What a sender's `BroadcastOptions` ask that the app host acts on.
#[derive(Default)]
struct Options {
    flags: i32,
    temporary_allowlist_duration: i64,
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
        Ok(Options {
            flags: match entries.get("android:broadcast.flags") {
                Some(Value::Int(v)) => *v,
                _ => 0,
            },
            temporary_allowlist_duration: long("android:broadcast.temporaryAppAllowlistDuration").unwrap_or(0),
            id_for_response_event: long("android:broadcast.idForResponseEvent").unwrap_or(0),
            // `CHANGE_INVALID` is no change.
            require_compat_change: long("android:broadcast.requireCompatChangeId").filter(|&id| id != 0),
        })
    }
}

fn strings(list: Option<Vec<Option<String>>>) -> Vec<String> {
    list.unwrap_or_default().into_iter().flatten().collect()
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
fn takes(filter: &IntentFilter, intent: &Intent, resolved_type: Option<&str>) -> Result<bool, Exception> {
    if intent.action.is_none() && resolved_type.is_none() && intent.scheme().is_none() {
        return Ok(false);
    }
    filter
        .matches(intent.action.as_deref(), resolved_type, intent.scheme(), intent.data.as_ref(), intent.categories.as_deref(), false, None)
        .map(|m| m >= 0)
        .map_err(|e| Exception::illegal_argument(format!("{e:?}")))
}

/// `Intent.filterEquals`.
fn filter_equals(a: &Intent, b: &Intent) -> bool {
    a.action == b.action
        && a.data.as_ref().map(|d| d.as_str()) == b.data.as_ref().map(|d| d.as_str())
        && a.ty == b.ty
        && a.identifier == b.identifier
        && a.package == b.package
        && a.component == b.component
        && a.categories == b.categories
}

impl AppHost {
    /// `CompatChanges.isChangeEnabled(change, uid)`: enabled for each of
    /// the uid's packages.
    fn change_enabled_for_uid(&self, change: i64, uid: i32) -> Result<bool, Exception> {
        let (state, _) = self.packages()?;
        let compat = &self.inputs()?.compat;
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        Ok(state.packages.values().filter(|ps| ps.app_id == app_id).all(|ps| compat.is_enabled(change, ps.target_sdk_version)))
    }

    /// Whether `package` is an instant app for `user`.
    fn instant_app(&self, package: &str, user: i32) -> Result<bool, Exception> {
        let (state, _) = self.packages()?;
        Ok(state.packages.get(package).and_then(|ps| ps.users.get(&user.max(0))).is_some_and(|u| u.instant_app))
    }

    /// Whether `permission` has an app op (`AppOpsManager.permissionToOp`):
    /// a runtime permission's, or an app op permission's.
    fn permission_has_op(&self, permission: &str) -> Result<bool, Exception> {
        Ok(self.inputs()?.packages.permissions().iter().find(|p| p.name == permission).is_some_and(|p| {
            p.protection_level & PROTECTION_MASK_BASE == PROTECTION_DANGEROUS || p.protection_level & PROTECTION_FLAG_APPOP != 0
        }))
    }

    /// Whether `uid` holds `permission`, and with it its app op, which the
    /// app host does not decide.
    fn holds(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        if !self.check_component_permission(Some(permission), uid, -1, true)? {
            return Ok(false);
        }
        if self.permission_has_op(permission)? {
            return Err(self.unimplemented_named("activity", &format!("the app op of {permission} in a broadcast (#1463)")));
        }
        Ok(true)
    }
}

impl Broadcasts {
    /// `registerReceiverWithFeature`: the first sticky broadcast the
    /// filter matches.
    pub fn register(&self, host: &AppHost, call: &Call<'_>, args: Registration) -> Result<Option<RawIntent>, Exception> {
        if is_isolated(call.sender_euid as i32) {
            return Err(Exception::security("Isolated process not allowed to call registerReceiver"));
        }
        let mut filter = args.filter.ok_or_else(|| Exception::new(EX_NULL_POINTER, "filter"))?.0;
        let Some((start_seq, package, uid, pid)) = host.app_of_thread(args.caller) else {
            return Ok(None);
        };
        let caller_package = args.caller_package.unwrap_or_default();
        if !is_core(uid) && caller_package != package {
            return Err(Exception::security(format!("Given caller package {caller_package} is not running in process {pid}:{package}/{uid}")));
        }
        let instant_app = host.instant_app(&caller_package, uid / PER_USER_RANGE as i32)?;
        let user = host.incoming_user(uid, args.user, false)?;
        // System internals hear these before apps unless they chose.
        // (An unset priority is no system one.)
        if is_core(uid) && filter.priority == 0 {
            let important = filter.actions.iter().any(|a| {
                ["android.intent.action.USER_", "android.intent.action.PACKAGE_", "android.intent.action.UID_", "android.intent.action.EXTERNAL_", "android.bluetooth."]
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
            only_protected &= crate::package::query::is_protected_broadcast(&model, action).map_err(|e| host.unimplemented_named("package", e.0))?;
        }
        let mut flags = args.flags;
        if flags & RECEIVER_EXPORTED != 0 && flags & RECEIVER_NOT_EXPORTED != 0 {
            return Err(Exception::illegal_argument("Receiver can't specify both RECEIVER_EXPORTED and RECEIVER_NOT_EXPORTEDflag"));
        }
        let require_explicit = host.change_enabled_for_uid(DYNAMIC_RECEIVER_EXPLICIT_EXPORT_REQUIRED, uid)?;
        if flags & RECEIVER_VISIBLE_TO_INSTANT_APPS != 0 && flags & RECEIVER_NOT_EXPORTED != 0 && require_explicit {
            return Err(Exception::illegal_argument("Receiver can't specify both RECEIVER_VISIBLE_TO_INSTANT_APPS and RECEIVER_NOT_EXPORTED flag"));
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
            if !action_listed || instant_app && r.intent.intent.flags & FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS == 0 {
                continue;
            }
            let i = &r.intent.intent;
            let m = filter
                .matches(i.action.as_deref(), sticky.resolved_type.as_deref(), i.scheme(), i.data.as_ref(), i.categories.as_deref(), false, None)
                .map_err(|e| Exception::illegal_argument(format!("{e:?}")))?;
            if m >= 0 {
                matched.push(r.clone());
            }
        }
        let Some(Binder::Handle(handle)) = args.receiver else {
            return Ok(matched.first().map(|r| r.intent.clone()));
        };
        match st.receivers.iter().position(|r| r.receiver.handle == handle) {
            Some(at) => {
                let r = &st.receivers[at];
                let mismatch = if r.uid != uid {
                    Some(format!("uid {uid} was previously registered for uid {}", r.uid))
                } else if r.pid != pid {
                    Some(format!("pid {pid} was previously registered for pid {}", r.pid))
                } else if r.user != user {
                    Some(format!("user {user} was previously registered for user {}", r.user))
                } else {
                    None
                };
                if let Some(m) = mismatch {
                    return Err(Exception::illegal_argument(format!("Receiver requested to register for {m} callerPackage is {caller_package}")));
                }
            }
            None => {
                let registered = st.receivers.iter().filter(|r| r.start_seq == start_seq).count();
                if registered >= MAX_RECEIVERS_ALLOWED_PER_APP {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("Too many receivers, total of {registered}, registered for pid: {pid}, callerPackage: {caller_package}"),
                    ));
                }
                st.receivers.push(Receiver { receiver: host.process.strong(handle), start_seq, pid, uid, user, filters: Vec::new() });
            }
        }
        let receiver = st.receivers.iter_mut().find(|r| r.receiver.handle == handle).unwrap();
        receiver.filters.push(Filter {
            filter,
            package: caller_package,
            permission: args.permission,
            instant_app,
            visible_to_instant_apps: flags & RECEIVER_VISIBLE_TO_INSTANT_APPS != 0,
            exported,
        });
        // Each matching sticky goes to the new receiver; the first it may
        // have is returned.
        let mut first = None;
        for r in matched {
            if first.is_none() && (exported || r.calling_uid == uid || can_access_unexported(r.calling_uid)) {
                first = Some(r.intent.clone());
            }
            let sender = Sender {
                uid: INVALID_UID,
                original_uid: r.calling_uid,
                instant: false,
                required_permissions: &[],
                excluded_permissions: &[],
                excluded_packages: &[],
                app_op: OP_NONE,
                require_compat_change: None,
            };
            let receiver = st.receivers.iter().find(|r| r.receiver.handle == handle).unwrap();
            if !self.skip(host, &sender, receiver, receiver.filters.last().unwrap(), r.intent.intent.flags)? {
                let replay = Record {
                    intent: r.intent.clone(),
                    binders: r.binders.clone(),
                    calling_uid: INVALID_UID,
                    calling_package: None,
                    ordered: false,
                    initial_sticky: true,
                    user: USER_ALL,
                    share_identity: false,
                    result_code: 0,
                    result_data: None,
                    result_extras: None,
                };
                st.queues.entry(start_seq).or_default().push_back(Delivery::Once(Arc::new(replay), handle));
            }
        }
        self.send_next(host, &mut st, start_seq);
        Ok(first)
    }

    /// `unregisterReceiver`: a delivery it is in the middle of finishes.
    pub fn unregister(&self, host: &AppHost, receiver: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = receiver else {
            return Err(Exception::new(EX_NULL_POINTER, "receiver"));
        };
        let mut st = self.state.lock().unwrap();
        let Some(at) = st.receivers.iter().position(|r| r.receiver.handle == handle) else {
            return Ok(());
        };
        let removed = st.receivers.remove(at);
        let seq = removed.start_seq;
        let waiting = st.queues.get(&seq).and_then(|q| q.front()).and_then(|d| match d {
            Delivery::Chained(id, h) if *h == handle => Some(*id),
            _ => None,
        });
        if let Some(id) = waiting.filter(|_| st.busy.remove(&seq)) {
            st.queues.get_mut(&seq).unwrap().pop_front();
            self.advance(host, &mut st, id);
            self.send_next(host, &mut st, seq);
        }
        Ok(())
    }

    /// `broadcastIntentWithFeature` (`broadcastIntentLocked`).
    pub fn broadcast(&self, host: &AppHost, call: &Call<'_>, args: am::BroadcastIntentWithFeature<RawIntent, RawBundle>) -> Result<i32, Exception> {
        let calling_uid = call.sender_euid as i32;
        if is_isolated(calling_uid) {
            return Err(Exception::security("Isolated process not allowed to call broadcastIntent"));
        }
        let mut intent = args.intent.ok_or_else(|| Exception::new(EX_NULL_POINTER, "intent"))?;
        // `verifyBroadcastLocked`.
        if intent.has_fds() {
            return Err(Exception::illegal_argument("File descriptors passed in Intent"));
        }
        let mut flags = intent.intent.flags;
        if flags & FLAG_RECEIVER_BOOT_UPGRADE != 0 {
            return Err(Exception::illegal_argument("Can't use FLAG_RECEIVER_BOOT_UPGRADE here"));
        }
        if flags & FLAG_RECEIVER_FROM_SHELL != 0 && !matches!(calling_uid, ROOT_UID | SHELL_UID) {
            flags &= !FLAG_RECEIVER_FROM_SHELL;
        }
        let extended_flags = intent.intent.extended_flags & !EXTENDED_FLAG_FILTER_MISMATCH;
        let caller = host.app_of_thread(args.caller);
        let caller_package = caller.as_ref().map(|c| c.1.clone());
        let options = Options::read(args.options.as_ref())?;
        // `enforceBroadcastOptionPermissionsInternal`.
        if calling_uid != SYSTEM_UID {
            if options.flags & OPTION_IS_ALARM_BROADCAST != 0 {
                return Err(Exception::security("Non-system callers may not flag broadcasts as alarm"));
            }
            if options.flags & OPTION_INTERACTIVE != 0 && !host.holds("android.permission.BROADCAST_OPTION_INTERACTIVE", calling_uid)? {
                return Err(Exception::security(format!(
                    "Permission Denial: setInteractive() from pid={}, uid={calling_uid} requires android.permission.BROADCAST_OPTION_INTERACTIVE",
                    call.sender_pid
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
                call.sender_pid
            ))
        };
        if options.temporary_allowlist_duration > 0
            && ![
                "android.permission.CHANGE_DEVICE_IDLE_TEMP_WHITELIST",
                "android.permission.START_ACTIVITIES_FROM_BACKGROUND",
                "android.permission.START_FOREGROUND_SERVICES_FROM_BACKGROUND",
            ]
            .iter()
            .try_fold(false, |held, p| Ok::<_, Exception>(held || host.holds(p, calling_uid)?))?
        {
            return Err(denied(
                "android.permission.CHANGE_DEVICE_IDLE_TEMP_WHITELIST or android.permission.START_ACTIVITIES_FROM_BACKGROUND or android.permission.START_FOREGROUND_SERVICES_FROM_BACKGROUND",
            ));
        }
        if options.flags & OPTION_ALLOW_BACKGROUND_ACTIVITY_STARTS != 0 && !host.holds("android.permission.START_ACTIVITIES_FROM_BACKGROUND", calling_uid)? {
            return Err(denied("android.permission.START_ACTIVITIES_FROM_BACKGROUND"));
        }
        if options.id_for_response_event > 0 && !host.holds("android.permission.ACCESS_BROADCAST_RESPONSE_STATS", calling_uid)? {
            return Err(Exception::security(format!(
                "recordResponseEventWhileInBackground: Neither user {calling_uid} nor current process has android.permission.ACCESS_BROADCAST_RESPONSE_STATS."
            )));
        }
        let result_to_app = caller.as_ref().map(|c| c.0);
        let mut st = self.state.lock().unwrap();
        // User 0 is the one user, and runs.
        if user != USER_ALL && user != 0 {
            self.cancel_result_to(host, &intent, result_to, result_to_app, user);
            return Ok(BROADCAST_FAILED_USER_STOPPED);
        }
        let (model, resolution) = host.packages()?;
        let action = intent.intent.action.clone();
        let protected = match action.as_deref() {
            Some(a) => crate::package::query::is_protected_broadcast(&model, a).map_err(|e| host.unimplemented_named("package", e.0))?,
            None => false,
        };
        // `isCallerSystem`: the core uids that send system broadcasts; the
        // app host runs no persistent processes.
        let caller_system = matches!(calling_uid.rem_euclid(PER_USER_RANGE as i32), 0 | 1000 | 1001 | 1002 | 1027 | 1068 | 1073);
        if !caller_system {
            if protected {
                return Err(Exception::security(format!(
                    "Permission Denial: not allowed to send broadcast {} from pid={}, uid={calling_uid}",
                    action.as_deref().unwrap_or("null"),
                    call.sender_pid
                )));
            }
            if matches!(action.as_deref(), Some("android.appwidget.action.APPWIDGET_CONFIGURE" | "android.appwidget.action.APPWIDGET_UPDATE")) {
                let a = action.as_deref().unwrap();
                let Some(package) = &caller_package else {
                    return Err(Exception::security(format!("Permission Denial: not allowed to send broadcast {a} from unknown caller.")));
                };
                match &intent.intent.component {
                    Some(c) if c.package != *package => {
                        return Err(Exception::security(format!(
                            "Permission Denial: not allowed to send broadcast {a} to {} from {package}",
                            c.package
                        )));
                    }
                    Some(_) => {}
                    // The receivers are the sender's.
                    None => return Err(host.unimplemented_named("activity", "a widget broadcast confined to its sender's package (#1462)")),
                }
            }
        }
        if let Some(a) = &action {
            if model.system.allow_implicit_broadcasts.contains(a) {
                flags |= FLAG_RECEIVER_INCLUDE_BACKGROUND;
            }
            match a.as_str() {
                "com.android.launcher.action.INSTALL_SHORTCUT" => {
                    eprintln!("app host: broadcast {a} no longer supported. It will not be delivered.");
                    self.cancel_result_to(host, &intent, result_to, result_to_app, user);
                    return Ok(BROADCAST_SUCCESS);
                }
                // `checkCanCloseSystemDialogs`: the permission, or an app
                // before S with a window over the shade (the app host's
                // apps are in front).
                "android.intent.action.CLOSE_SYSTEM_DIALOGS" => {
                    if !host.holds("android.permission.BROADCAST_CLOSE_SYSTEM_DIALOGS", calling_uid)?
                        && host.change_enabled_for_uid(LOCK_DOWN_CLOSE_SYSTEM_DIALOGS, calling_uid)?
                    {
                        return Err(Exception::security(format!(
                            "Permission Denial: android.intent.action.CLOSE_SYSTEM_DIALOGS broadcast from {} (pid={}, uid={calling_uid}) requires android.permission.BROADCAST_CLOSE_SYSTEM_DIALOGS.",
                            caller_package.as_deref().unwrap_or("null"),
                            call.sender_pid
                        )));
                    }
                }
                _ => {}
            }
        }
        intent.set_flags(flags, extended_flags);
        let required_permissions = strings(args.required_permissions);
        let excluded_permissions = strings(args.exclude_permissions);
        let excluded_packages = strings(args.exclude_packages);
        let resolved_type = args.resolved_type;
        // Manifest receivers, which the app host does not deliver to.
        if flags & FLAG_RECEIVER_REGISTERED_ONLY == 0 {
            let found = resolution
                .query_intent_receivers(&intent.intent, resolved_type.as_deref(), super::STOCK_PM_FLAGS, user.max(0), SYSTEM_UID)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("queryIntentReceivers: {e:?}")))?;
            if !found.is_empty() {
                return Err(host.unimplemented_named("activity", "a broadcast to manifest receivers (#1462)"));
            }
        }
        let binders = intent.handles().into_iter().map(|h| host.process.strong(h)).collect();
        let result_extras = args.map.map(|m| KeptBundle::new(m, &host.process)).transpose().map_err(|_| Exception::illegal_argument("result extras"))?;
        let record = Arc::new(Record {
            intent,
            binders: Arc::new(binders),
            calling_uid,
            calling_package: caller_package.clone(),
            ordered: args.serialized,
            initial_sticky: false,
            user,
            share_identity: options.flags & OPTION_SHARE_IDENTITY != 0,
            result_code: args.result_code,
            result_data: args.result_data,
            result_extras,
        });
        if args.sticky {
            if !host.check_component_permission(Some(BROADCAST_STICKY), calling_uid, -1, true)? {
                return Err(Exception::security(format!(
                    "Permission Denial: broadcastIntent() requesting a sticky broadcast from pid={}, uid={calling_uid} requires {BROADCAST_STICKY}",
                    call.sender_pid
                )));
            }
            if !required_permissions.is_empty() {
                self.cancel_result_to(host, &record.intent, result_to, result_to_app, user);
                return Ok(BROADCAST_STICKY_CANT_HAVE_PERMISSION);
            }
            if record.intent.intent.component.is_some() {
                return Err(Exception::security("Sticky broadcasts can't target a specific component"));
            }
            let i = &record.intent.intent;
            if user != USER_ALL && st.stickies.iter().any(|s| s.record.user == USER_ALL && filter_equals(&s.record.intent.intent, i)) {
                return Err(Exception::illegal_argument(format!(
                    "Sticky broadcast {} for user {user} conflicts with existing global broadcast",
                    i.action.as_deref().unwrap_or("null")
                )));
            }
            let sticky = Sticky { record: record.clone(), resolved_type: resolved_type.clone() };
            match st.stickies.iter().position(|s| s.record.user == user && filter_equals(&s.record.intent.intent, i)) {
                Some(at) => st.stickies[at] = sticky,
                None => st.stickies.push(sticky),
            }
        }

        // The registered receivers, by priority, one filter per receiver.
        let mut targets: Vec<(i32, u32, i64)> = Vec::new();
        let block_null_action = action.is_none()
            && !can_access_unexported(calling_uid)
            && model.system.flags.iter().any(|(name, on)| name == "android.security.block_null_action_intents" && *on)
            && host.change_enabled_for_uid(BLOCK_NULL_ACTION_INTENTS, calling_uid)?;
        if record.intent.intent.component.is_none() && !block_null_action {
            let i = &record.intent.intent;
            let sender = Sender {
                uid: calling_uid,
                original_uid: calling_uid,
                instant: caller_instant,
                required_permissions: &required_permissions,
                excluded_permissions: &excluded_permissions,
                excluded_packages: &excluded_packages,
                app_op: args.app_op,
                require_compat_change: options
                    .require_compat_change
                    .map(|id| (id, options.flags & OPTION_REQUIRE_COMPAT_CHANGE_ENABLED != 0)),
            };
            targets = self.registered_targets(host, &st, i, resolved_type.as_deref(), user, &sender)?;
        }
        if targets.is_empty() && result_to.is_none() {
            return Ok(BROADCAST_SUCCESS);
        }
        match result_to {
            None if !record.ordered => {
                let mut touched = Vec::new();
                for (_, handle, seq) in targets {
                    st.queues.entry(seq).or_default().push_back(Delivery::Once(record.clone(), handle));
                    touched.push(seq);
                }
                for seq in touched {
                    self.send_next(host, &mut st, seq);
                }
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
                        targets: targets.into_iter().map(|(_, h, seq)| (h, seq)).collect(),
                        aborted: false,
                        result_to: result_to.map(|h| (host.process.strong(h), result_to_app)),
                    },
                );
                self.advance(host, &mut st, id);
            }
        }
        Ok(BROADCAST_SUCCESS)
    }

    /// The registered receivers `intent` goes to, by priority, one filter
    /// per receiver (`mReceiverResolver.queryIntent`, then
    /// BroadcastSkipPolicy): priority, receiver, its process's start.
    fn registered_targets(
        &self,
        host: &AppHost,
        st: &State,
        i: &Intent,
        resolved_type: Option<&str>,
        user: i32,
        sender: &Sender<'_>,
    ) -> Result<Vec<(i32, u32, i64)>, Exception> {
        let mut targets = Vec::new();
        for r in &st.receivers {
            if user != USER_ALL && r.user != USER_ALL && r.user != user {
                continue;
            }
            let mut taken = None;
            for f in &r.filters {
                if i.package.as_ref().is_some_and(|p| *p != f.package) || !takes(&f.filter, i, resolved_type)? {
                    continue;
                }
                taken = Some(f);
                break;
            }
            let Some(f) = taken else { continue };
            if !self.skip(host, sender, r, f, i.flags)? {
                targets.push((f.filter.priority, r.receiver.handle, r.start_seq));
            }
        }
        targets.sort_by_key(|t| std::cmp::Reverse(t.0));
        Ok(targets)
    }

    /// `sendBroadcastAsUser(intent, UserHandle.ALL)` of a system service of
    /// the app host: to the registered receivers (manifest receivers:
    /// #1462).
    pub fn send_from_system(&self, host: &AppHost, mut intent: RawIntent) {
        let flags = intent.intent.flags | FLAG_EXCLUDE_STOPPED_PACKAGES;
        let extended = intent.intent.extended_flags;
        intent.set_flags(flags, extended);
        let binders = intent.handles().into_iter().map(|h| host.process.strong(h)).collect();
        let record = Arc::new(Record {
            intent,
            binders: Arc::new(binders),
            calling_uid: SYSTEM_UID,
            calling_package: Some("android".into()),
            ordered: false,
            initial_sticky: false,
            user: USER_ALL,
            share_identity: false,
            result_code: 0,
            result_data: None,
            result_extras: None,
        });
        let sender = Sender {
            uid: SYSTEM_UID,
            original_uid: SYSTEM_UID,
            instant: false,
            required_permissions: &[],
            excluded_permissions: &[],
            excluded_packages: &[],
            app_op: OP_NONE,
            require_compat_change: None,
        };
        let mut st = self.state.lock().unwrap();
        let targets = match self.registered_targets(host, &st, &record.intent.intent, None, USER_ALL, &sender) {
            Ok(targets) => targets,
            Err(e) => {
                eprintln!("app host: a system broadcast: {e:?}");
                return;
            }
        };
        let mut touched = Vec::new();
        for (_, handle, seq) in targets {
            st.queues.entry(seq).or_default().push_back(Delivery::Once(record.clone(), handle));
            touched.push(seq);
        }
        for seq in touched {
            self.send_next(host, &mut st, seq);
        }
    }

    /// `unbroadcastIntent`: a sticky broadcast goes.
    pub fn unbroadcast(&self, host: &AppHost, call: &Call<'_>, intent: Option<RawIntent>, user: i32) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        if intent.as_ref().is_some_and(RawIntent::has_fds) {
            return Err(Exception::illegal_argument("File descriptors passed in Intent"));
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
        if let Some(at) = st.stickies.iter().position(|s| s.record.user == user && filter_equals(&s.record.intent.intent, &intent.intent)) {
            st.stickies.remove(at);
        }
        Ok(())
    }

    /// `finishReceiver`: the caller's process finished the chained
    /// delivery it was sent; its result goes on to the next receiver.
    pub fn finish(&self, host: &AppHost, args: am::FinishReceiver<RawBundle>) -> Result<(), Exception> {
        if args.map.as_ref().is_some_and(RawBundle::has_fds) {
            return Err(Exception::illegal_argument("File descriptors passed in Bundle"));
        }
        let Some((seq, ..)) = host.app_of_thread(args.who) else {
            eprintln!("app host: finishReceiver: no app");
            return Ok(());
        };
        let extras = args.map.map(|m| KeptBundle::new(m, &host.process)).transpose().map_err(|_| Exception::illegal_argument("result extras"))?;
        let mut st = self.state.lock().unwrap();
        let Some(Delivery::Chained(id, _)) = st.queues.get(&seq).and_then(|q| q.front()).filter(|_| st.busy.contains(&seq)) else {
            return Ok(());
        };
        let id = *id;
        st.busy.remove(&seq);
        st.queues.get_mut(&seq).unwrap().pop_front();
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
        self.send_next(host, &mut st, seq);
        Ok(())
    }

    /// A process died: its receivers go, and what it was to finish is
    /// skipped.
    pub fn process_died(&self, host: &AppHost, start_seq: i64) {
        let mut st = self.state.lock().unwrap();
        st.receivers.retain(|r| r.start_seq != start_seq);
        st.busy.remove(&start_seq);
        for delivery in st.queues.remove(&start_seq).unwrap_or_default() {
            if let Delivery::Chained(id, _) = delivery {
                self.advance(host, &mut st, id);
            }
        }
    }

    /// BroadcastSkipPolicy's checks of a registered receiver: whether the
    /// broadcast skips it.
    fn skip(&self, host: &AppHost, s: &Sender<'_>, r: &Receiver, f: &Filter, intent_flags: i32) -> Result<bool, Exception> {
        if let Some((change, enabled)) = s.require_compat_change
            && host.change_enabled_for_uid(change, r.uid)? != enabled
        {
            return Ok(true);
        }
        if let Some(permission) = &f.permission
            && !host.holds(permission, s.uid)?
        {
            return Ok(true);
        }
        let visible = intent_flags & FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS != 0;
        if (!visible && f.instant_app || !f.visible_to_instant_apps && s.instant) && r.uid != s.uid {
            return Ok(true);
        }
        for permission in s.required_permissions {
            if !host.holds(permission, r.uid)? {
                return Ok(true);
            }
        }
        if s.required_permissions.is_empty() && !host.check_component_permission(None, r.uid, -1, true)? {
            return Ok(true);
        }
        for permission in s.excluded_permissions {
            if host.holds(permission, r.uid)? {
                return Ok(true);
            }
        }
        if s.excluded_packages.contains(&f.package) {
            return Ok(true);
        }
        if s.app_op != OP_NONE {
            return Err(host.unimplemented_named("activity", &format!("app op {} of a broadcast (#1463)", s.app_op)));
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
            let (record, handle, chain) = match front {
                Delivery::Once(record, handle) => (record.clone(), *handle, None),
                Delivery::Chained(id, handle) => match st.chains.get(id) {
                    Some(chain) => (chain.record.clone(), *handle, Some(*id)),
                    None => {
                        st.queues.get_mut(&start_seq).unwrap().pop_front();
                        continue;
                    }
                },
            };
            let registered = st.receivers.iter().any(|r| r.receiver.handle == handle);
            let sent = registered
                && match chain.and_then(|id| st.chains.get(&id)) {
                    Some(c) => self.deliver(host, start_seq, handle, &record, c.result_code, c.result_data.as_deref(), c.result_extras.as_ref(), false),
                    None => self.deliver(host, start_seq, handle, &record, record.result_code, record.result_data.as_deref(), record.result_extras.as_ref(), true),
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
            let Some(chain) = st.chains.get_mut(&id) else { return };
            let next = if chain.aborted { None } else { chain.targets.pop_front() };
            let Some((handle, seq)) = next else {
                let chain = st.chains.remove(&id).unwrap();
                self.send_result(host, chain);
                return;
            };
            if !st.receivers.iter().any(|r| r.receiver.handle == handle) {
                continue;
            }
            let queue = st.queues.entry(seq).or_default();
            queue.push_back(Delivery::Chained(id, handle));
            if queue.len() == 1 {
                self.send_next(host, st, seq);
            }
            return;
        }
    }

    /// The final result of a chain to its result receiver: through its
    /// process's thread, else to the receiver itself.
    fn send_result(&self, host: &AppHost, chain: Chain) {
        let Some((result_to, app)) = chain.result_to else { return };
        let record = &chain.record;
        let extras = chain.result_extras.as_ref().map(|e| By(&e.bundle));
        let sent = match app {
            Some(seq) => {
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
                    sent_from_uid: if record.share_identity { record.calling_uid } else { INVALID_UID },
                    sent_from_package: record.share_identity.then(|| record.calling_package.clone()).flatten(),
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
                result_to.transact(intent_receiver::PERFORM_RECEIVE, &p, true).map(|_| ()).map_err(|s| format!("{s}"))
            }
        };
        if let Err(e) = sent {
            eprintln!("app host: a broadcast's result was not delivered: {e}");
        }
    }

    /// `scheduleCanceledResultTo`.
    fn cancel_result_to(&self, host: &AppHost, intent: &RawIntent, result_to: Option<u32>, app: Option<i64>, user: i32) {
        let (Some(handle), Some(seq)) = (result_to, app) else { return };
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
            sent_from_uid: if record.share_identity { record.calling_uid } else { INVALID_UID },
            sent_from_package: record.share_identity.then(|| record.calling_package.clone()).flatten(),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::uri::Uri;

    fn intent(action: Option<&str>, data: Option<&str>) -> Intent {
        Intent { action: action.map(Into::into), data: data.map(Uri::parse), ..Intent::default() }
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
        assert!(!filter_equals(&a, &Intent { package: Some("p".into()), ..a.clone() }));
        // The flags are not compared.
        assert!(filter_equals(&a, &Intent { flags: 0x10, ..a.clone() }));
    }

    #[test]
    fn options_are_read_from_their_bundle() {
        let mut p = Parcel::new();
        crate::bundle::write(
            &mut p,
            &[
                ("android:broadcast.flags", Value::Int(OPTION_SHARE_IDENTITY)),
                ("android:broadcast.requireCompatChangeId", Value::Long(42)),
                ("android:broadcast.requireAllOfPermissions", Value::StringArray(vec![Some("p".into())])),
            ],
        );
        let raw = RawBundle::read_from(&mut Reader::new(p.data(), &[])).unwrap();
        let o = Options::read(Some(&raw)).unwrap();
        assert_eq!((o.flags, o.require_compat_change, o.temporary_allowlist_duration), (OPTION_SHARE_IDENTITY, Some(42), 0));
    }

    #[test]
    fn core_and_isolated_uids() {
        assert!(is_core(1000) && is_core(101_000) && !is_core(10_154));
        assert!(is_isolated(99_001) && is_isolated(190_001) && !is_isolated(10_154));
        assert!(can_access_unexported(1000) && !can_access_unexported(2000));
    }
}
