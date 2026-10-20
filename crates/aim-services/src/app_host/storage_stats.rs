//! `storagestats`: StorageStatsService for the app host
//! (`android-16.0.0_r1`). Sizes are measured as installd measures them
//! when the volume has no quotas, which the data volume never has (the
//! syscall layer has no `quotactl`): by walking the app's code, its CE and
//! DE data, profiles, external data and OBBs, and the user's directories,
//! counting allocated blocks (`getAppSize`, `getUserSize`,
//! `getExternalSize`). No `StorageStatsAugmenter` is registered in the app
//! host (AppSearch's runs in its own process, #1520). Cache quotas are
//! `super::cache_quota`'s; ART-managed file stats are #1825, crates #1826.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_usage_istoragestatsmanager as ss;

use super::appops_service::{self, MODE_ALLOWED, MODE_DEFAULT, MODE_ERRORED};
use super::cache_quota::CacheQuotas;
use super::storage::{io_exception, parcelable_io};
use super::{AppHost, reply, system_boolean};
use crate::package::info::{ApplicationInfo, FLAG_SYSTEM, FLAG_UPDATED_SYSTEM_APP};
use crate::package::query::Query;

const TAG: &str = "StorageStatsService";
const PER_USER_RANGE: i32 = 100_000;
/// The app host runs user 0 (`UserManager.getUsers`).
const USERS: [i32; 1] = [0];
const PACKAGE_USAGE_STATS: &str = "android.permission.PACKAGE_USAGE_STATS";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
/// `AppOpsManager.OP_GET_USAGE_STATS`.
const OP_GET_USAGE_STATS: i32 = 43;
/// `PackageManager.MATCH_UNINSTALLED_PACKAGES`.
const MATCH_UNINSTALLED_PACKAGES: i64 = 0x2000;
const NAME_NOT_FOUND: &str = "android.content.pm.PackageManager$NameNotFoundException";
const GIGABYTE: i64 = 1_000_000_000;
/// `android.app.usage.Flags`.
const GET_APP_BYTES_BY_DATA_TYPE_API: &str = "android.app.usage.get_app_bytes_by_data_type_api";
const GET_APP_ART_MANAGED_BYTES: &str = "android.app.usage.get_app_art_managed_bytes";
/// `android_filesystem_config.h`.
const AID_APP_START: i32 = 10_000;
const AID_APP_END: i32 = 19_999;
const AID_SHARED_GID_START: i32 = 50_000;
/// installd's `android_profiles_dir`.
const PROFILES: &str = "/data/misc/profiles";

/// `PackageStats` as installd fills it and `translate` reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PackageStats {
    code: i64,
    data: i64,
    cache: i64,
    external_code: i64,
    external_data: i64,
    external_cache: i64,
    apk: i64,
    lib: i64,
    dm: i64,
}

impl PackageStats {
    /// installd's six sizes added in (`Installer.getAppSize`, `getUserSize`).
    fn add(&mut self, (internal, external): (Sizes, Sizes)) {
        self.code += internal.code;
        self.data += internal.data;
        self.cache += internal.cache;
        self.external_code += external.code;
        self.external_data += external.data;
        self.external_cache += external.cache;
    }
}

/// installd's `struct stats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sizes {
    code: i64,
    data: i64,
    cache: i64,
}

/// `StorageStats`, as `translate` makes it from a `PackageStats`.
#[derive(Debug, Default, PartialEq, Eq)]
struct StorageStats {
    package_name: Option<String>,
    user_handle: i32,
    uid: i32,
    code: i64,
    data: i64,
    cache: i64,
    apk: i64,
    lib: i64,
    dm: i64,
    external_cache: i64,
}

impl From<PackageStats> for StorageStats {
    /// `translate`: the user handle is the system's own (`PackageStats(TAG)`).
    fn from(s: PackageStats) -> Self {
        StorageStats {
            code: s.code + s.external_code,
            data: s.data + s.external_data,
            cache: s.cache + s.external_cache,
            apk: s.apk,
            lib: s.lib,
            dm: s.dm,
            external_cache: s.external_cache,
            ..Default::default()
        }
    }
}

impl WriteParcelable for StorageStats {
    /// The dexopt and profile sizes are 0: ART-managed stats are #1825.
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(self.package_name.as_deref());
        p.write_i32(self.user_handle);
        p.write_i32(self.uid);
        for v in [
            self.code,
            self.data,
            self.cache,
            0,
            0,
            0,
            self.apk,
            self.lib,
            self.dm,
            self.external_cache,
        ] {
            p.write_i64(v);
        }
    }
}

/// `ExternalStorageStats`: installd's `getExternalSize`.
#[derive(Debug, Default, PartialEq, Eq)]
struct ExternalStats {
    total: i64,
    audio: i64,
    video: i64,
    image: i64,
    app: i64,
    obb: i64,
}

impl WriteParcelable for ExternalStats {
    fn write_to(&self, p: &mut Parcel) {
        for v in [
            self.total, self.audio, self.video, self.image, self.app, self.obb,
        ] {
            p.write_i64(v);
        }
    }
}

/// `FileUtils.roundStorageSize`.
fn round_storage_size(size: i64) -> i64 {
    let (mut val, mut pow) = (1i64, 1i64);
    while val * pow < size {
        val <<= 1;
        if val > 512 {
            val = 1;
            pow *= 1000;
        }
    }
    val * pow
}

/// `getTotalBytes` of internal storage over 512 GB from its block
/// device's size: rounded only when that is at most 3 GB more ("1 TB" for
/// 997 GB to 999 GB), else as it is.
fn total_from_block_device(total: i64) -> i64 {
    let rounded = round_storage_size(total);
    if rounded - total <= 3 * GIGABYTE {
        rounded
    } else {
        total
    }
}

/// `File.getTotalSpace` of `path`; 0 when it cannot be read.
fn total_space(path: &Path) -> i64 {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return 0;
    };
    // SAFETY: statvfs into a local struct.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    st.f_blocks as i64 * st.f_frsize as i64
}

/// `multiuser_get_shared_gid`.
fn shared_gid(app_id: i32) -> Option<u32> {
    match app_id {
        AID_APP_START..=AID_APP_END => Some((app_id - AID_APP_START + AID_SHARED_GID_START) as u32),
        0..AID_APP_START => Some(app_id as u32),
        _ => None,
    }
}

/// installd's generated `MatchExtension`: the media kind of a file
/// extension, matched without case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Media {
    Audio,
    Video,
    Image,
}

const EXTENSIONS: &[(&str, Media)] = {
    use Media::{Audio as A, Image as I, Video as V};
    &[
        ("3g2", V),
        ("3gp", V),
        ("3gpp", V),
        ("3gpp2", V),
        ("aac", A),
        ("aif", A),
        ("aifc", A),
        ("aiff", A),
        ("amr", A),
        ("art", I),
        ("arw", I),
        ("asf", V),
        ("asx", V),
        ("avi", V),
        ("awb", A),
        ("bmp", I),
        ("cr2", I),
        ("dif", V),
        ("dl", V),
        ("dng", I),
        ("dv", V),
        ("flac", A),
        ("fli", V),
        ("gif", I),
        ("gsm", A),
        ("jng", I),
        ("jpe", I),
        ("jpeg", I),
        ("jpg", I),
        ("lsf", V),
        ("lsx", V),
        ("m3u", A),
        ("m4a", A),
        ("m4v", V),
        ("mka", A),
        ("mkv", V),
        ("mng", V),
        ("mov", V),
        ("movie", V),
        ("mp2", A),
        ("mp3", A),
        ("mp4", V),
        ("mpe", V),
        ("mpeg", V),
        ("mpega", A),
        ("mpg", V),
        ("mpga", A),
        ("mxu", V),
        ("nef", I),
        ("nrw", I),
        ("oga", A),
        ("ogg", A),
        ("orf", I),
        ("pbm", I),
        ("pcx", I),
        ("pef", I),
        ("pgm", I),
        ("pls", A),
        ("png", I),
        ("pnm", I),
        ("ppm", I),
        ("psd", I),
        ("qt", V),
        ("ra", A),
        ("ram", A),
        ("ras", I),
        ("rgb", I),
        ("rm", A),
        ("rw2", I),
        ("sd2", A),
        ("snd", A),
        ("srw", I),
        ("svg", I),
        ("svgz", I),
        ("tif", I),
        ("tiff", I),
        ("ts", V),
        ("vob", V),
        ("wav", A),
        ("wax", A),
        ("wbmp", I),
        ("webm", V),
        ("webp", I),
        ("wm", V),
        ("wma", A),
        ("wmv", V),
        ("wmx", V),
        ("wrf", V),
        ("wvx", V),
        ("xbm", I),
        ("xpm", I),
        ("xwd", I),
    ]
};

fn match_extension(ext: &str) -> Option<Media> {
    EXTENSIONS
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(ext))
        .map(|&(_, m)| m)
}

/// installd's measurements over the guest's files: `/data` on the data
/// volume, any other path in the image. Sizes are allocated blocks
/// (`st_blocks * 512`); group ownership is the guest's
/// (`dev.aim.guest-inode`, root's where none was recorded).
struct Measure {
    data: PathBuf,
    root: PathBuf,
}

impl Measure {
    fn host(&self, guest: &str) -> PathBuf {
        match guest.strip_prefix("/data") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                self.data.join(rest.trim_start_matches('/'))
            }
            _ => self.root.join(guest.trim_start_matches('/')),
        }
    }

    fn guest_gid(path: &Path) -> u32 {
        aim_storage::guest_inode::read(path)
            .ok()
            .flatten()
            .and_then(|g| g.gid)
            .unwrap_or(0)
    }

    /// `calculate_tree_size` (`FTS_PHYSICAL | FTS_XDEV`): every entry of
    /// the tree, the root included, of group `include_gid` if given.
    fn tree_size(&self, guest: &str, include_gid: Option<u32>) -> i64 {
        fn walk(path: &Path, dev: u64, include_gid: Option<u32>, size: &mut i64) {
            let Ok(m) = fs::symlink_metadata(path) else {
                return;
            };
            if include_gid.is_none_or(|gid| Measure::guest_gid(path) == gid) {
                *size += m.blocks() as i64 * 512;
            }
            if m.is_dir() && m.dev() == dev {
                for entry in fs::read_dir(path).into_iter().flatten().flatten() {
                    walk(&entry.path(), dev, include_gid, size);
                }
            }
        }
        let path = self.host(guest);
        let Ok(m) = fs::symlink_metadata(&path) else {
            return 0;
        };
        let mut size = 0;
        walk(&path, m.dev(), include_gid, &mut size);
        size
    }

    /// `collectManualStats`: the directory's own node and everything in it
    /// is data; its `cache` and `code_cache` directories are cache too; a
    /// `lib` symlink is not the app's.
    fn manual_stats(&self, guest: &str, stats: &mut Sizes) {
        let path = self.host(guest);
        let (Ok(own), Ok(entries)) = (fs::metadata(&path), fs::read_dir(&path)) else {
            return;
        };
        stats.data += own.blocks() as i64 * 512;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let size = if kind.is_dir() {
                let size = self.tree_size(&format!("{guest}/{name}"), None);
                if name == "cache" || name == "code_cache" {
                    stats.cache += size;
                }
                size
            } else {
                if kind.is_symlink() && name == "lib" {
                    continue;
                }
                fs::symlink_metadata(entry.path()).map_or(0, |m| m.blocks() as i64 * 512)
            };
            stats.data += size;
        }
    }

    /// The subdirectories of `guest` (`foreach_subdir`).
    fn subdirs(&self, guest: &str) -> Vec<String> {
        let entries = fs::read_dir(self.host(guest))
            .into_iter()
            .flatten()
            .flatten();
        entries
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| format!("{guest}/{}", e.file_name().to_string_lossy()))
            .collect()
    }

    /// `collectManualStatsForUser` without excluding apps: each directory
    /// in it measured, an SDK sandbox's by its subdirectories.
    fn manual_stats_for_user(&self, guest: &str, stats: &mut Sizes, sdk_sandbox: bool) {
        for dir in self.subdirs(guest) {
            if sdk_sandbox {
                self.subdirs(&dir)
                    .iter()
                    .for_each(|sub| self.manual_stats(sub, stats));
            } else {
                self.manual_stats(&dir, stats);
            }
        }
    }

    /// Walks `guest` as installd's `fts` walks of `/data/media/<user>` do:
    /// `visit` sees each entry's names below `guest`, its metadata and
    /// its parent's mark, and returns the mark its children see.
    fn walk_media(&self, guest: &str, visit: &mut dyn FnMut(&[String], &fs::Metadata, u8) -> u8) {
        fn walk(
            path: &Path,
            dev: u64,
            names: &mut Vec<String>,
            parent_mark: u8,
            visit: &mut dyn FnMut(&[String], &fs::Metadata, u8) -> u8,
        ) {
            let Ok(m) = fs::symlink_metadata(path) else {
                return;
            };
            let mark = visit(names, &m, parent_mark);
            if m.is_dir() && m.dev() == dev {
                for entry in fs::read_dir(path).into_iter().flatten().flatten() {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                    walk(&entry.path(), dev, names, mark, visit);
                    names.pop();
                }
            }
        }
        let path = self.host(guest);
        let Ok(m) = fs::symlink_metadata(&path) else {
            return;
        };
        walk(&path, m.dev(), &mut Vec::new(), 0, visit);
    }

    /// `collectManualExternalStatsForUser`: `Android/obb/<package>` is
    /// code, `Android/data/<package>/cache` cache and data, the rest data.
    fn manual_external_stats_for_user(&self, guest: &str, stats: &mut Sizes) {
        self.walk_media(guest, &mut |names, m, parent| {
            let size = m.blocks() as i64 * 512;
            let mark = match names {
                [android, obb, _] if m.is_dir() && obb == "obb" && android == "Android" => 1,
                [android, data, _, cache]
                    if m.is_dir() && cache == "cache" && data == "data" && android == "Android" =>
                {
                    2
                }
                _ => parent,
            };
            match mark {
                1 => stats.code += size,
                2 => {
                    stats.cache += size;
                    stats.data += size;
                }
                _ => stats.data += size,
            }
            mark
        });
    }
}

/// installd's paths of a volume (`create_data_path`); the test volume,
/// which installd only serves debuggable, is `/data/local/tmp`.
fn data_path(uuid: Option<&str>) -> String {
    match uuid {
        None => "/data".into(),
        Some("TEST") => "/data/local/tmp".into(),
        Some(uuid) => format!("/mnt/expand/{uuid}"),
    }
}

/// `checkArgumentUuid`, as `InstallerException` carries it to
/// StorageStatsService, which rethrows it as an `IOException`.
fn check_uuid(uuid: Option<&str>) -> Result<(), Exception> {
    match uuid {
        Some(u) if u.is_empty() || u == "." || u == ".." || u.contains('/') => Err(io_exception(
            &format!("java.lang.IllegalArgumentException: UUID {u} is malformed"),
        )),
        _ => Ok(()),
    }
}

impl Measure {
    /// `create_data_user_ce_path`: user 0's is `/data/data` where that is
    /// a directory.
    fn ce_path(&self, uuid: Option<&str>, user: i32) -> String {
        let data = data_path(uuid);
        if uuid.is_none()
            && user == 0
            && fs::symlink_metadata(self.host("/data/data")).is_ok_and(|m| m.is_dir())
        {
            return format!("{data}/data");
        }
        format!("{data}/user/{user}")
    }

    /// `getAppSize` without quotas: the internal and external sizes.
    fn app_size(
        &self,
        uuid: Option<&str>,
        packages: &[String],
        user: i32,
        app_id: i32,
        code_paths: &[String],
    ) -> (Sizes, Sizes) {
        let (mut stats, mut ext) = (Sizes::default(), Sizes::default());
        let data = data_path(uuid);
        let media = format!("{data}/media/{user}/Android");
        for package in packages {
            ext.code += self.tree_size(&format!("{media}/obb/{package}"), None);
        }
        for code_path in code_paths {
            stats.code += self.tree_size(code_path, None);
        }
        for package in packages {
            self.manual_stats(
                &format!("{}/{package}", self.ce_path(uuid, user)),
                &mut stats,
            );
            self.manual_stats(&format!("{data}/user_de/{user}/{package}"), &mut stats);
            if (AID_APP_START..=AID_APP_END).contains(&app_id) {
                for ce in ["misc_ce", "misc_de"] {
                    for sub in self.subdirs(&format!("{data}/{ce}/{user}/sdksandbox/{package}")) {
                        self.manual_stats(&sub, &mut stats);
                    }
                }
            }
            if uuid.is_none() {
                stats.data += self.tree_size(&format!("{PROFILES}/cur/{user}/{package}"), None);
                stats.code += self.tree_size(&format!("{PROFILES}/ref/{package}"), None);
            }
            self.manual_stats(&format!("{media}/data/{package}"), &mut ext);
            ext.data += self.tree_size(&format!("{media}/media/{package}"), None);
        }
        if uuid.is_none()
            && let Some(gid) = shared_gid(app_id)
        {
            stats.code += self.tree_size("/data/dalvik-cache", Some(gid));
        }
        (stats, ext)
    }

    /// `getUserSize` without quotas.
    fn user_size(&self, uuid: Option<&str>, user: i32) -> (Sizes, Sizes) {
        let (mut stats, mut ext) = (Sizes::default(), Sizes::default());
        let data = data_path(uuid);
        stats.code += self.tree_size(&format!("{data}/app"), None);
        self.manual_stats_for_user(&self.ce_path(uuid, user), &mut stats, false);
        self.manual_stats_for_user(&format!("{data}/user_de/{user}"), &mut stats, false);
        self.manual_stats_for_user(
            &format!("{data}/misc_ce/{user}/sdksandbox"),
            &mut stats,
            true,
        );
        self.manual_stats_for_user(
            &format!("{data}/misc_de/{user}/sdksandbox"),
            &mut stats,
            true,
        );
        if uuid.is_none() {
            stats.data += self.tree_size(&format!("{PROFILES}/cur/{user}"), None);
            stats.code += self.tree_size(&format!("{PROFILES}/ref"), None);
        }
        self.manual_external_stats_for_user(&format!("{data}/media/{user}"), &mut ext);
        if uuid.is_none() {
            // installd measures these twice.
            stats.code += self.tree_size("/data/dalvik-cache", None);
            stats.data += self.tree_size(&format!("{PROFILES}/cur/{user}"), None);
        }
        (stats, ext)
    }

    /// `getExternalSize` without quotas: media files outside `Android` by
    /// kind, what is in `Android` as the apps', OBBs apart from the total.
    fn external_size(&self, uuid: Option<&str>, user: i32) -> ExternalStats {
        let mut s = ExternalStats::default();
        let media = format!("{}/media/{user}", data_path(uuid));
        self.walk_media(&media, &mut |names, m, parent| {
            let size = m.blocks() as i64 * 512;
            if m.is_file() && parent == 0 {
                let name = names.last().map_or("", String::as_str);
                match name
                    .rfind('.')
                    .and_then(|dot| match_extension(&name[dot + 1..]))
                {
                    Some(Media::Audio) => s.audio += size,
                    Some(Media::Video) => s.video += size,
                    Some(Media::Image) => s.image += size,
                    None => {}
                }
            }
            if parent == 1 {
                s.app += size;
            }
            s.total += size;
            if names.len() == 1 && names[0] == "Android" {
                1
            } else {
                parent
            }
        });
        s.obb = self.tree_size(&format!("{media}/Android/obb"), None);
        s.total -= s.obb;
        s
    }

    /// `computeAppStatsByDataTypes` without ART's stats: the APKs and dex
    /// metadata in the source directory and its `lib`, by file length.
    fn by_data_types(&self, stats: &mut PackageStats, source_dir: &str) {
        let mut dir = self.host(source_dir);
        if dir.is_file() {
            dir.pop();
        }
        let files_ending = |suffix: &str| -> i64 {
            fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
                .filter_map(|e| fs::metadata(e.path()).ok().filter(|m| m.is_file()))
                .map(|m| m.len() as i64)
                .sum()
        };
        fn dir_bytes(dir: &Path) -> i64 {
            fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| match fs::metadata(e.path()) {
                    Ok(m) if m.is_file() => m.len() as i64,
                    Ok(m) if m.is_dir() => dir_bytes(&e.path()),
                    _ => 0,
                })
                .sum()
        }
        stats.apk += files_ending(".apk");
        stats.dm += files_ending(".dm");
        stats.lib += dir_bytes(&dir.join("lib"));
    }
}

pub struct StorageStatsService {
    host: Arc<AppHost>,
    cache_quotas: Arc<CacheQuotas>,
}

impl StorageStatsService {
    pub fn new(host: Arc<AppHost>) -> Self {
        let cache_quotas = CacheQuotas::new(&host);
        StorageStatsService { host, cache_quotas }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("storagestats", what)
    }

    fn measure(&self) -> Result<Measure, Exception> {
        Ok(Measure {
            data: self.host.data()?.to_path_buf(),
            root: self.host.root()?.to_path_buf(),
        })
    }

    fn flag(&self, name: &str) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    /// `AppOpsManager.noteOp` or `checkOp` of OP_GET_USAGE_STATS, which
    /// throw when the mode is errored.
    fn usage_stats_mode(
        &self,
        uid: i32,
        package: Option<&str>,
        note: bool,
    ) -> Result<i32, Exception> {
        let mode = if note {
            appops_service::note(&self.host, OP_GET_USAGE_STATS, uid, package)?
        } else {
            appops_service::mode(&self.host, OP_GET_USAGE_STATS, uid, package)?
        };
        if mode == MODE_ERRORED {
            let package = package.unwrap_or("null");
            return Err(Exception::security(format!(
                "{package} from uid {uid} not allowed to perform GET_USAGE_STATS"
            )));
        }
        Ok(mode)
    }

    /// `checkStatsPermission`: why the caller may not see stats, if not.
    fn check_stats_permission(
        &self,
        uid: i32,
        package: Option<&str>,
        note: bool,
    ) -> Result<Option<String>, Exception> {
        let package_text = package.unwrap_or("null");
        Ok(match self.usage_stats_mode(uid, package, note)? {
            MODE_ALLOWED => None,
            MODE_DEFAULT if self.host.check_permission(Some(PACKAGE_USAGE_STATS), uid)? == 0 => {
                None
            }
            MODE_DEFAULT => Some(format!(
                "Caller does not have {PACKAGE_USAGE_STATS}; callingPackage={package_text}, callingUid={uid}"
            )),
            mode => Some(format!(
                "Package {package_text} from UID {uid} blocked by mode {mode}"
            )),
        })
    }

    /// `enforceStatsPermission`.
    fn enforce_stats_permission(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        match self.check_stats_permission(uid, package, true)? {
            Some(message) => Err(Exception::security(message)),
            None => Ok(()),
        }
    }

    /// `enforceCallingOrSelfPermission(INTERACT_ACROSS_USERS)` for a user
    /// that is not the caller's.
    fn enforce_user(&self, uid: i32, user: i32) -> Result<(), Exception> {
        if user != uid.div_euclid(PER_USER_RANGE)
            && self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "{TAG}: Neither user {uid} nor current process has {INTERACT_ACROSS_USERS}."
            )));
        }
        Ok(())
    }

    /// `getApplicationInfoAsUser(MATCH_UNINSTALLED_PACKAGES)` from the
    /// service's own `PackageManager`, which keeps its caller's identity.
    fn application_info(
        &self,
        uid: i32,
        package: &str,
        user: i32,
    ) -> Result<ApplicationInfo, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        match query.application_info(package, MATCH_UNINSTALLED_PACKAGES, user) {
            Err(e) => Err(self.unimplemented(e.0)),
            Ok(Err(e)) => Err(e),
            Ok(Ok(Some(info))) => Ok(info),
            Ok(Ok(None)) => Err(parcelable_io(NAME_NOT_FOUND, package)),
        }
    }

    /// `getPackagesForUid`, none for null (`defeatNullable`).
    fn packages_for_uid(&self, caller: i32, uid: i32) -> Result<Vec<String>, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: caller,
        };
        let names = query
            .packages_for_uid(uid)
            .map_err(|e| self.unimplemented(e.0))?;
        Ok(names.unwrap_or_default().into_iter().flatten().collect())
    }

    /// `ApplicationInfo.getCodePath` unless the code is the system
    /// image's.
    fn counted_code_path(info: &ApplicationInfo) -> Option<String> {
        let baked = info.flags & FLAG_SYSTEM != 0 && info.flags & FLAG_UPDATED_SYSTEM_APP == 0;
        if baked {
            None
        } else {
            info.scan_source_dir.clone()
        }
    }

    fn is_reserved_supported(&self, uuid: Option<&str>) -> bool {
        uuid.is_none() && system_boolean(self.host.property("vold.has_reserved").as_deref(), false)
    }

    /// `getTotalBytes`: internal storage is the data volume and the system
    /// partition rounded (`StorageManager.getPrimaryStorageSize`) up to
    /// 512 GB, above that its block device's size
    /// (`getInternalStorageBlockDeviceSize`); no other volume of the app
    /// host has a UUID.
    fn total_bytes(&self, uuid: Option<&str>) -> Result<i64, Exception> {
        let Some(uuid) = uuid else {
            let rounded = round_storage_size(
                total_space(self.host.data()?) + total_space(&self.host.root()?.join("system")),
            );
            if rounded <= 512 * GIGABYTE {
                return Ok(rounded);
            }
            return Ok(total_from_block_device(
                self.host.internal_storage_block_device_size()?,
            ));
        };
        Err(io_exception(&format!(
            "Failed to find storage device for UUID {uuid}"
        )))
    }

    /// `getFreeBytes`: the usable space, without quotas no cache being
    /// clearable.
    fn free_bytes(&self, uuid: Option<&str>) -> Result<i64, Exception> {
        AppHost::find_path_for_uuid(uuid)?;
        Ok(self.host.data_space()?.1)
    }

    /// `getCacheBytes`: each user's cache.
    fn cache_bytes(
        &self,
        uid: i32,
        uuid: Option<&str>,
        package: Option<&str>,
    ) -> Result<i64, Exception> {
        self.enforce_stats_permission(uid, package)?;
        USERS.iter().try_fold(0, |sum, &user| {
            Ok(sum + self.stats_for_user(uid, uuid, user, None)?.cache)
        })
    }

    /// `queryStatsForPackage`: through its uid when the package has it
    /// alone.
    fn stats_for_package(
        &self,
        uid: i32,
        uuid: Option<&str>,
        package: &str,
        user: i32,
        calling: Option<&str>,
    ) -> Result<StorageStats, Exception> {
        self.enforce_user(uid, user)?;
        let info = self.application_info(uid, package, user)?;
        if uid == info.uid {
            self.check_stats_permission(uid, calling, false)?;
        } else {
            self.enforce_stats_permission(uid, calling)?;
        }
        let mut stats = if self.packages_for_uid(uid, info.uid)?.len() == 1 {
            self.stats_for_uid(uid, uuid, info.uid, calling)?
        } else {
            check_uuid(uuid)?;
            let code_paths: Vec<String> = Self::counted_code_path(&info).into_iter().collect();
            let mut stats = PackageStats::default();
            stats.add(self.measure()?.app_size(
                uuid,
                &[package.to_string()],
                user,
                info.uid.rem_euclid(PER_USER_RANGE),
                &code_paths,
            ));
            StorageStats::from(stats)
        };
        stats.package_name = Some(package.into());
        stats.user_handle = user;
        Ok(stats)
    }

    /// `queryStatsForUid`.
    fn stats_for_uid(
        &self,
        caller: i32,
        uuid: Option<&str>,
        uid: i32,
        calling: Option<&str>,
    ) -> Result<StorageStats, Exception> {
        let (user, app_id) = (
            uid.div_euclid(PER_USER_RANGE),
            uid.rem_euclid(PER_USER_RANGE),
        );
        self.enforce_user(caller, user)?;
        if caller == uid {
            self.check_stats_permission(caller, calling, false)?;
        } else {
            self.enforce_stats_permission(caller, calling)?;
        }
        let packages = self.packages_for_uid(caller, uid)?;
        let measure = self.measure()?;
        let mut stats = PackageStats::default();
        let mut code_paths = Vec::new();
        for package in &packages {
            let info = self.application_info(caller, package, user)?;
            if let Some(code_path) = Self::counted_code_path(&info) {
                code_paths.push(code_path);
                if self.flag(GET_APP_BYTES_BY_DATA_TYPE_API)? {
                    measure
                        .by_data_types(&mut stats, info.source_dir.as_deref().unwrap_or_default());
                    if !self.flag(GET_APP_ART_MANAGED_BYTES)? {
                        return Err(self.unimplemented("ART-managed file stats (#1825)"));
                    }
                }
            }
        }
        check_uuid(uuid)?;
        stats.add(measure.app_size(uuid, &packages, user, app_id, &code_paths));
        Ok(StorageStats {
            user_handle: user,
            uid,
            ..StorageStats::from(stats)
        })
    }

    /// `queryStatsForUser`; the app ids it lists only matter with quotas.
    fn stats_for_user(
        &self,
        uid: i32,
        uuid: Option<&str>,
        user: i32,
        calling: Option<&str>,
    ) -> Result<StorageStats, Exception> {
        self.enforce_user(uid, user)?;
        self.enforce_stats_permission(uid, calling)?;
        check_uuid(uuid)?;
        let mut stats = PackageStats::default();
        stats.add(self.measure()?.user_size(uuid, user));
        Ok(StorageStats::from(stats))
    }

    /// `queryExternalStatsForUser`.
    fn external_stats_for_user(
        &self,
        uid: i32,
        uuid: Option<&str>,
        user: i32,
        calling: Option<&str>,
    ) -> Result<ExternalStats, Exception> {
        self.enforce_user(uid, user)?;
        self.enforce_stats_permission(uid, calling)?;
        check_uuid(uuid)?;
        Ok(self.measure()?.external_size(uuid, user))
    }

    /// `checkCratesEnable`.
    fn crates(&self) -> Exception {
        if system_boolean(self.host.property("fw.storage_crates").as_deref(), false) {
            self.unimplemented("storage crates (#1826)")
        } else {
            Exception::new(EX_ILLEGAL_STATE, "Storage Crate feature is disabled.")
        }
    }
}

impl Service for StorageStatsService {
    fn descriptor(&self) -> &str {
        ss::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            ss::IS_QUOTA_SUPPORTED => {
                ss::IsQuotaSupported::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                ss::write_is_quota_supported_reply(&mut p, false);
                Ok(())
            }
            ss::IS_RESERVED_SUPPORTED => {
                let a = ss::IsReservedSupported::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                ss::write_is_reserved_supported_reply(
                    &mut p,
                    self.is_reserved_supported(a.volume_uuid.as_deref()),
                );
                Ok(())
            }
            ss::GET_TOTAL_BYTES => {
                let a = ss::GetTotalBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.total_bytes(a.volume_uuid.as_deref())
                    .map(|n| ss::write_get_total_bytes_reply(&mut p, n))
            }
            ss::GET_FREE_BYTES => {
                let a = ss::GetFreeBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.free_bytes(a.volume_uuid.as_deref())
                    .map(|n| ss::write_get_free_bytes_reply(&mut p, n))
            }
            ss::GET_CACHE_BYTES => {
                let a = ss::GetCacheBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.cache_bytes(uid, a.volume_uuid.as_deref(), a.calling_package.as_deref())
                    .map(|n| ss::write_get_cache_bytes_reply(&mut p, n))
            }
            ss::GET_CACHE_QUOTA_BYTES => {
                let a = ss::GetCacheQuotaBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_stats_permission(uid, a.calling_package.as_deref())
                    .map(|()| {
                        let quota = self.cache_quotas.quota(a.volume_uuid.as_deref(), a.uid);
                        ss::write_get_cache_quota_bytes_reply(&mut p, quota)
                    })
            }
            ss::QUERY_STATS_FOR_PACKAGE => {
                let a = ss::QueryStatsForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let package = a.package_name.unwrap_or_default();
                self.stats_for_package(
                    uid,
                    a.volume_uuid.as_deref(),
                    &package,
                    a.user_id,
                    a.calling_package.as_deref(),
                )
                .map(|s| ss::write_query_stats_for_package_reply(&mut p, Some(&s)))
            }
            ss::QUERY_STATS_FOR_UID => {
                let a = ss::QueryStatsForUid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.stats_for_uid(
                    uid,
                    a.volume_uuid.as_deref(),
                    a.uid,
                    a.calling_package.as_deref(),
                )
                .map(|s| ss::write_query_stats_for_uid_reply(&mut p, Some(&s)))
            }
            ss::QUERY_STATS_FOR_USER => {
                let a = ss::QueryStatsForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.stats_for_user(
                    uid,
                    a.volume_uuid.as_deref(),
                    a.user_id,
                    a.calling_package.as_deref(),
                )
                .map(|s| ss::write_query_stats_for_user_reply(&mut p, Some(&s)))
            }
            ss::QUERY_EXTERNAL_STATS_FOR_USER => {
                let a =
                    ss::QueryExternalStatsForUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.external_stats_for_user(
                    uid,
                    a.volume_uuid.as_deref(),
                    a.user_id,
                    a.calling_package.as_deref(),
                )
                .map(|s| ss::write_query_external_stats_for_user_reply(&mut p, Some(&s)))
            }
            ss::QUERY_ART_MANAGED_STATS => Err(self.unimplemented("queryArtManagedStats (#1825)")),
            ss::QUERY_CRATES_FOR_PACKAGE | ss::QUERY_CRATES_FOR_UID | ss::QUERY_CRATES_FOR_USER => {
                Err(self.crates())
            }
            code => {
                let name = ss::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.unimplemented(name))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let dir = std::env::temp_dir()
                .join(format!("aim-storage-stats-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("data")).unwrap();
            fs::create_dir_all(dir.join("root")).unwrap();
            Tree(dir)
        }

        fn measure(&self) -> Measure {
            Measure {
                data: self.0.join("data"),
                root: self.0.join("root"),
            }
        }

        /// A file of `len` bytes at guest `/data/<rel>`; its allocated size.
        fn file(&self, rel: &str, len: usize) -> i64 {
            let path = self.0.join("data").join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, vec![7u8; len]).unwrap();
            blocks(&path)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn blocks(path: &Path) -> i64 {
        fs::symlink_metadata(path).unwrap().blocks() as i64 * 512
    }

    #[test]
    fn storage_sizes_round_as_the_original() {
        assert_eq!(round_storage_size(1), 1);
        assert_eq!(round_storage_size(100), 128);
        assert_eq!(round_storage_size(1000), 1000);
        assert_eq!(round_storage_size(30 * GIGABYTE), 32 * GIGABYTE);
        assert_eq!(round_storage_size(500 * GIGABYTE), 512 * GIGABYTE);
        assert_eq!(round_storage_size(513 * GIGABYTE), 1000 * GIGABYTE);
    }

    /// Above 512 GB: a block device just short of a power of two shows as
    /// it, one further below as its own size.
    #[test]
    fn large_internal_storage_rounds_only_within_3_gb() {
        assert_eq!(total_from_block_device(998 * GIGABYTE), 1000 * GIGABYTE);
        assert_eq!(total_from_block_device(1000 * GIGABYTE), 1000 * GIGABYTE);
        assert_eq!(total_from_block_device(994_610_155_520), 994_610_155_520);
        assert_eq!(
            total_from_block_device(1_000_555_581_440),
            1_000_555_581_440
        );
    }

    #[test]
    fn extensions_match_their_media_kind_without_case() {
        assert_eq!(match_extension("JPG"), Some(Media::Image));
        assert_eq!(match_extension("mp3"), Some(Media::Audio));
        assert_eq!(match_extension("3gpp2"), Some(Media::Video));
        assert_eq!(match_extension("txt"), None);
        assert_eq!(match_extension(""), None);
        assert!(EXTENSIONS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn shared_gids_follow_libcutils() {
        assert_eq!(shared_gid(10_123), Some(50_123));
        assert_eq!(shared_gid(1000), Some(1000));
        assert_eq!(shared_gid(90_000), None);
    }

    #[test]
    fn malformed_uuids_are_refused_as_installd_refuses_them() {
        assert!(check_uuid(None).is_ok() && check_uuid(Some("1234-ABCD")).is_ok());
        let e = check_uuid(Some("../x")).unwrap_err();
        assert_eq!(
            e.message,
            "java.io.IOException: java.lang.IllegalArgumentException: UUID ../x is malformed"
        );
    }

    /// An app's data is its CE and DE directories, their caches counted
    /// as cache too; its external data and OBB count as external.
    #[test]
    fn an_apps_size_is_measured_as_installd_measures_it() {
        let t = Tree::new("app");
        let m = t.measure();
        fs::create_dir_all(t.0.join("data/data")).unwrap();
        let code = t.file("app/~~x/p-y/base.apk", 9000);
        let ce = t.file("data/p/files/a", 5000);
        let cache = t.file("data/p/cache/c", 3000);
        let de = t.file("user_de/0/p/d", 100);
        let ext = t.file("media/0/Android/data/p/files/e", 2000);
        let ext_cache = t.file("media/0/Android/data/p/cache/x", 700);
        let obb = t.file("media/0/Android/obb/p/main.obb", 4000);
        let dir = |rel: &str| blocks(&t.0.join("data").join(rel));
        let (stats, external) = m.app_size(
            None,
            &["p".into()],
            0,
            10_050,
            &["/data/app/~~x/p-y".into()],
        );
        assert_eq!(stats.code, code + dir("app/~~x/p-y"));
        let ce_dirs = dir("data/p") + dir("data/p/files") + dir("data/p/cache");
        assert_eq!(stats.data, ce + cache + ce_dirs + de + dir("user_de/0/p"));
        assert_eq!(stats.cache, cache + dir("data/p/cache"));
        assert_eq!(external.code, obb + dir("media/0/Android/obb/p"));
        let ext_dirs = dir("media/0/Android/data/p")
            + dir("media/0/Android/data/p/files")
            + dir("media/0/Android/data/p/cache");
        assert_eq!(external.data, ext + ext_cache + ext_dirs);
        assert_eq!(
            external.cache,
            ext_cache + dir("media/0/Android/data/p/cache")
        );
    }

    /// External storage: media files outside `Android` by kind, the apps'
    /// directories apart, OBBs out of the total.
    #[test]
    fn external_storage_is_sorted_by_kind() {
        let t = Tree::new("external");
        let m = t.measure();
        let photo = t.file("media/0/DCIM/a.JPG", 6000);
        let song = t.file("media/0/Music/b.mp3", 3000);
        let note = t.file("media/0/Documents/c.txt", 100);
        let app = t.file("media/0/Android/data/p/f.png", 2000);
        let obb = t.file("media/0/Android/obb/p/main.obb", 4000);
        let s = m.external_size(None, 0);
        assert_eq!((s.image, s.audio, s.video), (photo, song, 0));
        let dir = |rel: &str| blocks(&t.0.join("data/media/0").join(rel));
        let android = [
            "Android/data",
            "Android/data/p",
            "Android/obb",
            "Android/obb/p",
        ]
        .iter()
        .map(|d| dir(d))
        .sum::<i64>();
        assert_eq!(s.app, app + obb + android);
        assert_eq!(s.obb, obb + dir("Android/obb") + dir("Android/obb/p"));
        let all = photo
            + song
            + note
            + app
            + obb
            + android
            + ["", "DCIM", "Music", "Documents", "Android"]
                .iter()
                .map(|d| dir(d))
                .sum::<i64>();
        assert_eq!(s.total, all - s.obb);
    }

    /// The dalvik cache counts toward an app's code only where its guest
    /// group is the app's shared group.
    #[test]
    fn the_dalvik_cache_counts_by_guest_group() {
        let t = Tree::new("dalvik");
        let m = t.measure();
        let mine = t.file("dalvik-cache/arm64/mine.art", 4000);
        t.file("dalvik-cache/arm64/other.art", 4000);
        let path = t.0.join("data/dalvik-cache/arm64/mine.art");
        let attr = aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(50_050),
            mode: Some(0o644),
        };
        aim_storage::guest_inode::record(&path, attr).unwrap();
        assert_eq!(m.tree_size("/data/dalvik-cache", Some(50_050)), mine);
    }
}
