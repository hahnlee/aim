//! Process-lifetime ownership for a caller-provided Android snapshot.
//!
//! The host owns construction and configuration of [`Snapshot`]. This module
//! only owns the installed `Arc` and its activation for the process lifetime.

use crate::{Activation, AuxSnapshot, SAFE_HWCAP, Snapshot};
use std::ffi::c_int;
use std::sync::{Arc, Mutex};

// Activation must be dropped before its snapshot storage.
struct ProcessOwner {
    activation: Activation,
    _snapshot: Arc<Snapshot>,
}

static PROCESS_OWNER: Mutex<Option<ProcessOwner>> = Mutex::new(None);

/// Borrowing readiness check, not installation or ownership transfer. The
/// embedding engine must keep its owner alive until all runtime work drains.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_process_state_is_installed() -> c_int {
    match PROCESS_OWNER.lock() {
        Ok(owner) => i32::from(owner.is_some()),
        Err(_) => -1,
    }
}

/// Install a caller-configured snapshot and retain it until uninstall.
///
/// The caller may drop its `Arc` after this returns successfully: the process
/// owner retains the backing storage, while `Snapshot::activate` retains the
/// active view. A second installation is rejected without changing the active
/// snapshot.
pub fn install_process_snapshot(snapshot: Arc<Snapshot>) -> Result<(), &'static str> {
    let mut owner = PROCESS_OWNER
        .lock()
        .map_err(|_| "process owner lock poisoned")?;
    if owner.is_some() {
        return Err("another process snapshot is active");
    }
    let activation = snapshot.activate()?;
    *owner = Some(ProcessOwner {
        activation,
        _snapshot: snapshot,
    });
    Ok(())
}

/// Uninstall the process-owned snapshot and clear the exported active view.
pub fn uninstall_process_snapshot() -> Result<(), &'static str> {
    let mut owner = PROCESS_OWNER
        .lock()
        .map_err(|_| "process owner lock poisoned")?;
    let Some(process) = owner.take() else {
        return Err("no process snapshot is active");
    };
    drop(process.activation);
    drop(process._snapshot);
    Ok(())
}

/// Compatibility fixture entry point. Production callers must construct and
/// install their configured `Snapshot` through `install_process_snapshot`;
/// these fixed values are retained only for the existing fixture ABI.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_process_state_process_install() -> c_int {
    let mut random = [0_u8; 16];
    unsafe extern "C" {
        fn getentropy(buffer: *mut std::ffi::c_void, length: usize) -> c_int;
    }
    // SAFETY: random is writable for exactly the requested byte count.
    if unsafe { getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
        return -1;
    }
    let snapshot = match Snapshot::new(
        vec![
            (b"ANDROID_ROOT".to_vec(), b"/system".to_vec()),
            (b"ANDROID_DATA".to_vec(), b"/data".to_vec()),
            (b"ANDROID_STORAGE".to_vec(), b"/storage".to_vec()),
            (
                b"EXTERNAL_STORAGE".to_vec(),
                b"/storage/emulated/0".to_vec(),
            ),
            (b"LANG".to_vec(), b"en-US".to_vec()),
        ],
        vec![
            (b"device.cpu.count".to_vec(), b"8".to_vec()),
            (b"device.cpu.frequency_mhz".to_vec(), b"2400".to_vec()),
            (b"device.cpu.model".to_vec(), b"Darwin ARM64".to_vec()),
            (b"ro.build.version.sdk".to_vec(), b"36".to_vec()),
            (b"ro.build.version.release".to_vec(), b"16".to_vec()),
            (b"ro.product.cpu.abi".to_vec(), b"arm64-v8a".to_vec()),
            (b"ro.product.cpu.abilist".to_vec(), b"arm64-v8a".to_vec()),
        ],
        AuxSnapshot {
            page_size: 16_384,
            hwcap: SAFE_HWCAP,
            hwcap2: 0,
            secure: false,
            random,
        },
    ) {
        Ok(snapshot) => Arc::new(snapshot),
        Err(_) => return -1,
    };
    install_process_snapshot(snapshot).map_or(-1, |_| 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_process_state_process_uninstall() -> c_int {
    uninstall_process_snapshot().map_or(-1, |_| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_snapshot;

    struct OwnerGuard;

    impl Drop for OwnerGuard {
        fn drop(&mut self) {
            let _ = uninstall_process_snapshot();
        }
    }

    fn snapshot(marker: &[u8]) -> Arc<Snapshot> {
        Arc::new(
            Snapshot::new(
                vec![(b"OWNER_MARKER".to_vec(), marker.to_vec())],
                vec![(b"owner.marker".to_vec(), marker.to_vec())],
                AuxSnapshot {
                    page_size: 16_384,
                    hwcap: SAFE_HWCAP,
                    hwcap2: 0,
                    secure: false,
                    random: [0x5a; 16],
                },
            )
            .unwrap(),
        )
    }

    #[test]
    fn caller_snapshot_values_are_retained_without_fixture_seed() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let caller_snapshot = snapshot(b"caller-owned");
        install_process_snapshot(caller_snapshot.clone()).unwrap();
        let _owner = OwnerGuard;

        let active = active_snapshot().unwrap();
        assert_eq!(
            active
                .environment
                .get(b"OWNER_MARKER".as_slice())
                .unwrap()
                .as_ref(),
            b"caller-owned\0"
        );
        assert_eq!(
            active
                .properties
                .get(b"owner.marker")
                .unwrap()
                .unwrap()
                .value
                .as_ref(),
            b"caller-owned\0"
        );
    }

    #[test]
    fn duplicate_install_preserves_existing_active_snapshot() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let first = snapshot(b"first");
        install_process_snapshot(first).unwrap();
        let _owner = OwnerGuard;
        let second = snapshot(b"second");

        assert_eq!(
            install_process_snapshot(second),
            Err("another process snapshot is active")
        );
        assert_eq!(
            active_snapshot()
                .unwrap()
                .environment
                .get(b"OWNER_MARKER".as_slice())
                .unwrap()
                .as_ref(),
            b"first\0"
        );
    }

    #[test]
    fn caller_arc_drop_does_not_end_process_lifetime() {
        let _test_owner_lock = crate::test_process_owner_guard();
        assert_eq!(darwin_art_bionic_process_state_is_installed(), 0);
        let caller_snapshot = snapshot(b"lifetime");
        let weak = Arc::downgrade(&caller_snapshot);
        install_process_snapshot(caller_snapshot.clone()).unwrap();
        let _owner = OwnerGuard;
        drop(caller_snapshot);
        assert_eq!(darwin_art_bionic_process_state_is_installed(), 1);
        assert!(weak.upgrade().is_some());

        uninstall_process_snapshot().unwrap();
        assert_eq!(darwin_art_bionic_process_state_is_installed(), 0);
        assert!(active_snapshot().is_none());
        assert!(weak.upgrade().is_none());
    }
}
