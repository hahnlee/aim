//! Minimal ZIP reader (stored and deflate, ZIP64 sizes) over a `ReadAt`.
//! Used for the SDK system-image archive, APEX and CAPEX containers.
use crate::source::{ReadAt, Reader, Slice, check_range};
use crate::{Result, invalid, le16, le32, le64};
use std::io::{self, Read, Write};

const EOCD: &[u8; 4] = b"PK\x05\x06";
const EOCD64_LOCATOR: &[u8; 4] = b"PK\x06\x07";
const EOCD64: &[u8; 4] = b"PK\x06\x06";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";
const LOCAL: &[u8; 4] = b"PK\x03\x04";
const MAX_ENTRIES: u64 = 1 << 20;
const MAX_CENTRAL_BYTES: u64 = 256 * 1024 * 1024;

pub const STORED: u16 = 0;
pub const DEFLATE: u16 = 8;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: Vec<u8>,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub size: u64,
    local_offset: u64,
}

pub struct Archive<'a> {
    source: &'a dyn ReadAt,
    pub entries: Vec<Entry>,
}

impl<'a> Archive<'a> {
    pub fn open(source: &'a dyn ReadAt) -> Result<Self> {
        let size = source.size();
        if size < 22 {
            return Err(invalid("ZIP is shorter than its end record"));
        }
        let tail_len = size.min(65_557);
        let tail = source.read_vec(size - tail_len, tail_len as usize)?;
        let eocd = (0..=tail.len() - 22)
            .rev()
            .find(|&at| {
                tail[at..at + 4] == *EOCD
                    && le16(&tail, at + 20)
                        .is_ok_and(|comment| at + 22 + usize::from(comment) == tail.len())
            })
            .ok_or_else(|| invalid("ZIP end-of-central-directory record not found"))?;
        if le16(&tail, eocd + 4)? != 0 || le16(&tail, eocd + 6)? != 0 {
            return Err(invalid("multi-disk ZIP is unsupported"));
        }
        let mut count = u64::from(le16(&tail, eocd + 10)?);
        let mut central_size = u64::from(le32(&tail, eocd + 12)?);
        let mut central_offset = u64::from(le32(&tail, eocd + 16)?);
        if count == 0xffff || central_size == 0xffff_ffff || central_offset == 0xffff_ffff {
            let locator_at = eocd
                .checked_sub(20)
                .ok_or_else(|| invalid("ZIP64 locator missing"))?;
            if tail[locator_at..locator_at + 4] != *EOCD64_LOCATOR {
                return Err(invalid("ZIP64 locator missing"));
            }
            let record = source.read_vec(le64(&tail, locator_at + 8)?, 56)?;
            if record[..4] != *EOCD64 {
                return Err(invalid("invalid ZIP64 end record"));
            }
            count = le64(&record, 32)?;
            central_size = le64(&record, 40)?;
            central_offset = le64(&record, 48)?;
        }
        if count > MAX_ENTRIES || central_size > MAX_CENTRAL_BYTES {
            return Err(invalid("ZIP central directory is too large"));
        }
        check_range(size, central_offset, central_size)?;
        let central = source.read_vec(central_offset, central_size as usize)?;
        let mut entries = Vec::with_capacity(count as usize);
        let mut at = 0usize;
        for _ in 0..count {
            let header = central
                .get(at..at + 46)
                .ok_or_else(|| invalid("truncated ZIP central directory"))?;
            if header[..4] != *CENTRAL {
                return Err(invalid("invalid ZIP central-directory entry"));
            }
            let name_len = usize::from(le16(header, 28)?);
            let extra_len = usize::from(le16(header, 30)?);
            let comment_len = usize::from(le16(header, 32)?);
            let name_at = at + 46;
            let extra_at = name_at + name_len;
            let next = extra_at + extra_len + comment_len;
            if next > central.len() {
                return Err(invalid(
                    "ZIP central-directory entry overruns the directory",
                ));
            }
            let mut size = u64::from(le32(header, 24)?);
            let mut compressed_size = u64::from(le32(header, 20)?);
            let mut local_offset = u64::from(le32(header, 42)?);
            // ZIP64 extended information: present fields follow in fixed order.
            let extra = &central[extra_at..extra_at + extra_len];
            let mut cursor = 0usize;
            while cursor + 4 <= extra.len() {
                let id = le16(extra, cursor)?;
                let len = usize::from(le16(extra, cursor + 2)?);
                let body = extra
                    .get(cursor + 4..cursor + 4 + len)
                    .ok_or_else(|| invalid("truncated ZIP extra field"))?;
                if id == 1 {
                    let mut field = 0usize;
                    for value in [&mut size, &mut compressed_size, &mut local_offset] {
                        if *value == 0xffff_ffff {
                            *value = le64(body, field)?;
                            field += 8;
                        }
                    }
                }
                cursor += 4 + len;
            }
            entries.push(Entry {
                name: central[name_at..extra_at].to_vec(),
                method: le16(header, 10)?,
                flags: le16(header, 8)?,
                crc32: le32(header, 16)?,
                compressed_size,
                size,
                local_offset,
            });
            at = next;
        }
        Ok(Self { source, entries })
    }

    pub fn find(&self, name: &[u8]) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// Absolute offset of the entry's (possibly compressed) bytes.
    pub fn data_offset(&self, entry: &Entry) -> Result<u64> {
        if entry.flags & 1 != 0 {
            return Err(invalid("encrypted ZIP entries are unsupported"));
        }
        let local = self.source.read_vec(entry.local_offset, 30)?;
        if local[..4] != *LOCAL {
            return Err(invalid("invalid ZIP local header"));
        }
        if le16(&local, 8)? != entry.method {
            return Err(invalid("ZIP local and central compression methods differ"));
        }
        let name_len = u64::from(le16(&local, 26)?);
        let extra_len = u64::from(le16(&local, 28)?);
        let name = self
            .source
            .read_vec(entry.local_offset + 30, name_len as usize)?;
        if name != entry.name {
            return Err(invalid("ZIP local and central names differ"));
        }
        let offset = entry.local_offset + 30 + name_len + extra_len;
        check_range(self.source.size(), offset, entry.compressed_size)?;
        Ok(offset)
    }

    /// Random access to a stored (uncompressed) entry without copying it.
    pub fn stored(&self, entry: &Entry) -> Result<Slice<'a>> {
        if entry.method != STORED || entry.size != entry.compressed_size {
            return Err(invalid(format!(
                "{} is not stored uncompressed",
                String::from_utf8_lossy(&entry.name)
            )));
        }
        Slice::new(self.source, self.data_offset(entry)?, entry.size)
    }

    /// Stream the decoded entry into `out`, verifying size and CRC-32.
    pub fn copy_to(&self, entry: &Entry, out: &mut dyn Write) -> Result<u64> {
        let raw = Reader::new(self.source, self.data_offset(entry)?, entry.compressed_size)?;
        let mut decoder: Box<dyn Read> = match entry.method {
            STORED => Box::new(raw),
            DEFLATE => Box::new(flate2::read::DeflateDecoder::new(
                io::BufReader::with_capacity(1 << 20, raw),
            )),
            method => return Err(invalid(format!("unsupported ZIP method {method}"))),
        };
        let mut hasher = crc32fast::Hasher::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            let read = decoder.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
            total += read as u64;
            if total > entry.size {
                return Err(invalid("ZIP entry decodes past its declared size"));
            }
        }
        if total != entry.size || hasher.finalize() != entry.crc32 {
            return Err(invalid(format!(
                "ZIP entry {} failed its size/CRC-32 check",
                String::from_utf8_lossy(&entry.name)
            )));
        }
        Ok(total)
    }

    pub fn read(&self, entry: &Entry, limit: u64) -> Result<Vec<u8>> {
        if entry.size > limit {
            return Err(invalid("ZIP entry exceeds the in-memory limit"));
        }
        let mut out = Vec::with_capacity(entry.size as usize);
        self.copy_to(entry, &mut out)?;
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a single-disk ZIP from (name, method, payload) triples.
    pub(crate) fn build(entries: &[(&str, u16, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, method, data) in entries {
            let crc = crc32fast::hash(data);
            let body = if *method == DEFLATE {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                encoder.write_all(data).unwrap();
                encoder.finish().unwrap()
            } else {
                data.to_vec()
            };
            let offset = out.len() as u32;
            let mut header = Vec::new();
            header.extend_from_slice(&0u16.to_le_bytes()); // flags
            header.extend_from_slice(&method.to_le_bytes());
            header.extend_from_slice(&[0; 4]); // time, date
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(body.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes()); // extra
            out.extend_from_slice(LOCAL);
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&header);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);
            central.extend_from_slice(CENTRAL);
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&header);
            central.extend_from_slice(&[0; 6]); // comment, disk, internal attrs
            central.extend_from_slice(&[0; 4]); // external attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(EOCD);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn reads_stored_and_deflated_entries() {
        let text = b"hello hello hello hello".repeat(10);
        let zip = build(&[("a.img", STORED, b"payload"), ("b/c.txt", DEFLATE, &text)]);
        let archive = Archive::open(&zip).unwrap();
        let stored = archive.stored(archive.find(b"a.img").unwrap()).unwrap();
        assert_eq!(stored.read_vec(0, 7).unwrap(), b"payload");
        let deflated = archive.find(b"b/c.txt").unwrap();
        assert!(archive.stored(deflated).is_err());
        assert_eq!(archive.read(deflated, 1 << 20).unwrap(), text);
    }

    #[test]
    fn rejects_corruption() {
        let mut zip = build(&[("a", DEFLATE, b"abcdefabcdef")]);
        assert!(Archive::open(&zip[..zip.len() - 1].to_vec()).is_err());
        // Flip the recorded CRC in the central directory.
        let central = zip.windows(4).position(|w| w == CENTRAL).unwrap();
        zip[central + 16] ^= 1;
        let archive = Archive::open(&zip).unwrap();
        assert!(archive.read(&archive.entries[0], 1 << 20).is_err());
    }
}
