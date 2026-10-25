//! `country_detector`: CountryDetectorService for the app host
//! (`android-16.0.0_r1`) with its `ComprehensiveCountryDetector`: the
//! country of the network, else the one the location gave, else the
//! SIM's, else the device locale's; the device has no telephony, so there
//! is no network or SIM country. While a listener is registered, and the
//! country is not the location's, the location is asked
//! (`LocationBasedCountryDetector`) if a geocoder is there and the device
//! is not in airplane mode (or has Wi-Fi on): the last known location of
//! the providers, or the first fix of the passive provider within five
//! minutes, reverse geocoded; the location is asked again a day later.
//! The service is ready from the start (`systemRunning`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_location_icountrydetector as cd;
use aim_service_aidl::android_location_icountrylistener as cl;
use aim_service_aidl::android_location_ilocationlistener as ll;
use aim_service_aidl::android_location_ilocationmanager as lm;
use aim_service_aidl::android_location_provider_igeocodecallback as gc;
use aim_service_aidl::android_os_iremotecallback as remote_callback;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::kept::{By, RawBundle};
use super::settings_provider::{Caller, SettingsProvider};
use super::text_services::Locale;
use super::{AppHost, reply};
use crate::bundle::Value;
use crate::location::parcels::{Location, write_f64};

/// `Country.COUNTRY_SOURCE_*`.
const COUNTRY_SOURCE_LOCATION: i32 = 1;
const COUNTRY_SOURCE_LOCALE: i32 = 3;
/// `ComprehensiveCountryDetector.LOCATION_REFRESH_INTERVAL`.
const LOCATION_REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// `LocationBasedCountryDetector.QUERY_LOCATION_TIMEOUT`.
const QUERY_LOCATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// `Geocoder.TIMEOUT_MS`.
const GEOCODE_TIMEOUT: Duration = Duration::from_secs(15);
/// `LocationManager.PASSIVE_PROVIDER`.
const PASSIVE_PROVIDER: &str = "passive";
/// `PackageManager.FEATURE_TELEPHONY`.
const FEATURE_TELEPHONY: &str = "android.hardware.telephony";
/// `CountryDetectorBase.ATTRIBUTION_TAG`; the context is system_server's.
const ATTRIBUTION_TAG: &str = "CountryDetector";
const PACKAGE: &str = "android";
const SYSTEM_UID: i32 = 1000;
/// `LocationRequest.POWER_NONE`.
const POWER_NONE: i32 = 200;

/// `android.location.Country`: equal by its code and source.
#[derive(Clone, Debug)]
struct Country {
    iso: String,
    source: i32,
    timestamp: i64,
}

impl Country {
    /// `new Country(countryIso, source)`: the code in upper case, now.
    fn new(iso: &str, source: i32) -> Country {
        Country {
            iso: iso.to_uppercase(),
            source,
            timestamp: super::clock_ms(libc::CLOCK_MONOTONIC_RAW),
        }
    }
}

impl PartialEq for Country {
    fn eq(&self, other: &Country) -> bool {
        self.iso == other.iso && self.source == other.source
    }
}

impl WriteParcelable for Country {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.iso));
        p.write_i32(self.source);
        p.write_i64(self.timestamp);
    }
}

/// A listener of `addCountryListener` and its death notification.
struct Receiver {
    strong: Arc<Strong>,
    death: u64,
}

/// `LocationBasedCountryDetector`.
struct LocationDetector {
    /// `mEnabledProviders`, asked once.
    enabled_providers: Option<Vec<String>>,
    /// `mLocationListeners`.
    listeners: Option<Vec<Arc<Transport>>>,
    /// `mTimer`, by its id.
    timer: Option<u64>,
    /// `mQueryThread != null`.
    querying: bool,
}

/// `ComprehensiveCountryDetector`'s state.
#[derive(Default)]
struct State {
    /// `mReceivers`, by binder handle.
    receivers: HashMap<u32, Receiver>,
    /// `mCountry`.
    country: Option<Country>,
    /// `mCountryFromLocation`.
    from_location: Option<Country>,
    /// `mListener != null`: the service listens while it has receivers.
    listening: bool,
    /// `mLocationRefreshTimer`, by its id.
    refresh: Option<u64>,
    /// `mLocationBasedCountryDetector`.
    location: Option<LocationDetector>,
    next_timer: u64,
}

type Job = Box<dyn FnOnce() + Send>;

/// `android.permission.DUMP`.
const DUMP: &str = "android.permission.DUMP";

pub struct CountryDetector {
    host: Arc<AppHost>,
    settings: Arc<SettingsProvider>,
    state: Mutex<State>,
    /// `BackgroundThread`'s handler, which the detectors share.
    handler: Mutex<mpsc::Sender<Job>>,
    me: Weak<CountryDetector>,
}

impl CountryDetector {
    pub fn new(host: Arc<AppHost>, settings: Arc<SettingsProvider>) -> Arc<CountryDetector> {
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("CountryDetector".into())
            .spawn(move || jobs.into_iter().for_each(|job| job()))
            .expect("the country detector's thread");
        let custom = host
            .framework()
            .ok()
            .and_then(|i| i.framework_string("config_customCountryDetector"))
            .filter(|c| !c.is_empty());
        if let Some(class) = custom {
            // `loadCustomCountryDetectorIfAvailable` loads a class of
            // system_server's.
            eprintln!(
                "app host: country_detector: Could not instantiate the custom country detector class {class}"
            );
        }
        Arc::new_cyclic(|me| CountryDetector {
            host,
            settings,
            state: Mutex::default(),
            handler: Mutex::new(sender),
            me: me.clone(),
        })
    }

    fn post(&self, job: impl FnOnce() + Send + 'static) {
        if self.handler.lock().unwrap().send(Box::new(job)).is_err() {
            eprintln!("app host: country_detector: the handler stopped");
        }
    }

    /// A job for the handler that runs while the service does.
    fn posted(&self, job: impl FnOnce(&CountryDetector) + Send + 'static) {
        let me = self.me.clone();
        self.post(move || {
            if let Some(me) = me.upgrade() {
                job(&me);
            }
        });
    }

    /// `job` once `delay` has passed, unless the timer `id` of `slot` was
    /// cancelled or replaced by then (`java.util.Timer`).
    fn schedule(
        &self,
        id: u64,
        delay: Duration,
        slot: fn(&mut State) -> Option<&mut Option<u64>>,
        job: impl FnOnce(&CountryDetector) + Send + 'static,
    ) {
        let me = self.me.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let Some(me) = me.upgrade() else { return };
            {
                let mut state = me.state.lock().unwrap();
                match slot(&mut state) {
                    Some(timer) if *timer == Some(id) => *timer = None,
                    _ => return,
                }
            }
            job(&me);
        });
    }

    fn global_int(&self, name: &str, default: i32) -> i32 {
        let caller = Caller {
            uid: SYSTEM_UID,
            package: Some(PACKAGE.into()),
        };
        let extras = HashMap::from([("_user".to_string(), Value::Int(0))]);
        match self
            .settings
            .call(&caller, "GET_global", Some(name), &extras, None)
        {
            Ok(Some(entries)) => match entries.first() {
                Some((_, Value::String(Some(v)))) => v.parse().unwrap_or(default),
                _ => default,
            },
            Ok(None) => default,
            Err(e) => {
                eprintln!("app host: country_detector: {name}: {}", e.message);
                default
            }
        }
    }

    // The countries of each source (`getCountry`).

    /// `getNetworkBasedCountry` and `getSimBasedCountry`: TelephonyManager
    /// has no phone (`PHONE_TYPE_NONE`) and no SIM without telephony.
    fn telephony_country(&self) -> Option<Country> {
        let declared = self.host.packages().ok().is_some_and(|(state, _)| {
            state
                .system
                .features
                .iter()
                .any(|(n, _)| n == FEATURE_TELEPHONY)
        });
        if declared {
            // The image declares no telephony; telephony backed by the Mac
            // is #1923.
            self.host.unimplemented_named(
                "country_detector",
                "the network's and SIM's country (#1923)",
            );
        }
        None
    }

    /// `getLocaleCountry`: the country of the device's locale
    /// (`Locale.getDefault()`).
    fn locale_country(&self) -> Option<Country> {
        let locales = self.host.property("persist.sys.locale")?;
        let locale = Locale::of_device(&locales)?;
        Some(Country::new(&locale.country, COUNTRY_SOURCE_LOCALE))
    }

    fn country(&self) -> Option<Country> {
        self.telephony_country()
            .or_else(|| self.state.lock().unwrap().from_location.clone())
            .or_else(|| self.locale_country())
    }

    /// `detectCountry(notifyChange, startLocationBasedDetection)`.
    fn detect(&self, notify: bool, start_location: bool) -> Option<Country> {
        let country = self.country();
        let previous = std::mem::replace(&mut self.state.lock().unwrap().country, country.clone());
        let detected = country.clone();
        self.posted(move |me| me.after_detection(previous, detected, notify, start_location));
        country
    }

    /// `runAfterDetection`, on the handler.
    fn after_detection(
        &self,
        previous: Option<Country>,
        detected: Option<Country>,
        notify: bool,
        start_location: bool,
    ) {
        let listening = self.state.lock().unwrap().listening;
        if notify
            && listening
            && let Some(country) = &detected
            && previous.as_ref() != Some(country)
        {
            self.notify_listener(country.clone());
        }
        let not_located = detected
            .as_ref()
            .is_none_or(|c| c.source > COUNTRY_SOURCE_LOCATION);
        if start_location
            && not_located
            && (self.global_int("airplane_mode_on", 0) == 0 || self.global_int("wifi_on", 0) != 0)
            && self.state.lock().unwrap().listening
            && self.geocoder_present()
        {
            self.start_location_detector();
        }
        if detected
            .as_ref()
            .is_none_or(|c| c.source >= COUNTRY_SOURCE_LOCATION)
        {
            self.schedule_refresh();
        } else {
            self.state.lock().unwrap().refresh = None;
            self.stop_location_detector();
        }
    }

    /// The service's `mLocationBasedDetectorListener`: its receivers told
    /// on the handler.
    fn notify_listener(&self, country: Country) {
        self.posted(move |me| me.notify_receivers(&country));
    }

    /// `notifyReceivers`.
    fn notify_receivers(&self, country: &Country) {
        let mut p = Parcel::new();
        cl::OnCountryDetected {
            country: Some(By(country)),
        }
        .write(&mut p);
        let receivers: Vec<Arc<Strong>> = self
            .state
            .lock()
            .unwrap()
            .receivers
            .values()
            .map(|r| r.strong.clone())
            .collect();
        for r in receivers {
            if let Err(e) = r.transact(cl::ON_COUNTRY_DETECTED, &p, true) {
                eprintln!("app host: country_detector: notifyReceivers failed: {e}");
            }
        }
    }

    /// `scheduleLocationRefresh`.
    fn schedule_refresh(&self) {
        let id = {
            let mut state = self.state.lock().unwrap();
            if state.refresh.is_some() {
                return;
            }
            state.next_timer += 1;
            state.refresh = Some(state.next_timer);
            state.next_timer
        };
        self.schedule(
            id,
            LOCATION_REFRESH_INTERVAL,
            |s| Some(&mut s.refresh),
            |me| {
                me.detect(false, true);
            },
        );
    }

    /// `setCountryListener` of the comprehensive detector, on the handler.
    fn set_listening(&self, listening: bool) {
        let previous = std::mem::replace(&mut self.state.lock().unwrap().listening, listening);
        if !listening {
            self.stop_location_detector();
            self.state.lock().unwrap().refresh = None;
        } else if !previous {
            // `addPhoneStateListener`: without telephony no service state
            // ever has a network country.
            self.detect(false, true);
        }
    }

    // The location-based detector.

    fn location_call<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<aim_service_aidl::Returned<T>>,
    ) -> Result<T, String> {
        let mut data = Parcel::new();
        write(&mut data);
        let reply = super::display::transact_service(&self.host.process, "location", code, &data)?;
        read(&mut reply.reader())
            .map_err(|s| s.to_string())?
            .map_err(|e| format!("{e:?}"))
    }

    /// `Geocoder.isPresent`.
    fn geocoder_present(&self) -> bool {
        self.location_call(
            lm::IS_GEOCODE_AVAILABLE,
            |p| lm::IsGeocodeAvailable {}.write(p),
            lm::read_is_geocode_available_reply,
        )
        .unwrap_or_else(|e| {
            eprintln!("app host: country_detector: isGeocodeAvailable: {e}");
            false
        })
    }

    /// `startLocationBasedDetector`, then its `detectCountry`.
    fn start_location_detector(&self) {
        {
            let mut state = self.state.lock().unwrap();
            if state.location.is_some() {
                return;
            }
            state.location = Some(LocationDetector {
                enabled_providers: None,
                listeners: None,
                timer: None,
                querying: false,
            });
        }
        let providers = match self.location_call(
            lm::GET_PROVIDERS,
            |p| {
                lm::GetProviders::<NoCriteria> {
                    criteria: None,
                    enabled_only: true,
                }
                .write(p)
            },
            lm::read_get_providers_reply,
        ) {
            Ok(list) => list.unwrap_or_default().into_iter().flatten().collect(),
            Err(e) => {
                eprintln!("app host: country_detector: getProviders: {e}");
                Vec::new()
            }
        };
        let mut state = self.state.lock().unwrap();
        state.next_timer += 1;
        let id = state.next_timer;
        let Some(detector) = state.location.as_mut() else {
            return;
        };
        let providers = detector.enabled_providers.get_or_insert(providers).clone();
        if providers.is_empty() {
            drop(state);
            self.query(self.last_known_location());
            return;
        }
        let mut listeners = Vec::new();
        for provider in providers.iter().filter(|p| *p == PASSIVE_PROVIDER) {
            let transport = Arc::new(Transport {
                detector: self.me.clone(),
                unregistered: Arc::new(AtomicBool::new(false)),
                binder: Mutex::new(None),
            });
            listeners.push((provider.clone(), transport));
        }
        detector.listeners = Some(listeners.iter().map(|(_, t)| t.clone()).collect());
        detector.timer = Some(id);
        drop(state);
        for (provider, transport) in &listeners {
            self.register_listener(provider, transport);
        }
        self.schedule(
            id,
            QUERY_LOCATION_TIMEOUT,
            |s| s.location.as_mut().map(|d| &mut d.timer),
            |me| {
                me.stop_location_listeners();
                me.query(me.last_known_location());
            },
        );
    }

    /// `registerListener`: `requestLocationUpdates(provider, 0, 0,
    /// listener)`, as the system.
    fn register_listener(&self, provider: &str, transport: &Arc<Transport>) {
        let binder = self.host.process.add_service(transport.clone());
        *transport.binder.lock().unwrap() = Some(binder);
        let listener_id = format!(
            "com.android.server.location.countrydetector.LocationBasedCountryDetector$1@{:x}",
            Arc::as_ptr(transport) as usize as u32
        );
        if let Err(e) = self.location_call(
            lm::REGISTER_LOCATION_LISTENER,
            |p| {
                lm::RegisterLocationListener {
                    provider: Some(provider.into()),
                    request: Some(DeprecatedRequest(provider.into())),
                    listener: Some(binder),
                    package_name: Some(PACKAGE.into()),
                    attribution_tag: Some(ATTRIBUTION_TAG.into()),
                    listener_id: Some(listener_id),
                }
                .write(p)
            },
            lm::read_register_location_listener_reply,
        ) {
            eprintln!("app host: country_detector: requestLocationUpdates({provider}): {e}");
        }
    }

    /// `LocationBasedCountryDetector.stop`.
    fn stop_location_listeners(&self) {
        let listeners = {
            let mut state = self.state.lock().unwrap();
            let Some(detector) = state.location.as_mut() else {
                return;
            };
            detector.timer = None;
            detector.listeners.take().unwrap_or_default()
        };
        for transport in listeners {
            transport.unregistered.store(true, Ordering::Release);
            let Some(binder) = *transport.binder.lock().unwrap() else {
                continue;
            };
            if let Err(e) = self.location_call(
                lm::UNREGISTER_LOCATION_LISTENER,
                |p| {
                    lm::UnregisterLocationListener {
                        listener: Some(binder),
                    }
                    .write(p)
                },
                lm::read_unregister_location_listener_reply,
            ) {
                eprintln!("app host: country_detector: removeUpdates: {e}");
            }
        }
    }

    /// `stopLocationBasedDetector`.
    fn stop_location_detector(&self) {
        self.stop_location_listeners();
        self.state.lock().unwrap().location = None;
    }

    /// `getLastKnownLocation`: the newest last location of all providers.
    fn last_known_location(&self) -> Option<Location> {
        let providers: Vec<String> = match self.location_call(
            lm::GET_ALL_PROVIDERS,
            |p| lm::GetAllProviders {}.write(p),
            lm::read_get_all_providers_reply,
        ) {
            Ok(list) => list.unwrap_or_default().into_iter().flatten().collect(),
            Err(e) => {
                eprintln!("app host: country_detector: getAllProviders: {e}");
                return None;
            }
        };
        let mut best: Option<Location> = None;
        for provider in providers {
            let last = self.location_call(
                lm::GET_LAST_LOCATION,
                |p| {
                    lm::GetLastLocation {
                        provider: Some(provider.clone()),
                        request: Some(LastLocationRequest),
                        package_name: Some(PACKAGE.into()),
                        attribution_tag: Some(ATTRIBUTION_TAG.into()),
                    }
                    .write(p)
                },
                lm::read_get_last_location_reply::<Location>,
            );
            match last {
                Ok(Some(l))
                    if best
                        .as_ref()
                        .is_none_or(|b| b.elapsed_realtime_ns < l.elapsed_realtime_ns) =>
                {
                    best = Some(l)
                }
                Ok(_) => {}
                Err(e) => eprintln!("app host: country_detector: getLastKnownLocation: {e}"),
            }
        }
        best
    }

    /// `queryCountryCode`: on a thread of its own, once at a time; a
    /// query of no location leaves it taken, as the original's does.
    fn query(&self, location: Option<Location>) {
        {
            let mut state = self.state.lock().unwrap();
            let Some(detector) = state.location.as_mut() else {
                return;
            };
            if detector.querying {
                return;
            }
            detector.querying = true;
        }
        let me = self.me.clone();
        std::thread::spawn(move || {
            let Some(me) = me.upgrade() else { return };
            let Some(location) = location else {
                me.location_country(None);
                return;
            };
            let detected = me
                .country_from_location(&location)
                .map(|iso| Country::new(&iso, COUNTRY_SOURCE_LOCATION));
            me.location_country(detected);
            if let Some(d) = me.state.lock().unwrap().location.as_mut() {
                d.querying = false;
            }
        });
    }

    /// The comprehensive detector's `mLocationBasedCountryDetectionListener`.
    fn location_country(&self, country: Option<Country>) {
        self.state.lock().unwrap().from_location = country;
        self.detect(true, false);
        self.stop_location_detector();
    }

    /// `getCountryFromLocation`: the country code of the location's first
    /// address (`Geocoder.getFromLocation(latitude, longitude, 1)` in the
    /// device's locale, waited for 15 s).
    fn country_from_location(&self, location: &Location) -> Option<String> {
        let (sender, results) = mpsc::channel();
        let callback = self.host.process.add_service(Arc::new(GeocodeCallback {
            results: Mutex::new(Some(sender)),
        }));
        let locale = self
            .host
            .property("persist.sys.locale")
            .and_then(|l| Locale::of_device(&l))
            .unwrap_or_default();
        let request = ReverseGeocodeRequest {
            latitude: location.latitude,
            longitude: location.longitude,
            locale,
        };
        let sent = self.location_call(
            lm::REVERSE_GEOCODE,
            |p| {
                lm::ReverseGeocode {
                    request: Some(request),
                    callback: Some(callback),
                }
                .write(p)
            },
            lm::read_reverse_geocode_reply,
        );
        let got = match sent {
            Ok(()) => results
                .recv_timeout(GEOCODE_TIMEOUT)
                .unwrap_or_else(|_| Err("java.util.concurrent.TimeoutException".into())),
            Err(e) => Err(e),
        };
        match got {
            Ok(code) => code,
            Err(_) => {
                eprintln!(
                    "app host: country_detector: Exception occurs when getting country from location"
                );
                None
            }
        }
    }

    /// A location the passive provider gave a listener of the detector.
    fn location_changed(&self, location: Location) {
        self.stop_location_listeners();
        self.query(Some(location));
    }

    // `ICountryDetector`.

    /// `addListener`.
    fn add_listener(&self, handle: u32) {
        let strong = Arc::new(self.host.process.strong(handle));
        let me = self.me.clone();
        let death = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(me) = me.upgrade() {
                    me.remove_listener(handle);
                }
            }),
        );
        if let Some(country) = self.detect(false, true) {
            let mut p = Parcel::new();
            cl::OnCountryDetected {
                country: Some(By(&country)),
            }
            .write(&mut p);
            if let Err(e) = strong.transact(cl::ON_COUNTRY_DETECTED, &p, true) {
                eprintln!("app host: country_detector: linkToDeath failed: {e}");
                self.host.process.clear_death(&strong, death);
                return;
            }
        }
        let mut state = self.state.lock().unwrap();
        if let Some(old) = state.receivers.insert(handle, Receiver { strong, death }) {
            self.host.process.clear_death(&old.strong, old.death);
        }
        if state.receivers.len() == 1 {
            eprintln!("app host: country_detector: The first listener is added");
            drop(state);
            self.posted(|me| me.set_listening(true));
        }
    }

    /// `removeListener`.
    fn remove_listener(&self, handle: u32) {
        let mut state = self.state.lock().unwrap();
        if let Some(r) = state.receivers.remove(&handle) {
            self.host.process.clear_death(&r.strong, r.death);
        }
        if state.receivers.is_empty() {
            drop(state);
            self.posted(|me| me.set_listening(false));
            eprintln!("app host: country_detector: No listener is left");
        }
    }
}

impl Service for CountryDetector {
    fn descriptor(&self) -> &str {
        cd::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            cd::DETECT_COUNTRY => {
                cd::DetectCountry::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let country = self.detect(false, true);
                cd::write_detect_country_reply(&mut p, country.as_ref());
                Ok(())
            }
            cd::ADD_COUNTRY_LISTENER | cd::REMOVE_COUNTRY_LISTENER => {
                let listener = if call.code == cd::ADD_COUNTRY_LISTENER {
                    cd::AddCountryListener::read(&mut call.data).map(|a| a.listener)
                } else {
                    cd::RemoveCountryListener::read(&mut call.data).map(|a| a.listener)
                }
                .map_err(|_| BAD_VALUE)?;
                match listener {
                    Some(Binder::Handle(handle)) => {
                        if call.code == cd::ADD_COUNTRY_LISTENER {
                            self.add_listener(handle);
                        } else {
                            self.remove_listener(handle);
                        }
                        p.write_no_exception();
                        Ok(())
                    }
                    _ => Err(Exception::new(
                        aim_binder_host::parcel::EX_NULL_POINTER,
                        "Attempt to invoke interface method 'android.os.IBinder android.location.ICountryListener.asBinder()' on a null object reference",
                    )),
                }
            }
            // `CountryDetectorService.dump`: DumpUtils' DUMP check, then
            // nothing (its DEBUG is off).
            aim_binder_host::local::DUMP_TRANSACTION => {
                let out = call
                    .data
                    .read_fd()
                    .ok()
                    .and_then(|fd| self.host.process.file(fd))
                    .and_then(|f| aim_binder_host::server::file_fd(&f));
                let uid = call.sender_euid as i32;
                if let Some(mut out) = out
                    && self
                        .host
                        .check_permission(Some(DUMP), uid)
                        .map_or(true, |r| r != 0)
                {
                    use std::io::Write;
                    let _ = writeln!(
                        out,
                        "Permission Denial: can't dump CountryDetector from from pid={}, uid={uid} \
                         due to missing android.permission.DUMP permission",
                        call.sender_pid
                    );
                }
                p.write_no_exception();
                Ok(())
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

/// `LocationManager`'s `LocationListenerTransport` of a detector's
/// listener: its calls run on the handler while it is registered.
struct Transport {
    detector: Weak<CountryDetector>,
    /// `mListener == null`.
    unregistered: Arc<AtomicBool>,
    binder: Mutex<Option<Binder>>,
}

impl Service for Transport {
    fn descriptor(&self) -> &str {
        ll::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            ll::ON_LOCATION_CHANGED => {
                let a = ll::OnLocationChanged::<Location>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let Some(detector) = self.detector.upgrade() else {
                    return Ok(Parcel::new());
                };
                if self.unregistered.load(Ordering::Acquire) {
                    return Ok(Parcel::new());
                }
                let complete = match a.on_complete_callback {
                    Some(Binder::Handle(h)) => Some(detector.host.process.strong(h)),
                    _ => None,
                };
                let locations: Vec<Location> = a
                    .locations
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect();
                let unregistered = self.unregistered.clone();
                // `executeSafely`: nothing, not even the completion, once
                // the listener is gone when it runs.
                detector.posted(move |d| {
                    if unregistered.load(Ordering::Acquire) {
                        return;
                    }
                    // `LocationListener.onLocationChanged(List)`: each in
                    // turn.
                    for location in locations {
                        d.location_changed(location);
                    }
                    if let Some(complete) = complete {
                        let mut p = Parcel::new();
                        remote_callback::SendResult::<RawBundle> { data: None }.write(&mut p);
                        let _ = complete.transact(remote_callback::SEND_RESULT, &p, true);
                    }
                });
                Ok(Parcel::new())
            }
            // The detector's listener does nothing with these.
            ll::ON_PROVIDER_ENABLED_CHANGED | ll::ON_FLUSH_COMPLETE => Ok(Parcel::new()),
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

/// A geocode's first country code, or its error.
type Geocoded = Result<Option<String>, String>;

/// `Geocoder.GeocodeCallbackImpl` with its `SynchronousGeocoder`: the
/// first address's country code, or the error, once.
struct GeocodeCallback {
    results: Mutex<Option<mpsc::Sender<Geocoded>>>,
}

impl Service for GeocodeCallback {
    fn descriptor(&self) -> &str {
        gc::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let result = match call.code {
            gc::ON_ERROR => {
                let a = gc::OnError::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Err(a.error.unwrap_or_default())
            }
            gc::ON_RESULTS => {
                let a =
                    gc::OnResults::<AddressCountry>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Ok(a.results
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .next()
                    .and_then(|a| a.0))
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if let Some(sender) = self.results.lock().unwrap().take() {
            let _ = sender.send(result);
        }
        Ok(Parcel::new())
    }
}

/// `android.location.Address`, read for its country code.
struct AddressCountry(Option<String>);

impl ReadParcelable for AddressCountry {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // Its locale's language and country.
        r.read_string16()?;
        r.read_string16()?;
        let lines = r.read_i32()?;
        for _ in 0..lines.max(0) {
            r.read_i32()?;
            r.read_string16()?;
        }
        // Feature name, admin area, sub-admin area, locality, sub-locality,
        // thoroughfare, sub-thoroughfare, premises, postal code.
        for _ in 0..9 {
            r.read_string16()?;
        }
        let country_code = r.read_string16()?;
        r.read_string16()?; // country name
        for _ in 0..2 {
            if r.read_i32()? != 0 {
                crate::location::parcels::read_f64(r)?;
            }
        }
        r.read_string16()?; // phone
        r.read_string16()?; // url
        crate::clip::bundle(r)?;
        Ok(AddressCountry(country_code))
    }
}

/// `ILocationManager.getProviders`' criteria: none.
struct NoCriteria;

impl WriteParcelable for NoCriteria {
    fn write_to(&self, _: &mut Parcel) {}
}

/// `new LastLocationRequest.Builder().build()`.
struct LastLocationRequest;

impl WriteParcelable for LastLocationRequest {
    fn write_to(&self, p: &mut Parcel) {
        // Hidden from app ops, ADAS GNSS bypass, location settings ignored.
        p.write_bool(false);
        p.write_bool(false);
        p.write_bool(false);
    }
}

/// `LocationRequest.createFromDeprecatedProvider(provider, 0, 0, false)`.
struct DeprecatedRequest(String);

impl WriteParcelable for DeprecatedRequest {
    fn write_to(&self, p: &mut Parcel) {
        let quality = match self.0.as_str() {
            PASSIVE_PROVIDER => POWER_NONE,
            "gps" => 100,
            _ => 102,
        };
        p.write_string16(Some(&self.0));
        p.write_i64(0); // interval
        p.write_i32(quality);
        p.write_i64(i64::MAX); // expire at
        p.write_i64(i64::MAX); // duration
        p.write_i32(i32::MAX); // max updates
        p.write_i64(0); // min update interval
        p.write_f32(0.0); // min update distance
        p.write_i64(0); // max update delay
        p.write_bool(false); // hidden from app ops
        p.write_bool(false); // ADAS GNSS bypass
        p.write_bool(false); // bypass
        p.write_bool(false); // low power
        // `writeTypedObject(new WorkSource(null), 0)`: no uids, names or
        // chains.
        p.write_i32(1);
        p.write_i32(0);
        p.write_i32(-1);
        p.write_i32(-1);
        p.write_i32(-1);
    }
}

/// `ReverseGeocodeRequest` of a location, in the device's locale, for
/// system_server's context.
struct ReverseGeocodeRequest {
    latitude: f64,
    longitude: f64,
    locale: Locale,
}

impl WriteParcelable for ReverseGeocodeRequest {
    fn write_to(&self, p: &mut Parcel) {
        write_f64(p, self.latitude);
        write_f64(p, self.longitude);
        p.write_i32(1); // max results
        p.write_string8(Some(&self.locale.language));
        p.write_string8(Some(&self.locale.country));
        p.write_string8(Some(&self.locale.variant));
        p.write_i32(SYSTEM_UID);
        p.write_string8(Some(PACKAGE));
        p.write_string8(Some(ATTRIBUTION_TAG));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countries_are_equal_by_code_and_source() {
        let a = Country::new("kr", COUNTRY_SOURCE_LOCALE);
        assert_eq!(a.iso, "KR");
        let mut b = a.clone();
        b.timestamp += 5;
        assert_eq!(a, b);
        assert_ne!(a, Country::new("KR", COUNTRY_SOURCE_LOCATION));
    }

    #[test]
    fn countries_are_written_as_their_parcel_reads_them() {
        let mut p = Parcel::new();
        Country {
            iso: "US".into(),
            source: COUNTRY_SOURCE_LOCATION,
            timestamp: 42,
        }
        .write_to(&mut p);
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("US"));
        assert_eq!(r.read_i32().unwrap(), COUNTRY_SOURCE_LOCATION);
        assert_eq!(r.read_i64().unwrap(), 42);
    }

    /// An address as `Address.writeToParcel` writes it, two in a list.
    #[test]
    fn addresses_are_read_for_their_country_code() {
        let mut p = Parcel::new();
        let address = |p: &mut Parcel, code: &str| {
            p.write_string16(Some("ko"));
            p.write_string16(Some("KR"));
            p.write_i32(1);
            p.write_i32(0);
            p.write_string16(Some("Seoul"));
            for _ in 0..9 {
                p.write_string16(None);
            }
            p.write_string16(Some(code));
            p.write_string16(Some("Korea"));
            p.write_i32(1);
            write_f64(p, 37.5);
            p.write_i32(0);
            p.write_string16(None);
            p.write_string16(None);
            p.write_i32(-1); // no extras
        };
        address(&mut p, "KR");
        address(&mut p, "JP");
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(
            AddressCountry::read_from(&mut r).unwrap().0.as_deref(),
            Some("KR")
        );
        assert_eq!(
            AddressCountry::read_from(&mut r).unwrap().0.as_deref(),
            Some("JP")
        );
        assert_eq!(r.remaining(), 0);
    }
}
