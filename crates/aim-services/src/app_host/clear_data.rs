//! A package's data cleared in the app host (`android-16.0.0_r1`):
//! ActivityManagerService's `clearApplicationUserData`, with its checks and
//! the package's force stop, around PackageManagerService's on its handler
//! (`clearApplicationUserDataLIF`: the runtime permissions reset, installd's
//! `clearAppData`, the keystore namespace, the native library link; the
//! instant-app metadata; the components' states of a package that asks),
//! then `ACTION_PACKAGE_RESTARTED` and `ACTION_PACKAGE_DATA_CLEARED`, and
//! the package's URI grants, notification state, jobs and alarms. The
//! native application-data owner freezes the package while its data goes;
//! what a force stop does to its activities, services, providers and
//! recent tasks is #1570.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

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
use crate::package::model::PackageState;
use crate::package::query::Query;

const CLEAR_APP_USER_DATA: &str = "android.permission.CLEAR_APP_USER_DATA";
const MANAGE_USERS: &str = "android.permission.MANAGE_USERS";
const ACCESS_INSTANT_APPS: &str = "android.permission.ACCESS_INSTANT_APPS";
/// `StorageManager.FLAG_STORAGE_EXTERNAL`.
const FLAG_STORAGE_EXTERNAL: i32 = 4;
/// `ApplicationInfo.PRIVATE_FLAG_INSTANT`.
const PRIVATE_FLAG_INSTANT: i32 = 1 << 7;
/// `Process.INVALID_UID`, `SYSTEM_UID`.
const INVALID_UID: i32 = -1;
const SYSTEM_UID: i32 = 1000;
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
        let (state, resolution) = self.live_packages()?;
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
        let instant_metadata = state
            .system
            .instant_registry
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "instant metadata owner unavailable"))?
            .has_metadata(Some(package), user)
            .map_err(|e| self.not_modelled(e))?;
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
        let system = self.package_system()?;
        let bridge = system.package_bootstrap()?;
        let effects = system.package_effects_owner(&bridge)?;
        system.package_application_data()?.clear_with_callback(
            Some(package.into()),
            user,
            0,
            SYSTEM_UID,
            completed,
            &effects,
        )?;
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

    /// `clearApplicationUserDataLIF`: whether the package's data was
    /// cleared.
    pub(super) fn clear_app_data_lif(
        &self,
        ps: &PackageState,
        user: i32,
    ) -> Result<bool, Exception> {
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
        )
        .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        self.clear_keystore_data(user, ps.app_id);
        // The CE storage is prepared as it is unlocked; user 0 runs.
        let flags = if self.ce_storage_prepared(user) {
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
}
