use std::env;
mod directory;
mod inode;
use inode::Inode;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

const SECTOR: u64 = 512;
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const LP_GEOMETRY_MAGIC: u32 = 0x616c_4467;
const LP_HEADER_MAGIC: u32 = 0x414c_5030;
const EROFS_MAGIC: u32 = 0xe0f5_e1e2;
const DEFAULT_APEX_NAME: &str = "com.android.i18n.apex";
const MAX_TABLE_BYTES: usize = 1024 * 1024;
const MAX_TARGET_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PCLUSTER_BYTES: usize = 1024 * 1024;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn le16(b: &[u8], o: usize) -> Result<u16> {
    let v = b.get(o..o + 2).ok_or_else(|| invalid("truncated u16"))?;
    Ok(u16::from_le_bytes([v[0], v[1]]))
}
fn le32(b: &[u8], o: usize) -> Result<u32> {
    let v = b.get(o..o + 4).ok_or_else(|| invalid("truncated u32"))?;
    Ok(u32::from_le_bytes(v.try_into().unwrap()))
}
fn le64(b: &[u8], o: usize) -> Result<u64> {
    let v = b.get(o..o + 8).ok_or_else(|| invalid("truncated u64"))?;
    Ok(u64::from_le_bytes(v.try_into().unwrap()))
}
fn add(a: u64, b: u64, what: &str) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| invalid(format!("overflow {what}")).into())
}
fn mul(a: u64, b: u64, what: &str) -> Result<u64> {
    a.checked_mul(b)
        .ok_or_else(|| invalid(format!("overflow {what}")).into())
}
fn align(value: u64, alignment: u64) -> Result<u64> {
    if !alignment.is_power_of_two() {
        return Err(invalid("invalid alignment").into());
    }
    add(value, alignment - 1, "aligning").map(|v| v & !(alignment - 1))
}

struct Disk {
    file: File,
    size: u64,
}
impl Disk {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self { file, size })
    }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let end = add(offset, length as u64, "checking disk read")?;
        if end > self.size {
            return Err(invalid("read outside disk image").into());
        }
        let mut out = vec![0; length];
        self.file.read_exact_at(&mut out, offset)?;
        Ok(out)
    }
}

#[derive(Clone, Copy, Debug)]
struct Region {
    offset: u64,
    size: u64,
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn gpt_super(disk: &Disk) -> Result<(Region, (u16, u16))> {
    let h = disk.read(SECTOR, 512)?;
    if h.get(..8) != Some(GPT_SIGNATURE) {
        return Err(invalid("missing primary GPT").into());
    }
    let revision = le32(&h, 8)?;
    let header_size = usize::try_from(le32(&h, 12)?)?;
    if !(92..=512).contains(&header_size) || le64(&h, 24)? != 1 {
        return Err(invalid("invalid GPT header size or LBA").into());
    }
    let expected = le32(&h, 16)?;
    let mut checked = h[..header_size].to_vec();
    checked[16..20].fill(0);
    if crc32(&checked) != expected {
        return Err(invalid("GPT header CRC mismatch").into());
    }
    let table_lba = le64(&h, 72)?;
    let count = usize::try_from(le32(&h, 80)?)?;
    let entry_size = usize::try_from(le32(&h, 84)?)?;
    if count == 0 || count > 4096 || !(128..=4096).contains(&entry_size) {
        return Err(invalid("invalid GPT entry geometry").into());
    }
    let table_len = count
        .checked_mul(entry_size)
        .ok_or_else(|| invalid("GPT table overflow"))?;
    if table_len > MAX_TABLE_BYTES {
        return Err(invalid("GPT table too large").into());
    }
    let table = disk.read(mul(table_lba, SECTOR, "locating GPT table")?, table_len)?;
    if crc32(&table) != le32(&h, 88)? {
        return Err(invalid("GPT table CRC mismatch").into());
    }
    let mut found = None;
    for entry in table.chunks_exact(entry_size) {
        if entry[..16].iter().all(|b| *b == 0) {
            continue;
        }
        let mut name = String::new();
        for pair in entry[56..128].chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            if unit == 0 {
                break;
            }
            name.push(char::from_u32(u32::from(unit)).ok_or_else(|| invalid("invalid GPT name"))?);
        }
        if name == "super" {
            if found.is_some() {
                return Err(invalid("duplicate GPT super partition").into());
            }
            let first = le64(entry, 32)?;
            let last = le64(entry, 40)?;
            if last < first {
                return Err(invalid("reversed GPT super partition").into());
            }
            found = Some(Region {
                offset: mul(first, SECTOR, "locating super")?,
                size: mul(last - first + 1, SECTOR, "sizing super")?,
            });
        }
    }
    let region = found.ok_or_else(|| invalid("GPT super partition not found"))?;
    if add(region.offset, region.size, "checking super")? > disk.size {
        return Err(invalid("super outside image").into());
    }
    Ok((region, ((revision >> 16) as u16, revision as u16)))
}

#[derive(Clone, Debug)]
struct LogicalExtent {
    logical: u64,
    physical: u64,
    length: u64,
}

fn descriptor(
    header: &[u8],
    offset: usize,
    expected: u32,
    tables: usize,
) -> Result<(usize, usize)> {
    let start = usize::try_from(le32(header, offset)?)?;
    let count = usize::try_from(le32(header, offset + 4)?)?;
    let size = le32(header, offset + 8)?;
    if size != expected || count > 1_000_000 {
        return Err(invalid("invalid LP table descriptor").into());
    }
    let bytes = count
        .checked_mul(expected as usize)
        .ok_or_else(|| invalid("LP descriptor overflow"))?;
    if start.checked_add(bytes).is_none_or(|end| end > tables) {
        return Err(invalid("LP descriptor outside tables").into());
    }
    Ok((start, count))
}

fn lp_system(disk: &Disk, super_region: Region) -> Result<(Vec<LogicalExtent>, u64, (u16, u16))> {
    let geometry = disk.read(add(super_region.offset, 4096, "locating LP geometry")?, 52)?;
    if le32(&geometry, 0)? != LP_GEOMETRY_MAGIC || le32(&geometry, 4)? != 52 {
        return Err(invalid("invalid LP geometry").into());
    }
    let mut geometry_checked = geometry.clone();
    geometry_checked[8..40].fill(0);
    if sha256(&geometry_checked) != geometry[8..40] {
        return Err(invalid("LP geometry SHA-256 mismatch").into());
    }
    let max_size = usize::try_from(le32(&geometry, 40)?)?;
    let slots = le32(&geometry, 44)?;
    if max_size == 0
        || max_size > MAX_TABLE_BYTES
        || max_size % 4096 != 0
        || !(1..=4).contains(&slots)
        || le32(&geometry, 48)? != 4096
    {
        return Err(invalid("unsupported LP geometry values").into());
    }
    let metadata_offset = add(super_region.offset, 3 * 4096, "locating LP metadata")?;
    let prefix = disk.read(metadata_offset, 128)?;
    if le32(&prefix, 0)? != LP_HEADER_MAGIC {
        return Err(invalid("invalid LP metadata magic").into());
    }
    let major = le16(&prefix, 4)?;
    let minor = le16(&prefix, 6)?;
    let header_size = usize::try_from(le32(&prefix, 8)?)?;
    if major != 10 || header_size != 128 {
        return Err(invalid("unsupported LP metadata version").into());
    }
    let header = disk.read(metadata_offset, header_size)?;
    let mut checked = header.clone();
    checked[12..44].fill(0);
    if sha256(&checked) != header[12..44] {
        return Err(invalid("LP header SHA-256 mismatch").into());
    }
    let table_size = usize::try_from(le32(&header, 44)?)?;
    if table_size > max_size - header_size {
        return Err(invalid("LP tables exceed metadata slot").into());
    }
    let tables = disk.read(
        add(metadata_offset, header_size as u64, "locating LP tables")?,
        table_size,
    )?;
    if sha256(&tables) != header[48..80] {
        return Err(invalid("LP table SHA-256 mismatch").into());
    }
    let (po, pn) = descriptor(&header, 80, 52, table_size)?;
    let (eo, en) = descriptor(&header, 92, 24, table_size)?;
    let (_bo, bn) = descriptor(&header, 116, 64, table_size)?;
    if bn == 0 {
        return Err(invalid("LP has no block device").into());
    }
    let mut partition = None;
    for i in 0..pn {
        let e = &tables[po + i * 52..po + (i + 1) * 52];
        let end = e[..36].iter().position(|b| *b == 0).unwrap_or(36);
        if &e[..end] == b"system" {
            if partition.is_some() {
                return Err(invalid("duplicate LP system partition").into());
            }
            partition = Some((
                usize::try_from(le32(e, 40)?)?,
                usize::try_from(le32(e, 44)?)?,
            ));
        }
    }
    let (first, count) = partition.ok_or_else(|| invalid("LP system partition missing"))?;
    if count == 0 || first.checked_add(count).is_none_or(|v| v > en) {
        return Err(invalid("system LP extents invalid").into());
    }
    let mut extents = Vec::with_capacity(count);
    let mut logical = 0u64;
    for i in first..first + count {
        let e = &tables[eo + i * 24..eo + (i + 1) * 24];
        let sectors = le64(e, 0)?;
        let kind = le32(e, 8)?;
        let target = le64(e, 12)?;
        let source = usize::try_from(le32(e, 20)?)?;
        if sectors == 0 || kind != 0 || source >= bn {
            return Err(invalid("unsupported LP system extent").into());
        }
        let length = mul(sectors, SECTOR, "sizing LP extent")?;
        let physical = add(
            super_region.offset,
            mul(target, SECTOR, "locating LP extent")?,
            "locating LP extent",
        )?;
        if add(physical, length, "checking LP extent")?
            > add(super_region.offset, super_region.size, "checking super")?
        {
            return Err(invalid("LP extent outside super").into());
        }
        extents.push(LogicalExtent {
            logical,
            physical,
            length,
        });
        logical = add(logical, length, "sizing system")?;
    }
    Ok((extents, logical, (major, minor)))
}

struct ExtentView<'a> {
    disk: &'a Disk,
    extents: Vec<LogicalExtent>,
    size: u64,
    reads: u64,
    bytes: u64,
}
impl<'a> ExtentView<'a> {
    fn read(&mut self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let end = add(offset, length as u64, "checking logical read")?;
        if end > self.size {
            return Err(invalid("read outside logical partition").into());
        }
        let mut out = vec![0; length];
        let mut at = offset;
        let mut written = 0usize;
        while written < length {
            let extent = self
                .extents
                .iter()
                .find(|e| at >= e.logical && at < e.logical + e.length)
                .ok_or_else(|| invalid("logical extent hole"))?;
            let within = at - extent.logical;
            let take = usize::try_from((extent.length - within).min((length - written) as u64))?;
            out[written..written + take]
                .copy_from_slice(&self.disk.read(extent.physical + within, take)?);
            written += take;
            at += take as u64;
            self.reads += 1;
            self.bytes += take as u64;
        }
        Ok(out)
    }
}

struct Erofs<'a> {
    view: ExtentView<'a>,
    block: u64,
    meta: u64,
    root: u64,
    incompat: u32,
}
impl<'a> Erofs<'a> {
    fn open(mut view: ExtentView<'a>) -> Result<Self> {
        let sb = view.read(1024, 128)?;
        if le32(&sb, 0)? != EROFS_MAGIC {
            return Err(invalid("system is not EROFS").into());
        }
        let bits = sb[12];
        if bits != 12 {
            return Err(invalid("only 4KiB EROFS is supported").into());
        }
        let incompat = le32(&sb, 80)?;
        if incompat & !0x3 != 0 || incompat & 1 == 0 {
            return Err(invalid("unsupported EROFS feature set").into());
        }
        Ok(Self {
            view,
            block: 1 << bits,
            meta: u64::from(le32(&sb, 40)?),
            root: u64::from(le16(&sb, 14)?),
            incompat,
        })
    }
    fn inode(&mut self, nid: u64) -> Result<Inode> {
        let offset = add(
            mul(self.meta, self.block, "locating EROFS metadata")?,
            mul(nid, 32, "locating EROFS inode")?,
            "locating EROFS inode",
        )?;
        let first = self.view.read(offset, 64)?;
        inode::decode(nid, offset, &first)
    }
    fn flat_data(&mut self, inode: &Inode, maximum: u64) -> Result<Vec<u8>> {
        if inode.size > maximum {
            return Err(invalid("EROFS flat file too large").into());
        }
        let raw = u64::from(le32(&self.view.read(inode.offset + 16, 4)?, 0)?);
        match inode.layout {
            0 => self.view.read(
                mul(raw, self.block, "locating flat data")?,
                usize::try_from(inode.size)?,
            ),
            2 => {
                let full = inode.size / self.block * self.block;
                let mut out = if full == 0 {
                    Vec::new()
                } else {
                    self.view.read(
                        mul(raw, self.block, "locating inline data blocks")?,
                        usize::try_from(full)?,
                    )?
                };
                let tail = inode.size - full;
                if tail != 0 {
                    out.extend(self.view.read(
                        add(
                            inode.offset,
                            inode.inode_size + inode.xattr_size,
                            "locating inline tail",
                        )?,
                        usize::try_from(tail)?,
                    )?);
                }
                Ok(out)
            }
            _ => Err(invalid("compressed directory is unsupported").into()),
        }
    }
    fn child(&mut self, directory: &Inode, wanted: &[u8]) -> Result<Inode> {
        if directory.mode & 0xf000 != 0x4000 {
            return Err(invalid("EROFS path parent is not a directory").into());
        }
        let data = self.flat_data(directory, 64 * 1024 * 1024)?;
        for (nid, name) in directory::entries(&data, self.block as usize)? {
            if name == wanted {
                return self.inode(nid);
            }
        }
        Err(invalid(format!(
            "EROFS path component {:?} missing",
            String::from_utf8_lossy(wanted)
        ))
        .into())
    }
    fn target_path(&mut self, path: &str) -> Result<Inode> {
        if !path.starts_with('/') || path.ends_with('/') || path.contains("//") {
            return Err(invalid("target path must be absolute and normalized").into());
        }
        let components: Vec<&str> = path.split('/').skip(1).collect();
        if components.is_empty()
            || components
                .iter()
                .any(|component| component.is_empty() || *component == "." || *component == "..")
        {
            return Err(invalid("target path must contain normalized components").into());
        }
        let mut inode = self.inode(self.root)?;
        for component in components {
            inode = self.child(&inode, component.as_bytes())?;
        }
        Ok(inode)
    }
}

#[derive(Clone, Copy, Debug)]
struct Index {
    kind: u8,
    low: u16,
    pblk: Option<u32>,
}

fn packed_bits(bytes: &[u8], bit: usize, count: usize) -> Result<u32> {
    if count > 24 {
        return Err(invalid("packed field too wide").into());
    }
    let byte = bit / 8;
    let shift = bit % 8;
    let mut word = 0u64;
    for i in 0..5 {
        if let Some(v) = bytes.get(byte + i) {
            word |= u64::from(*v) << (i * 8);
        }
    }
    Ok(((word >> shift) & ((1u64 << count) - 1)) as u32)
}

fn compact_index(
    fs: &mut Erofs<'_>,
    _inode: &Inode,
    base: u64,
    total: usize,
    lcn: usize,
) -> Result<Index> {
    let initial = usize::try_from(((32 - base % 32) / 4) % 8)?;
    let compact2 = if initial < total {
        (total - initial) / 16 * 16
    } else {
        0
    };
    let (mut pos, shift, local) = if lcn < initial {
        (base + (lcn * 4) as u64, 2usize, lcn)
    } else if lcn - initial < compact2 {
        (
            base + (initial * 4 + (lcn - initial) * 2) as u64,
            1usize,
            lcn - initial,
        )
    } else {
        let remaining = lcn - initial - compact2;
        (
            base + (initial * 4 + compact2 * 2 + remaining * 4) as u64,
            2usize,
            remaining,
        )
    };
    let _ = local;
    let count = if shift == 2 { 2 } else { 16 };
    let pack_size = count << shift;
    let pack_start = pos / pack_size as u64 * pack_size as u64;
    let pack = fs.view.read(pack_start, pack_size)?;
    let index = usize::try_from((pos - pack_start) >> shift)?;
    let encode = (pack_size - 4) * 8 / count;
    let raw = packed_bits(&pack, encode * index, 14)?;
    let kind = ((raw >> 12) & 3) as u8;
    let low = (raw & 0xfff) as u16;
    let pblk = if kind == 2 {
        None
    } else {
        let mut blocks = 0u32;
        let mut i = isize::try_from(index)?;
        while i > 0 {
            i -= 1;
            let previous = packed_bits(&pack, encode * usize::try_from(i)?, 14)?;
            let plow = (previous & 0xfff) as u16;
            let pkind = ((previous >> 12) & 3) as u8;
            if pkind == 2 {
                if plow & 0x800 != 0 {
                    i -= 1;
                    blocks = blocks
                        .checked_add(u32::from(plow & 0x7ff))
                        .ok_or_else(|| invalid("pblk overflow"))?;
                } else {
                    if plow <= 1 {
                        return Err(invalid("invalid compact EROFS lookback").into());
                    }
                    i -= isize::try_from(plow - 2)?;
                }
            } else {
                blocks += 1;
            }
        }
        Some(
            le32(&pack, pack_size - 4)?
                .checked_add(blocks)
                .ok_or_else(|| invalid("pblk overflow"))?,
        )
    };
    pos = 0;
    let _ = pos;
    Ok(Index { kind, low, pblk })
}

fn lz4_decode(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(expected);
    let mut cursor = 0usize;
    while cursor < input.len() {
        let token = input[cursor];
        cursor += 1;
        let mut literals = usize::from(token >> 4);
        if literals == 15 {
            loop {
                let value = *input
                    .get(cursor)
                    .ok_or_else(|| invalid("truncated LZ4 literal length"))?;
                cursor += 1;
                literals = literals
                    .checked_add(usize::from(value))
                    .ok_or_else(|| invalid("LZ4 length overflow"))?;
                if value != 255 {
                    break;
                }
            }
        }
        let literal_end = cursor
            .checked_add(literals)
            .ok_or_else(|| invalid("LZ4 literal overflow"))?;
        if literal_end > input.len()
            || output
                .len()
                .checked_add(literals)
                .is_none_or(|v| v > expected)
        {
            return Err(invalid("invalid LZ4 literals").into());
        }
        output.extend_from_slice(&input[cursor..literal_end]);
        cursor = literal_end;
        if cursor == input.len() {
            break;
        }
        let offset = usize::from(le16(input, cursor)?);
        cursor += 2;
        if offset == 0 || offset > output.len() {
            return Err(invalid("invalid LZ4 match offset").into());
        }
        let mut matched = usize::from(token & 15) + 4;
        if token & 15 == 15 {
            loop {
                let value = *input
                    .get(cursor)
                    .ok_or_else(|| invalid("truncated LZ4 match length"))?;
                cursor += 1;
                matched = matched
                    .checked_add(usize::from(value))
                    .ok_or_else(|| invalid("LZ4 length overflow"))?;
                if value != 255 {
                    break;
                }
            }
        }
        if output
            .len()
            .checked_add(matched)
            .is_none_or(|v| v > expected)
        {
            return Err(invalid("LZ4 output exceeds extent").into());
        }
        for _ in 0..matched {
            let value = output[output.len() - offset];
            output.push(value);
        }
    }
    if output.len() != expected {
        return Err(invalid("LZ4 output length mismatch").into());
    }
    Ok(output)
}

fn extract(
    fs: &mut Erofs<'_>,
    inode: &Inode,
    output_path: &Path,
    expected_prefix: Option<&[u8]>,
) -> Result<(u64, [u8; 32], usize)> {
    if inode.size == 0 || inode.size > MAX_TARGET_BYTES {
        return Err(invalid("unsupported target EROFS layout or size").into());
    }
    if inode.layout != 3 {
        let decoded = fs.flat_data(inode, MAX_TARGET_BYTES)?;
        if expected_prefix.is_some_and(|prefix| !decoded.starts_with(prefix)) {
            return Err(invalid("target file has an unexpected format").into());
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_path)?;
        output.write_all(&decoded)?;
        inode.persist(&output)?;
        output.sync_all()?;
        return Ok((decoded.len() as u64, sha256(&decoded), 0));
    }
    let header_pos = align(
        add(
            inode.offset,
            inode.inode_size + inode.xattr_size,
            "locating compression header",
        )?,
        8,
    )?;
    let header = fs.view.read(header_pos, 8)?;
    let advise = le16(&header, 4)?;
    if advise & !0x7 != 0 || advise & 0x7 != 0x7 || header[6] != 0 || header[7] != 0 {
        return Err(invalid("unsupported EROFS compression header").into());
    }
    let index_base = header_pos + 8;
    let total = usize::try_from(inode.size.div_ceil(fs.block))?;
    let mut heads = Vec::new();
    for lcn in 0..total {
        let index = compact_index(fs, inode, index_base, total, lcn)?;
        if index.kind != 2 {
            if index.kind > 1 || u64::from(index.low) >= fs.block {
                return Err(invalid("unsupported EROFS head index").into());
            }
            let start =
                mul(lcn as u64, fs.block, "locating logical cluster")? + u64::from(index.low);
            // erofs-utils z_erofs_fini_full_indexes emits a final PLAIN
            // index at EOF when clusterofs != 0. It terminates the previous
            // extent; it does not describe an additional data extent.
            if index.kind == 0 && lcn + 1 == total && start == inode.size {
                continue;
            }
            if start >= inode.size {
                return Err(invalid("EROFS head lies beyond file contents").into());
            }
            heads.push((start, lcn, index));
        }
    }
    if heads.is_empty() || heads[0].0 != 0 {
        return Err(invalid("EROFS compressed file has no initial head").into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut physical_blocks = 0u64;
    for i in 0..heads.len() {
        let (start, lcn, head) = heads[i];
        let end = if i + 1 == heads.len() {
            inode.size
        } else {
            heads[i + 1].0
        };
        if start != written || end <= start {
            return Err(invalid(format!(
                "overlapping or holed EROFS compression extents: head={i}/{} lcn={lcn} start={start} end={end} written={written} size={} kind={} low={}",
                heads.len(), inode.size, head.kind, head.low
            )).into());
        }
        let expected = usize::try_from(end - start)?;
        let pblk = u64::from(
            head.pblk
                .ok_or_else(|| invalid("head without physical block"))?,
        );
        let (decoded, extent_blocks) = if head.kind == 0 {
            if expected > fs.block as usize {
                return Err(invalid("oversized plain EROFS extent").into());
            }
            (
                fs.view
                    .read(mul(pblk, fs.block, "locating plain pcluster")?, expected)?,
                1usize,
            )
        } else {
            let count = if lcn + 1 < total {
                let next = compact_index(fs, inode, index_base, total, lcn + 1)?;
                if next.kind == 2 && next.low & 0x800 != 0 {
                    usize::from(next.low & 0x7ff)
                } else if next.kind != 2 {
                    1
                } else {
                    return Err(invalid("missing EROFS compressed block count").into());
                }
            } else {
                1
            };
            let compressed = count
                .checked_mul(fs.block as usize)
                .ok_or_else(|| invalid("pcluster overflow"))?;
            if compressed == 0 || compressed > MAX_PCLUSTER_BYTES {
                return Err(invalid("invalid EROFS pcluster size").into());
            }
            let bytes = fs.view.read(
                mul(pblk, fs.block, "locating compressed pcluster")?,
                compressed,
            )?;
            let margin = bytes[..fs.block as usize]
                .iter()
                .position(|b| *b != 0)
                .ok_or_else(|| invalid("zero EROFS pcluster"))?;
            (lz4_decode(&bytes[margin..], expected)?, count)
        };
        physical_blocks = physical_blocks
            .checked_add(extent_blocks as u64)
            .ok_or_else(|| invalid("physical block count overflow"))?;
        if written == 0 && expected_prefix.is_some_and(|prefix| !decoded.starts_with(prefix)) {
            return Err(invalid("target file has an unexpected format").into());
        }
        output.write_all(&decoded)?;
        hasher.update(&decoded);
        written += decoded.len() as u64;
    }
    if written != inode.size {
        return Err(invalid("extracted APEX size mismatch").into());
    }
    if physical_blocks != u64::from(inode.compressed_blocks) {
        return Err(invalid(format!(
            "EROFS physical block count mismatch: index={physical_blocks} inode={}",
            inode.compressed_blocks
        ))
        .into());
    }
    inode.persist(&output)?;
    output.sync_all()?;
    Ok((written, hasher.finish(), heads.len()))
}

struct Sha256 {
    state: [u32; 8],
    buffer: Vec<u8>,
    length: u64,
}
impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: Vec::new(),
            length: 0,
        }
    }
    fn update(&mut self, bytes: &[u8]) {
        self.length += bytes.len() as u64;
        self.buffer.extend_from_slice(bytes);
        while self.buffer.len() >= 64 {
            let block: [u8; 64] = self.buffer[..64].try_into().unwrap();
            self.compress(&block);
            self.buffer.drain(..64);
        }
    }
    fn compress(&mut self, b: &[u8; 64]) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        for (i, c) in b.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(c.try_into().unwrap())
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1)
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2)
        }
        let old = self.state;
        self.state = [
            old[0].wrapping_add(a),
            old[1].wrapping_add(b),
            old[2].wrapping_add(c),
            old[3].wrapping_add(d),
            old[4].wrapping_add(e),
            old[5].wrapping_add(f),
            old[6].wrapping_add(g),
            old[7].wrapping_add(h),
        ];
    }
    fn finish(mut self) -> [u8; 32] {
        let bits = self.length * 8;
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0)
        }
        self.buffer.extend_from_slice(&bits.to_be_bytes());
        while !self.buffer.is_empty() {
            let block: [u8; 64] = self.buffer[..64].try_into().unwrap();
            self.compress(&block);
            self.buffer.drain(..64);
        }
        let mut out = [0; 32];
        for (i, v) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes())
        }
        out
    }
}
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finish()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

mod symlink;

fn run() -> Result<()> {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_default();
    let input = args.next().map(PathBuf::from).ok_or_else(|| {
        invalid(format!(
            "usage: {} INPUT-system.img OUTPUT [APEX_NAME | --path INTERNAL_PATH | --symlink INTERNAL_PATH | --stat INTERNAL_PATH]",
            PathBuf::from(program).display()
        ))
    })?;
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| invalid("missing OUTPUT.apex"))?;
    let selector = args.next().and_then(|value| value.into_string().ok());
    let preserve_link = selector.as_deref() == Some("--symlink");
    let metadata_only = selector.as_deref() == Some("--stat");
    if metadata_only && output != Path::new("-") {
        return Err(invalid("--stat requires OUTPUT '-' (stdout only)").into());
    }
    let (target_path, expected_prefix) = if selector.as_deref() == Some("--path")
        || preserve_link
        || metadata_only
    {
        let path = args
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| invalid("path selector requires an internal absolute path"))?;
        (path, None)
    } else {
        let apex_name = selector.unwrap_or_else(|| DEFAULT_APEX_NAME.to_owned());
        if apex_name.is_empty()
            || apex_name == "."
            || apex_name == ".."
            || apex_name.as_bytes().contains(&b'/')
        {
            return Err(invalid("target APEX name must be one normalized path component").into());
        }
        (
            format!("/system/apex/{apex_name}"),
            Some(b"PK\x03\x04".as_slice()),
        )
    };
    if args.next().is_some() {
        return Err(invalid("too many arguments").into());
    }
    let disk = Disk::open(&input)?;
    let (super_region, gpt) = gpt_super(&disk)?;
    let (extents, system_size, lp) = lp_system(&disk, super_region)?;
    let view = ExtentView {
        disk: &disk,
        extents,
        size: system_size,
        reads: 0,
        bytes: 0,
    };
    let mut fs = Erofs::open(view)?;
    let inode = fs.target_path(&target_path)?;
    if metadata_only {
        println!(
            "mode={:o} uid={} gid={} size={}",
            inode.mode, inode.uid, inode.gid, inode.size
        );
        return Ok(());
    }
    if preserve_link {
        return symlink::extract(&mut fs, &inode, &output);
    }
    if output == Path::new("-") {
        if inode.mode & 0xf000 != 0x4000 {
            return Err(invalid("listing target is not a directory").into());
        }
        let data = fs.flat_data(&inode, 64 * 1024 * 1024)?;
        for (_, name) in directory::entries(&data, fs.block as usize)? {
            println!("{}", std::str::from_utf8(&name)?);
        }
        return Ok(());
    }
    if inode.mode & 0xf000 != 0x8000 {
        return Err(invalid("target path is not a regular file").into());
    }
    let (bytes, digest, pclusters) = extract(&mut fs, &inode, &output, expected_prefix)?;
    println!(
        "super-i18n-apex-extract: input={} output={}",
        input.display(),
        output.display()
    );
    println!(
        "gpt.version={}.{} super.offset={} super.size={}",
        gpt.0, gpt.1, super_region.offset, super_region.size
    );
    println!(
        "lp.version={}.{} system.extents={} system.size={}",
        lp.0,
        lp.1,
        fs.view.extents.len(),
        system_size
    );
    println!(
        "erofs.block_size={} erofs.incompat={:#x} target.nid={} target.compressed_blocks={} target.pclusters={}",
        fs.block, fs.incompat, inode.nid, inode.compressed_blocks, pclusters
    );
    println!(
        "target.path={target_path} target.bytes={bytes} target.sha256={}",
        hex(&digest),
    );
    println!(
        "io.physical_reads={} io.physical_bytes={}",
        fs.view.reads, fs.view.bytes
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("super-i18n-apex-extract: {e}");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hashes_known_vector() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )
    }
    #[test]
    fn lz4_literal() {
        assert_eq!(lz4_decode(b"\x30abc", 3).unwrap(), b"abc")
    }
    #[test]
    fn readers_reject_bounds() {
        assert!(le64(&[0; 7], 0).is_err());
        assert!(packed_bits(&[0], 8, 14).unwrap() == 0)
    }
    #[test]
    fn malformed_lz4_is_rejected() {
        assert!(lz4_decode(b"\x10", 1).is_err());
        assert!(lz4_decode(b"\x00\x00\x00", 4).is_err());
        assert!(lz4_decode(b"\x40abcd", 3).is_err());
    }
}
