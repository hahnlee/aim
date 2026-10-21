//! The Mac's power assertions (IOKit's `IOPMAssertionCreateWithName`),
//! which keep the Mac from what an Android device would not do while
//! Android asks it not to (docs/power.md, "The Mac's part"). An assertion
//! is released when dropped, and powerd drops a dead process's.

use std::ffi::{CString, c_char, c_void};

type CFStringRef = *const c_void;

aim_hostcall::dylib! {
    static IOKIT = c"/System/Library/Frameworks/IOKit.framework/IOKit" {
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, encoding: u32) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
        fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
}

const UTF8: u32 = 0x0800_0100;
/// `kIOPMAssertionLevelOn`.
const LEVEL_ON: u32 = 255;
/// `kIOPMAssertionTypePreventUserIdleDisplaySleep`.
const PREVENT_USER_IDLE_DISPLAY_SLEEP: &std::ffi::CStr = c"PreventUserIdleDisplaySleep";

/// An assertion held until dropped.
#[derive(Debug)]
pub struct Assertion(u32);

impl Assertion {
    /// Keeps the Mac's display from sleeping for the user's idleness,
    /// named `name` in `pmset -g assertions`; an IOKit error otherwise.
    pub fn prevent_display_sleep(name: &str) -> Result<Assertion, i32> {
        let name = CString::new(name).map_err(|_| -1)?;
        let mut id = 0;
        // SAFETY: two CFStrings made from NUL-terminated UTF-8 and released
        // after the call, which copies them; `id` is written on success.
        let status = unsafe {
            let kind = CFStringCreateWithCString(
                std::ptr::null(),
                PREVENT_USER_IDLE_DISPLAY_SLEEP.as_ptr(),
                UTF8,
            );
            let name = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8);
            let status = IOPMAssertionCreateWithName(kind, LEVEL_ON, name, &mut id);
            CFRelease(name);
            CFRelease(kind);
            status
        };
        match status {
            0 => Ok(Assertion(id)),
            e => Err(e),
        }
    }
}

impl Drop for Assertion {
    fn drop(&mut self) {
        // SAFETY: an assertion this process created and has not released.
        let status = unsafe { IOPMAssertionRelease(self.0) };
        if status != 0 {
            eprintln!(
                "power assertion {}: release: IOKit error {status:#x}",
                self.0
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An assertion is made and released, as `pmset -g assertions` would
    /// show it while held; nothing of the Mac's settings changes.
    #[test]
    fn a_display_assertion_is_made_and_released() {
        let assertion = Assertion::prevent_display_sleep("aim-services test").unwrap();
        assert_ne!(assertion.0, 0);
    }
}
