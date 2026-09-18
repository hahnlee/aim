//! Android application-process lifetime at the Darwin process boundary.

/// AOSP application threads and loaded native images live until process exit.
/// Do not run Rust/C++ destructors or DestroyJavaVM at this boundary.
pub(crate) fn exit_android_process(status: i32) -> ! {
    unsafe {
        libc::fflush(std::ptr::null_mut());
        libc::_exit(status);
    }
}
