//! Read-only ext2/3/4 reader for APEX payloads: extent trees and legacy
//! indirect block maps, fast and slow symlinks, linear (and htree) directories.
//! Generalized from `tools/apex-ext2-extract` to any `ReadAt` source.
use crate::source::ReadAt;
use crate::tree::{Node, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, Tree};
use crate::{Result, add, invalid, le16, le32, mul};
use std::io::Write;

pub const MAGIC: u16 = 0xef53;
const INCOMPAT_FILETYPE: u32 = 0x2;
const INCOMPAT_EXTENTS: u32 = 0x40;
const INCOMPAT_64BIT: u32 = 0x80;
const INCOMPAT_MMP: u32 = 0x100;
const INCOMPAT_FLEX_BG: u32 = 0x200;
const INCOMPAT_EA_INODE: u32 = 0x400;
const INCOMPAT_CSUM_SEED: u32 = 0x2000;
const INCOMPAT_LARGEDIR: u32 = 0x4000;
const INCOMPAT_CASEFOLD: u32 = 0x20000;
const SUPPORTED_INCOMPAT: u32 = INCOMPAT_FILETYPE
    | INCOMPAT_EXTENTS
    | INCOMPAT_64BIT
    | INCOMPAT_MMP
    | INCOMPAT_FLEX_BG
    | INCOMPAT_EA_INODE
    | INCOMPAT_CSUM_SEED
    | INCOMPAT_LARGEDIR
    | INCOMPAT_CASEFOLD;
const EXTENTS_FL: u32 = 0x0008_0000;
const INLINE_DATA_FL: u32 = 0x1000_0000;
const ROOT: u64 = 2;
const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RUNS: usize = 4_000_000;

const XATTR_MAGIC: u32 = 0xea02_0000;
const XATTR_INDEX_SECURITY: u8 = 6;

/// Walk `ext4_xattr_entry`s in `area` from `start` up to the zero terminator
/// and return the `security.selinux` value; value offsets are relative to
/// `area`.
fn find_xattr(area: &[u8], start: usize) -> Result<Option<Vec<u8>>> {
    let mut at = start;
    while at + 4 <= area.len() && le32(area, at)? != 0 {
        if at + 16 > area.len() {
            return Err(invalid("truncated ext4 xattr entry"));
        }
        let name_len = usize::from(area[at]);
        let index = area[at + 1];
        let offset = usize::from(le16(area, at + 2)?);
        let inum = le32(area, at + 4)?;
        let size = le32(area, at + 8)? as usize;
        let name = area
            .get(at + 16..at + 16 + name_len)
            .ok_or_else(|| invalid("truncated ext4 xattr name"))?;
        if index == XATTR_INDEX_SECURITY && name == b"selinux" {
            if inum != 0 {
                return Err(invalid(
                    "ext4 security.selinux stored in an EA inode is unsupported",
                ));
            }
            return offset
                .checked_add(size)
                .and_then(|end| area.get(offset..end))
                .map(|value| Some(value.to_vec()))
                .ok_or_else(|| invalid("ext4 xattr value lies outside its area"));
        }
        at += (16 + name_len).div_ceil(4) * 4;
    }
    Ok(None)
}

pub fn is_ext4(device: &dyn ReadAt) -> bool {
    device
        .read_vec(1024 + 0x38, 2)
        .is_ok_and(|b| le16(&b, 0).is_ok_and(|m| m == MAGIC))
}

#[derive(Clone, Debug)]
pub struct Inode {
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub nlink: u16,
    flags: u32,
    blocks: u32,
    file_acl: u64,
    block: [u8; 60],
}

/// `count` blocks from logical block `logical`; `None` is a hole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    logical: u64,
    physical: Option<u64>,
    count: u64,
}

pub struct Ext4<'a> {
    device: &'a dyn ReadAt,
    pub block: u64,
    inode_size: u64,
    inodes_per_group: u64,
    inodes_count: u64,
    descriptor_size: u64,
    descriptors: u64,
    wide: bool,
}

impl<'a> Ext4<'a> {
    pub fn open(device: &'a dyn ReadAt) -> Result<Self> {
        let sb = device.read_vec(1024, 1024)?;
        if le16(&sb, 0x38)? != MAGIC {
            return Err(invalid("not an ext2/3/4 filesystem"));
        }
        let log = le32(&sb, 0x18)?;
        if log > 6 {
            return Err(invalid("unsupported ext4 block size"));
        }
        let block = 1024u64 << log;
        let incompat = le32(&sb, 0x60)?;
        if incompat & !SUPPORTED_INCOMPAT != 0 {
            return Err(invalid(format!(
                "unsupported ext4 incompatible features {incompat:#x}"
            )));
        }
        let inode_size = if le32(&sb, 0x4c)? == 0 {
            128
        } else {
            u64::from(le16(&sb, 0x58)?)
        };
        if inode_size < 128 || inode_size > block || !inode_size.is_power_of_two() {
            return Err(invalid("invalid ext4 inode size"));
        }
        let inodes_per_group = u64::from(le32(&sb, 0x28)?);
        if inodes_per_group == 0 {
            return Err(invalid("invalid ext4 inodes per group"));
        }
        let wide = incompat & INCOMPAT_64BIT != 0;
        let descriptor_size = if wide {
            u64::from(le16(&sb, 0xfe)?).max(64)
        } else {
            32
        };
        if descriptor_size > block || descriptor_size % 8 != 0 {
            return Err(invalid("invalid ext4 group descriptor size"));
        }
        let first_data_block = u64::from(le32(&sb, 0x14)?);
        Ok(Self {
            device,
            block,
            inode_size,
            inodes_per_group,
            inodes_count: u64::from(le32(&sb, 0)?),
            descriptor_size,
            descriptors: mul(first_data_block + 1, block, "locating ext4 descriptors")?,
            wide,
        })
    }

    pub fn inode(&self, number: u64) -> Result<Inode> {
        let b = self.device.read_vec(self.inode_offset(number)?, 128)?;
        Ok(Inode {
            mode: le16(&b, 0)?,
            uid: u32::from(le16(&b, 2)?) | u32::from(le16(&b, 0x78)?) << 16,
            gid: u32::from(le16(&b, 0x18)?) | u32::from(le16(&b, 0x7a)?) << 16,
            size: u64::from(le32(&b, 4)?) | u64::from(le32(&b, 0x6c)?) << 32,
            nlink: le16(&b, 0x1a)?,
            flags: le32(&b, 0x20)?,
            blocks: le32(&b, 0x1c)?,
            file_acl: u64::from(le32(&b, 0x68)?) | u64::from(le16(&b, 0x76)?) << 32,
            block: b[0x28..0x28 + 60].try_into().unwrap(),
        })
    }

    /// The `security.selinux` value: in-inode xattrs first, then the
    /// external xattr block.
    pub fn label(&self, number: u64) -> Result<Option<Vec<u8>>> {
        let raw = self
            .device
            .read_vec(self.inode_offset(number)?, self.inode_size as usize)?;
        if raw.len() > 128 {
            let start = 128 + usize::from(le16(&raw, 0x80)?);
            if start > raw.len() {
                return Err(invalid(format!(
                    "ext4 inode {number}: invalid i_extra_isize"
                )));
            }
            if start + 4 <= raw.len() && le32(&raw, start)? == XATTR_MAGIC {
                // In-inode values are relative to the first entry.
                if let Some(value) = find_xattr(&raw[start + 4..], 0)? {
                    return Ok(Some(value));
                }
            }
        }
        let file_acl = u64::from(le32(&raw, 0x68)?) | u64::from(le16(&raw, 0x76)?) << 32;
        if file_acl == 0 {
            return Ok(None);
        }
        let block = self.device.read_vec(
            mul(file_acl, self.block, "locating ext4 xattr block")?,
            self.block as usize,
        )?;
        if le32(&block, 0)? != XATTR_MAGIC || le32(&block, 8)? != 1 {
            return Err(invalid(format!("ext4 inode {number}: invalid xattr block")));
        }
        // Block values are relative to the block start; entries follow the
        // 32-byte header.
        find_xattr(&block, 32)
    }

    fn inode_offset(&self, number: u64) -> Result<u64> {
        if number == 0 || number > self.inodes_count {
            return Err(invalid(format!("ext4 inode {number} out of range")));
        }
        let group = (number - 1) / self.inodes_per_group;
        let index = (number - 1) % self.inodes_per_group;
        let descriptor = self.device.read_vec(
            self.descriptors + group * self.descriptor_size,
            self.descriptor_size as usize,
        )?;
        let high = if self.wide {
            u64::from(le32(&descriptor, 0x28)?)
        } else {
            0
        };
        let table = u64::from(le32(&descriptor, 8)?) | high << 32;
        if table == 0 {
            return Err(invalid("ext4 group has no inode table"));
        }
        add(
            mul(table, self.block, "locating ext4 inode table")?,
            index * self.inode_size,
            "locating ext4 inode",
        )
    }

    fn extent_node(&self, node: &[u8], depth: Option<u16>, runs: &mut Vec<Run>) -> Result<()> {
        if node.len() < 12 || le16(node, 0)? != 0xf30a {
            return Err(invalid("invalid ext4 extent node"));
        }
        let entries = usize::from(le16(node, 2)?);
        let level = le16(node, 6)?;
        if level > 5 || depth.is_some_and(|d| d != level) || entries > (node.len() - 12) / 12 {
            return Err(invalid("invalid ext4 extent node header"));
        }
        if runs.len() + entries > MAX_RUNS {
            return Err(invalid("ext4 file has too many extents"));
        }
        for i in 0..entries {
            let e = &node[12 + i * 12..24 + i * 12];
            if level == 0 {
                let raw = u64::from(le16(e, 4)?);
                // Lengths above 32768 mark unwritten extents, which read as zeroes.
                let (count, written) = if raw > 32768 {
                    (raw - 32768, false)
                } else {
                    (raw, true)
                };
                let start = u64::from(le32(e, 8)?) | u64::from(le16(e, 6)?) << 32;
                runs.push(Run {
                    logical: u64::from(le32(e, 0)?),
                    physical: written.then_some(start),
                    count,
                });
            } else {
                let child = u64::from(le32(e, 4)?) | u64::from(le16(e, 8)?) << 32;
                let bytes = self
                    .device
                    .read_vec(child * self.block, self.block as usize)?;
                self.extent_node(&bytes, Some(level - 1), runs)?;
            }
        }
        Ok(())
    }

    fn indirect(
        &self,
        block: u64,
        level: u32,
        logical: &mut u64,
        limit: u64,
        runs: &mut Vec<Run>,
    ) -> Result<()> {
        let per = self.block / 4;
        let span = per.pow(level);
        if block == 0 {
            *logical += span;
            return Ok(());
        }
        if level == 0 {
            runs.push(Run {
                logical: *logical,
                physical: Some(block),
                count: 1,
            });
            *logical += 1;
            return Ok(());
        }
        let table = self
            .device
            .read_vec(block * self.block, self.block as usize)?;
        for i in 0..per as usize {
            if *logical >= limit {
                break;
            }
            if runs.len() > MAX_RUNS {
                return Err(invalid("ext4 file has too many blocks"));
            }
            self.indirect(
                u64::from(le32(&table, i * 4)?),
                level - 1,
                logical,
                limit,
                runs,
            )?;
        }
        Ok(())
    }

    fn runs(&self, inode: &Inode) -> Result<Vec<Run>> {
        if inode.flags & INLINE_DATA_FL != 0 {
            return Err(invalid("ext4 inline data is unsupported"));
        }
        let limit = inode.size.div_ceil(self.block);
        let mut runs = Vec::new();
        if inode.flags & EXTENTS_FL != 0 {
            self.extent_node(&inode.block, None, &mut runs)?;
            runs.sort_by_key(|run| run.logical);
        } else {
            let mut logical = 0u64;
            for (i, level) in (0..15).zip([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3]) {
                if logical >= limit {
                    break;
                }
                let block = u64::from(le32(&inode.block, i * 4)?);
                self.indirect(block, level, &mut logical, limit, &mut runs)?;
            }
        }
        let mut end = 0u64;
        for run in &runs {
            if run.logical < end {
                return Err(invalid("overlapping ext4 block runs"));
            }
            end = run.logical + run.count;
        }
        Ok(runs)
    }

    pub fn copy(&self, inode: &Inode, out: &mut dyn Write) -> Result<u64> {
        let mut written = 0u64;
        let zeros = vec![0u8; self.block as usize];
        let fill = |out: &mut dyn Write, mut bytes: u64| -> Result<()> {
            while bytes > 0 {
                let take = bytes.min(zeros.len() as u64);
                out.write_all(&zeros[..take as usize])?;
                bytes -= take;
            }
            Ok(())
        };
        for run in self.runs(inode)? {
            let start = run.logical * self.block;
            if start >= inode.size {
                break;
            }
            fill(out, start - written)?;
            written = start;
            let length = (run.count * self.block).min(inode.size - start);
            match run.physical {
                Some(physical) => {
                    let mut done = 0u64;
                    while done < length {
                        let take = (length - done).min(1 << 20);
                        let at = physical * self.block + done;
                        out.write_all(&self.device.read_vec(at, take as usize)?)?;
                        done += take;
                    }
                }
                None => fill(out, length)?,
            }
            written += length;
        }
        fill(out, inode.size - written)?;
        Ok(inode.size)
    }

    fn read(&self, inode: &Inode, limit: u64) -> Result<Vec<u8>> {
        if inode.size > limit {
            return Err(invalid("ext4 file exceeds the in-memory limit"));
        }
        let mut out = Vec::with_capacity(inode.size as usize);
        self.copy(inode, &mut out)?;
        Ok(out)
    }

    pub fn entries(&self, directory: &Inode) -> Result<Vec<(Vec<u8>, u64)>> {
        if u32::from(directory.mode) & S_IFMT != S_IFDIR {
            return Err(invalid("ext4 inode is not a directory"));
        }
        let bytes = self.read(directory, MAX_DIRECTORY_BYTES)?;
        let mut out = Vec::new();
        for block in bytes.chunks(self.block as usize) {
            let mut at = 0usize;
            while at + 8 <= block.len() {
                let number = le32(block, at)?;
                let record = usize::from(le16(block, at + 4)?);
                let name_len = usize::from(block[at + 6]);
                if record < 8
                    || record % 4 != 0
                    || at + record > block.len()
                    || name_len + 8 > record
                {
                    return Err(invalid("invalid ext4 directory entry"));
                }
                let name = &block[at + 8..at + 8 + name_len];
                if number != 0 && name != b"." && name != b".." {
                    out.push((name.to_vec(), u64::from(number)));
                }
                at += record;
            }
        }
        Ok(out)
    }

    fn link(&self, inode: &Inode) -> Result<Vec<u8>> {
        let xattr_blocks = if inode.file_acl != 0 {
            self.block / 512
        } else {
            0
        };
        let fast = inode.size < 60 && u64::from(inode.blocks) == xattr_blocks;
        if fast {
            Ok(inode.block[..inode.size as usize].to_vec())
        } else {
            self.read(inode, 4096)
        }
    }
}

impl Tree for Ext4<'_> {
    fn root(&self) -> u64 {
        ROOT
    }
    fn node(&self, id: u64) -> Result<Node> {
        let inode = self.inode(id)?;
        Ok(Node {
            mode: u32::from(inode.mode),
            uid: inode.uid,
            gid: inode.gid,
            size: inode.size,
            nlink: u32::from(inode.nlink),
        })
    }
    fn children(&self, id: u64) -> Result<Vec<(Vec<u8>, u64)>> {
        self.entries(&self.inode(id)?)
    }
    fn read_link(&self, id: u64) -> Result<Vec<u8>> {
        let inode = self.inode(id)?;
        if u32::from(inode.mode) & S_IFMT != S_IFLNK {
            return Err(invalid("ext4 inode is not a symlink"));
        }
        self.link(&inode)
    }
    fn copy_file(&self, id: u64, out: &mut dyn Write) -> Result<u64> {
        let inode = self.inode(id)?;
        if u32::from(inode.mode) & S_IFMT != S_IFREG {
            return Err(invalid("ext4 inode is not a regular file"));
        }
        self.copy(&inode, out)
    }
    fn label(&self, id: u64) -> Result<Option<Vec<u8>>> {
        Ext4::label(self, id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tree::{Materializer, lookup, read_file, tests::temp_dir};

    const BLOCK: usize = 1024;
    const INODE: usize = 256;
    const SYSTEM_FILE: &[u8] = b"u:object_r:system_file:s0\0";
    const TOYBOX_EXEC: &[u8] = b"u:object_r:toybox_exec:s0\0";

    /// An `ext4_xattr_entry` with its padded name.
    fn xattr_entry(index: u8, name: &str, offset: u16, size: u32, inum: u32) -> Vec<u8> {
        let mut out = vec![name.len() as u8, index];
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&inum.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.resize((16 + name.len()).div_ceil(4) * 4, 0);
        out
    }

    /// In-inode xattrs for inode `number`: `user.a`, then `security.selinux`.
    fn inline_label(image: &mut [u8], number: usize, label: &[u8]) {
        let at = 5 * BLOCK + (number - 1) * INODE;
        put(image, at + 0x80, &32u16.to_le_bytes()); // i_extra_isize
        let area = at + 128 + 32;
        put(image, area, &XATTR_MAGIC.to_le_bytes());
        let entries = area + 4;
        let mut table = xattr_entry(1, "a", 56, 1, 0);
        table.extend(xattr_entry(6, "selinux", 60, label.len() as u32, 0));
        put(image, entries, &table);
        put(image, entries + 56, b"z");
        put(image, entries + 60, label);
    }

    /// An external xattr block for inode `number` at `block`.
    fn block_label(image: &mut [u8], number: usize, block: usize, label: &[u8]) {
        let at = 5 * BLOCK + (number - 1) * INODE;
        put(image, at + 0x68, &(block as u32).to_le_bytes());
        let b = block * BLOCK;
        put(image, b, &XATTR_MAGIC.to_le_bytes());
        put(image, b + 4, &1u32.to_le_bytes()); // h_refcount
        put(image, b + 8, &1u32.to_le_bytes()); // h_blocks
        let mut table = xattr_entry(6, "selinuxx", 400, 1, 0);
        table.extend(xattr_entry(6, "selinux", 512, label.len() as u32, 0));
        put(image, b + 32, &table);
        put(image, b + 512, label);
    }

    fn put(image: &mut [u8], at: usize, data: &[u8]) {
        image[at..at + data.len()].copy_from_slice(data);
    }

    fn inode(
        image: &mut [u8],
        number: usize,
        mode: u16,
        size: u32,
        flags: u32,
        blocks: u32,
        block: &[u8],
    ) {
        // Inode table at block 5, 256-byte inodes.
        let at = 5 * BLOCK + (number - 1) * INODE;
        put(image, at, &mode.to_le_bytes());
        put(image, at + 4, &size.to_le_bytes());
        put(image, at + 0x18, &3003u16.to_le_bytes());
        put(image, at + 0x1a, &1u16.to_le_bytes());
        put(image, at + 0x1c, &blocks.to_le_bytes());
        put(image, at + 0x20, &flags.to_le_bytes());
        put(image, at + 0x28, block);
    }

    fn dirents(entries: &[(&str, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, (name, number)) in entries.iter().enumerate() {
            let record = if i + 1 == entries.len() {
                BLOCK - out.len()
            } else {
                (8 + name.len()).div_ceil(4) * 4
            };
            out.extend_from_slice(&number.to_le_bytes());
            out.extend_from_slice(&(record as u16).to_le_bytes());
            out.push(name.len() as u8);
            out.push(0);
            out.extend_from_slice(name.as_bytes());
            out.resize(out.len() + record - 8 - name.len(), 0);
        }
        out
    }

    fn extent_root(runs: &[(u32, u16, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0xf30au16.to_le_bytes());
        out.extend_from_slice(&(runs.len() as u16).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for (logical, count, start) in runs {
            out.extend_from_slice(&logical.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&start.to_le_bytes());
        }
        out
    }

    /// 1 KiB-block ext4: root (2) holds `data` (12, extents with a hole),
    /// `old` (13, indirect map), `sh` (14, fast symlink) and `etc` (15).
    pub(crate) fn build() -> Vec<u8> {
        let mut image = vec![0u8; 64 * BLOCK];
        let sb = 1024;
        put(&mut image, sb, &32u32.to_le_bytes()); // inodes
        put(&mut image, sb + 0x14, &1u32.to_le_bytes()); // first data block
        put(&mut image, sb + 0x28, &32u32.to_le_bytes()); // inodes per group
        put(&mut image, sb + 0x38, &MAGIC.to_le_bytes());
        put(&mut image, sb + 0x4c, &1u32.to_le_bytes());
        put(&mut image, sb + 0x58, &(INODE as u16).to_le_bytes());
        put(
            &mut image,
            sb + 0x60,
            &(INCOMPAT_FILETYPE | INCOMPAT_EXTENTS).to_le_bytes(),
        );
        put(&mut image, 2 * BLOCK + 8, &5u32.to_le_bytes()); // descriptor: inode table

        let root = dirents(&[
            (".", 2),
            ("..", 2),
            ("data", 12),
            ("old", 13),
            ("sh", 14),
            ("etc", 15),
        ]);
        put(&mut image, 20 * BLOCK, &root);
        inode(
            &mut image,
            2,
            0o040755,
            BLOCK as u32,
            EXTENTS_FL,
            2,
            &extent_root(&[(0, 1, 20)]),
        );
        let etc = dirents(&[(".", 15), ("..", 2)]);
        put(&mut image, 21 * BLOCK, &etc);
        inode(
            &mut image,
            15,
            0o040750,
            BLOCK as u32,
            EXTENTS_FL,
            2,
            &extent_root(&[(0, 1, 21)]),
        );

        // data: block 0 at 30, block 1 a hole, block 2 unwritten, block 3 at 31.
        put(&mut image, 30 * BLOCK, &[b'a'; BLOCK]);
        put(&mut image, 31 * BLOCK, &[b'd'; BLOCK]);
        let data = extent_root(&[(0, 1, 30), (2, 32769, 40), (3, 1, 31)]);
        inode(
            &mut image,
            12,
            0o100644,
            (3 * BLOCK + 100) as u32,
            EXTENTS_FL,
            6,
            &data,
        );

        // old: 13 direct blocks; the 13th goes through the indirect block 33.
        let mut map = Vec::new();
        for i in 0..12u32 {
            map.extend_from_slice(&(if i == 5 { 0u32 } else { 34 }).to_le_bytes());
        }
        map.extend_from_slice(&33u32.to_le_bytes());
        put(&mut image, 33 * BLOCK, &35u32.to_le_bytes());
        put(&mut image, 34 * BLOCK, &[b'o'; BLOCK]);
        put(&mut image, 35 * BLOCK, &[b'x'; BLOCK]);
        inode(
            &mut image,
            13,
            0o100755,
            (12 * BLOCK + 10) as u32,
            0,
            26,
            &map,
        );

        inode(&mut image, 14, 0o120777, 6, 0, 0, b"toybox");
        image
    }

    #[test]
    fn reads_extents_block_maps_and_links() {
        let image = build();
        let fs = Ext4::open(&image).unwrap();
        let data = read_file(&fs, lookup(&fs, "/data").unwrap().unwrap(), 1 << 20).unwrap();
        let mut expected = vec![b'a'; BLOCK];
        expected.extend(vec![0u8; 2 * BLOCK]);
        expected.extend(vec![b'd'; 100]);
        assert_eq!(data, expected);
        let old = read_file(&fs, 13, 1 << 20).unwrap();
        assert_eq!(old.len(), 12 * BLOCK + 10);
        assert_eq!(&old[..BLOCK], &[b'o'; BLOCK][..]);
        assert!(old[5 * BLOCK..6 * BLOCK].iter().all(|b| *b == 0));
        assert_eq!(&old[12 * BLOCK..], &[b'x'; 10][..]);
        assert_eq!(fs.read_link(14).unwrap(), b"toybox");

        let out = temp_dir("ext4");
        let mut materializer = Materializer::default();
        materializer.extract(&fs, &out).unwrap();
        let report = materializer.finish().unwrap();
        assert_eq!(
            (report.directories, report.files, report.symlinks),
            (2, 2, 1)
        );
        std::fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn reads_in_inode_and_block_labels() {
        let mut image = build();
        inline_label(&mut image, 12, SYSTEM_FILE);
        inline_label(&mut image, 14, TOYBOX_EXEC);
        block_label(&mut image, 15, 50, SYSTEM_FILE);
        // In-inode xattrs without the label fall back to the block.
        inline_label(&mut image, 2, SYSTEM_FILE);
        put(&mut image, 5 * BLOCK + INODE + 128 + 32 + 4 + 41, b"x"); // "selinux" -> "selinxx"
        block_label(&mut image, 2, 51, TOYBOX_EXEC);
        let fs = Ext4::open(&image).unwrap();
        assert_eq!(fs.label(12).unwrap().as_deref(), Some(SYSTEM_FILE));
        assert_eq!(fs.label(14).unwrap().as_deref(), Some(TOYBOX_EXEC));
        assert_eq!(fs.label(15).unwrap().as_deref(), Some(SYSTEM_FILE));
        assert_eq!(fs.label(2).unwrap().as_deref(), Some(TOYBOX_EXEC));
        assert_eq!(fs.label(13).unwrap(), None);

        let out = temp_dir("ext4-labels");
        let mut materializer = Materializer::default();
        materializer.extract(&fs, &out).unwrap();
        materializer.finish().unwrap();
        let host = |p: &str| crate::tree::tests::host_label(&out.join(p));
        assert_eq!(host("data").as_deref(), Some(SYSTEM_FILE));
        assert_eq!(host("sh").as_deref(), Some(TOYBOX_EXEC));
        assert_eq!(host("etc").as_deref(), Some(SYSTEM_FILE));
        assert_eq!(host("").as_deref(), Some(TOYBOX_EXEC));
        assert_eq!(host("old"), None);
        std::fs::remove_dir_all(&out).unwrap();

        // Out-of-area values, EA-inode values and a bad block are errors.
        let mut bad = image.clone();
        put(&mut bad, 51 * BLOCK + 32 + 24 + 8, &2000u32.to_le_bytes());
        assert!(Ext4::open(&bad).unwrap().label(2).is_err());
        let mut bad = image.clone();
        put(&mut bad, 51 * BLOCK + 32 + 24 + 4, &9u32.to_le_bytes());
        assert!(Ext4::open(&bad).unwrap().label(2).is_err());
        let mut bad = image.clone();
        put(&mut bad, 50 * BLOCK, &0u32.to_le_bytes());
        assert!(Ext4::open(&bad).unwrap().label(15).is_err());
        let mut bad = image;
        put(
            &mut bad,
            5 * BLOCK + 11 * INODE + 0x80,
            &200u16.to_le_bytes(),
        );
        assert!(Ext4::open(&bad).unwrap().label(12).is_err());
    }

    #[test]
    fn rejects_unsupported_layouts() {
        let mut image = build();
        put(&mut image, 1024 + 0x60, &0x8000u32.to_le_bytes()); // inline data
        assert!(Ext4::open(&image).is_err());
        let mut image = build();
        image[1024 + 0x38] = 0;
        assert!(Ext4::open(&image).is_err());
        let mut image = build();
        put(
            &mut image,
            5 * BLOCK + 11 * INODE + 0x28,
            &0u16.to_le_bytes(),
        ); // extent magic
        let fs = Ext4::open(&image).unwrap();
        assert!(read_file(&fs, 12, 1 << 20).is_err());
        assert!(fs.inode(0).is_err() && fs.inode(33).is_err());
    }
}
