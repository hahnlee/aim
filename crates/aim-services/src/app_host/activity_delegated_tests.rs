//! ATMS.startActivityAsCaller / ActivityStarter, android-16.0.0_r1.
use super::super::*;
use super::*;
use aim_binder_host::{
    local::{Service, Strong},
    parcel::EX_SECURITY,
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
                        host.as_caller_background_allowed(
                            10101,
                            1000,
                            Some(&options(creator, sender))
                        ),
                        Ok(allowed)
                    );
                }
                assert_eq!(
                    host.as_caller_background_allowed(10101, 1027, Some(&options(0, 3))),
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
                        host.as_caller_background_allowed(10101, 1000, Some(&options(2, sender))),
                        Ok(sender != 4 || !additional)
                    );
                }
                assert_eq!(
                    host.as_caller_background_allowed(10101, 1000, Some(&options(0, 2))),
                    Ok(false)
                );
                assert_eq!(
                    host.as_caller_background_allowed(1000, 10101, Some(&options(0, 2))),
                    Ok(true)
                );
                assert_eq!(
                    host.as_caller_background_allowed(1000, 10101, Some(&options(2, 2))),
                    Ok(false)
                );
                assert_eq!(
                    host.as_caller_background_allowed(1000, 10101, Some(&options(4, 2))),
                    Ok(!additional)
                );
                // Same-uid launches reuse the original allowed verdict in the pinned controller.
                assert_eq!(
                    host.as_caller_background_allowed(1000, 1000, Some(&options(2, 1))),
                    Ok(true)
                );
            },
        );
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
