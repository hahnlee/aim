//! memfd_create and file seals.
//!
//! A memfd is an unlinked temporary file, so read/write, ftruncate, fstat,
//! MAP_SHARED mappings, fork, dup and descriptor passing all work natively
//! and across processes.
//!
//! Darwin refuses executable views of file-backed shared memory (ADR 0012,
//! "Platform probes"). ART's JIT maps one memfd twice, RW and RX. When a
//! memfd is first mapped (or mprotect'ed) executable in a process, its
//! contents move to anonymous memory inherited as shared by fork children;
//! every existing view is remapped onto it, and later mappings alias it with
//! `mach_vm_remap`, which gives the dual views. From then on that process's
//! reads and writes through the fd use the same memory.
//!
//! Seals: SHRINK/GROW refuse ftruncate and fallocate; WRITE/FUTURE_WRITE
//! refuse write and writable shared mappings. A write seal does not revoke
//! writable mappings that already exist.

use std::collections::HashMap;
use std::sync::Mutex;

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

const PAGE: u64 = 16384;

/// (device, inode) of the backing file.
pub type Key = (u64, u64);

struct Memfd {
    name: String,
    seals: u32,
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

/// The memfd key of `fd`, if it is one of ours.
pub fn key(fd: i32) -> Option<Key> {
    match fdtab::get(fd) {
        Some(Kind::Memfd(k)) => Some(k),
        _ => None,
    }
}

pub fn memfd_create(a: [u64; 6]) -> i64 {
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
    let mut tmpl = std::env::temp_dir()
        .join("linux-abi-memfd.XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    tmpl.push(0);
    // SAFETY: mkstemp fills in the template in place.
    let fd = unsafe { libc::mkstemp(tmpl.as_mut_ptr().cast()) };
    if fd < 0 {
        return -(errno::last() as i64);
    }
    // SAFETY: removing the name we just created; the fd keeps the file.
    unsafe { libc::unlink(tmpl.as_ptr().cast()) };
    fdtab::set_flags(fd, false, flags & MFD_CLOEXEC != 0);
    let Some(st) = stat_fd(fd) else {
        return -(errno::last() as i64);
    };
    let mut seals = 0;
    if flags & (MFD_ALLOW_SEALING | MFD_NOEXEC_SEAL) == 0 {
        seals |= F_SEAL_SEAL;
    }
    if flags & MFD_NOEXEC_SEAL != 0 {
        seals |= F_SEAL_EXEC;
    }
    let k = key_of(&st);
    with(|m| {
        m.insert(
            k,
            Memfd {
                name: String::from_utf8_lossy(name).into_owned(),
                seals,
                anon: None,
            },
        )
    });
    fdtab::insert(fd, Kind::Memfd(k));
    fd as i64
}

pub fn seals(fd: i32) -> Option<u32> {
    let k = key(fd)?;
    with(|m| m.get(&k).map(|f| f.seals))
}

pub fn get_seals(fd: i32) -> i64 {
    match seals(fd) {
        Some(s) => s as i64,
        None => -(EINVAL as i64),
    }
}

pub fn add_seals(fd: i32, add: u32) -> i64 {
    let Some(k) = key(fd) else {
        return -(EINVAL as i64);
    };
    // SAFETY: plain fcntl.
    if unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_ACCMODE == libc::O_RDONLY {
        return -(EPERM as i64);
    }
    with(|m| {
        let Some(f) = m.get_mut(&k) else {
            return -(EINVAL as i64);
        };
        if f.seals & F_SEAL_SEAL != 0 {
            return -(EPERM as i64);
        }
        f.seals |= add & 0x3f;
        0
    })
}

/// Whether a write through `fd` is refused by a seal.
pub fn write_sealed(fd: i32) -> bool {
    seals(fd).is_some_and(|s| s & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0)
}

/// Whether resizing `fd` from its current size to `new` is refused.
pub fn resize_sealed(fd: i32, new: u64) -> bool {
    let Some(s) = seals(fd) else {
        return false;
    };
    let Some(st) = stat_fd(fd) else {
        return false;
    };
    let cur = st.st_size as u64;
    (new < cur && s & F_SEAL_SHRINK != 0) || (new > cur && s & F_SEAL_GROW != 0)
}

/// `/memfd:name (deleted)` for /proc links and maps.
pub fn link_name(dev: u64, ino: u64) -> Option<String> {
    with(|m| {
        m.get(&(dev, ino))
            .map(|f| format!("/memfd:{} (deleted)", f.name))
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
    let views: Vec<vmmap::Region> = vmmap::regions(0, u64::MAX)
        .filter(|r| r.shared && r.file.as_ref().is_some_and(|f| (f.1, f.2) == k))
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

/// mmap(MAP_SHARED) of `fd`. None when the host mapping of the file itself
/// is right (not a memfd, or not executable and not converted yet).
pub fn map_shared(fd: i32, addr: u64, len: u64, prot: i32, fixed: bool, off: u64) -> Option<i64> {
    let st = stat_fd(fd)?;
    let k = key_of(&st);
    let f = with(|m| m.get(&k).map(|f| (f.seals, f.anon)))?;
    if prot & libc::PROT_WRITE != 0 && f.0 & (F_SEAL_WRITE | F_SEAL_FUTURE_WRITE) != 0 {
        return Some(-(EPERM as i64));
    }
    if f.1.is_none() && prot & libc::PROT_EXEC == 0 {
        return None;
    }
    if prot & libc::PROT_EXEC != 0 && f.0 & F_SEAL_EXEC != 0 {
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

/// Fork: the memfds this process knows, with their executable-mode
/// memory (shared with the child, as the file is).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let m = MEMFDS.lock().unwrap();
    let all: Vec<_> = m.iter().flatten().collect();
    w.seq(all.into_iter(), |w, (k, f)| {
        w.u64(k.0);
        w.u64(k.1);
        w.str(&f.name);
        w.u32(f.seals);
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
                seals: r.u32(),
                anon: r.opt(|r| (r.u64(), r.u64())),
            },
        )
    });
    with(|m| m.extend(v));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_rules() {
        let name = std::ffi::CString::new("t").unwrap();
        let fd = memfd_create([name.as_ptr() as u64, MFD_ALLOW_SEALING, 0, 0, 0, 0]) as i32;
        assert!(fd >= 0);
        let st = stat_fd(fd).unwrap();
        assert_eq!(
            link_name(st.st_dev as u32 as u64, st.st_ino).unwrap(),
            "/memfd:t (deleted)"
        );
        // SAFETY: our test fd.
        unsafe { libc::ftruncate(fd, 4096) };
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
        fdtab::on_close(fd);
        // SAFETY: our test fd.
        unsafe { libc::close(fd) };
    }
}
