//! Processes' exit history in the app host (`AppExitInfoTracker` and
//! `ActivityManagerService.getHistoricalProcessExitReasons` at
//! `android-16.0.0_r1`, ADR 0013): the history the original persisted in
//! `/data/system/procexitstore/procexitinfo` (`AppsExitInfoProto`), read
//! as `loadExistingProcessExitInfo` reads it, and the exits of the app
//! host's own processes, noted as the original notes them: the kills it
//! makes (`scheduleNoteAppKill`, with the reason of their site), a crash
//! (`AppErrors.crashApplicationInner`) and a death (`scheduleNoteProcessDied`),
//! each kept per package and uid up to `config_app_exit_info_history_list_size`
//! and written back 30 minutes after the first unwritten one, as
//! `persistProcessExitInfo` writes it. The app host has no SDK sandbox
//! processes, no ANR traces, no tombstones and no zygote or lmkd
//! reports of an exit's status (`handleZygoteSigChld`,
//! `scheduleNoteLmkdProcKilled`), so a death without a kill stays
//! `REASON_UNKNOWN` (#1540); packages and users are not removed from it.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iapptraceretriever as retriever;

use super::{AppHost, PER_USER_RANGE, is_isolated, reply};
use crate::package::app_metadata::Descriptor;
use crate::package::info::java_hash;
use crate::package::query::Query;

/// `ApplicationExitInfo.REASON_*`.
pub(super) const REASON_UNKNOWN: i32 = 0;
pub(super) const REASON_CRASH: i32 = 4;
pub(super) const REASON_CRASH_NATIVE: i32 = 5;
pub(super) const REASON_ANR: i32 = 6;
pub(super) const REASON_PERMISSION_CHANGE: i32 = 8;
pub(super) const REASON_USER_REQUESTED: i32 = 10;
pub(super) const REASON_DEPENDENCY_DIED: i32 = 12;
pub(super) const REASON_PACKAGE_STATE_CHANGE: i32 = 15;
/// `ApplicationExitInfo.SUBREASON_*`.
pub(super) const SUBREASON_UNKNOWN: i32 = 0;
pub(super) const SUBREASON_REMOVE_TASK: i32 = 22;
/// `RunningAppProcessInfo.IMPORTANCE_FOREGROUND`, `IMPORTANCE_GONE`.
const IMPORTANCE_FOREGROUND: i32 = 100;
const IMPORTANCE_GONE: i32 = 1000;
/// `APP_EXIT_STORE_DIR`, `APP_EXIT_INFO_FILE`.
const STORE_DIR: &str = "system/procexitstore";
const INFO_FILE: &str = "procexitinfo";
/// `APP_EXIT_INFO_PERSIST_INTERVAL`.
const PERSIST_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// `ActivityManagerService.MAX_STATE_DATA_SIZE`.
const MAX_STATE_DATA_SIZE: usize = 128;
/// `StackTracesDumpHelper.ANR_FILE_PREFIX`, `APP_TRACE_FILE_SUFFIX`.
const ANR_FILE_PREFIX: &str = "anr_";
const TRACE_FILE_SUFFIX: &str = ".gz";
/// `Process.INVALID_UID`.
const INVALID_UID: i32 = -1;
/// `UserHandle.USER_ALL`, `USER_CURRENT`.
const USER_ALL: i32 = -1;
const USER_CURRENT: i32 = -2;
/// `PackageManager.MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE`.
const MATCH_DIRECT_BOOT_AWARE_UNAWARE: i64 = 0x0004_0000 | 0x0008_0000;
/// `Process.SYSTEM_UID`.
const SYSTEM_UID: i32 = 1000;

/// `android.app.ApplicationExitInfo`, as the app host keeps it. A process
/// of the app host runs its one package (`ProcessRecord.getPackageList`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ExitInfo {
    pub pid: i32,
    pub real_uid: i32,
    pub package_uid: i32,
    pub defining_uid: i32,
    pub process_name: Option<String>,
    /// None for a record read back from storage, as `readFromProto` leaves it.
    pub package_name: Option<String>,
    pub connection_group: i32,
    pub reason: i32,
    pub sub_reason: i32,
    pub status: i32,
    pub importance: i32,
    pub pss: i64,
    pub rss: i64,
    pub timestamp: i64,
    pub description: Option<String>,
    pub state: Option<Vec<u8>>,
    /// The guest path of its ANR trace.
    pub trace_file: Option<String>,
    /// `mAppTraceRetriever`.
    pub trace_retriever: Option<Binder>,
}

impl ExitInfo {
    /// `obtainRawRecord` of a process of the app host with `uid`, of a
    /// package of `package_uid`, each of which is top once it attached
    /// (`procStateToImportance` of its reported state).
    pub(super) fn raw(
        package: &str,
        process: &str,
        uid: u32,
        package_uid: u32,
        pid: i32,
        attached: bool,
    ) -> ExitInfo {
        ExitInfo {
            pid,
            real_uid: uid as i32,
            package_uid: package_uid as i32,
            defining_uid: package_uid as i32,
            process_name: Some(process.to_string()),
            package_name: Some(package.to_string()),
            reason: REASON_UNKNOWN,
            sub_reason: SUBREASON_UNKNOWN,
            importance: if attached {
                IMPORTANCE_FOREGROUND
            } else {
                IMPORTANCE_GONE
            },
            timestamp: now_ms(),
            ..Default::default()
        }
    }

    /// `ApplicationExitInfo.readFromProto`.
    fn read_proto(data: &[u8]) -> Result<ExitInfo, String> {
        let mut info = ExitInfo::default();
        fields(data, |number, value| {
            match number {
                1 => info.pid = value.int()?,
                2 => info.real_uid = value.int()?,
                3 => info.package_uid = value.int()?,
                4 => info.defining_uid = value.int()?,
                5 => info.process_name = Some(value.string()?),
                6 => info.connection_group = value.int()?,
                7 => info.reason = value.int()?,
                8 => info.sub_reason = value.int()?,
                9 => info.status = value.int()?,
                10 => info.importance = value.int()?,
                11 => info.pss = value.long()?,
                12 => info.rss = value.long()?,
                13 => info.timestamp = value.long()?,
                14 => info.description = Some(value.string()?),
                15 => info.state = Some(value.bytes()?.to_vec()),
                16 => info.trace_file = Some(value.string()?).filter(|p| !p.is_empty()),
                _ => {}
            }
            Ok(())
        })?;
        Ok(info)
    }

    /// `ApplicationExitInfo.writeToProto`.
    fn write_proto(&self) -> Vec<u8> {
        let mut w = ProtoWriter::default();
        w.varint(1, self.pid.into());
        w.varint(2, self.real_uid.into());
        w.varint(3, self.package_uid.into());
        w.varint(4, self.defining_uid.into());
        w.string(5, self.process_name.as_deref());
        w.varint(6, self.connection_group.into());
        w.varint(7, self.reason.into());
        w.varint(8, self.sub_reason.into());
        w.varint(9, self.status.into());
        w.varint(10, self.importance.into());
        w.varint(11, self.pss);
        w.varint(12, self.rss);
        w.varint(13, self.timestamp);
        w.string(14, self.description.as_deref());
        w.bytes(15, self.state.as_deref().unwrap_or_default());
        w.string(16, self.trace_file.as_deref());
        w.0
    }
}

impl WriteParcelable for ExitInfo {
    /// `ApplicationExitInfo.writeToParcel`; the app host has no tombstones,
    /// so no `mNativeTombstoneRetriever`.
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.pid);
        p.write_i32(self.real_uid);
        p.write_i32(self.package_uid);
        p.write_i32(self.defining_uid);
        p.write_string16(self.process_name.as_deref());
        p.write_string16(self.package_name.as_deref());
        p.write_i32(self.connection_group);
        p.write_i32(self.reason);
        p.write_i32(self.sub_reason);
        p.write_i32(self.status);
        p.write_i32(self.importance);
        p.write_i64(self.pss);
        p.write_i64(self.rss);
        p.write_i64(self.timestamp);
        p.write_string16(self.description.as_deref());
        aim_service_aidl::write_byte_array(p, self.state.as_deref());
        match self.trace_retriever {
            Some(binder) => {
                p.write_i32(1);
                p.write_binder(Some(binder));
            }
            None => p.write_i32(0),
        }
        p.write_i32(0);
    }
}

/// `AppExitInfoContainer`: a package's exits in one uid.
#[derive(Debug, Default)]
struct Container {
    uid: i32,
    infos: Vec<ExitInfo>,
}

impl Container {
    /// `getExitInfosLocked`: the exits of `filter_pid` (all, if not
    /// positive), newest first, at most `max_num` (all, if not positive).
    fn exit_infos(&self, filter_pid: i32, max_num: i32) -> Vec<&ExitInfo> {
        if max_num == 1 {
            return self
                .latest(filter_pid)
                .map(|i| &self.infos[i])
                .into_iter()
                .collect();
        }
        let mut infos: Vec<&ExitInfo> = self
            .infos
            .iter()
            .filter(|i| filter_pid <= 0 || i.pid == filter_pid)
            .collect();
        infos.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        if max_num > 0 {
            infos.truncate(max_num as usize);
        }
        infos
    }

    /// The first of the newest exits of `filter_pid`, as the single-result
    /// walk of `getExitInfosLocked` finds it.
    fn latest(&self, filter_pid: i32) -> Option<usize> {
        let mut found: Option<usize> = None;
        for (i, info) in self.infos.iter().enumerate() {
            if filter_pid > 0 && info.pid != filter_pid {
                continue;
            }
            if found.is_none_or(|f| self.infos[f].timestamp < info.timestamp) {
                found = Some(i);
            }
        }
        found
    }

    /// `addExitInfoLocked`, with the state the process set claimed; the
    /// oldest exit beyond `capacity`, whose trace goes with it.
    fn add(
        &mut self,
        mut info: ExitInfo,
        summaries: &mut Vec<Summary>,
        retriever: Option<Binder>,
        capacity: usize,
    ) -> Option<ExitInfo> {
        if info.state.is_none() {
            let claimed = summaries
                .iter()
                .position(|s| s.uid == info.package_uid && s.pid == info.pid);
            info.state = claimed.and_then(|at| summaries.remove(at).state);
        }
        info.trace_retriever = retriever;
        self.infos.push(info);
        if self.infos.len() <= capacity {
            return None;
        }
        let oldest = (0..self.infos.len()).fold(0, |o, i| {
            if self.infos[i].timestamp < self.infos[o].timestamp {
                i
            } else {
                o
            }
        });
        Some(self.infos.remove(oldest))
    }
}

/// A process's state (`setProcessStateSummary`), until its exit claims it.
#[derive(Debug)]
struct Summary {
    uid: i32,
    pid: i32,
    state: Option<Vec<u8>>,
}

/// `mData` (packages in `ArrayMap` order, each its containers by uid) and
/// `mActiveAppStateSummary`.
#[derive(Debug, Default)]
struct History {
    packages: Vec<(String, std::collections::BTreeMap<i32, Container>)>,
    summaries: Vec<Summary>,
    persist_scheduled: bool,
}

impl History {
    fn users(&self, package: &str) -> Option<&std::collections::BTreeMap<i32, Container>> {
        self.packages
            .iter()
            .find(|(p, _)| p == package)
            .map(|(_, users)| users)
    }

    /// `ProcessMap.put`'s package: added in `ArrayMap` order (by hash, then
    /// as added).
    fn users_mut(&mut self, package: &str) -> &mut std::collections::BTreeMap<i32, Container> {
        let at = match self.packages.iter().position(|(p, _)| p == package) {
            Some(at) => at,
            None => {
                let hash = java_hash(package);
                let at = self.packages.partition_point(|(p, _)| java_hash(p) <= hash);
                self.packages
                    .insert(at, (package.to_string(), Default::default()));
                at
            }
        };
        &mut self.packages[at].1
    }

    /// `getExitInfo`: `package`'s exits in `filter_uid`, or, without a
    /// package, the uid's in every package, newest first.
    fn exit_info(
        &self,
        package: Option<&str>,
        filter_uid: i32,
        filter_pid: i32,
        max_num: i32,
    ) -> Vec<&ExitInfo> {
        if let Some(package) = package.filter(|p| !p.is_empty()) {
            return self
                .users(package)
                .and_then(|u| u.get(&filter_uid))
                .map(|c| c.exit_infos(filter_pid, max_num))
                .unwrap_or_default();
        }
        // `forEachPackageLocked` walks the packages from the last.
        let mut results: Vec<&ExitInfo> = self
            .packages
            .iter()
            .rev()
            .filter_map(|(_, users)| users.get(&filter_uid))
            .flat_map(|c| c.exit_infos(filter_pid, 0))
            .collect();
        results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        if max_num > 0 {
            results.truncate(max_num as usize);
        }
        results
    }

    /// `getExitInfoLocked`: the newest exit of `pid` in `package` and `uid`.
    fn latest_mut(&mut self, package: &str, uid: i32, pid: i32) -> Option<&mut ExitInfo> {
        let container = self
            .packages
            .iter_mut()
            .find(|(p, _)| p == package)?
            .1
            .get_mut(&uid)?;
        container.latest(pid).map(|i| &mut container.infos[i])
    }

    /// `addExitInfoLocked` of `raw` under its package and real uid (an
    /// isolated process's owner's): the exits that fell out of the history.
    fn add(
        &mut self,
        raw: &ExitInfo,
        retriever: Option<Binder>,
        capacity: usize,
    ) -> Option<ExitInfo> {
        let package = raw.package_name.as_deref()?;
        let uid = if is_isolated(raw.real_uid) {
            raw.package_uid
        } else {
            raw.real_uid
        };
        let mut summaries = std::mem::take(&mut self.summaries);
        let container = self
            .users_mut(package)
            .entry(uid)
            .or_insert_with(|| Container {
                uid,
                infos: Vec::new(),
            });
        let dropped = container.add(raw.clone(), &mut summaries, retriever, capacity);
        self.summaries = summaries;
        dropped
    }

    /// `handleNoteProcessDiedLocked`, without zygote's or lmkd's reports:
    /// a death not noted before is added.
    fn note_process_died(
        &mut self,
        raw: &ExitInfo,
        retriever: Option<Binder>,
        capacity: usize,
    ) -> Option<Option<ExitInfo>> {
        let package = raw.package_name.as_deref()?;
        if self.latest_mut(package, raw.package_uid, raw.pid).is_some() {
            return None;
        }
        Some(self.add(raw, retriever, capacity))
    }

    /// `handleNoteAppKillLocked`: an earlier record of the pid takes the
    /// kill's reason, unless it is a crash or an ANR (`preventExitInfoUpdate`).
    fn note_app_kill(
        &mut self,
        raw: &ExitInfo,
        retriever: Option<Binder>,
        capacity: usize,
    ) -> Option<Option<ExitInfo>> {
        let package = raw.package_name.as_deref()?;
        match self.latest_mut(package, raw.package_uid, raw.pid) {
            Some(info)
                if !matches!(info.reason, REASON_ANR | REASON_CRASH | REASON_CRASH_NATIVE) =>
            {
                info.reason = raw.reason;
                info.sub_reason = raw.sub_reason;
                info.status = 0;
                info.timestamp = now_ms();
                info.description = raw.description.clone();
                None
            }
            _ => Some(self.add(raw, retriever, capacity)),
        }
    }

    /// `setProcessStateSummary`.
    fn set_summary(&mut self, uid: i32, pid: i32, state: Option<Vec<u8>>) {
        match self
            .summaries
            .iter_mut()
            .find(|s| s.uid == uid && s.pid == pid)
        {
            Some(s) => s.state = state,
            None => self.summaries.push(Summary { uid, pid, state }),
        }
    }

    /// `loadExistingProcessExitInfo`'s reading of `AppsExitInfoProto`: what
    /// it read up to an error, and the error.
    fn read(data: &[u8], history: &mut History) -> Result<(), String> {
        fields(data, |number, value| {
            if number == 2 {
                // `loadPackagesFromProto`: each user's container as it is read.
                let mut package = String::new();
                fields(value.bytes()?, |number, value| {
                    match number {
                        1 => package = value.string()?,
                        2 => {
                            let mut container = Container::default();
                            fields(value.bytes()?, |number, value| {
                                match number {
                                    1 => container.uid = value.int()?,
                                    2 => {
                                        container.infos.push(ExitInfo::read_proto(value.bytes()?)?)
                                    }
                                    _ => {}
                                }
                                Ok(())
                            })?;
                            let uid = container.uid;
                            history.users_mut(&package).insert(uid, container);
                        }
                        _ => {}
                    }
                    Ok(())
                })?;
            } else if number == 1 {
                value.long()?;
            }
            Ok(())
        })
    }

    /// `persistProcessExitInfo`'s `AppsExitInfoProto` at `now`.
    fn write(&self, now: i64) -> Vec<u8> {
        let mut w = ProtoWriter::default();
        w.varint(1, now);
        for (package, users) in self.packages.iter().rev() {
            let mut p = ProtoWriter::default();
            p.string(1, Some(package));
            for container in users.values() {
                let mut u = ProtoWriter::default();
                u.varint(1, container.uid.into());
                for info in &container.infos {
                    u.message(2, &info.write_proto());
                }
                p.message(2, &u.0);
            }
            w.message(2, &p.0);
        }
        w.0
    }

    /// The guest paths of the traces the records own.
    fn traces(&self) -> impl Iterator<Item = &str> {
        self.packages
            .iter()
            .flat_map(|(_, users)| users.values())
            .flat_map(|c| &c.infos)
            .filter_map(|i| i.trace_file.as_deref())
    }
}

/// `AppExitInfoTracker`, its history loaded.
pub(super) struct Tracker {
    me: Weak<Tracker>,
    /// The guest's `/data`.
    data: PathBuf,
    capacity: usize,
    /// `mAppTraceRetriever`.
    retriever: Option<Binder>,
    history: Mutex<History>,
}

impl Tracker {
    /// `loadExistingProcessExitInfo`, then `pruneAnrTracesIfNecessaryLocked`.
    pub(super) fn load(data: &Path, capacity: usize, retriever: Option<Binder>) -> Arc<Tracker> {
        let mut history = History::default();
        let file = data.join(STORE_DIR).join(INFO_FILE);
        match super::device_policy::read_atomic_bytes(&file) {
            Ok(Some(bytes)) => {
                if let Err(e) = History::read(&bytes, &mut history) {
                    eprintln!(
                        "app host: Error in loading historical app exit info from persistent storage: {e}"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!(
                "app host: Error in loading historical app exit info from persistent storage: {e}"
            ),
        }
        let tracker = Arc::new_cyclic(|me| Tracker {
            me: me.clone(),
            data: data.to_path_buf(),
            capacity,
            retriever,
            history: Mutex::new(history),
        });
        tracker.prune_anr_traces();
        tracker
    }

    /// The host path of a guest path under `/data`.
    fn host_path(&self, guest: &str) -> Option<PathBuf> {
        let relative = Path::new(guest).strip_prefix("/data").ok()?;
        (!relative
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)))
        .then(|| self.data.join(relative))
    }

    /// The ANR traces in the store no record owns are removed.
    fn prune_anr_traces(&self) {
        let store = self.data.join(STORE_DIR);
        let Ok(entries) = std::fs::read_dir(&store) else {
            return;
        };
        let history = self.history.lock().unwrap();
        let owned: Vec<&str> = history
            .traces()
            .filter_map(|t| Path::new(t).file_name()?.to_str())
            .collect();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(ANR_FILE_PREFIX)
                && name.ends_with(TRACE_FILE_SUFFIX)
                && !owned.contains(&name)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// An exit that fell out of the history: its trace goes, and the
    /// history is written in a while (`schedulePersistProcessExitInfo`).
    fn added(&self, history: &mut History, dropped: Option<ExitInfo>) {
        if let Some(path) = dropped
            .and_then(|d| d.trace_file)
            .and_then(|t| self.host_path(&t))
        {
            let _ = std::fs::remove_file(path);
        }
        if history.persist_scheduled {
            return;
        }
        history.persist_scheduled = true;
        let me = self.me.clone();
        std::thread::spawn(move || {
            std::thread::sleep(PERSIST_INTERVAL);
            if let Some(tracker) = me.upgrade() {
                tracker.persist();
            }
        });
    }

    /// `persistProcessExitInfo`, as `AtomicFile` writes the file.
    fn persist(&self) {
        let bytes = {
            let mut history = self.history.lock().unwrap();
            history.persist_scheduled = false;
            history.write(now_ms())
        };
        if let Err(e) = write_store(&self.data.join(STORE_DIR), &bytes) {
            eprintln!(
                "app host: Unable to write historical app exit info into persistent storage: {e}"
            );
        }
    }

    pub(super) fn note_process_died(&self, raw: &ExitInfo) {
        let mut history = self.history.lock().unwrap();
        if let Some(dropped) = history.note_process_died(raw, self.retriever, self.capacity) {
            self.added(&mut history, dropped);
        }
    }

    pub(super) fn note_app_kill(&self, raw: &ExitInfo) {
        let mut history = self.history.lock().unwrap();
        if let Some(dropped) = history.note_app_kill(raw, self.retriever, self.capacity) {
            self.added(&mut history, dropped);
        }
    }

    fn exit_info(
        &self,
        package: Option<&str>,
        filter_uid: i32,
        filter_pid: i32,
        max_num: i32,
    ) -> Vec<ExitInfo> {
        self.history
            .lock()
            .unwrap()
            .exit_info(package, filter_uid, filter_pid, max_num)
            .into_iter()
            .cloned()
            .collect()
    }
}

/// Writes `bytes` as the store's info file, as `AtomicFile` writes it,
/// making the store as `FileUtils.createDir` does.
fn write_store(store: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let system = aim_storage::guest_inode::GuestInode {
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o700),
    };
    if !store.is_dir() {
        std::fs::create_dir(store)?;
        aim_storage::guest_inode::record(store, system)?;
    }
    let new = store.join(format!("{INFO_FILE}.new"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            mode: Some(0o600),
            ..system
        },
    )?;
    std::fs::rename(&new, store.join(INFO_FILE))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A field's value as `ProtoInputStream` reads it.
pub(super) enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    /// A fixed32 or fixed64 field's little-endian bytes.
    Fixed(&'a [u8]),
}

impl Value<'_> {
    pub(super) fn int(&self) -> Result<i32, String> {
        self.long().map(|v| v as i32)
    }

    fn long(&self) -> Result<i64, String> {
        match self {
            Value::Varint(v) => Ok(*v as i64),
            _ => Err("wire type mismatch".into()),
        }
    }

    /// A `float` (fixed32) field.
    pub(super) fn float(&self) -> Result<f32, String> {
        match self {
            Value::Fixed(b) if b.len() == 4 => Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            _ => Err("wire type mismatch".into()),
        }
    }

    pub(super) fn bytes(&self) -> Result<&[u8], String> {
        match self {
            Value::Bytes(b) => Ok(b),
            _ => Err("wire type mismatch".into()),
        }
    }

    pub(super) fn string(&self) -> Result<String, String> {
        self.bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

fn varint(data: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*at).ok_or("truncated varint")?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("overlong varint".into())
}

/// Calls `field(number, value)` for each field of a message.
pub(super) fn fields(
    data: &[u8],
    mut field: impl FnMut(u64, Value<'_>) -> Result<(), String>,
) -> Result<(), String> {
    let mut at = 0;
    while at < data.len() {
        let key = varint(data, &mut at)?;
        let value = match key & 7 {
            0 => Value::Varint(varint(data, &mut at)?),
            1 | 5 => {
                let len = if key & 7 == 1 { 8 } else { 4 };
                let bytes = data.get(at..at + len).ok_or("truncated field")?;
                at += len;
                Value::Fixed(bytes)
            }
            2 => {
                let len = varint(data, &mut at)? as usize;
                let bytes = data
                    .get(at..at.checked_add(len).ok_or("truncated field")?)
                    .ok_or("truncated field")?;
                at += len;
                Value::Bytes(bytes)
            }
            wire => return Err(format!("unexpected wire type {wire}")),
        };
        field(key >> 3, value)?;
    }
    Ok(())
}

/// `ProtoOutputStream`'s writes: a scalar field only when it is not zero
/// or empty, a repeated message always.
#[derive(Default)]
struct ProtoWriter(Vec<u8>);

impl ProtoWriter {
    fn raw_varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    /// An `int32` (sign-extended) or `int64` field.
    fn varint(&mut self, number: u64, value: i64) {
        if value != 0 {
            self.raw_varint(number << 3);
            self.raw_varint(value as u64);
        }
    }

    fn bytes(&mut self, number: u64, value: &[u8]) {
        if !value.is_empty() {
            self.message(number, value);
        }
    }

    fn string(&mut self, number: u64, value: Option<&str>) {
        self.bytes(number, value.unwrap_or_default().as_bytes());
    }

    fn message(&mut self, number: u64, value: &[u8]) {
        self.raw_varint(number << 3 | 2);
        self.raw_varint(value.len() as u64);
        self.0.extend_from_slice(value);
    }
}

impl AppHost {
    /// The exit history, loaded at its first use.
    fn exit_infos(&self) -> Result<&Arc<Tracker>, Exception> {
        if let Some(tracker) = self.exit_infos.get() {
            return Ok(tracker);
        }
        let name = "config_app_exit_info_history_list_size";
        let capacity = self
            .inputs()?
            .packages
            .framework_integer(name)
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("no integer {name}"),
                )
            })?;
        let data = self.data()?.to_path_buf();
        let host = self.me.clone();
        Ok(self.exit_infos.get_or_init(|| {
            let retriever = self.process.add_service(Arc::new(TraceRetriever(host)));
            Tracker::load(&data, capacity.max(0) as usize, Some(retriever))
        }))
    }

    /// Notes `raw`'s death or kill, if the history can be loaded.
    pub(super) fn note_exit(&self, raw: &ExitInfo, kill: bool) {
        match self.exit_infos() {
            Ok(tracker) if kill => tracker.note_app_kill(raw),
            Ok(tracker) => tracker.note_process_died(raw),
            Err(e) => eprintln!("app host: exit of {} not noted: {}", raw.pid, e.message),
        }
    }

    /// `enforceDumpPermissionForPackage`: the uid of `package` in `user`,
    /// which DUMP is needed to see unless it is the caller's app.
    fn dump_permission_for_package(
        &self,
        call: &Call<'_>,
        package: &str,
        user: i32,
        function: &str,
    ) -> Result<i32, Exception> {
        let calling_uid = call.sender_euid as i32;
        if (20_000..30_000).contains(&(calling_uid % PER_USER_RANGE as i32)) {
            return Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("activity {function}: an SDK sandbox caller (#1540)"),
            ));
        }
        let (state, resolution) = self.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let uid = query
            .package_uid(package, MATCH_DIRECT_BOOT_AWARE_UNAWARE, user)
            .map_err(|e| {
                Exception::new(
                    EX_UNSUPPORTED_OPERATION,
                    format!("activity {function}: {}", e.0),
                )
            })??;
        // `UserHandle.getAppId`, -1 for no package.
        if uid % PER_USER_RANGE as i32 != calling_uid % PER_USER_RANGE as i32
            && self.check_permission(Some("android.permission.DUMP"), calling_uid)? != 0
        {
            return Err(Exception::security(format!(
                "Permission Denial: {function} from pid={}, uid={calling_uid} requires android.permission.DUMP",
                call.sender_pid
            )));
        }
        Ok(uid)
    }

    /// `getHistoricalProcessExitReasons`: the exits of `package` (the
    /// caller's uid's, without one) in `user`, newest first.
    pub(super) fn historical_process_exit_reasons(
        &self,
        call: &Call<'_>,
        package: Option<&str>,
        pid: i32,
        max_num: i32,
        user: i32,
    ) -> Result<Vec<ExitInfo>, Exception> {
        const NAME: &str = "getHistoricalProcessExitReasons";
        let calling_uid = call.sender_euid as i32;
        if is_isolated(calling_uid) {
            return Err(Exception::security(format!(
                "Isolated process not allowed to call {NAME}"
            )));
        }
        if user == USER_ALL || user == USER_CURRENT {
            return Err(Exception::illegal_argument("Unsupported userId"));
        }
        self.handle_incoming_user(calling_uid, user, true, false, Some(NAME))?;
        let package = package.filter(|p| !p.is_empty());
        let filter_uid = match package {
            Some(package) => self.dump_permission_for_package(call, package, user, NAME)?,
            None => calling_uid,
        };
        // `NativeTombstoneManager.collectTombstones` adds none: the app
        // host's processes leave no tombstones.
        if filter_uid == INVALID_UID {
            return Ok(Vec::new());
        }
        Ok(self
            .exit_infos()?
            .exit_info(package, filter_uid, pid, max_num))
    }

    /// `setProcessStateSummary` of the calling process.
    pub(super) fn set_process_state_summary(
        &self,
        call: &Call<'_>,
        state: Option<Vec<u8>>,
    ) -> Result<(), Exception> {
        if state
            .as_ref()
            .is_some_and(|s| s.len() > MAX_STATE_DATA_SIZE)
        {
            return Err(Exception::illegal_argument("Data size is too large"));
        }
        self.exit_infos()?.history.lock().unwrap().set_summary(
            call.sender_euid as i32,
            call.sender_pid,
            state,
        );
        Ok(())
    }

    /// `AppTraceRetriever.getTraceFileDescriptor`: the ANR trace of the
    /// newest exit of `pid` in `package` and `uid`'s user.
    fn trace_file_descriptor(
        &self,
        call: &Call<'_>,
        package: Option<&str>,
        uid: i32,
        pid: i32,
    ) -> Result<Option<Descriptor>, Exception> {
        const NAME: &str = "getTraceFileDescriptor";
        let calling_uid = call.sender_euid as i32;
        if is_isolated(calling_uid) {
            return Err(Exception::security(format!(
                "Isolated process not allowed to call {NAME}"
            )));
        }
        let package = package
            .filter(|p| !p.is_empty())
            .ok_or_else(|| Exception::illegal_argument("Invalid package name"))?;
        let user = uid / PER_USER_RANGE as i32;
        self.handle_incoming_user(calling_uid, user, true, false, Some(NAME))?;
        let filter_uid = self.dump_permission_for_package(call, package, user, NAME)?;
        if filter_uid == INVALID_UID {
            return Ok(None);
        }
        let tracker = self.exit_infos()?;
        let trace = tracker
            .exit_info(Some(package), filter_uid, pid, 1)
            .into_iter()
            .next()
            .and_then(|i| i.trace_file);
        let Some(path) = trace.and_then(|t| tracker.host_path(&t)) else {
            return Ok(None);
        };
        let Ok(file) = std::fs::File::open(&path) else {
            return Ok(None);
        };
        let file = aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&file))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("{NAME}: no fileport"),
                )
            })?;
        Ok(Some(Descriptor(file)))
    }
}

/// `AppExitInfoTracker.AppTraceRetriever`.
struct TraceRetriever(Weak<AppHost>);

impl Service for TraceRetriever {
    fn descriptor(&self) -> &str {
        retriever::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(host) = self.0.upgrade() else {
            return Err(aim_binder_host::parcel::DEAD_OBJECT);
        };
        match call.code {
            retriever::GET_TRACE_FILE_DESCRIPTOR => {
                let a = retriever::GetTraceFileDescriptor::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    host.trace_file_descriptor(call, a.package_name.as_deref(), a.uid, a.pid)
                        .map(|fd| {
                            let mut p = Parcel::new();
                            retriever::write_get_trace_file_descriptor_reply(&mut p, fd.as_ref());
                            p
                        }),
                )
            }
            code => reply(Err(host.unimplemented("app_trace_retriever", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(package: &str, uid: i32, pid: i32, timestamp: i64) -> ExitInfo {
        ExitInfo {
            pid,
            real_uid: uid,
            package_uid: uid,
            defining_uid: uid,
            process_name: Some(package.into()),
            package_name: Some(package.into()),
            timestamp,
            ..Default::default()
        }
    }

    fn timestamps(infos: &[&ExitInfo]) -> Vec<i64> {
        infos.iter().map(|i| i.timestamp).collect()
    }

    /// `ApplicationExitInfo.writeToParcel`: the ints, the names, the
    /// reasons, the sizes and time, the description, the state, then the
    /// trace retriever and the (absent) tombstone retriever.
    #[test]
    fn an_exit_parcels_as_the_original() {
        let info = ExitInfo {
            pid: 4321,
            real_uid: 10123,
            package_uid: 10123,
            defining_uid: 10124,
            process_name: Some("com.example:remote".into()),
            package_name: Some("com.example".into()),
            connection_group: 3,
            reason: REASON_CRASH,
            sub_reason: SUBREASON_UNKNOWN,
            status: 9,
            importance: IMPORTANCE_FOREGROUND,
            pss: 11,
            rss: 12,
            timestamp: 1_760_000_000_000,
            description: Some("crash".into()),
            state: Some(vec![1, 2, 3]),
            trace_file: Some("/data/system/procexitstore/anr_1.gz".into()),
            trace_retriever: Some(Binder::Local(0x77)),
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        for v in [4321, 10123, 10123, 10124] {
            assert_eq!(r.read_i32().unwrap(), v);
        }
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("com.example:remote")
        );
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("com.example"));
        for v in [3, REASON_CRASH, SUBREASON_UNKNOWN, 9, IMPORTANCE_FOREGROUND] {
            assert_eq!(r.read_i32().unwrap(), v);
        }
        for v in [11, 12, 1_760_000_000_000] {
            assert_eq!(r.read_i64().unwrap(), v);
        }
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("crash"));
        assert_eq!(
            aim_service_aidl::read_byte_array(&mut r).unwrap(),
            Some(vec![1, 2, 3])
        );
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_binder().unwrap(), Some(Binder::Local(0x77)));
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);

        let read_back = ExitInfo {
            package_name: None,
            state: None,
            trace_retriever: None,
            ..info
        };
        let mut p = Parcel::new();
        read_back.write_to(&mut p);
        let mut r = p.reader();
        for _ in 0..4 {
            r.read_i32().unwrap();
        }
        r.read_string16().unwrap();
        assert_eq!(r.read_string16().unwrap(), None);
        for _ in 0..5 {
            r.read_i32().unwrap();
        }
        for _ in 0..3 {
            r.read_i64().unwrap();
        }
        r.read_string16().unwrap();
        assert_eq!(r.read_i32().unwrap(), -1);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
    }

    /// `getExitInfosLocked`: newest first, by pid, at most `maxNum`; the
    /// single-result walk keeps the first of equally new exits.
    #[test]
    fn a_containers_exits_are_newest_first() {
        let mut c = Container {
            uid: 10_100,
            infos: Vec::new(),
        };
        for (pid, t) in [(1, 10), (2, 30), (1, 20), (3, 30)] {
            c.infos.push(exit("a", 10_100, pid, t));
        }
        assert_eq!(timestamps(&c.exit_infos(0, 0)), [30, 30, 20, 10]);
        assert_eq!(c.exit_infos(0, 0)[0].pid, 2);
        assert_eq!(timestamps(&c.exit_infos(1, 0)), [20, 10]);
        assert_eq!(timestamps(&c.exit_infos(0, 2)), [30, 30]);
        assert_eq!(c.exit_infos(0, 1)[0].pid, 2);
        assert_eq!(timestamps(&c.exit_infos(1, 1)), [20]);
        assert!(c.exit_infos(9, 0).is_empty());
    }

    /// `getExitInfo` without a package: the uid's exits of every package,
    /// sorted together and cut to `maxNum`; with one, only its.
    #[test]
    fn exits_of_a_uid_across_packages() {
        let mut h = History::default();
        h.add(&exit("a", 10_100, 1, 5), None, 16);
        h.add(&exit("b", 10_100, 2, 7), None, 16);
        h.add(&exit("a", 10_100, 3, 6), None, 16);
        h.add(&exit("c", 10_200, 4, 9), None, 16);
        assert_eq!(timestamps(&h.exit_info(None, 10_100, 0, 0)), [7, 6, 5]);
        assert_eq!(timestamps(&h.exit_info(Some(""), 10_100, 0, 2)), [7, 6]);
        assert_eq!(timestamps(&h.exit_info(Some("a"), 10_100, 0, 0)), [6, 5]);
        assert_eq!(timestamps(&h.exit_info(Some("a"), 10_100, 1, 0)), [5]);
        assert!(h.exit_info(Some("a"), 10_200, 0, 0).is_empty());
    }

    /// `addExitInfoLocked`: the process's state is claimed, the trace
    /// retriever set, and the oldest exit beyond the capacity dropped.
    #[test]
    fn the_history_keeps_its_capacity_and_claims_state() {
        let mut h = History::default();
        h.set_summary(10_100, 2, Some(vec![7]));
        assert!(
            h.add(&exit("a", 10_100, 1, 20), Some(Binder::Local(1)), 2)
                .is_none()
        );
        assert!(
            h.add(&exit("a", 10_100, 2, 10), Some(Binder::Local(1)), 2)
                .is_none()
        );
        let dropped = h
            .add(&exit("a", 10_100, 3, 30), Some(Binder::Local(1)), 2)
            .unwrap();
        assert_eq!(dropped.pid, 2);
        assert_eq!(dropped.state, Some(vec![7]));
        assert!(h.summaries.is_empty());
        let kept = h.exit_info(Some("a"), 10_100, 0, 0);
        assert_eq!(timestamps(&kept), [30, 20]);
        assert!(
            kept.iter()
                .all(|i| i.trace_retriever == Some(Binder::Local(1)))
        );
    }

    /// A kill overrides the pid's earlier exit, but not a crash's, and a
    /// death after a kill adds nothing.
    #[test]
    fn kills_and_deaths_as_the_original_notes_them() {
        let mut h = History::default();
        let mut crash = exit("a", 10_100, 5, 1);
        crash.reason = REASON_CRASH;
        crash.description = Some("crash".into());
        h.note_app_kill(&crash, None, 16);
        h.note_process_died(&exit("a", 10_100, 5, 2), None, 16);
        assert_eq!(h.exit_info(Some("a"), 10_100, 5, 0).len(), 1);

        let mut kill = exit("a", 10_100, 5, 3);
        kill.reason = REASON_USER_REQUESTED;
        kill.sub_reason = SUBREASON_REMOVE_TASK;
        h.note_app_kill(&kill, None, 16);
        assert_eq!(h.exit_info(Some("a"), 10_100, 5, 0).len(), 2);

        h.note_process_died(&exit("a", 10_100, 6, 4), None, 16);
        let mut kill = exit("a", 10_100, 6, 5);
        kill.reason = REASON_DEPENDENCY_DIED;
        kill.description = Some("depends".into());
        h.note_app_kill(&kill, None, 16);
        let six = h.exit_info(Some("a"), 10_100, 6, 0);
        assert_eq!(six.len(), 1);
        assert_eq!(
            (six[0].reason, six[0].description.as_deref()),
            (REASON_DEPENDENCY_DIED, Some("depends"))
        );
    }

    /// An `AppsExitInfoProto` as the original wrote it: a package with two
    /// users, unknown and fixed-width fields skipped, a negative int32
    /// sign-extended; read back as `readFromProto` reads it (no package
    /// name), and written again byte for byte.
    #[test]
    fn the_persisted_history_reads_and_writes_as_the_original() {
        let mut info = ProtoWriter::default();
        info.varint(1, 4321);
        info.varint(2, 10_123);
        info.varint(3, 10_123);
        info.varint(4, 10_123);
        info.string(5, Some("com.example"));
        info.varint(7, REASON_CRASH.into());
        info.varint(9, -1);
        info.varint(10, IMPORTANCE_FOREGROUND.into());
        info.varint(13, 1_760_000_000_000);
        info.string(14, Some("crash"));
        info.bytes(15, &[9, 8]);
        info.string(16, Some("/data/system/procexitstore/anr_x.gz"));
        let mut user = ProtoWriter::default();
        user.varint(1, 10_123);
        user.message(2, &info.0);
        let mut other = ProtoWriter::default();
        other.varint(1, 1_010_123);
        let mut package = ProtoWriter::default();
        package.string(1, Some("com.example"));
        package.message(2, &user.0);
        package.message(2, &other.0);
        let mut file = ProtoWriter::default();
        file.varint(1, 1_760_000_000_500);
        file.message(2, &package.0);
        // An unknown fixed32 and fixed64 field.
        file.0
            .extend_from_slice(&[0x1d, 1, 2, 3, 4, 0x21, 1, 2, 3, 4, 5, 6, 7, 8]);

        let mut h = History::default();
        History::read(&file.0, &mut h).unwrap();
        let read = h.exit_info(Some("com.example"), 10_123, 0, 0);
        assert_eq!(read.len(), 1);
        let read = read[0].clone();
        assert_eq!(
            (read.pid, read.package_uid, read.reason, read.status),
            (4321, 10_123, REASON_CRASH, -1)
        );
        assert_eq!(read.package_name, None);
        assert_eq!(read.process_name.as_deref(), Some("com.example"));
        assert_eq!(read.state, Some(vec![9, 8]));
        assert_eq!(
            read.trace_file.as_deref(),
            Some("/data/system/procexitstore/anr_x.gz")
        );
        assert!(h.users("com.example").unwrap().contains_key(&1_010_123));
        assert_eq!(h.write(1_760_000_000_500), file.0[..file.0.len() - 14]);

        // A wire type mismatch stops the read, keeping what came before.
        let mut bad = ProtoWriter::default();
        bad.message(2, &package.0);
        bad.message(2, &[0x0a, 1, b'z', 0x10, 1]);
        let mut h = History::default();
        assert!(History::read(&bad.0, &mut h).is_err());
        assert!(h.users("com.example").is_some());
        assert!(h.users("z").is_none());
    }

    /// The tracker writes its history where the original keeps it and
    /// loads it back.
    #[test]
    fn the_tracker_persists_and_loads() {
        let data = std::env::temp_dir().join(format!("aim-exit-info-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(data.join("system/procexitstore")).unwrap();
        std::fs::write(data.join("system/procexitstore/anr_orphan.gz"), b"x").unwrap();
        let tracker = Tracker::load(&data, 16, None);
        assert!(!data.join("system/procexitstore/anr_orphan.gz").exists());
        tracker.note_process_died(&ExitInfo::raw(
            "com.example",
            "com.example",
            10_123,
            10_123,
            77,
            true,
        ));
        tracker.persist();
        let loaded = Tracker::load(&data, 16, None);
        let infos = loaded.exit_info(Some("com.example"), 10_123, 0, 0);
        assert_eq!(infos.len(), 1);
        assert_eq!(
            (infos[0].pid, infos[0].reason, infos[0].importance),
            (77, REASON_UNKNOWN, IMPORTANCE_FOREGROUND)
        );
        std::fs::remove_dir_all(&data).unwrap();
    }
}
