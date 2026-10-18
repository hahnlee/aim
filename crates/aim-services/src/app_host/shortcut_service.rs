//! `shortcut`: ShortcutService for the app host (`android-16.0.0_r1`), its
//! publisher reads: the shortcuts ShortcutUser and ShortcutPackage last
//! persisted (`/data/system_ce/<user>/shortcut_service`: `shortcuts.xml`
//! and `packages/<package>.xml`), the limits, and the throttling state
//! (`/data/system/shortcut_service.xml`), each kept as the original keeps
//! it. A state that needs its packages rescanned (a package gone,
//! updated, or new with manifest shortcuts; a new OS build or locale), the
//! writes and the launcher and sharing calls are not implemented (#1669).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aim_android_xml::{Element, Node, TEXT, Value as Xml};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_pm_ishortcutservice as ss;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;
use crate::package::info::array_order;
use crate::package::intent::ComponentName;
use crate::package::intent_uri::FullIntent;
use crate::package::list_slice::Slice;

/// `ShortcutInfo.FLAG_*`.
const FLAG_DYNAMIC: i32 = 1 << 0;
const FLAG_PINNED: i32 = 1 << 1;
const FLAG_MANIFEST: i32 = 1 << 5;
const FLAG_DISABLED: i32 = 1 << 6;
const FLAG_RETURNED_BY_SERVICE: i32 = 1 << 10;
const FLAG_CACHED_ALL: i32 = (1 << 14) | (1 << 29) | (1 << 30);
/// `ShortcutManager.FLAG_MATCH_*`.
const MATCH_MANIFEST: i32 = 1 << 0;
const MATCH_DYNAMIC: i32 = 1 << 1;
const MATCH_PINNED: i32 = 1 << 2;
const MATCH_CACHED: i32 = 1 << 3;
/// `ShortcutInfo.DISABLED_REASON_BY_APP`, `DISABLED_REASON_RESTORE_ISSUE_START`.
const DISABLED_REASON_BY_APP: i32 = 1;
const DISABLED_REASON_RESTORE_ISSUE_START: i32 = 100;
/// `ShortcutService`'s defaults.
const DEFAULT_RESET_INTERVAL_SEC: i64 = 24 * 60 * 60;
const DEFAULT_MAX_UPDATES_PER_INTERVAL: i64 = 10;
const DEFAULT_MAX_SHORTCUTS_PER_ACTIVITY: i64 = 15;
const DEFAULT_MAX_ICON_DIMENSION_DP: i64 = 96;
const DEFAULT_MAX_ICON_DIMENSION_LOWRAM_DP: i64 = 48;
/// `ShortcutParser.METADATA_KEY`.
const SHORTCUTS_METADATA: &str = "android.app.shortcuts";

fn not_modelled(what: &str) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, format!("shortcut: {what} (#1669)"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// `PersistableBundle.restoreFromXml` of `e`'s children: its entries in
/// their `ArrayMap` order.
fn persistable(e: &Element) -> Result<Vec<(String, Value)>, String> {
    let mut entries = Vec::new();
    for c in e.children() {
        let name = c.string("name").map(|s| s.into_owned()).ok_or("an entry without a name")?;
        let value = match c.name.as_str() {
            "string" => Value::String(Some(text(c))),
            "int" => Value::Int(c.int("value")?.ok_or("an int without a value")?),
            "long" => Value::Long(c.long("value")?.ok_or("a long without a value")?),
            "boolean" => Value::Bool(c.bool("value")?.ok_or("a boolean without a value")?),
            "string-array" => Value::StringArray(c.children().filter(|i| i.name == "item").map(|i| i.string("value").map(|s| s.into_owned())).collect()),
            other => return Err(format!("a persistable bundle's <{other}>")),
        };
        entries.retain(|(k, _): &(String, Value)| *k != name);
        entries.push((name, value));
    }
    Ok(array_order(entries, |(k, _)| k.as_str()))
}

/// An element's text.
fn text(e: &Element) -> String {
    e.content.iter().filter_map(|n| match n {
        Node::Token(TEXT, Some(t)) => Some(t.as_str()),
        _ => None,
    }).collect()
}

/// A `ShortcutInfo` as `ShortcutPackage.parseShortcut` reads it.
#[derive(Clone, Debug, PartialEq)]
struct Shortcut {
    id: String,
    activity: Option<ComponentName>,
    title: Option<String>,
    title_res_id: i32,
    text: Option<String>,
    text_res_id: i32,
    disabled_message: Option<String>,
    disabled_message_res_id: i32,
    disabled_reason: i32,
    /// Each intent without its extras, and its extras
    /// (`fixUpIntentExtras`).
    intents: Vec<(FullIntent, Option<Vec<(String, Value)>>)>,
    rank: i32,
    extras: Option<Vec<(String, Value)>>,
    timestamp: i64,
    flags: i32,
    icon_res_id: i32,
    splash_theme: Option<String>,
    locus_id: Option<String>,
    categories: Option<Vec<String>>,
}

impl Shortcut {
    fn parse(e: &Element) -> Result<Shortcut, String> {
        let string = |n: &str| e.string(n).map(|s| s.into_owned());
        // `parseIntAttribute`, `parseLongAttribute`: 0 when absent.
        let long = |n: &str| e.long(n).map(|v| v.unwrap_or(0));
        let int = |n: &str| e.int(n).map(|v| v.unwrap_or(0));
        if string("intent").is_some() {
            return Err("a shortcut of the legacy format".into());
        }
        let mut intents = Vec::new();
        let mut extras = None;
        let mut categories = None;
        for c in e.children() {
            match c.name.as_str() {
                "intent" => {
                    let uri = c.string("intent-base").ok_or("an intent without intent-base")?;
                    let mut intent = FullIntent::parse_uri(&uri, 0)?;
                    let mut bundle = intent.extras.take();
                    if let Some(x) = c.children().find(|x| x.name == "extras") {
                        // `replaceExtras` with the persisted ones.
                        bundle = Some(persistable(x)?);
                    }
                    intents.push((intent, bundle));
                }
                "extras" => extras = Some(persistable(c)?),
                // `<categories>` holds the `XmlUtils` string array the pull
                // parser reads next.
                "categories" => {
                    for array in c.children().filter(|a| a.name == "string-array" && a.string("name").as_deref() == Some("categories")) {
                        let mut items: Vec<String> = Vec::new();
                        for item in array.children().filter(|i| i.name == "item") {
                            let v = item.string("value").map(|s| s.into_owned()).unwrap_or_default();
                            if !items.contains(&v) {
                                items.push(v);
                            }
                        }
                        categories = Some(array_order(items, |s| s.as_str()));
                    }
                }
                other => return Err(format!("a shortcut's <{other}>")),
            }
        }
        let flags = long("flags")? as i32;
        let mut disabled_reason = int("disabled-reason")?;
        if disabled_reason == 0 && flags & FLAG_DISABLED != 0 {
            disabled_reason = DISABLED_REASON_BY_APP;
        }
        Ok(Shortcut {
            id: string("id").ok_or("a shortcut without an id")?,
            activity: string("activity").and_then(|a| crate::package::preferred::unflatten(&a)),
            title: string("title"),
            title_res_id: int("titleid")?,
            text: string("text"),
            text_res_id: int("textid")?,
            disabled_message: string("dmessage"),
            disabled_message_res_id: int("dmessageid")?,
            disabled_reason,
            intents,
            rank: long("rank")? as i32,
            extras,
            timestamp: long("timestamp")?,
            flags,
            icon_res_id: long("icon-res")? as i32,
            splash_theme: string("splash-screen-theme-name"),
            locus_id: string("locus-id"),
            categories,
        })
    }
}

/// `ShortcutInfo.writeToParcel` of a shortcut cloned for its publisher
/// (`CLONE_REMOVE_FOR_CREATOR`: no icon or resource names) and returned
/// by the service.
struct ForCreator<'a>(&'a Shortcut, i32, &'a str);

impl WriteParcelable for ForCreator<'_> {
    fn write_to(&self, p: &mut Parcel) {
        let ForCreator(s, user, package) = self;
        let char_sequence = |p: &mut Parcel, t: &Option<String>| crate::clip::write_char_sequence(p, t.as_deref());
        let persistable = |p: &mut Parcel, b: &Option<Vec<(String, Value)>>| match b {
            Some(entries) => {
                p.write_string16(Some("android.os.PersistableBundle"));
                let entries: Vec<(&str, Value)> = entries.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
                crate::bundle::write(p, &entries);
            }
            None => p.write_string16(None),
        };
        p.write_i32(*user);
        p.write_string8(Some(&s.id));
        p.write_string8(Some(package));
        match &s.activity {
            Some(a) => {
                p.write_string16(Some("android.content.ComponentName"));
                p.write_string16(Some(&a.package));
                p.write_string16(Some(&a.class));
            }
            None => p.write_string16(None),
        }
        p.write_i32(s.flags | FLAG_RETURNED_BY_SERVICE);
        p.write_i32(s.icon_res_id);
        p.write_i64(s.timestamp);
        p.write_i32(s.disabled_reason);
        p.write_i32(1);
        p.write_string16(None); // no icon
        char_sequence(p, &s.title);
        p.write_i32(s.title_res_id);
        char_sequence(p, &s.text);
        p.write_i32(s.text_res_id);
        char_sequence(p, &s.disabled_message);
        p.write_i32(s.disabled_message_res_id);
        p.write_i32(s.intents.len() as i32);
        for (intent, _) in &s.intents {
            p.write_string16(Some("android.content.Intent"));
            intent.write(p);
        }
        p.write_i32(s.intents.len() as i32);
        for (_, extras) in &s.intents {
            persistable(p, extras);
        }
        p.write_i32(s.rank);
        persistable(p, &s.extras);
        p.write_string8(None); // no bitmap path
        for _ in 0..4 {
            p.write_string8(None); // no resource names
        }
        let categories = s.categories.as_deref().unwrap_or_default();
        p.write_i32(categories.len() as i32);
        for c in categories {
            p.write_string8(Some(c));
        }
        p.write_i32(0); // no persons
        match &s.locus_id {
            Some(l) => {
                p.write_string16(Some("android.content.LocusId"));
                p.write_string8(Some(l));
            }
            None => p.write_string16(None),
        }
        p.write_string8(None); // no icon URI
        p.write_string8(s.splash_theme.as_deref());
        p.write_i32(0); // excluded surfaces
        p.write_i32(-1); // no capability bindings
    }
}

/// A `ShortcutPackage`: its document (written back as read, its counters
/// updated), its package info and its shortcuts in their `ArrayMap`
/// order.
struct Package {
    file: PathBuf,
    root: Element,
    version: i64,
    last_update_time: i64,
    shortcuts: Vec<Shortcut>,
}

impl Package {
    fn load(file: PathBuf, root: Element) -> Result<Package, String> {
        if root.name != "package" {
            return Err(format!("<{}>", root.name));
        }
        let info = root.children().find(|c| c.name == "package-info");
        if info.and_then(|i| i.int("shadow").ok().flatten()).unwrap_or(0) != 0 {
            return Err("a shadow package".into());
        }
        let mut shortcuts: Vec<Shortcut> = Vec::new();
        for c in root.children() {
            match c.name.as_str() {
                "package-info" | "share-target" => {}
                "shortcut" => {
                    let s = Shortcut::parse(c)?;
                    shortcuts.retain(|x| x.id != s.id);
                    shortcuts.push(s);
                }
                other => return Err(format!("a package's <{other}>")),
            }
        }
        Ok(Package {
            version: info.and_then(|i| i.long("version").ok().flatten()).unwrap_or(0),
            last_update_time: info.and_then(|i| i.long("last_udpate_time").ok().flatten()).unwrap_or(0),
            shortcuts: array_order(shortcuts, |s| s.id.as_str()),
            file,
            root,
        })
    }

    fn attr_long(&self, name: &str) -> i64 {
        self.root.long(name).ok().flatten().unwrap_or(0)
    }

    fn set_attr_long(&mut self, name: &str, v: i64) {
        self.root.attrs.retain(|(n, _)| n != name);
        self.root.attrs.push((name.into(), Xml::Long(v)));
    }
}

/// A user's shortcuts (`ShortcutUser`).
struct User {
    file: PathBuf,
    root: Element,
    packages: HashMap<String, Package>,
}

/// `ResilientAtomicFile.openRead` of `file`: its reserve copy when it
/// cannot be read.
fn read_resilient(file: &Path) -> Result<Option<Element>, String> {
    let read = |path: &Path| -> Result<Option<Element>, String> {
        match super::device_policy::read_atomic_bytes(path)? {
            Some(b) => aim_android_xml::read(&b).map(Some),
            None => Ok(None),
        }
    };
    match read(file) {
        Ok(e) => Ok(e),
        Err(e) => {
            let mut reserve = file.as_os_str().to_owned();
            reserve.push(".reservecopy");
            eprintln!("app host: shortcut: {}: {e}; reading its reserve copy", file.display());
            read(Path::new(&reserve))
        }
    }
}

/// `ResilientAtomicFile.finishWrite`: the file, then its reserve copy, as
/// the system's.
fn write_resilient(file: &Path, root: &Element) -> std::io::Result<()> {
    let bytes = aim_android_xml::abx::write(root).map_err(std::io::Error::other)?;
    let mut reserve = file.as_os_str().to_owned();
    reserve.push(".reservecopy");
    for path in [file.to_path_buf(), PathBuf::from(reserve)] {
        let mut new = path.as_os_str().to_owned();
        new.push(".new");
        let new = PathBuf::from(new);
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new)?;
        std::io::Write::write_all(&mut f, &bytes)?;
        crate::fsync(&f)?;
        aim_storage::guest_inode::record(&new, aim_storage::guest_inode::GuestInode { uid: Some(1000), gid: Some(1000), mode: Some(0o600) })?;
        std::fs::rename(&new, &path)?;
    }
    Ok(())
}

pub struct ShortcutManager {
    host: Arc<AppHost>,
    /// `mUsers`: user 0, once loaded.
    user: Mutex<Option<User>>,
    /// `mRawLastResetTime`, once loaded.
    last_reset: Mutex<Option<i64>>,
}

impl ShortcutManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        ShortcutManager { host, user: Mutex::new(None), last_reset: Mutex::new(None) }
    }

    /// `KeyValueListParser.getLong` of `shortcut_manager_constants`.
    fn constant(&self, key: &str, default: i64) -> Result<i64, Exception> {
        let config = self.host.inputs()?.settings.get(super::settings::Table::Global, "shortcut_manager_constants");
        Ok(config
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| k.trim() == key)
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(default))
    }

    /// `verifyCaller`.
    fn verify_caller(&self, uid: i32, package: Option<&str>, user: i32) -> Result<String, Exception> {
        let package = package.filter(|p| !p.is_empty()).ok_or_else(|| Exception::illegal_argument("packageName"))?;
        if uid.rem_euclid(PER_USER_RANGE as i32) == 1000 {
            return Ok(package.to_owned());
        }
        if uid / PER_USER_RANGE as i32 != user {
            return Err(Exception::security("Invalid userId"));
        }
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security("Calling package name mismatch"));
        }
        let (state, _) = self.host.packages()?;
        if state.packages.get(package).and_then(|ps| ps.users.get(&user)).is_some_and(|u| u.instant_app) {
            return Err(Exception::new(EX_ILLEGAL_STATE, "Ephemeral apps can't use ShortcutManager"));
        }
        Ok(package.to_owned())
    }

    /// `getUserShortcutsLocked(0)`: the user loaded, then
    /// `checkPackageChanges` (removed packages cleaned up, updated ones
    /// rescanned, the scan time saved).
    fn with_user<T>(&self, f: impl FnOnce(&mut User) -> Result<T, Exception>) -> Result<T, Exception> {
        let mut guard = self.user.lock().unwrap();
        if guard.is_none() {
            *guard = Some(self.load_user()?);
        }
        f(guard.as_mut().unwrap())
    }

    fn load_user(&self) -> Result<User, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, format!("shortcut: {e}"));
        let dir = self.host.data()?.join("system_ce/0/shortcut_service");
        let file = dir.join("shortcuts.xml");
        let root = read_resilient(&file).map_err(state)?.unwrap_or_else(|| Element { name: "user".into(), attrs: Vec::new(), content: Vec::new() });
        if root.name != "user" {
            return Err(state(format!("shortcuts.xml: <{}>", root.name)));
        }
        if root.children().next().is_some() {
            return Err(not_modelled("a user of the shortcuts.xml-only format"));
        }
        let mut packages = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("packages")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
                // `forMainFilesIn`: the files but their reserve copies.
                if !name.ends_with(".xml") {
                    continue;
                }
                let Some(root) = read_resilient(&path).map_err(state)? else { continue };
                let package = Package::load(path.clone(), root).map_err(|e| not_modelled(&format!("{}: {e}", path.display())))?;
                let key = package.root.string("name").map(|s| s.into_owned()).unwrap_or_default();
                packages.insert(key, package);
            }
        }
        let mut user = User { file, root, packages };
        self.check_package_changes(&mut user)?;
        Ok(user)
    }

    /// `checkPackageChanges` and `rescanUpdatedPackagesLocked`.
    fn check_package_changes(&self, user: &mut User) -> Result<(), Exception> {
        let (state, _) = self.host.packages()?;
        for name in user.packages.keys() {
            if !state.packages.get(name).is_some_and(|ps| ps.users.get(&0).is_none_or(|u| u.installed)) {
                return Err(not_modelled("cleaning up a removed package's shortcuts"));
            }
        }
        let fingerprint = self.host.property("ro.build.fingerprint").unwrap_or_default();
        let after_ota = user.root.string("last-app-scan-fp").as_deref() != Some(fingerprint.as_str());
        let now = now_ms();
        let scanned = user.root.long("last-app-scan-time2").ok().flatten().unwrap_or(0);
        // `ShortcutUser.loadFromXml`: a scan time in the future is none.
        let scanned = if scanned < now { scanned } else { 0 };
        let updated = state.packages.values().any(|ps| {
            ps.users.get(&0).is_none_or(|u| u.installed) && ((after_ota && ps.is.system) || ps.last_update_time >= scanned)
        });
        if updated {
            return Err(not_modelled("rescanning an updated package's manifest shortcuts"));
        }
        user.root.attrs.retain(|(n, _)| n != "last-app-scan-time2" && n != "last-app-scan-fp");
        user.root.attrs.push(("last-app-scan-time2".into(), Xml::Long(now)));
        user.root.attrs.push(("last-app-scan-fp".into(), Xml::String(fingerprint)));
        write_resilient(&user.file, &user.root).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", user.file.display())))
    }

    /// `ShortcutUser.onCalledByPublisher`: `detectLocaleChange`, then
    /// `rescanPackageIfNeeded(package, false)`.
    fn on_called_by_publisher(&self, user: &User, package: &str) -> Result<(), Exception> {
        let locales = self.host.inputs()?.locales.clone();
        if user.root.string("locales").is_some_and(|l| !l.is_empty() && *l != locales) {
            return Err(not_modelled("relabelling shortcuts for a new locale"));
        }
        let (state, _) = self.host.packages()?;
        let Some(ps) = state.packages.get(package) else { return Ok(()) };
        let pkg = ps.pkg.as_deref();
        match user.packages.get(package) {
            Some(p) => {
                // `areAllActivitiesStillEnabled`.
                let enabled = p.shortcuts.iter().filter_map(|s| s.activity.as_ref()).all(|a| {
                    pkg.is_some_and(|pkg| pkg.activities.iter().any(|act| act.main.component.name == a.class && act.main.exported && act.main.enabled))
                });
                if p.version != ps.version_code || p.last_update_time != ps.last_update_time || !enabled {
                    return Err(not_modelled("rescanning a package's manifest shortcuts"));
                }
            }
            None => {
                let manifest = pkg.is_some_and(|pkg| {
                    pkg.activities.iter().any(|a| a.main.component.meta_data.as_ref().is_some_and(|m| m.0.iter().any(|(k, _)| k == SHORTCUTS_METADATA)))
                });
                if manifest {
                    return Err(not_modelled("publishing a new package's manifest shortcuts"));
                }
            }
        }
        Ok(())
    }

    /// `updateTimesLocked`: the base state's last reset time, moved on
    /// each interval passed and saved when changed.
    fn last_reset(&self) -> Result<i64, Exception> {
        let interval = self.constant("reset_interval_sec", DEFAULT_RESET_INTERVAL_SEC)?.max(1) * 1000;
        let file = self.host.data()?.join("system/shortcut_service.xml");
        let mut guard = self.last_reset.lock().unwrap();
        let prev = match *guard {
            Some(v) => v,
            None => {
                let root = read_resilient(&file).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
                root.and_then(|r| r.children().find(|c| c.name == "last_reset_time").and_then(|c| c.long("value").ok().flatten())).unwrap_or(0)
            }
        };
        let now = now_ms();
        let mut next = prev;
        if next == 0 || (now < next && now >= 1_420_070_400) {
            next = now;
        } else if next + interval <= now {
            next = (now / interval) * interval + next % interval;
        }
        if Some(next) != *guard && next != prev {
            let value = Element { name: "last_reset_time".into(), attrs: vec![("value".into(), Xml::Long(next))], content: Vec::new() };
            let root = Element { name: "root".into(), attrs: Vec::new(), content: vec![Node::Element(value)] };
            write_resilient(&file, &root).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", file.display())))?;
        }
        *guard = Some(next);
        Ok(next)
    }

    /// `ShortcutPackage.getApiCallCount`: reset while the app is in the
    /// foreground (each app host process is) or since the last reset.
    fn api_call_count(&self, package: &mut Package, uid: i32) -> Result<i64, Exception> {
        let last = self.last_reset()?;
        let mut changed = false;
        if self.host.uid_running(uid) && package.attr_long("call-count") > 0 {
            package.set_attr_long("call-count", 0);
            changed = true;
        }
        if package.attr_long("last-reset") < last {
            if package.attr_long("call-count") != 0 {
                package.set_attr_long("call-count", 0);
            }
            package.set_attr_long("last-reset", last);
            changed = true;
        }
        if changed {
            write_resilient(&package.file, &package.root).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", package.file.display())))?;
        }
        Ok(package.attr_long("call-count"))
    }

    fn transact_inner(&self, call: &mut Call<'_>, p: &mut Parcel) -> Result<Result<(), Exception>, i32> {
        let uid = call.sender_euid as i32;
        Ok(match call.code {
            ss::GET_SHORTCUTS => {
                let a = ss::GetShortcuts::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = self.verify_caller(uid, a.package_name.as_deref(), a.user_id)?;
                    let shortcuts = self.with_user(|user| {
                        self.on_called_by_publisher(user, &package)?;
                        let want = (if a.match_flags & MATCH_DYNAMIC != 0 { FLAG_DYNAMIC } else { 0 })
                            | (if a.match_flags & MATCH_PINNED != 0 { FLAG_PINNED } else { 0 })
                            | (if a.match_flags & MATCH_MANIFEST != 0 { FLAG_MANIFEST } else { 0 })
                            | (if a.match_flags & MATCH_CACHED != 0 { FLAG_CACHED_ALL } else { 0 });
                        Ok(user
                            .packages
                            .get(&package)
                            .map(|p| p.shortcuts.iter().filter(|s| s.disabled_reason < DISABLED_REASON_RESTORE_ISSUE_START && s.flags & want != 0).cloned().collect::<Vec<_>>())
                            .unwrap_or_default())
                    })?;
                    let items: Vec<OwnedForCreator> = shortcuts.into_iter().map(|s| OwnedForCreator(s, a.user_id, package.clone())).collect();
                    ss::write_get_shortcuts_reply(p, Some(&Slice::new(self.host.process.clone(), "android.content.pm.ShortcutInfo", items)));
                    Ok(())
                })()
            }
            ss::GET_MAX_SHORTCUT_COUNT_PER_ACTIVITY => {
                let a = ss::GetMaxShortcutCountPerActivity::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.verify_caller(uid, a.package_name.as_deref(), a.user_id)?;
                    let max = self.constant("max_shortcuts", DEFAULT_MAX_SHORTCUTS_PER_ACTIVITY)?.max(0) as i32;
                    ss::write_get_max_shortcut_count_per_activity_reply(p, max);
                    Ok(())
                })()
            }
            ss::GET_ICON_MAX_DIMENSIONS => {
                let a = ss::GetIconMaxDimensions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.verify_caller(uid, a.package_name.as_deref(), a.user_id)?;
                    let low_ram = self.host.property("ro.config.low_ram").is_some_and(|v| v == "true");
                    let dp = if low_ram {
                        self.constant("max_icon_dimension_dp_lowram", DEFAULT_MAX_ICON_DIMENSION_LOWRAM_DP)?
                    } else {
                        self.constant("max_icon_dimension_dp", DEFAULT_MAX_ICON_DIMENSION_DP)?
                    }
                    .max(1);
                    // `injectDipToPixel`: in the system's display metrics.
                    let density = self.host.display()?.1.density_dpi as f32 / 160.0;
                    ss::write_get_icon_max_dimensions_reply(p, (dp as f32 * density) as i32);
                    Ok(())
                })()
            }
            ss::GET_REMAINING_CALL_COUNT => {
                let a = ss::GetRemainingCallCount::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let package = self.verify_caller(uid, a.package_name.as_deref(), a.user_id)?;
                    let max = self.constant("max_updates_per_interval", DEFAULT_MAX_UPDATES_PER_INTERVAL)?.max(0);
                    let count = self.with_user(|user| {
                        self.on_called_by_publisher(user, &package)?;
                        match user.packages.get_mut(&package) {
                            Some(p) => self.api_call_count(p, uid),
                            None => Err(not_modelled("a package's first shortcut state")),
                        }
                    })?;
                    ss::write_get_remaining_call_count_reply(p, (max - count) as i32);
                    Ok(())
                })()
            }
            ss::GET_RATE_LIMIT_RESET_TIME => {
                let a = ss::GetRateLimitResetTime::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    self.verify_caller(uid, a.package_name.as_deref(), a.user_id)?;
                    let interval = self.constant("reset_interval_sec", DEFAULT_RESET_INTERVAL_SEC)?.max(1) * 1000;
                    ss::write_get_rate_limit_reset_time_reply(p, self.last_reset()? + interval);
                    Ok(())
                })()
            }
            code => {
                let name = ss::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("shortcut", &format!("{name} (#1669)")))
            }
        })
    }
}

/// A shortcut, its user and package, owned for a slice.
struct OwnedForCreator(Shortcut, i32, String);

impl WriteParcelable for OwnedForCreator {
    fn write_to(&self, p: &mut Parcel) {
        ForCreator(&self.0, self.1, &self.2).write_to(p);
    }
}

impl Service for ShortcutManager {
    fn descriptor(&self) -> &str {
        ss::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let result = self.transact_inner(call, &mut p)?;
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest shortcut and its intent's extras, read as
    /// `parseShortcut` reads them.
    #[test]
    fn shortcuts_are_read_as_the_package_reads_them() {
        let xml = br##"<package name="com.example" call-count="3" last-reset="5">
<package-info version="7" last_udpate_time="9" shadow="0" />
<shortcut id="b" activity="com.example/.Main" title="B" titleid="1" textid="0" dmessageid="0" disabled-reason="0" timestamp="11" rank="0" flags="420" icon-res="2">
<intent intent-base="#Intent;action=a;launchFlags=0x1000c000;component=com.example/.A;end"><extras><boolean name="k" value="true" /><int name="n" value="2" /><string name="s">v</string></extras></intent>
<categories><string-array name="categories" num="1"><item value="c" /></string-array></categories>
</shortcut>
<shortcut id="a" activity="com.example/.Main" flags="64" />
<share-target targetClass="com.example.Share"><data mimeType="text/plain" /></share-target>
</package>"##;
        let root = aim_android_xml::read(xml).unwrap();
        let p = Package::load(PathBuf::from("p.xml"), root).unwrap();
        assert_eq!((p.version, p.last_update_time, p.attr_long("call-count")), (7, 9, 3));
        assert_eq!(p.shortcuts.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        let b = &p.shortcuts[1];
        assert_eq!((b.flags, b.icon_res_id, b.timestamp), (420, 2, 11));
        assert_eq!(b.activity, Some(ComponentName { package: "com.example".into(), class: "com.example.Main".into() }));
        assert_eq!(b.intents[0].0.intent.action.as_deref(), Some("a"));
        assert_eq!(b.intents[0].1.as_ref().map(|e| e.len()), Some(3));
        assert_eq!(b.categories, Some(vec!["c".to_string()]));
        // Disabled without a reason: disabled by the app.
        assert_eq!(p.shortcuts[0].disabled_reason, DISABLED_REASON_BY_APP);
    }
}
