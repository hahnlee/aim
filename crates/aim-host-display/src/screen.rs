//! Whether the Mac shows windows now: while its screen is locked or its
//! main display asleep, Core Animation shows no window's drawable, and a
//! drawable presented then waits, its presented handler unrun, until the
//! screen shows windows again.

use std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFBooleanGetValue(b: *const c_void) -> bool;
    fn CFRelease(obj: *const c_void);
}

const UTF8: u32 = 0x0800_0100;

/// Why the Mac shows no windows now, if it does not: its main display is
/// asleep, or its screen is locked.
pub fn hidden() -> Option<&'static str> {
    // SAFETY: plain queries; the session dictionary and key are released.
    unsafe {
        if CGDisplayIsAsleep(CGMainDisplayID()) != 0 {
            return Some("the Mac's main display is asleep");
        }
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() {
            return Some("no window server session");
        }
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"CGSSessionScreenIsLocked".as_ptr(), UTF8);
        let v = CFDictionaryGetValue(session, key);
        let locked = !v.is_null() && CFBooleanGetValue(v);
        CFRelease(key);
        CFRelease(session);
        locked.then_some("the Mac's screen is locked")
    }
}
