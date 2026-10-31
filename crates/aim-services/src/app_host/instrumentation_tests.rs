//! am/Instrument.run and AMS instrumentation contracts, android-16.0.0_r1.
use super::webview_zygote::pending_start_tests::attach;
use super::*;
use crate::package::{intent::ComponentName, pkg};
use aim_service_aidl::android_app_iinstrumentationwatcher as watcher;
use aim_service_aidl::android_app_iuiautomationconnection as ui;
use kept::RawBundle;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;

type Publish<'a> = dyn FnMut(&mut dyn FnMut(&mut pkg::AndroidPackage)) + 'a;
type Export<'a> = dyn Fn(Arc<dyn Service>) -> Strong + 'a;
fn with(test: impl FnOnce(&Arc<AppHost>, &mut Publish<'_>, &Export<'_>)) {
    crate::system::with_discovery_binders_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        true,
        |host, publish, _, export| {
            let pm = permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
            test(host, publish, export);
        },
    );
}
type Request = am::StartInstrumentation<ComponentName, RawBundle>;
type Event = (u32, i32, Option<RawBundle>);
struct Recorder(Weak<AppHost>, mpsc::Sender<Event>, &'static str);
impl Service for Recorder {
    fn descriptor(&self) -> &str {
        self.2
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let host = self.0.upgrade().unwrap();
        let mut p = Parcel::new();
        let mut code = 0;
        let mut bundle = None;
        if self.2 == watcher::DESCRIPTOR {
            // A synchronous callback can reenter the native process owner.
            drop(host.apps.lock().unwrap());
            let a =
                watcher::InstrumentationStatus::<ComponentName, RawBundle>::read(&mut call.data)?;
            code = a.result_code;
            bundle = a.results;
            p.write_no_exception();
        } else if call.code == thread::BIND_APPLICATION {
            call.data.enforce_interface(thread::DESCRIPTOR)?;
            assert_eq!(call.data.read_string16()?.as_deref(), Some("p"));
            assert_eq!(call.data.read_i32()?, 1);
            let mut info = Parcel::new();
            host.launch_inputs("p")
                .unwrap()
                .application
                .write_to(&mut info);
            call.data
                .set_position(call.data.position() + info.data().len());
            call.data.read_string16()?;
            call.data.read_string16()?;
            call.data.read_bool()?;
            assert_eq!(call.data.read_i32()?, 1); // ProviderInfoList
            assert_eq!(call.data.read_i32()?, 0);
            assert_eq!(call.data.read_i32()?, 1);
            assert_eq!(ComponentName::read_from(&mut call.data)?.class, "p.Test");
            assert_eq!(call.data.read_i32()?, 1);
            activity::ProfilerInfo::read_from(&mut call.data)?;
            assert_eq!(call.data.read_i32()?, 1);
            let args = RawBundle::read_from(&mut call.data)?;
            assert_eq!(args.int("argument", -1)?, 7);
            assert!(call.data.read_binder()?.is_some());
            assert!(call.data.read_binder()?.is_some());
        } else if call.code == thread::INSTRUMENT_WITHOUT_RESTART {
            call.data.enforce_interface(thread::DESCRIPTOR)?;
            assert_eq!(call.data.read_i32()?, 1);
            assert_eq!(ComponentName::read_from(&mut call.data)?.class, "p.Test");
        } else if self.2 == AGENT_DESCRIPTOR {
            call.data.enforce_interface(AGENT_DESCRIPTOR)?;
            assert!(call.data.read_i32()? > 0);
            p.write_no_exception();
        }
        self.1.send((call.code, code, bundle)).unwrap();
        Ok(p)
    }
}
fn recorders(
    host: &Arc<AppHost>,
    export: &Export<'_>,
) -> (Strong, Strong, Strong, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let make = |descriptor| {
        export(Arc::new(Recorder(
            Arc::downgrade(host),
            tx.clone(),
            descriptor,
        )))
    };
    let w = make(watcher::DESCRIPTOR);
    let ui = make(ui::DESCRIPTOR);
    let thread = make(thread::DESCRIPTOR);
    host.attach_agent(make(AGENT_DESCRIPTOR));
    (w, ui, thread, rx)
}
fn recv(rx: &mpsc::Receiver<Event>) -> Event {
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}
fn call(host: &Arc<AppHost>, uid: u32, code: u32, p: &Parcel) -> Parcel {
    Activity(host.clone())
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 42,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap()
}
fn start(host: &Arc<AppHost>, uid: u32, a: Request) -> Result<bool, Exception> {
    let mut p = Parcel::new();
    a.write(&mut p);
    let reply = call(host, uid, am::START_INSTRUMENTATION, &p);
    am::read_start_instrumentation_reply(&mut reply.reader()).unwrap()
}
fn request(flags: i32, watcher: Binder, connection: Binder) -> Request {
    let args = object_bundle(|p| p.write_binder(Some(watcher)));
    am::StartInstrumentation {
        class_name: Some(ComponentName {
            package: "p".into(),
            class: "p.Test".into(),
        }),
        profile_file: Some("/data/local/tmp/test.trace".into()),
        flags,
        arguments: Some(args.with_ints(&[("argument", 7)]).unwrap()),
        watcher: Some(watcher),
        connection: Some(connection),
        user_id: 0,
        abi_override: Some("arm64-v8a".into()),
    }
}
fn manifest(p: &mut pkg::AndroidPackage, processes: Option<&str>) {
    if p.package_name == "p" {
        p.process_name = Some("p".into());
        p.instrumentations = vec![pkg::Instrumentation {
            component: pkg::Component {
                name: "p.Test".into(),
                package_name: "p".into(),
                ..Default::default()
            },
            target_package: Some("p".into()),
            target_processes: processes.map(Into::into),
            ..Default::default()
        }];
    }
}
fn object_bundle(write: impl FnOnce(&mut Parcel)) -> RawBundle {
    let mut p = Parcel::new();
    p.write_i32(0);
    p.write_i32(crate::bundle::MAGIC);
    p.write_i32(1);
    p.write_string16(Some("object"));
    p.write_i32(4);
    let at = p.data().len();
    p.write_i32(0);
    p.write_string16(Some("test.Payload"));
    write(&mut p);
    p.set_i32_at(at, (p.data().len() - at - 4) as i32);
    p.set_i32_at(0, (p.data().len() - 8) as i32);
    p.write_bool(false);
    RawBundle::read_from(&mut p.reader()).unwrap()
}
fn insert(host: &AppHost, seq: i64, thread: Option<Strong>, name: &str) {
    let fixture = tests::host_with_start(seq, Arc::new(|| {}));
    let mut app = fixture.apps.lock().unwrap().remove(&seq).unwrap();
    app.package = name.into();
    app.process = name.into();
    app.packages = vec![name.into()];
    app.uid = 10100;
    app.package_uid = 10100;
    app.isolated = false;
    app.thread = thread;
    app.pid = 0;
    app.entry_point = None;
    host.apps.lock().unwrap().insert(seq, app);
}
fn results(
    host: &Arc<AppHost>,
    target: Binder,
    code: Option<i32>,
    results: Option<RawBundle>,
) -> Result<(), Exception> {
    let mut p = Parcel::new();
    let transaction = if let Some(result_code) = code {
        am::FinishInstrumentation {
            target: Some(target),
            result_code,
            results,
        }
        .write(&mut p);
        am::FINISH_INSTRUMENTATION
    } else {
        am::AddInstrumentationResults {
            target: Some(target),
            results,
        }
        .write(&mut p);
        am::ADD_INSTRUMENTATION_RESULTS
    };
    let p = call(host, 10100, transaction, &p);
    p.reader().read_exception().unwrap()
}

#[test]
fn instrumentation_admission_and_no_restart_results_cleanup() {
    with(|host, publish, export| {
        let (w, ui, t, rx) = recorders(host, export);
        let who = t.binder();
        assert!(!start(host, 0, request(8, w.binder(), ui.binder())).unwrap());
        let (_, code, bundle) = recv(&rx);
        assert_eq!(code, -1);
        assert!(bundle.unwrap().contains("Error").unwrap());
        publish(&mut |p| manifest(p, Some("*")));
        for (uid, flags) in [(99001, 8), (0, 32), (0, 64)] {
            assert!(start(host, uid, request(flags, w.binder(), ui.binder())).is_err());
        }
        let fd_bundle =
            object_bundle(|p| p.write_file(Arc::new(std::fs::File::open("/dev/null").unwrap())));
        let mut a = request(8, w.binder(), ui.binder());
        a.arguments = Some(fd_bundle.clone());
        assert_eq!(
            start(host, 0, a).unwrap_err().code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
        for code in [None, Some(0)] {
            assert!(results(host, who, code, Some(fd_bundle.clone())).is_err());
        }
        insert(host, 100, Some(t), "p");
        let probe = super::input_manager::injection_tests::permission_probe;
        assert_eq!(probe(host, 10100), -1); // no INJECT_EVENTS grant
        assert!(start(host, 0, request(10, w.binder(), ui.binder())).unwrap());
        assert_eq!(host.instrumentation_source(10100), Some(0));
        assert_eq!(probe(host, 10100), -4); // source grant passes permission, then rejects null

        assert_eq!(host.instrumentation_source(110100), None);
        assert_eq!(appops_service::mode(host, 99, 10100, Some("p")).unwrap(), 0);
        assert_eq!(recv(&rx).0, thread::INSTRUMENT_WITHOUT_RESTART);
        let sessions = &host.instrumentation;
        for (name, uid, expected) in [
            ("p", 10100, true),
            ("q", 10100, false),
            ("p", 110100, false),
        ] {
            assert_eq!(sessions.matching(name, "p:other", uid).is_some(), expected);
        }
        let first = object_bundle(|p| p.write_binder(Some(w.binder())))
            .with_ints(&[("keep", 1), ("overwrite", 2)])
            .unwrap();
        results(host, who, None, Some(first)).unwrap();
        let last = RawBundle::empty()
            .with_null("overwrite")
            .unwrap()
            .with_ints(&[("last", 3)])
            .unwrap();
        results(host, who, Some(-1), Some(last)).unwrap();
        let (method, code, bundle) = recv(&rx);
        assert_eq!((method, code), (watcher::INSTRUMENTATION_FINISHED, -1));
        let bundle = bundle.unwrap();
        let mut encoded = Parcel::new();
        bundle.write_to(&mut encoded);
        assert_eq!(encoded.objects().len(), 1);
        assert_eq!(bundle.int("keep", -1).unwrap(), 1);
        assert_eq!(bundle.int("overwrite", -1).unwrap(), -1);
        assert_eq!(bundle.int("last", -1).unwrap(), 3);
        assert_eq!(recv(&rx).0, ui::SHUTDOWN);
        assert_eq!(appops_service::mode(host, 99, 10100, Some("p")).unwrap(), 2);
        results(host, who, Some(1), None).unwrap();
        assert_eq!(host.instrumentation_source(10100), None);
        assert_eq!(probe(host, 10100), -1); // retired instrumentation cannot grant injection

        assert!(host.apps.lock().unwrap()[&100].instrumentation.is_none());
        assert!(host.instrumentation.matching("p", "p", 10100).is_none());
        assert!(host.apps.lock().unwrap()[&100].thread.is_some());
    });
}

#[test]
fn instrumentation_restart_early_attach_and_failures_retire_once() {
    with(|host, publish, export| {
        publish(&mut |p| manifest(p, None));
        let metrics = display::Metrics::of(&display::Display::phone_for_test());
        host.display_metrics.set(Ok(metrics)).unwrap();
        let framework =
            resources::Framework::load(&aim_paths::original_image(), 160, "en-US").unwrap();
        assert!(host.framework.set(framework).is_ok());
        let backing = host.data().unwrap().join("instrumentation-memory");
        std::fs::write(&backing, []).unwrap();
        let fd = std::fs::File::open(backing).unwrap().into();
        let memory = bind::ApplicationSharedMemory::for_test(fd);
        assert!(host.platform().unwrap().shared_memory.set(memory).is_ok());
        let (w, ui, t, rx) = recorders(host, export);
        for outcome in 0..4 {
            host.package_system()
                .unwrap()
                .internal_set_package_stopped_state(Some("p".into()), true, 0, 1000, 0)
                .unwrap();
            assert!(host.packages().unwrap().0.packages["p"].users[&0].stopped);
            insert(host, 9001, None, "q");
            insert(host, 9000, None, "p"); // restart cancels a pending predecessor, never joining it.
            let (socket, relay) = UnixStream::pair().unwrap();
            relay
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            *host.zygote.lock().unwrap() = Some(zygote::Session::new(socket));
            std::thread::scope(|scope| {
                let task = scope.spawn(|| start(host, 0, request(5, w.binder(), ui.binder())));
                let mut relay = BufReader::new(relay);
                let mut line = String::new();
                relay.read_line(&mut line).unwrap();
                let args: Vec<_> = (&mut relay)
                    .lines()
                    .take(line.trim().parse().unwrap())
                    .map(Result::unwrap)
                    .collect();
                let seq: i64 = args
                    .last()
                    .unwrap()
                    .strip_prefix("seq=")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_ne!(seq, 9000);
                assert!(!host.packages().unwrap().0.packages["p"].users[&0].stopped);
                assert!(!host.apps.lock().unwrap()[&9001].removed);
                assert!(host.apps.lock().unwrap()[&9000].removed);
                let flags: i32 = args
                    .iter()
                    .find_map(|a| a.strip_prefix("--runtime-flags="))
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(flags & (3 << 12), 0);
                assert_ne!(flags & (1 << 18), 0);
                assert!(args.contains(&"--instruction-set=arm64".into()));
                let pid = std::process::id() as i32;
                if outcome == 0 {
                    attach(host, t.binder(), seq, 10100, pid);
                    assert_eq!(recv(&rx).0, thread::BIND_APPLICATION);
                    assert_eq!(recv(&rx).0, thread::SET_PROCESS_STATE);
                }
                let result = if outcome == 1 { -1i32 } else { pid };
                relay
                    .get_mut()
                    .write_all(&[result.to_be_bytes().as_slice(), &[0]].concat())
                    .unwrap();
                assert_eq!(task.join().unwrap().is_err(), outcome == 1);
                match outcome {
                    0 => results(host, t.binder(), Some(-1), None).unwrap(),
                    1 => (),
                    2 => host.start_timed_out(seq),
                    _ => host.process_died(seq, pid),
                }
                // Agent notifications may interleave with the ordered reporter.
                let mut reports = std::iter::from_fn(|| Some(recv(&rx)))
                    .filter(|e| e.0 == watcher::INSTRUMENTATION_FINISHED || e.0 == ui::SHUTDOWN);
                let event = reports.next().unwrap();
                assert_eq!(
                    (event.0, event.1),
                    (
                        watcher::INSTRUMENTATION_FINISHED,
                        if outcome == 0 { -1 } else { 0 }
                    )
                );
                assert_eq!(reports.next().unwrap().0, ui::SHUTDOWN);
                host.process_died(seq, pid);
                assert!(host.instrumentation.matching("p", "p", 10100).is_none());
            });
        }
    });
}

fn adopt(
    host: &Arc<AppHost>,
    caller: u32,
    uid: i32,
    names: Option<&[&str]>,
) -> Result<(), Exception> {
    let mut p = Parcel::new();
    am::StartDelegateShellPermissionIdentity {
        uid,
        permissions: names.map(|n| n.iter().map(|n| Some((*n).into())).collect()),
    }
    .write(&mut p);
    call(
        host,
        caller,
        am::START_DELEGATE_SHELL_PERMISSION_IDENTITY,
        &p,
    )
    .reader()
    .read_exception()
    .unwrap()
}
fn adopted(host: &Arc<AppHost>, caller: u32) -> Result<Option<Vec<Option<String>>>, Exception> {
    let mut p = Parcel::new();
    am::GetDelegatedShellPermissions {}.write(&mut p);
    am::read_get_delegated_shell_permissions_reply(
        &mut call(host, caller, am::GET_DELEGATED_SHELL_PERMISSIONS, &p).reader(),
    )
    .unwrap()
}
fn drop_permissions(host: &Arc<AppHost>, caller: u32) -> Result<(), Exception> {
    let mut p = Parcel::new();
    am::StopDelegateShellPermissionIdentity {}.write(&mut p);
    call(
        host,
        caller,
        am::STOP_DELEGATE_SHELL_PERMISSION_IDENTITY,
        &p,
    )
    .reader()
    .read_exception()
    .unwrap()
}

/// ContextImpl -> PermissionManager.sPermissionCache -> activity#258,
/// android-16.0.0_r1. Model the client cache; exercise the real Binder handlers
/// and the synchronous nonce-invalidation callback, including session cleanup.
#[test]
fn system_context_permission_cache_follows_shell_delegation() {
    use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};
    const MANAGE: &str = "android.permission.MANAGE_SAFETY_CENTER";
    const UID: i32 = 10213;
    struct Nonce(Arc<AtomicU64>);
    impl Service for Nonce {
        fn descriptor(&self) -> &str {
            bridge::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!((call.sender_euid, call.flags & 1), (1000, 0));
            assert_eq!(call.code, bridge::INVALIDATE_PACKAGE_INFO_CACHE);
            bridge::InvalidatePackageInfoCache::read(&mut call.data)?;
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut p = Parcel::new();
            bridge::write_invalidate_package_info_cache_reply(&mut p);
            Ok(p)
        }
    }
    with(|host, publish, export| {
        publish(&mut |p| manifest(p, None));
        let system = host.package_system().unwrap();
        let base = system.capture_package_queries().unwrap();
        let mut owner = base.scan().owner().clone();
        owner
            .settings
            .packages
            .iter_mut()
            .find(|p| p.name == "p")
            .unwrap()
            .app_id = UID;
        let identity = owner.identities.ids.remove(10100).unwrap();
        owner
            .identities
            .ids
            .register_existing(UID, identity)
            .unwrap();
        let mut code = (**base.state().packages["p"].pkg.as_ref().unwrap()).clone();
        code.uid = UID;
        owner.load_test_code(code);
        let groups = owner
            .identities
            .shared_users
            .keys()
            .map(|name| (name.clone(), Default::default()))
            .collect();
        owner
            .capture_legacy_permissions(
                &[0],
                ["p", "q"]
                    .map(|name| ((name.into(), false), Default::default()))
                    .into(),
                groups,
            )
            .unwrap();
        owner
            .capture_install_permissions_fixed(
                ["p", "q"].map(|name| ((name.into(), false), false)).into(),
            )
            .unwrap();
        owner
            .assign_seinfo_at_boot(
                &crate::package::owner::seinfo::Policy::parse(&[]).unwrap(),
                &mut |_: &pkg::AndroidPackage| Ok(36),
            )
            .unwrap();
        owner
            .complete_library_dependencies(&|_, _| {
                Ok(crate::package::libraries::Policy {
                    enforce_native_dependencies: false,
                    sdk_library_independence: false,
                })
            })
            .unwrap();
        owner
            .rebuild_shared_processes_from_native_members()
            .unwrap();
        owner
            .capture_replica_runtime(
                ["p", "q"]
                    .map(|name| {
                        (
                            (name.into(), false),
                            crate::package::scan::ReplicaRuntime {
                                usage: [0; 8],
                                seinfo: owner.seinfo(name).unwrap().map(str::to_owned),
                                override_seinfo: None,
                                library_files: vec![],
                                libraries: vec![],
                            },
                        )
                    })
                    .into(),
            )
            .unwrap();
        let mut context = (**base.context()).clone();
        context.scan_version = base.scan().version() + 1;
        context
            .packages
            .get_mut(&("p".into(), false))
            .unwrap()
            .app_id = UID;
        context.system.system_permissions = Some([(2000, [MANAGE.into()].into())].into());
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(base.scan()),
                owner,
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        let nonce = Arc::new(AtomicU64::new(4));
        let java = export(Arc::new(Nonce(nonce.clone())));
        host.attach_package_bootstrap_java(Some(java.binder()))
            .unwrap();
        let cached = RefCell::new(HashMap::new());
        let check = |permission: &str, uid| {
            let key = (permission.to_owned(), uid);
            let generation = nonce.load(Ordering::SeqCst);
            if let Some(&(n, result)) = cached.borrow().get(&key) {
                if n == generation {
                    return result;
                }
            }
            let mut p = Parcel::new();
            am::CheckPermissionForDevice {
                permission: Some(permission.into()),
                pid: 42,
                uid,
                device_id: 0,
            }
            .write(&mut p);
            let reply = call(host, 1000, am::CHECK_PERMISSION_FOR_DEVICE, &p);
            let result = am::read_check_permission_for_device_reply(&mut reply.reader())
                .unwrap()
                .unwrap();
            cached.borrow_mut().insert(key, (generation, result));
            result
        };
        let (w, ui, t, rx) = recorders(host, export);
        let who = t.binder();
        insert(host, 100, Some(t), "p");
        {
            let mut apps = host.apps.lock().unwrap();
            let app = apps.get_mut(&100).unwrap();
            app.uid = UID as u32;
            app.package_uid = UID as u32;
        }
        assert!(start(host, 2000, request(8, w.binder(), ui.binder())).unwrap());
        recv(&rx);
        assert_eq!(check(MANAGE, UID), -1); // Seed the denial before adoption.
        for names in [None, Some(&[MANAGE][..])] {
            adopt(host, 2000, UID, names).unwrap();
            assert_eq!(check(MANAGE, UID), 0);
            assert_eq!(check(MANAGE, UID + 1), -1);
            assert_eq!(check("not.a.shell.permission", UID), -1);
            adopt(host, 2000, UID, Some(&[])).unwrap();
            assert_eq!(check(MANAGE, UID), -1); // Replacing the list invalidates too.
            adopt(host, 2000, UID, names).unwrap();
            assert_eq!(check(MANAGE, UID), 0);
            drop_permissions(host, 2000).unwrap();
            assert_eq!(check(MANAGE, UID), -1);
        }
        for death in [false, true] {
            adopt(host, 2000, UID, None).unwrap();
            assert_eq!(check(MANAGE, UID), 0);
            if death {
                host.apps.lock().unwrap().get_mut(&100).unwrap().pid = 42;
                host.process_died(100, 42);
            } else {
                results(host, who, Some(-1), None).unwrap();
            }
            assert_eq!(check(MANAGE, UID), -1);
            recv(&rx);
            recv(&rx);
            if !death {
                assert!(start(host, 2000, request(8, w.binder(), ui.binder())).unwrap());
                recv(&rx);
            }
        }
    });
}

/// UiAutomationTest.testAdoptAllShellPermissions / testAdoptSomeShellPermissions,
/// plus AccessCheckDelegateHelper admission and AppOps delegation (Android 16).
#[test]
fn instrumentation_shell_delegation_scope_and_cleanup() {
    const BATTERY: &str = "android.permission.BATTERY_STATS";
    const USAGE: &str = "android.permission.PACKAGE_USAGE_STATS";
    with(|host, publish, export| {
        publish(&mut |p| manifest(p, None));
        let system = host.package_system().unwrap();
        let base = system.capture_package_queries().unwrap();
        let mut context = (**base.context()).clone();
        context.scan_version = base.scan().version() + 1;
        context.system.system_permissions =
            Some([(2000, [BATTERY.into(), USAGE.into()].into())].into());
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(base.scan()),
                base.scan().owner().clone(),
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        let (w, ui, t, rx) = recorders(host, export);
        let who = t.binder();
        let perms = host.permission_manager.get().unwrap().clone();
        let ops = appops_service::AppOps::new(host.clone(), perms.clone());
        for (code, uid, mode) in [
            (43, 10101, 0),
            (43, 10100, 1),
            (43, 2000, 0),
            (26, 10100, 1),
            (26, 2000, 2),
            (2, 10100, 1),
            (2, 2000, 0),
        ] {
            ops.set_mode(0, code, uid, None, mode).unwrap();
        }
        let checks = |battery, usage, mode| {
            assert_eq!(
                host.check_permission(Some(BATTERY), 10100).unwrap(),
                battery
            );
            assert_eq!(host.check_permission(Some(USAGE), 10100).unwrap(), usage);
            assert_eq!(
                appops_service::check_from(host, 10100, 43, 10100, Some("p")).unwrap(),
                mode
            );
            assert_eq!(
                appops_service::note_from(host, 10100, 43, 10100, Some("p"), None).unwrap(),
                mode
            );
            assert_eq!(host.check_permission(Some(BATTERY), 10101).unwrap(), -1);
            assert_eq!(
                host.check_permission(Some("not.a.shell.permission"), 10100)
                    .unwrap(),
                -1
            );
        };
        assert_eq!(adopted(host, 2000).unwrap(), Some(vec![]));
        adopt(host, 2000, 10100, None).unwrap(); // No active session: no delegation.
        checks(-1, -1, 1);
        insert(host, 100, Some(t), "p");
        assert!(start(host, 2000, request(8, w.binder(), ui.binder())).unwrap());
        recv(&rx);
        for uid in [1000, 10100, 10101] {
            assert_eq!(
                adopt(host, uid, 10100, None).unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert!(drop_permissions(host, uid).is_err());
            assert!(adopted(host, uid).is_err());
        }
        adopt(host, 2000, 10100, Some(&[BATTERY])).unwrap();
        checks(0, -1, 1);
        assert_eq!(adopted(host, 0).unwrap(), Some(vec![Some(BATTERY.into())]));
        adopt(host, 2000, 10100, None).unwrap();
        checks(0, 0, 0);
        assert_eq!(adopted(host, 2000).unwrap(), None);
        assert_eq!(appops_service::mode(host, 26, 10100, Some("p")).unwrap(), 2);
        assert!(adopt(host, 2000, 10101, None).is_err()); // One delegate globally.
        adopt(host, 0, 10100, Some(&[USAGE])).unwrap();
        checks(-1, 0, 0);
        assert_eq!(appops_service::mode(host, 26, 10100, Some("p")).unwrap(), 1);
        let client = Some(w.binder());
        assert_eq!(
            appops_service::start(
                host,
                10100,
                client,
                43,
                10100,
                Some("p"),
                None,
                appops_service::Attribution {
                    start_if_mode_default: false,
                    flags: 0,
                    chain_id: -1
                }
            )
            .unwrap(),
            0
        );
        appops_service::finish(host, 10100, client, 43, 10100, Some("p"), None).unwrap();
        let q = permission_checker::AttributionSourceState {
            pid: -1,
            uid: 10101,
            device_id: 0,
            package: Some("q".into()),
            tag: None,
            token: None,
            renounced: None,
            next: None,
        };
        let p = permission_checker::AttributionSourceState {
            uid: 10100,
            package: Some("p".into()),
            ..q.clone()
        };
        assert_eq!(
            appops_service::check_raw_from(host, 10100, 43, &p, "p").unwrap(),
            0
        );
        let proxy = permission_checker::AttributionSourceState {
            next: Some(vec![Some(p.clone())]),
            ..q.clone()
        };
        assert_eq!(
            appops_service::note_proxy(host, 1000, 43, &proxy, false, None).unwrap(),
            0
        );
        let proxy = permission_checker::AttributionSourceState {
            next: Some(vec![Some(q)]),
            ..p
        };
        assert_eq!(
            appops_service::start_proxy(
                host,
                10100,
                client,
                43,
                &proxy,
                false,
                appops_service::ProxyAttribution {
                    start_if_mode_default: false,
                    proxy_flags: 0,
                    proxied_flags: 0,
                    chain_id: -1
                },
                None
            )
            .unwrap(),
            0
        );
        appops_service::finish_proxy(host, 10100, client, 43, &proxy, false).unwrap();
        adopt(host, 2000, 10100, Some(&[])).unwrap();
        checks(-1, -1, 1);
        assert_eq!(appops_service::mode(host, 2, 10100, Some("p")).unwrap(), 0); // No permission mapping.
        drop_permissions(host, 2000).unwrap();
        drop_permissions(host, 0).unwrap();
        checks(-1, -1, 1);
        assert_eq!(appops_service::mode(host, 2, 10100, Some("p")).unwrap(), 1);
        adopt(host, 2000, 10100, None).unwrap();
        results(host, who, Some(-1), None).unwrap();
        checks(-1, -1, 1);
        recv(&rx);
        recv(&rx);
        assert_eq!(adopted(host, 2000).unwrap(), Some(vec![]));
        assert!(start(host, 0, request(8, w.binder(), ui.binder())).unwrap());
        recv(&rx);
        adopt(host, 2000, 10100, None).unwrap();
        host.apps.lock().unwrap().get_mut(&100).unwrap().pid = 42;
        host.process_died(100, 42);
        checks(-1, -1, 1);
        recv(&rx);
        recv(&rx);
        // A later session with no UiAutomationConnection cannot adopt shell state.
        let (w, ui, t, rx) = recorders(host, export);
        let who = t.binder();
        insert(host, 101, Some(t), "p");
        let mut a = request(8, w.binder(), ui.binder());
        a.connection = None;
        assert!(start(host, 0, a).unwrap());
        recv(&rx);
        assert!(adopt(host, 2000, 10100, None).is_err());
        results(host, who, Some(0), None).unwrap();
        recv(&rx);
    });
}
