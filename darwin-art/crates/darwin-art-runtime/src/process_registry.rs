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
