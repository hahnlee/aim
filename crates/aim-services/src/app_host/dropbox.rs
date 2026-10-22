//! `dropbox`: DropBoxManagerService for the app host
//! (`android-16.0.0_r1`): each entry a file of `/data/system/dropbox`,
//! named and flagged as the original names them
//! (`<tag>@<millis>.{txt,dat}[.gz]`, `.lost` for a tombstone), a tag
//! enabled or disabled by `Settings.Global`'s `dropbox:<tag>`, aged out
//! and trimmed to the quota the global settings and the free space allow
//! (`trimToFit`), and read back (`getNextEntry`) with PEEK_DROPBOX_DATA,
//! or READ_LOGS (READ_DROPBOX_DATA where `ENFORCE_READ_DROPBOX_DATA`
//! applies) and the usage stats op. Each entry added is broadcast
//! (`ACTION_DROPBOX_ENTRY_ADDED`), a low priority tag's at most once a
//! rate limit period with the count of those dropped meanwhile. The
//! broadcasts' delivery groups and deferral until their receivers are
//! active, the dropped entries' statsd atoms, `dump` and the trim at
//! low storage (the app host has no DeviceStorageMonitorService) are not
//! implemented (#1937).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_NULL_POINTER, Exception, Parcel, Reader,
};
use aim_service_aidl::com_android_internal_os_idropboxmanagerservice as db;
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use aim_storage::guest_inode::{self, GuestInode};

use super::content_service::LocalObserver;
use super::kept::RawIntent;
use super::settings::Table;
use super::usage_database::mkdirs;
use super::{AppHost, appops_service, reply};
use crate::bundle::Value;
use crate::package::uri::Uri;
use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};

/// `DropBoxManager.IS_EMPTY`, `IS_TEXT`, `IS_GZIPPED`, `HAS_BYTE_ARRAY`.
const IS_EMPTY: i32 = 1;
const IS_TEXT: i32 = 2;
const IS_GZIPPED: i32 = 4;
const HAS_BYTE_ARRAY: i32 = 8;
const ACTION_DROPBOX_ENTRY_ADDED: &str = "android.intent.action.DROPBOX_ENTRY_ADDED";
const EXTRA_TAG: &str = "tag";
const EXTRA_TIME: &str = "time";
const EXTRA_DROPPED_COUNT: &str = "android.os.extra.DROPPED_COUNT";
/// `Intent.FLAG_RECEIVER_REGISTERED_ONLY`.
const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
const USER_ALL: i32 = -1;

const READ_LOGS: &str = "android.permission.READ_LOGS";
const READ_DROPBOX_DATA: &str = "android.permission.READ_DROPBOX_DATA";
const PEEK_DROPBOX_DATA: &str = "android.permission.PEEK_DROPBOX_DATA";
const PACKAGE_USAGE_STATS: &str = "android.permission.PACKAGE_USAGE_STATS";
/// `AppOpsManager.OP_GET_USAGE_STATS`.
const OP_GET_USAGE_STATS: i32 = 43;
/// `ENFORCE_READ_DROPBOX_DATA`, and the flag that turns it on.
const ENFORCE_READ_DROPBOX_DATA: i64 = 296060945;
const ENABLE_READ_DROPBOX_PERMISSION: &str =
    "com.android.server.feature.flags.enable_read_dropbox_permission";
const TAG: &str = "DropBoxManagerService";

/// The service's defaults.
const DEFAULT_AGE_SECONDS: i32 = 3 * 86400;
const DEFAULT_MAX_FILES: i32 = 1000;
const DEFAULT_MAX_FILES_LOWRAM: i32 = 300;
const DEFAULT_QUOTA_PERCENT: i32 = 10;
const DEFAULT_RESERVE_PERCENT: i32 = 0;
const QUOTA_RESCAN: Duration = Duration::from_millis(5000);
/// Size beyond which an entry is compressed as it is written.
const COMPRESS_THRESHOLD_BYTES: u64 = 16_384;
/// Tags dropped unless enabled.
const DISABLED_BY_DEFAULT_TAGS: &[&str] = &["data_app_wtf", "system_app_wtf", "system_server_wtf"];

/// `Settings.Global`'s names.
const DROPBOX_TAG_PREFIX: &str = "dropbox:";
const DROPBOX_AGE_SECONDS: &str = "dropbox_age_seconds";
const DROPBOX_MAX_FILES: &str = "dropbox_max_files";
const DROPBOX_QUOTA_KB: &str = "dropbox_quota_kb";
const DROPBOX_QUOTA_PERCENT: &str = "dropbox_quota_percent";
const DROPBOX_RESERVE_PERCENT: &str = "dropbox_reserve_percent";

/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");

/// What system_server makes in `/data/system` (umask 077).
const SYSTEM_FILE: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o600),
};

fn null_tag() -> Exception {
    Exception::new(EX_NULL_POINTER, "")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `Uri.encode`: all but the unreserved characters escaped, as UTF-8.
fn uri_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_-!.~'()*".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `Uri.decode`: `%XX` escapes decoded, a malformed one kept as it is.
fn uri_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `EntryFile`: an entry's metadata, ordered as the original orders
/// them (time, tag, flags, then identity: `seq`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EntryFile {
    timestamp: i64,
    tag: Option<String>,
    flags: i32,
    seq: u64,
    blocks: i32,
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn blocks_of(len: u64, block_size: u64) -> i32 {
    len.div_ceil(block_size) as i32
}

impl EntryFile {
    fn new(timestamp: i64, tag: Option<String>, flags: i32, blocks: i32) -> Self {
        EntryFile {
            timestamp,
            tag,
            flags,
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            blocks,
        }
    }

    /// The `EntryFile(file, blockSize)` of an existing file: none for a
    /// name it does not recognize.
    fn parse(name: &str, len: u64, block_size: u64) -> Option<Self> {
        let at = name.rfind('@')?;
        let tag = uri_decode(&name[..at]);
        let mut flags = 0;
        let mut rest = name;
        if let Some(r) = rest.strip_suffix(".gz") {
            flags |= IS_GZIPPED;
            rest = r;
        }
        let millis = if let Some(r) = rest.strip_suffix(".lost") {
            flags |= IS_EMPTY;
            r.get(at + 1..)?
        } else if let Some(r) = rest.strip_suffix(".txt") {
            flags |= IS_TEXT;
            r.get(at + 1..)?
        } else {
            rest.strip_suffix(".dat")?.get(at + 1..)?
        };
        let millis = java_parse_long(millis)?;
        Some(Self::new(
            millis,
            Some(tag),
            flags,
            blocks_of(len, block_size),
        ))
    }

    /// `getExtension`.
    fn extension(&self) -> &'static str {
        if self.flags & IS_EMPTY != 0 {
            return ".lost";
        }
        match (self.flags & IS_TEXT != 0, self.flags & IS_GZIPPED != 0) {
            (true, true) => ".txt.gz",
            (true, false) => ".txt",
            (false, true) => ".dat.gz",
            (false, false) => ".dat",
        }
    }

    /// `getFile`: none for an entry with no tag.
    fn file(&self, dir: &Path) -> Option<PathBuf> {
        let tag = self.tag.as_ref()?;
        Some(dir.join(format!(
            "{}@{}{}",
            uri_encode(tag),
            self.timestamp,
            self.extension()
        )))
    }
}

/// `Long.parseLong`.
fn java_parse_long(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `FileList`.
#[derive(Default)]
struct FileList {
    blocks: i32,
    contents: BTreeSet<EntryFile>,
}

/// The files enrolled (`mAllFiles`, `mFilesByTag`).
#[derive(Default)]
struct Files {
    all: FileList,
    by_tag: HashMap<String, FileList>,
}

impl Files {
    /// `enrollEntry`.
    fn enroll(&mut self, entry: EntryFile) {
        self.all.blocks += entry.blocks;
        if let Some(tag) = entry.tag.clone().filter(|_| entry.blocks > 0) {
            let list = self.by_tag.entry(tag).or_default();
            list.blocks += entry.blocks;
            list.contents.insert(entry.clone());
        }
        self.all.contents.insert(entry);
    }

    /// The entry removed from its tag's list and from all.
    fn remove(&mut self, entry: &EntryFile) {
        if let Some(list) = entry.tag.as_ref().and_then(|t| self.by_tag.get_mut(t))
            && list.contents.remove(entry)
        {
            list.blocks -= entry.blocks;
        }
        if self.all.contents.remove(entry) {
            self.all.blocks -= entry.blocks;
        }
    }
}

/// What `init` sets up, and the quota it caches.
struct Store {
    dir: PathBuf,
    /// `mStatFs`'s block size, once the directory is there.
    block_size: Option<u64>,
    files: Option<Files>,
    cached_quota_blocks: i32,
    cached_quota_at: Option<Instant>,
    max_files: i32,
    low_priority_period: i64,
    low_priority_tags: Option<HashSet<String>>,
}

/// What the global settings set of the service: a setting absent or not
/// an integer is its default (`Settings.Global.getInt`).
trait Inputs {
    fn global(&self, name: &str) -> Option<String>;
    fn global_int(&self, name: &str, default: i32) -> i32 {
        self.global(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    fn low_ram(&self) -> bool;
    fn userdebug(&self) -> bool;
}

impl Inputs for AppHost {
    fn global(&self, name: &str) -> Option<String> {
        self.inputs().ok()?.settings.get(Table::Global, name)
    }
    fn low_ram(&self) -> bool {
        self.property("ro.config.low_ram").as_deref() == Some("true")
    }
    fn userdebug(&self) -> bool {
        self.property("ro.build.type").as_deref() == Some("userdebug")
    }
}

/// `StatFs` of the directory: its block size, available and total blocks.
fn statfs(dir: &Path) -> std::io::Result<(u64, u64, u64)> {
    let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a statvfs to fill.
    if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((s.f_frsize as u64, s.f_bavail as u64, s.f_blocks as u64))
}

impl Store {
    fn new(dir: PathBuf) -> Self {
        Store {
            dir,
            block_size: None,
            files: None,
            cached_quota_blocks: 0,
            cached_quota_at: None,
            max_files: -1,
            low_priority_period: 0,
            low_priority_tags: None,
        }
    }

    /// `init`: the directory made and statted, its files scanned once.
    fn init(&mut self) -> std::io::Result<(&mut Files, u64)> {
        if self.dir.as_os_str().is_empty() {
            return Err(std::io::Error::other("the app host is not configured"));
        }
        let block_size = match self.block_size {
            Some(b) => b,
            None => {
                mkdirs(&self.dir).map_err(|e| {
                    std::io::Error::new(e.kind(), format!("Can't mkdir: {}", self.dir.display()))
                })?;
                let (b, _, _) = statfs(&self.dir).map_err(|e| {
                    std::io::Error::new(e.kind(), format!("Can't statfs: {}", self.dir.display()))
                })?;
                self.block_size = Some(b);
                b
            }
        };
        if self.files.is_none() {
            let list = std::fs::read_dir(&self.dir).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("Can't list files: {}", self.dir.display()),
                )
            })?;
            let mut files = Files::default();
            for f in list.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let path = f.path();
                if name.ends_with(".tmp") {
                    eprintln!("app host: {TAG}: Cleaning temp file: {}", path.display());
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                let len = f.metadata().map_or(0, |m| m.len());
                match EntryFile::parse(&name, len, block_size) {
                    Some(entry) => files.enroll(entry),
                    None => {
                        eprintln!("app host: {TAG}: Invalid filename: {}", path.display());
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            self.files = Some(files);
        }
        Ok((self.files.as_mut().unwrap(), block_size))
    }

    /// A zero-length tombstone of an entry whose contents were lost.
    fn tombstone(dir: &Path, tag: Option<String>, timestamp: i64) -> std::io::Result<EntryFile> {
        let entry = EntryFile::new(timestamp, tag, IS_EMPTY, 0);
        if let Some(path) = entry.file(dir) {
            create_system_file(&path)?;
        }
        Ok(entry)
    }

    /// `createEntry`: `temp` (or a tombstone without it) renamed to the
    /// entry of `tag` at a unique time, which it answers; entries more
    /// than 10 s ahead (clock skew) are brought back after it.
    fn create_entry(&mut self, temp: Option<&Path>, tag: &str, flags: i32) -> std::io::Result<i64> {
        let dir = self.dir.clone();
        let (files, block_size) = self.init()?;
        let mut t = now_millis();
        let future: Vec<EntryFile> = files
            .all
            .contents
            .iter()
            .filter(|e| e.timestamp >= t + 10000)
            .cloned()
            .collect();
        for late in &future {
            files.all.contents.remove(late);
        }
        if let Some(last) = files.all.contents.last() {
            t = t.max(last.timestamp + 1);
        }
        for late in future {
            files.all.blocks -= late.blocks;
            if let Some(list) = late.tag.as_ref().and_then(|t| files.by_tag.get_mut(t))
                && list.contents.remove(&late)
            {
                list.blocks -= late.blocks;
            }
            let moved = if late.flags & IS_EMPTY == 0 {
                rename_entry(
                    &dir,
                    late.file(&dir).as_deref(),
                    late.tag,
                    t,
                    late.flags,
                    block_size,
                )?
            } else {
                Self::tombstone(&dir, late.tag, t)?
            };
            t += 1;
            files.enroll(moved);
        }
        let entry = match temp {
            None => Self::tombstone(&dir, Some(tag.to_string()), t)?,
            Some(temp) => rename_entry(
                &dir,
                Some(temp),
                Some(tag.to_string()),
                t,
                flags,
                block_size,
            )?,
        };
        files.enroll(entry);
        Ok(t)
    }

    /// `trimToFit`: aged entries expunged, then the tags squeezed fairly
    /// into the quota, their lost entries tombstoned. The quota in bytes.
    fn trim_to_fit(&mut self, inputs: &dyn Inputs) -> std::io::Result<i64> {
        let dir = self.dir.clone();
        let age_seconds = inputs.global_int(DROPBOX_AGE_SECONDS, DEFAULT_AGE_SECONDS);
        self.max_files = inputs.global_int(
            DROPBOX_MAX_FILES,
            if inputs.low_ram() {
                DEFAULT_MAX_FILES_LOWRAM
            } else {
                DEFAULT_MAX_FILES
            },
        );
        let max_files = self.max_files;
        let cutoff = now_millis() - age_seconds as i64 * 1000;
        {
            let (files, _) = self.init()?;
            while let Some(first) = files.all.contents.first().cloned() {
                if first.timestamp > cutoff && (files.all.contents.len() as i32) < max_files {
                    break;
                }
                files.remove(&first);
                if let Some(path) = first.file(&dir) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let block_size = self.block_size.unwrap_or(1);
        if self
            .cached_quota_at
            .is_none_or(|at| at.elapsed() > QUOTA_RESCAN)
        {
            let quota_percent = inputs.global_int(DROPBOX_QUOTA_PERCENT, DEFAULT_QUOTA_PERCENT);
            let reserve_percent =
                inputs.global_int(DROPBOX_RESERVE_PERCENT, DEFAULT_RESERVE_PERCENT);
            let default_quota_kb = if inputs.userdebug() {
                20 * 1024
            } else {
                10 * 1024
            };
            let quota_kb = inputs.global_int(DROPBOX_QUOTA_KB, default_quota_kb);
            let (_, available, total) = statfs(&dir).map_err(|e| {
                std::io::Error::new(e.kind(), format!("Can't restat: {}", dir.display()))
            })?;
            let nonreserved = available as i64 - total as i64 * reserve_percent as i64 / 100;
            let max_available =
                (nonreserved * quota_percent as i64 / 100).clamp(0, i32::MAX as i64);
            let maximum = (quota_kb as i64 * 1024 / block_size as i64) as i32;
            self.cached_quota_blocks = maximum.min(max_available as i32);
            self.cached_quota_at = Some(Instant::now());
        }
        let quota = self.cached_quota_blocks;
        let (files, _) = self.init()?;
        if files.all.blocks > quota {
            // The tags biggest first; each squeezed to a fair share.
            let mut tags: Vec<(String, i32)> = files
                .by_tag
                .iter()
                .map(|(t, l)| (t.clone(), l.blocks))
                .collect();
            tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let (mut unsqueezed, mut squeezed) = (files.all.blocks, 0);
            for (_, blocks) in &tags {
                if squeezed > 0 && *blocks <= (quota - unsqueezed) / squeezed {
                    break;
                }
                unsqueezed -= blocks;
                squeezed += 1;
            }
            let tag_quota = (quota - unsqueezed) / squeezed.max(1);
            for (tag, _) in &tags {
                if files.all.blocks < quota {
                    break;
                }
                while let Some(entry) = files
                    .by_tag
                    .get(tag)
                    .filter(|list| list.blocks > tag_quota)
                    .and_then(|list| list.contents.first().cloned())
                {
                    files.remove(&entry);
                    if let Some(path) = entry.file(&dir) {
                        let _ = std::fs::remove_file(path);
                    }
                    match Self::tombstone(&dir, entry.tag.clone(), entry.timestamp) {
                        Ok(t) => files.enroll(t),
                        Err(e) => eprintln!("app host: {TAG}: Can't write tombstone file: {e}"),
                    }
                }
            }
        }
        Ok(quota as i64 * block_size as i64)
    }
}

/// A file system_server creates, owned by the system.
fn create_system_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    guest_inode::record(path, SYSTEM_FILE)?;
    Ok(file)
}

/// `EntryFile(temp, dir, tag, timestamp, flags, blockSize)`: `temp`
/// renamed to the entry's file.
fn rename_entry(
    dir: &Path,
    temp: Option<&Path>,
    tag: Option<String>,
    timestamp: i64,
    flags: i32,
    block_size: u64,
) -> std::io::Result<EntryFile> {
    let mut entry = EntryFile::new(timestamp, tag, flags, 0);
    let (Some(temp), Some(file)) = (temp, entry.file(dir)) else {
        return Err(std::io::Error::other(
            "Can't rename an entry without a file",
        ));
    };
    std::fs::rename(temp, &file).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("Can't rename {} to {}", temp.display(), file.display()),
        )
    })?;
    entry.blocks = blocks_of(std::fs::metadata(&file)?.len(), block_size);
    Ok(entry)
}

/// `DropBoxManager.Entry` as `writeToParcel` writes it: a tombstone's
/// null byte array, else its file, read-only.
enum Entry {
    Empty {
        tag: String,
        time: i64,
    },
    File {
        tag: String,
        time: i64,
        flags: i32,
        file: aim_binder_driver::File,
    },
}

impl WriteParcelable for Entry {
    fn write_to(&self, p: &mut Parcel) {
        match self {
            Entry::Empty { tag, time } => {
                p.write_string16(Some(tag));
                p.write_i64(*time);
                p.write_i32(IS_EMPTY | HAS_BYTE_ARRAY);
                p.write_i32(-1);
            }
            Entry::File {
                tag,
                time,
                flags,
                file,
            } => {
                p.write_string16(Some(tag));
                p.write_i64(*time);
                p.write_i32(flags & !HAS_BYTE_ARRAY);
                p.write_i32(0);
                p.write_file(file.clone());
            }
        }
    }
}

/// A `ParcelFileDescriptor` as it is parcelled: its fd (the call's).
struct ReceivedFd(u32);

impl ReadParcelable for ReceivedFd {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let comm = r.read_i32()? != 0;
        let fd = r.read_fd()?;
        if comm {
            r.read_fd()?;
        }
        Ok(ReceivedFd(fd))
    }
}

/// An entry's contents as it is written (`SimpleEntrySource`).
enum Source<'a> {
    Data(&'a [u8]),
    File(aim_binder_host::server::RetainedFd),
}

pub struct DropBox {
    host: Arc<AppHost>,
    store: Mutex<Store>,
    booted: AtomicBool,
    /// The low priority tags' broadcasts waiting out the rate limit
    /// (`mDeferredMap`): their latest time and the count dropped.
    deferred: Mutex<HashMap<String, (i64, i32)>>,
    me: Weak<DropBox>,
}

impl DropBox {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        Arc::new_cyclic(|me| DropBox {
            host,
            store: Mutex::new(Store::new(PathBuf::new())),
            booted: AtomicBool::new(false),
            deferred: Mutex::new(HashMap::new()),
            me: me.clone(),
        })
    }

    /// The store, in the guest's `/data/system/dropbox` once the app host
    /// knows its data.
    fn locked(&self) -> std::sync::MutexGuard<'_, Store> {
        let mut store = self.store.lock().unwrap();
        if store.dir.as_os_str().is_empty()
            && let Ok(data) = self.host.data()
        {
            store.dir = data.join("system/dropbox");
        }
        store
    }

    /// `PHASE_SYSTEM_SERVICES_READY`: the global settings watched, the low
    /// priority tags and their rate limit read.
    pub fn system_services_ready(&self) -> Result<(), Exception> {
        self.low_priority_resource_configs()?;
        let content = self.host.content_service.get().cloned().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "no content service",
            )
        })?;
        let observer: Arc<dyn LocalObserver> = Arc::new(SettingsObserver(self.me.clone()));
        content.register_local(&Uri::parse("content://settings/global"), true, 0, observer)?;
        Ok(())
    }

    /// `PHASE_BOOT_COMPLETED`.
    /// `DropBoxManager.addText(tag, data)`: the text added as an
    /// `IS_TEXT` entry of the tag (`addData`).
    pub(super) fn add_text(&self, tag: &str, text: &str) -> Result<(), Exception> {
        self.add_entry(
            Some(tag),
            Source::Data(text.as_bytes()),
            text.len() as u64,
            IS_TEXT,
        )
    }

    pub fn boot_completed(&self) {
        self.booted.store(true, Ordering::Release);
    }

    /// `getLowPriorityResourceConfigs`.
    fn low_priority_resource_configs(&self) -> Result<(), Exception> {
        let inputs = self.host.inputs()?;
        let period = inputs
            .packages
            .framework_integer("config_dropboxLowPriorityBroadcastRateLimitPeriod")
            .unwrap_or(0);
        let tags: HashSet<String> = inputs
            .packages
            .framework_string_array("config_dropboxLowPriorityTags")
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect();
        let mut store = self.store.lock().unwrap();
        store.low_priority_period = period as i64;
        store.low_priority_tags = (!tags.is_empty()).then_some(tags);
        Ok(())
    }

    /// `isTagEnabled`; of a null tag, the NullPointerException
    /// `List.contains` throws.
    fn tag_enabled(&self, tag: Option<&str>) -> Result<bool, Exception> {
        let tag = tag.ok_or_else(null_tag)?;
        let setting = self.host.global(&format!("{DROPBOX_TAG_PREFIX}{tag}"));
        Ok(if DISABLED_BY_DEFAULT_TAGS.contains(&tag) {
            setting.as_deref() == Some("enabled")
        } else {
            setting.as_deref() != Some("disabled")
        })
    }

    /// `addEntry`: the entry written, unless its tag is disabled, or a
    /// tombstone of one too big for the quota, then broadcast. A failure
    /// to write is logged, as the original logs it.
    fn add_entry(
        &self,
        tag: Option<&str>,
        source: Source<'_>,
        length: u64,
        mut flags: i32,
    ) -> Result<(), Exception> {
        let enabled = self.tag_enabled(tag)?;
        let tag = tag.unwrap_or_default();
        if flags & IS_EMPTY != 0 {
            return Err(Exception::new(EX_ILLEGAL_ARGUMENT, ""));
        }
        let force_compress = flags & IS_GZIPPED == 0 && length > COMPRESS_THRESHOLD_BYTES;
        if force_compress {
            flags |= IS_GZIPPED;
        }
        let written = (|| -> std::io::Result<Option<i64>> {
            let mut store = self.locked();
            store.init()?;
            if !enabled {
                return Ok(None);
            }
            let max = store.trim_to_fit(&*self.host)?;
            let temp = if length as i64 > max {
                eprintln!("app host: {TAG}: Dropping: {tag} ({length} > {max} bytes)");
                None
            } else {
                let temp = store
                    .dir
                    .join(format!("drop{}.tmp", SEQ.fetch_add(1, Ordering::Relaxed)));
                let out = create_system_file(&temp)?;
                let result = write_source(source, out, force_compress);
                if let Err(e) = result {
                    let _ = std::fs::remove_file(&temp);
                    return Err(e);
                }
                Some(temp)
            };
            let created = store.create_entry(temp.as_deref(), tag, flags);
            if created.is_err()
                && let Some(temp) = &temp
            {
                let _ = std::fs::remove_file(temp);
            }
            created.map(Some)
        })();
        match written {
            Ok(Some(time)) => self.entry_added(tag, time),
            Ok(None) => {}
            Err(e) => eprintln!("app host: {TAG}: Can't write: {tag}: {e}"),
        }
        Ok(())
    }

    /// The entry's broadcast: a low priority tag's deferred by the rate
    /// limit (`maybeDeferBroadcast`), else sent at once.
    fn entry_added(&self, tag: &str, time: i64) {
        let (low, period) = {
            let store = self.store.lock().unwrap();
            let low = store
                .low_priority_tags
                .as_ref()
                .is_some_and(|t| t.contains(tag));
            (low, store.low_priority_period)
        };
        if !low {
            return self.send_broadcast(tag, time, 0);
        }
        let mut deferred = self.deferred.lock().unwrap();
        if let Some((latest, dropped)) = deferred.get_mut(tag) {
            *latest = time;
            *dropped += 1;
            return;
        }
        deferred.insert(tag.to_string(), (time, 0));
        let me = self.me.clone();
        let tag = tag.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(period.max(0) as u64));
            if let Some(me) = me.upgrade()
                && let Some((time, dropped)) = me.deferred.lock().unwrap().remove(&tag)
            {
                me.send_broadcast(&tag, time, dropped);
            }
        });
    }

    /// `prepareAndSendBroadcast`: to receivers holding READ_DROPBOX_DATA
    /// whose apps the compat change applies to and those holding
    /// READ_LOGS whose apps it does not, or with the flag off to READ_LOGS
    /// alone; registered receivers only until the boot completes.
    fn send_broadcast(&self, tag: &str, time: i64, dropped: i32) {
        let flags = if self.booted.load(Ordering::Acquire) {
            0
        } else {
            FLAG_RECEIVER_REGISTERED_ONLY
        };
        let intent = || {
            RawIntent::new(
                ACTION_DROPBOX_ENTRY_ADDED,
                flags,
                &[
                    (EXTRA_TAG, Value::String(Some(tag.to_string()))),
                    (EXTRA_TIME, Value::Long(time)),
                    (EXTRA_DROPPED_COUNT, Value::Int(dropped)),
                ],
            )
        };
        let broadcasts = &self.host.broadcasts;
        match self.host.flag(ENABLE_READ_DROPBOX_PERMISSION) {
            Ok(true) => {
                broadcasts.send_from_system_requiring_change(
                    &self.host,
                    intent(),
                    USER_ALL,
                    READ_DROPBOX_DATA,
                    (ENFORCE_READ_DROPBOX_DATA, true),
                );
                broadcasts.send_from_system_requiring_change(
                    &self.host,
                    intent(),
                    USER_ALL,
                    READ_LOGS,
                    (ENFORCE_READ_DROPBOX_DATA, false),
                );
            }
            Ok(false) => broadcasts.send_from_system_with_permission(
                &self.host,
                intent(),
                USER_ALL,
                READ_LOGS,
            ),
            Err(e) => eprintln!("app host: {TAG}: the entry's broadcast: {}", e.message),
        }
    }

    /// `checkPermission`: PEEK_DROPBOX_DATA, or the read permission and
    /// the usage stats op. Whether the caller may read.
    fn may_read(
        &self,
        uid: i32,
        package: Option<&str>,
        attribution: Option<&str>,
    ) -> Result<bool, Exception> {
        let host = &self.host;
        if host.check_permission(Some(PEEK_DROPBOX_DATA), uid)? == 0 {
            return Ok(true);
        }
        let permission = if host.flag(ENABLE_READ_DROPBOX_PERMISSION)?
            && host.change_enabled(ENFORCE_READ_DROPBOX_DATA, uid)?
        {
            READ_DROPBOX_DATA
        } else {
            READ_LOGS
        };
        let enforce = |permission: &str| -> Result<(), Exception> {
            if host.check_permission(Some(permission), uid)? != 0 {
                return Err(Exception::security(format!(
                    "{TAG}: Neither user {uid} nor current process has {permission}."
                )));
            }
            Ok(())
        };
        enforce(permission)?;
        match appops_service::note_from(
            host,
            super::SYSTEM_UID as i32,
            OP_GET_USAGE_STATS,
            uid,
            package,
            attribution,
        )? {
            appops_service::MODE_ALLOWED => Ok(true),
            appops_service::MODE_DEFAULT => enforce(PACKAGE_USAGE_STATS).map(|()| true),
            _ => Ok(false),
        }
    }

    /// `getNextEntry`: the first entry of `tag` (any, with none) after
    /// `millis` whose file opens.
    fn next_entry(
        &self,
        uid: i32,
        tag: Option<&str>,
        millis: i64,
        package: Option<&str>,
        attribution: Option<&str>,
    ) -> Result<Option<Entry>, Exception> {
        if !self.may_read(uid, package, attribution)? {
            return Ok(None);
        }
        let mut store = self.locked();
        let dir = store.dir.clone();
        let files = match store.init() {
            Ok((files, _)) => files,
            Err(e) => {
                eprintln!("app host: {TAG}: Can't init: {e}");
                return Ok(None);
            }
        };
        let list = match tag {
            None => &files.all,
            Some(t) => match files.by_tag.get(t) {
                Some(list) => list,
                None => return Ok(None),
            },
        };
        for entry in list.contents.iter().filter(|e| e.timestamp > millis) {
            let Some(tag) = entry.tag.clone() else {
                continue;
            };
            if entry.flags & IS_EMPTY != 0 {
                return Ok(Some(Entry::Empty {
                    tag,
                    time: entry.timestamp,
                }));
            }
            let path = entry.file(&dir).unwrap();
            let opened = std::fs::File::open(&path)
                .ok()
                .and_then(|f| aim_binder_host::server::file_from_fd(std::os::fd::AsFd::as_fd(&f)));
            match opened {
                Some(file) => {
                    return Ok(Some(Entry::File {
                        tag,
                        time: entry.timestamp,
                        flags: entry.flags,
                        file,
                    }));
                }
                None => eprintln!("app host: {TAG}: Can't read: {}", path.display()),
            }
        }
        Ok(None)
    }

    /// `addFile`: a regular file's contents, from its offset.
    fn add_file(
        &self,
        tag: Option<&str>,
        fd: Option<ReceivedFd>,
        flags: i32,
    ) -> Result<(), Exception> {
        let file = fd
            .and_then(|fd| self.host.process.file(fd.0))
            .and_then(|f| aim_binder_host::server::file_fd(&f))
            .ok_or_else(|| {
                Exception::new(
                    EX_NULL_POINTER,
                    "Attempt to invoke virtual method 'java.io.FileDescriptor \
                     android.os.ParcelFileDescriptor.getFileDescriptor()' on a null object reference",
                )
            })?;
        let meta = file.metadata().map_err(|e| {
            Exception::illegal_argument(format!("android.system.ErrnoException: fstat failed: {e}"))
        })?;
        if !meta.file_type().is_file() {
            return Err(Exception::illegal_argument(format!(
                "{} entry must be real file",
                tag.unwrap_or("null")
            )));
        }
        let length = meta.size();
        self.add_entry(tag, Source::File(file), length, flags)
    }

    /// `ShellCmd.onCommand`.
    fn shell_command(&self, c: &mut ShellCommand) -> i32 {
        let Some(cmd) = c.command().map(str::to_string) else {
            return default_command(c, None);
        };
        let result: Result<(), String> = (|| {
            match cmd.as_str() {
                "set-rate-limit" => {
                    let arg = c.next_arg_required()?;
                    let period = java_parse_long(&arg).ok_or(format!(
                        "java.lang.NumberFormatException: For input string: \"{arg}\""
                    ))?;
                    self.store.lock().unwrap().low_priority_period = period;
                }
                "add-low-priority" | "remove-low-priority" => {
                    let tag = c.next_arg_required()?;
                    let mut store = self.store.lock().unwrap();
                    let tags = store.low_priority_tags.as_mut().ok_or_else(|| {
                        let method = if cmd == "add-low-priority" { "add" } else { "remove" };
                        format!(
                            "java.lang.NullPointerException: Attempt to invoke virtual method \
                             'boolean android.util.ArraySet.{method}(java.lang.Object)' on a null object reference"
                        )
                    })?;
                    if cmd == "add-low-priority" {
                        tags.insert(tag);
                    } else {
                        tags.remove(&tag);
                    }
                }
                "restore-defaults" => {
                    self.low_priority_resource_configs()
                        .map_err(|e| super::settings_service::java_exception(&e))?;
                }
                _ => return Err(String::new()),
            }
            Ok(())
        })();
        match result {
            Ok(()) => 0,
            Err(e) if e.is_empty() => default_command(c, Some(&cmd)),
            Err(e) => {
                c.println(&e);
                0
            }
        }
    }
}

/// `handleDefaultCommands`.
fn default_command(c: &mut ShellCommand, cmd: Option<&str>) -> i32 {
    match cmd {
        None | Some("help") | Some("-h") => c.print(
            "Dropbox manager service commands:\n  help\n    Print this help text.\n  \
             set-rate-limit PERIOD\n    Sets low priority broadcast rate limit period to PERIOD ms\n  \
             add-low-priority TAG\n    Add TAG to dropbox low priority list\n  \
             remove-low-priority TAG\n    Remove TAG from dropbox low priority list\n  \
             restore-defaults\n    restore dropbox settings to defaults\n",
        ),
        Some(cmd) => c.println(&format!("Unknown command: {cmd}")),
    }
    -1
}

/// `SimpleEntrySource.writeTo`: the contents copied, compressed if forced.
fn write_source(source: Source<'_>, out: std::fs::File, compress: bool) -> std::io::Result<()> {
    let mut input: Box<dyn Read> = match source {
        Source::Data(data) => Box::new(data),
        Source::File(file) => Box::new(file),
    };
    if compress {
        let mut gz = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        std::io::copy(&mut input, &mut gz)?;
        gz.finish()?.flush()
    } else {
        let mut out = out;
        std::io::copy(&mut input, &mut out)?;
        out.flush()
    }
}

/// The global settings' observer: a change rescans the quota and trims.
struct SettingsObserver(Weak<DropBox>);

impl LocalObserver for SettingsObserver {
    fn on_change(&self, _self_change: bool, _uris: &[Uri], _flags: i32, _user: i32) {
        let Some(dropbox) = self.0.upgrade() else {
            return;
        };
        // In the background, as `mReceiver.onReceive` runs them.
        std::thread::spawn(move || {
            let mut store = dropbox.locked();
            store.cached_quota_at = None;
            if let Err(e) = store.trim_to_fit(&*dropbox.host) {
                eprintln!("app host: {TAG}: Can't init: {e}");
            }
        });
    }
}

impl Service for DropBox {
    fn descriptor(&self) -> &str {
        db::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            db::ADD_DATA => {
                let a = db::AddData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match &a.data {
                    None => Err(Exception::new(
                        EX_NULL_POINTER,
                        "Attempt to get length of null array",
                    )),
                    Some(data) => self.add_entry(
                        a.tag.as_deref(),
                        Source::Data(data),
                        data.len() as u64,
                        a.flags,
                    ),
                }
                .map(|()| db::write_add_data_reply(&mut p))
            }
            db::ADD_FILE => {
                let a = db::AddFile::<ReceivedFd>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.add_file(a.tag.as_deref(), a.fd, a.flags)
                    .map(|()| db::write_add_file_reply(&mut p))
            }
            db::IS_TAG_ENABLED => {
                let a = db::IsTagEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.tag_enabled(a.tag.as_deref())
                    .map(|on| db::write_is_tag_enabled_reply(&mut p, on))
            }
            db::GET_NEXT_ENTRY => {
                let a = db::GetNextEntry::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.next_entry(
                    uid,
                    a.tag.as_deref(),
                    a.millis,
                    a.package_name.as_deref(),
                    None,
                )
                .map(|e| db::write_get_next_entry_reply(&mut p, e.as_ref()))
            }
            db::GET_NEXT_ENTRY_WITH_ATTRIBUTION => {
                let a =
                    db::GetNextEntryWithAttribution::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.next_entry(
                    uid,
                    a.tag.as_deref(),
                    a.millis,
                    a.package_name.as_deref(),
                    a.attribution_tag.as_deref(),
                )
                .map(|e| db::write_get_next_entry_with_attribution_reply(&mut p, e.as_ref()))
            }
            SHELL_COMMAND_TRANSACTION => {
                let mut command = ShellCommand::read(&self.host.process, &mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if command.has_output() {
                    let result = self.shell_command(&mut command);
                    command.finish(result);
                }
                Ok(())
            }
            DUMP_TRANSACTION => Err(self.host.unimplemented_named("dropbox", "dump (#1937)")),
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Globals(HashMap<&'static str, &'static str>);

    impl Inputs for Globals {
        fn global(&self, name: &str) -> Option<String> {
            self.0.get(name).map(|v| v.to_string())
        }
        fn low_ram(&self) -> bool {
            false
        }
        fn userdebug(&self) -> bool {
            false
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aim-dropbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `getFilename` and its parse: tags escaped as `Uri.encode` escapes
    /// them, the flags in the extension.
    #[test]
    fn entry_names_round_trip() {
        let e = EntryFile::new(
            1234,
            Some("system_app_crash".into()),
            IS_TEXT | IS_GZIPPED,
            0,
        );
        assert_eq!(
            e.file(Path::new("/d")).unwrap(),
            Path::new("/d/system_app_crash@1234.txt.gz")
        );
        let e = EntryFile::new(5, Some("a b/c:d".into()), 0, 0);
        let name = e.file(Path::new("")).unwrap();
        let name = name.to_str().unwrap();
        assert_eq!(name, "a%20b%2Fc%3Ad@5.dat");
        let parsed = EntryFile::parse(name, 4097, 4096).unwrap();
        assert_eq!(
            (
                parsed.tag.as_deref(),
                parsed.timestamp,
                parsed.flags,
                parsed.blocks
            ),
            (Some("a b/c:d"), 5, 0, 2)
        );
        let lost = EntryFile::parse("x@7.lost", 0, 4096).unwrap();
        assert_eq!((lost.flags, lost.timestamp), (IS_EMPTY, 7));
        assert!(EntryFile::parse("no-at.txt", 0, 4096).is_none());
        assert!(EntryFile::parse("x@12.log", 0, 4096).is_none());
        assert!(EntryFile::parse("x@1 2.txt", 0, 4096).is_none());
    }

    /// `createEntry`: unique, increasing times, a tombstone without a
    /// file, and the scan at `init` enrolling what is there.
    #[test]
    fn entries_get_unique_times_and_are_rescanned() {
        let dir = temp_dir("create");
        let mut store = Store::new(dir.clone());
        store.init().unwrap();
        let temp = dir.join("drop0.tmp");
        std::fs::write(&temp, b"hello").unwrap();
        let t1 = store.create_entry(Some(&temp), "tag", IS_TEXT).unwrap();
        let t2 = store.create_entry(None, "tag", 0).unwrap();
        assert!(t2 > t1);
        assert_eq!(
            names(&dir),
            [format!("tag@{t1}.txt"), format!("tag@{t2}.lost")]
        );
        std::fs::write(dir.join("drop9.tmp"), b"").unwrap();
        std::fs::write(dir.join("garbage"), b"").unwrap();
        let mut again = Store::new(dir.clone());
        let (files, _) = again.init().unwrap();
        assert_eq!(files.all.contents.len(), 2);
        // The tombstone has no blocks, so its tag's list is the text's.
        assert_eq!(files.by_tag["tag"].contents.len(), 1);
        assert_eq!(names(&dir).len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `trimToFit`: entries older than the age and beyond the count go,
    /// and a tag over its fair share of the quota loses its oldest to
    /// tombstones.
    #[test]
    fn trimming_ages_counts_and_squeezes() {
        let dir = temp_dir("trim");
        let mut store = Store::new(dir.clone());
        store.init().unwrap();
        let now = now_millis();
        for t in [now - 10_000_000, now - 2, now - 1] {
            std::fs::write(dir.join(format!("big@{t}.dat")), vec![0u8; 8192]).unwrap();
        }
        std::fs::write(dir.join(format!("small@{now}.dat")), b"x").unwrap();
        let mut store = Store::new(dir.clone());
        let block = {
            let (_, b) = store.init().unwrap();
            b as i64
        };
        // Two blocks of quota at most; an age of 1000 s.
        let kb = (2 * block / 1024).max(1);
        let quota_kb: &'static str = Box::leak(kb.to_string().into_boxed_str());
        let globals = Globals(HashMap::from([
            (DROPBOX_AGE_SECONDS, "1000"),
            (DROPBOX_QUOTA_KB, quota_kb),
        ]));
        store.trim_to_fit(&globals).unwrap();
        let names = names(&dir);
        assert!(
            !names
                .iter()
                .any(|n| n == &format!("big@{}.dat", now - 10_000_000))
        );
        assert!(names.contains(&format!("small@{now}.dat")));
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("big@") && n.ends_with(".lost"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uri_escapes_decode_back() {
        assert_eq!(uri_decode("a%20b%zz%2"), "a b%zz%2");
        assert_eq!(uri_encode("é"), "%C3%A9");
        assert_eq!(uri_decode(&uri_encode("é/x")), "é/x");
    }
}
