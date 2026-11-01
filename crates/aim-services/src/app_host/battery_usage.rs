//! Current-session battery usage: android-16.0.0_r1 query and consumer layout.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;

const COMPONENT_COUNT: usize = 19;
const BASE: usize = 18;
const WAKELOCK: usize = 12;
const CAMERA: usize = 3;
const PER_STATE: [usize; 12] = [1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 18];

#[derive(Debug, PartialEq)]
struct Query {
    start: i64,
    end: i64,
    flags: i32,
    users: Vec<i32>,
    max_age: i64,
    threshold: f64,
    from: i64,
    to: i64,
    components: Option<Vec<i32>>,
    history_duration: i64,
}

impl ReadParcelable for Query {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.read_i64()?;
        let end = r.read_i64()?;
        let flags = r.read_i32()?;
        let user_count = r.read_i32()?;
        let users = aim_service_aidl::read_int_array(r)?.ok_or(BAD_VALUE)?;
        if user_count < 0 || user_count as usize != users.len() {
            return Err(BAD_VALUE);
        }
        Ok(Self {
            start,
            end,
            flags,
            users,
            max_age: r.read_i64()?,
            threshold: f64::from_bits(r.read_i64()? as u64),
            from: r.read_i64()?,
            to: r.read_i64()?,
            components: aim_service_aidl::read_int_array(r)?,
            history_duration: r.read_i64()?,
        })
    }
}

impl Query {
    /// Settings DataProcessor.getBatteryUsageStats: no mode substitution.
    fn is_current_session_query(&self) -> bool {
        self.start == -1
            && self.end == -1
            && self.flags == 0x000a
            && self.users == [-1]
            && self.threshold == 0.0
            && self.from == 0
            && self.to == 0
            && self.components.is_none()
    }
}

/// Null means unproduced, as PowerComponents/BatteryConsumerData.hasValue
/// distinguishes it from a produced zero. This pin has no power-model columns.
struct Layout {
    columns: usize,
    durations: [[Option<usize>; 5]; COMPONENT_COUNT],
}

impl Layout {
    fn new(process_states: bool) -> Self {
        let mut layout = Self {
            columns: 4, // type, UID/scope, package/device total, total component power
            durations: [[None; 5]; COMPONENT_COUNT],
        };
        for component in 0..COMPONENT_COUNT {
            let count = if (process_states || component == BASE) && PER_STATE.contains(&component) {
                5
            } else {
                1
            };
            for state in 0..count {
                layout.durations[component][state] = Some(layout.columns + 1);
                layout.columns += 2;
            }
        }
        layout
    }

    fn row(&self, kind: i64, id: i32) -> Vec<Option<i64>> {
        let mut row = vec![None; self.columns];
        row[0] = Some(kind);
        row[1] = Some(i64::from(id));
        row[3] = Some(0); // PowerComponents.Builder.build's sum of unset powers.
        if kind == 0 {
            row[2] = Some(0); // AggregateBatteryConsumer's separate device total.
        }
        row
    }

    fn duration(&self, row: &mut [Option<i64>], component: usize, state: usize, ms: i64) {
        row[self.durations[component][state].unwrap()] = Some(ms);
    }
}

/// Frozen measured durations only. Sync has no BatteryConsumer component:
/// its presence creates a UID and its events are exported by the history owner.
fn rows(stats: &Stats, now: i64, up: i64) -> (Layout, Vec<Vec<Option<i64>>>) {
    let layout = Layout::new(true);
    let mut rows = vec![layout.row(0, 0), layout.row(0, 1)];
    let rt = stats.on_battery_time_base.realtime(now * 1000);
    let wake = stats.wakes.usage_durations(now);
    let uids: BTreeSet<i32> = stats.uids.keys().chain(wake.keys()).copied().collect();
    for uid in uids {
        if uid == 1090 {
            // Process.SDK_SANDBOX_VIRTUAL_UID, excluded by this query.
            continue;
        }
        let mut row = layout.row(1, uid);
        if let Some(u) = stats.uids.get(&uid) {
            let mut states = [0; 5];
            // BatteryStats.mapUidProcessStateToBatteryConsumerProcessState.
            for (source, target) in [(0, 1), (1, 3), (2, 1), (3, 2), (4, 2), (6, 4)] {
                states[target] += u.process_state_timers[source]
                    .as_ref()
                    .map_or(0, |t| t.run_time(rt));
            }
            for (state, us) in states.into_iter().enumerate().skip(1) {
                layout.duration(&mut row, BASE, state, us / 1000);
            }
            if let Some(camera) = &u.camera {
                layout.duration(
                    &mut row,
                    CAMERA,
                    0,
                    camera.run_time_in(rt, stats.camera_pool.len()) / 1000,
                );
            }
        }
        if let Some(ms) = wake.get(&uid) {
            layout.duration(&mut row, WAKELOCK, 0, *ms);
        }
        rows.push(row);
    }
    for (component, timer) in [
        (CAMERA, &stats.camera_timer),
        (6, &stats.flashlight_timer),
        (4, &stats.audio_on_timer),
        (5, &stats.video_on_timer),
    ] {
        layout.duration(&mut rows[0], component, 0, timer.run_time(rt) / 1000);
    }
    layout.duration(
        &mut rows[0],
        0,
        0,
        stats.screen_on.realtime(now * 1000) / 1000,
    );
    let wake_ms = wake.values().sum();
    layout.duration(&mut rows[1], WAKELOCK, 0, wake_ms);
    let awake = stats.on_battery_uptime.realtime(up * 1000);
    layout.duration(
        &mut rows[0],
        WAKELOCK,
        0,
        (awake - stats.screen_on.realtime(now * 1000)).max(0) / 1000,
    );
    let camera_ms = rows[2..]
        .iter()
        .map(|r| r[layout.durations[CAMERA][0].unwrap()].unwrap_or(0))
        .sum();
    layout.duration(&mut rows[1], CAMERA, 0, camera_ms);
    rows[0][2] = Some(stats.discharge_uah / 1000); // Numeric double via CursorWindow's coercion.
    (layout, rows)
}

impl BatteryStats {
    /// BatteryStatsService.getBatteryUsageStats_enforcePermission has no
    /// own-UID exemption (unlike the health snapshot API).
    fn enforce_usage_permission(&self, caller: i32) -> Result<(), Exception> {
        let host = self.host.upgrade().ok_or_else(|| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "no app host")
        })?;
        if host.check_permission(Some(BATTERY_STATS), caller)? != 0 {
            return Err(Exception::security(format!(
                "Neither user {caller} nor current process has {BATTERY_STATS}."
            )));
        }
        Ok(())
    }
}

struct Snapshot(Parcel);
impl aim_service_aidl::WriteParcelable for Snapshot {
    fn write_to(&self, out: &mut Parcel) {
        out.write_raw_files(self.0.data(), self.0.objects(), self.0.files());
    }
}

impl Stats {
    fn usage_snapshot(
        &mut self,
        now: i64,
        clock: i64,
        preferred_ms: i64,
    ) -> Result<Snapshot, Exception> {
        let (_, rows) = rows(self, now, uptime());
        let mut p = Parcel::new();
        p.write_i64(self.session_clock);
        p.write_i64(clock);
        p.write_i64(now - self.session_start);
        p.write_i64(0); // Capacity unavailable; no device power profile.
        let discharge = self.high_discharge_since_charge
            + if self.on_battery {
                (self.discharge_unplug_level - self.level).max(0)
            } else {
                0
            };
        p.write_i32(discharge);
        p.write_i64(0); // Capacity-based discharge bounds unavailable.
        p.write_i64(0);
        p.write_i64(self.on_battery_time_base.realtime(now * 1000) / 1000);
        let ms = |us: i64| if us >= 0 { us / 1000 } else { us };
        p.write_i64(ms(self.battery_time_remaining()));
        p.write_i64(ms(self.charge_time_remaining()));
        p.write_i32(0); // No custom energy consumers.
        p.write_bool(true);
        p.write_bool(false);
        p.write_bool(false);
        super::super::cursor::write_numbers(&mut p, &rows)?;
        let h = self.history.as_mut().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "battery history is not initialized (#1928)",
            )
        })?;
        p.write_bool(true);
        h.write_usage(&mut p, preferred_ms)
            .map_err(|e| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, e))?;
        // The pinned writer omits preferredHistoryDurationMs although its reader
        // reads it. Preserve its exact wire format (readLong at EOF returns 0).
        Ok(Snapshot(p))
    }
}

impl BatteryStats {
    pub(super) fn usage(&self, call: &mut Call<'_>) -> Reply {
        let args = bs::GetBatteryUsageStats::<Query>::read(&mut call.data)?;
        reply((|| {
            self.enforce_usage_permission(call.sender_euid as i32)?;
            let queries = args.queries.ok_or_else(|| {
                Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "queries is null")
            })?;
            let [Some(query)] = queries.as_slice() else {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                    "battery usage batch queries are not implemented (#1928)",
                ));
            };
            if !query.is_current_session_query() {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
                    "battery usage query mode is not implemented (#1928)",
                ));
            }
            let host = self
                .host
                .upgrade()
                .ok_or_else(|| Exception::illegal_argument("no app host"))?;
            let mut s = self.stats.lock().unwrap();
            let config = self.config(&host).ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "battery configuration is not initialized (#1928)",
                )
            })?;
            self.start_history(&host, &config, &mut s);
            let preferred = if config.continuous_history {
                query.history_duration
            } else {
                i64::MAX
            };
            let snapshot = s.usage_snapshot(elapsed_realtime(), wall_time(), preferred)?;
            let mut p = Parcel::new();
            bs::write_get_battery_usage_stats_reply(&mut p, Some(&[Some(snapshot)]));
            Ok(p)
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> Parcel {
        let mut p = Parcel::new();
        p.write_i64(-1);
        p.write_i64(-1);
        p.write_i32(0x000a);
        p.write_i32(1);
        aim_service_aidl::write_int_array(&mut p, Some(&[-1]));
        p.write_i64(300_000);
        p.write_i64(0); // threshold double
        p.write_i64(0);
        p.write_i64(0);
        aim_service_aidl::write_int_array(&mut p, None);
        p.write_i64(7_200_000);
        p
    }

    #[test]
    fn settings_query_uses_pinned_field_order_and_two_array_lengths() {
        let p = query();
        let mut r = p.reader();
        let mut q = Query::read_from(&mut r).unwrap();
        assert_eq!(r.position(), p.data().len());
        assert!(q.is_current_session_query());
        assert_eq!((q.max_age, q.history_duration), (300_000, 7_200_000));
        q.flags |= 0x80;
        assert!(!q.is_current_session_query());
        let mut bad = p.data().to_vec();
        bad[20..24].copy_from_slice(&2i32.to_le_bytes());
        assert_eq!(
            Query::read_from(&mut Reader::new(&bad, &[])).unwrap_err(),
            BAD_VALUE
        );
    }

    #[test]
    fn usage_permission_has_no_own_uid_exemption() {
        crate::system::with_discovery_session(|host, _, _| {
            super::super::contract_tests::permissions(host);
            let stats = BatteryStats::new(host);
            let mut request = Parcel::new();
            request.write_interface_token(bs::DESCRIPTOR);
            request.write_i32(1);
            request.write_i32(1);
            request.write_raw(query().data(), &[]);
            let denied = super::super::contract_tests::invoke(
                &stats,
                10100,
                bs::GET_BATTERY_USAGE_STATS,
                &request,
            )
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
            assert_eq!(denied.code, aim_binder_host::parcel::EX_SECURITY);
            assert!(denied.message.contains(BATTERY_STATS));
            stats.enforce_usage_permission(1000).unwrap();
        });
    }

    #[test]
    fn native_notes_produce_uid_rows_without_synthetic_cpu_or_sync_power() {
        let dir = super::super::super::battery_history::tests::temp_dir("usage-jvm");
        let mut history = super::super::super::battery_history::tests::history(&dir, 128, 0);
        history.set_initial_battery_state(true, 3, 50, 0);
        history.start_recording(0, true);
        for i in 1..20 {
            history.record_event(
                i * 10,
                EVENT_SYNC | EVENT_FLAG_START,
                Some("fragment"),
                10001,
            );
        }
        let mut stats = Stats {
            session_clock: 1234,
            history: Some(history),
            ..Stats::default()
        };
        stats.update_time_base(true, 0);
        stats.note_uid_process_state(10001, false, 2, 1000);
        stats.note_sync(true, Some("authority"), 10002, 1000, 200);
        stats.note_camera(true, 10001, 2000);
        stats.update_time_base(false, 4000);
        let (layout, rows) = rows(&stats, 10_000, 10_000);
        assert_eq!(layout.columns, 138);
        assert_eq!(rows.len(), 4);
        assert_eq!((rows[2][0], rows[2][1]), (Some(1), Some(10001)));
        assert_eq!(rows[2][layout.durations[BASE][1].unwrap()], Some(3000));
        assert_eq!(rows[2][layout.durations[CAMERA][0].unwrap()], Some(2000));
        assert!(rows[2][layout.durations[1][0].unwrap()].is_none()); // CPU unmeasured.
        for row in &rows {
            for column in layout.durations.iter().flatten().flatten() {
                assert!(row[column - 1].is_none()); // No power producer.
            }
        }
        assert_eq!(stats.uids.len(), 2); // Reads do not manufacture UIDs.
        let mut parcel = Parcel::new();
        crate::app_host::cursor::write_numbers(&mut parcel, &rows).unwrap();
        assert!(!parcel.data().is_empty());
        let snapshot = stats.usage_snapshot(10_000, 11234, i64::MAX).unwrap();
        jvm(&snapshot.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transaction_16_checks_permission_and_rejects_other_query_modes() {
        crate::system::with_discovery_session(|host, _, _| {
            super::super::contract_tests::permissions(host);
            let owner = BatteryStats::for_jobs_test(host);
            let dir = super::super::super::battery_history::tests::temp_dir("usage-endpoint");
            owner.set_history_for_test(super::super::super::battery_history::tests::history(
                &dir, 1024, 0,
            ));
            let mut p = Parcel::new();
            p.write_interface_token(bs::DESCRIPTOR);
            p.write_i32(1);
            p.write_i32(1);
            let start = p.position();
            p.write_raw(query().data(), &[]);
            assert_eq!(bs::GET_BATTERY_USAGE_STATS, 16);
            let result =
                super::super::contract_tests::invoke(&owner, 1000, bs::GET_BATTERY_USAGE_STATS, &p);
            let mut r = result.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (1, 1));
            p.set_i32_at(start + 16, 0);
            let result =
                super::super::contract_tests::invoke(&owner, 1000, bs::GET_BATTERY_USAGE_STATS, &p);
            assert_eq!(
                result.reader().read_exception().unwrap().unwrap_err().code,
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
            );
            std::fs::remove_dir_all(dir).unwrap();
        });
    }

    #[test]
    fn charged_reset_preserves_active_notes_and_excludes_prior_session_and_plugged_time() {
        let mut s = Stats::default();
        let state = |plug, status, level| BatteryState {
            plug_type: plug,
            status,
            level,
            ..BatteryState::default()
        };
        s.set_battery_state(&super::super::tests::C, state(0, 3, 50), 0);
        s.note_uid_process_state(10001, false, 2, 1000);
        s.note_sync(true, Some("authority"), 10001, 1000, 200);
        s.note_camera(true, 10001, 2000);
        s.set_battery_state(&super::super::tests::C, state(1, 5, 100), 4000);
        s.set_battery_state(&super::super::tests::C, state(0, 3, 100), 10_000);
        let (layout, rows) = rows(&s, 12_000, 12_000);
        assert_eq!(s.session_start, 10_000);
        assert_eq!(s.on_battery_time_base.realtime(12_000_000), 2_000_000);
        assert_eq!(rows[2][layout.durations[BASE][1].unwrap()], Some(2000));
        assert_eq!(rows[2][layout.durations[CAMERA][0].unwrap()], Some(2000));
        s.note_sync(false, Some("authority"), 10001, 13_000, 200);
        s.note_camera(false, 10001, 13_000);
        s.note_uid_process_state(10001, false, 20, 13_000);
        assert_eq!(
            s.uids[&10001].sync.timers["authority"]
                .timer
                .timer
                .total_time_us,
            3_000_000
        );
        assert_eq!(s.uids[&10001].camera.as_ref().unwrap().count, 0);
        assert_eq!(s.screen_on.realtime(14_000_000), 4_000_000);
    }

    #[test]
    fn large_numeric_window_preserves_fd_ownership_in_typed_reply() {
        use std::os::unix::fs::FileExt;
        let mut p = Parcel::new();
        let rows = vec![vec![Some(10123); 138]; 8];
        super::super::super::cursor::write_numbers(&mut p, &rows).unwrap();
        assert_eq!(p.files().len(), 1);
        let offset = p.objects()[0];
        let mut reply = Parcel::new();
        bs::write_get_battery_usage_stats_reply(&mut reply, Some(&[Some(Snapshot(p))]));
        assert_eq!(reply.objects(), &[offset + 12]);
        assert_eq!(reply.files()[0].0, offset + 12);
        let mut slot = [0; 16];
        aim_binder_host::server::file_fd(&reply.files()[0].1)
            .unwrap()
            .read_exact_at(&mut slot, 20_000_000 - 16)
            .unwrap();
        assert_eq!(i32::from_le_bytes(slot[..4].try_into().unwrap()), 1);
        assert_eq!(i64::from_le_bytes(slot[4..12].try_into().unwrap()), 10123);
    }

    #[test]
    fn discharge_samples_do_not_repeat_history_threshold_deltas() {
        let mut s = Stats::default();
        for charge in [1000, 995, 995, 990] {
            s.set_battery_state(
                &super::super::tests::C,
                BatteryState {
                    status: 3,
                    level: 50,
                    charge_uah: charge,
                    ..BatteryState::default()
                },
                0,
            );
        }
        assert_eq!(s.discharge_uah, 10);
    }

    #[test]
    fn continuous_history_survives_accounting_reset() {
        let dir = super::super::super::battery_history::tests::temp_dir("usage-continuous");
        let mut h = super::super::super::battery_history::tests::history(&dir, 1024, 0);
        h.set_initial_battery_state(true, 3, 50, 0);
        h.start_recording(0, true);
        let before = h.buffer().to_vec();
        let mut s = Stats {
            history: Some(h),
            ..Stats::default()
        };
        s.reset_session(1000, 1000, false);
        assert_eq!(s.history.as_ref().unwrap().buffer(), before);
        s.reset_session(2000, 2000, true);
        assert!(s.history.as_ref().unwrap().buffer().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn jvm(parcel: &Parcel) {
        use std::{fs, process::Command};
        let work = aim_paths::scratch_dir("battery-usage-java");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("snapshot"), parcel.data()).unwrap();
        let jdk =
            aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
        let out = Command::new("python3")
            .arg(
                aim_paths::root()
                    .join("crates/aim-services/tests/fixtures/battery_usage/oracle.py"),
            )
            .arg(aim_paths::root())
            .arg(&work)
            .arg(jdk)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(fs::read(work.join("roundtrip")).unwrap(), parcel.data());
        fs::remove_dir_all(work).unwrap();
    }
}

pub(super) fn wall_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl TimeBase {
    pub(super) fn reset_usage(&mut self, now: i64) {
        self.past_realtime_us = 0;
        self.realtime_start_us = now * 1000;
    }
}

impl StopwatchTimer {
    pub(super) fn reset_usage(&mut self) {
        *self = Self {
            nesting: self.nesting,
            acquire_time_us: -1,
            ..Self::default()
        };
    }
}

impl DualTimer {
    pub(super) fn reset_usage(&mut self, running: bool, background: bool) {
        for (t, on) in [
            (&mut self.timer, running),
            (&mut self.background, background),
        ] {
            let nesting = t.timer.nesting;
            *t = DurationTimer::default();
            t.timer.nesting = nesting;
            t.timer.acquire_time_us = -1;
            if nesting > 0 && on {
                t.start_time_ms = 0;
            }
        }
    }
}

impl Stats {
    /// resetAllStatsLocked: keep live nesting/pools and event membership, clear
    /// accumulated counters and rebase every timer onto the new session.
    pub(super) fn reset_session(&mut self, now: i64, uptime: i64, clear_history: bool) {
        self.session_start = now;
        self.session_clock = wall_time();
        self.discharge_uah = 0;
        self.on_battery_time_base.reset_usage(now);
        self.on_battery_uptime.reset_usage(uptime);
        self.screen_on.reset_usage(now);
        for t in [
            &mut self.camera_timer,
            &mut self.flashlight_timer,
            &mut self.audio_on_timer,
            &mut self.video_on_timer,
            &mut self.bluetooth_scan_timer,
        ] {
            t.reset_usage();
        }
        for u in self.uids.values_mut() {
            u.background.reset_usage(now);
            for t in u
                .process_state_timers
                .iter_mut()
                .flatten()
                .chain(u.camera.iter_mut())
            {
                t.reset_usage();
            }
            for t in u
                .sync
                .timers
                .values_mut()
                .chain(u.bluetooth_scan.iter_mut())
                .chain(u.bluetooth_unoptimized_scan.iter_mut())
            {
                t.reset_usage(self.on_battery, u.background.running);
            }
            for c in [
                &mut u.bluetooth_scan_results,
                &mut u.bluetooth_scan_results_background,
            ]
            .into_iter()
            .flatten()
            {
                c.count = 0;
            }
        }
        self.uids.retain(|_, u| {
            u.process_state != UID_PROCESS_STATE_NONEXISTENT
                || u.camera.as_ref().is_some_and(|t| t.nesting > 0)
                || u.sync
                    .timers
                    .values()
                    .chain(u.bluetooth_scan.iter())
                    .chain(u.bluetooth_unoptimized_scan.iter())
                    .any(|t| t.timer.timer.nesting > 0)
        });
        self.wakes.reset_usage(now);
        if clear_history && let Some(h) = &mut self.history {
            h.reset(now);
        }
    }
}
