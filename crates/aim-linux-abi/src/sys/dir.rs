//! Directory streams: `getdents64` with Linux `struct linux_dirent64`.
//!
//! Darwin's `getdirentries64` gives no usable seek cookies (APFS reports 0),
//! so the entries of a directory are read once into a stream kept with the
//! fd (see `fdtab`); `d_off` is the index of the next entry, and `lseek` on
//! the fd moves the index. Synthesized `/proc` and `/sys` directories use
//! the same stream with fixed entries.

use std::sync::{Arc, Mutex};

use super::fdtab::{self, Kind};
use crate::errno::{self, EINVAL, ENOTDIR};

// Linux (and Darwin) d_type values.
pub const DT_DIR: u8 = 4;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;

pub struct Entry {
    pub ino: u64,
    pub ty: u8,
    pub name: Vec<u8>,
}

impl Entry {
    pub fn new(ino: u64, ty: u8, name: impl Into<Vec<u8>>) -> Entry {
        Entry {
            ino,
            ty,
            name: name.into(),
        }
    }
}

pub struct DirStream {
    /// None: a host directory not read yet (or rewound).
    entries: Option<Vec<Entry>>,
    pos: usize,
    /// Guest path of a synthesized directory; None for host directories.
    pub guest: Option<String>,
}

/// Fork: a stream's entries and position.
pub(super) fn save(d: &DirStream, w: &mut super::fork_state::Writer) {
    w.opt(d.entries.as_ref(), |w, v| {
        w.seq(v.iter(), |w, e| {
            w.u64(e.ino);
            w.u32(e.ty as u32);
            w.bytes(&e.name);
        })
    });
    w.u64(d.pos as u64);
    w.opt(d.guest.as_deref(), |w, g| w.str(g));
}

pub(super) fn load(r: &mut super::fork_state::Reader) -> DirStream {
    DirStream {
        entries: r.opt(|r| r.seq(|r| Entry::new(r.u64(), r.u32() as u8, r.bytes()))),
        pos: r.u64() as usize,
        guest: r.opt(|r| r.str()),
    }
}

impl DirStream {
    pub fn synthesized(guest: String, entries: Vec<Entry>) -> DirStream {
        DirStream {
            entries: Some(entries),
            pos: 0,
            guest: Some(guest),
        }
    }
}

/// Darwin `struct dirent64` header (the name follows).
#[repr(C)]
struct DarwinDirent64 {
    ino: u64,
    seekoff: u64,
    reclen: u16,
    namlen: u16,
    ty: u8,
}

const SYS_GETDIRENTRIES64: i32 = 344;

fn read_host_entries(fd: i32) -> Result<Vec<Entry>, i64> {
    // SAFETY: rewinding a directory fd.
    if unsafe { libc::lseek(fd, 0, libc::SEEK_SET) } < 0 {
        return Err(-(errno::last() as i64));
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 32 << 10];
    loop {
        let mut base = 0i64;
        // SAFETY: getdirentries64 fills at most buf.len() bytes.
        let n = unsafe {
            libc::syscall(
                SYS_GETDIRENTRIES64,
                fd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut base as *mut i64,
            )
        };
        if n < 0 {
            return Err(-(errno::last() as i64));
        }
        if n == 0 {
            return Ok(out);
        }
        let mut off = 0usize;
        while off + std::mem::size_of::<DarwinDirent64>() <= n as usize {
            // SAFETY: a record the kernel wrote inside buf.
            let d = unsafe { (buf.as_ptr().add(off) as *const DarwinDirent64).read_unaligned() };
            if d.reclen == 0 {
                break;
            }
            let name_at = off + std::mem::offset_of!(DarwinDirent64, ty) + 1;
            let name = &buf[name_at..name_at + d.namlen as usize];
            // An unlinked O_TMPFILE file has no entry.
            if !super::tmpfile::is_hidden(name) {
                out.push(Entry::new(d.ino, d.ty, name));
            }
            off += d.reclen as usize;
        }
    }
}

fn stream_of(fd: i32) -> Result<Arc<Mutex<DirStream>>, i64> {
    if let Some(Kind::Dir(d)) = fdtab::get(fd) {
        return Ok(d);
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return Err(-(errno::last() as i64));
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(-(ENOTDIR as i64));
    }
    let d = Arc::new(Mutex::new(DirStream {
        entries: None,
        pos: 0,
        guest: None,
    }));
    fdtab::insert(fd, Kind::Dir(d.clone()));
    Ok(d)
}

/// Called under FD lifecycle admission before a directory is pinned.
pub(super) fn adopt(fd: i32) -> Result<(), crate::errno::Errno> {
    if fdtab::get(fd).is_some() { return Ok(()); }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 { return Err(errno::last()); }
    if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
        stream_of(fd).map_err(|error| (-error) as crate::errno::Errno)?;
    }
    Ok(())
}

const DIRENT_HEADER: usize = 19;

pub fn getdents64(a: [u64; 6]) -> i64 {
    use std::os::fd::AsRawFd;
    let pin = match fdtab::pin_guest(a[0] as i32) { Ok(pin) => pin, Err(error) => return -(error as i64) };
    if matches!(pin.kind(), Some(Kind::Path(_))) { return -(crate::errno::EBADF as i64); }
    let (fd, buf, count) = (pin.descriptor().as_raw_fd(), a[1], a[2] as usize);
    if let Some(result)=super::fuse_client::getdents(fd,buf,count){return result;}
    let Some(Kind::Dir(d)) = pin.kind() else { return -(ENOTDIR as i64); };
    let mut d = d.lock().unwrap();
    if d.entries.is_none() {
        match read_host_entries(fd) {
            Ok(mut e) => {
                if super::evdev::is_device_dir(fd) {
                    super::evdev::fix_types(&mut e);
                }
                d.entries = Some(e)
            }
            Err(e) => return e,
        }
    }
    let mut pos = d.pos;
    let entries = d.entries.as_ref().unwrap();
    let mut off = 0usize;
    while let Some(e) = entries.get(pos) {
        let reclen = (DIRENT_HEADER + e.name.len() + 1 + 7) & !7;
        if off + reclen > count {
            if off == 0 {
                return -(EINVAL as i64);
            }
            break;
        }
        let mut rec = vec![0u8; reclen];
        rec[0..8].copy_from_slice(&e.ino.to_le_bytes());
        rec[8..16].copy_from_slice(&((pos + 1) as i64).to_le_bytes());
        rec[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        rec[18] = e.ty;
        rec[DIRENT_HEADER..DIRENT_HEADER + e.name.len()].copy_from_slice(&e.name);
        // SAFETY: guest buffer of `count` bytes.
        unsafe { std::ptr::copy_nonoverlapping(rec.as_ptr(), (buf as *mut u8).add(off), reclen) };
        off += reclen;
        pos += 1;
    }
    d.pos = pos;
    off as i64
}

/// lseek on a directory stream: the offset is an entry index. Returns None
/// when `fd` has no stream.
pub fn lseek(fd: i32, off: i64, whence: i32) -> Option<i64> {
    let Some(Kind::Dir(d)) = fdtab::get(fd) else {
        return None;
    };
    let mut d = d.lock().unwrap();
    let new = match whence {
        libc::SEEK_SET => off,
        libc::SEEK_CUR => d.pos as i64 + off,
        _ => return Some(-(EINVAL as i64)),
    };
    if new < 0 {
        return Some(-(EINVAL as i64));
    }
    d.pos = new as usize;
    if new == 0 && d.guest.is_none() {
        // rewinddir: see entries created since.
        d.entries = None;
    }
    Some(new)
}

/// Guest path of a synthesized directory fd.
pub fn synthesized_path(fd: i32) -> Option<String> {
    match fdtab::get(fd) {
        Some(Kind::Dir(d)) => d.lock().unwrap().guest.clone(),
        _ => None,
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn repeated_pins_retain_directory_position_and_unknown_slots_are_rejected() {
        if fdtab::isolated_kernel_test("sys::dir::pin_tests::repeated_pins_retain_directory_position_and_unknown_slots_are_rejected"){return;}
        let (_view, root) = crate::vfs::test_view();
        let directory = root.join("directory-pin");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("entry"), b"data").unwrap();
        let file = std::fs::File::open(&directory).unwrap();
        let fd = unsafe { libc::dup(file.as_raw_fd()) };
        assert!(fd >= 0);
        fdtab::publish_guest(fd).unwrap();
        let mut first = [0u8; 4096];
        assert!(getdents64([fd as u64, first.as_mut_ptr() as u64, first.len() as u64, 0, 0, 0]) > 0);
        let mut next = [0u8; 4096];
        assert_eq!(getdents64([fd as u64, next.as_mut_ptr() as u64, next.len() as u64, 0, 0, 0]), 0);
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(getdents64([file.as_raw_fd() as u64, next.as_mut_ptr() as u64, next.len() as u64, 0, 0, 0]), -(crate::errno::EBADF as i64));
    }
}
