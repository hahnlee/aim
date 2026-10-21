//! `crossprofileapps`: CrossProfileAppsServiceImpl for the app host
//! (`android-16.0.0_r1`): the profiles an app can reach from its user,
//! which are none, as user 0 has no profiles. The starts across profiles
//! and the app-op configuration are not implemented (#1640).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_pm_icrossprofileapps as cpa;

use super::{AppHost, PER_USER_RANGE, reply, user_service};

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
