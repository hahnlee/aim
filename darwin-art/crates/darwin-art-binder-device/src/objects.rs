//! Structural Binder object validation before any handle/FD translation.
//! Follows binder_get_object's four-byte offsets and complete-object checks.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Binder,
    WeakBinder,
    Handle,
    WeakHandle,
    Fd,
    Buffer,
    FdArray,
}

impl Kind {
    pub const fn tag(self) -> u32 {
        let bytes = match self {
            Self::Binder => *b"sb*",
            Self::WeakBinder => *b"wb*",
            Self::Handle => *b"sh*",
            Self::WeakHandle => *b"wh*",
            Self::Fd => *b"fd*",
            Self::Buffer => *b"pt*",
            Self::FdArray => *b"fda",
        };
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], 0x85])
    }
    pub const fn size(self) -> usize {
        match self {
            Self::Buffer => 40,
            Self::FdArray => 32,
            _ => 24,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    OffsetTableSize,
    InvalidOffset,
    UnknownType(u32),
    IncompleteObject,
    OutOfMemory,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Object<'a> {
    kind: Kind,
    offset: usize,
    bytes: &'a [u8],
}

impl<'a> Object<'a> {
    pub fn kind(&self) -> Kind {
        self.kind
    }
    pub fn offset(&self) -> usize {
        self.offset
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// Validates the whole table before returning anything to an executor. Embedded
/// pointers are opaque bytes; parent/fixup and reference checks are separate.
pub fn validate<'a>(data: &'a [u8], offsets: &[u8]) -> Result<Vec<Object<'a>>, Error> {
    if offsets.len() % 8 != 0 {
        return Err(Error::OffsetTableSize);
    }
    let mut objects = Vec::new();
    let mut minimum = 0;
    for encoded in offsets.chunks_exact(8) {
        let offset = usize::try_from(u64::from_le_bytes(encoded.try_into().unwrap()))
            .map_err(|_| Error::InvalidOffset)?;
        if offset < minimum || offset % 4 != 0 {
            return Err(Error::InvalidOffset);
        }
        let header_end = offset.checked_add(4).ok_or(Error::InvalidOffset)?;
        let header = data
            .get(offset..header_end)
            .ok_or(Error::IncompleteObject)?;
        let tag = u32::from_le_bytes(header.try_into().unwrap());
        let kind = [
            Kind::Binder,
            Kind::WeakBinder,
            Kind::Handle,
            Kind::WeakHandle,
            Kind::Fd,
            Kind::Buffer,
            Kind::FdArray,
        ]
        .into_iter()
        .find(|kind| kind.tag() == tag)
        .ok_or(Error::UnknownType(tag))?;
        let end = offset
            .checked_add(kind.size())
            .ok_or(Error::InvalidOffset)?;
        let bytes = data.get(offset..end).ok_or(Error::IncompleteObject)?;
        objects.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        objects.push(Object {
            kind,
            offset,
            bytes,
        });
        minimum = end;
    }
    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn offset_bytes(offsets: &[u64]) -> Vec<u8> {
        offsets
            .iter()
            .flat_map(|offset| offset.to_le_bytes())
            .collect()
    }
    #[test]
    fn every_type_and_complete_length_at_four_byte_alignment() {
        for kind in [
            Kind::Binder,
            Kind::WeakBinder,
            Kind::Handle,
            Kind::WeakHandle,
            Kind::Fd,
            Kind::Buffer,
            Kind::FdArray,
        ] {
            let mut data = vec![0u8; 4 + kind.size()];
            data[4..8].copy_from_slice(&kind.tag().to_le_bytes());
            let offsets = offset_bytes(&[4]);
            for len in 0..data.len() {
                assert!(validate(&data[..len], &offsets).is_err());
            }
            let objects = validate(&data, &offsets).unwrap();
            assert_eq!(objects.len(), 1);
            assert_eq!(objects[0].kind, kind);
            assert_eq!(objects[0].bytes.as_ptr(), data[4..].as_ptr());
        }
    }
    #[test]
    fn malformed_offsets_unknown_types_overlap_and_duplicates() {
        let mut data = vec![0u8; 48];
        for offset in [0, 24] {
            data[offset..offset + 4].copy_from_slice(&Kind::Fd.tag().to_le_bytes());
        }
        assert_eq!(validate(&data, &[0]), Err(Error::OffsetTableSize));
        for offsets in [vec![1], vec![u64::MAX], vec![24, 0], vec![0, 0], vec![0, 4]] {
            assert!(validate(&data, &offset_bytes(&offsets)).is_err());
        }
        assert_eq!(validate(&data, &offset_bytes(&[0, 24])).unwrap().len(), 2);
        data[24..28].fill(0xff);
        assert_eq!(
            validate(&data, &offset_bytes(&[0, 24])),
            Err(Error::UnknownType(u32::MAX))
        );
        assert!(validate(&[], &[]).unwrap().is_empty());
    }
}
