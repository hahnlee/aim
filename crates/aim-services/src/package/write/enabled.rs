//! `setComponentEnabledSetting` and `setApplicationEnabledSetting`: the
//! checks and the state change of `PackageManagerService.setEnabledSettings`
//! and `setEnabledSettingInternalLocked` at `android-16.0.0_r1`, for the
//! one setting each call carries.

use aim_binder_host::parcel::{EX_SECURITY, Exception, Parcel, Reader};
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

const CHANGE_COMPONENT_ENABLED_STATE: &str = "android.permission.CHANGE_COMPONENT_ENABLED_STATE";
const SUSPEND_APPS: &str = "android.permission.SUSPEND_APPS";
/// `PackageManager.APP_DETAILS_ACTIVITY_CLASS_NAME`.
const APP_DETAILS_ACTIVITY: &str = "android.app.AppDetailsActivity";
/// `Build.VERSION_CODES.JELLY_BEAN`.
const JELLY_BEAN: i32 = 16;

/// One `ComponentEnabledSetting`, with the call's user and package.
#[derive(Debug, PartialEq)]
pub struct Setting {
    pub package: String,
    /// The component's class; `None` for the package.
    pub class: Option<String>,
    pub new_state: i32,
    /// `PackageManager.DONT_KILL_APP` and `SYNCHRONOUS`, passed unchanged
    /// to the service's side effects and persistence policy.
    pub flags: i32,
    pub user: i32,
    /// `callingPackage`, the calling uid when the caller gave none.
    pub calling_package: String,
}

impl Setting {
    /// The setting a call carries; `None` for another call, or one with
    /// no component (a `NullPointerException` in the original).
    pub fn read(code: u32, uid: u32, data: &mut Reader<'_>) -> Option<Setting> {
        let (package, class, new_state, flags, user, calling) = match code {
            pm::SET_COMPONENT_ENABLED_SETTING => {
                let a = pm::SetComponentEnabledSetting::<ComponentName>::read(data).ok()?;
                let c = a.component_name?;
                (
                    c.package,
                    Some(c.class),
                    a.new_state,
                    a.flags,
                    a.user_id,
                    a.calling_package,
                )
            }
            pm::SET_APPLICATION_ENABLED_SETTING => {
                let a = pm::SetApplicationEnabledSetting::read(data).ok()?;
                (
                    a.package_name?,
                    None,
                    a.new_state,
                    a.flags,
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
            flags,
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
pub fn reply(code: u32) -> Parcel {
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
#[derive(Clone, Copy, Debug, Default)]
pub struct Policy {
    pub protected: Option<bool>,
    pub shell_restricted: Option<bool>,
    /// Result of the real compressed-package enable operation, when required.
    pub compressed_enabled: Option<bool>,
    pub suspension_cleanup: bool,
}

pub fn decide(
    q: &Query<'_>,
    s: &Setting,
    calling_pid: i32,
    current: &dyn Fn(&PackageState, i32) -> Enabled,
) -> Result<Result<Option<Enabled>, Exception>, NotModelled> {
    decide_with_policy(q, s, calling_pid, current, None)
}
pub fn decide_with_policy(q: &Query<'_>, s: &Setting, calling_pid: i32,
    current: &dyn Fn(&PackageState, i32) -> Enabled, policy: Option<Policy>)
    -> Result<Result<Option<Enabled>, Exception>, NotModelled> {
    if !q.state.users.contains_key(&s.user) {
        return Ok(Ok(None));
    }
    let uid = q.calling_uid;
    if uid == SHELL_UID {
        match policy.and_then(|policy| policy.shell_restricted) {
            None => return Err(NotModelled("the shell's user restrictions and state rules")),
            Some(true) => return Ok(Err(Exception::security("Shell does not have permission to access this user"))),
            Some(false) => {}
        }
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
        match policy.and_then(|policy| policy.protected) {
            None => return Err(NotModelled("ProtectedPackages: the provisioning package and DevicePolicy's owners")),
            Some(true) => return Ok(Err(Exception::security("Cannot change the package state of a protected package"))),
            Some(false) => {}
        }
    }
    let pkg = match (&ps.pkg, &ps.parcel) {
        (Some(pkg), _) => Some(pkg.as_ref()),
        (None, Some(_)) => return Err(NotModelled("a parsed package that does not read")),
        (None, None) => None,
    };
    let mut after = current(ps, s.user);
    if uid == SHELL_UID && (s.class.is_some()
        || !matches!(after.enabled, COMPONENT_ENABLED_STATE_DEFAULT | COMPONENT_ENABLED_STATE_ENABLED | COMPONENT_ENABLED_STATE_DISABLED_USER)
        || !matches!(s.new_state, COMPONENT_ENABLED_STATE_DEFAULT | COMPONENT_ENABLED_STATE_ENABLED | COMPONENT_ENABLED_STATE_DISABLED_USER)) {
        return Ok(Err(Exception::security("Shell cannot change this component state")));
    }
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
                match policy.and_then(|policy| policy.compressed_enabled) {
                    Some(true) => {}
                    Some(false) => return Ok(Ok(Some(after))),
                    None => return Err(NotModelled("enabling a system stub, which installs it")),
                }
            }
            if matches!(
                s.new_state,
                COMPONENT_ENABLED_STATE_DISABLED_USER | COMPONENT_ENABLED_STATE_DISABLED
            ) && ps
                .users
                .get(&s.user)
                .is_some_and(|u| u.granted_permissions.iter().any(|p| p == SUSPEND_APPS))
            {
                if !policy.is_some_and(|policy| policy.suspension_cleanup) {
                    return Err(NotModelled("disabling a suspending app, which unsuspends what it suspended"));
                }
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

#[derive(Debug)]
pub struct Argument { pub package: Option<String>, pub component: Option<ComponentName>, pub state: i32, pub flags: i32 }
impl aim_service_aidl::ReadParcelable for Argument {
    fn read_from(reader: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let flags = reader.read_i32()?;
        let package = if flags & 1 != 0 { reader.read_string16()? } else { None };
        let component = if flags & 2 != 0 { aim_service_aidl::read_typed(reader)? } else { None };
        Ok(Self { package, component, state: reader.read_i32()?, flags: reader.read_i32()? })
    }
}
#[derive(Debug)]
pub struct Batch {
    pub settings: Option<Vec<Option<Argument>>>, pub user: i32, pub caller: String,
}
impl Batch {
    pub fn read(uid: i32, reader: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let args = pm::SetComponentEnabledSettings::<Argument>::read(reader)?;
        if reader.remaining() != 0 { return Err(aim_binder_host::parcel::BAD_VALUE); }
        Ok(Self { settings: args.settings, user: args.user_id, caller: args.calling_package.unwrap_or_else(|| uid.to_string()) })
    }
    pub fn settings(&self, query: &Query<'_>) -> Result<Vec<Setting>, Exception> {
        if !query.state.users.contains_key(&self.user) { return Ok(Vec::new()); }
        let values = self.settings.as_ref().filter(|values| !values.is_empty()).ok_or_else(|| Exception::illegal_argument("The list of enabled settings is empty"))?;
        let mut result = Vec::new();
        let mut packages = std::collections::BTreeSet::new();
        let mut components = std::collections::BTreeSet::new();
        let mut component_flags = std::collections::BTreeMap::new();
        for value in values {
            let value = value.as_ref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null enabled setting"))?;
            if !(0..=4).contains(&value.state) { return Err(Exception::illegal_argument("Invalid new component state")); }
            let (package, class) = if let Some(component) = &value.component { (component.package.clone(), Some(component.class.clone())) }
                else { (value.package.clone().ok_or_else(|| Exception::illegal_argument("Unknown package: null"))?, None) };
            if let Some(class) = &class {
                if !components.insert((package.clone(), class.clone())) { return Err(Exception::illegal_argument("The component is duplicated")); }
                if component_flags.get(&package).is_some_and(|flags| *flags != (value.flags & 1)) { return Err(Exception::illegal_argument("A conflict of the DONT_KILL_APP flag between components")); }
                component_flags.insert(package.clone(), value.flags & 1);
            } else if !packages.insert(package.clone()) { return Err(Exception::illegal_argument("The package is duplicated")); }
            result.push(Setting { package, class, new_state: value.state, flags: value.flags, user: self.user, calling_package: self.caller.clone() });
        }
        Ok(result)
    }
    /// Every entry is validated before the returned plans publish anything.
    /// Policies come from the retained original owners; compressed enable runs
    /// through the native install pipeline before supplying its actual result.
    pub fn prepare(&self, query: &Query<'_>, pid: i32, settings: &[Setting], policies: &[Policy])
        -> Result<Vec<super::mutation::Plan>, Exception> {
        if settings.len() != policies.len() { return Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "enabled policy inventory differs")); }
        let mut current = std::collections::BTreeMap::<String, Enabled>::new();
        for (setting, policy) in settings.iter().zip(policies) {
            let after = decide_with_policy(query, setting, pid, &|package, user| current.get(&package.name).cloned().unwrap_or_else(|| Enabled::of(package, user)), Some(*policy))
                .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))??;
            if let Some(after) = after { current.insert(setting.package.clone(), after); }
        }
        Ok(current.into_iter().filter_map(|(package, enabled)| {
            let before = query.state.packages.get(&package).map(|package| Enabled::of(package, self.user));
            (before.as_ref() != Some(&enabled)).then(|| super::mutation::Plan { package, user: Some(self.user), change: super::mutation::Change::Enabled(enabled) })
        }).collect())
    }
}
