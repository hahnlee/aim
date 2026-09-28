//! The writing end of an AIDL fast message queue (libfmq's
//! `AidlMessageQueue<T, SynchronizedReadWrite>`), and its event flag.
//!
//! The reader (sensorservice) creates the queue and sends its
//! `MQDescriptor`: grantors 0 and 1 are the 64-bit read and write counters,
//! grantor 2 the ring, grantor 3 the event flag word, each a range of one of
//! the handle's fds. The counters only grow; a position in the ring is the
//! counter modulo the ring size. The writer copies whole items in, then
//! publishes them by storing the new write counter with release ordering,
//! as `MessageQueueBase::write` does. A synchronized queue never
//! overwrites: a write that does not fit fails.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_common_fmq::aidl::android::hardware::common::fmq::GrantorDescriptor::GrantorDescriptor;

const READ_PTR: usize = 0;
const WRITE_PTR: usize = 1;
const DATA: usize = 2;
const EVENT_FLAG: usize = 3;

/// One `mmap` of a grantor's page-aligned range.
struct Mapping {
    base: *mut libc::c_void,
    len: usize,
    /// The grantor's first byte.
    at: *mut u8,
    extent: usize,
}

// SAFETY: shared memory accessed through atomics or under the queue's
// single writer.
unsafe impl Send for Mapping {}

impl Mapping {
    fn new(fd: BorrowedFd, g: &GrantorDescriptor) -> std::io::Result<Self> {
        let invalid = || std::io::Error::from_raw_os_error(libc::EINVAL);
        let (offset, extent) = (
            usize::try_from(g.offset).map_err(|_| invalid())?,
            usize::try_from(g.extent).map_err(|_| invalid())?,
        );
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let start = offset / page * page;
        let len = offset - start + extent;
        // SAFETY: a fresh shared mapping of the fd's range.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                start as libc::off_t,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            base,
            len,
            // SAFETY: within the mapping.
            at: unsafe { base.cast::<u8>().add(offset - start) },
            extent,
        })
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: our own mapping.
        unsafe { libc::munmap(self.base, self.len) };
    }
}

/// The writing end of a queue of `T`.
pub struct Writer<T> {
    read: Mapping,
    write: Mapping,
    ring: Mapping,
    flag: Option<Mapping>,
    _items: std::marker::PhantomData<T>,
}

impl<T> Writer<T> {
    /// Maps the queue a descriptor describes. `quantum` must be the size of
    /// `T`, or the two ends disagree on the layout.
    pub fn new(
        grantors: &[GrantorDescriptor],
        handle: &NativeHandle,
        quantum: i32,
    ) -> std::io::Result<Self> {
        let invalid = || std::io::Error::from_raw_os_error(libc::EINVAL);
        if quantum as usize != size_of::<T>() || grantors.len() <= DATA {
            return Err(invalid());
        }
        let map = |i: usize, min: usize| {
            let g = &grantors[i];
            let fd = handle.fds.get(g.fdIndex as usize).ok_or_else(invalid)?;
            let m = Mapping::new(fd.as_ref().as_fd(), g)?;
            if m.extent < min || m.at as usize % align_of::<u64>() != 0 {
                return Err(invalid());
            }
            Ok(m)
        };
        let ring = map(DATA, size_of::<T>())?;
        if ring.extent % size_of::<T>() != 0 {
            return Err(invalid());
        }
        Ok(Self {
            read: map(READ_PTR, 8)?,
            write: map(WRITE_PTR, 8)?,
            ring,
            flag: grantors
                .get(EVENT_FLAG)
                .map(|_| map(EVENT_FLAG, 4))
                .transpose()?,
            _items: std::marker::PhantomData,
        })
    }

    fn counter(m: &Mapping) -> &AtomicU64 {
        // SAFETY: an aligned 8-byte grantor, shared with the reader.
        unsafe { &*m.at.cast::<AtomicU64>() }
    }

    /// Appends `items`, all or nothing. Returns false when they do not fit.
    pub fn write(&self, items: &[T]) -> bool {
        let size = self.ring.extent as u64;
        let bytes = std::mem::size_of_val(items) as u64;
        let read = Self::counter(&self.read).load(Ordering::Acquire);
        let write = Self::counter(&self.write).load(Ordering::Relaxed);
        if write.wrapping_sub(read) > size || bytes > size - write.wrapping_sub(read) {
            return false;
        }
        let offset = (write % size) as usize;
        let first = (bytes as usize).min(self.ring.extent - offset);
        let src = items.as_ptr().cast::<u8>();
        // SAFETY: both pieces lie within the ring; the reader does not read
        // them until the counter below publishes them.
        unsafe {
            std::ptr::copy_nonoverlapping(src, self.ring.at.add(offset), first);
            std::ptr::copy_nonoverlapping(src.add(first), self.ring.at, bytes as usize - first);
        }
        Self::counter(&self.write).store(write.wrapping_add(bytes), Ordering::Release);
        true
    }

    /// Sets `bits` in the event flag and wakes its waiters, as libfmq's
    /// `EventFlag::wake` does.
    pub fn wake(&self, bits: u32) {
        let Some(flag) = &self.flag else { return };
        // SAFETY: an aligned 4-byte grantor, shared with the reader.
        let word = unsafe { &*flag.at.cast::<AtomicU32>() };
        let old = word.fetch_or(bits, Ordering::SeqCst);
        if !old & bits != 0 {
            // SAFETY: a futex wake on the shared word (not FUTEX_PRIVATE:
            // the waiter is another process).
            unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    word.as_ptr(),
                    libc::FUTEX_WAKE_BITSET,
                    i32::MAX,
                    std::ptr::null::<libc::timespec>(),
                    std::ptr::null::<u32>(),
                    bits,
                )
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use binder::ParcelFileDescriptor;
    use std::os::fd::{FromRawFd, OwnedFd};

    /// A queue of `slots` u64 items laid out as libfmq lays it out: the two
    /// counters, the ring and the flag in one memfd.
    fn queue(slots: i64) -> (Vec<GrantorDescriptor>, NativeHandle) {
        let grantor = |offset: i32, extent: i64| GrantorDescriptor {
            fdIndex: 0,
            offset,
            extent,
        };
        let grantors = vec![
            grantor(0, 8),
            grantor(8, 8),
            grantor(16, slots * 8),
            grantor(16 + slots as i32 * 8, 4),
        ];
        // SAFETY: a new memfd, owned by the handle.
        let fd = unsafe {
            let fd = libc::memfd_create(c"fmq".as_ptr(), 0);
            assert!(fd >= 0);
            assert_eq!(libc::ftruncate(fd, 4096), 0);
            OwnedFd::from_raw_fd(fd)
        };
        let handle = NativeHandle {
            fds: vec![ParcelFileDescriptor::new(fd)],
            ints: Vec::new(),
        };
        (grantors, handle)
    }

    #[test]
    fn writes_wrap_and_never_overwrite_unread_items() {
        let (grantors, handle) = queue(4);
        let q = Writer::<u64>::new(&grantors, &handle, 8).unwrap();
        // A second mapping plays the reader.
        let r = Writer::<u64>::new(&grantors, &handle, 8).unwrap();
        let ring = |i: usize| unsafe { *r.ring.at.cast::<u64>().add(i) };
        assert!(q.write(&[1, 2, 3]));
        assert!(!q.write(&[4, 5]), "only one slot is free");
        Writer::<u64>::counter(&r.read).store(16, Ordering::Release);
        assert!(q.write(&[4, 5, 6]));
        assert_eq!(Writer::<u64>::counter(&r.write).load(Ordering::Acquire), 48);
        assert_eq!([ring(0), ring(1), ring(2), ring(3)], [5, 6, 3, 4]);
        q.wake(1);
        let flag = unsafe { &*r.flag.as_ref().unwrap().at.cast::<AtomicU32>() };
        assert_eq!(flag.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_mismatched_quantum_is_refused() {
        let (grantors, handle) = queue(4);
        assert!(Writer::<u64>::new(&grantors, &handle, 96).is_err());
        assert!(Writer::<u64>::new(&grantors[..2], &handle, 8).is_err());
    }
}
