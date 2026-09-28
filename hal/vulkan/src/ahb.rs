//! `VK_ANDROID_external_memory_android_hardware_buffer`. An AHardwareBuffer
//! is one of our gralloc buffers (docs/graphics-buffers.md), so importing it
//! maps its memfd:
//!
//! - into an image (a dedicated allocation, which the extension requires
//!   for images): the mapping becomes the image's storage, as for swapchain
//!   images, and the memory object itself holds nothing;
//! - into a buffer (a `BLOB` AHardwareBuffer): the mapping is imported as
//!   host memory (`VK_EXT_external_memory_host`).
//!
//! Exporting allocates an AHardwareBuffer that fits the image or buffer and
//! imports it. A buffer's external format is its AHardwareBuffer format;
//! 3-byte RGB has no Vulkan format and is refused as an invalid handle.
//!
//! YUV buffers are multi-planar images, which MoltenVK makes of one Metal
//! texture per plane. Their host images are linear, and their memory is the
//! mapping imported as host memory, over which MoltenVK makes the planes'
//! textures in its own layout of a linear image: the allocator lays YUV
//! buffers out the same way (16-byte rows, planes one after the other), and
//! an import checks that they agree.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::buffers;
use crate::driver;
use crate::native_buffer::images;
use crate::thunks::host;
use crate::types::*;

#[link(name = "nativewindow")]
unsafe extern "C" {
    fn AHardwareBuffer_allocate(desc: *const AHardwareBufferDesc, out: *mut *mut c_void) -> i32;
    fn AHardwareBuffer_acquire(buffer: *mut c_void);
    fn AHardwareBuffer_release(buffer: *mut c_void);
    fn AHardwareBuffer_getNativeHandle(
        buffer: *const c_void,
    ) -> *const aim_gralloc::handle::NativeHandle;
}

/// `AHARDWAREBUFFER_FORMAT_*` (the `PixelFormat` values) and their Vulkan
/// formats; `BLOB` has none.
const FORMATS: &[(u32, i32)] = &[
    (1, 37),                                     // R8G8B8A8_UNORM
    (2, 37),                                     // R8G8B8X8_UNORM: alpha swizzled to one
    (4, 4),                                      // R5G6B5_UNORM: VK_FORMAT_R5G6B5_UNORM_PACK16
    (5, 44),                                     // B8G8R8A8_UNORM
    (0x16, 97),                                  // R16G16B16A16_FLOAT
    (0x2b, 64),                                  // R10G10B10A2_UNORM: A2B10G10R10_UNORM_PACK32
    (0x38, 9),                                   // R8_UNORM
    (0x23, G8_B8R8_2PLANE_420),                  // Y8Cb8Cr8_420 (NV12)
    (0x11, G8_B8R8_2PLANE_420),                  // YCrCb_420_SP (NV21): Cb and Cr swizzled
    (0x3231_5659, G8_B8_R8_3PLANE_420),          // YV12: Cb and Cr swizzled
    (0x36, G10X6_B10X6R10X6_2PLANE_420_3PACK16), // YCbCr_P010
];
const FORMAT_BLOB: u32 = 0x21;
const FORMAT_RGBX: u32 = 2;
/// Formats whose chroma planes (or pairs) hold Cr before Cb.
const CR_FIRST: &[u32] = &[0x11, 0x3231_5659];
const G8_B8_R8_3PLANE_420: i32 = 1000156002;
const G8_B8R8_2PLANE_420: i32 = 1000156003;
const G10X6_B10X6R10X6_2PLANE_420_3PACK16: i32 = 1000156013;
// VkComponentSwizzle.
const SWIZZLE_IDENTITY: i32 = 0;
const SWIZZLE_ONE: i32 = 2;
const SWIZZLE_R: i32 = 3;
const SWIZZLE_B: i32 = 5;
/// `VK_FORMAT_R8G8B8A8_SRGB`: exported as an RGBA_8888 buffer.
const VK_FORMAT_R8G8B8A8_SRGB: i32 = 43;

// AHARDWAREBUFFER_USAGE_* bits.
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
const USAGE_GPU_FRAMEBUFFER: u64 = 1 << 9;
const USAGE_GPU_DATA_BUFFER: u64 = 1 << 24;
const USAGE_GPU_CUBE_MAP: u64 = 1 << 25;
const USAGE_GPU_MIPMAP_COMPLETE: u64 = 1 << 26;

fn vk_format(ahb: u32) -> Option<i32> {
    FORMATS.iter().find(|f| f.0 == ahb).map(|f| f.1)
}

/// The Vulkan format of an external format (`VkExternalFormatANDROID`),
/// which is an AHardwareBuffer format.
pub fn external_format(external: u64) -> Option<i32> {
    u32::try_from(external).ok().and_then(vk_format)
}

/// A multi-planar (YUV) format.
pub fn is_yuv(vk: i32) -> bool {
    matches!(
        vk,
        G8_B8_R8_3PLANE_420 | G8_B8R8_2PLANE_420 | G10X6_B10X6R10X6_2PLANE_420_3PACK16
    )
}

fn ahb_format(vk: i32) -> Option<u32> {
    let vk = if vk == VK_FORMAT_R8G8B8A8_SRGB {
        37
    } else {
        vk
    };
    FORMATS
        .iter()
        .find(|f| f.1 == vk && f.0 != FORMAT_RGBX)
        .map(|f| f.0)
}

/// AHardwareBuffer usage for an image's Vulkan usage and flags.
fn ahb_usage(usage: u32, flags: u32, mip_levels: u32) -> u64 {
    let mut u = 0;
    // Sampled, input attachment, transfer source.
    if usage & (0x04 | 0x80 | 0x01) != 0 {
        u |= USAGE_GPU_SAMPLED_IMAGE;
    }
    // Color attachment, storage, transfer destination.
    if usage & (0x10 | 0x08 | 0x02) != 0 {
        u |= USAGE_GPU_FRAMEBUFFER;
    }
    if flags & 0x10 != 0 {
        u |= USAGE_GPU_CUBE_MAP;
    }
    if mip_levels > 1 {
        u |= USAGE_GPU_MIPMAP_COMPLETE;
    }
    u
}

/// Memory types whose property flags satisfy `pick`.
fn memory_types(physical: VkDispatch, pick: impl Fn(u32) -> bool) -> u32 {
    // SAFETY: an output structure the host fills.
    let props = unsafe {
        let mut p: VkPhysicalDeviceMemoryProperties = std::mem::zeroed();
        host::vkGetPhysicalDeviceMemoryProperties(physical, (&raw mut p).cast());
        p
    };
    (0..props.memoryTypeCount)
        .filter(|&i| pick(props.memoryTypes[i as usize].propertyFlags))
        .fold(0, |bits, i| bits | 1 << i)
}

/// Images live in device memory the driver never allocates (the buffer's
/// mapping is their storage); buffers in host-visible memory, which the
/// mapping is imported as.
fn memory_type_bits(physical: VkDispatch, blob: bool) -> u32 {
    if blob {
        memory_types(physical, |f| f & MEMORY_HOST_VISIBLE != 0)
    } else {
        memory_types(physical, |f| {
            f & MEMORY_DEVICE_LOCAL != 0 && f & (MEMORY_HOST_VISIBLE | MEMORY_LAZILY_ALLOCATED) == 0
        })
    }
}

pub unsafe extern "C" fn vkGetAndroidHardwareBufferPropertiesANDROID(
    device: VkDispatch,
    buffer: *const c_void,
    props: *mut VkAndroidHardwareBufferPropertiesANDROID,
) -> VkResult {
    let Some(physical) = driver::physical_device(device) else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the buffer's own handle.
    let Some((_, h)) =
        (unsafe { aim_gralloc::handle::parse(AHardwareBuffer_getNativeHandle(buffer)) })
    else {
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    };
    // The allocator's resolved format.
    let ahb = h.format as u32;
    let blob = ahb == FORMAT_BLOB;
    let format = vk_format(ahb);
    if format.is_none() && !blob {
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    }
    let format = format.unwrap_or(0);
    let yuv = is_yuv(format);
    // SAFETY: the application's output structure and its chain.
    unsafe {
        (*props).allocationSize = h.metadata_offset;
        (*props).memoryTypeBits = memory_type_bits(physical, blob || yuv);
        let mut features = VkFormatProperties::default();
        if format != 0 {
            host::vkGetPhysicalDeviceFormatProperties(physical, format, (&raw mut features).cast());
        }
        // YUV images are linear on the host.
        let features = if yuv {
            features.linearTilingFeatures
        } else {
            features.optimalTilingFeatures
        };
        let mut suggested = Suggested {
            format,
            external: ahb as u64,
            components: [SWIZZLE_IDENTITY; 4],
            // RGB identity, full range.
            model: 0,
            range: 0,
            // The allocator's CHROMA_SITING of YUV: SITED_INTERSTITIAL.
            chroma: if yuv { MIDPOINT } else { 0 },
        };
        if ahb == FORMAT_RGBX {
            suggested.components[3] = SWIZZLE_ONE;
        }
        if CR_FIRST.contains(&ahb) {
            suggested.components[0] = SWIZZLE_B;
            suggested.components[2] = SWIZZLE_R;
        }
        if yuv {
            // BT.601, narrow range: what video and cameras produce unless
            // a dataspace says otherwise.
            (suggested.model, suggested.range) = (YCBCR_601, RANGE_ITU_NARROW);
        }
        if let Some(f) = find::<VkAndroidHardwareBufferFormatPropertiesANDROID<u32>>(
            (*props).pNext,
            stype::ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID,
        ) {
            suggested.fill(&mut *f, features);
        }
        if let Some(f) = find::<VkAndroidHardwareBufferFormatPropertiesANDROID<u64>>(
            (*props).pNext,
            stype::ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_2_ANDROID,
        ) {
            suggested.fill(&mut *f, features as u64);
        }
    }
    VK_SUCCESS
}

// VkSamplerYcbcrModelConversion, VkSamplerYcbcrRange, VkChromaLocation.
const YCBCR_601: i32 = 3;
const RANGE_ITU_NARROW: i32 = 1;
const MIDPOINT: i32 = 1;

/// What `VkAndroidHardwareBufferFormatProperties(2)ANDROID` reports.
struct Suggested {
    format: i32,
    external: u64,
    components: [i32; 4],
    model: i32,
    range: i32,
    chroma: i32,
}

impl Suggested {
    fn fill<F>(&self, f: &mut VkAndroidHardwareBufferFormatPropertiesANDROID<F>, features: F) {
        f.format = self.format;
        f.externalFormat = self.external;
        f.formatFeatures = features;
        f.samplerYcbcrConversionComponents = self.components;
        f.suggestedYcbcrModel = self.model;
        f.suggestedYcbcrRange = self.range;
        f.suggestedXChromaOffset = self.chroma;
        f.suggestedYChromaOffset = self.chroma;
    }
}

/// An allocation backed by an AHardwareBuffer.
struct Memory {
    ahb: *mut c_void,
    buffer: u64,
}

// SAFETY: an AHardwareBuffer reference is usable from any thread.
unsafe impl Send for Memory {}

fn memories() -> MutexGuard<'static, HashMap<u64, Memory>> {
    static MEMORIES: OnceLock<Mutex<HashMap<u64, Memory>>> = OnceLock::new();
    MEMORIES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Allocate an AHardwareBuffer for exported memory: one that fits the
/// dedicated image, or a `BLOB` of `size` bytes.
fn allocate(image: u64, size: u64) -> Result<*mut c_void, VkResult> {
    let desc = if image != 0 {
        let Some(i) = images().get(&image).copied() else {
            return Err(VK_ERROR_INVALID_EXTERNAL_HANDLE);
        };
        let Some(format) = ahb_format(i.format) else {
            return Err(VK_ERROR_FORMAT_NOT_SUPPORTED);
        };
        AHardwareBufferDesc {
            width: i.extent.width,
            height: i.extent.height,
            layers: i.layers,
            format,
            usage: ahb_usage(i.usage, i.flags, 1),
            ..Default::default()
        }
    } else {
        AHardwareBufferDesc {
            width: size as u32,
            height: 1,
            layers: 1,
            format: FORMAT_BLOB,
            usage: USAGE_GPU_DATA_BUFFER,
            ..Default::default()
        }
    };
    let mut out = std::ptr::null_mut();
    // SAFETY: a valid description and out pointer.
    if unsafe { AHardwareBuffer_allocate(&desc, &mut out) } != 0 {
        return Err(VK_ERROR_OUT_OF_HOST_MEMORY);
    }
    Ok(out)
}

pub unsafe extern "C" fn vkAllocateMemory(
    device: VkDispatch,
    info: *const VkMemoryAllocateInfo,
    _allocator: *const c_void,
    memory: *mut u64,
) -> VkResult {
    // SAFETY: the application's allocate info and its chain.
    let mut info = unsafe { *info };
    let (import, export, dedicated) = unsafe {
        (
            find::<VkImportAndroidHardwareBufferInfoANDROID>(
                info.pNext,
                stype::IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID,
            )
            .map(|i| (*i).buffer),
            find::<VkExternalMemoryCreateInfo>(info.pNext, stype::EXPORT_MEMORY_ALLOCATE_INFO)
                .is_some_and(|e| (*e).handleTypes & HANDLE_TYPE_ANDROID_HARDWARE_BUFFER != 0),
            find::<VkMemoryDedicatedAllocateInfo>(
                info.pNext,
                stype::MEMORY_DEDICATED_ALLOCATE_INFO,
            )
            .map(|d| ((*d).image, (*d).buffer)),
        )
    };
    if import.is_none() && !export {
        // SAFETY: the application's arguments.
        return unsafe {
            host::vkAllocateMemory(
                device,
                (&raw const info).cast(),
                std::ptr::null(),
                memory.cast(),
            )
        };
    }
    let Some(physical) = driver::physical_device(device) else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let image = dedicated.map_or(0, |d| d.0);
    let format = images().get(&image).map(|i| i.format);
    // A YUV image's memory is the mapping as host memory, as a buffer's;
    // MoltenVK makes its planes' textures only over memory it was not
    // told is dedicated.
    let yuv = format.is_some_and(is_yuv);
    let ahb = match import {
        Some(b) => {
            // SAFETY: the application's buffer; the memory holds a
            // reference for its lifetime.
            unsafe { AHardwareBuffer_acquire(b) };
            b
        }
        None => match allocate(image, info.allocationSize) {
            Ok(b) => b,
            Err(r) => return r,
        },
    };
    let release = |r| {
        // SAFETY: the reference taken above.
        unsafe { AHardwareBuffer_release(ahb) };
        r
    };
    // SAFETY: the buffer's own handle.
    let Some(m) = (unsafe { buffers::map(AHardwareBuffer_getNativeHandle(ahb)) }) else {
        return release(VK_ERROR_INVALID_EXTERNAL_HANDLE);
    };
    // The host sees only what it knows: the dedicated image, or the
    // mapping as host memory.
    // SAFETY: the chain is the caller's for this call.
    let unlinked = unsafe {
        Unlinked::new(&mut info.pNext, |s| match (*s).sType {
            stype::IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID
            | stype::EXPORT_MEMORY_ALLOCATE_INFO => true,
            stype::MEMORY_DEDICATED_ALLOCATE_INFO => yuv,
            _ => false,
        })
    };
    let host_pointer = VkImportMemoryHostPointerInfoEXT {
        sType: stype::IMPORT_MEMORY_HOST_POINTER_INFO_EXT,
        pNext: info.pNext,
        handleType: HANDLE_TYPE_HOST_ALLOCATION,
        pHostPointer: m.address as *mut c_void,
    };
    if image != 0 && !yuv {
        let r = match format {
            Some(f) => buffers::attach(physical, image, &m, f),
            None => VK_ERROR_INVALID_EXTERNAL_HANDLE,
        };
        if r != VK_SUCCESS {
            buffers::unmap(m.id);
            return release(r);
        }
    } else {
        if yuv && !planes_agree(device, image, &m) {
            buffers::unmap(m.id);
            return release(VK_ERROR_INVALID_EXTERNAL_HANDLE);
        }
        info.pNext = (&raw const host_pointer).cast();
        info.allocationSize = m.length as u64;
    }
    // SAFETY: a valid allocate info.
    let r = unsafe {
        host::vkAllocateMemory(
            device,
            (&raw const info).cast(),
            std::ptr::null(),
            memory.cast(),
        )
    };
    drop(unlinked);
    if r != VK_SUCCESS {
        buffers::unmap(m.id);
        return release(r);
    }
    // SAFETY: set by the successful call.
    memories().insert(unsafe { *memory }, Memory { ahb, buffer: m.id });
    VK_SUCCESS
}

/// Whether MoltenVK's layout of the linear YUV `image` is the buffer's.
fn planes_agree(device: VkDispatch, image: u64, m: &buffers::Mapped) -> bool {
    let Some(layout) = m.handle.layout() else {
        return false;
    };
    layout.planes.iter().enumerate().all(|(i, plane)| {
        let subresource = VkImageSubresource {
            aspectMask: ASPECT_PLANE_0 << i,
            mipLevel: 0,
            arrayLayer: 0,
        };
        let mut host = VkSubresourceLayout::default();
        // SAFETY: the application's image, one of its planes.
        unsafe {
            host::vkGetImageSubresourceLayout(
                device,
                image,
                (&raw const subresource).cast(),
                (&raw mut host).cast(),
            )
        };
        (host.offset, host.rowPitch) == (plane.offset, plane.stride_bytes)
    })
}

pub unsafe extern "C" fn vkFreeMemory(device: VkDispatch, memory: u64, _allocator: *const c_void) {
    // SAFETY: the application's memory.
    unsafe { host::vkFreeMemory(device, memory, std::ptr::null()) };
    if let Some(m) = memories().remove(&memory) {
        buffers::unmap(m.buffer);
        // SAFETY: the memory's reference.
        unsafe { AHardwareBuffer_release(m.ahb) };
    }
}

pub unsafe extern "C" fn vkGetMemoryAndroidHardwareBufferANDROID(
    _device: VkDispatch,
    info: *const VkMemoryGetAndroidHardwareBufferInfoANDROID,
    buffer: *mut *mut c_void,
) -> VkResult {
    // SAFETY: the application's arguments.
    let memory = unsafe { (*info).memory };
    match memories().get(&memory) {
        Some(m) => {
            // SAFETY: a reference for the caller, and its out pointer.
            unsafe {
                AHardwareBuffer_acquire(m.ahb);
                *buffer = m.ahb;
            }
            VK_SUCCESS
        }
        None => VK_ERROR_INVALID_EXTERNAL_HANDLE,
    }
}

pub unsafe extern "C" fn vkCreateBuffer(
    device: VkDispatch,
    info: *const VkBaseOutStructure,
    _allocator: *const c_void,
    buffer: *mut u64,
) -> VkResult {
    // The buffer's memory is host memory to MoltenVK; an external info
    // naming an AHardwareBuffer is the driver's.
    // SAFETY: the application's create info; its chain is ours for the
    // call.
    let _unlinked = unsafe {
        Unlinked::new((&raw mut (*info.cast_mut()).pNext).cast(), |s| {
            (*s).sType == stype::EXTERNAL_MEMORY_BUFFER_CREATE_INFO
                && (*(s as *const VkExternalMemoryCreateInfo)).handleTypes
                    & HANDLE_TYPE_ANDROID_HARDWARE_BUFFER
                    != 0
        })
    };
    // SAFETY: a valid create info.
    unsafe { host::vkCreateBuffer(device, info.cast(), std::ptr::null(), buffer.cast()) }
}

pub unsafe extern "C" fn vkGetPhysicalDeviceImageFormatProperties2(
    physical: VkDispatch,
    info: *const VkPhysicalDeviceImageFormatInfo2,
    props: *mut VkBaseOutStructure,
) -> VkResult {
    // SAFETY: the application's query and its chain.
    let external = unsafe {
        find::<VkPhysicalDeviceExternalInfo>(
            (*info).pNext,
            stype::PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO,
        )
    };
    let ahb =
        external.is_some_and(|e| unsafe { (*e).handleType } == HANDLE_TYPE_ANDROID_HARDWARE_BUFFER);
    if !ahb {
        // SAFETY: the application's arguments.
        return unsafe {
            host::vkGetPhysicalDeviceImageFormatProperties2(physical, info.cast(), props.cast())
        };
    }
    // SAFETY: as above.
    let mut query = unsafe { *info };
    let (format, usage, flags) = (query.format, query.usage, query.flags);
    if ahb_format(format).is_none() {
        return VK_ERROR_FORMAT_NOT_SUPPORTED;
    }
    // YUV images are linear on the host.
    if is_yuv(format) {
        query.tiling = IMAGE_TILING_LINEAR;
    }
    // SAFETY: the chains are the caller's for the call.
    let r = unsafe {
        let _unlinked = Unlinked::new(&mut query.pNext, |s| {
            (*s).sType == stype::PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO
        });
        host::vkGetPhysicalDeviceImageFormatProperties2(
            physical,
            (&raw const query).cast(),
            props.cast(),
        )
    };
    if r != VK_SUCCESS {
        return r;
    }
    // SAFETY: the application's output chain.
    unsafe {
        let next = (*props).pNext as *const c_void;
        if let Some(e) =
            find::<VkExternalImageFormatProperties>(next, stype::EXTERNAL_IMAGE_FORMAT_PROPERTIES)
        {
            (*e).externalMemoryProperties = VkExternalMemoryProperties {
                externalMemoryFeatures: EXTERNAL_DEDICATED_ONLY
                    | EXTERNAL_EXPORTABLE
                    | EXTERNAL_IMPORTABLE,
                exportFromImportedHandleTypes: HANDLE_TYPE_ANDROID_HARDWARE_BUFFER,
                compatibleHandleTypes: HANDLE_TYPE_ANDROID_HARDWARE_BUFFER,
            };
        }
        if let Some(u) = find::<VkAndroidHardwareBufferUsageANDROID>(
            next,
            stype::ANDROID_HARDWARE_BUFFER_USAGE_ANDROID,
        ) {
            (*u).androidHardwareBufferUsage = ahb_usage(usage, flags, 1);
        }
    }
    VK_SUCCESS
}

pub unsafe extern "C" fn vkGetPhysicalDeviceExternalBufferProperties(
    physical: VkDispatch,
    info: *const VkPhysicalDeviceExternalBufferInfo,
    props: *mut VkExternalBufferProperties,
) {
    // SAFETY: the application's arguments.
    unsafe {
        if (*info).handleType != HANDLE_TYPE_ANDROID_HARDWARE_BUFFER {
            return host::vkGetPhysicalDeviceExternalBufferProperties(
                physical,
                info.cast(),
                props.cast(),
            );
        }
        (*props).externalMemoryProperties = VkExternalMemoryProperties {
            externalMemoryFeatures: EXTERNAL_EXPORTABLE | EXTERNAL_IMPORTABLE,
            exportFromImportedHandleTypes: HANDLE_TYPE_ANDROID_HARDWARE_BUFFER,
            compatibleHandleTypes: HANDLE_TYPE_ANDROID_HARDWARE_BUFFER,
        };
    }
}

pub unsafe extern "C" fn vkCreateImageView(
    device: VkDispatch,
    info: *const VkImageViewCreateInfo,
    _allocator: *const c_void,
    view: *mut u64,
) -> VkResult {
    // SAFETY: the application's create info.
    let mut info = unsafe { *info };
    // A view of an image with an external format has none of its own.
    if info.format == 0 {
        match images().get(&info.image) {
            Some(i) => info.format = i.format,
            None => return VK_ERROR_FORMAT_NOT_SUPPORTED,
        }
    }
    // SAFETY: a valid create info.
    unsafe {
        host::vkCreateImageView(
            device,
            (&raw const info).cast(),
            std::ptr::null(),
            view.cast(),
        )
    }
}

pub unsafe extern "C" fn vkCreateSamplerYcbcrConversion(
    device: VkDispatch,
    info: *const VkSamplerYcbcrConversionCreateInfo,
    _allocator: *const c_void,
    conversion: *mut u64,
) -> VkResult {
    // SAFETY: the application's create info and its chain.
    let mut info = unsafe { *info };
    let external = unsafe {
        find::<VkExternalFormatANDROID>(info.pNext, stype::EXTERNAL_FORMAT_ANDROID)
            .map(|e| (*e).externalFormat)
    };
    if let Some(e) = external.filter(|&e| e != 0) {
        match external_format(e) {
            Some(f) => info.format = f,
            None => return VK_ERROR_FORMAT_NOT_SUPPORTED,
        }
    }
    // SAFETY: the chain is the caller's for this call.
    let _unlinked = unsafe {
        Unlinked::new(&mut info.pNext, |s| {
            (*s).sType == stype::EXTERNAL_FORMAT_ANDROID
        })
    };
    // SAFETY: a valid create info.
    unsafe {
        host::vkCreateSamplerYcbcrConversion(
            device,
            (&raw const info).cast(),
            std::ptr::null(),
            conversion.cast(),
        )
    }
}
