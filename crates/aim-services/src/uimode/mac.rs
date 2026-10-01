//! The Mac's appearance, which owns dark mode: `AppleInterfaceStyle` of
//! the global domain (`Dark`, or absent in Light; macOS keeps it current
//! when the appearance is Auto), read through CFPreferences, with the
//! distributed notification macOS posts on each change
//! (`AppleInterfaceThemeChangedNotification`), heard on a thread of its
//! own that runs a CFRunLoop. Android never changes it.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

type CFTypeRef = *const c_void;

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        static kCFPreferencesAnyApplication: CFTypeRef;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFPreferencesAppSynchronize(app: CFTypeRef) -> bool;
        fn CFPreferencesCopyAppValue(key: CFTypeRef, app: CFTypeRef) -> CFTypeRef;
        fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> bool;
        fn CFRelease(cf: CFTypeRef);
        fn CFNotificationCenterGetDistributedCenter() -> CFTypeRef;
        fn CFNotificationCenterAddObserver(
            center: CFTypeRef,
            observer: *const c_void,
            callback: extern "C" fn(CFTypeRef, *mut c_void, CFTypeRef, *const c_void, CFTypeRef),
            name: CFTypeRef,
            object: *const c_void,
            behavior: isize,
        );
        fn CFRunLoopRun();
    }
}

const UTF8: u32 = 0x0800_0100;
/// `CFNotificationSuspensionBehaviorDeliverImmediately`.
const DELIVER_IMMEDIATELY: isize = 4;
static STYLE: OnceLock<usize> = OnceLock::new();
static DARK: OnceLock<usize> = OnceLock::new();
static CHANGED: OnceLock<usize> = OnceLock::new();

/// A CFString, made once and kept for the process's life.
fn cf_string(cell: &'static OnceLock<usize>, s: &CStr) -> CFTypeRef {
    // SAFETY: a NUL-terminated UTF-8 literal.
    *cell.get_or_init(|| unsafe {
        CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) as usize
    }) as CFTypeRef
}

/// Whether the Mac's appearance is Dark now.
pub fn dark() -> bool {
    let style = cf_string(&STYLE, c"AppleInterfaceStyle");
    let dark_style = cf_string(&DARK, c"Dark");
    // SAFETY: the preference is copied (+1) and released.
    unsafe {
        let any = *kCFPreferencesAnyApplication();
        CFPreferencesAppSynchronize(any);
        let value = CFPreferencesCopyAppValue(style, any);
        if value.is_null() {
            return false;
        }
        let dark = CFEqual(value, dark_style);
        CFRelease(value);
        dark
    }
}

/// What a change is told to.
type OnChange = Box<dyn Fn(bool) + Send + Sync>;

extern "C" fn changed(
    _center: CFTypeRef,
    observer: *mut c_void,
    _name: CFTypeRef,
    _object: *const c_void,
    _info: CFTypeRef,
) {
    // SAFETY: the observer is the leaked `OnChange` of `watch`.
    let on_change = unsafe { &*(observer as *const OnChange) };
    on_change(dark());
}

/// Calls `on_change` with the appearance each time the Mac changes it,
/// for the life of the process.
pub fn watch(on_change: OnChange) {
    let observer = Box::into_raw(Box::new(on_change)) as usize;
    std::thread::Builder::new()
        .name("uimode-mac".into())
        .spawn(move || {
            // SAFETY: the observer lives for the process; the run loop is
            // this thread's, so the notification comes here.
            unsafe {
                CFNotificationCenterAddObserver(
                    CFNotificationCenterGetDistributedCenter(),
                    observer as *const c_void,
                    changed,
                    cf_string(&CHANGED, c"AppleInterfaceThemeChangedNotification"),
                    std::ptr::null(),
                    DELIVER_IMMEDIATELY,
                );
                CFRunLoopRun();
            }
        })
        .expect("spawn the appearance thread");
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_appearance() {
        // Either is valid; the read must not fail or hang.
        eprintln!("dark: {}", super::dark());
    }
}
