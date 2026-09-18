//! Android Vulkan memory/image import and Metal texture ownership.
use crate::darwin_art_android_platform_symbol;
use crate::vulkan_backend::{moltenvk, provider_exported_symbol, raw_device_symbol};
use crate::vulkan_device_owner;
use crate::vulkan_types::{
    AHardwareBufferDesc, VulkanAndroidHardwareBufferFormatProperties,
    VulkanAndroidHardwareBufferProperties, VulkanBaseInStructure, VulkanBaseOutStructure,
    VulkanBindImageMemoryInfo, VulkanExternalImageFormatProperties,
    VulkanExternalMemoryImageCreateInfo, VulkanGetDeviceProcAddr, VulkanImageCreateInfo,
    VulkanImportAndroidHardwareBufferInfo, VulkanImportMemoryMetalHandleInfo,
    VulkanMemoryAllocateInfo, VulkanMemoryDedicatedAllocateInfo, VulkanMemoryMetalHandleProperties,
    VulkanPhysicalDeviceExternalImageFormatInfo, VulkanPhysicalDeviceImageFormatInfo,
    VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT,
    VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID,
    VK_EXTERNAL_MEMORY_HANDLE_TYPE_MTLTEXTURE_BIT_EXT, VK_FORMAT_FEATURE_RGBA_RENDERABLE,
    VK_FORMAT_R8G8B8A8_UNORM, VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID,
    VK_STRUCTURE_TYPE_EXTERNAL_IMAGE_FORMAT_PROPERTIES,
    VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
    VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID,
    VK_STRUCTURE_TYPE_IMPORT_MEMORY_METAL_HANDLE_INFO_EXT,
    VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
    VK_STRUCTURE_TYPE_MEMORY_METAL_HANDLE_PROPERTIES_EXT,
    VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO, VK_SUCCESS,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy)]
struct ImportedAndroidMemory {
    metal_texture: usize,
    hardware_buffer: usize,
}

static IMPORTED_ANDROID_MEMORY: OnceLock<Mutex<HashMap<usize, ImportedAndroidMemory>>> =
    OnceLock::new();
static VULKAN_IMAGE_FORMATS: OnceLock<Mutex<HashMap<usize, i32>>> = OnceLock::new();
const VK_ERROR_FORMAT_NOT_SUPPORTED: i32 = -11;

pub(super) fn forget_image_format(image: usize) {
    if let Some(images) = VULKAN_IMAGE_FORMATS.get() {
        images.lock().unwrap().remove(&image);
    }
}

pub(super) unsafe extern "C" fn moltenvk_get_android_hardware_buffer_properties(
    device: *mut c_void,
    buffer: *const c_void,
    properties: *mut VulkanAndroidHardwareBufferProperties,
) -> i32 {
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: pid={} get-AHB-properties entry device={device:p} buffer={buffer:p} output={properties:p}",
            std::process::id()
        );
    }
    if device.is_null() || buffer.is_null() || properties.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let Some(_provider) = moltenvk() else {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    };
    let Some(owner) = vulkan_device_owner::snapshot(device) else {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    };
    let physical_device = owner.physical_device;
    let metal_device = owner.metal_device;

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

    if metal_device.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }

    let metal_texture = unsafe { create_texture(buffer.cast_mut(), metal_device) };
    if metal_texture.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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
            "ART Android Vulkan: pid={} AHardwareBuffer properties {}x{} stride={} allocation={} memory-types={:#x}",
            std::process::id(),
            description.width,
            description.height,
            description.stride,
            unsafe { (*properties).allocation_size },
            metal_properties.memory_type_bits
        );
    }
    VK_SUCCESS
}

pub(super) unsafe extern "C" fn moltenvk_get_physical_device_image_format_properties(
    physical_device: *mut c_void,
    image_format_info: *const VulkanPhysicalDeviceImageFormatInfo,
    image_format_properties: *mut VulkanBaseOutStructure,
) -> i32 {
    type GetProperties = unsafe extern "C" fn(
        *mut c_void,
        *const VulkanPhysicalDeviceImageFormatInfo,
        *mut VulkanBaseOutStructure,
    ) -> i32;
    let address = unsafe { provider_exported_symbol(c"vkGetPhysicalDeviceImageFormatProperties2") };
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
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: pid={} image-format-query physical={physical_device:p} format={} type={} usage={:#x} AHB={} result={result}",
            std::process::id(), unsafe { (*image_format_info).format },
            unsafe { (*image_format_info).image_type }, unsafe { (*image_format_info).usage },
            !translated_input.is_null()
        );
    }
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

pub(super) unsafe extern "C" fn moltenvk_create_image(
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
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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
        eprintln!("ART Android Vulkan: pid={} create-image result={result} image={:p} format={} size={:?} usage={:#x}", std::process::id(), if image.is_null() { ptr::null_mut() } else { unsafe { *image } }, unsafe { (*create_info).format }, unsafe { (*create_info).extent }, unsafe { (*create_info).usage });
    }
    if !translated.is_null() {
        unsafe { (*translated).handle_types = original_handle_types };
    }
    result
}

pub(super) unsafe extern "C" fn moltenvk_destroy_image(
    device: *mut c_void,
    image: *mut c_void,
    allocator: *const c_void,
) {
    if let Some(images) = VULKAN_IMAGE_FORMATS.get() {
        images.lock().unwrap().remove(&(image as usize));
    }
    let address = unsafe { raw_device_symbol(device as usize, c"vkDestroyImage") };
    if !address.is_null() {
        let destroy: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void) =
            unsafe { std::mem::transmute(address) };
        unsafe { destroy(device, image, allocator) };
    }
}

pub(super) unsafe extern "C" fn moltenvk_create_image_view(
    device: *mut c_void,
    info: *const c_void,
    allocator: *const c_void,
    output: *mut *mut c_void,
) -> i32 {
    let address = unsafe { raw_device_symbol(device as usize, c"vkCreateImageView") };
    if address.is_null() || info.is_null() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        let words = unsafe { std::slice::from_raw_parts(info.cast::<u64>(), 10) };
        eprintln!(
            "ART Android Vulkan: pid={} create-image-view info={info:p} words={words:x?}",
            std::process::id()
        );
    }
    let create: unsafe extern "C" fn(
        *mut c_void,
        *const c_void,
        *const c_void,
        *mut *mut c_void,
    ) -> i32 = unsafe { std::mem::transmute(address) };
    unsafe { create(device, info, allocator, output) }
}

pub(super) unsafe extern "C" fn moltenvk_allocate_memory(
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
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: pid={} import-AHB-memory entry device={device:p} buffer={hardware_buffer:p} dedicated-image={:p}",
            std::process::id(), dedicated.map_or(ptr::null_mut(), |info| info.image)
        );
    }
    let Some(owner) = vulkan_device_owner::snapshot(device) else {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    };
    let metal_device = owner.metal_device;
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
        let set_texture_address = unsafe { provider_exported_symbol(c"vkSetMTLTextureMVK") };
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
            let get_texture_address = unsafe { provider_exported_symbol(c"vkGetMTLTextureMVK") };
            let mut bound_texture = ptr::null_mut();
            if !get_texture_address.is_null() && !dedicated_copy.image.is_null() {
                let get_texture: GetMetalTexture =
                    unsafe { std::mem::transmute(get_texture_address) };
                unsafe { get_texture(dedicated_copy.image, &mut bound_texture) };
            }
            eprintln!(
                "ART Android Vulkan: pid={} imported AHardwareBuffer={hardware_buffer:p} MetalTexture={metal_texture:p} memory={:p} dedicated-image={:p} image-texture={bound_texture:p} match={}",
                std::process::id(),
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

pub(super) unsafe extern "C" fn moltenvk_free_memory(
    device: *mut c_void,
    memory: *mut c_void,
    allocator: *const c_void,
) {
    type FreeMemory = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void);
    type ReleaseObject = unsafe extern "C" fn(*mut c_void);
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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
    let address = unsafe { provider_exported_symbol(c"vkGetMTLTextureMVK") };
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
            "ART Android Vulkan: pid={} bound image={image:p} memory={memory:p} imported-texture={imported_texture:p} bound-texture={bound_texture:p} match={}",
            std::process::id(),
            imported_texture == bound_texture
        ),
        None => eprintln!(
            "ART Android Vulkan: pid={} bound image={image:p} memory={memory:p} imported-texture=none bound-texture={bound_texture:p}",
            std::process::id()
        ),
    }
}

pub(super) unsafe extern "C" fn moltenvk_bind_image_memory(
    device: *mut c_void,
    image: *mut c_void,
    memory: *mut c_void,
    memory_offset: u64,
) -> i32 {
    type Bind = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, u64) -> i32;
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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

pub(super) unsafe extern "C" fn moltenvk_bind_image_memory2(
    device: *mut c_void,
    bind_info_count: u32,
    bind_infos: *const VulkanBindImageMemoryInfo,
) -> i32 {
    type Bind = unsafe extern "C" fn(*mut c_void, u32, *const VulkanBindImageMemoryInfo) -> i32;
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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
