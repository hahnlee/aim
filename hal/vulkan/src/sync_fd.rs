//! `VK_KHR_external_semaphore_fd` with sync-file fds, which HWUI requires
//! to hand its rendering to SurfaceFlinger and back. MoltenVK has no sync
//! files, and the syscall layer none the host can signal yet, so the
//! payload crosses on the CPU:
//!
//! - export waits until the semaphore's pending signal has run and returns
//!   -1, the sync fd of an already signaled payload; the wait consumes the
//!   payload, as the copy transference of a sync fd does;
//! - import waits for the fd to signal, closes it, and signals the
//!   semaphore.

use std::ffi::c_void;

use crate::driver::{submit, wait_fd};
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
    let r = submit(device, None, &[semaphore], &[], 0, true);
    if r == VK_SUCCESS {
        // SAFETY: the application's out pointer.
        unsafe { *fd = -1 };
    }
    r
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
    wait_fd(fd);
    submit(device, None, &[], &[semaphore], 0, false)
}
