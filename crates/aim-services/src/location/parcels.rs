//! The location API's parcelables in their Java wire format (their
//! `writeToParcel` and `CREATOR` at the pinned tag), with the few
//! behaviors the service needs of them (`LocationRequest`'s derived
//! values, `WorkSource.add`, `PackageTagsList.contains`,
//! `Location.distanceTo`).

use std::collections::BTreeMap;

use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader, Result};
use aim_service_aidl::{ReadParcelable, WriteParcelable, read_int_array, read_string_list};

pub fn read_f64(r: &mut Reader<'_>) -> Result<f64> {
    Ok(f64::from_bits(r.read_i64()? as u64))
}

pub fn write_f64(p: &mut Parcel, v: f64) {
    p.write_i64(v.to_bits() as i64);
}

/// `Double.compare(a, b) == 0` and `Float.compare`: NaN equals NaN, 0.0
/// differs from -0.0.
fn same_f64(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn same_f32(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// A bundle kept as the bytes it arrived as (`writeBundle`'s form, its
/// length first); a bundle carrying binders or file descriptors is not
/// kept (see [`RawBundle::read`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawBundle(Vec<u8>);

impl RawBundle {
    /// `readBundle`: `None` for a null or empty bundle. The contents are
    /// not parsed; a bundle with objects in it is refused, since its
    /// binders and files would not outlive the call.
    pub fn read(r: &mut Reader<'_>) -> Result<Option<RawBundle>> {
        let start = r.position();
        let length = r.read_i32()?;
        if length <= 0 {
            return Ok(None);
        }
        r.read_i32()?; // magic
        r.skip(length as usize)?;
        r.read_bool()?; // has an intent
        let (bytes, objects) = r.since(start);
        if !objects.is_empty() {
            return Err(BAD_VALUE);
        }
        Ok(Some(RawBundle(bytes.to_vec())))
    }

    /// A bundle [`write_bundle`] wrote alone into `p`.
    pub fn from_parcel(p: &Parcel) -> Option<RawBundle> {
        RawBundle::read(&mut Reader::new(p.data(), p.objects())).ok()?
    }

    pub fn write(bundle: Option<&RawBundle>, p: &mut Parcel) {
        match bundle {
            Some(b) => p.write_raw(&b.0, &[]),
            None => p.write_i32(-1),
        }
    }
}

/// `Location`'s field mask bits.
pub const HAS_ALTITUDE: i32 = 1 << 0;
pub const HAS_SPEED: i32 = 1 << 1;
pub const HAS_BEARING: i32 = 1 << 2;
pub const HAS_HORIZONTAL_ACCURACY: i32 = 1 << 3;
pub const HAS_MOCK_PROVIDER: i32 = 1 << 4;
pub const HAS_ALTITUDE_ACCURACY: i32 = 1 << 5;
pub const HAS_SPEED_ACCURACY: i32 = 1 << 6;
pub const HAS_BEARING_ACCURACY: i32 = 1 << 7;
pub const HAS_ELAPSED_REALTIME_UNCERTAINTY: i32 = 1 << 8;
pub const HAS_MSL_ALTITUDE: i32 = 1 << 9;
pub const HAS_MSL_ALTITUDE_ACCURACY: i32 = 1 << 10;

/// `LocationResult.MAX_ACCURACY_M` and `MAX_SPEED_MPS`.
const MAX_ACCURACY_M: f32 = 1000.0 * 1000.0;
const MAX_SPEED_MPS: f32 = 1000.0 * 1000.0;

/// `android.location.Location`.
#[derive(Clone, Debug, Default)]
pub struct Location {
    pub provider: Option<String>,
    pub fields: i32,
    pub time_ms: i64,
    pub elapsed_realtime_ns: i64,
    pub elapsed_realtime_uncertainty_ns: f64,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: f64,
    pub speed: f32,
    pub bearing: f32,
    pub accuracy: f32,
    pub vertical_accuracy: f32,
    pub speed_accuracy: f32,
    pub bearing_accuracy: f32,
    pub msl_altitude: f64,
    pub msl_altitude_accuracy: f32,
    pub extras: Option<RawBundle>,
}

impl Location {
    pub fn has(&self, field: i32) -> bool {
        self.fields & field != 0
    }

    pub fn is_mock(&self) -> bool {
        self.has(HAS_MOCK_PROVIDER)
    }

    /// `isComplete`.
    pub fn is_complete(&self) -> bool {
        self.provider.is_some()
            && self.has(HAS_HORIZONTAL_ACCURACY)
            && self.time_ms != 0
            && self.elapsed_realtime_ns != 0
    }

    pub fn elapsed_realtime_ms(&self) -> i64 {
        self.elapsed_realtime_ns / 1_000_000
    }

    /// `getElapsedRealtimeAgeMillis()`, `now_ms` being the elapsed
    /// realtime clock.
    pub fn age_ms(&self, now_ms: i64) -> i64 {
        now_ms - self.elapsed_realtime_ms()
    }

    /// `distanceTo`: the WGS84 inverse formula, as `Location` computes it.
    pub fn distance_to(&self, other: &Location) -> f32 {
        distance(
            self.latitude,
            self.longitude,
            other.latitude,
            other.longitude,
        )
    }

    /// `LocationResult.validate` of one location with location validation
    /// on (the `location_validation` flag of the image); `previous_ns` is
    /// the realtime of the one before it. Removes an unreasonable speed,
    /// as the original does.
    pub fn validate(&mut self, previous_ns: i64, now_ns: i64) -> std::result::Result<(), String> {
        let (lat, lng) = (self.latitude, self.longitude);
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lng) {
            return Err("location must have valid lat/lng".into());
        }
        if !self.has(HAS_HORIZONTAL_ACCURACY) {
            return Err("location must have accuracy".into());
        }
        if !(0.0..=MAX_ACCURACY_M).contains(&self.accuracy) {
            return Err("location must have reasonable accuracy".into());
        }
        if self.time_ms < 0 {
            return Err("location must have valid time".into());
        }
        if previous_ns > self.elapsed_realtime_ns {
            return Err("location must have valid monotonically increasing realtime".into());
        }
        if self.elapsed_realtime_ns > now_ns {
            return Err("location must not have realtime in the future".into());
        }
        if !self.is_mock() {
            if self.provider.is_none() {
                return Err("location must have valid provider".into());
            }
            if lat == 0.0 && lng == 0.0 {
                return Err("location must not be at 0,0".into());
            }
        }
        if self.has(HAS_SPEED) && !(0.0..=MAX_SPEED_MPS).contains(&self.speed) {
            self.fields &= !HAS_SPEED;
            self.speed = 0.0;
        }
        Ok(())
    }
}

impl PartialEq for Location {
    /// `Location.equals`.
    fn eq(&self, o: &Location) -> bool {
        let both = |bit: i32| self.has(bit) == o.has(bit);
        let f64s = |bit: i32, a: f64, b: f64| both(bit) && (!self.has(bit) || same_f64(a, b));
        let f32s = |bit: i32, a: f32, b: f32| both(bit) && (!self.has(bit) || same_f32(a, b));
        self.time_ms == o.time_ms
            && self.elapsed_realtime_ns == o.elapsed_realtime_ns
            && f64s(
                HAS_ELAPSED_REALTIME_UNCERTAINTY,
                self.elapsed_realtime_uncertainty_ns,
                o.elapsed_realtime_uncertainty_ns,
            )
            && same_f64(self.latitude, o.latitude)
            && same_f64(self.longitude, o.longitude)
            && f64s(HAS_ALTITUDE, self.altitude, o.altitude)
            && f32s(HAS_SPEED, self.speed, o.speed)
            && f32s(HAS_BEARING, self.bearing, o.bearing)
            && f32s(HAS_HORIZONTAL_ACCURACY, self.accuracy, o.accuracy)
            && f32s(
                HAS_ALTITUDE_ACCURACY,
                self.vertical_accuracy,
                o.vertical_accuracy,
            )
            && f32s(HAS_SPEED_ACCURACY, self.speed_accuracy, o.speed_accuracy)
            && f32s(
                HAS_BEARING_ACCURACY,
                self.bearing_accuracy,
                o.bearing_accuracy,
            )
            && f64s(HAS_MSL_ALTITUDE, self.msl_altitude, o.msl_altitude)
            && f32s(
                HAS_MSL_ALTITUDE_ACCURACY,
                self.msl_altitude_accuracy,
                o.msl_altitude_accuracy,
            )
            && self.provider == o.provider
            && self.extras == o.extras
    }
}

impl ReadParcelable for Location {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let mut l = Location {
            provider: r.read_string8()?,
            fields: r.read_i32()?,
            time_ms: r.read_i64()?,
            elapsed_realtime_ns: r.read_i64()?,
            ..Location::default()
        };
        if l.has(HAS_ELAPSED_REALTIME_UNCERTAINTY) {
            l.elapsed_realtime_uncertainty_ns = read_f64(r)?;
        }
        l.latitude = read_f64(r)?;
        l.longitude = read_f64(r)?;
        if l.has(HAS_ALTITUDE) {
            l.altitude = read_f64(r)?;
        }
        if l.has(HAS_SPEED) {
            l.speed = r.read_f32()?;
        }
        if l.has(HAS_BEARING) {
            l.bearing = r.read_f32()?;
        }
        if l.has(HAS_HORIZONTAL_ACCURACY) {
            l.accuracy = r.read_f32()?;
        }
        if l.has(HAS_ALTITUDE_ACCURACY) {
            l.vertical_accuracy = r.read_f32()?;
        }
        if l.has(HAS_SPEED_ACCURACY) {
            l.speed_accuracy = r.read_f32()?;
        }
        if l.has(HAS_BEARING_ACCURACY) {
            l.bearing_accuracy = r.read_f32()?;
        }
        if l.has(HAS_MSL_ALTITUDE) {
            l.msl_altitude = read_f64(r)?;
        }
        if l.has(HAS_MSL_ALTITUDE_ACCURACY) {
            l.msl_altitude_accuracy = r.read_f32()?;
        }
        l.extras = RawBundle::read(r)?;
        Ok(l)
    }
}

impl WriteParcelable for Location {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(self.provider.as_deref());
        p.write_i32(self.fields);
        p.write_i64(self.time_ms);
        p.write_i64(self.elapsed_realtime_ns);
        if self.has(HAS_ELAPSED_REALTIME_UNCERTAINTY) {
            write_f64(p, self.elapsed_realtime_uncertainty_ns);
        }
        write_f64(p, self.latitude);
        write_f64(p, self.longitude);
        if self.has(HAS_ALTITUDE) {
            write_f64(p, self.altitude);
        }
        if self.has(HAS_SPEED) {
            p.write_f32(self.speed);
        }
        if self.has(HAS_BEARING) {
            p.write_f32(self.bearing);
        }
        if self.has(HAS_HORIZONTAL_ACCURACY) {
            p.write_f32(self.accuracy);
        }
        if self.has(HAS_ALTITUDE_ACCURACY) {
            p.write_f32(self.vertical_accuracy);
        }
        if self.has(HAS_SPEED_ACCURACY) {
            p.write_f32(self.speed_accuracy);
        }
        if self.has(HAS_BEARING_ACCURACY) {
            p.write_f32(self.bearing_accuracy);
        }
        if self.has(HAS_MSL_ALTITUDE) {
            write_f64(p, self.msl_altitude);
        }
        if self.has(HAS_MSL_ALTITUDE_ACCURACY) {
            p.write_f32(self.msl_altitude_accuracy);
        }
        RawBundle::write(self.extras.as_ref(), p);
    }
}

/// `Location.computeDistanceAndBearing`'s distance (the "Inverse Formula"
/// of the NGS's inverse.pdf, section 4, on WGS84).
pub fn distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f32 {
    let (lat1, lat2) = (lat1.to_radians(), lat2.to_radians());
    let (lon1, lon2) = (lon1.to_radians(), lon2.to_radians());
    let a = 6378137.0_f64;
    let b = 6356752.3142_f64;
    let f = (a - b) / a;
    let a_sq_minus_b_sq_over_b_sq = (a * a - b * b) / (b * b);
    let l = lon2 - lon1;
    let mut a_a = 0.0;
    let u1 = ((1.0 - f) * lat1.tan()).atan();
    let u2 = ((1.0 - f) * lat2.tan()).atan();
    let (cos_u1, cos_u2, sin_u1, sin_u2) = (u1.cos(), u2.cos(), u1.sin(), u2.sin());
    let cos_u1_cos_u2 = cos_u1 * cos_u2;
    let sin_u1_sin_u2 = sin_u1 * sin_u2;
    let mut sigma = 0.0;
    let mut delta_sigma = 0.0;
    let mut lambda = l;
    for _ in 0..20 {
        let lambda_orig = lambda;
        let (cos_lambda, sin_lambda) = (lambda.cos(), lambda.sin());
        let t1 = cos_u2 * sin_lambda;
        let t2 = cos_u1 * sin_u2 - sin_u1 * cos_u2 * cos_lambda;
        let sin_sigma = (t1 * t1 + t2 * t2).sqrt();
        let cos_sigma = sin_u1_sin_u2 + cos_u1_cos_u2 * cos_lambda;
        sigma = sin_sigma.atan2(cos_sigma);
        let sin_alpha = if sin_sigma == 0.0 {
            0.0
        } else {
            cos_u1_cos_u2 * sin_lambda / sin_sigma
        };
        let cos_sq_alpha = 1.0 - sin_alpha * sin_alpha;
        let cos2_sm = if cos_sq_alpha == 0.0 {
            0.0
        } else {
            cos_sigma - 2.0 * sin_u1_sin_u2 / cos_sq_alpha
        };
        let u_squared = cos_sq_alpha * a_sq_minus_b_sq_over_b_sq;
        a_a = 1.0
            + (u_squared / 16384.0)
                * (4096.0 + u_squared * (-768.0 + u_squared * (320.0 - 175.0 * u_squared)));
        let b_b = (u_squared / 1024.0)
            * (256.0 + u_squared * (-128.0 + u_squared * (74.0 - 47.0 * u_squared)));
        let c_c = (f / 16.0) * cos_sq_alpha * (4.0 + f * (4.0 - 3.0 * cos_sq_alpha));
        let cos2_sm_sq = cos2_sm * cos2_sm;
        delta_sigma = b_b
            * sin_sigma
            * (cos2_sm
                + (b_b / 4.0)
                    * (cos_sigma * (-1.0 + 2.0 * cos2_sm_sq)
                        - (b_b / 6.0)
                            * cos2_sm
                            * (-3.0 + 4.0 * sin_sigma * sin_sigma)
                            * (-3.0 + 4.0 * cos2_sm_sq)));
        lambda = l
            + (1.0 - c_c)
                * f
                * sin_alpha
                * (sigma
                    + c_c
                        * sin_sigma
                        * (cos2_sm + c_c * cos_sigma * (-1.0 + 2.0 * cos2_sm * cos2_sm)));
        let delta = (lambda - lambda_orig) / lambda;
        if delta.abs() < 1.0e-12 {
            break;
        }
    }
    (b * a_a * (sigma - delta_sigma)) as f32
}

/// `WorkSource`: sorted (uid, name) pairs, or uids without names, and
/// work chains.
#[derive(Clone, Debug, Default)]
pub struct WorkSource {
    pub uids: Vec<i32>,
    /// Present when the pairs have names.
    pub names: Option<Vec<Option<String>>>,
    pub chains: Option<Vec<WorkChain>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkChain {
    pub uids: Vec<i32>,
    pub tags: Vec<Option<String>>,
}

impl WorkSource {
    pub fn is_empty(&self) -> bool {
        self.uids.is_empty() && self.chains.as_ref().is_none_or(Vec::is_empty)
    }

    /// `add(uid, name)`: one named pair, in order. Adding a name to a
    /// source of bare uids throws in the original; the service only adds
    /// named pairs.
    pub fn add_named(&mut self, uid: i32, name: &str) {
        let names = self.names.get_or_insert_with(Vec::new);
        let mut i = 0;
        while i < self.uids.len() {
            if self.uids[i] > uid {
                break;
            }
            if self.uids[i] == uid {
                match names[i].as_deref().unwrap_or("").cmp(name) {
                    std::cmp::Ordering::Greater => break,
                    std::cmp::Ordering::Equal => return,
                    std::cmp::Ordering::Less => {}
                }
            }
            i += 1;
        }
        self.uids.insert(i, uid);
        names.insert(i, Some(name.to_string()));
    }

    /// `add(WorkSource)`: the other's pairs merged in order, and its
    /// chains not yet present.
    pub fn add(&mut self, other: &WorkSource) {
        for (i, &uid) in other.uids.iter().enumerate() {
            match other.names.as_ref() {
                Some(names) => self.add_named(uid, names[i].as_deref().unwrap_or("")),
                None if self.names.is_none() => {
                    if let Err(at) = self.uids.binary_search(&uid) {
                        self.uids.insert(at, uid);
                    }
                }
                None => {}
            }
        }
        if let Some(chains) = &other.chains {
            let mine = self.chains.get_or_insert_with(Vec::new);
            for chain in chains {
                if !mine.contains(chain) {
                    mine.push(chain.clone());
                }
            }
        }
    }

    /// The first pair's name, if the pairs have names.
    pub fn first_name(&self) -> Option<&str> {
        self.names.as_ref()?.first()?.as_deref()
    }
}

impl PartialEq for WorkSource {
    /// `WorkSource.equals`: `diff`, then the chains.
    fn eq(&self, o: &WorkSource) -> bool {
        if self.uids != o.uids {
            return false;
        }
        if let (Some(a), Some(b)) = (&self.names, &o.names)
            && a != b
        {
            return false;
        }
        match &self.chains {
            Some(c) if !c.is_empty() => o.chains.as_ref() == Some(c),
            _ => o.chains.as_ref().is_none_or(Vec::is_empty),
        }
    }
}

impl ReadParcelable for WorkSource {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let num = r.read_i32()?.max(0) as usize;
        let mut uids = read_int_array(r)?.unwrap_or_default();
        let mut names = read_string_list(r)?;
        uids.truncate(num);
        if let Some(n) = names.as_mut() {
            n.truncate(num);
        }
        let chains = match r.read_i32()? {
            n if n < 0 => None,
            n => {
                let mut chains = Vec::new();
                for _ in 0..n {
                    // `readParcelableList`: each a class name, then the chain.
                    if r.read_string16()?.is_none() {
                        continue;
                    }
                    let size = r.read_i32()?.max(0) as usize;
                    let mut uids = read_int_array(r)?.unwrap_or_default();
                    let mut tags = read_string_list(r)?.unwrap_or_default();
                    uids.truncate(size);
                    tags.truncate(size);
                    chains.push(WorkChain { uids, tags });
                }
                Some(chains)
            }
        };
        Ok(WorkSource {
            uids,
            names,
            chains,
        })
    }
}

impl WriteParcelable for WorkSource {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.uids.len() as i32);
        aim_service_aidl::write_int_array(p, Some(&self.uids));
        aim_service_aidl::write_string_list(p, self.names.as_deref());
        match &self.chains {
            None => p.write_i32(-1),
            Some(chains) => {
                p.write_i32(chains.len() as i32);
                for chain in chains {
                    p.write_string16(Some("android.os.WorkSource$WorkChain"));
                    p.write_i32(chain.uids.len() as i32);
                    aim_service_aidl::write_int_array(p, Some(&chain.uids));
                    aim_service_aidl::write_string_list(p, Some(&chain.tags));
                }
            }
        }
    }
}

/// `LocationRequest.QUALITY_*`.
pub const QUALITY_HIGH_ACCURACY: i32 = 100;
pub const QUALITY_LOW_POWER: i32 = 104;
/// `LocationRequest.PASSIVE_INTERVAL`.
pub const PASSIVE_INTERVAL: i64 = i64::MAX;
/// `LocationRequest.IMPLICIT_MIN_UPDATE_INTERVAL` and the factor legacy
/// requests get it by (`update_min_location_request_interval` is off in
/// the image).
const IMPLICIT_MIN_UPDATE_INTERVAL: i64 = -1;
const LEGACY_IMPLICIT_MIN_UPDATE_INTERVAL_FACTOR: f64 = 1.0 / 6.0;

/// `android.location.LocationRequest`, as its fields.
#[derive(Clone, Debug)]
pub struct LocationRequest {
    pub provider: Option<String>,
    pub interval_ms: i64,
    pub quality: i32,
    pub expire_at_realtime_ms: i64,
    pub duration_ms: i64,
    pub max_updates: i32,
    min_update_interval_ms: i64,
    pub min_update_distance_m: f32,
    pub max_update_delay_ms: i64,
    pub hidden_from_app_ops: bool,
    pub adas_gnss_bypass: bool,
    pub location_settings_ignored: bool,
    pub low_power: bool,
    pub work_source: WorkSource,
}

impl LocationRequest {
    /// `new LocationRequest.Builder(interval).build()`.
    pub fn new(interval_ms: i64) -> LocationRequest {
        LocationRequest {
            provider: None,
            interval_ms,
            quality: 102, // QUALITY_BALANCED_POWER_ACCURACY
            expire_at_realtime_ms: i64::MAX,
            duration_ms: i64::MAX,
            max_updates: i32::MAX,
            min_update_interval_ms: IMPLICIT_MIN_UPDATE_INTERVAL,
            min_update_distance_m: 0.0,
            max_update_delay_ms: 0,
            hidden_from_app_ops: false,
            adas_gnss_bypass: false,
            location_settings_ignored: false,
            low_power: false,
            work_source: WorkSource::default(),
        }
    }

    /// `getMinUpdateIntervalMillis`.
    pub fn min_update_interval_ms(&self) -> i64 {
        if self.min_update_interval_ms == IMPLICIT_MIN_UPDATE_INTERVAL {
            (self.interval_ms as f64 * LEGACY_IMPLICIT_MIN_UPDATE_INTERVAL_FACTOR) as i64
        } else {
            self.min_update_interval_ms.min(self.interval_ms)
        }
    }

    pub fn set_min_update_interval_ms(&mut self, ms: i64) {
        self.min_update_interval_ms = ms;
    }

    /// `isBypass`.
    pub fn is_bypass(&self) -> bool {
        self.adas_gnss_bypass || self.location_settings_ignored
    }

    /// `getExpirationRealtimeMs(start)`.
    pub fn expiration_realtime_ms(&self, start_ms: i64) -> i64 {
        let expiration = if self.duration_ms > i64::MAX - start_ms {
            i64::MAX
        } else {
            start_ms + self.duration_ms
        };
        expiration.min(self.expire_at_realtime_ms)
    }

    /// `new LocationRequest.Builder(request).build()`: the provider and
    /// the absolute expiration dropped, the minimum interval at most the
    /// interval, a legacy passive request's implicit minimum made
    /// explicit.
    pub fn rebuilt(&self) -> LocationRequest {
        let mut min = self.min_update_interval_ms;
        if self.interval_ms == PASSIVE_INTERVAL && min == IMPLICIT_MIN_UPDATE_INTERVAL {
            min = 10 * 60 * 1000;
        }
        LocationRequest {
            provider: None,
            expire_at_realtime_ms: i64::MAX,
            min_update_interval_ms: min.min(self.interval_ms),
            ..self.clone()
        }
    }
}

impl PartialEq for LocationRequest {
    fn eq(&self, o: &LocationRequest) -> bool {
        self.interval_ms == o.interval_ms
            && self.quality == o.quality
            && self.expire_at_realtime_ms == o.expire_at_realtime_ms
            && self.duration_ms == o.duration_ms
            && self.max_updates == o.max_updates
            && self.min_update_interval_ms == o.min_update_interval_ms
            && same_f32(self.min_update_distance_m, o.min_update_distance_m)
            && self.max_update_delay_ms == o.max_update_delay_ms
            && self.hidden_from_app_ops == o.hidden_from_app_ops
            && self.adas_gnss_bypass == o.adas_gnss_bypass
            && self.location_settings_ignored == o.location_settings_ignored
            && self.low_power == o.low_power
            && self.provider == o.provider
            && self.work_source == o.work_source
    }
}

impl ReadParcelable for LocationRequest {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(LocationRequest {
            provider: r.read_string16()?,
            interval_ms: r.read_i64()?,
            quality: r.read_i32()?,
            expire_at_realtime_ms: r.read_i64()?,
            duration_ms: r.read_i64()?,
            max_updates: r.read_i32()?,
            min_update_interval_ms: r.read_i64()?,
            min_update_distance_m: r.read_f32()?,
            max_update_delay_ms: r.read_i64()?,
            hidden_from_app_ops: r.read_bool()?,
            adas_gnss_bypass: r.read_bool()?,
            location_settings_ignored: r.read_bool()?,
            low_power: r.read_bool()?,
            // `Objects.requireNonNull` in the constructor.
            work_source: aim_service_aidl::read_typed(r)?.ok_or(BAD_VALUE)?,
        })
    }
}

impl std::fmt::Display for LocationRequest {
    /// `LocationRequest.toString`, as `dumpsys location` shows it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Request[")?;
        if self.interval_ms == PASSIVE_INTERVAL {
            write!(f, "PASSIVE")?;
        } else {
            let quality = match self.quality {
                QUALITY_HIGH_ACCURACY => "HIGH_ACCURACY",
                QUALITY_LOW_POWER => "LOW_POWER",
                _ => "BALANCED",
            };
            write!(f, "@{}ms {quality}", self.interval_ms)?;
        }
        if self.max_updates != i32::MAX {
            write!(f, ", maxUpdates={}", self.max_updates)?;
        }
        if self.hidden_from_app_ops {
            write!(f, ", hiddenFromAppOps")?;
        }
        if self.location_settings_ignored {
            write!(f, ", settingsBypass")?;
        }
        write!(f, "]")
    }
}

/// `android.location.LastLocationRequest`.
#[derive(Clone, Debug, Default)]
pub struct LastLocationRequest {
    pub hidden_from_app_ops: bool,
    pub adas_gnss_bypass: bool,
    pub location_settings_ignored: bool,
}

impl LastLocationRequest {
    pub fn is_bypass(&self) -> bool {
        self.adas_gnss_bypass || self.location_settings_ignored
    }
}

impl ReadParcelable for LastLocationRequest {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(LastLocationRequest {
            hidden_from_app_ops: r.read_bool()?,
            adas_gnss_bypass: r.read_bool()?,
            location_settings_ignored: r.read_bool()?,
        })
    }
}

/// `ProviderRequest.INTERVAL_DISABLED`.
pub const INTERVAL_DISABLED: i64 = i64::MAX;

/// `android.location.provider.ProviderRequest`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderRequest {
    pub interval_ms: i64,
    pub quality: i32,
    pub max_update_delay_ms: i64,
    pub low_power: bool,
    pub adas_gnss_bypass: bool,
    pub location_settings_ignored: bool,
    pub work_source: WorkSource,
}

impl ProviderRequest {
    /// `EMPTY_REQUEST`.
    pub fn empty() -> ProviderRequest {
        ProviderRequest {
            interval_ms: INTERVAL_DISABLED,
            quality: QUALITY_LOW_POWER,
            max_update_delay_ms: 0,
            low_power: false,
            adas_gnss_bypass: false,
            location_settings_ignored: false,
            work_source: WorkSource::default(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.interval_ms != INTERVAL_DISABLED
    }

    pub fn is_bypass(&self) -> bool {
        self.adas_gnss_bypass || self.location_settings_ignored
    }
}

impl WriteParcelable for ProviderRequest {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.interval_ms);
        if self.interval_ms != INTERVAL_DISABLED {
            p.write_i32(self.quality);
            p.write_i64(self.max_update_delay_ms);
            p.write_bool(self.low_power);
            p.write_bool(self.adas_gnss_bypass);
            p.write_bool(self.location_settings_ignored);
            aim_service_aidl::write_typed(p, Some(&self.work_source));
        }
    }
}

impl std::fmt::Display for ProviderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.is_active() {
            return write!(f, "ProviderRequest[OFF]");
        }
        write!(f, "ProviderRequest[@{}ms", self.interval_ms)?;
        let quality = match self.quality {
            QUALITY_HIGH_ACCURACY => "HIGH_ACCURACY",
            QUALITY_LOW_POWER => "LOW_POWER",
            _ => "BALANCED",
        };
        write!(f, ", {quality}]")
    }
}

/// `ProviderProperties.POWER_USAGE_*` and `ACCURACY_*`.
pub const POWER_USAGE_LOW: i32 = 1;
pub const POWER_USAGE_HIGH: i32 = 3;
pub const ACCURACY_FINE: i32 = 1;
pub const ACCURACY_COARSE: i32 = 2;

/// `android.location.provider.ProviderProperties`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderProperties {
    pub has_network_requirement: bool,
    pub has_satellite_requirement: bool,
    pub has_cell_requirement: bool,
    pub has_monetary_cost: bool,
    pub has_altitude_support: bool,
    pub has_speed_support: bool,
    pub has_bearing_support: bool,
    pub power_usage: i32,
    pub accuracy: i32,
}

impl ProviderProperties {
    /// `new ProviderProperties.Builder()`'s defaults, with a power usage
    /// and an accuracy.
    pub const fn with(power_usage: i32, accuracy: i32) -> ProviderProperties {
        ProviderProperties {
            has_network_requirement: false,
            has_satellite_requirement: false,
            has_cell_requirement: false,
            has_monetary_cost: false,
            has_altitude_support: false,
            has_speed_support: false,
            has_bearing_support: false,
            power_usage,
            accuracy,
        }
    }
}

impl ReadParcelable for ProviderProperties {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(ProviderProperties {
            has_network_requirement: r.read_bool()?,
            has_satellite_requirement: r.read_bool()?,
            has_cell_requirement: r.read_bool()?,
            has_monetary_cost: r.read_bool()?,
            has_altitude_support: r.read_bool()?,
            has_speed_support: r.read_bool()?,
            has_bearing_support: r.read_bool()?,
            power_usage: r.read_i32()?,
            accuracy: r.read_i32()?,
        })
    }
}

impl WriteParcelable for ProviderProperties {
    fn write_to(&self, p: &mut Parcel) {
        p.write_bool(self.has_network_requirement);
        p.write_bool(self.has_satellite_requirement);
        p.write_bool(self.has_cell_requirement);
        p.write_bool(self.has_monetary_cost);
        p.write_bool(self.has_altitude_support);
        p.write_bool(self.has_speed_support);
        p.write_bool(self.has_bearing_support);
        p.write_i32(self.power_usage);
        p.write_i32(self.accuracy);
    }
}

/// `android.location.Criteria`.
#[derive(Clone, Debug)]
pub struct Criteria {
    pub horizontal_accuracy: i32,
    pub vertical_accuracy: i32,
    pub speed_accuracy: i32,
    pub bearing_accuracy: i32,
    pub power_requirement: i32,
    pub altitude_required: bool,
    pub bearing_required: bool,
    pub speed_required: bool,
    pub cost_allowed: bool,
}

/// `Criteria.NO_REQUIREMENT`.
const CRITERIA_NO_REQUIREMENT: i32 = 0;

impl Criteria {
    /// `LocationProvider.propertiesMeetCriteria`.
    pub fn met_by(&self, name: &str, properties: Option<&ProviderProperties>) -> bool {
        if name == "passive" {
            // passive provider never matches
            return false;
        }
        let Some(properties) = properties else {
            // unfortunately this can happen for provider in remote services
            // that have not finished binding yet
            return false;
        };
        if self.horizontal_accuracy != CRITERIA_NO_REQUIREMENT
            && self.horizontal_accuracy < properties.accuracy
        {
            return false;
        }
        if self.power_requirement != CRITERIA_NO_REQUIREMENT
            && self.power_requirement < properties.power_usage
        {
            return false;
        }
        if self.altitude_required && !properties.has_altitude_support {
            return false;
        }
        if self.speed_required && !properties.has_speed_support {
            return false;
        }
        if self.bearing_required && !properties.has_bearing_support {
            return false;
        }
        if !self.cost_allowed && properties.has_monetary_cost {
            return false;
        }
        true
    }
}

impl ReadParcelable for Criteria {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Criteria {
            horizontal_accuracy: r.read_i32()?,
            vertical_accuracy: r.read_i32()?,
            speed_accuracy: r.read_i32()?,
            bearing_accuracy: r.read_i32()?,
            power_requirement: r.read_i32()?,
            altitude_required: r.read_i32()? != 0,
            bearing_required: r.read_i32()? != 0,
            speed_required: r.read_i32()? != 0,
            cost_allowed: r.read_i32()? != 0,
        })
    }
}

/// `android.os.PackageTagsList`: packages, each with its attribution
/// tags or with every tag (an empty set).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackageTagsList(pub BTreeMap<String, std::collections::BTreeSet<Option<String>>>);

impl PackageTagsList {
    pub fn contains(&self, package: &str, tag: Option<&str>) -> bool {
        match self.0.get(package) {
            None => false,
            Some(tags) => tags.is_empty() || tags.contains(&tag.map(String::from)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `Builder.add(package)`: every tag of it.
    pub fn add_all(&mut self, package: &str) {
        self.0.entry(package.into()).or_default().clear();
    }

    /// `Builder.add(package, tag)`.
    pub fn add(&mut self, package: &str, tag: Option<&str>) {
        match self.0.get_mut(package) {
            None => {
                self.0
                    .insert(package.into(), [tag.map(String::from)].into());
            }
            Some(tags) if !tags.is_empty() => {
                tags.insert(tag.map(String::from));
            }
            Some(_) => {}
        }
    }

    /// `Builder.add(PackageTagsList)`.
    pub fn add_list(&mut self, other: &PackageTagsList) {
        for (package, tags) in &other.0 {
            if tags.is_empty() {
                self.add_all(package);
            } else {
                for tag in tags {
                    self.add(package, tag.as_deref());
                }
            }
        }
    }
}

impl WriteParcelable for PackageTagsList {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0.len() as i32);
        for (package, tags) in &self.0 {
            p.write_string8(Some(package));
            // `writeArraySet`: its size, then each as a value.
            p.write_i32(tags.len() as i32);
            for tag in tags {
                match tag {
                    Some(tag) => {
                        p.write_i32(0); // VAL_STRING
                        p.write_string16(Some(tag));
                    }
                    None => p.write_i32(-1), // VAL_NULL
                }
            }
        }
    }
}

/// `android.location.LocationTime`.
pub struct LocationTime {
    pub unix_epoch_time_ms: i64,
    pub elapsed_realtime_ns: i64,
}

impl WriteParcelable for LocationTime {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i64(self.unix_epoch_time_ms);
        p.write_i64(self.elapsed_realtime_ns);
    }
}

/// `android.location.GnssCapabilities`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GnssCapabilities {
    pub top_flags: i32,
    pub is_adr_capability_known: bool,
    pub measurement_corrections_flags: i32,
    pub power_flags: i32,
}

impl WriteParcelable for GnssCapabilities {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.top_flags);
        p.write_bool(self.is_adr_capability_known);
        p.write_i32(self.measurement_corrections_flags);
        p.write_i32(self.power_flags);
        p.write_i32(0); // no signal types
    }
}

/// `android.location.Geofence`.
#[derive(Clone, Debug)]
pub struct Geofence {
    pub latitude: f64,
    pub longitude: f64,
    pub radius: f32,
    pub expiration_realtime_ms: i64,
}

impl PartialEq for Geofence {
    fn eq(&self, o: &Geofence) -> bool {
        same_f64(self.latitude, o.latitude)
            && same_f64(self.longitude, o.longitude)
            && same_f32(self.radius, o.radius)
            && self.expiration_realtime_ms == o.expiration_realtime_ms
    }
}

impl ReadParcelable for Geofence {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Geofence {
            latitude: read_f64(r)?,
            longitude: read_f64(r)?,
            radius: r.read_f32()?,
            expiration_realtime_ms: r.read_i64()?,
        })
    }
}

/// `android.location.GnssMeasurementRequest`, up to what the service
/// decides by.
pub struct GnssMeasurementRequest {
    pub correlation_vector_outputs_enabled: bool,
}

impl ReadParcelable for GnssMeasurementRequest {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        r.read_bool()?; // full tracking
        let correlation_vector_outputs_enabled = r.read_bool()?;
        r.read_i32()?; // interval
        aim_service_aidl::read_typed::<WorkSource>(r)?;
        Ok(GnssMeasurementRequest {
            correlation_vector_outputs_enabled,
        })
    }
}

/// `ReverseGeocodeRequest` or `ForwardGeocodeRequest`: the caller it
/// names, and its bytes, which go on to the geocode provider unchanged.
pub struct GeocodeRequest {
    pub calling_uid: i32,
    pub calling_package: String,
    pub calling_attribution_tag: Option<String>,
    bytes: Vec<u8>,
}

impl GeocodeRequest {
    /// Reads the request's end (its locale and caller) after `start`.
    fn finish(r: &mut Reader<'_>, start: usize) -> Result<GeocodeRequest> {
        for _ in 0..3 {
            r.read_string8()?; // the locale's language, country and variant
        }
        let calling_uid = r.read_i32()?;
        let calling_package = r.read_string8()?.ok_or(BAD_VALUE)?;
        let calling_attribution_tag = r.read_string8()?;
        let (bytes, objects) = r.since(start);
        if !objects.is_empty() {
            return Err(BAD_VALUE);
        }
        Ok(GeocodeRequest {
            calling_uid,
            calling_package,
            calling_attribution_tag,
            bytes: bytes.to_vec(),
        })
    }
}

impl WriteParcelable for GeocodeRequest {
    fn write_to(&self, p: &mut Parcel) {
        p.write_raw(&self.bytes, &[]);
    }
}

pub struct ReverseGeocodeRequest(pub GeocodeRequest);

impl ReadParcelable for ReverseGeocodeRequest {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.position();
        read_f64(r)?;
        read_f64(r)?;
        r.read_i32()?; // max results
        GeocodeRequest::finish(r, start).map(ReverseGeocodeRequest)
    }
}

pub struct ForwardGeocodeRequest(pub GeocodeRequest);

impl ReadParcelable for ForwardGeocodeRequest {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.position();
        r.read_string8()?.ok_or(BAD_VALUE)?; // location name
        for _ in 0..4 {
            read_f64(r)?; // the bounds
        }
        r.read_i32()?; // max results
        GeocodeRequest::finish(r, start).map(ForwardGeocodeRequest)
    }
}

/// `PendingIntent`: its `IIntentSender`.
pub struct PendingIntent(pub Binder);

impl ReadParcelable for PendingIntent {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        r.read_binder()?.map(PendingIntent).ok_or(BAD_VALUE)
    }
}

/// A parcelable whose content the service does not use: its presence
/// only.
pub struct Unread;

impl ReadParcelable for Unread {
    fn read_from(_: &mut Reader<'_>) -> Result<Self> {
        Ok(Unread)
    }
}

/// `Parcel.VAL_*` of the values the service puts in bundles.
pub const VAL_NULL: i32 = -1;
pub const VAL_STRING: i32 = 0;
pub const VAL_INTEGER: i32 = 1;
pub const VAL_PARCELABLE: i32 = 4;
pub const VAL_LONG: i32 = 6;
pub const VAL_BOOLEAN: i32 = 9;
pub const VAL_PARCELABLEARRAY: i32 = 16;

/// A value the service writes into a bundle.
pub enum Value<'a> {
    Bool(bool),
    Int(i32),
    Long(i64),
    String(&'a str),
    /// A parcelable: its class name, then its fields.
    Parcelable(&'a str, &'a dyn WriteParcelable),
    /// An array of parcelables of one class.
    Parcelables(&'a str, Vec<&'a dyn WriteParcelable>),
}

impl Value<'_> {
    /// `Parcel.writeValue`: the type, then the value; a parcelable (array)
    /// is length-prefixed.
    fn write(&self, p: &mut Parcel) {
        let prefixed = |p: &mut Parcel, kind: i32, body: &dyn Fn(&mut Parcel)| {
            p.write_i32(kind);
            let at = p.position();
            p.write_i32(-1);
            let start = p.position();
            body(p);
            p.set_i32_at(at, (p.position() - start) as i32);
        };
        match self {
            Value::Bool(v) => {
                p.write_i32(VAL_BOOLEAN);
                p.write_bool(*v);
            }
            Value::Int(v) => {
                p.write_i32(VAL_INTEGER);
                p.write_i32(*v);
            }
            Value::Long(v) => {
                p.write_i32(VAL_LONG);
                p.write_i64(*v);
            }
            Value::String(v) => {
                p.write_i32(VAL_STRING);
                p.write_string16(Some(v));
            }
            Value::Parcelable(class, v) => prefixed(p, VAL_PARCELABLE, &|p| {
                p.write_string16(Some(class));
                v.write_to(p);
            }),
            Value::Parcelables(class, vs) => prefixed(p, VAL_PARCELABLEARRAY, &|p| {
                p.write_i32(vs.len() as i32);
                for v in vs {
                    p.write_string16(Some(class));
                    v.write_to(p);
                }
            }),
        }
    }
}

/// `writeBundle` of a bundle of `entries`.
pub fn write_bundle(p: &mut Parcel, entries: &[(&str, Value<'_>)]) {
    if entries.is_empty() {
        return p.write_i32(0);
    }
    let at = p.position();
    p.write_i32(-1);
    p.write_i32(crate::bundle::MAGIC);
    let start = p.position();
    p.write_i32(entries.len() as i32);
    for (key, value) in entries {
        p.write_string16(Some(key));
        value.write(p);
    }
    p.set_i32_at(at, (p.position() - start) as i32);
    p.write_bool(false); // has an intent
}

/// `Intent.FLAG_RECEIVER_REGISTERED_ONLY` and `FLAG_RECEIVER_FOREGROUND`.
pub const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
pub const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;

/// An `Intent` with an action, flags and extras, as `Intent.writeToParcel`
/// writes it.
pub struct Intent<'a> {
    pub action: Option<&'a str>,
    pub flags: i32,
    pub extras: Vec<(&'a str, Value<'a>)>,
}

impl WriteParcelable for &Intent<'_> {
    fn write_to(&self, p: &mut Parcel) {
        (*self).write_to(p)
    }
}

impl WriteParcelable for Intent<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(self.action);
        p.write_i32(0); // data: Uri.NULL_TYPE_ID
        p.write_string8(None); // type
        p.write_string8(None); // identifier
        p.write_i32(self.flags);
        p.write_i32(0); // extended flags
        p.write_string8(None); // package
        p.write_string16(None); // component
        p.write_i32(0); // source bounds
        p.write_i32(0); // categories
        p.write_i32(0); // selector
        p.write_i32(0); // clip data
        p.write_i32(-2); // content user hint: UserHandle.USER_CURRENT
        write_bundle(p, &self.extras);
        p.write_i32(0); // original intent
        p.write_i32(0); // creator token (prevent_intent_redirect is on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: ReadParcelable + WriteParcelable>(v: &T) -> T {
        let mut p = Parcel::new();
        v.write_to(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        let back = T::read_from(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        back
    }

    #[test]
    fn locations_round_trip() {
        let l = Location {
            provider: Some("gps".into()),
            fields: HAS_HORIZONTAL_ACCURACY | HAS_ALTITUDE | HAS_SPEED | HAS_MOCK_PROVIDER,
            time_ms: 1_700_000_000_000,
            elapsed_realtime_ns: 5_000_000_000,
            latitude: 37.5,
            longitude: 127.0,
            altitude: 30.0,
            speed: 1.5,
            accuracy: 10.0,
            ..Location::default()
        };
        assert_eq!(round_trip(&l), l);
    }

    #[test]
    fn distance_matches_java() {
        // Location.distanceBetween(0, 0, 0, 1): 111319.49 m on WGS84.
        let d = distance(0.0, 0.0, 0.0, 1.0);
        assert!((d - 111_319.49).abs() < 0.1, "{d}");
        assert_eq!(distance(10.0, 20.0, 10.0, 20.0), 0.0);
    }

    #[test]
    fn work_sources_merge_in_order() {
        let mut w = WorkSource::default();
        w.add_named(10_050, "b");
        w.add_named(10_010, "a");
        w.add_named(10_050, "a");
        w.add_named(10_050, "b");
        assert_eq!(w.uids, [10_010, 10_050, 10_050]);
        let names: Vec<_> = w.names.clone().unwrap().into_iter().flatten().collect();
        assert_eq!(names, ["a", "a", "b"]);
        assert_eq!(round_trip(&w), w);
    }

    #[test]
    fn package_tags() {
        let mut l = PackageTagsList::default();
        l.add("p", Some("t"));
        assert!(l.contains("p", Some("t")));
        assert!(!l.contains("p", None));
        l.add_all("p");
        assert!(l.contains("p", Some("x")));
        l.add("p", Some("y"));
        assert!(l.0["p"].is_empty());
    }

    #[test]
    fn validates_as_the_original() {
        let mut l = Location {
            provider: Some("gps".into()),
            fields: HAS_HORIZONTAL_ACCURACY,
            accuracy: 5.0,
            elapsed_realtime_ns: 10,
            latitude: 1.0,
            ..Location::default()
        };
        assert!(l.validate(0, 20).is_ok());
        assert!(l.validate(11, 20).is_err());
        assert!(l.validate(0, 5).is_err());
        l.latitude = 0.0;
        assert!(l.validate(0, 20).is_err());
        l.fields |= HAS_MOCK_PROVIDER;
        assert!(l.validate(0, 20).is_ok());
    }
}
