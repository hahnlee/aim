//! The device's density (`ro.sf.lcd_density`, ADR 0008). Android lays out
//! in dp, a pixel of a 160 dpi screen; macOS lays out in points, and a
//! display draws `scale` backing pixels per point. The device's raster
//! is the backing's, so it shows Android at 160 dpi per point of scale: a
//! Retina display (2x) at 320 (xhdpi), a 1x display at 160 (mdpi). The
//! main display's scale decides it when the device starts, for guest-init
//! (the property) and the display server (its input) alike.

use std::ffi::c_void;

/// The property SurfaceFlinger reads the device's density from.
pub const PROP: &str = "ro.sf.lcd_density";

/// `DisplayMetrics.DENSITY_DEFAULT`: dpi of one pixel per dp.
pub const DENSITY_DEFAULT: u32 = 160;

/// The density of a display drawing `scale` backing pixels per point.
pub fn of_scale(scale: f64) -> Option<u32> {
    (scale.is_finite() && scale > 0.0).then(|| (DENSITY_DEFAULT as f64 * scale).round() as u32)
}

/// The main display's: its current mode's pixels over its points. None
/// without a display mode (no window server session).
pub fn main_display() -> Option<u32> {
    // SAFETY: CoreGraphics queries on the main display; the mode is
    // released.
    let (pixels, points) = unsafe {
        let mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
        if mode.is_null() {
            return None;
        }
        let size = (
            CGDisplayModeGetPixelWidth(mode),
            CGDisplayModeGetWidth(mode),
        );
        CGDisplayModeRelease(mode);
        size
    };
    if points == 0 {
        return None;
    }
    of_scale(pixels as f64 / points as f64)
}

/// Display pixels of `dp` at `density`.
pub fn pixels(dp: f64, density: u32) -> f64 {
    dp * density as f64 / DENSITY_DEFAULT as f64
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayCopyDisplayMode(display: u32) -> *const c_void;
    fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeGetWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeRelease(mode: *const c_void);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_follows_the_backing_scale() {
        assert_eq!(of_scale(2.0), Some(320));
        assert_eq!(of_scale(1.0), Some(160));
        assert_eq!(of_scale(3.0), Some(480));
        assert_eq!(of_scale(0.0), None);
        assert_eq!(of_scale(f64::NAN), None);
        assert_eq!(pixels(64.0, 320), 128.0);
        assert_eq!(pixels(64.0, 160), 64.0);
    }
}
