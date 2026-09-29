//! `MAP_SHARED` mappings of files whose pages the boot's init shares as
//! memory objects: its property areas (`/dev/__properties__`, tmpfs on
//! Linux). The binder host, init's kernel-state server, hands out a
//! read-only memory entry of init's own shared mapping of each file
//! (`aim_binder_host::server::Server::share_file`); a read-only mapping of
//! such a file maps that entry, the file's own pages, instead of the file.
//!
//! A host file mapping costs 1-3 ms per call in a guest process while init
//! holds the file mapped writable (the host's endpoint security agent,
//! #337), and bionic maps about a dozen areas in every program it starts;
//! mapping the entry costs microseconds. Writes by init, futex wakes on the
//! areas and fork children see the same pages either way.

use std::collections::HashMap;
use std::sync::OnceLock;

use aim_binder_host::mach::Port;

use crate::errno::ENOMEM;
use crate::patch::vm::{VM_FLAGS_FIXED, VM_FLAGS_OVERWRITE, mach_vm_map, task};

const VM_PROT_READ: i32 = 1;
const VM_INHERIT_SHARE: u32 = 0;

/// (device, inode) -> (size, memory entry), fetched on first use.
static FILES: OnceLock<HashMap<(u64, u64), (u64, Port)>> = OnceLock::new();

fn files() -> &'static HashMap<(u64, u64), (u64, Port)> {
    FILES.get_or_init(|| {
        super::binder::shared_files()
            .into_iter()
            .map(|f| ((f.dev, f.ino), (f.size, f.entry)))
            .collect()
    })
}

/// `mmap(addr, len, prot, MAP_SHARED, fd, off)` of a shared file: at `addr`
/// when `fixed`, else anywhere in the guest range. None: not one of them,
/// or a mapping the entry cannot give (writable or executable, a
/// descriptor open for writing, past the file's pages); the caller maps
/// the file.
pub fn map(
    fd: i32,
    addr: u64,
    len: u64,
    prot: i32,
    fixed: bool,
    off: u64,
) -> Option<Result<u64, i64>> {
    if prot & !libc::PROT_READ != 0 {
        return None;
    }
    // SAFETY: plain fcntl and fstat on the guest's fd.
    let st = unsafe {
        if libc::fcntl(fd, libc::F_GETFL) & libc::O_ACCMODE != libc::O_RDONLY {
            return None;
        }
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 || st.st_mode & libc::S_IFMT != libc::S_IFREG {
            return None;
        }
        st
    };
    let &(size, entry) = files().get(&(st.st_dev as u32 as u64, st.st_ino))?;
    if off.checked_add(len)? > size {
        return None;
    }
    let mut at = if fixed {
        addr
    } else {
        match super::mem::allocate(len) {
            Ok(a) => a,
            Err(e) => return Some(Err(e)),
        }
    };
    // SAFETY: mapping init's memory entry over guest range we own.
    let kr = unsafe {
        mach_vm_map(
            task(),
            &mut at,
            len,
            0,
            VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
            entry,
            off,
            0,
            prot,
            VM_PROT_READ,
            VM_INHERIT_SHARE,
        )
    };
    if kr == 0 {
        return Some(Ok(at));
    }
    if !fixed {
        // SAFETY: the placeholder we allocated above.
        unsafe { libc::munmap(at as *mut _, len as usize) };
    }
    Some(Err(-(ENOMEM as i64)))
}
