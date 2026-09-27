//! Host-call modules [`darwin_hostcall::module::THERMAL`] and
//! [`darwin_hostcall::module::SENSORS`]: what the Mac can tell about its
//! temperature and surroundings without privileges, for the guest thermal
//! and sensors HALs.
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

use darwin_hostcall::sensors::{self, Readings, present};
use darwin_hostcall::thermal::{self, Thermal};
use darwin_hostcall::{HostModule, args_mut, errno, module};

pub static THERMAL: HostModule = HostModule {
    id: module::THERMAL,
    name: "thermal",
    version: thermal::VERSION,
    call: call_thermal,
};

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

unsafe fn call_thermal(func: u32, args: u64, len: u64) -> i64 {
    match func {
        // SAFETY: the registry passes the guest's argument block.
        thermal::FN_READ => unsafe { fill(args, len, read_thermal) },
        _ => -(errno::ENOSYS as i64),
    }
}

unsafe fn call_sensors(func: u32, args: u64, len: u64) -> i64 {
    match func {
        // SAFETY: the registry passes the guest's argument block.
        sensors::FN_READ => unsafe { fill(args, len, read_sensors) },
        _ => -(errno::ENOSYS as i64),
    }
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *const c_void;
    fn objc_msgSend();
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

/// `[[NSProcessInfo processInfo] thermalState]`.
fn thermal_state() -> u32 {
    type Send = unsafe extern "C" fn(*mut c_void, *const c_void) -> isize;
    // SAFETY: both messages take no argument; `processInfo` returns the
    // shared (never released) instance and `thermalState` an NSInteger.
    unsafe {
        let send: Send = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let info = send(
            objc_getClass(c"NSProcessInfo".as_ptr()),
            sel_registerName(c"processInfo".as_ptr()),
        ) as *mut c_void;
        let state = send(info, sel_registerName(c"thermalState".as_ptr()));
        state.clamp(0, thermal::state::CRITICAL as isize) as u32
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

pub fn read_thermal() -> Thermal {
    // CPU die sensors are named "PMU tdie<n>"; the battery's are "gas gauge
    // battery".
    let cpu = temperatures(|n| n.starts_with("PMU tdie"));
    let battery = temperatures(|n| n == "gas gauge battery");
    Thermal {
        state: thermal_state(),
        cpu_celsius: cpu.iter().copied().reduce(f32::max).unwrap_or(f32::NAN),
        battery_celsius: if battery.is_empty() {
            f32::NAN
        } else {
            battery.iter().sum::<f32>() / battery.len() as f32
        },
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
        assert!(t.state <= thermal::state::CRITICAL);
        for c in [t.cpu_celsius, t.battery_celsius] {
            assert!(c.is_nan() || (0.0..150.0).contains(&c));
        }
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
        let mut t = Thermal::default();
        let p = &mut t as *mut Thermal as u64;
        let mut r = Readings::default();
        let q = &mut r as *mut Readings as u64;
        let einval = -(errno::EINVAL as i64);
        // SAFETY: live, correctly sized blocks; the bad calls never touch them.
        unsafe {
            assert_eq!(call_thermal(thermal::FN_READ, p, 11), einval);
            assert_eq!(call_thermal(thermal::FN_READ, 0, 12), einval);
            assert_eq!(call_thermal(9, p, 12), -(errno::ENOSYS as i64));
            assert_eq!(call_thermal(thermal::FN_READ, p, 12), 0);
            assert_eq!(call_sensors(sensors::FN_READ, q, 16), einval);
            assert_eq!(call_sensors(sensors::FN_READ, q, 12), 0);
        }
    }
}
