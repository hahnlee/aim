//! `safety_center`: SafetyCenterService for the app host
//! (packages/modules/Permission at `android-16.0.0_r1`): `onStart`'s setup,
//! done when first asked (the device's support from
//! `config_enableSafetyCenter`, the resources APK of the Permission APEX
//! as `SafetyCenterResourcesApk` finds it, its configuration as
//! `SafetyCenterConfigReader.loadConfig` reads it), `isSafetyCenterEnabled`
//! and `getSafetyCenterConfig`; `setSafetySourceData` checked and taken
//! while Safety Center is disabled. On this image the configuration loads
//! (the device's SDK picks `raw-v36/safety_center_config.xml`, which gives
//! the private profile titles `private_profile_title_api` requires), so it
//! is enabled. The sources' data, refreshes, issues, listeners and
//! notifications are not implemented (#1785).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::ReadParcelable;
use aim_service_aidl::android_safetycenter_isafetycentermanager as sc;

use super::safety_center_config::{self as config, Flags, SafetyCenterConfig, StringResources, Written};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::clip::char_sequence;
use crate::package::query::Query;
use crate::package::component_resolver::Info;
use crate::package::intent::Intent;
use crate::package::parse::resources::{Resources, Table};

const SYSTEM_UID: i32 = 1000;
const READ_SAFETY_CENTER_STATUS: &str = "android.permission.READ_SAFETY_CENTER_STATUS";
const SEND_SAFETY_CENTER_UPDATE: &str = "android.permission.SEND_SAFETY_CENTER_UPDATE";
const MANAGE_SAFETY_CENTER: &str = "android.permission.MANAGE_SAFETY_CENTER";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
/// `SafetyCenterResourcesApk`'s intent action and the Permission APEX.
const RESOURCES_APK_ACTION: &str = "com.android.safetycenter.intent.action.SAFETY_CENTER_RESOURCES_APK";
const APEX_MODULE_PATH: &str = "/apex/com.android.permission";
/// `PackageManager.MATCH_SYSTEM_ONLY`.
const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
/// The aconfig flags of `com.android.permission.flags.Flags` it reads.
const ENABLED_NO_DEVICE_CONFIG: &str = "com.android.permission.flags.safety_center_enabled_no_device_config";
const PRIVATE_PROFILE_TITLE_API: &str = "com.android.permission.flags.private_profile_title_api";

/// A `writeTypedObject`'s null marker: whether a value follows.
fn present(r: &mut Reader<'_>) -> ParcelResult<bool> {
    Ok(r.read_i32()? != 0)
}

/// A `writeTypedList` of values `item` reads.
fn typed_list(r: &mut Reader<'_>, mut item: impl FnMut(&mut Reader<'_>) -> ParcelResult<()>) -> ParcelResult<()> {
    for _ in 0..r.read_i32()?.max(0) {
        if present(r)? {
            item(r)?;
        }
    }
    Ok(())
}

/// A `PendingIntent` as a typed object: its target's binder.
fn pending_intent(r: &mut Reader<'_>) -> ParcelResult<()> {
    if present(r)? {
        r.read_binder()?;
    }
    Ok(())
}

/// `SafetySourceData` as its `writeToParcel` writes it (Upside Down Cake
/// and later), read past: the service does not keep it (#1785).
struct SourceData;

impl ReadParcelable for SourceData {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // `SafetySourceStatus`, its `IconAction`.
        if present(r)? {
            char_sequence(r)?;
            char_sequence(r)?;
            r.read_i32()?;
            pending_intent(r)?;
            if present(r)? {
                r.read_i32()?;
                pending_intent(r)?;
            }
            r.read_bool()?;
        }
        // `SafetySourceIssue`s.
        typed_list(r, |r| {
            r.read_string16()?;
            for _ in 0..3 {
                char_sequence(r)?;
            }
            r.read_i32()?;
            r.read_i32()?;
            typed_list(r, action)?;
            pending_intent(r)?;
            r.read_string16()?;
            // Its custom `Notification`.
            if present(r)? {
                char_sequence(r)?;
                char_sequence(r)?;
                typed_list(r, action)?;
            }
            r.read_i32()?;
            char_sequence(r)?;
            r.read_string16()?;
            r.read_i32()?;
            Ok(())
        })?;
        // `mExtras`: `readBundle`'s length, then its magic, data and
        // whether it holds an intent.
        let length = r.read_i32()?;
        if length > 0 {
            r.read_i32()?;
            r.skip(length as usize)?;
            r.read_bool()?;
        }
        Ok(SourceData)
    }
}

/// A `SafetySourceIssue.Action`, its `ConfirmationDialogDetails`.
fn action(r: &mut Reader<'_>) -> ParcelResult<()> {
    r.read_string16()?;
    char_sequence(r)?;
    pending_intent(r)?;
    r.read_bool()?;
    char_sequence(r)?;
    if present(r)? {
        for _ in 0..4 {
            char_sequence(r)?;
        }
    }
    Ok(())
}

/// A `SafetyEvent`, of which only its presence matters here.
struct Event;

impl ReadParcelable for Event {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        r.read_i32()?;
        for _ in 0..3 {
            r.read_string16()?;
        }
        Ok(Event)
    }
}

/// What `onStart` leaves: the configuration when Safety Center can be
/// used (`canUseSafetyCenter`).
struct Started {
    config: Option<SafetyCenterConfig>,
    flags: Flags,
}

/// The resources APK's context: its strings over the framework's.
struct ApkStrings<'a>(Resources<'a>);

impl StringResources for ApkStrings<'_> {
    fn identifier(&self, package: &str, kind: &str, entry: &str) -> Option<u32> {
        self.0.identifier(package, kind, entry)
    }

    fn string(&self, id: u32) -> Result<String, String> {
        self.0.resource_string(id).ok_or_else(|| format!("String resource ID #{id:#x}"))
    }
}

pub struct SafetyCenter {
    host: Arc<AppHost>,
    started: OnceLock<Started>,
}

impl SafetyCenter {
    pub fn new(host: Arc<AppHost>) -> Self {
        SafetyCenter { host, started: OnceLock::new() }
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
        let flags = Flags { private_profile_title_api: self.flag(PRIVATE_PROFILE_TITLE_API)? };
        let supported = self.host.inputs()?.packages.framework_bool("config_enableSafetyCenter").unwrap_or(false);
        let config = if supported { self.load_config(flags)? } else {
            eprintln!("app host: safety_center: Device does not support Safety Center, it will be disabled");
            None
        };
        Ok(self.started.get_or_init(|| Started { config, flags }))
    }

    /// `SafetyCenterResourcesApk.init` and `SafetyCenterConfigReader.loadConfig`:
    /// none when either fails, as logged.
    fn load_config(&self, flags: Flags) -> Result<Option<SafetyCenterConfig>, Exception> {
        let Some((package, apk)) = self.resources_apk()? else {
            eprintln!("app host: safety_center: Cannot init Safety Center resources, Safety Center will be disabled");
            return Ok(None);
        };
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("safety_center: {}: {e}", apk.display()));
        let apk_file = aim_apps::apk::Apk::open(&apk).map_err(|e| state(e.to_string()))?;
        let table = match apk_file.file_if_present("resources.arsc").map_err(|e| state(e.to_string()))? {
            Some(bytes) => Table::parse(&bytes).map_err(|e| state(e.to_string()))?,
            None => return Err(state("no resources.arsc".into())),
        };
        let inputs = self.host.inputs()?;
        let resources = inputs.packages.package_resources(&table);
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
            eprintln!("app host: safety_center: Cannot init Safety Center config, Safety Center will be disabled");
            return Ok(None);
        };
        match config::parse(&xml, &ApkStrings(resources), flags) {
            Ok(config) => Ok(Some(config)),
            Err(e) => {
                eprintln!("app host: safety_center: Cannot parse SafetyCenterConfig: {e}");
                eprintln!("app host: safety_center: Cannot init Safety Center config, Safety Center will be disabled");
                Ok(None)
            }
        }
    }

    /// `loadResourcesApkContext`: the first activity of the resources
    /// APK's action, among the system's, that lives in the Permission
    /// APEX; its package and its APK on the host.
    fn resources_apk(&self) -> Result<Option<(String, PathBuf)>, Exception> {
        let (_, resolution) = self.host.packages()?;
        let intent = Intent { action: Some(RESOURCES_APK_ACTION.into()), ..Default::default() };
        let found = resolution
            .query_intent_activities(&intent, None, MATCH_SYSTEM_ONLY, 0, SYSTEM_UID)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("safety_center: resources APK: {e:?}")))?;
        let found = found.into_iter().find_map(|r| match r.info {
            Info::Activity(a) => {
                let app = &a.info.application_info;
                let source = app.source_dir.clone().filter(|d| d.starts_with(APEX_MODULE_PATH))?;
                Some((app.item.package_name.clone()?, source))
            }
            _ => None,
        });
        let Some((package, source)) = found else { return Ok(None) };
        let relative = source.trim_start_matches('/');
        let path = match relative.strip_prefix("data/") {
            Some(rest) => self.host.data()?.join(rest),
            None => self.host.root()?.join(relative),
        };
        Ok(Some((package, path)))
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
        Ok(match self.host.inputs()?.settings.device_config("privacy", "safety_center_is_enabled") {
            Some(v) => v.eq_ignore_ascii_case("true"),
            None => true,
        })
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
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        let actual = query.package_uid(package, 0, user).map_err(|e| self.host.unimplemented_named("safety_center", e.0))??;
        if actual < 0 {
            return Ok(false);
        }
        if matches!(uid, 0 | SYSTEM_UID) {
            return Ok(true);
        }
        if uid % PER_USER_RANGE as i32 != actual % PER_USER_RANGE as i32 {
            return Err(Exception::security(format!("Package: {package}, does not belong to calling uid: {uid}")));
        }
        Ok(true)
    }

    /// `setSafetySourceData`: checked, then nothing while Safety Center is
    /// disabled (`checkApiEnabled`).
    fn set_source_data(&self, uid: i32, a: &sc::SetSafetySourceData<SourceData, Event>) -> Result<(), Exception> {
        let null = || Exception::new(EX_NULL_POINTER, "");
        a.source_id.as_ref().ok_or_else(null)?;
        a.safety_event.as_ref().ok_or_else(null)?;
        let package = a.package_name.as_deref().ok_or_else(null)?;
        if !self.held(SEND_SAFETY_CENTER_UPDATE, uid)? {
            return Err(Exception::security(format!(
                "setSafetySourceData: Neither user {uid} nor current process has {SEND_SAFETY_CENTER_UPDATE}."
            )));
        }
        if !self.cross_user("setSafetySourceData", uid, a.user_id)? || !self.package(uid, package, a.user_id)? || !self.api_enabled()? {
            return Ok(());
        }
        Err(self.host.unimplemented_named("safety_center", "the sources' data (#1785)"))
    }
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
                    if !self.held(READ_SAFETY_CENTER_STATUS, uid)? && !self.held(SEND_SAFETY_CENTER_UPDATE, uid)? {
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
                let a = sc::SetSafetySourceData::<SourceData, Event>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_source_data(uid, &a).map(|()| sc::write_set_safety_source_data_reply(&mut p))
            }
            code => {
                let name = sc::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("safety_center", &format!("{name} (#1785)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::write_char_sequence;

    /// `SafetySourceData.writeToParcel` of a status with an icon action
    /// and an issue with an action, a confirmation dialog and a custom
    /// notification, then its extras: read to its end.
    #[test]
    fn source_data_is_read_past_as_it_is_written() {
        let mut p = Parcel::new();
        let text = |p: &mut Parcel, s: &str| write_char_sequence(p, Some(s));
        let no_intent = |p: &mut Parcel| p.write_i32(0);
        let action = |p: &mut Parcel| {
            p.write_i32(1);
            p.write_string16(Some("a"));
            text(p, "Fix");
            no_intent(p);
            p.write_bool(true);
            text(p, "Done");
            p.write_i32(1);
            for t in ["T", "X", "Yes", "No"] {
                text(p, t);
            }
        };
        p.write_i32(1); // status
        text(&mut p, "Title");
        text(&mut p, "Summary");
        p.write_i32(300);
        no_intent(&mut p);
        p.write_i32(1); // icon action
        p.write_i32(100);
        no_intent(&mut p);
        p.write_bool(true);
        p.write_i32(1); // one issue
        p.write_i32(1);
        p.write_string16(Some("issue"));
        for t in ["T", "S", "Sum"] {
            text(&mut p, t);
        }
        p.write_i32(300);
        p.write_i32(100);
        p.write_i32(1);
        action(&mut p);
        no_intent(&mut p);
        p.write_string16(Some("type"));
        p.write_i32(1); // custom notification
        text(&mut p, "N");
        text(&mut p, "Body");
        p.write_i32(0);
        p.write_i32(0);
        text(&mut p, "Attribution");
        p.write_string16(None);
        p.write_i32(100);
        crate::bundle::write(&mut p, &[("k", crate::bundle::Value::Int(1))]);
        p.write_i32(42);
        let mut r = p.reader();
        SourceData::read_from(&mut r).unwrap();
        assert_eq!(r.read_i32().unwrap(), 42);
    }
}
