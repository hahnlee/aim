//! Level-trigger support for the Binder process todo queues.
//!
//! Producers increment a generation only after publishing work. A waiter takes
//! a snapshot, checks every eligible queue, then waits for the generation to
//! change. This closes the check-to-sleep lost-wakeup race without making the
//! signal itself the source of truth for work availability.

use std::{
    sync::{Condvar, Mutex},
    time::Duration,
};

#[derive(Debug, Default)]
struct State {
    generation: u64,
    closed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Signal {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    Changed,
    Closed,
    TimedOut,
}

impl Signal {
    /// Production Binder reads have no idle deadline. Notifications are only
    /// wakeups; the caller still checks the eligible work queues after waking.
    pub(crate) fn wait_after_indefinite(&self, observed: u64) -> WaitOutcome {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state = self
            .changed
            .wait_while(state, |state| !state.closed && state.generation == observed)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            WaitOutcome::Closed
        } else {
            WaitOutcome::Changed
        }
    }
    pub(crate) fn snapshot(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation
    }

    pub(crate) fn notify(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.generation = state.generation.wrapping_add(1);
        self.changed.notify_all();
    }

    pub(crate) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        self.changed.notify_all();
    }

    pub(crate) fn wait_after(&self, observed: u64, timeout: Duration) -> WaitOutcome {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return WaitOutcome::Closed;
        }
        if state.generation != observed {
            return WaitOutcome::Changed;
        }
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                !state.closed && state.generation == observed
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            WaitOutcome::Closed
        } else if state.generation != observed {
            WaitOutcome::Changed
        } else if timeout.timed_out() {
            WaitOutcome::TimedOut
        } else {
            WaitOutcome::Changed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn generation_closes_check_to_wait_race_and_close_wakes() {
        let signal = Signal::default();
        let generation = signal.snapshot();
        signal.notify();
        assert_eq!(
            signal.wait_after(generation, Duration::ZERO),
            WaitOutcome::Changed
        );
        let generation = signal.snapshot();
        assert_eq!(
            signal.wait_after(generation, Duration::ZERO),
            WaitOutcome::TimedOut
        );
        signal.close();
        assert_eq!(
            signal.wait_after(generation, Duration::ZERO),
            WaitOutcome::Closed
        );
    }

    #[test]
    fn waiter_observes_a_concurrent_publication() {
        let signal = Arc::new(Signal::default());
        let generation = signal.snapshot();
        let barrier = Arc::new(Barrier::new(2));
        let waiter = {
            let signal = Arc::clone(&signal);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                signal.wait_after(generation, Duration::from_secs(1))
            })
        };
        barrier.wait();
        signal.notify();
        assert_eq!(waiter.join().unwrap(), WaitOutcome::Changed);
    }

    #[test]
    fn indefinite_wait_has_no_idle_deadline_and_close_wakes() {
        use std::sync::mpsc;
        for close in [false, true] {
            let signal = Arc::new(Signal::default());
            let observed = signal.snapshot();
            let (done_tx, done_rx) = mpsc::channel();
            let waiter = {
                let signal = Arc::clone(&signal);
                std::thread::spawn(move || {
                    done_tx
                        .send(signal.wait_after_indefinite(observed))
                        .unwrap()
                })
            };
            assert!(matches!(
                done_rx.recv_timeout(Duration::from_millis(20)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            if close {
                signal.close();
            } else {
                signal.notify();
            }
            assert_eq!(
                done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                if close {
                    WaitOutcome::Closed
                } else {
                    WaitOutcome::Changed
                }
            );
            waiter.join().unwrap();
        }
        let signal = Signal::default();
        let observed = signal.snapshot();
        signal.notify();
        assert_eq!(signal.wait_after_indefinite(observed), WaitOutcome::Changed);
        signal.close();
        assert_eq!(signal.wait_after_indefinite(observed), WaitOutcome::Closed);
    }

    #[test]
    fn indefinite_waiters_ignore_spurious_wakes_and_all_observe_close() {
        use std::sync::mpsc;
        let signal = Arc::new(Signal::default());
        let observed = signal.snapshot();
        let (done_tx, done_rx) = mpsc::channel();
        let mut waiters = Vec::new();
        for _ in 0..4 {
            let signal = Arc::clone(&signal);
            let done_tx = done_tx.clone();
            waiters.push(std::thread::spawn(move || {
                done_tx
                    .send(signal.wait_after_indefinite(observed))
                    .unwrap()
            }));
        }
        // No new queue generation: a notification alone is not eligible work.
        signal.changed.notify_all();
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        signal.close();
        for _ in 0..4 {
            assert_eq!(
                done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                WaitOutcome::Closed
            );
        }
        for waiter in waiters {
            waiter.join().unwrap();
        }
    }
}
