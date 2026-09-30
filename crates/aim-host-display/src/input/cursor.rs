//! The hot spot of the pointer's sprite (`docs/input.md`, "The pointer
//! icon").
//!
//! Android draws the mouse pointer as a sprite, a `CURSOR` layer that the
//! composer HAL passes on as the display's hardware cursor: its image and
//! where its top left corner lies. The icon's hot spot (the pixel that is
//! the pointer: an arrow's tip, an I-beam's middle) is not among them; a
//! composer has no call for it, unlike a DRM cursor plane. It follows from
//! the mouse: the sprite lies at the pointer's position minus the hot
//! spot, and the pointer's position is one the mouse device reported, a
//! few events old while it moves. The hot spot is the offset that puts the
//! most of an image's recent positions on positions the mouse had; on a
//! tie, an image's known one, else the one as many events old as the last
//! image showed. It is exact at once when the mouse rests, and while it
//! moves once an image was seen at rest. Each image's hot spot is kept for
//! when it comes back.

use std::collections::{HashMap, VecDeque};

/// Mouse positions kept: more than the frames between an event and the
/// sprite's move.
const TRAIL: usize = 32;
/// An image's positions compared.
const PLACED: usize = 8;

#[derive(Default)]
pub struct Hotspots {
    /// The mouse's positions (display pixels), newest last.
    trail: VecDeque<(i32, i32)>,
    /// The image shown and its recent positions, newest last.
    image: u64,
    placed: VecDeque<(i32, i32)>,
    known: HashMap<u64, (i32, i32)>,
    /// How many mouse positions the sprite's last position was behind.
    lag: usize,
}

impl Hotspots {
    /// The mouse is at display pixel `(x, y)`.
    pub fn pointer(&mut self, x: i32, y: i32) {
        if self.trail.back() != Some(&(x, y)) {
            if self.trail.len() == TRAIL {
                self.trail.pop_front();
            }
            self.trail.push_back((x, y));
        }
    }

    /// Image `key`, `w` x `h` pixels, lies at `(x, y)`: its hot spot, in
    /// the image's pixels.
    pub fn place(&mut self, key: u64, w: i32, h: i32, x: i32, y: i32) -> (i32, i32) {
        if key != self.image {
            self.image = key;
            self.placed.clear();
        }
        if self.placed.len() == PLACED {
            self.placed.pop_front();
        }
        self.placed.push_back((x, y));
        let score = |(hx, hy): (i32, i32)| {
            self.placed
                .iter()
                .filter(|&&(px, py)| self.trail.contains(&(px + hx, py + hy)))
                .count()
        };
        // The most placed positions, then the lag closest to the last one.
        let best = (self.trail.iter().rev().enumerate())
            .map(|(age, &(mx, my))| ((mx - x, my - y), age))
            .filter(|&((hx, hy), _)| (0..w).contains(&hx) && (0..h).contains(&hy))
            .max_by_key(|&(hot, age)| (score(hot), std::cmp::Reverse(age.abs_diff(self.lag))))
            .map(|(hot, _)| hot);
        let hot = match (best, self.known.get(&key).copied()) {
            (Some(b), Some(k)) if score(b) > score(k) => b,
            (_, Some(k)) => k,
            (b, None) => b.unwrap_or((0, 0)),
        };
        self.known.insert(key, hot);
        let pointer = (x + hot.0, y + hot.1);
        if let Some(age) = self.trail.iter().rev().position(|&p| p == pointer) {
            self.lag = age;
        }
        hot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rest_the_hot_spot_is_exact() {
        let mut h = Hotspots::default();
        h.pointer(100, 200);
        // An I-beam 20x40 whose middle is the pointer.
        assert_eq!(h.place(1, 20, 40, 90, 180), (10, 20));
        // Outside the image: the last known one stays.
        h.pointer(500, 500);
        assert_eq!(h.place(1, 20, 40, 90, 180), (10, 20));
        // A new image with no position inside it: none known.
        assert_eq!(h.place(2, 8, 8, 0, 0), (0, 0));
    }

    #[test]
    fn a_moving_pointer_settles_on_the_hot_spot() {
        let mut h = Hotspots::default();
        let (arrow, beam) = ((4, 6), (10, 20));
        // The mouse moves 3 pixels an event; the sprite shows the position
        // two events old.
        h.pointer(97, 100);
        h.pointer(100, 100);
        let mut i = 0;
        let mut step = |h: &mut Hotspots, key, (hx, hy): (i32, i32), moving: bool| {
            if moving {
                i += 1;
            }
            h.pointer(100 + 3 * i, 100);
            let shown = if moving { i - 2 } else { i };
            h.place(key, 32, 48, 100 + 3 * shown - hx, 100 - hy)
        };
        for _ in 0..3 {
            step(&mut h, 1, arrow, true);
        }
        // At first the guess is the newest position, as good as any while
        // the mouse moves steadily.
        assert_eq!(step(&mut h, 1, arrow, true), (10, 6));
        // At rest it is exact.
        assert_eq!(step(&mut h, 1, arrow, false), arrow);
        // Moving again, the arrow keeps it and shows the lag, which a new
        // image, the I-beam, then gets right at once.
        assert_eq!(step(&mut h, 1, arrow, true), arrow);
        assert_eq!(step(&mut h, 1, arrow, true), arrow);
        assert_eq!(step(&mut h, 2, beam, true), beam);
        // Back to the arrow: its hot spot at once.
        assert_eq!(step(&mut h, 1, arrow, true), arrow);
    }
}
