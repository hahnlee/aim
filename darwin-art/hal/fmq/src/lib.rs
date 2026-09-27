//! AIDL fast message queues (`android.hardware.common.fmq`) in Rust, laid
//! out as libfmq's `MessageQueueBase` lays them out, so a peer using the
//! original libfmq maps and uses them unchanged:
//!
//! - one shared memory file holding, at 8-byte aligned offsets, the read
//!   counter (u64), the write counter (u64), the ring and, optionally, the
//!   EventFlag word (u32), described by one grantor each;
//! - the counters are byte positions since creation; the ring offset is the
//!   position modulo the ring size;
//! - the synchronized read/write flavour: a writer never overruns the
//!   reader;
//! - blocking reads and writes use the EventFlag protocol (`EventFlag.cpp`):
//!   the writer sets `NOT_EMPTY` and futex-wakes, the reader `NOT_FULL`.
//!
//! [`Queue::new`] creates a queue (the side that hands out the
//! descriptor); [`Queue::from_descriptor`] maps a peer's.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::sync::atomic::{AtomicU32, AtomicU64};

use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_common_fmq::aidl::android::hardware::common::fmq::{
    GrantorDescriptor::GrantorDescriptor, MQDescriptor::MQDescriptor,
};
use binder::ParcelFileDescriptor;

/// `MessageQueueBase::EventFlagBits`.
const NOT_FULL: u32 = 0x01;
const NOT_EMPTY: u32 = 0x02;
/// `MQFlavor::kSynchronizedReadWrite`, the descriptor's `flags`.
const SYNCHRONIZED_READ_WRITE: i32 = 0x01;

const FUTEX_WAIT_BITSET: i32 = 9;
const FUTEX_WAKE_BITSET: i32 = 10;

fn align8(n: usize) -> usize {
    (n + 7) & !7
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why.to_string())
}

/// A queue of bytes; [`TypedQueue`] carries fixed-size elements.
pub struct Queue {
    fd: OwnedFd,
    base: *mut u8,
    length: usize,
    /// Offsets of the read and write counters, the ring and the EventFlag
    /// word (`None` without one).
    read: usize,
    write: usize,
    ring: usize,
    flag: Option<usize>,
    /// Ring size in bytes.
    size: usize,
    quantum: usize,
}

// SAFETY: the mapping is shared memory accessed through atomics and the
// single-producer/single-consumer protocol.
unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

fn map(fd: &OwnedFd, length: usize) -> io::Result<*mut u8> {
    // SAFETY: a shared mapping of a descriptor we hold.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(base.cast())
}

impl Queue {
    /// A queue of `count` elements of `quantum` bytes, in a new memfd.
    pub fn new(quantum: usize, count: usize, event_flag: bool) -> io::Result<Self> {
        let size = quantum * count;
        let meta = 16 + if event_flag { 4 } else { 0 };
        // SAFETY: plain libc calls; the fd is owned from here on.
        let (fd, length) = unsafe {
            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let length = (align8(size) + meta).div_ceil(page) * page;
            let fd = libc::memfd_create(c"MessageQueue".as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(fd);
            if libc::ftruncate(fd.as_raw_fd(), length as i64) != 0 {
                return Err(io::Error::last_os_error());
            }
            (fd, length)
        };
        let base = map(&fd, length)?;
        Ok(Self {
            fd,
            base,
            length,
            read: 0,
            write: 8,
            ring: 16,
            flag: event_flag.then_some(align8(16 + size)),
            size,
            quantum,
        })
    }

    /// Map the queue a peer described. All grantors must be in its first
    /// file, as libfmq lays out queues it allocates itself.
    pub fn from_descriptor<T, F>(desc: &MQDescriptor<T, F>) -> io::Result<Self> {
        let g = &desc.grantors;
        if g.len() < 3
            || g.iter()
                .any(|g| g.fdIndex != 0 || g.offset < 0 || g.extent < 0)
        {
            return Err(invalid("unsupported grantors"));
        }
        let fd = desc
            .handle
            .fds
            .first()
            .ok_or_else(|| invalid("no fd"))?
            .as_ref()
            .try_clone()?;
        let end = g
            .iter()
            .map(|g| g.offset as usize + g.extent as usize)
            .max()
            .unwrap();
        let base = map(&fd, end)?;
        Ok(Self {
            fd,
            base,
            length: end,
            read: g[0].offset as usize,
            write: g[1].offset as usize,
            ring: g[2].offset as usize,
            flag: g.get(3).map(|g| g.offset as usize),
            size: g[2].extent as usize,
            quantum: desc.quantum as usize,
        })
    }

    fn at(&self, offset: usize) -> *mut u8 {
        // SAFETY: offsets come from the layout, within the mapping.
        unsafe { self.base.add(offset) }
    }

    fn read_counter(&self) -> &AtomicU64 {
        // SAFETY: an aligned u64 inside the mapping.
        unsafe { AtomicU64::from_ptr(self.at(self.read).cast()) }
    }

    fn write_counter(&self) -> &AtomicU64 {
        // SAFETY: as above.
        unsafe { AtomicU64::from_ptr(self.at(self.write).cast()) }
    }

    fn flag(&self) -> Option<&AtomicU32> {
        // SAFETY: an aligned u32 inside the mapping.
        self.flag
            .map(|f| unsafe { AtomicU32::from_ptr(self.at(f).cast()) })
    }

    /// The descriptor the peer builds its queue from.
    pub fn descriptor<T: Default, F: Default>(&self) -> io::Result<MQDescriptor<T, F>> {
        let mut grantors = vec![
            grantor(self.read, 8),
            grantor(self.write, 8),
            grantor(self.ring, self.size),
        ];
        if let Some(f) = self.flag {
            grantors.push(grantor(f, 4));
        }
        let mut desc = MQDescriptor::<T, F>::default();
        desc.grantors = grantors;
        desc.handle = NativeHandle {
            fds: vec![ParcelFileDescriptor::new(self.fd.try_clone()?)],
            ints: vec![],
        };
        desc.quantum = self.quantum as i32;
        desc.flags = SYNCHRONIZED_READ_WRITE;
        Ok(desc)
    }

    /// Ring size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    pub fn available_to_read(&self) -> usize {
        let w = self.write_counter().load(Acquire);
        let r = self.read_counter().load(Acquire);
        w.wrapping_sub(r) as usize
    }

    pub fn available_to_write(&self) -> usize {
        self.size.saturating_sub(self.available_to_read())
    }

    /// Copy between the ring at byte position `at` and `buf`, wrapping.
    fn copy(&self, at: u64, buf: *mut u8, len: usize, to_ring: bool) {
        let offset = (at % self.size as u64) as usize;
        let first = len.min(self.size - offset);
        // SAFETY: both spans lie within the ring and within `buf`.
        unsafe {
            let (a, b) = (self.at(self.ring + offset), self.at(self.ring));
            if to_ring {
                std::ptr::copy_nonoverlapping(buf, a, first);
                std::ptr::copy_nonoverlapping(buf.add(first), b, len - first);
            } else {
                std::ptr::copy_nonoverlapping(a, buf, first);
                std::ptr::copy_nonoverlapping(b, buf.add(first), len - first);
            }
        }
    }

    /// Read exactly `buf.len()` bytes, or nothing.
    pub fn read(&self, buf: &mut [u8]) -> bool {
        if buf.is_empty() || self.available_to_read() < buf.len() {
            return buf.is_empty();
        }
        let r = self.read_counter().load(Relaxed);
        self.copy(r, buf.as_mut_ptr(), buf.len(), false);
        self.read_counter().store(r + buf.len() as u64, Release);
        true
    }

    /// Write exactly `buf.len()` bytes, or nothing.
    pub fn write(&self, buf: &[u8]) -> bool {
        if buf.is_empty() || self.available_to_write() < buf.len() {
            return buf.is_empty();
        }
        let w = self.write_counter().load(Relaxed);
        self.copy(w, buf.as_ptr().cast_mut(), buf.len(), true);
        self.write_counter().store(w + buf.len() as u64, Release);
        true
    }

    /// `MessageQueueBase::readBlocking` with the default notification
    /// bits: read, or wait for `NOT_EMPTY` and retry; then wake the writer.
    pub fn read_blocking(&self, buf: &mut [u8]) -> bool {
        while !self.read(buf) {
            if !self.wait(NOT_EMPTY) {
                return false;
            }
        }
        self.wake(NOT_FULL);
        true
    }

    /// `MessageQueueBase::writeBlocking`, the mirror of [`Self::read_blocking`].
    pub fn write_blocking(&self, buf: &[u8]) -> bool {
        while !self.write(buf) {
            if !self.wait(NOT_FULL) {
                return false;
            }
        }
        self.wake(NOT_EMPTY);
        true
    }

    /// `EventFlag::wake`: set the bits; futex-wake only when one was clear.
    fn wake(&self, bits: u32) {
        let Some(flag) = self.flag() else { return };
        let old = flag.fetch_or(bits, SeqCst);
        if !old & bits != 0 {
            // SAFETY: a futex word in the shared mapping.
            unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    flag.as_ptr(),
                    FUTEX_WAKE_BITSET,
                    i32::MAX,
                    0usize,
                    0usize,
                    bits,
                )
            };
        }
    }

    /// `EventFlag::wait` with retries on spurious wakeups: consume `bits`,
    /// or sleep until one is set. False without an EventFlag word or on an
    /// unexpected error.
    fn wait(&self, bits: u32) -> bool {
        let Some(flag) = self.flag() else {
            return false;
        };
        loop {
            let old = flag.fetch_and(!bits, SeqCst);
            if old & bits != 0 {
                return true;
            }
            // SAFETY: a futex word in the shared mapping.
            let r = unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    flag.as_ptr(),
                    FUTEX_WAIT_BITSET,
                    old & !bits,
                    0usize,
                    0usize,
                    bits,
                )
            };
            if r != 0 {
                let e = io::Error::last_os_error().raw_os_error();
                if e != Some(libc::EAGAIN) && e != Some(libc::EINTR) {
                    log::error!("fmq: futex wait failed: {e:?}");
                    return false;
                }
            }
        }
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        // SAFETY: our mapping.
        unsafe { libc::munmap(self.base.cast(), self.length) };
    }
}

fn grantor(offset: usize, extent: usize) -> GrantorDescriptor {
    GrantorDescriptor {
        fdIndex: 0,
        offset: offset as i32,
        extent: extent as i64,
    }
}

/// A queue of one element type: a fixed-size parcelable, `#[repr(C)]` as
/// the peer's C++ lays it out, with no padding left implicit.
pub struct TypedQueue<T> {
    queue: Queue,
    _t: std::marker::PhantomData<T>,
}

impl<T: Copy> TypedQueue<T> {
    /// A queue of `count` elements with an EventFlag word.
    pub fn new(count: usize) -> io::Result<Self> {
        Ok(Self {
            queue: Queue::new(std::mem::size_of::<T>(), count, true)?,
            _t: std::marker::PhantomData,
        })
    }

    pub fn from_descriptor<D, F>(desc: &MQDescriptor<D, F>) -> io::Result<Self> {
        let queue = Queue::from_descriptor(desc)?;
        if queue.quantum != std::mem::size_of::<T>() {
            return Err(invalid("element size mismatch"));
        }
        Ok(Self {
            queue,
            _t: std::marker::PhantomData,
        })
    }

    pub fn descriptor<D: Default, F: Default>(&self) -> io::Result<MQDescriptor<D, F>> {
        self.queue.descriptor()
    }

    pub fn read_blocking(&self) -> Option<T> {
        let mut value = std::mem::MaybeUninit::<T>::zeroed();
        // SAFETY: T is plain data; the peer writes whole Ts.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                value.as_mut_ptr().cast::<u8>(),
                std::mem::size_of::<T>(),
            )
        };
        // SAFETY: as above; every byte was written.
        self.queue
            .read_blocking(bytes)
            .then(|| unsafe { value.assume_init() })
    }

    pub fn write_blocking(&self, value: &T) -> bool {
        // SAFETY: T is plain data without implicit padding.
        let bytes = unsafe {
            std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
        };
        self.queue.write_blocking(bytes)
    }
}
