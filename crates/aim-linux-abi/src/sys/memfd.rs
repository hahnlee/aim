//! memfd_create and file seals.
//!
//! A memfd is a host temporary file, so read/write, ftruncate, fstat,
//! MAP_SHARED mappings, fork, dup and descriptor passing all work natively
//! and across processes.
//!
//! The memfd's state lives with the file, as ashmem's does, so every
//! process and fd that reaches it (dup, fork, exec, `SCM_RIGHTS`, binder,
//! a reopen) sees the same object:
//! - marker flags ([`MARK`]) that fstat reports at no cost, so a memfd is
//!   recognized wherever it arrives ([`adopt`]);
//! - its name in the `dev.aim.memfd` attribute;
//! - one attribute per seal (`dev.aim.memfd.seal.<bit>`), so processes
//!   adding seals at once never lose one.
//!
//! Opening `/proc/<pid>/fd/N` of a memfd opens its file again: a new open
//! file description, with its own access mode and offset, as on Linux.
//! Darwin cannot open an unlinked file, so the file keeps its name in
//! [`dir`] while an fd refers to it. Each open file description holds a
//! shared `flock` on the file; one no description holds is unreferenced,
//! and a sweep that can take the exclusive lock removes its name. Its
//! mappings, if any, stay valid. A guest `flock` on a memfd works on the
//! same lock, so `LOCK_UN` makes a memfd sweepable early (it then cannot be
//! reopened) and `LOCK_EX` conflicts with a reopened description (#258).
//!
//! Darwin refuses executable views of file-backed shared memory (ADR 0012,
//! "Platform probes"). ART's JIT maps one memfd twice, RW and RX. When a
//! memfd is first mapped (or mprotect'ed) executable in a process, its
//! contents move to anonymous memory inherited as shared by fork children;
//! every existing view is remapped onto it, and later mappings alias it with
//! `mach_vm_remap`, which gives the dual views. From then on that process's
//! reads and writes through the fd use the same memory.
//!
//! Seals: SHRINK/GROW refuse ftruncate, fallocate and an `O_TRUNC` reopen;
//! WRITE/FUTURE_WRITE refuse write, hole punching and writable shared
//! mappings. Adding WRITE while this process has a writable shared mapping
//! fails with EBUSY; a writable mapping in another process is not seen, and
//! mprotect can still make a read-only view writable (#258).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use super::fdtab::{self, Kind};
use super::vmmap;
use crate::errno::{self, EINVAL, EPERM};
use crate::sys::guest_cstr;

const MFD_CLOEXEC: u64 = 1;
const MFD_ALLOW_SEALING: u64 = 2;
const MFD_HUGETLB: u64 = 4;
const MFD_NOEXEC_SEAL: u64 = 8;
const MFD_EXEC: u64 = 0x10;

pub const F_SEAL_SEAL: u32 = 1;
pub const F_SEAL_SHRINK: u32 = 2;
pub const F_SEAL_GROW: u32 = 4;
pub const F_SEAL_WRITE: u32 = 8;
pub const F_SEAL_FUTURE_WRITE: u32 = 0x10;
pub const F_SEAL_EXEC: u32 = 0x20;
const F_ALL_SEALS: u32 = 0x3f;

const PAGE: u64 = 16384;

/// `st_flags` of a memfd file (both set). Ashmem's mark has UF_OPAQUE
/// instead of UF_HIDDEN; guest files have neither.
const MARK: u32 = libc::UF_NODUMP | libc::UF_HIDDEN;
const NAME_ATTR: &CStr = c"dev.aim.memfd";
const SEAL_ATTR: &str = "dev.aim.memfd.seal.";
/// A sweep runs at a process's first memfd_create and every this many after.
const SWEEP_EVERY: u32 = 64;
/// Files younger than this are left to their creator, which may not hold
/// its lock yet.
const SWEEP_MIN_AGE: i64 = 5;

/// (device, inode) of the backing file.
pub type Key = (u64, u64);

/// What this process knows of a memfd besides the file's state.
struct Memfd {
    name: String,
    /// Anonymous memory holding the contents once mapped executable:
    /// (base, size).
    anon: Option<(u64, u64)>,
}

static MEMFDS: Mutex<Option<HashMap<Key, Memfd>>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut HashMap<Key, Memfd>) -> T) -> T {
    f(MEMFDS.lock().unwrap().get_or_insert_with(HashMap::new))
}

fn stat_fd(fd: i32) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    (unsafe { libc::fstat(fd, &mut st) } == 0).then_some(st)
}

fn key_of(st: &libc::stat) -> Key {
    (st.st_dev as u32 as u64, st.st_ino)
}

fn marked(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG && st.st_flags & (MARK | libc::UF_OPAQUE) == MARK
}

/// fstat of `fd` when it is a memfd.
fn memfd_stat(fd: i32) -> Option<libc::stat> {
    stat_fd(fd).filter(marked)
}

/// Whether `fd` is a memfd.
pub fn is_memfd(fd: i32) -> bool {
    memfd_stat(fd).is_some()
}

/// Where memfd files are named while referenced.
fn dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join("aim-memfd");
        let _ = std::fs::create_dir_all(&d);
        d
    })
}

/// The memfd's name from its file.
fn read_name(fd: i32) -> String {
    let mut buf = [0u8; 256];
    // SAFETY: reading our attribute into a local buffer.
    let n = unsafe {
        libc::fgetxattr(
            fd,
            NAME_ATTR.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0,
        )
    };
    String::from_utf8_lossy(&buf[..n.max(0) as usize]).into_owned()
}

/// Remember a memfd this process holds, with its name.
fn know(k: Key, fd: i32) {
    if with(|m| m.contains_key(&k)) {
        return;
    }
    let name = read_name(fd);
    with(|m| {
        m.entry(k).or_insert(Memfd { name, anon: None });
    });
}

/// The memfd key of `fd`, if it is one.
pub fn key(fd: i32) -> Option<Key> {
    match fdtab::get(fd) {
        Some(Kind::Memfd(k)) => Some(k),
        _ => memfd_stat(fd).map(|st| key_of(&st)),
    }
}

/// Recognize a memfd that arrived from elsewhere (exec, `SCM_RIGHTS`,
/// binder), so its seals apply to it.
pub fn adopt(fd: i32) {
    if let Some(st) = memfd_stat(fd) {
        let k = key_of(&st);
        know(k, fd);
        fdtab::insert(fd, Kind::Memfd(k));
    }
}

/// Take this description's reference on the file.
fn hold(fd: i32) {
    // SAFETY: plain flock on our fd; a guest's exclusive lock only
    // leaves the file sweepable.
    unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
}

fn set_seals(fd: i32, seals: u32) -> i64 {
    for bit in (0..6).map(|i| 1u32 << i).filter(|b| seals & b != 0) {
        let Ok(n) = CString::new(format!("{SEAL_ATTR}{bit:x}")) else {
            continue;
        };
        // SAFETY: writing our attribute from a static value.
        if unsafe { libc::fsetxattr(fd, n.as_ptr(), c"1".as_ptr().cast(), 1, 0, 0) } < 0 {
            return -(errno::last() as i64);
        }
    }
    0
}

fn read_seals(fd: i32) -> u32 {
    let mut buf = [0u8; 1024];
    // SAFETY: listing attributes into a local buffer.
    let n = unsafe { libc::flistxattr(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
    buf[..n.max(0) as usize]
        .split(|&c| c == 0)
        .filter_map(|a| std::str::from_utf8(a).ok()?.strip_prefix(SEAL_ATTR))
        .filter_map(|b| u32::from_str_radix(b, 16).ok())
        .fold(0, |s, b| s | b)
}

/// Remove the names of memfd files in `dir` no open file description
/// refers to, unless younger than `min_age` seconds.
fn sweep(dir: &std::path::Path, min_age: i64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    for e in entries.flatten() {
        let Ok(p) = CString::new(e.path().into_os_string().into_encoded_bytes()) else {
            continue;
        };
        // SAFETY: probing a file of our directory; the name goes only when
        // the exclusive lock shows nothing holds it.
        unsafe {
            let fd = libc::open(
                p.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_EXLOCK,
            );
            if fd < 0 {
                continue;
            }
            if stat_fd(fd).is_some_and(|st| now - st.st_birthtime >= min_age) {
                libc::unlink(p.as_ptr());
            }
            libc::close(fd);
        }
    }
}

pub fn memfd_create(a: [u64; 6]) -> i64 {
    static CREATED: AtomicU32 = AtomicU32::new(0);
    let flags = a[1];
    if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_NOEXEC_SEAL | MFD_EXEC) != 0
        || flags & MFD_HUGETLB != 0
        || flags & (MFD_NOEXEC_SEAL | MFD_EXEC) == (MFD_NOEXEC_SEAL | MFD_EXEC)
    {
        return -(EINVAL as i64);
    }
    // SAFETY: guest string.
    let name = unsafe { guest_cstr(a[0]) };
    if name.len() > 249 {
        return -(EINVAL as i64);
    }
    if CREATED.fetch_add(1, Ordering::Relaxed) % SWEEP_EVERY == 0 {
        sweep(dir(), SWEEP_MIN_AGE);
    }
    let mut tmpl = dir().join("XXXXXX").into_os_string().into_encoded_bytes();
    tmpl.push(0);
    // SAFETY: mkstemp fills in the template in place.
    let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    hold(fd);
    fdtab::set_flags(fd, false, flags & MFD_CLOEXEC != 0);
    let mut seals = 0;
    if flags & (MFD_ALLOW_SEALING | MFD_NOEXEC_SEAL) == 0 {
        seals |= F_SEAL_SEAL;
    }
    if flags & MFD_NOEXEC_SEAL != 0 {
        seals |= F_SEAL_EXEC;
    }
    let mode = if flags & MFD_NOEXEC_SEAL != 0 {
        0o666
    } else {
        0o777
    };
    // SAFETY: setting up our own file.
    let r = unsafe {
        if libc::fchmod(fd, mode) < 0
            || libc::fchflags(fd, MARK) < 0
            || libc::fsetxattr(
                fd,
                NAME_ATTR.as_ptr(),
                name.as_ptr().cast(),
                name.len(),
                0,
                0,
            ) < 0
        {
            -(errno::last() as i64)
        } else {
            set_seals(fd, seals)
        }
    };
    let st = stat_fd(fd);
    let (Some(st), 0) = (st, r) else {
        // SAFETY: dropping the file we made.
        unsafe {
            libc::unlink(tmpl.as_ptr().cast());
            libc::close(fd);
        }
        return if r < 0 { r } else { -(errno::last() as i64) };
    };
    let k = key_of(&st);
    with(|m| {
        m.insert(
            k,
            Memfd {
                name: String::from_utf8_lossy(name).into_owned(),
                anon: None,
            },
        )
    });
    fdtab::insert(fd, Kind::Memfd(k));
    fd as i64
}

/// Open of `/proc/<pid>/fd/N` naming memfd `fd`: a new open file
/// description of the same file. None when `fd` is not a memfd.
pub fn reopen(fd: i32, host_flags: i32) -> Option<i64> {
    let st = memfd_stat(fd)?;
    if host_flags & libc::O_TRUNC != 0 && st.st_size > 0 && read_seals(fd) & F_SEAL_SHRINK != 0 {
        return Some(-(EPERM as i64));
    }
    let mut path = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH into a local buffer, then opening that file.
    let new = unsafe {
        if libc::fcntl(fd, libc::F_GETPATH, path.as_mut_ptr()) < 0 {
            return Some(-(errno::last() as i64));
        }
        libc::open(
            path.as_ptr().cast(),
            host_flags & !(libc::O_CREAT | libc::O_EXCL),
        )
    };
    if new < 0 {
        return Some(-(errno::last() as i64));
    }
    // A sweep may have removed a name no lock protected, and a new memfd
    // may since have taken it.
    if stat_fd(new).map(|s| key_of(&s)) != Some(key_of(&st)) {
        // SAFETY: our fd.
        unsafe { libc::close(new) };
        return Some(-(libc::ENOENT as i64));
    }
    hold(new);
    know(key_of(&st), fd);
    fdtab::insert(new, Kind::Memfd(key_of(&st)));
    Some(new as i64)
}

pub fn seals(fd: i32) -> Option<u32> {
    key(fd)?;
    Some(read_seals(fd))
}

pub fn get_seals(fd: i32) -> i64 {
    match seals(fd) {
        Some(s) => s as i64,
        None => -(EINVAL as i64),
    }
}

/// A shared writable mapping of `k` by the guest of this process.
fn mapped_writable(k: Key) -> bool {
    vmmap::shared_regions(super::arena::LO, super::arena::HI)
        .into_iter()
        .any(|r| r.prot & 2 != 0 && r.file.as_ref().is_some_and(|f| (f.1, f.2) == k))
}

/// Serializes this process's check-and-add of seals.
static SEAL_LOCK: Mutex<()> = Mutex::new(());

pub fn add_seals(fd: i32, add: u32) -> i64 {
    // SAFETY: plain fcntl.
    let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if fl < 0 {
        return -(errno::last() as i64);
    }
    if fl & libc::O_ACCMODE == libc::O_RDONLY {
        return -(EPERM as i64);
    }
    if add & !F_ALL_SEALS != 0 {
        return -(EINVAL as i64);
    }
    let Some(st) = memfd_stat(fd) else {
        return -(EINVAL as i64);
    };
    let _g = SEAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cur = read_seals(fd);
    if cur & F_SEAL_SEAL != 0 {
        return -(EPERM as i64);
    }
    if add & F_SEAL_WRITE != 0 && cur & F_SEAL_WRITE == 0 && mapped_writable(key_of(&st)) {
        return -(libc::EBUSY as i64);
    }
    let mut add = add;
    // SEAL_EXEC implies the write seals on an executable memfd (W^X).
    if add & F_SEAL_EXEC != 0 && st.st_mode & 0o111 != 0 {
        add |= F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE;
    }
    set_seals(fd, add & !cur)
}

/// Whether a write through `fd` is refused by a seal.
pub fn write_sealed(fd: i32) -> bool {
    seals(fd).is_some_and(|s| s & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0)
}

/// Whether resizing `fd` from its current size to `new` is refused.
pub fn resize_sealed(fd: i32, new: u64) -> bool {
    let Some(st) = memfd_stat(fd) else {
        return false;
    };
    let s = read_seals(fd);
    let cur = st.st_size as u64;
    (new < cur && s & F_SEAL_SHRINK != 0) || (new > cur && s & F_SEAL_GROW != 0)
}

/// `/memfd:name (deleted)` for `/proc/self/fd` of a memfd.
pub fn fd_link(fd: i32, st: &libc::stat) -> Option<String> {
    marked(st).then(|| {
        let k = key_of(st);
        know(k, fd);
        with(|m| format!("/memfd:{} (deleted)", m[&k].name))
    })
}

/// `/memfd:name (deleted)` for a mapping of a memfd in `/proc/self/maps`.
pub fn link_name(path: &std::path::Path, dev: u64, ino: u64) -> Option<String> {
    if let Some(n) = with(|m| m.get(&(dev, ino)).map(|f| f.name.clone())) {
        return Some(format!("/memfd:{n} (deleted)"));
    }
    if !path.starts_with(dir()) {
        return None;
    }
    let p = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut buf = [0u8; 256];
    // SAFETY: reading our attribute into a local buffer.
    let n = unsafe {
        libc::getxattr(
            p.as_ptr(),
            NAME_ATTR.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0,
        )
    };
    (n >= 0).then(|| {
        format!(
            "/memfd:{} (deleted)",
            String::from_utf8_lossy(&buf[..n as usize])
        )
    })
}

/// The memfd whose executable-mode memory contains `addr`.
pub fn anon_name(addr: u64) -> Option<String> {
    with(|m| {
        m.values()
            .find(|f| f.anon.is_some_and(|(b, s)| (b..b + s).contains(&addr)))
            .map(|f| format!("/memfd:{} (deleted)", f.name))
    })
}

/// Move a memfd's contents into shared anonymous memory and remap every
/// existing view of it onto that memory. `fd`, when known, supplies the
/// contents; otherwise the existing views do.
fn convert(k: Key, fd: Option<i32>, min_size: u64) -> Result<(u64, u64), i64> {
    if let Some(a) = with(|m| m.get(&k).and_then(|f| f.anon)) {
        return Ok(a);
    }
    // The guest's views, all in the guest range.
    let views: Vec<vmmap::Region> = vmmap::shared_regions(super::arena::LO, super::arena::HI)
        .into_iter()
        .filter(|r| r.file.as_ref().is_some_and(|f| (f.1, f.2) == k))
        .collect();
    let file_size = fd.and_then(stat_fd).map_or(0, |s| s.st_size as u64);
    let extent = views
        .iter()
        .map(|v| v.offset + (v.end - v.start))
        .max()
        .unwrap_or(0);
    let size = file_size.max(extent).max(min_size).div_ceil(PAGE) * PAGE;
    let base = super::mem::allocate(size)?;
    // Fork children share it, as they share the file.
    super::mem::inherit_shared(base, size);
    match fd {
        // A file with no blocks once what its mappings wrote is flushed
        // holds only zeros, as the fresh memory does: ART sizes its JIT
        // cache's memfd to the cache's capacity and maps it twice at once,
        // and reading 64 MiB of holes would touch every page.
        Some(fd) if !holds_data(fd) => {}
        Some(fd) => {
            let mut done = 0u64;
            while done < file_size {
                // SAFETY: reading into our allocation.
                let n = unsafe {
                    libc::pread(
                        fd,
                        (base + done) as *mut _,
                        (file_size - done) as usize,
                        done as i64,
                    )
                };
                if n <= 0 {
                    break;
                }
                done += n as u64;
            }
        }
        None => {
            for v in &views {
                // SAFETY: copying a readable view into our allocation.
                if v.prot & 1 != 0 {
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            v.start as *const u8,
                            (base + v.offset) as *mut u8,
                            (v.end - v.start) as usize,
                        )
                    };
                }
            }
        }
    }
    for v in &views {
        super::mem::remap_shared(v.start, base + v.offset, v.end - v.start)?;
        // SAFETY: restoring the view's protection.
        unsafe { libc::mprotect(v.start as *mut _, (v.end - v.start) as usize, v.prot as i32) };
    }
    with(|m| {
        if let Some(f) = m.get_mut(&k) {
            f.anon = Some((base, size));
        }
    });
    Ok((base, size))
}

/// Whether the file of `fd` has any block, with the pages its mappings
/// dirtied written first (a sparse file's holes have none).
fn holds_data(fd: i32) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: plain fsync and fstat of our fd.
    unsafe { libc::fsync(fd) != 0 || libc::fstat(fd, &mut st) != 0 || st.st_blocks != 0 }
}

/// mmap(MAP_SHARED) of `fd`. None when the host mapping of the file itself
/// is right (not a memfd, or not executable and not converted yet).
pub fn map_shared(fd: i32, addr: u64, len: u64, prot: i32, fixed: bool, off: u64) -> Option<i64> {
    let st = memfd_stat(fd)?;
    let k = key_of(&st);
    know(k, fd);
    let seals = read_seals(fd);
    if prot & libc::PROT_WRITE != 0 && seals & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0 {
        return Some(-(EPERM as i64));
    }
    let anon = with(|m| m.get(&k).and_then(|f| f.anon));
    if anon.is_none() && prot & libc::PROT_EXEC == 0 {
        return None;
    }
    if prot & libc::PROT_EXEC != 0 && seals & F_SEAL_EXEC != 0 {
        return Some(-(libc::EACCES as i64));
    }
    let (base, size) = match convert(k, Some(fd), off + len) {
        Ok(a) => a,
        Err(e) => return Some(e),
    };
    if off + len > size {
        return Some(-(libc::ENOMEM as i64));
    }
    let target = if fixed {
        addr
    } else {
        match super::mem::allocate(len) {
            Ok(a) => a,
            Err(e) => return Some(e),
        }
    };
    if let Err(e) = super::mem::remap_shared(target, base + off, len) {
        return Some(e);
    }
    // SAFETY: the view we just created.
    if unsafe { libc::mprotect(target as *mut _, len as usize, prot) } < 0 {
        return Some(-(errno::last() as i64));
    }
    Some(target as i64)
}

/// mprotect adding PROT_EXEC over `[lo, hi)`: shared views of memfds there
/// move to executable-capable memory first.
pub fn before_exec_protect(lo: u64, hi: u64) {
    let keys: Vec<Key> = vmmap::regions(lo, hi)
        .filter(|r| r.shared)
        .filter_map(|r| r.file.map(|f| (f.1, f.2)))
        .filter(|k| with(|m| m.contains_key(k)))
        .collect();
    for k in keys {
        let _ = convert(k, None, 0);
    }
}

/// read/write/pread/pwrite on a memfd in executable mode go to the shared
/// memory. None: not a converted memfd (use the file).
pub fn rw(fd: i32, buf: u64, len: usize, pos: Option<i64>, write: bool) -> Option<i64> {
    let k = key(fd)?;
    if write && write_sealed(fd) {
        return Some(-(EPERM as i64));
    }
    let (base, size) = with(|m| m.get(&k).and_then(|f| f.anon))?;
    let st = stat_fd(fd)?;
    // SAFETY: plain lseek on our fd.
    let at = pos.unwrap_or_else(|| unsafe { libc::lseek(fd, 0, libc::SEEK_CUR) }) as u64;
    let end = if write {
        size
    } else {
        (st.st_size as u64).min(size)
    };
    let n = (len as u64).min(end.saturating_sub(at));
    // SAFETY: inside the memfd's memory and the guest buffer.
    unsafe {
        if write {
            std::ptr::copy_nonoverlapping(buf as *const u8, (base + at) as *mut u8, n as usize);
        } else {
            std::ptr::copy_nonoverlapping((base + at) as *const u8, buf as *mut u8, n as usize);
        }
        if write && at + n > st.st_size as u64 {
            libc::ftruncate(fd, (at + n) as i64);
        }
        if pos.is_none() {
            libc::lseek(fd, (at + n) as i64, libc::SEEK_SET);
        }
    }
    Some(n as i64)
}

/// Whether a memfd's contents moved to this process's memory (mapped
/// executable), which an exec in place would lose.
pub fn has_exec_copies() -> bool {
    with(|m| m.values().any(|f| f.anon.is_some()))
}

/// Fork: the memfds this process knows, with their executable-mode
/// memory (shared with the child, as the file is).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let m = MEMFDS.lock().unwrap();
    let all: Vec<_> = m.iter().flatten().collect();
    w.seq(all.into_iter(), |w, (k, f)| {
        w.u64(k.0);
        w.u64(k.1);
        w.str(&f.name);
        w.opt(f.anon, |w, (b, s)| {
            w.u64(b);
            w.u64(s);
        });
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let v = r.seq(|r| {
        (
            (r.u64(), r.u64()),
            Memfd {
                name: r.str(),
                anon: r.opt(|r| (r.u64(), r.u64())),
            },
        )
    });
    with(|m| m.extend(v));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(name: &str, flags: u64) -> i32 {
        let name = CString::new(name).unwrap();
        memfd_create([name.as_ptr() as u64, flags, 0, 0, 0, 0]) as i32
    }

    fn close(fd: i32) {
        fdtab::on_close(fd);
        // SAFETY: our test fd.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn seal_rules() {
        let fd = create("t", MFD_ALLOW_SEALING);
        assert!(fd >= 0);
        let st = stat_fd(fd).unwrap();
        assert_eq!(fd_link(fd, &st).unwrap(), "/memfd:t (deleted)");
        assert_eq!(st.st_mode & 0o777, 0o777);
        // SAFETY: our test fd.
        unsafe { libc::ftruncate(fd, 4096) };
        assert_eq!(add_seals(fd, 0x40), -(EINVAL as i64));
        assert_eq!(add_seals(fd, F_SEAL_SHRINK), 0);
        assert!(resize_sealed(fd, 0));
        assert!(!resize_sealed(fd, 8192));
        assert_eq!(add_seals(fd, F_SEAL_SEAL | F_SEAL_WRITE), 0);
        assert!(write_sealed(fd));
        assert_eq!(add_seals(fd, F_SEAL_GROW), -(EPERM as i64));
        assert_eq!(
            get_seals(fd) as u32,
            F_SEAL_SHRINK | F_SEAL_SEAL | F_SEAL_WRITE
        );
        close(fd);
        let plain = create("plain", 0);
        assert_eq!(get_seals(plain) as u32, F_SEAL_SEAL);
        close(plain);
    }

    /// F_SEAL_EXEC on an executable memfd brings the write seals; a
    /// MFD_NOEXEC_SEAL memfd is not executable and sealable.
    #[test]
    fn executable_view_keeps_what_was_written_through_a_mapping() {
        let fd = create("jit", 0);
        let size = 8 << 20;
        // SAFETY: our test fd, sparse, mapped writable: data written
        // through the mapping at the start and past a hole.
        let rw = unsafe {
            libc::ftruncate(fd, size);
            let rw = libc::mmap(
                std::ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            ) as *mut u8;
            std::ptr::copy_nonoverlapping(b"head".as_ptr(), rw, 4);
            std::ptr::copy_nonoverlapping(b"tail".as_ptr(), rw.add((4 << 20) + 7), 4);
            rw
        };
        let view = map_shared(
            fd,
            0,
            size as u64,
            libc::PROT_READ | libc::PROT_EXEC,
            false,
            0,
        )
        .unwrap();
        assert!(view > 0);
        // SAFETY: the view is readable and `size` long.
        unsafe {
            let v = std::slice::from_raw_parts(view as *const u8, size as usize);
            assert_eq!(&v[..4], b"head");
            assert_eq!(&v[(4 << 20) + 7..(4 << 20) + 11], b"tail");
            assert!(v[4..(4 << 20) + 7].iter().all(|&b| b == 0));
            libc::munmap(view as *mut _, size as usize);
            libc::munmap(rw.cast(), size as usize);
        }
        close(fd);
    }

    #[test]
    fn exec_seal() {
        let fd = create("x", MFD_ALLOW_SEALING);
        assert_eq!(add_seals(fd, F_SEAL_EXEC), 0);
        assert_eq!(
            get_seals(fd) as u32,
            F_SEAL_EXEC | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE | F_SEAL_FUTURE_WRITE
        );
        close(fd);
        let fd = create("n", MFD_NOEXEC_SEAL);
        assert_eq!(stat_fd(fd).unwrap().st_mode & 0o777, 0o666);
        assert_eq!(get_seals(fd) as u32, F_SEAL_EXEC);
        assert_eq!(add_seals(fd, F_SEAL_GROW), 0);
        close(fd);
    }

    /// A reopen is a new description of the same memfd; an fd the table
    /// never saw (as one received from another process) is still one.
    #[test]
    fn reopen_and_foreign_fds() {
        let fd = create("chromium-ashmem", MFD_ALLOW_SEALING);
        // SAFETY: our test fd.
        assert_eq!(unsafe { libc::write(fd, b"shared".as_ptr().cast(), 6) }, 6);
        let ro = reopen(fd, libc::O_RDONLY).unwrap() as i32;
        assert!(ro >= 0 && ro != fd);
        // SAFETY: our test fds.
        unsafe {
            assert_eq!(
                libc::fcntl(ro, libc::F_GETFL) & libc::O_ACCMODE,
                libc::O_RDONLY
            );
            assert_eq!(
                libc::fcntl(fd, libc::F_GETFL) & libc::O_ACCMODE,
                libc::O_RDWR
            );
            // Its own offset.
            let mut b = [0u8; 6];
            assert_eq!(libc::read(ro, b.as_mut_ptr().cast(), 6), 6);
            assert_eq!(&b, b"shared");
            assert_eq!(libc::write(ro, b.as_ptr().cast(), 1), -1);
        }
        assert_eq!(add_seals(ro, F_SEAL_GROW), -(EPERM as i64));
        assert_eq!(add_seals(fd, F_SEAL_SHRINK | F_SEAL_FUTURE_WRITE), 0);
        assert_eq!(get_seals(ro) as u32, F_SEAL_SHRINK | F_SEAL_FUTURE_WRITE);
        assert_eq!(
            reopen(fd, libc::O_RDWR | libc::O_TRUNC),
            Some(-(EPERM as i64))
        );
        // SAFETY: a host dup the fd table does not know.
        let foreign = unsafe { libc::dup(fd) };
        assert_eq!(
            get_seals(foreign) as u32,
            F_SEAL_SHRINK | F_SEAL_FUTURE_WRITE
        );
        adopt(foreign);
        assert!(write_sealed(foreign));
        let st = stat_fd(foreign).unwrap();
        assert_eq!(
            fd_link(foreign, &st).unwrap(),
            "/memfd:chromium-ashmem (deleted)"
        );
        for f in [fd, ro, foreign] {
            close(f);
        }
        // SAFETY: plain test file.
        let other = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) };
        assert_eq!(get_seals(other), -(EINVAL as i64));
        assert!(reopen(other, libc::O_RDONLY).is_none());
        // SAFETY: our fd.
        unsafe { libc::close(other) };
    }

    /// A sweep removes the names only of files no description holds.
    #[test]
    fn sweep_keeps_held_files() {
        let d = std::env::temp_dir().join(format!("aim-memfd-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let held = std::fs::File::create(d.join("held")).unwrap();
        drop(std::fs::File::create(d.join("free")).unwrap());
        use std::os::fd::AsRawFd;
        hold(held.as_raw_fd());
        sweep(&d, 0);
        assert!(d.join("held").exists());
        assert!(!d.join("free").exists());
        // Unlocking the description rather than closing it: a child another
        // test forks meanwhile may hold a copy of it.
        // SAFETY: our fd.
        unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_UN) };
        sweep(&d, 0);
        assert!(!d.join("held").exists());
        drop(held);
        std::fs::remove_dir(&d).unwrap();
    }
}
