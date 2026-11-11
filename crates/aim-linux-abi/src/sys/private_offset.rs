//! File offsets the layer keeps for the host (#3033).
//!
//! A program that seeks and reads a file in small steps (netbpfload parsing
//! ELF objects through stdio: an `lseek(SEEK_SET)`, an `lseek(SEEK_CUR)` and
//! a 4 KiB `read` per field) pays a Darwin syscall for each. While the
//! process is single-threaded and no other file descriptor anywhere refers
//! to a read-only regular file's open description (Darwin's
//! `PROC_FP_SHARED`), nothing but this thread can see or move its offset, so
//! the layer holds the offset itself: `lseek` sets it, `read` and `readv`
//! read at it with `preadv` and advance it.
//!
//! Every other syscall, and every host call, first writes the held offsets
//! back to their descriptions and stops holding them ([`release`]): only
//! the syscalls in [`keeps`] know the held offsets, and none of them shares a
//! description or uses another fd's offset. So a dup, fork, exec, `SCM_RIGHTS`
//! or binder transfer, a new thread, `sendfile` or `/proc/self/fd` reopen
//! finds the host offset exact. The next `lseek` takes the offset again
//! after checking the description is still unshared.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy)]
enum State {
    /// The layer holds the description's offset; the host's is stale.
    Held(i64),
    /// Other descriptors refer to the description: the host's offset is
    /// the one.
    Shared,
}

/// Whether any offset is held; read on every syscall.
static HELD: AtomicBool = AtomicBool::new(false);
/// Whether any fd has a state; read on every close.
static KNOWN: AtomicBool = AtomicBool::new(false);
/// The fds this module knows, each with its state; a few at a time.
static STATES: Mutex<Vec<(i32, State)>> = Mutex::new(Vec::new());

fn publish(states: &[(i32, State)]) {
    let held = states
        .iter()
        .any(|(_, state)| matches!(state, State::Held(_)));
    HELD.store(held, Ordering::Relaxed);
    KNOWN.store(!states.is_empty(), Ordering::Relaxed);
}

fn set(states: &mut Vec<(i32, State)>, fd: i32, state: Option<State>) {
    states.retain(|(known, _)| *known != fd);
    if let Some(state) = state {
        states.push((fd, state));
    }
    publish(states);
}

/// Whether syscall `nr` keeps held offsets: lseek, read, write, readv,
/// writev, pread64, pwrite64, preadv, pwritev and fstat share no description
/// and use no offset but through this module (a write of a held offset's
/// read-only description fails first).
pub fn keeps(nr: u64) -> bool {
    matches!(nr, 62..=70 | 80)
}

pub fn held() -> bool {
    HELD.load(Ordering::Relaxed)
}

/// Writes every held offset back to its description and stops holding it.
pub fn release() {
    let mut states = STATES.lock().unwrap();
    for &(fd, state) in states.iter() {
        if let State::Held(offset) = state {
            // SAFETY: plain lseek of a descriptor this process holds; a
            // nonnegative SEEK_SET of a regular file cannot fail.
            if unsafe { libc::lseek(fd, offset, libc::SEEK_SET) } != offset {
                crate::diag!(
                    "[linux-abi] fd {fd}: restoring its held offset {offset} failed: {}",
                    std::io::Error::last_os_error()
                );
                std::process::abort();
            }
        }
    }
    states.retain(|(_, state)| matches!(state, State::Shared));
    publish(&states);
}

/// `fd` is closed, replaced or no longer a plain regular file.
pub fn forget(fd: i32) {
    if !KNOWN.load(Ordering::Relaxed) {
        return;
    }
    let mut states = STATES.lock().unwrap();
    if states.iter().any(|(known, _)| *known == fd) {
        set(&mut states, fd, None);
    }
}

/// `lseek` of `fd`, a plain read-only regular file, when the layer holds or
/// can take its offset; None leaves the seek to the host.
pub fn seek(fd: i32, offset: i64, whence: i32) -> Option<i64> {
    if !super::thread::alone() {
        return None;
    }
    let mut states = STATES.lock().unwrap();
    let current = match states.iter().find(|(known, _)| *known == fd) {
        Some((_, State::Held(current))) => *current,
        Some((_, State::Shared)) => return None,
        None => match unshared_offset(fd) {
            Some(current) => current,
            None => {
                set(&mut states, fd, Some(State::Shared));
                return None;
            }
        },
    };
    let target = match whence {
        libc::SEEK_SET if offset >= 0 => offset,
        libc::SEEK_CUR => match current.checked_add(offset) {
            Some(target) if target >= 0 => target,
            _ => return Some(host_seek(&mut states, fd, current, offset, whence)),
        },
        _ => return Some(host_seek(&mut states, fd, current, offset, whence)),
    };
    set(&mut states, fd, Some(State::Held(target)));
    Some(target)
}

/// The host's own answer (SEEK_END, SEEK_DATA, SEEK_HOLE, a bad offset or
/// whence) from the held position, which the host then holds again.
fn host_seek(
    states: &mut Vec<(i32, State)>,
    fd: i32,
    current: i64,
    offset: i64,
    whence: i32,
) -> i64 {
    // SAFETY: plain lseeks of a descriptor this process holds.
    unsafe {
        if libc::lseek(fd, current, libc::SEEK_SET) != current {
            return -(crate::errno::last() as i64);
        }
        set(states, fd, None);
        crate::errno::check(libc::lseek(fd, offset, whence))
    }
}

/// `read` or `readv` of `fd` at its held offset; None when the layer holds
/// none for it.
pub fn read(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    if !held() {
        return None;
    }
    let mut states = STATES.lock().unwrap();
    let (_, State::Held(offset)) = states.iter_mut().find(|(known, _)| *known == fd)? else {
        return None;
    };
    // SAFETY: guest buffers, as the host read would take them.
    let n = unsafe { libc::preadv(fd, iov.as_ptr(), iov.len() as i32, *offset) };
    if n < 0 {
        return Some(-(crate::errno::last() as i64));
    }
    *offset += n as i64;
    Some(n as i64)
}

/// The host offset of `fd` when no other descriptor refers to its open
/// description (`PROC_FP_SHARED`: its fileglob has one reference).
fn unshared_offset(fd: i32) -> Option<i64> {
    const PROC_PIDFDVNODEINFO: i32 = 1;
    const PROC_FP_SHARED: u32 = 1;
    // struct vnode_fdinfo (176 bytes) begins with struct proc_fileinfo:
    // fi_openflags, fi_status, then the 64-bit fi_offset.
    let mut info = [0u64; 22];
    // SAFETY: a buffer of the flavour's size.
    let n = unsafe {
        libc::proc_pidfdinfo(
            libc::getpid(),
            fd,
            PROC_PIDFDVNODEINFO,
            info.as_mut_ptr().cast(),
            std::mem::size_of_val(&info) as i32,
        )
    };
    let status = (info[0] >> 32) as u32;
    (n == std::mem::size_of_val(&info) as i32 && status & PROC_FP_SHARED == 0)
        .then_some(info[1] as i64)
}
