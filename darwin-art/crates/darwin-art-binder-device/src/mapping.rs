//! Darwin backing for Binder receive memory: writable transport view and
//! read-only client view of the same pages. No Parcel allocation policy here.
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;

#[cfg(test)]
pub(crate) static MAPPING_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct View {
    address: NonNull<u8>,
    length: usize,
}

// SAFETY: `View` has unique ownership of one mmap lifetime. Moving that
// ownership between threads does not invalidate the mapping, and all mutation
// of the transport view is reached through `&mut ReceiveMapping` (the shared
// Binder process owner places it behind a mutex). `View` is intentionally not
// `Sync`; an unguarded shared view must never become a writable alias.
unsafe impl Send for View {}

impl View {
    fn map(fd: &OwnedFd, length: usize, protection: i32) -> io::Result<Self> {
        // SAFETY: kernel chooses address; fd is owned and sized by constructor.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                protection,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        if let Some(address) = NonNull::new(address.cast()) {
            Ok(Self { address, length })
        } else {
            // SAFETY: successful mapping must be released even at address zero.
            unsafe {
                libc::munmap(address, length);
            }
            Err(io::Error::other("null mapping address"))
        }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: this owner has exactly one live mapping of this length.
        unsafe {
            libc::munmap(self.address.as_ptr().cast(), self.length);
        }
    }
}

pub struct ReceiveMapping {
    backing: OwnedFd,
    writer: View,
    reader: View,
}

impl ReceiveMapping {
    pub fn new(length: usize) -> io::Result<Self> {
        if length == 0 || length > isize::MAX as usize {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let backing = anonymous_backing()?;
        // SAFETY: owned descriptor, checked positive length fits signed off_t.
        if unsafe { libc::ftruncate(backing.as_raw_fd(), length as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let writer = View::map(&backing, length, libc::PROT_READ | libc::PROT_WRITE)?;
        let reader = View::map(&backing, length, libc::PROT_READ)?;
        // Mapping lifetime is independent of the fd. No backing descriptor or
        // persistent shm name survives construction, including error paths.
        Ok(Self {
            backing,
            writer,
            reader,
        })
    }

    pub fn client_address(&self) -> usize {
        self.reader.address.as_ptr() as usize
    }

    pub fn duplicate_backing(&self) -> io::Result<OwnedFd> {
        self.backing.try_clone()
    }

    pub fn bytes(&self) -> &[u8] {
        // SAFETY: live initialized read-only mapping. Writes require &mut self,
        // so safe callers cannot mutate while this borrowed slice exists.
        unsafe { std::slice::from_raw_parts(self.reader.address.as_ptr(), self.reader.length) }
    }

    pub fn write(&mut self, offset: usize, data: &[u8]) -> io::Result<()> {
        let end = offset
            .checked_add(data.len())
            .filter(|end| *end <= self.writer.length)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        let _ = end;
        // SAFETY: validated range, exclusive owner, input cannot alias a safe
        // borrow of this mapping while &mut self is held.
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.writer.address.as_ptr().add(offset),
                data.len(),
            );
        }
        Ok(())
    }

    pub(crate) fn clear(&mut self, offset: usize, length: usize) -> io::Result<()> {
        offset
            .checked_add(length)
            .filter(|end| *end <= self.writer.length)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        // SAFETY: checked live mapping range and exclusive owner.
        unsafe {
            std::ptr::write_bytes(self.writer.address.as_ptr().add(offset), 0, length);
        }
        Ok(())
    }
}

fn anonymous_backing() -> io::Result<OwnedFd> {
    loop {
        let mut random = [0u8; 8];
        // SAFETY: initialized writable buffer, under getentropy's size limit.
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let name = CString::new(format!("/dart-bd-{:016x}", u64::from_ne_bytes(random))).unwrap();
        // POSIX shm_open sets FD_CLOEXEC atomically; exclusive private name,
        // mode0600. Never truncate or unlink someone else's existing object.
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EEXIST) {
                continue;
            }
            return Err(error);
        }
        // SAFETY: successful shm_open transfers exclusive descriptor ownership.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: remove only the exact name exclusively created above. Object
        // lives through the fd and subsequently mappings, not a disk file.
        if unsafe { libc::shm_unlink(name.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(fd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod protection;
    #[test]
    fn shared_views_and_bounds() {
        let _guard = MAPPING_TEST_LOCK.lock().unwrap();
        let mut map = ReceiveMapping::new(16385).unwrap();
        assert_ne!(map.client_address(), map.writer.address.as_ptr() as usize);
        assert!(map.bytes().iter().all(|byte| *byte == 0));
        map.write(16381, b"abcd").unwrap();
        assert_eq!(&map.bytes()[16381..], b"abcd");
        map.write(16381, b"wxyz").unwrap();
        assert_eq!(&map.bytes()[16381..], b"wxyz");
        assert!(map.write(usize::MAX, b"x").is_err());
        assert!(map.write(16385, b"x").is_err());
        map.write(16385, b"").unwrap();
    }

    #[test]
    fn duplicated_backing_accepts_shared_host_device_mapping() {
        let _guard = MAPPING_TEST_LOCK.lock().unwrap();
        let map = ReceiveMapping::new(1024 * 1024).unwrap();
        let duplicate = map.duplicate_backing().unwrap();
        // ProcessState reserves two guard pages from its 1 MiB arena. Android
        // requests MAP_PRIVATE from the Binder driver, but Darwin POSIX shared
        // memory rejects that combination. The VM boundary deliberately maps
        // this exact special-device backing MAP_SHARED so delivered transaction
        // bytes remain visible in the client's read-only view.
        let length = 1024 * 1024 - 2 * 16 * 1024;
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_SHARED,
                duplicate.as_raw_fd(),
                0,
            )
        };
        assert_ne!(
            address,
            libc::MAP_FAILED,
            "{}",
            std::io::Error::last_os_error()
        );
        assert_eq!(unsafe { libc::munmap(address, length) }, 0);
    }
    #[test]
    fn backing_is_cloexec_and_invalid_sizes_rejected() {
        let _guard = MAPPING_TEST_LOCK.lock().unwrap();
        let fd = anonymous_backing().unwrap();
        // SAFETY: query live owned descriptor.
        assert_ne!(
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert!(ReceiveMapping::new(0).is_err());
        assert!(ReceiveMapping::new(usize::MAX).is_err());
    }
}
