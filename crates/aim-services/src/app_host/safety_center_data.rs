//! The sources' data of the app host's `safety_center` (packages/modules/
//! Permission at `android-16.0.0_r1`): `SafetySourceData` and `SafetyEvent`
//! as their creators read them, `SafetySourceDataValidator`'s checks,
//! `SafetySourceDataRepository`, `SafetyCenterIssueDismissalRepository`
//! with its file (`SafetyCenterIssuesPersistence`), the deduplicated issues
//! of `SafetyCenterIssueRepository` and the issues
//! `SafetyCenterNotificationSender` would notify now.
//!
//! The app host starts no refresh and runs no issue action, so a source's
//! event changes nothing (`processSafetyEvent`), no in-flight action fixes
//! an event (`SafetyEventFix`) and no listener is registered. Texts are
//! kept without their spans; a source's extras that hold binders or files
//! are not handed back. Posting notifications, the default action override
//! (`DefaultActionOverrideFix`) and profiles are not implemented (#1785).
//! The issues' file lists them in the order they were first seen, where
//! the original's `ArrayMap` orders them by their keys' hashes.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::safety_center_config::{
    SafetyCenterConfig, SafetySource, SafetySourcesGroup, TYPE_DYNAMIC, TYPE_ISSUE_ONLY,
    TYPE_STATIC,
};
use crate::clip::{char_sequence, write_char_sequence};

/// `SafetySourceData.SEVERITY_LEVEL_*`.
pub const SEVERITY_UNSPECIFIED: i32 = 100;
pub const SEVERITY_INFORMATION: i32 = 200;
/// `SafetyEvent.SAFETY_EVENT_TYPE_*`.
pub const EVENT_RESOLVING_ACTION_SUCCEEDED: i32 = 300;
pub const EVENT_RESOLVING_ACTION_FAILED: i32 = 400;
/// `SafetySourceIssue.NOTIFICATION_BEHAVIOR_*`.
const BEHAVIOR_UNSPECIFIED: i32 = 0;
const BEHAVIOR_DELAYED: i32 = 200;
const BEHAVIOR_IMMEDIATELY: i32 = 300;
/// `SafetySource.PROFILE_ALL`, `INITIAL_DISPLAY_STATE_HIDDEN` and
/// `SafetySourcesGroup.SAFETY_SOURCES_GROUP_TYPE_STATELESS`.
const PROFILE_ALL: i32 = 2;
const DISPLAY_HIDDEN: i32 = 2;
const GROUP_STATELESS: i32 = 1;
/// `SafetyCenterFlags`' defaults.
const RESURFACE_ISSUE_MAX_COUNT_DEFAULT: &str = "200:0,300:1,400:1";
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
const RESURFACE_ISSUE_DELAY_DEFAULT_MILLIS: i64 = 180 * DAY_MILLIS;
const TEMP_HIDDEN_RESURFACE_DELAY_DEFAULT_MILLIS: i64 = 2 * DAY_MILLIS;
const NOTIFICATIONS_MIN_DELAY_DEFAULT_MILLIS: i64 = 180 * DAY_MILLIS;
/// `sAllowedNotificationSources` (with `odad_notifications_supported`)
/// and `sAllowedNotificationSourcesUPlus`.
const ODAD_NOTIFICATION_SOURCES: &[&str] = &["GoogleAppProtectionService"];
const U_PLUS_NOTIFICATION_SOURCES: &[&str] = &["GoogleBackupAndRestore"];
/// The issues' file, in the module's device protected data
/// (`ISSUE_DISMISSAL_REPOSITORY_FILE_NAME`), and its version.
pub const ISSUES_FILE: &str = "misc/apexdata/com.android.permission/safety_center_issues.xml";
const ISSUES_VERSION: i32 = 2;

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A `writeTypedObject`'s null marker: whether a value follows.
fn present(r: &mut Reader<'_>) -> ParcelResult<bool> {
    Ok(r.read_i32()? != 0)
}

/// `createTypedArrayList`: a null list, or one with a null element, is
/// refused as the creators refuse it (`requireNonNull`).
fn typed_list<T>(
    r: &mut Reader<'_>,
    item: impl Fn(&mut Reader<'_>) -> ParcelResult<T>,
) -> ParcelResult<Vec<T>> {
    let n = r.read_i32()?;
    if n < 0 {
        return Err(BAD_VALUE);
    }
    (0..n)
        .map(|_| if present(r)? { item(r) } else { Err(BAD_VALUE) })
        .collect()
}

fn write_typed_list<T>(p: &mut Parcel, items: &[T], item: impl Fn(&mut Parcel, &T)) {
    p.write_i32(items.len() as i32);
    for i in items {
        p.write_i32(1);
        item(p, i);
    }
}

/// A `PendingIntent` as a typed object: its `IIntentSender`.
fn pending_intent(r: &mut Reader<'_>) -> ParcelResult<Option<Binder>> {
    if present(r)? {
        r.read_binder()?.map(Some).ok_or(BAD_VALUE)
    } else {
        Ok(None)
    }
}

fn write_pending_intent(p: &mut Parcel, target: &Option<Binder>) {
    match target {
        Some(b) => {
            p.write_i32(1);
            p.write_binder(Some(b.clone()));
        }
        None => p.write_i32(0),
    }
}

fn text(r: &mut Reader<'_>) -> ParcelResult<Option<String>> {
    char_sequence(r)
}

fn write_text(p: &mut Parcel, t: &Option<String>) {
    match t {
        Some(t) => write_char_sequence(p, Some(t)),
        // `TextUtils.writeToParcel(null)`.
        None => {
            p.write_i32(1);
            p.write_string8(None);
        }
    }
}

/// `SafetySourceStatus`.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub severity: i32,
    pub pending_intent: Option<Binder>,
    /// `IconAction`: its type and intent.
    pub icon_action: Option<(i32, Option<Binder>)>,
    pub enabled: bool,
}

/// `SafetySourceIssue.Action`.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: Option<String>,
    pub label: Option<String>,
    pub pending_intent: Option<Binder>,
    pub will_resolve: bool,
    pub success_message: Option<String>,
    /// `ConfirmationDialogDetails`: title, text, accept and deny texts.
    pub confirmation: Option<[Option<String>; 4]>,
}

/// `SafetySourceIssue.Notification`.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub title: Option<String>,
    pub text: Option<String>,
    pub actions: Vec<Action>,
}

/// `SafetySourceIssue`.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub id: Option<String>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub summary: Option<String>,
    pub severity: i32,
    pub category: i32,
    pub actions: Vec<Action>,
    pub on_dismiss: Option<Binder>,
    pub type_id: Option<String>,
    pub notification: Option<Notification>,
    pub notification_behavior: i32,
    pub attribution_title: Option<String>,
    pub deduplication_id: Option<String>,
    pub actionability: i32,
}

/// `SafetySourceData` as its creator reads it (Upside Down Cake and
/// later); equal as `equals` compares it, without its extras.
#[derive(Clone, Debug)]
pub struct SourceData {
    pub status: Option<Status>,
    pub issues: Vec<Issue>,
    /// `mExtras` as written (`writeBundle`), and whether it holds binders.
    extras: (Vec<u8>, bool),
}

impl PartialEq for SourceData {
    fn eq(&self, other: &Self) -> bool {
        self.status == other.status && self.issues == other.issues
    }
}

fn action(r: &mut Reader<'_>) -> ParcelResult<Action> {
    Ok(Action {
        id: r.read_string16()?,
        label: text(r)?,
        pending_intent: pending_intent(r)?,
        will_resolve: r.read_bool()?,
        success_message: text(r)?,
        confirmation: if present(r)? {
            Some([text(r)?, text(r)?, text(r)?, text(r)?])
        } else {
            None
        },
    })
}

fn write_action(p: &mut Parcel, a: &Action) {
    p.write_string16(a.id.as_deref());
    write_text(p, &a.label);
    write_pending_intent(p, &a.pending_intent);
    p.write_bool(a.will_resolve);
    write_text(p, &a.success_message);
    match &a.confirmation {
        Some(texts) => {
            p.write_i32(1);
            for t in texts {
                write_text(p, t);
            }
        }
        None => p.write_i32(0),
    }
}

impl ReadParcelable for SourceData {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let status = if present(r)? {
            Some(Status {
                title: text(r)?,
                summary: text(r)?,
                severity: r.read_i32()?,
                pending_intent: pending_intent(r)?,
                icon_action: if present(r)? {
                    Some((r.read_i32()?, pending_intent(r)?))
                } else {
                    None
                },
                enabled: r.read_bool()?,
            })
        } else {
            None
        };
        let issues = typed_list(r, |r| {
            Ok(Issue {
                id: r.read_string16()?,
                title: text(r)?,
                subtitle: text(r)?,
                summary: text(r)?,
                severity: r.read_i32()?,
                category: r.read_i32()?,
                actions: typed_list(r, action)?,
                on_dismiss: pending_intent(r)?,
                type_id: r.read_string16()?,
                notification: if present(r)? {
                    Some(Notification {
                        title: text(r)?,
                        text: text(r)?,
                        actions: typed_list(r, action)?,
                    })
                } else {
                    None
                },
                notification_behavior: r.read_i32()?,
                attribution_title: text(r)?,
                deduplication_id: r.read_string16()?,
                actionability: r.read_i32()?,
            })
        })?;
        // `readBundle`: its length, then its magic, data and whether it
        // holds an intent.
        let start = r.position();
        let length = r.read_i32()?;
        if length > 0 {
            r.read_i32()?;
            r.skip(length as usize)?;
            r.read_bool()?;
        }
        let (bytes, objects) = r.since(start);
        Ok(SourceData {
            status,
            issues,
            extras: (bytes.to_vec(), !objects.is_empty()),
        })
    }
}

impl WriteParcelable for SourceData {
    fn write_to(&self, p: &mut Parcel) {
        match &self.status {
            Some(s) => {
                p.write_i32(1);
                write_text(p, &s.title);
                write_text(p, &s.summary);
                p.write_i32(s.severity);
                write_pending_intent(p, &s.pending_intent);
                match &s.icon_action {
                    Some((kind, target)) => {
                        p.write_i32(1);
                        p.write_i32(*kind);
                        write_pending_intent(p, target);
                    }
                    None => p.write_i32(0),
                }
                p.write_bool(s.enabled);
            }
            None => p.write_i32(0),
        }
        write_typed_list(p, &self.issues, |p, i| {
            p.write_string16(i.id.as_deref());
            write_text(p, &i.title);
            write_text(p, &i.subtitle);
            write_text(p, &i.summary);
            p.write_i32(i.severity);
            p.write_i32(i.category);
            write_typed_list(p, &i.actions, write_action);
            write_pending_intent(p, &i.on_dismiss);
            p.write_string16(i.type_id.as_deref());
            match &i.notification {
                Some(n) => {
                    p.write_i32(1);
                    write_text(p, &n.title);
                    write_text(p, &n.text);
                    write_typed_list(p, &n.actions, write_action);
                }
                None => p.write_i32(0),
            }
            p.write_i32(i.notification_behavior);
            write_text(p, &i.attribution_title);
            p.write_string16(i.deduplication_id.as_deref());
            p.write_i32(i.actionability);
        });
        p.write_raw(&self.extras.0, &[]);
    }
}

impl SourceData {
    /// `Builder.build`: unique issue ids, and no issue requiring attention
    /// more severe than the status.
    pub fn build(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        let mut max = i32::MIN;
        for issue in &self.issues {
            if !ids.insert(&issue.id) {
                return Err("Safety source data cannot have duplicate issue ids".into());
            }
            max = max.max(issue.severity);
        }
        if let Some(status) = &self.status
            && max > SEVERITY_INFORMATION
            && status.severity < max
        {
            return Err(
                "Safety source data cannot have issues that are more severe than its status".into(),
            );
        }
        Ok(())
    }

    /// Whether its extras can be handed back: they hold no binder or file.
    pub fn extras_kept(&self) -> bool {
        !self.extras.1
    }

    /// The handles its intents hold, to keep while it is stored.
    pub fn retain(&self, process: &LocalProcess) -> Vec<Strong> {
        let mut targets: Vec<&Option<Binder>> = Vec::new();
        if let Some(s) = &self.status {
            targets.push(&s.pending_intent);
            if let Some((_, t)) = &s.icon_action {
                targets.push(t);
            }
        }
        for i in &self.issues {
            targets.push(&i.on_dismiss);
            let notification = i.notification.iter().flat_map(|n| &n.actions);
            targets.extend(
                i.actions
                    .iter()
                    .chain(notification)
                    .map(|a| &a.pending_intent),
            );
        }
        targets
            .into_iter()
            .filter_map(|t| match t {
                Some(Binder::Handle(h)) => Some(process.strong(*h)),
                _ => None,
            })
            .collect()
    }
}

/// `SafetyEvent`: its type, refresh broadcast, issue and action ids.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: i32,
    pub refresh_broadcast_id: Option<String>,
    pub issue_id: Option<String>,
    pub action_id: Option<String>,
}

impl ReadParcelable for Event {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Event {
            kind: r.read_i32()?,
            refresh_broadcast_id: r.read_string16()?,
            issue_id: r.read_string16()?,
            action_id: r.read_string16()?,
        })
    }
}

/// `SafetySourceErrorDetails`: its event.
pub struct ErrorDetails(pub Event);

impl ReadParcelable for ErrorDetails {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        if !present(r)? {
            return Err(BAD_VALUE);
        }
        Event::read_from(r).map(ErrorDetails)
    }
}

/// `SafetyCenterFlags` read from DeviceConfig's `privacy` namespace and the
/// resources APK's defaults.
pub struct Flags<'a> {
    pub privacy: &'a dyn Fn(&str) -> Option<String>,
    /// `config_defaultIssueCategoryAllowlist`.
    pub issue_category_allowlist_default: &'a str,
    /// `config_defaultActionsToOverrideWithDefaultIntent`.
    pub actions_to_override_default: &'a str,
    /// The aconfig flag `odad_notifications_supported`.
    pub odad_notifications_supported: bool,
}

impl Flags<'_> {
    fn string(&self, name: &str, default: &str) -> String {
        (self.privacy)(name).unwrap_or_else(|| default.to_string())
    }

    /// `DeviceConfig.getLong`: the default for a value not a long.
    fn long(&self, name: &str, default: i64) -> i64 {
        (self.privacy)(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn boolean(&self, name: &str, default: bool) -> bool {
        (self.privacy)(name).map_or(default, |v| v.eq_ignore_ascii_case("true"))
    }

    fn comma_separated(&self, name: &str) -> HashSet<String> {
        self.string(name, "")
            .split(',')
            .map(str::to_string)
            .collect()
    }

    pub fn notifications_enabled(&self) -> bool {
        self.boolean("safety_center_notifications_enabled", true)
    }

    fn notifications_allowed_sources(&self) -> HashSet<String> {
        let mut sources = self.comma_separated("safety_center_notifications_allowed_sources");
        if self.odad_notifications_supported {
            sources.extend(ODAD_NOTIFICATION_SOURCES.iter().map(|s| s.to_string()));
        }
        sources.extend(U_PLUS_NOTIFICATION_SOURCES.iter().map(|s| s.to_string()));
        sources
    }

    /// `isIssueCategoryAllowedForSource`.
    fn issue_category_allowed(&self, category: i32, source: &str) -> bool {
        let lists = self.string(
            "safety_center_issue_category_allowlists",
            self.issue_category_allowlist_default,
        );
        let list = mapping(&lists, &category.to_string());
        list.is_none_or(|l| l.split('|').any(|s| s == source))
    }

    /// `getActionsToOverrideWithDefaultIntentForSource`.
    pub fn actions_to_override(&self, source: &str) -> Vec<String> {
        let lists = self.string(
            "safety_center_actions_to_override_with_default_intent",
            self.actions_to_override_default,
        );
        mapping(&lists, source)
            .map_or_else(Vec::new, |l| l.split('|').map(str::to_string).collect())
    }

    /// `getAdditionalAllowedPackageCerts`.
    pub fn additional_package_certs(&self, package: &str) -> Vec<String> {
        let lists = self.string("safety_center_additional_allow_package_certs", "");
        mapping(&lists, package)
            .map_or_else(Vec::new, |l| l.split('|').map(str::to_string).collect())
    }

    fn severity_long(&self, name: &str, default: &str, severity: i32) -> Option<i64> {
        mapping(&self.string(name, default), &severity.to_string()).and_then(|v| v.parse().ok())
    }

    fn resurface_max_count(&self, severity: i32) -> i64 {
        self.severity_long(
            "safety_center_resurface_issue_max_counts",
            RESURFACE_ISSUE_MAX_COUNT_DEFAULT,
            severity,
        )
        .unwrap_or(0)
    }

    fn resurface_delay(&self, severity: i32) -> i64 {
        self.severity_long("safety_center_resurface_issue_delays_millis", "", severity)
            .unwrap_or(RESURFACE_ISSUE_DELAY_DEFAULT_MILLIS)
    }

    fn notification_resurface_interval(&self) -> Option<i64> {
        Some(self.long("safety_center_notification_resurface_interval", -1)).filter(|m| *m >= 0)
    }
}

/// `getStringValueFromStringMapping`: the value of `key` in comma
/// separated `key:value` pairs (badly formatted pairs skipped).
fn mapping<'m>(mapping: &'m str, key: &str) -> Option<&'m str> {
    if mapping.is_empty() {
        return None;
    }
    mapping.split(',').find_map(|pair| {
        let mut parts = pair.split(':');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(k), Some(v), None) if k == key => Some(v),
            _ => None,
        }
    })
}

/// `SafetyCenterConfigReader.ExternalSafetySource`: a dynamic or issue
/// only source, its group, and whether that group is stateless.
pub struct External<'c> {
    pub source: &'c SafetySource,
    pub group: &'c SafetySourcesGroup,
}

impl External<'_> {
    fn in_stateless_group(&self) -> bool {
        self.source.kind == TYPE_DYNAMIC && self.group.kind == GROUP_STATELESS
    }
}

/// `getExternalSafetySource` of the configuration (no test override).
pub fn external<'c>(config: &'c SafetyCenterConfig, id: &str) -> Option<External<'c>> {
    config.groups.iter().find_map(|g| {
        g.sources
            .iter()
            .find(|s| s.id == id && s.kind != TYPE_STATIC)
            .map(|s| External {
                source: s,
                group: g,
            })
    })
}

/// `SafetySourceDataValidator.validateRequest` past its calling package's
/// checks: an `IllegalArgumentException`'s message for a request refused.
pub fn validate(
    data: Option<&SourceData>,
    external: &External<'_>,
    flags: &Flags<'_>,
) -> Result<(), String> {
    let source = external.source;
    let id = &source.id;
    let Some(data) = data else { return Ok(()) };
    if source.kind == TYPE_ISSUE_ONLY && data.status.is_some() {
        return Err(format!(
            "Unexpected status for issue only safety source: {id}"
        ));
    }
    if source.kind == TYPE_DYNAMIC
        && source.initial_display_state != DISPLAY_HIDDEN
        && data.status.is_none()
    {
        return Err(format!("Missing status for dynamic safety source: {id}"));
    }
    if let Some(status) = &data.status {
        let level = status.severity;
        if external.in_stateless_group() && level != SEVERITY_UNSPECIFIED {
            return Err(format!(
                "Safety source: {id} is in a stateless group but specified a severity level: {level}"
            ));
        }
        if level > SEVERITY_INFORMATION.max(source.max_severity_level) {
            return Err(format!(
                "Unexpected severity level: {level}, for safety source: {id}"
            ));
        }
    }
    for issue in &data.issues {
        if issue.severity > source.max_severity_level {
            return Err(format!(
                "Unexpected severity level: {}, for issue in safety source: {id}",
                issue.severity
            ));
        }
        if !flags.issue_category_allowed(issue.category, id) {
            return Err(format!(
                "Unexpected issue category: {}, for issue in safety source: {id}",
                issue.category
            ));
        }
    }
    Ok(())
}

/// `SafetyCenterIssueKey`: source, source issue and user.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IssueKey {
    pub source: String,
    pub issue: String,
    pub user: i32,
}

impl IssueKey {
    /// `SafetyCenterIds.encodeToString`: the proto's fields 1 to 3,
    /// URL-safe base64 without wrapping.
    fn encode(&self) -> String {
        let mut b = Vec::new();
        for (tag, s) in [(0x0a, &self.source), (0x12, &self.issue)] {
            b.push(tag);
            varint(&mut b, s.len() as u64);
            b.extend_from_slice(s.as_bytes());
        }
        b.push(0x18);
        varint(&mut b, self.user as i64 as u64);
        aim_android_xml::base64::encode(&b)
            .replace('+', "-")
            .replace('/', "_")
    }

    /// `issueKeyFromString`.
    fn decode(s: &str) -> Option<IssueKey> {
        let b = aim_android_xml::base64::decode(&s.replace('-', "+").replace('_', "/"))?;
        let (mut at, mut key) = (
            0,
            IssueKey {
                source: String::new(),
                issue: String::new(),
                user: 0,
            },
        );
        while at < b.len() {
            let tag = b[at];
            at += 1;
            match tag {
                0x0a | 0x12 => {
                    let len = read_varint(&b, &mut at)? as usize;
                    let s = String::from_utf8(b.get(at..at.checked_add(len)?)?.to_vec()).ok()?;
                    at += len;
                    if tag == 0x0a {
                        key.source = s
                    } else {
                        key.issue = s
                    }
                }
                0x18 => key.user = read_varint(&b, &mut at)? as i32,
                _ => return None,
            }
        }
        Some(key)
    }
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn read_varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*at)?;
        *at += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// `SafetyCenterIssueDismissalRepository.IssueData`.
#[derive(Clone, Debug, PartialEq)]
struct IssueData {
    first_seen_at: i64,
    dismissed_at: Option<i64>,
    dismiss_count: i32,
    notification_dismissed_at: Option<i64>,
    hidden: bool,
    resurface_timer_start: Option<i64>,
}

impl IssueData {
    fn new(first_seen_at: i64) -> Self {
        IssueData {
            first_seen_at,
            dismissed_at: None,
            dismiss_count: 0,
            notification_dismissed_at: None,
            hidden: false,
            resurface_timer_start: None,
        }
    }
}

/// `SafetySourceIssueInfo`: an issue with its source, group and user.
#[derive(Clone, Debug)]
pub struct IssueInfo<'c> {
    pub key: IssueKey,
    pub issue: Issue,
    pub source: &'c SafetySource,
    pub group: &'c SafetySourcesGroup,
}

/// What the data manager keeps: the sources' data and errors, the issues
/// the dismissal repository tracks and each user's deduplicated issues.
#[derive(Default)]
pub struct Data {
    sources: HashMap<(String, i32), (SourceData, Vec<Strong>)>,
    errors: HashSet<(String, i32)>,
    issues: Vec<(IssueKey, IssueData)>,
    /// Each user's deduplicated issues, sorted (`mUserIdToDedupInfo`).
    deduplicated: HashMap<i32, Vec<IssueKey>>,
}

/// `SafetyCenterIssuesPersistence.read`: the persisted issues, an error for
/// a file it refuses.
pub fn read_issues(
    bytes: &[u8],
) -> Result<Vec<(String, i64, Option<i64>, i32, Option<i64>)>, String> {
    let root = aim_android_xml::read(bytes)?;
    if root.name != "issues" {
        return Err("Element issues missing".into());
    }
    let mut version = None;
    for (name, _) in &root.attrs {
        match name.as_str() {
            "version" => version = root.int("version")?,
            other => return Err(format!("Unexpected attribute {other}")),
        }
    }
    let version = version.ok_or("Missing version")?;
    if !(0..=ISSUES_VERSION).contains(&version) {
        return Err(format!("Unsupported version: {version}"));
    }
    let mut out = Vec::new();
    for issue in root.children() {
        if issue.name != "issue" {
            return Err("Element issues not closed".into());
        }
        let (mut key, mut first_seen, mut dismissed, mut count, mut notification) =
            (None, None, None, None, None);
        for (name, _) in &issue.attrs {
            let long = |n: &str| {
                issue
                    .long(n)
                    .map_err(|_| format!("Attribute value for {n} invalid"))
            };
            match name.as_str() {
                "key" => key = issue.string("key").map(|k| k.into_owned()),
                "first_seen_at_epoch_millis" => first_seen = long(name)?,
                "dismissed_at_epoch_millis" => dismissed = long(name)?,
                "dismiss_count" => {
                    count = issue
                        .int(name)
                        .map_err(|_| format!("Attribute value for {name} invalid"))?
                }
                "notification_dismissed_at_epoch_millis" => notification = long(name)?,
                other => return Err(format!("Unexpected attribute {other}")),
            }
        }
        if issue.children().next().is_some() {
            return Err("Element issue not closed".into());
        }
        // `PersistedSafetyCenterIssue.Builder.build`.
        let key = key.ok_or("Element issue invalid")?;
        let first_seen = first_seen.ok_or("Element issue invalid")?;
        let count = match (dismissed, count) {
            (Some(_), None) => 1,
            (_, c) => c.unwrap_or(0),
        };
        if count < 0 {
            return Err("Element issue invalid".into());
        }
        out.push((key, first_seen, dismissed, count, notification));
    }
    Ok(out)
}

/// `SafetyCenterIssuesPersistence.write`'s document, as the indenting
/// serializer writes it.
fn issues_document(issues: &[(IssueKey, IssueData)]) -> String {
    let mut s = String::from("<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>\r\n");
    s.push_str(&format!("<issues version=\"{ISSUES_VERSION}\""));
    if issues.is_empty() {
        s.push_str(" />");
        return s;
    }
    s.push('>');
    for (key, d) in issues {
        s.push_str(&format!(
            "\r\n  <issue key=\"{}\" first_seen_at_epoch_millis=\"{}\"",
            key.encode(),
            d.first_seen_at
        ));
        if let Some(at) = d.dismissed_at {
            s.push_str(&format!(" dismissed_at_epoch_millis=\"{at}\""));
        }
        if d.dismiss_count > 0 {
            s.push_str(&format!(" dismiss_count=\"{}\"", d.dismiss_count));
        }
        if let Some(at) = d.notification_dismissed_at {
            s.push_str(&format!(" notification_dismissed_at_epoch_millis=\"{at}\""));
        }
        s.push_str(" />");
    }
    s.push_str("\r\n</issues>");
    s
}

impl Data {
    /// `loadStateFromFile`'s `load`: only the issues of real external
    /// sources; whether some were dropped.
    pub fn load(
        &mut self,
        persisted: Vec<(String, i64, Option<i64>, i32, Option<i64>)>,
        config: &SafetyCenterConfig,
    ) -> Result<bool, String> {
        self.issues.clear();
        let mut dropped = false;
        for (key, first_seen, dismissed, count, notification) in persisted {
            let key = IssueKey::decode(&key).ok_or_else(|| format!("Invalid issue key: {key}"))?;
            if external(config, &key.source).is_none() {
                dropped = true;
                continue;
            }
            let data = IssueData {
                dismissed_at: dismissed,
                dismiss_count: count,
                notification_dismissed_at: notification,
                ..IssueData::new(first_seen)
            };
            self.issues.push((key, data));
        }
        Ok(dropped)
    }

    /// The issues' file as `writeStateToFile` writes it.
    pub fn issues_file(&self) -> String {
        issues_document(&self.issues)
    }

    fn issue(&self, key: &IssueKey) -> Option<&IssueData> {
        self.issues.iter().find(|(k, _)| k == key).map(|(_, d)| d)
    }

    fn issue_mut(&mut self, key: &IssueKey) -> Option<&mut IssueData> {
        self.issues
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, d)| d)
    }

    pub fn source_data(&self, id: &str, user: i32) -> Option<&SourceData> {
        self.sources.get(&(id.to_string(), user)).map(|(d, _)| d)
    }

    /// `SafetySourceDataRepository.setSafetySourceData`, with the issues it
    /// tracks for dismissal: whether the data changed, and whether the
    /// issues' file should be written.
    pub fn set(
        &mut self,
        id: &str,
        user: i32,
        data: Option<(SourceData, Vec<Strong>)>,
    ) -> (bool, bool) {
        let key = (id.to_string(), user);
        let differs = data.as_ref().map(|(d, _)| d) != self.sources.get(&key).map(|(d, _)| d);
        let removed_error = self.errors.remove(&key);
        let mut write = false;
        if differs {
            let ids: Vec<String> = data
                .iter()
                .flat_map(|(d, _)| d.issues.iter().filter_map(|i| i.id.clone()))
                .collect();
            match data {
                Some(d) => {
                    self.sources.insert(key, d);
                }
                None => {
                    self.sources.remove(&key);
                }
            }
            write = self.update_issues_for_source(&ids, id, user);
        }
        (differs || removed_error, write)
    }

    /// `reportSafetySourceError` of the repository: whether it changed.
    pub fn set_error(&mut self, id: &str, user: i32, event: &Event) -> bool {
        if matches!(
            event.kind,
            EVENT_RESOLVING_ACTION_FAILED | EVENT_RESOLVING_ACTION_SUCCEEDED
        ) {
            return false;
        }
        let key = (id.to_string(), user);
        let removed = self.sources.remove(&key).is_some();
        let added = self.errors.insert(key);
        removed || added
    }

    /// `updateIssuesForSource`: whether the issues changed.
    fn update_issues_for_source(&mut self, ids: &[String], source: &str, user: i32) -> bool {
        let before = self.issues.len();
        self.issues
            .retain(|(k, _)| k.user != user || k.source != source || ids.contains(&k.issue));
        let mut changed = self.issues.len() != before;
        let now = now_millis();
        for id in ids {
            let key = IssueKey {
                source: source.to_string(),
                issue: id.clone(),
                user,
            };
            if self.issue(&key).is_none() {
                self.issues.push((key, IssueData::new(now)));
                changed = true;
            }
        }
        changed
    }

    /// `isIssueDismissed`.
    fn dismissed(&self, key: &IssueKey, severity: i32, flags: &Flags<'_>, now: i64) -> bool {
        let Some(d) = self.issue(key) else {
            return false;
        };
        let Some(at) = d.dismissed_at else {
            return false;
        };
        if i64::from(d.dismiss_count) > flags.resurface_max_count(severity) {
            return true;
        }
        now - at < flags.resurface_delay(severity)
    }

    /// `isNotificationDismissedNow`.
    fn notification_dismissed(
        &self,
        key: &IssueKey,
        severity: i32,
        flags: &Flags<'_>,
        now: i64,
    ) -> bool {
        if self.dismissed(key, severity, flags, now) {
            return true;
        }
        let Some(at) = self.issue(key).and_then(|d| d.notification_dismissed_at) else {
            return false;
        };
        flags
            .notification_resurface_interval()
            .is_none_or(|delay| now < at + delay)
    }

    /// `isIssueHidden`, resurfacing an issue whose timer ran out.
    fn hidden(&mut self, key: &IssueKey, flags: &Flags<'_>, now: i64) -> bool {
        let delay = flags.long(
            "safety_center_temp_hidden_issue_resurface_delay_millis",
            TEMP_HIDDEN_RESURFACE_DELAY_DEFAULT_MILLIS,
        );
        let Some(d) = self.issue_mut(key) else {
            return false;
        };
        if !d.hidden {
            return false;
        }
        let Some(start) = d.resurface_timer_start else {
            return true;
        };
        if now - start >= delay {
            d.hidden = false;
            d.resurface_timer_start = None;
            return false;
        }
        true
    }

    /// `SafetyCenterIssueRepository.updateIssues`: the user's issues of
    /// external sources, by severity, deduplicated
    /// (`SafetyCenterIssueDeduplicator`); whether the issues' file should
    /// be written.
    pub fn update_issues<'c>(
        &mut self,
        user: i32,
        config: &'c SafetyCenterConfig,
        flags: &Flags<'_>,
    ) -> bool {
        let mut issues = self.stored_issues(user, config);
        // `List.sort` is stable.
        issues.sort_by(|a, b| b.issue.severity.cmp(&a.issue.severity));
        let now = now_millis();
        // Buckets of (deduplication group, deduplication id), each sorted.
        let mut buckets: Vec<((String, String), Vec<&IssueInfo<'c>>)> = Vec::new();
        for info in &issues {
            let (Some(group), Some(id)) = (
                &info.source.deduplication_group,
                &info.issue.deduplication_id,
            ) else {
                continue;
            };
            let key = (group.clone(), id.clone());
            match buckets.iter_mut().find(|(k, _)| *k == key) {
                Some((_, bucket)) => bucket.push(info),
                None => buckets.push((key, vec![info])),
            }
        }
        let mut write = false;
        let mut filtered_out = HashSet::new();
        // `alignAllDismissals`.
        for (_, bucket) in buckets.iter().filter(|(_, b)| b.len() >= 2) {
            let top_dismissed = bucket
                .iter()
                .find(|i| self.dismissed(&i.key, i.issue.severity, flags, now));
            if let Some(top) = top_dismissed {
                for r in recipients(top, bucket) {
                    let from = self.issue(&top.key).cloned();
                    if let (Some(from), Some(to)) = (from, self.issue_mut(&r)) {
                        (to.dismissed_at, to.dismiss_count) =
                            (from.dismissed_at, from.dismiss_count);
                        write = true;
                    }
                }
            }
            let top_notification = bucket
                .iter()
                .find(|i| self.notification_dismissed(&i.key, i.issue.severity, flags, now));
            if let Some(top) = top_notification {
                for r in recipients(top, bucket) {
                    let from = self.issue(&top.key).cloned();
                    if let (Some(from), Some(to)) = (from, self.issue_mut(&r)) {
                        to.notification_dismissed_at = from.notification_dismissed_at;
                        write = true;
                    }
                }
            }
        }
        for (_, bucket) in &buckets {
            filtered_out.extend(bucket.iter().skip(1).map(|i| i.key.clone()));
        }
        // `resurfaceHiddenIssuesIfNeeded`.
        for (_, bucket) in &buckets {
            let top = &bucket[0].key;
            if self.hidden(top, flags, now)
                && let Some(d) = self.issue_mut(top)
                && d.resurface_timer_start.is_none()
            {
                d.resurface_timer_start = Some(now);
            }
        }
        let mut deduplicated = Vec::new();
        for info in &issues {
            if filtered_out.contains(&info.key) {
                // `hideIssue`.
                if let Some(d) = self.issue_mut(&info.key) {
                    d.hidden = true;
                    d.resurface_timer_start = None;
                }
            } else {
                deduplicated.push(info.key.clone());
            }
        }
        self.deduplicated.insert(user, deduplicated);
        write
    }

    /// `getAllStoredIssuesFromRawSourceData` for the primary profile.
    fn stored_issues<'c>(&self, user: i32, config: &'c SafetyCenterConfig) -> Vec<IssueInfo<'c>> {
        let mut out = Vec::new();
        for group in &config.groups {
            for source in group.sources.iter().filter(|s| s.kind != TYPE_STATIC) {
                let Some(data) = self.source_data(&source.id, user) else {
                    continue;
                };
                for issue in &data.issues {
                    let key = IssueKey {
                        source: source.id.clone(),
                        issue: issue.id.clone().unwrap_or_default(),
                        user,
                    };
                    out.push(IssueInfo {
                        key,
                        issue: issue.clone(),
                        source,
                        group,
                    });
                }
            }
        }
        out
    }

    /// `SafetyCenterNotificationSender.getIssuesToNotify`: the user's
    /// deduplicated, not hidden issues notifications are allowed for, not
    /// dismissed, and due.
    pub fn issues_to_notify<'c>(
        &mut self,
        user: i32,
        config: &'c SafetyCenterConfig,
        flags: &Flags<'_>,
    ) -> Vec<IssueInfo<'c>> {
        let now = now_millis();
        let all = self.stored_issues(user, config);
        let keys = self.deduplicated.get(&user).cloned().unwrap_or_default();
        let allowed = flags.notifications_allowed_sources();
        let immediate =
            flags.comma_separated("safety_center_notifications_immediate_behavior_issues");
        let min_delay = flags.long(
            "safety_center_notifications_min_delay",
            NOTIFICATIONS_MIN_DELAY_DEFAULT_MILLIS,
        );
        let mut out = Vec::new();
        for key in keys {
            if self.hidden(&key, flags, now) {
                continue;
            }
            let Some(info) = all.iter().find(|i| i.key == key) else {
                continue;
            };
            if !info.source.notifications_allowed && !allowed.contains(&info.source.id) {
                continue;
            }
            if self.notification_dismissed(&key, info.issue.severity, flags, now) {
                continue;
            }
            let behavior = match info.issue.notification_behavior {
                BEHAVIOR_UNSPECIFIED => {
                    let flag = format!(
                        "{}/{}",
                        key.source,
                        info.issue.type_id.as_deref().unwrap_or("null")
                    );
                    if immediate.contains(&flag) {
                        BEHAVIOR_IMMEDIATELY
                    } else {
                        0
                    }
                }
                b => b,
            };
            let due = match behavior {
                BEHAVIOR_IMMEDIATELY => true,
                BEHAVIOR_DELAYED => self
                    .issue(&key)
                    .is_some_and(|d| d.first_seen_at < now - min_delay),
                _ => false,
            };
            if due {
                out.push(info.clone());
            }
        }
        out
    }
}

/// `getRecipientKeys`: the other issues of the bucket no more severe than
/// `top`.
fn recipients(top: &IssueInfo<'_>, bucket: &[&IssueInfo<'_>]) -> Vec<IssueKey> {
    bucket
        .iter()
        .filter(|i| i.key != top.key && i.issue.severity <= top.issue.severity)
        .map(|i| i.key.clone())
        .collect()
}

/// `SafetySources.supportsProfileType` of the primary profile, and of
/// others.
pub fn supports_profile(source: &SafetySource, primary: bool) -> bool {
    primary || source.profile == PROFILE_ALL
}

/// Writes the issues' file under `data` as the system's, through a new
/// file (`AtomicFile`).
pub fn write_issues_file(data: &Path, document: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = data.join(ISSUES_FILE);
    let new = data.join(format!("{ISSUES_FILE}.new"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, document.as_bytes())?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        },
    )?;
    std::fs::rename(&new, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, severity: i32) -> Issue {
        Issue {
            id: Some(id.into()),
            title: Some("T".into()),
            subtitle: None,
            summary: Some("S".into()),
            severity,
            category: 100,
            actions: vec![Action {
                id: Some("a".into()),
                label: Some("Fix".into()),
                pending_intent: Some(Binder::Local(0x40)),
                will_resolve: true,
                success_message: Some("Done".into()),
                confirmation: Some([
                    Some("T".into()),
                    Some("X".into()),
                    Some("Yes".into()),
                    Some("No".into()),
                ]),
            }],
            on_dismiss: None,
            type_id: Some("type".into()),
            notification: Some(Notification {
                title: Some("N".into()),
                text: Some("Body".into()),
                actions: vec![],
            }),
            notification_behavior: 0,
            attribution_title: None,
            deduplication_id: None,
            actionability: 0,
        }
    }

    fn data(issues: Vec<Issue>) -> SourceData {
        let status = Status {
            title: Some("Title".into()),
            summary: Some("Summary".into()),
            severity: 300,
            pending_intent: Some(Binder::Local(0x50)),
            icon_action: Some((100, Some(Binder::Local(0x60)))),
            enabled: true,
        };
        SourceData {
            status: Some(status),
            issues,
            extras: (vec![0, 0, 0, 0], false),
        }
    }

    /// `SafetySourceData.writeToParcel`, read back by its creator to the
    /// same data, its extras and the bytes after it kept.
    #[test]
    fn source_data_round_trips_as_written() {
        let d = data(vec![issue("i", 300)]);
        let mut p = Parcel::new();
        d.write_to(&mut p);
        crate::bundle::write(&mut p, &[]);
        p.write_i32(42);
        let mut r = p.reader();
        let read = SourceData::read_from(&mut r).unwrap();
        assert_eq!(read, d);
        assert!(read.extras_kept());
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), 42);
    }

    /// `Builder.build`: issue ids are unique, and no issue that needs
    /// attention is more severe than the status.
    #[test]
    fn data_is_built_as_the_builder_builds_it() {
        assert!(data(vec![issue("i", 300)]).build().is_ok());
        assert!(
            data(vec![issue("i", 300), issue("i", 200)])
                .build()
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(
            data(vec![issue("i", 400)])
                .build()
                .unwrap_err()
                .contains("more severe")
        );
    }

    fn source(id: &str, kind: i32) -> SafetySource {
        SafetySource {
            kind,
            id: id.into(),
            package_name: Some("com.example".into()),
            title: 1,
            title_for_work: 0,
            summary: 1,
            intent_action: None,
            profile: 1,
            initial_display_state: 0,
            max_severity_level: 300,
            search_terms: 0,
            logging_allowed: true,
            refresh_on_page_open_allowed: false,
            notifications_allowed: true,
            deduplication_group: Some("g".into()),
            package_certificate_hashes: vec![],
            title_for_private_profile: 0,
        }
    }

    fn config() -> SafetyCenterConfig {
        let group = |kind, sources| SafetySourcesGroup {
            kind,
            id: "group".into(),
            title: 0,
            summary: 0,
            stateless_icon_type: 0,
            sources,
        };
        SafetyCenterConfig {
            groups: vec![
                group(
                    0,
                    vec![source("A", TYPE_DYNAMIC), source("B", TYPE_DYNAMIC)],
                ),
                group(GROUP_STATELESS, vec![source("C", TYPE_DYNAMIC)]),
            ],
        }
    }

    fn no_flags(_: &str) -> Option<String> {
        None
    }

    fn flags<'a>(privacy: &'a dyn Fn(&str) -> Option<String>) -> Flags<'a> {
        Flags {
            privacy,
            issue_category_allowlist_default: "",
            actions_to_override_default: "",
            odad_notifications_supported: true,
        }
    }

    /// `validateRequest`'s checks of the data against its source.
    #[test]
    fn data_is_validated_against_its_source() {
        let config = config();
        let f = flags(&no_flags);
        assert!(
            validate(
                Some(&data(vec![issue("i", 300)])),
                &external(&config, "A").unwrap(),
                &f
            )
            .is_ok()
        );
        let critical = validate(
            Some(&data(vec![issue("i", 400)])),
            &external(&config, "A").unwrap(),
            &f,
        );
        assert_eq!(
            critical.unwrap_err(),
            "Unexpected severity level: 400, for issue in safety source: A"
        );
        let stateless = validate(Some(&data(vec![])), &external(&config, "C").unwrap(), &f);
        assert_eq!(
            stateless.unwrap_err(),
            "Safety source: C is in a stateless group but specified a severity level: 300"
        );
        let mut missing = data(vec![]);
        missing.status = None;
        assert_eq!(
            validate(Some(&missing), &external(&config, "A").unwrap(), &f).unwrap_err(),
            "Missing status for dynamic safety source: A"
        );
        let allowlists = |n: &str| {
            (n == "safety_center_issue_category_allowlists").then(|| "100:B|C".to_string())
        };
        let f = flags(&allowlists);
        let refused = validate(
            Some(&data(vec![issue("i", 300)])),
            &external(&config, "A").unwrap(),
            &f,
        );
        assert_eq!(
            refused.unwrap_err(),
            "Unexpected issue category: 100, for issue in safety source: A"
        );
    }

    /// The repository: a change and its tracked issues, the same data
    /// again unchanged, an error clearing the data; duplicates hidden but
    /// the top issue notified.
    #[test]
    fn data_is_kept_and_issues_tracked_and_deduplicated() {
        let config = config();
        let f = flags(&no_flags);
        let mut d = Data::default();
        let mut a = issue("i", 300);
        a.deduplication_id = Some("dup".into());
        a.notification_behavior = BEHAVIOR_IMMEDIATELY;
        let mut b = a.clone();
        b.severity = 200;
        assert_eq!(
            d.set("A", 0, Some((data(vec![a.clone()]), vec![]))),
            (true, true)
        );
        assert_eq!(d.set("A", 0, Some((data(vec![a]), vec![]))), (false, false));
        assert_eq!(d.set("B", 0, Some((data(vec![b]), vec![]))), (true, true));
        d.update_issues(0, &config, &f);
        let notified: Vec<String> = d
            .issues_to_notify(0, &config, &f)
            .iter()
            .map(|i| i.key.source.clone())
            .collect();
        assert_eq!(notified, ["A"]);
        assert!(d.set_error(
            "A",
            0,
            &Event {
                kind: 100,
                refresh_broadcast_id: None,
                issue_id: None,
                action_id: None
            }
        ));
        assert!(d.source_data("A", 0).is_none());
        // Clearing it clears the error alone: the data is already none.
        assert_eq!(d.set("A", 0, None), (true, false));
    }

    /// The issues' file as the original wrote it: read, its keys decoded,
    /// and written back to the same document.
    #[test]
    fn issues_file_round_trips() {
        let file = "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>\r\n<issues version=\"2\">\r\n  <issue key=\"ChJHb29nbGVGaW5kTXlEZXZpY2USFE5vR29vZ2xlQWNjb3VudElzc3VlGAA=\" first_seen_at_epoch_millis=\"1792319637127\" />\r\n</issues>";
        let issues = read_issues(file.as_bytes()).unwrap();
        let key = IssueKey::decode(&issues[0].0).unwrap();
        assert_eq!(
            key,
            IssueKey {
                source: "GoogleFindMyDevice".into(),
                issue: "NoGoogleAccountIssue".into(),
                user: 0
            }
        );
        let mut config = config();
        config.groups[0].sources[0].id = "GoogleFindMyDevice".into();
        let mut d = Data::default();
        assert!(!d.load(issues, &config).unwrap());
        assert_eq!(d.issues_file(), file);
        assert_eq!(
            Data::default().issues_file(),
            "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>\r\n<issues version=\"2\" />"
        );
    }
}
