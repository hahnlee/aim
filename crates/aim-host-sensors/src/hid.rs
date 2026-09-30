//! The HID event system's sensor services: temperatures and the ambient
//! light sensor.
//!
//! On Apple silicon these sensors are HID event services (usage page
//! 0xff00), not HID devices, so the public `IOHIDManager` cannot read them.
//! The `IOHIDEventSystemClient` calls that can are exported by IOKit but are
//! private API: undocumented, with no compatibility promise (they need no
//! privileges, and third-party temperature monitors rely on them). They are
//! optional: they are looked up at run time and never linked, so a macOS
//! without them, or a call that yields nothing, just means no such readings
//! (docs/vendor-hals.md, "Private API").

use std::ffi::{CStr, c_void};
use std::sync::{Mutex, OnceLock};

use crate::cf::{self, CFArrayGetCount, CFArrayGetValueAtIndex, CFTypeRef, Owned};

/// `kHIDPage_AppleVendor` usages of the sensor services.
const PAGE_APPLE_VENDOR: i32 = 0xff00;
pub const USAGE_TEMPERATURE: i32 = 5;
pub const USAGE_AMBIENT_LIGHT: i32 = 4;
/// `IOHIDEventType`s; a type's first field is `type << 16`.
pub const EVENT_TEMPERATURE: i64 = 15;
pub const EVENT_AMBIENT_LIGHT: i64 = 12;

struct Api {
    create: unsafe extern "C" fn(CFTypeRef) -> CFTypeRef,
    set_matching: unsafe extern "C" fn(CFTypeRef, CFTypeRef) -> i32,
    copy_services: unsafe extern "C" fn(CFTypeRef) -> CFTypeRef,
    copy_property: unsafe extern "C" fn(CFTypeRef, CFTypeRef) -> CFTypeRef,
    copy_event: unsafe extern "C" fn(CFTypeRef, i64, i32, i64) -> CFTypeRef,
    float_value: unsafe extern "C" fn(CFTypeRef, u32) -> f64,
}

/// The exported function `name`, typed as `F`.
///
/// # Safety
/// `F` must be an `extern "C"` function pointer type with the export's
/// signature.
unsafe fn symbol<F: Copy>(name: &CStr) -> Option<F> {
    const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
    // SAFETY: a NUL-terminated name; IOKit is open ([`api`]), so its
    // exports are in the default namespace.
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    // SAFETY: caller contract; a non-null export is a function of type F.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) })
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        // IOKit is opened on first use (aim_hostcall::dylib), and these
        // lookups are not declared there, so open it first.
        if !crate::lid::IO_KIT.load() {
            return None;
        }
        // SAFETY: each symbol has the signature IOKit has exported since
        // macOS 10.8.
        unsafe {
            Some(Api {
                create: symbol(c"IOHIDEventSystemClientCreate")?,
                set_matching: symbol(c"IOHIDEventSystemClientSetMatching")?,
                copy_services: symbol(c"IOHIDEventSystemClientCopyServices")?,
                copy_property: symbol(c"IOHIDServiceClientCopyProperty")?,
                copy_event: symbol(c"IOHIDServiceClientCopyEvent")?,
                float_value: symbol(c"IOHIDEventGetFloatValue")?,
            })
        }
    })
    .as_ref()
}

/// The services of one usage, each with its product name. Looked up once:
/// these are built-in sensors.
pub struct Services {
    services: Owned,
    /// The services read through the client's connection, so it lives as
    /// long as they do (it is dropped after them).
    _client: Owned,
    pub names: Vec<String>,
}

impl Services {
    fn find(usage: i32) -> Option<Self> {
        let api = api()?;
        // SAFETY: the client and service list are owned; the list retains
        // its services, and we keep the client they need.
        unsafe {
            let client = Owned::new((api.create)(std::ptr::null()))?;
            let matching = Owned::matching(&[
                (c"PrimaryUsagePage", PAGE_APPLE_VENDOR),
                (c"PrimaryUsage", usage),
            ]);
            (api.set_matching)(client.0, matching.0);
            let services = Owned::new((api.copy_services)(client.0))?;
            let product = Owned::string(c"Product");
            let names = (0..CFArrayGetCount(services.0))
                .map(|i| {
                    let service = CFArrayGetValueAtIndex(services.0, i);
                    let name = Owned::new((api.copy_property)(service, product.0));
                    name.and_then(|n| cf::string(n.0)).unwrap_or_default()
                })
                .collect();
            Some(Self {
                services,
                _client: client,
                names,
            })
        }
    }

    /// The first field of service `i`'s current event of type `event`.
    pub fn read(&self, i: usize, event: i64) -> Option<f64> {
        let api = api()?;
        // SAFETY: `i` indexes the retained list; the event is owned.
        unsafe {
            let service = CFArrayGetValueAtIndex(self.services.0, i as isize);
            let e = Owned::new((api.copy_event)(service, event, 0, 0))?;
            Some((api.float_value)(e.0, (event as u32) << 16))
        }
    }
}

/// The services of `usage`, found on first use.
pub fn services(usage: i32) -> &'static Mutex<Option<Services>> {
    static TEMPERATURE: OnceLock<Mutex<Option<Services>>> = OnceLock::new();
    static LIGHT: OnceLock<Mutex<Option<Services>>> = OnceLock::new();
    let cell = if usage == USAGE_TEMPERATURE {
        &TEMPERATURE
    } else {
        &LIGHT
    };
    cell.get_or_init(|| Mutex::new(Services::find(usage)))
}
