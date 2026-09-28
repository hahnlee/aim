//! `VK_KHR_external_semaphore_fd` with sync-file fds, which HWUI requires
//! to hand its rendering to SurfaceFlinger and back. MoltenVK has no sync
//! files; the driver connects them to its own timeline semaphores
//! ([`driver::fence_after`], [`driver::signal_after`]):
//!
//! - export submits a batch that waits for the semaphore and returns a
//!   sync_file that signals after it; the wait consumes the payload, as the
//!   copy transference of a sync fd does;
//! - import submits a batch that signals the semaphore once the fd has
//!   signaled.

use std::ffi::c_void;

use crate::driver;
use crate::thunks::host;
use crate::types::*;

pub unsafe extern "C" fn vkCreateSemaphore(
    device: VkDispatch,
    info: *const VkBaseOutStructure,
    _allocator: *const c_void,
    semaphore: *mut u64,
) -> VkResult {
    // A semaphore exportable as a sync fd is an ordinary one to MoltenVK.
    // SAFETY: the application's create info; its chain is ours for the
    // call.
    let _unlinked = unsafe {
        Unlinked::new((&raw mut (*info.cast_mut()).pNext).cast(), |s| {
            (*s).sType == stype::EXPORT_SEMAPHORE_CREATE_INFO
                && (*(s as *const VkExternalMemoryCreateInfo)).handleTypes
                    & SEMAPHORE_HANDLE_TYPE_SYNC_FD
                    != 0
        })
    };
    // SAFETY: a valid create info.
    unsafe { host::vkCreateSemaphore(device, info.cast(), std::ptr::null(), semaphore.cast()) }
}

pub unsafe extern "C" fn vkGetPhysicalDeviceExternalSemaphoreProperties(
    physical: VkDispatch,
    info: *const VkPhysicalDeviceExternalInfo,
    props: *mut VkExternalSemaphoreProperties,
) {
    // SAFETY: the application's arguments.
    unsafe {
        if (*info).handleType != SEMAPHORE_HANDLE_TYPE_SYNC_FD {
            return host::vkGetPhysicalDeviceExternalSemaphoreProperties(
                physical,
                info.cast(),
                props.cast(),
            );
        }
        (*props).exportFromImportedHandleTypes = SEMAPHORE_HANDLE_TYPE_SYNC_FD;
        (*props).compatibleHandleTypes = SEMAPHORE_HANDLE_TYPE_SYNC_FD;
        // VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT | IMPORTABLE_BIT.
        (*props).externalSemaphoreFeatures = 1 | 2;
    }
}

pub unsafe extern "C" fn vkGetSemaphoreFdKHR(
    device: VkDispatch,
    info: *const VkSemaphoreGetFdInfoKHR,
    fd: *mut i32,
) -> VkResult {
    // SAFETY: the application's arguments.
    let (semaphore, kind) = unsafe { ((*info).semaphore, (*info).handleType) };
    if kind != SEMAPHORE_HANDLE_TYPE_SYNC_FD {
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    }
    match driver::fence_after(device, None, &[(semaphore, 0)]) {
        Ok(f) => {
            // SAFETY: the application's out pointer.
            unsafe { *fd = f };
            VK_SUCCESS
        }
        Err(r) => r,
    }
}

pub unsafe extern "C" fn vkImportSemaphoreFdKHR(
    device: VkDispatch,
    info: *const VkImportSemaphoreFdInfoKHR,
) -> VkResult {
    // SAFETY: the application's arguments.
    let (semaphore, kind, fd) = unsafe { ((*info).semaphore, (*info).handleType, (*info).fd) };
    if kind != SEMAPHORE_HANDLE_TYPE_SYNC_FD {
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    }
    driver::signal_after(device, fd, &[(semaphore, 0)], 0)
}
