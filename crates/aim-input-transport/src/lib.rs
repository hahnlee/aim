//! Android's input transport as an input dispatcher fills it at
//! `android-16.0.0_r1`: the `InputMessage`s of `InputTransport.h`
//! (`InputPublisher`), and a direct touchscreen's multitouch protocol B
//! cooked into motions in the display's pixels, as InputReader's
//! `MultiTouchInputMapper` and `TouchInputMapper::dispatchTouches` cook it.

/// `InputMessage::Type`.
const MOTION: u32 = 1;
const FOCUS: u32 = 3;
/// `AINPUT_SOURCE_TOUCHSCREEN`, `ToolType::FINGER`.
const SOURCE_TOUCHSCREEN: i32 = 0x1002;
const TOOL_FINGER: i32 = 1;
/// `AMOTION_EVENT_ACTION_*`, `AMOTION_EVENT_ACTION_POINTER_INDEX_SHIFT`.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVE: i32 = 2;
const ACTION_POINTER_DOWN: i32 = 5;
const ACTION_POINTER_UP: i32 = 6;
const POINTER_INDEX_SHIFT: i32 = 8;
/// `AMOTION_EVENT_AXIS_X`, `_Y`, `_PRESSURE`.
const AXIS_X: u32 = 0;
const AXIS_Y: u32 = 1;
const AXIS_PRESSURE: u32 = 2;
/// `MAX_POINTERS`, `PointerCoords::MAX_AXES`, `MAX_POINTER_ID`.
const MAX_POINTERS: usize = 16;
const MAX_AXES: usize = 30;
const MAX_POINTER_ID: u32 = 31;
/// The first input device InputReader numbers (`END_RESERVED_ID`).
const DEVICE_ID: i32 = 1;

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
            let Some(tracking) = slot.tracking else { continue };
            let x = (slot.x - x_min) as f32 * scale.0;
            let y = (slot.y - y_min) as f32 * scale.1;
            // A pointer keeps its id while its tracking id lasts; a new one
            // takes the lowest free id.
            let id = self.pointers.iter().find(|p| p.1 == tracking).map(|p| p.0).unwrap_or_else(|| {
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
        let coords = |s: &[(u32, i32, f32, f32)]| s.iter().map(|p| (p.0, p.2, p.3)).collect::<Vec<_>>();
        // Ups, with the last positions of the pointers that stay.
        for up in self.pointers.iter().filter(|p| !current.iter().any(|c| c.0 == p.0)) {
            for p in state.iter_mut() {
                if let Some(c) = current.iter().find(|c| c.0 == p.0) {
                    *p = *c;
                }
            }
            let index = state.iter().position(|p| p.0 == up.0).unwrap() as i32;
            let action = if state.len() == 1 { ACTION_UP } else { ACTION_POINTER_UP | (index << POINTER_INDEX_SHIFT) };
            motions.push(Motion { action, time, down_time: self.down_time, pointers: coords(&state) });
            state.retain(|p| p.0 != up.0);
        }
        // A move of the pointers that stayed.
        let moved = state.iter().any(|p| current.iter().any(|c| c.0 == p.0 && (c.2, c.3) != (p.2, p.3)));
        for p in state.iter_mut() {
            if let Some(c) = current.iter().find(|c| c.0 == p.0) {
                *p = *c;
            }
        }
        if moved {
            motions.push(Motion { action: ACTION_MOVE, time, down_time: self.down_time, pointers: coords(&state) });
        }
        // Downs.
        for down in current.iter().filter(|c| !self.pointers.iter().any(|p| p.0 == c.0)) {
            state.push(*down);
            state.sort_by_key(|p| p.0);
            let action = if state.len() == 1 {
                self.down_time = time;
                ACTION_DOWN
            } else {
                let index = state.iter().position(|p| p.0 == down.0).unwrap() as i32;
                ACTION_POINTER_DOWN | (index << POINTER_INDEX_SHIFT)
            };
            motions.push(Motion { action, time, down_time: self.down_time, pointers: coords(&state) });
        }
        self.pointers = current;
        motions
    }
}

impl Motion {
    /// `InputMessage` of type `MOTION` as `InputPublisher::publishMotionEvent`
    /// fills it: the window and the display at the origin, unscaled.
    pub fn message(&self, seq: u32, id: i32) -> Vec<u8> {
        let mut m = header(MOTION, seq);
        let i32s = |m: &mut Vec<u8>, v: &[i32]| v.iter().for_each(|v| m.extend_from_slice(&v.to_ne_bytes()));
        let f32s = |m: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|v| m.extend_from_slice(&v.to_ne_bytes()));
        i32s(&mut m, &[id, self.pointers.len() as i32]);
        m.extend_from_slice(&self.time.to_ne_bytes());
        i32s(&mut m, &[DEVICE_ID, SOURCE_TOUCHSCREEN, 0]); // display 0
        m.extend_from_slice(&[0; 32]); // no HMAC
        i32s(&mut m, &[self.action, 0, 0, 0, 0]); // action, button, flags, meta, buttons
        m.extend_from_slice(&[0; 4]); // classification NONE
        i32s(&mut m, &[0]); // no edge flags
        m.extend_from_slice(&self.down_time.to_ne_bytes());
        f32s(&mut m, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]); // the window's transform
        f32s(&mut m, &[1.0, 1.0, f32::NAN, f32::NAN]); // precision; no cursor
        f32s(&mut m, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]); // the raw transform
        for &(pointer, x, y) in &self.pointers {
            i32s(&mut m, &[pointer as i32, TOOL_FINGER]);
            // `PointerCoords`: the axes' bits (highest first), their values,
            // not resampled.
            let bits = [AXIS_X, AXIS_Y, AXIS_PRESSURE].iter().fold(0u64, |b, a| b | (1 << 63) >> a);
            m.extend_from_slice(&bits.to_ne_bytes());
            let mut values = [0f32; MAX_AXES];
            values[..3].copy_from_slice(&[x, y, 1.0]);
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
        assert_eq!((m[0].action, m[0].pointers[0].1, m[0].down_time), (ACTION_MOVE, 110.0, 10));
        assert!(t.report(25, 0, 0, (1.0, 1.0)).is_empty());
        slot(&mut t, 0, -1, 0, 0);
        let m = t.report(30, 0, 0, (1.0, 1.0));
        assert_eq!((m[0].action, m[0].pointers.clone()), (ACTION_UP, vec![(0, 110.0, 200.0)]));
    }

    #[test]
    fn a_second_finger_is_a_pointer_down_and_up() {
        let mut t = Touch::default();
        slot(&mut t, 0, 1, 10, 10);
        t.report(1, 0, 0, (1.0, 1.0));
        slot(&mut t, 1, 2, 50, 50);
        let m = t.report(2, 0, 0, (1.0, 1.0));
        assert_eq!(m[0].action, ACTION_POINTER_DOWN | (1 << POINTER_INDEX_SHIFT));
        assert_eq!(m[0].pointers.len(), 2);
        slot(&mut t, 0, -1, 0, 0);
        let m = t.report(3, 0, 0, (1.0, 1.0));
        assert_eq!(m[0].action, ACTION_POINTER_UP);
        assert_eq!(m[0].pointers, vec![(0, 10.0, 10.0), (1, 50.0, 50.0)]);
        // The remaining pointer keeps its id.
        slot(&mut t, 1, -1, 0, 0);
        let m = t.report(4, 0, 0, (1.0, 1.0));
        assert_eq!((m[0].action, m[0].pointers.clone()), (ACTION_UP, vec![(1, 50.0, 50.0)]));
    }

    #[test]
    fn a_motion_has_the_published_layout() {
        let m = Motion { action: ACTION_DOWN, time: 5, down_time: 5, pointers: vec![(0, 1.0, 2.0)] };
        let bytes = m.message(1, 2);
        assert_eq!(focus(1, 2, true).len(), 16);
        // The header, the body up to the pointers, one pointer.
        assert_eq!(bytes.len(), 8 + 160 + 144);
        assert_eq!(&bytes[8 + 160..8 + 168], &[0, 0, 0, 0, TOOL_FINGER as u8, 0, 0, 0]);
        assert_eq!(u64::from_ne_bytes(bytes[176..184].try_into().unwrap()), 0xe000_0000_0000_0000);
    }
}
