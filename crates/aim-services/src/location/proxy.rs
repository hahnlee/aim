//! The providers bound from apps, as the original binds them: location
//! providers (`ProxyLocationProvider`: `network` and `fused`, Google Play
//! services' on this image), the geocoder (`ProxyGeocodeProvider`) and
//! the population density provider (`ProxyPopulationDensityProvider`,
//! with `LocationFudgerCache`).
//!
//! system_server's bridge binds their services with the original's
//! `ServiceWatcher` (a service is bound by a process ActivityManager
//! knows) and hands the service host each binder
//! (`ILocationHost.onProviderBound`); the service host then speaks each
//! provider's AIDL with it directly.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{
    Binder, Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::{
    ReadParcelable, android_location_provider_ilocationprovider as provider,
    android_location_provider_ilocationprovidermanager as manager,
    android_location_provider_ipopulationdensityprovider as density,
    android_location_provider_is2cellidscallback as cells_callback,
    android_location_provider_is2levelcallback as level_callback,
};

use super::LocationManagerService;
use super::env::Identity;
use super::parcels::{Location, ProviderProperties, ProviderRequest};
use super::provider::{ProviderState, RealProvider};

/// How long a provider keeps its state after its service unbound, in
/// case it binds again (`RESET_DELAY_MS`).
pub const RESET_DELAY_MS: i64 = 10_000;

type Done = Box<dyn FnOnce() + Send>;

struct Bound {
    provider: Arc<Strong>,
    package: String,
    extra_tags: Vec<String>,
}

#[derive(Default)]
struct ProxyState {
    bound: Option<Bound>,
    /// Counts binds: a manager node of an older bind is ignored.
    generation: u64,
    state: ProviderState,
    request: Option<ProviderRequest>,
    flushes: VecDeque<Done>,
    /// The generation a pending reset is for.
    reset: Option<u64>,
}

pub struct ProxyProvider {
    pub name: &'static str,
    process: Arc<LocalProcess>,
    service: Weak<LocationManagerService>,
    state: Mutex<ProxyState>,
    this: Weak<ProxyProvider>,
}

impl ProxyProvider {
    pub fn new(
        name: &'static str,
        process: Arc<LocalProcess>,
        service: Weak<LocationManagerService>,
    ) -> Arc<ProxyProvider> {
        Arc::new_cyclic(|this| ProxyProvider {
            name,
            process,
            service,
            state: Mutex::new(ProxyState::default()),
            this: this.clone(),
        })
    }

    /// `onBind`: the service bound; it gets this provider's manager and
    /// the current request.
    pub fn bind(&self, strong: Arc<Strong>, package: String, extra_tags: Option<String>) {
        let mut s = self.state.lock().unwrap();
        s.generation += 1;
        let node = self.process.add_service(Arc::new(Manager {
            proxy: self.this.clone(),
            generation: s.generation,
        }));
        let extra_tags = extra_tags
            .filter(|t| !t.is_empty())
            .map(|t| t.split(';').map(String::from).collect())
            .unwrap_or_default();
        let mut data = Parcel::new();
        provider::SetLocationProviderManager {
            manager: Some(node),
        }
        .write(&mut data);
        let _ = strong.transact(provider::SET_LOCATION_PROVIDER_MANAGER, &data, true);
        if let Some(request) = s.request.clone().filter(|r| r != &ProviderRequest::empty()) {
            send_request(&strong, &request);
        }
        s.bound = Some(Bound {
            provider: strong,
            package,
            extra_tags,
        });
    }

    /// `onUnbind`: pending flushes complete, and the state is dropped
    /// unless the service binds again within [`RESET_DELAY_MS`]; returns
    /// the generation the reset is for.
    pub fn unbind(&self) -> Option<u64> {
        let (flushes, reset) = {
            let mut s = self.state.lock().unwrap();
            s.bound = None;
            if s.reset.is_none() {
                s.reset = Some(s.generation);
            }
            (std::mem::take(&mut s.flushes), s.reset)
        };
        for done in flushes {
            done();
        }
        reset
    }

    /// The reset of [`ProxyProvider::unbind`] is due: whether the state
    /// was dropped.
    pub fn reset_due(&self, generation: u64) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.reset != Some(generation) {
            return false;
        }
        s.reset = None;
        s.state = ProviderState::default();
        true
    }

    /// Runs `f` on the state if `generation` is the current bind's, and
    /// tells the service its provider changed.
    fn update(&self, generation: u64, f: impl FnOnce(&mut ProxyState)) {
        {
            let mut s = self.state.lock().unwrap();
            if s.generation != generation || s.bound.is_none() {
                return;
            }
            f(&mut s);
        }
        if let Some(service) = self.service.upgrade() {
            service.provider_state_changed(self.name);
        }
    }

    fn current(&self, generation: u64) -> bool {
        let s = self.state.lock().unwrap();
        s.generation == generation && s.bound.is_some()
    }
}

fn send_request(provider: &Strong, request: &ProviderRequest) {
    let mut data = Parcel::new();
    provider::SetRequest {
        request: Some(request.clone()),
    }
    .write(&mut data);
    let _ = provider.transact(provider::SET_REQUEST, &data, true);
}

impl RealProvider for ProxyProvider {
    fn state(&self) -> ProviderState {
        self.state.lock().unwrap().state.clone()
    }

    fn set_request(&self, request: &ProviderRequest) {
        let mut s = self.state.lock().unwrap();
        s.request = Some(request.clone());
        if let Some(b) = &s.bound {
            send_request(&b.provider, request);
        }
    }

    fn defers_flush(&self) -> bool {
        true
    }

    fn set_started(&self, started: bool) {
        if let Some(service) = self.service.upgrade() {
            service.set_provider_started(self.name, started);
        }
    }

    /// `onFlush`: completes when the service says so, or at once without
    /// a bound service.
    fn flush(&self, done: Done) {
        let mut s = self.state.lock().unwrap();
        let Some(b) = &s.bound else {
            drop(s);
            return done();
        };
        let mut data = Parcel::new();
        provider::Flush {}.write(&mut data);
        if b.provider.transact(provider::FLUSH, &data, true).is_ok() {
            s.flushes.push_back(done);
        } else {
            drop(s);
            done();
        }
    }
}

/// The `ILocationProviderManager` a bound provider reports to.
struct Manager {
    proxy: Weak<ProxyProvider>,
    generation: u64,
}

impl Service for Manager {
    fn descriptor(&self) -> &str {
        manager::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(proxy) = self.proxy.upgrade() else {
            return Ok(Parcel::new());
        };
        let generation = self.generation;
        let r = &mut call.data;
        let mut reply = Parcel::new();
        match call.code {
            manager::ON_INITIALIZE => {
                let a = manager::OnInitialize::<ProviderProperties>::read(r)?;
                let (uid, pid) = (call.sender_euid as i32, call.sender_pid);
                proxy.update(generation, |s| {
                    let b = s.bound.as_ref().unwrap();
                    s.reset = None;
                    s.state = ProviderState {
                        allowed: a.allowed,
                        properties: a.properties,
                        // `CallerIdentity.fromBinderUnsafe(package, tag)`.
                        identity: Some(Identity {
                            uid,
                            pid,
                            package: b.package.clone(),
                            attribution_tag: a.attribution_tag,
                            listener_id: None,
                        }),
                        extra_attribution_tags: b.extra_tags.clone(),
                    };
                });
                manager::write_on_initialize_reply(&mut reply);
            }
            manager::ON_SET_ALLOWED => {
                let a = manager::OnSetAllowed::read(r)?;
                proxy.update(generation, |s| s.state.allowed = a.allowed);
                manager::write_on_set_allowed_reply(&mut reply);
            }
            manager::ON_SET_PROPERTIES => {
                let a = manager::OnSetProperties::<ProviderProperties>::read(r)?;
                proxy.update(generation, |s| s.state.properties = a.properties);
                manager::write_on_set_properties_reply(&mut reply);
            }
            manager::ON_REPORT_LOCATION | manager::ON_REPORT_LOCATIONS => {
                let locations = if call.code == manager::ON_REPORT_LOCATION {
                    let a = manager::OnReportLocation::<Location>::read(r)?;
                    a.location.into_iter().collect()
                } else {
                    r.enforce_interface(manager::DESCRIPTOR)?;
                    read_locations(r)?
                };
                if proxy.current(generation)
                    && !locations.is_empty()
                    && let Some(service) = proxy.service.upgrade()
                {
                    service.report(proxy.name, locations);
                }
                reply.write_no_exception();
            }
            manager::ON_FLUSH_COMPLETE => {
                manager::OnFlushComplete::read(r)?;
                let done = {
                    let mut s = proxy.state.lock().unwrap();
                    if s.generation == generation && s.bound.is_some() {
                        s.flushes.pop_front()
                    } else {
                        None
                    }
                };
                if let Some(done) = done {
                    done();
                }
                manager::write_on_flush_complete_reply(&mut reply);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}

/// `createTypedArrayList(Location.CREATOR)`.
fn read_locations(r: &mut Reader<'_>) -> ParcelResult<Vec<Location>> {
    let n = r.read_i32()?;
    let mut locations = Vec::new();
    for _ in 0..n.max(0) {
        if r.read_i32()? != 0 {
            locations.push(Location::read_from(r)?);
        }
    }
    Ok(locations)
}

/// `LocationFudgerCache`: the population density provider's coarsening
/// levels, the S2 cells it last returned, and its default level.
pub struct Density {
    provider: Mutex<Option<Arc<Strong>>>,
    cache: Mutex<DensityCache>,
    level_callback: Binder,
    cells_callback: Binder,
}

/// `MAX_CACHE_SIZE`.
const MAX_CACHE_SIZE: usize = 20;

#[derive(Default)]
struct DensityCache {
    cells: VecDeque<i64>,
    default_level: Option<i32>,
}

impl Density {
    pub fn new(process: &Arc<LocalProcess>) -> Arc<Density> {
        Arc::new_cyclic(|this: &Weak<Density>| Density {
            provider: Mutex::new(None),
            cache: Mutex::new(DensityCache::default()),
            level_callback: process.add_service(Arc::new(DensityCallback {
                density: this.clone(),
                cells: false,
            })),
            cells_callback: process.add_service(Arc::new(DensityCallback {
                density: this.clone(),
                cells: true,
            })),
        })
    }

    pub fn bind(&self, provider: Option<Arc<Strong>>) {
        *self.provider.lock().unwrap() = provider;
    }

    /// `hasDefaultValue`.
    pub fn has_default(&self) -> bool {
        self.cache.lock().unwrap().default_level.is_some()
    }

    /// `onDefaultCoarseningLevelNotSet`.
    pub fn default_not_set(&self) {
        if !self.has_default() {
            self.fetch_default();
        }
    }

    /// `getCoarseningLevel(lat, lng)`: a cached cell's level, else the
    /// default while the cells around are asked for.
    pub fn coarsening_level(&self, lat: f64, lng: f64) -> i32 {
        if !self.has_default() {
            self.fetch_default();
        }
        let (cell, default) = {
            let c = self.cache.lock().unwrap();
            let cell = c
                .cells
                .iter()
                .copied()
                .find(|&cell| super::s2::contains(cell, lat, lng));
            (cell, c.default_level.unwrap_or(0))
        };
        match cell {
            Some(cell) => super::s2::level(cell),
            None => {
                self.call(
                    |p, cb| {
                        let mut data = Parcel::new();
                        density::GetCoarsenedS2Cells {
                            latitude_degrees: lat,
                            longitude_degrees: lng,
                            num_additional_cells: (MAX_CACHE_SIZE - 1) as i32,
                            callback: Some(cb),
                        }
                        .write(&mut data);
                        let _ = p.transact(density::GET_COARSENED_S2CELLS, &data, true);
                    },
                    self.cells_callback,
                );
                default
            }
        }
    }

    fn fetch_default(&self) {
        self.call(
            |p, cb| {
                let mut data = Parcel::new();
                density::GetDefaultCoarseningLevel { callback: Some(cb) }.write(&mut data);
                let _ = p.transact(density::GET_DEFAULT_COARSENING_LEVEL, &data, true);
            },
            self.level_callback,
        );
    }

    /// Calls the provider if bound (`runOnBinder`; without it the query
    /// fails, as `onError` logs it).
    fn call(&self, f: impl FnOnce(&Strong, Binder), callback: Binder) {
        match self.provider.lock().unwrap().as_deref() {
            Some(p) => f(p, callback),
            None => eprintln!("location: could not get population density"),
        }
    }

    /// `addToCache(cells)`: newest first, at most [`MAX_CACHE_SIZE`].
    fn add(&self, cells: &[i64]) {
        let mut c = self.cache.lock().unwrap();
        for &cell in cells.iter().take(MAX_CACHE_SIZE).rev() {
            if c.cells.len() == MAX_CACHE_SIZE {
                c.cells.pop_front();
            }
            c.cells.push_back(cell);
        }
    }
}

/// `IS2LevelCallback` or `IS2CellIdsCallback`.
struct DensityCallback {
    density: Weak<Density>,
    cells: bool,
}

impl Service for DensityCallback {
    fn descriptor(&self) -> &str {
        if self.cells {
            cells_callback::DESCRIPTOR
        } else {
            level_callback::DESCRIPTOR
        }
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(density) = self.density.upgrade() else {
            return Ok(Parcel::new());
        };
        let r = &mut call.data;
        match (self.cells, call.code) {
            (false, level_callback::ON_RESULT) => {
                let a = level_callback::OnResult::read(r)?;
                density.cache.lock().unwrap().default_level = Some(a.s2level);
            }
            (true, cells_callback::ON_RESULT) => {
                r.enforce_interface(cells_callback::DESCRIPTOR)?;
                // `createLongArray`.
                let n = r.read_i32()?;
                let cells = (0..n.max(0))
                    .map(|_| r.read_i64())
                    .collect::<ParcelResult<Vec<i64>>>()?;
                density.add(&cells);
            }
            (false, level_callback::ON_ERROR) | (true, cells_callback::ON_ERROR) => {
                eprintln!("location: could not get population density")
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(Parcel::new())
    }
}
