//! An app's own starts and finishes of activities, as ActivityTaskManager
//! takes them at `android-16.0.0_r1`: `startActivity` (`ActivityStarter`:
//! the intent resolved, `checkStartAnyActivityPermission`, the task it goes
//! into and the reuse of an activity there as [`super::activity_stack`]
//! decides them, the activity put on top of its task, in its app's
//! process, started for it if need be), `finishActivity` with its result,
//! the URI grants an intent or a result carries held by the activity it
//! goes to ([`super::uri_grants`]), and what the activities' reports set
//! moving, sent as client
//! transactions. Not yet: forwarded results, documents, a task's reset,
//! launch options, and the resolver for an intent of several activities
//! (#1485).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use aim_binder_host::local::Call;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iapplicationthread as thread;

use super::activity::{ActivityRecord, Item, Token};
use super::activity_stack::{self as stack, Command, Decision, Outcome, Then};
use super::kept::{KeptIntent, RawIntent};
use super::packages::WindowStyle;
use super::uri_grants::{NeededUriGrants, Owner};
use super::{AppHost, PER_USER_RANGE};
use crate::package::info::ActivityInfo;
use crate::package::intent::{ComponentName, Intent};

/// `ActivityManager.START_*`.
pub const START_SUCCESS: i32 = stack::START_SUCCESS;
const START_INTENT_NOT_RESOLVED: i32 = -1;
const START_CLASS_NOT_FOUND: i32 = -2;
/// `Intent.FLAG_ACTIVITY_FORWARD_RESULT`,
/// `EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN`.
const FLAG_ACTIVITY_FORWARD_RESULT: i32 = 0x0200_0000;
const EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN: i32 = 1 << 2;
/// `ActivityInfo.FLAG_EXCLUDE_FROM_RECENTS`, `FLAG_NO_HISTORY`.
pub const FLAG_EXCLUDE_FROM_RECENTS: i32 = 0x20;
const FLAG_NO_HISTORY: i32 = 0x80;
/// `Activity.RESULT_CANCELED`.
const RESULT_CANCELED: i32 = 0;
/// `ActivityTaskSupervisor.TOP_RESUMED_STATE_LOSS_TIMEOUT`.
const TOP_RESUMED_STATE_LOSS_TIMEOUT: Duration = Duration::from_millis(500);
/// `PackageManager.MATCH_DEFAULT_ONLY`.
const MATCH_DEFAULT_ONLY: i64 = 0x1_0000;
/// `ActivityManager.FINISH_TASK_WITH_ROOT_ACTIVITY`, `FINISH_TASK_WITH_ACTIVITY`.
const FINISH_TASK_WITH_ROOT_ACTIVITY: i32 = 1;
const FINISH_TASK_WITH_ACTIVITY: i32 = 2;
/// `IWindow.dispatchAppVisibility(boolean)`, in the interface's order (one-way).
const WINDOW_DESCRIPTOR: &str = "android.view.IWindow";
const DISPATCH_APP_VISIBILITY: u32 = 7;
/// The resolver the system shows for an intent of several activities.
const RESOLVER: &str = "com.android.internal.app.ResolverActivity";
/// `ActivityInfo.CONTENT_URI_PERMISSION_NONE`, and the flag that has an
/// activity's other values enforced.
const CONTENT_URI_PERMISSION_NONE: i32 = 0;
const CONTENT_URI_PERMISSION_APIS: &str = "android.security.content_uri_permission_apis";

/// The intent an activity launches with.
#[derive(Clone)]
pub enum LaunchIntent {
    /// The launcher's: its component, as the app host launches an app.
    Component,
    /// An app's, its component set to the activity's.
    Raw(Box<KeptIntent>),
}

impl LaunchIntent {
    /// The intent as a task keeps it, of `package`'s `class`.
    fn intent(&self, package: &str, class: &str) -> Intent {
        match self {
            LaunchIntent::Component => Intent {
                flags: stack::FLAG_ACTIVITY_NEW_TASK,
                component: Some(ComponentName { package: package.to_string(), class: class.to_string() }),
                ..Default::default()
            },
            LaunchIntent::Raw(kept) => kept.intent.intent.clone(),
        }
    }
}

/// What starts an activity, in a process yet to start.
pub struct StartRequest {
    pub class: String,
    pub intent: LaunchIntent,
    pub referrer: String,
    pub result_to: Option<u64>,
    pub result_who: Option<String>,
    pub request_code: i32,
    /// The task it goes on top of, else a new one.
    pub task: Option<i32>,
    /// The URI grants its intent carries, the activity's once it is made.
    pub(super) grants: Option<NeededUriGrants>,
}

impl StartRequest {
    /// The launcher's start of `class`.
    pub fn launcher(class: &str) -> StartRequest {
        StartRequest {
            class: class.to_string(),
            intent: LaunchIntent::Component,
            referrer: "com.android.shell".into(),
            result_to: None,
            result_who: None,
            request_code: -1,
            task: None,
            grants: None,
        }
    }

    /// Whether the user leaves the activity below it
    /// (`FLAG_ACTIVITY_NO_USER_ACTION` unset).
    fn user_leaving(&self) -> bool {
        match &self.intent {
            LaunchIntent::Component => true,
            LaunchIntent::Raw(kept) => kept.intent.intent.flags & stack::FLAG_ACTIVITY_NO_USER_ACTION == 0,
        }
    }
}

/// `mActivityComponent` of `info`: its class, or an alias's target.
pub fn real_component(info: &ActivityInfo) -> String {
    let package = info.info.item.package_name.as_deref().unwrap_or_default();
    let class = info.target_activity.as_deref().or(info.info.item.name.as_deref()).unwrap_or_default();
    format!("{package}/{class}")
}

/// A result or a new intent the stack holds for an activity.
pub enum Payload {
    /// `ResultInfo`: who asked, its request code, the result.
    Result { who: Option<String>, request_code: i32, result_code: i32, data: Option<KeptIntent> },
    /// `ReferrerIntent`.
    NewIntent { intent: KeptIntent, referrer: String },
}

impl WriteParcelable for Payload {
    fn write_to(&self, p: &mut Parcel) {
        match self {
            Payload::Result { who, request_code, result_code, data } => {
                p.write_string16(who.as_deref());
                p.write_i32(*request_code);
                p.write_i32(*result_code);
                match data {
                    Some(d) => {
                        p.write_i32(1);
                        d.intent.write_to(p);
                    }
                    None => p.write_i32(0),
                }
                p.write_binder(None); // no caller token
            }
            Payload::NewIntent { intent, referrer } => {
                intent.intent.write_to(p);
                p.write_string16(Some(referrer));
                p.write_binder(None); // no caller token
            }
        }
    }
}

struct PauseActivityItem {
    token: Binder,
    finishing: bool,
    user_leaving: bool,
}

impl Item for PauseActivityItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.PauseActivityItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        p.write_bool(self.finishing);
        p.write_bool(self.user_leaving);
        p.write_bool(false); // reported
        p.write_bool(false); // not entering PiP
    }
}

struct StartActivityItem(Binder);

impl Item for StartActivityItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.StartActivityItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.0));
        p.write_i32(0); // no scene transition
    }
}

struct StopActivityItem(Binder);

impl Item for StopActivityItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.StopActivityItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.0));
    }
}

struct DestroyActivityItem(Binder);

impl Item for DestroyActivityItem {
    fn class(&self) -> &'static str {
        "android.app.servertransaction.DestroyActivityItem"
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.0));
        p.write_bool(true); // finished
    }
}

/// `ActivityResultItem` or `NewIntentItem`: a typed list of payloads.
struct PayloadItem {
    token: Binder,
    class: &'static str,
    /// `NewIntentItem`'s: whether the activity is resumed.
    resume: Option<bool>,
    payloads: Vec<u64>,
    host: Arc<AppHost>,
}

impl Item for PayloadItem {
    fn class(&self) -> &'static str {
        self.class
    }

    fn write_fields(&self, p: &mut Parcel) {
        p.write_binder(Some(self.token));
        if let Some(resume) = self.resume {
            p.write_bool(resume);
        }
        let payloads = self.host.activities.payloads.lock().unwrap();
        let list: Vec<&Payload> = self.payloads.iter().filter_map(|id| payloads.get(id)).collect();
        p.write_i32(list.len() as i32);
        for payload in list {
            p.write_i32(1);
            payload.write_to(p);
        }
    }
}

impl AppHost {
    fn new_payload(&self, payload: Payload) -> u64 {
        let id = self.activities.next_payload.fetch_add(1, Ordering::Relaxed);
        self.activities.payloads.lock().unwrap().insert(id, payload);
        id
    }

    /// A new activity of `start_seq`'s process in `task`, of `style`: its
    /// token.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_activity(
        &self,
        start_seq: i64,
        uid: u32,
        package: &str,
        info: ActivityInfo,
        style: WindowStyle,
        task: i32,
        mut request: StartRequest,
    ) -> u64 {
        let Binder::Local(token) = self.process.add_service(Arc::new(Token)) else { unreachable!("a local binder") };
        let grants = request.grants.take();
        let record = ActivityRecord {
            package: package.to_string(),
            class: request.class,
            uid,
            start_seq,
            started: std::time::Instant::now(),
            resumed: false,
            fully_drawn: false,
            requested_orientation: -1,
            config_changes: ActivityRecord::real_config_changes(&info),
            size_buckets: None,
            reported: None,
            info,
            style,
            intent: request.intent,
            referrer: request.referrer,
            task,
            result_to: request.result_to,
            result_who: request.result_who,
            request_code: request.request_code,
        };
        self.activities.records.lock().unwrap().insert(token, record);
        // `startActivityInner`: the intent's grants are the activity's.
        if let Err(e) = self.grant_from_intent(grants.as_ref(), Some(Owner::Activity(token))) {
            eprintln!("app host: activity {token:#x}: URI grants: {}", e.message);
        }
        token
    }

    /// Sends what the stack decided, each as a transaction to its
    /// activity's process, in order.
    pub(super) fn run(self: &Arc<Self>, commands: Vec<Command>) {
        for command in commands {
            if let Err(e) = self.run_one(command) {
                eprintln!("app host: activity: {e}");
            }
        }
    }

    fn run_one(self: &Arc<Self>, command: Command) -> Result<(), String> {
        let token_of = |t: u64| Binder::Local(t);
        let start_of = |t: u64| self.activities.records.lock().unwrap().get(&t).map(|r| (r.start_seq, r.uid));
        let send = |t: u64, items: Vec<Box<dyn Item>>| -> Result<(), String> {
            let (start_seq, _) = start_of(t).ok_or_else(|| format!("no activity {t:#x}"))?;
            self.call_thread(start_seq, thread::SCHEDULE_TRANSACTION, &super::activity::transaction(items))
        };
        match command {
            Command::Launch(t) => {
                let (start_seq, uid) = start_of(t).ok_or("no activity to launch")?;
                let global = self.global_config(uid).ok_or("launch before bind")?;
                let task = self.activities.records.lock().unwrap().get(&t).map(|r| r.task);
                let bounds = task.and_then(|id| self.tasks.freeform_bounds(id));
                let items = self
                    .launch_items(token_of(t), super::activity::Launch { global: &global, task: bounds })
                    .ok_or("no activity to launch")?;
                self.call_thread(start_seq, thread::SCHEDULE_TRANSACTION, &super::activity::transaction(items))
            }
            Command::Pause { token, finishing, user_leaving } => {
                send(token, vec![Box::new(PauseActivityItem { token: token_of(token), finishing, user_leaving })])
            }
            Command::Resume { token, results, new_intents } => {
                let mut items: Vec<Box<dyn Item>> = Vec::new();
                if !results.is_empty() {
                    items.push(Box::new(PayloadItem {
                        token: token_of(token),
                        class: "android.app.servertransaction.ActivityResultItem",
                        resume: None,
                        payloads: results.clone(),
                        host: self.clone(),
                    }));
                }
                if !new_intents.is_empty() {
                    items.push(Box::new(PayloadItem {
                        token: token_of(token),
                        class: "android.app.servertransaction.NewIntentItem",
                        resume: Some(false),
                        payloads: new_intents.clone(),
                        host: self.clone(),
                    }));
                }
                items.push(super::activity::resume_item(token_of(token)));
                let sent = send(token, items);
                self.forget_payloads(results.iter().chain(&new_intents));
                sent
            }
            Command::Result { token, results } => {
                let sent = send(
                    token,
                    vec![Box::new(PayloadItem {
                        token: token_of(token),
                        class: "android.app.servertransaction.ActivityResultItem",
                        resume: None,
                        payloads: results.clone(),
                        host: self.clone(),
                    })],
                );
                self.forget_payloads(results.iter());
                sent
            }
            Command::NewIntent { token, intents } => {
                let sent = send(
                    token,
                    vec![Box::new(PayloadItem {
                        token: token_of(token),
                        class: "android.app.servertransaction.NewIntentItem",
                        resume: Some(true),
                        payloads: intents.clone(),
                        host: self.clone(),
                    })],
                );
                self.forget_payloads(intents.iter());
                sent
            }
            Command::Start(t) => send(t, vec![Box::new(StartActivityItem(token_of(t)))]),
            Command::Stop(t) => send(t, vec![Box::new(StopActivityItem(token_of(t)))]),
            Command::Destroy(t) => send(t, vec![Box::new(DestroyActivityItem(token_of(t)))]),
            // The top resumed activity's latest window takes the input
            // focus.
            Command::TopResumed { token, on_top } => {
                send(token, vec![super::activity::top_resumed_item(token_of(token), on_top)])?;
                match self.windows_of_activity(token).last() {
                    Some((_, channel)) if on_top => self.focus_input_channel(*channel).map_err(|e| format!("{e:?}")),
                    _ => Ok(()),
                }
            }
            Command::LossTimeout(loss) => {
                let host = self.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(TOP_RESUMED_STATE_LOSS_TIMEOUT);
                    let commands = host.activities.stack.lock().unwrap().top_resumed_released(Some(loss));
                    host.run(commands);
                });
                Ok(())
            }
            Command::Visible { token, visible } => {
                let mut p = Parcel::new();
                p.write_interface_token(WINDOW_DESCRIPTOR);
                p.write_bool(visible);
                for (window, _) in self.windows_of_activity(token) {
                    self.process.transact(window, DISPATCH_APP_VISIBILITY, &p, true).map_err(|s| format!("dispatchAppVisibility: {s}"))?;
                }
                Ok(())
            }
            Command::Front(task) => self.task_to_front(task),
            Command::TaskEmpty(task) => {
                self.remove_task(task);
                Ok(())
            }
        }
    }

    fn forget_payloads<'a>(&self, ids: impl Iterator<Item = &'a u64>) {
        let mut payloads = self.activities.payloads.lock().unwrap();
        for id in ids {
            payloads.remove(id);
        }
    }

    /// `startActivityInner`'s new activity for `request`, `info` of
    /// `package`, in the attached process of `start_seq` of `uid`: on top
    /// of the task `d` placed it in if that is still there, else of a new
    /// one, and the focused tasks' tops resumed.
    pub(super) fn start_in_process(self: &Arc<Self>, start_seq: i64, uid: u32, package: &str, info: ActivityInfo, request: StartRequest, d: &Decision) -> Result<(), Exception> {
        let real = real_component(&info);
        let user = uid / PER_USER_RANGE;
        let user_leaving = request.user_leaving();
        let intent = request.intent.intent(package, &request.class);
        let base = request.intent.clone();
        let affinity = info.task_affinity.clone();
        let existing = request.task.filter(|&t| self.activities.stack.lock().unwrap().task(t).is_some());
        let (task, new) = match existing {
            Some(task) => (task, None),
            None => {
                let id = self.new_task(uid, package, &request.class, super::freeform::Activity::from(&info))?;
                let task = stack::Task {
                    id,
                    affinity: affinity.clone(),
                    real_activity: real.clone(),
                    intent: intent.clone(),
                    base: base.clone(),
                    uid,
                    user,
                    last_active: 0,
                };
                (id, Some(Box::new(task)))
            }
        };
        let style = self.window_style(&info);
        let no_history = intent.flags & stack::FLAG_ACTIVITY_NO_HISTORY != 0 || info.flags & FLAG_NO_HISTORY != 0;
        let (launch_mode, result_to) = (info.launch_mode, request.result_to);
        // Its result, `RESULT_CANCELED` until it sets one.
        let result = result_to.map(|_| self.new_payload(Payload::Result { who: request.result_who.clone(), request_code: request.request_code, result_code: RESULT_CANCELED, data: None }));
        let token = self.new_activity(start_seq, uid, package, info, style, task, request);
        let mut activity = stack::Activity::new(token, task, start_seq, &real, launch_mode, user, result_to);
        (activity.result, activity.occludes_parent, activity.no_history) = (result, style.occludes_parent(), no_history);
        let identity = stack::Identity { component: &real, intent: &intent, base: &base, affinity: affinity.as_deref(), uid };
        let commands = self.activities.stack.lock().unwrap().apply(d, Then::Add(activity, new), Some(identity), user_leaving);
        self.run(commands);
        Ok(())
    }

    /// The launcher's start of `package`'s `class` (`info`) in a new task:
    /// in its process if that runs, else in the process started for it.
    pub(super) fn start_launcher(self: &Arc<Self>, package: &str, class: &str, info: ActivityInfo) -> Result<(), Exception> {
        let process = super::providers::process_name(&info.info);
        let uid = info.info.application_info.uid as u32;
        match self.running_process(&process, uid).filter(|(_, attached)| *attached) {
            Some((start_seq, _)) => self.start_in_process(start_seq, uid, package, info, StartRequest::launcher(class), &Decision::default()),
            None => self.start_process_for(package, &process, Some(StartRequest::launcher(class))).map(|_| ()),
        }
    }

    /// `IActivityTaskManager.startActivity`: the start's result code.
    pub(super) fn start_activity(self: &Arc<Self>, call: &Call<'_>, args: StartArgs) -> Result<i32, Exception> {
        let Some((_, caller_package, _, caller_pid)) = self.app_of_thread(args.caller) else {
            return Err(Exception::security("Unable to find app for caller"));
        };
        self.start_activity_in_package(call.sender_euid as i32, caller_package, caller_pid, args)
    }

    /// `ActivityStarter`'s start of `args` by `uid` of `caller_package`
    /// (a PendingIntent's creator for one it sends,
    /// `startActivityInPackage`): the start's result code.
    pub(super) fn start_activity_in_package(self: &Arc<Self>, uid: i32, caller_package: String, caller_pid: i32, args: StartArgs) -> Result<i32, Exception> {
        let mut intent = args.intent.ok_or_else(|| Exception::illegal_argument("intent"))?;
        if intent.has_fds() {
            return Err(Exception::illegal_argument("File descriptors passed in Intent"));
        }
        // `ActivityTaskSupervisor.resolveIntent`.
        let (_, resolution) = self.packages()?;
        let found = resolution
            .resolve_intent(&intent.intent, args.resolved_type.as_deref(), super::STOCK_PM_FLAGS | MATCH_DEFAULT_ONLY, uid / PER_USER_RANGE as i32, uid)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("resolveIntent: {e:?}")))?;
        let info = match found.map(|r| r.info) {
            Some(crate::package::component_resolver::Info::Activity(info)) => info,
            _ => return Ok(if intent.intent.component.is_some() { START_CLASS_NOT_FOUND } else { START_INTENT_NOT_RESOLVED }),
        };
        if info.info.item.name.as_deref() == Some(RESOLVER) {
            return Err(self.unimplemented_named("activity_task", "the resolver for an intent of several activities (#1485)"));
        }
        let target_uid = info.info.application_info.uid;
        let package = info.info.application_info.item.package_name.clone().unwrap_or_default();
        let class = info.info.item.name.clone().unwrap_or_default();
        // `ActivityStarter.Request.resolveActivity`: the intent's grants,
        // checked for the caller.
        if info.require_content_uri_permission_from_caller != CONTENT_URI_PERMISSION_NONE && self.flag(CONTENT_URI_PERMISSION_APIS)? {
            return Err(self.unimplemented_named("activity_task", "requireContentUriPermissionFromCaller (#1568)"));
        }
        let grants = self.check_grant_from_intent(&intent, uid, &package, target_uid / PER_USER_RANGE as i32)?;
        // `checkStartAnyActivityPermission`.
        if !self.check_component_permission(info.permission.as_deref(), uid, target_uid, info.info.exported)? {
            let what = format!("Permission Denial: starting Intent {{ cmp={package}/{class} }} from {caller_package} (pid={caller_pid}, uid={uid})");
            return Err(Exception::security(if info.info.exported {
                format!("{what} requires {}", info.permission.as_deref().unwrap_or("null"))
            } else {
                format!("{what} not exported from uid {target_uid}")
            }));
        }
        if intent.intent.flags & FLAG_ACTIVITY_FORWARD_RESULT != 0 {
            return Err(self.unimplemented_named("activity_task", "a forwarded result (#1485)"));
        }
        // `sourceRecord`, the caller's activity, and `resultRecord`, the
        // source when it asks for a result.
        let (source, result_to) = {
            let stack = self.activities.stack.lock().unwrap();
            let source = match args.result_to {
                Some(Binder::Local(t)) => stack.get(t),
                _ => None,
            };
            (source.map(|a| a.token), source.filter(|a| args.request_code >= 0 && !a.finishing).map(|a| a.token))
        };
        intent = intent.with_component(&package, &class);
        let real = real_component(&info);
        let target_uid = target_uid as u32;
        let start = stack::Start {
            component: &real,
            affinity: info.task_affinity.as_deref(),
            launch_mode: info.launch_mode,
            document_launch_mode: info.document_launch_mode,
            intent: &intent.intent,
            user: target_uid / PER_USER_RANGE,
            source,
            result_to,
        };
        let d = self.activities.stack.lock().unwrap().decide(&start).map_err(|what| self.unimplemented_named("activity_task", &format!("{what} (#1485)")))?;
        // `ActivityRecord`'s intent: the launch flags, and its exclusion
        // from recents.
        let exclude = if info.flags & FLAG_EXCLUDE_FROM_RECENTS != 0 { stack::FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS } else { 0 };
        let extended = intent.intent.extended_flags | EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN;
        intent.set_flags(d.flags | exclude, extended);
        let kept = KeptIntent::new(intent, &self.process);
        let base = kept.intent.intent.clone();
        let launch = LaunchIntent::Raw(Box::new(kept.clone()));
        let identity = || stack::Identity { component: &real, intent: &base, base: &launch, affinity: info.task_affinity.as_deref(), uid: target_uid };
        let user_leaving = d.flags & stack::FLAG_ACTIVITY_NO_USER_ACTION == 0;
        let mut commands = Vec::new();
        if d.cancel_result
            && let Some(to) = result_to
        {
            let payload = self.new_payload(Payload::Result { who: args.result_who.clone(), request_code: args.request_code, result_code: RESULT_CANCELED, data: None });
            commands.extend(self.activities.stack.lock().unwrap().send_result(to, payload));
        }
        let task = match d.outcome {
            Outcome::Deliver(token, code) => {
                // `deliverNewIntentLocked`: the activity gets the intent's
                // grants.
                self.grant_from_intent(grants.as_ref(), Some(Owner::Activity(token)))?;
                let payload = self.new_payload(Payload::NewIntent { intent: kept, referrer: caller_package });
                commands.extend(self.activities.stack.lock().unwrap().apply(&d, Then::Deliver(token, payload), Some(identity()), user_leaving));
                self.run(commands);
                return Ok(code);
            }
            Outcome::Front(code) => {
                commands.extend(self.activities.stack.lock().unwrap().apply(&d, Then::Nothing, Some(identity()), user_leaving));
                self.run(commands);
                return Ok(code);
            }
            Outcome::New(task) => task,
        };
        let request = StartRequest {
            class,
            intent: LaunchIntent::Raw(Box::new(kept)),
            referrer: caller_package,
            result_to: d.result_to,
            result_who: args.result_who,
            request_code: args.request_code,
            task,
            grants,
        };
        let process = super::providers::process_name(&info.info);
        match self.running_process(&process, target_uid).filter(|(_, attached)| *attached) {
            Some((target_seq, _)) => {
                self.run(commands);
                self.start_in_process(target_seq, target_uid, &package, info, request, &d)?;
            }
            None => {
                // What comes before the activity is done now; it is added
                // once its process attached (`startProcessLocked`).
                if d.front.is_some() || !d.clear.is_empty() || d.reorder.is_some() {
                    commands.extend(self.activities.stack.lock().unwrap().apply(&d, Then::Nothing, None, user_leaving));
                }
                self.run(commands);
                self.start_process_for(&package, &process, Some(request))?;
            }
        }
        Ok(START_SUCCESS)
    }

    /// `finishActivity` by `uid`: whether it finishes; its result to the
    /// activity that asked for it, with the URI grants of its data; its
    /// task too with `FINISH_TASK_WITH_*`.
    pub(super) fn finish_activity(self: &Arc<Self>, uid: i32, token: Option<Binder>, result_code: i32, data: Option<RawIntent>, finish_task: i32) -> Result<bool, Exception> {
        let Some(Binder::Local(token)) = token else { return Ok(true) };
        if data.as_ref().is_some_and(RawIntent::has_fds) {
            return Err(Exception::illegal_argument("File descriptors passed in Intent"));
        }
        let Some((who, request_code, result_to)) = self.activities.records.lock().unwrap().get(&token).map(|r| (r.result_who.clone(), r.request_code, r.result_to)) else {
            return Ok(true);
        };
        // `collectGrants`: what the data grants the activity that asked.
        let asker = result_to.and_then(|to| self.activities.records.lock().unwrap().get(&to).map(|r| (r.package.clone(), r.uid)));
        let grants = match (&data, asker) {
            (Some(data), Some((package, asker_uid))) => self.check_grant_from_intent(data, uid, &package, (asker_uid / PER_USER_RANGE) as i32)?,
            _ => None,
        };
        let (task, root, result) = {
            let stack = self.activities.stack.lock().unwrap();
            let a = stack.get(token);
            let task = a.map(|a| a.task);
            (task, task.and_then(|t| stack.root(t)).is_some_and(|r| r.token == token), a.and_then(|a| a.result))
        };
        // The result it sets, in place of its `RESULT_CANCELED`.
        if let Some(result) = result {
            let data = data.map(|d| KeptIntent::new(d, &self.process));
            self.activities.payloads.lock().unwrap().insert(result, Payload::Result { who, request_code, result_code, data });
        }
        let (finished, mut commands) = self.activities.stack.lock().unwrap().finish(token);
        // `finishActivityResults`: the grants are the asker's.
        if finished && let Some(to) = result_to {
            self.grant_from_intent(grants.as_ref(), Some(Owner::Activity(to)))?;
        }
        // The task goes with it: the rest of its activities finish.
        if let Some(task) = task
            && (finish_task == FINISH_TASK_WITH_ACTIVITY || (finish_task == FINISH_TASK_WITH_ROOT_ACTIVITY && root))
        {
            commands.extend(self.activities.stack.lock().unwrap().remove(task));
        }
        self.run(commands);
        Ok(finished)
    }

    /// `activityTopResumedStateLost`: the activity told it lost the top
    /// resumed state handled it.
    pub(super) fn top_resumed_lost(self: &Arc<Self>) {
        let commands = self.activities.stack.lock().unwrap().top_resumed_released(None);
        self.run(commands);
    }

    /// `convertFromTranslucent` (`occludes`), `convertToTranslucent`:
    /// whether `token`'s activity changed; the activities' visibility
    /// follows.
    pub(super) fn set_occludes_parent(self: &Arc<Self>, token: Option<Binder>, occludes: bool) -> bool {
        let Some(Binder::Local(token)) = token else { return false };
        let (changed, commands) = self.activities.stack.lock().unwrap().set_occludes_parent(token, occludes);
        self.run(commands);
        changed
    }

    /// `getTaskForActivity`: `token`'s task, if it is the task's root or
    /// `only_root` is not asked, else -1.
    pub(super) fn task_of_activity(&self, token: Option<Binder>, only_root: bool) -> i32 {
        let Some(Binder::Local(token)) = token else { return -1 };
        let stack = self.activities.stack.lock().unwrap();
        let Some(task) = stack.get(token).map(|a| a.task) else { return -1 };
        // Above the root (`getRootActivity`: the bottom one not finishing,
        // else the bottom one).
        let root = stack.root(task).or_else(|| stack.activities.iter().find(|a| a.task == task)).map(|a| a.token);
        let position = |t| stack.activities.iter().position(|a| Some(a.token) == t);
        if only_root && position(Some(token)) > position(root) {
            return -1;
        }
        task
    }

    /// `activityPaused`, `activityIdle`, `activityStopped`,
    /// `activityDestroyed`: the stack moves on.
    pub(super) fn activity_reported(self: &Arc<Self>, token: Option<Binder>, report: Report) {
        let Some(Binder::Local(token)) = token else { return };
        let destroyed = matches!(report, Report::Destroyed);
        let commands = {
            let mut stack = self.activities.stack.lock().unwrap();
            match report {
                Report::Paused => stack.paused(token),
                Report::Idle => stack.idle(token),
                Report::Stopped => {
                    stack.stopped(token);
                    Vec::new()
                }
                Report::Destroyed => {
                    let out = stack.destroyed(token);
                    self.activities.records.lock().unwrap().remove(&token);
                    // `removeUriPermissionsLocked`.
                    self.release_uri_grants(Owner::Activity(token));
                    out
                }
            }
        };
        if destroyed {
            self.activity_connections_gone(token);
        }
        self.run(commands);
    }

    /// Whether `token` is the root of a task the launcher started
    /// (`ActivityClientController.shouldMoveTaskToBack`).
    pub(super) fn launcher_root(&self, token: Option<Binder>) -> bool {
        let Some(Binder::Local(token)) = token else { return false };
        let stack = self.activities.stack.lock().unwrap();
        let root = stack.get(token).and_then(|a| stack.root(a.task)).map(|a| a.token);
        root == Some(token) && self.activities.records.lock().unwrap().get(&token).is_some_and(|r| matches!(r.intent, LaunchIntent::Component))
    }

    /// `getCallingActivity`, `getCallingPackage`: the activity that asked
    /// for `token`'s result.
    pub(super) fn calling_activity(&self, token: Option<Binder>) -> Option<(String, String)> {
        let Some(Binder::Local(token)) = token else { return None };
        let records = self.activities.records.lock().unwrap();
        let to = records.get(&token)?.result_to?;
        records.get(&to).map(|r| (r.package.clone(), r.class.clone()))
    }
}

/// An activity's report to the stack.
pub enum Report {
    Paused,
    Idle,
    Stopped,
    Destroyed,
}

/// `startActivity`'s arguments the app host reads.
pub struct StartArgs {
    pub caller: Option<Binder>,
    pub intent: Option<RawIntent>,
    pub resolved_type: Option<String>,
    pub result_to: Option<Binder>,
    pub result_who: Option<String>,
    pub request_code: i32,
}

/// A component name as `ComponentName.writeToParcel` writes it.
pub struct Component<'a>(pub &'a str, pub &'a str);

impl WriteParcelable for Component<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(self.0));
        p.write_string16(Some(self.1));
    }
}
