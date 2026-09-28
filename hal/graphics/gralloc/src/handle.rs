//! The buffer's `native_handle_t`: one fd (the memfd) and [`NUM_INTS`] ints
//! (docs/graphics-buffers.md, "The handle").

use crate::layout::Layout;
use crate::{Descriptor, PAGE_SIZE, align, metadata};

pub const MAGIC: i32 = 0x4447_4231;
pub const NUM_FDS: usize = 1;
pub const NUM_INTS: usize = 19;

/// Offset of the reserved region inside the metadata area.
pub const RESERVED_OFFSET: u64 = 8192;

/// The ints of a buffer handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub width: u32,
    pub height: u32,
    /// Resolved format.
    pub format: i32,
    pub requested_format: i32,
    pub layer_count: u32,
    pub usage: u64,
    /// Pixels.
    pub stride: u32,
    pub id: u64,
    pub data_size: u64,
    pub metadata_offset: u64,
    pub reserved_size: u64,
    pub layer_size: u64,
}

fn split(v: u64) -> [i32; 2] {
    [v as u32 as i32, (v >> 32) as u32 as i32]
}

fn join(lo: i32, hi: i32) -> u64 {
    lo as u32 as u64 | (hi as u32 as u64) << 32
}

impl Handle {
    /// A new buffer's handle for `layout`.
    pub fn new(desc: &Descriptor, layout: &Layout, id: u64) -> Handle {
        Handle {
            width: desc.width as u32,
            height: desc.height as u32,
            format: layout.format,
            requested_format: desc.format,
            layer_count: desc.layer_count as u32,
            usage: desc.usage,
            stride: layout.stride,
            id,
            data_size: layout.data_size,
            metadata_offset: align(layout.data_size.max(1), PAGE_SIZE),
            reserved_size: desc.reserved_size as u64,
            layer_size: layout.layer_size,
        }
    }

    /// Bytes of the metadata area (shared metadata and reserved region).
    pub fn metadata_size(&self) -> u64 {
        align(RESERVED_OFFSET + self.reserved_size, PAGE_SIZE)
    }

    /// Size of the memfd.
    pub fn total_size(&self) -> u64 {
        self.metadata_offset + self.metadata_size()
    }

    /// The plane layout (always `Some` for a handle the allocator made).
    pub fn layout(&self) -> Option<Layout> {
        Layout::new(self.format, self.width, self.height, self.layer_count)
    }

    pub fn to_ints(&self) -> [i32; NUM_INTS] {
        let mut v = [0; NUM_INTS];
        v[0] = MAGIC;
        v[1] = self.width as i32;
        v[2] = self.height as i32;
        v[3] = self.format;
        v[4] = self.requested_format;
        v[5] = self.layer_count as i32;
        v[6..8].copy_from_slice(&split(self.usage));
        v[8] = self.stride as i32;
        v[9..11].copy_from_slice(&split(self.id));
        v[11..13].copy_from_slice(&split(self.data_size));
        v[13..15].copy_from_slice(&split(self.metadata_offset));
        v[15..17].copy_from_slice(&split(self.reserved_size));
        v[17..19].copy_from_slice(&split(self.layer_size));
        v
    }

    /// Parse and check the ints of a handle.
    pub fn from_ints(v: &[i32]) -> Option<Handle> {
        if v.len() != NUM_INTS || v[0] != MAGIC {
            return None;
        }
        let h = Handle {
            width: v[1] as u32,
            height: v[2] as u32,
            format: v[3],
            requested_format: v[4],
            layer_count: v[5] as u32,
            usage: join(v[6], v[7]),
            stride: v[8] as u32,
            id: join(v[9], v[10]),
            data_size: join(v[11], v[12]),
            metadata_offset: join(v[13], v[14]),
            reserved_size: join(v[15], v[16]),
            layer_size: join(v[17], v[18]),
        };
        let layout = h.layout()?;
        let consistent = layout.stride == h.stride
            && layout.data_size == h.data_size
            && layout.layer_size == h.layer_size
            && h.metadata_offset >= h.data_size
            && h.metadata_offset % PAGE_SIZE == 0
            && h.reserved_size <= metadata::MAX_RESERVED_SIZE;
        consistent.then_some(h)
    }
}

/// `native_handle_t` from `<cutils/native_handle.h>`.
#[repr(C)]
pub struct NativeHandle {
    /// `sizeof(native_handle_t)`.
    pub version: i32,
    pub num_fds: i32,
    pub num_ints: i32,
    // followed by `num_fds` fds, then `num_ints` ints
}

/// `sizeof(native_handle_t)`, its `version` field.
pub const NATIVE_HANDLE_VERSION: i32 = 12;

/// The fd and ints of a buffer handle, if it is one of ours.
///
/// # Safety
/// `h` must point to a valid `native_handle_t`.
pub unsafe fn parse(h: *const NativeHandle) -> Option<(i32, Handle)> {
    if h.is_null() {
        return None;
    }
    // SAFETY: caller contract; the data follows the header.
    unsafe {
        let n = &*h;
        if n.version != NATIVE_HANDLE_VERSION
            || n.num_fds != NUM_FDS as i32
            || n.num_ints != NUM_INTS as i32
        {
            return None;
        }
        let data = (h as *const i32).add(3);
        let ints = std::slice::from_raw_parts(data.add(NUM_FDS), NUM_INTS);
        Some((*data, Handle::from_ints(ints)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format;

    #[test]
    fn round_trip() {
        let desc = Descriptor {
            width: 100,
            height: 30,
            layer_count: 1,
            format: format::RGBA_8888,
            usage: 0x333,
            reserved_size: 64,
        };
        let h = Handle::new(&desc, &desc.layout().unwrap(), 7 << 32 | 3);
        assert_eq!(h.metadata_offset, PAGE_SIZE);
        assert_eq!(h.total_size(), 2 * PAGE_SIZE);
        let ints = h.to_ints();
        assert_eq!(Handle::from_ints(&ints), Some(h));
        let mut bad = ints;
        bad[0] = 0;
        assert_eq!(Handle::from_ints(&bad), None);
        let mut bad = ints;
        bad[8] += 1;
        assert_eq!(Handle::from_ints(&bad), None);

        let mut raw = vec![NATIVE_HANDLE_VERSION, 1, NUM_INTS as i32, 42];
        raw.extend_from_slice(&ints);
        // SAFETY: a well-formed native_handle_t image.
        let parsed = unsafe { parse(raw.as_ptr().cast()) };
        assert_eq!(parsed, Some((42, h)));
    }
}
