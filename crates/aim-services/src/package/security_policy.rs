//! Native ProtectedPackages and original AppOps/Settings/DevicePolicy leaves.
//! android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
use super::{parse::{Platform, resources::{Config, Resources}}, query::Query, apps_filter::NotModelled};
use aim_binder_host::parcel::Exception;
use std::{collections::{BTreeMap, BTreeSet}, sync::Mutex};
pub type InstallDisabled = Box<dyn Fn(&str, i32, i32) -> Result<bool, Exception> + Send + Sync>;
pub type AutoRevoke = Box<dyn Fn(i32, Option<&str>) -> Result<i32, Exception> + Send + Sync>;
pub struct AdminFacts {
    pub managers_present: bool,
    pub device_owner: Option<String>,
    pub active_admin_users: BTreeSet<i32>,
    pub managed_role_holder_users: BTreeSet<i32>,
}
pub type DeviceAdmin = Box<dyn Fn(Option<&str>) -> Result<AdminFacts, Exception> + Send + Sync>;
pub type InstallLocation = Box<dyn Fn() -> Result<i32, Exception> + Send + Sync>;
#[derive(Default)]
struct Protected {
    device_owner_user: i32, device_owner: Option<String>, profiles: Option<BTreeMap<i32, Option<String>>>,
    owners: BTreeMap<i32, BTreeSet<Option<String>>>,
}
pub struct Owner {
    provisioning: String, protected: Mutex<Protected>,
    disabled: InstallDisabled, auto_revoke: AutoRevoke, admin: DeviceAdmin, location: InstallLocation,
}
impl Owner {
    pub fn new(platform: &Platform, config: Config, disabled: InstallDisabled, auto_revoke: AutoRevoke,
        admin: DeviceAdmin, location: InstallLocation) -> Result<Self, String> {
        let resources = Resources { tables: vec![&platform.framework], overlays: &platform.framework_overlays, config };
        let provisioning = platform.framework.id("string", "config_deviceProvisioningPackage")
            .and_then(|id| resources.resource_string(id)).ok_or("native ProtectedPackages provisioning resource unavailable")?;
        Ok(Self { provisioning, protected: Mutex::new(Protected::default()), disabled, auto_revoke, admin, location })
    }
    pub fn set_device_and_profile_owners(&self, user: i32, package: Option<String>, profiles: Option<BTreeMap<i32, Option<String>>>) {
        let mut state = self.protected.lock().unwrap(); state.device_owner_user = user;
        state.device_owner = if user == -10000 { None } else { package }; state.profiles = profiles;
    }
    pub fn set_owner_protected(&self, user: i32, packages: Option<Vec<Option<String>>>) {
        let mut state = self.protected.lock().unwrap();
        if let Some(packages) = packages { state.owners.insert(user, packages.into_iter().collect()); }
        else { state.owners.remove(&user); }
    }
    pub fn protected(&self, user: i32, package: Option<&str>) -> bool {
        let Some(package) = package else { return false; };
        let state = self.protected.lock().unwrap();
        state.device_owner_user == user && state.device_owner.as_deref() == Some(package)
            || state.profiles.as_ref().and_then(|profiles| profiles.get(&user)).and_then(|name| name.as_deref()) == Some(package)
            || package == self.provisioning
            || state.owners.get(&user).or_else(|| state.owners.get(&-1)).is_some_and(|names| names.contains(&Some(package.into())))
    }
    pub fn owner_package(&self, user: i32) -> Option<String> {
        let state = self.protected.lock().unwrap();
        if state.device_owner_user == user { return state.device_owner.clone(); }
        state.profiles.as_ref().and_then(|profiles| profiles.get(&user)).cloned().flatten()
    }
    pub fn device_provisioning(&self, package: Option<&str>) -> bool { !self.provisioning.is_empty() && package == Some(self.provisioning.as_str()) }
    pub fn install_disabled(&self, package: &str, uid: i32, user: i32) -> Result<bool, Exception> { (self.disabled)(package, uid, user) }
    pub fn auto_revoke(&self, uid: i32, package: Option<&str>) -> Result<i32, Exception> { (self.auto_revoke)(uid, package) }
    pub fn admin_any_user(&self, package: Option<&str>, setting_exists: bool) -> Result<bool, Exception> {
        let facts = (self.admin)(package)?;
        if !facts.managers_present { return Ok(false); }
        if package.is_none() { return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName is null")); }
        if package.is_some() && package == facts.device_owner.as_deref() || !facts.active_admin_users.is_empty() { return Ok(true); }
        Ok(setting_exists && !facts.managed_role_holder_users.is_empty())
    }
    pub fn install_location(&self) -> Result<i32, Exception> { (self.location)() }
}
impl std::fmt::Debug for Owner { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("PackageSecurityPolicy").finish_non_exhaustive() } }
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
impl Query<'_> {
    pub(crate) fn security_policy(&self) -> Result<&Owner, NotModelled> {
        self.state.system.security_policy.as_deref().ok_or(NotModelled("native package security policy owner unavailable"))
    }
}
