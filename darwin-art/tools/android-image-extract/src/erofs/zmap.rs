//! Compressed EROFS inodes: logical-cluster indexes (full and compact) and
//! whole-file LZ4 decoding. Mirrors Linux v6.12 fs/erofs/zmap.c.
use super::{Erofs, INCOMPAT_ZERO_PADDING, Inode, inode};
use crate::{Result, align, invalid, le16, le32, lz4};
use std::io::Write;

const ADVISE_COMPACTED_2B: u16 = 0x1;
const ADVISE_BIG_PCLUSTER_1: u16 = 0x2;
const ADVISE_BIG_PCLUSTER_2: u16 = 0x4;
const SUPPORTED_ADVISE: u16 = ADVISE_COMPACTED_2B | ADVISE_BIG_PCLUSTER_1 | ADVISE_BIG_PCLUSTER_2;
const FRAGMENT_INODE_BIT: u8 = 0x80;
const D0_CBLKCNT: u32 = 1 << 11;
const ALG_LZ4: u8 = 0;

pub(super) const PLAIN: u8 = 0;
pub(super) const HEAD1: u8 = 1;
pub(super) const NONHEAD: u8 = 2;
pub(super) const HEAD2: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Lcluster {
    pub kind: u8,
    pub clusterofs: u32,
    /// Physical block of a head's pcluster.
    pub pblk: u32,
    /// Compressed block count carried by the first non-head lcluster.
    pub cblkcnt: Option<u32>,
}

/// Decoded map header plus the whole index area of one inode.
pub(super) struct Map {
    compact: bool,
    advise: u16,
    algorithms: [u8; 2],
    lcluster_bits: u32,
    block_bits: u32,
    size: u64,
    ebase: u64,
    index: Vec<u8>,
}

impl Map {
    pub(super) fn load(fs: &Erofs<'_>, inode: &Inode) -> Result<Self> {
        let header_at = align(inode.offset + inode.inode_size + inode.xattr_size, 8)?;
        let header = fs.device.read_vec(header_at, 8)?;
        let advise = le16(&header, 4)?;
        let clusterbits = header[7];
        if clusterbits & FRAGMENT_INODE_BIT != 0 || advise & !SUPPORTED_ADVISE != 0 {
            return Err(invalid(format!(
                "unsupported EROFS compression header advise={advise:#x} clusterbits={clusterbits:#x} (nid {})",
                inode.nid
            )));
        }
        let compact = inode.layout == inode::COMPRESSED_COMPACT;
        let big1 = advise & ADVISE_BIG_PCLUSTER_1 != 0;
        let big2 = advise & ADVISE_BIG_PCLUSTER_2 != 0;
        if compact && big1 != big2 {
            return Err(invalid(
                "inconsistent big-pcluster flags in compact indexes",
            ));
        }
        let lcluster_bits = fs.block_bits + u32::from(clusterbits & 7);
        let lclusters = inode.size.div_ceil(1 << lcluster_bits);
        let (ebase, bytes) = if compact {
            // Upper bound: every index in 4-byte form, plus pack alignment.
            (header_at + 8, lclusters * 4 + 64)
        } else {
            (header_at + 16, lclusters * 8)
        };
        let available = fs.device.size().saturating_sub(ebase);
        let mut index = fs.device.read_vec(ebase, bytes.min(available) as usize)?;
        index.resize(bytes as usize, 0);
        Ok(Self {
            compact,
            advise,
            algorithms: [header[6] & 15, header[6] >> 4],
            lcluster_bits,
            block_bits: fs.block_bits,
            size: inode.size,
            ebase,
            index,
        })
    }

    fn lclusters(&self) -> u64 {
        self.size.div_ceil(1 << self.lcluster_bits)
    }

    pub(super) fn lcluster(&self, lcn: u64) -> Result<Lcluster> {
        if self.compact {
            self.compact_lcluster(lcn)
        } else {
            self.full_lcluster(lcn)
        }
    }

    fn full_lcluster(&self, lcn: u64) -> Result<Lcluster> {
        let at = (lcn * 8) as usize;
        let advise = u32::from(le16(&self.index, at)?);
        let kind = (advise & 3) as u8;
        if kind == NONHEAD {
            let delta0 = u32::from(le16(&self.index, at + 4)?);
            let cblkcnt = (delta0 & D0_CBLKCNT != 0).then_some(delta0 & !D0_CBLKCNT);
            if cblkcnt.is_some()
                && self.advise & (ADVISE_BIG_PCLUSTER_1 | ADVISE_BIG_PCLUSTER_2) == 0
            {
                return Err(invalid("compressed block count without big pclusters"));
            }
            return Ok(Lcluster {
                kind,
                clusterofs: 1 << self.lcluster_bits,
                pblk: 0,
                cblkcnt,
            });
        }
        let clusterofs = u32::from(le16(&self.index, at + 2)?);
        if clusterofs >= 1 << self.lcluster_bits {
            return Err(invalid("EROFS cluster offset exceeds its lcluster"));
        }
        Ok(Lcluster {
            kind,
            clusterofs,
            pblk: le32(&self.index, at + 4)?,
            cblkcnt: None,
        })
    }

    fn compact_lcluster(&self, lcn: u64) -> Result<Lcluster> {
        let total = self.size.div_ceil(1 << self.block_bits);
        if lcn >= total || self.lcluster_bits > 14 {
            return Err(invalid("EROFS compact index out of range"));
        }
        let initial = ((32 - self.ebase % 32) / 4) & 7;
        let compacted_2b = if self.advise & ADVISE_COMPACTED_2B != 0 && initial < total {
            (total - initial) / 16 * 16
        } else {
            0
        };
        let mut pos = self.ebase;
        let mut shift = 2u32;
        let mut rest = lcn;
        if rest >= initial {
            pos += initial * 4;
            rest -= initial;
            if rest < compacted_2b {
                shift = 1;
            } else {
                pos += compacted_2b * 2;
                rest -= compacted_2b;
            }
        }
        pos += rest << shift;
        let vcnt: u64 = match shift {
            2 if self.lcluster_bits <= 14 => 2,
            1 if self.lcluster_bits <= 12 => 16,
            _ => return Err(invalid("unsupported EROFS compact index geometry")),
        };
        let pack_bytes = vcnt << shift;
        let pack_start = pos / pack_bytes * pack_bytes;
        let rel = (pack_start - self.ebase) as usize;
        let pack = self
            .index
            .get(rel..rel + pack_bytes as usize)
            .ok_or_else(|| invalid("EROFS compact index pack out of range"))?;
        let lobits = self.lcluster_bits.max(12);
        let encodebits = ((pack_bytes - 4) * 8 / vcnt) as usize;
        let decode = |i: usize| -> Result<(u32, u8)> {
            let bit = encodebits * i;
            let v = le32(pack, bit / 8).or_else(|_| {
                let mut word = [0u8; 4];
                let tail = &pack[bit / 8..];
                word[..tail.len()].copy_from_slice(tail);
                Ok::<u32, crate::Error>(u32::from_le_bytes(word))
            })? >> (bit % 8);
            Ok((v & ((1 << lobits) - 1), ((v >> lobits) & 3) as u8))
        };
        let mut i = ((pos - pack_start) >> shift) as isize;
        let (lo, kind) = decode(i as usize)?;
        if kind == NONHEAD {
            let cblkcnt = if lo & D0_CBLKCNT != 0 {
                if self.advise & ADVISE_BIG_PCLUSTER_1 == 0 {
                    return Err(invalid("compressed block count without big pclusters"));
                }
                Some(lo & !D0_CBLKCNT)
            } else {
                None
            };
            return Ok(Lcluster {
                kind,
                clusterofs: 1 << self.lcluster_bits,
                pblk: 0,
                cblkcnt,
            });
        }
        // Heads: the pack's base block plus the blocks of preceding pclusters.
        let mut blocks: u32 = 0;
        if self.advise & ADVISE_BIG_PCLUSTER_1 == 0 {
            blocks = 1;
            while i > 0 {
                i -= 1;
                let (lo, kind) = decode(i as usize)?;
                if kind == NONHEAD {
                    i -= lo as isize;
                }
                if i >= 0 {
                    blocks += 1;
                }
            }
        } else {
            while i > 0 {
                i -= 1;
                let (lo, kind) = decode(i as usize)?;
                if kind == NONHEAD {
                    if lo & D0_CBLKCNT != 0 {
                        i -= 1;
                        blocks += lo & !D0_CBLKCNT;
                        continue;
                    }
                    if lo <= 1 {
                        return Err(invalid("invalid EROFS compact lookback distance"));
                    }
                    i -= lo as isize - 2;
                    continue;
                }
                blocks += 1;
            }
        }
        let base = le32(pack, pack.len() - 4)?;
        Ok(Lcluster {
            kind,
            clusterofs: lo,
            pblk: base
                .checked_add(blocks)
                .ok_or_else(|| invalid("EROFS physical block overflow"))?,
            cblkcnt: None,
        })
    }

    /// Physical blocks of the pcluster whose head is lcluster `lcn`.
    fn pcluster_blocks(&self, lcn: u64, kind: u8) -> Result<u32> {
        let big = match kind {
            HEAD1 => self.advise & ADVISE_BIG_PCLUSTER_1 != 0,
            _ => self.advise & ADVISE_BIG_PCLUSTER_2 != 0,
        };
        if !big || lcn + 1 >= self.lclusters() {
            return Ok(1);
        }
        let next = self.lcluster(lcn + 1)?;
        match (next.kind, next.cblkcnt) {
            (NONHEAD, Some(count)) if count > 0 => Ok(count),
            (NONHEAD, _) => Err(invalid("missing EROFS compressed block count")),
            _ => Ok(1),
        }
    }
}

/// One decoded extent: logical `[start, end)` from `blocks` blocks at `pblk`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Extent {
    pub start: u64,
    pub end: u64,
    pub kind: u8,
    pub pblk: u32,
    pub blocks: u32,
}

pub(super) fn extents(map: &Map) -> Result<Vec<Extent>> {
    let lcsize = 1u64 << map.lcluster_bits;
    let mut heads: Vec<(u64, u64, Lcluster)> = Vec::new();
    for lcn in 0..map.lclusters() {
        let lc = map.lcluster(lcn)?;
        if lc.kind == NONHEAD {
            if lcn == 0 {
                return Err(invalid(
                    "EROFS compressed file starts with a non-head lcluster",
                ));
            }
            continue;
        }
        let start = lcn * lcsize + u64::from(lc.clusterofs);
        // A trailing index may only terminate the previous extent at EOF.
        if start >= map.size {
            continue;
        }
        heads.push((start, lcn, lc));
    }
    if heads.first().is_none_or(|(start, _, _)| *start != 0) {
        return Err(invalid("EROFS compressed file has no initial head"));
    }
    let mut out = Vec::with_capacity(heads.len());
    for (i, (start, lcn, lc)) in heads.iter().enumerate() {
        let end = heads.get(i + 1).map_or(map.size, |next| next.0);
        if end <= *start {
            return Err(invalid("EROFS compressed extents are out of order"));
        }
        out.push(Extent {
            start: *start,
            end,
            kind: lc.kind,
            pblk: lc.pblk,
            blocks: map.pcluster_blocks(*lcn, lc.kind)?,
        });
    }
    Ok(out)
}

pub(super) fn copy(fs: &Erofs<'_>, inode: &Inode, out: &mut dyn Write) -> Result<u64> {
    if inode.size == 0 {
        return Ok(0);
    }
    let map = Map::load(fs, inode)?;
    let mut buffer = Vec::new();
    let mut written = 0u64;
    for extent in extents(&map)? {
        let length = (extent.end - extent.start) as usize;
        let input = fs.device.read_vec(
            u64::from(extent.pblk) * fs.block,
            (u64::from(extent.blocks) * fs.block) as usize,
        )?;
        buffer.clear();
        match extent.kind {
            PLAIN => {
                let data = input
                    .get(..length)
                    .ok_or_else(|| invalid("EROFS plain extent exceeds its pcluster"))?;
                buffer.extend_from_slice(data);
            }
            HEAD1 | HEAD2 => {
                let algorithm = map.algorithms[usize::from(extent.kind == HEAD2)];
                if algorithm != ALG_LZ4 {
                    return Err(invalid(format!(
                        "unsupported EROFS compression algorithm {algorithm}"
                    )));
                }
                let margin = if fs.incompat & INCOMPAT_ZERO_PADDING != 0 {
                    let first = &input[..fs.block as usize];
                    first.iter().position(|b| *b != 0).unwrap_or(first.len())
                } else {
                    0
                };
                if margin >= input.len() {
                    return Err(invalid("EROFS pcluster holds no compressed data"));
                }
                lz4::decode_block_into(&input[margin..], length, &mut buffer).map_err(|error| {
                    invalid(format!(
                        "nid {} extent {}..{}: {error}",
                        inode.nid, extent.start, extent.end
                    ))
                })?;
            }
            _ => unreachable!(),
        }
        out.write_all(&buffer)?;
        written += length as u64;
    }
    if written != inode.size {
        return Err(invalid("EROFS decoded size mismatch"));
    }
    Ok(written)
}
