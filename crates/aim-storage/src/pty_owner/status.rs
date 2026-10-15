//! Read-only observations do not retain a guest endpoint capability.
use super::{Pair, failure};
use crate::private_fd::PrivateFd;
use std::{
    ffi::CString,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::atomic::{AtomicU64, Ordering},
};
const PAGE: usize = 4096;
const MAGIC: u64 = 0x32415454594d4941;
#[repr(C)]
struct Record {
    magic: u64,
    bits: AtomicU64,
    flags: AtomicU64,
}
pub(super) struct Status {
    _write: PrivateFd,
    read: PrivateFd,
    mapped: usize,
    name: CString,
    directory: PrivateFd,
}
pub struct ReadOnly {
    descriptor: PrivateFd,
    mapped: usize,
}
struct PendingName<'a> {
    directory: &'a PrivateFd,
    name: &'a CString,
    armed: bool,
}
impl Drop for PendingName<'_> {
    fn drop(&mut self) {
        if self.armed
            && unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) } < 0
        {
            eprintln!("PTY status cleanup: {}", io::Error::last_os_error());
        }
    }
}
unsafe impl Send for ReadOnly {}
unsafe impl Sync for ReadOnly {}
impl Status {
    pub(super) fn new(pair: &Pair, number: u32, flags: u64) -> io::Result<Self> {
        let name = CString::new(format!("status-{number}")).unwrap();
        let directory = pair.directory.fd.try_clone()?;
        let write = PrivateFd::allocate(|| {
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        })?;
        let mut pending = PendingName {
            directory: &directory,
            name: &name,
            armed: true,
        };
        if unsafe { libc::ftruncate(write.as_raw_fd(), PAGE as i64) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let read = PrivateFd::allocate(|| {
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        })?;
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                write.as_raw_fd(),
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            mapped.cast::<Record>().write(Record {
                magic: MAGIC,
                bits: AtomicU64::new(0),
                flags: AtomicU64::new(flags),
            });
        }
        pending.armed = false;
        drop(pending);
        Ok(Self {
            _write: write,
            read,
            mapped: mapped as usize,
            name,
            directory,
        })
    }
    pub(super) fn read_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.read.as_fd()
    }
    pub(super) fn publish(&self, hangup: bool, retired: bool, flags: u64) {
        let record = unsafe { &*(self.mapped as *const Record) };
        record.flags.store(flags, Ordering::Release);
        record.bits.store(
            u64::from(hangup) | u64::from(retired) << 1,
            Ordering::Release,
        );
    }
}
impl Drop for Status {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mapped as *mut _, PAGE);
            if libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) < 0 {
                eprintln!("PTY status cleanup: {}", io::Error::last_os_error());
            }
        }
    }
}
impl ReadOnly {
    pub fn adopt(descriptor: PrivateFd) -> io::Result<Self> {
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if flags & libc::O_ACCMODE != libc::O_RDONLY {
            return Err(failure(libc::EPERM));
        }
        let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(descriptor.as_raw_fd(), &mut metadata) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if metadata.st_size != PAGE as i64 {
            return Err(failure(libc::EPROTO));
        }
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                descriptor.as_raw_fd(),
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let value = Self {
            descriptor,
            mapped: mapped as usize,
        };
        if unsafe { (*(mapped.cast::<Record>())).magic } != MAGIC {
            return Err(failure(libc::EPROTO));
        }
        Ok(value)
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Self::adopt(self.descriptor.try_clone()?)
    }
    pub fn descriptor(&self) -> std::os::fd::BorrowedFd<'_> {
        use std::os::fd::AsFd;
        self.descriptor.as_fd()
    }
    pub fn hangup(&self) -> bool {
        unsafe { &*(self.mapped as *const Record) }
            .bits
            .load(Ordering::Acquire)
            & 1
            != 0
    }
    pub fn retired(&self) -> bool {
        unsafe { &*(self.mapped as *const Record) }
            .bits
            .load(Ordering::Acquire)
            & 2
            != 0
    }
    pub fn flags(&self) -> u64 {
        unsafe { &*(self.mapped as *const Record) }
            .flags
            .load(Ordering::Acquire)
    }
}
impl Drop for ReadOnly {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mapped as *mut _, PAGE);
        }
    }
}
