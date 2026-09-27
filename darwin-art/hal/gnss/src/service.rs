//! `IGnss` over CoreLocation ([`darwin_hostcall::location`]). The Mac has
//! no GNSS receiver: the HAL is a location source that reports CoreLocation
//! fixes (Wi-Fi positioning, or a paired iPhone's GNSS) as `GnssLocation`s,
//! with CoreLocation's accuracies. It has no satellites, NMEA, measurements
//! or navigation messages, so the extensions are absent and SV status and
//! NMEA stay silent.
//!
//! Fixes are polled from the host at the requested interval (at least
//! [`MIN_INTERVAL`]), and each new one is delivered once.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use android_hardware_gnss::aidl::android::hardware::gnss::{
    ElapsedRealtime::{ElapsedRealtime, HAS_TIME_UNCERTAINTY_NS, HAS_TIMESTAMP_NS},
    GnssLocation::{
        GnssLocation, HAS_ALTITUDE, HAS_BEARING, HAS_BEARING_ACCURACY, HAS_HORIZONTAL_ACCURACY,
        HAS_LAT_LONG, HAS_SPEED, HAS_SPEED_ACCURACY, HAS_VERTICAL_ACCURACY,
    },
    IAGnss::IAGnss,
    IAGnssRil::IAGnssRil,
    IGnss::{
        GnssAidingData::GnssAidingData, GnssPositionRecurrence::GnssPositionRecurrence, IGnss,
        PositionModeOptions::PositionModeOptions,
    },
    IGnssAntennaInfo::IGnssAntennaInfo,
    IGnssBatching::IGnssBatching,
    IGnssCallback::{
        CAPABILITY_SCHEDULING, GnssStatusValue::GnssStatusValue, GnssSystemInfo::GnssSystemInfo,
        IGnssCallback,
    },
    IGnssConfiguration::IGnssConfiguration,
    IGnssDebug::IGnssDebug,
    IGnssGeofence::IGnssGeofence,
    IGnssMeasurementInterface::IGnssMeasurementInterface,
    IGnssNavigationMessageInterface::IGnssNavigationMessageInterface,
    IGnssPowerIndication::IGnssPowerIndication,
    IGnssPsds::IGnssPsds,
    measurement_corrections::IMeasurementCorrectionsInterface::IMeasurementCorrectionsInterface,
    visibility_control::IGnssVisibilityControl::IGnssVisibilityControl,
};
use binder::{ExceptionCode, Interface, Status, Strong};
use darwin_hostcall::guest;
use darwin_hostcall::location::{Fix, authorization};

/// The shortest interval between fixes; CoreLocation updates about once a
/// second at best.
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);

fn unsupported<T>() -> binder::Result<T> {
    Err(Status::new_exception(
        ExceptionCode::UNSUPPORTED_OPERATION,
        None,
    ))
}

fn boottime_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid clock and output.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// A host fix as a `GnssLocation`, `now_ns` being the current
/// elapsed-realtime clock. `None` when the fix has no valid position.
pub fn to_location(f: &Fix, now_ns: i64) -> Option<GnssLocation> {
    if f.valid == 0 || f.horizontal_accuracy < 0.0 {
        return None;
    }
    let mut flags = HAS_LAT_LONG | HAS_HORIZONTAL_ACCURACY;
    // CoreLocation marks an invalid value with a negative accuracy (or a
    // negative value, for speed and course).
    if f.vertical_accuracy > 0.0 {
        flags |= HAS_ALTITUDE | HAS_VERTICAL_ACCURACY;
    }
    if f.speed >= 0.0 {
        flags |= HAS_SPEED;
        if f.speed_accuracy >= 0.0 {
            flags |= HAS_SPEED_ACCURACY;
        }
    }
    if f.course >= 0.0 {
        flags |= HAS_BEARING;
        if f.course_accuracy >= 0.0 {
            flags |= HAS_BEARING_ACCURACY;
        }
    }
    let has = |bit: i32, v: f64| if flags & bit != 0 { v } else { 0.0 };
    Some(GnssLocation {
        gnssLocationFlags: flags,
        latitudeDegrees: f.latitude,
        longitudeDegrees: f.longitude,
        altitudeMeters: has(HAS_ALTITUDE, f.altitude),
        speedMetersPerSec: has(HAS_SPEED, f.speed),
        bearingDegrees: has(HAS_BEARING, f.course),
        horizontalAccuracyMeters: f.horizontal_accuracy,
        verticalAccuracyMeters: has(HAS_VERTICAL_ACCURACY, f.vertical_accuracy),
        speedAccuracyMetersPerSecond: has(HAS_SPEED_ACCURACY, f.speed_accuracy),
        bearingAccuracyDegrees: has(HAS_BEARING_ACCURACY, f.course_accuracy),
        timestampMillis: f.unix_ms,
        elapsedRealtime: ElapsedRealtime {
            flags: HAS_TIMESTAMP_NS | HAS_TIME_UNCERTAINTY_NS,
            timestampNs: now_ns.saturating_sub(f.age_ns as i64),
            // The host keeps fix times in milliseconds.
            timeUncertaintyNs: 1_000_000.0,
        },
    })
}

#[derive(Default)]
struct State {
    callback: Option<Strong<dyn IGnssCallback>>,
    running: bool,
    interval: Duration,
    single: bool,
    /// The time of the last fix delivered.
    last_ms: i64,
    /// The host authorization last seen, to log changes.
    authorization: Option<u32>,
}

#[derive(Clone, Default)]
pub struct Gnss {
    state: Arc<(Mutex<State>, Condvar)>,
}

impl Interface for Gnss {}

impl Gnss {
    /// Delivers new host fixes while a session runs; never returns.
    pub fn run(&self) {
        let (lock, cv) = &*self.state;
        let mut state = lock.lock().unwrap();
        loop {
            while !state.running {
                state = cv.wait(state).unwrap();
            }
            let fix = guest::location();
            if let Ok(f) = &fix
                && state.authorization != Some(f.authorization)
            {
                state.authorization = Some(f.authorization);
                log_authorization(f.authorization);
            }
            let location = fix.ok().and_then(|f| to_location(&f, boottime_ns()));
            if let Some(l) = location.filter(|l| l.timestampMillis > state.last_ms) {
                state.last_ms = l.timestampMillis;
                if let Some(cb) = &state.callback {
                    let _ = cb.gnssLocationCb(&l);
                }
                if state.single {
                    self.stop_locked(&mut state);
                }
            }
            let interval = state.interval.max(MIN_INTERVAL);
            state = cv.wait_timeout(state, interval).unwrap().0;
        }
    }

    fn status(state: &State, status: GnssStatusValue) {
        if let Some(cb) = &state.callback {
            let _ = cb.gnssStatusCb(status);
        }
    }

    fn stop_locked(&self, state: &mut State) {
        if !state.running {
            return;
        }
        state.running = false;
        if let Err(e) = guest::location_updates(false) {
            log::warn!("host call location.stop failed: errno {}", e.0);
        }
        Self::status(state, GnssStatusValue::SESSION_END);
        Self::status(state, GnssStatusValue::ENGINE_OFF);
    }
}

fn log_authorization(a: u32) {
    match a {
        authorization::AUTHORIZED => log::info!("CoreLocation access authorized"),
        authorization::NOT_DETERMINED => log::info!("CoreLocation access not yet answered"),
        authorization::SERVICES_OFF => log::warn!("Location Services are off on the Mac"),
        _ => log::warn!("CoreLocation access denied or restricted ({a})"),
    }
}

impl IGnss for Gnss {
    fn setCallback(&self, callback: &Strong<dyn IGnssCallback>) -> binder::Result<()> {
        let _ = callback.gnssSetCapabilitiesCb(CAPABILITY_SCHEDULING);
        let _ = callback.gnssSetSystemInfoCb(&GnssSystemInfo {
            // The model year of the Mac's positioning is unknown.
            yearOfHw: 0,
            name: "darwin CoreLocation".into(),
        });
        self.state.0.lock().unwrap().callback = Some(callback.clone());
        Ok(())
    }

    fn close(&self) -> binder::Result<()> {
        let mut state = self.state.0.lock().unwrap();
        self.stop_locked(&mut state);
        state.callback = None;
        Ok(())
    }

    fn getExtensionPsds(&self) -> binder::Result<Option<Strong<dyn IGnssPsds>>> {
        Ok(None)
    }

    fn getExtensionGnssConfiguration(&self) -> binder::Result<Strong<dyn IGnssConfiguration>> {
        unsupported()
    }

    fn getExtensionGnssMeasurement(&self) -> binder::Result<Strong<dyn IGnssMeasurementInterface>> {
        unsupported()
    }

    fn getExtensionGnssPowerIndication(&self) -> binder::Result<Strong<dyn IGnssPowerIndication>> {
        unsupported()
    }

    fn getExtensionGnssBatching(&self) -> binder::Result<Option<Strong<dyn IGnssBatching>>> {
        Ok(None)
    }

    fn getExtensionGnssGeofence(&self) -> binder::Result<Option<Strong<dyn IGnssGeofence>>> {
        Ok(None)
    }

    fn getExtensionGnssNavigationMessage(
        &self,
    ) -> binder::Result<Option<Strong<dyn IGnssNavigationMessageInterface>>> {
        Ok(None)
    }

    fn getExtensionAGnss(&self) -> binder::Result<Strong<dyn IAGnss>> {
        unsupported()
    }

    fn getExtensionAGnssRil(&self) -> binder::Result<Strong<dyn IAGnssRil>> {
        unsupported()
    }

    fn getExtensionGnssDebug(&self) -> binder::Result<Strong<dyn IGnssDebug>> {
        unsupported()
    }

    fn getExtensionGnssVisibilityControl(
        &self,
    ) -> binder::Result<Strong<dyn IGnssVisibilityControl>> {
        unsupported()
    }

    fn start(&self) -> binder::Result<()> {
        let (lock, cv) = &*self.state;
        let mut state = lock.lock().unwrap();
        if state.running {
            return Ok(());
        }
        guest::location_updates(true).map_err(|e| {
            log::error!("host call location.start failed: errno {}", e.0);
            Status::new_exception(ExceptionCode::ILLEGAL_STATE, None)
        })?;
        state.running = true;
        Self::status(&state, GnssStatusValue::ENGINE_ON);
        Self::status(&state, GnssStatusValue::SESSION_BEGIN);
        cv.notify_all();
        Ok(())
    }

    fn stop(&self) -> binder::Result<()> {
        let mut state = self.state.0.lock().unwrap();
        self.stop_locked(&mut state);
        Ok(())
    }

    // Time and location injection feed a receiver's assistance data; there
    // is no receiver.
    fn injectTime(
        &self,
        _time_ms: i64,
        _reference_ms: i64,
        _uncertainty_ms: i32,
    ) -> binder::Result<()> {
        Ok(())
    }

    fn injectLocation(&self, _location: &GnssLocation) -> binder::Result<()> {
        Ok(())
    }

    fn injectBestLocation(&self, _location: &GnssLocation) -> binder::Result<()> {
        Ok(())
    }

    fn deleteAidingData(&self, _flags: GnssAidingData) -> binder::Result<()> {
        Ok(())
    }

    fn setPositionMode(&self, options: &PositionModeOptions) -> binder::Result<()> {
        let mut state = self.state.0.lock().unwrap();
        state.interval = Duration::from_millis(options.minIntervalMs.max(0) as u64);
        state.single = options.recurrence == GnssPositionRecurrence::RECURRENCE_SINGLE;
        Ok(())
    }

    fn getExtensionGnssAntennaInfo(&self) -> binder::Result<Strong<dyn IGnssAntennaInfo>> {
        unsupported()
    }

    fn getExtensionMeasurementCorrections(
        &self,
    ) -> binder::Result<Option<Strong<dyn IMeasurementCorrectionsInterface>>> {
        Ok(None)
    }

    fn startSvStatus(&self) -> binder::Result<()> {
        Ok(())
    }

    fn stopSvStatus(&self) -> binder::Result<()> {
        Ok(())
    }

    fn startNmea(&self) -> binder::Result<()> {
        Ok(())
    }

    fn stopNmea(&self) -> binder::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> Fix {
        Fix {
            authorization: authorization::AUTHORIZED,
            valid: 1,
            latitude: 37.5665,
            longitude: 126.978,
            altitude: 38.0,
            horizontal_accuracy: 35.0,
            vertical_accuracy: -1.0,
            speed: -1.0,
            speed_accuracy: -1.0,
            course: 90.0,
            course_accuracy: -1.0,
            age_ns: 2_000_000_000,
            unix_ms: 1_790_000_000_000,
        }
    }

    #[test]
    fn only_valid_values_are_flagged() {
        let l = to_location(&fix(), 10_000_000_000).unwrap();
        assert_eq!(
            l.gnssLocationFlags,
            HAS_LAT_LONG | HAS_HORIZONTAL_ACCURACY | HAS_BEARING
        );
        assert_eq!(l.altitudeMeters, 0.0);
        assert_eq!(l.bearingDegrees, 90.0);
        assert_eq!(l.horizontalAccuracyMeters, 35.0);
        assert_eq!(l.timestampMillis, 1_790_000_000_000);
        assert_eq!(l.elapsedRealtime.timestampNs, 8_000_000_000);
    }

    #[test]
    fn a_fix_without_a_position_is_dropped() {
        assert!(to_location(&Fix::default(), 0).is_none());
        let mut f = fix();
        f.horizontal_accuracy = -1.0;
        assert!(to_location(&f, 0).is_none());
    }
}
