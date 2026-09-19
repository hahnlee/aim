//! Platform boundary for Android WSI. Pixels stay in shared IOSurface storage.
use crate::vulkan_android_memory::{
    forget_image_format, moltenvk_allocate_memory, moltenvk_bind_image_memory,
    moltenvk_create_image, moltenvk_free_memory, moltenvk_get_android_hardware_buffer_properties,
};
use crate::vulkan_android_sync::{
    moltenvk_create_semaphore, moltenvk_destroy_semaphore, moltenvk_get_semaphore_fd,
    moltenvk_signal_wsi_acquire,
};
use crate::vulkan_backend::raw_device_symbol;
use crate::vulkan_types::{
    AHardwareBufferDesc, VulkanAndroidHardwareBufferProperties,
    VulkanExternalMemoryImageCreateInfo, VulkanImageCreateInfo,
    VulkanImportAndroidHardwareBufferInfo, VulkanMemoryAllocateInfo,
    VulkanMemoryDedicatedAllocateInfo, VulkanSemaphoreCreateInfo, VulkanSemaphoreGetFdInfo,
    VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID,
    VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
    VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
    VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID,
    VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
};
use crate::{darwin_art_android_platform_symbol, vulkan_acquire_fences};
use std::ffi::c_void;
use std::ptr;

macro_rules! platform {
    ($name:literal, $ty:ty, $fail:expr) => {{
        let address = unsafe { darwin_art_android_platform_symbol($name.as_ptr()) };
        if address.is_null() {
            return { $fail };
        }
        unsafe { std::mem::transmute::<*mut c_void, $ty>(address) }
    }};
}

#[repr(C)]
#[derive(Default)]
struct HardwareBufferDescription {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
}

#[repr(C)]
#[derive(Default)]
struct HardwareBufferIdentity {
    surface_id: u32,
    description: HardwareBufferDescription,
}

pub(super) unsafe fn wsi_backend_window_acquire(window: usize) -> bool {
    let valid = platform!(
        c"darwin_art_android_ANativeWindow_is_managed",
        unsafe extern "C" fn(*mut c_void) -> bool,
        false
    );
    if !unsafe { valid(window as *mut c_void) } {
        return false;
    }
    let acquire = platform!(
        c"darwin_art_android_ANativeWindow_acquire",
        unsafe extern "C" fn(*mut c_void),
        false
    );
    unsafe { acquire(window as *mut c_void) };
    true
}
pub(super) unsafe fn wsi_backend_window_release(window: usize) {
    let release = platform!(
        c"darwin_art_android_ANativeWindow_release",
        unsafe extern "C" fn(*mut c_void),
        ()
    );
    unsafe { release(window as *mut c_void) };
}
pub(super) unsafe fn wsi_backend_window_size(window: usize) -> Result<(u32, u32), i32> {
    let width = platform!(
        c"darwin_art_android_ANativeWindow_getWidth",
        unsafe extern "C" fn(*mut c_void) -> i32,
        Err(-3)
    );
    let height = platform!(
        c"darwin_art_android_ANativeWindow_getHeight",
        unsafe extern "C" fn(*mut c_void) -> i32,
        Err(-3)
    );
    let (w, h) = unsafe { (width(window as *mut c_void), height(window as *mut c_void)) };
    if w <= 0 || h <= 0 {
        Err(-3)
    } else {
        Ok((w as u32, h as u32))
    }
}
pub(super) unsafe fn wsi_backend_window_geometry(window: usize, w: u32, h: u32) -> i32 {
    let set = platform!(
        c"darwin_art_android_ANativeWindow_prepare_swapchain",
        unsafe extern "C" fn(*mut c_void, i32, i32) -> i32,
        -3
    );
    unsafe { set(window as *mut c_void, w as i32, h as i32) }
}
pub(super) unsafe fn wsi_backend_window_supports_mailbox(window: usize) -> bool {
    let supports = platform!(
        c"darwin_art_android_ANativeWindow_supports_mailbox",
        unsafe extern "C" fn(*mut c_void) -> bool,
        false
    );
    unsafe { supports(window as *mut c_void) }
}
pub(super) unsafe fn wsi_backend_window_set_present_mode(window: usize, mode: i32) -> i32 {
    let set = platform!(
        c"darwin_art_android_ANativeWindow_set_present_mode",
        unsafe extern "C" fn(*mut c_void, i32) -> i32,
        -3
    );
    unsafe { set(window as *mut c_void, mode) }
}
pub(super) unsafe fn wsi_backend_window_dequeue(window: usize) -> Result<(usize, usize, i32), i32> {
    let dequeue = platform!(
        c"darwin_art_android_ANativeWindow_dequeue_hardware_buffer",
        unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut *mut c_void, *mut i32) -> i32,
        Err(-3)
    );
    let (mut ahb, mut buffer, mut fence) = (ptr::null_mut(), ptr::null_mut(), -1);
    let result = unsafe { dequeue(window as *mut c_void, &mut ahb, &mut buffer, &mut fence) };
    if result == 0 {
        Ok((ahb as usize, buffer as usize, fence))
    } else {
        Err(result)
    }
}
pub(super) unsafe fn wsi_backend_window_cancel(window: usize, buffer: usize, fence: i32) -> i32 {
    let cancel = platform!(
        c"darwin_art_android_ANativeWindow_cancel_hardware_buffer",
        unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32,
        -3
    );
    unsafe { cancel(window as *mut c_void, buffer as *mut c_void, fence) }
}
pub(super) unsafe fn wsi_backend_window_queue(window: usize, buffer: usize, fence: i32) -> i32 {
    let queue = platform!(
        c"darwin_art_android_ANativeWindow_queue_hardware_buffer",
        unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> i32,
        -3
    );
    unsafe { queue(window as *mut c_void, buffer as *mut c_void, fence) }
}
pub(super) unsafe fn wsi_backend_buffer_size(ahb: usize) -> Result<(u32, u32), i32> {
    let describe = platform!(
        c"AHardwareBuffer_describe",
        unsafe extern "C" fn(*const c_void, *mut AHardwareBufferDesc),
        Err(-3)
    );
    let mut desc = AHardwareBufferDesc::default();
    unsafe { describe(ahb as *const c_void, &mut desc) };
    Ok((desc.width, desc.height))
}
pub(super) unsafe fn wsi_backend_buffer_identity(ahb: usize) -> Result<u32, i32> {
    let export = platform!(
        c"darwin_art_android_hardware_buffer_export_identity",
        unsafe extern "C" fn(*const c_void, *mut HardwareBufferIdentity) -> i32,
        Err(-3)
    );
    let mut identity = HardwareBufferIdentity::default();
    let status = unsafe { export(ahb as *const c_void, &mut identity) };
    if status != 0 || identity.surface_id == 0 {
        Err(if status != 0 { status } else { -3 })
    } else {
        Ok(identity.surface_id)
    }
}
pub(super) unsafe fn wsi_backend_close_fd(fd: i32) {
    if fd < 0 {
        return;
    }
    let close = platform!(
        c"darwin_art_bionic_socket_broker_close",
        unsafe extern "C" fn(i32) -> i32,
        ()
    );
    unsafe { close(fd) };
}

pub(super) unsafe fn wsi_backend_import_image(
    device: usize,
    ahb: usize,
    format: u32,
    w: u32,
    h: u32,
    usage: u32,
) -> Result<(usize, usize), i32> {
    let device = device as *mut c_void;
    let mut properties = VulkanAndroidHardwareBufferProperties {
        s_type: 1_000_129_001,
        p_next: ptr::null_mut(),
        allocation_size: 0,
        memory_type_bits: 0,
    };
    let result = unsafe {
        moltenvk_get_android_hardware_buffer_properties(
            device,
            ahb as *const c_void,
            &mut properties,
        )
    };
    if result != 0 || properties.memory_type_bits == 0 {
        return Err(if result != 0 { result } else { -11 });
    }
    let external = VulkanExternalMemoryImageCreateInfo {
        s_type: VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
        p_next: ptr::null(),
        handle_types: VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID,
    };
    let info = VulkanImageCreateInfo {
        s_type: 14,
        p_next: (&external as *const VulkanExternalMemoryImageCreateInfo).cast(),
        flags: 0,
        image_type: 1,
        format: format as i32,
        extent: [w, h, 1],
        mip_levels: 1,
        array_layers: 1,
        samples: 1,
        tiling: 0,
        usage,
        sharing_mode: 0,
        queue_family_index_count: 0,
        queue_family_indices: ptr::null(),
        initial_layout: 0,
    };
    let mut image = ptr::null_mut();
    let result = unsafe { moltenvk_create_image(device, &info, ptr::null(), &mut image) };
    if result != 0 {
        return Err(result);
    }
    let import = VulkanImportAndroidHardwareBufferInfo {
        s_type: VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID,
        p_next: ptr::null(),
        buffer: ahb as *mut c_void,
    };
    let dedicated = VulkanMemoryDedicatedAllocateInfo {
        s_type: VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
        p_next: (&import as *const VulkanImportAndroidHardwareBufferInfo).cast(),
        image,
        buffer: ptr::null_mut(),
    };
    let allocation = VulkanMemoryAllocateInfo {
        s_type: 5,
        p_next: (&dedicated as *const VulkanMemoryDedicatedAllocateInfo).cast(),
        allocation_size: properties.allocation_size,
        memory_type_index: properties.memory_type_bits.trailing_zeros(),
    };
    let mut memory = ptr::null_mut();
    let mut result =
        unsafe { moltenvk_allocate_memory(device, &allocation, ptr::null(), &mut memory) };
    if result == 0 {
        result = unsafe { moltenvk_bind_image_memory(device, image, memory, 0) };
    }
    if result != 0 {
        unsafe { wsi_backend_destroy_image(device as usize, image as usize, memory as usize) };
        return Err(result);
    }
    Ok((image as usize, memory as usize))
}
pub(super) unsafe fn wsi_backend_destroy_image(device: usize, image: usize, memory: usize) {
    forget_image_format(image);
    if image != 0 {
        let address = unsafe { raw_device_symbol(device, c"vkDestroyImage") };
        if !address.is_null() {
            let destroy: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void) =
                unsafe { std::mem::transmute(address) };
            unsafe { destroy(device as *mut c_void, image as *mut c_void, ptr::null()) };
        }
    }
    if memory != 0 {
        unsafe { moltenvk_free_memory(device as *mut c_void, memory as *mut c_void, ptr::null()) };
    }
}
pub(super) unsafe fn wsi_backend_create_completion_semaphore(device: usize) -> Result<usize, i32> {
    let info = VulkanSemaphoreCreateInfo {
        s_type: 9,
        p_next: ptr::null(),
        flags: 0,
    };
    let mut semaphore = ptr::null_mut();
    let result = unsafe {
        moltenvk_create_semaphore(device as *mut c_void, &info, ptr::null(), &mut semaphore)
    };
    if result == 0 {
        Ok(semaphore as usize)
    } else {
        Err(result)
    }
}
pub(super) unsafe fn wsi_backend_destroy_semaphore(device: usize, semaphore: usize) {
    unsafe {
        moltenvk_destroy_semaphore(device as *mut c_void, semaphore as *mut c_void, ptr::null())
    };
}
pub(super) unsafe fn wsi_backend_signal_acquire(
    device: usize,
    semaphore: usize,
    fence: usize,
) -> i32 {
    if semaphore != 0 {
        let result =
            unsafe { moltenvk_signal_wsi_acquire(device as *mut c_void, semaphore as *mut c_void) };
        if result != 0 {
            return result;
        }
    }
    if fence != 0 {
        vulkan_acquire_fences::mark_acquired(device, fence);
    }
    0
}

#[repr(C)]
struct SubmitInfo {
    s_type: i32,
    p_next: *const c_void,
    wait_count: u32,
    waits: *const usize,
    stages: *const u32,
    command_count: u32,
    commands: *const usize,
    signal_count: u32,
    signals: *const usize,
}
pub(super) unsafe fn wsi_backend_present(
    queue: usize,
    device: usize,
    waits: &[usize],
    completion: usize,
) -> Result<i32, i32> {
    let address = unsafe { raw_device_symbol(device, c"vkQueueSubmit") };
    if address.is_null() {
        return Err(-3);
    }
    let submit: unsafe extern "C" fn(*mut c_void, u32, *const SubmitInfo, usize) -> i32 =
        unsafe { std::mem::transmute(address) };
    let stages = vec![0x10000; waits.len()]; // ALL_COMMANDS
    let consume_stage = 0x10000;
    // Consume our binary semaphore payload too: MoltenVK advances its shared
    // event value on wait, not signal. Otherwise reuse hangs after one cycle.
    let submissions = [
        SubmitInfo {
            s_type: 4,
            p_next: ptr::null(),
            wait_count: waits.len() as u32,
            waits: waits.as_ptr(),
            stages: stages.as_ptr(),
            command_count: 0,
            commands: ptr::null(),
            signal_count: 1,
            signals: &completion,
        },
        SubmitInfo {
            s_type: 4,
            p_next: ptr::null(),
            wait_count: 1,
            waits: &completion,
            stages: &consume_stage,
            command_count: 0,
            commands: ptr::null(),
            signal_count: 0,
            signals: ptr::null(),
        },
    ];
    let result = unsafe { submit(queue as *mut c_void, 2, submissions.as_ptr(), 0) };
    if result != 0 {
        return Err(result);
    }
    let info = VulkanSemaphoreGetFdInfo {
        s_type: 1_000_079_001,
        p_next: ptr::null(),
        semaphore: completion as *mut c_void,
        handle_type: VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
    };
    let mut fd = -1;
    let result = unsafe { moltenvk_get_semaphore_fd(device as *mut c_void, &info, &mut fd) };
    if result == 0 {
        Ok(fd)
    } else {
        Err(result)
    }
}
pub(super) unsafe fn wsi_backend_wait_idle(device: usize) -> i32 {
    let address = unsafe { raw_device_symbol(device, c"vkDeviceWaitIdle") };
    if address.is_null() {
        return -3;
    }
    let wait: unsafe extern "C" fn(*mut c_void) -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { wait(device as *mut c_void) }
}
