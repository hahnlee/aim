//! `IGnss` over CoreLocation ([`aim_hostcall::location`]). The Mac has
//! no GNSS receiver: the HAL is a location source that reports CoreLocation
//! fixes (Wi-Fi positioning, or a paired iPhone's GNSS) as `GnssLocation`s,
//! with CoreLocation's accuracies. It has no satellites, NMEA, measurements
//! or navigation messages, so the extensions are absent and SV status and
//! NMEA stay silent.
//!
//! Fixes are polled from the host at the requested interval (at least
//! [`MIN_INTERVAL`]), and each new one is delivered once.
//!
//! As a receiver's chip reports asynchronously, every callback of a session
//! (status and location) is delivered by the [`Reporter`]'s own thread,
//! after the call that caused it has returned and with no lock of the HAL
//! held. The framework calls `start` and `stop` holding its own locks, and
//! its callbacks take them; a callback inside such a call re-enters
//! system_server on the calling thread and deadlocks (#459).

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use aim_hostcall::guest;
use aim_hostcall::location::{Fix, authorization};
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

/// A callback of a session, queued for the [`Reporter`].
#[derive(Debug)]
pub enum Report {
    Status(GnssStatusValue),
    Location(GnssLocation),
}

#[derive(Default)]
struct Reports {
    callback: Option<Strong<dyn IGnssCallback>>,
    queue: VecDeque<Report>,
}

/// Delivers [`Report`]s in order to the framework's callback, from its own
/// thread.
#[derive(Clone, Default)]
pub struct Reporter(Arc<(Mutex<Reports>, Condvar)>);

impl Reporter {
    /// Replaces the callback; reports still queued go to the new one, or
    /// nowhere.
    fn set_callback(&self, callback: Option<Strong<dyn IGnssCallback>>) {
        self.0.0.lock().unwrap().callback = callback;
    }

    pub fn post(&self, report: Report) {
        self.0.0.lock().unwrap().queue.push_back(report);
        self.0.1.notify_one();
    }

    /// Delivers the queued reports; never returns.
    pub fn run(&self) {
        let (lock, cv) = &*self.0;
        loop {
            let mut reports = lock.lock().unwrap();
            let report = loop {
                match reports.queue.pop_front() {
                    Some(r) => break r,
                    None => reports = cv.wait(reports).unwrap(),
                }
            };
            let Some(cb) = reports.callback.clone() else {
                continue;
            };
            drop(reports);
            let delivered = match &report {
                Report::Status(s) => cb.gnssStatusCb(*s),
                Report::Location(l) => cb.gnssLocationCb(l),
            };
            if let Err(e) = delivered {
                log::warn!("cannot deliver {report:?}: {e}");
            }
        }
    }
}

#[derive(Default)]
struct State {
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
    reporter: Reporter,
}

impl Interface for Gnss {}

impl Gnss {
    pub fn reporter(&self) -> Reporter {
        self.reporter.clone()
    }

    /// Reports new host fixes while a session runs; never returns.
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
                self.reporter.post(Report::Location(l));
                if state.single {
                    self.stop_locked(&mut state);
                }
            }
            let interval = state.interval.max(MIN_INTERVAL);
            state = cv.wait_timeout(state, interval).unwrap().0;
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
        self.reporter
            .post(Report::Status(GnssStatusValue::SESSION_END));
        self.reporter
            .post(Report::Status(GnssStatusValue::ENGINE_OFF));
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
        // Capabilities and system info are answered within setCallback, as
        // AOSP's default IGnss does: the framework initializes the HAL with
        // it, before any session and outside the locks its session
        // callbacks take.
        let _ = callback.gnssSetCapabilitiesCb(CAPABILITY_SCHEDULING);
        let _ = callback.gnssSetSystemInfoCb(&GnssSystemInfo {
            // The model year of the Mac's positioning is unknown.
            yearOfHw: 0,
            name: "darwin CoreLocation".into(),
        });
        self.reporter.set_callback(Some(callback.clone()));
        Ok(())
    }

    fn close(&self) -> binder::Result<()> {
        let mut state = self.state.0.lock().unwrap();
        self.stop_locked(&mut state);
        self.reporter.set_callback(None);
        Ok(())
    }

    // The nullable extensions too answer UNSUPPORTED_OPERATION: the
    // framework's GnssHal wraps whatever a successful call returns and then
    // calls setCallback on it, so a null would crash system_server.
    fn getExtensionPsds(&self) -> binder::Result<Option<Strong<dyn IGnssPsds>>> {
        unsupported()
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
        unsupported()
    }

    fn getExtensionGnssGeofence(&self) -> binder::Result<Option<Strong<dyn IGnssGeofence>>> {
        unsupported()
    }

    fn getExtensionGnssNavigationMessage(
        &self,
    ) -> binder::Result<Option<Strong<dyn IGnssNavigationMessageInterface>>> {
        unsupported()
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
        self.reporter
            .post(Report::Status(GnssStatusValue::ENGINE_ON));
        self.reporter
            .post(Report::Status(GnssStatusValue::SESSION_BEGIN));
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
        unsupported()
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
    use android_hardware_gnss::aidl::android::hardware::gnss::IGnssCallback::{
        BnGnssCallback, GnssSvInfo::GnssSvInfo,
    };
    use binder::BinderFeatures;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread::{self, ThreadId};

    #[derive(Debug, PartialEq)]
    enum Seen {
        Status(GnssStatusValue),
        Location(i64),
    }

    /// Plays the framework: each session callback takes `lock`, as
    /// `GnssStatusProvider.onReportStatus` takes its multiplexer's lock.
    struct Framework {
        lock: Arc<Mutex<()>>,
        reports: Mutex<Sender<(Seen, ThreadId)>>,
    }

    impl Interface for Framework {}

    impl Framework {
        fn record(&self, what: Seen) -> binder::Result<()> {
            let _held = self.lock.lock().unwrap();
            let _ = self
                .reports
                .lock()
                .unwrap()
                .send((what, thread::current().id()));
            Ok(())
        }
    }

    impl IGnssCallback for Framework {
        fn gnssSetCapabilitiesCb(&self, _: i32) -> binder::Result<()> {
            Ok(())
        }
        fn gnssStatusCb(&self, status: GnssStatusValue) -> binder::Result<()> {
            self.record(Seen::Status(status))
        }
        fn gnssSvStatusCb(&self, _: &[GnssSvInfo]) -> binder::Result<()> {
            Ok(())
        }
        fn gnssLocationCb(&self, l: &GnssLocation) -> binder::Result<()> {
            self.record(Seen::Location(l.timestampMillis))
        }
        fn gnssNmeaCb(&self, _: i64, _: &str) -> binder::Result<()> {
            Ok(())
        }
        fn gnssAcquireWakelockCb(&self) -> binder::Result<()> {
            Ok(())
        }
        fn gnssReleaseWakelockCb(&self) -> binder::Result<()> {
            Ok(())
        }
        fn gnssSetSystemInfoCb(&self, _: &GnssSystemInfo) -> binder::Result<()> {
            Ok(())
        }
        fn gnssRequestTimeCb(&self) -> binder::Result<()> {
            Ok(())
        }
        fn gnssRequestLocationCb(&self, _: bool, _: bool) -> binder::Result<()> {
            Ok(())
        }
    }

    fn framework(gnss: &Gnss) -> (Arc<Mutex<()>>, Receiver<(Seen, ThreadId)>) {
        let (tx, rx) = channel();
        let lock = Arc::new(Mutex::new(()));
        let cb = BnGnssCallback::new_binder(
            Framework {
                lock: lock.clone(),
                reports: Mutex::new(tx),
            },
            BinderFeatures::default(),
        );
        gnss.setCallback(&cb).unwrap();
        let reporter = gnss.reporter();
        thread::spawn(move || reporter.run());
        (lock, rx)
    }

    fn next(rx: &Receiver<(Seen, ThreadId)>) -> (Seen, ThreadId) {
        rx.recv_timeout(Duration::from_secs(10))
            .expect("no report within 10 s")
    }

    #[test]
    fn stop_reports_after_it_returns_from_another_thread() {
        let gnss = Gnss::default();
        let (lock, rx) = framework(&gnss);
        // Fixes keep coming while sessions start and stop.
        let reporter = gnss.reporter();
        thread::spawn(move || {
            for ms in 0.. {
                reporter.post(Report::Location(GnssLocation {
                    timestampMillis: ms,
                    ..Default::default()
                }));
                thread::sleep(Duration::from_millis(1));
            }
        });
        for _ in 0..50 {
            gnss.state.0.lock().unwrap().running = true;
            {
                // The framework calls stop holding the lock its callbacks
                // take; a callback within stop would never return.
                let _held = lock.lock().unwrap();
                gnss.stop().unwrap();
            }
            let mut statuses = Vec::new();
            while statuses.len() < 2 {
                let (what, thread) = next(&rx);
                assert_ne!(thread, thread::current().id());
                if let Seen::Status(s) = what {
                    statuses.push(s);
                }
            }
            assert_eq!(
                statuses,
                [GnssStatusValue::SESSION_END, GnssStatusValue::ENGINE_OFF]
            );
        }
    }

    #[test]
    fn reports_arrive_in_order() {
        let gnss = Gnss::default();
        let (_lock, rx) = framework(&gnss);
        let reporter = gnss.reporter();
        reporter.post(Report::Status(GnssStatusValue::SESSION_BEGIN));
        reporter.post(Report::Location(GnssLocation {
            timestampMillis: 7,
            ..Default::default()
        }));
        reporter.post(Report::Status(GnssStatusValue::SESSION_END));
        let got: Vec<Seen> = (0..3).map(|_| next(&rx).0).collect();
        assert_eq!(
            got,
            [
                Seen::Status(GnssStatusValue::SESSION_BEGIN),
                Seen::Location(7),
                Seen::Status(GnssStatusValue::SESSION_END),
            ]
        );
    }

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
