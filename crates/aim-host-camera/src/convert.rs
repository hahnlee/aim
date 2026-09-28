//! Frame conversion: a captured BGRA frame, scaled with a centered crop to
//! an output's size, packed as RGBA/RGBX or as NV12/NV21.
//!
//! The crop keeps the output's aspect ratio, as Android's camera outputs
//! do: every output shows the center of the sensor image, cropped to its
//! own aspect ratio (`android.scaler.cropRegion` is the whole array).
//! Scaling is bilinear. YUV is BT.601 full range (JFIF), the camera
//! outputs' default dataspace.

/// A BGRA frame whose rows are `stride` bytes apart.
#[derive(Clone, Debug, Default)]
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl Bgra {
    /// A tightly packed frame of `width` × `height`.
    pub fn new(width: u32, height: u32) -> Bgra {
        Bgra {
            width,
            height,
            stride: width as usize * 4,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// This frame scaled to `width` × `height`, cropped around the center
    /// to that aspect ratio.
    pub fn scaled(&self, width: u32, height: u32) -> Bgra {
        if (width, height) == (self.width, self.height) && self.stride == width as usize * 4 {
            return self.clone();
        }
        let mut out = Bgra::new(width, height);
        if self.width == 0 || self.height == 0 {
            return out;
        }
        let (cx, cy, cw, ch) = crop(self.width, self.height, width, height);
        // For each output column (row): the two source columns (rows) around
        // its center, within the crop, and the weight of the second one in
        // 1/256ths.
        let axis = |n: u32, start: u32, len: u32| -> Vec<(usize, usize, u32)> {
            (0..n)
                .map(|i| {
                    let pos = ((i as u64 * 2 + 1) * len as u64 * 256 / (n as u64 * 2)) as i64 - 128
                        + start as i64 * 256;
                    let pos = pos.clamp(start as i64 * 256, (start + len - 1) as i64 * 256);
                    let i0 = (pos >> 8) as usize;
                    let i1 = (i0 + 1).min((start + len - 1) as usize);
                    (i0, i1, (pos & 0xff) as u32)
                })
                .collect()
        };
        let xs = axis(width, cx, cw);
        let ys = axis(height, cy, ch);
        let row_bytes = self.width as usize * 4;
        for (out_row, &(y0, y1, fy)) in out.data.chunks_exact_mut(width as usize * 4).zip(&ys) {
            let r0 = &self.data[y0 * self.stride..][..row_bytes];
            let r1 = &self.data[y1 * self.stride..][..row_bytes];
            for (o, &(x0, x1, fx)) in out_row.chunks_exact_mut(4).zip(&xs) {
                let (a, b) = (&r0[x0 * 4..x0 * 4 + 4], &r0[x1 * 4..x1 * 4 + 4]);
                let (c, d) = (&r1[x0 * 4..x0 * 4 + 4], &r1[x1 * 4..x1 * 4 + 4]);
                for k in 0..4 {
                    let top = a[k] as u32 * (256 - fx) + b[k] as u32 * fx;
                    let bottom = c[k] as u32 * (256 - fx) + d[k] as u32 * fx;
                    o[k] = ((top * (256 - fy) + bottom * fy + 32768) >> 16) as u8;
                }
            }
        }
        out
    }
}

/// The centered crop (x, y, width, height) of a `sw` × `sh` source with the
/// aspect ratio of `dw` × `dh`.
pub fn crop(sw: u32, sh: u32, dw: u32, dh: u32) -> (u32, u32, u32, u32) {
    let (sw64, sh64, dw64, dh64) = (sw as u64, sh as u64, dw as u64, dh as u64);
    if sw64 * dh64 > dw64 * sh64 {
        let w = ((sh64 * dw64 + dh64 / 2) / dh64).clamp(1, sw64) as u32;
        ((sw - w) / 2, 0, w, sh)
    } else {
        let h = ((sw64 * dh64 + dw64 / 2) / dw64).clamp(1, sh64) as u32;
        (0, (sh - h) / 2, sw, h)
    }
}

/// Write `src` (already at the output's size) as RGBA rows `stride` bytes
/// apart. RGBX gets alpha 255 too.
pub fn to_rgba(src: &Bgra, dst: &mut [u8], stride: usize) {
    let w = src.width as usize;
    for y in 0..src.height as usize {
        let s = &src.data[y * src.stride..][..w * 4];
        let d = &mut dst[y * stride..][..w * 4];
        for (d, s) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], 255]);
        }
    }
}

fn luma(p: &[u8]) -> u8 {
    let (b, g, r) = (p[0] as i32, p[1] as i32, p[2] as i32);
    ((77 * r + 150 * g + 29 * b + 128) >> 8) as u8
}

fn chroma(b: i32, g: i32, r: i32) -> (u8, u8) {
    let cb = ((-43 * r - 85 * g + 128 * b + 128) >> 8) + 128;
    let cr = ((128 * r - 107 * g - 21 * b + 128) >> 8) + 128;
    (cb.clamp(0, 255) as u8, cr.clamp(0, 255) as u8)
}

/// Write `src` (already at the output's size) as a Y plane of `stride`
/// bytes per row and an interleaved chroma plane at `chroma_offset`, Cb
/// first for NV12 (`cr_first` false) or Cr first for NV21. Chroma is the
/// mean of each 2×2 block (edge pixels repeated for odd sizes).
pub fn to_nv(
    src: &Bgra,
    dst: &mut [u8],
    stride: usize,
    chroma_offset: usize,
    chroma_stride: usize,
    cr_first: bool,
) {
    let (w, h) = (src.width as usize, src.height as usize);
    let row = |y: usize| &src.data[y * src.stride..][..w * 4];
    for y in 0..h {
        let d = &mut dst[y * stride..][..w];
        for (d, p) in d.iter_mut().zip(row(y).chunks_exact(4)) {
            *d = luma(p);
        }
    }
    for cy in 0..h.div_ceil(2) {
        let (r0, r1) = (row(cy * 2), row((cy * 2 + 1).min(h - 1)));
        let out = &mut dst[chroma_offset + cy * chroma_stride..][..w.div_ceil(2) * 2];
        for (cx, pair) in out.chunks_exact_mut(2).enumerate() {
            let (x0, x1) = (cx * 8, (cx * 2 + 1).min(w - 1) * 4);
            let sum = |k: usize| {
                (r0[x0 + k] as i32 + r0[x1 + k] as i32 + r1[x0 + k] as i32 + r1[x1 + k] as i32 + 2)
                    / 4
            };
            let (cb, cr) = chroma(sum(0), sum(1), sum(2));
            pair.copy_from_slice(&if cr_first { [cr, cb] } else { [cb, cr] });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Bgra {
        let mut f = Bgra::new(w, h);
        for p in f.data.chunks_exact_mut(4) {
            p.copy_from_slice(&bgra);
        }
        f
    }

    #[test]
    fn crop_keeps_the_output_aspect_ratio() {
        assert_eq!(crop(1920, 1080, 640, 480), (240, 0, 1440, 1080));
        assert_eq!(crop(640, 480, 1280, 720), (0, 60, 640, 360));
        assert_eq!(crop(1280, 720, 640, 360), (0, 0, 1280, 720));
    }

    #[test]
    fn scaling_crops_the_center() {
        // Left third red, middle blue, right third green; a 1:1 output of a
        // 3:1 frame is the middle third.
        let mut f = Bgra::new(6, 2);
        for y in 0..2 {
            for x in 0..6 {
                let px = match x / 2 {
                    0 => [0, 0, 255, 255],
                    1 => [255, 0, 0, 255],
                    _ => [0, 255, 0, 255],
                };
                f.data[(y * 6 + x) * 4..][..4].copy_from_slice(&px);
            }
        }
        let s = f.scaled(4, 4);
        assert!(s.data.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
        // Downscaling a solid frame keeps its color exactly.
        let s = solid(1920, 1080, [10, 20, 30, 255]).scaled(320, 240);
        assert_eq!((s.width, s.height, s.stride), (320, 240, 1280));
        assert!(s.data.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn rgba_swaps_channels_and_honors_the_stride() {
        let f = solid(3, 2, [1, 2, 3, 0]);
        let mut dst = vec![0xee; 16 * 2];
        to_rgba(&f, &mut dst, 16);
        assert_eq!(&dst[..12], &[3, 2, 1, 255, 3, 2, 1, 255, 3, 2, 1, 255]);
        assert_eq!(&dst[12..16], &[0xee; 4]);
        assert_eq!(&dst[16..20], &[3, 2, 1, 255]);
    }

    #[test]
    fn nv12_and_nv21_of_known_colors() {
        // BT.601 full range: white (255, 128, 128), red (77, 85, 255).
        let (w, h, stride) = (4usize, 2usize, 64usize);
        let chroma_offset = stride * h;
        let mut dst = vec![0; chroma_offset + stride];
        to_nv(
            &solid(4, 2, [255, 255, 255, 255]),
            &mut dst,
            stride,
            chroma_offset,
            stride,
            false,
        );
        assert!(dst[..w].iter().all(|&y| y == 255));
        assert_eq!(
            &dst[chroma_offset..chroma_offset + 4],
            &[128, 128, 128, 128]
        );
        to_nv(
            &solid(4, 2, [0, 0, 255, 255]),
            &mut dst,
            stride,
            chroma_offset,
            stride,
            false,
        );
        assert_eq!(dst[stride], 77);
        assert_eq!(&dst[chroma_offset..chroma_offset + 2], &[85, 255]);
        to_nv(
            &solid(4, 2, [0, 0, 255, 255]),
            &mut dst,
            stride,
            chroma_offset,
            stride,
            true,
        );
        assert_eq!(&dst[chroma_offset..chroma_offset + 2], &[255, 85]);
    }

    #[test]
    fn odd_sizes_average_partial_blocks() {
        let f = solid(3, 3, [0, 255, 0, 255]);
        let mut dst = vec![0; 64 * 3 + 64 * 2];
        to_nv(&f, &mut dst, 64, 64 * 3, 64, false);
        assert_eq!(dst[2], 149);
        assert_eq!(dst[64 * 2 + 2], 149);
        let c = &dst[64 * 3..];
        assert_eq!(&c[..4], &c[64..68]);
    }
}
