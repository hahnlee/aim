//! The property areas as files: bionic's `/dev/__properties__` backed by
//! `MAP_SHARED` mappings the guest maps too, the cross-process wake, and
//! `/data/property/persistent_properties`.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::AtomicU32;

use aim_android_init::props::area::{AreaMemory, HeapMemory};
use aim_android_init::props::areas::{PROPERTIES_SERIAL, PROPERTY_INFO};
use aim_android_init::props::{FutexWaker, PA_SIZE, PropertyAreas, PropertyService, WakeTarget};
use aim_binder_host::mach;
use aim_binder_host::server::Server;
use aim_binder_host::wire::SharedFile;

use crate::futex::{SharedFutex, UlockShared};

/// A `MAP_SHARED`, read-write mapping of one area file.
pub struct MmapMemory {
    base: *mut u8,
    len: usize,
    /// The file's device and inode.
    dev: u64,
    ino: u64,
}

impl MmapMemory {
    /// Creates `path` (mode 0444, as bionic's `map_prop_area_rw` leaves
    /// it), sizes it to `len` and maps it shared.
    pub fn create_file(path: &Path, len: usize) -> io::Result<Self> {
        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: plain libc calls on a path we own; the fd is closed below.
        unsafe {
            let fd = libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o444 as libc::c_uint,
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let result = (|| {
                let mut st: libc::stat = std::mem::zeroed();
                if libc::ftruncate(fd, len as libc::off_t) != 0 || libc::fstat(fd, &mut st) != 0 {
                    return Err(io::Error::last_os_error());
                }
                let base = libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                );
                if base == libc::MAP_FAILED {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self {
                    base: base.cast(),
                    len,
                    dev: st.st_dev as u32 as u64,
                    ino: st.st_ino,
                })
            })();
            libc::close(fd);
            result
        }
    }
}

impl Drop for MmapMemory {
    fn drop(&mut self) {
        // SAFETY: mapped in create_file with this length.
        unsafe { libc::munmap(self.base.cast(), self.len) };
    }
}

// SAFETY: the mapping is valid, page aligned and writable for its lifetime.
unsafe impl AreaMemory for MmapMemory {
    fn base(&self) -> *mut u8 {
        self.base
    }
    fn len(&self) -> usize {
        self.len
    }
}

/// Heap areas for a dry run, file-backed shared areas for a real boot.
pub enum AreaBacking {
    Heap(HeapMemory),
    Mapped(MmapMemory),
}

// SAFETY: delegates to the two implementations.
unsafe impl AreaMemory for AreaBacking {
    fn base(&self) -> *mut u8 {
        match self {
            AreaBacking::Heap(memory) => memory.base(),
            AreaBacking::Mapped(memory) => memory.base(),
        }
    }
    fn len(&self) -> usize {
        match self {
            AreaBacking::Heap(memory) => memory.len(),
            AreaBacking::Mapped(memory) => memory.len(),
        }
    }
}

pub type Properties = PropertyService<AreaBacking>;

/// Wakes readers of the mapped areas: the area name and offset bionic
/// reports become an address in aimd's own shared mapping of the
/// same file, woken with the SHARED flavour.
pub struct SharedAreaWaker<F: SharedFutex = UlockShared> {
    bases: HashMap<String, usize>,
    futex: F,
}

impl<F: SharedFutex> SharedAreaWaker<F> {
    pub fn new(bases: HashMap<String, usize>, futex: F) -> Self {
        Self { bases, futex }
    }
}

impl<F: SharedFutex> FutexWaker for SharedAreaWaker<F> {
    fn wake_all(&self, target: WakeTarget<'_>) {
        let Some(base) = self.bases.get(target.area) else {
            return;
        };
        // SAFETY: the offset is a 4-byte aligned serial word inside the
        // area mapping, which lives as long as the property service.
        let word = unsafe { &*((base + target.offset) as *const AtomicU32) };
        self.futex.wake_all(word);
    }
}

/// Property areas on the heap (dry run): nothing can wait on them.
pub fn heap_properties(property_info: Vec<u8>) -> Result<Properties, String> {
    let areas = PropertyAreas::with_memory(property_info, |_| {
        AreaBacking::Heap(HeapMemory::new(PA_SIZE))
    })?;
    Ok(PropertyService::with_areas(areas))
}

/// `__system_property_area_init` into `dir` (the host directory the guest
/// sees as `/dev/__properties__`): `property_info`, one shared-mapped file
/// per context and `properties_serial`, with a [`SharedAreaWaker`].
pub fn mapped_properties(dir: &Path, property_info: Vec<u8>) -> Result<Properties, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let info_path = dir.join(PROPERTY_INFO);
    fs::write(&info_path, &property_info).map_err(|e| format!("{}: {e}", info_path.display()))?;
    set_mode(&info_path, 0o444);
    let mut bases = HashMap::new();
    let mut failure = None;
    let areas = PropertyAreas::with_memory(property_info, |name| {
        match MmapMemory::create_file(&dir.join(name), PA_SIZE) {
            Ok(memory) => {
                bases.insert(name.to_string(), memory.base() as usize);
                AreaBacking::Mapped(memory)
            }
            Err(error) => {
                failure.get_or_insert(format!("{}: {error}", dir.join(name).display()));
                AreaBacking::Heap(HeapMemory::new(PA_SIZE))
            }
        }
    })?;
    if let Some(error) = failure {
        return Err(error);
    }
    debug_assert!(bases.contains_key(PROPERTIES_SERIAL));
    let mut service = PropertyService::with_areas(areas);
    service.set_waker(Box::new(SharedAreaWaker::new(bases, UlockShared)));
    Ok(service)
}

/// Share the mapped areas' pages with the guests through the binder host
/// (the kernel-state server), as tmpfs pages are shared: a guest's
/// `MAP_SHARED` mapping of an area maps them without a host file mapping,
/// which costs 1-3 ms in a guest process while this process holds the file
/// mapped writable (the host's endpoint security agent, #337).
pub fn share_areas(properties: &Properties, server: &Server) {
    let areas = properties.areas();
    let all = areas.contexts().iter().filter_map(|c| areas.area(c));
    for area in all.chain([areas.serial_area()]) {
        if let AreaBacking::Mapped(m) = area.memory()
            && let Ok(entry) = mach::share_read_only(m.base as u64, m.len as u64)
        {
            server.share_file(SharedFile {
                dev: m.dev,
                ino: m.ino,
                size: m.len as u64,
                entry,
            });
        }
    }
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

/// `/data/property/persistent_properties`: `PersistentProperties { repeated
/// PersistentPropertyRecord properties = 1; }`, each `{ string name = 1;
/// string value = 2; }` (system/core/init/persistent_properties.proto).
pub fn decode_persistent_properties(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut reader = ProtoReader { bytes, at: 0 };
    while let Some((field, wire)) = reader.key()? {
        if field == 1 && wire == 2 {
            let record = reader.bytes()?;
            let mut inner = ProtoReader {
                bytes: record,
                at: 0,
            };
            let (mut name, mut value) = (String::new(), String::new());
            while let Some((field, wire)) = inner.key()? {
                match (field, wire) {
                    (1, 2) => name = String::from_utf8_lossy(inner.bytes()?).into_owned(),
                    (2, 2) => value = String::from_utf8_lossy(inner.bytes()?).into_owned(),
                    _ => inner.skip(wire)?,
                }
            }
            out.push((name, value));
        } else {
            reader.skip(wire)?;
        }
    }
    Ok(out)
}

pub fn encode_persistent_properties(properties: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in properties {
        let mut record = Vec::new();
        put_bytes(&mut record, 1, name.as_bytes());
        put_bytes(&mut record, 2, value.as_bytes());
        put_bytes(&mut out, 1, &record);
    }
    out
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// A minimal protobuf wire reader (varint, length-delimited, fixed).
pub(crate) struct ProtoReader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

impl<'a> ProtoReader<'a> {
    pub fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.bytes.get(self.at).ok_or("truncated varint")?;
            self.at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("varint too long".to_string())
    }

    pub fn key(&mut self) -> Result<Option<(u64, u8)>, String> {
        if self.at >= self.bytes.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        Ok(Some((key >> 3, (key & 7) as u8)))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = self.varint()? as usize;
        let end = self.at.checked_add(len).ok_or("length overflow")?;
        let slice = self.bytes.get(self.at..end).ok_or("truncated field")?;
        self.at = end;
        Ok(slice)
    }

    pub fn skip(&mut self, wire: u8) -> Result<(), String> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.at += 8,
            2 => {
                self.bytes()?;
            }
            5 => self.at += 4,
            other => return Err(format!("unsupported wire type {other}")),
        }
        if self.at > self.bytes.len() {
            return Err("truncated field".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_properties_round_trip() {
        let props = vec![
            ("persist.sys.usb.config".to_string(), "adb".to_string()),
            ("persist.x".to_string(), "".to_string()),
            ("persist.long".to_string(), "v".repeat(300)),
        ];
        let bytes = encode_persistent_properties(&props);
        assert_eq!(decode_persistent_properties(&bytes).unwrap(), props);
        assert!(decode_persistent_properties(&bytes[..bytes.len() - 1]).is_err());
    }
}
