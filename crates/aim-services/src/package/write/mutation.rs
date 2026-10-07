//! Native mutation decisions from android-16.0.0_r1 PackageManagerService.
//! Copyright AOSP, Apache License 2.0. Effects are explicit for the service owner.
use super::{Enabled, enabled};
use crate::package::{
    apps_filter::{self, NotModelled},
    info, model,
    query::Query,
};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Reader};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

#[derive(Debug)]
pub enum Request {
    Invalid(Exception),
    Enabled(enabled::Setting),
    Stopped {
        package: String,
        user: i32,
        stopped: bool,
    },
    SplashTheme {
        package: String,
        user: i32,
        theme: Option<String>,
    },
    CategoryHint {
        package: Option<String>,
        category: i32,
        caller: Option<String>,
    },
    HarmfulWarning {
        package: Option<String>,
        user: i32,
        warning: Option<String>,
    },
    MinAspectRatio {
        package: String,
        user: i32,
        ratio: i32,
    },
    MimeGroup {
        package: String,
        group: Option<String>,
        types: Vec<Option<String>>,
    },
    UpdateAvailable {
        package: String,
        available: bool,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    None,
    Enabled(Enabled),
    Stopped {
        stopped: bool,
        not_launched: bool,
        first_launch_installer: Option<String>,
        was_stopped: bool,
    },
    SplashTheme(Option<String>),
    HarmfulWarning(Option<String>),
    CategoryHint(i32),
    MinAspectRatio(i32),
    MimeGroup {
        group: Option<String>,
        types: Vec<String>,
    },
    UpdateAvailable(bool),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub package: String,
    pub user: Option<i32>,
    pub change: Change,
}
struct Warning(Option<String>);
impl aim_service_aidl::ReadParcelable for Warning {
    fn read_from(r: &mut Reader<'_>) -> Result<Self, i32> {
        crate::clip::char_sequence(r).map(Self)
    }
}
impl Request {
    pub fn read(code: u32, uid: u32, r: &mut Reader<'_>) -> Option<Result<Self, i32>> {
        let result = (|| {
            Ok(match code {
                pm::SET_COMPONENT_ENABLED_SETTING | pm::SET_APPLICATION_ENABLED_SETTING => {
                    Self::Enabled(enabled::Setting::read(code, uid, r).ok_or(BAD_VALUE)?)
                }
                pm::SET_PACKAGE_STOPPED_STATE => {
                    let a = pm::SetPackageStoppedState::read(r)?;
                    let Some(package) = a.package_name else {
                        return Ok(Self::Invalid(Exception::new(
                            EX_NULL_POINTER,
                            "null package name",
                        )));
                    };
                    Self::Stopped {
                        package,
                        user: a.user_id,
                        stopped: a.stopped,
                    }
                }
                pm::SET_SPLASH_SCREEN_THEME => {
                    let a = pm::SetSplashScreenTheme::read(r)?;
                    let Some(package) = a.package_name else {
                        return Ok(Self::Invalid(Exception::new(
                            EX_NULL_POINTER,
                            "null package name",
                        )));
                    };
                    Self::SplashTheme {
                        package,
                        user: a.user_id,
                        theme: a.theme_name,
                    }
                }
                pm::SET_APPLICATION_CATEGORY_HINT => {
                    let a = pm::SetApplicationCategoryHint::read(r)?;
                    Self::CategoryHint {
                        package: a.package_name,
                        category: a.category_hint,
                        caller: a.caller_package_name,
                    }
                }
                pm::SET_HARMFUL_APP_WARNING => {
                    let a = pm::SetHarmfulAppWarning::<Warning>::read(r)?;
                    Self::HarmfulWarning {
                        package: a.package_name,
                        user: a.user_id,
                        warning: a.warning.and_then(|value| value.0),
                    }
                }
                pm::SET_USER_MIN_ASPECT_RATIO => {
                    let a = pm::SetUserMinAspectRatio::read(r)?;
                    let Some(package) = a.package_name else {
                        return Ok(Self::Invalid(Exception::new(
                            EX_NULL_POINTER,
                            "null package name",
                        )));
                    };
                    Self::MinAspectRatio {
                        package,
                        user: a.user_id,
                        ratio: a.aspect_ratio,
                    }
                }
                pm::SET_MIME_GROUP => {
                    let a = pm::SetMimeGroup::read(r)?;
                    let Some(package) = a.package_name else {
                        return Ok(Self::Invalid(Exception::new(
                            EX_NULL_POINTER,
                            "null package name",
                        )));
                    };
                    Self::MimeGroup {
                        package,
                        group: a.group,
                        types: a.mime_types.unwrap_or_default(),
                    }
                }
                pm::SET_UPDATE_AVAILABLE => {
                    let a = pm::SetUpdateAvailable::read(r)?;
                    let Some(package) = a.package_name else {
                        return Ok(Self::Invalid(Exception::new(
                            EX_NULL_POINTER,
                            "null package name",
                        )));
                    };
                    Self::UpdateAvailable {
                        package,
                        available: a.update_avaialble,
                    }
                }
                _ => return Err(BAD_VALUE),
            })
        })();
        if !matches!(
            code,
            pm::SET_COMPONENT_ENABLED_SETTING
                | pm::SET_APPLICATION_ENABLED_SETTING
                | pm::SET_PACKAGE_STOPPED_STATE
                | pm::SET_SPLASH_SCREEN_THEME
                | pm::SET_HARMFUL_APP_WARNING
                | pm::SET_APPLICATION_CATEGORY_HINT
                | pm::SET_USER_MIN_ASPECT_RATIO
                | pm::SET_MIME_GROUP
                | pm::SET_UPDATE_AVAILABLE
        ) {
            return None;
        }
        Some(result.and_then(|request| {
            if r.remaining() == 0 {
                Ok(request)
            } else {
                Err(BAD_VALUE)
            }
        }))
    }
    pub fn decide(&self, q: &Query<'_>, pid: i32) -> Result<Result<Plan, Exception>, NotModelled> {
        if let Self::Invalid(error) = self {
            return Ok(Err(error.clone()));
        }
        if let Self::CategoryHint {
            package,
            category,
            caller,
        } = self
        {
            if apps_filter::instant_app_package_name(q.state, q.calling_uid)?.is_some() {
                return Ok(Err(Exception::security(
                    "Instant applications don't have access to this method",
                )));
            }
            let user = apps_filter::user_id(q.calling_uid);
            let caller_uid = match q.package_uid(caller.as_deref().unwrap_or_default(), 0, user)? {
                Ok(uid) => uid,
                Err(error) => return Ok(Err(error)),
            };
            if caller_uid != q.calling_uid {
                return Ok(Err(Exception::security(format!(
                    "Package {} does not belong to {}",
                    caller.as_deref().unwrap_or("null"),
                    q.calling_uid
                ))));
            }
            let target = package.as_deref().unwrap_or_default();
            let name = q.internal_resolve_name(target, -1)?;
            let state = q.state.packages.get(&name);
            if state.is_none_or(|state| !info::user_state(state, user).installed)
                || q.filtered_including_uninstalled(state, user)?
            {
                return Ok(Err(Exception::illegal_argument(format!(
                    "Unknown target package {}",
                    package.as_deref().unwrap_or("null")
                ))));
            }
            let state = state.unwrap();
            if caller != &state.install_source.installer {
                return Ok(Err(Exception::illegal_argument(format!(
                    "Calling package {} is not installer for {}",
                    caller.as_deref().unwrap_or("null"),
                    target
                ))));
            }
            return Ok(Ok(Plan {
                package: name,
                user: None,
                change: if state.category_override == *category {
                    Change::None
                } else {
                    Change::CategoryHint(*category)
                },
            }));
        }
        if let Self::HarmfulWarning {
            package,
            user,
            warning,
        } = self
        {
            if let Err(error) = q.full_cross_user(*user, true)? {
                return Ok(Err(error));
            }
            if !permission(q, "android.permission.SET_HARMFUL_APP_WARNINGS")? {
                return Ok(Err(Exception::security(
                    "Caller must have the android.permission.SET_HARMFUL_APP_WARNINGS permission.",
                )));
            }
            let Some(package) = package
                .as_ref()
                .filter(|name| q.state.packages.contains_key(*name))
            else {
                return Ok(Err(Exception::illegal_argument(format!(
                    "Unknown package: {}",
                    package.as_deref().unwrap_or("null")
                ))));
            };
            return Ok(Ok(Plan {
                package: package.clone(),
                user: Some(*user),
                change: Change::HarmfulWarning(warning.clone()),
            }));
        }
        let (package, user) = match self {
            Self::Invalid(_) | Self::HarmfulWarning { .. } | Self::CategoryHint { .. } => {
                unreachable!()
            }
            Self::Enabled(s) => (&s.package, Some(s.user)),
            Self::Stopped { package, user, .. }
            | Self::SplashTheme { package, user, .. }
            | Self::MinAspectRatio { package, user, .. } => (package, Some(*user)),
            Self::MimeGroup { package, .. } | Self::UpdateAvailable { package, .. } => {
                (package, None)
            }
        };
        let plan = |change| Plan {
            package: package.clone(),
            user,
            change,
        };
        if let Self::Enabled(s) = self {
            return Ok(
                enabled::decide(q, s, pid, &|ps, user| Enabled::of(ps, user))?
                    .map(|state| plan(state.map(Change::Enabled).unwrap_or(Change::None))),
            );
        }
        if let Self::Stopped { user, stopped, .. } = self {
            if !q.state.users.contains_key(user) {
                return Ok(Ok(plan(Change::None)));
            }
            if apps_filter::instant_app_package_name(q.state, q.calling_uid)?.is_none() {
                if !permission(q, "android.permission.CHANGE_COMPONENT_ENABLED_STATE")?
                    && !owns(q, package)?
                {
                    return Ok(Err(Exception::security(format!(
                        "Permission Denial: attempt to change stopped state from pid={pid}, uid={}, package={package}",
                        q.calling_uid
                    ))));
                }
                if let Err(e) = q.enforce_cross_user(*user, true, true, "stop package")? {
                    return Ok(Err(e));
                }
                if let Some(ps) = q.state.packages.get(package) {
                    let state = info::user_state(ps, *user);
                    if state.installed
                        && !q.filtered_including_uninstalled(Some(ps), *user)?
                        && state.stopped != *stopped
                    {
                        return Ok(Ok(plan(Change::Stopped {
                            stopped: *stopped,
                            not_launched: false,
                            first_launch_installer: state
                                .not_launched
                                .then(|| ps.install_source.installer.clone())
                                .flatten(),
                            was_stopped: state.stopped,
                        })));
                    }
                }
            }
            return Ok(Ok(plan(Change::None)));
        }
        if matches!(
            self,
            Self::MinAspectRatio { .. } | Self::UpdateAvailable { .. }
        ) && !permission(q, "android.permission.INSTALL_PACKAGES")?
        {
            return Ok(Err(Exception::security(
                "requires INSTALL_PACKAGES permission",
            )));
        }
        if !matches!(self, Self::UpdateAvailable { .. }) {
            if let Some(user) = user {
                if let Err(e) =
                    q.enforce_cross_user(user, false, false, "package state mutation")?
                {
                    return Ok(Err(e));
                }
            }
            if apps_filter::app_id(q.calling_uid) != apps_filter::SYSTEM_UID {
                if !owns(q, package)? {
                    return Ok(Err(Exception::security(format!(
                        "Calling uid {} does not own package {package}",
                        q.calling_uid
                    ))));
                }
                let caller_user = apps_filter::user_id(q.calling_uid);
                match q.package_info(package, -1, 0, caller_user)? {
                    Err(e) => return Ok(Err(e)),
                    Ok(None) => {
                        return Ok(Err(Exception::illegal_argument(format!(
                            "Unknown package {package} on user {caller_user}"
                        ))));
                    }
                    _ => {}
                }
            }
        }
        let Some(ps) = q.state.packages.get(package) else {
            if matches!(self, Self::MimeGroup { .. }) {
                return Ok(Err(Exception::new(
                    EX_NULL_POINTER,
                    "MIME group package absent",
                )));
            }
            return Ok(Ok(plan(Change::None)));
        };
        if let Some(user) = user {
            if !q.state.users.contains_key(&user)
                || !info::user_state(ps, user).installed
                || q.filtered_including_uninstalled(Some(ps), user)?
            {
                return Ok(Ok(plan(Change::None)));
            }
        }
        let change = match self {
            Self::SplashTheme { theme, .. } => Change::SplashTheme(theme.clone()),
            Self::MinAspectRatio { ratio, .. } => Change::MinAspectRatio(*ratio),
            Self::UpdateAvailable { available, .. } => Change::UpdateAvailable(*available),
            Self::MimeGroup { group, types, .. } => {
                let mut values = Vec::new();
                for value in types {
                    let Some(value) = value else {
                        return Ok(Err(Exception::new(EX_NULL_POINTER, "null MIME type")));
                    };
                    if value.encode_utf16().count() > 255 {
                        return Ok(Err(Exception::illegal_argument(
                            "MIME type length exceeds 255 characters",
                        )));
                    }
                    values.push(value.clone());
                }
                let Some((_, existing)) = ps.mime_groups.iter().find(|(name, _)| name == group)
                else {
                    return Ok(Err(Exception::illegal_argument(format!(
                        "Unknown MIME group {group:?} for package {package}"
                    ))));
                };
                if existing.len() == values.len()
                    && values
                        .iter()
                        .all(|value| existing.contains(&Some(value.clone())))
                {
                    return Ok(Ok(plan(Change::None)));
                }
                if values.len() > 500 {
                    return Ok(Err(Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "Max limit on MIME types for MIME group exceeded",
                    )));
                }
                let mut distinct = Vec::new();
                for value in values {
                    if !distinct.contains(&value) {
                        distinct.push(value);
                    }
                }
                distinct.sort_by_key(|value| info::java_hash(value));
                Change::MimeGroup {
                    group: group.clone(),
                    types: distinct,
                }
            }
            _ => unreachable!(),
        };
        Ok(Ok(plan(change)))
    }
}
fn permission(q: &Query<'_>, name: &str) -> Result<bool, NotModelled> {
    match apps_filter::app_id(q.calling_uid) {
        apps_filter::ROOT_UID | apps_filter::SYSTEM_UID => Ok(true),
        _ if apps_filter::is_isolated(q.calling_uid) => Ok(false),
        _ => q.uid_has_permission(q.calling_uid, name),
    }
}
fn owns(q: &Query<'_>, package: &str) -> Result<bool, NotModelled> {
    Ok(q.packages_for_uid(q.calling_uid)?
        .is_some_and(|names| names.iter().any(|name| name.as_deref() == Some(package))))
}
impl Plan {
    pub fn apply(&self, state: &mut model::State) {
        let Some(ps) = state.packages.get_mut(&self.package) else {
            return;
        };
        match &self.change {
            Change::None => {}
            Change::MimeGroup { group, types } => {
                if let Some((_, current)) =
                    ps.mime_groups.iter_mut().find(|(name, _)| name == group)
                {
                    *current = types.iter().cloned().map(Some).collect();
                }
            }
            Change::UpdateAvailable(value) => ps.is.update_available = *value,
            Change::CategoryHint(value) => ps.category_override = *value,
            change => {
                let user = self.user.expect("user mutation");
                let current = ps.users.entry(user).or_default();
                match change {
                    Change::Enabled(enabled) => {
                        current.enabled = enabled.enabled;
                        current.last_disable_app_caller = enabled.last_disable_app_caller.clone();
                        current.enabled_components =
                            enabled.enabled_components.iter().cloned().collect();
                        current.disabled_components =
                            enabled.disabled_components.iter().cloned().collect();
                    }
                    Change::Stopped {
                        stopped,
                        not_launched,
                        ..
                    } => {
                        current.stopped = *stopped;
                        current.not_launched = *not_launched;
                    }
                    Change::SplashTheme(theme) => current.splash_screen_theme = theme.clone(),
                    Change::HarmfulWarning(warning) => {
                        current.harmful_app_warning = warning.clone()
                    }
                    Change::MinAspectRatio(ratio) => current.min_aspect_ratio = *ratio,
                    _ => unreachable!(),
                }
            }
        }
    }
}

impl Plan {
    pub fn apply_scan(&self, scan: &mut crate::package::scan::SigningScan) -> Result<(), String> {
        match &self.change {
            Change::None => Ok(()),
            Change::MimeGroup { group, types } => {
                let package = scan
                    .settings
                    .packages
                    .iter_mut()
                    .find(|package| package.name == self.package)
                    .ok_or("mutation package absent")?;
                let (_, current) = package
                    .mime_groups
                    .iter_mut()
                    .find(|(name, _)| name == group)
                    .ok_or("mutation MIME group absent")?;
                *current = types.iter().cloned().map(Some).collect();
                Ok(())
            }
            Change::CategoryHint(value) => {
                scan.settings
                    .packages
                    .iter_mut()
                    .find(|package| package.name == self.package)
                    .ok_or("mutation package absent")?
                    .category_hint = *value;
                Ok(())
            }
            Change::UpdateAvailable(value) => {
                scan.settings
                    .packages
                    .iter_mut()
                    .find(|package| package.name == self.package)
                    .ok_or("mutation package absent")?
                    .update_available = *value;
                Ok(())
            }
            change => {
                let user = self.user.ok_or("mutation user absent")?;
                let mut current = scan
                    .scanned_user_states(&self.package)
                    .and_then(|users| users.get(&user))
                    .cloned()
                    .ok_or("mutation user owner absent")?;
                match change {
                    Change::Enabled(enabled) => {
                        current.enabled = enabled.enabled;
                        current.last_disable_app_caller = enabled.last_disable_app_caller.clone();
                        current.enabled_components =
                            Some(enabled.enabled_components.iter().cloned().collect());
                        current.disabled_components =
                            Some(enabled.disabled_components.iter().cloned().collect());
                    }
                    Change::Stopped {
                        stopped,
                        not_launched,
                        ..
                    } => {
                        current.stopped = *stopped;
                        current.not_launched = *not_launched;
                    }
                    Change::SplashTheme(theme) => current.splash_screen_theme = theme.clone(),
                    Change::HarmfulWarning(warning) => {
                        current.harmful_app_warning = warning.clone()
                    }
                    Change::MinAspectRatio(ratio) => current.min_aspect_ratio = *ratio,
                    _ => unreachable!(),
                }
                scan.set_user_state(&self.package, user, current)
            }
        }
    }
}

pub fn reply(code: u32) -> aim_binder_host::parcel::Parcel {
    let mut reply = aim_binder_host::parcel::Parcel::new();
    match code {
        pm::SET_COMPONENT_ENABLED_SETTING | pm::SET_APPLICATION_ENABLED_SETTING => {
            return enabled::reply(code);
        }
        pm::SET_PACKAGE_STOPPED_STATE => pm::write_set_package_stopped_state_reply(&mut reply),
        pm::SET_SPLASH_SCREEN_THEME => pm::write_set_splash_screen_theme_reply(&mut reply),
        pm::SET_HARMFUL_APP_WARNING => pm::write_set_harmful_app_warning_reply(&mut reply),
        pm::SET_APPLICATION_CATEGORY_HINT => {
            pm::write_set_application_category_hint_reply(&mut reply)
        }
        pm::SET_USER_MIN_ASPECT_RATIO => pm::write_set_user_min_aspect_ratio_reply(&mut reply),
        pm::SET_MIME_GROUP => pm::write_set_mime_group_reply(&mut reply),
        pm::SET_UPDATE_AVAILABLE => pm::write_set_update_available_reply(&mut reply),
        _ => unreachable!("unsupported mutation reply"),
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::apps_filter::{AppsFilter, Config};
    fn state() -> model::State {
        model::State {
            packages: [(
                "fixture".into(),
                model::PackageState {
                    name: "fixture".into(),
                    app_id: 10100,
                    mime_groups: vec![(Some("group".into()), vec![Some("old".into())])],
                    users: [(
                        0,
                        model::PackageUserState {
                            stopped: true,
                            not_launched: true,
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                model::User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        }
    }
    #[test]
    fn category_hint_requires_recorded_installer_and_keeps_noop_generation() {
        let mut state = state();
        state.renamed_packages = Some(Vec::new());
        let ps = state.packages.get_mut("fixture").unwrap();
        ps.pkg = Some(std::sync::Arc::new(crate::package::pkg::AndroidPackage {
            package_name: "fixture".into(),
            uid: 10100,
            ..Default::default()
        }));
        ps.install_source.installer = Some("fixture".into());
        ps.category_override = 4;
        let decide = |state: &model::State, caller: Option<String>, category| {
            let filter = AppsFilter::new(state, &Config::default()).unwrap();
            Request::CategoryHint {
                package: Some("fixture".into()),
                caller,
                category,
            }
            .decide(
                &Query {
                    state,
                    filter: &filter,
                    calling_uid: 10100,
                },
                1,
            )
            .unwrap()
        };
        assert_eq!(
            decide(&state, Some("fixture".into()), 4).unwrap().change,
            Change::None
        );
        let plan = decide(&state, Some("fixture".into()), -123).unwrap();
        assert_eq!(plan.change, Change::CategoryHint(-123)); // original accepts any int
        let mut next = state.clone();
        plan.apply(&mut next);
        assert_eq!(next.packages["fixture"].category_override, -123);
        assert_eq!(
            decide(&state, None, 5).unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        state
            .packages
            .get_mut("fixture")
            .unwrap()
            .install_source
            .installer = None;
        assert_eq!(
            decide(&state, Some("fixture".into()), 5).unwrap_err().code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
    }
    #[test]
    fn harmful_warning_mutates_uninstalled_hidden_state_and_rejects_unknown_package() {
        let mut state = state();
        let user = state
            .packages
            .get_mut("fixture")
            .unwrap()
            .users
            .get_mut(&0)
            .unwrap();
        user.installed = false;
        user.hidden = true;
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        let q = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let request = Request::HarmfulWarning {
            package: Some("fixture".into()),
            user: 0,
            warning: Some(" ⚠<&😀 ".into()),
        };
        let plan = request.decide(&q, 1).unwrap().unwrap();
        let mut next = state.clone();
        plan.apply(&mut next);
        assert_eq!(
            next.packages["fixture"].users[&0]
                .harmful_app_warning
                .as_deref(),
            Some(" ⚠<&😀 ")
        );
        for package in [None, Some("missing".into())] {
            assert_eq!(
                Request::HarmfulWarning {
                    package,
                    user: 0,
                    warning: None
                }
                .decide(&q, 1)
                .unwrap()
                .unwrap_err()
                .code,
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
            );
        }
        assert!(
            Request::HarmfulWarning {
                package: Some("fixture".into()),
                user: -1,
                warning: None
            }
            .decide(&q, 1)
            .unwrap()
            .is_err()
        );
    }
    #[test]
    fn harmful_warning_checks_permission_before_package_and_requires_full_cross_user() {
        let mut state = state();
        let request = Request::HarmfulWarning {
            package: None,
            user: 0,
            warning: None,
        };
        let decide = |state: &model::State, request: &Request| {
            let filter = AppsFilter::new(state, &Config::default()).unwrap();
            request
                .decide(
                    &Query {
                        state,
                        filter: &filter,
                        calling_uid: 10100,
                    },
                    1,
                )
                .unwrap()
        };
        assert_eq!(
            decide(&state, &request).unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        state
            .packages
            .get_mut("fixture")
            .unwrap()
            .users
            .get_mut(&0)
            .unwrap()
            .granted_permissions
            .extend([
                "android.permission.SET_HARMFUL_APP_WARNINGS".into(),
                "android.permission.INTERACT_ACROSS_USERS".into(),
            ]);
        assert_eq!(
            decide(&state, &request).unwrap_err().code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
        let other = Request::HarmfulWarning {
            package: Some("fixture".into()),
            user: 10,
            warning: None,
        };
        assert_eq!(
            decide(&state, &other).unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
        state
            .packages
            .get_mut("fixture")
            .unwrap()
            .users
            .get_mut(&0)
            .unwrap()
            .granted_permissions
            .push("android.permission.INTERACT_ACROSS_USERS_FULL".into());
        assert!(decide(&state, &other).is_ok());
    }
    #[test]
    fn harmful_warning_generated_decoder_consumes_styled_text_and_rejects_tail() {
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        parcel.write_interface_token(pm::DESCRIPTOR);
        parcel.write_string16(Some("fixture"));
        parcel.write_i32(1); // non-null typed CharSequence
        parcel.write_i32(0); // SpannedString
        parcel.write_string8(Some("warning😀"));
        parcel.write_i32(2); // ForegroundColorSpan
        parcel.write_i32(0xff123456u32 as i32);
        for value in [0, 7, 0] {
            parcel.write_i32(value);
        }
        parcel.write_i32(0); // spans end
        parcel.write_i32(0); // user
        assert!(
            matches!(Request::read(pm::SET_HARMFUL_APP_WARNING, 1000, &mut Reader::new(parcel.data(), &[])), Some(Ok(Request::HarmfulWarning { warning: Some(value), .. })) if value == "warning😀")
        );
        parcel.write_i32(123);
        assert!(matches!(
            Request::read(
                pm::SET_HARMFUL_APP_WARNING,
                1000,
                &mut Reader::new(parcel.data(), &[])
            ),
            Some(Err(BAD_VALUE))
        ));
    }
    #[test]
    fn stopped_transition_clears_first_launch_only_on_actual_change() {
        let state = state();
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        let q = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let request = Request::Stopped {
            package: "fixture".into(),
            user: 0,
            stopped: false,
        };
        let plan = request.decide(&q, 1).unwrap().unwrap();
        assert_eq!(
            plan.change,
            Change::Stopped {
                stopped: false,
                not_launched: false,
                first_launch_installer: None,
                was_stopped: true
            }
        );
        let mut next = state.clone();
        plan.apply(&mut next);
        let filter = AppsFilter::new(&next, &Config::default()).unwrap();
        let q = Query {
            state: &next,
            filter: &filter,
            calling_uid: 1000,
        };
        assert_eq!(request.decide(&q, 1).unwrap().unwrap().change, Change::None);
    }
    #[test]
    fn mime_group_rejects_utf16_length_null_and_raw_count_before_deduplication() {
        let state = state();
        let filter = AppsFilter::new(&state, &Config::default()).unwrap();
        let q = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        for (types, code) in [
            (
                vec![Some("😀".repeat(128))],
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT,
            ),
            (vec![None], EX_NULL_POINTER),
            (
                vec![Some("new".into()); 501],
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
            ),
        ] {
            assert_eq!(
                Request::MimeGroup {
                    package: "fixture".into(),
                    group: Some("group".into()),
                    types
                }
                .decide(&q, 1)
                .unwrap()
                .unwrap_err()
                .code,
                code
            );
        }
        let plan = Request::MimeGroup {
            package: "fixture".into(),
            group: Some("group".into()),
            types: vec![Some("new".into()), Some("new".into())],
        }
        .decide(&q, 1)
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.change,
            Change::MimeGroup {
                group: Some("group".into()),
                types: vec!["new".into()]
            }
        );
    }
    #[test]
    fn null_package_is_an_exception_not_a_transport_failure() {
        let mut data = aim_binder_host::parcel::Parcel::new();
        pm::SetSplashScreenTheme {
            package_name: None,
            theme_name: None,
            user_id: 0,
        }
        .write(&mut data);
        assert!(matches!(
            Request::read(
                pm::SET_SPLASH_SCREEN_THEME,
                1000,
                &mut Reader::new(data.data(), &[])
            ),
            Some(Ok(Request::Invalid(Exception {
                code: EX_NULL_POINTER,
                ..
            })))
        ));
    }
    #[test]
    fn generated_decoder_rejects_tail_without_planning_a_write() {
        let mut data = aim_binder_host::parcel::Parcel::new();
        pm::SetSplashScreenTheme {
            package_name: Some("fixture".into()),
            theme_name: None,
            user_id: 0,
        }
        .write(&mut data);
        data.write_i32(1);
        assert!(matches!(
            Request::read(
                pm::SET_SPLASH_SCREEN_THEME,
                1000,
                &mut Reader::new(data.data(), &[])
            ),
            Some(Err(BAD_VALUE))
        ));
    }
}
