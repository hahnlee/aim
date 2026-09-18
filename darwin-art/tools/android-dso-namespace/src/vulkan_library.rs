//! Logical Vulkan library boundary. Backend residency belongs to the existing
//! process-wide MoltenVK owner; namespace images must not unload that owner.
use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr;

use crate::vulkan_provider::{ready, symbol_lookup};

#[no_mangle]
pub extern "C" fn darwin_art_bionic_vulkan_provider_ready() -> c_int {
    i32::from(ready())
}

/// Resolve only from the real backend, never from no-driver fallback PFNs.
/// # Safety
/// Names must be NUL-terminated if non-null; output must be writable.
#[no_mangle]
pub unsafe extern "C" fn darwin_art_bionic_vulkan_symbol(
    symbol: *const c_char,
    version: *const c_char,
    output: *mut usize,
) -> c_int {
    if output.is_null() {
        return 3;
    }
    unsafe {
        *output = 0;
    }
    if symbol.is_null() {
        return 3;
    }
    let symbol = unsafe { CStr::from_ptr(symbol) };
    if symbol.to_bytes().is_empty() {
        return 3;
    }
    if !version.is_null() {
        return 2;
    }
    if !ready() {
        return 1;
    }
    let address = unsafe { symbol_lookup(ptr::null_mut(), symbol.as_ptr()) };
    if address.is_null() {
        return 1;
    }
    unsafe {
        *output = address as usize;
    }
    0
}

/// Legacy libdl caller retains its defined no-driver error entrypoints. Those
/// pointers are deliberately not used as proof that a real provider is ready.
pub(super) unsafe fn legacy_lookup(symbol: *const c_char) -> *mut c_void {
    if symbol.is_null() {
        super::set_error("null Android Vulkan DSO symbol");
        return ptr::null_mut();
    }
    let result = unsafe { symbol_lookup(ptr::null_mut(), symbol) };
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        let name = unsafe { CStr::from_ptr(symbol) };
        eprintln!(
            "ART Android libdl: pid={} libvulkan.so dlsym {} resolved={}",
            std::process::id(),
            name.to_string_lossy(),
            !result.is_null()
        );
    }
    if result.is_null() {
        super::set_error("Android Vulkan entrypoint is unavailable");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fallback_pointer_does_not_make_real_provider_ready() {
        assert_eq!(darwin_art_bionic_vulkan_provider_ready(), 0);
        assert!(!unsafe { legacy_lookup(c"vkCreateInstance".as_ptr()) }.is_null());
        let mut out = usize::MAX;
        assert_eq!(
            unsafe {
                darwin_art_bionic_vulkan_symbol(c"vkCreateInstance".as_ptr(), ptr::null(), &mut out)
            },
            1
        );
        assert_eq!(out, 0);
    }
    #[test]
    fn rejects_bad_arguments_and_versions_before_backend_lookup() {
        let mut out = usize::MAX;
        assert_eq!(
            unsafe { darwin_art_bionic_vulkan_symbol(ptr::null(), ptr::null(), &mut out) },
            3
        );
        assert_eq!(out, 0);
        assert_eq!(
            unsafe {
                darwin_art_bionic_vulkan_symbol(
                    c"vkCreateInstance".as_ptr(),
                    c"LIBVULKAN".as_ptr(),
                    &mut out,
                )
            },
            2
        );
        assert_eq!(out, 0);
        assert_eq!(
            unsafe { darwin_art_bionic_vulkan_symbol(c"".as_ptr(), ptr::null(), &mut out) },
            3
        );
        assert_eq!(
            unsafe {
                darwin_art_bionic_vulkan_symbol(
                    c"vkCreateInstance".as_ptr(),
                    ptr::null(),
                    ptr::null_mut(),
                )
            },
            3
        );
    }
}
