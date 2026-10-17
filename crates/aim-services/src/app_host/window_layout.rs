//! A window's layout as WindowManager computes it at `android-16.0.0_r1`:
//! its `WindowManager.LayoutParams` as an app writes them, its frames as
//! `WindowLayout.computeFrames` places it (with `Gravity.apply` and
//! `applyDisplay`) on a display without system bars or cutout, and the
//! region it takes touches in (`WindowState.getTouchableRegion`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::{Binder, Reader, Result};

use super::config::Rect;

/// `LayoutParams.MATCH_PARENT`; `WindowLayout.UNSPECIFIED_LENGTH`.
const MATCH_PARENT: i32 = -1;
pub const UNSPECIFIED_LENGTH: i32 = -1;
/// `LayoutParams.FIRST_SUB_WINDOW`, `LAST_SUB_WINDOW`; `TYPE_BASE_APPLICATION`.
const FIRST_SUB_WINDOW: i32 = 1000;
const LAST_SUB_WINDOW: i32 = 1999;
const TYPE_BASE_APPLICATION: i32 = 1;
/// `LayoutParams.FLAG_*` the layout and input read.
const FLAG_NOT_FOCUSABLE: i32 = 0x8;
const FLAG_NOT_TOUCHABLE: i32 = 0x10;
const FLAG_NOT_TOUCH_MODAL: i32 = 0x20;
const FLAG_LAYOUT_IN_SCREEN: i32 = 0x100;
const FLAG_LAYOUT_NO_LIMITS: i32 = 0x200;
const FLAG_SCALED: i32 = 0x4000;
const FLAG_WATCH_OUTSIDE_TOUCH: i32 = 0x40000;
/// `Gravity`'s axis bits.
const AXIS_PULL_BEFORE: i32 = 0x2;
const AXIS_PULL_AFTER: i32 = 0x4;
const AXIS_CLIP: i32 = 0x8;
const AXIS_X_SHIFT: i32 = 0;
const AXIS_Y_SHIFT: i32 = 4;
const DISPLAY_CLIP_VERTICAL: i32 = 0x1000_0000;
const DISPLAY_CLIP_HORIZONTAL: i32 = 0x0100_0000;
/// `WindowLayout.MIN_X` and its kin: the bounds of a window without limits.
const MIN: i32 = -100_000;
const MAX: i32 = 100_000;

/// `WindowManager.LayoutParams`, of what the layout reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutParams {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub kind: i32,
    pub flags: i32,
    pub gravity: i32,
    pub horizontal_margin: f32,
    pub vertical_margin: f32,
    /// The activity's token, or a sub-window's parent window.
    pub token: Option<Binder>,
    /// How much larger than its frame its surface is on each side (left,
    /// top, right, bottom): room for its shadows.
    pub surface_insets: [i32; 4],
}

impl LayoutParams {
    /// `LayoutParams(Parcel)`, up to its surface insets; the rest is not
    /// read.
    pub fn read(r: &mut Reader<'_>) -> Result<LayoutParams> {
        let width = r.read_i32()?;
        let height = r.read_i32()?;
        let x = r.read_i32()?;
        let y = r.read_i32()?;
        let kind = r.read_i32()?;
        let flags = r.read_i32()?;
        r.read_i32()?; // private flags
        r.read_i32()?; // soft input mode
        r.read_i32()?; // display cutout mode
        let gravity = r.read_i32()?;
        let horizontal_margin = r.read_f32()?;
        let vertical_margin = r.read_f32()?;
        // format, window animations, alpha, dim amount, screen and button
        // brightness, rotation animation
        for _ in 0..7 {
            r.read_i32()?;
        }
        let token = r.read_binder()?;
        r.read_binder()?; // window context token
        r.read_string16()?; // package name
        crate::clip::char_sequence(r)?; // title
        r.read_i32()?; // screen orientation
        r.read_f32()?; // preferred refresh rate
        r.read_i32()?; // preferred display mode id
        r.read_f32()?; // preferred min refresh rate
        r.read_f32()?; // preferred max refresh rate
        r.read_i32()?; // system UI visibility
        r.read_i32()?; // subtree system UI visibility
        r.read_bool()?; // has system UI listeners
        r.read_i32()?; // input features
        r.read_i64()?; // user activity timeout
        let surface_insets = [r.read_i32()?, r.read_i32()?, r.read_i32()?, r.read_i32()?];
        Ok(LayoutParams {
            width,
            height,
            x,
            y,
            kind,
            flags,
            gravity,
            horizontal_margin,
            vertical_margin,
            token,
            surface_insets,
        })
    }

    /// A sub-window: one attached to its parent window (a panel, a
    /// popup).
    pub fn is_sub_window(&self) -> bool {
        (FIRST_SUB_WINDOW..=LAST_SUB_WINDOW).contains(&self.kind)
    }

    /// `WindowState.isModal`: it takes the touches outside it.
    fn is_modal(&self) -> bool {
        self.flags & (FLAG_NOT_TOUCH_MODAL | FLAG_NOT_FOCUSABLE) == 0
    }

    /// It is told of touches outside it (`ACTION_OUTSIDE`).
    pub fn watches_outside(&self) -> bool {
        self.flags & FLAG_WATCH_OUTSIDE_TOUCH != 0
    }
}

/// `ClientWindowFrames`' rectangles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frames {
    pub frame: Rect,
    pub display: Rect,
    pub parent: Rect,
    pub attached: Option<Rect>,
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}

/// `Gravity.apply(gravity, w, h, container, xAdj, yAdj, outRect)`.
fn apply(gravity: i32, w: i32, h: i32, container: Rect, x_adj: i32, y_adj: i32) -> Rect {
    // One axis: its start and end in the container's, of size `size`.
    let axis = |shift: i32, start: i32, end: i32, size: i32, adj: i32| {
        let clip = gravity & (AXIS_CLIP << shift) == AXIS_CLIP << shift;
        match gravity & ((AXIS_PULL_BEFORE | AXIS_PULL_AFTER) << shift) {
            0 => {
                let mut a = start + (end - start - size) / 2 + adj;
                let mut b = a + size;
                if clip {
                    a = a.max(start);
                    b = b.min(end);
                }
                (a, b)
            }
            g if g == AXIS_PULL_BEFORE << shift => {
                let a = start + adj;
                let b = a + size;
                (a, if clip { b.min(end) } else { b })
            }
            g if g == AXIS_PULL_AFTER << shift => {
                let b = end - adj;
                let a = b - size;
                (if clip { a.max(start) } else { a }, b)
            }
            _ => (start + adj, end + adj),
        }
    };
    let (left, right) = axis(AXIS_X_SHIFT, container.left, container.right, w, x_adj);
    let (top, bottom) = axis(AXIS_Y_SHIFT, container.top, container.bottom, h, y_adj);
    rect(left, top, right, bottom)
}

/// `Gravity.applyDisplay`: `frame` moved (or clipped) into `display`.
fn apply_display(gravity: i32, display: Rect, mut frame: Rect) -> Rect {
    // One axis: its start and end within the display's.
    let axis = |clip: bool, (start, end): (i32, i32), (lo, hi): (i32, i32)| {
        if clip {
            return (start.max(lo), end.min(hi));
        }
        let off = if start < lo {
            lo - start
        } else if end > hi {
            hi - end
        } else {
            0
        };
        if off == 0 {
            (start, end)
        } else if end - start > hi - lo {
            (lo, hi)
        } else {
            (start + off, end + off)
        }
    };
    (frame.top, frame.bottom) = axis(
        gravity & DISPLAY_CLIP_VERTICAL != 0,
        (frame.top, frame.bottom),
        (display.top, display.bottom),
    );
    (frame.left, frame.right) = axis(
        gravity & DISPLAY_CLIP_HORIZONTAL != 0,
        (frame.left, frame.right),
        (display.left, display.right),
    );
    frame
}

/// `WindowLayout.computeFrames` of a window of `attrs` in `bounds` (its
/// task's, or the display's), attached to the frame `attached` (a
/// sub-window's parent's), in a multi-window (freeform) task or not, of
/// the size its view asked for (`UNSPECIFIED_LENGTH` before it was
/// measured). No insets or cutout limit it, and nothing is scaled.
pub fn compute_frames(
    attrs: &LayoutParams,
    bounds: Rect,
    attached: Option<Rect>,
    multi_window: bool,
    (requested_width, requested_height): (i32, i32),
) -> Frames {
    let layout_in_screen = attrs.flags & FLAG_LAYOUT_IN_SCREEN != 0;
    let mut display = bounds;
    let parent = match attached {
        Some(frame) if !layout_in_screen => frame,
        _ => display,
    };
    let no_limits = attrs.flags & FLAG_LAYOUT_NO_LIMITS != 0;
    if no_limits && !multi_window {
        display = rect(MIN, MIN, MAX, MAX);
    }
    let (pw, ph) = (parent.right - parent.left, parent.bottom - parent.top);
    let rw = if requested_width == UNSPECIFIED_LENGTH {
        if attrs.width >= 0 { attrs.width } else { pw }
    } else {
        requested_width
    };
    let rh = if requested_height == UNSPECIFIED_LENGTH {
        if attrs.height >= 0 { attrs.height } else { ph }
    } else {
        requested_height
    };
    let (mut w, mut h) = if attrs.flags & FLAG_SCALED != 0 {
        (
            if attrs.width < 0 { pw } else { attrs.width },
            if attrs.height < 0 { ph } else { attrs.height },
        )
    } else {
        (
            if attrs.width == MATCH_PARENT { pw } else { rw },
            if attrs.height == MATCH_PARENT { ph } else { rh },
        )
    };
    if multi_window {
        // PRIVATE_FLAG_LAYOUT_CHILD_WINDOW_IN_PARENT_FRAME is the
        // system's (Toast's); apps' windows fit their task.
        w = w.min(pw);
        h = h.min(ph);
    }
    let fit_to_display = !multi_window || (attrs.kind != TYPE_BASE_APPLICATION && !no_limits);
    let mut frame = apply(
        attrs.gravity,
        w,
        h,
        parent,
        (attrs.x as f32 + attrs.horizontal_margin * pw as f32) as i32,
        (attrs.y as f32 + attrs.vertical_margin * ph as f32) as i32,
    );
    if fit_to_display {
        frame = apply_display(attrs.gravity, display, frame);
    }
    Frames {
        frame,
        display,
        parent,
        attached,
    }
}

/// The region a window takes touches in (`WindowState.getTouchableRegion`
/// and `populateInputWindowHandle`): none if it is not touchable; its
/// activity's task bounds (`updateRegionForModalActivityWindow`) if it is
/// modal; else its frame.
pub fn touchable_region(attrs: &LayoutParams, frame: Rect, task_bounds: Rect) -> Rect {
    if attrs.flags & FLAG_NOT_TOUCHABLE != 0 {
        return rect(0, 0, 0, 0);
    }
    if attrs.is_modal() {
        return task_bounds;
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: Rect = Rect {
        left: 0,
        top: 0,
        right: 1080,
        bottom: 1920,
    };
    /// `Gravity.START | Gravity.TOP`, `Gravity.CENTER`.
    const START_TOP: i32 = 0x0080_0003 | 0x30;
    const CENTER: i32 = 0x11;

    fn popup(x: i32, y: i32) -> LayoutParams {
        LayoutParams {
            width: -2,
            height: -2,
            x,
            y,
            kind: 1000,
            flags: 0x0002_0000 | 0x200 | 0x40000,
            gravity: START_TOP,
            ..Default::default()
        }
    }

    /// A popup at its anchor's position, of its measured size; a dialog
    /// centered; an activity's window the whole bounds.
    #[test]
    fn frames_follow_gravity_and_position() {
        let f = compute_frames(&popup(600, 40), DISPLAY, Some(DISPLAY), false, (400, 500));
        assert_eq!(f.frame, rect(600, 40, 1000, 540));
        let dialog = LayoutParams {
            width: -2,
            height: -2,
            kind: 2,
            gravity: CENTER,
            ..Default::default()
        };
        let f = compute_frames(&dialog, DISPLAY, None, false, (800, 400));
        assert_eq!(f.frame, rect(140, 760, 940, 1160));
        let activity = LayoutParams {
            width: -1,
            height: -1,
            kind: 1,
            ..Default::default()
        };
        let f = compute_frames(
            &activity,
            DISPLAY,
            None,
            false,
            (UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH),
        );
        assert_eq!(f.frame, DISPLAY);
    }

    /// A window past the display's edge is moved back in, unless it has no
    /// limits.
    #[test]
    fn windows_fit_the_display() {
        let mut p = popup(900, 40);
        p.flags &= !0x200;
        let f = compute_frames(&p, DISPLAY, Some(DISPLAY), false, (400, 500));
        assert_eq!(f.frame, rect(680, 40, 1080, 540));
        let f = compute_frames(&popup(900, 40), DISPLAY, Some(DISPLAY), false, (400, 500));
        assert_eq!(f.frame, rect(900, 40, 1300, 540));
    }

    /// In a freeform task a window fits its task.
    #[test]
    fn multi_window_windows_fit_their_task() {
        let task = rect(100, 100, 600, 900);
        let mut p = popup(0, 0);
        p.flags &= !0x200;
        let f = compute_frames(&p, task, Some(task), true, (800, 300));
        assert_eq!(f.frame, rect(100, 100, 600, 400));
    }

    #[test]
    fn modal_windows_take_their_tasks_touches() {
        let modal = LayoutParams {
            flags: 0,
            ..Default::default()
        };
        let frame = rect(10, 10, 20, 20);
        assert_eq!(touchable_region(&modal, frame, DISPLAY), DISPLAY);
        assert_eq!(touchable_region(&popup(0, 0), frame, DISPLAY), DISPLAY);
        let not_modal = LayoutParams {
            flags: FLAG_NOT_FOCUSABLE,
            ..Default::default()
        };
        assert_eq!(touchable_region(&not_modal, frame, DISPLAY), frame);
        let untouchable = LayoutParams {
            flags: FLAG_NOT_TOUCHABLE,
            ..Default::default()
        };
        assert_eq!(
            touchable_region(&untouchable, frame, DISPLAY),
            rect(0, 0, 0, 0)
        );
    }
}
