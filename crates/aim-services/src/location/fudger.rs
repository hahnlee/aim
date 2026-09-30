//! `LocationFudger`: the coarse location a coarse-permission client gets
//! of a fine one, snapped to a grid at the coarse accuracy after a
//! slowly drifting random offset, following the original at the pinned
//! tag.
//!
//! The image turns on density-based coarsening
//! (`density_based_coarse_locations`, `population_density_provider`):
//! with the population density provider's cache, a location snaps to the
//! center of the S2 cell of its coarsening level, and its accuracy is
//! that cell's edge.

use super::parcels::{HAS_ALTITUDE, HAS_BEARING, HAS_SPEED, Location};
use super::proxy::Density;

const MIN_ACCURACY_M: f32 = 200.0;
const OFFSET_UPDATE_INTERVAL_MS: i64 = 60 * 60 * 1000;
const CHANGE_PER_INTERVAL: f64 = 0.03;
const APPROXIMATE_METERS_PER_DEGREE_AT_EQUATOR: f64 = 111_000.0;
const MAX_LATITUDE: f64 = 90.0 - (1.0 / APPROXIMATE_METERS_PER_DEGREE_AT_EQUATOR);
/// The average edge of an S2 cell per level, in km.
const S2_CELL_AVG_EDGE_PER_LEVEL: [f32; 15] = [
    9220.14, 4610.07, 2305.04, 1152.52, 576.26, 288.13, 144.06, 72.03, 36.02, 20.79, 9.0, 5.05,
    2.25, 1.13, 0.57,
];

pub struct Fudger {
    accuracy_m: f32,
    latitude_offset_m: f64,
    longitude_offset_m: f64,
    next_update_realtime_ms: i64,
    /// The last fine location made coarse, and what it became.
    cached: Option<(Location, Location)>,
    random: Random,
}

impl Fudger {
    pub fn new(accuracy_m: f32, now_ms: i64) -> Fudger {
        let mut f = Fudger {
            accuracy_m: accuracy_m.max(MIN_ACCURACY_M),
            latitude_offset_m: 0.0,
            longitude_offset_m: 0.0,
            next_update_realtime_ms: 0,
            cached: None,
            random: Random::seeded(),
        };
        f.reset_offsets(now_ms);
        f
    }

    /// `resetOffsets`.
    pub fn reset_offsets(&mut self, now_ms: i64) {
        self.latitude_offset_m = self.next_random_offset();
        self.longitude_offset_m = self.next_random_offset();
        self.next_update_realtime_ms = now_ms + OFFSET_UPDATE_INTERVAL_MS;
        self.cached = None;
    }

    /// `createCoarse`, with the population density provider's cache if
    /// the provider has one.
    pub fn coarse(&mut self, fine: &Location, now_ms: i64, density: Option<&Density>) -> Location {
        if let Some((f, c)) = &self.cached
            && (f == fine || c == fine)
        {
            return c.clone();
        }
        self.update_offsets(now_ms);
        let mut coarse = fine.clone();
        coarse.fields &= !(HAS_BEARING | HAS_SPEED | HAS_ALTITUDE);
        coarse.bearing = 0.0;
        coarse.speed = 0.0;
        coarse.altitude = 0.0;
        coarse.extras = None;
        let mut latitude = wrap_latitude(coarse.latitude);
        let mut longitude = wrap_longitude(coarse.longitude);
        longitude += wrap_longitude(meters_to_degrees_longitude(
            self.longitude_offset_m,
            latitude,
        ));
        latitude += wrap_latitude(meters_to_degrees_latitude(self.latitude_offset_m));
        let mut accuracy = self.accuracy_m;
        let (lat, lng) = match density {
            Some(d) if d.has_default() => {
                let level = d.coarsening_level(latitude, longitude);
                let cell =
                    super::s2::parent(super::s2::from_lat_lng_degrees(latitude, longitude), level);
                accuracy = s2_cell_edge_m(level);
                super::s2::to_lat_lng_degrees(cell)
            }
            Some(d) => {
                d.default_not_set();
                self.snap_to_grid(latitude, longitude)
            }
            None => self.snap_to_grid(latitude, longitude),
        };
        coarse.latitude = lat;
        coarse.longitude = lng;
        coarse.fields |= super::parcels::HAS_HORIZONTAL_ACCURACY;
        coarse.accuracy = accuracy.max(fine.accuracy_or_zero());
        self.cached = Some((fine.clone(), coarse.clone()));
        coarse
    }

    fn snap_to_grid(&self, latitude: f64, longitude: f64) -> (f64, f64) {
        let lat_granularity = meters_to_degrees_latitude(self.accuracy_m as f64);
        let lon_granularity = meters_to_degrees_longitude(self.accuracy_m as f64, latitude);
        (
            wrap_latitude(java_round(latitude / lat_granularity) * lat_granularity),
            wrap_longitude(java_round(longitude / lon_granularity) * lon_granularity),
        )
    }

    fn update_offsets(&mut self, now_ms: i64) {
        if now_ms < self.next_update_realtime_ms {
            return;
        }
        let new_weight = CHANGE_PER_INTERVAL;
        let old_weight = (1.0 - new_weight * new_weight).sqrt();
        self.latitude_offset_m =
            old_weight * self.latitude_offset_m + new_weight * self.next_random_offset();
        self.longitude_offset_m =
            old_weight * self.longitude_offset_m + new_weight * self.next_random_offset();
        self.next_update_realtime_ms = now_ms + OFFSET_UPDATE_INTERVAL_MS;
    }

    fn next_random_offset(&mut self) -> f64 {
        self.random.next_gaussian() * (self.accuracy_m as f64 / 4.0)
    }
}

impl Location {
    /// `getAccuracy()`: 0 without one.
    fn accuracy_or_zero(&self) -> f32 {
        if self.has(super::parcels::HAS_HORIZONTAL_ACCURACY) {
            self.accuracy
        } else {
            0.0
        }
    }
}

/// `getS2CellApproximateEdge(level)`, in meters.
fn s2_cell_edge_m(level: i32) -> f32 {
    let level = level.clamp(0, S2_CELL_AVG_EDGE_PER_LEVEL.len() as i32 - 1);
    S2_CELL_AVG_EDGE_PER_LEVEL[level as usize] * 1000.0
}

/// `Math.round(double)`: half up.
fn java_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

fn wrap_latitude(lat: f64) -> f64 {
    lat.clamp(-MAX_LATITUDE, MAX_LATITUDE)
}

fn wrap_longitude(lon: f64) -> f64 {
    let mut lon = lon % 360.0;
    if lon >= 180.0 {
        lon -= 360.0;
    }
    if lon < -180.0 {
        lon += 360.0;
    }
    lon
}

fn meters_to_degrees_latitude(distance: f64) -> f64 {
    distance / APPROXIMATE_METERS_PER_DEGREE_AT_EQUATOR
}

fn meters_to_degrees_longitude(distance: f64, lat: f64) -> f64 {
    let cos_lat = lat.to_radians().cos();
    if cos_lat == 0.0 {
        return 0.0001;
    }
    distance / APPROXIMATE_METERS_PER_DEGREE_AT_EQUATOR / cos_lat
}

/// A normally distributed random source for the offsets (the original's
/// `SecureRandom.nextGaussian`), seeded from the system's entropy.
struct Random {
    state: u64,
    spare: Option<f64>,
}

impl Random {
    fn seeded() -> Random {
        let mut seed = [0u8; 8];
        // SAFETY: a valid buffer of its length.
        unsafe { libc::arc4random_buf(seed.as_mut_ptr().cast(), seed.len()) };
        Random {
            state: u64::from_le_bytes(seed) | 1,
            spare: None,
        }
    }

    /// A uniform value in (0, 1] (xorshift64*).
    fn next_uniform(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let v = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        (v as f64 + 1.0) / (1u64 << 53) as f64
    }

    /// Marsaglia's polar method, as `Random.nextGaussian`.
    fn next_gaussian(&mut self) -> f64 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        loop {
            let v1 = 2.0 * self.next_uniform() - 1.0;
            let v2 = 2.0 * self.next_uniform() - 1.0;
            let s = v1 * v1 + v2 * v2;
            if s < 1.0 && s != 0.0 {
                let multiplier = (-2.0 * s.ln() / s).sqrt();
                self.spare = Some(v2 * multiplier);
                return v1 * multiplier;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coarse_locations_are_on_the_grid_and_stable() {
        let mut f = Fudger::new(2000.0, 0);
        let fine = Location {
            provider: Some("gps".into()),
            fields: super::super::parcels::HAS_HORIZONTAL_ACCURACY | HAS_SPEED,
            latitude: 37.5665,
            longitude: 126.978,
            accuracy: 5.0,
            speed: 3.0,
            ..Location::default()
        };
        let c = f.coarse(&fine, 1, None);
        assert_eq!(c.accuracy, 2000.0);
        assert!(!c.has(HAS_SPEED));
        assert!(fine.distance_to(&c) < 2000.0 * 2.5);
        let granularity = 2000.0 / APPROXIMATE_METERS_PER_DEGREE_AT_EQUATOR;
        let steps = c.latitude / granularity;
        assert!((steps - steps.round()).abs() < 1e-6);
        // The same fine location, or the coarse one, coarsens the same.
        assert_eq!(f.coarse(&fine, 2, None), c);
        assert_eq!(f.coarse(&c, 2, None), c);
    }

    #[test]
    fn gaussian_is_centered() {
        let mut r = Random::seeded();
        let n = 20_000;
        let mean: f64 = (0..n).map(|_| r.next_gaussian()).sum::<f64>() / n as f64;
        assert!(mean.abs() < 0.05, "{mean}");
    }
}
