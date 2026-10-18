//! The `settings` provider for the app host: `IContentProvider.call` of
//! `SettingsProvider` at `android-16.0.0_r1` over the settings the app
//! host keeps (`super::settings`): `GET_*` with the generation tracking a
//! client asks for (`_track_generation`, `super::generations`), `PUT_*`,
//! `DELETE_*` and `RESET_*` of the system, secure and global settings with
//! the original's permission, app op, user restriction and validator
//! checks (`super::settings_validators`), and `LIST_config`. A change bumps
//! its generation, then is notified to the observers of
//! `content://settings/<table>/<name>` (`super::content_service`); no
//! backup is run, so none is told of it.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use aim_android_image::dex::{self, Dex};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::android_content_icontentprovider as cp;

use super::appops_service::{self, MODE_DEFAULT, OP_WRITE_SETTINGS};
use super::generations::{self, GenerationRegistry, TRACK_GENERATION};
use super::settings::{Change, ResetMode, SYSTEM_PACKAGE_NAME, Table};
use super::settings_validators::{PRIVATE_SETTINGS, PUBLIC_SETTINGS, validate_system_value};
use super::{AppHost, FIRST_APPLICATION_UID, PER_USER_RANGE, reply};
use crate::bundle::{self, Value};
use crate::package::info::{array_order, flags};
use crate::package::uri::Uri;

/// `Settings.NameValueTable.VALUE` and the `Settings.CALL_METHOD_*_KEY`s;
/// `SettingsProvider.RESULT_ROWS_DELETED`.
const VALUE: &str = "value";
const USER: &str = "_user";
const PREFIX: &str = "_prefix";
const TAG: &str = "_tag";
const MAKE_DEFAULT: &str = "_make_default";
const RESET_MODE: &str = "_reset_mode";
const OVERRIDEABLE_BY_RESTORE: &str = "_overrideable_by_restore";
const RESULT_ROWS_DELETED: &str = "result_rows_deleted";
/// The permissions it checks.
const WRITE_SECURE_SETTINGS: &str = "android.permission.WRITE_SECURE_SETTINGS";
const WRITE_SETTINGS: &str = "android.permission.WRITE_SETTINGS";
const LOCAL_MAC_ADDRESS: &str = "android.permission.LOCAL_MAC_ADDRESS";
const MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE: &str = "android.permission.MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE";
/// `Process.ROOT_UID`, `SYSTEM_UID`, `SHELL_UID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const SHELL_UID: i32 = 2000;
/// `Build.VERSION_CODES.LOLLIPOP_MR1`.
const LOLLIPOP_MR1: i32 = 22;
/// `ApplicationInfo.FLAG_SYSTEM`, `FLAG_PERSISTENT`,
/// `PRIVATE_FLAG_PRIVILEGED`, `PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY`.
const FLAG_SYSTEM: i32 = 1;
const FLAG_PERSISTENT: i32 = 1 << 3;
const PRIVATE_FLAG_PRIVILEGED: i32 = 1 << 3;
const PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY: i32 = 1 << 20;
/// `CRITICAL_GLOBAL_SETTINGS`, `CRITICAL_SECURE_SETTINGS`: written at once.
const CRITICAL_GLOBAL_SETTINGS: &[&str] = &["device_provisioned"];
const CRITICAL_SECURE_SETTINGS: &[&str] = &["user_setup_complete"];
/// The DeviceConfig and listing calls not taken (#1526).
const NOT_TAKEN: &[&str] = &[
    "PUT_config",
    "DELETE_config",
    "RESET_config",
    "SET_ALL_config",
    "SET_SYNC_DISABLED_MODE_config",
    "GET_SYNC_DISABLED_MODE_config",
    "LIST_namespaces_config",
    "REGISTER_MONITOR_CALLBACK_config",
    "UNREGISTER_MONITOR_CALLBACK_config",
    "LIST_global",
    "LIST_secure",
    "LIST_system",
];

impl Table {
    /// Its `CONTENT_URI`'s path.
    fn path(self) -> &'static str {
        match self {
            Table::System => "system",
            Table::Secure => "secure",
            Table::Global => "global",
            Table::Config => "config",
        }
    }
}

/// The caller of a call: its uid and its `AttributionSource`'s package.
struct Caller {
    uid: i32,
    package: Option<String>,
}

impl Caller {
    /// `UserHandle.getCallingUserId`.
    fn user(&self) -> i32 {
        self.uid / PER_USER_RANGE as i32
    }

    fn app_id(&self) -> i32 {
        self.uid.rem_euclid(PER_USER_RANGE as i32)
    }
}

/// A mutation (`MUTATION_OPERATION_*`); an update is an insertion here.
#[derive(Clone, Copy)]
enum Op<'a> {
    Insert {
        value: Option<&'a str>,
        tag: Option<&'a str>,
        make_default: bool,
        overrideable_by_restore: bool,
    },
    Delete,
    Reset {
        mode: ResetMode,
        tag: Option<&'a str>,
    },
}

/// The names `Settings.System`, `Secure` and `Global` declare: the values
/// of their String fields (`getPublicSettingsForClass`, `Global.Wearable`'s
/// only on a watch), read from the image's framework.
struct Declared {
    system: HashSet<String>,
    secure: HashSet<String>,
    global: HashSet<String>,
}

impl Declared {
    fn read(jar: &Path) -> Result<Declared, String> {
        const CLASSES: [&str; 3] = [
            "Landroid/provider/Settings$System;",
            "Landroid/provider/Settings$Secure;",
            "Landroid/provider/Settings$Global;",
        ];
        let jar = aim_apps::apk::Apk::open(jar).map_err(|e| e.0)?;
        let mut sets: [Option<HashSet<String>>; 3] = Default::default();
        for index in 1.. {
            let name = if index == 1 { "classes.dex".to_string() } else { format!("classes{index}.dex") };
            let Some(bytes) = jar.file_if_present(&name).map_err(|e| e.0)? else { break };
            let dex = Dex::parse(&bytes)?;
            for (set, class) in sets.iter_mut().zip(CLASSES) {
                if let Some(class) = dex.class(class) {
                    let names = dex.static_values(class)?.into_iter().filter_map(|(_, value)| match value {
                        dex::Value::String(s) => Some(s),
                        _ => None,
                    });
                    set.get_or_insert_default().extend(names);
                }
            }
        }
        let [Some(system), Some(secure), Some(global)] = sets else {
            return Err("no android.provider.Settings classes".into());
        };
        Ok(Declared { system, secure, global })
    }
}

pub struct SettingsProvider {
    host: Arc<AppHost>,
    generations: OnceLock<GenerationRegistry>,
    declared: OnceLock<Declared>,
    /// `PackageManager.getSetupWizardPackageName`, cached as
    /// `cacheSystemPackageNamesAndSystemSignature` caches it.
    setup_wizard: OnceLock<Option<String>>,
}

impl Service for SettingsProvider {
    fn descriptor(&self) -> &str {
        cp::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // A call's extras may carry descriptors (none are read).
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if matches!(call.code, OPEN_FILE_TRANSACTION | OPEN_ASSET_FILE_TRANSACTION | OPEN_TYPED_ASSET_FILE_TRANSACTION) {
            return self.open(call);
        }
        if call.code != cp::CALL_TRANSACTION {
            return reply(Err(self.host.unimplemented("settings provider", call.code)));
        }
        let r = &mut call.data;
        r.enforce_interface(cp::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let package = read_attribution_source(r)?;
        let _authority = r.read_string16().map_err(|_| BAD_VALUE)?;
        let method = r.read_string16().map_err(|_| BAD_VALUE)?.ok_or(BAD_VALUE)?;
        let arg = r.read_string16().map_err(|_| BAD_VALUE)?;
        let extras = bundle::read(r).map_err(|_| BAD_VALUE)?.unwrap_or_default();
        let caller = Caller { uid: call.sender_euid as i32, package };
        reply(self.call(&caller, &method, arg.as_deref(), &extras).map(|result| {
            let mut p = Parcel::new();
            p.write_no_exception();
            match result {
                Some(result) => {
                    bundle::write(&mut p, &result.iter().map(|(k, v)| (k.as_str(), v.clone())).collect::<Vec<_>>())
                }
                None => p.write_i32(-1),
            }
            p
        }))
    }
}

/// `IContentProvider`'s file transactions.
const OPEN_FILE_TRANSACTION: u32 = 14;
const OPEN_ASSET_FILE_TRANSACTION: u32 = 15;
const OPEN_TYPED_ASSET_FILE_TRANSACTION: u32 = 23;
/// `DatabaseUtils.writeExceptionToParcel`'s codes.
const FILE_NOT_FOUND: i32 = 1;
const ILLEGAL_ARGUMENT: i32 = 2;
/// `AssetFileDescriptor.UNKNOWN_LENGTH`.
const UNKNOWN_LENGTH: i64 = -1;

/// An exception a provider's transaction writes as
/// `DatabaseUtils.writeExceptionToParcel` writes it: one of its codes, or
/// the binder's own.
enum Thrown {
    Database(i32, String),
    Binder(Exception),
}

impl From<Exception> for Thrown {
    fn from(e: Exception) -> Self {
        Thrown::Binder(e)
    }
}

/// A file a provider opened: `ParcelFileDescriptor`'s parcel, and for an
/// `AssetFileDescriptor` its whole extent and no extras.
struct Opened(crate::package::app_metadata::Descriptor);

impl SettingsProvider {
    /// `ContentProviderNative`'s `OPEN_FILE`, `OPEN_ASSET_FILE` and
    /// `OPEN_TYPED_ASSET_FILE` through `ContentProvider.Transport`.
    fn open(&self, call: &mut Call<'_>) -> Reply {
        let code = call.code;
        let uid = call.sender_euid as i32;
        let r = &mut call.data;
        r.enforce_interface(cp::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let package = read_attribution_source(r)?;
        let uri = Uri::read(r, &mut crate::package::intent_filter::Plain).map_err(|_| BAD_VALUE)?.ok_or(BAD_VALUE)?;
        let mode = r.read_string16().map_err(|_| BAD_VALUE)?;
        let mime_type = if code == OPEN_TYPED_ASSET_FILE_TRANSACTION {
            let mime_type = r.read_string16().map_err(|_| BAD_VALUE)?;
            bundle::read(r).map_err(|_| BAD_VALUE)?; // the options, which openFile ignores
            mime_type
        } else {
            None
        };
        let caller = Caller { uid, package };
        let opened = (|| -> Result<Opened, Thrown> {
            // `validateIncomingUri`: the settings authority (the provider is
            // a single user's, so the URI keeps its user); its read and
            // write permissions are none (`enforceFilePermission`).
            let authority = uri.authority().unwrap_or_default();
            if authority.rsplit('@').next() != Some("settings") {
                return Err(Exception::security(format!(
                    "The authority of the uri {} does not match the one of the contentProvider: settings",
                    uri.as_str()
                ))
                .into());
            }
            match code {
                OPEN_TYPED_ASSET_FILE_TRANSACTION => {
                    let filter = mime_type.unwrap_or_default();
                    if filter != "*/*" && !self.has_mime_type(&uri, &filter)? {
                        return Err(Thrown::Database(FILE_NOT_FOUND, format!("Can't open {} as type {filter}", uri.as_str())));
                    }
                    self.open_file(&caller, &uri, "r")
                }
                _ => self.open_file(&caller, &uri, mode.as_deref().unwrap_or_default()),
            }
        })();
        let mut p = Parcel::new();
        match opened {
            Ok(Opened(fd)) => {
                p.write_no_exception();
                p.write_i32(1);
                aim_service_aidl::WriteParcelable::write_to(&fd, &mut p);
                if code != OPEN_FILE_TRANSACTION {
                    p.write_i64(0);
                    p.write_i64(UNKNOWN_LENGTH);
                    p.write_i32(0);
                }
                Ok(p)
            }
            Err(Thrown::Database(code, message)) => {
                p.write_i32(code);
                p.write_string16(Some(&message));
                Ok(p)
            }
            Err(Thrown::Binder(e)) => reply(Err(e)),
        }
    }

    /// Whether `getType` of `uri` is of `desired`
    /// (`ClipDescription.compareMimeTypes`): a table's directory, or one
    /// of its settings. A setting moved between tables is of the table it
    /// moved to, which the app host does not know (#1525); that matters
    /// only to a desired type naming the table.
    fn has_mime_type(&self, uri: &Uri, desired: &str) -> Result<bool, Thrown> {
        const ITEM: &str = "vnd.android.cursor.item/";
        let segments = uri.path_segments();
        let table = segments.first().ok_or_else(|| Thrown::Database(ILLEGAL_ARGUMENT, format!("Invalid URI:{}", uri.as_str())))?;
        if !matches!(table.as_str(), "system" | "secure" | "global") {
            return Err(Thrown::Database(ILLEGAL_ARGUMENT, format!("Bad root path: {table}")));
        }
        match segments.len() {
            1 => Ok(compare_mime_types(&format!("vnd.android.cursor.dir/{table}"), desired)),
            2 if desired.starts_with(ITEM) && desired != format!("{ITEM}*") => {
                Err(self.host.unimplemented_named("settings provider", "the table of a setting moved between tables (#1525)").into())
            }
            2 => Ok(compare_mime_types(&format!("{ITEM}{table}"), desired)),
            _ => Err(Thrown::Database(
                ILLEGAL_ARGUMENT,
                format!(
                    "Supported SQL:\n  uri content://some_table/some_property with null where and where args\n  uri content://some_table with query name=? and single name as arg\n  uri content://some_table with query name=some_name and null args\n  but got - uri:{}, where:null whereArgs:null",
                    uri.as_str()
                ),
            )),
        }
    }

    /// `SettingsProvider.openFile`: only the ringtone caches, opened as
    /// `ParcelFileDescriptor.open` opens them.
    fn open_file(&self, caller: &Caller, uri: &Uri, mode: &str) -> Result<Opened, Thrown> {
        let calling_user = caller.user();
        let authority = uri.authority().unwrap_or_default();
        // `getUserIdFromUri`.
        let user = match authority.split_once('@') {
            Some((user, _)) => user.parse().unwrap_or(-10_000),
            None => calling_user,
        };
        if user != calling_user && !self.has(caller, "android.permission.INTERACT_ACROSS_USERS")? {
            return Err(Exception::security(format!(
                "Access files from the settings of another user: Neither user {} nor current process has android.permission.INTERACT_ACROSS_USERS.",
                caller.uid
            ))
            .into());
        }
        if mode.contains('w') {
            self.check_and_note_write_settings(caller, caller.package.as_deref())?;
        }
        // `getUriWithoutUserId`.
        let without_user = match authority.split_once('@') {
            Some((prefix, _)) => uri.as_str().replacen(&format!("{prefix}@"), "", 1),
            None => uri.as_str().to_string(),
        };
        let setting = ["ringtone", "notification_sound", "alarm_alert"]
            .into_iter()
            .find(|s| without_user == format!("content://settings/system/{}", ringtone_cache(s).unwrap_or_default()))
            .ok_or_else(|| {
                Thrown::Database(FILE_NOT_FOUND, "Direct file access no longer supported; ringtone playback is available through android.media.Ringtone".into())
            })?;
        let (read, write, create, truncate, append) = match mode {
            "r" => (true, false, false, false, false),
            "w" | "wt" => (false, true, true, true, false),
            "wa" => (false, true, true, false, true),
            "rw" => (true, true, true, false, false),
            "rwt" => (true, true, true, true, false),
            _ => return Err(Thrown::Database(ILLEGAL_ARGUMENT, format!("Bad mode '{mode}'"))),
        };
        // `getCacheFile` in `getRingtoneCacheDir`, made if missing.
        let data = self.host.data()?;
        let dir = data.join(format!("system_de/{user}/ringtones"));
        if !dir.exists() {
            std::fs::create_dir(&dir).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", dir.display())))?;
            aim_storage::guest_inode::record(&dir, aim_storage::guest_inode::GuestInode { uid: Some(SYSTEM_UID as u32), gid: Some(SYSTEM_UID as u32), mode: Some(0o700) })
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", dir.display())))?;
        }
        let path = dir.join(ringtone_cache(setting).unwrap_or_default());
        let existed = path.exists();
        let file = std::fs::OpenOptions::new()
            .read(read)
            .write(write)
            .create(create)
            .truncate(truncate)
            .append(append)
            .open(&path)
            .map_err(|e| Thrown::Database(FILE_NOT_FOUND, open_failed(&e)))?;
        if create && !existed {
            aim_storage::guest_inode::record(&path, aim_storage::guest_inode::GuestInode { uid: Some(SYSTEM_UID as u32), gid: Some(SYSTEM_UID as u32), mode: Some(0o700) })
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display())))?;
        }
        let file = aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file))
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "settings provider: no fileport"))?;
        Ok(Opened(crate::package::app_metadata::Descriptor(file)))
    }
}

/// `ErrnoException.getMessage` of `Os.open`'s failure.
fn open_failed(e: &std::io::Error) -> String {
    let errno = e.raw_os_error().unwrap_or(0);
    let name = match errno {
        libc::ENOENT => "ENOENT",
        libc::EACCES => "EACCES",
        libc::EISDIR => "EISDIR",
        libc::ENOTDIR => "ENOTDIR",
        libc::EROFS => "EROFS",
        libc::ENOSPC => "ENOSPC",
        _ => return format!("open failed: {e}"),
    };
    let text = std::io::Error::from_raw_os_error(errno).to_string();
    let text = text.split(" (os error").next().unwrap_or_default();
    format!("open failed: {name} ({text})")
}

/// `ClipDescription.compareMimeTypes(concrete, desired)`.
fn compare_mime_types(concrete: &str, desired: &str) -> bool {
    if desired == "*/*" {
        return true;
    }
    match desired.find('/') {
        Some(slash) if slash > 0 => {
            if desired.len() == slash + 2 && desired.as_bytes()[slash + 1] == b'*' {
                concrete.as_bytes().get(..=slash) == Some(&desired.as_bytes()[..=slash])
            } else {
                desired == concrete
            }
        }
        _ => false,
    }
}

/// `AttributionSourceState`, a structured parcelable (its size, then pid,
/// uid, device id, package, attribution tag, token, renounced permissions
/// and the next sources): the package.
fn read_attribution_source(r: &mut Reader<'_>) -> Result<Option<String>, i32> {
    let start = r.position();
    let size = usize::try_from(r.read_i32().map_err(|_| BAD_VALUE)?).map_err(|_| BAD_VALUE)?;
    if size < 4 || size - 4 > r.remaining() {
        return Err(BAD_VALUE);
    }
    let end = start + size;
    let mut package = None;
    for field in 0..4 {
        if r.position() >= end {
            break;
        }
        if field < 3 {
            r.read_i32().map_err(|_| BAD_VALUE)?;
        } else {
            package = r.read_string16().map_err(|_| BAD_VALUE)?;
        }
    }
    r.set_position(end);
    Ok(package)
}

/// `packageValueForCallResult`: the value, and the generation data a
/// tracking client asked for, in the reply `Bundle`'s order.
fn value_result(value: Option<String>, generation: Vec<(String, Value)>) -> Vec<(String, Value)> {
    let mut result = vec![(VALUE.to_string(), value.map_or(Value::Null, |v| Value::String(Some(v))))];
    result.extend(generation);
    array_order(result, |(key, _)| key)
}

/// `UserRestrictionsUtils.isSettingRestrictedForUser`: whether a change of
/// `name` to `value` by `uid` is one a user restriction (`restricted`)
/// forbids.
fn setting_restricted(name: &str, value: Option<&str>, uid: i32, restricted: &dyn Fn(&str) -> bool) -> bool {
    let restriction = match name {
        "location_mode" => {
            if restricted("no_config_location") && uid != SYSTEM_UID {
                return true;
            }
            // `LOCATION_MODE_OFF`.
            if value == Some("0") {
                return false;
            }
            "no_share_location"
        }
        "install_non_market_apps" | "adb_enabled" | "adb_wifi_enabled" | "airplane_mode_on" | "doze_enabled"
        | "doze_always_on" | "doze_pulse_on_pick_up" | "doze_pulse_on_long_press" | "doze_pulse_on_double_tap"
            if value == Some("0") =>
        {
            return false;
        }
        "install_non_market_apps" => "no_install_unknown_sources",
        "adb_enabled" | "adb_wifi_enabled" => "no_debugging_features",
        "verifier_verify_adb_installs" if value == Some("1") => return false,
        "verifier_verify_adb_installs" => "ensure_verify_apps",
        "preferred_network_mode" => "no_config_mobile_networks",
        "always_on_vpn_app" | "always_on_vpn_lockdown" | "always_on_vpn_lockdown_whitelist" => {
            if matches!(uid.rem_euclid(PER_USER_RANGE as i32), SYSTEM_UID | ROOT_UID) {
                return false;
            }
            "no_config_vpn"
        }
        "safe_boot_disallowed" if value == Some("1") => return false,
        "safe_boot_disallowed" => "no_safe_boot",
        "airplane_mode_on" => "no_airplane_mode",
        "doze_enabled" | "doze_always_on" | "doze_pulse_on_pick_up" | "doze_pulse_on_long_press"
        | "doze_pulse_on_double_tap" => "no_ambient_display",
        "screen_brightness" | "screen_brightness_mode" | "auto_time" | "auto_time_zone" | "screen_off_timeout"
        | "private_dns_mode" | "private_dns_specifier"
            if uid == SYSTEM_UID =>
        {
            return false;
        }
        "screen_brightness" | "screen_brightness_mode" => "no_config_brightness",
        "auto_time" | "auto_time_zone" => "no_config_date_time",
        "screen_off_timeout" => "no_config_screen_timeout",
        "private_dns_mode" | "private_dns_specifier" => "disallow_config_private_dns",
        _ if name.starts_with("data_roaming") => {
            if value == Some("0") {
                return false;
            }
            "no_data_roaming"
        }
        _ => return false,
    };
    restricted(restriction)
}

/// `enforceRestrictedSystemSettingsMutationForCallingPackage`: what an app
/// may do to the system settings; `package` is the calling package's
/// whether privileged and target SDK.
fn enforce_system_mutation(
    app_id: i32,
    op: &Op<'_>,
    name: &str,
    package: &dyn Fn() -> Result<(bool, i32), Exception>,
) -> Result<(), Exception> {
    // System, root and shell can mutate whatever they want.
    if matches!(app_id, SYSTEM_UID | SHELL_UID | ROOT_UID) {
        return Ok(());
    }
    match op {
        Op::Insert { .. } if PUBLIC_SETTINGS.contains(&name) => return Ok(()),
        Op::Delete if PUBLIC_SETTINGS.contains(&name) || PRIVATE_SETTINGS.contains(&name) => {
            return Err(Exception::illegal_argument("You cannot delete system defined secure settings."));
        }
        Op::Reset { .. } => return Ok(()),
        _ => {}
    }
    let (privileged, target_sdk) = package()?;
    if privileged {
        return Ok(());
    }
    // `warnOrThrowForUndesiredSecureSettingsMutationForTargetSdk`.
    let private = PRIVATE_SETTINGS.contains(&name);
    if target_sdk <= LOLLIPOP_MR1 {
        eprintln!(
            "app host: settings: {}",
            if private {
                "You shouldn't not change private system settings. This will soon become an error."
            } else {
                "You shouldn't keep your settings in the secure settings. This will soon become an error."
            }
        );
        return Ok(());
    }
    Err(Exception::illegal_argument(if private {
        "You cannot change private secure settings."
    } else {
        "You cannot keep your settings in the secure settings."
    }))
}

/// `getResetModeEnforcingPermission`: a reset of others' changes only by
/// the system, or by the shell or root on a debuggable build.
fn reset_mode(mode: i32, trusted_caller: bool) -> Result<ResetMode, Exception> {
    let (mode, what) = match mode {
        1 => return Ok(ResetMode::PackageDefaults),
        2 => (ResetMode::UntrustedDefaults, "reset to untrusted defaults"),
        3 => (ResetMode::UntrustedChanges, "reset untrusted changes"),
        4 => (ResetMode::TrustedDefaults, "reset to trusted defaults"),
        _ => return Err(Exception::illegal_argument(format!("Invalid reset mode: {mode}"))),
    };
    if !trusted_caller {
        return Err(Exception::security(format!("Only system, shell/root on a debuggable build can {what}")));
    }
    Ok(mode)
}

/// `getCacheName`: the ringtone cache of a sound setting.
fn ringtone_cache(name: &str) -> Option<&'static str> {
    match name {
        "ringtone" => Some("ringtone_cache"),
        "notification_sound" => Some("notification_sound_cache"),
        "alarm_alert" => Some("alarm_alert_cache"),
        _ => None,
    }
}

/// `SettingsState.isShellOrRoot`.
fn shell_or_root(package: &str) -> bool {
    matches!(package, "com.android.shell" | "root")
}

impl SettingsProvider {
    pub fn new(host: Arc<AppHost>) -> SettingsProvider {
        SettingsProvider { host, generations: OnceLock::new(), declared: OnceLock::new(), setup_wizard: OnceLock::new() }
    }

    /// `SettingsProvider.call`.
    fn call(
        &self,
        caller: &Caller,
        method: &str,
        name: Option<&str>,
        extras: &HashMap<String, Value>,
    ) -> Result<Option<Vec<(String, Value)>>, Exception> {
        // `getRequestingUserId`.
        let requesting = match extras.get(USER) {
            Some(Value::Int(user)) => *user,
            _ => caller.user(),
        };
        let tracking = extras.contains_key(TRACK_GENERATION);
        let string = |key| match extras.get(key) {
            Some(Value::String(s)) => s.as_deref(),
            _ => None,
        };
        let boolean = |key| matches!(extras.get(key), Some(Value::Bool(true)));
        let (verb, table) = match method.split_once('_') {
            Some((verb, "system")) => (verb, Some(Table::System)),
            Some((verb, "secure")) => (verb, Some(Table::Secure)),
            Some((verb, "global")) => (verb, Some(Table::Global)),
            Some((verb, "config")) => (verb, Some(Table::Config)),
            _ => (method, None),
        };
        Ok(match (verb, table) {
            ("GET", Some(table)) => Some(self.get(caller, table, name, requesting, tracking)?),
            ("PUT", Some(table)) if table != Table::Config => {
                // `insertSystemSetting` takes no tag and no default.
                let system = table == Table::System;
                let insert = Op::Insert {
                    value: string(VALUE),
                    tag: if system { None } else { string(TAG) },
                    make_default: !system && boolean(MAKE_DEFAULT),
                    overrideable_by_restore: boolean(OVERRIDEABLE_BY_RESTORE),
                };
                self.mutate(caller, table, name, insert, requesting)?;
                None
            }
            ("RESET", Some(table)) if table != Table::Config => {
                let mode = match extras.get(RESET_MODE) {
                    Some(Value::Int(mode)) => *mode,
                    _ => 0,
                };
                let reset = Op::Reset { mode: reset_mode(mode, self.trusted_for_reset(caller))?, tag: string(TAG) };
                self.mutate(caller, table, None, reset, requesting)?;
                None
            }
            ("DELETE", Some(table)) if table != Table::Config => {
                let deleted = self.mutate(caller, table, name, Op::Delete, requesting)?;
                Some(vec![(RESULT_ROWS_DELETED.to_string(), Value::Int(deleted as i32))])
            }
            ("LIST", Some(Table::Config)) => {
                let prefix = string(PREFIX).unwrap_or_default();
                let settings = &self.host.inputs()?.settings;
                let mut result = vec![(VALUE.to_string(), Value::StringMap(settings.list(Table::Config, prefix)))];
                if tracking {
                    result.extend(self.generations()?.generation_data(Table::Config, 0, prefix));
                }
                Some(array_order(result, |(key, _)| key))
            }
            _ if NOT_TAKEN.contains(&method) => {
                return Err(self.host.unimplemented_named("settings provider", &format!("{method} (#1526)")));
            }
            _ => {
                eprintln!("app host: settings: call() with invalid method: {method}");
                None
            }
        })
    }

    /// `getConfigSetting`, `getGlobalSetting`, `getSecureSetting` and
    /// `getSystemSetting`, then `packageValueForCallResult`.
    fn get(
        &self,
        caller: &Caller,
        table: Table,
        name: Option<&str>,
        requesting: i32,
        tracking: bool,
    ) -> Result<Vec<(String, Value)>, Exception> {
        let name = name.unwrap_or_default();
        let user = match table {
            Table::Secure | Table::System => self.calling_user(caller, requesting)?,
            Table::Global | Table::Config => 0,
        };
        // `isSecureSettingAccessible`: the Bluetooth address only to who
        // may see the device's MAC addresses.
        let accessible = table != Table::Secure || name != "bluetooth_address" || self.has(caller, LOCAL_MAC_ADDRESS)?;
        // The app host keeps user 0's settings only (#1523).
        let setting = if accessible && user == 0 { self.host.inputs()?.settings.setting(table, name) } else { None };
        let mut generation = Vec::new();
        if tracking {
            // A setting set or declared has its own generation, the unset
            // others share one.
            let key = if setting.is_some() || self.predefined(table, name)? { name } else { generations::UNSET };
            generation = self.generations()?.generation_data(table, requesting, key);
        }
        Ok(value_result(setting.and_then(|s| s.value), generation))
    }

    /// `mutateGlobalSetting`, `mutateSecureSetting` and
    /// `mutateSystemSetting`: whether the settings changed.
    fn mutate(
        &self,
        caller: &Caller,
        table: Table,
        name: Option<&str>,
        op: Op<'_>,
        requesting: i32,
    ) -> Result<bool, Exception> {
        let value = match op {
            Op::Insert { value, .. } => value,
            _ => None,
        };
        if table == Table::System {
            return self.mutate_system(caller, name, op, value, requesting);
        }
        // The global and secure settings are WRITE_SECURE_SETTINGS'.
        if !self.has(caller, WRITE_SECURE_SETTINGS)? {
            return Err(Exception::security(format!("Permission denial, must have one of: [{WRITE_SECURE_SETTINGS}]")));
        }
        let user = self.calling_user(caller, requesting)?;
        if self.restricted(name, value, caller.uid)? {
            return Ok(false);
        }
        let package = self.calling_package(caller)?;
        let (state_user, critical) = match table {
            Table::Global => (0, CRITICAL_GLOBAL_SETTINGS),
            _ => (user, CRITICAL_SECURE_SETTINGS),
        };
        self.apply(caller, table, state_user, name.unwrap_or_default(), op, package, critical)
    }

    fn mutate_system(
        &self,
        caller: &Caller,
        name: Option<&str>,
        op: Op<'_>,
        value: Option<&str>,
        requesting: i32,
    ) -> Result<bool, Exception> {
        let package = self.calling_package(caller)?;
        // Without WRITE_SECURE_SETTINGS, the WRITE_SETTINGS app op decides.
        if !self.has(caller, WRITE_SECURE_SETTINGS)? && !self.check_and_note_write_settings(caller, package)? {
            eprintln!(
                "app host: settings: calling package {} is not allowed to write system settings: {}",
                package.unwrap_or("null"),
                name.unwrap_or("null")
            );
            return Ok(false);
        }
        let user = self.calling_user(caller, requesting)?;
        if self.restricted(name, value, caller.uid)? {
            eprintln!("app host: settings: user {user} is disallowed to change system setting: {name:?}");
            return Ok(false);
        }
        let name = name.unwrap_or_default();
        enforce_system_mutation(caller.app_id(), &op, name, &|| {
            let packages = &self.host.inputs()?.packages;
            let info = package
                .and_then(|p| packages.application_info(p, 0))
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Calling package doesn't exist"))?;
            Ok((info.private_flags & PRIVATE_FLAG_PRIVILEGED != 0, info.target_sdk_version))
        })?;
        let cache = ringtone_cache(name);
        // `isValidMediaUri`: a sound's type is MediaProvider's (#1525).
        if cache.is_some() && value.is_some() {
            return Err(self.host.unimplemented_named("settings provider", "a ringtone setting (#1525)"));
        }
        if let Op::Insert { value, .. } = op {
            validate_system_value(name, value)?;
        }
        // A reset is of the user asked for.
        let state_user = if matches!(op, Op::Reset { .. }) { requesting } else { user };
        if !self.apply(caller, Table::System, state_user, name, op, package, &[])? {
            return Ok(false);
        }
        if let Some(cache) = cache {
            let path = self.host.data()?.join(format!("system_de/{user}/ringtones/{cache}"));
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!("app host: settings: {}: {e}", path.display());
            }
        }
        Ok(true)
    }

    /// The `SettingsRegistry` mutation of `op` on user `user`'s `table`,
    /// and the notification of each change: whether the settings changed.
    #[allow(clippy::too_many_arguments)]
    fn apply(
        &self,
        caller: &Caller,
        table: Table,
        user: i32,
        name: &str,
        op: Op<'_>,
        package: Option<&str>,
        critical: &[&str],
    ) -> Result<bool, Exception> {
        if user != 0 {
            return Err(self.host.unimplemented_named("settings provider", "another user's settings (#1523)"));
        }
        let generations = self.generations()?;
        let settings = &self.host.inputs()?.settings;
        let changed = match op {
            Op::Insert { value, tag, make_default, overrideable_by_restore } => {
                if overrideable_by_restore && !self.has(caller, MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE)? {
                    return Err(Exception::security(format!(
                        "Caller is not allowed to modify settings overrideable by restore: Neither user {} nor current process has {MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE}.",
                        caller.uid
                    )));
                }
                let unset_undeclared = !self.predefined(table, name)? && settings.setting(table, name).is_none();
                let change = Change {
                    value,
                    tag,
                    make_default,
                    package,
                    caller_system: self.caller_system(caller, package)?,
                    overrideable_by_restore,
                };
                if !settings.insert(table, name, &change, critical.contains(&name))? {
                    return Ok(false);
                }
                // A setting added that is not declared: the unset ones'
                // generation goes too.
                if unset_undeclared {
                    generations.increment_unset(table, user);
                }
                vec![name.to_string()]
            }
            Op::Delete => {
                if !settings.delete(table, name, critical.contains(&name)) {
                    return Ok(false);
                }
                vec![name.to_string()]
            }
            Op::Reset { mode, tag } => settings.reset(table, mode, package, tag, &|p| self.system_package(p))?,
        };
        for name in &changed {
            self.notify(table, user, name)?;
        }
        Ok(!changed.is_empty())
    }

    /// `notifyForSettingsChange`: the setting's generation first, so
    /// observers always see the new value, then its URI's observers, for
    /// the running users (the app host's one) of a global setting.
    fn notify(&self, table: Table, user: i32, name: &str) -> Result<(), Exception> {
        self.generations()?.increment(table, user, name);
        let uri = Uri::parse(&format!("content://settings/{}/{name}", table.path()));
        if let Some(content) = self.host.content_service.get()
            && let Err(e) = content.notify_from_system(uri, user)
        {
            eprintln!("app host: settings: failed to notify for {user}: {name}: {e:?}");
        }
        Ok(())
    }

    /// `resolveCallingUserIdEnforcingPermissions`.
    fn calling_user(&self, caller: &Caller, requesting: i32) -> Result<i32, Exception> {
        if requesting == caller.user() {
            return Ok(requesting);
        }
        let user = self.host.incoming_user(caller.uid, requesting, false)?;
        if user < 0 {
            return Err(Exception::illegal_argument(format!("Call does not support special user #{user}")));
        }
        Ok(user)
    }

    /// `ContentProvider.getCallingPackage`: the attribution source's
    /// package, which must be the caller's (`AppOpsManager.checkPackage`).
    fn calling_package<'c>(&self, caller: &'c Caller) -> Result<Option<&'c str>, Exception> {
        let package = caller.package.as_deref();
        let owned = match package {
            Some(package) => self.host.owns_package(caller.uid, package)?,
            None => false,
        };
        if !owned {
            return Err(Exception::security(format!(
                "Package {} does not belong to {}",
                package.unwrap_or("null"),
                caller.uid
            )));
        }
        Ok(package)
    }

    /// `checkCallingOrSelfPermission(permission) == PERMISSION_GRANTED`.
    fn has(&self, caller: &Caller, permission: &str) -> Result<bool, Exception> {
        Ok(self.host.check_permission(Some(permission), caller.uid)? == 0)
    }

    /// `isCallerSystemOrShellOrRootOnDebuggableBuild`.
    fn trusted_for_reset(&self, caller: &Caller) -> bool {
        let debuggable = || {
            self.host.configured.get().and_then(|c| (c.properties)("ro.debuggable")).as_deref() == Some("1")
        };
        caller.app_id() == SYSTEM_UID || matches!(caller.app_id(), SHELL_UID | ROOT_UID) && debuggable()
    }

    /// `Settings.checkAndNoteWriteSettingsOperation(…, true)`: the
    /// WRITE_SETTINGS op noted, its default mode WRITE_SETTINGS'.
    fn check_and_note_write_settings(&self, caller: &Caller, package: Option<&str>) -> Result<bool, Exception> {
        let Some(package) = package else {
            return Ok(false);
        };
        match appops_service::note(&self.host, OP_WRITE_SETTINGS, caller.uid, Some(package))? {
            // `MODE_ALLOWED`.
            0 => return Ok(true),
            MODE_DEFAULT if self.has(caller, WRITE_SETTINGS)? => return Ok(true),
            _ => {}
        }
        Err(Exception::security(format!("{package} was not granted  this permission: {WRITE_SETTINGS}.")))
    }

    /// `isSettingRestrictedForUser` under the app host's user's
    /// restrictions.
    fn restricted(&self, name: Option<&str>, value: Option<&str>, uid: i32) -> Result<bool, Exception> {
        let Some(name) = name else {
            return Ok(false);
        };
        let restrictions = self.host.user_restrictions()?;
        Ok(setting_restricted(name, value, uid, &|r| restrictions.contains(r)))
    }

    /// `Setting.update`'s `callerSystem`: `isCalledFromSystem` (a uid of
    /// the system's) or `isSystemPackage`, never the shell or root.
    fn caller_system(&self, caller: &Caller, package: Option<&str>) -> Result<bool, Exception> {
        if package.is_some_and(shell_or_root) {
            return Ok(false);
        }
        Ok(caller.app_id() < FIRST_APPLICATION_UID as i32 || self.system_package(package)?)
    }

    /// `SettingsState.isSystemPackage`: the system and the setup wizard,
    /// apps of the system's uids, persistent system apps and apps signed
    /// with the platform key; not the shell or root.
    fn system_package(&self, package: Option<&str>) -> Result<bool, Exception> {
        let Some(package) = package.filter(|p| !shell_or_root(p)) else {
            return Ok(false);
        };
        if package == SYSTEM_PACKAGE_NAME || self.setup_wizard()?.as_deref() == Some(package) {
            return Ok(true);
        }
        let Some(info) = self.host.inputs()?.packages.application_info(package, 0) else {
            return Ok(false);
        };
        Ok(info.uid < FIRST_APPLICATION_UID as i32
            || info.flags & FLAG_PERSISTENT != 0 && info.flags & FLAG_SYSTEM != 0
            || info.private_flags & PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY != 0)
    }

    /// `getSetupWizardPackageName`: the package of the one system activity
    /// for `MAIN` in `SETUP_WIZARD`, if there is one.
    fn setup_wizard(&self) -> Result<&Option<String>, Exception> {
        if let Some(package) = self.setup_wizard.get() {
            return Ok(package);
        }
        let (_, resolution) = self.host.packages()?;
        let intent = crate::package::intent::Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec!["android.intent.category.SETUP_WIZARD".into()]),
            ..Default::default()
        };
        let query = flags::MATCH_SYSTEM_ONLY
            | flags::MATCH_DIRECT_BOOT_AWARE
            | flags::MATCH_DIRECT_BOOT_UNAWARE
            | flags::MATCH_DISABLED_COMPONENTS;
        let found = resolution
            .query_intent_activities(&intent, None, query, 0, SYSTEM_UID)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("setup wizard: {e:?}")))?;
        let package = match found.as_slice() {
            [one] => Some(one.component().0.to_string()),
            _ => None,
        };
        Ok(self.setup_wizard.get_or_init(|| package))
    }

    /// `isSettingPreDefined`: every flag is.
    fn predefined(&self, table: Table, name: &str) -> Result<bool, Exception> {
        let declared = self.declared()?;
        Ok(match table {
            Table::System => declared.system.contains(name),
            Table::Secure => declared.secure.contains(name),
            Table::Global => declared.global.contains(name),
            Table::Config => true,
        })
    }

    fn declared(&self) -> Result<&Declared, Exception> {
        if let Some(declared) = self.declared.get() {
            return Ok(declared);
        }
        let jar = self.host.root()?.join("system/framework/framework.jar");
        let declared =
            Declared::read(&jar).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", jar.display())))?;
        Ok(self.declared.get_or_init(|| declared))
    }

    /// The `GenerationRegistry`, for `UserManager.getMaxSupportedUsers`.
    fn generations(&self) -> Result<&GenerationRegistry, Exception> {
        if let Some(generations) = self.generations.get() {
            return Ok(generations);
        }
        let property = |name: &str| self.host.configured.get().and_then(|c| (c.properties)(name));
        let max_users = if property("ro.build.id").is_some_and(|id| id.starts_with("JVP")) {
            1
        } else {
            let configured = match property("fw.max_users").and_then(|v| v.parse::<i32>().ok()) {
                Some(users) => users,
                None => self
                    .host
                    .inputs()?
                    .packages
                    .framework_integer("config_multiuserMaximumUsers")
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no config_multiuserMaximumUsers"))?,
            };
            configured.max(1) as usize
        };
        Ok(self.generations.get_or_init(|| GenerationRegistry::new(max_users)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_types_compare_as_clip_description_compares_them() {
        assert!(compare_mime_types("vnd.android.cursor.item/system", "vnd.android.cursor.item/*"));
        assert!(compare_mime_types("vnd.android.cursor.item/system", "vnd.android.cursor.item/system"));
        assert!(!compare_mime_types("vnd.android.cursor.item/system", "audio/*"));
        assert!(!compare_mime_types("vnd.android.cursor.item/system", "*"));
    }

    #[test]
    fn an_open_failure_reads_as_errno_exception() {
        let e = std::io::Error::from_raw_os_error(libc::ENOENT);
        assert_eq!(open_failed(&e), "open failed: ENOENT (No such file or directory)");
    }

    /// A tracked `GET_secure`'s reply, as the original's to Calculator:
    /// the bundle in `ArrayMap` order, the `MemoryIntArray` as a
    /// length-prefixed parcelable whose `ParcelFileDescriptor` is an fd
    /// object, then its index and generation.
    #[test]
    fn tracked_reply_carries_the_generation_array() {
        let registry = GenerationRegistry::new(4);
        let result = value_result(Some("1".into()), registry.generation_data(Table::Secure, 0, "a"));
        let keys: Vec<_> = result.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["_track_generation", "value", "_generation_index", "_generation"]);
        let mut p = Parcel::new();
        bundle::write(&mut p, &result.iter().map(|(k, v)| (k.as_str(), v.clone())).collect::<Vec<_>>());
        let mut r = Reader::new(p.data(), p.objects());
        let entries = bundle::read(&mut r).unwrap().unwrap();
        assert_eq!(entries["value"], Value::String(Some("1".into())));
        assert_eq!(entries["_generation"], Value::Int(1));
        assert_eq!(entries["_generation_index"], Value::Int(0));
        let Value::Lazy { kind, start, len } = entries["_track_generation"] else {
            panic!("no parcelable");
        };
        assert_eq!(kind, 4); // VAL_PARCELABLE
        r.set_position(start);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android.util.MemoryIntArray"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android.os.ParcelFileDescriptor"));
        assert_eq!(r.read_i32(), Ok(0));
        assert_eq!(p.objects(), [r.position() as u64]);
        assert_eq!(p.files().len(), 1);
        r.read_fd().unwrap();
        assert_eq!(r.position(), start + len);
        // An unset value is a null, not a null string.
        let mut p = Parcel::new();
        bundle::write(&mut p, &[("value", value_result(None, Vec::new())[0].1.clone())]);
        assert_eq!(&p.data()[p.data().len() - 8..p.data().len() - 4], (-1i32).to_le_bytes());
    }

    #[test]
    fn restrictions_forbid_changes() {
        let set = |r: &str| r == "no_config_location" || r == "no_debugging_features";
        assert!(setting_restricted("location_mode", Some("0"), 10_050, &set));
        assert!(!setting_restricted("location_mode", Some("3"), SYSTEM_UID, &|_| false));
        assert!(setting_restricted("adb_enabled", Some("1"), 2000, &set));
        assert!(!setting_restricted("adb_enabled", Some("0"), 2000, &set));
        assert!(!setting_restricted("screen_brightness", Some("5"), SYSTEM_UID, &|_| true));
        assert!(setting_restricted("screen_brightness", Some("5"), 10_050, &|r| r == "no_config_brightness"));
        assert!(setting_restricted("data_roaming3", Some("1"), 10_050, &|r| r == "no_data_roaming"));
        assert!(!setting_restricted("font_scale", Some("1"), 10_050, &|_| true));
    }

    /// `SettingsProviderTest.testWriteSystemSetting*` and CTS's
    /// `Settings_SystemTest`: apps write public settings, privileged apps
    /// any; others fail by their target SDK.
    #[test]
    fn apps_write_public_system_settings() {
        let insert = Op::Insert { value: Some("1"), tag: None, make_default: false, overrideable_by_restore: false };
        let app = |privileged, sdk| move || Ok((privileged, sdk));
        assert!(enforce_system_mutation(10_050, &insert, "font_scale", &app(false, 34)).is_ok());
        let e = enforce_system_mutation(10_050, &insert, "pointer_speed", &app(false, 34)).unwrap_err();
        assert_eq!(e.message, "You cannot change private secure settings.");
        let e = enforce_system_mutation(10_050, &insert, "my_own", &app(false, 34)).unwrap_err();
        assert_eq!(e.message, "You cannot keep your settings in the secure settings.");
        assert!(enforce_system_mutation(10_050, &insert, "my_own", &app(false, 22)).is_ok());
        assert!(enforce_system_mutation(10_050, &insert, "my_own", &app(true, 34)).is_ok());
        assert!(enforce_system_mutation(SHELL_UID, &insert, "my_own", &app(false, 34)).is_ok());
        let e = enforce_system_mutation(10_050, &Op::Delete, "font_scale", &app(true, 34)).unwrap_err();
        assert_eq!(e.message, "You cannot delete system defined secure settings.");
        assert!(enforce_system_mutation(10_050, &Op::Delete, "my_own", &app(true, 34)).is_ok());
    }

    #[test]
    fn reset_modes() {
        assert_eq!(reset_mode(1, false).unwrap(), ResetMode::PackageDefaults);
        assert_eq!(reset_mode(4, true).unwrap(), ResetMode::TrustedDefaults);
        assert_eq!(
            reset_mode(2, false).unwrap_err().message,
            "Only system, shell/root on a debuggable build can reset to untrusted defaults"
        );
        assert_eq!(reset_mode(0, true).unwrap_err().message, "Invalid reset mode: 0");
    }

    #[test]
    fn attribution_source_package() {
        let mut p = Parcel::new();
        p.write_i32(0);
        for v in [-1, 10_050, 0] {
            p.write_i32(v);
        }
        p.write_string16(Some("com.example"));
        p.write_string16(Some("tag"));
        p.write_binder(None);
        p.write_i32(-1);
        p.write_i32(0);
        let size = p.position() as i32;
        p.set_i32_at(0, size);
        p.write_string16(Some("settings"));
        let mut r = Reader::new(p.data(), p.objects());
        let package = read_attribution_source(&mut r).unwrap();
        assert_eq!(package.as_deref(), Some("com.example"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("settings"));
    }

    /// The names hand-ported from `Settings.System` are the image's.
    #[test]
    #[ignore = "requires pinned original image; run explicitly"]
    fn system_names_are_the_framework_s() {
        let declared = Declared::read(&aim_paths::original_image().join("system/framework/framework.jar")).unwrap();
        let names = PUBLIC_SETTINGS.iter().chain(PRIVATE_SETTINGS).copied();
        for name in names.chain(crate::app_host::settings_validators::validated_names()) {
            assert!(declared.system.contains(name), "{name}");
        }
        assert!(declared.secure.contains("user_setup_complete"));
        assert!(declared.global.contains("device_provisioned"));
    }
}
