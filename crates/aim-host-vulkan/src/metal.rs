//! Metal objects behind MoltenVK's: graphics buffers as the storage of
//! `VkImage`s (`docs/graphics-buffers.md`, `docs/vulkan-driver.md`), and
//! the shared events of timeline semaphores, which sync_files signal and
//! are signaled by ([`aim_sync_file::metal`]).
//!
//! The guest maps the buffer's memfd; the mapping starts on a page, so
//! Metal wraps it without copying (`newBufferWithBytesNoCopy`). A linear
//! texture over that `MTLBuffer`, in the Metal format MoltenVK uses for the
//! image's `VkFormat`, replaces the image's own texture
//! (`vkSetMTLTextureMVK`). MoltenVK keeps the texture, and the texture
//! keeps the buffer, until the image is destroyed.

use std::ffi::{CStr, c_char, c_void};
use std::os::fd::{BorrowedFd, OwnedFd};

use aim_hostcall::vulkan::{Attach, Timeline};

use crate::{EBADF, EINVAL, Private};

type Id = *mut c_void;
type Sel = *const c_void;

aim_hostcall::dylib! {
    static METAL = c"/System/Library/Frameworks/Metal.framework/Metal" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        static objc_msgSend: c_void;
        fn objc_release(obj: Id);
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

fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's C signature.
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute(objc_msgSend()) };
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

/// `VkExportMetalObjectsInfoEXT` with a `VkExportMetalSharedEventInfoEXT`.
#[repr(C)]
struct ExportObjects {
    s_type: i32,
    next: *mut ExportSharedEvent,
}

#[repr(C)]
struct ExportSharedEvent {
    s_type: i32,
    next: *const c_void,
    semaphore: u64,
    event: u64,
    shared_event: Id,
}

const EXPORT_METAL_OBJECTS_INFO_EXT: i32 = 1000311001;
const EXPORT_METAL_SHARED_EVENT_INFO_EXT: i32 = 1000311010;

/// The `MTLSharedEvent` of a timeline semaphore (MoltenVK's reference).
fn shared_event(p: &Private, a: &Timeline) -> Option<Id> {
    // SAFETY: MoltenVK's exported function with its C signature.
    let export = unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(u64, *mut ExportObjects)>(
            p.export_objects,
        )
    };
    let mut event = ExportSharedEvent {
        s_type: EXPORT_METAL_SHARED_EVENT_INFO_EXT,
        next: std::ptr::null(),
        semaphore: a.semaphore,
        event: 0,
        shared_event: std::ptr::null_mut(),
    };
    let mut info = ExportObjects {
        s_type: EXPORT_METAL_OBJECTS_INFO_EXT,
        next: &mut event,
    };
    if a.device == 0 || a.semaphore == 0 {
        return None;
    }
    // SAFETY: the guest's VkDevice and VkSemaphore, MoltenVK handles.
    unsafe { export(a.device, &mut info) };
    (!event.shared_event.is_null()).then_some(event.shared_event)
}

/// A sync_file for the semaphore's value.
pub fn fence(p: &Private, a: &Timeline) -> Result<OwnedFd, i64> {
    let event = shared_event(p, a).ok_or(EINVAL)?;
    let pool = pool_push();
    // SAFETY: an MTLSharedEvent.
    let file = unsafe { aim_sync_file::metal::fence(event, a.value) };
    pool_pop(pool);
    file.map_err(|_| EINVAL)
}

/// Set the semaphore to its value once the guest's sync_file has signaled.
pub fn signal(p: &Private, a: &Timeline) -> i64 {
    // SAFETY: F_GETFD only checks that the guest's fd is open.
    if unsafe { libc::fcntl(a.fd, libc::F_GETFD) } < 0 {
        return EBADF;
    }
    let Some(event) = shared_event(p, a) else {
        return EINVAL;
    };
    // SAFETY: the guest's open fd, borrowed for the call; an MTLSharedEvent.
    match unsafe { aim_sync_file::metal::signal_when(BorrowedFd::borrow_raw(a.fd), event, a.value) }
    {
        Ok(()) => 0,
        Err(_) => EBADF,
    }
}
