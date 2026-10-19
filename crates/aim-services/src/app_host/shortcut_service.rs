//! `shortcut`: ShortcutService for the app host (`android-16.0.0_r1`), its
//! publisher reads: the shortcuts ShortcutUser and ShortcutPackage last
//! persisted (`/data/system_ce/<user>/shortcut_service`: `shortcuts.xml`
//! and `packages/<package>.xml`), the limits, and the throttling state
//! (`/data/system/shortcut_service.xml`), each kept as the original keeps
//! it; and the dynamic shortcut writes (set, add, remove, disable and
//! enable), saved as `ShortcutPackage.saveToXml` saves them. A state that needs its packages
//! rescanned (a package gone, updated, or new with manifest shortcuts; a
//! new OS build or locale), a package's first shortcuts, bitmap and URI
//! icons, persons and capability bindings, the other writes and the
//! launcher and sharing calls are not implemented (#1669).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aim_android_xml::{Element, Node, TEXT, Value as Xml};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_pm_ishortcutservice as ss;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::bundle::Value;
use crate::package::info::array_order;
use crate::package::intent::ComponentName;
use crate::package::intent_uri::FullIntent;
use crate::package::list_slice::Slice;
use crate::statusbar::parcels::Text;

/// `RoleManager.ROLE_HOME`; `LauncherApps.PinItemRequest.REQUEST_TYPE_SHORTCUT`,
/// `ACTION_CONFIRM_PIN_SHORTCUT`, `ACTION_CONFIRM_PIN_APPWIDGET`.
const ROLE_HOME: &str = "android.app.role.HOME";
const REQUEST_TYPE_SHORTCUT: i32 = 1;
const ACTION_CONFIRM_PIN_SHORTCUT: &str = "android.content.pm.action.CONFIRM_PIN_SHORTCUT";
const ACTION_CONFIRM_PIN_APPWIDGET: &str = "android.content.pm.action.CONFIRM_PIN_APPWIDGET";
/// `ShortcutService.PACKAGE_MATCH_FLAGS` (direct boot aware and unaware,
/// uninstalled packages) and `MATCH_DISABLED_COMPONENTS`; `SYSTEM_APP_MASK`,
/// `ApplicationInfo.FLAG_INSTALLED`; `COMPONENT_ENABLED_STATE_*`.
const PACKAGE_MATCH_FLAGS: i64 = 0x0004_0000 | 0x0008_0000 | 0x2000;
const MATCH_DISABLED_COMPONENTS: i64 = 0x200;
const SYSTEM_APP_MASK: i32 = 0x1 | 0x80;
const FLAG_INSTALLED: i32 = 0x0080_0000;
const COMPONENT_ENABLED_STATE_DEFAULT: i32 = 0;
const COMPONENT_ENABLED_STATE_ENABLED: i32 = 1;

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
    /// The resource names of the title, text, disabled message and icon.
    title_res_name: Option<String>,
    text_res_name: Option<String>,
    disabled_message_res_name: Option<String>,
    icon_res_name: Option<String>,
    bitmap_path: Option<String>,
    icon_uri: Option<String>,
    /// `mExcludedSurfaces`.
    excluded_surfaces: i32,
    /// `mImplicitRank`, which is not persisted.
    implicit_rank: i32,
    /// The element it was read from, written back while it is unchanged.
    element: Option<Element>,
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
            title_res_name: string("titlename"),
            text_res_name: string("textname"),
            disabled_message_res_name: string("dmessagename"),
            icon_res_name: string("icon-resname"),
            bitmap_path: string("bitmap-path"),
            icon_uri: string("icon-uri"),
            excluded_surfaces: 0,
            implicit_rank: 0,
            element: Some(e.clone()),
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

/// `ShortcutInfo` flags and constants the writes use.
const FLAG_HAS_ICON_RES: i32 = 1 << 2;
const FLAG_HAS_ICON_FILE: i32 = 1 << 3;
const FLAG_STRINGS_RESOLVED: i32 = 1 << 7;
const FLAG_IMMUTABLE: i32 = 1 << 8;
const FLAG_ADAPTIVE_BITMAP: i32 = 1 << 9;
const FLAG_LONG_LIVED: i32 = 1 << 13;
const FLAG_HAS_ICON_URI: i32 = 1 << 15;
const RANK_CHANGED_BIT: i32 = i32::MIN;
const IMPLICIT_RANK_MASK: i32 = i32::MAX;
const SURFACE_LAUNCHER: i32 = 1;
const MAX_ID_LENGTH: usize = 1000;
/// `Icon.TYPE_*`.
const TYPE_BITMAP: i32 = 1;
const TYPE_RESOURCE: i32 = 2;
const TYPE_URI: i32 = 4;
const TYPE_ADAPTIVE_BITMAP: i32 = 5;
const TYPE_URI_ADAPTIVE_BITMAP: i32 = 6;
/// `ShortcutService.DUMMY_MAIN_ACTIVITY`, `DEFAULT_MAX_SHORTCUTS_PER_APP`.
const DUMMY_MAIN_ACTIVITY: &str = "android.__dummy__";
const DEFAULT_MAX_SHORTCUTS_PER_APP: i64 = 100;
const UNLIMITED_SHORTCUTS_API_CALLS: &str = "android.permission.UNLIMITED_SHORTCUTS_API_CALLS";
const ACTION_MAIN: &str = "android.intent.action.MAIN";
const CATEGORY_LAUNCHER: &str = "android.intent.category.LAUNCHER";

/// `ShortcutService.OPERATION_SET`, `OPERATION_ADD`.
#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Set,
    Add,
}

/// No file of a parcel is read: a bitmap icon in shared memory is not
/// kept (bitmap icons are not saved, #1669).
struct NoFiles;

impl crate::notifications::parcels::Files for NoFiles {
    fn read(&self, _: u32, _: usize) -> Option<Vec<u8>> {
        None
    }
}

/// A `ShortcutInfo` an app passed (`ShortcutInfo(Parcel)`), and what of
/// it the app host does not keep.
struct Incoming {
    shortcut: Shortcut,
    user: i32,
    package: Option<String>,
    /// Key fields only (`FLAG_KEY_FIELDS_ONLY`'s parcel).
    key_only: bool,
    /// The icon: its type, itself, and whether it has a tint.
    icon: Option<(i32, crate::notifications::parcels::Icon, bool)>,
    /// Intents (each without extras), when set.
    has_intents: bool,
    /// Persons or capability bindings, which are not saved (#1669).
    unsupported: Option<&'static str>,
}

/// A `PersistableBundle` parcel (`readParcelable`): its entries in their
/// `ArrayMap` order, of the types `saveToXml` writes here.
fn persistable_parcel(r: &mut aim_binder_host::parcel::Reader<'_>, unsupported: &mut Option<&'static str>) -> aim_binder_host::parcel::Result<Option<Vec<(String, Value)>>> {
    if r.read_string16()?.is_none() {
        return Ok(None);
    }
    let Some(map) = crate::bundle::read(r)? else { return Ok(None) };
    let mut entries = Vec::new();
    for (k, v) in map {
        match v {
            Value::String(_) | Value::Int(_) | Value::Long(_) | Value::Bool(_) | Value::StringArray(_) => entries.push((k, v)),
            _ => *unsupported = Some("a persistable bundle value of another type"),
        }
    }
    Ok(Some(array_order(entries, |(k, _)| k.as_str())))
}

impl aim_service_aidl::ReadParcelable for Incoming {
    fn read_from(r: &mut aim_binder_host::parcel::Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        use aim_binder_host::parcel::BAD_VALUE;
        let user = r.read_i32()?;
        let mut id = r.read_string8()?.filter(|i| !i.is_empty()).ok_or(BAD_VALUE)?;
        // `getSafeId`.
        if id.chars().count() > MAX_ID_LENGTH {
            id = id.chars().take(MAX_ID_LENGTH).collect();
        }
        let package = r.read_string8()?;
        let activity = match r.read_string16()? {
            Some(_) => Some(ComponentName { package: r.read_string16()?.unwrap_or_default(), class: r.read_string16()?.unwrap_or_default() }),
            None => None,
        };
        let mut shortcut = Shortcut {
            id,
            activity,
            title: None,
            title_res_id: 0,
            text: None,
            text_res_id: 0,
            disabled_message: None,
            disabled_message_res_id: 0,
            disabled_reason: 0,
            intents: Vec::new(),
            rank: 0,
            extras: None,
            timestamp: 0,
            flags: r.read_i32()?,
            icon_res_id: r.read_i32()?,
            splash_theme: None,
            locus_id: None,
            categories: None,
            title_res_name: None,
            text_res_name: None,
            disabled_message_res_name: None,
            icon_res_name: None,
            bitmap_path: None,
            icon_uri: None,
            excluded_surfaces: 0,
            implicit_rank: 0,
            element: None,
        };
        shortcut.timestamp = r.read_i64()?;
        shortcut.disabled_reason = r.read_i32()?;
        let mut incoming = Incoming { shortcut, user, package, key_only: true, icon: None, has_intents: false, unsupported: None };
        if r.read_i32()? == 0 {
            return Ok(incoming);
        }
        incoming.key_only = false;
        let s = &mut incoming.shortcut;
        if r.read_string16()?.is_some() {
            incoming.icon = Some(crate::notifications::parcels::icon_typed(r, &NoFiles)?);
        }
        s.title = crate::clip::char_sequence(r)?;
        s.title_res_id = r.read_i32()?;
        s.text = crate::clip::char_sequence(r)?;
        s.text_res_id = r.read_i32()?;
        s.disabled_message = crate::clip::char_sequence(r)?;
        s.disabled_message_res_id = r.read_i32()?;
        let n = r.read_i32()?;
        let mut intents = Vec::new();
        if n >= 0 {
            incoming.has_intents = true;
            for _ in 0..n {
                if r.read_string16()?.is_some() {
                    let raw = <super::kept::RawIntent as aim_service_aidl::ReadParcelable>::read_from(r)?;
                    intents.push(FullIntent { intent: raw.intent, source_bounds: None, extras: None });
                }
            }
        }
        let n = r.read_i32()?;
        let mut extras = Vec::new();
        for _ in 0..n.max(0) {
            extras.push(persistable_parcel(r, &mut incoming.unsupported)?);
        }
        extras.resize(intents.len(), None);
        s.intents = intents.into_iter().zip(extras).collect();
        s.rank = r.read_i32()?;
        s.extras = persistable_parcel(r, &mut incoming.unsupported)?;
        s.bitmap_path = r.read_string8()?;
        s.icon_res_name = r.read_string8()?;
        s.title_res_name = r.read_string8()?;
        s.text_res_name = r.read_string8()?;
        s.disabled_message_res_name = r.read_string8()?;
        let n = r.read_i32()?;
        if n > 0 {
            let mut categories: Vec<String> = Vec::new();
            for _ in 0..n {
                let c = r.read_string8()?.unwrap_or_default();
                if !categories.contains(&c) {
                    categories.push(c);
                }
            }
            s.categories = Some(array_order(categories, |c| c.as_str()));
        }
        // `Person(Parcel)`: name, icon, uri, key, important, bot.
        for _ in 0..r.read_i32()?.max(0) {
            if r.read_string16()?.is_some() {
                crate::clip::char_sequence(r)?;
                if r.read_i32()? != 0 {
                    crate::notifications::parcels::icon(r, &NoFiles)?;
                }
                r.read_string16()?;
                r.read_string16()?;
                r.read_bool()?;
                r.read_bool()?;
                incoming.unsupported = Some("a shortcut's persons");
            }
        }
        if r.read_string16()?.is_some() {
            s.locus_id = r.read_string16()?;
        }
        s.icon_uri = r.read_string8()?;
        s.splash_theme = r.read_string8()?;
        s.excluded_surfaces = r.read_i32()?;
        // `readHashMap` of the capability bindings.
        let n = r.read_i32()?;
        for _ in 0..n.max(0) {
            crate::notifications::parcels::skip_value(r)?;
            crate::notifications::parcels::skip_value(r)?;
            incoming.unsupported = Some("a shortcut's capability bindings");
        }
        Ok(incoming)
    }
}

/// `writeAttr` of a string: none when empty.
fn attr(attrs: &mut Vec<(String, Xml)>, name: &str, value: Option<&str>) {
    if let Some(v) = value.filter(|v| !v.is_empty()) {
        attrs.push((name.into(), Xml::String(v.into())));
    }
}

/// `PersistableBundle.saveToXml` (`XmlUtils.writeMapXml`) of `entries`.
fn persistable_xml(entries: &[(String, Value)]) -> Vec<Node> {
    let element = |name: &str, key: &str, attrs: Vec<(String, Xml)>, content: Vec<Node>| {
        let mut all = vec![("name".to_string(), Xml::String(key.into()))];
        all.extend(attrs);
        Node::Element(Element { name: name.into(), attrs: all, content })
    };
    entries
        .iter()
        .filter_map(|(k, v)| {
            Some(match v {
                Value::String(Some(s)) => element("string", k, Vec::new(), vec![Node::Token(TEXT, Some(s.clone()))]),
                Value::String(None) => Node::Element(Element { name: "null".into(), attrs: vec![("name".into(), Xml::String(k.clone()))], content: Vec::new() }),
                Value::Int(i) => element("int", k, vec![("value".into(), Xml::Int(*i))], Vec::new()),
                Value::Long(l) => element("long", k, vec![("value".into(), Xml::Long(*l))], Vec::new()),
                Value::Bool(b) => element("boolean", k, vec![("value".into(), Xml::Bool(*b))], Vec::new()),
                Value::StringArray(items) => element(
                    "string-array",
                    k,
                    vec![("num".into(), Xml::Int(items.len() as i32))],
                    items
                        .iter()
                        .map(|i| match i {
                            Some(v) => Node::Element(Element { name: "item".into(), attrs: vec![("value".into(), Xml::String(v.clone()))], content: Vec::new() }),
                            None => Node::Element(Element { name: "null".into(), attrs: Vec::new(), content: Vec::new() }),
                        })
                        .collect(),
                ),
                _ => return None,
            })
        })
        .collect()
}

impl Shortcut {
    /// `ShortcutPackage.saveShortcut`, not for backup.
    fn to_xml(&self) -> Element {
        let mut attrs = Vec::new();
        let string = |attrs: &mut Vec<(String, Xml)>, n: &str, v: Option<&str>| attr(attrs, n, v);
        let long = |attrs: &mut Vec<(String, Xml)>, n: &str, v: i64| attr(attrs, n, Some(&v.to_string()));
        string(&mut attrs, "id", Some(&self.id));
        let activity = self.activity.as_ref().map(|a| format!("{}/{}", a.package, a.class));
        string(&mut attrs, "activity", activity.as_deref());
        string(&mut attrs, "title", self.title.as_deref());
        long(&mut attrs, "titleid", self.title_res_id.into());
        string(&mut attrs, "titlename", self.title_res_name.as_deref());
        string(&mut attrs, "splash-screen-theme-name", self.splash_theme.as_deref());
        string(&mut attrs, "text", self.text.as_deref());
        long(&mut attrs, "textid", self.text_res_id.into());
        string(&mut attrs, "textname", self.text_res_name.as_deref());
        string(&mut attrs, "dmessage", self.disabled_message.as_deref());
        long(&mut attrs, "dmessageid", self.disabled_message_res_id.into());
        string(&mut attrs, "dmessagename", self.disabled_message_res_name.as_deref());
        long(&mut attrs, "disabled-reason", self.disabled_reason.into());
        long(&mut attrs, "timestamp", self.timestamp);
        string(&mut attrs, "locus-id", self.locus_id.as_deref());
        long(&mut attrs, "rank", self.rank.into());
        long(&mut attrs, "flags", self.flags.into());
        long(&mut attrs, "icon-res", self.icon_res_id.into());
        string(&mut attrs, "icon-resname", self.icon_res_name.as_deref());
        string(&mut attrs, "bitmap-path", self.bitmap_path.as_deref());
        string(&mut attrs, "icon-uri", self.icon_uri.as_deref());
        let mut content = Vec::new();
        if let Some(categories) = self.categories.as_deref().filter(|c| !c.is_empty()) {
            let items = categories
                .iter()
                .map(|c| Node::Element(Element { name: "item".into(), attrs: vec![("value".into(), Xml::String(c.clone()))], content: Vec::new() }))
                .collect();
            let array = Element {
                name: "string-array".into(),
                attrs: vec![("name".into(), Xml::String("categories".into())), ("num".into(), Xml::Int(categories.len() as i32))],
                content: items,
            };
            content.push(Node::Element(Element { name: "categories".into(), attrs: Vec::new(), content: vec![Node::Element(array)] }));
        }
        for (intent, extras) in &self.intents {
            let mut e = Element { name: "intent".into(), attrs: Vec::new(), content: Vec::new() };
            attr(&mut e.attrs, "intent-base", Some(&intent.to_uri()));
            if let Some(extras) = extras {
                e.content.push(Node::Element(Element { name: "extras".into(), attrs: Vec::new(), content: persistable_xml(extras) }));
            }
            content.push(Node::Element(e));
        }
        if let Some(extras) = &self.extras {
            content.push(Node::Element(Element { name: "extras".into(), attrs: Vec::new(), content: persistable_xml(extras) }));
        }
        Element { name: "shortcut".into(), attrs, content }
    }

    fn is_floating(&self) -> bool {
        (self.flags & (FLAG_PINNED | FLAG_CACHED_ALL) != 0) && self.flags & (FLAG_DYNAMIC | FLAG_MANIFEST) == 0
    }

    fn is_visible_to_publisher(&self) -> bool {
        self.disabled_reason < DISABLED_REASON_RESTORE_ISSUE_START
    }

    fn is_immutable(&self) -> bool {
        self.flags & FLAG_IMMUTABLE != 0
    }

    fn is_alive(&self) -> bool {
        self.flags & (FLAG_PINNED | FLAG_DYNAMIC | FLAG_MANIFEST | FLAG_CACHED_ALL) != 0
    }

    fn rank_changed(&self) -> bool {
        self.implicit_rank & RANK_CHANGED_BIT != 0
    }

    /// Changed, so written anew.
    fn touched(&mut self) {
        self.element = None;
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
        self.root.attrs.push((name.into(), Xml::String(v.to_string())));
    }

    fn find(&self, id: &str) -> Option<&Shortcut> {
        self.shortcuts.iter().find(|s| s.id == id)
    }

    /// `mShortcuts.put`: in its `ArrayMap` place.
    fn put(&mut self, shortcut: Shortcut) {
        self.shortcuts.retain(|s| s.id != shortcut.id);
        self.shortcuts.push(shortcut);
        self.shortcuts = array_order(std::mem::take(&mut self.shortcuts), |s| s.id.as_str());
    }

    /// `ShortcutPackage.saveToXml`, not for backup, to its file: none
    /// written when it has nothing (`hasNoShortcut`, no share target, no
    /// call made). Its package info and share targets are written as read.
    fn save(&mut self) -> Result<(), Exception> {
        let kept = |name: &str| self.root.children().filter(|c| c.name == name).cloned().map(Node::Element).collect::<Vec<_>>();
        let share_targets = kept("share-target");
        let mut content = kept("package-info");
        content.extend(self.shortcuts.iter().map(|s| Node::Element(s.element.clone().unwrap_or_else(|| s.to_xml()))));
        content.extend(share_targets.iter().cloned());
        let mut attrs = Vec::new();
        attr(&mut attrs, "name", self.root.string("name").as_deref());
        for name in ["call-count", "last-reset"] {
            attr(&mut attrs, name, Some(&self.attr_long(name).to_string()));
        }
        self.root = Element { name: "package".into(), attrs, content };
        let roots = if self.shortcuts.is_empty() && share_targets.is_empty() && self.attr_long("call-count") == 0 { &[][..] } else { std::slice::from_ref(&self.root) };
        write_resilient(&self.file, roots).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", self.file.display())))
    }

    /// `forceDeleteShortcutInner`: its bitmap file left for the dangling
    /// files' cleanup (`ShortcutBitmapSaver.removeIcon`).
    fn force_delete(&mut self, id: &str) -> Option<Shortcut> {
        let at = self.shortcuts.iter().position(|s| s.id == id)?;
        Some(self.shortcuts.remove(at))
    }

    /// `removeOrphans`.
    fn remove_orphans(&mut self) {
        self.shortcuts.retain(Shortcut::is_alive);
    }

    /// `deleteAllDynamicShortcuts`.
    fn delete_all_dynamic(&mut self) {
        let now = now_ms();
        let mut changed = false;
        for s in self.shortcuts.iter_mut().filter(|s| s.flags & FLAG_DYNAMIC != 0 && s.is_visible_to_publisher()) {
            changed = true;
            s.timestamp = now;
            s.flags &= !FLAG_DYNAMIC;
            s.rank = 0;
            s.touched();
        }
        if changed {
            self.remove_orphans();
        }
    }

    /// `ensureNotImmutable` of the shortcut `id`, if any, ignoring
    /// invisible ones.
    fn ensure_not_immutable(&self, id: &str) -> Result<(), Exception> {
        match self.find(id) {
            Some(s) if s.is_immutable() && s.is_visible_to_publisher() => Err(Exception::illegal_argument(format!("Manifest shortcut ID={id} may not be manipulated via APIs"))),
            _ => Ok(()),
        }
    }

    /// `deleteOrDisableWithId(id, disable, false, true, reason)`, the
    /// disabled reason given to disable: the shortcut deleted, if it was
    /// not kept pinned or cached.
    fn delete_or_disable(&mut self, id: &str, disable: Option<i32>, dummy_main: impl Fn(&ComponentName) -> bool) -> Result<Option<Shortcut>, Exception> {
        let Some(old) = self.find(id) else { return Ok(None) };
        if old.flags & FLAG_DISABLED != 0 && !old.is_visible_to_publisher() {
            return Ok(None);
        }
        self.ensure_not_immutable(id)?;
        if old.flags & (FLAG_PINNED | FLAG_CACHED_ALL) != 0 {
            let s = self.shortcuts.iter_mut().find(|s| s.id == id).unwrap();
            s.rank = 0;
            s.flags &= !(FLAG_DYNAMIC | FLAG_MANIFEST);
            if let Some(reason) = disable {
                s.flags |= FLAG_DISABLED;
                if s.disabled_reason == 0 {
                    s.disabled_reason = reason;
                }
            }
            s.timestamp = now_ms();
            // `ShortcutRequestPinProcessor.directPinShortcut`'s activity.
            if s.activity.as_ref().is_some_and(&dummy_main) {
                s.activity = None;
            }
            s.touched();
            Ok(None)
        } else {
            Ok(self.force_delete(id))
        }
    }

    /// `deleteDynamicWithId(id, true)`.
    fn delete_dynamic(&mut self, id: &str, dummy_main: impl Fn(&ComponentName) -> bool) -> Result<(), Exception> {
        self.delete_or_disable(id, None, dummy_main).map(|_| ())
    }

    /// `enableWithId`.
    fn enable(&mut self, id: &str) -> Result<(), Exception> {
        self.ensure_not_immutable(id)?;
        if let Some(s) = self.shortcuts.iter_mut().find(|s| s.id == id) {
            s.flags &= !FLAG_DISABLED;
            s.disabled_reason = 0;
            s.touched();
        }
        Ok(())
    }

    /// `adjustRanks`: floating shortcuts' ranks cleared, each activity's
    /// dynamic shortcuts ranked from 0 (`mShortcutRankComparator`).
    fn adjust_ranks(&mut self) {
        let now = now_ms();
        for s in self.shortcuts.iter_mut().filter(|s| s.is_floating() && s.rank != 0) {
            s.timestamp = now;
            s.rank = 0;
            s.touched();
        }
        let mut activities: Vec<ComponentName> = Vec::new();
        for s in self.shortcuts.iter().filter(|s| !s.is_floating()) {
            if let Some(a) = &s.activity
                && !activities.contains(a)
            {
                activities.push(a.clone());
            }
        }
        for activity in activities {
            let mut list: Vec<usize> = (0..self.shortcuts.len()).filter(|&i| !self.shortcuts[i].is_floating() && self.shortcuts[i].activity.as_ref() == Some(&activity)).collect();
            list.sort_by(|&a, &b| {
                let (a, b) = (&self.shortcuts[a], &self.shortcuts[b]);
                a.rank
                    .cmp(&b.rank)
                    .then(b.rank_changed().cmp(&a.rank_changed()))
                    .then((a.implicit_rank & IMPLICIT_RANK_MASK).cmp(&(b.implicit_rank & IMPLICIT_RANK_MASK)))
                    .then(a.id.encode_utf16().cmp(b.id.encode_utf16()))
            });
            let mut rank = 0;
            for i in list {
                let s = &mut self.shortcuts[i];
                if s.flags & FLAG_MANIFEST != 0 || s.flags & FLAG_DYNAMIC == 0 {
                    continue;
                }
                if s.rank != rank {
                    s.timestamp = now;
                    s.rank = rank;
                    s.touched();
                }
                rank += 1;
            }
        }
    }

    /// `enforceShortcutCountsBeforeOperation`.
    fn enforce_counts(&self, new: &[Incoming], operation: Operation, max: i64) -> Result<(), Exception> {
        let mut counts: Vec<(ComponentName, i64)> = Vec::new();
        let mut add = |a: Option<&ComponentName>, n: i64| {
            let Some(a) = a else { return };
            match counts.iter_mut().find(|(c, _)| c == a) {
                Some((_, v)) => *v += n,
                None => counts.push((a.clone(), n)),
            }
        };
        for s in &self.shortcuts {
            if s.flags & FLAG_MANIFEST != 0 || (s.flags & FLAG_DYNAMIC != 0 && operation != Operation::Set) {
                add(s.activity.as_ref(), 1);
            }
        }
        for n in new.iter().rev() {
            let Some(activity) = n.shortcut.activity.as_ref() else { continue };
            if let Some(original) = self.find(&n.shortcut.id)
                && operation != Operation::Set
                && !original.is_floating()
            {
                add(original.activity.as_ref(), -1);
            }
            add(Some(activity), 1);
        }
        if counts.iter().any(|(_, n)| *n > max) {
            return Err(Exception::illegal_argument("Max number of dynamic shortcuts exceeded"));
        }
        Ok(())
    }

    /// `ensureShortcutCountBeforePush`: the shortcuts not pinned kept
    /// under `max` (`mShortcutTypeRankAndTimeComparator`'s last ones
    /// removed).
    fn ensure_count_before_push(&mut self, max: i64) -> Result<(), Exception> {
        let mut app: Vec<usize> = (0..self.shortcuts.len()).filter(|&i| self.shortcuts[i].flags & FLAG_PINNED == 0).collect();
        if (app.len() as i64) < max {
            return Ok(());
        }
        let declared = |s: &Shortcut| s.flags & FLAG_MANIFEST != 0;
        let dynamic = |s: &Shortcut| s.flags & FLAG_DYNAMIC != 0;
        app.sort_by(|&a, &b| {
            let (a, b) = (&self.shortcuts[a], &self.shortcuts[b]);
            declared(b)
                .cmp(&declared(a))
                .then_with(|| if dynamic(a) && dynamic(b) { a.rank.cmp(&b.rank) } else { dynamic(b).cmp(&dynamic(a)) })
                .then_with(|| if dynamic(a) || dynamic(b) { std::cmp::Ordering::Equal } else { b.timestamp.cmp(&a.timestamp) })
        });
        let mut removed = Vec::new();
        while app.len() as i64 >= max {
            let i = app.pop().unwrap();
            if declared(&self.shortcuts[i]) {
                let name = self.root.string("name").map(|s| s.into_owned()).unwrap_or_default();
                return Err(Exception::illegal_argument(format!("{name} has published {} manifest shortcuts across different activities.", app.len())));
            }
            removed.push(self.shortcuts[i].id.clone());
        }
        for id in removed {
            self.force_delete(&id);
        }
        Ok(())
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
            Some(b) => aim_android_xml::read_next_optional(&b),
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
fn write_resilient(file: &Path, roots: &[Element]) -> std::io::Result<()> {
    let bytes = aim_android_xml::abx::write_sequence(roots).map_err(std::io::Error::other)?;
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

    /// `ShortcutRequestPinProcessor.getRequestPinConfirmationActivity`: the
    /// default launcher's (the home role holder's) activity confirming a
    /// pinned shortcut or widget (`injectGetPinConfirmationActivity`).
    /// The system user's items are not restricted on its home screen
    /// (`areShortcutsSupportedOnHomeScreen`).
    pub(super) fn pin_confirmation_activity(&self, user: i32, request_type: i32) -> Result<Option<String>, Exception> {
        if user != 0 {
            return Err(not_modelled("a pin request of another user"));
        }
        let roles = self.host.role_service.get().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "shortcut: no role service"))?;
        let Some(launcher) = roles.holders_of(ROLE_HOME)?.into_iter().next() else {
            eprintln!("app host: shortcut: Default launcher not found. userId={user}");
            return Ok(None);
        };
        let action = if request_type == REQUEST_TYPE_SHORTCUT { ACTION_CONFIRM_PIN_SHORTCUT } else { ACTION_CONFIRM_PIN_APPWIDGET };
        let intent = crate::package::intent::Intent { action: Some(action.into()), package: Some(launcher), ..Default::default() };
        Ok(self.query_activities(&intent, user, false)?.into_iter().next().map(|c| c.class))
    }

    /// `queryActivities`: the activities resolved, of installed packages,
    /// enabled unless system, and exported if `exported_only`.
    fn query_activities(&self, intent: &crate::package::intent::Intent, user: i32, exported_only: bool) -> Result<Vec<ComponentName>, Exception> {
        let (state, resolution) = self.host.packages()?;
        let found = resolution
            .query_intent_activities(intent, None, PACKAGE_MATCH_FLAGS | MATCH_DISABLED_COMPONENTS, user, 1000)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("shortcut: {e:?}")))?;
        let query = crate::package::query::Query { state: &state, filter: &resolution.apps_filter, calling_uid: 1000 };
        let mut activities = Vec::new();
        for r in found {
            let crate::package::component_resolver::Info::Activity(a) = r.info else { continue };
            let app = &a.info.application_info;
            // `isInstalled`, then `isSystem` or `isEnabled`.
            if !app.enabled || app.flags & FLAG_INSTALLED == 0 {
                continue;
            }
            let component = ComponentName { package: app.item.package_name.clone().unwrap_or_default(), class: a.info.item.name.clone().unwrap_or_default() };
            let setting = query
                .internal_component_enabled(Some(&component), 1000, user, true)
                .map_err(|e| not_modelled(&e.0))??;
            let enabled = (setting == COMPONENT_ENABLED_STATE_DEFAULT && a.info.enabled) || setting == COMPONENT_ENABLED_STATE_ENABLED;
            if (app.flags & SYSTEM_APP_MASK == 0 && !enabled) || (exported_only && !a.info.exported) {
                continue;
            }
            activities.push(component);
        }
        Ok(activities)
    }

    /// `queryActivities(getMainActivityIntent(), package, activity, user)`.
    fn main_activities(&self, package: &str, activity: Option<&ComponentName>, user: i32) -> Result<Vec<ComponentName>, Exception> {
        let intent = crate::package::intent::Intent {
            action: Some(ACTION_MAIN.into()),
            categories: Some(vec![CATEGORY_LAUNCHER.into()]),
            package: Some(package.into()),
            component: activity.cloned(),
            ..Default::default()
        };
        self.query_activities(&intent, user, true)
    }

    /// `ShortcutService.setDynamicShortcuts` and `addDynamicShortcuts`:
    /// whether the call was within the rate limit.
    fn publish(&self, uid: i32, package: Option<&str>, mut new: Vec<Incoming>, user: i32, operation: Operation) -> Result<bool, Exception> {
        let package = self.verify_caller(uid, package, user)?;
        let unlimited = self.host.check_permission(Some(UNLIMITED_SHORTCUTS_API_CALLS), uid)? == 0;
        for n in &new {
            // `verifyShortcutInfoPackage`.
            if n.package.as_deref() != Some(package.as_str()) {
                return Err(Exception::security("Shortcut package name mismatch"));
            }
            if uid / PER_USER_RANGE as i32 != n.user {
                return Err(Exception::security("UserId in shortcut doesn't match the caller"));
            }
            if let Some(what) = n.unsupported {
                return Err(self.host.unimplemented_named("shortcut", &format!("{what} (#1669)")));
            }
            if let Some((ty @ (TYPE_BITMAP | TYPE_ADAPTIVE_BITMAP | TYPE_URI | TYPE_URI_ADAPTIVE_BITMAP), _, _)) = n.icon {
                let what = if ty == TYPE_BITMAP || ty == TYPE_ADAPTIVE_BITMAP { "a bitmap icon" } else { "an icon URI's grant check" };
                return Err(self.host.unimplemented_named("shortcut", &format!("{what} (#1669)")));
            }
        }
        let max_shortcuts = self.constant("max_shortcuts", DEFAULT_MAX_SHORTCUTS_PER_ACTIVITY)?.max(0);
        let max_updates = self.constant("max_updates_per_interval", DEFAULT_MAX_UPDATES_PER_INTERVAL)?.max(0);
        let max_per_app = self.constant("max_shortcuts_per_app", DEFAULT_MAX_SHORTCUTS_PER_APP)?.max(0);
        self.with_user(|u| {
            self.on_called_by_publisher(u, &package)?;
            let p = u.packages.get_mut(&package).ok_or_else(|| not_modelled("a package's first shortcut state"))?;
            for n in new.iter().rev() {
                p.ensure_not_immutable(&n.shortcut.id)?;
            }
            // `fillInDefaultActivity`.
            if new.iter().any(|n| n.shortcut.activity.is_none()) {
                let default = self.main_activities(&package, None, user)?.into_iter().next();
                let default = default.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("Launcher activity not found for package {package}")))?;
                for n in new.iter_mut().filter(|n| n.shortcut.activity.is_none()) {
                    n.shortcut.activity = Some(default.clone());
                }
            }
            p.enforce_counts(&new, operation, max_shortcuts)?;
            // `tryApiCall`, around the implicit ranks for `adjustRanks` as
            // each operation orders them.
            let rank = |p: &mut Package, new: &mut [Incoming]| {
                p.shortcuts.iter_mut().for_each(|s| s.implicit_rank = 0);
                for (i, n) in new.iter_mut().enumerate() {
                    n.shortcut.implicit_rank = (n.shortcut.implicit_rank & RANK_CHANGED_BIT) | (i as i32 & IMPLICIT_RANK_MASK);
                }
            };
            if operation == Operation::Add {
                rank(p, &mut new);
            }
            if self.api_call_count(p, uid, unlimited)? >= max_updates {
                return Ok(false);
            }
            let count = p.attr_long("call-count");
            p.set_attr_long("call-count", count + 1);
            if operation == Operation::Set {
                rank(p, &mut new);
            }
            for n in &mut new {
                self.fix_up_incoming(&package, n, user)?;
            }
            if operation == Operation::Set {
                p.delete_all_dynamic();
            }
            for mut n in new {
                if operation == Operation::Add {
                    n.shortcut.implicit_rank |= RANK_CHANGED_BIT;
                }
                self.add_or_replace_dynamic(p, n, max_per_app)?;
            }
            p.adjust_ranks();
            p.save()?;
            Ok(true)
        })
    }

    /// `fixUpIncomingShortcutInfo(shortcut, false)`.
    fn fix_up_incoming(&self, package: &str, n: &mut Incoming, user: i32) -> Result<(), Exception> {
        let s = &mut n.shortcut;
        if s.flags & FLAG_RETURNED_BY_SERVICE != 0 {
            eprintln!("app host: shortcut: Re-publishing ShortcutInfo returned by server is not supported. Some information such as icon may lost from shortcut.");
        }
        if let Some(a) = &s.activity {
            if a.package != package {
                return Err(Exception::new(EX_ILLEGAL_STATE, format!("Cannot publish shortcut: activity {}/{} does not belong to package {package}", a.package, a.class)));
            }
            if a.class != DUMMY_MAIN_ACTIVITY && self.main_activities(package, Some(a), user)?.is_empty() {
                return Err(Exception::new(EX_ILLEGAL_STATE, format!("Cannot publish shortcut: activity {}/{} is not main activity", a.package, a.class)));
            }
        }
        // `enforceMandatoryFields(false)`.
        if s.activity.is_none() {
            return Err(Exception::new(EX_NULL_POINTER, "Activity must be provided"));
        }
        if s.title.is_none() && s.title_res_id == 0 {
            return Err(Exception::illegal_argument("Short label must be provided"));
        }
        if !n.has_intents {
            return Err(Exception::new(EX_NULL_POINTER, "Shortcut Intent must be provided"));
        }
        if s.intents.is_empty() {
            return Err(Exception::illegal_argument("Shortcut Intent must be provided"));
        }
        // `validateIcon`.
        if let Some((ty, _, tinted)) = &n.icon {
            if !matches!(*ty, TYPE_RESOURCE | TYPE_BITMAP | TYPE_ADAPTIVE_BITMAP | TYPE_URI | TYPE_URI_ADAPTIVE_BITMAP) {
                return Err(Exception::illegal_argument("Unsupported icon type: only the bitmap and resource types are supported"));
            }
            if *tinted {
                return Err(Exception::illegal_argument("Icons with tints are not supported"));
            }
        }
        s.flags &= FLAG_LONG_LIVED;
        Ok(())
    }

    /// `addOrReplaceDynamicShortcut`, then `forceReplaceShortcutInner`
    /// unless excluded from the launcher.
    fn add_or_replace_dynamic(&self, p: &mut Package, n: Incoming, max_per_app: i64) -> Result<(), Exception> {
        let Incoming { mut shortcut, icon, .. } = n;
        shortcut.flags |= FLAG_DYNAMIC;
        if let Some(old) = p.find(&shortcut.id) {
            // `ensureUpdatableWith(newShortcut, false)`.
            if old.is_visible_to_publisher() && old.is_immutable() {
                return Err(Exception::new(EX_ILLEGAL_STATE, "Target ShortcutInfo is immutable"));
            }
            shortcut.flags |= old.flags & (FLAG_PINNED | FLAG_CACHED_ALL);
        }
        if shortcut.excluded_surfaces & SURFACE_LAUNCHER != 0 {
            return Ok(());
        }
        p.force_delete(&shortcut.id);
        // `saveIconAndFixUpShortcutLocked`: a resource icon only (the
        // others are refused before).
        shortcut.icon_res_id = 0;
        shortcut.icon_res_name = None;
        shortcut.bitmap_path = None;
        shortcut.icon_uri = None;
        shortcut.flags &= !(FLAG_HAS_ICON_FILE | FLAG_ADAPTIVE_BITMAP | FLAG_HAS_ICON_RES | FLAG_HAS_ICON_URI);
        if let Some((_, icon, _)) = icon {
            let crate::notifications::parcels::Icon::Resource { package, id } = icon else { unreachable!("refused by publish") };
            if package != p.root.string("name").as_deref().unwrap_or_default() {
                return Err(Exception::illegal_argument("Icon resource must reside in shortcut owner package"));
            }
            shortcut.icon_res_id = id;
            shortcut.flags |= FLAG_HAS_ICON_RES;
        }
        self.fix_up_resource_names_and_values(&mut shortcut, &p.root.string("name").unwrap_or_default())?;
        p.ensure_count_before_push(max_per_app)?;
        p.put(shortcut);
        Ok(())
    }

    /// `fixUpShortcutResourceNamesAndValues`: `lookupAndFillInResourceNames`
    /// and `resolveResourceStrings` in the publisher's resources.
    fn fix_up_resource_names_and_values(&self, s: &mut Shortcut, package: &str) -> Result<(), Exception> {
        let ids = [s.title_res_id, s.text_res_id, s.disabled_message_res_id, s.icon_res_id];
        s.flags |= FLAG_STRINGS_RESOLVED;
        if ids.iter().all(|&i| i == 0) {
            return Ok(());
        }
        let (state, _) = self.host.packages()?;
        let pkg = state.packages.get(package).and_then(|ps| ps.pkg.clone()).ok_or_else(|| Exception::illegal_argument(format!("no package {package}")))?;
        let inputs = self.host.inputs()?;
        let ids_u: Vec<u32> = ids.iter().map(|&i| i as u32).collect();
        let found = inputs.packages.resource_names_and_strings(self.host.root()?, &inputs.data, &pkg, &ids_u).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("shortcut: {e}")))?;
        // `lookUpResourceName`: a framework resource by its ID.
        let name = |i: usize, with_type: bool| -> Option<String> {
            if ids[i] == 0 {
                return None;
            }
            match &found[i].0 {
                Some((0x01, _, _)) => Some(ids[i].to_string()),
                Some((_, ty, entry)) => Some(if with_type { format!("{ty}/{entry}") } else { entry.clone() }),
                None => {
                    eprintln!("app host: shortcut: Resource name for ID={} not found in package {package}. Resource IDs may change when the application is upgraded, and the system may not be able to find the correct resource.", ids[i]);
                    None
                }
            }
        };
        s.title_res_name = name(0, false);
        s.text_res_name = name(1, false);
        s.disabled_message_res_name = name(2, false);
        s.icon_res_name = name(3, true);
        // `getResourceString`: the value set when not found.
        let string = |i: usize, current: &mut Option<String>| {
            if ids[i] != 0 {
                match &found[i].1 {
                    Some(v) => *current = Some(v.clone()),
                    None => eprintln!("app host: shortcut: Resource for ID={} not found in package {package}", ids[i]),
                }
            }
        };
        string(0, &mut s.title);
        string(1, &mut s.text);
        string(2, &mut s.disabled_message);
        Ok(())
    }

    /// `removeDynamicShortcuts` and `removeAllDynamicShortcuts` (`ids`
    /// none).
    fn remove(&self, uid: i32, package: Option<&str>, ids: Option<Vec<Option<String>>>, user: i32, all: bool) -> Result<(), Exception> {
        let package = self.verify_caller(uid, package, user)?;
        let ids = match (all, ids) {
            (true, _) => Vec::new(),
            (false, Some(ids)) => ids,
            (false, None) => return Err(Exception::new(EX_NULL_POINTER, "shortcutIds must be provided")),
        };
        self.with_user(|u| {
            self.on_called_by_publisher(u, &package)?;
            let p = u.packages.get_mut(&package).ok_or_else(|| not_modelled("a package's first shortcut state"))?;
            if all {
                p.delete_all_dynamic();
            } else {
                for id in ids.iter().rev() {
                    p.ensure_not_immutable(id.as_deref().unwrap_or_default())?;
                }
                for id in ids.iter().rev() {
                    let id = id.as_deref().filter(|i| !i.is_empty()).ok_or_else(|| Exception::illegal_argument(""))?;
                    if p.find(id).is_some_and(Shortcut::is_visible_to_publisher) {
                        p.delete_dynamic(id, |a| a.class == DUMMY_MAIN_ACTIVITY)?;
                    }
                }
                p.adjust_ranks();
            }
            p.save()
        })
    }

    /// `disableShortcuts` (`message` some, or `message_res` given) and
    /// `enableShortcuts` (neither) of the caller's shortcuts `ids`.
    fn disable_or_enable(&self, uid: i32, package: Option<&str>, ids: Option<Vec<Option<String>>>, disable: Option<(Option<String>, i32)>, user: i32) -> Result<(), Exception> {
        let package = self.verify_caller(uid, package, user)?;
        let ids = ids.ok_or_else(|| Exception::new(EX_NULL_POINTER, "shortcutIds must be provided"))?;
        self.with_user(|u| {
            self.on_called_by_publisher(u, &package)?;
            let p = u.packages.get_mut(&package).ok_or_else(|| not_modelled("a package's first shortcut state"))?;
            for id in ids.iter().rev() {
                p.ensure_not_immutable(id.as_deref().unwrap_or_default())?;
            }
            for id in ids.iter().rev() {
                let id = id.as_deref().filter(|i| !i.is_empty()).ok_or_else(|| Exception::illegal_argument(""))?;
                if !p.find(id).is_some_and(Shortcut::is_visible_to_publisher) {
                    continue;
                }
                let Some((message, message_res)) = &disable else {
                    p.enable(id)?;
                    continue;
                };
                // `disableWithId`: a kept shortcut's disabled message.
                p.delete_or_disable(id, Some(DISABLED_REASON_BY_APP), |a| a.class == DUMMY_MAIN_ACTIVITY)?;
                if let Some(s) = p.shortcuts.iter_mut().find(|s| s.id == id) {
                    if let Some(message) = message {
                        s.disabled_message = Some(message.clone());
                        s.disabled_message_res_id = 0;
                        s.disabled_message_res_name = None;
                    } else if *message_res != 0 {
                        if s.disabled_message_res_id != *message_res {
                            s.disabled_message_res_name = None;
                        }
                        s.disabled_message_res_id = *message_res;
                        s.disabled_message = None;
                        self.fix_up_resource_names_and_values(s, &package)?;
                    }
                    s.touched();
                }
            }
            if disable.is_some() {
                p.adjust_ranks();
            }
            p.save()
        })
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
        write_resilient(&user.file, std::slice::from_ref(&user.root)).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", user.file.display())))
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
            write_resilient(&file, &[root]).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", file.display())))?;
        }
        *guard = Some(next);
        Ok(next)
    }

    /// `ShortcutPackage.getApiCallCount`: reset while the app is in the
    /// foreground (each app host process is), for an unlimited caller, or
    /// since the last reset.
    fn api_call_count(&self, package: &mut Package, uid: i32, unlimited: bool) -> Result<i64, Exception> {
        let last = self.last_reset()?;
        let mut changed = false;
        if (self.host.uid_running(uid) || unlimited) && package.attr_long("call-count") > 0 {
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
            package.save()?;
        }
        Ok(package.attr_long("call-count"))
    }

    /// A call taking a package, a `ParceledListSlice` of `ShortcutInfo`
    /// and a user.
    fn read_list_call(&self, call: &mut Call<'_>) -> aim_binder_host::parcel::Result<(Option<String>, Vec<Incoming>, i32)> {
        let r = &mut call.data;
        r.enforce_interface(ss::DESCRIPTOR)?;
        let package = r.read_string16()?;
        let list = if r.read_i32()? != 0 { crate::package::list_slice::read(r, &self.host.process)? } else { Vec::new() };
        Ok((package, list, r.read_i32()?))
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
                            Some(p) => self.api_call_count(p, uid, false),
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
            ss::IS_REQUEST_PIN_ITEM_SUPPORTED => {
                let a = ss::IsRequestPinItemSupported::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    // `verifyCallerUserId`.
                    if uid.rem_euclid(PER_USER_RANGE as i32) != 1000 && uid / PER_USER_RANGE as i32 != a.user {
                        return Err(Exception::security("Invalid userId"));
                    }
                    let supported = self.pin_confirmation_activity(a.user, a.request_type)?.is_some();
                    ss::write_is_request_pin_item_supported_reply(p, supported);
                    Ok(())
                })()
            }
            ss::SET_DYNAMIC_SHORTCUTS | ss::ADD_DYNAMIC_SHORTCUTS => {
                let (package, list, user) = self.read_list_call(call).map_err(|_| BAD_VALUE)?;
                let operation = if call.code == ss::SET_DYNAMIC_SHORTCUTS { Operation::Set } else { Operation::Add };
                self.publish(uid, package.as_deref(), list, user, operation).map(|ok| ss::write_set_dynamic_shortcuts_reply(p, ok))
            }
            ss::REMOVE_DYNAMIC_SHORTCUTS => {
                let a = ss::RemoveDynamicShortcuts::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.remove(uid, a.package_name.as_deref(), a.shortcut_ids, a.user_id, false).map(|()| p.write_no_exception())
            }
            ss::REMOVE_ALL_DYNAMIC_SHORTCUTS => {
                let a = ss::RemoveAllDynamicShortcuts::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.remove(uid, a.package_name.as_deref(), None, a.user_id, true).map(|()| p.write_no_exception())
            }
            ss::DISABLE_SHORTCUTS => {
                let a = ss::DisableShortcuts::<Text>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let disable = Some((a.disabled_message.and_then(|t| t.0), a.disabled_message_res_id));
                self.disable_or_enable(uid, a.package_name.as_deref(), a.shortcut_ids, disable, a.user_id).map(|()| p.write_no_exception())
            }
            ss::ENABLE_SHORTCUTS => {
                let a = ss::EnableShortcuts::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.disable_or_enable(uid, a.package_name.as_deref(), a.shortcut_ids, None, a.user_id).map(|()| p.write_no_exception())
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

    /// `ShortcutInfo.writeToParcel` of a shortcut with a resource icon, a
    /// title, a rank not set and a category, read back.
    #[test]
    fn incoming_shortcuts_are_read_as_shortcut_info_writes_them() {
        let mut p = Parcel::new();
        p.write_i32(0);
        p.write_string8(Some("s1"));
        p.write_string8(Some("com.example"));
        p.write_string16(None);
        p.write_i32(FLAG_LONG_LIVED | FLAG_PINNED);
        p.write_i32(0);
        p.write_i64(5);
        p.write_i32(0);
        p.write_i32(1);
        p.write_string16(Some("android.graphics.drawable.Icon"));
        p.write_i32(TYPE_RESOURCE);
        p.write_string16(Some("com.example"));
        p.write_i32(0x7f01_0002);
        p.write_bool(false);
        p.write_f32(0.0);
        p.write_i32(0);
        p.write_i32(0);
        for title in [Some("T"), None, None] {
            p.write_i32(1);
            p.write_string8(title);
            p.write_i32(0);
        }
        p.write_i32(-1);
        p.write_i32(-1);
        p.write_i32(i32::MAX);
        p.write_string16(None);
        for _ in 0..5 {
            p.write_string8(None);
        }
        p.write_i32(1);
        p.write_string8(Some("c"));
        p.write_i32(-1);
        p.write_string16(Some("android.content.LocusId"));
        p.write_string16(Some("l"));
        p.write_string8(None);
        p.write_string8(None);
        p.write_i32(0);
        p.write_i32(-1);
        let n = <Incoming as aim_service_aidl::ReadParcelable>::read_from(&mut p.reader()).unwrap();
        assert_eq!((n.user, n.package.as_deref(), n.key_only, n.has_intents, n.unsupported), (0, Some("com.example"), false, false, None));
        let s = &n.shortcut;
        assert_eq!((s.id.as_str(), s.title.as_deref(), s.rank, s.timestamp, s.flags), ("s1", Some("T"), i32::MAX, 5, FLAG_LONG_LIVED | FLAG_PINNED));
        assert_eq!((s.categories.as_deref(), s.locus_id.as_deref()), (Some(&["c".to_string()][..]), Some("l")));
        let Some((TYPE_RESOURCE, crate::notifications::parcels::Icon::Resource { package, id }, false)) = &n.icon else { panic!("{:?}", n.icon.map(|i| i.0)) };
        assert_eq!((package.as_str(), *id), ("com.example", 0x7f01_0002));
    }

    /// A shortcut written as `saveShortcut` writes it reads back as it
    /// was, its intent's extras with it.
    #[test]
    fn shortcuts_are_written_as_the_package_writes_them() {
        let xml = br##"<shortcut id="d" activity="com.example/.Main" title="D" titleid="0" textid="0" dmessageid="0" disabled-reason="0" timestamp="11" rank="2" flags="129">
<intent intent-base="#Intent;action=a;component=com.example/.A;end"><extras><int name="n" value="2" /><string name="s">v</string></extras></intent>
<extras><boolean name="b" value="true" /></extras>
<categories><string-array name="categories" num="1"><item value="c" /></string-array></categories>
</shortcut>"##;
        let mut read = Shortcut::parse(&aim_android_xml::read(xml).unwrap()).unwrap();
        read.element = None;
        let mut again = Shortcut::parse(&read.to_xml()).unwrap();
        again.element = None;
        assert_eq!(again, read);
    }

    /// `adjustRanks`: a shortcut whose rank changed goes before one of the
    /// same rank, one with no rank set last, and manifest shortcuts keep
    /// theirs.
    #[test]
    fn ranks_are_adjusted_per_activity() {
        let root = aim_android_xml::read(br#"<package name="com.example"><package-info version="1" /></package>"#).unwrap();
        let mut p = Package::load(PathBuf::from("p.xml"), root).unwrap();
        let main = ComponentName { package: "com.example".into(), class: "com.example.Main".into() };
        let shortcut = |id: &str, rank, flags, implicit_rank| {
            let mut s = Shortcut::parse(&aim_android_xml::read(format!(r#"<shortcut id="{id}" />"#).as_bytes()).unwrap()).unwrap();
            (s.activity, s.rank, s.flags, s.implicit_rank, s.element) = (Some(main.clone()), rank, flags, implicit_rank, None);
            s
        };
        p.put(shortcut("m", 0, FLAG_MANIFEST | FLAG_IMMUTABLE, 0));
        p.put(shortcut("a", 0, FLAG_DYNAMIC, 0));
        p.put(shortcut("b", 1, FLAG_DYNAMIC, 0));
        p.put(shortcut("new", 1, FLAG_DYNAMIC, RANK_CHANGED_BIT));
        p.put(shortcut("last", i32::MAX, FLAG_DYNAMIC, RANK_CHANGED_BIT | 1));
        p.adjust_ranks();
        let mut ranks: Vec<(&str, i32)> = p.shortcuts.iter().map(|s| (s.id.as_str(), s.rank)).collect();
        ranks.sort();
        assert_eq!(ranks, [("a", 0), ("b", 2), ("last", 3), ("m", 0), ("new", 1)]);
    }

    /// `deleteOrDisableWithId(disable)` and `enableWithId`: a pinned
    /// dynamic shortcut is kept disabled by the app, its first reason
    /// kept; one only dynamic is deleted; enabling clears the reason.
    #[test]
    fn shortcuts_are_disabled_and_enabled_as_the_package_does() {
        let root = aim_android_xml::read(br#"<package name="com.example"><package-info version="1" /></package>"#).unwrap();
        let mut p = Package::load(PathBuf::from("p.xml"), root).unwrap();
        let shortcut = |id: &str, flags, reason| {
            let mut s = Shortcut::parse(&aim_android_xml::read(format!(r#"<shortcut id="{id}" />"#).as_bytes()).unwrap()).unwrap();
            (s.rank, s.flags, s.disabled_reason, s.element) = (3, flags, reason, None);
            s
        };
        p.put(shortcut("pinned", FLAG_DYNAMIC | FLAG_PINNED, 0));
        p.put(shortcut("restored", FLAG_PINNED, DISABLED_REASON_RESTORE_ISSUE_START));
        p.put(shortcut("dynamic", FLAG_DYNAMIC, 0));
        let never = |_: &ComponentName| false;
        assert!(p.delete_or_disable("pinned", Some(DISABLED_REASON_BY_APP), never).unwrap().is_none());
        assert!(p.delete_or_disable("restored", Some(DISABLED_REASON_BY_APP), never).unwrap().is_none());
        assert_eq!(p.delete_or_disable("dynamic", Some(DISABLED_REASON_BY_APP), never).unwrap().map(|s| s.id), Some("dynamic".into()));
        let state = |p: &Package, id| p.find(id).map(|s| (s.flags, s.disabled_reason, s.rank));
        assert_eq!(state(&p, "pinned"), Some((FLAG_PINNED | FLAG_DISABLED, DISABLED_REASON_BY_APP, 0)));
        assert_eq!(state(&p, "restored"), Some((FLAG_PINNED | FLAG_DISABLED, DISABLED_REASON_RESTORE_ISSUE_START, 0)));
        assert_eq!(state(&p, "dynamic"), None);
        p.enable("pinned").unwrap();
        assert_eq!(state(&p, "pinned"), Some((FLAG_PINNED, 0, 0)));
    }
}
