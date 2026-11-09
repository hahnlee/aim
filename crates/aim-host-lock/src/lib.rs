//! The Mac's lock as Android's `trust` service sees it. The app host runs
//! inside the Mac's user session, whose lock is the device's: the device
//! is secure when the Mac's user has a password, as LocalAuthentication
//! tells (`LAContext canEvaluatePolicy:error:` with
//! `LAPolicyDeviceOwnerAuthentication`, which fails with
//! `LAErrorPasscodeNotSet` without one). Read when asked, never cached;
//! asking evaluates nothing and shows no prompt. The session's screen lock
//! is the device's keyguard: whether it is locked now
//! (`CGSSessionScreenIsLocked`), and each locking and unlocking, heard on
//! a thread of its own that runs a CFRunLoop.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

pub mod keychain;

type Id = *mut c_void;
type Sel = *const c_void;

aim_hostcall::dylib! {
    static LOCAL_AUTHENTICATION = c"/System/Library/Frameworks/LocalAuthentication.framework/LocalAuthentication" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        static objc_msgSend: c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
}

aim_hostcall::dylib! {
    static CORE_GRAPHICS = c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics" {
        fn CGSessionCopyCurrentDictionary() -> *const c_void;
    }
}

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, encoding: u32) -> *const c_void;
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFBooleanGetValue(b: *const c_void) -> bool;
        fn CFEqual(a: *const c_void, b: *const c_void) -> bool;
        fn CFRelease(cf: *const c_void);
        fn CFNotificationCenterGetDistributedCenter() -> *const c_void;
        fn CFNotificationCenterAddObserver(
            center: *const c_void,
            observer: *const c_void,
            callback: extern "C" fn(*const c_void, *mut c_void, *const c_void, *const c_void, *const c_void),
            name: *const c_void,
            object: *const c_void,
            behavior: isize,
        );
        fn CFRunLoopRun();
    }
}

const UTF8: u32 = 0x0800_0100;
/// `CFNotificationSuspensionBehaviorDeliverImmediately`.
const DELIVER_IMMEDIATELY: isize = 4;
static SCREEN_IS_LOCKED_KEY: OnceLock<usize> = OnceLock::new();
static SCREEN_LOCKED: OnceLock<usize> = OnceLock::new();
static SCREEN_UNLOCKED: OnceLock<usize> = OnceLock::new();

/// `LAPolicyDeviceOwnerAuthentication`, `LAErrorPasscodeNotSet`.
const LA_POLICY_DEVICE_OWNER_AUTHENTICATION: isize = 2;
const LA_ERROR_PASSCODE_NOT_SET: isize = -5;

fn selector(name: &CStr) -> Sel {
    // SAFETY: registering a selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// Whether the Mac's user has a password; an error LocalAuthentication
/// gave otherwise (its `NSError` code).
pub fn secure() -> Result<bool, isize> {
    // SAFETY: LocalAuthentication is loaded; each message goes to an object
    // of the class that declares it with its documented signature, inside
    // an autorelease pool, and the context made is released.
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let result = (|| {
            let class = objc_getClass(c"LAContext".as_ptr());
            if class.is_null() {
                return Err(0);
            }
            let send0: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend());
            let context = send0(send0(class, selector(c"alloc")), selector(c"init"));
            if context.is_null() {
                return Err(0);
            }
            let can: unsafe extern "C" fn(Id, Sel, isize, *mut Id) -> bool =
                std::mem::transmute(objc_msgSend());
            let mut error: Id = std::ptr::null_mut();
            let ok = can(
                context,
                selector(c"canEvaluatePolicy:error:"),
                LA_POLICY_DEVICE_OWNER_AUTHENTICATION,
                &mut error,
            );
            let code = if ok || error.is_null() {
                0
            } else {
                let code: unsafe extern "C" fn(Id, Sel) -> isize =
                    std::mem::transmute(objc_msgSend());
                code(error, selector(c"code"))
            };
            send0(context, selector(c"release"));
            match (ok, code) {
                (true, _) => Ok(true),
                (false, LA_ERROR_PASSCODE_NOT_SET) => Ok(false),
                (false, code) => Err(code),
            }
        })();
        objc_autoreleasePoolPop(pool);
        result
    }
}

/// A CFString, made once and kept for the process's life.
fn cf_string(cell: &'static OnceLock<usize>, s: &CStr) -> *const c_void {
    // SAFETY: a NUL-terminated UTF-8 literal.
    *cell.get_or_init(|| unsafe {
        CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) as usize
    }) as *const c_void
}

/// Whether the Mac's screen is locked now; without a window server
/// session, nobody is present at it either.
pub fn screen_locked() -> bool {
    let key = cf_string(&SCREEN_IS_LOCKED_KEY, c"CGSSessionScreenIsLocked");
    // SAFETY: the session dictionary is copied (+1) and released; the
    // value it holds for the key, when any, is a CFBoolean.
    unsafe {
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() {
            return true;
        }
        let value = CFDictionaryGetValue(session, key);
        let locked = !value.is_null() && CFBooleanGetValue(value);
        CFRelease(session);
        locked
    }
}

/// What a locking (`true`) or an unlocking (`false`) is told to.
pub type OnChange = Box<dyn Fn(bool) + Send + Sync>;

extern "C" fn screen_changed(
    _center: *const c_void,
    observer: *mut c_void,
    name: *const c_void,
    _object: *const c_void,
    _info: *const c_void,
) {
    // SAFETY: the observer is the leaked `OnChange` of `watch_screen`;
    // the name is a CFString.
    let (on_change, locked) = unsafe {
        (
            &*(observer as *const OnChange),
            CFEqual(name, cf_string(&SCREEN_LOCKED, c"com.apple.screenIsLocked")),
        )
    };
    on_change(locked);
}

/// Calls `on_change` each time the Mac's screen locks or unlocks (the
/// distributed `com.apple.screenIsLocked` and `com.apple.screenIsUnlocked`),
/// for the life of the process.
pub fn watch_screen(on_change: OnChange) {
    let observer = Box::into_raw(Box::new(on_change)) as usize;
    std::thread::Builder::new()
        .name("screen-lock-mac".into())
        .spawn(move || {
            // SAFETY: the observer lives for the process; the run loop is
            // this thread's, so the notifications come here.
            unsafe {
                let center = CFNotificationCenterGetDistributedCenter();
                for name in [
                    cf_string(&SCREEN_LOCKED, c"com.apple.screenIsLocked"),
                    cf_string(&SCREEN_UNLOCKED, c"com.apple.screenIsUnlocked"),
                ] {
                    CFNotificationCenterAddObserver(
                        center,
                        observer as *const c_void,
                        screen_changed,
                        name,
                        std::ptr::null(),
                        DELIVER_IMMEDIATELY,
                    );
                }
                CFRunLoopRun();
            }
        })
        .expect("spawn the screen lock thread");
}

#[cfg(test)]
mod tests {
    /// This Mac's user answers one way or the other, without a prompt.
    #[test]
    fn the_mac_answers_whether_it_is_secure() {
        assert!(super::secure().is_ok(), "{:?}", super::secure());
    }

    /// The session answers whether its screen is locked; a test runs at an
    /// unlocked one or without one, so either is valid.
    #[test]
    fn the_mac_answers_whether_its_screen_is_locked() {
        eprintln!("locked: {}", super::screen_locked());
    }
}
