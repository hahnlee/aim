//! `bpf(2)`: shared maps and cgroup socket program execution (ADR 0012).
//!
//! Android's network stack keeps its state in eBPF maps that the loader
//! (NetBpfLoad) creates and pins under `/sys/fs/bpf`, and that netd,
//! system_server (BpfNetMaps, TrafficStats) and the DnsResolver open and
//! read. Attached cgroup create/bind programs run in the socket owner;
//! traffic counters are filled there too. Other hooks remain #2268.
//!
//! Every map and program is a file in the boot's runtime directory
//! (`<runtime>/bpf-objects`); a pin is an empty file of its own in the bpffs
//! area of the path map that names it in a host attribute the guest cannot
//! set (`bpf_object::pin`), and `BPF_OBJ_GET` opens the object it names, so
//! any process that holds the fd (by fork, SCM_RIGHTS or binder) reaches the
//! same object. As bpffs, the area makes no other files (`fs::openat`). A map
//! file is its data (at offset 0, so a ring buffer's mmap sees the kernel
//! layout) followed by a header (`aim_storage::bpf_object`, which host
//! services read too); processes map it shared and serialize updates with
//! a spin lock in the header.
//!
//! Supported map types: arrays and hashes, with their per-CPU and LRU
//! variants (an LRU hash fails with E2BIG when full), maps of programs,
//! maps and devices (holding what userspace stored), LPM tries as
//! exact-match hashes, and ring buffers for mmap (no producer ever writes
//! them).

use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use super::fdtab;
use crate::errno::{self, E2BIG, EBADF, EEXIST, EFAULT, EINVAL, ENOENT, EPERM};
use crate::vfs::{self, LINUX_AT_FDCWD};
use aim_storage::bpf_object::*;
use aim_storage::private_fd::PrivateFile;
use std::os::fd::{AsRawFd, FromRawFd};

mod sock_create;
pub(crate) use sock_create::check as socket_create;
pub(crate) use sock_create::{bind as socket_bind, membership as socket_cgroup};

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

/// Maps mapped into this process, by file.
static MAPPED: Mutex<Option<HashMap<(u64, u64), Map>>> = Mutex::new(None);

/// Where the object files of this boot live.
pub(super) fn objects_dir() -> PathBuf {
    let dir = match vfs::runtime_dir() {
        Some(r) => r.join(OBJECTS),
        None => std::env::temp_dir().join(format!("linux-abi-bpf-{}", unsafe { libc::getpid() })),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A new object file of `data_len` bytes plus the header; returns its fd,
/// open for `access` (`O_RDONLY`, `O_WRONLY` or `O_RDWR`).
fn create_object(mut header: Header, data_len: usize, initial: Option<&[u8]>, access: i32) -> i64 {
    let Ok(tmpl) = CString::new(objects_dir().join("obj.XXXXXX").as_os_str().as_bytes()) else {
        return -(EINVAL as i64);
    };
    let mut tmpl = tmpl.into_bytes_with_nul();
    let mut created = false;
    let file = match PrivateFile::allocate(|| {
        let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        created = true;
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }) {
        Ok(file) => file,
        Err(error) => {
            if created {
                unsafe {
                    libc::unlink(tmpl.as_ptr().cast());
                }
            }
            return -(errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)) as i64);
        }
    };
    let object = std::path::Path::new(std::ffi::OsStr::from_bytes(&tmpl[..tmpl.len() - 1]));
    let result = (|| {
        let fd = file.as_raw_fd();
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
            || unsafe { libc::fchflags(fd, MARK) } < 0
        {
            return Err(errno::last());
        }
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } < 0 {
            return Err(errno::last());
        }
        header.id = stat.st_ino as u32;
        header.data_len = data_len as u64;
        let length = data_len
            .checked_add(HEADER)
            .filter(|length| *length <= i64::MAX as usize)
            .ok_or(E2BIG)?;
        if unsafe { libc::ftruncate(fd, length as i64) } < 0 {
            return Err(errno::last());
        }
        let count =
            unsafe { libc::pwrite(fd, (&raw const header).cast(), HEADER, data_len as i64) };
        if count != HEADER as isize {
            return Err(if count < 0 { errno::last() } else { errno::EIO });
        }
        if let Some(bytes) = initial {
            let count = unsafe { libc::pwrite(fd, bytes.as_ptr().cast(), bytes.len(), 0) };
            if count != bytes.len() as isize {
                return Err(if count < 0 { errno::last() } else { errno::EIO });
            }
        }
        make_pin_template(object).map_err(io_errno)?;
        if access == libc::O_RDWR {
            return Ok(publish_object(fd));
        }
        let narrowed = PrivateFile::allocate(|| open_object(object, access)).map_err(io_errno)?;
        Ok(publish_object(narrowed.as_raw_fd()))
    })();
    let result = result.unwrap_or_else(|error| -(error as i64));
    if result < 0 {
        remove_pin_template(object);
        unsafe {
            libc::unlink(tmpl.as_ptr().cast());
        }
    }
    result
}

fn io_errno(error: std::io::Error) -> errno::Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}

/// Object file `path`, opened for `access`.
fn open_object(path: &std::path::Path, access: i32) -> std::io::Result<std::fs::File> {
    std::fs::File::options()
        .read(access != libc::O_WRONLY)
        .write(access != libc::O_RDONLY)
        .open(path)
}

/// The access (`O_RDONLY`, `O_WRONLY`, `O_RDWR`) of object fd `fd`: what
/// the kernel keeps as the file's `f_mode`.
fn access_of(fd: i32) -> i32 {
    (unsafe { libc::fcntl(fd, libc::F_GETFL) }) & libc::O_ACCMODE
}

/// `bpf_get_file_flag`: the access `BPF_F_RDONLY` or `BPF_F_WRONLY` asks.
fn file_access(flags: u32) -> Result<i32, i64> {
    match flags & (F_RDONLY | F_WRONLY) {
        F_RDONLY => Ok(libc::O_RDONLY),
        F_WRONLY => Ok(libc::O_WRONLY),
        0 => Ok(libc::O_RDWR),
        _ => Err(-(EINVAL as i64)),
    }
}
/// `st_flags` of an object file: neither ashmem's, a memfd's nor a
/// dma-buf's, which all have UF_NODUMP.
const MARK: u32 = libc::UF_HIDDEN | libc::UF_OPAQUE;

/// Duplicate a fully initialized private description into a new guest
/// slot, an object's (`fdtab::Kind::Bpf`) before the guest can reach it.
fn publish_object(private: i32) -> i64 {
    publish_object_with(private, fdtab::publish_typed_guest)
}
fn publish_object_with(private: i32, publish: impl FnOnce(i32) -> Result<(), errno::Errno>) -> i64 {
    let _guard = fdtab::lifecycle();
    let fd = unsafe { libc::fcntl(private, libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    fdtab::insert(fd, fdtab::Kind::Bpf);
    match publish(fd) {
        Ok(()) => fd as i64,
        Err(error) => {
            fdtab::on_close(fd);
            unsafe {
                libc::close(fd);
            }
            -(error as i64)
        }
    }
}

/// An object's fd that arrived (by exec, binder or `SCM_RIGHTS`) as a
/// plain fd.
pub(super) fn adopt(fd: i32) {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } == 0
        && st.st_mode & libc::S_IFMT == libc::S_IFREG
        && st.st_flags & (MARK | libc::UF_NODUMP) == MARK
        && is_object(fd)
    {
        fdtab::insert(fd, fdtab::Kind::Bpf);
    }
}

/// `BPF_F_MMAPABLE`.
const F_MMAPABLE: u32 = 1 << 10;
/// The kernel's ENOTSUPP, which `bpf_map_mmap` returns for maps without
/// an mmap.
const ENOTSUPP: i64 = 524;

/// `bpf_map_mmap`: the pieces of object file `fd` that a guest mmap of
/// `len` bytes (whole pages) at offset `offset` maps, in order, as (file
/// offset, length). Only an mmapable array's values and a ring buffer's
/// position pages and data are mapped, never the header after them; a ring
/// buffer's data follows itself again, as the kernel maps it twice so that
/// a record that wraps reads whole.
pub(super) fn mmap_pieces(
    fd: i32,
    offset: u64,
    len: u64,
    write: bool,
    shared: bool,
) -> Result<Vec<(u64, u64)>, i64> {
    // `do_mmap`: the file's access, then whether it maps at all.
    let access = access_of(fd);
    if access == libc::O_WRONLY || shared && write && access == libc::O_RDONLY {
        return Err(-(errno::EACCES as i64));
    }
    let (h, st) = read_header(fd).ok_or(-(EBADF as i64))?;
    if h.kind != KIND_MAP {
        return Err(-(errno::ENODEV as i64));
    }
    let map = mapped(fd)?;
    let p = map.params();
    if !matches!(p.ty, ARRAY | RINGBUF) {
        return Err(-ENOTSUPP);
    }
    if !shared {
        return Err(-(EINVAL as i64));
    }
    if write && map.frozen() {
        return Err(-(EPERM as i64));
    }
    if write && p.flags & F_RDONLY_PROG != 0 {
        return Err(-(errno::EACCES as i64));
    }
    let data = (st.st_size as u64).saturating_sub(HEADER as u64);
    let end = offset.checked_add(len).ok_or(-(EINVAL as i64))?;
    if p.ty == ARRAY {
        // `array_map_mmap`.
        if p.flags & F_MMAPABLE == 0 || end > data {
            return Err(-(EINVAL as i64));
        }
        return Ok(vec![(offset, len)]);
    }
    // `ringbuf_map_mmap_kern`: only the consumer page is writable.
    if write && (offset != 0 || len != PAGE as u64) {
        return Err(-(EPERM as i64));
    }
    let size = p.max_entries as u64;
    if end > data + size {
        return Err(-(EINVAL as i64));
    }
    let again = offset.max(data);
    let mut pieces = Vec::new();
    if offset < data {
        pieces.push((offset, end.min(data) - offset));
    }
    if end > data {
        pieces.push((again - size, end - again));
    }
    Ok(pieces)
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
    // A write-only object fd reads through the object's own file.
    let reader = if access_of(fd) == libc::O_WRONLY {
        Some(open_object(&objects_dir().join(object_name(fd)?), libc::O_RDONLY).ok()?)
    } else {
        None
    };
    let from = reader.as_ref().map_or(fd, |file| file.as_raw_fd());
    // SAFETY: reading into the local header.
    if unsafe { libc::pread(from, (&raw mut h).cast(), HEADER, at) } != HEADER as isize
        || h.magic != MAGIC
    {
        return None;
    }
    Some((h, st))
}

/// Run `f` on the map behind `fd`.
fn with_map<R>(fd: i32, f: impl FnOnce(&Map) -> Result<R, i64>) -> Result<R, i64> {
    f(&mapped(fd)?)
}

/// The map behind `fd`, mapped into this process once.
fn mapped(fd: i32) -> Result<Map, i64> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return Err(-(EBADF as i64));
    }
    // Object files are never removed during a boot, so a mapped one is
    // known by its inode alone.
    let key = (st.st_dev as u32 as u64, st.st_ino);
    let mut g = MAPPED.lock().unwrap();
    let maps = g.get_or_insert_with(HashMap::new);
    let map = match maps.entry(key) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => {
            let (h, st) = read_header(fd).ok_or(-(EBADF as i64))?;
            if h.kind != KIND_MAP {
                return Err(-(EINVAL as i64));
            }
            let len = st.st_size as usize;
            // The layer reads and writes the map whatever `fd`'s access.
            let writable = if access_of(fd) == libc::O_RDWR {
                None
            } else {
                let name = object_name(fd).ok_or(-(EBADF as i64))?;
                Some(
                    open_object(&objects_dir().join(name), libc::O_RDWR)
                        .map_err(|e| -(io_errno(e) as i64))?,
                )
            };
            // SAFETY: a shared mapping of the whole object file.
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    writable.as_ref().map_or(fd, |file| file.as_raw_fd()),
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(-(errno::last() as i64));
            }
            // The layer reaches the elements its header lays out: only
            // inside the file.
            // SAFETY: the mapping of the whole file, kept for the process.
            let Some(map) = (unsafe { Map::from_raw(base as u64, len) }) else {
                // SAFETY: the mapping made above.
                unsafe { libc::munmap(base, len) };
                return Err(-(EINVAL as i64));
            };
            e.insert(map)
        }
    };
    Ok(*map)
}

/// A map as the kernel's programs reach it: what `bpf_map_lookup_elem`
/// and `bpf_map_update_elem` do from a program, for the effects the layer
/// provides in their place (`net::traffic`). Programs write frozen maps,
/// and see one CPU's value of a per-CPU map (the first's, here).
#[derive(Clone, Copy)]
pub(super) struct Kernel(Map);

impl Kernel {
    /// The map pinned at guest `path`, if there is one.
    pub(super) fn pinned(path: &[u8]) -> Option<Kernel> {
        let r = vfs::resolve(LINUX_AT_FDCWD, path, true).ok()?;
        let host = std::path::Path::new(std::ffi::OsStr::from_bytes(r.host.as_bytes()));
        let path = object(host, &objects_dir()).ok()?;
        let file = PrivateFile::allocate(|| open_object(&path, libc::O_RDWR)).ok()?;
        Kernel::of(file.as_raw_fd())
    }

    /// The map behind `fd`, if it is one.
    pub(super) fn of(fd: i32) -> Option<Kernel> {
        mapped(fd).ok().map(Kernel)
    }

    /// The value of `key`.
    pub(super) fn lookup(&self, key: &[u8]) -> Option<Vec<u8>> {
        let m = &self.0;
        if key.len() != m.params().key_size as usize {
            return None;
        }
        let _g = m.lock();
        let v = m.find(key)?;
        // SAFETY: the value in the mapping.
        Some(unsafe { std::slice::from_raw_parts(v, m.params().value_size as usize) }.to_vec())
    }

    /// Run `f` on the value of `key`, created zeroed (BPF_NOEXIST) when the
    /// map has no such key and room for it. False: no such element.
    pub(super) fn update(&self, key: &[u8], f: impl FnOnce(&mut [u8])) -> bool {
        let m = &self.0;
        if key.len() != m.params().key_size as usize {
            return false;
        }
        let _g = m.lock();
        let v = if hashed(m.params().ty) {
            match m.probe(key) {
                Ok(i) => m.value_of(i),
                Err(_) => match m.claim(key, BPF_NOEXIST) {
                    Ok(i) => {
                        let v = m.value_of(i);
                        // SAFETY: the new slot's value.
                        unsafe { std::ptr::write_bytes(v, 0, m.value_len()) };
                        v
                    }
                    Err(_) => return false,
                },
            }
        } else {
            match m.find(key) {
                Some(v) => v,
                None => return false,
            }
        };
        // SAFETY: the value in the mapping, under the map's lock.
        f(unsafe { std::slice::from_raw_parts_mut(v, m.params().value_size as usize) });
        true
    }
}

/// Copy `len` guest bytes at `p`.
fn guest_bytes(p: u64, len: usize) -> Result<Vec<u8>, i64> {
    if p == 0 && len != 0 {
        return Err(-(EFAULT as i64));
    }
    // SAFETY: guest buffer of `len` bytes.
    Ok(user_try!(crate::sys::user_memory::read_exact(p, len)))
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
    // The elements, in whole pages.
    let Some(data_len) = elements_len(ty, key_size, value_size, max_entries)
        .and_then(|len| len.checked_next_multiple_of(PAGE))
    else {
        return -(EINVAL as i64);
    };
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
    // `map_create`: the fd's access is what the map's flags ask.
    match file_access(flags) {
        Ok(access) => create_object(header, data_len, None, access),
        Err(error) => error,
    }
}

fn elem(cmd: u64, a: &[u64]) -> i64 {
    let fd = a[0] as u32 as i32;
    let (key_p, value_p, flags) = (a[1], a[2], a[3]);
    let access = access_of(fd);
    let r = with_map(fd, |m| {
        let h = m.params();
        // `map_get_sys_perms`: a lookup needs a readable fd, a change a
        // writable one, and a frozen map writes through none.
        let permitted = match cmd {
            MAP_LOOKUP_ELEM | MAP_GET_NEXT_KEY => access != libc::O_WRONLY,
            _ => access != libc::O_RDONLY && !m.frozen(),
        };
        if !permitted {
            return Err(-(EPERM as i64));
        }
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
                user_try!(unsafe {
                    crate::sys::user_memory::copy_bytes(v as *const u8, value_p as *mut u8, vlen)
                });
                Ok(0)
            }
            MAP_UPDATE_ELEM => {
                let key = key?;
                let value = guest_bytes(value_p, vlen)?;
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
                let slot = m.claim(&key, flags).map_err(|e| match e {
                    NoSlot::Exists => -(EEXIST as i64),
                    NoSlot::Missing => -(ENOENT as i64),
                    NoSlot::Full => -(E2BIG as i64),
                })?;
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
                m.remove(i);
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
                {
                    user_try!(unsafe {
                        crate::sys::user_memory::copy_bytes(
                            next.as_ptr() as *const u8,
                            value_p as *mut u8,
                            next.len(),
                        )
                    })
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
    let data = match super::user_memory::read_exact(btf, size as usize) {
        Ok(data) => data,
        Err(error) => return -(error as i64),
    };
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
    create_object(header, size as usize, Some(&data), libc::O_RDWR)
}

fn prog_load(a: &[u32]) -> i64 {
    // prog_type, insn_cnt, insns (u64), license (u64), log_level, log_size,
    // log_buf (u64), kern_version, prog_flags, prog_name[16].
    let mut name = [0u8; 16];
    for (i, w) in a[12..16].iter().enumerate() {
        name[i * 4..i * 4 + 4].copy_from_slice(&w.to_ne_bytes());
    }
    let data = if matches!(a[0], 9 | 18) {
        match sock_create::prepare(a) {
            Ok(data) => data,
            Err(error) => return error,
        }
    } else {
        Vec::new()
    };
    let mut pad = [0; 56];
    pad[..4].copy_from_slice(&a[17].to_ne_bytes());
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
        _pad: pad,
    };
    create_object(header, data.len(), Some(&data), libc::O_RDWR)
}

/// The host path of the object behind `fd`.
fn object_path(fd: i32) -> Option<CString> {
    use std::os::unix::fs::MetadataExt;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return None;
    }
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } == 0 {
        let n = buf.iter().position(|&c| c == 0)?;
        let path = std::ffi::OsStr::from_bytes(&buf[..n]);
        if let Ok(metadata) = std::fs::metadata(path)
            && metadata.ino() == stat.st_ino
            && metadata.dev() == stat.st_dev as u32 as u64
        {
            return CString::new(&buf[..n]).ok();
        }
    }
    // Darwin may cache the last hard-link name on the vnode. Unpin/detach
    // can remove or replace that name while the boot-owned object still lives.
    for entry in std::fs::read_dir(objects_dir()).ok()? {
        let entry = entry.ok()?;
        let metadata = entry.metadata().ok()?;
        if metadata.ino() == stat.st_ino && metadata.dev() == stat.st_dev as u32 as u64 {
            return CString::new(entry.path().as_os_str().as_bytes()).ok();
        }
    }
    None
}

/// Whether `fd` is an object's (a map's, program's or type
/// information's): a file the objects directory names, where all of an
/// object's names are. Linux's are anon inodes, on no mount a path is on,
/// that no open reopens (an inode's default `no_open_fops`: ENXIO).
pub(crate) fn is_object(fd: i32) -> bool {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } != 0 {
        return false;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(&buf[..n]));
    objects_dir()
        .canonicalize()
        .is_ok_and(|dir| path.starts_with(dir))
}

/// The name in the objects directory of the object behind `fd` (a
/// program's attachments are other names of its file).
fn object_name(fd: i32) -> Option<std::ffi::OsString> {
    use std::os::unix::fs::MetadataExt;
    let path = object_path(fd)?;
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(path.as_bytes()));
    let (dev, ino) = std::fs::metadata(path).map(|m| (m.dev(), m.ino())).ok()?;
    let same = |name: &std::ffi::OsStr| {
        std::fs::metadata(objects_dir().join(name)).is_ok_and(|m| m.dev() == dev && m.ino() == ino)
    };
    if let Some(name) = path.file_name().filter(|name| same(name)) {
        return Some(name.to_os_string());
    }
    std::fs::read_dir(objects_dir())
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .find(|name| same(name))
}

/// `BPF_F_RDONLY`, `BPF_F_WRONLY`, `BPF_F_PATH_FD`.
const F_RDONLY: u32 = 1 << 3;
const F_WRONLY: u32 = 1 << 4;
const F_PATH_FD: u32 = 1 << 14;

/// The directory `path` is relative to (`attr.path_fd` with
/// `BPF_F_PATH_FD`, else the working directory), as `bpf_obj_pin` and
/// `bpf_obj_get` take it.
fn path_dirfd(file_flags: u32, path_fd: u32) -> Result<i32, i64> {
    if file_flags & F_PATH_FD == 0 {
        return if path_fd != 0 {
            Err(-(EINVAL as i64))
        } else {
            Ok(LINUX_AT_FDCWD)
        };
    }
    Ok(path_fd as i32)
}

/// `bpf_obj_pin_user`: a new pin at guest `path`, which the caller must be
/// able to create in its directory (`vfs_mkobj`'s `may_create`).
fn obj_pin(path: u64, fd: i32, file_flags: u32, path_fd: u32) -> i64 {
    if file_flags & !F_PATH_FD != 0 {
        return -(EINVAL as i64);
    }
    let dirfd = match path_dirfd(file_flags, path_fd) {
        Ok(dirfd) => dirfd,
        Err(error) => return error,
    };
    if read_header(fd).is_none() {
        return -(EBADF as i64);
    }
    // SAFETY: guest string.
    let owned_guest = match super::guest_cstr(path) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    let id = super::cred::current();
    let r = match vfs::resolve_checked_new(dirfd, owned_guest.as_slice(), false, |dir, known| {
        super::attrs::search(dir, &id, super::attrs::FS, known)
    }) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    // `bpf_obj_do_pin`: only into bpffs.
    if vfs::fstype(&r.guest).as_deref() != Some("bpf") {
        return -(EPERM as i64);
    }
    let parent = if super::attrs::recording() {
        let parent = r
            .guest
            .rsplit_once('/')
            .map_or("/", |(p, _)| if p.is_empty() { "/" } else { p });
        match super::attrs::path_stat(parent) {
            Ok(st) if super::attrs::permits(&st, 3, &id, super::attrs::FS) => Some(st),
            Ok(_) => return -(errno::EACCES as i64),
            Err(error) => return -(error as i64),
        }
    } else {
        None
    };
    let Some(name) = object_name(fd) else {
        return -(EBADF as i64);
    };
    let host = std::path::Path::new(std::ffi::OsStr::from_bytes(r.host.as_bytes()));
    if let Err(error) = pin(host, &objects_dir().join(name)) {
        return -(io_errno(error) as i64);
    }
    // The new inode is the pinner's (its fsuid and fsgid, or a setgid
    // directory's group), mode 0600 less its umask. Unrecorded, a file
    // reads as root's, as the template's 0600.
    let owned = (|| {
        let mode = 0o600 & !super::pstate::current_umask();
        if mode != 0o600 && unsafe { libc::chmod(r.host.as_ptr(), mode as libc::mode_t) } != 0 {
            return Err(errno::last());
        }
        let Some(parent) = parent else { return Ok(()) };
        let gid = if parent.st_mode & libc::S_ISGID != 0 {
            parent.st_gid
        } else {
            id.gid[super::attrs::FS]
        };
        if id.uid[super::attrs::FS] != 0 || gid != 0 {
            super::attrs::created(
                super::attrs::Host::Path(&r.host),
                || r.guest.clone(),
                Some(mode),
            )?;
        }
        Ok(())
    })();
    match owned {
        Ok(()) => 0,
        Err(error) => {
            let _ = std::fs::remove_file(host);
            -(error as i64)
        }
    }
}

/// `bpf_obj_get_user`: the object pinned at guest `path`, which the caller
/// must be able to access as `file_flags` ask (`path_permission`), in the
/// access they ask.
fn obj_get(path: u64, file_flags: u32, path_fd: u32) -> i64 {
    if file_flags & !(F_RDONLY | F_WRONLY | F_PATH_FD) != 0 {
        return -(EINVAL as i64);
    }
    let access = match file_access(file_flags) {
        Ok(access) => access,
        Err(error) => return error,
    };
    let dirfd = match path_dirfd(file_flags, path_fd) {
        Ok(dirfd) => dirfd,
        Err(error) => return error,
    };
    let want = match access {
        libc::O_RDONLY => 4,
        libc::O_WRONLY => 2,
        _ => 6,
    };
    // SAFETY: guest string.
    let owned_guest = match super::guest_cstr(path) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    let id = super::cred::current();
    let r = match vfs::resolve_checked(dirfd, owned_guest.as_slice(), true, |dir, known| {
        super::attrs::search(dir, &id, super::attrs::FS, known)
    }) {
        Ok(r) => r,
        Err(e) => return -(e as i64),
    };
    if super::attrs::recording() {
        match super::attrs::path_stat(&r.guest) {
            Ok(st) if super::attrs::permits(&st, want, &id, super::attrs::FS) => {}
            Ok(_) => return -(errno::EACCES as i64),
            Err(error) => return -(error as i64),
        }
    }
    // `bpf_inode_type`: a pin is a bpffs inode.
    if vfs::fstype(&r.guest).as_deref() != Some("bpf") {
        return -(errno::EACCES as i64);
    }
    let host = std::path::Path::new(std::ffi::OsStr::from_bytes(r.host.as_bytes()));
    let (file, path) = match object(host, &objects_dir()).and_then(|path| {
        PrivateFile::allocate(|| open_object(&path, access)).map(|file| (file, path))
    }) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return -(errno::EACCES as i64);
        }
        Err(error) => return -(io_errno(error) as i64),
    };
    match read_header(file.as_raw_fd()) {
        Some((h, _)) if h.kind == KIND_MAP || access == libc::O_RDWR => {
            publish_object(file.as_raw_fd())
        }
        // `bpf_prog_new_fd`: a program's fd is read-write whatever the
        // flags asked: of the object opened, not what the path names by now.
        Some(_) => match PrivateFile::allocate(|| open_object(&path, libc::O_RDWR)) {
            Ok(file) => publish_object(file.as_raw_fd()),
            Err(error) => -(io_errno(error) as i64),
        },
        None => -(errno::EACCES as i64),
    }
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
    let bytes = match super::user_memory::read_exact(len_p, 4) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    let n = (u32::from_ne_bytes(bytes.try_into().unwrap()) as usize).min(used);
    if let Err(error) = super::user_memory::write_exact(info, &out[..n]) {
        return -(error as i64);
    }
    if let Err(error) = super::user_memory::write_exact(len_p, &(n as u32).to_ne_bytes()) {
        return -(error as i64);
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
    let bytes = match super::user_memory::read_exact(attr, words * 4) {
        Ok(bytes) => bytes,
        Err(error) => return -(error as i64),
    };
    // SAFETY: the guest's bpf_attr of `size` bytes.
    let u32s: Vec<u32> = (0..words.max(24))
        .map(|i| {
            if i < words {
                u32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
            } else {
                0
            }
        })
        .collect();
    let u64_at = |w: usize| u32s[w] as u64 | (u32s[w + 1] as u64) << 32;
    let fd_word = match cmd {
        MAP_LOOKUP_ELEM | MAP_UPDATE_ELEM | MAP_DELETE_ELEM | MAP_GET_NEXT_KEY
        | OBJ_GET_INFO_BY_FD | MAP_FREEZE => Some(0),
        OBJ_PIN => Some(2),
        _ => None,
    };
    let pinned = if let Some(word) = fd_word {
        match fdtab::pin_guest(u32s[word] as i32) {
            Ok(pin) => Some(pin),
            Err(error) => return -(error as i64),
        }
    } else {
        None
    };
    let object_fd = pinned.as_ref().map(|pin| pin.descriptor().as_raw_fd());
    match cmd {
        MAP_CREATE => map_create(&u32s),
        MAP_LOOKUP_ELEM | MAP_UPDATE_ELEM | MAP_DELETE_ELEM | MAP_GET_NEXT_KEY => {
            // map_fd, pad, key, value/next_key, flags
            elem(
                cmd,
                &[object_fd.unwrap() as u64, u64_at(2), u64_at(4), u64_at(6)],
            )
        }
        PROG_LOAD => prog_load(&u32s),
        // btf, btf_log_buf, btf_size
        BTF_LOAD => btf_load(u64_at(0), u32s[4]),
        // pathname, bpf_fd, file_flags, path_fd
        OBJ_PIN => obj_pin(u64_at(0), object_fd.unwrap(), u32s[3], u32s[4]),
        OBJ_GET => obj_get(u64_at(0), u32s[3], u32s[4]),
        PROG_ATTACH | PROG_DETACH if matches!(u32s[2], sock_create::ATTACH | 8 | 9) => {
            sock_create::attach(&u32s, cmd == PROG_DETACH)
                .map_or_else(|error| -(error as i64), |()| 0)
        }
        // Other hook families remain #2268.
        PROG_ATTACH | PROG_DETACH => 0,
        LINK_CREATE => -(EINVAL as i64),
        OBJ_GET_INFO_BY_FD => {
            // bpf_fd, info_len, info
            let len_p = attr + 4;
            info_by_fd(object_fd.unwrap(), len_p, u64_at(2))
        }
        MAP_FREEZE => {
            let fd = object_fd.unwrap();
            match read_header(fd) {
                // `map_freeze` needs a writable fd, which a frozen map
                // has not (`map_get_sys_perms`).
                Some((h, _))
                    if h.kind == KIND_MAP && (access_of(fd) == libc::O_RDONLY || h.frozen != 0) =>
                {
                    -(EPERM as i64)
                }
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

    #[test]
    fn returned_objects_are_guest_owned_cloexec_and_reopened_with_fresh_visibility() {
        if fdtab::isolated_kernel_test(
            "sys::bpf::tests::returned_objects_are_guest_owned_cloexec_and_reopened_with_fresh_visibility",
        ) {
            return;
        }
        crate::diag::install_signal_handlers();
        let (_view, root) = vfs::test_view();
        std::fs::create_dir_all(root.join("data/bpffs")).unwrap();
        vfs::add_mount(
            "/sys/fs/bpf",
            root.join("data/bpffs"),
            vfs::Area::Writable,
            "bpf",
            "bpf",
        )
        .unwrap();
        let pin = root.join("data/bpffs/publication-pin");
        let guest = std::ffi::CString::new("/sys/fs/bpf/publication-pin").unwrap();
        let mut map = [0u32; 24];
        map[..5].copy_from_slice(&[ARRAY, 4, 8, 2, 0]);
        let mut prog = [0u32; 24];
        prog[0] = 1;
        prog[1] = 1;
        let btf = [0x9fu8, 0xeb, 1, 0, 24, 0, 0, 0];
        let mut btf_attr = [0u32; 24];
        btf_attr[..2].copy_from_slice(&ptr(btf.as_ptr()));
        btf_attr[4] = btf.len() as u32;
        for (cmd, attr) in [
            (MAP_CREATE, &mut map),
            (PROG_LOAD, &mut prog),
            (BTF_LOAD, &mut btf_attr),
        ] {
            let fd = call(cmd, attr);
            assert!(fd >= 0, "command {cmd} returned {fd}");
            assert!(fdtab::visible(fd as i32));
            assert_eq!(
                super::super::fs::fcntl([fd as u64, 1, 0, 0, 0, 0]),
                libc::FD_CLOEXEC as i64
            );
            let mut before = [0u64; 16];
            assert_eq!(
                super::super::fs::fstat([fd as u64, before.as_mut_ptr() as u64, 0, 0, 0, 0]),
                0
            );
            let mut info = [0u8; 96];
            let mut info_attr = [0u32; 24];
            info_attr[0] = fd as u32;
            info_attr[1] = 96;
            info_attr[2..4].copy_from_slice(&ptr(info.as_mut_ptr()));
            assert_eq!(call(OBJ_GET_INFO_BY_FD, &mut info_attr), 0);
            info_attr[2..4].copy_from_slice(&ptr(1usize as *const u8));
            assert_eq!(call(OBJ_GET_INFO_BY_FD, &mut info_attr), -(EFAULT as i64));
            info_attr[2..4].copy_from_slice(&ptr(info.as_mut_ptr()));
            let held = fdtab::pin_guest(fd as i32).unwrap();
            info_attr[0] = held.descriptor().as_raw_fd() as u32;
            assert_eq!(call(OBJ_GET_INFO_BY_FD, &mut info_attr), -(EBADF as i64));
            drop(held);
            let mut pin_attr = [0u32; 24];
            pin_attr[..2].copy_from_slice(&ptr(guest.as_ptr().cast()));
            pin_attr[2] = fd as u32;
            assert_eq!(call(OBJ_PIN, &mut pin_attr), 0);
            assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
            assert!(!fdtab::visible(fd as i32));
            let mut reopen = [0u32; 24];
            reopen[..2].copy_from_slice(&ptr(guest.as_ptr().cast()));
            let reopened = call(OBJ_GET, &mut reopen);
            assert!(reopened >= 0);
            assert!(fdtab::visible(reopened as i32));
            let mut after = [0u64; 16];
            assert_eq!(
                super::super::fs::fstat([reopened as u64, after.as_mut_ptr() as u64, 0, 0, 0, 0]),
                0
            );
            assert_eq!(&before[..2], &after[..2]);
            assert_eq!(
                super::super::fs::fcntl([reopened as u64, 1, 0, 0, 0, 0]),
                libc::FD_CLOEXEC as i64
            );
            assert_eq!(super::super::fs::close([reopened as u64, 0, 0, 0, 0, 0]), 0);
            std::fs::remove_file(&pin).unwrap();
        }
        let mut unsupported = [0u32; 24];
        assert_eq!(call(LINK_CREATE, &mut unsupported), -(EINVAL as i64));
        let private = PrivateFile::allocate(|| std::fs::File::open("/dev/null")).unwrap();
        let mut rejected = -1;
        assert_eq!(
            publish_object_with(private.as_raw_fd(), |fd| {
                rejected = fd;
                super::super::fd_visibility::hide(fd)?;
                fdtab::publish_typed_guest(fd)
            }),
            -(EBADF as i64)
        );
        assert_eq!(unsafe { libc::fcntl(rejected, libc::F_GETFD) }, -1);
        assert_eq!(errno::last(), EBADF);
        super::super::fd_visibility::private_closed(rejected);
        assert!(
            unsafe { libc::fcntl(private.as_raw_fd(), libc::F_GETFD) } >= 0,
            "failed guest publication must retain private owner"
        );
    }

    /// bpffs: only BPF_OBJ_PIN makes a file there, and only there; a pin
    /// opens as no file, after `may_open`'s checks; neither a pin leaves
    /// bpffs nor a file enters it; BPF_OBJ_GET checks its flags, the
    /// caller's access to the pin and that it is one, and opens a map in
    /// the access asked; a pin is its pinner's.
    #[test]
    fn bpffs_holds_only_pins_and_gets_check_flags_and_access() {
        if fdtab::isolated_kernel_test(
            "sys::bpf::tests::bpffs_holds_only_pins_and_gets_check_flags_and_access",
        ) {
            return;
        }
        crate::diag::install_signal_handlers();
        let (_view, root) = vfs::test_view();
        let bpffs = root.join("data/bpffs");
        std::fs::create_dir_all(&bpffs).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bpffs, std::fs::Permissions::from_mode(0o1777)).unwrap();
        vfs::add_mount(
            "/sys/fs/bpf",
            bpffs.clone(),
            vfs::Area::Writable,
            "bpf",
            "bpf",
        )
        .unwrap();
        use super::super::fs::{O_CREAT, O_PATH, close, openat};
        let at = LINUX_AT_FDCWD as u64;
        let open = |path: &std::ffi::CStr, flags: u64| {
            openat([at, path.as_ptr() as u64, flags, 0o600, 0, 0])
        };
        let shut = |fd: i64| assert_eq!(close([fd as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(
            open(c"/sys/fs/bpf/made", O_CREAT | 1),
            -(errno::EACCES as i64)
        );
        let create = |flags: u32| {
            let mut map = [0u32; 24];
            map[..5].copy_from_slice(&[ARRAY, 4, 8, 2, flags]);
            call(MAP_CREATE, &mut map)
        };
        let fd = create(0);
        assert!(fd >= 0);
        let pin_at = |path: &std::ffi::CStr, fd: i64| {
            let mut attr = [0u32; 24];
            attr[..2].copy_from_slice(&ptr(path.as_ptr().cast()));
            attr[2] = fd as u32;
            call(OBJ_PIN, &mut attr)
        };
        assert_eq!(pin_at(c"/data/pinned", fd), -(EPERM as i64));
        assert_eq!(pin_at(c"/sys/fs/bpf/map", fd), 0);
        assert_eq!(pin_at(c"/sys/fs/bpf/map", fd), -(EEXIST as i64));
        // `may_open`'s order: EEXIST, ENOTDIR, then the open's EIO.
        assert_eq!(open(c"/sys/fs/bpf/map", O_CREAT | 0o200), -(EEXIST as i64));
        assert_eq!(open(c"/sys/fs/bpf/map", 0o40000), -(errno::ENOTDIR as i64));
        assert_eq!(open(c"/sys/fs/bpf/map", 0), -(errno::EIO as i64));
        let path = open(c"/sys/fs/bpf/map", O_PATH);
        assert!(path >= 0, "O_PATH: {path}");
        shut(path);
        // A pin neither leaves bpffs nor a file enters it.
        let link = super::super::fsops::linkat([
            at,
            c"/sys/fs/bpf/map".as_ptr() as u64,
            at,
            c"/data/linked".as_ptr() as u64,
            0,
            0,
        ]);
        assert_eq!(link, -(errno::EXDEV as i64));
        // Neither by its open file (AT_EMPTY_PATH, or its /proc/self/fd
        // link), nor does an object fd (an anon inode) take a name.
        let link_fd = |fd: i64, path: &std::ffi::CStr| {
            super::super::fsops::linkat([
                fd as u64,
                c"".as_ptr() as u64,
                at,
                path.as_ptr() as u64,
                super::super::fs::AT_EMPTY_PATH,
                0,
            ])
        };
        let path = open(c"/sys/fs/bpf/map", O_PATH);
        assert!(path >= 0, "O_PATH: {path}");
        assert_eq!(link_fd(path, c"/data/linked"), -(errno::EXDEV as i64));
        assert_eq!(link_fd(path, c"/sys/fs/bpf/map"), -(EEXIST as i64));
        assert_eq!(link_fd(path, c"/sys/fs/bpf/again"), 0);
        assert!(bpffs.join("again").exists());
        shut(path);
        assert_eq!(link_fd(fd, c"/data/object"), -(errno::EXDEV as i64));
        assert_eq!(link_fd(fd, c"/sys/fs/bpf/object"), -(errno::EXDEV as i64));
        assert!(!root.join("data/object").exists());
        let proc_host = root.join("run/proc-bpf");
        std::fs::create_dir_all(&proc_host).unwrap();
        vfs::add_mount(
            "/proc",
            proc_host.canonicalize().unwrap(),
            vfs::Area::Kernfs,
            "proc",
            "proc",
        )
        .unwrap();
        let magic = std::ffi::CString::new(format!("/proc/self/fd/{fd}")).unwrap();
        let link = super::super::fsops::linkat([
            at,
            magic.as_ptr() as u64,
            at,
            c"/data/object".as_ptr() as u64,
            0x400,
            0,
        ]);
        assert_eq!(link, -(errno::EXDEV as i64));
        let rename = super::super::fsops::renameat2([
            at,
            c"/sys/fs/bpf/map".as_ptr() as u64,
            at,
            c"/data/renamed".as_ptr() as u64,
            0,
            0,
        ]);
        assert_eq!(rename, -(errno::EXDEV as i64));
        let get = |path: &std::ffi::CStr, flags: u32, path_fd: u32| {
            let mut attr = [0u32; 24];
            attr[..2].copy_from_slice(&ptr(path.as_ptr().cast()));
            attr[3] = flags;
            attr[4] = path_fd;
            call(OBJ_GET, &mut attr)
        };
        std::fs::write(bpffs.join("forged"), b"").unwrap();
        assert_eq!(get(c"/sys/fs/bpf/forged", 0, 0), -(errno::EACCES as i64));
        // A pin moved out of bpffs on the host is no bpffs inode.
        std::fs::hard_link(bpffs.join("map"), root.join("data/outside")).unwrap();
        assert_eq!(get(c"/data/outside", 0, 0), -(errno::EACCES as i64));
        for (flags, path_fd) in [(F_RDONLY | F_WRONLY, 0), (1 << 20, 0), (0, 5)] {
            assert_eq!(get(c"/sys/fs/bpf/map", flags, path_fd), -(EINVAL as i64));
        }
        // The access asked is the fd's: `map_get_sys_perms`.
        let (key, mut value) = (0u32.to_ne_bytes(), [0u8; 8]);
        let read = get(c"/sys/fs/bpf/map", F_RDONLY, 0);
        assert!(read >= 0, "get: {read}");
        assert_eq!(
            elem(read, MAP_LOOKUP_ELEM, key.as_ptr(), value.as_mut_ptr(), 0),
            0
        );
        assert_eq!(
            elem(read, MAP_UPDATE_ELEM, key.as_ptr(), value.as_mut_ptr(), 0),
            -(EPERM as i64)
        );
        // Nor does its /proc/self/fd link reopen it, in any access.
        for flags in [0, 2] {
            let magic = std::ffi::CString::new(format!("/proc/self/fd/{read}")).unwrap();
            assert_eq!(open(&magic, flags), -(errno::ENXIO as i64));
        }
        let write = get(c"/sys/fs/bpf/map", F_WRONLY, 0);
        assert!(write >= 0, "get: {write}");
        assert_eq!(
            elem(write, MAP_UPDATE_ELEM, key.as_ptr(), value.as_mut_ptr(), 0),
            0
        );
        assert_eq!(
            elem(write, MAP_LOOKUP_ELEM, key.as_ptr(), value.as_mut_ptr(), 0),
            -(EPERM as i64)
        );
        let mut info = [0u8; 96];
        let mut info_attr = [0u32; 24];
        info_attr[0] = write as u32;
        info_attr[1] = 96;
        info_attr[2..4].copy_from_slice(&ptr(info.as_mut_ptr()));
        assert_eq!(call(OBJ_GET_INFO_BY_FD, &mut info_attr), 0);
        shut(read);
        shut(write);
        let created = create(F_RDONLY);
        assert!(created >= 0);
        assert_eq!(
            elem(
                created,
                MAP_UPDATE_ELEM,
                key.as_ptr(),
                value.as_mut_ptr(),
                0
            ),
            -(EPERM as i64)
        );
        shut(created);
        assert_eq!(create(F_RDONLY | F_WRONLY), -(EINVAL as i64));
        // A program's fd is read-write whatever the flags ask.
        let mut prog = [0u32; 24];
        prog[0] = 1;
        prog[1] = 1;
        let program = call(PROG_LOAD, &mut prog);
        assert!(program >= 0);
        assert_eq!(pin_at(c"/sys/fs/bpf/prog", program), 0);
        let got = get(c"/sys/fs/bpf/prog", F_RDONLY, 0);
        assert!(got >= 0, "program: {got}");
        assert_eq!(access_of(got as i32), libc::O_RDWR);
        shut(got);
        shut(program);
        // A pin netbpfload makes 0460 root:net_bw_acct: generic_permission
        // grants its group class (read-write) to a supplementary group, and
        // CAP_DAC_OVERRIDE whatever the class.
        assert_eq!(pin_at(c"/sys/fs/bpf/netd", fd), 0);
        let netd = c"/sys/fs/bpf/netd".as_ptr() as u64;
        assert_eq!(super::super::fsops::fchmodat([at, netd, 0o460, 0, 0, 0]), 0);
        assert_eq!(super::super::fsops::fchownat([at, netd, 0, 3007, 0, 0]), 0);
        let root_identity = super::super::cred::current();
        let mut identity = root_identity.clone();
        identity.uid = [1000; 4];
        identity.gid = [1000; 4];
        identity.groups = vec![3006, 3007];
        identity.cap_eff = 0;
        identity.cap_perm = 0;
        super::super::cred::init(identity.clone(), None);
        for flags in [F_RDONLY, 0] {
            let got = get(c"/sys/fs/bpf/netd", flags, 0);
            assert!(got >= 0, "supplementary group: {got}");
            shut(got);
        }
        identity.groups = vec![3006];
        super::super::cred::init(identity.clone(), None);
        assert_eq!(
            get(c"/sys/fs/bpf/netd", F_RDONLY, 0),
            -(errno::EACCES as i64)
        );
        identity.cap_eff = 1 << 1;
        super::super::cred::init(identity, None);
        let got = get(c"/sys/fs/bpf/netd", 0, 0);
        assert!(got >= 0, "CAP_DAC_OVERRIDE: {got}");
        shut(got);
        super::super::cred::init(root_identity, None);
        // The pin is its creator's, mode 0600: another uid without
        // capabilities may not open it, and its own pin is its own, less
        // its umask.
        let mut identity = super::super::cred::current();
        identity.uid = [10123; 4];
        identity.gid = [10123; 4];
        identity.groups.clear();
        identity.cap_eff = 0;
        identity.cap_perm = 0;
        super::super::cred::init(identity, None);
        assert_eq!(
            get(c"/sys/fs/bpf/map", F_RDONLY, 0),
            -(errno::EACCES as i64)
        );
        assert_eq!(open(c"/sys/fs/bpf/map", 0), -(errno::EACCES as i64));
        super::super::pstate::umask([0o200, 0, 0, 0, 0, 0]);
        assert_eq!(pin_at(c"/sys/fs/bpf/own", fd), 0);
        let own = get(c"/sys/fs/bpf/own", F_RDONLY, 0);
        assert!(own >= 0, "own pin: {own}");
        shut(own);
        let mut stat = [0u64; 16];
        let stat_at = super::super::fs::newfstatat([
            at,
            c"/sys/fs/bpf/own".as_ptr() as u64,
            stat.as_mut_ptr() as u64,
            0,
            0,
            0,
        ]);
        assert_eq!(stat_at, 0);
        // struct stat: st_mode at 16, st_uid at 24 (arm64).
        let words: &[u8] = unsafe { std::slice::from_raw_parts(stat.as_ptr().cast(), 128) };
        let word = |at: usize| u32::from_ne_bytes(words[at..at + 4].try_into().unwrap());
        assert_eq!(word(16) & 0o7777, 0o400);
        assert_eq!(word(24), 10123);
        shut(fd);
    }

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
        // The host reader uses the same owner API on real BPF-created
        // files, including their page-aligned data area.
        let map = mapped(fd as i32).unwrap();
        assert!(map.keys().unwrap().is_empty());
        let key = |k: &u64| (k as *const u64).cast::<u8>();
        let mut v = 7u32;
        let vp = (&raw mut v).cast::<u8>();
        for k in [10u64, 20, 30] {
            assert_eq!(elem(fd, MAP_UPDATE_ELEM, key(&k), vp, 0), 0);
        }
        let mut keys = map.keys().unwrap();
        keys.sort();
        assert_eq!(keys, [10u64, 20, 30].map(|k| k.to_ne_bytes().to_vec()));
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
        let mut keys: Vec<_> = map
            .keys()
            .unwrap()
            .iter()
            .map(|k| u64::from_ne_bytes(k.as_slice().try_into().unwrap()))
            .collect();
        keys.sort();
        assert_eq!(keys, seen);
        for k in seen {
            assert_eq!(
                map.lookup(&k.to_ne_bytes()),
                Some(7u32.to_ne_bytes().to_vec())
            );
            assert_eq!(
                elem(fd, MAP_DELETE_ELEM, key(&k), std::ptr::null_mut(), 0),
                0
            );
        }
        assert!(map.keys().unwrap().is_empty());
        // A frozen map changes through no fd, before its key is read, and
        // freezes once.
        assert_eq!(elem(fd, MAP_UPDATE_ELEM, key(&10), vp, 0), 0);
        let mut freeze = [0u32; 24];
        freeze[0] = fd as u32;
        assert_eq!(call(MAP_FREEZE, &mut freeze), 0);
        assert_eq!(call(MAP_FREEZE, &mut freeze), -(EPERM as i64));
        assert_eq!(
            elem(fd, MAP_DELETE_ELEM, key(&10), std::ptr::null_mut(), 0),
            -(EPERM as i64)
        );
        let bad = 1usize as *mut u8;
        assert_eq!(elem(fd, MAP_UPDATE_ELEM, bad, bad, 0), -(EPERM as i64));
        assert_eq!(elem(fd, MAP_LOOKUP_ELEM, key(&10), op, 0), 0);
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
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    /// A map's header lays out its elements inside its file, or the map
    /// is mapped not at all.
    #[test]
    fn a_map_whose_header_overruns_its_file_is_not_mapped() {
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[ARRAY, 4, 8, 2, 0]);
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0);
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(fd as i32, &mut stat) }, 0);
        let entries =
            stat.st_size - HEADER as i64 + std::mem::offset_of!(Header, max_entries) as i64;
        let many = (1u32 << 30).to_ne_bytes();
        // SAFETY: writing the map's entry count in its header.
        assert_eq!(
            unsafe { libc::pwrite(fd as i32, many.as_ptr().cast(), 4, entries) },
            4
        );
        assert_eq!(mapped(fd as i32).err(), Some(-(EINVAL as i64)));
        let (key, mut value) = ((1u32 << 29).to_ne_bytes(), [0u8; 8]);
        assert_eq!(
            elem(fd, MAP_LOOKUP_ELEM, key.as_ptr(), value.as_mut_ptr(), 0),
            -(EINVAL as i64)
        );
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    #[test]
    fn ring_buffers_reject_key_snapshots() {
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[RINGBUF, 0, 0, 16384, 0]);
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0);
        assert_eq!(
            mapped(fd as i32)
                .unwrap()
                .keys()
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    #[test]
    fn arrays_have_every_index_and_no_deletes() {
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[PERCPU_ARRAY, 4, 8, 2, 0]);
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0);
        assert_eq!(
            mapped(fd as i32).unwrap().keys().unwrap(),
            [0u32, 1].map(|k| k.to_ne_bytes().to_vec())
        );
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
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }

    /// A map's fd is an anon inode's: no read, write, seek, truncation,
    /// allocation or attribute change reaches its file, whose header stays
    /// as the map was made, and it stays an object's fd as a plain fd.
    #[test]
    fn object_fds_reach_no_file() {
        use super::super::{fs, fsops, mem};
        const ESPIPE: i64 = 29;
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[HASH, 4, 4, 4, 0]);
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0);
        assert!(matches!(fdtab::get(fd as i32), Some(fdtab::Kind::Bpf)));
        let (_, st) = read_header(fd as i32).unwrap();
        let at = st.st_size - HEADER as i64 + std::mem::offset_of!(Header, max_entries) as i64;
        let many = (1u32 << 30).to_ne_bytes();
        let (bytes, fd) = (many.as_ptr() as u64, fd as u64);
        assert_eq!(fs::pwrite64([fd, bytes, 4, at as u64, 0, 0]), -ESPIPE);
        assert_eq!(fs::write([fd, bytes, 4, 0, 0, 0]), -(EINVAL as i64));
        let mut out = [0u8; 4];
        let out = out.as_mut_ptr() as u64;
        assert_eq!(fs::read([fd, out, 4, 0, 0, 0]), -(EINVAL as i64));
        assert_eq!(fs::pread64([fd, out, 4, 0, 0, 0]), -ESPIPE);
        assert_eq!(fs::lseek([fd, at as u64, 0, 0, 0, 0]), -ESPIPE);
        assert_eq!(fsops::ftruncate([fd, 0, 0, 0, 0, 0]), -(EINVAL as i64));
        assert_eq!(
            fsops::fallocate([fd, 0, 0, 16384, 0, 0]),
            -(errno::ENODEV as i64)
        );
        assert_eq!(
            fsops::fchmod([fd, 0, 0, 0, 0, 0]),
            -(errno::EOPNOTSUPP as i64)
        );
        assert_eq!(
            fsops::fchown([fd, 0, 0, 0, 0, 0]),
            -(errno::EOPNOTSUPP as i64)
        );
        let whole = st.st_size as u64;
        assert_eq!(mem::mmap([0, whole, 3, 1, fd, 0]), -ENOTSUPP);
        let (h, after) = read_header(fd as i32).unwrap();
        assert_eq!((h.max_entries, after.st_size), (4, st.st_size));
        let plain = unsafe { libc::fcntl(fd as i32, libc::F_DUPFD_CLOEXEC, 0) };
        assert!(plain >= 0 && fdtab::get(plain).is_none());
        fdtab::adopt(plain);
        assert!(matches!(fdtab::get(plain), Some(fdtab::Kind::Bpf)));
        fdtab::on_close(plain);
        unsafe { libc::close(plain) };
        assert_eq!(fs::close([fd, 0, 0, 0, 0, 0]), 0);
    }

    /// mmap maps an mmapable array's values and a ring buffer's pages, its
    /// data twice over, and never the header after them.
    #[test]
    fn mmap_maps_only_mappable_data() {
        use super::super::mem;
        let page = PAGE as u64;
        let context: crate::context::GuestContext = unsafe { std::mem::zeroed() };
        let unmap = |at: i64, len: u64| {
            assert_eq!(mem::munmap(&context, [at as u64, len, 0, 0, 0, 0]), 0);
        };
        let mut create = [0u32; 24];
        create[..5].copy_from_slice(&[ARRAY, 4, 8, 4, 0]);
        let plain = call(MAP_CREATE, &mut create) as u64;
        assert_eq!(mem::mmap([0, page, 1, 1, plain, 0]), -(EINVAL as i64));
        create[4] = F_MMAPABLE;
        let fd = call(MAP_CREATE, &mut create);
        assert!(fd >= 0);
        let fd = fd as u64;
        assert_eq!(mem::mmap([0, page, 3, 2, fd, 0]), -(EINVAL as i64));
        assert_eq!(mem::mmap([0, 2 * page, 1, 1, fd, 0]), -(EINVAL as i64));
        let at = mem::mmap([0, page, 3, 1, fd, 0]);
        assert!(at > 0, "{at}");
        // SAFETY: element 1 of the mapping.
        unsafe { ((at as u64 + 8) as *mut u64).write(77) };
        let (key, mut value) = (1u32, 0u64);
        assert_eq!(
            elem(
                fd as i64,
                MAP_LOOKUP_ELEM,
                (&raw const key).cast(),
                (&raw mut value).cast(),
                0
            ),
            0
        );
        assert_eq!(value, 77);
        unmap(at, page);
        let mut ring = [0u32; 24];
        ring[..5].copy_from_slice(&[RINGBUF, 0, 0, PAGE as u32, 0]);
        let rb = call(MAP_CREATE, &mut ring);
        assert!(rb >= 0);
        let rb = rb as u64;
        let consumer = mem::mmap([0, page, 3, 1, rb, 0]);
        assert!(consumer > 0, "{consumer}");
        unmap(consumer, page);
        assert_eq!(mem::mmap([0, 3 * page, 3, 1, rb, page]), -(EPERM as i64));
        assert_eq!(mem::mmap([0, 4 * page, 1, 1, rb, page]), -(EINVAL as i64));
        let mark = 0x5au8;
        // SAFETY: the first byte of the ring buffer's data.
        assert_eq!(
            unsafe { libc::pwrite(rb as i32, (&raw const mark).cast(), 1, 2 * PAGE as i64) },
            1
        );
        let producer = mem::mmap([0, 3 * page, 1, 1, rb, page]);
        assert!(producer > 0, "{producer}");
        // SAFETY: the data's two mappings.
        unsafe {
            assert_eq!(((producer as u64 + page) as *const u8).read(), mark);
            assert_eq!(((producer as u64 + 2 * page) as *const u8).read(), mark);
        }
        unmap(producer, 3 * page);
        for fd in [plain, fd, rb] {
            assert_eq!(super::super::fs::close([fd, 0, 0, 0, 0, 0]), 0);
        }
    }
}
