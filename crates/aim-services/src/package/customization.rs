//! Component UI and retained-install mutation owners, Android 16 r1.
//! Copyright The Android Open Source Project, Apache License 2.0.
use super::{apps_filter, effects, intent::ComponentName, model, owner::{self, user_runtime},
    parse::{Platform, resources::{Config, Resources}}, query::Query, scan::SigningScan};
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, Reader, BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER}};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use std::{collections::BTreeMap, sync::{Arc, Mutex}};

pub struct Owner { live: Mutex<Live> }
struct Live { allowed_ui_package: String, keep: Vec<Option<String>>, no_kill_observers: BTreeMap<Option<String>, InstalledObserver> }
impl Owner {
    /// The configured package comes from the real framework table/overlays.
    pub fn load(platform: &Platform, config: Config) -> Result<Arc<Self>, String> {
        let resources = Resources { tables: vec![&platform.framework], overlays: &platform.framework_overlays, config };
        let id = platform.framework.id("string", "config_overrideComponentUiPackage").ok_or("component UI package resource absent")?;
        let allowed_ui_package = resources.resource_string(id).ok_or("component UI package string unavailable")?;
        Ok(Arc::new(Self { live: Mutex::new(Live { allowed_ui_package, keep: Vec::new(), no_kill_observers: BTreeMap::new() }) }))
    }
    pub fn refresh_resources(&self, platform: &Platform, config: Config) -> Result<(), String> {
        let resources = Resources { tables: vec![&platform.framework], overlays: &platform.framework_overlays, config };
        let id = platform.framework.id("string", "config_overrideComponentUiPackage").ok_or("component UI package resource absent")?;
        let value = resources.resource_string(id).ok_or("component UI package string unavailable")?;
        self.live.lock().unwrap().allowed_ui_package = value;
        Ok(())
    }
    pub fn register_no_kill_observer(&self, package: Option<String>, observer: InstalledObserver) {
        self.live.lock().unwrap().no_kill_observers.insert(package, observer);
    }
    /// Returning the record removes it before delivery, including on a dead
    /// observer, just as mNoKillInstallObservers.remove does in original PMS.
    pub fn take_no_kill_observer(&self, package: Option<&str>) -> Option<InstalledObserver> {
        self.live.lock().unwrap().no_kill_observers.remove(&package.map(str::to_owned))
    }
    pub fn should_keep(&self, package: Option<&str>) -> bool {
        self.live.lock().unwrap().keep.iter().any(|name| name.as_deref() == package)
    }
    pub fn keep_packages(&self) -> Vec<Option<String>> { self.live.lock().unwrap().keep.clone() }

    pub fn prepare_label(&self, query: &Query<'_>, request: Label) -> Result<LabelPlan, Exception> {
        if request.overriding && request.label.as_ref().is_none_or(String::is_empty) {
            return Err(Exception::illegal_argument("Override label should be a valid String"));
        }
        let component = request.component.ok_or_else(|| Exception::illegal_argument("Must specify a component"))?;
        let component_uid = query.package_uid(&component.package, 0, request.user).map_err(missing)??;
        if apps_filter::app_id(query.calling_uid) != apps_filter::app_id(component_uid) {
            return Err(Exception::security(format!("The calling UID ({}) does not match the target UID", query.calling_uid)));
        }
        let allowed = self.live.lock().unwrap().allowed_ui_package.clone();
        if allowed.is_empty() { return Err(Exception::security("There is no package defined as allowed to change a component's label or icon")); }
        let allowed_uid = query.package_uid(&allowed, super::info::flags::MATCH_SYSTEM_ONLY, request.user).map_err(missing)??;
        if allowed_uid == -1 || apps_filter::app_id(allowed_uid) != apps_filter::app_id(query.calling_uid) {
            return Err(Exception::security(format!("The calling UID ({}) is not allowed to change a component's label or icon", query.calling_uid)));
        }
        let package = query.state.packages.get(&component.package).filter(|package| package.pkg.is_some() && (package.is.system || package.is.updated_system_app))
            .ok_or_else(|| Exception::security("Changing the label is not allowed for this component"))?;
        let code = package.pkg.as_ref().unwrap();
        let exists = code.activities.iter().chain(&code.receivers).map(|value| &value.main.component.name)
            .chain(code.services.iter().map(|value| &value.main.component.name))
            .chain(code.providers.iter().map(|value| &value.main.component.name)).any(|name| name == &component.class);
        if !exists { return Err(Exception::illegal_argument("Component not found")); }
        let user = super::info::user_state(package, request.user);
        let previous = user.component_label_icon_overrides.iter().find(|(name, _, _)| name == &component.class);
        let old = previous.map(|(_, label, icon)| user_runtime::LabelIcon { label: label.clone(), icon: *icon }).unwrap_or_default();
        let value = user_runtime::LabelIcon { label: request.label, icon: request.icon };
        Ok(LabelPlan { component, user: request.user, app_id: package.app_id, changed: old != value, value })
    }
    pub fn notify_replaced(&self, query: &Query<'_>, packages: Option<Vec<Option<String>>>) -> Result<(), Exception> {
        let packages = packages.ok_or_else(|| Exception::new(EX_NULL_POINTER, "null packages"))?;
        let user = apps_filter::user_id(query.calling_uid);
        let mut visible = Vec::new();
        for package in packages {
            let state = package.as_ref().and_then(|name| query.state.packages.get(name));
            if !query.filtered(state, query.calling_uid, user).map_err(missing)? && !visible.contains(&package) { visible.push(package); }
        }
        visible.sort_by_key(|name| name.as_deref().map(super::info::java_hash).unwrap_or(0));
        for package in visible {
            if let Some(observer) = self.take_no_kill_observer(package.as_deref()) {
                if let Err(error) = observer.notify() { eprintln!("Install observer no longer exists: {error:?}"); }
            }
        }
        Ok(())
    }
    /// Native delete pipeline schedules DELETE_ALL_USERS/version-highest with
    /// removedBySystem=true. The retained set changes before delete work begins.
    pub fn set_keep(&self, query: &Query<'_>, packages: Option<Vec<Option<String>>>, mut enqueue_delete: impl FnMut(&str) -> Result<(), Exception>) -> Result<(), Exception> {
        if !matches!(query.calling_uid, 0 | 1000) && !query.uid_has_permission(query.calling_uid, "android.permission.KEEP_UNINSTALLED_PACKAGES").map_err(missing)? {
            return Err(Exception::security("setKeepUninstalledPackages requires KEEP_UNINSTALLED_PACKAGES permission"));
        }
        let packages = packages.ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageList"))?;
        let mut live = self.live.lock().unwrap();
        let removed: Vec<_> = live.keep.iter().filter(|name| !packages.contains(name)).cloned().collect();
        let mut next = Vec::new();
        for name in packages { if !next.contains(&name) { next.push(name); } }
        next.sort_by_key(|name| name.as_deref().map(super::info::java_hash).unwrap_or(0));
        live.keep = next;
        for name in removed {
            let Some(package) = name.as_ref().and_then(|name| query.state.packages.get(name)) else { continue; };
            if package.users.values().any(|user| user.installed) { continue; }
            enqueue_delete(&package.name)?;
        }
        Ok(())
    }
}

pub struct InstalledObserver {
    pub observer: Strong,
    pub package: Option<String>,
    pub return_code: i32,
    pub message: Option<String>,
    /// Complete real Bundle parcel, retaining its object metadata.
    pub extras: Option<Parcel>,
}
impl InstalledObserver {
    pub fn notify(self) -> Result<(), Exception> {
        use aim_service_aidl::android_content_pm_ipackageinstallobserver2 as observer;
        let mut request = Parcel::new();
        request.write_interface_token(observer::DESCRIPTOR);
        request.write_string16(self.package.as_deref()); request.write_i32(self.return_code); request.write_string16(self.message.as_deref());
        request.write_i32(i32::from(self.extras.is_some()));
        if let Some(extras) = self.extras { request.write_raw(extras.data(), extras.objects()); }
        self.observer.transact(observer::ON_PACKAGE_INSTALLED, &request, true).map_err(|status| Exception::new(EX_ILLEGAL_STATE, format!("install observer transport: {status}")))?;
        Ok(())
    }
}
#[derive(Debug)]
pub struct Label { pub component: Option<ComponentName>, pub label: Option<String>, pub icon: Option<i32>, pub user: i32, pub overriding: bool }
impl Label {
    pub fn read(code: u32, reader: &mut Reader<'_>) -> Result<Self, i32> {
        let value = if code == pm::OVERRIDE_LABEL_AND_ICON {
            let args = pm::OverrideLabelAndIcon::<ComponentName>::read(reader)?;
            Self { component: args.component_name, label: args.non_localized_label, icon: Some(args.icon), user: args.user_id, overriding: true }
        } else {
            let args = pm::RestoreLabelAndIcon::<ComponentName>::read(reader)?;
            Self { component: args.component_name, label: None, icon: None, user: args.user_id, overriding: false }
        };
        if reader.remaining() != 0 { return Err(BAD_VALUE); }
        Ok(value)
    }
}
pub struct LabelPlan { pub component: ComponentName, pub user: i32, pub app_id: i32, pub changed: bool, pub value: user_runtime::LabelIcon }
impl LabelPlan {
    /// Runtime-only state: original Settings never serializes these overrides.
    pub fn apply_scan(&self, scan: &mut SigningScan) -> Result<(), String> {
        if !self.changed { return Ok(()); }
        let state = scan.scanned_user_states(&self.component.package).ok_or("component user owner absent")?.get(&self.user).cloned().unwrap_or_default();
        let mut runtime = state.runtime;
        runtime.override_label_icon(user_runtime::Component { package: self.component.package.clone(), class: self.component.class.clone() }, self.value.clone());
        scan.set_user_runtime(&self.component.package, self.user, runtime)
    }
    pub fn finish(&self, effects: &effects::Owner, calling_uid: i32) -> Result<(), Exception> {
        if self.changed { effects.component_label_changed(&self.component.package, apps_filter::uid(self.user, self.app_id), &self.component.class, calling_uid)?; }
        Ok(())
    }
}
/// Genuine synchronous ABX restriction write. The dirty-owner callback cancels
/// only that user's pending write, and the shared message when no dirty users remain.
pub fn flush(query: &Query<'_>, disk: &mut owner::Store, scan: &SigningScan, user: i32,
    mut clear_dirty: impl FnMut(i32) -> Result<(), Exception>) -> Result<(), Exception> {
    if apps_filter::instant_app_package_name(query.state, query.calling_uid).map_err(missing)?.is_some() || !query.state.users.contains_key(&user) { return Ok(()); }
    if let Err(error) = query.enforce_cross_user(user, false, false, "flushPackageRestrictions").map_err(missing)? { return Err(error); }
    let user_id = u32::try_from(user).map_err(|error| Exception::illegal_argument(error.to_string()))?;
    disk.commit_updated_scan_restrictions(scan, user_id, false).map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
    clear_dirty(user)
}
fn missing(error: apps_filter::NotModelled) -> Exception { Exception::new(EX_ILLEGAL_STATE, error.0) }

pub enum Request {
    Label(Label), Flush(i32), Replaced(Option<Vec<Option<String>>>), Keep(Option<Vec<Option<String>>>),
}
impl Request {
    pub fn read(code: u32, reader: &mut Reader<'_>) -> Option<Result<Self, i32>> {
        if !matches!(code, pm::OVERRIDE_LABEL_AND_ICON | pm::RESTORE_LABEL_AND_ICON | pm::FLUSH_PACKAGE_RESTRICTIONS_AS_USER | pm::NOTIFY_PACKAGES_REPLACED_RECEIVED | pm::SET_KEEP_UNINSTALLED_PACKAGES) { return None; }
        Some((|| {
            let value = match code {
                pm::OVERRIDE_LABEL_AND_ICON | pm::RESTORE_LABEL_AND_ICON => Self::Label(Label::read(code, reader)?),
                pm::FLUSH_PACKAGE_RESTRICTIONS_AS_USER => Self::Flush(pm::FlushPackageRestrictionsAsUser::read(reader)?.user_id),
                pm::NOTIFY_PACKAGES_REPLACED_RECEIVED => Self::Replaced(pm::NotifyPackagesReplacedReceived::read(reader)?.packages),
                pm::SET_KEEP_UNINSTALLED_PACKAGES => Self::Keep(pm::SetKeepUninstalledPackages::read(reader)?.package_list),
                _ => return Err(BAD_VALUE),
            };
            if reader.remaining() != 0 { return Err(BAD_VALUE); }
            Ok(value)
        })())
    }
}
