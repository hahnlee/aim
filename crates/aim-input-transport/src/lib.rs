//! Android's input transport as an input dispatcher fills it at
//! `android-16.0.0_r1`: the `InputMessage`s of `InputTransport.h`
//! (`InputPublisher`), and a direct touchscreen's multitouch protocol B
//! cooked into motions in the display's pixels, as InputReader's
//! `MultiTouchInputMapper` and `TouchInputMapper::dispatchTouches` cook it.
//! The window a touch goes to is chosen as `InputDispatcher` chooses it
//! (`findTouchedWindowTargets`).

pub mod keyboard;
pub mod keychars;
pub mod keylayout;

/// `InputMessage::Type`.
const MOTION: u32 = 1;
const FOCUS: u32 = 3;
const TOUCH_MODE: u32 = 7;
/// `AINPUT_SOURCE_TOUCHSCREEN`, `ToolType::FINGER`.
const SOURCE_TOUCHSCREEN: i32 = 0x1002;
const TOOL_FINGER: i32 = 1;
/// `AMOTION_EVENT_ACTION_*`, `AMOTION_EVENT_ACTION_POINTER_INDEX_SHIFT`.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVE: i32 = 2;
const ACTION_CANCEL: i32 = 3;
const ACTION_OUTSIDE: i32 = 4;
const ACTION_POINTER_DOWN: i32 = 5;
const ACTION_POINTER_UP: i32 = 6;
const POINTER_INDEX_SHIFT: i32 = 8;
/// `AMOTION_EVENT_AXIS_X`, `_Y`, `_PRESSURE`, `_SIZE`.
const AXIS_X: u32 = 0;
const AXIS_Y: u32 = 1;
const AXIS_PRESSURE: u32 = 2;
const AXIS_SIZE: u32 = 3;
/// `MAX_POINTERS`, `PointerCoords::MAX_AXES`, `MAX_POINTER_ID`.
const MAX_POINTERS: usize = 16;
const MAX_AXES: usize = 30;
const MAX_POINTER_ID: u32 = 31;
/// The first input device InputReader numbers (`END_RESERVED_ID`).
const DEVICE_ID: i32 = 1;
/// The device of an injected event that names none
/// (`InputShellCommand.DEFAULT_DEVICE_ID`).
const INJECTED_DEVICE_ID: i32 = 0;

/// `<linux/input-event-codes.h>`: the multitouch axes.
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;

/// `InputMessage` of type `FOCUS`: the window has the focus, or lost it.
pub fn focus(seq: u32, id: i32, has_focus: bool) -> Vec<u8> {
    let mut m = header(FOCUS, seq);
    m.extend_from_slice(&id.to_ne_bytes());
    m.extend_from_slice(&[has_focus as u8, 0, 0, 0]);
    m
}

/// `InputMessage` of type `TOUCH_MODE`: the display's touch mode changed
/// (`publishTouchModeEvent`).
pub fn touch_mode(seq: u32, id: i32, in_touch_mode: bool) -> Vec<u8> {
    let mut m = header(TOUCH_MODE, seq);
    m.extend_from_slice(&id.to_ne_bytes());
    m.extend_from_slice(&[in_touch_mode as u8, 0, 0, 0]);
    m
}

/// What WindowManager tells of a window's input (`InputWindowInfo`): its
/// frame and the region it takes touches in, on the display (left, top,
/// right, bottom, in pixels), whether it watches touches outside it, and
/// the uid and pid of its owner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub frame: [i32; 4],
    pub touchable: [i32; 4],
    pub watch_outside: bool,
    pub owner: i32,
    pub owner_pid: i32,
}

impl Region {
    fn takes(&self, (x, y): (f32, f32)) -> bool {
        let [left, top, right, bottom] = self.touchable.map(|v| v as f32);
        (left..right).contains(&x) && (top..bottom).contains(&y)
    }
}

/// `findTouchedWindowTargets` at a gesture's `start` among windows back
/// to front: the front one that takes it, and those in front of it that
/// watch outside touches.
pub fn touched(regions: &[Region], start: (f32, f32)) -> (Option<usize>, Vec<usize>) {
    let at = regions.iter().rposition(|r| r.takes(start));
    let above = at.map_or(0, |at| at + 1);
    let outside = (above..regions.len())
        .filter(|&i| regions[i].watch_outside)
        .collect();
    (at, outside)
}

/// A multitouch slot.
#[derive(Clone, Copy, Default)]
struct Slot {
    tracking: Option<i32>,
    x: i32,
    y: i32,
}

/// The touchscreen's slots and the pointers of the last report.
#[derive(Default)]
pub struct Touch {
    slots: Vec<Slot>,
    slot: usize,
    /// The last report's pointers: (pointer id, tracking id, x, y), by id.
    pointers: Vec<(u32, i32, f32, f32)>,
    down_time: i64,
}

/// A motion to publish.
pub struct Motion {
    action: i32,
    time: i64,
    down_time: i64,
    pointers: Vec<(u32, f32, f32)>,
    device: i32,
    /// Each pointer's pressure and size.
    pressure: f32,
    size: f32,
}

impl Touch {
    /// An `EV_ABS` event.
    pub fn abs(&mut self, code: u16, value: i32) {
        if code == ABS_MT_SLOT {
            self.slot = value.max(0) as usize;
            return;
        }
        if self.slots.len() <= self.slot {
            self.slots.resize(self.slot + 1, Slot::default());
        }
        let slot = &mut self.slots[self.slot];
        match code {
            ABS_MT_TRACKING_ID => slot.tracking = (value >= 0).then_some(value),
            ABS_MT_POSITION_X => slot.x = value,
            ABS_MT_POSITION_Y => slot.y = value,
            _ => {}
        }
    }

    /// The motions a report (`SYN_REPORT`) at `time` makes, the axes'
    /// minimums at the display's origin, scaled by `scale` to its pixels
    /// (`TouchInputMapper::dispatchTouches`):
    /// pointers that went up, one at a time; a move if any stayed and
    /// moved; pointers that went down, one at a time.
    pub fn report(&mut self, time: i64, x_min: i32, y_min: i32, scale: (f32, f32)) -> Vec<Motion> {
        let mut current: Vec<(u32, i32, f32, f32)> = Vec::new();
        for slot in &self.slots {
            let Some(tracking) = slot.tracking else {
                continue;
            };
            let x = (slot.x - x_min) as f32 * scale.0;
            let y = (slot.y - y_min) as f32 * scale.1;
            // A pointer keeps its id while its tracking id lasts; a new one
            // takes the lowest free id.
            let id = self
                .pointers
                .iter()
                .find(|p| p.1 == tracking)
                .map(|p| p.0)
                .unwrap_or_else(|| {
                    (0..=MAX_POINTER_ID)
                        .find(|id| !self.pointers.iter().chain(&current).any(|p| p.0 == *id))
                        .unwrap_or(MAX_POINTER_ID)
                });
            if current.len() < MAX_POINTERS {
                current.push((id, tracking, x, y));
            }
        }
        current.sort_by_key(|p| p.0);
        let mut motions = Vec::new();
        let mut state = self.pointers.clone();
        let coords =
            |s: &[(u32, i32, f32, f32)]| s.iter().map(|p| (p.0, p.2, p.3)).collect::<Vec<_>>();
        // Ups, with the last positions of the pointers that stay.
        for up in self
            .pointers
            .iter()
            .filter(|p| !current.iter().any(|c| c.0 == p.0))
        {
            for p in state.iter_mut() {
                if let Some(c) = current.iter().find(|c| c.0 == p.0) {
                    *p = *c;
                }
            }
            let index = state.iter().position(|p| p.0 == up.0).unwrap() as i32;
            let action = if state.len() == 1 {
                ACTION_UP
            } else {
                ACTION_POINTER_UP | (index << POINTER_INDEX_SHIFT)
            };
            motions.push(Motion::touched(
                action,
                time,
                self.down_time,
                coords(&state),
            ));
            state.retain(|p| p.0 != up.0);
        }
        // A move of the pointers that stayed.
        let moved = state.iter().any(|p| {
            current
                .iter()
                .any(|c| c.0 == p.0 && (c.2, c.3) != (p.2, p.3))
        });
        for p in state.iter_mut() {
            if let Some(c) = current.iter().find(|c| c.0 == p.0) {
                *p = *c;
            }
        }
        if moved {
            motions.push(Motion::touched(
                ACTION_MOVE,
                time,
                self.down_time,
                coords(&state),
            ));
        }
        // Downs.
        for down in current
            .iter()
            .filter(|c| !self.pointers.iter().any(|p| p.0 == c.0))
        {
            state.push(*down);
            state.sort_by_key(|p| p.0);
            let action = if state.len() == 1 {
                self.down_time = time;
                ACTION_DOWN
            } else {
                let index = state.iter().position(|p| p.0 == down.0).unwrap() as i32;
                ACTION_POINTER_DOWN | (index << POINTER_INDEX_SHIFT)
            };
            motions.push(Motion::touched(
                action,
                time,
                self.down_time,
                coords(&state),
            ));
        }
        self.pointers = current;
        motions
    }
}

impl Motion {
    /// A motion of the touchscreen, its pointers pressed.
    fn touched(action: i32, time: i64, down_time: i64, pointers: Vec<(u32, f32, f32)>) -> Motion {
        Motion {
            action,
            time,
            down_time,
            pointers,
            device: DEVICE_ID,
            pressure: 1.0,
            size: 0.0,
        }
    }

    /// A one-finger touchscreen motion injected as `InputShellCommand`
    /// makes it (`injectMotionEvent`): `action` (down, move or up) at
    /// `(x, y)` in the display's pixels, with `pressure` and the default
    /// size, of no device; `None` for another action.
    pub fn injected(
        action: i32,
        down_time: i64,
        time: i64,
        x: f32,
        y: f32,
        pressure: f32,
    ) -> Option<Motion> {
        matches!(action, ACTION_DOWN | ACTION_MOVE | ACTION_UP).then(|| Motion {
            action,
            time,
            down_time,
            pointers: vec![(0, x, y)],
            device: INJECTED_DEVICE_ID,
            pressure,
            size: 1.0,
        })
    }

    /// Where a gesture starts (the first pointer's down), in the display's
    /// pixels.
    pub fn gesture_start(&self) -> Option<(f32, f32)> {
        (self.action == ACTION_DOWN).then(|| (self.pointers[0].1, self.pointers[0].2))
    }

    /// The `ACTION_OUTSIDE` a window watching outside touches gets of a
    /// gesture's start: its first pointer, where it is only if `visible`
    /// (the window is of the touched window's app), zeroed otherwise.
    pub fn outside(&self, visible: bool) -> Motion {
        let (id, x, y) = self.pointers[0];
        let (x, y) = if visible { (x, y) } else { (0.0, 0.0) };
        Motion {
            action: ACTION_OUTSIDE,
            pointers: vec![(id, x, y)],
            ..*self
        }
    }

    /// Whether the window it goes to is interacted with: anything but a
    /// pointer's up or a cancel (`processInteractionsLocked`).
    pub fn interacts(&self) -> bool {
        !matches!(
            self.action & 0xff,
            ACTION_UP | ACTION_CANCEL | ACTION_POINTER_UP
        )
    }

    /// Whether the gesture ends with it (the last pointer's up).
    pub fn ends_gesture(&self) -> bool {
        self.action == ACTION_UP
    }

    /// `InputMessage` of type `MOTION` as `InputPublisher::publishMotionEvent`
    /// fills it for a window at `origin` on the display, unscaled: the
    /// window's transform takes the display's pixels to the window's.
    pub fn message(&self, seq: u32, id: i32, origin: (f32, f32)) -> Vec<u8> {
        let mut m = header(MOTION, seq);
        let i32s = |m: &mut Vec<u8>, v: &[i32]| {
            v.iter().for_each(|v| m.extend_from_slice(&v.to_ne_bytes()))
        };
        let f32s = |m: &mut Vec<u8>, v: &[f32]| {
            v.iter().for_each(|v| m.extend_from_slice(&v.to_ne_bytes()))
        };
        i32s(&mut m, &[id, self.pointers.len() as i32]);
        m.extend_from_slice(&self.time.to_ne_bytes());
        i32s(&mut m, &[self.device, SOURCE_TOUCHSCREEN, 0]); // display 0
        m.extend_from_slice(&[0; 32]); // no HMAC
        i32s(&mut m, &[self.action, 0, 0, 0, 0]); // action, button, flags, meta, buttons
        m.extend_from_slice(&[0; 4]); // classification NONE
        i32s(&mut m, &[0]); // no edge flags
        m.extend_from_slice(&self.down_time.to_ne_bytes());
        f32s(&mut m, &[1.0, 0.0, 0.0, 1.0, -origin.0, -origin.1]); // the window's transform
        f32s(&mut m, &[1.0, 1.0, f32::NAN, f32::NAN]); // precision; no cursor
        f32s(&mut m, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]); // the raw transform
        for &(pointer, x, y) in &self.pointers {
            i32s(&mut m, &[pointer as i32, TOOL_FINGER]);
            // `PointerCoords`: the bits of its axes that are not zero
            // (`setAxisValue` clears an axis set to zero), highest first,
            // their values in axis order, not resampled.
            let axes = [
                (AXIS_X, x),
                (AXIS_Y, y),
                (AXIS_PRESSURE, self.pressure),
                (AXIS_SIZE, self.size),
            ];
            let set: Vec<_> = axes.iter().filter(|(_, v)| *v != 0.0).collect();
            let bits = set.iter().fold(0u64, |b, (a, _)| b | (1 << 63) >> a);
            m.extend_from_slice(&bits.to_ne_bytes());
            let mut values = [0f32; MAX_AXES];
            for (i, (_, v)) in set.iter().enumerate() {
                values[i] = *v;
            }
            f32s(&mut m, &values);
            m.extend_from_slice(&[0; 8]);
        }
        m
    }
}

/// `InputMessage::Header`.
fn header(kind: u32, seq: u32) -> Vec<u8> {
    let mut m = Vec::with_capacity(8 + 160 + 144 * MAX_POINTERS);
    m.extend_from_slice(&kind.to_ne_bytes());
    m.extend_from_slice(&seq.to_ne_bytes());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(touchable: [i32; 4], watch_outside: bool) -> Region {
        Region {
            frame: touchable,
            touchable,
            watch_outside,
            owner: 10_001,
            owner_pid: 4_000,
        }
    }

    /// A touch goes to the front window that takes it; the windows in
    /// front of it that watch outside touches are told.
    #[test]
    fn touches_go_to_the_front_window_that_takes_them() {
        let activity = region([0, 0, 1080, 1920], false);
        let tooltip = region([0, 0, 400, 120], true);
        // Outside the tooltip: the activity, the tooltip told.
        assert_eq!(
            touched(&[activity, tooltip], (540.0, 1000.0)),
            (Some(0), vec![1])
        );
        // On it: the tooltip.
        assert_eq!(
            touched(&[activity, tooltip], (100.0, 50.0)),
            (Some(1), vec![])
        );
        // A modal popup takes its task's touches, outside it too.
        let menu = region([0, 0, 1080, 1920], true);
        assert_eq!(
            touched(&[activity, menu], (540.0, 1000.0)),
            (Some(1), vec![])
        );
        // An untouchable window takes none.
        let untouchable = region([0, 0, 0, 0], false);
        assert_eq!(
            touched(&[activity, untouchable], (5.0, 5.0)),
            (Some(0), vec![])
        );
    }

    fn slot(t: &mut Touch, slot: i32, tracking: i32, x: i32, y: i32) {
        t.abs(ABS_MT_SLOT, slot);
        t.abs(ABS_MT_TRACKING_ID, tracking);
        if tracking >= 0 {
            t.abs(ABS_MT_POSITION_X, x);
            t.abs(ABS_MT_POSITION_Y, y);
        }
    }

    #[test]
    fn one_finger_taps() {
        let mut t = Touch::default();
        slot(&mut t, 0, 7, 100, 200);
        let m = t.report(10, 0, 0, (1.0, 1.0));
        assert_eq!((m.len(), m[0].action, m[0].down_time), (1, ACTION_DOWN, 10));
        assert_eq!(m[0].pointers, vec![(0, 100.0, 200.0)]);
        t.abs(ABS_MT_POSITION_X, 110);
        let m = t.report(20, 0, 0, (1.0, 1.0));
        assert_eq!(
            (m[0].action, m[0].pointers[0].1, m[0].down_time),
            (ACTION_MOVE, 110.0, 10)
        );
        assert!(t.report(25, 0, 0, (1.0, 1.0)).is_empty());
        slot(&mut t, 0, -1, 0, 0);
        let m = t.report(30, 0, 0, (1.0, 1.0));
        assert_eq!(
            (m[0].action, m[0].pointers.clone()),
            (ACTION_UP, vec![(0, 110.0, 200.0)])
        );
    }

    #[test]
    fn a_second_finger_is_a_pointer_down_and_up() {
        let mut t = Touch::default();
        slot(&mut t, 0, 1, 10, 10);
        t.report(1, 0, 0, (1.0, 1.0));
        slot(&mut t, 1, 2, 50, 50);
        let m = t.report(2, 0, 0, (1.0, 1.0));
        assert_eq!(
            m[0].action,
            ACTION_POINTER_DOWN | (1 << POINTER_INDEX_SHIFT)
        );
        assert_eq!(m[0].pointers.len(), 2);
        slot(&mut t, 0, -1, 0, 0);
        let m = t.report(3, 0, 0, (1.0, 1.0));
        assert_eq!(m[0].action, ACTION_POINTER_UP);
        assert_eq!(m[0].pointers, vec![(0, 10.0, 10.0), (1, 50.0, 50.0)]);
        // The remaining pointer keeps its id.
        slot(&mut t, 1, -1, 0, 0);
        let m = t.report(4, 0, 0, (1.0, 1.0));
        assert_eq!(
            (m[0].action, m[0].pointers.clone()),
            (ACTION_UP, vec![(1, 50.0, 50.0)])
        );
    }

    #[test]
    fn a_motion_has_the_published_layout() {
        let m = Motion::touched(ACTION_DOWN, 5, 5, vec![(0, 1.0, 2.0)]);
        let bytes = m.message(1, 2, (30.0, 40.0));
        assert_eq!(focus(1, 2, true).len(), 16);
        assert_eq!(m.gesture_start(), Some((1.0, 2.0)));
        // The window's transform moves the display's origin to the window's.
        let float = |at: usize| f32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!((float(8 + 112), float(8 + 116)), (-30.0, -40.0));
        // The header, the body up to the pointers, one pointer.
        assert_eq!(bytes.len(), 8 + 160 + 144);
        assert_eq!(
            &bytes[8 + 160..8 + 168],
            &[0, 0, 0, 0, TOOL_FINGER as u8, 0, 0, 0]
        );
        assert_eq!(
            u64::from_ne_bytes(bytes[176..184].try_into().unwrap()),
            0xe000_0000_0000_0000
        );
    }

    /// An injected tap's down and up: no device, the default size, and
    /// pressure only while down (an axis set to zero has no bit).
    #[test]
    fn injected_motions_are_of_no_device() {
        let down = Motion::injected(ACTION_DOWN, 7, 7, 1.0, 2.0, 1.0).unwrap();
        let bytes = down.message(1, 2, (0.0, 0.0));
        let int = |b: &[u8], at: usize| i32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(int(&bytes, 8 + 16), INJECTED_DEVICE_ID);
        assert_eq!(
            u64::from_ne_bytes(bytes[176..184].try_into().unwrap()),
            0xf000_0000_0000_0000
        );
        let up = Motion::injected(ACTION_UP, 7, 7, 1.0, 2.0, 0.0).unwrap();
        let bytes = up.message(1, 2, (0.0, 0.0));
        assert_eq!(
            u64::from_ne_bytes(bytes[176..184].try_into().unwrap()),
            0xd000_0000_0000_0000
        );
        let float = |at: usize| f32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!((float(184), float(188), float(192)), (1.0, 2.0, 1.0));
        assert!(Motion::injected(ACTION_OUTSIDE, 7, 7, 1.0, 2.0, 1.0).is_none());
    }
}
