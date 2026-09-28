//! Host-call module [`aim_hostcall::module::HEALTH`]: the Mac's battery
//! for the guest `android.hardware.health` service.
//!
//! Status, level and times come from the power-source API
//! (`IOPSCopyPowerSourcesInfo`), the same source as the menu bar battery.
//! Charge in mAh, cycle count and (where macOS still publishes it)
//! temperature come from the `AppleSmartBattery` registry entry.

use std::ffi::{CStr, c_char, c_void};

use aim_hostcall::health::{Battery, FN_BATTERY, VERSION, condition, status};
use aim_hostcall::{HostModule, args_mut, errno, module};

pub static MODULE: HostModule = HostModule {
    id: module::HEALTH,
    name: "health",
    version: VERSION,
    call,
};

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        FN_BATTERY => {
            // SAFETY: the registry passes the guest's argument block.
            match unsafe { args_mut::<Battery>(args, len) } {
                Ok(out) => {
                    *out = battery();
                    0
                }
                Err(e) => e,
            }
        }
        _ => -(errno::ENOSYS as i64),
    }
}

type CFTypeRef = *const c_void;

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFNumberGetValue(number: CFTypeRef, the_type: isize, value: *mut c_void) -> bool;
        fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    }
}

aim_hostcall::dylib! {
    static IO_KIT = c"/System/Library/Frameworks/IOKit.framework/IOKit" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
        fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
        fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
        fn IOServiceGetMatchingService(main_port: u32, matching: CFTypeRef) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: CFTypeRef,
            alloc: CFTypeRef,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: u32) -> i32;
    }
}

const UTF8: u32 = 0x0800_0100;
const SINT64: isize = 4;

/// An owned (+1) CoreFoundation reference.
struct Owned(CFTypeRef);

impl Owned {
    fn new(r: CFTypeRef) -> Option<Self> {
        (!r.is_null()).then(|| Self(r))
    }

    fn string(s: &CStr) -> Self {
        // SAFETY: a NUL-terminated UTF-8 literal.
        Self(unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) })
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we hold one reference.
        unsafe { CFRelease(self.0) };
    }
}

fn is(value: CFTypeRef, type_id: usize) -> bool {
    // SAFETY: a live CF object.
    !value.is_null() && unsafe { CFGetTypeID(value) } == type_id
}

fn number(value: CFTypeRef) -> Option<i64> {
    // SAFETY: type-checked CFNumber read into a local.
    unsafe {
        if !is(value, CFNumberGetTypeID()) {
            return None;
        }
        let mut v = 0i64;
        CFNumberGetValue(value, SINT64, (&mut v as *mut i64).cast()).then_some(v)
    }
}

fn boolean(value: CFTypeRef) -> Option<bool> {
    // SAFETY: type-checked CFBoolean.
    unsafe { is(value, CFBooleanGetTypeID()).then(|| CFBooleanGetValue(value)) }
}

fn string_is(value: CFTypeRef, s: &CStr) -> bool {
    // SAFETY: type-checked CFString comparison.
    is(value, unsafe { CFStringGetTypeID() }) && unsafe { CFEqual(value, Owned::string(s).0) }
}

/// A borrowed CFDictionary.
struct Description(CFTypeRef);

impl Description {
    fn get(&self, key: &CStr) -> CFTypeRef {
        // SAFETY: a live dictionary; the key lives for the call.
        unsafe { CFDictionaryGetValue(self.0, Owned::string(key).0) }
    }
}

/// Read the battery now.
pub fn battery() -> Battery {
    let mut b = Battery {
        status: status::UNKNOWN,
        health: condition::UNKNOWN,
        time_to_full_seconds: -1,
        time_to_empty_seconds: -1,
        ..Battery::default()
    };
    // SAFETY: the power-source snapshot and list are owned; descriptions
    // are borrowed from the snapshot, which outlives their use.
    unsafe {
        let Some(info) = Owned::new(IOPSCopyPowerSourcesInfo()) else {
            return b;
        };
        let Some(list) = Owned::new(IOPSCopyPowerSourcesList(info.0)) else {
            return b;
        };
        for i in 0..CFArrayGetCount(list.0) {
            let d = IOPSGetPowerSourceDescription(info.0, CFArrayGetValueAtIndex(list.0, i));
            if d.is_null() {
                continue;
            }
            let d = Description(d);
            if string_is(d.get(c"Type"), c"InternalBattery") {
                from_power_source(&d, &mut b);
                break;
            }
        }
    }
    if b.present != 0 {
        from_smart_battery(&mut b);
    } else {
        b.health = condition::NOT_AVAILABLE;
    }
    b
}

fn from_power_source(d: &Description, b: &mut Battery) {
    b.present = boolean(d.get(c"Is Present")).unwrap_or(true) as u32;
    b.ac_online = string_is(d.get(c"Power Source State"), c"AC Power") as u32;
    let current = number(d.get(c"Current Capacity"));
    let max = number(d.get(c"Max Capacity")).filter(|&m| m > 0);
    if let (Some(c), Some(m)) = (current, max) {
        b.level_percent = (c * 100 / m).clamp(0, 100) as i32;
    }
    let charging = boolean(d.get(c"Is Charging")).unwrap_or(false);
    let charged = boolean(d.get(c"Is Charged")).unwrap_or(false);
    b.status = match (b.ac_online != 0, charging, charged) {
        (_, true, _) => status::CHARGING,
        (true, false, true) => status::FULL,
        (true, false, false) => status::NOT_CHARGING,
        (false, _, _) => status::DISCHARGING,
    };
    // Minutes; -1 while the system is still estimating.
    let minutes = |key: &CStr| {
        number(d.get(key))
            .filter(|&m| m >= 0)
            .map_or(-1, |m| m * 60)
    };
    b.time_to_full_seconds = if charging {
        minutes(c"Time to Full Charge")
    } else {
        -1
    };
    b.time_to_empty_seconds = if b.status == status::DISCHARGING {
        minutes(c"Time to Empty")
    } else {
        -1
    };
    if let Some(v) = number(d.get(c"Voltage")) {
        b.voltage_millivolts = v as i32;
    }
    if let Some(ma) = number(d.get(c"Current")) {
        b.current_microamps = (ma * 1000) as i32;
    }
    // macOS publishes these only once the battery has degraded.
    let health = d.get(c"BatteryHealth");
    b.health = if string_is(health, c"Fair") {
        condition::FAIR
    } else if string_is(health, c"Poor") || !d.get(c"BatteryHealthCondition").is_null() {
        condition::UNSPECIFIED_FAILURE
    } else {
        condition::GOOD
    };
}

fn from_smart_battery(b: &mut Battery) {
    // SAFETY: the matching dictionary is consumed by the lookup; the service
    // and every property are released; `data` outlives its borrowed values.
    unsafe {
        let service =
            IOServiceGetMatchingService(0, IOServiceMatching(c"AppleSmartBattery".as_ptr()));
        if service == 0 {
            return;
        }
        let prop = |key: &CStr| {
            Owned::new(IORegistryEntryCreateCFProperty(
                service,
                Owned::string(key).0,
                std::ptr::null(),
                0,
            ))
        };
        let top = |key: &CStr| prop(key).and_then(|v| number(v.0));
        // Capacities in mAh: in "BatteryData" on current macOS, as
        // AppleRaw* properties on older releases.
        let data = prop(c"BatteryData");
        let capacity = |key: &CStr, legacy: &CStr| {
            data.as_ref()
                .and_then(|d| number(Description(d.0).get(key)))
                .or_else(|| top(legacy))
                .map(|mah| (mah * 1000).clamp(0, i32::MAX as i64) as i32)
        };
        if let Some(c) = capacity(c"RemainingCapacity", c"AppleRawCurrentCapacity") {
            b.charge_counter_uah = c;
        }
        if let Some(c) = capacity(c"FullChargeCapacity", c"AppleRawMaxCapacity") {
            b.full_charge_uah = c;
        }
        if let Some(c) = capacity(c"DesignCapacity", c"DesignCapacity") {
            b.full_charge_design_uah = c;
        }
        if let Some(t) = top(c"Temperature") {
            b.temperature_tenths_celsius = (t / 10) as i32; // hundredths of a degree
        }
        if let Some(c) = top(c"CycleCount") {
            b.cycle_count = c as i32;
        }
        if b.voltage_millivolts == 0
            && let Some(v) = top(c"Voltage")
        {
            b.voltage_millivolts = v as i32;
        }
        if b.current_microamps == 0
            && let Some(ma) = top(c"Amperage")
        {
            b.current_microamps = (ma * 1000) as i32;
        }
        IOObjectRelease(service);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_is_consistent() {
        let b = battery();
        eprintln!("{b:?}");
        if b.present == 0 {
            assert_eq!(b.health, condition::NOT_AVAILABLE);
            return;
        }
        assert!((0..=100).contains(&b.level_percent));
        assert!((status::UNKNOWN..=status::FULL).contains(&b.status));
        assert!(b.full_charge_design_uah >= 0 && b.charge_counter_uah >= 0);
    }

    #[test]
    fn call_checks_the_argument_block() {
        let mut b = Battery::default();
        let p = &mut b as *mut Battery as u64;
        // SAFETY: a live, correctly sized block; the bad calls never touch it.
        unsafe {
            assert_eq!(call(FN_BATTERY, p, 63), -(errno::EINVAL as i64));
            assert_eq!(call(FN_BATTERY, 0, 64), -(errno::EINVAL as i64));
            assert_eq!(call(99, p, 64), -(errno::ENOSYS as i64));
            assert_eq!(call(FN_BATTERY, p, 64), 0);
        }
        assert_ne!(b.status, 0);
    }
}
