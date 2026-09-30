//! Window mode's geometry (`docs/windows.md`).
//!
//! In window mode the display is congruent with the Mac's main screen:
//! display pixel (x, y) lies under screen point (x, H·s − y) / s, for a
//! screen H points tall at s pixels per point, and a task's window sits
//! exactly over the task. Android pixels and macOS points stay separate
//! coordinate systems; this module is the one place that converts.
//!
//! Below the screen the display has [`BAR_MARGIN`] more rows. Android's
//! navigation bar or taskbar lies there, under no window. Its status bar
//! lies under the menu bar, which no window covers either.
//!
//! A freeform task's caption is the original window decoration's, inside
//! the top of the task's bounds. The window shows the rest of the task,
//! its content; the caption lies under the window's own title bar.

/// Rows below the screen for Android's bottom bars: a taskbar or
/// navigation bar is 48 to 60 dp, 96 to 120 pixels at the image's density.
pub const BAR_MARGIN: u32 = 192;

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
    /// The display's size in pixels: the screen and the bar margin.
    pub fn display_size(&self) -> (u32, u32) {
        (
            (self.width * self.scale).round() as u32,
            (self.height * self.scale).round() as u32 + BAR_MARGIN,
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

/// A task's content: its bounds below the caption.
pub fn content(bounds: [i32; 4], caption: i32) -> [i32; 4] {
    let [l, t, r, b] = bounds;
    [l, (t + caption).min(b), r, b]
}

/// The task bounds whose content is `content`.
pub fn bounds(content: [i32; 4], caption: i32) -> [i32; 4] {
    let [l, t, r, b] = content;
    [l, t - caption, r, b]
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

#[cfg(test)]
mod tests {
    use super::*;

    const RETINA: Screen = Screen {
        width: 1728.0,
        height: 1117.0,
        scale: 2.0,
    };

    #[test]
    fn the_display_is_the_screen_and_the_bar_margin() {
        assert_eq!(RETINA.display_size(), (3456, 2234 + BAR_MARGIN));
        let one_x = Screen {
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
        };
        assert_eq!(one_x.display_size(), (1920, 1080 + BAR_MARGIN));
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

    #[test]
    fn content_is_below_the_caption() {
        let b = [100, 200, 1180, 2120];
        assert_eq!(content(b, 84), [100, 284, 1180, 2120]);
        assert_eq!(bounds(content(b, 84), 84), b);
        // A task shorter than its caption has no content.
        assert_eq!(content([0, 0, 10, 50], 84), [0, 50, 10, 50]);
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
}
