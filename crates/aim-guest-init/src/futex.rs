//! Cross-process futex wakes on `MAP_SHARED` memory.
//!
//! bionic's property readers sleep in `__system_property_wait` with a
//! non-private `FUTEX_WAIT_BITSET` on a serial word of a `MAP_SHARED`
//! mapping of `/dev/__properties__/<area>`. The syscall layer turns that
//! wait into Darwin's `__ulock_wait2(UL_COMPARE_AND_WAIT_SHARED, ...)`
//! because the flavour follows the mapping type
//! (`experiments/p0/03-futex/README.md`: SHARED and non-shared waiters live
//! in separate namespaces, and a SHARED key is the backing object and
//! offset, not the address). The writer here therefore wakes with the same
//! SHARED flavour through its own mapping of the same file.

use std::sync::atomic::AtomicU32;

/// `UL_COMPARE_AND_WAIT_SHARED` (xnu `sys/ulock.h`).
const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
const ULF_WAKE_ALL: u32 = 0x0000_0100;
const ULF_NO_ERRNO: u32 = 0x0100_0000;

unsafe extern "C" {
    fn __ulock_wait2(
        operation: u32,
        addr: *mut libc::c_void,
        value: u64,
        timeout_ns: u64,
        value2: u64,
    ) -> i32;
    fn __ulock_wake(operation: u32, addr: *mut libc::c_void, wake_value: u64) -> i32;
}

/// The shared-memory futex operations aimd needs from the host.
///
/// `word` must lie in a `MAP_SHARED` mapping; the wake reaches waiters that
/// map the same object at any address, in any process.
pub trait SharedFutex: Send + Sync {
    /// Wakes every waiter on `word` (`FUTEX_WAKE` with `INT32_MAX`).
    fn wake_all(&self, word: &AtomicU32);
}

/// [`SharedFutex`] over `__ulock_wake(UL_COMPARE_AND_WAIT_SHARED |
/// ULF_WAKE_ALL)`, the flavour `os_sync_wake_by_address_all` with
/// `OS_SYNC_WAKE_BY_ADDRESS_SHARED` uses.
#[derive(Clone, Copy, Debug, Default)]
pub struct UlockShared;

impl SharedFutex for UlockShared {
    fn wake_all(&self, word: &AtomicU32) {
        // SAFETY: the word is valid memory; a wake with no waiters returns
        // ENOENT, which is not an error for a broadcast.
        unsafe {
            __ulock_wake(
                UL_COMPARE_AND_WAIT_SHARED | ULF_WAKE_ALL | ULF_NO_ERRNO,
                word.as_ptr().cast(),
                0,
            );
        }
    }
}

/// What [`wait_shared`] observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitResult {
    /// Woken (or spuriously returned); re-read the word.
    Woken,
    /// The word no longer held `expected` when the wait began.
    ValueChanged,
    TimedOut,
    Interrupted,
}

/// A SHARED-flavour wait on `word` while it holds `expected`, as the
/// syscall layer performs it for a guest `FUTEX_WAIT` on shared memory.
/// `timeout_ns == 0` waits forever. Used by tests and by aimd's own
/// readers.
pub fn wait_shared(word: &AtomicU32, expected: u32, timeout_ns: u64) -> WaitResult {
    if word.load(std::sync::atomic::Ordering::Acquire) != expected {
        return WaitResult::ValueChanged;
    }
    // SAFETY: waiting on a valid word.
    let r = unsafe {
        __ulock_wait2(
            UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO,
            word.as_ptr().cast(),
            expected as u64,
            timeout_ns,
            0,
        )
    };
    if r >= 0 {
        return WaitResult::Woken;
    }
    match -r {
        libc::ETIMEDOUT => WaitResult::TimedOut,
        libc::EINTR => WaitResult::Interrupted,
        libc::EFAULT => WaitResult::ValueChanged,
        _ => WaitResult::Woken,
    }
}
