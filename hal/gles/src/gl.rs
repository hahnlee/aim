//! GLES entry points the guest implements: debug callbacks, which the host
//! never calls (host code does not call guest code). Messages stay
//! available through `glGetDebugMessageLog`.

use core::ffi::c_void;

/// # Safety
/// Any arguments; they are not used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glDebugMessageCallback(_callback: *const c_void, _user: *const c_void) {}

/// # Safety
/// Any arguments; they are not used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn glDebugMessageCallbackKHR(_callback: *const c_void, _user: *const c_void) {
}
