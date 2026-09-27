//! Plane layouts: a pure function of (format, width, height, layer count),
//! so every process derives the same layout from a handle.

use crate::{align, format};

/// Rows of every plane are aligned to this many bytes (Metal needs 16 for
/// linear textures; 64 is a cache line).
pub const ROW_ALIGN: u64 = 64;

/// `android.hardware.graphics.common.PlaneLayoutComponentType` values.
pub mod component {
    pub const Y: i64 = 1 << 0;
    pub const CB: i64 = 1 << 1;
    pub const CR: i64 = 1 << 2;
    pub const R: i64 = 1 << 10;
    pub const G: i64 = 1 << 11;
    pub const B: i64 = 1 << 12;
    pub const RAW: i64 = 1 << 20;
    pub const A: i64 = 1 << 30;
}

/// One component of a plane: (type, offset in bits, size in bits).
pub type Component = (i64, i64, i64);

/// `android.hardware.graphics.common.PlaneLayout`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    pub components: Vec<Component>,
    pub offset: u64,
    pub sample_increment_bits: u64,
    pub stride_bytes: u64,
    pub width_samples: u64,
    pub height_samples: u64,
    pub total_size: u64,
    pub horizontal_subsampling: u64,
    pub vertical_subsampling: u64,
}

/// The layout of one buffer's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The resolved format.
    pub format: i32,
    /// Row pitch in pixels (luma samples for YUV).
    pub stride: u32,
    /// Planes of layer 0; layer n is at n × `layer_size`.
    pub planes: Vec<Plane>,
    pub layer_size: u64,
    /// Bytes of all layers.
    pub data_size: u64,
}

/// Single-plane formats: (bytes per pixel, components).
fn packed(format: i32) -> Option<(u64, &'static [Component])> {
    use component::*;
    Some(match format {
        format::RGBA_8888 => (4, &[(R, 0, 8), (G, 8, 8), (B, 16, 8), (A, 24, 8)]),
        format::RGBX_8888 => (4, &[(R, 0, 8), (G, 8, 8), (B, 16, 8)]),
        format::BGRA_8888 => (4, &[(B, 0, 8), (G, 8, 8), (R, 16, 8), (A, 24, 8)]),
        format::RGB_888 => (3, &[(R, 0, 8), (G, 8, 8), (B, 16, 8)]),
        format::RGB_565 => (2, &[(B, 0, 5), (G, 5, 6), (R, 11, 5)]),
        format::RGBA_FP16 => (8, &[(R, 0, 16), (G, 16, 16), (B, 32, 16), (A, 48, 16)]),
        format::RGBA_1010102 => (4, &[(R, 0, 10), (G, 10, 10), (B, 20, 10), (A, 30, 2)]),
        format::R_8 => (1, &[(R, 0, 8)]),
        format::BLOB => (1, &[(RAW, 0, 8)]),
        _ => return None,
    })
}

/// Formats the host GPU imports as a texture (docs/graphics-buffers.md).
pub fn gpu_capable(format: i32) -> bool {
    matches!(
        format,
        format::RGBA_8888
            | format::RGBX_8888
            | format::BGRA_8888
            | format::RGB_565
            | format::RGBA_FP16
            | format::RGBA_1010102
            | format::R_8
    )
}

/// The smallest pixel stride >= `width` whose rows are `ROW_ALIGN`-aligned.
fn pixel_stride(width: u64, bpp: u64) -> u64 {
    let mut stride = width;
    while (stride * bpp) % ROW_ALIGN != 0 {
        stride += 1;
    }
    stride
}

fn plane(
    components: &[Component],
    offset: u64,
    increment_bits: u64,
    stride_bytes: u64,
    width: u64,
    height: u64,
    subsampling: u64,
) -> Plane {
    Plane {
        components: components.to_vec(),
        offset,
        sample_increment_bits: increment_bits,
        stride_bytes,
        width_samples: width,
        height_samples: height,
        total_size: stride_bytes * height,
        horizontal_subsampling: subsampling,
        vertical_subsampling: subsampling,
    }
}

impl Layout {
    /// The layout of a buffer, or `None` for an unsupported format.
    pub fn new(format: i32, width: u32, height: u32, layers: u32) -> Option<Layout> {
        use component::*;
        let (w, h) = (width as u64, height as u64);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let (stride, planes) = if let Some((bpp, components)) = packed(format) {
            let stride = pixel_stride(w, bpp);
            let p = plane(components, 0, bpp * 8, stride * bpp, w, h, 1);
            (stride, vec![p])
        } else {
            match format {
                format::YCBCR_420_888 | format::YCRCB_420_SP => {
                    let stride = align(w, ROW_ALIGN);
                    let chroma: &[Component] = if format == format::YCBCR_420_888 {
                        &[(CB, 0, 8), (CR, 8, 8)]
                    } else {
                        &[(CR, 0, 8), (CB, 8, 8)]
                    };
                    let y = plane(&[(Y, 0, 8)], 0, 8, stride, w, h, 1);
                    let c = plane(chroma, y.total_size, 16, stride, cw, ch, 2);
                    (stride, vec![y, c])
                }
                format::YV12 => {
                    let stride = align(w, ROW_ALIGN);
                    let c_stride = align(stride / 2, 16);
                    let y = plane(&[(Y, 0, 8)], 0, 8, stride, w, h, 1);
                    let cr = plane(&[(CR, 0, 8)], y.total_size, 8, c_stride, cw, ch, 2);
                    let cb = plane(
                        &[(CB, 0, 8)],
                        cr.offset + cr.total_size,
                        8,
                        c_stride,
                        cw,
                        ch,
                        2,
                    );
                    (stride, vec![y, cr, cb])
                }
                format::YCBCR_P010 => {
                    let stride = pixel_stride(w, 2);
                    let y = plane(&[(Y, 6, 10)], 0, 16, stride * 2, w, h, 1);
                    let c = plane(
                        &[(CB, 6, 10), (CR, 22, 10)],
                        y.total_size,
                        32,
                        stride * 2,
                        cw,
                        ch,
                        2,
                    );
                    (stride, vec![y, c])
                }
                _ => return None,
            }
        };
        let last = planes.last().unwrap();
        let layer_size = align(last.offset + last.total_size, ROW_ALIGN);
        Some(Layout {
            format,
            stride: stride as u32,
            planes,
            layer_size,
            data_size: layer_size * layers as u64,
        })
    }

    /// Row pitch of plane 0 in bytes.
    pub fn stride_bytes(&self) -> u64 {
        self.planes[0].stride_bytes
    }
}

/// The DRM fourcc of a format (`PIXEL_FORMAT_FOURCC`); 0 for `BLOB`.
pub fn fourcc(format: i32) -> u32 {
    let code = |s: &[u8; 4]| u32::from_le_bytes(*s);
    match format {
        format::RGBA_8888 => code(b"AB24"),
        format::RGBX_8888 => code(b"XB24"),
        format::BGRA_8888 => code(b"AR24"),
        format::RGB_888 => code(b"BG24"),
        format::RGB_565 => code(b"RG16"),
        format::RGBA_FP16 => code(b"AB4H"),
        format::RGBA_1010102 => code(b"AB30"),
        format::R_8 => code(b"R8  "),
        format::YCBCR_420_888 => code(b"NV12"),
        format::YCRCB_420_SP => code(b"NV21"),
        format::YV12 => code(b"YV12"),
        format::YCBCR_P010 => code(b"P010"),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_rows_are_aligned() {
        let l = Layout::new(format::RGBA_8888, 100, 10, 1).unwrap();
        assert_eq!(l.stride, 112);
        assert_eq!(l.stride_bytes(), 448);
        assert_eq!(l.data_size, 4480);
        let l = Layout::new(format::RGB_888, 10, 2, 1).unwrap();
        assert_eq!(l.stride_bytes() % 64, 0);
        assert_eq!(l.stride_bytes(), l.stride as u64 * 3);
    }

    #[test]
    fn yuv_planes() {
        let l = Layout::new(format::YCBCR_420_888, 100, 50, 1).unwrap();
        assert_eq!(l.stride, 128);
        assert_eq!(l.planes.len(), 2);
        assert_eq!(l.planes[1].offset, 128 * 50);
        assert_eq!(l.planes[1].height_samples, 25);
        assert_eq!(l.planes[1].width_samples, 50);
        let l = Layout::new(format::YV12, 64, 64, 1).unwrap();
        assert_eq!(l.planes[1].stride_bytes, 32);
        assert_eq!(l.planes[1].offset, 64 * 64);
        assert_eq!(l.planes[2].offset, 64 * 64 + 32 * 32);
        assert_eq!(l.planes[1].components[0].0, component::CR);
    }

    #[test]
    fn layers_follow_each_other() {
        let l = Layout::new(format::R_8, 64, 64, 3).unwrap();
        assert_eq!(l.layer_size, 64 * 64);
        assert_eq!(l.data_size, 3 * 64 * 64);
        assert!(Layout::new(0x30, 64, 64, 1).is_none());
    }

    #[test]
    fn fourccs() {
        assert_eq!(fourcc(format::RGBA_8888), 0x3432_4241);
        assert_eq!(fourcc(format::BLOB), 0);
    }
}
