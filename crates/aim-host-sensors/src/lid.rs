//! The lid angle of MacBooks that have a lid angle sensor (2019 16-inch
//! MacBook Pro and later): a HID sensor device (usage page Sensor, usage
//! Orientation) whose feature report 1 carries the angle in degrees as a
//! little-endian u16 after the report id. It is read with the public
//! `IOHIDManager` API and needs no permission.

use std::sync::{Mutex, OnceLock};

use crate::cf::{CFTypeRef, Owned};

const VENDOR_APPLE: i32 = 0x05ac;
const PRODUCT_LID_SENSOR: i32 = 0x8104;
const PAGE_SENSOR: i32 = 0x20;
const USAGE_ORIENTATION: i32 = 0x8a;
const REPORT_TYPE_FEATURE: u32 = 2;
const REPORT_ANGLE: isize = 1;

aim_hostcall::dylib! {
    static IO_KIT = c"/System/Library/Frameworks/IOKit.framework/IOKit" {
        fn IOHIDManagerCreate(alloc: CFTypeRef, options: u32) -> CFTypeRef;
        fn IOHIDManagerSetDeviceMatching(manager: CFTypeRef, matching: CFTypeRef);
        fn IOHIDManagerCopyDevices(manager: CFTypeRef) -> CFTypeRef;
        fn IOHIDDeviceOpen(device: CFTypeRef, options: u32) -> i32;
        fn IOHIDDeviceGetReport(
            device: CFTypeRef,
            report_type: u32,
            report_id: isize,
            report: *mut u8,
            len: *mut isize,
        ) -> i32;
    }
}

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        fn CFSetGetCount(set: CFTypeRef) -> isize;
        fn CFSetGetValues(set: CFTypeRef, values: *mut CFTypeRef);
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    }
}

/// The opened sensor device (and its manager, which owns the connection).
struct Sensor {
    _manager: Owned,
    device: Owned,
}

fn open() -> Option<Sensor> {
    // SAFETY: owned manager, matching dictionary and device set; the one
    // device kept is retained.
    unsafe {
        let manager = Owned::new(IOHIDManagerCreate(std::ptr::null(), 0))?;
        let matching = Owned::matching(&[
            (c"VendorID", VENDOR_APPLE),
            (c"ProductID", PRODUCT_LID_SENSOR),
            (c"PrimaryUsagePage", PAGE_SENSOR),
            (c"PrimaryUsage", USAGE_ORIENTATION),
        ]);
        IOHIDManagerSetDeviceMatching(manager.0, matching.0);
        let devices = Owned::new(IOHIDManagerCopyDevices(manager.0))?;
        if CFSetGetCount(devices.0) < 1 {
            return None;
        }
        let mut device = std::ptr::null();
        CFSetGetValues(devices.0, &mut device);
        let device = Owned(CFRetain(device));
        (IOHIDDeviceOpen(device.0, 0) == 0).then_some(Sensor {
            _manager: manager,
            device,
        })
    }
}

/// The lid angle in degrees, or `None` on a Mac without the sensor.
pub fn angle() -> Option<f32> {
    static SENSOR: OnceLock<Mutex<Option<Sensor>>> = OnceLock::new();
    let sensor = SENSOR.get_or_init(|| Mutex::new(open())).lock().unwrap();
    let sensor = sensor.as_ref()?;
    let mut report = [0u8; 8];
    let mut len = report.len() as isize;
    // SAFETY: an open device and a local buffer of `len` bytes.
    let r = unsafe {
        IOHIDDeviceGetReport(
            sensor.device.0,
            REPORT_TYPE_FEATURE,
            REPORT_ANGLE,
            report.as_mut_ptr(),
            &mut len,
        )
    };
    (r == 0 && len >= 3).then(|| u16::from_le_bytes([report[1], report[2]]) as f32)
}
