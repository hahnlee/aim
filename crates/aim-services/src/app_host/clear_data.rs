//! A package's data cleared in the app host (`android-16.0.0_r1`):
//! ActivityManagerService's `clearApplicationUserData`, with its checks and
//! the package's force stop, around PackageManagerService's on its handler
//! (`clearApplicationUserDataLIF`: the runtime permissions reset, installd's
//! `clearAppData`, the keystore namespace, the native library link; the
//! instant-app metadata; the components' states of a package that asks),
//! then `ACTION_PACKAGE_RESTARTED` and `ACTION_PACKAGE_DATA_CLEARED`, and
//! the package's URI grants, notification state, jobs and alarms. The
//! package is not frozen while its data goes (`PackageFreezer`, #1986);
//! what a force stop does to its activities, services, providers and
//! recent tasks is #1570.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_content_pm_ipackagedataobserver as data_observer;
use aim_service_aidl::android_security_maintenance_ikeystoremaintenance as maintenance;
use aim_service_aidl::android_system_keystore2_domain as domain;

use super::app_data::{FLAG_STORAGE_CE, FLAG_STORAGE_DE};
use super::job_scheduler::{INTERNAL_STOP_REASON_DATA_CLEARED, STOP_REASON_USER};
use super::{AppHost, PER_USER_RANGE};
use crate::package::apps_filter::NotModelled;
use crate::package::info::flags::MATCH_UNINSTALLED_PACKAGES;
use crate::package::info::user_state;
use crate::package::model::PackageState;
use crate::package::query::Query;

const CLEAR_APP_USER_DATA: &str = "android.permission.CLEAR_APP_USER_DATA";
const MANAGE_USERS: &str = "android.permission.MANAGE_USERS";
const ACCESS_INSTANT_APPS: &str = "android.permission.ACCESS_INSTANT_APPS";
const SUSPEND_APPS: &str = "android.permission.SUSPEND_APPS";
/// `StorageManager.FLAG_STORAGE_EXTERNAL`.
const FLAG_STORAGE_EXTERNAL: i32 = 4;
/// `ApplicationInfo.PRIVATE_FLAG_INSTANT`.
const PRIVATE_FLAG_INSTANT: i32 = 1 << 7;
/// `Process.INVALID_UID`, `SYSTEM_UID`.
const INVALID_UID: i32 = -1;
const SYSTEM_UID: i32 = 1000;
/// `InstantAppRegistry`'s files of an instant app.
const INSTANT_APP_FILES: [&str; 3] = ["metadata.xml", "icon.png", "android_id"];
const INSTANT_APP_COOKIE_FILE_PREFIX: &str = "cookie_";

/// What learns whether the data was cleared
/// (`IPackageDataObserver.onRemoveCompleted(packageName, succeeded)`).
pub(super) type Done = Box<dyn FnOnce(bool) + Send>;

/// `observer`'s `onRemoveCompleted`, one-way; none without one.
pub(super) fn observer_of(host: &AppHost, package: Option<&str>, observer: Option<Binder>) -> Done {
    let target = match observer {
        Some(Binder::Handle(handle)) => Some(Ok(host.process.strong(handle))),
        Some(Binder::Local(ptr)) => host.process.local_service(ptr).map(Err),
        None => None,
    };
    let package = package.map(str::to_string);
    Box::new(move |succeeded| {
        let Some(target) = target else { return };
        let mut data = Parcel::new();
        data_observer::OnRemoveCompleted {
            package_name: package,
            succeeded,
        }
        .write(&mut data);
        let sent = match target {
            Ok(strong) => strong.transact(data_observer::ON_REMOVE_COMPLETED, &data, true),
            Err(local) => local.transact(data_observer::ON_REMOVE_COMPLETED, &data, true),
        };
        if sent.is_err() {
            eprintln!("app host: Observer no longer exists.");
        }
    })
}

/// `InstantAppRegistry.getInstantApplicationDir`.
fn instant_app_dir(data: &Path, package: &str, user: i32) -> PathBuf {
    data.join(format!("system/users/{user}/instant/{package}"))
}

/// `peekInstantCookieFile`: the instant app's cookie file, if any.
fn instant_cookie(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(INSTANT_APP_COOKIE_FILE_PREFIX)
            .then(|| e.path())
    })
}

/// `hasInstantApplicationMetadata`: its metadata, icon, Android id or
/// cookie kept (an uninstalled instant app's state is its metadata file).
fn has_instant_metadata(data: &Path, package: &str, user: i32) -> bool {
    let dir = instant_app_dir(data, package, user);
    INSTANT_APP_FILES.iter().any(|f| dir.join(f).exists()) || instant_cookie(&dir).is_some()
}

/// `deleteInstantApplicationMetadata`.
fn delete_instant_metadata(data: &Path, package: &str, user: i32) {
    let dir = instant_app_dir(data, package, user);
    for file in INSTANT_APP_FILES
        .iter()
        .map(|f| dir.join(f))
        .chain(instant_cookie(&dir))
    {
        // `File.delete`: a file that is not there is no failure.
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                eprintln!("app host: deleting {}: {e}", file.display());
            }
            _ => {}
        }
    }
}

impl AppHost {
    fn not_modelled(&self, e: NotModelled) -> Exception {
        self.unimplemented_named("package", &format!("{} (#1425)", e.0))
    }

    fn writes(&self) -> Result<&Arc<super::package_writes::Writes>, Exception> {
        self.package_writes
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "the package service is not started"))
    }

    /// `ProtectedPackages.isPackageDataProtected`: as its state is (a
    /// device or profile owner's packages are #1521).
    pub(super) fn data_protected(&self, package: &str) -> Result<bool, Exception> {
        self.writes()?.protected(package)?.ok_or_else(|| {
            self.unimplemented_named("activity", "a device or profile owner's packages (#1521)")
        })
    }

    /// `IActivityManager.clearApplicationUserData(package, keepState,
    /// observer, user)` of the caller `uid` (of `pid`): whether the clear
    /// was started; `done` learns its outcome.
    pub(super) fn clear_application_user_data(
        self: &Arc<Self>,
        uid: i32,
        pid: i32,
        package: Option<&str>,
        keep_state: bool,
        done: Done,
        user: i32,
    ) -> Result<bool, Exception> {
        if super::is_isolated(uid) {
            return Err(Exception::security(
                "Isolated process not allowed to call clearApplicationUserData",
            ));
        }
        let user = self.incoming_user(uid, user, false)?;
        if user < 0 {
            return Err(Exception::illegal_argument(format!(
                "Call does not support special user #{user}"
            )));
        }
        // Instant packages are not protected.
        let mut permitted = true;
        if let Some(package) = package
            && self.data_protected(package)?
        {
            if self.check_permission(Some(MANAGE_USERS), uid)? == 0 {
                return Err(Exception::security(format!(
                    "Cannot clear data for a protected package: {package}"
                )));
            }
            permitted = false;
        }
        let (state, resolution) = self.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let info = match package {
            Some(package) if permitted => query
                .application_info(package, MATCH_UNINSTALLED_PACKAGES, user)
                .map_err(|e| self.not_modelled(e))??,
            _ => None,
        };
        let name = package.unwrap_or("null");
        permitted = permitted
            && (info.as_ref().is_some_and(|i| i.uid == uid)
                || self.check_component_permission(Some(CLEAR_APP_USER_DATA), uid, -1, true)?);
        if !permitted {
            return Err(Exception::security(format!(
                "PID {pid} does not have permission {CLEAR_APP_USER_DATA} to clear data of package {name}"
            )));
        }
        // `new File(dir, null)`.
        let package = package.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let data = self.data()?.to_path_buf();
        let instant_metadata = has_instant_metadata(&data, package, user);
        let instant = info
            .as_ref()
            .is_some_and(|i| i.private_flags & PRIVATE_FLAG_INSTANT != 0)
            || instant_metadata;
        let instant_visible =
            self.check_component_permission(Some(ACCESS_INSTANT_APPS), uid, -1, true)?;
        if info.is_none() && !instant_metadata || instant && !instant_visible {
            eprintln!("app host: Invalid packageName: {package}");
            done(false);
            return Ok(false);
        }
        let app_uid = info.as_ref().map(|i| i.uid);
        if let Some(app_uid) = app_uid {
            self.force_stop_package(
                package,
                app_uid.rem_euclid(PER_USER_RANGE as i32),
                user,
                "clear data",
            );
        }
        let writes = self.writes()?.clone();
        let name = package.to_string();
        let completed: Done = Box::new(move |succeeded| {
            if let Some(app_uid) = app_uid {
                writes.package_restarted(&name, app_uid);
            }
            if succeeded {
                writes.package_data_cleared(
                    &name,
                    app_uid.unwrap_or(INVALID_UID),
                    user,
                    false,
                    instant,
                );
            }
            done(succeeded);
        });
        self.clear_package_data(SYSTEM_UID, Some(package), user, completed)?;
        if let Some(app_uid) = app_uid {
            // Kept when the app is about to put back data that suits them.
            if !keep_state {
                self.remove_uri_permissions_for_package(package, user, true, false)?;
                if let Some(notifications) = self.notification_service.get() {
                    notifications.clear_data(package, app_uid, uid == app_uid)?;
                }
            }
            let jobs = self.job_scheduler.get().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "the job scheduler is not started")
            })?;
            jobs.cancel_jobs_for_uid(
                app_uid,
                true,
                STOP_REASON_USER,
                INTERNAL_STOP_REASON_DATA_CLEARED,
                "clear data",
            )?;
            let alarms = self
                .alarm_service
                .get()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "alarms are not started"))?;
            alarms.remove_alarms_for_uid(app_uid)?;
        }
        Ok(true)
    }

    /// `IPackageManager.clearApplicationUserData(package, observer, user)`
    /// of the caller `uid`: the checks, then the clear on the package
    /// handler.
    pub(super) fn clear_package_data(
        self: &Arc<Self>,
        uid: i32,
        package: Option<&str>,
        user: i32,
        done: Done,
    ) -> Result<(), Exception> {
        if self.check_permission(Some(CLEAR_APP_USER_DATA), uid)? != 0 {
            return Err(Exception::security(format!(
                "Access denied, requires: {CLEAR_APP_USER_DATA}"
            )));
        }
        let (state, resolution) = self.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        query
            .internal_enforce_cross_user(uid, user, true, false, "clear application data")
            .map_err(|e| self.not_modelled(e))??;
        // `getPackageStateForInstalledAndFiltered`, which knows no package
        // by a null name.
        let visible = match package.and_then(|p| state.packages.get(p)) {
            Some(ps) if user_state(ps, user).installed => !query
                .filtered(Some(ps), uid, user)
                .map_err(|e| self.not_modelled(e))?,
            _ => false,
        };
        let writes = self.writes()?.clone();
        let Some(package) = package.filter(|_| visible) else {
            writes.post(move || done(false));
            return Ok(());
        };
        if self.data_protected(package)? {
            return Err(Exception::security(format!(
                "Cannot clear data for a protected package: {package}"
            )));
        }
        let host = self.clone();
        let package = package.to_string();
        writes.post(move || {
            let succeeded = host.clear_package_data_now(&package, user);
            done(succeeded);
        });
        Ok(())
    }

    /// The clear on the package handler (`clearApplicationUserData`'s
    /// runnable): the package's processes killed, `clearApplicationUserDataLIF`,
    /// its instant metadata deleted and, once cleared, its components'
    /// states reset and the suspensions it made lifted.
    fn clear_package_data_now(self: &Arc<Self>, package: &str, user: i32) -> bool {
        let Ok(capture) = self.app_data_packages().inspect_err(|e| {
            eprintln!("app host: clearing {package}'s data: {}", e.message);
        }) else {
            return false;
        };
        let state = capture.state();
        let Some(ps) = state.packages.get(package) else {
            eprintln!("app host: Package named '{package}' doesn't exist.");
            return false;
        };
        // `freezePackage(package, USER_ALL, ...)`: the app host's one user.
        self.force_stop_package(package, ps.app_id, user, "clearApplicationUserData");
        let succeeded = match self.clear_app_data_lif(ps, user) {
            Ok(succeeded) => succeeded,
            Err(e) => {
                eprintln!("app host: clearing {package}'s data: {}", e.message);
                false
            }
        };
        match self.data() {
            Ok(data) => delete_instant_metadata(data, package, user),
            Err(e) => eprintln!("app host: {package}'s instant metadata: {}", e.message),
        }
        if !succeeded {
            return false;
        }
        if let Err(e) = self
            .writes()
            .and_then(|w| w.reset_components_on_data_cleared(package, user))
        {
            eprintln!("app host: resetting {package}'s components: {}", e.message);
        }
        if let Err(e) = self.unsuspend_for(ps, user) {
            eprintln!("app host: {package}'s suspensions: {}", e.message);
        }
        true
    }

    /// `clearApplicationUserDataLIF`: whether the package's data was
    /// cleared.
    fn clear_app_data_lif(&self, ps: &PackageState, user: i32) -> Result<bool, Exception> {
        if ps.pkg.is_none() {
            eprintln!("app host: Package named '{}' doesn't exist.", ps.name);
            return Ok(false);
        }
        self.permission_manager
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "permissionmgr is not started"))?
            .reset_runtime_permissions(&ps.name, user)?;
        let installd = self
            .installd()
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        self.clear_app_data(
            &installd,
            ps,
            user,
            FLAG_STORAGE_DE | FLAG_STORAGE_CE | FLAG_STORAGE_EXTERNAL,
        );
        self.clear_keystore_data(user, ps.app_id);
        // The CE storage is prepared as it is unlocked; user 0 runs.
        let flags = if self.user_unlocking_or_unlocked(user)? {
            FLAG_STORAGE_DE | FLAG_STORAGE_CE
        } else {
            FLAG_STORAGE_DE
        };
        self.prepare_app_data_contents(&installd, ps, user, flags)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        Ok(true)
    }

    /// `AppDataHelper.clearKeystoreData`: the app's keystore namespace
    /// cleared (`AndroidKeyStoreMaintenance.clearNamespace`), a failure
    /// logged.
    fn clear_keystore_data(&self, user: i32, app_id: i32) {
        if app_id < 0 {
            return;
        }
        let mut data = Parcel::new();
        maintenance::ClearNamespace {
            domain: domain::APP as i32,
            nspace: i64::from(user * PER_USER_RANGE as i32 + app_id),
        }
        .write(&mut data);
        let failure = match super::display::transact_service(
            &self.process,
            "android.security.maintenance",
            maintenance::CLEAR_NAMESPACE,
            &data,
        ) {
            Ok(reply) => match maintenance::read_clear_namespace_reply(&mut reply.reader()) {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.message),
                Err(s) => Some(s.to_string()),
            },
            Err(e) => Some(e),
        };
        if let Some(e) = failure {
            eprintln!("app host: keystore: clearNamespace failed: {e}");
        }
    }

    /// Once the data of `ps`, which may suspend apps, is cleared: the
    /// suspensions it made in `user` and every distraction restriction
    /// lifted (`unsuspendForSuspendingPackage`,
    /// `removeAllDistractingPackageRestrictions`). Lifting them is not
    /// implemented (#1987); with none, nothing changes.
    fn unsuspend_for(&self, ps: &PackageState, user: i32) -> Result<(), Exception> {
        let uid = user * PER_USER_RANGE as i32 + ps.app_id;
        if self.check_permission(Some(SUSPEND_APPS), uid)? != 0 {
            return Ok(());
        }
        let (state, _) = self.packages()?;
        let restricted = state.packages.values().any(|other| {
            let u = user_state(other, user);
            u.distraction_flags != 0 || u.suspensions.iter().flatten().any(|s| s.package == ps.name)
        });
        if restricted {
            return Err(self.unimplemented_named(
                "package",
                &format!("lifting the suspensions of {} (#1987)", ps.name),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hasInstantApplicationMetadata` and `deleteInstantApplicationMetadata`
    /// over `InstantAppRegistry`'s files.
    #[test]
    fn instant_metadata_is_found_and_deleted() {
        let data = std::env::temp_dir().join(format!("clear-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let dir = instant_app_dir(&data, "org.example.instant", 0);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!has_instant_metadata(&data, "org.example.instant", 0));
        std::fs::write(dir.join("cookie_1a2b"), b"").unwrap();
        assert!(has_instant_metadata(&data, "org.example.instant", 0));
        std::fs::write(dir.join("metadata.xml"), b"").unwrap();
        std::fs::write(dir.join("other"), b"").unwrap();
        delete_instant_metadata(&data, "org.example.instant", 0);
        assert!(!has_instant_metadata(&data, "org.example.instant", 0));
        assert!(dir.join("other").exists());
        std::fs::remove_dir_all(&data).unwrap();
    }
}
