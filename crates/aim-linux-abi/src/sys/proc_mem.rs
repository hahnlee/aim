//! /proc/<pid>/mem (proc_pid_mem(5), Linux fs/proc/base.c): an open pins
//! an address-space identity, not its pages; access is checked only at open.
//! The target's agent copies its own memory. An unlinked carrier keeps the
//! offset and private metadata across dup, fork, exec and descriptor passing.

use std::sync::{LazyLock, RwLock};

use super::{cred, fdtab, ptrace, user_memory};
use crate::errno::{self, EACCES, EINVAL, EIO, Errno};

const META: &std::ffi::CStr = c"dev.aim.proc-mem.v1";
const POSITION: &std::ffi::CStr = c"dev.aim.proc-mem.position";
const PAGE: u64 = super::mem::PAGE;
unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
}

#[derive(Clone, Copy)]
pub struct Open {
    pub pid: i32,
    pub guest_pid: i32,
    pub mode: u32,
    uid: u32,
    gid: u32,
    epoch: [u8; 16],
}

static EPOCH: LazyLock<RwLock<[u8; 16]>> = LazyLock::new(|| RwLock::new(new_epoch()));
fn new_epoch() -> [u8; 16] {
    let mut bytes = [0; 16];
    unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
    bytes
}
pub(super) fn epoch() -> [u8; 16] {
    *EPOCH.read().unwrap_or_else(|e| e.into_inner())
}
/// In-place exec holds this until the new mappings are ready. Transfers
/// already in flight finish first; an old open can never reach the new mm.
pub(super) fn exec_reset() -> std::sync::RwLockWriteGuard<'static, [u8; 16]> {
    let mut epoch = EPOCH.write().unwrap_or_else(|e| e.into_inner());
    *epoch = new_epoch();
    epoch
}

fn set(fd: i32, name: &std::ffi::CStr, bytes: &[u8]) -> Result<(), Errno> {
    if unsafe { libc::fsetxattr(fd, name.as_ptr(), bytes.as_ptr().cast(), bytes.len(), 0, 0) } < 0 {
        return Err(errno::last());
    }
    Ok(())
}
fn get<const N: usize>(fd: i32, name: &std::ffi::CStr) -> Result<[u8; N], Errno> {
    let mut bytes = [0; N];
    let n = unsafe { libc::fgetxattr(fd, name.as_ptr(), bytes.as_mut_ptr().cast(), N, 0, 0) };
    if n < 0 {
        Err(errno::last())
    } else if n as usize != N {
        Err(EIO)
    } else {
        Ok(bytes)
    }
}

impl Open {
    fn encode(self) -> [u8; 36] {
        let mut bytes = [0; 36];
        for (chunk, word) in bytes[..20].chunks_exact_mut(4).zip([
            self.pid as u32,
            self.mode,
            self.uid,
            self.gid,
            self.guest_pid as u32,
        ]) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes[20..].copy_from_slice(&self.epoch);
        bytes
    }
    pub(super) fn load(fd: i32) -> Result<Self, Errno> {
        let bytes = get::<36>(fd, META)?;
        let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Ok(Self {
            pid: word(0) as i32,
            mode: word(4),
            uid: word(8),
            gid: word(12),
            guest_pid: word(16) as i32,
            epoch: bytes[20..].try_into().unwrap(),
        })
    }
    pub fn stat(self) -> libc::stat {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        st.st_mode = libc::S_IFREG | 0o600;
        st.st_ino = format!("/proc/{}/mem", self.guest_pid)
            .bytes()
            .fold(1469598103934665603u64, |h, b| {
                (h ^ b as u64).wrapping_mul(1099511628211)
            });
        st.st_uid = self.uid;
        st.st_gid = self.gid;
        st.st_nlink = 1;
        st.st_blksize = 1024;
        st
    }
}

pub fn adopt(fd: i32) {
    if let Ok(open) = Open::load(fd) {
        fdtab::insert(fd, fdtab::Kind::ProcMem(open));
    }
}
pub fn is_mem(fd: i32) -> bool {
    matches!(fdtab::get(fd), Some(fdtab::Kind::ProcMem(_)))
}

pub fn open(pid: i32, flags: u64) -> i64 {
    let result = (|| {
        if flags & 0o40000 != 0 {
            return Err(errno::ENOTDIR);
        } // O_DIRECTORY
        let own = pid == unsafe { libc::getpid() };
        if !own && !cred::may_ptrace(pid, true) {
            return Err(EACCES);
        }
        let epoch = if own {
            epoch()
        } else {
            ptrace::open_mem(pid).map_err(|error| -error as Errno)?
        };
        let peer = cred::peer(pid);
        let open = Open {
            pid,
            guest_pid: super::pidns::guest_pid(pid).map_err(|error| -error as Errno)?,
            mode: (flags & 3) as u32,
            uid: peer.uid,
            gid: peer.gid,
            epoch,
        };
        let fd = super::procfs::content_fd_unpublished(&[], flags & super::fs::O_CLOEXEC != 0);
        if fd < 0 {
            return Err(-fd as Errno);
        }
        let fd = fd as i32;
        fdtab::insert(fd, fdtab::Kind::ProcMem(open));
        let result = set(fd, META, &open.encode())
            .and_then(|()| set(fd, POSITION, &0u64.to_le_bytes()))
            .and_then(|()| fdtab::publish_guest(fd));
        if let Err(error) = result {
            fdtab::on_close(fd);
            unsafe { libc::close(fd) };
            return Err(error);
        }
        Ok(fd as i64)
    })();
    result.unwrap_or_else(|error| -(error as i64))
}

/// POSIX locks serialize processes; the descriptor lifecycle lock serializes
/// local threads and prevents any local close from releasing the POSIX lock.
/// The kernel releases it if the holder exits. No process-shared Rust mutex
/// or inherited userspace lock is involved.
struct PositionLock {
    fd: i32,
    _lifecycle: fdtab::Lifecycle,
}
impl PositionLock {
    fn new(fd: i32) -> Result<Self, Errno> {
        let guard = fdtab::lifecycle();
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK;
        lock.l_whence = libc::SEEK_SET as i16;
        loop {
            if unsafe { libc::fcntl(fd, libc::F_SETLKW, &lock) } == 0 {
                return Ok(Self {
                    fd,
                    _lifecycle: guard,
                });
            }
            let error = errno::last();
            if error != errno::EINTR {
                return Err(error);
            }
        }
    }
}
impl Drop for PositionLock {
    fn drop(&mut self) {
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_UNLCK;
        lock.l_whence = libc::SEEK_SET as i16;
        // The descriptor is pinned and its lifecycle is locked.
        unsafe { libc::fcntl(self.fd, libc::F_SETLK, &lock) };
    }
}

pub fn seek(fd: i32, offset: i64, whence: i32) -> i64 {
    let result = (|| {
        let _guard = PositionLock::new(fd)?;
        let pos = match whence {
            libc::SEEK_SET => offset as u64,
            libc::SEEK_CUR => u64::from_le_bytes(get(fd, POSITION)?).wrapping_add(offset as u64),
            _ => return Err(EINVAL),
        };
        set(fd, POSITION, &pos.to_le_bytes())?;
        Ok(pos as i64)
    })();
    result.unwrap_or_else(|error| -(error as i64))
}

pub fn rw(fd: i32, open: Open, iov: &[libc::iovec], pos: Option<u64>, write: bool) -> i64 {
    let result = (|| {
        if open.mode == 3 || write && open.mode == 0 || !write && open.mode == 1 {
            return Err(errno::EBADF);
        }
        let own = open.pid == unsafe { libc::getpid() };
        if own && open.epoch != epoch() {
            return Ok(0);
        }
        let (guard, mut at, failed) = loop {
            let guard = PositionLock::new(fd)?;
            let at = pos
                .map(Ok)
                .unwrap_or_else(|| get(fd, POSITION).map(u64::from_le_bytes))?;
            if !own {
                break (guard, at, None);
            }
            // A verified lazy mapping may allocate private descriptors while
            // faulting in. Prepare outside the lifecycle lock, then confirm
            // f_pos did not change before taking it for the actual transfer.
            drop(guard);
            let mut next = at;
            let mut failed = None;
            'pages: for vector in iov {
                let mut left = vector.iov_len;
                while left > 0 {
                    if let Err(error) = super::verity_pager::prepare_debug_page(next) {
                        failed = Some((next, error));
                        break 'pages;
                    }
                    let n = ((PAGE - next % PAGE) as usize).min(left);
                    left -= n;
                    next = next.wrapping_add(n as u64);
                }
            }
            let guard = PositionLock::new(fd)?;
            if pos.is_some() || u64::from_le_bytes(get(fd, POSITION)?) == at {
                break (guard, at, failed);
            }
        };
        let _guard = guard;
        let mut done = 0usize;
        for vector in iov {
            let mut used = 0;
            while used < vector.iov_len {
                let count = (PAGE - at % PAGE).min((vector.iov_len - used) as u64) as usize;
                let local = vector.iov_base as u64 + used as u64;
                let data = if write {
                    user_memory::read_exact(local, count)?
                } else {
                    Vec::new()
                };
                let (n, data) =
                    if let Some((_, error)) = failed.filter(|(address, _)| *address == at) {
                        (-(error as i64), Vec::new())
                    } else if own {
                        transfer_ready(open.epoch, at, count, write, &data)
                    } else {
                        ptrace::mem_io(open.pid, open.epoch, at, count, write, &data)
                    };
                if n < 0 {
                    if done == 0 {
                        return Err(-n as Errno);
                    }
                    break;
                }
                if n == 0 {
                    break;
                }
                if !write {
                    user_memory::write_exact(local, &data)?;
                }
                at = at.wrapping_add(n as u64);
                done += n as usize;
                used += n as usize;
                if pos.is_none() {
                    set(fd, POSITION, &at.to_le_bytes())?;
                }
                if (n as usize) < count {
                    break;
                }
            }
            if used < vector.iov_len {
                break;
            }
        }
        Ok(done as i64)
    })();
    result.unwrap_or_else(|error| -(error as i64))
}

/// One page of the target's own address space. An alias provides FOLL_FORCE
/// access without changing the protections visible to its running threads.
/// VM remap resolves private COW before sharing the alias, as patch::vm does
/// when it writes translated code. A shared read-only mapping cannot be
/// forced writable (Linux's check_vma_flags also rejects that case).
pub(super) fn transfer(
    id: [u8; 16],
    address: u64,
    count: usize,
    write: bool,
    data: &[u8],
) -> (i64, Vec<u8>) {
    if id != epoch() {
        return (0, Vec::new());
    }
    if let Err(error) = super::verity_pager::prepare_debug_page(address) {
        return (if id != epoch() { 0 } else { -(error as i64) }, Vec::new());
    }
    transfer_ready(id, address, count, write, data)
}

fn transfer_ready(
    id: [u8; 16],
    address: u64,
    count: usize,
    write: bool,
    data: &[u8],
) -> (i64, Vec<u8>) {
    use crate::patch::vm;
    let epoch = EPOCH.read().unwrap_or_else(|e| e.into_inner());
    if id != *epoch {
        return (0, Vec::new());
    }
    if count == 0 {
        return (0, Vec::new());
    }
    if count as u64 > PAGE - address % PAGE || write && data.len() != count {
        return (-(EINVAL as i64), Vec::new());
    }
    let result = (|| {
        let region = super::vmmap::region_at(address).ok_or(EIO)?;
        if region.start > address
            || count as u64 > region.end - address
            || write && region.shared && region.prot & libc::PROT_WRITE as u32 == 0
        {
            return Err(EIO);
        }
        let base = address & !(PAGE - 1);
        let protection = if write {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        let alias = vm::alias(base, PAGE, None, protection).ok_or(EIO)?;
        let result = if write {
            user_memory::write_exact(alias + address - base, data).map(|()| Vec::new())
        } else {
            user_memory::read_exact(alias + address - base, count)
        };
        if write && result.is_ok() && region.prot & libc::PROT_EXEC as u32 != 0 {
            unsafe { sys_icache_invalidate(address as *mut _, count) };
        }
        unsafe { vm::mach_vm_deallocate(vm::task(), alias, PAGE) };
        // Darwin cannot alias pages lacking the required maximum protection;
        // this is mem_rw's EIO when access_remote_vm cannot pin a page.
        result.map_err(|_| EIO)
    })();
    match result {
        Ok(bytes) => (count as i64, bytes),
        Err(error) => (-(error as i64), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_private_access_preserves_protections_and_file_contents() {
        let fd = super::super::procfs::content_fd_unpublished(b"original", true) as i32;
        assert!(fd >= 0);
        assert_eq!(unsafe { libc::ftruncate(fd, PAGE as i64) }, 0);
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE as usize,
                libc::PROT_NONE,
                libc::MAP_PRIVATE,
                fd,
                0,
            )
        };
        assert_ne!(address, libc::MAP_FAILED);
        let address = address as u64;
        assert_eq!(
            transfer(epoch(), address, 8, false, &[]),
            (8, b"original".to_vec())
        );
        assert_eq!(transfer(epoch(), address, 8, true, b"modified").0, 8);
        assert_eq!(transfer(epoch(), address, 8, false, &[]).1, b"modified");
        assert_eq!(
            crate::patch::vm::region(address).unwrap().2,
            libc::PROT_NONE
        );
        let mut bytes = [0u8; 8];
        assert_eq!(
            unsafe { libc::pread(fd, bytes.as_mut_ptr().cast(), 8, 0) },
            8
        );
        assert_eq!(&bytes, b"original");
        assert_eq!(transfer([0; 16], address, 8, false, &[]).0, 0);
        assert_eq!(transfer(epoch(), 16, 8, false, &[]).0, -(EIO as i64));
        unsafe {
            libc::munmap(address as *mut _, PAGE as usize);
        }
        let shared = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE as usize,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(shared, libc::MAP_FAILED);
        assert_eq!(
            transfer(epoch(), shared as u64, 8, true, b"denied!!").0,
            -(EIO as i64)
        );
        assert_eq!(
            unsafe { libc::mprotect(shared, PAGE as usize, libc::PROT_READ | libc::PROT_WRITE) },
            0
        );
        assert_eq!(transfer(epoch(), shared as u64, 8, true, b"shared!!").0, 8);
        assert_eq!(
            unsafe { libc::pread(fd, bytes.as_mut_ptr().cast(), 8, 0) },
            8
        );
        assert_eq!(&bytes, b"shared!!");
        unsafe {
            libc::munmap(shared, PAGE as usize);
        }
        fdtab::on_close(fd);
        unsafe {
            libc::close(fd);
        }
    }

    #[test]
    fn descriptor_metadata_unsigned_offsets_and_access_modes() {
        if fdtab::isolated_kernel_test(
            "sys::proc_mem::tests::descriptor_metadata_unsigned_offsets_and_access_modes",
        ) {
            return;
        }
        let (_view, _root) = crate::vfs::test_view();
        let fd = open(unsafe { libc::getpid() }, 0) as i32;
        assert!(fd >= 0);
        let state = Open::load(fd).unwrap();
        assert_eq!(state.mode, 0);
        assert_eq!(state.epoch, epoch());
        assert_eq!(seek(fd, -8192, libc::SEEK_SET), -8192);
        assert_eq!(seek(fd, 8193, libc::SEEK_CUR), 1);
        assert_eq!(seek(fd, 0, libc::SEEK_END), -(EINVAL as i64));
        let vector = libc::iovec {
            iov_base: std::ptr::null_mut(),
            iov_len: 0,
        };
        assert_eq!(rw(fd, state, &[vector], None, true), -(errno::EBADF as i64));
        let input = *b"self mem";
        let mut output = [0; 8];
        assert_eq!(
            super::super::fs::pread64([
                fd as u64,
                output.as_mut_ptr() as u64,
                8,
                input.as_ptr() as u64,
                0,
                0
            ]),
            8
        );
        assert_eq!(output, input);
        let bytes = state.encode();
        fdtab::on_close(fd);
        adopt(fd);
        assert_eq!(Open::load(fd).unwrap().encode(), bytes);
        assert!(is_mem(fd));
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
    }
}
