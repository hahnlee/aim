//! PMS configured package selections, android-16.0.0_r1. Copyright AOSP Apache-2.0.
use super::{
    apps_filter::{AppsFilter, Config as FilterConfig},
    info::flags::*,
    intent::Intent,
    model::State,
    parse::{
        Platform,
        resources::{Config, Resources},
    },
    query::Query,
    resolve::Resolution,
};
use aim_binder_host::parcel::Exception;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    DefaultTextClassifier,
    SystemTextClassifier,
    AppPrediction,
    Configurator,
    AmbientContext,
    WearableSensing,
    Recents,
    IncidentApprover,
    Attention,
    RotationResolver,
    SystemCaptions,
    SetupWizard,
    ServicesExtension,
    SharedSystemLibrary,
}
/// DefaultAppProvider queries RoleManager for each invocation, independently
/// of Computer's captured package metadata. It does not cache role callbacks.
#[derive(Clone)]
pub struct BrowserSource(Arc<super::bootstrap::Bridge>);
impl BrowserSource {
    pub fn original(bridge: Arc<super::bootstrap::Bridge>) -> Self { Self(bridge) }
    fn get(&self, user: i32) -> Result<Option<String>, super::apps_filter::NotModelled> {
        self.0.preferred_role_holder("android.app.role.BROWSER", user)
            .map_err(|_| super::apps_filter::NotModelled("actual DefaultAppProvider browser read failed"))
    }
}
impl std::fmt::Debug for BrowserSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { formatter.write_str("DefaultAppProvider") }
}
impl PartialEq for BrowserSource {
    fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    frozen: BTreeMap<Role, Option<String>>,
    live: BTreeMap<Role, Option<String>>,
    known: Option<BTreeMap<i32, Vec<Option<String>>>>,
    browsers: BTreeMap<i32, Option<String>>,
    browser_source: Option<BrowserSource>,
}
impl Owner {
    /// None is the boot scan's deferred phase; Some(None) is an authoritative
    /// selection with no setup wizard. No package name is inferred here.
    #[cfg(test)]
    pub(crate) fn priority_fixture(wizard:Option<String>)->Arc<Self>{
        Arc::new(Self{frozen:[(Role::SetupWizard,wizard)].into(),live:Default::default(),known:None,browsers:Default::default(),browser_source:None})
    }
    pub(crate) fn setup_wizard_priority_owner(&self)->Option<Option<&str>> {
        self.frozen.get(&Role::SetupWizard).map(|name|name.as_deref())
    }

    /// Called at the full native constructor gate using the applied resource configuration.
    pub fn load(state: &Arc<State>, platform: &Platform, config: Config) -> Result<Self, String> {
        let resources = Resources {
            tables: vec![&platform.framework],
            overlays: &platform.framework_overlays,
            config,
        };
        let string = |name: &str| {
            platform
                .framework
                .id("string", name)
                .and_then(|id| resources.resource_string(id))
                .ok_or_else(|| format!("configured role resource unavailable: {name}"))
        };
        let filter = AppsFilter::new(
            state,
            &FilterConfig {
                force_system_packages_queryable: state.system.force_system_packages_queryable,
                force_queryable_packages: state.system.force_queryable_packages.clone(),
            },
        )
        .map_err(|e| format!("configured role visibility: {e:?}"))?;
        let query = Query {
            state,
            filter: &filter,
            calling_uid: 1000,
        };
        let mut frozen = BTreeMap::new();
        for (role, name, component) in [
            (
                Role::DefaultTextClassifier,
                "config_servicesExtensionPackage",
                false,
            ),
            (
                Role::SystemTextClassifier,
                "config_defaultTextClassifierPackage",
                false,
            ),
            (
                Role::AppPrediction,
                "config_defaultAppPredictionService",
                true,
            ),
            (
                Role::IncidentApprover,
                "config_incidentReportApproverPackage",
                false,
            ),
            (
                Role::Configurator,
                "config_deviceConfiguratorPackageName",
                false,
            ),
            (
                Role::AmbientContext,
                "config_defaultAmbientContextDetectionService",
                true,
            ),
            (
                Role::WearableSensing,
                "config_defaultWearableSensingService",
                true,
            ),
            (Role::Recents, "config_recentsComponentName", true),
        ] {
            let value = string(name)?;
            let package = if component {
                component_package(&value)
            } else {
                Some(value)
            };
            frozen.insert(
                role,
                ensure(&query, package.as_deref())
                    .map_err(|e| format!("configured role {role:?}: {e:?}"))?,
            );
        }
        let extension = string("config_servicesExtensionPackage")?;
        if extension.is_empty() {
            return Err("required services extension configuration is empty".into());
        }
        let extension = ensure(&query, Some(&extension))
            .map_err(|e| format!("services extension selection: {e:?}"))?
            .ok_or("required services extension package is missing")?;
        frozen.insert(Role::ServicesExtension, Some(extension));
        frozen.insert(
            Role::SharedSystemLibrary,
            Some(required_shared_library(state)?),
        );
        let resolution = Resolution::new(
            state.clone(),
            &FilterConfig {
                force_system_packages_queryable: state.system.force_system_packages_queryable,
                force_queryable_packages: state.system.force_queryable_packages.clone(),
            },
        )
        .map_err(|e| format!("setup wizard resolver: {e:?}"))?;
        let intent = Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec!["android.intent.category.SETUP_WIZARD".into()]),
            ..Default::default()
        };
        let matches = resolution
            .query_intent_activities(
                &intent,
                None,
                MATCH_SYSTEM_ONLY
                    | MATCH_DIRECT_BOOT_AWARE
                    | MATCH_DIRECT_BOOT_UNAWARE
                    | MATCH_DISABLED_COMPONENTS,
                0,
                1000,
            )
            .map_err(|e| format!("setup wizard resolution: {e:?}"))?;
        frozen.insert(
            Role::SetupWizard,
            if matches.len() == 1 {
                Some(matches[0].component().0.into())
            } else {
                None
            },
        );
        let mut live = BTreeMap::new();
        for (role, name) in [
            (Role::Attention, "config_defaultAttentionService"),
            (
                Role::RotationResolver,
                "config_defaultRotationResolverService",
            ),
            (
                Role::SystemCaptions,
                "config_defaultSystemCaptionsManagerService",
            ),
        ] {
            live.insert(role, component_package(&string(name)?));
        }
        Ok(Self {
            frozen,
            live,
            known: None,
            browsers: BTreeMap::new(),
            browser_source: None,
        })
    }

    /// PMS constructor selections after the accepted system/non-system scan.
    /// `overlay_signature_package` is SystemConfig's real selected package,
    /// including its legitimate null value, not a framework resource guess.
    pub fn prepare_known_packages(
        &self,
        state: &Arc<State>,
        platform: &Platform,
        config: Config,
        overlay_signature_package: Option<&str>,
    ) -> Result<Self, String> {
        use super::uri::Uri;
        use sha2::{Digest, Sha256};
        let filter_config = FilterConfig {
            force_system_packages_queryable: state.system.force_system_packages_queryable,
            force_queryable_packages: state.system.force_queryable_packages.clone(),
        };
        let resolution = Resolution::new(state.clone(), &filter_config)
            .map_err(|error| format!("known package resolution: {error:?}"))?;
        let filter = AppsFilter::new(state, &filter_config)
            .map_err(|error| format!("known package visibility: {error:?}"))?;
        let query = Query {
            state,
            filter: &filter,
            calling_uid: 1000,
        };
        let flags = MATCH_SYSTEM_ONLY | MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE;
        let installer_intent = Intent {
            action: Some("android.intent.action.INSTALL_PACKAGE".into()),
            categories: Some(vec!["android.intent.category.DEFAULT".into()]),
            data: Some(Uri::parse("content://com.example/foo.apk")),
            ty: Some("application/vnd.android.package-archive".into()),
            ..Default::default()
        };
        let installer = resolution
            .query_intent_activities(
                &installer_intent,
                Some("application/vnd.android.package-archive"),
                flags,
                0,
                1000,
            )
            .map_err(|error| format!("installer resolution: {error:?}"))?;
        if installer.len() != 1 {
            return Err("There must be exactly one installer".into());
        }
        let installer = installer[0].component().0.to_owned();
        if !state
            .packages
            .get(&installer)
            .is_some_and(|package| package.is.privileged)
        {
            return Err("The installer must be a privileged app".into());
        }
        let uninstall_intent = Intent {
            action: Some("android.intent.action.UNINSTALL_PACKAGE".into()),
            categories: Some(vec!["android.intent.category.DEFAULT".into()]),
            data: Some(Uri::preferred_opaque("package", "foo.bar")),
            ..Default::default()
        };
        let uninstaller = resolution
            .resolve_intent(&uninstall_intent, None, flags, 0, 1000)
            .map_err(|error| format!("uninstaller resolution: {error:?}"))?
            .ok_or("There must be exactly one uninstaller")?;
        if uninstaller.component().1 == "com.android.internal.app.ResolverActivity" {
            return Err("There must be exactly one uninstaller".into());
        }
        let verifier_intent = Intent {
            action: Some("android.intent.action.PACKAGE_NEEDS_VERIFICATION".into()),
            ..Default::default()
        };
        let verifiers = resolution
            .query_intent_receivers(
                &verifier_intent,
                Some("application/vnd.android.package-archive"),
                flags,
                0,
                1000,
            )
            .map_err(|error| format!("verifier resolution: {error:?}"))?;
        if verifiers.len() > 2 {
            return Err("There must be no more than 2 verifiers".into());
        }
        let mut verifier_names = Vec::with_capacity(verifiers.len());
        for verifier in verifiers {
            let name = verifier.component().0;
            if name.is_empty() {
                return Err("Invalid verifier".into());
            }
            verifier_names.push(Some(name.to_owned()));
        }
        let permission_intent = Intent {
            action: Some("android.intent.action.MANAGE_PERMISSIONS".into()),
            categories: Some(vec!["android.intent.category.DEFAULT".into()]),
            ..Default::default()
        };
        let controllers = resolution
            .query_intent_activities(&permission_intent, None, flags, 0, 1000)
            .map_err(|error| format!("permission controller resolution: {error:?}"))?;
        if controllers.len() != 1 {
            return Err("There must be exactly one permissions manager".into());
        }
        let controller = controllers[0].component().0.to_owned();
        if !state
            .packages
            .get(&controller)
            .is_some_and(|package| package.is.privileged)
        {
            return Err("The permissions manager must be a privileged app".into());
        }
        let resources = Resources {
            tables: vec![&platform.framework],
            overlays: &platform.framework_overlays,
            config,
        };
        let text = |name: &str| {
            platform
                .framework
                .id("string", name)
                .and_then(|id| resources.resource_string(id))
                .ok_or_else(|| format!("known package resource unavailable: {name}"))
        };
        let retail_name = text("config_retailDemoPackage")?;
        let retail_signature = text("config_retailDemoPackageSignature")?;
        let retail = if retail_name.is_empty() || retail_signature.is_empty() {
            None
        } else {
            state
                .packages
                .get(&retail_name)
                .and_then(|package| package.pkg.as_ref())
                .and_then(|package| package.signing_details.as_ref())
                .and_then(|details| details.signatures.as_ref())
                .filter(|signatures| {
                    signatures.iter().any(|signature| {
                        let hash = Sha256::digest(signature);
                        let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
                        hex == retail_signature
                    })
                })
                .map(|_| retail_name)
        };
        let frozen = |role| {
            self.frozen
                .get(&role)
                .cloned()
                .ok_or_else(|| format!("known constructor selection unavailable: {role:?}"))
        };
        let overlay = ensure(&query, overlay_signature_package).map_err(|error| error.message)?;
        let mut known = BTreeMap::from([
            (1, vec![frozen(Role::SetupWizard)?]),
            (2, vec![Some(installer)]),
            (3, vec![Some(uninstaller.component().0.to_owned())]),
            (4, verifier_names),
            (
                6,
                vec![
                    frozen(Role::DefaultTextClassifier)?,
                    frozen(Role::SystemTextClassifier)?,
                ],
            ),
            (7, vec![Some(controller)]),
            (10, vec![frozen(Role::Configurator)?]),
            (11, vec![frozen(Role::IncidentApprover)?]),
            (12, vec![frozen(Role::AppPrediction)?]),
            (13, vec![overlay]),
            (15, vec![Some("com.android.companiondevicemanager".into())]),
            (17, vec![frozen(Role::Recents)?]),
            (18, vec![frozen(Role::AmbientContext)?]),
            (19, vec![frozen(Role::WearableSensing)?]),
        ]);
        known.insert(16, retail.map(|name| vec![Some(name)]).unwrap_or_default());
        let mut result = self.clone();
        result.known = Some(known);
        Ok(result)
    }

    pub fn with_browser_source(&self, source: BrowserSource) -> Self {
        let mut result = self.clone();
        result.browser_source = Some(source);
        result
    }

    /// Apply the DefaultAppProvider owner's current result, preserving null.
    pub fn with_default_browser(&self, user: i32, browser: Option<String>) -> Self {
        let mut result = self.clone();
        result.browsers.insert(user, browser);
        result
    }

    pub fn known_packages(
        &self,
        query: &Query<'_>,
        kind: i32,
        user: i32,
    ) -> Result<Vec<Option<String>>, super::apps_filter::NotModelled> {
        use super::apps_filter::NotModelled;
        match kind {
            0 => return Ok(vec![Some("android".into())]),
            5 => {
                if let Some(source) = &self.browser_source { return source.get(user).map(|name| vec![name]); }
                return self
                    .browsers
                    .get(&user)
                    .cloned()
                    .map(|browser| vec![browser])
                    .ok_or(NotModelled("DefaultAppProvider browser owner unavailable"));
            }
            8 | 9 | 14 => return Ok(Vec::new()),
            1..=19 => {}
            _ => return Ok(Vec::new()),
        }
        let selected = self
            .known
            .as_ref()
            .ok_or(NotModelled(
                "KnownPackages constructor selections unavailable",
            ))?
            .get(&kind)
            .ok_or(NotModelled("KnownPackages selection unavailable"))?;
        if kind == 16 {
            return Ok(selected.clone());
        }
        Ok(selected
            .iter()
            .filter_map(|name| {
                let name = name.as_ref()?;
                query
                    .state
                    .packages
                    .get(name)
                    .filter(|package| package.pkg.is_some() && package.is.system)
                    .map(|_| Some(name.clone()))
            })
            .collect())
    }
    /// Resource updates change live selections while constructor selections stay frozen.
    pub fn refresh_resources(&self, platform: &Platform, config: Config) -> Result<Self, String> {
        let resources = Resources {
            tables: vec![&platform.framework],
            overlays: &platform.framework_overlays,
            config,
        };
        let mut result = self.clone();
        for (role, name) in [
            (Role::Attention, "config_defaultAttentionService"),
            (
                Role::RotationResolver,
                "config_defaultRotationResolverService",
            ),
            (
                Role::SystemCaptions,
                "config_defaultSystemCaptionsManagerService",
            ),
        ] {
            let value = platform
                .framework
                .id("string", name)
                .and_then(|id| resources.resource_string(id))
                .ok_or_else(|| format!("configured role resource unavailable: {name}"))?;
            result.live.insert(role, component_package(&value));
        }
        Ok(result)
    }
    pub fn package(&self, role: Role, query: &Query<'_>) -> Result<Option<String>, Exception> {
        if let Some(value) = self.frozen.get(&role) {
            return Ok(value.clone());
        }
        if let Some(value) = self.live.get(&role) {
            return ensure(query, value.as_deref());
        }
        Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,
            "configured role owner is unavailable",
        ))
    }
}
fn required_shared_library(state: &State) -> Result<String, String> {
    let libraries = state
        .shared_libraries
        .as_ref()
        .ok_or("native shared library owner unavailable")?;
    let library = libraries
        .iter()
        .find(|library| {
            library.name.as_deref() == Some("android.ext.shared") && library.version == -1
        })
        .ok_or("Missing required shared library:android.ext.shared")?;
    library
        .package_name
        .clone()
        .ok_or_else(|| "Expected a package for shared library android.ext.shared".into())
}
fn component_package(value: &str) -> Option<String> {
    let (package, class) = value.split_once('/')?;
    // ComponentName.unflattenFromString rejects a missing class, not an empty package.
    (!class.is_empty()).then(|| package.to_owned())
}
fn ensure(query: &Query<'_>, package: Option<&str>) -> Result<Option<String>, Exception> {
    let Some(package) = package else {
        return Ok(None);
    };
    let cleared = Query {
        state: query.state,
        filter: query.filter,
        calling_uid: 1000,
    };
    match cleared.package_info(package, -1, MATCH_FACTORY_ONLY, 0) {
        Ok(Ok(Some(_))) => Ok(Some(package.into())),
        Ok(Ok(None)) => Ok(None),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(Exception::new(
            aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION,
            e.0,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        model::{PackageState, PackageUserState, User},
        pkg::{AndroidPackage, booleans},
    };
    fn state() -> Arc<State> {
        Arc::new(State {
            users: [(
                0,
                User {
                    id: 0,
                    unlocking_or_unlocked: true,
                    ..Default::default()
                },
            )]
            .into(),
            packages: [(
                "factory".into(),
                PackageState {
                    name: "factory".into(),
                    app_id: 10100,
                    is: crate::package::model::StateFlags {
                        system: true,
                        ..Default::default()
                    },
                    users: [(0, PackageUserState::default())].into(),
                    pkg: Some(Arc::new(AndroidPackage {
                        package_name: "factory".into(),
                        uid: 10100,
                        booleans: booleans::SYSTEM | booleans::ENABLED,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        })
    }
    #[test]
    fn instant_visibility_requires_permission_and_home_or_prediction_identity() {
        use crate::package::preferred::{self, registry::{Actions, ActionError, Captured, Handle, Identity, IdentityProvider, Registry}};
        struct Identities;
        impl IdentityProvider for Identities {
            fn allocate(&self, _: preferred::records::Record<'_>) -> Result<Identity, String> { panic!("no preferred records") }
        }
        struct Home;
        impl Actions for Home {
            fn default_home(&self, user: i32) -> Result<Option<String>, ActionError> { Ok((user == 0).then(|| "factory".into())) }
            fn context_permission(&self, _: i32, _: &str) -> Result<bool, ActionError> { panic!("unexpected permission mutation") }
            fn commit_after_selection(&self, _: i32, _: &preferred::Mutation, _: i32) -> Result<bool, ActionError> { panic!("unexpected mutation") }
            fn commit_mutation(&self, _: i32, _: u64, _: &preferred::Mutation, _: i32) -> Result<bool, ActionError> { panic!("unexpected mutation") }
            fn cross_access(&self, _: i32, _: i32, _: i32, _: bool) -> Result<i32, ActionError> { panic!("unexpected cross access") }
            fn cross_accessible(&self, _: i32, _: i32, _: i32) -> Result<bool, ActionError> { panic!("unexpected cross access") }
            fn enforce_shell_restriction(&self, _: i32, _: i32) -> Result<(), ActionError> { panic!("unexpected shell restriction") }
            fn reconcile_home(&self, _: i32, _: i32) -> Result<bool, ActionError> { panic!("unexpected reconcile") }
            fn commit_home_selection(&self, _: i32, _: u64, _: &preferred::Selection) -> Result<(), ActionError> { panic!("unexpected selection mutation") }
            fn default_browser(&self, _: i32) -> Result<Option<String>, ActionError> { panic!("unexpected browser") }
            fn restore_browser(&self, _: i32, _: &str, _: bool) -> Result<(), ActionError> { panic!("unexpected browser restore") }
            fn default_preferences(&self, _: i32) -> Result<Vec<preferred::PreferredActivity>, ActionError> { panic!("unexpected defaults") }
        }
        let mut state = (*state()).clone();
        state.system.system_permissions = Some(BTreeMap::new());
        let factory = state.packages.get_mut("factory").unwrap();
        factory.users.insert(10, PackageUserState::default());
        for user in factory.users.values_mut() { user.granted_permissions.push("android.permission.VIEW_INSTANT_APPS".into()); }
        let pkg = Arc::make_mut(factory.pkg.as_mut().unwrap());
        let mut filter = crate::package::intent_filter::IntentFilter::default();
        filter.add_action("android.intent.action.MAIN"); filter.add_category("android.intent.category.HOME"); filter.add_category("android.intent.category.DEFAULT");
        let mut activity = crate::package::pkg::Activity::default();
        activity.main.component.package_name = "factory".into(); activity.main.component.name = "factory.Home".into();
        activity.main.component.intents.push(crate::package::intent_filter::ParsedIntentInfo { filter, has_default: true, ..Default::default() });
        activity.main.enabled = true; activity.main.exported = true; pkg.activities.push(activity);
        let registry = Registry::new(Arc::new(Identities));
        registry.insert_user(0, preferred::Preferred::default()).unwrap(); registry.insert_user(10, preferred::Preferred::default()).unwrap();
        let captured: Captured = registry.capture().unwrap();
        state.system.preferred_owner = Some(Arc::new(Handle::new(Arc::new(captured), Arc::new(|_, _, _| panic!("unexpected commit")), Arc::new(Home))));
        state.system.roles = Some(Arc::new(Owner { frozen: [(Role::AppPrediction, None)].into(), live: BTreeMap::new(), known: None, browsers: BTreeMap::new(), browser_source: None }));
        let query_filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        let query = Query { state: &state, filter: &query_filter, calling_uid: 10100 };
        assert!(query.internal_can_view_instant(10100, 0).unwrap());
        assert!(!query.internal_can_view_instant(10199, 0).unwrap());
        drop(query);
        state.users.clear();
        state.users.insert(10, User { id: 10, unlocking_or_unlocked: true, ..Default::default() });
        state.packages.get_mut("factory").unwrap().users.get_mut(&10).unwrap()
            .granted_permissions.push("android.permission.INTERACT_ACROSS_USERS_FULL".into());
        state.packages.get_mut("factory").unwrap().pkg.as_mut().map(Arc::make_mut).unwrap().activities.clear();
        state.system.roles = Some(Arc::new(Owner { frozen: [(Role::AppPrediction, Some("factory".into()))].into(), live: BTreeMap::new(), known: None, browsers: BTreeMap::new(), browser_source: None }));
        let query_filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        let query = Query { state: &state, filter: &query_filter, calling_uid: 1010100 };
        assert!(query.internal_can_view_instant(1010100, 10).unwrap());
        assert!(!query.internal_can_view_instant(1010199, 10).unwrap());
        let unprivileged = Query { calling_uid: 1010199, ..query };
        assert!(!unprivileged.internal_can_view_instant(1010100, 10).unwrap());
    }

    #[test]
    fn configured_packages_use_factory_owner_and_cleared_identity() {
        let state = state();
        let filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 10199,
        };
        let owner = Owner {
            frozen: [(Role::DefaultTextClassifier, Some("captured".into()))].into(),
            live: [
                (Role::Attention, Some("factory".into())),
                (Role::RotationResolver, Some("missing".into())),
            ]
            .into(),
            known: None,
            browsers: BTreeMap::new(),
            browser_source: None,
        };
        assert_eq!(
            owner
                .package(Role::DefaultTextClassifier, &query)
                .unwrap()
                .as_deref(),
            Some("captured")
        );
        assert_eq!(
            owner.package(Role::Attention, &query).unwrap().as_deref(),
            Some("factory")
        );
        assert_eq!(owner.package(Role::RotationResolver, &query).unwrap(), None);
        let mut removed = (*state).clone();
        removed.packages.get_mut("factory").unwrap().is.system = false;
        let filter = AppsFilter::new(&removed, &FilterConfig::default()).unwrap();
        let query = Query {
            state: &removed,
            filter: &filter,
            calling_uid: 1000,
        };
        assert_eq!(owner.package(Role::Attention, &query).unwrap(), None);
    }
    #[test]
    fn role_dispatch_enforces_exact_system_uid_and_requires_owner() {
        use aim_binder_host::parcel::{EX_SECURITY, Parcel, Reader};
        use aim_service_aidl::android_content_pm_ipackagemanager as pm;
        let mut state = (*state()).clone();
        let filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let mut request = Parcel::new();
        pm::GetSetupWizardPackageName {}.write(&mut request);
        assert!(
            query
                .answer(
                    pm::DESCRIPTOR,
                    pm::GET_ATTENTION_SERVICE_PACKAGE_NAME,
                    &mut Reader::new(request.data(), request.objects())
                )
                .is_err()
        );
        state.system.roles = Some(Arc::new(Owner {
            frozen: [(Role::SetupWizard, Some("factory".into()))].into(),
            live: BTreeMap::new(),
            known: None,
            browsers: BTreeMap::new(),
            browser_source: None,
        }));
        let filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        for uid in [0, 10100, 101000] {
            let query = Query {
                state: &state,
                filter: &filter,
                calling_uid: uid,
            };
            let reply = query
                .answer(
                    pm::DESCRIPTOR,
                    pm::GET_SETUP_WIZARD_PACKAGE_NAME,
                    &mut Reader::new(request.data(), request.objects()),
                )
                .unwrap();
            let error = Reader::new(reply.data(), reply.objects())
                .read_exception()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.code, EX_SECURITY);
        }
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let reply = query
            .answer(
                pm::DESCRIPTOR,
                pm::GET_SETUP_WIZARD_PACKAGE_NAME,
                &mut Reader::new(request.data(), request.objects()),
            )
            .unwrap();
        let value = pm::read_get_setup_wizard_package_name_reply(&mut Reader::new(
            reply.data(),
            reply.objects(),
        ))
        .unwrap();
        assert_eq!(value.unwrap().as_deref(), Some("factory"));
    }
    #[test]
    fn required_system_shared_library_uses_finalized_owner_package() {
        let mut state = (*state()).clone();
        assert!(required_shared_library(&state).is_err());
        state.shared_libraries = Some(vec![crate::package::model::SharedLibrary {
            name: Some("android.ext.shared".into()),
            version: 7,
            package_name: Some("wrong.version".into()),
            ..Default::default()
        }]);
        assert!(required_shared_library(&state).is_err());
        state
            .shared_libraries
            .as_mut()
            .unwrap()
            .push(crate::package::model::SharedLibrary {
                name: Some("android.ext.shared".into()),
                version: -1,
                package_name: Some("actual.owner".into()),
                ..Default::default()
            });
        assert_eq!(required_shared_library(&state).unwrap(), "actual.owner");
    }
    #[test]
    fn flattened_components_match_framework_rejection_rules() {
        assert_eq!(component_package("pkg/.Service").as_deref(), Some("pkg"));
        assert_eq!(component_package("/Service").as_deref(), Some(""));
        assert_eq!(component_package("pkg/"), None);
        assert_eq!(component_package("pkg"), None);
        assert_eq!(component_package(""), None);
    }
}
