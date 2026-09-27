//! A hand-assembled EROFS image covering every supported inode layout.
use super::zmap::{self, HEAD1, Map, NONHEAD, PLAIN};
use super::*;
use crate::tree::{Materializer, lookup, read_file, tests::temp_dir};

const BLOCK: usize = 4096;
const META: usize = 1;

/// An LZ4 sequence: `literals`, then an optional (offset, length) match.
pub(crate) fn lz4_sequence(literals: &[u8], matched: Option<(u16, usize)>) -> Vec<u8> {
    let mut out = Vec::new();
    let lit_nibble = literals.len().min(15) as u8;
    let match_len = matched.map_or(0, |(_, length)| length - 4);
    out.push(lit_nibble << 4 | match_len.min(15) as u8);
    let extend = |out: &mut Vec<u8>, value: usize| {
        if value >= 15 {
            let mut rest = value - 15;
            while rest >= 255 {
                out.push(255);
                rest -= 255;
            }
            out.push(rest as u8);
        }
    };
    extend(&mut out, literals.len());
    out.extend_from_slice(literals);
    if let Some((offset, _)) = matched {
        out.extend_from_slice(&offset.to_le_bytes());
        extend(&mut out, match_len);
    }
    out
}

fn pattern(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|i| seed.wrapping_add((i % 16) as u8))
        .collect()
}

/// `pattern(length)` as `literals` literals, one long match and 6 literals.
fn compressed_pattern(length: usize, seed: u8, literals: usize) -> Vec<u8> {
    let data = pattern(length, seed);
    let mut out = lz4_sequence(&data[..literals], Some((16, length - literals - 6)));
    out.extend(lz4_sequence(&data[length - 6..], None));
    out
}

struct Image {
    bytes: Vec<u8>,
}

impl Image {
    fn new(blocks: usize) -> Self {
        let mut bytes = vec![0u8; blocks * BLOCK];
        let sb = &mut bytes[1024..1152];
        sb[..4].copy_from_slice(&MAGIC.to_le_bytes());
        sb[12] = 12;
        sb[40..44].copy_from_slice(&(META as u32).to_le_bytes());
        sb[80..84].copy_from_slice(&(INCOMPAT_ZERO_PADDING | INCOMPAT_COMPR_CFGS).to_le_bytes());
        sb[84..86].copy_from_slice(&1u16.to_le_bytes());
        Self { bytes }
    }

    fn at(nid: u64) -> usize {
        META * BLOCK + nid as usize * 32
    }

    /// A compact (32-byte) inode.
    fn inode(&mut self, nid: u64, layout: u16, mode: u32, size: u32, i_u: u32, nlink: u16) {
        let at = Self::at(nid);
        let i = &mut self.bytes[at..at + 32];
        i[0..2].copy_from_slice(&(layout << 1).to_le_bytes());
        i[4..6].copy_from_slice(&(mode as u16).to_le_bytes());
        i[6..8].copy_from_slice(&nlink.to_le_bytes());
        i[8..12].copy_from_slice(&size.to_le_bytes());
        i[16..20].copy_from_slice(&i_u.to_le_bytes());
        i[26..28].copy_from_slice(&1000u16.to_le_bytes());
    }

    fn put(&mut self, at: usize, data: &[u8]) {
        self.bytes[at..at + data.len()].copy_from_slice(data);
    }

    fn directory(&mut self, nid: u64, parent: u64, entries: &[(&str, u64)]) {
        let mut all = vec![(".", nid), ("..", parent)];
        all.extend_from_slice(entries);
        let mut table = Vec::new();
        let mut names = Vec::new();
        let base = all.len() * 12;
        for (name, child) in &all {
            table.extend_from_slice(&child.to_le_bytes());
            table.extend_from_slice(&((base + names.len()) as u16).to_le_bytes());
            table.extend_from_slice(&[0, 0]);
            names.extend_from_slice(name.as_bytes());
        }
        table.extend(names);
        self.inode(
            nid,
            inode::FLAT_INLINE,
            S_IFDIR | 0o755,
            table.len() as u32,
            0,
            2,
        );
        self.put(Self::at(nid) + 32, &table);
    }
}

pub(crate) fn build() -> Vec<u8> {
    let mut image = Image::new(16);
    image.directory(
        0,
        0,
        &[
            ("bin", 16),
            ("compact", 48),
            ("full", 64),
            ("hello", 80),
            ("link", 96),
            ("null", 112),
            ("small", 128),
        ],
    );
    image.directory(16, 0, &[("toybox", 80)]);

    // Compact indexes (4B packs, big pclusters): [0, 8692) is LZ4 in blocks
    // 4-5 (count in lcn 1), [8692, 12188) is PLAIN in block 6.
    let (a, b) = (8692usize, 12188usize);
    image.inode(
        48,
        inode::COMPRESSED_COMPACT,
        S_IFREG | 0o644,
        b as u32,
        3,
        1,
    );
    let header = Image::at(48) + 32;
    image.put(header + 4, &0x6u16.to_le_bytes());
    let index = header + 8;
    image.put(index, &0x1000u16.to_le_bytes()); // lcn 0: HEAD1, ofs 0
    image.put(index + 2, &0x2802u16.to_le_bytes()); // lcn 1: NONHEAD, CBLKCNT 2
    image.put(index + 4, &4u32.to_le_bytes());
    image.put(index + 8, &(((PLAIN as u16) << 12) | 500).to_le_bytes()); // lcn 2
    image.put(index + 12, &6u32.to_le_bytes());
    // Over one block of stream, so the 2-block pcluster is genuine.
    let stream = compressed_pattern(a, 1, 4200);
    assert!(stream.len() > BLOCK);
    image.put(6 * BLOCK - stream.len(), &stream); // right-aligned (zero padding)
    image.put(6 * BLOCK, &pattern(b - a, 7));

    // Full indexes: one 1-block LZ4 pcluster in block 7 for all 5000 bytes.
    image.inode(64, inode::COMPRESSED_FULL, S_IFREG | 0o600, 5000, 1, 1);
    let header = Image::at(64) + 32;
    let index = header + 16;
    image.put(index, &u16::from(HEAD1).to_le_bytes());
    image.put(index + 4, &7u32.to_le_bytes());
    image.put(index + 8, &u16::from(NONHEAD).to_le_bytes());
    image.put(index + 12, &1u16.to_le_bytes());
    let stream = compressed_pattern(5000, 3, 16);
    image.put(8 * BLOCK - stream.len(), &stream);

    // Flat plain file shared by two names, blocks 8-9.
    image.inode(80, inode::FLAT_PLAIN, S_IFREG | 0o755, 5000, 8, 2);
    image.put(8 * BLOCK, &pattern(5000, 9));

    image.inode(96, inode::FLAT_INLINE, S_IFLNK | 0o777, 14, 0, 1);
    image.put(Image::at(96) + 32, b"/system/bin/sh");
    image.inode(112, inode::FLAT_PLAIN, 0o020666, 0, 0x0103, 1);

    // Inline file: one full block (block 10) plus a 10-byte inline tail.
    image.inode(128, inode::FLAT_INLINE, S_IFREG | 0o644, 4106, 10, 1);
    image.put(10 * BLOCK, &pattern(4096, 5));
    image.put(Image::at(128) + 32, &pattern(10, 5));
    image.bytes
}

#[test]
fn decodes_every_layout() {
    let bytes = build();
    let fs = Erofs::open(&bytes).unwrap();
    let file = |path: &str| read_file(&fs, lookup(&fs, path).unwrap().unwrap(), 1 << 20).unwrap();
    let mut expected = pattern(8692, 1);
    expected.extend(pattern(12188 - 8692, 7));
    assert_eq!(file("/compact"), expected);
    assert_eq!(file("/full"), pattern(5000, 3));
    assert_eq!(file("/bin/toybox"), pattern(5000, 9));
    let mut small = pattern(4096, 5);
    small.extend(pattern(10, 5));
    assert_eq!(file("/small"), small);
    let link = lookup(&fs, "/link").unwrap().unwrap();
    assert_eq!(fs.read_link(link).unwrap(), b"/system/bin/sh");
    assert!(lookup(&fs, "/missing").unwrap().is_none());

    let compact = fs.inode(48).unwrap();
    let map = Map::load(&fs, &compact).unwrap();
    let extents = zmap::extents(&map).unwrap();
    assert_eq!(
        extents
            .iter()
            .map(|e| (e.start, e.end, e.pblk, e.blocks))
            .collect::<Vec<_>>(),
        vec![(0, 8692, 4, 2), (8692, 12188, 6, 1)]
    );
}

#[test]
fn materializes_tree_with_hardlinks_and_specials() {
    let bytes = build();
    let fs = Erofs::open(&bytes).unwrap();
    let out = temp_dir("erofs");
    let mut materializer = Materializer::default();
    materializer.extract(&fs, &out).unwrap();
    let report = materializer.finish().unwrap();
    assert_eq!((report.files, report.hardlinks, report.symlinks), (4, 1, 1));
    assert_eq!(report.special.len(), 1);
    std::fs::remove_dir_all(&out).unwrap();
}

#[test]
fn rejects_unsupported_features_and_corruption() {
    let mut bytes = build();
    bytes[1024 + 80] |= 0x10; // ztailpacking
    assert!(Erofs::open(&bytes).is_err());
    let mut bytes = build();
    bytes[1024] ^= 1;
    assert!(Erofs::open(&bytes).is_err());
    // A compressed file whose first lcluster is a non-head.
    let mut bytes = build();
    let index = Image::at(48) + 40;
    bytes[index..index + 2].copy_from_slice(&0x2001u16.to_le_bytes());
    let fs = Erofs::open(&bytes).unwrap();
    assert!(read_file(&fs, 48, 1 << 20).is_err());
    // A pcluster that decodes to fewer bytes than the inode declares.
    let mut bytes = build();
    let at = Image::at(64) + 8;
    bytes[at..at + 4].copy_from_slice(&5001u32.to_le_bytes());
    let fs = Erofs::open(&bytes).unwrap();
    assert!(read_file(&fs, 64, 1 << 20).is_err());
}
