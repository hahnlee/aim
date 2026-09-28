//! `VK_ANDROID_native_buffer`: what the original loader's `VK_KHR_swapchain`
//! (`vulkan/libvulkan/swapchain.cpp`) builds on. The loader dequeues the
//! window's buffers and creates an image for each with a
//! `VkNativeBufferANDROID`; the driver makes the buffer's memory the image's
//! storage (`buffers::attach`), so rendering writes the buffer directly.
//!
//! Synchronization is by sync_files on the GPU ([`driver::fence_after`],
//! [`driver::signal_after`]): an acquire signals the application's
//! semaphore or fence once the buffer's fence has signaled, and a release
//! queues the buffer with a fence that signals once the application's
//! semaphores have.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::driver::{self, Op};
use crate::thunks::host;
use crate::types::*;
use crate::{ahb, buffers};

/// gralloc1 usage (`GRALLOC1_PRODUCER_USAGE_GPU_RENDER_TARGET`,
/// `GRALLOC1_CONSUMER_USAGE_GPU_TEXTURE`).
const PRODUCER_GPU_RENDER_TARGET: u64 = 1 << 10;
const CONSUMER_GPU_TEXTURE: u64 = 1 << 8;

/// What the driver keeps per image it made itself or will import into.
#[derive(Clone, Copy)]
pub struct Image {
    /// The gralloc buffer id whose mapping backs the image, if any.
    pub buffer: Option<u64>,
    /// For an image created for AHardwareBuffer memory: what an exported
    /// buffer is allocated with.
    pub format: i32,
    pub extent: VkExtent3D,
    pub layers: u32,
    pub usage: u32,
    pub flags: u32,
}

pub fn images() -> MutexGuard<'static, HashMap<u64, Image>> {
    static IMAGES: OnceLock<Mutex<HashMap<u64, Image>>> = OnceLock::new();
    IMAGES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub unsafe extern "C" fn vkGetSwapchainGrallocUsage2ANDROID(
    _device: VkDispatch,
    _format: i32,
    _image_usage: u32,
    _swapchain_usage: u32,
    consumer: *mut u64,
    producer: *mut u64,
) -> VkResult {
    // SAFETY: the loader's out pointers.
    unsafe {
        *consumer = CONSUMER_GPU_TEXTURE;
        *producer = PRODUCER_GPU_RENDER_TARGET;
    }
    VK_SUCCESS
}

pub unsafe extern "C" fn vkAcquireImageANDROID(
    device: VkDispatch,
    _image: u64,
    fence_fd: i32,
    semaphore: u64,
    fence: u64,
) -> VkResult {
    let signals: &[Op] = if semaphore != 0 {
        &[(semaphore, 0)]
    } else {
        &[]
    };
    driver::signal_after(device, fence_fd, signals, fence)
}

pub unsafe extern "C" fn vkQueueSignalReleaseImageANDROID(
    queue: VkDispatch,
    wait_count: u32,
    waits: *const u64,
    _image: u64,
    fence_fd: *mut i32,
) -> VkResult {
    let waits: Vec<Op> = (0..wait_count as usize)
        // SAFETY: the loader's semaphores.
        .map(|i| (unsafe { *waits.add(i) }, 0))
        .collect();
    match driver::fence_after(std::ptr::null_mut(), Some(queue), &waits) {
        Ok(fd) => {
            // SAFETY: the loader's out pointer, or the fd nobody takes.
            unsafe {
                match fence_fd.is_null() {
                    false => *fence_fd = fd,
                    true => drop(libc::close(fd)),
                }
            };
            VK_SUCCESS
        }
        Err(r) => r,
    }
}

pub unsafe extern "C" fn vkCreateImage(
    device: VkDispatch,
    info: *const VkImageCreateInfo,
    _allocator: *const c_void,
    image: *mut u64,
) -> VkResult {
    // SAFETY: the application's create info.
    let mut info = unsafe { *info };
    // SAFETY: as above; the chain is the caller's for this call.
    let native = unsafe { find::<VkNativeBufferANDROID>(info.pNext, stype::NATIVE_BUFFER_ANDROID) };
    // SAFETY: as above.
    let external = unsafe {
        find::<VkExternalFormatANDROID>(info.pNext, stype::EXTERNAL_FORMAT_ANDROID)
            .map(|e| (*e).externalFormat)
    };
    if let Some(e) = external.filter(|&e| e != 0) {
        match ahb::external_format(e) {
            Some(f) => info.format = f,
            None => return VK_ERROR_FORMAT_NOT_SUPPORTED,
        }
    }
    let mut for_ahb = false;
    // Android structures MoltenVK does not know: the native buffer is
    // attached below, AHardwareBuffer memory when it is allocated.
    // SAFETY: as above.
    let unlinked = unsafe {
        Unlinked::new(&mut info.pNext, |s| match (*s).sType {
            stype::NATIVE_BUFFER_ANDROID | stype::SWAPCHAIN_IMAGE_CREATE_INFO_ANDROID => true,
            stype::EXTERNAL_MEMORY_IMAGE_CREATE_INFO => {
                let e = &*(s as *const VkExternalMemoryCreateInfo);
                for_ahb |= e.handleTypes & HANDLE_TYPE_ANDROID_HARDWARE_BUFFER != 0;
                e.handleTypes & HANDLE_TYPE_ANDROID_HARDWARE_BUFFER != 0
            }
            stype::EXTERNAL_FORMAT_ANDROID => true,
            _ => false,
        })
    };
    // An AHardwareBuffer's YUV image is linear on the host (see `ahb`).
    if for_ahb && ahb::is_yuv(info.format) {
        info.tiling = IMAGE_TILING_LINEAR;
    }
    // SAFETY: a valid create info.
    let r = unsafe {
        host::vkCreateImage(
            device,
            (&raw const info).cast(),
            std::ptr::null(),
            image.cast(),
        )
    };
    drop(unlinked);
    if r != VK_SUCCESS {
        return r;
    }
    // SAFETY: set by the successful call.
    let handle = unsafe { *image };
    let mut record = Image {
        buffer: None,
        format: info.format,
        extent: info.extent,
        layers: info.arrayLayers,
        usage: info.usage,
        flags: info.flags,
    };
    if let Some(native) = native {
        let Some(physical) = driver::physical_device(device) else {
            return destroy_failed(device, handle, VK_ERROR_INITIALIZATION_FAILED);
        };
        // SAFETY: the loader's native buffer and its gralloc handle.
        let Some(m) = (unsafe { buffers::map((*native).handle) }) else {
            return destroy_failed(device, handle, VK_ERROR_INVALID_EXTERNAL_HANDLE);
        };
        let r = buffers::attach(physical, handle, &m, info.format);
        if r != VK_SUCCESS {
            buffers::unmap(m.id);
            return destroy_failed(device, handle, r);
        }
        record.buffer = Some(m.id);
    } else if !for_ahb {
        return VK_SUCCESS;
    }
    images().insert(handle, record);
    VK_SUCCESS
}

fn destroy_failed(device: VkDispatch, image: u64, r: VkResult) -> VkResult {
    // SAFETY: the image just created.
    unsafe { host::vkDestroyImage(device, image, std::ptr::null()) };
    r
}

pub unsafe extern "C" fn vkDestroyImage(device: VkDispatch, image: u64, _allocator: *const c_void) {
    // SAFETY: the application's image.
    unsafe { host::vkDestroyImage(device, image, std::ptr::null()) };
    if let Some(Image {
        buffer: Some(id), ..
    }) = images().remove(&image)
    {
        buffers::unmap(id);
    }
}
