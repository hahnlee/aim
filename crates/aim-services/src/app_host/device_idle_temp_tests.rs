//! DeviceIdleController's temp lifetime and PowerAllowlistInternal delivery, Android 16 r1.
use super::*;

fn fixture(test: impl FnOnce(Arc<DeviceIdle>, mpsc::Receiver<Work>)) {
    const FLAGS: &[(&str, bool)] = &[("android.permission.flags.appop_mode_caching_enabled", true)];
    crate::system::with_discovery_binders_flags(FLAGS, true, |host, _, _, _| {
        for (path, xml) in [
            (
                "misc/apexdata/com.android.permission/access.abx",
                "<access><permissions><permission name='android.permission.INTERNET' packageName='android' protectionLevel='0' type='0'/><permission name='android.permission.ACCESS_NETWORK_STATE' packageName='android' protectionLevel='0' type='0'/></permissions></access>",
            ),
            (
                "misc_de/0/apexdata/com.android.permission/access.abx",
                "<access><app-id-permissions><app-id id='10100'><permission name='android.permission.INTERNET' flags='1'/><permission name='android.permission.ACCESS_NETWORK_STATE' flags='1'/></app-id></app-id-permissions></access>",
            ),
        ] {
            let path = host.data().unwrap().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, xml).unwrap();
        }
        host.permission_manager
            .set(Arc::new(
                super::super::permission_manager::PermissionManager::new(host.clone()),
            ))
            .ok()
            .unwrap();
        host.inputs()
            .unwrap()
            .settings
            .set_config(
                "device_idle/",
                &mut [(
                    "device_idle/max_temp_app_allowlist_duration_ms".into(),
                    Some("100000".into()),
                )],
                Some("android"),
                true,
            )
            .unwrap();
        let (tx, rx) = mpsc::channel();
        test(
            Arc::new(DeviceIdle {
                host: host.clone(),
                lists: Mutex::new(Some(Lists::default())),
                handler: Mutex::new(tx),
                listeners: Mutex::new(Vec::new()),
                temp_changes: Mutex::new(()),
            }),
            rx,
        );
    });
}

#[test]
fn missing_packages_users_and_exact_capped_attribution() {
    fixture(|idle, _| {
        for (package, user) in [(None, 0), (Some("missing"), 0), (Some("p"), 999)] {
            idle.add_temp(12345, package, 10000, 1, user, 306, None)
                .unwrap();
            assert!(idle.with(|l| l.temp.is_empty()).unwrap());
        }
        for (i, duration) in [1, 100000, 100001, i64::MAX, 0, -10]
            .into_iter()
            .enumerate()
        {
            let uid = 12345 + i as i32;
            let before = idle.host.temp_allowlist_now().unwrap();
            idle.add_temp_direct(54321, uid, duration, 1, 306, Some("sync by top app".into()))
                .unwrap();
            let after = idle.host.temp_allowlist_now().unwrap();
            idle.with(|l| {
                let entry = &l.temp[&uid];
                let duration = duration.min(100000);
                assert!((before + duration..=after + duration).contains(&entry.end_ms));
                assert_eq!(entry.check_at_ms, entry.end_ms);
                assert_eq!(
                    entry.allowance,
                    TempAllowance {
                        duration_ms: duration,
                        kind: 1,
                        reason_code: 306,
                        reason: Some("sync by top app".into()),
                        calling_uid: 54321
                    }
                );
            })
            .unwrap();
        }
    });
}

#[test]
fn renewals_reset_deadlines_but_keep_first_reason_timeout_and_uid() {
    fixture(|idle, rx| {
        let uid = 112345;
        idle.add_temp_direct(1000, uid, 50000, 1, 306, Some("first".into()))
            .unwrap();
        let first = idle.with(|l| l.temp[&12345].end_ms).unwrap();
        idle.add_temp_direct(
            2000,
            uid + PER_USER_RANGE,
            100000,
            1,
            307,
            Some("second".into()),
        )
        .unwrap();
        let end = idle
            .with(|l| {
                let e = &l.temp[&12345];
                assert_eq!(
                    (e.uid, e.reason.as_deref(), e.check_at_ms),
                    (uid, Some("first"), first)
                );
                assert_eq!(
                    (e.allowance.calling_uid, e.allowance.reason_code),
                    (2000, 307)
                );
                assert_eq!(e.allowance.reason.as_deref(), Some("second"));
                e.end_ms
            })
            .unwrap();
        assert_eq!(idle.expire_at(first - 1).unwrap(), Some(first));
        assert_eq!(idle.expire_at(first).unwrap(), Some(end));
        assert_eq!(idle.expire_at(end - 1).unwrap(), Some(end));
        assert_eq!(idle.expire_at(end).unwrap(), None);
        let changes: Vec<_> = rx
            .try_iter()
            .filter_map(|w| match w {
                Work::TempChanged(uid, added) => Some((uid, added)),
                _ => None,
            })
            .collect();
        assert_eq!(changes, [(uid, true), (uid, false)]);
        idle.add_temp_direct(1000, uid, 90000, 1, 306, None)
            .unwrap();
        let pending = idle.with(|l| l.temp[&12345].check_at_ms).unwrap();
        idle.add_temp_direct(1000, uid, 4000, 1, 306, None).unwrap();
        let shorter = idle.with(|l| l.temp[&12345].end_ms).unwrap();
        assert!(shorter < pending);
        assert_eq!(idle.expire_at(shorter).unwrap(), Some(pending));
        assert_eq!(idle.expire_at(pending).unwrap(), None);
    });
}

struct Listener(Box<dyn Fn(i32, bool) + Send + Sync>);
impl TempAllowlistChangeListener for Listener {
    fn on_app_added(&self, uid: i32) {
        (self.0)(uid, true);
    }
    fn on_app_removed(&self, uid: i32) {
        (self.0)(uid, false);
    }
}

#[test]
fn listeners_snapshot_at_delivery_and_allow_reentrant_unregister() {
    fixture(|idle, rx| {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = seen.clone();
        let second: Arc<dyn TempAllowlistChangeListener> =
            Arc::new(Listener(Box::new(move |uid, added| {
                events.lock().unwrap().push((2, uid, added));
            })));
        let (weak, removed, events) = (Arc::downgrade(&idle), second.clone(), seen.clone());
        let first: Arc<dyn TempAllowlistChangeListener> =
            Arc::new(Listener(Box::new(move |uid, added| {
                let idle = weak.upgrade().unwrap();
                idle.unregister_temp_allowlist_listener(&removed);
                idle.with(|_| ()).unwrap(); // Delivery holds neither registry nor membership lock.
                events.lock().unwrap().push((1, uid, added));
            })));
        idle.register_temp_allowlist_listener(first.clone());
        idle.register_temp_allowlist_listener(first.clone());
        idle.register_temp_allowlist_listener(second.clone());
        idle.add_temp_direct(1000, 12346, 10000, 1, 306, None)
            .unwrap();
        idle.add_temp_direct(1000, 12345, 20000, 1, 306, None)
            .unwrap();
        idle.add_temp_direct(1000, 12345, 30000, 1, 306, None)
            .unwrap();
        idle.expire_at(i64::MAX).unwrap();
        for work in rx.try_iter() {
            if let Work::TempChanged(uid, added) = work {
                idle.deliver_temp_change(uid, added);
            }
        }
        assert_eq!(
            *seen.lock().unwrap(),
            [
                (1, 12346, true),
                (2, 12346, true),
                (1, 12345, true),
                (1, 12346, false),
                (1, 12345, false),
            ]
        );
        idle.unregister_temp_allowlist_listener(&first);
        idle.deliver_temp_change(12345, true);
        assert_eq!(seen.lock().unwrap().len(), 5);
    });
}

#[test]
fn handler_delivers_add_before_immediate_expiry_without_duplicate_remove() {
    fixture(|manual, _| {
        let idle = manual.host.device_idle();
        let (tx, rx) = mpsc::channel();
        let listener: Arc<dyn TempAllowlistChangeListener> =
            Arc::new(Listener(Box::new(move |uid, added| {
                tx.send((uid, added)).unwrap();
            })));
        idle.register_temp_allowlist_listener(listener.clone());
        idle.add_temp_direct(1000, 12345, 0, 1, 306, None).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            (12345, true)
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            (12345, false)
        );
        idle.expire_at(i64::MAX).unwrap();
        assert!(rx.try_recv().is_err());
        idle.unregister_temp_allowlist_listener(&listener);
    });
}

fn package_uid(host: &AppHost) -> i32 {
    host.installed(0, 0)
        .unwrap()
        .into_iter()
        .find(|(n, _, _)| n == "p")
        .unwrap()
        .1
}

fn invoke(owner: &dyn Service, caller: u32, code: u32, data: &Parcel) -> Parcel {
    owner
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 1,
            sender_euid: caller,
            data: data.reader(),
        })
        .unwrap()
}

fn setting(host: &AppHost, table: Table, name: &str, value: &str) {
    host.platform()
        .unwrap()
        .settings
        .insert(
            table,
            name,
            &super::super::settings::Change {
                value: Some(value),
                tag: None,
                make_default: false,
                package: Some("android"),
                caller_system: true,
                overrideable_by_restore: false,
            },
            false,
        )
        .unwrap();
}

fn policy_inputs(host: &Arc<AppHost>) -> aim_storage::system::ImageLease {
    let lease = aim_storage::system::ImageLease::read(&aim_paths::system_image()).unwrap();
    let framework =
        super::super::resources::Framework::load(&aim_paths::original_image(), 160, "en-US")
            .unwrap();
    host.framework.set(framework).ok().unwrap();
    assert!(host.app_idle_enabled().unwrap());
    lease
}

fn deliver(idle: &DeviceIdle, rx: &mpsc::Receiver<Work>) {
    for work in rx.try_iter() {
        match work {
            Work::NetworkTemp(uid, added) => idle
                .host
                .network_policy()
                .temp_allowlist_changed(uid, added),
            Work::TempChanged(uid, added) => idle.deliver_temp_change(uid, added),
            _ => {}
        }
    }
}

#[test]
fn consumers_follow_membership_and_removal_revokes_only_the_original_uids_fgs() {
    fixture(|idle, rx| {
        let host = &idle.host;
        let uid = package_uid(host);
        let app = crate::package::info::ApplicationInfo {
            uid,
            target_sdk_version: 35,
            ..Default::default()
        };
        assert_eq!(
            host.background_service_start_mode(&app, "p", None, false)
                .unwrap(),
            2
        );
        idle.add_temp_direct(1000, uid, 90000, 0, 306, None)
            .unwrap();
        let pending = idle.with(|l| l.temp[&uid].check_at_ms).unwrap();
        // The shared app-id's shorter type-1 renewal cannot extend FGS permission.
        idle.add_temp_direct(1000, uid + PER_USER_RANGE, 1, 1, 307, None)
            .unwrap();
        assert_eq!(host.fgs_temp_reason_for_test(uid), Some(306));
        assert_eq!(host.fgs_temp_reason_for_test(uid + PER_USER_RANGE), None);
        // A separate, longer FGS grant must still be revoked for the timeout's UID.
        for u in [uid, uid + PER_USER_RANGE] {
            host.temp_allowlist_for_fgs(
                u,
                TempAllowance {
                    duration_ms: 100000,
                    kind: 0,
                    reason_code: 308,
                    reason: None,
                    calling_uid: 1000,
                },
            )
            .unwrap();
        }
        assert_eq!(
            host.background_service_start_mode(&app, "p", None, false)
                .unwrap(),
            0
        );
        assert_eq!(host.device_idle_temp_ids(), [uid]);
        assert_eq!(host.power_service.get().unwrap().temp_ids_for_test(), [uid]);
        let (tx, events) = mpsc::channel();
        let listener: Arc<dyn TempAllowlistChangeListener> =
            Arc::new(Listener(Box::new(move |u, on| {
                tx.send((u, on)).unwrap();
            })));
        idle.register_temp_allowlist_listener(listener.clone());
        deliver(&idle, &rx);
        assert_eq!(events.try_recv().unwrap(), (uid, true));
        assert_eq!(host.network_policy().temp_ids_for_test(), [uid]);
        assert!(events.try_recv().is_err());
        assert_eq!(idle.expire_at(pending - 1).unwrap(), Some(pending));
        assert_eq!(host.fgs_temp_reason_for_test(uid), Some(308));
        assert_eq!(idle.expire_at(pending).unwrap(), None);
        assert!(host.device_idle_temp_ids().is_empty());
        assert!(
            host.power_service
                .get()
                .unwrap()
                .temp_ids_for_test()
                .is_empty()
        );
        assert_eq!(host.fgs_temp_reason_for_test(uid), None);
        assert_eq!(
            host.fgs_temp_reason_for_test(uid + PER_USER_RANGE),
            Some(308)
        );
        assert_eq!(
            host.background_service_start_mode(&app, "p", None, false)
                .unwrap(),
            2
        );
        deliver(&idle, &rx);
        assert_eq!(events.try_recv().unwrap(), (uid, false));
        assert!(host.network_policy().temp_ids_for_test().is_empty());
        idle.expire_at(i64::MAX).unwrap();
        deliver(&idle, &rx);
        assert!(events.try_recv().is_err());
        idle.unregister_temp_allowlist_listener(&listener);
    });
}

#[test]
fn generated_sync_network_updates_keep_guards_types_and_actual_non_doze_state() {
    use aim_service_aidl::{android_os_ipowermanager as pm, dev_aim_server_isystemprocess as sp};
    fixture(|idle, _| {
        let host = &idle.host;
        let uid = package_uid(host);
        let owner = super::super::system_process::SystemProcess::new(host.clone());
        let grant = |caller, kind, reason_code| {
            let mut p = Parcel::new();
            sp::AddSyncTempAllowlist {
                calling_uid: 12345,
                package_name: Some("p".into()),
                duration_ms: 90000,
                r#type: kind,
                user_id: 0,
                sync: true,
                reason_code,
                reason: None,
            }
            .write(&mut p);
            let response = invoke(&owner, caller, sp::ADD_SYNC_TEMP_ALLOWLIST, &p);
            sp::read_add_sync_temp_allowlist_reply(&mut response.reader()).unwrap()
        };
        assert_eq!(
            grant(uid as u32, 1, 306).unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        assert_eq!(
            grant(1000, 2, 306).unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
        grant(1000, 1, -1).unwrap();
        assert!(host.device_idle_temp_ids().is_empty());
        grant(1000, 1, 306).unwrap();
        assert_eq!(host.device_idle_temp_ids(), [uid]);
        assert_eq!(host.network_policy().temp_ids_for_test(), [uid]);
        assert_eq!(host.fgs_temp_reason_for_test(uid), None);
        // Actual active device queries remain false in both membership states.
        for code in [pm::IS_DEVICE_IDLE_MODE, pm::IS_LIGHT_DEVICE_IDLE_MODE] {
            let mut data = Parcel::new();
            data.write_interface_token(pm::DESCRIPTOR);
            let response = invoke(
                host.power_service.get().unwrap().as_ref(),
                1000,
                code,
                &data,
            );
            let mut r = response.reader();
            r.read_exception().unwrap().unwrap();
            assert!(!r.read_bool().unwrap());
        }
    });
}

#[test]
fn network_transitions_recheck_standby_and_saver_without_changing_metered_policy() {
    use aim_service_aidl::android_net_inetworkpolicymanager as np;
    fixture(|idle, rx| {
        let host = &idle.host;
        let _image = policy_inputs(host);
        host.with_standby_checks_paused(|| {
            let uid = package_uid(host);
            assert_eq!(host.check_permission(Some("android.permission.INTERNET"), uid).unwrap(), 0);
            let battery = Arc::new(super::super::battery_stats::BatteryStats::for_jobs_test(host));
            battery.set_battery_state(super::super::battery_stats::BatteryState { status: 3, level: 50, ..Default::default() }, 0);
            host.battery_stats.set(battery).ok().unwrap();
            let network = host.network_policy();
            let path = host.data().unwrap().join("system/netpolicy.xml");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("<policy-list version='14' restrictBackground='true'><uid-policy uid='{uid}' policy='1'/></policy-list>")).unwrap();
            // Read permissions are checked against the actual authenticated caller.
            for (caller, expected) in [(uid as u32, false), (1000, true)] {
                let mut p = Parcel::new();
                np::GetRestrictBackgroundStatus { uid }.write(&mut p);
                let response = invoke(network.as_ref(), caller, np::GET_RESTRICT_BACKGROUND_STATUS, &p);
                let result = np::read_get_restrict_background_status_reply(&mut response.reader()).unwrap();
                if expected { assert_eq!(result.unwrap(), 3); }
                else { assert_eq!(result.unwrap_err().code, aim_binder_host::parcel::EX_SECURITY); }
            }
            for bucket in [10, 45] {
                host.set_restriction_test_bucket("p", bucket, 0x600).unwrap();
                setting(host, Table::Global, "low_power", if bucket == 10 { "1" } else { "0" });
                setting(host, Table::Global, "battery_saver_constants", "enable_firewall=true,enable_datasaver=false");
                assert_eq!(network.check_power_restrictions(uid).unwrap_err().code, EX_UNSUPPORTED_OPERATION);
                idle.add_temp_direct(1000, uid, 90000, 1, 306, None).unwrap();
                assert!(network.temp_ids_for_test().is_empty());
                deliver(&idle, &rx);
                network.check_power_restrictions(uid).unwrap();
                network.check_power_restrictions(uid + PER_USER_RANGE).unwrap();
                let mut p = Parcel::new();
                np::GetRestrictBackgroundByCaller {}.write(&mut p);
                let response = invoke(network.as_ref(), uid as u32, np::GET_RESTRICT_BACKGROUND_BY_CALLER, &p);
                assert_eq!(np::read_get_restrict_background_by_caller_reply(&mut response.reader()).unwrap().unwrap(), 3);
                let response = invoke(network.as_ref(), 10101, np::GET_RESTRICT_BACKGROUND_BY_CALLER, &p);
                assert_eq!(response.reader().read_exception().unwrap().unwrap_err().code, aim_binder_host::parcel::EX_SECURITY);
                idle.expire_at(i64::MAX).unwrap();
                network.check_power_restrictions(uid).unwrap(); // Removal still queued.
                deliver(&idle, &rx);
                assert!(network.temp_ids_for_test().is_empty());
                let e = network.check_power_restrictions(uid).unwrap_err();
                assert!(e.message.contains(if bucket == 10 { "#1489" } else { "#1711" }));
            }
            setting(host, Table::Global, "low_power", "1");
            setting(host, Table::Global, "battery_saver_constants", "enable_datasaver=true");
            let mut p = Parcel::new();
            np::GetRestrictBackgroundByCaller {}.write(&mut p);
            let response = invoke(network.as_ref(), uid as u32, np::GET_RESTRICT_BACKGROUND_BY_CALLER, &p);
            assert_eq!(response.reader().read_exception().unwrap().unwrap_err().code, EX_UNSUPPORTED_OPERATION);
        });
    });
}

#[test]
fn legacy_service_starts_follow_restricted_battery_appops_and_foreground_caller() {
    use super::super::{appops_service, battery_stats};
    fixture(|idle, _| {
        let host = &idle.host;
        let _image = policy_inputs(host);
        host.with_standby_checks_paused(|| {
            let uid = package_uid(host);
            let mut app = crate::package::info::ApplicationInfo {
                uid,
                target_sdk_version: 25,
                ..Default::default()
            };
            let mode = |app: &crate::package::info::ApplicationInfo, pid, forced| {
                host.background_service_start_mode(app, "p", pid, forced)
                    .unwrap()
            };
            let permissions = host.permission_manager.get().unwrap();
            let battery = Arc::new(battery_stats::BatteryStats::for_jobs_test(host));
            host.battery_stats.set(battery.clone()).ok().unwrap();
            host.set_restriction_test_bucket("p", 10, 0x600).unwrap();
            assert_eq!(mode(&app, None, false), 0);
            permissions
                .set_app_op_mode(uid, Some("p"), 63, appops_service::MODE_IGNORED)
                .unwrap();
            assert_eq!(mode(&app, None, false), 1);
            let donor = super::super::tests::host_with_start(7, Arc::new(|| {}));
            let mut caller = donor.apps.lock().unwrap().remove(&7).unwrap();
            caller.proc_state = 2;
            host.apps.lock().unwrap().insert(7, caller);
            assert_eq!(mode(&app, Some(4321), false), 0);
            host.apps.lock().unwrap().get_mut(&7).unwrap().proc_state = 16;
            assert_eq!(mode(&app, Some(4321), false), 1);
            permissions
                .set_app_op_mode(uid, Some("p"), 63, appops_service::MODE_ERRORED)
                .unwrap();
            assert_eq!(mode(&app, Some(4321), false), 2);
            permissions.set_app_op_mode(uid, Some("p"), 63, 0).unwrap();
            host.set_restriction_test_bucket("p", 45, 0x600).unwrap();
            assert_eq!(mode(&app, None, false), 0); // Plugged in.
            battery.set_battery_state(
                battery_stats::BatteryState {
                    status: 3,
                    level: 50,
                    ..Default::default()
                },
                0,
            );
            assert_eq!(mode(&app, None, false), 1);
            setting(
                host,
                Table::Config,
                "activity_manager/force_bg_check_on_restricted",
                "false",
            );
            assert_eq!(mode(&app, None, false), 0);
            setting(host, Table::Global, "low_power", "1");
            setting(
                host,
                Table::Global,
                "battery_saver_constants",
                "force_background_check=true",
            );
            assert_eq!(mode(&app, None, false), 1);
            setting(
                host,
                Table::Global,
                "battery_saver_constants",
                "force_background_check=false",
            );
            assert_eq!(mode(&app, None, false), 0);
            app.target_sdk_version = 35;
            host.apps.lock().unwrap().get_mut(&7).unwrap().uid = uid as u32;
            assert_eq!(mode(&app, None, false), 0);
            assert_eq!(mode(&app, None, true), 2);
            idle.add_temp_direct(1000, uid, 90000, 1, 306, None)
                .unwrap();
            assert_eq!(mode(&app, None, true), 0);
            assert_eq!(host.fgs_temp_reason_for_test(uid), None);
            idle.expire_at(i64::MAX).unwrap();
            assert_eq!(mode(&app, None, true), 2);
            host.apps.lock().unwrap().remove(&7);
        });
    });
}

#[test]
fn temporary_membership_history_keeps_initial_reason_and_balanced_user_zero_events() {
    use super::super::battery_history::tests::{events, history};
    fixture(|idle, _| {
        let stats = idle.battery_stats();
        let mut h = history(
            &idle.host.data().unwrap().join("history-test"),
            128 * 1024,
            0,
        );
        h.set_initial_battery_state(false, 3, 50, 0);
        h.start_recording(0, true);
        stats.set_history_for_test(h);
        idle.add_temp_direct(54321, 12345, 50000, 1, 306, Some("sync by top app".into()))
            .unwrap();
        let first = idle.with(|l| l.temp[&12345].check_at_ms).unwrap();
        idle.add_temp_direct(1000, 112345, 100000, 0, 307, Some("renewal".into()))
            .unwrap();
        let end = idle.with(|l| l.temp[&12345].end_ms).unwrap();
        assert_eq!(
            events(&stats.history_buffer()),
            [(0x8011, "sync by top app".into(), 12345)]
        );
        assert_eq!(idle.expire_at(first).unwrap(), Some(end));
        assert_eq!(idle.expire_at(end).unwrap(), None);
        assert_eq!(idle.expire_at(end).unwrap(), None);
        assert_eq!(
            events(&stats.history_buffer()),
            [
                (0x8011, "sync by top app".into(), 12345),
                (0x4011, "sync by top app".into(), 12345),
            ]
        );
        idle.add_temp_direct(1000, 12345, 0, 1, 306, None).unwrap();
        let end = idle.with(|l| l.temp[&12345].end_ms).unwrap();
        idle.expire_at(end).unwrap();
        assert_eq!(
            &events(&stats.history_buffer())[2..],
            [(0x8011, "".into(), 12345), (0x4011, "".into(), 12345),]
        );
    });
}

#[test]
fn temporary_history_preserves_pinned_cross_user_uid_asymmetry_and_failed_adds() {
    use super::super::battery_history::tests::{events, history};
    fixture(|idle, _| {
        let stats = idle.battery_stats();
        let mut h = history(
            &idle.host.data().unwrap().join("history-test"),
            128 * 1024,
            0,
        );
        h.set_initial_battery_state(false, 3, 50, 0);
        h.start_recording(0, true);
        stats.set_history_for_test(h);
        idle.add_temp(1000, Some("missing"), 1000, 1, 0, 306, None)
            .unwrap();
        for (caller, reason_code, exception) in [
            (10100, 306, aim_binder_host::parcel::EX_SECURITY),
            (
                1000,
                REASON_PUSH_MESSAGING_OVER_QUOTA,
                EX_UNSUPPORTED_OPERATION,
            ),
        ] {
            let mut data = Parcel::new();
            di::AddPowerSaveTempWhitelistApp {
                name: Some("p".into()),
                duration: 1000,
                user_id: 0,
                reason_code,
                reason: Some("rejected".into()),
            }
            .write(&mut data);
            let reply = invoke(
                idle.as_ref(),
                caller,
                di::ADD_POWER_SAVE_TEMP_WHITELIST_APP,
                &data,
            );
            assert_eq!(
                reply.reader().read_exception().unwrap().unwrap_err().code,
                exception
            );
            assert!(idle.with(|l| l.temp.is_empty()).unwrap());
        }
        assert!(events(&stats.history_buffer()).is_empty());
        idle.add_temp_direct(1000, 112345, 50000, 1, 306, Some("user one".into()))
            .unwrap();
        let end = idle.with(|l| l.temp[&12345].end_ms).unwrap();
        idle.expire_at(end).unwrap();
        assert!(idle.with(|l| l.temp.is_empty()).unwrap());
        // AOSP adds with uid 112345, removes with appId 12345. Its history
        // tracker therefore drops the unmatched finish and subsequent duplicate start.
        idle.add_temp_direct(1000, 112345, 50000, 1, 306, Some("user one".into()))
            .unwrap();
        assert_eq!(
            events(&stats.history_buffer()),
            [(0x8011, "user one".into(), 112345)]
        );
    });
}
