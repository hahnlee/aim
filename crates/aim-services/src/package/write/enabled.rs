//! `setComponentEnabledSetting` and `setApplicationEnabledSetting`: the
//! checks and the state change of `PackageManagerService.setEnabledSettings`
//! and `setEnabledSettingInternalLocked` at `android-16.0.0_r1`, for the
//! one setting each call carries.

use aim_binder_host::parcel::{EX_SECURITY, Exception, Parcel};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::Enabled;
use crate::package::apps_filter::{
    NotModelled, ROOT_UID, SHELL_UID, SYSTEM_UID, app_id, is_isolated,
};
use crate::package::info::{
    COMPONENT_ENABLED_STATE_DEFAULT, COMPONENT_ENABLED_STATE_DISABLED,
    COMPONENT_ENABLED_STATE_DISABLED_UNTIL_USED, COMPONENT_ENABLED_STATE_DISABLED_USER,
    COMPONENT_ENABLED_STATE_ENABLED,
};
use crate::package::intent::ComponentName;
use crate::package::model::PackageState;
use crate::package::pkg::{AndroidPackage, booleans2};
use crate::package::query::Query;
use crate::shadow::ShadowCall;

const CHANGE_COMPONENT_ENABLED_STATE: &str = "android.permission.CHANGE_COMPONENT_ENABLED_STATE";
const SUSPEND_APPS: &str = "android.permission.SUSPEND_APPS";
/// `PackageManager.APP_DETAILS_ACTIVITY_CLASS_NAME`.
const APP_DETAILS_ACTIVITY: &str = "android.app.AppDetailsActivity";
/// `Build.VERSION_CODES.JELLY_BEAN`.
const JELLY_BEAN: i32 = 16;

/// One `ComponentEnabledSetting`, with the call's user and package.
#[derive(Debug, PartialEq)]
pub(super) struct Setting {
    pub package: String,
    /// The component's class; `None` for the package.
    pub class: Option<String>,
    pub new_state: i32,
    pub user: i32,
    /// `callingPackage`, the calling uid when the caller gave none.
    pub calling_package: String,
}

impl Setting {
    /// The setting a call carries; `None` for another call, or one with
    /// no component (a `NullPointerException` in the original).
    pub fn read(call: &mut ShadowCall<'_>) -> Option<Setting> {
        let uid = call.sender_euid;
        let (package, class, new_state, user, calling) = match call.code {
            pm::SET_COMPONENT_ENABLED_SETTING => {
                let a =
                    pm::SetComponentEnabledSetting::<ComponentName>::read(&mut call.data).ok()?;
                let c = a.component_name?;
                (
                    c.package,
                    Some(c.class),
                    a.new_state,
                    a.user_id,
                    a.calling_package,
                )
            }
            pm::SET_APPLICATION_ENABLED_SETTING => {
                let a = pm::SetApplicationEnabledSetting::read(&mut call.data).ok()?;
                (
                    a.package_name?,
                    None,
                    a.new_state,
                    a.user_id,
                    a.calling_package,
                )
            }
            _ => return None,
        };
        Some(Setting {
            package,
            class,
            new_state,
            user,
            calling_package: calling.unwrap_or_else(|| uid.to_string()),
        })
    }

    /// `ComponentEnabledSetting`'s name in the original's messages.
    fn describe(&self) -> String {
        match &self.class {
            Some(class) => format!("component=ComponentInfo{{{}/{class}}}", self.package),
            None => format!("package={}", self.package),
        }
    }
}

/// The reply of a call that threw nothing.
pub(super) fn reply(code: u32) -> Parcel {
    let mut p = Parcel::new();
    match code {
        pm::SET_COMPONENT_ENABLED_SETTING => pm::write_set_component_enabled_setting_reply(&mut p),
        _ => pm::write_set_application_enabled_setting_reply(&mut p),
    }
    p
}

/// What `s` leaves of its package's state in its user, given the state
/// `current` holds now: `None` when the user does not exist, and the
/// exception the original throws.
pub(super) fn decide(
    q: &Query<'_>,
    s: &Setting,
    calling_pid: i32,
    current: &dyn Fn(&PackageState, i32) -> Enabled,
) -> Result<Result<Option<Enabled>, Exception>, NotModelled> {
    if !q.state.users.contains_key(&s.user) {
        return Ok(Ok(None));
    }
    let uid = q.calling_uid;
    if uid == SHELL_UID {
        return Err(NotModelled("the shell's user restrictions and state rules"));
    }
    if let Err(e) = q.enforce_cross_user(s.user, false, false, "set enabled")? {
        return Ok(Err(e));
    }
    if !(COMPONENT_ENABLED_STATE_DEFAULT..=COMPONENT_ENABLED_STATE_DISABLED_UNTIL_USED)
        .contains(&s.new_state)
    {
        return Ok(Err(Exception::illegal_argument(format!(
            "Invalid new component state: {}",
            s.new_state
        ))));
    }
    // checkCallingOrSelfPermission: root and system hold every
    // permission, isolated processes none.
    let allowed = match app_id(uid) {
        ROOT_UID | SYSTEM_UID => true,
        _ if is_isolated(uid) => false,
        _ => q.uid_has_permission(uid, CHANGE_COMPONENT_ENABLED_STATE)?,
    };
    let caller_is_target = q
        .packages_for_uid(uid)?
        .is_some_and(|names| names.iter().any(|n| n.as_deref() == Some(&s.package)));
    if !caller_is_target && !allowed {
        return Ok(Err(Exception::new(
            EX_SECURITY,
            format!(
                "Attempt to change component state; pid={calling_pid}, uid={uid}, {}",
                s.describe()
            ),
        )));
    }
    let ps = match q.state.packages.get(&s.package) {
        Some(ps) if !q.filtered_including_uninstalled(Some(ps), s.user)? => ps,
        _ => {
            let what = match &s.class {
                Some(class) => format!("component: ComponentInfo{{{}/{class}}}", s.package),
                None => format!("package: {}", s.package),
            };
            return Ok(Err(Exception::illegal_argument(format!("Unknown {what}"))));
        }
    };
    if !caller_is_target {
        return Err(NotModelled(
            "ProtectedPackages: the provisioning package and DevicePolicy's owners",
        ));
    }
    let pkg = match (&ps.pkg, &ps.parcel) {
        (Some(pkg), _) => Some(pkg.as_ref()),
        (None, Some(_)) => return Err(NotModelled("a parsed package that does not read")),
        (None, None) => None,
    };
    let mut after = current(ps, s.user);
    match &s.class {
        Some(class) => {
            if !allowed && class == APP_DETAILS_ACTIVITY {
                return Ok(Err(Exception::new(
                    EX_SECURITY,
                    "Cannot disable a system-generated component",
                )));
            }
            if !pkg.is_some_and(|p| has_component_class_name(p, class)) {
                if pkg.is_some_and(|p| p.target_sdk_version >= JELLY_BEAN) {
                    return Ok(Err(Exception::illegal_argument(format!(
                        "Component class {class} does not exist in {}",
                        s.package
                    ))));
                }
                return Ok(Ok(Some(after)));
            }
            // enableComponentLPw, disableComponentLPw, restoreComponentLPw;
            // the other states change nothing.
            match s.new_state {
                COMPONENT_ENABLED_STATE_ENABLED => {
                    after.disabled_components.remove(class);
                    after.enabled_components.insert(class.clone());
                }
                COMPONENT_ENABLED_STATE_DISABLED => {
                    after.enabled_components.remove(class);
                    after.disabled_components.insert(class.clone());
                }
                COMPONENT_ENABLED_STATE_DEFAULT => {
                    after.enabled_components.remove(class);
                    after.disabled_components.remove(class);
                }
                _ => {}
            }
        }
        None => {
            if after.enabled == s.new_state {
                return Ok(Ok(Some(after)));
            }
            let stub = pkg.is_some_and(|p| p.booleans2 & booleans2::STUB != 0) && ps.is.system;
            if stub
                && matches!(
                    s.new_state,
                    COMPONENT_ENABLED_STATE_DEFAULT | COMPONENT_ENABLED_STATE_ENABLED
                )
            {
                return Err(NotModelled("enabling a system stub, which installs it"));
            }
            if matches!(
                s.new_state,
                COMPONENT_ENABLED_STATE_DISABLED_USER | COMPONENT_ENABLED_STATE_DISABLED
            ) && ps
                .users
                .get(&s.user)
                .is_some_and(|u| u.granted_permissions.iter().any(|p| p == SUSPEND_APPS))
            {
                return Err(NotModelled(
                    "disabling a suspending app, which unsuspends what it suspended",
                ));
            }
            after.enabled = s.new_state;
            after.last_disable_app_caller = Some(s.calling_package.clone());
        }
    }
    Ok(Ok(Some(after)))
}

/// `AndroidPackageUtils.hasComponentClassName`.
fn has_component_class_name(pkg: &AndroidPackage, class: &str) -> bool {
    pkg.activities
        .iter()
        .chain(&pkg.receivers)
        .map(|a| &a.main.component.name)
        .chain(pkg.providers.iter().map(|p| &p.main.component.name))
        .chain(pkg.services.iter().map(|s| &s.main.component.name))
        .chain(pkg.instrumentations.iter().map(|i| &i.component.name))
        .chain(&pkg.backup_agent_name)
        .any(|n| n == class)
}
