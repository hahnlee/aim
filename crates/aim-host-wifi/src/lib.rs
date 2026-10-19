//! The Mac's Wi-Fi as Android's `wifi` service sees it: its default
//! CoreWLAN interface (`CWWiFiClient.sharedWiFiClient.interface`), the
//! interface's BSD name and whether its radio is on (`powerOn`). Read when
//! asked, never cached: the user turns the radio on and off. Reading these
//! needs no permission and shows no prompt.

use std::ffi::{CStr, c_char, c_void};

type Id = *mut c_void;
type Sel = *const c_void;

aim_hostcall::dylib! {
    static CORE_WLAN = c"/System/Library/Frameworks/CoreWLAN.framework/CoreWLAN" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        static objc_msgSend: c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
}

/// The Mac's Wi-Fi interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wifi {
    /// Its BSD name (`en0`, `en1`).
    pub interface: String,
    /// Whether its radio is on.
    pub powered: bool,
}

/// `objc_msgSend` typed as `fn(receiver, selector) -> R`.
///
/// # Safety
/// `receiver` must answer `selector`, which takes no argument and returns
/// an `R` by the AAPCS64 rules.
unsafe fn send<R>(receiver: Id, selector: &CStr) -> R {
    // SAFETY: caller contract; objc_msgSend has the callee's signature.
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> R = std::mem::transmute(objc_msgSend());
        f(receiver, sel_registerName(selector.as_ptr()))
    }
}

/// The Mac's Wi-Fi interface now; none on a Mac without Wi-Fi hardware.
pub fn wifi() -> Option<Wifi> {
    // SAFETY: CoreWLAN is loaded; each message is sent to an object of the
    // class that declares it, with its documented signature, inside an
    // autorelease pool that outlives the strings copied.
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let wifi = (|| {
            let class = objc_getClass(c"CWWiFiClient".as_ptr());
            if class.is_null() {
                return None;
            }
            let client: Id = send(class, c"sharedWiFiClient");
            let interface: Id = send(client, c"interface");
            if interface.is_null() {
                return None;
            }
            let name: Id = send(interface, c"interfaceName");
            if name.is_null() {
                return None;
            }
            let utf8: *const c_char = send(name, c"UTF8String");
            Some(Wifi {
                interface: CStr::from_ptr(utf8).to_string_lossy().into_owned(),
                powered: send::<bool>(interface, c"powerOn"),
            })
        })();
        objc_autoreleasePoolPop(pool);
        wifi
    }
}

#[cfg(test)]
mod tests {
    /// The Mac's interface, if it has one, is named as a BSD interface.
    #[test]
    fn reads_the_macs_interface() {
        if let Some(w) = super::wifi() {
            assert!(w.interface.starts_with("en"), "{w:?}");
            eprintln!("{w:?}");
        }
    }
}
