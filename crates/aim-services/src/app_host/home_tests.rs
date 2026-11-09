use super::super::activity_stack::{
    Activity, Command, Decision, Outcome, Stack, State, Task, Then, Visibility,
};
use super::super::activity_start::LaunchIntent;
use super::*;

/// The root home task's id, as the task owner gives it.
const HOME_ROOT_TASK: i32 = 1;

fn task(id: i32, component: &str) -> Box<Task> {
    Box::new(Task {
        id,
        affinity: None,
        real_activity: component.into(),
        package: component.split_once('/').unwrap().0.into(),
        intent: Default::default(),
        base: LaunchIntent::Component,
        uid: 10_100,
        user: 0,
        last_active: 0,
    })
}

/// `startHomeActivity` of `h/h.Home` (token 1) into the root home task,
/// then an app's start (token 2) into task 7.
fn start_home_then_app(freeform: bool) -> (Stack, Vec<Command>, Vec<Command>) {
    let mut s = Stack::default();
    s.freeform = freeform;
    s.home = Some(HOME_ROOT_TASK);
    let new = Decision {
        outcome: Outcome::New(None),
        ..Default::default()
    };
    let home = Activity::new(1, HOME_ROOT_TASK, 1, "h/h.Home", 2, 0, None);
    let home = s.apply(
        &new,
        Then::Add(home, Some(task(HOME_ROOT_TASK, "h/h.Home"))),
        None,
        false,
    );
    let app = Activity::new(2, 7, 2, "p/p.Main", 0, 0, None);
    let app = s.apply(&new, Then::Add(app, Some(task(7, "p/p.Main"))), None, true);
    (s, home, app)
}

#[test]
fn home_is_launched_stopped_behind_the_desktop_in_window_mode() {
    let (mut s, home, app) = start_home_then_app(true);
    // `realStartActivityLocked` neither resumes nor shows it.
    assert_eq!(home, [Command::LaunchStopped(1)]);
    assert_eq!(s.get(1).unwrap().state, State::Stopping);
    assert_eq!(s.get(1).unwrap().launch_count, 1);
    s.stopped(1, false);
    assert_eq!(s.get(1).unwrap().state, State::Stopped);
    // The app's task opens over it; it is never paused, resumed or focused.
    assert!(app.starts_with(&[Command::Launch(2), Command::Front(7)]));
    assert!(!app.iter().any(|c| matches!(
        c,
        Command::Pause { token: 1, .. }
            | Command::Resume { token: 1, .. }
            | Command::TopResumed { token: 1, .. }
    )));
    assert_eq!(s.visibility(HOME_ROOT_TASK), Visibility::Invisible);
    assert_eq!(s.focused(), Some(7));
    let order = |s: &Stack| s.tasks.iter().map(|t| t.id).collect::<Vec<_>>();
    assert_eq!(order(&s), [HOME_ROOT_TASK, 7]);
    // A task moved back goes below it; then nothing is focused.
    let (moved, commands) = s.move_to_back(2, false);
    assert!(moved);
    assert_eq!(order(&s), [7, HOME_ROOT_TASK]);
    assert_eq!(s.focused(), None);
    assert!(!commands.iter().any(|c| matches!(
        c,
        Command::Launch(1) | Command::Resume { token: 1, .. } | Command::Front(HOME_ROOT_TASK)
    )));
}

/// The home intent's start of the running home (`singleTask`), by token
/// 2's app: `startActivityInner` brings the root home task to the front and
/// delivers the intent (payload 5).
fn start_home_intent(s: &mut Stack) -> Vec<Command> {
    use crate::app_host::activity_stack::{FLAG_ACTIVITY_NEW_TASK, START_TASK_TO_FRONT, Start};
    let intent = crate::package::intent::Intent {
        flags: FLAG_ACTIVITY_NEW_TASK,
        ..Default::default()
    };
    let d = s
        .decide(&Start {
            component: "h/h.Home",
            affinity: Some("h"),
            launch_mode: 2,
            document_launch_mode: 0,
            intent: &intent,
            user: 0,
            source: Some(2),
            result_to: None,
            activity_type: ACTIVITY_TYPE_HOME,
        })
        .unwrap();
    assert_eq!(d.front, Some(HOME_ROOT_TASK));
    assert_eq!(d.outcome, Outcome::Deliver(1, START_TASK_TO_FRONT));
    s.apply(&d, Then::Deliver(1, 5), None, true)
}

#[test]
fn a_home_intent_in_window_mode_hides_the_front_app_and_resumes_the_home() {
    let (mut s, _, _) = start_home_then_app(true);
    s.stopped(1, false);
    // Another task of the app, and one of another app, which it is in front of.
    let new = Decision::default();
    let other = Activity::new(3, 9, 3, "q/q.Main", 0, 0, None);
    s.apply(
        &new,
        Then::Add(other, Some(task(9, "q/q.Main"))),
        None,
        true,
    );
    let second = Activity::new(4, 8, 4, "p/p.Other", 0, 0, None);
    s.apply(
        &new,
        Then::Add(second, Some(task(8, "p/p.Other"))),
        None,
        true,
    );
    s.bring_to_front(7);
    assert_eq!(s.focused(), Some(7));
    let commands = start_home_intent(&mut s);
    // The desktop is in front: the app's tasks go behind it and pause, and
    // the server hides the app; the other app's task stays resumed.
    assert_eq!(s.focused(), Some(HOME_ROOT_TASK));
    assert!(commands.contains(&Command::Hide(7)));
    for token in [2, 4] {
        assert!(commands.contains(&Command::Pause {
            token,
            finishing: false,
            user_leaving: true,
        }));
    }
    assert_eq!(s.visibility(7), Visibility::Invisible);
    assert_eq!(s.visibility(8), Visibility::Invisible);
    assert_eq!(s.visibility(9), Visibility::Visible);
    assert_eq!(s.get(3).unwrap().state, State::Resumed);
    // The home is resumed with the intent once they paused, and they stop.
    let mut commands = s.paused(2);
    commands.extend(s.paused(4));
    // It is told it is the top resumed one once the app's lost it.
    commands.extend(s.top_resumed_released(None));
    assert!(commands.contains(&Command::Resume {
        token: 1,
        results: Vec::new(),
        new_intents: vec![5],
    }));
    assert!(commands.contains(&Command::Stop(2)));
    assert!(commands.contains(&Command::Stop(4)));
    assert!(commands.contains(&Command::TopResumed {
        token: 1,
        on_top: true,
    }));
    assert_eq!(s.visibility(HOME_ROOT_TASK), Visibility::Visible);
    // Its window back from the Dock brings the app's tasks back, as the
    // app's windows all show: the home goes behind the desktop again,
    // paused, then stopped.
    let commands = s.bring_to_front(7);
    assert_eq!(s.visibility(8), Visibility::Visible);
    assert!(commands.contains(&Command::Pause {
        token: 1,
        finishing: false,
        user_leaving: true,
    }));
    assert!(!commands.iter().any(|c| matches!(c, Command::Hide(_))));
    assert_eq!(s.focused(), Some(7));
    let commands = s.paused(1);
    assert!(commands.contains(&Command::Stop(1)));
    for token in [2, 4] {
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, Command::Resume { token: t, .. } if *t == token))
        );
    }
    assert_eq!(s.visibility(HOME_ROOT_TASK), Visibility::Invisible);
}

#[test]
fn a_new_task_puts_the_home_behind_the_desktop_again() {
    let (mut s, _, _) = start_home_then_app(true);
    start_home_intent(&mut s);
    s.paused(2);
    assert_eq!(s.focused(), Some(HOME_ROOT_TASK));
    let app = Activity::new(3, 9, 3, "q/q.Main", 0, 0, None);
    let commands = s.apply(
        &Decision::default(),
        Then::Add(app, Some(task(9, "q/q.Main"))),
        None,
        true,
    );
    assert_eq!(s.focused(), Some(9));
    assert!(commands.contains(&Command::Pause {
        token: 1,
        finishing: false,
        user_leaving: true,
    }));
    assert_eq!(s.visibility(HOME_ROOT_TASK), Visibility::Invisible);
}

#[test]
fn a_home_finished_in_front_restarts_stopped_behind_the_desktop() {
    let (mut s, _, _) = start_home_then_app(true);
    start_home_intent(&mut s);
    s.paused(2);
    assert_eq!(s.focused(), Some(HOME_ROOT_TASK));
    // A second activity keeps the task while the home finishes.
    let second = Activity::new(3, HOME_ROOT_TASK, 1, "h/h.Other", 0, 0, None);
    s.apply(&Decision::default(), Then::Add(second, None), None, false);
    s.finish(3);
    s.finish(1);
    assert_eq!(s.focused(), None);
    // `resumeHomeActivity` starts it again in its root task.
    let home = Activity::new(5, HOME_ROOT_TASK, 1, "h/h.Home", 2, 0, None);
    let d = Decision {
        outcome: Outcome::New(Some(HOME_ROOT_TASK)),
        ..Default::default()
    };
    let commands = s.apply(&d, Then::Add(home, None), None, false);
    assert!(commands.contains(&Command::LaunchStopped(5)));
    assert_eq!(s.visibility(HOME_ROOT_TASK), Visibility::Invisible);
}

#[test]
fn home_is_resumed_on_a_fullscreen_display_until_an_app_covers_it() {
    let (s, home, app) = start_home_then_app(false);
    assert_eq!(home[0], Command::Launch(1));
    assert!(home.contains(&Command::Front(HOME_ROOT_TASK)));
    assert!(app.contains(&Command::Pause {
        token: 1,
        finishing: false,
        user_leaving: true,
    }));
    assert_eq!(s.visibility(HOME_ROOT_TASK), Visibility::Invisible);
    assert_eq!(s.focused(), Some(7));
}

#[test]
fn the_home_intent_names_main_and_home() {
    let intent = home_intent().unwrap();
    assert_eq!(intent.intent.action.as_deref(), Some(ACTION_MAIN));
    assert_eq!(
        intent.intent.categories.as_deref(),
        Some(&[CATEGORY_HOME.to_string()][..])
    );
    assert_eq!(intent.intent.flags, FLAG_DEBUG_TRIAGED_MISSING);
    assert!(intent.intent.component.is_none());
}

#[test]
fn a_standard_start_never_lands_in_the_root_home_task() {
    use crate::app_host::activity::ACTIVITY_TYPE_STANDARD;
    use crate::package::intent::Intent;
    let (mut s, _, _) = start_home_then_app(true);
    // The root home task of the launcher's default affinity.
    s.tasks[0].affinity = Some("h".into());
    let new_task = Intent {
        flags: crate::app_host::activity_stack::FLAG_ACTIVITY_NEW_TASK,
        ..Default::default()
    };
    let start = |component, affinity, activity_type| crate::app_host::activity_stack::Start {
        component,
        affinity: Some(affinity),
        launch_mode: 0,
        document_launch_mode: 0,
        intent: &new_task,
        user: 0,
        source: None,
        result_to: None,
        activity_type,
    };
    // `FindTaskResult` skips a task of another activity type, by component
    // or affinity alike.
    for component in ["h/h.Home", "h/h.Settings"] {
        assert_eq!(
            s.find_task(component, Some("h"), 0, None, ACTIVITY_TYPE_STANDARD),
            None
        );
        let d = s
            .decide(&start(component, "h", ACTIVITY_TYPE_STANDARD))
            .unwrap();
        assert_eq!(d.outcome, Outcome::New(None));
    }
    assert_eq!(
        s.find_task("h/h.Home", None, 0, None, ACTIVITY_TYPE_HOME),
        Some(1)
    );
    // A home activity of another task goes to the root home task too.
    let d = s
        .decide(&start("q/q.Home", "q", ACTIVITY_TYPE_HOME))
        .unwrap();
    assert_eq!(d.outcome, Outcome::New(Some(HOME_ROOT_TASK)));
}

#[test]
fn a_home_intent_starts_a_home_activity_unless_a_component_is_named_by_an_app() {
    let host = crate::app_host::tests::host_with_start(1, Arc::new(|| {}));
    let home = home_intent().unwrap();
    assert!(is_home_intent(&home.intent));
    // Resolved by anyone, or named by the system or root.
    assert_eq!(
        host.start_activity_type(&home.intent, 10_100).unwrap(),
        ACTIVITY_TYPE_HOME
    );
    let named = home.with_component("h", "h.Home");
    for uid in [SYSTEM_UID, ROOT_UID] {
        assert_eq!(
            host.start_activity_type(&named.intent, uid).unwrap(),
            ACTIVITY_TYPE_HOME
        );
    }
    // Another category, data or none is not a home intent.
    let launcher = RawIntent::new(ACTION_MAIN, 0, &[])
        .with_category("android.intent.category.LAUNCHER")
        .unwrap();
    assert_eq!(
        host.start_activity_type(&launcher.intent, 10_100).unwrap(),
        0
    );
    let two = home
        .with_category("android.intent.category.DEFAULT")
        .unwrap();
    assert!(!is_home_intent(&two.intent));
    let mut data = home.intent.clone();
    data.data = Some(crate::package::uri::Uri::parse("content://h"));
    assert!(!is_home_intent(&data));
}

// android-16.0.0_r1 ActivityRecord.handleAppDied keeps the stopped home
// that saved its state in the root home task. Behind the desktop in window
// mode it should not be visible, so it is not restarted
// (RootWindowContainer.attachApplication starts only those that should be)
// until a start brings it to the front.
#[test]
fn a_dead_home_with_saved_state_stays_unlaunched_behind_the_desktop() {
    for saved_state in [true, false] {
        let (mut s, _, _) = start_home_then_app(true);
        // It was resumed once, as when brought to the front before.
        s.get_mut(1).unwrap().have_state = false;
        assert!(s.stopped(1, saved_state));
        let commands = s.process_died(1, false);
        if !saved_state {
            assert!(s.get(1).is_none());
            assert!(commands.contains(&Command::TaskEmpty(HOME_ROOT_TASK)));
            continue;
        }
        assert!(!commands.contains(&Command::TaskEmpty(HOME_ROOT_TASK)));
        assert!(!commands.contains(&Command::Launch(1)), "{commands:?}");
        assert_eq!(s.top(HOME_ROOT_TASK).unwrap().token, 1);
        // Its process restarts as it comes to the front, but the app's
        // task is back in front before it attaches.
        // The app's activity pauses first.
        let home_front = |s: &mut Stack| {
            let mut commands = s.bring_to_front(HOME_ROOT_TASK);
            commands.extend(s.paused(2));
            commands
        };
        assert!(home_front(&mut s).contains(&Command::Launch(1)));
        s.bring_to_front(7);
        s.get_mut(1).unwrap().start_seq = 3;
        let commands = s.attached(1);
        assert!(!commands.iter().any(|c| matches!(
            c,
            Command::Launch(1) | Command::LaunchPaused(1) | Command::LaunchStopped(1)
        )));
        assert_eq!(s.get(1).unwrap().state, State::Initializing);
        // Brought to the front again, it is launched resumed.
        assert!(home_front(&mut s).contains(&Command::Launch(1)));
        assert_eq!(s.get(1).unwrap().state, State::Resumed);
    }
}
