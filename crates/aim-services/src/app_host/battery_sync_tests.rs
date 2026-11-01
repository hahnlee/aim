//! android-16.0.0_r1 BatteryStatsImpl: Uid.noteStart/StopSyncLocked,
//! OverflowArrayMap, DualTimer, DurationTimer and StopwatchTimer.
use super::contract_tests::{invoke, permissions};
use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, EX_SECURITY};

fn sync<'a>(s: &'a Stats, uid: i32, name: &str) -> &'a DualTimer {
    &s.uids[&uid].sync.timers[name]
}

fn values(t: &DurationTimer) -> (i32, i64, i64, i64, i32) {
    (
        t.timer.count,
        t.timer.total_time_us,
        t.total_duration_ms,
        t.max_duration_ms,
        t.timer.nesting,
    )
}

#[test]
fn generated_sync_notes_enforce_permission_map_uids_and_preserve_errors() {
    crate::system::with_discovery_session(|host, _, _| {
        permissions(host);
        let mut stats = BatteryStats::new(host);
        let mut start = Parcel::new();
        bs::NoteSyncStart {
            name: Some("adapter/authority".into()),
            uid: 20100,
        }
        .write(&mut start);
        let mut finish = Parcel::new();
        bs::NoteSyncFinish {
            name: Some("adapter/authority".into()),
            uid: 20100,
        }
        .write(&mut finish);
        for (code, data) in [
            (bs::NOTE_SYNC_START, &start),
            (bs::NOTE_SYNC_FINISH, &finish),
        ] {
            let denied = invoke(&stats, 10100, code, data);
            let e = denied.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(e.code, EX_SECURITY);
            assert!(e.message.contains(UPDATE_DEVICE_STATS));
            assert!(stats.stats.lock().unwrap().uids.is_empty());
        }
        bs::read_note_sync_start_reply(
            &mut invoke(&stats, 1000, bs::NOTE_SYNC_START, &start).reader(),
        )
        .unwrap()
        .unwrap();
        {
            let s = stats.stats.lock().unwrap();
            assert_eq!(s.uids.len(), 1);
            assert_eq!(sync(&s, 10100, "adapter/authority").timer.timer.nesting, 1);
        }
        invoke(&stats, 10100, bs::NOTE_SYNC_FINISH, &finish)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            sync(&stats.stats.lock().unwrap(), 10100, "adapter/authority")
                .timer
                .timer
                .nesting,
            1
        );
        bs::read_note_sync_finish_reply(
            &mut invoke(&stats, 1000, bs::NOTE_SYNC_FINISH, &finish).reader(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            sync(&stats.stats.lock().unwrap(), 10100, "adapter/authority")
                .timer
                .timer
                .nesting,
            0
        );
        // A malformed generated note is a transport error, before any owner mutation.
        let empty = Parcel::new();
        for code in [bs::NOTE_SYNC_START, bs::NOTE_SYNC_FINISH] {
            assert_eq!(
                stats
                    .transact(&mut Call {
                        code,
                        flags: 0,
                        sender_pid: 71,
                        sender_euid: 1000,
                        data: empty.reader(),
                    })
                    .unwrap_err(),
                BAD_VALUE
            );
        }
        stats.host = Weak::new();
        let unavailable = invoke(&stats, 1000, bs::NOTE_SYNC_START, &start);
        assert_eq!(
            unavailable
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_STATE
        );
    });
}

#[test]
fn sync_nesting_names_and_uids_keep_independent_unpooled_time() {
    let mut s = Stats::default();
    s.update_time_base(true, 0);
    s.note_uid_process_state(10001, false, PROCESS_STATE_TOP, 0);
    s.note_sync(true, Some("a"), 10001, 1000, 200);
    s.note_sync(true, Some("a"), 10001, 2000, 200);
    s.note_sync(true, Some("b"), 10001, 3000, 200);
    s.note_sync(true, Some("a"), 10002, 4000, 200);
    s.note_sync(false, Some("a"), 10001, 5000, 200);
    let t = sync(&s, 10001, "a");
    assert_eq!(t.timer.timer.nesting, 1);
    assert_eq!(
        t.timer
            .timer
            .run_time(s.on_battery_time_base.realtime(6_000_000)),
        5_000_000
    );
    assert_eq!(
        t.timer.current_duration_ms(&s.on_battery_time_base, 6000),
        5000
    );
    s.note_sync(false, Some("a"), 10001, 7000, 200);
    s.note_sync(false, Some("b"), 10001, 8000, 200);
    s.note_sync(false, Some("a"), 10002, 9000, 200);
    assert_eq!(
        values(&sync(&s, 10001, "a").timer),
        (1, 6_000_000, 6000, 6000, 0)
    );
    assert_eq!(
        values(&sync(&s, 10001, "b").timer),
        (1, 5_000_000, 5000, 5000, 0)
    );
    assert_eq!(
        values(&sync(&s, 10002, "a").timer),
        (1, 5_000_000, 5000, 5000, 0)
    );
    s.note_sync(false, Some("a"), 10001, 10000, 200); // duplicate finish
    s.note_sync(false, Some("missing"), 10001, 10000, 200);
    s.note_sync(false, Some("missing"), 10003, 10000, 200);
    assert!(s.uids[&10003].sync.timers.is_empty());
    s.note_sync(true, Some("a"), 10001, 10000, 200);
    s.note_sync(false, Some("a"), 10001, 12000, 200);
    assert_eq!(
        values(&sync(&s, 10001, "a").timer),
        (2, 8_000_000, 8000, 6000, 0)
    );
    assert_eq!(values(&sync(&s, 10001, "a").background), (0, 0, 0, 0, 0));
    s.note_sync(true, None, 10001, 13000, 200);
    s.note_sync(true, Some(""), 10001, 14000, 200);
    s.note_sync(false, None, 10001, 15000, 200);
    s.note_sync(false, Some(""), 10001, 16000, 200);
    assert_eq!(
        values(&sync(&s, 10001, "").timer),
        (1, 3_000_000, 3000, 3000, 0)
    );
    s.note_sync(true, Some("zero"), 10001, 17000, 200);
    s.note_sync(false, Some("zero"), 10001, 17000, 200);
    assert_eq!(values(&sync(&s, 10001, "zero").timer), (0, 0, 0, 0, 0));
}

#[test]
fn sync_battery_and_background_bases_pause_and_resume_running_timers() {
    let mut s = Stats::default();
    let battery = |status, plug_type, level, time_to_full| BatteryState {
        status,
        plug_type,
        level,
        time_to_full,
        ..BatteryState::default()
    };
    let c = &Config {
        charged_delay_ms: DEFAULT_BATTERY_CHARGED_DELAY_MS,
        charging_enforce_level: DEFAULT_BATTERY_CHARGING_ENFORCE_LEVEL,
        reset_high_level: false,
        reset_significant_charge: false,
        max_history_buffer: 128 * 1024,
        max_history_size: 4 * 1024 * 1024,
    };
    s.set_battery_state(c, battery(2, 1, 50, -1), 0);
    s.note_uid_process_state(10001, false, PROCESS_STATE_RECEIVER, 0);
    s.note_sync(true, Some("a"), 10001, 1000, 200); // starts plugged: no count
    s.set_battery_state(c, battery(3, PLUGGED_NONE, 50, -1), 2000);
    s.note_uid_process_state(10001, false, PROCESS_STATE_TOP, 4000);
    s.set_battery_state(c, battery(2, 1, 50, -1), 5000);
    s.note_uid_process_state(10001, false, PROCESS_STATE_RECEIVER, 6000);
    s.set_battery_state(c, battery(3, PLUGGED_NONE, 50, -1), 8000);
    s.note_sync(false, Some("a"), 10001, 11000, 200);
    let t = sync(&s, 10001, "a");
    assert_eq!(values(&t.timer), (0, 6_000_000, 6000, 6000, 0));
    assert_eq!(values(&t.background), (0, 5_000_000, 5000, 5000, 0));
    s.note_sync(true, Some("a"), 10001, 12000, 200);
    s.set_battery_state(c, battery(2, 1, 50, -1), 14000);
    s.note_sync(false, Some("a"), 10001, 19000, 200);
    let t = sync(&s, 10001, "a");
    assert_eq!(values(&t.timer), (1, 8_000_000, 8000, 6000, 0));
    assert_eq!(values(&t.background), (1, 7_000_000, 7000, 5000, 0));
    s.note_sync(true, Some("plugged"), 10001, 20000, 200);
    s.note_sync(false, Some("plugged"), 10001, 21000, 200);
    assert_eq!(values(&sync(&s, 10001, "plugged").timer), (0, 0, 0, 0, 0));
}

#[test]
fn sync_background_base_starts_only_on_the_original_uid_transitions() {
    let mut s = Stats::default();
    s.update_time_base(true, 0);
    s.note_sync(true, Some("new uid"), 10001, 1000, 200);
    // A newly created uid has no running background base until a state or battery change.
    s.note_uid_process_state(10001, false, PROCESS_STATE_NONEXISTENT, 2000);
    s.note_sync(false, Some("new uid"), 10001, 3000, 200);
    assert_eq!(
        values(&sync(&s, 10001, "new uid").background),
        (0, 0, 0, 0, 0)
    );
    s.note_uid_process_state(10001, false, PROCESS_STATE_TOP, 4000);
    s.note_sync(true, Some("active"), 10001, 5000, 200);
    s.note_uid_process_state(10001, false, PROCESS_STATE_RECEIVER, 6000);
    s.note_uid_process_state(10001, false, PROCESS_STATE_TOP, 8000);
    s.note_uid_process_state(10001, false, PROCESS_STATE_NONEXISTENT, 10000);
    s.note_sync(false, Some("active"), 10001, 13000, 200);
    let t = sync(&s, 10001, "active");
    assert_eq!(values(&t.timer), (1, 8_000_000, 8000, 8000, 0));
    assert_eq!(values(&t.background), (0, 5_000_000, 5000, 5000, 0));
}

#[test]
fn sync_overflow_keeps_per_name_nesting_and_reuses_the_uid_aggregate() {
    for limit in [40, 200] {
        let mut s = Stats::default();
        s.note_uid_process_state(10001, false, PROCESS_STATE_RECEIVER, 0);
        s.update_time_base(true, 0);
        for n in 0..limit {
            let name = n.to_string();
            s.note_sync(true, Some(&name), 10001, 1000, limit);
            s.note_sync(false, Some(&name), 10001, 2000, limit);
        }
        assert_eq!(s.uids[&10001].sync.timers.len(), limit);
        s.note_sync(true, Some("extra-a"), 10001, 3000, limit);
        s.note_sync(true, Some("extra-a"), 10001, 4000, limit);
        s.note_sync(true, Some("extra-b"), 10001, 5000, limit);
        s.note_sync(true, Some("extra-a"), 10002, 5000, limit);
        assert_eq!(s.uids[&10001].sync.timers.len(), limit + 1);
        s.note_sync(false, Some("extra-a"), 10001, 6000, limit);
        s.note_sync(false, Some("extra-b"), 10001, 7000, limit);
        s.note_sync(false, Some("extra-b"), 10001, 8000, limit); // cannot stop extra-a
        assert_eq!(s.uids[&10001].sync.active_overflow["extra-a"], 1);
        assert_eq!(sync(&s, 10001, OVERFLOW_NAME).timer.timer.nesting, 1);
        s.update_time_base(false, 9000);
        s.note_sync(false, Some("extra-a"), 10001, 10000, limit);
        assert!(s.uids[&10001].sync.active_overflow.is_empty());
        let t = sync(&s, 10001, OVERFLOW_NAME);
        assert_eq!(values(&t.timer), (1, 6_000_000, 6000, 6000, 0));
        assert_eq!(values(&t.background), (1, 6_000_000, 6000, 6000, 0));
        s.update_time_base(true, 11000);
        s.note_sync(true, Some("extra-c"), 10001, 12000, limit);
        s.note_sync(false, Some("extra-c"), 10001, 14000, limit);
        assert_eq!(
            values(&sync(&s, 10001, OVERFLOW_NAME).timer),
            (2, 8_000_000, 8000, 6000, 0)
        );
        assert!(!s.uids[&10002].sync.timers.contains_key(OVERFLOW_NAME));
        assert_eq!(sync(&s, 10002, "extra-a").timer.timer.nesting, 1);
        assert_eq!(
            values(&sync(&s, 10001, "0").timer),
            (1, 1_000_000, 1000, 1000, 0)
        );
    }
}

#[test]
fn sync_literal_overflow_name_follows_the_original_map_lookup_order() {
    let mut map = SyncTimers::default();
    map.timer(Some(OVERFLOW_NAME), true, 40)
        .unwrap()
        .timer
        .timer
        .count = 7;
    for n in 1..40 {
        map.timer(Some(&n.to_string()), true, 40).unwrap();
    }
    // mCurOverflow is allocated separately and replaces an earlier literal entry.
    assert_eq!(
        map.timer(Some("extra"), true, 40)
            .unwrap()
            .timer
            .timer
            .count,
        0
    );
    assert_eq!(map.timers.len(), 40);
    map.timer(Some(OVERFLOW_NAME), true, 40).unwrap();
    assert_eq!(map.active_overflow.len(), 1);
    map.timer(Some("extra"), false, 40).unwrap();
    assert!(map.active_overflow.is_empty());
    assert!(map.timer(Some("extra"), false, 40).is_none());
}

use super::super::battery_history::tests::{events, history, temp_dir};

fn recording(dir: &std::path::Path) -> Stats {
    let mut s = Stats::default();
    let mut h = history(dir, 128 * 1024, 0);
    h.set_initial_battery_state(false, 3, 50, 0);
    h.start_recording(0, true);
    s.history = Some(h);
    s.update_time_base(true, 0);
    s
}

#[test]
fn sync_history_tracks_presence_separately_from_nesting_overflow_and_battery_time() {
    let dir = temp_dir("sync-events");
    let mut s = recording(&dir);
    s.note_sync(false, Some("unknown"), 10001, 100, 1);
    s.note_sync(true, Some("authority/account"), 10001, 1000, 1);
    s.note_sync(true, Some("authority/account"), 10001, 1100, 1);
    s.note_sync(true, Some("overflowed"), 10001, 1200, 1);
    s.note_sync(true, Some("authority/account"), 10002, 1300, 1);
    s.update_time_base(false, 1400);
    s.note_sync(false, Some("authority/account"), 10001, 1500, 1);
    assert_eq!(sync(&s, 10001, "authority/account").timer.timer.nesting, 1);
    s.note_sync(false, Some("authority/account"), 10001, 1600, 1);
    s.note_sync(false, Some("overflowed"), 10001, 1700, 1);
    s.note_sync(false, Some("authority/account"), 10002, 1800, 1);
    // A new start is a new history interval even while the battery base is stopped.
    s.note_sync(true, Some("authority/account"), 10001, 1900, 1);
    s.note_sync(false, Some("authority/account"), 10001, 2000, 1);
    let expected = vec![
        (0x8004, "authority/account".into(), 10001),
        (0x8004, "overflowed".into(), 10001),
        (0x8004, "authority/account".into(), 10002),
        (0x4004, "authority/account".into(), 10001),
        (0x4004, "overflowed".into(), 10001),
        (0x4004, "authority/account".into(), 10002),
        (0x8004, "authority/account".into(), 10001),
        (0x4004, "authority/account".into(), 10001),
    ];
    let h = s.history.as_mut().unwrap();
    assert_eq!(events(h.buffer()), expected);
    h.write_history(2100, false);
    let mut reopened = history(&dir, 128 * 1024, 2200);
    assert!(reopened.read_summary(2200));
    assert_eq!(events(reopened.buffer()), expected);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn history_distinguishes_null_and_empty_names_before_tag_normalization() {
    let dir = temp_dir("sync-null-events");
    let mut s = recording(&dir);
    for (i, (start, name)) in [
        (true, None),
        (true, Some("")),
        (true, None),
        (false, None),
        (false, None),
        (false, Some("")),
    ]
    .into_iter()
    .enumerate()
    {
        s.note_sync(start, name, 10001, 1000 + i as i64, 200);
    }
    assert_eq!(
        events(s.history.as_ref().unwrap().buffer()),
        [
            (0x8004, "".into(), 10001),
            (0x8004, "".into(), 10001),
            (0x4004, "".into(), 10001),
            (0x4004, "".into(), 10001),
        ]
    );
    assert!(s.active_events.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn generated_sync_history_maps_uids_and_failed_notes_leave_no_events() {
    crate::system::with_discovery_session(|host, _, _| {
        permissions(host);
        let dir = temp_dir("sync-binder-history");
        let mut owner = BatteryStats::new(host);
        *owner.stats.lock().unwrap() = recording(&dir);
        let mut start = Parcel::new();
        bs::NoteSyncStart {
            name: Some("adapter".into()),
            uid: 20100,
        }
        .write(&mut start);
        let mut stop = Parcel::new();
        bs::NoteSyncFinish {
            name: Some("adapter".into()),
            uid: 20100,
        }
        .write(&mut stop);
        invoke(&owner, 10100, bs::NOTE_SYNC_START, &start)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert!(events(&owner.history_buffer()).is_empty());
        invoke(&owner, 1000, bs::NOTE_SYNC_START, &start)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        invoke(&owner, 10100, bs::NOTE_SYNC_FINISH, &stop)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            events(&owner.history_buffer()),
            [(0x8004, "adapter".into(), 10100)]
        );
        invoke(&owner, 1000, bs::NOTE_SYNC_FINISH, &stop)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(
            events(&owner.history_buffer()),
            [
                (0x8004, "adapter".into(), 10100),
                (0x4004, "adapter".into(), 10100),
            ]
        );
        owner.host = Weak::new();
        invoke(&owner, 1000, bs::NOTE_SYNC_START, &start)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert_eq!(events(&owner.history_buffer()).len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn temporary_history_owner_failure_precedes_membership_mutation() {
    crate::system::with_discovery_session(|host, _, _| {
        permissions(host);
        let dir = temp_dir("temp-history-failure");
        let mut owner = BatteryStats::new(host);
        *owner.stats.lock().unwrap() = recording(&dir);
        let mut changed = false;
        let result = owner.temp_allowlist_history(
            || Err(Exception::illegal_argument("allowance preparation failed")),
            |record| {
                changed = true;
                record(true, Some("must not appear"), 12345);
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().message, "allowance preparation failed");
        assert!(!changed);
        assert!(events(&owner.history_buffer()).is_empty());
        owner.host = Weak::new();
        let mut prepared = false;
        let result = owner.temp_allowlist_history(
            || {
                prepared = true;
                Ok(())
            },
            |record| {
                changed = true;
                record(true, Some("must not appear"), 12345);
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().code, EX_ILLEGAL_STATE);
        assert!(!prepared);
        assert!(!changed);
        assert!(events(&owner.history_buffer()).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    });
}
