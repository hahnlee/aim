//! Opaque, bounded attributes carried alongside one imported Binder FD.
//!
//! The attributes are transport metadata only.  This module intentionally
//! does not interpret their bytes or authenticate a carrier identity.

use std::os::fd::OwnedFd;

pub const MAX_ATTRIBUTES_BYTES: usize = 256;
pub const ENTRY_HEADER_BYTES: usize = 24;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    Truncated,
    Reserved,
    Overflow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescriptorAttributes(Box<[u8]>);

impl DescriptorAttributes {
    pub fn empty() -> Self {
        Self(Box::new([]))
    }

    pub fn new(bytes: Vec<u8>) -> Result<Self, Error> {
        if bytes.len() > MAX_ATTRIBUTES_BYTES {
            return Err(Error::TooLarge);
        }
        Ok(Self(bytes.into_boxed_slice()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

pub struct DescriptorBundle {
    offset: usize,
    ordinal: u64,
    descriptor: OwnedFd,
    attributes: DescriptorAttributes,
}

impl DescriptorBundle {
    pub fn new(
        offset: usize,
        ordinal: u64,
        descriptor: OwnedFd,
        attributes: DescriptorAttributes,
    ) -> Self {
        Self {
            offset,
            ordinal,
            descriptor,
            attributes,
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    pub fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }

    pub fn attributes(&self) -> &DescriptorAttributes {
        &self.attributes
    }

    pub fn into_parts(self) -> (usize, u64, OwnedFd, DescriptorAttributes) {
        (self.offset, self.ordinal, self.descriptor, self.attributes)
    }
}

pub fn encoded_len(attributes_len: usize) -> Result<usize, Error> {
    if attributes_len > MAX_ATTRIBUTES_BYTES {
        return Err(Error::TooLarge);
    }
    let unaligned = ENTRY_HEADER_BYTES
        .checked_add(attributes_len)
        .ok_or(Error::Overflow)?;
    unaligned
        .checked_add(7)
        .map(|length| length & !7)
        .ok_or(Error::Overflow)
}

pub fn encode(
    destination: &mut [u8],
    offset: usize,
    ordinal: u64,
    attributes: &DescriptorAttributes,
) -> Result<usize, Error> {
    let length = encoded_len(attributes.as_bytes().len())?;
    if destination.len() < length {
        return Err(Error::Truncated);
    }
    let offset = u64::try_from(offset).map_err(|_| Error::Overflow)?;
    destination[..length].fill(0);
    destination[..8].copy_from_slice(&offset.to_le_bytes());
    destination[8..16].copy_from_slice(&ordinal.to_le_bytes());
    destination[16..20].copy_from_slice(&(attributes.as_bytes().len() as u32).to_le_bytes());
    // bytes 20..24 are reserved and remain zero.
    destination[ENTRY_HEADER_BYTES..ENTRY_HEADER_BYTES + attributes.as_bytes().len()]
        .copy_from_slice(attributes.as_bytes());
    Ok(length)
}

pub struct Decoded<'a> {
    pub offset: usize,
    pub ordinal: u64,
    pub attributes: &'a [u8],
    pub encoded_len: usize,
}

pub fn decode(source: &[u8]) -> Result<Decoded<'_>, Error> {
    if source.len() < ENTRY_HEADER_BYTES {
        return Err(Error::Truncated);
    }
    let offset = usize::try_from(u64::from_le_bytes(source[..8].try_into().unwrap()))
        .map_err(|_| Error::Overflow)?;
    let ordinal = u64::from_le_bytes(source[8..16].try_into().unwrap());
    if source[20..24].iter().any(|byte| *byte != 0) {
        return Err(Error::Reserved);
    }
    let attributes_len = usize::try_from(u32::from_le_bytes(source[16..20].try_into().unwrap()))
        .map_err(|_| Error::Overflow)?;
    let encoded_len = encoded_len(attributes_len)?;
    if source.len() < encoded_len
        || source[ENTRY_HEADER_BYTES + attributes_len..encoded_len]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(Error::Truncated);
    }
    Ok(Decoded {
        offset,
        ordinal,
        attributes: &source[ENTRY_HEADER_BYTES..ENTRY_HEADER_BYTES + attributes_len],
        encoded_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_rejects_reserved_and_nonzero_padding() {
        let attributes = DescriptorAttributes::new(vec![1, 2, 3]).unwrap();
        let mut bytes = vec![0_u8; encoded_len(3).unwrap()];
        encode(&mut bytes, 4, 0, &attributes).unwrap();
        bytes[20] = 1;
        assert!(matches!(decode(&bytes), Err(Error::Reserved)));
        bytes[20] = 0;
        bytes[ENTRY_HEADER_BYTES + 3] = 1;
        assert!(matches!(decode(&bytes), Err(Error::Truncated)));
    }

    #[test]
    fn codec_rejects_oversized_attribute_length() {
        let mut bytes = vec![0_u8; ENTRY_HEADER_BYTES];
        bytes[16..20].copy_from_slice(&((MAX_ATTRIBUTES_BYTES as u32) + 1).to_le_bytes());
        assert!(matches!(decode(&bytes), Err(Error::TooLarge)));
    }
}
