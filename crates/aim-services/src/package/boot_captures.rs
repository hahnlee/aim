//! Historical boot-session queries follow live native snapshot leases.
use super::{bootstrap::Bridge, scan_snapshot::{query_state::Capture, endpoint::Endpoint}, resolve::Resolver, query::Query};
use crate::system::System;
use aim_binder_host::parcel::{Binder, Exception, EX_ILLEGAL_STATE};
use std::{collections::BTreeMap, sync::{Arc, Mutex, Weak}};

pub struct Captures {
    system: Weak<System>, bridge: Arc<Bridge>,
    versions: Mutex<BTreeMap<u64, Weak<Capture>>>, resolver: Resolver,
}
fn error(message: impl Into<String>) -> Exception { Exception::new(EX_ILLEGAL_STATE, message) }
impl Captures {
    pub fn new(system: &Arc<System>, bridge: Arc<Bridge>) -> Self {
        Self { system: Arc::downgrade(system), bridge, versions: Mutex::new(BTreeMap::new()), resolver: Resolver::default() }
    }
    fn system(&self) -> Result<Arc<System>, Exception> {
        let system = self.system.upgrade().ok_or_else(|| error("boot capture system stopped"))?;
        system.check_package_bootstrap(&self.bridge)?;
        Ok(system)
    }
    pub fn snapshot(&self) -> Result<Binder, Exception> {
        let system = self.system()?;
        let capture = system.capture_package_queries()?;
        let mut versions = self.versions.lock().unwrap();
        versions.retain(|_, capture| capture.strong_count() != 0);
        versions.insert(capture.scan().version(), Arc::downgrade(&capture));
        drop(versions);
        Ok(system.binder_process().add_service(Arc::new(Endpoint::with_computer(
            capture.scan().clone(), capture, &system.binder_process()))))
    }
    pub fn capture(&self, version: i64) -> Result<Arc<Capture>, Exception> {
        let system = self.system()?;
        let version = u64::try_from(version).map_err(|_| Exception::illegal_argument("negative boot capture version"))?;
        let current = system.capture_package_queries()?;
        if current.scan().version() == version { return Ok(current); }
        self.versions.lock().unwrap().get(&version).and_then(Weak::upgrade)
            .ok_or_else(|| error("requested boot capture is no longer retained"))
    }
    pub fn filter_package_name(&self, version: i64, name: &str, uid: i32, user: i32) -> Result<Option<String>, Exception> {
        let capture = self.capture(version)?;
        let resolution = self.resolver.resolution(capture.state()).map_err(|cause| error(format!("boot visibility resolution: {cause:?}")))?;
        Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: uid }
            .internal_filtered_package_name(name, user).map_err(|cause| error(cause.0))
    }
    pub fn should_filter(&self, version: i64, name: &str, uid: i32, user: i32) -> Result<bool, Exception> {
        let capture = self.capture(version)?;
        let resolution = self.resolver.resolution(capture.state()).map_err(|cause| error(format!("boot visibility resolution: {cause:?}")))?;
        Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: uid }
            .filtered(capture.state().packages.get(name), uid, user).map_err(|cause| error(cause.0))
    }
    pub fn bootstrap_state(&self, version: i64) -> Result<Vec<u8>, Exception> {
        let capture = self.capture(version)?;
        let lifecycle = capture.state().system.lifecycle.as_ref().ok_or_else(|| error("boot lifecycle unavailable"))?;
        let web = capture.state().system.web_instant_policy.as_ref().ok_or_else(|| error("boot web policy unavailable"))?;
        let mut record = aim_binder_host::parcel::Parcel::new();
        record.write_i32(2);
        record.write_bool(lifecycle.first_boot());
        record.write_bool(lifecycle.device_upgrading());
        record.write_bool(capture.context().cross_user_suspensions);
        record.write_bool(lifecycle.safe_mode());
        let platform = capture.state().packages.get("android").and_then(|state| state.pkg.as_ref())
            .ok_or_else(|| error("accepted platform package absent"))?;
        record.write_string16(Some(&platform.package_name));
        record.write_i32(i32::try_from(web.disabled.len()).map_err(|_| error("boot web policy count exceeds original range"))?);
        for (user, disabled) in &web.disabled { record.write_i32(*user); record.write_bool(*disabled); }
        Ok(record.data().to_vec())
    }
    pub fn close(&self) { self.versions.lock().unwrap().clear(); }
}
