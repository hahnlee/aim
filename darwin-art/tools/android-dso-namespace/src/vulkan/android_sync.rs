//! Android Vulkan semaphore and sync-fd ownership.
use crate::darwin_art_android_platform_symbol;
use crate::vulkan_backend::provider_exported_symbol;
use crate::vulkan_device_owner;
use crate::vulkan_sync_fd_import::import_consuming_sync_fd;
use crate::vulkan_types::{
    VulkanBaseInStructure, VulkanExternalSemaphoreProperties, VulkanGetDeviceProcAddr,
    VulkanImportMetalSharedEventInfo, VulkanImportSemaphoreFdInfo,
    VulkanPhysicalDeviceExternalSemaphoreInfo, VulkanSemaphoreCreateInfo, VulkanSemaphoreGetFdInfo,
    VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT, VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT,
    VK_STRUCTURE_TYPE_IMPORT_METAL_SHARED_EVENT_INFO_EXT,
};
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy)]
struct VulkanSemaphoreState {
    metal_shared_event: usize,
    next_value: u64,
}

static VULKAN_DEVICE_SYNC_FD: OnceLock<Mutex<HashMap<usize, bool>>> = OnceLock::new();
static VULKAN_SEMAPHORES: OnceLock<Mutex<HashMap<usize, VulkanSemaphoreState>>> = OnceLock::new();
const VK_SUCCESS: i32 = 0;
const VK_ERROR_FORMAT_NOT_SUPPORTED: i32 = -11;
const VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT: u32 = 0x0000_0010;

#[cfg(not(test))]
unsafe extern "C" {
    fn darwin_art_bionic_socket_broker_dup(fd: c_int) -> c_int;
    fn darwin_art_bionic_socket_broker_close(fd: c_int) -> c_int;
}

#[cfg(test)]
unsafe fn darwin_art_bionic_socket_broker_dup(_fd: c_int) -> c_int {
    -1
}

#[cfg(test)]
unsafe fn darwin_art_bionic_socket_broker_close(_fd: c_int) -> c_int {
    0
}

pub(super) fn remember_device_sync_fd(device: usize, enabled: bool) {
    VULKAN_DEVICE_SYNC_FD
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .insert(device, enabled);
}

pub(super) fn forget_device_sync_fd(device: usize) {
    if let Some(devices) = VULKAN_DEVICE_SYNC_FD.get() {
        devices.lock().unwrap().remove(&device);
    }
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

pub(super) unsafe extern "C" fn moltenvk_get_physical_device_external_semaphore_properties(
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
                "ART Android Vulkan: pid={} external-semaphore-properties sync-fd importable=1 exportable=1",
                std::process::id()
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
        unsafe { provider_exported_symbol(c"vkGetPhysicalDeviceExternalSemaphoreProperties") };
    if !address.is_null() {
        let get_properties: GetProperties = unsafe { std::mem::transmute(address) };
        unsafe { get_properties(physical_device, external_info, properties) };
    }
}

pub(super) unsafe extern "C" fn moltenvk_create_semaphore(
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
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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
        let metal_device = vulkan_device_owner::snapshot(device)
            .map(|owner| owner.metal_device)
            .unwrap_or(ptr::null_mut());
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
                "ART Android Vulkan: pid={} created Metal-shared-event semaphore={:p} event={shared_event:p} value={next_value}",
                std::process::id(),
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

pub(super) unsafe extern "C" fn moltenvk_destroy_semaphore(
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
    let get_device_proc_address = unsafe { provider_exported_symbol(c"vkGetDeviceProcAddr") };
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

pub(super) unsafe fn moltenvk_signal_wsi_acquire(
    device: *mut c_void,
    semaphore: *mut c_void,
) -> i32 {
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
        eprintln!(
            "ART Android Vulkan: pid={} acquire-signaled semaphore={semaphore:p} value={value}",
            std::process::id()
        );
    }
    VK_SUCCESS
}

pub(super) unsafe extern "C" fn moltenvk_import_semaphore_fd(
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
            "ART Android Vulkan: pid={} import sync-fd begin fd={} semaphore={semaphore:p}",
            std::process::id(),
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
    let import_fence: ImportFence = unsafe { std::mem::transmute(import_address) };
    let fd = unsafe { (*import_info).fd };
    let result = import_consuming_sync_fd(
        fd,
        |guest_fd| unsafe { darwin_art_bionic_socket_broker_dup(guest_fd) },
        |imported_fd| unsafe { import_fence(shared_event, signal_value, imported_fd) },
        |guest_fd| unsafe { darwin_art_bionic_socket_broker_close(guest_fd) },
    );
    if result != VK_SUCCESS {
        // The consuming provider returns its own status, not a VkResult.
        // Preserve the wrapper's existing Vulkan error mapping.
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    state.next_value = signal_value.saturating_add(1);
    if std::env::var_os("DARWIN_ART_DEBUG_GRAPHICS_DSO").is_some() {
        eprintln!(
            "ART Android Vulkan: pid={} imported sync-fd={} semaphore={semaphore:p} MetalSharedEvent={shared_event:p} value={signal_value}",
            std::process::id(),
            unsafe { (*import_info).fd }
        );
    }
    VK_SUCCESS
}

pub(super) unsafe extern "C" fn moltenvk_get_semaphore_fd(
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
            "ART Android Vulkan: pid={} exported sync-fd={fd} semaphore={semaphore:p} MetalSharedEvent={shared_event:p} value={signal_value}",
            std::process::id()
        );
    }
    VK_SUCCESS
}
