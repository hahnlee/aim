//! DeviceIdleController's temp lifetime and PowerAllowlistInternal delivery, Android 16 r1.
use super::*;

fn fixture(test: impl FnOnce(Arc<DeviceIdle>, mpsc::Receiver<Work>)) {
    crate::system::with_discovery_binders(|host, _, _, _| {
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
