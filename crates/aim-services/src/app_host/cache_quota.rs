//! StorageStatsService's cache quotas for the app host
//! (`android-16.0.0_r1`): `mCacheQuotas`, which `getCacheQuotaBytes`
//! answers, filled as its handler `H` and `CacheQuotaStrategy` fill them.
//! The quotas are loaded from `/data/system/cachequota.xml`, or
//! recalculated when it cannot be read; then recalculated every 10 hours
//! and when the data volume's free space moved by more than its threshold
//! (checked every 30 seconds). A recalculation binds ExtServices'
//! `CacheQuotaService`, asks it for a hint for each app used in the last
//! year (with its usage stats) and keeps and writes the quotas it answers.
//! installd's `setAppQuota` only checks the volume: the quota it records
//! is read by its `freeCache` (#1819). The handler starts when the quotas
//! are first asked for; no package is removed in the app host
//! (`MSG_PACKAGE_REMOVED`; installs and removals: #1757).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aim_android_xml::{Element, Node, Value as XmlValue};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader};
use aim_service_aidl::WriteParcelable;

use super::services::HostConnection;
use super::settings::Table;
use super::usage_events::{INTERVAL_BEST, UsageStats};
use super::{AppHost, usage_database};
use crate::bundle::{VAL_LIST, VAL_PARCELABLE, Value};
use crate::package::component_resolver::Info;
use crate::package::info::flags::MATCH_FACTORY_ONLY;
use crate::package::intent::Intent;
use crate::package::query::Query;
use crate::package::uri::Uri;

/// `StorageStatsService.DEFAULT_QUOTA`.
const DEFAULT_QUOTA: i64 = 64 << 20;
/// `DELAY_CHECK_STORAGE_DELTA`; `DELAY_RECALCULATE_QUOTAS` in its checks.
const DELAY_CHECK_STORAGE_DELTA: Duration = Duration::from_secs(30);
const RECALCULATE_EVERY_CHECKS: u64 = 10 * 60 * 60 / 30;
/// `H.MINIMUM_CHANGE_DELTA_PERCENT_HIGH`, `_LOW`.
const MINIMUM_CHANGE_DELTA_PERCENT_HIGH: i64 = 5;
const MINIMUM_CHANGE_DELTA_PERCENT_LOW: i64 = 2;
/// `StorageManager.DEFAULT_STORAGE_THRESHOLD_PERCENT_HIGH` and its
/// DeviceConfig flag.
const DEFAULT_STORAGE_THRESHOLD_PERCENT_HIGH: i64 = 20;
const STORAGE_THRESHOLD_PERCENT_HIGH: &str = "storage_native_boot/storage_threshold_percent_high";
/// `Settings.Global.ENABLE_CACHE_QUOTA_CALCULATION`.
const ENABLE_CACHE_QUOTA_CALCULATION: &str = "enable_cache_quota_calculation";
/// `CacheQuotaHint.QUOTA_NOT_SET`, `CacheQuotaService.SERVICE_INTERFACE`
/// and `REQUEST_LIST_KEY`.
const QUOTA_NOT_SET: i64 = -1;
const SERVICE_INTERFACE: &str = "android.app.usage.CacheQuotaService";
const REQUEST_LIST_KEY: &str = "requests";
/// `ICacheQuotaService.computeCacheQuotaHints`, `IRemoteCallback.sendResult`.
const CACHE_QUOTA_DESCRIPTOR: &str = "android.app.usage.ICacheQuotaService";
const COMPUTE_CACHE_QUOTA_HINTS: u32 = 1;
const REMOTE_CALLBACK_DESCRIPTOR: &str = "android.os.IRemoteCallback";
const SEND_RESULT: u32 = 1;
/// `PackageManager.GET_SERVICES`, `GET_META_DATA`; `BIND_AUTO_CREATE`.
const GET_SERVICES: i64 = 0x4;
const GET_META_DATA: i64 = 0x80;
const BIND_AUTO_CREATE: i64 = 1;
/// `DateUtils.YEAR_IN_MILLIS`: 52 weeks.
const YEAR_IN_MILLIS: i64 = 52 * 7 * 24 * 60 * 60 * 1000;
const SYSTEM_UID: i32 = 1000;
/// `UserManager.getUsers`: the app host runs user 0.
const USERS: [i32; 1] = [0];

/// A `CacheQuotaHint`: the volume, the uid and its quota.
#[derive(Clone, Debug, PartialEq)]
struct Hint {
    uuid: Option<String>,
    uid: i32,
    quota: i64,
}

#[derive(Default)]
struct State {
    /// `mCacheQuotas`: each volume's quotas by uid.
    quotas: HashMap<Option<String>, HashMap<i32, i64>>,
    started: bool,
    /// `H.mPreviousBytes`.
    previous_bytes: i64,
}

/// `mCacheQuotas` and the handler that fills them.
pub struct CacheQuotas {
    host: Weak<AppHost>,
    state: Mutex<State>,
}

impl CacheQuotas {
    pub fn new(host: &Arc<AppHost>) -> Arc<CacheQuotas> {
        Arc::new(CacheQuotas {
            host: Arc::downgrade(host),
            state: Mutex::default(),
        })
    }

    /// `getCacheQuotaBytes`' answer: the quota of `uid` on the volume,
    /// the default before one is known. The handler is started first.
    pub fn quota(self: &Arc<Self>, uuid: Option<&str>, uid: i32) -> i64 {
        let mut state = self.state.lock().unwrap();
        if !std::mem::replace(&mut state.started, true) {
            let me = Arc::downgrade(self);
            std::thread::spawn(move || handle(me));
        }
        state
            .quotas
            .get(&uuid.map(str::to_owned))
            .and_then(|quotas| quotas.get(&uid))
            .copied()
            .unwrap_or(DEFAULT_QUOTA)
    }

    /// `MSG_LOAD_CACHED_QUOTAS_FROM_FILE`: the quotas of the file, else
    /// recalculated.
    fn load(self: &Arc<Self>, host: &Arc<AppHost>) {
        let previous = self.setup_quotas_from_file(host).unwrap_or_else(|e| {
            eprintln!("app host: storagestats: Cache quota XML file is malformed? {e}");
            -1
        });
        let previous = if previous < 0 {
            let available = available_bytes(host);
            self.recalculate(host);
            available
        } else {
            previous
        };
        self.state.lock().unwrap().previous_bytes = previous;
    }

    /// `MSG_CHECK_STORAGE_DELTA`: the quotas recalculated, and the free
    /// space asked again, when it moved by more than the threshold.
    fn check_storage_delta(self: &Arc<Self>, host: &Arc<AppHost>) {
        let (total, available) = match host.data_space() {
            Ok(space) => space,
            Err(e) => return eprintln!("app host: storagestats: {}", e.message),
        };
        let percent_high = host
            .inputs()
            .ok()
            .and_then(|i| {
                i.settings
                    .get(Table::Config, STORAGE_THRESHOLD_PERCENT_HIGH)
            })
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_STORAGE_THRESHOLD_PERCENT_HIGH);
        let threshold = if available > total * percent_high / 100 {
            total * MINIMUM_CHANGE_DELTA_PERCENT_HIGH / 100
        } else {
            total * MINIMUM_CHANGE_DELTA_PERCENT_LOW / 100
        };
        {
            let mut state = self.state.lock().unwrap();
            if (state.previous_bytes - available).abs() <= threshold {
                return;
            }
            state.previous_bytes = available;
        }
        self.recalculate(host);
        notify_significant_delta(host);
    }

    /// `setupQuotasFromFile`: the file's quotas pushed, and the free bytes
    /// they were calculated at; -1 without a file or quotas.
    fn setup_quotas_from_file(&self, host: &AppHost) -> Result<i64, String> {
        let path = host
            .data()
            .map_err(|e| e.message)?
            .join("system/cachequota.xml");
        let bytes = match usage_database::atomic_read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(-1),
            Err(e) => return Err(e.to_string()),
        };
        let Some((previous, hints)) = read_xml(&bytes)? else {
            eprintln!(
                "app host: storagestats: An error occurred while parsing the cache quota file."
            );
            return Ok(-1);
        };
        self.push_processed_quotas(&hints);
        Ok(previous)
    }

    /// `pushProcessedQuotas`: each hint's quota recorded by installd
    /// (`setAppQuota`, which takes no UUID or a valid one) and kept.
    fn push_processed_quotas(&self, hints: &[Hint]) {
        let mut state = self.state.lock().unwrap();
        for hint in hints.iter().filter(|h| h.quota != QUOTA_NOT_SET) {
            if hint.uuid.as_deref().is_some_and(|uuid| !valid_uuid(uuid)) {
                eprintln!(
                    "app host: storagestats: Failed to set cache quota for {}",
                    hint.uid
                );
                continue;
            }
            state
                .quotas
                .entry(hint.uuid.clone())
                .or_default()
                .insert(hint.uid, hint.quota);
        }
    }

    /// `CacheQuotaStrategy.recalculateQuotas`: the cache quota service
    /// bound, asked for the hints when it connects.
    fn recalculate(self: &Arc<Self>, host: &Arc<AppHost>) {
        let (package, class) = match service_component(host) {
            Ok(Some(component)) => component,
            Ok(None) => return eprintln!("app host: storagestats: No valid components found."),
            Err(e) => {
                return eprintln!(
                    "app host: storagestats: could not access the cache quota service: {e}"
                );
            }
        };
        let quotas = Arc::downgrade(self);
        let conn = Arc::new_cyclic(|me: &Weak<Connection>| Connection {
            quotas,
            me: me.clone(),
        });
        if let Err(e) =
            host.bind_service_from_host(None, &package, &class, 0, BIND_AUTO_CREATE, 0, conn)
        {
            eprintln!("app host: storagestats: bindServiceAsUser: {}", e.message);
        }
    }

    /// `onResult`: the processed hints pushed, the service unbound
    /// (`disconnectService`), and the hints written with the free bytes
    /// they were calculated at (`writeXmlToFile`).
    fn on_result(&self, host: &AppHost, conn: &Arc<dyn HostConnection>, hints: &[Hint]) {
        self.push_processed_quotas(hints);
        host.unbind_service_from_host(conn);
        let written = host.data().map_err(|e| e.message).and_then(|data| {
            let bytes = aim_android_xml::abx::write(&to_xml(hints, available_bytes(host)))?;
            usage_database::atomic_write(&data.join("system/cachequota.xml"), &bytes)
                .map_err(|e| e.to_string())
        });
        if let Err(e) = written {
            eprintln!(
                "app host: storagestats: An error occurred while writing the cache quota file: {e}"
            );
        }
    }
}

/// The handler: `MSG_LOAD_CACHED_QUOTAS_FROM_FILE`, then
/// `MSG_CHECK_STORAGE_DELTA` every 30 seconds and `MSG_RECALCULATE_QUOTAS`
/// every 10 hours; a message stops it while the calculation is turned off.
fn handle(quotas: Weak<CacheQuotas>) {
    let running = || {
        let quotas = quotas.upgrade()?;
        let host = quotas.host.upgrade()?;
        enabled(&host).then_some((quotas, host))
    };
    let Some((q, host)) = running() else {
        return;
    };
    q.load(&host);
    drop((q, host));
    for checks in 1.. {
        std::thread::sleep(DELAY_CHECK_STORAGE_DELTA);
        let Some((q, host)) = running() else {
            return;
        };
        q.check_storage_delta(&host);
        if checks % RECALCULATE_EVERY_CHECKS == 0 {
            q.recalculate(&host);
        }
    }
}

/// `isCacheQuotaCalculationsEnabled`.
fn enabled(host: &AppHost) -> bool {
    host.global_setting(ENABLE_CACHE_QUOTA_CALCULATION)
        .unwrap_or(1)
        != 0
}

/// The data volume's available bytes (`StatFs.getAvailableBytes`).
fn available_bytes(host: &AppHost) -> i64 {
    host.data_space().map_or(0, |(_, available)| available)
}

/// installd's `CHECK_ARGUMENT_UUID`: a UUID that is a file name.
fn valid_uuid(uuid: &str) -> bool {
    !uuid.is_empty() && uuid != "." && uuid != ".." && !uuid.contains('/')
}

/// `notifySignificantDelta`: the external storage provider's documents
/// changed, so their free space is asked again.
fn notify_significant_delta(host: &AppHost) {
    let Some(content) = host.content_service.get() else {
        return;
    };
    let uri = Uri::parse("content://com.android.externalstorage.documents/");
    if let Err(e) = content.notify_from_system(uri, 0, 0) {
        eprintln!("app host: storagestats: notifyChange: {}", e.message);
    }
}

/// `readFromXml`: the free bytes and the hints of a `cache-info` file,
/// none without an element; a quota without its uid or bytes is skipped
/// (`getRequestFromXml`).
fn read_xml(bytes: &[u8]) -> Result<Option<(i64, Vec<Hint>)>, String> {
    let Some(root) = aim_android_xml::read_next_optional(bytes)? else {
        return Ok(None);
    };
    if root.name != "cache-info" {
        return Err("Invalid starting tag.".into());
    }
    let previous = root
        .long("previousBytes")
        .ok()
        .flatten()
        .ok_or("Previous bytes formatted incorrectly; aborting quota read.")?;
    let mut hints = Vec::new();
    let mut pending: Vec<&Element> = root.children().collect();
    pending.reverse();
    while let Some(e) = pending.pop() {
        if e.name == "quota" {
            match (e.int("uid"), e.long("bytes")) {
                (Ok(Some(uid)), Ok(Some(quota))) => hints.push(Hint {
                    uuid: e.string("uuid").map(|s| s.into_owned()),
                    uid,
                    quota,
                }),
                _ => eprintln!("app host: storagestats: Invalid cache quota request, skipping."),
            }
        }
        pending.extend(e.children().collect::<Vec<_>>().into_iter().rev());
    }
    Ok(Some((previous, hints)))
}

/// `saveToXml`.
fn to_xml(hints: &[Hint], bytes_when_calculated: i64) -> Element {
    let quota = |hint: &Hint| {
        let mut attrs = Vec::new();
        if let Some(uuid) = &hint.uuid {
            attrs.push(("uuid".into(), XmlValue::String(uuid.clone())));
        }
        attrs.push(("uid".into(), XmlValue::Int(hint.uid)));
        attrs.push(("bytes".into(), XmlValue::Long(hint.quota)));
        Node::Element(Element {
            name: "quota".into(),
            attrs,
            content: Vec::new(),
        })
    };
    Element {
        name: "cache-info".into(),
        attrs: vec![(
            "previousBytes".into(),
            XmlValue::Long(bytes_when_calculated),
        )],
        content: hints.iter().map(quota).collect(),
    }
}

/// `getServiceComponentName`: the cache quota service of the services
/// extension package (`getServicesSystemSharedLibraryPackageName`:
/// `mServicesExtensionPackageName`: `config_servicesExtensionPackage`, a
/// system image package) as the system resolves it.
fn service_component(host: &AppHost) -> Result<Option<(String, String)>, String> {
    let (state, resolution) = host.live_packages().map_err(|e| e.message)?;
    let query = Query {
        state: &state,
        filter: &resolution.apps_filter,
        calling_uid: SYSTEM_UID,
    };
    let package = host
        .framework()
        .map_err(|e| e.message)?
        .framework_string("config_servicesExtensionPackage")
        .filter(|p| !p.is_empty())
        .ok_or("config_servicesExtensionPackage is empty")?;
    // `ensureSystemPackageName`: a factory package of user 0.
    query
        .package_info(&package, -1, MATCH_FACTORY_ONLY, 0)
        .map_err(|e| format!("getPackageInfo: {}", e.0))?
        .map_err(|e| e.message)?
        .ok_or(format!("{package} is not on the system image"))?;
    let intent = Intent {
        action: Some(SERVICE_INTERFACE.into()),
        package: Some(package),
        ..Default::default()
    };
    let resolved = resolution
        .resolve_service(&intent, None, GET_SERVICES | GET_META_DATA, 0, SYSTEM_UID)
        .map_err(|e| format!("resolveService: {e:?}"))?;
    let Some(Info::Service(si)) = resolved.map(|r| r.info) else {
        return Ok(None);
    };
    Ok(Some((
        si.info.item.package_name.clone().unwrap_or_default(),
        si.info.item.name.clone().unwrap_or_default(),
    )))
}

/// `getUnfulfilledRequests`: a hint without a quota for each app of each
/// user used in the last year, with its volume and uid; an app no longer
/// installed is left out.
fn unfulfilled_requests(host: &AppHost) -> Result<Vec<(Hint, UsageStats)>, String> {
    let now = super::usage_stats::now_millis();
    let mut requests = Vec::new();
    for user in USERS {
        let Some(stats) = host
            .usage_stats_in_range(INTERVAL_BEST, now - YEAR_IN_MILLIS, now)
            .map_err(|e| e.message)?
        else {
            continue;
        };
        let (state, resolution) = host.live_packages().map_err(|e| e.message)?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        for stat in stats {
            let info = query
                .application_info(&stat.package, 0, user)
                .map_err(|e| format!("getApplicationInfoAsUser: {}", e.0))?
                .map_err(|e| e.message)?;
            let Some(info) = info else {
                continue;
            };
            let hint = Hint {
                uuid: info.volume_uuid.clone(),
                uid: info.uid,
                quota: QUOTA_NOT_SET,
            };
            requests.push((hint, stat));
        }
    }
    Ok(requests)
}

/// The strategy's `ServiceConnection` to the cache quota service.
struct Connection {
    quotas: Weak<CacheQuotas>,
    me: Weak<Connection>,
}

impl HostConnection for Connection {
    fn connected(&self, service: Option<Binder>, _dead: bool) {
        // `onServiceDisconnected` forgets the service; `onServiceConnected`
        // asks it on a thread of its own.
        let (Some(Binder::Handle(handle)), Some(conn)) = (service, self.me.upgrade()) else {
            return;
        };
        let quotas = self.quotas.clone();
        std::thread::spawn(move || {
            let Some(q) = quotas.upgrade() else {
                return;
            };
            let Some(host) = q.host.upgrade() else {
                return;
            };
            let requests = match unfulfilled_requests(&host) {
                Ok(requests) => requests,
                Err(e) => return eprintln!("app host: storagestats: cache quota requests: {e}"),
            };
            let callback = host.process.add_service(Arc::new(ResultCallback {
                quotas: Arc::downgrade(&q),
                conn,
            }));
            let mut p = Parcel::new();
            p.write_interface_token(CACHE_QUOTA_DESCRIPTOR);
            // The `RemoteCallback` (its binder), then the typed list of
            // `CacheQuotaHint`s, each with its `UsageStats` parcelable.
            p.write_i32(1);
            p.write_binder(Some(callback));
            p.write_i32(requests.len() as i32);
            for (hint, stats) in &requests {
                p.write_i32(1);
                p.write_string16(hint.uuid.as_deref());
                p.write_i32(hint.uid);
                p.write_i64(hint.quota);
                p.write_string16(Some("android.app.usage.UsageStats"));
                stats.write_to(&mut p);
            }
            if let Err(e) = host
                .process
                .transact(handle, COMPUTE_CACHE_QUOTA_HINTS, &p, true)
            {
                eprintln!(
                    "app host: storagestats: Remote exception occurred while trying to get cache quota: {e}"
                );
            }
        });
    }
}

/// The strategy as the service's `RemoteCallback`.
struct ResultCallback {
    quotas: Weak<CacheQuotas>,
    conn: Arc<Connection>,
}

impl Service for ResultCallback {
    fn descriptor(&self) -> &str {
        REMOTE_CALLBACK_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != SEND_RESULT {
            return Err(BAD_VALUE);
        }
        call.data
            .enforce_interface(REMOTE_CALLBACK_DESCRIPTOR)
            .map_err(|_| BAD_VALUE)?;
        let hints = if call.data.read_i32()? == 0 {
            Vec::new()
        } else {
            read_hints(&mut call.data)?
        };
        if let Some(q) = self.quotas.upgrade()
            && let Some(host) = q.host.upgrade()
        {
            let conn: Arc<dyn HostConnection> = self.conn.clone();
            q.on_result(&host, &conn, &hints);
        }
        Ok(Parcel::new())
    }
}

/// The result bundle's `requests` (`getParcelableArrayList`): each
/// `CacheQuotaHint` of the list, its usage stats skipped.
fn read_hints(r: &mut Reader<'_>) -> Result<Vec<Hint>, i32> {
    let parcel = r.clone();
    let Some(entries) = crate::bundle::read(r)? else {
        return Ok(Vec::new());
    };
    let Some(&Value::Lazy {
        kind: VAL_LIST,
        start,
        len,
    }) = entries.get(REQUEST_LIST_KEY)
    else {
        return Ok(Vec::new());
    };
    let mut list = parcel;
    list.set_position(start);
    let mut hints = Vec::new();
    for _ in 0..list.read_i32()?.max(0) {
        // `writeValue` of a parcelable: its length, its creator's name and
        // its fields.
        if list.read_i32()? != VAL_PARCELABLE {
            return Err(BAD_VALUE);
        }
        let item_len = usize::try_from(list.read_i32()?).map_err(|_| BAD_VALUE)?;
        let item_end = list.position() + item_len;
        list.read_string16()?;
        hints.push(Hint {
            uuid: list.read_string16()?,
            uid: list.read_i32()?,
            quota: list.read_i64()?,
        });
        list.set_position(item_end);
    }
    if list.position() != start + len {
        return Err(BAD_VALUE);
    }
    Ok(hints)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hints of a result bundle, as `putParcelableArrayList` writes
    /// them.
    #[test]
    fn hints_are_read_from_the_result_bundle() {
        let mut item = Parcel::new();
        item.write_string16(Some("android.app.usage.CacheQuotaHint"));
        item.write_string16(None);
        item.write_i32(10_123);
        item.write_i64(5 << 20);
        item.write_string16(None);
        let mut list = Parcel::new();
        list.write_i32(1);
        list.write_i32(VAL_PARCELABLE);
        list.write_i32(item.data().len() as i32);
        list.write_raw(item.data(), &[]);
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_i32(crate::bundle::MAGIC);
        p.write_i32(1);
        p.write_string16(Some(REQUEST_LIST_KEY));
        p.write_i32(VAL_LIST);
        p.write_i32(list.data().len() as i32);
        p.write_raw(list.data(), &[]);
        let length = p.data().len() - 8;
        p.set_i32_at(0, length as i32);
        p.write_bool(false);
        let hints = read_hints(&mut p.reader()).unwrap();
        let expected = Hint {
            uuid: None,
            uid: 10_123,
            quota: 5 << 20,
        };
        assert_eq!(hints, [expected]);
    }

    /// `saveToXml`'s file read back as `readFromXml` reads it.
    #[test]
    fn the_quota_file_round_trips() {
        let hints = [
            Hint {
                uuid: None,
                uid: 10_123,
                quota: 5 << 20,
            },
            Hint {
                uuid: Some("1234-ABCD".into()),
                uid: 10_124,
                quota: QUOTA_NOT_SET,
            },
        ];
        let bytes = aim_android_xml::abx::write(&to_xml(&hints, 42)).unwrap();
        assert_eq!(read_xml(&bytes).unwrap(), Some((42, hints.to_vec())));
    }

    /// A file of the original: its free bytes and no quota.
    #[test]
    fn a_file_of_no_quotas_is_read() {
        let xml = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<cache-info previousBytes="7" />"#;
        assert_eq!(read_xml(xml).unwrap(), Some((7, Vec::new())));
        assert!(read_xml(b"<other />").is_err());
    }

    /// installd's check of a volume's UUID.
    #[test]
    fn uuids_are_file_names() {
        assert!(valid_uuid("1234-ABCD"));
        assert!(!valid_uuid("../x") && !valid_uuid("") && !valid_uuid(".."));
    }
}
