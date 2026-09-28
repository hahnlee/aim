//! What the driver needs from the process that issues an ioctl, and from the
//! memory that backs a process's receive buffer.
//!
//! The driver never dereferences a guest address itself. The in-process
//! Linux syscall layer implements [`GuestProcess`] over the calling process's
//! address space and Linux fd table; a daemon-hosted driver implements it
//! over the bytes the caller's shim gathered (see the crate documentation).

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::any::Any;
use std::sync::Arc;

/// A positive Linux errno value.
pub type Errno = i32;

pub mod errno {
    use super::Errno;
    pub const EPERM: Errno = 1;
    pub const ESRCH: Errno = 3;
    pub const EINTR: Errno = 4;
    pub const EBADF: Errno = 9;
    pub const EAGAIN: Errno = 11;
    pub const ENOMEM: Errno = 12;
    pub const EFAULT: Errno = 14;
    pub const EBUSY: Errno = 16;
    pub const EINVAL: Errno = 22;
    pub const ENOSPC: Errno = 28;
    pub const EPROTO: Errno = 71;
}

/// An open file description being passed through binder. The driver only
/// moves it from the sender's fd table to the receiver's; what it is (a host
/// fd, a Mach fileport, an in-process object) belongs to the fd layer.
pub type File = Arc<dyn Any + Send + Sync>;

/// The Android identity a binder process was opened with. It comes from the
/// process registry (the uid the service or app was started as), never from
/// the host process's credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// The Linux tgid the guest sees for itself (`getpid()`).
    pub pid: i32,
    /// The Android uid reported as `sender_euid`.
    pub euid: u32,
    /// The SELinux context delivered with `BR_TRANSACTION_SEC_CTX`, for
    /// nodes that request it. `None` delivers no context.
    pub security_context: Option<String>,
}

/// The calling process, as seen from one ioctl.
pub trait GuestProcess {
    /// `copy_from_user`: fails with `EFAULT` for an unmapped range.
    fn copy_from_user(&mut self, address: u64, out: &mut [u8]) -> Result<(), Errno>;
    /// `copy_to_user`.
    fn copy_to_user(&mut self, address: u64, data: &[u8]) -> Result<(), Errno>;
    /// `fget`: the open file behind a guest fd (`EBADF` if none).
    fn get_file(&mut self, fd: u32) -> Result<File, Errno>;
    /// Install a received file as a new close-on-exec fd.
    fn install_file(&mut self, file: File) -> Result<u32, Errno>;
    /// Whether `count` files can be installed in this ioctl. A caller that
    /// prepares its fds before the ioctl (the daemon's clients) answers no
    /// when it prepared fewer: the transaction stays queued, the read
    /// returns, and the caller reads again with `count` fds ready.
    fn can_install(&mut self, _count: usize) -> bool {
        true
    }
    /// Close an fd the driver installed (fd arrays of a freed buffer).
    fn close_fd(&mut self, fd: u32);
}

/// The memory behind a process's binder mapping. The guest maps it
/// read-only; the driver writes each transaction into it exactly once.
pub trait ReceiveMemory: Send + Sync {
    /// Write `data` at `offset` into the mapping. The range is always inside
    /// a buffer the allocator handed out.
    fn write(&self, offset: usize, data: &[u8]);

    /// Zero a range (`TF_CLEAR_BUF`).
    fn clear(&self, offset: usize, len: usize) {
        self.write(offset, &vec![0; len]);
    }
}

/// A receive buffer in this address space: a page-aligned heap allocation
/// whose address is the guest-visible `vm_start`. It serves the in-process
/// harness, and it is the shape the in-process syscall layer uses when the
/// guest's binder mmap is plain host memory.
pub struct HeapReceiveMemory {
    base: *mut u8,
    len: usize,
}

// SAFETY: the allocation is owned by this value; writes go through raw
// pointers into ranges the driver's allocator hands out one at a time, under
// the driver's lock.
unsafe impl Send for HeapReceiveMemory {}
unsafe impl Sync for HeapReceiveMemory {}

const RECEIVE_ALIGN: usize = 16 * 1024;

impl HeapReceiveMemory {
    pub fn new(len: usize) -> Arc<Self> {
        let layout = Layout::from_size_align(len.max(1), RECEIVE_ALIGN).expect("receive layout");
        // SAFETY: the layout has a non-zero size.
        let base = unsafe { alloc_zeroed(layout) };
        assert!(!base.is_null(), "receive buffer allocation failed");
        Arc::new(Self { base, len })
    }

    /// The guest-visible start of the mapping.
    pub fn address(&self) -> u64 {
        self.base as u64
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl ReceiveMemory for HeapReceiveMemory {
    fn write(&self, offset: usize, data: &[u8]) {
        assert!(
            offset
                .checked_add(data.len())
                .is_some_and(|end| end <= self.len)
        );
        // SAFETY: bounds were checked above; the range lies in the allocation.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), self.base.add(offset), data.len()) };
    }

    fn clear(&self, offset: usize, len: usize) {
        assert!(offset.checked_add(len).is_some_and(|end| end <= self.len));
        // SAFETY: as above.
        unsafe { std::ptr::write_bytes(self.base.add(offset), 0, len) };
    }
}

impl Drop for HeapReceiveMemory {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.len.max(1), RECEIVE_ALIGN).unwrap();
        // SAFETY: allocated in `new` with the same layout.
        unsafe { dealloc(self.base, layout) };
    }
}
