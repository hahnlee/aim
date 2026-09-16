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
}
