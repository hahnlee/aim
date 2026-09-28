//! Graphics buffers as host `EGLImage`s (docs/graphics-buffers.md, "GPU
//! access"): the driver maps a buffer's memfd itself, once per process and
//! buffer, and the host wraps the mapping as a Metal texture.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use aim_gralloc::handle::{self, NativeHandle};
use aim_hostcall::gpu::ImportBuffer;

use crate::types::*;

/// `ANativeWindowBuffer` (`<nativebase/nativebase.h>`) up to its handle.
#[repr(C)]
pub struct NativeBuffer {
    pub magic: i32,
    pub version: i32,
    reserved: [usize; 4],
    inc_ref: usize,
    dec_ref: usize,
    pub width: i32,
    pub height: i32,
    pub stride: i32,
    pub format: i32,
    usage_deprecated: i32,
    layer_count: usize,
    reserved2: usize,
    pub handle: *const NativeHandle,
}

/// `ANDROID_NATIVE_BUFFER_MAGIC`: `'_bfr'`.
const NATIVE_BUFFER_MAGIC: i32 = 0x5f62_6672;

struct Mapping {
    address: usize,
    length: usize,
    refs: usize,
}

// SAFETY: a process-wide shared mapping.
unsafe impl Send for Mapping {}

fn mappings() -> MutexGuard<'static, HashMap<u64, Mapping>> {
    static MAPPINGS: OnceLock<Mutex<HashMap<u64, Mapping>>> = OnceLock::new();
    MAPPINGS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// A buffer imported into the host: its id (for [`release`]) and image.
pub struct Imported {
    pub id: u64,
    pub image: EGLImage,
}

/// Import `buffer` (an `ANativeWindowBuffer`) on the host display `display`.
///
/// # Safety
/// `buffer` must be null or point to an `ANativeWindowBuffer`.
pub unsafe fn import(display: EGLDisplay, buffer: *const NativeBuffer) -> Option<Imported> {
    // SAFETY: caller contract; the handle is the buffer's.
    let (b, (fd, h)) = unsafe {
        let b = buffer.as_ref()?;
        if b.magic != NATIVE_BUFFER_MAGIC {
            return None;
        }
        (b, handle::parse(b.handle)?)
    };
    let layout = h.layout()?;
    let length = h.metadata_offset as usize;
    let address = {
        let mut maps = mappings();
        let m = match maps.get_mut(&h.id) {
            Some(m) => m,
            None => {
                // SAFETY: the buffer's memfd; the mapping is ours.
                let address = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        length,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                };
                if address == libc::MAP_FAILED {
                    return None;
                }
                maps.entry(h.id).or_insert(Mapping {
                    address: address as usize,
                    length,
                    refs: 0,
                })
            }
        };
        m.refs += 1;
        m.address
    };
    let mut args = ImportBuffer {
        display: display as u64,
        address: address as u64,
        length: length as u64,
        width: b.width as u32,
        height: b.height as u32,
        stride_bytes: layout.stride_bytes() as u32,
        format: h.format,
        image: 0,
    };
    if aim_hostcall::guest::gpu_import_buffer(&mut args).is_err() {
        unmap(h.id);
        return None;
    }
    Some(Imported {
        id: h.id,
        image: args.image as EGLImage,
    })
}

/// Drop one import's hold on the buffer's mapping (its image destroyed).
pub fn unmap(id: u64) {
    let mut maps = mappings();
    let Some(m) = maps.get_mut(&id) else { return };
    m.refs -= 1;
    if m.refs == 0 {
        let m = maps.remove(&id).unwrap();
        // SAFETY: our mapping, no longer used by any image.
        unsafe { libc::munmap(m.address as *mut _, m.length) };
    }
}
