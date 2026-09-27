//! Primary GUID partition table (512-byte sectors), with header/table CRCs.
use crate::source::{ReadAt, check_range};
use crate::{Result, invalid, le32, le64, mul};

pub const SECTOR: u64 = 512;
const SIGNATURE: &[u8; 8] = b"EFI PART";
const MAX_TABLE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

pub struct Table {
    pub version: (u16, u16),
    pub partitions: Vec<Partition>,
}

impl Table {
    pub fn get(&self, name: &str) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.name == name)
    }
}

pub fn is_gpt(disk: &dyn ReadAt) -> bool {
    disk.read_vec(SECTOR, 8).is_ok_and(|b| b == SIGNATURE)
}

pub fn read(disk: &dyn ReadAt) -> Result<Table> {
    let h = disk.read_vec(SECTOR, 512)?;
    if h[..8] != *SIGNATURE {
        return Err(invalid("missing primary GPT"));
    }
    let revision = le32(&h, 8)?;
    let header_size = le32(&h, 12)? as usize;
    if !(92..=512).contains(&header_size) || le64(&h, 24)? != 1 {
        return Err(invalid("invalid GPT header size or LBA"));
    }
    let mut checked = h[..header_size].to_vec();
    checked[16..20].fill(0);
    if crc32fast::hash(&checked) != le32(&h, 16)? {
        return Err(invalid("GPT header CRC mismatch"));
    }
    let table_lba = le64(&h, 72)?;
    let count = le32(&h, 80)? as usize;
    let entry_size = le32(&h, 84)? as usize;
    if count == 0 || count > 4096 || !(128..=4096).contains(&entry_size) {
        return Err(invalid("invalid GPT entry geometry"));
    }
    let table_len = count * entry_size;
    if table_len > MAX_TABLE_BYTES {
        return Err(invalid("GPT table too large"));
    }
    let table = disk.read_vec(mul(table_lba, SECTOR, "locating GPT table")?, table_len)?;
    if crc32fast::hash(&table) != le32(&h, 88)? {
        return Err(invalid("GPT table CRC mismatch"));
    }
    let mut partitions: Vec<Partition> = Vec::new();
    for entry in table.chunks_exact(entry_size) {
        if entry[..16].iter().all(|b| *b == 0) {
            continue;
        }
        let units: Vec<u16> = entry[56..128]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        let name = String::from_utf16(&units).map_err(|_| invalid("invalid GPT name"))?;
        let first = le64(entry, 32)?;
        let last = le64(entry, 40)?;
        if last < first {
            return Err(invalid(format!("reversed GPT partition {name}")));
        }
        if partitions.iter().any(|p| p.name == name) {
            return Err(invalid(format!("duplicate GPT partition {name}")));
        }
        let partition = Partition {
            name,
            offset: mul(first, SECTOR, "locating GPT partition")?,
            size: mul(last - first + 1, SECTOR, "sizing GPT partition")?,
        };
        check_range(disk.size(), partition.offset, partition.size)?;
        partitions.push(partition);
    }
    Ok(Table {
        version: ((revision >> 16) as u16, revision as u16),
        partitions,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A disk image with a primary GPT at LBA 1 and its table at LBA 2.
    pub(crate) fn build(parts: &[(&str, u64, u64)], disk_sectors: u64) -> Vec<u8> {
        let mut disk = vec![0u8; (disk_sectors * SECTOR) as usize];
        let mut table = vec![0u8; 128 * 4];
        for (i, (name, first, last)) in parts.iter().enumerate() {
            let e = &mut table[i * 128..(i + 1) * 128];
            e[0] = 1; // non-zero type GUID
            e[32..40].copy_from_slice(&first.to_le_bytes());
            e[40..48].copy_from_slice(&last.to_le_bytes());
            for (j, unit) in name.encode_utf16().enumerate() {
                e[56 + j * 2..58 + j * 2].copy_from_slice(&unit.to_le_bytes());
            }
        }
        let mut h = vec![0u8; 92];
        h[..8].copy_from_slice(SIGNATURE);
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&1u64.to_le_bytes());
        h[72..80].copy_from_slice(&2u64.to_le_bytes());
        h[80..84].copy_from_slice(&4u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&crc32fast::hash(&table).to_le_bytes());
        let crc = crc32fast::hash(&h);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        disk[512..604].copy_from_slice(&h);
        disk[1024..1024 + table.len()].copy_from_slice(&table);
        disk
    }

    #[test]
    fn reads_named_partitions_and_checks_crcs() {
        let mut disk = build(&[("super", 8, 15), ("vbmeta", 16, 16)], 20);
        assert!(is_gpt(&disk));
        let table = read(&disk).unwrap();
        assert_eq!(table.version, (1, 0));
        assert_eq!(
            table.get("super").unwrap(),
            &Partition {
                name: "super".into(),
                offset: 4096,
                size: 4096
            }
        );
        disk[1024 + 40] ^= 1;
        assert!(read(&disk).is_err());
        let outside = build(&[("big", 8, 99)], 20);
        assert!(read(&outside).is_err());
    }
}
