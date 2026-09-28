//! The test pattern a session streams when macOS gives it no camera
//! frames: eight vertical color bars (white, yellow, cyan, green, magenta,
//! red, blue, black) and a white band that moves down one row step per
//! frame, so consecutive frames differ.

use crate::convert::Bgra;

/// BGRA colors of the bars, left to right.
pub const BARS: [[u8; 4]; 8] = [
    [255, 255, 255, 255],
    [0, 255, 255, 255],
    [255, 255, 0, 255],
    [0, 255, 0, 255],
    [255, 0, 255, 255],
    [0, 0, 255, 255],
    [255, 0, 0, 255],
    [0, 0, 0, 255],
];

/// Frame `n` of the pattern into `f` (whose size is kept).
pub fn draw(f: &mut Bgra, n: u64) {
    let (w, h) = (f.width as usize, f.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let band = (h / 16).max(1);
    let top = (n as usize * band / 4) % h;
    let bars: Vec<u8> = (0..w).flat_map(|x| BARS[x * 8 / w]).collect();
    let white: Vec<u8> = (0..w).flat_map(|_| BARS[0]).collect();
    for y in 0..h {
        let in_band = (y + h - top) % h < band;
        f.data[y * f.stride..][..w * 4].copy_from_slice(if in_band { &white } else { &bars });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_a_moving_band() {
        let mut f = Bgra::new(64, 32);
        draw(&mut f, 0);
        // Row 0 is in the band; row 16 shows the bars.
        assert!(f.data[..64 * 4].chunks_exact(4).all(|p| p == BARS[0]));
        let row = &f.data[16 * f.stride..][..64 * 4];
        for (i, bar) in BARS.iter().enumerate() {
            assert_eq!(&row[i * 8 * 4..][..4], bar);
        }
        let first = f.data.clone();
        draw(&mut f, 8);
        assert_ne!(f.data, first);
        assert!(
            f.data[4 * f.stride..][..64 * 4]
                .chunks_exact(4)
                .all(|p| p == BARS[0])
        );
    }
}
