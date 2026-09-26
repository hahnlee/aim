//! The system server's end of the host command relay (ADR 0009): commands a
//! local client sends through the profile daemon, run as `cmd` would.
use darwin_art_profile::HostCommandListener;
use std::ffi::{c_int, c_void};
use std::path::Path;

/// Connects to the profile daemon named by DARWIN_ART_PROFILE_SOCKET; null
/// on failure.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_runtime_host_command_connect() -> *mut c_void {
    std::panic::catch_unwind(|| {
        let Some(socket) = std::env::var_os("DARWIN_ART_PROFILE_SOCKET") else {
            return std::ptr::null_mut();
        };
        match HostCommandListener::connect(Path::new(&socket)) {
            Ok(listener) => Box::into_raw(Box::new(listener)).cast(),
            Err(error) => {
                eprintln!("host command relay: {error}");
                std::ptr::null_mut()
            }
        }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Blocks for the next command. Returns its NUL-separated arguments in a
/// buffer the caller releases with `darwin_art_runtime_host_command_free`,
/// or null when the daemon connection ended.
///
/// # Safety
/// `listener` comes from `darwin_art_runtime_host_command_connect`;
/// `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_host_command_next(
    listener: *mut c_void,
    length: *mut usize,
) -> *mut u8 {
    if listener.is_null() || length.is_null() {
        return std::ptr::null_mut();
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: caller contract.
        let listener = unsafe { &mut *listener.cast::<HostCommandListener>() };
        match listener.next() {
            Ok(arguments) => {
                let bytes = arguments.join("\0").into_bytes().into_boxed_slice();
                // SAFETY: caller contract.
                unsafe { length.write(bytes.len()) };
                Box::into_raw(bytes).cast::<u8>()
            }
            Err(_) => std::ptr::null_mut(),
        }
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `bytes`/`length` come from `darwin_art_runtime_host_command_next`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_host_command_free(bytes: *mut u8, length: usize) {
    if !bytes.is_null() {
        // SAFETY: allocated by `next` as a boxed slice of `length` bytes.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(bytes, length)) });
    }
}

/// Sends the command's exit status and output. Returns 0 on success.
///
/// # Safety
/// `listener` as above; `output` readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_host_command_reply(
    listener: *mut c_void,
    status: u32,
    output: *const u8,
    length: usize,
) -> c_int {
    if listener.is_null() || (output.is_null() && length != 0) {
        return -1;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: caller contract.
        let listener = unsafe { &mut *listener.cast::<HostCommandListener>() };
        let output = if length == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(output, length) }
        };
        if listener.reply(status, output).is_ok() {
            0
        } else {
            -1
        }
    }))
    .unwrap_or(-1)
}
