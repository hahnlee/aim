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
                wm::GET_CURRENT_NETWORK,
                wm::GET_CONNECTION_INFO,
                wm::GET_SCAN_RESULTS,
                wm::GET_CONFIGURED_NETWORKS,
                wm::GET_PRIVILEGED_CONFIGURED_NETWORKS,
                wm::GET_WIFI_CONFIG_FOR_MATCHED_NETWORK_SUGGESTIONS_SHARED_WITH_USER,
                wm::GET_ALL_MATCHING_PASSPOINT_PROFILES_FOR_SCAN_RESULTS,
                wm::GET_MATCHING_OSU_PROVIDERS,
                wm::GET_MATCHING_PASSPOINT_CONFIGS_FOR_OSU_PROVIDERS,
                wm::GET_WIFI_CONFIGS_FOR_PASSPOINT_PROFILES,
                wm::IS_AUTO_WAKEUP_ENABLED,
            ]
            .contains(&code)
            {
                continue;
            }
            let e = invoke(wifi.as_ref(), 1000, code, &Parcel::new());
            let e = e.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(e.code, EX_UNSUPPORTED_OPERATION, "{name}");
            let issue = if code == wm::START_SCAN {
                "#2459"
            } else {
                "#2261"
            };
            assert!(e.message.contains(issue), "{name}: {}", e.message);
            assert!(!e.message.contains("#1686"));
        }
    });
}

/// WifiServiceImpl / WifiConfigManager at android-16.0.0_r1: denial order,
/// and the shared conversion's password/MAC policy.
#[test]
fn privileged_networks_preserve_credentials_macs_and_empty_slice_protocol() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let wifi = service(host);
        let mut p = Parcel::new();
        wm::GetPrivilegedConfiguredNetworks::<NoConnection> {
            package_name: None,
            feature_id: None,
            extras: None,
        }
        .write(&mut p);
        let denied = invoke(
            wifi.as_ref(),
            10100,
            wm::GET_PRIVILEGED_CONFIGURED_NETWORKS,
            &p,
        );
        let error = denied.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(error.code, EX_SECURITY);
        assert_eq!(
            error.message,
            "WifiService: Neither user 10100 nor current process has android.permission.READ_WIFI_CREDENTIAL."
        );
        wifi.resources
            .set(WifiResources {
                mac_randomization_supported: true,
                owe_upgrade: true,
                sae_upgrade: true,
                oem_admins: vec![],
            })
            .ok()
            .unwrap();
        let encode = |slice: Slice<WifiConfiguration>| {
            let mut p = Parcel::new();
            wm::write_get_privileged_configured_networks_reply(&mut p, Some(&slice));
            p.data().to_vec()
        };
        assert_eq!(
            encode(
                wifi.network_configurations(0, WIFI_UID, false, false)
                    .unwrap()
            ),
            [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        let mut c = WifiConfiguration::default();
        c.pre_shared_key = Some("\"password\"".into());
        c.randomized_mac = 0x6e53_d6a0_fc69;
        c.security = vec![SecurityParams::of(wifi_config::SECURITY_TYPE_PSK).unwrap()];
        let mut wifi = Arc::try_unwrap(wifi).ok().unwrap();
        wifi.networks = OnceLock::from(vec![c.clone()]);
        let privileged = encode(
            wifi.network_configurations(0, WIFI_UID, false, false)
                .unwrap(),
        );
        let mut masked = c.external(WIFI_UID, true, 0);
        assert_eq!(masked.pre_shared_key.as_deref(), Some("*"));
        assert_eq!(masked.randomized_mac, c.randomized_mac);
        let expected = |c| {
            encode(Slice::new(
                host.process.clone(),
                WIFI_CONFIGURATION,
                wifi_config::legacy_configs(&[c], false, &|_| false),
            ))
        };
        assert_eq!(privileged, expected(c.clone()));
        assert_eq!(
            encode(wifi.configured_networks(0, Some("root"), false).unwrap()),
            expected(masked.clone())
        );
        // The privileged API also includes configured ephemeral/suggestion entries.
        masked.ephemeral = true;
        wifi.networks = OnceLock::from(vec![masked]);
        assert_ne!(
            encode(
                wifi.network_configurations(0, WIFI_UID, false, false)
                    .unwrap()
            ),
            encode(wifi.configured_networks(0, Some("root"), false).unwrap())
        );
    });
}

/// WifiServiceImpl's Settings/SUW checks precede null/empty matching results.
#[test]
fn matching_queries_check_permissions_before_empty_results() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let wifi = service(host);
        for code in [
            wm::GET_WIFI_CONFIG_FOR_MATCHED_NETWORK_SUGGESTIONS_SHARED_WITH_USER,
            wm::GET_ALL_MATCHING_PASSPOINT_PROFILES_FOR_SCAN_RESULTS,
            wm::GET_MATCHING_OSU_PROVIDERS,
            wm::GET_MATCHING_PASSPOINT_CONFIGS_FOR_OSU_PROVIDERS,
            wm::GET_WIFI_CONFIGS_FOR_PASSPOINT_PROFILES,
        ] {
            for present in [false, true] {
                let mut p = Parcel::new();
                p.write_interface_token(wm::DESCRIPTOR);
                p.write_i32(i32::from(present));
                if present {
                    p.write_i32(0);
                }
                for uid in [10100, 1000] {
                    let result = invoke(wifi.as_ref(), uid, code, &p);
                    let mut r = result.reader();
                    if uid == 10100 {
                        let e = r.read_exception().unwrap().unwrap_err();
                        assert_eq!(e.code, EX_SECURITY);
                        assert_eq!(e.message, "WifiService: Permission denied");
                    } else {
                        r.read_exception().unwrap().unwrap();
                        if [
                            wm::GET_WIFI_CONFIG_FOR_MATCHED_NETWORK_SUGGESTIONS_SHARED_WITH_USER,
                            wm::GET_WIFI_CONFIGS_FOR_PASSPOINT_PROFILES,
                        ]
                        .contains(&code)
                        {
                            assert_eq!(r.read_i32().unwrap(), 1);
                        }
                        assert_eq!(r.read_i32().unwrap(), 0);
                        assert_eq!(r.remaining(), 0);
                    }
                }
            }
            let nonempty = wifi.empty_matches(1000, Some(&MatchingList(1)), code);
            if code == wm::GET_MATCHING_OSU_PROVIDERS {
                assert!(nonempty.unwrap_err().message.contains("ANQP"));
            } else {
                nonempty.unwrap();
            }
        }
    });
}

#[test]
fn current_network_checks_settings_or_suw_before_radio_state() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let wifi = service(host);
        for state in [WIFI_STATE_DISABLED, WIFI_STATE_ENABLED] {
            let error = wifi
                .current_network(10100, || panic!("denied caller read radio state {state}"))
                .err()
                .unwrap();
            assert_eq!(error.code, EX_SECURITY);
            assert_eq!(error.message, "WifiService: Permission denied");
        }
        let network = wifi.current_network(1000, || WIFI_STATE_DISABLED).unwrap();
        assert!(network.is_none());
        let mut p = Parcel::new();
        wm::write_get_current_network_reply(&mut p, network.as_ref());
        let mut r = p.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0); // typed null Network
        assert_eq!(r.remaining(), 0);
        let error = wifi
            .current_network(1000, || WIFI_STATE_ENABLED)
            .err()
            .unwrap();
        assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
        assert!(error.message.contains("current Wi-Fi Network (#2459)"));
        let mut request = Parcel::new();
        wm::GetCurrentNetwork {}.write(&mut request);
        let denied = invoke(wifi.as_ref(), 10100, wm::GET_CURRENT_NETWORK, &request);
        assert_eq!(
            denied.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
    });
}

#[test]
fn wifi_wakeup_checks_access_and_reads_the_live_global_setting() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let wifi = service(host);
        let mut p = Parcel::new();
        wm::IsAutoWakeupEnabled {}.write(&mut p);
        let denied = invoke(wifi.as_ref(), 10100, wm::IS_AUTO_WAKEUP_ENABLED, &p);
        let e = denied.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(e.code, EX_SECURITY);
        assert_eq!(
            e.message,
            "WifiService: Neither user 10100 nor current process has android.permission.ACCESS_WIFI_STATE."
        );
        for (value, expected) in [
            (None, false),
            (Some("1"), true),
            (Some("2"), false),
            (Some("bad"), false),
        ] {
            host.inputs()
                .unwrap()
                .settings
                .insert(
                    Table::Global,
                    "wifi_wakeup_enabled",
                    &super::super::settings::Change {
                        value,
                        tag: None,
                        make_default: false,
                        package: Some("android"),
                        caller_system: true,
                        overrideable_by_restore: false,
                    },
                    false,
                )
                .unwrap();
            let result = invoke(wifi.as_ref(), 1000, wm::IS_AUTO_WAKEUP_ENABLED, &p);
            assert_eq!(
                wm::read_is_auto_wakeup_enabled_reply(&mut result.reader())
                    .unwrap()
                    .unwrap(),
                expected
            );
        }
    });
}

#[test]
fn resources_apk_requires_wifi_action_system_app_and_wifi_apex() {
    use crate::package::{
        intent_filter::{IntentFilter, ParsedIntentInfo},
        pkg::booleans,
    };
    crate::system::with_discovery_session(|host, publish, _| {
        for (system, action, apex, found) in [
            (false, RESOURCES_APK_ACTION, WIFI_APEX, false),
            (true, "other.action", WIFI_APEX, false),
            (
                true,
                RESOURCES_APK_ACTION,
                "/apex/com.android.permission/",
                false,
            ),
            (
                true,
                RESOURCES_APK_ACTION,
                "/apex/com.android.wifi.other/",
                false,
            ),
            (true, RESOURCES_APK_ACTION, WIFI_APEX, true),
        ] {
            let source = format!("{apex}app/Resources/base.apk");
            publish(&mut |package| {
                package.activities.clear();
                if package.package_name != "p" {
                    return;
                }
                if system {
                    package.booleans |= booleans::SYSTEM;
                }
                package.base_apk_path = Some(source.clone());
                let mut activity = crate::package::pkg::Activity::default();
                activity.max_aspect_ratio = Some(0.0);
                activity.min_aspect_ratio = Some(0.0);
                activity.main.enabled = true;
                activity.main.exported = true;
                activity.main.direct_boot_aware = true;
                activity.main.component.package_name = "p".into();
                activity.main.component.name = "p.Resources".into();
                let mut filter = IntentFilter::default();
                filter.add_action(action);
                activity.main.component.intents = vec![ParsedIntentInfo {
                    filter,
                    ..Default::default()
                }];
                package.activities.push(activity);
            });
            let result = resources_apk(host, "wifi", RESOURCES_APK_ACTION, WIFI_APEX).unwrap();
            let expected = found.then(|| ("p".into(), host.root().unwrap().join(&source[1..])));
            assert_eq!(
                result, expected,
                "system={system}, action={action}, apex={apex}"
            );
        }
    });
}
