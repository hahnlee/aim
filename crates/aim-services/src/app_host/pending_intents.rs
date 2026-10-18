//! PendingIntents in the app host, as `PendingIntentController` and
//! `PendingIntentRecord` keep and send them at `android-16.0.0_r1`: a
//! record by its key (type, package, activity, request code, the last
//! intent by `filterEquals`, flags, user), made, updated, cancelled or
//! returned as `FLAG_*` ask, an `IIntentSender` of its own; sent as its
//! creator (an activity started in its package, a broadcast, a service),
//! the sender's intent filled in unless it is immutable, once if one-shot,
//! its finished receiver told. Results to an activity
//! (`createPendingResult`) are not sent, and the creator's activity
//! options are not applied to its start (#1493).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iactivitymanager as am;
use aim_service_aidl::android_content_iintentreceiver as intent_receiver;
use aim_service_aidl::android_content_iintentsender as sender;

use super::activity_start::StartArgs;
use super::broadcasts::filter_equals;
use super::kept::{By, KeptIntent, RawBundle, RawIntent};
use super::{AppHost, PER_USER_RANGE};

/// `ActivityManager.INTENT_SENDER_*`.
const INTENT_SENDER_BROADCAST: i32 = 1;
const INTENT_SENDER_ACTIVITY: i32 = 2;
const INTENT_SENDER_ACTIVITY_RESULT: i32 = 3;
const INTENT_SENDER_SERVICE: i32 = 4;
const INTENT_SENDER_FOREGROUND_SERVICE: i32 = 5;
const INTENT_SENDER_UNKNOWN: i32 = 0;
/// `PendingIntent.FLAG_*`.
const FLAG_ONE_SHOT: i32 = 1 << 30;
const FLAG_NO_CREATE: i32 = 1 << 29;
const FLAG_CANCEL_CURRENT: i32 = 1 << 28;
const FLAG_UPDATE_CURRENT: i32 = 1 << 27;
const FLAG_IMMUTABLE: i32 = 1 << 26;
const FLAG_MUTABLE: i32 = 1 << 25;
const FLAG_ALLOW_UNSAFE_IMPLICIT_INTENT: i32 = 1 << 24;
/// `Intent.FLAG_RECEIVER_BOOT_UPGRADE`.
const FLAG_RECEIVER_BOOT_UPGRADE: i32 = 0x0200_0000;
/// `PendingIntent.BLOCK_MUTABLE_IMPLICIT_PENDING_INTENT`.
const BLOCK_MUTABLE_IMPLICIT_PENDING_INTENT: i64 = 236_704_164;
/// `ActivityManager.START_SUCCESS`, `START_CANCELED`; `Activity.RESULT_CANCELED`.
const START_SUCCESS: i32 = 0;
const START_CANCELED: i32 = -96;
const RESULT_CANCELED: i32 = 0;
/// `Process.INVALID_UID`; `UserHandle.USER_CURRENT`.
const INVALID_UID: i32 = -1;
const USER_CURRENT: i32 = -2;
/// `IResultReceiver.send`.
const RESULT_RECEIVER_DESCRIPTOR: &str = "com.android.internal.os.IResultReceiver";
const RESULT_RECEIVER_SEND: u32 = 1;

/// `PendingIntentRecord.Key`, and what the record holds.
struct Record {
    binder: Binder,
    kind: i32,
    package: String,
    feature: Option<String>,
    activity: Option<Binder>,
    result_who: Option<String>,
    request_code: i32,
    /// Its intents, the last its request intent, and their resolved
    /// types.
    intents: Vec<KeptIntent>,
    resolved_types: Option<Vec<Option<String>>>,
    flags: i32,
    user: i32,
    /// The creator's uid.
    uid: i32,
    canceled: bool,
    cancel_listeners: Vec<Strong>,
}

impl Record {
    /// `Key.equals`: the same type, package, feature, activity, result
    /// who, request code, request intent (`filterEquals`) and resolved
    /// type, flags and user.
    fn matches(&self, other: &Record) -> bool {
        fn last(r: &Record) -> Option<&crate::package::intent::Intent> {
            r.intents.last().map(|i| &i.intent.intent)
        }
        let resolved = |r: &Record| {
            r.resolved_types
                .as_ref()
                .and_then(|t| t.last().cloned().flatten())
        };
        self.kind == other.kind
            && self.package == other.package
            && self.feature == other.feature
            && self.activity == other.activity
            && self.result_who == other.result_who
            && self.request_code == other.request_code
            && match (last(self), last(other)) {
                (Some(a), Some(b)) => filter_equals(a, b),
                (None, None) => true,
                _ => false,
            }
            && resolved(self) == resolved(other)
            && self.flags == other.flags
            && self.user == other.user
    }
}

#[derive(Default)]
pub struct PendingIntents {
    records: Mutex<Vec<Record>>,
}

/// A record's `IIntentSender` (its binder, once made): `send` (one-way)
/// sends it as `sendIntentSender` does without a caller.
struct IntentSender {
    host: Weak<AppHost>,
    me: OnceLock<Binder>,
}

impl Service for IntentSender {
    fn descriptor(&self) -> &str {
        sender::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != sender::SEND {
            return Err(UNKNOWN_TRANSACTION);
        }
        let a =
            sender::Send::<RawIntent, RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
        if let Some(host) = self.host.upgrade()
            && let Err(e) = host.send_intent_sender(
                call.sender_euid as i32,
                None,
                self.me.get().copied(),
                a.code,
                a.intent,
                a.resolved_type,
                a.finished_receiver,
                a.required_permission,
                a.options,
            )
        {
            eprintln!("app host: IIntentSender.send: {}", e.message);
        }
        Ok(Parcel::new())
    }
}

/// `ActivityManager.PendingIntentInfo`.
struct Info {
    package: Option<String>,
    uid: i32,
    immutable: bool,
    kind: i32,
}

impl WriteParcelable for Info {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.package.as_deref());
        p.write_i32(self.uid);
        p.write_bool(self.immutable);
        p.write_i32(self.kind);
    }
}

/// `getIntentSenderWithFeature`'s arguments.
pub struct Request {
    pub kind: i32,
    pub package: Option<String>,
    pub feature: Option<String>,
    pub token: Option<Binder>,
    pub result_who: Option<String>,
    pub request_code: i32,
    pub intents: Option<Vec<Option<RawIntent>>>,
    pub resolved_types: Option<Vec<Option<String>>>,
    pub flags: i32,
    pub options: Option<RawBundle>,
    pub user: i32,
}

impl AppHost {
    /// `getIntentSenderWithFeatureAsApp` and
    /// `PendingIntentController.getIntentSender`: the record's
    /// `IIntentSender`, none with `FLAG_NO_CREATE` and no record.
    pub(super) fn get_intent_sender(
        &self,
        uid: i32,
        req: Request,
    ) -> Result<Option<Binder>, Exception> {
        let package = req
            .package
            .ok_or_else(|| Exception::illegal_argument("packageName is null"))?;
        let intents: Vec<RawIntent> = match req.intents {
            Some(list) => {
                if list.is_empty() {
                    return Err(Exception::illegal_argument(
                        "Intents array length must be >= 1",
                    ));
                }
                list.into_iter()
                    .map(|i| i.ok_or_else(|| Exception::illegal_argument("null intent")))
                    .collect::<Result<_, _>>()?
            }
            None => Vec::new(),
        };
        let target_sdk = |p: &str| -> Result<i32, Exception> {
            let (state, _) = self.packages()?;
            Ok(state.packages.get(p).map_or(0, |p| p.target_sdk_version))
        };
        for intent in &intents {
            if intent.has_fds() {
                return Err(Exception::illegal_argument(
                    "File descriptors passed in Intent",
                ));
            }
            if req.kind == INTENT_SENDER_BROADCAST
                && intent.intent.flags & FLAG_RECEIVER_BOOT_UPGRADE != 0
            {
                return Err(Exception::illegal_argument(
                    "Can't use FLAG_RECEIVER_BOOT_UPGRADE here",
                ));
            }
            // `isNewMutableDisallowedImplicitPendingIntent`.
            let implicit = intent.intent.component.is_none() && intent.intent.package.is_none();
            if req.kind != INTENT_SENDER_ACTIVITY_RESULT
                && req.flags & FLAG_NO_CREATE == 0
                && req.flags & FLAG_MUTABLE != 0
                && implicit
                && req.flags & FLAG_ALLOW_UNSAFE_IMPLICIT_INTENT == 0
                && self
                    .inputs()?
                    .compat
                    .is_enabled(BLOCK_MUTABLE_IMPLICIT_PENDING_INTENT, Some(&package), target_sdk(&package)?)
            {
                return Err(Exception::illegal_argument(format!(
                    "{package}: Targeting U+ (version 34 and above) disallows creating or retrieving a PendingIntent with FLAG_MUTABLE, an implicit Intent within and without FLAG_NO_CREATE and FLAG_ALLOW_UNSAFE_IMPLICIT_INTENT for security reasons. To retrieve an already existing PendingIntent, use FLAG_NO_CREATE, however, to create a new PendingIntent with an implicit Intent use FLAG_IMMUTABLE."
                )));
            }
        }
        if req
            .resolved_types
            .as_ref()
            .is_some_and(|t| t.len() != intents.len())
        {
            return Err(Exception::illegal_argument(
                "Intent array length does not match resolvedTypes length",
            ));
        }
        if req.options.as_ref().is_some_and(RawBundle::has_fds) {
            return Err(Exception::illegal_argument(
                "File descriptors passed in options",
            ));
        }
        let user = if req.user == USER_CURRENT {
            self.incoming_user(uid, 0, true)?;
            USER_CURRENT
        } else {
            self.incoming_user(uid, req.user, true)?
        };
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        if app_id != 0 && app_id != 1000 && !self.owns_package(uid, &package)? {
            return Err(Exception::security(format!(
                "Permission Denial: getIntentSender() from uid={uid} is not allowed to send as package {package}"
            )));
        }
        if req.kind == INTENT_SENDER_ACTIVITY_RESULT {
            return Err(self
                .unimplemented_named("activity", "a PendingIntent of an activity result (#1493)"));
        }
        let no_create = req.flags & FLAG_NO_CREATE != 0;
        let cancel_current = req.flags & FLAG_CANCEL_CURRENT != 0;
        let update_current = req.flags & FLAG_UPDATE_CURRENT != 0;
        let flags = req.flags & !(FLAG_NO_CREATE | FLAG_CANCEL_CURRENT | FLAG_UPDATE_CURRENT);
        let mut key = Record {
            binder: Binder::Local(0),
            kind: req.kind,
            package,
            feature: req.feature,
            activity: req.token,
            result_who: req.result_who,
            request_code: req.request_code,
            intents: intents
                .into_iter()
                .map(|i| KeptIntent::new(i, &self.process))
                .collect(),
            resolved_types: req.resolved_types,
            flags,
            user,
            uid,
            canceled: false,
            cancel_listeners: Vec::new(),
        };
        let mut records = self.pending_intents.records.lock().unwrap();
        if let Some(at) = records.iter().position(|r| !r.canceled && r.matches(&key)) {
            if !cancel_current {
                let rec = &mut records[at];
                if update_current && !key.intents.is_empty() {
                    // `replaceExtras` of the request intent: the new intents
                    // are kept, as `allIntents` are.
                    rec.intents = key.intents;
                    rec.resolved_types = key.resolved_types;
                }
                return Ok(Some(rec.binder));
            }
            let rec = records.remove(at);
            cancel(rec);
        }
        if no_create {
            return Ok(None);
        }
        let node = Arc::new(IntentSender {
            host: self.me.clone(),
            me: OnceLock::new(),
        });
        key.binder = self.process.add_service(node.clone());
        let _ = node.me.set(key.binder);
        let binder = key.binder;
        records.push(key);
        Ok(Some(binder))
    }

    /// `cancelIntentSender`: only its creator's app may cancel it.
    pub(super) fn cancel_intent_sender(
        &self,
        uid: i32,
        target: Option<Binder>,
    ) -> Result<(), Exception> {
        let mut records = self.pending_intents.records.lock().unwrap();
        let Some(at) = records.iter().position(|r| Some(r.binder) == target) else {
            return Ok(());
        };
        if !self.owns_package(uid, &records[at].package)?
            && uid.rem_euclid(PER_USER_RANGE as i32) != 1000
        {
            return Err(Exception::security(format!(
                "Permission Denial: cancelIntentSender() from uid={uid} is not allowed to cancel package {}",
                records[at].package
            )));
        }
        cancel(records.remove(at));
        Ok(())
    }

    /// The uid that made the PendingIntent `target`, if it is the app
    /// host's (`PendingIntentRecord.uid`).
    pub(super) fn intent_sender_uid(&self, target: Binder) -> Option<i32> {
        self.pending_intents.records.lock().unwrap().iter().find(|r| r.binder == target).map(|r| r.uid)
    }

    /// `getInfoForIntentSender`.
    pub(super) fn intent_sender_info(&self, target: Option<Binder>) -> impl WriteParcelable {
        let records = self.pending_intents.records.lock().unwrap();
        match records.iter().find(|r| Some(r.binder) == target) {
            Some(r) => Info {
                package: Some(r.package.clone()),
                uid: r.uid,
                immutable: r.flags & FLAG_IMMUTABLE != 0,
                kind: r.kind,
            },
            None => Info {
                package: None,
                uid: INVALID_UID,
                immutable: false,
                kind: INTENT_SENDER_UNKNOWN,
            },
        }
    }

    /// `registerIntentSenderCancelListenerEx`: false if it is cancelled.
    pub(super) fn listen_for_cancel(
        &self,
        target: Option<Binder>,
        receiver: Option<Binder>,
    ) -> bool {
        let mut records = self.pending_intents.records.lock().unwrap();
        let Some(r) = records.iter_mut().find(|r| Some(r.binder) == target) else {
            // Cancelled records are gone.
            return false;
        };
        if let Some(Binder::Handle(h)) = receiver {
            r.cancel_listeners.push(self.process.strong(h));
        }
        true
    }

    /// `unregisterIntentSenderCancelListener`.
    pub(super) fn unlisten_for_cancel(&self, target: Option<Binder>, receiver: Option<Binder>) {
        let mut records = self.pending_intents.records.lock().unwrap();
        if let (Some(r), Some(Binder::Handle(h))) = (
            records.iter_mut().find(|r| Some(r.binder) == target),
            receiver,
        ) {
            r.cancel_listeners.retain(|l| l.handle != h);
        }
    }

    /// `getIntentForIntentSender`, for holders of GET_INTENT_SENDER_INTENT.
    pub(super) fn intent_for_intent_sender(
        &self,
        uid: i32,
        target: Option<Binder>,
    ) -> Result<Option<RawIntent>, Exception> {
        if self.check_permission(Some("android.permission.GET_INTENT_SENDER_INTENT"), uid)? != 0 {
            return Err(Exception::security(
                "getIntentForIntentSender() requires android.permission.GET_INTENT_SENDER_INTENT",
            ));
        }
        let records = self.pending_intents.records.lock().unwrap();
        Ok(records
            .iter()
            .find(|r| Some(r.binder) == target)
            .and_then(|r| r.intents.last())
            .map(|i| i.intent.clone()))
    }

    /// `sendIntentSender` (and `IIntentSender.send`): a record of this
    /// host is sent as `PendingIntentRecord.sendInner` sends it; another
    /// `IIntentSender` is sent on and its finished receiver told.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_intent_sender(
        self: &Arc<Self>,
        calling_uid: i32,
        caller: Option<Binder>,
        target: Option<Binder>,
        code: i32,
        intent: Option<RawIntent>,
        resolved_type: Option<String>,
        finished: Option<Binder>,
        required_permission: Option<String>,
        options: Option<RawBundle>,
    ) -> Result<i32, Exception> {
        if intent.as_ref().is_some_and(RawIntent::has_fds) {
            return Err(Exception::illegal_argument(
                "File descriptors passed in Intent",
            ));
        }
        let Some(target) = target else {
            return Err(Exception::illegal_argument("target is null"));
        };
        let finished = match finished {
            Some(Binder::Handle(h)) => Some(self.process.strong(h)),
            _ => None,
        };
        let Binder::Handle(other) = target else {
            return self.send_record(
                caller,
                target,
                code,
                intent,
                resolved_type,
                finished,
                required_permission,
            );
        };
        // Another implementation of IIntentSender: sent on, its result
        // reported here.
        let intent = intent.unwrap_or_else(|| RawIntent::new("android.intent.action.MAIN", 0, &[]));
        let mut p = Parcel::new();
        sender::Send {
            code,
            intent: Some(By(&intent)),
            resolved_type,
            whitelist_token: None,
            finished_receiver: None,
            required_permission,
            options: options.as_ref().map(By),
        }
        .write(&mut p);
        let _ = self.process.strong(other).transact(sender::SEND, &p, true);
        if let Some(f) = &finished {
            perform_receive(f, &intent, calling_uid / PER_USER_RANGE as i32);
        }
        Ok(0)
    }

    /// `PendingIntentRecord.sendInner`.
    #[allow(clippy::too_many_arguments)]
    fn send_record(
        self: &Arc<Self>,
        caller: Option<Binder>,
        target: Binder,
        code: i32,
        intent: Option<RawIntent>,
        resolved_type: Option<String>,
        finished: Option<Strong>,
        required_permission: Option<String>,
    ) -> Result<i32, Exception> {
        let (kind, package, feature, uid, user, final_intent, resolved) = {
            let mut records = self.pending_intents.records.lock().unwrap();
            let Some(at) = records.iter().position(|r| r.binder == target) else {
                return Ok(START_CANCELED);
            };
            let r = &records[at];
            let request = r
                .intents
                .last()
                .map(|k| k.intent.clone())
                .unwrap_or_else(|| RawIntent::of(None, 0, &[]));
            let mut resolved = r
                .resolved_types
                .as_ref()
                .and_then(|t| t.last().cloned().flatten());
            let mut final_intent = request;
            if r.flags & FLAG_IMMUTABLE == 0
                && let Some(fill) = &intent
            {
                let (filled, changes) = final_intent.fill_in(fill, r.flags).map_err(|_| {
                    self.unimplemented_named("activity", "a fill-in intent this host cannot merge (#1493)")
                })?;
                final_intent = filled;
                if changes & super::kept::FILL_IN_DATA != 0 {
                    resolved = resolved_type;
                }
            }
            let (kind, package, feature, uid, user) =
                (r.kind, r.package.clone(), r.feature.clone(), r.uid, r.user);
            if r.flags & FLAG_ONE_SHOT != 0 {
                cancel(records.remove(at));
            }
            (kind, package, feature, uid, user, final_intent, resolved)
        };
        let user = if user == USER_CURRENT { 0 } else { user };
        // The finished receiver's process gets its result.
        let caller_app = self.app_of_thread(caller);
        let mut res = START_SUCCESS;
        let mut send_finish = finished.is_some();
        match kind {
            INTENT_SENDER_ACTIVITY => {
                let args = StartArgs {
                    caller: None,
                    intent: Some(final_intent.clone()),
                    resolved_type: resolved,
                    result_to: None,
                    result_who: None,
                    request_code: -1,
                };
                res = match self.start_activity_in_package(uid, package.clone(), 0, args) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!(
                            "app host: a PendingIntent's activity of {package}: {}",
                            e.message
                        );
                        START_CANCELED
                    }
                };
            }
            INTENT_SENDER_BROADCAST => {
                let args = am::BroadcastIntentWithFeature {
                    caller: None,
                    calling_feature_id: feature,
                    intent: Some(final_intent.clone()),
                    resolved_type: resolved,
                    result_to: finished.as_ref().map(|f| Binder::Handle(f.handle)),
                    result_code: code,
                    result_data: None,
                    map: None,
                    required_permissions: required_permission.map(|p| vec![Some(p)]),
                    exclude_permissions: None,
                    exclude_packages: None,
                    app_op: -1,
                    options: None,
                    serialized: finished.is_some(),
                    sticky: false,
                    user_id: user,
                };
                match self.broadcasts.broadcast_as(
                    self,
                    uid,
                    0,
                    caller_app.as_ref().map(|c| c.0),
                    Some(package.clone()),
                    args,
                ) {
                    Ok(_) => send_finish = false,
                    Err(e) => eprintln!(
                        "app host: a PendingIntent's broadcast of {package}: {}",
                        e.message
                    ),
                }
            }
            INTENT_SENDER_SERVICE | INTENT_SENDER_FOREGROUND_SERVICE => {
                let foreground = kind == INTENT_SENDER_FOREGROUND_SERVICE;
                if let Err(e) = self.start_service(None, uid, final_intent.clone(), resolved, foreground, Some(&package), user) {
                    eprintln!(
                        "app host: a PendingIntent's service of {package}: {}",
                        e.message
                    );
                }
            }
            _ => {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("unknown PendingIntent type {kind}"),
                ));
            }
        }
        if let (true, Some(f)) = (send_finish && res != START_CANCELED, &finished) {
            perform_receive(f, &final_intent, user);
        }
        Ok(res)
    }
}

/// `makeIntentSenderCanceled`: its cancel listeners are told.
fn cancel(r: Record) {
    for l in &r.cancel_listeners {
        let mut p = Parcel::new();
        p.write_interface_token(RESULT_RECEIVER_DESCRIPTOR);
        p.write_i32(RESULT_CANCELED);
        p.write_i32(0); // no result data
        let _ = l.transact(RESULT_RECEIVER_SEND, &p, true);
    }
}

/// `IIntentReceiver.performReceive` of a send's finished receiver.
fn perform_receive(receiver: &Strong, intent: &RawIntent, user: i32) {
    let mut p = Parcel::new();
    intent_receiver::PerformReceive {
        intent: Some(By(intent)),
        result_code: 0,
        data: None,
        extras: None::<By<'_, RawBundle>>,
        ordered: false,
        sticky: false,
        sending_user: user,
    }
    .write(&mut p);
    let _ = receiver.transact(intent_receiver::PERFORM_RECEIVE, &p, true);
}
