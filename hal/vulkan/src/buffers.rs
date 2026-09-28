//! Graphics buffers as image storage (docs/graphics-buffers.md, "GPU
//! access"): the driver maps a buffer's memfd itself, once per process and
//! buffer, and the host makes the mapping the storage of a `VkImage`
//! (`FN_ATTACH`) or imports it as host memory.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use aim_gralloc::handle::{self, Handle, NativeHandle};
use aim_hostcall::vulkan::Attach;

use crate::types::*;

struct Mapping {
    address: usize,
    length: usize,
    refs: usize,
}

fn mappings() -> MutexGuard<'static, HashMap<u64, Mapping>> {
    static MAPPINGS: OnceLock<Mutex<HashMap<u64, Mapping>>> = OnceLock::new();
    MAPPINGS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// A buffer mapped for the GPU; [`unmap`] its `id` when done.
pub struct Mapped {
    pub id: u64,
    pub address: usize,
    /// The data region (pixels of all layers), page aligned.
    pub length: usize,
    pub handle: Handle,
}

/// Map the buffer behind `native` (once per process and buffer).
///
/// # Safety
/// `native` must be null or a valid `native_handle_t`.
pub unsafe fn map(native: *const NativeHandle) -> Option<Mapped> {
    // SAFETY: caller contract.
    let (fd, h) = unsafe { handle::parse(native)? };
    let length = h.metadata_offset as usize;
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
    Some(Mapped {
        id: h.id,
        address: m.address,
        length: m.length,
        handle: h,
    })
}

/// Drop one hold on a buffer's mapping.
pub fn unmap(id: u64) {
    let mut maps = mappings();
    let Some(m) = maps.get_mut(&id) else { return };
    m.refs -= 1;
    if m.refs == 0 {
        let m = maps.remove(&id).unwrap();
        // SAFETY: our mapping, no longer used by any image or memory.
        unsafe { libc::munmap(m.address as *mut _, m.length) };
    }
}

/// Make the mapped buffer the storage of `image` (2D, one level, format
/// `format`) on `physical_device`.
pub fn attach(physical_device: VkDispatch, image: u64, m: &Mapped, format: i32) -> VkResult {
    let Some(layout) = m.handle.layout() else {
        return VK_ERROR_INVALID_EXTERNAL_HANDLE;
    };
    let mut args = Attach {
        physical_device: physical_device as u64,
        image,
        address: m.address as u64,
        length: m.length as u64,
        width: m.handle.width,
        height: m.handle.height,
        stride_bytes: layout.stride_bytes() as u32,
        format,
        ..Default::default()
    };
    match aim_hostcall::guest::vulkan_attach(&mut args) {
        Ok(()) => args.result,
        Err(_) => VK_ERROR_INVALID_EXTERNAL_HANDLE,
    }
}
