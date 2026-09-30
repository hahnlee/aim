//! The GNSS side of the location service (`GnssManagerService` and its
//! listener multiplexers), over the Mac's location: no satellites, NMEA,
//! measurements, navigation messages or antenna information exist, as
//! with the GNSS HAL (`hal/gnss`), whose capabilities (scheduling only)
//! and system information the service reports.
//!
//! GNSS status listeners hear when the gps provider starts and stops
//! navigating and of its first fix after each start, as the HAL's session
//! status reaches them. Measurement and navigation message listeners are
//! told they are ready and are never active: collection is not supported.

use std::collections::BTreeMap;
use std::sync::Arc;

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::{
    android_location_ignssmeasurementslistener as measurements,
    android_location_ignssnavigationmessagelistener as navigation,
    android_location_ignssstatuslistener as status,
};

use super::env::{Env, Identity};
use super::parcels::GnssCapabilities;

/// `IGnssCallback.CAPABILITY_SCHEDULING`, the HAL's only capability.
const CAPABILITY_SCHEDULING: i32 = 1;
/// The HAL's `GnssSystemInfo`.
pub const HARDWARE_MODEL_NAME: &str = "darwin CoreLocation";
pub const YEAR_OF_HARDWARE: i32 = 0;
/// `GnssMeasurementsEvent.Callback.STATUS_READY` and
/// `GnssNavigationMessage.Callback.STATUS_READY`.
const STATUS_READY: i32 = 1;

/// `GnssNative.getCapabilities()` with the HAL's capabilities.
pub fn capabilities() -> GnssCapabilities {
    GnssCapabilities {
        top_flags: CAPABILITY_SCHEDULING,
        ..GnssCapabilities::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Status,
    Nmea,
    Measurements,
    NavigationMessages,
    AntennaInfo,
}

pub struct Registration {
    pub identity: Identity,
    pub listener: Arc<Strong>,
    pub cleanup: Vec<Box<dyn FnOnce() + Send>>,
}

#[derive(Default)]
pub struct Gnss {
    registrations: BTreeMap<(Kind, u32), Registration>,
    navigating: bool,
    has_first_fix: bool,
    start_ms: i64,
}

impl Gnss {
    /// `addListener`: replaces a registration of the same binder. A
    /// measurement or navigation message listener is told it is ready.
    pub fn add(&mut self, kind: Kind, handle: u32, registration: Registration) {
        let ready = match kind {
            Kind::Measurements => Some(measurements::ON_STATUS_CHANGED),
            Kind::NavigationMessages => Some(navigation::ON_STATUS_CHANGED),
            _ => None,
        };
        if let Some(code) = ready {
            let mut data = Parcel::new();
            match kind {
                Kind::Measurements => measurements::OnStatusChanged {
                    status: STATUS_READY,
                }
                .write(&mut data),
                _ => navigation::OnStatusChanged {
                    status: STATUS_READY,
                }
                .write(&mut data),
            }
            let _ = registration.listener.transact(code, &data, true);
        }
        self.remove(kind, handle);
        self.registrations.insert((kind, handle), registration);
    }

    pub fn has_package(&self, package: &str) -> bool {
        self.registrations
            .values()
            .any(|r| r.identity.package == package)
    }

    /// `onPackageReset`: the package's listeners go.
    pub fn package_reset(&mut self, package: &str) {
        let keys: Vec<(Kind, u32)> = self
            .registrations
            .iter()
            .filter(|(_, r)| r.identity.package == package)
            .map(|(k, _)| *k)
            .collect();
        for (kind, handle) in keys {
            self.remove(kind, handle);
        }
    }

    pub fn remove(&mut self, kind: Kind, handle: u32) {
        if let Some(mut r) = self.registrations.remove(&(kind, handle)) {
            for undo in r.cleanup.drain(..) {
                undo();
            }
        }
    }

    /// The registrations a status delivery goes to: fine location
    /// permission, foreground or exempt from background restrictions, and
    /// a caller for whom the gps provider is on (`isActive`).
    fn active_status(&self, env: &Env, active: &dyn Fn(&Identity) -> bool) -> Vec<Arc<Strong>> {
        self.registrations
            .iter()
            .filter(|((kind, _), _)| *kind == Kind::Status)
            .filter(|(_, r)| {
                let i = &r.identity;
                env.has_location_permissions(super::env::PERMISSION_FINE, i)
                    && (env.foreground(i.uid) || background_exempt(env, i))
                    && active(i)
            })
            .map(|(_, r)| r.listener.clone())
            .collect()
    }

    /// The gps provider's request became active or inactive: the HAL's
    /// session began or ended (`GnssStatusProvider.onReportStatus`).
    pub fn set_navigating(
        &mut self,
        env: &Env,
        navigating: bool,
        active: &dyn Fn(&Identity) -> bool,
    ) {
        if navigating == self.navigating {
            return;
        }
        self.navigating = navigating;
        if navigating {
            self.has_first_fix = false;
            self.start_ms = env.now_ms();
        }
        let code = if navigating {
            status::ON_GNSS_STARTED
        } else {
            status::ON_GNSS_STOPPED
        };
        for listener in self.active_status(env, active) {
            let mut data = Parcel::new();
            data.write_interface_token(status::DESCRIPTOR);
            let _ = listener.transact(code, &data, true);
        }
    }

    /// A gps fix: the first after a start is reported with the time it
    /// took (`GnssNative.reportLocation`).
    pub fn on_fix(&mut self, env: &Env, active: &dyn Fn(&Identity) -> bool) {
        if self.has_first_fix {
            return;
        }
        self.has_first_fix = true;
        let ttff = (env.now_ms() - self.start_ms) as i32;
        for listener in self.active_status(env, active) {
            let mut data = Parcel::new();
            status::OnFirstFix { ttff }.write(&mut data);
            let _ = listener.transact(status::ON_FIRST_FIX, &data, true);
        }
    }
}

/// `isBackgroundRestrictionExempt`.
fn background_exempt(env: &Env, identity: &Identity) -> bool {
    identity.uid == super::env::SYSTEM_UID
        || env
            .background_throttle_package_whitelist()
            .contains(&identity.package)
        || env.is_provider(None, identity)
}
