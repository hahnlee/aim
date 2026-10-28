//! WifiServiceImpl / ActiveModeWarden, android-16.0.0_r1. Host Binder tests;
//! the Java fixture tests initial callbacks, duplicate registration and death.
use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_UNSUPPORTED_OPERATION};
use std::sync::mpsc;

struct Java(mpsc::Sender<(u32, Option<Binder>, i32)>);
impl Service for Java {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        let mut p = Parcel::new();
        match call.code {
            bridge::ADD_WIFI_STATE_CHANGED_LISTENER => {
                let a = bridge::AddWifiStateChangedListener::read(&mut call.data)?;
                self.0.send((call.code, a.listener, 0)).unwrap();
                bridge::write_add_wifi_state_changed_listener_reply(&mut p);
            }
            bridge::REMOVE_WIFI_STATE_CHANGED_LISTENER => {
                let a = bridge::RemoveWifiStateChangedListener::read(&mut call.data)?;
                self.0.send((call.code, a.listener, 0)).unwrap();
                // Prove Java errors survive the native frontend.
                return reply(Err(Exception::new(EX_ILLEGAL_STATE, "original error")));
            }
            bridge::ON_WIFI_STATE_CHANGED => {
                let a = bridge::OnWifiStateChanged::read(&mut call.data)?;
                assert_ne!(call.flags & 1, 0);
                self.0.send((call.code, None, a.state)).unwrap();
            }
            bridge::GET_PASSPOINT_CONFIGURATIONS => {
                bridge::GetPasspointConfigurations::read(&mut call.data)?;
                self.0.send((call.code, None, 0)).unwrap();
                bridge::write_get_passpoint_configurations_reply(
                    &mut p,
                    &Some(vec![1, 0, 0, 0, 0, 0, 0, 0]),
                );
            }
            _ => panic!("unexpected wifi bridge call"),
        }
        assert_eq!(call.data.remaining(), 0);
        Ok(p)
    }
}

fn service(host: &Arc<AppHost>) -> Arc<WifiService> {
    let manager = Arc::new(
        super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        ),
    );
    assert!(host.permission_manager.set(manager).is_ok());
    // Deterministic state input instead of observing the real Mac in these tests.
    Arc::new(WifiService {
        host: host.clone(),
        multicast: Arc::default(),
        resources: OnceLock::new(),
        networks: OnceLock::new(),
        bridge: Mutex::new(None),
    })
}

fn invoke(service: &dyn Service, uid: u32, code: u32, p: &Parcel) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 42,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap()
}

#[test]
fn wifi_listener_pair_checks_access_before_null_and_preserves_java_identity_and_errors() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let wifi = service(host);
        let (tx, rx) = mpsc::channel();
        let owner = export(Arc::new(Java(tx.clone())));
        wifi.attach_bridge(Some(owner.binder())).unwrap();
        rx.recv().unwrap(); // initial radio state
        let listener = export(Arc::new(Java(tx)));
        for (code, add) in [
            (wm::ADD_WIFI_STATE_CHANGED_LISTENER, true),
            (wm::REMOVE_WIFI_STATE_CHANGED_LISTENER, false),
        ] {
            for (uid, binder, expected) in [
                (10100, None, EX_SECURITY),
                (10100, Some(listener.binder()), EX_SECURITY),
                (1000, None, EX_ILLEGAL_ARGUMENT),
                (
                    1000,
                    Some(listener.binder()),
                    if add { 0 } else { EX_ILLEGAL_STATE },
                ),
            ] {
                let mut p = Parcel::new();
                if add {
                    wm::AddWifiStateChangedListener { listener: binder }.write(&mut p);
                } else {
                    wm::RemoveWifiStateChangedListener { listener: binder }.write(&mut p);
                }
                let result = invoke(wifi.as_ref(), uid, code, &p);
                let error = result.reader().read_exception().unwrap();
                if expected == 0 {
                    error.unwrap();
                } else {
                    let error = error.unwrap_err();
                    assert_eq!(error.code, expected);
                    if expected == EX_ILLEGAL_ARGUMENT {
                        assert_eq!(error.message, "listener must not be null");
                    }
                    if expected == EX_ILLEGAL_STATE {
                        assert_eq!(error.message, "original error");
                    }
                }
            }
        }
        let added = rx.recv().unwrap();
        let removed = rx.recv().unwrap();
        assert_eq!(added.0, bridge::ADD_WIFI_STATE_CHANGED_LISTENER);
        assert_eq!(removed.0, bridge::REMOVE_WIFI_STATE_CHANGED_LISTENER);
        assert_eq!(added.1, removed.1);
        assert!(added.1.is_some());
        assert!(rx.try_recv().is_err());
    });
}

#[test]
fn wifi_bridge_attachment_is_system_only_and_state_changes_are_not_repeated() {
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    crate::system::with_discovery_binders(|host, _, _, export| {
        let wifi = service(host);
        assert!(host.wifi_service.set(wifi.clone()).is_ok());
        let (tx, rx) = mpsc::channel();
        let owner = export(Arc::new(Java(tx)));
        let endpoint = super::super::system_process::SystemProcess::new(host.clone());
        let mut p = Parcel::new();
        sp::AttachWifiBridge {
            bridge: Some(owner.binder()),
        }
        .write(&mut p);
        let error = invoke(&endpoint, 10100, sp::ATTACH_WIFI_BRIDGE, &p);
        assert_eq!(
            error.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        assert!(rx.try_recv().is_err());
        invoke(&endpoint, 1000, sp::ATTACH_WIFI_BRIDGE, &p)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let initial = rx.recv().unwrap().2;
        wifi.follow_state(initial);
        let changed = if initial == WIFI_STATE_ENABLED {
            WIFI_STATE_DISABLED
        } else {
            WIFI_STATE_ENABLED
        };
        wifi.follow_state(changed);
        wifi.follow_state(changed);
        wifi.follow_state(initial);
        assert_eq!(
            rx.recv().unwrap(),
            (bridge::ON_WIFI_STATE_CHANGED, None, changed)
        );
        assert_eq!(
            rx.recv().unwrap(),
            (bridge::ON_WIFI_STATE_CHANGED, None, initial)
        );
        assert!(rx.try_recv().is_err());
        assert!(wifi.attach_bridge(None).is_err());
        assert!(wifi.attach_bridge(Some(Binder::Local(1))).is_err());
    });
}

#[test]
fn wifi_passpoint_checks_package_but_needs_no_access_permission() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let wifi = service(host);
        let (tx, rx) = mpsc::channel();
        let owner = export(Arc::new(Java(tx)));
        wifi.attach_bridge(Some(owner.binder())).unwrap();
        rx.recv().unwrap();
        // p belongs to 10100; an unprivileged owner and root both see the empty list.
        for (uid, package, expected) in [
            (10100, None, EX_SECURITY),
            (10101, Some("p"), EX_SECURITY),
            (10100, Some("p"), 0),
            (0, Some("root"), 0),
        ] {
            let mut p = Parcel::new();
            wm::GetPasspointConfigurations {
                package_name: package.map(str::to_owned),
            }
            .write(&mut p);
            let result = invoke(wifi.as_ref(), uid, wm::GET_PASSPOINT_CONFIGURATIONS, &p);
            let mut r = result.reader();
            let error = r.read_exception().unwrap();
            if expected == 0 {
                error.unwrap();
                assert_eq!(r.read_i32().unwrap(), 1); // nonnull typed slice
                assert_eq!(r.read_i32().unwrap(), 0); // empty
                assert_eq!(r.remaining(), 0);
                assert_eq!(rx.recv().unwrap().0, bridge::GET_PASSPOINT_CONFIGURATIONS);
            } else {
                assert_eq!(error.unwrap_err().code, expected);
            }
        }
        assert!(rx.try_recv().is_err());
    });
}

#[test]
fn wifi_missing_bridge_and_other_calls_explicitly_name_2261() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let wifi = service(host);
        let e = wifi
            .state_listener(1000, Some(Binder::Local(1)), true)
            .unwrap_err();
        assert_eq!(e.code, EX_UNSUPPORTED_OPERATION);
        assert!(e.message.contains("#2261"));
        for &(code, name) in wm::METHODS {
            if [
                wm::ADD_WIFI_STATE_CHANGED_LISTENER,
                wm::REMOVE_WIFI_STATE_CHANGED_LISTENER,
                wm::GET_PASSPOINT_CONFIGURATIONS,
                wm::ACQUIRE_MULTICAST_LOCK,
                wm::RELEASE_MULTICAST_LOCK,
                wm::IS_MULTICAST_ENABLED,
                wm::GET_WIFI_ENABLED_STATE,
                wm::GET_VERBOSE_LOGGING_LEVEL,
                wm::IS_SCAN_ALWAYS_AVAILABLE,
                wm::GET_CONNECTION_INFO,
                wm::GET_SCAN_RESULTS,
                wm::GET_CONFIGURED_NETWORKS,
            ]
            .contains(&code)
            {
                continue;
            }
            let e = invoke(wifi.as_ref(), 1000, code, &Parcel::new());
            let e = e.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(e.code, EX_UNSUPPORTED_OPERATION, "{name}");
            assert!(e.message.contains("#2261"), "{name}: {}", e.message);
            assert!(!e.message.contains("#1686"));
        }
    });
}
