//! `batteryproperties` and the battery's broadcasts: BatteryService for
//! the app host (`BatteryService`, `BatteryManager`,
//! `IBatteryPropertiesRegistrar` and `com.android.server.health` at
//! `android-16.0.0_r1`, ADR 0013). It is the health HAL's client as
//! `HealthServiceWrapperAidl` is (`IHealth/default`, `vendor.health-aim`
//! over the Mac's battery): an `IHealthInfoCallback` registered with it,
//! then `update`, again when a restarted HAL registers. Each health info
//! is processed as `processValuesLocked` processes it: the plug type, low
//! battery against `config_lowBatteryWarningLevel` (or the
//! `low_power_trigger_level` setting), `config_lowBatteryCloseWarningBump`
//! and `config_criticalBatteryWarningLevel`, read once at the start as at
//! `PHASE_ACTIVITY_MANAGER_READY`; the sticky `ACTION_BATTERY_CHANGED`
//! with its extras (rate limited as the original's flag says), and
//! `ACTION_POWER_CONNECTED`/`DISCONNECTED` and `ACTION_BATTERY_LOW`/`OKAY`
//! to every user. `getProperty` reads the HAL as the wrapper does, with
//! its permissions; `scheduleUpdate` asks the HAL to update. Battery
//! stats, `ACTION_BATTERY_LEVEL_CHANGED` (of battery stats' permission),
//! the shutdown on a critical or overheated battery, the LED and the
//! charging policy's listeners are not run, nor is the copy of
//! `ACTION_BATTERY_CHANGED` for SystemUI sent, which the app host does not
//! run. The `battery` service, which only dumps and takes shell commands,
//! is not registered.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_SERVICE_SPECIFIC, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_os_ibatterypropertiesregistrar as registrar;
use aim_service_aidl::WriteParcelable;

use super::display::parcelable;
use super::kept::RawIntent;
use super::settings::Table;
use super::{AppHost, reply};
use crate::bundle::Value;

/// The HAL, its interface and its callback's, with their codes.
const HEALTH: &str = "android.hardware.health.IHealth/default";
const HEALTH_DESCRIPTOR: &str = "android.hardware.health.IHealth";
const REGISTER_CALLBACK: u32 = 1;
const UPDATE: u32 = 3;
const GET_CHARGE_COUNTER_UAH: u32 = 4;
const GET_CURRENT_NOW_MICROAMPS: u32 = 5;
const GET_CURRENT_AVERAGE_MICROAMPS: u32 = 6;
const GET_CAPACITY: u32 = 7;
const GET_ENERGY_COUNTER_NWH: u32 = 8;
const GET_CHARGE_STATUS: u32 = 9;
const GET_CHARGING_POLICY: u32 = 14;
const GET_BATTERY_HEALTH_DATA: u32 = 15;
const CALLBACK_DESCRIPTOR: &str = "android.hardware.health.IHealthInfoCallback";
const HEALTH_INFO_CHANGED: u32 = 1;
/// How long the HAL may take to register (`waitForDeclaredService`).
const PATIENCE: Duration = Duration::from_secs(30);

/// `BatteryManager.BATTERY_STATUS_*`, `BATTERY_PLUGGED_*`.
const STATUS_UNKNOWN: i32 = 1;
const STATUS_CHARGING: i32 = 2;
const STATUS_DISCHARGING: i32 = 3;
const STATUS_NOT_CHARGING: i32 = 4;
const STATUS_FULL: i32 = 5;
const PLUGGED_NONE: i32 = 0;
const PLUGGED_AC: i32 = 1;
const PLUGGED_USB: i32 = 2;
const PLUGGED_WIRELESS: i32 = 4;
const PLUGGED_DOCK: i32 = 8;
const PLUGGED_ANY: i32 = PLUGGED_AC | PLUGGED_USB | PLUGGED_WIRELESS | PLUGGED_DOCK;
/// `BatteryManager.BATTERY_PROPERTY_*`.
const PROPERTY_CHARGE_COUNTER: i32 = 1;
const PROPERTY_CURRENT_NOW: i32 = 2;
const PROPERTY_CURRENT_AVERAGE: i32 = 3;
const PROPERTY_CAPACITY: i32 = 4;
const PROPERTY_ENERGY_COUNTER: i32 = 5;
const PROPERTY_STATUS: i32 = 6;
const PROPERTY_MANUFACTURING_DATE: i32 = 7;
const PROPERTY_FIRST_USAGE_DATE: i32 = 8;
const PROPERTY_CHARGING_POLICY: i32 = 9;
const PROPERTY_STATE_OF_HEALTH: i32 = 10;
const PROPERTY_SERIAL_NUMBER: i32 = 11;
const PROPERTY_PART_STATUS: i32 = 12;
/// `BATTERY_SCALE`: the level is a percentage.
const BATTERY_SCALE: i32 = 100;
/// The rate limit of `ACTION_BATTERY_CHANGED`.
const ABSOLUTE_DECI_CELSIUS_DIFF_FOR_TEMP_UPDATE: i32 = 10;
const TIME_DIFF_FOR_VOLTAGE_UPDATE_MS: i64 = 20_000;
const BASE_POINT_DIFF_FOR_VOLTAGE_UPDATE: f32 = 0.01;
/// The broadcasts and their flags.
const ACTION_BATTERY_CHANGED: &str = "android.intent.action.BATTERY_CHANGED";
const ACTION_POWER_CONNECTED: &str = "android.intent.action.ACTION_POWER_CONNECTED";
const ACTION_POWER_DISCONNECTED: &str = "android.intent.action.ACTION_POWER_DISCONNECTED";
const ACTION_BATTERY_LOW: &str = "android.intent.action.BATTERY_LOW";
const ACTION_BATTERY_OKAY: &str = "android.intent.action.BATTERY_OKAY";
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const FLAG_RECEIVER_REPLACE_PENDING: i32 = 0x2000_0000;
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
/// The aconfig flags it follows.
const RATE_LIMIT_BATTERY_CHANGED: &str = "com.android.server.flags.rate_limit_battery_changed_broadcast";
const STATE_OF_HEALTH_PUBLIC: &str = "android.os.state_of_health_public";
const BATTERY_PART_STATUS_API: &str = "android.os.battery_part_status_api";
const BATTERY_STATS: &str = "android.permission.BATTERY_STATS";

/// `android.hardware.health.HealthInfo` (V4), as far as BatteryService
/// reads it.
#[derive(Clone, Debug, Default, PartialEq)]
struct HealthInfo {
    charger_ac_online: bool,
    charger_usb_online: bool,
    charger_wireless_online: bool,
    charger_dock_online: bool,
    max_charging_current: i32,
    max_charging_voltage: i32,
    status: i32,
    health: i32,
    present: bool,
    level: i32,
    voltage: i32,
    temperature: i32,
    cycle_count: i32,
    charge_counter: i32,
    technology: Option<String>,
    capacity_level: i32,
    charge_time_to_full: i64,
    charging_state: i32,
}

/// A nullable structured parcelable as the HAL's backend writes it: 1,
/// then its size and fields.
fn nullable<T>(r: &mut Reader<'_>, read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>) -> ParcelResult<Option<T>> {
    match r.read_i32()? {
        0 => Ok(None),
        _ => parcelable(r, read).map(Some),
    }
}

/// An array of structured parcelables, read past.
fn skip_parcelables(r: &mut Reader<'_>) -> ParcelResult<()> {
    for _ in 0..r.read_i32()?.max(0) {
        nullable(r, |_| Ok(()))?;
    }
    Ok(())
}

impl HealthInfo {
    /// Its fields in their order; the battery's health data and the
    /// hinges after them are skipped by its size ([`nullable`]).
    fn read(r: &mut Reader<'_>) -> ParcelResult<HealthInfo> {
        let mut info = HealthInfo {
            charger_ac_online: r.read_bool()?,
            charger_usb_online: r.read_bool()?,
            charger_wireless_online: r.read_bool()?,
            charger_dock_online: r.read_bool()?,
            max_charging_current: r.read_i32()?,
            max_charging_voltage: r.read_i32()?,
            status: r.read_i32()?,
            health: r.read_i32()?,
            present: r.read_bool()?,
            level: r.read_i32()?,
            voltage: r.read_i32()?,
            temperature: r.read_i32()?,
            ..HealthInfo::default()
        };
        r.read_i32()?; // current
        info.cycle_count = r.read_i32()?;
        r.read_i32()?; // full charge
        info.charge_counter = r.read_i32()?;
        info.technology = r.read_string16()?;
        r.read_i32()?; // average current
        skip_parcelables(r)?; // disk stats
        skip_parcelables(r)?; // storage infos
        info.capacity_level = r.read_i32()?;
        info.charge_time_to_full = r.read_i64()?;
        r.read_i32()?; // full charge design capacity
        info.charging_state = r.read_i32()?;
        Ok(info)
    }

    /// `plugType`.
    fn plug_type(&self) -> i32 {
        if self.charger_ac_online {
            PLUGGED_AC
        } else if self.charger_usb_online {
            PLUGGED_USB
        } else if self.charger_wireless_online {
            PLUGGED_WIRELESS
        } else if self.charger_dock_online {
            PLUGGED_DOCK
        } else {
            PLUGGED_NONE
        }
    }

    /// `isPoweredLocked`: an unknown state counts as powered.
    fn is_powered(&self, plug_types: i32) -> bool {
        self.status == STATUS_UNKNOWN
            || plug_types & PLUGGED_AC != 0 && self.charger_ac_online
            || plug_types & PLUGGED_USB != 0 && self.charger_usb_online
            || plug_types & PLUGGED_WIRELESS != 0 && self.charger_wireless_online
            || plug_types & PLUGGED_DOCK != 0 && self.charger_dock_online
    }
}

/// `android.hardware.health.BatteryHealthData`.
struct HealthData {
    manufacturing_date: i64,
    first_usage: i64,
    state_of_health: i64,
    serial_number: Option<String>,
    part_status: i32,
}

impl HealthData {
    fn read(r: &mut Reader<'_>) -> ParcelResult<HealthData> {
        Ok(HealthData {
            manufacturing_date: r.read_i64()?,
            first_usage: r.read_i64()?,
            state_of_health: r.read_i64()?,
            serial_number: r.read_string16()?,
            part_status: r.read_i32()?,
        })
    }
}

/// `android.os.BatteryProperty`: unset until the HAL answers.
struct BatteryProperty {
    long: i64,
    string: Option<String>,
}

impl WriteParcelable for BatteryProperty {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.long);
        p.write_string8(self.string.as_deref());
    }
}

/// What BatteryService reads from the framework, the settings and the
/// flags at its start.
#[derive(Default)]
struct Config {
    low_level: i32,
    /// `mLastLowBatteryWarningLevel`.
    last_low_level: i32,
    close_level: i32,
    rate_limit: bool,
    state_of_health_public: bool,
    part_status_api: bool,
    /// `R.drawable.stat_sys_battery_charge`, `stat_sys_battery`,
    /// `stat_sys_battery_unknown`.
    icons: [i32; 3],
}

/// The values `ACTION_BATTERY_CHANGED` was last sent with.
#[derive(Default)]
struct Sent {
    status: i32,
    health: i32,
    present: bool,
    level: i32,
    plug_type: i32,
    voltage: i32,
    temperature: i32,
    max_charging_current: i32,
    max_charging_voltage: i32,
    charge_counter: i32,
    cycle_count: i32,
    charging_state: i32,
    capacity_level: i32,
}

/// A broadcast to every user: sticky, or sent.
#[derive(Debug, PartialEq)]
struct Outgoing {
    action: &'static str,
    flags: i32,
    extras: Vec<(&'static str, Value)>,
    sticky: bool,
}

struct State {
    config: Config,
    info: Option<HealthInfo>,
    plug_type: i32,
    level_low: bool,
    sent_low: bool,
    sequence: i32,
    last: Sent,
    first_update: bool,
    /// When a voltage change was last sent.
    voltage_update_time: i64,
}

impl Default for State {
    fn default() -> State {
        State {
            config: Config::default(),
            info: None,
            plug_type: PLUGGED_NONE,
            level_low: false,
            sent_low: false,
            sequence: 1,
            // The first run, which no plug type was sent before.
            last: Sent { plug_type: -1, ..Sent::default() },
            first_update: true,
            voltage_update_time: 0,
        }
    }
}

impl State {
    /// `processValuesLocked` at elapsed time `now`: the broadcasts it sends.
    fn process(&mut self, force: bool, now: i64) -> Vec<Outgoing> {
        let Some(info) = self.info.clone() else {
            return Vec::new();
        };
        let c = &self.config;
        self.plug_type = info.plug_type();
        let include_charge_counter = !c.rate_limit && info.charge_counter != self.last.charge_counter;
        let changed = force
            || info.status != self.last.status
            || info.health != self.last.health
            || info.present != self.last.present
            || info.level != self.last.level
            || self.plug_type != self.last.plug_type
            || info.voltage != self.last.voltage
            || info.temperature != self.last.temperature
            || info.max_charging_current != self.last.max_charging_current
            || info.max_charging_voltage != self.last.max_charging_voltage
            || include_charge_counter
            || info.cycle_count != self.last.cycle_count
            || info.charging_state != self.last.charging_state
            || info.capacity_level != self.last.capacity_level;
        if !changed {
            return Vec::new();
        }
        let unplugged = self.plug_type == PLUGGED_NONE;
        if !self.level_low {
            self.level_low = unplugged && info.status != STATUS_UNKNOWN && info.level <= c.low_level;
        } else if !unplugged || info.level >= c.close_level || force && info.level >= c.low_level {
            self.level_low = false;
        }
        self.sequence += 1;
        let mut out = Vec::new();
        let status = |action| Outgoing {
            action,
            flags: FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT,
            extras: vec![("seq", Value::Int(self.sequence))],
            sticky: false,
        };
        if self.plug_type != 0 && self.last.plug_type == 0 {
            out.push(status(ACTION_POWER_CONNECTED));
        } else if self.plug_type == 0 && self.last.plug_type != 0 {
            out.push(status(ACTION_POWER_DISCONNECTED));
        }
        // `shouldSendBatteryLowLocked`: just unplugged at or below the
        // warning level, or falling to it.
        let send_low = unplugged
            && info.status != STATUS_UNKNOWN
            && info.level <= c.low_level
            && (self.last.plug_type != PLUGGED_NONE || self.last.level > c.low_level || info.level > c.last_low_level);
        if send_low {
            self.sent_low = true;
            out.push(status(ACTION_BATTERY_LOW));
        } else if self.sent_low && info.level >= c.close_level {
            self.sent_low = false;
            out.push(status(ACTION_BATTERY_OKAY));
        }
        let limited = self.rate_limited(&info, force, now);
        if !limited {
            out.push(Outgoing {
                action: ACTION_BATTERY_CHANGED,
                flags: FLAG_RECEIVER_REGISTERED_ONLY | FLAG_RECEIVER_REPLACE_PENDING,
                extras: self.battery_changed_extras(&info),
                sticky: true,
            });
            self.last = Sent {
                status: info.status,
                health: info.health,
                present: info.present,
                level: info.level,
                plug_type: self.plug_type,
                voltage: info.voltage,
                temperature: info.temperature,
                max_charging_current: info.max_charging_current,
                max_charging_voltage: info.max_charging_voltage,
                charge_counter: info.charge_counter,
                cycle_count: info.cycle_count,
                charging_state: info.charging_state,
                capacity_level: info.capacity_level,
            };
        }
        out
    }

    /// `rateLimitBatteryChangedBroadcast`: a change of only the voltage or
    /// the temperature waits until it is large enough.
    fn rate_limited(&mut self, info: &HealthInfo, force: bool, now: i64) -> bool {
        if !self.config.rate_limit {
            return false;
        }
        if self.first_update {
            self.voltage_update_time = now;
            self.first_update = false;
            return false;
        }
        let last = &self.last;
        let voltage_updated = last.voltage != info.voltage;
        let temperature_updated = last.temperature != info.temperature;
        let others_updated = force
            || info.status != last.status
            || info.health != last.health
            || info.present != last.present
            || info.level != last.level
            || self.plug_type != last.plug_type
            || info.max_charging_voltage != last.max_charging_voltage
            || info.cycle_count != last.cycle_count
            || info.charging_state != last.charging_state
            || info.capacity_level != last.capacity_level;
        let base_point_diff = (last.voltage - info.voltage) as f32 / last.voltage as f32;
        let send = others_updated
            || voltage_updated
                && base_point_diff.abs() >= BASE_POINT_DIFF_FOR_VOLTAGE_UPDATE
                && now - self.voltage_update_time >= TIME_DIFF_FOR_VOLTAGE_UPDATE_MS
            || temperature_updated
                && (last.temperature - info.temperature).abs() >= ABSOLUTE_DECI_CELSIUS_DIFF_FOR_TEMP_UPDATE;
        if send && voltage_updated {
            self.voltage_update_time = now;
        }
        !send
    }

    /// `getIconLocked`.
    fn icon(&self, info: &HealthInfo) -> i32 {
        let [charge, discharge, unknown] = self.config.icons;
        match info.status {
            STATUS_CHARGING => charge,
            STATUS_DISCHARGING => discharge,
            STATUS_NOT_CHARGING | STATUS_FULL if info.is_powered(PLUGGED_ANY) && info.level >= 100 => charge,
            STATUS_NOT_CHARGING | STATUS_FULL => discharge,
            _ => unknown,
        }
    }

    /// `sendBatteryChangedIntentLocked`'s extras, in their order.
    fn battery_changed_extras(&self, info: &HealthInfo) -> Vec<(&'static str, Value)> {
        vec![
            ("seq", Value::Int(self.sequence)),
            ("status", Value::Int(info.status)),
            ("health", Value::Int(info.health)),
            ("present", Value::Bool(info.present)),
            ("level", Value::Int(info.level)),
            ("battery_low", Value::Bool(self.sent_low)),
            ("scale", Value::Int(BATTERY_SCALE)),
            ("icon-small", Value::Int(self.icon(info))),
            ("plugged", Value::Int(self.plug_type)),
            ("voltage", Value::Int(info.voltage)),
            ("temperature", Value::Int(info.temperature)),
            ("technology", Value::String(info.technology.clone())),
            // The Mac has no invalid charger switch.
            ("invalid_charger", Value::Int(0)),
            ("max_charging_current", Value::Int(info.max_charging_current)),
            ("max_charging_voltage", Value::Int(info.max_charging_voltage)),
            ("charge_counter", Value::Int(info.charge_counter)),
            ("android.os.extra.CYCLE_COUNT", Value::Int(info.cycle_count)),
            ("android.os.extra.CHARGING_STATUS", Value::Int(info.charging_state)),
            ("android.os.extra.CAPACITY_LEVEL", Value::Int(info.capacity_level)),
        ]
    }
}

/// How a call of the HAL failed, as the wrapper tells them apart.
enum HalError {
    Unsupported,
    ServiceSpecific,
    Other(Exception),
}

impl HalError {
    fn message(self) -> String {
        match self {
            HalError::Unsupported => "unsupported".into(),
            HalError::ServiceSpecific => "a service-specific error".into(),
            HalError::Other(e) => e.message,
        }
    }
}

/// A call of the HAL and its reply's status, then what `read` reads.
fn hal_call<T>(
    hal: &Strong,
    code: u32,
    write: impl FnOnce(&mut Parcel),
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<T>,
) -> Result<T, HalError> {
    let failed = |s| HalError::Other(Exception::new(EX_ILLEGAL_STATE, format!("health HAL#{code}: status {s}")));
    let mut data = Parcel::new();
    data.write_interface_token(HEALTH_DESCRIPTOR);
    write(&mut data);
    let reply = hal.transact(code, &data, false).map_err(failed)?;
    let mut r = reply.reader();
    match r.read_exception().map_err(failed)? {
        Ok(()) => read(&mut r).map_err(failed),
        Err(e) if e.code == EX_UNSUPPORTED_OPERATION => Err(HalError::Unsupported),
        Err(e) if e.code == EX_SERVICE_SPECIFIC => Err(HalError::ServiceSpecific),
        Err(e) => Err(HalError::Other(e)),
    }
}

pub struct BatteryService {
    host: Arc<AppHost>,
    state: Mutex<State>,
    /// The HAL (`mLastService`), and the callback registered with it.
    health: Mutex<Option<Arc<Strong>>>,
    callback: OnceLock<Binder>,
}

impl BatteryService {
    pub fn new(host: Arc<AppHost>) -> BatteryService {
        BatteryService { host, state: Mutex::default(), health: Mutex::new(None), callback: OnceLock::new() }
    }

    /// `BatteryManagerInternal.isPowered(BATTERY_PLUGGED_ANY)` and
    /// `getBatteryLevelLow`: as the service starts (neither) until the HAL
    /// first reports.
    pub(super) fn powered_and_low(&self) -> (bool, bool) {
        let s = self.state.lock().unwrap();
        (s.info.as_ref().is_some_and(|i| i.is_powered(PLUGGED_ANY)), s.level_low)
    }

    /// The constructor's and `updateBatteryWarningLevelLocked`'s levels,
    /// then the HAL.
    pub(super) fn start(self: &Arc<Self>) -> Result<(), String> {
        let inputs = self.host.inputs().map_err(|e| e.message)?;
        let (model, _) = self.host.packages().map_err(|e| e.message)?;
        let flag = |name: &str| model.system.flags.iter().any(|(n, on)| n == name && *on);
        let integer = |name: &str| inputs.packages.framework_integer(name).ok_or(format!("no integer {name}"));
        let icon = |name: &str| inputs.packages.framework_id("drawable", name).map(|id| id as i32).ok_or(format!("no drawable {name}"));
        let critical_level = integer("config_criticalBatteryWarningLevel")?;
        let default_low = integer("config_lowBatteryWarningLevel")?;
        let bump = integer("config_lowBatteryCloseWarningBump")?;
        let setting = inputs.settings.get(Table::Global, "low_power_trigger_level").and_then(|v| v.parse().ok());
        let low_level = match setting.unwrap_or(default_low) {
            0 => default_low,
            level => level,
        }
        .max(critical_level);
        self.state.lock().unwrap().config = Config {
            low_level,
            last_low_level: default_low,
            close_level: low_level + bump,
            rate_limit: flag(RATE_LIMIT_BATTERY_CHANGED),
            state_of_health_public: flag(STATE_OF_HEALTH_PUBLIC),
            part_status_api: flag(BATTERY_PART_STATUS_API),
            icons: [icon("stat_sys_battery_charge")?, icon("stat_sys_battery")?, icon("stat_sys_battery_unknown")?],
        };
        self.connect()
    }

    /// `HealthServiceWrapperAidl`: the HAL once it registers, the callback
    /// registered with it and an update asked (`HealthRegCallbackAidl`),
    /// and the same with the HAL that registers after it dies.
    fn connect(self: &Arc<Self>) -> Result<(), String> {
        let deadline = Instant::now() + PATIENCE;
        let hal = Arc::new(loop {
            if let Ok(hal) = super::display::check_service(&self.host.process, HEALTH) {
                break hal;
            }
            if Instant::now() > deadline {
                return Err(format!("{HEALTH} did not register"));
            }
            std::thread::sleep(Duration::from_millis(100));
        });
        *self.health.lock().unwrap() = Some(hal.clone());
        let me = Arc::downgrade(self);
        self.host.process.link_to_death(
            &hal,
            Box::new(move || {
                std::thread::spawn(move || {
                    if let Some(battery) = me.upgrade()
                        && let Err(e) = battery.connect()
                    {
                        eprintln!("app host: battery: {e}");
                    }
                });
            }),
        );
        let callback = *self
            .callback
            .get_or_init(|| self.host.process.add_service(Arc::new(HealthInfoCallback(Arc::downgrade(self)))));
        hal_call(&hal, REGISTER_CALLBACK, |p| p.write_vintf_binder(callback), |_| Ok(()))
            .map_err(|e| format!("health: cannot register callback: {}", e.message()))?;
        // `registerCallback` does not promise a first callback.
        hal_call(&hal, UPDATE, |_| {}, |_| Ok(())).map_err(|e| format!("health: cannot update: {}", e.message()))
    }

    /// `update`: the HAL's new health info, processed, and its
    /// broadcasts sent in order.
    fn update(&self, info: HealthInfo) {
        let now = super::clock_ms(libc::CLOCK_MONOTONIC_RAW);
        // `processValuesLocked` first lets the battery stats keep track of
        // the level (`IBatteryStats.setBatteryState`).
        if let Some(stats) = self.host.battery_stats.get() {
            stats.set_battery_state(info.status, info.plug_type(), info.level, info.charge_time_to_full, now);
        }
        let mut state = self.state.lock().unwrap();
        state.info = Some(info);
        for o in state.process(false, now) {
            let intent = RawIntent::new(o.action, o.flags, &o.extras);
            if o.sticky {
                self.host.broadcasts.send_sticky_from_system(&self.host, intent);
            } else {
                self.host.broadcasts.send_from_system(&self.host, intent);
            }
        }
    }

    /// `BatteryPropertiesRegistrar.getProperty`, then
    /// `HealthServiceWrapperAidl.getProperty`: 0, or -1 when the HAL does
    /// not support it, -2 on its error, the property untouched.
    fn get_property(&self, uid: i32, id: i32) -> Result<(i32, BatteryProperty), Exception> {
        let (state_of_health_public, part_status_api) = {
            let c = &self.state.lock().unwrap().config;
            (c.state_of_health_public, c.part_status_api)
        };
        let restricted = match id {
            PROPERTY_STATE_OF_HEALTH => !state_of_health_public,
            PROPERTY_MANUFACTURING_DATE | PROPERTY_FIRST_USAGE_DATE | PROPERTY_CHARGING_POLICY | PROPERTY_SERIAL_NUMBER | PROPERTY_PART_STATUS => true,
            _ => false,
        };
        if restricted && self.host.check_permission(Some(BATTERY_STATS), uid)? != 0 {
            return Err(Exception::security(format!("uid {uid} does not have {BATTERY_STATS}.")));
        }
        let hal = self.health.lock().unwrap().clone().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no health service"))?;
        let mut prop = BatteryProperty { long: i64::MIN, string: None };
        let int = |code| hal_call(&hal, code, |_| {}, |r| r.read_i32().map(i64::from));
        let data = || hal_call(&hal, GET_BATTERY_HEALTH_DATA, |_| {}, |r| nullable(r, HealthData::read)?.ok_or(BAD_VALUE));
        let read = match id {
            PROPERTY_CHARGE_COUNTER => int(GET_CHARGE_COUNTER_UAH).map(|v| prop.long = v),
            PROPERTY_CURRENT_NOW => int(GET_CURRENT_NOW_MICROAMPS).map(|v| prop.long = v),
            PROPERTY_CURRENT_AVERAGE => int(GET_CURRENT_AVERAGE_MICROAMPS).map(|v| prop.long = v),
            PROPERTY_CAPACITY => int(GET_CAPACITY).map(|v| prop.long = v),
            PROPERTY_STATUS => int(GET_CHARGE_STATUS).map(|v| prop.long = v),
            PROPERTY_ENERGY_COUNTER => hal_call(&hal, GET_ENERGY_COUNTER_NWH, |_| {}, |r| r.read_i64()).map(|v| prop.long = v),
            PROPERTY_MANUFACTURING_DATE => data().map(|d| prop.long = d.manufacturing_date),
            PROPERTY_FIRST_USAGE_DATE => data().map(|d| prop.long = d.first_usage),
            PROPERTY_CHARGING_POLICY => int(GET_CHARGING_POLICY).map(|v| prop.long = v),
            PROPERTY_STATE_OF_HEALTH => data().map(|d| prop.long = d.state_of_health),
            PROPERTY_SERIAL_NUMBER if part_status_api => data().map(|d| prop.string = d.serial_number),
            PROPERTY_PART_STATUS if part_status_api => data().map(|d| prop.long = i64::from(d.part_status)),
            _ => Ok(()),
        };
        match read {
            Ok(()) => Ok((0, prop)),
            Err(HalError::Unsupported) => Ok((-1, prop)),
            Err(HalError::ServiceSpecific) => Ok((-2, prop)),
            Err(HalError::Other(e)) => Err(e),
        }
    }
}

/// `HealthRegCallbackAidl.HalInfoCallback`: the HAL's health info.
struct HealthInfoCallback(Weak<BatteryService>);

impl Service for HealthInfoCallback {
    fn descriptor(&self) -> &str {
        CALLBACK_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(battery) = self.0.upgrade() else {
            return Ok(Parcel::new());
        };
        if call.code != HEALTH_INFO_CHANGED {
            return reply(Err(battery.host.unimplemented("health callback", call.code)));
        }
        call.data.enforce_interface(CALLBACK_DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let info = nullable(&mut call.data, HealthInfo::read).map_err(|_| BAD_VALUE)?.ok_or(BAD_VALUE)?;
        battery.update(info);
        Ok(Parcel::new())
    }
}

/// `batteryproperties`.
pub struct BatteryProperties(pub Arc<BatteryService>);

impl Service for BatteryProperties {
    fn descriptor(&self) -> &str {
        registrar::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            registrar::GET_PROPERTY => {
                let a = registrar::GetProperty::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.get_property(call.sender_euid as i32, a.id).map(|(result, prop)| {
                    let mut p = Parcel::new();
                    registrar::write_get_property_reply(&mut p, &registrar::GetPropertyReply { result, prop: Some(prop) });
                    p
                }))
            }
            registrar::SCHEDULE_UPDATE => {
                registrar::ScheduleUpdate::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let hal = self.0.health.lock().unwrap().clone();
                match hal.map(|hal| hal_call(&hal, UPDATE, |_| {}, |_| Ok(()))) {
                    None => eprintln!("app host: battery: no health service"),
                    Some(Err(e)) => eprintln!("app host: battery: cannot call update on health AIDL HAL: {}", e.message()),
                    Some(Ok(())) => {}
                }
                Ok(Parcel::new())
            }
            code => reply(Err(self.0.host.unimplemented("batteryproperties", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structured parcelable as the HAL's backend writes it.
    fn structured(p: &mut Parcel, fields: impl FnOnce(&mut Parcel)) {
        p.write_i32(1);
        let start = p.position();
        p.write_i32(0);
        fields(p);
        p.set_i32_at(start, (p.position() - start) as i32);
    }

    /// `HealthInfo` as the HAL fills it from the Mac's battery: AC online,
    /// charging at `level`, one disk stat and no health data.
    fn written(level: i32, ac: bool) -> Parcel {
        let mut p = Parcel::new();
        structured(&mut p, |p| {
            p.write_bool(ac);
            p.write_bool(false); // usb
            p.write_bool(false); // wireless
            p.write_bool(false); // dock
            p.write_i32(0); // max charging current
            p.write_i32(0); // max charging voltage
            p.write_i32(if ac { STATUS_CHARGING } else { STATUS_DISCHARGING });
            p.write_i32(2); // good
            p.write_bool(true);
            p.write_i32(level);
            p.write_i32(12_600);
            p.write_i32(301);
            p.write_i32(-1_500_000); // current
            p.write_i32(87);
            p.write_i32(4_400_000); // full charge
            p.write_i32(3_960_000);
            p.write_string16(Some("Li-ion"));
            p.write_i32(-1_500_000); // average current
            p.write_i32(1);
            structured(p, |p| {
                for _ in 0..10 {
                    p.write_i64(7);
                }
            });
            p.write_i32(0); // storage infos
            p.write_i32(-1); // capacity level: unsupported
            p.write_i64(3600); // time to full
            p.write_i32(5_000_000); // full charge design capacity
            p.write_i32(1); // charging state: normal
            p.write_i32(1); // charging policy: default
            p.write_i32(0); // no health data
            p.write_i32(-1); // no hinges
        });
        p
    }

    fn info(level: i32, ac: bool) -> HealthInfo {
        let p = written(level, ac);
        nullable(&mut p.reader(), HealthInfo::read).unwrap().unwrap()
    }

    fn state() -> State {
        State {
            config: Config {
                low_level: 15,
                last_low_level: 15,
                close_level: 20,
                icons: [11, 12, 13],
                ..Config::default()
            },
            ..State::default()
        }
    }

    /// The actions `info` makes `state` send.
    fn actions(state: &mut State, info: HealthInfo) -> Vec<&'static str> {
        state.info = Some(info);
        state.process(false, 0).into_iter().map(|o| o.action).collect()
    }

    #[test]
    fn health_info_reads_as_the_hal_writes_it() {
        let p = written(80, true);
        let mut r = p.reader();
        let info = nullable(&mut r, HealthInfo::read).unwrap().unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(
            info,
            HealthInfo {
                charger_ac_online: true,
                status: STATUS_CHARGING,
                health: 2,
                present: true,
                level: 80,
                voltage: 12_600,
                temperature: 301,
                cycle_count: 87,
                charge_counter: 3_960_000,
                technology: Some("Li-ion".into()),
                capacity_level: -1,
                charge_time_to_full: 3600,
                charging_state: 1,
                ..HealthInfo::default()
            }
        );
        assert_eq!(info.plug_type(), PLUGGED_AC);
    }

    #[test]
    fn plug_types_go_by_the_first_charger_online() {
        let usb_and_dock = HealthInfo { charger_usb_online: true, charger_dock_online: true, status: STATUS_CHARGING, ..HealthInfo::default() };
        assert_eq!(usb_and_dock.plug_type(), PLUGGED_USB);
        assert!(usb_and_dock.is_powered(PLUGGED_DOCK));
        assert!(!usb_and_dock.is_powered(PLUGGED_AC));
        assert_eq!(HealthInfo { status: STATUS_DISCHARGING, ..HealthInfo::default() }.plug_type(), PLUGGED_NONE);
        // An unknown state counts as powered.
        assert!(HealthInfo { status: STATUS_UNKNOWN, ..HealthInfo::default() }.is_powered(PLUGGED_AC));
    }

    /// Unplugging, falling to the warning level, plugging in and rising
    /// past the close level.
    #[test]
    fn power_and_low_battery_broadcasts() {
        let mut s = state();
        // The first run on the charger: no plug type was sent before.
        assert_eq!(actions(&mut s, info(30, true)), [ACTION_BATTERY_CHANGED]);
        assert_eq!(actions(&mut s, info(30, true)), Vec::<&str>::new());
        assert_eq!(actions(&mut s, info(16, false)), [ACTION_POWER_DISCONNECTED, ACTION_BATTERY_CHANGED]);
        assert_eq!(actions(&mut s, info(15, false)), [ACTION_BATTERY_LOW, ACTION_BATTERY_CHANGED]);
        assert!(s.level_low && s.sent_low);
        // Still low: no second warning.
        assert_eq!(actions(&mut s, info(14, false)), [ACTION_BATTERY_CHANGED]);
        assert_eq!(actions(&mut s, info(14, true)), [ACTION_POWER_CONNECTED, ACTION_BATTERY_CHANGED]);
        assert!(!s.level_low && s.sent_low);
        assert_eq!(actions(&mut s, info(19, true)), [ACTION_BATTERY_CHANGED]);
        assert_eq!(actions(&mut s, info(20, true)), [ACTION_BATTERY_OKAY, ACTION_BATTERY_CHANGED]);
        assert!(!s.sent_low);
    }

    /// The first run unplugged, already low: disconnected and low.
    #[test]
    fn a_first_run_on_a_low_battery_warns() {
        let mut s = state();
        assert_eq!(actions(&mut s, info(10, false)), [ACTION_POWER_DISCONNECTED, ACTION_BATTERY_LOW, ACTION_BATTERY_CHANGED]);
    }

    #[test]
    fn battery_changed_is_sticky_with_every_extra() {
        let mut s = state();
        s.info = Some(info(80, true));
        let out = s.process(false, 0);
        let changed = out.last().unwrap();
        assert!(changed.sticky);
        assert_eq!(changed.flags, FLAG_RECEIVER_REGISTERED_ONLY | FLAG_RECEIVER_REPLACE_PENDING);
        assert_eq!(
            changed.extras,
            [
                ("seq", Value::Int(2)),
                ("status", Value::Int(STATUS_CHARGING)),
                ("health", Value::Int(2)),
                ("present", Value::Bool(true)),
                ("level", Value::Int(80)),
                ("battery_low", Value::Bool(false)),
                ("scale", Value::Int(100)),
                ("icon-small", Value::Int(11)),
                ("plugged", Value::Int(PLUGGED_AC)),
                ("voltage", Value::Int(12_600)),
                ("temperature", Value::Int(301)),
                ("technology", Value::String(Some("Li-ion".into()))),
                ("invalid_charger", Value::Int(0)),
                ("max_charging_current", Value::Int(0)),
                ("max_charging_voltage", Value::Int(0)),
                ("charge_counter", Value::Int(3_960_000)),
                ("android.os.extra.CYCLE_COUNT", Value::Int(87)),
                ("android.os.extra.CHARGING_STATUS", Value::Int(1)),
                ("android.os.extra.CAPACITY_LEVEL", Value::Int(-1)),
            ]
        );
        // A power broadcast carries the sequence of the change it comes with.
        s.info = Some(info(80, false));
        let out = s.process(false, 0);
        assert_eq!(out[0].action, ACTION_POWER_DISCONNECTED);
        assert_eq!(out[0].extras, [("seq", Value::Int(3))]);
        assert_eq!(out[0].flags, FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT);
        assert_eq!(out[1].extras[0], ("seq", Value::Int(3)));
        // Full on the charger shows the charging icon.
        let full = HealthInfo { status: STATUS_FULL, level: 100, charger_ac_online: true, ..HealthInfo::default() };
        assert_eq!(s.icon(&full), 11);
        assert_eq!(s.icon(&HealthInfo { status: STATUS_FULL, level: 100, ..HealthInfo::default() }), 12);
        assert_eq!(s.icon(&HealthInfo { status: STATUS_UNKNOWN, ..HealthInfo::default() }), 13);
    }

    /// With the rate limit, a small voltage or temperature change waits.
    #[test]
    fn small_voltage_and_temperature_changes_are_rate_limited() {
        let mut s = state();
        s.config.rate_limit = true;
        assert_eq!(actions(&mut s, info(80, true)).last(), Some(&ACTION_BATTERY_CHANGED));
        let warmer = HealthInfo { temperature: 305, ..info(80, true) };
        assert_eq!(actions(&mut s, warmer.clone()), Vec::<&str>::new());
        let hot = HealthInfo { temperature: 311, ..warmer };
        assert_eq!(actions(&mut s, hot.clone()), [ACTION_BATTERY_CHANGED]);
        let lower = HealthInfo { voltage: 12_000, ..hot };
        s.info = Some(lower.clone());
        assert!(s.process(false, 1_000).is_empty());
        assert_eq!(s.process(false, TIME_DIFF_FOR_VOLTAGE_UPDATE_MS).len(), 1);
    }

    /// A property's reply as BatteryManager's proxy reads it: the result,
    /// then the property, unset.
    #[test]
    fn battery_property_is_written_as_java_reads_it() {
        let mut p = Parcel::new();
        registrar::write_get_property_reply(&mut p, &registrar::GetPropertyReply { result: -1, prop: Some(BatteryProperty { long: i64::MIN, string: None }) });
        let mut r = p.reader();
        assert_eq!(r.read_exception(), Ok(Ok(())));
        assert_eq!(r.read_i32(), Ok(-1));
        assert_eq!(r.read_i32(), Ok(1));
        assert_eq!(r.read_i64(), Ok(i64::MIN));
        assert_eq!(r.read_string8(), Ok(None));
        assert_eq!(r.remaining(), 0);
    }
}
