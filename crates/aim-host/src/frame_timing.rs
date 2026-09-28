//! Low-overhead timing for the host/UI frame hand-off.
//!
//! The production path remains unchanged unless `AIM_DEBUG_FRAME_TIMING`
//! is present.  Counters are process-local because one Android application
//! process owns one host loop.  Keeping the measurement here avoids threading
//! timing state through every synthetic-input branch and gives the next
//! scheduler split a stable before/after contract.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static ENABLED: OnceLock<bool> = OnceLock::new();
static LOOPER_COUNT: AtomicU64 = AtomicU64::new(0);
static LOOPER_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static LOOPER_MAX_US: AtomicU64 = AtomicU64::new(0);
static GRAPHICS_COUNT: AtomicU64 = AtomicU64::new(0);
static GRAPHICS_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static GRAPHICS_MAX_US: AtomicU64 = AtomicU64::new(0);
static PULSE_COUNT: AtomicU64 = AtomicU64::new(0);
static PULSE_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static PULSE_MAX_US: AtomicU64 = AtomicU64::new(0);
static HANDOFF_OVER_16MS: AtomicU64 = AtomicU64::new(0);
static HANDOFF_OVER_50MS: AtomicU64 = AtomicU64::new(0);
static HANDOFF_OVER_500MS: AtomicU64 = AtomicU64::new(0);
static HANDOFF_OVER_1S: AtomicU64 = AtomicU64::new(0);
pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("AIM_DEBUG_FRAME_TIMING").is_some())
}

pub(crate) fn record_looper(micros: u64) {
    if !enabled() {
        return;
    }
    record(&LOOPER_COUNT, &LOOPER_TOTAL_US, &LOOPER_MAX_US, micros);
}

pub(crate) fn record_graphics(micros: u64) {
    if !enabled() {
        return;
    }
    record(
        &GRAPHICS_COUNT,
        &GRAPHICS_TOTAL_US,
        &GRAPHICS_MAX_US,
        micros,
    );
}

pub(crate) fn record_pulse(micros: u64) {
    if !enabled() {
        return;
    }
    record(&PULSE_COUNT, &PULSE_TOTAL_US, &PULSE_MAX_US, micros);
    if micros > 16_000 {
        HANDOFF_OVER_16MS.fetch_add(1, Ordering::Relaxed);
    }
    if micros > 50_000 {
        HANDOFF_OVER_50MS.fetch_add(1, Ordering::Relaxed);
    }
    if micros > 500_000 {
        HANDOFF_OVER_500MS.fetch_add(1, Ordering::Relaxed);
    }
    if micros > 1_000_000 {
        HANDOFF_OVER_1S.fetch_add(1, Ordering::Relaxed);
    }
}

fn record(count: &AtomicU64, total: &AtomicU64, max: &AtomicU64, micros: u64) {
    count.fetch_add(1, Ordering::Relaxed);
    total.fetch_add(micros, Ordering::Relaxed);
    let mut current = max.load(Ordering::Relaxed);
    while micros > current {
        match max.compare_exchange_weak(current, micros, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

pub(crate) fn report() {
    if !enabled() {
        return;
    }
    let looper = snapshot(&LOOPER_COUNT, &LOOPER_TOTAL_US, &LOOPER_MAX_US);
    let graphics = snapshot(&GRAPHICS_COUNT, &GRAPHICS_TOTAL_US, &GRAPHICS_MAX_US);
    let handoff = snapshot(&PULSE_COUNT, &PULSE_TOTAL_US, &PULSE_MAX_US);
    let over_16ms = HANDOFF_OVER_16MS.swap(0, Ordering::Relaxed);
    let over_50ms = HANDOFF_OVER_50MS.swap(0, Ordering::Relaxed);
    let over_500ms = HANDOFF_OVER_500MS.swap(0, Ordering::Relaxed);
    let over_1s = HANDOFF_OVER_1S.swap(0, Ordering::Relaxed);
    eprintln!(
        "AIM frame-timing looper=count:{} avg_us:{} max_us:{} graphics=count:{} avg_us:{} max_us:{} handoff=count:{} avg_us:{} max_us:{} over_16ms:{} over_50ms:{} over_500ms:{} over_1s:{}",
        looper.0,
        looper.1,
        looper.2,
        graphics.0,
        graphics.1,
        graphics.2,
        handoff.0,
        handoff.1,
        handoff.2,
        over_16ms,
        over_50ms,
        over_500ms,
        over_1s
    );
}

fn snapshot(count: &AtomicU64, total: &AtomicU64, max: &AtomicU64) -> (u64, u64, u64) {
    let count = count.swap(0, Ordering::Relaxed);
    let total = total.swap(0, Ordering::Relaxed);
    let max = max.swap(0, Ordering::Relaxed);
    (count, total.checked_div(count.max(1)).unwrap_or(0), max)
}
