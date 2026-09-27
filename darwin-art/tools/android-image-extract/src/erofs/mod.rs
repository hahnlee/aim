//! EROFS reader: flat, inline and LZ4-compressed (full or compact index)
//! inodes. Layout reference: Linux v6.12 fs/erofs/erofs_fs.h and zmap.c.
pub mod directory;
pub mod inode;
mod zmap;

use crate::source::ReadAt;
use crate::tree::{Node, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, Tree};
use crate::{Result, add, invalid, le16, le32, mul};
pub use inode::Inode;
use std::io::Write;

pub const MAGIC: u32 = 0xe0f5_e1e2;
pub const INCOMPAT_ZERO_PADDING: u32 = 0x1;
pub const INCOMPAT_COMPR_CFGS: u32 = 0x2;
/// Extended-attribute name prefixes; xattrs are not read, so this is harmless.
const INCOMPAT_XATTR_PREFIXES: u32 = 0x40;
const SUPPORTED_INCOMPAT: u32 =
    INCOMPAT_ZERO_PADDING | INCOMPAT_COMPR_CFGS | INCOMPAT_XATTR_PREFIXES;
const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
const CHUNK: u64 = 1 << 20;

pub fn is_erofs(device: &dyn ReadAt) -> bool {
    device
        .read_vec(1024, 4)
        .is_ok_and(|b| le32(&b, 0).is_ok_and(|m| m == MAGIC))
}

pub struct Erofs<'a> {
    pub device: &'a dyn ReadAt,
    pub block_bits: u32,
    pub block: u64,
    meta: u64,
    pub root: u64,
    pub incompat: u32,
}

impl<'a> Erofs<'a> {
    pub fn open(device: &'a dyn ReadAt) -> Result<Self> {
        let sb = device.read_vec(1024, 128)?;
        if le32(&sb, 0)? != MAGIC {
            return Err(invalid("not an EROFS filesystem"));
        }
        let block_bits = u32::from(sb[12]);
        if !(9..=16).contains(&block_bits) {
            return Err(invalid("unsupported EROFS block size"));
        }
        let incompat = le32(&sb, 80)?;
        if incompat & !SUPPORTED_INCOMPAT != 0 {
            return Err(invalid(format!(
                "unsupported EROFS incompatible features {incompat:#x}"
            )));
        }
        // With per-algorithm configs, only LZ4 may be available.
        if incompat & INCOMPAT_COMPR_CFGS != 0 && le16(&sb, 84)? & !1 != 0 {
            return Err(invalid("EROFS uses a compression algorithm other than LZ4"));
        }
        Ok(Self {
            device,
            block_bits,
            block: 1 << block_bits,
            meta: u64::from(le32(&sb, 40)?),
            root: u64::from(le16(&sb, 14)?),
            incompat,
        })
    }

    pub fn inode(&self, nid: u64) -> Result<Inode> {
        let offset = add(
            mul(self.meta, self.block, "locating EROFS metadata")?,
            mul(nid, 32, "locating EROFS inode")?,
            "locating EROFS inode",
        )?;
        let length = (self.device.size().saturating_sub(offset)).min(64) as usize;
        inode::decode(nid, offset, &self.device.read_vec(offset, length)?)
    }

    fn tail_offset(&self, inode: &Inode) -> Result<u64> {
        add(
            inode.offset,
            inode.inode_size + inode.xattr_size,
            "locating EROFS inline data",
        )
    }

    /// Stream a file's decoded contents to `out`; returns the byte count.
    pub fn copy(&self, inode: &Inode, out: &mut dyn Write) -> Result<u64> {
        match inode.layout {
            inode::FLAT_PLAIN | inode::FLAT_INLINE => {
                let full = if inode.layout == inode::FLAT_PLAIN {
                    inode.size
                } else {
                    inode.size / self.block * self.block
                };
                let base = mul(inode.raw_blkaddr(), self.block, "locating flat data")?;
                let mut done = 0u64;
                while done < full {
                    let take = (full - done).min(CHUNK);
                    out.write_all(&self.device.read_vec(base + done, take as usize)?)?;
                    done += take;
                }
                let tail = inode.size - full;
                if tail != 0 {
                    if tail >= self.block {
                        return Err(invalid("EROFS inline tail exceeds a block"));
                    }
                    out.write_all(
                        &self
                            .device
                            .read_vec(self.tail_offset(inode)?, tail as usize)?,
                    )?;
                }
                Ok(inode.size)
            }
            inode::COMPRESSED_FULL | inode::COMPRESSED_COMPACT => zmap::copy(self, inode, out),
            layout => Err(invalid(format!(
                "unsupported EROFS data layout {layout} (nid {})",
                inode.nid
            ))),
        }
    }

    pub fn read(&self, inode: &Inode, limit: u64) -> Result<Vec<u8>> {
        if inode.size > limit {
            return Err(invalid("EROFS file exceeds the in-memory limit"));
        }
        let mut out = Vec::with_capacity(inode.size as usize);
        self.copy(inode, &mut out)?;
        Ok(out)
    }

    /// Directory entries, excluding `.` and `..`.
    pub fn entries(&self, directory: &Inode) -> Result<Vec<(Vec<u8>, u64)>> {
        if u32::from(directory.mode) & S_IFMT != S_IFDIR {
            return Err(invalid("EROFS inode is not a directory"));
        }
        let data = self.read(directory, MAX_DIRECTORY_BYTES)?;
        Ok(directory::entries(&data, self.block as usize)?
            .into_iter()
            .filter(|(_, name)| name != b"." && name != b"..")
            .map(|(nid, name)| (name, nid))
            .collect())
    }
}

impl Tree for Erofs<'_> {
    fn root(&self) -> u64 {
        self.root
    }
    fn node(&self, id: u64) -> Result<Node> {
        let inode = self.inode(id)?;
        Ok(Node {
            mode: u32::from(inode.mode),
            uid: inode.uid,
            gid: inode.gid,
            size: inode.size,
            nlink: inode.nlink,
        })
    }
    fn children(&self, id: u64) -> Result<Vec<(Vec<u8>, u64)>> {
        self.entries(&self.inode(id)?)
    }
    fn read_link(&self, id: u64) -> Result<Vec<u8>> {
        let inode = self.inode(id)?;
        if u32::from(inode.mode) & S_IFMT != S_IFLNK {
            return Err(invalid("EROFS inode is not a symlink"));
        }
        self.read(&inode, 4096)
    }
    fn copy_file(&self, id: u64, out: &mut dyn Write) -> Result<u64> {
        let inode = self.inode(id)?;
        if u32::from(inode.mode) & S_IFMT != S_IFREG {
            return Err(invalid("EROFS inode is not a regular file"));
        }
        self.copy(&inode, out)
    }
}

#[cfg(test)]
pub(crate) mod tests;
