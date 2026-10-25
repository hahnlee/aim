//! `webviewupdate`: WebViewUpdateService for the app host
//! (`WebViewUpdateServiceImpl2` and `SystemImpl` at `android-16.0.0_r1`):
//! the WebView providers the framework configures
//! (`config_webview_packages`), the one in use (the user's choice if it is
//! valid and installed and enabled for every user, else the default), its
//! RELRO files made by `RelroFileCreator` in isolated processes
//! (`WebViewLibraryLoader.createRelros`), and an app's wait for them
//! (`waitForAndGetProvider`), which makes the provider visible to it.
//!
//! The provider is prepared (`prepareWebViewInSystemServer`) when the
//! service is first asked, and the WebView zygote started once its RELRO
//! files are made (`webview_zygote`). Package notifications reselect the
//! provider, defer a second preparation until pending RELRO work finishes,
//! and stop dependents of the previous provider. Explicit provider choice,
//! default-provider repair and file pinning remain #1491.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_webkit_iwebviewupdateservice as wv;

use super::settings::Table;
use super::{AppHost, reply};
use crate::package::info::PackageInfo;
use crate::package::query::Query;

/// `WebViewFactory.LIBLOAD_*`.
const LIBLOAD_SUCCESS: i32 = 0;
const LIBLOAD_FAILED_WAITING_FOR_RELRO: i32 = 3;
const LIBLOAD_FAILED_LISTING_WEBVIEW_PACKAGES: i32 = 4;
/// `WebViewUpdateServiceImpl2.WAIT_TIMEOUT_MS`.
const WAIT_TIMEOUT: Duration = Duration::from_millis(1000);
/// `SystemImpl.PACKAGE_FLAGS`: `GET_META_DATA | GET_SIGNATURES |
/// GET_SHARED_LIBRARY_FILES | MATCH_ANY_USER`; `MATCH_FACTORY_ONLY`.
const PACKAGE_FLAGS: i64 = 0x80 | 0x40 | 0x400 | 0x0040_0000;
const MATCH_FACTORY_ONLY: i64 = 0x0020_0000;
/// `ApplicationInfo.FLAG_SYSTEM`, `FLAG_INSTALLED`.
const FLAG_SYSTEM: i32 = 0x1;
const FLAG_INSTALLED: i32 = 0x0080_0000;
const SYSTEM_UID: i32 = 1000;
/// `Process.SHARED_RELRO_UID`.
const SHARED_RELRO_UID: u32 = 1037;
/// `WebViewFactoryProvider.MINIMUM_SUPPORTED_VERSION_CODE`,
/// `MINIMUM_SUPPORTED_TARGET_SDK` (Tiramisu).
const MINIMUM_SUPPORTED_VERSION_CODE: i64 = 661_308_800;
const MINIMUM_SUPPORTED_TARGET_SDK: i32 = 33;
const WEBVIEW_LIBRARY: &str = "com.android.webview.WebViewLibrary";
const RELRO_FILE_CREATOR: &str = "android.webkit.WebViewLibraryLoader$RelroFileCreator";

/// `WebViewProviderInfo`: a provider the framework configures.
#[derive(Clone, Debug, PartialEq)]
struct Provider {
    package: String,
    description: String,
    available_by_default: bool,
    is_fallback: bool,
    /// Its signatures, decoded.
    signatures: Vec<Vec<u8>>,
}

impl WriteParcelable for Provider {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.package));
        p.write_string16(Some(&self.description));
        p.write_i32(self.available_by_default as i32);
        p.write_i32(self.is_fallback as i32);
        // `writeTypedArray` of `Signature`s (`writeByteArray`).
        p.write_i32(self.signatures.len() as i32);
        for s in &self.signatures {
            p.write_i32(1);
            write_byte_array(p, s);
        }
    }
}

/// `Parcel.writeByteArray`: the length, then the bytes, padded to four.
fn write_byte_array(p: &mut Parcel, bytes: &[u8]) {
    p.write_i32(bytes.len() as i32);
    let mut padded = bytes.to_vec();
    padded.resize(bytes.len().div_ceil(4) * 4, 0);
    p.write_raw(&padded, &[]);
}

/// The providers of `config_webview_packages`, as `SystemImpl` reads
/// them, or why it cannot.
fn providers(xml: &[u8]) -> Result<Vec<Provider>, String> {
    let root =
        aim_apps::res::xml(xml).map_err(|e| format!("Error when parsing WebView config {e:?}"))?;
    // `getAttributeValue`: a compiled boolean as `true` or `false`.
    let attr = |e: &aim_apps::res::Element, name: &str| match e.named(name) {
        Some(aim_apps::res::Value::String(s)) => Some(s.clone()),
        Some(aim_apps::res::Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    };
    let mut providers = Vec::new();
    for e in root.children.iter().filter(|e| e.name == "webviewprovider") {
        let package = attr(e, "packageName")
            .ok_or("WebView provider in framework resources missing package name")?;
        let description = attr(e, "description")
            .ok_or("WebView provider in framework resources missing description")?;
        let mut signatures = Vec::new();
        for s in e.children.iter().filter(|s| s.name == "signature") {
            let text: String = s.text.chars().filter(|c| !c.is_whitespace()).collect();
            signatures.push(aim_android_xml::base64::decode(&text).ok_or("bad base-64")?);
        }
        let provider = Provider {
            package,
            description,
            available_by_default: attr(e, "availableByDefault").as_deref() == Some("true"),
            is_fallback: attr(e, "isFallback").as_deref() == Some("true"),
            signatures,
        };
        if provider.is_fallback {
            if !provider.available_by_default {
                return Err("Each WebView fallback package must be available by default.".into());
            }
            if providers.iter().any(|p: &Provider| p.is_fallback) {
                return Err("There can be at most one WebView fallback package.".into());
            }
        }
        providers.push(provider);
    }
    if !providers.iter().any(|p| p.available_by_default) {
        return Err(
            "There must be at least one WebView package that is available by default".into(),
        );
    }
    Ok(providers)
}

/// The provider in use and its RELRO files' progress.
#[derive(Default)]
struct State {
    capture: Option<Arc<crate::package::scan_snapshot::query_state::Capture>>,
    current: Option<PackageInfo>,
    relros_started: u32,
    relros_finished: u32,
    any_installed: bool,
    /// `mWebViewPackageDirty`: the provider changed during RELRO creation.
    dirty: bool,
}

pub struct WebViewUpdate {
    host: Arc<AppHost>,
    /// The configured providers, read once; and whether the provider was
    /// prepared.
    providers: std::sync::OnceLock<Vec<Provider>>,
    /// The state, and its change (relro files finished), shared with the
    /// thread that starts the WebView zygote once they are.
    shared: Arc<(Mutex<Option<State>>, Condvar)>,
}

impl WebViewUpdate {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        let service = Arc::new(WebViewUpdate {
            host,
            providers: std::sync::OnceLock::new(),
            shared: Arc::new((Mutex::new(None), Condvar::new())),
        });
        let monitor = service.clone() as Arc<dyn super::package_writes::PackageMonitor>;
        service
            .host
            .package_monitors
            .register(super::USER_ALL, Arc::downgrade(&monitor));
        service
    }

    fn providers(&self) -> Result<&[Provider], Exception> {
        if let Some(p) = self.providers.get() {
            return Ok(p);
        }
        let xml = self
            .host
            .framework()?
            .framework_file(self.host.root()?, "xml", "config_webview_packages")
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "webviewupdate: no config_webview_packages",
                )
            })?;
        let parsed = providers(&xml)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("webviewupdate: {e}")))?;
        Ok(self.providers.get_or_init(|| parsed))
    }

    fn default_provider(&self) -> Result<&Provider, Exception> {
        // The constructor's: the first available by default.
        Ok(self
            .providers()?
            .iter()
            .find(|p| p.available_by_default)
            .unwrap())
    }

    /// `pm.getPackageInfo(package, flags)` as the system asks it.
    fn package_info(
        &self,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        package: &str,
        flags: i64,
    ) -> Result<Option<PackageInfo>, Exception> {
        let state = capture.state();
        let resolution = capture.resolution().map_err(|e| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("webviewupdate: resolution: {e:?}"),
            )
        })?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        query.package_info(package, -1, flags, 0).map_err(|e| {
            self.host
                .unimplemented_named("webviewupdate", &format!("getPackageInfo: {} (#1491)", e.0))
        })?
    }

    fn property(&self, name: &str) -> Option<String> {
        self.host.property(name)
    }

    /// `validityResult(...) == VALIDITY_OK`.
    fn valid(
        &self,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        provider: &Provider,
        info: &PackageInfo,
    ) -> Result<bool, Exception> {
        let app = info.application_info.as_ref().ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "webviewupdate: a package without application info",
            )
        })?;
        // `WebViewFactoryProvider.isCompatibleImplementationPackage`: with
        // the B entry point, a version from `MINIMUM_SUPPORTED_VERSION_CODE`,
        // else one targeting Tiramisu or after.
        let compatible = if capture
            .state()
            .system
            .flags
            .iter()
            .any(|(name, on)| name == "android.webkit.use_b_entry_point" && *on)
        {
            i64::from(info.version_code) >= MINIMUM_SUPPORTED_VERSION_CODE
        } else {
            app.target_sdk_version >= MINIMUM_SUPPORTED_TARGET_SDK
        };
        if !compatible {
            return Ok(false);
        }
        let debuggable = self.property("ro.debuggable").as_deref() == Some("1");
        let version = |i: &PackageInfo| {
            (i64::from(i.version_code_major) << 32) | i64::from(i.version_code as u32)
        };
        if !debuggable && version(info) / 100_000 < self.minimum_version_code(capture)? / 100_000 {
            return Ok(false);
        }
        // `providerHasValidSignature`.
        let signed = debuggable
            || app.flags & FLAG_SYSTEM != 0
            || matches!(info.signatures.as_deref(), Some([only]) if provider.signatures.iter().any(|s| s == only));
        if !signed {
            return Ok(false);
        }
        Ok(library(info).is_some())
    }

    /// `getMinimumVersionCode`: the lowest factory version of the providers
    /// available by default.
    fn minimum_version_code(
        &self,
        capture: &crate::package::scan_snapshot::query_state::Capture,
    ) -> Result<i64, Exception> {
        let mut minimum = -1;
        for p in self.providers()?.iter().filter(|p| p.available_by_default) {
            if let Some(info) = self.package_info(capture, &p.package, MATCH_FACTORY_ONLY)? {
                let v = (i64::from(info.version_code_major) << 32)
                    | i64::from(info.version_code as u32);
                if minimum < 0 || v < minimum {
                    minimum = v;
                }
            }
        }
        Ok(minimum)
    }

    /// `findPreferredWebViewPackage`: the user's choice if valid and
    /// installed and enabled for every user (here, user 0), else the
    /// default if valid.
    fn preferred(
        &self,
        capture: &crate::package::scan_snapshot::query_state::Capture,
    ) -> Result<Option<PackageInfo>, Exception> {
        let chosen = self
            .host
            .platform()?
            .settings
            .get(Table::Global, "webview_provider");
        let configured = self.providers()?;
        if let Some(provider) =
            chosen.and_then(|c| configured.iter().find(|p| p.package == c).cloned())
            && let Some(info) = self.package_info(capture, &provider.package, PACKAGE_FLAGS)?
            && self.valid(capture, &provider, &info)?
            && info
                .application_info
                .as_ref()
                .is_some_and(|a| a.flags & FLAG_INSTALLED != 0 && a.enabled)
        {
            return Ok(Some(info));
        }
        let default = self.default_provider()?.clone();
        if let Some(info) = self.package_info(capture, &default.package, PACKAGE_FLAGS)?
            && self.valid(capture, &default, &info)?
            && info
                .application_info
                .as_ref()
                .is_some_and(|a| a.flags & FLAG_INSTALLED != 0 && a.enabled)
        {
            return Ok(Some(info));
        }
        Ok(None)
    }

    /// The state, the provider prepared first (`prepareWebViewInSystemServer`,
    /// then `onWebViewProviderChanged`: its RELRO files made).
    fn state(&self) -> Result<std::sync::MutexGuard<'_, Option<State>>, Exception> {
        let mut guard = self.shared.0.lock().unwrap();
        let capture = self.host.package_capture()?;
        let state = guard.get_or_insert_with(State::default);
        if state
            .capture
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &capture))
            && !state.dirty
        {
            return Ok(guard);
        }
        let preferred = self.preferred(&capture)?;
        let changed = !same_provider(
            state.capture.as_deref(),
            &capture,
            state.current.as_ref(),
            preferred.as_ref(),
        );
        if changed && state.relros_started != state.relros_finished {
            state.dirty = true;
            return Ok(guard);
        }
        if changed {
            let old = state.current.take();
            state.any_installed = preferred.is_some();
            state.dirty = false;
            state.current = preferred;
            if let Some(info) = &state.current {
                (state.relros_started, state.relros_finished) = self.create_relros(info);
                if let Some(app) = &info.application_info {
                    self.host.webview_provider_changed(Some(app.clone()));
                }
                let (host, shared) = (self.host.clone(), self.shared.clone());
                std::thread::spawn(move || {
                    drop(wait_ready(&shared));
                    host.ensure_webview_zygote();
                });
            } else {
                self.host.webview_provider_changed(None);
            }
            if let Some(old) = old.and_then(|info| info.application_info) {
                if let Some(name) = &old.item.package_name {
                    self.host.kill_package_processes(name, old.uid as u32);
                }
            }
            self.shared.1.notify_all();
        }
        state.capture = Some(capture);
        Ok(guard)
    }

    /// `WebViewLibraryLoader.createRelros`: a `RelroFileCreator` per ABI
    /// the device supports, 32-bit first; the number started, and of
    /// those finished: one that cannot start (its crash handler's notice).
    fn create_relros(&self, info: &PackageInfo) -> (u32, u32) {
        let Some(library) = library(info) else {
            return (0, 0);
        };
        let package = info.package_name.clone().unwrap_or_default();
        let (mut started, mut finished) = (0, 0);
        for list in ["ro.product.cpu.abilist32", "ro.product.cpu.abilist64"] {
            let Some(abi) = self
                .property(list)
                .and_then(|l| l.split(',').find(|a| !a.is_empty()).map(str::to_string))
            else {
                continue;
            };
            started += 1;
            let (shared, crashed) = (self.shared.clone(), abi.clone());
            if let Err(e) = self.host.start_isolated_entry_point(
                RELRO_FILE_CREATOR,
                &[package.clone(), library.clone()],
                &format!("WebViewLoader-{abi}"),
                &abi,
                SHARED_RELRO_UID,
                Arc::new(move || {
                    eprintln!(
                        "app host: webviewupdate: relro file creator for {crashed} crashed. Proceeding without"
                    );
                    relro_finished(&shared);
                }),
            ) {
                eprintln!(
                    "app host: webviewupdate: error starting relro file creator for abi {abi}: {}",
                    e.message
                );
                finished += 1;
            }
        }
        (started, finished)
    }

    /// `waitForAndGetProvider`: the provider once its RELRO files are made,
    /// waiting at most `WAIT_TIMEOUT`.
    fn wait_for_and_get_provider(
        &self,
        caller: i32,
    ) -> Result<(Option<PackageInfo>, i32), Exception> {
        drop(self.state()?);
        let guard = wait_ready(&self.shared);
        let s = guard.as_ref().unwrap();
        let status = if ready(s) {
            LIBLOAD_SUCCESS
        } else if !s.any_installed {
            LIBLOAD_FAILED_LISTING_WEBVIEW_PACKAGES
        } else {
            LIBLOAD_FAILED_WAITING_FOR_RELRO
        };
        let current = s.current.clone();
        drop(guard);
        if let Some(info) = &current {
            self.grant_visibility(info, caller)?;
        }
        Ok((current, status))
    }

    /// `grantVisibilityToCaller`: the provider's package visible to the
    /// caller (`grantImplicitAccess`).
    fn grant_visibility(&self, info: &PackageInfo, caller: i32) -> Result<(), Exception> {
        let webview_uid = info.application_info.as_ref().map_or(-1, |a| a.uid);
        self.host.grant_implicit_access(caller, webview_uid)
    }

    fn notify_relro_creation_completed(&self, caller: i32) {
        if caller != SHARED_RELRO_UID as i32 && caller != SYSTEM_UID {
            return;
        }
        if relro_finished(&self.shared) {
            if let Err(e) = self.state() {
                eprintln!("app host: webviewupdate: {}", e.message);
            }
        }
    }
}

impl super::package_writes::PackageMonitor for WebViewUpdate {
    fn package_changed(
        &self,
        change: &super::package_writes::PackageChanged<'_>,
    ) -> Result<(), Exception> {
        if self
            .providers()?
            .iter()
            .any(|p| p.package == change.package)
        {
            drop(self.state()?);
        }
        Ok(())
    }
}

fn same_provider(
    before: Option<&crate::package::scan_snapshot::query_state::Capture>,
    after: &crate::package::scan_snapshot::query_state::Capture,
    old: Option<&PackageInfo>,
    new: Option<&PackageInfo>,
) -> bool {
    match (old, new) {
        (None, None) => true,
        (Some(old), Some(new)) => {
            let (mut a, mut b) = (Parcel::new(), Parcel::new());
            old.write_to(&mut a);
            new.write_to(&mut b);
            let code = |capture: &crate::package::scan_snapshot::query_state::Capture,
                        name: Option<&str>| {
                name.and_then(|name| capture.state().packages.get(name))
                    .and_then(|p| p.pkg.clone())
            };
            let same_code = before
                .and_then(|before| code(before, old.package_name.as_deref()))
                .zip(code(after, new.package_name.as_deref()))
                .is_some_and(|(a, b)| Arc::ptr_eq(&a, &b));
            a.data() == b.data() && same_code
        }
        _ => false,
    }
}

/// `notifyRelroCreationCompleted`: one more RELRO file creator finished.
/// Whether the provider was changed meanwhile and every RELRO is now done,
/// so that the change is applied (`onWebViewProviderChanged`'s deferral).
fn relro_finished(shared: &(Mutex<Option<State>>, Condvar)) -> bool {
    let mut guard = shared.0.lock().unwrap();
    let dirty = guard.as_mut().is_some_and(|s| {
        s.relros_finished += 1;
        s.dirty && s.relros_started == s.relros_finished
    });
    shared.1.notify_all();
    dirty
}

/// `webViewIsReadyLocked`.
fn ready(s: &State) -> bool {
    !s.dirty && s.relros_started == s.relros_finished && s.any_installed
}

/// The state once the provider is ready, waiting for it at most
/// `WAIT_TIMEOUT` (`waitForAndGetProvider`'s wait).
fn wait_ready(
    shared: &(Mutex<Option<State>>, Condvar),
) -> std::sync::MutexGuard<'_, Option<State>> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut guard = shared.0.lock().unwrap();
    while guard.as_ref().is_some_and(|s| !ready(s)) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        guard = shared.1.wait_timeout(guard, deadline - now).unwrap().0;
    }
    guard
}

/// `WebViewFactory.getWebViewLibrary`: its meta-data's library.
fn library(info: &PackageInfo) -> Option<String> {
    let meta = info.application_info.as_ref()?.item.meta_data.as_ref()?;
    match meta.0.iter().find(|(k, _)| k == WEBVIEW_LIBRARY)? {
        (_, crate::package::pkg::Value::String(Some(s))) => Some(s.clone()),
        _ => None,
    }
}

/// `WebViewProviderResponse`.
struct Response(Option<PackageInfo>, i32);

impl WriteParcelable for Response {
    fn write_to(&self, p: &mut Parcel) {
        match &self.0 {
            Some(info) => {
                p.write_i32(1);
                info.write_to(p);
            }
            None => p.write_i32(0),
        }
        p.write_i32(self.1);
    }
}

impl Service for WebViewUpdate {
    fn descriptor(&self) -> &str {
        wv::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        match call.code {
            wv::NOTIFY_RELRO_CREATION_COMPLETED => {
                wv::NotifyRelroCreationCompleted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.notify_relro_creation_completed(uid);
                wv::write_notify_relro_creation_completed_reply(&mut p);
            }
            wv::WAIT_FOR_AND_GET_PROVIDER => {
                wv::WaitForAndGetProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.wait_for_and_get_provider(uid) {
                    Ok((info, status)) => wv::write_wait_for_and_get_provider_reply(
                        &mut p,
                        Some(&Response(info, status)),
                    ),
                    Err(e) => return reply(Err(e)),
                }
            }
            wv::GET_CURRENT_WEB_VIEW_PACKAGE | wv::GET_CURRENT_WEB_VIEW_PACKAGE_NAME => {
                call.data
                    .enforce_interface(wv::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let current = match self.state() {
                    Ok(s) => s.as_ref().unwrap().current.clone(),
                    Err(e) => return reply(Err(e)),
                };
                if let Some(info) = &current
                    && let Err(e) = self.grant_visibility(info, uid)
                {
                    return reply(Err(e));
                }
                p.write_no_exception();
                if call.code == wv::GET_CURRENT_WEB_VIEW_PACKAGE_NAME {
                    p.write_string16(current.as_ref().and_then(|i| i.package_name.as_deref()));
                } else {
                    match &current {
                        Some(info) => {
                            p.write_i32(1);
                            info.write_to(&mut p);
                        }
                        None => p.write_i32(0),
                    }
                }
            }
            wv::GET_ALL_WEB_VIEW_PACKAGES | wv::GET_VALID_WEB_VIEW_PACKAGES => {
                call.data
                    .enforce_interface(wv::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                let providers = (|| {
                    let all = self.providers()?.to_vec();
                    if call.code == wv::GET_ALL_WEB_VIEW_PACKAGES {
                        return Ok(all);
                    }
                    let capture = self.host.package_capture()?;
                    let capture = capture.as_ref();
                    let mut valid = Vec::new();
                    for provider in all {
                        if let Some(info) =
                            self.package_info(capture, &provider.package, PACKAGE_FLAGS)?
                            && self.valid(capture, &provider, &info)?
                        {
                            valid.push(provider);
                        }
                    }
                    Ok(valid)
                })();
                match providers {
                    Ok(providers) => {
                        p.write_no_exception();
                        p.write_i32(providers.len() as i32);
                        for provider in &providers {
                            p.write_i32(1);
                            provider.write_to(&mut p);
                        }
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            wv::GET_DEFAULT_WEB_VIEW_PACKAGE => {
                call.data
                    .enforce_interface(wv::DESCRIPTOR)
                    .map_err(|_| BAD_VALUE)?;
                match self.default_provider() {
                    Ok(provider) => {
                        p.write_no_exception();
                        p.write_i32(1);
                        provider.write_to(&mut p);
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                let name = wv::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                return reply(Err(self
                    .host
                    .unimplemented_named("webviewupdate", &format!("{name} (#1491)"))));
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn provider_removal_during_relro_creation_reselects_after_completion() {
        crate::system::with_discovery_session(|host, publish| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    pkg.target_sdk_version = 36;
                    pkg.meta_data = Some(crate::package::pkg::MetaData(vec![(
                        WEBVIEW_LIBRARY.into(),
                        crate::package::pkg::Value::String(Some("libwebview.so".into())),
                    )]));
                }
            });
            let service = WebViewUpdate::new(host.clone());
            service
                .providers
                .set(vec![Provider {
                    package: "p".into(),
                    description: "provider".into(),
                    available_by_default: true,
                    is_fallback: false,
                    signatures: Vec::new(),
                }])
                .ok()
                .unwrap();
            let before = host.package_capture().unwrap();
            let current = service
                .preferred(&before)
                .unwrap()
                .expect("installed provider");
            *service.shared.0.lock().unwrap() = Some(State {
                capture: Some(before),
                current: Some(current),
                relros_started: 1,
                relros_finished: 0,
                any_installed: true,
                dirty: false,
            });
            publish(&mut |pkg| pkg.booleans &= !crate::package::pkg::booleans::ENABLED);
            let held = service.state().unwrap();
            assert!(held.as_ref().unwrap().dirty);
            assert!(held.as_ref().unwrap().current.is_some());
            drop(held);
            service.notify_relro_creation_completed(SHARED_RELRO_UID as i32);
            let held = service.state().unwrap();
            let state = held.as_ref().unwrap();
            assert!(!state.dirty);
            assert!(!state.any_installed);
            assert!(state.current.is_none());
        });
    }
    use super::*;

    #[test]
    fn providers_are_parcelled_as_webview_provider_info() {
        let mut p = Parcel::new();
        Provider {
            package: "a.b".into(),
            description: "B".into(),
            available_by_default: true,
            is_fallback: false,
            signatures: vec![vec![1, 2]],
        }
        .write_to(&mut p);
        let mut expected = Parcel::new();
        expected.write_string16(Some("a.b"));
        expected.write_string16(Some("B"));
        expected.write_i32(1);
        expected.write_i32(0);
        expected.write_i32(1);
        expected.write_i32(1);
        expected.write_i32(2);
        expected.write_raw(&[1, 2, 0, 0], &[]);
        assert_eq!(p.data(), expected.data());
    }
}
