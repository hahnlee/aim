//! Graphics buffers of the derived image (`docs/graphics-buffers.md`): the
//! plane layout of each format, the `native_handle` layout, and the metadata
//! shared through the buffer's memory. The allocator, the mapper and the
//! GLES driver all use this crate, so they agree by construction.
//!
//! Everything here is plain computation; the services own the fds and
//! mappings.

pub mod handle;
pub mod layout;
pub mod metadata;

pub use handle::Handle;
pub use layout::{Layout, Plane};

/// Page size of the guest (a 16 KiB Linux kernel) and of the host.
pub const PAGE_SIZE: u64 = 16384;

/// `android.hardware.graphics.common.PixelFormat` values.
pub mod format {
    pub const RGBA_8888: i32 = 0x1;
    pub const RGBX_8888: i32 = 0x2;
    pub const RGB_888: i32 = 0x3;
    pub const RGB_565: i32 = 0x4;
    pub const BGRA_8888: i32 = 0x5;
    pub const YCRCB_420_SP: i32 = 0x11;
    pub const RGBA_FP16: i32 = 0x16;
    pub const BLOB: i32 = 0x21;
    pub const IMPLEMENTATION_DEFINED: i32 = 0x22;
    pub const YCBCR_420_888: i32 = 0x23;
    pub const RGBA_1010102: i32 = 0x2b;
    pub const YCBCR_P010: i32 = 0x36;
    pub const R_8: i32 = 0x38;
    pub const YV12: i32 = 0x3231_5659;
}

/// `android.hardware.graphics.common.BufferUsage` bits.
pub mod usage {
    pub const CPU_READ_MASK: u64 = 0xf;
    pub const CPU_WRITE_MASK: u64 = 0xf0;
    pub const GPU_TEXTURE: u64 = 1 << 8;
    pub const GPU_RENDER_TARGET: u64 = 1 << 9;
    pub const COMPOSER_OVERLAY: u64 = 1 << 11;
    pub const COMPOSER_CLIENT_TARGET: u64 = 1 << 12;
    pub const PROTECTED: u64 = 1 << 14;
    pub const COMPOSER_CURSOR: u64 = 1 << 15;
    pub const VIDEO_ENCODER: u64 = 1 << 16;
    pub const CAMERA_OUTPUT: u64 = 1 << 17;
    pub const CAMERA_INPUT: u64 = 1 << 18;
    pub const VIDEO_DECODER: u64 = 1 << 22;
    pub const GPU_DATA_BUFFER: u64 = 1 << 24;
    pub const GPU_CUBE_MAP: u64 = 1 << 25;
    pub const GPU_MIPMAP_COMPLETE: u64 = 1 << 26;

    /// Usage that makes the host GPU import the buffer as a texture.
    pub const GPU_IMAGE: u64 = GPU_TEXTURE
        | GPU_RENDER_TARGET
        | COMPOSER_OVERLAY
        | COMPOSER_CLIENT_TARGET
        | COMPOSER_CURSOR;
}

/// What an allocation asks for (`BufferDescriptorInfo` without the name and
/// the options).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub width: i32,
    pub height: i32,
    pub layer_count: i32,
    pub format: i32,
    pub usage: u64,
    pub reserved_size: i64,
}

/// Largest width or height we allocate (Metal's 2D texture limit).
pub const MAX_DIMENSION: i32 = 16384;

/// Why a descriptor cannot be allocated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    /// Sizes out of range (`AllocationError::BAD_DESCRIPTOR`).
    BadDescriptor,
    /// A valid request this allocator does not serve
    /// (`AllocationError::UNSUPPORTED`).
    Unsupported,
}

/// The format a request resolves to.
pub fn resolve_format(requested: i32, usage: u64) -> i32 {
    if requested != format::IMPLEMENTATION_DEFINED {
        return requested;
    }
    let video =
        usage::VIDEO_ENCODER | usage::VIDEO_DECODER | usage::CAMERA_OUTPUT | usage::CAMERA_INPUT;
    if usage & video != 0 {
        format::YCBCR_420_888
    } else {
        format::RGBX_8888
    }
}

impl Descriptor {
    /// The layout of this descriptor, or why it cannot be allocated.
    pub fn layout(&self) -> Result<Layout, Unsupported> {
        let d = self;
        let format = resolve_format(d.format, d.usage);
        // A BLOB's width is its size in bytes (camera JPEGs, codec data).
        let max_width = if format == format::BLOB {
            i32::MAX
        } else {
            MAX_DIMENSION
        };
        if d.width <= 0
            || d.height <= 0
            || d.layer_count <= 0
            || d.reserved_size < 0
            || d.width > max_width
            || d.height > MAX_DIMENSION
        {
            return Err(Unsupported::BadDescriptor);
        }
        let unsupported = usage::PROTECTED | usage::GPU_CUBE_MAP | usage::GPU_MIPMAP_COMPLETE;
        if d.usage & unsupported != 0 {
            return Err(Unsupported::Unsupported);
        }
        if format == format::BLOB && d.height != 1 {
            return Err(Unsupported::BadDescriptor);
        }
        let layout = Layout::new(
            format,
            d.width as u32,
            d.height as u32,
            d.layer_count as u32,
        )
        .ok_or(Unsupported::Unsupported)?;
        let gpu = d.usage & usage::GPU_IMAGE;
        if gpu != 0 && (!layout::gpu_capable(format, gpu) || d.layer_count > 1) {
            return Err(Unsupported::Unsupported);
        }
        Ok(layout)
    }
}

/// Round `v` up to a multiple of `to` (a power of two or any positive value).
pub const fn align(v: u64, to: u64) -> u64 {
    v.div_ceil(to) * to
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(format: i32, width: i32, height: i32, usage: u64) -> Descriptor {
        Descriptor {
            width,
            height,
            layer_count: 1,
            format,
            usage,
            reserved_size: 0,
        }
    }

    #[test]
    fn implementation_defined_resolves_by_usage() {
        assert_eq!(
            resolve_format(format::IMPLEMENTATION_DEFINED, usage::GPU_TEXTURE),
            format::RGBX_8888
        );
        assert_eq!(
            resolve_format(format::IMPLEMENTATION_DEFINED, usage::VIDEO_ENCODER),
            format::YCBCR_420_888
        );
        assert_eq!(resolve_format(format::RGBA_8888, 0), format::RGBA_8888);
    }

    #[test]
    fn descriptors_are_checked() {
        let gpu = usage::GPU_TEXTURE | usage::GPU_RENDER_TARGET;
        assert!(desc(format::RGBA_8888, 64, 64, gpu).layout().is_ok());
        assert_eq!(
            desc(format::RGBA_8888, 0, 64, gpu).layout(),
            Err(Unsupported::BadDescriptor)
        );
        assert_eq!(
            desc(format::RGBA_8888, 64, 64, usage::PROTECTED).layout(),
            Err(Unsupported::Unsupported)
        );
        // CPU only is fine for any format; GPU needs a Metal format.
        assert!(desc(format::RGB_888, 64, 64, 0x33).layout().is_ok());
        assert_eq!(
            desc(format::RGB_888, 64, 64, gpu).layout(),
            Err(Unsupported::Unsupported)
        );
        // YUV is sampled (by the Vulkan driver), not rendered to.
        assert!(
            desc(format::YV12, 64, 64, usage::GPU_TEXTURE)
                .layout()
                .is_ok()
        );
        assert_eq!(
            desc(format::YCBCR_420_888, 64, 64, gpu).layout(),
            Err(Unsupported::Unsupported)
        );
        assert_eq!(
            desc(0x30, 64, 64, 0x33).layout(),
            Err(Unsupported::Unsupported)
        );
        assert_eq!(
            desc(format::BLOB, 4096, 2, 0x33).layout(),
            Err(Unsupported::BadDescriptor)
        );
        // A 3 MB JPEG buffer.
        assert!(desc(format::BLOB, 3 << 20, 1, 0x33).layout().is_ok());
        assert_eq!(
            desc(format::RGBA_8888, MAX_DIMENSION + 1, 1, 0x33).layout(),
            Err(Unsupported::BadDescriptor)
        );
        let mut layers = desc(format::RGBA_8888, 64, 64, gpu);
        layers.layer_count = 2;
        assert_eq!(layers.layout(), Err(Unsupported::Unsupported));
        layers.usage = 0x33;
        assert!(layers.layout().is_ok());
    }
}
