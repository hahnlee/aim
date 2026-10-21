//! `deviceidle`: DeviceIdleController for the app host
//! (`android-16.0.0_r1`, `apex/jobscheduler`): the power save allowlists
//! (the system's from SystemConfig's `allow-in-power-save` and
//! `allow-in-power-save-except-idle`, the user's and the system apps the
//! user removed from `/data/system/deviceidle.xml`), changed with
//! DEVICE_POWER and `ACTION_POWER_SAVE_WHITELIST_CHANGED` sent; the
//! temporary allowlist of app ids (CHANGE_DEVICE_IDLE_TEMP_WHITELIST),
//! which also allows the uid foreground service starts as
//! `updateDeviceIdleTempAllowlist` does, until its end; and `exitIdle`.
//! The device stays active: the app host's battery service reports the
//! Mac charging or its screen on, or auto power modes are off
//! (`config_enableAutoPowerModes`). The idle states a device on battery
//! with its screen off enters, the change broadcasts' delivery group and
//! deferral, and its shell command and dump are #2016.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::android_os_ideviceidlecontroller as di;

use super::kept::RawIntent;
use super::settings::Table;
use super::{AppHost, reply};
use crate::package::info::java_hash;

const PER_USER_RANGE: i32 = 100_000;
const DEVICE_POWER: &str = "android.permission.DEVICE_POWER";
const CHANGE_DEVICE_IDLE_TEMP_WHITELIST: &str =
    "android.permission.CHANGE_DEVICE_IDLE_TEMP_WHITELIST";
const ACTION_POWER_SAVE_WHITELIST_CHANGED: &str = "android.os.action.POWER_SAVE_WHITELIST_CHANGED";
const ACTION_POWER_SAVE_TEMP_WHITELIST_CHANGED: &str =
    "android.os.action.POWER_SAVE_TEMP_WHITELIST_CHANGED";
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
/// `PackageManager.MATCH_SYSTEM_ONLY`, `MATCH_ANY_USER`.
const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
const MATCH_ANY_USER: i64 = 0x0040_0000;
/// `PowerExemptionManager.TEMPORARY_ALLOW_LIST_TYPE_NONE`,
/// `TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED`.
const TEMPORARY_ALLOW_LIST_TYPE_NONE: i32 = -1;
pub(super) const TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED: i32 = 0;
/// `PowerExemptionManager.REASON_DENIED`, `REASON_PUSH_MESSAGING_OVER_QUOTA`.
const REASON_DENIED: i32 = -1;
const REASON_PUSH_MESSAGING_OVER_QUOTA: i32 = 200;
/// `writeConfigFileLocked`'s delay of `MSG_WRITE_CONFIG`.
const WRITE_CONFIG_DELAY: Duration = Duration::from_secs(5);

/// An `ArrayMap<String, Integer>` of packages and their app ids, in its
/// order: by the name's hash, then as added.
#[derive(Clone, Debug, Default, PartialEq)]
struct Apps(Vec<(String, i32)>);

impl Apps {
    fn get(&self, name: &str) -> Option<i32> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, a)| *a)
    }

    fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// `put`: the previous app id, if the name was there.
    fn put(&mut self, name: &str, app_id: i32) -> Option<i32> {
        if let Some(entry) = self.0.iter_mut().find(|(n, _)| n == name) {
            return Some(std::mem::replace(&mut entry.1, app_id));
        }
        let hash = java_hash(name);
        let at = self.0.partition_point(|(n, _)| java_hash(n) <= hash);
        self.0.insert(at, (name.to_string(), app_id));
        None
    }

    fn remove(&mut self, name: &str) -> Option<i32> {
        let at = self.0.iter().position(|(n, _)| n == name)?;
        Some(self.0.remove(at).1)
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(n, _)| n.as_str())
    }
}

/// `buildAppIdArray`: the app ids of `system` and `user`, ascending.
fn app_ids(system: Option<&Apps>, user: &Apps) -> Vec<i32> {
    let mut ids: Vec<i32> = system
        .into_iter()
        .flat_map(|a| a.0.iter())
        .chain(&user.0)
        .map(|(_, a)| *a)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[derive(Debug, Default, PartialEq)]
struct Lists {
    /// `mPowerSaveWhitelistAppsExceptIdle`.
    except_idle: Apps,
    /// `mPowerSaveWhitelistApps`.
    system: Apps,
    /// `mPowerSaveWhitelistUserApps`.
    user: Apps,
    /// `mRemovedFromSystemWhitelistApps`.
    removed: Apps,
    /// `mTempWhitelistAppIdEndTimes`, by app id.
    temp: BTreeMap<i32, (Instant, Option<String>)>,
}

impl Lists {
    /// `onStart`'s system lists, from SystemConfig's packages that are
    /// installed system apps (with their app ids), then
    /// `readConfigFileLocked` of `config`: the user's packages that are
    /// installed, and the system's the user removed.
    fn new(
        except_idle: &[String],
        allow: &[String],
        system_apps: &dyn Fn(&str) -> Option<i32>,
        installed: &dyn Fn(&str) -> Option<i32>,
        config: Option<&aim_android_xml::Element>,
    ) -> Self {
        let mut lists = Lists::default();
        for name in except_idle {
            if let Some(app_id) = system_apps(name) {
                lists.except_idle.put(name, app_id);
            }
        }
        for name in allow {
            if let Some(app_id) = system_apps(name) {
                lists.except_idle.put(name, app_id);
                lists.system.put(name, app_id);
            }
        }
        for e in config.into_iter().flat_map(|c| c.children()) {
            let Some(name) = e.string("n") else { continue };
            match e.name.as_str() {
                "wl" => {
                    if let Some(app_id) = installed(&name) {
                        lists.user.put(&name, app_id);
                    }
                }
                "un-wl" => {
                    if let Some(app_id) = lists.system.remove(&name) {
                        lists.removed.put(&name, app_id);
                    }
                }
                other => eprintln!("app host: deviceidle: Unknown element under <config>: {other}"),
            }
        }
        lists
    }

    /// `writeConfigFileLocked(XmlSerializer)`, as `FastXmlSerializer`
    /// writes it.
    fn config(&self) -> String {
        let mut out = String::from("<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n");
        let entries: Vec<(&str, &str)> = self
            .user
            .names()
            .map(|n| ("wl", n))
            .chain(self.removed.names().map(|n| ("un-wl", n)))
            .collect();
        if entries.is_empty() {
            out.push_str("<config />\n");
            return out;
        }
        out.push_str("<config>");
        for (tag, name) in entries {
            out.push_str(&format!("<{tag} n=\""));
            for c in name.chars() {
                match c {
                    '"' => out.push_str("&quot;"),
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    c if (c as u32) < 32 => out.push_str(&format!("&#{};", c as u32)),
                    c => out.push(c),
                }
            }
            out.push_str("\" />\n");
        }
        out.push_str("</config>\n");
        out
    }

    /// `getFullPowerWhitelistInternalUnchecked`.
    fn full(&self) -> Vec<String> {
        self.system
            .names()
            .chain(self.user.names())
            .map(String::from)
            .collect()
    }

    /// `getFullPowerWhitelistExceptIdleInternal` before its filtering.
    fn full_except_idle(&self) -> Vec<String> {
        self.except_idle
            .names()
            .chain(self.user.names())
            .map(String::from)
            .collect()
    }

    /// `isPowerSaveWhitelistExceptIdleAppInternal`.
    fn allowed_except_idle(&self, name: &str) -> bool {
        self.except_idle.contains(name) || self.user.contains(name)
    }

    /// `isPowerSaveWhitelistAppInternal`.
    fn allowed(&self, name: &str) -> bool {
        self.system.contains(name) || self.user.contains(name)
    }
}

enum Work {
    /// `MSG_WRITE_CONFIG`, coalesced.
    WriteConfig,
    /// A temporary entry added: its end to be waited for.
    Temp,
}

pub struct DeviceIdle {
    host: Arc<AppHost>,
    /// Loaded at the first call (`onStart`).
    lists: Mutex<Option<Lists>>,
    handler: Mutex<mpsc::Sender<Work>>,
}

impl DeviceIdle {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let me = Arc::new(DeviceIdle {
            host,
            lists: Mutex::new(None),
            handler: Mutex::new(tx),
        });
        let weak = Arc::downgrade(&me);
        std::thread::Builder::new()
            .name("deviceidle".into())
            .spawn(move || handler(weak, rx))
            .expect("deviceidle handler");
        me
    }

    fn post(&self, work: Work) {
        let _ = self.handler.lock().unwrap().send(work);
    }

    /// Runs `f` on the lists, loaded first if they are not.
    fn with<T>(&self, f: impl FnOnce(&mut Lists) -> T) -> Result<T, Exception> {
        let mut guard = self.lists.lock().unwrap();
        if guard.is_none() {
            *guard = Some(self.load()?);
        }
        Ok(f(guard.as_mut().unwrap()))
    }

    fn load(&self) -> Result<Lists, Exception> {
        let (state, _) = self.host.packages()?;
        let app_id = |installed: Vec<(String, i32, i32)>| {
            move |name: &str| {
                installed
                    .iter()
                    .find(|(n, _, _)| n == name)
                    .map(|(_, uid, _)| uid % PER_USER_RANGE)
            }
        };
        let system_apps = app_id(self.host.installed(MATCH_ANY_USER | MATCH_SYSTEM_ONLY, 0)?);
        let installed = app_id(self.host.installed(MATCH_ANY_USER, 0)?);
        let path = self.config_path()?;
        let config = match super::device_policy::read_atomic_bytes(&path) {
            Ok(Some(bytes)) => aim_android_xml::read(&bytes)
                .map_err(|e| eprintln!("app host: deviceidle: Failed parsing config {e}"))
                .ok(),
            Ok(None) => None,
            Err(e) => {
                eprintln!("app host: deviceidle: {}: {e}", path.display());
                None
            }
        };
        let sorted = |set: &std::collections::BTreeSet<String>| {
            crate::package::info::array_order(set.iter().cloned().collect(), |s| s.as_str())
        };
        Ok(Lists::new(
            &sorted(&state.system.allow_in_power_save_except_idle),
            &sorted(&state.system.allow_in_power_save),
            &system_apps,
            &installed,
            config.as_ref(),
        ))
    }

    fn config_path(&self) -> Result<std::path::PathBuf, Exception> {
        Ok(self.host.inputs()?.data.join("system/deviceidle.xml"))
    }

    /// `enforceCallingOrSelfPermission(permission, message)`.
    fn enforce(&self, uid: i32, permission: &str, message: Option<&str>) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? == 0 {
            return Ok(());
        }
        let prefix = message.map(|m| format!("{m}: ")).unwrap_or_default();
        Err(Exception::security(format!(
            "{prefix}Neither user {uid} nor current process has {permission}."
        )))
    }

    /// A change of the allowlists: `reportPowerSaveWhitelistChangedLocked`
    /// (to user 0's registered receivers) and `writeConfigFileLocked`.
    fn changed(&self) {
        self.host.broadcasts.send_from_system_as_user(
            &self.host,
            RawIntent::of(
                Some(ACTION_POWER_SAVE_WHITELIST_CHANGED),
                FLAG_RECEIVER_REGISTERED_ONLY,
                &[],
            ),
            0,
        );
        self.post(Work::WriteConfig);
    }

    /// `handleWriteConfigFile`.
    fn write_config(&self) {
        let text = match self.with(|l| l.config()) {
            Ok(text) => text,
            Err(e) => return eprintln!("app host: deviceidle: config: {}", e.message),
        };
        let result = self.config_path().map_err(|e| e.message).and_then(|path| {
            let dir = path.parent().unwrap_or(&path);
            super::slice::write_atomic(dir, "deviceidle.xml", text.as_bytes())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            eprintln!("app host: deviceidle: Error writing config file: {e}");
        }
    }

    /// `addPowerSaveWhitelistAppsInternal`: the names that are packages
    /// added (in reverse, as the original walks them); the count of those
    /// not in error.
    fn add_user_apps(&self, names: &[Option<String>]) -> Result<i32, Exception> {
        let installed = self.host.installed(MATCH_ANY_USER, 0)?;
        let mut errors = 0;
        let added = self.with(|l| {
            let mut added = false;
            for name in names.iter().rev() {
                let Some(name) = name else {
                    errors += 1;
                    continue;
                };
                match installed.iter().find(|(n, _, _)| n == name) {
                    Some((_, uid, _)) => added |= l.user.put(name, uid % PER_USER_RANGE).is_none(),
                    None => {
                        eprintln!(
                            "app host: deviceidle: Tried to add unknown package to power save whitelist: {name}"
                        );
                        errors += 1;
                    }
                }
            }
            added
        })?;
        if added {
            self.changed();
        }
        Ok(names.len() as i32 - errors)
    }

    /// `getTempAllowListType`.
    fn temp_allowlist_type(reason_code: i32, default: i32) -> Result<i32, String> {
        match reason_code {
            // `ActivityManagerInternal.getPushMessagingOverQuotaBehavior`.
            REASON_PUSH_MESSAGING_OVER_QUOTA => {
                Err("the push messaging over quota behaviour".into())
            }
            REASON_DENIED => Ok(TEMPORARY_ALLOW_LIST_TYPE_NONE),
            _ => Ok(default),
        }
    }

    /// `addPowerSaveTempAllowlistAppChecked`.
    fn add_temp_checked(
        &self,
        call: &Call<'_>,
        package: Option<&str>,
        duration_ms: i64,
        user: i32,
        reason_code: i32,
        reason: Option<String>,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        self.enforce(
            uid,
            CHANGE_DEVICE_IDLE_TEMP_WHITELIST,
            Some("No permission to change device idle whitelist"),
        )?;
        let user = self.host.incoming_user(uid, user, true)?;
        let kind = Self::temp_allowlist_type(
            reason_code,
            TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED,
        )
        .map_err(|what| {
            self.host
                .unimplemented_named("deviceidle", &format!("{what} (#2016)"))
        })?;
        if kind != TEMPORARY_ALLOW_LIST_TYPE_NONE {
            self.add_temp(package, duration_ms, kind, user, reason_code, reason)?;
        }
        Ok(())
    }

    /// `addPowerSaveTempAllowlistAppInternal`: nothing for a package not
    /// installed in `user` (`NameNotFoundException`). The caller's uid,
    /// which the original keeps with the foreground service allowance,
    /// the app host does not.
    pub(super) fn add_temp(
        &self,
        package: Option<&str>,
        duration_ms: i64,
        kind: i32,
        user: i32,
        reason_code: i32,
        reason: Option<String>,
    ) -> Result<(), Exception> {
        let Some(package) = package else {
            return Ok(());
        };
        let installed = self.host.installed(0, user)?;
        let Some((_, uid, _)) = installed.iter().find(|(n, _, _)| n == package) else {
            return Ok(());
        };
        self.add_temp_direct(*uid, duration_ms, kind, reason_code, reason)
    }

    /// `addPowerSaveTempWhitelistAppDirectInternal`: the app id allowed
    /// until the duration (at most `MAX_TEMP_APP_ALLOWLIST_DURATION_MS`)
    /// ends; a new entry reported (`reportTempWhitelistChangedLocked`), and
    /// the uid allowed foreground service starts for the duration
    /// (`updateDeviceIdleTempAllowlist`). The battery history's
    /// `EVENT_TEMP_WHITELIST_START` is #1928; the network policy's
    /// exemption, the app host has no device idle restrictions to lift.
    pub(super) fn add_temp_direct(
        &self,
        uid: i32,
        duration_ms: i64,
        kind: i32,
        reason_code: i32,
        reason: Option<String>,
    ) -> Result<(), Exception> {
        let duration = duration_ms.min(self.constant("max_temp_app_allowlist_duration_ms")?);
        let duration = Duration::from_millis(duration.max(0) as u64);
        let app_id = uid % PER_USER_RANGE;
        let new = self.with(|l| {
            let end = Instant::now() + duration;
            l.temp.insert(app_id, (end, reason.clone())).is_none()
        })?;
        if kind == TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED {
            self.host.temp_allowlist_for_fgs(uid, duration, reason_code);
        }
        if new {
            self.post(Work::Temp);
            self.temp_changed();
        }
        Ok(())
    }

    /// `reportTempWhitelistChangedLocked`'s broadcast.
    fn temp_changed(&self) {
        self.host.broadcasts.send_from_system_as_user(
            &self.host,
            RawIntent::of(
                Some(ACTION_POWER_SAVE_TEMP_WHITELIST_CHANGED),
                FLAG_RECEIVER_REGISTERED_ONLY,
                &[],
            ),
            0,
        );
    }

    /// `checkTempAppWhitelistTimeout` of every entry: the ended ones
    /// removed and reported (`onAppRemovedFromTempWhitelistLocked`); the
    /// next end, if any.
    fn expire(&self) -> Option<Instant> {
        let now = Instant::now();
        let (removed, next) = self
            .with(|l| {
                let before = l.temp.len();
                l.temp.retain(|_, (end, _)| *end > now);
                (
                    before - l.temp.len(),
                    l.temp.values().map(|(end, _)| *end).min(),
                )
            })
            .ok()?;
        for _ in 0..removed {
            self.temp_changed();
        }
        next
    }

    /// `Constants`' `key`: the `device_idle_constants` setting's (a
    /// `KeyValueListParser` list), else DeviceConfig `device_idle`'s
    /// (`UserSettingDeviceConfigMediator`), else the framework's default
    /// (`device_idle_<key>`).
    fn constant(&self, key: &str) -> Result<i64, Exception> {
        let inputs = self.host.inputs()?;
        let setting = inputs
            .settings
            .get(Table::Global, "device_idle_constants")
            .and_then(|s| {
                s.split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .find(|(k, _)| k.trim() == key)
                    .and_then(|(_, v)| v.trim().parse().ok())
            });
        if let Some(value) = setting {
            return Ok(value);
        }
        if let Some(value) = inputs
            .settings
            .device_config("device_idle", key)
            .and_then(|v| v.parse().ok())
        {
            return Ok(value);
        }
        inputs
            .packages
            .framework_integer(&format!("device_idle_{key}"))
            .map(i64::from)
            .ok_or_else(|| {
                Exception::new(
                    EX_UNSUPPORTED_OPERATION,
                    format!("deviceidle: no default for {key}"),
                )
            })
    }

    /// The package names `uid` (of `user`) may see
    /// (`PackageManagerInternal.filterAppAccess`).
    fn visible(&self, names: Vec<String>, uid: i32) -> Result<Vec<Option<String>>, Exception> {
        let user = uid / PER_USER_RANGE;
        let mut out = Vec::new();
        for name in names {
            if !self.host.filter_package_access(&name, uid, user)? {
                out.push(Some(name));
            }
        }
        Ok(out)
    }

    /// `isPowerSaveWhitelistExceptIdleApp` of the system: the app host's
    /// own question (`AppStandbyController.isNonIdleWhitelisted`).
    pub(super) fn allowed_except_idle(&self, name: &str) -> Result<bool, Exception> {
        self.with(|l| l.allowed_except_idle(name))
    }
}

/// The service's handler: the config written once no change came for
/// `WRITE_CONFIG_DELAY`, the temporary entries removed as they end.
fn handler(me: Weak<DeviceIdle>, rx: mpsc::Receiver<Work>) {
    let mut write_at: Option<Instant> = None;
    let mut temp_at: Option<Instant> = None;
    loop {
        let next = [write_at, temp_at].into_iter().flatten().min();
        let work = match next {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let Some(service) = me.upgrade() else {
            return;
        };
        match work {
            Ok(Work::WriteConfig) => write_at = Some(Instant::now() + WRITE_CONFIG_DELAY),
            Ok(Work::Temp) => temp_at = service.expire(),
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                if write_at.is_some_and(|at| at <= now) {
                    write_at = None;
                    service.write_config();
                }
                if temp_at.is_some_and(|at| at <= now) {
                    temp_at = service.expire();
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Service for DeviceIdle {
    fn descriptor(&self) -> &str {
        di::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let names = |p: &mut Parcel,
                     list: Result<Vec<String>, Exception>,
                     write: fn(&mut Parcel, &Option<Vec<Option<String>>>)| {
            list.and_then(|names| self.visible(names, uid))
                .map(|names| write(p, &Some(names)))
        };
        let result: Result<(), Exception> = match call.code {
            di::ADD_POWER_SAVE_WHITELIST_APP => {
                let a =
                    di::AddPowerSaveWhitelistApp::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, DEVICE_POWER, None)
                    .and_then(|()| self.add_user_apps(&[a.name]))
                    .map(|_| di::write_add_power_save_whitelist_app_reply(&mut p))
            }
            di::ADD_POWER_SAVE_WHITELIST_APPS => {
                let a =
                    di::AddPowerSaveWhitelistApps::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, DEVICE_POWER, None)
                    .and_then(|()| self.add_user_apps(&a.package_names.unwrap_or_default()))
                    .map(|n| di::write_add_power_save_whitelist_apps_reply(&mut p, n))
            }
            di::REMOVE_POWER_SAVE_WHITELIST_APP => {
                let a =
                    di::RemovePowerSaveWhitelistApp::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let name = a.name.unwrap_or_default();
                self.enforce(uid, DEVICE_POWER, None)
                    .and_then(|()| {
                        self.with(|l| {
                            (
                                l.user.remove(&name).is_some(),
                                l.except_idle.contains(&name),
                            )
                        })
                    })
                    .and_then(|(removed, system)| {
                        if removed {
                            self.changed();
                        } else if system {
                            return Err(Exception::new(
                                EX_UNSUPPORTED_OPERATION,
                                "Cannot remove system whitelisted app",
                            ));
                        }
                        di::write_remove_power_save_whitelist_app_reply(&mut p);
                        Ok(())
                    })
            }
            di::REMOVE_SYSTEM_POWER_WHITELIST_APP | di::RESTORE_SYSTEM_POWER_WHITELIST_APP => {
                let restore = call.code == di::RESTORE_SYSTEM_POWER_WHITELIST_APP;
                let name = if restore {
                    di::RestoreSystemPowerWhitelistApp::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .name
                } else {
                    di::RemoveSystemPowerWhitelistApp::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .name
                }
                .unwrap_or_default();
                self.enforce(uid, DEVICE_POWER, None)
                    .and_then(|()| {
                        self.with(|l| {
                            let (from, to) = match restore {
                                true => (&mut l.removed, &mut l.system),
                                false => (&mut l.system, &mut l.removed),
                            };
                            from.remove(&name)
                                .map(|app_id| to.put(&name, app_id))
                                .is_some()
                        })
                    })
                    .map(|moved| {
                        if moved {
                            self.changed();
                        }
                        if restore {
                            di::write_restore_system_power_whitelist_app_reply(&mut p)
                        } else {
                            di::write_remove_system_power_whitelist_app_reply(&mut p)
                        }
                    })
            }
            di::GET_REMOVED_SYSTEM_POWER_WHITELIST_APPS => names(
                &mut p,
                self.with(|l| l.removed.names().map(String::from).collect()),
                di::write_get_removed_system_power_whitelist_apps_reply,
            ),
            di::GET_SYSTEM_POWER_WHITELIST_EXCEPT_IDLE => names(
                &mut p,
                self.with(|l| l.except_idle.names().map(String::from).collect()),
                di::write_get_system_power_whitelist_except_idle_reply,
            ),
            di::GET_SYSTEM_POWER_WHITELIST => names(
                &mut p,
                self.with(|l| l.system.names().map(String::from).collect()),
                di::write_get_system_power_whitelist_reply,
            ),
            di::GET_USER_POWER_WHITELIST => names(
                &mut p,
                self.with(|l| l.user.names().map(String::from).collect()),
                di::write_get_user_power_whitelist_reply,
            ),
            di::GET_FULL_POWER_WHITELIST_EXCEPT_IDLE => names(
                &mut p,
                self.with(|l| l.full_except_idle()),
                di::write_get_full_power_whitelist_except_idle_reply,
            ),
            di::GET_FULL_POWER_WHITELIST => names(
                &mut p,
                self.with(|l| l.full()),
                di::write_get_full_power_whitelist_reply,
            ),
            di::GET_APP_ID_WHITELIST_EXCEPT_IDLE => self
                .with(|l| app_ids(Some(&l.except_idle), &l.user))
                .map(|ids| di::write_get_app_id_whitelist_except_idle_reply(&mut p, &Some(ids))),
            di::GET_APP_ID_WHITELIST => self
                .with(|l| app_ids(Some(&l.system), &l.user))
                .map(|ids| di::write_get_app_id_whitelist_reply(&mut p, &Some(ids))),
            di::GET_APP_ID_USER_WHITELIST => self
                .with(|l| app_ids(None, &l.user))
                .map(|ids| di::write_get_app_id_user_whitelist_reply(&mut p, &Some(ids))),
            di::GET_APP_ID_TEMP_WHITELIST => self
                .with(|l| l.temp.keys().copied().collect::<Vec<_>>())
                .map(|ids| di::write_get_app_id_temp_whitelist_reply(&mut p, &Some(ids))),
            di::IS_POWER_SAVE_WHITELIST_EXCEPT_IDLE_APP | di::IS_POWER_SAVE_WHITELIST_APP => {
                let except_idle = call.code == di::IS_POWER_SAVE_WHITELIST_EXCEPT_IDLE_APP;
                let name = if except_idle {
                    di::IsPowerSaveWhitelistExceptIdleApp::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .name
                } else {
                    di::IsPowerSaveWhitelistApp::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                        .name
                }
                .unwrap_or_default();
                self.host
                    .filter_package_access(&name, uid, uid / PER_USER_RANGE)
                    .and_then(|filtered| {
                        if filtered {
                            return Ok(false);
                        }
                        self.with(|l| match except_idle {
                            true => l.allowed_except_idle(&name),
                            false => l.allowed(&name),
                        })
                    })
                    .map(|allowed| match except_idle {
                        true => {
                            di::write_is_power_save_whitelist_except_idle_app_reply(&mut p, allowed)
                        }
                        false => di::write_is_power_save_whitelist_app_reply(&mut p, allowed),
                    })
            }
            di::ADD_POWER_SAVE_TEMP_WHITELIST_APP => {
                let a = di::AddPowerSaveTempWhitelistApp::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.add_temp_checked(
                    call,
                    a.name.as_deref(),
                    a.duration,
                    a.user_id,
                    a.reason_code,
                    a.reason,
                )
                .map(|()| di::write_add_power_save_temp_whitelist_app_reply(&mut p))
            }
            di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_MMS
            | di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_SMS
            | di::WHITELIST_APP_TEMPORARILY => {
                let a = match call.code {
                    di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_MMS => {
                        let a = di::AddPowerSaveTempWhitelistAppForMms::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?;
                        (a.name, a.user_id, a.reason_code, a.reason)
                    }
                    di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_SMS => {
                        let a = di::AddPowerSaveTempWhitelistAppForSms::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?;
                        (a.name, a.user_id, a.reason_code, a.reason)
                    }
                    _ => {
                        let a = di::WhitelistAppTemporarily::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?;
                        (a.name, a.user_id, a.reason_code, a.reason)
                    }
                };
                let duration = match call.code {
                    di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_MMS => {
                        self.constant("mms_temp_app_allowlist_duration_ms")
                    }
                    di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_SMS => {
                        self.constant("sms_temp_app_allowlist_duration_ms")
                    }
                    // At least 10 seconds.
                    _ => self
                        .constant("max_temp_app_allowlist_duration_ms")
                        .map(|max| (max / 2).max(10_000)),
                };
                duration
                    .and_then(|d| {
                        self.add_temp_checked(call, a.0.as_deref(), d, a.1, a.2, a.3)
                            .map(|()| d)
                    })
                    .map(|d| match call.code {
                        di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_MMS => {
                            di::write_add_power_save_temp_whitelist_app_for_mms_reply(&mut p, d)
                        }
                        di::ADD_POWER_SAVE_TEMP_WHITELIST_APP_FOR_SMS => {
                            di::write_add_power_save_temp_whitelist_app_for_sms_reply(&mut p, d)
                        }
                        _ => di::write_whitelist_app_temporarily_reply(&mut p, d),
                    })
            }
            // `exitIdleInternal`: `becomeActiveLocked` of a device that is
            // active already changes nothing.
            di::EXIT_IDLE => {
                di::ExitIdle::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(uid, DEVICE_POWER, None)
                    .map(|()| di::write_exit_idle_reply(&mut p))
            }
            code => Err(self
                .host
                .unimplemented("deviceidle", self.descriptor(), code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ArrayMap`'s order: by hash, an equal hash after the keys before it.
    #[test]
    fn apps_keep_array_map_order() {
        let mut apps = Apps::default();
        for name in ["com.b", "com.a", "Aa", "BB"] {
            apps.put(name, 1);
        }
        // "Aa" and "BB" share a hash, so they stay as added.
        let mut expected: Vec<&str> = vec!["com.b", "com.a"];
        expected.sort_by_key(|n| java_hash(n));
        let names: Vec<&str> = apps.names().collect();
        assert_eq!(
            names
                .iter()
                .filter(|n| n.starts_with("com."))
                .copied()
                .collect::<Vec<_>>(),
            expected
        );
        let aa = names.iter().position(|n| *n == "Aa").unwrap();
        let bb = names.iter().position(|n| *n == "BB").unwrap();
        assert!(aa < bb);
        assert_eq!(apps.put("com.a", 7), Some(1));
        assert_eq!(apps.get("com.a"), Some(7));
    }

    /// `onStart` and `readConfigFileLocked`: the allow-in-power-save
    /// packages are on both system lists, the user's installed ones are
    /// read, and `un-wl` moves a system app to the removed list.
    #[test]
    fn lists_from_system_config_and_the_user_config() {
        let config = aim_android_xml::read(
            b"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n\
              <config><wl n=\"user.app\" />\n<wl n=\"gone.app\" />\n<un-wl n=\"sys.full\" />\n</config>\n",
        )
        .unwrap();
        let system = |n: &str| match n {
            "sys.idle" => Some(1001),
            "sys.full" => Some(1002),
            _ => None,
        };
        let installed = |n: &str| match n {
            "user.app" => Some(10_100),
            _ => system(n),
        };
        let lists = Lists::new(
            &["sys.idle".into(), "missing".into()],
            &["sys.full".into()],
            &system,
            &installed,
            Some(&config),
        );
        assert!(lists.except_idle.contains("sys.idle") && lists.except_idle.contains("sys.full"));
        assert!(!lists.system.contains("sys.full") && lists.removed.contains("sys.full"));
        assert_eq!(lists.user.0, vec![("user.app".to_string(), 10_100)]);
        assert!(lists.allowed_except_idle("sys.full") && !lists.allowed("sys.full"));
        assert_eq!(
            app_ids(Some(&lists.except_idle), &lists.user),
            vec![1001, 1002, 10_100]
        );
        assert_eq!(
            lists.config(),
            "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n\
             <config><wl n=\"user.app\" />\n<un-wl n=\"sys.full\" />\n</config>\n"
        );
        assert_eq!(
            Lists::default().config(),
            "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<config />\n"
        );
    }

    #[test]
    fn temp_allowlist_types_follow_the_reason() {
        let fgs = TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED;
        assert_eq!(DeviceIdle::temp_allowlist_type(0, fgs), Ok(fgs));
        assert_eq!(
            DeviceIdle::temp_allowlist_type(REASON_DENIED, fgs),
            Ok(TEMPORARY_ALLOW_LIST_TYPE_NONE)
        );
        assert!(DeviceIdle::temp_allowlist_type(REASON_PUSH_MESSAGING_OVER_QUOTA, fgs).is_err());
    }
}
