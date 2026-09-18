use crate::config::{HostError, HostOutcome, RunOptions};
use crate::frame_clock::FrameClock;
use crate::frame_timing;
use crate::gpu_input::{
    dispatch_synthetic_keys, pulse_frame_with_latency, pump_frame_with_latency, pump_main_looper,
};
use crate::gpu_test_config::GpuTestConfig;
use crate::runtime::HostRuntime;
use crate::surface::owned_surface_wait_slice;
use darwin_art_engine_sys::{PointerEventV2, ProcessResult};
use darwin_art_runtime::Subsystem;
use std::time::{Duration, Instant};

const OWNER_WAIT_MAX: Duration = Duration::from_millis(16);

#[cfg(target_os = "macos")]
pub(super) fn run(
    runtime: &mut HostRuntime,
    process: ProcessResult,
    options: &RunOptions,
    graphics_attached: bool,
) -> Result<HostOutcome, HostError> {
    let debug_boundaries = std::env::var_os("DARWIN_ART_DEBUG_FRAME_TIMING").is_some();
    if debug_boundaries {
        eprintln!(
            "DARWIN_ART gpu-loop enter visible_seconds={}",
            options.visible_seconds
        );
    }
    // Only the wake token crosses into the display-clock helper. The opaque
    // GraphicsSession remains owned and consumed by this ART owner thread.
    let owner_wake = runtime
        .graphics()
        .and_then(|graphics| graphics.looper_wake_token());
    let scanout = runtime
        .surface()
        .and_then(|surface| surface.scanout_token());
    let frame_clock = FrameClock::start(owner_wake, scanout);
    if runtime.surface().is_none() {
        return Err(HostError::SurfaceFailed {
            operation: "gpu_active_surface",
            status: -1,
        });
    }
    // Graphics is normally already leased before process execution. The
    // surface lease is attached afterward, making it the newest lease so
    // shutdown removes the surface before graphics in strict LIFO order.
    if graphics_attached && !runtime.subsystem_active(Subsystem::Graphics) {
        match runtime.install_subsystem(Subsystem::Graphics) {
            Ok(_) => {}
            Err(error) => return Err(HostError::RuntimeFailed(error.status() as i32)),
        }
    }
    let mut frames_presented = 1_u64;
    let mut loop_error: Option<HostError> = None;
    // Benchmark mode collects the same owner-thread latency samples as the
    // diagnostic switch, but deliberately does not enable the native bridge's
    // per-event logging. This keeps a repeated-input run representative of a
    // release launch while still emitting one aggregate summary at shutdown.
    let benchmark_latency = std::env::var_os("DARWIN_ART_BENCHMARK").is_some();
    let debug_latency =
        std::env::var_os("DARWIN_ART_DEBUG_INPUT_LATENCY").is_some() || benchmark_latency;
    let mut last_input_dispatch: Option<Instant> = None;
    let mut frame_latencies_us = Vec::new();
    let test_config = GpuTestConfig::from_env();
    let replay_standalone_pointer = test_config.standalone_pointer_replay();
    let GpuTestConfig {
        resize: test_resize,
        resize_after_ms: test_resize_after_ms,
        pointer: test_pointer,
        drag: test_drag,
        post_sequence_drag,
        tap_sequence: test_tap_sequence,
        pointer_sequence_post_delay_ms,
        post_drag_tap_sequence,
        hold_ms: test_hold_ms,
        cancel: test_cancel,
        pointer_hz: test_pointer_hz,
        key_sequence: test_key_sequence,
        post_pointer_key_sequence,
        post_pointer_key_delay_ms,
    } = test_config;
    let mut pending_test_resize = test_resize;
    if test_resize_after_ms.is_none()
        && let Some((width, height)) = pending_test_resize.take()
    {
        let status = runtime
            .surface()
            .map_or(-1, |surface| surface.resize(width, height));
        if status != 0 {
            loop_error = Some(HostError::SurfaceFailed {
                operation: "gpu_test_window_resize",
                status,
            });
        } else {
            eprintln!("DARWIN_ART gpu test resize={width}x{height}");
        }
    }
    // Keep native and synthetic input on the same owner thread as ART and the
    // Metal surface. The host loop is only the display-clock/event source: a
    // pump publishes a pending vsync, then Android's normal
    // DisplayEventReceiver -> Choreographer -> ViewRootImpl -> ThreadedRenderer
    // path decides whether there is damage to record and submit. Product APKs
    // are never redrawn by a host-side View.draw() poll.
    // Acceptance scripts may need ordinary taps to reach a gesture surface
    // first (for example selecting Calendar's Month view). This remains the
    // same MotionEvent ABI path; it only schedules a drag after the tap list.
    // Test-only multi-tap input uses the same MotionEvent/DecorView route as
    // native mouse input. Samples after the first wait for their third field
    // (milliseconds) before dispatch: "x,y,0;x,y,500".
    let mut synthetic_moves = 0_u64;
    // Test coordinates are expressed in the same logical AppKit points used
    // by mouse events and capture scripts. Android lays the retained view out
    // in backing pixels, so apply the live launch scale before entering the
    // common MotionEvent bridge. Real NSEvents receive the equivalent
    // drawable-size transform in DarwinArtMetalView.
    let synthetic_pointer_scale = std::env::var("DARWIN_ART_WINDOW_SCALE")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0);
    eprintln!(
        "DARWIN_ART gpu test pointer={test_pointer:?} drag_points={} tap_sequence={} hold_ms={test_hold_ms} cancel={test_cancel} hz={test_pointer_hz:?} keys={}",
        test_drag.as_ref().map_or(0, Vec::len),
        test_tap_sequence.as_ref().map_or(0, Vec::len),
        test_key_sequence.as_ref().map_or(0, Vec::len),
    );

    let synthetic_event = |action: u32, x: f32, y: f32| {
        let mut event = PointerEventV2::default();
        event.version = 2;
        event.size = std::mem::size_of::<PointerEventV2>() as u32;
        event.action = action;
        // Zero delegates timestamping to the native input bridge, which uses
        // Android's suspend-excluding uptime domain. An Instant elapsed from
        // this loop's start is not Android uptime and is treated as stale by
        // ViewRoot's input stages.
        event.event_time_nanos = 0;
        event.down_time_nanos = 0;
        event.pointer_count = 1;
        event.x = x * synthetic_pointer_scale;
        event.y = y * synthetic_pointer_scale;
        event.raw_x = event.x;
        event.raw_y = event.y;
        event.pressure = 1.0;
        event.size_value = 1.0;
        event
    };

    // WindowManager assigns focus when the ViewRoot is attached. Drain that
    // focus/touch-mode work before admitting the first host event, preserving
    // Android InputDispatcher's focus-before-input ordering. This is also
    // important for synthetic acceptance tests, which otherwise can inject a
    // DOWN in the same host turn that made the window visible.
    if loop_error.is_none()
        && let Err(error) = pump_frame_with_latency(
            runtime,
            debug_latency,
            &mut last_input_dispatch,
            &mut frame_latencies_us,
        )
    {
        loop_error = Some(error);
    }

    if loop_error.is_none()
        && let Some((x, y)) = test_pointer
    {
        let down = synthetic_event(0, x, y);
        let dispatch_status = runtime
            .graphics()
            .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&down));
        if dispatch_status != 0 {
            loop_error = Some(HostError::RuntimeFailed(dispatch_status));
        } else {
            if debug_latency {
                last_input_dispatch = Some(Instant::now());
            }
            frames_presented += 1;
            if let Err(error) = pump_frame_with_latency(
                runtime,
                debug_latency,
                &mut last_input_dispatch,
                &mut frame_latencies_us,
            ) {
                loop_error = Some(error);
            }
            let mut held_ms = 0_u64;
            while held_ms < test_hold_ms && loop_error.is_none() {
                let slice_ms = (test_hold_ms - held_ms).min(16);
                let pump_status = owned_surface_wait_slice(runtime, slice_ms as f64 / 1000.0);
                if pump_status == 7 {
                    break;
                }
                if pump_status != 0 {
                    loop_error = Some(HostError::SurfaceFailed {
                        operation: "gpu_test_pointer_pump",
                        status: pump_status,
                    });
                    break;
                }
                if let Err(error) = pump_main_looper(runtime) {
                    loop_error = Some(error);
                }
                if loop_error.is_none() {
                    // ACTION_MOVE is dispatched before the pulse so the
                    // next framework frame observes the newest pointer
                    // state, matching a native input/vsync handoff.
                    let moves_this_slice = test_pointer_hz
                        .map(|hz| ((slice_ms * u64::from(hz) + 999) / 1000).max(1))
                        .unwrap_or(1);
                    for move_offset in 0..moves_this_slice {
                        let move_index = (held_ms / 16 + move_offset + 1) as usize;
                        let (move_x, move_y) = test_drag
                            .as_ref()
                            .and_then(|points| points.get(move_index).copied())
                            .unwrap_or((x, y));
                        let move_event = synthetic_event(2, move_x, move_y);
                        let dispatch_status = runtime
                            .graphics()
                            .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&move_event));
                        if dispatch_status != 0 {
                            loop_error = Some(HostError::RuntimeFailed(dispatch_status));
                            break;
                        }
                        if debug_latency {
                            last_input_dispatch = Some(Instant::now());
                        }
                        synthetic_moves += 1;
                    }
                    if loop_error.is_none()
                        && let Err(error) = pump_frame_with_latency(
                            runtime,
                            debug_latency,
                            &mut last_input_dispatch,
                            &mut frame_latencies_us,
                        )
                    {
                        loop_error = Some(error);
                    }
                }
                held_ms += slice_ms;
            }
            if loop_error.is_none() {
                let (up_x, up_y) = test_drag
                    .as_ref()
                    .and_then(|points| points.last().copied())
                    .unwrap_or((x, y));
                let up = synthetic_event(if test_cancel { 3 } else { 1 }, up_x, up_y);
                let dispatch_status = runtime
                    .graphics()
                    .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&up));
                if dispatch_status != 0 {
                    loop_error = Some(HostError::RuntimeFailed(dispatch_status));
                } else {
                    if debug_latency {
                        last_input_dispatch = Some(Instant::now());
                    }
                    frames_presented += 1;
                    if let Err(error) = pump_frame_with_latency(
                        runtime,
                        debug_latency,
                        &mut last_input_dispatch,
                        &mut frame_latencies_us,
                    ) {
                        loop_error = Some(error);
                    }
                }
            }
        }
    }
    if loop_error.is_none()
        && let Some(key_sequence) = test_key_sequence
    {
        match dispatch_synthetic_keys(runtime, &key_sequence) {
            Ok(dispatched) => frames_presented += dispatched,
            Err(error) => loop_error = Some(error),
        }
        if loop_error.is_none()
            && let Err(error) = pump_frame_with_latency(
                runtime,
                debug_latency,
                &mut last_input_dispatch,
                &mut frame_latencies_us,
            )
        {
            loop_error = Some(error);
        }
    }
    if loop_error.is_none()
        && let Some(sequence) = test_tap_sequence.as_ref()
    {
        for &(x, y, delay_ms) in sequence.iter().skip(1) {
            let wait_started = Instant::now();
            while wait_started.elapsed() < Duration::from_millis(delay_ms) && loop_error.is_none() {
                let remaining =
                    Duration::from_millis(delay_ms).saturating_sub(wait_started.elapsed());
                // AppKit runs on its own actor; Handler/Binder work on the
                // Android owner must not be throttled to display rate. Service
                // the owner Looper at a short cadence and publish
                // Choreographer vsync only at the display cadence.
                let slice_ms = remaining.as_millis().min(OWNER_WAIT_MAX.as_millis()).max(1) as u64;
                let pump_status = owned_surface_wait_slice(runtime, slice_ms as f64 / 1000.0);
                if pump_status != 0 {
                    loop_error = Some(HostError::SurfaceFailed {
                        operation: "gpu_test_pointer_sequence_wait",
                        status: pump_status,
                    });
                    break;
                }
                if let Err(error) = pump_main_looper(runtime) {
                    loop_error = Some(error);
                    break;
                }
                if let Err(error) = pump_main_looper(runtime) {
                    loop_error = Some(error);
                    break;
                }
                if frame_clock.take_latest().is_some() {
                    if let Err(error) = pulse_frame_with_latency(
                        runtime,
                        debug_latency,
                        &mut last_input_dispatch,
                        &mut frame_latencies_us,
                    ) {
                        loop_error = Some(error);
                        break;
                    }
                }
            }
            if loop_error.is_some() {
                break;
            }
            let tap_started = Instant::now();
            for action in [0_u32, 1_u32] {
                if loop_error.is_some() {
                    break;
                }
                if action == 1 && debug_latency {
                    eprintln!(
                        "DARWIN_ART synthetic tap x={x} y={y} hold_requested_ms={test_hold_ms} down_to_up_us={}",
                        tap_started.elapsed().as_micros()
                    );
                }
                let event = synthetic_event(action, x, y);
                let status = runtime
                    .graphics()
                    .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&event));
                if status != 0 {
                    loop_error = Some(HostError::RuntimeFailed(status));
                    break;
                }
                if debug_latency {
                    last_input_dispatch = Some(Instant::now());
                }
                frames_presented += 1;
                if let Err(error) = pump_frame_with_latency(
                    runtime,
                    debug_latency,
                    &mut last_input_dispatch,
                    &mut frame_latencies_us,
                ) {
                    loop_error = Some(error);
                    break;
                }
                // Preserve a real tap interval for every sample, not only the
                // first one dispatched above. Blink's Android gesture path
                // legitimately consumes an instantaneous DOWN/UP pair but
                // does not promote it to a click when both timestamps fall
                // in the same millisecond. Real NSEvents naturally have this
                // spacing; synthetic acceptance taps must model it too.
                // Native Looper waits can wake early for queued work. A
                // single wait slice (also capped at 16 ms) cannot guarantee
                // the requested DOWN/UP spacing. Measure actual elapsed time
                // and keep servicing Android work until the tap deadline.
                while action == 0 && tap_started.elapsed() < Duration::from_millis(test_hold_ms) {
                    let remaining =
                        Duration::from_millis(test_hold_ms).saturating_sub(tap_started.elapsed());
                    let pump_status = owned_surface_wait_slice(runtime, remaining.as_secs_f64());
                    if pump_status != 0 {
                        loop_error = Some(HostError::SurfaceFailed {
                            operation: "gpu_test_pointer_sequence_hold",
                            status: pump_status,
                        });
                        break;
                    }
                    if let Err(error) = pump_main_looper(runtime) {
                        loop_error = Some(error);
                        break;
                    }
                    if let Err(error) = pump_frame_with_latency(
                        runtime,
                        debug_latency,
                        &mut last_input_dispatch,
                        &mut frame_latencies_us,
                    ) {
                        loop_error = Some(error);
                        break;
                    }
                }
            }
        }
    }
    if loop_error.is_none() && pointer_sequence_post_delay_ms > 0 {
        let delay = Duration::from_millis(pointer_sequence_post_delay_ms);
        let wait_started = Instant::now();
        while wait_started.elapsed() < delay && loop_error.is_none() {
            let remaining = delay.saturating_sub(wait_started.elapsed());
            let slice_ms = remaining.as_millis().min(OWNER_WAIT_MAX.as_millis()).max(1) as u64;
            let pump_status = owned_surface_wait_slice(runtime, slice_ms as f64 / 1000.0);
            if pump_status != 0 {
                loop_error = Some(HostError::SurfaceFailed {
                    operation: "gpu_test_pointer_sequence_post_delay",
                    status: pump_status,
                });
                break;
            }
            if let Err(error) = pump_main_looper(runtime) {
                loop_error = Some(error);
                break;
            }
            if let Err(error) = pump_main_looper(runtime) {
                loop_error = Some(error);
                break;
            }
            if frame_clock.take_latest().is_some() {
                if let Err(error) = pulse_frame_with_latency(
                    runtime,
                    debug_latency,
                    &mut last_input_dispatch,
                    &mut frame_latencies_us,
                ) {
                    loop_error = Some(error);
                    break;
                }
            }
        }
    }
    if loop_error.is_none()
        && let Some(sequence) = post_pointer_key_sequence.as_ref()
    {
        let wait_started = Instant::now();
        while wait_started.elapsed() < Duration::from_millis(post_pointer_key_delay_ms)
            && loop_error.is_none()
        {
            let remaining = Duration::from_millis(post_pointer_key_delay_ms)
                .saturating_sub(wait_started.elapsed());
            let slice_ms = remaining.as_millis().min(16).max(1) as u64;
            let pump_status = owned_surface_wait_slice(runtime, slice_ms as f64 / 1000.0);
            if pump_status != 0 {
                loop_error = Some(HostError::SurfaceFailed {
                    operation: "gpu_test_post_pointer_key_wait",
                    status: pump_status,
                });
                break;
            }
            if let Err(error) = pump_main_looper(runtime) {
                loop_error = Some(error);
                break;
            }
            if let Err(error) = pump_frame_with_latency(
                runtime,
                debug_latency,
                &mut last_input_dispatch,
                &mut frame_latencies_us,
            ) {
                loop_error = Some(error);
            }
        }
        match dispatch_synthetic_keys(runtime, sequence) {
            Ok(dispatched) => frames_presented += dispatched,
            Err(error) => loop_error = Some(error),
        }
        if loop_error.is_none()
            && let Err(error) = pump_frame_with_latency(
                runtime,
                debug_latency,
                &mut last_input_dispatch,
                &mut frame_latencies_us,
            )
        {
            loop_error = Some(error);
        }
    }
    if loop_error.is_none()
        && let Some(points) = post_sequence_drag.as_ref()
    {
        let (down_x, down_y) = points[0];
        let down = synthetic_event(0, down_x, down_y);
        let status = runtime
            .graphics()
            .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&down));
        if status != 0 {
            loop_error = Some(HostError::RuntimeFailed(status));
        } else {
            frames_presented += 1;
            for &(move_x, move_y) in points.iter().skip(1) {
                if loop_error.is_some() {
                    break;
                }
                let pump_status = owned_surface_wait_slice(runtime, 0.016);
                if pump_status != 0 {
                    loop_error = Some(HostError::SurfaceFailed {
                        operation: "gpu_test_post_sequence_drag_pump",
                        status: pump_status,
                    });
                    break;
                }
                if let Err(error) = pump_main_looper(runtime) {
                    loop_error = Some(error);
                    break;
                }
                let move_event = synthetic_event(2, move_x, move_y);
                let status = runtime
                    .graphics()
                    .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&move_event));
                if status != 0 {
                    loop_error = Some(HostError::RuntimeFailed(status));
                    break;
                }
                frames_presented += 1;
                if let Err(error) = pump_frame_with_latency(
                    runtime,
                    debug_latency,
                    &mut last_input_dispatch,
                    &mut frame_latencies_us,
                ) {
                    loop_error = Some(error);
                }
            }
            if loop_error.is_none() {
                let (up_x, up_y) = *points.last().expect("post drag has points");
                let up = synthetic_event(1, up_x, up_y);
                let status = runtime
                    .graphics()
                    .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&up));
                if status != 0 {
                    loop_error = Some(HostError::RuntimeFailed(status));
                } else {
                    frames_presented += 1;
                    if let Err(error) = pump_frame_with_latency(
                        runtime,
                        debug_latency,
                        &mut last_input_dispatch,
                        &mut frame_latencies_us,
                    ) {
                        loop_error = Some(error);
                    }
                }
            }
        }
    }
    if loop_error.is_none()
        && let Some(sequence) = post_drag_tap_sequence.as_ref()
    {
        for &(x, y, delay_ms) in sequence {
            let mut waited_ms = 0_u64;
            while waited_ms < delay_ms && loop_error.is_none() {
                let slice_ms = (delay_ms - waited_ms).min(16);
                let pump_status = owned_surface_wait_slice(runtime, slice_ms as f64 / 1000.0);
                if pump_status != 0 {
                    loop_error = Some(HostError::SurfaceFailed {
                        operation: "gpu_test_post_drag_pointer_sequence_wait",
                        status: pump_status,
                    });
                    break;
                }
                if let Err(error) = pump_main_looper(runtime) {
                    loop_error = Some(error);
                    break;
                }
                if let Err(error) = pump_frame_with_latency(
                    runtime,
                    debug_latency,
                    &mut last_input_dispatch,
                    &mut frame_latencies_us,
                ) {
                    loop_error = Some(error);
                    break;
                }
                waited_ms += slice_ms;
            }
            if loop_error.is_some() {
                break;
            }
            let tap_started = Instant::now();
            for action in [0_u32, 1_u32] {
                if loop_error.is_some() {
                    break;
                }
                if action == 1 && debug_latency {
                    eprintln!(
                        "DARWIN_ART synthetic tap x={x} y={y} hold_requested_ms={test_hold_ms} down_to_up_us={}",
                        tap_started.elapsed().as_micros()
                    );
                }
                let event = synthetic_event(action, x, y);
                let status = runtime
                    .graphics()
                    .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&event));
                if status != 0 {
                    loop_error = Some(HostError::RuntimeFailed(status));
                    break;
                }
                frames_presented += 1;
                if let Err(error) = pump_frame_with_latency(
                    runtime,
                    debug_latency,
                    &mut last_input_dispatch,
                    &mut frame_latencies_us,
                ) {
                    loop_error = Some(error);
                    break;
                }
                while action == 0 && tap_started.elapsed() < Duration::from_millis(test_hold_ms) {
                    let remaining =
                        Duration::from_millis(test_hold_ms).saturating_sub(tap_started.elapsed());
                    let pump_status = owned_surface_wait_slice(runtime, remaining.as_secs_f64());
                    if pump_status != 0 {
                        loop_error = Some(HostError::SurfaceFailed {
                            operation: "gpu_test_post_drag_pointer_sequence_hold",
                            status: pump_status,
                        });
                        break;
                    }
                    if let Err(error) = pump_main_looper(runtime) {
                        loop_error = Some(error);
                        break;
                    }
                    if let Err(error) = pump_frame_with_latency(
                        runtime,
                        debug_latency,
                        &mut last_input_dispatch,
                        &mut frame_latencies_us,
                    ) {
                        loop_error = Some(error);
                        break;
                    }
                }
            }
        }
    }
    // Bound the visible lifetime by a monotonic deadline. Framework work can
    // legitimately take longer than one display interval (for example a
    // Binder service retry while Chromium initializes Graphite). Subtracting
    // a nominal 16 ms per iteration made --window-seconds describe a frame
    // count instead of wall time and could stretch a 9-second acceptance run
    // into minutes.
    let visible_deadline = Instant::now()
        .checked_add(Duration::from_secs_f64(options.visible_seconds))
        .unwrap_or_else(Instant::now);
    // A delayed synthetic resize is relative to the completed input setup,
    // not process launch. This lets an Android click finish creating its
    // popup ViewRoot before the acceptance resize exercises every live root.
    let test_resize_clock = Instant::now();
    while Instant::now() < visible_deadline && !crate::process_signal::termination_requested() {
        if loop_error.is_none()
            && let (Some((width, height)), Some(after_ms)) =
                (pending_test_resize, test_resize_after_ms)
            && test_resize_clock.elapsed().as_millis() >= u128::from(after_ms)
        {
            let status = runtime
                .surface()
                .map_or(-1, |surface| surface.resize(width, height));
            if status != 0 {
                loop_error = Some(HostError::SurfaceFailed {
                    operation: "gpu_delayed_test_window_resize",
                    status,
                });
            } else {
                eprintln!(
                    "DARWIN_ART gpu delayed test resize={width}x{height} after_ms={after_ms}"
                );
                pending_test_resize = None;
            }
        }
        let slice = visible_deadline
            .saturating_duration_since(Instant::now())
            .min(OWNER_WAIT_MAX)
            .as_secs_f64();
        if slice <= 0.0 {
            break;
        }
        let pump_status = owned_surface_wait_slice(runtime, slice);
        if pump_status == 7 {
            break;
        }
        if pump_status != 0 {
            loop_error = Some(HostError::SurfaceFailed {
                operation: "gpu_pump_events",
                status: pump_status,
            });
            break;
        }
        if let Err(error) = pump_main_looper(runtime) {
            loop_error = Some(error);
        }
        if loop_error.is_none()
            && let Err(error) = pump_main_looper(runtime)
        {
            loop_error = Some(error);
        }
        if loop_error.is_none() && frame_clock.take_latest().is_some() {
            if let Err(error) = pulse_frame_with_latency(
                runtime,
                debug_latency,
                &mut last_input_dispatch,
                &mut frame_latencies_us,
            ) {
                loop_error = Some(error);
            }
        }
        // The standalone capture gate has no Android ViewRoot to
        // request redraws after ACTION_UP. Keep the framework pulse
        // and GPU RenderNode replay alive for the test pointer so the
        // real ripple/compatibility bridge can finish on-screen.
        if loop_error.is_none()
            && replay_standalone_pointer
            && let Some((x, y)) = test_pointer
        {
            let move_event = synthetic_event(2, x, y);
            let replay_status = runtime
                .graphics()
                .map_or(-1, |graphics| graphics.dispatch_pointer_v2(&move_event));
            if replay_status != 0 {
                loop_error = Some(HostError::RuntimeFailed(replay_status));
            } else {
                frames_presented += 1;
            }
        }
        if loop_error.is_some() {
            break;
        }
    }
    if crate::process_signal::termination_requested() {
        eprintln!("DARWIN_ART host received graceful termination request");
    }
    if let Some(error) = loop_error {
        if debug_boundaries {
            eprintln!("DARWIN_ART gpu-loop exit error={error}");
        }
        return Err(error);
    }
    if test_pointer.is_some() {
        eprintln!("DARWIN_ART gpu synthetic moves dispatched={synthetic_moves}");
    }
    if debug_latency && !frame_latencies_us.is_empty() {
        frame_latencies_us.sort_unstable();
        let percentile = |numerator: usize, denominator: usize| -> u64 {
            let index = (frame_latencies_us.len() * numerator)
                .div_ceil(denominator)
                .saturating_sub(1);
            frame_latencies_us[index.min(frame_latencies_us.len() - 1)]
        };
        eprintln!(
            "DARWIN_ART input->framework-pulse samples={} p50_us={} p95_us={} p99_us={} max_us={}",
            frame_latencies_us.len(),
            percentile(50, 100),
            percentile(95, 100),
            percentile(99, 100),
            frame_latencies_us.last().copied().unwrap_or(0),
        );
    }
    frame_timing::report();
    if debug_boundaries {
        eprintln!("DARWIN_ART gpu-loop exit status=0 frames_presented={frames_presented}");
    }
    Ok(HostOutcome {
        process,
        frames_presented,
        last_frame: None,
    })
}
