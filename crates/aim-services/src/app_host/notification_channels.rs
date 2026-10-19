//! Notification channels and channel groups as NotificationManagerService
//! keeps them (`PreferencesHelper` at `android-16.0.0_r1`): per package
//! and uid, read from and written to the `<ranking>` element of
//! `/data/system/notification_policy.xml`, created and updated by the
//! rules apps and the system are held to. `NotificationChannel` and
//! `NotificationChannelGroup` are read and written as their
//! `writeToParcel` and `writeXml` write them.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_android_xml::{Element, Node, Value};
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use std::sync::Arc;

use aim_binder_host::local::LocalProcess;

use super::playback::Attributes;
use crate::package::intent_filter::Plain;
use crate::package::list_slice::Slice;
use crate::package::uri::Uri;

/// The parcelable classes of a `ParceledListSlice`.
pub const CHANNEL_CLASS: &str = "android.app.NotificationChannel";
pub const GROUP_CLASS: &str = "android.app.NotificationChannelGroup";
/// `NotificationChannel.DEFAULT_CHANNEL_ID`: the channel of apps
/// targeting an SDK before O.
pub const DEFAULT_CHANNEL_ID: &str = "miscellaneous";
/// `NotificationChannel.SYSTEM_RESERVED_IDS` (`notification_classification`
/// is on): the bundle channels only the system posts to.
pub const SYSTEM_RESERVED_IDS: [&str; 4] = [
    "android.app.promotions",
    "android.app.social",
    "android.app.news",
    "android.app.recs",
];
/// `NotificationManager.IMPORTANCE_*`.
pub const IMPORTANCE_UNSPECIFIED: i32 = -1000;
pub const IMPORTANCE_NONE: i32 = 0;
pub const IMPORTANCE_MAX: i32 = 5;
/// `NotificationManager.VISIBILITY_NO_OVERRIDE`.
pub const VISIBILITY_NO_OVERRIDE: i32 = -1000;
/// `Notification.VISIBILITY_PUBLIC`, `Notification.PRIORITY_MAX`.
const VISIBILITY_PUBLIC: i32 = 1;
const PRIORITY_MAX: i32 = 2;
/// `NotificationChannel.USER_LOCKED_*` this module sets.
const USER_LOCKED_PRIORITY: i32 = 0x1;
const USER_LOCKED_VISIBILITY: i32 = 0x2;
const USER_LOCKED_IMPORTANCE: i32 = 0x4;
/// `NotificationChannel.DEFAULT_ALLOW_BUBBLE`.
const DEFAULT_ALLOW_BUBBLE: i32 = -1;
/// `NotificationChannel.MAX_TEXT_LENGTH`, `MAX_VIBRATION_LENGTH`.
const MAX_TEXT_LENGTH: usize = 1000;
const MAX_VIBRATION_LENGTH: usize = 500;
/// `PreferencesHelper`'s limits and its unknown uid (`UserHandle.USER_NULL`).
const NOTIFICATION_CHANNEL_COUNT_LIMIT: usize = 5000;
const NOTIFICATION_CHANNEL_GROUP_COUNT_LIMIT: usize = 6000;
const UNKNOWN_UID: i32 = -10000;
/// How long a deleted channel is kept (`NOTIFICATION_CHANNEL_DELETION_RETENTION_DAYS`).
const DELETION_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// `AudioAttributes.USAGE_NOTIFICATION`, `CONTENT_TYPE_SONIFICATION`.
const USAGE_NOTIFICATION: i32 = 5;
const CONTENT_TYPE_SONIFICATION: i32 = 4;
/// `VibrationEffect`'s parcel tokens; `VibrationEffectSegment`'s step
/// token; `Parcel.VAL_PARCELABLE`.
const PARCEL_TOKEN_COMPOSED: i32 = 1;
const PARCEL_TOKEN_STEP: i32 = 3;
const VAL_PARCELABLE: i32 = 4;
const STEP_SEGMENT: &str = "android.os.vibrator.StepSegment";
/// `VibrationEffect.DEFAULT_AMPLITUDE`.
const DEFAULT_AMPLITUDE: f32 = -1.0;

/// `getTrimmedString`: at most `MAX_TEXT_LENGTH` UTF-16 units.
fn trimmed(s: String) -> String {
    let mut units = 0;
    for (at, c) in s.char_indices() {
        units += c.len_utf16();
        if units > MAX_TEXT_LENGTH {
            return s[..at].to_string();
        }
    }
    s
}

/// `String.hashCode`, by which an `ArrayMap` orders its keys.
fn java_hash(s: Option<&str>) -> i32 {
    s.map_or(0, |s| {
        s.encode_utf16()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
    })
}

/// Puts `item` where `ArrayMap.put` puts a new key: after the keys of a
/// smaller or equal hash.
fn array_map_insert<T>(items: &mut Vec<T>, item: T, key: impl Fn(&T) -> Option<&str>) {
    let hash = java_hash(key(&item));
    let at = items.partition_point(|i| java_hash(key(i)) <= hash);
    items.insert(at, item);
}

/// A channel's `VibrationEffect`, kept as written, and the legacy pattern
/// it is equivalent to (`computeCreateWaveformOffOnTimingsOrNull`).
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    raw: Vec<u8>,
    pattern: Option<Vec<i64>>,
}

impl Effect {
    /// `VibrationEffect.createWaveform(pattern, -1)`, or none where it
    /// throws: no timing, a negative one, or none non-zero.
    fn waveform(pattern: &[i64]) -> Option<Effect> {
        if pattern.iter().all(|&t| t == 0) || pattern.iter().any(|&t| t < 0) {
            return None;
        }
        let mut p = Parcel::new();
        p.write_i32(PARCEL_TOKEN_COMPOSED);
        p.write_i32(pattern.len() as i32);
        for (i, &t) in pattern.iter().enumerate() {
            p.write_i32(VAL_PARCELABLE);
            let length_at = p.position();
            p.write_i32(0);
            let start = p.position();
            p.write_string16(Some(STEP_SEGMENT));
            p.write_i32(PARCEL_TOKEN_STEP);
            p.write_f32(if i % 2 == 1 { DEFAULT_AMPLITUDE } else { 0.0 });
            p.write_f32(0.0);
            p.write_i32(t as i32);
            let length = p.position() - start;
            p.set_i32_at(length_at, length as i32);
        }
        p.write_i32(-1); // repeat
        Some(Effect {
            raw: p.data().to_vec(),
            pattern: Some(pattern.to_vec()),
        })
    }

    /// `VibrationEffect.CREATOR`. A composed effect's segments are
    /// length-prefixed values; its pattern is known when every one is a
    /// step at the default frequency, off or on at the default amplitude.
    fn read(r: &mut Reader<'_>) -> ParcelResult<Effect> {
        let start = r.position();
        if r.read_i32()? != PARCEL_TOKEN_COMPOSED {
            // A vendor effect: kept, not convertible to a pattern.
            return Err(BAD_VALUE);
        }
        let count = r.read_i32()?;
        let mut steps = Some(Vec::new());
        for _ in 0..count.max(0) {
            if r.read_i32()? != VAL_PARCELABLE {
                return Err(BAD_VALUE);
            }
            let length = r.read_i32()?.max(0) as usize;
            let end = r.position() + length;
            let step = (|| {
                if r.read_string16().ok()?.as_deref() != Some(STEP_SEGMENT)
                    || r.read_i32().ok()? != PARCEL_TOKEN_STEP
                {
                    return None;
                }
                let amplitude = r.read_f32().ok()?;
                let frequency = r.read_f32().ok()?;
                let duration = r.read_i32().ok()?;
                (frequency == 0.0 && (amplitude == 0.0 || amplitude == DEFAULT_AMPLITUDE))
                    .then_some((amplitude == 0.0, duration as i64))
            })();
            r.set_position(end);
            match (&mut steps, step) {
                (Some(s), Some(step)) => s.push(step),
                _ => steps = None,
            }
        }
        let repeat = r.read_i32()?;
        let pattern = steps.filter(|_| repeat < 0).map(|steps| {
            // Even entries are off, odd ones on; a run of either is one.
            let mut pattern = vec![0i64];
            for (off, duration) in steps {
                if off != (pattern.len() % 2 == 1) {
                    pattern.push(0);
                }
                *pattern.last_mut().unwrap() += duration;
            }
            pattern
        });
        let (bytes, _) = r.since(start);
        Ok(Effect {
            raw: bytes.to_vec(),
            pattern,
        })
    }
}

/// `android.app.NotificationChannel`.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub importance: i32,
    pub bypass_dnd: bool,
    pub lockscreen_visibility: i32,
    pub sound: Option<Uri>,
    pub lights: bool,
    pub user_locked_fields: i32,
    pub user_visible_task_shown: bool,
    pub vibration_enabled: bool,
    pub show_badge: bool,
    pub deleted: bool,
    pub group: Option<String>,
    pub audio_attributes: Option<Attributes>,
    pub light_color: i32,
    pub blockable_system: bool,
    pub allow_bubbles: i32,
    pub original_importance: i32,
    pub parent_id: Option<String>,
    pub conversation_id: Option<String>,
    pub demoted: bool,
    pub important_conversation: bool,
    pub deleted_time: i64,
    pub importance_locked_default_app: bool,
    pub vibration_pattern: Option<Vec<i64>>,
    pub effect: Option<Effect>,
}

impl PartialEq for Attributes {
    fn eq(&self, other: &Attributes) -> bool {
        (self.usage, self.content_type(), self.flags())
            == (other.usage, other.content_type(), other.flags())
    }
}

impl std::fmt::Debug for Attributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "AudioAttributes({}, {}, {:#x})",
            self.usage,
            self.content_type(),
            self.flags()
        )
    }
}

impl Channel {
    /// `new NotificationChannel(id, name, importance)`.
    pub fn new(id: &str, name: &str, importance: i32) -> Channel {
        Channel {
            id: trimmed(id.into()),
            name: Some(trimmed(name.into())),
            description: None,
            importance,
            bypass_dnd: false,
            lockscreen_visibility: VISIBILITY_NO_OVERRIDE,
            // Settings.System.DEFAULT_NOTIFICATION_URI and
            // Notification.AUDIO_ATTRIBUTES_DEFAULT.
            sound: Some(Uri::parse("content://settings/system/notification_sound")),
            lights: false,
            user_locked_fields: 0,
            user_visible_task_shown: false,
            vibration_enabled: false,
            show_badge: true,
            deleted: false,
            group: None,
            audio_attributes: Some(Attributes::built(
                USAGE_NOTIFICATION,
                CONTENT_TYPE_SONIFICATION,
                0,
            )),
            light_color: 0,
            blockable_system: false,
            allow_bubbles: DEFAULT_ALLOW_BUBBLE,
            original_importance: IMPORTANCE_UNSPECIFIED,
            parent_id: None,
            conversation_id: None,
            demoted: false,
            important_conversation: false,
            deleted_time: -1,
            importance_locked_default_app: false,
            vibration_pattern: None,
            effect: None,
        }
    }

    /// `setVibrationPattern`.
    fn set_vibration_pattern(&mut self, pattern: Option<Vec<i64>>) {
        let mut pattern = pattern;
        if let Some(p) = &mut pattern {
            p.truncate(MAX_VIBRATION_LENGTH);
        }
        self.vibration_enabled = pattern.as_ref().is_some_and(|p| !p.is_empty());
        self.effect = pattern.as_deref().and_then(Effect::waveform);
        self.vibration_pattern = pattern;
    }

    /// `NotificationChannel(Parcel)`'s check for what this store keeps: a
    /// vibration effect it cannot write as `notification_policy.xml` does
    /// (`VibrationXmlSerializer`) is refused.
    pub fn persistable(&self) -> bool {
        self.effect.as_ref().is_none_or(|e| e.pattern.is_some())
    }

    /// `populateFromXml` of a `<channel>`, or none where `restoreChannel`
    /// skips it.
    fn from_xml(e: &Element) -> Option<Channel> {
        let int = |name: &str, default: i32| e.int(name).ok().flatten().unwrap_or(default);
        let flag = |name: &str| e.bool(name).ok().flatten().unwrap_or(false);
        let string = |name: &str| e.string(name).map(|s| s.into_owned());
        let id = string("id").filter(|s| !s.is_empty())?;
        let name = string("name").filter(|s| !s.is_empty())?;
        let mut c = Channel::new(&id, &name, int("importance", IMPORTANCE_UNSPECIFIED));
        c.description = string("desc").map(trimmed);
        c.bypass_dnd = int("priority", 0) != 0;
        c.lockscreen_visibility = int("visibility", VISIBILITY_NO_OVERRIDE);
        c.sound = string("sound").map(|s| Uri::parse(&s));
        // safeAudioAttributes; an invalid usage or content type throws.
        let usage = int("usage", USAGE_NOTIFICATION);
        let content_type = int("content_type", CONTENT_TYPE_SONIFICATION);
        if !matches!(usage, 0..=14 | 16) || !(0..=4).contains(&content_type) {
            return None;
        }
        c.audio_attributes = Some(Attributes::built(usage, content_type, int("flags", 0)));
        c.lights = flag("lights");
        c.light_color = int("light_color", 0);
        c.set_vibration_pattern(
            string("vibration")
                .filter(|v| !v.is_empty())
                .map(|v| v.split(',').map(|t| t.parse().unwrap_or(0)).collect()),
        );
        if e.attr("vibration_effect").is_some() {
            // VibrationXmlSerializer's effects are not read (#1487).
            return None;
        }
        c.vibration_enabled = flag("vibration_enabled");
        c.show_badge = flag("show_badge");
        c.deleted = flag("deleted");
        c.deleted_time = e
            .long("del_time")
            .ok()
            .flatten()
            .or_else(|| string("del_time")?.parse().ok())
            .unwrap_or(-1);
        c.group = string("group");
        c.user_locked_fields = int("locked", 0);
        c.user_visible_task_shown = flag("fgservice");
        c.blockable_system = flag("blockable_system");
        c.allow_bubbles = int("allow_bubbles", DEFAULT_ALLOW_BUBBLE);
        c.original_importance = int("orig_imp", IMPORTANCE_UNSPECIFIED);
        c.parent_id = string("parent");
        c.conversation_id = string("conv_id");
        c.demoted = flag("dem");
        c.important_conversation = flag("imp_conv");
        Some(c)
    }

    /// `writeXml`.
    fn to_xml(&self) -> Element {
        let mut a: Vec<(String, Value)> = Vec::new();
        let mut put = |name: &str, v: Value| a.push((name.into(), v));
        put("id", Value::String(self.id.clone()));
        if let Some(n) = &self.name {
            put("name", Value::String(n.clone()));
        }
        if let Some(d) = &self.description {
            put("desc", Value::String(d.clone()));
        }
        if self.importance != IMPORTANCE_UNSPECIFIED {
            put("importance", Value::Int(self.importance));
        }
        if self.bypass_dnd {
            put("priority", Value::Int(PRIORITY_MAX));
        }
        if self.lockscreen_visibility != VISIBILITY_NO_OVERRIDE {
            put("visibility", Value::Int(self.lockscreen_visibility));
        }
        if let Some(s) = &self.sound {
            put("sound", Value::String(s.as_str().into()));
        }
        if let Some(aa) = &self.audio_attributes {
            put("usage", Value::Int(aa.usage));
            put("content_type", Value::Int(aa.content_type()));
            put("flags", Value::Int(aa.flags()));
        }
        if self.lights {
            put("lights", Value::Bool(true));
        }
        if self.light_color != 0 {
            put("light_color", Value::Int(self.light_color));
        }
        if self.vibration_enabled {
            put("vibration_enabled", Value::Bool(true));
        }
        if let Some(p) = &self.vibration_pattern {
            let p: Vec<String> = p.iter().map(i64::to_string).collect();
            put("vibration", Value::String(p.join(",")));
        }
        if self.user_locked_fields != 0 {
            put("locked", Value::Int(self.user_locked_fields));
        }
        if self.user_visible_task_shown {
            put("fgservice", Value::Bool(true));
        }
        if self.show_badge {
            put("show_badge", Value::Bool(true));
        }
        if self.deleted {
            put("deleted", Value::Bool(true));
        }
        if self.deleted_time >= 0 {
            put("del_time", Value::Long(self.deleted_time));
        }
        if let Some(g) = &self.group {
            put("group", Value::String(g.clone()));
        }
        if self.blockable_system {
            put("blockable_system", Value::Bool(true));
        }
        if self.allow_bubbles != DEFAULT_ALLOW_BUBBLE {
            put("allow_bubbles", Value::Int(self.allow_bubbles));
        }
        if self.original_importance != IMPORTANCE_UNSPECIFIED {
            put("orig_imp", Value::Int(self.original_importance));
        }
        if let Some(p) = &self.parent_id {
            put("parent", Value::String(p.clone()));
        }
        if let Some(c) = &self.conversation_id {
            put("conv_id", Value::String(c.clone()));
        }
        if self.demoted {
            put("dem", Value::Bool(true));
        }
        if self.important_conversation {
            put("imp_conv", Value::Bool(true));
        }
        Element {
            name: "channel".into(),
            attrs: a,
            content: Vec::new(),
        }
    }
}

/// A byte flag (`writeByte`, an int on the wire), then the string.
fn read_flagged(r: &mut Reader<'_>) -> ParcelResult<Option<String>> {
    Ok(if r.read_i32()? != 0 {
        r.read_string16()?.map(trimmed)
    } else {
        None
    })
}

fn write_flagged(p: &mut Parcel, s: Option<&str>) {
    p.write_i32(s.is_some() as i32);
    if let Some(s) = s {
        p.write_string16(Some(s));
    }
}

impl ReadParcelable for Channel {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let id = read_flagged(r)?;
        let name = read_flagged(r)?;
        let description = read_flagged(r)?;
        let importance = r.read_i32()?;
        let bypass_dnd = r.read_i32()? != 0;
        let lockscreen_visibility = r.read_i32()?;
        let sound = if r.read_i32()? != 0 {
            Uri::read(r, &mut Plain)?.map(|u| Uri::parse(&trimmed(u.as_str().into())))
        } else {
            None
        };
        let lights = r.read_i32()? != 0;
        let user_locked_fields = r.read_i32()?;
        let user_visible_task_shown = r.read_i32()? != 0;
        let vibration_enabled = r.read_i32()? != 0;
        let show_badge = r.read_i32()? != 0;
        let deleted = r.read_i32()? != 0;
        let group = read_flagged(r)?;
        let audio_attributes = if r.read_i32()? > 0 {
            Some(Attributes::read_from(r)?)
        } else {
            None
        };
        let light_color = r.read_i32()?;
        let blockable_system = r.read_bool()?;
        let allow_bubbles = r.read_i32()?;
        let original_importance = r.read_i32()?;
        let parent_id = r.read_string16()?.map(trimmed);
        let conversation_id = r.read_string16()?.map(trimmed);
        let demoted = r.read_bool()?;
        let important_conversation = r.read_bool()?;
        let deleted_time = r.read_i64()?;
        let importance_locked_default_app = r.read_bool()?;
        let mut vibration_pattern = aim_service_aidl::read_long_array(r)?;
        if let Some(p) = &mut vibration_pattern {
            p.truncate(MAX_VIBRATION_LENGTH);
        }
        let effect = if r.read_i32()? != 0 {
            Some(Effect::read(r)?)
        } else {
            None
        };
        Ok(Channel {
            // A channel without an id is refused where it is used.
            id: id.unwrap_or_default(),
            name,
            description,
            importance,
            bypass_dnd,
            lockscreen_visibility,
            sound,
            lights,
            user_locked_fields,
            user_visible_task_shown,
            vibration_enabled,
            show_badge,
            deleted,
            group,
            audio_attributes,
            light_color,
            blockable_system,
            allow_bubbles,
            original_importance,
            parent_id,
            conversation_id,
            demoted,
            important_conversation,
            deleted_time,
            importance_locked_default_app,
            vibration_pattern,
            effect,
        })
    }
}

impl WriteParcelable for Channel {
    fn write_to(&self, p: &mut Parcel) {
        write_flagged(p, Some(&self.id));
        write_flagged(p, self.name.as_deref());
        write_flagged(p, self.description.as_deref());
        p.write_i32(self.importance);
        p.write_i32(self.bypass_dnd as i32);
        p.write_i32(self.lockscreen_visibility);
        p.write_i32(self.sound.is_some() as i32);
        if let Some(s) = &self.sound {
            s.write(p);
        }
        p.write_i32(self.lights as i32);
        p.write_i32(self.user_locked_fields);
        p.write_i32(self.user_visible_task_shown as i32);
        p.write_i32(self.vibration_enabled as i32);
        p.write_i32(self.show_badge as i32);
        p.write_i32(self.deleted as i32);
        write_flagged(p, self.group.as_deref());
        p.write_i32(self.audio_attributes.is_some() as i32);
        if let Some(a) = &self.audio_attributes {
            a.write_to(p);
        }
        p.write_i32(self.light_color);
        p.write_bool(self.blockable_system);
        p.write_i32(self.allow_bubbles);
        p.write_i32(self.original_importance);
        p.write_string16(self.parent_id.as_deref());
        p.write_string16(self.conversation_id.as_deref());
        p.write_bool(self.demoted);
        p.write_bool(self.important_conversation);
        p.write_i64(self.deleted_time);
        p.write_bool(self.importance_locked_default_app);
        aim_service_aidl::write_long_array(p, self.vibration_pattern.as_deref());
        p.write_i32(self.effect.is_some() as i32);
        if let Some(e) = &self.effect {
            p.write_raw_files(&e.raw, &[], &[]);
        }
    }
}

/// `android.app.NotificationChannelGroup`.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub channels: Vec<Channel>,
    pub blocked: bool,
    pub user_locked_fields: i32,
}

impl Group {
    /// `populateFromXml` of a `<channelGroup>` (its locked fields are
    /// written but not read).
    fn from_xml(e: &Element) -> Option<Group> {
        let id = e
            .string("id")
            .map(|s| s.into_owned())
            .filter(|s| !s.is_empty())?;
        Some(Group {
            id: Some(trimmed(id)),
            name: Some(trimmed(
                e.string("name").map(|s| s.into_owned()).unwrap_or_default(),
            )),
            description: e.string("desc").map(|s| trimmed(s.into_owned())),
            channels: Vec::new(),
            blocked: e.bool("blocked").ok().flatten().unwrap_or(false),
            user_locked_fields: 0,
        })
    }

    fn to_xml(&self) -> Element {
        let mut attrs = vec![(
            "id".to_string(),
            Value::String(self.id.clone().unwrap_or_default()),
        )];
        if let Some(n) = &self.name {
            attrs.push(("name".into(), Value::String(n.clone())));
        }
        if let Some(d) = &self.description {
            attrs.push(("desc".into(), Value::String(d.clone())));
        }
        attrs.push(("blocked".into(), Value::Bool(self.blocked)));
        attrs.push(("locked".into(), Value::Int(self.user_locked_fields)));
        Element {
            name: "channelGroup".into(),
            attrs,
            content: Vec::new(),
        }
    }
}

impl ReadParcelable for Group {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let id = read_flagged(r)?;
        let name = Some(read_flagged(r)?.unwrap_or_default());
        let description = read_flagged(r)?;
        if r.read_i32()? != 0 && r.read_string16()?.is_some() {
            // Its channels, which createNotificationChannelGroup replaces
            // with those it has: read past, the retriever of any not
            // written inline left unasked.
            let count = r.read_i32()?;
            if count > 0 {
                r.read_string16()?;
                let mut read = 0;
                while read < count && r.read_i32()? != 0 {
                    Channel::read_from(r)?;
                    read += 1;
                }
                if read < count {
                    r.read_binder()?;
                }
            }
        }
        Ok(Group {
            id,
            name,
            description,
            channels: Vec::new(),
            blocked: r.read_bool()?,
            user_locked_fields: r.read_i32()?,
        })
    }
}

/// A group as written to a parcel: its channels in a `ParceledListSlice`
/// of their own, served by `process`.
pub struct GroupParcel {
    pub group: Group,
    pub process: Arc<LocalProcess>,
}

impl WriteParcelable for GroupParcel {
    fn write_to(&self, p: &mut Parcel) {
        let g = &self.group;
        write_flagged(p, g.id.as_deref());
        write_flagged(p, g.name.as_deref());
        write_flagged(p, g.description.as_deref());
        p.write_i32(1);
        p.write_string16(Some("android.content.pm.ParceledListSlice"));
        Slice::new(self.process.clone(), CHANNEL_CLASS, g.channels.clone()).write_to(p);
        p.write_bool(g.blocked);
        p.write_i32(g.user_locked_fields);
    }
}

/// `PreferencesHelper.PackagePreferences`: its `<package>` attributes as
/// read, its delegate, groups and channels in `ArrayMap` order.
#[derive(Clone, Debug)]
pub struct Package {
    attrs: Vec<(String, Value)>,
    delegate: Option<Element>,
    pub groups: Vec<Group>,
    pub channels: Vec<Channel>,
}

impl Package {
    fn attr_int(&self, name: &str, default: i32) -> i32 {
        match self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v) {
            Some(Value::Int(v) | Value::IntHex(v)) => *v,
            Some(Value::String(s) | Value::Interned(s)) => s.parse().unwrap_or(default),
            _ => default,
        }
    }

    pub fn name(&self) -> &str {
        match self.attrs.iter().find(|(n, _)| n == "name").map(|(_, v)| v) {
            Some(Value::String(s) | Value::Interned(s)) => s,
            _ => "",
        }
    }

    pub fn uid(&self) -> i32 {
        self.attr_int("uid", UNKNOWN_UID)
    }

    /// The package's importance (`IMPORTANCE_UNSPECIFIED` unless set),
    /// priority and visibility.
    fn importance(&self) -> i32 {
        self.attr_int("importance", IMPORTANCE_UNSPECIFIED)
    }

    fn priority(&self) -> i32 {
        self.attr_int("priority", 0)
    }

    fn visibility(&self) -> i32 {
        self.attr_int("visibility", VISIBILITY_NO_OVERRIDE)
    }

    fn show_badge(&self) -> bool {
        match self
            .attrs
            .iter()
            .find(|(n, _)| n == "show_badge")
            .map(|(_, v)| v)
        {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s == "true",
            _ => true,
        }
    }

    /// Sets the attribute `name` to `value`, or removes it.
    fn set_attr(&mut self, name: &str, value: Option<Value>) {
        let at = self.attrs.iter().position(|(n, _)| n == name);
        match (at, value) {
            (Some(at), Some(v)) => self.attrs[at].1 = v,
            (None, Some(v)) => self.attrs.push((name.into(), v)),
            (Some(at), None) => drop(self.attrs.remove(at)),
            (None, None) => {}
        }
    }

    /// A channel by id.
    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    fn channel_mut(&mut self, id: &str) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    pub fn group(&self, id: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.id.as_deref() == Some(id))
    }

    /// `writePackageXml` (not for backup).
    fn to_xml(&self) -> Element {
        let mut content = Vec::new();
        content.extend(self.delegate.clone().map(Node::Element));
        content.extend(self.groups.iter().map(|g| Node::Element(g.to_xml())));
        content.extend(self.channels.iter().map(|c| Node::Element(c.to_xml())));
        Element {
            name: "package".into(),
            attrs: self.attrs.clone(),
            content,
        }
    }

    /// `createDefaultChannelIfNeededLocked` and
    /// `deleteDefaultChannelIfNeededLocked`: apps targeting an SDK before
    /// O have the default channel, named by `label`.
    fn fix_default_channel(&mut self, pre_o: bool, label: &str) {
        if !pre_o {
            self.channels.retain(|c| c.id != DEFAULT_CHANNEL_ID);
            return;
        }
        if let Some(c) = self.channel_mut(DEFAULT_CHANNEL_ID) {
            c.name = Some(trimmed(label.into()));
            return;
        }
        let mut c = Channel::new(DEFAULT_CHANNEL_ID, label, self.importance());
        c.bypass_dnd = self.priority() == PRIORITY_MAX;
        c.lockscreen_visibility = self.visibility();
        if self.importance() != IMPORTANCE_UNSPECIFIED {
            c.user_locked_fields |= USER_LOCKED_IMPORTANCE;
        }
        if self.priority() != 0 {
            c.user_locked_fields |= USER_LOCKED_PRIORITY;
        }
        if self.visibility() != VISIBILITY_NO_OVERRIDE {
            c.user_locked_fields |= USER_LOCKED_VISIBILITY;
        }
        array_map_insert(&mut self.channels, c, |c| Some(&c.id));
    }
}

/// What a package's preferences need of PackageManager: whether it
/// targets an SDK before O, and the default channel's label.
pub struct Defaults<'a> {
    pub pre_o: &'a dyn Fn(&str) -> bool,
    pub label: &'a str,
}

/// The channels and groups of every package (`mPackagePreferences`), and
/// the rest of the `<ranking>` element as read.
#[derive(Clone, Debug, Default)]
pub struct Preferences {
    ranking_attrs: Vec<(String, Value)>,
    /// Elements other than packages (`silent_status_icons`).
    other: Vec<Element>,
    packages: Vec<Package>,
}

impl Preferences {
    /// `readXml` of `ranking`, not for a restore.
    pub fn read(ranking: &Element, defaults: &Defaults<'_>, now_ms: i64) -> Preferences {
        let mut prefs = Preferences {
            ranking_attrs: ranking.attrs.clone(),
            ..Default::default()
        };
        for e in ranking.children() {
            if e.name != "package" {
                prefs.other.push(e.clone());
                continue;
            }
            let Some(name) = e.string("name").filter(|n| !n.is_empty()) else {
                continue;
            };
            let mut p = Package {
                attrs: e.attrs.clone(),
                delegate: None,
                groups: Vec::new(),
                channels: Vec::new(),
            };
            for c in e.children() {
                match c.name.as_str() {
                    "channelGroup" if p.groups.len() < NOTIFICATION_CHANNEL_GROUP_COUNT_LIMIT => {
                        if let Some(g) = Group::from_xml(c) {
                            p.groups.retain(|x| x.id != g.id);
                            array_map_insert(&mut p.groups, g, |g| g.id.as_deref());
                        }
                    }
                    "channel" if p.channels.len() < NOTIFICATION_CHANNEL_COUNT_LIMIT => {
                        // isShortcutOk and isDeletionOk.
                        let Some(ch) = Channel::from_xml(c) else {
                            continue;
                        };
                        let placeholder = ch
                            .conversation_id
                            .as_deref()
                            .is_some_and(|id| id.contains(":placeholder_id"));
                        let expired =
                            ch.deleted && ch.deleted_time <= now_ms - DELETION_RETENTION_MS;
                        if !placeholder && !expired {
                            p.channels.retain(|x| x.id != ch.id);
                            array_map_insert(&mut p.channels, ch, |c| Some(&c.id));
                        }
                    }
                    "delegate" => p.delegate = Some(c.clone()),
                    _ => {}
                }
            }
            if p.uid() != UNKNOWN_UID {
                p.fix_default_channel((defaults.pre_o)(&name), defaults.label);
            }
            prefs.packages.push(p);
        }
        prefs
    }

    /// The `<ranking>` element `writeXml` writes.
    pub fn to_xml(&self) -> Element {
        let mut content: Vec<Node> = self.other.iter().cloned().map(Node::Element).collect();
        content.extend(self.packages.iter().map(|p| Node::Element(p.to_xml())));
        Element {
            name: "ranking".into(),
            attrs: self.ranking_attrs.clone(),
            content,
        }
    }

    /// `getPackagePreferencesLocked`.
    pub fn package(&self, name: &str, uid: i32) -> Option<&Package> {
        self.packages
            .iter()
            .find(|p| p.name() == name && p.uid() == uid)
    }

    fn package_mut(&mut self, name: &str, uid: i32) -> Option<&mut Package> {
        self.packages
            .iter_mut()
            .find(|p| p.name() == name && p.uid() == uid)
    }

    /// `getOrCreatePackagePreferencesLocked` with the defaults.
    pub fn package_or_create(
        &mut self,
        name: &str,
        uid: i32,
        defaults: &Defaults<'_>,
    ) -> &mut Package {
        if let Some(at) = self
            .packages
            .iter()
            .position(|p| p.name() == name && p.uid() == uid)
        {
            return &mut self.packages[at];
        }
        let mut p = Package {
            attrs: vec![
                ("name".into(), Value::String(name.into())),
                ("show_badge".into(), Value::Bool(true)),
                ("app_user_locked_fields".into(), Value::Int(0)),
                ("sent_invalid_msg".into(), Value::Bool(false)),
                ("sent_valid_msg".into(), Value::Bool(false)),
                ("user_demote_msg_app".into(), Value::Bool(false)),
                ("sent_valid_bubble".into(), Value::Bool(false)),
                ("uid".into(), Value::Int(uid)),
            ],
            delegate: None,
            groups: Vec::new(),
            channels: Vec::new(),
        };
        p.fix_default_channel((defaults.pre_o)(name), defaults.label);
        self.packages.push(p);
        self.packages.last_mut().unwrap()
    }

    /// `setNotificationDelegate`: an enabled delegate of the package.
    pub fn set_delegate(
        &mut self,
        name: &str,
        uid: i32,
        delegate: &str,
        delegate_uid: i32,
        defaults: &Defaults<'_>,
    ) {
        self.package_or_create(name, uid, defaults).delegate = Some(Element {
            name: "delegate".into(),
            attrs: vec![
                ("name".into(), Value::String(delegate.into())),
                ("uid".into(), Value::Int(delegate_uid)),
            ],
            content: Vec::new(),
        });
    }

    /// `revokeNotificationDelegate`: the package's delegate disabled
    /// (`writeXml` names `enabled` only when it is not the default).
    pub fn revoke_delegate(&mut self, name: &str, uid: i32) {
        if let Some(d) = self
            .package_mut(name, uid)
            .and_then(|p| p.delegate.as_mut())
        {
            d.attrs.retain(|(n, _)| n != "enabled");
            d.attrs.push(("enabled".into(), Value::Bool(false)));
        }
    }

    /// `createNotificationChannelGroup`; whether it changed.
    pub fn create_group(
        &mut self,
        name: &str,
        uid: i32,
        mut group: Group,
        from_target_app: bool,
        defaults: &Defaults<'_>,
    ) -> Result<bool, String> {
        if group.id.is_none() {
            return Err("NullPointerException: group id".into());
        }
        if group.name.as_deref().is_none_or(str::is_empty) {
            return Err("group.getName() can't be empty".into());
        }
        let p = self.package_or_create(name, uid, defaults);
        if p.groups.len() >= NOTIFICATION_CHANNEL_GROUP_COUNT_LIMIT {
            return Err("IllegalStateException: Limit exceed; cannot create more groups".into());
        }
        if from_target_app {
            group.blocked = false;
        }
        group.channels.clear();
        let old = p.groups.iter().position(|g| g.id == group.id);
        if let Some(at) = old {
            let old = &p.groups[at];
            if from_target_app {
                group.blocked = old.blocked;
                group.user_locked_fields = old.user_locked_fields;
            }
            let changed = *old != group;
            p.groups[at] = group;
            return Ok(changed);
        }
        array_map_insert(&mut p.groups, group, |g| g.id.as_deref());
        Ok(true)
    }

    /// `createNotificationChannel`: whether the policy file changes.
    /// `has_dnd_access` is the caller's `isPackageOrComponentAllowed`.
    #[allow(clippy::too_many_arguments)]
    pub fn create_channel(
        &mut self,
        name: &str,
        uid: i32,
        mut channel: Channel,
        from_target_app: bool,
        has_dnd_access: bool,
        from_system: bool,
        defaults: &Defaults<'_>,
    ) -> Result<bool, String> {
        if channel.id.is_empty() && channel.name.is_none() {
            return Err("NullPointerException: channel id".into());
        }
        if channel.name.as_deref().is_none_or(str::is_empty) {
            return Err("IllegalArgumentException".into());
        }
        if !(IMPORTANCE_NONE..=IMPORTANCE_MAX).contains(&channel.importance) {
            return Err("IllegalArgumentException: Invalid importance level".into());
        }
        let p = self.package_or_create(name, uid, defaults);
        if let Some(g) = &channel.group
            && p.group(g).is_none()
        {
            return Err("IllegalArgumentException: NotificationChannelGroup doesn't exist".into());
        }
        if channel.id == DEFAULT_CHANNEL_ID {
            return Err("IllegalArgumentException: Reserved id".into());
        }
        if !from_system && SYSTEM_RESERVED_IDS.contains(&channel.id.as_str()) {
            return Ok(false);
        }
        let (priority, visibility, show_badge) = (p.priority(), p.visibility(), p.show_badge());
        if let (Some(existing), true) = (p.channel_mut(&channel.id), from_target_app) {
            let mut changed = false;
            let mut undeleted = false;
            if existing.deleted {
                existing.deleted = false;
                existing.deleted_time = -1;
                changed = true;
                undeleted = true;
            }
            if channel.name != existing.name {
                existing.name = channel.name;
                changed = true;
            }
            if channel.description != existing.description {
                existing.description = channel.description;
                changed = true;
            }
            if channel.blockable_system != existing.blockable_system {
                existing.blockable_system = channel.blockable_system;
                changed = true;
            }
            if channel.group.is_some() && existing.group.is_none() {
                existing.group = channel.group;
                changed = true;
            }
            if existing.user_locked_fields == 0 && channel.importance < existing.importance {
                existing.importance = channel.importance;
                changed = true;
            }
            if existing.user_locked_fields == 0
                && has_dnd_access
                && (channel.bypass_dnd != existing.bypass_dnd || undeleted)
            {
                existing.bypass_dnd = channel.bypass_dnd;
                changed = true;
            }
            if existing.original_importance == IMPORTANCE_UNSPECIFIED {
                existing.original_importance = channel.importance;
                changed = true;
            }
            return Ok(changed);
        }
        if p.channels.len() >= NOTIFICATION_CHANNEL_COUNT_LIMIT {
            return Err("IllegalStateException: Limit exceed; cannot create more channels".into());
        }
        if from_target_app && !has_dnd_access {
            channel.bypass_dnd = priority == PRIORITY_MAX;
        }
        if from_target_app {
            channel.lockscreen_visibility = visibility;
            channel.allow_bubbles = p
                .channel(&channel.id)
                .map_or(DEFAULT_ALLOW_BUBBLE, |e| e.allow_bubbles);
            channel.important_conversation = false;
        }
        channel.user_locked_fields = 0;
        if channel.lockscreen_visibility == VISIBILITY_PUBLIC {
            channel.lockscreen_visibility = VISIBILITY_NO_OVERRIDE;
        }
        if !show_badge {
            channel.show_badge = false;
        }
        channel.original_importance = channel.importance;
        if let Some(parent) = &channel.parent_id
            && p.channel(parent).is_none()
        {
            return Err(
                "IllegalArgumentException: Tried to create a conversation channel without a preexisting parent"
                    .into(),
            );
        }
        p.channels.retain(|c| c.id != channel.id);
        array_map_insert(&mut p.channels, channel, |c| Some(&c.id));
        Ok(true)
    }

    /// `updateNotificationChannel` by the system, not from the user:
    /// `channel` replaces the one of its id, its fields unlocked (but an
    /// importance a critical device function locked); a package with only
    /// the default channel takes its priority, visibility and badge.
    pub fn update_channel(
        &mut self,
        name: &str,
        uid: i32,
        channel: &mut Channel,
    ) -> Result<(), String> {
        let p = self
            .package_mut(name, uid)
            .ok_or("IllegalArgumentException: Invalid package")?;
        let Some(existing) = p.channel(&channel.id).filter(|c| !c.deleted) else {
            return Err("IllegalArgumentException: Channel does not exist".into());
        };
        if channel.lockscreen_visibility == VISIBILITY_PUBLIC {
            channel.lockscreen_visibility = VISIBILITY_NO_OVERRIDE;
        }
        channel.user_locked_fields = 0;
        if existing.importance_locked_default_app
            && !(existing.blockable_system || existing.importance == IMPORTANCE_NONE)
        {
            channel.importance = existing.importance;
        }
        if p.channels.len() == 1 && channel.id == DEFAULT_CHANNEL_ID {
            let priority = if channel.bypass_dnd { PRIORITY_MAX } else { 0 };
            p.set_attr("priority", (priority != 0).then_some(Value::Int(priority)));
            let visibility = channel.lockscreen_visibility;
            p.set_attr(
                "visibility",
                (visibility != VISIBILITY_NO_OVERRIDE).then_some(Value::Int(visibility)),
            );
            p.set_attr("show_badge", Some(Value::Bool(channel.show_badge)));
        }
        *p.channel_mut(&channel.id).unwrap() = channel.clone();
        Ok(())
    }

    /// `getConversationNotificationChannel`, with the package's
    /// preferences created as `getOrCreatePackagePreferencesLocked` does.
    #[allow(clippy::too_many_arguments)]
    pub fn conversation_channel(
        &mut self,
        name: &str,
        uid: i32,
        channel_id: Option<&str>,
        conversation_id: Option<&str>,
        return_parent: bool,
        include_deleted: bool,
        defaults: &Defaults<'_>,
    ) -> Option<Channel> {
        let p = self.package_or_create(name, uid, defaults);
        let channel_id = channel_id.unwrap_or(DEFAULT_CHANNEL_ID);
        let conversation = conversation_id.and_then(|conversation| {
            p.channels.iter().find(|c| {
                c.conversation_id.as_deref() == Some(conversation)
                    && c.parent_id.as_deref() == Some(channel_id)
                    && (include_deleted || !c.deleted)
            })
        });
        if let Some(c) = conversation {
            return Some(c.clone());
        }
        if return_parent {
            return p
                .channel(channel_id)
                .filter(|c| include_deleted || !c.deleted)
                .cloned();
        }
        None
    }

    /// `getNotificationChannels`.
    pub fn channels(
        &mut self,
        name: &str,
        uid: i32,
        include_deleted: bool,
        include_bundles: bool,
        defaults: &Defaults<'_>,
    ) -> Vec<Channel> {
        // nm_binder_perf_cache_channels creates the preferences.
        let p = self.package_or_create(name, uid, defaults);
        p.channels
            .iter()
            .filter(|c| include_deleted || !c.deleted)
            .filter(|c| include_bundles || !SYSTEM_RESERVED_IDS.contains(&c.id.as_str()))
            .cloned()
            .collect()
    }

    /// `deleteNotificationChannel`: whether it existed and was not deleted.
    pub fn delete_channel(&mut self, name: &str, uid: i32, id: &str, now_ms: i64) -> bool {
        let Some(c) = self.package_mut(name, uid).and_then(|p| p.channel_mut(id)) else {
            return false;
        };
        if c.deleted {
            return false;
        }
        c.deleted = true;
        c.deleted_time = now_ms;
        true
    }

    /// `getNotificationChannelGroupWithChannels`.
    pub fn group_with_channels(
        &self,
        name: &str,
        uid: i32,
        id: &str,
        include_deleted: bool,
    ) -> Option<Group> {
        let p = self.package(name, uid)?;
        let mut g = p.group(id)?.clone();
        g.channels = p
            .channels
            .iter()
            .filter(|c| (include_deleted || !c.deleted) && c.group.as_deref() == Some(id))
            .cloned()
            .collect();
        Some(g)
    }

    /// `getNotificationChannelGroups` with `Params.forAllGroups()`
    /// (`NotificationChannelGroupsHelper.getGroupsWithChannels`).
    pub fn groups_with_channels(&self, name: &str, uid: i32) -> Vec<Group> {
        let Some(p) = self.package(name, uid) else {
            return Vec::new();
        };
        let mut out: Vec<Group> = Vec::new();
        for c in p
            .channels
            .iter()
            .filter(|c| !c.deleted && !SYSTEM_RESERVED_IDS.contains(&c.id.as_str()))
        {
            let Some(id) = c.group.as_deref() else {
                continue;
            };
            let Some(g) = p.group(id) else { continue };
            match out.iter_mut().find(|o| o.id.as_deref() == Some(id)) {
                Some(o) => o.channels.push(c.clone()),
                None => {
                    let mut g = g.clone();
                    g.channels = vec![c.clone()];
                    array_map_insert(&mut out, g, |g| g.id.as_deref());
                }
            }
        }
        for g in &p.groups {
            if !out.iter().any(|o| o.id == g.id) {
                array_map_insert(&mut out, g.clone(), |g| g.id.as_deref());
            }
        }
        out
    }

    /// `deleteNotificationChannelGroup`: the channels it deleted.
    pub fn delete_group(&mut self, name: &str, uid: i32, id: &str, now_ms: i64) -> Vec<Channel> {
        let Some(p) = self.package_mut(name, uid) else {
            return Vec::new();
        };
        if id.is_empty() {
            return Vec::new();
        }
        p.groups.retain(|g| g.id.as_deref() != Some(id));
        let mut deleted = Vec::new();
        for c in p
            .channels
            .iter_mut()
            .filter(|c| c.group.as_deref() == Some(id))
        {
            if !c.deleted {
                c.deleted = true;
                c.deleted_time = now_ms;
            }
            deleted.push(c.clone());
        }
        deleted
    }

    /// `isGroupBlocked`.
    pub fn group_blocked(&self, name: &str, uid: i32, id: Option<&str>) -> bool {
        id.and_then(|id| self.package(name, uid)?.group(id).map(|g| g.blocked))
            .unwrap_or(false)
    }

    /// `canShowBadge`.
    pub fn can_show_badge(&self, name: &str, uid: i32) -> bool {
        self.package(name, uid).is_none_or(Package::show_badge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Defaults<'static> {
        Defaults {
            pre_o: &|name| name == "old.app",
            label: "Other",
        }
    }

    fn channel(id: &str, importance: i32) -> Channel {
        Channel::new(id, id, importance)
    }

    /// `ArrayMap` orders by `String.hashCode`, so channels come back in
    /// the original's order, not their creation's.
    #[test]
    fn channels_are_in_array_map_order() {
        let mut prefs = Preferences::default();
        for id in ["zeta", "alpha", "mid"] {
            prefs
                .create_channel(
                    "app",
                    10001,
                    channel(id, 3),
                    true,
                    false,
                    false,
                    &defaults(),
                )
                .unwrap();
        }
        let ids: Vec<String> = prefs
            .channels("app", 10001, false, true, &defaults())
            .into_iter()
            .map(|c| c.id)
            .collect();
        let mut expected = vec!["zeta", "alpha", "mid"];
        expected.sort_by_key(|s| java_hash(Some(s)));
        assert_eq!(ids, expected);
        assert_eq!(java_hash(Some("alpha")), 92909918);
    }

    /// An app may lower its channel's importance, not raise it, and its
    /// name and description follow the app.
    #[test]
    fn an_app_updates_its_channel_within_limits() {
        let mut prefs = Preferences::default();
        let d = defaults();
        assert!(
            prefs
                .create_channel("app", 10001, channel("c", 3), true, false, false, &d)
                .unwrap()
        );
        let mut louder = channel("c", 4);
        louder.name = Some("renamed".into());
        assert!(
            prefs
                .create_channel("app", 10001, louder, true, false, false, &d)
                .unwrap()
        );
        let c = prefs
            .package("app", 10001)
            .unwrap()
            .channel("c")
            .unwrap()
            .clone();
        assert_eq!(
            (c.importance, c.name.as_deref(), c.original_importance),
            (3, Some("renamed"), 3)
        );
        let mut quieter = channel("c", 2);
        quieter.name = Some("renamed".into());
        assert!(
            prefs
                .create_channel("app", 10001, quieter.clone(), true, false, false, &d)
                .unwrap()
        );
        assert_eq!(
            prefs
                .package("app", 10001)
                .unwrap()
                .channel("c")
                .unwrap()
                .importance,
            2
        );
        // Unchanged: no policy file change.
        assert!(
            !prefs
                .create_channel("app", 10001, quieter, true, false, false, &d)
                .unwrap()
        );
    }

    #[test]
    fn reserved_and_default_ids_are_refused() {
        let mut prefs = Preferences::default();
        let d = defaults();
        assert!(
            prefs
                .create_channel(
                    "app",
                    1,
                    channel(DEFAULT_CHANNEL_ID, 3),
                    true,
                    false,
                    false,
                    &d
                )
                .is_err()
        );
        assert!(
            !prefs
                .create_channel(
                    "app",
                    1,
                    channel("android.app.news", 3),
                    true,
                    false,
                    false,
                    &d
                )
                .unwrap()
        );
        let mut grouped = channel("g", 3);
        grouped.group = Some("missing".into());
        assert!(
            prefs
                .create_channel("app", 1, grouped, true, false, false, &d)
                .is_err()
        );
    }

    /// A package targeting an SDK before O gets the default channel.
    #[test]
    fn pre_o_apps_have_the_default_channel() {
        let mut prefs = Preferences::default();
        let d = defaults();
        let c = prefs
            .conversation_channel("old.app", 10002, None, None, true, false, &d)
            .unwrap();
        assert_eq!(
            (c.id.as_str(), c.name.as_deref()),
            (DEFAULT_CHANNEL_ID, Some("Other"))
        );
        assert!(
            prefs
                .conversation_channel("app", 10003, None, None, true, false, &d)
                .is_none()
        );
    }

    /// A deleted channel is hidden, comes back when the app creates it
    /// again, and a group's deletion deletes its channels.
    #[test]
    fn deletion_and_groups() {
        let mut prefs = Preferences::default();
        let d = defaults();
        let group = Group {
            id: Some("g".into()),
            name: Some("G".into()),
            description: None,
            channels: Vec::new(),
            blocked: true,
            user_locked_fields: 0,
        };
        assert!(prefs.create_group("app", 1, group, true, &d).unwrap());
        let mut c = channel("c", 3);
        c.group = Some("g".into());
        prefs
            .create_channel("app", 1, c.clone(), true, false, false, &d)
            .unwrap();
        prefs
            .create_channel("app", 1, channel("loose", 3), true, false, false, &d)
            .unwrap();
        let groups = prefs.groups_with_channels("app", 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].channels.len(), 1);
        assert!(!groups[0].blocked, "an app cannot block its group");
        assert!(prefs.delete_channel("app", 1, "loose", 5));
        assert!(!prefs.delete_channel("app", 1, "loose", 6));
        assert_eq!(prefs.channels("app", 1, false, true, &d).len(), 1);
        assert!(
            prefs
                .create_channel("app", 1, channel("loose", 3), true, false, false, &d)
                .unwrap()
        );
        assert_eq!(prefs.delete_group("app", 1, "g", 7).len(), 1);
        assert!(
            prefs
                .package("app", 1)
                .unwrap()
                .channel("c")
                .unwrap()
                .deleted
        );
    }

    /// A channel goes through `notification_policy.xml` and a parcel as
    /// the original writes them.
    #[test]
    fn channels_round_trip() {
        let mut c = channel("alarms", 4);
        c.description = Some("Alarms".into());
        c.group = Some("g".into());
        c.set_vibration_pattern(Some(vec![0, 250, 250, 250]));
        c.show_badge = true;
        let back = Channel::from_xml(&c.to_xml()).unwrap();
        assert_eq!(back, c);
        let mut p = Parcel::new();
        c.write_to(&mut p);
        let parsed = Channel::read_from(&mut p.reader()).unwrap();
        assert_eq!(parsed, c);
        assert_eq!(parsed.effect.unwrap().pattern, Some(vec![0, 250, 250, 250]));
    }

    /// A pattern's waveform has its off and on steps; a pattern of zeros
    /// has none (`createWaveform` throws).
    #[test]
    fn waveforms_follow_create_waveform() {
        assert!(Effect::waveform(&[0, 0]).is_none());
        let e = Effect::waveform(&[100, 200]).unwrap();
        let mut p = Parcel::new();
        p.write_raw_files(&e.raw, &[], &[]);
        let back = Effect::read(&mut p.reader()).unwrap();
        assert_eq!(back.pattern, Some(vec![100, 200]));
    }

    #[test]
    fn long_text_is_trimmed() {
        let c = Channel::new(&"x".repeat(1200), "n", 3);
        assert_eq!(c.id.len(), MAX_TEXT_LENGTH);
    }

    /// A delegate set is written enabled (no `enabled`); revoked, it is
    /// written disabled.
    #[test]
    fn delegates_are_set_and_revoked() {
        let ranking = aim_android_xml::read(br#"<ranking version="4" />"#).unwrap();
        let mut prefs = Preferences::read(&ranking, &defaults(), 0);
        prefs.set_delegate("app", 10001, "d", 10002, &defaults());
        let delegate = |prefs: &Preferences| {
            prefs
                .to_xml()
                .children()
                .next()
                .unwrap()
                .children()
                .find(|c| c.name == "delegate")
                .cloned()
                .unwrap()
        };
        let d = delegate(&prefs);
        assert_eq!(
            (
                d.string("name").as_deref(),
                d.int("uid").unwrap(),
                d.attr("enabled").is_none()
            ),
            (Some("d"), Some(10002), true)
        );
        prefs.revoke_delegate("app", 10001);
        assert_eq!(delegate(&prefs).bool("enabled").unwrap(), Some(false));
    }

    /// The `<ranking>` element is written back with what it held.
    #[test]
    fn ranking_round_trips() {
        let xml = br#"<ranking version="4"><silent_status_icons hide_gentle="true" /><package name="app" uid="10001" show_badge="true"><delegate name="d" uid="10002" /><channelGroup id="g" name="G" blocked="false" locked="0" /><channel id="c" name="C" importance="3" group="g" /></package></ranking>"#;
        let ranking = aim_android_xml::read(xml).unwrap();
        let prefs = Preferences::read(&ranking, &defaults(), 0);
        let out = prefs.to_xml();
        assert_eq!(out.children().count(), 2);
        let p = prefs.package("app", 10001).unwrap();
        assert_eq!(p.channel("c").unwrap().importance, 3);
        assert!(p.group("g").is_some());
        let package = out.children().nth(1).unwrap();
        assert_eq!(
            package
                .children()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["delegate", "channelGroup", "channel"]
        );
    }
}
