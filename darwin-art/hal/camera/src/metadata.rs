//! `camera_metadata_t`, the packed buffer every `CameraMetadata` parcelable
//! carries (`system/media/camera/src/camera_metadata.c`), written and read
//! without libcamera_metadata (a system library a vendor process cannot
//! link).
//!
//! Layout: a 48-byte header, the entries (16 bytes each, sorted by tag),
//! then the data area, 8-byte aligned. A value of at most 4 bytes is stored
//! in its entry; a larger one at an 8-byte aligned offset of the data area.

use std::collections::BTreeMap;

use android_hardware_camera_metadata::aidl::android::hardware::camera::metadata::CameraMetadataTag::CameraMetadataTag as Tag;

/// `camera_metadata_type`.
pub mod kind {
    pub const BYTE: u8 = 0;
    pub const INT32: u8 = 1;
    pub const FLOAT: u8 = 2;
    pub const INT64: u8 = 3;
    pub const DOUBLE: u8 = 4;
    pub const RATIONAL: u8 = 5;
}

fn type_size(t: u8) -> Option<usize> {
    Some(match t {
        kind::BYTE => 1,
        kind::INT32 | kind::FLOAT => 4,
        kind::INT64 | kind::DOUBLE | kind::RATIONAL => 8,
        _ => return None,
    })
}

const HEADER: usize = 48;
const ENTRY: usize = 16;
const DATA_ALIGNMENT: usize = 8;
const VERSION: u32 = 1;
const FLAG_SORTED: u32 = 1;
/// `CAMERA_METADATA_INVALID_VENDOR_ID`.
const NO_VENDOR: u64 = u64::MAX;

fn align(v: usize, to: usize) -> usize {
    v.div_ceil(to) * to
}

/// One entry: its type and its values' bytes (little-endian).
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub kind: u8,
    pub bytes: Vec<u8>,
}

impl Value {
    pub fn count(&self) -> usize {
        self.bytes.len() / type_size(self.kind).unwrap()
    }

    pub fn i32s(&self) -> Vec<i32> {
        self.bytes
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }
}

/// A set of metadata entries, by tag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    entries: BTreeMap<u32, Value>,
}

fn tag(t: Tag) -> u32 {
    t.0 as u32
}

impl Metadata {
    pub fn new() -> Metadata {
        Metadata::default()
    }

    fn put(&mut self, t: Tag, kind: u8, bytes: Vec<u8>) -> &mut Self {
        self.entries.insert(tag(t), Value { kind, bytes });
        self
    }

    pub fn u8s(&mut self, t: Tag, v: &[u8]) -> &mut Self {
        self.put(t, kind::BYTE, v.to_vec())
    }

    pub fn i32s(&mut self, t: Tag, v: &[i32]) -> &mut Self {
        self.put(
            t,
            kind::INT32,
            v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        )
    }

    pub fn f32s(&mut self, t: Tag, v: &[f32]) -> &mut Self {
        self.put(
            t,
            kind::FLOAT,
            v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        )
    }

    pub fn i64s(&mut self, t: Tag, v: &[i64]) -> &mut Self {
        self.put(
            t,
            kind::INT64,
            v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        )
    }

    /// Rationals as (numerator, denominator) pairs.
    pub fn rationals(&mut self, t: Tag, v: &[(i32, i32)]) -> &mut Self {
        let bytes = v
            .iter()
            .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
            .collect();
        self.put(t, kind::RATIONAL, bytes)
    }

    pub fn get(&self, t: Tag) -> Option<&Value> {
        self.entries.get(&tag(t))
    }

    /// The first byte of a byte entry.
    pub fn u8(&self, t: Tag) -> Option<u8> {
        self.get(t)
            .filter(|v| v.kind == kind::BYTE)
            .and_then(|v| v.bytes.first().copied())
    }

    /// The first value of an int32 entry.
    pub fn i32(&self, t: Tag) -> Option<i32> {
        self.get(t)
            .filter(|v| v.kind == kind::INT32)
            .and_then(|v| v.i32s().first().copied())
    }

    pub fn tags(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.keys().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The packed `camera_metadata_t`, sorted, with no spare capacity.
    pub fn to_bytes(&self) -> Vec<u8> {
        let n = self.entries.len();
        let data_len: usize = self
            .entries
            .values()
            .map(|v| {
                if v.bytes.len() > 4 {
                    align(v.bytes.len(), DATA_ALIGNMENT)
                } else {
                    0
                }
            })
            .sum();
        let entries_start = HEADER;
        let data_start = align(entries_start + n * ENTRY, DATA_ALIGNMENT);
        let size = align(data_start + data_len, DATA_ALIGNMENT);
        let mut out = vec![0u8; size];
        let header = [
            size as u32,
            VERSION,
            FLAG_SORTED,
            n as u32,
            n as u32,
            entries_start as u32,
            data_len as u32,
            data_len as u32,
            data_start as u32,
            0,
        ];
        for (i, v) in header.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out[40..48].copy_from_slice(&NO_VENDOR.to_le_bytes());
        let mut offset = 0;
        for (i, (t, v)) in self.entries.iter().enumerate() {
            let e = &mut out[entries_start + i * ENTRY..][..ENTRY];
            e[0..4].copy_from_slice(&t.to_le_bytes());
            e[4..8].copy_from_slice(&(v.count() as u32).to_le_bytes());
            e[12] = v.kind;
            if v.bytes.len() <= 4 {
                e[8..8 + v.bytes.len()].copy_from_slice(&v.bytes);
            } else {
                e[8..12].copy_from_slice(&(offset as u32).to_le_bytes());
                out[data_start + offset..][..v.bytes.len()].copy_from_slice(&v.bytes);
                offset += align(v.bytes.len(), DATA_ALIGNMENT);
            }
        }
        out
    }

    /// Parse a packed `camera_metadata_t`; `None` when it is malformed.
    /// An empty buffer is empty metadata.
    pub fn parse(b: &[u8]) -> Option<Metadata> {
        let mut m = Metadata::new();
        if b.is_empty() {
            return Some(m);
        }
        let word = |i: usize| -> Option<usize> {
            Some(u32::from_le_bytes(b.get(i * 4..i * 4 + 4)?.try_into().ok()?) as usize)
        };
        let (size, count, entries_start) = (word(0)?, word(3)?, word(5)?);
        let (data_count, data_start) = (word(6)?, word(8)?);
        if b.len() < HEADER || size > b.len() || data_start.checked_add(data_count)? > size {
            return None;
        }
        for i in 0..count {
            let e = b.get(entries_start + i * ENTRY..)?.get(..ENTRY)?;
            let t = u32::from_le_bytes(e[0..4].try_into().unwrap());
            let n = u32::from_le_bytes(e[4..8].try_into().unwrap()) as usize;
            let k = e[12];
            let len = n.checked_mul(type_size(k)?)?;
            let bytes = if len <= 4 {
                e[8..8 + len].to_vec()
            } else {
                let off = u32::from_le_bytes(e[8..12].try_into().unwrap()) as usize;
                if off.checked_add(len)? > data_count {
                    return None;
                }
                b[data_start + off..][..len].to_vec()
            };
            m.entries.insert(t, Value { kind: k, bytes });
        }
        Some(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_like_libcamera_metadata() {
        let mut m = Metadata::new();
        m.i32s(
            Tag::ANDROID_SENSOR_INFO_ACTIVE_ARRAY_SIZE,
            &[0, 0, 1920, 1080],
        )
        .u8s(Tag::ANDROID_CONTROL_AE_MODE, &[1])
        .i64s(Tag::ANDROID_SENSOR_TIMESTAMP, &[123]);
        let b = m.to_bytes();
        // Header: size, version, sorted, 3 entries, data 16 + 8 bytes.
        let word = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!(word(0) as usize, b.len());
        assert_eq!((word(1), word(2), word(3), word(4)), (1, 1, 3, 3));
        assert_eq!((word(5), word(6), word(8)), (48, 24, 96));
        assert_eq!(b.len(), 96 + 24);
        assert_eq!(&b[40..48], &[0xff; 8]);
        // Entries are sorted by tag: control (0x1....) first. AE mode is
        // inline; the timestamp (sensor, 0xe....) goes before the active
        // array (sensor info, 0xf....) in the data area.
        let tag_at = |i: usize| u32::from_le_bytes(b[48 + i * 16..][..4].try_into().unwrap());
        assert_eq!(tag_at(0), tag(Tag::ANDROID_CONTROL_AE_MODE));
        assert_eq!(b[48 + 8], 1);
        assert_eq!(b[48 + 12], kind::BYTE);
        assert_eq!(tag_at(1), tag(Tag::ANDROID_SENSOR_TIMESTAMP));
        assert_eq!(&b[48 + 16 + 8..][..4], &[0; 4]);
        assert_eq!(&b[96..104], &123i64.to_le_bytes());
        assert_eq!(&b[48 + 32 + 8..][..4], &8u32.to_le_bytes());
        assert_eq!(Metadata::parse(&b), Some(m));
    }

    #[test]
    fn rationals_floats_and_empty() {
        let mut m = Metadata::new();
        m.rationals(Tag::ANDROID_CONTROL_AE_COMPENSATION_STEP, &[(1, 3)])
            .f32s(Tag::ANDROID_LENS_INFO_AVAILABLE_FOCAL_LENGTHS, &[2.5]);
        let p = Metadata::parse(&m.to_bytes()).unwrap();
        assert_eq!(p, m);
        assert_eq!(
            p.get(Tag::ANDROID_LENS_INFO_AVAILABLE_FOCAL_LENGTHS)
                .unwrap()
                .bytes,
            2.5f32.to_le_bytes()
        );
        assert_eq!(Metadata::new().to_bytes().len(), 48);
        assert!(Metadata::parse(&[]).unwrap().is_empty());
        assert!(Metadata::parse(&[1, 2, 3]).is_none());
    }

    #[test]
    fn lookups_check_the_type() {
        let mut a = Metadata::new();
        a.u8s(Tag::ANDROID_CONTROL_MODE, &[1])
            .i32s(Tag::ANDROID_JPEG_ORIENTATION, &[0]);
        a.i32s(Tag::ANDROID_JPEG_ORIENTATION, &[90]);
        assert_eq!(a.i32(Tag::ANDROID_JPEG_ORIENTATION), Some(90));
        assert_eq!(a.u8(Tag::ANDROID_CONTROL_MODE), Some(1));
        assert_eq!(a.u8(Tag::ANDROID_JPEG_ORIENTATION), None);
    }
}
