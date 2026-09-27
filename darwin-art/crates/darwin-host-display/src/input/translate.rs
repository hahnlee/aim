//! Window events to device events: what `darwin-display`'s view reports,
//! already reduced to plain values, becomes evdev packets. Tests drive this
//! the same way the view does.
//!
//! - **Pointer.** The primary button is one finger on the touchscreen
//!   (multitouch protocol B, slot 0). A point in the view (points, origin
//!   at the bottom left) maps to the display's pixels through the layer's
//!   aspect fit (`resizeAspect`), so the mapping holds at any window size
//!   and backing scale. A press outside the picture is ignored; a drag that
//!   leaves it is clamped to the edge.
//! - **Keys.** Physical keys ([`keymap`](super::keymap)), down and up. Key
//!   repeat is Android's (the keyboard declares no `EV_REP`), so AppKit's
//!   repeats are dropped.
//! - **Scrolling.** Wheel and trackpad scrolling are the rotary encoder's
//!   `REL_WHEEL_HI_RES` (120 per line) and `REL_WHEEL` (whole lines), in
//!   the direction the Mac's own content would move.

use std::sync::Mutex;

use super::codes::*;
use super::keymap::{self, Modifier};
use super::server::Devices;
use super::{KEYBOARD, TOUCHSCREEN, WHEEL};

/// Trackpad scrolling comes in points; a line is this many.
pub const POINTS_PER_LINE: f64 = 10.0;

/// The display pixel under view point `(x, y)` (origin at the bottom left)
/// of a `view_w` x `view_h` point view showing a `width` x `height` pixel
/// display aspect-fitted and centered; None outside the picture.
pub fn to_display(
    x: f64,
    y: f64,
    view_w: f64,
    view_h: f64,
    width: u32,
    height: u32,
) -> Option<(i32, i32)> {
    let (px, py) = to_display_unclamped(x, y, view_w, view_h, width, height)?;
    let inside = (0.0..width as f64).contains(&px) && (0.0..height as f64).contains(&py);
    inside.then_some((px as i32, py as i32))
}

/// As [`to_display`], clamped to the picture's edge.
pub fn to_display_clamped(
    x: f64,
    y: f64,
    view_w: f64,
    view_h: f64,
    width: u32,
    height: u32,
) -> Option<(i32, i32)> {
    let (px, py) = to_display_unclamped(x, y, view_w, view_h, width, height)?;
    let c = |v: f64, n: u32| v.clamp(0.0, n as f64 - 1.0) as i32;
    Some((c(px, width), c(py, height)))
}

fn to_display_unclamped(
    x: f64,
    y: f64,
    view_w: f64,
    view_h: f64,
    width: u32,
    height: u32,
) -> Option<(f64, f64)> {
    if view_w <= 0.0 || view_h <= 0.0 || width == 0 || height == 0 {
        return None;
    }
    // Points per display pixel, and the picture's offset in the view.
    let scale = (view_w / width as f64).min(view_h / height as f64);
    let ox = (view_w - width as f64 * scale) / 2.0;
    let oy = (view_h - height as f64 * scale) / 2.0;
    let px = (x - ox) / scale;
    let from_bottom = (y - oy) / scale;
    Some((px, height as f64 - from_bottom))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Drag,
    Up,
}

#[derive(Default)]
struct Pointer {
    touching: bool,
    next_tracking_id: i32,
    keys: Vec<u16>,
    /// `REL_WHEEL_HI_RES` not sent yet (fractions of a unit) and not yet
    /// a whole line.
    hi_res_rest: f64,
    line_rest: i32,
}

/// The input side of the display server: its devices and the translation
/// state.
pub struct Input {
    devices: Devices,
    width: u32,
    height: u32,
    state: Mutex<Pointer>,
}

impl Input {
    /// `width` x `height`: the display, in pixels.
    pub fn new(devices: Devices, width: u32, height: u32) -> Input {
        Input {
            devices,
            width,
            height,
            state: Mutex::new(Pointer::default()),
        }
    }

    /// The primary button at view point `(x, y)` of a `view_w` x `view_h`
    /// view, at `t` (guest `CLOCK_MONOTONIC` ns).
    pub fn pointer(&self, phase: Phase, x: f64, y: f64, view_w: f64, view_h: f64, t: i64) {
        let mut s = self.state.lock().unwrap();
        let (w, h) = (self.width, self.height);
        match phase {
            Phase::Down => {
                if s.touching {
                    return;
                }
                let Some((px, py)) = to_display(x, y, view_w, view_h, w, h) else {
                    return;
                };
                s.touching = true;
                let id = s.next_tracking_id;
                s.next_tracking_id = (id + 1) & 0xffff;
                self.devices.emit(
                    TOUCHSCREEN,
                    t,
                    &[
                        (EV_ABS, ABS_MT_TRACKING_ID, id),
                        (EV_ABS, ABS_MT_POSITION_X, px),
                        (EV_ABS, ABS_MT_POSITION_Y, py),
                        (EV_KEY, BTN_TOUCH, 1),
                    ],
                );
            }
            Phase::Drag if s.touching => {
                if let Some((px, py)) = to_display_clamped(x, y, view_w, view_h, w, h) {
                    self.devices.emit(
                        TOUCHSCREEN,
                        t,
                        &[
                            (EV_ABS, ABS_MT_POSITION_X, px),
                            (EV_ABS, ABS_MT_POSITION_Y, py),
                        ],
                    );
                }
            }
            Phase::Up if s.touching => {
                s.touching = false;
                self.lift(t);
            }
            Phase::Drag | Phase::Up => {}
        }
    }

    fn lift(&self, t: i64) {
        self.devices.emit(
            TOUCHSCREEN,
            t,
            &[(EV_ABS, ABS_MT_TRACKING_ID, -1), (EV_KEY, BTN_TOUCH, 0)],
        );
    }

    fn set_key(&self, s: &mut Pointer, code: u16, down: bool, t: i64) {
        s.keys.retain(|&k| k != code);
        if down {
            s.keys.push(code);
        }
        self.devices
            .emit(KEYBOARD, t, &[(EV_KEY, code, down as i32)]);
    }

    /// `keyDown:` (`repeat`: `isARepeat`) or `keyUp:` of virtual key `mac`.
    pub fn key(&self, mac: u16, down: bool, repeat: bool, t: i64) {
        if repeat {
            return;
        }
        if let Some(code) = keymap::linux_key(mac) {
            self.set_key(&mut self.state.lock().unwrap(), code, down, t);
        }
    }

    /// `flagsChanged:` for virtual key `mac` with `modifierFlags` `flags`.
    pub fn flags_changed(&self, mac: u16, flags: u64, t: i64) {
        let (Some(m), Some(code)) = (keymap::modifier(mac, flags), keymap::linux_key(mac)) else {
            return;
        };
        let mut s = self.state.lock().unwrap();
        match m {
            Modifier::Held(down) => self.set_key(&mut s, code, down, t),
            Modifier::Toggled => {
                self.set_key(&mut s, code, true, t);
                self.set_key(&mut s, code, false, t);
            }
        }
    }

    /// `scrollWheel:`: `dy` is `scrollingDeltaY`, in points when `precise`
    /// (trackpad), else in lines.
    pub fn scroll(&self, dy: f64, precise: bool, t: i64) {
        let lines = if precise { dy / POINTS_PER_LINE } else { dy };
        let mut s = self.state.lock().unwrap();
        let hi_res = lines * 120.0 + s.hi_res_rest;
        let units = hi_res.trunc();
        s.hi_res_rest = hi_res - units;
        let units = units as i32;
        if units == 0 {
            return;
        }
        let total = s.line_rest + units;
        let whole = total / 120;
        s.line_rest = total - whole * 120;
        self.devices.emit(
            WHEEL,
            t,
            &[
                (EV_REL, REL_WHEEL, whole),
                (EV_REL, REL_WHEEL_HI_RES, units),
            ],
        );
    }

    pub fn devices(&self) -> &Devices {
        &self.devices
    }

    /// Remove the devices ([`Devices::close`]).
    pub fn close(&self) {
        self.devices.close();
    }

    /// The window lost the keyboard or the mouse: lift the finger and
    /// release every key, so nothing stays down in the guest.
    pub fn release_all(&self, t: i64) {
        let mut s = self.state.lock().unwrap();
        if s.touching {
            s.touching = false;
            self.lift(t);
        }
        for code in std::mem::take(&mut s.keys) {
            self.devices.emit(KEYBOARD, t, &[(EV_KEY, code, 0)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_points_map_to_display_pixels() {
        // A 1080x1920 display in a 540x960 point view (backing scale 2):
        // a point is two pixels, and y flips.
        assert_eq!(
            to_display(0.0, 960.0, 540.0, 960.0, 1080, 1920),
            Some((0, 0))
        );
        assert_eq!(
            to_display(270.0, 480.0, 540.0, 960.0, 1080, 1920),
            Some((540, 960))
        );
        assert_eq!(
            to_display(539.9, 0.1, 540.0, 960.0, 1080, 1920),
            Some((1079, 1919))
        );
        // The window made wider: the picture is centered with bars at the
        // sides, and a press on a bar is outside.
        assert_eq!(
            to_display(370.0, 480.0, 740.0, 960.0, 1080, 1920),
            Some((540, 960))
        );
        assert_eq!(to_display(50.0, 480.0, 740.0, 960.0, 1080, 1920), None);
        assert_eq!(
            to_display_clamped(50.0, 1000.0, 740.0, 960.0, 1080, 1920),
            Some((0, 0))
        );
        // Scaled down to half: a point is four pixels.
        assert_eq!(
            to_display(135.0, 240.0, 270.0, 480.0, 1080, 1920),
            Some((540, 960))
        );
        assert_eq!(to_display(1.0, 1.0, 0.0, 0.0, 1080, 1920), None);
    }
}
