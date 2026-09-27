//! Buffer metadata: the mutable part kept in the buffer's memory
//! ([`SharedMetadata`]) and the `StandardMetadataType` encoding of
//! `IMapperMetadataTypes.h`.

use crate::handle::Handle;
use crate::layout::{self, Layout, Plane};

/// Largest HDR dynamic metadata blob kept per buffer.
pub const MAX_BLOB: usize = 2048;
/// Largest reserved region (`BufferDescriptorInfo.reservedSize`) we allocate.
pub const MAX_RESERVED_SIZE: u64 = 1 << 20;

const MAGIC: u32 = 0x4447_4d31;
const VERSION: u32 = 1;

/// The shared, mutable metadata at the start of the metadata area. Every
/// process that imports the buffer maps it, so a value set anywhere is seen
/// everywhere.
#[repr(C)]
pub struct SharedMetadata {
    pub magic: u32,
    pub version: u32,
    pub name_len: u32,
    pub name: [u8; 128],
    pub dataspace: i32,
    pub blend_mode: i32,
    pub has_smpte2086: u32,
    /// Red, green, blue and white point (x, y), then max and min luminance.
    pub smpte2086: [f32; 10],
    pub has_cta861_3: u32,
    /// Max content and max frame-average light level.
    pub cta861_3: [f32; 2],
    /// -1 when absent.
    pub smpte2094_40_len: i32,
    pub smpte2094_40: [u8; MAX_BLOB],
    pub smpte2094_10_len: i32,
    pub smpte2094_10: [u8; MAX_BLOB],
}

const _: () = assert!(size_of::<SharedMetadata>() <= crate::handle::RESERVED_OFFSET as usize);

impl SharedMetadata {
    /// Initialize a new buffer's metadata.
    pub fn init(&mut self, name: &[u8]) {
        let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
        let n = name.len().min(self.name.len());
        self.magic = MAGIC;
        self.version = VERSION;
        self.name_len = n as u32;
        self.name[..n].copy_from_slice(&name[..n]);
        self.dataspace = 0;
        self.blend_mode = 0;
        self.has_smpte2086 = 0;
        self.has_cta861_3 = 0;
        self.smpte2094_40_len = -1;
        self.smpte2094_10_len = -1;
    }

    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC && self.version == VERSION
    }

    pub fn name(&self) -> &[u8] {
        &self.name[..(self.name_len as usize).min(self.name.len())]
    }
}

/// `AIMapper_Error` values.
pub mod error {
    pub const BAD_BUFFER: i32 = 2;
    pub const BAD_VALUE: i32 = 3;
    pub const UNSUPPORTED: i32 = 7;
}

/// `StandardMetadataType` values.
pub mod standard {
    pub const BUFFER_ID: i64 = 1;
    pub const NAME: i64 = 2;
    pub const WIDTH: i64 = 3;
    pub const HEIGHT: i64 = 4;
    pub const LAYER_COUNT: i64 = 5;
    pub const PIXEL_FORMAT_REQUESTED: i64 = 6;
    pub const PIXEL_FORMAT_FOURCC: i64 = 7;
    pub const PIXEL_FORMAT_MODIFIER: i64 = 8;
    pub const USAGE: i64 = 9;
    pub const ALLOCATION_SIZE: i64 = 10;
    pub const PROTECTED_CONTENT: i64 = 11;
    pub const COMPRESSION: i64 = 12;
    pub const INTERLACED: i64 = 13;
    pub const CHROMA_SITING: i64 = 14;
    pub const PLANE_LAYOUTS: i64 = 15;
    pub const CROP: i64 = 16;
    pub const DATASPACE: i64 = 17;
    pub const BLEND_MODE: i64 = 18;
    pub const SMPTE2086: i64 = 19;
    pub const CTA861_3: i64 = 20;
    pub const SMPTE2094_40: i64 = 21;
    pub const SMPTE2094_10: i64 = 22;
    pub const STRIDE: i64 = 23;
}

pub const STANDARD_NAME: &str = "android.hardware.graphics.common.StandardMetadataType";
const COMPRESSION_NAME: &str = "android.hardware.graphics.common.Compression";
const INTERLACED_NAME: &str = "android.hardware.graphics.common.Interlaced";
const CHROMA_SITING_NAME: &str = "android.hardware.graphics.common.ChromaSiting";
const COMPONENT_NAME: &str = "android.hardware.graphics.common.PlaneLayoutComponentType";
const CHROMA_SITING_SITED_INTERSTITIAL: i64 = 2;

/// Every standard type: (value, settable).
pub const SUPPORTED: [(i64, bool); 23] = {
    let mut v = [(0, false); 23];
    let mut i = 0;
    while i < 23 {
        let t = i as i64 + 1;
        v[i] = (
            t,
            t == standard::DATASPACE
                || t == standard::BLEND_MODE
                || t == standard::SMPTE2086
                || t == standard::CTA861_3
                || t == standard::SMPTE2094_40
                || t == standard::SMPTE2094_10,
        );
        i += 1;
    }
    v
};

/// `MetadataWriter`: counts the size it needs and writes while it fits.
struct Writer<'a> {
    dest: &'a mut [u8],
    size: usize,
}

impl Writer<'_> {
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        let end = self.size + b.len();
        if end <= self.dest.len() {
            self.dest[self.size..end].copy_from_slice(b);
        }
        self.size = end;
        self
    }

    fn i32(&mut self, v: i32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    fn i64(&mut self, v: i64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    fn f32(&mut self, v: f32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    fn string(&mut self, s: &[u8]) -> &mut Self {
        self.i64(s.len() as i64).bytes(s)
    }

    fn header(&mut self, t: i64) -> &mut Self {
        self.string(STANDARD_NAME.as_bytes()).i64(t)
    }

    fn extendable(&mut self, name: &str, value: i64) -> &mut Self {
        self.string(name.as_bytes()).i64(value)
    }

    fn planes(&mut self, planes: &[Plane]) -> &mut Self {
        self.i64(planes.len() as i64);
        for p in planes {
            self.i64(p.components.len() as i64);
            for &(kind, offset, size) in &p.components {
                self.extendable(COMPONENT_NAME, kind).i64(offset).i64(size);
            }
            for v in [
                p.offset,
                p.sample_increment_bits,
                p.stride_bytes,
                p.width_samples,
                p.height_samples,
                p.total_size,
                p.horizontal_subsampling,
                p.vertical_subsampling,
            ] {
                self.i64(v as i64);
            }
        }
        self
    }
}

/// `getStandardMetadata`: the encoded value's size (written into `dest`
/// when it fits), 0 for an absent optional value, or `-AIMapper_Error`.
pub fn get_standard(h: &Handle, m: &SharedMetadata, t: i64, dest: &mut [u8]) -> i32 {
    let Some(layout) = h.layout() else {
        return -error::BAD_BUFFER;
    };
    let mut w = Writer { dest, size: 0 };
    match t {
        standard::BUFFER_ID => w.header(t).i64(h.id as i64),
        standard::NAME => w.header(t).string(m.name()),
        standard::WIDTH => w.header(t).i64(h.width as i64),
        standard::HEIGHT => w.header(t).i64(h.height as i64),
        standard::LAYER_COUNT => w.header(t).i64(h.layer_count as i64),
        standard::PIXEL_FORMAT_REQUESTED => w.header(t).i32(h.requested_format),
        standard::PIXEL_FORMAT_FOURCC => w.header(t).i32(layout::fourcc(h.format) as i32),
        standard::PIXEL_FORMAT_MODIFIER => w.header(t).i64(0),
        standard::USAGE => w.header(t).i64(h.usage as i64),
        standard::ALLOCATION_SIZE => w.header(t).i64(h.data_size as i64),
        standard::PROTECTED_CONTENT => w.header(t).i64(0),
        standard::COMPRESSION => w.header(t).extendable(COMPRESSION_NAME, 0),
        standard::INTERLACED => w.header(t).extendable(INTERLACED_NAME, 0),
        standard::CHROMA_SITING => {
            let siting = if layout.planes.len() > 1 {
                CHROMA_SITING_SITED_INTERSTITIAL
            } else {
                0
            };
            w.header(t).extendable(CHROMA_SITING_NAME, siting)
        }
        standard::PLANE_LAYOUTS => w.header(t).planes(&layout.planes),
        standard::CROP => crop(w.header(t), &layout),
        standard::DATASPACE => w.header(t).i32(m.dataspace),
        standard::BLEND_MODE => w.header(t).i32(m.blend_mode),
        standard::SMPTE2086 if m.has_smpte2086 == 0 => return 0,
        standard::SMPTE2086 => {
            w.header(t);
            for v in m.smpte2086 {
                w.f32(v);
            }
            &mut w
        }
        standard::CTA861_3 if m.has_cta861_3 == 0 => return 0,
        standard::CTA861_3 => w.header(t).f32(m.cta861_3[0]).f32(m.cta861_3[1]),
        standard::SMPTE2094_40 | standard::SMPTE2094_10 => {
            let (len, blob) = if t == standard::SMPTE2094_40 {
                (m.smpte2094_40_len, &m.smpte2094_40)
            } else {
                (m.smpte2094_10_len, &m.smpte2094_10)
            };
            if len < 0 {
                return 0;
            }
            w.header(t).string(&blob[..(len as usize).min(MAX_BLOB)])
        }
        standard::STRIDE => w.header(t).i32(h.stride as i32),
        _ => return -error::UNSUPPORTED,
    };
    i32::try_from(w.size).unwrap_or(-error::BAD_VALUE)
}

fn crop<'w, 'a>(w: &'w mut Writer<'a>, layout: &Layout) -> &'w mut Writer<'a> {
    w.i64(layout.planes.len() as i64);
    for p in &layout.planes {
        w.i32(0)
            .i32(0)
            .i32(p.width_samples as i32)
            .i32(p.height_samples as i32);
    }
    w
}

/// `MetadataReader`.
struct Reader<'a> {
    src: &'a [u8],
    ok: bool,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        if self.ok && self.src.len() >= n {
            let (head, rest) = self.src.split_at(n);
            self.src = rest;
            head
        } else {
            self.ok = false;
            &[]
        }
    }

    fn i32(&mut self) -> i32 {
        self.take(4).try_into().map_or(0, i32::from_le_bytes)
    }

    fn i64(&mut self) -> i64 {
        self.take(8).try_into().map_or(0, i64::from_le_bytes)
    }

    fn f32(&mut self) -> f32 {
        self.take(4).try_into().map_or(0.0, f32::from_le_bytes)
    }

    fn string(&mut self) -> &'a [u8] {
        let n = self.i64();
        if n < 0 {
            self.ok = false;
            return &[];
        }
        self.take(n as usize)
    }

    fn header(&mut self, t: i64) {
        if self.string() != STANDARD_NAME.as_bytes() || self.i64() != t {
            self.ok = false;
        }
    }
}

/// `setStandardMetadata`: 0 or an `AIMapper_Error`.
pub fn set_standard(m: &mut SharedMetadata, t: i64, src: &[u8]) -> i32 {
    if !SUPPORTED.iter().any(|&(v, settable)| v == t && settable) {
        return error::UNSUPPORTED;
    }
    // An empty value clears an optional type.
    if src.is_empty() {
        match t {
            standard::SMPTE2086 => m.has_smpte2086 = 0,
            standard::CTA861_3 => m.has_cta861_3 = 0,
            standard::SMPTE2094_40 => m.smpte2094_40_len = -1,
            standard::SMPTE2094_10 => m.smpte2094_10_len = -1,
            _ => return error::BAD_VALUE,
        }
        return 0;
    }
    let mut r = Reader { src, ok: true };
    r.header(t);
    match t {
        standard::DATASPACE | standard::BLEND_MODE => {
            let v = r.i32();
            if !r.ok {
                return error::BAD_VALUE;
            }
            if t == standard::DATASPACE {
                m.dataspace = v;
            } else {
                m.blend_mode = v;
            }
        }
        standard::SMPTE2086 => {
            let mut v = [0f32; 10];
            for x in &mut v {
                *x = r.f32();
            }
            if !r.ok {
                return error::BAD_VALUE;
            }
            m.smpte2086 = v;
            m.has_smpte2086 = 1;
        }
        standard::CTA861_3 => {
            let v = [r.f32(), r.f32()];
            if !r.ok {
                return error::BAD_VALUE;
            }
            m.cta861_3 = v;
            m.has_cta861_3 = 1;
        }
        _ => {
            let blob = r.string();
            if !r.ok || blob.len() > MAX_BLOB {
                return error::BAD_VALUE;
            }
            let (len, dest) = if t == standard::SMPTE2094_40 {
                (&mut m.smpte2094_40_len, &mut m.smpte2094_40)
            } else {
                (&mut m.smpte2094_10_len, &mut m.smpte2094_10)
            };
            dest[..blob.len()].copy_from_slice(blob);
            *len = blob.len() as i32;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Descriptor, format};

    fn fixture() -> (Handle, Box<SharedMetadata>) {
        let desc = Descriptor {
            width: 32,
            height: 16,
            layer_count: 1,
            format: format::RGBA_8888,
            usage: 0x333,
            reserved_size: 0,
        };
        let h = Handle::new(&desc, &desc.layout().unwrap(), 5);
        // SAFETY: SharedMetadata is plain data; all-zero is a valid value.
        let mut m: Box<SharedMetadata> = unsafe { Box::new(std::mem::zeroed()) };
        m.init(b"test\0junk");
        (h, m)
    }

    fn get(h: &Handle, m: &SharedMetadata, t: i64) -> Vec<u8> {
        let n = get_standard(h, m, t, &mut []);
        assert!(n >= 0, "type {t}: {n}");
        let mut v = vec![0; n as usize];
        assert_eq!(get_standard(h, m, t, &mut v), n);
        v
    }

    fn header(t: i64) -> Vec<u8> {
        let mut v = (STANDARD_NAME.len() as i64).to_le_bytes().to_vec();
        v.extend_from_slice(STANDARD_NAME.as_bytes());
        v.extend_from_slice(&t.to_le_bytes());
        v
    }

    #[test]
    fn encodes_like_imapper_metadata_types() {
        let (h, m) = fixture();
        let mut want = header(standard::WIDTH);
        want.extend_from_slice(&32i64.to_le_bytes());
        assert_eq!(get(&h, &m, standard::WIDTH), want);
        let mut want = header(standard::NAME);
        want.extend_from_slice(&4i64.to_le_bytes());
        want.extend_from_slice(b"test");
        assert_eq!(get(&h, &m, standard::NAME), want);
        let mut want = header(standard::STRIDE);
        want.extend_from_slice(&(h.stride as i32).to_le_bytes());
        assert_eq!(get(&h, &m, standard::STRIDE), want);
        // One plane with four components.
        let planes = get(&h, &m, standard::PLANE_LAYOUTS);
        let body = &planes[header(0).len()..];
        assert_eq!(&body[..8], &1i64.to_le_bytes());
        assert_eq!(&body[8..16], &4i64.to_le_bytes());
        // Absent optional values encode as nothing.
        assert_eq!(get_standard(&h, &m, standard::SMPTE2086, &mut []), 0);
        assert_eq!(get_standard(&h, &m, 99, &mut []), -error::UNSUPPORTED);
        // Every supported type encodes.
        for (t, _) in SUPPORTED {
            assert!(get_standard(&h, &m, t, &mut []) >= 0);
        }
    }

    #[test]
    fn settable_types_round_trip() {
        let (h, mut m) = fixture();
        let mut v = header(standard::DATASPACE);
        v.extend_from_slice(&0x0891_0000i32.to_le_bytes());
        assert_eq!(set_standard(&mut m, standard::DATASPACE, &v), 0);
        assert_eq!(get(&h, &m, standard::DATASPACE), v);

        let mut v = header(standard::CTA861_3);
        v.extend_from_slice(&1000f32.to_le_bytes());
        v.extend_from_slice(&400f32.to_le_bytes());
        assert_eq!(set_standard(&mut m, standard::CTA861_3, &v), 0);
        assert_eq!(get(&h, &m, standard::CTA861_3), v);
        assert_eq!(set_standard(&mut m, standard::CTA861_3, &[]), 0);
        assert_eq!(get_standard(&h, &m, standard::CTA861_3, &mut []), 0);

        let mut v = header(standard::SMPTE2094_40);
        v.extend_from_slice(&3i64.to_le_bytes());
        v.extend_from_slice(&[1, 2, 3]);
        assert_eq!(set_standard(&mut m, standard::SMPTE2094_40, &v), 0);
        assert_eq!(get(&h, &m, standard::SMPTE2094_40), v);

        // Wrong header, read-only and unknown types.
        assert_eq!(
            set_standard(&mut m, standard::BLEND_MODE, &v),
            error::BAD_VALUE
        );
        assert_eq!(
            set_standard(&mut m, standard::WIDTH, &v),
            error::UNSUPPORTED
        );
        assert_eq!(set_standard(&mut m, 99, &v), error::UNSUPPORTED);
    }
}
