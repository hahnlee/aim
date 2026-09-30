//! S2 cell ids, as the density-based coarsening of locations uses them:
//! a port of the parts of AOSP's
//! `com.android.internal.location.geometry.S2CellIdUtils` (Apache-2.0,
//! frameworks/base at the pinned tag) that `LocationFudger` and
//! `LocationFudgerCache` call.

use std::sync::OnceLock;

pub const MAX_LEVEL: i32 = 30;
const MAX_SIZE: i32 = 1 << MAX_LEVEL;
const POS_BITS: i32 = 2 * MAX_LEVEL + 1;
const SWAP_MASK: i32 = 0x1;
const INVERT_MASK: i32 = 0x2;
const LOOKUP_BITS: i32 = 4;
const LOOKUP_MASK: i32 = (1 << LOOKUP_BITS) - 1;
const MAX_SI_TI: i64 = 1 << (MAX_LEVEL + 1);
const I_SHIFT: i32 = 33;
const J_SHIFT: i32 = 2;
const J_MASK: i64 = (1 << 31) - 1;
const SI_SHIFT: i32 = 32;
const TI_MASK: i64 = (1 << 32) - 1;
const POS_TO_ORIENTATION: [i32; 4] = [SWAP_MASK, 0, 0, INVERT_MASK + SWAP_MASK];
const POS_TO_IJ: [[i32; 4]; 4] = [[0, 1, 3, 2], [0, 2, 3, 1], [3, 2, 0, 1], [3, 1, 0, 2]];

struct Lookup {
    pos: Vec<i32>,
    ij: Vec<i32>,
}

fn lookup() -> &'static Lookup {
    static LOOKUP: OnceLock<Lookup> = OnceLock::new();
    LOOKUP.get_or_init(|| {
        let size = 1 << (2 * LOOKUP_BITS + 2);
        let mut l = Lookup {
            pos: vec![0; size],
            ij: vec![0; size],
        };
        for o in [0, SWAP_MASK, INVERT_MASK, SWAP_MASK | INVERT_MASK] {
            init_cell(&mut l, 0, 0, 0, o, 0, o);
        }
        l
    })
}

fn init_cell(l: &mut Lookup, level: i32, i: i32, j: i32, orig: i32, pos: i32, orientation: i32) {
    if level == LOOKUP_BITS {
        let ij = (i << LOOKUP_BITS) + j;
        l.pos[((ij << 2) + orig) as usize] = (pos << 2) + orientation;
        l.ij[((pos << 2) + orig) as usize] = (ij << 2) + orientation;
        return;
    }
    for (sub, &mask) in POS_TO_ORIENTATION.iter().enumerate() {
        let ij = POS_TO_IJ[orientation as usize][sub];
        init_cell(
            l,
            level + 1,
            (i << 1) + (ij >> 1),
            (j << 1) + (ij & 1),
            orig,
            (pos << 2) + sub as i32,
            orientation ^ mask,
        );
    }
}

/// `fromLatLngDegrees`: the leaf cell of a point.
pub fn from_lat_lng_degrees(lat: f64, lng: f64) -> i64 {
    let (lat, lng) = (lat.to_radians(), lng.to_radians());
    let cos_lat = lat.cos();
    let (x, y, z) = (lng.cos() * cos_lat, lng.sin() * cos_lat, lat.sin());
    let face = xyz_to_face(x, y, z);
    let (u, v) = xyz_to_uv(face, x, y, z);
    from_fij(face, st_to_ij(u), st_to_ij(v))
}

/// `getParent(cell, level)`.
pub fn parent(cell: i64, level: i32) -> i64 {
    let lsb = lowest_on_bit_for_level(level);
    (cell & lsb.wrapping_neg()) | lsb
}

/// `getLevel`.
pub fn level(cell: i64) -> i32 {
    if is_leaf(cell) {
        return MAX_LEVEL;
    }
    MAX_LEVEL - (cell.trailing_zeros() as i32 >> 1)
}

/// `containsLatLngDegrees`.
pub fn contains(cell: i64, lat: f64, lng: f64) -> bool {
    parent(from_lat_lng_degrees(lat, lng), level(cell)) == cell
}

/// `toLatLngDegrees`: the cell's center.
pub fn to_lat_lng_degrees(cell: i64) -> (f64, f64) {
    let si_ti = to_si_ti(cell);
    let u = si_to_u((si_ti >> SI_SHIFT) as i32 as i64);
    let v = si_to_u(si_ti as i32 as i64);
    let (x, y, z) = uv_to_xyz(face(cell), u, v);
    let lat = z.atan2((x * x + y * y).sqrt());
    let lng = y.atan2(x);
    (lat.to_degrees(), lng.to_degrees())
}

fn face(cell: i64) -> i32 {
    ((cell as u64) >> POS_BITS) as i32
}

fn is_leaf(cell: i64) -> bool {
    (cell as i32) & 1 != 0
}

fn lowest_on_bit(cell: i64) -> i64 {
    cell & cell.wrapping_neg()
}

fn lowest_on_bit_for_level(level: i32) -> i64 {
    1i64 << (2 * (MAX_LEVEL - level))
}

fn from_fij(face: i32, i: i32, j: i32) -> i64 {
    let l = lookup();
    let mut bits = face & SWAP_MASK;
    let lookup_bits = |bits: i32, k: i32| {
        let mut b = bits;
        b += ((i >> (k * LOOKUP_BITS)) & LOOKUP_MASK) << (LOOKUP_BITS + 2);
        b += ((j >> (k * LOOKUP_BITS)) & LOOKUP_MASK) << 2;
        l.pos[b as usize]
    };
    let update =
        |sb: i64, k: i32, bits: i32| sb | (((bits as i64) >> 2) << ((k & 0x3) * 2 * LOOKUP_BITS));
    let mut msb = (face as i64) << (POS_BITS - 33);
    for k in (4..=7).rev() {
        bits = lookup_bits(bits, k);
        msb = update(msb, k, bits);
        bits &= SWAP_MASK | INVERT_MASK;
    }
    let mut lsb = 0i64;
    for k in (0..=3).rev() {
        bits = lookup_bits(bits, k);
        lsb = update(lsb, k, bits);
        bits &= SWAP_MASK | INVERT_MASK;
    }
    ((msb << 32).wrapping_add(lsb) << 1) + 1
}

fn to_ijo(cell: i64) -> i64 {
    let l = lookup();
    let mut bits = face(cell) & SWAP_MASK;
    let (mut i, mut j) = (0i32, 0i32);
    for k in (0..=7).rev() {
        let nbits = if k == 7 {
            MAX_LEVEL - 7 * LOOKUP_BITS
        } else {
            LOOKUP_BITS
        };
        bits +=
            (((cell as u64) >> (k * 2 * LOOKUP_BITS + 1)) as i32 & ((1 << (2 * nbits)) - 1)) << 2;
        bits = l.ij[bits as usize];
        i += (bits >> (LOOKUP_BITS + 2)) << (k * LOOKUP_BITS);
        j += ((bits >> 2) & ((1 << LOOKUP_BITS) - 1)) << (k * LOOKUP_BITS);
        bits &= SWAP_MASK | INVERT_MASK;
    }
    let orientation = if lowest_on_bit(cell) & 0x1111_1111_1111_1110 != 0 {
        bits ^ SWAP_MASK
    } else {
        bits
    };
    ((i as i64) << I_SHIFT) | ((j as i64) << J_SHIFT) | orientation as i64
}

fn to_si_ti(cell: i64) -> i64 {
    let ijo = to_ijo(cell);
    let i = ((ijo as u64) >> I_SHIFT) as i32;
    let j = (((ijo as u64) >> J_SHIFT) as i64 & J_MASK) as i32;
    let delta = if is_leaf(cell) {
        1
    } else if (i ^ ((cell as i32 as u32 >> 2) as i32)) & 1 != 0 {
        2
    } else {
        0
    };
    (((2 * i + delta) as i64) << SI_SHIFT) | ((2 * j + delta) as i64 & TI_MASK)
}

fn si_to_u(si: i64) -> f64 {
    let s = (1.0 / MAX_SI_TI as f64) * si as f64;
    if s >= 0.5 {
        (1.0 / 3.0) * (4.0 * s * s - 1.0)
    } else {
        (1.0 / 3.0) * (1.0 - 4.0 * (1.0 - s) * (1.0 - s))
    }
}

/// `uToI`.
fn st_to_ij(u: f64) -> i32 {
    let s = if u >= 0.0 {
        0.5 * (1.0 + 3.0 * u).sqrt()
    } else {
        1.0 - 0.5 * (1.0 - 3.0 * u).sqrt()
    };
    // Math.round: floor(x + 0.5).
    let rounded = (MAX_SIZE as f64 * s - 0.5 + 0.5).floor() as i64;
    rounded.clamp(0, MAX_SIZE as i64 - 1) as i32
}

fn xyz_to_face(x: f64, y: f64, z: f64) -> i32 {
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    if ax > ay {
        if ax > az {
            return if x < 0.0 { 3 } else { 0 };
        }
        return if z < 0.0 { 5 } else { 2 };
    }
    if ay > az {
        return if y < 0.0 { 4 } else { 1 };
    }
    if z < 0.0 { 5 } else { 2 }
}

fn xyz_to_uv(face: i32, x: f64, y: f64, z: f64) -> (f64, f64) {
    match face {
        0 => (y / x, z / x),
        1 => (-x / y, z / y),
        2 => (-x / z, -y / z),
        3 => (z / x, y / x),
        4 => (z / y, -x / y),
        _ => (-y / z, -x / z),
    }
}

fn uv_to_xyz(face: i32, u: f64, v: f64) -> (f64, f64, f64) {
    match face.min(5) {
        0 => (1.0, u, v),
        1 => (-u, 1.0, v),
        2 => (-u, -v, 1.0),
        3 => (-1.0, -v, -u),
        4 => (v, -1.0, -u),
        _ => (v, u, -1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_contain_their_points() {
        for (lat, lng) in [
            (37.5665, 126.978),
            (-33.86, 151.2),
            (51.5, -0.12),
            (0.0, 0.0),
        ] {
            let leaf = from_lat_lng_degrees(lat, lng);
            assert_eq!(level(leaf), MAX_LEVEL);
            for l in [0, 5, 10, 13, 20] {
                let cell = parent(leaf, l);
                assert_eq!(level(cell), l);
                assert!(contains(cell, lat, lng));
            }
            let (clat, clng) = to_lat_lng_degrees(leaf);
            assert!((clat - lat).abs() < 1e-6 && (clng - lng).abs() < 1e-6);
        }
    }

    #[test]
    fn matches_known_ids() {
        // S2CellId.fromLatLng(0, 0) is face 0's center: 0x1000000000000001.
        assert_eq!(from_lat_lng_degrees(0.0, 0.0) as u64, 0x1000_0000_0000_0001);
        // The face 0 cell at level 0.
        assert_eq!(
            parent(from_lat_lng_degrees(0.0, 0.0), 0) as u64,
            0x1000_0000_0000_0000
        );
    }
}
