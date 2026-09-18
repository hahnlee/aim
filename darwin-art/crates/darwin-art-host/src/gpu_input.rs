//! Native and synthetic Android input dispatch on the runtime owner thread.

use crate::config::HostError;
use crate::frame_timing;
use crate::runtime::HostRuntime;
use crate::surface::owned_surface_wait_slice;
use darwin_art_engine_sys::KeyEventV1;
use std::time::{Duration, Instant};

pub(super) fn pump_frame_with_latency(
    runtime: &mut HostRuntime,
    debug_latency: bool,
    last_input_dispatch: &mut Option<Instant>,
    frame_latencies_us: &mut Vec<u64>,
) -> Result<(), HostError> {
    let frame_started = frame_timing::enabled().then(Instant::now);
    pump_main_looper(runtime)?;
    let result = pulse_frame_with_latency(
        runtime,
        debug_latency,
        last_input_dispatch,
        frame_latencies_us,
    );
    if let Some(started) = frame_started {
        frame_timing::record_pulse(started.elapsed().as_micros() as u64);
    }
    result
}

pub(super) fn pulse_frame_with_latency(
    runtime: &mut HostRuntime,
    debug_latency: bool,
    last_input_dispatch: &mut Option<Instant>,
    frame_latencies_us: &mut Vec<u64>,
) -> Result<(), HostError> {
    let started = frame_timing::enabled().then(Instant::now);
    let pulse_status = runtime
        .graphics()
        .map_or(-1, |graphics| graphics.pump_frame(0));
    if let Some(started) = started {
        frame_timing::record_graphics(started.elapsed().as_micros() as u64);
    }
    if pulse_status != 0 {
        return Err(HostError::RuntimeFailed(pulse_status));
    }
    if debug_latency && let Some(dispatched_at) = last_input_dispatch.take() {
        frame_latencies_us.push(dispatched_at.elapsed().as_micros() as u64);
    }
    Ok(())
}

pub(super) fn pump_main_looper(runtime: &mut HostRuntime) -> Result<(), HostError> {
    let started = frame_timing::enabled().then(Instant::now);
    let status = runtime
        .graphics()
        .map_or(-1, darwin_art_engine::GraphicsSession::pump_main_looper);
    if let Some(started) = started {
        frame_timing::record_looper(started.elapsed().as_micros() as u64);
    }
    if status != 0 {
        return Err(HostError::RuntimeFailed(status));
    }
    Ok(())
}

pub(super) fn dispatch_synthetic_keys(
    runtime: &mut HostRuntime,
    sequence: &[(u32, u32)],
) -> Result<u64, HostError> {
    let interval = std::env::var("DARWIN_ART_TEST_KEY_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    eprintln!(
        "DARWIN_ART gpu synthetic key dispatch count={} interval_ms={}",
        sequence.len(),
        interval.as_millis()
    );
    let mut dispatched = 0_u64;
    for &(key_code, meta_state) in sequence {
        for action in [0_u32, 1_u32] {
            let mut event = KeyEventV1::default();
            event.version = 1;
            event.size = std::mem::size_of::<KeyEventV1>() as u32;
            event.action = action;
            // InputDispatcher marks hardware events as originating from the
            // system. Chromium requires that contract for focused web input.
            event.flags = 0x8;
            event.event_time_nanos = 0;
            event.down_time_nanos = 0;
            event.key_code = key_code;
            event.scan_code = linux_scan_code(key_code);
            event.meta_state = meta_state;
            event.device_id = 1;
            event.source = 0x101;
            let status = runtime
                .graphics()
                .map_or(-1, |graphics| graphics.dispatch_key_v1(&event));
            if status != 0 {
                return Err(HostError::RuntimeFailed(status));
            }
            dispatched += 1;
            if !interval.is_zero() {
                // Return to the owner Looper between hardware events so
                // Chromium can consume the renderer ACK gating the next key.
                let event_interval = (interval.as_secs_f64() / 2.0).max(0.001);
                let pump_status = owned_surface_wait_slice(runtime, event_interval);
                if pump_status != 0 {
                    return Err(HostError::SurfaceFailed {
                        operation: "gpu_test_key_interval_pump",
                        status: pump_status,
                    });
                }
                // Keep the test-only interval on the same owner-turn contract
                // as the product loop: drain framework/native Looper work
                // once, then publish a display edge. pump_frame deliberately
                // does not run a second opaque callback batch.
                pump_main_looper(runtime)?;
                let pulse_status = runtime
                    .graphics()
                    .map_or(-1, |graphics| graphics.pump_frame(0));
                if pulse_status != 0 {
                    return Err(HostError::RuntimeFailed(pulse_status));
                }
            }
        }
    }
    Ok(dispatched)
}

fn linux_scan_code(key_code: u32) -> u32 {
    match key_code {
        7 => 11,
        8..=16 => key_code - 6,
        29 => 30,
        30 => 48,
        31 => 46,
        32 => 32,
        33 => 18,
        34 => 33,
        35 => 34,
        36 => 35,
        37 => 23,
        38 => 36,
        39 => 37,
        40 => 38,
        41 => 50,
        42 => 49,
        43 => 24,
        44 => 25,
        45 => 16,
        46 => 19,
        47 => 31,
        48 => 20,
        49 => 22,
        50 => 47,
        51 => 17,
        52 => 45,
        53 => 21,
        54 => 44,
        55 => 51,
        56 => 52,
        62 => 57,
        66 => 28,
        67 => 14,
        68 => 41,
        69 => 12,
        70 => 13,
        71 => 26,
        72 => 27,
        73 => 43,
        74 => 39,
        75 => 40,
        76 => 53,
        111 => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::linux_scan_code;

    #[test]
    fn android_key_codes_map_to_linux_evdev_scan_codes() {
        assert_eq!(linux_scan_code(29), 30);
        assert_eq!(linux_scan_code(36), 35);
        assert_eq!(linux_scan_code(66), 28);
        assert_eq!(linux_scan_code(999), 0);
    }
}
