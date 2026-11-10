//! The property areas as files: bionic's `/dev/__properties__` backed by
//! `MAP_SHARED` mappings the guest maps too, the cross-process wake, and
//! `/data/property/persistent_properties`.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Instant;

use aim_android_init::props::area::{AreaMemory, HeapMemory};
use aim_android_init::props::areas::{PROPERTIES_SERIAL, PROPERTY_INFO, read_from_dir};
use aim_android_init::props::{
    FutexWaker, PA_SIZE, PropertyAreas, PropertyInfoArea, PropertyService, SharedAreaReader,
    WakeTarget,
};
use aim_binder_host::mach;
use aim_binder_host::server::Server;
use aim_binder_host::wire::SharedFile;

use crate::futex::{SharedFutex, UlockShared, wait_shared};

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

// SAFETY: the mapping belongs to the value alone; nothing else unmaps it.
unsafe impl Send for MmapMemory {}

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
    let mut names: Vec<String> = PropertyInfoArea::new(&property_info)?
        .contexts()
        .into_iter()
        .map(str::to_string)
        .collect();
    names.push(PROPERTIES_SERIAL.to_string());
    let mut created = create_files(dir, &names);
    let mut bases = HashMap::new();
    let mut failure = None;
    let areas = PropertyAreas::with_memory(property_info, |name| {
        match created.remove(name).expect("created for every context") {
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

/// The in-process services' reader of the areas in `dir`, as bionic's
/// readers are: `property_info`, which init writes once before they start,
/// read at the first lookup, and each context's area mapped read-only and
/// shared at its first lookup and kept, then read as the writer updates it
/// ([`SharedAreaReader`]). What is missing or does not parse is not kept,
/// and its lookup fails as [`read_from_dir`]'s does.
pub struct AreaReader {
    dir: std::path::PathBuf,
    info: std::sync::Mutex<Option<std::sync::Arc<Vec<u8>>>>,
    maps: std::sync::Mutex<HashMap<String, std::sync::Arc<ReadMapping>>>,
}

struct ReadMapping {
    base: *const u8,
    len: usize,
}

// SAFETY: a read-only mapping owned by the value, read only through
// SharedAreaReader's atomic loads.
unsafe impl Send for ReadMapping {}
unsafe impl Sync for ReadMapping {}

impl ReadMapping {
    fn open(path: &Path) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "empty"));
        }
        // SAFETY: maps the open file read-only; the fd may close after.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            base: base.cast(),
            len,
        })
    }
}

impl Drop for ReadMapping {
    fn drop(&mut self) {
        // SAFETY: mapped in open with this length.
        unsafe { libc::munmap(self.base.cast_mut().cast(), self.len) };
    }
}

impl AreaReader {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            info: Default::default(),
            maps: Default::default(),
        }
    }

    fn property_info(&self) -> io::Result<std::sync::Arc<Vec<u8>>> {
        let mut info = self.info.lock().unwrap();
        if let Some(info) = info.as_ref() {
            return Ok(info.clone());
        }
        let read = std::sync::Arc::new(fs::read(self.dir.join(PROPERTY_INFO))?);
        *info = Some(read.clone());
        Ok(read)
    }

    fn mapping(&self, file: &str) -> io::Result<std::sync::Arc<ReadMapping>> {
        let mut maps = self.maps.lock().unwrap();
        if let Some(mapping) = maps.get(file) {
            return Ok(mapping.clone());
        }
        let mapping = std::sync::Arc::new(ReadMapping::open(&self.dir.join(file))?);
        maps.insert(file.to_string(), mapping.clone());
        Ok(mapping)
    }

    /// [`read_from_dir`] of `name`.
    pub fn get(&self, name: &str) -> io::Result<Option<String>> {
        let invalid = |file: &str, error: String| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{file}: {error}"))
        };
        let bytes = self.property_info()?;
        let info = PropertyInfoArea::new(&bytes).map_err(|e| {
            *self.info.lock().unwrap() = None;
            invalid(PROPERTY_INFO, e)
        })?;
        let (index, _) = info.property_info_indexes(name);
        let context = *info
            .contexts()
            .get(index as usize)
            .ok_or_else(|| invalid(PROPERTY_INFO, format!("no context for {name}")))?;
        let area = self.mapping(context)?;
        // SAFETY: the mapping is page aligned and lives as long as `area`;
        // init writes the file only through its PropArea.
        let reader = unsafe { SharedAreaReader::new(area.base, area.len) }.map_err(|e| {
            self.maps.lock().unwrap().remove(context);
            invalid(context, e)
        })?;
        Ok(reader.get(name).map(|p| p.value))
    }
}

/// Waits for a property's value through bionic's global area serial
/// (`SystemProperties::WaitAny`), including properties not yet added.
pub fn wait_property(dir: &Path, name: &str, value: &str, deadline: Instant) -> io::Result<bool> {
    let file = fs::File::open(dir.join(PROPERTIES_SERIAL))?;
    if file.metadata()?.len() < 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short property serial area",
        ));
    }
    // SAFETY: the file contains the area header, mapped read-only and shared.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            8,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    struct Mapping(*mut libc::c_void);
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: this mapping owns the range mapped above.
            unsafe { libc::munmap(self.0, 8) };
        }
    }
    let mapping = Mapping(base);
    // SAFETY: the aligned serial word is at offset 4 in the mapped header.
    let serial = unsafe { &*mapping.0.cast::<u8>().add(4).cast::<AtomicU32>() };
    loop {
        // Snapshot before reading the condition so a concurrent write cannot
        // be lost between the read and the shared wait's compare.
        let expected = serial.load(Ordering::Acquire);
        if read_from_dir(dir, name)?.as_deref() == Some(value) {
            return Ok(true);
        }
        let Some(remaining) = deadline
            .checked_duration_since(Instant::now())
            .filter(|t| !t.is_zero())
        else {
            return Ok(false);
        };
        wait_shared(
            serial,
            expected,
            remaining.as_nanos().min(u64::MAX as u128) as u64,
        );
    }
}

/// Creates and maps the area files `names` in `dir` on several threads: a
/// writable shared mapping of a file costs about a millisecond each while
/// the host's endpoint security agent looks at it (#337), and those waits
/// overlap.
fn create_files(dir: &Path, names: &[String]) -> HashMap<String, io::Result<MmapMemory>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let chunk = names.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = names
            .chunks(chunk)
            .map(|names| {
                scope.spawn(move || {
                    names
                        .iter()
                        .map(|name| {
                            let memory = MmapMemory::create_file(&dir.join(name), PA_SIZE);
                            (name.clone(), memory)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("area file thread"))
            .collect()
    })
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
    fn property_wait_follows_shared_service_wakes_and_deadline() {
        use aim_android_init::props::build_trie;
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("aim-property-wait-{}", std::process::id()));
        let info = build_trie(&[], "u:object_r:default_prop:s0", "string").unwrap();
        let mut service = mapped_properties(&dir, info).unwrap();
        for value in ["added", "updated"] {
            std::thread::scope(|scope| {
                let (sent, received) = mpsc::channel();
                let dir = &dir;
                scope.spawn(move || {
                    sent.send(wait_property(
                        dir,
                        "debug.ready",
                        value,
                        Instant::now() + Duration::from_secs(5),
                    ))
                    .unwrap();
                });
                assert!(matches!(
                    received.recv_timeout(Duration::from_millis(50)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ));
                assert!(service.init_set("debug.unrelated", value).is_success());
                assert!(service.init_set("debug.ready", "wrong").is_success());
                assert!(matches!(
                    received.recv_timeout(Duration::from_millis(50)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ));
                assert!(service.init_set("debug.ready", value).is_success());
                assert!(
                    received
                        .recv_timeout(Duration::from_secs(1))
                        .unwrap()
                        .unwrap()
                );
            });
            assert!(wait_property(&dir, "debug.ready", value, Instant::now()).unwrap());
        }
        let started = Instant::now();
        assert!(
            !wait_property(
                &dir,
                "debug.missing",
                "true",
                started + Duration::from_millis(30)
            )
            .unwrap()
        );
        assert!(started.elapsed() >= Duration::from_millis(30));
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn area_reader_follows_the_shared_areas_and_retries_what_is_missing() {
        use aim_android_init::props::build_trie;

        let dir = std::env::temp_dir().join(format!("aim-area-reader-{}", std::process::id()));
        let reader = AreaReader::new(&dir);
        assert!(reader.get("debug.value").is_err());
        let info = build_trie(&[], "u:object_r:default_prop:s0", "string").unwrap();
        let mut service = mapped_properties(&dir, info).unwrap();
        assert_eq!(reader.get("debug.value").unwrap(), None);
        for value in ["added", "updated", "a longer value than the first two"] {
            assert!(service.init_set("debug.value", value).is_success());
            assert_eq!(reader.get("debug.value").unwrap().as_deref(), Some(value));
            assert_eq!(
                reader.get("debug.value").unwrap(),
                read_from_dir(&dir, "debug.value").unwrap()
            );
        }
        drop(service);
        fs::remove_dir_all(dir).unwrap();
    }

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
