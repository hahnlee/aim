//! Production FFI declarations for the Android float-conversion provider.
//!
//! The implementation lives in the native provider archive.  Keeping these
//! declarations in the library target lets production consumers link the
//! provider without enabling the audit executable or its fixture helpers.

use std::ffi::{c_char, c_void};

unsafe extern "C" {
    pub fn darwin_art_bionic_float_conversion_resolve(
        soname: *const c_char,
        symbol: *const c_char,
        version: *const c_char,
    ) -> *mut c_void;

    pub fn darwin_art_bionic_float_conversion_capability(capability: *const c_char) -> i32;
}
