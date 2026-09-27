//! The layer's own locks, held across a fork.
//!
//! A forked child is a copy of the calling thread only. A lock another
//! thread held at that moment stays held in the child forever, and the
//! child's next syscall that needs it (a `stat` reading fs-attrs, the
//! `execve` that follows every fork) hangs. `fork` therefore takes every
//! global lock of the layer first, all or none with a retry, so that no
//! lock order between them is assumed, and releases them in both
//! processes. The thread layer's locks have their own protocol
//! (`thread::fork_prepare`).
//!
//! A `Mutex` (a pthread mutex on Darwin) is simply unlocked in the child.
//! An `RwLock` is not: std's queue-based lock would wake the parent's
//! threads queued on it, through dispatch semaphores the child does not
//! have. The child gets a fresh lock around the same data instead.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock, TryLockError};

/// Releases one held lock; the argument is whether this is the child.
pub type Guard = Box<dyn FnOnce(bool)>;

/// Try `m`; a poisoned lock is taken as well.
pub fn mutex<T: 'static>(m: &'static Mutex<T>, held: &mut Vec<Guard>) -> bool {
    let g = match m.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return false,
    };
    held.push(Box::new(move |_| drop(g)));
    true
}

/// An `RwLock` a forked child can replace with a fresh one.
pub struct ForkRwLock<T>(UnsafeCell<RwLock<T>>);

// SAFETY: as RwLock; the cell is only written in a forked child, whose
// one thread holds the lock (`rwlock`).
unsafe impl<T: Send + Sync> Sync for ForkRwLock<T> {}

impl<T> ForkRwLock<T> {
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(RwLock::new(value)))
    }
}

impl<T: Default> Default for ForkRwLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> std::ops::Deref for ForkRwLock<T> {
    type Target = RwLock<T>;
    fn deref(&self) -> &RwLock<T> {
        // SAFETY: only replaced in a forked child with no other reference.
        unsafe { &*self.0.get() }
    }
}

/// Try `l` for writing; a poisoned lock is taken as well.
pub fn rwlock<T: Default + 'static>(l: &'static ForkRwLock<T>, held: &mut Vec<Guard>) -> bool {
    let mut g = match l.try_write() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return false,
    };
    held.push(Box::new(move |child| {
        if !child {
            drop(g);
            return;
        }
        let data = std::mem::take(&mut *g);
        std::mem::forget(g);
        // SAFETY: the child's only thread holds the lock, and the guard
        // that referred to it is gone.
        unsafe { l.0.get().write(RwLock::new(data)) };
    }));
    true
}

/// Every lock's `fork_try`, which pushes its guards or returns false.
const LOCKS: [fn(&mut Vec<Guard>) -> bool; 15] = [
    super::thread::fork_try,
    crate::patch::fork_try,
    crate::diag::fork_try,
    crate::vfs::fork_try,
    super::fdtab::fork_try,
    super::cred::fork_try,
    super::binder::fork_try,
    super::attrs::fork_try,
    super::wait::fork_try,
    super::memfd::fork_try,
    super::copies::fork_try,
    super::mem::fork_try,
    super::bpf::fork_try,
    super::ashmem::fork_try,
    super::selinuxfs::fork_try,
];

/// Whether a fork holds the locks: `pthread_atfork` child handlers, which
/// Darwin runs inside `fork()`, must leave their work to the fork path
/// then.
static HELD: AtomicBool = AtomicBool::new(false);

pub fn held() -> bool {
    HELD.load(Ordering::Acquire)
}

/// The layer's locks, held until `release`.
pub struct Held(Vec<Guard>);

impl Held {
    /// Release every lock, in the parent or in the child.
    pub fn release(self, child: bool) {
        for g in self.0.into_iter().rev() {
            g(child);
        }
        HELD.store(false, Ordering::Release);
    }
}

/// Take every lock, backing off while any is busy.
pub fn acquire() -> Held {
    super::thread::wait_exited();
    loop {
        let mut held: Vec<Guard> = Vec::new();
        if LOCKS.iter().all(|try_lock| try_lock(&mut held)) {
            HELD.store(true, Ordering::Release);
            return Held(held);
        }
        for g in held.into_iter().rev() {
            g(false);
        }
        std::thread::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static M: Mutex<u32> = Mutex::new(1);
    static L: ForkRwLock<Vec<u32>> = ForkRwLock::new(Vec::new());

    #[test]
    fn a_child_gets_a_fresh_rwlock_with_the_data() {
        L.write().unwrap().push(7);
        let mut held = Vec::new();
        assert!(mutex(&M, &mut held) && rwlock(&L, &mut held));
        assert!(M.try_lock().is_err() && L.try_read().is_err());
        Held(held).release(true);
        assert_eq!(*L.read().unwrap(), vec![7]);
        assert_eq!(*M.lock().unwrap(), 1);
    }
}
