//! Window mode's geometry (`docs/windows.md`).
//!
//! In window mode the display is congruent with the Mac's main screen:
//! display pixel (x, y) lies under screen point (x, H·s − y) / s, for a
//! screen H points tall at s pixels per point, and a task's window sits
//! exactly over the task. Android pixels and macOS points stay separate
//! coordinate systems; this module is the one place that converts.
//!
use aim_hostcall::display::windowing_mode;

/// The Mac's main screen: its size in points and pixels per point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// A rectangle in AppKit's screen coordinates: points, origin at the main
/// screen's bottom left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Screen {
    /// The display's size in backing pixels.
    pub fn display_size(&self) -> (u32, u32) {
        (
            (self.width * self.scale).round() as u32,
            (self.height * self.scale).round() as u32,
        )
    }

    /// The screen rectangle over display pixels `r` (left, top, right,
    /// bottom).
    pub fn to_frame(&self, r: [i32; 4]) -> Frame {
        let s = self.scale;
        let [l, t, right, b] = r.map(|v| v as f64 / s);
        Frame {
            x: l,
            y: self.height - b,
            width: right - l,
            height: b - t,
        }
    }

    /// The display pixels under screen rectangle `f`, rounded to whole
    /// pixels.
    pub fn to_pixels(&self, f: Frame) -> [i32; 4] {
        let s = self.scale;
        let px = |v: f64| (v * s).round() as i32;
        [
            px(f.x),
            px(self.height - f.y - f.height),
            px(f.x + f.width),
            px(self.height - f.y),
        ]
    }
}

/// Window content `c` (a frame) turned to landscape or portrait
/// proportions, as an activity asked: its width and height swapped around
/// its centre, fitted into `area`. None when it has them already, or is
/// square.
pub fn turn(c: Frame, landscape: bool, area: Frame) -> Option<Frame> {
    if c.width == c.height || (c.width > c.height) == landscape {
        return None;
    }
    Some(fit(c, c.height, c.width, area))
}

/// A `width` by `height` frame around `c`'s centre, scaled down (in
/// proportion) to fit into `area` and moved into it.
pub fn fit(c: Frame, width: f64, height: f64, area: Frame) -> Frame {
    let k = (area.width / width).min(area.height / height).min(1.0);
    let (width, height) = (width * k, height * k);
    Frame {
        x: (c.x + (c.width - width) / 2.0).clamp(area.x, area.x + area.width - width),
        y: (c.y + (c.height - height) / 2.0).clamp(area.y, area.y + area.height - height),
        width,
        height,
    }
}

/// The display pixel under point (`x`, `y`) of a view `view_height` points
/// tall (origin at the bottom left) whose top left shows display pixel
/// `origin`, one pixel per 1/`scale` point.
pub fn view_to_display(
    x: f64,
    y: f64,
    view_height: f64,
    origin: [i32; 2],
    scale: f64,
) -> (f64, f64) {
    (
        origin[0] as f64 + x * scale,
        origin[1] as f64 + (view_height - y) * scale,
    )
}

/// A task window's place in the Mac's full screen, which follows its
/// task's windowing mode: a fullscreen task's window is in a space of its
/// own, any other's is not. Toggling either way animates; while it does,
/// AppKit owns the window's frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FullScreen {
    /// In the full screen, or going there.
    pub on: bool,
    /// Between AppKit's `windowWill…FullScreen` and its end.
    pub moving: bool,
    /// The user toggled it, and the task's record has not answered yet.
    pub asked: bool,
}

impl FullScreen {
    /// Whether the window should toggle to follow its task's windowing
    /// `mode`: never while it moves or waits for an answer.
    pub fn toggle(&self, mode: i32) -> bool {
        !self.moving && !self.asked && self.on != (mode == windowing_mode::FULLSCREEN)
    }

    /// Whether AppKit places the window: in the full screen or on its way
    /// in or out.
    pub fn places(&self) -> bool {
        self.on || self.moving
    }

    /// Whether the window takes its task's bounds, for a task in windowing
    /// `mode`: not while AppKit places it, nor while it is out of the full
    /// screen for a fullscreen task (on its way in, or waiting for the
    /// answer to the user's leaving it). AppKit animates into the full
    /// screen from the window's frame, and back to it.
    pub fn follows(&self, mode: i32) -> bool {
        !self.places() && mode != windowing_mode::FULLSCREEN
    }

    /// AppKit starts to move the window into (`on`) or out of the full
    /// screen, for the task in windowing `mode`: the mode to ask the task
    /// for when the user did it, not the task.
    pub fn will(&mut self, on: bool, mode: i32) -> Option<i32> {
        self.on = on;
        self.moving = true;
        if on == (mode == windowing_mode::FULLSCREEN) {
            return None;
        }
        self.asked = true;
        Some(if on {
            windowing_mode::FULLSCREEN
        } else {
            windowing_mode::FREEFORM
        })
    }

    /// The move ended with the window in the full screen (`on`) or not,
    /// as it went or after AppKit failed to move it.
    pub fn did(&mut self, on: bool) {
        self.on = on;
        self.moving = false;
    }

    /// The task's record (its windowing mode) answers what the user asked.
    pub fn answered(&mut self) {
        self.asked = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETINA: Screen = Screen {
        width: 1728.0,
        height: 1117.0,
        scale: 2.0,
    };

    #[test]
    fn the_display_is_the_screen_in_backing_pixels() {
        assert_eq!(RETINA.display_size(), (3456, 2234));
        let one_x = Screen {
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
        };
        assert_eq!(one_x.display_size(), (1920, 1080));
    }

    #[test]
    fn pixels_and_points_round_trip() {
        // A 1080x1920 task 100 px from the left and 200 from the top: its
        // frame is half as large in points, measured from the bottom.
        let r = [100, 200, 1180, 2120];
        let f = RETINA.to_frame(r);
        assert_eq!(
            f,
            Frame {
                x: 50.0,
                y: 1117.0 - 1060.0,
                width: 540.0,
                height: 960.0
            }
        );
        assert_eq!(RETINA.to_pixels(f), r);
        // A frame between pixels rounds to whole ones.
        let f = Frame {
            x: 10.3,
            y: 20.2,
            width: 100.0,
            height: 50.0,
        };
        assert_eq!(RETINA.to_pixels(f), [21, 2094, 221, 2194]);
    }

    fn f(x: f64, y: f64, width: f64, height: f64) -> Frame {
        Frame {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn turning_swaps_the_sides_around_the_centre() {
        let area = f(0.0, 0.0, 1728.0, 1080.0);
        let portrait = f(600.0, 200.0, 400.0, 700.0);
        assert_eq!(
            turn(portrait, true, area),
            Some(f(450.0, 350.0, 700.0, 400.0))
        );
        assert_eq!(turn(portrait, false, area), None);
        assert_eq!(turn(f(0.0, 0.0, 500.0, 500.0), true, area), None);
        // Near an edge it moves into the area; too large, it shrinks in
        // proportion, never filling more than the area.
        assert_eq!(
            turn(f(1500.0, 0.0, 200.0, 400.0), true, area),
            Some(f(1328.0, 100.0, 400.0, 200.0))
        );
        assert_eq!(
            turn(f(0.0, 0.0, 600.0, 2000.0), true, area),
            Some(f(0.0, 561.6, 1728.0, 518.4))
        );
    }

    #[test]
    fn view_points_map_one_to_one() {
        // A 540x960 point view over content at (100, 284): its top left is
        // that pixel, and a point is two pixels down and right.
        assert_eq!(
            view_to_display(0.0, 960.0, 960.0, [100, 284], 2.0),
            (100.0, 284.0)
        );
        assert_eq!(
            view_to_display(10.0, 950.0, 960.0, [100, 284], 2.0),
            (120.0, 304.0)
        );
        assert_eq!(
            view_to_display(540.0, 0.0, 960.0, [100, 284], 2.0),
            (1180.0, 2204.0)
        );
    }

    #[test]
    fn the_full_screen_follows_the_tasks_mode() {
        use windowing_mode::{FREEFORM, FULLSCREEN, UNDEFINED};
        // Android makes the task fullscreen: the window toggles in, asks
        // nothing, and stays there.
        let mut f = FullScreen::default();
        assert!(f.follows(FREEFORM));
        assert!(f.toggle(FULLSCREEN));
        assert!(!f.follows(FULLSCREEN), "it enters from its freeform frame");
        assert!(!f.toggle(FREEFORM));
        assert_eq!(f.will(true, FULLSCREEN), None);
        assert!(!f.toggle(FREEFORM), "not while it moves");
        f.did(true);
        assert!(!f.toggle(FULLSCREEN));
        // Then freeform: out again, back to that frame, and then to the
        // task's bounds.
        assert!(f.toggle(FREEFORM));
        assert!(!f.follows(FREEFORM), "AppKit restores its frame");
        assert_eq!(f.will(false, FREEFORM), None);
        assert!(!f.follows(FREEFORM));
        f.did(false);
        assert_eq!(f, FullScreen::default());
        assert!(f.follows(FREEFORM));
        // A bridge that does not say keeps the window out.
        assert!(!f.toggle(UNDEFINED));
    }

    #[test]
    fn the_users_full_screen_asks_the_task_and_waits_for_its_answer() {
        use windowing_mode::{FREEFORM, FULLSCREEN};
        let mut f = FullScreen::default();
        // The green button on a freeform task's window asks for fullscreen.
        assert_eq!(f.will(true, FREEFORM), Some(FULLSCREEN));
        f.did(true);
        // Until the task's record answers, its old mode does not toggle back.
        assert!(!f.toggle(FREEFORM));
        f.answered();
        assert!(!f.toggle(FULLSCREEN));
        // A refusal (freeform still) takes the window out again.
        assert!(f.toggle(FREEFORM));
        // Leaving it asks for freeform; out, the window waits for the
        // answer's bounds instead of taking the fullscreen task's.
        assert_eq!(f.will(false, FULLSCREEN), Some(FREEFORM));
        f.did(false);
        assert!(!f.follows(FULLSCREEN));
        f.answered();
        assert!(!f.toggle(FREEFORM));
        assert!(f.follows(FREEFORM));
        // AppKit failing to enter keeps the window out, and it toggles
        // again for a task that is fullscreen.
        f.will(true, FULLSCREEN);
        f.did(false);
        assert!(f.toggle(FULLSCREEN));
    }
}
