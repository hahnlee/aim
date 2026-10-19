//! The file of one eBPF object of a boot (`<runtime>/bpf-objects`), which
//! the Linux layer's `bpf(2)` creates and pins and which the host's services
//! read: its data (at offset 0, so a ring buffer's mmap sees the kernel
//! layout) followed by a [`Header`]. A map's data is its elements: an
//! array's values in key order, or a hash's open-addressed slots (an
//! 8-byte state, the key and the value, each padded to 8 bytes). Processes
//! map it shared and serialize updates with the spin lock in the header.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

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
    pub lock: AtomicU32,
    pub count: AtomicU32,
    pub frozen: u32,
    pub name: [u8; 16],
    /// Bytes of data before the header.
    pub data_len: u64,
    pub _pad: [u8; 56],
}
const _: () = assert!(std::mem::size_of::<Header>() == HEADER);

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

/// A map file mapped into a process.
#[derive(Clone, Copy)]
pub struct Map {
    base: u64,
    header: *const Header,
}

// SAFETY: the mapping lives as long as its owner keeps it; access goes
// through the header's lock.
unsafe impl Send for Map {}

impl Map {
    /// The map whose data starts at `base`, with its header at `header`.
    ///
    /// # Safety
    /// Both point into a live shared mapping of a whole map file.
    pub unsafe fn from_raw(base: u64, header: *const Header) -> Map {
        Map { base, header }
    }

    pub fn h(&self) -> &Header {
        // SAFETY: a live shared mapping of a checked object file.
        unsafe { &*self.header }
    }

    /// Bytes of one value as userspace sees it.
    pub fn value_len(&self) -> usize {
        let h = self.h();
        if percpu(h.ty) {
            round8(h.value_size) * ncpu()
        } else {
            h.value_size as usize
        }
    }

    fn slot_len(&self) -> usize {
        8 + round8(self.h().key_size) + round8(self.value_len() as u32)
    }

    pub fn lock(&self) -> Guard<'_> {
        let l = &self.h().lock;
        while l
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::thread::yield_now();
        }
        Guard(l)
    }

    pub fn slot(&self, i: usize) -> *mut u8 {
        (self.base as usize + i * self.slot_len()) as *mut u8
    }

    /// The array element or hash slot value for `key`, if present.
    pub fn find(&self, key: &[u8]) -> Option<*mut u8> {
        let h = self.h();
        if !hashed(h.ty) {
            let i = u32::from_ne_bytes(key.try_into().ok()?) as usize;
            if i >= h.max_entries as usize {
                return None;
            }
            return Some((self.base as usize + i * round8(self.value_len() as u32)) as *mut u8);
        }
        self.probe(key).ok().map(|i| self.value_of(i))
    }

    pub fn value_of(&self, i: usize) -> *mut u8 {
        // SAFETY: inside slot i.
        unsafe { self.slot(i).add(8 + round8(self.h().key_size)) }
    }

    /// The slot to store `key` in under `flags`, taking a free one for a
    /// new key. Under the lock.
    pub fn claim(&self, key: &[u8], flags: u64) -> Result<usize, NoSlot> {
        let h = self.h();
        match self.probe(key) {
            Ok(_) if flags == BPF_NOEXIST => Err(NoSlot::Exists),
            Ok(i) => Ok(i),
            Err(_) if flags == BPF_EXIST => Err(NoSlot::Missing),
            Err(Some(i)) if h.count.load(Ordering::Relaxed) < h.max_entries => {
                let s = self.slot(i);
                // SAFETY: a free slot of the mapping.
                unsafe {
                    std::ptr::copy_nonoverlapping(key.as_ptr(), s.add(8), key.len());
                    (s as *mut u32).write(SLOT_USED);
                }
                h.count.fetch_add(1, Ordering::Relaxed);
                Ok(i)
            }
            Err(_) => Err(NoSlot::Full),
        }
    }

    /// The slot of `key` (Ok), or the first free slot on its probe path.
    pub fn probe(&self, key: &[u8]) -> Result<usize, Option<usize>> {
        let n = self.h().max_entries as usize;
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
        if key.len() != self.h().key_size as usize {
            return None;
        }
        let _g = self.lock();
        let v = self.find(key)?;
        // SAFETY: the value in the mapping, under the map's lock.
        Some(unsafe { std::slice::from_raw_parts(v, self.value_len()) }.to_vec())
    }
}

pub struct Guard<'a>(&'a AtomicU32);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
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

impl Mapped {
    /// The map file at `path`.
    pub fn open(path: &Path) -> io::Result<Mapped> {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::options().read(true).write(true).open(path)?;
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
        let header = (base as usize + len - HEADER) as *const Header;
        // SAFETY: the header at the end of the mapping.
        let h = unsafe { &*header };
        let mapped = Mapped {
            map: Map {
                base: base as u64,
                header,
            },
            len,
        };
        if h.magic != MAGIC
            || h.kind != KIND_MAP
            || h.data_len as usize + HEADER != len
            || h.max_entries == 0
        {
            return Err(invalid());
        }
        Ok(mapped)
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

    /// A hash map file of `n` slots, as `bpf(BPF_MAP_CREATE)` lays it out.
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
        let path = dir.join("map");
        std::fs::write(&path, b).unwrap();
        path
    }

    #[test]
    fn a_mapped_file_claims_finds_and_looks_up_keys() {
        let dir = std::env::temp_dir().join(format!("aim-bpf-object-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file(&dir, 2);
        let m = Mapped::open(&path).unwrap();
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
        let again = Mapped::open(&path).unwrap();
        assert_eq!(
            again.map().lookup(&7u32.to_ne_bytes()),
            Some(70u64.to_ne_bytes().to_vec())
        );
        assert_eq!(again.map().lookup(&[0; 3]), None);
        std::fs::write(&path, [0u8; 64]).unwrap();
        assert_eq!(
            Mapped::open(&path).err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
