//! Device lifetime records, kept beside the device directory so short-lived
//! devices survive notification coalescing. The syscall layer consumes these
//! once per kernel namespace. Writers hold the server's directory lock.

use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use super::Descriptor;
use crate::wire;

fn path(dir: &Path) -> PathBuf {
    let mut p = dir.as_os_str().to_owned();
    p.push(".hotplug-v1");
    p.into()
}

pub(crate) fn append(dir: &Path, added: bool, desc: &Descriptor) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(dir))?;
    let mut record = vec![u8::from(added)];
    record.extend_from_slice(wire::bytes(desc));
    f.write_all(&record)
}

/// Complete records after byte offset `at`, and the next offset. A writer
/// may still be finishing the last record; leave it for the next read.
pub fn read(dir: &Path, at: u64) -> io::Result<(u64, Vec<(bool, Descriptor)>)> {
    let mut f = match std::fs::File::open(path(dir)) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((at, Vec::new())),
        Err(e) => return Err(e),
    };
    f.seek(io::SeekFrom::Start(at))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    let size = 1 + size_of::<Descriptor>();
    let mut records = Vec::new();
    for b in bytes.chunks_exact(size) {
        if b[0] > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "input hotplug action",
            ));
        }
        // SAFETY: Descriptor is the wire's plain integer/array structure,
        // and this complete record contains exactly its bytes.
        let desc = unsafe { std::ptr::read_unaligned(b[1..].as_ptr().cast::<Descriptor>()) };
        records.push((b[0] == 1, desc));
    }
    Ok((at + (records.len() * size) as u64, records))
}
