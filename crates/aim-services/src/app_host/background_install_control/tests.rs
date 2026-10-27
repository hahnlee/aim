//! Assertions from services/tests/servicestests' BackgroundInstallControlServiceTest
//! and mockingservicestests' BackgroundInstallControlCallbackHelperTest,
//! android-16.0.0_r1: windows, packageAdded/Removed, state I/O and sendResult.
use super::super::usage_events::Extra;
use super::*;
use aim_binder_host::parcel::EX_SECURITY;
use std::time::Duration;

fn event(kind: i32, instance: i32, time: i64) -> Event {
    Event {
        package: "installer".into(),
        class: None,
        instance_id: instance,
        task_root_package: None,
        task_root_class: None,
        event_type: kind,
        time_stamp: time,
        flags: 0,
        extra: Extra::None,
    }
}

#[test]
fn foreground_windows_include_boundaries_track_all_activities_and_keep_ten() {
    let mut state = State::default();
    assert!(!state.foreground("installer", 0, 100));
    state.usage(0, &event(ACTIVITY_PAUSED, 1, 90));
    assert!(state.frames[&(0, "installer".into())].is_empty());
    state.usage(0, &event(ACTIVITY_RESUMED, 1, 100));
    state.usage(0, &event(ACTIVITY_RESUMED, 2, 110));
    state.usage(0, &event(ACTIVITY_PAUSED, 1, 120));
    state.usage(0, &event(ACTIVITY_STOPPED, 99, 130));
    assert!(!state.foreground("installer", 0, 99));
    assert!(state.foreground("installer", 0, 100));
    assert!(state.foreground("installer", 0, 1000));
    assert!(!state.foreground("installer", 10, 110));
    state.usage(0, &event(ACTIVITY_STOPPED, 2, 140));
    assert!(state.foreground("installer", 0, 140));
    assert!(!state.foreground("installer", 0, 141));
    for n in 0..10 {
        state.usage(0, &event(ACTIVITY_RESUMED, 1, 200 + n * 10));
        state.usage(0, &event(ACTIVITY_PAUSED, 1, 201 + n * 10));
    }
    assert_eq!(state.frames[&(0, "installer".into())].len(), 10);
    assert!(!state.foreground("installer", 0, 100));
    assert!(state.foreground("installer", 0, 290));
}

#[test]
fn persistence_uses_original_proto_user_offset_hash_order_and_atomic_recovery() {
    let dir = super::super::test_temp_dir("bic-proto");
    let file = dir.join(STATES);
    let mut state = State::default();
    insert(state.load(&file).unwrap(), 10, "z".into());
    insert(state.load(&file).unwrap(), 0, "p".into());
    insert(state.load(&file).unwrap(), 10, "a".into());
    insert(state.load(&file).unwrap(), 10, "a".into());
    state.persist(&file).unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"\x0a\x05\x0a\x01p\x10\x01\x0a\x05\x0a\x01a\x10\x0b\x0a\x05\x0a\x01z\x10\x0b"
    );
    let mut restarted = State::default();
    assert_eq!(
        restarted.load(&file).unwrap(),
        state.packages.as_ref().unwrap()
    );
    std::fs::copy(&file, file.with_extension("bak")).unwrap();
    std::fs::write(&file, b"partial").unwrap();
    std::fs::write(file.with_extension("new"), b"unfinished").unwrap();
    assert_eq!(
        State::default().load(&file).unwrap(),
        state.packages.as_ref().unwrap()
    );
    assert!(!file.with_extension("new").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

fn drain(owner: &BackgroundInstallControl) {
    let (tx, rx) = mpsc::channel();
    owner.post(Message::Barrier(tx)).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
}

struct Listener(mpsc::Sender<(String, i32, i32)>);
impl Service for Listener {
    fn descriptor(&self) -> &str {
        callback::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, callback::SEND_RESULT);
        call.data.enforce_interface(callback::DESCRIPTOR).unwrap();
        assert_eq!(call.data.read_i32().unwrap(), 1);
        let b = crate::bundle::read(&mut call.data).unwrap().unwrap();
        let crate::bundle::Value::String(Some(name)) = &b["packageName"] else {
            panic!()
        };
        let crate::bundle::Value::Int(user) = b["userId"] else {
            panic!()
        };
        let crate::bundle::Value::Int(kind) = b["installEventType"] else {
            panic!()
        };
        self.0.send((name.clone(), user, kind)).unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn observer_removes_only_uid_user_and_callbacks_keep_message_order_across_restart() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        permissions(host);
        let _bootstrap = super::super::package_bootstrap::PackageBootstrap::new(host.clone());
        let events = super::super::package_observers::Events(
            host.package_observers.lock().unwrap().upgrade().unwrap(),
        );
        let owner = host.background_install_control();
        let file = host.data().unwrap().join(STATES);
        {
            let mut state = owner.state.lock().unwrap();
            for user in [0, 10] {
                insert(state.load(&file).unwrap(), user, "p".into());
            }
            state.persist(&file).unwrap();
        }
        let (tx, rx) = mpsc::channel();
        let listener = export(Arc::new(Listener(tx)));
        let (tx2, rx2) = mpsc::channel();
        let listener2 = export(Arc::new(Listener(tx2)));
        owner.register(host, listener.handle);
        owner.register(host, listener.handle);
        owner.register(host, listener2.handle);
        use aim_service_aidl::dev_aim_server_ipackageobservereventsbridge as transport;
        for uid in [
            10 * PER_USER_RANGE as i32 + 10100,
            10 * PER_USER_RANGE as i32 + 10100,
            10100,
        ] {
            let mut p = Parcel::new();
            transport::PackageRemoved {
                package_name: Some("p".into()),
                uid,
            }
            .write(&mut p);
            events
                .transact(&mut Call {
                    code: transport::PACKAGE_REMOVED,
                    flags: 0,
                    sender_euid: SYSTEM_UID,
                    sender_pid: 77,
                    data: p.reader(),
                })
                .unwrap()
                .reader()
                .read_exception()
                .unwrap()
                .unwrap();
        }
        drain(&owner);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ("p".into(), 10, 2)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ("p".into(), 0, 2)
        );
        assert!(rx.try_recv().is_err());
        for user in [10, 0] {
            assert_eq!(
                rx2.recv_timeout(Duration::from_secs(5)).unwrap(),
                ("p".into(), user, 2)
            );
        }
        assert!(rx2.try_recv().is_err());
        assert!(State::default().load(&file).unwrap().is_empty());
        let restarted = BackgroundInstallControl::new(host);
        assert!(
            restarted
                .state
                .lock()
                .unwrap()
                .load(&file)
                .unwrap()
                .is_empty()
        );
        owner.unregister(host, listener.handle);
        owner.unregister(host, listener2.handle);
    });
}

#[test]
fn aidl_query_is_system_identity_filtered_and_permission_denial_is_flag_gated() {
    crate::system::with_discovery_session(|host, _, _| {
        permissions(host);
        let owner = host.background_install_control();
        let file = host.data().unwrap().join(STATES);
        insert(
            owner.state.lock().unwrap().load(&file).unwrap(),
            0,
            "p".into(),
        );
        let infos = owner.packages(host, SYSTEM_UID as i32, 0, 0).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].package_name.as_deref(), Some("p"));
        let mut request = Parcel::new();
        bic::GetBackgroundInstalledPackages {
            flags: 0,
            user_id: 0,
        }
        .write(&mut request);
        let result = owner
            .transact(&mut Call {
                code: bic::GET_BACKGROUND_INSTALLED_PACKAGES,
                flags: 0,
                sender_euid: 10100,
                sender_pid: 77,
                data: request.reader(),
            })
            .unwrap();
        let mut r = result.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1); // typed slice present
        assert_eq!(r.read_i32().unwrap(), 1); // one item
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.content.pm.PackageInfo")
        );
        assert!(
            owner
                .packages(host, SYSTEM_UID as i32, 0, 10)
                .unwrap()
                .is_empty()
        );
        let system = host.package_system().unwrap();
        let capture = host.package_capture().unwrap();
        let mut context = (**capture.context()).clone();
        context
            .system
            .flags
            .push(("android.app.bic_client".into(), true));
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
        let result = owner
            .transact(&mut Call {
                code: bic::GET_BACKGROUND_INSTALLED_PACKAGES,
                flags: 0,
                sender_euid: 10100,
                sender_pid: 77,
                data: request.reader(),
            })
            .unwrap();
        let e = result.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(e.code, EX_SECURITY);
        assert_eq!(
            e.message,
            format!("User is not permitted to call service: {GET_PACKAGES}")
        );
    });
}

fn permissions(host: &Arc<AppHost>) {
    use crate::package::permissions::{AccessSystem, Permission};
    let path = host
        .data()
        .unwrap()
        .join("misc_de/0/apexdata/com.android.permission/access.abx");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let xml = aim_android_xml::read(br#"<access><app-id-permissions><app-id id="10101"><permission name="android.permission.INSTALL_PACKAGES" flags="4"/></app-id></app-id-permissions></access>"#).unwrap();
    std::fs::write(path, aim_android_xml::abx::write(&xml).unwrap()).unwrap();
    let manager = Arc::new(
        super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            AccessSystem {
                permissions: vec![Permission {
                    name: INSTALL_PACKAGES.into(),
                    package: "q".into(),
                    protection_level: 2,
                    dynamic: None,
                }],
                ..Default::default()
            },
        ),
    );
    assert!(host.permission_manager.set(manager).is_ok());
}

fn source(host: &AppHost, installer: &str, initiating: Option<&str>) {
    let system = host.package_system().unwrap();
    let before = host.package_capture().unwrap();
    let mut owner = before.scan().owner().clone();
    let p = owner
        .settings
        .packages
        .iter_mut()
        .find(|p| p.name == "p")
        .unwrap();
    p.install_source.installer = Some(installer.into());
    p.install_source.initiating_package = initiating.map(str::to_owned);
    let mut context = (**before.context()).clone();
    context.scan_version = before.scan().version() + 1;
    system
        .publish_package_scan_with_queries(
            &system.package_bootstrap().unwrap(),
            Some(before.scan()),
            owner,
            before.scan().usage().clone(),
            context,
        )
        .unwrap();
}

#[test]
fn handler_classifies_privileged_installer_windows_adb_and_missing_history_then_restarts() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        permissions(host);
        source(host, "q", Some("q"));
        let owner = host.background_install_control();
        let (tx, rx) = mpsc::channel();
        let listener = export(Arc::new(Listener(tx)));
        let mut p = Parcel::new();
        bic::RegisterBackgroundInstallCallback {
            callback: Some(listener.binder()),
        }
        .write(&mut p);
        // There is deliberately no permission check on registration in the pinned AIDL server.
        let r = owner
            .transact(&mut Call {
                code: bic::REGISTER_BACKGROUND_INSTALL_CALLBACK,
                flags: 0,
                sender_euid: 10100,
                sender_pid: 77,
                data: p.reader(),
            })
            .unwrap();
        r.reader().read_exception().unwrap().unwrap();
        let file = host.data().unwrap().join(STATES);
        assert!(owner.is_installer(host, "q", 0).unwrap());
        assert!(!owner.is_installer(host, "p", 0).unwrap());
        let boot_wall = super::super::usage_stats::now_millis()
            - super::super::clock_ms(libc::CLOCK_UPTIME_RAW);
        let mut resumed = event(ACTIVITY_RESUMED, 1, boot_wall - 1000);
        resumed.package = "q".into();
        owner.usage(0, resumed.clone()).unwrap();
        owner.package_added(Some("p"), 10100).unwrap();
        drain(&owner);
        assert!(
            !file.exists(),
            "open foreground window excludes the install"
        );
        let mut paused = resumed;
        paused.event_type = ACTIVITY_PAUSED;
        paused.time_stamp = boot_wall - 500;
        owner.usage(0, paused).unwrap();
        owner.package_added(Some("p"), 10100).unwrap();
        drain(&owner);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ("p".into(), 0, 1)
        );
        let restarted = BackgroundInstallControl::new(host);
        assert_eq!(
            restarted.packages(host, 1000, 0, 0).unwrap()[0]
                .package_name
                .as_deref(),
            Some("p")
        );
        owner.package_removed(Some("p"), 10100).unwrap();
        drain(&owner);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ("p".into(), 0, 2)
        );
        source(host, "q", None);
        owner.package_added(Some("p"), 10100).unwrap();
        drain(&owner);
        assert!(
            owner.packages(host, 1000, 0, 0).unwrap().is_empty(),
            "absent initiating package is ADB"
        );
        source(host, "p", Some("p"));
        owner.package_added(Some("p"), 10100).unwrap();
        drain(&owner);
        assert!(
            owner.packages(host, 1000, 0, 0).unwrap().is_empty(),
            "nonprivileged installer excluded"
        );
        owner.unregister(host, listener.handle);
        assert!(rx.try_recv().is_err());
    });
}
