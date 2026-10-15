//! PMS constructor known-package selections before PermissionManager attaches.
//! Port of KnownPackages/PMS android-16.0.0_r1, AOSP Apache-2.0.
use super::*;
use crate::package::{component_resolver::{ComponentResolver, Kind, Results}, info::flags::*, intent::Intent, parse::resources::Resources, uri::Uri};
use sha2::{Digest, Sha256};

impl RawScanned {
    pub fn known_package_names(&self, kind: i32, user: i32) -> Result<Vec<Option<String>>> {
        let guard = self.projection.lock().unwrap();
        let raw = guard.as_ref().ok_or_else(|| failure("initial role scan already consumed"))?;
        if kind == 0 { return Ok(vec![Some("android".into())]); }
        if !matches!(kind, 1..=7 | 10..=13 | 15..=19) { return Ok(Vec::new()); }

        if kind == 5 {
            return raw.original.users.iter().find(|record| record.id == user)
                .map(|record| vec![record.default_browser.clone()])
                .ok_or_else(|| failure("initial browser UserManager owner absent"));
        }
        // This metadata index is never made into a permission-queryable Capture.
        let mut state = self.visibility.metadata_state().clone();
        state.package_registry = Some(Arc::new(raw.owner.package_registry().map_err(failure)?.clone()));
        for setting in &raw.owner.settings.packages {
            let package = state.packages.get_mut(&setting.name).ok_or_else(|| failure("initial role setting absent from metadata"))?;
            package.is.privileged = setting.private_flags & crate::package::settings::PRIVATE_FLAG_PRIVILEGED != 0;
            package.is.scanned_as_stopped_system_app = setting.scanned_as_stopped_system_app;
            package.mime_groups = setting.mime_groups.clone();
            let stored = raw.owner.scanned_user_states(&setting.name).ok_or_else(|| failure("initial role user owner absent"))?;
            for (id, user_state) in &mut package.users {
                user_state.stopped = stored.get(id).map_or(false, |source| source.stopped);
            }
        }
        let resources = Resources { tables: vec![&raw.platform.framework], overlays: &raw.platform.framework_overlays, config: raw.resource_config };
        let text = |name: &str| -> Result<String> {
            raw.platform.framework.id("string", name).and_then(|id| resources.resource_string(id))
                .ok_or_else(|| failure(format!("initial known-package resource unavailable: {name}")))
        };
        let factory = |name: Option<String>| -> Option<String> {
            let name = name?;
            if name.is_empty() { return None; }
            if raw.owner.settings.disabled_system_packages.iter().any(|setting| setting.name == name) {
                return raw.owner.disabled_loaded_packages().contains_key(&name).then_some(name);
            }
            state.packages.get(&name).filter(|package| package.is.system && package.pkg.is_some()).map(|_| name)
        };
        let configured = |name: &str, component: bool| -> Result<Option<String>> {
            let value = text(name)?;
            let package = if component {
                value.split_once('/').filter(|(_, class)| !class.is_empty()).map(|(name, _)| name.to_owned())
            } else { Some(value) };
            Ok(factory(package))
        };
        let flags = MATCH_SYSTEM_ONLY | MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE;
        let query = |kind: Kind, intent: Intent, ty: Option<&str>, flags: i64| -> Result<Vec<crate::package::component_resolver::ResolveInfo>> {
            let resolver = ComponentResolver::new(&state).map_err(|error| failure(format!("initial role component registry: {error:?}")))?;
            resolver.query(kind, Results {state: &state, user: 0, flags}, &intent, ty, None)
                .map(|results| results.unwrap_or_default()).map_err(|error| failure(format!("initial role intent: {error:?}")))
        };
        let activity = |action: &str, categories: Vec<String>, data: Option<Uri>, ty: Option<&str>, flags: i64| {
            query(Kind::Activity, Intent {action: Some(action.into()), categories: Some(categories), data, ty: ty.map(str::to_owned), ..Default::default()}, ty, flags)
        };
        let privileged_unique = |matches: Vec<crate::package::component_resolver::ResolveInfo>, role: &str| -> Result<Vec<Option<String>>> {
            if matches.len() != 1 { return Err(failure(format!("There must be exactly one {role}; found {}", matches.len()))); }
            let name = matches[0].component().0;
            if !state.packages.get(name).is_some_and(|package| package.is.privileged) { return Err(failure(format!("The {role} must be a privileged app"))); }
            Ok(vec![Some(name.into())])
        };
        let selected = match kind {
            1 => {
                let matches = activity("android.intent.action.MAIN", vec!["android.intent.category.SETUP_WIZARD".into()], None, None, flags | MATCH_DISABLED_COMPONENTS)?;
                vec![if matches.len() == 1 { Some(matches[0].component().0.into()) } else { None }]
            }
            2 => privileged_unique(activity("android.intent.action.INSTALL_PACKAGE", vec!["android.intent.category.DEFAULT".into()], Some(Uri::parse("content://com.example/foo.apk")), Some("application/vnd.android.package-archive"), flags)?, "installer")?,
            3 => {
                let matches = activity("android.intent.action.UNINSTALL_PACKAGE", vec!["android.intent.category.DEFAULT".into()], Some(Uri::preferred_opaque("package", "foo.bar")), None, flags)?;
                let first = matches.first().ok_or_else(|| failure("There must be exactly one uninstaller"))?;
                // chooseBestActivity returns the first for distinct priority/order/default.
                if matches.get(1).is_some_and(|next| first.priority == next.priority && first.preferred_order == next.preferred_order && first.is_default == next.is_default) {
                    return Err(failure("initial uninstaller requires preferred-activity owner"));
                }
                vec![Some(first.component().0.into())]
            }
            4 => {
                let matches = query(Kind::Receiver, Intent {action: Some("android.intent.action.PACKAGE_NEEDS_VERIFICATION".into()), ..Default::default()}, Some("application/vnd.android.package-archive"), flags)?;
                if matches.len() > 2 { return Err(failure("There must be no more than 2 verifiers")); }
                matches.iter().map(|result| Some(result.component().0.into())).collect()
            }
            6 => vec![configured("config_servicesExtensionPackage", false)?, configured("config_defaultTextClassifierPackage", false)?],
            7 => privileged_unique(activity("android.intent.action.MANAGE_PERMISSIONS", vec!["android.intent.category.DEFAULT".into()], None, None, flags)?, "permissions manager")?,
            10 => vec![configured("config_deviceConfiguratorPackageName", false)?],
            11 => vec![configured("config_incidentReportApproverPackage", false)?],
            12 => vec![configured("config_defaultAppPredictionService", true)?],
            13 => return Err(failure("initial overlay signature selection owner is not attached")),
            15 => vec![Some("com.android.companiondevicemanager".into())],
            16 => {
                let name = text("config_retailDemoPackage")?;
                let expected = text("config_retailDemoPackageSignature")?;
                if name.is_empty() || expected.is_empty() { return Ok(Vec::new()); }
                let matched = raw.owner.loaded_packages().get(&name).and_then(|code| code.runtime_package().signing_details.as_ref())
                    .and_then(|details| details.signatures.as_ref()).is_some_and(|signatures| signatures.iter().any(|signature| {
                        let hash = Sha256::digest(signature);
                        let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
                        hex == expected
                    }));
                return Ok(if matched {vec![Some(name)]} else {Vec::new()});
            }
            17 => vec![configured("config_recentsComponentName", true)?],
            18 => vec![configured("config_defaultAmbientContextDetectionService", true)?],
            19 => vec![configured("config_defaultWearableSensingService", true)?],
            _ => Vec::new(),
        };
        // Computer.filterOnlySystemPackages reads loaded code and system status;
        // it does not perform a permission check or filter by the requested user.
        Ok(selected.into_iter().filter(|name| name.as_ref().is_some_and(|name| state.packages.get(name).is_some_and(|package| package.is.system && package.pkg.is_some()))).collect())
    }
}
