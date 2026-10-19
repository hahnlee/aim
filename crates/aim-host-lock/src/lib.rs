//! The Mac's lock as Android's `trust` service sees it. The app host runs
//! inside the Mac's user session, whose lock is the device's: the device
//! is secure when the Mac's user has a password, as LocalAuthentication
//! tells (`LAContext canEvaluatePolicy:error:` with
//! `LAPolicyDeviceOwnerAuthentication`, which fails with
//! `LAErrorPasscodeNotSet` without one). Read when asked, never cached;
//! asking evaluates nothing and shows no prompt.

use std::ffi::{CStr, c_char, c_void};

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

#[cfg(test)]
mod tests {
    /// This Mac's user answers one way or the other, without a prompt.
    #[test]
    fn the_mac_answers_whether_it_is_secure() {
        assert!(super::secure().is_ok(), "{:?}", super::secure());
    }
}
