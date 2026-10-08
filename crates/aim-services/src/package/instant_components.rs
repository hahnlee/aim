//! PMS instant-app resolver components at android-16.0.0_r1 (AOSP Apache-2.0).
use super::{
    apps_filter::{self, Config as FilterConfig, NotModelled},
    info::flags::*,
    intent::{ComponentName, Intent},
    model::State,
    parse::{
        Platform,
        resources::{Config, Resources},
    },
    query::Query,
    resolve::{Resolution, ResolutionError},
};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    resolver_packages: Vec<Option<String>>,
    debuggable: bool,
    installer: Option<ComponentName>,
    settings: Option<ComponentName>,
    installer_info: Option<Vec<u8>>,
    installer_resolve_info: Option<super::component_resolver::ResolveInfo>,
}
impl Owner {
    pub fn load(
        platform: &Platform,
        config: Config,
        debuggable: bool,
        installer: Option<ComponentName>,
        settings: Option<ComponentName>,
    ) -> Result<Self, String> {
        let resources = Resources {
            tables: vec![&platform.framework],
            overlays: &platform.framework_overlays,
            config,
        };
        let resolver_packages = platform
            .framework
            .id("array", "config_ephemeralResolverPackage")
            .and_then(|id| resources.string_array(id))
            .ok_or("instant resolver package resource unavailable")?;
        Ok(Self {
            resolver_packages,
            debuggable,
            installer,
            settings,
            installer_info: None,
            installer_resolve_info: None,
        })
    }
    /// The installer and settings are selected once by the native PMS constructor.
    pub fn select_boot(state: &Arc<State>, platform: &Platform, config: Config,
        debuggable: bool, eng_build: bool) -> Result<Self, String> {
        let resolution = Resolution::new(state.clone(), &FilterConfig {
            force_system_packages_queryable: state.system.force_system_packages_queryable,
            force_queryable_packages: state.system.force_queryable_packages.clone(),
        }).map_err(|error| format!("instant constructor registration: {error:?}"))?;
        let query = Query { state, filter: &resolution.apps_filter, calling_uid: 1000 };
        let mut owner = Self::load(platform, config, debuggable, None, None)?;
        // PMS constructs one empty ResolveInfo even when no installer is selected.
        let mut empty = aim_binder_host::parcel::Parcel::new();
        empty.write_i32(0);
        empty.write_i32(0);
        for value in [0, 0, 0, -1, 0] { empty.write_i32(value); }
        crate::clip::write_char_sequence(&mut empty, None);
        empty.write_i32(0);
        empty.write_string8(None);
        empty.write_i32(super::component_resolver::USER_CURRENT);
        for _ in 0..6 { empty.write_i32(0); }
        empty.write_i32(super::component_resolver::USER_CURRENT);
        owner.installer_info = Some(empty.data().to_vec());

        let mut matches = Vec::new();
        let actions: &[&str] = if eng_build {
            &["android.intent.action.INSTALL_INSTANT_APP_PACKAGE_TEST", "android.intent.action.INSTALL_INSTANT_APP_PACKAGE"]
        } else { &["android.intent.action.INSTALL_INSTANT_APP_PACKAGE"] };
        for action in actions {
            matches = resolution.query_intent_activities(&Intent {
                action: Some((*action).into()), categories: Some(vec!["android.intent.category.DEFAULT".into()]),
                data: Some(super::uri::Uri::parse("file:///foo.apk")),
                ty: Some("application/vnd.android.package-archive".into()), ..Default::default()
            }, Some("application/vnd.android.package-archive"),
                MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE
                    | i64::from(super::intent::FLAG_IGNORE_EPHEMERAL)
                    | if eng_build { 0 } else { MATCH_SYSTEM_ONLY }, 0, 1000)
                .map_err(|error| format!("instant installer selection: {error:?}"))?;
            if !matches.is_empty() { break; }
        }
        let mut accepted = Vec::new();
        for entry in matches {
            let (package, class) = entry.component();
            if eng_build || query.check_package_permission(Some(package), Some("android.permission.INSTALL_PACKAGES"), 0)
                .map_err(|error| format!("instant installer permission: {error:?}"))? == 0 {
                let component = ComponentName { package: package.into(), class: class.into() };
                let super::component_resolver::Info::Activity(activity) = entry.info else {
                    return Err("instant installer resolution returned non-activity".into());
                };
                accepted.push((component, activity));
            }
        }
        if accepted.len() > 1 { return Err(format!("There must be at most one ephemeral installer; found {accepted:?}")); }
        if let Some((component, mut activity)) = accepted.pop() {
            owner.installer = Some(component);
            activity.flags |= 0x20 | 0x100;
            activity.info.exported = true;
            activity.info.enabled = true;
            let mut info = super::component_resolver::ResolveInfo::new(super::component_resolver::Info::Activity(activity));
            info.priority = 1;
            info.preferred_order = 1;
            info.is_default = true;
            info.match_ = super::intent_filter::MATCH_CATEGORY_SCHEME_SPECIFIC_PART
                | super::intent_filter::MATCH_ADJUSTMENT_NORMAL;
            let mut parcel = aim_binder_host::parcel::Parcel::new();
            aim_service_aidl::WriteParcelable::write_to(&info, &mut parcel);
            owner.installer_info = Some(parcel.data().to_vec());
            owner.installer_resolve_info = Some(info);
        }

        if let Some(resolver) = owner.resolver(&query).map_err(|error| format!("instant resolver selection: {error:?}"))? {
            let matches = resolution.query_intent_activities(&Intent {
                action: Some("android.intent.action.INSTANT_APP_RESOLVER_SETTINGS".into()),
                categories: Some(vec!["android.intent.category.DEFAULT".into()]),
                package: Some(resolver.package), ..Default::default()
            }, None, MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE, 0, 1000)
                .map_err(|error| format!("instant resolver settings selection: {error:?}"))?;
            owner.settings = matches.first().map(|entry| {
                let (package, class) = entry.component();
                ComponentName { package: package.into(), class: class.into() }
            });
        }
        Ok(owner)
    }

    #[cfg(test)]
    pub(crate) fn for_resolution_test(installer: Option<super::component_resolver::ResolveInfo>) -> Self {
        Self { installer: installer.as_ref().map(|info| {let (package,class)=info.component();ComponentName {package:package.into(),class:class.into()}}),
            installer_resolve_info: installer, installer_info: None, settings: None,
            resolver_packages: Vec::new(), debuggable: false }
    }

    pub fn installer_resolve_info(&self)->Option<&super::component_resolver::ResolveInfo> {self.installer_resolve_info.as_ref()}

    pub fn installer_info_record(&self) -> Option<&[u8]> { self.installer_info.as_deref() }

    pub fn installer_component(&self) -> Option<super::intent::ComponentName> {
        self.installer.clone()
    }

    pub fn installer_package(&self) -> Option<String> {
        self.installer.as_ref().map(|component| component.package.clone())
    }

    pub(crate) fn needs_resolution(&self) -> bool {
        !self.resolver_packages.is_empty() || self.debuggable
    }
    pub(crate) fn resolver(&self, query: &Query<'_>) -> Result<Option<ComponentName>, NotModelled> {
        if apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some() {
            return Ok(None);
        }
        if self.resolver_packages.is_empty() && !self.debuggable {
            return Ok(None);
        }
        let resolution = Resolution::new(
            Arc::new(query.state.clone()),
            &FilterConfig {
                force_system_packages_queryable: query.state.system.force_system_packages_queryable,
                force_queryable_packages: query.state.system.force_queryable_packages.clone(),
            },
        )
        .map_err(|_| NotModelled("instant resolver registration owner"))?;
        let matches = resolution
            .query_intent_services(
                &Intent {
                    action: Some("android.intent.action.RESOLVE_INSTANT_APP_PACKAGE".into()),
                    ..Default::default()
                },
                None,
                MATCH_DIRECT_BOOT_AWARE
                    | MATCH_DIRECT_BOOT_UNAWARE
                    | if self.debuggable {
                        0
                    } else {
                        MATCH_SYSTEM_ONLY
                    },
                0,
                query.calling_uid,
            )
            .map_err(|error| match error {
                ResolutionError::Original(_) => NotModelled("original resolver permission owner exception"),
                ResolutionError::NotModelled(error) => error,
                ResolutionError::UriMatching(_) => NotModelled("instant resolver URI matching"),
            })?;
        Ok(matches.into_iter().find_map(|entry| {
            let (package, class) = entry.component();
            (self.debuggable
                || self
                    .resolver_packages
                    .iter()
                    .any(|name| name.as_deref() == Some(package)))
            .then(|| ComponentName {
                package: package.into(),
                class: class.into(),
            })
        }))
    }
    pub(crate) fn installer(
        &self,
        query: &Query<'_>,
    ) -> Result<Option<ComponentName>, NotModelled> {
        if apps_filter::instant_app_package_name(query.state, query.calling_uid)?.is_some() {
            return Ok(None);
        }
        Ok(self.installer.clone())
    }
    pub(crate) fn settings(&self) -> Option<ComponentName> {
        self.settings.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        apps_filter::AppsFilter,
        model::{PackageState, PackageUserState, User},
    };
    use aim_binder_host::parcel::{Parcel, Reader};
    use aim_service_aidl::android_content_pm_ipackagemanager as pm;
    #[test]
    fn instant_caller_hides_installer_and_resolver_but_not_frozen_settings() {
        let component = ComponentName {
            package: "installer".into(),
            class: "Installer".into(),
        };
        let owner = Owner {
            installer_info: None, installer_resolve_info: None,
            resolver_packages: vec![],
            debuggable: false,
            installer: Some(component.clone()),
            settings: Some(component.clone()),
        };
        let mut state = State::default();
        state.users.insert(
            0,
            User {
                id: 0,
                ..Default::default()
            },
        );
        state.packages.insert(
            "instant".into(),
            PackageState {
                name: "instant".into(),
                app_id: 10100,
                users: [(
                    0,
                    PackageUserState {
                        instant_app: true,
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            },
        );
        state.system.instant_components = Some(Arc::new(owner));
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 10100,
        };
        let owner = state.system.instant_components.as_ref().unwrap();
        assert!(owner.resolver(&query).unwrap().is_none());
        assert!(owner.installer(&query).unwrap().is_none());
        assert_eq!(owner.settings(), Some(component));
        let mut args = Parcel::new();
        pm::GetInstantAppResolverSettingsComponent {}.write(&mut args);
        let reply = query
            .answer(
                pm::DESCRIPTOR,
                pm::GET_INSTANT_APP_RESOLVER_SETTINGS_COMPONENT,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        let mut reader = Reader::new(reply.data(), reply.objects());
        assert!(reader.read_exception().unwrap().is_ok());
        assert_eq!(reader.read_i32().unwrap(), 1);
        assert_eq!(
            reader.read_string16().unwrap().as_deref(),
            Some("installer")
        );
        assert_eq!(
            reader.read_string16().unwrap().as_deref(),
            Some("Installer")
        );
        assert_eq!(reader.remaining(), 0);
        let normal = Query {
            calling_uid: 1000,
            ..query
        };
        assert!(owner.resolver(&normal).unwrap().is_none());
        assert!(owner.installer(&normal).unwrap().is_some());
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::package::{apps_filter::AppsFilter, model::{PackageState, PackageUserState, StateFlags, User},
        pkg::{AndroidPackage, Service, booleans}, intent_filter::{IntentFilter, ParsedIntentInfo}};
    #[test]
    fn resolver_selection_applies_caller_visibility_and_configured_allowlist() {
        let mut service = Service::default();
        service.main.enabled = true;
        service.main.exported = true;
        service.main.component.name = "p.Resolver".into();
        service.main.component.package_name = "p".into();
        service.main.component.intents.push(ParsedIntentInfo { filter: IntentFilter {
            actions: vec!["android.intent.action.RESOLVE_INSTANT_APP_PACKAGE".into()],
            ..Default::default() }, ..Default::default() });
        let state = State { packages: [("p".into(), PackageState {
            name: "p".into(), app_id: 10100, is: StateFlags { system: true, ..Default::default() },
            pkg: Some(Arc::new(AndroidPackage { package_name: "p".into(), uid: 10100,
                booleans: booleans::ENABLED | booleans::SYSTEM, services: vec![service], ..Default::default() })),
            users: [(0, PackageUserState::default())].into(), ..Default::default() })].into(),
            users: [(0, User { id: 0, ..Default::default() })].into(), ..Default::default() };
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query { state: &state, filter: &filter, calling_uid: 1000 };
        let mut owner = Owner { installer_info: None, installer_resolve_info: None, resolver_packages: vec![Some("p".into())], debuggable: false,
            installer: None, settings: None };
        assert_eq!(owner.resolver(&query).unwrap().unwrap().class, "p.Resolver");
        owner.resolver_packages = vec![Some("foreign".into())];
        assert!(owner.resolver(&query).unwrap().is_none());
        owner.debuggable = true;
        assert!(owner.resolver(&query).unwrap().is_some());
    }
}
