//! Checked ABI for `__system_property_foreach` over the process property owner.
//!
//! The property area owns the opaque entries. This layer only snapshots their
//! stable handles and invokes the guest callback without holding the area lock.

use crate::{
    ANDROID_EFAULT, ANDROID_EIO, CAPABILITY_FAILURE, missing_snapshot, property_snapshot, set_errno,
};
use std::ffi::{c_int, c_void};
use std::sync::atomic::Ordering;

type PropertyForeachCallback = unsafe extern "C" fn(property: *const c_void, cookie: *mut c_void);

#[unsafe(no_mangle)]
/// # Safety
/// `callback`, when present, must remain callable for the duration of this
/// call. It may re-enter any property API, including this function.
pub unsafe extern "C" fn darwin_art_bionic_process_property_foreach_core(
    callback: Option<PropertyForeachCallback>,
    cookie: *mut c_void,
) -> c_int {
    let Some(callback) = callback else {
        set_errno(ANDROID_EFAULT);
        return -1;
    };
    let Some(snapshot) = property_snapshot() else {
        missing_snapshot();
        return -1;
    };
    let tokens = match snapshot.properties.tokens() {
        Ok(tokens) => tokens,
        Err(_) => {
            CAPABILITY_FAILURE.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return -1;
        }
    };
    // The snapshot Arc remains held across the complete callback sequence,
    // including a callback that uninstalls the process owner. `tokens()` has
    // already released the PropertyArea lock, so callbacks may re-enter the
    // property owner without deadlocking it.
    for token in tokens {
        // SAFETY: the callback is supplied by the guest under the ABI
        // contract, and each token points to an Entry retained by `snapshot`.
        unsafe { callback(token, cookie) };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuxSnapshot, SAFE_HWCAP, Snapshot, install_process_snapshot, uninstall_process_snapshot,
    };
    use std::sync::Arc;

    struct OwnerGuard;

    impl Drop for OwnerGuard {
        fn drop(&mut self) {
            let _ = uninstall_process_snapshot();
        }
    }

    struct Capture {
        names: Vec<Vec<u8>>,
        lengths: Vec<c_int>,
        reentrant_updates: usize,
        failed: bool,
    }

    unsafe extern "C" fn reentrant_callback(property: *const c_void, cookie: *mut c_void) {
        let capture = unsafe { &mut *cookie.cast::<Capture>() };
        let Some(snapshot) = crate::active_snapshot() else {
            capture.failed = true;
            return;
        };
        let entry = match snapshot.properties.read(property) {
            Ok(Some(entry)) => entry,
            _ => {
                capture.failed = true;
                return;
            }
        };
        let name = entry.name.clone();
        capture.names.push(name[..name.len() - 1].to_vec());
        let mut value = [0_i8; 92];
        let length = unsafe {
            crate::darwin_art_bionic_process_property_get_core(
                name.as_ptr().cast(),
                value.as_mut_ptr(),
            )
        };
        if length != (entry.value.len() - 1) as c_int {
            capture.failed = true;
        }
        capture.lengths.push(length);
        if snapshot
            .properties
            .update(b"runtime.updated", b"yes")
            .is_err()
        {
            capture.failed = true;
        } else {
            capture.reentrant_updates += 1;
        }
    }

    unsafe extern "C" fn no_op_callback(_property: *const c_void, _cookie: *mut c_void) {}

    fn snapshot() -> Arc<Snapshot> {
        Arc::new(
            Snapshot::new(
                vec![],
                vec![
                    (b"a.key".to_vec(), b"a".to_vec()),
                    (b"z.key".to_vec(), b"z-value".to_vec()),
                ],
                AuxSnapshot {
                    page_size: 16_384,
                    hwcap: SAFE_HWCAP,
                    hwcap2: 0,
                    secure: false,
                    random: [0x27; 16],
                },
            )
            .unwrap(),
        )
    }

    #[test]
    fn foreach_releases_area_lock_for_reentrant_read_and_update() {
        let _test_owner_lock = crate::test_process_owner_guard();
        install_process_snapshot(snapshot()).unwrap();
        let _owner = OwnerGuard;
        let mut capture = Capture {
            names: Vec::new(),
            lengths: Vec::new(),
            reentrant_updates: 0,
            failed: false,
        };
        let result = unsafe {
            darwin_art_bionic_process_property_foreach_core(
                Some(reentrant_callback),
                (&mut capture as *mut Capture).cast(),
            )
        };
        assert_eq!(result, 0);
        assert!(!capture.failed);
        assert_eq!(capture.names, vec![b"a.key".to_vec(), b"z.key".to_vec()]);
        assert_eq!(capture.lengths, vec![1, 7]);
        assert_eq!(capture.reentrant_updates, 2);
        let active = crate::active_snapshot().unwrap();
        assert_eq!(
            active
                .properties
                .get(b"runtime.updated")
                .unwrap()
                .unwrap()
                .value
                .as_ref(),
            b"yes\0"
        );
    }

    #[test]
    fn foreach_rejects_null_callback_and_absent_owner() {
        let _test_owner_lock = crate::test_process_owner_guard();
        assert_eq!(
            unsafe { darwin_art_bionic_process_property_foreach_core(None, std::ptr::null_mut()) },
            -1
        );
        install_process_snapshot(snapshot()).unwrap();
        let owner = OwnerGuard;
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_property_foreach_core(
                    Some(no_op_callback),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        drop(owner);
        assert_eq!(
            unsafe {
                darwin_art_bionic_process_property_foreach_core(
                    Some(no_op_callback),
                    std::ptr::null_mut(),
                )
            },
            -1
        );
    }
}
