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
        let (package, user) = match self {
            Self::Invalid(_) => unreachable!(),
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
