//! `media_router`'s route provider services for `MediaRouter2`
//! (`MediaRoute2ProviderWatcher`, `MediaRoute2ProviderServiceProxy` and
//! the user handler of `MediaRouter2ServiceImpl` at `android-16.0.0_r1`).
//! A user's handler starts with its first router: the watcher finds the
//! `MediaRoute2ProviderService`s (a self-scan-only one by its category;
//! without the mirroring flag none supports system media routing), and
//! each proxy is told the routers' composite discovery preference. A proxy
//! binds its service while an app scans for features (and it is not
//! self-scan-only, or its own package scans actively); as the keep-alive
//! prevention flag has it, it is not bound before, and not bound again when
//! its binding dies. Bound, it gives the service an
//! `IMediaRoute2ProviderServiceCallback` and the preference. The routes the
//! service publishes (checked as `ServiceCallbackStub` checks them, then
//! given its package and unique id) reach the routers as
//! `onProviderStateChangedOnHandler` and `dispatchUpdatesOnHandler` have
//! it: every route to a router with system routing permissions, the
//! providers' and the default route to the others, each filtered by the
//! route's visibility. Package notifications rescan the watcher, retaining
//! existing proxies in resolver order and stopping retired providers. No router request reaches a provider
//! (#1663), so a session it creates is not implemented; a session list it
//! reports empty, the release of a session it never had or the failure of
//! a request no one made changes nothing.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader,
    Result as ParcelResult,
};
use aim_service_aidl::android_media_imediaroute2providerservice as provider_service;
use aim_service_aidl::android_media_imediaroute2providerservicecallback as provider_callback;
use aim_service_aidl::android_media_imediarouter2 as router2;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::media_router::{Router, Routers};
use super::media_router2::{Preference, Route};
use super::services::HostConnection;
use super::{AppHost, reply};
use crate::clip::{self, Raw};
use crate::package::component_resolver::Info;
use crate::package::intent::Intent;
use crate::package::intent_filter::Plain;
use crate::package::uri::Uri;

/// `MediaRoute2ProviderService.SERVICE_INTERFACE`, `CATEGORY_SELF_SCAN_ONLY`.
const SERVICE_INTERFACE: &str = "android.media.MediaRoute2ProviderService";
const CATEGORY_SELF_SCAN_ONLY: &str = "android.media.MediaRoute2ProviderService.SELF_SCAN_ONLY";
/// `PackageManager.GET_RESOLVED_FILTER`.
const GET_RESOLVED_FILTER: i64 = 0x40;
/// `Context.BIND_AUTO_CREATE | Context.BIND_FOREGROUND_SERVICE`.
const BIND_FLAGS: i64 = 0x1 | 0x0400_0000;
/// `MediaRoute2Info.FLAG_ROUTING_TYPE_*`.
const FLAG_ROUTING_TYPE_SYSTEM: i32 = 0b011;
const FLAG_ROUTING_TYPE_ALL: i32 = 0b111;
/// `MediaRoute2Info.SUITABILITY_STATUS_NOT_SUITABLE_FOR_TRANSFER`.
const SUITABILITY_STATUS_NOT_SUITABLE_FOR_TRANSFER: i32 = 2;
/// `MediaRoute2Info.isSystemRouteType`'s types.
const SYSTEM_ROUTE_TYPES: [i32; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 19, 22, 23, 26, 29];
/// `MediaRoute2ProviderService.REQUEST_ID_NONE`.
const REQUEST_ID_NONE: i64 = 0;

type Job = Box<dyn FnOnce() + Send>;

/// `ComponentName.flattenToShortString`.
fn short_name(package: &str, class: &str) -> String {
    match class
        .strip_prefix(package)
        .filter(|rest| rest.starts_with('.'))
    {
        Some(rest) => format!("{package}/{rest}"),
        None => format!("{package}/{class}"),
    }
}

/// The parcel of `s` alone.
fn string16(s: &str) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_string16(Some(s));
    p.data().to_vec()
}

/// A `MediaRoute2Info` a provider published, kept as it came, with what
/// the service reads of it and where the fields it sets are.
#[derive(Clone, PartialEq)]
pub(super) struct ProviderRoute {
    raw: Raw,
    /// `mId`, the provider's own.
    id: String,
    has_name: bool,
    kind: i32,
    routing_type_flags: i32,
    is_system: bool,
    suitability: i32,
    visibility_restricted: bool,
    provider_package: Option<String>,
    allowed_packages: Vec<String>,
    provider_id: Option<String>,
    /// The spans of `mProviderPackageName` and `mProviderId`.
    provider_package_at: (usize, usize),
    provider_id_at: (usize, usize),
}

/// `readStringArray`, not null, as a set.
fn string_set<'a>(
    r: &mut Reader<'a>,
    read: impl Fn(&mut Reader<'a>) -> ParcelResult<Option<String>>,
) -> ParcelResult<Vec<String>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Err(BAD_VALUE);
    }
    (0..n).map(|_| Ok(read(r)?.unwrap_or_default())).collect()
}

impl ReadParcelable for ProviderRoute {
    /// `MediaRoute2Info(Parcel)`.
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let id = r
            .read_string16()?
            .filter(|id| !id.is_empty())
            .ok_or(BAD_VALUE)?;
        let has_name = clip::char_sequence(r)?.is_some_and(|n| !n.is_empty());
        aim_service_aidl::read_string_list(r)?; // features
        let kind = r.read_i32()?;
        // `validateRoutingTypeFlags`.
        let routing_type_flags = r.read_i32()?;
        if routing_type_flags == 0 || routing_type_flags & !FLAG_ROUTING_TYPE_ALL != 0 {
            return Err(BAD_VALUE);
        }
        let is_system = r.read_bool()?;
        // `readParcelable(null, Uri.class)`: its creator's name, then the URI.
        if r.read_string16()?.is_some() {
            Uri::read(r, &mut Plain)?;
        }
        clip::char_sequence(r)?; // description
        r.read_i32()?; // connection state
        r.read_string16()?; // client package
        let at = r.position() - start;
        let provider_package = r.read_string16()?;
        let provider_package_at = (at, r.position() - start);
        for _ in 0..3 {
            r.read_i32()?; // volume handling, maximum and value
        }
        r.read_string16()?; // address
        string_set(r, |r| r.read_string16())?; // deduplication ids
        clip::bundle(r)?; // extras
        let at = r.position() - start;
        let provider_id = r.read_string16()?;
        let provider_id_at = (at, r.position() - start);
        let visibility_restricted = r.read_bool()?;
        let allowed_packages = string_set(r, |r| r.read_string8())?;
        // The required permissions, not enforced without the visibility
        // control flag.
        for _ in 0..r.read_i32()?.max(0) {
            string_set(r, |r| r.read_string8())?;
        }
        let suitability = r.read_i32()?;
        Ok(ProviderRoute {
            raw: Raw::capture(r, start),
            id,
            has_name,
            kind,
            routing_type_flags,
            is_system,
            suitability,
            visibility_restricted,
            provider_package,
            allowed_packages,
            provider_id,
            provider_package_at,
            provider_id_at,
        })
    }
}

impl WriteParcelable for ProviderRoute {
    fn write_to(&self, p: &mut Parcel) {
        self.raw.write(p);
    }
}

impl ProviderRoute {
    /// `MediaRoute2Info.Builder(route).setProviderPackageName(package)
    /// .setProviderId(uniqueId)`: the later field first, so the earlier
    /// span stays where it was.
    fn with_provider(&self, package: &str, unique_id: &str) -> ProviderRoute {
        let (package_bytes, id_bytes) = (string16(package), string16(unique_id));
        let (pa, ia) = (self.provider_package_at, self.provider_id_at);
        let raw = self
            .raw
            .splice(ia.0, ia.1, &id_bytes)
            .splice(pa.0, pa.1, &package_bytes);
        let id_start = ia.0 + package_bytes.len() - (pa.1 - pa.0);
        ProviderRoute {
            raw,
            provider_package: Some(package.into()),
            provider_package_at: (pa.0, pa.0 + package_bytes.len()),
            provider_id: Some(unique_id.into()),
            provider_id_at: (id_start, id_start + id_bytes.len()),
            ..self.clone()
        }
    }

    /// `getId`: its provider's unique id and its own.
    fn unique_id(&self) -> String {
        match self.provider_id.as_deref().filter(|p| !p.is_empty()) {
            Some(provider) => format!("{provider}:{}", self.id),
            None => self.id.clone(),
        }
    }

    /// `isValid`.
    fn is_valid(&self) -> bool {
        self.has_name && self.provider_id.as_deref().is_some_and(|p| !p.is_empty())
    }

    /// `isVisibleTo`.
    fn is_visible_to(&self, package: &str) -> bool {
        !self.visibility_restricted
            || self.provider_package.as_deref() == Some(package)
            || self.allowed_packages.iter().any(|p| p == package)
    }

    /// `ServiceCallbackStub.notifyProviderUpdated`'s checks of a route,
    /// for a provider not allowed system media routes.
    fn check_published(&self) -> Result<(), Exception> {
        let refused = if self.is_system {
            "Only the system is allowed to publish system routes."
        } else if self.suitability == SUITABILITY_STATUS_NOT_SUITABLE_FOR_TRANSFER {
            "Only the system is allowed to set not suitable for transfer status."
        } else if SYSTEM_ROUTE_TYPES.contains(&self.kind) {
            "Only the system is allowed to publish routes with system route types."
        } else if self.routing_type_flags & FLAG_ROUTING_TYPE_SYSTEM != 0 {
            "This provider is not allowed to publish routes that support system media routing."
        } else {
            return Ok(());
        };
        Err(Exception::security(format!(
            "{refused} Disallowed route: {}",
            self.id
        )))
    }
}

/// A `MediaRoute2ProviderInfo` as a provider sent it: its routes, by
/// their own id (`createTypedArrayMap`).
struct ProviderInfo(Vec<ProviderRoute>);

impl ReadParcelable for ProviderInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        r.read_string16()?; // its unique id, which the proxy sets
        let mut routes: Vec<(String, ProviderRoute)> = Vec::new();
        for _ in 0..r.read_i32()?.max(0) {
            let key = r.read_string16()?.unwrap_or_default();
            if r.read_i32()? == 0 {
                return Err(BAD_VALUE);
            }
            let route = ProviderRoute::read_from(r)?;
            match routes.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = route,
                None => routes.push((key, route)),
            }
        }
        Ok(ProviderInfo(
            crate::package::info::array_order(routes, |(k, _)| k)
                .into_iter()
                .map(|(_, r)| r)
                .collect(),
        ))
    }
}

/// The routers' composite `RouteDiscoveryPreference`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Composite {
    features: Vec<String>,
    active_scan: bool,
}

impl WriteParcelable for Composite {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.features.len() as i32);
        for f in &self.features {
            p.write_string16(Some(f));
        }
        p.write_i32(0); // package order
        p.write_i32(0); // allowed packages
        p.write_bool(self.active_scan);
        p.write_i32(-1); // extras: none from the builder
    }
}

/// `buildCompositeDiscoveryPreference` of the active routers, in their
/// registration order: their features (a `HashSet`'s, empty ones dropped
/// by the builder), and the packages scanning actively (with features, as
/// manager scans are prevented).
fn composite(routers: &[(String, Preference)]) -> (Composite, Vec<String>) {
    let mut features: Vec<(String, ())> = Vec::new();
    let mut actively = Vec::new();
    for (package, preference) in routers {
        for f in &preference.features {
            if !features.iter().any(|(g, _)| g == f) {
                features.push((f.clone(), ()));
            }
        }
        if preference.active_scan && !preference.features.is_empty() && !actively.contains(package)
        {
            actively.push(package.clone());
        }
    }
    let features = super::device_config::hash_map_order(features, 16)
        .into_iter()
        .map(|(f, ())| f)
        .filter(|f| !f.is_empty())
        .collect();
    (
        Composite {
            features,
            active_scan: !actively.is_empty(),
        },
        actively,
    )
}

/// A route a router is told of: the system provider's or a provider's.
pub(super) enum Told<'a> {
    System(&'a Route),
    Provider(&'a ProviderRoute),
}

impl WriteParcelable for Told<'_> {
    fn write_to(&self, p: &mut Parcel) {
        match self {
            Told::System(r) => r.write_to(p),
            Told::Provider(r) => r.write_to(p),
        }
    }
}

/// `RouterRecord.getVisibleRoutes` of `routes` for `package`.
pub(super) fn visible<'a>(
    routes: impl IntoIterator<Item = Told<'a>>,
    package: &str,
) -> Vec<Option<Told<'a>>> {
    routes
        .into_iter()
        .filter(|t| match t {
            Told::System(_) => true,
            Told::Provider(r) => r.is_visible_to(package),
        })
        .map(Some)
        .collect()
}

/// A connection to a provider service: its binder and death link.
struct Connection {
    id: u64,
    service: Arc<Strong>,
    death: u64,
}

#[derive(Default)]
struct ProxyState {
    running: bool,
    bound: bool,
    active: Option<Connection>,
    ready: bool,
    /// `mLastDiscoveryPreference`, `mLastDiscoveryPreferenceIncludesThisPackage`.
    last: Option<Composite>,
    includes_package: bool,
    /// `mProviderInfo`'s routes, given their provider, and their binders.
    info: Option<Vec<ProviderRoute>>,
    held: Vec<Strong>,
}

/// A `MediaRoute2ProviderServiceProxy`.
struct Proxy {
    package: String,
    class: String,
    /// `mUniqueId`.
    unique_id: String,
    self_scan_only: bool,
    user: i32,
    connection: Arc<dyn HostConnection>,
    state: Mutex<ProxyState>,
}

/// The proxy's `ServiceConnection`, which posts what it is told.
struct ProxyConnection {
    providers: Weak<Providers>,
    proxy: Weak<Proxy>,
}

impl HostConnection for ProxyConnection {
    fn connected(&self, service: Option<Binder>, dead: bool) {
        let proxy = self.proxy.clone();
        let Some(providers) = self.providers.upgrade() else {
            return;
        };
        providers.post_with(move |p| {
            let Some(proxy) = proxy.upgrade() else { return };
            match service {
                Some(Binder::Handle(h)) => p.service_connected(&proxy, h),
                // `onBindingDied`.
                _ if dead => p.binding_died(&proxy),
                // `onServiceDisconnected`.
                _ => p.disconnect(&proxy),
            }
        });
    }
}

/// A user's `UserHandler` state: its proxies, the composite preference
/// and the providers' routes last told.
#[derive(Default)]
struct User {
    started: bool,
    proxies: Vec<Arc<Proxy>>,
    composite: Composite,
    /// `mActivelyScanningPackages`.
    actively: Vec<String>,
    /// The providers' entries of `mLastProviderInfos` (the system
    /// provider's is its own).
    infos: Vec<(String, Vec<ProviderRoute>)>,
    /// The providers' entries of `mLastNotifiedRoutesTo*Routers`, as put.
    routes: Vec<(String, ProviderRoute)>,
}

pub(super) struct Providers {
    host: Arc<AppHost>,
    me: Weak<Providers>,
    routers: Routers,
    handler: Mutex<mpsc::Sender<Job>>,
    users: Mutex<HashMap<i32, User>>,
    next_connection: AtomicU64,
}

impl super::package_writes::PackageMonitor for Providers {
    fn package_changed(
        &self,
        change: &super::package_writes::PackageChanged<'_>,
    ) -> Result<(), Exception> {
        let user = change.user;
        self.post_with(move |p| {
            if p.users
                .lock()
                .unwrap()
                .get(&user)
                .is_some_and(|u| u.started)
            {
                if let Err(e) = p.scan(user) {
                    eprintln!("app host: media_router: scanPackages: {e:?}");
                }
            }
        });
        Ok(())
    }
}

impl Providers {
    pub(super) fn new(host: Arc<AppHost>, routers: Routers) -> Arc<Providers> {
        let (send, receive) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("media_router2".into())
            .spawn(move || {
                for job in receive {
                    job();
                }
            })
            .expect("the media_router2 handler thread");
        let service = Arc::new_cyclic(|me| Providers {
            host,
            me: me.clone(),
            routers,
            handler: Mutex::new(send),
            users: Mutex::default(),
            next_connection: AtomicU64::new(1),
        });
        let monitor = service.clone() as Arc<dyn super::package_writes::PackageMonitor>;
        service
            .host
            .package_monitors
            .register(super::USER_ALL, Arc::downgrade(&monitor));
        service
    }

    /// Runs `f` on the handler's thread.
    fn post_with(&self, f: impl FnOnce(&Providers) + Send + 'static) {
        let me = self.me.clone();
        let job: Job = Box::new(move || {
            if let Some(p) = me.upgrade() {
                f(&p);
            }
        });
        let _ = self.handler.lock().unwrap().send(job);
    }

    /// `UserRecord`'s handler started (`UserHandler.start`, its watcher
    /// scanning); the user stays current, so it is never stopped.
    pub(super) fn start(&self, user: i32) {
        self.post_with(move |p| p.start_on_handler(user));
    }

    /// `updateDiscoveryPreferenceOnHandler` of `user`.
    pub(super) fn update_discovery(&self, user: i32) {
        self.post_with(move |p| p.update_discovery_on_handler(user));
    }

    /// The providers' routes `notifyRouterRegistered` gives first, in
    /// `mLastProviderInfos` order.
    pub(super) fn registered_routes(&self, user: i32) -> Vec<ProviderRoute> {
        let users = self.users.lock().unwrap();
        users
            .get(&user)
            .map(|u| {
                u.infos
                    .iter()
                    .flat_map(|(_, r)| r.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn start_on_handler(&self, user: i32) {
        {
            let mut users = self.users.lock().unwrap();
            let u = users.entry(user).or_default();
            if u.started {
                return;
            }
            u.started = true;
        }
        if let Err(e) = self.scan(user) {
            eprintln!("app host: media_router: scanPackages: {e:?}");
        }
    }

    /// `MediaRoute2ProviderWatcher.scanPackages`: a proxy started for each
    /// new service, then added (`onAddProviderService`: told the current
    /// preference).
    fn scan(&self, user: i32) -> Result<(), Exception> {
        let (_, resolution) = self.host.packages()?;
        let intent = Intent {
            action: Some(SERVICE_INTERFACE.into()),
            ..Default::default()
        };
        let found = resolution
            .query_intent_services(&intent, None, GET_RESOLVED_FILTER, user, 1000)
            .map_err(|e| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("media_router: queryIntentServices: {e:?}"),
                )
            })?;
        let mut remaining = self
            .users
            .lock()
            .unwrap()
            .entry(user)
            .or_default()
            .proxies
            .clone();
        let mut ordered = Vec::new();
        for r in found {
            let Info::Service(si) = &r.info else { continue };
            let package = si.info.item.package_name.clone().unwrap_or_default();
            let class = si.info.item.name.clone().unwrap_or_default();
            let self_scan_only = r
                .filter
                .as_ref()
                .and_then(|f| f.categories.as_ref())
                .is_some_and(|c| c.iter().any(|c| c == CATEGORY_SELF_SCAN_ONLY));
            let unique_id = short_name(&package, &class);
            if let Some(index) = remaining.iter().position(|p| p.unique_id == unique_id) {
                let proxy = remaining.remove(index);
                self.update_binding(&proxy);
                ordered.push(proxy);
                continue;
            }
            let providers = self.me.clone();
            let proxy = Arc::new_cyclic(|proxy| Proxy {
                unique_id,
                package,
                class,
                self_scan_only,
                user,
                connection: Arc::new(ProxyConnection {
                    providers,
                    proxy: proxy.clone(),
                }),
                state: Mutex::new(ProxyState {
                    running: true,
                    ..Default::default()
                }),
            });
            let (actively, composite) = {
                let mut users = self.users.lock().unwrap();
                let u = users.entry(user).or_default();
                (u.actively.clone(), u.composite.clone())
            };
            self.proxy_update_discovery(&proxy, actively.contains(&proxy.package), composite);
            ordered.push(proxy);
        }
        self.users.lock().unwrap().entry(user).or_default().proxies = ordered;
        for proxy in remaining {
            proxy.state.lock().unwrap().running = false;
            self.unbind(&proxy);
        }
        Ok(())
    }

    /// The routers of `user`, in their registration order.
    fn routers_of(&self, user: i32) -> Vec<Router> {
        let mut routers: Vec<Router> = self
            .routers
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.user == user)
            .cloned()
            .collect();
        routers.sort_by_key(|r| r.seq);
        routers
    }

    /// Every router is active: the app host's processes are all top
    /// (#1738), and no manager scans.
    fn update_discovery_on_handler(&self, user: i32) {
        let routers: Vec<(String, Preference)> = self
            .routers_of(user)
            .into_iter()
            .map(|r| (r.package, r.preference))
            .collect();
        let (composite, actively) = composite(&routers);
        let proxies = {
            let mut users = self.users.lock().unwrap();
            let u = users.entry(user).or_default();
            let same_set = u.actively.len() == actively.len()
                && u.actively.iter().all(|p| actively.contains(p));
            if u.composite == composite && same_set {
                return;
            }
            u.composite = composite.clone();
            u.actively = actively.clone();
            u.proxies.clone()
        };
        for proxy in proxies {
            self.proxy_update_discovery(
                &proxy,
                actively.contains(&proxy.package),
                composite.clone(),
            );
        }
    }

    /// `MediaRoute2ProviderServiceProxy.updateDiscoveryPreference`.
    fn proxy_update_discovery(
        &self,
        proxy: &Arc<Proxy>,
        includes_package: bool,
        preference: Composite,
    ) {
        let service = {
            let mut s = proxy.state.lock().unwrap();
            s.last = Some(preference.clone());
            s.includes_package = includes_package;
            s.active
                .as_ref()
                .filter(|_| s.ready)
                .map(|c| c.service.clone())
        };
        if let Some(service) = service {
            self.send_preference(proxy, &service, preference);
        }
        self.update_binding(proxy);
    }

    /// `Connection.updateDiscoveryPreference`.
    fn send_preference(&self, proxy: &Proxy, service: &Strong, preference: Composite) {
        let mut p = Parcel::new();
        provider_service::UpdateDiscoveryPreference {
            discovery_preference: Some(preference),
        }
        .write(&mut p);
        if service
            .transact(provider_service::UPDATE_DISCOVERY_PREFERENCE, &p, true)
            .is_err()
        {
            eprintln!(
                "app host: media_router: {}: updateDiscoveryPreference: Failed to deliver request.",
                proxy.unique_id
            );
        }
    }

    /// `shouldBind`: no session is kept, no manager scans and no provider
    /// supports system media routing.
    fn should_bind(proxy: &Proxy, s: &ProxyState) -> bool {
        let scanning = s.last.as_ref().is_some_and(|p| !p.features.is_empty());
        s.running && scanning && (s.includes_package || !proxy.self_scan_only)
    }

    fn update_binding(&self, proxy: &Arc<Proxy>) {
        let bind = Self::should_bind(proxy, &proxy.state.lock().unwrap());
        if bind {
            self.bind(proxy)
        } else {
            self.unbind(proxy)
        }
    }

    fn bind(&self, proxy: &Arc<Proxy>) {
        if proxy.state.lock().unwrap().bound {
            return;
        }
        let bound = self.host.bind_service_from_host(
            Some(SERVICE_INTERFACE),
            &proxy.package,
            &proxy.class,
            0,
            BIND_FLAGS,
            proxy.user,
            proxy.connection.clone(),
        );
        // A `SecurityException` leaves it unbound, as any failure here.
        let bound = bound.unwrap_or_else(|e| {
            eprintln!(
                "app host: media_router: {}: Bind failed: {e:?}",
                proxy.unique_id
            );
            false
        });
        proxy.state.lock().unwrap().bound = bound;
    }

    fn unbind(&self, proxy: &Arc<Proxy>) {
        {
            let mut s = proxy.state.lock().unwrap();
            if !s.bound {
                return;
            }
            s.bound = false;
        }
        self.disconnect(proxy);
        self.host.unbind_service_from_host(&proxy.connection);
    }

    /// `onServiceConnectedInternal`: a new connection given the callback
    /// (`Connection.register`), ready once the handler gets to it.
    fn service_connected(&self, proxy: &Arc<Proxy>, handle: u32) {
        if !proxy.state.lock().unwrap().bound {
            return;
        }
        self.disconnect(proxy);
        let id = self.next_connection.fetch_add(1, Ordering::Relaxed);
        let service = Arc::new(self.host.process.strong(handle));
        let weak = Arc::downgrade(proxy);
        let me = self.me.clone();
        let death = self.host.process.link_to_death(
            &service,
            Box::new(move || {
                // `Connection.binderDied`: `onConnectionDied`.
                let weak = weak.clone();
                if let Some(p) = me.upgrade() {
                    p.post_with(move |p| {
                        if let Some(proxy) = weak.upgrade() {
                            p.connection_died(&proxy, id);
                        }
                    });
                }
            }),
        );
        let callback = self.host.process.add_service(Arc::new(ProviderCallback {
            providers: self.me.clone(),
            proxy: Arc::downgrade(proxy),
            connection: id,
        }));
        let mut p = Parcel::new();
        provider_service::SetCallback {
            callback: Some(callback),
        }
        .write(&mut p);
        if service
            .transact(provider_service::SET_CALLBACK, &p, true)
            .is_err()
        {
            // `register` failing is `binderDied`.
            self.host.process.clear_death(&service, death);
            eprintln!(
                "app host: media_router: {}: Registration failed",
                proxy.unique_id
            );
            return;
        }
        proxy.state.lock().unwrap().active = Some(Connection { id, service, death });
        let weak = Arc::downgrade(proxy);
        self.post_with(move |p| {
            if let Some(proxy) = weak.upgrade() {
                p.connection_ready(&proxy, id);
            }
        });
    }

    /// `onConnectionReady`: the last preference sent again.
    fn connection_ready(&self, proxy: &Arc<Proxy>, id: u64) {
        let ready = {
            let mut s = proxy.state.lock().unwrap();
            if !s.active.as_ref().is_some_and(|c| c.id == id) {
                return;
            }
            s.ready = true;
            s.last.clone().map(|l| (l, s.includes_package))
        };
        if let Some((last, includes)) = ready {
            self.proxy_update_discovery(proxy, includes, last);
        }
    }

    fn connection_died(&self, proxy: &Arc<Proxy>, id: u64) {
        if proxy
            .state
            .lock()
            .unwrap()
            .active
            .as_ref()
            .is_some_and(|c| c.id == id)
        {
            self.disconnect(proxy);
        }
    }

    /// `onBindingDiedInternal`: unbound, and with the keep-alive
    /// prevention flag not bound again.
    fn binding_died(&self, proxy: &Arc<Proxy>) {
        self.unbind(proxy);
        eprintln!(
            "app host: media_router: Route provider service ({}) binding died, but we did not rebind.",
            proxy.unique_id
        );
    }

    /// `disconnect`: the connection disposed and the provider's state gone.
    fn disconnect(&self, proxy: &Arc<Proxy>) {
        let gone = {
            let mut s = proxy.state.lock().unwrap();
            s.ready = false;
            let gone = s.active.take();
            if gone.is_some() {
                s.info = None;
                s.held.clear();
            }
            gone
        };
        if let Some(c) = gone {
            self.host.process.clear_death(&c.service, c.death);
            self.provider_state_changed(proxy);
        }
    }

    /// `onProviderUpdated` of the connection `id`: `setProviderState`
    /// (`setUniqueId(package, uniqueId)`, not the system provider), then
    /// the change handled.
    fn provider_updated(
        &self,
        proxy: &Arc<Proxy>,
        id: u64,
        routes: Vec<ProviderRoute>,
        held: Vec<Strong>,
    ) {
        {
            let mut s = proxy.state.lock().unwrap();
            if !s.active.as_ref().is_some_and(|c| c.id == id) {
                return;
            }
            s.info = Some(
                routes
                    .iter()
                    .map(|r| r.with_provider(&proxy.package, &proxy.unique_id))
                    .collect(),
            );
            s.held = held;
        }
        self.provider_state_changed(proxy);
    }

    /// `onProviderStateChangedOnHandler` of a provider service, and
    /// `dispatchUpdatesOnHandler` if a route was added, changed or removed.
    fn provider_state_changed(&self, proxy: &Arc<Proxy>) {
        let new = proxy.state.lock().unwrap().info.clone();
        let changed = {
            let mut users = self.users.lock().unwrap();
            let u = users.entry(proxy.user).or_default();
            update_routes(u, &proxy.unique_id, new)
        };
        if changed {
            self.dispatch_routes(proxy.user);
        }
    }

    /// `dispatchUpdatesOnHandler` of a provider service's change: every
    /// route to the routers with system routing permissions, the
    /// providers' and the default route to the others.
    fn dispatch_routes(&self, user: i32) {
        let (privileged, default) = match self.host.media_router2_system_route_forms() {
            Ok(forms) => forms,
            Err(e) => return eprintln!("app host: media_router: the system routes: {e:?}"),
        };
        let routes: Vec<ProviderRoute> = self
            .users
            .lock()
            .unwrap()
            .get(&user)
            .map(|u| u.routes.iter().map(|(_, r)| r.clone()).collect())
            .unwrap_or_default();
        for router in self.routers_of(user) {
            let system = if router.system_routing {
                &privileged
            } else {
                &default
            };
            let told = visible(notified(system, &routes), &router.package);
            let mut p = Parcel::new();
            router2::NotifyRoutesUpdated { routes: Some(told) }.write(&mut p);
            if let Err(e) = router
                .binder
                .transact(router2::NOTIFY_ROUTES_UPDATED, &p, true)
            {
                eprintln!("app host: media_router: notifyRoutesUpdated: {e:?}");
            }
        }
    }
}

/// `mLastNotifiedRoutesTo*Routers`' values: an `ArrayMap` by unique id of
/// the system route (put first) and the providers' routes.
fn notified<'a>(system: &'a Route, routes: &'a [ProviderRoute]) -> Vec<Told<'a>> {
    let mut all: Vec<(String, Told<'a>)> = vec![(system.unique_id(), Told::System(system))];
    all.extend(routes.iter().map(|r| (r.unique_id(), Told::Provider(r))));
    crate::package::info::array_order(all, |(id, _)| id)
        .into_iter()
        .map(|(_, t)| t)
        .collect()
}

/// `onProviderStateChangedOnHandler`'s bookkeeping of the provider
/// `unique_id`'s new routes (none once gone): whether a route was added,
/// changed or removed.
fn update_routes(u: &mut User, unique_id: &str, new: Option<Vec<ProviderRoute>>) -> bool {
    let at = u.infos.iter().position(|(id, _)| id == unique_id);
    let old = match (at, &new) {
        (None, None) => return false,
        (Some(i), None) => Some(u.infos.remove(i).1),
        (Some(i), Some(routes)) => Some(std::mem::replace(&mut u.infos[i].1, routes.clone())),
        (None, Some(routes)) => {
            u.infos.push((unique_id.into(), routes.clone()));
            None
        }
    };
    let new = new.unwrap_or_default();
    let mut changed = false;
    for route in &new {
        if !route.is_valid() {
            eprintln!(
                "app host: media_router: onProviderStateChangedOnHandler: Ignoring invalid route : {}",
                route.unique_id()
            );
            continue;
        }
        let id = route.unique_id();
        match u.routes.iter_mut().find(|(k, _)| *k == id) {
            Some(entry) if entry.1 == *route => {}
            Some(entry) => {
                entry.1 = route.clone();
                changed = true;
            }
            None => {
                u.routes.push((id, route.clone()));
                changed = true;
            }
        }
    }
    for route in old.unwrap_or_default() {
        let id = route.unique_id();
        if !new.iter().any(|r| r.unique_id() == id) {
            u.routes.retain(|(k, _)| *k != id);
            changed = true;
        }
    }
    changed
}

/// `ServiceCallbackStub` of a proxy's connection.
struct ProviderCallback {
    providers: Weak<Providers>,
    proxy: Weak<Proxy>,
    connection: u64,
}

impl ProviderCallback {
    /// `notifyProviderUpdated`: the routes checked, kept past the call (its
    /// binders held, its files taken) and posted.
    fn provider_updated(
        &self,
        providers: &Providers,
        info: Option<ProviderInfo>,
    ) -> Result<(), Exception> {
        let ProviderInfo(mut routes) =
            info.ok_or_else(|| Exception::new(EX_NULL_POINTER, "providerInfo must not be null"))?;
        for route in &routes {
            route.check_published()?;
        }
        let process = &providers.host.process;
        for route in &mut routes {
            route
                .raw
                .keep_files(&|fd| process.file(fd))
                .map_err(|_| Exception::illegal_argument("a route's file descriptor"))?;
        }
        let held: Vec<Strong> = routes
            .iter()
            .flat_map(|r| r.raw.handles().collect::<Vec<_>>())
            .map(|h| process.strong(h))
            .collect();
        let (proxy, id) = (self.proxy.clone(), self.connection);
        providers.post_with(move |p| {
            if let Some(proxy) = proxy.upgrade() {
                p.provider_updated(&proxy, id, routes, held);
            }
        });
        Ok(())
    }

    /// The proxy, while this is its active connection's callback (the
    /// stub not disposed).
    fn active(&self) -> Option<Arc<Proxy>> {
        self.proxy.upgrade().filter(|p| {
            p.state
                .lock()
                .unwrap()
                .active
                .as_ref()
                .is_some_and(|c| c.id == self.connection)
        })
    }
}

/// A typed list, its elements unread: whether one is not null.
fn any_typed(r: &mut Reader<'_>) -> ParcelResult<bool> {
    for _ in 0..r.read_i32()?.max(0) {
        if r.read_i32()? != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Service for ProviderCallback {
    fn descriptor(&self) -> &str {
        provider_callback::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(providers) = self.providers.upgrade() else {
            return reply(Ok(Parcel::new()));
        };
        let host = &providers.host;
        let result: Result<(), Exception> = match call.code {
            provider_callback::NOTIFY_PROVIDER_UPDATED => {
                let a =
                    provider_callback::NotifyProviderUpdated::<ProviderInfo>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                self.provider_updated(&providers, a.provider_info)
            }
            // `onSessionCreated`: a null session is ignored; one of its own
            // is not kept (#1663).
            provider_callback::NOTIFY_SESSION_CREATED => {
                call.data
                    .enforce_interface(provider_callback::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                call.data.read_i64().map_err(|_| BAD_VALUE)?;
                let session = call.data.read_i32().map_err(|_| BAD_VALUE)? != 0;
                match self.active() {
                    Some(_) if session => Err(host
                        .unimplemented_named("media_router", "a route provider's session (#1663)")),
                    Some(proxy) => {
                        eprintln!(
                            "app host: media_router: onSessionCreated: Ignoring null session sent from {}",
                            proxy.unique_id
                        );
                        Ok(())
                    }
                    None => Ok(()),
                }
            }
            // `onSessionsUpdated`: with no session kept, a list of none
            // changes nothing.
            provider_callback::NOTIFY_SESSIONS_UPDATED => {
                call.data
                    .enforce_interface(provider_callback::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let sessions = any_typed(&mut call.data).map_err(|_| BAD_VALUE)?;
                if sessions && self.active().is_some() {
                    Err(host
                        .unimplemented_named("media_router", "a route provider's sessions (#1663)"))
                } else {
                    Ok(())
                }
            }
            // `onSessionReleased`: no session was kept, so none matches.
            provider_callback::NOTIFY_SESSION_RELEASED => {
                call.data
                    .enforce_interface(provider_callback::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let session = call.data.read_i32().map_err(|_| BAD_VALUE)? != 0;
                match self.active() {
                    Some(_) if session => eprintln!(
                        "app host: media_router: onSessionReleased: Matching session info not found"
                    ),
                    Some(proxy) => eprintln!(
                        "app host: media_router: onSessionReleased: Ignoring null session sent from {}",
                        proxy.unique_id
                    ),
                    None => {}
                }
                Ok(())
            }
            // `onRequestFailed`, then `onRequestFailedOnHandler`: no
            // session creation request is pending and no manager is
            // registered, so none is told.
            provider_callback::NOTIFY_REQUEST_FAILED => {
                let a = provider_callback::NotifyRequestFailed::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match self.active() {
                    Some(_) if a.request_id == REQUEST_ID_NONE => {
                        eprintln!(
                            "app host: media_router: onRequestFailed: Ignoring requestId REQUEST_ID_NONE"
                        )
                    }
                    Some(proxy) => eprintln!(
                        "app host: media_router: handleSessionCreationRequestFailed | No matching request found for provider: {}, uniqueRequestId: {}, reason: {}",
                        proxy.unique_id, a.request_id, a.reason
                    ),
                    None => {}
                }
                Ok(())
            }
            code => Err(host
                .unimplemented_named("media_router", &format!("route provider callback #{code}"))),
        };
        reply(result.map(|()| Parcel::new()))
    }
}

#[cfg(test)]
mod tests {
    /// MediaRoute2ProviderWatcher scans every PACKAGE_CHANGED on its handler.
    #[test]
    fn route_providers_track_session_enabled_changes() {
        crate::system::with_discovery_session(|host, publish, enabled| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    let mut service = crate::package::pkg::Service::default();
                    service.main.enabled = true;
                    service.main.exported = true;
                    service.main.component.name = "p.Routes".into();
                    service.main.component.package_name = "p".into();
                    service.main.component.intents =
                        vec![crate::package::intent_filter::ParsedIntentInfo {
                            filter: crate::package::intent_filter::IntentFilter {
                                actions: vec![SERVICE_INTERFACE.into()],
                                ..Default::default()
                            },
                            ..Default::default()
                        }];
                    pkg.services = vec![service];
                }
            });
            let providers = Providers::new(host.clone(), Arc::new(Mutex::new(HashMap::new())));
            let flush = || {
                let (send, receive) = mpsc::channel();
                providers.post_with(move |_| send.send(()).unwrap());
                receive
                    .recv_timeout(std::time::Duration::from_secs(3))
                    .unwrap();
            };
            providers.start(0);
            flush();
            assert_eq!(providers.users.lock().unwrap()[&0].proxies.len(), 1);
            for component in [Some("p.Routes"), None] {
                let old = providers.users.lock().unwrap()[&0].proxies[0].clone();
                enabled(component, 2);
                flush();
                assert!(providers.users.lock().unwrap()[&0].proxies.is_empty());
                assert!(!old.state.lock().unwrap().running);
                enabled(component, 1);
                flush();
                assert_eq!(providers.users.lock().unwrap()[&0].proxies.len(), 1);
                assert!(
                    providers.users.lock().unwrap()[&0].proxies[0]
                        .state
                        .lock()
                        .unwrap()
                        .running
                );
            }
        });
    }

    #[test]
    fn watcher_removes_retired_proxies_and_their_routes() {
        crate::system::with_discovery_session(|host, publish, _| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    let mut service = crate::package::pkg::Service::default();
                    service.main.enabled = true;
                    service.main.exported = true;
                    service.main.component.name = "p.Routes".into();
                    service.main.component.package_name = "p".into();
                    service.main.component.intents =
                        vec![crate::package::intent_filter::ParsedIntentInfo {
                            filter: crate::package::intent_filter::IntentFilter {
                                actions: vec![SERVICE_INTERFACE.into()],
                                ..Default::default()
                            },
                            ..Default::default()
                        }];
                    pkg.services = vec![service];
                }
            });
            let providers = Providers::new(host.clone(), Arc::new(Mutex::new(HashMap::new())));
            providers.scan(0).unwrap();
            let proxy = providers.users.lock().unwrap()[&0].proxies[0].clone();
            assert!(proxy.state.lock().unwrap().running);
            publish(&mut |pkg| pkg.services.clear());
            providers.scan(0).unwrap();
            assert!(providers.users.lock().unwrap()[&0].proxies.is_empty());
            assert!(!proxy.state.lock().unwrap().running);
            assert!(!proxy.state.lock().unwrap().bound);
        });
    }
    use super::*;
    use aim_binder_host::parcel::EX_SECURITY;

    /// A route as `MediaRoute2Info.writeToParcel` writes it.
    fn route(id: &str, kind: i32, provider: Option<(&str, &str)>, restricted: bool) -> Parcel {
        let mut p = Parcel::new();
        p.write_string16(Some(id));
        clip::write_char_sequence(&mut p, Some("Living room"));
        p.write_i32(1);
        p.write_string16(Some("android.media.route.feature.REMOTE_PLAYBACK"));
        p.write_i32(kind);
        p.write_i32(1 << 2);
        p.write_bool(false);
        p.write_string16(None); // icon
        clip::write_char_sequence(&mut p, None);
        p.write_i32(0);
        p.write_string16(None);
        p.write_string16(provider.map(|(package, _)| package));
        p.write_i32(1);
        p.write_i32(20);
        p.write_i32(5);
        p.write_string16(None);
        p.write_i32(1);
        p.write_string16(Some("dedup"));
        p.write_i32(-1); // extras
        p.write_string16(provider.map(|(_, id)| id));
        p.write_bool(restricted);
        p.write_i32(1);
        p.write_string8(Some("com.example.allowed"));
        p.write_i32(0);
        p.write_i32(0);
        p
    }

    fn read(p: &Parcel) -> ProviderRoute {
        let mut r = p.reader();
        let route = ProviderRoute::read_from(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        route
    }

    const TYPE_REMOTE_TV: i32 = 1001;
    const GMS: (&str, &str) = ("com.google.android.gms", "com.google.android.gms/.Provider");

    /// A route is read to its end, and given its provider as
    /// `Builder.setProviderPackageName` and `setProviderId` write it.
    #[test]
    fn a_route_is_given_its_provider_as_the_builder_gives_it() {
        let read = read(&route("tv", TYPE_REMOTE_TV, None, false));
        assert!(read.check_published().is_ok());
        assert!(!read.is_valid());
        let given = read.with_provider(GMS.0, GMS.1);
        let mut p = Parcel::new();
        given.write_to(&mut p);
        assert_eq!(
            p.data(),
            route("tv", TYPE_REMOTE_TV, Some(GMS), false).data()
        );
        assert!(given.is_valid());
        assert_eq!(given.unique_id(), "com.google.android.gms/.Provider:tv");
        let again = given.with_provider("p", "p/.Q");
        let mut q = Parcel::new();
        again.write_to(&mut q);
        assert_eq!(
            q.data(),
            route("tv", TYPE_REMOTE_TV, Some(("p", "p/.Q")), false).data()
        );
    }

    /// `ServiceCallbackStub`'s checks: a system route type is refused.
    #[test]
    fn a_system_route_type_is_refused() {
        let speaker = read(&route("speaker", 2, None, false));
        assert_eq!(speaker.check_published().unwrap_err().code, EX_SECURITY);
    }

    /// `isVisibleTo`: a restricted route to its provider's package and the
    /// allowed ones.
    #[test]
    fn a_restricted_route_is_visible_to_the_allowed_packages() {
        let restricted = read(&route("tv", TYPE_REMOTE_TV, Some(GMS), true));
        assert!(restricted.is_visible_to("com.example.allowed"));
        assert!(restricted.is_visible_to(GMS.0));
        assert!(!restricted.is_visible_to("com.example.other"));
    }

    /// A provider's routes are told when added, changed or removed (an
    /// invalid one, never added, still removed), and not when the same.
    #[test]
    fn route_changes_are_told_once() {
        let mut u = User::default();
        let tv = read(&route("tv", TYPE_REMOTE_TV, Some(GMS), false));
        let unnamed = read(&route("x", TYPE_REMOTE_TV, None, false));
        assert!(update_routes(
            &mut u,
            GMS.1,
            Some(vec![tv.clone(), unnamed.clone()])
        ));
        assert_eq!(u.routes.len(), 1);
        assert!(!update_routes(
            &mut u,
            GMS.1,
            Some(vec![tv.clone(), unnamed])
        ));
        assert!(update_routes(&mut u, GMS.1, Some(vec![tv.clone()])));
        assert!(!update_routes(&mut u, GMS.1, Some(vec![tv])));
        assert!(update_routes(&mut u, GMS.1, None));
        assert!(u.routes.is_empty() && u.infos.is_empty());
        assert!(!update_routes(&mut u, GMS.1, None));
    }

    /// The composite preference: the features of every router (a
    /// `HashSet`'s order), active from a router scanning actively with
    /// features, written as `RouteDiscoveryPreference.writeToParcel` does.
    #[test]
    fn the_composite_preference_is_built_as_the_service_builds_it() {
        let pref = |features: &[&str], active_scan| Preference {
            features: features.iter().map(|f| f.to_string()).collect(),
            active_scan,
        };
        let (c, actively) = composite(&[
            ("a".into(), pref(&["x", "y", ""], false)),
            ("b".into(), pref(&["y", "z"], true)),
            ("c".into(), pref(&[], true)),
        ]);
        assert!(c.active_scan);
        assert_eq!(actively, ["b"]);
        assert_eq!(c.features, ["x", "y", "z"]);
        assert_eq!(composite(&[]).0, Composite::default());
        let mut p = Parcel::new();
        c.write_to(&mut p);
        let read = <Preference as ReadParcelable>::read_from(&mut p.reader()).unwrap();
        assert_eq!(
            read,
            Preference {
                features: c.features.clone(),
                active_scan: true
            }
        );
    }

    #[test]
    fn short_names_drop_the_package_of_the_class() {
        assert_eq!(short_name("p", "p.Q"), "p/.Q");
        assert_eq!(short_name("p", "o.Q"), "p/o.Q");
    }
}
