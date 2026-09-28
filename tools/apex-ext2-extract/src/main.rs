use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
mod inventory;
mod tree;

const TARGET_APEX_ENTRY: &[u8] = b"apex_payload.img";
const DEFAULT_EXT4_PATH: &str = "/javalib/core-icu4j.jar";
const MAX_APEX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ZIP_ENTRIES: u16 = 16_384;
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXTENTS: usize = 1_000_000;
const EXT4_EXTENTS_FL: u32 = 0x0008_0000;
const EXT4_FEATURE_INCOMPAT_64BIT: u32 = 0x80;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn le16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| invalid("truncated u16"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("truncated u32"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn checked_add(left: u64, right: u64, context: &str) -> Result<u64> {
    left.checked_add(right)
        .ok_or_else(|| invalid(format!("overflow while {context}")).into())
}

fn checked_mul(left: u64, right: u64, context: &str) -> Result<u64> {
    left.checked_mul(right)
        .ok_or_else(|| invalid(format!("overflow while {context}")).into())
}

#[derive(Clone, Copy, Debug)]
struct StoredZipEntry {
    data_offset: u64,
    size: u64,
}

fn read_exact_at(file: &mut File, offset: u64, bytes: &mut [u8]) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(bytes)?;
    Ok(())
}

fn find_stored_zip_entry(file: &mut File, target: &[u8]) -> Result<StoredZipEntry> {
    let file_size = file.metadata()?.len();
    if !(22..=MAX_APEX_BYTES).contains(&file_size) {
        return Err(invalid(format!(
            "APEX size {file_size} is outside the supported range"
        ))
        .into());
    }

    let tail_size = file_size.min(65_557) as usize;
    let mut tail = vec![0_u8; tail_size];
    read_exact_at(file, file_size - tail_size as u64, &mut tail)?;
    let eocd = (0..=tail.len() - 22)
        .rev()
        .find(|&offset| {
            tail.get(offset..offset + 4) == Some(b"PK\x05\x06")
                && le16(&tail, offset + 20)
                    .ok()
                    .map(|length| offset + 22 + usize::from(length) == tail.len())
                    .unwrap_or(false)
        })
        .ok_or_else(|| invalid("ZIP end-of-central-directory record not found"))?;

    if le16(&tail, eocd + 4)? != 0 || le16(&tail, eocd + 6)? != 0 {
        return Err(invalid("multi-disk ZIP APEX is unsupported").into());
    }
    let disk_entries = le16(&tail, eocd + 8)?;
    let total_entries = le16(&tail, eocd + 10)?;
    if disk_entries != total_entries || total_entries == u16::MAX {
        return Err(invalid("ZIP64 or inconsistent ZIP entry count is unsupported").into());
    }
    if total_entries > MAX_ZIP_ENTRIES {
        return Err(invalid("APEX ZIP has too many entries").into());
    }
    let central_size = u64::from(le32(&tail, eocd + 12)?);
    let central_offset = u64::from(le32(&tail, eocd + 16)?);
    if central_size == u64::from(u32::MAX) || central_offset == u64::from(u32::MAX) {
        return Err(invalid("ZIP64 APEX is unsupported").into());
    }
    let central_end = checked_add(central_offset, central_size, "checking central directory")?;
    if central_end > file_size {
        return Err(invalid("ZIP central directory lies outside the APEX").into());
    }

    let mut cursor = central_offset;
    for _ in 0..total_entries {
        let mut header = [0_u8; 46];
        read_exact_at(file, cursor, &mut header)?;
        if &header[0..4] != b"PK\x01\x02" {
            return Err(invalid("invalid ZIP central-directory entry").into());
        }
        let flags = le16(&header, 8)?;
        let method = le16(&header, 10)?;
        let compressed_size = u64::from(le32(&header, 20)?);
        let uncompressed_size = u64::from(le32(&header, 24)?);
        let name_length = usize::from(le16(&header, 28)?);
        let extra_length = u64::from(le16(&header, 30)?);
        let comment_length = u64::from(le16(&header, 32)?);
        let start_disk = le16(&header, 34)?;
        let local_offset = u64::from(le32(&header, 42)?);
        if compressed_size == u64::from(u32::MAX)
            || uncompressed_size == u64::from(u32::MAX)
            || local_offset == u64::from(u32::MAX)
        {
            return Err(invalid("ZIP64 entry is unsupported").into());
        }

        let mut name = vec![0_u8; name_length];
        read_exact_at(file, cursor + 46, &mut name)?;
        let variable_size = checked_add(
            name_length as u64,
            checked_add(extra_length, comment_length, "checking ZIP entry")?,
            "checking ZIP entry",
        )?;
        cursor = checked_add(cursor, 46 + variable_size, "advancing ZIP directory")?;
        if cursor > central_end {
            return Err(invalid("ZIP central-directory entry overruns the directory").into());
        }
        if name != target {
            continue;
        }
        if flags & 1 != 0 || start_disk != 0 {
            return Err(invalid("encrypted or split APEX payload is unsupported").into());
        }
        if method != 0 || compressed_size != uncompressed_size {
            return Err(invalid("apex_payload.img must be stored without ZIP compression").into());
        }

        let mut local = [0_u8; 30];
        read_exact_at(file, local_offset, &mut local)?;
        if &local[0..4] != b"PK\x03\x04" {
            return Err(invalid("invalid local ZIP header for apex_payload.img").into());
        }
        if le16(&local, 6)? & 1 != 0 || le16(&local, 8)? != 0 {
            return Err(invalid("local apex_payload.img header is encrypted or compressed").into());
        }
        let local_name_length = u64::from(le16(&local, 26)?);
        let local_extra_length = u64::from(le16(&local, 28)?);
        if local_name_length != target.len() as u64 {
            return Err(invalid("local apex_payload.img name length is inconsistent").into());
        }
        let mut local_name = vec![0_u8; target.len()];
        read_exact_at(file, local_offset + 30, &mut local_name)?;
        if local_name != target {
            return Err(invalid("local apex_payload.img name is inconsistent").into());
        }
        let data_offset = checked_add(
            local_offset,
            checked_add(
                30,
                checked_add(
                    local_name_length,
                    local_extra_length,
                    "checking local ZIP header",
                )?,
                "checking local ZIP header",
            )?,
            "checking local ZIP header",
        )?;
        if checked_add(
            data_offset,
            uncompressed_size,
            "checking APEX payload bounds",
        )? > file_size
        {
            return Err(invalid("apex_payload.img lies outside the APEX").into());
        }
        return Ok(StoredZipEntry {
            data_offset,
            size: uncompressed_size,
        });
    }
    Err(invalid("APEX does not contain apex_payload.img").into())
}

#[derive(Clone, Debug)]
struct Inode {
    mode: u16,
    size: u64,
    flags: u32,
    extent_root: [u8; 60],
}

impl Inode {
    fn is_directory(&self) -> bool {
        self.mode & 0xf000 == 0x4000
    }

    fn is_regular(&self) -> bool {
        self.mode & 0xf000 == 0x8000
    }
}

#[derive(Clone, Copy, Debug)]
struct Extent {
    logical_block: u64,
    physical_block: u64,
    block_count: u64,
}

struct Ext4Image {
    file: File,
    payload: StoredZipEntry,
    block_size: u64,
    inode_size: u64,
    inodes_per_group: u64,
    descriptor_size: u64,
    descriptor_table_offset: u64,
    has_64bit_descriptors: bool,
}

impl Ext4Image {
    fn open(mut file: File, payload: StoredZipEntry) -> Result<Self> {
        if payload.size < 2048 {
            return Err(invalid("APEX payload is too small for an ext4 superblock").into());
        }
        let mut superblock = [0_u8; 1024];
        read_exact_at(&mut file, payload.data_offset + 1024, &mut superblock)?;
        if le16(&superblock, 0x38)? != 0xef53 {
            return Err(invalid("apex_payload.img is not an ext2/3/4 filesystem").into());
        }
        let block_shift = le32(&superblock, 0x18)?;
        if block_shift > 6 {
            return Err(invalid("unsupported ext4 block size").into());
        }
        let block_size = 1024_u64 << block_shift;
        let inode_size = u64::from(le16(&superblock, 0x58)?);
        if inode_size < 128 || inode_size > block_size || !inode_size.is_power_of_two() {
            return Err(invalid("invalid ext4 inode size").into());
        }
        let inodes_per_group = u64::from(le32(&superblock, 0x28)?);
        if inodes_per_group == 0 {
            return Err(invalid("invalid ext4 inodes-per-group value").into());
        }
        let first_data_block = u64::from(le32(&superblock, 0x14)?);
        let incompat = le32(&superblock, 0x60)?;
        let has_64bit_descriptors = incompat & EXT4_FEATURE_INCOMPAT_64BIT != 0;
        let descriptor_size = if has_64bit_descriptors {
            u64::from(le16(&superblock, 0xfe)?).max(64)
        } else {
            32
        };
        if descriptor_size > block_size || descriptor_size % 8 != 0 {
            return Err(invalid("invalid ext4 group-descriptor size").into());
        }
        let descriptor_table_offset = checked_mul(
            first_data_block + 1,
            block_size,
            "locating ext4 group descriptors",
        )?;
        Ok(Self {
            file,
            payload,
            block_size,
            inode_size,
            inodes_per_group,
            descriptor_size,
            descriptor_table_offset,
            has_64bit_descriptors,
        })
    }

    fn read_payload_exact(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let end = checked_add(offset, bytes.len() as u64, "checking ext4 read")?;
        if end > self.payload.size {
            return Err(invalid("ext4 read lies outside apex_payload.img").into());
        }
        read_exact_at(&mut self.file, self.payload.data_offset + offset, bytes)
    }

    fn read_block(&mut self, block: u64) -> Result<Vec<u8>> {
        let offset = checked_mul(block, self.block_size, "locating ext4 block")?;
        let mut bytes = vec![0_u8; self.block_size as usize];
        self.read_payload_exact(offset, &mut bytes)?;
        Ok(bytes)
    }

    fn read_inode(&mut self, inode_number: u32) -> Result<Inode> {
        if inode_number == 0 {
            return Err(invalid("ext4 inode zero is invalid").into());
        }
        let inode_index = u64::from(inode_number - 1);
        let group = inode_index / self.inodes_per_group;
        let index_in_group = inode_index % self.inodes_per_group;
        let descriptor_offset = checked_add(
            self.descriptor_table_offset,
            checked_mul(
                group,
                self.descriptor_size,
                "locating ext4 group descriptor",
            )?,
            "locating ext4 group descriptor",
        )?;
        let mut descriptor = vec![0_u8; self.descriptor_size as usize];
        self.read_payload_exact(descriptor_offset, &mut descriptor)?;
        let inode_table_low = u64::from(le32(&descriptor, 8)?);
        let inode_table_high = if self.has_64bit_descriptors {
            u64::from(le32(&descriptor, 0x28)?)
        } else {
            0
        };
        let inode_table_block = inode_table_low | (inode_table_high << 32);
        if inode_table_block == 0 {
            return Err(invalid("ext4 group has no inode table").into());
        }
        let inode_offset = checked_add(
            checked_mul(
                inode_table_block,
                self.block_size,
                "locating ext4 inode table",
            )?,
            checked_mul(index_in_group, self.inode_size, "locating ext4 inode")?,
            "locating ext4 inode",
        )?;
        let mut bytes = vec![0_u8; self.inode_size as usize];
        self.read_payload_exact(inode_offset, &mut bytes)?;
        let mode = le16(&bytes, 0)?;
        let size = u64::from(le32(&bytes, 4)?) | (u64::from(le32(&bytes, 0x6c)?) << 32);
        let flags = le32(&bytes, 0x20)?;
        let mut extent_root = [0_u8; 60];
        extent_root.copy_from_slice(
            bytes
                .get(0x28..0x28 + 60)
                .ok_or_else(|| invalid("truncated ext4 inode block map"))?,
        );
        Ok(Inode {
            mode,
            size,
            flags,
            extent_root,
        })
    }

    fn collect_extent_node(
        &mut self,
        node: &[u8],
        expected_depth: Option<u16>,
        extents: &mut Vec<Extent>,
    ) -> Result<()> {
        if node.len() < 12 || le16(node, 0)? != 0xf30a {
            return Err(invalid("invalid ext4 extent-tree node").into());
        }
        let entries = usize::from(le16(node, 2)?);
        let maximum = usize::from(le16(node, 4)?);
        let depth = le16(node, 6)?;
        if depth > 5 || expected_depth.is_some_and(|value| value != depth) {
            return Err(invalid("invalid ext4 extent-tree depth").into());
        }
        let capacity = (node.len() - 12) / 12;
        if entries > maximum || entries > capacity {
            return Err(invalid("invalid ext4 extent-tree entry count").into());
        }
        if extents.len().saturating_add(entries) > MAX_EXTENTS {
            return Err(invalid("ext4 file has too many extents").into());
        }
        for index in 0..entries {
            let offset = 12 + index * 12;
            if depth == 0 {
                let raw_length = le16(node, offset + 4)?;
                if raw_length == 0 || raw_length > 0x8000 {
                    return Err(invalid("unwritten or invalid ext4 extent is unsupported").into());
                }
                let physical =
                    u64::from(le32(node, offset + 8)?) | (u64::from(le16(node, offset + 6)?) << 32);
                if physical == 0 {
                    return Err(invalid("ext4 extent points at block zero").into());
                }
                extents.push(Extent {
                    logical_block: u64::from(le32(node, offset)?),
                    physical_block: physical,
                    block_count: u64::from(raw_length),
                });
            } else {
                let child =
                    u64::from(le32(node, offset + 4)?) | (u64::from(le16(node, offset + 8)?) << 32);
                if child == 0 {
                    return Err(invalid("ext4 extent index points at block zero").into());
                }
                let child_node = self.read_block(child)?;
                self.collect_extent_node(&child_node, Some(depth - 1), extents)?;
            }
        }
        Ok(())
    }

    fn extents(&mut self, inode: &Inode) -> Result<Vec<Extent>> {
        if inode.flags & EXT4_EXTENTS_FL == 0 {
            return Err(
                invalid("legacy ext2 block maps are unsupported; extents are required").into(),
            );
        }
        let mut extents = Vec::new();
        self.collect_extent_node(&inode.extent_root, None, &mut extents)?;
        extents.sort_by_key(|extent| extent.logical_block);
        let mut previous_end = 0_u64;
        for extent in &extents {
            if extent.logical_block < previous_end {
                return Err(invalid("overlapping ext4 extents").into());
            }
            previous_end = checked_add(
                extent.logical_block,
                extent.block_count,
                "checking ext4 extents",
            )?;
        }
        Ok(extents)
    }

    fn read_inode_data(&mut self, inode: &Inode, maximum: u64) -> Result<Vec<u8>> {
        if inode.size > maximum {
            return Err(invalid(format!(
                "ext4 file size {} exceeds the {} byte limit",
                inode.size, maximum
            ))
            .into());
        }
        let size = usize::try_from(inode.size)
            .map_err(|_| invalid("ext4 file does not fit in host address space"))?;
        let mut output = vec![0_u8; size];
        for extent in self.extents(inode)? {
            let logical_offset = checked_mul(
                extent.logical_block,
                self.block_size,
                "locating logical extent",
            )?;
            if logical_offset >= inode.size {
                continue;
            }
            let extent_bytes =
                checked_mul(extent.block_count, self.block_size, "sizing ext4 extent")?;
            let bytes_to_read = extent_bytes.min(inode.size - logical_offset);
            let physical_offset = checked_mul(
                extent.physical_block,
                self.block_size,
                "locating physical extent",
            )?;
            let start = usize::try_from(logical_offset)
                .map_err(|_| invalid("logical extent offset is too large"))?;
            let length = usize::try_from(bytes_to_read)
                .map_err(|_| invalid("extent length is too large"))?;
            self.read_payload_exact(physical_offset, &mut output[start..start + length])?;
        }
        Ok(output)
    }

    fn find_directory_entry(&mut self, directory: &Inode, wanted: &[u8]) -> Result<u32> {
        if !directory.is_directory() {
            return Err(invalid("path component parent is not a directory").into());
        }
        let bytes = self.read_inode_data(directory, MAX_DIRECTORY_BYTES)?;
        let mut cursor = 0_usize;
        while cursor < bytes.len() {
            if bytes.len() - cursor < 8 {
                return Err(invalid("truncated ext4 directory entry").into());
            }
            let inode = le32(&bytes, cursor)?;
            let record_length = usize::from(le16(&bytes, cursor + 4)?);
            let name_length = usize::from(bytes[cursor + 6]);
            if record_length < 8
                || record_length % 4 != 0
                || record_length > bytes.len() - cursor
                || name_length > record_length - 8
            {
                return Err(invalid("invalid ext4 directory entry").into());
            }
            if inode != 0 && bytes[cursor + 8..cursor + 8 + name_length] == *wanted {
                return Ok(inode);
            }
            cursor += record_length;
        }
        Err(invalid(format!(
            "ext4 directory entry {:?} was not found",
            String::from_utf8_lossy(wanted)
        ))
        .into())
    }

    fn read_path(&mut self, path: &str) -> Result<Vec<u8>> {
        if !path.starts_with('/') || path.contains("//") {
            return Err(invalid("internal ext4 path must be absolute and normalized").into());
        }
        let components: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        if components.is_empty()
            || components
                .iter()
                .any(|component| *component == "." || *component == "..")
        {
            return Err(invalid("invalid internal ext4 path").into());
        }
        let mut inode = self.read_inode(2)?;
        for (index, component) in components.iter().enumerate() {
            let child_number = self.find_directory_entry(&inode, component.as_bytes())?;
            inode = self.read_inode(child_number)?;
            if index + 1 != components.len() && !inode.is_directory() {
                return Err(invalid("intermediate ext4 path component is not a directory").into());
            }
        }
        if !inode.is_regular() {
            return Err(invalid("target ext4 path is not a regular file").into());
        }
        self.read_inode_data(&inode, MAX_EXTRACTED_BYTES)
    }

    fn list_directory(&mut self, path: &str) -> Result<Vec<String>> {
        if !path.starts_with('/') || path.contains("//") {
            return Err(invalid("internal ext4 path must be absolute and normalized").into());
        }
        let components: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        if components
            .iter()
            .any(|component| *component == "." || *component == "..")
        {
            return Err(invalid("invalid internal ext4 path").into());
        }
        let mut inode = self.read_inode(2)?;
        for component in components {
            let child_number = self.find_directory_entry(&inode, component.as_bytes())?;
            inode = self.read_inode(child_number)?;
        }
        if !inode.is_directory() {
            return Err(invalid("target ext4 path is not a directory").into());
        }
        let bytes = self.read_inode_data(&inode, MAX_DIRECTORY_BYTES)?;
        let mut names = Vec::new();
        let mut cursor = 0_usize;
        while cursor < bytes.len() {
            if bytes.len() - cursor < 8 {
                return Err(invalid("truncated ext4 directory entry").into());
            }
            let inode_number = le32(&bytes, cursor)?;
            let record_length = usize::from(le16(&bytes, cursor + 4)?);
            let name_length = usize::from(bytes[cursor + 6]);
            if record_length < 8
                || record_length % 4 != 0
                || record_length > bytes.len() - cursor
                || name_length > record_length - 8
            {
                return Err(invalid("invalid ext4 directory entry").into());
            }
            if inode_number != 0 {
                let name = std::str::from_utf8(&bytes[cursor + 8..cursor + 8 + name_length])?;
                if name != "." && name != ".." {
                    names.push(name.to_owned());
                }
            }
            cursor += record_length;
        }
        names.sort();
        Ok(names)
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = output.write_all(bytes).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = std::fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

fn usage(program: &str) -> String {
    format!("usage: {program} INPUT.apex OUTPUT [INTERNAL_PATH] [--inventory|--tree]")
}

fn run() -> Result<()> {
    let mut arguments = env::args_os();
    let program = arguments
        .next()
        .and_then(|value| PathBuf::from(value).file_name().map(|name| name.to_owned()))
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "apex-ext2-extract".to_owned());
    let input = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| invalid(usage(&program)))?;
    let output = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| invalid(usage(&program)))?;
    let internal_path = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| DEFAULT_EXT4_PATH.to_owned());
    let mode = match arguments.next() {
        None => "file",
        Some(flag) if flag == "--inventory" && output == Path::new("-") => "inventory",
        Some(flag) if flag == "--tree" && output != Path::new("-") && internal_path == "/" => {
            "tree"
        }
        _ => return Err(invalid(usage(&program)).into()),
    };
    if arguments.next().is_some() {
        return Err(invalid(usage(&program)).into());
    }

    let mut file = File::open(&input)?;
    let payload = find_stored_zip_entry(&mut file, TARGET_APEX_ENTRY)?;
    let mut filesystem = Ext4Image::open(file, payload)?;
    if mode == "tree" {
        filesystem.extract_tree(&output)?;
        println!(
            "APEX payload extracted to {} (not activated)",
            output.display()
        );
        return Ok(());
    }
    if mode == "inventory" {
        for (path, inode) in filesystem.inventory(&internal_path)? {
            println!(
                "mode={:06o} bytes={} path={:?} link={:?}",
                inode.mode,
                inode.size,
                path,
                filesystem.inventory_link(&inode)?
            );
        }
        return Ok(());
    }
    if output == Path::new("-") {
        for name in filesystem.list_directory(&internal_path)? {
            println!("{name}");
        }
        return Ok(());
    }
    let bytes = filesystem.read_path(&internal_path)?;
    if internal_path == DEFAULT_EXT4_PATH && bytes.get(0..4) != Some(b"PK\x03\x04") {
        return Err(invalid("extracted core-icu4j.jar is not a ZIP/JAR file").into());
    }
    write_new_file(&output, &bytes)?;
    println!(
        "apex-ext2-extract: {} -> {} bytes={} internal={}",
        input.display(),
        output.display(),
        bytes.len(),
        internal_path
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("apex-ext2-extract: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_readers_reject_truncation() {
        assert_eq!(le16(&[0x34, 0x12], 0).unwrap(), 0x1234);
        assert_eq!(le32(&[0x78, 0x56, 0x34, 0x12], 0).unwrap(), 0x1234_5678);
        assert!(le32(&[0, 1, 2], 0).is_err());
    }

    #[test]
    fn inode_type_checks_are_exact() {
        let regular = Inode {
            mode: 0x81a4,
            size: 0,
            flags: 0,
            extent_root: [0; 60],
        };
        let directory = Inode {
            mode: 0x41ed,
            ..regular.clone()
        };
        assert!(regular.is_regular());
        assert!(!regular.is_directory());
        assert!(directory.is_directory());
        assert!(!directory.is_regular());
    }
}
