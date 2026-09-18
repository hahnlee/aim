//! Process-lifetime MoltenVK dylib and raw PFN/Metal boundary.
#[cfg(not(test))]
use crate::vulkan_types::VK_INCOMPLETE;
use crate::vulkan_types::{VulkanGetDeviceProcAddr, VulkanGetInstanceProcAddr};
#[cfg(not(test))]
use std::ffi::{c_char, c_int, CString};
use std::ffi::{c_void, CStr};
use std::ptr;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

const VK_SUCCESS: i32 = 0;
const VK_INCOMPATIBLE_DRIVER: i32 = -9;
#[cfg(not(test))]
const RTLD_NOW: c_int = 0x2;
#[cfg(not(test))]
const RTLD_LOCAL: c_int = 0x4;

#[cfg(not(test))]
unsafe extern "C" {
    #[link_name = "dlopen"]
    fn host_dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    #[link_name = "dlsym"]
    fn host_dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

pub(super) struct MoltenVkProvider {
    // The provider is process-lifetime by design. Store the opaque address as
    // an integer so no Darwin loader ownership escapes this closed namespace.
    _handle: usize,
    get_instance_proc_addr: VulkanGetInstanceProcAddr,
}

// Stable prefix of MoltenVK's append-only MVKConfiguration ABI through
// semaphoreSupportStyle. Keeping the prefix local avoids exposing MoltenVK's
// private headers to Android-facing provider code.
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[cfg_attr(test, allow(dead_code))]
pub(super) struct MoltenVkConfigurationPrefix {
    debug_mode: u32,
    shader_conversion_flip_vertex_y: u32,
    synchronous_queue_submits: u32,
    prefill_metal_command_buffers: u32,
    max_active_metal_command_buffers_per_queue: u32,
    support_large_query_pools: u32,
    present_with_command_buffer: u32,
    swapchain_min_mag_filter_use_nearest: u32,
    metal_compile_timeout: u64,
    performance_tracking: u32,
    performance_logging_frame_count: u32,
    display_watermark: u32,
    specialized_queue_families: u32,
    switch_system_gpu: u32,
    full_image_view_swizzle: u32,
    default_gpu_capture_scope_queue_family_index: u32,
    default_gpu_capture_scope_queue_index: u32,
    fast_math_enabled: u32,
    log_level: u32,
    trace_vulkan_calls: u32,
    force_low_power_gpu: u32,
    semaphore_use_mtl_fence: u32,
    semaphore_support_style: u32,
}

static MOLTENVK: OnceLock<Option<MoltenVkProvider>> = OnceLock::new();

#[cfg(test)]
static TEST_DEVICE_B_DESTROYED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
unsafe extern "C" fn test_destroy_device(_device: *mut c_void, _allocator: *const c_void) {
    TEST_DEVICE_B_DESTROYED.store(true, Ordering::Release);
}

#[cfg(test)]
unsafe extern "C" fn test_get_device_proc_addr(
    device: *mut c_void,
    name: *const std::ffi::c_char,
) -> *mut c_void {
    let name = unsafe { CStr::from_ptr(name) };
    if device as usize == 0xb && name == c"vkDestroyDevice" {
        return test_destroy_device as *mut c_void;
    }
    if device as usize == 0xa
        && name == c"vkQueueSubmit"
        && TEST_DEVICE_B_DESTROYED.load(Ordering::Acquire)
    {
        return 0xfeed as *mut c_void;
    }
    ptr::null_mut()
}

#[cfg(not(test))]
fn load_moltenvk() -> Option<MoltenVkProvider> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("DARWIN_ART_MOLTENVK_DYLIB") {
        if path.starts_with('/') {
            candidates.push(path);
        }
    }

    // Android capability discovery must describe the runtime package, not
    // whichever libraries happen to be installed on the host.  In
    // particular, finding Homebrew's MoltenVK must not silently make
    // libvulkan report a physical device and move every Android application
    // away from the supported GLES/ANGLE path.  A packaged runtime may opt in
    // by supplying its checksum-pinned provider explicitly.
    if candidates.is_empty() {
        return None;
    }

    for candidate in candidates {
        let Ok(path) = CString::new(candidate.as_bytes()) else {
            continue;
        };
        // SAFETY: path is NUL-terminated and the returned handle is retained
        // for process lifetime by MOLTENVK.
        let handle = unsafe { host_dlopen(path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            continue;
        }
        // SAFETY: MoltenVK's public dylib exports the Vulkan loader ABI.
        let address = unsafe { host_dlsym(handle, c"vkGetInstanceProcAddr".as_ptr()) };
        if address.is_null() {
            continue;
        }
        // SAFETY: the symbol has the exact Vulkan PFN_vkGetInstanceProcAddr ABI.
        let get_instance_proc_addr: VulkanGetInstanceProcAddr =
            unsafe { std::mem::transmute(address) };
        type GetConfiguration =
            unsafe extern "C" fn(*mut c_void, *mut MoltenVkConfigurationPrefix, *mut usize) -> i32;
        type SetConfiguration = unsafe extern "C" fn(
            *mut c_void,
            *const MoltenVkConfigurationPrefix,
            *mut usize,
        ) -> i32;
        let get_configuration_address =
            unsafe { host_dlsym(handle, c"vkGetMoltenVKConfigurationMVK".as_ptr()) };
        let set_configuration_address =
            unsafe { host_dlsym(handle, c"vkSetMoltenVKConfigurationMVK".as_ptr()) };
        if get_configuration_address.is_null() || set_configuration_address.is_null() {
            continue;
        }
        let get_configuration: GetConfiguration =
            unsafe { std::mem::transmute(get_configuration_address) };
        let set_configuration: SetConfiguration =
            unsafe { std::mem::transmute(set_configuration_address) };
        let mut configuration = MoltenVkConfigurationPrefix::default();
        let mut configuration_size = std::mem::size_of::<MoltenVkConfigurationPrefix>();
        let get_result = unsafe {
            get_configuration(ptr::null_mut(), &mut configuration, &mut configuration_size)
        };
        if get_result != VK_SUCCESS && get_result != VK_INCOMPLETE {
            continue;
        }
        configuration.semaphore_support_style = 2;
        if std::env::var_os("DARWIN_ART_DEBUG_VULKAN_CALLS").is_some() {
            configuration.trace_vulkan_calls = 4;
        }
        configuration_size = std::mem::size_of::<MoltenVkConfigurationPrefix>();
        let set_result =
            unsafe { set_configuration(ptr::null_mut(), &configuration, &mut configuration_size) };
        if set_result != VK_SUCCESS && set_result != VK_INCOMPLETE {
            continue;
        }
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            eprintln!(
                "ART Android Vulkan: pid={} Metal provider={} semaphore-style={}",
                std::process::id(),
                path.to_string_lossy(),
                configuration.semaphore_support_style
            );
        }
        return Some(MoltenVkProvider {
            _handle: handle as usize,
            get_instance_proc_addr,
        });
    }
    None
}

#[cfg(test)]
fn load_moltenvk() -> Option<MoltenVkProvider> {
    None
}

pub(super) fn moltenvk() -> Option<&'static MoltenVkProvider> {
    MOLTENVK.get_or_init(load_moltenvk).as_ref()
}

pub(super) unsafe fn raw_instance_symbol(instance: *mut c_void, name: &CStr) -> *mut c_void {
    let Some(provider) = moltenvk() else {
        return ptr::null_mut();
    };
    unsafe { (provider.get_instance_proc_addr)(instance, name.as_ptr()) }
}

pub(super) unsafe fn provider_exported_symbol(name: &CStr) -> *mut c_void {
    #[cfg(test)]
    if name == c"vkGetDeviceProcAddr" {
        return test_get_device_proc_addr as *mut c_void;
    }
    #[cfg(not(test))]
    if let Some(provider) = moltenvk() {
        // SAFETY: the provider handle is retained for process lifetime by the
        // OnceLock, and name is a caller-owned NUL-terminated symbol.
        return unsafe { host_dlsym(provider._handle as *mut c_void, name.as_ptr()) };
    }
    ptr::null_mut()
}

pub(super) unsafe fn raw_device_symbol(device: usize, name: &CStr) -> *mut c_void {
    let address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
    if address.is_null() {
        return ptr::null_mut();
    }
    let get: VulkanGetDeviceProcAddr = unsafe { std::mem::transmute(address) };
    unsafe { get(device as *mut c_void, name.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_dispatch_uses_explicit_device_after_other_device_destroyed() {
        TEST_DEVICE_B_DESTROYED.store(false, Ordering::Release);
        let destroy_address = unsafe { raw_device_symbol(0xb, c"vkDestroyDevice") };
        assert!(!destroy_address.is_null());
        let destroy: unsafe extern "C" fn(*mut c_void, *const c_void) =
            unsafe { std::mem::transmute(destroy_address) };
        unsafe { destroy(0xb as *mut c_void, ptr::null()) };
        assert!(TEST_DEVICE_B_DESTROYED.load(Ordering::Acquire));

        let queue_address = unsafe { raw_device_symbol(0xa, c"vkQueueSubmit") };
        assert_eq!(queue_address as usize, 0xfeed);
    }
}
