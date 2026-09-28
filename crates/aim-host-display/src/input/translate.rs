//! Window events to device events: what `aim-display`'s view reports,
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
//! - **Back.** The Mac's ways back are the keyboard's `KEY_BACK`
//!   (`Generic.kl`: BACK): Cmd+[, the mouse's back button and a two-finger
//!   swipe to the right. Esc stays Esc (`KEY_ESC`), for the apps and games
//!   that use it; Android 16 turns an Esc no app handles into closing
//!   system dialogs, not Back (its `Generic.kcm` fallback to BACK is
//!   intercepted first), and its own Meta+Esc and Meta+Left (Cmd+Esc,
//!   Cmd+Left) are Back.

use std::sync::Mutex;

use super::codes::*;
use super::keymap::{self, Modifier};
use super::server::Devices;
use super::{KEYBOARD, TOUCHSCREEN, WHEEL};

/// Trackpad scrolling comes in points; a line is this many.
pub const POINTS_PER_LINE: f64 = 10.0;
/// A horizontal two-finger swipe this far (points) is Back.
pub const SWIPE_BACK_POINTS: f64 = 80.0;
/// The macOS virtual key of `[`.
const MAC_LEFT_BRACKET: u16 = 0x21;

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

/// A trackpad gesture's phase (`NSEvent.phase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Began,
    Changed,
    Ended,
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
    /// The fingers' travel in this gesture, and whether it was Back yet.
    swipe: (f64, f64, bool),
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
    /// view showing the whole display, at `t` (guest `CLOCK_MONOTONIC` ns).
    pub fn pointer(&self, phase: Phase, x: f64, y: f64, view_w: f64, view_h: f64, t: i64) {
        let (w, h) = (self.width, self.height);
        if let Some((px, py)) = to_display_unclamped(x, y, view_w, view_h, w, h) {
            self.touch(phase, px, py, [0, 0, w as i32, h as i32], t);
        }
    }

    /// The primary button at display pixel `(x, y)`, which `area` (left,
    /// top, right, bottom) bounds: a press outside it is ignored, a drag
    /// leaving it is clamped to its edge.
    pub fn touch(&self, phase: Phase, x: f64, y: f64, area: [i32; 4], t: i64) {
        let mut s = self.state.lock().unwrap();
        let [l, top, r, b] = area.map(|v| v as f64);
        match phase {
            Phase::Down => {
                if s.touching || !(l..r).contains(&x) || !(top..b).contains(&y) {
                    return;
                }
                s.touching = true;
                let id = s.next_tracking_id;
                s.next_tracking_id = (id + 1) & 0xffff;
                self.devices.emit(
                    TOUCHSCREEN,
                    t,
                    &[
                        (EV_ABS, ABS_MT_TRACKING_ID, id),
                        (EV_ABS, ABS_MT_POSITION_X, x as i32),
                        (EV_ABS, ABS_MT_POSITION_Y, y as i32),
                        (EV_KEY, BTN_TOUCH, 1),
                    ],
                );
            }
            Phase::Drag if s.touching && l < r && top < b => {
                self.devices.emit(
                    TOUCHSCREEN,
                    t,
                    &[
                        (EV_ABS, ABS_MT_POSITION_X, x.clamp(l, r - 1.0) as i32),
                        (EV_ABS, ABS_MT_POSITION_Y, y.clamp(top, b - 1.0) as i32),
                    ],
                );
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

    /// Back, the key: pressed (`down`) or released.
    pub fn back(&self, down: bool, t: i64) {
        self.set_key(&mut self.state.lock().unwrap(), KEY_BACK, down, t);
    }

    /// macOS's Back shortcut, Cmd+[: `keyDown:` (`down`, with `command`
    /// held) or `keyUp:` of virtual key `mac`. Returns whether the key was
    /// the shortcut's. AppKit sends no `keyUp:` while Command is held, so
    /// the down presses and releases Back, and the up is dropped.
    pub fn back_shortcut(&self, mac: u16, down: bool, command: bool, t: i64) -> bool {
        if mac != MAC_LEFT_BRACKET || !command {
            return false;
        }
        if down {
            let mut s = self.state.lock().unwrap();
            self.set_key(&mut s, KEY_BACK, true, t);
            self.set_key(&mut s, KEY_BACK, false, t);
        }
        true
    }

    /// A two-finger trackpad gesture: the fingers moved `dx`, `dy` points
    /// (right and down positive). Moving right by [`SWIPE_BACK_POINTS`],
    /// mostly sideways, presses and releases Back, once per gesture.
    pub fn swipe(&self, phase: Gesture, dx: f64, dy: f64, t: i64) {
        let mut s = self.state.lock().unwrap();
        match phase {
            Gesture::Began => s.swipe = (dx, dy, false),
            Gesture::Changed => {
                s.swipe.0 += dx;
                s.swipe.1 += dy;
                let (x, y, done) = s.swipe;
                if !done && x >= SWIPE_BACK_POINTS && x > 2.0 * y.abs() {
                    s.swipe.2 = true;
                    self.set_key(&mut s, KEY_BACK, true, t);
                    self.set_key(&mut s, KEY_BACK, false, t);
                }
            }
            Gesture::Ended => s.swipe = (0.0, 0.0, false),
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
    use super::super::{Descriptor, Hello, OP_OPEN, Record, VERSION, connect_in, devices};
    use super::*;
    use crate::wire;
    use std::io::Read;
    use std::os::fd::AsFd;

    /// The keyboard's packets as (code, value) pairs, `n` records.
    fn keys(k: &mut std::os::unix::net::UnixStream, n: usize) -> Vec<(u16, i32)> {
        let mut v = vec![Record::default(); n];
        // SAFETY: `Record` is plain old data.
        let b = unsafe {
            std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), n * size_of::<Record>())
        };
        k.read_exact(b).unwrap();
        v.iter()
            .filter(|r| r.kind == EV_KEY)
            .map(|r| (r.code, r.value))
            .collect()
    }

    #[test]
    fn back_from_the_shortcut_the_button_and_a_swipe() {
        let dir = std::env::temp_dir().join(format!("back-{}", std::process::id()));
        let input = Input::new(
            Devices::create(&dir, devices(100, 200, 160.0, 160.0)).unwrap(),
            100,
            200,
        );
        let mut k = connect_in(&dir, "event1").unwrap();
        let hello = Hello {
            version: VERSION,
            op: OP_OPEN,
            ..Default::default()
        };
        wire::send(k.as_fd(), wire::bytes(&hello), None).unwrap();
        wire::recv_record::<Descriptor>(k.as_fd()).unwrap().unwrap();

        // Cmd+[ is Back, pressed and released at its down; [ alone is [.
        assert!(input.back_shortcut(0x21, true, true, 1));
        assert!(input.back_shortcut(0x21, false, true, 2));
        assert!(!input.back_shortcut(0x21, true, false, 3));
        input.key(0x21, true, false, 3);
        assert!(!input.back_shortcut(0x21, false, false, 4));
        input.key(0x21, false, false, 4);
        assert!(!input.back_shortcut(0x00, true, true, 5));
        // The mouse's back button.
        input.back(true, 6);
        input.back(false, 7);
        assert_eq!(
            keys(&mut k, 12),
            [
                (KEY_BACK, 1),
                (KEY_BACK, 0),
                (26, 1),
                (26, 0),
                (KEY_BACK, 1),
                (KEY_BACK, 0)
            ]
        );

        // A swipe to the right is Back once; a mostly vertical one, or one
        // to the left, is not.
        input.swipe(Gesture::Began, 10.0, 0.0, 8);
        for _ in 0..10 {
            input.swipe(Gesture::Changed, 20.0, 2.0, 9);
        }
        input.swipe(Gesture::Ended, 0.0, 0.0, 10);
        input.swipe(Gesture::Began, 0.0, 0.0, 11);
        input.swipe(Gesture::Changed, 90.0, 60.0, 12);
        input.swipe(Gesture::Changed, -200.0, 0.0, 12);
        input.swipe(Gesture::Ended, 0.0, 0.0, 13);
        input.back(true, 14);
        assert_eq!(
            keys(&mut k, 6),
            [(KEY_BACK, 1), (KEY_BACK, 0), (KEY_BACK, 1)]
        );
        input.close();
    }

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
