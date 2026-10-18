//! The app host's tasks and their activities, as ActivityTaskManager keeps
//! them at `android-16.0.0_r1`: the task a start goes into
//! (`ActivityStarter.startActivityInner`: `computeLaunchingTaskFlags`,
//! `resolveReusableTask` with `RootWindowContainer.findTask` and
//! `findActivity`, `recycleTask`, `complyActivityFlags`,
//! `deliverToCurrentTopIfNeeded`), the tasks' order and focus, and the
//! activities' lifecycle (`RootWindowContainer
//! .resumeFocusedTasksTopActivities`, `TaskFragment.resumeTopActivity`,
//! `TaskDisplayArea.pauseBackTasks`, `ActivityRecord.finishIfPossible` and
//! `completeFinishing`, `ActivityTaskSupervisor.activityIdleInternal` and
//! `updateTopResumedActivityIfNeeded`). The focused task is the front one
//! with a running activity. A task's top activity is resumed once no
//! activity is pausing; the activities it hides are stopped once it is
//! idle, the finishing ones destroyed, a no-history one finished instead
//! of stopped (`stopIfPossible`). What hides what is the original's
//! (`TaskFragment.getVisibility`, `EnsureActivitiesVisibleHelper`): an
//! activity that occludes its parent (`ActivityRecord.occludesParent`: its
//! theme neither translucent nor floating) hides the activities under it,
//! a translucent one does not, and they stay visible, paused or started;
//! on a fullscreen display a task hides the tasks behind it unless none
//! of its activities occludes it, and those behind a translucent task
//! stay visible but paused; on a freeform one (window mode) every task
//! with a running activity is visible and its top resumed. The decisions
//! are [`Command`]s the app host sends as client transactions.

use super::activity_start::LaunchIntent;
use crate::package::intent::Intent;

/// `Intent.FLAG_ACTIVITY_*`.
pub const FLAG_ACTIVITY_NO_HISTORY: i32 = 0x4000_0000;
pub const FLAG_ACTIVITY_SINGLE_TOP: i32 = 0x2000_0000;
pub const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
pub const FLAG_ACTIVITY_MULTIPLE_TASK: i32 = 0x0800_0000;
pub const FLAG_ACTIVITY_CLEAR_TOP: i32 = 0x0400_0000;
pub const FLAG_ACTIVITY_PREVIOUS_IS_TOP: i32 = 0x0100_0000;
pub const FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS: i32 = 0x0080_0000;
pub const FLAG_ACTIVITY_BROUGHT_TO_FRONT: i32 = 0x0040_0000;
pub const FLAG_ACTIVITY_RESET_TASK_IF_NEEDED: i32 = 0x0020_0000;
pub const FLAG_ACTIVITY_NEW_DOCUMENT: i32 = 0x0008_0000;
pub const FLAG_ACTIVITY_NO_USER_ACTION: i32 = 0x0004_0000;
pub const FLAG_ACTIVITY_REORDER_TO_FRONT: i32 = 0x0002_0000;
pub const FLAG_ACTIVITY_CLEAR_TASK: i32 = 0x0000_8000;
pub const FLAG_ACTIVITY_LAUNCH_ADJACENT: i32 = 0x0000_1000;
/// `ActivityInfo.LAUNCH_*`.
pub const LAUNCH_MULTIPLE: i32 = 0;
pub const LAUNCH_SINGLE_TOP: i32 = 1;
pub const LAUNCH_SINGLE_TASK: i32 = 2;
pub const LAUNCH_SINGLE_INSTANCE: i32 = 3;
pub const LAUNCH_SINGLE_INSTANCE_PER_TASK: i32 = 4;
/// `ActivityInfo.DOCUMENT_LAUNCH_*`.
const DOCUMENT_LAUNCH_INTO_EXISTING: i32 = 1;
const DOCUMENT_LAUNCH_ALWAYS: i32 = 2;
const DOCUMENT_LAUNCH_NEVER: i32 = 3;
/// `ActivityManager.START_*`.
pub const START_SUCCESS: i32 = 0;
pub const START_TASK_TO_FRONT: i32 = 2;
pub const START_DELIVERED_TO_TOP: i32 = 3;

/// `ActivityRecord.State`, of the ones the app host sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Made, not yet launched in its process.
    Initializing,
    /// Visible, not resumed: started again from stopped.
    Started,
    Resumed,
    Pausing,
    Paused,
    Stopping,
    Stopped,
    Destroying,
}

/// What an activity's results and new intents carry: opaque to the stack.
pub type Payload = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub token: u64,
    pub task: i32,
    pub start_seq: i64,
    /// `mActivityComponent`, `package/class`: its class, or an alias's
    /// target.
    pub component: String,
    pub launch_mode: i32,
    pub user: u32,
    pub state: State,
    pub finishing: bool,
    /// The activity its result goes to (`resultTo`), and its result:
    /// `RESULT_CANCELED` unless it set one.
    pub result_to: Option<u64>,
    pub result: Option<Payload>,
    /// Results and new intents to deliver at its next resume.
    pub results: Vec<Payload>,
    pub new_intents: Vec<Payload>,
    /// `mOccludesParent`: it hides what is under it.
    pub occludes_parent: bool,
    /// `isNoHistory`: finished rather than stopped.
    pub no_history: bool,
}

impl Activity {
    /// A new opaque activity, not yet launched.
    pub fn new(token: u64, task: i32, start_seq: i64, component: &str, launch_mode: i32, user: u32, result_to: Option<u64>) -> Activity {
        Activity {
            token,
            task,
            start_seq,
            component: component.to_string(),
            launch_mode,
            user,
            state: State::Initializing,
            finishing: false,
            result_to,
            result: None,
            results: Vec::new(),
            new_intents: Vec::new(),
            occludes_parent: true,
            no_history: false,
        }
    }
}

/// A task's identity (`Task.setIntent`).
#[derive(Clone)]
pub struct Task {
    pub id: i32,
    /// `rootAffinity`: its first activity's affinity.
    pub affinity: Option<String>,
    /// `realActivity`: the component of its intent.
    pub real_activity: String,
    /// `intent`, its base intent, and the intent as the app host writes
    /// it back.
    pub intent: Intent,
    pub base: LaunchIntent,
    /// `effectiveUid`: the uid of the activity that set its identity.
    pub uid: u32,
    pub user: u32,
    /// `lastActiveTime`: when it was made, last resumed or paused
    /// (`touchActiveTime`), in `elapsedRealtime` milliseconds.
    pub last_active: i64,
}

/// What a start sets a task's identity to: its activity's component,
/// intent, affinity and uid.
pub struct Identity<'a> {
    pub component: &'a str,
    pub intent: &'a Intent,
    pub base: &'a LaunchIntent,
    pub affinity: Option<&'a str>,
    pub uid: u32,
}

/// `TaskFragment.TASK_FRAGMENT_VISIBILITY_*` of a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    /// Behind a translucent task: visible, not resumed.
    BehindTranslucent,
    Invisible,
}

/// A decision of the stack, for the app host to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `LaunchActivityItem` and `ResumeActivityItem`.
    Launch(u64),
    /// `PauseActivityItem`: finishing, user leaving.
    Pause { token: u64, finishing: bool, user_leaving: bool },
    /// `ActivityResultItem`, `NewIntentItem` and `ResumeActivityItem`.
    Resume { token: u64, results: Vec<Payload>, new_intents: Vec<Payload> },
    /// `NewIntentItem` to a resumed activity.
    NewIntent { token: u64, intents: Vec<Payload> },
    /// `ActivityResultItem` to a resumed activity.
    Result { token: u64, results: Vec<Payload> },
    /// `StartActivityItem`: a stopped activity visible again, not resumed.
    Start(u64),
    Stop(u64),
    /// `DestroyActivityItem`, finishing.
    Destroy(u64),
    TopResumed { token: u64, on_top: bool },
    /// The top resumed activity's loss is given up on after
    /// `TOP_RESUMED_STATE_LOSS_TIMEOUT` unless it reported it: the loss of
    /// this number.
    LossTimeout(u64),
    /// Its windows shown or hidden (`dispatchAppVisibility`).
    Visible { token: u64, visible: bool },
    /// The task is in front and focused.
    Front(i32),
    /// A task without activities, gone.
    TaskEmpty(i32),
}

/// Where a start goes, as `startActivityInner` decides it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decision {
    /// The launch flags (`mLaunchFlags`).
    pub flags: i32,
    /// The result the start asked for is cancelled (`RESULT_CANCELED`).
    pub cancel_result: bool,
    /// The activity its result goes to, if it still does.
    pub result_to: Option<u64>,
    /// The task brought to the front (`setTargetRootTaskIfNeeded`).
    pub front: Option<i32>,
    /// The activities finished first, top first (`performClearTop`,
    /// `performClearTaskForReuse`).
    pub clear: Vec<u64>,
    /// An activity moved to the top of its task (`REORDER_TO_FRONT`).
    pub reorder: Option<u64>,
    /// The target task takes the start's intent (`Task.setIntent`).
    pub set_intent: bool,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// A new activity on top of a task, or of a new one.
    New(Option<i32>),
    /// The intent goes to an existing activity: the start's result.
    Deliver(u64, i32),
    /// Nothing but the task in front: the start's result.
    Front(i32),
}

impl Default for Outcome {
    fn default() -> Self {
        Outcome::New(None)
    }
}

/// A start, as `ActivityStarter` sees it.
pub struct Start<'a> {
    /// `mActivityComponent`.
    pub component: &'a str,
    pub affinity: Option<&'a str>,
    pub launch_mode: i32,
    pub document_launch_mode: i32,
    /// Its intent, with the caller's flags.
    pub intent: &'a Intent,
    pub user: u32,
    /// The activity it is started from (`sourceRecord`), and the one its
    /// result goes to (`resultRecord`).
    pub source: Option<u64>,
    pub result_to: Option<u64>,
}

/// What a start comes to once decided.
pub enum Then {
    /// A new activity, in a new task if given one.
    Add(Activity, Option<Box<Task>>),
    /// A new intent to an existing activity.
    Deliver(u64, Payload),
    Nothing,
}

/// The tasks, back to front, and the activities, bottom to top, of all
/// tasks.
#[derive(Default)]
pub struct Stack {
    pub activities: Vec<Activity>,
    pub tasks: Vec<Task>,
    /// Whether the display is freeform: tasks do not hide each other.
    pub freeform: bool,
    /// The task last reported focused.
    front: Option<i32>,
    /// `mTopResumedActivity`, `mLastReportedTopResumedActivity`,
    /// `mTopResumedActivityWaitingForPrev`, and the latest loss.
    top_resumed: Option<u64>,
    last_reported: Option<u64>,
    waiting_for_prev: bool,
    loss: u64,
}

impl Stack {
    pub fn get(&self, token: u64) -> Option<&Activity> {
        self.activities.iter().find(|a| a.token == token)
    }

    fn get_mut(&mut self, token: u64) -> Option<&mut Activity> {
        self.activities.iter_mut().find(|a| a.token == token)
    }

    pub fn task(&self, id: i32) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// The top activity of `task` that is not finishing.
    pub fn top(&self, task: i32) -> Option<&Activity> {
        self.activities.iter().rev().find(|a| a.task == task && !a.finishing)
    }

    /// The root activity of `task`, not finishing.
    pub fn root(&self, task: i32) -> Option<&Activity> {
        self.activities.iter().find(|a| a.task == task && !a.finishing)
    }

    /// The focused task: the front one with a running activity
    /// (`getTopDisplayFocusedRootTask`).
    pub fn focused(&self) -> Option<i32> {
        self.tasks.iter().rev().map(|t| t.id).find(|&t| self.top(t).is_some())
    }

    /// The focused task's top activity (`topRunningActivity`).
    pub fn focused_top(&self) -> Option<&Activity> {
        self.focused().and_then(|t| self.top(t))
    }

    /// `TaskFragment.getVisibility` of `task`: invisible without a running
    /// activity; on a fullscreen display, behind a task in front with a
    /// running activity, invisible unless that is translucent. Freeform
    /// tasks neither fill the display nor match its bounds, so do not
    /// hide one another.
    pub fn visibility(&self, task: i32) -> Visibility {
        if self.top(task).is_none() {
            return Visibility::Invisible;
        }
        if self.freeform {
            return Visibility::Visible;
        }
        let mut translucent = false;
        for other in self.tasks.iter().rev().take_while(|t| t.id != task) {
            if self.top(other.id).is_none() {
                continue;
            }
            if !self.translucent(other.id) {
                return Visibility::Invisible;
            }
            translucent = true;
        }
        if translucent { Visibility::BehindTranslucent } else { Visibility::Visible }
    }

    /// `TaskFragment.isTranslucent` of a visible task: none of its visible
    /// activities occludes it, so none that is not finishing does (its top
    /// one that does is visible).
    pub fn translucent(&self, task: i32) -> bool {
        !self.activities.iter().any(|a| a.task == task && !a.finishing && a.occludes_parent)
    }

    /// Whether `a` is visible (`EnsureActivitiesVisibleHelper`): in a
    /// visible task, not under an activity of it that occludes it.
    pub fn shown(&self, a: &Activity) -> bool {
        if a.finishing || self.visibility(a.task) == Visibility::Invisible {
            return false;
        }
        for above in self.activities.iter().rev().filter(|b| b.task == a.task && !b.finishing) {
            if above.token == a.token {
                return true;
            }
            if above.occludes_parent {
                return false;
            }
        }
        false
    }

    /// Whether `a` is to be resumed: the top of a visible task
    /// (`shouldResumeActivity`).
    fn resumable(&self, a: &Activity) -> bool {
        self.visibility(a.task) == Visibility::Visible && self.top(a.task).is_some_and(|t| t.token == a.token)
    }

    /// `FindTaskResult`: the top activity of the front task of
    /// `component`, else of the front task of `affinity`, skipping tasks
    /// topped by a `singleInstance` activity.
    pub fn find_task(&self, component: &str, affinity: Option<&str>, user: u32) -> Option<u64> {
        let mut candidate = None;
        for task in self.tasks.iter().rev().filter(|t| t.user == user) {
            let Some(top) = self.top(task.id).filter(|a| a.user == user && a.launch_mode != LAUNCH_SINGLE_INSTANCE) else { continue };
            if task.real_activity == component {
                return Some(top.token);
            }
            if candidate.is_none() && task.affinity.is_some() && task.affinity.as_deref() == affinity {
                candidate = Some(top.token);
            }
        }
        candidate
    }

    /// `findActivity` of `component`, not comparing intents.
    fn find_activity(&self, component: &str, user: u32) -> Option<&Activity> {
        self.activities.iter().rev().find(|a| !a.finishing && a.user == user && a.component == component)
    }

    /// `Task.findActivityInHistory`.
    fn find_in_history(&self, task: i32, component: &str, user: u32) -> Option<&Activity> {
        self.activities.iter().rev().find(|a| a.task == task && !a.finishing && a.user == user && a.component == component)
    }

    /// `startActivityInner`'s decision for `s`, or what of it the app host
    /// does not do.
    pub fn decide(&self, s: &Start<'_>) -> Result<Decision, &'static str> {
        let mode = s.launch_mode;
        let single = matches!(mode, LAUNCH_SINGLE_INSTANCE | LAUNCH_SINGLE_TASK);
        let mut flags = document_flags(s.intent.flags, mode, s.document_launch_mode);
        if flags & FLAG_ACTIVITY_NEW_DOCUMENT != 0 {
            return Err("documents");
        }
        // `setInitialState`.
        if mode == LAUNCH_SINGLE_INSTANCE_PER_TASK {
            flags |= FLAG_ACTIVITY_NEW_TASK;
        }
        let mut d = Decision { result_to: s.result_to, ..Default::default() };
        if d.result_to.is_some() && flags & FLAG_ACTIVITY_NEW_TASK != 0 {
            d.cancel_result = true;
            d.result_to = None;
        }
        let source = s.source.and_then(|t| self.get(t));
        let not_top = s.source.filter(|_| flags & FLAG_ACTIVITY_PREVIOUS_IS_TOP != 0);
        // `computeLaunchingTaskFlags`.
        if source.is_none_or(|src| src.launch_mode == LAUNCH_SINGLE_INSTANCE) || single {
            flags |= FLAG_ACTIVITY_NEW_TASK;
        }
        if flags & FLAG_ACTIVITY_LAUNCH_ADJACENT != 0 {
            if flags & FLAG_ACTIVITY_NEW_TASK == 0 || source.is_none() {
                flags &= !FLAG_ACTIVITY_LAUNCH_ADJACENT;
            } else {
                return Err("an adjacent launch");
            }
        }
        // `resolveReusableTask`.
        let put = ((flags & FLAG_ACTIVITY_NEW_TASK != 0 && flags & FLAG_ACTIVITY_MULTIPLE_TASK == 0) || single) && d.result_to.is_none();
        let mut reused = match put {
            false => None,
            true if mode == LAUNCH_SINGLE_INSTANCE => self.find_activity(s.component, s.user).map(|a| a.task),
            true => self.find_task(s.component, s.affinity, s.user).and_then(|t| self.get(t)).map(|a| a.task),
        };
        if mode == LAUNCH_SINGLE_INSTANCE_PER_TASK && reused.and_then(|t| self.root(t)).is_some_and(|r| r.component != s.component) {
            reused = None;
        }
        // `computeTargetTask`.
        let target = reused.or_else(|| if d.result_to.is_none() && flags & FLAG_ACTIVITY_NEW_TASK != 0 { None } else { source.map(|a| a.task) });
        d.outcome = Outcome::New(target);
        if let Some(task) = target
            && let Some(top) = self.top(task)
        {
            if mode == LAUNCH_SINGLE_INSTANCE && source.is_some_and(|a| a.task == task) && self.find_activity(s.component, s.user).is_some_and(|a| a.task != task) {
                return Err("a singleInstance activity in its caller's task while another instance runs");
            }
            // `setTargetRootTaskIfNeeded`.
            let current = self.focused().and_then(|f| self.activities.iter().rev().find(|a| a.task == f && !a.finishing && Some(a.token) != not_top));
            let mut moved_to_front = false;
            if current.is_none_or(|a| a.task != task) || self.focused() != Some(task) {
                flags |= FLAG_ACTIVITY_BROUGHT_TO_FRONT;
                d.front = Some(task);
                // A task already visible in multi-window mode was on top.
                moved_to_front = !(self.freeform && self.visibility(task) == Visibility::Visible);
            }
            // `complyActivityFlags`.
            let reused_top = reused.and_then(|t| self.top(t));
            if reused_top.is_some() && flags & FLAG_ACTIVITY_RESET_TASK_IF_NEEDED != 0 {
                return Err("a task's reset");
            }
            let task_record = self.task(task).ok_or("a task without its record")?;
            let mut deliver = None;
            let mut adding = false;
            if flags & (FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_CLEAR_TASK) == FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_CLEAR_TASK {
                d.clear = self.activities.iter().rev().filter(|a| a.task == task && !a.finishing).map(|a| a.token).collect();
                d.set_intent = true;
                adding = true;
            } else if flags & FLAG_ACTIVITY_CLEAR_TOP != 0 || single || mode == LAUNCH_SINGLE_INSTANCE_PER_TASK {
                match self.find_in_history(task, s.component, s.user) {
                    Some(r) => {
                        let at = self.activities.iter().position(|a| a.token == r.token).unwrap();
                        d.clear = self.activities[at + 1..].iter().rev().filter(|a| a.task == task && !a.finishing).map(|a| a.token).collect();
                        // A standard activity is made anew.
                        if r.launch_mode == LAUNCH_MULTIPLE && flags & FLAG_ACTIVITY_SINGLE_TOP == 0 {
                            d.clear.push(r.token);
                            adding = true;
                        } else {
                            d.set_intent = self.root(task).is_some_and(|root| root.token == r.token);
                            deliver = Some(r.token);
                        }
                    }
                    None => adding = true,
                }
            } else if flags & FLAG_ACTIVITY_REORDER_TO_FRONT != 0 {
                match self.find_in_history(task, s.component, s.user) {
                    Some(r) => {
                        d.reorder = Some(r.token);
                        deliver = Some(r.token);
                    }
                    None => adding = true,
                }
            } else if s.component == task_record.real_activity {
                if (flags & FLAG_ACTIVITY_SINGLE_TOP != 0 || mode == LAUNCH_SINGLE_TOP) && top.component == s.component && d.result_to.is_none() {
                    d.set_intent = self.root(task).is_some_and(|root| root.token == top.token);
                    deliver = Some(top.token);
                } else if !same_intent_filter(&task_record.intent, s.intent) || reused_top.is_none() {
                    adding = true;
                }
            } else {
                adding = true;
            }
            if !adding {
                let code = if moved_to_front { START_TASK_TO_FRONT } else { START_DELIVERED_TO_TOP };
                d.outcome = deliver.map_or(Outcome::Front(code), |t| Outcome::Deliver(t, code));
                d.flags = flags;
                return Ok(d);
            }
        }
        // `deliverToCurrentTopIfNeeded`, after the target task came to the
        // front and was cleared.
        let focused = d.front.or_else(|| self.focused());
        let top = focused.and_then(|f| self.activities.iter().rev().find(|a| a.task == f && !a.finishing && Some(a.token) != not_top && !d.clear.contains(&a.token)));
        if let Some(top) = top
            && top.component == s.component
            && top.user == s.user
            && top.state != State::Initializing
            && (flags & FLAG_ACTIVITY_SINGLE_TOP != 0 || mode == LAUNCH_SINGLE_TOP)
        {
            if d.result_to.take().is_some() {
                d.cancel_result = true;
            }
            d.outcome = Outcome::Deliver(top.token, START_DELIVERED_TO_TOP);
        }
        d.flags = flags;
        Ok(d)
    }

    /// Carries out `d` with what it comes to: the task brought to the
    /// front, the activities cleared, then the new activity or intent,
    /// and the focused tasks' top activities resumed.
    pub fn apply(&mut self, d: &Decision, then: Then, identity: Option<Identity<'_>>, user_leaving: bool) -> Vec<Command> {
        let mut out = Vec::new();
        if let Some(task) = d.front {
            self.move_to_front(task);
        }
        for &t in &d.clear {
            self.finish_one(t, user_leaving, &mut out);
        }
        if let Some(t) = d.reorder
            && let Some(at) = self.activities.iter().position(|a| a.token == t)
        {
            let a = self.activities.remove(at);
            self.activities.push(a);
        }
        match then {
            Then::Add(activity, task) => {
                if let Some(mut task) = task {
                    task.last_active = super::alarm_service::elapsed_ms();
                    self.tasks.push(*task);
                } else {
                    self.move_to_front(activity.task);
                }
                if d.set_intent
                    && let Some(identity) = identity
                {
                    self.set_intent(activity.task, identity);
                }
                self.activities.push(activity);
            }
            Then::Deliver(token, payload) => {
                if d.set_intent
                    && let Some(identity) = identity
                    && let Some(task) = self.get(token).map(|a| a.task)
                {
                    self.set_intent(task, identity);
                }
                out.extend(self.new_intent(token, payload));
            }
            Then::Nothing => {}
        }
        self.resume_focused(user_leaving, &mut out);
        self.settle(&mut out);
        out
    }

    /// `Task.setIntent` of a start into it.
    fn set_intent(&mut self, task: i32, identity: Identity<'_>) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task) {
            t.real_activity = identity.component.to_string();
            t.intent = identity.intent.clone();
            t.base = identity.base.clone();
            t.uid = identity.uid;
            if t.affinity.is_none() {
                t.affinity = identity.affinity.map(str::to_string);
            }
        }
    }

    /// `touchActiveTime` of `task`.
    fn touch(&mut self, task: i32) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task) {
            t.last_active = super::alarm_service::elapsed_ms();
        }
    }

    fn move_to_front(&mut self, task: i32) {
        if let Some(at) = self.tasks.iter().position(|t| t.id == task) {
            let t = self.tasks.remove(at);
            self.tasks.push(t);
        }
    }

    /// The task is brought to the front (`moveTaskToFront`).
    pub fn bring_to_front(&mut self, task: i32) -> Vec<Command> {
        let mut out = Vec::new();
        self.move_to_front(task);
        self.resume_focused(true, &mut out);
        self.settle(&mut out);
        out
    }

    /// `startPausing` of the resumed `token`; the top resumed activity is
    /// told it lost its state first.
    fn pause(&mut self, token: u64, user_leaving: bool, out: &mut Vec<Command>) {
        let Some(a) = self.get_mut(token) else { return };
        a.state = State::Pausing;
        let (finishing, task) = (a.finishing, a.task);
        self.touch(task);
        self.update_top_resumed(out);
        out.push(Command::Pause { token, finishing, user_leaving });
    }

    /// `resumeFocusedTasksTopActivities`: the resumed activities of tasks
    /// not visible, or visible behind a translucent one, pause
    /// (`pauseBackTasks`); once none is pausing, the focused task's top
    /// activity is launched or resumed, then those of the other visible
    /// tasks. Whether one was.
    fn resume_focused(&mut self, user_leaving: bool, out: &mut Vec<Command>) -> bool {
        let hidden: Vec<u64> = self.activities.iter().filter(|a| a.state == State::Resumed && self.visibility(a.task) != Visibility::Visible).map(|a| a.token).collect();
        for t in hidden {
            self.pause(t, user_leaving, out);
        }
        let focused = self.focused();
        let mut order: Vec<i32> = focused.into_iter().collect();
        order.extend(self.tasks.iter().rev().map(|t| t.id).filter(|&t| Some(t) != focused && self.visibility(t) == Visibility::Visible));
        let mut resumed = false;
        for task in order {
            resumed |= self.resume_top(task, user_leaving, out);
        }
        resumed
    }

    /// `TaskFragment.resumeTopActivity`: pause another resumed activity of
    /// the task first, else, with no activity pausing, launch or resume
    /// the top one. Whether it was.
    fn resume_top(&mut self, task: i32, user_leaving: bool, out: &mut Vec<Command>) -> bool {
        let Some(top) = self.top(task).map(|a| a.token) else { return false };
        let others: Vec<u64> = self.activities.iter().filter(|a| a.task == task && a.token != top && a.state == State::Resumed).map(|a| a.token).collect();
        for t in others {
            self.pause(t, user_leaving, out);
        }
        if self.activities.iter().any(|a| a.state == State::Pausing) {
            return false;
        }
        if !matches!(self.get(top).unwrap().state, State::Resumed | State::Pausing | State::Destroying) {
            // The task becomes active (`setResumedActivity`).
            self.touch(task);
        }
        let a = self.get_mut(top).unwrap();
        match a.state {
            State::Initializing => {
                a.state = State::Resumed;
                out.push(Command::Launch(top));
            }
            State::Started | State::Paused | State::Stopping | State::Stopped => {
                a.state = State::Resumed;
                let (results, new_intents) = (std::mem::take(&mut a.results), std::mem::take(&mut a.new_intents));
                out.push(Command::Visible { token: top, visible: true });
                out.push(Command::Resume { token: top, results, new_intents });
            }
            _ => return false,
        }
        true
    }

    /// The focus and the top resumed activity follow what changed, and
    /// the stopped activities now visible that are not to be resumed are
    /// started (`makeVisibleIfNeeded`, `shouldStartActivity`).
    fn settle(&mut self, out: &mut Vec<Command>) {
        let focused = self.focused();
        if focused != self.front {
            self.front = focused;
            if let Some(task) = focused {
                out.push(Command::Front(task));
            }
        }
        self.update_top_resumed(out);
        let shown: Vec<u64> = self
            .activities
            .iter()
            .filter(|a| matches!(a.state, State::Stopping | State::Stopped) && self.shown(a) && !self.resumable(a))
            .map(|a| a.token)
            .collect();
        for token in shown {
            self.get_mut(token).unwrap().state = State::Started;
            out.push(Command::Visible { token, visible: true });
            out.push(Command::Start(token));
        }
    }

    /// The activities no longer visible but `except` are stopped
    /// (`makeInvisible`, `stopIfPossible`), a no-history one finished
    /// instead.
    fn stop_hidden(&mut self, except: Option<u64>, out: &mut Vec<Command>) {
        let hidden: Vec<(u64, bool)> = self
            .activities
            .iter()
            .filter(|a| Some(a.token) != except && !a.finishing && matches!(a.state, State::Started | State::Paused) && !self.shown(a))
            .map(|a| (a.token, a.no_history))
            .collect();
        for (token, no_history) in hidden {
            if no_history {
                self.finish_one(token, false, out);
                continue;
            }
            self.get_mut(token).unwrap().state = State::Stopping;
            out.push(Command::Visible { token, visible: false });
            out.push(Command::Stop(token));
        }
    }

    /// `updateTopResumedActivityIfNeeded`: the focused task's resumed
    /// activity is the top resumed one; the one last told so is told it
    /// lost it, and the new one is told once that one reported it did.
    fn update_top_resumed(&mut self, out: &mut Vec<Command>) {
        let new = self.focused().and_then(|f| self.activities.iter().find(|a| a.task == f && a.state == State::Resumed)).map(|a| a.token);
        if new == self.top_resumed {
            return;
        }
        if let Some(last) = self.last_reported
            && !self.waiting_for_prev
        {
            out.push(Command::TopResumed { token: last, on_top: false });
            self.loss += 1;
            out.push(Command::LossTimeout(self.loss));
            self.waiting_for_prev = true;
            self.last_reported = None;
        }
        self.top_resumed = new;
        self.report_top_resumed(out);
    }

    fn report_top_resumed(&mut self, out: &mut Vec<Command>) {
        if let Some(top) = self.top_resumed
            && !self.waiting_for_prev
        {
            out.push(Command::TopResumed { token: top, on_top: true });
            self.last_reported = Some(top);
        }
    }

    /// `handleTopResumedStateReleased`: the last top resumed activity
    /// reported its loss (`activityTopResumedStateLost`), or loss `timeout`
    /// timed out.
    pub fn top_resumed_released(&mut self, timeout: Option<u64>) -> Vec<Command> {
        let mut out = Vec::new();
        if !self.waiting_for_prev || timeout.is_some_and(|l| l != self.loss) {
            return out;
        }
        self.waiting_for_prev = false;
        self.report_top_resumed(&mut out);
        out
    }

    /// `activityPaused`: a finishing activity completes finishing, then
    /// the focused tasks' tops resume.
    pub fn paused(&mut self, token: u64) -> Vec<Command> {
        let mut out = Vec::new();
        let Some(a) = self.get_mut(token) else { return out };
        a.state = State::Paused;
        if a.finishing {
            self.complete_finishing(token, &mut out);
        }
        self.resume_focused(false, &mut out);
        self.settle(&mut out);
        out
    }

    /// `completeFinishing` of the paused, finishing `token`: destroyed
    /// once the next activity is idle if that is not yet visible or is
    /// resumed in its place, else at once.
    fn complete_finishing(&mut self, token: u64, out: &mut Vec<Command>) {
        let not_yet_visible = self.focused_top().is_some_and(|n| !matches!(n.state, State::Started | State::Resumed | State::Pausing | State::Paused));
        if not_yet_visible || self.resume_focused(false, out) {
            return;
        }
        self.get_mut(token).unwrap().state = State::Destroying;
        out.push(Command::Destroy(token));
    }

    /// `activityIdleInternal` of the top of a task: the finishing
    /// activities are destroyed, those no longer visible stopped.
    pub fn idle(&mut self, token: u64) -> Vec<Command> {
        let Some(task) = self.get(token).map(|a| a.task) else { return Vec::new() };
        if self.top(task).map(|a| a.token) != Some(token) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for a in self.activities.iter_mut().filter(|a| a.token != token) {
            if a.finishing && matches!(a.state, State::Started | State::Paused | State::Stopping | State::Stopped) {
                a.state = State::Destroying;
                out.push(Command::Destroy(a.token));
            }
        }
        self.stop_hidden(Some(token), &mut out);
        self.settle(&mut out);
        out
    }

    /// `setOccludesParent` of `token` (`convertFromTranslucent`,
    /// `convertToTranslucent`): whether it changed; the activities'
    /// visibility follows (`ensureActivitiesVisible`), those it now hides
    /// stopped.
    pub fn set_occludes_parent(&mut self, token: u64, occludes: bool) -> (bool, Vec<Command>) {
        let mut out = Vec::new();
        let Some(a) = self.get_mut(token) else { return (false, out) };
        let changed = std::mem::replace(&mut a.occludes_parent, occludes) != occludes;
        if changed || !occludes {
            self.resume_focused(false, &mut out);
            self.stop_hidden(None, &mut out);
            self.settle(&mut out);
        }
        (changed, out)
    }

    /// `activityStopped`.
    pub fn stopped(&mut self, token: u64) {
        if let Some(a) = self.get_mut(token)
            && a.state == State::Stopping
        {
            a.state = State::Stopped;
        }
    }

    /// `activityDestroyed`: it goes; a task left without activities goes
    /// too.
    pub fn destroyed(&mut self, token: u64) -> Vec<Command> {
        let mut out = Vec::new();
        let Some(at) = self.activities.iter().position(|a| a.token == token) else { return out };
        let task = self.activities.remove(at).task;
        self.remove_if_empty(task, &mut out);
        self.settle(&mut out);
        out
    }

    fn remove_if_empty(&mut self, task: i32, out: &mut Vec<Command>) {
        if !self.activities.iter().any(|a| a.task == task) && self.task(task).is_some() {
            self.tasks.retain(|t| t.id != task);
            out.push(Command::TaskEmpty(task));
        }
    }

    /// `finishIfPossible` of `token`, its result to the activity that
    /// asked for it: whether it finishes.
    pub fn finish(&mut self, token: u64) -> (bool, Vec<Command>) {
        let mut out = Vec::new();
        if self.get(token).is_none() {
            return (false, out);
        }
        self.finish_one(token, false, &mut out);
        self.resume_focused(false, &mut out);
        self.settle(&mut out);
        (true, out)
    }

    /// `removeActivities`: the task's activities finish, bottom first.
    pub fn remove(&mut self, task: i32) -> Vec<Command> {
        let mut out = Vec::new();
        let all: Vec<u64> = self.activities.iter().filter(|a| a.task == task && !a.finishing).map(|a| a.token).collect();
        for t in all {
            self.finish_one(t, false, &mut out);
        }
        self.resume_focused(false, &mut out);
        self.settle(&mut out);
        out
    }

    fn finish_one(&mut self, token: u64, user_leaving: bool, out: &mut Vec<Command>) {
        let Some(a) = self.get_mut(token) else { return };
        if a.finishing {
            return;
        }
        a.finishing = true;
        let (task, state) = (a.task, a.state);
        // `finishActivityResults`.
        if let (Some(to), Some(result)) = (a.result_to.take(), a.result.take()) {
            out.extend(self.send_result(to, result));
        }
        match state {
            State::Resumed => self.pause(token, user_leaving, out),
            State::Pausing | State::Destroying => {}
            // Never launched: it just goes.
            State::Initializing => {
                self.activities.retain(|a| a.token != token);
                self.remove_if_empty(task, out);
            }
            State::Started | State::Paused => self.complete_finishing(token, out),
            State::Stopping | State::Stopped => {
                self.get_mut(token).unwrap().state = State::Destroying;
                out.push(Command::Destroy(token));
            }
        }
    }

    /// `sendResult` to `to`: now if it is resumed, else at its resume.
    pub fn send_result(&mut self, to: u64, result: Payload) -> Vec<Command> {
        let Some(r) = self.get_mut(to) else { return Vec::new() };
        if r.state == State::Resumed {
            vec![Command::Result { token: to, results: vec![result] }]
        } else {
            r.results.push(result);
            Vec::new()
        }
    }

    /// `deliverNewIntentLocked` to `token`: now if it is resumed, else at
    /// its next resume.
    fn new_intent(&mut self, token: u64, intent: Payload) -> Vec<Command> {
        let Some(a) = self.get_mut(token) else { return Vec::new() };
        if a.state == State::Resumed {
            vec![Command::NewIntent { token, intents: vec![intent] }]
        } else {
            a.new_intents.push(intent);
            Vec::new()
        }
    }

    /// The process of `start_seq` died: its activities go, and the tasks
    /// they leave empty.
    pub fn process_died(&mut self, start_seq: i64) -> Vec<Command> {
        let mut out = Vec::new();
        let gone: Vec<(u64, i32)> = self.activities.iter().filter(|a| a.start_seq == start_seq).map(|a| (a.token, a.task)).collect();
        self.activities.retain(|a| a.start_seq != start_seq);
        for &(token, task) in &gone {
            if self.last_reported == Some(token) {
                self.last_reported = None;
            }
            self.remove_if_empty(task, &mut out);
        }
        self.resume_focused(false, &mut out);
        self.settle(&mut out);
        out
    }
}

/// `adjustLaunchFlagsToDocumentMode`.
fn document_flags(flags: i32, mode: i32, document_launch_mode: i32) -> i32 {
    let document = FLAG_ACTIVITY_NEW_DOCUMENT | FLAG_ACTIVITY_MULTIPLE_TASK;
    if flags & FLAG_ACTIVITY_NEW_DOCUMENT != 0 && matches!(mode, LAUNCH_SINGLE_INSTANCE | LAUNCH_SINGLE_TASK) {
        return flags & !document;
    }
    match document_launch_mode {
        DOCUMENT_LAUNCH_INTO_EXISTING | DOCUMENT_LAUNCH_ALWAYS => flags | FLAG_ACTIVITY_NEW_DOCUMENT,
        DOCUMENT_LAUNCH_NEVER if mode != LAUNCH_SINGLE_INSTANCE_PER_TASK || flags & FLAG_ACTIVITY_NEW_DOCUMENT != 0 => flags & !document,
        _ => flags,
    }
}

/// `Task.isSameIntentFilter` of a start of the task's real activity: its
/// intent, with the task's component and package, `filterEquals` the
/// task's.
fn same_intent_filter(task: &Intent, start: &Intent) -> bool {
    let mut intent = start.clone();
    intent.component = task.component.clone();
    if intent.selector.is_none() {
        intent.package = task.package.clone();
    }
    super::broadcasts::filter_equals(&intent, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Command::*;

    const APP: &str = "com.example";

    fn task(id: i32, class: &str, affinity: Option<&str>) -> Box<Task> {
        Box::new(Task { id, affinity: affinity.map(str::to_string), real_activity: format!("{APP}/{class}"), intent: Intent::default(), base: LaunchIntent::Component, uid: 10_100, user: 0, last_active: 0 })
    }

    fn activity(token: u64, task: i32, class: &str, mode: i32) -> Activity {
        Activity::new(token, task, 1, &format!("{APP}/{class}"), mode, 0, None)
    }

    /// `token` of `class` started as the first activity of a new task
    /// `id`, then paused-for, launched and its loss reported as the app
    /// would.
    fn launch_new(s: &mut Stack, token: u64, id: i32, class: &str, mode: i32) -> Vec<Command> {
        let d = Decision { outcome: Outcome::New(None), ..Default::default() };
        s.apply(&d, Then::Add(activity(token, id, class, mode), Some(task(id, class, Some(APP)))), None, true)
    }

    fn start<'a>(component: &'a str, mode: i32, intent: &'a Intent, source: Option<u64>) -> super::Start<'a> {
        super::Start { component, affinity: Some(APP), launch_mode: mode, document_launch_mode: 0, intent, user: 0, source, result_to: None }
    }

    #[test]
    fn a_started_activity_resumes_once_the_one_under_it_paused() {
        let mut s = Stack::default();
        assert_eq!(launch_new(&mut s, 1, 7, "A", 0), [Launch(1), Front(7), TopResumed { token: 1, on_top: true }]);
        // B on A: A pauses first, told it lost the top state.
        let d = Decision { outcome: Outcome::New(Some(7)), ..Default::default() };
        assert_eq!(
            s.apply(&d, Then::Add(activity(2, 7, "B", 0), None), None, true),
            [TopResumed { token: 1, on_top: false }, LossTimeout(1), Pause { token: 1, finishing: false, user_leaving: true }]
        );
        // B launches; it is told it is on top once A reported its loss.
        assert_eq!(s.paused(1), [Launch(2)]);
        assert_eq!(s.top_resumed_released(None), [TopResumed { token: 2, on_top: true }]);
        // B idle: A is stopped.
        assert_eq!(s.idle(2), [Visible { token: 1, visible: false }, Stop(1)]);
        s.stopped(1);
        assert_eq!(s.get(1).unwrap().state, State::Stopped);
    }

    #[test]
    fn a_finishing_activity_hands_its_result_back_and_is_destroyed() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", 0);
        let d = Decision { outcome: Outcome::New(Some(7)), ..Default::default() };
        let mut b = activity(2, 7, "B", 0);
        (b.result_to, b.result) = (Some(1), Some(42));
        s.apply(&d, Then::Add(b, None), None, true);
        s.paused(1);
        s.top_resumed_released(None);
        s.idle(2);
        s.stopped(1);
        let (finished, out) = s.finish(2);
        assert!(finished);
        assert_eq!(out, [TopResumed { token: 2, on_top: false }, LossTimeout(2), Pause { token: 2, finishing: true, user_leaving: false }]);
        // B paused: A resumes with B's result, B waits for A's idle.
        assert_eq!(s.paused(2), [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![42], new_intents: vec![] }]);
        assert_eq!(s.top_resumed_released(Some(2)), [TopResumed { token: 1, on_top: true }]);
        assert_eq!(s.idle(1), [Destroy(2)]);
        assert_eq!(s.destroyed(2), []);
        // A, the last activity, finishes: nothing takes its place, so it
        // is destroyed at once and the task goes.
        s.finish(1);
        assert_eq!(s.paused(1), [Destroy(1)]);
        assert_eq!(s.destroyed(1), [TaskEmpty(7)]);
        assert!(s.tasks.is_empty());
    }

    #[test]
    fn a_new_intent_reaches_a_resumed_activity_now_else_at_its_resume() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", LAUNCH_SINGLE_TASK);
        let none = Intent::default();
        // A singleTask activity started again from its own task: delivered.
        let d = s.decide(&start("com.example/A", LAUNCH_SINGLE_TASK, &none, Some(1))).unwrap();
        assert_eq!(d.outcome, Outcome::Deliver(1, START_DELIVERED_TO_TOP));
        assert_eq!(s.apply(&d, Then::Deliver(1, 5), None, true), [NewIntent { token: 1, intents: vec![5] }]);
        let d = Decision { outcome: Outcome::New(Some(7)), ..Default::default() };
        s.apply(&d, Then::Add(activity(2, 7, "B", 0), None), None, true);
        s.paused(1);
        // From B: B is cleared, and A gets the intent at its resume.
        let d = s.decide(&start("com.example/A", LAUNCH_SINGLE_TASK, &none, Some(2))).unwrap();
        assert_eq!((d.clear.clone(), d.outcome.clone()), (vec![2], Outcome::Deliver(1, START_DELIVERED_TO_TOP)));
        let out = s.apply(&d, Then::Deliver(1, 6), None, true);
        assert_eq!(out, [Pause { token: 2, finishing: true, user_leaving: true }]);
        assert_eq!(s.paused(2), [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![], new_intents: vec![6] }]);
    }

    #[test]
    fn a_launch_from_no_activity_or_a_single_instance_is_a_new_task() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "Main", LAUNCH_SINGLE_TASK);
        let none = Intent::default();
        // From a service: NEW_TASK, and the task of the affinity is reused.
        let d = s.decide(&start("com.example/Other", 0, &none, None)).unwrap();
        assert_eq!((d.flags & FLAG_ACTIVITY_NEW_TASK, d.outcome), (FLAG_ACTIVITY_NEW_TASK, Outcome::New(Some(7))));
        // A singleInstance activity of no affinity goes alone.
        let expired = super::Start { affinity: None, ..start("com.example/Expired", LAUNCH_SINGLE_INSTANCE, &none, None) };
        let d = s.decide(&expired).unwrap();
        assert_eq!(d.outcome, Outcome::New(None));
        let out = s.apply(&d, Then::Add(activity(2, 8, "Expired", LAUNCH_SINGLE_INSTANCE), Some(task(8, "Expired", None))), None, true);
        // On a fullscreen display the task behind pauses first.
        assert_eq!(out, [TopResumed { token: 1, on_top: false }, LossTimeout(1), Pause { token: 1, finishing: false, user_leaving: true }, Front(8)]);
        assert_eq!(s.paused(1), [Launch(2)]);
        // What it starts goes into a task of its own too: the affinity's.
        let d = s.decide(&start("com.example/Other", 0, &none, Some(2))).unwrap();
        assert_eq!(d.outcome, Outcome::New(Some(7)));
        assert_ne!(d.flags & FLAG_ACTIVITY_BROUGHT_TO_FRONT, 0);
        // Started again, it is found and gets the intent.
        let d = s.decide(&expired).unwrap();
        assert_eq!(d.outcome, Outcome::Deliver(2, START_DELIVERED_TO_TOP));
    }

    #[test]
    fn affinity_component_and_multiple_task_pick_the_task() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "Main", 0);
        let none = Intent::default();
        let new_task = Intent { flags: FLAG_ACTIVITY_NEW_TASK, ..Default::default() };
        // NEW_TASK of an activity of another affinity: a new task.
        let other = super::Start { affinity: Some("other"), ..start("com.example/B", 0, &new_task, Some(1)) };
        assert_eq!(s.decide(&other).unwrap().outcome, Outcome::New(None));
        // Of the same affinity: on top of the existing task.
        assert_eq!(s.decide(&start("com.example/B", 0, &new_task, Some(1))).unwrap().outcome, Outcome::New(Some(7)));
        // The task's own root, same intent: the task in front, nothing
        // added.
        assert_eq!(s.decide(&start("com.example/Main", 0, &new_task, Some(1))).unwrap().outcome, Outcome::Front(START_DELIVERED_TO_TOP));
        // MULTIPLE_TASK: always a new one.
        let multiple = Intent { flags: FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_MULTIPLE_TASK, ..Default::default() };
        assert_eq!(s.decide(&start("com.example/Main", 0, &multiple, Some(1))).unwrap().outcome, Outcome::New(None));
        // Without NEW_TASK from an activity: its task.
        assert_eq!(s.decide(&start("com.example/B", 0, &none, Some(1))).unwrap().outcome, Outcome::New(Some(7)));
        // A result asked for with NEW_TASK is cancelled.
        let for_result = super::Start { result_to: Some(1), ..other };
        let d = s.decide(&for_result).unwrap();
        assert!(d.cancel_result && d.result_to.is_none());
        // Documents are not done.
        let document = Intent { flags: FLAG_ACTIVITY_NEW_DOCUMENT, ..Default::default() };
        assert!(s.decide(&start("com.example/B", 0, &document, Some(1))).is_err());
    }

    #[test]
    fn single_top_delivers_to_the_focused_top() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", LAUNCH_SINGLE_TOP);
        let none = Intent::default();
        assert_eq!(s.decide(&start("com.example/A", LAUNCH_SINGLE_TOP, &none, Some(1))).unwrap().outcome, Outcome::Deliver(1, START_DELIVERED_TO_TOP));
        let flagged = Intent { flags: FLAG_ACTIVITY_SINGLE_TOP, ..Default::default() };
        assert_eq!(s.decide(&start("com.example/A", 0, &flagged, Some(1))).unwrap().outcome, Outcome::Deliver(1, START_DELIVERED_TO_TOP));
        assert_eq!(s.decide(&start("com.example/A", 0, &none, Some(1))).unwrap().outcome, Outcome::New(Some(7)));
        // CLEAR_TOP keeps a singleTop activity, and makes a standard one
        // anew.
        let clear = Intent { flags: FLAG_ACTIVITY_CLEAR_TOP, ..Default::default() };
        assert_eq!(s.decide(&start("com.example/A", 0, &clear, Some(1))).unwrap().outcome, Outcome::Deliver(1, START_DELIVERED_TO_TOP));
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", 0);
        let d = s.decide(&start("com.example/A", 0, &clear, Some(1))).unwrap();
        assert_eq!((d.clear, d.outcome), (vec![1], Outcome::New(Some(7))));
    }

    #[test]
    fn freeform_tasks_stay_resumed_and_focus_moves() {
        let mut s = Stack { freeform: true, ..Default::default() };
        launch_new(&mut s, 1, 7, "A", 0);
        // Another task's start pauses nothing of this one; it takes the
        // top resumed state.
        assert_eq!(
            launch_new(&mut s, 2, 8, "B", 0),
            [Launch(2), Front(8), TopResumed { token: 1, on_top: false }, LossTimeout(1)]
        );
        assert_eq!(s.top_resumed_released(Some(1)), [TopResumed { token: 2, on_top: true }]);
        // Brought to the front, the first takes it back.
        assert_eq!(s.bring_to_front(7), [Front(7), TopResumed { token: 2, on_top: false }, LossTimeout(2)]);
        // A stale timeout changes nothing.
        assert_eq!(s.top_resumed_released(Some(1)), []);
        assert_eq!(s.top_resumed_released(None), [TopResumed { token: 1, on_top: true }]);
        // The front task's last activity finishes: focus moves back, and,
        // the other being resumed already, it is destroyed at once.
        assert_eq!(s.finish(1).1, [TopResumed { token: 1, on_top: false }, LossTimeout(3), Pause { token: 1, finishing: true, user_leaving: false }, Front(8)]);
        assert_eq!(s.paused(1), [Destroy(1)]);
        assert_eq!(s.destroyed(1), [TaskEmpty(7)]);
    }

    #[test]
    fn a_fullscreen_task_returns_when_the_one_in_front_finishes() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", 0);
        launch_new(&mut s, 2, 8, "B", 0);
        s.paused(1);
        s.top_resumed_released(None);
        assert_eq!(s.idle(2), [Visible { token: 1, visible: false }, Stop(1)]);
        s.stopped(1);
        // B finishes: A's task is focused, and A comes back once B paused;
        // B is destroyed once A is idle.
        assert_eq!(s.finish(2).1.last(), Some(&Front(7)));
        assert_eq!(s.paused(2), [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![], new_intents: vec![] }]);
        assert_eq!(s.idle(1), [Destroy(2)]);
        assert_eq!(s.destroyed(2), [TaskEmpty(8)]);
    }

    #[test]
    fn a_dead_process_takes_its_tasks() {
        let mut s = Stack { freeform: true, ..Default::default() };
        launch_new(&mut s, 1, 7, "A", 0);
        let mut b = activity(2, 8, "B", 0);
        b.start_seq = 2;
        let d = Decision::default();
        s.apply(&d, Then::Add(b, Some(task(8, "B", None))), None, true);
        assert_eq!(s.process_died(2), [TaskEmpty(8), Front(7)]);
        assert!(s.get(2).is_none() && s.get(1).is_some());
    }

    #[test]
    fn the_activity_under_a_translucent_one_stays_visible_and_paused() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", 0);
        let mut b = activity(2, 7, "Dialog", 0);
        b.occludes_parent = false;
        let d = Decision { outcome: Outcome::New(Some(7)), ..Default::default() };
        s.apply(&d, Then::Add(b, None), None, true);
        assert_eq!(s.paused(1), [Launch(2)]);
        s.top_resumed_released(None);
        // B idle: A is not stopped.
        assert_eq!(s.idle(2), []);
        assert_eq!(s.get(1).unwrap().state, State::Paused);
        // B finishes: A resumes; B is destroyed once A is idle.
        s.finish(2);
        assert_eq!(s.paused(2), [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![], new_intents: vec![] }]);
        assert_eq!(s.idle(1), [Destroy(2)]);
    }

    #[test]
    fn a_task_behind_a_translucent_one_is_paused_and_shown_again_when_uncovered() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "A", 0);
        let mut b = activity(2, 8, "B", 0);
        b.occludes_parent = false;
        let out = s.apply(&Decision::default(), Then::Add(b, Some(task(8, "B", None))), None, true);
        assert_eq!(out, [TopResumed { token: 1, on_top: false }, LossTimeout(1), Pause { token: 1, finishing: false, user_leaving: true }, Front(8)]);
        assert_eq!(s.paused(1), [Launch(2)]);
        assert_eq!(s.visibility(7), Visibility::BehindTranslucent);
        assert_eq!(s.idle(2), []);
        // B turns opaque (`convertFromTranslucent`): A is hidden and stops.
        assert_eq!(s.set_occludes_parent(2, true), (true, vec![Visible { token: 1, visible: false }, Stop(1)]));
        s.stopped(1);
        assert_eq!(s.set_occludes_parent(2, true), (false, vec![]));
        // Translucent again: A shows, started but not resumed.
        assert_eq!(s.set_occludes_parent(2, false), (true, vec![Visible { token: 1, visible: true }, Start(1)]));
        assert_eq!(s.get(1).unwrap().state, State::Started);
    }

    #[test]
    fn a_no_history_activity_finishes_instead_of_stopping() {
        let mut s = Stack::default();
        launch_new(&mut s, 1, 7, "Main", 0);
        let mut b = activity(2, 7, "Pick", 0);
        b.no_history = true;
        (b.result_to, b.result) = (Some(1), Some(42));
        let d = Decision { outcome: Outcome::New(Some(7)), ..Default::default() };
        s.apply(&d, Then::Add(b, None), None, true);
        s.paused(1);
        s.top_resumed_released(None);
        assert_eq!(s.idle(2), [Visible { token: 1, visible: false }, Stop(1)]);
        s.stopped(1);
        s.apply(&d, Then::Add(activity(3, 7, "Next", 0), None), None, true);
        assert_eq!(s.paused(2), [Launch(3)]);
        s.top_resumed_released(None);
        // Next idle: Pick, hidden, finishes with its result cancelled to
        // Main and is destroyed; Main stays stopped.
        assert_eq!(s.idle(3), [Destroy(2)]);
        assert_eq!((s.get(1).unwrap().state, s.get(1).unwrap().results.clone()), (State::Stopped, vec![42]));
        assert_eq!(s.destroyed(2), []);
        // Main gets it when it comes back.
        s.finish(3);
        assert_eq!(s.paused(3), [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![42], new_intents: vec![] }]);
    }

    #[test]
    fn document_modes_adjust_the_flags() {
        assert_eq!(document_flags(FLAG_ACTIVITY_NEW_DOCUMENT | FLAG_ACTIVITY_MULTIPLE_TASK, LAUNCH_SINGLE_TASK, 0), 0);
        assert_eq!(document_flags(0, 0, DOCUMENT_LAUNCH_ALWAYS), FLAG_ACTIVITY_NEW_DOCUMENT);
        assert_eq!(document_flags(FLAG_ACTIVITY_MULTIPLE_TASK, 0, DOCUMENT_LAUNCH_NEVER), 0);
        assert_eq!(document_flags(FLAG_ACTIVITY_MULTIPLE_TASK, LAUNCH_SINGLE_INSTANCE_PER_TASK, DOCUMENT_LAUNCH_NEVER), FLAG_ACTIVITY_MULTIPLE_TASK);
    }
}
