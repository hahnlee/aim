use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn realloc(pointer: *mut c_void, size: usize) -> *mut c_void;
}

unsafe fn set_status(status: *mut c_int, value: c_int) {
    if !status.is_null() {
        unsafe { *status = value };
    }
}

/// C allocation-compatible implementation of Android's rustc-demangle ABI.
/// The returned storage is deliberately allocated with libc so AOSP
/// libunwindstack can release it with `free()`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustc_demangle(
    mangled: *const c_char,
    output_buffer: *mut c_char,
    output_size: *mut usize,
    status: *mut c_int,
) -> *mut c_char {
    if mangled.is_null() || (!output_buffer.is_null() && output_size.is_null()) {
        unsafe { set_status(status, -3) };
        return ptr::null_mut();
    }
    let Ok(symbol) = unsafe { CStr::from_ptr(mangled) }.to_str() else {
        unsafe { set_status(status, -2) };
        return ptr::null_mut();
    };
    let Ok(demangled) = rustc_demangle::try_demangle(symbol) else {
        unsafe { set_status(status, -2) };
        return ptr::null_mut();
    };
    let rendered = format!("{demangled:#}");
    let required = rendered.len() + 1;
    let destination = if output_buffer.is_null() {
        unsafe { malloc(required) }.cast::<c_char>()
    } else {
        let capacity = unsafe { *output_size };
        if capacity < required {
            unsafe { realloc(output_buffer.cast::<c_void>(), required) }.cast::<c_char>()
        } else {
            output_buffer
        }
    };
    if destination.is_null() {
        unsafe { set_status(status, -1) };
        return ptr::null_mut();
    }
    unsafe {
        ptr::copy_nonoverlapping(rendered.as_ptr(), destination.cast::<u8>(), rendered.len());
        *destination.add(rendered.len()) = 0;
        if !output_size.is_null() {
            *output_size = required;
        }
        set_status(status, 0);
    }
    destination
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    unsafe extern "C" {
        fn free(pointer: *mut c_void);
    }

    #[test]
    fn matches_aosp_v0_examples() {
        for (mangled, expected) in [
            (
                "_RNvNtCs2WRBrrl1bb1_3std2rt19lang_start_internal",
                "std::rt::lang_start_internal",
            ),
            ("_RNvCs4VPobU5SDH_12profcollectd4main", "profcollectd::main"),
        ] {
            let input = CString::new(mangled).unwrap();
            let mut status = 99;
            let result = unsafe {
                rustc_demangle(
                    input.as_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut status,
                )
            };
            assert_eq!(status, 0);
            assert_eq!(
                unsafe { CStr::from_ptr(result) }.to_str().unwrap(),
                expected
            );
            unsafe { free(result.cast::<c_void>()) };
        }
    }
}
