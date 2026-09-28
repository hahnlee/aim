//! The Vulkan types the driver reads or writes itself (`vulkan_core.h`,
//! `vulkan_android.h`, `vk_android_native_buffer.h`), as they are on arm64
//! Android, and the pNext-chain helpers. Everything else passes through as
//! pointers.
#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

use core::ffi::{c_char, c_void};

/// A dispatchable handle (`VkInstance`, `VkPhysicalDevice`, `VkDevice`,
/// `VkQueue`, `VkCommandBuffer`): MoltenVK's, whose first word is the
/// loader's.
pub type VkDispatch = *mut c_void;
pub type VkResult = i32;

/// An entry point `vkGet*ProcAddr` can hand out.
pub struct Proc {
    pub name: &'static str,
    pub addr: *const c_void,
    /// Host table index, or -1 for the driver's own.
    pub id: i32,
}

// SAFETY: code addresses.
unsafe impl Sync for Proc {}

pub const VK_SUCCESS: VkResult = 0;
pub const VK_INCOMPLETE: VkResult = 5;
pub const VK_ERROR_OUT_OF_HOST_MEMORY: VkResult = -1;
pub const VK_ERROR_INITIALIZATION_FAILED: VkResult = -3;
pub const VK_ERROR_LAYER_NOT_PRESENT: VkResult = -6;
pub const VK_ERROR_FORMAT_NOT_SUPPORTED: VkResult = -11;
pub const VK_ERROR_INVALID_EXTERNAL_HANDLE: VkResult = -1000072003;

pub mod stype {
    pub const SUBMIT_INFO: i32 = 4;
    pub const FENCE_CREATE_INFO: i32 = 8;
    pub const DEBUG_REPORT_CALLBACK_CREATE_INFO_EXT: i32 = 1000011000;
    pub const NATIVE_BUFFER_ANDROID: i32 = 1000010000;
    pub const SWAPCHAIN_IMAGE_CREATE_INFO_ANDROID: i32 = 1000010001;
    pub const PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO: i32 = 1000071000;
    pub const EXTERNAL_IMAGE_FORMAT_PROPERTIES: i32 = 1000071001;
    pub const EXTERNAL_MEMORY_BUFFER_CREATE_INFO: i32 = 1000072000;
    pub const EXTERNAL_MEMORY_IMAGE_CREATE_INFO: i32 = 1000072001;
    pub const EXPORT_MEMORY_ALLOCATE_INFO: i32 = 1000072002;
    pub const EXPORT_SEMAPHORE_CREATE_INFO: i32 = 1000077000;
    pub const MEMORY_DEDICATED_ALLOCATE_INFO: i32 = 1000127001;
    pub const DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT: i32 = 1000128004;
    pub const ANDROID_HARDWARE_BUFFER_USAGE_ANDROID: i32 = 1000129000;
    pub const ANDROID_HARDWARE_BUFFER_PROPERTIES_ANDROID: i32 = 1000129001;
    pub const ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID: i32 = 1000129002;
    pub const IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID: i32 = 1000129003;
    pub const EXTERNAL_FORMAT_ANDROID: i32 = 1000129005;
    pub const ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_2_ANDROID: i32 = 1000129006;
    pub const IMPORT_MEMORY_HOST_POINTER_INFO_EXT: i32 = 1000178000;
    pub const LAYER_SETTINGS_CREATE_INFO_EXT: i32 = 1000496000;
}

/// `VkExternalMemoryHandleTypeFlagBits`.
pub const HANDLE_TYPE_HOST_ALLOCATION: u32 = 0x80;
pub const HANDLE_TYPE_ANDROID_HARDWARE_BUFFER: u32 = 0x400;
/// `VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT`.
pub const SEMAPHORE_HANDLE_TYPE_SYNC_FD: u32 = 0x10;
/// `VkExternalMemoryFeatureFlagBits` (the semaphore ones are 1 and 2 too).
pub const EXTERNAL_DEDICATED_ONLY: u32 = 1;
pub const EXTERNAL_EXPORTABLE: u32 = 2;
pub const EXTERNAL_IMPORTABLE: u32 = 4;
pub const PIPELINE_STAGE_ALL_COMMANDS: u32 = 0x10000;

#[repr(C)]
pub struct VkBaseOutStructure {
    pub sType: i32,
    pub pNext: *mut VkBaseOutStructure,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkExtensionProperties {
    pub extensionName: [c_char; 256],
    pub specVersion: u32,
}

impl VkExtensionProperties {
    pub fn new(name: &str, spec: u32) -> Self {
        let mut p = VkExtensionProperties {
            extensionName: [0; 256],
            specVersion: spec,
        };
        for (d, s) in p.extensionName.iter_mut().zip(name.bytes()) {
            *d = s as c_char;
        }
        p
    }

    pub fn name(&self) -> &[u8] {
        let n = self
            .extensionName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(256);
        // SAFETY: c_char and u8 have the same layout.
        unsafe { core::slice::from_raw_parts(self.extensionName.as_ptr().cast(), n) }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkInstanceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub pApplicationInfo: *const c_void,
    pub enabledLayerCount: u32,
    pub ppEnabledLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub ppEnabledExtensionNames: *const *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkDeviceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueCreateInfoCount: u32,
    pub pQueueCreateInfos: *const c_void,
    pub enabledLayerCount: u32,
    pub ppEnabledLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub ppEnabledExtensionNames: *const *const c_char,
    pub pEnabledFeatures: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkDeviceQueueCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueFamilyIndex: u32,
    pub queueCount: u32,
    pub pQueuePriorities: *const f32,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VkQueueFamilyProperties {
    pub queueFlags: u32,
    pub queueCount: u32,
    pub timestampValidBits: u32,
    pub minImageTransferGranularity: VkExtent3D,
}

#[repr(C)]
pub struct VkQueueFamilyProperties2 {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub queueFamilyProperties: VkQueueFamilyProperties,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VkExtent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkImageCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub imageType: i32,
    pub format: i32,
    pub extent: VkExtent3D,
    pub mipLevels: u32,
    pub arrayLayers: u32,
    pub samples: u32,
    pub tiling: i32,
    pub usage: u32,
    pub sharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub initialLayout: i32,
}

/// `VkNativeBufferANDROID` (spec version 11 layout, which the loader
/// always fills).
#[repr(C)]
pub struct VkNativeBufferANDROID {
    pub sType: i32,
    pub pNext: *const c_void,
    pub handle: *const aim_gralloc::handle::NativeHandle,
    pub stride: i32,
    pub format: i32,
    pub usage: i32,
    pub usage2: [u64; 2],
    pub usage3: u64,
    pub ahb: *mut c_void,
}

#[repr(C)]
pub struct VkExternalMemoryCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub handleTypes: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkMemoryAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub allocationSize: u64,
    pub memoryTypeIndex: u32,
}

#[repr(C)]
pub struct VkImportAndroidHardwareBufferInfoANDROID {
    pub sType: i32,
    pub pNext: *const c_void,
    pub buffer: *mut c_void,
}

#[repr(C)]
pub struct VkMemoryDedicatedAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub image: u64,
    pub buffer: u64,
}

#[repr(C)]
pub struct VkImportMemoryHostPointerInfoEXT {
    pub sType: i32,
    pub pNext: *const c_void,
    pub handleType: u32,
    pub pHostPointer: *mut c_void,
}

#[repr(C)]
pub struct VkAndroidHardwareBufferPropertiesANDROID {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub allocationSize: u64,
    pub memoryTypeBits: u32,
}

/// `VkAndroidHardwareBufferFormatPropertiesANDROID`, and the `2` variant
/// with 64-bit `formatFeatures` (`fmt2`).
#[repr(C)]
pub struct VkAndroidHardwareBufferFormatPropertiesANDROID<F> {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub format: i32,
    pub externalFormat: u64,
    pub formatFeatures: F,
    pub samplerYcbcrConversionComponents: [i32; 4],
    pub suggestedYcbcrModel: i32,
    pub suggestedYcbcrRange: i32,
    pub suggestedXChromaOffset: i32,
    pub suggestedYChromaOffset: i32,
}

#[repr(C)]
pub struct VkMemoryGetAndroidHardwareBufferInfoANDROID {
    pub sType: i32,
    pub pNext: *const c_void,
    pub memory: u64,
}

#[repr(C)]
pub struct VkPhysicalDeviceImageFormatInfo2 {
    pub sType: i32,
    pub pNext: *const c_void,
    pub format: i32,
    pub r#type: i32,
    pub tiling: i32,
    pub usage: u32,
    pub flags: u32,
}

#[repr(C)]
pub struct VkPhysicalDeviceExternalInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub handleType: u32,
}

#[repr(C)]
pub struct VkExternalMemoryProperties {
    pub externalMemoryFeatures: u32,
    pub exportFromImportedHandleTypes: u32,
    pub compatibleHandleTypes: u32,
}

#[repr(C)]
pub struct VkExternalImageFormatProperties {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub externalMemoryProperties: VkExternalMemoryProperties,
}

#[repr(C)]
pub struct VkAndroidHardwareBufferUsageANDROID {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub androidHardwareBufferUsage: u64,
}

#[repr(C)]
pub struct VkPhysicalDeviceExternalBufferInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub usage: u32,
    pub handleType: u32,
}

#[repr(C)]
pub struct VkExternalBufferProperties {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub externalMemoryProperties: VkExternalMemoryProperties,
}

#[repr(C)]
pub struct VkMemoryType {
    pub propertyFlags: u32,
    pub heapIndex: u32,
}

#[repr(C)]
pub struct VkPhysicalDeviceMemoryProperties {
    pub memoryTypeCount: u32,
    pub memoryTypes: [VkMemoryType; 32],
    pub memoryHeapCount: u32,
    pub memoryHeaps: [[u64; 2]; 16],
}

pub const MEMORY_DEVICE_LOCAL: u32 = 1;
pub const MEMORY_HOST_VISIBLE: u32 = 2;
pub const MEMORY_LAZILY_ALLOCATED: u32 = 0x10;

#[repr(C)]
#[derive(Default)]
pub struct VkFormatProperties {
    pub linearTilingFeatures: u32,
    pub optimalTilingFeatures: u32,
    pub bufferFeatures: u32,
}

#[repr(C)]
pub struct VkSemaphoreGetFdInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub semaphore: u64,
    pub handleType: u32,
}

#[repr(C)]
pub struct VkImportSemaphoreFdInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub semaphore: u64,
    pub flags: u32,
    pub handleType: u32,
    pub fd: i32,
}

#[repr(C)]
pub struct VkExternalSemaphoreProperties {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub exportFromImportedHandleTypes: u32,
    pub compatibleHandleTypes: u32,
    pub externalSemaphoreFeatures: u32,
}

#[repr(C)]
pub struct VkSubmitInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const u64,
    pub pWaitDstStageMask: *const u32,
    pub commandBufferCount: u32,
    pub pCommandBuffers: *const VkDispatch,
    pub signalSemaphoreCount: u32,
    pub pSignalSemaphores: *const u64,
}

#[repr(C)]
pub struct VkFenceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkDeviceQueueInfo2 {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueFamilyIndex: u32,
    pub queueIndex: u32,
}

/// `AHardwareBuffer_Desc`.
#[repr(C)]
#[derive(Default)]
pub struct AHardwareBufferDesc {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub format: u32,
    pub usage: u64,
    pub stride: u32,
    pub rfu0: u32,
    pub rfu1: u64,
}

/// The first structure of type `stype` in a pNext chain.
///
/// # Safety
/// `next` must be null or a valid chain of Vulkan structures.
pub unsafe fn find<T>(mut next: *const c_void, stype: i32) -> Option<*mut T> {
    while !next.is_null() {
        // SAFETY: caller contract.
        let s = unsafe { &*(next as *const VkBaseOutStructure) };
        if s.sType == stype {
            return Some(next as *mut T);
        }
        next = s.pNext as *const c_void;
    }
    None
}

/// Structures taken out of the pNext chain of a structure the caller owns
/// for the duration of one host call, put back on drop.
///
/// The head is a copy the driver made; members further down are the
/// application's, so they are relinked in place and restored, as the
/// application's input is `const` and must look unchanged afterwards.
pub struct Unlinked {
    restore: Vec<(*mut *mut VkBaseOutStructure, *mut VkBaseOutStructure)>,
}

impl Unlinked {
    /// Take the structures `drop` selects out of the chain at `*head`.
    ///
    /// # Safety
    /// `head` must point to the pNext field of a live structure whose
    /// chain is valid and not used by anyone else until the guard drops.
    pub unsafe fn new(
        head: *mut *const c_void,
        mut drop: impl FnMut(*mut VkBaseOutStructure) -> bool,
    ) -> Unlinked {
        let mut restore = Vec::new();
        let mut link = head as *mut *mut VkBaseOutStructure;
        // SAFETY: caller contract; every link is a live structure's pNext.
        unsafe {
            while !(*link).is_null() {
                let s = *link;
                if drop(s) {
                    restore.push((link, s));
                    *link = (*s).pNext;
                } else {
                    link = &mut (*s).pNext;
                }
            }
        }
        Unlinked { restore }
    }
}

impl Drop for Unlinked {
    fn drop(&mut self) {
        for &(link, s) in self.restore.iter().rev() {
            // SAFETY: the links recorded in `new`, restored in reverse.
            unsafe { *link = s };
        }
    }
}
