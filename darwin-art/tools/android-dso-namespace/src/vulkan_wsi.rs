//! Android Vulkan WSI backed by the emulated `ANativeWindow` buffer queue.
//!
//! This module intentionally contains no host-window or CPU-pixel path.  A
//! swapchain image is the Vulkan image imported from the `AHardwareBuffer`
//! returned by the native window.  The native-buffer token is retained until
//! queue-present gives it back to the window.

use crate::vulkan_wsi_backend as backend;
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const VK_SUCCESS: i32 = 0;
const VK_NOT_READY: i32 = 1;
const VK_TIMEOUT: i32 = 2;
const VK_ERROR_INITIALIZATION_FAILED: i32 = -3;
const VK_ERROR_FEATURE_NOT_PRESENT: i32 = -8;
const VK_ERROR_SURFACE_LOST_KHR: i32 = -1_000_000_000;
const VK_ERROR_OUT_OF_DATE_KHR: i32 = -1_000_100_004;
const VK_INCOMPLETE: i32 = 5;

const VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR: i32 = 1_000_008_000;
const VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR: i32 = 1_000_001_000;
const VK_STRUCTURE_TYPE_PRESENT_INFO_KHR: i32 = 1_000_001_001;
const VK_STRUCTURE_TYPE_ACQUIRE_NEXT_IMAGE_INFO_KHR: i32 = 1_000_060_010;
const VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SURFACE_INFO_2_KHR: i32 = 1_000_119_000;
const VK_STRUCTURE_TYPE_SURFACE_CAPABILITIES_2_KHR: i32 = 1_000_119_001;
const VK_STRUCTURE_TYPE_SURFACE_FORMAT_2_KHR: i32 = 1_000_119_002;

const VK_FORMAT_R8G8B8A8_UNORM: i32 = 37;
const SURFACE_FORMATS: [i32; 2] = [VK_FORMAT_R8G8B8A8_UNORM, 43];
const VK_COLOR_SPACE_SRGB_NONLINEAR_KHR: i32 = 0;
const VK_PRESENT_MODE_FIFO_KHR: i32 = 2;
const VK_PRESENT_MODE_MAILBOX_KHR: i32 = 1;
const VK_IMAGE_USAGE_TRANSFER_SRC_BIT: u32 = 1;
const VK_IMAGE_USAGE_TRANSFER_DST_BIT: u32 = 2;
const VK_IMAGE_USAGE_SAMPLED_BIT: u32 = 4;
const VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT: u32 = 0x10;
const SUPPORTED_USAGE: u32 = VK_IMAGE_USAGE_TRANSFER_SRC_BIT
    | VK_IMAGE_USAGE_TRANSFER_DST_BIT
    | VK_IMAGE_USAGE_SAMPLED_BIT
    | VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
const VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR: u32 = 1;
const VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR: u32 = 1;

type VkInstance = *mut c_void;
type VkPhysicalDevice = *mut c_void;
type VkDevice = *mut c_void;
type VkQueue = *mut c_void;
// NDK r28 uses pointer-sized non-dispatchable handles on LP64.  The Android
// runtime and this static library are both LP64, so `u64` is the exact ABI.
type VkHandle = u64;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VkExtent2D {
    width: u32,
    height: u32,
}

#[repr(C)]
struct VkAndroidSurfaceCreateInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    window: *mut c_void,
}

#[repr(C)]
struct VkSwapchainCreateInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    surface: VkHandle,
    min_image_count: u32,
    image_format: i32,
    image_color_space: i32,
    image_extent: VkExtent2D,
    image_array_layers: u32,
    image_usage: u32,
    image_sharing_mode: i32,
    queue_family_index_count: u32,
    queue_family_indices: *const u32,
    pre_transform: u32,
    composite_alpha: u32,
    present_mode: i32,
    clipped: u32,
    old_swapchain: VkHandle,
}

#[repr(C)]
struct VkPresentInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    wait_semaphore_count: u32,
    wait_semaphores: *const VkHandle,
    swapchain_count: u32,
    swapchains: *const VkHandle,
    image_indices: *const u32,
    results: *mut i32,
}

#[repr(C)]
struct VkAcquireNextImageInfoKHR {
    s_type: i32,
    p_next: *const c_void,
    swapchain: VkHandle,
    timeout: u64,
    semaphore: VkHandle,
    fence: VkHandle,
    device_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkSurfaceCapabilitiesKHR {
    min_image_count: u32,
    max_image_count: u32,
    current_extent: VkExtent2D,
    min_image_extent: VkExtent2D,
    max_image_extent: VkExtent2D,
    max_image_array_layers: u32,
    supported_transforms: u32,
    current_transform: u32,
    supported_composite_alpha: u32,
    supported_usage_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkSurfaceFormatKHR {
    format: i32,
    color_space: i32,
}

#[repr(C)]
struct VkPhysicalDeviceSurfaceInfo2KHR {
    s_type: i32,
    p_next: *const c_void,
    surface: VkHandle,
}

#[repr(C)]
struct VkSurfaceCapabilities2KHR {
    s_type: i32,
    p_next: *mut c_void,
    surface_capabilities: VkSurfaceCapabilitiesKHR,
}

#[repr(C)]
struct VkSurfaceFormat2KHR {
    s_type: i32,
    p_next: *mut c_void,
    surface_format: VkSurfaceFormatKHR,
}

struct Surface {
    instance: usize,
    window: usize,
    state: Mutex<SurfaceState>,
}

struct SurfaceState {
    extent: VkExtent2D,
    generation: u64,
}

struct ImageSlot {
    surface_id: u32,
    native_buffer: usize,
    image: VkHandle,
    memory: usize,
    completion_semaphore: usize,
    acquired: bool,
}

struct Swapchain {
    closing: std::sync::atomic::AtomicBool,
    device: usize,
    surface: usize,
    window: usize,
    extent: VkExtent2D,
    generation: u64,
    slots: Mutex<Vec<ImageSlot>>,
}

static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(0x1000);
static PRESENT_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SURFACES: OnceLock<Mutex<HashMap<usize, Surface>>> = OnceLock::new();
static SWAPCHAINS: OnceLock<Mutex<HashMap<usize, Arc<Swapchain>>>> = OnceLock::new();

fn surfaces() -> &'static Mutex<HashMap<usize, Surface>> {
    SURFACES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn swapchains() -> &'static Mutex<HashMap<usize, Arc<Swapchain>>> {
    SWAPCHAINS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_handle() -> usize {
    loop {
        let value = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        if value != 0 {
            return value;
        }
    }
}

fn as_handle(value: usize) -> VkHandle {
    value as VkHandle
}

fn from_handle(value: VkHandle) -> usize {
    value as usize
}

fn trace_failure(operation: &str, result: i32) {
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan WSI: pid={} {operation} result={result}",
            std::process::id()
        );
    }
}

fn trace_present(result: i32, image_index: usize) {
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some()
        && PRESENT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 4
    {
        eprintln!(
            "ART Android Vulkan WSI: pid={} present image={image_index} result={result}",
            std::process::id()
        );
    }
}

fn trace_swapchain_request(info: &VkSwapchainCreateInfoKHR) {
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan WSI: pid={} create-swapchain request min={} format={} color={} extent={}x{} layers={} usage={:#x} sharing={} pre={} alpha={} mode={}",
            std::process::id(),
            info.min_image_count,
            info.image_format,
            info.image_color_space,
            info.image_extent.width,
            info.image_extent.height,
            info.image_array_layers,
            info.image_usage,
            info.image_sharing_mode,
            info.pre_transform,
            info.composite_alpha,
            info.present_mode
        );
    }
}

fn valid_pnext_s_type(actual: i32, expected: i32) -> bool {
    actual == expected
}

fn current_surface(surface_handle: usize) -> Result<(usize, VkExtent2D, u64), i32> {
    let registry = surfaces().lock().expect("Vulkan surface registry poisoned");
    let surface = registry
        .get(&surface_handle)
        .ok_or(VK_ERROR_SURFACE_LOST_KHR)?;
    let extent_result = unsafe { backend::wsi_backend_window_size(surface.window) };
    let mut state = surface.state.lock().expect("Vulkan surface poisoned");
    let (width, height) = extent_result.map_err(|_| VK_ERROR_SURFACE_LOST_KHR)?;
    if width == 0 || height == 0 {
        return Err(VK_ERROR_SURFACE_LOST_KHR);
    }
    if width != state.extent.width || height != state.extent.height {
        state.extent = VkExtent2D { width, height };
        state.generation = state.generation.saturating_add(1);
    }
    if state.extent.width == 0 || state.extent.height == 0 {
        return Err(VK_ERROR_SURFACE_LOST_KHR);
    }
    Ok((surface.window, state.extent, state.generation))
}

fn surface_caps(extent: VkExtent2D) -> VkSurfaceCapabilitiesKHR {
    VkSurfaceCapabilitiesKHR {
        min_image_count: 3,
        max_image_count: 3,
        current_extent: extent,
        min_image_extent: extent,
        max_image_extent: extent,
        max_image_array_layers: 1,
        supported_transforms: VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR,
        current_transform: VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR,
        supported_composite_alpha: VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR,
        supported_usage_flags: SUPPORTED_USAGE,
    }
}

fn close_fence(fence: i32) {
    if fence >= 0 {
        unsafe { backend::wsi_backend_close_fd(fence) };
    }
}

fn cancel_native(window: usize, native_buffer: usize, fence: i32) {
    // The native bridge owns a supplied fence (and closes it on cancellation).
    // Never close it again here: Darwin's broker descriptors are not reusable.
    unsafe { backend::wsi_backend_window_cancel(window, native_buffer, fence) };
}

fn destroy_slots(device: usize, window: usize, slots: &mut [ImageSlot]) {
    // The caller has already waited for queue work.  Native buffers therefore
    // can be returned without a GPU fence, and imported images can be torn
    // down while no global registry lock is held.
    for slot in slots {
        if slot.acquired {
            cancel_native(window, slot.native_buffer, -1);
            slot.acquired = false;
        }
        unsafe { backend::wsi_backend_destroy_image(device, slot.image as usize, slot.memory) };
        unsafe { backend::wsi_backend_destroy_semaphore(device, slot.completion_semaphore) };
    }
}

fn result_for_dequeue_error(error: i32) -> i32 {
    if error == -16 || error == -11 || error == -35 {
        VK_ERROR_OUT_OF_DATE_KHR
    } else {
        VK_ERROR_SURFACE_LOST_KHR
    }
}

fn acquire_next(
    device: usize,
    swapchain_handle: usize,
    timeout: u64,
    semaphore: VkHandle,
    fence: VkHandle,
    image_index: *mut u32,
) -> i32 {
    if image_index.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let swapchain = {
        let registry = swapchains()
            .lock()
            .expect("Vulkan swapchain registry poisoned");
        registry.get(&swapchain_handle).cloned()
    };
    let Some(swapchain) = swapchain else {
        return VK_ERROR_OUT_OF_DATE_KHR;
    };
    if swapchain.device != device {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let Ok((window, current_extent, generation)) = current_surface(swapchain.surface) else {
        return VK_ERROR_OUT_OF_DATE_KHR;
    };
    if generation != swapchain.generation
        || current_extent.width != swapchain.extent.width
        || current_extent.height != swapchain.extent.height
    {
        return VK_ERROR_OUT_OF_DATE_KHR;
    }
    let start = Instant::now();
    let deadline = if timeout == u64::MAX {
        None
    } else {
        Some(start + Duration::from_nanos(timeout))
    };
    loop {
        if swapchain.closing.load(Ordering::Acquire) {
            return VK_ERROR_OUT_OF_DATE_KHR;
        }
        match unsafe { backend::wsi_backend_window_dequeue(window) } {
            Ok((ahb, native_buffer, native_fence)) => {
                // A non-negative native fence is not expected from the Darwin
                // queue.  The bridge owns no wait primitive here; close it
                // before handing the slot to Vulkan's acquire synchronization.
                close_fence(native_fence);
                let mut slots = swapchain
                    .slots
                    .lock()
                    .expect("Vulkan swapchain slots poisoned");
                if swapchain.closing.load(Ordering::Acquire) {
                    drop(slots);
                    cancel_native(window, native_buffer, -1);
                    return VK_ERROR_OUT_OF_DATE_KHR;
                }
                let surface_id = match unsafe { backend::wsi_backend_buffer_identity(ahb) } {
                    Ok(id) => id,
                    Err(_) => {
                        drop(slots);
                        cancel_native(window, native_buffer, -1);
                        return VK_ERROR_OUT_OF_DATE_KHR;
                    }
                };
                let Some(index) = slots
                    .iter()
                    .position(|slot| slot.surface_id == surface_id && !slot.acquired)
                else {
                    trace_failure("acquire-unknown-buffer-identity", VK_ERROR_OUT_OF_DATE_KHR);
                    drop(slots);
                    cancel_native(window, native_buffer, -1);
                    return VK_ERROR_OUT_OF_DATE_KHR;
                };
                slots[index].native_buffer = native_buffer;
                slots[index].acquired = true;
                drop(slots);
                if semaphore != 0 || fence != 0 {
                    let signal_result = unsafe {
                        backend::wsi_backend_signal_acquire(
                            device,
                            from_handle(semaphore),
                            from_handle(fence),
                        )
                    };
                    if signal_result != VK_SUCCESS {
                        if let Ok(mut slots) = swapchain.slots.lock() {
                            slots[index].acquired = false;
                        }
                        cancel_native(window, native_buffer, -1);
                        return signal_result;
                    }
                }
                unsafe { *image_index = index as u32 };
                return VK_SUCCESS;
            }
            Err(-16) => {
                if timeout == 0 {
                    return VK_NOT_READY;
                }
                if let Some(end) = deadline {
                    if Instant::now() >= end {
                        return VK_TIMEOUT;
                    }
                }
                // The native bridge has a condition-backed queue, but its
                // dequeue ABI is nonblocking.  A short yield keeps bounded
                // acquire responsive without a hot spin on the render thread.
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return result_for_dequeue_error(error),
        }
    }
}

unsafe extern "C" fn create_android_surface(
    instance: VkInstance,
    create_info: *const VkAndroidSurfaceCreateInfoKHR,
    _allocator: *const c_void,
    output: *mut VkHandle,
) -> i32 {
    if create_info.is_null() || output.is_null() {
        trace_failure(
            "create-surface-invalid-args",
            VK_ERROR_INITIALIZATION_FAILED,
        );
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let info = &*create_info;
    if !valid_pnext_s_type(
        info.s_type,
        VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR,
    ) || info.window.is_null()
    {
        *output = 0;
        trace_failure(
            "create-surface-invalid-info",
            VK_ERROR_INITIALIZATION_FAILED,
        );
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let window = info.window as usize;
    if !backend::wsi_backend_window_acquire(window) {
        *output = 0;
        trace_failure("create-surface-window", VK_ERROR_SURFACE_LOST_KHR);
        return VK_ERROR_SURFACE_LOST_KHR;
    }
    let extent = match backend::wsi_backend_window_size(window) {
        Ok((width, height)) if width != 0 && height != 0 => VkExtent2D { width, height },
        _ => {
            backend::wsi_backend_window_release(window);
            *output = 0;
            trace_failure("create-surface-size", VK_ERROR_INITIALIZATION_FAILED);
            return VK_ERROR_INITIALIZATION_FAILED;
        }
    };
    let handle = new_handle();
    surfaces()
        .lock()
        .expect("Vulkan surface registry poisoned")
        .insert(
            handle,
            Surface {
                instance: instance as usize,
                window,
                state: Mutex::new(SurfaceState {
                    extent,
                    generation: 1,
                }),
            },
        );
    *output = as_handle(handle);
    VK_SUCCESS
}

unsafe extern "C" fn destroy_surface(
    instance: VkInstance,
    surface: VkHandle,
    _allocator: *const c_void,
) {
    let handle = from_handle(surface);
    let removed = surfaces()
        .lock()
        .expect("Vulkan surface registry poisoned")
        .remove(&handle);
    if let Some(surface) = removed {
        if surface.instance == instance as usize {
            backend::wsi_backend_window_release(surface.window);
        } else {
            // Vulkan requires the matching instance.  Keep cleanup safe even
            // for a malformed caller rather than leaking the ANativeWindow.
            backend::wsi_backend_window_release(surface.window);
        }
    }
}

unsafe extern "C" fn get_surface_support(
    _physical_device: VkPhysicalDevice,
    _queue_family_index: u32,
    surface: VkHandle,
    supported: *mut u32,
) -> i32 {
    if supported.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if current_surface(from_handle(surface)).is_err() {
        *supported = 0;
        return VK_ERROR_SURFACE_LOST_KHR;
    }
    *supported = 1;
    VK_SUCCESS
}

unsafe extern "C" fn get_surface_capabilities(
    _physical_device: VkPhysicalDevice,
    surface: VkHandle,
    capabilities: *mut VkSurfaceCapabilitiesKHR,
) -> i32 {
    if capabilities.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let Ok((_window, extent, _generation)) = current_surface(from_handle(surface)) else {
        return VK_ERROR_SURFACE_LOST_KHR;
    };
    *capabilities = surface_caps(extent);
    VK_SUCCESS
}

unsafe extern "C" fn get_surface_formats(
    _physical_device: VkPhysicalDevice,
    surface: VkHandle,
    count: *mut u32,
    formats: *mut VkSurfaceFormatKHR,
) -> i32 {
    if count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if current_surface(from_handle(surface)).is_err() {
        return VK_ERROR_SURFACE_LOST_KHR;
    }
    if formats.is_null() {
        *count = SURFACE_FORMATS.len() as u32;
        return VK_SUCCESS;
    }
    if *count == 0 {
        return VK_INCOMPLETE;
    }
    let written = (*count as usize).min(SURFACE_FORMATS.len());
    for (index, format) in SURFACE_FORMATS.iter().take(written).enumerate() {
        *formats.add(index) = VkSurfaceFormatKHR {
            format: *format,
            color_space: VK_COLOR_SPACE_SRGB_NONLINEAR_KHR,
        };
    }
    *count = written as u32;
    if written < SURFACE_FORMATS.len() {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

unsafe extern "C" fn get_surface_present_modes(
    _physical_device: VkPhysicalDevice,
    surface: VkHandle,
    count: *mut u32,
    modes: *mut i32,
) -> i32 {
    if count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let Ok((window, _, _)) = current_surface(from_handle(surface)) else {
        return VK_ERROR_SURFACE_LOST_KHR;
    };
    let mailbox = unsafe { backend::wsi_backend_window_supports_mailbox(window) };
    if modes.is_null() {
        *count = if mailbox { 2 } else { 1 };
        return VK_SUCCESS;
    }
    if *count == 0 {
        return VK_INCOMPLETE;
    }
    let available = [VK_PRESENT_MODE_FIFO_KHR, VK_PRESENT_MODE_MAILBOX_KHR];
    let available_count = if mailbox { 2 } else { 1 };
    let written = (*count as usize).min(available_count);
    for (index, mode) in available.iter().take(written).enumerate() {
        *modes.add(index) = *mode;
    }
    *count = written as u32;
    if written < available_count {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

unsafe extern "C" fn create_swapchain(
    device: VkDevice,
    create_info: *const VkSwapchainCreateInfoKHR,
    _allocator: *const c_void,
    output: *mut VkHandle,
) -> i32 {
    if create_info.is_null() || output.is_null() {
        trace_failure(
            "create-swapchain-invalid-args",
            VK_ERROR_INITIALIZATION_FAILED,
        );
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    *output = 0;
    let info = &*create_info;
    trace_swapchain_request(info);
    if !valid_pnext_s_type(info.s_type, VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR) {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!("ART Android Vulkan WSI: pid={} requested swapchain type={} surface={:#x} count={} format={} colorspace={} size={}x{} layers={} usage={:#x} transform={:#x} alpha={:#x} mode={} flags={:#x}", std::process::id(), info.s_type,info.surface,info.min_image_count,info.image_format,info.image_color_space,info.image_extent.width,info.image_extent.height,info.image_array_layers,info.image_usage,info.pre_transform,info.composite_alpha,info.present_mode,info.flags);
    }
    let surface_handle = from_handle(info.surface);
    let Ok((window, extent, generation)) = current_surface(surface_handle) else {
        trace_failure("create-swapchain-surface", VK_ERROR_OUT_OF_DATE_KHR);
        return VK_ERROR_OUT_OF_DATE_KHR;
    };
    let mailbox = info.present_mode == VK_PRESENT_MODE_MAILBOX_KHR;
    if info.surface == 0
        || info.min_image_count != 3
        || !SURFACE_FORMATS.contains(&info.image_format)
        || info.image_color_space != VK_COLOR_SPACE_SRGB_NONLINEAR_KHR
        || info.image_extent.width == 0
        || info.image_extent.height == 0
        || info.image_array_layers != 1
        || (info.present_mode != VK_PRESENT_MODE_FIFO_KHR
            && (!mailbox || !unsafe { backend::wsi_backend_window_supports_mailbox(window) }))
        || info.image_usage & !SUPPORTED_USAGE != 0
        || info.pre_transform != VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR
        || info.composite_alpha != VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR
    {
        trace_failure("create-swapchain-unsupported", VK_ERROR_FEATURE_NOT_PRESENT);
        return VK_ERROR_FEATURE_NOT_PRESENT;
    }
    if extent.width != info.image_extent.width || extent.height != info.image_extent.height {
        trace_failure("create-swapchain-extent", VK_ERROR_OUT_OF_DATE_KHR);
        return VK_ERROR_OUT_OF_DATE_KHR;
    }
    let mode_result =
        unsafe { backend::wsi_backend_window_set_present_mode(window, info.present_mode) };
    if mode_result != 0 {
        trace_failure("create-swapchain-present-mode", mode_result);
        return VK_ERROR_FEATURE_NOT_PRESENT;
    }
    let device_value = device as usize;
    let geometry_result = backend::wsi_backend_window_geometry(window, extent.width, extent.height);
    if geometry_result != 0 {
        trace_failure("create-swapchain-geometry", geometry_result);
        return if geometry_result == VK_ERROR_OUT_OF_DATE_KHR {
            geometry_result
        } else {
            VK_ERROR_INITIALIZATION_FAILED
        };
    }
    let mut slots: Vec<ImageSlot> = Vec::with_capacity(3);
    for _ in 0..3 {
        let (ahb, native_buffer, fence) = match backend::wsi_backend_window_dequeue(window) {
            Ok(value) => value,
            Err(error) => {
                destroy_slots(device_value, window, &mut slots);
                let result = result_for_dequeue_error(error);
                trace_failure("create-swapchain-dequeue", result);
                return result;
            }
        };
        close_fence(fence);
        let surface_id = match backend::wsi_backend_buffer_identity(ahb) {
            Ok(id) => id,
            Err(_) => {
                cancel_native(window, native_buffer, -1);
                destroy_slots(device_value, window, &mut slots);
                trace_failure("create-swapchain-buffer-identity", VK_ERROR_OUT_OF_DATE_KHR);
                return VK_ERROR_OUT_OF_DATE_KHR;
            }
        };
        if slots.iter().any(|slot: &ImageSlot| slot.surface_id == surface_id) {
            cancel_native(window, native_buffer, -1);
            destroy_slots(device_value, window, &mut slots);
            trace_failure(
                "create-swapchain-duplicate-buffer",
                VK_ERROR_OUT_OF_DATE_KHR,
            );
            return VK_ERROR_OUT_OF_DATE_KHR;
        }
        let Ok((buffer_width, buffer_height)) = backend::wsi_backend_buffer_size(ahb) else {
            cancel_native(window, native_buffer, -1);
            destroy_slots(device_value, window, &mut slots);
            trace_failure("create-swapchain-buffer-size", VK_ERROR_OUT_OF_DATE_KHR);
            return VK_ERROR_OUT_OF_DATE_KHR;
        };
        if buffer_width != extent.width || buffer_height != extent.height {
            cancel_native(window, native_buffer, -1);
            destroy_slots(device_value, window, &mut slots);
            trace_failure("create-swapchain-buffer-extent", VK_ERROR_OUT_OF_DATE_KHR);
            return VK_ERROR_OUT_OF_DATE_KHR;
        }
        let (image, memory) = match backend::wsi_backend_import_image(
            device_value,
            ahb,
            info.image_format as u32,
            extent.width,
            extent.height,
            info.image_usage,
        ) {
            Ok((image, memory)) if image != 0 && memory != 0 => (image, memory),
            Ok(_) => {
                cancel_native(window, native_buffer, -1);
                destroy_slots(device_value, window, &mut slots);
                trace_failure(
                    "create-swapchain-import-zero",
                    VK_ERROR_INITIALIZATION_FAILED,
                );
                return VK_ERROR_INITIALIZATION_FAILED;
            }
            Err(error) => {
                cancel_native(window, native_buffer, -1);
                destroy_slots(device_value, window, &mut slots);
                trace_failure("create-swapchain-import", error);
                return error;
            }
        };
        let completion_semaphore =
            match backend::wsi_backend_create_completion_semaphore(device_value) {
                Ok(value) if value != 0 => value,
                Ok(_) => {
                    cancel_native(window, native_buffer, -1);
                    unsafe { backend::wsi_backend_destroy_image(device_value, image, memory) };
                    destroy_slots(device_value, window, &mut slots);
                    trace_failure(
                        "create-swapchain-completion-semaphore",
                        VK_ERROR_INITIALIZATION_FAILED,
                    );
                    return VK_ERROR_INITIALIZATION_FAILED;
                }
                Err(error) => {
                    cancel_native(window, native_buffer, -1);
                    unsafe { backend::wsi_backend_destroy_image(device_value, image, memory) };
                    destroy_slots(device_value, window, &mut slots);
                    trace_failure("create-swapchain-completion-semaphore", error);
                    return error;
                }
            };
        slots.push(ImageSlot {
            surface_id,
            native_buffer,
            image: as_handle(image),
            memory,
            completion_semaphore,
            acquired: true,
        });
    }
    for slot in &mut slots {
        cancel_native(window, slot.native_buffer, -1);
        slot.acquired = false;
    }
    let handle = new_handle();
    swapchains()
        .lock()
        .expect("Vulkan swapchain registry poisoned")
        .insert(
            handle,
            Arc::new(Swapchain {
                closing: std::sync::atomic::AtomicBool::new(false),
                device: device_value,
                surface: surface_handle,
                window,
                extent,
                generation,
                slots: Mutex::new(slots),
            }),
        );
    *output = as_handle(handle);
    VK_SUCCESS
}

unsafe extern "C" fn destroy_swapchain(
    device: VkDevice,
    swapchain: VkHandle,
    _allocator: *const c_void,
) {
    let handle = from_handle(swapchain);
    let removed = swapchains()
        .lock()
        .expect("Vulkan swapchain registry poisoned")
        .remove(&handle);
    let Some(swapchain) = removed else { return };
    let device_value = device as usize;
    swapchain.closing.store(true, Ordering::Release);
    // This is deliberately outside the global swapchain registry lock.  The
    // backend serializes queue work and guarantees all imported images are no
    // longer in use after wait-idle.
    let _ = backend::wsi_backend_wait_idle(device_value);
    let mut slots = swapchain
        .slots
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    destroy_slots(device_value, swapchain.window, &mut slots);
}

unsafe extern "C" fn get_swapchain_images(
    device: VkDevice,
    swapchain: VkHandle,
    count: *mut u32,
    images: *mut VkHandle,
) -> i32 {
    if count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let swapchain = {
        let registry = swapchains()
            .lock()
            .expect("Vulkan swapchain registry poisoned");
        registry.get(&from_handle(swapchain)).cloned()
    };
    let Some(swapchain) = swapchain else {
        return VK_ERROR_OUT_OF_DATE_KHR;
    };
    if swapchain.device != device as usize {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if images.is_null() {
        *count = 3;
        return VK_SUCCESS;
    }
    let capacity = *count as usize;
    let slots = swapchain
        .slots
        .lock()
        .expect("Vulkan swapchain slots poisoned");
    let written = capacity.min(slots.len());
    for (index, slot) in slots.iter().take(written).enumerate() {
        *images.add(index) = slot.image;
    }
    *count = written as u32;
    if written < slots.len() {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

unsafe extern "C" fn acquire_next_image(
    device: VkDevice,
    swapchain: VkHandle,
    timeout: u64,
    semaphore: VkHandle,
    fence: VkHandle,
    image_index: *mut u32,
) -> i32 {
    acquire_next(
        device as usize,
        from_handle(swapchain),
        timeout,
        semaphore,
        fence,
        image_index,
    )
}

unsafe extern "C" fn acquire_next_image2(
    device: VkDevice,
    acquire_info: *const VkAcquireNextImageInfoKHR,
    image_index: *mut u32,
) -> i32 {
    if acquire_info.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let info = &*acquire_info;
    if !valid_pnext_s_type(info.s_type, VK_STRUCTURE_TYPE_ACQUIRE_NEXT_IMAGE_INFO_KHR) {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if info.device_mask == 0 || info.device_mask.count_ones() != 1 {
        return VK_ERROR_FEATURE_NOT_PRESENT;
    }
    acquire_next(
        device as usize,
        from_handle(info.swapchain),
        info.timeout,
        info.semaphore,
        info.fence,
        image_index,
    )
}

unsafe extern "C" fn queue_present(queue: VkQueue, present_info: *const VkPresentInfoKHR) -> i32 {
    if present_info.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let info = &*present_info;
    if !valid_pnext_s_type(info.s_type, VK_STRUCTURE_TYPE_PRESENT_INFO_KHR)
        || (info.swapchain_count != 0
            && (info.swapchains.is_null() || info.image_indices.is_null()))
        || (info.wait_semaphore_count != 0 && info.wait_semaphores.is_null())
    {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    // The backend's ordered submit/export operation has one completion
    // semaphore per image slot.  Android engines in this path present one
    // swapchain; rejecting a multi-swapchain packet avoids consuming a binary
    // wait semaphore more than once.
    if info.swapchain_count > 1 {
        if !info.results.is_null() {
            for index in 0..info.swapchain_count as usize {
                *info.results.add(index) = VK_ERROR_FEATURE_NOT_PRESENT;
            }
        }
        return VK_ERROR_FEATURE_NOT_PRESENT;
    }
    let wait_semaphores = if info.wait_semaphore_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(info.wait_semaphores, info.wait_semaphore_count as usize)
    };
    let mut overall = VK_SUCCESS;
    for index in 0..info.swapchain_count as usize {
        let swapchain_handle = from_handle(*info.swapchains.add(index));
        let image_index = *info.image_indices.add(index) as usize;
        let swapchain = {
            let registry = swapchains()
                .lock()
                .expect("Vulkan swapchain registry poisoned");
            registry.get(&swapchain_handle).cloned()
        };
        let Some(swapchain) = swapchain else {
            if !info.results.is_null() {
                *info.results.add(index) = VK_ERROR_OUT_OF_DATE_KHR;
            }
            overall = VK_ERROR_OUT_OF_DATE_KHR;
            continue;
        };
        if swapchain.closing.load(Ordering::Acquire) {
            if !info.results.is_null() {
                *info.results.add(index) = VK_ERROR_OUT_OF_DATE_KHR;
            }
            overall = VK_ERROR_OUT_OF_DATE_KHR;
            continue;
        }
        let Ok((_window, extent, generation)) = current_surface(swapchain.surface) else {
            if !info.results.is_null() {
                *info.results.add(index) = VK_ERROR_OUT_OF_DATE_KHR;
            }
            overall = VK_ERROR_OUT_OF_DATE_KHR;
            continue;
        };
        if generation != swapchain.generation
            || extent.width != swapchain.extent.width
            || extent.height != swapchain.extent.height
        {
            if !info.results.is_null() {
                *info.results.add(index) = VK_ERROR_OUT_OF_DATE_KHR;
            }
            overall = VK_ERROR_OUT_OF_DATE_KHR;
            continue;
        }
        let mut slots = swapchain
            .slots
            .lock()
            .expect("Vulkan swapchain slots poisoned");
        if swapchain.closing.load(Ordering::Acquire) {
            if !info.results.is_null() {
                *info.results.add(index) = VK_ERROR_OUT_OF_DATE_KHR;
            }
            overall = VK_ERROR_OUT_OF_DATE_KHR;
            continue;
        }
        if image_index >= slots.len() || !slots[image_index].acquired {
            if !info.results.is_null() {
                *info.results.add(index) = VK_ERROR_INITIALIZATION_FAILED;
            }
            overall = VK_ERROR_INITIALIZATION_FAILED;
            continue;
        }
        let result = match backend::wsi_backend_present(
            queue as usize,
            swapchain.device,
            wait_semaphores
                .iter()
                .map(|value| from_handle(*value))
                .collect::<Vec<_>>()
                .as_slice(),
            slots[image_index].completion_semaphore,
        ) {
            Ok(fence_fd) => {
                let native_buffer = slots[image_index].native_buffer;
                let queue_result =
                    backend::wsi_backend_window_queue(swapchain.window, native_buffer, fence_fd);
                if queue_result == 0 {
                    slots[image_index].acquired = false;
                    VK_SUCCESS
                } else {
                    close_fence(fence_fd);
                    queue_result
                }
            }
            Err(error) => error,
        };
        if !info.results.is_null() {
            *info.results.add(index) = result;
        }
        trace_present(result, image_index);
        if result != VK_SUCCESS && overall == VK_SUCCESS {
            overall = result;
        }
    }
    overall
}

unsafe extern "C" fn get_surface_capabilities2(
    _physical_device: VkPhysicalDevice,
    info: *const VkPhysicalDeviceSurfaceInfo2KHR,
    output: *mut VkSurfaceCapabilities2KHR,
) -> i32 {
    if info.is_null() || output.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if (*info).s_type != VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SURFACE_INFO_2_KHR
        || (*output).s_type != VK_STRUCTURE_TYPE_SURFACE_CAPABILITIES_2_KHR
    {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    get_surface_capabilities(
        ptr::null_mut(),
        (*info).surface,
        &mut (*output).surface_capabilities,
    )
}

unsafe extern "C" fn get_surface_formats2(
    _physical_device: VkPhysicalDevice,
    info: *const VkPhysicalDeviceSurfaceInfo2KHR,
    count: *mut u32,
    formats: *mut VkSurfaceFormat2KHR,
) -> i32 {
    if info.is_null() || count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if (*info).s_type != VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SURFACE_INFO_2_KHR {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    if formats.is_null() {
        *count = SURFACE_FORMATS.len() as u32;
        return if current_surface(from_handle((*info).surface)).is_ok() {
            VK_SUCCESS
        } else {
            VK_ERROR_SURFACE_LOST_KHR
        };
    }
    if *count == 0 {
        return VK_INCOMPLETE;
    }
    if current_surface(from_handle((*info).surface)).is_err() {
        return VK_ERROR_SURFACE_LOST_KHR;
    }
    let written = (*count as usize).min(SURFACE_FORMATS.len());
    for (index, format) in SURFACE_FORMATS.iter().take(written).enumerate() {
        let entry = &mut *formats.add(index);
        if entry.s_type != VK_STRUCTURE_TYPE_SURFACE_FORMAT_2_KHR {
            return VK_ERROR_INITIALIZATION_FAILED;
        }
        entry.surface_format = VkSurfaceFormatKHR {
            format: *format,
            color_space: VK_COLOR_SPACE_SRGB_NONLINEAR_KHR,
        };
    }
    *count = written as u32;
    if written < SURFACE_FORMATS.len() {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

/// Looks up the Android WSI entrypoints exposed by this virtual Vulkan DSO.
pub(crate) fn lookup(name: &CStr) -> Option<*mut c_void> {
    let address = match name.to_bytes() {
        b"vkCreateAndroidSurfaceKHR" => create_android_surface as *mut c_void,
        b"vkDestroySurfaceKHR" => destroy_surface as *mut c_void,
        b"vkGetPhysicalDeviceSurfaceSupportKHR" => get_surface_support as *mut c_void,
        b"vkGetPhysicalDeviceSurfaceCapabilitiesKHR" => get_surface_capabilities as *mut c_void,
        b"vkGetPhysicalDeviceSurfaceFormatsKHR" => get_surface_formats as *mut c_void,
        b"vkGetPhysicalDeviceSurfacePresentModesKHR" => get_surface_present_modes as *mut c_void,
        b"vkGetPhysicalDeviceSurfaceCapabilities2KHR" => get_surface_capabilities2 as *mut c_void,
        b"vkGetPhysicalDeviceSurfaceFormats2KHR" => get_surface_formats2 as *mut c_void,
        b"vkCreateSwapchainKHR" => create_swapchain as *mut c_void,
        b"vkDestroySwapchainKHR" => destroy_swapchain as *mut c_void,
        b"vkGetSwapchainImagesKHR" => get_swapchain_images as *mut c_void,
        b"vkAcquireNextImageKHR" => acquire_next_image as *mut c_void,
        b"vkAcquireNextImage2KHR" => acquire_next_image2 as *mut c_void,
        b"vkQueuePresentKHR" => queue_present as *mut c_void,
        _ => return None,
    };
    Some(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_exposes_only_android_wsi_subset() {
        for name in [
            c"vkCreateAndroidSurfaceKHR",
            c"vkDestroySurfaceKHR",
            c"vkGetPhysicalDeviceSurfaceSupportKHR",
            c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
            c"vkGetPhysicalDeviceSurfaceFormatsKHR",
            c"vkGetPhysicalDeviceSurfacePresentModesKHR",
            c"vkCreateSwapchainKHR",
            c"vkDestroySwapchainKHR",
            c"vkGetSwapchainImagesKHR",
            c"vkAcquireNextImageKHR",
            c"vkAcquireNextImage2KHR",
            c"vkQueuePresentKHR",
        ] {
            assert!(lookup(name).is_some(), "missing {}", name.to_string_lossy());
        }
        assert!(lookup(c"vkCreateMetalSurfaceEXT").is_none());
    }

    #[test]
    fn capability_values_are_three_fifo_images_and_rgba_formats() {
        let caps = surface_caps(VkExtent2D {
            width: 720,
            height: 1280,
        });
        assert_eq!(caps.min_image_count, 3);
        assert_eq!(caps.max_image_count, 3);
        assert_eq!(caps.supported_usage_flags, SUPPORTED_USAGE);
        assert_eq!(VK_FORMAT_R8G8B8A8_UNORM, 37);
        assert_eq!(SURFACE_FORMATS, [37, 43]);
        assert_eq!(VK_COLOR_SPACE_SRGB_NONLINEAR_KHR, 0);
    }
}
