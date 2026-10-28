//! PackageInternalEventsBridge / ActivityManagerInternal at android-16.0.0_r1.
use super::super::package_writes::{DataCleared, PackageMonitor};
use super::*;
use aim_binder_host::local::{Reply, Service};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const RESTARTED: &str = "android.intent.action.PACKAGE_RESTARTED";
const CLEARED: &str = "android.intent.action.PACKAGE_DATA_CLEARED";
const ACCESS: &str = "android.permission.ACCESS_INSTANT_APPS";

struct AppThread(mpsc::Sender<(RawIntent, i32)>);
impl Service for AppThread {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, thread::SCHEDULE_REGISTERED_RECEIVER);
        let a = thread::ScheduleRegisteredReceiver::<RawIntent, RawBundle>::read(&mut call.data)?;
        assert!(a.assume_delivered && !a.ordered && !a.sticky);
        self.0.send((a.intent.unwrap(), a.sending_user)).unwrap();
        Ok(Parcel::new())
    }
}

fn register(
    host: &Arc<AppHost>,
    export: &dyn Fn(Arc<dyn Service>) -> Strong,
    package: &str,
    uid: i32,
) -> mpsc::Receiver<(RawIntent, i32)> {
    let (send, receive) = mpsc::channel();
    let thread = export(Arc::new(AppThread(send)));
    let binder = thread.binder();
    let seq = i64::from(uid);
    host.apps.lock().unwrap().insert(
        seq,
        super::super::App {
            package: package.into(),
            packages: vec![package.into()],
            process: package.into(),
            uid: uid as u32,
            package_uid: uid as u32,
            isolated: false,
            entry_point: None,
            crash_handler: None,
            application_info: None,
            pid: uid,
            started: Instant::now(),
            start_elapsed: 0,
            start_uptime: 0,
            thread: Some(thread),
            activities: vec![],
            config: None,
            providers: vec![],
            removed: false,
            first_frame: None,
            last_activity: 0,
            persistent: false,
            package_deps: vec![],
            last_mem_info: None,
            proc_state: PROCESS_STATE_TOP,
            oom_adj: 0,
        },
    );
    let mut filter = IntentFilter::default();
    filter.add_action(RESTARTED);
    filter.add_action(CLEARED);
    filter.add_data_scheme("package");
    let data = Parcel::new();
    host.broadcasts
        .register(
            host,
            &Call {
                code: am::REGISTER_RECEIVER,
                flags: 0,
                sender_pid: uid,
                sender_euid: uid as u32,
                data: data.reader(),
            },
            Registration {
                caller: Some(binder),
                caller_package: Some(package.into()),
                feature_id: None,
                receiver_id: None,
                receiver: Some(binder),
                filter: Some(ParcelFilter(filter)),
                permission: None,
                user: uid / PER_USER_RANGE as i32,
                flags: RECEIVER_EXPORTED,
            },
        )
        .unwrap();
    receive
}

/// BroadcastController.registerReceiverWithFeature and
/// ContentProviderHelper.generateApplicationProvidersLocked, android-16.0.0_r1:
/// a package of the same uid becomes a caller only once hosted in the process.
#[test]
fn shared_process_receiver_registration_uses_all_hosted_packages() {
    crate::system::with_discovery_binders(|host, publish, _, export| {
        publish(&mut |package| {
            if package.package_name == "p" {
                let mut provider = crate::package::pkg::Provider {
                    main: package.activities[0].main.clone(),
                    authority: Some("shared.provider".into()),
                    ..Default::default()
                };
                provider.main.component.name = "p.SharedProvider".into();
                provider.main.process_name = Some("shared.process".into());
                provider.main.direct_boot_aware = true;
                package.providers.push(provider);
            }
        });
        // A ProcessRecord initially bound to q, with p's uid, as for a shared uid.
        let _receivers = register(host, export, "q", 10100);
        let binder = host.apps.lock().unwrap()[&10100]
            .thread
            .as_ref()
            .unwrap()
            .binder();
        let data = Parcel::new();
        let call = Call {
            code: am::REGISTER_RECEIVER,
            flags: 0,
            sender_pid: 10100,
            sender_euid: 10100,
            data: data.reader(),
        };
        let register_package = |package: &str| {
            let mut filter = IntentFilter::default();
            filter.add_action(RESTARTED);
            host.broadcasts.register(
                host,
                &call,
                Registration {
                    caller: Some(binder),
                    caller_package: Some(package.into()),
                    feature_id: None,
                    receiver_id: None,
                    receiver: Some(binder),
                    filter: Some(ParcelFilter(filter)),
                    permission: None,
                    user: 0,
                    flags: RECEIVER_EXPORTED,
                },
            )
        };
        assert!(register_package("q").is_ok());
        // The package exists with this uid, but has not run in the process.
        assert_eq!(
            register_package("p").err().unwrap().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        let providers = host.process_providers("shared.process", 10100).unwrap();
        assert_eq!(providers.len(), 1);
        {
            let mut apps = host.apps.lock().unwrap();
            let app = apps.get_mut(&10100).unwrap();
            app.add_provider(providers[0].clone());
            app.add_provider(providers[0].clone());
            assert_eq!(app.packages, ["q", "p"]);
            app.package_deps.push("unloaded.dependency".into());
        }
        assert!(register_package("p").is_ok());
        assert_eq!(
            register_package("unloaded.dependency").err().unwrap().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        assert_eq!(
            register_package("unrelated").err().unwrap().code,
            aim_binder_host::parcel::EX_SECURITY
        );
    });
}

#[derive(Default)]
struct Monitor(Mutex<Vec<(&'static str, String, i32, i32)>>);
impl PackageMonitor for Monitor {
    fn package_restarted(&self, package: &str, uid: i32, user: i32) {
        self.0
            .lock()
            .unwrap()
            .push((RESTARTED, package.into(), uid, user));
    }
    fn package_data_cleared(&self, cleared: &DataCleared<'_>) {
        self.0
            .lock()
            .unwrap()
            .push((CLEARED, cleared.package.into(), cleared.uid, cleared.user));
    }
}

struct Callback(mpsc::Sender<RawIntent>);
impl Service for Callback {
    fn descriptor(&self) -> &str {
        "android.os.IRemoteCallback"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data.enforce_interface(self.descriptor())?;
        assert_eq!(call.data.read_i32()?, 1);
        let values = bundle::read(&mut call.data)?.unwrap();
        let Value::Lazy { start, .. } =
            values["android.content.pm.extra.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT"]
        else {
            panic!("Intent callback");
        };
        call.data.set_position(start);
        assert_eq!(
            call.data.read_string16()?.as_deref(),
            Some("android.content.Intent")
        );
        self.0.send(RawIntent::read_from(&mut call.data)?).unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn internal_package_events_deliver_once_to_registered_native_and_callback_receivers() {
    for asynchronous in [false, true] {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let mut framework =
                super::super::resources::Framework::load(host.root().unwrap(), 160, "en-US")
                    .unwrap();
            framework
                .0
                .flags
                .insert("android.content.pm.stay_stopped".into(), asynchronous);
            assert!(host.framework.set(framework).is_ok());
            let registered = register(host, export, "q", 10101);
            let other_user = register(host, export, "q", 1010101);
            let native = Arc::new(Monitor::default());
            let other_native = Arc::new(Monitor::default());
            host.package_monitors.register(
                0,
                Arc::downgrade(&(native.clone() as Arc<dyn PackageMonitor>)),
            );
            host.package_monitors.register(
                10,
                Arc::downgrade(&(other_native.clone() as Arc<dyn PackageMonitor>)),
            );
            let system = host.package_system().unwrap();
            let events = system.package_events().unwrap();
            let (send, callbacks) = mpsc::channel();
            events
                .register(
                    Some(host.process.add_service(Arc::new(Callback(send)))),
                    0,
                    1000,
                )
                .unwrap();
            for action in [RESTARTED, CLEARED] {
                if action == RESTARTED {
                    let (ready, blocked) = mpsc::channel();
                    let (release, resume) = mpsc::channel();
                    events
                        .post(
                            false,
                            Box::new(move || {
                                ready.send(()).unwrap();
                                resume.recv_timeout(Duration::from_secs(2)).unwrap();
                            }),
                        )
                        .unwrap();
                    blocked.recv_timeout(Duration::from_secs(2)).unwrap();
                    system
                        .internal_send_package_restarted_broadcast(
                            Some("p".into()),
                            10100,
                            0x20,
                            1000,
                            1,
                        )
                        .unwrap();
                    assert_eq!(native.0.lock().unwrap().len(), usize::from(!asynchronous));
                    release.send(()).unwrap();
                } else {
                    system
                        .internal_send_package_data_cleared_broadcast(
                            Some("p".into()),
                            10100,
                            0,
                            true,
                            false,
                            1000,
                            1,
                        )
                        .unwrap();
                }
                let (intent, user) = registered.recv_timeout(Duration::from_secs(2)).unwrap();
                let callback = callbacks.recv_timeout(Duration::from_secs(2)).unwrap();
                assert_eq!(intent.intent.action.as_deref(), Some(action));
                assert_eq!(user, 0);
                assert_eq!(intent.intent.data.as_ref().unwrap().as_str(), "package:p");
                assert_eq!(intent.test_extras(), callback.test_extras());
                assert_eq!(
                    callback.intent.flags,
                    if action == RESTARTED {
                        0x04000020
                    } else {
                        0x05000000
                    }
                );
                if action == RESTARTED {
                    assert_eq!(
                        intent
                            .test_extras()
                            .contains_key("android.intent.extra.TIME"),
                        asynchronous
                    );
                } else {
                    assert_eq!(
                        intent.test_extras()["android.intent.extra.IS_RESTORE"],
                        Value::Bool(true)
                    );
                }
            }
            assert!(events.wait(2000, false).unwrap());
            assert_eq!(
                *native.0.lock().unwrap(),
                [
                    (RESTARTED, "p".into(), 10100, 0),
                    (CLEARED, "p".into(), 10100, 0)
                ]
            );
            assert!(other_native.0.lock().unwrap().is_empty());
            assert!(registered.try_recv().is_err());
            assert!(other_user.try_recv().is_err());
            assert!(callbacks.try_recv().is_err());
        });
    }
}

#[test]
fn internal_instant_package_events_require_access_instant_apps_and_the_allow_list() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        for (path, xml) in [
            (
                "misc/apexdata/com.android.permission/access.abx",
                format!(
                    r#"<access><permissions><permission name="{ACCESS}" packageName="android" protectionLevel="2" type="0"/></permissions></access>"#
                ),
            ),
            (
                "misc_de/0/apexdata/com.android.permission/access.abx",
                format!(
                    r#"<access><app-id-permissions><app-id id="10100"><permission name="{ACCESS}" flags="4"/></app-id></app-id-permissions></access>"#
                ),
            ),
        ] {
            let path = host.data().unwrap().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, xml).unwrap();
        }
        let granted = register(host, export, "p", 10100);
        let denied = register(host, export, "q", 10101);
        let leaf = super::super::package_internal_events::InternalEvents::new(host.clone());
        let intent = super::super::package_writes::data_cleared_intent("p", 10100, 0, false, true);
        for (instant, allow, expected, other_expected) in [
            (false, None, true, true),
            (true, None, true, false),
            (true, Some(vec![]), false, false),
            (true, Some(vec![10100, 10101]), true, false),
            (true, Some(vec![10101]), false, false),
        ] {
            use aim_service_aidl::dev_aim_server_ipackageinternaleventsbridge as api;
            let mut request = Parcel::new();
            api::Broadcast {
                intent: Some(intent.parcel().data().to_vec()),
                user_id: 0,
                instant,
                allow_list: allow,
            }
            .write(&mut request);
            let response = leaf
                .transact(&mut Call {
                    code: api::BROADCAST,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 1000,
                    data: request.reader(),
                })
                .unwrap();
            api::read_broadcast_reply(&mut response.reader())
                .unwrap()
                .unwrap();
            if expected {
                let (received, _) = granted.recv_timeout(Duration::from_secs(2)).unwrap();
                assert_eq!(
                    received.test_extras()["android.intent.extra.PACKAGE_NAME"],
                    Value::String(Some("p".into()))
                );
            } else {
                assert!(granted.recv_timeout(Duration::from_millis(100)).is_err());
            }
            if other_expected {
                denied.recv_timeout(Duration::from_secs(2)).unwrap();
            } else {
                assert!(denied.recv_timeout(Duration::from_millis(100)).is_err());
            }
            assert!(granted.try_recv().is_err());
            assert!(denied.try_recv().is_err());
        }
    });
}
