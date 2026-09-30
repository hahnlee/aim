//! `GeofenceManager`: proximity alerts (`LocationManager.addProximityAlert`)
//! over the fused provider, following the original at the pinned tag. The
//! service asks the fused provider for locations as its own client
//! ("GeofencingService"), at an interval that shrinks as the closest
//! fence boundary gets nearer, and sends each fence's pending intent
//! when the device enters or leaves it.

use std::sync::Arc;

use aim_binder_host::local::Strong;

use super::env::{Env, Identity, PERMISSION_FINE};
use super::parcels::{Geofence, Intent, Location, LocationRequest, Value, WorkSource};

/// `LocationManager.KEY_PROXIMITY_ENTERING`.
const KEY_PROXIMITY_ENTERING: &str = "entering";
const MAX_SPEED_M_S: f64 = 100.0;
pub const MAX_LOCATION_AGE_MS: i64 = 5 * 60 * 1000;
const MAX_LOCATION_INTERVAL_MS: f64 = (2 * 60 * 60 * 1000) as f64;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Unknown,
    Inside,
    Outside,
}

pub struct Registration {
    pub pending_intent: Arc<Strong>,
    pub geofence: Geofence,
    pub identity: Identity,
    center: Location,
    state: State,
    permitted: bool,
    pub cleanup: Vec<Box<dyn FnOnce() + Send>>,
}

impl Registration {
    pub fn new(
        pending_intent: Arc<Strong>,
        geofence: Geofence,
        identity: Identity,
    ) -> Registration {
        let center = Location {
            provider: Some(String::new()),
            latitude: geofence.latitude,
            longitude: geofence.longitude,
            ..Location::default()
        };
        Registration {
            pending_intent,
            geofence,
            identity,
            center,
            state: State::Unknown,
            permitted: false,
            cleanup: Vec::new(),
        }
    }

    fn distance_to_boundary(&self, l: &Location) -> f64 {
        (self.geofence.radius as f64 - self.center.distance_to(l) as f64).abs()
    }

    /// `onLocationChanged`: the transition to tell of, if any.
    fn on_location(&mut self, l: &Location) -> Option<bool> {
        let distance = self.center.distance_to(l);
        let old = self.state;
        let accuracy = if l.has(super::parcels::HAS_HORIZONTAL_ACCURACY) {
            l.accuracy
        } else {
            0.0
        };
        let radius = self.geofence.radius.max(accuracy);
        if distance <= radius {
            self.state = State::Inside;
            (old != State::Inside).then_some(true)
        } else {
            self.state = State::Outside;
            (old == State::Inside).then_some(false)
        }
    }
}

#[derive(Default)]
pub struct Geofences {
    registrations: Vec<Registration>,
    /// The fused location the fences were last evaluated at.
    pub last_location: Option<Location>,
    /// Whether the service's fused request is in place.
    pub registered: bool,
}

impl Geofences {
    /// `addGeofence`: replaces the registration of the same intent and
    /// fence; returns the replaced one's cleanup.
    pub fn add(&mut self, env: &Env, mut r: Registration) -> Vec<Box<dyn FnOnce() + Send>> {
        let mut undone = Vec::new();
        if let Some(at) = self.registrations.iter().position(|o| {
            o.pending_intent.handle == r.pending_intent.handle && o.geofence == r.geofence
        }) {
            undone = std::mem::take(&mut self.registrations.remove(at).cleanup);
        }
        r.permitted = env.has_location_permissions(PERMISSION_FINE, &r.identity);
        self.registrations.push(r);
        undone
    }

    /// `removeGeofence(pendingIntent)`: every fence of the intent.
    pub fn remove_intent(&mut self, handle: u32) -> Vec<Box<dyn FnOnce() + Send>> {
        let mut undone = Vec::new();
        self.registrations.retain_mut(|r| {
            if r.pending_intent.handle == handle {
                undone.append(&mut r.cleanup);
                false
            } else {
                true
            }
        });
        undone
    }

    /// `mergeRegistrations`: the fused request the fences need, or none
    /// without an active fence.
    pub fn request(
        &mut self,
        env: &Env,
        caller_active: &dyn Fn(&Identity) -> bool,
        last: Option<&Location>,
    ) -> Option<LocationRequest> {
        let now = env.now_ms();
        let mut work_source: Option<WorkSource> = None;
        let mut min_distance = f64::MAX;
        let mut any = false;
        for r in &mut self.registrations {
            r.permitted = env.has_location_permissions(PERMISSION_FINE, &r.identity);
            if !(r.permitted && caller_active(&r.identity)) {
                continue;
            }
            any = true;
            if now >= r.geofence.expiration_realtime_ms {
                continue;
            }
            work_source
                .get_or_insert_with(WorkSource::default)
                .add_named(r.identity.uid, &r.identity.package);
            if let Some(l) = last {
                min_distance = min_distance.min(r.distance_to_boundary(l));
            }
        }
        if !any {
            return None;
        }
        let floor = env.background_throttle_proximity_alert_interval_ms();
        let interval = if min_distance < f64::MAX {
            MAX_LOCATION_INTERVAL_MS.min((floor as f64).max(min_distance * 1000.0 / MAX_SPEED_M_S))
                as i64
        } else {
            floor
        };
        let mut request = LocationRequest::new(interval);
        request.set_min_update_interval_ms(0);
        request.hidden_from_app_ops = true;
        request.work_source = work_source.unwrap_or_default();
        Some(request.rebuilt())
    }

    /// `onLocationChanged`: each active fence's transition is sent to
    /// its intent; an expired fence goes, as does one whose intent was
    /// cancelled. With `only`, just that (new) registration is evaluated,
    /// as `onActive` evaluates a fence at the last location.
    pub fn on_location(
        &mut self,
        env: &Env,
        caller_active: &dyn Fn(&Identity) -> bool,
        l: &Location,
        only: Option<(u32, &Geofence)>,
    ) -> Vec<Box<dyn FnOnce() + Send>> {
        if only.is_none() {
            self.last_location = Some(l.clone());
        }
        let now = env.now_ms();
        let mut undone = Vec::new();
        let mut i = 0;
        while i < self.registrations.len() {
            let r = &mut self.registrations[i];
            if only.is_some_and(|(h, g)| r.pending_intent.handle != h || &r.geofence != g) {
                i += 1;
                continue;
            }
            r.permitted = env.has_location_permissions(PERMISSION_FINE, &r.identity);
            if !(r.permitted && caller_active(&r.identity)) {
                i += 1;
                continue;
            }
            let mut remove = now >= r.geofence.expiration_realtime_ms;
            if !remove && let Some(entering) = r.on_location(l) {
                let intent = Intent {
                    action: None,
                    flags: 0,
                    extras: vec![(KEY_PROXIMITY_ENTERING, Value::Bool(entering))],
                };
                remove = !env.send_intent(&r.pending_intent, &intent);
            }
            if remove {
                undone.append(&mut self.registrations.remove(i).cleanup);
            } else {
                i += 1;
            }
        }
        undone
    }
}
