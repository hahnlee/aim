//! An app's own starts and finishes of activities, as ActivityTaskManager
//! takes them at `android-16.0.0_r1`: `startActivity` (`ActivityStarter`:
//! the intent resolved, `checkStartAnyActivityPermission`, the launch
//! mode's and flags' reuse of an activity in the task, the activity put on
//! top of its task or, of another app, started in that app's process),
//! `finishActivity` with its result, and what the activities' reports set
//! moving ([`super::activity_stack`]), sent as client transactions.
//! Not yet: activities in tasks of their own within a process
//! (`singleInstance`, documents, `FLAG_ACTIVITY_MULTIPLE_TASK`), forwarded
//! results, launch options, and the resolver for an intent of several
//! activities (#1485).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use aim_binder_host::local::Call;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iapplicationthread as thread;

use super::activity::{ActivityRecord, Item, Token};
use super::activity_stack::Command;
use super::kept::{KeptIntent, RawIntent};
use super::{AppHost, PER_USER_RANGE};
use crate::package::info::ActivityInfo;

/// `ActivityManager.START_*`.
pub const START_SUCCESS: i32 = 0;
const START_DELIVERED_TO_TOP: i32 = 3;
const START_INTENT_NOT_RESOLVED: i32 = -1;
const START_CLASS_NOT_FOUND: i32 = -2;
/// `Intent.FLAG_ACTIVITY_*`, `EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN`.
const FLAG_ACTIVITY_SINGLE_TOP: i32 = 0x2000_0000;
const FLAG_ACTIVITY_MULTIPLE_TASK: i32 = 0x0800_0000;
const FLAG_ACTIVITY_CLEAR_TOP: i32 = 0x0400_0000;
const FLAG_ACTIVITY_FORWARD_RESULT: i32 = 0x0200_0000;
const FLAG_ACTIVITY_NEW_DOCUMENT: i32 = 0x0008_0000;
const FLAG_ACTIVITY_NO_USER_ACTION: i32 = 0x0004_0000;
const EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN: i32 = 1 << 2;
/// `ActivityInfo.LAUNCH_*`.
const LAUNCH_SINGLE_TOP: i32 = 1;
const LAUNCH_SINGLE_TASK: i32 = 2;
const LAUNCH_SINGLE_INSTANCE: i32 = 3;
const LAUNCH_SINGLE_INSTANCE_PER_TASK: i32 = 4;
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

/// The intent an activity launches with.
#[derive(Clone)]
pub enum LaunchIntent {
    /// The launcher's: its component, as the app host launches an app.
    Component,
    /// An app's, its component set to the activity's.
    Raw(Box<KeptIntent>),
}

/// What starts an activity in a process yet to start.
pub struct StartRequest {
    pub class: String,
    pub intent: LaunchIntent,
    pub referrer: String,
    pub result_to: Option<u64>,
    pub result_who: Option<String>,
    pub request_code: i32,
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
        }
    }
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

    /// A new activity of `start_seq`'s process in `task`: its token.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_activity(
        &self,
        start_seq: i64,
        uid: u32,
        package: &str,
        info: ActivityInfo,
        task: i32,
        request: StartRequest,
    ) -> u64 {
        let Binder::Local(token) = self.process.add_service(Arc::new(Token)) else { unreachable!("a local binder") };
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
            intent: request.intent,
            referrer: request.referrer,
            task,
            result_to: request.result_to,
            result_who: request.result_who,
            request_code: request.request_code,
        };
        self.activities.records.lock().unwrap().insert(token, record);
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
                let bounds = task.and_then(|id| self.tasks.bounds(id));
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
            Command::Stop(t) => send(t, vec![Box::new(StopActivityItem(token_of(t)))]),
            Command::Destroy(t) => send(t, vec![Box::new(DestroyActivityItem(token_of(t)))]),
            Command::TopResumed { token, on_top } => send(token, vec![super::activity::top_resumed_item(token_of(token), on_top)]),
            Command::Visible { token, visible } => {
                let mut p = Parcel::new();
                p.write_interface_token(WINDOW_DESCRIPTOR);
                p.write_bool(visible);
                let windows = self.windows_of_activity(token);
                for (window, _) in &windows {
                    self.process.transact(*window, DISPATCH_APP_VISIBILITY, &p, true).map_err(|s| format!("dispatchAppVisibility: {s}"))?;
                }
                if visible && let Some((_, channel)) = windows.last() {
                    self.focus_input_channel(*channel).map_err(|e| format!("{e:?}"))?;
                }
                Ok(())
            }
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

    /// The task an activity of `start_seq`'s process starts into: that of
    /// `result_to`, else of the process's top activity, else a new one.
    fn task_for(&self, start_seq: i64, uid: u32, package: &str, info: &ActivityInfo, result_to: Option<u64>) -> i32 {
        let stack = self.activities.stack.lock().unwrap();
        let of = |t: u64| stack.get(t).filter(|a| a.start_seq == start_seq).map(|a| a.task);
        if let Some(task) = result_to.and_then(of) {
            return task;
        }
        if let Some(task) = stack.activities.iter().rev().find(|a| a.start_seq == start_seq && !a.finishing).map(|a| a.task) {
            return task;
        }
        drop(stack);
        let class = info.info.item.name.clone().unwrap_or_default();
        self.new_task(start_seq, uid, package, &class, super::freeform::Activity::from(info));
        self.tasks.of_uid(uid).map_or(-(start_seq as i32), |(id, _)| id)
    }

    /// `IActivityTaskManager.startActivity`: the start's result code.
    pub(super) fn start_activity(self: &Arc<Self>, call: &Call<'_>, args: StartArgs) -> Result<i32, Exception> {
        let Some((caller_seq, caller_package, _, caller_pid)) = self.app_of_thread(args.caller) else {
            return Err(Exception::security("Unable to find app for caller"));
        };
        self.start_activity_in_package(call.sender_euid as i32, caller_package, caller_pid, Some(caller_seq), args)
    }

    /// `ActivityStarter`'s start of `args` by `uid` of `caller_package`,
    /// from the process of `caller_seq` (none for a PendingIntent sent as
    /// its creator, `startActivityInPackage`): the start's result code.
    pub(super) fn start_activity_in_package(self: &Arc<Self>, uid: i32, caller_package: String, caller_pid: i32, caller_seq: Option<i64>, args: StartArgs) -> Result<i32, Exception> {
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
        // `checkStartAnyActivityPermission`.
        if !self.check_component_permission(info.permission.as_deref(), uid, target_uid, info.info.exported)? {
            let what = format!("Permission Denial: starting Intent {{ cmp={package}/{class} }} from {caller_package} (pid={caller_pid}, uid={uid})");
            return Err(Exception::security(if info.info.exported {
                format!("{what} requires {}", info.permission.as_deref().unwrap_or("null"))
            } else {
                format!("{what} not exported from uid {target_uid}")
            }));
        }
        let flags = intent.intent.flags;
        if flags & (FLAG_ACTIVITY_MULTIPLE_TASK | FLAG_ACTIVITY_NEW_DOCUMENT | FLAG_ACTIVITY_FORWARD_RESULT) != 0
            || matches!(info.launch_mode, LAUNCH_SINGLE_INSTANCE | LAUNCH_SINGLE_INSTANCE_PER_TASK)
        {
            return Err(self.unimplemented_named("activity_task", "an activity in a task of its own, or a forwarded result (#1485)"));
        }
        intent = intent.with_component(&package, &class);
        let extended = intent.intent.extended_flags | EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN;
        intent.set_flags(flags, extended);
        let kept = KeptIntent::new(intent, &self.process);
        let result_to = match args.result_to {
            Some(Binder::Local(t)) if args.request_code >= 0 => Some(t),
            _ => None,
        };
        let request = StartRequest {
            class: class.clone(),
            intent: LaunchIntent::Raw(Box::new(kept.clone())),
            referrer: caller_package.clone(),
            result_to,
            result_who: args.result_who,
            request_code: args.request_code,
        };
        let process = super::providers::process_name(&info.info);
        let target = self.running_process(&process, target_uid as u32).filter(|(_, attached)| *attached);
        let Some((target_seq, _)) = target else {
            // `startProcessLocked` for the activity.
            self.start_process_for(&package, &process, Some(request))?;
            return Ok(START_SUCCESS);
        };
        let target_uid = target_uid as u32;
        let task = self.task_for(target_seq, target_uid, &package, &info, result_to.filter(|_| Some(target_seq) == caller_seq));
        // The launch mode's and flags' reuse of an activity of the task.
        let reuse = {
            let stack = self.activities.stack.lock().unwrap();
            let records = self.activities.records.lock().unwrap();
            let same = |t: u64| records.get(&t).is_some_and(|r| r.class == class);
            let top = stack.top(task).map(|a| a.token).filter(|&t| same(t));
            let clear = flags & FLAG_ACTIVITY_CLEAR_TOP != 0 || info.launch_mode == LAUNCH_SINGLE_TASK;
            let existing = stack.activities.iter().rev().find(|a| a.task == task && !a.finishing && same(a.token)).map(|a| a.token);
            if clear && let Some(existing) = existing {
                // A standard activity under CLEAR_TOP is made anew.
                let again = info.launch_mode == 0 && flags & FLAG_ACTIVITY_SINGLE_TOP == 0 && info.launch_mode != LAUNCH_SINGLE_TASK;
                Some((existing, true, again))
            } else if (flags & FLAG_ACTIVITY_SINGLE_TOP != 0 || info.launch_mode == LAUNCH_SINGLE_TOP) && let Some(top) = top {
                Some((top, false, false))
            } else {
                None
            }
        };
        let mut commands = Vec::new();
        if let Some((existing, clear, again)) = reuse {
            if clear {
                commands.extend(self.activities.stack.lock().unwrap().clear_above(existing, again));
            }
            if !again {
                let payload = self.new_payload(Payload::NewIntent { intent: kept, referrer: caller_package });
                commands.extend(self.activities.stack.lock().unwrap().new_intent(existing, payload));
                self.run(commands);
                return Ok(START_DELIVERED_TO_TOP);
            }
        }
        let token = self.new_activity(target_seq, target_uid, &package, info, task, request);
        let mut started = self.activities.stack.lock().unwrap().start(token, task, target_seq, result_to);
        if flags & FLAG_ACTIVITY_NO_USER_ACTION != 0 {
            for c in &mut started {
                if let Command::Pause { user_leaving, .. } = c {
                    *user_leaving = false;
                }
            }
        }
        commands.extend(started);
        self.run(commands);
        Ok(START_SUCCESS)
    }

    /// `finishActivity`: whether it finishes; its result to the activity
    /// that asked for it; its task too with `FINISH_TASK_WITH_*`.
    pub(super) fn finish_activity(self: &Arc<Self>, token: Option<Binder>, result_code: i32, data: Option<RawIntent>, finish_task: i32) -> Result<bool, Exception> {
        let Some(Binder::Local(token)) = token else { return Ok(true) };
        if data.as_ref().is_some_and(RawIntent::has_fds) {
            return Err(Exception::illegal_argument("File descriptors passed in Intent"));
        }
        let Some((who, request_code, asked)) = self.activities.records.lock().unwrap().get(&token).map(|r| (r.result_who.clone(), r.request_code, r.result_to.is_some())) else {
            return Ok(true);
        };
        let result = asked.then(|| {
            let data = data.map(|d| KeptIntent::new(d, &self.process));
            self.new_payload(Payload::Result { who, request_code, result_code, data })
        });
        let task_root = {
            let stack = self.activities.stack.lock().unwrap();
            let task = stack.get(token).map(|a| a.task);
            let root = stack.activities.iter().find(|a| Some(a.task) == task && !a.finishing).map(|a| a.token);
            (task, root == Some(token))
        };
        let mut commands = Vec::new();
        let (finished, out) = self.activities.stack.lock().unwrap().finish(token, result);
        commands.extend(out);
        // The task goes with it: the rest of its activities finish.
        if finish_task == FINISH_TASK_WITH_ACTIVITY || (finish_task == FINISH_TASK_WITH_ROOT_ACTIVITY && task_root.1) {
            let rest: Vec<u64> = {
                let stack = self.activities.stack.lock().unwrap();
                stack.activities.iter().rev().filter(|a| Some(a.task) == task_root.0 && !a.finishing).map(|a| a.token).collect()
            };
            for t in rest {
                commands.extend(self.activities.stack.lock().unwrap().finish(t, None).1);
            }
        }
        self.run(commands);
        Ok(finished)
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
        let root = stack.get(token).and_then(|a| stack.activities.iter().find(|r| r.task == a.task && !r.finishing)).map(|a| a.token);
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
