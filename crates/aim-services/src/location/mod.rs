//! `location` (ILocationManager): the location service as a native
//! service (ADR 0013), in place of SystemServer's `LocationManagerService`,
//! following it at the pinned tag.
//!
//! The Mac owns location: the providers are backed by CoreLocation
//! ([`mac`]) with the original providers' names and properties, and the
//! rest is the original's: registrations and their merged requests per
//! provider ([`provider`]), coarse locations ([`fudger`]), test
//! providers, GNSS listeners ([`gnss`]), proximity alerts ([`geofence`]),
//! the location setting per user, and the permission, app op, foreground
//! and settings checks of its injector ([`env`]).

mod api;
mod bridge;
pub mod env;
mod fudger;
mod geofence;
mod gnss;
mod mac;
pub mod parcels;
mod provider;
mod proxy;
mod s2;
mod timer;
mod watch;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    android_location_ilocationmanager as ilm,
    android_location_provider_iproviderrequestlistener as request_listener,
    android_os_icancellationsignal as cancellation,
    com_android_internal_os_iresultreceiver as result_receiver,
};

use crate::settings::Settings;
use crate::system::System;
use env::{Env, Identity, PERMISSION_FINE, SYSTEM_UID};
use parcels::{
    ACCURACY_FINE, LastLocationRequest, Location, POWER_USAGE_HIGH, POWER_USAGE_LOW,
    ProviderProperties,
};
use provider::{
    Effects, GPS, Key, PASSIVE, ProviderManager, ProviderState, Registration, Transport,
};

/// The app ops whose modes decide location access (`AppOpsManager.OP_*`):
/// their mode watchers feed the mirror ([`crate::system`]).
pub const APP_OPS: [i32; 3] = [0, 1, OP_MOCK_LOCATION];
const OP_MOCK_LOCATION: i32 = 58;

const NETWORK: &str = "network";
const FUSED: &str = "fused";

/// The attribution tags of the original's contexts.
const ATTRIBUTION_TAG: &str = "LocationService";
const GNSS_ATTRIBUTION_TAG: &str = "GnssService";
const GEOFENCE_ATTRIBUTION_TAG: &str = "GeofencingService";

const LOCATION_BYPASS: &str = "android.permission.LOCATION_BYPASS";
const LOCATION_HARDWARE: &str = "android.permission.LOCATION_HARDWARE";
const UPDATE_DEVICE_STATS: &str = "android.permission.UPDATE_DEVICE_STATS";
const UPDATE_APP_OPS_STATS: &str = "android.permission.UPDATE_APP_OPS_STATS";
const WRITE_SECURE_SETTINGS: &str = "android.permission.WRITE_SECURE_SETTINGS";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const READ_DEVICE_CONFIG: &str = "android.permission.READ_DEVICE_CONFIG";
const ACCESS_LOCATION_EXTRA_COMMANDS: &str = "android.permission.ACCESS_LOCATION_EXTRA_COMMANDS";
const CONTROL_AUTOMOTIVE_GNSS: &str = "android.permission.CONTROL_AUTOMOTIVE_GNSS";

/// Compat changes (`LocationManager`, `LocationRequest`).
const BLOCK_PENDING_INTENT_SYSTEM_API_USAGE: i64 = 169887240;
const LOW_POWER_EXCEPTIONS: i64 = 168936375;

/// `LocationManager.MODE_CHANGED_ACTION`.
const MODE_CHANGED_ACTION: &str = "android.location.MODE_CHANGED";
const EXTRA_LOCATION_ENABLED: &str = "android.location.extra.LOCATION_ENABLED";
/// `Settings.Secure.LOCATION_MODE_ON` and `_OFF`.
const LOCATION_MODE_ON: &str = "3";
const LOCATION_MODE_OFF: &str = "0";
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");
/// The key of the geofence manager's fused request.
const GEOFENCE_KEY: Key = Key::Internal(0);

type Result<T> = std::result::Result<T, Exception>;

fn bad_parcel(status: i32) -> Exception {
    Exception::illegal_argument(format!("bad parcel: status {status}"))
}

fn null(what: &str) -> Exception {
    Exception::new(EX_NULL_POINTER, what)
}

/// The properties of the original's own providers (`GnssLocationProvider`,
/// `PassiveLocationProvider`).
const GPS_PROPERTIES: ProviderProperties = ProviderProperties {
    has_satellite_requirement: true,
    has_altitude_support: true,
    has_speed_support: true,
    has_bearing_support: true,
    ..ProviderProperties::with(POWER_USAGE_HIGH, ACCURACY_FINE)
};
const PASSIVE_PROPERTIES: ProviderProperties =
    ProviderProperties::with(POWER_USAGE_LOW, ACCURACY_FINE);

/// The passive provider: it reports what the others report.
struct Passive(ProviderState);

impl provider::RealProvider for Passive {
    fn state(&self) -> ProviderState {
        self.0.clone()
    }

    fn set_request(&self, _: &parcels::ProviderRequest) {}
}

struct Inner {
    /// In the original's order: passive, network, fused, gps, then the
    /// test providers.
    providers: Vec<ProviderManager>,
    extra_controller: Option<String>,
    extra_controller_enabled: bool,
    gnss: gnss::Gnss,
    geofences: geofence::Geofences,
    /// The deprecated GNSS batching's listener (`startGnssBatch`).
    batching: Option<u32>,
    /// Alarms set for registrations' expiration and delayed requests.
    alarms: HashMap<(String, Option<Key>), (i64, u64)>,
    /// The providers bound from apps.
    proxies: HashMap<&'static str, Arc<proxy::ProxyProvider>>,
    /// Whether a geocode provider resolves, and its service once bound.
    geocoder: Option<Option<Arc<Strong>>>,
    density: Option<Arc<proxy::Density>>,
}

impl Inner {
    fn index(&self, name: &str) -> Option<usize> {
        self.providers.iter().position(|m| m.name == name)
    }

    fn manager(&mut self, name: &str) -> Option<&mut ProviderManager> {
        self.providers.iter_mut().find(|m| m.name == name)
    }
}

pub struct LocationManagerService {
    env: Arc<Env>,
    inner: Mutex<Inner>,
    fg: timer::Executor,
    alarms: timer::Alarms,
    /// The token of the service's app op user restrictions (the original
    /// passes itself).
    restriction_token: Binder,
    /// Whether the listeners of [`watch`] are registered.
    watching: AtomicBool,
    /// The system_server bridge, once attached.
    bridge: Mutex<Option<Arc<bridge::Bridge>>>,
    /// The service's `ILocationHost`, which the bridge is handed.
    host: Binder,
    /// The Mac's location, which the gps provider serves.
    source: Arc<mac::MacSource>,
    this: Weak<LocationManagerService>,
}

/// Who called.
#[derive(Clone, Copy)]
struct Caller {
    pid: i32,
    uid: i32,
}

impl Caller {
    fn user_id(&self) -> i32 {
        self.uid / 100_000
    }
}

fn now_ns() -> i64 {
    aim_hostcall::clock::boottime_ns()
}

fn now_ms() -> i64 {
    now_ns() / 1_000_000
}

impl LocationManagerService {
    pub fn new(
        process: Arc<LocalProcess>,
        system: Arc<System>,
        settings: Arc<Settings>,
    ) -> Arc<Self> {
        let env = Arc::new(Env::new(process.clone(), system, settings));
        let restriction_token = process.add_service(Arc::new(Token));
        let service = Arc::new_cyclic(|this: &Weak<Self>| {
            let weak = this.clone();
            let source = mac::MacSource::new(
                Box::new(move |name, location| {
                    if let Some(service) = weak.upgrade() {
                        service.report(name, vec![location]);
                    }
                }),
                now_ns,
            );
            // The passive provider first; the others when the system says
            // which resolve (`onSystemThirdPartyAppsCanStart`).
            let mut passive = ProviderManager::new(PASSIVE, false, Vec::new(), &env);
            passive.real = Some(Arc::new(Passive(ProviderState {
                allowed: true,
                properties: Some(PASSIVE_PROPERTIES),
                identity: Some(env.own_identity(ATTRIBUTION_TAG)),
                extra_attribution_tags: Vec::new(),
            })));
            let providers = vec![passive];
            let host = process.add_service(Arc::new(bridge::Host {
                service: this.clone(),
            }));
            LocationManagerService {
                env,
                inner: Mutex::new(Inner {
                    providers,
                    extra_controller: None,
                    extra_controller_enabled: false,
                    gnss: gnss::Gnss::default(),
                    geofences: geofence::Geofences::default(),
                    batching: None,
                    alarms: HashMap::new(),
                    proxies: HashMap::new(),
                    geocoder: None,
                    density: None,
                }),
                fg: timer::Executor::new("location-fg"),
                alarms: timer::Alarms::new(now_ms),
                restriction_token,
                watching: AtomicBool::new(false),
                bridge: Mutex::new(None),
                host,
                source,
                this: this.clone(),
            }
        });
        let weak = Arc::downgrade(&service);
        service
            .env
            .system
            .add_bridge_listener(Box::new(move |handle| {
                if let Some(service) = weak.upgrade() {
                    service.attach(handle);
                }
            }));
        service
    }

    /// Runs `f` under the service's lock, then what every operation ends
    /// with (the buffered `updateService` of each provider, alarms, the
    /// GNSS session), and after the lock the effects.
    fn with<T>(&self, f: impl FnOnce(&mut Inner, &Env, &mut Effects) -> T) -> T {
        let mut effects = Effects::default();
        let result = {
            let mut inner = self.inner.lock().unwrap();
            let result = f(&mut inner, &self.env, &mut effects);
            self.settle(&mut inner, &mut effects);
            result
        };
        self.apply(effects);
        result
    }

    fn settle(&self, inner: &mut Inner, effects: &mut Effects) {
        let env = &*self.env;
        for m in &mut inner.providers {
            m.update_service(env, effects);
        }
        // The gps provider's request is the HAL session.
        if let Some((_, request)) = effects.requests.iter().rev().find(|(n, _)| n == GPS) {
            let navigating = request.is_active();
            let active = gnss_caller_active(inner, env, effects);
            inner.gnss.set_navigating(env, navigating, &active);
        }
        env.set_provider_identities(
            inner
                .providers
                .iter()
                .filter_map(ProviderManager::identity)
                .collect(),
        );
        self.schedule(inner);
    }

    /// Sets the alarms of registrations' expirations and delayed
    /// requests, and drops those no longer wanted.
    fn schedule(&self, inner: &mut Inner) {
        let mut wanted: HashMap<(String, Option<Key>), i64> = HashMap::new();
        for m in &inner.providers {
            if let Some((_, due)) = &m.delayed {
                wanted.insert((m.name.clone(), None), *due);
            }
            for key in m.keys() {
                if let Some(due) = m.expiration(key) {
                    wanted.insert((m.name.clone(), Some(key)), due);
                }
            }
        }
        inner.alarms.retain(|k, (due, id)| {
            let keep = wanted.get(k) == Some(due);
            if !keep {
                self.alarms.cancel(*id);
            }
            keep
        });
        for (k, due) in wanted {
            if inner.alarms.contains_key(&k) {
                continue;
            }
            let this = self.this.clone();
            let (name, key) = k.clone();
            let id = self.alarms.set(due, move || {
                if let Some(service) = this.upgrade() {
                    service.alarm(&name, key);
                }
            });
            inner.alarms.insert(k, (due, id));
        }
    }

    fn alarm(&self, name: &str, key: Option<Key>) {
        self.with(|inner, env, effects| {
            inner.alarms.remove(&(name.to_string(), key));
            let Some(m) = inner.manager(name) else { return };
            match key {
                None => {
                    if m.delayed
                        .as_ref()
                        .is_some_and(|(_, due)| *due <= env.now_ms())
                    {
                        m.delayed_due(effects);
                    }
                }
                Some(key) => m.expire(env, key, effects),
            }
        });
    }

    /// What the operations leave to do without the lock: broadcasts,
    /// the service's own listeners, provider request listeners, and app
    /// op restrictions.
    fn apply(&self, effects: Effects) {
        let Effects {
            broadcasts,
            internal,
            requests,
            state_changed,
            refresh_restrictions,
        } = effects;
        let tag_uids: Vec<i32> = state_changed
            .iter()
            .flat_map(|(old, new)| [old, new])
            .filter_map(|i| i.as_ref().map(|i| i.uid))
            .collect();
        let env = self.env.clone();
        let this = self.this.clone();
        self.fg.post(move || {
            for (intent, user) in broadcasts {
                env.broadcast(&|p| intent.write(p), user);
            }
            for (f, l) in internal {
                f(l);
            }
            if let Some(service) = this.upgrade() {
                for (name, request) in requests {
                    service.tell_request_listeners(&name, &request);
                }
                if refresh_restrictions {
                    service.refresh_app_ops_restrictions(env::USER_ALL);
                }
                if !tag_uids.is_empty() {
                    let mut uids = tag_uids;
                    uids.sort_unstable();
                    uids.dedup();
                    service.push_package_tags(Some(uids));
                }
            }
        });
    }

    /// `IProviderRequestListener.onProviderRequestChanged`, dropping
    /// listeners that are gone.
    fn tell_request_listeners(&self, name: &str, request: &parcels::ProviderRequest) {
        let listeners = {
            let mut inner = self.inner.lock().unwrap();
            match inner.manager(name) {
                Some(m) => m.request_listeners.clone(),
                None => return,
            }
        };
        let mut gone = Vec::new();
        for l in listeners {
            let mut data = Parcel::new();
            request_listener::OnProviderRequestChanged {
                provider: Some(name.into()),
                request: Some(request.clone()),
            }
            .write(&mut data);
            if l.transact(request_listener::ON_PROVIDER_REQUEST_CHANGED, &data, true)
                .is_err()
            {
                gone.push(l.handle);
            }
        }
        if !gone.is_empty() {
            let mut inner = self.inner.lock().unwrap();
            if let Some(m) = inner.manager(name) {
                m.request_listeners.retain(|l| !gone.contains(&l.handle));
            }
        }
    }

    /// `refreshAppOpsRestrictions(userId)`: with location off for a user,
    /// app ops for location are restricted but for the providers and the
    /// allowlisted.
    fn refresh_app_ops_restrictions(&self, user: i32) {
        let env = &*self.env;
        let users = if user == env::USER_ALL {
            env.running_users()
        } else {
            vec![user]
        };
        let providers: Vec<Identity> = {
            let inner = self.inner.lock().unwrap();
            inner
                .providers
                .iter()
                .filter_map(ProviderManager::identity)
                .collect()
        };
        for user in users {
            let enabled = env.location_enabled(user);
            let allowed = (!enabled).then(|| {
                let mut list = parcels::PackageTagsList::default();
                for i in &providers {
                    list.add(&i.package, i.attribution_tag.as_deref());
                }
                list.add_list(&env.ignore_settings_allowlist());
                list.add_list(&env.adas_allowlist());
                list
            });
            for op in [provider::OP_COARSE_LOCATION, provider::OP_FINE_LOCATION] {
                env.set_user_restriction(
                    op,
                    !enabled,
                    self.restriction_token,
                    allowed.as_ref(),
                    user,
                );
            }
        }
    }

    /// A provider reported `locations` (the Mac's, or a test provider's).
    fn report(&self, name: &str, locations: Vec<Location>) {
        self.with(|inner, env, effects| {
            report_locations(inner, env, name, locations, effects);
        });
    }

    /// `onLocationModeChanged(userId)` and each provider's
    /// `onLocationEnabledChanged`: the location setting of `user` changed.
    pub fn on_location_mode_changed(&self, user: i32) {
        if let Some(bridge) = self.bridge.lock().unwrap().clone() {
            bridge.invalidate_location_enabled_cache();
        }
        let enabled = self.env.location_enabled(user);
        let env = self.env.clone();
        self.fg.post(move || {
            env.broadcast(
                &|p| {
                    use aim_service_aidl::WriteParcelable;
                    parcels::Intent {
                        action: Some(MODE_CHANGED_ACTION),
                        flags: parcels::FLAG_RECEIVER_REGISTERED_ONLY
                            | parcels::FLAG_RECEIVER_FOREGROUND,
                        extras: vec![(EXTRA_LOCATION_ENABLED, parcels::Value::Bool(enabled))],
                    }
                    .write_to(p)
                },
                user,
            );
        });
        self.refresh_app_ops_restrictions(user);
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.on_enabled_changed(env, user, effects);
            }
            update_geofences(inner, env, effects, self);
        });
    }

    /// system_server attached its bridge (at boot, and again after it
    /// restarted): the location bridge is asked for, settings observed,
    /// the system configuration read, the client cache invalidated
    /// (`Lifecycle.onStart`) and the running users' app op restrictions
    /// set (`onUserStarting`).
    fn attach(&self, handle: u32) {
        let strong = self.env.process.strong(handle);
        let Some(this) = self.this.upgrade() else {
            return;
        };
        self.fg.post(move || {
            use aim_service_aidl::dev_aim_server_ibridge as ib;
            let mut data = Parcel::new();
            ib::GetLocationBridge {
                host: Some(this.host),
            }
            .write(&mut data);
            let reply = match strong.transact(ib::GET_LOCATION_BRIDGE, &data, false) {
                Ok(reply) => reply,
                Err(s) => return eprintln!("location: no location bridge: status {s}"),
            };
            let Ok(Ok(Some(Binder::Handle(h)))) =
                ib::read_get_location_bridge_reply(&mut reply.reader())
            else {
                return eprintln!("location: no location bridge");
            };
            let bridge = Arc::new(bridge::Bridge {
                strong: this.env.process.strong(h),
            });
            drop(reply);
            let weak = Arc::downgrade(&this);
            this.env.process.link_to_death(
                &bridge.strong,
                Box::new(move || {
                    if let Some(service) = weak.upgrade() {
                        service.bridge.lock().unwrap().take();
                        service.env.users.lock().unwrap().take();
                        service.watching.store(false, Ordering::Release);
                        // system_server's bindings went with it.
                        let s = service.clone();
                        service.fg.post(move || {
                            for name in [NETWORK, FUSED, "geocoder", "density"] {
                                s.provider_bound(name, None);
                            }
                        });
                    }
                }),
            );
            *this.bridge.lock().unwrap() = Some(bridge.clone());
            *this.env.users.lock().unwrap() = Some(env::Users {
                current: this.env.ask_current_user(),
                running: this.env.ask_running_users(),
                visible: HashMap::new(),
            });
            bridge.observe(&this);
            *this.env.system_config.lock().unwrap() = bridge.system_config();
            bridge.invalidate_location_enabled_cache();
            this.ensure_watchers();
            this.refresh_app_ops_restrictions(env::USER_ALL);
            this.push_package_tags(None);
        });
    }

    /// `onSystemThirdPartyAppsCanStart`: the providers are added in the
    /// original's order (network and fused if their services resolve,
    /// then gps), and the geocode and population density providers.
    fn providers_resolved(&self, resolved: &[String]) {
        let has = |name: &str| resolved.iter().any(|r| r == name);
        self.with(|inner, env, effects| {
            for name in [NETWORK, FUSED] {
                if !has(name) || inner.index(name).is_some() {
                    continue;
                }
                let proxy = proxy::ProxyProvider::new(name, env.process.clone(), self.this.clone());
                inner.proxies.insert(name, proxy.clone());
                let mut m = ProviderManager::new(name, true, Vec::new(), env);
                m.start(env, effects);
                m.set_real(env, proxy, effects);
                inner.providers.push(m);
            }
            if inner.index(GPS).is_none() {
                let mut m = ProviderManager::new(GPS, true, Vec::new(), env);
                m.start(env, effects);
                let gps = mac::MacProvider {
                    name: GPS,
                    state: ProviderState {
                        allowed: true,
                        properties: Some(GPS_PROPERTIES),
                        identity: Some(env.own_identity(GNSS_ATTRIBUTION_TAG)),
                        extra_attribution_tags: Vec::new(),
                    },
                    source: self.source.clone(),
                };
                m.set_real(env, Arc::new(gps), effects);
                inner.providers.push(m);
            }
            if has("geocoder") && inner.geocoder.is_none() {
                inner.geocoder = Some(None);
            }
            if has("density") && inner.density.is_none() {
                let density = proxy::Density::new(&env.process);
                // `setLocationFudgerCache`: the providers there are now.
                for m in &mut inner.providers {
                    m.density = Some(density.clone());
                }
                inner.density = Some(density);
            }
        });
    }

    /// A bound provider's service came (`onBind`) or went (`onUnbind`).
    fn provider_bound(&self, name: &str, bound: Option<(Arc<Strong>, String, Option<String>)>) {
        let (proxy, reset) = {
            let mut inner = self.inner.lock().unwrap();
            match name {
                "geocoder" => {
                    if inner.geocoder.is_some() {
                        inner.geocoder = Some(bound.map(|b| b.0));
                    }
                    return;
                }
                "density" => {
                    if let Some(d) = &inner.density {
                        let fetch = bound.is_some();
                        d.bind(bound.map(|b| b.0));
                        if fetch {
                            d.default_not_set();
                        }
                    }
                    return;
                }
                _ => match inner.proxies.get(name) {
                    Some(p) => (p.clone(), None::<u64>),
                    None => return,
                },
            }
        };
        let _ = reset;
        match bound {
            Some((binder, package, tags)) => proxy.bind(binder, package, tags),
            None => {
                if let Some(generation) = proxy.unbind() {
                    let this = self.this.clone();
                    let due = self.env.now_ms() + proxy::RESET_DELAY_MS;
                    self.alarms.set(due, move || {
                        if let Some(service) = this.upgrade()
                            && proxy.reset_due(generation)
                        {
                            service.provider_state_changed(proxy.name);
                        }
                    });
                }
            }
        }
    }

    /// A bound provider's service watched or not, off the service's lock
    /// (the bridge's call waits on system_server).
    fn set_provider_started(&self, name: &'static str, started: bool) {
        let this = self.this.clone();
        self.fg.post(move || {
            let Some(service) = this.upgrade() else {
                return;
            };
            let bridge = service.bridge.lock().unwrap().clone();
            if let Some(bridge) = bridge {
                bridge.set_provider_started(name, started);
            }
        });
    }

    /// A real provider's state changed (`onStateChanged`).
    fn provider_state_changed(&self, name: &str) {
        self.with(|inner, env, effects| {
            if let Some(m) = inner.manager(name) {
                m.on_state_changed(env, effects);
            }
            update_geofences(inner, env, effects, self);
        });
    }

    /// `LocationManagerInternal.isProvider(provider, identity)`.
    fn is_provider(&self, provider: Option<&str>, identity: &Identity) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.providers.iter().any(|m| {
            provider.is_none_or(|p| p == m.name) && m.identity().as_ref() == Some(identity)
        })
    }

    /// `calculateAppOpsLocationSourceTags(uid)` of `uids` (every provider's
    /// with none), told to the location package tags listener.
    fn push_package_tags(&self, uids: Option<Vec<i32>>) {
        let Some(bridge) = self.bridge.lock().unwrap().clone() else {
            return;
        };
        let tags: Vec<(i32, parcels::PackageTagsList)> = {
            let inner = self.inner.lock().unwrap();
            let states: Vec<ProviderState> =
                inner.providers.iter().map(|m| m.provider_state()).collect();
            let uids = uids.unwrap_or_else(|| {
                let mut all: Vec<i32> = states
                    .iter()
                    .filter_map(|s| s.identity.as_ref().map(|i| i.uid))
                    .collect();
                all.sort_unstable();
                all.dedup();
                all
            });
            uids.into_iter()
                .map(|uid| {
                    let mut list = parcels::PackageTagsList::default();
                    for state in &states {
                        let Some(i) = state.identity.as_ref().filter(|i| i.uid == uid) else {
                            continue;
                        };
                        for tag in &state.extra_attribution_tags {
                            list.add(&i.package, Some(tag));
                        }
                        if state.extra_attribution_tags.is_empty() || i.attribution_tag.is_some() {
                            list.add(&i.package, i.attribution_tag.as_deref());
                        }
                    }
                    (uid, list)
                })
                .collect()
        };
        for (uid, list) in tags {
            bridge.set_location_package_tags(uid, &list);
        }
    }

    /// An observed setting changed for `user`.
    fn on_setting_changed(&self, setting: bridge::Setting, user: i32) {
        match setting {
            bridge::Setting::LocationMode => self.on_location_mode_changed(user),
            bridge::Setting::BackgroundThrottle => self.with(|inner, env, effects| {
                for m in &mut inner.providers {
                    m.update_registrations(env, effects, |_| true);
                }
            }),
            bridge::Setting::PackageDenylist => self.with(|inner, env, effects| {
                for m in &mut inner.providers {
                    m.update_registrations(env, effects, |r| r.identity.user_id() == user);
                }
                update_geofences(inner, env, effects, self);
            }),
            bridge::Setting::LocationConfig => {
                self.refresh_app_ops_restrictions(env::USER_ALL);
                self.with(|inner, env, effects| {
                    for m in &mut inner.providers {
                        m.update_registrations(env, effects, |_| true);
                    }
                });
            }
        }
    }

    /// `onUserStarted`.
    fn on_user_started(&self, user: i32) {
        if let Some(users) = &mut *self.env.users.lock().unwrap()
            && !users.running.contains(&user)
        {
            users.running.push(user);
        }
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.on_user_started(env, user, effects);
            }
        });
        self.refresh_app_ops_restrictions(user);
    }

    /// `onUserStopped`.
    fn on_user_stopped(&self, user: i32) {
        if let Some(users) = &mut *self.env.users.lock().unwrap() {
            users.running.retain(|&u| u != user);
        }
        let mut inner = self.inner.lock().unwrap();
        for m in &mut inner.providers {
            m.on_user_stopped(user);
        }
    }

    /// `onCurrentUserChanged`: the registrations of both users' profiles
    /// are evaluated again.
    fn on_user_switching(&self, from: i32, to: i32) {
        if let Some(users) = &mut *self.env.users.lock().unwrap() {
            users.current = to;
        }
        let mut profiles = Vec::new();
        for user in [from, to] {
            profiles.extend(
                self.env
                    .system
                    .profile_ids(user)
                    .unwrap_or_else(|_| vec![user]),
            );
        }
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.update_registrations(env, effects, |r| profiles.contains(&r.identity.user_id()));
            }
            update_geofences(inner, env, effects, self);
        });
    }

    /// The user visibility listener.
    fn on_user_visibility_changed(&self, user: i32, visible: bool) {
        if let Some(users) = &mut *self.env.users.lock().unwrap() {
            users.visible.insert(user, visible);
        }
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.update_registrations(env, effects, |r| r.identity.user_id() == user);
            }
            update_geofences(inner, env, effects, self);
        });
    }

    /// The location power save mode, or the screen's interactive state,
    /// changed.
    fn on_power_state(&self, mode: Option<i32>, interactive: Option<bool>) {
        let power = {
            let mut power = self.env.power.lock().unwrap();
            if let Some(mode) = mode {
                power.mode = mode;
            }
            if let Some(i) = interactive {
                power.interactive = i;
            }
            *power
        };
        let screen_matters = match power.mode {
            env::LOCATION_MODE_GPS_DISABLED_WHEN_SCREEN_OFF => Some(GPS),
            env::LOCATION_MODE_THROTTLE_REQUESTS_WHEN_SCREEN_OFF
            | env::LOCATION_MODE_ALL_DISABLED_WHEN_SCREEN_OFF => None,
            _ if mode.is_none() => return,
            _ => None,
        };
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                if mode.is_some() || screen_matters.is_none_or(|n| n == m.name) {
                    m.update_registrations(env, effects, |_| true);
                }
            }
        });
    }

    /// `isResetableForPackage(packageName)`: whether the package has a
    /// registration or a GNSS listener.
    fn is_resetable(&self, package: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.providers.iter().any(|m| {
            m.keys().into_iter().any(|k| {
                m.registration(k)
                    .is_some_and(|r| r.identity.package == package)
            })
        }) || inner.gnss.has_package(package)
    }

    /// `onPackageReset(packageName)`.
    fn on_package_reset(&self, package: &str) {
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.package_reset(env, package, effects);
            }
            inner.gnss.package_reset(package);
        });
    }

    /// Registers the service's listeners once the system can take them
    /// (the original's helpers register at `onSystemReady`).
    fn ensure_watchers(&self) {
        if self.watching.load(Ordering::Acquire) {
            return;
        }
        if let Some(this) = self.this.upgrade()
            && watch::register(&this)
        {
            self.watching.store(true, Ordering::Release);
        }
    }

    /// `onAppForegroundChanged(uid, foreground)`.
    fn on_foreground_changed(&self, uid: i32, foreground: bool) {
        self.with(|inner, env, effects| {
            for m in &mut inner.providers {
                m.foreground_changed(env, effects, uid, foreground);
            }
        });
    }

    /// `onLocationPermissionsChanged(uid)` or `(packageName)` (every
    /// package when none is named).
    fn on_permissions_changed(&self, uid: Option<i32>, package: Option<String>) {
        self.with(|inner, env, effects| {
            let select = |i: &Identity| match (uid, &package) {
                (Some(uid), _) => i.uid == uid,
                (None, Some(p)) => &i.package == p,
                (None, None) => true,
            };
            for m in &mut inner.providers {
                m.permissions_changed(env, effects, select);
            }
            update_geofences(inner, env, effects, self);
        });
    }
}

/// The provider named `name`, if the caller can see it.
fn visible<'a>(
    inner: &'a mut Inner,
    env: &Env,
    name: &str,
    caller: Caller,
) -> Option<&'a mut ProviderManager> {
    inner
        .providers
        .iter_mut()
        .find(|m| m.name == name)
        .filter(|m| m.visible_to(env, caller.uid, caller.pid))
}

fn does_not_exist(name: &str) -> Exception {
    Exception::illegal_argument(format!("provider \"{name}\" does not exist"))
}

fn handle(binder: Option<Binder>) -> Option<u32> {
    match binder {
        Some(Binder::Handle(h)) => Some(h),
        _ => None,
    }
}

/// A provider's report, and the passive provider's copy of it.
fn report_locations(
    inner: &mut Inner,
    env: &Env,
    name: &str,
    locations: Vec<Location>,
    effects: &mut Effects,
) {
    let Some(m) = inner.manager(name) else { return };
    let gps_fix = name == GPS && !m.is_mock();
    if let Some(passive) = m.on_report_location(env, locations, effects)
        && let Some(p) = inner.manager(PASSIVE)
    {
        p.on_report_location(env, passive, effects);
    }
    if gps_fix {
        let active = gnss_caller_active(inner, env, effects);
        inner.gnss.on_fix(env, &active);
    }
}

/// The geofence manager's clients: location on for their user, a visible
/// user, a package not denied (`GeofenceManager.isActive`).
fn geofence_caller_active(env: &Env) -> impl Fn(&Identity) -> bool + '_ {
    move |i: &Identity| {
        if i.uid == SYSTEM_UID {
            return env.location_enabled(env.current_user());
        }
        env.location_enabled(i.user_id())
            && env.user_visible(i.user_id())
            && !env.package_blacklisted(i.user_id(), &i.package)
    }
}

/// The GNSS listeners' clients: the gps provider on for their user, a
/// visible user, a package not denied (`GnssListenerMultiplexer.isActive`).
fn gnss_caller_active<'a>(
    inner: &mut Inner,
    env: &'a Env,
    effects: &mut Effects,
) -> impl Fn(&Identity) -> bool + 'a {
    let current = env.current_user();
    let mut enabled: HashMap<i32, bool> = HashMap::new();
    if let Some(gps) = inner.manager(GPS) {
        for user in env.running_users().into_iter().chain([current]) {
            enabled.insert(user, gps.is_enabled(env, user, effects));
        }
    }
    move |i: &Identity| {
        if i.uid == SYSTEM_UID {
            return enabled.get(&current).copied().unwrap_or(false);
        }
        let user = i.user_id();
        enabled.get(&user).copied().unwrap_or(false)
            && env.user_visible(user)
            && !env.package_blacklisted(user, &i.package)
    }
}

/// `LocationManager.getLastLocation()` of the geofence manager: the fused
/// provider's.
fn fused_last_location(inner: &mut Inner, env: &Env, effects: &mut Effects) -> Option<Location> {
    let identity = env.own_identity(GEOFENCE_ATTRIBUTION_TAG);
    inner.manager(FUSED)?.get_last_location(
        env,
        &LastLocationRequest::default(),
        &identity,
        PERMISSION_FINE,
        effects,
    )
}

/// `GeofenceManager.updateService`: its fused request as the fences need
/// it, or none.
fn update_geofences(
    inner: &mut Inner,
    env: &Env,
    effects: &mut Effects,
    service: &LocationManagerService,
) {
    let last = inner.geofences.last_location.clone();
    let active = geofence_caller_active(env);
    let request = inner.geofences.request(env, &active, last.as_ref());
    let registered = inner.geofences.registered;
    let Some(fused) = inner.manager(FUSED) else {
        return;
    };
    match request {
        Some(request) => {
            if fused
                .registration(GEOFENCE_KEY)
                .is_some_and(|r| r.request() == &request)
            {
                return;
            }
            let this = service.this.clone();
            let deliver: provider::Deliver = Arc::new(move |l| {
                if let Some(service) = this.upgrade() {
                    service.on_geofence_location(l);
                }
            });
            let reg = Registration::new(
                Transport::Internal(deliver),
                request,
                env.own_identity(GEOFENCE_ATTRIBUTION_TAG),
                PERMISSION_FINE,
            );
            fused.put(env, GEOFENCE_KEY, reg, effects);
            inner.geofences.registered = true;
        }
        None => {
            if registered {
                fused.remove(env, GEOFENCE_KEY, effects);
                inner.geofences.registered = false;
                inner.geofences.last_location = None;
            }
        }
    }
}

impl Service for LocationManagerService {
    fn descriptor(&self) -> &str {
        ilm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match self.dispatch(call) {
            Ok(Some(reply)) => Ok(reply),
            Ok(None) => Err(UNKNOWN_TRANSACTION),
            Err(exception) => {
                let mut reply = Parcel::new();
                reply.write_exception(&exception);
                Ok(reply)
            }
        }
    }

    fn accepts_fds(&self) -> bool {
        // A dump's fd.
        true
    }
}

/// The `ICancellationSignal` of a current-location request.
struct CancelCurrent {
    service: Weak<LocationManagerService>,
    provider: String,
    key: Key,
    /// Keeps the callback's handle this registration's until cancelled.
    callback: Arc<Strong>,
}

impl Service for CancelCurrent {
    fn descriptor(&self) -> &str {
        cancellation::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != cancellation::CANCEL {
            return Err(UNKNOWN_TRANSACTION);
        }
        let _ = &self.callback;
        if let Some(service) = self.service.upgrade() {
            service.with(|inner, env, effects| {
                if let Some(m) = inner.manager(&self.provider) {
                    m.remove(env, self.key, effects);
                }
            });
        }
        Ok(Parcel::new())
    }
}

/// A pending intent's cancel listener (`IResultReceiver`).
struct CancelReceiver {
    service: Weak<LocationManagerService>,
    provider: String,
    key: Key,
}

impl Service for CancelReceiver {
    fn descriptor(&self) -> &str {
        result_receiver::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != result_receiver::SEND {
            return Err(UNKNOWN_TRANSACTION);
        }
        let (service, provider, key) = (self.service.clone(), self.provider.clone(), self.key);
        if let Some(s) = service.upgrade() {
            let fg = s.fg.clone();
            fg.post(move || {
                if let Some(service) = service.upgrade() {
                    service.with(|inner, env, effects| {
                        if let Some(m) = inner.manager(&provider) {
                            m.remove(env, key, effects);
                        }
                    });
                }
            });
        }
        Ok(Parcel::new())
    }
}

/// A token binder: the owner of the service's app op restrictions.
struct Token;

impl Service for Token {
    fn descriptor(&self) -> &str {
        ""
    }

    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Err(UNKNOWN_TRANSACTION)
    }
}
