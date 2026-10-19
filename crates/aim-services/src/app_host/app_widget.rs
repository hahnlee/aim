//! `appwidget`: AppWidgetServiceImpl for the app host (`android-16.0.0_r1`):
//! a user's group state loaded as `ensureGroupStateLoadedLocked` loads it,
//! when first asked: the providers (`loadGroupWidgetProvidersLocked`: the
//! receivers of `APPWIDGET_UPDATE` with `android.appwidget.provider`
//! meta-data), then `appwidgets.xml` (`loadGroupStateLocked`): each
//! provider's info persisted with this build's fingerprint, the hosts, the
//! packages allowed to bind and the widgets, bound to their provider and
//! host. The calls apps make at start are answered as the original
//! answers them for a user with no bound widgets; their RemoteViews are
//! not read where no widget takes them. Bound widgets, RemoteViews updates
//! of widgets, providers whose info must be parsed (`getInfoLocked`),
//! generated previews, the host side and the writes are not implemented
//! (#1789).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::com_android_internal_appwidget_iappwidgetservice as aw;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::component_resolver::Info;
use crate::package::info::ActivityInfo;
use crate::package::info::flags::{GET_META_DATA, GET_SHARED_LIBRARY_FILES};
use crate::package::intent::{ComponentName, Intent};
use crate::package::list_slice::Slice;
use crate::package::pkg::Value;
use crate::package::query::Query;

const SYSTEM_UID: i32 = 1000;
/// `AppWidgetManager.ACTION_APPWIDGET_UPDATE`, `META_DATA_APPWIDGET_PROVIDER`.
const ACTION_APPWIDGET_UPDATE: &str = "android.appwidget.action.APPWIDGET_UPDATE";
const META_DATA_APPWIDGET_PROVIDER: &str = "android.appwidget.provider";
/// `PackageManager.MATCH_DEBUG_TRIAGED_MISSING`.
const MATCH_DEBUG_TRIAGED_MISSING: i64 = 0x1000_0000;
/// `ApplicationInfo.FLAG_EXTERNAL_STORAGE`.
const FLAG_EXTERNAL_STORAGE: i32 = 1 << 18;
const MODIFY_APPWIDGET_BIND_PERMISSIONS: &str =
    "android.permission.MODIFY_APPWIDGET_BIND_PERMISSIONS";
/// `LauncherApps.PinItemRequest.REQUEST_TYPE_APPWIDGET`.
const REQUEST_TYPE_APPWIDGET: i32 = 2;
/// `AppWidgetServiceImpl.CURRENT_VERSION`.
const CURRENT_VERSION: i32 = 1;
/// `TAG_UNDEFINED`.
const TAG_UNDEFINED: i32 = -1;
/// The aconfig flags the service reads.
const REMOTE_VIEWS_PROTO: &str = "android.appwidget.flags.remote_views_proto";
const GENERATED_PREVIEWS: &str = "android.appwidget.flags.generated_previews";
const PROVIDER_INFO: &str = "android.appwidget.AppWidgetProviderInfo";

/// An `AppWidgetProviderInfo` as `AppWidgetXmlUtil.readAppWidgetProviderInfoLocked`
/// reads it from the state file.
#[derive(Clone, Debug, Default, PartialEq)]
struct Persisted {
    min_width: i32,
    min_height: i32,
    min_resize_width: i32,
    min_resize_height: i32,
    max_resize_width: i32,
    max_resize_height: i32,
    target_cell_width: i32,
    target_cell_height: i32,
    update_period_millis: i32,
    initial_layout: i32,
    initial_keyguard_layout: i32,
    configure: Option<ComponentName>,
    label: Option<String>,
    icon: i32,
    preview_image: i32,
    preview_layout: i32,
    auto_advance_view_id: i32,
    resize_mode: i32,
    widget_category: i32,
    widget_features: i32,
    description_res: i32,
    extended_from_app_widget_provider: bool,
}

/// `readAppWidgetProviderInfoLocked`: none unless the info was persisted
/// with this build's fingerprint.
fn read_persisted(
    e: &aim_android_xml::Element,
    fingerprint: Option<&str>,
) -> Result<Option<Persisted>, String> {
    if e.string("os_fingerprint").as_deref() != fingerprint || fingerprint.is_none() {
        return Ok(None);
    }
    let int = |name: &str| e.int(name).map(|v| v.unwrap_or(0));
    Ok(Some(Persisted {
        min_width: int("min_width")?,
        min_height: int("min_height")?,
        min_resize_width: int("min_resize_width")?,
        min_resize_height: int("min_resize_height")?,
        max_resize_width: int("max_resize_width")?,
        max_resize_height: int("max_resize_height")?,
        target_cell_width: int("target_cell_width")?,
        target_cell_height: int("target_cell_height")?,
        update_period_millis: int("update_period_millis")?,
        initial_layout: int("initial_layout")?,
        initial_keyguard_layout: int("initial_keyguard_layout")?,
        // `ComponentName.unflattenFromString` of a non-empty one.
        configure: e
            .string("configure")
            .filter(|c| !c.is_empty())
            .and_then(|c| {
                let (package, class) = c.split_once('/')?;
                let class = if class.starts_with('.') {
                    format!("{package}{class}")
                } else {
                    class.to_string()
                };
                Some(ComponentName {
                    package: package.to_string(),
                    class,
                })
            }),
        label: e.string("label").map(|l| l.into_owned()),
        icon: int("icon")?,
        preview_image: int("preview_image")?,
        preview_layout: int("preview_layout")?,
        auto_advance_view_id: int("auto_advance_view_id")?,
        resize_mode: int("resize_mode")?,
        widget_category: int("widget_category")?,
        widget_features: int("widget_features")?,
        description_res: int("description_res")?,
        extended_from_app_widget_provider: e.bool("provider_inheritance")?.unwrap_or(false),
    }))
}

/// An `AppWidgetProviderInfo` as `writeToParcel` writes it, with its
/// receiver's `ActivityInfo`.
struct ProviderInfo<A = ActivityInfo> {
    provider: ComponentName,
    info: Persisted,
    activity: A,
    generated_previews: bool,
}

impl<A: WriteParcelable> WriteParcelable for ProviderInfo<A> {
    fn write_to(&self, p: &mut Parcel) {
        let i = &self.info;
        p.write_i32(1);
        self.provider.write_to(p);
        for v in [
            i.min_width,
            i.min_height,
            i.min_resize_width,
            i.min_resize_height,
            i.max_resize_width,
            i.max_resize_height,
            i.target_cell_width,
            i.target_cell_height,
            i.update_period_millis,
            i.initial_layout,
            i.initial_keyguard_layout,
        ] {
            p.write_i32(v);
        }
        match &i.configure {
            Some(c) => {
                p.write_i32(1);
                c.write_to(p);
            }
            None => p.write_i32(0),
        }
        p.write_string16(i.label.as_deref());
        for v in [
            i.icon,
            i.preview_image,
            i.preview_layout,
            i.auto_advance_view_id,
            i.resize_mode,
            i.widget_category,
        ] {
            p.write_i32(v);
        }
        p.write_i32(1);
        self.activity.write_to(p);
        p.write_i32(i.widget_features);
        p.write_i32(i.description_res);
        p.write_bool(i.extended_from_app_widget_provider);
        if self.generated_previews {
            // `generatedPreviewCategories`: none was loaded.
            p.write_i32(0);
        }
    }
}

/// A `Provider` of the group.
struct Provider {
    uid: i32,
    component: ComponentName,
    /// `info`, as `setInfoLocked` set it from the state file, with the
    /// receiver it was read for.
    persisted: Option<(Persisted, ActivityInfo)>,
    tag: i32,
}

/// What `ensureGroupStateLoadedLocked` loaded of user 0's group.
struct Group {
    providers: Vec<Provider>,
    /// `mPackagesWithBindWidgetPermission`.
    bind_allowed: Vec<String>,
    /// Widgets bound to a provider and a host (`bindLoadedWidgetsLocked`).
    bound_widgets: usize,
}

pub struct AppWidgetService {
    host: Arc<AppHost>,
    /// `mLoadedUserIds` and the group, once loaded.
    group: Mutex<Option<Arc<Group>>>,
}

impl AppWidgetService {
    pub fn new(host: Arc<AppHost>) -> Self {
        AppWidgetService {
            host,
            group: Mutex::new(None),
        }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("appwidget", &format!("{what} (#1789)"))
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    /// `SecurityPolicy.enforceCallFromPackage` (`AppOpsManager.checkPackage`).
    fn enforce_call_from_package(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        let package = package.ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        Ok(())
    }

    /// `queryIntentReceivers(intent, user)` as the service asks it.
    fn receivers(&self, intent: &Intent, user: i32) -> Result<Vec<ActivityInfo>, Exception> {
        let (_, resolution) = self.host.packages()?;
        // No user of the app host is a profile, so none has an unlocked
        // parent (`isProfileWithUnlockedParent`).
        let flags = GET_META_DATA | MATCH_DEBUG_TRIAGED_MISSING | GET_SHARED_LIBRARY_FILES;
        let found = resolution
            .query_intent_receivers(intent, None, flags, user, SYSTEM_UID)
            .map_err(|e| self.unimplemented(&format!("queryIntentReceivers: {e:?}")))?;
        Ok(found
            .into_iter()
            .filter_map(|r| match r.info {
                Info::Activity(a) => Some(a),
                _ => None,
            })
            .collect())
    }

    /// `getUidForPackage`, or -1.
    fn package_uid(&self, package: &str, user: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        query
            .package_uid(package, 0, user)
            .map_err(|e| self.unimplemented(e.0))?
    }

    /// `ensureGroupStateLoadedLocked(userId)`: the app host's one user is
    /// running and unlocked.
    fn group(&self, user: i32) -> Result<Arc<Group>, Exception> {
        if user != 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("User {user} must be unlocked for widgets to be available"),
            ));
        }
        let mut loaded = self.group.lock().unwrap();
        if let Some(group) = &*loaded {
            return Ok(group.clone());
        }
        let (group, upgraded) = self.load(user)?;
        let group = Arc::new(group);
        *loaded = Some(group.clone());
        upgraded.map(|()| group)
    }

    /// `loadGroupWidgetProvidersLocked`, then `loadGroupStateLocked`: the
    /// group, and whether `performUpgradeLocked` succeeded.
    fn load(&self, user: i32) -> Result<(Group, Result<(), Exception>), Exception> {
        let mut providers: Vec<Provider> = Vec::new();
        let update = Intent {
            action: Some(ACTION_APPWIDGET_UPDATE.into()),
            ..Default::default()
        };
        for ai in self.receivers(&update, user)? {
            // `addProviderLocked`, with `createPartialProviderInfo`.
            if ai.info.application_info.flags & FLAG_EXTERNAL_STORAGE != 0 {
                continue;
            }
            let has_xml = ai.info.item.meta_data.as_ref().is_some_and(|m| {
                m.0.iter().any(|(k, v)| {
                    k == META_DATA_APPWIDGET_PROVIDER && matches!(v, Value::Int(id) if *id != 0)
                })
            });
            let uid = ai.info.application_info.uid;
            let component = ComponentName {
                package: ai.info.item.package_name.clone().unwrap_or_default(),
                class: ai.info.item.name.clone().unwrap_or_default(),
            };
            if has_xml
                && !providers
                    .iter()
                    .any(|p| p.uid == uid && p.component == component)
            {
                providers.push(Provider {
                    uid,
                    component,
                    persisted: None,
                    tag: TAG_UNDEFINED,
                });
            }
        }
        let mut group = Group {
            providers,
            bind_allowed: Vec::new(),
            bound_widgets: 0,
        };
        let data = self.host.data()?;
        let path = data.join(format!("system/users/{user}/appwidgets.xml"));
        let version = match super::device_policy::read_atomic_bytes(&path) {
            Ok(None) => 0,
            Ok(Some(bytes)) => match self.read_state(&bytes, user, &mut group) {
                Ok(version) => version,
                Err(e) => {
                    eprintln!(
                        "app host: appwidget: failed parsing {}: {e}",
                        path.display()
                    );
                    -1
                }
            },
            Err(e) => {
                eprintln!("app host: appwidget: Failed to read state: {e}");
                0
            }
        };
        if self.flag(REMOTE_VIEWS_PROTO)? {
            // `loadGeneratedPreviewCategoriesLocked`, its directory made
            // by `getWidgetPreviewsDirectory`.
            let previews = data.join(format!("system_ce/{user}/appwidget/previews"));
            if let Err(e) = std::fs::create_dir_all(&previews) {
                eprintln!("app host: appwidget: Failed to read preview categories: {e}");
            }
            for p in &group.providers {
                if previews
                    .join(format!(
                        "{}-{}-{}.binpb",
                        p.component.package, p.component.class, p.uid
                    ))
                    .exists()
                {
                    return Err(self.unimplemented("generated widget previews"));
                }
            }
        }
        if version < 0 {
            // `clearWidgetsLocked` and the hosts.
            group.bound_widgets = 0;
            return Ok((group, Ok(())));
        }
        // `performUpgradeLocked`: version 0 is taken for 1.
        let upgraded = match version {
            0 | CURRENT_VERSION => Ok(()),
            _ => Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Failed to upgrade widget database",
            )),
        };
        Ok((group, upgraded))
    }

    /// `readProfileStateFromFileLocked` of the state file `bytes`: its
    /// version.
    fn read_state(&self, bytes: &[u8], user: i32, group: &mut Group) -> Result<i32, String> {
        let root = aim_android_xml::read(bytes)?;
        let fingerprint = self.host.property("ro.build.fingerprint");
        let mut version = -1;
        let (mut provider_index, mut host_index) = (-1, -1);
        // Hosts by tag, and widgets' host and provider tags.
        let mut hosts: Vec<i32> = Vec::new();
        let mut widgets: Vec<(i32, i32)> = Vec::new();
        let fail = |e: Exception| e.message;
        let mut elements = vec![&root];
        while let Some(e) = elements.pop() {
            elements.extend(e.children().collect::<Vec<_>>().into_iter().rev());
            match e.name.as_str() {
                "gs" => version = e.int("version")?.unwrap_or(0),
                "p" => {
                    provider_index += 1;
                    // `getCanonicalPackageName` keeps the name: the
                    // receiver lookup does not throw.
                    let package = e
                        .string("pkg")
                        .ok_or("a provider without pkg")?
                        .into_owned();
                    let class = e.string("cl").ok_or("a provider without cl")?.into_owned();
                    let uid = self.package_uid(&package, user).map_err(fail)?;
                    if uid < 0 {
                        continue;
                    }
                    let component = ComponentName { package, class };
                    // `getProviderInfo`.
                    let intent = Intent {
                        action: Some(ACTION_APPWIDGET_UPDATE.into()),
                        component: Some(component.clone()),
                        ..Default::default()
                    };
                    let Some(activity) = self
                        .receivers(&intent, user)
                        .map_err(fail)?
                        .into_iter()
                        .next()
                    else {
                        continue;
                    };
                    let info = read_persisted(e, fingerprint.as_deref())?;
                    // Outside safe mode the provider must have been
                    // loaded; the original fails the read on a null one.
                    let provider = group
                        .providers
                        .iter_mut()
                        .find(|p| p.uid == uid && p.component == component)
                        .ok_or_else(|| {
                            format!(
                                "no loaded provider {}/{}",
                                component.package, component.class
                            )
                        })?;
                    if let Some(info) = info {
                        provider.persisted = Some((info, activity));
                    }
                    provider.tag = e.int_hex("tag")?.unwrap_or(provider_index);
                    if let Some(ids) = e.string("pending_deleted_ids").filter(|s| !s.is_empty()) {
                        for id in ids.split(',') {
                            id.parse::<i32>()
                                .map_err(|err| format!("pending_deleted_ids: {err}"))?;
                        }
                    }
                }
                "h" => {
                    host_index += 1;
                    let package = e.string("pkg").map(|p| p.into_owned()).unwrap_or_default();
                    // A host of no installed package is a zombie, pruned
                    // outside safe mode.
                    if self.package_uid(&package, user).map_err(fail)? >= 0 {
                        e.int_hex("id")?.ok_or("a host without id")?;
                        hosts.push(e.int_hex("tag")?.unwrap_or(host_index));
                    }
                }
                "b" => {
                    if let Some(package) = e.string("packageName").map(|p| p.into_owned())
                        && self.package_uid(&package, user).map_err(fail)? >= 0
                    {
                        group.bind_allowed.push(package);
                    }
                }
                "g" => {
                    e.int_hex("id")?.ok_or("a widget without id")?;
                    let host = e.int_hex("h")?.ok_or("a widget without h")?;
                    let provider = e.int_hex("p")?.unwrap_or(TAG_UNDEFINED);
                    widgets.push((host, provider));
                }
                _ => {}
            }
        }
        // `bindLoadedWidgetsLocked`.
        group.bound_widgets = widgets
            .iter()
            .filter(|(host, provider)| {
                *provider >= 0
                    && group.providers.iter().any(|p| p.tag == *provider)
                    && *host >= 0
                    && hosts.contains(host)
            })
            .count();
        Ok(version)
    }

    /// A group with no bound widgets, which the calls below answer.
    fn unbound_group(&self, user: i32) -> Result<Arc<Group>, Exception> {
        let group = self.group(user)?;
        if group.bound_widgets > 0 {
            return Err(self.unimplemented("bound app widgets"));
        }
        Ok(group)
    }

    /// `getInstalledProvidersForProfile`: none for a profile outside the
    /// caller's group.
    fn installed_providers(
        &self,
        uid: i32,
        category_filter: i32,
        profile: i32,
        package: Option<&str>,
        p: &mut Parcel,
    ) -> Result<(), Exception> {
        let user = uid / PER_USER_RANGE as i32;
        let (state, resolution) = self.host.packages()?;
        // `isEnabledGroupProfile`: no user of the app host has profiles.
        if profile != user || !state.users.contains_key(&profile) {
            aw::write_get_installed_providers_for_profile_reply::<Slice<ProviderInfo>>(p, None);
            return Ok(());
        }
        let empty = |p: &mut Parcel| {
            let slice: Slice<ProviderInfo> =
                Slice::new(self.host.process.clone(), PROVIDER_INFO, Vec::new());
            aw::write_get_installed_providers_for_profile_reply(p, Some(&slice));
        };
        // `isCallerInstantAppLocked`.
        if crate::package::apps_filter::instant_app_package_name(&state, uid)
            .map_err(|e| self.unimplemented(e.0))?
            .is_some()
        {
            empty(p);
            return Ok(());
        }
        let group = self.unbound_group(user)?;
        let generated_previews = self.flag(GENERATED_PREVIEWS)?;
        let mut result = Vec::new();
        for provider in &group.providers {
            if package.is_some_and(|name| name != provider.component.package) {
                continue;
            }
            // `getInfoLocked` parses what was not persisted.
            let Some((info, activity)) = &provider.persisted else {
                return Err(self.unimplemented("a provider's info not persisted with this build"));
            };
            if info.widget_category & category_filter == 0 {
                continue;
            }
            let provider_profile = activity.info.application_info.uid / PER_USER_RANGE as i32;
            // `canAccessProvider` (the caller's own profile), then
            // `filterAppAccess`.
            let filtered = crate::package::apps_filter::should_filter_application(
                &state,
                &resolution.apps_filter,
                state.packages.get(&provider.component.package),
                uid,
                profile,
                true,
                true,
            )
            .map_err(|e| self.unimplemented(e.0))?;
            if provider_profile == profile && !filtered {
                result.push(ProviderInfo {
                    provider: provider.component.clone(),
                    info: info.clone(),
                    activity: activity.clone(),
                    generated_previews,
                });
            }
        }
        let slice = Slice::new(self.host.process.clone(), PROVIDER_INFO, result);
        aw::write_get_installed_providers_for_profile_reply(p, Some(&slice));
        Ok(())
    }

    /// `updateAppWidgetIds` and `partiallyUpdateAppWidgetIds` of `ids`:
    /// nothing for none; with ids the views' URIs are checked first
    /// (`checkRemoteViewsUris`).
    fn update_ids(
        &self,
        uid: i32,
        package: Option<&str>,
        ids: Option<&[i32]>,
    ) -> Result<(), Exception> {
        if ids.is_none_or(<[i32]>::is_empty) {
            return Ok(());
        }
        self.enforce_call_from_package(uid, package)?;
        Err(self.unimplemented("the RemoteViews of a widget update"))
    }
}

impl Service for AppWidgetService {
    fn descriptor(&self) -> &str {
        aw::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let user = uid / PER_USER_RANGE as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            aw::GET_APP_WIDGET_IDS => {
                let a = aw::GetAppWidgetIds::<ComponentName>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let component = a
                        .provider_component
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "componentName"))?;
                    self.enforce_call_from_package(uid, Some(&component.package))?;
                    // A provider of the caller's has no widget.
                    self.unbound_group(user)?;
                    aw::write_get_app_widget_ids_reply(&mut p, &Some(Vec::new()));
                    Ok(())
                })()
            }
            aw::GET_APP_WIDGET_INFO => {
                let a = aw::GetAppWidgetInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce_call_from_package(uid, a.calling_package.as_deref())?;
                    // No widget is found (`lookupWidgetLocked`).
                    self.unbound_group(user)?;
                    aw::write_get_app_widget_info_reply::<ProviderInfo>(&mut p, None);
                    Ok(())
                })()
            }
            aw::HAS_BIND_APP_WIDGET_PERMISSION => {
                let a =
                    aw::HasBindAppWidgetPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = a.package_name.unwrap_or_default();
                    // `enforceModifyAppWidgetBindPermissions`
                    // (`enforceCallingPermission`).
                    if self
                        .host
                        .check_permission(Some(MODIFY_APPWIDGET_BIND_PERMISSIONS), uid)?
                        != 0
                    {
                        return Err(Exception::security(format!(
                            "hasBindAppWidgetPermission packageName={package}: uid {uid} does not have {MODIFY_APPWIDGET_BIND_PERMISSIONS}."
                        )));
                    }
                    let group = self.group(a.user_id)?;
                    let allowed = self.package_uid(&package, a.user_id)? >= 0
                        && group.bind_allowed.contains(&package);
                    aw::write_has_bind_app_widget_permission_reply(&mut p, allowed);
                    Ok(())
                })()
            }
            // `computeMaximumWidgetBitmapMemory` at the service's start:
            // 1.5 times the default display's real size at 4 bytes a pixel.
            aw::GET_MAX_BITMAP_MEMORY => {
                aw::GetMaxBitmapMemory::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host.display().map(|display| {
                    let (width, height) = (display.0.width, display.0.height);
                    aw::write_get_max_bitmap_memory_reply(&mut p, 6 * width * height);
                })
            }
            aw::IS_REQUEST_PIN_APP_WIDGET_SUPPORTED => {
                aw::IsRequestPinAppWidgetSupported::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let (state, _) = self.host.packages()?;
                    let instant =
                        crate::package::apps_filter::instant_app_package_name(&state, uid)
                            .map_err(|e| self.unimplemented(e.0))?
                            .is_some();
                    // `ShortcutServiceInternal.isRequestPinItemSupported`.
                    let supported = !instant
                        && super::shortcut_service::ShortcutManager::new(self.host.clone())
                            .pin_confirmation_activity(user, REQUEST_TYPE_APPWIDGET)?
                            .is_some();
                    aw::write_is_request_pin_app_widget_supported_reply(&mut p, supported);
                    Ok(())
                })()
            }
            aw::GET_INSTALLED_PROVIDERS_FOR_PROFILE => {
                let a = aw::GetInstalledProvidersForProfile::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.installed_providers(
                    uid,
                    a.category_filter,
                    a.profile_id,
                    a.package_name.as_deref(),
                    &mut p,
                )
            }
            aw::UPDATE_APP_WIDGET_IDS | aw::PARTIALLY_UPDATE_APP_WIDGET_IDS => {
                // The package and the ids; the views are not read where no
                // widget takes them.
                call.data
                    .enforce_interface(aw::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let package = call.data.read_string16().map_err(|_| BAD_VALUE)?;
                let ids =
                    aim_service_aidl::read_int_array(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.update_ids(uid, package.as_deref(), ids.as_deref())
                    .map(|()| p.write_no_exception())
            }
            aw::UPDATE_APP_WIDGET_PROVIDER => {
                // The provider; its views go to its widgets, of which it
                // has none.
                call.data
                    .enforce_interface(aw::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let component = aim_service_aidl::read_typed::<ComponentName>(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    let component = component
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "componentName"))?;
                    self.enforce_call_from_package(uid, Some(&component.package))?;
                    self.unbound_group(user)?;
                    p.write_no_exception();
                    Ok(())
                })()
            }
            aw::NOTIFY_APP_WIDGET_VIEW_DATA_CHANGED => {
                let a = aw::NotifyAppWidgetViewDataChanged::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce_call_from_package(uid, a.package_name.as_deref())?;
                    if a.app_widget_ids
                        .as_deref()
                        .is_some_and(|ids| !ids.is_empty())
                    {
                        self.unbound_group(user)?;
                    }
                    aw::write_notify_app_widget_view_data_changed_reply(&mut p);
                    Ok(())
                })()
            }
            code => {
                let name = aw::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.unimplemented(name))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `readAppWidgetProviderInfoLocked`: the attributes of a provider
    /// persisted with this build, none of another build's.
    #[test]
    fn provider_info_is_read_as_the_state_file_persists_it() {
        let xml = br#"<p pkg="com.example" cl="com.example.Widget" tag="0" min_width="66561" min_height="12289" min_resize_width="66561" min_resize_height="12289" max_resize_width="0" max_resize_height="0" target_cell_width="2" target_cell_height="1" update_period_millis="86400000" initial_layout="2131624638" initial_keyguard_layout="0" configure="com.example/.Configure" label="Example" icon="7" preview_image="8" preview_layout="9" auto_advance_view_id="-1" resize_mode="3" widget_category="5" widget_features="1" description_res="10" provider_inheritance="true" os_fingerprint="build/1" />"#;
        let e = aim_android_xml::read(xml).unwrap();
        assert_eq!(read_persisted(&e, Some("build/2")).unwrap(), None);
        assert_eq!(read_persisted(&e, None).unwrap(), None);
        let info = read_persisted(&e, Some("build/1")).unwrap().unwrap();
        assert_eq!(
            (info.min_width, info.min_height, info.target_cell_width),
            (66561, 12289, 2)
        );
        assert_eq!(
            (
                info.update_period_millis,
                info.initial_layout,
                info.auto_advance_view_id
            ),
            (86400000, 2131624638, -1)
        );
        assert_eq!(
            info.configure,
            Some(ComponentName {
                package: "com.example".into(),
                class: "com.example.Configure".into()
            })
        );
        assert_eq!(info.label.as_deref(), Some("Example"));
        assert_eq!(
            (
                info.resize_mode,
                info.widget_category,
                info.widget_features,
                info.description_res
            ),
            (3, 5, 1, 10)
        );
        assert!(info.extended_from_app_widget_provider);
    }

    /// `AppWidgetProviderInfo.writeToParcel`: the provider, the sizes, the
    /// configure activity, the label, the receiver and, with generated
    /// previews, their categories.
    #[test]
    fn provider_info_parcels_as_the_original_writes_it() {
        let info = Persisted {
            min_width: 1,
            label: Some("L".into()),
            widget_features: 4,
            ..Default::default()
        };
        let provider = ComponentName {
            package: "a".into(),
            class: "a.B".into(),
        };
        // A receiver, as two ints.
        struct Receiver;
        impl WriteParcelable for Receiver {
            fn write_to(&self, p: &mut Parcel) {
                p.write_i32(7);
                p.write_i32(8);
            }
        }
        let mut p = Parcel::new();
        ProviderInfo {
            provider,
            info,
            activity: Receiver,
            generated_previews: true,
        }
        .write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("a"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("a.B"));
        assert_eq!(r.read_i32().unwrap(), 1);
        for _ in 0..10 {
            assert_eq!(r.read_i32().unwrap(), 0);
        }
        assert_eq!(r.read_i32().unwrap(), 0, "no configure activity");
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("L"));
        for _ in 0..6 {
            r.read_i32().unwrap();
        }
        assert_eq!(r.read_i32().unwrap(), 1, "the receiver follows");
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (7, 8));
        assert_eq!(r.read_i32().unwrap(), 4);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
    }
}
