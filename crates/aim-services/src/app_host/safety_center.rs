//! `safety_center`: SafetyCenterService for the app host
//! (packages/modules/Permission at `android-16.0.0_r1`): `onStart`'s setup,
//! done when first asked (the device's support from
//! `config_enableSafetyCenter`, the resources APK of the Permission APEX
//! as `SafetyCenterResourcesApk` finds it, its configuration as
//! `SafetyCenterConfigReader.loadConfig` reads it), `isSafetyCenterEnabled`
//! and `getSafetyCenterConfig`; the sources' data (`setSafetySourceData`,
//! `getSafetySourceData`, `reportSafetySourceError`) with the original's
//! checks, kept by the data manager (`app_host/safety_center_data.rs`).
//! On this image the configuration loads (the device's SDK picks
//! `raw-v36/safety_center_config.xml`, which gives the private profile
//! titles `private_profile_title_api` requires), so it is enabled.
//! Refreshes, issue actions and dismissals, `getSafetyCenterData` and its
//! listeners, posting notifications and the for-tests calls are not
//! implemented (#1785).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_safetycenter_isafetycentermanager as sc;

use super::safety_center_config::{
    self as config, Flags, SafetyCenterConfig, StringResources, Written,
};
use super::safety_center_data::{self as data, Data, ErrorDetails, Event, SourceData};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::component_resolver::Info;
use crate::package::intent::Intent;
use crate::package::parse::resources::{Resources, Table};
use crate::package::query::Query;

const SYSTEM_UID: i32 = 1000;
const READ_SAFETY_CENTER_STATUS: &str = "android.permission.READ_SAFETY_CENTER_STATUS";
const SEND_SAFETY_CENTER_UPDATE: &str = "android.permission.SEND_SAFETY_CENTER_UPDATE";
const MANAGE_SAFETY_CENTER: &str = "android.permission.MANAGE_SAFETY_CENTER";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
/// `SafetyCenterResourcesApk`'s intent action and the Permission APEX.
const RESOURCES_APK_ACTION: &str =
    "com.android.safetycenter.intent.action.SAFETY_CENTER_RESOURCES_APK";
const APEX_MODULE_PATH: &str = "/apex/com.android.permission";
/// `PackageManager.MATCH_SYSTEM_ONLY`.
const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
/// The aconfig flags of `com.android.permission.flags.Flags` it reads.
const ENABLED_NO_DEVICE_CONFIG: &str =
    "com.android.permission.flags.safety_center_enabled_no_device_config";
const PRIVATE_PROFILE_TITLE_API: &str = "com.android.permission.flags.private_profile_title_api";
const ODAD_NOTIFICATIONS_SUPPORTED: &str =
    "com.android.permission.flags.odad_notifications_supported";
/// `PackageManager.CERT_INPUT_SHA256`.
const CERT_INPUT_SHA256: i32 = 1;

/// What `onStart` leaves: the configuration when Safety Center can be
/// used (`canUseSafetyCenter`).
struct Started {
    config: Option<SafetyCenterConfig>,
    flags: Flags,
    /// `SafetyCenterFlags.init`'s defaults from the resources APK:
    /// `config_defaultIssueCategoryAllowlist` and
    /// `config_defaultActionsToOverrideWithDefaultIntent`.
    defaults: (String, String),
}

/// The resources APK's context: its strings over the framework's.
struct ApkStrings<'a>(Resources<'a>);

impl StringResources for ApkStrings<'_> {
    fn identifier(&self, package: &str, kind: &str, entry: &str) -> Option<u32> {
        self.0.identifier(package, kind, entry)
    }

    fn string(&self, id: u32) -> Result<String, String> {
        self.0
            .resource_string(id)
            .ok_or_else(|| format!("String resource ID #{id:#x}"))
    }
}

pub struct SafetyCenter {
    host: Arc<AppHost>,
    started: OnceLock<Started>,
    /// `SafetyCenterDataManager`'s state, once loaded.
    data: Mutex<Option<Data>>,
}

impl SafetyCenter {
    pub fn new(host: Arc<AppHost>) -> Self {
        SafetyCenter {
            host,
            started: OnceLock::new(),
            data: Mutex::new(None),
        }
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    /// `onStart`.
    fn started(&self) -> Result<&Started, Exception> {
        if let Some(started) = self.started.get() {
            return Ok(started);
        }
        let flags = Flags {
            private_profile_title_api: self.flag(PRIVATE_PROFILE_TITLE_API)?,
        };
        let supported = self
            .host
            .framework()?
            .framework_bool("config_enableSafetyCenter")
            .unwrap_or(false);
        let (config, defaults) = if supported {
            self.load_config(flags)?
        } else {
            eprintln!(
                "app host: safety_center: Device does not support Safety Center, it will be disabled"
            );
            (None, Default::default())
        };
        Ok(self.started.get_or_init(|| Started {
            config,
            flags,
            defaults,
        }))
    }

    /// `SafetyCenterResourcesApk.init` and `SafetyCenterConfigReader.loadConfig`:
    /// none when either fails, as logged.
    fn load_config(
        &self,
        flags: Flags,
    ) -> Result<(Option<SafetyCenterConfig>, (String, String)), Exception> {
        let Some((package, apk)) = self.resources_apk()? else {
            eprintln!(
                "app host: safety_center: Cannot init Safety Center resources, Safety Center will be disabled"
            );
            return Ok((None, Default::default()));
        };
        let state = |e: String| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("safety_center: {}: {e}", apk.display()),
            )
        };
        let apk_file = aim_apps::apk::Apk::open(&apk).map_err(|e| state(e.to_string()))?;
        let table = match apk_file
            .file_if_present("resources.arsc")
            .map_err(|e| state(e.to_string()))?
        {
            Some(bytes) => Table::parse(&bytes).map_err(|e| state(e.to_string()))?,
            None => return Err(state("no resources.arsc".into())),
        };
        let inputs = self.host.inputs()?;
        let resources = inputs.packages.package_resources(&table);
        // `getOptionalStringByName`.
        let string = |name: &str| {
            resources
                .identifier(&package, "string", name)
                .and_then(|id| resources.resource_string(id))
                .unwrap_or_default()
        };
        let defaults = (
            string("config_defaultIssueCategoryAllowlist"),
            string("config_defaultActionsToOverrideWithDefaultIntent"),
        );
        // `getResIdAndMaybeThrowIfNull("safety_center_config", "raw")`,
        // falling back to none.
        let Some(xml) = resources
            .identifier(&package, "raw", "safety_center_config")
            .and_then(|id| resources.resource_string(id))
            .map(|path| apk_file.file_if_present(&path))
            .transpose()
            .map_err(|e| state(e.to_string()))?
            .flatten()
        else {
            eprintln!("app host: safety_center: Cannot access Safety Center config file");
            eprintln!(
                "app host: safety_center: Cannot init Safety Center config, Safety Center will be disabled"
            );
            return Ok((None, defaults));
        };
        match config::parse(&xml, &ApkStrings(resources), flags) {
            Ok(config) => Ok((Some(config), defaults)),
            Err(e) => {
                eprintln!("app host: safety_center: Cannot parse SafetyCenterConfig: {e}");
                eprintln!(
                    "app host: safety_center: Cannot init Safety Center config, Safety Center will be disabled"
                );
                Ok((None, defaults))
            }
        }
    }

    /// `loadResourcesApkContext`.
    fn resources_apk(&self) -> Result<Option<(String, PathBuf)>, Exception> {
        resources_apk(
            &self.host,
            "safety_center",
            RESOURCES_APK_ACTION,
            APEX_MODULE_PATH,
        )
    }

    /// `canUseSafetyCenter`.
    fn usable(&self) -> Result<Option<(&SafetyCenterConfig, Flags)>, Exception> {
        let started = self.started()?;
        Ok(started.config.as_ref().map(|c| (c, started.flags)))
    }

    /// `isApiEnabled`: usable and `SafetyCenterFlags.getSafetyCenterEnabled`
    /// (always from Upside Down Cake with the no-device-config flag, else
    /// DeviceConfig `privacy/safety_center_is_enabled`, on by default).
    fn api_enabled(&self) -> Result<bool, Exception> {
        if self.usable()?.is_none() {
            return Ok(false);
        }
        if self.flag(ENABLED_NO_DEVICE_CONFIG)? {
            return Ok(true);
        }
        // `DeviceConfig.getBoolean`: `Boolean.parseBoolean` of a value set.
        Ok(
            match self
                .host
                .inputs()?
                .settings
                .device_config("privacy", "safety_center_is_enabled")
            {
                Some(v) => v.eq_ignore_ascii_case("true"),
                None => true,
            },
        )
    }

    fn held(&self, permission: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), uid)? == 0)
    }

    /// `enforceCrossUserPermission(message, user)`: false for a user that
    /// does not exist (each existing user of the app host is a supported
    /// profile group's).
    fn cross_user(&self, message: &str, uid: i32, user: i32) -> Result<bool, Exception> {
        if user != uid / PER_USER_RANGE as i32 {
            if user < 0 {
                return Err(Exception::illegal_argument(format!("Invalid user {user}")));
            }
            if !self.held(INTERACT_ACROSS_USERS_FULL, uid)? {
                return Err(Exception::security(format!(
                    "{message}: Neither user {uid} nor current process has {INTERACT_ACROSS_USERS_FULL}."
                )));
            }
        }
        let (state, _) = self.host.packages()?;
        Ok(state.users.contains_key(&user))
    }

    /// `enforcePackage`: false for a package the user does not have.
    fn package(&self, uid: i32, package: &str, user: i32) -> Result<bool, Exception> {
        if package.is_empty() {
            return Err(Exception::illegal_argument("packageName may not be empty"));
        }
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let actual = query
            .package_uid(package, 0, user)
            .map_err(|e| self.host.unimplemented_named("safety_center", e.0))??;
        if actual < 0 {
            return Ok(false);
        }
        if matches!(uid, 0 | SYSTEM_UID) {
            return Ok(true);
        }
        if uid % PER_USER_RANGE as i32 != actual % PER_USER_RANGE as i32 {
            return Err(Exception::security(format!(
                "Package: {package}, does not belong to calling uid: {uid}"
            )));
        }
        Ok(true)
    }

    /// The data manager's state, its issues loaded from their file first
    /// (`loadPersistableDataStateFromFile`), written back as `load` asks.
    fn data(
        &self,
        config: &SafetyCenterConfig,
    ) -> Result<std::sync::MutexGuard<'_, Option<Data>>, Exception> {
        let mut guard = self.data.lock().unwrap();
        if guard.is_none() {
            let root = self.host.data()?;
            let file = root.join(data::ISSUES_FILE);
            let persisted = match super::device_policy::read_atomic_bytes(&file) {
                Ok(None) => Ok(Vec::new()),
                Ok(Some(bytes)) => data::read_issues(&bytes),
                Err(e) => Err(e),
            };
            let persisted = persisted.unwrap_or_else(|e| {
                eprintln!(
                    "app host: safety_center: Cannot read Safety Center persisted issues: {}: {e}",
                    file.display()
                );
                Vec::new()
            });
            let mut loaded = Data::default();
            loaded
                .load(persisted, config)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("safety_center: {e}")))?;
            self.write_issues(&loaded)?;
            *guard = Some(loaded);
        }
        Ok(guard)
    }

    fn write_issues(&self, d: &Data) -> Result<(), Exception> {
        data::write_issues_file(&self.host.data()?, &d.issues_file()).map_err(|e| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("safety_center: {}: {e}", data::ISSUES_FILE),
            )
        })
    }

    /// `SafetyCenterFlags` of the started service.
    fn with_flags<T>(
        &self,
        started: &Started,
        f: impl FnOnce(&data::Flags<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let settings = &self.host.inputs()?.settings;
        let privacy = |name: &str| settings.device_config("privacy", name);
        let flags = data::Flags {
            privacy: &privacy,
            issue_category_allowlist_default: &started.defaults.0,
            actions_to_override_default: &started.defaults.1,
            odad_notifications_supported: self.flag(ODAD_NOTIFICATIONS_SUPPORTED)?,
        };
        f(&flags)
    }

    /// `SafetySourceDataValidator.validateRequest`: the source the request
    /// is for, or none for a request ignored.
    fn validate<'c>(
        &self,
        config: &'c SafetyCenterConfig,
        flags: &data::Flags<'_>,
        uid: i32,
        source_data: Option<&SourceData>,
        any_source: bool,
        id: &str,
        package: &str,
        user: i32,
    ) -> Result<data::External<'c>, Exception> {
        let illegal = |m: String| Exception::illegal_argument(m);
        let external = data::external(config, id)
            .ok_or_else(|| illegal(format!("Unexpected safety source: {id}")))?;
        let source = external.source;
        if !any_source {
            // `validateCallingPackage`.
            if source.package_name.as_deref() != Some(package) {
                return Err(illegal(format!(
                    "Unexpected package name: {package}, for safety source: {id}"
                )));
            }
            if !source.package_certificate_hashes.is_empty()
                && !self.has_certificate(uid, package, &source.package_certificate_hashes)?
                && !self.has_certificate(uid, package, &flags.additional_package_certs(package))?
            {
                return Err(illegal(format!("Invalid signature for package {package}")));
            }
        }
        // `getProfileTypeOfUser`: the app host's users are not profiles.
        if user != 0 {
            return Err(self.host.unimplemented_named(
                "safety_center",
                "the sources' data of another user (#1785)",
            ));
        }
        if !data::supports_profile(source, true) {
            return Err(illegal(format!(
                "Unexpected profile type: 0 for safety source: {id}"
            )));
        }
        data::validate(source_data, &external, flags).map_err(illegal)?;
        Ok(external)
    }

    /// `checkCerts`: `hasSigningCertificate(package, hash, CERT_INPUT_SHA256)`
    /// of one of `hashes`.
    fn has_certificate(
        &self,
        uid: i32,
        package: &str,
        hashes: &[String],
    ) -> Result<bool, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let mut matched = false;
        for hash in hashes {
            // `new Signature(hash).toByteArray()`.
            let bytes = (hash.len() % 2 == 0)
                .then(|| {
                    (0..hash.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&hash[i..i + 2], 16).ok())
                        .collect::<Option<Vec<u8>>>()
                })
                .flatten()
                .ok_or_else(|| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("Failed to parse signing certificate: {hash}"),
                    )
                })?;
            if query
                .package_certificate(package, Some(&bytes), CERT_INPUT_SHA256)
                .map_err(|e| self.host.unimplemented_named("safety_center", e.0))?
            {
                matched = true;
            }
        }
        Ok(matched)
    }

    /// `SafetyCenterDataChangeNotifier.updateDataConsumers` of a user:
    /// `SafetyCenterNotificationSender.updateNotifications` (no listener is
    /// registered).
    fn update_consumers(
        &self,
        d: &mut Data,
        config: &SafetyCenterConfig,
        flags: &data::Flags<'_>,
        user: i32,
    ) -> Result<(), Exception> {
        if !flags.notifications_enabled() {
            return Ok(());
        }
        if !d.issues_to_notify(user, config, flags).is_empty() {
            return Err(self.host.unimplemented_named(
                "safety_center",
                "posting Safety Center notifications (#1785)",
            ));
        }
        Ok(())
    }

    /// The checks before the data manager of `setSafetySourceData` and
    /// `reportSafetySourceError`: whether to go on.
    fn checked(
        &self,
        message: &str,
        uid: i32,
        package: &str,
        user: i32,
    ) -> Result<bool, Exception> {
        if !self.held(SEND_SAFETY_CENTER_UPDATE, uid)? {
            return Err(Exception::security(format!(
                "{message}: Neither user {uid} nor current process has {SEND_SAFETY_CENTER_UPDATE}."
            )));
        }
        Ok(self.cross_user(message, uid, user)?
            && self.package(uid, package, user)?
            && self.api_enabled()?)
    }

    /// `setSafetySourceData`: checked, validated, kept; the issues and
    /// their consumers updated on a change.
    fn set_source_data(
        &self,
        uid: i32,
        a: sc::SetSafetySourceData<SourceData, Event>,
    ) -> Result<(), Exception> {
        let null = || Exception::new(EX_NULL_POINTER, "");
        if let Some(d) = &a.safety_source_data {
            d.build().map_err(Exception::illegal_argument)?;
        }
        let id = a.source_id.as_deref().ok_or_else(null)?;
        a.safety_event.as_ref().ok_or_else(null)?;
        let package = a.package_name.as_deref().ok_or_else(null)?;
        if !self.checked("setSafetySourceData", uid, package, a.user_id)? {
            return Ok(());
        }
        let started = self.started()?;
        let config = started
            .config
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "safety_center: no config"))?;
        self.with_flags(started, |flags| {
            let source_data = a.safety_source_data.as_ref();
            let external = self.validate(
                config,
                flags,
                uid,
                source_data,
                false,
                id,
                package,
                a.user_id,
            )?;
            // `SafetySourceDataFix`: the default action override needs a
            // PendingIntent of the source's intent action.
            let overridden = flags.actions_to_override(id);
            let overrides = source_data.is_some_and(|d| {
                d.issues.iter().any(|i| {
                    let notification = i.notification.iter().flat_map(|n| &n.actions);
                    i.actions
                        .iter()
                        .chain(notification)
                        .any(|act| act.id.as_ref().is_some_and(|x| overridden.contains(x)))
                })
            });
            if overrides && external.source.intent_action.is_some() {
                return Err(self
                    .host
                    .unimplemented_named("safety_center", "the default action override (#1785)"));
            }
            let mut guard = self.data(config)?;
            let d = guard.as_mut().unwrap();
            // `processSafetyEvent` changes nothing: no refresh is in
            // progress and no action in flight.
            let kept = a.safety_source_data.map(|s| {
                let strongs = s.retain(&self.host.process);
                (s, strongs)
            });
            let (changed, mut write) = d.set(id, a.user_id, kept);
            if changed {
                write |= d.update_issues(a.user_id, config, flags);
            }
            if write {
                self.write_issues(d)?;
            }
            if changed {
                self.update_consumers(d, config, flags, a.user_id)?;
            }
            Ok(())
        })
    }

    /// `getSafetySourceData`.
    fn source_data(
        &self,
        uid: i32,
        id: Option<&str>,
        package: Option<&str>,
        user: i32,
    ) -> Result<Option<SourceData>, Exception> {
        let null = || Exception::new(EX_NULL_POINTER, "");
        let id = id.ok_or_else(null)?;
        let package = package.ok_or_else(null)?;
        if !self.held(SEND_SAFETY_CENTER_UPDATE, uid)? && !self.held(MANAGE_SAFETY_CENTER, uid)? {
            return Err(Exception::security(format!(
                "getSafetySourceData requires any of: [{SEND_SAFETY_CENTER_UPDATE}, {MANAGE_SAFETY_CENTER}], but none were granted"
            )));
        }
        if !self.cross_user("getSafetySourceData", uid, user)?
            || !self.package(uid, package, user)?
            || !self.api_enabled()?
        {
            return Ok(None);
        }
        let started = self.started()?;
        let config = started
            .config
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "safety_center: no config"))?;
        let any_source = self.held(MANAGE_SAFETY_CENTER, uid)?;
        self.with_flags(started, |flags| {
            self.validate(config, flags, uid, None, any_source, id, package, user)?;
            let guard = self.data(config)?;
            let found = guard.as_ref().unwrap().source_data(id, user).cloned();
            if found.as_ref().is_some_and(|d| !d.extras_kept()) {
                return Err(self.host.unimplemented_named(
                    "safety_center",
                    "extras holding binders or files (#1785)",
                ));
            }
            Ok(found)
        })
    }

    /// `reportSafetySourceError`.
    fn report_error(
        &self,
        uid: i32,
        a: sc::ReportSafetySourceError<ErrorDetails>,
    ) -> Result<(), Exception> {
        let null = || Exception::new(EX_NULL_POINTER, "");
        let id = a.safety_source_id.as_deref().ok_or_else(null)?;
        let details = a.safety_source_error_details.as_ref().ok_or_else(null)?;
        let package = a.package_name.as_deref().ok_or_else(null)?;
        if !self.checked("reportSafetySourceError", uid, package, a.user_id)? {
            return Ok(());
        }
        let started = self.started()?;
        let config = started
            .config
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "safety_center: no config"))?;
        self.with_flags(started, |flags| {
            self.validate(config, flags, uid, None, false, id, package, a.user_id)?;
            let mut guard = self.data(config)?;
            let d = guard.as_mut().unwrap();
            if d.set_error(id, a.user_id, &details.0) {
                d.update_issues(a.user_id, config, flags);
                self.update_consumers(d, config, flags, a.user_id)?;
            }
            Ok(())
        })
    }
}

/// A module's resources APK: the first activity of its `action`, among the
/// system's, that lives in the module's APEX at `apex`; its package and its
/// APK on the host.
pub(super) fn resources_apk(
    host: &AppHost,
    service: &str,
    action: &str,
    apex: &str,
) -> Result<Option<(String, PathBuf)>, Exception> {
    let (_, resolution) = host.packages()?;
    let intent = Intent {
        action: Some(action.into()),
        ..Default::default()
    };
    let found = resolution
        .query_intent_activities(&intent, None, MATCH_SYSTEM_ONLY, 0, SYSTEM_UID)
        .map_err(|e| {
            Exception::new(EX_ILLEGAL_STATE, format!("{service}: resources APK: {e:?}"))
        })?;
    let found = found.into_iter().find_map(|r| match r.info {
        Info::Activity(a) => {
            let app = &a.info.application_info;
            let source = app.source_dir.clone().filter(|d| d.starts_with(apex))?;
            Some((app.item.package_name.clone()?, source))
        }
        _ => None,
    });
    let Some((package, source)) = found else {
        return Ok(None);
    };
    let relative = source.trim_start_matches('/');
    let path = match relative.strip_prefix("data/") {
        Some(rest) => host.data()?.join(rest),
        None => host.root()?.join(relative),
    };
    Ok(Some((package, path)))
}

impl Service for SafetyCenter {
    fn descriptor(&self) -> &str {
        sc::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            sc::IS_SAFETY_CENTER_ENABLED => {
                sc::IsSafetyCenterEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    // `enforceAnyCallingOrSelfPermissions`.
                    if !self.held(READ_SAFETY_CENTER_STATUS, uid)?
                        && !self.held(SEND_SAFETY_CENTER_UPDATE, uid)?
                    {
                        return Err(Exception::security(format!(
                            "isSafetyCenterEnabled requires any of: [{READ_SAFETY_CENTER_STATUS}, {SEND_SAFETY_CENTER_UPDATE}], but none were granted"
                        )));
                    }
                    sc::write_is_safety_center_enabled_reply(&mut p, self.api_enabled()?);
                    Ok(())
                })()
            }
            sc::GET_SAFETY_CENTER_CONFIG => {
                sc::GetSafetyCenterConfig::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    if !self.held(MANAGE_SAFETY_CENTER, uid)? {
                        return Err(Exception::security(format!(
                            "getSafetyCenterConfig: Neither user {uid} nor current process has {MANAGE_SAFETY_CENTER}."
                        )));
                    }
                    let written = self.usable()?.map(|(c, flags)| Written(c, flags));
                    sc::write_get_safety_center_config_reply(&mut p, written.as_ref());
                    Ok(())
                })()
            }
            sc::SET_SAFETY_SOURCE_DATA => {
                let a = sc::SetSafetySourceData::<SourceData, Event>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.set_source_data(uid, a)
                    .map(|()| sc::write_set_safety_source_data_reply(&mut p))
            }
            sc::GET_SAFETY_SOURCE_DATA => {
                let a = sc::GetSafetySourceData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.source_data(
                    uid,
                    a.safety_source_id.as_deref(),
                    a.package_name.as_deref(),
                    a.user_id,
                )
                .map(|d| sc::write_get_safety_source_data_reply(&mut p, d.as_ref()))
            }
            sc::REPORT_SAFETY_SOURCE_ERROR => {
                let a = sc::ReportSafetySourceError::<ErrorDetails>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.report_error(uid, a)
                    .map(|()| sc::write_report_safety_source_error_reply(&mut p))
            }
            code => {
                let name = sc::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("safety_center", &format!("{name} (#1785)")))
            }
        };
        reply(result.map(|()| p))
    }
}
