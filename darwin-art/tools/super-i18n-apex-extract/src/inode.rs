//! EROFS on-disk inode decoding; never projects Darwin file ownership.
//! Layout reference: Linux v6.12 fs/erofs/erofs_fs.h,
//! erofs_inode_compact and erofs_inode_extended.
use crate::{add, invalid, le16, le32, le64, mul, Result};

#[derive(Clone, Debug)]
pub(super) struct Inode {
    pub nid: u64,
    pub offset: u64,
    pub size: u64,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub layout: u16,
    pub inode_size: u64,
    pub xattr_size: u64,
    pub compressed_blocks: u32,
}

impl Inode {
    pub fn persist(&self, output: &std::fs::File) -> std::io::Result<()> {
        use darwin_art_fs_broker::inode_metadata::{write_new, AndroidInodeMetadata};
        write_new(
            output,
            AndroidInodeMetadata {
                uid: self.uid,
                gid: self.gid,
                mode: u32::from(self.mode),
            },
        )
    }
}

pub(super) fn decode(nid: u64, offset: u64, bytes: &[u8]) -> Result<Inode> {
    let format = le16(bytes, 0)?;
    if format & !0xf != 0 {
        return Err(invalid("reserved EROFS inode format bits").into());
    }
    let extended = format & 1 != 0;
    let inode_size = if extended { 64 } else { 32 };
    if bytes.len() < inode_size {
        return Err(invalid("truncated EROFS inode").into());
    }
    let count = u64::from(le16(bytes, 2)?);
    Ok(Inode {
        nid,
        offset,
        size: if extended {
            le64(bytes, 8)?
        } else {
            u64::from(le32(bytes, 8)?)
        },
        mode: le16(bytes, 4)?,
        uid: if extended {
            le32(bytes, 24)?
        } else {
            u32::from(le16(bytes, 24)?)
        },
        gid: if extended {
            le32(bytes, 28)?
        } else {
            u32::from(le16(bytes, 26)?)
        },
        layout: (format >> 1) & 7,
        inode_size: inode_size as u64,
        xattr_size: if count == 0 {
            0
        } else {
            add(12, mul(count - 1, 4, "sizing xattrs")?, "sizing xattrs")?
        },
        compressed_blocks: le32(bytes, 16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_ownership_uses_independent_16_bit_fields() {
        let mut bytes = [0u8; 32];
        bytes[4..6].copy_from_slice(&0o100644u16.to_le_bytes());
        bytes[8..12].copy_from_slice(&123u32.to_le_bytes());
        bytes[24..26].copy_from_slice(&65534u16.to_le_bytes());
        bytes[26..28].copy_from_slice(&1000u16.to_le_bytes());
        let inode = decode(7, 64, &bytes).unwrap();
        assert_eq!(
            (inode.uid, inode.gid, inode.mode, inode.size),
            (65534, 1000, 0o100644, 123)
        );
        assert_eq!((inode.nid, inode.offset, inode.inode_size), (7, 64, 32));
    }
    #[test]
    fn extended_ownership_and_size_do_not_truncate() {
        let mut bytes = [0u8; 64];
        bytes[0..2].copy_from_slice(&7u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&2u16.to_le_bytes());
        bytes[8..16].copy_from_slice(&(1u64 << 35).to_le_bytes());
        bytes[24..28].copy_from_slice(&123456u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&234567u32.to_le_bytes());
        let inode = decode(0, 0, &bytes).unwrap();
        assert_eq!(
            (inode.uid, inode.gid, inode.size),
            (123456, 234567, 1u64 << 35)
        );
        assert_eq!(
            (inode.layout, inode.inode_size, inode.xattr_size),
            (3, 64, 16)
        );
        for size in 0..64 {
            assert!(decode(0, 0, &bytes[..size]).is_err());
        }
        bytes[0] = 16;
        assert!(decode(0, 0, &bytes).is_err());
    }
}
