//! Android 16 TimeZoneDetectorService, with the Mac as the automatic origin.
//! Host adaptation (#283): the Mac occupies the original's passive external
//! automatic-source policy slot (telephony), without declaring radio hardware.
//! ConfigurationInternal derives RUNNING/RUNNING/NOT_SUPPORTED; Android user
//! configuration is forbidden. Geo, fallback, background geo, enhanced metrics
//! and notification support/settings are false; no location algorithm event.
//! The alarm owner follows vendor.aim.mac.time_zone and writes persist.sys.timezone
//! and persist.sys.timezone_confidence (alarm_service::follow_mac_zone/set_time_zone).
//! Sources: android-16.0.0_r1 frameworks/base, services/core/java/com/android/server/
//! timezonedetector/{ConfigurationInternal,TimeZoneDetectorService,TimeZoneDetectorStrategyImpl}.java
//! and core/java/android/app/time/{TimeZoneCapabilities,TimeZoneConfiguration}.java.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_BAD_PARCELABLE, EX_NULL_POINTER, Exception, Parcel, Reader,
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, android_app_time_itimezonedetectorlistener as listener,
    android_app_timezonedetector_itimezonedetectorservice as tz,
};

use super::{AppHost, reply, settings::Table};
use crate::bundle::{self, Value};

fn parcel_error(_: i32) -> Exception {
    Exception::new(EX_BAD_PARCELABLE, "Invalid time zone parcel")
}

const MANAGE: &str = "android.permission.MANAGE_TIME_AND_ZONE_DETECTION";
const SUGGEST: &str = "android.permission.SUGGEST_MANUAL_TIME_AND_ZONE";
const ZONE: &str = "persist.sys.timezone";
const CONFIDENCE: &str = "persist.sys.timezone_confidence";
const AUTO: &str = "autoDetectionEnabled";
const GEO: &str = "geoDetectionEnabled";
const NOTIFICATIONS: &str = "notificationsEnabled";

pub struct Configuration(HashMap<String, Value>);
impl ReadParcelable for Configuration {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        Ok(Self(bundle::read(r)?.unwrap_or_default()))
    }
}

pub struct ManualSuggestion;
impl ReadParcelable for ManualSuggestion {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        r.read_string16()?.ok_or(BAD_VALUE)?;
        // ManualTimeZoneSuggestion.writeList(debugInfo): null or String values.
        for _ in 0..r.read_i32()?.max(0) {
            match r.read_i32()? {
                bundle::VAL_NULL => (),
                bundle::VAL_STRING => {
                    r.read_string16()?;
                }
                _ => return Err(BAD_VALUE),
            }
        }
        Ok(Self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Inputs {
    passive: bool,
    geo: bool,
    auto: bool,
    allowed: bool,
    location: bool,
}
impl Inputs {
    fn host(location: bool) -> Self {
        Self {
            passive: true,
            geo: false,
            auto: true,
            allowed: false,
            location,
        }
    }
    // ConfigurationInternal.asCapabilities / StrategyImpl.create*Status.
    fn derive(self) -> ([i32; 5], [i32; 3]) {
        let supported = self.passive || self.geo;
        let automatic = supported && self.auto;
        let auto = if !supported {
            10
        } else if !self.allowed {
            20
        } else {
            40
        };
        let geo = if !self.geo || !self.passive {
            10
        } else if !self.auto || !self.location {
            30
        } else {
            40
        };
        let manual = if !self.allowed {
            20
        } else if automatic {
            30
        } else {
            40
        };
        let status = if !supported {
            1
        } else if automatic {
            3
        } else {
            2
        };
        (
            [auto, self.location as i32, geo, manual, 10],
            [
                status,
                if self.passive { 3 } else { 1 },
                if self.geo { 2 } else { 1 },
            ],
        )
    }
}
struct Capabilities {
    user: i32,
    inputs: Inputs,
}
impl WriteParcelable for Capabilities {
    fn write_to(&self, p: &mut Parcel) {
        // TimeZoneCapabilitiesAndConfig.writeParcelable fields, Android 16.
        p.write_string16(Some("android.app.time.TimeZoneDetectorStatus"));
        let (capabilities, status) = self.inputs.derive();
        p.write_i32(status[0]);
        p.write_string16(Some("android.app.time.TelephonyTimeZoneAlgorithmStatus"));
        p.write_i32(status[1]);
        p.write_string16(Some("android.app.time.LocationTimeZoneAlgorithmStatus"));
        p.write_i32(status[2]);
        let provider = if self.inputs.geo { 2 } else { 1 };
        p.write_i32(provider);
        p.write_string16(None);
        p.write_i32(provider);
        p.write_string16(None);
        p.write_string16(Some("android.app.time.TimeZoneCapabilities"));
        p.write_i32(self.user);
        for capability in capabilities {
            p.write_i32(capability);
        }
        p.write_string16(Some("android.app.time.TimeZoneConfiguration"));
        bundle::write(
            p,
            &[
                (AUTO, Value::Bool(self.inputs.auto)),
                (GEO, Value::Bool(false)),
                (NOTIFICATIONS, Value::Bool(false)),
            ],
        );
    }
}

struct ZoneState {
    zone: String,
    confirm: bool,
}
impl WriteParcelable for ZoneState {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(Some(&self.zone));
        p.write_i32(self.confirm as i32);
    }
}

pub struct TimeZoneDetector {
    host: Arc<AppHost>,
    listeners: Arc<Mutex<HashMap<u32, (Strong, u64)>>>,
    configuration: Mutex<Option<Inputs>>,
}
impl TimeZoneDetector {
    pub fn new(host: Arc<AppHost>) -> Self {
        Self {
            configuration: Mutex::new(None),
            host,
            listeners: Default::default(),
        }
    }

    fn permission(&self, uid: i32, permission: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? != 0 {
            return Err(Exception::security(format!("{permission} required")));
        }
        Ok(())
    }

    fn listener(&self, binder: Option<Binder>, add: bool) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::new(EX_NULL_POINTER, "null listener"));
        };
        let mut listeners = self.listeners.lock().unwrap();
        if add {
            self.configuration
                .lock()
                .unwrap()
                .get_or_insert(Inputs::host(Self::location(&self.host)?));
            if listeners.contains_key(&handle) {
                return Ok(());
            }
            let strong = self.host.process.strong(handle);
            let weak = Arc::downgrade(&self.listeners);
            let cookie = self.host.process.link_to_death(
                &strong,
                Box::new(move || {
                    if let Some(listeners) = weak.upgrade() {
                        listeners.lock().unwrap().remove(&handle);
                    }
                }),
            );
            listeners.insert(handle, (strong, cookie));
        } else if let Some((strong, cookie)) = listeners.remove(&handle) {
            self.host.process.clear_death(&strong, cookie);
        }
        Ok(())
    }

    fn location(host: &AppHost) -> Result<bool, Exception> {
        Ok(host
            .inputs()?
            .settings
            .get(Table::Secure, "location_mode")
            .and_then(|v| v.parse::<i32>().ok())
            .is_some_and(|v| v != 0))
    }

    // Cache ConfigurationInternal before posting, as the original strategy does.
    pub(super) fn location_changed(&self) -> Result<(), Exception> {
        let new = Inputs::host(Self::location(&self.host)?);
        let mut current = self.configuration.lock().unwrap();
        if current.replace(new).is_some_and(|old| old != new) {
            self.host.post_job(|host| {
                let Some(detector) = host.time_zone_detector.get() else {
                    return;
                };
                let mut p = Parcel::new();
                listener::OnChange {}.write(&mut p);
                for (listener, _) in detector.listeners.lock().unwrap().values() {
                    if let Err(e) = listener.transact(listener::ON_CHANGE, &p, true) {
                        eprintln!("app host: time_zone_detector: Unable to notify listener: {e}");
                    }
                }
            });
        }
        Ok(())
    }
    fn confidence(&self) -> i32 {
        self.host
            .property(CONFIDENCE)
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|v| (0..=100).contains(v))
            .unwrap_or(0)
    }
}

impl Service for TimeZoneDetector {
    fn descriptor(&self) -> &str {
        tz::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = (|| {
            let permission = if call.code == tz::SUGGEST_MANUAL_TIME_ZONE {
                SUGGEST
            } else {
                MANAGE
            };
            match call.code {
                tz::GET_CAPABILITIES_AND_CONFIG => {
                    tz::GetCapabilitiesAndConfig::read(&mut call.data).map_err(parcel_error)?;
                    self.permission(call.sender_euid as i32, permission)?;
                    tz::write_get_capabilities_and_config_reply(
                        &mut p,
                        Some(&Capabilities {
                            user: call.sender_euid as i32 / 100_000,
                            inputs: Inputs::host(Self::location(&self.host)?),
                        }),
                    );
                }
                tz::GET_TIME_ZONE_STATE => {
                    tz::GetTimeZoneState::read(&mut call.data).map_err(parcel_error)?;
                    self.permission(call.sender_euid as i32, permission)?;
                    tz::write_get_time_zone_state_reply(
                        &mut p,
                        Some(&ZoneState {
                            zone: self.host.property(ZONE).unwrap_or_default(),
                            confirm: self.confidence() < 100,
                        }),
                    );
                }
                tz::ADD_LISTENER | tz::REMOVE_LISTENER => {
                    let listener = if call.code == tz::ADD_LISTENER {
                        tz::AddListener::read(&mut call.data)
                            .map_err(parcel_error)?
                            .listener
                    } else {
                        tz::RemoveListener::read(&mut call.data)
                            .map_err(parcel_error)?
                            .listener
                    };
                    self.permission(call.sender_euid as i32, permission)?;
                    self.listener(listener, call.code == tz::ADD_LISTENER)?;
                    p.write_no_exception();
                }
                tz::UPDATE_CONFIGURATION => {
                    let a = tz::UpdateConfiguration::<Configuration>::read(&mut call.data)
                        .map_err(parcel_error)?;
                    self.permission(call.sender_euid as i32, permission)?;
                    let c = a
                        .configuration
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "null configuration"))?;
                    // tryApplyConfigChanges checks capability even for an unchanged value.
                    tz::write_update_configuration_reply(
                        &mut p,
                        [AUTO, GEO, NOTIFICATIONS]
                            .iter()
                            .zip(
                                Inputs::host(Self::location(&self.host)?)
                                    .derive()
                                    .0
                                    .into_iter()
                                    .step_by(2),
                            )
                            .all(|(key, capability)| !c.0.contains_key(*key) || capability >= 30),
                    );
                }
                tz::SET_MANUAL_TIME_ZONE | tz::SUGGEST_MANUAL_TIME_ZONE => {
                    let suggestion = if call.code == tz::SET_MANUAL_TIME_ZONE {
                        tz::SetManualTimeZone::<ManualSuggestion>::read(&mut call.data)
                            .map_err(parcel_error)?
                            .time_zone_suggestion
                    } else {
                        tz::SuggestManualTimeZone::<ManualSuggestion>::read(&mut call.data)
                            .map_err(parcel_error)?
                            .time_zone_suggestion
                    };
                    self.permission(call.sender_euid as i32, permission)?;
                    suggestion.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null suggestion"))?;
                    p.write_no_exception();
                    p.write_i32(0); // NOT_ALLOWED
                }
                tz::CONFIRM_TIME_ZONE => {
                    let a = tz::ConfirmTimeZone::read(&mut call.data).map_err(parcel_error)?;
                    self.permission(call.sender_euid as i32, permission)?;
                    let zone = a
                        .time_zone_id
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "null timeZoneId"))?;
                    let matches = self.host.property(ZONE).unwrap_or_default() == zone;
                    if matches && self.confidence() < 100 {
                        self.host.set_property(CONFIDENCE, "100");
                    }
                    tz::write_confirm_time_zone_reply(&mut p, matches);
                }
                code => {
                    return Err(self.host.unimplemented(
                        "time_zone_detector",
                        tz::DESCRIPTOR,
                        code,
                    ));
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(p),
            Err(e) => reply(Err(e)),
        }
    }
}

#[cfg(test)]
#[path = "time_zone_detector_tests.rs"]
mod tests;
