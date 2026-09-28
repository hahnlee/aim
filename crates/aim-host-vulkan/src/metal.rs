//! Graphics buffers as the storage of `VkImage`s (`docs/graphics-buffers.md`,
//! `docs/vulkan-driver.md`).
//!
//! The guest maps the buffer's memfd; the mapping starts on a page, so
//! Metal wraps it without copying (`newBufferWithBytesNoCopy`). A linear
//! texture over that `MTLBuffer`, in the Metal format MoltenVK uses for the
//! image's `VkFormat`, replaces the image's own texture
//! (`vkSetMTLTextureMVK`). MoltenVK keeps the texture, and the texture
//! keeps the buffer, until the image is destroyed.

use std::ffi::{CStr, c_char, c_void};

use aim_hostcall::vulkan::Attach;

use crate::{EINVAL, Private};

type Id = *mut c_void;
type Sel = *const c_void;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_release(obj: Id);
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {}

pub fn pool_push() -> *mut c_void {
    // SAFETY: no preconditions.
    unsafe { objc_autoreleasePoolPush() }
}

pub fn pool_pop(pool: *mut c_void) {
    // SAFETY: `pool` came from `pool_push` on this thread.
    unsafe { objc_autoreleasePoolPop(pool) }
}

fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's C signature.
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { f($obj, sel($sel) $(, $a)*) }
    }};
}

const PAGE: u64 = 16384;
const MTL_STORAGE_MODE_SHARED: usize = 0;
/// Shader read and write, render target, pixel format views (MoltenVK makes
/// views for a format-list or mutable-format image).
const MTL_TEXTURE_USAGE: usize = 0x01 | 0x02 | 0x04 | 0x10;
const MTL_PIXEL_FORMAT_INVALID: usize = 0;

pub fn attach(p: &Private, a: &mut Attach) -> i64 {
    // SAFETY: MoltenVK's exported functions with their C signatures.
    let (pixel_format, bytes_per_block, get_device, set_texture) = unsafe {
        (
            std::mem::transmute::<usize, unsafe extern "C" fn(i32) -> usize>(p.pixel_format),
            std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> u32>(p.bytes_per_block),
            std::mem::transmute::<usize, unsafe extern "C" fn(u64, *mut Id)>(p.get_mtl_device),
            std::mem::transmute::<usize, unsafe extern "C" fn(u64, Id) -> i32>(p.set_mtl_texture),
        )
    };
    // SAFETY: a pure table lookup.
    let format = unsafe { pixel_format(a.format) };
    if format == MTL_PIXEL_FORMAT_INVALID {
        return EINVAL;
    }
    // SAFETY: as above.
    let bpp = unsafe { bytes_per_block(format) } as u64;
    let min_len = a.stride_bytes as u64 * a.height as u64;
    if a.address % PAGE != 0
        || a.length % PAGE != 0
        || a.length < min_len
        || a.width == 0
        || a.height == 0
        || a.physical_device == 0
        || a.image == 0
        || (a.stride_bytes as u64) < a.width as u64 * bpp
        || a.stride_bytes % 16 != 0
    {
        return EINVAL;
    }
    let pool = pool_push();
    let texture = (|| {
        let mut device: Id = std::ptr::null_mut();
        // SAFETY: the guest's VkPhysicalDevice, a MoltenVK handle.
        unsafe { get_device(a.physical_device, &mut device) };
        if device.is_null() {
            return None;
        }
        let buffer = send!(device, c"newBufferWithBytesNoCopy:length:options:deallocator:" => Id,
            *mut c_void = a.address as *mut c_void, usize = a.length as usize,
            usize = MTL_STORAGE_MODE_SHARED, Id = std::ptr::null_mut());
        if buffer.is_null() {
            return None;
        }
        // SAFETY: a class name.
        let class = unsafe { objc_getClass(c"MTLTextureDescriptor".as_ptr()) };
        let desc = send!(class, c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:" => Id,
            usize = format, usize = a.width as usize, usize = a.height as usize, bool = false);
        send!(desc, c"setStorageMode:" => (), usize = MTL_STORAGE_MODE_SHARED);
        send!(desc, c"setUsage:" => (), usize = MTL_TEXTURE_USAGE);
        let texture = send!(buffer, c"newTextureWithDescriptor:offset:bytesPerRow:" => Id,
            Id = desc, usize = 0, usize = a.stride_bytes as usize);
        // SAFETY: we own `buffer` (a `new...` result); the texture keeps it.
        unsafe { objc_release(buffer) };
        (!texture.is_null()).then_some(texture)
    })();
    let r = match texture {
        Some(texture) => {
            // SAFETY: the guest's VkImage (a MoltenVK handle); MoltenVK
            // retains the texture, so ours is released.
            a.result = unsafe { set_texture(a.image, texture) };
            unsafe { objc_release(texture) };
            0
        }
        None => EINVAL,
    };
    pool_pop(pool);
    r
}
