use super::super::permission_manager::PermissionManager;
use super::*;
use aim_binder_host::parcel::EX_SECURITY;

fn source(uid: i32) -> WorkSource {
    WorkSource {
        num: 1,
        uids: vec![uid],
        ..WorkSource::default()
    }
}

#[test]
fn wake_attribution_permissions_updates_and_release_reach_battery_and_assertion() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let manager = PermissionManager::with_system(host.clone(), Default::default());
        assert!(host.permission_manager.set(Arc::new(manager)).is_ok());
        let system = host.package_system().unwrap();
        let base = host.package_capture().unwrap();
        let mut context = (**base.context()).clone();
        context.system.system_permissions =
            Some([(2901, ["android.permission.WAKE_LOCK".into()].into())].into());
        context.scan_version = base.scan().version() + 1;
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(base.scan()),
                base.scan().owner().clone(),
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        let power = Arc::new(PowerManager::new(host.clone()));
        let token = export(power.clone());
        let p = Parcel::new();
        let call = |uid| Call {
            code: 0,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: p.reader(),
        };
        let acquire = |uid, flags, ws| {
            power.acquire(
                &call(uid),
                Some(token.binder()),
                flags,
                Some("adapter".into()),
                Some("p".into()),
                ws,
                Some("history".into()),
                INVALID_DISPLAY,
                None,
            )
        };
        let screen_name = || {
            power
                .state
                .lock()
                .unwrap()
                .screen_assertion
                .name()
                .map(str::to_owned)
        };
        for uid in [2901, 10100] {
            let e = acquire(uid, PARTIAL_WAKE_LOCK, Some(source(10002))).unwrap_err();
            assert_eq!(e.code, EX_SECURITY);
            assert!(e.message.contains(if uid == 2901 {
                "UPDATE_DEVICE_STATS"
            } else {
                "WAKE_LOCK"
            }));
            assert!(power.state.lock().unwrap().locks.is_empty());
        }
        acquire(1000, PARTIAL_WAKE_LOCK, Some(source(20100))).unwrap(); // SDK sandbox maps to app UID
        assert_eq!(power.battery().wake_nesting(10100, "adapter"), 1);
        acquire(1000, PARTIAL_WAKE_LOCK, Some(source(20100))).unwrap(); // reacquire does not nest
        assert_eq!(power.battery().wake_nesting(10100, "adapter"), 1);
        assert_eq!(
            power
                .update_work_source(&call(2901), Some(token.binder()), Some(source(10003)), None)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        power
            .update_work_source(
                &call(1000),
                Some(token.binder()),
                Some(source(10003)),
                Some("new history".into()),
            )
            .unwrap();
        assert_eq!(power.battery().wake_nesting(10100, "adapter"), 0);
        assert_eq!(power.battery().wake_nesting(10003, "adapter"), 1);
        let state = power.state.lock().unwrap();
        assert_eq!(
            state.locks[0].attribution.history.as_deref(),
            Some("new history")
        );
        assert!(state.screen_assertion.name().is_none());
        drop(state);
        power
            .update_work_source(
                &call(2901),
                Some(token.binder()),
                Some(WorkSource::default()),
                None,
            )
            .unwrap();
        assert_eq!(power.battery().wake_nesting(10003, "adapter"), 0);
        assert_eq!(power.battery().wake_nesting(1000, "adapter"), 1);
        assert_eq!(
            power
                .release(&call(10100), Some(token.binder()))
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(power.battery().wake_nesting(1000, "adapter"), 1);
        power.release(&call(2901), Some(token.binder())).unwrap();
        power.release(&call(2901), Some(token.binder())).unwrap();
        assert_eq!(power.battery().wake_nesting(1000, "adapter"), 0);
        assert!(power.state.lock().unwrap().locks.is_empty());
        assert!(
            power
                .update_work_source(&call(1000), Some(token.binder()), None, None)
                .is_err()
        );
        power
            .acquire(
                &call(1000),
                Some(token.binder()),
                PARTIAL_WAKE_LOCK | SYSTEM_WAKELOCK,
                Some("system".into()),
                Some("p".into()),
                None,
                None,
                INVALID_DISPLAY,
                None,
            )
            .unwrap();
        let state = power.state.lock().unwrap();
        let owner = &state.locks[0].attribution;
        assert_eq!(owner.pid, host.process.pid());
        let ws = owner.source.as_ref().unwrap();
        assert_eq!(ws.names, Some(vec![Some("p".into())]));
        drop(state);
        power.release(&call(1000), Some(token.binder())).unwrap();
        assert!(screen_name().is_none());
        acquire(1000, SCREEN_BRIGHT_WAKE_LOCK, Some(source(20100))).unwrap();
        assert!(screen_name().unwrap().contains("20100"));
        power
            .update_work_source(&call(1000), Some(token.binder()), Some(source(10003)), None)
            .unwrap();
        let state = power.state.lock().unwrap();
        let name = state.screen_assertion.name().unwrap();
        assert!(name.contains("10003") && !name.contains("20100"));
        drop(state);
        power.release(&call(1000), Some(token.binder())).unwrap();
        assert!(screen_name().is_none());
        power.set_window_holds_screen(true);
        assert!(screen_name().unwrap().contains("Android display"));
        power.set_window_holds_screen(false);
        assert!(screen_name().is_none());
        acquire(1000, SCREEN_BRIGHT_WAKE_LOCK, None).unwrap();
        // Android sleep releases the display assertion even with a screen lock.
        let mut state = power.state.lock().unwrap();
        state.asleep = true;
        power.update_blockers(&mut state);
        assert!(state.screen_assertion.name().is_none());
        drop(state);
        power.release(&call(1000), Some(token.binder())).unwrap();
    });
}

#[test]
fn work_chains_keep_identity_and_only_first_nodes_are_charged() {
    let ws = WorkSource::read_from(
        &mut tests::source_with_chain(Some("android.os.WorkSource$WorkChain"), 2).reader(),
    )
    .unwrap();
    let mut changed = ws.clone();
    changed.chains.as_mut().unwrap()[0].tags[1] = Some("changed".into());
    for chained in [false, true] {
        crate::system::with_discovery_binders_flags(
            &[(
                "com.android.server.power.feature.flags.wakelock_attribution_via_workchain",
                chained,
            )],
            true,
            |host, _, _, export| {
                let manager = PermissionManager::with_system(host.clone(), Default::default());
                assert!(host.permission_manager.set(Arc::new(manager)).is_ok());
                let power = Arc::new(PowerManager::new(host.clone()));
                let token = export(power.clone());
                let p = Parcel::new();
                let call = Call {
                    code: 0,
                    flags: 0,
                    sender_pid: 71,
                    sender_euid: 1000,
                    data: p.reader(),
                };
                power
                    .acquire(
                        &call,
                        Some(token.binder()),
                        PARTIAL_WAKE_LOCK,
                        Some("adapter".into()),
                        Some("p".into()),
                        Some(ws.clone()),
                        None,
                        INVALID_DISPLAY,
                        None,
                    )
                    .unwrap();
                power
                    .update_work_source(&call, Some(token.binder()), Some(changed.clone()), None)
                    .unwrap();
                assert_eq!(power.battery().wake_nesting(10001, "adapter"), 1);
                assert_eq!(power.battery().wake_nesting(10002, "adapter"), 1);
                assert_eq!(power.battery().wake_nesting(1000, "adapter"), 0);
                let mut data = Parcel::new();
                pm::UpdateWakeLockUids {
                    lock: Some(token.binder()),
                    uids: Some(vec![10005, 10004, 10005]),
                }
                .write(&mut data);
                power
                    .transact(&mut Call {
                        code: pm::UPDATE_WAKE_LOCK_UIDS,
                        data: data.reader(),
                        ..call
                    })
                    .unwrap()
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap();
                assert_eq!(power.battery().wake_nesting(10002, "adapter"), 0);
                assert_eq!(power.battery().wake_nesting(10004, "adapter"), 1);
                assert_eq!(
                    power.battery().wake_nesting(10005, "adapter"),
                    if chained { 2 } else { 1 }
                );
                power.remove(token.handle);
                assert_eq!(power.battery().wake_nesting(10004, "adapter"), 0);
            },
        );
    }
}
