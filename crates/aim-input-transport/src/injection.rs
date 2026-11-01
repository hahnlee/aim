//! Pinned InputEvent/KeyEvent/MotionEvent parcels and InputDispatcher injection
//! normalization (android-16.0.0_r1, AOSP Apache-2.0). InputMessage offsets are
//! the arm64 InputTransport.h layout; parcel transforms use a different order.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::keyboard::KeyEvent;

#[derive(Clone)]
pub struct Event {
    pub parcel: Vec<u8>,
    pub messages: Vec<Vec<u8>>,
    pub valid: bool,
}
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let bytes = self.0.get(..n)?;
        self.0 = self.0.get(n.next_multiple_of(4)..)?;
        Some(bytes)
    }
    fn int(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn float(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.int()? as u32))
    }
    fn long(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn transform(&mut self) -> Option<[f32; 6]> {
        let [a, b, tx, c, d, ty] = [
            self.float()?,
            self.float()?,
            self.float()?,
            self.float()?,
            self.float()?,
            self.float()?,
        ];
        Some([a, b, c, d, tx, ty])
    }
}
pub fn int(m: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(m[at..at + 4].try_into().unwrap())
}
fn put(m: &mut [u8], at: usize, value: i32) {
    m[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn floats(m: &mut [u8], at: usize, values: &[f32]) {
    for (i, v) in values.iter().enumerate() {
        put(m, at + i * 4, v.to_bits() as i32);
    }
}
fn xy(t: [f32; 6], x: f32, y: f32, offset: bool) -> (f32, f32) {
    let [a, b, c, d, tx, ty] = t;
    let round = |v: f32| (v * 1024.0).round() / 1024.0;
    (
        round(a * x + b * y + if offset { tx } else { 0.0 }),
        round(c * x + d * y + if offset { ty } else { 0.0 }),
    )
}
impl Event {
    /// InputReader's cooked InputMessage as the pinned Java InputEvent parcel.
    /// Messages here have display-space coordinates and identity transforms.
    pub fn from_message(m: &[u8]) -> Vec<u8> {
        let motion = int(m, 0) == 1;
        let mut p = Vec::new();
        let mut word = |v: i32| p.extend_from_slice(&v.to_le_bytes());
        word(if motion { 1 } else { 2 });
        if motion {
            word(int(m, 12));
            word(1);
        }
        for at in [8, 24, 28, 32] {
            word(int(m, at));
        }
        word(32);
        p.extend_from_slice(&m[36..68]);
        if !motion {
            for at in [68, 76, 88, 84, 80, 72, 96, 100, 16, 20] {
                p.extend_from_slice(&m[at..at + 4]);
            }
            p.extend_from_slice(&(-1i32).to_le_bytes()); // null characters
        } else {
            p.extend_from_slice(&m[68..96]);
            // Native parcel Transform is [a,b,tx,c,d,ty], unlike InputMessage.
            for at in [
                104, 108, 120, 112, 116, 124, 128, 132, 136, 140, 144, 148, 160, 152, 156, 164, 96,
                100,
            ] {
                p.extend_from_slice(&m[at..at + 4]);
            }
            let count = int(m, 12) as usize;
            for i in 0..count {
                p.extend_from_slice(&m[168 + i * 144..176 + i * 144]);
            }
            p.extend_from_slice(&m[16..24]);
            for i in 0..count {
                let at = 168 + i * 144;
                let bits = u64::from_le_bytes(m[at + 8..at + 16].try_into().unwrap());
                p.extend_from_slice(&m[at + 8..at + 16 + bits.count_ones() as usize * 4]);
                p.extend_from_slice(&(m[at + 136] as i32).to_le_bytes());
            }
        }
        p
    }

    /// Filter re-injection preserves the device and accessibility-origin flags.
    pub fn filtered(&mut self, flags: i32) {
        let device = int(&self.parcel, if self.motion() { 16 } else { 8 });
        let motion = self.motion();
        for m in &mut self.messages {
            put(m, 24, device);
            if flags & 0x20000 != 0 {
                let at = if motion { 76 } else { 72 };
                let value = int(m, at) | 0x800;
                put(m, at, value);
            }
        }
    }
    /// Consume one parcel body, retaining every original byte, including key
    /// characters, HMAC, history, sparse axes, resampling and both transforms.
    pub fn decode(bytes: &[u8]) -> Option<(Self, usize)> {
        let mut r = Reader(bytes);
        let kind = r.int()?;
        let mut messages = Vec::new();
        let valid;
        if kind == 2 {
            let (id, device, source, display) = (r.int()?, r.int()?, r.int()?, r.int()?);
            let hmac = r.int()?;
            if hmac != 32 {
                return None;
            }
            r.take(32)?;
            let (action, key_code, repeat_count, meta_state, scan_code, flags) =
                (r.int()?, r.int()?, r.int()?, r.int()?, r.int()?, r.int()?);
            let (down_time, event_time) = (r.long()?, r.long()?);
            let chars = r.int()?;
            if chars >= 0 {
                r.take((chars as usize).checked_add(1)?.checked_mul(2)?)?;
            }
            let mut m = KeyEvent {
                device,
                source,
                display,
                action,
                flags,
                key_code,
                scan_code,
                meta_state,
                repeat_count,
                down_time,
                event_time,
            }
            .message(0, id);
            put(&mut m, 24, -1); // InputDispatcher resolves every injected device to virtual.
            messages.push(m);
            valid = matches!(action, 0 | 1);
        } else if kind == 1 {
            let (count, samples) = (r.int()?, r.int()?);
            if !(1..=16).contains(&count) || !(1..=65535).contains(&samples) {
                return None;
            }
            let (id, _device, source, display) = (r.int()?, r.int()?, r.int()?, r.int()?);
            if r.int()? != 32 {
                return None;
            }
            r.take(32)?;
            let (action, button, flags, edge, meta, buttons, class) = (
                r.int()?,
                r.int()?,
                r.int()?,
                r.int()?,
                r.int()?,
                r.int()?,
                r.int()?,
            );
            let transform = r.transform()?;
            let precision = [r.float()?, r.float()?];
            let cursor = (r.float()?, r.float()?);
            r.transform()?; // Injection uses the logical transform, not the raw transform.
            let down = r.long()?;
            let mut properties = Vec::new();
            for _ in 0..count {
                properties.push((r.int()?, r.int()?));
            }
            let action_ok = match action & 255 {
                0 | 1 => count == 1,
                2 | 3 | 4 | 7 | 8 | 9 | 10 => true,
                5 | 6 => count > 1 && ((action >> 8) & 255) < count,
                11 | 12 => button != 0,
                _ => false,
            };
            valid = action_ok
                && properties.iter().enumerate().all(|(i, p)| {
                    (0..=31).contains(&p.0) && !properties[..i].iter().any(|q| q.0 == p.0)
                });
            let pointer = source & 2 != 0;
            let transform_axes = source & (8 | 16) == 0 && source & 0x20004 != 0x20004;
            let cursor = if transform_axes {
                xy(transform, cursor.0, cursor.1, pointer)
            } else {
                cursor
            };
            for _ in 0..samples {
                let mut m = vec![0; 168 + count as usize * 144];
                put(&mut m, 0, 1);
                put(&mut m, 8, id);
                put(&mut m, 12, count);
                m[16..24].copy_from_slice(&r.long()?.to_le_bytes());
                for (at, value) in [
                    (24, -1),
                    (28, source),
                    (32, if pointer && display == -1 { 0 } else { display }),
                    (68, action),
                    (72, button),
                    (76, flags),
                    (80, meta),
                    (84, buttons),
                    (88, class & 255),
                    (92, edge),
                ] {
                    put(&mut m, at, value);
                }
                m[96..104].copy_from_slice(&down.to_le_bytes());
                floats(&mut m, 104, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                floats(&mut m, 128, &precision);
                floats(&mut m, 136, &[cursor.0, cursor.1]);
                floats(&mut m, 144, &[1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                for (i, &(id, tool)) in properties.iter().enumerate() {
                    let at = 168 + i * 144;
                    put(&mut m, at, id);
                    put(&mut m, at + 4, tool);
                    let mut bits = r.long()? as u64;
                    if bits.count_ones() > 30 {
                        return None;
                    }
                    let mut axes = [0.0; 64];
                    for (a, v) in axes.iter_mut().enumerate() {
                        if bits & (1 << 63 >> a) != 0 {
                            *v = r.float()?;
                        }
                    }
                    let resampled = r.int()? != 0;
                    if transform_axes {
                        (axes[0], axes[1]) = xy(transform, axes[0], axes[1], pointer);
                        (axes[27], axes[28]) = xy(transform, axes[27], axes[28], false);
                        let (x, y) = (axes[8].sin(), -axes[8].cos());
                        let (mut x, mut y) = (
                            transform[0] * x + transform[1] * y,
                            transform[2] * x + transform[3] * y,
                        );
                        if flags & 0x100 == 0 && y > 0.0 {
                            x = -x;
                            y = -y;
                        }
                        axes[8] = if flags & 0x80 != 0 { x.atan2(-y) } else { 0.0 };
                    }
                    if transform_axes {
                        for a in [0, 1, 27, 28, 8] {
                            let bit = 1 << 63 >> a;
                            if axes[a] != 0.0 && bits.count_ones() < 30 {
                                bits |= bit;
                            }
                        }
                    }
                    m[at + 8..at + 16].copy_from_slice(&bits.to_le_bytes());
                    let values: Vec<_> = axes
                        .into_iter()
                        .enumerate()
                        .filter_map(|(a, v)| (bits & (1 << 63 >> a) != 0).then_some(v))
                        .collect();
                    floats(&mut m, at + 16, &values);
                    m[at + 136] = resampled as u8;
                }
                messages.push(m);
            }
        } else {
            return None;
        }
        let consumed = bytes.len() - r.0.len();
        Some((
            Self {
                parcel: bytes[..consumed].to_vec(),
                messages,
                valid,
            },
            consumed,
        ))
    }
    pub fn motion(&self) -> bool {
        int(&self.messages[0], 0) == 1
    }
    pub fn action(&self) -> i32 {
        int(&self.messages[0], 68) & 255
    }
    pub fn pointer(&self) -> bool {
        self.motion() && int(&self.messages[0], 28) & 2 != 0
    }
    pub fn display(&self) -> i32 {
        int(&self.messages[0], 32)
    }
    pub fn position(&self) -> (f32, f32) {
        let m = &self.messages[0];
        if int(m, 28) & 0x2002 == 0x2002 {
            return (
                f32::from_bits(int(m, 136) as u32),
                f32::from_bits(int(m, 140) as u32),
            );
        }
        let bits = u64::from_le_bytes(m[176..184].try_into().unwrap());
        let x = if bits >> 63 != 0 {
            f32::from_bits(int(m, 184) as u32)
        } else {
            0.0
        };
        let y = if bits & (1 << 62) != 0 {
            f32::from_bits(int(m, 184 + (bits >> 63) as usize * 4) as u32)
        } else {
            0.0
        };
        (x, y)
    }
    pub fn message(
        &self,
        sample: usize,
        seq: u32,
        origin: (f32, f32),
        outside: Option<bool>,
    ) -> Vec<u8> {
        let mut m = self.messages[sample].clone();
        put(&mut m, 4, seq as i32);
        if self.motion() {
            floats(&mut m, 120, &[-origin.0, -origin.1]);
            if let Some(visible) = outside {
                put(&mut m, 68, 4);
                put(&mut m, 12, 1);
                m.truncate(312);
                if !visible {
                    m[176..312].fill(0);
                    floats(&mut m, 120, &[0.0, 0.0]);
                }
            }
        }
        m
    }
}

#[cfg(test)]
#[path = "injection_tests.rs"]
mod tests;
