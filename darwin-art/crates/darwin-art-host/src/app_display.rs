//! Visible macOS display target for one Android application process.
//!
//! ActivityThread owns UI lifecycle and BLAST/HWUI own buffers. This module
//! only allocates the desktop scanout target before ActivityThread enters its
//! permanent Looper; it never calls into Java or draws application content.

use crate::config::{HostError, RunOptions};
use darwin_art_engine::{EngineSession, SurfaceSession};
use darwin_art_engine_sys::SurfaceCreateInfo;
use std::ffi::CString;

const WINDOW_WIDTH_POINTS: u32 = 360;
const WINDOW_HEIGHT_POINTS: u32 = 640;

pub(crate) fn create(
    engine: &EngineSession,
    options: &RunOptions,
) -> Result<Option<SurfaceSession>, HostError> {
    // Android application processes always own a display target. A zero
    // visible duration means an unbounded application lifetime; it must not
    // turn the process into a headless runtime probe.
    // The shared system service also has process-scoped ART lifetime, but
    // owns no application window. Lifetime policy is not display ownership.
    if !options.terminate_android_process
        || std::env::var("DARWIN_ART_DESKTOP_PRESENTATION").as_deref() != Ok("1")
    {
        return Ok(None);
    }
    let title = std::env::var("DARWIN_ART_APK_WINDOW_TITLE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("DARWIN_ART_APK_APP_LABEL")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "Android Application".to_owned());
    let title = CString::new(title).map_err(|_| {
        HostError::HostService("Android application title contains an interior NUL".to_owned())
    })?;
    let android_scale = match std::env::var("DARWIN_ART_WINDOW_SCALE").as_deref() {
        Ok("2") => 2,
        Ok("1") | Err(_) => 1,
        Ok(_) => {
            return Err(HostError::HostService(
                "DARWIN_ART_WINDOW_SCALE must be 1 or 2".to_owned(),
            ));
        }
    };
    let info = SurfaceCreateInfo {
        width: WINDOW_WIDTH_POINTS * android_scale,
        height: WINDOW_HEIGHT_POINTS * android_scale,
        title: title.as_ptr(),
        visible: true,
        // Width/height are Android backing pixels. The AppKit boundary uses
        // the screen scale to expose the same 360x640-point desktop window.
        scale_to_display: false,
    };
    engine
        .create_surface(&info)
        .map(Some)
        .map_err(|status| HostError::SurfaceFailed {
            operation: "android_app_display_create",
            status,
        })
}
