//! The Mac's preferred languages (`CFLocaleCopyPreferredLanguages`) and
//! region (`AppleLocale` of the global domain), read through
//! CoreFoundation, and their changes: the distributed
//! `AppleLanguagePreferencesChangedNotification` macOS posts when the
//! list changes, and CoreFoundation's own
//! `kCFLocaleCurrentLocaleDidChangeNotification` (NSCurrentLocaleDidChange),
//! heard on a thread of its own that runs a CFRunLoop. Nothing here
//! changes them.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

type CFTypeRef = *const c_void;

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        static kCFPreferencesAnyApplication: CFTypeRef;
        static kCFLocaleCurrentLocaleDidChangeNotification: CFTypeRef;
        fn CFLocaleCopyPreferredLanguages() -> CFTypeRef;
        fn CFPreferencesAppSynchronize(app: CFTypeRef) -> bool;
        fn CFPreferencesCopyAppValue(key: CFTypeRef, app: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFNotificationCenterGetDistributedCenter() -> CFTypeRef;
        fn CFNotificationCenterGetLocalCenter() -> CFTypeRef;
        fn CFNotificationCenterAddObserver(
            center: CFTypeRef,
            observer: *const c_void,
            callback: extern "C" fn(CFTypeRef, *mut c_void, CFTypeRef, *const c_void, CFTypeRef),
            name: CFTypeRef,
            object: *const c_void,
            behavior: isize,
        );
        #[cfg(test)]
        fn CFNotificationCenterPostNotification(
            center: CFTypeRef,
            name: CFTypeRef,
            object: *const c_void,
            info: CFTypeRef,
            deliver_immediately: bool,
        );
        fn CFRunLoopRun();
    }
}

const UTF8: u32 = 0x0800_0100;
/// `CFNotificationSuspensionBehaviorDeliverImmediately`.
const DELIVER_IMMEDIATELY: isize = 4;
static APPLE_LOCALE: OnceLock<usize> = OnceLock::new();
static LANGUAGES_CHANGED: OnceLock<usize> = OnceLock::new();

/// A CFString, made once and kept for the process's life.
fn cf_string(cell: &'static OnceLock<usize>, s: &CStr) -> CFTypeRef {
    // SAFETY: a NUL-terminated UTF-8 literal.
    *cell.get_or_init(|| unsafe {
        CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) as usize
    }) as CFTypeRef
}

/// The Mac's preferred languages as the device's locale list
/// ([`super::android_locales`]).
pub fn preferred_locales() -> Vec<String> {
    // SAFETY: the copied (+1) values are released; array elements are
    // borrowed while the array lives.
    unsafe {
        let any = *kCFPreferencesAnyApplication();
        CFPreferencesAppSynchronize(any);
        let mut languages = Vec::new();
        let array = CFLocaleCopyPreferredLanguages();
        if !array.is_null() {
            if CFGetTypeID(array) == CFArrayGetTypeID() {
                languages = (0..CFArrayGetCount(array))
                    .filter_map(|i| to_string(CFArrayGetValueAtIndex(array, i)))
                    .collect();
            }
            CFRelease(array);
        }
        let value = CFPreferencesCopyAppValue(cf_string(&APPLE_LOCALE, c"AppleLocale"), any);
        let apple_locale = to_string(value);
        if !value.is_null() {
            CFRelease(value);
        }
        super::android_locales(&languages, apple_locale.as_deref())
    }
}

unsafe fn to_string(value: CFTypeRef) -> Option<String> {
    // SAFETY: a valid CF value, checked to be a string.
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8)
            .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// What a possible change is told to.
type OnChange = Box<dyn Fn() + Send + Sync>;

extern "C" fn changed(
    _center: CFTypeRef,
    observer: *mut c_void,
    _name: CFTypeRef,
    _object: *const c_void,
    _info: CFTypeRef,
) {
    // SAFETY: the observer is the leaked `OnChange` of `watch`.
    let on_change = unsafe { &*(observer as *const OnChange) };
    on_change();
}

/// Calls `on_change` each time the Mac's languages or region may have
/// changed, for the life of the process.
pub fn watch(on_change: OnChange) {
    let observer = Box::into_raw(Box::new(on_change)) as usize;
    std::thread::Builder::new()
        .name("locale-mac".into())
        .spawn(move || {
            // SAFETY: the observer lives for the process; the run loop is
            // this thread's, so the distributed notification comes here.
            unsafe {
                let add = |center, name| {
                    CFNotificationCenterAddObserver(
                        center,
                        observer as *const c_void,
                        changed,
                        name,
                        std::ptr::null(),
                        DELIVER_IMMEDIATELY,
                    )
                };
                add(
                    CFNotificationCenterGetDistributedCenter(),
                    cf_string(
                        &LANGUAGES_CHANGED,
                        c"AppleLanguagePreferencesChangedNotification",
                    ),
                );
                add(
                    CFNotificationCenterGetLocalCenter(),
                    *kCFLocaleCurrentLocaleDidChangeNotification(),
                );
                CFRunLoopRun();
            }
        })
        .expect("spawn the language thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn reads_the_preferred_languages() {
        // Whatever the Mac has, each is a tag the mapping made, once.
        let locales = preferred_locales();
        eprintln!("the Mac's languages: {locales:?}");
        for (i, tag) in locales.iter().enumerate() {
            assert_eq!(super::super::android_locale(tag, None).as_ref(), Some(tag));
            assert!(!locales[..i].contains(tag), "{tag} repeated");
        }
    }

    #[test]
    fn a_current_locale_change_is_heard() {
        // Posted to this process's local center only: the Mac's settings
        // and other processes are not involved.
        let (sender, received) = mpsc::sync_channel(16);
        watch(Box::new(move || {
            let _ = sender.try_send(());
        }));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // SAFETY: CF's own notification name, posted without info.
            unsafe {
                CFNotificationCenterPostNotification(
                    CFNotificationCenterGetLocalCenter(),
                    *kCFLocaleCurrentLocaleDidChangeNotification(),
                    std::ptr::null(),
                    std::ptr::null(),
                    true,
                );
            }
            if received.recv_timeout(Duration::from_millis(100)).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "no change heard");
        }
    }
}
