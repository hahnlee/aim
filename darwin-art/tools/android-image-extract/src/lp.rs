//! Android dynamic-partition ("super", liblp) metadata, slot 0 primary copy.
//! Layout reference: system/core/fs_mgr/liblp/include/liblp/metadata_format.h.
use crate::source::{Extent, check_range};
use crate::{Result, invalid, le16, le32, le64, mul};
use sha2::{Digest, Sha256};

use crate::source::ReadAt;

const GEOMETRY_MAGIC: u32 = 0x616c_4467;
const HEADER_MAGIC: u32 = 0x414c_5030;
const RESERVED: u64 = 4096;
const GEOMETRY_SIZE: u64 = 4096;
const SECTOR: u64 = 512;
const MAX_METADATA: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Partition {
    pub name: String,
    pub attributes: u32,
    /// Extents relative to the start of the super region.
    pub extents: Vec<Extent>,
    pub size: u64,
}

pub struct Metadata {
    pub version: (u16, u16),
    pub partitions: Vec<Partition>,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn table(header: &[u8], at: usize, entry: u32, tables: usize) -> Result<(usize, usize)> {
    let offset = le32(header, at)? as usize;
    let count = le32(header, at + 4)? as usize;
    if le32(header, at + 8)? != entry || count > 1_000_000 {
        return Err(invalid("invalid LP table descriptor"));
    }
    let end = count
        .checked_mul(entry as usize)
        .and_then(|bytes| bytes.checked_add(offset));
    if end.is_none_or(|end| end > tables) {
        return Err(invalid("LP table lies outside the metadata"));
    }
    Ok((offset, count))
}

/// Read the partition table of a super region `super_image`.
pub fn read(super_image: &dyn ReadAt) -> Result<Metadata> {
    let geometry = super_image.read_vec(RESERVED, 52)?;
    if le32(&geometry, 0)? != GEOMETRY_MAGIC || le32(&geometry, 4)? != 52 {
        return Err(invalid("invalid LP geometry"));
    }
    let mut checked = geometry.clone();
    checked[8..40].fill(0);
    if sha256(&checked) != geometry[8..40] {
        return Err(invalid("LP geometry SHA-256 mismatch"));
    }
    let max_size = le32(&geometry, 40)? as usize;
    if max_size == 0 || max_size > MAX_METADATA || max_size % 512 != 0 {
        return Err(invalid("unsupported LP metadata size"));
    }
    let metadata_at = RESERVED + 2 * GEOMETRY_SIZE;
    let prefix = super_image.read_vec(metadata_at, 12)?;
    if le32(&prefix, 0)? != HEADER_MAGIC {
        return Err(invalid("invalid LP metadata magic"));
    }
    let version = (le16(&prefix, 4)?, le16(&prefix, 6)?);
    let header_size = le32(&prefix, 8)? as usize;
    if version.0 != 10 || !(header_size == 128 || header_size == 256) {
        return Err(invalid(format!(
            "unsupported LP metadata version {version:?}"
        )));
    }
    let header = super_image.read_vec(metadata_at, header_size)?;
    let mut checked = header.clone();
    checked[12..44].fill(0);
    if sha256(&checked) != header[12..44] {
        return Err(invalid("LP header SHA-256 mismatch"));
    }
    let tables_size = le32(&header, 44)? as usize;
    if tables_size > max_size - header_size {
        return Err(invalid("LP tables exceed the metadata slot"));
    }
    let tables = super_image.read_vec(metadata_at + header_size as u64, tables_size)?;
    if sha256(&tables) != header[48..80] {
        return Err(invalid("LP tables SHA-256 mismatch"));
    }
    let (po, pn) = table(&header, 80, 52, tables_size)?;
    let (eo, en) = table(&header, 92, 24, tables_size)?;
    let (_, bn) = table(&header, 116, 64, tables_size)?;
    if bn != 1 {
        return Err(invalid("only single-device super images are supported"));
    }
    let mut partitions: Vec<Partition> = Vec::with_capacity(pn);
    for i in 0..pn {
        let e = &tables[po + i * 52..po + (i + 1) * 52];
        let end = e[..36].iter().position(|b| *b == 0).unwrap_or(36);
        let name = std::str::from_utf8(&e[..end])
            .map_err(|_| invalid("LP partition name is not UTF-8"))?
            .to_owned();
        if name.is_empty() || partitions.iter().any(|p| p.name == name) {
            return Err(invalid(format!("empty or duplicate LP partition {name:?}")));
        }
        let first = le32(e, 40)? as usize;
        let count = le32(e, 44)? as usize;
        if first.checked_add(count).is_none_or(|last| last > en) {
            return Err(invalid(format!("LP partition {name} extents out of range")));
        }
        let mut extents = Vec::with_capacity(count);
        let mut logical = 0u64;
        for j in first..first + count {
            let x = &tables[eo + j * 24..eo + (j + 1) * 24];
            let length = mul(le64(x, 0)?, SECTOR, "sizing LP extent")?;
            let physical = match le32(x, 8)? {
                0 => {
                    if le32(x, 20)? != 0 {
                        return Err(invalid("LP extent on a secondary block device"));
                    }
                    let at = mul(le64(x, 12)?, SECTOR, "locating LP extent")?;
                    check_range(super_image.size(), at, length)?;
                    Some(at)
                }
                1 => None,
                kind => return Err(invalid(format!("unsupported LP extent type {kind}"))),
            };
            extents.push(Extent {
                logical,
                physical,
                length,
            });
            logical += length;
        }
        partitions.push(Partition {
            name,
            attributes: le32(e, 36)?,
            extents,
            size: logical,
        });
    }
    Ok(Metadata {
        version,
        partitions,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A super region holding the given (name, [(sectors, target_sector)]) partitions.
    pub(crate) fn build(parts: &[(&str, &[(u64, u64)])], size: usize) -> Vec<u8> {
        let mut image = vec![0u8; size];
        let mut partitions = Vec::new();
        let mut extents = Vec::new();
        let mut index = 0u32;
        for (name, list) in parts {
            let mut e = [0u8; 52];
            e[..name.len()].copy_from_slice(name.as_bytes());
            e[36..40].copy_from_slice(&1u32.to_le_bytes());
            e[40..44].copy_from_slice(&index.to_le_bytes());
            e[44..48].copy_from_slice(&(list.len() as u32).to_le_bytes());
            partitions.extend_from_slice(&e);
            for (sectors, target) in *list {
                let mut x = [0u8; 24];
                x[..8].copy_from_slice(&sectors.to_le_bytes());
                x[12..20].copy_from_slice(&target.to_le_bytes());
                extents.extend_from_slice(&x);
                index += 1;
            }
        }
        let device = [0u8; 64];
        let mut tables = partitions.clone();
        tables.extend_from_slice(&extents);
        tables.extend_from_slice(&device);
        let mut header = vec![0u8; 128];
        header[..4].copy_from_slice(&HEADER_MAGIC.to_le_bytes());
        header[4..6].copy_from_slice(&10u16.to_le_bytes());
        header[8..12].copy_from_slice(&128u32.to_le_bytes());
        header[44..48].copy_from_slice(&(tables.len() as u32).to_le_bytes());
        header[48..80].copy_from_slice(&sha256(&tables));
        let descriptors = [
            (80, 0, parts.len(), 52),
            (92, partitions.len(), extents.len() / 24, 24),
            (104, partitions.len() + extents.len(), 0, 48),
            (116, partitions.len() + extents.len(), 1, 64),
        ];
        for (at, offset, count, entry) in descriptors {
            header[at..at + 4].copy_from_slice(&(offset as u32).to_le_bytes());
            header[at + 4..at + 8].copy_from_slice(&(count as u32).to_le_bytes());
            header[at + 8..at + 12].copy_from_slice(&(entry as u32).to_le_bytes());
        }
        let digest = sha256(&header);
        header[12..44].copy_from_slice(&digest);
        let mut geometry = vec![0u8; 52];
        geometry[..4].copy_from_slice(&GEOMETRY_MAGIC.to_le_bytes());
        geometry[4..8].copy_from_slice(&52u32.to_le_bytes());
        geometry[40..44].copy_from_slice(&65536u32.to_le_bytes());
        geometry[44..48].copy_from_slice(&1u32.to_le_bytes());
        geometry[48..52].copy_from_slice(&4096u32.to_le_bytes());
        let digest = sha256(&geometry);
        geometry[8..40].copy_from_slice(&digest);
        image[4096..4148].copy_from_slice(&geometry);
        image[12288..12416].copy_from_slice(&header);
        image[12416..12416 + tables.len()].copy_from_slice(&tables);
        image
    }

    #[test]
    fn reads_partitions_and_verifies_checksums() {
        let mut image = build(
            &[("system", &[(8, 64), (8, 80)]), ("vendor", &[(8, 96)])],
            64 * 1024,
        );
        let metadata = read(&image).unwrap();
        assert_eq!(metadata.version, (10, 0));
        let system = &metadata.partitions[0];
        assert_eq!((system.name.as_str(), system.size), ("system", 8192));
        assert_eq!(system.extents[1].physical, Some(80 * 512));
        assert_eq!(system.extents[1].logical, 4096);
        image[12416] ^= 1;
        assert!(read(&image).is_err());
    }
}
