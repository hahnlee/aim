//! A display clock independent from the host event-pump loop.
//!
//! Android's DisplayEventReceiver keeps producing vsync edges even while the
//! app main thread is busy.  The old host loop derived vsync from the same
//! `Instant` polling loop that services AppKit and Binder, so a long turn also
//! delayed the next frame edge.  This clock only produces bounded, coalesced
//! edges; Android work still runs on the ART owner thread when the edge is
//! consumed.

use aim_engine::LooperWakeToken;
use aim_engine::ScanoutToken;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DISPLAY_INTERVAL: Duration = Duration::from_nanos(16_666_667);

pub(crate) struct FrameClock {
    ticks: Arc<Mutex<TickState>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

struct TickState {
    latest: Option<Instant>,
    pending: bool,
}

impl FrameClock {
    pub(crate) fn start(wake: Option<LooperWakeToken>, scanout: Option<ScanoutToken>) -> Self {
        let ticks = Arc::new(Mutex::new(TickState {
            latest: None,
            pending: false,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_ticks = Arc::clone(&ticks);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("aim-display-clock".to_owned())
            .spawn(move || run_clock(worker_ticks, worker_stop, wake, scanout))
            .expect("display clock thread must start");
        Self {
            ticks,
            stop,
            worker: Some(worker),
        }
    }

    /// Consume all pending edges and return only the newest one.
    ///
    /// A blocked UI thread must not receive a burst of stale frames after it
    /// becomes available; dropping intermediate edges matches Android's
    /// latest-vsync scheduling and keeps the queue bounded.
    pub(crate) fn take_latest(&self) -> Option<Instant> {
        let mut state = self.ticks.lock().expect("display clock state poisoned");
        let latest = state.latest.take();
        state.pending = false;
        latest
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_clock(
    ticks: Arc<Mutex<TickState>>,
    stop: Arc<AtomicBool>,
    wake: Option<LooperWakeToken>,
    scanout: Option<ScanoutToken>,
) {
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        if now < next {
            thread::sleep(next - now);
            continue;
        }
        // Keep the display cadence anchored to the previous deadline, as a
        // real DisplayEventReceiver does. Re-basing every tick on `now`
        // accumulates scheduler jitter and slowly phase-shifts scanout. If
        // this worker was actually delayed past one interval, skip the stale
        // deadline and resume one interval from the observation point rather
        // than emitting a burst of catch-up edges.
        let deadline = next;
        // Publish the newest edge. A single pending bit makes the wake token
        // edge-triggered: a blocked owner consumes one latest timestamp,
        // while intermediate vsyncs replace the stale value without filling
        // a queue or generating a wake storm.
        let should_wake = {
            let mut state = ticks.lock().expect("display clock state poisoned");
            state.latest = Some(now);
            if state.pending {
                false
            } else {
                state.pending = true;
                true
            }
        };
        if should_wake {
            if let Some(token) = wake {
                token.wake();
            }
        }
        // Scanout is a separate consumer from the ART owner pulse. It must
        // receive every display edge even when the owner already has a
        // pending wake (for example while a Chromium Looper callback is
        // busy). The native present_async path applies the ready-generation
        // gate and coalesces these calls into one bounded latest-wins AppKit
        // request; Android UI/JNI work stays on the owner thread.
        if let Some(token) = scanout {
            let _ = token.present();
        }
        let after = Instant::now();
        next = deadline
            .checked_add(DISPLAY_INTERVAL)
            .filter(|candidate| *candidate > after)
            .unwrap_or_else(|| after + DISPLAY_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameClock, ScanoutToken};
    use core::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    static SCANOUT_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn count_scanout(_: *mut c_void) -> i32 {
        SCANOUT_CALLS.fetch_add(1, Ordering::Relaxed);
        0
    }

    #[test]
    fn clock_is_bounded_and_produces_edges() {
        let clock = FrameClock::start(None, None);
        thread::sleep(Duration::from_millis(20));
        assert!(clock.take_latest().is_some());
        thread::sleep(Duration::from_millis(40));
        assert!(clock.take_latest().is_some());
        assert!(clock.take_latest().is_none());
    }

    #[test]
    fn scanout_ticks_continue_when_owner_wake_is_pending() {
        SCANOUT_CALLS.store(0, Ordering::Relaxed);
        // SAFETY: the test callback ignores the opaque handle and remains
        // valid for the clock's bounded lifetime.
        let token = unsafe { ScanoutToken::from_raw_for_test(std::ptr::null_mut(), count_scanout) };
        // Do not consume the owner edge. A coupled implementation would
        // invoke scanout only once because its pending bit stays set.
        let _clock = FrameClock::start(None, Some(token));
        thread::sleep(Duration::from_millis(55));
        assert!(SCANOUT_CALLS.load(Ordering::Relaxed) >= 2);
    }
}
