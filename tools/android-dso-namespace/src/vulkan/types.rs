//! Shared private Vulkan ABI records and Android capability constants.
//!
//! These records are not exported from the namespace root; owner modules
//! import only the records they need.

use std::ffi::{c_char, c_int, c_void, CStr};

pub(super) type VulkanGetInstanceProcAddr =
    unsafe extern "C" fn(instance: *mut c_void, name: *const c_char) -> *mut c_void;
pub(super) type VulkanGetDeviceProcAddr =
    unsafe extern "C" fn(device: *mut c_void, name: *const c_char) -> *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanExtensionProperties {
    pub(super) extension_name: [c_char; 256],
    pub(super) spec_version: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanDeviceCreateInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const c_void,
    pub(super) flags: u32,
    pub(super) queue_create_info_count: u32,
    pub(super) queue_create_infos: *const c_void,
    pub(super) enabled_layer_count: u32,
    pub(super) enabled_layer_names: *const *const c_char,
    pub(super) enabled_extension_count: u32,
    pub(super) enabled_extension_names: *const *const c_char,
    pub(super) enabled_features: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct AHardwareBufferDesc {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) layers: u32,
    pub(super) format: u32,
    pub(super) usage: u64,
    pub(super) stride: u32,
    pub(super) rfu0: u32,
    pub(super) rfu1: u64,
}

#[repr(C)]
pub(super) struct VulkanBaseOutStructure {
    pub(super) s_type: i32,
    pub(super) p_next: *mut VulkanBaseOutStructure,
}

#[repr(C)]
pub(super) struct VulkanAndroidHardwareBufferProperties {
    pub(super) s_type: i32,
    pub(super) p_next: *mut VulkanBaseOutStructure,
    pub(super) allocation_size: u64,
    pub(super) memory_type_bits: u32,
}

#[repr(C)]
pub(super) struct VulkanAndroidHardwareBufferFormatProperties {
    pub(super) s_type: i32,
    pub(super) p_next: *mut VulkanBaseOutStructure,
    pub(super) format: i32,
    pub(super) external_format: u64,
    pub(super) format_features: u32,
    pub(super) sampler_ycbcr_conversion_components: [i32; 4],
    pub(super) suggested_ycbcr_model: i32,
    pub(super) suggested_ycbcr_range: i32,
    pub(super) suggested_x_chroma_offset: i32,
    pub(super) suggested_y_chroma_offset: i32,
}

#[repr(C)]
pub(super) struct VulkanMemoryMetalHandleProperties {
    pub(super) s_type: i32,
    pub(super) p_next: *mut c_void,
    pub(super) memory_type_bits: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanPhysicalDeviceImageFormatInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) format: i32,
    pub(super) image_type: i32,
    pub(super) tiling: i32,
    pub(super) usage: u32,
    pub(super) flags: u32,
}

#[repr(C)]
pub(super) struct VulkanBaseInStructure {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
}

#[repr(C)]
pub(super) struct VulkanPhysicalDeviceExternalImageFormatInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) handle_type: u32,
}

#[repr(C)]
pub(super) struct VulkanExternalImageFormatProperties {
    pub(super) s_type: i32,
    pub(super) p_next: *mut VulkanBaseOutStructure,
    pub(super) external_memory_features: u32,
    pub(super) export_from_imported_handle_types: u32,
    pub(super) compatible_handle_types: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanImageCreateInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) flags: u32,
    pub(super) image_type: i32,
    pub(super) format: i32,
    pub(super) extent: [u32; 3],
    pub(super) mip_levels: u32,
    pub(super) array_layers: u32,
    pub(super) samples: u32,
    pub(super) tiling: i32,
    pub(super) usage: u32,
    pub(super) sharing_mode: i32,
    pub(super) queue_family_index_count: u32,
    pub(super) queue_family_indices: *const u32,
    pub(super) initial_layout: i32,
}

#[repr(C)]
pub(super) struct VulkanExternalMemoryImageCreateInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) handle_types: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanMemoryAllocateInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) allocation_size: u64,
    pub(super) memory_type_index: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanMemoryDedicatedAllocateInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) image: *mut c_void,
    pub(super) buffer: *mut c_void,
}

#[repr(C)]
pub(super) struct VulkanImportAndroidHardwareBufferInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) buffer: *mut c_void,
}

#[repr(C)]
pub(super) struct VulkanImportMemoryMetalHandleInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) handle_type: u32,
    pub(super) handle: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanBindImageMemoryInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) image: *mut c_void,
    pub(super) memory: *mut c_void,
    pub(super) memory_offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct VulkanSemaphoreCreateInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) flags: u32,
}

#[repr(C)]
pub(super) struct VulkanImportMetalSharedEventInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) metal_shared_event: *mut c_void,
}

#[repr(C)]
pub(super) struct VulkanPhysicalDeviceExternalSemaphoreInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) handle_type: u32,
}

#[repr(C)]
pub(super) struct VulkanExternalSemaphoreProperties {
    pub(super) s_type: i32,
    pub(super) p_next: *mut VulkanBaseOutStructure,
    pub(super) export_from_imported_handle_types: u32,
    pub(super) compatible_handle_types: u32,
    pub(super) external_semaphore_features: u32,
}

#[repr(C)]
pub(super) struct VulkanImportSemaphoreFdInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) semaphore: *mut c_void,
    pub(super) flags: u32,
    pub(super) handle_type: u32,
    pub(super) fd: c_int,
}

#[repr(C)]
pub(super) struct VulkanSemaphoreGetFdInfo {
    pub(super) s_type: i32,
    pub(super) p_next: *const VulkanBaseInStructure,
    pub(super) semaphore: *mut c_void,
    pub(super) handle_type: u32,
}

pub(super) const VK_INCOMPLETE: i32 = 5;
pub(super) const VK_ERROR_EXTENSION_NOT_PRESENT: i32 = -7;
pub(super) const VK_ERROR_FORMAT_NOT_SUPPORTED: i32 = -11;
pub(super) const VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID: i32 =
    1_000_129_002;
pub(super) const VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID: i32 = 1_000_129_003;
pub(super) const VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO: i32 = 1_000_127_001;
pub(super) const VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO: i32 = 1_000_071_000;
pub(super) const VK_STRUCTURE_TYPE_EXTERNAL_IMAGE_FORMAT_PROPERTIES: i32 = 1_000_071_001;
pub(super) const VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO: i32 = 1_000_072_001;
pub(super) const VK_STRUCTURE_TYPE_IMPORT_MEMORY_METAL_HANDLE_INFO_EXT: i32 = 1_000_602_000;
pub(super) const VK_STRUCTURE_TYPE_MEMORY_METAL_HANDLE_PROPERTIES_EXT: i32 = 1_000_602_001;
pub(super) const VK_STRUCTURE_TYPE_IMPORT_METAL_SHARED_EVENT_INFO_EXT: i32 = 1_000_311_011;
pub(super) const VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID: u32 =
    0x0000_0400;
pub(super) const VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT: u32 = 0x0002_0000;
pub(super) const VK_SUCCESS: i32 = 0;
pub(super) const VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT: u32 = 0x0000_0004;
pub(super) const VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT: u32 = 0x0000_0010;
pub(super) const VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT: u32 = 0x0000_0001;
pub(super) const VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT: u32 = 0x0000_0002;
pub(super) const VK_FORMAT_R8G8B8A8_UNORM: i32 = 37;
pub(super) const VK_FORMAT_FEATURE_RGBA_RENDERABLE: u32 = 0x0000_c083;
pub(super) const ANDROID_AHB_EXTENSION: &CStr =
    c"VK_ANDROID_external_memory_android_hardware_buffer";
pub(super) const QUEUE_FAMILY_FOREIGN_EXTENSION: &CStr = c"VK_EXT_queue_family_foreign";
pub(super) const METAL_EXTERNAL_MEMORY_EXTENSION: &CStr = c"VK_EXT_external_memory_metal";
pub(super) const EXTERNAL_SEMAPHORE_FD_EXTENSION: &CStr = c"VK_KHR_external_semaphore_fd";
pub(super) const METAL_OBJECTS_EXTENSION: &CStr = c"VK_EXT_metal_objects";
pub(super) const HOST_METAL_SURFACE_EXTENSION: &CStr = c"VK_EXT_metal_surface";
pub(super) const HOST_MACOS_SURFACE_EXTENSION: &CStr = c"VK_MVK_macos_surface";
pub(super) const ANDROID_SURFACE_EXTENSION: &CStr = c"VK_KHR_android_surface";
