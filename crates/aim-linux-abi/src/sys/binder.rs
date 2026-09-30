//! The binder device nodes `/dev/binder`, `/dev/hwbinder` and
//! `/dev/vndbinder` (#167), backed by the driver in the binder host daemon
//! (`aim-binder-host`, `linux-run --binder NAME`).
//!
//! A binder fd is a host socket whose peer is the daemon: epoll and poll see
//! it readable while a read would find work, and its last close (including
//! process exit) releases the binder process. ioctl, mmap and poll
//! registration go to the daemon. The fd table (`fdtab`) holds its file, so
//! dup'ed fds resolve and a closed one no longer does.

use std::sync::{Mutex, OnceLock};

use aim_binder_driver::Device;
use aim_binder_host::client::{BinderFile, Client, UserMemory};

use super::fdtab::Kind;
use crate::errno::{EINTR, EINVAL, ENODEV, ENOMEM, EPERM};

/// The daemon's bootstrap name, to reconnect after fork.
static NAME: OnceLock<String> = OnceLock::new();
static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

/// The files whose pages the daemon's process shares (`sharedfile`); none
/// without a daemon.
pub fn shared_files() -> Vec<aim_binder_host::wire::SharedFile> {
    CLIENT
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|c| c.shared_files().ok())
        .unwrap_or_default()
}

/// execve in place: the calling thread's client state is the old image's.
pub fn exec_reset() {
    aim_binder_host::client::forget_thread();
}

/// Connect to the daemon serving bootstrap name `name`.
pub fn init(name: &str) -> Result<(), String> {
    let client =
        Client::connect(name).ok_or_else(|| format!("binder host '{name}' is not running"))?;
    *CLIENT.lock().unwrap() = Some(client);
    let _ = NAME.set(name.to_string());
    Ok(())
}

fn lookup(fd: i32) -> Option<BinderFile> {
    match super::fdtab::get(fd)? {
        Kind::Binder(file) => Some(file),
        _ => None,
    }
}

struct Guest;

/// Nothing maps below 4 GiB on macOS arm64 (`__PAGEZERO`), so a pointer
/// there, null included, is EFAULT as copy_from_user would make it. A copy
/// of nothing touches no memory and succeeds whatever the pointer, as on
/// Linux: an empty Parcel (a ping, a void reply) has a null data pointer.
fn check_user(address: u64, len: usize) -> Result<(), i32> {
    if len > 0 && address < 1 << 32 {
        Err(14)
    } else {
        Ok(())
    }
}

impl UserMemory for Guest {
    fn read(&mut self, address: u64, len: usize) -> Result<Vec<u8>, i32> {
        check_user(address, len)?;
        let mut v = vec![0u8; len];
        // SAFETY: guest memory the guest passed to the ioctl.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, v.as_mut_ptr(), len) };
        Ok(v)
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), i32> {
        check_user(address, data.len())?;
        // SAFETY: as above.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
        Ok(())
    }

    fn installed(&mut self, fd: i32) {
        // A received SEQPACKET or datagram socket (an InputChannel) or
        // memfd keeps its Linux semantics.
        super::fdtab::on_close(fd);
        super::fdtab::adopt(fd);
    }

    fn closed(&mut self, fd: i32) {
        super::fdtab::on_close(fd);
    }
}

const O_NONBLOCK: u64 = 0o4000;
const O_CLOEXEC: u64 = 0o2000000;

/// The device a guest path names: `/dev/binder` and the others, or their
/// binderfs nodes `/dev/binderfs/<name>`. init mounts binderfs there and
/// symlinks the former to them; this layer plays that mount.
fn device(guest_path: &str) -> Option<Device> {
    match guest_path.strip_prefix("/dev/binderfs/") {
        Some(name) => Device::from_path(&format!("/dev/{name}")),
        None => Device::from_path(guest_path),
    }
}

/// Whether `guest_path` is a binder device node that exists.
pub fn is_device(guest_path: &str) -> bool {
    CLIENT.lock().unwrap().is_some() && device(guest_path).is_some()
}

/// `openat` of a binder device node; None for any other path.
pub fn open(guest_path: &str, flags: u64) -> Option<i64> {
    let device = device(guest_path)?;
    let client = CLIENT.lock().unwrap();
    let Some(client) = client.as_ref() else {
        return Some(-(ENODEV as i64));
    };
    let euid = super::cred::getuid(175) as u32;
    let file = client.open(
        device,
        flags & O_NONBLOCK != 0,
        flags & O_CLOEXEC != 0,
        euid,
        &super::cred::seclabel(),
    );
    Some(match file {
        Ok(f) => {
            super::fdtab::insert(f.fd, Kind::Binder(f));
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
    // A guest signal for this thread interrupts a read parked in the
    // daemon, as `binder_wait_for_work` returns on a pending signal; the
    // syscall layer restarts the ioctl when no handler is to run.
    let interrupt = move || {
        let _ = file.interrupt(tid);
    };
    let r =
        super::signal::interruptible(&interrupt, || file.ioctl(tid, cmd as u32, arg, &mut Guest));
    Some(match r {
        None => -(EINTR as i64),
        Some(Ok(())) => 0,
        Some(Err(e)) => -(e as i64),
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
    let hflags = libc::MAP_PRIVATE | libc::MAP_ANON;
    let base = if flags & MAP_FIXED != 0 {
        // SAFETY: reserving the range the receive buffer will replace.
        let b = unsafe {
            libc::mmap(
                addr as *mut _,
                len as usize,
                libc::PROT_NONE,
                hflags | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if b == libc::MAP_FAILED {
            return Some(-(ENOMEM as i64));
        }
        b as u64
    } else {
        match super::arena::map(
            super::arena::hint(addr, len),
            len,
            libc::PROT_NONE,
            hflags,
            -1,
            0,
        ) {
            Ok(b) => b,
            Err(_) => return Some(-(ENOMEM as i64)),
        }
    };
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
