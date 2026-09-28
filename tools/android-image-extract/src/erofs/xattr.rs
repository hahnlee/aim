//! EROFS extended attributes, read only for `security.selinux`.
//! Layout reference: Linux v6.12 fs/erofs/erofs_fs.h and xattr.c.
use super::{Erofs, Inode};
use crate::source::ReadAt;
use crate::{Result, add, invalid, le16, le32, mul};

const INDEX_SECURITY: u8 = 6;
/// `e_name_index` flag selecting a long prefix from the superblock table.
const LONG_PREFIX: u8 = 0x80;
const MAX_PREFIXES: u8 = 0x80;
const IBODY_HEADER: usize = 12;
const ENTRY_HEADER: usize = 4;

/// One long-prefix record: `base_index` and the infix appended to it.
#[derive(Clone, Debug)]
pub struct Prefix {
    base: u8,
    infix: Vec<u8>,
}

/// Read `count` long prefixes starting at byte `start` of the metadata
/// (the table lives on the device itself: packed inodes are unsupported).
pub fn prefixes(device: &dyn ReadAt, start: u64, count: u8) -> Result<Vec<Prefix>> {
    if count > MAX_PREFIXES {
        return Err(invalid("too many EROFS long xattr prefixes"));
    }
    let mut at = start;
    let mut out = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        at = crate::align(at, 4)?;
        let length = match le16(&device.read_vec(at, 2)?, 0)? {
            0 => 1 << 16,
            length => usize::from(length),
        };
        let record = device.read_vec(add(at, 2, "reading EROFS xattr prefix")?, length)?;
        out.push(Prefix {
            base: record[0],
            infix: record[1..].to_vec(),
        });
        at += 2 + length as u64;
    }
    Ok(out)
}

impl Erofs<'_> {
    /// Whether an entry with `index` and `name` is `security.selinux`.
    fn is_selinux(&self, index: u8, name: &[u8]) -> Result<bool> {
        if index & LONG_PREFIX == 0 {
            return Ok(index == INDEX_SECURITY && name == b"selinux");
        }
        let prefix = self
            .prefixes
            .get(usize::from(index & !LONG_PREFIX))
            .ok_or_else(|| invalid("EROFS xattr names a missing long prefix"))?;
        Ok(prefix.base == INDEX_SECURITY
            && name.len() + prefix.infix.len() == b"selinux".len()
            && b"selinux".starts_with(&prefix.infix)
            && b"selinux".ends_with(name))
    }

    /// Decode one `erofs_xattr_entry` at the start of `bytes`: returns its
    /// padded length and, if it is `security.selinux`, its value.
    fn entry(&self, bytes: &[u8]) -> Result<(usize, Option<Vec<u8>>)> {
        if bytes.len() < ENTRY_HEADER {
            return Err(invalid("truncated EROFS xattr entry"));
        }
        let name_len = usize::from(bytes[0]);
        let index = bytes[1];
        let value_len = usize::from(le16(bytes, 2)?);
        let end = ENTRY_HEADER + name_len + value_len;
        let (name, value) = bytes
            .get(ENTRY_HEADER..end)
            .ok_or_else(|| invalid("truncated EROFS xattr entry"))?
            .split_at(name_len);
        let found = self.is_selinux(index, name)?.then(|| value.to_vec());
        Ok((end.div_ceil(4) * 4, found))
    }

    fn shared(&self, id: u32) -> Result<Option<Vec<u8>>> {
        let at = add(
            mul(
                self.xattr_blkaddr,
                self.block,
                "locating EROFS shared xattrs",
            )?,
            mul(u64::from(id), 4, "locating EROFS shared xattr")?,
            "locating EROFS shared xattr",
        )?;
        let header = self.device.read_vec(at, ENTRY_HEADER)?;
        let length = ENTRY_HEADER + usize::from(header[0]) + usize::from(le16(&header, 2)?);
        Ok(self.entry(&self.device.read_vec(at, length)?)?.1)
    }

    /// The inode's `security.selinux` value: inline entries, then shared.
    pub fn label(&self, inode: &Inode) -> Result<Option<Vec<u8>>> {
        if inode.xattr_size == 0 {
            return Ok(None);
        }
        let at = add(inode.offset, inode.inode_size, "locating EROFS xattrs")?;
        let area = self.device.read_vec(at, inode.xattr_size as usize)?;
        let shared = usize::from(area[4]);
        let mut offset = IBODY_HEADER + shared * 4;
        if offset > area.len() {
            return Err(invalid("EROFS shared xattr ids exceed the xattr area"));
        }
        while offset < area.len() {
            let (length, value) = self.entry(&area[offset..])?;
            if value.is_some() {
                return Ok(value);
            }
            offset += length;
        }
        for i in 0..shared {
            let value = self.shared(le32(&area, IBODY_HEADER + i * 4)?)?;
            if value.is_some() {
                return Ok(value);
            }
        }
        Ok(None)
    }
}
