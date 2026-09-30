//! Window events to device events: what `aim-display`'s views report,
//! already reduced to plain values, becomes evdev packets. Tests drive this
//! the same way the views do.
//!
//! - **Pointer.** The primary button is one finger on the touchscreen
//!   (multitouch protocol B). A point in the view (points, origin at the
//!   bottom left) maps to the display's pixels through the layer's aspect
//!   fit (`resizeAspect`), so the mapping holds at any window size and
//!   backing scale. A press outside the picture is ignored; a drag that
//!   leaves it is clamped to the edge. Everything else the pointer does is
//!   the mouse's, an absolute pointer at the same pixel: hovering (the
//!   tool in range), the right, middle, back and forward buttons, and
//!   scrolling.
//! - **Scrolling.** Both axes, where the pointer is: `REL_WHEEL_HI_RES` and
//!   `REL_HWHEEL_HI_RES` (120 per unit) and whole units, in the direction
//!   the Mac's own content would move. A unit is what Android scrolls for
//!   one (`AXIS_VSCROLL` 1.0: 64 dp), so a trackpad's deltas, in pixels,
//!   move the content as far as the fingers; a wheel's lines are units.
//! - **Pinch and rotate.** A trackpad's magnify and rotate gestures are two
//!   fingers on the touchscreen centered on the pointer, which spread and
//!   turn with the gesture, as on a phone.
//! - **Smart zoom.** A trackpad's two-finger double tap is a double tap
//!   where the pointer is ([`double_tap`]).
//! - **Keys.** Physical keys ([`keymap`](super::keymap)) through the
//!   keyboard's keymap, down and up. Key repeat is Android's (the keyboard
//!   declares no `EV_REP`), so AppKit's repeats are dropped.
//! - **Command.** The Mac's shortcut key is Android's Ctrl: Cmd+C, V, X,
//!   A, Z and every other Cmd+key press and release Ctrl and the key.
//!   Cmd+Left and Right are Home and End, Cmd+Up and Down Ctrl+Home and
//!   Ctrl+End, as in a Mac text field. Cmd+Delete deletes to the line's
//!   start (Shift+Home, then Backspace), Cmd+Forward Delete to its end.
//!   Command itself does not reach Android (whose Meta taps and
//!   Meta+letter shortcuts it would trigger).
//! - **Option** is Alt, except that Option+Left and Right move by word
//!   and Option+Delete and Forward Delete delete one: Android's Ctrl with
//!   the key (its Alt there moves to the line's edge and deletes the whole
//!   line). Shift, held, extends the selection, as on the Mac.
//! - **Back.** The Mac's ways back are the keyboard's `KEY_BACK`
//!   (`Generic.kl`: BACK): Cmd+[, the mouse's back button and a two-finger
//!   swipe to the right. A trackpad gesture that starts mostly rightward
//!   is a swipe, not horizontal scrolling. Esc stays Esc (`KEY_ESC`), for
//!   the apps and games that use it; Android 16 turns an Esc no app
//!   handles into closing system dialogs, not Back (its `Generic.kcm`
//!   fallback to BACK is intercepted first).

use std::sync::Mutex;

use super::codes::*;
use super::cursor::Hotspots;
use super::keymap::{self, Modifier};
use super::server::Devices;
use super::{KEYBOARD, MOUSE, TOUCHSCREEN};

/// Display pixels Android scrolls for one scroll unit: 64 dp
/// (`config_verticalScrollFactor`) at the image's density, 320 dpi
/// (`ro.sf.lcd_density`, init.aim.rc).
pub const SCROLL_UNIT_PX: f64 = 128.0;
/// A horizontal two-finger swipe this far (points) is Back.
pub const SWIPE_BACK_POINTS: f64 = 80.0;
/// How far (points) a trackpad gesture moves before it is a swipe or
/// scrolling.
pub const SWIPE_DECIDE_POINTS: f64 = 8.0;
/// The two fingers of a pinch start this far apart (mm): above Android's
/// smallest scaling span (27 mm), so the first change already zooms.
pub const PINCH_SPAN_MM: f64 = 40.0;
/// The macOS virtual key of `[`.
const MAC_LEFT_BRACKET: u16 = 0x21;
// The macOS virtual keys of the arrows.
const MAC_LEFT: u16 = 0x7b;
const MAC_RIGHT: u16 = 0x7c;
const MAC_DOWN: u16 = 0x7d;
const MAC_UP: u16 = 0x7e;
// The macOS virtual keys of Delete and Forward Delete.
const MAC_DELETE: u16 = 0x33;
const MAC_FORWARD_DELETE: u16 = 0x75;
/// `NSEventModifierFlagOption`.
pub const OPTION: u64 = 1 << 19;
/// `NSEventModifierFlagCommand`.
pub const COMMAND: u64 = 1 << 20;
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

/// A double tap ending at `t` (ns): the primary button's phases and
/// times. Android's GestureDetector takes taps shorter than `TAP_TIMEOUT`
/// (100 ms), the second down 40 to 300 ms after the first up
/// (`DOUBLE_TAP_MIN_TIME`, `DOUBLE_TAP_TIMEOUT`), as a double tap. The
/// taps lie before `t`, as the gesture did: EventHub takes a time in the
/// future as the present.
pub fn double_tap(t: i64) -> [(Phase, i64); 4] {
    const MS: i64 = 1_000_000;
    [
        (Phase::Down, t - 150 * MS),
        (Phase::Up, t - 110 * MS),
        (Phase::Down, t - 40 * MS),
        (Phase::Up, t),
    ]
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

/// The mouse's buttons other than the primary one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Right,
    Middle,
    Back,
    Forward,
}

impl Button {
    fn code(self) -> u16 {
        match self {
            Button::Right => BTN_RIGHT,
            Button::Middle => BTN_MIDDLE,
            Button::Back => BTN_SIDE,
            Button::Forward => BTN_EXTRA,
        }
    }
}

/// A trackpad's two-finger gesture other than scrolling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Twist {
    /// `magnification`: the span grows by this fraction.
    Magnify(f64),
    /// `rotation`: degrees, counterclockwise.
    Rotate(f64),
}

/// One `scrollWheel:`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    /// The pointer, in display pixels.
    pub x: f64,
    pub y: f64,
    /// How far the content moves, right and down positive: display pixels
    /// when `precise` (a trackpad), else lines (a wheel).
    pub dx: f64,
    pub dy: f64,
    pub precise: bool,
    /// After the fingers lifted (`momentumPhase`).
    pub momentum: bool,
    /// A trackpad gesture that may swipe back ("swipe between pages" on):
    /// its phase and the fingers' motion (points, right and down positive).
    pub swipe: Option<(Gesture, f64, f64)>,
}

/// What a trackpad gesture turned out to be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Axis {
    #[default]
    Undecided,
    Scroll,
    Back,
}

#[derive(Default)]
struct Swipe {
    /// The fingers' travel in this gesture.
    x: f64,
    y: f64,
    axis: Axis,
    /// Back was pressed.
    done: bool,
    /// Horizontal scrolling held back while undecided.
    held: f64,
    /// The last gesture was a swipe: its momentum does not scroll.
    quiet: bool,
}

struct Pinch {
    center: (f64, f64),
    area: [f64; 4],
    span: f64,
    /// Radians, counterclockwise.
    angle: f64,
    magnify: bool,
    rotate: bool,
}

#[derive(Default)]
struct Pointer {
    touching: bool,
    next_tracking_id: i32,
    /// Physical keys down: scan code and the key sent for it.
    keys: Vec<(u32, u16)>,
    /// Scroll not sent yet, per axis (vertical, horizontal): fractions of a
    /// `_HI_RES` unit, and `_HI_RES` units not yet a whole unit.
    hi_res_rest: [f64; 2],
    unit_rest: [i32; 2],
    swipe: Swipe,
    pinch: Option<Pinch>,
    /// Mouse buttons down.
    buttons: Vec<Button>,
    hotspots: Hotspots,
}

/// The input side of the display server: its devices and the translation
/// state.
pub struct Input {
    devices: Devices,
    width: u32,
    height: u32,
    /// Display pixels per millimeter.
    px_per_mm: f64,
    state: Mutex<Pointer>,
}

impl Input {
    /// `width` x `height`: the display, in pixels, at `dpi`.
    pub fn new(devices: Devices, width: u32, height: u32, dpi: f64) -> Input {
        Input {
            devices,
            width,
            height,
            px_per_mm: dpi / 25.4,
            state: Mutex::new(Pointer::default()),
        }
    }

    /// The display pixel under view point `(x, y)` of a `view_w` x `view_h`
    /// view showing the whole display (device mode), unclamped.
    pub fn at(&self, x: f64, y: f64, view_w: f64, view_h: f64) -> Option<(f64, f64)> {
        to_display_unclamped(x, y, view_w, view_h, self.width, self.height)
    }

    /// Display pixels per point of a `view_w` x `view_h` view showing the
    /// whole display.
    pub fn pixels_per_point(&self, view_w: f64, view_h: f64) -> f64 {
        (self.width as f64 / view_w).max(self.height as f64 / view_h)
    }

    /// The whole display as an area.
    pub fn display(&self) -> [i32; 4] {
        [0, 0, self.width as i32, self.height as i32]
    }

    /// The primary button at view point `(x, y)` of a `view_w` x `view_h`
    /// view showing the whole display, at `t` (guest `CLOCK_MONOTONIC` ns).
    pub fn pointer(&self, phase: Phase, x: f64, y: f64, view_w: f64, view_h: f64, t: i64) {
        if let Some((px, py)) = self.at(x, y, view_w, view_h) {
            self.touch(phase, px, py, self.display(), t);
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
                let outside = !(l..r).contains(&x) || !(top..b).contains(&y);
                if s.touching || s.pinch.is_some() || outside {
                    return;
                }
                s.touching = true;
                let id = next_id(&mut s);
                self.devices.emit(
                    TOUCHSCREEN,
                    t,
                    &[
                        (EV_ABS, ABS_MT_SLOT, 0),
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
            &[
                (EV_ABS, ABS_MT_SLOT, 0),
                (EV_ABS, ABS_MT_TRACKING_ID, -1),
                (EV_KEY, BTN_TOUCH, 0),
            ],
        );
    }

    /// The mouse's position, display pixel `(x, y)`, clamped to the display.
    fn place(&self, s: &mut Pointer, x: f64, y: f64) -> [(u16, u16, i32); 3] {
        let c = |v: f64, n: u32| v.clamp(0.0, n as f64 - 1.0) as i32;
        let (x, y) = (c(x, self.width), c(y, self.height));
        s.hotspots.pointer(x, y);
        [
            (EV_KEY, BTN_TOOL_MOUSE, 1),
            (EV_ABS, ABS_X, x),
            (EV_ABS, ABS_Y, y),
        ]
    }

    /// The pointer moved to display pixel `(x, y)` with no button down:
    /// the mouse hovers there.
    pub fn hover(&self, x: f64, y: f64, t: i64) {
        let at = self.place(&mut self.state.lock().unwrap(), x, y);
        self.devices.emit(MOUSE, t, &at);
    }

    /// The pointer's sprite, image `key` of `w` x `h` pixels, lies at
    /// display pixel `(x, y)`: its hot spot ([`Hotspots`]).
    pub fn hotspot(&self, key: u64, w: i32, h: i32, x: i32, y: i32) -> (i32, i32) {
        let mut s = self.state.lock().unwrap();
        s.hotspots.place(key, w, h, x, y)
    }

    /// The pointer left the display's windows: the mouse leaves (its tool
    /// out of range), unless a button is held.
    pub fn leave(&self, t: i64) {
        if self.state.lock().unwrap().buttons.is_empty() {
            self.devices.emit(MOUSE, t, &[(EV_KEY, BTN_TOOL_MOUSE, 0)]);
        }
    }

    /// Mouse button `b` down or up at display pixel `(x, y)`.
    pub fn button(&self, b: Button, down: bool, x: f64, y: f64, t: i64) {
        let mut s = self.state.lock().unwrap();
        s.buttons.retain(|&h| h != b);
        if down {
            s.buttons.push(b);
        }
        let [a, bx, by] = self.place(&mut s, x, y);
        self.devices
            .emit(MOUSE, t, &[a, bx, by, (EV_KEY, b.code(), down as i32)]);
    }

    /// `scrollWheel:`: the mouse scrolls where the pointer is; a trackpad
    /// gesture may instead be a swipe back.
    pub fn scroll(&self, e: Scroll, t: i64) {
        let mut s = self.state.lock().unwrap();
        let (mut dx, dy) = (e.dx, e.dy);
        match e.swipe {
            Some((Gesture::Began, fx, fy)) => {
                s.swipe = Swipe {
                    x: fx,
                    y: fy,
                    ..Default::default()
                };
            }
            Some((Gesture::Changed, fx, fy)) => {
                let w = &mut s.swipe;
                w.x += fx;
                w.y += fy;
                if w.axis == Axis::Undecided && w.x.hypot(w.y) >= SWIPE_DECIDE_POINTS {
                    w.axis = if w.x > 0.0 && w.x > 2.0 * w.y.abs() {
                        Axis::Back
                    } else {
                        Axis::Scroll
                    };
                    if w.axis == Axis::Scroll {
                        dx += std::mem::take(&mut w.held);
                    }
                }
                let back = w.axis == Axis::Back && !w.done && w.x >= SWIPE_BACK_POINTS;
                if back {
                    w.done = true;
                    drop(s);
                    self.tap(KEYBOARD_BACK, t);
                    return;
                }
            }
            Some((Gesture::Ended, ..)) => {
                s.swipe = Swipe {
                    quiet: s.swipe.axis == Axis::Back,
                    ..Default::default()
                };
                return;
            }
            None => {}
        }
        if e.momentum && s.swipe.quiet {
            return;
        }
        if e.swipe.is_some() {
            match s.swipe.axis {
                Axis::Back => return,
                Axis::Undecided => s.swipe.held += std::mem::take(&mut dx),
                Axis::Scroll => {}
            }
        }
        let per_unit = |d: f64| if e.precise { d / SCROLL_UNIT_PX } else { d };
        // Down is a positive REL_WHEEL (content moving down), right a
        // negative REL_HWHEEL (content moving right: scrolling left).
        let (v_units, v_whole) = units(&mut s, 0, per_unit(dy));
        let (h_units, h_whole) = units(&mut s, 1, -per_unit(dx));
        if v_units == 0 && h_units == 0 {
            return;
        }
        let [a, bx, by] = self.place(&mut s, e.x, e.y);
        self.devices.emit(
            MOUSE,
            t,
            &[
                a,
                bx,
                by,
                (EV_REL, REL_WHEEL, v_whole),
                (EV_REL, REL_HWHEEL, h_whole),
                (EV_REL, REL_WHEEL_HI_RES, v_units),
                (EV_REL, REL_HWHEEL_HI_RES, h_units),
            ],
        );
    }

    /// A pinch or rotation at display pixel `(x, y)` of `area`: two
    /// fingers centered there, spreading and turning with the gesture.
    pub fn twist(&self, twist: Twist, phase: Gesture, x: f64, y: f64, area: [i32; 4], t: i64) {
        let mut s = self.state.lock().unwrap();
        match phase {
            Gesture::Began => {
                if s.touching {
                    return;
                }
                if s.pinch.is_none() {
                    s.pinch = Some(Pinch {
                        center: (x, y),
                        area: area.map(|v| v as f64),
                        span: PINCH_SPAN_MM * self.px_per_mm,
                        angle: 0.0,
                        magnify: false,
                        rotate: false,
                    });
                    let ids = [next_id(&mut s), next_id(&mut s)];
                    let [a, b] = fingers(s.pinch.as_ref().unwrap());
                    self.devices.emit(
                        TOUCHSCREEN,
                        t,
                        &[
                            (EV_ABS, ABS_MT_SLOT, 0),
                            (EV_ABS, ABS_MT_TRACKING_ID, ids[0]),
                            (EV_ABS, ABS_MT_POSITION_X, a.0),
                            (EV_ABS, ABS_MT_POSITION_Y, a.1),
                            (EV_ABS, ABS_MT_SLOT, 1),
                            (EV_ABS, ABS_MT_TRACKING_ID, ids[1]),
                            (EV_ABS, ABS_MT_POSITION_X, b.0),
                            (EV_ABS, ABS_MT_POSITION_Y, b.1),
                            (EV_KEY, BTN_TOUCH, 1),
                        ],
                    );
                }
                let p = s.pinch.as_mut().unwrap();
                match twist {
                    Twist::Magnify(_) => p.magnify = true,
                    Twist::Rotate(_) => p.rotate = true,
                }
            }
            Gesture::Changed => {
                let Some(p) = &mut s.pinch else { return };
                match twist {
                    // Never closer than a finger's width.
                    Twist::Magnify(m) => p.span = (p.span * (1.0 + m)).max(8.0 * self.px_per_mm),
                    Twist::Rotate(deg) => p.angle += deg.to_radians(),
                }
                let [a, b] = fingers(p);
                self.devices.emit(
                    TOUCHSCREEN,
                    t,
                    &[
                        (EV_ABS, ABS_MT_SLOT, 0),
                        (EV_ABS, ABS_MT_POSITION_X, a.0),
                        (EV_ABS, ABS_MT_POSITION_Y, a.1),
                        (EV_ABS, ABS_MT_SLOT, 1),
                        (EV_ABS, ABS_MT_POSITION_X, b.0),
                        (EV_ABS, ABS_MT_POSITION_Y, b.1),
                    ],
                );
            }
            Gesture::Ended => {
                let Some(p) = &mut s.pinch else { return };
                match twist {
                    Twist::Magnify(_) => p.magnify = false,
                    Twist::Rotate(_) => p.rotate = false,
                }
                if !p.magnify && !p.rotate {
                    s.pinch = None;
                    self.lift_two(t);
                }
            }
        }
    }

    fn lift_two(&self, t: i64) {
        self.devices.emit(
            TOUCHSCREEN,
            t,
            &[
                (EV_ABS, ABS_MT_SLOT, 0),
                (EV_ABS, ABS_MT_TRACKING_ID, -1),
                (EV_ABS, ABS_MT_SLOT, 1),
                (EV_ABS, ABS_MT_TRACKING_ID, -1),
                (EV_KEY, BTN_TOUCH, 0),
            ],
        );
    }

    /// Press and release `key` (a Linux key, or [`KEYBOARD_BACK`]).
    fn tap(&self, key: u16, t: i64) {
        let code = if key == KEYBOARD_BACK {
            self.devices.key_of(KEYBOARD, keymap::AC_BACK)
        } else {
            Some(key)
        };
        if let Some(code) = code {
            self.devices.emit(KEYBOARD, t, &[(EV_KEY, code, 1)]);
            self.devices.emit(KEYBOARD, t, &[(EV_KEY, code, 0)]);
        }
    }

    /// A physical key with scan code `usage` down or up.
    fn set_key(&self, s: &mut Pointer, usage: u32, down: bool, t: i64) {
        let held = s.keys.iter().position(|&(u, _)| u == usage);
        let code = match (down, held) {
            (true, Some(_)) => return,
            (false, None) => return,
            (true, None) => match self.devices.key_of(KEYBOARD, usage) {
                Some(code) => {
                    s.keys.push((usage, code));
                    code
                }
                None => return,
            },
            // The key it went down as, whatever the keymap says now.
            (false, Some(i)) => s.keys.remove(i).1,
        };
        self.devices
            .emit(KEYBOARD, t, &[(EV_KEY, code, down as i32)]);
    }

    /// `keyDown:` (`repeat`: `isARepeat`) or `keyUp:` of virtual key `mac`,
    /// with `modifierFlags` `flags`.
    pub fn key(&self, mac: u16, down: bool, repeat: bool, flags: u64, t: i64) {
        if flags & COMMAND != 0 {
            // AppKit sends no keyUp: while Command is held: the down
            // presses and releases, and each repeat does again.
            if down {
                self.shortcut(mac, t);
            }
            return;
        }
        if down && flags & OPTION != 0 {
            let word = match mac {
                MAC_LEFT => Some(KEY_LEFT),
                MAC_RIGHT => Some(KEY_RIGHT),
                MAC_DELETE => Some(KEY_BACKSPACE),
                MAC_FORWARD_DELETE => Some(KEY_DELETE),
                _ => None,
            };
            // Each AppKit repeat does it again; its keyUp finds no key
            // down.
            if let Some(key) = word {
                return self.chord(&[KEY_LEFTCTRL], true, key, t);
            }
        }
        if repeat {
            return;
        }
        if let Some(usage) = keymap::usage(mac) {
            self.set_key(&mut self.state.lock().unwrap(), usage, down, t);
        }
    }

    /// Cmd with virtual key `mac`: Back, a Mac text shortcut, or Ctrl and
    /// the key.
    fn shortcut(&self, mac: u16, t: i64) {
        let (ctrl, key) = match mac {
            MAC_LEFT_BRACKET => return self.tap(KEYBOARD_BACK, t),
            MAC_LEFT => (false, KEY_HOME),
            MAC_RIGHT => (false, KEY_END),
            MAC_UP => (true, KEY_HOME),
            MAC_DOWN => (true, KEY_END),
            MAC_DELETE => {
                self.chord(&[KEY_LEFTSHIFT], false, KEY_HOME, t);
                return self.tap(KEY_BACKSPACE, t);
            }
            MAC_FORWARD_DELETE => {
                self.chord(&[KEY_LEFTSHIFT], false, KEY_END, t);
                return self.tap(KEY_DELETE, t);
            }
            _ => match keymap::linux_key(mac) {
                Some(k) => (true, k),
                None => return,
            },
        };
        let mods: &[u16] = if ctrl { &[KEY_LEFTCTRL] } else { &[] };
        self.chord(mods, false, key, t);
    }

    /// Press and release `key` with modifiers `mods` (left ones) down,
    /// pressing only those not held on the Mac already (either side);
    /// `lift_alt` releases the Alts held meanwhile.
    fn chord(&self, mods: &[u16], lift_alt: bool, key: u16, t: i64) {
        let held: Vec<u16> = self
            .state
            .lock()
            .unwrap()
            .keys
            .iter()
            .map(|k| k.1)
            .collect();
        let right = |m: u16| match m {
            KEY_LEFTCTRL => KEY_RIGHTCTRL,
            KEY_LEFTSHIFT => KEY_RIGHTSHIFT,
            _ => m,
        };
        let press: Vec<u16> = mods
            .iter()
            .copied()
            .filter(|&m| !held.contains(&m) && !held.contains(&right(m)))
            .collect();
        let lift: Vec<u16> = held
            .into_iter()
            .filter(|&k| lift_alt && (k == KEY_LEFTALT || k == KEY_RIGHTALT))
            .collect();
        let emit = |k: u16, v: i32| self.devices.emit(KEYBOARD, t, &[(EV_KEY, k, v)]);
        lift.iter().for_each(|&k| emit(k, 0));
        press.iter().for_each(|&k| emit(k, 1));
        self.tap(key, t);
        press.iter().rev().for_each(|&k| emit(k, 0));
        lift.iter().for_each(|&k| emit(k, 1));
    }

    /// `flagsChanged:` for virtual key `mac` with `modifierFlags` `flags`.
    /// Command is the shortcut key and stays on the Mac.
    pub fn flags_changed(&self, mac: u16, flags: u64, t: i64) {
        let (Some(m), Some(usage)) = (keymap::modifier(mac, flags), keymap::usage(mac)) else {
            return;
        };
        if matches!(keymap::linux_key(mac), Some(KEY_LEFTMETA | KEY_RIGHTMETA)) {
            return;
        }
        let mut s = self.state.lock().unwrap();
        match m {
            Modifier::Held(down) => self.set_key(&mut s, usage, down, t),
            Modifier::Toggled => {
                self.set_key(&mut s, usage, true, t);
                self.set_key(&mut s, usage, false, t);
            }
        }
    }

    pub fn devices(&self) -> &Devices {
        &self.devices
    }

    /// Remove the devices ([`Devices::close`]).
    pub fn close(&self) {
        self.devices.close();
    }

    /// The window lost the keyboard or the mouse: lift the fingers, release
    /// every key and button, so nothing stays down in the guest.
    pub fn release_all(&self, t: i64) {
        let mut s = self.state.lock().unwrap();
        if s.touching {
            s.touching = false;
            self.lift(t);
        }
        if s.pinch.take().is_some() {
            self.lift_two(t);
        }
        for (_, code) in std::mem::take(&mut s.keys) {
            self.devices.emit(KEYBOARD, t, &[(EV_KEY, code, 0)]);
        }
        for b in std::mem::take(&mut s.buttons) {
            self.devices.emit(MOUSE, t, &[(EV_KEY, b.code(), 0)]);
        }
    }
}

/// Back, for [`Input::tap`]: the keyboard's key for AC Back.
const KEYBOARD_BACK: u16 = u16::MAX;

fn next_id(s: &mut Pointer) -> i32 {
    let id = s.next_tracking_id;
    s.next_tracking_id = (id + 1) & 0xffff;
    id
}

/// `units` scroll units more on `axis`: the `_HI_RES` value and the whole
/// units to send now.
fn units(s: &mut Pointer, axis: usize, units: f64) -> (i32, i32) {
    let hi_res = units * 120.0 + s.hi_res_rest[axis];
    let whole_hi_res = hi_res.trunc();
    s.hi_res_rest[axis] = hi_res - whole_hi_res;
    let hi_res = whole_hi_res as i32;
    let total = s.unit_rest[axis] + hi_res;
    let whole = total / 120;
    s.unit_rest[axis] = total - whole * 120;
    (hi_res, whole)
}

/// The two fingers of a pinch, kept inside its area.
fn fingers(p: &Pinch) -> [(i32, i32); 2] {
    let (r, a) = (p.span / 2.0, p.angle);
    let (ox, oy) = (r * a.cos(), -r * a.sin());
    let [l, top, right, bottom] = p.area;
    let at = |x: f64, y: f64| {
        (
            x.clamp(l, right - 1.0).round() as i32,
            y.clamp(top, bottom - 1.0).round() as i32,
        )
    };
    [
        at(p.center.0 - ox, p.center.1 - oy),
        at(p.center.0 + ox, p.center.1 + oy),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::{Descriptor, Hello, OP_OPEN, Record, VERSION, connect_in, devices};
    use super::*;
    use crate::wire;
    use std::io::Read;
    use std::os::fd::AsFd;
    use std::os::unix::net::UnixStream;

    fn setup(name: &str) -> (Input, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let input = Input::new(
            Devices::create(&dir, devices(100, 200, 254.0, 254.0)).unwrap(),
            100,
            200,
            254.0,
        );
        (input, dir)
    }

    fn open(dir: &std::path::Path, index: u32) -> UnixStream {
        let k = connect_in(dir, &format!("event{index}")).unwrap();
        k.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let hello = Hello {
            version: VERSION,
            op: OP_OPEN,
            ..Default::default()
        };
        wire::send(k.as_fd(), wire::bytes(&hello), None).unwrap();
        wire::recv_record::<Descriptor>(k.as_fd()).unwrap().unwrap();
        k
    }

    /// `n` records, as (type, code, value), reports left out.
    fn read(k: &mut UnixStream, n: usize) -> Vec<(u16, u16, i32)> {
        let mut v = vec![Record::default(); n];
        // SAFETY: `Record` is plain old data.
        let b = unsafe {
            std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), n * size_of::<Record>())
        };
        let mut got = 0;
        while got < b.len() {
            match k.read(&mut b[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        v.truncate(got / size_of::<Record>());
        v.iter()
            .filter(|r| r.kind != EV_SYN)
            .map(|r| (r.kind, r.code, r.value))
            .collect()
    }

    fn keys(k: &mut UnixStream, n: usize) -> Vec<(u16, i32)> {
        read(k, n).into_iter().map(|(_, c, v)| (c, v)).collect()
    }

    #[test]
    fn back_from_the_shortcut_the_button_and_a_swipe() {
        let (input, dir) = setup("back");
        let mut k = open(&dir, KEYBOARD);
        // Cmd+[ is Back, pressed and released at its down; [ alone is [.
        input.key(0x21, true, false, COMMAND, 1);
        input.key(0x21, false, false, COMMAND, 2);
        input.key(0x21, true, false, 0, 3);
        input.key(0x21, false, false, 0, 4);
        assert_eq!(
            keys(&mut k, 8),
            [(KEY_BACK, 1), (KEY_BACK, 0), (26, 1), (26, 0)]
        );

        // A swipe to the right is Back once and does not scroll; a mostly
        // vertical one, or one to the left, scrolls.
        let mut m = open(&dir, MOUSE);
        let swipe = |phase, fx, fy| Scroll {
            x: 10.0,
            y: 20.0,
            dx: fx * 2.0,
            dy: fy * 2.0,
            precise: true,
            swipe: Some((phase, fx, fy)),
            ..Default::default()
        };
        input.scroll(swipe(Gesture::Began, 10.0, 0.0), 8);
        for _ in 0..10 {
            input.scroll(swipe(Gesture::Changed, 20.0, 2.0), 9);
        }
        input.scroll(swipe(Gesture::Ended, 0.0, 0.0), 10);
        // Its momentum does not scroll either.
        let momentum = Scroll {
            dx: 100.0,
            precise: true,
            momentum: true,
            ..Default::default()
        };
        input.scroll(momentum, 10);
        input.scroll(swipe(Gesture::Began, 0.0, 0.0), 11);
        input.scroll(swipe(Gesture::Changed, -64.0, 0.0), 12);
        input.scroll(swipe(Gesture::Changed, 200.0, 0.0), 12);
        input.scroll(swipe(Gesture::Ended, 0.0, 0.0), 13);
        assert_eq!(keys(&mut k, 4), [(KEY_BACK, 1), (KEY_BACK, 0)]);
        // The leftward gesture scrolled: its fingers moved 64 points, the
        // content 128 pixels left: a unit to the right.
        assert_eq!(
            read(&mut m, 6),
            [
                (EV_KEY, BTN_TOOL_MOUSE, 1),
                (EV_ABS, ABS_X, 10),
                (EV_ABS, ABS_Y, 20),
                (EV_REL, REL_HWHEEL, 1),
                (EV_REL, REL_HWHEEL_HI_RES, 120),
            ]
        );
        input.close();
    }

    #[test]
    fn command_is_ctrl() {
        let (input, dir) = setup("cmd");
        let mut k = open(&dir, KEYBOARD);
        // Command alone does not reach Android.
        input.flags_changed(0x37, COMMAND | 0x08, 1);
        // Cmd+C, and its repeat: Ctrl+C each time.
        input.key(0x08, true, false, COMMAND, 2);
        input.key(0x08, true, true, COMMAND, 3);
        input.key(0x08, false, false, COMMAND, 4);
        // Cmd+Left is Home, Cmd+Down Ctrl+End.
        input.key(0x7b, true, false, COMMAND, 5);
        input.key(0x7d, true, false, COMMAND, 6);
        input.flags_changed(0x37, 0, 7);
        assert_eq!(
            keys(&mut k, 28),
            [
                (KEY_LEFTCTRL, 1),
                (46, 1),
                (46, 0),
                (KEY_LEFTCTRL, 0),
                (KEY_LEFTCTRL, 1),
                (46, 1),
                (46, 0),
                (KEY_LEFTCTRL, 0),
                (KEY_HOME, 1),
                (KEY_HOME, 0),
                (KEY_LEFTCTRL, 1),
                (KEY_END, 1),
                (KEY_END, 0),
                (KEY_LEFTCTRL, 0),
            ]
        );
        // A Ctrl held on the Mac stays down through Cmd+V.
        input.flags_changed(0x3b, 1 << 18 | 0x01, 8);
        input.key(0x09, true, false, COMMAND | 1 << 18, 9);
        assert_eq!(keys(&mut k, 6), [(KEY_LEFTCTRL, 1), (47, 1), (47, 0)]);
        input.close();
    }

    #[test]
    fn mac_text_shortcuts() {
        let (input, dir) = setup("text");
        let mut k = open(&dir, KEYBOARD);
        let option = OPTION | 0x20;
        let shift = 1 << 17 | 0x02;
        // Option+Left is Ctrl+Left, with Alt lifted meanwhile; its repeat
        // does it again and its keyUp sends nothing.
        input.flags_changed(0x3a, option, 1);
        input.key(0x7b, true, false, option, 2);
        input.key(0x7b, true, true, option, 3);
        input.key(0x7b, false, false, option, 4);
        let word = |key| {
            [
                (KEY_LEFTALT, 0),
                (KEY_LEFTCTRL, 1),
                (key, 1),
                (key, 0),
                (KEY_LEFTCTRL, 0),
                (KEY_LEFTALT, 1),
            ]
        };
        assert_eq!(keys(&mut k, 2), [(KEY_LEFTALT, 1)]);
        assert_eq!(keys(&mut k, 24), [word(KEY_LEFT), word(KEY_LEFT)].concat());
        // Option+Delete deletes a word: Ctrl+Backspace; Option+Shift+Right
        // selects one: Shift stays down.
        input.key(0x33, true, false, option, 5);
        input.flags_changed(0x38, option | shift, 6);
        input.key(0x7c, true, false, option | shift, 7);
        input.flags_changed(0x38, option, 8);
        input.flags_changed(0x3a, 0, 9);
        assert_eq!(
            keys(&mut k, 30),
            [
                &word(KEY_BACKSPACE)[..],
                &[(KEY_LEFTSHIFT, 1)],
                &word(KEY_RIGHT),
                &[(KEY_LEFTSHIFT, 0), (KEY_LEFTALT, 0)],
            ]
            .concat()
        );
        // Cmd+Delete deletes to the line's start, Cmd+Forward Delete to
        // its end.
        input.key(0x33, true, false, COMMAND, 10);
        input.key(0x75, true, false, COMMAND, 11);
        assert_eq!(
            keys(&mut k, 24),
            [
                (KEY_LEFTSHIFT, 1),
                (KEY_HOME, 1),
                (KEY_HOME, 0),
                (KEY_LEFTSHIFT, 0),
                (KEY_BACKSPACE, 1),
                (KEY_BACKSPACE, 0),
                (KEY_LEFTSHIFT, 1),
                (KEY_END, 1),
                (KEY_END, 0),
                (KEY_LEFTSHIFT, 0),
                (KEY_DELETE, 1),
                (KEY_DELETE, 0),
            ]
        );
        // Cmd+Shift+Left and Up select to the line's and the text's start:
        // Shift+Home and Ctrl+Shift+Home, Shift held from the Mac.
        input.flags_changed(0x38, shift, 12);
        input.key(0x7b, true, false, COMMAND | shift, 13);
        input.key(0x7e, true, false, COMMAND | shift, 14);
        input.flags_changed(0x38, 0, 15);
        assert_eq!(
            keys(&mut k, 16),
            [
                (KEY_LEFTSHIFT, 1),
                (KEY_HOME, 1),
                (KEY_HOME, 0),
                (KEY_LEFTCTRL, 1),
                (KEY_HOME, 1),
                (KEY_HOME, 0),
                (KEY_LEFTCTRL, 0),
                (KEY_LEFTSHIFT, 0),
            ]
        );
        input.close();
    }

    #[test]
    fn mouse_hovers_clicks_and_scrolls_where_the_pointer_is() {
        let (input, dir) = setup("mouse");
        let mut m = open(&dir, MOUSE);
        input.hover(30.4, 40.6, 1);
        // The right button, and the back button, which Android turns into
        // Back (BUTTON_BACK).
        input.button(Button::Right, true, 31.0, 40.0, 2);
        input.button(Button::Right, false, 31.0, 40.0, 3);
        input.button(Button::Back, true, 31.0, 40.0, 3);
        input.button(Button::Back, false, 31.0, 40.0, 3);
        // A wheel line; a trackpad's pixels, both axes.
        let wheel = Scroll {
            x: 31.0,
            y: 40.0,
            dy: 1.0,
            ..Default::default()
        };
        input.scroll(wheel, 4);
        let pad = Scroll {
            x: 31.0,
            y: 40.0,
            dx: 64.0,
            dy: -32.0,
            precise: true,
            ..Default::default()
        };
        input.scroll(pad, 5);
        input.leave(6);
        assert_eq!(
            read(&mut m, 21),
            [
                (EV_KEY, BTN_TOOL_MOUSE, 1),
                (EV_ABS, ABS_X, 30),
                (EV_ABS, ABS_Y, 40),
                (EV_ABS, ABS_X, 31),
                (EV_KEY, BTN_RIGHT, 1),
                (EV_KEY, BTN_RIGHT, 0),
                (EV_KEY, BTN_SIDE, 1),
                (EV_KEY, BTN_SIDE, 0),
                (EV_REL, REL_WHEEL, 1),
                (EV_REL, REL_WHEEL_HI_RES, 120),
                // Half a unit left, a quarter down: no whole units yet.
                (EV_REL, REL_WHEEL_HI_RES, -30),
                (EV_REL, REL_HWHEEL_HI_RES, -60),
                (EV_KEY, BTN_TOOL_MOUSE, 0),
            ]
        );
        input.close();
    }

    #[test]
    fn pinch_is_two_fingers_on_the_pointer() {
        let (input, dir) = setup("pinch");
        let mut t = open(&dir, TOUCHSCREEN);
        let area = input.display();
        input.twist(Twist::Magnify(0.0), Gesture::Began, 50.0, 100.0, area, 1);
        // 40 mm at 10 px/mm: 400 px, clamped to the 100 px wide display.
        input.twist(Twist::Rotate(0.0), Gesture::Began, 50.0, 100.0, area, 2);
        input.twist(Twist::Magnify(-0.9), Gesture::Changed, 0.0, 0.0, area, 3);
        input.twist(Twist::Rotate(90.0), Gesture::Changed, 0.0, 0.0, area, 4);
        input.twist(Twist::Magnify(0.0), Gesture::Ended, 0.0, 0.0, area, 5);
        input.twist(Twist::Rotate(0.0), Gesture::Ended, 0.0, 0.0, area, 6);
        let got = read(&mut t, 26);
        // The first finger's X is the slot's 0 already: not sent again.
        assert_eq!(
            got[..7],
            [
                (EV_ABS, ABS_MT_TRACKING_ID, 0),
                (EV_ABS, ABS_MT_POSITION_Y, 100),
                (EV_ABS, ABS_MT_SLOT, 1),
                (EV_ABS, ABS_MT_TRACKING_ID, 1),
                (EV_ABS, ABS_MT_POSITION_X, 99),
                (EV_ABS, ABS_MT_POSITION_Y, 100),
                (EV_KEY, BTN_TOUCH, 1),
            ]
        );
        // Closer, but not closer than 8 mm: 10 to 90.
        assert_eq!(
            got[7..11],
            [
                (EV_ABS, ABS_MT_SLOT, 0),
                (EV_ABS, ABS_MT_POSITION_X, 10),
                (EV_ABS, ABS_MT_SLOT, 1),
                (EV_ABS, ABS_MT_POSITION_X, 90),
            ]
        );
        // Turned a quarter: one above the other.
        assert_eq!(
            got[11..17],
            [
                (EV_ABS, ABS_MT_SLOT, 0),
                (EV_ABS, ABS_MT_POSITION_X, 50),
                (EV_ABS, ABS_MT_POSITION_Y, 140),
                (EV_ABS, ABS_MT_SLOT, 1),
                (EV_ABS, ABS_MT_POSITION_X, 50),
                (EV_ABS, ABS_MT_POSITION_Y, 60),
            ]
        );
        // Both gestures over: the fingers lift.
        assert_eq!(
            got[17..],
            [
                (EV_ABS, ABS_MT_SLOT, 0),
                (EV_ABS, ABS_MT_TRACKING_ID, -1),
                (EV_ABS, ABS_MT_SLOT, 1),
                (EV_ABS, ABS_MT_TRACKING_ID, -1),
                (EV_KEY, BTN_TOUCH, 0),
            ]
        );
        input.close();
    }

    #[test]
    fn smart_zoom_is_a_double_tap() {
        let (input, dir) = setup("zoom");
        let mut t = open(&dir, TOUCHSCREEN);
        let taps = double_tap(1_000_000_000);
        let ms = |i: usize, j: usize| (taps[j].1 - taps[i].1) / 1_000_000;
        // Two taps shorter than TAP_TIMEOUT, the second down within the
        // double tap window, ending at the gesture's time.
        assert!(ms(0, 1) < 100 && ms(2, 3) < 100);
        assert!((40..=300).contains(&ms(1, 2)));
        assert_eq!(taps[3].1, 1_000_000_000);
        for (phase, at) in taps {
            input.pointer(phase, 25.0, 50.0, 50.0, 100.0, at);
        }
        let down = |id| {
            [
                (EV_ABS, ABS_MT_TRACKING_ID, id),
                (EV_ABS, ABS_MT_POSITION_X, 50),
                (EV_ABS, ABS_MT_POSITION_Y, 100),
                (EV_KEY, BTN_TOUCH, 1),
            ]
        };
        let up = [(EV_ABS, ABS_MT_TRACKING_ID, -1), (EV_KEY, BTN_TOUCH, 0)];
        // The second tap's position is unchanged: not sent again.
        assert_eq!(
            read(&mut t, 13),
            [&down(0)[..], &up, &down(1)[..1], &down(1)[3..], &up].concat()
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
