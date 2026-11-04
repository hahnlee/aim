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
    s.stopped(1);
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
    // Bringing it to the front leaves it behind the desktop.
    assert!(s.bring_to_front(HOME_ROOT_TASK).is_empty());
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
