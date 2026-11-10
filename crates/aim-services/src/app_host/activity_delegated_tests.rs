//! ATMS.startActivityAsCaller / ActivityStarter, android-16.0.0_r1.
use super::super::*;
use super::*;
use aim_binder_host::{
    local::{Service, Strong},
    parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY},
};
use aim_service_aidl::WriteParcelable;
struct Thread;
impl Service for Thread {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Ok(Parcel::new())
    }
}
fn fixture(host: &Arc<AppHost>, thread: Strong) {
    activity_start_tests::permissions(host);
    let f = super::super::tests::host_with_start(1, Arc::new(|| {}));
    let mut app = f.apps.lock().unwrap().remove(&1).unwrap();
    app.uid = 10100;
    app.package_uid = 10100;
    app.package = "p".into();
    app.process = "p".into();
    app.thread = Some(thread);
    app.config = Some(host.global_configuration().unwrap());
    host.apps.lock().unwrap().insert(1, app);
    let f = super::super::tests::host_with_start(2, Arc::new(|| {}));
    let mut system = f.apps.lock().unwrap().remove(&2).unwrap();
    system.uid = 1000;
    system.proc_state = 0;
    host.apps.lock().unwrap().insert(2, system);
}
fn source(host: &Arc<AppHost>) -> u64 {
    let token = host.new_activity(
        1,
        10100,
        "p",
        host.activity_info_of("p", "p.Main").unwrap(),
        Default::default(),
        7,
        activity_start::StartRequest::launcher("p.Main"),
    );
    host.tasks.animation_test_task(7, Default::default());
    let mut stack = host.activities.stack.lock().unwrap();
    stack.tasks.push(activity_stack::Task {
        id: 7,
        affinity: None,
        real_activity: "p/p.Main".into(),
        package: "p".into(),
        intent: Default::default(),
        base: activity_start::LaunchIntent::Component,
        uid: 10100,
        user: 0,
        last_active: 0,
        root_was_reset: false,
    });
    stack.activities.push(activity_stack::Activity::new(
        token, 7, 1, "p/p.Main", 0, 0, None,
    ));
    token
}

fn invoke(host: &Arc<AppHost>, uid: u32, code: u32, p: &Parcel) -> Result<i32, Exception> {
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
fn denied(result: Result<i32, Exception>) {
    assert_eq!(result.unwrap_err().code, EX_SECURITY);
}
fn next(
    host: &Arc<AppHost>,
    uid: u32,
    token: Option<Binder>,
    intent: Option<&RawIntent>,
) -> Result<i32, Exception> {
    let mut p = Parcel::new();
    p.write_interface_token(atm::DESCRIPTOR);
    p.write_binder(token);
    aim_service_aidl::write_typed(&mut p, intent);
    p.write_i32(0);
    invoke(host, uid, atm::START_NEXT_MATCHING_ACTIVITY, &p)
}
fn caller(
    host: &Arc<AppHost>,
    uid: u32,
    token: Option<Binder>,
    intent: &RawIntent,
    ignore: bool,
) -> Result<i32, Exception> {
    caller_options(host, uid, token, intent, ignore, (None, -10000, 0))
}
fn caller_options(
    host: &Arc<AppHost>,
    uid: u32,
    token: Option<Binder>,
    intent: &RawIntent,
    ignore: bool,
    settings: (Option<&RawBundle>, i32, i32),
) -> Result<i32, Exception> {
    let mut p = Parcel::new();
    p.write_interface_token(atm::DESCRIPTOR);
    p.write_binder(Some(Binder::Handle(u32::MAX)));
    p.write_string16(Some("untrusted.package"));
    aim_service_aidl::write_typed(&mut p, Some(intent));
    p.write_string16(None);
    p.write_binder(token);
    p.write_string16(None);
    p.write_i32(-1);
    p.write_i32(settings.2);
    p.write_i32(0); // profiler
    aim_service_aidl::write_typed(&mut p, settings.0);
    p.write_bool(ignore);
    p.write_i32(settings.1);
    invoke(host, uid, atm::START_ACTIVITY_AS_CALLER, &p)
}
#[test]
fn as_caller_checks_token_permission_original_identity_and_target_security() {
    crate::system::with_discovery_binders_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        true,
        |host, publish, _, export| {
            fixture(host, export(Arc::new(Thread)));
            let t = source(host);
            let token = Some(Binder::Local(t));
            let missing = RawIntent::of(None, 0, &[]).with_component("p", "missing");
            for bad in [None, Some(Binder::Local(u64::MAX))] {
                denied(caller(host, 1000, bad, &missing, false));
            }
            denied(caller(host, 10100, token, &missing, false));
            assert_eq!(caller(host, 1000, token, &missing, false), Ok(-2));
            denied(caller(
                host,
                1000,
                token,
                &RawIntent::of(None, 0, &[]),
                true,
            ));
            host.activities.with(token, |r| {
                r.launched_from.uid = 10101;
                r.launched_from.package = Some("q".into());
            });
            let intent = RawIntent::of(None, 0, &[]).with_component("p", "p.Main");
            publish(&mut |p| p.activities[0].permission = Some("test.DENIED".into()));
            denied(caller(host, 1000, token, &intent, false));
            publish(&mut |p| p.activities[0].main.exported = false);
            denied(caller(host, 1000, token, &intent, false));
            assert_eq!(caller(host, 1000, token, &intent, true), Ok(0));
            let records = host.activities.records.lock().unwrap();
            let from = &records
                .iter()
                .find(|(id, _)| **id != t)
                .unwrap()
                .1
                .launched_from;
            assert_eq!((from.uid, from.package.as_deref()), (10101, Some("q")));
            drop(records);
            query_owners(host);
            host.activities
                .with(token, |r| r.package = "android".into());
            denied(caller(host, 10100, token, &missing, false));
            host.activities.with(token, |r| r.launched_from.uid = 10100);
            assert_eq!(caller(host, 10100, token, &missing, false), Ok(-2));
            host.apps.lock().unwrap().get_mut(&1).unwrap().uid = 1000;
            assert_eq!(caller(host, 10101, token, &missing, false), Ok(-2));
            host.apps.lock().unwrap().clear();
            denied(caller(host, 1000, token, &missing, false));
        },
    );
}

fn query_owners(host: &Arc<AppHost>) {
    static CLASSES: std::sync::OnceLock<Arc<aim_android_image::linkage::Hierarchy>> =
        std::sync::OnceLock::new();
    let classes = CLASSES.get_or_init(|| {
        let image = aim_paths::original_image();
        let jars = aim_android_image::classpath::jars(
            &image,
            "bootclasspath.pb",
            aim_android_image::classpath::BOOTCLASSPATH,
        )
        .unwrap();
        Arc::new(
            aim_android_image::linkage::ClassPath::read(&image, &jars)
                .unwrap()
                .hierarchy()
                .unwrap(),
        )
    });
    let system = host.package_system().unwrap();
    let base = host.package_scan_capture().unwrap();
    let mut context = (**base.context()).clone();
    context.boot_classes = Some(classes.clone());
    context.system.system_permissions = Some(Default::default());
    for p in context.packages.values_mut() {
        p.filter_application_query = true;
    }
    context.scan_version += 1;
    system
        .publish_package_scan_with_queries(
            &system.package_bootstrap().unwrap(),
            Some(base.scan()),
            base.scan().owner().clone(),
            base.scan().usage().clone(),
            context,
        )
        .unwrap();
}

#[test]
fn as_caller_background_modes_preserve_both_opt_outs_and_real_caller_exemptions() {
    for additional in [false, true] {
        crate::system::with_discovery_binders_flags(
            &[(
                "com.android.window.flags.bal_additional_start_modes",
                additional,
            )],
            true,
            |host, _, _, _| {
                activity_start_tests::permissions(host);
                query_owners(host);
                let as_caller = |host: &AppHost, uid, real, options: Option<&RawBundle>| {
                    let real = super::super::background_start::RealCaller {
                        uid: real,
                        pending_intent: false,
                        bal_allowed: true,
                    };
                    host.background_start_verdict(uid, None, Some(real), false, options)
                        .map(|v| v.allows())
                };
                let options =
                    |creator, sender| {
                        RawBundle::empty().with_ints(&[
                ("android.activity.pendingIntentCreatorBackgroundActivityStartMode", creator),
                ("android.pendingIntent.backgroundActivityAllowed", sender),
            ]).unwrap()
                    };
                // No original uid process is running; a system real caller's permission
                // is usable only in ALLOW_ALWAYS, unless it is actually persistent.
                for (creator, sender, allowed) in [
                    (0, 0, false),
                    (0, 1, false),
                    (0, 2, false),
                    (0, 3, true),
                    (0, 4, false),
                ] {
                    assert_eq!(
                        as_caller(host, 10101, 1000, Some(&options(creator, sender))),
                        Ok(allowed)
                    );
                }
                assert_eq!(
                    as_caller(host, 10101, 1027, Some(&options(0, 3))),
                    Ok(false)
                );
                let f = super::super::tests::host_with_start(2, Arc::new(|| {}));
                let mut app = f.apps.lock().unwrap().remove(&2).unwrap();
                app.uid = 1000;
                app.proc_state = 0;
                host.apps.lock().unwrap().insert(2, app);
                host.tasks.app_switches.lock().unwrap().switches(true);
                for sender in [-1, 0, 1, 3, 4] {
                    assert_eq!(
                        as_caller(host, 10101, 1000, Some(&options(2, sender))),
                        Ok(sender != 4 || !additional)
                    );
                }
                // startNextMatchingActivity's real caller has neither the
                // default ALLOWED mode nor the persistent-system exemption.
                let next_matching = |sender| {
                    let real = super::super::background_start::RealCaller {
                        uid: 1000,
                        pending_intent: false,
                        bal_allowed: false,
                    };
                    host.background_start_verdict(
                        10101,
                        None,
                        Some(real),
                        false,
                        Some(&options(0, sender)),
                    )
                    .map(|v| v.allows())
                };
                assert_eq!(as_caller(host, 10101, 1000, Some(&options(0, 0))), Ok(true));
                assert_eq!(next_matching(0), Ok(false));
                assert_eq!(next_matching(3), Ok(true));
                assert_eq!(
                    as_caller(host, 10101, 1000, Some(&options(0, 2))),
                    Ok(false)
                );
                assert_eq!(as_caller(host, 1000, 10101, Some(&options(0, 2))), Ok(true));
                assert_eq!(
                    as_caller(host, 1000, 10101, Some(&options(2, 2))),
                    Ok(false)
                );
                assert_eq!(
                    as_caller(host, 1000, 10101, Some(&options(4, 2))),
                    Ok(!additional)
                );
                // Same-uid launches reuse the original allowed verdict in the pinned controller.
                assert_eq!(as_caller(host, 1000, 1000, Some(&options(2, 1))), Ok(true));
            },
        );
    }
}

/// `checkBackgroundActivityStart` of a PendingIntent: a start its
/// visible creator allows and its sender does not is allowed by the
/// creator, only the creator allowing it; a visible sender, or the
/// creator's own uid sending, allows it too.
#[test]
fn a_pending_intent_start_only_its_creator_allows_says_so() {
    use super::super::background_start::{BAL_ALLOW_VISIBLE_WINDOW, BalVerdict, RealCaller};
    crate::system::with_discovery_binders(|host, _, _, _| {
        activity_start_tests::permissions(host);
        query_owners(host);
        let run = |seq, uid| {
            let f = super::super::tests::host_with_start(seq, Arc::new(|| {}));
            let mut app = f.apps.lock().unwrap().remove(&seq).unwrap();
            app.uid = uid;
            host.apps.lock().unwrap().insert(seq, app);
        };
        run(2, 10100);
        let options = RawBundle::empty()
            .with_ints(&[(
                "android.activity.pendingIntentCreatorBackgroundActivityStartMode",
                1,
            )])
            .unwrap();
        let sent_by = |sender| {
            host.background_start_verdict(
                10100,
                Some("p"),
                Some(RealCaller::pending_intent(sender)),
                false,
                Some(&options),
            )
        };
        let by_creator = |only_creator_allows| {
            Ok(BalVerdict {
                code: BAL_ALLOW_VISIBLE_WINDOW,
                only_creator_allows,
            })
        };
        assert_eq!(sent_by(10101), by_creator(true));
        assert_eq!(sent_by(10100), by_creator(false));
        run(3, 10101);
        assert_eq!(sent_by(10101), by_creator(false));
    });
}

/// The image's declarations of the background-start changes
/// (`@EnabledAfter`), as an undeclared change is enabled.
fn declare_background_start_changes(host: &AppHost) {
    let dir = host.root().unwrap().join("system/etc/compatconfig");
    std::fs::create_dir_all(&dir).unwrap();
    let changes: String = [(296478951, 34), (244637991, 33), (320664730, 34)]
        .map(|(id, after)| format!(r#"<compat-change id="{id}" enableAfterTargetSdk="{after}"/>"#))
        .concat();
    std::fs::write(dir.join("bal.xml"), format!("<config>{changes}</config>")).unwrap();
}

/// A PendingIntent's creator and sender without a mode have their
/// privileges by default up to the target SDK rescinding them (the
/// creator's above 34, the sender's above 33), root and system never: a
/// system sender of no mode, as alarms send, does not allow a start. A
/// mode set by the other party is reset, and refused above target SDK 34
/// (`PENDING_INTENT_OPTIONS_CHECK`), when made and when sent.
#[test]
fn pending_intent_privileges_by_default_follow_the_target_sdk() {
    use super::super::background_start::{KEY_CREATOR_MODE, KEY_SENDER_MODE, RealCaller};
    for target in [33, 34, 35] {
        crate::system::with_discovery_binders_flags(&[], true, |host, publish, _, _| {
            declare_background_start_changes(host);
            activity_start_tests::permissions(host);
            query_owners(host);
            publish(&mut |p| p.target_sdk_version = target);
            for (seq, uid) in [(2, 10100), (3, 1000)] {
                let f = super::super::tests::host_with_start(seq, Arc::new(|| {}));
                let mut app = f.apps.lock().unwrap().remove(&seq).unwrap();
                app.uid = uid;
                host.apps.lock().unwrap().insert(seq, app);
            }
            let allows = |uid, package, sender| {
                let real = Some(RealCaller::pending_intent(sender));
                host.background_start_verdict(uid, package, real, false, None)
                    .unwrap()
                    .allows()
            };
            // A visible creator, a background sender.
            assert_eq!(allows(10100, Some("p"), 10101), target <= 34);
            assert!(!allows(1000, Some("android"), 10101));
            // A background creator, a visible sender.
            assert_eq!(allows(10102, None, 10100), target <= 33);
            assert!(!allows(10102, None, 1000));
            let mode = |key| RawBundle::empty().with_ints(&[(key, 1)]).unwrap();
            let reset = host.without_others_mode(mode(KEY_CREATOR_MODE), KEY_CREATOR_MODE, 10100);
            match reset {
                Ok(o) if target <= 34 => assert_eq!(o.contains(KEY_CREATOR_MODE), Ok(false)),
                Err(e) if target > 34 => assert_eq!(e.code, EX_ILLEGAL_ARGUMENT),
                other => panic!("{:?}", other.err()),
            }
            let request = |options| pending_intents::Request {
                kind: pending_intents::INTENT_SENDER_ACTIVITY,
                package: Some("p".into()),
                feature: None,
                token: None,
                result_who: None,
                request_code: 0,
                intents: Some(vec![Some(
                    RawIntent::of(None, 0, &[]).with_component("p", "p.Main"),
                )]),
                resolved_types: None,
                flags: 0x0400_0000, // FLAG_IMMUTABLE
                options,
                user: 0,
            };
            let made = host.get_intent_sender(10100, request(Some(mode(KEY_SENDER_MODE))));
            assert_eq!(made.is_ok(), target <= 34);
            let target_pi = host
                .get_intent_sender(10100, request(None))
                .unwrap()
                .unwrap();
            let sent = pending_intents::Sent {
                options: Some(mode(KEY_CREATOR_MODE)),
                ..Default::default()
            };
            let send = host.send_record(10100, None, target_pi, 0, None, None, None, None, sent);
            assert_eq!(send.is_ok(), target <= 34);
        });
    }
}

#[test]
fn as_caller_binder_cancellation_selector_and_source_process_checks() {
    activity_start_tests::with_start(|host, export| {
        fixture(host, export(Arc::new(Thread)));
        let t = source(host);
        let token = Some(Binder::Local(t));
        host.activities.with(token, |r| {
            r.launched_from.uid = 10101;
            r.launched_from.package = Some("q".into());
        });
        let intent = RawIntent::of(None, 0, &[]).with_component("p", "p.Main");
        let opt_out = RawBundle::empty()
            .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
            .unwrap();
        assert_eq!(
            caller_options(host, 1000, token, &intent, false, (Some(&opt_out), 0, 0)),
            Ok(0)
        );
        assert_eq!(host.activities.records.lock().unwrap().len(), 1); // BAL aborted.
        assert_eq!(
            caller_options(host, 1000, token, &intent, false, (None, 0, 2)),
            Ok(-96)
        );
        assert_eq!(
            caller_options(host, 1000, token, &intent, false, (None, 10, 0)),
            Ok(-2)
        );
        assert_eq!(
            caller(
                host,
                1000,
                token,
                &RawIntent::of(Some("missing"), 0, &[]),
                false
            ),
            Ok(-1)
        );
        let p = intent.parcel();
        let at = p.data().len() - 24; // selector before clip/user/extras/original/creator.
        let mut selected = Parcel::new();
        selected.write_raw(&p.data()[..at], &[]);
        selected.write_i32(1);
        RawIntent::of(None, 0, &[]).write_to(&mut selected);
        selected.write_raw(&p.data()[at + 4..], &[]);
        let selected = RawIntent::read_from(&mut selected.reader()).unwrap();
        assert!(selected.intent.selector.is_some());
        denied(caller(host, 1000, token, &selected, true));
        host.apps.lock().unwrap().get_mut(&1).unwrap().uid = 110100;
        assert_eq!(caller(host, 1000, token, &intent, false), Ok(-2)); // USER_NULL uses source user.
        let thread = host.apps.lock().unwrap().get_mut(&1).unwrap().thread.take();
        denied(caller(host, 1000, token, &intent, false));
        host.apps.lock().unwrap().get_mut(&1).unwrap().thread = thread;
        host.activities.stack.lock().unwrap().tasks.clear();
        denied(caller(host, 1000, token, &intent, false));
    });
}

// ActivityStarter.isAllowedToStart and setInitialState: a blocked
// background start goes only into a task holding its caller's activity, and
// leaves that task where it is (mCanMoveToFrontCode), as an option can ask.
#[test]
fn a_blocked_start_goes_only_into_its_callers_task_left_behind() {
    activity_start_tests::with_start(|host, export| {
        fixture(host, export(Arc::new(Thread)));
        let t = source(host);
        let token = Some(Binder::Local(t));
        host.tasks.animation_test_task(8, Default::default());
        {
            // A fullscreen display: the task left behind stays hidden.
            let mut stack = host.activities.stack.lock().unwrap();
            stack.freeform = false;
            stack.tasks.push(activity_stack::Task {
                id: 8,
                affinity: None,
                real_activity: "q/q.Main".into(),
                package: "q".into(),
                intent: Default::default(),
                base: activity_start::LaunchIntent::Component,
                uid: 10101,
                user: 0,
                last_active: 0,
                root_was_reset: false,
            });
            stack.activities.push(activity_stack::Activity::new(
                u64::MAX,
                8,
                2,
                "q/q.Main",
                0,
                0,
                None,
            ));
        }
        let intent = RawIntent::of(None, 0, &[]).with_component("p", "p.Main");
        let opt_out = RawBundle::empty()
            .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
            .unwrap();
        host.activities.with(token, |r| {
            r.launched_from.uid = 10101;
            r.launched_from.package = Some("q".into());
        });
        // handleBackgroundActivityAbort: the result forwarded into a new
        // task is cancelled to the activity that asked for it.
        host.activities
            .stack
            .lock()
            .unwrap()
            .get_mut(t)
            .unwrap()
            .result_to = Some(u64::MAX);
        let forward = RawIntent::of(None, 0x0200_0000 | 0x1000_0000, &[]) // FORWARD_RESULT|NEW_TASK
            .with_component("p", "p.Main");
        assert_eq!(
            caller_options(host, 1000, token, &forward, false, (Some(&opt_out), 0, 0)),
            Ok(0)
        );
        let results = {
            let stack = host.activities.stack.lock().unwrap();
            assert_eq!(stack.get(t).unwrap().result_to, None);
            stack.get(u64::MAX).unwrap().results.clone()
        };
        let payloads = host.activities.payloads.lock().unwrap();
        assert!(matches!(
            results[..]
                .iter()
                .map(|id| payloads.get(id))
                .collect::<Vec<_>>()[..],
            [Some(activity_start::Payload::Result {
                request_code: -1,
                result_code: 0,
                ..
            })]
        ));
        drop(payloads);
        let start = || caller_options(host, 1000, token, &intent, false, (Some(&opt_out), 0, 0));
        assert_eq!(start(), Ok(0));
        assert_eq!(host.activities.records.lock().unwrap().len(), 1); // Aborted.
        host.activities.with(token, |r| r.uid = 10101);
        assert_eq!(start(), Ok(0));
        assert_eq!(host.activities.records.lock().unwrap().len(), 2);
        let stack = host.activities.stack.lock().unwrap();
        let added = stack
            .activities
            .iter()
            .find(|a| a.token != t && a.task == 7);
        assert_eq!(
            added.map(|a| a.state),
            Some(activity_stack::State::Initializing)
        );
        assert_eq!(stack.focused(), Some(8));
        drop(stack);
        let mut p = Parcel::new();
        let avoid = [(
            "android.activity.avoidMoveToFront",
            crate::bundle::Value::Bool(true),
        )];
        crate::bundle::write(&mut p, &avoid);
        let avoid = aim_service_aidl::ReadParcelable::read_from(&mut p.reader()).unwrap();
        let options = host.activity_launch_options(Some(&avoid), 10100).unwrap();
        assert!(options.avoid_move_to_front);
    });
}

#[test]
fn as_caller_resolver_child_uses_system_visibility_but_keeps_original_identity() {
    crate::system::with_discovery_binders_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        true,
        |host, publish, _, export| {
            fixture(host, export(Arc::new(Thread)));
            publish(&mut |p| {
                p.target_sdk_version = 36;
                p.activities[0].main.exported = false;
            });
            query_owners(host);
            let t = source(host);
            let token = Some(Binder::Local(t));
            host.activities.with(token, |r| {
                r.launched_from.uid = 10101;
                r.launched_from.package = Some("q".into());
                r.class = "com.android.internal.app.ChooserActivity".into();
            });
            let intent = RawIntent::of(None, 0, &[]).with_component("p", "p.Main");
            assert_eq!(caller(host, 1000, token, &intent, true), Ok(-2)); // wrong package
            host.activities
                .with(token, |r| r.package = "android".into());
            for class in ["missing.Class", "android.app.Activity"] {
                host.activities.with(token, |r| r.class = class.into());
                assert_eq!(caller(host, 1000, token, &intent, true), Ok(-2));
            }
            for class in [
                "com.android.internal.app.ResolverActivity",
                "com.android.internal.app.ChooserActivity",
                "test.Alias",
            ] {
                host.activities.with(token, |r| {
                    r.class = class.into();
                    r.info.target_activity = (class == "test.Alias")
                        .then(|| "com.android.internal.app.ChooserActivity".into());
                });
                assert_eq!(caller(host, 1000, token, &intent, true), Ok(0));
            }
            let records = host.activities.records.lock().unwrap();
            assert_eq!(records.len(), 4);
            assert!(
                records
                    .iter()
                    .filter(|(id, _)| **id != t)
                    .all(|(_, r)| r.launched_from.uid == 10101
                        && r.launched_from.package.as_deref() == Some("q"))
            );
        },
    );
}

// ATMS.startNextMatchingActivity: query as Binder caller, launch as source process.
#[test]
fn next_matching_order_visibility_caller_and_result_transfer() {
    crate::system::with_discovery_binders_flags(&[], true, |host, publish, _, export| {
        publish(&mut |p| {
            let mut f = crate::package::intent_filter::ParsedIntentInfo::default();
            f.has_default = true;
            f.filter.add_action("test.NEXT");
            f.filter.add_category("android.intent.category.DEFAULT");
            f.filter.add_data_scheme("https");
            f.filter.add_data_type("image/png").unwrap();
            let mut a = p.activities[0].clone();
            p.activities.clear();
            for (class, priority) in [("p.Main", 3), ("p.Next", 2), ("p.Last", 1)] {
                a.main.component.name = class.into();
                f.filter.priority = priority;
                a.main.component.intents = vec![f.clone()];
                p.activities.push(a.clone());
            }
        });
        fixture(host, export(Arc::new(Thread)));
        query_owners(host);
        let t = source(host);
        let anchor = host.new_activity(
            1,
            10100,
            "p",
            host.activity_info_of("p", "p.Main").unwrap(),
            Default::default(),
            7,
            activity_start::StartRequest::launcher("p.Anchor"),
        );
        host.activities
            .stack
            .lock()
            .unwrap()
            .activities
            .push(activity_stack::Activity::new(
                anchor,
                7,
                1,
                "p/p.Anchor",
                0,
                0,
                None,
            ));
        let original = RawIntent::with_data(
            Some("test.NEXT"),
            Some(&crate::package::uri::Uri::parse("https://original/")),
            None,
            0,
            &[],
        );
        host.activities.with(Some(Binder::Local(t)), |r| {
            r.intent = LaunchIntent::Raw(Box::new(kept::KeptIntent::new(original, &host.process)));
            r.resolved_type = Some("image/png".into());
            r.launched_from.uid = 1000;
            r.result_to = Some(anchor);
        });
        let next_intent = RawIntent::with_data(
            Some("test.NEXT"),
            Some(&crate::package::uri::Uri::parse("wrong://changed/")),
            None,
            0x1e000000,
            &[],
        );
        let next = |uid, token| next(host, uid, token, Some(&next_intent));
        assert_eq!(next(10100, None), Ok(0));
        assert_eq!(next(10100, Some(Binder::Local(u64::MAX))), Ok(0));
        assert_eq!(next(10101, Some(Binder::Local(t))), Ok(0)); // source is invisible
        assert_eq!(next(10100, Some(Binder::Local(t))), Ok(1));
        let n = host
            .activities
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|(_, r)| r.class == "p.Next")
            .map(|(t, r)| {
                let LaunchIntent::Raw(k) = &r.intent else {
                    panic!()
                };
                assert_eq!(
                    k.intent.intent.data,
                    Some(crate::package::uri::Uri::parse("https://original/"))
                );
                assert_eq!(k.intent.intent.flags & 0x0e000000, 0);
                assert_eq!(r.launched_from.uid, 10100);
                assert_eq!(r.resolved_type.as_deref(), Some("image/png"));
                *t
            })
            .unwrap();
        assert!(
            !host
                .activities
                .stack
                .lock()
                .unwrap()
                .get(t)
                .unwrap()
                .finishing
        );
        host.activities.with(Some(Binder::Local(n)), |r| {
            r.result_to = Some(t);
            r.result_who = Some("reply".into());
            r.request_code = 8;
        });
        assert_eq!(next(10100, Some(Binder::Local(n))), Ok(1));
        let last = host
            .activities
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|(_, r)| r.class == "p.Last")
            .map(|(last, r)| {
                assert_eq!(
                    (r.result_to, r.result_who.as_deref(), r.request_code),
                    (Some(t), Some("reply"), 8)
                );
                *last
            });
        assert!(last.is_some());
        assert_eq!(next(10100, Some(Binder::Local(last.unwrap()))), Ok(0));
        // Target security uses the attached process, never launchedFromUid (the system here).
        publish(&mut |p| p.activities[1].main.exported = false);
        host.apps.lock().unwrap().get_mut(&1).unwrap().uid = 10101;
        host.activities
            .with(Some(Binder::Local(t)), |r| r.result_to = Some(anchor));
        {
            let mut stack = host.activities.stack.lock().unwrap();
            stack.get_mut(t).unwrap().result = Some(44);
            stack.get_mut(anchor).unwrap().results = vec![44, 45];
        }
        let e = next(10100, Some(Binder::Local(t))).unwrap_err();
        assert_eq!(e.code, EX_SECURITY);
        assert!(e.message.contains("uid=10101"));
        let stack = host.activities.stack.lock().unwrap();
        assert!(stack.get(t).unwrap().finishing); // exceptions do not restore finishing
        assert_eq!(stack.get(anchor).unwrap().results, [45]);
        assert_eq!(stack.get(t).unwrap().result_to, None);
        drop(stack);
        host.apps.lock().unwrap().get_mut(&1).unwrap().thread = None;
        assert_eq!(next(10100, Some(Binder::Local(t))), Ok(0));
        host.activities.stack.lock().unwrap().tasks.clear();
        assert_eq!(next(10100, Some(Binder::Local(t))), Ok(0));
    });
}

#[test]
fn next_matching_null_and_fd_checks_precede_only_the_original_caller_checks() {
    activity_start_tests::with_start(|host, export| {
        fixture(host, export(Arc::new(Thread)));
        let t = source(host);
        let token = Some(Binder::Local(t));
        assert_eq!(next(host, 10100, None, None), Ok(0));
        assert_eq!(
            next(host, 10100, token, None).unwrap_err().code,
            aim_binder_host::parcel::EX_NULL_POINTER
        );
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_i32(crate::bundle::MAGIC);
        p.write_i32(1);
        p.write_string16(Some("fd"));
        p.write_i32(4);
        let at = p.data().len();
        p.write_i32(0);
        p.write_string16(Some("android.os.ParcelFileDescriptor"));
        p.write_file(Arc::new(std::fs::File::open("/dev/null").unwrap()));
        p.set_i32_at(at, (p.data().len() - at - 4) as i32);
        p.set_i32_at(0, (p.data().len() - 8) as i32);
        p.write_bool(false);
        let fd = RawIntent::of(None, 0, &[])
            .with_extras_bundle(&RawBundle::read_from(&mut p.reader()).unwrap())
            .unwrap();
        assert!(fd.has_fds());
        for token in [None, token] {
            assert_eq!(
                next(host, 10100, token, Some(&fd)).unwrap_err().code,
                EX_ILLEGAL_ARGUMENT
            );
        }
        host.apps.lock().unwrap().get_mut(&1).unwrap().thread = None;
        assert_eq!(next(host, 10100, token, None), Ok(0));
        host.activities.stack.lock().unwrap().tasks.clear();
        assert_eq!(next(host, 10100, token, None), Ok(0));
    });
}

#[test]
fn with_config_component_exception_precedes_configuration_permission_and_publication() {
    crate::system::with_discovery_binders_flags(&[], true, |host, publish, _, export| {
        fixture(host, export(Arc::new(Thread)));
        let t = source(host);
        publish(&mut |p| p.activities[0].main.exported = false);
        let old = host.global_configuration().unwrap();
        let a = atm::StartActivityWithConfig::<RawIntent, config::Configuration, RawBundle> {
            caller: None,
            calling_package: Some("q".into()),
            calling_feature_id: None,
            intent: Some(RawIntent::of(None, 0, &[]).with_component("p", "p.Main")),
            resolved_type: None,
            result_to: Some(Binder::Local(t)),
            result_who: None,
            request_code: -1,
            start_flags: 0,
            new_config: Some(config::Configuration {
                font_scale: 1.25,
                ..config::Configuration::from_proto(&[]).unwrap()
            }),
            options: None,
            user_id: 0,
        };
        let mut p = Parcel::new();
        a.write(&mut p);
        let e = invoke(host, 10101, atm::START_ACTIVITY_WITH_CONFIG, &p).unwrap_err();
        assert_eq!(e.code, EX_SECURITY);
        assert!(e.message.contains("not exported"));
        assert_eq!(host.global_configuration().unwrap(), old);
        assert_eq!(host.activities.records.lock().unwrap().len(), 1);
    });
}

// ActivityStarter.execute: admission, CHANGE_CONFIGURATION, publication, then client launch.
#[test]
fn with_config_failure_order_publication_and_configuration_ownership() {
    struct Events(std::sync::mpsc::Sender<String>);
    impl Service for Events {
        fn descriptor(&self) -> &str {
            thread::DESCRIPTOR
        }
        fn transact(&self, c: &mut Call<'_>) -> Reply {
            if c.code == thread::SCHEDULE_TRANSACTION {
                c.data.enforce_interface(thread::DESCRIPTOR)?;
                c.data.read_i32()?;
                if c.data.read_i32()? > 0 {
                    self.0.send(c.data.read_string16()?.unwrap()).unwrap();
                }
            }
            Ok(Parcel::new())
        }
    }
    activity_start_tests::with_start(|host, export| {
        let (tx, rx) = std::sync::mpsc::channel();
        fixture(host, export(Arc::new(Events(tx))));
        host.services();
        settings_provider::prepare_shell_test(host);
        let t = source(host);
        let old = host.global_configuration().unwrap();
        let mut a = atm::StartActivityWithConfig::<RawIntent, config::Configuration, RawBundle> {
            caller: None,
            calling_package: Some("p".into()),
            calling_feature_id: None,
            intent: Some(RawIntent::of(None, 0, &[]).with_component("p", "missing")),
            resolved_type: None,
            result_to: Some(Binder::Local(t)),
            result_who: None,
            request_code: -1,
            start_flags: 0,
            new_config: Some(config::Configuration {
                font_scale: 1.25,
                locales: "ja-JP".into(),
                ..config::Configuration::from_proto(&[]).unwrap()
            }),
            options: None,
            user_id: 0,
        };
        let send =
            |uid, a: &atm::StartActivityWithConfig<RawIntent, config::Configuration, RawBundle>| {
                let mut p = Parcel::new();
                a.write(&mut p);
                invoke(host, uid, atm::START_ACTIVITY_WITH_CONFIG, &p)
            };
        denied(send(10100, &a)); // even a normal resolution failure enforces the permission
        assert_eq!(host.global_configuration().unwrap(), old);
        a.intent = None;
        assert_eq!(
            send(10100, &a).unwrap_err().code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
        a.intent = Some(RawIntent::of(None, 0, &[]).with_component("p", "p.Main"));
        a.calling_package = Some("q".into());
        denied(send(10100, &a));
        a.calling_package = Some("p".into());
        a.user_id = 10;
        denied(send(10100, &a));
        a.user_id = 0;
        assert_eq!(send(1000, &a), Ok(0));
        let mut events = Vec::new();
        loop {
            let event = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
            let launched = event.ends_with("LaunchActivityItem");
            events.push(event);
            if launched {
                break;
            }
        }
        let config = events
            .iter()
            .position(|e| e.ends_with("ConfigurationChangeItem"))
            .unwrap();
        let launch = events
            .iter()
            .position(|e| e.ends_with("LaunchActivityItem"))
            .unwrap();
        assert!(config < launch);
        let inputs = host.platform().unwrap();
        assert_ne!(
            inputs.settings.get(settings::Table::System, "font_scale"),
            Some("1.25".into())
        );
        let current = host.global_configuration().unwrap();
        assert_eq!(
            (current.font_scale, current.locales.as_str()),
            (1.25, "ja-JP")
        );
        host.apply_persistent_configuration(&config::Configuration {
            font_scale: 1.25,
            ..config::Configuration::from_proto(&[]).unwrap()
        })
        .unwrap();
        assert_eq!(
            inputs.settings.get(settings::Table::System, "font_scale"),
            Some("1.25".into())
        );
        assert_eq!(host.global_configuration().unwrap().seq, current.seq);
        host.apply_configuration(
            &config::Configuration {
                font_scale: 1.5,
                ..config::Configuration::from_proto(&[]).unwrap()
            },
            false,
        )
        .unwrap();
        host.apply_persistent_configuration(&config::Configuration {
            locales: "en-US".into(),
            ..config::Configuration::from_proto(&[]).unwrap()
        })
        .unwrap();
        assert_eq!(host.global_configuration().unwrap().font_scale, 1.5);
        let provider = host.settings_provider().unwrap();
        provider
            .call(
                &settings_provider::Caller {
                    uid: 0,
                    package: Some("root".into()),
                },
                "PUT_system",
                Some("font_scale"),
                &HashMap::from([(
                    "value".into(),
                    crate::bundle::Value::String(Some("1.75".into())),
                )]),
                None,
            )
            .unwrap();
        assert_eq!(host.global_configuration().unwrap().font_scale, 1.75);
        let current = host.global_configuration().unwrap();
        let mut ordinary = activity_start_tests::args(
            Some("p"),
            0,
            Some(RawIntent::of(None, 0x10008000, &[]).with_component("p", "p.Main")),
        );
        ordinary.flags = 1;
        ordinary.result_to = Some(Binder::Local(t));
        let mut p = Parcel::new();
        ordinary.write(&mut p);
        assert_eq!(invoke(host, 10100, atm::START_ACTIVITY_AS_USER, &p), Ok(1));
        a.new_config = Some(current.clone());
        assert_eq!(send(10100, &a), Ok(0)); // no change needs no CHANGE_CONFIGURATION
        assert_eq!(host.global_configuration().unwrap().seq, current.seq);
        a.new_config = Some(config::Configuration {
            font_scale: 2.0,
            ..config::Configuration::from_proto(&[]).unwrap()
        });
        a.intent = Some(RawIntent::of(None, 0, &[]).with_component("p", "missing"));
        assert_eq!(send(1000, &a), Ok(-2)); // normal failure still applies the configuration
        assert_eq!(host.global_configuration().unwrap().font_scale, 2.0);
        a.caller = Some(Binder::Handle(u32::MAX));
        a.new_config.as_mut().unwrap().font_scale = 2.25;
        denied(send(10100, &a));
        assert_eq!(send(1000, &a), Ok(-94));
        assert_eq!(host.global_configuration().unwrap().font_scale, 2.25);
        a.caller = None;
        a.intent = Some(RawIntent::of(None, 0, &[]).with_component("p", "p.Main"));
        a.new_config.as_mut().unwrap().font_scale = 2.5;
        let before = host.activities.records.lock().unwrap().len();
        let old = host.global_configuration().unwrap();
        let _: Vec<_> = rx.try_iter().collect();
        let e = send(10100, &a).unwrap_err();
        assert!(e.message.contains("CHANGE_CONFIGURATION"));
        assert_eq!(host.activities.records.lock().unwrap().len(), before + 1);
        assert_eq!(host.global_configuration().unwrap(), old);
    });
}
