//! Android Vulkan provider: guest proc dispatch, capability filtering, and device policy.
use crate::vulkan_acquire_fences;
use crate::vulkan_android_memory::{
    moltenvk_allocate_memory, moltenvk_bind_image_memory, moltenvk_bind_image_memory2,
    moltenvk_create_image, moltenvk_create_image_view, moltenvk_destroy_image,
    moltenvk_free_memory, moltenvk_get_android_hardware_buffer_properties,
    moltenvk_get_physical_device_image_format_properties,
};
use crate::vulkan_android_sync::{
    forget_device_sync_fd, moltenvk_create_semaphore, moltenvk_destroy_semaphore,
    moltenvk_get_physical_device_external_semaphore_properties, moltenvk_get_semaphore_fd,
    moltenvk_import_semaphore_fd, remember_device_sync_fd,
};
use crate::vulkan_backend::{
    moltenvk, provider_exported_symbol, raw_device_symbol, raw_instance_symbol,
};
use crate::vulkan_device_owner;
use crate::vulkan_types::{
    VulkanDeviceCreateInfo, VulkanExtensionProperties, VulkanGetDeviceProcAddr,
    ANDROID_AHB_EXTENSION, ANDROID_SURFACE_EXTENSION, EXTERNAL_SEMAPHORE_FD_EXTENSION,
    HOST_MACOS_SURFACE_EXTENSION, HOST_METAL_SURFACE_EXTENSION, METAL_EXTERNAL_MEMORY_EXTENSION,
    METAL_OBJECTS_EXTENSION, QUEUE_FAMILY_FOREIGN_EXTENSION, VK_ERROR_EXTENSION_NOT_PRESENT,
    VK_ERROR_FORMAT_NOT_SUPPORTED, VK_INCOMPLETE,
};
use crate::vulkan_wsi;
use std::ffi::{c_char, c_void, CStr};
use std::ptr;

const VK_SUCCESS: i32 = 0;
const VK_ERROR_INCOMPATIBLE_DRIVER: i32 = -9;

pub(super) fn ready() -> bool {
    moltenvk().is_some()
}

// Trace only the Android import contract, not every Vulkan command. This
// distinguishes guest lookup routes without changing dispatch or capabilities.
fn trace_android_import_lookup(route: &str, owner: *mut c_void, name: &CStr) {
    if matches!(
        name.to_bytes(),
        b"vkGetAndroidHardwareBufferPropertiesANDROID"
            | b"vkGetMemoryAndroidHardwareBufferANDROID"
            | b"vkImportSemaphoreFdKHR"
            | b"vkGetSemaphoreFdKHR"
    ) && std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some()
    {
        eprintln!(
            "ART Android Vulkan: pid={} import-proc-request route={} owner={:p} name={}",
            std::process::id(),
            route,
            owner,
            name.to_string_lossy()
        );
    }
}

pub(super) unsafe extern "C" fn moltenvk_create_instance(
    create_info: *const c_void,
    allocator: *const c_void,
    instance: *mut *mut c_void,
) -> i32 {
    if !ready() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let address = unsafe { raw_instance_symbol(ptr::null_mut(), c"vkCreateInstance") };
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
    result
}

pub(super) unsafe extern "C" fn moltenvk_android_hardware_buffer_unsupported(
    device: *mut c_void,
    argument: *const c_void,
    output: *mut c_void,
) -> i32 {
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: pid={} get-memory-AHB entry device={device:p} argument={argument:p} output={output:p} result={VK_ERROR_FORMAT_NOT_SUPPORTED}",
            std::process::id()
        );
    }
    VK_ERROR_FORMAT_NOT_SUPPORTED
}

fn is_host_surface_extension(property: &VulkanExtensionProperties) -> bool {
    // SAFETY: VkExtensionProperties guarantees a NUL-terminated name.
    let name = unsafe { CStr::from_ptr(property.extension_name.as_ptr()) };
    name == HOST_METAL_SURFACE_EXTENSION || name == HOST_MACOS_SURFACE_EXTENSION
}

pub(super) unsafe extern "C" fn moltenvk_enumerate_instance_extensions(
    layer_name: *const c_char,
    property_count: *mut u32,
    properties: *mut VulkanExtensionProperties,
) -> i32 {
    type Enumerate =
        unsafe extern "C" fn(*const c_char, *mut u32, *mut VulkanExtensionProperties) -> i32;
    if !ready() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    if property_count.is_null() {
        return VK_ERROR_INCOMPATIBLE_DRIVER;
    }
    let address =
        unsafe { raw_instance_symbol(ptr::null_mut(), c"vkEnumerateInstanceExtensionProperties") };
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
            "ART Android Vulkan: pid={} instance-extensions native={} returned={} host-wsi=0 android-wsi={}",
            std::process::id(),
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

pub(super) unsafe extern "C" fn moltenvk_enumerate_device_extensions(
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
    let address = unsafe { provider_exported_symbol(c"vkEnumerateDeviceExtensionProperties") };
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
            "ART Android Vulkan: pid={} device-extensions native={} returned={} android-ahb=1 queue-family-foreign=1 sync-fd=1 android-swapchain=1",
            std::process::id(),
            native_count, written
        );
    }
    if written < native.len() {
        VK_INCOMPLETE
    } else {
        VK_SUCCESS
    }
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

pub(super) unsafe extern "C" fn moltenvk_create_device(
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
    let address = unsafe { provider_exported_symbol(c"vkCreateDevice") };
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
            "ART Android Vulkan: pid={} create-device android-ahb={} queue-family-foreign={} sync-fd={} metal-external-memory={} metal-objects={} extensions={}",
            std::process::id(),
            requested_android_ahb,
            requested_queue_family_foreign,
            requested_sync_fd,
            requested_android_ahb || has_metal_external_memory,
            requested_sync_fd || has_metal_objects,
            translated.len()
        );
    }
    let bridge_requested = requested_android_ahb || requested_sync_fd;
    type GetMetalDevice = unsafe extern "C" fn(*mut c_void, *mut *mut c_void);
    let get_metal_address = unsafe { provider_exported_symbol(c"vkGetMTLDeviceMVK") };
    let mut metal_device = ptr::null_mut();
    if !get_metal_address.is_null() {
        let get_metal: GetMetalDevice = unsafe { std::mem::transmute(get_metal_address) };
        unsafe { get_metal(physical_device, &mut metal_device) };
    }
    let mut translated_info = *original;
    translated_info.enabled_extension_count = translated.len() as u32;
    translated_info.enabled_extension_names = translated.as_ptr();
    let result = unsafe {
        vulkan_device_owner::create_and_record(
            device,
            physical_device,
            metal_device,
            bridge_requested,
            || unsafe { create(physical_device, &translated_info, allocator, device) },
        )
    };
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: pid={} create-device result={} device={:p}",
            std::process::id(),
            result,
            if device.is_null() {
                ptr::null_mut()
            } else {
                unsafe { *device }
            }
        );
    }
    if result == VK_SUCCESS && !device.is_null() && !unsafe { *device }.is_null() {
        remember_device_sync_fd(unsafe { *device } as usize, requested_sync_fd);
    }
    result
}
pub(super) unsafe extern "C" fn moltenvk_destroy_device(
    device: *mut c_void,
    allocator: *const c_void,
) {
    vulkan_device_owner::remove(device);
    vulkan_acquire_fences::forget_device(device as usize);
    forget_device_sync_fd(device as usize);
    let address = unsafe { raw_device_symbol(device as usize, c"vkDestroyDevice") };
    if !address.is_null() {
        let destroy: unsafe extern "C" fn(*mut c_void, *const c_void) =
            unsafe { std::mem::transmute(address) };
        unsafe { destroy(device, allocator) };
    }
}

pub(super) unsafe extern "C" fn moltenvk_get_device_proc_addr(
    device: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    if name.is_null() {
        return ptr::null_mut();
    }
    let value = unsafe { CStr::from_ptr(name) };
    trace_android_import_lookup("device", device, value);
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
            let address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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

// MoltenVK does not expose Android's WSI surface object on macOS. Still
// provide the entry point so engines that resolve it unconditionally receive
// a normal Vulkan capability error instead of branching through a null PFN.
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
    if ready() {
        let value = unsafe { CStr::from_ptr(name) };
        trace_android_import_lookup("instance", instance, value);
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
        return unsafe { raw_instance_symbol(instance, CStr::from_ptr(name)) };
    }
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

/// The namespace and legacy libdl paths use this narrow interface. Dispatch
/// routing and provider PFNs remain private to this provider owner.
pub(super) unsafe fn symbol_lookup(instance: *mut c_void, name: *const c_char) -> *mut c_void {
    unsafe { vulkan_get_instance_proc_addr(instance, name) }
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
