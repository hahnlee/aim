//! BatteryStatsService / BatteryStatsImpl at android-16.0.0_r1; health assertions
//! follow CtsOsTestCases SystemHealthManagerTest (own uid, ordered UID arrays).
use super::*;
use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};
use std::sync::mpsc;

fn permissions(host: &Arc<AppHost>) {
    let manager = Arc::new(
        super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        ),
    );
    assert!(host.permission_manager.set(manager).is_ok());
}

fn invoke(stats: &BatteryStats, uid: u32, code: u32, data: &Parcel) -> Parcel {
    stats
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}

#[test]
fn camera_pool_and_reset_preserve_counts_and_stop_every_uid() {
    let mut s = Stats::default();
    s.update_time_base(true, 0);
    s.note_camera(true, 10001, 1000);
    s.note_camera(true, 10001, 1500);
    s.note_camera(true, 10002, 2000);
    s.note_camera(false, 10001, 3000); // nested, still pooled
    s.reset_camera(4000);
    let a = s.uids[&10001].camera.as_ref().unwrap();
    let b = s.uids[&10002].camera.as_ref().unwrap();
    assert_eq!((a.count, a.total_time_us, a.nesting), (1, 2_000_000, 0));
    assert_eq!((b.count, b.total_time_us, b.nesting), (1, 1_000_000, 0));
    assert_eq!(s.camera_timer.total_time_us, 3_000_000);
    assert!(s.camera_pool.is_empty());
    s.reset_camera(5000);
    s.note_camera(false, 10003, 6000);
    assert!(!s.uids.contains_key(&10003));
    s.note_camera(true, 10002, 7000);
    s.note_camera(false, 10002, 8000);
    assert_eq!(
        s.uids[&10002].camera.as_ref().unwrap().total_time_us,
        2_000_000
    );
}

#[test]
fn camera_and_flashlight_drop_plugged_time_and_reset_history_flags() {
    use super::super::battery_history::tests::{decode, history, temp_dir};
    let dir = temp_dir("camera-flashlight");
    let mut s = Stats::default();
    s.history = Some(history(&dir, 128 * 1024, 0));
    s.history
        .as_mut()
        .unwrap()
        .set_initial_battery_state(true, 3, 50, 0);
    s.history.as_mut().unwrap().start_recording(0, true);
    s.update_time_base(true, 0);
    s.note_camera(true, 10001, 1000);
    s.note_flashlight(true, 10001, 1000);
    s.note_flashlight(true, 10001, 2000);
    s.update_time_base(false, 4000);
    s.reset_camera(6000);
    s.reset_flashlight(6000);
    assert_eq!(s.camera_timer.total_time_us, 3_000_000);
    assert_eq!(
        s.uids[&10001].camera.as_ref().unwrap().total_time_us,
        3_000_000
    );
    assert_eq!(s.flashlight_timer.total_time_us, 3_000_000);
    let items = decode(s.history.as_ref().unwrap().buffer());
    assert_eq!(
        items.last().unwrap().states2 & (STATE2_CAMERA_FLAG | STATE2_FLASHLIGHT_FLAG),
        0
    );
    assert!(items.iter().any(|i| i.states2 & STATE2_CAMERA_FLAG != 0));
    assert!(
        items
            .iter()
            .any(|i| i.states2 & STATE2_FLASHLIGHT_FLAG != 0)
    );
    s.note_flashlight(false, 10001, 7000);
    assert_eq!(s.flashlight_nesting, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn health_counters_preserve_missing_uids_duplicates_and_timer_presence() {
    let mut s = Stats::default();
    s.update_time_base(true, 0);
    s.on_battery_uptime.set_running(true, 0);
    s.note_uid_process_state(10001, false, 2, 1000);
    s.note_camera(true, 10001, 1000);
    s.note_bluetooth_scan_started(10001, false, 2000);
    let data = s.health_counters(&[9999, 10001, 10001], 4000, 3000);
    assert_eq!(&data[..2], &[4_000_000, 3_000_000]);
    assert_eq!(&data[2..4], &[9999, 0]);
    assert_eq!(
        &data[24..32],
        &[10001, 1, 1, 3_000_000, -1, 0, 1, 2_000_000]
    );
    assert_eq!(&data[24..46], &data[46..68]);
    assert!(!s.uids.contains_key(&9999));
}

struct Writer(mpsc::Sender<(i32, Vec<i64>)>);
impl Service for Writer {
    fn descriptor(&self) -> &str {
        health::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        assert_eq!(call.code, health::WRITE_SNAPSHOT);
        let a = health::WriteSnapshot::read(&mut call.data).unwrap();
        self.0.send((a.format, a.counters.unwrap())).unwrap();
        let mut p = Parcel::new();
        health::write_write_snapshot_reply(&mut p, &Some(vec![1, 2, 3, 4]));
        Ok(p)
    }
}

struct Receiver(mpsc::Sender<Vec<u8>>);
impl Service for Receiver {
    fn descriptor(&self) -> &str {
        RESULT_RECEIVER_DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        call.data
            .enforce_interface(RESULT_RECEIVER_DESCRIPTOR)
            .unwrap();
        self.0
            .send({
                let start = call.data.position();
                call.data.skip(call.data.remaining()).unwrap();
                call.data.since(start).0.to_vec()
            })
            .unwrap();
        Ok(Parcel::new())
    }
}
impl aim_service_aidl::WriteParcelable for ResultReceiver {
    fn write_to(&self, p: &mut Parcel) {
        p.write_binder(self.0);
    }
}

#[test]
fn binder_health_reads_check_permissions_and_preserve_java_bytes_in_all_forms() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        permissions(host);
        let mut attach = Parcel::new();
        aim_service_aidl::dev_aim_server_isystemprocess::AttachSystemProcess {
            application_thread: None,
            foreground_service_policy: None,
            safe_labels: None,
            battery_health_stats: None,
        }
        .write(&mut attach);
        let denied = super::super::system_process::SystemProcess::new(host.clone())
            .transact(&mut Call {
                code: aim_service_aidl::dev_aim_server_isystemprocess::ATTACH_SYSTEM_PROCESS,
                flags: 0,
                sender_pid: 71,
                sender_euid: 10100,
                data: attach.reader(),
            })
            .unwrap();
        assert_eq!(
            denied.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        assert!(host.battery_stats.get().is_none());
        let stats = host
            .battery_stats
            .get_or_init(|| Arc::new(BatteryStats::new(host)));
        let (tx, calls) = mpsc::channel();
        let writer = export(Arc::new(Writer(tx)));
        stats
            .attach_health(Some(Binder::Handle(writer.handle)))
            .unwrap();
        let mut p = Parcel::new();
        bs::TakeUidSnapshot { uid: 10100 }.write(&mut p);
        let result = invoke(stats, 10100, bs::TAKE_UID_SNAPSHOT, &p);
        let mut r = result.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(&result.data()[r.position()..], &[1, 2, 3, 4]);
        assert_eq!(calls.recv().unwrap().0, HEALTH_SINGLE);
        let result = invoke(stats, 10101, bs::TAKE_UID_SNAPSHOT, &p);
        assert_eq!(
            result.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        assert!(calls.try_recv().is_err());
        let mut p = Parcel::new();
        bs::TakeUidSnapshots {
            uid: Some(vec![10101, 10100, 10101]),
        }
        .write(&mut p);
        invoke(stats, 1000, bs::TAKE_UID_SNAPSHOTS, &p)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let (format, data) = calls.recv().unwrap();
        assert_eq!(format, HEALTH_ARRAY);
        assert_eq!([data[2], data[24], data[46]], [10101, 10100, 10101]);
        let (tx, callbacks) = mpsc::channel();
        let receiver = export(Arc::new(Receiver(tx)));
        let mut p = Parcel::new();
        bs::TakeUidSnapshotsAsync {
            uid: Some(vec![10100]),
            result: Some(ResultReceiver(Some(Binder::Handle(receiver.handle)))),
        }
        .write(&mut p);
        invoke(stats, 10100, bs::TAKE_UID_SNAPSHOTS_ASYNC, &p);
        assert_eq!(
            callbacks.recv_timeout(Duration::from_secs(5)).unwrap(),
            [1, 2, 3, 4]
        );
        assert_eq!(calls.recv().unwrap().0, HEALTH_RESULT);
        invoke(stats, 10101, bs::TAKE_UID_SNAPSHOTS_ASYNC, &p);
        let denied = callbacks.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            i32::from_le_bytes(denied[..4].try_into().unwrap()),
            RESULT_SECURITY_EXCEPTION
        );
        assert!(calls.try_recv().is_err());
        // Empty arrays need no permission; null fails before a callback is scheduled.
        let mut p = Parcel::new();
        bs::TakeUidSnapshots { uid: Some(vec![]) }.write(&mut p);
        invoke(stats, 10101, bs::TAKE_UID_SNAPSHOTS, &p)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(calls.recv().unwrap().1.len(), 2);
        let mut p = Parcel::new();
        bs::TakeUidSnapshots { uid: None }.write(&mut p);
        assert_eq!(
            invoke(stats, 10101, bs::TAKE_UID_SNAPSHOTS, &p)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_NULL_POINTER
        );
    });
}

#[test]
fn binder_camera_and_flashlight_pairs_enforce_update_device_stats() {
    crate::system::with_discovery_session(|host, _, _| {
        permissions(host);
        let stats = BatteryStats::new(host);
        assert_eq!((bs::NOTE_RESET_CAMERA, bs::NOTE_RESET_FLASHLIGHT), (13, 14));
        for (code, data) in [
            (bs::NOTE_START_CAMERA, {
                let mut p = Parcel::new();
                bs::NoteStartCamera { uid: 10100 }.write(&mut p);
                p
            }),
            (bs::NOTE_STOP_CAMERA, {
                let mut p = Parcel::new();
                bs::NoteStopCamera { uid: 10100 }.write(&mut p);
                p
            }),
            (bs::NOTE_RESET_CAMERA, {
                let mut p = Parcel::new();
                bs::NoteResetCamera {}.write(&mut p);
                p
            }),
            (bs::NOTE_FLASHLIGHT_ON, {
                let mut p = Parcel::new();
                bs::NoteFlashlightOn { uid: 10100 }.write(&mut p);
                p
            }),
            (bs::NOTE_FLASHLIGHT_OFF, {
                let mut p = Parcel::new();
                bs::NoteFlashlightOff { uid: 10100 }.write(&mut p);
                p
            }),
            (bs::NOTE_RESET_FLASHLIGHT, {
                let mut p = Parcel::new();
                bs::NoteResetFlashlight {}.write(&mut p);
                p
            }),
        ] {
            assert_eq!(
                invoke(&stats, 10100, code, &data)
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
            invoke(&stats, 1000, code, &data)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap();
        }
        let mut p = Parcel::new();
        p.write_interface_token(bs::DESCRIPTOR);
        let e = invoke(&stats, 1000, bs::GET_BATTERY_USAGE_STATS, &p)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert_eq!(e.code, EX_UNSUPPORTED_OPERATION);
        assert!(e.message.contains("#1928"));
    });
}
