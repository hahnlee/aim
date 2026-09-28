//! Vulkan fence wrappers for the Android acquire path.
//!
//! The compositor can make an acquire operation complete from a buffer that
//! is already ready on the native side.  In that case `mark_acquired` records
//! the acquire completion for the exact device/fence pair.  It does not claim
//! that GPU rendering has completed and it never replaces a native fence.

use std::collections::HashSet;
use std::ffi::{c_void, CStr};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::vulkan_backend::raw_device_symbol as backend_raw_device_symbol;

const VK_SUCCESS: i32 = 0;
const VK_TIMEOUT: i32 = 2;
const VK_ERROR_INITIALIZATION_FAILED: i32 = -3;
const MAX_FENCE_COUNT: usize = 1 << 20;
const NATIVE_WAIT_SLICE_NS: u64 = 1_000_000;

type FenceKey = (usize, usize);

static ACQUIRED_FENCES: OnceLock<Mutex<HashSet<FenceKey>>> = OnceLock::new();

fn acquired_fences() -> &'static Mutex<HashSet<FenceKey>> {
    ACQUIRED_FENCES.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Mark one real Vulkan fence as acquire-complete for `device`.
///
/// This is deliberately separate from rendering completion.  The key includes
/// the device because Vulkan fence handles are only meaningful in that device.
pub(crate) fn mark_acquired(device: usize, fence: usize) {
    if device == 0 || fence == 0 {
        return;
    }
    acquired_fences()
        .lock()
        .expect("Vulkan acquire-fence registry poisoned")
        .insert((device, fence));
}

/// Forget all virtual acquire marks belonging to a destroyed device.
pub(crate) fn forget_device(device: usize) {
    if device == 0 {
        return;
    }
    acquired_fences()
        .lock()
        .expect("Vulkan acquire-fence registry poisoned")
        .retain(|(marked_device, _)| *marked_device != device);
}

fn clear_fence(device: usize, fence: usize) {
    acquired_fences()
        .lock()
        .expect("Vulkan acquire-fence registry poisoned")
        .remove(&(device, fence));
}

fn virtual_satisfied(device: usize, fences: &[usize], wait_all: bool) -> bool {
    if fences.is_empty() {
        return false;
    }
    let marked = acquired_fences()
        .lock()
        .expect("Vulkan acquire-fence registry poisoned");
    if wait_all {
        fences
            .iter()
            .all(|fence| marked.contains(&(device, *fence)))
    } else {
        fences
            .iter()
            .any(|fence| marked.contains(&(device, *fence)))
    }
}

fn pending_native_fences(
    device: usize,
    fences: &[*mut c_void],
    wait_all: bool,
) -> Vec<*mut c_void> {
    if !wait_all {
        return fences.to_vec();
    }
    let marked = acquired_fences()
        .lock()
        .expect("Vulkan acquire-fence registry poisoned");
    fences
        .iter()
        .copied()
        .filter(|fence| !marked.contains(&(device, *fence as usize)))
        .collect()
}

fn combine_wait_result(virtual_complete: bool, native_result: i32) -> i32 {
    if virtual_complete {
        VK_SUCCESS
    } else {
        native_result
    }
}

fn clear_after_reset(device: usize, fences: &[*mut c_void], result: i32) {
    if result == VK_SUCCESS {
        for fence in fences {
            clear_fence(device, *fence as usize);
        }
    }
}

fn raw_device_symbol(device: usize, name: &CStr) -> *mut c_void {
    // The parent WSI backend owns the device-proc lookup and returns a native
    // driver address without exposing that address through the Android DSO.
    unsafe { backend_raw_device_symbol(device, name) }
}

fn fence_slice<'a>(fences: *const *mut c_void, count: u32) -> Result<&'a [*mut c_void], i32> {
    let count = count as usize;
    if count > MAX_FENCE_COUNT {
        return Err(VK_ERROR_INITIALIZATION_FAILED);
    }
    if count == 0 {
        return Ok(&[]);
    }
    if fences.is_null() {
        return Err(VK_ERROR_INITIALIZATION_FAILED);
    }
    // SAFETY: the Vulkan caller owns a contiguous array of `count` fence
    // handles for the duration of the call; the size is bounded above.
    Ok(unsafe { std::slice::from_raw_parts(fences, count) })
}

unsafe fn call_native_get_status(device: *mut c_void, fence: *mut c_void) -> i32 {
    type GetFenceStatus = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
    let address = raw_device_symbol(device as usize, c"vkGetFenceStatus");
    if address.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let get_status: GetFenceStatus = unsafe { std::mem::transmute(address) };
    unsafe { get_status(device, fence) }
}

unsafe fn call_native_wait(
    device: *mut c_void,
    fences: &[*mut c_void],
    wait_all: bool,
    timeout_ns: u64,
) -> i32 {
    type WaitForFences =
        unsafe extern "C" fn(*mut c_void, u32, *const *mut c_void, u32, u64) -> i32;
    let address = raw_device_symbol(device as usize, c"vkWaitForFences");
    if address.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let wait: WaitForFences = unsafe { std::mem::transmute(address) };
    unsafe {
        wait(
            device,
            fences.len() as u32,
            fences.as_ptr(),
            u32::from(wait_all),
            timeout_ns,
        )
    }
}

unsafe fn call_native_reset(device: *mut c_void, fences: &[*mut c_void]) -> i32 {
    type ResetFences = unsafe extern "C" fn(*mut c_void, u32, *const *mut c_void) -> i32;
    let address = raw_device_symbol(device as usize, c"vkResetFences");
    if address.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let reset: ResetFences = unsafe { std::mem::transmute(address) };
    unsafe { reset(device, fences.len() as u32, fences.as_ptr()) }
}

unsafe fn call_native_destroy(device: *mut c_void, fence: *mut c_void, allocator: *const c_void) {
    type DestroyFence = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void);
    let address = raw_device_symbol(device as usize, c"vkDestroyFence");
    if address.is_null() {
        return;
    }
    let destroy: DestroyFence = unsafe { std::mem::transmute(address) };
    unsafe { destroy(device, fence, allocator) };
}

unsafe extern "C" fn get_fence_status(device: *mut c_void, fence: *mut c_void) -> i32 {
    if device.is_null() || fence.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if virtual_satisfied(device as usize, &[fence as usize], false) {
        return VK_SUCCESS;
    }
    unsafe { call_native_get_status(device, fence) }
}

unsafe extern "C" fn wait_for_fences(
    device: *mut c_void,
    fence_count: u32,
    fences: *const *mut c_void,
    wait_all: u32,
    timeout_ns: u64,
) -> i32 {
    if device.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let fences = match fence_slice(fences, fence_count) {
        Ok(fences) => fences,
        Err(error) => return error,
    };
    // Vulkan's zero-fence wait has no fence that can satisfy the request.
    if fences.is_empty() {
        return VK_TIMEOUT;
    }
    let wait_all = wait_all != 0;
    let device_key = device as usize;
    let start = Instant::now();
    let mut native_poll_attempted = false;
    loop {
        let keys = fences
            .iter()
            .map(|fence| *fence as usize)
            .collect::<Vec<_>>();
        if virtual_satisfied(device_key, &keys, wait_all) {
            return VK_SUCCESS;
        }

        let elapsed_ns = start.elapsed().as_nanos();
        let remaining = if timeout_ns == u64::MAX {
            u64::MAX
        } else if elapsed_ns >= u128::from(timeout_ns) {
            if native_poll_attempted {
                return VK_TIMEOUT;
            }
            // Even timeout==0 performs one non-blocking native poll.  A
            // fence already signaled by the driver must still return success.
            0
        } else {
            timeout_ns - elapsed_ns as u64
        };
        // A virtual acquire mark satisfies waitAll without asking the native
        // driver to wait on that fence (which may never be signaled natively).
        // For waitAny, no marked fence reached this point, so all handles are
        // still relevant to the native wait.
        let pending = pending_native_fences(device_key, fences, wait_all);
        if pending.is_empty() {
            return VK_SUCCESS;
        }
        let native_timeout = remaining.min(NATIVE_WAIT_SLICE_NS);
        let native_result = unsafe { call_native_wait(device, &pending, wait_all, native_timeout) };
        native_poll_attempted = true;
        let result = combine_wait_result(
            virtual_satisfied(device_key, &keys, wait_all),
            native_result,
        );
        if result == VK_SUCCESS {
            return VK_SUCCESS;
        }
        if result != VK_TIMEOUT {
            return result;
        }
        // A concurrent compositor/dequeue operation may mark one of these
        // fences while the bounded native wait was in progress.  Always loop
        // back through the virtual registry before consuming another slice.
    }
}

unsafe extern "C" fn reset_fences(
    device: *mut c_void,
    fence_count: u32,
    fences: *const *mut c_void,
) -> i32 {
    if device.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let fences = match fence_slice(fences, fence_count) {
        Ok(fences) => fences,
        Err(error) => return error,
    };
    // Preserve virtual state if the native reset fails.  This matters for a
    // lost device, where the caller may still need to observe an acquire mark
    // while unwinding its synchronization objects.
    let result = unsafe { call_native_reset(device, fences) };
    clear_after_reset(device as usize, fences, result);
    result
}

unsafe extern "C" fn destroy_fence(
    device: *mut c_void,
    fence: *mut c_void,
    allocator: *const c_void,
) {
    if !device.is_null() && !fence.is_null() {
        clear_fence(device as usize, fence as usize);
    }
    if !device.is_null() {
        unsafe { call_native_destroy(device, fence, allocator) };
    }
}

/// Resolve one of the acquire-fence entry points for the parent Vulkan DSO
/// resolver.  Returning wrappers here keeps native fence handles untouched.
pub(crate) fn lookup(name: &CStr) -> Option<*mut c_void> {
    match name.to_bytes() {
        b"vkGetFenceStatus" => Some(get_fence_status as *mut c_void),
        b"vkWaitForFences" => Some(wait_for_fences as *mut c_void),
        b"vkResetFences" => Some(reset_fences as *mut c_void),
        b"vkDestroyFence" => Some(destroy_fence as *mut c_void),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_wait_any_and_all_aggregate_exact_handles() {
        let device = 0x100usize;
        let first = 0x200usize;
        let second = 0x300usize;
        forget_device(device);
        assert!(!virtual_satisfied(device, &[first, second], false));
        mark_acquired(device, first);
        assert!(virtual_satisfied(device, &[first, second], false));
        assert!(!virtual_satisfied(device, &[first, second], true));
        mark_acquired(device, second);
        assert!(virtual_satisfied(device, &[first, second], true));
        forget_device(device);
        assert!(!virtual_satisfied(device, &[first, second], false));
    }

    #[test]
    fn reset_clear_is_scoped_to_device_and_fence() {
        let device = 0x400usize;
        let other_device = 0x401usize;
        let fence = 0x500usize;
        mark_acquired(device, fence);
        mark_acquired(other_device, fence);
        clear_fence(device, fence);
        assert!(!virtual_satisfied(device, &[fence], false));
        assert!(virtual_satisfied(other_device, &[fence], false));
        forget_device(device);
        forget_device(other_device);
    }

    #[test]
    fn mixed_wait_all_excludes_virtual_handles_from_native_set() {
        let device = 0x700usize;
        let virtual_fence = 0x701usize as *mut c_void;
        let native_fence = 0x702usize as *mut c_void;
        forget_device(device);
        mark_acquired(device, virtual_fence as usize);
        let pending = pending_native_fences(device, &[virtual_fence, native_fence], true);
        assert_eq!(pending, vec![native_fence]);
        assert_eq!(combine_wait_result(false, VK_TIMEOUT), VK_TIMEOUT);
        assert_eq!(combine_wait_result(true, VK_TIMEOUT), VK_SUCCESS);
        forget_device(device);
    }

    #[test]
    fn reset_helper_preserves_mark_on_native_failure() {
        let device = 0x710usize;
        let fence = 0x711usize as *mut c_void;
        forget_device(device);
        mark_acquired(device, fence as usize);
        clear_after_reset(device, &[fence], VK_ERROR_INITIALIZATION_FAILED);
        assert!(virtual_satisfied(device, &[fence as usize], false));
        clear_after_reset(device, &[fence], VK_SUCCESS);
        assert!(!virtual_satisfied(device, &[fence as usize], false));
        forget_device(device);
    }

    #[test]
    fn empty_wait_is_timeout_without_driver_claim() {
        assert!(!virtual_satisfied(0x600, &[], false));
        assert!(!virtual_satisfied(0x600, &[], true));
    }
}
