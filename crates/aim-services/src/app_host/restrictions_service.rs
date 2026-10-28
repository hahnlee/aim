//! `restrictions`: RestrictionsManagerService for the app host
//! (`android-16.0.0_r1`), on a device with no device or profile owner
//! (`device_policy`): no restrictions provider, so no permission request
//! or local approval, and no admin's restrictions; an app's own
//! restrictions as UserManager keeps them. A device with admins or
//! policies is not implemented (#2324).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_content_irestrictionsmanager as rm;

use super::{AppHost, PER_USER_RANGE, reply};

const NO_PROVIDER: &str = "Cannot request permission without a restrictions provider registered";

pub struct RestrictionsManager {
    host: Arc<AppHost>,
}

impl RestrictionsManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        RestrictionsManager { host }
    }

    /// `IDevicePolicyManager.getRestrictionsProvider` is the profile or
    /// device owner's, and with no admin there is none.
    fn no_provider(&self) -> Result<(), Exception> {
        if self.host.device_policies()?.has_admins_or_policies() {
            return Err(self
                .host
                .unimplemented_named("restrictions", "a device with admins or policies (#2324)"));
        }
        Ok(())
    }
}

impl Service for RestrictionsManager {
    fn descriptor(&self) -> &str {
        rm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let user = uid / PER_USER_RANGE as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            rm::GET_APPLICATION_RESTRICTIONS => {
                let a =
                    rm::GetApplicationRestrictions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.package_name.unwrap_or_default();
                super::user_service::application_restrictions(&self.host, uid, &package, user)
                    .map(|r| rm::write_get_application_restrictions_reply(&mut p, Some(&r)))
            }
            rm::GET_APPLICATION_RESTRICTIONS_PER_ADMIN_FOR_USER => {
                rm::GetApplicationRestrictionsPerAdminForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.no_provider().map(|()| {
                    rm::write_get_application_restrictions_per_admin_for_user_reply::<
                        super::bind::Bundle,
                    >(&mut p, Some(&[]))
                })
            }
            rm::HAS_RESTRICTIONS_PROVIDER => {
                rm::HasRestrictionsProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.no_provider()
                    .map(|()| rm::write_has_restrictions_provider_reply(&mut p, false))
            }
            rm::REQUEST_PERMISSION | rm::CREATE_LOCAL_APPROVAL_INTENT => {
                if call.code == rm::REQUEST_PERMISSION {
                    rm::RequestPermission::<super::kept::RawBundle>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                } else {
                    rm::CreateLocalApprovalIntent::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                }
                self.no_provider()
                    .and_then(|()| Err(Exception::new(EX_ILLEGAL_STATE, NO_PROVIDER)))
            }
            rm::NOTIFY_PERMISSION_RESPONSE => {
                rm::NotifyPermissionResponse::<super::kept::RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.no_provider().and_then(|()| {
                    Err(Exception::security(
                        "No restrictions provider registered for user",
                    ))
                })
            }
            code => {
                let name = rm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("restrictions", &format!("{name} (#2324)")))
            }
        };
        reply(result.map(|()| p))
    }
}
