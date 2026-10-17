//! Where a new task opens in window mode: the bounds freeform windowing
//! gives a task started without bounds of its own, as
//! `TaskLaunchParamsModifier.getTaskBounds` lays it out at
//! `android-16.0.0_r1` (`LaunchParamsUtil.getDefaultFreeformSize`,
//! `adjustBoundsToFitInDisplayArea`, `centerBounds`, then
//! `adjustBoundsToAvoidConflict`). The display has no bars, so its stable
//! area is all of it.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::config::Rect;
use crate::package::info::ActivityInfo;

/// `ActivityInfo.SCREEN_ORIENTATION_*`.
const LANDSCAPE: i32 = 0;
const PORTRAIT: i32 = 1;
const NOSENSOR: i32 = 5;
const SENSOR_LANDSCAPE: i32 = 6;
const REVERSE_LANDSCAPE: i32 = 8;
const USER_LANDSCAPE: i32 = 11;
const LOCKED: i32 = 14;
/// `LaunchParamsUtil`'s default sizes and display edge offset, in dp.
const PORTRAIT_SIZE_DP: (i32, i32) = (412, 732);
const LANDSCAPE_SIZE_DP: (i32, i32) = (1064, 600);
const DISPLAY_EDGE_OFFSET_DP: i32 = 27;
/// `TaskLaunchParamsModifier`'s.
const BOUNDS_CONFLICT_THRESHOLD: i32 = 4;
const STEP_DENOMINATOR: i32 = 16;
const MINIMAL_STEP: i32 = 1;

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    fn offset(&mut self, dx: i32, dy: i32) {
        *self = Rect { left: self.left + dx, top: self.top + dy, right: self.right + dx, bottom: self.bottom + dy };
    }

    fn contains(&self, r: &Rect) -> bool {
        self.left < self.right && self.top < self.bottom && self.left <= r.left && self.top <= r.top && self.right >= r.right && self.bottom >= r.bottom
    }
}

/// What of a task's root activity its bounds depend on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Activity {
    pub screen_orientation: i32,
    /// Its window layout's minimum width and height, -1 for none.
    pub min_size: (i32, i32),
    pub min_aspect_ratio: f32,
    pub max_aspect_ratio: f32,
}

impl From<&ActivityInfo> for Activity {
    fn from(info: &ActivityInfo) -> Activity {
        Activity {
            screen_orientation: info.screen_orientation,
            min_size: info.window_layout.as_ref().map_or((-1, -1), |l| (l.min_width, l.min_height)),
            min_aspect_ratio: info.min_aspect_ratio,
            max_aspect_ratio: info.max_aspect_ratio,
        }
    }
}

/// The bounds of a new task of `info` on a display of `display` bounds
/// and `density_dpi`, laid out right to left if `rtl`, away from the
/// freeform tasks of `others` bounds.
pub fn default_bounds(info: &Activity, display: Rect, density_dpi: i32, rtl: bool, others: &[Rect]) -> Rect {
    let orientation = match info.screen_orientation {
        SENSOR_LANDSCAPE | REVERSE_LANDSCAPE | USER_LANDSCAPE | LANDSCAPE => LANDSCAPE,
        // Locked to the display's, without suggested bounds.
        NOSENSOR | LOCKED if display.width() > display.height() => LANDSCAPE,
        // Portrait ones, and unspecified without suggested bounds.
        _ => PORTRAIT,
    };
    let density = density_dpi as f32 / 160.0;
    let (min_width, min_height) = info.min_size;
    let (width, height) = default_size(info, orientation, density, (min_width, min_height), display);
    let fitted = fit_in_display(display, density, rtl, (min_width, min_height), Rect { left: 0, top: 0, right: width, bottom: height });
    let (center_x, center_y) = ((display.left + display.right) / 2, (display.top + display.bottom) / 2);
    let (left, top) = (center_x - fitted.width() / 2, center_y - fitted.height() / 2);
    let mut bounds = Rect { left, top, right: left + fitted.width(), bottom: top + fitted.height() };
    avoid_conflict(display, others, &mut bounds);
    bounds
}

/// `LaunchParamsUtil.getDefaultFreeformSize`.
fn default_size(info: &Activity, orientation: i32, density: f32, min: (i32, i32), stable: Rect) -> (i32, i32) {
    let (width_dp, height_dp) = if orientation == LANDSCAPE { LANDSCAPE_SIZE_DP } else { PORTRAIT_SIZE_DP };
    let freeform = ((width_dp as f32 * density + 0.5) as i32, (height_dp as f32 * density + 0.5) as i32);
    let portrait_height = stable.width().min(stable.height());
    let other = stable.width().max(stable.height());
    let portrait_width = portrait_height * portrait_height / other;
    let (max_width, max_height) =
        if orientation == LANDSCAPE { (portrait_height, portrait_width) } else { (portrait_width, portrait_height) };
    let width = max_width.min(freeform.0.max(min.0));
    let height = max_height.min(freeform.1.max(min.1));
    let aspect = width.max(height) as f32 / width.min(height) as f32;
    let (mut adjusted_width, mut adjusted_height) = (width, height);
    let landscape = orientation == LANDSCAPE;
    for (ratio, applies) in [
        (info.min_aspect_ratio, info.min_aspect_ratio >= 1.0 && aspect < info.min_aspect_ratio),
        (info.max_aspect_ratio, info.max_aspect_ratio >= 1.0 && aspect > info.max_aspect_ratio),
    ] {
        if applies {
            if landscape {
                adjusted_height = (adjusted_width as f32 / ratio + 0.5) as i32;
            } else {
                adjusted_width = (adjusted_height as f32 / ratio + 0.5) as i32;
            }
            break;
        }
    }
    (adjusted_width, adjusted_height)
}

/// `LaunchParamsUtil.adjustBoundsToFitInDisplayArea`.
fn fit_in_display(display: Rect, density: f32, rtl: bool, min: (i32, i32), mut bounds: Rect) -> Rect {
    let edge = (DISPLAY_EDGE_OFFSET_DP as f32 * density + 0.5) as i32;
    let stable = Rect { left: display.left + edge, top: display.top + edge, right: display.right - edge, bottom: display.bottom - edge };
    if stable.width() < bounds.width() || stable.height() < bounds.height() {
        let shrink = (stable.width() as f32 / bounds.width() as f32).min(stable.height() as f32 / bounds.height() as f32);
        let width = min.0.max((bounds.width() as f32 * shrink) as i32);
        let height = min.1.max((bounds.height() as f32 * shrink) as i32);
        if stable.width() < width || stable.height() < height {
            let left = if rtl { stable.right - width } else { stable.left };
            return Rect { left, top: stable.top, right: left + width, bottom: stable.top + height };
        }
        bounds = Rect { left: bounds.left, top: bounds.top, right: bounds.left + width, bottom: bounds.top + height };
    }
    let dx = if bounds.right > stable.right {
        stable.right - bounds.right
    } else if bounds.left < stable.left {
        stable.left - bounds.left
    } else {
        0
    };
    let dy = if bounds.top < stable.top {
        stable.top - bounds.top
    } else if bounds.bottom > stable.bottom {
        stable.bottom - bounds.bottom
    } else {
        0
    };
    bounds.offset(dx, dy);
    bounds
}

/// `TaskLaunchParamsModifier.adjustBoundsToAvoidConflict`: scans away
/// from the display's middle for bounds whose corners lie near no other
/// task's, else keeps them.
fn avoid_conflict(display: Rect, others: &[Rect], bounds: &mut Rect) {
    if !display.contains(bounds) || !conflicts(others, bounds) {
        return;
    }
    let (center_x, center_y) = ((bounds.left + bounds.right) / 2, (bounds.top + bounds.bottom) / 2);
    let third = |a: i32, b: i32| ((2 * a + b) / 3, (a + 2 * b) / 3);
    let (left_third, right_third) = third(display.left, display.right);
    let (top_third, bottom_third) = third(display.top, display.bottom);
    // Directions as (x, y) steps' signs.
    let directions: &[(i32, i32)] = if center_x < left_third {
        &[(1, 0)]
    } else if center_x > right_third {
        &[(-1, 0)]
    } else if center_y < top_third || center_y > bottom_third {
        &[(1, 0), (-1, 0)]
    } else {
        &[(1, 1), (-1, -1)]
    };
    let step = (MINIMAL_STEP.max(display.width() / STEP_DENOMINATOR), MINIMAL_STEP.max(display.height() / STEP_DENOMINATOR));
    for (x, y) in directions {
        let mut candidate = *bounds;
        while conflicts(others, &candidate) && display.contains(&candidate) {
            candidate.offset(x * step.0, y * step.1);
        }
        if !conflicts(others, &candidate) && display.contains(&candidate) {
            *bounds = candidate;
            return;
        }
    }
}

/// `TaskLaunchParamsModifier.boundsConflict`: a corner of `candidate`
/// lies near the same corner of another task.
fn conflicts(others: &[Rect], candidate: &Rect) -> bool {
    others.iter().any(|t| {
        let close = |a: i32, b: i32| (a - b).abs() < BOUNDS_CONFLICT_THRESHOLD;
        let (left, top) = (close(t.left, candidate.left), close(t.top, candidate.top));
        let (right, bottom) = (close(t.right, candidate.right), close(t.bottom, candidate.bottom));
        (left && top) || (left && bottom) || (right && top) || (right && bottom)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: Rect = Rect { left: 0, top: 0, right: 3456, bottom: 2426 };

    fn activity(screen_orientation: i32) -> Activity {
        Activity { screen_orientation, min_size: (-1, -1), min_aspect_ratio: 0.0, max_aspect_ratio: 0.0 }
    }

    #[test]
    fn a_task_opens_phone_sized_in_the_middle() {
        let info = activity(-1);
        // 412 x 732 dp at 2x, centered.
        assert_eq!(default_bounds(&info, DISPLAY, 320, false, &[]), Rect { left: 1316, top: 481, right: 2140, bottom: 1945 });
        // A landscape activity: 1064 x 600 dp.
        let landscape = activity(SENSOR_LANDSCAPE);
        let b = default_bounds(&landscape, DISPLAY, 320, false, &[]);
        assert_eq!((b.width(), b.height()), (2128, 1200));
    }

    #[test]
    fn the_next_task_cascades_off_the_first() {
        let info = activity(-1);
        let first = default_bounds(&info, DISPLAY, 320, false, &[]);
        // From the middle, down and right by a sixteenth of the display.
        let second = default_bounds(&info, DISPLAY, 320, false, &[first]);
        assert_eq!(second, Rect { left: 1316 + 216, top: 481 + 151, right: 2140 + 216, bottom: 1945 + 151 });
    }

    #[test]
    fn a_small_display_shrinks_the_window_into_it() {
        let info = activity(PORTRAIT);
        let small = Rect { left: 0, top: 0, right: 1280, bottom: 800 };
        let b = default_bounds(&info, small, 320, false, &[]);
        // The portrait default fits the display's letterbox: 800 x 500,
        // then into the stable area 54 px in.
        assert!(b.height() <= 800 - 2 * 54 && b.width() < b.height());
        assert!(small.contains(&b));
    }
}
