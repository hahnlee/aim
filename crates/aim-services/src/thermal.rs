//! `thermalservice` (IThermalService): the Mac's thermal state as a native
//! service (ADR 0013), in place of SystemServer's ThermalManagerService.
//!
//! This is `ThermalManagerService.java` at the pinned tag with the Mac in
//! the thermal HAL's place (the image has none, docs/vendor-hals.md). What
//! our HAL reported to the original is read here in this process
//! ([`aim_host_sensors`]):
//!
//! - the `SKIN` temperature, whose throttling status is macOS's thermal
//!   state and whose value is unknown (NaN); `CPU` and `BATTERY` when the
//!   Mac reads them, with no status of their own;
//! - no cooling devices and no temperature thresholds, so no headroom:
//!   `getThermalHeadroom` is NaN, every headroom threshold is NaN and no
//!   headroom listener is ever called, as with the original over that HAL;
//! - a change of the thermal state is noticed within [`POLL`], as the HAL
//!   noticed it.
//!
//! The thermal status is the highest `SKIN` status, unless the shell
//! overrides it (`cmd thermalservice`); status and temperature listeners
//! hear of changes, and a `SHUTDOWN` temperature shuts the device down
//! through `power`.
//!
//! Not here: the statsd atoms (`THERMAL_STATUS_CALLED`,
//! `THERMAL_HEADROOM_CALLED`, the `THERMAL_HEADROOM_THRESHOLDS` pull) and
//! the `thermal_changed` event log entry (#617).

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_host_sensors::{Thermal, state};
use aim_service_aidl::{
    WriteParcelable, android_os_ipowermanager as power,
    android_os_ithermaleventlistener as event_listener, android_os_ithermalservice as its,
    android_os_ithermalstatuslistener as status_listener,
};

use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};
use crate::system::System;

const DEVICE_POWER: &str = "android.permission.DEVICE_POWER";
const DUMP: &str = "android.permission.DUMP";
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");
/// How often the Mac's thermal state is read, as the HAL read it.
const POLL: Duration = Duration::from_secs(5);

/// `Temperature.TYPE_*` (`TemperatureType`).
const TYPE_CPU: i32 = 0;
const TYPE_BATTERY: i32 = 2;
const TYPE_SKIN: i32 = 3;
/// `Temperature.THROTTLING_*` (`ThrottlingSeverity`).
const THROTTLING_NONE: i32 = 0;
const THROTTLING_LIGHT: i32 = 1;
const THROTTLING_SEVERE: i32 = 3;
const THROTTLING_CRITICAL: i32 = 4;
const THROTTLING_SHUTDOWN: i32 = 6;
/// The temperature types `inject-temperature` takes, by name.
const TYPES: &[(&str, i32)] = &[
    ("UNKNOWN", -1),
    ("CPU", 0),
    ("GPU", 1),
    ("BATTERY", 2),
    ("SKIN", 3),
    ("USB_PORT", 4),
    ("POWER_AMPLIFIER", 5),
    ("BCL_VOLTAGE", 6),
    ("BCL_CURRENT", 7),
    ("BCL_PERCENTAGE", 8),
    ("NPU", 9),
    ("TPU", 10),
    ("DISPLAY", 11),
    ("MODEM", 12),
    ("SOC", 13),
    ("WIFI", 14),
    ("CAMERA", 15),
    ("FLASHLIGHT", 16),
    ("SPEAKER", 17),
    ("AMBIENT", 18),
    ("POGO", 19),
];
/// The throttling statuses it takes, by name.
const STATUSES: &[&str] = &[
    "NONE",
    "LIGHT",
    "MODERATE",
    "SEVERE",
    "CRITICAL",
    "EMERGENCY",
    "SHUTDOWN",
];
/// `getThermalHeadroom`'s forecast range.
const MIN_FORECAST_SEC: i32 = 0;
const MAX_FORECAST_SEC: i32 = 60;
/// `PowerManager.SHUTDOWN_THERMAL_STATE` and
/// `SHUTDOWN_BATTERY_THERMAL_STATE`.
const SHUTDOWN_THERMAL_STATE: &str = "thermal";
const SHUTDOWN_BATTERY_THERMAL_STATE: &str = "thermal,battery";
/// `Process.SHELL_UID` and `ROOT_UID`, who may run shell commands.
const SHELL_UID: u32 = 2000;
const ROOT_UID: u32 = 0;

type Result<T> = std::result::Result<T, Exception>;

fn bad_parcel(status: i32) -> Exception {
    Exception::illegal_argument(format!("bad parcel: status {status}"))
}

/// `android.os.Temperature`.
#[derive(Clone, Debug, PartialEq)]
pub struct Temperature {
    value: f32,
    kind: i32,
    name: String,
    status: i32,
}

impl WriteParcelable for Temperature {
    fn write_to(&self, p: &mut Parcel) {
        p.write_f32(self.value);
        p.write_i32(self.kind);
        p.write_string16(Some(&self.name));
        p.write_i32(self.status);
    }
}

impl std::fmt::Display for Temperature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Temperature{{mValue={}, mType={}, mName={}, mStatus={}}}",
            java_float(self.value),
            self.kind,
            self.name,
            self.status
        )
    }
}

/// `Float.toString` for what a temperature holds.
fn java_float(v: f32) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e7 {
        return format!("{v:.1}");
    }
    format!("{v}")
}

/// The throttling status Android gives macOS's thermal state, as the HAL
/// gives it.
fn severity(s: u32) -> i32 {
    match s {
        state::NOMINAL => THROTTLING_NONE,
        state::FAIR => THROTTLING_LIGHT,
        state::SERIOUS => THROTTLING_SEVERE,
        _ => THROTTLING_CRITICAL,
    }
}

/// The Mac's `SKIN` temperature in a thermal state.
fn skin(state: u32) -> Temperature {
    Temperature {
        value: f32::NAN,
        kind: TYPE_SKIN,
        name: "skin".into(),
        status: severity(state),
    }
}

/// The Mac's temperatures in a reading, as the HAL's `getTemperatures`
/// lists them.
fn temperatures(t: Thermal) -> Vec<Temperature> {
    let mut list = vec![skin(t.state)];
    for (kind, name, value) in [
        (TYPE_CPU, "cpu", t.cpu_celsius),
        (TYPE_BATTERY, "battery", t.battery_celsius),
    ] {
        if !value.is_nan() {
            list.push(Temperature {
                value,
                kind,
                name: name.into(),
                status: THROTTLING_NONE,
            });
        }
    }
    list
}

/// `String.hashCode`, which orders an `ArrayMap`'s keys.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// A registered listener (`RemoteCallbackList`), with its type filter.
struct Listener {
    strong: Strong,
    death: u64,
    kind: Option<i32>,
}

/// The three `RemoteCallbackList`s.
#[derive(Clone, Copy)]
enum Kind {
    Event,
    Status,
    Headroom,
}

#[derive(Default)]
struct State {
    status: i32,
    /// `mIsStatusOverride`.
    overridden: bool,
    /// `mTemperatureMap`: by name, in the map's order.
    temperatures: Vec<Temperature>,
    events: Vec<Listener>,
    statuses: Vec<Listener>,
    headrooms: Vec<Listener>,
    /// The thermal state last read, as the HAL's poll kept it.
    mac_state: u32,
}

impl State {
    fn list(&mut self, kind: Kind) -> &mut Vec<Listener> {
        match kind {
            Kind::Event => &mut self.events,
            Kind::Status => &mut self.statuses,
            Kind::Headroom => &mut self.headrooms,
        }
    }
}

pub struct ThermalManagerService {
    process: Arc<LocalProcess>,
    system: Arc<System>,
    this: Weak<Self>,
    state: Mutex<State>,
}

impl ThermalManagerService {
    pub fn new(process: Arc<LocalProcess>, system: Arc<System>) -> Arc<Self> {
        let service = Arc::new_cyclic(|this| Self {
            process,
            system,
            this: this.clone(),
            state: Mutex::new(State::default()),
        });
        // `onActivityManagerReady` with the HAL connected: the current
        // temperatures, then the status they give.
        let reading = aim_host_sensors::read_thermal();
        service.state.lock().unwrap().mac_state = reading.state;
        for t in temperatures(reading) {
            service.temperature_changed(t, false);
        }
        service.map_changed(&mut service.state.lock().unwrap());
        let weak = Arc::downgrade(&service);
        std::thread::Builder::new()
            .name("thermal-mac".into())
            .spawn(move || poll(weak))
            .expect("spawn the Mac thermal thread");
        service
    }

    /// The HAL's `notifyThrottling(skin)` when the thermal state changed.
    fn poll_mac(&self) {
        let now = aim_host_sensors::thermal_state();
        let changed = {
            let mut s = self.state.lock().unwrap();
            std::mem::replace(&mut s.mac_state, now) != now
        };
        if changed {
            self.temperature_changed(skin(now), true);
        }
    }

    /// `onTemperatureChanged`.
    fn temperature_changed(&self, t: Temperature, send_callback: bool) {
        self.shutdown_if_needed(&t);
        let mut s = self.state.lock().unwrap();
        let old = match s.temperatures.iter().position(|o| o.name == t.name) {
            Some(at) => Some(std::mem::replace(&mut s.temperatures[at], t.clone())),
            None => {
                let hash = java_hash(&t.name);
                let at = s
                    .temperatures
                    .partition_point(|o| java_hash(&o.name) <= hash);
                s.temperatures.insert(at, t.clone());
                None
            }
        };
        if old.is_none_or(|o| o.status != t.status) {
            for l in &s.events {
                self.post_temperature(l, &t);
            }
        }
        if send_callback {
            self.map_changed(&mut s);
        }
        // A SKIN sample feeds the headroom forecast, which without
        // thresholds is NaN and is not sent to headroom listeners.
    }

    /// `onTemperatureMapChangedLocked`.
    fn map_changed(&self, s: &mut State) {
        let status = s
            .temperatures
            .iter()
            .filter(|t| t.kind == TYPE_SKIN)
            .map(|t| t.status)
            .fold(THROTTLING_NONE, i32::max);
        if !s.overridden {
            self.set_status(s, status);
        }
    }

    /// `setStatusLocked`.
    fn set_status(&self, s: &mut State, status: i32) {
        if status != s.status {
            s.status = status;
            for l in &s.statuses {
                self.post_status(l, status);
            }
        }
    }

    /// `IThermalEventListener.notifyThrottling`, one-way, if the listener
    /// takes the temperature's type.
    fn post_temperature(&self, l: &Listener, t: &Temperature) {
        if l.kind.is_some_and(|k| k != t.kind) {
            return;
        }
        let mut data = Parcel::new();
        event_listener::NotifyThrottling {
            temperature: Some(t.clone()),
        }
        .write(&mut data);
        let _ = l
            .strong
            .transact(event_listener::NOTIFY_THROTTLING, &data, true);
    }

    /// `IThermalStatusListener.onStatusChange`, one-way.
    fn post_status(&self, l: &Listener, status: i32) {
        let mut data = Parcel::new();
        status_listener::OnStatusChange { status }.write(&mut data);
        let _ = l
            .strong
            .transact(status_listener::ON_STATUS_CHANGE, &data, true);
    }

    /// `shutdownIfNeeded`: `PowerManager.shutdown` for a temperature at
    /// `SHUTDOWN`.
    fn shutdown_if_needed(&self, t: &Temperature) {
        if t.status != THROTTLING_SHUTDOWN {
            return;
        }
        let reason = match t.kind {
            // CPU, GPU, NPU, SKIN.
            0 | 1 | 9 | TYPE_SKIN => SHUTDOWN_THERMAL_STATE,
            TYPE_BATTERY => SHUTDOWN_BATTERY_THERMAL_STATE,
            _ => return,
        };
        let shutdown = power::Shutdown {
            confirm: false,
            reason: Some(reason.into()),
            wait: false,
        };
        if let Err(e) = self.system.call(
            "power",
            power::SHUTDOWN,
            |p| shutdown.write(p),
            power::read_shutdown_reply,
        ) {
            eprintln!("services: thermal shutdown: {}", e.message);
        }
    }

    /// `RemoteCallbackList.register`: a listener registered again replaces
    /// its registration.
    fn register(&self, kind: Kind, binder: Option<Binder>, filter: Option<i32>) -> Result<()> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke interface method 'android.os.IBinder \
                 android.os.IInterface.asBinder()' on a null object reference",
            ));
        };
        self.unregister(kind, handle);
        let strong = self.process.strong(handle);
        let this = self.this.clone();
        let death = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(service) = this.upgrade() {
                    let mut s = service.state.lock().unwrap();
                    s.list(kind).retain(|l| l.strong.handle != handle);
                }
            }),
        );
        let mut s = self.state.lock().unwrap();
        let listener = Listener {
            strong,
            death,
            kind: filter,
        };
        // Told of what is current once registered.
        match kind {
            Kind::Event => {
                for t in &s.temperatures {
                    self.post_temperature(&listener, t);
                }
            }
            Kind::Status => self.post_status(&listener, s.status),
            Kind::Headroom => {}
        }
        s.list(kind).push(listener);
        Ok(())
    }

    /// `RemoteCallbackList.unregister`: whether it was registered.
    fn unregister(&self, kind: Kind, handle: u32) -> bool {
        let taken = {
            let mut s = self.state.lock().unwrap();
            let list = s.list(kind);
            list.iter()
                .position(|l| l.strong.handle == handle)
                .map(|at| list.remove(at))
        };
        match taken {
            Some(l) => {
                self.process.clear_death(&l.strong, l.death);
                true
            }
            None => false,
        }
    }

    /// `Context.enforceCallingOrSelfPermission(permission, null)`.
    fn enforce(&self, call: &Call<'_>, permission: &str) -> Result<()> {
        let uid = call.sender_euid as i32;
        if self
            .system
            .check_permission(permission, call.sender_pid, uid)?
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Neither user {uid} nor current process has {permission}."
        )))
    }

    fn dispatch(&self, call: &mut Call<'_>) -> Result<Option<Parcel>> {
        let mut reply = Parcel::new();
        match call.code {
            its::REGISTER_THERMAL_EVENT_LISTENER => {
                let a =
                    its::RegisterThermalEventListener::read(&mut call.data).map_err(bad_parcel)?;
                self.enforce(call, DEVICE_POWER)?;
                self.register(Kind::Event, a.listener, None)?;
                its::write_register_thermal_event_listener_reply(&mut reply, true);
            }
            its::REGISTER_THERMAL_EVENT_LISTENER_WITH_TYPE => {
                let a = its::RegisterThermalEventListenerWithType::read(&mut call.data)
                    .map_err(bad_parcel)?;
                self.enforce(call, DEVICE_POWER)?;
                self.register(Kind::Event, a.listener, Some(a.r#type))?;
                its::write_register_thermal_event_listener_with_type_reply(&mut reply, true);
            }
            its::UNREGISTER_THERMAL_EVENT_LISTENER => {
                let a = its::UnregisterThermalEventListener::read(&mut call.data)
                    .map_err(bad_parcel)?;
                self.enforce(call, DEVICE_POWER)?;
                let done = self.unregister_binder(Kind::Event, a.listener)?;
                its::write_unregister_thermal_event_listener_reply(&mut reply, done);
            }
            its::GET_CURRENT_TEMPERATURES | its::GET_CURRENT_TEMPERATURES_WITH_TYPE => {
                // Hand-read: the generator has no parcelable arrays.
                call.data
                    .enforce_interface(its::DESCRIPTOR)
                    .map_err(bad_parcel)?;
                let filter = match call.code {
                    its::GET_CURRENT_TEMPERATURES => None,
                    _ => Some(call.data.read_i32().map_err(bad_parcel)?),
                };
                self.enforce(call, DEVICE_POWER)?;
                let list: Vec<_> = temperatures(aim_host_sensors::read_thermal())
                    .into_iter()
                    .filter(|t| filter.is_none_or(|k| k == t.kind))
                    .collect();
                reply.write_no_exception();
                // `writeTypedArray`.
                reply.write_i32(list.len() as i32);
                for t in &list {
                    reply.write_i32(1);
                    t.write_to(&mut reply);
                }
            }
            its::REGISTER_THERMAL_STATUS_LISTENER => {
                let a =
                    its::RegisterThermalStatusListener::read(&mut call.data).map_err(bad_parcel)?;
                self.register(Kind::Status, a.listener, None)?;
                its::write_register_thermal_status_listener_reply(&mut reply, true);
            }
            its::UNREGISTER_THERMAL_STATUS_LISTENER => {
                let a = its::UnregisterThermalStatusListener::read(&mut call.data)
                    .map_err(bad_parcel)?;
                let done = self.unregister_binder(Kind::Status, a.listener)?;
                its::write_unregister_thermal_status_listener_reply(&mut reply, done);
            }
            its::GET_CURRENT_THERMAL_STATUS => {
                its::GetCurrentThermalStatus::read(&mut call.data).map_err(bad_parcel)?;
                let status = self.state.lock().unwrap().status;
                its::write_get_current_thermal_status_reply(&mut reply, status);
            }
            its::GET_CURRENT_COOLING_DEVICES | its::GET_CURRENT_COOLING_DEVICES_WITH_TYPE => {
                call.data
                    .enforce_interface(its::DESCRIPTOR)
                    .map_err(bad_parcel)?;
                if call.code == its::GET_CURRENT_COOLING_DEVICES_WITH_TYPE {
                    call.data.read_i32().map_err(bad_parcel)?;
                }
                self.enforce(call, DEVICE_POWER)?;
                // The Mac's fans are the SMC's: no cooling devices.
                reply.write_no_exception();
                reply.write_i32(0);
            }
            its::GET_THERMAL_HEADROOM => {
                // Without temperature thresholds there is no forecast, in
                // the range or out of it.
                its::GetThermalHeadroom::read(&mut call.data).map_err(bad_parcel)?;
                its::write_get_thermal_headroom_reply(&mut reply, f32::NAN);
            }
            its::GET_THERMAL_HEADROOM_THRESHOLDS => {
                call.data
                    .enforce_interface(its::DESCRIPTOR)
                    .map_err(bad_parcel)?;
                // `allow_thermal_headroom_thresholds` is on in the image: a
                // threshold per status up to SHUTDOWN, none known.
                reply.write_no_exception();
                reply.write_i32(THROTTLING_SHUTDOWN + 1);
                for _ in 0..=THROTTLING_SHUTDOWN {
                    reply.write_f32(f32::NAN);
                }
            }
            its::REGISTER_THERMAL_HEADROOM_LISTENER => {
                let a = its::RegisterThermalHeadroomListener::read(&mut call.data)
                    .map_err(bad_parcel)?;
                self.register(Kind::Headroom, a.listener, None)?;
                its::write_register_thermal_headroom_listener_reply(&mut reply, true);
            }
            its::UNREGISTER_THERMAL_HEADROOM_LISTENER => {
                let a = its::UnregisterThermalHeadroomListener::read(&mut call.data)
                    .map_err(bad_parcel)?;
                let done = self.unregister_binder(Kind::Headroom, a.listener)?;
                its::write_unregister_thermal_headroom_listener_reply(&mut reply, done);
            }
            DUMP_TRANSACTION => {
                self.dump(call);
                reply.write_no_exception();
            }
            SHELL_COMMAND_TRANSACTION => {
                let command =
                    ShellCommand::read(&self.process, &mut call.data).map_err(bad_parcel)?;
                // Only the shell and root; anyone else gets no result.
                if command.has_output() && [SHELL_UID, ROOT_UID].contains(&call.sender_euid) {
                    self.shell_command(command);
                }
                reply.write_no_exception();
            }
            _ => return Ok(None),
        }
        Ok(Some(reply))
    }

    fn unregister_binder(&self, kind: Kind, binder: Option<Binder>) -> Result<bool> {
        match binder {
            Some(Binder::Handle(handle)) => Ok(self.unregister(kind, handle)),
            _ => Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke interface method 'android.os.IBinder \
                 android.os.IInterface.asBinder()' on a null object reference",
            )),
        }
    }

    /// `ThermalShellCommand.onCommand`, run by `exec`.
    fn shell_command(&self, mut c: ShellCommand) {
        let result = match c.command().map(str::to_string).as_deref() {
            Some("inject-temperature") => self.inject_temperature(&mut c),
            Some("override-status") => match c.next_arg_required().and_then(|a| java_int(&a)) {
                Err(e) => {
                    c.println(&format!("Error: {e}"));
                    -1
                }
                Ok(status) if !(0..=THROTTLING_SHUTDOWN).contains(&status) => {
                    c.println(&format!("Invalid status: {status}"));
                    -1
                }
                Ok(status) => {
                    let mut s = self.state.lock().unwrap();
                    s.overridden = true;
                    self.set_status(&mut s, status);
                    0
                }
            },
            Some("reset") => {
                let mut s = self.state.lock().unwrap();
                s.overridden = false;
                self.map_changed(&mut s);
                0
            }
            Some("headroom") => match c.next_arg_required().and_then(|a| java_int(&a)) {
                Err(e) => {
                    c.println(&format!("Error: {e}"));
                    -1
                }
                Ok(secs) if !(MIN_FORECAST_SEC..=MAX_FORECAST_SEC).contains(&secs) => {
                    c.println(&format!(
                        "Error: forecast second input should be in range \
                         [{MIN_FORECAST_SEC},{MAX_FORECAST_SEC}]"
                    ));
                    -1
                }
                Ok(secs) => {
                    c.println(&format!("Headroom in {secs} seconds: NaN"));
                    0
                }
            },
            None | Some("help" | "-h") => {
                help(&mut c);
                -1
            }
            Some(other) => {
                c.println(&format!("Unknown command: {other}"));
                -1
            }
        };
        c.finish(result);
    }

    /// `runInjectTemperature`.
    fn inject_temperature(&self, c: &mut ShellCommand) -> i32 {
        let args = (|| {
            let kind = c.next_arg_required()?;
            let Some(&(_, kind)) = TYPES.iter().find(|(n, _)| *n == kind.to_uppercase()) else {
                return Ok(Err(format!("Invalid temperature type: {kind}")));
            };
            let status = c.next_arg_required()?;
            let Some(status) = STATUSES.iter().position(|n| *n == status.to_uppercase()) else {
                return Ok(Err(format!("Invalid throttle status: {status}")));
            };
            let name = c.next_arg_required()?;
            let value = match c.next_arg() {
                None => 28.0,
                Some(v) => match java_float_parse(&v) {
                    Some(v) => v,
                    None => {
                        return Ok(Err(format!(
                            "Error: java.lang.NumberFormatException: For input string: \"{v}\""
                        )));
                    }
                },
            };
            // `Temperature`'s `checkStringNotEmpty`.
            if name.is_empty() {
                return Err("java.lang.IllegalArgumentException".to_string());
            }
            Ok(Ok(Temperature {
                value,
                kind,
                name,
                status: status as i32,
            }))
        })();
        match args {
            Err(exception) => {
                c.exception(&exception);
                -1
            }
            Ok(Err(line)) => {
                c.println(&line);
                -1
            }
            Ok(Ok(t)) => {
                self.temperature_changed(t, true);
                0
            }
        }
    }

    /// `dumpInternal`, after `DumpUtils.checkDumpPermission`.
    fn dump(&self, call: &mut Call<'_>) {
        use std::io::Write;
        let Some(mut out) = call
            .data
            .read_fd()
            .ok()
            .and_then(|fd| self.process.file(fd))
            .and_then(|f| aim_binder_host::server::file_fd(&f))
            .map(std::fs::File::from)
        else {
            return;
        };
        let (pid, uid) = (call.sender_pid, call.sender_euid as i32);
        if !self
            .system
            .check_permission(DUMP, pid, uid)
            .unwrap_or(false)
        {
            let _ = writeln!(
                out,
                "Permission Denial: can't dump ThermalManagerService from from pid={pid}, \
                 uid={uid} due to missing android.permission.DUMP permission"
            );
            return;
        }
        let mut text = String::new();
        {
            let s = self.state.lock().unwrap();
            text += &format!("IsStatusOverride: {}\n", s.overridden);
            // `RemoteCallbackList.dump`, outside a broadcast.
            for (name, count) in [
                ("ThermalEventListeners", s.events.len()),
                ("ThermalStatusListeners", s.statuses.len()),
            ] {
                text += &format!(
                    "{name}:\n\tcallbacks: {count}\n\tkilled: false\n\tbroadcasts count: -1\n"
                );
            }
            text += &format!("Thermal Status: {}\nCached temperatures:\n", s.status);
            for t in &s.temperatures {
                text += &format!("\t{t}\n");
            }
        }
        text += "HAL Ready: true\nHAL connection:\n\tthe Mac (NSProcessInfo.thermalState)\n";
        text += "Current temperatures from HAL:\n";
        for t in temperatures(aim_host_sensors::read_thermal()) {
            text += &format!("\t{t}\n");
        }
        text += "Current cooling devices from HAL:\n";
        text += "Temperature static thresholds from HAL:\n";
        text += "Temperature headroom thresholds:\n";
        text += &format!(
            "[{}]\n",
            ["NaN"; THROTTLING_SHUTDOWN as usize + 1].join(", ")
        );
        let _ = out.write_all(text.as_bytes());
    }
}

/// `ThermalShellCommand.onHelp`.
fn help(c: &mut ShellCommand) {
    for line in [
        "Thermal service (thermalservice) commands:",
        "  help",
        "    Print this help text.",
        "",
        "  inject-temperature TYPE STATUS NAME [VALUE]",
        "    injects a new temperature sample for the specified device.",
        "    type and status strings follow the names in android.os.Temperature.",
        "  override-status STATUS",
        "    sets and locks the thermal status of the device to STATUS.",
        "    status code is defined in android.os.Temperature.",
        "  reset",
        "    unlocks the thermal status of the device.",
        "  headroom FORECAST_SECONDS",
        &format!(
            "    gets the thermal headroom forecast in specified seconds, from \
             [{MIN_FORECAST_SEC},{MAX_FORECAST_SEC}]."
        ),
        "",
    ] {
        c.println(line);
    }
}

/// `Integer.parseInt`, or its exception as `toString` prints it.
fn java_int(s: &str) -> std::result::Result<i32, String> {
    s.parse()
        .map_err(|_| format!("java.lang.NumberFormatException: For input string: \"{s}\""))
}

/// `Float.parseFloat`: a decimal with an optional `f` or `d` suffix,
/// `NaN` or `Infinity`, around blanks.
pub(crate) fn java_float_parse(s: &str) -> Option<f32> {
    let t = s.trim();
    let t = t.strip_suffix(['f', 'F', 'd', 'D']).unwrap_or(t);
    let (sign, body) = match t.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    match body {
        "NaN" => Some(f32::NAN),
        "Infinity" => Some(sign * f32::INFINITY),
        _ if body
            .chars()
            .all(|c| c.is_ascii_digit() || ".eE+-".contains(c)) =>
        {
            body.parse::<f32>().ok().map(|v| sign * v)
        }
        _ => None,
    }
}

/// Reads the Mac's thermal state every [`POLL`] while the service lives.
fn poll(service: Weak<ThermalManagerService>) {
    loop {
        std::thread::sleep(POLL);
        let Some(service) = service.upgrade() else {
            return;
        };
        service.poll_mac();
    }
}

impl Service for ThermalManagerService {
    fn descriptor(&self) -> &str {
        its::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match self.dispatch(call) {
            Ok(Some(reply)) => Ok(reply),
            Ok(None) => Err(UNKNOWN_TRANSACTION),
            Err(exception) => {
                let mut reply = Parcel::new();
                reply.write_exception(&exception);
                Ok(reply)
            }
        }
    }

    fn accepts_fds(&self) -> bool {
        // A dump's fd, a shell command's streams.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Reader;

    #[test]
    fn thermal_states_map_to_rising_severities() {
        let s: Vec<_> = (0..=4).map(severity).collect();
        assert_eq!(
            s,
            [
                THROTTLING_NONE,
                THROTTLING_LIGHT,
                THROTTLING_SEVERE,
                THROTTLING_CRITICAL,
                THROTTLING_CRITICAL
            ]
        );
    }

    #[test]
    fn missing_sensors_are_left_out() {
        let kinds = |t| temperatures(t).iter().map(|t| t.kind).collect::<Vec<_>>();
        let reading = |cpu_celsius, battery_celsius| Thermal {
            state: state::FAIR,
            cpu_celsius,
            battery_celsius,
        };
        assert_eq!(
            kinds(reading(50.0, 30.0)),
            [TYPE_SKIN, TYPE_CPU, TYPE_BATTERY]
        );
        // A Mac without a battery, and one whose sensors are unreadable.
        assert_eq!(kinds(reading(50.0, f32::NAN)), [TYPE_SKIN, TYPE_CPU]);
        let none = temperatures(reading(f32::NAN, f32::NAN));
        assert_eq!(none.len(), 1);
        assert_eq!(
            (none[0].kind, none[0].status),
            (TYPE_SKIN, THROTTLING_LIGHT)
        );
    }

    #[test]
    fn temperatures_print_and_parcel_as_java() {
        let t = Temperature {
            value: 50.0,
            kind: TYPE_CPU,
            name: "cpu".into(),
            status: THROTTLING_NONE,
        };
        assert_eq!(
            t.to_string(),
            "Temperature{mValue=50.0, mType=0, mName=cpu, mStatus=0}"
        );
        assert_eq!(java_float(f32::NAN), "NaN");
        assert_eq!(java_float(48.9), "48.9");
        let mut p = Parcel::new();
        t.write_to(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        assert_eq!(r.read_f32().unwrap(), 50.0);
        assert_eq!(r.read_i32().unwrap(), TYPE_CPU);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("cpu"));
        assert_eq!(r.read_i32().unwrap(), THROTTLING_NONE);
    }

    #[test]
    fn parses_numbers_as_java() {
        assert_eq!(java_int("3"), Ok(3));
        assert_eq!(java_int("-1"), Ok(-1));
        assert_eq!(
            java_int("x"),
            Err("java.lang.NumberFormatException: For input string: \"x\"".into())
        );
        assert_eq!(java_float_parse("41.5f"), Some(41.5));
        assert_eq!(java_float_parse(" -2 "), Some(-2.0));
        assert!(java_float_parse("NaN").unwrap().is_nan());
        assert_eq!(java_float_parse("inf"), None);
        assert_eq!(java_float_parse("abc"), None);
    }

    #[test]
    fn hashes_as_java() {
        assert_eq!(java_hash("cpu"), 98728);
        assert_eq!(java_hash("skin"), 3532157);
        assert_eq!(java_hash("battery"), -331239923);
    }
}
