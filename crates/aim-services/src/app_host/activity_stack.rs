//! The activities of the app host's tasks and their lifecycle, as
//! ActivityTaskManager moves them at `android-16.0.0_r1`
//! (`TaskFragment.resumeTopActivity`, `ActivityRecord.finishIfPossible`
//! and `completeFinishing`, `ActivityTaskSupervisor.activityIdleInternal`):
//! the top activity of a task is resumed once the one resumed before it
//! has paused; an activity under the top is stopped once the top is idle;
//! a finishing activity is paused if resumed, then destroyed once the
//! activity under it is resumed and idle, its result delivered to the
//! activity that asked for it. The decisions are [`Command`]s the app host
//! sends as client transactions.

/// `ActivityRecord.State`, of the ones the app host sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Made, not yet launched in its process.
    Initializing,
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
    pub state: State,
    pub finishing: bool,
    /// The activity its result goes to (`resultTo`).
    pub result_to: Option<u64>,
    /// Results and new intents to deliver at its next resume.
    pub results: Vec<Payload>,
    pub new_intents: Vec<Payload>,
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
    /// `ActivityResultItem` to a resumed activity (of another task).
    Result { token: u64, results: Vec<Payload> },
    Stop(u64),
    /// `DestroyActivityItem`, finishing.
    Destroy(u64),
    TopResumed { token: u64, on_top: bool },
    /// Its windows shown or hidden (`dispatchAppVisibility`).
    Visible { token: u64, visible: bool },
    /// A task without activities.
    TaskEmpty(i32),
}

/// The activities, bottom to top, of all tasks.
#[derive(Default)]
pub struct Stack {
    pub activities: Vec<Activity>,
}

impl Stack {
    pub fn get(&self, token: u64) -> Option<&Activity> {
        self.activities.iter().find(|a| a.token == token)
    }

    fn get_mut(&mut self, token: u64) -> Option<&mut Activity> {
        self.activities.iter_mut().find(|a| a.token == token)
    }

    /// The top activity of `task` that is not finishing.
    pub fn top(&self, task: i32) -> Option<&Activity> {
        self.activities.iter().rev().find(|a| a.task == task && !a.finishing)
    }

    /// `startActivityInner` of a new activity on top of `task`: the one
    /// resumed there pauses first (the user leaves it).
    pub fn start(&mut self, token: u64, task: i32, start_seq: i64, result_to: Option<u64>) -> Vec<Command> {
        self.activities.push(Activity {
            token,
            task,
            start_seq,
            state: State::Initializing,
            finishing: false,
            result_to,
            results: Vec::new(),
            new_intents: Vec::new(),
        });
        self.resume_top(task, true)
    }

    /// `TaskFragment.resumeTopActivity`: pause another resumed activity of
    /// the task first, else launch or resume the top one.
    fn resume_top(&mut self, task: i32, user_leaving: bool) -> Vec<Command> {
        let Some(top) = self.top(task).map(|a| a.token) else {
            return if self.activities.iter().any(|a| a.task == task) { Vec::new() } else { vec![Command::TaskEmpty(task)] };
        };
        let mut out = Vec::new();
        for a in self.activities.iter_mut().filter(|a| a.task == task && a.token != top) {
            match a.state {
                State::Resumed => {
                    a.state = State::Pausing;
                    out.push(Command::TopResumed { token: a.token, on_top: false });
                    out.push(Command::Pause { token: a.token, finishing: a.finishing, user_leaving });
                }
                State::Pausing => {}
                _ => continue,
            }
        }
        if !out.is_empty() || self.activities.iter().any(|a| a.task == task && a.state == State::Pausing) {
            return out;
        }
        let a = self.get_mut(top).unwrap();
        match a.state {
            State::Initializing => {
                a.state = State::Resumed;
                out.push(Command::Launch(top));
                out.push(Command::TopResumed { token: top, on_top: true });
            }
            State::Paused | State::Stopping | State::Stopped => {
                a.state = State::Resumed;
                let (results, new_intents) = (std::mem::take(&mut a.results), std::mem::take(&mut a.new_intents));
                out.push(Command::Visible { token: top, visible: true });
                out.push(Command::Resume { token: top, results, new_intents });
                out.push(Command::TopResumed { token: top, on_top: true });
            }
            _ => {}
        }
        out
    }

    /// `activityPaused`.
    pub fn paused(&mut self, token: u64) -> Vec<Command> {
        let Some(a) = self.get_mut(token) else { return Vec::new() };
        a.state = State::Paused;
        let task = a.task;
        self.resume_top(task, false)
    }

    /// `activityIdleInternal` of the top of a task: what is under it is
    /// stopped, what is finishing destroyed.
    pub fn idle(&mut self, token: u64) -> Vec<Command> {
        let Some(task) = self.get(token).map(|a| a.task) else { return Vec::new() };
        if self.top(task).map(|a| a.token) != Some(token) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for a in self.activities.iter_mut().filter(|a| a.task == task && a.token != token) {
            match (a.finishing, a.state) {
                (true, State::Paused | State::Stopping | State::Stopped) => {
                    a.state = State::Destroying;
                    out.push(Command::Destroy(a.token));
                }
                (false, State::Paused) => {
                    a.state = State::Stopping;
                    out.push(Command::Visible { token: a.token, visible: false });
                    out.push(Command::Stop(a.token));
                }
                _ => {}
            }
        }
        out
    }

    /// `activityStopped`.
    pub fn stopped(&mut self, token: u64) {
        if let Some(a) = self.get_mut(token)
            && a.state == State::Stopping
        {
            a.state = State::Stopped;
        }
    }

    /// `activityDestroyed`: it goes; a task left without activities is
    /// empty.
    pub fn destroyed(&mut self, token: u64) -> Vec<Command> {
        let Some(at) = self.activities.iter().position(|a| a.token == token) else { return Vec::new() };
        let task = self.activities.remove(at).task;
        if self.activities.iter().any(|a| a.task == task) { Vec::new() } else { vec![Command::TaskEmpty(task)] }
    }

    /// `finishIfPossible` of `token`, with its result (`result`) for the
    /// activity that asked for it: whether it finishes.
    pub fn finish(&mut self, token: u64, result: Option<Payload>) -> (bool, Vec<Command>) {
        let Some(a) = self.get_mut(token) else { return (false, Vec::new()) };
        if a.finishing {
            return (true, Vec::new());
        }
        a.finishing = true;
        let (task, state, result_to) = (a.task, a.state, a.result_to);
        let mut out = Vec::new();
        // `sendResult`: at once to a resumed activity, else at its resume.
        if let (Some(to), Some(result)) = (result_to, result)
            && let Some(r) = self.get_mut(to)
        {
            if r.state == State::Resumed {
                out.push(Command::Result { token: to, results: vec![result] });
            } else {
                r.results.push(result);
            }
        }
        match state {
            State::Resumed => {
                let a = self.get_mut(token).unwrap();
                a.state = State::Pausing;
                out.push(Command::TopResumed { token, on_top: false });
                out.push(Command::Pause { token, finishing: true, user_leaving: false });
            }
            State::Pausing | State::Destroying => {}
            // Never launched: it just goes.
            State::Initializing => {
                self.activities.retain(|a| a.token != token);
                out.extend(self.resume_top(task, false));
            }
            State::Paused | State::Stopping | State::Stopped => {
                self.get_mut(token).unwrap().state = State::Destroying;
                out.push(Command::Destroy(token));
                out.extend(self.resume_top(task, false));
            }
        }
        (true, out)
    }

    /// `deliverNewIntentLocked` to `token`: now if it is resumed, else at
    /// its next resume.
    pub fn new_intent(&mut self, token: u64, intent: Payload) -> Vec<Command> {
        let Some(a) = self.get_mut(token) else { return Vec::new() };
        if a.state == State::Resumed {
            vec![Command::NewIntent { token, intents: vec![intent] }]
        } else {
            a.new_intents.push(intent);
            Vec::new()
        }
    }

    /// The activities above `token` in its task finish
    /// (`FLAG_ACTIVITY_CLEAR_TOP`), and with them `token` itself if
    /// `inclusive`.
    pub fn clear_above(&mut self, token: u64, inclusive: bool) -> Vec<Command> {
        let Some(at) = self.activities.iter().position(|a| a.token == token) else { return Vec::new() };
        let task = self.activities[at].task;
        let above: Vec<u64> = self.activities[at + usize::from(!inclusive)..].iter().filter(|a| a.task == task && !a.finishing).map(|a| a.token).collect();
        let mut out = Vec::new();
        for t in above.into_iter().rev() {
            out.extend(self.finish(t, None).1);
        }
        out
    }

    /// The process of `start_seq` died: its activities go.
    pub fn process_died(&mut self, start_seq: i64) {
        self.activities.retain(|a| a.start_seq != start_seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Command::*;

    #[test]
    fn a_started_activity_resumes_once_the_one_under_it_paused() {
        let mut s = Stack::default();
        assert_eq!(s.start(1, 7, 1, None), [Launch(1), TopResumed { token: 1, on_top: true }]);
        // B on A: A pauses first.
        assert_eq!(s.start(2, 7, 1, Some(1)), [TopResumed { token: 1, on_top: false }, Pause { token: 1, finishing: false, user_leaving: true }]);
        assert_eq!(s.paused(1), [Launch(2), TopResumed { token: 2, on_top: true }]);
        // B idle: A is stopped.
        assert_eq!(s.idle(2), [Visible { token: 1, visible: false }, Stop(1)]);
        s.stopped(1);
        assert_eq!(s.get(1).unwrap().state, State::Stopped);
    }

    #[test]
    fn a_finishing_activity_hands_its_result_back_and_is_destroyed() {
        let mut s = Stack::default();
        s.start(1, 7, 1, None);
        s.start(2, 7, 1, Some(1));
        s.paused(1);
        s.idle(2);
        s.stopped(1);
        let (finished, out) = s.finish(2, Some(42));
        assert!(finished);
        assert_eq!(out, [TopResumed { token: 2, on_top: false }, Pause { token: 2, finishing: true, user_leaving: false }]);
        // B paused: A resumes with B's result.
        assert_eq!(
            s.paused(2),
            [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![42], new_intents: vec![] }, TopResumed { token: 1, on_top: true }]
        );
        assert_eq!(s.idle(1), [Destroy(2)]);
        assert_eq!(s.destroyed(2), []);
        // A finishes too: the task is left empty.
        s.finish(1, None);
        s.paused(1);
        assert_eq!(s.destroyed(1), [TaskEmpty(7)]);
    }

    #[test]
    fn a_new_intent_reaches_a_resumed_activity_now_else_at_its_resume() {
        let mut s = Stack::default();
        s.start(1, 7, 1, None);
        assert_eq!(s.new_intent(1, 5), [NewIntent { token: 1, intents: vec![5] }]);
        s.start(2, 7, 1, None);
        s.paused(1);
        assert_eq!(s.new_intent(1, 6), []);
        // Clearing the top brings A back with its intent.
        let out = s.clear_above(1, false);
        assert_eq!(out, [TopResumed { token: 2, on_top: false }, Pause { token: 2, finishing: true, user_leaving: false }]);
        assert_eq!(
            s.paused(2),
            [Visible { token: 1, visible: true }, Resume { token: 1, results: vec![], new_intents: vec![6] }, TopResumed { token: 1, on_top: true }]
        );
    }

    #[test]
    fn a_result_reaches_a_resumed_activity_of_another_task_at_once() {
        let mut s = Stack::default();
        s.start(1, 7, 1, None);
        s.start(2, 8, 2, Some(1));
        assert_eq!(s.finish(2, Some(9)).1[0], Result { token: 1, results: vec![9] });
    }

    #[test]
    fn tasks_move_apart() {
        let mut s = Stack::default();
        s.start(1, 7, 1, None);
        // Another task's start pauses nothing of this one.
        assert_eq!(s.start(2, 8, 2, None), [Launch(2), TopResumed { token: 2, on_top: true }]);
        s.process_died(2);
        assert!(s.get(2).is_none() && s.get(1).is_some());
    }
}
