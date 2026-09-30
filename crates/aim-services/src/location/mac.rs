//! The Mac's location as the service's real providers: CoreLocation's
//! fixes ([`aim_host_location`], in this process), which macOS derives
//! from Wi-Fi positioning or a nearby iPhone's GNSS.
//!
//! One source serves every provider backed by it: CoreLocation updates
//! run while any of them has an active request, the latest fix is read
//! every [`MIN_INTERVAL_MS`] (CoreLocation updates about once a second at
//! best), and each new fix is reported once to each provider whose
//! interval has passed. A fix is
//! built as the GNSS HAL builds it (`hal/gnss`): CoreLocation's
//! accuracies, and altitude, speed and bearing only when CoreLocation
//! marks them valid.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_hostcall::location::Fix;

use super::parcels::ProviderRequest;
use super::parcels::{
    HAS_ALTITUDE, HAS_ALTITUDE_ACCURACY, HAS_BEARING, HAS_BEARING_ACCURACY,
    HAS_ELAPSED_REALTIME_UNCERTAINTY, HAS_HORIZONTAL_ACCURACY, HAS_SPEED, HAS_SPEED_ACCURACY,
    Location, RawBundle, Value,
};
use super::provider::{ProviderState, RealProvider};

const MIN_INTERVAL_MS: i64 = 1000;

/// Who is told of each provider's locations.
pub type Report = Box<dyn Fn(&str, Location) + Send + Sync>;

#[derive(Default)]
struct Subscribers {
    /// Provider name → (interval, when it last got a fix).
    wanted: HashMap<String, (i64, i64)>,
    /// The timestamp of the last fix read.
    last_fix_ms: i64,
    running: bool,
}

pub struct MacSource {
    state: Mutex<Subscribers>,
    changed: Condvar,
    report: Report,
    now_ns: fn() -> i64,
}

impl MacSource {
    pub fn new(report: Report, now_ns: fn() -> i64) -> Arc<MacSource> {
        let source = Arc::new(MacSource {
            state: Mutex::new(Subscribers::default()),
            changed: Condvar::new(),
            report,
            now_ns,
        });
        let weak = Arc::downgrade(&source);
        std::thread::Builder::new()
            .name("location-mac".into())
            .spawn(move || poll(weak))
            .expect("spawn the Mac location thread");
        source
    }

    /// A provider's request: fixes at `interval_ms`, or none.
    fn set(&self, provider: &str, interval_ms: Option<i64>) {
        let mut s = self.state.lock().unwrap();
        match interval_ms {
            Some(i) => {
                let last = s.wanted.get(provider).map_or(i64::MIN, |w| w.1);
                s.wanted
                    .insert(provider.into(), (i.max(MIN_INTERVAL_MS), last));
            }
            None => {
                s.wanted.remove(provider);
            }
        }
        self.changed.notify_all();
    }
}

fn poll(source: Weak<MacSource>) {
    loop {
        let Some(s) = source.upgrade() else { return };
        let mut state = s.state.lock().unwrap();
        if state.wanted.is_empty() {
            if state.running {
                aim_host_location::set_updates(false);
                state.running = false;
            }
            // Holds no strong reference while it waits.
            let wait = Duration::from_secs(1);
            drop(s.changed.wait_timeout(state, wait).unwrap());
            continue;
        }
        if !state.running {
            aim_host_location::set_updates(true);
            state.running = true;
        }
        drop(state);
        let fix = aim_host_location::read();
        let now_ns = (s.now_ns)();
        let now_ms = now_ns / 1_000_000;
        let mut due = Vec::new();
        {
            let mut state = s.state.lock().unwrap();
            if fix.valid != 0 && fix.unix_ms != state.last_fix_ms {
                state.last_fix_ms = fix.unix_ms;
                for (name, (interval, last)) in state.wanted.iter_mut() {
                    if now_ms.saturating_sub(*last) >= *interval {
                        *last = now_ms;
                        due.push(name.clone());
                    }
                }
            }
        }
        for name in due {
            if let Some(l) = location(&name, &fix, now_ns) {
                (s.report)(&name, l);
            }
        }
        let state = s.state.lock().unwrap();
        let wait = Duration::from_millis(MIN_INTERVAL_MS as u64);
        drop(s.changed.wait_timeout(state, wait).unwrap());
    }
}

/// A host fix as a `provider` location, `now_ns` the elapsed-realtime
/// clock; `None` without a valid position.
pub fn location(provider: &str, f: &Fix, now_ns: i64) -> Option<Location> {
    if f.valid == 0 || f.horizontal_accuracy < 0.0 {
        return None;
    }
    let mut l = Location {
        provider: Some(provider.into()),
        fields: HAS_HORIZONTAL_ACCURACY | HAS_ELAPSED_REALTIME_UNCERTAINTY,
        time_ms: f.unix_ms,
        elapsed_realtime_ns: now_ns.saturating_sub(f.age_ns as i64),
        // The host keeps fix times in milliseconds.
        elapsed_realtime_uncertainty_ns: 1_000_000.0,
        latitude: f.latitude,
        longitude: f.longitude,
        accuracy: f.horizontal_accuracy as f32,
        ..Location::default()
    };
    // CoreLocation marks an invalid value with a negative accuracy (or a
    // negative value, for speed and course).
    if f.vertical_accuracy > 0.0 {
        l.fields |= HAS_ALTITUDE | HAS_ALTITUDE_ACCURACY;
        l.altitude = f.altitude;
        l.vertical_accuracy = f.vertical_accuracy as f32;
    }
    if f.speed >= 0.0 {
        l.fields |= HAS_SPEED;
        l.speed = f.speed as f32;
        if f.speed_accuracy >= 0.0 {
            l.fields |= HAS_SPEED_ACCURACY;
            l.speed_accuracy = f.speed_accuracy as f32;
        }
    }
    if f.course >= 0.0 {
        l.fields |= HAS_BEARING;
        l.bearing = f.course as f32;
        if f.course_accuracy >= 0.0 {
            l.fields |= HAS_BEARING_ACCURACY;
            l.bearing_accuracy = f.course_accuracy as f32;
        }
    }
    if provider == super::provider::GPS {
        // GnssLocationProvider's extras, reset at each start: no
        // satellites are ever reported.
        let mut p = aim_binder_host::parcel::Parcel::new();
        super::parcels::write_bundle(
            &mut p,
            &[
                ("satellites", Value::Int(0)),
                ("meanCn0", Value::Int(0)),
                ("maxCn0", Value::Int(0)),
            ],
        );
        l.extras = RawBundle::from_parcel(&p);
    }
    Some(l)
}

/// A provider served by the Mac's location, with the original provider's
/// properties.
pub struct MacProvider {
    pub name: &'static str,
    pub state: ProviderState,
    pub source: Arc<MacSource>,
}

impl RealProvider for MacProvider {
    fn state(&self) -> ProviderState {
        self.state.clone()
    }

    fn set_request(&self, request: &ProviderRequest) {
        self.source.set(
            self.name,
            request.is_active().then_some(request.interval_ms),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_become_locations() {
        let fix = Fix {
            valid: 1,
            latitude: 37.5,
            longitude: 127.0,
            altitude: 40.0,
            horizontal_accuracy: 35.0,
            vertical_accuracy: -1.0,
            speed: -1.0,
            speed_accuracy: -1.0,
            course: 90.0,
            course_accuracy: 10.0,
            unix_ms: 1_700_000_000_000,
            age_ns: 2_000_000,
            ..Fix::default()
        };
        let l = location("gps", &fix, 10_000_000).unwrap();
        assert!(l.has(HAS_BEARING) && l.has(HAS_BEARING_ACCURACY));
        assert!(!l.has(HAS_ALTITUDE) && !l.has(HAS_SPEED));
        assert_eq!(l.elapsed_realtime_ns, 8_000_000);
        assert!(l.extras.is_some());
        assert!(
            location("network", &fix, 10_000_000)
                .unwrap()
                .extras
                .is_none()
        );
        assert!(location("gps", &Fix::default(), 1).is_none());
    }
}
