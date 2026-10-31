//! Same-task collection and native playback (#1725); Java owns animation resources.
use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Binder, Exception, Parcel};
use aim_host_display::animation::{self, Animation, IDENTITY};
use aim_service_aidl::dev_aim_server_iactivityanimations as java;

use super::AppHost;
use super::activity_stack::{Command, Stack};
use super::kept::{By, KeptBundle};

#[derive(Default)]
pub(super) struct Transitions {
    pub owner: Option<Arc<Strong>>,
    pub socket: Option<PathBuf>,
    collecting: HashMap<i32, Transition>,
    retiring: usize,
    next_id: u64,
    // Serialize retirement with frame submission, without nesting the windows/state locks.
    playback: Arc<Mutex<()>>,
}

struct Transition {
    id: u64,
    observer: Option<UnixStream>,
    from: u64,
    to: u64,
    open: bool,
    playing: bool,
    custom: Option<(String, i32, i32)>,
    options: Option<KeptBundle>,
    notified: bool,
    deferred: Vec<Command>,
}

impl Transition {
    fn new(from: u64, to: u64, open: bool) -> Self {
        Self {
            id: 0,
            observer: None,
            from,
            to,
            open,
            playing: false,
            custom: None,
            options: None,
            notified: false,
            deferred: Vec::new(),
        }
    }
    fn animation(&self, entering: bool) -> (i32, i32) {
        let attr = if self.open { 0 } else { 2 } + i32::from(!entering);
        (
            attr,
            self.custom.as_ref().map_or(
                -1,
                |(_, enter, exit)| {
                    if entering { *enter } else { *exit }
                },
            ),
        )
    }
}

impl Stack {
    pub(super) fn closing_pair(&self, token: u64) -> Option<(i32, u64)> {
        let a = self.get(token)?;
        if a.finishing || self.top(a.task)?.token != token {
            return None;
        }
        self.activities
            .iter()
            .rev()
            .find(|b| b.task == a.task && b.token != token && !b.finishing)
            .map(|b| (a.task, b.token))
    }
}

impl AppHost {
    fn collect_transition(self: &Arc<Self>, task: i32, mut next: Transition) {
        let playback = self.windows.transitions.lock().unwrap().playback.clone();
        let guard = playback.lock().unwrap();
        let old = {
            let mut state = self.windows.transitions.lock().unwrap();
            if let Some(t) = state.collecting.get_mut(&task)
                && !t.playing
                && t.open
                && next.open
                && t.to == 0
            {
                t.to = next.to;
                return;
            }
            let old = state.collecting.remove(&task);
            state.retiring += usize::from(old.is_some());
            old
        };
        // DefaultTransitionHandler.mergeAnimation ends the previous animators first.
        if let Some(t) = old.as_ref() {
            self.reset_transition(t);
        }
        {
            let mut state = self.windows.transitions.lock().unwrap();
            state.next_id += 1;
            next.id = state.next_id;
            state.collecting.insert(task, next);
        }
        drop(guard);
        if let Some(t) = old {
            self.release_transition(t);
            self.transition_retired();
        }
    }

    pub(super) fn collect_open(self: &Arc<Self>, task: Option<i32>, to: u64) {
        if !self.tasks.window_mode() {
            return;
        }
        let from = task.and_then(|task| {
            self.activities
                .stack
                .lock()
                .unwrap()
                .top(task)
                .map(|a| a.token)
        });
        if let (Some(task), Some(from)) = (task, from) {
            self.collect_transition(task, Transition::new(from, to, true));
        }
    }

    pub(super) fn collect_close(self: &Arc<Self>, token: u64) {
        if !self.tasks.window_mode() {
            return;
        }
        let pair = self.activities.stack.lock().unwrap().closing_pair(token);
        if let Some((task, to)) = pair {
            self.collect_transition(task, Transition::new(token, to, false));
        }
    }

    fn reset_transition(&self, t: &Transition) {
        if let Some(observer) = &t.observer {
            let _ = observer.shutdown(Shutdown::Both);
        }
        if t.notified {
            self.notify_transition(t, false);
        }
        let mut exit = IDENTITY;
        if !t.deferred.is_empty() {
            exit[6] = 0.;
        }
        if let Err(e) = self.animate_activities(t.to, IDENTITY, t.from, exit) {
            eprintln!("app host: transition finish: {e}");
        }
    }

    fn notify_transition(&self, t: &Transition, started: bool) {
        let owner = self.windows.transitions.lock().unwrap().owner.clone();
        if let (Some(owner), Some(options)) = (owner, &t.options) {
            let mut p = Parcel::new();
            java::NotifyAnimation {
                options: Some(By(&options.bundle)),
                started,
            }
            .write(&mut p);
            if let Err(e) = owner.transact(java::NOTIFY_ANIMATION, &p, true) {
                eprintln!("app host: animation callback: {e}");
            }
        }
    }

    fn release_transition(self: &Arc<Self>, t: Transition) {
        // Already retired: never collect these commands again in a newer transition.
        for command in t.deferred {
            if let Err(e) = self.run_one(command) {
                eprintln!("app host: transition finish: {e}");
            }
        }
    }

    fn finish_transition(self: &Arc<Self>, task: i32, id: u64) {
        let playback = self.windows.transitions.lock().unwrap().playback.clone();
        let guard = playback.lock().unwrap();
        let old = {
            let mut state = self.windows.transitions.lock().unwrap();
            if !state.collecting.get(&task).is_some_and(|t| t.id == id) {
                return;
            }
            state.retiring += 1;
            state.collecting.remove(&task).unwrap()
        };
        self.reset_transition(&old);
        drop(guard);
        self.release_transition(old);
        self.transition_retired();
    }

    fn transition_retired(&self) {
        self.windows.transitions.lock().unwrap().retiring -= 1;
        self.windows.transitions_changed.notify_all();
    }

    /// WMS.waitForAnimationsToComplete, android-16.0.0_r1. Collection,
    /// playback and final reset/deferred window operations must all finish.
    pub(super) fn wait_for_window_animations(&self, timeout: std::time::Duration) -> bool {
        let (state, _) = self
            .windows
            .transitions_changed
            .wait_timeout_while(self.windows.transitions.lock().unwrap(), timeout, |s| {
                !s.collecting.is_empty() || s.retiring != 0
            })
            .unwrap();
        let complete = state.collecting.is_empty() && state.retiring == 0;
        if !complete {
            eprintln!(
                "app host: Timed out waiting for animations to complete, collecting={} retiring={}",
                state.collecting.len(),
                state.retiring
            );
        }
        complete
    }

    pub(super) fn cancel_activity_transition(&self, token: u64) {
        let work: Vec<_> = self
            .windows
            .transitions
            .lock()
            .unwrap()
            .collecting
            .iter()
            .filter(|(_, t)| t.from == token || t.to == token)
            .map(|(&task, t)| (task, t.id))
            .collect();
        if let Some(host) = self.me.upgrade() {
            for (task, id) in work {
                host.finish_transition(task, id);
            }
        }
    }

    fn transition_frame(
        &self,
        task: i32,
        id: u64,
        to: u64,
        enter: [f32; 11],
        from: u64,
        exit: [f32; 11],
    ) -> Result<bool, String> {
        let playback = self.windows.transitions.lock().unwrap().playback.clone();
        let _guard = playback.lock().unwrap();
        if !self
            .windows
            .transitions
            .lock()
            .unwrap()
            .collecting
            .get(&task)
            .is_some_and(|t| t.id == id)
        {
            return Ok(false);
        }
        self.animate_activities(to, enter, from, exit)?;
        Ok(true)
    }

    pub(super) fn override_activity_transition(
        &self,
        uid: u32,
        token: Option<Binder>,
        open: bool,
        custom: Option<(i32, i32, i32)>,
    ) -> Result<(), Exception> {
        let Some(Binder::Local(token)) = token else {
            return Ok(());
        };
        let mut records = self.activities.records.lock().unwrap();
        let Some(r) = records.get_mut(&token).filter(|r| r.uid == uid) else {
            return Ok(());
        };
        if custom.is_some_and(|(_, _, background)| background != 0) {
            return Err(
                self.unimplemented_named("activity_task", "transition background color (#1725)")
            );
        }
        r.transition_overrides[usize::from(!open)] =
            custom.map(|(enter, exit, _)| (r.package.clone(), enter, exit));
        Ok(())
    }

    pub(super) fn collect_open_with_options(
        self: &Arc<Self>,
        task: Option<i32>,
        options: Option<&super::kept::RawBundle>,
    ) -> Result<(), Exception> {
        let Some(options) = options else {
            self.collect_open(task, 0);
            return Ok(());
        };
        if options.has_fds() {
            return Err(Exception::illegal_argument(
                "File descriptors passed in options",
            ));
        }
        let bad = |_| Exception::illegal_argument("ActivityOptions animation bundle");
        // ActivityOptions.fromBundle, ANIM_CUSTOM at android-16.0.0_r1.
        if options.int("android:activity.animType", -1).map_err(bad)? != 1 {
            self.collect_open(task, 0);
            return Ok(());
        }
        let package = options
            .string("android:activity.packageName")
            .map_err(bad)?;
        let enter = options
            .int("android:activity.animEnterRes", 0)
            .map_err(bad)?;
        let exit = options
            .int("android:activity.animExitRes", 0)
            .map_err(bad)?;
        if options
            .int("android:activity.backgroundColor", 0)
            .map_err(bad)?
            != 0
        {
            return Err(
                self.unimplemented_named("activity_task", "transition background color (#1725)")
            );
        }
        let kept = KeptBundle::new(options.clone(), &self.process).map_err(bad)?;
        self.collect_open(task, 0);
        let mut state = self.windows.transitions.lock().unwrap();
        if let Some(t) = task.and_then(|task| state.collecting.get_mut(&task)) {
            t.custom = package.map(|p| (p, enter, exit));
            t.options = Some(kept);
        }
        Ok(())
    }

    pub(super) fn override_transition(
        &self,
        uid: u32,
        token: Option<Binder>,
        package: Option<String>,
        enter: i32,
        exit: i32,
        background: i32,
    ) -> Result<(), Exception> {
        let Some(Binder::Local(token)) = token else {
            return Ok(());
        };
        let allowed = self
            .activities
            .records
            .lock()
            .unwrap()
            .get(&token)
            .is_some_and(|r| r.uid == uid);
        if !allowed {
            return Ok(());
        }
        let mut state = self.windows.transitions.lock().unwrap();
        if let Some(t) = state
            .collecting
            .values_mut()
            .find(|t| !t.playing && (t.from == token || t.to == token))
        {
            if background != 0 {
                return Err(self
                    .unimplemented_named("activity_task", "transition background color (#1725)"));
            }
            t.custom = package.map(|pkg| (pkg, enter, exit));
        }
        Ok(())
    }

    pub(super) fn hold_transition(&self, command: &Command) -> bool {
        let token = match command {
            Command::Visible {
                token,
                visible: false,
            }
            | Command::Stop(token)
            | Command::Destroy(token) => *token,
            _ => return false,
        };
        let mut state = self.windows.transitions.lock().unwrap();
        if let Some(t) = state.collecting.values_mut().find(|t| t.from == token) {
            t.deferred.push(command.clone());
            return true;
        }
        false
    }

    pub(super) fn transition_entering(&self, token: Option<u64>) -> bool {
        self.windows
            .transitions
            .lock()
            .unwrap()
            .collecting
            .values()
            .any(|t| !t.playing && Some(t.to) == token)
    }

    pub(super) fn transition_ready(self: &Arc<Self>, token: u64) {
        let work = {
            let mut state = self.windows.transitions.lock().unwrap();
            let Some((&task, t)) = state
                .collecting
                .iter_mut()
                .find(|(_, t)| t.to == token && !t.playing)
            else {
                return;
            };
            t.playing = true;
            (
                task,
                t.id,
                t.from,
                t.to,
                t.open,
                t.custom.clone(),
                state.owner.clone(),
                state.socket.clone(),
            )
        };
        let host = self.clone();
        std::thread::spawn(move || {
            let (task, id, from, to, open, custom, owner, socket) = work;
            let play = || -> Result<(), String> {
                let owner = owner.ok_or("activity animation Java owner unavailable")?;
                let socket = socket.ok_or("activity animation display unavailable")?;
                let (package, theme, user, activity_override) = {
                    let records = host.activities.records.lock().unwrap();
                    let r = records
                        .get(&if open { to } else { from })
                        .ok_or("animation activity gone")?;
                    let app = &r.info.info.application_info;
                    (
                        r.package.clone(),
                        if r.info.theme != 0 {
                            r.info.theme
                        } else {
                            app.theme
                        },
                        (r.uid / 100000) as i32,
                        r.transition_overrides[usize::from(!open)].clone(),
                    )
                };
                let bounds = host.tasks.bounds(task).ok_or("animation task gone")?;
                let style = host.activity_animation_style(if open { to } else { from });
                let spec = Transition {
                    id,
                    observer: None,
                    from,
                    to,
                    open,
                    playing: true,
                    custom: custom.or(activity_override),
                    options: None,
                    notified: false,
                    deferred: Vec::new(),
                };
                let load = |entering| -> Result<Animation, String> {
                    let (attribute, custom) = spec.animation(entering);
                    let mut p = Parcel::new();
                    java::Sample {
                        package_name: Some(spec.custom.as_ref().map_or(&package, |c| &c.0).clone()),
                        user_id: user,
                        theme,
                        style,
                        attribute,
                        custom,
                        width: bounds.width(),
                        height: bounds.height(),
                    }
                    .write(&mut p);
                    let reply = owner
                        .transact(java::SAMPLE, &p, false)
                        .map_err(|e| e.to_string())?;
                    let samples = java::read_sample_reply(&mut reply.reader())
                        .map_err(|e| e.to_string())?
                        .map_err(|e| format!("{e:?}"))?
                        .ok_or("null animation samples")?;
                    Animation::new(samples).map_err(|e| e.to_string())
                };
                let (enter, exit) = (load(true)?, load(false)?);
                {
                    let playback = host.windows.transitions.lock().unwrap().playback.clone();
                    let _guard = playback.lock().unwrap();
                    let options = {
                        let mut state = host.windows.transitions.lock().unwrap();
                        let Some(t) = state.collecting.get_mut(&task).filter(|t| t.id == id) else {
                            return Ok(());
                        };
                        t.notified = true;
                        t.options.clone()
                    };
                    let mut spec = spec;
                    spec.options = options;
                    host.notify_transition(&spec, true);
                }
                let stream = animation::vsyncs(&socket).map_err(|e| e.to_string())?;
                {
                    let mut state = host.windows.transitions.lock().unwrap();
                    let Some(t) = state.collecting.get_mut(&task).filter(|t| t.id == id) else {
                        return Ok(());
                    };
                    t.observer = Some(stream.try_clone().map_err(|e| e.to_string())?);
                }
                let start = animation::next_vsync(&stream).map_err(|e| e.to_string())?;
                let mut now = start;
                loop {
                    let (a, more_a) = enter.at(now - start);
                    let (b, more_b) = exit.at(now - start);
                    if !host.transition_frame(task, id, to, a, from, b)? {
                        break;
                    }
                    if !more_a && !more_b {
                        break;
                    }
                    now = animation::next_vsync(&stream).map_err(|e| e.to_string())?;
                }
                Ok(())
            };
            if let Err(e) = play()
                && host
                    .windows
                    .transitions
                    .lock()
                    .unwrap()
                    .collecting
                    .get(&task)
                    .is_some_and(|t| t.id == id)
            {
                eprintln!("app host: activity transition: {e}");
            }
            host.finish_transition(task, id);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn open_close_and_pending_override_choose_both_animations() {
        let mut t = Transition {
            id: 0,
            observer: None,
            from: 1,
            to: 2,
            open: true,
            playing: false,
            custom: None,
            options: None,
            notified: false,
            deferred: Vec::new(),
        };
        assert_eq!((t.animation(true), t.animation(false)), ((0, -1), (1, -1)));
        t.open = false;
        assert_eq!((t.animation(true), t.animation(false)), ((2, -1), (3, -1)));
        t.custom = Some(("p".into(), 100, 101));
        assert_eq!(
            (t.animation(true), t.animation(false)),
            ((2, 100), (3, 101))
        );
    }
    #[test]
    fn collection_keeps_the_exit_until_ready_and_excludes_task_windows() {
        use super::super::activity_stack::Activity;
        let host = AppHost::new(crate::test_process(1000));
        host.tasks.windows.set(true).unwrap();
        let a = |token, task| Activity::new(token, task, 1, "p/A", 0, 0, None);
        host.activities.stack.lock().unwrap().activities = vec![a(1, 7), a(3, 8)];
        host.collect_open(None, 9); // A new task has only the Mac animation.
        assert!(
            host.windows
                .transitions
                .lock()
                .unwrap()
                .collecting
                .is_empty()
        );
        host.collect_open(Some(7), 0); // Collection precedes process attach.
        host.collect_open(Some(7), 2);
        assert!(host.hold_transition(&Command::Stop(1)));
        assert!(!host.hold_transition(&Command::Stop(3)));
        assert!(host.transition_entering(Some(2)));
        {
            let mut state = host.windows.transitions.lock().unwrap();
            let t = state.collecting.get_mut(&7).unwrap();
            assert_eq!((t.from, t.to, t.open), (1, 2, true));
            assert_eq!(t.deferred, [Command::Stop(1)]);
            t.playing = true;
        }
        assert!(!host.transition_entering(Some(2)));
        host.windows.transitions.lock().unwrap().collecting.clear();
        host.activities
            .stack
            .lock()
            .unwrap()
            .activities
            .push(a(2, 7));
        host.collect_close(3); // The last activity closes its Mac window.
        assert!(
            host.windows
                .transitions
                .lock()
                .unwrap()
                .collecting
                .is_empty()
        );
        host.collect_close(2); // Back returns to A within the same window.
        let state = host.windows.transitions.lock().unwrap();
        let t = state.collecting.get(&7).unwrap();
        assert_eq!((t.from, t.to, t.open), (2, 1, false));
    }

    #[test]
    fn pending_override_requires_a_participant_and_only_changes_collection() {
        use super::super::{activity_stack::Activity, activity_start::StartRequest};
        crate::system::with_discovery_session(|host, _, _| {
            host.tasks.windows.set(true).unwrap();
            let info = host.activity_info_of("p", "p.Main").unwrap();
            let token = host.new_activity(
                1,
                10100,
                "p",
                info,
                Default::default(),
                7,
                StartRequest::launcher("p.Main"),
            );
            host.activities
                .stack
                .lock()
                .unwrap()
                .activities
                .push(Activity::new(token, 7, 1, "p/p.Main", 0, 0, None));
            let override_with = |uid, enter| {
                host.override_transition(
                    uid,
                    Some(Binder::Local(token)),
                    Some("p".into()),
                    enter,
                    102,
                    0,
                )
                .unwrap()
            };
            override_with(10100, 100); // Nothing is collecting yet.
            host.collect_open(Some(7), 99);
            override_with(10101, 100);
            assert!(
                host.windows.transitions.lock().unwrap().collecting[&7]
                    .custom
                    .is_none()
            );
            override_with(10100, 100);
            {
                let mut state = host.windows.transitions.lock().unwrap();
                let t = state.collecting.get_mut(&7).unwrap();
                assert_eq!(t.animation(true), (0, 100));
                assert_eq!(t.animation(false), (1, 102));
                t.playing = true;
            }
            override_with(10100, 200);
            assert_eq!(
                host.windows.transitions.lock().unwrap().collecting[&7].animation(true),
                (0, 100)
            );
        });
    }
}

#[cfg(test)]
#[path = "activity_transition_tests.rs"]
mod playback_tests;
