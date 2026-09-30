//! One location provider's manager: `LocationProviderManager` and the
//! `ListenerMultiplexer` it extends, following the original at the pinned
//! tag. It keeps the provider's registrations (listeners, pending
//! intents, current-location requests and the service's own), decides
//! which are active, merges the active ones into the provider's request,
//! keeps the last locations per user and delivers what the provider
//! reports, with the original's checks: permissions, app ops, the
//! fastest interval, the smallest displacement, maximum updates and
//! expiration.
//!
//! Everything runs under the service's lock ([`super::Inner`]); what must
//! not (broadcasts, calls into system_server that could wait on it) is
//! queued as [`Effects`] and done after it is released.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::{
    android_location_ilocationcallback as location_callback,
    android_location_ilocationlistener as location_listener,
};

use super::env::{Env, Identity, PERMISSION_COARSE, PERMISSION_FINE};
use super::fudger::Fudger;
use super::parcels::{
    Intent, LastLocationRequest, Location, LocationRequest, PASSIVE_INTERVAL, ProviderProperties,
    ProviderRequest, QUALITY_LOW_POWER, Value, WorkSource,
};

pub const GPS: &str = "gps";
pub const PASSIVE: &str = "passive";

/// `LocationProviderManager`'s timing constants.
const MIN_COARSE_INTERVAL_MS: i64 = 10 * 60 * 1000;
const MAX_HIGH_POWER_INTERVAL_MS: i64 = 5 * 60 * 1000;
pub const MAX_CURRENT_LOCATION_AGE_MS: i64 = 30 * 1000;
pub const MAX_GET_CURRENT_LOCATION_TIMEOUT_MS: i64 = 30 * 1000;
const FASTEST_INTERVAL_JITTER_PERCENTAGE: f64 = 0.10;
const MAX_FASTEST_INTERVAL_JITTER_MS: i64 = 30 * 1000;
const MIN_REQUEST_DELAY_MS: i64 = 30 * 1000;

/// `AppOpsManager.OP_*`.
pub const OP_COARSE_LOCATION: i32 = 0;
pub const OP_FINE_LOCATION: i32 = 1;
pub const OP_MONITOR_LOCATION: i32 = 41;
pub const OP_MONITOR_HIGH_POWER_LOCATION: i32 = 42;

/// `LocationManager`'s intent extras and actions.
pub const KEY_LOCATION_CHANGED: &str = "location";
pub const KEY_LOCATIONS: &str = "locations";
pub const KEY_PROVIDER_ENABLED: &str = "providerEnabled";
pub const KEY_FLUSH_COMPLETE: &str = "flushComplete";
pub const PROVIDERS_CHANGED_ACTION: &str = "android.location.PROVIDERS_CHANGED";
pub const EXTRA_PROVIDER_NAME: &str = "android.location.extra.PROVIDER_NAME";
pub const EXTRA_PROVIDER_ENABLED: &str = "android.location.extra.PROVIDER_ENABLED";
const LOCATION_CLASS: &str = "android.location.Location";

/// `permissionLevel` as the app op the original notes a delivery with.
pub fn as_app_op(level: i32) -> i32 {
    if level == PERMISSION_FINE {
        OP_FINE_LOCATION
    } else {
        OP_COARSE_LOCATION
    }
}

/// What reaches a registration's client.
pub enum Transport {
    /// An `ILocationListener`.
    Listener(Arc<Strong>),
    /// A `PendingIntent`'s `IIntentSender`.
    PendingIntent(Arc<Strong>),
    /// A `getCurrentLocation` request's `ILocationCallback`.
    Current(Arc<Strong>),
    /// The service's own client (the geofence manager): told on the
    /// service's executor, never under the lock.
    Internal(Deliver),
}

/// How the service's own client is told of a location.
pub type Deliver = Arc<dyn Fn(Location) + Send + Sync>;

/// A registration's key: the client's binder (its handle in this
/// process), or an id of the service's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Binder(u32),
    Internal(u64),
}

pub struct Registration {
    pub transport: Transport,
    base: LocationRequest,
    pub identity: Identity,
    pub permission_level: i32,
    permitted: bool,
    foreground: bool,
    /// The request the provider serves it by (`mProviderLocationRequest`).
    request: LocationRequest,
    high_power: bool,
    last_delivered: Option<Location>,
    active: bool,
    registered: bool,
    delivered: i32,
    expiration_ms: i64,
    /// The service's hooks: its death link, its cancel listener, its
    /// alarm; undone when it goes.
    pub cleanup: Vec<Box<dyn FnOnce() + Send>>,
}

impl Registration {
    pub fn new(
        transport: Transport,
        request: LocationRequest,
        identity: Identity,
        permission_level: i32,
    ) -> Registration {
        Registration {
            transport,
            base: request.clone(),
            identity,
            permission_level,
            permitted: false,
            foreground: false,
            request,
            high_power: false,
            last_delivered: None,
            active: false,
            registered: false,
            delivered: 0,
            expiration_ms: i64::MAX,
            cleanup: Vec::new(),
        }
    }

    pub fn request(&self) -> &LocationRequest {
        &self.request
    }

    fn is_current(&self) -> bool {
        matches!(self.transport, Transport::Current(_))
    }

    fn is_location(&self) -> bool {
        !self.is_current()
    }
}

/// The last locations of one user, fine and coarse, with and without
/// the location setting's bypass.
#[derive(Default)]
struct LastLocation {
    fine: Option<Location>,
    coarse: Option<Location>,
    fine_bypass: Option<Location>,
    coarse_bypass: Option<Location>,
}

impl LastLocation {
    fn get(&self, level: i32, bypass: bool) -> Option<&Location> {
        match (level == PERMISSION_FINE, bypass) {
            (true, false) => self.fine.as_ref(),
            (true, true) => self.fine_bypass.as_ref(),
            (false, false) => self.coarse.as_ref(),
            (false, true) => self.coarse_bypass.as_ref(),
        }
    }

    fn set(&mut self, l: &Location) {
        next_fine(&mut self.fine, l);
        next_coarse(&mut self.coarse, l);
    }

    fn set_bypass(&mut self, l: &Location) {
        next_fine(&mut self.fine_bypass, l);
        next_coarse(&mut self.coarse_bypass, l);
    }

    fn clear_locations(&mut self) {
        self.fine = None;
        self.coarse = None;
    }

    fn clear_mock(&mut self) {
        for slot in [
            &mut self.fine,
            &mut self.coarse,
            &mut self.fine_bypass,
            &mut self.coarse_bypass,
        ] {
            if slot.as_ref().is_some_and(Location::is_mock) {
                *slot = None;
            }
        }
    }
}

fn next_fine(old: &mut Option<Location>, new: &Location) {
    if old
        .as_ref()
        .is_none_or(|o| new.elapsed_realtime_ns > o.elapsed_realtime_ns)
    {
        *old = Some(new.clone());
    }
}

fn next_coarse(old: &mut Option<Location>, new: &Location) {
    if old.as_ref().is_none_or(|o| {
        new.elapsed_realtime_ms() - MIN_COARSE_INTERVAL_MS > o.elapsed_realtime_ms()
    }) {
        *old = Some(new.clone());
    }
}

/// `AbstractLocationProvider.State`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderState {
    pub allowed: bool,
    pub properties: Option<ProviderProperties>,
    pub identity: Option<Identity>,
    pub extra_attribution_tags: Vec<String>,
}

/// A real provider behind a manager (`AbstractLocationProvider`'s
/// controller): the service sets its request, and it reports locations
/// and state through the service.
pub trait RealProvider: Send + Sync {
    fn state(&self) -> ProviderState;
    fn set_request(&self, request: &ProviderRequest);

    /// Whether a flush waits for the provider ([`RealProvider::flush`]);
    /// otherwise it completes at once, nothing being queued.
    fn defers_flush(&self) -> bool {
        false
    }

    /// `onFlush(callback)`: `done` runs when the provider's queued
    /// locations are out.
    fn flush(&self, done: Box<dyn FnOnce() + Send>) {
        done()
    }

    /// `start()` and `stop()`: a test provider in its place stops it.
    fn set_started(&self, _started: bool) {}
}

/// `MockLocationProvider`.
pub struct Mock {
    pub state: ProviderState,
    pub location: Option<Location>,
}

/// What to do once the service's lock is released.
#[derive(Default)]
pub struct Effects {
    /// Broadcasts: an intent to a user.
    pub broadcasts: Vec<(OwnedIntent, i32)>,
    /// The service's own clients to tell.
    pub internal: Vec<(Deliver, Location)>,
    /// Provider request listeners to tell: provider, request.
    pub requests: Vec<(String, ProviderRequest)>,
    /// A provider's state changed (identity or tags): the location
    /// package tags must be computed again for these uids.
    pub state_changed: Vec<(Option<Identity>, Option<Identity>)>,
    /// App op restrictions to refresh.
    pub refresh_restrictions: bool,
}

/// An intent to broadcast, owning its values.
pub struct OwnedIntent {
    pub action: &'static str,
    pub provider: String,
    pub enabled: bool,
}

impl OwnedIntent {
    pub fn write(&self, p: &mut Parcel) {
        use aim_service_aidl::WriteParcelable;
        Intent {
            action: Some(self.action),
            flags: super::parcels::FLAG_RECEIVER_REGISTERED_ONLY
                | super::parcels::FLAG_RECEIVER_FOREGROUND,
            extras: vec![
                (EXTRA_PROVIDER_NAME, Value::String(&self.provider)),
                (EXTRA_PROVIDER_ENABLED, Value::Bool(self.enabled)),
            ],
        }
        .write_to(p);
    }
}

pub struct ProviderManager {
    pub name: String,
    /// Whether locations also go to the passive provider (every provider
    /// but the passive one).
    feeds_passive: bool,
    /// Permissions a caller needs to see the provider at all.
    required_permissions: Vec<&'static str>,
    enabled: HashMap<i32, bool>,
    last: HashMap<i32, LastLocation>,
    registrations: BTreeMap<Key, Registration>,
    service_registered: bool,
    merged: Option<ProviderRequest>,
    /// The request the provider serves (`MockableLocationProvider.mRequest`).
    current_request: ProviderRequest,
    /// A delayed request and when it is due.
    pub delayed: Option<(ProviderRequest, i64)>,
    pub real: Option<Arc<dyn RealProvider>>,
    pub mock: Option<Mock>,
    /// The provider's state as last seen, to notice its changes.
    state: ProviderState,
    fudger: Fudger,
    /// The population density provider's cache
    /// (`setLocationFudgerCache`), for the providers that exist when it
    /// is found.
    pub density: Option<Arc<super::proxy::Density>>,
    pub request_listeners: Vec<Arc<Strong>>,
}

impl ProviderManager {
    pub fn new(
        name: &str,
        feeds_passive: bool,
        required_permissions: Vec<&'static str>,
        env: &Env,
    ) -> ProviderManager {
        ProviderManager {
            name: name.to_string(),
            feeds_passive,
            required_permissions,
            enabled: HashMap::new(),
            last: HashMap::new(),
            registrations: BTreeMap::new(),
            service_registered: false,
            merged: None,
            current_request: ProviderRequest::empty(),
            delayed: None,
            real: None,
            mock: None,
            state: ProviderState::default(),
            fudger: Fudger::new(env.coarse_accuracy_m(), env.now_ms()),
            density: None,
            request_listeners: Vec::new(),
        }
    }

    pub fn is_passive(&self) -> bool {
        self.name == PASSIVE
    }

    pub fn is_mock(&self) -> bool {
        self.mock.is_some()
    }

    /// The state of the provider in use: the mock one if any.
    pub fn provider_state(&self) -> ProviderState {
        match (&self.mock, &self.real) {
            (Some(m), _) => m.state.clone(),
            (None, Some(r)) => r.state(),
            (None, None) => ProviderState::default(),
        }
    }

    pub fn properties(&self) -> Option<ProviderProperties> {
        self.provider_state().properties
    }

    pub fn identity(&self) -> Option<Identity> {
        self.provider_state().identity
    }

    pub fn has_provider(&self) -> bool {
        self.mock.is_some() || self.real.is_some()
    }

    /// `isVisibleToCaller`.
    pub fn visible_to(&self, env: &Env, uid: i32, pid: i32) -> bool {
        if uid == super::env::SYSTEM_UID || self.is_mock() {
            return true;
        }
        self.required_permissions
            .iter()
            .all(|p| env.check_permission(p, pid, uid))
    }

    /// `startManager`: the users' enabled state, silently.
    pub fn start(&mut self, env: &Env, effects: &mut Effects) {
        self.state = self.provider_state();
        self.enabled.clear();
        for user in env.running_users() {
            self.on_enabled_changed(env, user, effects);
        }
    }

    /// `isEnabled(userId)`.
    pub fn is_enabled(&mut self, env: &Env, user: i32, effects: &mut Effects) -> bool {
        let user = if user == super::env::USER_CURRENT {
            env.current_user()
        } else {
            user
        };
        if user < 0 {
            return false;
        }
        if !self.enabled.contains_key(&user) {
            self.on_enabled_changed(env, user, effects);
        }
        self.enabled[&user]
    }

    /// `onEnabledChanged(userId)`.
    pub fn on_enabled_changed(&mut self, env: &Env, user: i32, effects: &mut Effects) {
        if user == super::env::USER_ALL {
            for user in env.running_users() {
                self.on_enabled_changed(env, user, effects);
            }
            return;
        }
        let enabled = self.provider_state().allowed && env.location_enabled(user);
        let was = self.enabled.insert(user, enabled);
        if was == Some(enabled) {
            return;
        }
        if !enabled && let Some(last) = self.last.get_mut(&user) {
            last.clear_locations();
        }
        if was.is_some() {
            // The passive provider never gets public updates.
            if !self.is_passive() {
                effects.broadcasts.push((
                    OwnedIntent {
                        action: PROVIDERS_CHANGED_ACTION,
                        provider: self.name.clone(),
                        enabled,
                    },
                    user,
                ));
            }
            // Clients registered for location hear of it.
            let name = self.name.clone();
            let mut failed = Vec::new();
            for (key, r) in &self.registrations {
                if r.is_location()
                    && r.identity.user_id() == user
                    && !deliver_enabled(env, &name, r, enabled)
                {
                    failed.push(*key);
                }
            }
            for key in failed {
                self.remove(env, key, effects);
            }
        }
        self.update_registrations(env, effects, |r| r.identity.user_id() == user);
    }

    /// `onUserStarted(userId)`: the user's enabled state computed again,
    /// silently.
    pub fn on_user_started(&mut self, env: &Env, user: i32, effects: &mut Effects) {
        self.enabled.remove(&user);
        self.on_enabled_changed(env, user, effects);
    }

    /// `onUserStopped(userId)`.
    pub fn on_user_stopped(&mut self, user: i32) {
        self.enabled.remove(&user);
        self.last.remove(&user);
    }

    /// `onPackageReset(packageName)`: the package's registrations go.
    pub fn package_reset(&mut self, env: &Env, package: &str, effects: &mut Effects) {
        for key in self.keys() {
            if self.registrations[&key].identity.package == package {
                self.remove(env, key, effects);
            }
        }
    }

    /// `onStateChanged`: the provider's state became `new`.
    pub fn on_state_changed(&mut self, env: &Env, effects: &mut Effects) {
        let new = self.provider_state();
        let old = std::mem::replace(&mut self.state, new.clone());
        if old == new {
            return;
        }
        if old.allowed != new.allowed {
            self.on_enabled_changed(env, super::env::USER_ALL, effects);
        }
        if old.properties != new.properties {
            let keys: Vec<Key> = self.registrations.keys().copied().collect();
            for key in keys {
                self.high_power_changed(env, key);
            }
        }
        if old.identity != new.identity || old.extra_attribution_tags != new.extra_attribution_tags
        {
            effects
                .state_changed
                .push((old.identity.clone(), new.identity.clone()));
        }
        if old.identity != new.identity {
            effects.refresh_restrictions = true;
        }
    }

    /// `setMockProvider`: a test provider in place of the real one, or
    /// the real one back; the real one serves no request meanwhile.
    pub fn set_mock(&mut self, env: &Env, mock: Option<Mock>, effects: &mut Effects) {
        let removing = mock.is_none();
        if removing && self.mock.is_none() {
            return;
        }
        let replacing = self.mock.is_some();
        self.mock = mock;
        if let Some(real) = &self.real {
            if removing {
                real.set_started(true);
                real.set_request(&self.current_request);
            } else if !replacing {
                real.set_request(&ProviderRequest::empty());
                real.set_started(false);
            }
        }
        self.on_state_changed(env, effects);
        if removing {
            for last in self.last.values_mut() {
                last.clear_mock();
            }
            self.fudger.reset_offsets(env.now_ms());
        }
    }

    /// `setRealProvider`.
    pub fn set_real(&mut self, env: &Env, real: Arc<dyn RealProvider>, effects: &mut Effects) {
        if self.mock.is_none() {
            real.set_request(&self.current_request);
        }
        self.real = Some(real);
        self.on_state_changed(env, effects);
    }

    /// `setMockProviderAllowed`.
    pub fn set_mock_allowed(&mut self, env: &Env, allowed: bool, effects: &mut Effects) {
        if let Some(m) = &mut self.mock {
            m.state.allowed = allowed;
        }
        self.on_state_changed(env, effects);
    }

    // ---- registrations (ListenerMultiplexer) ----

    /// `putRegistration`: adds (or replaces) `key`'s registration. As the
    /// original's `onRegister`, an expired registration goes (a
    /// current-location request gets null) and a location client hears
    /// at once of a disabled provider, before it can become active; a
    /// current-location request that does not become active fails.
    pub fn put(&mut self, env: &Env, key: Key, mut reg: Registration, effects: &mut Effects) {
        if let Some(mut old) = self.registrations.remove(&key) {
            old.registered = false;
            self.set_active(env, &mut old, false);
            reg.last_delivered = old.last_delivered.take();
            for undo in old.cleanup.drain(..) {
                undo();
            }
        }
        reg.registered = true;
        reg.permitted = env.has_location_permissions(reg.permission_level, &reg.identity);
        reg.foreground = env.foreground(reg.identity.uid);
        reg.request = self.provider_location_request(env, &reg);
        let now = env.now_ms();
        reg.expiration_ms = reg.request.expiration_realtime_ms(now);
        let (current, expired, user) = (
            reg.is_current(),
            reg.expiration_ms <= now,
            reg.identity.user_id(),
        );
        self.registrations.insert(key, reg);
        if expired {
            if current {
                self.deliver_current(env, key, None, effects);
            } else {
                self.remove(env, key, effects);
            }
            return;
        }
        if !current && !self.is_enabled(env, user, effects) {
            let reg = &self.registrations[&key];
            if !deliver_enabled(env, &self.name, reg, false) {
                self.remove(env, key, effects);
                return;
            }
        }
        self.update_one(env, key, effects);
        if current && self.registrations.get(&key).is_some_and(|r| !r.active) {
            self.deliver_current(env, key, None, effects);
        }
    }

    /// When a registration's expiration is due, if ever.
    pub fn expiration(&self, key: Key) -> Option<i64> {
        self.registrations
            .get(&key)
            .map(|r| r.expiration_ms)
            .filter(|&e| e < i64::MAX)
    }

    /// When a registration expires (its alarm).
    pub fn expire(&mut self, env: &Env, key: Key, effects: &mut Effects) {
        let Some(reg) = self.registrations.get(&key) else {
            return;
        };
        if reg.expiration_ms > env.now_ms() {
            return;
        }
        if reg.is_current() {
            self.deliver_current(env, key, None, effects);
        } else {
            self.remove(env, key, effects);
        }
    }

    /// `removeRegistration`.
    pub fn remove(&mut self, env: &Env, key: Key, effects: &mut Effects) {
        let _ = effects;
        if let Some(mut reg) = self.registrations.remove(&key) {
            reg.registered = false;
            self.set_active(env, &mut reg, false);
            for undo in reg.cleanup.drain(..) {
                undo();
            }
        }
    }

    pub fn registration(&self, key: Key) -> Option<&Registration> {
        self.registrations.get(&key)
    }

    pub fn keys(&self) -> Vec<Key> {
        self.registrations.keys().copied().collect()
    }

    /// `updateRegistrations(predicate)`: re-evaluates each registration
    /// the predicate selects.
    pub fn update_registrations(
        &mut self,
        env: &Env,
        effects: &mut Effects,
        select: impl Fn(&Registration) -> bool,
    ) {
        let keys: Vec<Key> = self
            .registrations
            .iter()
            .filter(|(_, r)| select(r))
            .map(|(k, _)| *k)
            .collect();
        for key in keys {
            self.update_one(env, key, effects);
        }
    }

    /// `onRegistrationActiveChanged`, after the registration's inputs were
    /// recomputed (`onProviderLocationRequestChanged`, permissions,
    /// foreground).
    pub fn update_one(&mut self, env: &Env, key: Key, effects: &mut Effects) {
        let Some(mut reg) = self.registrations.remove(&key) else {
            return;
        };
        let new_request = self.provider_location_request(env, &reg);
        if new_request != reg.request {
            reg.request = new_request;
        }
        let active = reg.registered && self.is_active(env, &reg, effects);
        let became = self.set_active(env, &mut reg, active);
        let current = reg.is_current();
        self.registrations.insert(key, reg);
        self.high_power_changed(env, key);
        match became {
            Some(true) => self.on_became_active(env, key, effects),
            // A current-location request that goes inactive fails at once.
            Some(false) if current => self.deliver_current(env, key, None, effects),
            _ => {}
        }
    }

    /// `onLocationPermissionsChanged` of the registrations `select`
    /// picks: their permission asked again.
    pub fn permissions_changed(
        &mut self,
        env: &Env,
        effects: &mut Effects,
        select: impl Fn(&Identity) -> bool,
    ) {
        for key in self.keys() {
            let reg = self.registrations.get_mut(&key).unwrap();
            if select(&reg.identity) {
                reg.permitted = env.has_location_permissions(reg.permission_level, &reg.identity);
                self.update_one(env, key, effects);
            }
        }
    }

    /// `onForegroundChanged(uid, foreground)`.
    pub fn foreground_changed(
        &mut self,
        env: &Env,
        effects: &mut Effects,
        uid: i32,
        foreground: bool,
    ) {
        for key in self.keys() {
            let reg = self.registrations.get_mut(&key).unwrap();
            if reg.identity.uid == uid && reg.foreground != foreground {
                reg.foreground = foreground;
                self.update_one(env, key, effects);
            }
        }
    }

    /// Sets the active state; `Some(new)` if it changed. Starts and
    /// finishes the monitoring app ops as the original does.
    fn set_active(&mut self, env: &Env, reg: &mut Registration, active: bool) -> Option<bool> {
        if reg.active == active {
            return None;
        }
        reg.active = active;
        if active {
            if !reg.request.hidden_from_app_ops {
                env.start_op(OP_MONITOR_LOCATION, &reg.identity);
            }
        } else {
            let hp = reg.high_power;
            if hp {
                reg.high_power = false;
                if !reg.request.hidden_from_app_ops {
                    env.finish_op(OP_MONITOR_HIGH_POWER_LOCATION, &reg.identity);
                }
            }
            if !reg.request.hidden_from_app_ops {
                env.finish_op(OP_MONITOR_LOCATION, &reg.identity);
            }
        }
        Some(active)
    }

    /// `onHighPowerUsageChanged`.
    fn high_power_changed(&mut self, env: &Env, key: Key) {
        let properties = self.properties();
        let Some(reg) = self.registrations.get_mut(&key) else {
            return;
        };
        let high = properties.is_some_and(|p| {
            reg.active
                && reg.request.interval_ms < MAX_HIGH_POWER_INTERVAL_MS
                && p.power_usage == super::parcels::POWER_USAGE_HIGH
        });
        if high != reg.high_power {
            reg.high_power = high;
            if !reg.request.hidden_from_app_ops {
                if high {
                    env.start_op(OP_MONITOR_HIGH_POWER_LOCATION, &reg.identity);
                } else {
                    env.finish_op(OP_MONITOR_HIGH_POWER_LOCATION, &reg.identity);
                }
            }
        }
    }

    /// A registration became active: a current-location request gets a
    /// recent enough last location, a location registration a historical
    /// one (apps targeting S and later, `DELIVER_HISTORICAL_LOCATIONS`).
    fn on_became_active(&mut self, env: &Env, key: Key, effects: &mut Effects) {
        let Some(reg) = self.registrations.get(&key) else {
            return;
        };
        let (user, level, bypass) = (
            reg.identity.user_id(),
            reg.permission_level,
            reg.request.is_bypass(),
        );
        if reg.is_current() {
            if let Some(last) =
                self.last_location_unsafe(env, user, level, bypass, MAX_CURRENT_LOCATION_AGE_MS)
            {
                self.deliver_current(env, key, Some(vec![last]), effects);
            }
            return;
        }
        if !env.deliver_historical_locations(reg.identity.uid) {
            return;
        }
        let now = env.now_ms();
        let mut max_age = reg.request.interval_ms;
        if let Some(last) = &reg.last_delivered {
            max_age = max_age.min(last.age_ms(now) - 1);
        }
        if max_age > MIN_REQUEST_DELAY_MS
            && let Some(last) = self.last_location_unsafe(env, user, level, bypass, max_age)
        {
            self.deliver_location(env, key, vec![last], effects);
        }
    }

    /// `calculateProviderLocationRequest`.
    fn provider_location_request(&self, env: &Env, reg: &Registration) -> LocationRequest {
        let base = &reg.base;
        let mut r = base.rebuilt();
        if reg.permission_level < PERMISSION_FINE {
            r.quality = QUALITY_LOW_POWER;
            if base.interval_ms < MIN_COARSE_INTERVAL_MS {
                r.interval_ms = MIN_COARSE_INTERVAL_MS;
            }
            if base.min_update_interval_ms() < MIN_COARSE_INTERVAL_MS {
                r.set_min_update_interval_ms(MIN_COARSE_INTERVAL_MS);
            }
            r = r.rebuilt();
        }
        let mut ignored = base.location_settings_ignored;
        if ignored
            && !env.ignore_settings_allowlist().contains(
                &reg.identity.package,
                reg.identity.attribution_tag.as_deref(),
            )
            && !env.is_provider(None, &reg.identity)
        {
            ignored = false;
        }
        r.location_settings_ignored = ignored;
        // ADAS bypass needs an automotive device and its setting, which
        // this device has not (LocationSettings keeps it off).
        r.adas_gnss_bypass = false;
        if !ignored && !self.throttling_exempt(env, reg) && !reg.foreground {
            r.interval_ms = base.interval_ms.max(env.background_throttle_interval_ms());
            r = r.rebuilt();
        }
        r
    }

    /// `isThrottlingExempt`.
    fn throttling_exempt(&self, env: &Env, reg: &Registration) -> bool {
        env.background_throttle_package_whitelist()
            .contains(&reg.identity.package)
            || env.is_provider(None, &reg.identity)
    }

    /// `isActive(registration)`.
    fn is_active(&mut self, env: &Env, reg: &Registration, effects: &mut Effects) -> bool {
        if !reg.permitted {
            return false;
        }
        let bypass = reg.request.is_bypass();
        if !self.caller_active(env, bypass, &reg.identity, effects) {
            return false;
        }
        if bypass {
            return true;
        }
        let power = *env.power.lock().unwrap();
        match power.mode {
            super::env::LOCATION_MODE_FOREGROUND_ONLY => reg.foreground,
            super::env::LOCATION_MODE_GPS_DISABLED_WHEN_SCREEN_OFF if self.name != GPS => true,
            super::env::LOCATION_MODE_GPS_DISABLED_WHEN_SCREEN_OFF
            | super::env::LOCATION_MODE_THROTTLE_REQUESTS_WHEN_SCREEN_OFF
            | super::env::LOCATION_MODE_ALL_DISABLED_WHEN_SCREEN_OFF => power.interactive,
            _ => true,
        }
    }

    /// `isActive(isBypass, identity)`.
    fn caller_active(
        &mut self,
        env: &Env,
        bypass: bool,
        identity: &Identity,
        effects: &mut Effects,
    ) -> bool {
        if identity.uid == super::env::SYSTEM_UID {
            if !bypass && !self.is_enabled(env, env.current_user(), effects) {
                return false;
            }
        } else {
            if !bypass {
                if !self.is_enabled(env, identity.user_id(), effects) {
                    return false;
                }
                if !env.user_visible(identity.user_id()) {
                    return false;
                }
            }
            if env.package_blacklisted(identity.user_id(), &identity.package) {
                return false;
            }
        }
        true
    }

    /// `updateService`: merges the active registrations into the
    /// provider's request (`registerWithService`,
    /// `reregisterWithService`), delayed as the original delays it.
    pub fn update_service(&mut self, env: &Env, effects: &mut Effects) {
        let actives: Vec<&Registration> =
            self.registrations.values().filter(|r| r.active).collect();
        if actives.is_empty() {
            if self.service_registered {
                self.merged = None;
                self.service_registered = false;
                self.set_provider_request(ProviderRequest::empty(), effects);
            }
            return;
        }
        let merged = self.merge(&actives);
        let registered = self.service_registered;
        if registered && self.merged.as_ref() == Some(&merged) {
            return;
        }
        let old = match (registered, &self.merged) {
            (true, Some(old)) => old.clone(),
            _ => ProviderRequest::empty(),
        };
        self.service_registered = true;
        self.merged = Some(merged.clone());
        // An inactive request is the provider's default: nothing to send.
        if !registered && !merged.is_active() {
            return;
        }
        let delay =
            if (!old.is_bypass() && merged.is_bypass()) || merged.interval_ms > old.interval_ms {
                0
            } else {
                self.request_delay_ms(env, merged.interval_ms)
            };
        if delay < MIN_REQUEST_DELAY_MS {
            self.set_provider_request(merged, effects);
        } else {
            self.delayed = Some((merged, env.now_ms() + delay));
        }
    }

    /// A delayed request that is due.
    pub fn delayed_due(&mut self, effects: &mut Effects) {
        if let Some((request, _)) = self.delayed.take() {
            self.set_provider_request(request, effects);
        }
    }

    /// `setProviderRequest`.
    fn set_provider_request(&mut self, request: ProviderRequest, effects: &mut Effects) {
        self.delayed = None;
        self.current_request = request.clone();
        if let (None, Some(real)) = (&self.mock, &self.real) {
            real.set_request(&request);
        }
        effects.requests.push((self.name.clone(), request));
    }

    pub fn current_request(&self) -> &ProviderRequest {
        &self.current_request
    }

    /// `mergeRegistrations`.
    fn merge(&self, actives: &[&Registration]) -> ProviderRequest {
        if self.is_passive() {
            return ProviderRequest {
                interval_ms: 0,
                ..ProviderRequest::empty()
            };
        }
        let mut interval = super::parcels::INTERVAL_DISABLED;
        let mut quality = QUALITY_LOW_POWER;
        let mut max_update_delay = i64::MAX;
        let mut adas = false;
        let mut ignored = false;
        let mut low_power = true;
        for r in actives {
            let q = &r.request;
            // passive requests do not contribute to the provider request
            if q.interval_ms == PASSIVE_INTERVAL {
                continue;
            }
            interval = interval.min(q.interval_ms);
            quality = quality.min(q.quality);
            max_update_delay = max_update_delay.min(q.max_update_delay_ms);
            adas |= q.adas_gnss_bypass;
            ignored |= q.location_settings_ignored;
            low_power &= q.low_power;
        }
        if interval == super::parcels::INTERVAL_DISABLED {
            return ProviderRequest::empty();
        }
        if max_update_delay / 2 < interval {
            max_update_delay = 0;
        }
        let threshold = interval
            .checked_add(1000)
            .map(|v| v / 2)
            .and_then(|v| v.checked_mul(3))
            .unwrap_or(PASSIVE_INTERVAL - 1);
        let mut work_source = WorkSource::default();
        for r in actives {
            if r.request.interval_ms <= threshold {
                work_source.add(&r.request.work_source);
            }
        }
        ProviderRequest {
            interval_ms: interval,
            quality,
            max_update_delay_ms: max_update_delay,
            low_power,
            adas_gnss_bypass: adas,
            location_settings_ignored: ignored,
            work_source,
        }
    }

    /// `calculateRequestDelayMillis`.
    fn request_delay_ms(&self, env: &Env, new_interval_ms: i64) -> i64 {
        if self.is_passive() {
            return 0;
        }
        let now = env.now_ms();
        let mut delay = new_interval_ms;
        for r in self.registrations.values().filter(|r| r.active) {
            if delay == 0 {
                break;
            }
            let q = &r.request;
            let last = r.last_delivered.clone().or_else(|| {
                (!q.location_settings_ignored)
                    .then(|| {
                        self.last_location_unsafe(
                            env,
                            r.identity.user_id(),
                            r.permission_level,
                            false,
                            q.interval_ms,
                        )
                    })
                    .flatten()
            });
            let registration_delay = match last {
                None => 0,
                Some(last) => (q.interval_ms - last.age_ms(now)).max(0),
            };
            delay = delay.min(registration_delay);
        }
        delay
    }

    // ---- last locations ----

    /// `getLastLocationUnsafe`: always the fine location.
    pub fn last_location_unsafe(
        &self,
        env: &Env,
        user: i32,
        level: i32,
        bypass: bool,
        max_age_ms: i64,
    ) -> Option<Location> {
        if user == super::env::USER_ALL {
            let mut best: Option<Location> = None;
            for u in env.running_users() {
                if let Some(next) = self.last_location_unsafe(env, u, level, bypass, max_age_ms)
                    && best
                        .as_ref()
                        .is_none_or(|b| next.elapsed_realtime_ns > b.elapsed_realtime_ns)
                {
                    best = Some(next);
                }
            }
            return best;
        }
        let user = if user == super::env::USER_CURRENT {
            env.current_user()
        } else {
            user
        };
        let location = self.last.get(&user)?.get(level, bypass)?.clone();
        (location.age_ms(env.now_ms()) <= max_age_ms).then_some(location)
    }

    /// `setLastLocation(location, userId)`.
    fn set_last_location(&mut self, env: &Env, l: &Location, user: i32, effects: &mut Effects) {
        if user == super::env::USER_ALL {
            for u in env.running_users() {
                self.set_last_location(env, l, u, effects);
            }
            return;
        }
        let enabled = self.is_enabled(env, user, effects);
        let last = self.last.entry(user).or_default();
        if enabled {
            last.set(l);
        }
        last.set_bypass(l);
    }

    /// `injectLastLocation`.
    pub fn inject_last_location(
        &mut self,
        env: &Env,
        l: &Location,
        user: i32,
        effects: &mut Effects,
    ) {
        if self
            .last_location_unsafe(env, user, PERMISSION_FINE, false, i64::MAX)
            .is_none()
        {
            self.set_last_location(env, l, user, effects);
        }
    }

    /// `getLastLocation(request, identity, permissionLevel)`, the app op
    /// noted.
    pub fn get_last_location(
        &mut self,
        env: &Env,
        request: &LastLocationRequest,
        identity: &Identity,
        level: i32,
        effects: &mut Effects,
    ) -> Option<Location> {
        let mut request = request.clone();
        if request.location_settings_ignored
            && !env
                .ignore_settings_allowlist()
                .contains(&identity.package, identity.attribution_tag.as_deref())
            && !env.is_provider(None, identity)
        {
            request.location_settings_ignored = false;
        }
        request.adas_gnss_bypass = false;
        if !self.caller_active(env, request.is_bypass(), identity, effects) {
            return None;
        }
        let fine = self.last_location_unsafe(
            env,
            identity.user_id(),
            level,
            request.is_bypass(),
            i64::MAX,
        )?;
        let location = self.permitted(env, fine, level);
        if !env.note_op_no_throw(as_app_op(level), identity) {
            return None;
        }
        Some(location)
    }

    /// `getPermittedLocation`.
    fn permitted(&mut self, env: &Env, fine: Location, level: i32) -> Location {
        if level == PERMISSION_COARSE {
            self.fudger
                .coarse(&fine, env.now_ms(), self.density.as_deref())
        } else {
            fine
        }
    }

    // ---- reports ----

    /// `onReportLocation`: the provider (or, for the passive provider,
    /// another provider) reported `locations`. Returns what the passive
    /// provider gets.
    pub fn on_report_location(
        &mut self,
        env: &Env,
        locations: Vec<Location>,
        effects: &mut Effects,
    ) -> Option<Vec<Location>> {
        let mut processed = locations;
        if self.feeds_passive {
            // `processReportedLocation`: validated; the MSL altitude the
            // original adds needs its altitude assets (#?), not here.
            let now_ns = env.now_ns();
            let mut previous = 0;
            for l in &mut processed {
                if let Err(e) = l.validate(previous, now_ns) {
                    eprintln!(
                        "location: {} provider: dropping invalid locations: {e}",
                        self.name
                    );
                    return None;
                }
                previous = l.elapsed_realtime_ns;
            }
        }
        let last = processed.last()?.clone();
        self.set_last_location(env, &last, super::env::USER_ALL, effects);
        for key in self.keys() {
            if self.registrations.get(&key).is_some_and(|r| r.active) {
                if self.registrations[&key].is_current() {
                    self.deliver_current(env, key, Some(processed.clone()), effects);
                } else {
                    self.deliver_location(env, key, processed.clone(), effects);
                }
            }
        }
        self.feeds_passive.then_some(processed)
    }

    /// `LocationRegistration.acceptLocationChange` and its delivery.
    fn deliver_location(
        &mut self,
        env: &Env,
        key: Key,
        fine: Vec<Location>,
        effects: &mut Effects,
    ) {
        let now = env.now_ms();
        let Some(reg) = self.registrations.get(&key) else {
            return;
        };
        if now >= reg.expiration_ms {
            self.remove(env, key, effects);
            return;
        }
        let level = reg.permission_level;
        let permitted: Vec<Location> = fine
            .into_iter()
            .map(|l| self.permitted(env, l, level))
            .collect();
        let reg = &self.registrations[&key];
        let mut previous = reg.last_delivered.clone();
        let jitter = ((FASTEST_INTERVAL_JITTER_PERCENTAGE * reg.request.interval_ms as f64) as i64)
            .min(MAX_FASTEST_INTERVAL_JITTER_MS);
        let min_interval = reg.request.min_update_interval_ms();
        let min_distance = reg.request.min_update_distance_m as f64;
        let mut result = Vec::new();
        for l in permitted {
            if l.latitude.is_nan()
                || !(-90.0..=90.0).contains(&l.latitude)
                || l.longitude.is_nan()
                || !(-180.0..=180.0).contains(&l.longitude)
            {
                continue;
            }
            if let Some(prev) = &previous {
                let delta = l.elapsed_realtime_ms() - prev.elapsed_realtime_ms();
                if delta < min_interval - jitter {
                    continue;
                }
                if min_distance > 0.0 && l.distance_to(prev) as f64 <= min_distance {
                    continue;
                }
            }
            previous = Some(l.clone());
            result.push(l);
        }
        if result.is_empty() {
            return;
        }
        if !env.note_op_no_throw(as_app_op(level), &reg.identity) {
            return;
        }
        let reg = self.registrations.get_mut(&key).unwrap();
        reg.last_delivered = result.last().cloned();
        let ok = match &reg.transport {
            Transport::Listener(l) => send_locations(l, &result),
            Transport::PendingIntent(pi) => env.send_locations_intent(pi, &result),
            Transport::Internal(f) => {
                effects
                    .internal
                    .push((f.clone(), result.last().unwrap().clone()));
                true
            }
            Transport::Current(_) => unreachable!("delivered by deliver_current"),
        };
        if !ok {
            self.remove(env, key, effects);
            return;
        }
        let reg = self.registrations.get_mut(&key).unwrap();
        reg.delivered += 1;
        if reg.delivered >= reg.request.max_updates {
            self.remove(env, key, effects);
        }
    }

    /// `GetCurrentLocationListenerRegistration.acceptLocationChange` and
    /// its delivery: one location or null, then it goes.
    pub fn deliver_current(
        &mut self,
        env: &Env,
        key: Key,
        fine: Option<Vec<Location>>,
        effects: &mut Effects,
    ) {
        let now = env.now_ms();
        let Some(reg) = self.registrations.get(&key) else {
            return;
        };
        let mut fine = fine.and_then(|v| v.last().cloned());
        if now >= reg.expiration_ms {
            fine = None;
        }
        let level = reg.permission_level;
        let identity = reg.identity.clone();
        if fine.is_some() && !env.note_op_no_throw(as_app_op(level), &identity) {
            fine = None;
        }
        let location = fine.map(|l| self.permitted(env, l, level));
        if let Some(Transport::Current(cb)) = self.registrations.get(&key).map(|r| &r.transport) {
            let mut data = Parcel::new();
            location_callback::OnLocation {
                location: location.clone(),
            }
            .write(&mut data);
            let _ = cb.transact(location_callback::ON_LOCATION, &data, true);
        }
        self.remove(env, key, effects);
    }

    /// `Registration.flush`: false if `key` is not registered. The flush
    /// completes once the provider in use has sent what it queued: at
    /// once for a test provider or one that queues nothing, else when
    /// `deferred` runs (not under the service's lock).
    pub fn flush(
        &self,
        env: &Env,
        key: Key,
        request_code: i32,
        deferred: impl FnOnce() -> Box<dyn FnOnce() + Send>,
    ) -> bool {
        if !self.registrations.contains_key(&key) {
            return false;
        }
        match &self.real {
            Some(real) if self.mock.is_none() && real.defers_flush() => real.flush(deferred()),
            _ => self.flush_complete(env, key, request_code),
        }
        true
    }

    /// `deliverOnFlushComplete(requestCode)` to `key`'s client, if still
    /// registered.
    pub fn flush_complete(&self, env: &Env, key: Key, request_code: i32) {
        let Some(reg) = self.registrations.get(&key) else {
            return;
        };
        match &reg.transport {
            Transport::Listener(l) => {
                let mut data = Parcel::new();
                location_listener::OnFlushComplete { request_code }.write(&mut data);
                let _ = l.transact(location_listener::ON_FLUSH_COMPLETE, &data, true);
            }
            Transport::PendingIntent(pi) => {
                env.send_intent(
                    pi,
                    &Intent {
                        action: None,
                        flags: 0,
                        extras: vec![(KEY_FLUSH_COMPLETE, Value::Int(request_code))],
                    },
                );
            }
            Transport::Current(_) | Transport::Internal(_) => {}
        }
    }
}

/// `ILocationListener.onLocationChanged(locations, null)`, one-way; false
/// if the client is gone.
fn send_locations(listener: &Strong, locations: &[Location]) -> bool {
    use aim_service_aidl::WriteParcelable;
    let mut data = Parcel::new();
    data.write_interface_token(location_listener::DESCRIPTOR);
    // `writeTypedList`: the size, then each non-null with a 1 before it.
    data.write_i32(locations.len() as i32);
    for l in locations {
        data.write_i32(1);
        l.write_to(&mut data);
    }
    data.write_binder(None); // no completion callback
    listener
        .transact(location_listener::ON_LOCATION_CHANGED, &data, true)
        .is_ok()
}

/// `deliverOnProviderEnabledChanged`; false if the client is gone.
fn deliver_enabled(env: &Env, provider: &str, reg: &Registration, enabled: bool) -> bool {
    match &reg.transport {
        Transport::Listener(l) => {
            let mut data = Parcel::new();
            location_listener::OnProviderEnabledChanged {
                provider: Some(provider.into()),
                enabled,
            }
            .write(&mut data);
            l.transact(location_listener::ON_PROVIDER_ENABLED_CHANGED, &data, true)
                .is_ok()
        }
        Transport::PendingIntent(pi) => env.send_intent(
            pi,
            &Intent {
                action: None,
                flags: 0,
                extras: vec![(KEY_PROVIDER_ENABLED, Value::Bool(enabled))],
            },
        ),
        Transport::Current(_) | Transport::Internal(_) => true,
    }
}

/// The extras of a location delivery by pending intent.
pub fn location_extras<'a>(locations: &'a [Location]) -> Vec<(&'static str, Value<'a>)> {
    let last: &Location = locations.last().expect("a location");
    let mut extras = vec![(
        KEY_LOCATION_CHANGED,
        Value::Parcelable(LOCATION_CLASS, last),
    )];
    if locations.len() > 1 {
        extras.push((
            KEY_LOCATIONS,
            Value::Parcelables(
                LOCATION_CLASS,
                locations
                    .iter()
                    .map(|l| l as &dyn aim_service_aidl::WriteParcelable)
                    .collect(),
            ),
        ));
    }
    extras
}
