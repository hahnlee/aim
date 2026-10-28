//! NativePackageEffectsBridge / BroadcastHelper, android-16.0.0_r1.
use super::super::package_writes::{PackageChanged, PackageMonitor};
use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_SECURITY, Reader};
use std::sync::{Mutex, mpsc};

fn install(replacing: bool) -> bridge::PostInstallPackage {
    bridge::PostInstallPackage {
        name: Some("p".into()),
        app_id: 10100,
        replacing,
        dont_kill: true,
        installer: Some("q".into()),
        old_installer: Some("old.installer".into()),
        data_loader_type: 0,
        system: false,
        virtual_preload: false,
        static_library: false,
        first_users: Some(if replacing { vec![] } else { vec![0] }),
        first_instant_users: Some(vec![]),
        update_users: Some(if replacing { vec![0] } else { vec![] }),
        update_instant_users: Some(vec![]),
        removed_users: Some(vec![0]),
        removed_instant_users: Some(vec![]),
        prior_visibility: Some(vec![0, 1, 54321]),
    }
}

fn install_owners(host: &Arc<AppHost>) {
    let system = host.package_system().unwrap();
    let capture = host.package_capture().unwrap();
    let mut context = (**capture.context()).clone();
    context.system.roles = Some(crate::package::roles::Owner::known_packages_fixture(
        [(2, vec![Some("q".into())]), (4, vec![Some("q".into())])].into(),
    ));
    context.system.permission_controller_package = Some(Some("q".into()));
    context.scan_version = capture.scan().version() + 1;
    system
        .publish_package_scan_with_queries(
            &system.package_bootstrap().unwrap(),
            Some(capture.scan()),
            capture.scan().owner().clone(),
            capture.scan().usage().clone(),
            context,
        )
        .unwrap();
}

#[test]
fn visibility_capture_binds_code_uid_and_metadata_of_one_session() {
    crate::system::with_discovery_session(|host, publish, _| {
        publish(&mut |_| {});
        let capture = host.package_capture().unwrap();
        let ps = &capture.state().packages["p"];
        let request = || bridge::CapturePostInstallVisibility {
            name: Some("p".into()),
            app_id: ps.app_id,
            code_path: Some(ps.path.clone()),
            version: ps.version_code,
            users: Some(vec![0, 10]),
        };
        let effects = PackageEffects::new(host.clone());
        let values = effects.capture_visibility(request()).unwrap();
        assert_eq!(values[0], 0x56495332);
        assert_eq!(
            ((values[1] as u32 as u64) << 32) | values[2] as u32 as u64,
            capture.scan().metadata_revision()
        );
        let lists = decode_visibility(&values[3..]).unwrap();
        let resolution = capture.resolution().unwrap();
        for user in [0, 10] {
            assert_eq!(
                lists[&user],
                resolution
                    .apps_filter
                    .visibility_allow_list(capture.state(), ps, user)
            );
        }
        let mut changed = request();
        changed.app_id += 1;
        assert_eq!(
            effects.capture_visibility(changed).unwrap_err().code,
            EX_ILLEGAL_STATE
        );
        let mut changed = request();
        changed.version += 1;
        assert!(
            effects
                .capture_visibility(changed)
                .unwrap_err()
                .message
                .contains("code/UID changed")
        );
        let mut changed = request();
        changed.code_path = Some("/data/app/other".into());
        assert!(
            effects
                .capture_visibility(changed)
                .unwrap_err()
                .message
                .contains("code/UID changed")
        );
        let mut changed = request();
        changed.name = Some("absent".into());
        assert!(
            effects
                .capture_visibility(changed)
                .unwrap_err()
                .message
                .contains("package absent")
        );
        assert_eq!(
            decode_visibility(&[0, -1, 10, 0]).unwrap(),
            [(0, None), (10, Some(vec![]))].into()
        );
        for invalid in [&[0][..], &[0, 2, 1], &[0, -2], &[0, 0, 0, -1], &[-1, 0]] {
            assert_eq!(
                decode_visibility(invalid).unwrap_err().code,
                EX_ILLEGAL_ARGUMENT
            );
        }
    });
}

#[test]
fn post_install_uses_prior_visibility_only_for_removal_and_replacement() {
    crate::system::with_discovery_session(|host, publish, _| {
        publish(&mut |pkg| {
            if pkg.package_name == "q" {
                pkg.booleans |= crate::package::pkg::booleans::SYSTEM;
            }
        });
        install_owners(host);
        let effects = PackageEffects::new(host.clone());
        let (events, discoveries) = effects.post_install_events(install(true)).unwrap();
        assert_eq!(discoveries, [("p".into(), 0)]);
        let generic: Vec<_> = events
            .iter()
            .filter(|event| event.intent.intent.package.is_none())
            .collect();
        assert_eq!(
            generic
                .iter()
                .map(|event| event.intent.intent.action.as_deref().unwrap())
                .collect::<Vec<_>>(),
            [REMOVED, ADDED, REPLACED]
        );
        let (state, resolution) = host.packages().unwrap();
        assert_eq!(generic[0].allow.as_deref(), Some(&[54321][..]));
        assert_eq!(generic[2].allow, generic[0].allow);
        assert_eq!(
            generic[1].allow,
            resolution
                .apps_filter
                .visibility_allow_list(&state, &state.packages["p"], 0)
        );
        for event in &generic {
            let extras = event.intent.test_extras();
            assert_eq!(extras[UID], Value::Int(10100));
            assert_eq!(extras[USER], Value::Int(0));
            assert_eq!(extras[REPLACING], Value::Bool(true));
            assert_eq!(event.intent.intent.flags, 0x04000000);
        }
        assert_eq!(
            generic[0].intent.test_extras()["android.intent.extra.DATA_REMOVED"],
            Value::Bool(false)
        );
        assert_eq!(
            generic[0].intent.test_extras()["android.intent.extra.DONT_KILL_APP"],
            Value::Bool(true)
        );
        let mine = events
            .iter()
            .find(|event| {
                event.intent.intent.action.as_deref()
                    == Some("android.intent.action.MY_PACKAGE_REPLACED")
            })
            .unwrap();
        assert_eq!(mine.intent.intent.package.as_deref(), Some("p"));
        assert!(mine.intent.intent.data.is_none());
        assert_eq!(mine.intent.test_extras().len(), 1);
        assert_eq!(
            mine.temp_allowlist.unwrap(),
            TempAllowlist {
                duration_ms: 20000,
                kind: 0,
                reason: 311
            }
        );
        assert!(!mine.notify);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.intent.intent.package.as_deref() == Some("q")
                    && event.intent.intent.action.as_deref() == Some(ADDED))
                .count(),
            2
        );
        let internal = events
            .iter()
            .find(|event| event.intent.intent.package.as_deref() == Some("android"))
            .unwrap();
        assert_eq!(
            internal.intent.test_extras()["android.intent.extra.VISIBILITY_ALLOW_LIST"],
            Value::IntArray(vec![54321])
        );
        let (fresh, _) = effects.post_install_events(install(false)).unwrap();
        let added = fresh.iter().find(|event| event.notify).unwrap();
        assert_eq!(added.intent.intent.action.as_deref(), Some(ADDED));
        assert!(!added.intent.test_extras().contains_key(REPLACING));
        assert!(
            fresh
                .iter()
                .any(|event| event.intent.intent.package.as_deref() == Some("q")
                    && event.intent.intent.flags == 0x05000000)
        );
        let mut wrong = install(true);
        wrong.app_id += 1;
        assert!(
            effects
                .post_install_events(wrong)
                .err()
                .unwrap()
                .message
                .contains("UID changed")
        );
    });
}

struct Seen(Mutex<Vec<(String, i32)>>);
impl PackageMonitor for Seen {
    fn package_changed(&self, change: &PackageChanged<'_>) -> Result<(), Exception> {
        assert!(!change.removed);
        self.0
            .lock()
            .unwrap()
            .push((change.package.into(), change.user));
        Ok(())
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
        let values = crate::bundle::read(&mut call.data)?.unwrap();
        let Value::Lazy { start, .. } =
            values["android.content.pm.extra.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT"]
        else {
            panic!("Intent");
        };
        call.data.set_position(start);
        assert_eq!(
            call.data.read_string16()?.as_deref(),
            Some("android.content.Intent")
        );
        self.0
            .send(aim_service_aidl::ReadParcelable::read_from(&mut call.data)?)
            .unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn install_and_update_notify_each_native_and_callback_monitor_once() {
    crate::system::with_discovery_session(|host, _, _| {
        install_owners(host);
        let native = Arc::new(Seen(Mutex::default()));
        host.package_monitors.register(
            0,
            Arc::downgrade(&(native.clone() as Arc<dyn PackageMonitor>)),
        );
        let (send, receive) = mpsc::channel();
        let callback = host.process.add_service(Arc::new(Callback(send)));
        let events = host.package_system().unwrap().package_events().unwrap();
        events.register(Some(callback), 0, 1000).unwrap();
        let effects = PackageEffects::new(host.clone());
        for replacing in [false, true] {
            effects.post_install(install(replacing)).unwrap();
            assert!(events.wait(2000, false).unwrap());
            assert!(events.wait(2000, false).unwrap());
            let intents: Vec<_> = receive.try_iter().collect();
            assert_eq!(
                intents
                    .iter()
                    .map(|intent| intent.intent.action.as_deref().unwrap())
                    .collect::<Vec<_>>(),
                if replacing {
                    vec![REMOVED, ADDED]
                } else {
                    vec![ADDED]
                }
            );
            assert!(intents.iter().all(|intent| intent.intent.flags == 0));
        }
        assert_eq!(
            *native.0.lock().unwrap(),
            [("p".into(), 0), ("p".into(), 0)]
        );
        effects
            .first_launch(bridge::FirstLaunch {
                package_name: Some("p".into()),
                installer_package: Some("q".into()),
                user_id: 0,
            })
            .unwrap();
        assert!(events.wait(2000, false).unwrap());
        assert!(receive.try_recv().is_err());
    });
}

#[test]
fn installer_permission_uses_compat_and_unknown_uid_requires_permission() {
    crate::system::with_discovery_session(|host, _, _| {
        let dir = host.root().unwrap().join("system/etc/compatconfig");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("installer.xml"),
            r#"<config><compat-change id="150857253" enableSinceTargetSdk="30" /></config>"#,
        )
        .unwrap();
        let effects = PackageEffects::new(host.clone());
        assert!(!effects.installer_permission_required(10100).unwrap());
        assert!(effects.installer_permission_required(12345).unwrap());
        let error = effects.require_installer_permission(10100).unwrap_err();
        assert_eq!(error.code, EX_ILLEGAL_STATE);
        assert!(error.message.contains("agent has not attached"));
        let mut data = Parcel::new();
        bridge::RequireInstallerPermission { calling_uid: 10100 }.write(&mut data);
        let reply = effects
            .transact(&mut Call {
                code: bridge::REQUIRE_INSTALLER_PERMISSION,
                flags: 0,
                sender_pid: 42,
                sender_euid: 2000,
                data: data.reader(),
            })
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        let error = effects
            .package_added(bridge::PackageAdded {
                package_name: Some("p".into()),
                user_id: 10,
                archived: false,
                data_loader_type: 0,
                prediction_package: None,
            })
            .unwrap_err();
        assert!(error.message.contains("#2015"));
    });
}

#[test]
fn instant_delivery_translates_uid_and_keeps_the_permission_gate() {
    let mut events = Vec::new();
    cohorts(
        &mut events,
        ADDED,
        Some("p"),
        &[(UID, Value::Int(10100))],
        0,
        None,
        &[0],
        &[10],
        Some(&[(10, Some(vec![10100]))].into()),
        None,
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].user, 10);
    assert_eq!(events[0].intent.test_extras()[UID], Value::Int(1010100));
    assert_eq!(
        events[0].permissions,
        ["android.permission.ACCESS_INSTANT_APPS"]
    );
    assert!(events[0].instant);
    assert_eq!(events[0].allow.as_deref(), Some(&[10100][..]));
    let mut bytes = Parcel::new();
    events[0].intent.write_to(&mut bytes);
    let mut reader = Reader::new(bytes.data(), bytes.objects());
    let intent: RawIntent = aim_service_aidl::ReadParcelable::read_from(&mut reader).unwrap();
    assert_eq!(intent.test_extras()[USER], Value::Int(10));
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn kill_package_scopes_app_id_and_user_and_preserves_the_exit_reason() {
    crate::system::with_discovery_session(|host, _, _| {
        let tracker = super::super::exit_info::Tracker::load(host.data().unwrap(), 16, None);
        assert!(host.exit_infos.set(tracker).is_ok());
        let mut apps = host.apps.lock().unwrap();
        for (seq, package, app_uid, deps) in [
            (1, "p", 10100, vec![]),
            (2, "shared.peer", 10100, vec![]),
            (3, "p", 1010100, vec![]),
            (4, "unrelated", 10101, vec![]),
            (5, "dependent", 10102, vec!["p".to_owned()]),
            (6, "dependent", 1010102, vec!["p".to_owned()]),
        ] {
            apps.insert(
                seq,
                super::super::App {
                    package: package.into(),
                    packages: vec![package.into()],
                    process: package.into(),
                    uid: app_uid,
                    package_uid: app_uid,
                    isolated: false,
                    entry_point: None,
                    crash_handler: None,
                    application_info: None,
                    pid: seq as i32,
                    started: std::time::Instant::now(),
                    start_elapsed: 0,
                    start_uptime: 0,
                    thread: None,
                    activities: vec![],
                    config: None,
                    providers: vec![],
                    removed: false,
                    first_frame: None,
                    last_activity: 0,
                    persistent: false,
                    package_deps: deps,
                    last_mem_info: None,
                },
            );
        }
        drop(apps);
        for app_uid in [10100, 1010100, 10101] {
            host.app_errors
                .lock()
                .unwrap()
                .crash_times
                .insert(("p".into(), app_uid), std::time::Instant::now());
        }
        let effects = PackageEffects::new(host.clone());
        let mut data = Parcel::new();
        bridge::KillPackage {
            package_name: Some("p".into()),
            app_id: 10100,
            user_id: 0,
            reason: Some("update pkg".into()),
            exit_reason: 15,
        }
        .write(&mut data);
        let reply = effects
            .transact(&mut Call {
                code: bridge::KILL_PACKAGE,
                flags: 0,
                sender_pid: 1,
                sender_euid: 1000,
                data: data.reader(),
            })
            .unwrap();
        // The fixture has no agent: transport failure must reach the caller.
        assert!(
            reply
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .message
                .contains("agent has not attached")
        );
        let empty = Parcel::new();
        let exits = |app_uid| {
            host.historical_process_exit_reasons(
                &Call {
                    code: 0,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: app_uid,
                    data: empty.reader(),
                },
                None,
                0,
                0,
                user_id(app_uid as i32),
            )
            .unwrap()
        };
        let shared = exits(10100);
        assert_eq!(
            shared.iter().map(|exit| exit.pid).collect::<BTreeSet<_>>(),
            [1, 2].into()
        );
        for exit in shared.iter().chain(exits(10102).iter()) {
            assert_eq!(exit.reason, 15);
            assert_eq!(exit.sub_reason, super::super::exit_info::SUBREASON_UNKNOWN);
            assert_eq!(
                exit.description.as_deref(),
                Some("stop p due to update pkg")
            );
        }
        for untouched in [1010100, 10101, 1010102] {
            assert!(exits(untouched).is_empty());
        }
        let errors = host.app_errors.lock().unwrap();
        assert!(!errors.crash_times.contains_key(&("p".into(), 10100)));
        assert!(errors.crash_times.contains_key(&("p".into(), 1010100)));
        assert!(errors.crash_times.contains_key(&("p".into(), 10101)));
    });
}
