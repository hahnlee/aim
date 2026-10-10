//! Native setInstallerPackageName owner, pinned PMS android-16.0.0_r1.
//! Copyright The Android Open Source Project, Apache License 2.0.
use super::{apps_filter, effects, info, model, query::Query, scan::SigningScan, settings::Signatures};
use aim_binder_host::parcel::{Exception, Reader, BAD_VALUE, EX_ILLEGAL_STATE};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

#[derive(Debug)]
pub struct Request { pub target: Option<String>, pub installer: Option<String> }
#[derive(Clone, Debug)]
pub struct Plan { pub target: String, pub installer: Option<String>, pub installer_uid: i32 }
impl Request {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, i32> {
        let args = pm::SetInstallerPackageName::read(reader)?;
        if reader.remaining() != 0 { return Err(BAD_VALUE); }
        Ok(Self { target: args.target_package, installer: args.installer_package_name })
    }
    /// Invoke again against the current native capture after any intervening
    /// package/state change. The original records installer UID before its
    /// retry and retains that UID, so the caller keeps the first Plan's UID.
    pub fn prepare(&self, query: &Query<'_>, effects: &effects::Owner) -> Result<Option<Plan>, Exception> {
        let uid = query.calling_uid;
        let user = apps_filter::user_id(uid);
        if apps_filter::instant_app_package_name(query.state, uid).map_err(missing)?.is_some() { return Ok(None); }
        let target = self.target.as_ref().and_then(|name| query.state.packages.get(name));
        let Some(target) = target.filter(|package| info::user_state(package, user).installed) else {
            return Err(Exception::illegal_argument(format!("Unknown target package: {}", self.target.as_deref().unwrap_or("null"))));
        };
        if query.filtered(Some(target), uid, user).map_err(missing)? {
            return Err(Exception::illegal_argument(format!("Unknown target package: {}", target.name)));
        }
        let installer = match self.installer.as_ref() {
            None => None,
            Some(name) => {
                let package = query.state.packages.get(name).filter(|package| info::user_state(package, user).installed);
                if package.is_none() || query.filtered(package, uid, user).map_err(missing)? {
                    return Err(Exception::illegal_argument(format!("Unknown installer package: {name}")));
                }
                package
            }
        };
        let registry = query.state.uid_owners.as_ref().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "native UID registry unavailable"))?;
        let calling = registry.get(&apps_filter::app_id(uid)).ok_or_else(|| Exception::security(format!("Unknown calling UID: {uid}")))?;
        let caller = match calling {
            model::UidOwner::Package(package) => package.signatures.as_ref(),
            model::UidOwner::SharedUser(name) => query.state.shared_users.get(name).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "registered shared signing owner unavailable"))?.signatures.as_ref(),
        };
        if let Some(installer) = installer {
            if !same_current_signers(caller, installer.signatures.as_ref()) {
                return Err(Exception::security(format!("Caller does not have same cert as new installer package {}", installer.name)));
            }
        }
        let old = target.install_source.installer.as_ref().and_then(|name| query.state.packages.get(name));
        if let Some(old) = old {
            // The old installer is looked up without installed/visibility gates.
            if !same_current_signers(caller, old.signatures.as_ref()) {
                return Err(Exception::security(format!("Caller does not have same cert as old installer package {}", old.name)));
            }
        } else if !matches!(apps_filter::app_id(uid), 0 | 1000)
            && !query.uid_has_permission(uid, "android.permission.INSTALL_PACKAGES").map_err(missing)? {
            // This invokes original EventLog and the real PlatformCompat UID
            // decision; a disabled compatibility change really returns silently.
            if effects.require_installer_permission(uid)? {
                return Err(Exception::security(format!("Neither user {uid} nor current process has android.permission.INSTALL_PACKAGES")));
            }
            return Ok(None);
        }
        let installer_uid = match self.installer.as_deref() {
            None => -1,
            Some(name) => query.package_uid(name, 0, user).map_err(missing)??,
        };
        Ok(Some(Plan { target: target.name.clone(), installer: self.installer.clone(), installer_uid }))
    }
}
impl Plan {
    pub fn reauthorize(&self, query: &Query<'_>, effects: &effects::Owner) -> Result<Option<Self>, Exception> {
        Request { target: Some(self.target.clone()), installer: self.installer.clone() }.prepare(query, effects)
            .map(|plan| plan.map(|mut plan| { plan.installer_uid = self.installer_uid; plan }))
    }
    pub fn apply_scan(&self, scan: &mut SigningScan) -> Result<(), String> {
        let package = scan.settings.packages.iter_mut().find(|package| package.name == self.target).ok_or("installer attribution target absent")?;
        if package.install_source.installer != self.installer {
            package.install_source.installer = self.installer.clone();
            package.install_source.installer_uid = self.installer_uid;
        }
        // Preserve initiating/originating/update owner, attribution tag, source,
        // orphan marker and historical signing. Disabled factories are untouched.
        scan.installers.add(&package.install_source);
        Ok(())
    }
    /// Root publishes the prepared capture through its generation gate. Rebuild
    /// native AppsFilter from that full capture before exposing the new
    /// install-source edge; client cache invalidation follows actual publication.
    pub fn finish_publication(&self, bridge: &super::bootstrap::Bridge) -> Result<(), Exception> {
        bridge.invalidate_package_info_cache().map_err(|error| match error {
            super::bootstrap::OwnerError::Owner(error) => error,
            other => Exception::new(EX_ILLEGAL_STATE, format!("installer attribution committed, cache invalidation failed: {other:?}")),
        })
    }
    pub fn persist(&self, disk: &mut super::owner::Store, scan: &SigningScan) -> Result<(), super::owner::WriteError> {
        disk.commit_scan_settings_owner(scan)
    }
}
/// This setter compares only current cert arrays. Rotation ancestry and signing
/// capabilities intentionally do not grant attribution changes.
fn same_current_signers(first: Option<&Signatures>, second: Option<&Signatures>) -> bool {
    let first = first.map(|value| value.signatures.as_slice());
    let second = second.map(|value| value.signatures.as_slice());
    match (first, second) {
        (Some(first), Some(second)) if first.len() == second.len() => {
            if first.len() == 1 { first[0] == second[0] }
            else { first.iter().all(|certificate| second.contains(certificate)) && second.iter().all(|certificate| first.contains(certificate)) }
        }
        _ => false,
    }
}
fn missing(error: apps_filter::NotModelled) -> Exception { Exception::new(EX_ILLEGAL_STATE, error.0) }
