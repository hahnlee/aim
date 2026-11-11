//! The file of one eBPF object of a boot (`<runtime>/bpf-objects`), which
//! the Linux layer's `bpf(2)` creates and pins and which the host's services
//! read: its data (at offset 0, so a ring buffer's mmap sees the kernel
//! layout) followed by a [`Header`]. A map's data is its elements: an
//! array's values in key order, or a hash's open-addressed slots (an
//! 8-byte state, the key and the value, each padded to 8 bytes). Processes
//! map it shared and serialize updates with the spin lock in the header.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const HASH: u32 = 1;
pub const ARRAY: u32 = 2;
pub const PROG_ARRAY: u32 = 3;
pub const PERCPU_HASH: u32 = 5;
pub const PERCPU_ARRAY: u32 = 6;
pub const LRU_HASH: u32 = 9;
pub const LRU_PERCPU_HASH: u32 = 10;
pub const LPM_TRIE: u32 = 11;
pub const ARRAY_OF_MAPS: u32 = 12;
pub const HASH_OF_MAPS: u32 = 13;
pub const DEVMAP: u32 = 14;
pub const DEVMAP_HASH: u32 = 25;
pub const RINGBUF: u32 = 27;

pub const MAGIC: u64 = u64::from_le_bytes(*b"LXBPFOBJ");
pub const KIND_MAP: u32 = 1;
pub const KIND_PROG: u32 = 2;
pub const KIND_BTF: u32 = 3;
pub const HEADER: usize = 128;
pub const SLOT_EMPTY: u32 = 0;
pub const SLOT_USED: u32 = 1;
pub const SLOT_DELETED: u32 = 2;
/// The guest's page: a ring buffer's positions and data are whole ones.
pub const PAGE: usize = 16384;

/// The header at the end of an object file.
#[repr(C)]
pub struct Header {
    pub magic: u64,
    pub kind: u32,
    /// Map type, or program type.
    pub ty: u32,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    pub flags: u32,
    pub id: u32,
    pub count: AtomicU32,
    pub frozen: u32,
    pub name: [u8; 16],
    /// Bytes of data before the header.
    pub data_len: u64,
    /// The spin lock: its holder ([`Holder`]), or 0.
    pub lock: AtomicU64,
    pub _pad: [u8; 48],
}
const _: () = assert!(std::mem::size_of::<Header>() == HEADER);

/// What a map's header says of its elements, as `bpf(BPF_MAP_CREATE)` set
/// it: fixed for the map's life.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub ty: u32,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    pub flags: u32,
}

impl Params {
    /// The parameters of the map whose file is `len` bytes, with its
    /// header at its end, read once: its data before the header, and its
    /// elements, as its parameters lay them out, inside its data.
    ///
    /// # Safety
    /// `header` is the last [`HEADER`] bytes of a live mapping of the file.
    unsafe fn read(header: *const Header, len: usize) -> Option<Params> {
        // SAFETY: one copy of the header; what is checked is the copy.
        let h = unsafe { std::ptr::read_volatile(header) };
        let p = Params {
            ty: h.ty,
            key_size: h.key_size,
            value_size: h.value_size,
            max_entries: h.max_entries,
            flags: h.flags,
        };
        (h.magic == MAGIC
            && h.kind == KIND_MAP
            && (h.data_len as usize).checked_add(HEADER) == Some(len)
            && elements_len(p.ty, p.key_size, p.value_size, p.max_entries)
                .is_some_and(|n| n <= h.data_len as usize))
        .then_some(p)
    }
}

pub fn round8(n: u32) -> usize {
    (n as usize).div_ceil(8) * 8
}

pub fn percpu(ty: u32) -> bool {
    matches!(ty, PERCPU_HASH | PERCPU_ARRAY | LRU_PERCPU_HASH)
}

/// Maps keyed by content. An LPM trie is kept as an exact-match hash
/// (userspace sets and reads its entries; longest-prefix lookups are the
/// programs').
pub fn hashed(ty: u32) -> bool {
    matches!(
        ty,
        HASH | PERCPU_HASH | LRU_HASH | LRU_PERCPU_HASH | LPM_TRIE | HASH_OF_MAPS | DEVMAP_HASH
    )
}

/// Maps indexed by a u32 key. Maps of programs, maps and devices hold
/// what userspace stored (fds, ifindexes).
pub fn arrayed(ty: u32) -> bool {
    matches!(
        ty,
        ARRAY | PERCPU_ARRAY | PROG_ARRAY | ARRAY_OF_MAPS | DEVMAP
    )
}

pub fn ncpu() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Bytes of the elements of a map of type `ty` with these parameters, as
/// `bpf(BPF_MAP_CREATE)` lays them out; None for parameters no map has.
/// A ring buffer is a consumer page, a producer page, then its data area
/// (a power of two, page aligned), as the kernel's mmap layout. Other
/// types (sockets, ...) hold kernel objects the programs would use; there
/// are none.
pub fn elements_len(ty: u32, key_size: u32, value_size: u32, max_entries: u32) -> Option<usize> {
    Some(if ty == RINGBUF {
        // `ringbuf_map_alloc`: a page-aligned power of two.
        if !max_entries.is_power_of_two()
            || (max_entries as usize) < PAGE
            || key_size != 0
            || value_size != 0
        {
            return None;
        }
        2 * PAGE + max_entries as usize
    } else if hashed(ty) || arrayed(ty) {
        if max_entries == 0 || value_size == 0 || key_size == 0 || (!hashed(ty) && key_size != 4) {
            return None;
        }
        let value = if percpu(ty) {
            round8(value_size).checked_mul(ncpu())?
        } else {
            value_size as usize
        };
        let value = value.checked_next_multiple_of(8)?;
        let per = if hashed(ty) {
            (8 + round8(key_size)).checked_add(value)?
        } else {
            value
        };
        per.checked_mul(max_entries as usize)?
    } else {
        return None;
    })
}

/// Why a key has no slot to store it in.
#[derive(Debug, PartialEq)]
pub enum NoSlot {
    /// BPF_NOEXIST, and the key is there.
    Exists,
    /// BPF_EXIST, and the key is not there.
    Missing,
    /// The map is full.
    Full,
}

/// `flags` of an update: BPF_ANY, BPF_NOEXIST, BPF_EXIST.
pub const BPF_NOEXIST: u64 = 1;
pub const BPF_EXIST: u64 = 2;

/// A map file mapped into a process. Its parameters are those its header
/// had when it was mapped: whatever later writes the shared header, the map
/// reaches only the elements they laid out inside the file.
#[derive(Clone, Copy)]
pub struct Map {
    base: u64,
    header: *const Header,
    params: Params,
}

// SAFETY: the mapping lives as long as its owner keeps it; access goes
// through the header's lock.
unsafe impl Send for Map {}

impl Map {
    /// The map file of `len` bytes mapped at `base`, if its header lays out
    /// its elements inside it.
    ///
    /// # Safety
    /// `base` is a live shared mapping of the whole file.
    pub unsafe fn from_raw(base: u64, len: usize) -> Option<Map> {
        let header = (base as usize + len.checked_sub(HEADER)?) as *const Header;
        // SAFETY: the header at the end of the mapping.
        let params = unsafe { Params::read(header, len) }?;
        Some(Map {
            base,
            header,
            params,
        })
    }

    pub fn params(&self) -> Params {
        self.params
    }

    fn h(&self) -> &Header {
        // SAFETY: a live shared mapping of a checked object file.
        unsafe { &*self.header }
    }

    /// Whether `BPF_MAP_FREEZE` froze the map.
    pub fn frozen(&self) -> bool {
        // SAFETY: the header's word, which only a freeze changes.
        unsafe { std::ptr::read_volatile(&raw const self.h().frozen) != 0 }
    }

    /// A ring buffer's bytes its consumer has yet to read: the producer
    /// position less the consumer's (`ringbuf_avail_data_sz`). None for
    /// other maps.
    pub fn ring_available(&self) -> Option<u64> {
        if self.params.ty != RINGBUF {
            return None;
        }
        // SAFETY: the consumer and producer pages that start a ring
        // buffer's elements, which `elements_len` lays out in the file.
        let (consumer, producer) = unsafe {
            (
                &*(self.base as *const AtomicU64),
                &*((self.base + PAGE as u64) as *const AtomicU64),
            )
        };
        Some(
            producer
                .load(Ordering::Acquire)
                .wrapping_sub(consumer.load(Ordering::Acquire)),
        )
    }

    /// Bytes of one value as userspace sees it.
    pub fn value_len(&self) -> usize {
        if percpu(self.params.ty) {
            round8(self.params.value_size) * ncpu()
        } else {
            self.params.value_size as usize
        }
    }

    fn slot_len(&self) -> usize {
        8 + round8(self.params.key_size) + round8(self.value_len() as u32)
    }

    /// The map's lock, which names its holder. A holder that is no
    /// longer running (it died, or its process exec'd, holding it) leaves
    /// it to the next.
    pub fn lock(&self) -> Guard<'_> {
        let l = &self.h().lock;
        let me = Holder::me();
        let mut spins = 0u32;
        loop {
            let holder = match l.compare_exchange_weak(0, me, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return Guard(l),
                Err(holder) => holder,
            };
            spins = spins.wrapping_add(1);
            if holder != 0
                && spins % 1024 == 0
                && !Holder::running(holder)
                && l.compare_exchange(holder, me, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return Guard(l);
            }
            std::thread::yield_now();
        }
    }

    pub fn slot(&self, i: usize) -> *mut u8 {
        (self.base as usize + i * self.slot_len()) as *mut u8
    }

    /// The array element or hash slot value for `key`, if present.
    pub fn find(&self, key: &[u8]) -> Option<*mut u8> {
        if !hashed(self.params.ty) {
            let i = u32::from_ne_bytes(key.try_into().ok()?) as usize;
            if i >= self.params.max_entries as usize {
                return None;
            }
            return Some((self.base as usize + i * round8(self.value_len() as u32)) as *mut u8);
        }
        self.probe(key).ok().map(|i| self.value_of(i))
    }

    pub fn value_of(&self, i: usize) -> *mut u8 {
        // SAFETY: inside slot i.
        unsafe { self.slot(i).add(8 + round8(self.params.key_size)) }
    }

    /// The slot to store `key` in under `flags`, taking a free one for a
    /// new key. Under the lock.
    pub fn claim(&self, key: &[u8], flags: u64) -> Result<usize, NoSlot> {
        let count = &self.h().count;
        match self.probe(key) {
            Ok(_) if flags == BPF_NOEXIST => Err(NoSlot::Exists),
            Ok(i) => Ok(i),
            Err(_) if flags == BPF_EXIST => Err(NoSlot::Missing),
            Err(Some(i)) if count.load(Ordering::Relaxed) < self.params.max_entries => {
                let s = self.slot(i);
                // SAFETY: a free slot of the mapping, and a key of its size.
                unsafe {
                    std::ptr::copy_nonoverlapping(key.as_ptr(), s.add(8), key.len());
                    (s as *mut u32).write(SLOT_USED);
                }
                count.fetch_add(1, Ordering::Relaxed);
                Ok(i)
            }
            Err(_) => Err(NoSlot::Full),
        }
    }

    /// Deletes the element in used slot `i`. Under the lock.
    pub fn remove(&self, i: usize) {
        // SAFETY: slot i of the mapping.
        unsafe { (self.slot(i) as *mut u32).write(SLOT_DELETED) };
        self.h().count.fetch_sub(1, Ordering::Relaxed);
    }

    /// The slot of `key` (Ok), or the first free slot on its probe path.
    pub fn probe(&self, key: &[u8]) -> Result<usize, Option<usize>> {
        if key.len() != self.params.key_size as usize {
            return Err(None);
        }
        let n = self.params.max_entries as usize;
        let start = fnv(key) as usize % n;
        let mut free = None;
        for step in 0..n {
            let i = (start + step) % n;
            let s = self.slot(i);
            // SAFETY: slot i of the mapping.
            let state = unsafe { (s as *const u32).read() };
            match state {
                SLOT_EMPTY => return Err(free.or(Some(i))),
                SLOT_DELETED => {
                    free.get_or_insert(i);
                }
                _ => {
                    // SAFETY: the key follows the 8-byte slot header.
                    let k = unsafe { std::slice::from_raw_parts(s.add(8), key.len()) };
                    if k == key {
                        return Ok(i);
                    }
                }
            }
        }
        Err(free)
    }

    /// A copy of the value of `key`, as BPF_MAP_LOOKUP_ELEM gives it.
    pub fn lookup(&self, key: &[u8]) -> Option<Vec<u8>> {
        if key.len() != self.params.key_size as usize {
            return None;
        }
        let _g = self.lock();
        let v = self.find(key)?;
        // SAFETY: the value in the mapping, under the map's lock.
        Some(unsafe { std::slice::from_raw_parts(v, self.value_len()) }.to_vec())
    }

    /// A snapshot of the keys visited by BPF_MAP_GET_NEXT_KEY. Values can
    /// change or disappear before a subsequent lookup.
    pub fn keys(&self) -> io::Result<Vec<Vec<u8>>> {
        let _g = self.lock();
        let h = self.params;
        if arrayed(h.ty) {
            return Ok((0..h.max_entries)
                .map(|i| i.to_ne_bytes().to_vec())
                .collect());
        }
        if !hashed(h.ty) {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        Ok((0..h.max_entries as usize)
            .filter_map(|i| {
                let slot = self.slot(i);
                // SAFETY: slot i and its key belong to the mapping; the
                // lock excludes updates and deletion while copying keys.
                unsafe {
                    (slot.cast::<u32>().read() == SLOT_USED).then(|| {
                        std::slice::from_raw_parts(slot.add(8), h.key_size as usize).to_vec()
                    })
                }
            })
            .collect())
    }
}

pub struct Guard<'a>(&'a AtomicU64);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

/// A lock holder: a process's pid (low half) and its identity version
/// (high half), which Darwin moves on every exec, so the word names one
/// program run by the process.
struct Holder;

/// `proc_uniqidentifierinfo`, after the BSD information in
/// `PROC_PIDT_BSDINFOWITHUNIQID`.
#[repr(C)]
struct BsdWithIdentity {
    bsd: libc::proc_bsdinfo,
    uuid: [u8; 16],
    unique_id: u64,
    parent_unique_id: u64,
    id_version: i32,
    parent_id_version: u32,
    _reserved: [u64; 2],
}
const PROC_PIDT_BSDINFOWITHUNIQID: i32 = 18;

impl Holder {
    /// The word naming this process's current program.
    fn me() -> u64 {
        static ME: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        let cached = ME.load(Ordering::Relaxed);
        // A fork child has its own pid; an exec, its own statics.
        if cached != 0 && cached as u32 == pid {
            return cached;
        }
        let word = Holder::word(pid).unwrap_or(pid as u64);
        ME.store(word, Ordering::Relaxed);
        word
    }

    /// The word of process `pid`, unless it is no running process (gone,
    /// or a zombie).
    fn word(pid: u32) -> Option<u64> {
        let mut info: BsdWithIdentity = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<BsdWithIdentity>() as i32;
        // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
        let n = unsafe {
            libc::proc_pidinfo(
                i32::try_from(pid).ok()?,
                PROC_PIDT_BSDINFOWITHUNIQID,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        (n == size && info.bsd.pbi_status != libc::SZOMB)
            .then_some((info.id_version as u32 as u64) << 32 | pid as u64)
    }

    /// Whether `holder` still runs the program that took the lock.
    fn running(holder: u64) -> bool {
        Holder::word(holder as u32) == Some(holder)
    }
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &c| {
        (h ^ c as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// A map file at a host path, mapped while this lives.
pub struct Mapped {
    map: Map,
    len: usize,
}

// SAFETY: the mapping is owned; access goes through the header's lock.
unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

/// The directory of a boot's object files, beside its bpffs area
/// (`<runtime>/bpf`).
pub const OBJECTS: &str = "bpf-objects";

/// The object files of the boot whose bpffs area is `area`.
pub fn objects_beside(area: &Path) -> std::path::PathBuf {
    area.with_file_name(OBJECTS)
}
/// The host attribute naming a pin's object file in [`OBJECTS`]: one the
/// guest cannot set or read (its attributes are `dev.aim.xattr.*`), so a
/// file the guest made is no pin.
const PIN: &std::ffi::CStr = c"dev.aim.bpf.pin";

/// The pin template of object file `object`: an empty file beside it that
/// carries [`PIN`].
fn template(object: &Path) -> std::path::PathBuf {
    let mut name = object.as_os_str().to_os_string();
    name.push(".pin");
    name.into()
}

/// Makes the pin template of the new object file `object` (in
/// [`OBJECTS`]), before any process can pin it.
pub fn make_pin_template(object: &Path) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let name = object
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let path = template(object);
    let file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    // SAFETY: the attribute's name and value outlive the call.
    if unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            PIN.as_ptr(),
            name.as_bytes().as_ptr().cast(),
            name.len(),
            0,
            0,
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(())
}

/// Pins the object file `object` (in [`OBJECTS`]) at `pin`: as bpffs's, an
/// empty file of its own, so its mode and owner are its own, that refers
/// to the object. A clone of the object's pin template, it appears whole
/// or not at all. Fails with `AlreadyExists` when `pin` exists.
pub fn pin(pin: &Path, object: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(template(object).as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(pin.as_os_str().as_bytes())?;
    // SAFETY: two paths; clonefile fails with EEXIST on an existing `to`.
    if unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Removes the pin template of `object`, whose creation failed.
pub fn remove_pin_template(object: &Path) {
    let _ = std::fs::remove_file(template(object));
}

/// The object file pinned at `pin`, of those in `objects`. A file that is
/// no pin fails with `InvalidData`.
pub fn object(pin: &Path, objects: &Path) -> io::Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(pin.as_os_str().as_bytes())?;
    let mut name = [0u8; 256];
    // SAFETY: a bounded read of the attribute into the local buffer.
    let n = unsafe {
        libc::getxattr(
            path.as_ptr(),
            PIN.as_ptr(),
            name.as_mut_ptr().cast(),
            name.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    let not_pin = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: not a pinned object", pin.display()),
        )
    };
    if n < 0 {
        let error = io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::ENOATTR) => not_pin(),
            _ => error,
        });
    }
    let name = std::ffi::OsStr::from_bytes(&name[..n as usize]);
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(std::path::Component::Normal(_))) || parts.next().is_some() {
        return Err(not_pin());
    }
    Ok(objects.join(name))
}

impl Mapped {
    /// The map pinned at `path`, of the objects in `objects`.
    pub fn open(path: &Path, objects: &Path) -> io::Result<Mapped> {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .open(object(path, objects)?)?;
        let len = file.metadata()?.len() as usize;
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: not a map file", path.display()),
            )
        };
        if len < HEADER {
            return Err(invalid());
        }
        // SAFETY: a shared mapping of the whole file.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the shared mapping of the whole file made above.
        let Some(map) = (unsafe { Map::from_raw(base as u64, len) }) else {
            // SAFETY: the mapping made above.
            unsafe { libc::munmap(base, len) };
            return Err(invalid());
        };
        Ok(Mapped { map, len })
    }

    pub fn map(&self) -> &Map {
        &self.map
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: the mapping this owns.
        unsafe { libc::munmap(self.map.base as *mut libc::c_void, self.len) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hash map file of `n` slots, as `bpf(BPF_MAP_CREATE)` lays it out,
    /// in `dir`'s objects, pinned at `dir/bpf/map`.
    fn file(dir: &Path, n: u32) -> std::path::PathBuf {
        let slot = 8 + round8(4) + round8(8);
        let data_len = slot * n as usize;
        let mut b = vec![0u8; data_len + HEADER];
        let h = &mut b[data_len..];
        h[..8].copy_from_slice(&MAGIC.to_le_bytes());
        for (at, v) in [(8, KIND_MAP), (12, HASH), (16, 4), (20, 8), (24, n)] {
            h[at..at + 4].copy_from_slice(&v.to_ne_bytes());
        }
        h[64..72].copy_from_slice(&(data_len as u64).to_ne_bytes());
        let object = dir.join(OBJECTS).join("obj.map");
        std::fs::create_dir_all(dir.join(OBJECTS)).unwrap();
        std::fs::create_dir_all(dir.join("bpf")).unwrap();
        std::fs::write(&object, b).unwrap();
        make_pin_template(&object).unwrap();
        let path = dir.join("bpf/map");
        pin(&path, &object).unwrap();
        path
    }

    #[test]
    fn a_mapped_file_claims_finds_and_looks_up_keys() {
        let dir = std::env::temp_dir().join(format!("aim-bpf-object-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir, 2);
        let objects = objects_beside(&dir.join("bpf"));
        let m = Mapped::open(&path, &objects).unwrap();
        let map = m.map();
        assert_eq!(map.lookup(&7u32.to_ne_bytes()), None);
        {
            let _g = map.lock();
            for k in [7u32, 8] {
                let i = map.claim(&k.to_ne_bytes(), 0).unwrap();
                // SAFETY: the claimed slot's value.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (k as u64 * 10).to_ne_bytes().as_ptr(),
                        map.value_of(i),
                        8,
                    )
                };
            }
            assert_eq!(map.claim(&9u32.to_ne_bytes(), 0), Err(NoSlot::Full));
            assert_eq!(
                map.claim(&7u32.to_ne_bytes(), BPF_NOEXIST),
                Err(NoSlot::Exists)
            );
        }
        assert_eq!(
            map.lookup(&8u32.to_ne_bytes()),
            Some(80u64.to_ne_bytes().to_vec())
        );
        // Another mapping of the file sees the same elements.
        let again = Mapped::open(&path, &objects).unwrap();
        assert_eq!(
            again.map().lookup(&7u32.to_ne_bytes()),
            Some(70u64.to_ne_bytes().to_vec())
        );
        assert_eq!(again.map().lookup(&[0; 3]), None);
        // A pin is its own inode; a second one names the same object.
        assert_eq!(
            pin(&path, &objects.join("obj.map")).err().map(|e| e.kind()),
            Some(io::ErrorKind::AlreadyExists)
        );
        let second = dir.join("bpf/second");
        pin(&second, &objects.join("obj.map")).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            std::fs::metadata(&path).unwrap().ino(),
            std::fs::metadata(&second).unwrap().ino()
        );
        assert_eq!(
            Mapped::open(&second, &objects)
                .unwrap()
                .map()
                .lookup(&8u32.to_ne_bytes()),
            Some(80u64.to_ne_bytes().to_vec())
        );
        // A header whose parameters lay out more than its data, or whose
        // data is not the file's, maps nothing.
        let backing = objects.join("obj.map");
        let whole = std::fs::read(&backing).unwrap();
        let at = whole.len() - HEADER;
        for (offset, value) in [(24, 3u64), (16, 9), (64, 32)] {
            let mut bytes = whole.clone();
            let width = if offset == 64 { 8 } else { 4 };
            bytes[at + offset..at + offset + width].copy_from_slice(&value.to_ne_bytes()[..width]);
            std::fs::write(&backing, bytes).unwrap();
            assert_eq!(
                Mapped::open(&path, &objects).err().map(|e| e.kind()),
                Some(io::ErrorKind::InvalidData),
                "header field at {offset}"
            );
        }
        std::fs::write(objects.join("obj.map"), [0u8; 64]).unwrap();
        assert_eq!(
            Mapped::open(&path, &objects).err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        // Neither a file the guest could make (whatever it holds), nor a pin
        // naming a path outside the objects, opens; a pin is empty.
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        let forged = dir.join("bpf/forged");
        std::fs::write(&forged, b"obj.map").unwrap();
        assert_eq!(
            object(&forged, &objects).err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        let outside = std::ffi::CString::new(forged.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a path, an attribute name and value.
        assert_eq!(
            unsafe {
                libc::setxattr(
                    outside.as_ptr(),
                    PIN.as_ptr(),
                    b"../bpf/map".as_ptr().cast(),
                    10,
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(
            object(&forged, &objects).err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mapped_map_keeps_its_parameters_and_frees_a_dead_holders_lock() {
        let dir = std::env::temp_dir().join(format!("aim-bpf-header-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir, 2);
        let objects = objects_beside(&dir.join("bpf"));
        let m = Mapped::open(&path, &objects).unwrap();
        let map = m.map();
        {
            let _g = map.lock();
            for k in [1u32, 2] {
                map.claim(&k.to_ne_bytes(), 0).unwrap();
            }
        }
        // Another writer of the shared header: more entries and a wider
        // key than the file holds, and no count.
        let backing = objects.join("obj.map");
        let mut bytes = std::fs::read(&backing).unwrap();
        let at = bytes.len() - HEADER;
        for (offset, value) in [(16, 64u32), (24, 1 << 30), (36, 0)] {
            bytes[at + offset..at + offset + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&backing)
            .unwrap();
        std::os::unix::fs::FileExt::write_all_at(&file, &bytes[at..], at as u64).unwrap();
        assert_eq!(
            map.params(),
            Params {
                ty: HASH,
                key_size: 4,
                value_size: 8,
                max_entries: 2,
                flags: 0
            }
        );
        let mut keys = map.keys().unwrap();
        keys.sort();
        assert_eq!(keys, [1u32, 2].map(|k| k.to_ne_bytes().to_vec()));
        {
            let _g = map.lock();
            assert_eq!(map.claim(&3u32.to_ne_bytes(), 0), Err(NoSlot::Full));
            assert_eq!(map.probe(&[0; 64]), Err(None));
        }
        // A lock is free once its holder is gone, a zombie, or runs another
        // program, and when its word names no process at all.
        let lock = at as u64 + std::mem::offset_of!(Header, lock) as u64;
        let hold = |word: u64| {
            std::os::unix::fs::FileExt::write_all_at(&file, &word.to_ne_bytes(), lock).unwrap();
            assert_eq!(map.lookup(&9u32.to_ne_bytes()), None);
        };
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let word = Holder::word(child.id()).unwrap();
        assert!(Holder::running(word));
        child.kill().unwrap();
        while Holder::word(child.id()).is_some() {
            std::thread::yield_now();
        }
        hold(word);
        child.wait().unwrap();
        hold(word);
        hold(Holder::me() + (1 << 32));
        hold(1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
