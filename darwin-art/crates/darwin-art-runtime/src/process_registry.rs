//! Rust-owned decoding of profile process identity for native Binder ingress.
use std::path::Path;

#[repr(C)]
pub struct RegisteredProcessIdentity {
    pub uid: i32,
    pub package: [u8; 256],
}

/// # Safety
/// `output` must point to writable storage for one RegisteredProcessIdentity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_registered_process_identity(
    pid: u32,
    output: *mut RegisteredProcessIdentity,
) -> bool {
    if output.is_null() {
        return false;
    }
    let value = std::panic::catch_unwind(|| {
        let socket = std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV)?;
        let identity =
            darwin_art_profile::resolve_process_identity_at(Path::new(&socket), pid).ok()?;
        let bytes = identity.package.as_bytes();
        if bytes.is_empty() || bytes.len() >= 256 {
            return None;
        }
        let mut result = RegisteredProcessIdentity {
            uid: i32::try_from(identity.uid).ok()?,
            package: [0; 256],
        };
        result.package[..bytes.len()].copy_from_slice(bytes);
        Some(result)
    })
    .ok()
    .flatten();
    match value {
        Some(value) => {
            unsafe {
                output.write(value);
            }
            true
        }
        None => false,
    }
}

/// The Intent the registered process's launch requested: 1 with `action` and
/// `data` (empty when absent) as NUL-terminated strings, 0 when the launch
/// requested none, -1 when it cannot be resolved or does not fit.
///
/// # Safety
/// `action` and `data` must point to writable buffers of the given capacities.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_registered_launch_intent(
    pid: u32,
    action: *mut u8,
    action_capacity: usize,
    data: *mut u8,
    data_capacity: usize,
) -> i32 {
    if action.is_null() || data.is_null() || action_capacity == 0 || data_capacity == 0 {
        return -1;
    }
    let resolved = std::panic::catch_unwind(|| {
        let socket = std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV)?;
        darwin_art_profile::resolve_launch_intent_at(Path::new(&socket), pid).ok()
    })
    .ok()
    .flatten();
    let Some(intent) = resolved else {
        return -1;
    };
    let Some(intent) = intent else {
        return 0;
    };
    let copy = |text: &str, output: *mut u8, capacity: usize| -> bool {
        let bytes = text.as_bytes();
        if bytes.len() >= capacity {
            return false;
        }
        // SAFETY: the caller's buffer holds `capacity` bytes; one is the NUL.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
            output.add(bytes.len()).write(0);
        }
        true
    };
    if copy(&intent.action, action, action_capacity)
        && copy(intent.data.as_deref().unwrap_or(""), data, data_capacity)
    {
        1
    } else {
        -1
    }
}

fn resolve_uid(pid: u32) -> Option<i32> {
    let socket = std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV)?;
    let identity = darwin_art_profile::resolve_process_identity_at(Path::new(&socket), pid).ok()?;
    i32::try_from(identity.uid).ok()
}

/// Called with a kernel-authenticated peer PID, not a PID read from a Parcel.
/// Unknown/old daemon, missing registration and invalid records all return -1.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_runtime_registered_process_uid(pid: u32) -> i32 {
    std::panic::catch_unwind(|| resolve_uid(pid))
        .ok()
        .flatten()
        .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_ffi_layout_and_failure_are_stable() {
        assert_eq!(std::mem::size_of::<RegisteredProcessIdentity>(), 260);
        assert_eq!(std::mem::offset_of!(RegisteredProcessIdentity, package), 4);
        assert!(!unsafe {
            darwin_art_runtime_registered_process_identity(0, std::ptr::null_mut())
        });
        let mut result = RegisteredProcessIdentity {
            uid: 99,
            package: [0xff; 256],
        };
        assert!(!unsafe { darwin_art_runtime_registered_process_identity(0, &mut result) });
        assert_eq!(result.uid, 99);
        assert_eq!(result.package, [0xff; 256]);
    }
}
