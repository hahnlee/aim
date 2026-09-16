//! A deliberately closed Android DSO namespace for the Darwin ELF loader.
//!
//! This is not a Darwin `dlsym(RTLD_DEFAULT, ...)` adapter. Every visible
//! SONAME and symbol is listed below, including its Android GNU symbol version.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

mod loader_open;
mod vulkan_acquire_fences;
mod vulkan_library;
mod vulkan_wsi;
mod vulkan_wsi_backend;
pub use loader_open::darwin_art_bionic_android_dlopen_ext;
use vulkan_wsi_backend::*;

pub const PROVIDER_LOADER_LIBDL: u32 = 1;
pub const PROVIDER_AOSP_LIBLOG: u32 = 2;

#[repr(C)]
pub struct AndroidDlExtInfo {
    pub flags: u64,
    pub reserved_addr: *mut c_void,
    pub reserved_size: usize,
    pub relro_fd: c_int,
    pub library_fd: c_int,
    pub library_fd_offset: i64,
    pub library_namespace: *mut c_void,
}

pub type OpenCallback = unsafe extern "C" fn(
    context: *mut c_void,
    filename: *const c_char,
    flags: c_int,
    extinfo: *const AndroidDlExtInfo,
    error: *mut c_char,
    error_capacity: usize,
) -> *mut c_void;
pub type LookupCallback = unsafe extern "C" fn(
    context: *mut c_void,
    handle: *mut c_void,
    symbol: *const c_char,
    version: *const c_char,
    error: *mut c_char,
    error_capacity: usize,
) -> *mut c_void;
pub type CloseCallback = unsafe extern "C" fn(
    context: *mut c_void,
    handle: *mut c_void,
    error: *mut c_char,
    error_capacity: usize,
) -> c_int;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LoaderCallbacks {
    pub context: *mut c_void,
    pub open: Option<OpenCallback>,
    pub lookup: Option<LookupCallback>,
    pub close: Option<CloseCallback>,
}

// The embedding loader owns the context lifetime and binds exactly once before
// any app thread can enter libdl. Access is read-only after OnceLock publication.
unsafe impl Send for LoaderCallbacks {}
unsafe impl Sync for LoaderCallbacks {}

static LOADER: OnceLock<LoaderCallbacks> = OnceLock::new();
static mut LIBANDROID_HANDLE_TOKEN: u8 = 0;
static mut LIBNATIVEWINDOW_HANDLE_TOKEN: u8 = 0;
static mut LIBEGL_HANDLE_TOKEN: u8 = 0;
static mut LIBGLESV2_HANDLE_TOKEN: u8 = 0;
static mut LIBVULKAN_HANDLE_TOKEN: u8 = 0;

#[cfg(not(test))]
unsafe extern "C" {
    #[link_name = "darwin_art_android_ANativeWindow_fromSurface"]
    fn host_anative_window_from_surface(env: *mut c_void, surface: *mut c_void) -> *mut c_void;
    #[link_name = "darwin_art_android_ANativeWindow_release"]
    fn host_anative_window_release(window: *mut c_void);
    #[link_name = "darwin_art_android_ANativeWindow_lock"]
    fn host_anative_window_lock(
        window: *mut c_void,
        buffer: *mut c_void,
        bounds: *mut c_void,
    ) -> c_int;
    #[link_name = "darwin_art_android_ANativeWindow_unlockAndPost"]
    fn host_anative_window_unlock_and_post(window: *mut c_void) -> c_int;
    #[link_name = "darwin_art_android_ANativeWindow_setBuffersGeometry"]
    fn host_anative_window_set_buffers_geometry(
        window: *mut c_void,
        width: c_int,
        height: c_int,
        format: c_int,
    ) -> c_int;
    #[link_name = "ASharedMemory_create"]
    fn host_ashared_memory_create(name: *const c_char, size: usize) -> c_int;
    #[link_name = "ASharedMemory_setProt"]
    fn host_ashared_memory_set_prot(fd: c_int, protection: c_int) -> c_int;
    fn darwin_art_android_platform_symbol(symbol: *const c_char) -> *mut c_void;
    fn darwin_art_angle_dso_symbol(soname: *const c_char, symbol: *const c_char) -> *mut c_void;
}

#[cfg(test)]
unsafe fn darwin_art_android_platform_symbol(_symbol: *const c_char) -> *mut c_void {
    ptr::null_mut()
}

fn libandroid_handle() -> *mut c_void {
    ptr::addr_of_mut!(LIBANDROID_HANDLE_TOKEN).cast()
}

fn libnativewindow_handle() -> *mut c_void {
    ptr::addr_of_mut!(LIBNATIVEWINDOW_HANDLE_TOKEN).cast()
}

fn virtual_graphics_handle(name: &[u8]) -> Option<*mut c_void> {
    match name {
        b"libEGL.so" => Some(ptr::addr_of_mut!(LIBEGL_HANDLE_TOKEN).cast()),
        b"libGLESv2.so" => Some(ptr::addr_of_mut!(LIBGLESV2_HANDLE_TOKEN).cast()),
        // Vulkan remains a virtual Android DSO even when MoltenVK is present:
        // callers must never receive a Darwin dlopen handle directly.
        b"libvulkan.so" => Some(ptr::addr_of_mut!(LIBVULKAN_HANDLE_TOKEN).cast()),
        _ => None,
    }
}

// Android Vulkan loader boundary. A bundled MoltenVK dylib is preferred and
// converts the offscreen Vulkan/AHardwareBuffer subset used by Graphite/Dawn
// to Metal on the host GPU. It deliberately does not synthesize
// VK_KHR_android_surface, so APK-facing Android Vulkan WSI stays unavailable.
// If the provider is absent, the closed loader reports Vulkan as unavailable
// instead of inventing a software device or leaking a Darwin loader handle.
const VK_SUCCESS: i32 = 0;
const VK_ERROR_INCOMPATIBLE_DRIVER: i32 = -9;

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

type VulkanGetInstanceProcAddr =
    unsafe extern "C" fn(instance: *mut c_void, name: *const c_char) -> *mut c_void;
type VulkanGetDeviceProcAddr =
    unsafe extern "C" fn(device: *mut c_void, name: *const c_char) -> *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanExtensionProperties {
    extension_name: [c_char; 256],
    spec_version: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanDeviceCreateInfo {
    s_type: i32,
    p_next: *const c_void,
    flags: u32,
    queue_create_info_count: u32,
    queue_create_infos: *const c_void,
    enabled_layer_count: u32,
    enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    enabled_extension_names: *const *const c_char,
    enabled_features: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AHardwareBufferDesc {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

#[repr(C)]
struct VulkanBaseOutStructure {
    s_type: i32,
    p_next: *mut VulkanBaseOutStructure,
}

#[repr(C)]
struct VulkanAndroidHardwareBufferProperties {
    s_type: i32,
    p_next: *mut VulkanBaseOutStructure,
    allocation_size: u64,
    memory_type_bits: u32,
}

#[repr(C)]
struct VulkanAndroidHardwareBufferFormatProperties {
    s_type: i32,
    p_next: *mut VulkanBaseOutStructure,
    format: i32,
    external_format: u64,
    format_features: u32,
    sampler_ycbcr_conversion_components: [i32; 4],
    suggested_ycbcr_model: i32,
    suggested_ycbcr_range: i32,
    suggested_x_chroma_offset: i32,
    suggested_y_chroma_offset: i32,
}

#[repr(C)]
struct VulkanMemoryMetalHandleProperties {
    s_type: i32,
    p_next: *mut c_void,
    memory_type_bits: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanPhysicalDeviceImageFormatInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    format: i32,
    image_type: i32,
    tiling: i32,
    usage: u32,
    flags: u32,
}

#[repr(C)]
struct VulkanBaseInStructure {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
}

#[repr(C)]
struct VulkanPhysicalDeviceExternalImageFormatInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    handle_type: u32,
}

#[repr(C)]
struct VulkanExternalImageFormatProperties {
    s_type: i32,
    p_next: *mut VulkanBaseOutStructure,
    external_memory_features: u32,
    export_from_imported_handle_types: u32,
    compatible_handle_types: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanImageCreateInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    flags: u32,
    image_type: i32,
    format: i32,
    extent: [u32; 3],
    mip_levels: u32,
    array_layers: u32,
    samples: u32,
    tiling: i32,
    usage: u32,
    sharing_mode: i32,
    queue_family_index_count: u32,
    queue_family_indices: *const u32,
    initial_layout: i32,
}

#[repr(C)]
struct VulkanExternalMemoryImageCreateInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    handle_types: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanMemoryAllocateInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    allocation_size: u64,
    memory_type_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanMemoryDedicatedAllocateInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    image: *mut c_void,
    buffer: *mut c_void,
}

#[repr(C)]
struct VulkanImportAndroidHardwareBufferInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    buffer: *mut c_void,
}

#[repr(C)]
struct VulkanImportMemoryMetalHandleInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    handle_type: u32,
    handle: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanBindImageMemoryInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    image: *mut c_void,
    memory: *mut c_void,
    memory_offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanSemaphoreCreateInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    flags: u32,
}

#[repr(C)]
struct VulkanImportMetalSharedEventInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    metal_shared_event: *mut c_void,
}

#[repr(C)]
struct VulkanPhysicalDeviceExternalSemaphoreInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    handle_type: u32,
}

#[repr(C)]
struct VulkanExternalSemaphoreProperties {
    s_type: i32,
    p_next: *mut VulkanBaseOutStructure,
    export_from_imported_handle_types: u32,
    compatible_handle_types: u32,
    external_semaphore_features: u32,
}

#[repr(C)]
struct VulkanImportSemaphoreFdInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    semaphore: *mut c_void,
    flags: u32,
    handle_type: u32,
    fd: c_int,
}

#[repr(C)]
struct VulkanSemaphoreGetFdInfo {
    s_type: i32,
    p_next: *const VulkanBaseInStructure,
    semaphore: *mut c_void,
    handle_type: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VulkanSemaphoreState {
    metal_shared_event: usize,
    next_value: u64,
}

#[derive(Clone, Copy)]
struct ImportedAndroidMemory {
    metal_texture: usize,
    hardware_buffer: usize,
}

struct MoltenVkProvider {
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
struct MoltenVkConfigurationPrefix {
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
static VULKAN_INSTANCE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static VULKAN_PHYSICAL_DEVICE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static VULKAN_METAL_DEVICE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static VULKAN_DEVICE_SYNC_FD: OnceLock<Mutex<HashMap<usize, bool>>> = OnceLock::new();
static IMPORTED_ANDROID_MEMORY: OnceLock<Mutex<HashMap<usize, ImportedAndroidMemory>>> =
    OnceLock::new();
static VULKAN_SEMAPHORES: OnceLock<Mutex<HashMap<usize, VulkanSemaphoreState>>> = OnceLock::new();
static VULKAN_IMAGE_FORMATS: OnceLock<Mutex<HashMap<usize, i32>>> = OnceLock::new();

const VK_INCOMPLETE: i32 = 5;
const VK_ERROR_EXTENSION_NOT_PRESENT: i32 = -7;
const VK_ERROR_FORMAT_NOT_SUPPORTED: i32 = -11;
const VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID: i32 = 1_000_129_002;
const VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID: i32 = 1_000_129_003;
const VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO: i32 = 1_000_127_001;
const VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO: i32 = 1_000_071_000;
const VK_STRUCTURE_TYPE_EXTERNAL_IMAGE_FORMAT_PROPERTIES: i32 = 1_000_071_001;
const VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO: i32 = 1_000_072_001;
const VK_STRUCTURE_TYPE_IMPORT_MEMORY_METAL_HANDLE_INFO_EXT: i32 = 1_000_602_000;
const VK_STRUCTURE_TYPE_MEMORY_METAL_HANDLE_PROPERTIES_EXT: i32 = 1_000_602_001;
const VK_STRUCTURE_TYPE_IMPORT_METAL_SHARED_EVENT_INFO_EXT: i32 = 1_000_311_011;
const VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID: u32 = 0x0000_0400;
const VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT: u32 = 0x0002_0000;
const VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT: u32 = 0x0000_0004;
const VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT: u32 = 0x0000_0010;
const VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT: u32 = 0x0000_0001;
const VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT: u32 = 0x0000_0002;
const VK_FORMAT_R8G8B8A8_UNORM: i32 = 37;
const VK_FORMAT_FEATURE_RGBA_RENDERABLE: u32 = 0x0000_c083;
const ANDROID_AHB_EXTENSION: &CStr = c"VK_ANDROID_external_memory_android_hardware_buffer";
const QUEUE_FAMILY_FOREIGN_EXTENSION: &CStr = c"VK_EXT_queue_family_foreign";
const METAL_EXTERNAL_MEMORY_EXTENSION: &CStr = c"VK_EXT_external_memory_metal";
const EXTERNAL_SEMAPHORE_FD_EXTENSION: &CStr = c"VK_KHR_external_semaphore_fd";
const METAL_OBJECTS_EXTENSION: &CStr = c"VK_EXT_metal_objects";
const HOST_METAL_SURFACE_EXTENSION: &CStr = c"VK_EXT_metal_surface";
const HOST_MACOS_SURFACE_EXTENSION: &CStr = c"VK_MVK_macos_surface";
const ANDROID_SURFACE_EXTENSION: &CStr = c"VK_KHR_android_surface";

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
        configuration_size = std::mem::size_of::<MoltenVkConfigurationPrefix>();
        let set_result =
            unsafe { set_configuration(ptr::null_mut(), &configuration, &mut configuration_size) };
        if set_result != VK_SUCCESS && set_result != VK_INCOMPLETE {
            continue;
        }
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            eprintln!(
                "ART Android Vulkan: Metal provider={} semaphore-style={}",
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

fn moltenvk() -> Option<&'static MoltenVkProvider> {
    MOLTENVK.get_or_init(load_moltenvk).as_ref()
}

unsafe fn moltenvk_instance_symbol(name: &CStr) -> *mut c_void {
    let Some(provider) = moltenvk() else {
        return ptr::null_mut();
    };
    let instance = VULKAN_INSTANCE.load(Ordering::Acquire);
    unsafe { (provider.get_instance_proc_addr)(instance, name.as_ptr()) }
}

unsafe fn moltenvk_exported_symbol(name: &CStr) -> *mut c_void {
    let address = unsafe { moltenvk_instance_symbol(name) };
    if !address.is_null() {
        return address;
    }
    #[cfg(not(test))]
    if let Some(provider) = moltenvk() {
        return unsafe { host_dlsym(provider._handle as *mut c_void, name.as_ptr()) };
    }
    ptr::null_mut()
}

unsafe fn moltenvk_metal_device() -> *mut c_void {
    let cached = VULKAN_METAL_DEVICE.load(Ordering::Acquire);
    if !cached.is_null() {
        return cached;
    }
    let physical_device = VULKAN_PHYSICAL_DEVICE.load(Ordering::Acquire);
    let Some(provider) = moltenvk() else {
        return ptr::null_mut();
    };
    if physical_device.is_null() {
        return ptr::null_mut();
    }
    type GetMetalDevice = unsafe extern "C" fn(*mut c_void, *mut *mut c_void);
    let address = unsafe { moltenvk_instance_symbol(c"vkGetMTLDeviceMVK") };
    #[cfg(not(test))]
    let address = if address.is_null() {
        unsafe {
            host_dlsym(
                provider._handle as *mut c_void,
                c"vkGetMTLDeviceMVK".as_ptr(),
            )
        }
    } else {
        address
    };
    #[cfg(test)]
    let _ = provider;
    if address.is_null() {
        return ptr::null_mut();
    }
    let get_metal_device: GetMetalDevice = unsafe { std::mem::transmute(address) };
    let mut metal_device = ptr::null_mut();
    unsafe { get_metal_device(physical_device, &mut metal_device) };
    if !metal_device.is_null() {
        VULKAN_METAL_DEVICE.store(metal_device, Ordering::Release);
    }
    metal_device
}

unsafe extern "C" fn moltenvk_create_instance(
    create_info: *const c_void,
    allocator: *const c_void,
    instance: *mut *mut c_void,
) -> i32 {
    let Some(provider) = moltenvk() else {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    };
    let address =
        unsafe { (provider.get_instance_proc_addr)(ptr::null_mut(), c"vkCreateInstance".as_ptr()) };
    if address.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let create: unsafe extern "C" fn(*const c_void, *const c_void, *mut *mut c_void) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InstanceInfo {
        s_type: i32,
        p_next: *const c_void,
        flags: u32,
        application: *const c_void,
        layer_count: u32,
        layers: *const *const c_char,
        extension_count: u32,
        extensions: *const *const c_char,
    }
    if create_info.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let mut info = unsafe { *(create_info as *const InstanceInfo) };
    let names = if info.extension_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(info.extensions, info.extension_count as usize) }
    };
    let translated: Vec<_> = names
        .iter()
        .copied()
        .filter(|name| {
            !name.is_null() && unsafe { CStr::from_ptr(*name) } != ANDROID_SURFACE_EXTENSION
        })
        .collect();
    info.extension_count = translated.len() as u32;
    info.extensions = translated.as_ptr();
    let result = unsafe { create((&info as *const InstanceInfo).cast(), allocator, instance) };
    if result == VK_SUCCESS && !instance.is_null() {
        let value = unsafe { *instance };
        VULKAN_INSTANCE.store(value, Ordering::Release);
    }
    result
}

fn is_host_surface_extension(property: &VulkanExtensionProperties) -> bool {
    // SAFETY: VkExtensionProperties guarantees a NUL-terminated name.
    let name = unsafe { CStr::from_ptr(property.extension_name.as_ptr()) };
    name == HOST_METAL_SURFACE_EXTENSION || name == HOST_MACOS_SURFACE_EXTENSION
}

unsafe extern "C" fn moltenvk_enumerate_instance_extensions(
    layer_name: *const c_char,
    property_count: *mut u32,
    properties: *mut VulkanExtensionProperties,
) -> i32 {
    type Enumerate =
        unsafe extern "C" fn(*const c_char, *mut u32, *mut VulkanExtensionProperties) -> i32;
    let Some(provider) = moltenvk() else {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    };
    if property_count.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let address = unsafe {
        (provider.get_instance_proc_addr)(
            ptr::null_mut(),
            c"vkEnumerateInstanceExtensionProperties".as_ptr(),
        )
    };
    if address.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let enumerate: Enumerate = unsafe { std::mem::transmute(address) };
    let mut native_count = 0;
    let result = unsafe { enumerate(layer_name, &mut native_count, ptr::null_mut()) };
    if result != VK_SUCCESS {
        return result;
    }
    let empty = VulkanExtensionProperties {
        extension_name: [0; 256],
        spec_version: 0,
    };
    let mut native = vec![empty; native_count as usize];
    let result = unsafe { enumerate(layer_name, &mut native_count, native.as_mut_ptr()) };
    if result != VK_SUCCESS && result != VK_INCOMPLETE {
        return result;
    }
    native.truncate(native_count as usize);
    let native_count = native.len();
    let advertised_android_surface = native.iter().any(|property| {
        // SAFETY: VkExtensionProperties guarantees a NUL-terminated name.
        (unsafe { CStr::from_ptr(property.extension_name.as_ptr()) }) == ANDROID_SURFACE_EXTENSION
    });
    native.retain(|property| !is_host_surface_extension(property));
    if layer_name.is_null() && !advertised_android_surface {
        let mut android = empty;
        android.spec_version = 6;
        for (dst, src) in android
            .extension_name
            .iter_mut()
            .zip(ANDROID_SURFACE_EXTENSION.to_bytes_with_nul())
        {
            *dst = *src as c_char;
        }
        native.push(android);
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: instance-extensions native={} returned={} host-wsi=0 android-wsi={}",
            native_count,
            native.len(),
            advertised_android_surface as u8
        );
    }

    if properties.is_null() {
        unsafe { *property_count = native.len() as u32 };
        return VK_SUCCESS;
    }
    let capacity = unsafe { *property_count } as usize;
    let written = capacity.min(native.len());
    unsafe {
        ptr::copy_nonoverlapping(native.as_ptr(), properties, written);
        *property_count = written as u32;
    }
    if written < native.len() {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

unsafe extern "C" fn moltenvk_enumerate_device_extensions(
    physical_device: *mut c_void,
    layer_name: *const c_char,
    property_count: *mut u32,
    properties: *mut VulkanExtensionProperties,
) -> i32 {
    type Enumerate = unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *mut u32,
        *mut VulkanExtensionProperties,
    ) -> i32;
    let address = unsafe { moltenvk_instance_symbol(c"vkEnumerateDeviceExtensionProperties") };
    if address.is_null() || property_count.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let enumerate: Enumerate = unsafe { std::mem::transmute(address) };
    if !layer_name.is_null() {
        return unsafe { enumerate(physical_device, layer_name, property_count, properties) };
    }

    let mut native_count = 0;
    let result = unsafe {
        enumerate(
            physical_device,
            layer_name,
            &mut native_count,
            ptr::null_mut(),
        )
    };
    if result != VK_SUCCESS {
        return result;
    }
    let mut native = vec![
        VulkanExtensionProperties {
            extension_name: [0; 256],
            spec_version: 0,
        };
        native_count as usize
    ];
    let result = unsafe {
        enumerate(
            physical_device,
            layer_name,
            &mut native_count,
            native.as_mut_ptr(),
        )
    };
    if result != VK_SUCCESS && result != VK_INCOMPLETE {
        return result;
    }
    native.truncate(native_count as usize);
    // Android WSI implements the base swapchain contract; extra presentation
    // extensions cannot be forwarded with our native-window surface handles.
    native.retain(|property| {
        let name = unsafe { CStr::from_ptr(property.extension_name.as_ptr()) };
        android_device_extension_supported(name)
    });
    let contains = |name: &CStr| {
        native.iter().any(|property| {
            // SAFETY: VkExtensionProperties guarantees a NUL-terminated name.
            (unsafe { CStr::from_ptr(property.extension_name.as_ptr()) }) == name
        })
    };
    let missing = [
        ANDROID_AHB_EXTENSION,
        QUEUE_FAMILY_FOREIGN_EXTENSION,
        EXTERNAL_SEMAPHORE_FD_EXTENSION,
        c"VK_KHR_swapchain",
    ]
    .into_iter()
    .filter(|name| !contains(name))
    .collect::<Vec<_>>();
    for name in missing.iter() {
        let mut extension = VulkanExtensionProperties {
            extension_name: [0; 256],
            spec_version: if *name == ANDROID_AHB_EXTENSION { 5 } else { 1 },
        };
        for (destination, source) in extension
            .extension_name
            .iter_mut()
            .zip(name.to_bytes_with_nul())
        {
            *destination = *source as c_char;
        }
        native.push(extension);
    }
    if properties.is_null() {
        unsafe { *property_count = native.len() as u32 };
        return VK_SUCCESS;
    }
    let written = (unsafe { *property_count } as usize).min(native.len());
    unsafe {
        ptr::copy_nonoverlapping(native.as_ptr(), properties, written);
        *property_count = written as u32;
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: device-extensions native={} returned={} android-ahb=1 queue-family-foreign=1 sync-fd=1 android-swapchain=1",
            native_count, written
        );
    }
    if written < native.len() {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
}

fn android_device_extension_supported(name: &CStr) -> bool {
    !matches!(
        name.to_bytes(),
        b"VK_KHR_incremental_present"
            | b"VK_KHR_swapchain_maintenance1"
            | b"VK_KHR_swapchain_mutable_format"
            | b"VK_EXT_swapchain_maintenance1"
            | b"VK_KHR_present_id"
            | b"VK_KHR_present_id2"
            | b"VK_KHR_present_wait"
            | b"VK_KHR_present_wait2"
            | b"VK_EXT_hdr_metadata"
            | b"VK_GOOGLE_display_timing"
    )
}

unsafe extern "C" fn moltenvk_create_device(
    physical_device: *mut c_void,
    create_info: *const VulkanDeviceCreateInfo,
    allocator: *const c_void,
    device: *mut *mut c_void,
) -> i32 {
    type Create = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanDeviceCreateInfo,
        *const c_void,
        *mut *mut c_void,
    ) -> i32;
    let address = unsafe { moltenvk_instance_symbol(c"vkCreateDevice") };
    if address.is_null() || create_info.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let create: Create = unsafe { std::mem::transmute(address) };
    let original = unsafe { &*create_info };
    let original_names = if original.enabled_extension_names.is_null() {
        &[][..]
    } else {
        unsafe {
            std::slice::from_raw_parts(
                original.enabled_extension_names,
                original.enabled_extension_count as usize,
            )
        }
    };
    let mut translated = Vec::with_capacity(original_names.len() + 2);
    let mut requested_android_ahb = false;
    let mut requested_queue_family_foreign = false;
    let mut requested_sync_fd = false;
    let mut has_metal_external_memory = false;
    let mut has_metal_objects = false;
    for &name in original_names {
        if name.is_null() {
            continue;
        }
        let value = unsafe { CStr::from_ptr(name) };
        if value == c"VK_KHR_swapchain" {
            requested_android_ahb = true;
            requested_sync_fd = true;
            continue;
        }
        if !android_device_extension_supported(value) {
            if !device.is_null() {
                unsafe { *device = ptr::null_mut() };
            }
            return VK_ERROR_EXTENSION_NOT_PRESENT;
        }
        if value == ANDROID_AHB_EXTENSION {
            requested_android_ahb = true;
            continue;
        }
        if value == QUEUE_FAMILY_FOREIGN_EXTENSION {
            requested_queue_family_foreign = true;
            continue;
        }
        if value == EXTERNAL_SEMAPHORE_FD_EXTENSION {
            requested_sync_fd = true;
            continue;
        }
        if value == METAL_EXTERNAL_MEMORY_EXTENSION {
            has_metal_external_memory = true;
        }
        if value == METAL_OBJECTS_EXTENSION {
            has_metal_objects = true;
        }
        translated.push(name);
    }
    if requested_android_ahb && !has_metal_external_memory {
        translated.push(METAL_EXTERNAL_MEMORY_EXTENSION.as_ptr());
    }
    if requested_sync_fd && !has_metal_objects {
        translated.push(METAL_OBJECTS_EXTENSION.as_ptr());
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: create-device android-ahb={} queue-family-foreign={} sync-fd={} metal-external-memory={} metal-objects={} extensions={}",
            requested_android_ahb,
            requested_queue_family_foreign,
            requested_sync_fd,
            requested_android_ahb || has_metal_external_memory,
            requested_sync_fd || has_metal_objects,
            translated.len()
        );
    }
    let mut translated_info = *original;
    translated_info.enabled_extension_count = translated.len() as u32;
    translated_info.enabled_extension_names = translated.as_ptr();
    let result = unsafe { create(physical_device, &translated_info, allocator, device) };
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: create-device result={} device={:p}",
            result,
            if device.is_null() {
                ptr::null_mut()
            } else {
                unsafe { *device }
            }
        );
    }
    if result == VK_SUCCESS && !device.is_null() && !unsafe { *device }.is_null() {
        VULKAN_PHYSICAL_DEVICE.store(physical_device, Ordering::Release);
        VULKAN_DEVICE_SYNC_FD
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .insert(unsafe { *device } as usize, requested_sync_fd);
    }
    result
}

unsafe extern "C" fn moltenvk_get_android_hardware_buffer_properties(
    device: *mut c_void,
    buffer: *const c_void,
    properties: *mut VulkanAndroidHardwareBufferProperties,
) -> i32 {
    if device.is_null() || buffer.is_null() || properties.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let physical_device = VULKAN_PHYSICAL_DEVICE.load(Ordering::Acquire);
    let Some(provider) = moltenvk() else {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    };
    #[cfg(test)]
    let _ = provider;
    if physical_device.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }

    type DescribeHardwareBuffer = unsafe extern "C" fn(*const c_void, *mut AHardwareBufferDesc);
    type CreateMetalTexture = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type ReleaseMetalTexture = unsafe extern "C" fn(*mut c_void);
    let describe_address =
        unsafe { darwin_art_android_platform_symbol(c"AHardwareBuffer_describe".as_ptr()) };
    let create_texture_address = unsafe {
        darwin_art_android_platform_symbol(
            c"darwin_art_android_hardware_buffer_vulkan_metal_texture".as_ptr(),
        )
    };
    let release_texture_address = unsafe {
        darwin_art_android_platform_symbol(c"darwin_art_android_metal_texture_release".as_ptr())
    };
    if describe_address.is_null()
        || create_texture_address.is_null()
        || release_texture_address.is_null()
    {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let describe: DescribeHardwareBuffer = unsafe { std::mem::transmute(describe_address) };
    let create_texture: CreateMetalTexture = unsafe { std::mem::transmute(create_texture_address) };
    let release_texture: ReleaseMetalTexture =
        unsafe { std::mem::transmute(release_texture_address) };

    type GetMetalDevice = unsafe extern "C" fn(*mut c_void, *mut *mut c_void);
    let get_metal_device_address = unsafe { moltenvk_instance_symbol(c"vkGetMTLDeviceMVK") };
    #[cfg(not(test))]
    let get_metal_device_address = if get_metal_device_address.is_null() {
        unsafe {
            host_dlsym(
                provider._handle as *mut c_void,
                c"vkGetMTLDeviceMVK".as_ptr(),
            )
        }
    } else {
        get_metal_device_address
    };
    if get_metal_device_address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_metal_device: GetMetalDevice = unsafe { std::mem::transmute(get_metal_device_address) };
    let mut metal_device = ptr::null_mut();
    unsafe { get_metal_device(physical_device, &mut metal_device) };
    if metal_device.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    VULKAN_METAL_DEVICE.store(metal_device, Ordering::Release);

    let metal_texture = unsafe { create_texture(buffer.cast_mut(), metal_device) };
    if metal_texture.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if get_device_proc_address.is_null() {
        unsafe { release_texture(metal_texture) };
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc: VulkanGetDeviceProcAddr =
        unsafe { std::mem::transmute(get_device_proc_address) };
    let get_properties_address =
        unsafe { get_device_proc(device, c"vkGetMemoryMetalHandlePropertiesEXT".as_ptr()) };
    if get_properties_address.is_null() {
        unsafe { release_texture(metal_texture) };
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    type GetMetalProperties = unsafe extern "C" fn(
        *mut c_void,
        u32,
        *const c_void,
        *mut VulkanMemoryMetalHandleProperties,
    ) -> i32;
    let get_properties: GetMetalProperties = unsafe { std::mem::transmute(get_properties_address) };
    let mut metal_properties = VulkanMemoryMetalHandleProperties {
        s_type: VK_STRUCTURE_TYPE_MEMORY_METAL_HANDLE_PROPERTIES_EXT,
        p_next: ptr::null_mut(),
        memory_type_bits: 0,
    };
    let result = unsafe {
        get_properties(
            device,
            VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT,
            metal_texture,
            &mut metal_properties,
        )
    };
    unsafe { release_texture(metal_texture) };
    if result != VK_SUCCESS || metal_properties.memory_type_bits == 0 {
        return if result == VK_SUCCESS {
            VK_ERROR_FORMAT_NOT_SUPPORTED
        } else {
            result
        };
    }

    let mut description = AHardwareBufferDesc::default();
    unsafe { describe(buffer, &mut description) };
    let format = match description.format {
        1 | 2 => VK_FORMAT_R8G8B8A8_UNORM,
        _ => return VK_ERROR_FORMAT_NOT_SUPPORTED,
    };
    unsafe {
        (*properties).allocation_size = u64::from(description.stride.max(description.width))
            .saturating_mul(u64::from(description.height))
            .saturating_mul(u64::from(description.layers.max(1)))
            .saturating_mul(4);
        (*properties).memory_type_bits = metal_properties.memory_type_bits;
        let mut next = (*properties).p_next;
        while !next.is_null() {
            if (*next).s_type == VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID
            {
                let format_properties = next.cast::<VulkanAndroidHardwareBufferFormatProperties>();
                (*format_properties).format = format;
                (*format_properties).external_format = 0;
                (*format_properties).format_features = VK_FORMAT_FEATURE_RGBA_RENDERABLE;
                (*format_properties).sampler_ycbcr_conversion_components = [0; 4];
                (*format_properties).suggested_ycbcr_model = 0;
                (*format_properties).suggested_ycbcr_range = 1;
                (*format_properties).suggested_x_chroma_offset = 0;
                (*format_properties).suggested_y_chroma_offset = 0;
            }
            next = (*next).p_next;
        }
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: AHardwareBuffer properties {}x{} stride={} allocation={} memory-types={:#x}",
            description.width,
            description.height,
            description.stride,
            unsafe { (*properties).allocation_size },
            metal_properties.memory_type_bits
        );
    }
    VK_SUCCESS
}

unsafe extern "C" fn moltenvk_get_physical_device_image_format_properties(
    physical_device: *mut c_void,
    image_format_info: *const VulkanPhysicalDeviceImageFormatInfo,
    image_format_properties: *mut VulkanBaseOutStructure,
) -> i32 {
    type GetProperties = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanPhysicalDeviceImageFormatInfo,
        *mut VulkanBaseOutStructure,
    ) -> i32;
    let address = unsafe { moltenvk_instance_symbol(c"vkGetPhysicalDeviceImageFormatProperties2") };
    if address.is_null() || image_format_info.is_null() || image_format_properties.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_properties: GetProperties = unsafe { std::mem::transmute(address) };
    let mut external_input = unsafe { (*image_format_info).p_next };
    let mut translated_input = ptr::null_mut();
    let mut original_handle_type = 0;
    while !external_input.is_null() {
        if unsafe { (*external_input).s_type }
            == VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO
        {
            let candidate = external_input
                .cast::<VulkanPhysicalDeviceExternalImageFormatInfo>()
                .cast_mut();
            if unsafe { (*candidate).handle_type }
                == VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID
            {
                original_handle_type = unsafe { (*candidate).handle_type };
                unsafe {
                    (*candidate).handle_type = VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT
                };
                translated_input = candidate;
            }
            break;
        }
        external_input = unsafe { (*external_input).p_next };
    }
    let result =
        unsafe { get_properties(physical_device, image_format_info, image_format_properties) };
    if !translated_input.is_null() {
        unsafe { (*translated_input).handle_type = original_handle_type };
    }
    if result != VK_SUCCESS {
        return result;
    }
    let mut output = unsafe { (*image_format_properties).p_next };
    while !output.is_null() {
        if unsafe { (*output).s_type } == VK_STRUCTURE_TYPE_EXTERNAL_IMAGE_FORMAT_PROPERTIES {
            let external = output.cast::<VulkanExternalImageFormatProperties>();
            unsafe {
                (*external).external_memory_features |= VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT;
                (*external).compatible_handle_types =
                    VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID;
                (*external).export_from_imported_handle_types = 0;
            }
            break;
        }
        output = unsafe { (*output).p_next };
    }
    VK_SUCCESS
}

unsafe extern "C" fn moltenvk_create_image(
    device: *mut c_void,
    create_info: *const VulkanImageCreateInfo,
    allocator: *const c_void,
    image: *mut *mut c_void,
) -> i32 {
    type CreateImage = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanImageCreateInfo,
        *const c_void,
        *mut *mut c_void,
    ) -> i32;
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if get_device_proc_address.is_null() || create_info.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc: VulkanGetDeviceProcAddr =
        unsafe { std::mem::transmute(get_device_proc_address) };
    let address = unsafe { get_device_proc(device, c"vkCreateImage".as_ptr()) };
    if address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let create: CreateImage = unsafe { std::mem::transmute(address) };
    let mut next = unsafe { (*create_info).p_next };
    let mut translated = ptr::null_mut();
    let mut original_handle_types = 0;
    while !next.is_null() {
        if unsafe { (*next).s_type } == VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO {
            let external = next
                .cast::<VulkanExternalMemoryImageCreateInfo>()
                .cast_mut();
            let handle_types = unsafe { (*external).handle_types };
            if handle_types & VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID
                != 0
            {
                original_handle_types = handle_types;
                unsafe {
                    (*external).handle_types = (handle_types
                        & !VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID)
                        | VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT;
                }
                translated = external;
            }
            break;
        }
        next = unsafe { (*next).p_next };
    }
    let result = unsafe { create(device, create_info, allocator, image) };
    if result == VK_SUCCESS && !image.is_null() {
        VULKAN_IMAGE_FORMATS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .insert(unsafe { *image } as usize, unsafe { (*create_info).format });
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!("ART Android Vulkan: create-image result={result} image={:p} format={} size={:?} usage={:#x}", if image.is_null() { ptr::null_mut() } else { unsafe { *image } }, unsafe { (*create_info).format }, unsafe { (*create_info).extent }, unsafe { (*create_info).usage });
    }
    if !translated.is_null() {
        unsafe { (*translated).handle_types = original_handle_types };
    }
    result
}

unsafe extern "C" fn moltenvk_allocate_memory(
    device: *mut c_void,
    allocate_info: *const VulkanMemoryAllocateInfo,
    allocator: *const c_void,
    memory: *mut *mut c_void,
) -> i32 {
    type AllocateMemory = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanMemoryAllocateInfo,
        *const c_void,
        *mut *mut c_void,
    ) -> i32;
    type CreateMetalTexture = unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> *mut c_void;
    type ReleaseMetalTexture = unsafe extern "C" fn(*mut c_void);
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if get_device_proc_address.is_null() || allocate_info.is_null() || memory.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc: VulkanGetDeviceProcAddr =
        unsafe { std::mem::transmute(get_device_proc_address) };
    let address = unsafe { get_device_proc(device, c"vkAllocateMemory".as_ptr()) };
    if address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let allocate: AllocateMemory = unsafe { std::mem::transmute(address) };
    let mut next = unsafe { (*allocate_info).p_next };
    let mut dedicated = None;
    let mut hardware_buffer = ptr::null_mut();
    while !next.is_null() {
        match unsafe { (*next).s_type } {
            VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO => {
                dedicated = Some(unsafe { *next.cast::<VulkanMemoryDedicatedAllocateInfo>() });
            }
            VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID => {
                hardware_buffer =
                    unsafe { (*next.cast::<VulkanImportAndroidHardwareBufferInfo>()).buffer };
            }
            _ => {}
        }
        next = unsafe { (*next).p_next };
    }
    if hardware_buffer.is_null() {
        return unsafe { allocate(device, allocate_info, allocator, memory) };
    }
    let metal_device = VULKAN_METAL_DEVICE.load(Ordering::Acquire);
    let create_texture_address = unsafe {
        darwin_art_android_platform_symbol(
            c"darwin_art_android_hardware_buffer_vulkan_metal_texture_for_format".as_ptr(),
        )
    };
    let release_texture_address = unsafe {
        darwin_art_android_platform_symbol(c"darwin_art_android_metal_texture_release".as_ptr())
    };
    if metal_device.is_null()
        || create_texture_address.is_null()
        || release_texture_address.is_null()
    {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let create_texture: CreateMetalTexture = unsafe { std::mem::transmute(create_texture_address) };
    let release_texture: ReleaseMetalTexture =
        unsafe { std::mem::transmute(release_texture_address) };
    let image_format = dedicated
        .and_then(|info| {
            VULKAN_IMAGE_FORMATS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap()
                .get(&(info.image as usize))
                .copied()
        })
        .unwrap_or(37);
    let metal_texture = unsafe { create_texture(hardware_buffer, metal_device, image_format) };
    if metal_texture.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let metal_import = VulkanImportMemoryMetalHandleInfo {
        s_type: VK_STRUCTURE_TYPE_IMPORT_MEMORY_METAL_HANDLE_INFO_EXT,
        p_next: ptr::null(),
        handle_type: VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT,
        handle: metal_texture,
    };
    let mut dedicated_copy = dedicated.unwrap_or(VulkanMemoryDedicatedAllocateInfo {
        s_type: VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
        p_next: ptr::null(),
        image: ptr::null_mut(),
        buffer: ptr::null_mut(),
    });
    dedicated_copy.p_next =
        (&metal_import as *const VulkanImportMemoryMetalHandleInfo).cast::<VulkanBaseInStructure>();
    let mut translated_info = unsafe { *allocate_info };
    translated_info.p_next = (&dedicated_copy as *const VulkanMemoryDedicatedAllocateInfo)
        .cast::<VulkanBaseInStructure>();
    let result = unsafe { allocate(device, &translated_info, allocator, memory) };
    if result == VK_SUCCESS && !unsafe { *memory }.is_null() {
        // MoltenVK accepts VK_EXT_external_memory_metal on VkDeviceMemory, but
        // its dedicated-allocation initialization can materialize a different
        // image texture before vkBindImageMemory associates the image with that
        // memory. Install the imported IOSurface texture on the dedicated image
        // explicitly so all subsequent Vulkan rendering targets the Android
        // AHardwareBuffer rather than MoltenVK's private texture.
        type SetMetalTexture = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;
        let set_texture_address = unsafe { moltenvk_exported_symbol(c"vkSetMTLTextureMVK") };
        if dedicated_copy.image.is_null() || set_texture_address.is_null() {
            type FreeMemory = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void);
            let free_address = unsafe { get_device_proc(device, c"vkFreeMemory".as_ptr()) };
            if !free_address.is_null() {
                let free: FreeMemory = unsafe { std::mem::transmute(free_address) };
                unsafe { free(device, *memory, allocator) };
            }
            unsafe { *memory = ptr::null_mut() };
            unsafe { release_texture(metal_texture) };
            return VK_ERROR_FORMAT_NOT_SUPPORTED;
        }
        let set_texture: SetMetalTexture = unsafe { std::mem::transmute(set_texture_address) };
        let set_result = unsafe { set_texture(dedicated_copy.image, metal_texture) };
        if set_result != VK_SUCCESS {
            type FreeMemory = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void);
            let free_address = unsafe { get_device_proc(device, c"vkFreeMemory".as_ptr()) };
            if !free_address.is_null() {
                let free: FreeMemory = unsafe { std::mem::transmute(free_address) };
                unsafe { free(device, *memory, allocator) };
            }
            unsafe { *memory = ptr::null_mut() };
            unsafe { release_texture(metal_texture) };
            return set_result;
        }
        let imports = IMPORTED_ANDROID_MEMORY.get_or_init(|| Mutex::new(HashMap::new()));
        let acquire_address =
            unsafe { darwin_art_android_platform_symbol(c"AHardwareBuffer_acquire".as_ptr()) };
        if !acquire_address.is_null() {
            let acquire: unsafe extern "C" fn(*mut c_void) =
                unsafe { std::mem::transmute(acquire_address) };
            unsafe { acquire(hardware_buffer) };
        }
        imports
            .lock()
            .expect("Vulkan import registry poisoned")
            .insert(
                unsafe { *memory } as usize,
                ImportedAndroidMemory {
                    metal_texture: metal_texture as usize,
                    hardware_buffer: hardware_buffer as usize,
                },
            );
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            type GetMetalTexture = unsafe extern "C" fn(*mut c_void, *mut *mut c_void);
            let get_texture_address = unsafe { moltenvk_exported_symbol(c"vkGetMTLTextureMVK") };
            let mut bound_texture = ptr::null_mut();
            if !get_texture_address.is_null() && !dedicated_copy.image.is_null() {
                let get_texture: GetMetalTexture =
                    unsafe { std::mem::transmute(get_texture_address) };
                unsafe { get_texture(dedicated_copy.image, &mut bound_texture) };
            }
            eprintln!(
                "ART Android Vulkan: imported AHardwareBuffer={hardware_buffer:p} MetalTexture={metal_texture:p} memory={:p} dedicated-image={:p} image-texture={bound_texture:p} match={}",
                unsafe { *memory },
                dedicated_copy.image,
                metal_texture == bound_texture
            );
        }
    } else {
        unsafe { release_texture(metal_texture) };
    }
    result
}

unsafe extern "C" fn moltenvk_free_memory(
    device: *mut c_void,
    memory: *mut c_void,
    allocator: *const c_void,
) {
    type FreeMemory = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void);
    type ReleaseObject = unsafe extern "C" fn(*mut c_void);
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if !get_device_proc_address.is_null() {
        let get_device_proc: VulkanGetDeviceProcAddr =
            unsafe { std::mem::transmute(get_device_proc_address) };
        let address = unsafe { get_device_proc(device, c"vkFreeMemory".as_ptr()) };
        if !address.is_null() {
            let free: FreeMemory = unsafe { std::mem::transmute(address) };
            unsafe { free(device, memory, allocator) };
        }
    }
    let imported = IMPORTED_ANDROID_MEMORY.get().and_then(|imports| {
        imports
            .lock()
            .expect("Vulkan import registry poisoned")
            .remove(&(memory as usize))
    });
    let Some(imported) = imported else { return };
    let release_texture_address = unsafe {
        darwin_art_android_platform_symbol(c"darwin_art_android_metal_texture_release".as_ptr())
    };
    let release_buffer_address =
        unsafe { darwin_art_android_platform_symbol(c"AHardwareBuffer_release".as_ptr()) };
    if !release_texture_address.is_null() {
        let release: ReleaseObject = unsafe { std::mem::transmute(release_texture_address) };
        unsafe { release(imported.metal_texture as *mut c_void) };
    }
    if !release_buffer_address.is_null() {
        let release: ReleaseObject = unsafe { std::mem::transmute(release_buffer_address) };
        unsafe { release(imported.hardware_buffer as *mut c_void) };
    }
}

unsafe fn debug_bound_metal_texture(image: *mut c_void, memory: *mut c_void) {
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_none() {
        return;
    }
    type GetMetalTexture = unsafe extern "C" fn(*mut c_void, *mut *mut c_void);
    let address = unsafe { moltenvk_exported_symbol(c"vkGetMTLTextureMVK") };
    let mut bound_texture = ptr::null_mut();
    if !address.is_null() {
        let get_texture: GetMetalTexture = unsafe { std::mem::transmute(address) };
        unsafe { get_texture(image, &mut bound_texture) };
    }
    let imported_texture = IMPORTED_ANDROID_MEMORY.get().and_then(|imports| {
        imports
            .lock()
            .expect("Vulkan import registry poisoned")
            .get(&(memory as usize))
            .map(|entry| entry.metal_texture as *mut c_void)
    });
    match imported_texture {
        Some(imported_texture) => eprintln!(
            "ART Android Vulkan: bound image={image:p} memory={memory:p} imported-texture={imported_texture:p} bound-texture={bound_texture:p} match={}",
            imported_texture == bound_texture
        ),
        None => eprintln!(
            "ART Android Vulkan: bound image={image:p} memory={memory:p} imported-texture=none bound-texture={bound_texture:p}"
        ),
    }
}

unsafe extern "C" fn moltenvk_bind_image_memory(
    device: *mut c_void,
    image: *mut c_void,
    memory: *mut c_void,
    memory_offset: u64,
) -> i32 {
    type Bind = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, u64) -> i32;
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if get_device_proc_address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc: VulkanGetDeviceProcAddr =
        unsafe { std::mem::transmute(get_device_proc_address) };
    let address = unsafe { get_device_proc(device, c"vkBindImageMemory".as_ptr()) };
    if address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let bind: Bind = unsafe { std::mem::transmute(address) };
    let result = unsafe { bind(device, image, memory, memory_offset) };
    if result == VK_SUCCESS {
        unsafe { debug_bound_metal_texture(image, memory) };
    }
    result
}

unsafe extern "C" fn moltenvk_bind_image_memory2(
    device: *mut c_void,
    bind_info_count: u32,
    bind_infos: *const VulkanBindImageMemoryInfo,
) -> i32 {
    type Bind = unsafe extern "C" fn(*mut c_void, u32, *const VulkanBindImageMemoryInfo) -> i32;
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if get_device_proc_address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc: VulkanGetDeviceProcAddr =
        unsafe { std::mem::transmute(get_device_proc_address) };
    let address = unsafe { get_device_proc(device, c"vkBindImageMemory2".as_ptr()) };
    if address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let bind: Bind = unsafe { std::mem::transmute(address) };
    let result = unsafe { bind(device, bind_info_count, bind_infos) };
    if result == VK_SUCCESS && !bind_infos.is_null() {
        for index in 0..bind_info_count as usize {
            let info = unsafe { *bind_infos.add(index) };
            unsafe { debug_bound_metal_texture(info.image, info.memory) };
        }
    }
    result
}

unsafe fn moltenvk_export_metal_shared_event(
    _device: *mut c_void,
    semaphore: *mut c_void,
) -> *mut c_void {
    VULKAN_SEMAPHORES
        .get()
        .and_then(|semaphores| {
            semaphores
                .lock()
                .expect("Vulkan semaphore registry poisoned")
                .get(&(semaphore as usize))
                .copied()
        })
        .map_or(ptr::null_mut(), |state| {
            state.metal_shared_event as *mut c_void
        })
}

unsafe extern "C" fn moltenvk_get_physical_device_external_semaphore_properties(
    physical_device: *mut c_void,
    external_info: *const VulkanPhysicalDeviceExternalSemaphoreInfo,
    properties: *mut VulkanExternalSemaphoreProperties,
) {
    if external_info.is_null() || properties.is_null() {
        return;
    }
    if unsafe { (*external_info).handle_type } == VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT {
        unsafe {
            (*properties).export_from_imported_handle_types =
                VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT;
            (*properties).compatible_handle_types = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT;
            (*properties).external_semaphore_features = VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT
                | VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT;
        }
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            eprintln!(
                "ART Android Vulkan: external-semaphore-properties sync-fd importable=1 exportable=1"
            );
        }
        return;
    }
    type GetProperties = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanPhysicalDeviceExternalSemaphoreInfo,
        *mut VulkanExternalSemaphoreProperties,
    );
    let address =
        unsafe { moltenvk_instance_symbol(c"vkGetPhysicalDeviceExternalSemaphoreProperties") };
    if !address.is_null() {
        let get_properties: GetProperties = unsafe { std::mem::transmute(address) };
        unsafe { get_properties(physical_device, external_info, properties) };
    }
}

unsafe extern "C" fn moltenvk_create_semaphore(
    device: *mut c_void,
    create_info: *const VulkanSemaphoreCreateInfo,
    allocator: *const c_void,
    semaphore: *mut *mut c_void,
) -> i32 {
    type Create = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanSemaphoreCreateInfo,
        *const c_void,
        *mut *mut c_void,
    ) -> i32;
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if get_device_proc_address.is_null() || create_info.is_null() || semaphore.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc: VulkanGetDeviceProcAddr =
        unsafe { std::mem::transmute(get_device_proc_address) };
    let address = unsafe { get_device_proc(device, c"vkCreateSemaphore".as_ptr()) };
    if address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let create: Create = unsafe { std::mem::transmute(address) };
    let mut translated = unsafe { *create_info };
    let mut shared_event = ptr::null_mut();
    let mut next_value = 0;
    let mut metal_import = VulkanImportMetalSharedEventInfo {
        s_type: VK_STRUCTURE_TYPE_IMPORT_METAL_SHARED_EVENT_INFO_EXT,
        p_next: translated.p_next,
        metal_shared_event: ptr::null_mut(),
    };
    if VULKAN_DEVICE_SYNC_FD
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .get(&(device as usize))
        .copied()
        .unwrap_or(false)
    {
        type CreateSharedEvent = unsafe extern "C" fn(*mut c_void, *mut u64) -> *mut c_void;
        let create_event_address = unsafe {
            darwin_art_android_platform_symbol(
                c"darwin_art_android_metal_shared_event_create".as_ptr(),
            )
        };
        let metal_device = unsafe { moltenvk_metal_device() };
        if create_event_address.is_null() || metal_device.is_null() {
            return VK_ERROR_FORMAT_NOT_SUPPORTED;
        }
        let create_event: CreateSharedEvent = unsafe { std::mem::transmute(create_event_address) };
        shared_event = unsafe { create_event(metal_device, &mut next_value) };
        if shared_event.is_null() || next_value == 0 {
            return VK_ERROR_FORMAT_NOT_SUPPORTED;
        }
        metal_import.metal_shared_event = shared_event;
        translated.p_next = (&metal_import as *const VulkanImportMetalSharedEventInfo)
            .cast::<VulkanBaseInStructure>();
    }
    let result = unsafe { create(device, &translated, allocator, semaphore) };
    if result == VK_SUCCESS && !unsafe { *semaphore }.is_null() && !shared_event.is_null() {
        VULKAN_SEMAPHORES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .expect("Vulkan semaphore registry poisoned")
            .insert(
                unsafe { *semaphore } as usize,
                VulkanSemaphoreState {
                    metal_shared_event: shared_event as usize,
                    next_value,
                },
            );
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            eprintln!(
                "ART Android Vulkan: created Metal-shared-event semaphore={:p} event={shared_event:p} value={next_value}",
                unsafe { *semaphore },
            );
        }
    } else if !shared_event.is_null() {
        type ReleaseSharedEvent = unsafe extern "C" fn(*mut c_void);
        let release_address = unsafe {
            darwin_art_android_platform_symbol(
                c"darwin_art_android_metal_shared_event_release".as_ptr(),
            )
        };
        if !release_address.is_null() {
            let release: ReleaseSharedEvent = unsafe { std::mem::transmute(release_address) };
            unsafe { release(shared_event) };
        }
    }
    result
}

unsafe extern "C" fn moltenvk_destroy_semaphore(
    device: *mut c_void,
    semaphore: *mut c_void,
    allocator: *const c_void,
) {
    let state = VULKAN_SEMAPHORES.get().and_then(|semaphores| {
        semaphores
            .lock()
            .expect("Vulkan semaphore registry poisoned")
            .remove(&(semaphore as usize))
    });
    let get_device_proc_address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
    if !get_device_proc_address.is_null() {
        let get_device_proc: VulkanGetDeviceProcAddr =
            unsafe { std::mem::transmute(get_device_proc_address) };
        let address = unsafe { get_device_proc(device, c"vkDestroySemaphore".as_ptr()) };
        if !address.is_null() {
            let destroy: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void) =
                unsafe { std::mem::transmute(address) };
            unsafe { destroy(device, semaphore, allocator) };
        }
    }
    if let Some(state) = state {
        type ReleaseSharedEvent = unsafe extern "C" fn(*mut c_void);
        let release_address = unsafe {
            darwin_art_android_platform_symbol(
                c"darwin_art_android_metal_shared_event_release".as_ptr(),
            )
        };
        if !release_address.is_null() {
            let release: ReleaseSharedEvent = unsafe { std::mem::transmute(release_address) };
            unsafe { release(state.metal_shared_event as *mut c_void) };
        }
    }
}

unsafe fn moltenvk_signal_wsi_acquire(device: *mut c_void, semaphore: *mut c_void) -> i32 {
    let event = unsafe { moltenvk_export_metal_shared_event(device, semaphore) };
    if event.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let next_address = unsafe {
        darwin_art_android_platform_symbol(
            c"darwin_art_android_metal_shared_event_next_value".as_ptr(),
        )
    };
    let import_address = unsafe {
        darwin_art_android_platform_symbol(
            c"darwin_art_android_metal_shared_event_import_fence".as_ptr(),
        )
    };
    if next_address.is_null() || import_address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let next: unsafe extern "C" fn(*mut c_void) -> u64 =
        unsafe { std::mem::transmute(next_address) };
    let signal: unsafe extern "C" fn(*mut c_void, u64, i32) -> i32 =
        unsafe { std::mem::transmute(import_address) };
    // Acquire's semaphore must be unsignaled with no unfinished operations.
    // Engines may recycle render-complete semaphores here: their previous
    // ordinary queue waits advanced MoltenVK's counter, not our FD bookkeeping.
    // At this quiescent boundary the next payload is the completed event + 1.
    let value = unsafe { next(event) };
    if value == 0 || unsafe { signal(event, value, -1) } != 0 {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    if let Some(state) = VULKAN_SEMAPHORES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .get_mut(&(semaphore as usize))
    {
        state.next_value = value.saturating_add(1);
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!("ART Android Vulkan: acquire-signaled semaphore={semaphore:p} value={value}");
    }
    VK_SUCCESS
}

unsafe extern "C" fn moltenvk_import_semaphore_fd(
    device: *mut c_void,
    import_info: *const VulkanImportSemaphoreFdInfo,
) -> i32 {
    if import_info.is_null()
        || unsafe { (*import_info).handle_type } != VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT
    {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let semaphore = unsafe { (*import_info).semaphore };
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: import sync-fd begin fd={} semaphore={semaphore:p}",
            unsafe { (*import_info).fd }
        );
    }
    let shared_event = unsafe { moltenvk_export_metal_shared_event(device, semaphore) };
    if shared_event.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    type ImportFence = unsafe extern "C" fn(*mut c_void, u64, c_int) -> c_int;
    let import_address = unsafe {
        darwin_art_android_platform_symbol(
            c"darwin_art_android_metal_shared_event_import_fence".as_ptr(),
        )
    };
    if import_address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let mut semaphores = VULKAN_SEMAPHORES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("Vulkan semaphore registry poisoned");
    let Some(state) = semaphores.get_mut(&(semaphore as usize)) else {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    };
    let signal_value = state.next_value;
    let import: ImportFence = unsafe { std::mem::transmute(import_address) };
    if unsafe { import(shared_event, signal_value, (*import_info).fd) } != 0 {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    state.next_value = signal_value.saturating_add(1);
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: imported sync-fd={} semaphore={semaphore:p} MetalSharedEvent={shared_event:p} value={signal_value}",
            unsafe { (*import_info).fd }
        );
    }
    VK_SUCCESS
}

unsafe extern "C" fn moltenvk_get_semaphore_fd(
    device: *mut c_void,
    get_info: *const VulkanSemaphoreGetFdInfo,
    output_fd: *mut c_int,
) -> i32 {
    if get_info.is_null()
        || output_fd.is_null()
        || unsafe { (*get_info).handle_type } != VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT
    {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let semaphore = unsafe { (*get_info).semaphore };
    let shared_event = unsafe { moltenvk_export_metal_shared_event(device, semaphore) };
    if shared_event.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    type FenceFd = unsafe extern "C" fn(*mut c_void, u64) -> c_int;
    let fence_address = unsafe {
        darwin_art_android_platform_symbol(
            c"darwin_art_android_metal_shared_event_fence_fd".as_ptr(),
        )
    };
    if fence_address.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let mut semaphores = VULKAN_SEMAPHORES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("Vulkan semaphore registry poisoned");
    let Some(state) = semaphores.get_mut(&(semaphore as usize)) else {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    };
    let signal_value = state.next_value;
    let fence: FenceFd = unsafe { std::mem::transmute(fence_address) };
    let fd = unsafe { fence(shared_event, signal_value) };
    if fd < 0 {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    unsafe { *output_fd = fd };
    state.next_value = signal_value.saturating_add(1);
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: exported sync-fd={fd} semaphore={semaphore:p} MetalSharedEvent={shared_event:p} value={signal_value}"
        );
    }
    VK_SUCCESS
}

unsafe extern "C" fn moltenvk_android_hardware_buffer_unsupported(
    _device: *mut c_void,
    _argument: *const c_void,
    _output: *mut c_void,
) -> i32 {
    VK_ERROR_FORMAT_NOT_SUPPORTED
}

unsafe extern "C" fn moltenvk_destroy_image(
    device: *mut c_void,
    image: *mut c_void,
    allocator: *const c_void,
) {
    if let Some(images) = VULKAN_IMAGE_FORMATS.get() {
        images.lock().unwrap().remove(&(image as usize));
    }
    let address = unsafe { wsi_backend_raw_device_symbol(device as usize, c"vkDestroyImage") };
    if !address.is_null() {
        let destroy: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void) =
            unsafe { std::mem::transmute(address) };
        unsafe { destroy(device, image, allocator) };
    }
}

unsafe extern "C" fn moltenvk_create_image_view(
    device: *mut c_void,
    info: *const c_void,
    allocator: *const c_void,
    output: *mut *mut c_void,
) -> i32 {
    let address = unsafe { wsi_backend_raw_device_symbol(device as usize, c"vkCreateImageView") };
    if address.is_null() || info.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        let words = unsafe { std::slice::from_raw_parts(info.cast::<u64>(), 10) };
        eprintln!("ART Android Vulkan: create-image-view info={info:p} words={words:x?}");
    }
    let create: unsafe extern "C" fn(
        *mut c_void,
        *const c_void,
        *const c_void,
        *mut *mut c_void,
    ) -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { create(device, info, allocator, output) }
}

unsafe extern "C" fn moltenvk_destroy_device(device: *mut c_void, allocator: *const c_void) {
    vulkan_acquire_fences::forget_device(device as usize);
    if let Some(devices) = VULKAN_DEVICE_SYNC_FD.get() {
        devices.lock().unwrap().remove(&(device as usize));
    }
    let address = unsafe { wsi_backend_raw_device_symbol(device as usize, c"vkDestroyDevice") };
    if !address.is_null() {
        let destroy: unsafe extern "C" fn(*mut c_void, *const c_void) =
            unsafe { std::mem::transmute(address) };
        unsafe { destroy(device, allocator) };
    }
}

unsafe extern "C" fn moltenvk_get_device_proc_addr(
    device: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    if name.is_null() {
        return ptr::null_mut();
    }
    let value = unsafe { CStr::from_ptr(name) };
    if let Some(address) =
        vulkan_wsi::lookup(value).or_else(|| vulkan_acquire_fences::lookup(value))
    {
        return address;
    }
    match value.to_bytes() {
        b"vkDestroyDevice" => moltenvk_destroy_device as *mut c_void,
        b"vkGetDeviceProcAddr" => moltenvk_get_device_proc_addr as *mut c_void,
        b"vkGetAndroidHardwareBufferPropertiesANDROID" => {
            moltenvk_get_android_hardware_buffer_properties as *mut c_void
        }
        b"vkGetMemoryAndroidHardwareBufferANDROID" => {
            moltenvk_android_hardware_buffer_unsupported as *mut c_void
        }
        b"vkCreateImage" => moltenvk_create_image as *mut c_void,
        b"vkDestroyImage" => moltenvk_destroy_image as *mut c_void,
        b"vkCreateImageView" => moltenvk_create_image_view as *mut c_void,
        b"vkAllocateMemory" => moltenvk_allocate_memory as *mut c_void,
        b"vkBindImageMemory" => moltenvk_bind_image_memory as *mut c_void,
        b"vkBindImageMemory2" | b"vkBindImageMemory2KHR" => {
            moltenvk_bind_image_memory2 as *mut c_void
        }
        b"vkFreeMemory" => moltenvk_free_memory as *mut c_void,
        b"vkCreateSemaphore" => moltenvk_create_semaphore as *mut c_void,
        b"vkDestroySemaphore" => moltenvk_destroy_semaphore as *mut c_void,
        b"vkImportSemaphoreFdKHR" => moltenvk_import_semaphore_fd as *mut c_void,
        b"vkGetSemaphoreFdKHR" => moltenvk_get_semaphore_fd as *mut c_void,
        _ => {
            let address = unsafe { moltenvk_instance_symbol(c"vkGetDeviceProcAddr") };
            if address.is_null() {
                return ptr::null_mut();
            }
            let get_device_proc_addr: VulkanGetDeviceProcAddr =
                unsafe { std::mem::transmute(address) };
            unsafe { get_device_proc_addr(device, name) }
        }
    }
}

unsafe extern "C" fn vulkan_enumerate_instance_version(_version: *mut u32) -> i32 {
    // This is the Vulkan loader's standard result when no ICD/device is
    // present.  Do not advertise a fake API version: callers should classify
    // Vulkan as unavailable and select their GLES/ANGLE path.
    VK_ERROR_INCOMPATIBLE_DRIVER
}

unsafe extern "C" fn vulkan_enumerate_instance_extension_properties(
    _layer_name: *const c_char,
    property_count: *mut u32,
    _properties: *mut c_void,
) -> i32 {
    if !property_count.is_null() {
        unsafe { *property_count = 0 };
    }
    VK_ERROR_INCOMPATIBLE_DRIVER
}

unsafe extern "C" fn vulkan_enumerate_instance_layer_properties(
    property_count: *mut u32,
    _properties: *mut c_void,
) -> i32 {
    if !property_count.is_null() {
        unsafe { *property_count = 0 };
    }
    VK_SUCCESS
}

unsafe extern "C" fn vulkan_create_instance(
    _create_info: *const c_void,
    _allocator: *const c_void,
    _instance: *mut *mut c_void,
) -> i32 {
    VK_ERROR_INCOMPATIBLE_DRIVER
}

// MoltenVK does not expose Android's WSI surface object on macOS.  Still
// provide the entry point so engines that resolve it unconditionally receive
// a normal Vulkan capability error instead of branching through a null PFN.
// Unity/Swappy treats VK_ERROR_EXTENSION_NOT_PRESENT as an unavailable WSI
// path and can select its GLES/ANGLE backend when one is present.
unsafe extern "C" fn vulkan_create_android_surface(
    _instance: *mut c_void,
    _create_info: *const c_void,
    _allocator: *const c_void,
    surface: *mut *mut c_void,
) -> i32 {
    if !surface.is_null() {
        unsafe { *surface = ptr::null_mut() };
    }
    VK_ERROR_EXTENSION_NOT_PRESENT
}

unsafe extern "C" fn vulkan_get_instance_proc_addr(
    instance: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    if name.is_null() {
        return ptr::null_mut();
    }
    if let Some(provider) = moltenvk() {
        let value = unsafe { CStr::from_ptr(name) };
        if let Some(address) =
            vulkan_wsi::lookup(value).or_else(|| vulkan_acquire_fences::lookup(value))
        {
            return address;
        }
        match value.to_bytes() {
            b"vkDestroyDevice" => return moltenvk_destroy_device as *mut c_void,
            b"vkCreateSemaphore" => return moltenvk_create_semaphore as *mut c_void,
            b"vkDestroySemaphore" => return moltenvk_destroy_semaphore as *mut c_void,
            b"vkCreateImage" => return moltenvk_create_image as *mut c_void,
            b"vkDestroyImage" => return moltenvk_destroy_image as *mut c_void,
            b"vkCreateImageView" => return moltenvk_create_image_view as *mut c_void,
            b"vkAllocateMemory" => return moltenvk_allocate_memory as *mut c_void,
            b"vkFreeMemory" => return moltenvk_free_memory as *mut c_void,
            b"vkGetInstanceProcAddr" => return vulkan_get_instance_proc_addr as *mut c_void,
            b"vkCreateInstance" => return moltenvk_create_instance as *mut c_void,
            b"vkEnumerateInstanceExtensionProperties" => {
                return moltenvk_enumerate_instance_extensions as *mut c_void;
            }
            b"vkCreateMetalSurfaceEXT" | b"vkCreateMacOSSurfaceMVK" => {
                return ptr::null_mut();
            }
            b"vkCreateAndroidSurfaceKHR" => return vulkan_create_android_surface as *mut c_void,
            b"vkEnumerateDeviceExtensionProperties" => {
                return moltenvk_enumerate_device_extensions as *mut c_void;
            }
            b"vkCreateDevice" => return moltenvk_create_device as *mut c_void,
            b"vkGetDeviceProcAddr" => return moltenvk_get_device_proc_addr as *mut c_void,
            b"vkBindImageMemory" => return moltenvk_bind_image_memory as *mut c_void,
            b"vkBindImageMemory2" | b"vkBindImageMemory2KHR" => {
                return moltenvk_bind_image_memory2 as *mut c_void;
            }
            b"vkGetPhysicalDeviceImageFormatProperties2"
            | b"vkGetPhysicalDeviceImageFormatProperties2KHR" => {
                return moltenvk_get_physical_device_image_format_properties as *mut c_void;
            }
            b"vkGetPhysicalDeviceExternalSemaphoreProperties"
            | b"vkGetPhysicalDeviceExternalSemaphorePropertiesKHR" => {
                return moltenvk_get_physical_device_external_semaphore_properties as *mut c_void;
            }
            _ => {}
        }
        // SAFETY: instance and name come directly from the Android Vulkan
        // caller and MoltenVK implements the same platform-neutral Vulkan ABI.
        return unsafe { (provider.get_instance_proc_addr)(instance, name) };
    }
    // SAFETY: callers provide a Vulkan function-name C string, as required by
    // vkGetInstanceProcAddr.  Invalid UTF-8 is irrelevant; compare bytes.
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    match name {
        b"vkGetInstanceProcAddr" => vulkan_get_instance_proc_addr as *mut c_void,
        b"vkEnumerateInstanceVersion" => vulkan_enumerate_instance_version as *mut c_void,
        b"vkEnumerateInstanceExtensionProperties" => {
            vulkan_enumerate_instance_extension_properties as *mut c_void
        }
        b"vkEnumerateInstanceLayerProperties" => {
            vulkan_enumerate_instance_layer_properties as *mut c_void
        }
        b"vkCreateInstance" => vulkan_create_instance as *mut c_void,
        b"vkCreateAndroidSurfaceKHR" => vulkan_create_android_surface as *mut c_void,
        _ => ptr::null_mut(),
    }
}

unsafe fn libandroid_symbol(name: &CStr) -> *mut c_void {
    #[cfg(test)]
    {
        let _ = name;
        ptr::dangling_mut::<c_void>()
    }
    #[cfg(not(test))]
    {
        match name.to_bytes() {
            b"ANativeWindow_fromSurface" => host_anative_window_from_surface as *mut c_void,
            b"ANativeWindow_release" => host_anative_window_release as *mut c_void,
            b"ANativeWindow_lock" => host_anative_window_lock as *mut c_void,
            b"ANativeWindow_unlockAndPost" => host_anative_window_unlock_and_post as *mut c_void,
            b"ANativeWindow_setBuffersGeometry" => {
                host_anative_window_set_buffers_geometry as *mut c_void
            }
            b"ASharedMemory_create" => host_ashared_memory_create as *mut c_void,
            b"ASharedMemory_setProt" => host_ashared_memory_set_prot as *mut c_void,
            // Android feature detection commonly dlopen()s libandroid.so and
            // resolves newer NDK APIs lazily. Keep that virtual DSO backed by
            // the same capability table used for regular ELF imports.
            _ => darwin_art_android_platform_symbol(name.as_ptr()),
        }
    }
}

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
    static ERROR_RETURN: RefCell<Option<CString>> = const { RefCell::new(None) };
}

const ERROR_CAPACITY: usize = 512;

fn set_error(message: impl AsRef<str>) {
    let bytes = message.as_ref().as_bytes();
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let value = CString::new(&bytes[..end]).expect("interior NUL removed");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(value));
}

fn call_error(buffer: &[c_char]) -> String {
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[no_mangle]
/// Binds the process-wide loader callbacks exactly once.
///
/// # Safety
///
/// `callbacks` must point to a readable `LoaderCallbacks`. Its context must
/// remain valid for the process lifetime and its function pointers must obey
/// their declared C ABIs.
pub unsafe extern "C" fn darwin_art_loader_bind(callbacks: *const LoaderCallbacks) -> c_int {
    if callbacks.is_null() {
        return -1;
    }
    // SAFETY: null was rejected and the value is copied immediately.
    let callbacks = unsafe { *callbacks };
    if callbacks.open.is_none() || callbacks.lookup.is_none() || callbacks.close.is_none() {
        return -1;
    }
    if LOADER.set(callbacks).is_err() {
        -1
    } else {
        0
    }
}

fn loader() -> Option<&'static LoaderCallbacks> {
    let value = LOADER.get();
    if value.is_none() {
        set_error("Android ELF loader is not bound");
    }
    value
}

#[no_mangle]
/// Opens an Android DSO through the bound loader.
///
/// # Safety
///
/// `filename` must be null or point to a valid NUL-terminated C string.
pub unsafe extern "C" fn darwin_art_bionic_dlopen(
    filename: *const c_char,
    flags: c_int,
) -> *mut c_void {
    darwin_art_bionic_android_dlopen_ext(filename, flags, ptr::null())
}

#[no_mangle]
/// Looks up an exported symbol in a loader-owned handle.
///
/// # Safety
///
/// `handle` must be accepted by the bound loader and `symbol` must point to a
/// valid NUL-terminated C string.
pub unsafe extern "C" fn darwin_art_bionic_dlsym(
    handle: *mut c_void,
    symbol: *const c_char,
) -> *mut c_void {
    let egl_handle = ptr::addr_of_mut!(LIBEGL_HANDLE_TOKEN).cast();
    let gles_handle = ptr::addr_of_mut!(LIBGLESV2_HANDLE_TOKEN).cast();
    let vulkan_handle = ptr::addr_of_mut!(LIBVULKAN_HANDLE_TOKEN).cast();
    if handle == egl_handle || handle == gles_handle {
        if symbol.is_null() {
            set_error("null Android graphics DSO symbol");
            return ptr::null_mut();
        }
        let soname = if handle == egl_handle {
            c"libEGL.so"
        } else {
            c"libGLESv2.so"
        };
        #[cfg(not(test))]
        let result = unsafe { darwin_art_angle_dso_symbol(soname.as_ptr(), symbol) };
        #[cfg(test)]
        let result = {
            let _ = soname;
            ptr::dangling_mut::<c_void>()
        };
        if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
            let name = unsafe { CStr::from_ptr(symbol) };
            eprintln!(
                "ART Android libdl: {} dlsym {} resolved={}",
                soname.to_string_lossy(),
                name.to_string_lossy(),
                !result.is_null()
            );
        }
        if result.is_null() {
            set_error("Android graphics DSO symbol is unavailable");
        }
        return result;
    }
    if handle == vulkan_handle {
        return unsafe { vulkan_library::legacy_lookup(symbol) };
    }
    if symbol.is_null() {
        set_error("dlsym symbol is null");
        return ptr::null_mut();
    }
    if handle == libandroid_handle() || handle == libnativewindow_handle() {
        let name = unsafe { CStr::from_ptr(symbol) };
        let result = unsafe { libandroid_symbol(name) };
        if std::env::var_os("DARWIN_ART_DEBUG_ANATIVEWINDOW").is_some() {
            let soname = if handle == libandroid_handle() {
                "libandroid.so"
            } else {
                "libnativewindow.so"
            };
            eprintln!(
                "ART Android libdl: {soname} dlsym {} resolved={}",
                name.to_string_lossy(),
                !result.is_null()
            );
        }
        if result.is_null() {
            set_error("Android platform DSO symbol is unsupported");
        }
        return result;
    }
    let Some(loader) = loader() else {
        return ptr::null_mut();
    };
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let result = unsafe {
        (loader.lookup.expect("validated"))(
            loader.context,
            handle,
            symbol,
            ptr::null(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if result.is_null() {
        let message = call_error(&error);
        set_error(if message.is_empty() {
            "Android dlsym failed"
        } else {
            &message
        });
    }
    result
}

#[no_mangle]
/// Releases a loader-owned DSO handle.
///
/// # Safety
///
/// `handle` must be null or a handle accepted by the bound loader.
pub unsafe extern "C" fn darwin_art_bionic_dlclose(handle: *mut c_void) -> c_int {
    if handle == libandroid_handle()
        || handle == libnativewindow_handle()
        || handle == ptr::addr_of_mut!(LIBEGL_HANDLE_TOKEN).cast()
        || handle == ptr::addr_of_mut!(LIBGLESV2_HANDLE_TOKEN).cast()
        || handle == ptr::addr_of_mut!(LIBVULKAN_HANDLE_TOKEN).cast()
    {
        return 0;
    }
    let Some(loader) = loader() else { return -1 };
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let result = unsafe {
        (loader.close.expect("validated"))(loader.context, handle, error.as_mut_ptr(), error.len())
    };
    if result != 0 {
        let message = call_error(&error);
        set_error(if message.is_empty() {
            "Android dlclose failed"
        } else {
            &message
        });
        -1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn darwin_art_bionic_dlerror() -> *mut c_char {
    let pending = LAST_ERROR.with(|slot| slot.borrow_mut().take());
    ERROR_RETURN.with(|slot| {
        *slot.borrow_mut() = pending;
        slot.borrow()
            .as_ref()
            .map_or(ptr::null_mut(), |message| message.as_ptr() as *mut c_char)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub provider: u32,
    pub ordinal: u32,
    pub address: usize,
}

const LIBDL: &[(&str, &str)] = &[
    ("android_dlopen_ext", "LIBC"),
    ("dlclose", "LIBC"),
    ("dlerror", "LIBC"),
    ("dlopen", "LIBC"),
    ("dlsym", "LIBC"),
];

// Exact NDK r28 API 35 liblog stub surface. An address of zero means that the
// ELF loader must obtain the address from the linked AOSP liblog provider table,
// never from Darwin's process-global namespace.
const LIBLOG: &[&str] = &[
    "__android_log_assert",
    "__android_log_buf_print",
    "__android_log_buf_write",
    "__android_log_call_aborter",
    "__android_log_default_aborter",
    "__android_log_get_minimum_priority",
    "__android_log_is_loggable",
    "__android_log_is_loggable_len",
    "__android_log_logd_logger",
    "__android_log_print",
    "__android_log_set_aborter",
    "__android_log_set_default_tag",
    "__android_log_set_logger",
    "__android_log_set_minimum_priority",
    "__android_log_stderr_logger",
    "__android_log_vprint",
    "__android_log_write",
    "__android_log_write_log_message",
];

pub fn resolve(soname: &str, symbol: &str, version: Option<&str>) -> Result<Resolution, u32> {
    match soname {
        "libdl.so" => {
            let Some((ordinal, (_, expected))) = LIBDL
                .iter()
                .enumerate()
                .find(|(_, entry)| entry.0 == symbol)
            else {
                return Err(2);
            };
            if version.is_some_and(|actual| actual != *expected) {
                return Err(3);
            }
            let address = match ordinal {
                0 => darwin_art_bionic_android_dlopen_ext as usize,
                1 => darwin_art_bionic_dlclose as usize,
                2 => darwin_art_bionic_dlerror as usize,
                3 => darwin_art_bionic_dlopen as usize,
                4 => darwin_art_bionic_dlsym as usize,
                _ => unreachable!(),
            };
            Ok(Resolution {
                provider: PROVIDER_LOADER_LIBDL,
                ordinal: ordinal as u32,
                address,
            })
        }
        "liblog.so" => {
            let Some(ordinal) = LIBLOG.iter().position(|candidate| *candidate == symbol) else {
                return Err(2);
            };
            // The API 35 NDK stub deliberately has no GNU version definitions.
            if version.is_some() {
                return Err(3);
            }
            Ok(Resolution {
                provider: PROVIDER_AOSP_LIBLOG,
                ordinal: ordinal as u32,
                address: 0,
            })
        }
        _ => Err(1),
    }
}

#[repr(C)]
pub struct CResolution {
    pub provider: u32,
    pub ordinal: u32,
    pub address: usize,
}

#[no_mangle]
/// Resolves an allowlisted virtual Android DSO symbol.
///
/// # Safety
///
/// `soname` and `symbol` must be valid C strings, `version` must be null or a
/// valid C string, and `output` must point to writable `CResolution` storage.
pub unsafe extern "C" fn darwin_art_dso_resolve(
    soname: *const c_char,
    symbol: *const c_char,
    version: *const c_char,
    output: *mut CResolution,
) -> c_int {
    if soname.is_null() || symbol.is_null() || output.is_null() {
        return 4;
    }
    let Ok(soname) = unsafe { CStr::from_ptr(soname) }.to_str() else {
        return 4;
    };
    let Ok(symbol) = unsafe { CStr::from_ptr(symbol) }.to_str() else {
        return 4;
    };
    let version = if version.is_null() {
        None
    } else {
        let Ok(value) = unsafe { CStr::from_ptr(version) }.to_str() else {
            return 4;
        };
        Some(value)
    };
    match resolve(soname, symbol, version) {
        Ok(result) => {
            unsafe {
                *output = CResolution {
                    provider: result.provider,
                    ordinal: result.ordinal,
                    address: result.address,
                }
            };
            0
        }
        Err(code) => code as c_int,
    }
}

pub fn manifest() -> String {
    let mut output = String::new();
    for (symbol, version) in LIBDL {
        output.push_str(&format!("libdl.so\t{symbol}\t{version}\tloader\n"));
    }
    for symbol in LIBLOG {
        output.push_str(&format!("liblog.so\t{symbol}\t-\taosp-liblog\n"));
    }
    output
}

#[cfg(test)]
mod tests {
    #[test]
    fn android_wsi_does_not_advertise_unsupported_presentation_extensions() {
        for name in [
            c"VK_KHR_present_wait",
            c"VK_EXT_hdr_metadata",
            c"VK_KHR_swapchain_maintenance1",
            c"VK_GOOGLE_display_timing",
        ] {
            assert!(!super::android_device_extension_supported(name));
        }
        for name in [
            c"VK_KHR_swapchain",
            c"VK_ANDROID_external_memory_android_hardware_buffer",
            c"VK_KHR_external_memory",
            c"VK_KHR_timeline_semaphore",
        ] {
            assert!(super::android_device_extension_supported(name));
        }
    }
    use super::*;

    #[test]
    fn namespace_is_closed_and_versioned() {
        unsafe extern "C" fn open(
            _: *mut c_void,
            filename: *const c_char,
            flags: c_int,
            extinfo: *const AndroidDlExtInfo,
            error: *mut c_char,
            _: usize,
        ) -> *mut c_void {
            if let Some(info) = unsafe { extinfo.as_ref() } {
                if info.flags != 0 {
                    assert_eq!(flags, 2);
                    assert_eq!(info.library_namespace as usize, 0xfeed);
                    assert_eq!(info.library_fd, 73);
                    let message = b"explicit request rejected by linker\0";
                    unsafe {
                        ptr::copy_nonoverlapping(message.as_ptr().cast(), error, message.len());
                    }
                    return ptr::null_mut();
                }
            }
            if !filename.is_null() && unsafe { CStr::from_ptr(filename) }.to_bytes() == b"bad.so" {
                let message = b"virtual SONAME denied\0";
                unsafe {
                    ptr::copy_nonoverlapping(
                        message.as_ptr() as *const c_char,
                        error,
                        message.len(),
                    )
                };
                ptr::null_mut()
            } else {
                0x1234usize as *mut c_void
            }
        }
        unsafe extern "C" fn lookup(
            _: *mut c_void,
            _: *mut c_void,
            _: *const c_char,
            _: *const c_char,
            _: *mut c_char,
            _: usize,
        ) -> *mut c_void {
            0x5678usize as *mut c_void
        }
        unsafe extern "C" fn close(
            _: *mut c_void,
            _: *mut c_void,
            _: *mut c_char,
            _: usize,
        ) -> c_int {
            0
        }

        assert_eq!(
            resolve("libdl.so", "dlopen", Some("LIBC"))
                .unwrap()
                .provider,
            1
        );
        assert_eq!(resolve("libdl.so", "dlopen", Some("GLIBC_2.0")), Err(3));
        assert_eq!(resolve("libc.so", "malloc", Some("LIBC")), Err(1));
        assert_eq!(
            resolve("liblog.so", "__android_log_print", None)
                .unwrap()
                .provider,
            2
        );
        assert_eq!(
            resolve("liblog.so", "__android_log_print", Some("LIBLOG")),
            Err(3)
        );
        assert_eq!(resolve("liblog.so", "android_log_private", None), Err(2));
        assert_eq!(std::mem::size_of::<AndroidDlExtInfo>(), 48);

        let callbacks = LoaderCallbacks {
            context: ptr::null_mut(),
            open: Some(open),
            lookup: Some(lookup),
            close: Some(close),
        };
        assert_eq!(unsafe { darwin_art_loader_bind(&callbacks) }, 0);
        let explicit = AndroidDlExtInfo {
            flags: 0x200,
            reserved_addr: ptr::null_mut(),
            reserved_size: 0,
            relro_fd: -1,
            library_fd: 73,
            library_fd_offset: 0,
            library_namespace: 0xfeedusize as *mut c_void,
        };
        for name in [
            c"libandroid.so",
            c"libnativewindow.so",
            c"libEGL.so",
            c"libGLESv2.so",
            c"libvulkan.so",
        ] {
            assert!(
                unsafe { darwin_art_bionic_android_dlopen_ext(name.as_ptr(), 2, &explicit) }
                    .is_null()
            );
            assert_eq!(
                unsafe { CStr::from_ptr(darwin_art_bionic_dlerror()) }.to_bytes(),
                b"explicit request rejected by linker"
            );
            assert!(darwin_art_bionic_dlerror().is_null());
        }
        // An explicit file is not substituted by a basename-matching token.
        assert_eq!(
            unsafe { darwin_art_bionic_dlopen(c"/private/libEGL.so".as_ptr(), 2) } as usize,
            0x1234
        );
        let good = CString::new("good.so").unwrap();
        let symbol = CString::new("fixture_symbol").unwrap();
        let handle = unsafe { darwin_art_bionic_dlopen(good.as_ptr(), 2) };
        assert_eq!(handle as usize, 0x1234);
        assert_eq!(
            unsafe { darwin_art_bionic_dlsym(handle, symbol.as_ptr()) } as usize,
            0x5678
        );
        assert_eq!(unsafe { darwin_art_bionic_dlclose(handle) }, 0);
        assert!(darwin_art_bionic_dlerror().is_null());

        let vulkan = CString::new("libvulkan.so").unwrap();
        let vulkan_handle = unsafe { darwin_art_bionic_dlopen(vulkan.as_ptr(), 2) };
        assert!(!vulkan_handle.is_null());
        let get_proc_name = CString::new("vkGetInstanceProcAddr").unwrap();
        let get_proc = unsafe { darwin_art_bionic_dlsym(vulkan_handle, get_proc_name.as_ptr()) };
        assert!(!get_proc.is_null());
        let enumerate_name = CString::new("vkEnumerateInstanceVersion").unwrap();
        let enumerate = unsafe { darwin_art_bionic_dlsym(vulkan_handle, enumerate_name.as_ptr()) };
        assert!(!enumerate.is_null());
        let enumerate: unsafe extern "C" fn(*mut u32) -> i32 =
            unsafe { std::mem::transmute(enumerate) };
        assert_eq!(
            unsafe { enumerate(ptr::null_mut()) },
            VK_ERROR_INCOMPATIBLE_DRIVER
        );
        assert_eq!(unsafe { darwin_art_bionic_dlclose(vulkan_handle) }, 0);

        let native_window = CString::new("libnativewindow.so").unwrap();
        let native_window_handle = unsafe { darwin_art_bionic_dlopen(native_window.as_ptr(), 2) };
        assert!(!native_window_handle.is_null());
        let describe_name = CString::new("AHardwareBuffer_describe").unwrap();
        assert!(
            !unsafe { darwin_art_bionic_dlsym(native_window_handle, describe_name.as_ptr()) }
                .is_null()
        );
        assert_eq!(
            unsafe { darwin_art_bionic_dlclose(native_window_handle) },
            0
        );

        let bad = CString::new("bad.so").unwrap();
        assert!(unsafe { darwin_art_bionic_dlopen(bad.as_ptr(), 2) }.is_null());
        let error = darwin_art_bionic_dlerror();
        assert_eq!(
            unsafe { CStr::from_ptr(error) }.to_bytes(),
            b"virtual SONAME denied"
        );
        assert!(darwin_art_bionic_dlerror().is_null());
    }
}
