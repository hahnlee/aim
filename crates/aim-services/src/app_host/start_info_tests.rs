//! AMS.reportStartInfoViewTimestamps / AppStartInfoTracker.addTimestampToStart,
//! android-16.0.0_r1. Binder's oneway sender pid is zero (binder.c).
use super::*;
use aim_binder_driver::uapi::TF_ONE_WAY;

fn start(host: &AppHost, seq: i64, package: &str, uid: u32, started: Instant) {
    let fixture = tests::host_with_start(seq, Arc::new(|| {}));
    let mut app = fixture.apps.lock().unwrap().remove(&seq).unwrap();
    app.package = package.into();
    app.uid = uid;
    app.package_uid = uid;
    app.isolated = false;
    app.pid = seq as i32;
    app.started = started;
    host.apps.lock().unwrap().insert(seq, app);
}

fn report(host: &Arc<AppHost>, uid: u32, draw: i64, presented: i64) {
    let mut data = Parcel::new();
    am::ReportStartInfoViewTimestamps {
        render_thread_draw_start_time_ns: draw,
        frame_presented_time_ns: presented,
    }
    .write(&mut data);
    Activity(host.clone())
        .transact(&mut Call {
            code: am::REPORT_START_INFO_VIEW_TIMESTAMPS,
            flags: TF_ONE_WAY,
            sender_pid: 0,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap();
}

#[test]
fn view_timestamps_from_oneway_uid_follow_a_cold_reopen() {
    crate::system::with_discovery_session(|host, _, _| {
        let now = Instant::now();
        start(host, 100, "p", 10100, now);
        report(host, 10100, 10, 20);
        assert_eq!(host.apps.lock().unwrap()[&100].first_frame, Some((10, 20)));
        report(host, 10100, 30, 40);
        assert_eq!(host.apps.lock().unwrap()[&100].first_frame, Some((10, 20)));

        host.process_died(100, 100);
        assert!(!host.apps.lock().unwrap().contains_key(&100));
        start(host, 101, "p", 10100, now + Duration::from_secs(1));
        // A late death notification for the predecessor cannot retire the reopen.
        host.process_died(100, 100);
        report(host, 10100, 50, 60);
        assert_eq!(host.apps.lock().unwrap()[&101].first_frame, Some((50, 60)));
    });
}

#[test]
fn view_timestamps_select_the_latest_package_and_uid_start() {
    crate::system::with_discovery_session(|host, _, _| {
        let now = Instant::now();
        for (seq, package, uid) in [
            (100, "p", 10100),
            (101, "p", 10100),
            (102, "shared.peer", 10100),
            (103, "p", 110100),
        ] {
            start(
                host,
                seq,
                package,
                uid,
                now + Duration::from_secs(seq as u64),
            );
        }
        report(host, 10100, 10, 20);
        // No start for q or an unknown uid: the original discards the report.
        report(host, 10101, 30, 40);
        report(host, 12345, 50, 60);
        let apps = host.apps.lock().unwrap();
        assert_eq!(apps[&101].first_frame, Some((10, 20)));
        for seq in [100, 102, 103] {
            assert_eq!(apps[&seq].first_frame, None);
        }
    });
}
