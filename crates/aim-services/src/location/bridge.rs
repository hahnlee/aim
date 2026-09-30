//! The location service's side of the system_server bridge
//! (`ILocationHost`, java/device-services): what the original learns
//! inside system_server comes here (bound providers, user lifecycle,
//! power save mode, the screen, reset packages), and what it does inside
//! system_server goes through `ILocationBridge` (content observers, the
//! system configuration's allowlists, the client cache, the package tags
//! of app op attribution).
//!
//! Settings are observed as the original's `SettingsHelper` observes
//! them: an observer per setting, on system_server's behalf.

use std::sync::{Arc, Weak};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    ReadParcelable, android_database_icontentobserver as observer,
    dev_aim_server_ilocationbridge as ilb, dev_aim_server_ilocationhost as ilh,
};

use super::LocationManagerService;
use super::env::{Identity, USER_ALL, java_split};
use super::parcels::PackageTagsList;

/// What an observed setting's change affects.
#[derive(Clone, Copy, Debug)]
pub enum Setting {
    /// `Settings.Secure.LOCATION_MODE`.
    LocationMode,
    /// The background throttle interval or its package allowlist.
    BackgroundThrottle,
    /// `locationPackagePrefixBlacklist` or `...Whitelist`.
    PackageDenylist,
    /// `DeviceConfig`'s `location` namespace (the ignore-settings and
    /// ADAS allowlists).
    LocationConfig,
}

/// The settings the original observes, and their URIs.
const OBSERVED: &[(&str, Setting)] = &[
    (
        "content://settings/secure/location_mode",
        Setting::LocationMode,
    ),
    (
        "content://settings/global/location_background_throttle_interval_ms",
        Setting::BackgroundThrottle,
    ),
    (
        "content://settings/global/location_background_throttle_package_whitelist",
        Setting::BackgroundThrottle,
    ),
    (
        "content://settings/secure/locationPackagePrefixBlacklist",
        Setting::PackageDenylist,
    ),
    (
        "content://settings/secure/locationPackagePrefixWhitelist",
        Setting::PackageDenylist,
    ),
    (
        "content://settings/config/location",
        Setting::LocationConfig,
    ),
];

/// The bridge, once system_server handed it over.
pub struct Bridge {
    pub strong: Strong,
}

impl Bridge {
    fn call<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<aim_service_aidl::Returned<T>>,
    ) -> Option<T> {
        let mut data = Parcel::new();
        write(&mut data);
        let reply = self.strong.transact(code, &data, false).ok()?;
        match read(&mut reply.reader()) {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) => {
                eprintln!("location: bridge call {code}: {}", e.message);
                None
            }
            Err(s) => {
                eprintln!("location: bridge call {code}: status {s}");
                None
            }
        }
    }

    /// Registers the settings observers, each its own node.
    pub fn observe(&self, service: &Arc<LocationManagerService>) {
        for &(uri, setting) in OBSERVED {
            let node = service.env.process.add_service(Arc::new(Observer {
                service: Arc::downgrade(service),
                setting,
            }));
            let args = ilb::RegisterContentObserver {
                uri: Some(uri.into()),
                notify_for_descendants: matches!(setting, Setting::LocationConfig),
                observer: Some(node),
                user_id: USER_ALL,
            };
            self.call(
                ilb::REGISTER_CONTENT_OBSERVER,
                |p| args.write(p),
                ilb::read_register_content_observer_reply,
            );
        }
    }

    /// SystemConfig's location allowlists.
    pub fn system_config(&self) -> super::env::SystemConfig {
        let list = |ignore| {
            let args = ilb::GetLocationSettingsAllowlist { ignore };
            let entries = self
                .call(
                    ilb::GET_LOCATION_SETTINGS_ALLOWLIST,
                    |p| args.write(p),
                    ilb::read_get_location_settings_allowlist_reply,
                )
                .flatten()
                .unwrap_or_default();
            let mut list = PackageTagsList::default();
            for entry in entries.into_iter().flatten() {
                let parts = java_split(&entry, ';');
                let Some((package, tags)) = parts.split_first() else {
                    continue;
                };
                if tags.is_empty() {
                    list.add_all(package);
                }
                for tag in tags {
                    match *tag {
                        "*" => list.add_all(package),
                        "null" => list.add(package, None),
                        tag => list.add(package, Some(tag)),
                    }
                }
            }
            list
        };
        let unthrottled = self
            .call(
                ilb::GET_UNTHROTTLED_LOCATION_PACKAGES,
                |p| ilb::GetUnthrottledLocationPackages {}.write(p),
                ilb::read_get_unthrottled_location_packages_reply,
            )
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        super::env::SystemConfig {
            unthrottled,
            ignore_settings: list(true),
            adas: list(false),
        }
    }

    /// `LocationManager.invalidateLocalLocationEnabledCaches()`.
    pub fn invalidate_location_enabled_cache(&self) {
        self.call(
            ilb::INVALIDATE_LOCATION_ENABLED_CACHE,
            |p| ilb::InvalidateLocationEnabledCache {}.write(p),
            ilb::read_invalidate_location_enabled_cache_reply,
        );
    }

    /// `ServiceWatcher.register` or `unregister` of a bound provider.
    pub fn set_provider_started(&self, provider: &str, started: bool) {
        let args = ilb::SetProviderStarted {
            provider: Some(provider.into()),
            started,
        };
        self.call(
            ilb::SET_PROVIDER_STARTED,
            |p| args.write(p),
            ilb::read_set_provider_started_reply,
        );
    }

    /// The location package tags listener's `onLocationPackageTagsChanged`.
    pub fn set_location_package_tags(&self, uid: i32, tags: &PackageTagsList) {
        let entries = tags
            .0
            .iter()
            .map(|(package, tags)| {
                let mut entry = package.clone();
                for tag in tags {
                    entry.push(';');
                    entry.push_str(tag.as_deref().unwrap_or("null"));
                }
                Some(entry)
            })
            .collect();
        let args = ilb::SetLocationPackageTags {
            uid,
            package_tags: Some(entries),
        };
        self.call(
            ilb::SET_LOCATION_PACKAGE_TAGS,
            |p| args.write(p),
            ilb::read_set_location_package_tags_reply,
        );
    }
}

/// A setting's `IContentObserver`.
struct Observer {
    service: Weak<LocationManagerService>,
    setting: Setting,
}

impl Service for Observer {
    fn descriptor(&self) -> &str {
        observer::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let r = &mut call.data;
        let user = match call.code {
            observer::ON_CHANGE => observer::OnChange::<Uri>::read(r)?.user_id,
            observer::ON_CHANGE_ETC => {
                r.enforce_interface(observer::DESCRIPTOR)?;
                r.read_bool()?; // self change
                for _ in 0..r.read_i32()?.max(0) {
                    if r.read_i32()? != 0 {
                        Uri::read_from(r)?;
                    }
                }
                r.read_i32()?; // flags
                r.read_i32()?
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if let Some(service) = self.service.upgrade() {
            let setting = self.setting;
            let s = service.clone();
            service.fg.post(move || s.on_setting_changed(setting, user));
        }
        Ok(Parcel::new())
    }
}

/// A `Uri`, read past.
struct Uri;

impl ReadParcelable for Uri {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        crate::clip::uri(r).map(|_| Uri)
    }
}

/// Work for the service, off the binder thread.
type Task = Box<dyn FnOnce(&Arc<LocationManagerService>) + Send>;

/// The service host's `ILocationHost`.
pub struct Host {
    pub service: Weak<LocationManagerService>,
}

impl Service for Host {
    fn descriptor(&self) -> &str {
        ilh::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // Only system_server's side of the bridge calls.
        if call.sender_euid != super::env::SYSTEM_UID as u32 {
            return Err(aim_binder_host::parcel::PERMISSION_DENIED);
        }
        let Some(service) = self.service.upgrade() else {
            return Ok(Parcel::new());
        };
        let r = &mut call.data;
        let mut reply = Parcel::new();
        let post = |f: Task| {
            let s = service.clone();
            service.fg.post(move || f(&s));
        };
        match call.code {
            ilh::ON_PROVIDERS_RESOLVED => {
                let a = ilh::OnProvidersResolved::read(r)?;
                let names: Vec<String> = a
                    .providers
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect();
                post(Box::new(move |s| s.providers_resolved(&names)));
            }
            ilh::ON_PROVIDER_BOUND => {
                let a = ilh::OnProviderBound::read(r)?;
                let (Some(name), Some(aim_binder_host::parcel::Binder::Handle(h))) =
                    (a.provider, a.binder)
                else {
                    return Ok(reply);
                };
                // Held before the call's buffer goes.
                let binder = Arc::new(service.env.process.strong(h));
                let package = a.package_name.unwrap_or_default();
                post(Box::new(move |s| {
                    s.provider_bound(&name, Some((binder, package, a.extra_tags)))
                }));
            }
            ilh::ON_PROVIDER_UNBOUND => {
                let a = ilh::OnProviderUnbound::read(r)?;
                let name = a.provider.unwrap_or_default();
                post(Box::new(move |s| s.provider_bound(&name, None)));
            }
            ilh::ON_USER_STARTING => {
                let a = ilh::OnUserStarting::read(r)?;
                post(Box::new(move |s| s.on_user_started(a.user_id)));
            }
            ilh::ON_USER_STOPPED => {
                let a = ilh::OnUserStopped::read(r)?;
                post(Box::new(move |s| s.on_user_stopped(a.user_id)));
            }
            ilh::ON_USER_SWITCHING => {
                let a = ilh::OnUserSwitching::read(r)?;
                post(Box::new(move |s| {
                    s.on_user_switching(a.from_user_id, a.to_user_id)
                }));
            }
            ilh::ON_USER_VISIBILITY_CHANGED => {
                let a = ilh::OnUserVisibilityChanged::read(r)?;
                post(Box::new(move |s| {
                    s.on_user_visibility_changed(a.user_id, a.visible)
                }));
            }
            ilh::ON_LOCATION_POWER_SAVE_MODE_CHANGED => {
                let a = ilh::OnLocationPowerSaveModeChanged::read(r)?;
                post(Box::new(move |s| s.on_power_state(Some(a.mode), None)));
            }
            ilh::ON_SCREEN_INTERACTIVE_CHANGED => {
                let a = ilh::OnScreenInteractiveChanged::read(r)?;
                post(Box::new(move |s| {
                    s.on_power_state(None, Some(a.interactive))
                }));
            }
            ilh::ON_PACKAGE_RESET => {
                let a = ilh::OnPackageReset::read(r)?;
                let package = a.package_name.unwrap_or_default();
                post(Box::new(move |s| s.on_package_reset(&package)));
            }
            ilh::IS_RESETABLE_FOR_PACKAGE => {
                let a = ilh::IsResetableForPackage::read(r)?;
                let package = a.package_name.unwrap_or_default();
                let resetable = service.is_resetable(&package);
                ilh::write_is_resetable_for_package_reply(&mut reply, resetable);
            }
            ilh::IS_PROVIDER => {
                let a = ilh::IsProvider::read(r)?;
                let identity = Identity {
                    uid: a.uid,
                    pid: a.pid,
                    package: a.package_name.unwrap_or_default(),
                    attribution_tag: a.attribution_tag,
                    listener_id: None,
                };
                let is = service.is_provider(a.provider.as_deref(), &identity);
                ilh::write_is_provider_reply(&mut reply, is);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_uri_type_as_its_string() {
        let mut p = Parcel::new();
        p.write_i32(2); // the array's length, as onChangeEtc's
        for (type_id, uri) in [(3, "content://settings/secure/location_mode"), (1, "a:b")] {
            p.write_i32(1); // non-null
            p.write_i32(type_id);
            p.write_string8(Some(uri));
        }
        p.write_i32(0); // flags
        let mut r = Reader::new(p.data(), p.objects());
        for _ in 0..r.read_i32().unwrap() {
            assert_eq!(r.read_i32().unwrap(), 1);
            Uri::read_from(&mut r).unwrap();
        }
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
    }
}
