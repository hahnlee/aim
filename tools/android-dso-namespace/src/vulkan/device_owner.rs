//! Process-local ownership facts keyed by the exact guest VkDevice handle.
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DeviceOwnerSnapshot {
    pub physical_device: *mut c_void,
    pub metal_device: *mut c_void,
}

// Raw handles are borrowed Vulkan/external-sync tokens, not leases. They are
// copied under this mutex and never escape through an async callback; callers
// must not hold the lock across a driver or Objective-C callback.
static OWNERS: OnceLock<Mutex<HashMap<usize, (usize, usize)>>> = OnceLock::new();
const VK_SUCCESS: i32 = 0;
const VK_ERROR_EXTENSION_NOT_PRESENT: i32 = -7;

fn owners() -> &'static Mutex<HashMap<usize, (usize, usize)>> {
    OWNERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn record(device: *mut c_void, physical_device: *mut c_void, metal_device: *mut c_void) {
    if device.is_null() {
        return;
    }
    owners().lock().unwrap().insert(
        device as usize,
        (physical_device as usize, metal_device as usize),
    );
}

pub(super) fn snapshot(device: *mut c_void) -> Option<DeviceOwnerSnapshot> {
    if device.is_null() {
        return None;
    }
    owners()
        .lock()
        .unwrap()
        .get(&(device as usize))
        .copied()
        .map(|(physical_device, metal_device)| DeviceOwnerSnapshot {
            physical_device: physical_device as *mut c_void,
            metal_device: metal_device as *mut c_void,
        })
}

pub(super) fn remove(device: *mut c_void) -> Option<DeviceOwnerSnapshot> {
    if device.is_null() {
        return None;
    }
    owners()
        .lock()
        .unwrap()
        .remove(&(device as usize))
        .map(|(physical_device, metal_device)| DeviceOwnerSnapshot {
            physical_device: physical_device as *mut c_void,
            metal_device: metal_device as *mut c_void,
        })
}

// The creation callback is invoked only after bridge capability preflight. The
// owner records a successful, non-null native device and nothing else.
pub(super) unsafe fn create_and_record<F>(
    device: *mut *mut c_void,
    physical_device: *mut c_void,
    metal_device: *mut c_void,
    bridge_requested: bool,
    native_create: F,
) -> i32
where
    F: FnOnce() -> i32,
{
    if bridge_requested && metal_device.is_null() {
        if !device.is_null() {
            unsafe { *device = std::ptr::null_mut() };
        }
        return VK_ERROR_EXTENSION_NOT_PRESENT;
    }
    let result = native_create();
    if result == VK_SUCCESS && !device.is_null() {
        let created_device = unsafe { *device };
        if !created_device.is_null() {
            record(created_device, physical_device, metal_device);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn entries_are_device_isolated_and_removal_preserves_other_device() {
        record(0xa as *mut c_void, 0x11 as *mut c_void, 0x21 as *mut c_void);
        record(0xb as *mut c_void, 0x12 as *mut c_void, 0x22 as *mut c_void);
        assert_eq!(
            snapshot(0xa as *mut c_void).unwrap().physical_device as usize,
            0x11
        );
        assert_eq!(
            remove(0xb as *mut c_void).unwrap().metal_device as usize,
            0x22
        );
        assert_eq!(
            snapshot(0xa as *mut c_void).unwrap().metal_device as usize,
            0x21
        );
        assert!(snapshot(0xb as *mut c_void).is_none());
    }

    #[test]
    fn unknown_creation_is_unpublished_and_reused_handle_has_no_old_owner() {
        assert!(snapshot(0xc as *mut c_void).is_none());
        record(0xc as *mut c_void, 0x31 as *mut c_void, 0x41 as *mut c_void);
        assert!(remove(0xc as *mut c_void).is_some());
        assert!(snapshot(0xc as *mut c_void).is_none());
        record(0xc as *mut c_void, 0x32 as *mut c_void, 0 as *mut c_void);
        assert_eq!(
            snapshot(0xc as *mut c_void).unwrap().physical_device as usize,
            0x32
        );
        assert!(snapshot(0xd as *mut c_void).is_none());
    }

    #[test]
    fn bridge_without_metal_rejects_before_native_create() {
        let mut output = 0x99 as *mut c_void;
        let mut calls = 0;
        let result = unsafe {
            create_and_record(
                &mut output,
                0x51 as *mut c_void,
                std::ptr::null_mut(),
                true,
                || {
                    calls += 1;
                    0
                },
            )
        };
        assert_eq!(result, VK_ERROR_EXTENSION_NOT_PRESENT);
        assert_eq!(calls, 0);
        assert!(output.is_null());
        assert!(snapshot(0x99 as *mut c_void).is_none());
    }

    #[test]
    fn ordinary_device_records_none_metal_after_one_native_create() {
        let mut output = std::ptr::null_mut();
        let output_ptr = &mut output as *mut *mut c_void;
        let mut calls = 0;
        let result = unsafe {
            create_and_record(
                output_ptr,
                0x61 as *mut c_void,
                ptr::null_mut(),
                false,
                || {
                    calls += 1;
                    unsafe { *output_ptr = 0x62 as *mut c_void };
                    0
                },
            )
        };
        assert_eq!(result, VK_SUCCESS);
        assert_eq!(calls, 1);
        assert_eq!(snapshot(output).unwrap().physical_device as usize, 0x61);
        assert!(snapshot(output).unwrap().metal_device.is_null());
    }

    #[test]
    fn native_failure_is_unpublished_and_success_records_exact_metal() {
        let mut failed = 0x71 as *mut c_void;
        let result = unsafe {
            create_and_record(
                &mut failed,
                0x72 as *mut c_void,
                0x73 as *mut c_void,
                false,
                || -9,
            )
        };
        assert_eq!(result, -9);
        assert!(snapshot(failed).is_none());

        let mut output = std::ptr::null_mut();
        let output_ptr = &mut output as *mut *mut c_void;
        let result = unsafe {
            create_and_record(
                output_ptr,
                0x81 as *mut c_void,
                0x82 as *mut c_void,
                false,
                || {
                    unsafe { *output_ptr = 0x83 as *mut c_void };
                    0
                },
            )
        };
        assert_eq!(result, VK_SUCCESS);
        let owner = snapshot(output).unwrap();
        assert_eq!(owner.physical_device as usize, 0x81);
        assert_eq!(owner.metal_device as usize, 0x82);
    }
}
