//! Offline, read-only decoders for the pinned Android system-image archive:
//! the outer ZIP, GPT disks, dynamic (LP/super) partitions, EROFS, ext4,
//! legacy-LZ4 cpio ramdisks and APEX/CAPEX containers, plus a materializer
//! that writes their trees to a plain host directory.
//!
//! Inputs are never mounted or modified. Malformed or unsupported layouts are
//! hard errors, never silently skipped.

pub mod apex;
pub mod cpio;
pub mod erofs;
pub mod ext4;
pub mod gpt;
pub mod image;
pub mod inode_metadata;
pub mod lp;
pub mod lz4;
pub mod source;
pub mod tree;
pub mod x18;
pub mod zip;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

pub fn invalid(message: impl Into<String>) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into()
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    offset
        .checked_add(N)
        .and_then(|end| bytes.get(offset..end))
        .map(|value| value.try_into().unwrap())
        .ok_or_else(|| invalid(format!("truncated {N}-byte field at {offset}")))
}

pub fn le16(bytes: &[u8], offset: usize) -> Result<u16> {
    field(bytes, offset).map(u16::from_le_bytes)
}

pub fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    field(bytes, offset).map(u32::from_le_bytes)
}

pub fn le64(bytes: &[u8], offset: usize) -> Result<u64> {
    field(bytes, offset).map(u64::from_le_bytes)
}

pub fn add(a: u64, b: u64, what: &str) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| invalid(format!("overflow {what}")))
}

pub fn mul(a: u64, b: u64, what: &str) -> Result<u64> {
    a.checked_mul(b)
        .ok_or_else(|| invalid(format!("overflow {what}")))
}

pub fn align(value: u64, alignment: u64) -> Result<u64> {
    if !alignment.is_power_of_two() {
        return Err(invalid("invalid alignment"));
    }
    add(value, alignment - 1, "aligning").map(|v| v & !(alignment - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_reject_truncation() {
        assert_eq!(le16(&[0x34, 0x12], 0).unwrap(), 0x1234);
        assert_eq!(le32(&[0x78, 0x56, 0x34, 0x12], 0).unwrap(), 0x1234_5678);
        assert!(le64(&[0; 7], 0).is_err());
        assert!(le16(&[0; 2], usize::MAX).is_err());
        assert_eq!(align(13, 8).unwrap(), 16);
        assert!(align(1, 3).is_err());
    }
}
