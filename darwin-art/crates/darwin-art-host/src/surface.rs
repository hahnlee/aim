//! Owner-bound surface and provider operations.

#[cfg(target_os = "macos")]
use darwin_art_engine::SurfaceSession;

#[cfg(target_os = "macos")]
use crate::runtime::HostRuntime;

#[cfg(target_os = "macos")]
pub fn owned_surface(runtime: &HostRuntime) -> Option<&SurfaceSession> {
    runtime.surface()
}

#[cfg(target_os = "macos")]
pub fn owned_surface_wait_slice(runtime: &HostRuntime, seconds: f64) -> i32 {
    // AppKit's main actor owns NSApplication event delivery. The ART owner
    // only yields briefly, then observes the worker-safe close snapshot; it
    // must never synchronously dispatch to the main queue from this path.
    if owned_surface(runtime).is_none_or(SurfaceSession::close_requested) {
        return 7;
    }
    if seconds.is_finite() && seconds > 0.0 {
        let timeout_ms = (seconds.min(0.016) * 1000.0).ceil() as i32;
        // Prefer the Android owner Looper's native wait. It observes native
        // fd readiness and the AppKit input wake token without moving any
        // JNI/session state across threads. Older engine images lack this
        // optional entrypoint, so retain the bounded sleep fallback.
        let wait_status = runtime.graphics().map_or(
            darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE,
            |graphics| graphics.wait_main_looper(timeout_ms),
        );
        if wait_status == darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE {
            std::thread::sleep(std::time::Duration::from_secs_f64(seconds.min(0.016)));
        } else if wait_status != 0 {
            return wait_status;
        }
    }
    if owned_surface(runtime).is_none_or(SurfaceSession::close_requested) {
        7
    } else {
        0
    }
}
