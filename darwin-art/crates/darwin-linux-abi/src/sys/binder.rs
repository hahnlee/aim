//! The binder device nodes `/dev/binder`, `/dev/hwbinder` and
//! `/dev/vndbinder` (#167), backed by the driver in the binder host daemon
//! (`darwin-binder-host`, `linux-run --binder NAME`).
//!
//! A binder fd is a host socket whose peer is the daemon: epoll and poll see
//! it readable while a read would find work, and its last close (including
//! process exit) releases the binder process. ioctl, mmap and poll
//! registration go to the daemon.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use darwin_binder_driver::Device;
use darwin_binder_host::client::{BinderFile, Client, UserMemory};

use crate::errno::{EINVAL, ENODEV, ENOMEM, EPERM};

static CLIENT: OnceLock<Client> = OnceLock::new();
/// Open binder files by the inode of their socket, so dup'ed fds resolve.
static FILES: Mutex<Option<HashMap<u64, BinderFile>>> = Mutex::new(None);

/// Connect to the daemon serving bootstrap name `name`.
pub fn init(name: &str) -> Result<(), String> {
    let client =
        Client::connect(name).ok_or_else(|| format!("binder host '{name}' is not running"))?;
    let _ = CLIENT.set(client);
    Ok(())
}

fn inode(fd: i32) -> Option<u64> {
    // SAFETY: fstat into a local buffer.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(fd, &mut st) } == 0 && st.st_mode & libc::S_IFMT == libc::S_IFSOCK)
        .then_some(st.st_ino)
}

fn lookup(fd: i32) -> Option<BinderFile> {
    let files = FILES.lock().unwrap();
    let files = files.as_ref()?;
    files.get(&inode(fd)?).copied()
}

struct Guest;

impl UserMemory for Guest {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, i32> {
        let mut v = vec![0u8; len];
        // SAFETY: guest memory the guest passed to the ioctl.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, v.as_mut_ptr(), len) };
        Ok(v)
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), i32> {
        // SAFETY: as above.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
        Ok(())
    }
}

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;

/// Whether `guest_path` is a binder device node that exists.
pub fn is_device(guest_path: &str) -> bool {
    CLIENT.get().is_some() && Device::from_path(guest_path).is_some()
}

/// `openat` of a binder device node; None for any other path.
pub fn open(guest_path: &str, flags: u64) -> Option<i64> {
    let device = Device::from_path(guest_path)?;
    let Some(client) = CLIENT.get() else {
        return Some(-(ENODEV as i64));
    };
    let euid = super::process::getuid() as u32;
    let file = client.open(
        device,
        flags & O_NONBLOCK != 0,
        flags & O_CLOEXEC != 0,
        euid,
        &super::procfs::security_context(),
    );
    Some(match file {
        Ok(f) => {
            let Some(ino) = inode(f.fd) else {
                return Some(-(ENOMEM as i64));
            };
            FILES
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .insert(ino, f);
            f.fd as i64
        }
        Err(e) => -(e as i64),
    })
}

/// Binder ioctls are `_IOC(dir, 'b', nr, size)`.
pub fn ioctl(fd: i32, cmd: u64, arg: u64) -> Option<i64> {
    if (cmd >> 8) & 0xff != u64::from(b'b') {
        return None;
    }
    let file = lookup(fd)?;
    let tid = super::process::gettid() as i32;
    Some(match file.ioctl(tid, cmd as u32, arg, &mut Guest) {
        Ok(()) => 0,
        Err(e) => -(e as i64),
    })
}

const PROT_WRITE: u64 = 2;
const MAP_FIXED: u64 = 0x10;
const PAGE: u64 = 16384;

/// `mmap` of a binder fd: the read-only receive buffer.
pub fn mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: i32) -> Option<i64> {
    let file = lookup(fd)?;
    if prot & PROT_WRITE != 0 {
        return Some(-(EPERM as i64));
    }
    let len = (len + PAGE - 1) & !(PAGE - 1);
    if len == 0 {
        return Some(-(EINVAL as i64));
    }
    let hflags = libc::MAP_PRIVATE
        | libc::MAP_ANON
        | if flags & MAP_FIXED != 0 {
            libc::MAP_FIXED
        } else {
            0
        };
    // SAFETY: reserving the range the receive buffer will replace.
    let base = unsafe { libc::mmap(addr as *mut _, len as usize, libc::PROT_NONE, hflags, -1, 0) };
    if base == libc::MAP_FAILED {
        return Some(-(ENOMEM as i64));
    }
    let base = base as u64;
    Some(match file.mmap(base, len) {
        Ok(()) => base as i64,
        Err(e) => {
            // SAFETY: undoing our reservation.
            unsafe { libc::munmap(base as *mut _, len as usize) };
            -(e as i64)
        }
    })
}

/// The calling thread starts polling `fd` (epoll_ctl, poll), as Linux calls
/// `binder_poll` from those paths.
pub fn poll(fd: i32) {
    if let Some(file) = lookup(fd) {
        let _ = file.poll(super::process::gettid() as i32);
    }
}
