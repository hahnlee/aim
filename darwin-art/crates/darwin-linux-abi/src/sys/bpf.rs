//! `bpf(2)`: maps backed by shared memory; programs are loaded but never
//! run (ADR 0012 appendix, "Kernel features to emulate").
//!
//! Android's network stack keeps its state in eBPF maps that the loader
//! (NetBpfLoad) creates and pins under `/sys/fs/bpf`, and that netd,
//! system_server (BpfNetMaps, TrafficStats) and the DnsResolver open and
//! read. The programs that would fill the maps from kernel hooks (socket
//! creation, cgroup ingress/egress) are accepted and attached, and do
//! nothing.
//!
//! Every map and program is a file in the boot's runtime directory
//! (`<runtime>/bpf-objects`), so a pin is a hard link into the bpffs area
//! of the path map and `BPF_OBJ_GET` an open, and any process that holds
//! the fd (by fork, SCM_RIGHTS or binder) reaches the same object. A map
//! file is its data (at offset 0, so a ring buffer's mmap sees the kernel
//! layout) followed by a header; processes map it shared and serialize
//! updates with a spin lock in the header.
//!
//! Supported map types: arrays and hashes, with their per-CPU and LRU
//! variants (an LRU hash fails with E2BIG when full), maps of programs,
//! maps and devices (holding what userspace stored), LPM tries as
//! exact-match hashes, and ring buffers for mmap (no producer ever writes
//! them).

use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::errno::{self, E2BIG, EBADF, EEXIST, EFAULT, EINVAL, ENOENT, EPERM};
use crate::vfs::{self, LINUX_AT_FDCWD};

const MAP_CREATE: u64 = 0;
const MAP_LOOKUP_ELEM: u64 = 1;
const MAP_UPDATE_ELEM: u64 = 2;
const MAP_DELETE_ELEM: u64 = 3;
const MAP_GET_NEXT_KEY: u64 = 4;
const PROG_LOAD: u64 = 5;
const OBJ_PIN: u64 = 6;
const OBJ_GET: u64 = 7;
const PROG_ATTACH: u64 = 8;
const PROG_DETACH: u64 = 9;
const OBJ_GET_INFO_BY_FD: u64 = 15;
const BTF_LOAD: u64 = 18;
const MAP_FREEZE: u64 = 22;
const LINK_CREATE: u64 = 28;

const HASH: u32 = 1;
const ARRAY: u32 = 2;
const PROG_ARRAY: u32 = 3;
const PERCPU_HASH: u32 = 5;
const PERCPU_ARRAY: u32 = 6;
const LRU_HASH: u32 = 9;
const LRU_PERCPU_HASH: u32 = 10;
const LPM_TRIE: u32 = 11;
const ARRAY_OF_MAPS: u32 = 12;
const HASH_OF_MAPS: u32 = 13;
const DEVMAP: u32 = 14;
const DEVMAP_HASH: u32 = 25;
const RINGBUF: u32 = 27;

const BPF_NOEXIST: u64 = 1;
const BPF_EXIST: u64 = 2;

const MAGIC: u64 = u64::from_le_bytes(*b"LXBPFOBJ");
const KIND_MAP: u32 = 1;
const KIND_PROG: u32 = 2;
const KIND_BTF: u32 = 3;
const HEADER: usize = 128;
const SLOT_EMPTY: u32 = 0;
const SLOT_USED: u32 = 1;
const SLOT_DELETED: u32 = 2;

/// The header at the end of an object file.
#[repr(C)]
struct Header {
    magic: u64,
    kind: u32,
    /// Map type, or program type.
    ty: u32,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
    flags: u32,
    id: u32,
    lock: AtomicU32,
    count: AtomicU32,
    frozen: u32,
    name: [u8; 16],
    /// Bytes of data before the header.
    data_len: u64,
    _pad: [u8; 56],
}
const _: () = assert!(std::mem::size_of::<Header>() == HEADER);

fn round8(n: u32) -> usize {
    (n as usize).div_ceil(8) * 8
}

fn percpu(ty: u32) -> bool {
    matches!(ty, PERCPU_HASH | PERCPU_ARRAY | LRU_PERCPU_HASH)
}

/// Maps keyed by content. An LPM trie is kept as an exact-match hash
/// (userspace sets and reads its entries; longest-prefix lookups are the
/// programs').
fn hashed(ty: u32) -> bool {
    matches!(
        ty,
        HASH | PERCPU_HASH | LRU_HASH | LRU_PERCPU_HASH | LPM_TRIE | HASH_OF_MAPS | DEVMAP_HASH
    )
}

/// Maps indexed by a u32 key. Maps of programs, maps and devices hold
/// what userspace stored (fds, ifindexes).
fn arrayed(ty: u32) -> bool {
    matches!(
        ty,
        ARRAY | PERCPU_ARRAY | PROG_ARRAY | ARRAY_OF_MAPS | DEVMAP
    )
}

fn ncpu() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// A map mapped into this process.
struct Map {
    base: u64,
    header: *const Header,
}

// SAFETY: the mapping lives for the process; access goes through the
// header's lock.
unsafe impl Send for Map {}

static MAPPED: Mutex<Option<HashMap<(u64, u64), Map>>> = Mutex::new(None);

impl Map {
    fn h(&self) -> &Header {
        // SAFETY: a live shared mapping of a checked object file.
        unsafe { &*self.header }
    }

    /// Bytes of one value as userspace sees it.
    fn value_len(&self) -> usize {
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

    fn lock(&self) -> Guard<'_> {
        let l = &self.h().lock;
        while l
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::thread::yield_now();
        }
        Guard(l)
    }

    fn slot(&self, i: usize) -> *mut u8 {
        (self.base as usize + i * self.slot_len()) as *mut u8
    }

    /// The array element or hash slot value for `key`, if present.
    fn find(&self, key: &[u8]) -> Option<*mut u8> {
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

    fn value_of(&self, i: usize) -> *mut u8 {
        // SAFETY: inside slot i.
        unsafe { self.slot(i).add(8 + round8(self.h().key_size)) }
    }

    /// The slot of `key` (Ok), or the first free slot on its probe path.
    fn probe(&self, key: &[u8]) -> Result<usize, Option<usize>> {
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
}

struct Guard<'a>(&'a AtomicU32);

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

/// Where the object files of this boot live.
fn objects_dir() -> PathBuf {
    let dir = match vfs::runtime_dir() {
        Some(r) => r.join("bpf-objects"),
        None => std::env::temp_dir().join(format!("linux-abi-bpf-{}", unsafe { libc::getpid() })),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A new object file of `data_len` bytes plus the header; returns its fd.
fn create_object(mut header: Header, data_len: usize) -> i64 {
    // A unique name: the pid survives exec, a counter does not.
    let Ok(tmpl) = CString::new(objects_dir().join("obj.XXXXXX").as_os_str().as_bytes()) else {
        return -(EINVAL as i64);
    };
    let mut tmpl = tmpl.into_bytes_with_nul();
    // SAFETY: mkstemp fills in our template and creates the file.
    let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    // SAFETY: plain fcntl on our new fd.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat of the file we created.
    unsafe { libc::fstat(fd, &mut st) };
    header.id = st.st_ino as u32;
    header.data_len = data_len as u64;
    // SAFETY: sizing the file and writing the header after the data.
    unsafe {
        if libc::ftruncate(fd, (data_len + HEADER) as i64) != 0
            || libc::pwrite(fd, (&raw const header).cast(), HEADER, data_len as i64)
                != HEADER as isize
        {
            let e = errno::last();
            libc::close(fd);
            return -(e as i64);
        }
    }
    fd as i64
}

/// The header of object file `fd`, if it is one.
fn read_header(fd: i32) -> Option<(Header, libc::stat)> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } != 0 || (st.st_size as usize) < HEADER {
        return None;
    }
    // SAFETY: a zeroed header, then filled from the file.
    let mut h: Header = unsafe { std::mem::zeroed() };
    let at = st.st_size - HEADER as i64;
    // SAFETY: reading into the local header.
    if unsafe { libc::pread(fd, (&raw mut h).cast(), HEADER, at) } != HEADER as isize
        || h.magic != MAGIC
    {
        return None;
    }
    Some((h, st))
}

/// Run `f` on the map behind `fd`, mapping it into this process once.
fn with_map<R>(fd: i32, f: impl FnOnce(&Map) -> Result<R, i64>) -> Result<R, i64> {
    let (h, st) = read_header(fd).ok_or(-(EBADF as i64))?;
    if h.kind != KIND_MAP {
        return Err(-(EINVAL as i64));
    }
    let key = (st.st_dev as u32 as u64, st.st_ino);
    let mut g = MAPPED.lock().unwrap();
    let maps = g.get_or_insert_with(HashMap::new);
    let map = match maps.entry(key) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => {
            let len = st.st_size as usize;
            // SAFETY: a shared mapping of the whole object file.
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(-(errno::last() as i64));
            }
            let header = (base as usize + h.data_len as usize) as *const Header;
            e.insert(Map {
                base: base as u64,
                header,
            })
        }
    };
    f(map)
}

/// Copy `len` guest bytes at `p`.
fn guest_bytes(p: u64, len: usize) -> Result<Vec<u8>, i64> {
    if p == 0 && len != 0 {
        return Err(-(EFAULT as i64));
    }
    // SAFETY: guest buffer of `len` bytes.
    Ok(unsafe { std::slice::from_raw_parts(p as *const u8, len) }.to_vec())
}

/// BPF_F_RDONLY_PROG: the kernel sets it on device maps.
const F_RDONLY_PROG: u32 = 1 << 7;

fn map_create(attr: &[u32]) -> i64 {
    let (ty, key_size, value_size, max_entries, mut flags) =
        (attr[0], attr[1], attr[2], attr[3], attr[4]);
    if matches!(ty, DEVMAP | DEVMAP_HASH) {
        flags |= F_RDONLY_PROG;
    }
    let mut name = [0u8; 16];
    for (i, w) in attr[7..11].iter().enumerate() {
        name[i * 4..i * 4 + 4].copy_from_slice(&w.to_ne_bytes());
    }
    let data_len = match ty {
        RINGBUF => {
            // Consumer page, producer page, then the data area (a power of
            // two, page aligned), as the kernel's mmap layout.
            if max_entries == 0
                || !max_entries.is_power_of_two()
                || key_size != 0
                || value_size != 0
            {
                return -(EINVAL as i64);
            }
            2 * 16384 + max_entries as usize
        }
        _ if hashed(ty) || arrayed(ty) => {
            if max_entries == 0
                || value_size == 0
                || (key_size == 0)
                || (!hashed(ty) && key_size != 4)
            {
                return -(EINVAL as i64);
            }
            let value = if percpu(ty) {
                round8(value_size) * ncpu()
            } else {
                value_size as usize
            };
            let per = if hashed(ty) {
                8 + round8(key_size) + round8(value as u32)
            } else {
                round8(value as u32)
            };
            per * max_entries as usize
        }
        // Other types (program arrays, maps of maps, sockets, ...) hold
        // kernel objects the programs would use; there are none.
        _ => return -(EINVAL as i64),
    };
    let data_len = data_len.div_ceil(16384) * 16384;
    let header = Header {
        magic: MAGIC,
        kind: KIND_MAP,
        ty,
        key_size,
        value_size,
        max_entries,
        flags,
        id: 0,
        lock: AtomicU32::new(0),
        count: AtomicU32::new(0),
        frozen: 0,
        name,
        data_len: 0,
        _pad: [0; 56],
    };
    create_object(header, data_len)
}

fn elem(cmd: u64, a: &[u64]) -> i64 {
    let fd = a[0] as u32 as i32;
    let (key_p, value_p, flags) = (a[1], a[2], a[3]);
    let r = with_map(fd, |m| {
        let h = m.h();
        let key = if h.ty == RINGBUF {
            return Err(-(EINVAL as i64));
        } else {
            guest_bytes(key_p, h.key_size as usize)
        };
        let vlen = m.value_len();
        match cmd {
            MAP_LOOKUP_ELEM => {
                let key = key?;
                let _g = m.lock();
                let v = m.find(&key).ok_or(-(ENOENT as i64))?;
                // SAFETY: the value in the mapping, the guest's buffer.
                unsafe { std::ptr::copy_nonoverlapping(v, value_p as *mut u8, vlen) };
                Ok(0)
            }
            MAP_UPDATE_ELEM => {
                let key = key?;
                let value = guest_bytes(value_p, vlen)?;
                if h.frozen != 0 {
                    return Err(-(EPERM as i64));
                }
                let _g = m.lock();
                if !hashed(h.ty) {
                    if flags == BPF_NOEXIST {
                        return Err(-(EEXIST as i64));
                    }
                    let v = m.find(&key).ok_or(-(E2BIG as i64))?;
                    // SAFETY: the element in the mapping.
                    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), v, vlen) };
                    return Ok(0);
                }
                let slot = match m.probe(&key) {
                    Ok(_) if flags == BPF_NOEXIST => return Err(-(EEXIST as i64)),
                    Ok(i) => i,
                    Err(_) if flags == BPF_EXIST => return Err(-(ENOENT as i64)),
                    Err(Some(i)) if h.count.load(Ordering::Relaxed) < h.max_entries => {
                        let s = m.slot(i);
                        // SAFETY: a free slot of the mapping.
                        unsafe {
                            std::ptr::copy_nonoverlapping(key.as_ptr(), s.add(8), key.len());
                            (s as *mut u32).write(SLOT_USED);
                        }
                        h.count.fetch_add(1, Ordering::Relaxed);
                        i
                    }
                    Err(_) => return Err(-(E2BIG as i64)),
                };
                // SAFETY: the slot's value.
                unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), m.value_of(slot), vlen) };
                Ok(0)
            }
            MAP_DELETE_ELEM => {
                let key = key?;
                if !hashed(h.ty) {
                    return Err(-(EINVAL as i64));
                }
                let _g = m.lock();
                let i = m.probe(&key).map_err(|_| -(ENOENT as i64))?;
                // SAFETY: the used slot i.
                unsafe { (m.slot(i) as *mut u32).write(SLOT_DELETED) };
                h.count.fetch_sub(1, Ordering::Relaxed);
                Ok(0)
            }
            MAP_GET_NEXT_KEY => {
                // A null or missing key starts from the first element.
                let key = if key_p == 0 { None } else { Some(key?) };
                let _g = m.lock();
                let next: Vec<u8> = if !hashed(h.ty) {
                    let i = key
                        .and_then(|k| k.try_into().ok().map(u32::from_ne_bytes))
                        .map_or(0, |i| if i >= h.max_entries { 0 } else { i + 1 });
                    if i >= h.max_entries {
                        return Err(-(ENOENT as i64));
                    }
                    i.to_ne_bytes().to_vec()
                } else {
                    let from = key.and_then(|k| m.probe(&k).ok()).map_or(0, |i| i + 1);
                    let n = h.max_entries as usize;
                    let i = (from..n)
                        // SAFETY: slot headers of the mapping.
                        .find(|&i| unsafe { (m.slot(i) as *const u32).read() } == SLOT_USED)
                        .ok_or(-(ENOENT as i64))?;
                    // SAFETY: the key of used slot i.
                    unsafe { std::slice::from_raw_parts(m.slot(i).add(8), h.key_size as usize) }
                        .to_vec()
                };
                // SAFETY: the guest's next-key buffer.
                unsafe {
                    std::ptr::copy_nonoverlapping(next.as_ptr(), value_p as *mut u8, next.len())
                };
                Ok(0)
            }
            _ => Err(-(EINVAL as i64)),
        }
    });
    r.unwrap_or_else(|e| e)
}

/// Type information for maps and programs: kept (as the object's data) so
/// its size can be reported; nothing verifies against it.
fn btf_load(btf: u64, size: u32) -> i64 {
    if btf == 0 || size == 0 {
        return -(EINVAL as i64);
    }
    // SAFETY: the guest's BTF blob.
    let data = unsafe { std::slice::from_raw_parts(btf as *const u8, size as usize) };
    let header = Header {
        magic: MAGIC,
        kind: KIND_BTF,
        ty: 0,
        key_size: 0,
        value_size: size,
        max_entries: 0,
        flags: 0,
        id: 0,
        lock: AtomicU32::new(0),
        count: AtomicU32::new(0),
        frozen: 0,
        name: [0; 16],
        data_len: 0,
        _pad: [0; 56],
    };
    let fd = create_object(header, size as usize);
    if fd >= 0 {
        // SAFETY: writing the blob at the start of our object file.
        unsafe { libc::pwrite(fd as i32, data.as_ptr().cast(), data.len(), 0) };
    }
    fd
}

fn prog_load(a: &[u32]) -> i64 {
    // prog_type, insn_cnt, insns (u64), license (u64), log_level, log_size,
    // log_buf (u64), kern_version, prog_flags, prog_name[16].
    let mut name = [0u8; 16];
    for (i, w) in a[12..16].iter().enumerate() {
        name[i * 4..i * 4 + 4].copy_from_slice(&w.to_ne_bytes());
    }
    let header = Header {
        magic: MAGIC,
        kind: KIND_PROG,
        ty: a[0],
        key_size: 0,
        value_size: 0,
        max_entries: a[1],
        flags: a[11],
        id: 0,
        lock: AtomicU32::new(0),
        count: AtomicU32::new(0),
        frozen: 0,
        name,
        data_len: 0,
        _pad: [0; 56],
    };
    create_object(header, 0)
}

/// The host path of the object behind `fd`.
fn object_path(fd: i32) -> Option<CString> {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0)?;
    CString::new(&buf[..n]).ok()
}

fn obj_pin(path: u64, fd: i32) -> i64 {
    if read_header(fd).is_none() {
        return -(EBADF as i64);
    }
    // SAFETY: guest string.
    let guest = unsafe { super::guest_cstr(path) };
    let r = match vfs::resolve(LINUX_AT_FDCWD, guest, false) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    let Some(src) = object_path(fd) else {
        return -(EBADF as i64);
    };
    // SAFETY: linking our object file at the host path.
    errno::check(unsafe { libc::link(src.as_ptr(), r.host.as_ptr()) } as i64)
}

fn obj_get(path: u64) -> i64 {
    // SAFETY: guest string.
    let guest = unsafe { super::guest_cstr(path) };
    let r = match vfs::resolve(LINUX_AT_FDCWD, guest, true) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    // SAFETY: opening the pinned object.
    let fd = unsafe { libc::open(r.host.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    if read_header(fd).is_none() {
        // SAFETY: closing what we opened.
        unsafe { libc::close(fd) };
        return -(EPERM as i64);
    }
    fd as i64
}

fn info_by_fd(fd: i32, len_p: u64, info: u64) -> i64 {
    let Some((h, _)) = read_header(fd) else {
        return -(EBADF as i64);
    };
    let mut out = [0u8; 96];
    let put = |out: &mut [u8], at: usize, v: u32| out[at..at + 4].copy_from_slice(&v.to_ne_bytes());
    let used = if h.kind == KIND_BTF {
        // struct bpf_btf_info: btf (u64), btf_size, id, name (u64),
        // name_len, kernel_btf.
        put(&mut out, 8, h.value_size);
        put(&mut out, 12, h.id);
        32
    } else if h.kind == KIND_MAP {
        // struct bpf_map_info: type, id, key_size, value_size, max_entries,
        // map_flags, name[16], ...
        put(&mut out, 0, h.ty);
        put(&mut out, 4, h.id);
        put(&mut out, 8, h.key_size);
        put(&mut out, 12, h.value_size);
        put(&mut out, 16, h.max_entries);
        put(&mut out, 20, h.flags);
        out[24..40].copy_from_slice(&h.name);
        80
    } else {
        // struct bpf_prog_info: type, id, tag[8], jited_prog_len,
        // xlated_prog_len, ..., name[16] at 64. Programs count as JITed
        // (loaders check), with the sizes their instructions would have.
        put(&mut out, 0, h.ty);
        put(&mut out, 4, h.id);
        put(&mut out, 16, h.max_entries * 4);
        put(&mut out, 20, h.max_entries * 8);
        out[64..80].copy_from_slice(&h.name);
        96
    };
    // SAFETY: the guest's info_len and info buffer.
    unsafe {
        let len = (len_p as *const u32).read_unaligned() as usize;
        let n = len.min(used);
        std::ptr::copy_nonoverlapping(out.as_ptr(), info as *mut u8, n);
        (len_p as *mut u32).write_unaligned(n as u32);
    }
    0
}

pub fn bpf(a: [u64; 6]) -> i64 {
    let (cmd, attr, size) = (a[0], a[1], a[2] as usize);
    if attr == 0 {
        return -(EFAULT as i64);
    }
    // bpf_attr is at most a few hundred bytes; read what the guest gave.
    let words = size.min(256) / 4;
    // SAFETY: the guest's bpf_attr of `size` bytes.
    let u32s: Vec<u32> = (0..words.max(24))
        .map(|i| {
            if i < words {
                unsafe { (attr as *const u32).add(i).read_unaligned() }
            } else {
                0
            }
        })
        .collect();
    let u64_at = |w: usize| u32s[w] as u64 | (u32s[w + 1] as u64) << 32;
    match cmd {
        MAP_CREATE => map_create(&u32s),
        MAP_LOOKUP_ELEM | MAP_UPDATE_ELEM | MAP_DELETE_ELEM | MAP_GET_NEXT_KEY => {
            // map_fd, pad, key, value/next_key, flags
            elem(cmd, &[u32s[0] as u64, u64_at(2), u64_at(4), u64_at(6)])
        }
        PROG_LOAD => prog_load(&u32s),
        // btf, btf_log_buf, btf_size
        BTF_LOAD => btf_load(u64_at(0), u32s[4]),
        OBJ_PIN => obj_pin(u64_at(0), u32s[2] as i32),
        OBJ_GET => obj_get(u64_at(0)),
        // Attached programs never run.
        PROG_ATTACH | PROG_DETACH => 0,
        LINK_CREATE => -(EINVAL as i64),
        OBJ_GET_INFO_BY_FD => {
            // bpf_fd, info_len, info
            let len_p = attr + 4;
            info_by_fd(u32s[0] as i32, len_p, u64_at(2))
        }
        MAP_FREEZE => {
            let fd = u32s[0] as i32;
            match read_header(fd) {
                Some((h, st)) if h.kind == KIND_MAP => {
                    let one = 1u32.to_ne_bytes();
                    let at =
                        st.st_size - HEADER as i64 + std::mem::offset_of!(Header, frozen) as i64;
                    // SAFETY: writing the frozen flag of the object's header.
                    errno::check(unsafe { libc::pwrite(fd, one.as_ptr().cast(), 4, at) } as i64)
                        .min(0)
                }
                _ => -(EBADF as i64),
            }
        }
        _ => -(EINVAL as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(cmd: u64, attr: &mut [u32]) -> i64 {
        bpf([
            cmd,
            attr.as_mut_ptr() as u64,
            (attr.len() * 4) as u64,
            0,
            0,
            0,
        ])
    }

    fn ptr(p: *const u8) -> [u32; 2] {
        let v = p as u64;
        [v as u32, (v >> 32) as u32]
    }

    fn elem(fd: i64, cmd: u64, key: *const u8, value: *mut u8, flags: u64) -> i64 {
        let mut a = [0u32; 8];
        a[0] = fd as u32;
        a[2..4].copy_from_slice(&ptr(key));
        a[4..6].copy_from_slice(&ptr(value));
        a[6] = flags as u32;
        call(cmd, &mut a)
    }

    #[test]
    fn a_hash_map_updates_looks_up_iterates_and_deletes() {
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[HASH, 8, 4, 4, 0]);
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0, "{fd}");
        let key = |k: &u64| (k as *const u64).cast::<u8>();
        let mut v = 7u32;
        let vp = (&raw mut v).cast::<u8>();
        for k in [10u64, 20, 30] {
            assert_eq!(elem(fd, MAP_UPDATE_ELEM, key(&k), vp, 0), 0);
        }
        assert_eq!(
            elem(fd, MAP_UPDATE_ELEM, key(&10), vp, BPF_NOEXIST),
            -(EEXIST as i64)
        );
        assert_eq!(elem(fd, MAP_UPDATE_ELEM, key(&40), vp, 0), 0);
        assert_eq!(elem(fd, MAP_UPDATE_ELEM, key(&50), vp, 0), -(E2BIG as i64));
        let mut out = 0u32;
        let op = (&raw mut out).cast::<u8>();
        assert_eq!(elem(fd, MAP_LOOKUP_ELEM, key(&20), op, 0), 0);
        assert_eq!(out, 7);
        assert_eq!(
            elem(fd, MAP_DELETE_ELEM, key(&20), std::ptr::null_mut(), 0),
            0
        );
        assert_eq!(elem(fd, MAP_LOOKUP_ELEM, key(&20), op, 0), -(ENOENT as i64));
        // Iteration from a missing key starts over and visits the rest.
        let (mut seen, mut k, mut next) = (Vec::new(), 999u64, 0u64);
        while elem(fd, MAP_GET_NEXT_KEY, key(&k), (&raw mut next).cast(), 0) == 0 {
            seen.push(next);
            k = next;
        }
        seen.sort();
        assert_eq!(seen, [10, 30, 40]);
        let mut info = [0u8; 80];
        let mut a = [0u32; 6];
        a[0] = fd as u32;
        a[1] = 80;
        a[2..4].copy_from_slice(&ptr(info.as_mut_ptr()));
        assert_eq!(call(OBJ_GET_INFO_BY_FD, &mut a), 0);
        assert_eq!(a[1], 80);
        let word = |i: usize| u32::from_ne_bytes(info[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([word(0), word(2), word(3), word(4)], [HASH, 8, 4, 4]);
        // SAFETY: closing the map fd.
        unsafe { libc::close(fd as i32) };
    }

    #[test]
    fn arrays_have_every_index_and_no_deletes() {
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[PERCPU_ARRAY, 4, 8, 2, 0]);
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0);
        let mut value = vec![0u64; ncpu()];
        let vp = value.as_mut_ptr().cast::<u8>();
        let (one, two) = (1u32, 2u32);
        assert_eq!(elem(fd, MAP_LOOKUP_ELEM, (&raw const one).cast(), vp, 0), 0);
        assert!(value.iter().all(|&v| v == 0));
        value[0] = 5;
        assert_eq!(elem(fd, MAP_UPDATE_ELEM, (&raw const one).cast(), vp, 0), 0);
        value[0] = 0;
        assert_eq!(elem(fd, MAP_LOOKUP_ELEM, (&raw const one).cast(), vp, 0), 0);
        assert_eq!(value[0], 5);
        assert_eq!(
            elem(fd, MAP_DELETE_ELEM, (&raw const one).cast(), vp, 0),
            -(EINVAL as i64)
        );
        assert_eq!(
            elem(fd, MAP_LOOKUP_ELEM, (&raw const two).cast(), vp, 0),
            -(ENOENT as i64)
        );
        // SAFETY: closing the map fd.
        unsafe { libc::close(fd as i32) };
    }
}
