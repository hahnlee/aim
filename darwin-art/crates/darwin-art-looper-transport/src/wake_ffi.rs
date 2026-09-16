//! Narrow native resource ABI. Errors are returned as host errno values, not TLS.
use crate::Wake;
use std::os::fd::AsRawFd;

fn error(error: std::io::Error) -> i32 {
    error.raw_os_error().unwrap_or(libc::EIO)
}

/// # Safety
/// `out` must be null or writable for one pointer. The returned owner must be
/// destroyed once, after all concurrent operations have completed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_wake_create(out: *mut *mut Wake) -> i32 {
    if out.is_null() {
        return libc::EINVAL;
    }
    // SAFETY: caller provides writable output storage.
    unsafe {
        *out = std::ptr::null_mut();
    }
    match Wake::new() {
        Ok(wake) => {
            // SAFETY: output is writable; ownership is transferred to caller.
            unsafe {
                *out = Box::into_raw(Box::new(wake));
            }
            0
        }
        Err(e) => error(e),
    }
}

/// # Safety
/// `wake` is null or a live pointer returned by create, consumed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_wake_destroy(wake: *mut Wake) {
    if !wake.is_null() {
        // SAFETY: caller transfers unique ownership, with no in-flight operations.
        drop(unsafe { Box::from_raw(wake) });
    }
}

/// # Safety
/// `wake` must remain live during the call. Result is borrowed, never close/read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_wake_fd(wake: *const Wake) -> i32 {
    // SAFETY: caller guarantees pointer lifetime; null is handled explicitly.
    unsafe { wake.as_ref() }.map_or(-1, |w| w.poll_fd().as_raw_fd())
}

/// # Safety
/// `wake` is null or live; concurrent signal/drain calls are supported.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_wake_signal(wake: *const Wake, n: u64) -> i32 {
    // SAFETY: caller guarantees pointer lifetime.
    let Some(wake) = (unsafe { wake.as_ref() }) else {
        return libc::EINVAL;
    };
    wake.signal(n).map_or_else(error, |()| 0)
}

/// # Safety
/// `wake` is null or live. `out` is null or writable for one u64 and must not
/// alias the wake object. Output is only modified on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_looper_wake_drain(wake: *const Wake, out: *mut u64) -> i32 {
    if out.is_null() {
        return libc::EINVAL;
    }
    // SAFETY: caller guarantees pointer lifetime.
    let Some(wake) = (unsafe { wake.as_ref() }) else {
        return libc::EINVAL;
    };
    match wake.drain() {
        Ok(n) => {
            // SAFETY: caller guarantees writable nonaliasing output.
            unsafe {
                *out = n;
            }
            0
        }
        Err(e) => error(e),
    }
}
