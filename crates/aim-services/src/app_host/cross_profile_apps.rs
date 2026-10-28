//! `crossprofileapps`: CrossProfileAppsServiceImpl for the app host
//! (`android-16.0.0_r1`): the profiles an app can reach from its user,
//! which are none, as user 0 has no profiles. The starts across profiles
//! are not implemented (#1640). Configuration eligibility and OTA resets
//! follow the original's single-user paths and access-state owner.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_pm_icrossprofileapps as cpa;

use super::appops_service::{self, OP_INTERACT_ACROSS_PROFILES};
use super::{AppHost, PER_USER_RANGE, reply, user_service};

const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const CONFIGURE_INTERACT_ACROSS_PROFILES: &str =
    "android.permission.CONFIGURE_INTERACT_ACROSS_PROFILES";
const MANAGE_APP_OPS_MODES: &str = "android.permission.MANAGE_APP_OPS_MODES";
const INTERACT_ACROSS_PROFILES: &str = "android.permission.INTERACT_ACROSS_PROFILES";

/// `UserHandle`: its user id.
struct UserHandle(i32);

impl WriteParcelable for UserHandle {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0);
    }
}

pub struct CrossProfileApps {
    host: Arc<AppHost>,
}

impl CrossProfileApps {
    pub fn new(host: Arc<AppHost>) -> Self {
        CrossProfileApps { host }
    }

    /// `verifyCallingPackage`: `AppOpsManager.checkPackage`.
    fn verify_calling_package(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        let package = package.ok_or_else(|| Exception::new(EX_NULL_POINTER, "callingPackage"))?;
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        Ok(())
    }

    /// `getTargetUserProfilesUnchecked`: the user's enabled profiles but
    /// itself (in which the package is enabled), none for user 0.
    fn target_profiles(user: i32) -> Vec<UserHandle> {
        user_service::profile_ids(user)
            .into_iter()
            .filter(|p| *p != user)
            .map(UserHandle)
            .collect()
    }

    fn permitted(&self, uid: i32, permission: &str) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    fn can_configure(&self, user: i32) -> Result<bool, Exception> {
        // canUserAttemptToConfigureInteractAcrossProfiles returns before
        // inspecting the package or any allowlist when there are < 2 profiles.
        if user_service::profile_ids(user).len() < 2 {
            return Ok(false);
        }
        Err(self
            .host
            .unimplemented_named("crossprofileapps", "configuration with profiles (#1640)"))
    }

    fn has_permission(&self, uid: i32, package: &str) -> Result<bool, Exception> {
        Ok(self.permitted(uid, INTERACT_ACROSS_USERS_FULL)?
            || self.permitted(uid, INTERACT_ACROSS_USERS)?
            || super::permission_checker::preflight(
                &self.host,
                INTERACT_ACROSS_PROFILES,
                uid,
                package,
            )?)
    }

    fn reset(&self, caller: i32, user: i32, package: Option<&str>) -> Result<(), Exception> {
        if self.can_configure(user)? {
            return Ok(());
        }
        if !self.permitted(caller, INTERACT_ACROSS_USERS_FULL)?
            && !self.permitted(caller, INTERACT_ACROSS_USERS)?
        {
            return Err(Exception::security(
                "INTERACT_ACROSS_USERS or INTERACT_ACROSS_USERS_FULL is required to set the app-op for interacting across profiles.",
            ));
        }
        if !self.permitted(caller, MANAGE_APP_OPS_MODES)?
            && !self.permitted(caller, CONFIGURE_INTERACT_ACROSS_PROFILES)?
        {
            return Err(Exception::security(
                "MANAGE_APP_OPS_MODES or CONFIGURE_INTERACT_ACROSS_PROFILES is required to set the app-op for interacting across profiles.",
            ));
        }
        for profile in user_service::profile_ids(user) {
            let Some(package) = package else { continue };
            let Some(info) = self.host.application_info(package, 0, None)? else {
                continue;
            };
            let mode =
                appops_service::policy_default_mode(&self.host, OP_INTERACT_ACROSS_PROFILES)?;
            if appops_service::mode(
                &self.host,
                OP_INTERACT_ACROSS_PROFILES,
                info.uid,
                Some(package),
            )? == mode
            {
                continue;
            }
            let had_permission = self.has_permission(info.uid, package)?;
            self.host
                .permission_manager
                .get()
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "no permissionmgr",
                    )
                })?
                .set_cross_profile_mode(info.uid, None, mode)?;
            if had_permission && !self.has_permission(info.uid, package)? {
                self.host
                    .kill_uid_for_permission_change(info.uid as u32, "permissions revoked");
            }
            let (_, resolution) = self.host.packages()?;
            let cross_profile = self
                .host
                .packages()?
                .0
                .packages
                .get(package)
                .and_then(|ps| ps.pkg.as_ref())
                .is_some_and(|pkg| pkg.is(crate::package::pkg::booleans::CROSS_PROFILE));
            let flags = if cross_profile {
                0x0100_0000 | 0x1000_0000
            } else {
                0x4000_0000
            };
            let intent = super::kept::RawIntent::with_data(
                Some("android.content.pm.action.CAN_INTERACT_ACROSS_PROFILES_CHANGED"),
                None,
                Some(package),
                flags,
                &[],
            );
            let receivers = resolution
                .query_intent_receivers(&intent.intent, None, 0, profile, 1000)
                .map_err(|e| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("queryIntentReceivers: {e:?}"),
                    )
                })?;
            for receiver in receivers {
                if let crate::package::component_resolver::Info::Activity(info) = receiver.info {
                    let (Some(package), Some(name)) =
                        (&info.info.item.package_name, &info.info.item.name)
                    else {
                        return Err(Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "receiver component absent",
                        ));
                    };
                    self.host.broadcasts.send_from_system_as_user(
                        &self.host,
                        intent.with_component(package, name),
                        profile,
                    );
                }
            }
        }
        Ok(())
    }
}

impl Service for CrossProfileApps {
    fn descriptor(&self) -> &str {
        cpa::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let user = (call.sender_euid / PER_USER_RANGE) as i32;
        let mut p = Parcel::new();
        match call.code {
            cpa::CAN_CONFIGURE_INTERACT_ACROSS_PROFILES => {
                let a = cpa::CanConfigureInteractAcrossProfiles::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                return reply((|| {
                    // The original calls checkCallingOrSelfPermission here,
                    // not enforce: its returned permission result is unused.
                    if user != a.user_id {
                        self.permitted(uid, INTERACT_ACROSS_USERS)?;
                    }
                    cpa::write_can_configure_interact_across_profiles_reply(
                        &mut p,
                        self.can_configure(a.user_id)?,
                    );
                    Ok(p)
                })());
            }
            cpa::RESET_INTERACT_ACROSS_PROFILES_APP_OPS => {
                let a = cpa::ResetInteractAcrossProfilesAppOps::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                return reply((|| {
                    let packages = a
                        .package_names
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageNames"))?;
                    for package in packages {
                        self.reset(uid, a.user_id, package.as_deref())?;
                    }
                    cpa::write_reset_interact_across_profiles_app_ops_reply(&mut p);
                    Ok(p)
                })());
            }
            cpa::GET_TARGET_USER_PROFILES => {
                let a = cpa::GetTargetUserProfiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self
                    .verify_calling_package(uid, a.calling_package.as_deref())
                    .map(|_| Self::target_profiles(user))
                {
                    Ok(targets) => {
                        let targets: Vec<_> = targets.into_iter().map(Some).collect();
                        cpa::write_get_target_user_profiles_reply(&mut p, Some(&targets));
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            // No target profile: false before any permission is asked.
            cpa::CAN_INTERACT_ACROSS_PROFILES => {
                let a =
                    cpa::CanInteractAcrossProfiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self
                    .verify_calling_package(uid, a.calling_package.as_deref())
                    .map(|_| Self::target_profiles(user))
                {
                    Ok(targets) if targets.is_empty() => {
                        cpa::write_can_interact_across_profiles_reply(&mut p, false)
                    }
                    Ok(_) => {
                        return reply(Err(self.host.unimplemented(
                            "crossprofileapps",
                            self.descriptor(),
                            call.code,
                        )));
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                return reply(Err(self.host.unimplemented(
                    "crossprofileapps",
                    self.descriptor(),
                    code,
                )));
            }
        }
        Ok(p)
    }
}
