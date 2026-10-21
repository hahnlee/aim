//! `batterystats`: BatteryStatsService's battery reads for apps, from
//! `BatteryStatsImpl` at `android-16.0.0_r1` as far as they go: the
//! charging state (`setBatteryStateLocked`, `setOnBatteryLocked`,
//! `setChargingLocked`: charging at once from
//! `BATTERY_CHARGING_ENFORCE_LEVEL` while plugged, after
//! `BATTERY_CHARGED_DELAY_MS` of a rising level below it, not once the
//! level falls or the device is unplugged) with its `ACTION_CHARGING` and
//! `ACTION_DISCHARGING`, and the charge and discharge step trackers
//! (`LevelStepTracker`) `computeChargeTimeRemaining` and
//! `computeBatteryTimeRemaining` read, fed by the battery service as
//! BatteryService feeds them. The battery history (`BatteryStatsHistory`,
//! `super::battery_history`) records the battery state and the audio and
//! video notes, which, with the image's `streamlined_misc_battery_stats`
//! on, are attributed from it rather than from per-uid timers; their
//! device timers (`mAudioOnTimer`, `mVideoOnTimer`) run on the on-battery
//! time base. The Bluetooth scan notes (`noteBleScanStarted`, `Stopped`,
//! `Reset`, `Results`) record the history and the device timer
//! (`mBluetoothScanTimer`), and the uids' stats the original keeps however
//! the collectors are: each uid's scan timers (`DualTimer`s, the scan's
//! battery time shared among the uids scanning at once, their background
//! time on the uid's background time base) and result counters. A uid's
//! process state (`noteUidProcessStateLocked`, from the uid's first
//! process to its last) runs its process state timers and background time
//! base, and is recorded in the history. `takeUidSnapshotsAsync` answers
//! its result receiver as the original does (its BATTERY_STATS check), the
//! snapshots themselves failing as not implemented. The other notes, the
//! readers, the counters a process state change switches and
//! `batterystats.bin`: #1928.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::com_android_internal_app_ibatterystats as bs;

use super::battery_history::{
    Directory, History, MonotonicClock, STATE_AUDIO_ON_FLAG, STATE2_BLUETOOTH_SCAN_FLAG,
    STATE2_VIDEO_ON_FLAG,
};
use super::kept::RawIntent;
use super::{AppHost, reply};
use crate::location::parcels::WorkSource;

/// `BatteryManager.BATTERY_STATUS_UNKNOWN`, `BATTERY_STATUS_FULL`,
/// `BATTERY_PLUGGED_NONE`.
const UPDATE_DEVICE_STATS: &str = "android.permission.UPDATE_DEVICE_STATS";
const BATTERY_STATS: &str = "android.permission.BATTERY_STATS";
/// `IBatteryStats.RESULT_RUNTIME_EXCEPTION`, `RESULT_SECURITY_EXCEPTION`,
/// `KEY_EXCEPTION_MESSAGE`; `IResultReceiver.send`.
const RESULT_RUNTIME_EXCEPTION: i32 = 1;
const RESULT_SECURITY_EXCEPTION: i32 = 2;
const KEY_EXCEPTION_MESSAGE: &str = "exception";
const RESULT_RECEIVER_DESCRIPTOR: &str = "com.android.internal.os.IResultReceiver";
const RESULT_RECEIVER_SEND: u32 = 1;

/// A `ResultReceiver`: its `IResultReceiver`.
struct ResultReceiver(Option<Binder>);

impl ReadParcelable for ResultReceiver {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(ResultReceiver(r.read_binder()?))
    }
}

/// `ResultReceiver.send(code, Bundle.forPair(KEY_EXCEPTION_MESSAGE,
/// message))`, one-way.
fn send_failure(host: &AppHost, receiver: Option<Binder>, code: i32, message: &str) {
    let Some(Binder::Handle(handle)) = receiver else {
        return;
    };
    let _ =
        host.process
            .strong(handle)
            .transact(RESULT_RECEIVER_SEND, &failure(code, message), true);
}

/// `IResultReceiver.send`'s data of a failure.
fn failure(code: i32, message: &str) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(RESULT_RECEIVER_DESCRIPTOR);
    p.write_i32(code);
    p.write_i32(1);
    crate::bundle::write(
        &mut p,
        &[(
            KEY_EXCEPTION_MESSAGE,
            crate::bundle::Value::String(Some(message.into())),
        )],
    );
    p
}
const STATUS_UNKNOWN: i32 = 1;
const STATUS_FULL: i32 = 5;
const PLUGGED_NONE: i32 = 0;
/// `BatteryStatsImpl.MAX_LEVEL_STEPS`; `BatteryStats.STEP_LEVEL_*`.
const MAX_LEVEL_STEPS: usize = 200;
const STEP_LEVEL_TIME_MASK: i64 = 0x0000_00ff_ffff_ffff;
const STEP_LEVEL_LEVEL_SHIFT: i64 = 40;
const STEP_LEVEL_INITIAL_MODE_SHIFT: i64 = 48;
const STEP_LEVEL_MODIFIED_MODE_SHIFT: i64 = 56;
/// The step mode of an on screen with no power save or idle
/// (`Display.STATE_ON - 1`): the app host's display is always on.
const STEP_MODE_SCREEN_ON: i64 = 1;
/// `BatteryStatsImpl.Constants`' defaults.
const DEFAULT_BATTERY_CHARGED_DELAY_MS: i64 = 900_000;
const DEFAULT_BATTERY_CHARGING_ENFORCE_LEVEL: i32 = 90;
/// `BatteryManager.ACTION_CHARGING`, `ACTION_DISCHARGING`;
/// `Intent.FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT`.
const ACTION_CHARGING: &str = "android.os.action.CHARGING";
const ACTION_DISCHARGING: &str = "android.os.action.DISCHARGING";
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
/// `Constants`' history buffer (`max_history_buffer_kb`).
const DEFAULT_MAX_HISTORY_BUFFER_KB: i64 = 128;
const DEFAULT_MAX_HISTORY_BUFFER_LOW_RAM_DEVICE_KB: i64 = 64;
/// `BatteryStatsConfig.Builder.DEFAULT_MAX_HISTORY_SIZE`.
const DEFAULT_MAX_HISTORY_SIZE: i64 = 4 * 1024 * 1024;
/// `setOnBatteryLocked` writes when a minute passed since the last write.
const WRITE_INTERVAL_MS: i64 = 60_000;
/// `Flags.extendedBatteryHistoryCompressionEnabled`.
const HISTORY_COMPRESSION_FLAG: &str =
    "com.android.server.power.optimization.extended_battery_history_compression_enabled";
/// `Process.FIRST_SDK_SANDBOX_UID`, `LAST_SDK_SANDBOX_UID`, and the
/// offset of a sandbox from its app.
const FIRST_SDK_SANDBOX_UID: i32 = 20_000;
const LAST_SDK_SANDBOX_UID: i32 = 29_999;
const SDK_SANDBOX_OFFSET: i32 = 10_000;
const PER_USER_RANGE: i32 = 100_000;

/// `BatteryStatsImpl.TimeBase`'s realtime, which the timers on it read.
#[derive(Debug, Default)]
struct TimeBase {
    running: bool,
    past_realtime_us: i64,
    realtime_start_us: i64,
}

impl TimeBase {
    /// `getRealtime`.
    fn realtime(&self, now_us: i64) -> i64 {
        self.past_realtime_us
            + if self.running {
                now_us - self.realtime_start_us
            } else {
                0
            }
    }

    /// `setRunning`: if it changed, the base's realtime its observers are
    /// told it started or stopped at.
    fn set_running(&mut self, running: bool, now_us: i64) -> Option<i64> {
        if self.running == running {
            return None;
        }
        self.running = running;
        if running {
            self.realtime_start_us = now_us;
        } else {
            self.past_realtime_us += now_us - self.realtime_start_us;
        }
        Some(self.realtime(now_us))
    }
}

/// A `StopwatchTimer` of the device, in no timer pool.
#[derive(Debug, Default)]
struct StopwatchTimer {
    count: i32,
    total_time_us: i64,
    nesting: i32,
    update_time_us: i64,
    acquire_time_us: i64,
}

impl StopwatchTimer {
    /// `computeRunTimeLocked`, its time shared among the `pool` running
    /// timers of its pool.
    fn run_time_in(&self, battery_realtime_us: i64, pool: usize) -> i64 {
        self.total_time_us
            + if self.nesting > 0 {
                (battery_realtime_us - self.update_time_us) / pool.max(1) as i64
            } else {
                0
            }
    }

    fn run_time(&self, battery_realtime_us: i64) -> i64 {
        self.run_time_in(battery_realtime_us, 0)
    }

    /// `startRunningLocked` up to its pool: whether it is the first start.
    fn begin_start(&mut self, battery_realtime_us: i64) -> bool {
        self.nesting += 1;
        if self.nesting == 1 {
            self.update_time_us = battery_realtime_us;
        }
        self.nesting == 1
    }

    /// `startRunningLocked` after its pool, of a first start.
    fn finish_start(&mut self, base: &TimeBase) {
        if base.running {
            self.count += 1;
            self.acquire_time_us = self.total_time_us;
        } else {
            self.acquire_time_us = -1;
        }
    }

    /// `startRunningLocked`, in no pool.
    fn start(&mut self, base: &TimeBase, now: i64) {
        if self.begin_start(base.realtime(now * 1000)) {
            self.finish_start(base);
        }
    }

    /// `stopRunningLocked` up to its pool: whether it is the last stop.
    fn begin_stop(&mut self) -> bool {
        if self.nesting == 0 {
            return false;
        }
        self.nesting -= 1;
        self.nesting == 0
    }

    /// `stopRunningLocked` after its pool, of a last stop: a start that
    /// added no time is not counted.
    fn finish_stop(&mut self) {
        if self.acquire_time_us >= 0 && self.total_time_us == self.acquire_time_us {
            self.count -= 1;
        }
    }

    /// `stopRunningLocked`, in no pool.
    fn stop(&mut self, base: &TimeBase, now: i64) {
        if self.begin_stop() {
            self.nesting = 1;
            self.total_time_us = self.run_time(base.realtime(now * 1000));
            self.nesting = 0;
            self.finish_stop();
        }
    }

    /// `stopAllRunningLocked`.
    fn stop_all(&mut self, base: &TimeBase, now: i64) {
        if self.nesting > 0 {
            self.nesting = 1;
            self.stop(base, now);
        }
    }

    fn start_or_stop(&mut self, start: bool, base: &TimeBase, now: i64) {
        if start {
            self.start(base, now);
        } else {
            self.stop(base, now);
        }
    }

    /// `onTimeStopped`: the time it ran while the base did.
    fn time_stopped(&mut self, battery_realtime_us: i64) {
        self.time_stopped_in(battery_realtime_us, 0);
    }

    fn time_stopped_in(&mut self, battery_realtime_us: i64, pool: usize) {
        if self.nesting > 0 {
            self.total_time_us = self.run_time_in(battery_realtime_us, pool);
            self.update_time_us = battery_realtime_us;
        }
    }
}

/// `refreshTimersLocked` of the Bluetooth scan pool (the uids whose scan
/// timer runs, in the order they started): the time held since each was
/// last updated, shared among them all.
fn refresh_scan_pool(battery_realtime_us: i64, pool: &[i32], uids: &mut BTreeMap<i32, Uid>) {
    let n = pool.len() as i64;
    for uid in pool.iter().rev() {
        let Some(t) = uids.get_mut(uid).and_then(|u| u.bluetooth_scan.as_mut()) else {
            continue;
        };
        let t = &mut t.timer.timer;
        let held = battery_realtime_us - t.update_time_us;
        if held > 0 {
            t.total_time_us += held / n;
        }
        t.update_time_us = battery_realtime_us;
    }
}

/// A `DurationTimer`: a `StopwatchTimer` that also keeps its longest and
/// total spans, which a pool does not share.
#[derive(Debug)]
struct DurationTimer {
    timer: StopwatchTimer,
    start_time_ms: i64,
    max_duration_ms: i64,
    current_duration_ms: i64,
    total_duration_ms: i64,
}

impl Default for DurationTimer {
    fn default() -> Self {
        DurationTimer {
            timer: StopwatchTimer::default(),
            start_time_ms: -1,
            max_duration_ms: 0,
            current_duration_ms: 0,
            total_duration_ms: 0,
        }
    }
}

impl DurationTimer {
    /// `startRunningLocked` after the stopwatch's.
    fn started(&mut self, base: &TimeBase, now: i64) {
        if self.timer.nesting == 1 && base.running {
            self.start_time_ms = base.realtime(now * 1000) / 1000;
        }
    }

    /// `stopRunningLocked` before the stopwatch's.
    fn stopping(&mut self, base: &TimeBase, now: i64) {
        if self.timer.nesting == 1 {
            let duration = self.current_duration_ms(base, now);
            self.total_duration_ms += duration;
            self.max_duration_ms = self.max_duration_ms.max(duration);
            self.start_time_ms = -1;
            self.current_duration_ms = 0;
        }
    }

    /// `getCurrentDurationMsLocked`.
    fn current_duration_ms(&self, base: &TimeBase, now: i64) -> i64 {
        self.current_duration_ms
            + if self.timer.nesting > 0 && base.running {
                base.realtime(now * 1000) / 1000 - self.start_time_ms
            } else {
                0
            }
    }

    /// `startRunningLocked`, in no pool.
    fn start(&mut self, base: &TimeBase, now: i64) {
        self.timer.start(base, now);
        self.started(base, now);
    }

    /// `stopRunningLocked`, in no pool.
    fn stop(&mut self, base: &TimeBase, now: i64) {
        self.stopping(base, now);
        self.timer.stop(base, now);
    }

    /// `stopAllRunningLocked`, in no pool.
    fn stop_all(&mut self, base: &TimeBase, now: i64) {
        if self.timer.nesting > 0 {
            self.timer.nesting = 1;
            self.stop(base, now);
        }
    }

    /// `onTimeStarted`.
    fn time_started(&mut self, battery_realtime_us: i64) {
        if self.timer.nesting > 0 {
            self.start_time_ms = battery_realtime_us / 1000;
        }
    }

    /// `onTimeStopped`, its stopwatch's time shared among `pool` timers.
    fn time_stopped_in(&mut self, battery_realtime_us: i64, pool: usize) {
        self.timer.time_stopped_in(battery_realtime_us, pool);
        if self.timer.nesting > 0 {
            self.current_duration_ms += battery_realtime_us / 1000 - self.start_time_ms;
        }
        self.start_time_ms = -1;
    }
}

/// A `DualTimer`: its time on the battery (`timer`) and in the
/// background (`background`, in no pool).
#[derive(Debug, Default)]
struct DualTimer {
    timer: DurationTimer,
    background: DurationTimer,
}

/// A `Counter`: what is added while its time base runs.
#[derive(Debug, Default)]
struct Counter {
    count: i32,
}

impl Counter {
    /// `addAtomic`.
    fn add(&mut self, base: &TimeBase, delta: i32) {
        if base.running {
            self.count += delta;
        }
    }
}

/// `BatteryStats.Uid.PROCESS_STATE_*`, `NUM_PROCESS_STATE`.
const UID_PROCESS_STATE_TOP: usize = 0;
const UID_PROCESS_STATE_FOREGROUND_SERVICE: usize = 1;
const UID_PROCESS_STATE_FOREGROUND: usize = 2;
const UID_PROCESS_STATE_BACKGROUND: usize = 3;
const UID_PROCESS_STATE_TOP_SLEEPING: usize = 4;
const UID_PROCESS_STATE_HEAVY_WEIGHT: usize = 5;
const UID_PROCESS_STATE_CACHED: usize = 6;
const NUM_PROCESS_STATE: usize = 7;
const UID_PROCESS_STATE_NONEXISTENT: usize = NUM_PROCESS_STATE;
/// `ActivityManager.PROCESS_STATE_*` that `mapToInternalProcessState`
/// tells apart.
const PROCESS_STATE_TOP: i32 = 2;
const PROCESS_STATE_BOUND_TOP: i32 = 3;
const PROCESS_STATE_FOREGROUND_SERVICE: i32 = 4;
const PROCESS_STATE_BOUND_FOREGROUND_SERVICE: i32 = 5;
const PROCESS_STATE_IMPORTANT_FOREGROUND: i32 = 6;
const PROCESS_STATE_RECEIVER: i32 = 11;
const PROCESS_STATE_TOP_SLEEPING: i32 = 12;
const PROCESS_STATE_HEAVY_WEIGHT: i32 = 13;
const PROCESS_STATE_NONEXISTENT: i32 = 20;
/// `BatteryConsumer.PROCESS_STATE_*`.
const CONSUMER_PROCESS_STATE_UNSPECIFIED: i32 = 0;
const CONSUMER_PROCESS_STATE_FOREGROUND: i32 = 1;
const CONSUMER_PROCESS_STATE_BACKGROUND: i32 = 2;
const CONSUMER_PROCESS_STATE_FOREGROUND_SERVICE: i32 = 3;
const CONSUMER_PROCESS_STATE_CACHED: i32 = 4;

/// `BatteryStats.mapToInternalProcessState`.
fn internal_process_state(state: i32) -> usize {
    match state {
        PROCESS_STATE_NONEXISTENT => UID_PROCESS_STATE_NONEXISTENT,
        PROCESS_STATE_TOP => UID_PROCESS_STATE_TOP,
        PROCESS_STATE_BOUND_TOP => UID_PROCESS_STATE_BACKGROUND,
        PROCESS_STATE_FOREGROUND_SERVICE | PROCESS_STATE_BOUND_FOREGROUND_SERVICE => {
            UID_PROCESS_STATE_FOREGROUND_SERVICE
        }
        s if s <= PROCESS_STATE_IMPORTANT_FOREGROUND => UID_PROCESS_STATE_FOREGROUND,
        s if s <= PROCESS_STATE_RECEIVER => UID_PROCESS_STATE_BACKGROUND,
        s if s <= PROCESS_STATE_TOP_SLEEPING => UID_PROCESS_STATE_TOP_SLEEPING,
        s if s <= PROCESS_STATE_HEAVY_WEIGHT => UID_PROCESS_STATE_HEAVY_WEIGHT,
        _ => UID_PROCESS_STATE_CACHED,
    }
}

/// `BatteryStats.mapUidProcessStateToBatteryConsumerProcessState`.
fn consumer_process_state(state: usize) -> i32 {
    match state {
        UID_PROCESS_STATE_TOP | UID_PROCESS_STATE_FOREGROUND => CONSUMER_PROCESS_STATE_FOREGROUND,
        UID_PROCESS_STATE_BACKGROUND | UID_PROCESS_STATE_TOP_SLEEPING => {
            CONSUMER_PROCESS_STATE_BACKGROUND
        }
        UID_PROCESS_STATE_FOREGROUND_SERVICE => CONSUMER_PROCESS_STATE_FOREGROUND_SERVICE,
        UID_PROCESS_STATE_CACHED => CONSUMER_PROCESS_STATE_CACHED,
        _ => CONSUMER_PROCESS_STATE_UNSPECIFIED,
    }
}

/// `BatteryStatsImpl.Uid`, as far as its process state and Bluetooth
/// scans go.
#[derive(Debug)]
struct Uid {
    process_state: usize,
    /// `mProcessStateTimer`, on the on-battery time base.
    process_state_timers: [Option<StopwatchTimer>; NUM_PROCESS_STATE],
    /// `mOnBatteryBackgroundTimeBase`.
    background: TimeBase,
    /// `mBluetoothScanTimer`: its battery time is pooled with the other
    /// uids' (`mBluetoothScanOnTimers`).
    bluetooth_scan: Option<DualTimer>,
    bluetooth_unoptimized_scan: Option<DualTimer>,
    bluetooth_scan_results: Option<Counter>,
    bluetooth_scan_results_background: Option<Counter>,
}

impl Uid {
    /// `new Uid`: no process yet, its background time base stopped.
    fn new(now: i64) -> Self {
        Uid {
            process_state: UID_PROCESS_STATE_NONEXISTENT,
            process_state_timers: Default::default(),
            background: TimeBase {
                realtime_start_us: now * 1000,
                ..TimeBase::default()
            },
            bluetooth_scan: None,
            bluetooth_unoptimized_scan: None,
            bluetooth_scan_results: None,
            bluetooth_scan_results_background: None,
        }
    }

    /// `isInBackground`: neither top nor foreground (nor in a foreground
    /// service), a uid with no process too.
    fn in_background(&self) -> bool {
        self.process_state >= UID_PROCESS_STATE_BACKGROUND
    }

    /// `updateOnBatteryBgTimeBase`, and the timers on it.
    fn update_background(&mut self, on_battery: bool, now: i64) {
        let running = on_battery && self.in_background();
        let Some(base_us) = self.background.set_running(running, now * 1000) else {
            return;
        };
        for t in [
            &mut self.bluetooth_scan,
            &mut self.bluetooth_unoptimized_scan,
        ]
        .into_iter()
        .flatten()
        {
            if running {
                t.background.time_started(base_us);
            } else {
                t.background.time_stopped_in(base_us, 0);
            }
        }
    }
}

/// A note's nesting counted in or out: whether it is the first start or
/// the last stop. A stop with none started is dropped.
fn count_note(nesting: &mut i32, start: bool) -> bool {
    if start {
        *nesting += 1;
        *nesting == 1
    } else if *nesting == 0 {
        false
    } else {
        *nesting -= 1;
        *nesting == 0
    }
}

/// What `setBatteryState` is given.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BatteryState {
    pub status: i32,
    pub health: i32,
    pub plug_type: i32,
    pub level: i32,
    pub temperature: i32,
    pub voltage: i32,
    pub charge_uah: i32,
    pub time_to_full: i64,
}

/// `BatteryStats.LevelStepTracker`.
#[derive(Clone, Debug, Default, PartialEq)]
struct LevelStepTracker {
    last_step_time: Option<i64>,
    durations: Vec<i64>,
}

impl LevelStepTracker {
    fn init(&mut self) {
        *self = LevelStepTracker::default();
    }

    fn clear_time(&mut self) {
        self.last_step_time = None;
    }

    /// `addLevelSteps`: the time since the last step spread over the
    /// levels passed, newest first, at most `MAX_LEVEL_STEPS` kept.
    fn add_level_steps(&mut self, levels: i32, mode_bits: i64, now: i64) {
        if let Some(last) = self.last_step_time
            && levels > 0
        {
            let mut duration = now - last;
            for i in 0..levels {
                let this = (duration / (levels - i) as i64).min(STEP_LEVEL_TIME_MASK);
                duration -= duration / (levels - i) as i64;
                self.durations.insert(0, this | mode_bits);
            }
            self.durations.truncate(MAX_LEVEL_STEPS);
        }
        self.last_step_time = Some(now);
    }

    /// `computeTimePerLevel`: the average step, none without one.
    fn time_per_level(&self) -> Option<i64> {
        if self.durations.is_empty() {
            return None;
        }
        Some(
            self.durations
                .iter()
                .map(|d| d & STEP_LEVEL_TIME_MASK)
                .sum::<i64>()
                / self.durations.len() as i64,
        )
    }
}

/// `BatteryStatsImpl`'s battery state.
#[derive(Debug, Default)]
struct Stats {
    have_level: bool,
    on_battery: bool,
    level: i32,
    status: i32,
    health: i32,
    plug_type: i32,
    temperature: i32,
    voltage: i32,
    charge_uah: i32,
    charging: bool,
    /// When the deferred `setChargingLocked(true)` is due
    /// (`mDeferSetCharging`).
    charging_due: Option<i64>,
    discharge_plug_level: i32,
    discharge_unplug_level: i32,
    high_discharge_since_charge: i32,
    last_charge_step_level: i32,
    max_charge_step_level: i32,
    last_discharge_step_level: i32,
    min_discharge_step_level: i32,
    init_step_mode: i64,
    mod_step_mode: i64,
    charge_steps: LevelStepTracker,
    discharge_steps: LevelStepTracker,
    time_to_full_seconds: i64,
    /// The history, once the guest's `/data` is known.
    history: Option<History>,
    /// When the history was last written (`mLastWriteTimeMs`).
    last_write: i64,
    on_battery_time_base: TimeBase,
    audio_on_nesting: i32,
    audio_on_timer: StopwatchTimer,
    video_on_nesting: i32,
    video_on_timer: StopwatchTimer,
    bluetooth_scan_nesting: i32,
    bluetooth_scan_timer: StopwatchTimer,
    /// The uids whose Bluetooth scan timer runs (`mBluetoothScanOnTimers`).
    bluetooth_scan_pool: Vec<i32>,
    /// `mUidStats`.
    uids: BTreeMap<i32, Uid>,
}

/// The configuration the stats are decided by.
#[derive(Clone, Copy, Debug)]
struct Config {
    charged_delay_ms: i64,
    charging_enforce_level: i32,
    /// `BatteryStatsConfig`'s resets on unplugging.
    reset_high_level: bool,
    reset_significant_charge: bool,
    /// `Constants.MAX_HISTORY_BUFFER` and `MAX_HISTORY_SIZE`, bytes.
    max_history_buffer: usize,
    max_history_size: u64,
}

impl Stats {
    /// `setBatteryStateLocked` at elapsed `now`: whether the charging
    /// state changed.
    fn set_battery_state(&mut self, c: &Config, b: BatteryState, now: i64) -> bool {
        let BatteryState {
            status,
            health,
            plug_type,
            level,
            temperature,
            voltage,
            charge_uah,
            time_to_full,
        } = b;
        let on_battery = plug_type == PLUGGED_NONE && status != STATUS_UNKNOWN;
        let mut changed = false;
        if !self.have_level {
            self.have_level = true;
            // `mOnBattery` starts false: a first reading on battery is a
            // change below; one plugged in is twiddled here.
            if let Some(h) = &mut self.history {
                if on_battery == self.on_battery {
                    h.set_plugged_in_state(!on_battery);
                }
                h.set_initial_battery_state(true, status, level, charge_uah);
            }
            self.status = status;
            self.level = level;
            self.charge_uah = charge_uah;
            (self.max_charge_step_level, self.min_discharge_step_level) = (level, level);
            (self.last_charge_step_level, self.last_discharge_step_level) = (level, level);
            self.init_step_mode = STEP_MODE_SCREEN_ON;
        }
        let old_status = self.status;
        if let Some(h) = &mut self.history
            && (on_battery || level < 96 && status != STATUS_UNKNOWN)
            && !h.is_recording()
        {
            h.start_recording(now, true);
        }
        if self.discharge_plug_level < 0 {
            self.discharge_plug_level = level;
        }
        if on_battery != self.on_battery {
            self.level = level;
            self.status = status;
            self.health = health;
            self.plug_type = plug_type;
            self.temperature = temperature;
            self.voltage = voltage;
            if let Some(h) = &mut self.history {
                h.set_battery_state(
                    status,
                    level,
                    health,
                    plug_type,
                    temperature,
                    voltage,
                    charge_uah,
                );
            }
            self.charge_uah = charge_uah;
            changed |= self.set_on_battery(c, on_battery, old_status, level, now);
        } else {
            let mut recorded = false;
            for (kept, new) in [
                (&mut self.level, level),
                (&mut self.status, status),
                (&mut self.health, health),
                (&mut self.plug_type, plug_type),
            ] {
                if *kept != new {
                    *kept = new;
                    recorded = true;
                }
            }
            for (kept, new, by) in [
                (&mut self.temperature, temperature, 10),
                (&mut self.voltage, voltage, 20),
                (&mut self.charge_uah, charge_uah, 10),
            ] {
                if new >= *kept + by || new <= *kept - by {
                    *kept = new;
                    recorded = true;
                }
            }
            let mode_bits = self.init_step_mode << STEP_LEVEL_INITIAL_MODE_SHIFT
                | self.mod_step_mode << STEP_LEVEL_MODIFIED_MODE_SHIFT
                | ((level & 0xff) as i64) << STEP_LEVEL_LEVEL_SHIFT;
            if on_battery {
                changed |= self.set_charging(false);
                if self.last_discharge_step_level != level && self.min_discharge_step_level > level
                {
                    self.discharge_steps.add_level_steps(
                        self.last_discharge_step_level - level,
                        mode_bits,
                        now,
                    );
                    self.last_discharge_step_level = level;
                    self.min_discharge_step_level = level;
                    (self.init_step_mode, self.mod_step_mode) = (STEP_MODE_SCREEN_ON, 0);
                }
            } else {
                if level >= c.charging_enforce_level {
                    changed |= self.set_charging(true);
                } else if !self.charging {
                    if self.last_charge_step_level < level {
                        self.charging_due.get_or_insert(now + c.charged_delay_ms);
                    } else if self.last_charge_step_level > level {
                        self.charging_due = None;
                    }
                } else if self.last_charge_step_level > level {
                    changed |= self.set_charging(false);
                }
                if self.last_charge_step_level != level && self.max_charge_step_level < level {
                    self.charge_steps.add_level_steps(
                        level - self.last_charge_step_level,
                        mode_bits,
                        now,
                    );
                    self.max_charge_step_level = level;
                    (self.init_step_mode, self.mod_step_mode) = (STEP_MODE_SCREEN_ON, 0);
                }
                self.last_charge_step_level = level;
            }
            if (recorded || changed)
                && let Some(h) = &mut self.history
            {
                h.set_battery_state(
                    self.status,
                    self.level,
                    self.health,
                    self.plug_type,
                    self.temperature,
                    self.voltage,
                    self.charge_uah,
                );
                h.write_item(now);
            }
        }
        self.time_to_full_seconds = time_to_full;
        changed
    }

    /// `setOnBatteryLocked`'s charging, step trackers, history and time
    /// base.
    fn set_on_battery(
        &mut self,
        c: &Config,
        on_battery: bool,
        old_status: i32,
        level: i32,
        now: i64,
    ) -> bool {
        let mut changed = false;
        let mut reset = false;
        if on_battery {
            // `shouldResetOnUnplugLocked`.
            reset = c.reset_high_level && (old_status == STATUS_FULL || level >= 90)
                || c.reset_significant_charge && self.discharge_plug_level < 20 && level >= 80
                || self.high_discharge_since_charge >= 200;
            if reset {
                self.discharge_steps.init();
                self.high_discharge_since_charge = 0;
            }
            if self.charging {
                changed |= self.set_charging(false);
            }
            self.on_battery = true;
            self.last_discharge_step_level = level;
            self.min_discharge_step_level = level;
            self.discharge_steps.clear_time();
            (self.init_step_mode, self.mod_step_mode) = (STEP_MODE_SCREEN_ON, 0);
            if let Some(h) = &mut self.history {
                if reset {
                    h.start_recording(now, true);
                }
                h.record_battery_state(now, level, false);
            }
            self.discharge_unplug_level = level;
        } else {
            self.on_battery = false;
            if let Some(h) = &mut self.history {
                h.record_battery_state(now, level, true);
            }
            self.discharge_plug_level = level;
            if level < self.discharge_unplug_level {
                self.high_discharge_since_charge += self.discharge_unplug_level - level;
            }
            self.charge_steps.init();
            self.last_charge_step_level = level;
            self.max_charge_step_level = level;
            (self.init_step_mode, self.mod_step_mode) = (STEP_MODE_SCREEN_ON, 0);
        }
        self.update_time_base(on_battery, now);
        if (reset || self.last_write + WRITE_INTERVAL_MS < now)
            && let Some(h) = &mut self.history
        {
            h.write_history(now, false);
            self.last_write = now;
        }
        changed
    }

    /// `updateTimeBasesLocked`'s on-battery time base, the timers on it,
    /// and the uids' background time bases.
    fn update_time_base(&mut self, unplugged: bool, now: i64) {
        let Some(base_us) = self.on_battery_time_base.set_running(unplugged, now * 1000) else {
            return;
        };
        let pool = self.bluetooth_scan_pool.len();
        if unplugged {
            for u in self.uids.values_mut() {
                for t in [&mut u.bluetooth_scan, &mut u.bluetooth_unoptimized_scan]
                    .into_iter()
                    .flatten()
                {
                    t.timer.time_started(base_us);
                }
            }
        } else {
            self.audio_on_timer.time_stopped(base_us);
            self.video_on_timer.time_stopped(base_us);
            self.bluetooth_scan_timer.time_stopped(base_us);
            for u in self.uids.values_mut() {
                if let Some(t) = &mut u.bluetooth_scan {
                    t.timer.time_stopped_in(base_us, pool);
                }
                if let Some(t) = &mut u.bluetooth_unoptimized_scan {
                    t.timer.time_stopped_in(base_us, 0);
                }
                for t in u.process_state_timers.iter_mut().flatten() {
                    t.time_stopped(base_us);
                }
            }
        }
        for u in self.uids.values_mut() {
            u.update_background(unplugged, now);
        }
    }

    /// `getUidStatsLocked`.
    fn uid(&mut self, uid: i32, now: i64) -> &mut Uid {
        self.uids.entry(uid).or_insert_with(|| Uid::new(now))
    }

    /// `noteUidProcessStateLocked` of the mapped `uid` (an isolated uid's
    /// state being its owner's already): `updateUidProcessStateLocked`'s
    /// process state timers, background time base and history.
    fn note_uid_process_state(&mut self, uid: i32, isolated: bool, state: i32, now: i64) {
        if isolated {
            return;
        }
        let on_battery = self.on_battery_time_base.running;
        let state = internal_process_state(state);
        let u = self.uids.entry(uid).or_insert_with(|| Uid::new(now));
        if u.process_state == state {
            return;
        }
        if u.process_state != UID_PROCESS_STATE_NONEXISTENT
            && let Some(t) = &mut u.process_state_timers[u.process_state]
        {
            t.stop(&self.on_battery_time_base, now);
        }
        if state != UID_PROCESS_STATE_NONEXISTENT {
            u.process_state_timers[state]
                .get_or_insert_with(StopwatchTimer::default)
                .start(&self.on_battery_time_base, now);
        }
        u.process_state = state;
        u.update_background(on_battery, now);
        if let Some(h) = &mut self.history {
            h.record_process_state_change(now, uid, consumer_process_state(state));
        }
    }

    /// `noteBluetoothScanStartedLocked` of the mapped `uid`.
    fn note_bluetooth_scan_started(&mut self, uid: i32, unoptimized: bool, now: i64) {
        if self.bluetooth_scan_nesting == 0 {
            if let Some(h) = &mut self.history {
                h.record_state2(now, STATE2_BLUETOOTH_SCAN_FLAG, true, None);
            }
            self.bluetooth_scan_timer
                .start(&self.on_battery_time_base, now);
        }
        self.bluetooth_scan_nesting += 1;
        // `Uid.noteBluetoothScanStartedLocked`: the pooled scan timer,
        // then its background half.
        let base = &self.on_battery_time_base;
        let rt = base.realtime(now * 1000);
        let u = self.uids.entry(uid).or_insert_with(|| Uid::new(now));
        let t = u.bluetooth_scan.get_or_insert_with(DualTimer::default);
        if t.timer.timer.begin_start(rt) {
            refresh_scan_pool(rt, &self.bluetooth_scan_pool, &mut self.uids);
            self.bluetooth_scan_pool.push(uid);
            let u = self.uids.get_mut(&uid).expect("just made");
            let t = u.bluetooth_scan.as_mut().expect("just made");
            t.timer.timer.finish_start(base);
        }
        let u = self.uids.get_mut(&uid).expect("just made");
        let t = u.bluetooth_scan.as_mut().expect("just made");
        t.timer.started(base, now);
        t.background.start(&u.background, now);
        if unoptimized {
            let t = u
                .bluetooth_unoptimized_scan
                .get_or_insert_with(DualTimer::default);
            t.timer.start(base, now);
            t.background.start(&u.background, now);
        }
    }

    /// `noteBluetoothScanStoppedLocked` of the mapped `uid`.
    fn note_bluetooth_scan_stopped(&mut self, uid: i32, unoptimized: bool, now: i64) {
        self.bluetooth_scan_nesting -= 1;
        if self.bluetooth_scan_nesting == 0 {
            if let Some(h) = &mut self.history {
                h.record_state2(now, STATE2_BLUETOOTH_SCAN_FLAG, false, None);
            }
            self.bluetooth_scan_timer
                .stop(&self.on_battery_time_base, now);
        }
        let u = self.uid(uid, now);
        if u.bluetooth_scan.is_some() {
            self.stop_bluetooth_scan(uid, now);
        }
        let base = &self.on_battery_time_base;
        let u = self.uids.get_mut(&uid).expect("just made");
        if unoptimized && let Some(t) = &mut u.bluetooth_unoptimized_scan {
            t.timer.stop(base, now);
            t.background.stop(&u.background, now);
        }
    }

    /// `DualTimer.stopRunningLocked` of `uid`'s pooled scan timer.
    fn stop_bluetooth_scan(&mut self, uid: i32, now: i64) {
        let base = &self.on_battery_time_base;
        let rt = base.realtime(now * 1000);
        let t = self
            .uids
            .get_mut(&uid)
            .and_then(|u| u.bluetooth_scan.as_mut());
        let Some(t) = t else { return };
        t.timer.stopping(base, now);
        if t.timer.timer.begin_stop() {
            refresh_scan_pool(rt, &self.bluetooth_scan_pool, &mut self.uids);
            self.bluetooth_scan_pool.retain(|&u| u != uid);
            let u = self.uids.get_mut(&uid).expect("scanning");
            let t = u.bluetooth_scan.as_mut().expect("scanning");
            t.timer.timer.finish_stop();
        }
        let u = self.uids.get_mut(&uid).expect("scanning");
        let t = u.bluetooth_scan.as_mut().expect("scanning");
        t.background.stop(&u.background, now);
    }

    /// `noteResetBluetoothScanLocked`.
    fn reset_bluetooth_scan(&mut self, now: i64) {
        if self.bluetooth_scan_nesting <= 0 {
            return;
        }
        self.bluetooth_scan_nesting = 0;
        if let Some(h) = &mut self.history {
            h.record_state2(now, STATE2_BLUETOOTH_SCAN_FLAG, false, None);
        }
        self.bluetooth_scan_timer
            .stop_all(&self.on_battery_time_base, now);
        let uids: Vec<i32> = self.uids.keys().copied().collect();
        for uid in uids {
            // `DualTimer.stopAllRunningLocked`: stopped once as a whole,
            // then its background half all the way.
            let running = self.uids[&uid]
                .bluetooth_scan
                .as_ref()
                .is_some_and(|t| t.timer.timer.nesting > 0);
            if running {
                let t = self
                    .uids
                    .get_mut(&uid)
                    .and_then(|u| u.bluetooth_scan.as_mut());
                t.expect("running").timer.timer.nesting = 1;
                self.stop_bluetooth_scan(uid, now);
            }
            let base = &self.on_battery_time_base;
            let u = self.uids.get_mut(&uid).expect("listed");
            if let Some(t) = &mut u.bluetooth_scan {
                t.background.stop_all(&u.background, now);
            }
            if let Some(t) = &mut u.bluetooth_unoptimized_scan {
                if t.timer.timer.nesting > 0 {
                    t.timer.timer.nesting = 1;
                    t.timer.stop(base, now);
                    t.background.stop(&u.background, now);
                }
                t.background.stop_all(&u.background, now);
            }
        }
    }

    /// `noteBluetoothScanResultsLocked` of the mapped `uid`.
    fn note_bluetooth_scan_results(&mut self, uid: i32, results: i32, now: i64) {
        let base = &self.on_battery_time_base;
        let u = self.uids.entry(uid).or_insert_with(|| Uid::new(now));
        u.bluetooth_scan_results
            .get_or_insert_with(Counter::default)
            .add(base, results);
        // On the background time base: counted only in the background.
        u.bluetooth_scan_results_background
            .get_or_insert_with(Counter::default)
            .add(&u.background, results);
    }

    /// `setChargingLocked`: whether it changed.
    fn set_charging(&mut self, charging: bool) -> bool {
        self.charging_due = None;
        if self.charging == charging {
            return false;
        }
        self.charging = charging;
        if let Some(h) = &mut self.history {
            h.set_charging_state(charging);
        }
        true
    }

    /// `noteAudioOnLocked`/`noteAudioOffLocked` of the mapped `uid`, with
    /// the audio power stats collector on: the history and the device
    /// timer, no per-uid timer.
    fn note_audio(&mut self, on: bool, uid: i32, now: i64) {
        if count_note(&mut self.audio_on_nesting, on) {
            if let Some(h) = &mut self.history {
                h.record_state(now, STATE_AUDIO_ON_FLAG, on, Some((uid, "audio")));
            }
            self.audio_on_timer
                .start_or_stop(on, &self.on_battery_time_base, now);
        }
    }

    /// `noteVideoOnLocked`/`noteVideoOffLocked`, as the audio's.
    fn note_video(&mut self, on: bool, uid: i32, now: i64) {
        if count_note(&mut self.video_on_nesting, on) {
            if let Some(h) = &mut self.history {
                h.record_state2(now, STATE2_VIDEO_ON_FLAG, on, Some((uid, "video")));
            }
            self.video_on_timer
                .start_or_stop(on, &self.on_battery_time_base, now);
        }
    }

    /// `noteResetAudioLocked`.
    fn reset_audio(&mut self, now: i64) {
        if self.audio_on_nesting > 0 {
            self.audio_on_nesting = 0;
            if let Some(h) = &mut self.history {
                h.record_state(now, STATE_AUDIO_ON_FLAG, false, None);
            }
            self.audio_on_timer
                .stop_all(&self.on_battery_time_base, now);
        }
    }

    /// `noteResetVideoLocked`.
    fn reset_video(&mut self, now: i64) {
        if self.video_on_nesting > 0 {
            self.video_on_nesting = 0;
            if let Some(h) = &mut self.history {
                h.record_state2(now, STATE2_VIDEO_ON_FLAG, false, None);
            }
            self.video_on_timer
                .stop_all(&self.on_battery_time_base, now);
        }
    }

    /// `computeChargeTimeRemaining`, microseconds.
    fn charge_time_remaining(&self) -> i64 {
        if self.on_battery {
            return -1;
        }
        if self.time_to_full_seconds >= 0 {
            return self.time_to_full_seconds * 1_000_000;
        }
        match self.charge_steps.time_per_level() {
            Some(ms) if ms > 0 => ms * (100 - self.level) as i64 * 1000,
            _ => -1,
        }
    }

    /// `computeBatteryTimeRemaining`, microseconds.
    fn battery_time_remaining(&self) -> i64 {
        if !self.on_battery {
            return -1;
        }
        match self.discharge_steps.time_per_level() {
            Some(ms) if ms > 0 => ms * self.level as i64 * 1000,
            _ => -1,
        }
    }
}

pub struct BatteryStats {
    host: Weak<AppHost>,
    stats: Mutex<Stats>,
    config: OnceLock<Config>,
}

impl BatteryStats {
    pub fn new(host: &Arc<AppHost>) -> Self {
        let stats = Stats {
            discharge_plug_level: -1,
            time_to_full_seconds: -1,
            ..Stats::default()
        };
        BatteryStats {
            host: Arc::downgrade(host),
            stats: Mutex::new(stats),
            config: OnceLock::new(),
        }
    }

    /// The constants (`Constants.updateConstants`: the
    /// `battery_stats_constants` setting, a `KeyValueListParser` list) and
    /// `BatteryStatsConfig`'s resets and history size, read once.
    fn config(&self, host: &AppHost) -> Option<Config> {
        if let Some(c) = self.config.get() {
            return Some(*c);
        }
        let inputs = host.inputs().ok()?;
        let setting = inputs
            .settings
            .get(super::settings::Table::Global, "battery_stats_constants");
        let constant = |key: &str| {
            setting
                .as_deref()?
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .and_then(|(_, v)| v.trim().parse::<i64>().ok())
        };
        let resource = |name: &str| inputs.packages.framework_bool(name).unwrap_or(false);
        // `ActivityManager.isLowRamDeviceStatic`.
        let low_ram = super::system_boolean(host.property("ro.config.low_ram").as_deref(), false)
            || host.property("ro.debuggable").as_deref() == Some("1")
                && super::system_boolean(host.property("debug.force_low_ram").as_deref(), false);
        let max_history_buffer = constant("max_history_buffer_kb").unwrap_or(if low_ram {
            DEFAULT_MAX_HISTORY_BUFFER_LOW_RAM_DEVICE_KB
        } else {
            DEFAULT_MAX_HISTORY_BUFFER_KB
        }) * 1024;
        let max_history_size = constant("max_history_size")
            .or_else(|| constant("max_history_files").map(|files| files * max_history_buffer))
            .unwrap_or_else(|| {
                inputs
                    .packages
                    .framework_integer("config_batteryHistoryStorageSize")
                    .map_or(DEFAULT_MAX_HISTORY_SIZE, i64::from)
            });
        let config = Config {
            charged_delay_ms: constant("battery_charged_delay_ms")
                .unwrap_or(DEFAULT_BATTERY_CHARGED_DELAY_MS),
            charging_enforce_level: constant("battery_charging_enforce_level")
                .map_or(DEFAULT_BATTERY_CHARGING_ENFORCE_LEVEL, |v| v as i32),
            reset_high_level: resource("config_batteryStatsResetOnUnplugHighBatteryLevel"),
            reset_significant_charge: resource(
                "config_batteryStatsResetOnUnplugAfterSignificantCharge",
            ),
            max_history_buffer: max_history_buffer.max(0) as usize,
            max_history_size: max_history_size.max(0) as u64,
        };
        Some(*self.config.get_or_init(|| config))
    }

    /// The history, made as `BatteryStatsService.create` makes it once
    /// the guest's `/data` is known: read back (`readLocked`, which has
    /// no `batterystats.bin` to read before it), continued, and written
    /// (`scheduleWriteToDisk`).
    fn start_history(&self, host: &AppHost, config: &Config, s: &mut Stats) {
        if s.history.is_some() {
            return;
        }
        let Ok(data) = host.data() else {
            return;
        };
        let now = elapsed_realtime();
        // Started again at the next call when the flags cannot be read.
        let compression = match host.flag(HISTORY_COMPRESSION_FLAG) {
            Ok(on) => on,
            Err(e) => return eprintln!("app host: batterystats: history: {}", e.message),
        };
        let (dir, clock) = super::battery_history::paths(&data.join("system"));
        let mut h = History::new(
            Directory::new(dir, config.max_history_size, compression),
            MonotonicClock::new(clock, now),
            config.max_history_buffer,
            now,
        );
        if !h.read_summary(now) {
            // `resetAllStatsLocked(RESET_REASON_CORRUPT_FILE)`, its
            // history reset and written.
            h.reset(now);
            h.write_history(now, false);
        }
        h.continue_recording(now);
        h.write_history(now, false);
        s.last_write = now;
        s.history = Some(h);
    }

    /// `isCharging` (`BatteryManager.isCharging`).
    pub(super) fn charging(&self) -> bool {
        self.stats.lock().unwrap().charging
    }

    /// `setBatteryState`, as BatteryService calls it with each health
    /// info it processes.
    pub(super) fn set_battery_state(self: &Arc<Self>, battery: BatteryState, now: i64) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let Some(config) = self.config(&host) else {
            return eprintln!("app host: batterystats: no configuration yet");
        };
        let (changed, due) = {
            let mut s = self.stats.lock().unwrap();
            self.start_history(&host, &config, &mut s);
            (s.set_battery_state(&config, battery, now), s.charging_due)
        };
        if changed {
            self.report_charging();
        }
        if let Some(due) = due {
            let me = Arc::downgrade(self);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis((due - now).max(0) as u64));
                if let Some(me) = me.upgrade() {
                    me.deferred_charging(due);
                }
            });
        }
    }

    /// `mDeferSetCharging`: charging, if still due and plugged in, and
    /// written to the history.
    fn deferred_charging(&self, due: i64) {
        let changed = {
            let mut s = self.stats.lock().unwrap();
            let changed = s.charging_due == Some(due) && !s.on_battery && s.set_charging(true);
            if changed && let Some(h) = &mut s.history {
                h.write_item(elapsed_realtime());
            }
            changed
        };
        if changed {
            self.report_charging();
        }
    }

    /// `MSG_REPORT_CHARGING`.
    fn report_charging(&self) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let action = if self.stats.lock().unwrap().charging {
            ACTION_CHARGING
        } else {
            ACTION_DISCHARGING
        };
        host.broadcasts.send_from_system(
            &host,
            RawIntent::new(action, FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT, &[]),
        );
    }

    /// A note from a caller holding UPDATE_DEVICE_STATS (its
    /// `@EnforcePermission`), applied at the time it is taken.
    fn note(
        &self,
        caller: i32,
        apply: impl FnOnce(&mut Stats, &dyn Fn(i32) -> i32, i64),
    ) -> Result<(), Exception> {
        let host = self.host.upgrade().ok_or_else(|| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "no app host")
        })?;
        if host.check_permission(Some(UPDATE_DEVICE_STATS), caller)? != 0 {
            return Err(Exception::security(format!(
                "Access denied, requires: {UPDATE_DEVICE_STATS}"
            )));
        }
        let now = elapsed_realtime();
        let (state, _) = host.packages()?;
        let map_uid = |uid: i32| map_uid(&state.system.isolated_owners, uid);
        let mut s = self.stats.lock().unwrap();
        if let Some(config) = self.config(&host) {
            self.start_history(&host, &config, &mut s);
        }
        apply(&mut s, &map_uid, now);
        Ok(())
    }

    /// `noteUidProcessState`, as the activity manager tells it a uid's
    /// process state (`ProcessList.addProcessNameLocked` and
    /// `removeProcessNameLocked`: a uid's first process and its last).
    pub(super) fn note_uid_process_state(&self, uid: i32, state: i32) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let now = elapsed_realtime();
        let (state_model, _) = match host.packages() {
            Ok(p) => p,
            Err(e) => return eprintln!("app host: batterystats: {}", e.message),
        };
        let parent = map_uid(&state_model.system.isolated_owners, uid);
        let isolated = parent != uid && super::is_isolated(uid);
        let mut s = self.stats.lock().unwrap();
        if let Some(config) = self.config(&host) {
            self.start_history(&host, &config, &mut s);
        }
        s.note_uid_process_state(parent, isolated, state, now);
    }
}

/// `noteBluetoothScan*FromSourceLocked`: each uid of `ws`, then each work
/// chain's attribution uid, mapped.
fn work_source_uids(ws: &WorkSource, map: &dyn Fn(i32) -> i32) -> Vec<i32> {
    ws.uids
        .iter()
        .copied()
        .chain(
            ws.chains
                .iter()
                .flatten()
                // `WorkChain.getAttributionUid`.
                .map(|c| c.uids.first().copied().unwrap_or(-1)),
        )
        .map(map)
        .collect()
}

/// A work source the original's handler would throw on
/// (`WorkSource.size()` of a null one) answered as that exception.
fn work_source(ws: Option<WorkSource>) -> Result<WorkSource, Exception> {
    ws.ok_or_else(|| {
        Exception::new(
            aim_binder_host::parcel::EX_NULL_POINTER,
            "Attempt to invoke virtual method 'int android.os.WorkSource.size()' on a null object reference",
        )
    })
}

/// `SystemClock.elapsedRealtime`, as the app host keeps it.
fn elapsed_realtime() -> i64 {
    super::clock_ms(libc::CLOCK_MONOTONIC_RAW)
}

/// `mapUid`: an SDK sandbox's app, an isolated uid's owner
/// (`PowerStatsUidResolver`), else the uid.
fn map_uid(isolated_owners: &[(i32, i32)], uid: i32) -> i32 {
    let app_id = uid.rem_euclid(PER_USER_RANGE);
    if (FIRST_SDK_SANDBOX_UID..=LAST_SDK_SANDBOX_UID).contains(&app_id) {
        return uid - SDK_SANDBOX_OFFSET;
    }
    isolated_owners
        .iter()
        .find(|(isolated, _)| *isolated == uid)
        .map_or(uid, |(_, owner)| *owner)
}

impl Service for BatteryStats {
    fn descriptor(&self) -> &str {
        bs::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let caller = call.sender_euid as i32;
        let bad = |_| BAD_VALUE;
        // The notes; the audio ones are one-way.
        let noted = match call.code {
            bs::NOTE_START_AUDIO => {
                let uid = bs::NoteStartAudio::read(&mut call.data).map_err(bad)?.uid;
                Some(self.note(caller, |s, map, now| s.note_audio(true, map(uid), now)))
            }
            bs::NOTE_STOP_AUDIO => {
                let uid = bs::NoteStopAudio::read(&mut call.data).map_err(bad)?.uid;
                Some(self.note(caller, |s, map, now| s.note_audio(false, map(uid), now)))
            }
            bs::NOTE_RESET_AUDIO => {
                bs::NoteResetAudio::read(&mut call.data).map_err(bad)?;
                Some(self.note(caller, |s, _, now| s.reset_audio(now)))
            }
            bs::NOTE_START_VIDEO => {
                let uid = bs::NoteStartVideo::read(&mut call.data).map_err(bad)?.uid;
                let r = self.note(caller, |s, map, now| s.note_video(true, map(uid), now));
                Some(r.map(|()| bs::write_note_start_video_reply(&mut p)))
            }
            bs::NOTE_STOP_VIDEO => {
                let uid = bs::NoteStopVideo::read(&mut call.data).map_err(bad)?.uid;
                let r = self.note(caller, |s, map, now| s.note_video(false, map(uid), now));
                Some(r.map(|()| bs::write_note_stop_video_reply(&mut p)))
            }
            bs::NOTE_RESET_VIDEO => {
                bs::NoteResetVideo::read(&mut call.data).map_err(bad)?;
                let r = self.note(caller, |s, _, now| s.reset_video(now));
                Some(r.map(|()| bs::write_note_reset_video_reply(&mut p)))
            }
            bs::NOTE_BLE_SCAN_STARTED => {
                let a = bs::NoteBleScanStarted::<WorkSource>::read(&mut call.data).map_err(bad)?;
                let r = work_source(a.ws).and_then(|ws| {
                    self.note(caller, |s, map, now| {
                        for uid in work_source_uids(&ws, map) {
                            s.note_bluetooth_scan_started(uid, a.is_unoptimized, now);
                        }
                    })
                });
                Some(r.map(|()| bs::write_note_ble_scan_started_reply(&mut p)))
            }
            bs::NOTE_BLE_SCAN_STOPPED => {
                let a = bs::NoteBleScanStopped::<WorkSource>::read(&mut call.data).map_err(bad)?;
                let r = work_source(a.ws).and_then(|ws| {
                    self.note(caller, |s, map, now| {
                        for uid in work_source_uids(&ws, map) {
                            s.note_bluetooth_scan_stopped(uid, a.is_unoptimized, now);
                        }
                    })
                });
                Some(r.map(|()| bs::write_note_ble_scan_stopped_reply(&mut p)))
            }
            bs::NOTE_BLE_SCAN_RESET => {
                bs::NoteBleScanReset::read(&mut call.data).map_err(bad)?;
                let r = self.note(caller, |s, _, now| s.reset_bluetooth_scan(now));
                Some(r.map(|()| bs::write_note_ble_scan_reset_reply(&mut p)))
            }
            bs::NOTE_BLE_SCAN_RESULTS => {
                let a = bs::NoteBleScanResults::<WorkSource>::read(&mut call.data).map_err(bad)?;
                let r = work_source(a.ws).and_then(|ws| {
                    self.note(caller, |s, map, now| {
                        for uid in work_source_uids(&ws, map) {
                            s.note_bluetooth_scan_results(uid, a.num_new_results, now);
                        }
                    })
                });
                Some(r.map(|()| bs::write_note_ble_scan_results_reply(&mut p)))
            }
            _ => None,
        };
        if let Some(noted) = noted {
            return reply(noted.map(|()| p));
        }
        let s = self.stats.lock().unwrap();
        // The times are microseconds, answered in milliseconds.
        let ms = |us: i64| if us >= 0 { us / 1000 } else { us };
        match call.code {
            bs::IS_CHARGING => {
                bs::IsCharging::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bs::write_is_charging_reply(&mut p, s.charging);
            }
            bs::COMPUTE_BATTERY_TIME_REMAINING => {
                bs::ComputeBatteryTimeRemaining::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bs::write_compute_battery_time_remaining_reply(
                    &mut p,
                    ms(s.battery_time_remaining()),
                );
            }
            bs::COMPUTE_CHARGE_TIME_REMAINING => {
                bs::ComputeChargeTimeRemaining::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bs::write_compute_charge_time_remaining_reply(
                    &mut p,
                    ms(s.charge_time_remaining()),
                );
            }
            // One-way: what fails reaches the caller's receiver. Asking
            // for other uids than its own needs BATTERY_STATS
            // (`onlyCaller`); the snapshots (`getHealthStatsForUidLocked`)
            // are #1928, sent as the original sends a failure to make them.
            bs::TAKE_UID_SNAPSHOTS_ASYNC => {
                let a = bs::TakeUidSnapshotsAsync::<ResultReceiver>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                drop(s);
                let Some(host) = self.host.upgrade() else {
                    return Err(BAD_VALUE);
                };
                let uid = call.sender_euid as i32;
                let receiver = a.result.and_then(|r| r.0);
                let only_caller = a.uid.unwrap_or_default().iter().all(|&u| u == uid);
                match host.check_permission(Some(BATTERY_STATS), uid) {
                    Ok(0) => {}
                    Ok(_) if only_caller => {}
                    Ok(_) => {
                        send_failure(
                            &host,
                            receiver,
                            RESULT_SECURITY_EXCEPTION,
                            &format!("Neither user {uid} nor current process has {BATTERY_STATS}."),
                        );
                        return Ok(p);
                    }
                    Err(e) => return reply(Err(e)),
                }
                let e = host.unimplemented_named("batterystats", "a uid's health stats (#1928)");
                send_failure(&host, receiver, RESULT_RUNTIME_EXCEPTION, &e.message);
            }
            code => {
                drop(s);
                let Some(host) = self.host.upgrade() else {
                    return Err(BAD_VALUE);
                };
                return reply(Err(
                    host.unimplemented_named("batterystats", &format!("#{code} (#1928)"))
                ));
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure reaches the receiver as `ResultReceiver.send` writes it:
    /// the code, then a bundle of the message.
    #[test]
    fn a_failure_is_sent_with_its_message() {
        let p = failure(RESULT_SECURITY_EXCEPTION, "denied");
        let mut r = p.reader();
        r.enforce_interface(RESULT_RECEIVER_DESCRIPTOR).unwrap();
        assert_eq!(r.read_i32().unwrap(), RESULT_SECURITY_EXCEPTION);
        assert_eq!(r.read_i32().unwrap(), 1);
        let bundle = crate::bundle::read(&mut r).unwrap().unwrap();
        assert!(matches!(
            bundle.get(KEY_EXCEPTION_MESSAGE),
            Some(crate::bundle::Value::String(Some(m))) if m == "denied"
        ));
    }

    const C: Config = Config {
        charged_delay_ms: 900_000,
        charging_enforce_level: 90,
        reset_high_level: true,
        reset_significant_charge: true,
        max_history_buffer: 128 * 1024,
        max_history_size: 4 * 1024 * 1024,
    };

    fn b(status: i32, plug_type: i32, level: i32, time_to_full: i64) -> BatteryState {
        BatteryState {
            status,
            plug_type,
            level,
            time_to_full,
            ..BatteryState::default()
        }
    }

    fn stats() -> Stats {
        Stats {
            discharge_plug_level: -1,
            time_to_full_seconds: -1,
            ..Stats::default()
        }
    }

    #[test]
    fn plugged_in_above_the_enforce_level_charges_at_once() {
        let mut s = stats();
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        assert!(s.on_battery && !s.charging);
        s.set_battery_state(&C, b(2, 1, 91, -1), 1000);
        assert!(
            !s.charging,
            "the plug change itself only resets the trackers"
        );
        assert!(s.set_battery_state(&C, b(2, 1, 92, -1), 2000));
        assert!(s.charging);
        assert!(s.set_battery_state(&C, b(3, PLUGGED_NONE, 92, -1), 3000));
        assert!(!s.charging);
    }

    #[test]
    fn a_rising_level_below_it_charges_after_the_delay() {
        let mut s = stats();
        s.set_battery_state(&C, b(2, 1, 40, -1), 0);
        s.set_battery_state(&C, b(2, 1, 41, -1), 60_000);
        assert_eq!(s.charging_due, Some(960_000));
        assert!(!s.charging);
        // The level falling cancels it.
        s.set_battery_state(&C, b(2, 1, 40, -1), 70_000);
        assert_eq!(s.charging_due, None);
    }

    #[test]
    fn charge_time_remaining_follows_the_steps() {
        let mut s = stats();
        s.set_battery_state(&C, b(2, 1, 50, -1), 0);
        s.set_battery_state(&C, b(2, 1, 51, -1), 60_000);
        assert_eq!(
            s.charge_time_remaining(),
            -1,
            "the first step only starts timing"
        );
        s.set_battery_state(&C, b(2, 1, 53, -1), 180_000);
        // 120 s over two levels: 60 s a level, 47 levels to go.
        assert_eq!(s.charge_time_remaining(), 60_000 * 47 * 1000);
        s.set_battery_state(&C, b(2, 1, 53, 600), 190_000);
        assert_eq!(
            s.charge_time_remaining(),
            600_000_000,
            "the HAL's time to full wins"
        );
    }

    use super::super::battery_history::tests::{decode, history, temp_dir};
    use super::super::battery_history::{CMD_RESET, CMD_UPDATE, STATE_BATTERY_PLUGGED_FLAG};

    #[test]
    fn nested_audio_records_its_first_start_and_last_stop() {
        let dir = temp_dir("stats-audio");
        let mut s = stats();
        s.history = Some(history(&dir, 128 * 1024, 0));
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        let before = decode(s.history.as_ref().unwrap().buffer()).len();
        s.note_audio(true, 10_100, 5_000);
        s.note_audio(true, 10_101, 7_000);
        s.note_audio(false, 10_101, 9_000);
        let items = decode(s.history.as_ref().unwrap().buffer());
        assert_eq!(items.len(), before + 1, "only the first start is recorded");
        assert_ne!(items.last().unwrap().states & STATE_AUDIO_ON_FLAG, 0);
        s.note_audio(false, 10_100, 12_000);
        let items = decode(s.history.as_ref().unwrap().buffer());
        assert_eq!(items.len(), before + 2);
        assert_eq!(items.last().unwrap().states & STATE_AUDIO_ON_FLAG, 0);
        // A stop with none running is dropped.
        s.note_audio(false, 10_100, 14_000);
        assert_eq!(
            decode(s.history.as_ref().unwrap().buffer()).len(),
            before + 2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reset_stops_video_however_nested() {
        let dir = temp_dir("stats-video");
        let mut s = stats();
        s.history = Some(history(&dir, 128 * 1024, 0));
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.note_video(true, 10_100, 5_000);
        s.note_video(true, 10_100, 6_000);
        s.reset_video(8_000);
        assert_eq!(s.video_on_nesting, 0);
        let items = decode(s.history.as_ref().unwrap().buffer());
        assert_eq!(items.last().unwrap().states2 & STATE2_VIDEO_ON_FLAG, 0);
        assert_eq!(s.video_on_timer.nesting, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_device_timer_runs_on_battery_only() {
        let mut s = stats();
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        assert!(s.on_battery_time_base.running);
        s.note_audio(true, 10_100, 1_000);
        s.set_battery_state(&C, b(2, 1, 50, -1), 4_000);
        assert!(!s.on_battery_time_base.running);
        s.note_audio(false, 10_100, 10_000);
        assert_eq!(s.audio_on_timer.total_time_us, 3_000_000);
        assert_eq!(s.audio_on_timer.count, 1);
    }

    #[test]
    fn the_history_follows_the_plug_and_the_level() {
        let dir = temp_dir("stats-plug");
        let mut s = stats();
        s.history = Some(history(&dir, 128 * 1024, 0));
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 49, -1), 60_000);
        s.set_battery_state(&C, b(2, 1, 49, -1), 120_000);
        let items = decode(s.history.as_ref().unwrap().buffer());
        assert_eq!(items[0].cmd, CMD_RESET);
        assert!(items[1..].iter().all(|d| d.cmd == CMD_UPDATE));
        let last = items.last().unwrap();
        assert_eq!(last.level, 49);
        assert_ne!(last.states & STATE_BATTERY_PLUGGED_FLAG, 0);
        assert!(
            items
                .iter()
                .any(|d| d.level == 49 && d.states & STATE_BATTERY_PLUGGED_FLAG == 0)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notes_are_kept_for_an_isolated_uids_owner_and_a_sandboxs_app() {
        let owners = [(99_005, 10_123)];
        assert_eq!(map_uid(&owners, 99_005), 10_123);
        assert_eq!(map_uid(&owners, 20_123), 10_123);
        assert_eq!(map_uid(&owners, 1_020_123), 1_010_123);
        assert_eq!(map_uid(&owners, 10_200), 10_200);
    }

    fn scan(s: &Stats, uid: i32) -> &DualTimer {
        s.uids[&uid].bluetooth_scan.as_ref().unwrap()
    }

    #[test]
    fn concurrent_scans_share_their_battery_time() {
        let mut s = stats();
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.note_bluetooth_scan_started(10_100, false, 1_000);
        s.note_bluetooth_scan_started(10_200, false, 3_000);
        s.note_bluetooth_scan_stopped(10_100, false, 5_000);
        s.note_bluetooth_scan_stopped(10_200, false, 9_000);
        // Alone 1 s to 3 s, then half of 3 s to 5 s; the other half and
        // alone after.
        assert_eq!(scan(&s, 10_100).timer.timer.total_time_us, 3_000_000);
        assert_eq!(scan(&s, 10_200).timer.timer.total_time_us, 5_000_000);
        // Durations are not pooled.
        assert_eq!(scan(&s, 10_100).timer.total_duration_ms, 4_000);
        assert_eq!(scan(&s, 10_200).timer.total_duration_ms, 6_000);
        assert_eq!(s.bluetooth_scan_timer.total_time_us, 8_000_000);
        assert!(s.bluetooth_scan_pool.is_empty());
        assert_eq!(s.bluetooth_scan_nesting, 0);
    }

    #[test]
    fn background_scan_time_follows_the_uids_process_state() {
        let mut s = stats();
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.note_uid_process_state(10_100, false, PROCESS_STATE_TOP, 0);
        s.note_bluetooth_scan_started(10_100, true, 1_000);
        s.note_bluetooth_scan_results(10_100, 5, 2_000);
        // Its last process goes: in the background from then on.
        s.note_uid_process_state(10_100, false, PROCESS_STATE_NONEXISTENT, 3_000);
        s.note_bluetooth_scan_results(10_100, 3, 4_000);
        s.note_bluetooth_scan_stopped(10_100, true, 7_000);
        let u = &s.uids[&10_100];
        let t = u.bluetooth_scan.as_ref().unwrap();
        assert_eq!(t.timer.timer.total_time_us, 6_000_000);
        assert_eq!(t.background.timer.total_time_us, 4_000_000);
        assert_eq!(t.background.total_duration_ms, 4_000);
        let unoptimized = u.bluetooth_unoptimized_scan.as_ref().unwrap();
        assert_eq!(unoptimized.timer.timer.total_time_us, 6_000_000);
        assert_eq!(unoptimized.background.timer.total_time_us, 4_000_000);
        assert_eq!(u.bluetooth_scan_results.as_ref().unwrap().count, 8);
        assert_eq!(
            u.bluetooth_scan_results_background.as_ref().unwrap().count,
            3
        );
        // Top from 0 s to 3 s, on the battery.
        let top = u.process_state_timers[UID_PROCESS_STATE_TOP]
            .as_ref()
            .unwrap();
        assert_eq!(top.total_time_us, 3_000_000);
    }

    #[test]
    fn plugging_in_stops_the_scan_times() {
        let mut s = stats();
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.note_bluetooth_scan_started(10_100, false, 1_000);
        s.note_bluetooth_scan_started(10_200, false, 1_000);
        s.set_battery_state(&C, b(2, 1, 50, -1), 5_000);
        s.note_bluetooth_scan_stopped(10_100, false, 8_000);
        s.note_bluetooth_scan_stopped(10_200, false, 9_000);
        // 4 s on the battery, shared by two.
        assert_eq!(scan(&s, 10_100).timer.timer.total_time_us, 2_000_000);
        assert_eq!(scan(&s, 10_200).timer.timer.total_time_us, 2_000_000);
        assert_eq!(scan(&s, 10_100).timer.total_duration_ms, 4_000);
        // Neither ever was in the background on the battery: a uid with
        // no process noted is, but its background base starts only with
        // a change.
        assert_eq!(scan(&s, 10_100).background.timer.total_time_us, 0);
    }

    #[test]
    fn a_reset_stops_every_scan_however_nested() {
        let dir = temp_dir("stats-scan-reset");
        let mut s = stats();
        s.history = Some(history(&dir, 128 * 1024, 0));
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.note_bluetooth_scan_started(10_100, true, 2_000);
        s.note_bluetooth_scan_started(10_100, true, 3_000);
        let items = decode(s.history.as_ref().unwrap().buffer());
        assert_ne!(
            items.last().unwrap().states2 & STATE2_BLUETOOTH_SCAN_FLAG,
            0
        );
        s.reset_bluetooth_scan(6_000);
        assert_eq!(s.bluetooth_scan_nesting, 0);
        assert!(s.bluetooth_scan_pool.is_empty());
        let u = &s.uids[&10_100];
        for t in [&u.bluetooth_scan, &u.bluetooth_unoptimized_scan] {
            let t = t.as_ref().unwrap();
            assert_eq!((t.timer.timer.nesting, t.background.timer.nesting), (0, 0));
            assert_eq!(t.timer.timer.total_time_us, 4_000_000);
        }
        assert_eq!(s.bluetooth_scan_timer.nesting, 0);
        let items = decode(s.history.as_ref().unwrap().buffer());
        assert_eq!(
            items.last().unwrap().states2 & STATE2_BLUETOOTH_SCAN_FLAG,
            0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_states_are_recorded_in_the_history() {
        let dir = temp_dir("stats-proc-state");
        let mut s = stats();
        s.history = Some(history(&dir, 128 * 1024, 0));
        s.set_battery_state(&C, b(3, PLUGGED_NONE, 50, -1), 0);
        s.note_uid_process_state(10_100, false, PROCESS_STATE_TOP, 5_000);
        s.note_uid_process_state(10_100, false, PROCESS_STATE_TOP, 6_000);
        s.note_uid_process_state(10_100, false, PROCESS_STATE_NONEXISTENT, 9_000);
        // An isolated uid's state is its owner's.
        s.note_uid_process_state(10_100, true, PROCESS_STATE_TOP, 12_000);
        let changes: Vec<_> = decode(s.history.as_ref().unwrap().buffer())
            .into_iter()
            .filter_map(|d| d.process_state_change)
            .collect();
        assert_eq!(
            changes,
            [
                (10_100, CONSUMER_PROCESS_STATE_FOREGROUND),
                (10_100, CONSUMER_PROCESS_STATE_UNSPECIFIED)
            ]
        );
        assert_eq!(s.uids[&10_100].process_state, UID_PROCESS_STATE_NONEXISTENT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_states_map_as_the_original_maps_them() {
        assert_eq!(internal_process_state(0), UID_PROCESS_STATE_FOREGROUND);
        assert_eq!(internal_process_state(3), UID_PROCESS_STATE_BACKGROUND);
        assert_eq!(
            internal_process_state(5),
            UID_PROCESS_STATE_FOREGROUND_SERVICE
        );
        assert_eq!(internal_process_state(10), UID_PROCESS_STATE_BACKGROUND);
        assert_eq!(internal_process_state(12), UID_PROCESS_STATE_TOP_SLEEPING);
        assert_eq!(internal_process_state(13), UID_PROCESS_STATE_HEAVY_WEIGHT);
        assert_eq!(internal_process_state(19), UID_PROCESS_STATE_CACHED);
        assert_eq!(
            consumer_process_state(UID_PROCESS_STATE_TOP_SLEEPING),
            CONSUMER_PROCESS_STATE_BACKGROUND
        );
        assert_eq!(
            consumer_process_state(UID_PROCESS_STATE_HEAVY_WEIGHT),
            CONSUMER_PROCESS_STATE_UNSPECIFIED
        );
    }

    #[test]
    fn a_work_sources_uids_and_chains_are_noted() {
        let ws = WorkSource {
            uids: vec![10_100, 99_001],
            names: None,
            chains: Some(vec![crate::location::parcels::WorkChain {
                uids: vec![10_300, 10_400],
                tags: vec![None, None],
            }]),
        };
        let map = |uid: i32| if uid == 99_001 { 10_200 } else { uid };
        assert_eq!(work_source_uids(&ws, &map), [10_100, 10_200, 10_300]);
        assert!(work_source(None).is_err());
    }
}
