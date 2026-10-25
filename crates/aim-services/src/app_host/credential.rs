//! `credential`: CredentialManagerService for the app host
//! (`android-16.0.0_r1`, ADR 0013), which SystemServer starts with
//! FEATURE_CREDENTIALS (declared in the image) unless DeviceConfig turns
//! `credential/enable_credential_manager` off: whether the service is
//! enabled, the providers the user enabled (`Settings.Secure`
//! `credential_service`) and made primary (`credential_service_primary`),
//! and the credential providers of the installed packages as
//! `CredentialProviderInfoFactory` describes them, the user's (services of
//! `CredentialProviderService.SERVICE_INTERFACE`) and the system's (of
//! `SYSTEM_SERVICE_INTERFACE`, in system apps holding
//! PROVIDE_DEFAULT_ENABLED_CREDENTIAL_SERVICE), with their capabilities
//! and settings from their `android.credentials.provider` meta-data. With
//! no device or profile owner there is no credential manager policy
//! (`device_policy`). The request sessions and the description registry
//! are not implemented (#1940).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::Arc;

use aim_apps::res::{Element, Value as ResValue};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
};
use aim_service_aidl::android_credentials_icredentialmanager as cm;
use aim_service_aidl::android_credentials_isetenabledproviderscallback as enabled_callback;
use aim_service_aidl::{WriteParcelable, write_typed};

use super::settings_provider::{Caller, SettingsProvider};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;
use crate::package::component_resolver::Info;
use crate::package::info::ServiceInfo;
use crate::package::info::flags::{GET_META_DATA, MATCH_SYSTEM_ONLY};
use crate::package::intent::{ComponentName, Intent};
use crate::package::pkg::Value as MetaValue;
use crate::package::preferred::unflatten;
use crate::package::query::Query;

/// `CredentialProviderService.SERVICE_INTERFACE`,
/// `SYSTEM_SERVICE_INTERFACE`, `SERVICE_META_DATA`,
/// `TEST_SYSTEM_PROVIDER_META_DATA_KEY`.
const SERVICE_INTERFACE: &str = "android.service.credentials.CredentialProviderService";
const SYSTEM_SERVICE_INTERFACE: &str =
    "android.service.credentials.system.CredentialProviderService";
const SERVICE_META_DATA: &str = "android.credentials.provider";
const TEST_SYSTEM_PROVIDER_META_DATA_KEY: &str = "android.credentials.testsystemprovider";
/// `Settings.Secure.CREDENTIAL_SERVICE`, `CREDENTIAL_SERVICE_PRIMARY`.
const CREDENTIAL_SERVICE: &str = "credential_service";
const CREDENTIAL_SERVICE_PRIMARY: &str = "credential_service_primary";
/// `DeviceConfig.NAMESPACE_CREDENTIAL` and its flags.
const NAMESPACE_CREDENTIAL: &str = "credential";
const ENABLE_CREDENTIAL_MANAGER: &str = "enable_credential_manager";
const ENABLE_CREDENTIAL_DESC_API: &str = "enable_credential_description_api";
/// `CredentialManager.PROVIDER_FILTER_*`.
const PROVIDER_FILTER_ALL_PROVIDERS: i32 = 0;
const PROVIDER_FILTER_SYSTEM_PROVIDERS_ONLY: i32 = 1;
const PROVIDER_FILTER_USER_PROVIDERS_ONLY: i32 = 2;
const PROVIDER_FILTER_USER_PROVIDERS_INCLUDING_HIDDEN: i32 = 3;
const WRITE_SECURE_SETTINGS: &str = "android.permission.WRITE_SECURE_SETTINGS";
const QUERY_ALL_PACKAGES: &str = "android.permission.QUERY_ALL_PACKAGES";
const LIST_ENABLED_CREDENTIAL_PROVIDERS: &str =
    "android.permission.LIST_ENABLED_CREDENTIAL_PROVIDERS";
const BIND_CREDENTIAL_PROVIDER_SERVICE: &str =
    "android.permission.BIND_CREDENTIAL_PROVIDER_SERVICE";
const PROVIDE_DEFAULT_ENABLED_CREDENTIAL_SERVICE: &str =
    "android.permission.PROVIDE_DEFAULT_ENABLED_CREDENTIAL_SERVICE";
/// `CredentialManagerService.PERMISSION_DENIED_ERROR` and its message.
const PERMISSION_DENIED_ERROR: &str = "permission_denied";
const PERMISSION_DENIED_WRITE_SECURE_SETTINGS_ERROR: &str =
    "Caller is missing WRITE_SECURE_SETTINGS permission";
const SYSTEM_UID: i32 = 1000;
/// `UserHandle.USER_CURRENT`.
const USER_CURRENT: i32 = -2;
/// `new HashMap<>()`'s and `new HashSet<>()`'s first table.
const DEFAULT_CAPACITY: usize = 16;

/// `CredentialProviderInfo`, of a `ServiceInfo`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderInfo<S = ServiceInfo> {
    service: S,
    /// The service's flattened component.
    component: String,
    system: bool,
    capabilities: Vec<String>,
    enabled: bool,
    primary: bool,
    settings_subtitle: Option<String>,
    settings_activity: Option<String>,
}

impl<S: WriteParcelable> WriteParcelable for ProviderInfo<S> {
    fn write_to(&self, p: &mut Parcel) {
        write_typed(p, Some(&self.service));
        p.write_i32(self.system as i32);
        // `writeStringList`.
        p.write_i32(self.capabilities.len() as i32);
        for c in &self.capabilities {
            p.write_string16(Some(c));
        }
        p.write_i32(self.enabled as i32);
        p.write_i32(self.primary as i32);
        // The override label, set only for tests.
        crate::clip::write_char_sequence(p, None);
        crate::clip::write_char_sequence(p, self.settings_subtitle.as_deref());
        crate::clip::write_char_sequence(p, self.settings_activity.as_deref());
    }
}

/// A provider's capabilities, settings subtitle and settings activity.
type Metadata = (Vec<String>, Option<String>, Option<String>);

/// `ComponentName.flattenToString`.
fn flatten(c: &ComponentName) -> String {
    format!("{}/{}", c.package, c.class)
}

/// `ComponentInfo.getComponentName`.
fn component(si: &ServiceInfo) -> ComponentName {
    ComponentName {
        package: si.info.item.package_name.clone().unwrap_or_default(),
        class: si.info.item.name.clone().unwrap_or_default(),
    }
}

/// The component names of a colon-separated setting, those that
/// unflatten (`getEnabledProvidersForUser`, `readServiceNameList`).
fn component_names(value: Option<&str>) -> Vec<ComponentName> {
    value
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .filter_map(unflatten)
        .collect()
}

/// `XmlPullParser.getAttributeValue(null, name)`: the attribute of no
/// namespace, as `XmlBlock` coerces a value that is not a string.
fn attribute_value(e: &Element, name: &str) -> Option<String> {
    let a = e.attrs.iter().find(|a| a.ns.is_empty() && a.name == name)?;
    match &a.value {
        ResValue::String(s) => Some(s.clone()),
        ResValue::Ref(id) => Some(format!("@{id}")),
        ResValue::Attr(id) => Some(format!("?{id}")),
        ResValue::Int(i) => Some(i.to_string()),
        ResValue::Bool(b) => Some(b.to_string()),
        ResValue::Float(f) => Some(f.to_string()),
        ResValue::Color(c) => Some(format!("#{c:08x}")),
        ResValue::Null | ResValue::Dimension(_) | ResValue::Fraction(_) => None,
    }
}

/// `parseXmlProviderOuterCapabilities`: of each `<capabilities>` below
/// `<credential-provider>`, the names of the `<capability>`s within it.
fn capabilities(root: &Element) -> Vec<String> {
    fn inner(e: &Element, out: &mut Vec<String>) {
        for c in &e.children {
            if c.name == "capability"
                && let Some(name) = attribute_value(c, "name").filter(|n| !n.is_empty())
            {
                out.push(name);
            }
            inner(c, out);
        }
    }
    fn outer(e: &Element, out: &mut Vec<String>) {
        for c in &e.children {
            if c.name == "capabilities" {
                inner(c, out);
            } else {
                outer(c, out);
            }
        }
    }
    let mut out = Vec::new();
    outer(root, &mut out);
    out
}

/// `ProviderGenerator`: the providers the filter keeps, keyed by their
/// flattened component in a `HashMap`, and in its order.
fn generate<S>(
    user_providers: Vec<ProviderInfo<S>>,
    system_providers: Vec<ProviderInfo<S>>,
    filter: i32,
) -> Vec<ProviderInfo<S>> {
    let allowed = |p: &ProviderInfo<S>| match filter {
        PROVIDER_FILTER_ALL_PROVIDERS => true,
        _ if p.system => filter == PROVIDER_FILTER_SYSTEM_PROVIDERS_ONLY,
        _ => matches!(
            filter,
            PROVIDER_FILTER_USER_PROVIDERS_ONLY | PROVIDER_FILTER_USER_PROVIDERS_INCLUDING_HIDDEN
        ),
    };
    let mut services: Vec<(String, ProviderInfo<S>)> = Vec::new();
    let users = user_providers.into_iter().filter(|p| !p.system);
    let systems = system_providers.into_iter().filter(|p| p.system);
    for p in users.chain(systems).filter(allowed) {
        let key = p.component.clone();
        match services.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = p,
            None => services.push((key, p)),
        }
    }
    super::device_config::hash_map_order(services, DEFAULT_CAPACITY)
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

/// `String.join(":", new HashSet<>(providers))` after `addAll(primary)`:
/// each once, in the set's order (a null as `null`).
fn join_enabled(providers: &[Option<String>], primary: &[Option<String>]) -> String {
    // `HashSet(Collection)`: room for the collection, at least 16.
    let capacity = ((providers.len() as f32 / 0.75) as usize + 1).max(DEFAULT_CAPACITY);
    let mut set: Vec<(String, ())> = Vec::new();
    let mut add = |p: &Option<String>| {
        let s = p.clone().unwrap_or_else(|| "null".into());
        if !set.iter().any(|(k, _)| *k == s) {
            set.push((s, ()));
        }
    };
    providers.iter().chain(primary).for_each(&mut add);
    super::device_config::hash_map_order(set, super::device_config::sized_capacity(capacity))
        .into_iter()
        .map(|(k, _)| k)
        .collect::<Vec<_>>()
        .join(":")
}

pub struct CredentialManager {
    host: Arc<AppHost>,
    settings: Arc<SettingsProvider>,
}

impl CredentialManager {
    pub fn new(host: Arc<AppHost>, settings: Arc<SettingsProvider>) -> Self {
        CredentialManager { host, settings }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("credential", &format!("{what} (#1940)"))
    }

    fn permitted(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        let granted = self.host.check_permission(Some(permission), uid)? == 0;
        if !granted {
            eprintln!("app host: credential: Caller does not have permission: {permission}");
        }
        Ok(granted)
    }

    /// `DeviceConfig.getBoolean(NAMESPACE_CREDENTIAL, name, default)`.
    fn flag(&self, name: &str, default: bool) -> Result<bool, Exception> {
        Ok(self
            .host
            .inputs()?
            .settings
            .device_config(NAMESPACE_CREDENTIAL, name)
            .map_or(default, |v| v.eq_ignore_ascii_case("true")))
    }

    /// `ActivityManager.handleIncomingUser(pid, uid, user, false, false,
    /// ...)`: no special user.
    fn incoming_user(&self, uid: i32, user: i32) -> Result<i32, Exception> {
        let user = if user == USER_CURRENT { 0 } else { user };
        if user < 0 {
            return Err(Exception::illegal_argument(format!(
                "Call does not support special user #{user}"
            )));
        }
        self.host.incoming_user(uid, user, true)
    }

    /// `Settings.Secure.getStringForUser`.
    fn secure(&self, name: &str, user: i32) -> Result<Option<String>, Exception> {
        let caller = Caller {
            uid: SYSTEM_UID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([("_user".to_string(), Value::Int(user))]);
        let got = self
            .settings
            .call(&caller, "GET_secure", Some(name), &extras, None)?;
        Ok(match got.as_deref() {
            Some([(_, Value::String(value)), ..]) => value.clone(),
            _ => None,
        })
    }

    /// `Settings.Secure.putStringForUser`, as the system once the
    /// caller's WRITE_SECURE_SETTINGS is checked.
    fn put_secure(&self, name: &str, value: &str, user: i32) -> Result<(), Exception> {
        let caller = Caller {
            uid: SYSTEM_UID,
            package: Some("android".into()),
        };
        let extras = HashMap::from([
            ("_user".to_string(), Value::Int(user)),
            ("value".to_string(), Value::String(Some(value.to_string()))),
        ]);
        self.settings
            .call(&caller, "PUT_secure", Some(name), &extras, None)
            .map(|_| ())
    }

    /// `getEnabledProvidersForUser`.
    fn enabled_providers(&self, uid: i32, user: i32) -> Result<Vec<ComponentName>, Exception> {
        let user = self.incoming_user(uid, user)?;
        Ok(component_names(
            self.secure(CREDENTIAL_SERVICE, user)?.as_deref(),
        ))
    }

    /// `getPrimaryProvidersForUserId`.
    fn primary_providers(&self, uid: i32, user: i32) -> Result<Vec<ComponentName>, Exception> {
        let user = self.incoming_user(uid, user)?;
        Ok(component_names(
            self.secure(CREDENTIAL_SERVICE_PRIMARY, user)?.as_deref(),
        ))
    }

    /// `enforceCallingPackage`.
    fn enforce_calling_package(&self, package: Option<&str>, uid: i32) -> Result<(), Exception> {
        let package = package.unwrap_or("null");
        let package_uid = self
            .host
            .package_uid(package, 0, uid / PER_USER_RANGE as i32)?;
        if package_uid < 0 {
            return Err(Exception::security(format!("{package} not found")));
        }
        if package_uid != uid {
            return Err(Exception::security(format!(
                "{package} does not belong to uid {uid}"
            )));
        }
        Ok(())
    }

    /// `getDeviceManagerPolicy`: with no device or profile owner, none.
    fn no_policy(&self) -> Result<(), Exception> {
        if self.host.device_policies()?.owned() {
            return Err(self.host.unimplemented_named(
                "credential",
                "the credential manager policy of a device or profile owner (#1521)",
            ));
        }
        Ok(())
    }

    /// The services of `action` `uid` sees in `user` (`queryIntentServicesAsUser`
    /// with GET_META_DATA).
    fn services(&self, action: &str, user: i32, uid: i32) -> Result<Vec<ServiceInfo>, Exception> {
        let (_, resolution) = self.host.live_packages()?;
        let intent = Intent {
            action: Some(action.into()),
            ..Default::default()
        };
        let found = resolution
            .query_intent_services(&intent, None, GET_META_DATA, user, uid)
            .map_err(|e| {
                Exception::new(
                    EX_UNSUPPORTED_OPERATION,
                    format!("queryIntentServices: {e:?}"),
                )
            })?;
        Ok(found
            .into_iter()
            .filter_map(|r| match r.info {
                Info::Service(si) => Some(si),
                _ => None,
            })
            .collect())
    }

    /// `CredentialProviderInfoFactory.create`: the provider of `si`, or
    /// why it is none (a SecurityException's message).
    fn provider(
        &self,
        si: ServiceInfo,
        system: bool,
        testing: bool,
        enabled: bool,
        primary: bool,
    ) -> Result<Result<ProviderInfo, String>, Exception> {
        // `verifyProviderPermission`.
        if si.permission.as_deref() != Some(BIND_CREDENTIAL_PROVIDER_SERVICE) {
            return Ok(Err(format!(
                "Service does not require the expected permission : {BIND_CREDENTIAL_PROVIDER_SERVICE}"
            )));
        }
        // `isValidSystemProvider`.
        if system {
            let valid = if testing {
                matches!(
                    meta_data(&si, TEST_SYSTEM_PROVIDER_META_DATA_KEY),
                    Some(MetaValue::Bool(true))
                )
            } else {
                // `PermissionUtils.hasPermission` of system_server's
                // context, user 0's.
                let package = si.info.item.package_name.clone().unwrap_or_default();
                self.host.check_package_permission(
                    PROVIDE_DEFAULT_ENABLED_CREDENTIAL_SERVICE,
                    &package,
                    0,
                )?
            };
            if !valid {
                return Ok(Err(format!(
                    "Provider is not a valid system provider: {}",
                    flatten(&component(&si))
                )));
            }
        }
        let (capabilities, settings_subtitle, settings_activity) = self.metadata(&si)?;
        Ok(Ok(ProviderInfo {
            component: flatten(&component(&si)),
            service: si,
            system,
            capabilities,
            enabled,
            primary,
            settings_subtitle,
            settings_activity,
        }))
    }

    /// `populateMetadata`: the capabilities, settings subtitle and
    /// settings activity the `<credential-provider>` of the service's
    /// meta-data names, or none of them.
    fn metadata(&self, si: &ServiceInfo) -> Result<Metadata, Exception> {
        let component = flatten(&component(si));
        if si.info.item.meta_data.is_none() {
            eprintln!("app host: credential: Metadata is null for provider: {component}");
            return Ok(Default::default());
        }
        // `loadXmlMetaData`: the meta-data's resource, else none.
        let Some(id) = (match meta_data(si, SERVICE_META_DATA) {
            Some(MetaValue::Int(id)) if *id != 0 => Some(*id as u32),
            _ => None,
        }) else {
            return Ok(Default::default());
        };
        let (state, _) = self.host.live_packages()?;
        let package = si.info.item.package_name.clone().unwrap_or_default();
        let Some(pkg) = state.packages.get(&package).and_then(|ps| ps.pkg.clone()) else {
            eprintln!(
                "app host: credential: Resources are null for the serviceInfo being processed: {component}"
            );
            return Ok(Default::default());
        };
        let inputs = self.host.platform()?;
        let parsed = self.host.framework()?
            .xml_resource(self.host.root()?, &inputs.data, &pkg, id, |e, ctx| {
                if e.name != "credential-provider" {
                    eprintln!(
                        "app host: credential: Meta-data does not start with credential-provider-service tag"
                    );
                    return Default::default();
                }
                let a = ctx.obtain(e);
                (
                    capabilities(e),
                    a.string("settingsSubtitle"),
                    a.string("settingsActivity"),
                )
            });
        Ok(parsed.unwrap_or_else(|e| {
            eprintln!("app host: credential: Failed to get XML metadata: {e}");
            Default::default()
        }))
    }

    /// `getUserProviders`.
    fn user_providers(
        &self,
        uid: i32,
        user: i32,
        testing: bool,
        enabled: &[ComponentName],
        primary: &[ComponentName],
    ) -> Result<Vec<ProviderInfo>, Exception> {
        let mut providers = Vec::new();
        for si in self.services(SERVICE_INTERFACE, user, uid)? {
            let component = component(&si);
            let (on, first) = (enabled.contains(&component), primary.contains(&component));
            match self.provider(si, false, testing, on, first)? {
                Ok(p) => providers.push(p),
                Err(e) => eprintln!(
                    "app host: credential: Error getting info for {}: {e}",
                    flatten(&component)
                ),
            }
        }
        Ok(providers)
    }

    /// `getAvailableSystemServices`.
    fn system_providers(
        &self,
        uid: i32,
        user: i32,
        testing: bool,
        enabled: &[ComponentName],
    ) -> Result<Vec<ProviderInfo>, Exception> {
        let mut providers = Vec::new();
        for si in self.services(SYSTEM_SERVICE_INTERFACE, user, uid)? {
            let component = component(&si);
            // `getApplicationInfo(MATCH_SYSTEM_ONLY)` of the context's user.
            if !testing && !self.system_app(&component.package, uid)? {
                eprintln!(
                    "app host: credential: Error getting info for {}: NameNotFoundException",
                    flatten(&component)
                );
                continue;
            }
            let on = enabled.contains(&component);
            match self.provider(si, true, testing, on, false)? {
                Ok(p) => providers.push(p),
                Err(e) => eprintln!(
                    "app host: credential: Failed to create CredentialProviderInfo: java.lang.SecurityException: {e}"
                ),
            }
        }
        Ok(providers)
    }

    /// Whether `package` is a system app `uid` sees in user 0.
    fn system_app(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        let (state, resolution) = self.host.live_packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let info = query
            .application_info(package, MATCH_SYSTEM_ONLY, 0)
            .map_err(|e| {
                Exception::new(
                    EX_UNSUPPORTED_OPERATION,
                    format!("getApplicationInfo: {}", e.0),
                )
            })??;
        Ok(info.is_some())
    }

    /// `CredentialProviderInfoFactory.getCredentialProviderServices` (or,
    /// `testing`, `...ForTesting`).
    fn providers(
        &self,
        uid: i32,
        user: i32,
        filter: i32,
        testing: bool,
    ) -> Result<Vec<ProviderInfo>, Exception> {
        let enabled = self.enabled_providers(uid, user)?;
        let primary = self.primary_providers(uid, user)?;
        if filter != PROVIDER_FILTER_USER_PROVIDERS_INCLUDING_HIDDEN {
            self.no_policy()?;
        }
        let users = self.user_providers(uid, user, testing, &enabled, &primary)?;
        let systems = self.system_providers(uid, user, testing, &enabled)?;
        Ok(generate(users, systems, filter))
    }

    /// `verifyGetProvidersPermission`.
    fn verify_get_providers_permission(&self, uid: i32) -> Result<(), Exception> {
        if self.permitted(QUERY_ALL_PACKAGES, uid)?
            || self.permitted(LIST_ENABLED_CREDENTIAL_PROVIDERS, uid)?
        {
            return Ok(());
        }
        Err(Exception::security(
            "Caller is missing permission: QUERY_ALL_PACKAGES or LIST_ENABLED_CREDENTIAL_PROVIDERS",
        ))
    }

    /// `setEnabledProviders`: the answer goes to `callback`.
    fn set_enabled_providers(&self, uid: i32, a: cm::SetEnabledProviders) -> Result<(), Exception> {
        let callback = match a.callback {
            Some(Binder::Handle(h)) => self.host.process.strong(h),
            _ => {
                return Err(Exception::new(
                    EX_NULL_POINTER,
                    "Attempt to invoke interface method 'void android.credentials.ISetEnabledProvidersCallback.onResponse()' on a null object reference",
                ));
            }
        };
        let error = |kind: &str, message: &str| {
            let mut data = Parcel::new();
            enabled_callback::OnError {
                error_type: Some(kind.into()),
                message: Some(message.into()),
            }
            .write(&mut data);
            if let Err(s) = callback.transact(enabled_callback::ON_ERROR, &data, true) {
                eprintln!("app host: credential: Issue with invoking response: {s}");
            }
        };
        if !self.permitted(WRITE_SECURE_SETTINGS, uid)? {
            error(
                PERMISSION_DENIED_ERROR,
                PERMISSION_DENIED_WRITE_SECURE_SETTINGS_ERROR,
            );
            return Ok(());
        }
        let user = self.incoming_user(uid, a.user_id)?;
        let providers = a.providers.unwrap_or_default();
        let primary = a.primary_providers.unwrap_or_default();
        let enabled = join_enabled(&providers, &primary);
        let primary = primary
            .iter()
            .map(|p| p.as_deref().unwrap_or("null"))
            .collect::<Vec<_>>()
            .join(":");
        let wrote_enabled = self.put_secure(CREDENTIAL_SERVICE, &enabled, user);
        let wrote_primary = self.put_secure(CREDENTIAL_SERVICE_PRIMARY, &primary, user);
        if let Some(e) = wrote_enabled.err().or(wrote_primary.err()) {
            eprintln!(
                "app host: credential: Failed to store setting containing enabled or primary providers: {}",
                e.message
            );
            error(
                "failed_setting_store",
                "Failed to store setting containing enabled or primary providers",
            );
        }
        // The original answers the response after an error too.
        if let Err(s) = callback.transact(enabled_callback::ON_RESPONSE, &Parcel::new(), true) {
            eprintln!("app host: credential: Issue with invoking response: {s}");
        }
        Ok(())
    }
}

/// The value of the service's meta-data `key`.
fn meta_data<'a>(si: &'a ServiceInfo, key: &str) -> Option<&'a MetaValue> {
    si.info
        .item
        .meta_data
        .as_ref()
        .and_then(|m| m.0.iter().find(|(k, _)| k == key))
        .map(|(_, v)| v)
}

impl Service for CredentialManager {
    fn descriptor(&self) -> &str {
        cm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let user = uid / PER_USER_RANGE as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            cm::IS_SERVICE_ENABLED => {
                cm::IsServiceEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.flag(ENABLE_CREDENTIAL_MANAGER, true)
                    .map(|v| cm::write_is_service_enabled_reply(&mut p, v))
            }
            cm::IS_ENABLED_CREDENTIAL_PROVIDER_SERVICE => {
                let a =
                    cm::IsEnabledCredentialProviderService::<ComponentName>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce_calling_package(a.calling_package.as_deref(), uid)?;
                    let Some(component) = a.component_name else {
                        eprintln!(
                            "app host: credential: isEnabledCredentialProviderService componentName is null"
                        );
                        return Ok(false);
                    };
                    if a.calling_package.as_deref() != Some(component.package.as_str()) {
                        return Err(Exception::illegal_argument(
                            "provided component name does not match does not match requesting component",
                        ));
                    }
                    Ok(self.enabled_providers(uid, user)?.contains(&component))
                })()
                .map(|v| cm::write_is_enabled_credential_provider_service_reply(&mut p, v))
            }
            cm::GET_CREDENTIAL_PROVIDER_SERVICES => {
                let a = cm::GetCredentialProviderServices::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.verify_get_providers_permission(uid)
                    .and_then(|()| self.providers(uid, a.user_id, a.provider_filter, false))
                    .map(|providers| {
                        let providers: Vec<_> = providers.into_iter().map(Some).collect();
                        cm::write_get_credential_provider_services_reply(&mut p, Some(&providers))
                    })
            }
            cm::GET_CREDENTIAL_PROVIDER_SERVICES_FOR_TESTING => {
                let a = cm::GetCredentialProviderServicesForTesting::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.verify_get_providers_permission(uid)
                    .and_then(|()| self.providers(uid, user, a.provider_filter, true))
                    .map(|providers| {
                        let providers: Vec<_> = providers.into_iter().map(Some).collect();
                        cm::write_get_credential_provider_services_for_testing_reply(
                            &mut p,
                            Some(&providers),
                        )
                    })
            }
            cm::SET_ENABLED_PROVIDERS => {
                let a = cm::SetEnabledProviders::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_enabled_providers(uid, a)
                    .map(|()| cm::write_set_enabled_providers_reply(&mut p))
            }
            // `isCredentialDescriptionApiEnabled` comes first; the
            // request's entries hold slices.
            cm::REGISTER_CREDENTIAL_DESCRIPTION | cm::UNREGISTER_CREDENTIAL_DESCRIPTION => {
                match self.flag(ENABLE_CREDENTIAL_DESC_API, false) {
                    Ok(false) => Err(Exception::new(
                        EX_UNSUPPORTED_OPERATION,
                        "Feature not supported",
                    )),
                    Ok(true) => Err(self.unimplemented("the credential description registry")),
                    Err(e) => Err(e),
                }
            }
            code => {
                let name = cm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.unimplemented(&format!("{name}: request sessions")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the service's `ServiceInfo` in the parcel.
    #[derive(Clone, Debug, PartialEq)]
    struct Service(i32);

    impl WriteParcelable for Service {
        fn write_to(&self, p: &mut Parcel) {
            p.write_i32(self.0);
        }
    }

    fn provider(component: &str, system: bool) -> ProviderInfo<Service> {
        ProviderInfo {
            service: Service(7),
            component: component.into(),
            system,
            capabilities: Vec::new(),
            enabled: false,
            primary: false,
            settings_subtitle: None,
            settings_activity: None,
        }
    }

    fn element(name: &str, children: Vec<Element>) -> Element {
        Element {
            name: name.into(),
            children,
            ..Default::default()
        }
    }

    fn capability(name: &str) -> Element {
        let mut e = element("capability", Vec::new());
        e.attrs.push(aim_apps::res::Attr {
            ns: String::new(),
            name: "name".into(),
            id: 0,
            value: ResValue::String(name.into()),
            kind: 0,
            data: 0,
        });
        e
    }

    fn order(keys: &[&str]) -> Vec<String> {
        super::super::device_config::hash_map_order(
            keys.iter().map(|k| (k.to_string(), ())).collect(),
            DEFAULT_CAPACITY,
        )
        .into_iter()
        .map(|(k, _)| k)
        .collect()
    }

    /// `parseXmlProviderOuterCapabilities`: the capabilities of every
    /// `<capabilities>`, at any depth, and none outside one.
    #[test]
    fn capabilities_are_read_as_the_original_parses_them() {
        let root = element(
            "credential-provider",
            vec![
                capability("outside"),
                element(
                    "capabilities",
                    vec![
                        capability("android.credentials.TYPE_PASSWORD_CREDENTIAL"),
                        element("group", vec![capability("nested")]),
                        capability(""),
                    ],
                ),
                element(
                    "wrapper",
                    vec![element("capabilities", vec![capability("deep")])],
                ),
            ],
        );
        assert_eq!(
            capabilities(&root),
            [
                "android.credentials.TYPE_PASSWORD_CREDENTIAL",
                "nested",
                "deep"
            ]
        );
    }

    /// `ProviderGenerator`: user providers then system ones, the filter
    /// applied, in a `HashMap`'s order of their flattened components.
    #[test]
    fn providers_are_filtered_and_ordered_as_a_hash_map() {
        let users = vec![
            provider("b.app/b.app.S", false),
            provider("a.app/a.app.S", false),
        ];
        let systems = vec![provider("c.sys/c.sys.S", true)];
        let all = generate(
            users.clone(),
            systems.clone(),
            PROVIDER_FILTER_ALL_PROVIDERS,
        );
        let keys: Vec<String> = all.iter().map(|p| p.component.clone()).collect();
        assert_eq!(
            keys,
            order(&["b.app/b.app.S", "a.app/a.app.S", "c.sys/c.sys.S"])
        );
        let system = generate(
            users.clone(),
            systems.clone(),
            PROVIDER_FILTER_SYSTEM_PROVIDERS_ONLY,
        );
        assert_eq!(system.len(), 1);
        assert!(system[0].system);
        for filter in [
            PROVIDER_FILTER_USER_PROVIDERS_ONLY,
            PROVIDER_FILTER_USER_PROVIDERS_INCLUDING_HIDDEN,
        ] {
            let user = generate(users.clone(), systems.clone(), filter);
            assert_eq!(user.len(), 2);
            assert!(user.iter().all(|p| !p.system));
        }
        // A system provider in the user list (and the reverse) is dropped.
        assert!(generate(systems, users, PROVIDER_FILTER_ALL_PROVIDERS).is_empty());
    }

    /// The settings' component names: empty parts and names that do not
    /// unflatten are skipped, relative classes expanded.
    #[test]
    fn enabled_providers_unflatten_as_the_original() {
        let names = component_names(Some("a.b/.S::bad:c.d/c.d.T:e/"));
        assert_eq!(
            names,
            [
                ComponentName {
                    package: "a.b".into(),
                    class: "a.b.S".into()
                },
                ComponentName {
                    package: "c.d".into(),
                    class: "c.d.T".into()
                },
            ]
        );
        assert!(component_names(None).is_empty());
    }

    /// `String.join(":", HashSet)`: each provider once, primary ones
    /// included, in the set's order.
    #[test]
    fn enabled_setting_is_the_set_joined() {
        let some = |s: &str| Some(s.to_string());
        let joined = join_enabled(
            &[some("x/x.A"), some("y/y.B")],
            &[some("x/x.A"), some("z/z.C")],
        );
        assert_eq!(joined, order(&["x/x.A", "y/y.B", "z/z.C"]).join(":"));
        assert_eq!(join_enabled(&[], &[]), "");
    }

    /// `CredentialProviderInfo.writeToParcel`.
    #[test]
    fn provider_info_parcels_as_the_original() {
        let mut info = provider("a.app/a.app.S", true);
        info.capabilities = vec!["cap".into()];
        info.enabled = true;
        info.settings_subtitle = Some("sub".into());
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut expected = Parcel::new();
        // `writeTypedObject`.
        expected.write_i32(1);
        expected.write_i32(7);
        expected.write_i32(1);
        expected.write_i32(1);
        expected.write_string16(Some("cap"));
        expected.write_i32(1);
        expected.write_i32(0);
        crate::clip::write_char_sequence(&mut expected, None);
        crate::clip::write_char_sequence(&mut expected, Some("sub"));
        crate::clip::write_char_sequence(&mut expected, None);
        assert_eq!(p.data(), expected.data());
    }
}
