//! Native publication counter shared with the SystemServer replica.
use std::{
    ffi::CString,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    sync::atomic::{AtomicU64, Ordering},
};
const SIZE: usize = 4096;
pub(crate) struct VersionPage {
    base: *mut AtomicU64,
    reader: OwnedFd,
}
// SAFETY: the mapping is aligned, retained until drop, and accessed atomically.
unsafe impl Send for VersionPage {}
unsafe impl Sync for VersionPage {}
impl VersionPage {
    pub(crate) fn new(version: u64) -> std::io::Result<Self> {
        let name = CString::new(format!("/aim-pm-{:x}-{:x}", std::process::id(), unsafe {
            libc::arc4random()
        }))
        .unwrap();
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let writer = unsafe { OwnedFd::from_raw_fd(fd) };
        let result = (|| {
            if unsafe { libc::ftruncate(writer.as_raw_fd(), SIZE as libc::off_t) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let reader = unsafe { OwnedFd::from_raw_fd(fd) };
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    SIZE,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    writer.as_raw_fd(),
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error());
            }
            let page = Self {
                base: base.cast(),
                reader,
            };
            page.publish(version);
            Ok(page)
        })();
        let unlink = unsafe { libc::shm_unlink(name.as_ptr()) };
        if unlink != 0 {
            return Err(std::io::Error::last_os_error());
        }
        result
    }
    pub(crate) fn publish(&self, version: u64) {
        unsafe { &*self.base }.store(version.to_le(), Ordering::Release);
    }
    pub(crate) fn file(&self) -> std::io::Result<aim_binder_driver::File> {
        aim_binder_host::server::file_from_fd(self.reader.as_fd())
            .ok_or_else(|| std::io::Error::other("package version fileport failed"))
    }
}
impl Drop for VersionPage {
    fn drop(&mut self) {
        self.publish(0);
        unsafe {
            libc::munmap(self.base.cast(), SIZE);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_is_shared_but_descriptor_cannot_write() {
        let page = VersionPage::new(7).unwrap();
        let fd = aim_binder_host::server::file_fd(&page.file().unwrap()).unwrap();
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED);
        let read = || u64::from_le(unsafe { &*base.cast::<AtomicU64>() }.load(Ordering::Acquire));
        assert_eq!(read(), 7);
        page.publish(9);
        assert_eq!(read(), 9);
        assert_eq!(
            unsafe { libc::pwrite(fd.as_raw_fd(), [1u8].as_ptr().cast(), 1, 0) },
            -1
        );
        drop(page);
        assert_eq!(read(), 0);
        unsafe {
            libc::munmap(base, SIZE);
        }
    }
}
