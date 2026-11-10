//! Where a task opens in window mode: the bounds freeform windowing gives
//! a task started without bounds of its own, or with launch bounds, as
//! `TaskLaunchParamsModifier.getTaskBounds` lays it out at
//! `android-16.0.0_r1` (`LaunchParamsUtil.getDefaultFreeformSize`,
//! `adjustBoundsToFitInDisplayArea`, `centerBounds`, then
//! `adjustBoundsToAvoidConflict`), and `cascadeBounds` from a freeform
//! source. The display has no bars, so its stable area is all of it.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::config::Rect;
use crate::package::info::ActivityInfo;

/// `ActivityInfo.SCREEN_ORIENTATION_*`.
const LANDSCAPE: i32 = 0;
const PORTRAIT: i32 = 1;
const NOSENSOR: i32 = 5;
const SENSOR_LANDSCAPE: i32 = 6;
const SENSOR_PORTRAIT: i32 = 7;
const REVERSE_LANDSCAPE: i32 = 8;
const REVERSE_PORTRAIT: i32 = 9;
const USER_LANDSCAPE: i32 = 11;
const USER_PORTRAIT: i32 = 12;
const LOCKED: i32 = 14;
/// `Gravity`'s axes and their ends.
const HORIZONTAL_GRAVITY_MASK: i32 = 0x07;
const VERTICAL_GRAVITY_MASK: i32 = 0x70;
const LEFT: i32 = 0x03;
const RIGHT: i32 = 0x05;
const TOP: i32 = 0x30;
const BOTTOM: i32 = 0x50;
/// `LaunchParamsUtil`'s default sizes and display edge offset, in dp.
const PORTRAIT_SIZE_DP: (i32, i32) = (412, 732);
const LANDSCAPE_SIZE_DP: (i32, i32) = (1064, 600);
const DISPLAY_EDGE_OFFSET_DP: i32 = 27;
/// `TaskLaunchParamsModifier`'s.
const EPSILON: i32 = 2;
const CASCADING_OFFSET_DP: i32 = 75;
const BOUNDS_CONFLICT_THRESHOLD: i32 = 4;
const STEP_DENOMINATOR: i32 = 16;
const MINIMAL_STEP: i32 = 1;
/// `WindowState.MINIMUM_VISIBLE_WIDTH_IN_DP`, `MINIMUM_VISIBLE_HEIGHT_IN_DP`.
const MINIMUM_VISIBLE_DP: (i32, i32) = (48, 32);

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub(super) fn offset(&mut self, dx: i32, dy: i32) {
        *self = Rect {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        };
    }

    fn contains(&self, r: &Rect) -> bool {
        self.left < self.right
            && self.top < self.bottom
            && self.left <= r.left
            && self.top <= r.top
            && self.right >= r.right
            && self.bottom >= r.bottom
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
    /// Its window layout, if it has one.
    pub layout: Option<Layout>,
}

/// An `ActivityInfo.WindowLayout`'s size and gravity: lengths of -1 and
/// fractions below 0 are unspecified.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: i32,
    pub width_fraction: f32,
    pub height: i32,
    pub height_fraction: f32,
    pub gravity: i32,
}

impl From<&ActivityInfo> for Activity {
    fn from(info: &ActivityInfo) -> Activity {
        Activity {
            screen_orientation: info.screen_orientation,
            min_size: info
                .window_layout
                .as_ref()
                .map_or((-1, -1), |l| (l.min_width, l.min_height)),
            min_aspect_ratio: info.min_aspect_ratio,
            max_aspect_ratio: info.max_aspect_ratio,
            layout: info.window_layout.as_ref().map(|l| Layout {
                width: l.width,
                width_fraction: l.width_fraction,
                height: l.height,
                height_fraction: l.height_fraction,
                gravity: l.gravity,
            }),
        }
    }
}

/// The bounds of a new task of `info` on a display of `display` bounds
/// and `density_dpi`, laid out right to left if `rtl`, away from the
/// freeform tasks of `others` bounds.
pub fn default_bounds(
    info: &Activity,
    display: Rect,
    density_dpi: i32,
    rtl: bool,
    others: &[Rect],
) -> Rect {
    task_bounds(info, display, density_dpi, rtl, false, None, others)
}

/// `TaskLaunchParamsModifier.getTaskBounds` of a freeform or fullscreen
/// task of `info` on a display of `display` bounds and `density_dpi`, laid
/// out right to left if `rtl`, from the suggested `bounds`: those kept if
/// they are `initial` ones (launch bounds) or of the default size,
/// transposed about their center to the activity's orientation, else the
/// default size in the display's middle; then away from the freeform
/// tasks of `others` bounds.
pub fn task_bounds(
    info: &Activity,
    display: Rect,
    density_dpi: i32,
    rtl: bool,
    initial: bool,
    bounds: Option<Rect>,
    others: &[Rect],
) -> Rect {
    let bounds = bounds.filter(|b| b.width() > 0 && b.height() > 0);
    let orientation = orientation(info, display, bounds);
    let density = density_dpi as f32 / 160.0;
    let (width, height) = default_size(info, orientation, density, info.min_size, display);
    let mut bounds = match bounds {
        Some(b)
            if initial
                || ((b.width() - width).abs() < EPSILON
                    && (b.height() - height).abs() < EPSILON) =>
        {
            if orientation == orientation_of(&b) {
                b
            } else {
                centered(&b, b.height(), b.width())
            }
        }
        _ => {
            let fitted = fit_in_display(
                display,
                density,
                rtl,
                info.min_size,
                Rect {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                },
            );
            centered(&display, fitted.width(), fitted.height())
        }
    };
    avoid_conflict(display, others, &mut bounds);
    bounds
}

/// `TaskLaunchParamsModifier.getLayoutBounds`: the bounds `info`'s window
/// layout asks for on a display of `display` bounds and `density_dpi`,
/// laid out right to left if `rtl`: of its size, the display's in a
/// dimension it leaves unspecified (`calculateLayoutBounds`), else of the
/// `current` bounds or the default ones, placed by its gravity
/// (`applyLayoutGravity`, centred on an axis of none). None for a layout
/// of neither size nor gravity.
pub fn layout_bounds(
    info: &Activity,
    display: Rect,
    density_dpi: i32,
    rtl: bool,
    current: Option<Rect>,
) -> Option<Rect> {
    let layout = info.layout?;
    let horizontal = layout.gravity & HORIZONTAL_GRAVITY_MASK;
    let vertical = layout.gravity & VERTICAL_GRAVITY_MASK;
    let sized = layout.width >= 0
        || layout.height >= 0
        || layout.width_fraction >= 0.0
        || layout.height_fraction >= 0.0;
    if !sized && horizontal == 0 && vertical == 0 {
        return None;
    }
    let (width, height) = if sized {
        let length = |length: i32, fraction: f32, full: i32| {
            if length > 0 && length < full {
                length
            } else if fraction > 0.0 && fraction < 1.0 {
                (full as f32 * fraction) as i32
            } else {
                full
            }
        };
        (
            length(layout.width, layout.width_fraction, display.width()),
            length(layout.height, layout.height_fraction, display.height()),
        )
    } else {
        let b = current
            .filter(|b| b.width() > 0 && b.height() > 0)
            .unwrap_or_else(|| default_bounds(info, display, density_dpi, rtl, &[]));
        (b.width(), b.height())
    };
    let fraction = |gravity: i32, start: i32, end: i32| match gravity {
        g if g == start => 0.0,
        g if g == end => 1.0,
        _ => 0.5,
    };
    let left = display.left
        + (fraction(horizontal, LEFT, RIGHT) * (display.width() - width) as f32) as i32;
    let top =
        display.top + (fraction(vertical, TOP, BOTTOM) * (display.height() - height) as f32) as i32;
    Some(Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    })
}

/// `TaskLaunchParamsModifier.cascadeBounds`: a freeform `source` task's
/// bounds moved 75 dp right and down, as far as the display's edge allows.
pub fn cascade(source: Rect, display: Rect, density_dpi: i32) -> Rect {
    let offset = (CASCADING_OFFSET_DP as f32 * density_dpi as f32 / 160.0 + 0.5) as i32;
    let mut bounds = source;
    bounds.offset(
        offset.min((display.right - source.right).max(0)),
        offset.min((display.bottom - source.bottom).max(0)),
    );
    bounds
}

/// `TaskLaunchParamsModifier.resolveOrientation` of `info` in `bounds`, if
/// any, on `display`: an orientation locked or left unspecified follows
/// the bounds, else the display's (locked) or portrait.
fn orientation(info: &Activity, display: Rect, bounds: Option<Rect>) -> i32 {
    match info.screen_orientation {
        SENSOR_LANDSCAPE | REVERSE_LANDSCAPE | USER_LANDSCAPE | LANDSCAPE => LANDSCAPE,
        SENSOR_PORTRAIT | REVERSE_PORTRAIT | USER_PORTRAIT | PORTRAIT => PORTRAIT,
        NOSENSOR | LOCKED => orientation_of(&bounds.unwrap_or(display)),
        _ => bounds.map_or(PORTRAIT, |b| orientation_of(&b)),
    }
}

/// `orientationFromBounds`.
fn orientation_of(bounds: &Rect) -> i32 {
    if bounds.width() > bounds.height() {
        LANDSCAPE
    } else {
        PORTRAIT
    }
}

/// `LaunchParamsUtil.centerBounds`: `width` x `height` about the center
/// of `around`.
fn centered(around: &Rect, width: i32, height: i32) -> Rect {
    let left = (around.left + around.right) / 2 - width / 2;
    let top = (around.top + around.bottom) / 2 - height / 2;
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// `TaskLaunchParamsModifier`'s fully resolved current params at `PHASE_BOUNDS`:
/// keep their size and orientation, but avoid existing tasks. Earlier phases
/// do not adjust bounds. This owner has one display area, so it has not changed.
pub fn restored_bounds(display: Rect, others: &[Rect], mut bounds: Rect) -> Rect {
    avoid_conflict(display, others, &mut bounds);
    bounds
}

/// `Task.computeFreeformBounds`: a freeform task's `bounds` moved to keep
/// a strip of them in its `parent`'s part of the stable area (`display`,
/// as it has no bars) (`fitWithinBounds`), their top inside it. `None` if
/// the moved bounds overflow.
pub fn within_parent(bounds: Rect, parent: Rect, display: Rect, density_dpi: i32) -> Option<Rect> {
    let density = density_dpi as f32 / 160.0;
    // `Rect.intersect`: unchanged if they do not intersect.
    let parent = if parent.left < display.right
        && display.left < parent.right
        && parent.top < display.bottom
        && display.top < parent.bottom
    {
        Rect {
            left: parent.left.max(display.left),
            top: parent.top.max(display.top),
            right: parent.right.min(display.right),
            bottom: parent.bottom.min(display.bottom),
        }
    } else {
        parent
    };
    let [left, top, right, bottom] =
        [bounds.left, bounds.top, bounds.right, bounds.bottom].map(i64::from);
    let [p_left, p_top, p_right, p_bottom] =
        [parent.left, parent.top, parent.right, parent.bottom].map(i64::from);
    let (mut dx, mut dy) = (0, 0);
    if parent.width() > 0 && parent.height() > 0 && !parent.contains(&bounds) {
        // Both at most the width of the bounds, as the original has it.
        let width = right - left;
        let x = ((density * MINIMUM_VISIBLE_DP.0 as f32) as i64).min(width);
        let y = ((density * MINIMUM_VISIBLE_DP.1 as f32) as i64).min(width);
        if right < p_left + x {
            dx = x - (right - p_left);
        } else if left > p_right - x {
            dx = -(x - (p_right - left));
        }
        if bottom < p_top + y {
            dy = y - (bottom - p_top);
        } else if top > p_bottom - y {
            dy = -(y - (p_bottom - top));
        }
    }
    // Its caption stays clear of the stable insets.
    dy += (p_top - (top + dy)).max(0);
    let moved = |v: i64, d: i64| i32::try_from(v + d).ok();
    Some(Rect {
        left: moved(left, dx)?,
        top: moved(top, dy)?,
        right: moved(right, dx)?,
        bottom: moved(bottom, dy)?,
    })
}

/// `LaunchParamsUtil.getDefaultFreeformSize`.
fn default_size(
    info: &Activity,
    orientation: i32,
    density: f32,
    min: (i32, i32),
    stable: Rect,
) -> (i32, i32) {
    let (width_dp, height_dp) = if orientation == LANDSCAPE {
        LANDSCAPE_SIZE_DP
    } else {
        PORTRAIT_SIZE_DP
    };
    let freeform = (
        (width_dp as f32 * density + 0.5) as i32,
        (height_dp as f32 * density + 0.5) as i32,
    );
    let portrait_height = stable.width().min(stable.height());
    let other = stable.width().max(stable.height());
    let portrait_width = portrait_height * portrait_height / other;
    let (max_width, max_height) = if orientation == LANDSCAPE {
        (portrait_height, portrait_width)
    } else {
        (portrait_width, portrait_height)
    };
    let width = max_width.min(freeform.0.max(min.0));
    let height = max_height.min(freeform.1.max(min.1));
    let aspect = width.max(height) as f32 / width.min(height) as f32;
    let (mut adjusted_width, mut adjusted_height) = (width, height);
    let landscape = orientation == LANDSCAPE;
    for (ratio, applies) in [
        (
            info.min_aspect_ratio,
            info.min_aspect_ratio >= 1.0 && aspect < info.min_aspect_ratio,
        ),
        (
            info.max_aspect_ratio,
            info.max_aspect_ratio >= 1.0 && aspect > info.max_aspect_ratio,
        ),
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
fn fit_in_display(
    display: Rect,
    density: f32,
    rtl: bool,
    min: (i32, i32),
    mut bounds: Rect,
) -> Rect {
    let edge = (DISPLAY_EDGE_OFFSET_DP as f32 * density + 0.5) as i32;
    let stable = Rect {
        left: display.left + edge,
        top: display.top + edge,
        right: display.right - edge,
        bottom: display.bottom - edge,
    };
    if stable.width() < bounds.width() || stable.height() < bounds.height() {
        let shrink = (stable.width() as f32 / bounds.width() as f32)
            .min(stable.height() as f32 / bounds.height() as f32);
        let width = min.0.max((bounds.width() as f32 * shrink) as i32);
        let height = min.1.max((bounds.height() as f32 * shrink) as i32);
        if stable.width() < width || stable.height() < height {
            let left = if rtl {
                stable.right - width
            } else {
                stable.left
            };
            return Rect {
                left,
                top: stable.top,
                right: left + width,
                bottom: stable.top + height,
            };
        }
        bounds = Rect {
            left: bounds.left,
            top: bounds.top,
            right: bounds.left + width,
            bottom: bounds.top + height,
        };
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
    let (center_x, center_y) = (
        (bounds.left + bounds.right) / 2,
        (bounds.top + bounds.bottom) / 2,
    );
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
    let step = (
        MINIMAL_STEP.max(display.width() / STEP_DENOMINATOR),
        MINIMAL_STEP.max(display.height() / STEP_DENOMINATOR),
    );
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
        let (right, bottom) = (
            close(t.right, candidate.right),
            close(t.bottom, candidate.bottom),
        );
        (left && top) || (left && bottom) || (right && top) || (right && bottom)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: Rect = Rect {
        left: 0,
        top: 0,
        right: 3456,
        bottom: 2426,
    };

    fn activity(screen_orientation: i32) -> Activity {
        Activity {
            screen_orientation,
            min_size: (-1, -1),
            min_aspect_ratio: 0.0,
            max_aspect_ratio: 0.0,
            layout: None,
        }
    }

    /// `computeFreeformBounds` in an organizer root task: its part of the
    /// display bounds the bounds; a strip narrower than the bounds' width
    /// is that width, in both directions as the original has it.
    #[test]
    fn freeform_bounds_fit_their_parent_on_the_display() {
        let r = |left, top, right, bottom| Rect {
            left,
            top,
            right,
            bottom,
        };
        let parent = r(1000, 200, 4000, 2000);
        assert_eq!(
            within_parent(r(3900, 100, 3940, 1000), parent, DISPLAY, 320),
            Some(r(3416, 200, 3456, 1100))
        );
        assert_eq!(
            within_parent(r(0, 2400, 40, 3000), parent, DISPLAY, 320),
            Some(r(1000, 1960, 1040, 2560))
        );
        assert_eq!(
            within_parent(r(1200, 300, 1300, 400), parent, DISPLAY, 320),
            Some(r(1200, 300, 1300, 400))
        );
        assert_eq!(
            within_parent(r(0, 0, 100, 100), r(0, 0, 100, 100), DISPLAY, 320),
            Some(r(0, 0, 100, 100))
        );
        assert_eq!(
            within_parent(r(0, -(1 << 30), 10, i32::MAX - 10), DISPLAY, DISPLAY, 320),
            None
        );
    }

    #[test]
    fn a_task_opens_phone_sized_in_the_middle() {
        let info = activity(-1);
        // 412 x 732 dp at 2x, centered.
        assert_eq!(
            default_bounds(&info, DISPLAY, 320, false, &[]),
            Rect {
                left: 1316,
                top: 481,
                right: 2140,
                bottom: 1945
            }
        );
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
        assert_eq!(
            second,
            Rect {
                left: 1316 + 216,
                top: 481 + 151,
                right: 2140 + 216,
                bottom: 1945 + 151
            }
        );
    }

    /// `getTaskBounds` of launch bounds: kept in an orientation the
    /// activity takes (one unspecified or locked follows them), else
    /// turned about their center.
    #[test]
    fn launch_bounds_turn_to_the_activitys_orientation() {
        let portrait = Rect {
            left: 1316,
            top: 481,
            right: 2140,
            bottom: 1945,
        };
        let bounds = |orientation| {
            task_bounds(
                &activity(orientation),
                DISPLAY,
                320,
                false,
                true,
                Some(portrait),
                &[],
            )
        };
        for orientation in [-1, PORTRAIT, LOCKED, NOSENSOR] {
            assert_eq!(bounds(orientation), portrait);
        }
        assert_eq!(
            bounds(SENSOR_LANDSCAPE),
            Rect {
                left: 996,
                top: 801,
                right: 2460,
                bottom: 1625
            }
        );
    }

    /// `cascadeBounds` from a freeform source of the default size: 75 dp
    /// down and right of it, kept as of the default size; from one of
    /// another size the default in the middle, off the source.
    #[test]
    fn a_task_cascades_from_its_freeform_source() {
        let info = activity(-1);
        let source = default_bounds(&info, DISPLAY, 320, false, &[]);
        let cascaded = cascade(source, DISPLAY, 320);
        assert_eq!(
            cascaded,
            Rect {
                left: 1316 + 150,
                top: 481 + 150,
                right: 2140 + 150,
                bottom: 1945 + 150
            }
        );
        assert_eq!(
            task_bounds(&info, DISPLAY, 320, false, false, Some(cascaded), &[source]),
            cascaded
        );
        let small = Rect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 400,
        };
        assert_eq!(
            task_bounds(
                &info,
                DISPLAY,
                320,
                false,
                false,
                Some(cascade(small, DISPLAY, 320)),
                &[source]
            ),
            default_bounds(&info, DISPLAY, 320, false, &[source])
        );
        // At the display's edge it moves no further.
        let edge = Rect {
            left: 3000,
            top: 2000,
            right: 3456,
            bottom: 2400,
        };
        assert_eq!(cascade(edge, DISPLAY, 320).right, 3456);
        assert_eq!(cascade(edge, DISPLAY, 320).bottom, 2426);
    }

    /// `getLayoutBounds`: a window layout's size, a fraction of the
    /// display, or the display's, placed by its gravity; a layout of
    /// gravity only places the current or default bounds; one of neither
    /// gives none.
    #[test]
    fn a_window_layout_sizes_and_places_the_task() {
        let layout = |width, width_fraction, height, height_fraction, gravity| Activity {
            layout: Some(Layout {
                width,
                width_fraction,
                height,
                height_fraction,
                gravity,
            }),
            ..activity(-1)
        };
        let r = |left, top, right, bottom| Rect {
            left,
            top,
            right,
            bottom,
        };
        let bounds = |info: &Activity, current| layout_bounds(info, DISPLAY, 320, false, current);
        // 1000 x half the display, top left.
        assert_eq!(
            bounds(&layout(1000, -1.0, -1, 0.5, TOP | LEFT), None),
            Some(r(0, 0, 1000, 1213))
        );
        // Of no height, the display's; centred without gravity.
        assert_eq!(
            bounds(&layout(1000, -1.0, -1, -1.0, 0), None),
            Some(r(1228, 0, 2228, 2426))
        );
        // A width past the display's, then the fraction's.
        assert_eq!(
            bounds(&layout(5000, 0.25, 600, -1.0, BOTTOM | RIGHT), None),
            Some(r(2592, 1826, 3456, 2426))
        );
        let current = r(10, 10, 410, 810);
        assert_eq!(
            bounds(&layout(-1, -1.0, -1, -1.0, RIGHT), Some(current)),
            Some(r(3056, 813, 3456, 1613))
        );
        let default = default_bounds(&activity(-1), DISPLAY, 320, false, &[]);
        assert_eq!(
            bounds(&layout(-1, -1.0, -1, -1.0, TOP), None),
            Some(r(default.left, 0, default.right, default.height()))
        );
        assert_eq!(bounds(&layout(-1, -1.0, -1, -1.0, 0), None), None);
        assert_eq!(bounds(&activity(-1), None), None);
    }

    #[test]
    fn a_small_display_shrinks_the_window_into_it() {
        let info = activity(PORTRAIT);
        let small = Rect {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 800,
        };
        let b = default_bounds(&info, small, 320, false, &[]);
        // The portrait default fits the display's letterbox: 800 x 500,
        // then into the stable area 54 px in.
        assert!(b.height() <= 800 - 2 * 54 && b.width() < b.height());
        assert!(small.contains(&b));
    }
}
