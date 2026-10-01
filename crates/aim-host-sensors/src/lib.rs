//! What the Mac can tell about its temperature and surroundings without
//! privileges: its thermal state and temperatures for the native
//! `thermalservice`, and host-call module
//! [`aim_hostcall::module::SENSORS`] for the guest sensors HAL.
//!
//! - Thermal state: `NSProcessInfo.thermalState`, the public summary of the
//!   Mac's thermal pressure.
//! - Temperatures and ambient light: the HID event system's sensor services
//!   ([`hid`]).
//! - Lid angle: the lid sensor's HID feature report ([`lid`]).
//!
//! The Mac's motion sensors are not reachable from user space, so there is
//! no accelerometer or gyroscope.

mod cf;
mod hid;
mod lid;

use std::ffi::{c_char, c_void};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aim_hostcall::sensors::{self, Readings, present};
use aim_hostcall::{HostModule, args_mut, errno, module};

pub static SENSORS: HostModule = HostModule {
    id: module::SENSORS,
    name: "sensors",
    version: sensors::VERSION,
    call: call_sensors,
};

/// Runs `fill` on the argument block of type `T`.
///
/// # Safety
/// `args`/`len` come from the guest (see [`args_mut`]).
unsafe fn fill<T>(args: u64, len: u64, fill: impl FnOnce() -> T) -> i64 {
    // SAFETY: the caller passes the guest's argument block.
    match unsafe { args_mut::<T>(args, len) } {
        Ok(out) => {
            *out = fill();
            0
        }
        Err(e) => e,
    }
}

unsafe fn call_sensors(func: u32, args: u64, len: u64) -> i64 {
    match func {
        // SAFETY: the registry passes the guest's argument block.
        sensors::FN_READ => unsafe { fill(args, len, read_sensors) },
        _ => -(errno::ENOSYS as i64),
    }
}

aim_hostcall::dylib! {
    static FOUNDATION = c"/System/Library/Frameworks/Foundation.framework/Foundation" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        static objc_msgSend: c_void;
    }
}

/// `NSProcessInfoThermalState` values.
pub mod state {
    pub const NOMINAL: u32 = 0;
    pub const FAIR: u32 = 1;
    pub const SERIOUS: u32 = 2;
    pub const CRITICAL: u32 = 3;
}

/// The Mac's thermal state and temperatures. A temperature the Mac cannot
/// read is NaN.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Thermal {
    /// One of [`state`].
    pub state: u32,
    /// The hottest CPU die sensor, in degrees Celsius.
    pub cpu_celsius: f32,
    /// The battery (mean of its gauge sensors), in degrees Celsius.
    pub battery_celsius: f32,
}

/// `[[NSProcessInfo processInfo] thermalState]`: one of [`state`].
pub fn thermal_state() -> u32 {
    type Send = unsafe extern "C" fn(*mut c_void, *const c_void) -> isize;
    // SAFETY: both messages take no argument; `processInfo` returns the
    // shared (never released) instance and `thermalState` an NSInteger.
    unsafe {
        let send: Send = std::mem::transmute(objc_msgSend());
        let info = send(
            objc_getClass(c"NSProcessInfo".as_ptr()),
            sel_registerName(c"processInfo".as_ptr()),
        ) as *mut c_void;
        let state = send(info, sel_registerName(c"thermalState".as_ptr()));
        state.clamp(0, state::CRITICAL as isize) as u32
    }
}

/// Temperatures of the services whose names satisfy `select`, from the
/// HID event system.
fn temperatures(select: impl Fn(&str) -> bool) -> Vec<f32> {
    let services = hid::services(hid::USAGE_TEMPERATURE).lock().unwrap();
    let Some(s) = services.as_ref() else {
        return Vec::new();
    };
    (0..s.names.len())
        .filter(|&i| select(&s.names[i]))
        .filter_map(|i| s.read(i, hid::EVENT_TEMPERATURE))
        // A sensor that is off reads 0 or less.
        .filter(|&t| t > 0.0 && t < 150.0)
        .map(|t| t as f32)
        .collect()
}

/// How long a temperature reading stands: the period the thermal HAL
/// polled at. Each sensor is a round trip to the HID event system (about
/// 0.7 ms, some 20 of them), and these change slowly.
const TEMPERATURES_MAX_AGE: Duration = Duration::from_secs(5);

/// The hottest CPU die and the battery, read at most every
/// [`TEMPERATURES_MAX_AGE`].
fn cpu_and_battery() -> (f32, f32) {
    static LAST: Mutex<Option<(Instant, f32, f32)>> = Mutex::new(None);
    // Held while reading, so callers at once share one reading.
    let mut last = LAST.lock().unwrap();
    if let Some((at, cpu, battery)) = *last
        && at.elapsed() < TEMPERATURES_MAX_AGE
    {
        return (cpu, battery);
    }
    // CPU die sensors are named "PMU tdie<n>"; the battery's are "gas gauge
    // battery".
    let cpu = temperatures(|n| n.starts_with("PMU tdie"));
    let battery = temperatures(|n| n == "gas gauge battery");
    let cpu = cpu.iter().copied().reduce(f32::max).unwrap_or(f32::NAN);
    let battery = if battery.is_empty() {
        f32::NAN
    } else {
        battery.iter().sum::<f32>() / battery.len() as f32
    };
    *last = Some((Instant::now(), cpu, battery));
    (cpu, battery)
}

/// The thermal state, read now, and the temperatures, read within
/// [`TEMPERATURES_MAX_AGE`].
pub fn read_thermal() -> Thermal {
    let (cpu_celsius, battery_celsius) = cpu_and_battery();
    Thermal {
        state: thermal_state(),
        cpu_celsius,
        battery_celsius,
    }
}

fn ambient_light() -> Option<f32> {
    let services = hid::services(hid::USAGE_AMBIENT_LIGHT).lock().unwrap();
    let s = services.as_ref()?;
    (0..s.names.len())
        .find_map(|i| s.read(i, hid::EVENT_AMBIENT_LIGHT))
        .map(|lux| lux.max(0.0) as f32)
}

pub fn read_sensors() -> Readings {
    let mut r = Readings::default();
    if let Some(lux) = ambient_light() {
        r.present |= present::LIGHT;
        r.light_lux = lux;
    }
    if let Some(angle) = lid::angle() {
        r.present |= present::HINGE;
        r.hinge_degrees = angle;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_is_plausible() {
        let t = read_thermal();
        eprintln!("{t:?}");
        assert!(t.state <= state::CRITICAL);
        for c in [t.cpu_celsius, t.battery_celsius] {
            assert!(c.is_nan() || (0.0..150.0).contains(&c));
        }
    }

    #[test]
    fn temperatures_are_read_once_per_period() {
        let first = read_thermal();
        let start = Instant::now();
        let again = read_thermal();
        assert!(start.elapsed() < Duration::from_millis(10));
        assert_eq!(first.cpu_celsius.to_bits(), again.cpu_celsius.to_bits());
        assert_eq!(
            first.battery_celsius.to_bits(),
            again.battery_celsius.to_bits()
        );
    }

    #[test]
    fn sensors_are_plausible() {
        let r = read_sensors();
        eprintln!("{r:?}");
        assert_eq!(r.present & !(present::LIGHT | present::HINGE), 0);
        assert!(r.light_lux >= 0.0);
        assert!((0.0..=360.0).contains(&r.hinge_degrees));
    }

    #[test]
    fn calls_check_the_argument_block() {
        let mut r = Readings::default();
        let q = &mut r as *mut Readings as u64;
        let einval = -(errno::EINVAL as i64);
        // SAFETY: live, correctly sized blocks; the bad calls never touch them.
        unsafe {
            assert_eq!(call_sensors(sensors::FN_READ, 0, 12), einval);
            assert_eq!(call_sensors(9, q, 12), -(errno::ENOSYS as i64));
            assert_eq!(call_sensors(sensors::FN_READ, q, 16), einval);
            assert_eq!(call_sensors(sensors::FN_READ, q, 12), 0);
        }
    }
}
