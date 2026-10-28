//! The `settings` provider for the app host: `SettingsProvider` at
//! `android-16.0.0_r1` over the settings the app host keeps
//! (`super::settings`). Its `call`: `GET_*` with the generation tracking a
//! client asks for (`_track_generation`, `super::generations`), `PUT_*`,
//! `DELETE_*` and `RESET_*` of the system, secure and global settings with
//! the original's permission, app op, user restriction and validator
//! checks (`super::settings_validators`), `LIST_*` of a table, and
//! DeviceConfig's calls: its flags listed with the aconfig defaults
//! (`super::device_config`), written, deleted, reset (banning what the
//! reset leaves) and set a namespace at a time by a WRITE_DEVICE_CONFIG
//! holder, the sync disabled mode, and a monitor told of accesses and
//! updates. A change bumps its generation, then is notified to the observers
//! of `content://settings/<table>/<name>` (`super::content_service`); no
//! backup is run, so none is told of it (#1925), and the daily copy of the
//! settings files is scheduled (`WriteFallbackSettingsFilesJobService`,
//! SettingsProvider's own code in the system process). Its table transactions: `query`
//! of a setting or a table (a `MatrixCursor` sent as `super::cursor`
//! sends it), `insert`, `bulkInsert`, `update`, `delete` and `getType`,
//! with the URI and where clause read as `Arguments` reads them and the
//! settings moved between tables (`Settings.*.getMovedTo*Settings`) found
//! in the table they moved to. A ringtone setting's sound must be audio or
//! video (`isValidMediaUri`, its type asked of its provider) and is copied
//! into the ringtone cache as the system (`openRingtone`). The `settings`
//! service runs the shell's `settings` command (`SettingsService`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use aim_android_image::dex::{self, Dex};
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, Reader,
    UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_content_icontentprovider as cp;
use aim_service_aidl::android_os_iremotecallback as remote_callback;
use aim_service_aidl::dev_aim_server_isettingsvalues as settings_values;

use super::activity_user::AllowMode;
use super::appops_service::{self, MODE_DEFAULT, OP_WRITE_SETTINGS};
use super::content_service::NOTIFY_SYNC_TO_NETWORK;
use super::cursor::{BulkCursor, Rows};
use super::device_config::{self, PUBLIC_NAMESPACES};
use super::generations::{self, GenerationRegistry, TRACK_GENERATION};
use super::job_info::{CONSTRAINT_FLAG_CHARGING, JobInfo};
use super::settings::{Change, ResetMode, SYSTEM_PACKAGE_NAME, Setting, Table};
use super::settings_validators::{PRIVATE_SETTINGS, PUBLIC_SETTINGS, validate_system_value};
use super::{AppHost, FIRST_APPLICATION_UID, PER_USER_RANGE, reply};
use crate::bundle::{self, Value};
use crate::package::info::{array_order, flags};
use crate::package::intent_filter::Plain;
use crate::package::restrictions::persistable;
use crate::package::uri::Uri;

/// SettingsProvider's package, and its `SETTINGS_PROVIDER_JOBS_NS`,
/// `WRITE_FALLBACK_SETTINGS_FILES_JOB_ID` and `ONE_DAY_INTERVAL_MILLIS`.
const SETTINGS_PACKAGE: &str = "com.android.providers.settings";
const SETTINGS_PROVIDER_JOBS_NS: &str = "SettingsProviderJobsNamespace";
const WRITE_FALLBACK_SETTINGS_FILES_JOB_ID: i32 = 1;
const ONE_DAY_INTERVAL_MILLIS: i64 = 24 * 60 * 60 * 1000;
/// `Settings.NameValueTable`'s columns: `_ID`, `NAME`, `VALUE`,
/// `IS_PRESERVED_IN_RESTORE`; `SettingsProvider.ALL_COLUMNS`.
const ID: &str = "_id";
const NAME: &str = "name";
const VALUE: &str = "value";
const IS_PRESERVED_IN_RESTORE: &str = "is_preserved_in_restore";
const ALL_COLUMNS: [&str; 4] = [ID, NAME, VALUE, IS_PRESERVED_IN_RESTORE];
/// `DatabaseHelper`'s valid tables; `SettingsProvider.REMOVED_LEGACY_TABLES`
/// among them.
const VALID_TABLES: [&str; 7] = [
    "system",
    "secure",
    "global",
    "bluetooth_devices",
    "favorites",
    "old_favorites",
    "android_metadata",
];
const REMOVED_LEGACY_TABLES: [&str; 5] = [
    "favorites",
    "old_favorites",
    "bluetooth_devices",
    "bookmarks",
    "android_metadata",
];
/// `ContentResolver.QUERY_ARG_SQL_SELECTION`, `QUERY_ARG_SQL_SELECTION_ARGS`.
const QUERY_ARG_SQL_SELECTION: &str = "android:query-arg-sql-selection";
const QUERY_ARG_SQL_SELECTION_ARGS: &str = "android:query-arg-sql-selection-args";
/// The provider's class (`getProviderName`), which names its cursor windows.
const PROVIDER_NAME: &str = "com.android.providers.settings.SettingsProvider";
/// The `Settings.CALL_METHOD_*_KEY`s; `SettingsProvider.RESULT_ROWS_DELETED`
/// and `RESULT_SETTINGS_LIST`.
const USER: &str = "_user";
const PREFIX: &str = "_prefix";
const TAG: &str = "_tag";
const MAKE_DEFAULT: &str = "_make_default";
const RESET_MODE: &str = "_reset_mode";
const OVERRIDEABLE_BY_RESTORE: &str = "_overrideable_by_restore";
const RESULT_ROWS_DELETED: &str = "result_rows_deleted";
const RESULT_SETTINGS_LIST: &str = "result_settings_list";
/// The permissions it checks.
const WRITE_SECURE_SETTINGS: &str = "android.permission.WRITE_SECURE_SETTINGS";
const WRITE_SETTINGS: &str = "android.permission.WRITE_SETTINGS";
const LOCAL_MAC_ADDRESS: &str = "android.permission.LOCAL_MAC_ADDRESS";
const MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE: &str =
    "android.permission.MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE";
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
/// The DeviceConfig calls' keys: `Settings.CALL_METHOD_FLAGS_KEY`,
/// `CALL_METHOD_SYNC_DISABLED_MODE_KEY`, `CALL_METHOD_MONITOR_CALLBACK_KEY`,
/// `KEY_CONFIG_SET_ALL_RETURN`, `KEY_CONFIG_GET_SYNC_DISABLED_MODE_RETURN`.
const FLAGS: &str = "_flags";
const SYNC_DISABLED_MODE: &str = "_disabled_mode";
const MONITOR_CALLBACK: &str = "_monitor_callback_key";
const SET_ALL_RETURN: &str = "config_set_all_return";
const GET_SYNC_DISABLED_MODE_RETURN: &str = "config_get_sync_disabled_mode_return";
/// `Settings.SET_ALL_RESULT_*`.
const SET_ALL_RESULT_FAILURE: i32 = 0;
const SET_ALL_RESULT_SUCCESS: i32 = 1;
const SET_ALL_RESULT_DISABLED: i32 = 2;
/// `Settings.Config.SYNC_DISABLED_MODE_*`.
const SYNC_DISABLED_MODE_NONE: i32 = 0;
const SYNC_DISABLED_MODE_PERSISTENT: i32 = 1;
const SYNC_DISABLED_MODE_UNTIL_REBOOT: i32 = 2;
/// `Settings.Global.DEVICE_CONFIG_SYNC_DISABLED`.
const DEVICE_CONFIG_SYNC_DISABLED: &str = "device_config_sync_disabled";
/// A monitor's results: `Settings.EXTRA_MONITOR_CALLBACK_TYPE`,
/// `EXTRA_ACCESS_CALLBACK`, `EXTRA_NAMESPACE_UPDATED_CALLBACK`,
/// `EXTRA_NAMESPACE`, `EXTRA_CALLING_PACKAGE`.
const EXTRA_MONITOR_CALLBACK_TYPE: &str = "monitor_callback_type";
const EXTRA_ACCESS_CALLBACK: &str = "access_callback";
const EXTRA_NAMESPACE_UPDATED_CALLBACK: &str = "namespace_updated_callback";
const EXTRA_NAMESPACE: &str = "namespace";
const EXTRA_CALLING_PACKAGE: &str = "calling_package";
/// The DeviceConfig permissions.
const WRITE_DEVICE_CONFIG: &str = "android.permission.WRITE_DEVICE_CONFIG";
const WRITE_ALLOWLISTED_DEVICE_CONFIG: &str = "android.permission.WRITE_ALLOWLISTED_DEVICE_CONFIG";
const READ_WRITE_SYNC_DISABLED_MODE_CONFIG: &str =
    "android.permission.READ_WRITE_SYNC_DISABLED_MODE_CONFIG";
const MONITOR_DEVICE_CONFIG_ACCESS: &str = "android.permission.MONITOR_DEVICE_CONFIG_ACCESS";
/// `ActivityManager.isRunningInUserTestHarness`'s property.
const TEST_HARNESS: &str = "persist.sys.test_harness";
/// `RemoteCallback`'s class, as a bundle holds it.
const REMOTE_CALLBACK: &str = "android.os.RemoteCallback";

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

    /// The table of the system, secure or global settings a path names.
    fn named(path: &str) -> Option<Table> {
        [Table::System, Table::Secure, Table::Global]
            .into_iter()
            .find(|t| t.path() == path)
    }
}

/// The caller of a call: its uid and its `AttributionSource`'s package.
pub(super) struct Caller {
    pub(super) uid: i32,
    pub(super) package: Option<String>,
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

/// A mutation (`MUTATION_OPERATION_*`); an update is an insertion of a
/// setting there is.
#[derive(Clone, Copy)]
enum Op<'a> {
    Insert {
        value: Option<&'a str>,
        tag: Option<&'a str>,
        make_default: bool,
        overrideable_by_restore: bool,
        update: bool,
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
            let name = if index == 1 {
                "classes.dex".to_string()
            } else {
                format!("classes{index}.dex")
            };
            let Some(bytes) = jar.file_if_present(&name).map_err(|e| e.0)? else {
                break;
            };
            let dex = Dex::parse(&bytes)?;
            for (set, class) in sets.iter_mut().zip(CLASSES) {
                if let Some(class) = dex.class(class) {
                    let names = dex
                        .static_values(class)?
                        .into_iter()
                        .filter_map(|(_, value)| match value {
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
        Ok(Declared {
            system,
            secure,
            global,
        })
    }
}

pub struct SettingsProvider {
    host: Arc<AppHost>,
    generations: OnceLock<GenerationRegistry>,
    declared: OnceLock<Declared>,
    /// `mSyncConfigDisabledUntilReboot`.
    sync_disabled_until_reboot: AtomicBool,
    /// `mConfigMonitorCallback`.
    monitor: Mutex<Option<Arc<Strong>>>,
    values_bridge: Mutex<Option<Arc<Strong>>>,
}

/// `SettingsProvider.scheduleWriteFallbackFilesJob`: once daily while
/// charging, a copy of user 0's settings files, unless one is pending.
fn schedule_write_fallback_files_job(host: &AppHost) -> Result<(), Exception> {
    // None before the job scheduler, as the original's SettingsProvider may
    // be created before JobSchedulerService.
    let Some(jobs) = host.job_scheduler.get() else {
        return Ok(());
    };
    if jobs
        .own_pending_job(
            SETTINGS_PROVIDER_JOBS_NS,
            WRITE_FALLBACK_SETTINGS_FILES_JOB_ID,
        )?
        .is_some()
    {
        return Ok(());
    }
    let mut job = JobInfo::persisted_builder(
        WRITE_FALLBACK_SETTINGS_FILES_JOB_ID,
        SETTINGS_PACKAGE.into(),
        format!("{SETTINGS_PACKAGE}.WriteFallbackSettingsFilesJobService"),
    );
    // `getSettingsFile` of each type for `USER_SYSTEM`.
    let file = |name: &str| {
        persistable::Value::String(format!("/data/system/users/0/settings_{name}.xml"))
    };
    job.extras.entries = ["global", "system", "secure", "ssaid", "config"]
        .into_iter()
        .map(|table| (Some(table.to_string()), file(table)))
        .collect();
    job.set_periodic(ONE_DAY_INTERVAL_MILLIS, ONE_DAY_INTERVAL_MILLIS);
    job.constraint_flags |= CONSTRAINT_FLAG_CHARGING;
    jobs.schedule_own(SETTINGS_PROVIDER_JOBS_NS, job)?;
    Ok(())
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
        match call.code {
            cp::CALL_TRANSACTION => {}
            cp::OPEN_FILE_TRANSACTION
            | cp::OPEN_ASSET_FILE_TRANSACTION
            | cp::OPEN_TYPED_ASSET_FILE_TRANSACTION => {
                return provider_reply(self.open(call));
            }
            cp::QUERY_TRANSACTION
            | cp::GET_TYPE_TRANSACTION
            | cp::INSERT_TRANSACTION
            | cp::BULK_INSERT_TRANSACTION
            | cp::DELETE_TRANSACTION
            | cp::UPDATE_TRANSACTION => return provider_reply(self.table_transaction(call)),
            code => {
                return reply(Err(self.host.unimplemented(
                    "settings provider",
                    self.descriptor(),
                    code,
                )));
            }
        }
        let r = &mut call.data;
        r.enforce_interface(cp::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let package = read_attribution_source(r)?;
        let _authority = r.read_string16().map_err(|_| BAD_VALUE)?;
        let method = r.read_string16().map_err(|_| BAD_VALUE)?.ok_or(BAD_VALUE)?;
        let arg = r.read_string16().map_err(|_| BAD_VALUE)?;
        let extras = bundle::read(r).map_err(|_| BAD_VALUE)?.unwrap_or_default();
        let caller = Caller {
            uid: call.sender_euid as i32,
            package,
        };
        reply(
            self.call(&caller, &method, arg.as_deref(), &extras, Some(&call.data))
                .map(|result| {
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    match result {
                        Some(result) => bundle::write(
                            &mut p,
                            &result
                                .iter()
                                .map(|(k, v)| (k.as_str(), v.clone()))
                                .collect::<Vec<_>>(),
                        ),
                        None => p.write_i32(-1),
                    }
                    p
                }),
        )
    }
}

/// `DatabaseUtils.writeExceptionToParcel`'s codes.
const FILE_NOT_FOUND: i32 = 1;
const ILLEGAL_ARGUMENT: i32 = 2;
/// `AssetFileDescriptor.UNKNOWN_LENGTH`.
const UNKNOWN_LENGTH: i64 = -1;

/// An exception a provider's transaction writes as
/// `DatabaseUtils.writeExceptionToParcel` writes it: one of its codes, the
/// binder's own, or a status for one no parcel carries (the transaction
/// fails).
enum Thrown {
    Database(i32, String),
    Binder(Exception),
    Status(i32),
}

impl From<Exception> for Thrown {
    fn from(e: Exception) -> Self {
        Thrown::Binder(e)
    }
}

impl From<i32> for Thrown {
    fn from(status: i32) -> Self {
        Thrown::Status(status)
    }
}

fn illegal_argument(message: String) -> Thrown {
    Thrown::Database(ILLEGAL_ARGUMENT, message)
}

/// A transaction's reply as `ContentProviderNative` writes it: the
/// result, or its exception.
fn provider_reply(result: Result<Parcel, Thrown>) -> Reply {
    match result {
        Ok(p) => Ok(p),
        Err(Thrown::Database(code, message)) => {
            let mut p = Parcel::new();
            p.write_i32(code);
            p.write_string16(Some(&message));
            Ok(p)
        }
        Err(Thrown::Binder(e)) => reply(Err(e)),
        Err(Thrown::Status(status)) => Err(status),
    }
}

/// `validateIncomingUri`: the settings authority (the provider is a single
/// user's, so the URI keeps its user). Its read and write permissions are
/// none, so `enforceReadPermission` and `enforceWritePermission` pass.
fn validate_authority(uri: &Uri) -> Result<(), Thrown> {
    let authority = uri.authority().unwrap_or_default();
    if authority.rsplit('@').next() != Some("settings") {
        return Err(Exception::security(format!(
            "The authority of the uri {} does not match the one of the contentProvider: settings",
            uri.as_str()
        ))
        .into());
    }
    Ok(())
}

/// A file a provider opened: `ParcelFileDescriptor`'s parcel, and for an
/// `AssetFileDescriptor` its whole extent and no extras.
struct Opened(crate::package::app_metadata::Descriptor);

/// A `ContentValues` value: simple text, or a scalar whose original Java
/// `toString` runs in the system process.
#[derive(Debug, PartialEq)]
enum Content {
    Text(Option<String>),
    Original(Vec<u8>),
}

/// `ContentValues.CREATOR.createFromParcel`: its map
/// (`readArrayMap`) of `writeValue`s, each as `getAsString` reads it.
fn read_content_values(r: &mut Reader<'_>) -> Result<HashMap<String, Content>, i32> {
    r.read_i32()?; // the capacity
    let n = r.read_i32()?;
    let mut values = HashMap::new();
    for _ in 0..n.max(0) {
        let key = r.read_string16()?.unwrap_or_default();
        let start = r.position();
        let kind = r.read_i32()?;
        let value = match kind {
            -1 => Content::Text(None),
            0 => Content::Text(r.read_string16()?),
            // `Integer`, `Short`, `Byte` and `Long`, `Boolean`.
            1 | 5 | 20 => Content::Text(Some(r.read_i32()?.to_string())),
            6 => Content::Text(Some(r.read_i64()?.to_string())),
            9 => Content::Text(Some((r.read_i32()? != 0).to_string())),
            7 | 8 | 13 => {
                let len = match kind {
                    7 => 4,
                    8 => 8,
                    _ => (r.read_i32()?.max(0) as usize).div_ceil(4) * 4,
                };
                r.skip(len)?;
                let (bytes, objects) = r.since(start);
                if !objects.is_empty() {
                    return Err(BAD_VALUE);
                }
                Content::Original(bytes.to_vec())
            }
            _ => return Err(BAD_VALUE),
        };
        values.insert(key, value);
    }
    Ok(values)
}

impl SettingsProvider {
    pub(super) fn attach_values(self: &Arc<Self>, binder: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::illegal_argument("no settings values bridge"));
        };
        let owner = Arc::new(self.host.process.strong(handle));
        let weak = Arc::downgrade(self);
        self.host.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(provider) = weak.upgrade() {
                    let mut current = provider.values_bridge.lock().unwrap();
                    if current.as_ref().is_some_and(|p| p.handle == handle) {
                        current.take();
                    }
                }
            }),
        );
        *self.values_bridge.lock().unwrap() = Some(owner);
        Ok(())
    }

    /// `ContentValues.getAsString(key)`.
    fn content_string(
        &self,
        values: &HashMap<String, Content>,
        key: &str,
    ) -> Result<Option<String>, Exception> {
        match values.get(key) {
            None | Some(Content::Text(None)) => Ok(None),
            Some(Content::Text(Some(s))) => Ok(Some(s.clone())),
            Some(Content::Original(value)) => {
                let owner = self.values_bridge.lock().unwrap().clone().ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "settings values bridge is not attached")
                })?;
                let mut data = Parcel::new();
                settings_values::GetAsString {
                    value: Some(value.clone()),
                }
                .write(&mut data);
                let transport = |status| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("settings values: binder status {status}"),
                    )
                };
                let result = owner
                    .transact(settings_values::GET_AS_STRING, &data, false)
                    .map_err(transport)?;
                settings_values::read_get_as_string_reply(&mut result.reader())
                    .map_err(transport)?
            }
        }
    }

    /// `ContentProviderNative`'s `QUERY`, `GET_TYPE`, `INSERT`,
    /// `BULK_INSERT`, `DELETE` and `UPDATE` through
    /// `ContentProvider.Transport` and the provider's defaults
    /// (`query`'s, `delete`'s and `update`'s selection from their
    /// arguments).
    fn table_transaction(&self, call: &mut Call<'_>) -> Result<Parcel, Thrown> {
        let (code, uid) = (call.code, call.sender_euid as i32);
        let r = &mut call.data;
        r.enforce_interface(cp::DESCRIPTOR)?;
        let package = read_attribution_source(r)?;
        let uri = Uri::read(r, &mut Plain)?.ok_or(BAD_VALUE)?;
        let caller = Caller { uid, package };
        validate_authority(&uri)?;
        let mut p = Parcel::new();
        match code {
            cp::QUERY_TRANSACTION => {
                let n = r.read_i32()?;
                let projection = (0..n.max(0))
                    .map(|_| r.read_string16())
                    .collect::<Result<Vec<_>, _>>()?;
                let args = bundle::read(r)?.unwrap_or_default();
                let observer = r.read_binder()?;
                r.read_binder()?; // the cancellation signal
                let (selection, selection_args) = selection(&args);
                let projection = (n > 0).then_some(projection);
                let rows = self.query(
                    &caller,
                    &uri,
                    projection.as_deref(),
                    selection,
                    selection_args.as_deref(),
                )?;
                let window_size = self.window_size()?;
                let cursor = BulkCursor::new(
                    self.host.process.clone(),
                    rows,
                    observer,
                    PROVIDER_NAME,
                    window_size,
                )?;
                let binder = self.host.process.add_service(cursor.clone());
                p.write_no_exception();
                p.write_i32(1);
                cursor.write_descriptor(binder, &mut p)?;
            }
            cp::GET_TYPE_TRANSACTION => {
                let mime_type = get_type(&uri)?;
                p.write_no_exception();
                p.write_string16(Some(&mime_type));
            }
            cp::INSERT_TRANSACTION => {
                let values = read_content_values(r)?;
                bundle::read(r)?; // the extras, which `insert(uri, values)` has not
                let inserted = self.insert(&caller, &uri, &values)?;
                p.write_no_exception();
                // `maybeAddUserId` of the URI's user, unless current (-2).
                let user = uri
                    .authority()
                    .unwrap_or_default()
                    .rsplit_once('@')
                    .map(|(u, _)| u.parse().unwrap_or(-10_000));
                let user = user.filter(|&user: &i32| user != -2);
                match (inserted, user) {
                    (Some(inserted), Some(user)) => inserted
                        .with_encoded_authority(Some(&format!("{user}@settings")))
                        .write(&mut p),
                    (Some(inserted), None) => inserted.write(&mut p),
                    (None, _) => p.write_i32(0),
                }
            }
            cp::BULK_INSERT_TRANSACTION => {
                // `createTypedArray`; `bulkInsert` inserts each.
                let n = r.read_i32()?;
                if n < 0 {
                    return Err(Exception::new(
                        EX_NULL_POINTER,
                        "Attempt to get length of null array",
                    )
                    .into());
                }
                let mut all = Vec::new();
                for _ in 0..n {
                    all.push(if r.read_i32()? != 0 {
                        Some(read_content_values(r)?)
                    } else {
                        None
                    });
                }
                let mut count = 0;
                for values in all {
                    let values = values.ok_or_else(|| {
                        Exception::new(EX_NULL_POINTER, "Attempt to invoke virtual method 'java.lang.String android.content.ContentValues.getAsString(java.lang.String)' on a null object reference")
                    })?;
                    count += self.insert(&caller, &uri, &values)?.is_some() as i32;
                }
                p.write_no_exception();
                p.write_i32(count);
            }
            cp::DELETE_TRANSACTION => {
                let extras = bundle::read(r)?.unwrap_or_default();
                let (selection, selection_args) = selection(&extras);
                let count = self.delete(&caller, &uri, selection, selection_args.as_deref())?;
                p.write_no_exception();
                p.write_i32(count);
            }
            _ => {
                let values = read_content_values(r)?;
                let extras = bundle::read(r)?.unwrap_or_default();
                let (selection, selection_args) = selection(&extras);
                let count =
                    self.update(&caller, &uri, &values, selection, selection_args.as_deref())?;
                p.write_no_exception();
                p.write_i32(count);
            }
        }
        Ok(p)
    }

    /// `ContentProviderNative`'s `OPEN_FILE`, `OPEN_ASSET_FILE` and
    /// `OPEN_TYPED_ASSET_FILE` through `ContentProvider.Transport`.
    fn open(&self, call: &mut Call<'_>) -> Result<Parcel, Thrown> {
        let (code, uid) = (call.code, call.sender_euid as i32);
        let r = &mut call.data;
        r.enforce_interface(cp::DESCRIPTOR)?;
        let package = read_attribution_source(r)?;
        let uri = Uri::read(r, &mut Plain)?.ok_or(BAD_VALUE)?;
        let mode = r.read_string16()?;
        let mime_type = if code == cp::OPEN_TYPED_ASSET_FILE_TRANSACTION {
            let mime_type = r.read_string16()?;
            bundle::read(r)?; // the options, which openFile ignores
            mime_type
        } else {
            None
        };
        let caller = Caller { uid, package };
        validate_authority(&uri)?;
        let Opened(fd) = match code {
            // `openTypedAssetFile`: any type, or the URI's.
            cp::OPEN_TYPED_ASSET_FILE_TRANSACTION => {
                let filter = mime_type.unwrap_or_default();
                if filter != "*/*" && !compare_mime_types(&get_type(&uri)?, &filter) {
                    return Err(Thrown::Database(
                        FILE_NOT_FOUND,
                        format!("Can't open {} as type {filter}", uri.as_str()),
                    ));
                }
                self.open_file(&caller, &uri, "r")?
            }
            _ => self.open_file(&caller, &uri, mode.as_deref().unwrap_or_default())?,
        };
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(1);
        aim_service_aidl::WriteParcelable::write_to(&fd, &mut p);
        if code != cp::OPEN_FILE_TRANSACTION {
            p.write_i64(0);
            p.write_i64(UNKNOWN_LENGTH);
            p.write_i32(0);
        }
        Ok(p)
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
            _ => {
                return Err(Thrown::Database(
                    ILLEGAL_ARGUMENT,
                    format!("Bad mode '{mode}'"),
                ));
            }
        };
        let path = self
            .ringtone_cache_dir(user)?
            .join(ringtone_cache(setting).unwrap_or_default());
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
            record_system(self.host.data()?, &path, 0o700)?;
        }
        let file = aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file))
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "settings provider: no fileport"))?;
        Ok(Opened(crate::package::app_metadata::Descriptor(file)))
    }

    /// `getRingtoneCacheDir`: user `user`'s, made if missing.
    fn ringtone_cache_dir(&self, user: i32) -> Result<PathBuf, Exception> {
        let dir = self
            .host
            .data()?
            .join(format!("system_de/{user}/ringtones"));
        if !dir.exists() {
            std::fs::create_dir(&dir).map_err(|e| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("/data/system_de/{user}/ringtones: {e}"),
                )
            })?;
            record_system(self.host.data()?, &dir, 0o700)?;
        }
        Ok(dir)
    }

    /// `isValidMediaUri`: a sound that is not a setting of this provider,
    /// set by a package there is, of an audio or video type as the
    /// content resolver tells it, asked as the system for a privileged
    /// app and as the app otherwise.
    fn valid_media_uri(
        &self,
        caller: &Caller,
        package: Option<&str>,
        name: &str,
        value: &str,
    ) -> Result<bool, Exception> {
        let uri = Uri::parse(value);
        if uri
            .authority()
            .is_some_and(|a| a.rsplit('@').next() == Some("settings"))
        {
            return Ok(false);
        }
        // `getCallingApplicationInfoOrThrow`.
        let info = package.and_then(|p| self.host.application_info(p, 0, None).ok().flatten());
        let Some(info) = info else {
            eprintln!(
                "app host: settings: isValidMediaUri: cannot get calling app info for setting: {name} URI: {value}"
            );
            return Ok(false);
        };
        let uid = if info.private_flags & PRIVATE_FLAG_PRIVILEGED != 0 {
            SYSTEM_UID
        } else {
            caller.uid
        };
        let Some(mime_type) = self.host.provider_type(&uri, uid)? else {
            eprintln!(
                "app host: settings: mutateSystemSetting for setting: {name} URI: {value} ignored: failure to find mimeType (no access from this context?)"
            );
            return Ok(false);
        };
        if !(mime_type.starts_with("audio/")
            || matches!(
                mime_type.as_str(),
                "application/ogg" | "application/x-flac" | "application/mp4"
            )
            || mime_type.starts_with("video/"))
        {
            eprintln!(
                "app host: settings: mutateSystemSetting for setting: {name} URI: {value} ignored: associated MIME type:{mime_type} is not a recognized audio or video type"
            );
            return Ok(false);
        }
        Ok(true)
    }

    /// The sound of a ringtone setting streamed into its cache as the
    /// system (`openRingtone`, then `FileUtils.copy`), so that it plays
    /// while credential storage is locked; a failure only logged. A sound
    /// the system cannot open directly would be asked of the ringtone
    /// player (#1584).
    fn cache_ringtone(&self, path: &Path, value: &str) {
        let copied = (|| -> Result<(), String> {
            let uri = Uri::parse(value);
            // `openRingtone`: directly, else through the ringtone player.
            let mut input = match self.host.open_provider_input(&uri) {
                Ok(input) => input,
                Err(e) => {
                    eprintln!(
                        "app host: settings: Failed to open directly; attempting failover: {e}"
                    );
                    self.host.open_ringtone_with_player(&uri)?
                }
            };
            let existed = path.exists();
            let mut out = std::fs::File::create(path)
                .map_err(|e| format!("java.io.FileNotFoundException: {}", open_failed(&e)))?;
            if !existed {
                record_system(self.host.data().map_err(|e| e.message)?, path, 0o600)
                    .map_err(|e| e.message)?;
            }
            std::io::copy(&mut input, &mut out)
                .map(|_| ())
                .map_err(|e| format!("java.io.IOException: {e}"))
        })();
        if let Err(e) = copied {
            eprintln!("app host: settings: Failed to cache ringtone: {e}");
        }
    }
}

/// A file or directory the system made, as the guest sees it.
fn record_system(data: &Path, path: &Path, mode: u32) -> Result<(), Exception> {
    let inode = aim_storage::guest_inode::GuestInode {
        uid: Some(SYSTEM_UID as u32),
        gid: Some(SYSTEM_UID as u32),
        mode: Some(mode),
    };
    aim_storage::guest_inode::record(path, inode).map_err(|e| {
        Exception::new(
            EX_ILLEGAL_STATE,
            format!("{}: {e}", crate::package::data_path(data, path).display()),
        )
    })
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
    let mut result = vec![(
        VALUE.to_string(),
        value.map_or(Value::Null, |v| Value::String(Some(v))),
    )];
    result.extend(generation);
    array_order(result, |(key, _)| key)
}

/// `UserRestrictionsUtils.isSettingRestrictedForUser`: whether a change of
/// `name` to `value` by `uid` is one a user restriction (`restricted`)
/// forbids.
fn setting_restricted(
    name: &str,
    value: Option<&str>,
    uid: i32,
    restricted: &dyn Fn(&str) -> bool,
) -> bool {
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
        "install_non_market_apps"
        | "adb_enabled"
        | "adb_wifi_enabled"
        | "airplane_mode_on"
        | "doze_enabled"
        | "doze_always_on"
        | "doze_pulse_on_pick_up"
        | "doze_pulse_on_long_press"
        | "doze_pulse_on_double_tap"
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
        "doze_enabled"
        | "doze_always_on"
        | "doze_pulse_on_pick_up"
        | "doze_pulse_on_long_press"
        | "doze_pulse_on_double_tap" => "no_ambient_display",
        "screen_brightness"
        | "screen_brightness_mode"
        | "auto_time"
        | "auto_time_zone"
        | "screen_off_timeout"
        | "private_dns_mode"
        | "private_dns_specifier"
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
            return Err(Exception::illegal_argument(
                "You cannot delete system defined secure settings.",
            ));
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
        _ => {
            return Err(Exception::illegal_argument(format!(
                "Invalid reset mode: {mode}"
            )));
        }
    };
    if !trusted_caller {
        return Err(Exception::security(format!(
            "Only system, shell/root on a debuggable build can {what}"
        )));
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

/// `String.split(separator)`: its trailing empty strings removed.
fn java_split(s: &str, separator: char) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split(separator).collect();
    while parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    if parts == [""] && !s.is_empty() {
        parts.clear();
    }
    parts
}

/// The `RemoteCallback` a bundle's `value` holds, read from `parcel`: its
/// `IRemoteCallback`'s handle, none for a null value or binder.
fn remote_callback_of(value: &Value, parcel: &Reader<'_>) -> Result<Option<u32>, Exception> {
    let Value::Lazy {
        kind: bundle::VAL_PARCELABLE,
        start,
        ..
    } = *value
    else {
        return Ok(None);
    };
    let mut r = parcel.clone();
    r.set_position(start);
    let class = r
        .read_string16()
        .map_err(|_| Exception::illegal_argument("a bad parcelable"))?;
    if class.as_deref() != Some(REMOTE_CALLBACK) {
        return Err(Exception::illegal_argument(format!(
            "{} is not a {REMOTE_CALLBACK}",
            class.unwrap_or_default()
        )));
    }
    match r
        .read_binder()
        .map_err(|_| Exception::illegal_argument("a bad parcelable"))?
    {
        Some(Binder::Handle(handle)) => Ok(Some(handle)),
        _ => Ok(None),
    }
}

/// A monitor's result bundle.
struct Results<'a>(&'a [(&'a str, Value)]);

impl aim_service_aidl::WriteParcelable for Results<'_> {
    fn write_to(&self, p: &mut Parcel) {
        bundle::write(p, self.0);
    }
}

/// `Settings.System.getMovedToSecureSettings`,
/// `System.getMovedToGlobalSettings` (`MOVED_TO_GLOBAL` and
/// `MOVED_TO_SECURE_THEN_GLOBAL`), `Secure.getMovedToGlobalSettings`,
/// `Global.getMovedToSecureSettings` and `Global.getMovedToSystemSettings`
/// at the tag.
const SYSTEM_MOVED_TO_SECURE: &[&str] = &[
    "adaptive_sleep",
    "android_id",
    "http_proxy",
    "location_providers_allowed",
    "lock_biometric_weak_flags",
    "lock_pattern_autolock",
    "lock_pattern_visible_pattern",
    "lock_pattern_tactile_feedback_enabled",
    "logging_id",
    "parental_control_enabled",
    "parental_control_last_update",
    "parental_control_redirect_url",
    "settings_classname",
    "use_google_mail",
    "wifi_networks_available_repeat_delay",
    "wifi_num_open_networks_kept",
    "wifi_on",
    "wifi_watchdog_acceptable_packet_loss_percentage",
    "wifi_watchdog_ap_count",
    "wifi_watchdog_background_check_delay_ms",
    "wifi_watchdog_background_check_enabled",
    "wifi_watchdog_background_check_timeout_ms",
    "wifi_watchdog_initial_ignored_ping_count",
    "wifi_watchdog_max_ap_checks",
    "wifi_watchdog_on",
    "wifi_watchdog_ping_count",
    "wifi_watchdog_ping_delay_ms",
    "wifi_watchdog_ping_timeout_ms",
    "install_non_market_apps",
];
const SYSTEM_MOVED_TO_GLOBAL: &[&str] = &[
    "airplane_mode_on",
    "airplane_mode_radios",
    "airplane_mode_toggleable_radios",
    "auto_time",
    "auto_time_zone",
    "car_dock_sound",
    "car_undock_sound",
    "desk_dock_sound",
    "desk_undock_sound",
    "dock_sounds_enabled",
    "lock_sound",
    "unlock_sound",
    "low_battery_sound",
    "power_sounds_enabled",
    "stay_on_while_plugged_in",
    "wifi_sleep_policy",
    "mode_ringer",
    "window_animation_scale",
    "transition_animation_scale",
    "animator_duration_scale",
    "fancy_ime_animations",
    "compatibility_mode",
    "emergency_tone",
    "call_auto_retry",
    "debug_app",
    "wait_for_debugger",
    "always_finish_activities",
    "tzinfo_content_url",
    "tzinfo_metadata_url",
    "selinux_content_url",
    "selinux_metadata_url",
    "sms_short_codes_content_url",
    "sms_short_codes_metadata_url",
    "cert_pin_content_url",
    "cert_pin_metadata_url",
    "nfc",
    "cell",
    "wifi",
    "bluetooth",
    "wimax",
    "show_processes",
    // `MOVED_TO_SECURE_THEN_GLOBAL`.
    "adb_enabled",
    "bluetooth_on",
    "data_roaming",
    "device_provisioned",
    "http_proxy",
    "network_preference",
    "usb_mass_storage_enabled",
    "wifi_mobile_data_transition_wakelock_timeout_ms",
    "wifi_max_dhcp_retry_count",
    "wifi_networks_available_notification_on",
];
const SECURE_MOVED_TO_GLOBAL: &[&str] = &[
    "adb_enabled",
    "assisted_gps_enabled",
    "bluetooth_on",
    "cdma_cell_broadcast_sms",
    "roaming_settings",
    "subscription_mode",
    "data_activity_timeout_mobile",
    "data_activity_timeout_wifi",
    "data_roaming",
    "development_settings_enabled",
    "device_provisioned",
    "display_size_forced",
    "download_manager_max_bytes_over_mobile",
    "download_manager_recommended_max_bytes_over_mobile",
    "mobile_data",
    "netstats_dev_bucket_duration",
    "netstats_dev_delete_age",
    "netstats_dev_persist_bytes",
    "netstats_dev_rotate_age",
    "netstats_enabled",
    "netstats_global_alert_bytes",
    "netstats_poll_interval",
    "netstats_sample_enabled",
    "netstats_time_cache_max_age",
    "netstats_uid_bucket_duration",
    "netstats_uid_delete_age",
    "netstats_uid_persist_bytes",
    "netstats_uid_rotate_age",
    "netstats_uid_tag_bucket_duration",
    "netstats_uid_tag_delete_age",
    "netstats_uid_tag_persist_bytes",
    "netstats_uid_tag_rotate_age",
    "network_preference",
    "nitz_update_diff",
    "nitz_update_spacing",
    "ntp_server",
    "ntp_timeout",
    "pdp_watchdog_error_poll_count",
    "pdp_watchdog_long_poll_interval_ms",
    "pdp_watchdog_max_pdp_reset_fail_count",
    "pdp_watchdog_poll_interval_ms",
    "pdp_watchdog_trigger_packet_count",
    "setup_prepaid_data_service_url",
    "setup_prepaid_detection_redir_host",
    "setup_prepaid_detection_target_url",
    "tether_dun_apn",
    "tether_dun_required",
    "tether_supported",
    "usb_mass_storage_enabled",
    "use_google_mail",
    "wifi_country_code",
    "wifi_framework_scan_interval_ms",
    "wifi_frequency_band",
    "wifi_idle_ms",
    "wifi_max_dhcp_retry_count",
    "wifi_mobile_data_transition_wakelock_timeout_ms",
    "wifi_networks_available_notification_on",
    "wifi_networks_available_repeat_delay",
    "wifi_num_open_networks_kept",
    "wifi_on",
    "wifi_p2p_device_name",
    "wifi_supplicant_scan_interval_ms",
    "wifi_verbose_logging_enabled",
    "wifi_enhanced_auto_join",
    "wifi_network_show_rssi",
    "wifi_watchdog_on",
    "wifi_watchdog_poor_network_test_enabled",
    "wifi_p2p_pending_factory_reset",
    "wimax_networks_available_notification_on",
    "verifier_timeout",
    "verifier_default_response",
    "data_stall_alarm_non_aggressive_delay_in_ms",
    "data_stall_alarm_aggressive_delay_in_ms",
    "gprs_register_check_period_ms",
    "wtf_is_fatal",
    "battery_discharge_duration_threshold",
    "battery_discharge_threshold",
    "send_action_app_error",
    "dropbox_age_seconds",
    "dropbox_max_files",
    "dropbox_quota_kb",
    "dropbox_quota_percent",
    "dropbox_reserve_percent",
    "dropbox:",
    "logcat_for_",
    "sys_free_storage_log_interval",
    "disk_free_change_reporting_threshold",
    "sys_storage_threshold_percentage",
    "sys_storage_threshold_max_bytes",
    "sys_storage_full_threshold_bytes",
    "sync_max_retry_delay_in_seconds",
    "connectivity_change_delay",
    "captive_portal_detection_enabled",
    "captive_portal_server",
    "set_install_location",
    "default_install_location",
    "inet_condition_debounce_up_delay",
    "inet_condition_debounce_down_delay",
    "read_external_storage_enforced_default",
    "http_proxy",
    "global_http_proxy_host",
    "global_http_proxy_port",
    "global_http_proxy_exclusion_list",
    "set_global_http_proxy",
    "default_dns_server",
    "preferred_network_mode",
    "webview_data_reduction_proxy_key",
    "secure_frp_mode",
];
const GLOBAL_MOVED_TO_SECURE: &[&str] = &[
    "install_non_market_apps",
    "zen_duration",
    "charging_sounds_enabled",
    "charging_vibration_enabled",
    "notification_bubbles",
    "bugreport_in_power_menu",
    "custom_bugreport_handler_app",
    "custom_bugreport_handler_user",
];
const GLOBAL_MOVED_TO_SYSTEM: &[&str] = &["apply_ramping_ringer"];

/// `Arguments`: the table and the setting a URI and a where clause name.
#[derive(Debug, PartialEq)]
struct Arguments {
    table: String,
    name: Option<String>,
}

/// `java.util.regex`'s `\s`.
fn java_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
}

/// The where clause inside `\s*` and, if `brackets`, `\(\s*...\s*\)\s*`;
/// then after `name\s*=\s*`.
fn where_value(clause: &str, brackets: bool) -> Option<&str> {
    let mut s = clause.trim_matches(java_space);
    if brackets {
        s = s
            .strip_prefix('(')?
            .strip_suffix(')')?
            .trim_matches(java_space);
    }
    s.strip_prefix("name")?
        .trim_start_matches(java_space)
        .strip_prefix('=')
        .map(|v| v.trim_start_matches(java_space))
}

/// `WHERE_PATTERN_WITH_PARAM_*`: `name = ?`, bare or in brackets.
fn where_with_param(clause: &str) -> bool {
    [false, true]
        .into_iter()
        .any(|brackets| where_value(clause, brackets) == Some("?"))
}

/// `WHERE_PATTERN_NO_PARAM_*`: `name = 'value'` (or `"value"`), bare or in
/// brackets; `.` matches no line terminator.
fn where_no_param(clause: &str) -> bool {
    [false, true].into_iter().any(|brackets| {
        where_value(clause, brackets).is_some_and(|v| {
            let mut chars = v.chars();
            let quoted = |c: Option<char>| matches!(c, Some('\'' | '"'));
            quoted(chars.next())
                && quoted(chars.next_back())
                && !chars.any(|c| matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'))
        })
    })
}

/// `Arrays.toString` of the where arguments.
fn arrays_to_string(args: Option<&[Option<String>]>) -> String {
    match args {
        None => "null".into(),
        Some(args) => format!(
            "[{}]",
            args.iter()
                .map(|a| a.as_deref().unwrap_or("null"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `new Arguments(uri, where, whereArgs, supportAll)`.
fn arguments(
    uri: &Uri,
    clause: Option<&str>,
    args: Option<&[Option<String>]>,
    support_all: bool,
) -> Result<Arguments, Thrown> {
    let segments = uri.path_segments();
    let table_for = |name: Option<String>| -> Result<Arguments, Thrown> {
        Ok(Arguments {
            table: table_for_setting(uri, name.as_deref())?,
            name,
        })
    };
    match (segments.len(), clause) {
        (1, Some(clause)) if where_with_param(clause) => match args {
            Some([name]) => return table_for(name.clone()),
            Some(_) => {}
            None => {
                return Err(
                    Exception::new(EX_NULL_POINTER, "Attempt to get length of null array").into(),
                );
            }
        },
        (1, Some(clause)) if where_no_param(clause) => {
            // `substring` between the first and the last quote, of
            // either kind; one before the other has no substring.
            let first = |q| clause.find(q).map_or(-1, |i| i as i64);
            let last = |q| clause.rfind(q).map_or(-1, |i| i as i64);
            let (start, end) = (first('\'').max(first('"')) + 1, last('\'').max(last('"')));
            if start > end {
                eprintln!(
                    "app host: settings: StringIndexOutOfBoundsException: begin {start}, end {end}"
                );
                return Err(UNKNOWN_TRANSACTION.into());
            }
            return table_for(Some(clause[start as usize..end as usize].to_string()));
        }
        (1, None) if support_all && args.is_none() => return table_for(None),
        (2, None) if args.is_none() => return table_for(Some(segments[1].clone())),
        _ => {}
    }
    Err(illegal_argument(format!(
        "Supported SQL:\n  uri content://some_table/some_property with null where and where args\n  uri content://some_table with query name=? and single name as arg\n  uri content://some_table with query name=some_name and null args\n  but got - uri:{}, where:{} whereArgs:{}",
        uri.as_str(),
        clause.unwrap_or("null"),
        arrays_to_string(args)
    )))
}

/// `getValidTableOrThrow`.
fn valid_table(uri: &Uri) -> Result<String, Thrown> {
    let segments = uri.path_segments();
    let table = segments
        .first()
        .ok_or_else(|| illegal_argument(format!("Invalid URI:{}", uri.as_str())))?;
    if !VALID_TABLES.contains(&table.as_str()) {
        return Err(illegal_argument(format!("Bad root path: {table}")));
    }
    Ok(table.clone())
}

/// `computeTableForSetting`: the URI's table, or the one its setting
/// moved to.
fn table_for_setting(uri: &Uri, name: Option<&str>) -> Result<String, Thrown> {
    let mut table = valid_table(uri)?;
    if let Some(name) = name {
        for (moved, to) in [
            (SYSTEM_MOVED_TO_SECURE, "secure"),
            (SYSTEM_MOVED_TO_GLOBAL, "global"),
            (SECURE_MOVED_TO_GLOBAL, "global"),
            (GLOBAL_MOVED_TO_SECURE, "secure"),
            (GLOBAL_MOVED_TO_SYSTEM, "system"),
        ] {
            if moved.contains(&name) {
                table = to.to_string();
            }
        }
    }
    Ok(table)
}

/// `SettingsProvider.getType`.
fn get_type(uri: &Uri) -> Result<String, Thrown> {
    let args = arguments(uri, None, None, true)?;
    let kind = if args.name.as_deref().is_none_or(str::is_empty) {
        "dir"
    } else {
        "item"
    };
    Ok(format!("vnd.android.cursor.{kind}/{}", args.table))
}

/// The selection `ContentProvider.query(uri, projection, queryArgs,
/// signal)`, `delete` and `update` take from their arguments.
fn selection(args: &HashMap<String, Value>) -> (Option<&str>, Option<Vec<Option<String>>>) {
    let clause = match args.get(QUERY_ARG_SQL_SELECTION) {
        Some(Value::String(s)) => s.as_deref(),
        _ => None,
    };
    let clause_args = match args.get(QUERY_ARG_SQL_SELECTION_ARGS) {
        Some(Value::StringArray(a)) => Some(a.clone()),
        _ => None,
    };
    (clause, clause_args)
}

/// `normalizeProjection`: all the columns, or those asked for if each is
/// one.
fn normalize_projection(projection: Option<&[Option<String>]>) -> Result<Vec<String>, Thrown> {
    let Some(projection) = projection else {
        return Ok(ALL_COLUMNS.map(str::to_string).to_vec());
    };
    projection
        .iter()
        .map(|column| match column.as_deref() {
            Some(c) if ALL_COLUMNS.contains(&c) => Ok(c.to_string()),
            c => Err(illegal_argument(format!(
                "Invalid column: {}",
                c.unwrap_or("null")
            ))),
        })
        .collect()
}

/// `appendSettingToCursor`'s row.
fn setting_row(setting: &Setting, columns: &[String]) -> Vec<Option<String>> {
    columns
        .iter()
        .map(|column| match column.as_str() {
            ID => Some(setting.id.to_string()),
            NAME => Some(setting.name.clone()),
            VALUE => setting.value.clone(),
            _ => Some(setting.preserved_in_restore.to_string()),
        })
        .collect()
}

/// `isKeyValid`: not empty, nor what `SettingsState.isBinary` would not
/// write to XML.
fn key_valid(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| matches!(c, '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}'))
}

impl SettingsProvider {
    pub fn new(host: Arc<AppHost>) -> SettingsProvider {
        SettingsProvider {
            host,
            generations: OnceLock::new(),
            declared: OnceLock::new(),
            sync_disabled_until_reboot: AtomicBool::new(false),
            monitor: Mutex::new(None),
            values_bridge: Mutex::new(None),
        }
    }

    /// `SettingsProvider.call`, its extras' length-prefixed values in
    /// `parcel`.
    pub(super) fn call(
        &self,
        caller: &Caller,
        method: &str,
        name: Option<&str>,
        extras: &HashMap<String, Value>,
        parcel: Option<&Reader<'_>>,
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
        // The DeviceConfig calls of no table's verb.
        match method {
            "SET_ALL_config" => {
                // `getSettingFlags`: the map `putSerializable` wrote as one.
                let flags = match (extras.get(FLAGS), parcel) {
                    (Some(value), Some(parcel)) => {
                        bundle::string_map(value, parcel).map_err(|_| {
                            Exception::illegal_argument("_flags is not a Map<String, String>")
                        })?
                    }
                    _ => None,
                };
                let result = self.set_all_config(caller, string(PREFIX), flags)?;
                return Ok(Some(vec![(SET_ALL_RETURN.to_string(), Value::Int(result))]));
            }
            "SET_SYNC_DISABLED_MODE_config" => {
                let mode = match extras.get(SYNC_DISABLED_MODE) {
                    Some(Value::Int(mode)) => *mode,
                    _ => -1,
                };
                if ![
                    SYNC_DISABLED_MODE_NONE,
                    SYNC_DISABLED_MODE_PERSISTENT,
                    SYNC_DISABLED_MODE_UNTIL_REBOOT,
                ]
                .contains(&mode)
                {
                    return Err(Exception::illegal_argument(format!(
                        "Invalid sync disabled mode: {mode}"
                    )));
                }
                self.set_sync_disabled_mode(caller, mode)?;
                return Ok(None);
            }
            "GET_SYNC_DISABLED_MODE_config" => {
                self.enforce_sync_disabled_mode_access(caller)?;
                let mode = self.sync_disabled_mode()?;
                return Ok(Some(vec![(
                    GET_SYNC_DISABLED_MODE_RETURN.to_string(),
                    Value::Int(mode),
                )]));
            }
            // `getAllConfigFlagNamespaces`, a `HashSet` put as a
            // serializable.
            "LIST_namespaces_config" => {
                let flags = self.host.platform()?.settings.config_flags(None)?;
                let mut namespaces: Vec<String> = flags
                    .keys()
                    .filter_map(|n| n.find('/').filter(|&i| i != 0).map(|i| n[..i].to_string()))
                    .collect();
                namespaces.sort();
                namespaces.dedup();
                let set = Value::Serializable {
                    class: "java.util.HashSet".into(),
                    data: device_config::serialize_hash_set(&namespaces),
                };
                return Ok(Some(vec![(VALUE.to_string(), set)]));
            }
            "REGISTER_MONITOR_CALLBACK_config" => {
                let callback = match (extras.get(MONITOR_CALLBACK), parcel) {
                    (Some(value), Some(parcel)) => remote_callback_of(value, parcel)?,
                    _ => None,
                };
                // `setMonitorCallback`: none is no change.
                if let Some(handle) = callback {
                    self.enforce_monitor(caller)?;
                    *self.monitor.lock().unwrap() =
                        Some(Arc::new(self.host.process.strong(handle)));
                }
                return Ok(None);
            }
            "UNREGISTER_MONITOR_CALLBACK_config" => {
                self.enforce_monitor(caller)?;
                *self.monitor.lock().unwrap() = None;
                return Ok(None);
            }
            _ => {}
        }
        let (verb, table) = match method.split_once('_') {
            Some((verb, "system")) => (verb, Some(Table::System)),
            Some((verb, "secure")) => (verb, Some(Table::Secure)),
            Some((verb, "global")) => (verb, Some(Table::Global)),
            Some((verb, "config")) => (verb, Some(Table::Config)),
            _ => (method, None),
        };
        Ok(match (verb, table) {
            ("GET", Some(table)) => Some(self.get(caller, table, name, requesting, tracking)?),
            ("PUT", Some(Table::Config)) => {
                self.insert_config(caller, name, string(VALUE), boolean(MAKE_DEFAULT))?;
                None
            }
            ("DELETE", Some(Table::Config)) => {
                let deleted = self.delete_config(caller, name)?;
                Some(vec![(
                    RESULT_ROWS_DELETED.to_string(),
                    Value::Int(deleted as i32),
                )])
            }
            ("RESET", Some(Table::Config)) => {
                let mode = match extras.get(RESET_MODE) {
                    Some(Value::Int(mode)) => *mode,
                    _ => 0,
                };
                let mode = reset_mode(mode, self.trusted_for_reset(caller))?;
                self.reset_config(caller, mode, string(PREFIX))?;
                None
            }
            ("PUT", Some(table)) => {
                // `insertSystemSetting` takes no tag and no default.
                let system = table == Table::System;
                let insert = Op::Insert {
                    value: string(VALUE),
                    tag: if system { None } else { string(TAG) },
                    make_default: !system && boolean(MAKE_DEFAULT),
                    overrideable_by_restore: boolean(OVERRIDEABLE_BY_RESTORE),
                    update: false,
                };
                self.mutate(caller, table, name, insert, requesting)?;
                None
            }
            ("RESET", Some(table)) => {
                let mode = match extras.get(RESET_MODE) {
                    Some(Value::Int(mode)) => *mode,
                    _ => 0,
                };
                let reset = Op::Reset {
                    mode: reset_mode(mode, self.trusted_for_reset(caller))?,
                    tag: string(TAG),
                };
                self.mutate(caller, table, None, reset, requesting)?;
                None
            }
            ("DELETE", Some(table)) => {
                let deleted = self.mutate(caller, table, name, Op::Delete, requesting)?;
                Some(vec![(
                    RESULT_ROWS_DELETED.to_string(),
                    Value::Int(deleted as i32),
                )])
            }
            // `getAllConfigFlags`, a `HashMap` put as a serializable and
            // so written as a map, then `reportDeviceConfigAccess`.
            ("LIST", Some(Table::Config)) => {
                let prefix = string(PREFIX);
                let mut flags: Vec<_> = self
                    .host
                    .platform()?
                    .settings
                    .config_flags(prefix)?
                    .into_iter()
                    .collect();
                flags.sort();
                let mut result = vec![(VALUE.to_string(), Value::StringMap(flags))];
                if tracking {
                    result.extend(self.generations()?.generation_data(
                        Table::Config,
                        0,
                        prefix.unwrap_or_default(),
                    ));
                }
                if let Some(prefix) = prefix {
                    let package = self.resolve_calling_package(caller)?;
                    self.report_to_monitor(&prefix.replace('/', ""), || {
                        vec![
                            (
                                EXTRA_MONITOR_CALLBACK_TYPE,
                                Value::String(Some(EXTRA_ACCESS_CALLBACK.into())),
                            ),
                            (EXTRA_CALLING_PACKAGE, Value::String(package.clone())),
                        ]
                    });
                }
                Some(array_order(result, |(key, _)| key))
            }
            // `buildSettingsList` of `getAll*Settings`.
            ("LIST", Some(table)) => {
                let lines = self.all(caller, table, requesting)?;
                let lines = lines
                    .iter()
                    .map(|s| format!("{}={}", s.name, s.value.as_deref().unwrap_or("null")))
                    .collect();
                Some(vec![(
                    RESULT_SETTINGS_LIST.to_string(),
                    Value::StringList(lines),
                )])
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
        let setting = self.setting_of(caller, table, name, requesting)?;
        let mut generation = Vec::new();
        if tracking {
            // A setting set or declared has its own generation, the unset
            // others share one.
            let key = if setting.is_some() || self.predefined(table, name)? {
                name
            } else {
                generations::UNSET
            };
            generation = self.generations()?.generation_data(table, requesting, key);
        }
        Ok(value_result(setting.and_then(|s| s.value), generation))
    }

    /// The setting `getGlobalSetting`, `getSecureSetting` and
    /// `getSystemSetting` read for `requesting` (a config flag's is user
    /// 0's): none when absent or hidden from the caller.
    fn setting_of(
        &self,
        caller: &Caller,
        table: Table,
        name: &str,
        requesting: i32,
    ) -> Result<Option<Setting>, Exception> {
        let user = match table {
            Table::Secure | Table::System => self.calling_user(caller, requesting)?,
            Table::Global | Table::Config => 0,
        };
        // `isSecureSettingAccessible`: the Bluetooth address only to who
        // may see the device's MAC addresses.
        let accessible = table != Table::Secure
            || name != "bluetooth_address"
            || self.has(caller, LOCAL_MAC_ADDRESS)?;
        // The app host keeps user 0's settings only (#1523).
        Ok(if accessible && user == 0 {
            self.host.platform()?.settings.setting(table, name)
        } else {
            None
        })
    }

    /// `getAllGlobalSettings`, `getAllSecureSettings` and
    /// `getAllSystemSettings`: the settings of `table` the caller sees.
    fn all(
        &self,
        caller: &Caller,
        table: Table,
        requesting: i32,
    ) -> Result<Vec<Setting>, Exception> {
        let user = if table == Table::Global {
            0
        } else {
            self.calling_user(caller, requesting)?
        };
        if user != 0 {
            return Err(self
                .host
                .unimplemented_named("settings provider", "another user's settings (#1523)"));
        }
        let hidden = table == Table::Secure && !self.has(caller, LOCAL_MAC_ADDRESS)?;
        let mut settings = self.host.platform()?.settings.all(table);
        settings.retain(|s| !(hidden && s.name == "bluetooth_address"));
        Ok(settings)
    }

    /// `SettingsProvider.query`: the setting the URI or its where clause
    /// names, or its table's settings, in the columns asked for.
    fn query(
        &self,
        caller: &Caller,
        uri: &Uri,
        projection: Option<&[Option<String>]>,
        selection: Option<&str>,
        selection_args: Option<&[Option<String>]>,
    ) -> Result<Rows, Thrown> {
        let args = arguments(uri, selection, selection_args, true)?;
        let columns = normalize_projection(projection)?;
        let settings = if REMOVED_LEGACY_TABLES.contains(&args.table.as_str()) {
            Vec::new()
        } else {
            let table = Table::named(&args.table)
                .ok_or_else(|| illegal_argument(format!("Invalid Uri path:{}", uri.as_str())))?;
            match &args.name {
                Some(name) => self
                    .setting_of(caller, table, name, caller.user())?
                    .into_iter()
                    .collect(),
                None => self.all(caller, table, caller.user())?,
            }
        };
        let rows = settings.iter().map(|s| setting_row(s, &columns)).collect();
        Ok(Rows { columns, rows })
    }

    /// `SettingsProvider.insert`: the URI of the setting changed, if it
    /// changed.
    fn insert(
        &self,
        caller: &Caller,
        uri: &Uri,
        values: &HashMap<String, Content>,
    ) -> Result<Option<Uri>, Thrown> {
        let table = valid_table(uri)?;
        if REMOVED_LEGACY_TABLES.contains(&table.as_str()) {
            return Ok(None);
        }
        let Some(name) = self.content_string(values, NAME)?.filter(|n| key_valid(n)) else {
            return Ok(None);
        };
        let value = self.content_string(values, VALUE)?;
        let table = Table::named(&table)
            .ok_or_else(|| illegal_argument(format!("Bad Uri path:{}", uri.as_str())))?;
        let insert = Op::Insert {
            value: value.as_deref(),
            tag: None,
            make_default: false,
            overrideable_by_restore: false,
            update: false,
        };
        let inserted = self.mutate(caller, table, Some(&name), insert, caller.user())?;
        Ok(inserted.then(|| {
            Uri::parse(&format!("content://settings/{}", table.path())).with_appended_path(&name)
        }))
    }

    /// `SettingsProvider.delete`: the rows deleted.
    fn delete(
        &self,
        caller: &Caller,
        uri: &Uri,
        selection: Option<&str>,
        selection_args: Option<&[Option<String>]>,
    ) -> Result<i32, Thrown> {
        let args = arguments(uri, selection, selection_args, false)?;
        if REMOVED_LEGACY_TABLES.contains(&args.table.as_str()) {
            return Ok(0);
        }
        let Some(name) = args.name.filter(|n| key_valid(n)) else {
            return Ok(0);
        };
        let table = Table::named(&args.table)
            .ok_or_else(|| illegal_argument(format!("Bad Uri path:{}", uri.as_str())))?;
        Ok(self.mutate(caller, table, Some(&name), Op::Delete, caller.user())? as i32)
    }

    /// `SettingsProvider.update`: the setting the URI or its where clause
    /// names set to the values' value, when there is one and the values
    /// name a setting; the rows updated.
    fn update(
        &self,
        caller: &Caller,
        uri: &Uri,
        values: &HashMap<String, Content>,
        selection: Option<&str>,
        selection_args: Option<&[Option<String>]>,
    ) -> Result<i32, Thrown> {
        let args = arguments(uri, selection, selection_args, false)?;
        if REMOVED_LEGACY_TABLES.contains(&args.table.as_str()) {
            return Ok(0);
        }
        if !self
            .content_string(values, NAME)?
            .is_some_and(|n| key_valid(&n))
        {
            return Ok(0);
        }
        let value = self.content_string(values, VALUE)?;
        let table = Table::named(&args.table)
            .ok_or_else(|| illegal_argument(format!("Invalid Uri path:{}", uri.as_str())))?;
        let update = Op::Insert {
            value: value.as_deref(),
            tag: None,
            make_default: false,
            overrideable_by_restore: false,
            update: true,
        };
        Ok(self.mutate(caller, table, args.name.as_deref(), update, caller.user())? as i32)
    }

    /// `CursorWindow.getCursorWindowSize`: `config_cursorWindowSize` KiB.
    fn window_size(&self) -> Result<usize, Exception> {
        let kib = self
            .host
            .framework()?
            .framework_integer("config_cursorWindowSize")
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no config_cursorWindowSize"))?;
        Ok(kib.max(0) as usize * 1024)
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
            return Err(Exception::security(format!(
                "Permission denial, must have one of: [{WRITE_SECURE_SETTINGS}]"
            )));
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
        self.apply(
            caller,
            table,
            state_user,
            name.unwrap_or_default(),
            op,
            package,
            critical,
        )
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
        if !self.has(caller, WRITE_SECURE_SETTINGS)?
            && !self.check_and_note_write_settings(caller, package)?
        {
            eprintln!(
                "app host: settings: calling package {} is not allowed to write system settings: {}",
                package.unwrap_or("null"),
                name.unwrap_or("null")
            );
            return Ok(false);
        }
        let user = self.calling_user(caller, requesting)?;
        if self.restricted(name, value, caller.uid)? {
            eprintln!(
                "app host: settings: user {user} is disallowed to change system setting: {name:?}"
            );
            return Ok(false);
        }
        let name = name.unwrap_or_default();
        enforce_system_mutation(caller.app_id(), &op, name, &|| {
            let (state, _) = self.host.packages()?;
            let info = package
                .map(|p| self.host.application_info(p, 0, Some(&state)))
                .transpose()?
                .flatten()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Calling package doesn't exist"))?;
            Ok((
                info.private_flags & PRIVATE_FLAG_PRIVILEGED != 0,
                info.target_sdk_version,
            ))
        })?;
        let cache = ringtone_cache(name)
            .map(|cache| self.ringtone_cache_dir(user).map(|dir| dir.join(cache)))
            .transpose()?;
        if cache.is_some()
            && let Some(value) = value
            && !self.valid_media_uri(caller, package, name, value)?
        {
            return Ok(false);
        }
        if let Op::Insert { value, .. } = op {
            validate_system_value(name, value)?;
        }
        // A reset is of the user asked for.
        let state_user = if matches!(op, Op::Reset { .. }) {
            requesting
        } else {
            user
        };
        if !self.apply(caller, Table::System, state_user, name, op, package, &[])? {
            return Ok(false);
        }
        if let Some(path) = cache {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!("app host: settings: {}: {e}", path.display());
            }
            if let (Op::Insert { .. }, Some(value)) = (op, value) {
                self.cache_ringtone(&path, value);
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
            return Err(self
                .host
                .unimplemented_named("settings provider", "another user's settings (#1523)"));
        }
        let generations = self.generations()?;
        let settings = &self.host.platform()?.settings;
        let changed = match op {
            Op::Insert {
                value,
                tag,
                make_default,
                overrideable_by_restore,
                update,
            } => {
                // `updateSettingLocked`: only a setting there is.
                if update && settings.setting(table, name).is_none() {
                    return Ok(false);
                }
                if overrideable_by_restore
                    && !self.has(caller, MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE)?
                {
                    return Err(Exception::security(format!(
                        "Caller is not allowed to modify settings overrideable by restore: Neither user {} nor current process has {MODIFY_SETTINGS_OVERRIDEABLE_BY_RESTORE}.",
                        caller.uid
                    )));
                }
                let unset_undeclared =
                    !self.predefined(table, name)? && settings.setting(table, name).is_none();
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
            Op::Reset { mode, tag } => {
                settings.reset(table, mode, package, tag, None, &|p| self.system_package(p))?
            }
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
        self.notify_uri(table, user, name);
        self.data_changed();
        Ok(())
    }

    /// `MSG_NOTIFY_DATA_CHANGED`, on a handler as the original posts it:
    /// no backup manager to tell (#1925), then
    /// `scheduleWriteFallbackFilesJob`.
    fn data_changed(&self) {
        self.host.post_job(|host| {
            if let Err(e) = schedule_write_fallback_files_job(host) {
                eprintln!(
                    "app host: settings: scheduling the fallback files job: {}",
                    e.message
                );
            }
        });
    }

    /// `content://settings/<table>/<path>`'s observers told of a change.
    fn notify_uri(&self, table: Table, user: i32, path: &str) {
        let uri = Uri::parse(&format!("content://settings/{}/{path}", table.path()));
        if let Some(content) = self.host.content_service.get()
            && let Err(e) = content.notify_from_system(uri, NOTIFY_SYNC_TO_NETWORK, user)
        {
            eprintln!("app host: settings: failed to notify for {user}: {path}: {e:?}");
        }
    }

    /// `notifyForConfigSettingsChangeLocked`: the prefix's generation, then
    /// one notification of the prefix and each changed flag's name past
    /// its namespace (`split("/")[1]`).
    fn notify_config_change(&self, prefix: &str, changed: &[String]) -> Result<(), Exception> {
        self.generations()?.increment(Table::Config, 0, prefix);
        let mut path = prefix.to_string();
        for name in changed {
            let flag = java_split(name, '/').get(1).copied().ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("ArrayIndexOutOfBoundsException: {name} has no flag name"),
                )
            })?;
            path.push_str(flag);
            path.push('/');
        }
        self.notify_uri(Table::Config, 0, &path);
        self.data_changed();
        Ok(())
    }

    /// `resolveCallingPackage`: root's and the shell's by their uids, else
    /// the caller's (`getCallingPackage`).
    fn resolve_calling_package(&self, caller: &Caller) -> Result<Option<String>, Exception> {
        Ok(match caller.uid {
            ROOT_UID => Some("root".into()),
            SHELL_UID => Some("com.android.shell".into()),
            _ => self.calling_package(caller)?.map(str::to_string),
        })
    }

    /// `enforceDeviceConfigWritePermission`: WRITE_DEVICE_CONFIG, whose
    /// background users' deny list (`assertCallingUserDenyList`) spares
    /// the system user, the app host's one. A caller with only
    /// WRITE_ALLOWLISTED_DEVICE_CONFIG is not implemented (#1776).
    fn enforce_device_config_write(&self, caller: &Caller) -> Result<(), Exception> {
        if self.has(caller, WRITE_DEVICE_CONFIG)? {
            if caller.user() != 0 {
                return Err(self.host.unimplemented_named(
                    "settings provider",
                    "another user's DeviceConfig writes (#1523)",
                ));
            }
            return Ok(());
        }
        if self.has(caller, WRITE_ALLOWLISTED_DEVICE_CONFIG)? {
            return Err(self.host.unimplemented_named(
                "settings provider",
                "allowlisted DeviceConfig writes (#1776)",
            ));
        }
        Err(Exception::security(
            "Permission denial to mutate flag, must have root, WRITE_DEVICE_CONFIG, or WRITE_ALLOWLISTED_DEVICE_CONFIG",
        ))
    }

    /// `insertConfigSetting`, as `mutateConfigSetting` inserts it: the
    /// caller's, as a package that is not the system's
    /// (`forceNonSystemPackage`), its staged name notified too for an
    /// aconfig flag (`notifyIndividualAconfigSyspropChanged`).
    fn insert_config(
        &self,
        caller: &Caller,
        name: Option<&str>,
        value: Option<&str>,
        make_default: bool,
    ) -> Result<bool, Exception> {
        let package = self.resolve_calling_package(caller)?;
        self.enforce_device_config_write(caller)?;
        let name = name.unwrap_or_default();
        let settings = &self.host.platform()?.settings;
        let change = Change {
            value,
            tag: None,
            make_default,
            package: package.as_deref(),
            caller_system: false,
            overrideable_by_restore: false,
        };
        if !settings.insert(Table::Config, name, &change, false)? {
            return Ok(false);
        }
        self.notify(Table::Config, 0, name)?;
        if let Some(slash) = name.find('/').filter(|&i| i != 0)
            && settings.aconfig_flag(&name[slash + 1..]).is_some()
        {
            self.notify(
                Table::Config,
                0,
                &format!("staged/{}*{}", &name[..slash], &name[slash + 1..]),
            )?;
        }
        Ok(true)
    }

    /// `deleteConfigSetting`.
    fn delete_config(&self, caller: &Caller, name: Option<&str>) -> Result<bool, Exception> {
        self.resolve_calling_package(caller)?;
        self.enforce_device_config_write(caller)?;
        let name = name.unwrap_or_default();
        if !self
            .host
            .platform()?
            .settings
            .delete(Table::Config, name, false)
        {
            return Ok(false);
        }
        self.notify(Table::Config, 0, name)?;
        Ok(true)
    }

    /// `resetConfigSetting`: the flags `prefix` starts (all without one)
    /// in `mode`, those of a namespace banned first unless the shell
    /// resets them (`banConfigurationIfNecessary`).
    fn reset_config(
        &self,
        caller: &Caller,
        mode: ResetMode,
        prefix: Option<&str>,
    ) -> Result<(), Exception> {
        let package = self.resolve_calling_package(caller)?;
        let settings = &self.host.platform()?.settings;
        settings.config_flags(prefix)?;
        self.enforce_device_config_write(caller)?;
        if caller.app_id() != SHELL_UID {
            match prefix {
                Some(prefix) => settings.ban_config(prefix, &settings.config_flags(Some(prefix))?),
                None => {
                    for prefix in settings.config_prefixes() {
                        settings.ban_config(&prefix, &settings.config_flags(Some(&prefix))?);
                    }
                }
            }
        }
        let changed = settings.reset(
            Table::Config,
            mode,
            package.as_deref(),
            None,
            prefix,
            &|p| self.system_package(p),
        )?;
        for name in &changed {
            self.notify(Table::Config, 0, name)?;
        }
        Ok(())
    }

    /// `setAllConfigSettings` of `prefix` to `flags` (`SET_ALL_RESULT_*`),
    /// the map as `readHashMapInternal` made it, iterated in its order.
    fn set_all_config(
        &self,
        caller: &Caller,
        prefix: Option<&str>,
        flags: Option<Vec<(String, Option<String>)>>,
    ) -> Result<i32, Exception> {
        let Some(flags) = flags else {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke interface method 'java.util.Set java.util.Map.keySet()' on a null object reference",
            ));
        };
        self.enforce_device_config_write(caller)?;
        let package = self.resolve_calling_package(caller)?;
        if self.sync_disabled_mode()? != SYNC_DISABLED_MODE_NONE {
            eprintln!(
                "app host: settings: did not write settings for prefix '{}' because sync is disabled",
                prefix.unwrap_or("null")
            );
            return Ok(SET_ALL_RESULT_DISABLED);
        }
        let Some(prefix) = prefix else {
            return Err(Exception::new(
                EX_NULL_POINTER,
                "Attempt to invoke virtual method 'int java.lang.String.lastIndexOf(int)' on a null object reference",
            ));
        };
        let capacity = device_config::sized_capacity(flags.len());
        let mut flags = device_config::hash_map_order(flags, capacity);
        let caller_system = self.caller_system(caller, package.as_deref())?;
        let settings = &self.host.platform()?.settings;
        let Some(changed) =
            settings.set_config(prefix, &mut flags, package.as_deref(), caller_system)
        else {
            return Ok(SET_ALL_RESULT_FAILURE);
        };
        if !changed.is_empty() {
            self.report_to_monitor(&prefix.replace('/', ""), || {
                vec![(
                    EXTRA_MONITOR_CALLBACK_TYPE,
                    Value::String(Some(EXTRA_NAMESPACE_UPDATED_CALLBACK.into())),
                )]
            });
            self.notify_config_change(prefix, &changed)?;
        }
        Ok(SET_ALL_RESULT_SUCCESS)
    }

    /// `enforceHasAtLeastOnePermission(WRITE_DEVICE_CONFIG,
    /// READ_WRITE_SYNC_DISABLED_MODE_CONFIG)`.
    fn enforce_sync_disabled_mode_access(&self, caller: &Caller) -> Result<(), Exception> {
        if self.has(caller, WRITE_DEVICE_CONFIG)?
            || self.has(caller, READ_WRITE_SYNC_DISABLED_MODE_CONFIG)?
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Permission denial, must have one of: [{WRITE_DEVICE_CONFIG}, {READ_WRITE_SYNC_DISABLED_MODE_CONFIG}]"
        )))
    }

    /// `setSyncDisabledModeConfig`: until reboot in memory, persistent as
    /// the global `DEVICE_CONFIG_SYNC_DISABLED`, set by the system.
    fn set_sync_disabled_mode(&self, caller: &Caller, mode: i32) -> Result<(), Exception> {
        self.enforce_sync_disabled_mode_access(caller)?;
        self.sync_disabled_until_reboot
            .store(mode == SYNC_DISABLED_MODE_UNTIL_REBOOT, Ordering::Relaxed);
        let value = if mode == SYNC_DISABLED_MODE_PERSISTENT {
            "1"
        } else {
            "0"
        };
        let settings = &self.host.platform()?.settings;
        let unset = !self.predefined(Table::Global, DEVICE_CONFIG_SYNC_DISABLED)?
            && settings
                .setting(Table::Global, DEVICE_CONFIG_SYNC_DISABLED)
                .is_none();
        let change = Change {
            value: Some(value),
            tag: None,
            make_default: false,
            package: Some(SYSTEM_PACKAGE_NAME),
            caller_system: true,
            overrideable_by_restore: false,
        };
        if settings.insert(Table::Global, DEVICE_CONFIG_SYNC_DISABLED, &change, false)? {
            self.notify(Table::Global, 0, DEVICE_CONFIG_SYNC_DISABLED)?;
            if unset {
                self.generations()?.increment_unset(Table::Global, 0);
            }
        }
        Ok(())
    }

    /// `getSyncDisabledModeConfigLocked`: until reboot, else as the global
    /// setting says, persistent by default in a user test harness.
    fn sync_disabled_mode(&self) -> Result<i32, Exception> {
        if self.sync_disabled_until_reboot.load(Ordering::Relaxed) {
            return Ok(SYNC_DISABLED_MODE_UNTIL_REBOOT);
        }
        Ok(
            match self
                .host
                .platform()?
                .settings
                .get(Table::Global, DEVICE_CONFIG_SYNC_DISABLED)
            {
                None if super::system_boolean(
                    self.host.property(TEST_HARNESS).as_deref(),
                    false,
                ) =>
                {
                    SYNC_DISABLED_MODE_PERSISTENT
                }
                None => SYNC_DISABLED_MODE_NONE,
                Some(value) if value != "0" => SYNC_DISABLED_MODE_PERSISTENT,
                Some(_) => SYNC_DISABLED_MODE_NONE,
            },
        )
    }

    /// `enforceCallingOrSelfPermission(MONITOR_DEVICE_CONFIG_ACCESS)`.
    fn enforce_monitor(&self, caller: &Caller) -> Result<(), Exception> {
        if self.has(caller, MONITOR_DEVICE_CONFIG_ACCESS)? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Permission denial: registering for config access requires: {MONITOR_DEVICE_CONFIG_ACCESS}: Neither user {} nor current process has {MONITOR_DEVICE_CONFIG_ACCESS}.",
            caller.uid
        )))
    }

    /// `reportDeviceConfigAccess` and `reportDeviceConfigUpdate`: the
    /// monitor sent `extras` and the namespace, unless it is public; a
    /// failed send is logged where `RemoteCallback.sendResult` ignores it.
    fn report_to_monitor(
        &self,
        namespace: &str,
        extras: impl FnOnce() -> Vec<(&'static str, Value)>,
    ) {
        if PUBLIC_NAMESPACES.contains(&namespace) {
            return;
        }
        let Some(monitor) = self.monitor.lock().unwrap().clone() else {
            return;
        };
        let mut entries = extras();
        entries.push((EXTRA_NAMESPACE, Value::String(Some(namespace.to_string()))));
        let mut request = Parcel::new();
        remote_callback::SendResult {
            data: Some(Results(&entries)),
        }
        .write(&mut request);
        if let Err(e) = monitor.transact(remote_callback::SEND_RESULT, &request, true) {
            eprintln!("app host: settings: the DeviceConfig monitor: {e}");
        }
    }

    /// `resolveCallingUserIdEnforcingPermissions`.
    fn calling_user(&self, caller: &Caller, requesting: i32) -> Result<i32, Exception> {
        if requesting == caller.user() {
            return Ok(requesting);
        }
        let user = self.host.handle_incoming_user(
            caller.uid,
            requesting,
            false,
            AllowMode::FullOnly,
            Some("get/set setting for user"),
            None,
        )?;
        Ok(user)
    }

    /// `ContentProvider.getCallingPackage`: the attribution source's
    /// package, which must be the caller's (`AppOpsManager.checkPackage`).
    fn calling_package<'c>(&self, caller: &'c Caller) -> Result<Option<&'c str>, Exception> {
        let package = caller.package.as_deref();
        let owned = match package {
            Some(package) => self.host.app_ops_owns_package(caller.uid, package)?,
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
            self.host
                .configured
                .get()
                .and_then(|c| (c.properties)("ro.debuggable"))
                .as_deref()
                == Some("1")
        };
        caller.app_id() == SYSTEM_UID
            || matches!(caller.app_id(), SHELL_UID | ROOT_UID) && debuggable()
    }

    /// `Settings.checkAndNoteWriteSettingsOperation(…, true)`: the
    /// WRITE_SETTINGS op noted, its default mode WRITE_SETTINGS'.
    fn check_and_note_write_settings(
        &self,
        caller: &Caller,
        package: Option<&str>,
    ) -> Result<bool, Exception> {
        let Some(package) = package else {
            return Ok(false);
        };
        match appops_service::note(&self.host, OP_WRITE_SETTINGS, caller.uid, Some(package))? {
            // `MODE_ALLOWED`.
            0 => return Ok(true),
            MODE_DEFAULT if self.has(caller, WRITE_SETTINGS)? => return Ok(true),
            _ => {}
        }
        Err(Exception::security(format!(
            "{package} was not granted  this permission: {WRITE_SETTINGS}."
        )))
    }

    /// `isSettingRestrictedForUser` under the app host's user's
    /// restrictions.
    fn restricted(
        &self,
        name: Option<&str>,
        value: Option<&str>,
        uid: i32,
    ) -> Result<bool, Exception> {
        let Some(name) = name else {
            return Ok(false);
        };
        let restrictions = self.host.user_restrictions()?;
        Ok(setting_restricted(name, value, uid, &|r| {
            restrictions.contains(r)
        }))
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
        let Some(info) = self.host.application_info(package, 0, None)? else {
            return Ok(false);
        };
        Ok(info.uid < FIRST_APPLICATION_UID as i32
            || info.flags & FLAG_PERSISTENT != 0 && info.flags & FLAG_SYSTEM != 0
            || info.private_flags & PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY != 0)
    }

    /// `getSetupWizardPackageName`: the package of the one system activity
    /// for `MAIN` in `SETUP_WIZARD`, if there is one.
    fn setup_wizard(&self) -> Result<Option<String>, Exception> {
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
        Ok(package)
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
        let declared = Declared::read(&jar)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", jar.display())))?;
        Ok(self.declared.get_or_init(|| declared))
    }

    /// The `GenerationRegistry`, for `UserManager.getMaxSupportedUsers`.
    fn generations(&self) -> Result<&GenerationRegistry, Exception> {
        if let Some(generations) = self.generations.get() {
            return Ok(generations);
        }
        let property = |name: &str| {
            self.host
                .configured
                .get()
                .and_then(|c| (c.properties)(name))
        };
        let max_users = if property("ro.build.id").is_some_and(|id| id.starts_with("JVP")) {
            1
        } else {
            let configured = match property("fw.max_users").and_then(|v| v.parse::<i32>().ok()) {
                Some(users) => users,
                None => self
                    .host
                    .framework()?
                    .framework_integer("config_multiuserMaximumUsers")
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "no config_multiuserMaximumUsers")
                    })?,
            };
            configured.max(1) as usize
        };
        Ok(self
            .generations
            .get_or_init(|| GenerationRegistry::new(max_users)))
    }
}

#[cfg(test)]
mod tests {
    struct ValuesBridge(Mutex<(Vec<Vec<u8>>, Result<Option<String>, Exception>)>);
    impl Service for ValuesBridge {
        fn descriptor(&self) -> &str {
            settings_values::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, SYSTEM_UID as u32);
            assert_eq!(call.code, settings_values::GET_AS_STRING);
            let args = settings_values::GetAsString::read(&mut call.data)?;
            assert_eq!(call.data.remaining(), 0);
            let mut state = self.0.lock().unwrap();
            state.0.push(args.value.unwrap());
            reply(state.1.clone().map(|text| {
                let mut p = Parcel::new();
                settings_values::write_get_as_string_reply(&mut p, &text);
                p
            }))
        }
    }

    #[test]
    fn scalar_values_preserve_wire_bits_store_java_strings_and_propagate_failures() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let permissions = super::super::permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
            assert!(host.user_restrictions.set(Default::default()).is_ok());
            let provider = Arc::new(SettingsProvider::new(host.clone()));
            assert!(provider.generations.set(GenerationRegistry::new(1)).is_ok());
            assert!(
                provider
                    .declared
                    .set(Declared {
                        system: HashSet::new(),
                        secure: HashSet::new(),
                        global: HashSet::new(),
                    })
                    .is_ok()
            );
            let original = Arc::new(ValuesBridge(Mutex::new((Vec::new(), Ok(None)))));
            let owner = export(original.clone());
            for invalid in [None, Some(Binder::Local(1))] {
                assert!(provider.attach_values(invalid).is_err());
            }
            let caller = Caller {
                uid: ROOT_UID,
                package: Some("root".into()),
            };
            for (kind, bits, expected) in [
                (7, 1.5f32.to_bits() as i64, "1.5"),
                (8, 1.25f64.to_bits() as i64, "1.25"),
                (13, 0, "[B@abc"),
            ] {
                let name = if kind == 13 {
                    "byte_setting"
                } else {
                    "font_scale"
                };
                let mut p = Parcel::new();
                p.write_i32(2);
                p.write_i32(2);
                p.write_string16(Some(NAME));
                p.write_i32(0);
                p.write_string16(Some(name));
                p.write_string16(Some(VALUE));
                let start = p.position();
                p.write_i32(kind);
                if kind == 8 {
                    p.write_i64(bits);
                } else {
                    p.write_i32(bits as i32);
                }
                let values = read_content_values(&mut p.reader()).unwrap();
                if kind == 7 {
                    assert!(provider.content_string(&values, VALUE).is_err());
                    provider
                        .attach_values(Some(Binder::Handle(owner.handle)))
                        .unwrap();
                }
                original.0.lock().unwrap().1 = Ok(Some(expected.into()));
                let uri = Uri::parse("content://settings/system");
                let result = if kind == 8 {
                    provider
                        .update(&caller, &uri.with_appended_path(name), &values, None, None)
                        .map(|count| count == 1)
                } else {
                    provider
                        .insert(&caller, &uri, &values)
                        .map(|uri| uri.is_some())
                };
                assert!(
                    result
                        .map_err(|e| match e {
                            Thrown::Binder(e) => e.message,
                            _ => "provider exception".into(),
                        })
                        .unwrap()
                );
                assert_eq!(
                    host.platform()
                        .unwrap()
                        .settings
                        .get(Table::System, name)
                        .as_deref(),
                    Some(expected)
                );
                assert_eq!(
                    original.0.lock().unwrap().0.last().unwrap(),
                    &p.data()[start..]
                );
                original.0.lock().unwrap().1 = Err(Exception::security("original failure"));
                assert_eq!(
                    provider.content_string(&values, VALUE).unwrap_err(),
                    Exception::security("original failure")
                );
            }
        });
    }

    /// PMS.getSetupWizardPackageNameImpl deliberately includes disabled
    /// components and applications in the system-only setup-wizard query.
    #[test]
    fn setup_wizard_selection_follows_session_enabled_changes() {
        crate::system::with_discovery_session(|host, publish, enabled| {
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    pkg.booleans |= crate::package::pkg::booleans::SYSTEM;
                    pkg.activities[0].main.component.intents =
                        vec![crate::package::intent_filter::ParsedIntentInfo {
                            filter: crate::package::intent_filter::IntentFilter {
                                actions: vec!["android.intent.action.MAIN".into()],
                                categories: Some(vec![
                                    "android.intent.category.SETUP_WIZARD".into(),
                                ]),
                                ..Default::default()
                            },
                            ..Default::default()
                        }];
                }
            });
            let settings = SettingsProvider::new(host.clone());
            let available = || {
                let (_, resolution) = host.packages().unwrap();
                let intent = crate::package::intent::Intent {
                    action: Some("android.intent.action.MAIN".into()),
                    categories: Some(vec!["android.intent.category.SETUP_WIZARD".into()]),
                    ..Default::default()
                };
                resolution
                    .query_intent_activities(
                        &intent,
                        None,
                        flags::MATCH_SYSTEM_ONLY
                            | flags::MATCH_DIRECT_BOOT_AWARE
                            | flags::MATCH_DIRECT_BOOT_UNAWARE,
                        0,
                        SYSTEM_UID,
                    )
                    .unwrap()
                    .len()
            };
            assert_eq!(available(), 1);
            assert_eq!(settings.setup_wizard().unwrap().as_deref(), Some("p"));
            for component in [Some("p.Main"), None] {
                enabled(component, 2);
                assert_eq!(available(), 0);
                assert_eq!(settings.setup_wizard().unwrap().as_deref(), Some("p"));
                enabled(component, 1);
                assert_eq!(available(), 1);
                assert_eq!(settings.setup_wizard().unwrap().as_deref(), Some("p"));
            }
        });
    }

    #[test]
    fn setup_wizard_selection_tracks_published_code() {
        crate::system::with_discovery_session(|host, publish, _| {
            let settings = SettingsProvider::new(host.clone());
            assert_eq!(settings.setup_wizard().unwrap(), None);
            publish(&mut |pkg| {
                if pkg.package_name == "p" {
                    pkg.booleans |= crate::package::pkg::booleans::SYSTEM;
                    let activity = &mut pkg.activities[0];
                    activity.main.component.intents =
                        vec![crate::package::intent_filter::ParsedIntentInfo {
                            filter: crate::package::intent_filter::IntentFilter {
                                actions: vec!["android.intent.action.MAIN".into()],
                                categories: Some(vec![
                                    "android.intent.category.SETUP_WIZARD".into(),
                                ]),
                                ..Default::default()
                            },
                            ..Default::default()
                        }];
                }
            });
            assert_eq!(settings.setup_wizard().unwrap().as_deref(), Some("p"));
            publish(&mut |pkg| pkg.activities.clear());
            assert_eq!(settings.setup_wizard().unwrap(), None);
        });
    }
    use super::*;

    /// `String.split("/")`, whose index 1 names a changed flag.
    #[test]
    fn names_split_as_java_splits_them() {
        assert_eq!(java_split("ns/flag", '/'), ["ns", "flag"]);
        assert_eq!(java_split("staged/ns*flag", '/'), ["staged", "ns*flag"]);
        assert_eq!(java_split("ns/", '/'), ["ns"]);
        assert_eq!(java_split("/", '/'), Vec::<&str>::new());
        assert_eq!(java_split("", '/'), [""]);
    }

    /// `getParcelable(CALL_METHOD_MONITOR_CALLBACK_KEY)`: a
    /// `RemoteCallback`'s binder; another parcelable is refused.
    #[test]
    fn monitor_callbacks_are_read_from_the_bundle() {
        // `putParcelable`: the class, then `RemoteCallback.writeToParcel`'s
        // binder, length-prefixed.
        let callback = |class: &str| {
            let mut p = Parcel::new();
            p.write_i32(0);
            p.write_i32(bundle::MAGIC);
            p.write_i32(1);
            p.write_string16(Some(MONITOR_CALLBACK));
            p.write_i32(bundle::VAL_PARCELABLE);
            let length_at = p.position();
            p.write_i32(0);
            p.write_string16(Some(class));
            p.write_binder(Some(Binder::Handle(7)));
            p.set_i32_at(length_at, (p.position() - length_at - 4) as i32);
            p.set_i32_at(0, (p.position() - 8) as i32);
            p.write_bool(false);
            p
        };
        let p = callback(REMOTE_CALLBACK);
        let parcel = Reader::new(p.data(), p.objects());
        let extras = bundle::read(&mut parcel.clone()).unwrap().unwrap();
        assert_eq!(
            remote_callback_of(&extras[MONITOR_CALLBACK], &parcel).unwrap(),
            Some(7)
        );
        let p = callback("android.os.Bundle");
        let parcel = Reader::new(p.data(), p.objects());
        let extras = bundle::read(&mut parcel.clone()).unwrap().unwrap();
        assert!(remote_callback_of(&extras[MONITOR_CALLBACK], &parcel).is_err());
        assert_eq!(remote_callback_of(&Value::Null, &parcel).unwrap(), None);
    }

    #[test]
    fn mime_types_compare_as_clip_description_compares_them() {
        assert!(compare_mime_types(
            "vnd.android.cursor.item/system",
            "vnd.android.cursor.item/*"
        ));
        assert!(compare_mime_types(
            "vnd.android.cursor.item/system",
            "vnd.android.cursor.item/system"
        ));
        assert!(!compare_mime_types(
            "vnd.android.cursor.item/system",
            "audio/*"
        ));
        assert!(!compare_mime_types("vnd.android.cursor.item/system", "*"));
    }

    #[test]
    fn an_open_failure_reads_as_errno_exception() {
        let e = std::io::Error::from_raw_os_error(libc::ENOENT);
        assert_eq!(
            open_failed(&e),
            "open failed: ENOENT (No such file or directory)"
        );
    }

    /// A tracked `GET_secure`'s reply, as the original's to Calculator:
    /// the bundle in `ArrayMap` order, the `MemoryIntArray` as a
    /// length-prefixed parcelable whose `ParcelFileDescriptor` is an fd
    /// object, then its index and generation.
    #[test]
    fn tracked_reply_carries_the_generation_array() {
        let registry = GenerationRegistry::new(4);
        let result = value_result(
            Some("1".into()),
            registry.generation_data(Table::Secure, 0, "a"),
        );
        let keys: Vec<_> = result.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "_track_generation",
                "value",
                "_generation_index",
                "_generation"
            ]
        );
        let mut p = Parcel::new();
        bundle::write(
            &mut p,
            &result
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect::<Vec<_>>(),
        );
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
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.util.MemoryIntArray")
        );
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.os.ParcelFileDescriptor")
        );
        assert_eq!(r.read_i32(), Ok(0));
        assert_eq!(p.objects(), [r.position() as u64]);
        assert_eq!(p.files().len(), 1);
        r.read_fd().unwrap();
        assert_eq!(r.position(), start + len);
        // An unset value is a null, not a null string.
        let mut p = Parcel::new();
        bundle::write(
            &mut p,
            &[("value", value_result(None, Vec::new())[0].1.clone())],
        );
        assert_eq!(
            &p.data()[p.data().len() - 8..p.data().len() - 4],
            (-1i32).to_le_bytes()
        );
    }

    #[test]
    fn restrictions_forbid_changes() {
        let set = |r: &str| r == "no_config_location" || r == "no_debugging_features";
        assert!(setting_restricted("location_mode", Some("0"), 10_050, &set));
        assert!(!setting_restricted(
            "location_mode",
            Some("3"),
            SYSTEM_UID,
            &|_| false
        ));
        assert!(setting_restricted("adb_enabled", Some("1"), 2000, &set));
        assert!(!setting_restricted("adb_enabled", Some("0"), 2000, &set));
        assert!(!setting_restricted(
            "screen_brightness",
            Some("5"),
            SYSTEM_UID,
            &|_| true
        ));
        assert!(setting_restricted(
            "screen_brightness",
            Some("5"),
            10_050,
            &|r| r == "no_config_brightness"
        ));
        assert!(setting_restricted(
            "data_roaming3",
            Some("1"),
            10_050,
            &|r| r == "no_data_roaming"
        ));
        assert!(!setting_restricted(
            "font_scale",
            Some("1"),
            10_050,
            &|_| true
        ));
    }

    /// `SettingsProviderTest.testWriteSystemSetting*` and CTS's
    /// `Settings_SystemTest`: apps write public settings, privileged apps
    /// any; others fail by their target SDK.
    #[test]
    fn apps_write_public_system_settings() {
        let insert = Op::Insert {
            value: Some("1"),
            tag: None,
            make_default: false,
            overrideable_by_restore: false,
            update: false,
        };
        let app = |privileged, sdk| move || Ok((privileged, sdk));
        assert!(enforce_system_mutation(10_050, &insert, "font_scale", &app(false, 34)).is_ok());
        let e =
            enforce_system_mutation(10_050, &insert, "pointer_speed", &app(false, 34)).unwrap_err();
        assert_eq!(e.message, "You cannot change private secure settings.");
        let e = enforce_system_mutation(10_050, &insert, "my_own", &app(false, 34)).unwrap_err();
        assert_eq!(
            e.message,
            "You cannot keep your settings in the secure settings."
        );
        assert!(enforce_system_mutation(10_050, &insert, "my_own", &app(false, 22)).is_ok());
        assert!(enforce_system_mutation(10_050, &insert, "my_own", &app(true, 34)).is_ok());
        assert!(enforce_system_mutation(SHELL_UID, &insert, "my_own", &app(false, 34)).is_ok());
        let e =
            enforce_system_mutation(10_050, &Op::Delete, "font_scale", &app(true, 34)).unwrap_err();
        assert_eq!(
            e.message,
            "You cannot delete system defined secure settings."
        );
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
        assert_eq!(
            reset_mode(0, true).unwrap_err().message,
            "Invalid reset mode: 0"
        );
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
        let declared =
            Declared::read(&aim_paths::original_image().join("system/framework/framework.jar"))
                .unwrap();
        let names = PUBLIC_SETTINGS.iter().chain(PRIVATE_SETTINGS).copied();
        for name in names.chain(crate::app_host::settings_validators::validated_names()) {
            assert!(declared.system.contains(name), "{name}");
        }
        assert!(declared.secure.contains("user_setup_complete"));
        assert!(declared.global.contains("device_provisioned"));
        // The moved settings are declared where they moved to.
        for name in SYSTEM_MOVED_TO_SECURE.iter().chain(GLOBAL_MOVED_TO_SECURE) {
            assert!(declared.secure.contains(*name), "{name}");
        }
        for name in SYSTEM_MOVED_TO_GLOBAL.iter().chain(SECURE_MOVED_TO_GLOBAL) {
            assert!(declared.global.contains(*name), "{name}");
        }
        for name in GLOBAL_MOVED_TO_SYSTEM {
            assert!(declared.system.contains(*name), "{name}");
        }
    }

    fn args(
        uri: &str,
        clause: Option<&str>,
        args: Option<&[&str]>,
        all: bool,
    ) -> Result<Arguments, String> {
        let args: Option<Vec<Option<String>>> =
            args.map(|a| a.iter().map(|s| Some(s.to_string())).collect());
        arguments(&Uri::parse(uri), clause, args.as_deref(), all).map_err(|e| match e {
            Thrown::Database(code, message) => format!("{code}: {message}"),
            Thrown::Binder(e) => format!("binder {}: {}", e.code, e.message),
            Thrown::Status(s) => format!("status {s}"),
        })
    }

    fn named(table: &str, name: Option<&str>) -> Result<Arguments, String> {
        Ok(Arguments {
            table: table.into(),
            name: name.map(str::to_string),
        })
    }

    /// `SettingsProviderTest`'s query forms and `Arguments`' patterns.
    #[test]
    fn arguments_read_as_the_provider_reads_them() {
        let uri = "content://settings/system";
        assert_eq!(
            args("content://settings/system/font_scale", None, None, false),
            named("system", Some("font_scale"))
        );
        assert_eq!(
            args(uri, Some("name=?"), Some(&["font_scale"]), false),
            named("system", Some("font_scale"))
        );
        assert_eq!(
            args(uri, Some(" ( name = ? ) "), Some(&["font_scale"]), false),
            named("system", Some("font_scale"))
        );
        assert_eq!(
            args(uri, Some("name='font_scale'"), None, false),
            named("system", Some("font_scale"))
        );
        assert_eq!(
            args(uri, Some("(name = \"font_scale\")"), None, false),
            named("system", Some("font_scale"))
        );
        assert_eq!(args(uri, None, None, true), named("system", None));
        // A moved setting is found where it moved to.
        assert_eq!(
            args("content://settings/system/adb_enabled", None, None, false),
            named("global", Some("adb_enabled"))
        );
        assert_eq!(
            args("content://settings/system/android_id", None, None, false),
            named("secure", Some("android_id"))
        );
        assert_eq!(
            args(
                "content://settings/global/apply_ramping_ringer",
                None,
                None,
                false
            ),
            named("system", Some("apply_ramping_ringer"))
        );
        let unsupported = "2: Supported SQL:\n  uri content://some_table/some_property with null where and where args\n  uri content://some_table with query name=? and single name as arg\n  uri content://some_table with query name=some_name and null args\n  but got - uri:content://settings/system, where:value=? whereArgs:[1, 2]";
        assert_eq!(
            args(uri, Some("value=?"), Some(&["1", "2"]), true),
            Err(unsupported.into())
        );
        assert!(
            args(uri, None, None, false)
                .unwrap_err()
                .ends_with("where:null whereArgs:null")
        );
        assert!(
            args(uri, Some("name=?"), Some(&["a", "b"]), true)
                .unwrap_err()
                .ends_with("whereArgs:[a, b]")
        );
        assert_eq!(
            args(uri, Some("name=?"), None, true),
            Err("binder -4: Attempt to get length of null array".into())
        );
        // `.` matches no line terminator.
        assert!(args(uri, Some("name='a\nb'"), None, true).is_err());
        assert_eq!(
            args(uri, Some("name='\"'"), None, true),
            named("system", Some(""))
        );
        assert_eq!(
            args(uri, Some("name='\""), None, true),
            Err("status -74".into())
        );
        assert_eq!(
            args("content://settings/nope/a", None, None, false),
            Err("2: Bad root path: nope".into())
        );
        assert_eq!(
            args("content://settings", None, None, true)
                .unwrap_err()
                .lines()
                .last(),
            Some("  but got - uri:content://settings, where:null whereArgs:null")
        );
    }

    #[test]
    fn types_are_of_the_table_a_setting_is_in() {
        let ty = |uri| get_type(&Uri::parse(uri)).map_err(|_| ());
        assert_eq!(
            ty("content://settings/secure"),
            Ok("vnd.android.cursor.dir/secure".into())
        );
        // Moved to secure, then from there to global.
        assert_eq!(
            ty("content://settings/system/wifi_on"),
            Ok("vnd.android.cursor.item/global".into())
        );
        assert_eq!(
            ty("content://settings/system/android_id"),
            Ok("vnd.android.cursor.item/secure".into())
        );
        assert_eq!(
            ty("content://settings/favorites"),
            Ok("vnd.android.cursor.dir/favorites".into())
        );
        assert!(ty("content://settings/system/a/b").is_err());
    }

    #[test]
    fn projections_rows_and_keys() {
        assert_eq!(
            normalize_projection(None).ok(),
            Some(ALL_COLUMNS.map(str::to_string).to_vec())
        );
        let projection = [Some("value".to_string()), Some("name".to_string())];
        let columns = normalize_projection(Some(&projection)).ok().unwrap();
        let setting = Setting {
            name: "a".into(),
            value: None,
            id: 7,
            preserved_in_restore: true,
            ..Default::default()
        };
        assert_eq!(setting_row(&setting, &columns), [None, Some("a".into())]);
        assert_eq!(
            setting_row(&setting, &ALL_COLUMNS.map(str::to_string)),
            [
                Some("7".into()),
                Some("a".into()),
                None,
                Some("true".into())
            ]
        );
        let Err(Thrown::Database(ILLEGAL_ARGUMENT, message)) = normalize_projection(Some(&[None]))
        else {
            panic!()
        };
        assert_eq!(message, "Invalid column: null");
        assert!(
            key_valid("font_scale")
                && !key_valid("")
                && !key_valid("a\u{1}")
                && !key_valid("\u{1f600}")
        );
    }

    /// `ContentValues.writeToParcel` of a name and values of the types
    /// `getAsString` reads.
    #[test]
    fn content_values() {
        let mut p = Parcel::new();
        p.write_i32(4);
        p.write_i32(4);
        for (key, kind) in [("name", 0), ("value", 1), ("flag", 9), ("ratio", 7)] {
            p.write_string16(Some(key));
            p.write_i32(kind);
            match kind {
                0 => p.write_string16(Some("font_scale")),
                1 => p.write_i32(2),
                9 => p.write_i32(1),
                _ => p.write_f32(1.5),
            }
        }
        let values = read_content_values(&mut Reader::new(p.data(), &[])).unwrap();
        assert_eq!(values["name"], Content::Text(Some("font_scale".into())));
        assert_eq!(values["value"], Content::Text(Some("2".into())));
        assert_eq!(values["flag"], Content::Text(Some("true".into())));
        let mut ratio = Parcel::new();
        ratio.write_i32(7);
        ratio.write_f32(1.5);
        assert_eq!(values["ratio"], Content::Original(ratio.data().to_vec()));
    }
}
