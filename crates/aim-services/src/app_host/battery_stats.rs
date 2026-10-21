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
//! BatteryService feeds them. `takeUidSnapshotsAsync` answers its result
//! receiver as the original does (its BATTERY_STATS check), the snapshots
//! themselves failing as not implemented. The stats themselves (wake
//! locks, power use, history, `batterystats.bin`): #1513.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::com_android_internal_app_ibatterystats as bs;

use super::kept::RawIntent;
use super::{AppHost, reply};

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
}

/// The configuration the stats are decided by.
#[derive(Clone, Copy, Debug)]
struct Config {
    charged_delay_ms: i64,
    charging_enforce_level: i32,
    /// `BatteryStatsConfig`'s resets on unplugging.
    reset_high_level: bool,
    reset_significant_charge: bool,
}

impl Stats {
    /// `setBatteryStateLocked` at elapsed `now`: whether the charging
    /// state changed.
    fn set_battery_state(
        &mut self,
        c: &Config,
        status: i32,
        plug_type: i32,
        level: i32,
        time_to_full: i64,
        now: i64,
    ) -> bool {
        let on_battery = plug_type == PLUGGED_NONE && status != STATUS_UNKNOWN;
        let mut changed = false;
        if !self.have_level {
            self.have_level = true;
            self.status = status;
            self.level = level;
            (self.max_charge_step_level, self.min_discharge_step_level) = (level, level);
            (self.last_charge_step_level, self.last_discharge_step_level) = (level, level);
            self.init_step_mode = STEP_MODE_SCREEN_ON;
            // `mOnBattery` starts false: a first reading on battery is a
            // change below.
        }
        let old_status = self.status;
        if self.discharge_plug_level < 0 {
            self.discharge_plug_level = level;
        }
        if on_battery != self.on_battery {
            self.level = level;
            self.status = status;
            changed |= self.set_on_battery(c, on_battery, old_status, level);
        } else {
            self.level = level;
            self.status = status;
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
        }
        self.time_to_full_seconds = time_to_full;
        changed
    }

    /// `setOnBatteryLocked`'s charging and step trackers.
    fn set_on_battery(
        &mut self,
        c: &Config,
        on_battery: bool,
        old_status: i32,
        level: i32,
    ) -> bool {
        let mut changed = false;
        if on_battery {
            // `shouldResetOnUnplugLocked`.
            let reset = c.reset_high_level && (old_status == STATUS_FULL || level >= 90)
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
            self.discharge_unplug_level = level;
        } else {
            self.on_battery = false;
            self.discharge_plug_level = level;
            if level < self.discharge_unplug_level {
                self.high_discharge_since_charge += self.discharge_unplug_level - level;
            }
            self.charge_steps.init();
            self.last_charge_step_level = level;
            self.max_charge_step_level = level;
            (self.init_step_mode, self.mod_step_mode) = (STEP_MODE_SCREEN_ON, 0);
        }
        changed
    }

    /// `setChargingLocked`: whether it changed.
    fn set_charging(&mut self, charging: bool) -> bool {
        self.charging_due = None;
        std::mem::replace(&mut self.charging, charging) != charging
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
    /// `BatteryStatsConfig`'s resets, read once.
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
        let config = Config {
            charged_delay_ms: constant("battery_charged_delay_ms")
                .unwrap_or(DEFAULT_BATTERY_CHARGED_DELAY_MS),
            charging_enforce_level: constant("battery_charging_enforce_level")
                .map_or(DEFAULT_BATTERY_CHARGING_ENFORCE_LEVEL, |v| v as i32),
            reset_high_level: resource("config_batteryStatsResetOnUnplugHighBatteryLevel"),
            reset_significant_charge: resource(
                "config_batteryStatsResetOnUnplugAfterSignificantCharge",
            ),
        };
        Some(*self.config.get_or_init(|| config))
    }

    /// `setBatteryState`, as BatteryService calls it with each health
    /// info it processes.
    /// `isCharging` (`BatteryManager.isCharging`).
    pub(super) fn charging(&self) -> bool {
        self.stats.lock().unwrap().charging
    }

    pub(super) fn set_battery_state(
        self: &Arc<Self>,
        status: i32,
        plug_type: i32,
        level: i32,
        time_to_full: i64,
        now: i64,
    ) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let Some(config) = self.config(&host) else {
            return eprintln!("app host: batterystats: no configuration yet");
        };
        let (changed, due) = {
            let mut s = self.stats.lock().unwrap();
            (
                s.set_battery_state(&config, status, plug_type, level, time_to_full, now),
                s.charging_due,
            )
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

    /// `mDeferSetCharging`: charging, if still due and plugged in.
    fn deferred_charging(&self, due: i64) {
        let changed = {
            let mut s = self.stats.lock().unwrap();
            s.charging_due == Some(due) && !s.on_battery && s.set_charging(true)
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
}

impl Service for BatteryStats {
    fn descriptor(&self) -> &str {
        bs::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
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
            // `noteResetBluetoothScanLocked` stops the BLE scan timers that
            // run. None does: their starts (`noteBleScanStarted` and the
            // rest) are not taken (#1513), so the reset changes nothing.
            bs::NOTE_BLE_SCAN_RESET => {
                bs::NoteBleScanReset::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                drop(s);
                let Some(host) = self.host.upgrade() else {
                    return Err(BAD_VALUE);
                };
                let permitted =
                    host.check_permission(Some(UPDATE_DEVICE_STATS), call.sender_euid as i32);
                return reply(
                    permitted
                        .and_then(|held| match held {
                            0 => Ok(p),
                            _ => Err(Exception::security(format!(
                                "Access denied, requires: {UPDATE_DEVICE_STATS}"
                            ))),
                        })
                        .map(|mut p| {
                            bs::write_note_ble_scan_reset_reply(&mut p);
                            p
                        }),
                );
            }
            // One-way: what fails reaches the caller's receiver. Asking
            // for other uids than its own needs BATTERY_STATS
            // (`onlyCaller`); the snapshots (`getHealthStatsForUidLocked`)
            // are #1513, sent as the original sends a failure to make them.
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
                let e = host.unimplemented_named("batterystats", "a uid's health stats (#1513)");
                send_failure(&host, receiver, RESULT_RUNTIME_EXCEPTION, &e.message);
            }
            code => {
                drop(s);
                let Some(host) = self.host.upgrade() else {
                    return Err(BAD_VALUE);
                };
                return reply(Err(
                    host.unimplemented_named("batterystats", &format!("#{code} (#1513)"))
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
    };

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
        s.set_battery_state(&C, 3, PLUGGED_NONE, 50, -1, 0);
        assert!(s.on_battery && !s.charging);
        s.set_battery_state(&C, 2, 1, 91, -1, 1000);
        assert!(
            !s.charging,
            "the plug change itself only resets the trackers"
        );
        assert!(s.set_battery_state(&C, 2, 1, 92, -1, 2000));
        assert!(s.charging);
        assert!(s.set_battery_state(&C, 3, PLUGGED_NONE, 92, -1, 3000));
        assert!(!s.charging);
    }

    #[test]
    fn a_rising_level_below_it_charges_after_the_delay() {
        let mut s = stats();
        s.set_battery_state(&C, 2, 1, 40, -1, 0);
        s.set_battery_state(&C, 2, 1, 41, -1, 60_000);
        assert_eq!(s.charging_due, Some(960_000));
        assert!(!s.charging);
        // The level falling cancels it.
        s.set_battery_state(&C, 2, 1, 40, -1, 70_000);
        assert_eq!(s.charging_due, None);
    }

    #[test]
    fn charge_time_remaining_follows_the_steps() {
        let mut s = stats();
        s.set_battery_state(&C, 2, 1, 50, -1, 0);
        s.set_battery_state(&C, 2, 1, 51, -1, 60_000);
        assert_eq!(
            s.charge_time_remaining(),
            -1,
            "the first step only starts timing"
        );
        s.set_battery_state(&C, 2, 1, 53, -1, 180_000);
        // 120 s over two levels: 60 s a level, 47 levels to go.
        assert_eq!(s.charge_time_remaining(), 60_000 * 47 * 1000);
        s.set_battery_state(&C, 2, 1, 53, 600, 190_000);
        assert_eq!(
            s.charge_time_remaining(),
            600_000_000,
            "the HAL's time to full wins"
        );
    }
}
