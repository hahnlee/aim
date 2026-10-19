//! `ZenModeHelper`'s config for the app host (`android-16.0.0_r1`): the
//! zen config of `/data/system/notification_policy.xml`
//! (`ZenModeConfig.readXml`, in its `modes_ui` form, #1647), as
//! `getZenModeConfig` returns it and `getAutomaticZenRules` its rules
//! (`zenRuleToAutomaticZenRule`), and the packages that
//! `ConditionProviders` approves for notification policy access
//! (`<dnd_apps>`). The rules are not run (#1488).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_android_xml::Element;
use aim_binder_host::local::LocalProcess;
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use crate::package::list_slice::Slice;
use crate::package::uri::Uri;

/// The classes `ParceledListSlice`s of `AzrWithId`s and `ZenRule`s create
/// their items with.
pub const AZR_WITH_ID_CLASS: &str = "android.app.AutomaticZenRule$AzrWithId";
const ZEN_RULE_CLASS: &str = "android.service.notification.ZenModeConfig$ZenRule";

/// `ZenPolicy.STATE_UNSET`, `STATE_ALLOW`, `STATE_DISALLOW`.
const STATE_UNSET: i32 = 0;
const STATE_ALLOW: i32 = 1;
const STATE_DISALLOW: i32 = 2;
/// `ZenPolicy.PEOPLE_TYPE_*`, `CONVERSATION_SENDERS_*`.
const PEOPLE_TYPE_ANYONE: i32 = 1;
const PEOPLE_TYPE_STARRED: i32 = 3;
const PEOPLE_TYPE_NONE: i32 = 4;
const CONVERSATION_SENDERS_ANYONE: i32 = 1;
const CONVERSATION_SENDERS_IMPORTANT: i32 = 2;
const CONVERSATION_SENDERS_NONE: i32 = 3;
/// `ZenPolicy.CHANNEL_POLICY_PRIORITY`, `CHANNEL_POLICY_NONE`.
const CHANNEL_POLICY_PRIORITY: i32 = 1;
const CHANNEL_POLICY_NONE: i32 = 2;
/// `ZenPolicy.PRIORITY_CATEGORY_*`.
const REMINDERS: usize = 0;
const EVENTS: usize = 1;
const MESSAGES: usize = 2;
const CALLS: usize = 3;
const REPEAT_CALLERS: usize = 4;
const ALARMS: usize = 5;
const MEDIA: usize = 6;
const SYSTEM: usize = 7;
const CONVERSATIONS: usize = 8;
/// `Global.ZEN_MODE_IMPORTANT_INTERRUPTIONS`, `ZEN_MODE_NO_INTERRUPTIONS`.
const ZEN_MODE_IMPORTANT_INTERRUPTIONS: i32 = 1;
const ZEN_MODE_NO_INTERRUPTIONS: i32 = 3;
/// `AutomaticZenRule.TYPE_UNKNOWN`, `MAX_STRING_LENGTH`.
const TYPE_UNKNOWN: i32 = -1;
const MAX_STRING_LENGTH: usize = 500;
/// `Condition.MAX_STRING_LENGTH`.
const CONDITION_MAX_STRING_LENGTH: usize = 1000;
/// `ZenModeConfig.ORIGIN_UNKNOWN`, `ZenRule.OVERRIDE_NONE`.
const ORIGIN_UNKNOWN: i32 = 0;
const OVERRIDE_NONE: i32 = 0;
/// `Parcel.VAL_STRING`, `VAL_INTEGER`.
const VAL_STRING: i32 = 0;
const VAL_INTEGER: i32 = 1;

/// `ZenPolicy`: its priority categories and visual effects (`STATE_*`),
/// then its message and call senders, conversation senders and channels.
#[derive(Clone, Debug, Default, PartialEq)]
struct ZenPolicy {
    categories: [i32; 9],
    effects: [i32; 7],
    messages: i32,
    calls: i32,
    conversations: i32,
    channels: i32,
}

impl WriteParcelable for ZenPolicy {
    fn write_to(&self, p: &mut Parcel) {
        // `writeList` of `Integer`s.
        for list in [&self.categories[..], &self.effects[..]] {
            p.write_i32(list.len() as i32);
            for v in list {
                p.write_i32(VAL_INTEGER);
                p.write_i32(*v);
            }
        }
        p.write_i32(self.messages);
        p.write_i32(self.calls);
        p.write_i32(self.conversations);
        p.write_i32(self.channels);
    }
}

/// `ZenDeviceEffects`: its effects in parcel order, and its extra ones in
/// the order their `HashSet` keeps.
#[derive(Clone, Debug, PartialEq)]
struct ZenDeviceEffects {
    effects: [bool; 11],
    extras: Vec<String>,
}

impl WriteParcelable for ZenDeviceEffects {
    fn write_to(&self, p: &mut Parcel) {
        for e in self.effects {
            p.write_bool(e);
        }
        // `writeArray` of a `String[]`.
        p.write_i32(self.extras.len() as i32);
        for e in &self.extras {
            p.write_i32(VAL_STRING);
            p.write_string16(Some(e));
        }
    }
}

/// A `Condition` (`readConditionXml`), its id and texts trimmed.
#[derive(Clone, Debug, PartialEq)]
struct Condition {
    id: String,
    summary: String,
    line1: Option<String>,
    line2: Option<String>,
    icon: i32,
    state: i32,
    source: i32,
    flags: i32,
}

impl WriteParcelable for Condition {
    fn write_to(&self, p: &mut Parcel) {
        write_uri(p, &self.id);
        p.write_string16(Some(&self.summary));
        p.write_string16(self.line1.as_deref());
        p.write_string16(self.line2.as_deref());
        p.write_i32(self.icon);
        p.write_i32(self.state);
        p.write_i32(self.source);
        p.write_i32(self.flags);
    }
}

/// A `ZenRule` (`readRuleXml`).
#[derive(Clone, Debug, PartialEq)]
pub struct ZenRule {
    id: Option<String>,
    enabled: bool,
    name: Option<String>,
    zen_mode: i32,
    condition_id: Option<String>,
    condition: Option<Condition>,
    component: Option<(String, String)>,
    configuration_activity: Option<(String, String)>,
    pub pkg: Option<String>,
    creation_time: i64,
    enabler: Option<String>,
    policy: Option<ZenPolicy>,
    effects: Option<ZenDeviceEffects>,
    allow_manual_invocation: bool,
    icon_res_name: Option<String>,
    trigger_description: Option<String>,
    kind: i32,
    user_modified_fields: i32,
    policy_user_modified_fields: i32,
    effects_user_modified_fields: i32,
    deletion_instant: Option<i64>,
    disabled_origin: i32,
    legacy_suppressed_effects: i32,
    condition_override: i32,
    last_activation: Option<i64>,
}

impl WriteParcelable for ZenRule {
    fn write_to(&self, p: &mut Parcel) {
        let optional = |p: &mut Parcel, s: &Option<String>| match s {
            Some(s) => {
                p.write_i32(1);
                p.write_string8(Some(s));
            }
            None => p.write_i32(0),
        };
        p.write_i32(i32::from(self.enabled));
        // `snoozing`, which no config persists.
        p.write_i32(0);
        optional(p, &self.name);
        p.write_i32(self.zen_mode);
        match &self.condition_id {
            Some(id) => write_uri(p, id),
            None => p.write_string16(None),
        }
        parcelable(
            p,
            "android.service.notification.Condition",
            self.condition.as_ref(),
        );
        write_component(p, self.component.as_ref(), false);
        write_component(p, self.configuration_activity.as_ref(), false);
        optional(p, &self.id);
        p.write_i64(self.creation_time);
        optional(p, &self.enabler);
        parcelable(
            p,
            "android.service.notification.ZenPolicy",
            self.policy.as_ref(),
        );
        parcelable(
            p,
            "android.service.notification.ZenDeviceEffects",
            self.effects.as_ref(),
        );
        p.write_string8(self.pkg.as_deref());
        p.write_bool(self.allow_manual_invocation);
        p.write_string8(self.icon_res_name.as_deref());
        p.write_string8(self.trigger_description.as_deref());
        p.write_i32(self.kind);
        p.write_i32(self.user_modified_fields);
        p.write_i32(self.policy_user_modified_fields);
        p.write_i32(self.effects_user_modified_fields);
        let instant = |p: &mut Parcel, ms: Option<i64>| match ms {
            Some(ms) => {
                p.write_i32(1);
                p.write_i64(ms);
            }
            None => p.write_i32(0),
        };
        instant(p, self.deletion_instant);
        // `modes_ui` and `modes_cleanup_implicit` on (#1647).
        p.write_i32(self.disabled_origin);
        p.write_i32(self.legacy_suppressed_effects);
        p.write_i32(self.condition_override);
        instant(p, self.last_activation);
    }
}

/// `Parcel.writeParcelable` of `value`, whose class is `class`.
fn parcelable(p: &mut Parcel, class: &str, value: Option<&impl WriteParcelable>) {
    match value {
        Some(value) => {
            p.write_string16(Some(class));
            value.write_to(p);
        }
        None => p.write_string16(None),
    }
}

/// `writeParcelable` of `Uri.parse(s)`.
fn write_uri(p: &mut Parcel, s: &str) {
    let uri = Uri::parse(s);
    p.write_string16(Some(uri.class_name()));
    uri.write(p);
}

/// `writeParcelable` of a `ComponentName`, its names trimmed for an
/// `AutomaticZenRule`.
fn write_component(p: &mut Parcel, component: Option<&(String, String)>, trim: bool) {
    match component {
        Some((package, class)) => {
            p.write_string16(Some("android.content.ComponentName"));
            let name = |s: &str| {
                if trim {
                    trimmed(s, MAX_STRING_LENGTH)
                } else {
                    s.to_owned()
                }
            };
            p.write_string16(Some(&name(package)));
            p.write_string16(Some(&name(class)));
        }
        None => p.write_string16(None),
    }
}

/// `ZenModeConfig` (`modes_ui` on, #1647), in `writeToParcel`'s order.
pub struct ZenModeConfig {
    user: i32,
    manual: ZenRule,
    automatic: Vec<ZenRule>,
    /// Keyed by `deletedRuleKey`.
    deleted: Vec<(String, ZenRule)>,
    has_priority_channels: bool,
}

/// A config as written to a parcel, its rules in `ParceledListSlice`s of
/// `process`.
pub struct ConfigParcel(pub ZenModeConfig, pub Arc<LocalProcess>);

impl WriteParcelable for ConfigParcel {
    fn write_to(&self, p: &mut Parcel) {
        let ConfigParcel(config, process) = self;
        p.write_i32(config.user);
        parcelable(p, ZEN_RULE_CLASS, Some(&config.manual));
        let automatic: Vec<_> = config
            .automatic
            .iter()
            .map(|r| (r.id.clone().unwrap_or_default(), r.clone()))
            .collect();
        for rules in [automatic, config.deleted.clone()] {
            // `writeRulesToParcel`.
            p.write_i32(rules.len() as i32);
            if rules.is_empty() {
                continue;
            }
            p.write_i32(rules.len() as i32);
            for (key, _) in &rules {
                p.write_string8(Some(key));
            }
            p.write_string16(Some("android.content.pm.ParceledListSlice"));
            Slice::new(
                process.clone(),
                ZEN_RULE_CLASS,
                rules.into_iter().map(|(_, r)| r).collect(),
            )
            .write_to(p);
        }
        p.write_i32(i32::from(config.has_priority_channels));
    }
}

/// `AutomaticZenRule.AzrWithId` of a rule.
pub struct AzrWithId(ZenRule);

impl WriteParcelable for AzrWithId {
    fn write_to(&self, p: &mut Parcel) {
        let rule = &self.0;
        p.write_string8(rule.id.as_deref());
        p.write_string16(Some("android.app.AutomaticZenRule"));
        p.write_i32(i32::from(rule.enabled));
        // The constructor's trimmed name, a `Uri.parse` of the trimmed
        // condition id, and trimmed components.
        p.write_i32(1);
        p.write_string8(
            rule.name
                .as_deref()
                .map(|n| trimmed(n, MAX_STRING_LENGTH))
                .as_deref(),
        );
        p.write_i32(match rule.zen_mode {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 3,
            _ => 0,
        });
        let condition = trimmed(
            rule.condition_id.as_deref().unwrap_or_default(),
            MAX_STRING_LENGTH,
        );
        write_uri(p, &condition);
        write_component(p, rule.component.as_ref(), true);
        write_component(p, rule.configuration_activity.as_ref(), true);
        p.write_i64(rule.creation_time);
        parcelable(
            p,
            "android.service.notification.ZenPolicy",
            rule.policy.as_ref(),
        );
        p.write_string8(rule.pkg.as_deref());
        parcelable(
            p,
            "android.service.notification.ZenDeviceEffects",
            rule.effects.as_ref(),
        );
        p.write_bool(rule.allow_manual_invocation);
        // `drawableResNameToResId`: none without a name.
        p.write_i32(0);
        p.write_string8(rule.trigger_description.as_deref());
        p.write_i32(rule.kind);
    }
}

/// `getTrimmedString`: the first `length` UTF-16 units.
fn trimmed(s: &str, length: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() <= length {
        return s.to_owned();
    }
    String::from_utf16_lossy(&units[..length])
}

/// `getAttributeInt(null, name, default)`.
fn int(e: &Element, name: &str, default: i32) -> i32 {
    e.int(name).ok().flatten().unwrap_or(default)
}

/// `getAttributeBoolean(null, name, default)`.
fn boolean(e: &Element, name: &str, default: bool) -> bool {
    e.bool(name).ok().flatten().unwrap_or(default)
}

/// `ComponentName.unflattenFromString` of a non-empty attribute.
fn component(e: &Element, name: &str) -> Option<(String, String)> {
    let c = crate::package::preferred::unflatten(&e.string(name)?)?;
    Some((c.package, c.class))
}

/// `ZenModeConfig.readZenPolicyXml`: none when no attribute sets one.
fn read_policy(e: &Element) -> Option<ZenPolicy> {
    let mut policy = ZenPolicy::default();
    let mut set = false;
    let state = |allow: bool| if allow { STATE_ALLOW } else { STATE_DISALLOW };
    let channels = int(e, "priorityChannelsAllowed", STATE_UNSET);
    if channels != STATE_UNSET {
        policy.channels = if channels == STATE_ALLOW {
            CHANNEL_POLICY_PRIORITY
        } else {
            CHANNEL_POLICY_NONE
        };
        set = true;
    }
    // `allowCalls`, `allowMessages`: a known audience sets the category.
    for (attr, category) in [("callsFrom", CALLS), ("messagesFrom", MESSAGES)] {
        let audience = int(e, attr, 0);
        if audience == 0 {
            continue;
        }
        set = true;
        if audience == PEOPLE_TYPE_NONE
            || (PEOPLE_TYPE_ANYONE..=PEOPLE_TYPE_STARRED).contains(&audience)
        {
            policy.categories[category] = state(audience != PEOPLE_TYPE_NONE);
            *if category == CALLS {
                &mut policy.calls
            } else {
                &mut policy.messages
            } = audience;
        }
    }
    let repeat_callers = int(e, "repeatCallers", STATE_UNSET);
    if repeat_callers != STATE_UNSET {
        policy.categories[REPEAT_CALLERS] = state(repeat_callers == STATE_ALLOW);
        set = true;
    }
    let conversations = int(e, "convosFrom", 0);
    if conversations != 0 {
        set = true;
        if matches!(
            conversations,
            CONVERSATION_SENDERS_ANYONE
                | CONVERSATION_SENDERS_IMPORTANT
                | CONVERSATION_SENDERS_NONE
        ) {
            policy.categories[CONVERSATIONS] = state(conversations != CONVERSATION_SENDERS_NONE);
            policy.conversations = conversations;
        }
    }
    for (attr, category) in [
        ("alarms", ALARMS),
        ("media", MEDIA),
        ("system", SYSTEM),
        ("events", EVENTS),
        ("reminders", REMINDERS),
    ] {
        let v = int(e, attr, STATE_UNSET);
        if v != STATE_UNSET {
            policy.categories[category] = state(v == STATE_ALLOW);
            set = true;
        }
    }
    // `VISUAL_EFFECT_*` in order.
    let effects = [
        "showFullScreenIntent",
        "showLights",
        "shoePeek",
        "showStatusBarIcons",
        "showBadges",
        "showAmbient",
        "showNotificationList",
    ];
    for (i, attr) in effects.iter().enumerate() {
        let v = int(e, attr, STATE_UNSET);
        if v != STATE_UNSET {
            policy.effects[i] = state(v == STATE_ALLOW);
            set = true;
        }
    }
    set.then_some(policy)
}

/// `ZenModeConfig.readZenDeviceEffectsXml`: none without an effect.
fn read_effects(e: &Element) -> Option<ZenDeviceEffects> {
    const EFFECTS: [&str; 11] = [
        "zdeDisplayGrayscale",
        "zdeSuppressAmbientDisplay",
        "zdeDimWallpaper",
        "zdeUseNightMode",
        "zdeDisableAutoBrightness",
        "zdeDisableTapToWake",
        "zdeDisableTiltToWake",
        "zdeDisableTouch",
        "zdeMinimizeRadioUsage",
        "zdeMaximizeDoze",
        "zdeUseNightLight",
    ];
    let effects = EFFECTS.map(|name| boolean(e, name, false));
    let extras = string_set(e.string("zdeExtraEffects").as_deref().unwrap_or_default());
    (effects.iter().any(|e| *e) || !extras.is_empty())
        .then_some(ZenDeviceEffects { effects, extras })
}

/// `safeStringSet`: the items split on each `,` no `\` escapes, unescaped,
/// in the order a `HashSet` of them iterates (each a builder's set after
/// another of the same size).
fn string_set(serialized: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    if serialized.is_empty() {
        return items;
    }
    let mut item = String::new();
    let mut previous = None;
    for c in serialized.chars() {
        if c == ',' && previous != Some('\\') {
            items.push(std::mem::take(&mut item));
        } else {
            item.push(c);
        }
        previous = Some(c);
    }
    items.push(item);
    let mut set: Vec<String> = Vec::new();
    for item in items {
        let item = item.replace("\\\\", "\\").replace("\\,", ",");
        if !set.contains(&item) {
            set.push(item);
        }
    }
    hash_order(set, |s| s)
}

/// The order a `HashMap` or `HashSet` of default capacity iterates `items`
/// added in their order: by bucket, then as added.
fn hash_order<T>(mut items: Vec<T>, key: impl Fn(&T) -> &str) -> Vec<T> {
    let mut capacity = 16usize;
    while items.len() > capacity * 3 / 4 {
        capacity *= 2;
    }
    items.sort_by_key(|item| {
        let hash = crate::package::info::java_hash(key(item)) as u32;
        (hash ^ (hash >> 16)) as usize & (capacity - 1)
    });
    items
}

/// `ZenModeConfig.readConditionXml`: none without an id, or for an
/// invalid one.
fn read_condition(e: &Element) -> Option<Condition> {
    let id = e.string("id")?;
    let summary = e.string("summary")?;
    let state = int(e, "state", -1);
    let source = int(e, "source", 0);
    if !(0..=3).contains(&state) || !(0..=3).contains(&source) {
        return None;
    }
    let text = |name: &str| {
        e.string(name)
            .map(|s| trimmed(&s, CONDITION_MAX_STRING_LENGTH))
    };
    Some(Condition {
        id: trimmed(&id, CONDITION_MAX_STRING_LENGTH),
        summary: trimmed(&summary, CONDITION_MAX_STRING_LENGTH),
        line1: text("line1"),
        line2: text("line2"),
        icon: int(e, "icon", -1),
        state,
        source,
        flags: int(e, "flags", -1),
    })
}

/// `ZenModeConfig.readRuleXml` (`modes_ui` and `modes_cleanup_implicit`
/// on).
fn read_rule(e: &Element) -> ZenRule {
    let owner = component(e, "component");
    let zen = int(e, "zen", ZEN_MODE_IMPORTANT_INTERRUPTIONS);
    let string = |name: &str| e.string(name).map(|s| s.into_owned());
    // `safeInstant`: a long, else none.
    let instant = |name: &str| e.string(name).and_then(|s| s.parse().ok());
    ZenRule {
        id: None,
        enabled: boolean(e, "enabled", true),
        name: string("name"),
        zen_mode: if (0..=ZEN_MODE_NO_INTERRUPTIONS).contains(&zen) {
            zen
        } else {
            ZEN_MODE_IMPORTANT_INTERRUPTIONS
        },
        condition_id: string("conditionId"),
        condition: read_condition(e),
        pkg: string("pkg").or_else(|| owner.as_ref().map(|c| c.0.clone())),
        component: owner,
        configuration_activity: component(e, "configActivity"),
        creation_time: e.long("creationTime").ok().flatten().unwrap_or(0),
        enabler: string("enabler"),
        policy: read_policy(e),
        effects: read_effects(e),
        allow_manual_invocation: boolean(e, "userInvokable", false),
        icon_res_name: string("rule_icon"),
        trigger_description: string("triggerDesc"),
        kind: int(e, "type", TYPE_UNKNOWN),
        user_modified_fields: int(e, "userModifiedFields", 0),
        policy_user_modified_fields: int(e, "policyUserModifiedFields", 0),
        effects_user_modified_fields: int(e, "zdeUserModifiedFields", 0),
        deletion_instant: instant("deletionInstant"),
        disabled_origin: int(e, "disabledOrigin", ORIGIN_UNKNOWN),
        legacy_suppressed_effects: int(e, "legacySuppressedEffects", 0),
        condition_override: int(e, "conditionOverride", OVERRIDE_NONE),
        last_activation: instant("lastActivation"),
    }
}

/// `ZenModeConfig.readXml` of the zen config of `policy`, the root of
/// `notification_policy.xml`: its rules in the order of the config's
/// `ArrayMap`s. Only the `modes_ui` form, with a manual rule that has a
/// policy, is read (#1647).
pub fn read(policy: &Element) -> Result<ZenModeConfig, Exception> {
    let unsupported = |what: &str| {
        Exception::new(
            EX_UNSUPPORTED_OPERATION,
            format!("notification: {what} (#1647)"),
        )
    };
    let zen = policy
        .children()
        .find(|e| e.name == "zen")
        .ok_or_else(|| unsupported("the default zen config"))?;
    let mut manual = None;
    let (mut automatic, mut deleted) = (Vec::new(), Vec::new());
    let mut has_priority_channels = false;
    for e in zen.children() {
        match e.name.as_str() {
            "manual" => {
                let mut rule = read_rule(e);
                rule.enabled = true;
                manual = Some(rule);
            }
            "automatic" | "deleted" => {
                let Some(id) = e.string("ruleId") else {
                    continue;
                };
                let rule = ZenRule {
                    id: Some(id.into_owned()),
                    ..read_rule(e)
                };
                if e.name == "automatic" {
                    automatic.push(rule);
                } else if let (Some(pkg), Some(condition)) = (&rule.pkg, &rule.condition_id) {
                    // `deletedRuleKey`.
                    deleted.push((format!("{pkg}|{condition}"), rule));
                }
            }
            "state" => has_priority_channels = boolean(e, "areChannelsBypassingDnd", false),
            _ => {}
        }
    }
    let manual = manual
        .filter(|m| m.policy.is_some())
        .ok_or_else(|| unsupported("a zen config before modes_ui"))?;
    Ok(ZenModeConfig {
        user: int(zen, "user", 0),
        manual,
        automatic: crate::package::info::array_order(automatic, |r: &ZenRule| {
            r.id.as_deref().unwrap_or_default()
        }),
        deleted: crate::package::info::array_order(deleted, |(k, _): &(String, ZenRule)| {
            k.as_str()
        }),
        has_priority_channels,
    })
}

impl ZenModeConfig {
    /// Its automatic rules.
    pub fn rules(self) -> Vec<ZenRule> {
        self.automatic
    }
}

/// `getAutomaticZenRules` of `rules` a caller may manage: each as an
/// `AutomaticZenRule` with its id, in the order of the `HashMap` that
/// collects them.
pub fn automatic_zen_rules(rules: Vec<ZenRule>) -> Result<Vec<AzrWithId>, Exception> {
    for rule in &rules {
        // `AutomaticZenRule.Builder` takes a name and condition id.
        if rule.name.is_none() || rule.condition_id.is_none() {
            return Err(Exception::new(EX_NULL_POINTER, "AutomaticZenRule.Builder"));
        }
        if rule.icon_res_name.as_deref().is_some_and(|n| !n.is_empty()) {
            return Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "notification: a zen rule's icon by name (#1646)",
            ));
        }
    }
    Ok(hash_order(rules, |r| r.id.as_deref().unwrap_or_default())
        .into_iter()
        .map(AzrWithId)
        .collect())
}

/// `ConditionProviders.isPackageOrComponentAllowed`: whether `<dnd_apps>`
/// approves `package` for `user`.
pub fn policy_access_approved(policy: &Element, package: &str, user: i32) -> bool {
    policy
        .children()
        .filter(|e| e.name == "dnd_apps")
        .flat_map(|e| e.children())
        .filter(|l| l.name == "service_listing" && int(l, "user", 0) == user)
        .any(|l| {
            l.string("approved")
                .is_some_and(|a| a.split(':').any(|p| p == package))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule's attributes, policy and effects read as `readRuleXml` reads
    /// them, and the approved packages.
    #[test]
    fn reads_rules_and_approvals_as_the_original() {
        let xml = br#"<notification-policy version="1"><zen version="12" user="0">
<manual enabled="false" zen="0" pkg="android" conditionId="" creationTime="0" callsFrom="3" alarms="1" type="0" />
<automatic ruleId="r1" enabled="false" name="Bedtime" zen="1" pkg="p" configActivity="p/.A" conditionId="condition://p/w" creationTime="17" id="condition://p/w" summary="s" line1="" icon="-1" state="0" source="0" flags="2" callsFrom="3" messagesFrom="4" repeatCallers="1" convosFrom="3" alarms="1" system="2" showStatusBarIcons="2" priorityChannelsAllowed="1" zdeDimWallpaper="true" userInvokable="true" triggerDesc="t" type="3" disabledOrigin="4" />
<automatic ruleId="r2" name="n" conditionId="c" id="c" state="9" />
<automatic name="no id" />
<deleted ruleId="r3" pkg="q" conditionId="condition://q/x" deletionInstant="5" />
<state areChannelsBypassingDnd="true" />
</zen><dnd_apps version="4"><service_listing approved="a:p" user="0" primary="true" /></dnd_apps></notification-policy>"#;
        let root = aim_android_xml::read(xml).unwrap();
        let config = read(&root).unwrap();
        assert!(
            config.manual.enabled && config.manual.policy.is_some() && config.manual.id.is_none()
        );
        assert_eq!(config.deleted.len(), 1);
        assert_eq!(
            (
                config.deleted[0].0.as_str(),
                config.deleted[0].1.deletion_instant
            ),
            ("q|condition://q/x", Some(5))
        );
        assert!(config.has_priority_channels);
        let rules = config.rules();
        assert_eq!(rules.len(), 2);
        let r1 = rules
            .iter()
            .find(|r| r.id.as_deref() == Some("r1"))
            .unwrap();
        assert!(!r1.enabled && r1.allow_manual_invocation);
        assert_eq!(
            (
                r1.disabled_origin,
                r1.condition
                    .as_ref()
                    .map(|c| (c.flags, c.line1.clone(), c.line2.clone()))
            ),
            (4, Some((2, Some(String::new()), None)))
        );
        assert_eq!(r1.configuration_activity, Some(("p".into(), "p.A".into())));
        let policy = r1.policy.as_ref().unwrap();
        assert_eq!(
            policy.categories,
            [
                0,
                0,
                STATE_DISALLOW,
                STATE_ALLOW,
                STATE_ALLOW,
                STATE_ALLOW,
                0,
                STATE_DISALLOW,
                STATE_DISALLOW
            ]
        );
        assert_eq!(
            (
                policy.messages,
                policy.calls,
                policy.conversations,
                policy.channels
            ),
            (4, 3, 3, CHANNEL_POLICY_PRIORITY)
        );
        assert_eq!(policy.effects, [0, 0, 0, STATE_DISALLOW, 0, 0, 0]);
        assert!(r1.effects.as_ref().unwrap().effects[2]);
        let r2 = rules
            .iter()
            .find(|r| r.id.as_deref() == Some("r2"))
            .unwrap();
        // A condition with an invalid state is none.
        assert!(
            r2.enabled && r2.policy.is_none() && r2.effects.is_none() && r2.condition.is_none()
        );
        assert_eq!(
            (r2.zen_mode, r2.kind),
            (ZEN_MODE_IMPORTANT_INTERRUPTIONS, TYPE_UNKNOWN)
        );
        assert!(policy_access_approved(&root, "p", 0) && !policy_access_approved(&root, "q", 0));
    }

    /// A config of before modes_ui, whose manual rule has no policy, is
    /// not read.
    #[test]
    fn a_manual_rule_without_policy_is_not_read() {
        let xml = br#"<notification-policy><zen version="8"><manual enabled="true" zen="1" /></zen></notification-policy>"#;
        assert!(read(&aim_android_xml::read(xml).unwrap()).is_err());
    }

    /// `safeStringSet` splits on each comma no backslash precedes (one
    /// after an escaped backslash too, as its look-behind does), then
    /// unescapes.
    #[test]
    fn string_sets_split_as_the_original() {
        let mut set = string_set(r"a\,b,c\\,a\,b,d,d");
        set.sort();
        assert_eq!(set, ["a,b", "c,a,b", "d"]);
    }
}
