//! Graphics buffers as ANGLE `EGLImage`s (`docs/graphics-buffers.md`).
//!
//! The guest maps the buffer's memfd; the mapping starts on a page, so
//! Metal wraps it without copying (`newBufferWithBytesNoCopy`). A linear
//! texture over that `MTLBuffer` is imported with
//! `EGL_ANGLE_metal_texture_client_buffer`. ANGLE keeps the texture, and the
//! texture keeps the buffer, until the image is destroyed.

use std::ffi::{CStr, c_char, c_void};

use aim_hostcall::gpu::ImportBuffer;

use crate::{EINVAL, resolved};

pub type Id = *mut c_void;
pub type Sel = *const c_void;

aim_hostcall::dylib! {
    static METAL = c"/System/Library/Frameworks/Metal.framework/Metal" {
        pub fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        pub static objc_msgSend: c_void;
        pub fn objc_release(obj: Id);
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
}

pub fn pool_push() -> *mut c_void {
    // SAFETY: no preconditions.
    unsafe { objc_autoreleasePoolPush() }
}

pub fn pool_pop(pool: *mut c_void) {
    // SAFETY: `pool` came from `pool_push` on this thread.
    unsafe { objc_autoreleasePoolPop(pool) }
}

pub fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's C signature.
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn($crate::metal::Id, $crate::metal::Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute($crate::metal::objc_msgSend()) };
        unsafe { f($obj, $crate::metal::sel($sel) $(, $a)*) }
    }};
}
pub(crate) use send;

// Metal and EGL constants.
const MTL_STORAGE_MODE_SHARED: usize = 0;
const MTL_TEXTURE_USAGE: usize = 0x01 | 0x04 | 0x10; // shader read, render target, format view
const EGL_NO_CONTEXT: usize = 0;
const EGL_NONE: i32 = 0x3038;
const EGL_DEVICE_EXT: i32 = 0x322c;
const EGL_METAL_DEVICE_ANGLE: i32 = 0x34a6;
const EGL_METAL_TEXTURE_ANGLE: u32 = 0x34a7;
const EGL_TEXTURE_INTERNAL_FORMAT_ANGLE: i32 = 0x345d;
const GL_RGB: i32 = 0x1907;

/// `PixelFormat` → (`MTLPixelFormat`, bytes per pixel, GL internal format
/// override).
fn metal_format(format: i32) -> Option<(usize, u32, Option<i32>)> {
    Some(match format {
        0x1 => (70, 4, None),         // RGBA_8888: RGBA8Unorm
        0x2 => (70, 4, Some(GL_RGB)), // RGBX_8888: alpha reads as 1
        0x4 => (40, 2, None),         // RGB_565: B5G6R5Unorm
        0x5 => (80, 4, None),         // BGRA_8888: BGRA8Unorm
        0x16 => (115, 8, None),       // RGBA_FP16: RGBA16Float
        0x2b => (90, 4, None),        // RGBA_1010102: RGB10A2Unorm
        0x38 => (10, 1, None),        // R_8: R8Unorm
        _ => return None,
    })
}

/// The `MTLDevice` behind an ANGLE display.
pub fn device(display: usize) -> Option<Id> {
    let query_display = resolved(c"eglQueryDisplayAttribEXT");
    let query_device = resolved(c"eglQueryDeviceAttribEXT");
    if query_display == 0 || query_device == 0 {
        return None;
    }
    // SAFETY: ANGLE's entry points with their EGL signatures.
    unsafe {
        let query_display: unsafe extern "C" fn(usize, i32, *mut isize) -> u32 =
            std::mem::transmute(query_display);
        let query_device: unsafe extern "C" fn(isize, i32, *mut isize) -> u32 =
            std::mem::transmute(query_device);
        let (mut dev, mut mtl) = (0isize, 0isize);
        if query_display(display, EGL_DEVICE_EXT, &mut dev) == 0
            || query_device(dev, EGL_METAL_DEVICE_ANGLE, &mut mtl) == 0
        {
            return None;
        }
        Some(mtl as Id)
    }
}

pub fn import_buffer(a: &mut ImportBuffer) -> i64 {
    let page = 16384;
    let Some((pixel_format, bpp, internal)) = metal_format(a.format) else {
        return EINVAL;
    };
    let min_len = a.stride_bytes as u64 * a.height as u64;
    if a.address % page != 0
        || a.length % page != 0
        || a.length < min_len
        || a.width == 0
        || a.height == 0
        || (a.stride_bytes as u64) < a.width as u64 * bpp as u64
        || a.stride_bytes % 16 != 0
    {
        return EINVAL;
    }
    let create_image = resolved(c"eglCreateImageKHR");
    if create_image == 0 {
        return EINVAL;
    }
    let pool = pool_push();
    let display = crate::display::host(a.display as usize);
    let image = (|| {
        let device = device(display)?;
        let buffer = send!(device, c"newBufferWithBytesNoCopy:length:options:deallocator:" => Id,
            *mut c_void = a.address as *mut c_void, usize = a.length as usize,
            usize = MTL_STORAGE_MODE_SHARED, Id = std::ptr::null_mut());
        if buffer.is_null() {
            return None;
        }
        // SAFETY: a class name.
        let class = unsafe { objc_getClass(c"MTLTextureDescriptor".as_ptr()) };
        let desc = send!(class, c"texture2DDescriptorWithPixelFormat:width:height:mipmapped:" => Id,
            usize = pixel_format, usize = a.width as usize, usize = a.height as usize,
            bool = false);
        send!(desc, c"setStorageMode:" => (), usize = MTL_STORAGE_MODE_SHARED);
        send!(desc, c"setUsage:" => (), usize = MTL_TEXTURE_USAGE);
        let texture = send!(buffer, c"newTextureWithDescriptor:offset:bytesPerRow:" => Id,
            Id = desc, usize = 0, usize = a.stride_bytes as usize);
        // SAFETY: we own `buffer` (a `new...` result); the texture keeps it.
        unsafe { objc_release(buffer) };
        if texture.is_null() {
            return None;
        }
        let attribs = match internal {
            Some(f) => [EGL_TEXTURE_INTERNAL_FORMAT_ANGLE, f, EGL_NONE],
            None => [EGL_NONE; 3],
        };
        // SAFETY: ANGLE's eglCreateImageKHR; the image keeps the texture.
        let image = unsafe {
            let f: unsafe extern "C" fn(usize, usize, u32, Id, *const i32) -> usize =
                std::mem::transmute(create_image);
            let image = f(
                display,
                EGL_NO_CONTEXT,
                EGL_METAL_TEXTURE_ANGLE,
                texture,
                attribs.as_ptr(),
            );
            objc_release(texture);
            image
        };
        (image != 0).then_some(image)
    })();
    pool_pop(pool);
    match image {
        Some(image) => {
            a.image = image as u64;
            0
        }
        None => EINVAL,
    }
}
