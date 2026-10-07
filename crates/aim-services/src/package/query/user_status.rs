//! Pinned ComputerEngine user-state queries and IPackageManager exception mapping.
use super::*;
use aim_service_aidl::WriteParcelable;
struct Warning(String);
impl WriteParcelable for Warning {
    fn write_to(&self, p: &mut Parcel) {
        crate::clip::write_char_sequence(p, Some(&self.0));
    }
}
impl Query<'_> {
    pub(super) fn user_status(&self, code: u32, r: &mut Reader<'_>) -> Answered {
        match code {
            pm::IS_PACKAGE_SUSPENDED_FOR_USER
            | pm::IS_PACKAGE_QUARANTINED_FOR_USER
            | pm::IS_PACKAGE_STOPPED_FOR_USER => {
                let (name, user) = match code {
                    pm::IS_PACKAGE_SUSPENDED_FOR_USER => {
                        let a = args(pm::IsPackageSuspendedForUser::read(r))?;
                        (a.package_name, a.user_id)
                    }
                    pm::IS_PACKAGE_QUARANTINED_FOR_USER => {
                        let a = args(pm::IsPackageQuarantinedForUser::read(r))?;
                        (a.package_name, a.user_id)
                    }
                    _ => {
                        let a = args(pm::IsPackageStoppedForUser::read(r))?;
                        (a.package_name, a.user_id)
                    }
                };
                let value = (|| {
                    if let Err(error) = self.full_cross_user(user, false)? {
                        return Ok(Err(error));
                    }
                    let package = self.state.packages.get(name.as_deref().unwrap_or_default());
                    if package.is_none() || self.filtered_including_uninstalled(package, user)? {
                        return Ok(Err(Exception::illegal_argument(format!(
                            "Unknown target package: {}",
                            name.as_deref().unwrap_or("null")
                        ))));
                    }
                    let state = user_state(package.unwrap(), user);
                    Ok(Ok(match code {
                        pm::IS_PACKAGE_SUSPENDED_FOR_USER => !state.suspended_by.is_empty(),
                        pm::IS_PACKAGE_QUARANTINED_FOR_USER => state.quarantined,
                        _ => state.stopped,
                    }))
                })();
                thrown(value, |p, v| match code {
                    pm::IS_PACKAGE_SUSPENDED_FOR_USER => {
                        pm::write_is_package_suspended_for_user_reply(p, v)
                    }
                    pm::IS_PACKAGE_QUARANTINED_FOR_USER => {
                        pm::write_is_package_quarantined_for_user_reply(p, v)
                    }
                    _ => pm::write_is_package_stopped_for_user_reply(p, v),
                })
            }
            pm::GET_HARMFUL_APP_WARNING => {
                let a = args(pm::GetHarmfulAppWarning::read(r))?;
                let value = (|| {
                    if let Err(error) = self.full_cross_user(a.user_id, true)? {
                        return Ok(Err(error));
                    }
                    if !matches!(app_id(self.calling_uid), ROOT_UID | SYSTEM_UID)
                        && !self.uid_has_permission(
                            self.calling_uid,
                            "android.permission.SET_HARMFUL_APP_WARNINGS",
                        )?
                    {
                        return Ok(Err(Exception::security(
                            "Caller must have the android.permission.SET_HARMFUL_APP_WARNINGS permission.",
                        )));
                    }
                    let Some(package) = self
                        .state
                        .packages
                        .get(a.package_name.as_deref().unwrap_or_default())
                    else {
                        return Ok(Err(Exception::illegal_argument(format!(
                            "Unknown package: {}",
                            a.package_name.as_deref().unwrap_or("null")
                        ))));
                    };
                    Ok(Ok(user_state(package, a.user_id)
                        .harmful_app_warning
                        .map(Warning)))
                })();
                thrown(value, |p, v| {
                    pm::write_get_harmful_app_warning_reply(p, v.as_ref())
                })
            }
            _ => Err(NotModelled("a method not modelled")),
        }
    }
    pub(crate) fn full_cross_user(&self, user: i32, check_shell: bool) -> Thrown<()> {
        if user < 0 {
            return Ok(Err(Exception::illegal_argument(format!(
                "Invalid userId {user}"
            ))));
        }
        if check_shell && self.calling_uid == apps_filter::SHELL_UID {
            return Err(NotModelled("UserManager shell-debugging restriction"));
        }
        if user == user_id(self.calling_uid) || matches!(self.calling_uid, ROOT_UID | SYSTEM_UID) {
            return Ok(Ok(()));
        }
        if self.uid_has_permission(self.calling_uid, INTERACT_ACROSS_USERS_FULL)? {
            return Ok(Ok(()));
        }
        if apps_filter::is_isolated(self.calling_uid) {
            return Err(NotModelled("isolated compute cross-user owner"));
        }
        Ok(Err(Exception::security(format!(
            "UID {} requires {INTERACT_ACROSS_USERS_FULL} to access user {user}.",
            self.calling_uid
        ))))
    }
}
