//! Rust-owned provider lease accounting.
//!
//! The host crate supplies the tiny unsafe FFI adapters.  This module owns
//! the state machine around them, so cross-thread ART callbacks cannot invent
//! a second count or clear hooks while a provider lease is still active.
//! Native install/uninstall callbacks are crossed only on the process-owner
//! transitions (zero-to-one and one-to-zero); individual ELF graphs retain
//! ordinary Rust leases without asking C++ to maintain a duplicate counter.

use crate::ProviderKind;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Condvar, Mutex};

pub struct ProviderLeaseTable {
    callbacks: ProviderCallbacks,
    state: Mutex<LeaseState>,
    quiescent: Condvar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderLeaseError {
    ActiveLeases,
    Poisoned,
    CallbackPanicked,
}

struct ProviderCallbacks {
    acquire: Box<dyn Fn(ProviderKind, i32) -> i32 + Send + Sync>,
    release: Box<dyn Fn(ProviderKind) -> i32 + Send + Sync>,
    clear: Box<dyn Fn() + Send + Sync>,
}

struct LeaseState {
    counts: [u32; 8],
    /// A provider transition is reserved before crossing the native ABI. This
    /// keeps concurrent graph loads from both observing a zero count and
    /// installing the same process-global provider.
    transitioning: [bool; 8],
    in_flight: usize,
    clearing: bool,
    /// Once the process-global callback table has been cleared, this bridge
    /// can never be reactivated.  Keeping that fact in the Rust state machine
    /// makes repeated shutdown/drop paths idempotent instead of crossing the
    /// foreign clear ABI more than once.
    cleared: bool,
}

impl ProviderLeaseTable {
    pub fn new<A, R, C>(acquire: A, release: R, clear: C) -> Self
    where
        A: Fn(ProviderKind, i32) -> i32 + Send + Sync + 'static,
        R: Fn(ProviderKind) -> i32 + Send + Sync + 'static,
        C: Fn() + Send + Sync + 'static,
    {
        Self {
            callbacks: ProviderCallbacks {
                acquire: Box::new(acquire),
                release: Box::new(release),
                clear: Box::new(clear),
            },
            state: Mutex::new(LeaseState {
                counts: [0; 8],
                transitioning: [false; 8],
                in_flight: 0,
                clearing: false,
                cleared: false,
            }),
            quiescent: Condvar::new(),
        }
    }

    pub fn acquire(&self, kind: ProviderKind, authority_fd: i32) -> i32 {
        let index = kind.index();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return -1,
        };
        loop {
            if state.clearing || state.cleared {
                return -1;
            }
            if state.transitioning[index] {
                state = match self.quiescent.wait(state) {
                    Ok(state) => state,
                    Err(_) => return -1,
                };
                continue;
            }
            if state.counts[index] != 0 {
                if state.counts[index] == u32::MAX {
                    return -1;
                }
                state.counts[index] += 1;
                return 0;
            }
            state.transitioning[index] = true;
            state.in_flight += 1;
            break;
        }
        drop(state);

        // Never invoke foreign code while holding the lease mutex. A provider
        // callback may re-enter another provider route on the same ART thread.
        let status = catch_unwind(AssertUnwindSafe(|| {
            (self.callbacks.acquire)(kind, authority_fd)
        }))
        .unwrap_or(-1);
        let Ok(mut state) = self.state.lock() else {
            return -1;
        };
        state.in_flight = state.in_flight.saturating_sub(1);
        if status == 0 {
            state.counts[index] = 1;
        }
        state.transitioning[index] = false;
        self.quiescent.notify_all();
        status
    }

    pub fn release(&self, kind: ProviderKind) -> i32 {
        let index = kind.index();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return -1,
        };
        loop {
            if state.clearing || state.cleared {
                return -1;
            }
            if state.transitioning[index] {
                state = match self.quiescent.wait(state) {
                    Ok(state) => state,
                    Err(_) => return -1,
                };
                continue;
            }
            if state.counts[index] == 0 {
                return -1;
            }
            if state.counts[index] > 1 {
                state.counts[index] -= 1;
                return 0;
            }
            state.counts[index] = 0;
            state.transitioning[index] = true;
            state.in_flight += 1;
            break;
        }
        drop(state);

        // Reserve the lease before dropping the lock; this prevents clear()
        // from observing a false quiescent state while the callback runs.
        let status =
            catch_unwind(AssertUnwindSafe(|| (self.callbacks.release)(kind))).unwrap_or(-1);
        let Ok(mut state) = self.state.lock() else {
            return -1;
        };
        state.in_flight = state.in_flight.saturating_sub(1);
        if status != 0 {
            state.counts[index] = 1;
        }
        state.transitioning[index] = false;
        self.quiescent.notify_all();
        status
    }

    pub fn clear(&self) -> Result<(), ProviderLeaseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ProviderLeaseError::Poisoned)?;
        if state.cleared {
            return Ok(());
        }
        if state.clearing {
            return Err(ProviderLeaseError::ActiveLeases);
        }
        state.clearing = true;
        while state.in_flight != 0 || state.transitioning.iter().any(|active| *active) {
            state = self
                .quiescent
                .wait(state)
                .map_err(|_| ProviderLeaseError::Poisoned)?;
        }
        if state.counts.iter().any(|count| *count != 0) {
            state.clearing = false;
            self.quiescent.notify_all();
            return Err(ProviderLeaseError::ActiveLeases);
        }
        drop(state);

        // The owner callback is deliberately outside the mutex. Reentrant
        // clear/acquire calls fail closed through `clearing` instead of
        // deadlocking the owner thread.
        let callback_result = catch_unwind(AssertUnwindSafe(|| (self.callbacks.clear)()));
        let mut state = self
            .state
            .lock()
            .map_err(|_| ProviderLeaseError::Poisoned)?;
        if callback_result.is_ok() {
            state.cleared = true;
        }
        state.clearing = false;
        self.quiescent.notify_all();
        callback_result
            .map(|_| ())
            .map_err(|_| ProviderLeaseError::CallbackPanicked)
    }

    /// Synchronize accounting after native ART has already released direct
    /// process-owner resources during DestroyJavaVM. Rust still clears the
    /// callback table, but must not invoke release callbacks a second time.
    pub fn adopt_native_shutdown(&self) -> Result<(), ProviderLeaseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ProviderLeaseError::Poisoned)?;
        if state.clearing
            || state.in_flight != 0
            || state.transitioning.iter().any(|active| *active)
        {
            return Err(ProviderLeaseError::ActiveLeases);
        }
        state.counts = [0; 8];
        self.quiescent.notify_all();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    #[test]
    fn rejects_invalid_and_duplicate_release() {
        let acquired = Arc::new(AtomicU32::new(0));
        let released = Arc::new(AtomicU32::new(0));
        let acquire_count = Arc::clone(&acquired);
        let release_count = Arc::clone(&released);
        let table = ProviderLeaseTable::new(
            move |_, _| {
                acquire_count.fetch_add(1, Ordering::Relaxed);
                0
            },
            move |_| {
                release_count.fetch_add(1, Ordering::Relaxed);
                0
            },
            || {},
        );
        assert!(ProviderKind::try_from(0).is_err());
        assert_eq!(table.acquire(ProviderKind::Filesystem, -1), 0);
        assert_eq!(table.release(ProviderKind::Filesystem), 0);
        assert_eq!(table.release(ProviderKind::Filesystem), -1);
        assert_eq!(acquired.load(Ordering::Relaxed), 1);
        assert_eq!(released.load(Ordering::Relaxed), 1);
        assert!(table.clear().is_ok());
    }

    #[test]
    fn clear_waits_for_all_leases() {
        let table = ProviderLeaseTable::new(|_, _| 0, |_| 0, || {});
        assert_eq!(table.acquire(ProviderKind::Network, 7), 0);
        assert_eq!(table.clear(), Err(ProviderLeaseError::ActiveLeases));
        assert_eq!(table.release(ProviderKind::Network), 0);
        assert_eq!(table.clear(), Ok(()));
    }

    #[test]
    fn shared_leases_cross_native_boundary_once() {
        let acquired = Arc::new(AtomicU32::new(0));
        let released = Arc::new(AtomicU32::new(0));
        let acquire_count = Arc::clone(&acquired);
        let release_count = Arc::clone(&released);
        let table = ProviderLeaseTable::new(
            move |_, _| {
                acquire_count.fetch_add(1, Ordering::SeqCst);
                0
            },
            move |_| {
                release_count.fetch_add(1, Ordering::SeqCst);
                0
            },
            || {},
        );

        assert_eq!(table.acquire(ProviderKind::Filesystem, 3), 0);
        assert_eq!(table.acquire(ProviderKind::Filesystem, 4), 0);
        assert_eq!(acquired.load(Ordering::SeqCst), 1);
        assert_eq!(table.release(ProviderKind::Filesystem), 0);
        assert_eq!(released.load(Ordering::SeqCst), 0);
        assert_eq!(table.release(ProviderKind::Filesystem), 0);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(table.clear(), Ok(()));
    }

    #[test]
    fn clear_is_idempotent_and_closes_the_acquire_boundary() {
        let clear_calls = Arc::new(AtomicU32::new(0));
        let clear_calls_in_callback = Arc::clone(&clear_calls);
        let table = ProviderLeaseTable::new(
            |_, _| 0,
            |_| 0,
            move || {
                clear_calls_in_callback.fetch_add(1, Ordering::SeqCst);
            },
        );

        assert_eq!(table.clear(), Ok(()));
        assert_eq!(table.clear(), Ok(()));
        assert_eq!(clear_calls.load(Ordering::SeqCst), 1);
        assert_eq!(table.acquire(ProviderKind::Filesystem, -1), -1);
        assert_eq!(table.release(ProviderKind::Filesystem), -1);
    }

    #[test]
    fn foreign_acquire_callback_can_reenter_without_deadlock() {
        use std::sync::{Arc, Weak, atomic::AtomicBool};

        let entered = Arc::new(AtomicBool::new(false));
        let entered_in_callback = Arc::clone(&entered);
        let table: Arc<ProviderLeaseTable> = Arc::new_cyclic(|weak| {
            let weak: Weak<ProviderLeaseTable> = weak.clone();
            ProviderLeaseTable::new(
                move |kind, _| {
                    if kind == ProviderKind::Network
                        && !entered_in_callback.swap(true, Ordering::SeqCst)
                    {
                        weak.upgrade()
                            .expect("table alive")
                            .acquire(ProviderKind::Ioctl, -1)
                    } else {
                        0
                    }
                },
                |_| 0,
                || {},
            )
        });
        assert_eq!(table.acquire(ProviderKind::Network, -1), 0);
        assert!(entered.load(Ordering::SeqCst));
        assert_eq!(table.release(ProviderKind::Ioctl), 0);
        assert_eq!(table.release(ProviderKind::Network), 0);
        assert_eq!(table.clear(), Ok(()));
    }

    #[test]
    fn callback_panics_fail_closed_without_poisoning_lease_state() {
        let table = ProviderLeaseTable::new(
            |_, _| panic!("foreign acquire panic"),
            |_| panic!("foreign release panic"),
            || panic!("foreign clear panic"),
        );
        assert_eq!(table.acquire(ProviderKind::Filesystem, -1), -1);
        assert_eq!(table.release(ProviderKind::Filesystem), -1);
        assert_eq!(table.clear(), Err(ProviderLeaseError::CallbackPanicked));

        // The table remains usable after the callback boundary recovers.
        assert_eq!(table.acquire(ProviderKind::Filesystem, -1), -1);
    }
}
