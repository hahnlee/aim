//! AppHibernationService reads (`android-16.0.0_r1`) for the app host,
//! which performs no hibernation transitions (#2064). Fresh packages are
//! not hibernating. Persisted states and mutations require that owner;
//! never discard them or report a successful hibernation without its effects.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::{io, sync::Arc};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::android_apphibernation_iapphibernationservice as ah;

use super::{AppHost, activity_user::AllowMode, reply};

const MANAGE: &str = "android.permission.MANAGE_APP_HIBERNATION";

pub struct AppHibernation(pub Arc<AppHost>);

impl AppHibernation {
    fn enforce(&self, uid: i32, user: i32, method: &str) -> Result<(), Exception> {
        let host = &self.0;
        // isDeviceConfigAppHibernationEnabled: disabled reads return before permissions.
        if host
            .platform()?
            .settings
            .device_config("app_hibernation", "app_hibernation_enabled")
            .is_some_and(|v| !v.eq_ignore_ascii_case("true"))
        {
            return Ok(());
        }
        if host.check_permission(Some(MANAGE), uid)? != 0 {
            let message = if method == "isHibernatingForUser" {
                format!("Caller did not have permission while calling {method}")
            } else {
                "Caller does not have MANAGE_APP_HIBERNATION permission.".into()
            };
            return Err(Exception::security(format!(
                "{message}: Neither user {uid} nor current process has {MANAGE}."
            )));
        }
        let user =
            host.handle_incoming_user(uid, user, false, AllowMode::FullOnly, Some(method), None)?;
        // InjectorImpl's disk stores. With no persisted records, all installed
        // packages have the original UserLevelState's initial hibernated=false.
        for path in [
            "system/hibernation/states".into(),
            format!("system_ce/{user}/hibernation/states"),
        ]
        .into_iter()
        .flat_map(|p| [p.clone(), format!("{p}.bak")])
        {
            match std::fs::read(host.data()?.join(path)) {
                Ok(bytes) if !bytes.is_empty() => {
                    return Err(
                        host.unimplemented_named("app_hibernation", "persisted states (#2064)")
                    );
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("app_hibernation: {e}"),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Service for AppHibernation {
    fn descriptor(&self) -> &str {
        ah::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = match call.code {
            ah::IS_HIBERNATING_FOR_USER => {
                let a = ah::IsHibernatingForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(call.sender_euid as i32, a.user_id, "isHibernatingForUser")
                    .map(|()| ah::write_is_hibernating_for_user_reply(&mut p, false))
            }
            ah::GET_HIBERNATING_PACKAGES_FOR_USER => {
                let a = ah::GetHibernatingPackagesForUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.enforce(
                    call.sender_euid as i32,
                    a.user_id,
                    "getHibernatingPackagesForUser",
                )
                .map(|()| ah::write_get_hibernating_packages_for_user_reply(&mut p, &Some(vec![])))
            }
            _ => Err(self
                .0
                .unimplemented_named("app_hibernation", "hibernation operations (#2064)")),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests;
