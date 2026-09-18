//! Production FFI declarations for the Android binary128 conversion provider.
//!
//! The implementation remains in the native provider archive.  This library
//! target is the production-facing contract; the audit executable and its
//! host-state fixture are opt-in through the `audit` feature.

use std::ffi::{c_char, c_void};

unsafe extern "C" {
    pub fn darwin_art_bionic_binary128_conversion_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;

    pub fn darwin_art_bionic_binary128_conversion_capability(capability: *const c_char) -> i32;
}
