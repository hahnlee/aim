use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, Parcel, Reader};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::*;
use crate::package::model::{PackageUserState, User};
use crate::package::pkg::{Activity, AndroidPackage};

const APP: &str = "org.example.app";
const OTHER: &str = "org.example.other";
const MAIN: &str = "org.example.app.Main";
const B: &str = "org.example.app.B";

fn activity(name: &str) -> Activity {
    let mut a = Activity::default();
    a.main.component.name = name.into();
    a.main.component.package_name = APP.into();
    a
}

fn package(name: &str, app_id: i32, edit: impl FnOnce(&mut PackageState)) -> PackageState {
    let mut ps = PackageState {
        name: name.into(),
        app_id,
        ..PackageState::default()
    };
    ps.pkg = Some(Arc::new(AndroidPackage {
        package_name: name.into(),
        target_sdk_version: 35,
        activities: vec![activity(MAIN), activity(B)],
        ..AndroidPackage::default()
    }));
    ps.users.insert(
        0,
        PackageUserState {
            disabled_components: vec![B.into()],
            ..PackageUserState::default()
        },
    );
    edit(&mut ps);
    ps
}

fn state(edit: impl FnOnce(&mut State)) -> Arc<State> {
    let mut state = State {
        packages: [package(APP, 10100, |_| {}), package(OTHER, 10101, |_| {})]
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect(),
        users: BTreeMap::from([(
            0,
            User {
                id: 0,
                unlocking_or_unlocked: true,
                ..User::default()
            },
        )]),
        ..State::default()
    };
    edit(&mut state);
    Arc::new(state)
}

/// A `setComponentEnabledSetting` of `uid` (pid 77), or with no class
/// `setApplicationEnabledSetting`.
fn set(class: Option<&str>, package: &str, new_state: i32) -> (u32, Parcel) {
    let mut p = Parcel::new();
    p.write_interface_token(pm::DESCRIPTOR);
    let code = match class {
        Some(class) => {
            p.write_i32(1);
            p.write_string16(Some(package));
            p.write_string16(Some(class));
            pm::SET_COMPONENT_ENABLED_SETTING
        }
        None => {
            p.write_string16(Some(package));
            pm::SET_APPLICATION_ENABLED_SETTING
        }
    };
    p.write_i32(new_state);
    p.write_i32(0);
    p.write_i32(0);
    p.write_string16(Some("org.example.caller"));
    (code, p)
}

fn answer(writes: &Writes, uid: u32, seq: u64, sent: Instant, call: (u32, Parcel)) -> Answer {
    let (code, data) = call;
    writes
        .answer(&mut ShadowCall {
            service: "package",
            descriptor: pm::DESCRIPTOR,
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            seq,
            sent,
            data: Reader::new(data.data(), &[]),
        })
        .expect("a write")
}

/// The exception of a reply; `None` for none.
fn exception(a: &Answer) -> Option<(i32, String)> {
    let Answer::Reply(p) = a else {
        panic!("not a reply")
    };
    Reader::new(p.data(), &[])
        .read_exception()
        .unwrap()
        .err()
        .map(|e| (e.code, e.message))
}

fn states(state: Arc<State>) -> States {
    Box::new(move |_| Some(state.clone()))
}

fn due(writes: &Writes, state: Arc<State>) -> Vec<Check> {
    writes.checks_at(&states(state), Instant::now() + SETTLE)
}

#[test]
fn an_app_changes_its_components_and_the_original_agrees() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre);
    let sent = Instant::now();
    let a = answer(&writes, 10100, 1, sent, set(Some(MAIN), APP, 2));
    assert_eq!(exception(&a), None);
    let a = answer(&writes, 10100, 2, sent, set(Some(B), APP, 0));
    assert_eq!(exception(&a), None);
    assert!(
        writes
            .checks_at(&states(pre.clone()), Instant::now())
            .is_empty(),
        "no check before the writes settle"
    );

    let post = state(|s| {
        let u = s.packages.get_mut(APP).unwrap().users.get_mut(&0).unwrap();
        u.disabled_components = vec![MAIN.into()];
    });
    let checks = due(&writes, post);
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].subject, "org.example.app user 0");
    assert_eq!(
        checks[0].calls,
        [
            (1, pm::SET_COMPONENT_ENABLED_SETTING),
            (2, pm::SET_COMPONENT_ENABLED_SETTING)
        ]
    );
    assert!(matches!(checks[0].outcome, CheckOutcome::Matched));
    assert!(due(&writes, pre).is_empty(), "a check is made once");
}

#[test]
fn a_difference_shows_both_states() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre);
    answer(&writes, 10100, 1, Instant::now(), set(None, APP, 3));
    // The original did not change the package.
    let checks = due(&writes, pre);
    let CheckOutcome::Differed { original, model } = &checks[0].outcome else {
        panic!("differed expected")
    };
    let enabled = |v: &Value| match v {
        Value::Fields(f) => f[..2].to_vec(),
        _ => panic!(),
    };
    assert_eq!(
        enabled(original),
        [
            ("enabled".to_string(), Value::Int(0)),
            ("lastDisableAppCaller".to_string(), Value::Null)
        ]
    );
    assert_eq!(
        enabled(model),
        [
            ("enabled".to_string(), Value::Int(3)),
            (
                "lastDisableAppCaller".to_string(),
                Value::Str("org.example.caller".into())
            )
        ]
    );
}

#[test]
fn the_pre_state_is_older_than_the_call() {
    let writes = Writes::default();
    let sent = Instant::now();
    std::thread::sleep(Duration::from_millis(2));
    writes.observe(&state(|_| {}));
    let a = answer(&writes, 10100, 1, sent, set(Some(MAIN), APP, 2));
    assert!(
        matches!(a, Answer::NotModelled),
        "no state from before the call"
    );
}

#[test]
fn the_original_s_exceptions() {
    let writes = Writes::default();
    writes.observe(&state(|_| {}));
    let now = Instant::now();
    let a = answer(&writes, 10100, 1, now, set(Some(MAIN), OTHER, 2));
    assert_eq!(
        exception(&a),
        Some((
            EX_SECURITY,
            "Attempt to change component state; pid=77, uid=10100, \
             component=ComponentInfo{org.example.other/org.example.app.Main}"
                .into()
        ))
    );
    let a = answer(&writes, 10100, 2, now, set(None, OTHER, 2));
    assert_eq!(
        exception(&a).unwrap().1,
        "Attempt to change component state; pid=77, uid=10100, package=org.example.other"
    );
    let a = answer(
        &writes,
        10100,
        3,
        now,
        set(Some("org.example.app.Gone"), APP, 2),
    );
    assert_eq!(
        exception(&a),
        Some((
            EX_ILLEGAL_ARGUMENT,
            "Component class org.example.app.Gone does not exist in org.example.app".into()
        ))
    );
    let a = answer(&writes, 10100, 4, now, set(Some(MAIN), APP, 7));
    assert_eq!(
        exception(&a),
        Some((EX_ILLEGAL_ARGUMENT, "Invalid new component state: 7".into()))
    );
    // System may change any package, and an unknown one is unknown.
    let a = answer(&writes, 1000, 5, now, set(None, "org.example.none", 2));
    assert_eq!(
        exception(&a),
        Some((
            EX_ILLEGAL_ARGUMENT,
            "Unknown package: org.example.none".into()
        ))
    );
    assert!(due(&writes, state(|_| {})).is_empty(), "nothing changed");
}

#[test]
fn another_package_s_protection_is_not_modelled() {
    let writes = Writes::default();
    writes.observe(&state(|_| {}));
    let a = answer(&writes, 1000, 1, Instant::now(), set(None, APP, 2));
    assert!(matches!(a, Answer::NotModelled));
}

#[test]
fn the_same_state_again_changes_nothing() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre);
    let a = answer(&writes, 10100, 1, Instant::now(), set(Some(B), APP, 2));
    assert_eq!(exception(&a), None);
    let checks = due(&writes, pre);
    assert!(matches!(checks[0].outcome, CheckOutcome::Matched));
}
