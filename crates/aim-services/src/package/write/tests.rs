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
        target_sdk_version: 35,
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
            dropped: 0,
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
    Box::new(move |_, _| Some(state.clone()))
}

fn due(writes: &Writes, state: Arc<State>) -> Vec<Check> {
    writes.checks_at(&states(state), Instant::now() + SETTLE)
}

#[test]
fn an_app_changes_its_components_and_the_original_agrees() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre, 0);
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
    writes.observe(&pre, 0);
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
    writes.observe(&state(|_| {}), 0);
    let a = answer(&writes, 10100, 1, sent, set(Some(MAIN), APP, 2));
    assert!(
        matches!(a, Answer::NotModelled),
        "no state from before the call"
    );
}

#[test]
fn the_original_s_exceptions() {
    let writes = Writes::default();
    writes.observe(&state(|_| {}), 0);
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
    writes.observe(&state(|_| {}), 0);
    let a = answer(&writes, 1000, 1, Instant::now(), set(None, APP, 2));
    assert!(matches!(a, Answer::NotModelled));
}

#[test]
fn the_same_state_again_changes_nothing() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre, 0);
    let a = answer(&writes, 10100, 1, Instant::now(), set(Some(B), APP, 2));
    assert_eq!(exception(&a), None);
    let checks = due(&writes, pre);
    assert!(matches!(checks[0].outcome, CheckOutcome::Matched));
}

/// `state` with `name` installed as the original would: app id `app_id`,
/// stopped and not launched in user 0.
fn installed(name: &str, app_id: i32, edit: impl FnOnce(&mut PackageState)) -> PackageState {
    package(name, app_id, |ps| {
        ps.path = format!("/data/app/~~AbCd==/{name}-EfGh==");
        let u = ps.users.get_mut(&0).unwrap();
        *u = PackageUserState {
            stopped: true,
            not_launched: true,
            first_install_time: 0x1234,
            ..PackageUserState::default()
        };
        edit(ps)
    })
}

fn assert_matched(check: &Check) {
    match &check.outcome {
        CheckOutcome::Matched => {}
        CheckOutcome::Differed { original, model } => {
            panic!("{}: original {original:?}, model {model:?}", check.subject)
        }
        CheckOutcome::NotModelled(reason) => panic!("{}: not modelled: {reason}", check.subject),
    }
}

fn change_checks(pre: &Arc<State>, post: &Arc<State>) -> Vec<Check> {
    let writes = Writes::default();
    writes.observe(pre, 0);
    writes.observe(post, 0);
    due(&writes, post.clone())
}

#[test]
fn an_install_takes_the_first_free_app_id() {
    let pre = state(|s| {
        s.packages.get_mut(OTHER).unwrap().app_id = 10102;
    });
    let post = state(|s| {
        s.packages.get_mut(OTHER).unwrap().app_id = 10102;
        let p = installed("org.example.new", 10000, |_| {});
        s.packages.insert(p.name.clone(), p);
    });
    let checks = change_checks(&pre, &post);
    assert_eq!(checks.len(), 1);
    assert_eq!(
        (checks[0].operation.as_str(), checks[0].subject.as_str()),
        ("install", "org.example.new")
    );
    assert_matched(&checks[0]);

    // Another app id is a difference.
    let post = state(|s| {
        s.packages.get_mut(OTHER).unwrap().app_id = 10102;
        let p = installed("org.example.new", 10103, |_| {});
        s.packages.insert(p.name.clone(), p);
    });
    let checks = change_checks(&pre, &post);
    assert!(matches!(checks[0].outcome, CheckOutcome::Differed { .. }));
}

#[test]
fn a_removal_leaves_its_shared_user_and_installer_records() {
    use crate::package::model::SharedUser;
    let shared = |packages: &[&str]| SharedUser {
        name: "org.example.shared".into(),
        app_id: 10200,
        packages: packages.iter().map(|p| p.to_string()).collect(),
        ..SharedUser::default()
    };
    let pre = state(|s| {
        let a = s.packages.get_mut(APP).unwrap();
        a.app_id = 10200;
        a.shared_user = Some("org.example.shared".into());
        s.packages.get_mut(OTHER).unwrap().install_source.installer = Some(APP.into());
        s.shared_users
            .insert("org.example.shared".into(), shared(&[APP]));
    });
    let post = state(|s| {
        s.packages.remove(APP);
        s.packages.get_mut(OTHER).unwrap().install_source.installer = None;
    });
    let checks = change_checks(&pre, &post);
    assert_eq!(checks[0].operation, "removal");
    assert_matched(&checks[0]);

    // The shared user kept is a difference.
    let post = state(|s| {
        s.packages.remove(APP);
        s.packages.get_mut(OTHER).unwrap().install_source.installer = None;
        s.shared_users
            .insert("org.example.shared".into(), shared(&[]));
    });
    let checks = change_checks(&pre, &post);
    assert!(matches!(checks[0].outcome, CheckOutcome::Differed { .. }));
}

#[test]
fn a_system_package_s_first_update_keeps_it_disabled() {
    let system = |ps: &mut PackageState| {
        ps.is.system = true;
        ps.path = "/product/app/Other".into();
    };
    let pre = state(|s| system(s.packages.get_mut(OTHER).unwrap()));
    let post = state(|s| {
        let disabled = s.packages.get(OTHER).cloned().map(|mut d| {
            system(&mut d);
            d
        });
        s.disabled_system_packages
            .insert(OTHER.into(), disabled.unwrap());
        let p = s.packages.get_mut(OTHER).unwrap();
        p.is.system = true;
        p.is.updated_system_app = true;
        p.path = format!("/data/app/~~x==/{OTHER}-y==");
    });
    let checks = change_checks(&pre, &post);
    assert_eq!(checks[0].operation, "update");
    assert_matched(&checks[0]);
}

#[test]
fn a_user_removal_resets_the_user_s_state() {
    let system = |ps: &mut PackageState| {
        ps.is.system = true;
        ps.path = "/product/app/Other".into();
    };
    let pre = state(|s| system(s.packages.get_mut(OTHER).unwrap()));
    let post = state(|s| {
        let p = s.packages.get_mut(OTHER).unwrap();
        system(p);
        *p.users.get_mut(&0).unwrap() = PackageUserState {
            installed: false,
            stopped: true,
            not_launched: true,
            ..PackageUserState::default()
        };
    });
    let checks = change_checks(&pre, &post);
    assert_eq!(checks[0].operation, "user removal");
    assert_matched(&checks[0]);
}

#[test]
fn a_session_reads_as_the_bridge_writes_it() {
    let mut p = Parcel::new();
    p.write_string16(Some(APP));
    for v in [-1, 1, 0x400002, 4] {
        p.write_i32(v);
    }
    p.write_string16(Some("com.android.vending"));
    for v in [10050, -1, 3, 0, 0, 0, -1, 1] {
        p.write_i32(v);
    }
    p.write_string16(None);
    let s = session::Session::read(7, p.data()).unwrap();
    assert_eq!(
        s,
        session::Session {
            id: 7,
            package: Some(APP.into()),
            user: -1,
            mode: 1,
            install_flags: 0x400002,
            install_reason: 4,
            installer: Some("com.android.vending".into()),
            installer_uid: 10050,
            originating_uid: -1,
            package_source: 3,
            parent: -1,
            committed: true,
            ..session::Session::default()
        }
    );
}

#[test]
fn an_install_session_decides_the_install_reason_and_the_enabler() {
    let s = session::Session {
        user: -1,
        install_reason: 4,
        installer: Some("com.android.vending".into()),
        ..session::Session::default()
    };
    let pre = state(|_| {});
    let new = installed("org.example.new", 10000, |ps| {
        let u = ps.users.get_mut(&0).unwrap();
        u.install_reason = 4;
        u.last_disable_app_caller = Some("com.android.vending".into());
    });
    let pkg = new.pkg.clone().unwrap();
    let post = state(|st| {
        st.packages.insert(new.name.clone(), new.clone());
    });
    let kind = change::Kind::Install;
    let model = change::model(
        kind,
        &pre,
        &post,
        "org.example.new",
        Some(&pkg),
        Some(&s),
        0,
    );
    let original = change::original(kind, &pre, &post, "org.example.new", true);
    assert_eq!(model.unwrap(), original);

    // An update keeps the reason the user installed it with, and is
    // enabled by its installer.
    let updated = state(|st| {
        let p = st.packages.get_mut(APP).unwrap();
        p.path = format!("/data/app/~~z==/{APP}-w==");
        let u = p.users.get_mut(&0).unwrap();
        u.last_disable_app_caller = Some("com.android.vending".into());
    });
    let pkg = updated.packages[APP].pkg.clone().unwrap();
    let kind = change::Kind::Update;
    let model = change::model(kind, &pre, &updated, APP, Some(&pkg), Some(&s), 0);
    let original = change::original(kind, &pre, &updated, APP, true);
    assert_eq!(model.unwrap(), original);
}

#[test]
fn an_install_s_signers_are_verified_from_its_apks() {
    // Signed with v3 and a proof-of-rotation lineage of two certificates.
    const GSF: &str = "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk";
    let Some(root) = aim_paths::original_image_with(GSF) else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("aim-write-apks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("base.apk");
    let _ = std::fs::remove_file(&base);
    std::os::unix::fs::symlink(root.join(GSF), &base).unwrap();
    let guest = "/data/app/~~a==/com.google.android.gsf-b==".to_string();
    let host = dir.clone();
    let apks = apk::Apks {
        files: Box::new(move |p| (p == guest).then(|| host.clone())),
        platform: crate::package::parse::Platform::load(&root, Default::default()).unwrap(),
    };
    let ps = PackageState {
        path: "/data/app/~~a==/com.google.android.gsf-b==".into(),
        ..PackageState::default()
    };
    let pkg = AndroidPackage {
        target_sdk_version: 36,
        ..AndroidPackage::default()
    };
    let signatures = apks.signatures(&ps, &pkg).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        signatures.scheme_version,
        crate::package::sign::SIGNING_BLOCK_V3
    );
    assert_eq!(signatures.signatures.len(), 1);
    assert_eq!(signatures.past_signatures.map(|p| p.len()), Some(2));
}

#[test]
fn only_what_the_writes_named_is_compared() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre, 0);
    answer(&writes, 10100, 1, Instant::now(), set(Some(MAIN), APP, 2));
    // system_server enabled B itself, in process: not the app's write.
    let post = state(|s| {
        let u = s.packages.get_mut(APP).unwrap().users.get_mut(&0).unwrap();
        u.disabled_components = vec![MAIN.into()];
        u.enabled_components = vec![B.into()];
    });
    let checks = due(&writes, post);
    assert_matched(&checks[0]);
}

#[test]
fn a_package_written_while_copies_were_dropped_is_not_compared() {
    let writes = Writes::default();
    let pre = state(|_| {});
    writes.observe(&pre, 3);
    let (code, data) = set(Some(MAIN), APP, 2);
    writes.answer(&mut ShadowCall {
        service: "package",
        descriptor: pm::DESCRIPTOR,
        code,
        flags: 0,
        sender_pid: 77,
        sender_euid: 10100,
        seq: 1,
        sent: Instant::now(),
        dropped: 5,
        data: Reader::new(data.data(), &[]),
    });
    let checks = due(&writes, pre);
    assert!(matches!(checks[0].outcome, CheckOutcome::NotModelled(_)));
}
