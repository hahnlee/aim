//! ATMS/ActivityStarter at android-16.0.0_r1; accessibility shortcut system starts.
use super::kept::{RawBundle, RawIntent};
use super::*;
use aim_binder_host::parcel::EX_SECURITY;
use aim_service_aidl::android_app_iactivitytaskmanager as atm;

fn invoke(host: &Arc<AppHost>, code: u32, p: &Parcel, uid: u32) -> Result<i32, Exception> {
    let p = activity::ActivityTaskManager(host.clone(), Binder::Local(0))
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap();
    atm::read_start_activity_reply(&mut p.reader()).unwrap()
}
pub(super) fn args(package: Option<&str>, user: i32, intent: Option<RawIntent>) -> AsUser {
    atm::StartActivityAsUser {
        caller: None,
        calling_package: package.map(Into::into),
        calling_feature_id: None,
        intent,
        resolved_type: None,
        result_to: None,
        result_who: None,
        request_code: -1,
        flags: 0,
        profiler_info: None,
        options: None,
        user_id: user,
    }
}
type AsUser = atm::StartActivityAsUser<RawIntent, RawBundle, RawBundle>;
fn send(host: &Arc<AppHost>, uid: u32, a: &AsUser) -> Result<i32, Exception> {
    let mut p = Parcel::new();
    a.write(&mut p);
    invoke(host, atm::START_ACTIVITY_AS_USER, &p, uid)
}
fn intent(class: &str) -> RawIntent {
    RawIntent::of(None, 0, &[]).with_component("p", class)
}

#[test]
fn launch_options_reject_unserved_owners_before_admitting_an_activity() {
    use crate::bundle::Value::{Bool, Int};
    use aim_service_aidl::ReadParcelable;
    with_start(|host, _| {
        permissions(host);
        for (key, value, name) in [
            ("android.activity.launchTaskId", Int(7), "task targeting"),
            (
                "android.activity.launchTaskDisplayAreaFeatureId",
                Int(1),
                "task display area",
            ),
            ("android.activity.activityType", Int(2), "activity type"),
            ("android.activity.taskOverlay", Bool(true), "task overlay"),
            (
                "android.activity.splashScreenStyle",
                Int(1),
                "splash screen",
            ),
            (
                "android.activity.dismissKeyguardIfInsecure",
                Bool(true),
                "dismiss keyguard",
            ),
            (
                "android:activity.lockTaskMode",
                Bool(true),
                "lockTaskMode=true",
            ),
            (
                "android.activity.launchDisplayId",
                Int(99),
                "launchDisplayId=99",
            ),
        ] {
            let mut p = Parcel::new();
            crate::bundle::write(&mut p, &[(key, value)]);
            let options = RawBundle::read_from(&mut p.reader()).unwrap();
            let error = host
                .activity_launch_options(Some(&options), 1000)
                .err()
                .unwrap();
            assert!(error.message.contains(name), "{error:?}");
            let mut a = args(Some("android"), 0, Some(intent("p.Main")));
            a.options = Some(options);
            if error.code == EX_SECURITY {
                assert_eq!(send(host, 1000, &a).unwrap_err().code, EX_SECURITY);
            } else {
                assert_eq!(send(host, 1000, &a), Ok(-96));
            }
            assert!(host.activities.records.lock().unwrap().is_empty());
        }
    });
}
pub(super) fn permissions(host: &Arc<AppHost>) {
    host.permission_manager
        .set(Arc::new(
            permission_manager::PermissionManager::with_system(host.clone(), Default::default()),
        ))
        .ok()
        .unwrap();
    let metrics = display::Metrics::of(&display::Display::phone_for_test());
    host.display_metrics.set(Ok(metrics)).unwrap();
}

pub(super) fn with_start(test: impl FnOnce(&Arc<AppHost>, &dyn Fn(Arc<dyn Service>) -> Strong)) {
    crate::system::with_discovery_binders_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        true,
        |host, _, _, export| test(host, export),
    );
}

// CTS ContextTest.testStartActivities; pinned ATMS/controller guards and START_* results.
fn multi(
    host: &Arc<AppHost>,
    uid: u32,
    a: &AsUser,
    intents: Option<&[Option<RawIntent>]>,
    types: Option<&[Option<String>]>,
) -> Result<i32, Exception> {
    let mut p = Parcel::new();
    p.write_interface_token(atm::DESCRIPTOR);
    p.write_binder(a.caller);
    p.write_string16(a.calling_package.as_deref());
    p.write_string16(None);
    aim_service_aidl::write_typed_list(&mut p, intents);
    aim_service_aidl::write_string_list(&mut p, types);
    p.write_binder(a.result_to);
    aim_service_aidl::write_typed(&mut p, a.options.as_ref());
    p.write_i32(a.user_id);
    invoke(host, atm::START_ACTIVITIES, &p, uid)
}

#[test]
fn multi_start_validates_arrays_caller_and_preflights_before_execution() {
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_NULL_POINTER};
    crate::system::with_discovery_binders(|host, publish, _, _| {
        permissions(host);
        let mut a = args(Some("android"), 0, None);
        let one = [Some(intent("missing"))];
        let types = [None];
        for (intents, types, expected) in [
            (None, Some(types.as_slice()), EX_NULL_POINTER),
            (Some(one.as_slice()), None, EX_NULL_POINTER),
            (
                Some(one.as_slice()),
                Some([].as_slice()),
                EX_ILLEGAL_ARGUMENT,
            ),
        ] {
            assert_eq!(
                multi(host, 1000, &a, intents, types).unwrap_err().code,
                expected
            );
        }
        assert_eq!(multi(host, 1000, &a, Some(&[]), Some(&[])), Ok(0));
        a.caller = Some(Binder::Handle(u32::MAX));
        assert_eq!(multi(host, 1000, &a, Some(&one), Some(&types)), Ok(-94));
        a.caller = None;
        for (uid, package, user) in [(10100, "q", 0), (10100, "p", 10), (99000, "p", 0)] {
            a.calling_package = Some(package.into());
            a.user_id = user;
            assert_eq!(
                multi(host, uid, &a, Some(&one), Some(&types))
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
        }
        a = args(Some("android"), 0, None);
        assert_eq!(multi(host, 1000, &a, Some(&one), Some(&types)), Ok(-2));
        let implicit = [Some(RawIntent::of(Some("missing"), 0, &[]))];
        assert_eq!(multi(host, 1000, &a, Some(&implicit), Some(&types)), Ok(-1));
        publish(&mut |p| p.booleans |= crate::package::pkg::booleans::CANT_SAVE_STATE);
        let intents = [Some(intent("missing")), Some(intent("p.Main"))];
        assert_eq!(
            multi(host, 1000, &a, Some(&intents), Some(&[None, None]))
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
        assert!(host.activities.records.lock().unwrap().is_empty());
    });
}

#[test]
fn start_as_user_validates_package_user_and_caller_before_resolution() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        permissions(host);
        let start = |uid, package, user, intent| send(host, uid, &args(package, user, intent));
        for package in [None, Some("q")] {
            let e = start(10100, package, 0, Some(intent("missing"))).unwrap_err();
            assert_eq!(e.code, EX_SECURITY);
            assert!(e.message.contains("does not belong to uid=10100"));
        }
        let e = start(10100, Some("p"), 10, Some(intent("missing"))).unwrap_err();
        assert_eq!(e.code, EX_SECURITY);
        assert!(e.message.contains("INTERACT_ACROSS_USERS"));
        for uid in [0, 1000] {
            for package in [None, Some("android"), Some("q")] {
                for user in [0, -2] {
                    assert_eq!(start(uid, package, user, Some(intent("missing"))), Ok(-2));
                }
            }
            // ActivityStarter resolves nothing for a user that does not exist.
            assert_eq!(start(uid, None, 10, Some(intent("p.Main"))), Ok(-2));
        }
        // START_FLAG_DEBUG is not served: the start is cancelled.
        let mut debug = args(Some("android"), 0, Some(intent("p.Main")));
        debug.flags = 2;
        assert_eq!(send(host, 1000, &debug), Ok(-96));
        // ActivityStarter exposes a denied BAL start as success, without launching.
        assert_eq!(start(10100, Some("p"), 0, Some(intent("p.Main"))), Ok(0));
        let implicit = RawIntent::of(Some("missing"), 0, &[]);
        assert_eq!(start(10100, Some("p"), 0, Some(implicit)), Ok(-1));
        assert!(host.activities.records.lock().unwrap().is_empty());
    });
}

#[test]
fn system_thread_start_creates_activity_and_returns_start_success() {
    struct Thread;
    impl Service for Thread {
        fn descriptor(&self) -> &str {
            thread::DESCRIPTOR
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            Ok(Parcel::new())
        }
    }
    with_start(|host, export| {
        permissions(host);
        let app_thread = export(Arc::new(Thread));
        let system_thread = export(Arc::new(Thread));
        for (seq, package, uid, thread) in [
            (1, "p", 10100, app_thread),
            (2, "android", 1000, system_thread),
        ] {
            let fixture = tests::host_with_start(seq, Arc::new(|| {}));
            let mut app = fixture.apps.lock().unwrap().remove(&seq).unwrap();
            app.package = package.into();
            app.process = package.into();
            app.uid = uid;
            app.package_uid = uid;
            app.isolated = false;
            app.thread = Some(thread);
            app.config = Some(host.global_configuration().unwrap());
            host.apps.lock().unwrap().insert(seq, app);
        }
        let info = host.activity_info_of("p", "p.Main").unwrap();
        let request = activity_start::StartRequest::launcher("p.Main");
        let token = host.new_activity(1, 10100, "p", info, Default::default(), 7, request);
        {
            let mut stack = host.activities.stack.lock().unwrap();
            stack.tasks.push(activity_stack::Task {
                id: 7,
                affinity: None,
                real_activity: "p/p.Main".into(),
                intent: Default::default(),
                base: activity_start::LaunchIntent::Component,
                uid: 10100,
                user: 0,
                last_active: 0,
            });
            stack.activities.push(activity_stack::Activity::new(
                token, 7, 1, "p/p.Main", 0, 0, None,
            ));
        }
        let mut a = args(Some("android"), 0, Some(intent("p.Main")));
        a.flags = 0x1000_0000; // ContextImpl.startActivityAsUser's FLAG_ACTIVITY_NEW_TASK.
        a.caller = Some(
            host.apps.lock().unwrap()[&2]
                .thread
                .as_ref()
                .unwrap()
                .binder(),
        );
        a.result_to = Some(Binder::Local(token));
        assert_eq!(send(host, 1000, &a), Ok(0));
        a.caller = Some(Binder::Handle(u32::MAX));
        assert_eq!(send(host, 1000, &a), Ok(-94));
        let records = host.activities.records.lock().unwrap();
        assert_eq!(records.len(), 2);
        let started = records.iter().find(|(t, _)| **t != token).unwrap().1;
        let from = &started.launched_from;
        assert_eq!((started.uid, started.task, from.uid), (10100, 7, 1000));
        assert_eq!(from.package.as_deref(), Some("android"));
        drop(records);
        let ordered = |n| {
            Some(
                RawIntent::of(None, 0, &[("order", crate::bundle::Value::Int(n))])
                    .with_component("p", "p.Main"),
            )
        };
        let mut a = args(Some("p"), 0, None);
        a.result_to = Some(Binder::Local(token));
        let intents = [ordered(1), ordered(2), Some(intent("missing"))];
        assert_eq!(
            multi(host, 10100, &a, Some(&intents), Some(&[None, None, None])),
            Ok(-2)
        );
        let orders = || {
            let stack = host.activities.stack.lock().unwrap();
            let records = host.activities.records.lock().unwrap();
            stack
                .activities
                .iter()
                .filter_map(|a| match &records[&a.token].intent {
                    activity_start::LaunchIntent::Raw(k) => {
                        let n = k.intent.int_extra("order", 0).unwrap();
                        (n != 0).then_some((n, a.task))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(orders(), [(1, 7), (2, 7)]);
        // PendingIntentRecord sends allIntents as the creator; immutable fill-in affects none.
        let target = host
            .get_intent_sender(
                10100,
                pending_intents::Request {
                    kind: pending_intents::INTENT_SENDER_ACTIVITY,
                    package: Some("p".into()),
                    feature: None,
                    token: None,
                    result_who: None,
                    request_code: 0,
                    intents: Some(vec![ordered(3), ordered(4)]),
                    resolved_types: None,
                    flags: 0x44000000,
                    options: None,
                    user: 0,
                },
            )
            .unwrap()
            .unwrap();
        let sent = || pending_intents::Sent {
            result_to: a.result_to,
            request_code: 12,
            ..Default::default()
        };
        let send_pending = || {
            host.send_record(None, target, 0, ordered(99), None, None, None, sent())
                .unwrap()
        };
        assert_eq!(send_pending(), 0);
        assert_eq!(send_pending(), -96);
        assert_eq!(orders(), [(1, 7), (2, 7), (3, 7), (4, 7)]);
        // Both cold activities join the pending process and retain their admitted tokens/task.
        let thread = host.apps.lock().unwrap().get_mut(&1).unwrap().thread.take();
        let intents = [ordered(5), ordered(6)];
        assert_eq!(
            multi(host, 10100, &a, Some(&intents), Some(&[None, None])),
            Ok(0)
        );
        assert_eq!(orders(), [(1, 7), (2, 7), (3, 7), (4, 7), (5, 7), (6, 7)]);
        assert_eq!(host.apps.lock().unwrap()[&1].activities.len(), 2);
        {
            let stack = host.activities.stack.lock().unwrap();
            let pending = stack
                .activities
                .iter()
                .filter(|a| a.pending && a.start_seq == 1);
            assert_eq!(pending.count(), 2);
        }
        host.apps.lock().unwrap().get_mut(&1).unwrap().thread = thread;
        host.finish_attach(
            &Call {
                code: 0,
                flags: 0,
                sender_pid: 71,
                sender_euid: 10100,
                data: Parcel::new().reader(),
            },
            1,
        )
        .unwrap();
        {
            let stack = host.activities.stack.lock().unwrap();
            assert!(!stack.activities.iter().any(|a| a.pending));
        }
        assert_eq!(orders().len(), 6);
    });
}

#[test]
fn configuration_changes_stay_explicit_outside_shared_path() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        permissions(host);
        let unchanged = host.global_configuration().unwrap();
        let changed = config::Configuration {
            ui_mode: 2,
            ..Default::default()
        };
        for (code, config, expected) in [
            (atm::START_ACTIVITY_WITH_CONFIG, None, -2),
            (atm::START_ACTIVITY_WITH_CONFIG, Some(unchanged), -2),
            (atm::START_ACTIVITY_WITH_CONFIG, Some(changed), -96),
        ] {
            let mut p = Parcel::new();
            p.write_interface_token(atm::DESCRIPTOR);
            p.write_binder(None);
            p.write_string16(Some("android"));
            if code == atm::START_ACTIVITY_WITH_CONFIG {
                p.write_string16(None);
            }
            p.write_i32(1);
            intent("missing").write_to(&mut p);
            p.write_string16(None);
            p.write_binder(None);
            p.write_string16(None);
            p.write_i32(-1);
            p.write_i32(0);
            p.write_i32(i32::from(config.is_some()));
            if let Some(config) = config {
                config.write_to(&mut p);
            }
            p.write_i32(0); // options
            if code == atm::START_ACTIVITY_AS_CALLER {
                p.write_bool(true);
            }
            p.write_i32(-2);
            assert_eq!(invoke(host, code, &p, 1000), Ok(expected));
        }
        assert!(host.activities.records.lock().unwrap().is_empty());
    });
}
