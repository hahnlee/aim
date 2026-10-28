//! `statsmanager` and `statscompanion`: StatsManagerService and
//! StatsCompanionService of the StatsD module for the app host
//! (`packages/modules/StatsD` at `android-16.0.0_r1`, ADR 0013), over the
//! original statsd, which init starts for it (`image/app-host/services`).
//!
//! As `StatsCompanion.Lifecycle` publishes them together, the two share
//! statsd: the companion fetches it (`stats`) when statsd says it is ready
//! or, at the app host's start, as `systemReady` does, cancels the alarms
//! of a previous statsd, hands it to the manager, says hi
//! (`statsCompanionReady`), tells it the `statsd_java` properties and its
//! boot completed, and on its death forgets it (and deletes a configuration
//! after ten deaths in a day, as the original does). The manager forwards
//! apps' calls with their permission checks (DUMP and PACKAGE_USAGE_STATS
//! with the GET_USAGE_STATS op, REGISTER_STATS_PULL_ATOM,
//! READ_RESTRICTED_STATS), waiting up to 5 s for statsd where the original
//! does and failing with IllegalStateException as it does when statsd is
//! not there; pullers and PendingIntent operations are kept and registered
//! again with each statsd (`sayHiToStatsd`), in the original's `ArrayMap`
//! order. statsd sends a PendingIntent through a `PendingIntentRef`, with
//! the original's extras. The companion's alarms are exact, on the elapsed
//! clock, and not wakeups, as the original sets them; the wake locks the
//! original holds while it calls statsd are not taken. The cold UID-map writer
//! and its receivers, live properties, deletion history/dump and system atom
//! registrations run in the system process: the pullers use the original
//! StatsPullAtomService from services.jar. The remaining Pinner local owner is #2274.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader, Result as ParcelResult,
    StatusCode,
};
use aim_service_aidl::android_os_ipendingintentref as pir;
use aim_service_aidl::android_os_istatscompanionservice as companion;
use aim_service_aidl::android_os_istatsd as statsd;
use aim_service_aidl::android_os_istatsmanagerservice as manager;
use aim_service_aidl::android_os_istatsquerycallback as query;
use aim_service_aidl::dev_aim_server_istatscompanionuidmap as uid_map;
use aim_service_aidl::{ReadParcelable, Returned, WriteParcelable};

use super::kept::RawIntent;
use super::{AppHost, reply};
use crate::bundle::Value;

/// `StatsCompanion.AID_STATSD`.
const AID_STATSD: i32 = 1066;
const STATSD_SERVICE: &str = "stats";
/// `StatsManagerService.STATSD_TIMEOUT_MILLIS`.
const STATSD_TIMEOUT: Duration = Duration::from_millis(5000);
/// `AppOpsManager.OP_GET_USAGE_STATS` (`USAGE_STATS_PERMISSION_OPS`) and
/// the modes it allows.
const OP_GET_USAGE_STATS: i32 = 43;
const USAGE_STATS_PERMISSION_OPS: &str = "android:get_usage_stats";
const DUMP: &str = "android.permission.DUMP";
const PACKAGE_USAGE_STATS: &str = "android.permission.PACKAGE_USAGE_STATS";
const REGISTER_STATS_PULL_ATOM: &str = "android.permission.REGISTER_STATS_PULL_ATOM";
const READ_RESTRICTED_STATS: &str = "android.permission.READ_RESTRICTED_STATS";
/// `StatsManager.ACTION_STATSD_STARTED`, `Intent.FLAG_RECEIVER_INCLUDE_BACKGROUND`.
const ACTION_STATSD_STARTED: &str = "android.app.action.STATSD_STARTED";
const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
/// `PendingIntentRef`'s extras and request codes (all 1).
const EXTRA_LAST_REPORT_TIME: &str = "android.app.extra.LAST_REPORT_TIME";
const EXTRA_STATS_CONFIG_UID: &str = "android.app.extra.STATS_CONFIG_UID";
const EXTRA_STATS_CONFIG_KEY: &str = "android.app.extra.STATS_CONFIG_KEY";
const EXTRA_STATS_SUBSCRIPTION_ID: &str = "android.app.extra.STATS_SUBSCRIPTION_ID";
const EXTRA_STATS_SUBSCRIPTION_RULE_ID: &str = "android.app.extra.STATS_SUBSCRIPTION_RULE_ID";
const EXTRA_STATS_BROADCAST_SUBSCRIBER_COOKIES: &str =
    "android.app.extra.STATS_BROADCAST_SUBSCRIBER_COOKIES";
const EXTRA_STATS_DIMENSIONS_VALUE: &str = "android.app.extra.STATS_DIMENSIONS_VALUE";
const EXTRA_STATS_ACTIVE_CONFIG_KEYS: &str = "android.app.extra.STATS_ACTIVE_CONFIG_KEYS";
const EXTRA_STATS_RESTRICTED_METRIC_IDS: &str = "android.app.extra.STATS_RESTRICTED_METRIC_IDS";
const PIR_CODE: i32 = 1;
/// `getDataFdFromStatsd`'s `CHUNK_SIZE`.
const CHUNK_SIZE: usize = 64 * 1024;

/// A key's Java `hashCode`, which orders an `ArrayMap`.
trait JavaHash {
    fn java_hash(&self) -> i32;
}

impl JavaHash for i32 {
    fn java_hash(&self) -> i32 {
        *self
    }
}

impl JavaHash for i64 {
    /// `Long.hashCode`.
    fn java_hash(&self) -> i32 {
        (*self ^ ((*self as u64) >> 32) as i64) as i32
    }
}

/// `String.hashCode`, over its UTF-16 units.
fn string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

/// `Objects.hash(a, b)`.
fn objects_hash(a: i32, b: i32) -> i32 {
    31i32.wrapping_add(a).wrapping_mul(31).wrapping_add(b)
}

/// `ArrayMap`: its entries ordered by their keys' hashes, a new key after
/// those of an equal hash, a key put again keeping its place.
struct ArrayMap<K, V>(Vec<(K, V)>);

impl<K, V> Default for ArrayMap<K, V> {
    fn default() -> Self {
        ArrayMap(Vec::new())
    }
}

impl<K: JavaHash + PartialEq, V> ArrayMap<K, V> {
    fn put(&mut self, key: K, value: V) {
        if let Some(entry) = self.0.iter_mut().find(|(k, _)| *k == key) {
            entry.1 = value;
            return;
        }
        let hash = key.java_hash();
        let at = self
            .0
            .iter()
            .position(|(k, _)| k.java_hash() > hash)
            .unwrap_or(self.0.len());
        self.0.insert(at, (key, value));
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let at = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(at).1)
    }

    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        self.0.iter()
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `ConfigKey`: a configuration of a uid.
#[derive(Clone, Copy, PartialEq)]
struct ConfigKey {
    uid: i32,
    config_id: i64,
}

impl JavaHash for ConfigKey {
    fn java_hash(&self) -> i32 {
        objects_hash(self.uid, self.config_id.java_hash())
    }
}

/// `ConfigKeyWithPackage`: a configuration of a package.
#[derive(Clone, PartialEq)]
struct ConfigKeyWithPackage {
    package: Option<String>,
    config_id: i64,
}

impl JavaHash for ConfigKeyWithPackage {
    fn java_hash(&self) -> i32 {
        objects_hash(
            self.package.as_deref().map_or(0, string_hash),
            self.config_id.java_hash(),
        )
    }
}

/// `PullerKey`: an atom a uid pulls.
#[derive(Clone, Copy, PartialEq)]
struct PullerKey {
    uid: i32,
    atom: i32,
}

impl JavaHash for PullerKey {
    fn java_hash(&self) -> i32 {
        objects_hash(self.uid, self.atom)
    }
}

/// `PullerValue`.
struct Puller {
    cool_down_millis: i64,
    timeout_millis: i64,
    additive_fields: Option<Vec<i32>>,
    callback: Option<Binder>,
}

/// What StatsManagerService keeps under `mLock`: statsd, and what it
/// registers again with each one.
#[derive(Default)]
struct Registered {
    statsd: Option<Strong>,
    pullers: ArrayMap<PullerKey, Puller>,
    data_fetch: ArrayMap<ConfigKey, Binder>,
    active_configs: ArrayMap<i32, Binder>,
    broadcast_subscribers: ArrayMap<ConfigKey, ArrayMap<i64, Binder>>,
    restricted_metrics: ArrayMap<ConfigKeyWithPackage, ArrayMap<i32, Binder>>,
}

/// One registration with statsd, as `sayHiToStatsd` makes them.
#[derive(Debug, PartialEq)]
enum Registration {
    Puller {
        uid: i32,
        atom: i32,
        cool_down_millis: i64,
        timeout_millis: i64,
        additive_fields: Option<Vec<i32>>,
        callback: Option<Binder>,
    },
    AllPullersFromBootRegistered,
    DataFetch {
        config_id: i64,
        pir: Binder,
        uid: i32,
    },
    ActiveConfigsChanged {
        pir: Binder,
        uid: i32,
    },
    BroadcastSubscriber {
        config_id: i64,
        subscriber_id: i64,
        pir: Binder,
        uid: i32,
    },
    RestrictedMetricsChanged {
        config_id: i64,
        package: Option<String>,
        pir: Binder,
        uid: i32,
    },
}

impl Registered {
    /// `registerAllPullers`, `registerAllDataFetchOperations`,
    /// `registerAllActiveConfigsChangedOperations`,
    /// `registerAllBroadcastSubscribers` and
    /// `registerAllRestrictedMetricsChangedOperations`, in their order.
    fn registrations(&self) -> Vec<Registration> {
        let mut all = Vec::new();
        for (k, v) in self.pullers.iter() {
            all.push(Registration::Puller {
                uid: k.uid,
                atom: k.atom,
                cool_down_millis: v.cool_down_millis,
                timeout_millis: v.timeout_millis,
                additive_fields: v.additive_fields.clone(),
                callback: v.callback,
            });
        }
        all.push(Registration::AllPullersFromBootRegistered);
        for (k, pir) in self.data_fetch.iter() {
            all.push(Registration::DataFetch {
                config_id: k.config_id,
                pir: *pir,
                uid: k.uid,
            });
        }
        for (uid, pir) in self.active_configs.iter() {
            all.push(Registration::ActiveConfigsChanged {
                pir: *pir,
                uid: *uid,
            });
        }
        for (k, subscribers) in self.broadcast_subscribers.iter() {
            for (subscriber_id, pir) in subscribers.iter() {
                all.push(Registration::BroadcastSubscriber {
                    config_id: k.config_id,
                    subscriber_id: *subscriber_id,
                    pir: *pir,
                    uid: k.uid,
                });
            }
        }
        for (k, uids) in self.restricted_metrics.iter() {
            for (uid, pir) in uids.iter() {
                all.push(Registration::RestrictedMetricsChanged {
                    config_id: k.config_id,
                    package: k.package.clone(),
                    pir: *pir,
                    uid: *uid,
                });
            }
        }
        all
    }
}

/// How a call to statsd failed: a `RemoteException` (the transaction), or
/// an exception statsd threw, which the original's callers do not catch.
enum Failure {
    Remote(String),
    Thrown(Exception),
}

impl From<StatusCode> for Failure {
    fn from(s: StatusCode) -> Self {
        Failure::Remote(format!("status {s}"))
    }
}

/// A call to statsd, its reply read with `read`.
fn two_way<T>(
    statsd: &Strong,
    code: u32,
    data: &Parcel,
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<Returned<T>>,
) -> Result<T, Failure> {
    let received = statsd.transact(code, data, false)?;
    read(&mut received.reader())?.map_err(Failure::Thrown)
}

/// A one-way call to statsd.
fn oneway(statsd: &Strong, code: u32, data: &Parcel) -> Result<(), Failure> {
    statsd
        .transact(code, data, true)
        .map(|_| ())
        .map_err(Failure::from)
}

/// A `PendingIntent` as it is parcelled: its `IIntentSender`.
struct PendingIntent(Option<Binder>);

impl ReadParcelable for PendingIntent {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(PendingIntent(r.read_binder()?))
    }
}

/// A `ParcelFileDescriptor` as it is parcelled: whether it has a comm fd,
/// its fd (the call's), and that one.
struct ReceivedFd(u32);

impl ReadParcelable for ReceivedFd {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let comm = r.read_i32()? != 0;
        let fd = r.read_fd()?;
        if comm {
            r.read_fd()?;
        }
        Ok(ReceivedFd(fd))
    }
}

/// A `ParcelFileDescriptor` of a file of this host, without a comm fd.
struct SentFd(aim_binder_driver::File);

impl WriteParcelable for SentFd {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
        p.write_file(self.0.clone());
    }
}

/// A `StatsDimensionsValueParcel` as statsd wrote it, which
/// `StatsDimensionsValue.writeToParcel` writes as it is.
struct DimensionsValue(Vec<u8>);

impl ReadParcelable for DimensionsValue {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let size = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
        if size < 4 {
            return Err(BAD_VALUE);
        }
        r.skip(size - 4)?;
        let (bytes, objects) = r.since(start);
        if !objects.is_empty() {
            return Err(BAD_VALUE);
        }
        Ok(DimensionsValue(bytes.to_vec()))
    }
}

/// `StatsCompanionService`'s `sStatsd`, linked to its death.
struct Linked {
    statsd: Strong,
    death: u64,
    dead: Arc<AtomicBool>,
    ready: bool,
}

/// What StatsCompanionService keeps under `sStatsdLock`.
#[derive(Default)]
struct Companion {
    statsd: Option<Linked>,
}

/// The companion's alarms (`mPullingAlarmListener`, `mPeriodicAlarmListener`):
/// when each goes off, on the elapsed clock.
#[derive(Default)]
struct Alarms {
    pull: Option<i64>,
    periodic: Option<i64>,
}

pub struct Stats {
    host: Arc<AppHost>,
    me: Weak<Stats>,
    /// StatsManagerService's `mLock`, and its waits for statsd.
    registered: Mutex<Registered>,
    statsd_ready: Condvar,
    companion: Mutex<Companion>,
    uid_map: Mutex<Option<Arc<Strong>>>,
    alarms: Mutex<Alarms>,
    alarms_changed: Condvar,
    boot_completed: AtomicBool,
    /// Each PendingIntent's `PendingIntentRef`, by its `IIntentSender`.
    pirs: Mutex<Vec<(Binder, Binder)>>,
}

impl Stats {
    pub fn new(host: Arc<AppHost>) -> Arc<Stats> {
        let stats = Arc::new_cyclic(|me| Stats {
            host,
            me: me.clone(),
            registered: Default::default(),
            statsd_ready: Condvar::new(),
            companion: Default::default(),
            uid_map: Default::default(),
            alarms: Default::default(),
            alarms_changed: Condvar::new(),
            boot_completed: AtomicBool::new(false),
            pirs: Default::default(),
        });
        let weak = Arc::downgrade(&stats);
        let _ = std::thread::Builder::new()
            .name("stats alarms".into())
            .spawn(move || run_alarms(weak));
        stats
    }

    /// `StatsCompanionService.systemReady`.
    pub(super) fn system_ready(&self) {
        if let Err(e) = self.say_hi_to_statsd() {
            eprintln!("app host: statscompanion: {}", e.message);
        }
    }

    /// `StatsCompanionService.bootCompleted`.
    pub(super) fn boot_completed(&self) {
        self.boot_completed.store(true, Ordering::SeqCst);
        let Some(statsd) = self.companion_statsd() else {
            return;
        };
        let mut p = Parcel::new();
        statsd::BootCompleted {}.write(&mut p);
        if oneway(&statsd, statsd::BOOT_COMPLETED, &p).is_err() {
            eprintln!("app host: statscompanion: Failed to notify statsd that boot completed");
        }
    }

    /// `StatsCompanionService.getStatsdNonblocking`.
    fn companion_statsd(&self) -> Option<Strong> {
        let companion = self.companion.lock().unwrap();
        companion
            .statsd
            .as_ref()
            .map(|l| self.host.process.strong(l.statsd.handle))
    }

    /// `StatsManagerService.getStatsdNonblocking`.
    fn statsd_nonblocking(&self) -> Option<Strong> {
        let registered = self.registered.lock().unwrap();
        registered
            .statsd
            .as_ref()
            .map(|s| self.host.process.strong(s.handle))
    }

    /// `StatsManagerService.waitForStatsd`.
    fn wait_for_statsd(&self) -> Option<Strong> {
        let mut registered = self.registered.lock().unwrap();
        if registered.statsd.is_none() {
            registered = self
                .statsd_ready
                .wait_timeout(registered, STATSD_TIMEOUT)
                .unwrap()
                .0;
        }
        registered
            .statsd
            .as_ref()
            .map(|s| self.host.process.strong(s.handle))
    }

    /// `StatsCompanionService.sayHiToStatsd`: an exception statsd threw
    /// is the caller's.
    fn say_hi_to_statsd(&self) -> Result<(), Exception> {
        let statsd = {
            let mut companion = self.companion.lock().unwrap();
            if companion
                .statsd
                .as_ref()
                .is_some_and(|l| !l.dead.load(Ordering::SeqCst))
            {
                eprintln!("app host: statscompanion: statsd has already been fetched before");
                return Ok(());
            }
            let Ok(statsd) = super::display::check_service(&self.host.process, STATSD_SERVICE)
            else {
                if let Some(old) = companion.statsd.take() {
                    self.host.process.clear_death(&old.statsd, old.death);
                }
                eprintln!(
                    "app host: statscompanion: Could not yet find statsd to tell it that StatsCompanion is alive."
                );
                return Ok(());
            };
            let dead = Arc::new(AtomicBool::new(false));
            let me = self.me.clone();
            let (handle, flag) = (statsd.handle, dead.clone());
            let death = self.host.process.link_to_death(
                &statsd,
                Box::new(move || {
                    flag.store(true, Ordering::SeqCst);
                    if let Some(stats) = me.upgrade() {
                        stats.statsd_died(handle);
                    }
                }),
            );
            let fetched = self.host.process.strong(statsd.handle);
            if let Some(old) = companion.statsd.replace(Linked {
                statsd,
                death,
                dead,
                ready: false,
            }) {
                self.host.process.clear_death(&old.statsd, old.death);
            }
            fetched
        };
        // A new statsd sets its own alarms.
        self.cancel_alarms(true, true);
        self.statsd_ready(self.host.process.strong(statsd.handle))?;
        let mut p = Parcel::new();
        statsd::StatsCompanionReady {}.write(&mut p);
        match two_way(
            &statsd,
            statsd::STATS_COMPANION_READY,
            &p,
            statsd::read_stats_companion_ready_reply,
        ) {
            Ok(()) => {}
            Err(Failure::Remote(e)) => {
                eprintln!(
                    "app host: statscompanion: Failed to inform statsd that statscompanion is ready: {e}"
                );
                return Ok(());
            }
            Err(Failure::Thrown(e)) => return Err(e),
        }
        {
            let mut companion = self.companion.lock().unwrap();
            if let Some(linked) = companion.statsd.as_mut()
                && linked.statsd.handle == statsd.handle
            {
                linked.ready = true;
            }
        }
        self.start_uid_map()?;
        if self.boot_completed.load(Ordering::SeqCst) {
            let mut p = Parcel::new();
            statsd::BootCompleted {}.write(&mut p);
            if oneway(&statsd, statsd::BOOT_COMPLETED, &p).is_err() {
                eprintln!(
                    "app host: statscompanion: Failed to inform statsd that statscompanion is ready"
                );
                return Ok(());
            }
        }
        eprintln!("app host: statscompanion: Told statsd that StatsCompanionService is alive.");
        Ok(())
    }

    /// The system process can attach after statsd's handshake, or restart
    /// while statsd stays alive. Each Java cohort owns its death/unregistration.
    pub(super) fn attach_uid_map(&self, controller: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = controller else {
            return Err(Exception::illegal_argument(
                "no statscompanion UID-map bridge",
            ));
        };
        let controller = Arc::new(self.host.process.strong(handle));
        let me = self.me.clone();
        self.host.process.link_to_death(
            &controller,
            Box::new(move || {
                if let Some(stats) = me.upgrade() {
                    let mut owner = stats.uid_map.lock().unwrap();
                    if owner.as_ref().is_some_and(|c| c.handle == handle) {
                        owner.take();
                    }
                }
            }),
        );
        *self.uid_map.lock().unwrap() = Some(controller);
        self.start_uid_map()
    }

    fn start_uid_map(&self) -> Result<(), Exception> {
        let controller = self.uid_map.lock().unwrap().clone();
        let statsd = self
            .companion
            .lock()
            .unwrap()
            .statsd
            .as_ref()
            .filter(|l| l.ready && !l.dead.load(Ordering::SeqCst))
            .map(|l| self.host.process.strong(l.statsd.handle));
        let (Some(controller), Some(statsd)) = (controller, statsd) else {
            return Ok(());
        };
        let mut p = Parcel::new();
        uid_map::Start {
            statsd: Some(Binder::Handle(statsd.handle)),
        }
        .write(&mut p);
        match two_way(&controller, uid_map::START, &p, uid_map::read_start_reply) {
            Ok(()) => Ok(()),
            Err(Failure::Thrown(e)) => Err(e),
            Err(Failure::Remote(e)) => Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("statscompanion UID-map bridge: {e}"),
            )),
        }
    }

    /// `StatsdDeathRecipient.binderDied`: forget this daemon; the retained
    /// Java recipient owns the configuration files and their deletion history.
    fn statsd_died(&self, handle: u32) {
        eprintln!(
            "app host: statscompanion: Statsd is dead - erase all my knowledge, except pullers"
        );
        let mut companion = self.companion.lock().unwrap();
        if companion
            .statsd
            .as_ref()
            .is_some_and(|l| l.statsd.handle == handle)
        {
            companion.statsd = None;
            self.registered.lock().unwrap().statsd = None;
        }
    }

    /// `StatsManagerService.statsdReady`: statsd kept, a waiter woken
    /// (`notify`), and what was registered registered with it.
    fn statsd_ready(&self, statsd: Strong) -> Result<(), Exception> {
        let registrations = {
            let mut registered = self.registered.lock().unwrap();
            let handle = statsd.handle;
            registered.statsd = Some(statsd);
            self.statsd_ready.notify_one();
            let all = registered.registrations();
            drop(registered);
            (self.host.process.strong(handle), all)
        };
        let (statsd, all) = registrations;
        for registration in all {
            if let Err(e) = register(&statsd, registration) {
                match e {
                    Failure::Remote(_) => {
                        eprintln!(
                            "app host: statsmanager: StatsManager failed to (re-)register data with statsd"
                        );
                        return Ok(());
                    }
                    Failure::Thrown(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    /// `StatsHelper.sendStatsdReadyBroadcast`.
    fn send_statsd_ready_broadcast(&self) {
        let intent = RawIntent::of(
            Some(ACTION_STATSD_STARTED),
            FLAG_RECEIVER_INCLUDE_BACKGROUND,
            &[],
        );
        self.host
            .broadcasts
            .send_from_system_with_permission(&self.host, intent, 0, DUMP);
    }

    /// `cancelPullingAlarm`, `cancelAlarmForSubscriberTriggering`.
    fn cancel_alarms(&self, pull: bool, periodic: bool) {
        let mut alarms = self.alarms.lock().unwrap();
        if pull {
            alarms.pull = None;
        }
        if periodic {
            alarms.periodic = None;
        }
        self.alarms_changed.notify_all();
    }

    /// `setPullingAlarm`, `setAlarmForSubscriberTriggering`.
    fn set_alarm(&self, pull: bool, at: i64) {
        let mut alarms = self.alarms.lock().unwrap();
        if pull {
            alarms.pull = Some(at);
        } else {
            alarms.periodic = Some(at);
        }
        self.alarms_changed.notify_all();
    }

    /// The `PendingIntentRef` of `pending_intent`, made once for each.
    fn pending_intent_ref(&self, pending_intent: Option<PendingIntent>) -> Binder {
        let target = pending_intent.and_then(|p| p.0);
        let mut pirs = self.pirs.lock().unwrap();
        if let Some(t) = target
            && let Some((_, pir)) = pirs.iter().find(|(known, _)| *known == t)
        {
            return *pir;
        }
        let kept = match target {
            Some(Binder::Handle(h)) => Some(self.host.process.strong(h)),
            _ => None,
        };
        let pir = self.host.process.add_service(Arc::new(PendingIntentRef {
            host: self.host.clone(),
            target,
            _kept: kept,
        }));
        if let Some(t) = target {
            pirs.push((t, pir));
        }
        pir
    }

    /// `enforceDumpAndUsageStatsPermission`; no caller is the app host
    /// itself (`Process.myPid()`).
    fn enforce_dump_and_usage_stats(
        &self,
        uid: i32,
        pid: i32,
        package: Option<&str>,
    ) -> Result<(), Exception> {
        for permission in [DUMP, PACKAGE_USAGE_STATS] {
            if self.host.check_permission(Some(permission), uid)? != 0 {
                return Err(Exception::security(format!(
                    "uid {uid} does not have {permission}."
                )));
            }
        }
        let Some(package) = package else {
            return Ok(());
        };
        match super::appops_service::note(&self.host, OP_GET_USAGE_STATS, uid, Some(package))? {
            super::appops_service::MODE_ALLOWED | super::appops_service::MODE_DEFAULT => Ok(()),
            // `noteOp` throws for an errored op.
            super::appops_service::MODE_ERRORED => Err(Exception::security(format!(
                "{package} from uid {uid} not allowed to perform GET_USAGE_STATS"
            ))),
            _ => Err(Exception::security(format!(
                "UID {uid} / PID {pid} lacks app-op {USAGE_STATS_PERMISSION_OPS}"
            ))),
        }
    }

    /// `enforceRegisterStatsPullAtomPermission`.
    fn enforce_register_pull_atom(&self, uid: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(REGISTER_STATS_PULL_ATOM), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "Need REGISTER_STATS_PULL_ATOM permission.: Neither user {uid} nor current process has {REGISTER_STATS_PULL_ATOM}."
            )));
        }
        Ok(())
    }

    /// `enforceRestrictedStatsPermission`.
    fn enforce_restricted_stats(&self, uid: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(READ_RESTRICTED_STATS), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "uid {uid} does not have {READ_RESTRICTED_STATS}."
            )));
        }
        Ok(())
    }

    /// A forwarded call the original makes only to a statsd it has
    /// (`getStatsdNonblocking`), logging a `RemoteException` as `failed`.
    fn if_ready<T>(
        &self,
        failed: &str,
        f: impl FnOnce(&Strong) -> Result<T, Failure>,
    ) -> Result<Option<T>, Exception> {
        let Some(statsd) = self.statsd_nonblocking() else {
            return Ok(None);
        };
        match f(&statsd) {
            Ok(v) => Ok(Some(v)),
            Err(Failure::Remote(_)) => {
                eprintln!("app host: statsmanager: {failed}");
                Ok(None)
            }
            Err(Failure::Thrown(e)) => Err(e),
        }
    }

    /// A forwarded call the original waits for statsd for
    /// (`waitForStatsd`), failing with IllegalStateException without one.
    fn waiting<T>(
        &self,
        what: &str,
        f: impl FnOnce(&Strong) -> Result<T, Failure>,
    ) -> Result<T, Exception> {
        let Some(statsd) = self.wait_for_statsd() else {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("Failed to connect to statsd to {what}"),
            ));
        };
        f(&statsd).map_err(|e| match e {
            Failure::Remote(message) => Exception::new(EX_ILLEGAL_STATE, message),
            Failure::Thrown(e) => e,
        })
    }

    /// `getDataFdFromStatsd` into the caller's `fd`: statsd writes the
    /// report's size, then the report, into a pipe, copied as it comes.
    fn get_data_fd(
        &self,
        statsd: &Strong,
        key: i64,
        uid: i32,
        fd: &mut impl Write,
    ) -> Result<(), Exception> {
        let state = |what: &str, e: std::io::Error| {
            Exception::new(EX_ILLEGAL_STATE, format!("{what}: {e}"))
        };
        let (read_end, write_end) =
            std::io::pipe().map_err(|e| state("Failed to create a pipe to receive reports.", e))?;
        let file = aim_binder_host::server::file_from_fd(write_end.as_fd()).ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "Failed to create a pipe to receive reports.",
            )
        })?;
        let mut p = Parcel::new();
        statsd::GetDataFd {
            key,
            calling_uid: uid,
            fd: Some(SentFd(file)),
        }
        .write(&mut p);
        oneway(statsd, statsd::GET_DATA_FD, &p).map_err(|e| match e {
            Failure::Remote(message) => Exception::new(EX_ILLEGAL_STATE, message),
            Failure::Thrown(e) => e,
        })?;
        drop(write_end);
        copy_report(read_end, fd).map_err(|e| state("Failed to read data from statsd pipe.", e))
    }
}

/// `getDataFdFromStatsd`'s copy: the big-endian size, then the report in
/// chunks, which must come to that size.
fn copy_report(mut from: impl Read, to: &mut impl Write) -> std::io::Result<()> {
    let mut size = [0u8; 4];
    from.read_exact(&mut size)?;
    to.write_all(&size)?;
    let expected = i32::from_be_bytes(size);
    let mut chunk = vec![0u8; CHUNK_SIZE];
    let mut read = 0i64;
    loop {
        let n = from.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        to.write_all(&chunk[..n])?;
        read += n as i64;
    }
    if read != i64::from(expected) {
        return Err(std::io::Error::other("Incomplete data read from StatsD."));
    }
    Ok(())
}

/// One registration's call to statsd.
fn register(statsd: &Strong, registration: Registration) -> Result<(), Failure> {
    let mut p = Parcel::new();
    match registration {
        Registration::Puller {
            uid,
            atom,
            cool_down_millis,
            timeout_millis,
            additive_fields,
            callback,
        } => {
            statsd::RegisterPullAtomCallback {
                uid,
                atom_tag: atom,
                cool_down_millis,
                timeout_millis,
                additive_fields,
                puller_callback: callback,
            }
            .write(&mut p);
            oneway(statsd, statsd::REGISTER_PULL_ATOM_CALLBACK, &p)
        }
        Registration::AllPullersFromBootRegistered => {
            statsd::AllPullersFromBootRegistered {}.write(&mut p);
            oneway(statsd, statsd::ALL_PULLERS_FROM_BOOT_REGISTERED, &p)
        }
        Registration::DataFetch {
            config_id,
            pir,
            uid,
        } => {
            statsd::SetDataFetchOperation {
                config_id,
                pending_intent_ref: Some(pir),
                calling_uid: uid,
            }
            .write(&mut p);
            two_way(
                statsd,
                statsd::SET_DATA_FETCH_OPERATION,
                &p,
                statsd::read_set_data_fetch_operation_reply,
            )
        }
        Registration::ActiveConfigsChanged { pir, uid } => {
            statsd::SetActiveConfigsChangedOperation {
                pending_intent_ref: Some(pir),
                calling_uid: uid,
            }
            .write(&mut p);
            two_way(
                statsd,
                statsd::SET_ACTIVE_CONFIGS_CHANGED_OPERATION,
                &p,
                statsd::read_set_active_configs_changed_operation_reply,
            )
            .map(|_| ())
        }
        Registration::BroadcastSubscriber {
            config_id,
            subscriber_id,
            pir,
            uid,
        } => {
            statsd::SetBroadcastSubscriber {
                config_id,
                subscriber_id,
                pir: Some(pir),
                calling_uid: uid,
            }
            .write(&mut p);
            two_way(
                statsd,
                statsd::SET_BROADCAST_SUBSCRIBER,
                &p,
                statsd::read_set_broadcast_subscriber_reply,
            )
        }
        Registration::RestrictedMetricsChanged {
            config_id,
            package,
            pir,
            uid,
        } => {
            statsd::SetRestrictedMetricsChangedOperation {
                config_key: config_id,
                config_package: package,
                pir: Some(pir),
                calling_uid: uid,
            }
            .write(&mut p);
            two_way(
                statsd,
                statsd::SET_RESTRICTED_METRICS_CHANGED_OPERATION,
                &p,
                statsd::read_set_restricted_metrics_changed_operation_reply,
            )
            .map(|_| ())
        }
    }
}

/// The companion's alarms: each, when it goes off, tells statsd
/// (`informPollAlarmFired`, `informAlarmForSubscriberTriggeringFired`) on
/// a thread of its own, as the original's `WakelockThread` does.
fn run_alarms(stats: Weak<Stats>) {
    loop {
        let Some(s) = stats.upgrade() else { return };
        let mut alarms = s.alarms.lock().unwrap();
        let now = super::alarm_service::elapsed_ms();
        let pull = alarms
            .pull
            .filter(|&at| at <= now)
            .inspect(|_| alarms.pull = None);
        let periodic = alarms
            .periodic
            .filter(|&at| at <= now)
            .inspect(|_| alarms.periodic = None);
        if pull.is_none() && periodic.is_none() {
            let next = [alarms.pull, alarms.periodic].into_iter().flatten().min();
            let wait = next.map_or(Duration::from_secs(60), |at| {
                Duration::from_millis((at - now).max(0) as u64)
            });
            drop(s.alarms_changed.wait_timeout(alarms, wait).unwrap());
            continue;
        }
        drop(alarms);
        for (fired, what) in [
            (pull.is_some(), "pulling"),
            (periodic.is_some(), "periodic"),
        ] {
            if !fired {
                continue;
            }
            let Some(statsd) = s.companion_statsd() else {
                eprintln!(
                    "app host: statscompanion: Could not access statsd to inform it of {what} alarm firing."
                );
                continue;
            };
            let _ = std::thread::Builder::new().name("stats alarm".into()).spawn(move || {
                let mut p = Parcel::new();
                let told = if what == "pulling" {
                    statsd::InformPollAlarmFired {}.write(&mut p);
                    two_way(&statsd, statsd::INFORM_POLL_ALARM_FIRED, &p, statsd::read_inform_poll_alarm_fired_reply)
                } else {
                    statsd::InformAlarmForSubscriberTriggeringFired {}.write(&mut p);
                    two_way(
                        &statsd,
                        statsd::INFORM_ALARM_FOR_SUBSCRIBER_TRIGGERING_FIRED,
                        &p,
                        statsd::read_inform_alarm_for_subscriber_triggering_fired_reply,
                    )
                };
                if let Err(Failure::Remote(e) | Failure::Thrown(Exception { message: e, .. })) = told {
                    eprintln!("app host: statscompanion: Failed to inform statsd of {what} alarm firing: {e}");
                }
            });
        }
    }
}

/// `StatsCompanion.enforceStatsdCallingUid`: only statsd.
fn enforce_statsd_calling_uid(uid: i32) -> Result<(), Exception> {
    if uid != AID_STATSD {
        return Err(Exception::security("Not allowed to access StatsCompanion"));
    }
    Ok(())
}

/// `statsmanager`.
pub struct StatsManagerService(pub Arc<Stats>);

impl Service for StatsManagerService {
    fn descriptor(&self) -> &str {
        manager::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let s = &self.0;
        let (uid, pid) = (call.sender_euid as i32, call.sender_pid);
        let mut p = Parcel::new();
        let result: Result<(), Exception> = match call.code {
            manager::SET_DATA_FETCH_OPERATION => {
                let a = manager::SetDataFetchOperation::<PendingIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let pir = s.pending_intent_ref(a.pending_intent);
                    let key = ConfigKey {
                        uid,
                        config_id: a.config_id,
                    };
                    s.registered.lock().unwrap().data_fetch.put(key, pir);
                    s.if_ready("Failed to setDataFetchOperation with statsd", |statsd| {
                        register(
                            statsd,
                            Registration::DataFetch {
                                config_id: a.config_id,
                                pir,
                                uid,
                            },
                        )
                    })?;
                    manager::write_set_data_fetch_operation_reply(&mut p);
                    Ok(())
                })()
            }
            manager::REMOVE_DATA_FETCH_OPERATION => {
                let a = manager::RemoveDataFetchOperation::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    s.registered.lock().unwrap().data_fetch.remove(&ConfigKey {
                        uid,
                        config_id: a.config_id,
                    });
                    s.if_ready("Failed to removeDataFetchOperation with statsd", |statsd| {
                        let mut d = Parcel::new();
                        statsd::RemoveDataFetchOperation {
                            config_id: a.config_id,
                            calling_uid: uid,
                        }
                        .write(&mut d);
                        two_way(
                            statsd,
                            statsd::REMOVE_DATA_FETCH_OPERATION,
                            &d,
                            statsd::read_remove_data_fetch_operation_reply,
                        )
                    })?;
                    manager::write_remove_data_fetch_operation_reply(&mut p);
                    Ok(())
                })()
            }
            manager::SET_ACTIVE_CONFIGS_CHANGED_OPERATION => {
                let a = manager::SetActiveConfigsChangedOperation::<PendingIntent>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let pir = s.pending_intent_ref(a.pending_intent);
                    s.registered.lock().unwrap().active_configs.put(uid, pir);
                    let ids = s.if_ready(
                        "Failed to setActiveConfigsChangedOperation with statsd",
                        |statsd| {
                            let mut d = Parcel::new();
                            statsd::SetActiveConfigsChangedOperation {
                                pending_intent_ref: Some(pir),
                                calling_uid: uid,
                            }
                            .write(&mut d);
                            two_way(
                                statsd,
                                statsd::SET_ACTIVE_CONFIGS_CHANGED_OPERATION,
                                &d,
                                statsd::read_set_active_configs_changed_operation_reply,
                            )
                        },
                    )?;
                    manager::write_set_active_configs_changed_operation_reply(
                        &mut p,
                        &Some(ids.flatten().unwrap_or_default()),
                    );
                    Ok(())
                })()
            }
            manager::REMOVE_ACTIVE_CONFIGS_CHANGED_OPERATION => {
                let a = manager::RemoveActiveConfigsChangedOperation::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    s.registered.lock().unwrap().active_configs.remove(&uid);
                    s.if_ready(
                        "Failed to removeActiveConfigsChangedOperation with statsd",
                        |statsd| {
                            let mut d = Parcel::new();
                            statsd::RemoveActiveConfigsChangedOperation { calling_uid: uid }
                                .write(&mut d);
                            two_way(
                                statsd,
                                statsd::REMOVE_ACTIVE_CONFIGS_CHANGED_OPERATION,
                                &d,
                                statsd::read_remove_active_configs_changed_operation_reply,
                            )
                        },
                    )?;
                    manager::write_remove_active_configs_changed_operation_reply(&mut p);
                    Ok(())
                })()
            }
            manager::SET_BROADCAST_SUBSCRIBER => {
                let a = manager::SetBroadcastSubscriber::<PendingIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let pir = s.pending_intent_ref(a.pending_intent);
                    let key = ConfigKey {
                        uid,
                        config_id: a.config_key,
                    };
                    {
                        let mut registered = s.registered.lock().unwrap();
                        let mut inner = registered
                            .broadcast_subscribers
                            .remove(&key)
                            .unwrap_or_default();
                        inner.put(a.subscriber_id, pir);
                        registered.broadcast_subscribers.put(key, inner);
                    }
                    s.if_ready("Failed to setBroadcastSubscriber with statsd", |statsd| {
                        register(
                            statsd,
                            Registration::BroadcastSubscriber {
                                config_id: a.config_key,
                                subscriber_id: a.subscriber_id,
                                pir,
                                uid,
                            },
                        )
                    })?;
                    manager::write_set_broadcast_subscriber_reply(&mut p);
                    Ok(())
                })()
            }
            manager::UNSET_BROADCAST_SUBSCRIBER => {
                let a = manager::UnsetBroadcastSubscriber::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let key = ConfigKey {
                        uid,
                        config_id: a.config_key,
                    };
                    {
                        let mut registered = s.registered.lock().unwrap();
                        if let Some(inner) = registered.broadcast_subscribers.get_mut(&key) {
                            inner.remove(&a.subscriber_id);
                            if inner.is_empty() {
                                registered.broadcast_subscribers.remove(&key);
                            }
                        }
                    }
                    s.if_ready("Failed to unsetBroadcastSubscriber with statsd", |statsd| {
                        let mut d = Parcel::new();
                        statsd::UnsetBroadcastSubscriber {
                            config_id: a.config_key,
                            subscriber_id: a.subscriber_id,
                            calling_uid: uid,
                        }
                        .write(&mut d);
                        two_way(
                            statsd,
                            statsd::UNSET_BROADCAST_SUBSCRIBER,
                            &d,
                            statsd::read_unset_broadcast_subscriber_reply,
                        )
                    })?;
                    manager::write_unset_broadcast_subscriber_reply(&mut p);
                    Ok(())
                })()
            }
            manager::GET_REGISTERED_EXPERIMENT_IDS => {
                manager::GetRegisteredExperimentIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, None)?;
                    let ids = s.waiting("registerExperimentIds", |statsd| {
                        let mut d = Parcel::new();
                        statsd::GetRegisteredExperimentIds {}.write(&mut d);
                        two_way(
                            statsd,
                            statsd::GET_REGISTERED_EXPERIMENT_IDS,
                            &d,
                            statsd::read_get_registered_experiment_ids_reply,
                        )
                    })?;
                    manager::write_get_registered_experiment_ids_reply(&mut p, &ids);
                    Ok(())
                })()
            }
            manager::GET_METADATA => {
                let a = manager::GetMetadata::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let metadata = s.waiting("getMetadata", |statsd| {
                        let mut d = Parcel::new();
                        statsd::GetMetadata {}.write(&mut d);
                        two_way(
                            statsd,
                            statsd::GET_METADATA,
                            &d,
                            statsd::read_get_metadata_reply,
                        )
                    })?;
                    manager::write_get_metadata_reply(&mut p, &metadata);
                    Ok(())
                })()
            }
            manager::GET_DATA => {
                let a = manager::GetData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let data = s.waiting("getData", |statsd| {
                        let mut d = Parcel::new();
                        statsd::GetData {
                            key: a.key,
                            calling_uid: uid,
                        }
                        .write(&mut d);
                        two_way(statsd, statsd::GET_DATA, &d, statsd::read_get_data_reply)
                    })?;
                    manager::write_get_data_reply(&mut p, &data);
                    Ok(())
                })()
            }
            manager::GET_DATA_FD => {
                let a = manager::GetDataFd::<ReceivedFd>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let file = a.fd.as_ref().and_then(|fd| s.host.process.file(fd.0));
                let result = (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    let mut out = file
                        .as_ref()
                        .and_then(aim_binder_host::server::file_fd)
                        .ok_or_else(|| {
                            Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "writeFd")
                        })?;
                    s.waiting("getDataFd", |statsd| {
                        Ok(s.get_data_fd(statsd, a.key, uid, &mut out))
                    })?
                })();
                // One-way: what it throws reaches no one.
                if let Err(e) = result {
                    eprintln!("app host: statsmanager: getDataFd: {}", e.message);
                }
                return Ok(Parcel::new());
            }
            manager::ADD_CONFIGURATION => {
                let a = manager::AddConfiguration::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    s.waiting("addConfig", |statsd| {
                        let mut d = Parcel::new();
                        statsd::AddConfiguration {
                            config_id: a.config_id,
                            config: a.config.clone(),
                            calling_uid: uid,
                        }
                        .write(&mut d);
                        two_way(
                            statsd,
                            statsd::ADD_CONFIGURATION,
                            &d,
                            statsd::read_add_configuration_reply,
                        )
                    })?;
                    manager::write_add_configuration_reply(&mut p);
                    Ok(())
                })()
            }
            manager::REMOVE_CONFIGURATION => {
                let a =
                    manager::RemoveConfiguration::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_dump_and_usage_stats(uid, pid, a.package_name.as_deref())?;
                    s.waiting("removeConfig", |statsd| {
                        let mut d = Parcel::new();
                        statsd::RemoveConfiguration {
                            config_id: a.config_id,
                            calling_uid: uid,
                        }
                        .write(&mut d);
                        two_way(
                            statsd,
                            statsd::REMOVE_CONFIGURATION,
                            &d,
                            statsd::read_remove_configuration_reply,
                        )
                    })?;
                    manager::write_remove_configuration_reply(&mut p);
                    Ok(())
                })()
            }
            manager::REGISTER_PULL_ATOM_CALLBACK => {
                let a = manager::RegisterPullAtomCallback::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let result = (|| -> Result<(), Exception> {
                    s.enforce_register_pull_atom(uid)?;
                    let Some(callback) = a.puller_callback else {
                        eprintln!(
                            "app host: statsmanager: Puller callback is null for atom {}",
                            a.atom_tag
                        );
                        return Ok(());
                    };
                    let key = PullerKey {
                        uid,
                        atom: a.atom_tag,
                    };
                    let puller = Puller {
                        cool_down_millis: a.cool_down_millis,
                        timeout_millis: a.timeout_millis,
                        additive_fields: a.additive_fields,
                        callback: Some(callback),
                    };
                    let registration = Registration::Puller {
                        uid,
                        atom: a.atom_tag,
                        cool_down_millis: puller.cool_down_millis,
                        timeout_millis: puller.timeout_millis,
                        additive_fields: puller.additive_fields.clone(),
                        callback: Some(callback),
                    };
                    s.registered.lock().unwrap().pullers.put(key, puller);
                    s.if_ready(
                        &format!(
                            "Failed to access statsd to register puller for atom {}",
                            a.atom_tag
                        ),
                        |statsd| register(statsd, registration),
                    )?;
                    Ok(())
                })();
                if let Err(e) = result {
                    eprintln!(
                        "app host: statsmanager: registerPullAtomCallback: {}",
                        e.message
                    );
                }
                return Ok(Parcel::new());
            }
            manager::UNREGISTER_PULL_ATOM_CALLBACK => {
                let a = manager::UnregisterPullAtomCallback::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let result = (|| -> Result<(), Exception> {
                    s.enforce_register_pull_atom(uid)?;
                    s.registered.lock().unwrap().pullers.remove(&PullerKey {
                        uid,
                        atom: a.atom_tag,
                    });
                    s.if_ready(
                        &format!(
                            "Failed to access statsd to unregister puller for atom {}",
                            a.atom_tag
                        ),
                        |statsd| {
                            let mut d = Parcel::new();
                            statsd::UnregisterPullAtomCallback {
                                uid,
                                atom_tag: a.atom_tag,
                            }
                            .write(&mut d);
                            oneway(statsd, statsd::UNREGISTER_PULL_ATOM_CALLBACK, &d)
                        },
                    )?;
                    Ok(())
                })();
                if let Err(e) = result {
                    eprintln!(
                        "app host: statsmanager: unregisterPullAtomCallback: {}",
                        e.message
                    );
                }
                return Ok(Parcel::new());
            }
            manager::QUERY_SQL => {
                let a = manager::QuerySql::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let result = (|| {
                    s.enforce_restricted_stats(uid)?;
                    let sent = match s.wait_for_statsd() {
                        Some(statsd) => {
                            let mut d = Parcel::new();
                            statsd::QuerySql {
                                sql_query: a.sql_query.clone(),
                                min_sql_client_version: a.min_sql_client_version,
                                policy_config: a.policy_config.clone(),
                                query_callback: a.query_callback,
                                config_key: a.config_key,
                                config_package: a.config_package.clone(),
                                calling_uid: uid,
                            }
                            .write(&mut d);
                            oneway(&statsd, statsd::QUERY_SQL, &d)
                        }
                        // `queryCallback.sendFailure`.
                        None => {
                            let Some(Binder::Handle(h)) = a.query_callback else {
                                return Err(Exception::new(
                                    aim_binder_host::parcel::EX_NULL_POINTER,
                                    "queryCallback",
                                ));
                            };
                            let mut d = Parcel::new();
                            query::SendFailure {
                                error: Some(
                                    "Could not connect to statsd from system server".into(),
                                ),
                            }
                            .write(&mut d);
                            oneway(&s.host.process.strong(h), query::SEND_FAILURE, &d)
                        }
                    };
                    sent.map_err(|e| match e {
                        Failure::Remote(message) => Exception::new(EX_ILLEGAL_STATE, message),
                        Failure::Thrown(e) => e,
                    })
                })();
                if let Err(e) = result {
                    eprintln!("app host: statsmanager: querySql: {}", e.message);
                }
                return Ok(Parcel::new());
            }
            manager::SET_RESTRICTED_METRICS_CHANGED_OPERATION => {
                let a = manager::SetRestrictedMetricsChangedOperation::<PendingIntent>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_restricted_stats(uid)?;
                    let pir = s.pending_intent_ref(a.pending_intent);
                    let key = ConfigKeyWithPackage {
                        package: a.config_package.clone(),
                        config_id: a.config_key,
                    };
                    {
                        let mut registered = s.registered.lock().unwrap();
                        let mut inner = registered
                            .restricted_metrics
                            .remove(&key)
                            .unwrap_or_default();
                        inner.put(uid, pir);
                        registered.restricted_metrics.put(key, inner);
                    }
                    let ids = s.if_ready(
                        "Failed to setRestrictedMetricsChangedOperation with statsd",
                        |statsd| {
                            let mut d = Parcel::new();
                            statsd::SetRestrictedMetricsChangedOperation {
                                config_key: a.config_key,
                                config_package: a.config_package.clone(),
                                pir: Some(pir),
                                calling_uid: uid,
                            }
                            .write(&mut d);
                            two_way(
                                statsd,
                                statsd::SET_RESTRICTED_METRICS_CHANGED_OPERATION,
                                &d,
                                statsd::read_set_restricted_metrics_changed_operation_reply,
                            )
                        },
                    )?;
                    manager::write_set_restricted_metrics_changed_operation_reply(
                        &mut p,
                        &Some(ids.flatten().unwrap_or_default()),
                    );
                    Ok(())
                })()
            }
            manager::REMOVE_RESTRICTED_METRICS_CHANGED_OPERATION => {
                let a = manager::RemoveRestrictedMetricsChangedOperation::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                (|| {
                    s.enforce_restricted_stats(uid)?;
                    let key = ConfigKeyWithPackage {
                        package: a.config_package.clone(),
                        config_id: a.config_key,
                    };
                    {
                        let mut registered = s.registered.lock().unwrap();
                        if let Some(inner) = registered.restricted_metrics.get_mut(&key) {
                            inner.remove(&uid);
                            if inner.is_empty() {
                                registered.restricted_metrics.remove(&key);
                            }
                        }
                    }
                    s.if_ready(
                        "Failed to removeRestrictedMetricsChangedOperation with statsd",
                        |statsd| {
                            let mut d = Parcel::new();
                            statsd::RemoveRestrictedMetricsChangedOperation {
                                config_key: a.config_key,
                                config_package: a.config_package.clone(),
                                calling_uid: uid,
                            }
                            .write(&mut d);
                            two_way(
                                statsd,
                                statsd::REMOVE_RESTRICTED_METRICS_CHANGED_OPERATION,
                                &d,
                                statsd::read_remove_restricted_metrics_changed_operation_reply,
                            )
                        },
                    )?;
                    manager::write_remove_restricted_metrics_changed_operation_reply(&mut p);
                    Ok(())
                })()
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

/// `statscompanion`.
pub struct StatsCompanionService(pub Arc<Stats>);

impl Service for StatsCompanionService {
    fn descriptor(&self) -> &str {
        companion::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let s = &self.0;
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        match call.code {
            aim_binder_host::local::DUMP_TRANSACTION => {
                let out = call
                    .data
                    .read_fd()
                    .ok()
                    .and_then(|fd| s.host.process.file(fd))
                    .and_then(|file| aim_binder_host::server::file_fd(&file));
                let result = (|| {
                    // The original dump silently returns without DUMP, and does
                    // not require PACKAGE_USAGE_STATS or the statsd UID.
                    if s.host.check_permission(Some(DUMP), uid)? != 0 {
                        return Ok(());
                    }
                    let Some(mut out) = out else {
                        return Ok(());
                    };
                    let controller = s.uid_map.lock().unwrap().clone().ok_or_else(|| {
                        s.host
                            .unimplemented_named("statscompanion", "Java dump owner (#1765)")
                    })?;
                    let mut request = Parcel::new();
                    uid_map::DumpStatsCompanion {}.write(&mut request);
                    let text = match two_way(
                        &controller,
                        uid_map::DUMP_STATS_COMPANION,
                        &request,
                        uid_map::read_dump_stats_companion_reply,
                    ) {
                        Ok(Some(text)) => text,
                        Ok(None) => {
                            return Err(Exception::new(
                                EX_ILLEGAL_STATE,
                                "null statscompanion dump",
                            ));
                        }
                        Err(Failure::Thrown(e)) => return Err(e),
                        Err(Failure::Remote(e)) => {
                            return Err(Exception::new(
                                EX_ILLEGAL_STATE,
                                format!("statscompanion dump bridge: {e}"),
                            ));
                        }
                    };
                    out.write_all(text.as_bytes()).map_err(|e| {
                        Exception::new(EX_ILLEGAL_STATE, format!("statscompanion dump: {e}"))
                    })
                })();
                return reply(result.map(|()| {
                    p.write_no_exception();
                    p
                }));
            }
            companion::STATSD_READY => {
                companion::StatsdReady::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let result = enforce_statsd_calling_uid(uid).and_then(|()| s.say_hi_to_statsd());
                match result {
                    Ok(()) => s.send_statsd_ready_broadcast(),
                    Err(e) => eprintln!("app host: statscompanion: statsdReady: {}", e.message),
                }
            }
            // Unused, but kept for the bootstrap APEX on R.
            companion::SET_ANOMALY_ALARM => {
                companion::SetAnomalyAlarm::read(&mut call.data).map_err(|_| BAD_VALUE)?;
            }
            companion::CANCEL_ANOMALY_ALARM => {
                companion::CancelAnomalyAlarm::read(&mut call.data).map_err(|_| BAD_VALUE)?;
            }
            companion::SET_PULLING_ALARM => {
                let a = companion::SetPullingAlarm::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match enforce_statsd_calling_uid(uid) {
                    Ok(()) => s.set_alarm(true, a.next_pull_time_ms),
                    Err(e) => eprintln!("app host: statscompanion: setPullingAlarm: {}", e.message),
                }
            }
            companion::CANCEL_PULLING_ALARM => {
                companion::CancelPullingAlarm::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match enforce_statsd_calling_uid(uid) {
                    Ok(()) => s.cancel_alarms(true, false),
                    Err(e) => eprintln!(
                        "app host: statscompanion: cancelPullingAlarm: {}",
                        e.message
                    ),
                }
            }
            companion::SET_ALARM_FOR_SUBSCRIBER_TRIGGERING => {
                let a = companion::SetAlarmForSubscriberTriggering::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match enforce_statsd_calling_uid(uid) {
                    Ok(()) => s.set_alarm(false, a.timestamp_ms),
                    Err(e) => eprintln!(
                        "app host: statscompanion: setAlarmForSubscriberTriggering: {}",
                        e.message
                    ),
                }
            }
            companion::CANCEL_ALARM_FOR_SUBSCRIBER_TRIGGERING => {
                companion::CancelAlarmForSubscriberTriggering::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                match enforce_statsd_calling_uid(uid) {
                    Ok(()) => s.cancel_alarms(false, true),
                    Err(e) => eprintln!(
                        "app host: statscompanion: cancelAlarmForSubscriberTriggering: {}",
                        e.message
                    ),
                }
            }
            companion::CHECK_PERMISSION => {
                let a = companion::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let result = enforce_statsd_calling_uid(uid)
                    .and_then(|()| {
                        a.permission
                            .as_deref()
                            .ok_or_else(|| Exception::illegal_argument("permission is null"))
                    })
                    .and_then(|permission| s.host.check_permission(Some(permission), a.uid))
                    .map(|granted| companion::write_check_permission_reply(&mut p, granted == 0));
                return reply(result.map(|()| p));
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
        // The rest are one-way.
        Ok(Parcel::new())
    }
}

/// `StatsCompanion.PendingIntentRef`: statsd's sends of an app's
/// PendingIntent, with the extras it asks for.
struct PendingIntentRef {
    host: Arc<AppHost>,
    target: Option<Binder>,
    _kept: Option<Strong>,
}

impl PendingIntentRef {
    /// `PendingIntent.send(context, code, intent, null, null)`: one the
    /// app host cancelled is logged, as the original logs a
    /// `CanceledException`.
    fn send(&self, extras: &[(&str, Value)], what: &str) -> Result<(), Exception> {
        let Some(target) = self.target else {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "pendingIntent",
            ));
        };
        let intent = RawIntent::of(None, 0, extras);
        let res = self.host.send_intent_sender(
            1000,
            None,
            Some(target),
            PIR_CODE,
            Some(intent),
            None,
            None,
            None,
            None,
        )?;
        if res < 0 {
            eprintln!("app host: statsmanager: {what}");
        }
        Ok(())
    }
}

impl Service for PendingIntentRef {
    fn descriptor(&self) -> &str {
        pir::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let result = match call.code {
            pir::SEND_DATA_BROADCAST => {
                let a = pir::SendDataBroadcast::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                enforce_statsd_calling_uid(uid).and_then(|()| {
                    self.send(
                        &[(EXTRA_LAST_REPORT_TIME, Value::Long(a.last_report_time_ns))],
                        "Unable to send PendingIntent",
                    )
                })
            }
            pir::SEND_ACTIVE_CONFIGS_CHANGED_BROADCAST => {
                let a = pir::SendActiveConfigsChangedBroadcast::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                enforce_statsd_calling_uid(uid).and_then(|()| {
                    self.send(
                        &[(
                            EXTRA_STATS_ACTIVE_CONFIG_KEYS,
                            Value::LongArray(a.config_ids.unwrap_or_default()),
                        )],
                        "Unable to send active configs changed broadcast using PendingIntent",
                    )
                })
            }
            pir::SEND_SUBSCRIBER_BROADCAST => {
                let a = pir::SendSubscriberBroadcast::<DimensionsValue>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                enforce_statsd_calling_uid(uid).and_then(|()| {
                    let Some(dimensions) = a.dimensions_value_parcel else {
                        return Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "dimensionsValueParcel"));
                    };
                    let cookies = a.cookies.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "cookies"))?;
                    self.send(
                        &[
                            (EXTRA_STATS_CONFIG_UID, Value::Long(a.config_uid)),
                            (EXTRA_STATS_CONFIG_KEY, Value::Long(a.config_id)),
                            (EXTRA_STATS_SUBSCRIPTION_ID, Value::Long(a.subscription_id)),
                            (EXTRA_STATS_SUBSCRIPTION_RULE_ID, Value::Long(a.subscription_rule_id)),
                            (EXTRA_STATS_DIMENSIONS_VALUE, Value::Parcelable { class: "android.os.StatsDimensionsValue".into(), data: dimensions.0 }),
                            (EXTRA_STATS_BROADCAST_SUBSCRIBER_COOKIES, Value::StringList(cookies.into_iter().map(Option::unwrap_or_default).collect())),
                        ],
                        &format!("Unable to send using PendingIntent from uid {}; presumably it had been cancelled.", a.config_uid),
                    )
                })
            }
            pir::SEND_RESTRICTED_METRICS_CHANGED_BROADCAST => {
                let a = pir::SendRestrictedMetricsChangedBroadcast::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                enforce_statsd_calling_uid(uid).and_then(|()| {
                    self.send(
                        &[(
                            EXTRA_STATS_RESTRICTED_METRIC_IDS,
                            Value::LongArray(a.metric_ids.unwrap_or_default()),
                        )],
                        "Unable to send restricted metrics changed broadcast using PendingIntent",
                    )
                })
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| {
            let mut p = Parcel::new();
            p.write_no_exception();
            p
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_puller_pair_and_companion_dump_keep_the_original_permissions() {
        use std::os::fd::AsFd;
        use std::sync::atomic::AtomicUsize;

        struct JavaOwner(Arc<AtomicUsize>);
        impl Service for JavaOwner {
            fn descriptor(&self) -> &str {
                uid_map::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.sender_euid, 1000);
                assert_eq!(call.code, uid_map::DUMP_STATS_COMPANION);
                uid_map::DumpStatsCompanion::read(&mut call.data).unwrap();
                self.0.fetch_add(1, Ordering::SeqCst);
                let mut p = Parcel::new();
                uid_map::write_dump_stats_companion_reply(
                    &mut p,
                    &Some("Number of configuration files deleted: 0\n".into()),
                );
                Ok(p)
            }
        }
        struct Statsd(std::sync::mpsc::Sender<(u32, i32, i32)>);
        impl Service for Statsd {
            fn descriptor(&self) -> &str {
                statsd::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                match call.code {
                    statsd::ALL_PULLERS_FROM_BOOT_REGISTERED => {}
                    statsd::REGISTER_PULL_ATOM_CALLBACK => {
                        let a = statsd::RegisterPullAtomCallback::read(&mut call.data).unwrap();
                        assert_eq!((a.cool_down_millis, a.timeout_millis), (1000, 500));
                        assert!(a.additive_fields.is_none());
                        assert!(a.puller_callback.is_some());
                        self.0.send((call.code, a.uid, a.atom_tag)).unwrap();
                    }
                    statsd::UNREGISTER_PULL_ATOM_CALLBACK => {
                        let a = statsd::UnregisterPullAtomCallback::read(&mut call.data).unwrap();
                        self.0.send((call.code, a.uid, a.atom_tag)).unwrap();
                    }
                    _ => panic!("unexpected statsd call {}", call.code),
                }
                Ok(Parcel::new())
            }
        }
        struct Output(std::fs::File);
        impl Service for Output {
            fn descriptor(&self) -> &str {
                "test.IOutput"
            }
            fn transact(&self, _: &mut Call<'_>) -> Reply {
                let mut p = Parcel::new();
                p.write_file(aim_binder_host::server::file_from_fd(self.0.as_fd()).unwrap());
                Ok(p)
            }
        }
        crate::system::with_discovery_binders(|host, _, _, export| {
            let permissions = Arc::new(
                super::super::permission_manager::PermissionManager::with_system(
                    host.clone(),
                    Default::default(),
                ),
            );
            assert!(host.permission_manager.set(permissions).is_ok());
            let stats = Stats::new(host.clone());
            let manager = StatsManagerService(stats.clone());
            let (sent, received) = std::sync::mpsc::channel();
            stats.statsd_ready(export(Arc::new(Statsd(sent)))).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let owner = export(Arc::new(JavaOwner(calls.clone())));
            stats
                .attach_uid_map(Some(Binder::Handle(owner.handle)))
                .unwrap();
            let mut registration = Parcel::new();
            manager::RegisterPullAtomCallback {
                atom_tag: 10014,
                cool_down_millis: 1000,
                timeout_millis: 500,
                // Original registerSystemElapsedRealtime metadata (Android 16).
                additive_fields: None,
                puller_callback: Some(Binder::Handle(owner.handle)),
            }
            .write(&mut registration);
            let mut removal = Parcel::new();
            manager::UnregisterPullAtomCallback { atom_tag: 10014 }.write(&mut removal);
            for (uid, code, request, expected) in [
                (
                    10100,
                    manager::REGISTER_PULL_ATOM_CALLBACK,
                    &registration,
                    0,
                ),
                (1000, manager::REGISTER_PULL_ATOM_CALLBACK, &registration, 1),
                (10100, manager::UNREGISTER_PULL_ATOM_CALLBACK, &removal, 1),
                (1000, manager::UNREGISTER_PULL_ATOM_CALLBACK, &removal, 0),
            ] {
                manager
                    .transact(&mut Call {
                        code,
                        flags: 1,
                        sender_pid: 71,
                        sender_euid: uid,
                        data: request.reader(),
                    })
                    .unwrap();
                assert_eq!(
                    stats.registered.lock().unwrap().pullers.iter().count(),
                    expected
                );
                if uid == 1000 {
                    let forwarded = if code == manager::REGISTER_PULL_ATOM_CALLBACK {
                        statsd::REGISTER_PULL_ATOM_CALLBACK
                    } else {
                        statsd::UNREGISTER_PULL_ATOM_CALLBACK
                    };
                    assert_eq!(
                        received.recv_timeout(Duration::from_secs(1)).unwrap(),
                        (forwarded, 1000, 10014)
                    );
                }
                assert!(received.try_recv().is_err());
            }
            let dir = super::super::test_temp_dir("statscompanion-dump");
            let path = dir.join("dump");
            let output = export(Arc::new(Output(std::fs::File::create(&path).unwrap())));
            // Receive a real Binder fd into the native endpoint's file table.
            let fd = output.transact(1, &Parcel::new(), false).unwrap();
            let companion = StatsCompanionService(stats);
            for (uid, expected_calls) in [(10100, 0), (1000, 1)] {
                companion
                    .transact(&mut Call {
                        code: aim_binder_host::local::DUMP_TRANSACTION,
                        flags: 0,
                        sender_pid: 71,
                        sender_euid: uid,
                        data: fd.reader(),
                    })
                    .unwrap()
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap();
                assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
                assert_eq!(
                    std::fs::read_to_string(&path).unwrap(),
                    if uid == 1000 {
                        "Number of configuration files deleted: 0\n"
                    } else {
                        ""
                    }
                );
            }
            std::fs::remove_dir_all(dir).unwrap();
        });
    }

    #[test]
    fn readiness_and_receiver_start_are_statsd_only() {
        assert!(enforce_statsd_calling_uid(AID_STATSD).is_ok());
        for uid in [0, 1000, 2000, 10123] {
            assert_eq!(
                enforce_statsd_calling_uid(uid).unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
        }
    }

    /// `String.hashCode`, `Long.hashCode` and `Objects.hash` as Java
    /// computes them.
    #[test]
    fn keys_hash_as_java_hashes_them() {
        // "hello".hashCode(), and "Aa" and "BB", which collide.
        assert_eq!(string_hash("hello"), 99_162_322);
        assert_eq!((string_hash("Aa"), string_hash("BB")), (2112, 2112));
        assert_eq!(string_hash(""), 0);
        assert_eq!((-1i64).java_hash(), 0);
        assert_eq!(0x1_0000_0002i64.java_hash(), 3);
        assert_eq!(objects_hash(1, 2), 994);
        assert_eq!(
            ConfigKey {
                uid: 10_150,
                config_id: 7
            }
            .java_hash(),
            objects_hash(10_150, 7)
        );
    }

    /// An `ArrayMap` keeps its keys by hash, a new key after equal hashes,
    /// and a key put again in its place.
    #[test]
    fn array_maps_order_as_android_orders_them() {
        let mut map = ArrayMap::default();
        for (k, v) in [(30i32, 'a'), (10, 'b'), (20, 'c'), (10, 'd'), (-5, 'e')] {
            map.put(k, v);
        }
        assert_eq!(
            map.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
            [(-5, 'e'), (10, 'd'), (20, 'c'), (30, 'a')]
        );
        // Two keys of one hash: `(1, 32)` and `(2, 1)` both hash to 1024.
        let mut pullers = ArrayMap::default();
        pullers.put(PullerKey { uid: 2, atom: 1 }, 'x');
        pullers.put(PullerKey { uid: 1, atom: 32 }, 'y');
        assert_eq!(
            PullerKey { uid: 2, atom: 1 }.java_hash(),
            PullerKey { uid: 1, atom: 32 }.java_hash()
        );
        assert_eq!(pullers.iter().map(|(_, v)| *v).collect::<String>(), "xy");
        assert_eq!(map.remove(&10), Some('d'));
        assert_eq!(map.remove(&10), None);
    }

    /// `sayHiToStatsd`'s registrations: the pullers, then
    /// `allPullersFromBootRegistered`, then the data fetch, active configs,
    /// broadcast subscriber and restricted metrics operations.
    #[test]
    fn registrations_follow_say_hi_to_statsd() {
        let mut r = Registered::default();
        let pir = |n: u64| Binder::Local(n << 4);
        r.restricted_metrics.put(
            ConfigKeyWithPackage {
                package: Some("p".into()),
                config_id: 1,
            },
            {
                let mut m = ArrayMap::default();
                m.put(10_100, pir(5));
                m
            },
        );
        r.broadcast_subscribers.put(
            ConfigKey {
                uid: 10_100,
                config_id: 2,
            },
            {
                let mut m = ArrayMap::default();
                m.put(9i64, pir(4));
                m.put(3i64, pir(3));
                m
            },
        );
        r.active_configs.put(10_101, pir(2));
        r.data_fetch.put(
            ConfigKey {
                uid: 10_100,
                config_id: 1,
            },
            pir(1),
        );
        r.pullers.put(
            PullerKey {
                uid: 10_100,
                atom: 10_000,
            },
            Puller {
                cool_down_millis: 1,
                timeout_millis: 2,
                additive_fields: None,
                callback: None,
            },
        );
        let kinds: Vec<_> = r
            .registrations()
            .into_iter()
            .map(|reg| match reg {
                Registration::Puller { atom, .. } => format!("puller {atom}"),
                Registration::AllPullersFromBootRegistered => "all pullers".into(),
                Registration::DataFetch { config_id, .. } => format!("fetch {config_id}"),
                Registration::ActiveConfigsChanged { uid, .. } => format!("active {uid}"),
                Registration::BroadcastSubscriber { subscriber_id, .. } => {
                    format!("subscriber {subscriber_id}")
                }
                Registration::RestrictedMetricsChanged { uid, .. } => format!("restricted {uid}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "puller 10000",
                "all pullers",
                "fetch 1",
                "active 10101",
                "subscriber 3",
                "subscriber 9",
                "restricted 10100"
            ]
        );
    }

    /// A report comes through as its size and bytes, and a short one fails.
    #[test]
    fn reports_are_copied_to_their_size() {
        let mut report = 5i32.to_be_bytes().to_vec();
        report.extend_from_slice(b"hello");
        let mut out = Vec::new();
        copy_report(&report[..], &mut out).unwrap();
        assert_eq!(out, report);
        let mut out = Vec::new();
        assert!(copy_report(&report[..7], &mut out).is_err());
    }

    /// A `StatsDimensionsValueParcel` is kept as statsd wrote it, size and
    /// all, and the reader moves past it.
    #[test]
    fn dimensions_values_are_kept_whole() {
        let mut p = Parcel::new();
        let start = p.position();
        p.write_i32(0);
        p.write_i32(3);
        p.write_i32(2);
        p.write_string16(Some("x"));
        p.set_i32_at(start, (p.position() - start) as i32);
        p.write_i32(42);
        let mut r = p.reader();
        let value = DimensionsValue::read_from(&mut r).unwrap();
        assert_eq!(value.0.len(), p.data().len() - 4);
        assert_eq!(r.read_i32().unwrap(), 42);
    }
}
