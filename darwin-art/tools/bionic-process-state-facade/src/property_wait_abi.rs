//! Checked arm64 property-wait ABI. Output changes only on a real update.
use super::*;
#[repr(C)]
pub struct WaitTimeout {
    pub seconds: i64,
    pub nanoseconds: i64,
}

/// # Safety
/// output is writable; a nonnull timeout points to the two signed 64-bit
/// arm64 timespec fields. Property is an opaque token, never dereferenced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_bionic_process_property_wait_core(
    property: *const std::ffi::c_void,
    old: u32,
    output: *mut u32,
    timeout: *const WaitTimeout,
) -> c_int {
    if output.is_null() {
        set_errno(ANDROID_EFAULT);
        return -1;
    }
    let duration = if timeout.is_null() {
        None
    } else {
        let timeout = unsafe { &*timeout };
        if timeout.seconds < 0 || !(0..1_000_000_000).contains(&timeout.nanoseconds) {
            set_errno(22);
            return -1;
        }
        Some(std::time::Duration::new(
            timeout.seconds as u64,
            timeout.nanoseconds as u32,
        ))
    };
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return -1;
    };
    match snapshot.properties.wait(property, old, duration) {
        Ok(Some(serial)) => {
            unsafe {
                *output = serial;
            }
            1
        }
        Ok(None) => 0,
        Err(error) => {
            set_errno(match error {
                "invalid property token" => ANDROID_EFAULT,
                "invalid property timeout" => 22,
                "property area closed" => 125,
                _ => ANDROID_EIO,
            });
            -1
        }
    }
}
