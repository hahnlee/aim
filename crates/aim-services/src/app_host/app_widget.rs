//! `appwidget`: AppWidgetServiceImpl for the app host (`android-16.0.0_r1`):
//! a user's group state loaded as `ensureGroupStateLoadedLocked` loads it,
//! when first asked: the providers (`loadGroupWidgetProvidersLocked`: the
//! receivers of `APPWIDGET_UPDATE` with `android.appwidget.provider`
//! meta-data), then `appwidgets.xml` (`loadGroupStateLocked`): each
//! provider's info persisted with this build's fingerprint, the hosts, the
//! packages allowed to bind and the widgets, bound to their provider and
//! host. A provider's info the state file did not persist is parsed from
//! its meta-data's XML when first asked for (`getInfoLocked`). The calls
//! apps make at start are answered as the original answers them for a
//! user with no bound widgets; their RemoteViews are not read where no
//! widget takes them. `notifyProviderInheritance` marks the caller's
//! providers and writes the group's state back (`saveStateLocked`). Bound
//! widgets, RemoteViews updates of widgets, generated previews, the host
//! side and the other writes are not implemented (#1789).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_android_xml::{Element, Node, Value as XmlValue};
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
use crate::package::parse::attrs::TypedArray;
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
/// `AppWidgetProviderInfo.RESIZE_NONE`, `WIDGET_CATEGORY_HOME_SCREEN`.
const RESIZE_NONE: i32 = 0;
const WIDGET_CATEGORY_HOME_SCREEN: i32 = 1;

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
    /// `info.providerInfo`: the receiver, as loaded or as read with the
    /// state file's info.
    activity: ActivityInfo,
    /// `info`: partial (all defaults) until the state file sets it or
    /// `getInfoLocked` parses it.
    info: Persisted,
    /// `mInfoParsed`.
    parsed: bool,
    /// `infoTag`, the meta-data key the state file names.
    info_tag: Option<String>,
    tag: i32,
    /// `pendingDeletedWidgetIds`.
    pending_deleted: Vec<i32>,
}

/// What `serializeProvider` writes of a provider.
struct Saved<'a> {
    component: &'a ComponentName,
    tag: i32,
    info_tag: Option<&'a str>,
    /// The info, if parsed (`mInfoParsed`).
    info: Option<&'a Persisted>,
    pending_deleted: &'a [i32],
}

impl Provider {
    fn saved(&self) -> Saved<'_> {
        Saved {
            component: &self.component,
            tag: self.tag,
            info_tag: self.info_tag.as_deref(),
            info: self.parsed.then_some(&self.info),
            pending_deleted: &self.pending_deleted,
        }
    }
}

/// A `Host` of the group the state file names.
struct Host {
    package: String,
    id: i32,
    tag: i32,
}

/// What `ensureGroupStateLoadedLocked` loaded of user 0's group.
struct Group {
    providers: Vec<Provider>,
    hosts: Vec<Host>,
    /// `mPackagesWithBindWidgetPermission`, in its `ArraySet` order.
    bind_allowed: Vec<String>,
    /// Widgets bound to a provider and a host (`bindLoadedWidgetsLocked`).
    bound_widgets: usize,
    /// Whether the state file holds the backup restore controller's state
    /// (`br`).
    restore_state: bool,
}

pub struct AppWidgetService {
    host: Arc<AppHost>,
    /// `mLoadedUserIds` and the group, once loaded.
    group: Mutex<Option<Group>>,
}

impl AppWidgetService {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        let service = Arc::new(AppWidgetService {
            host,
            group: Mutex::new(None),
        });
        let monitor = service.clone() as Arc<dyn super::package_writes::PackageMonitor>;
        service
            .host
            .package_monitors
            .register(super::USER_ALL, Arc::downgrade(&monitor));
        service
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

    /// `ensureGroupStateLoadedLocked(userId)`, then `f` over the group:
    /// the app host's one user is running and unlocked.
    fn with_group<T>(
        &self,
        user: i32,
        f: impl FnOnce(&mut Group) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        if user != 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("User {user} must be unlocked for widgets to be available"),
            ));
        }
        let mut loaded = self.group.lock().unwrap();
        if loaded.is_none() {
            let (group, upgraded) = self.load(user)?;
            let group = loaded.insert(group);
            upgraded?;
            return f(group);
        }
        f(loaded.as_mut().unwrap())
    }

    /// `loadGroupWidgetProvidersLocked`, then `loadGroupStateLocked`: the
    /// group, and whether `performUpgradeLocked` succeeded.
    fn load(&self, user: i32) -> Result<(Group, Result<(), Exception>), Exception> {
        let providers = self.discover_providers(user)?;
        let mut group = Group {
            providers,
            hosts: Vec::new(),
            bind_allowed: Vec::new(),
            bound_widgets: 0,
            restore_state: false,
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
            group.hosts.clear();
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
    fn discover_providers(&self, user: i32) -> Result<Vec<Provider>, Exception> {
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
                    activity: ai,
                    info: Persisted::default(),
                    parsed: false,
                    info_tag: None,
                    tag: TAG_UNDEFINED,
                    pending_deleted: Vec::new(),
                });
            }
        }
        Ok(providers)
    }

    fn read_state(&self, bytes: &[u8], user: i32, group: &mut Group) -> Result<i32, String> {
        let root = aim_android_xml::read(bytes)?;
        let fingerprint = self.host.property("ro.build.fingerprint");
        let mut version = -1;
        let (mut provider_index, mut host_index) = (-1, -1);
        // Widgets' host and provider tags.
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
                    // `setInfoLocked`.
                    if let Some(info) = info {
                        provider.info = info;
                        provider.activity = activity;
                        provider.parsed = true;
                    }
                    provider.tag = e.int_hex("tag")?.unwrap_or(provider_index);
                    provider.info_tag = e.string("info_tag").map(|t| t.into_owned());
                    if let Some(ids) = e.string("pending_deleted_ids").filter(|s| !s.is_empty()) {
                        for id in ids.split(',') {
                            provider.pending_deleted.push(
                                id.parse::<i32>()
                                    .map_err(|err| format!("pending_deleted_ids: {err}"))?,
                            );
                        }
                    }
                }
                "h" => {
                    host_index += 1;
                    let package = e.string("pkg").map(|p| p.into_owned()).unwrap_or_default();
                    // A host of no installed package is a zombie, pruned
                    // outside safe mode.
                    if self.package_uid(&package, user).map_err(fail)? >= 0 {
                        let id = e.int_hex("id")?.ok_or("a host without id")?;
                        let tag = e.int_hex("tag")?.unwrap_or(host_index);
                        group.hosts.push(Host { package, id, tag });
                    }
                }
                "b" => {
                    if let Some(package) = e.string("packageName").map(|p| p.into_owned())
                        && self.package_uid(&package, user).map_err(fail)? >= 0
                        && !group.bind_allowed.contains(&package)
                    {
                        group.bind_allowed.push(package);
                    }
                }
                "br" => group.restore_state = true,
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
                    && group.hosts.iter().any(|h| h.tag == *host)
            })
            .count();
        // `ArraySet<Pair<Integer, String>>`: by the pair's hash, user 0's
        // and the package's.
        let bind_allowed = std::mem::take(&mut group.bind_allowed);
        group.bind_allowed = crate::package::info::array_order(bind_allowed, |p| p.as_str());
        Ok(version)
    }

    /// `f` over a group with no bound widgets, which the calls below
    /// answer.
    fn with_unbound_group<T>(
        &self,
        user: i32,
        f: impl FnOnce(&mut Group) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        self.with_group(user, |group| {
            if group.bound_widgets > 0 {
                return Err(self.unimplemented("bound app widgets"));
            }
            f(group)
        })
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
        let generated_previews = self.flag(GENERATED_PREVIEWS)?;
        let result = self.with_unbound_group(user, |group| {
            let mut result = Vec::new();
            for provider in &mut group.providers {
                if package.is_some_and(|name| name != provider.component.package) {
                    continue;
                }
                let info = self.info(provider)?.clone();
                if info.widget_category & category_filter == 0 {
                    continue;
                }
                let activity = &provider.activity;
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
                        info,
                        activity: activity.clone(),
                        generated_previews,
                    });
                }
            }
            Ok(result)
        })?;
        let slice = Slice::new(self.host.process.clone(), PROVIDER_INFO, result);
        aw::write_get_installed_providers_for_profile_reply(p, Some(&slice));
        Ok(())
    }

    /// `Provider.getInfoLocked`: the info, parsed from the provider's
    /// meta-data the first time it is asked for when the state file did
    /// not set it (`parseAppWidgetProviderInfo` of `infoTag`, else of
    /// `android.appwidget.provider`); a provider whose XML does not parse
    /// keeps its partial info.
    fn info<'p>(&self, provider: &'p mut Provider) -> Result<&'p Persisted, Exception> {
        if !provider.parsed {
            let mut parsed = None;
            if let Some(tag) = provider.info_tag.clone().filter(|t| !t.is_empty()) {
                parsed = self.parse_info(provider, &tag)?;
            }
            if parsed.is_none() {
                parsed = self.parse_info(provider, META_DATA_APPWIDGET_PROVIDER)?;
            }
            if let Some(info) = parsed {
                provider.info = info;
            }
            provider.parsed = true;
        }
        Ok(&provider.info)
    }

    /// `parseAppWidgetProviderInfo` of the meta-data `key` of `provider`'s
    /// receiver: its `appwidget-provider` element's attributes as the
    /// package's resources resolve them (the dimensions their complex
    /// data, as `AppWidgetManager` converts them in the app), and the
    /// receiver's safe label and icon.
    fn parse_info(&self, provider: &Provider, key: &str) -> Result<Option<Persisted>, Exception> {
        let activity = &provider.activity;
        let item = &activity.info.item;
        let name = format!(
            "{}/{} for user {}",
            provider.component.package, provider.component.class, provider.uid
        );
        // `loadXmlMetaData`: the meta-data's resource.
        let id = item
            .meta_data
            .as_ref()
            .and_then(|m| m.0.iter().find(|(k, _)| k == key))
            .and_then(|(_, v)| match v {
                Value::Int(id) => Some(*id),
                _ => None,
            })
            .unwrap_or(0);
        if id == 0 {
            eprintln!("app host: appwidget: No {key} meta-data for AppWidget provider '{name}'");
            return Ok(None);
        }
        let (state, _) = self.host.packages()?;
        let Some(pkg) = state
            .packages
            .get(&provider.component.package)
            .and_then(|ps| ps.pkg.clone())
        else {
            eprintln!("app host: appwidget: XML parsing failed for AppWidget provider {name}");
            return Ok(None);
        };
        let inputs = self.host.platform()?;
        let root = self.host.root()?;
        let fail = |e: String| self.unimplemented(&format!("the provider XML of {name}: {e}"));
        let parsed = self
            .host
            .framework()?
            .xml_resource(root, &inputs.data, &pkg, id as u32, |e, ctx| {
                (e.name == "appwidget-provider")
                    .then(|| read_provider_info(&ctx.obtain(e), &provider.component.package))
            })
            .map_err(fail)?;
        let Some(mut info) = parsed else {
            eprintln!(
                "app host: appwidget: Meta-data does not start with appwidget-provider tag for AppWidget provider {name}"
            );
            return Ok(None);
        };
        // `ComponentInfo.loadUnsafeLabel`, then `loadSafeLabel` (the system
        // server loads safe labels).
        let app = &activity.info.application_info.item;
        let ids: Vec<u32> = [item.label_res, app.label_res]
            .iter()
            .map(|&r| r as u32)
            .collect();
        let texts = self
            .host
            .framework()?
            .resource_names_and_strings(root, &inputs.data, &pkg, &ids)
            .map_err(fail)?;
        let text = |i: usize, res: i32| (res != 0).then(|| texts[i].1.clone()).flatten();
        let unsafe_label = item
            .non_localized_label
            .clone()
            .or_else(|| text(0, item.label_res))
            .or_else(|| app.non_localized_label.clone())
            .or_else(|| text(1, app.label_res))
            .or_else(|| item.name.clone())
            .unwrap_or_default();
        info.label = Some(self.host.safe_label(&unsafe_label)?);
        // `ComponentInfo.getIconResource`.
        info.icon = if item.icon != 0 { item.icon } else { app.icon };
        Ok(Some(info))
    }

    /// `notifyProviderInheritance`: each named provider of the caller's
    /// marked as extending `AppWidgetProvider` (on its info as it is, which
    /// a later parse replaces), then the group's state saved.
    fn notify_provider_inheritance(
        &self,
        uid: i32,
        components: Option<Vec<Option<ComponentName>>>,
    ) -> Result<(), Exception> {
        let user = uid / PER_USER_RANGE as i32;
        let Some(components) = components else {
            return Ok(());
        };
        let mut named = Vec::new();
        for component in components {
            let Some(component) = component else {
                return Ok(());
            };
            self.enforce_call_from_package(uid, Some(&component.package))?;
            named.push(component);
        }
        self.with_group(user, |group| {
            for component in &named {
                // `lookupProviderLocked` of the caller's uid.
                let Some(provider) = group
                    .providers
                    .iter_mut()
                    .find(|p| p.uid == uid && p.component == *component)
                else {
                    return Ok(());
                };
                provider.info.extended_from_app_widget_provider = true;
            }
            self.save_state(user, group)
        })
    }

    /// `saveStateLocked` of `user`'s group (`saveGroupStateAsync`'s, made
    /// at once): the providers and hosts tagged by their index, then the
    /// state file written as `writeProfileStateToStreamLocked` writes it.
    fn save_state(&self, user: i32, group: &mut Group) -> Result<(), Exception> {
        if group.bound_widgets > 0 {
            return Err(self.unimplemented("saving bound app widgets"));
        }
        if group.restore_state {
            return Err(self.unimplemented("saving the backup restore controller's state"));
        }
        for (i, p) in group.providers.iter_mut().enumerate() {
            p.tag = i as i32;
        }
        for (i, h) in group.hosts.iter_mut().enumerate() {
            h.tag = i as i32;
        }
        let fingerprint = self.host.property("ro.build.fingerprint");
        let providers: Vec<Saved> = group.providers.iter().map(Provider::saved).collect();
        let root = state_document(
            &providers,
            &group.hosts,
            &group.bind_allowed,
            fingerprint.as_deref().unwrap_or_default(),
        );
        let path = self
            .host
            .data()?
            .join(format!("system/users/{user}/appwidgets.xml"));
        if let Err(e) = write_atomic(&path, &root) {
            eprintln!("app host: appwidget: Failed open state file for write: {e}");
        }
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

impl super::package_writes::PackageMonitor for AppWidgetService {
    fn package_changed(
        &self,
        change: &super::package_writes::PackageChanged<'_>,
    ) -> Result<(), Exception> {
        if change.user != 0 {
            return Ok(());
        }
        let mut held = self.group.lock().unwrap();
        let Some(group) = held.as_mut() else {
            return Ok(());
        };
        let mut found = self.discover_providers(change.user)?;
        if group.bound_widgets > 0 {
            if group
                .providers
                .iter()
                .chain(&found)
                .any(|p| p.component.package == change.package)
            {
                return Err(self.unimplemented("bound app widgets"));
            }
            return Ok(());
        }
        // Keep the tags/info of unaffected providers and the group's host state.
        for provider in &mut found {
            if provider.component.package != change.package {
                if let Some(index) = group
                    .providers
                    .iter()
                    .position(|old| old.component == provider.component && old.uid == provider.uid)
                {
                    std::mem::swap(provider, &mut group.providers[index]);
                }
            } else if let Some(old) = group
                .providers
                .iter()
                .find(|old| old.component == provider.component && old.uid == provider.uid)
            {
                provider.tag = old.tag;
                provider.info_tag = old.info_tag.clone();
                provider.pending_deleted = old.pending_deleted.clone();
            }
        }
        group.providers = found;
        self.save_state(change.user, group)
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
                    self.with_unbound_group(user, |_| Ok(()))?;
                    aw::write_get_app_widget_ids_reply(&mut p, &Some(Vec::new()));
                    Ok(())
                })()
            }
            aw::GET_APP_WIDGET_INFO => {
                let a = aw::GetAppWidgetInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.enforce_call_from_package(uid, a.calling_package.as_deref())?;
                    // No widget is found (`lookupWidgetLocked`).
                    self.with_unbound_group(user, |_| Ok(()))?;
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
                    let allowed = self.package_uid(&package, a.user_id)? >= 0
                        && self.with_group(a.user_id, |group| {
                            Ok(group.bind_allowed.contains(&package))
                        })?;
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
                    self.with_unbound_group(user, |_| Ok(()))?;
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
                        self.with_unbound_group(user, |_| Ok(()))?;
                    }
                    aw::write_notify_app_widget_view_data_changed_reply(&mut p);
                    Ok(())
                })()
            }
            aw::NOTIFY_PROVIDER_INHERITANCE => {
                let a = aw::NotifyProviderInheritance::<ComponentName>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.notify_provider_inheritance(uid, a.component_names)
                    .map(|()| aw::write_notify_provider_inheritance_reply(&mut p))
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

/// The `R.styleable.AppWidgetProviderInfo` attributes of a provider's
/// `appwidget-provider` element, as `parseAppWidgetProviderInfo` reads
/// them; the label and icon come from its receiver.
fn read_provider_info(a: &TypedArray, package: &str) -> Persisted {
    // `peekValue(..).data`: a dimension's complex data, which the app's
    // `AppWidgetManager` converts.
    let data = |name: &str| a.peek(name).map(|v| v.data as i32);
    let min_width = data("minWidth").unwrap_or(0);
    let min_height = data("minHeight").unwrap_or(0);
    Persisted {
        min_width,
        min_height,
        min_resize_width: data("minResizeWidth").unwrap_or(min_width),
        min_resize_height: data("minResizeHeight").unwrap_or(min_height),
        max_resize_width: data("maxResizeWidth").unwrap_or(0),
        max_resize_height: data("maxResizeHeight").unwrap_or(0),
        target_cell_width: a.int("targetCellWidth", 0),
        target_cell_height: a.int("targetCellHeight", 0),
        update_period_millis: a.int("updatePeriodMillis", 0),
        initial_layout: a.resource_id("initialLayout", 0),
        initial_keyguard_layout: a.resource_id("initialKeyguardLayout", 0),
        configure: a.string("configure").map(|class| ComponentName {
            package: package.to_string(),
            class,
        }),
        label: None,
        icon: 0,
        preview_image: a.resource_id("previewImage", 0),
        preview_layout: a.resource_id("previewLayout", 0),
        // `View.NO_ID`.
        auto_advance_view_id: a.resource_id("autoAdvanceViewId", -1),
        resize_mode: a.int("resizeMode", RESIZE_NONE),
        widget_category: a.int("widgetCategory", WIDGET_CATEGORY_HOME_SCREEN),
        widget_features: a.int("widgetFeatures", 0),
        description_res: a.resource_id("description", 0),
        extended_from_app_widget_provider: false,
    }
}

/// `ComponentName.flattenToShortString`.
fn flatten_to_short_string(c: &ComponentName) -> String {
    match c.class.strip_prefix(&c.package) {
        Some(rest) if rest.starts_with('.') => format!("{}/{rest}", c.package),
        _ => format!("{}/{}", c.package, c.class),
    }
}

/// `writeProfileStateToStreamLocked` of a group without widgets: the
/// providers (`serializeProvider`, with the info of those parsed,
/// `AppWidgetXmlUtil.writeAppWidgetProviderInfoLocked`), the hosts
/// (`serializeHost`) and the packages allowed to bind.
fn state_document(
    providers: &[Saved],
    hosts: &[Host],
    bind_allowed: &[String],
    fingerprint: &str,
) -> Element {
    let element = |name: &str, attrs: Vec<(&str, XmlValue)>| Element {
        name: name.into(),
        attrs: attrs.into_iter().map(|(n, v)| (n.to_string(), v)).collect(),
        content: Vec::new(),
    };
    let mut content = Vec::new();
    for p in providers {
        let mut attrs = vec![
            ("pkg", XmlValue::String(p.component.package.clone())),
            ("cl", XmlValue::String(p.component.class.clone())),
            ("tag", XmlValue::IntHex(p.tag)),
        ];
        if let Some(tag) = p.info_tag.filter(|t| !t.is_empty()) {
            attrs.push(("info_tag", XmlValue::String(tag.into())));
        }
        if let Some(i) = p.info {
            for (name, v) in [
                ("min_width", i.min_width),
                ("min_height", i.min_height),
                ("min_resize_width", i.min_resize_width),
                ("min_resize_height", i.min_resize_height),
                ("max_resize_width", i.max_resize_width),
                ("max_resize_height", i.max_resize_height),
                ("target_cell_width", i.target_cell_width),
                ("target_cell_height", i.target_cell_height),
                ("update_period_millis", i.update_period_millis),
                ("initial_layout", i.initial_layout),
                ("initial_keyguard_layout", i.initial_keyguard_layout),
            ] {
                attrs.push((name, XmlValue::Int(v)));
            }
            if let Some(c) = &i.configure {
                attrs.push(("configure", XmlValue::String(flatten_to_short_string(c))));
            }
            if let Some(label) = &i.label {
                attrs.push(("label", XmlValue::String(label.clone())));
            }
            for (name, v) in [
                ("icon", i.icon),
                ("preview_image", i.preview_image),
                ("preview_layout", i.preview_layout),
                ("auto_advance_view_id", i.auto_advance_view_id),
                ("resize_mode", i.resize_mode),
                ("widget_category", i.widget_category),
                ("widget_features", i.widget_features),
                ("description_res", i.description_res),
            ] {
                attrs.push((name, XmlValue::Int(v)));
            }
            attrs.push((
                "provider_inheritance",
                XmlValue::Bool(i.extended_from_app_widget_provider),
            ));
            attrs.push(("os_fingerprint", XmlValue::String(fingerprint.into())));
        }
        if !p.pending_deleted.is_empty() {
            let ids: Vec<String> = p.pending_deleted.iter().map(i32::to_string).collect();
            attrs.push(("pending_deleted_ids", XmlValue::String(ids.join(","))));
        }
        content.push(Node::Element(element("p", attrs)));
    }
    for h in hosts {
        content.push(Node::Element(element(
            "h",
            vec![
                ("pkg", XmlValue::String(h.package.clone())),
                ("id", XmlValue::IntHex(h.id)),
                ("tag", XmlValue::IntHex(h.tag)),
            ],
        )));
    }
    for package in bind_allowed {
        content.push(Node::Element(element(
            "b",
            vec![("packageName", XmlValue::String(package.clone()))],
        )));
    }
    let mut root = element("gs", vec![("version", XmlValue::Int(CURRENT_VERSION))]);
    root.content = content;
    root
}

/// `AtomicFile.startWrite` (a legacy backup is the file again), then
/// `finishWrite`: the new file synced and renamed over the file, the
/// system's, mode 0600.
fn write_atomic(path: &std::path::Path, root: &Element) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = aim_android_xml::abx::write(root).map_err(std::io::Error::other)?;
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        std::path::PathBuf::from(name)
    };
    let backup = with(".bak");
    if backup.exists() {
        std::fs::rename(&backup, path)?;
    }
    let new = with(".new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        },
    )?;
    std::fs::rename(&new, path)
}

#[cfg(test)]
mod tests {
    /// AppWidgetServiceImpl's package receiver updates the loaded providers
    /// for component and application PACKAGE_CHANGED alike.
    #[test]
    fn widget_providers_track_session_enabled_changes() {
        crate::system::with_discovery_session(|host, publish, enabled| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    let mut receiver = pkg.activities[0].clone();
                    receiver.main.component.name = "p.Widget".into();
                    receiver.main.component.meta_data =
                        Some(crate::package::pkg::MetaData(vec![(
                            META_DATA_APPWIDGET_PROVIDER.into(),
                            Value::Int(0x7f010002),
                        )]));
                    receiver.main.component.intents =
                        vec![crate::package::intent_filter::ParsedIntentInfo {
                            filter: crate::package::intent_filter::IntentFilter {
                                actions: vec![ACTION_APPWIDGET_UPDATE.into()],
                                ..Default::default()
                            },
                            ..Default::default()
                        }];
                    pkg.receivers = vec![receiver];
                }
            });
            let service = AppWidgetService::new(host.clone());
            let providers = service.discover_providers(0).unwrap();
            assert_eq!(providers.len(), 1);
            *service.group.lock().unwrap() = Some(Group {
                providers,
                hosts: vec![],
                bind_allowed: vec![],
                bound_widgets: 0,
                restore_state: false,
            });
            for component in [Some("p.Widget"), None] {
                enabled(component, 2);
                assert!(
                    service
                        .group
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .providers
                        .is_empty()
                );
                enabled(component, 1);
                let group = service.group.lock().unwrap();
                let providers = &group.as_ref().unwrap().providers;
                assert_eq!(providers.len(), 1);
                assert_eq!(providers[0].component.class, "p.Widget");
            }
        });
    }

    #[test]
    fn widget_refresh_keeps_hosts_and_rejects_bound_state() {
        use super::super::package_writes::{PackageChanged, PackageMonitor};
        crate::system::with_discovery_session(|host, publish, _| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    let mut receiver = pkg.activities[0].clone();
                    receiver.main.component.intents =
                        vec![crate::package::intent_filter::ParsedIntentInfo {
                            filter: crate::package::intent_filter::IntentFilter {
                                actions: vec![ACTION_APPWIDGET_UPDATE.into()],
                                ..Default::default()
                            },
                            ..Default::default()
                        }];
                    receiver.main.component.meta_data =
                        Some(crate::package::pkg::MetaData(vec![(
                            META_DATA_APPWIDGET_PROVIDER.into(),
                            Value::Int(1),
                        )]));
                    pkg.receivers = vec![receiver];
                }
            });
            let service = AppWidgetService::new(host.clone());
            let providers = service.discover_providers(0).unwrap();
            assert_eq!(providers.len(), 1);
            *service.group.lock().unwrap() = Some(Group {
                providers,
                hosts: vec![Host {
                    package: "q".into(),
                    id: 7,
                    tag: 8,
                }],
                bind_allowed: vec!["q".into()],
                bound_widgets: 0,
                restore_state: false,
            });
            let change = PackageChanged {
                package: "p",
                user: 0,
                components: &[],
                removed: true,
            };
            service
                .group
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .bound_widgets = 1;
            assert!(
                service
                    .package_changed(&change)
                    .unwrap_err()
                    .message
                    .contains("#1789")
            );
            assert_eq!(
                service
                    .group
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .providers
                    .len(),
                1
            );
            let unrelated = PackageChanged {
                package: "q",
                user: 0,
                components: &[],
                removed: false,
            };
            service.package_changed(&unrelated).unwrap();
            service
                .group
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .bound_widgets = 0;
            publish(&mut |pkg| pkg.receivers.clear());
            service.package_changed(&change).unwrap();
            let held = service.group.lock().unwrap();
            let group = held.as_ref().unwrap();
            assert!(group.providers.is_empty());
            assert_eq!(group.hosts[0].id, 7);
            assert_eq!(group.bind_allowed, ["q"]);
        });
    }
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

    /// `writeProfileStateToStreamLocked`: a parsed provider's info written
    /// as `writeAppWidgetProviderInfoLocked` writes it reads back as
    /// `readAppWidgetProviderInfoLocked` reads it; a provider not parsed is
    /// written without one; hosts, the packages allowed to bind and the
    /// version follow.
    #[test]
    fn the_state_file_is_written_as_the_original_writes_it() {
        let info = Persisted {
            min_width: 0x10301,
            min_resize_width: 0x10301,
            target_cell_width: 2,
            initial_layout: 0x7f0d0001,
            configure: Some(ComponentName {
                package: "com.example".into(),
                class: "com.example.Configure".into(),
            }),
            label: Some("Clock".into()),
            icon: 0x7f080001,
            auto_advance_view_id: -1,
            resize_mode: 3,
            widget_category: 1,
            extended_from_app_widget_provider: true,
            ..Default::default()
        };
        let component = |class: &str| ComponentName {
            package: "com.example".into(),
            class: class.into(),
        };
        let (a, b) = (component("com.example.A"), component("com.example.B"));
        let providers = [
            Saved {
                component: &a,
                tag: 0,
                info_tag: None,
                info: Some(&info),
                pending_deleted: &[3, 4],
            },
            Saved {
                component: &b,
                tag: 1,
                info_tag: None,
                info: None,
                pending_deleted: &[],
            },
        ];
        let hosts = [Host {
            package: "com.launcher".into(),
            id: 1024,
            tag: 0,
        }];
        let document = state_document(&providers, &hosts, &["com.launcher".into()], "build/1");
        let bytes = aim_android_xml::abx::write(&document).unwrap();
        let root = aim_android_xml::read(&bytes).unwrap();
        assert_eq!(root.name, "gs");
        assert_eq!(root.int("version").unwrap(), Some(CURRENT_VERSION));
        let children: Vec<_> = root.children().collect();
        assert_eq!(
            children.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["p", "p", "h", "b"]
        );
        let (a, b) = (children[0], children[1]);
        assert_eq!(
            a.string("configure").as_deref(),
            Some("com.example/.Configure")
        );
        assert_eq!(read_persisted(a, Some("build/1")).unwrap(), Some(info));
        assert_eq!(a.int_hex("tag").unwrap(), Some(0));
        assert_eq!(a.string("pending_deleted_ids").as_deref(), Some("3,4"));
        assert_eq!(read_persisted(b, Some("build/1")).unwrap(), None);
        assert_eq!(b.string("os_fingerprint"), None);
        assert_eq!(b.int_hex("tag").unwrap(), Some(1));
        let h = children[2];
        assert_eq!(
            (h.string("pkg").as_deref(), h.int_hex("id").unwrap()),
            (Some("com.launcher"), Some(1024))
        );
        assert_eq!(
            children[3].string("packageName").as_deref(),
            Some("com.launcher")
        );
    }

    /// `parseAppWidgetProviderInfo` of an `appwidget-provider` element:
    /// dimensions as their complex data, the resize minimums defaulting to
    /// the minimums, a configure activity of the provider's package, and
    /// the defaults of what it does not give.
    #[test]
    fn a_provider_xml_parses_as_the_original_parses_it() {
        use crate::package::parse::resources::{
            Config, Resources, TYPE_DIMENSION, TYPE_FIRST_INT, TYPE_INT_HEX, TYPE_STRING,
        };
        let names = [
            "minWidth",
            "minHeight",
            "minResizeWidth",
            "minResizeHeight",
            "maxResizeWidth",
            "maxResizeHeight",
            "targetCellWidth",
            "targetCellHeight",
            "updatePeriodMillis",
            "initialLayout",
            "initialKeyguardLayout",
            "configure",
            "previewImage",
            "previewLayout",
            "autoAdvanceViewId",
            "resizeMode",
            "widgetCategory",
            "widgetFeatures",
            "description",
        ];
        let ids: std::collections::HashMap<String, u32> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_string(), 0x0101_0000 + i as u32))
            .collect();
        let attr =
            |name: &str, value: aim_apps::res::Value, kind: u8, data: u32| aim_apps::res::Attr {
                ns: crate::package::parse::attrs::ANDROID.into(),
                name: name.into(),
                id: ids[name],
                value,
                kind,
                data,
            };
        // 110dp, 40dp: `TypedValue` complex data, unit dip.
        let (dp110, dp40) = ((110 << 8) | 1, (40 << 8) | 1);
        let e = aim_apps::res::Element {
            name: "appwidget-provider".into(),
            attrs: vec![
                attr(
                    "minWidth",
                    aim_apps::res::Value::Dimension(dp110),
                    TYPE_DIMENSION,
                    dp110,
                ),
                attr(
                    "minHeight",
                    aim_apps::res::Value::Dimension(dp40),
                    TYPE_DIMENSION,
                    dp40,
                ),
                attr(
                    "targetCellWidth",
                    aim_apps::res::Value::Int(2),
                    TYPE_FIRST_INT,
                    2,
                ),
                attr(
                    "updatePeriodMillis",
                    aim_apps::res::Value::Int(0),
                    TYPE_FIRST_INT,
                    0,
                ),
                attr(
                    "configure",
                    aim_apps::res::Value::String("com.example.Configure".into()),
                    TYPE_STRING,
                    0,
                ),
                attr("resizeMode", aim_apps::res::Value::Int(3), TYPE_INT_HEX, 3),
                attr(
                    "widgetFeatures",
                    aim_apps::res::Value::Int(5),
                    TYPE_INT_HEX,
                    5,
                ),
            ],
            ..Default::default()
        };
        let res = Resources {
            tables: Vec::new(),
            overlays: &[],
            config: Config::default(),
        };
        let ctx = crate::package::parse::attrs::Ctx {
            res: &res,
            attrs: &ids,
            error: Default::default(),
        };
        let info = read_provider_info(&ctx.obtain(&e), "com.example");
        assert!(ctx.error.take().is_none());
        assert_eq!(
            (
                info.min_width,
                info.min_height,
                info.min_resize_width,
                info.min_resize_height
            ),
            (dp110 as i32, dp40 as i32, dp110 as i32, dp40 as i32)
        );
        assert_eq!(
            (
                info.max_resize_width,
                info.target_cell_width,
                info.target_cell_height
            ),
            (0, 2, 0)
        );
        assert_eq!(
            info.configure,
            Some(ComponentName {
                package: "com.example".into(),
                class: "com.example.Configure".into()
            })
        );
        assert_eq!(
            (
                info.resize_mode,
                info.widget_category,
                info.widget_features,
                info.auto_advance_view_id,
                info.initial_layout
            ),
            (3, WIDGET_CATEGORY_HOME_SCREEN, 5, -1, 0)
        );
    }

    #[test]
    fn components_flatten_to_their_short_string() {
        let c = |package: &str, class: &str| ComponentName {
            package: package.into(),
            class: class.into(),
        };
        assert_eq!(flatten_to_short_string(&c("a.b", "a.b.C")), "a.b/.C");
        assert_eq!(flatten_to_short_string(&c("a.b", "x.C")), "a.b/x.C");
        assert_eq!(flatten_to_short_string(&c("a.b", "a.bC")), "a.b/a.bC");
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
