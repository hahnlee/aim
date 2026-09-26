//! `__system_property_set` for Darwin processes (ADR 0009). init's property
//! service is the profile daemon; the request travels over the profile
//! protocol instead of init's /dev/socket/property_service. As with init,
//! the setter observes an accepted value once the service has replied.
use darwin_art_profile::property_set_at;
use std::ffi::{CStr, c_char, c_int};
use std::path::Path;

/// system_properties.h PROP_SUCCESS.
const PROP_SUCCESS: u32 = 0;
const ANDROID_EINVAL: i32 = 22;
const ANDROID_ECONNREFUSED: i32 = 111;

unsafe extern "C" {
    fn darwin_art_bionic_errno_store(android_errno: i32);
    fn darwin_art_bionic_process_property_apply_service_update_core(
        name: *const c_char,
        value: *const c_char,
    ) -> c_int;
}

/// Bionic `__system_property_set`: 0 when the property service accepted the
/// value, -1 otherwise. As in bionic, a service rejection leaves errno 0 and
/// a transport failure sets it.
///
/// # Safety
/// `name` and `value` must be NUL-terminated strings (`value` may be null,
/// which Android treats as the empty string).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_bionic_property_service_set(
    name: *const c_char,
    value: *const c_char,
) -> c_int {
    let fail = |errno: i32| {
        // SAFETY: the Bionic errno owner is linked into every runtime image.
        unsafe { darwin_art_bionic_errno_store(errno) };
        -1
    };
    std::panic::catch_unwind(|| {
        if name.is_null() {
            return fail(ANDROID_EINVAL);
        }
        // SAFETY: caller contract.
        let name_text = unsafe { CStr::from_ptr(name) };
        let value_text = if value.is_null() {
            c""
        } else {
            unsafe { CStr::from_ptr(value) }
        };
        let (Ok(name_str), Ok(value_str)) = (name_text.to_str(), value_text.to_str()) else {
            return fail(ANDROID_EINVAL);
        };
        let Some(socket) = std::env::var_os("DARWIN_ART_PROFILE_SOCKET") else {
            return fail(ANDROID_ECONNREFUSED);
        };
        match property_set_at(Path::new(&socket), name_str, value_str) {
            Ok(PROP_SUCCESS) => {
                // SAFETY: both strings are NUL-terminated.
                unsafe {
                    darwin_art_bionic_process_property_apply_service_update_core(
                        name_text.as_ptr(),
                        value_text.as_ptr(),
                    )
                }
            }
            Ok(code) => {
                eprintln!("property service rejected {name_str}={value_str}: 0x{code:x}");
                fail(0)
            }
            Err(_) => fail(ANDROID_ECONNREFUSED),
        }
    })
    .unwrap_or(-1)
}
